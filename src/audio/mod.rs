pub mod device;
pub mod rt_priority;
pub mod codec;
pub mod jitter;
pub mod aac;
pub mod fmp4;
pub mod recorder;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleRate, Stream, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use tokio::net::UdpSocket;
use tokio::runtime::Handle;

use crate::net::protocol::{Packet, PacketHeader, PacketType};
use crate::net::session::SharedSessionState;
use codec::OPUS_FRAME_SAMPLES;
use jitter::JitterBuffer;
use recorder::AudioRecorder;

const SAMPLE_RATE: u32 = 48_000;

/// The networked audio pipeline with optional local recording.
///
/// Input: cpal callback → Ring A → encode thread → Opus encode → UDP send
///                       → Ring B → recorder thread → AAC encode → fMP4 file  [when recording]
/// Output: JitterBuffer → refill thread → playback ring → cpal callback
pub struct AudioPipeline {
    _input_stream: Stream,
    _output_stream: Stream,
    encode_thread: Option<JoinHandle<()>>,
    encode_stop: Arc<AtomicBool>,
    refill_thread: Option<JoinHandle<()>>,
    refill_stop: Arc<AtomicBool>,
    recorder: Option<AudioRecorder>,
}

impl AudioPipeline {
    pub fn new(
        state: SharedSessionState,
        socket: Arc<UdpSocket>,
        handle: Handle,
        jitter: Arc<Mutex<JitterBuffer>>,
        recording_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        device::log_all_devices();

        let input_dev = device::default_input()?;
        let output_dev = device::default_output()?;

        let in_name = input_dev.name().unwrap_or_else(|_| "<unknown>".into());
        let out_name = output_dev.name().unwrap_or_else(|_| "<unknown>".into());
        log::info!("AudioPipeline input:  {in_name}");
        log::info!("AudioPipeline output: {out_name}");

        let in_config = input_dev
            .default_input_config()
            .map_err(|e| format!("No default input config: {e}"))?;
        let out_config = output_dev
            .default_output_config()
            .map_err(|e| format!("No default output config: {e}"))?;

        let in_channels = in_config.channels() as usize;
        let out_channels = out_config.channels() as usize;

        let stream_config = StreamConfig {
            channels: in_channels as u16,
            sample_rate: SampleRate(SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        let out_stream_config = StreamConfig {
            channels: out_channels as u16,
            sample_rate: SampleRate(SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        // --- Ring A: input callback → encode thread (Opus) ---
        let capture_ring_size = (SAMPLE_RATE as usize) * 200 / 1000;
        let capture_ring = HeapRb::<f32>::new(capture_ring_size);
        let (mut capture_prod_a, mut capture_cons) = capture_ring.split();

        // --- Ring B: input callback → recorder thread (AAC) [optional] ---
        let (mut rec_prod_opt, rec_cons_opt) = if recording_path.is_some() {
            let ring_b = HeapRb::<f32>::new(capture_ring_size);
            let (prod, cons) = ring_b.split();
            (Some(prod), Some(cons))
        } else {
            (None, None)
        };

        // --- Playback ring: refill thread → output callback ---
        let playback_ring_size = (SAMPLE_RATE as usize) * 200 / 1000;
        let playback_ring = HeapRb::<f32>::new(playback_ring_size);
        let (mut playback_prod, mut playback_cons) = playback_ring.split();

        // Pre-fill playback ring with ~10ms of silence
        let prefill = (SAMPLE_RATE as usize) * 10 / 1000;
        for _ in 0..prefill {
            let _ = playback_prod.try_push(0.0);
        }

        // RT priority guards for cpal callbacks
        let in_rt_done = Arc::new(AtomicBool::new(false));
        let out_rt_done = Arc::new(AtomicBool::new(false));
        let in_rt = in_rt_done.clone();
        let out_rt = out_rt_done.clone();

        // --- Input stream: fan out mono samples to Ring A (and Ring B if recording) ---
        let input_stream = input_dev
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    rt_priority::promote_once(&in_rt, "input");
                    for frame in data.chunks(in_channels) {
                        let sample = frame[0];
                        let _ = capture_prod_a.try_push(sample);
                        if let Some(ref mut prod_b) = rec_prod_opt {
                            let _ = prod_b.try_push(sample);
                        }
                    }
                },
                |err| log::error!("Input stream error: {err}"),
                None,
            )
            .map_err(|e| format!("Failed to build input stream: {e}"))?;

        // --- Output stream: pull from playback ring ---
        let output_stream = output_dev
            .build_output_stream(
                &out_stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    rt_priority::promote_once(&out_rt, "output");
                    for frame in data.chunks_mut(out_channels) {
                        let sample = playback_cons.try_pop().unwrap_or(0.0);
                        for ch in frame.iter_mut() {
                            *ch = sample;
                        }
                    }
                },
                |err| log::error!("Output stream error: {err}"),
                None,
            )
            .map_err(|e| format!("Failed to build output stream: {e}"))?;

        input_stream
            .play()
            .map_err(|e| format!("Failed to start input stream: {e}"))?;
        output_stream
            .play()
            .map_err(|e| format!("Failed to start output stream: {e}"))?;

        // --- Start recorder thread (if recording) ---
        let recorder = match (rec_cons_opt, recording_path) {
            (Some(cons), Some(path)) => Some(AudioRecorder::start(cons, path)?),
            _ => None,
        };

        // --- Encode thread: raw OS thread with RT priority ---
        let encode_stop = Arc::new(AtomicBool::new(false));
        let stop_flag = encode_stop.clone();
        let encode_state = state.clone();
        let encode_socket = socket.clone();
        let encode_handle = handle.clone();

        let encode_thread = thread::Builder::new()
            .name("audio-encode".into())
            .spawn(move || {
                // Promote to real-time priority
                match rt_priority::promote_current_thread() {
                    Ok(()) => log::info!("Encode thread promoted to RT priority"),
                    Err(e) => log::warn!("Encode thread RT promotion failed: {e}"),
                }

                let mut encoder = match codec::create_encoder() {
                    Ok(e) => e,
                    Err(e) => {
                        log::error!("Failed to create Opus encoder: {e}");
                        return;
                    }
                };

                let mut accumulator = Vec::with_capacity(OPUS_FRAME_SAMPLES);

                while !stop_flag.load(Ordering::Relaxed) {
                    // Try to pull samples from capture ring
                    match capture_cons.try_pop() {
                        Some(sample) => {
                            accumulator.push(sample);

                            if accumulator.len() == OPUS_FRAME_SAMPLES {
                                // Encode the frame
                                let mut frame = [0.0f32; OPUS_FRAME_SAMPLES];
                                frame.copy_from_slice(&accumulator);
                                accumulator.clear();

                                let encoded = match codec::encode_frame(&mut encoder, &frame) {
                                    Ok(data) => data,
                                    Err(e) => {
                                        log::warn!("Opus encode error: {e}");
                                        continue;
                                    }
                                };

                                // Build packet and send to all peers
                                let (my_id, seq, ts, peer_addrs) = {
                                    let mut s = encode_state.lock().unwrap();
                                    let seq = s.next_seq();
                                    let ts = s.elapsed_ms();
                                    let addrs = s.connected_peer_addrs();
                                    (s.my_participant_id, seq, ts, addrs)
                                };

                                if !peer_addrs.is_empty() {
                                    let header = PacketHeader::new(
                                        PacketType::Audio,
                                        my_id,
                                        seq,
                                        ts,
                                        encoded.len() as u16,
                                    );
                                    let packet_bytes = Packet::new(header, encoded).to_bytes();

                                    for addr in &peer_addrs {
                                        let bytes = packet_bytes.clone();
                                        let sock = encode_socket.clone();
                                        let target = *addr;
                                        // Use block_on for quick UDP send
                                        let _ = encode_handle.block_on(async move {
                                            sock.send_to(&bytes, target).await
                                        });
                                    }
                                }
                            }
                        }
                        None => {
                            // No samples available, sleep briefly
                            thread::sleep(std::time::Duration::from_micros(500));
                        }
                    }
                }
                log::info!("Encode thread stopped");
            })
            .map_err(|e| format!("Failed to spawn encode thread: {e}"))?;

        // --- Refill thread: jitter buffer → playback ring ---
        let refill_stop = Arc::new(AtomicBool::new(false));
        let refill_stop_flag = refill_stop.clone();

        let refill_thread = thread::Builder::new()
            .name("audio-refill".into())
            .spawn(move || {
                log::info!("Refill thread started");
                while !refill_stop_flag.load(Ordering::Relaxed) {
                    // Check if playback ring needs more data
                    // Pull a frame from jitter buffer and push samples to playback ring
                    let frame = {
                        match jitter.lock() {
                            Ok(mut jb) => jb.pull(),
                            Err(_) => [0.0; OPUS_FRAME_SAMPLES],
                        }
                    };

                    for &sample in &frame {
                        // Spin briefly if ring is full
                        let mut attempts = 0;
                        while playback_prod.try_push(sample).is_err() {
                            attempts += 1;
                            if attempts > 100 {
                                break; // Drop sample rather than spin forever
                            }
                            thread::yield_now();
                        }
                    }

                    // Sleep for roughly one frame duration (5ms)
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                log::info!("Refill thread stopped");
            })
            .map_err(|e| format!("Failed to spawn refill thread: {e}"))?;

        log::info!("AudioPipeline running (recording={})", recorder.is_some());

        Ok(Self {
            _input_stream: input_stream,
            _output_stream: output_stream,
            encode_thread: Some(encode_thread),
            encode_stop,
            refill_thread: Some(refill_thread),
            refill_stop,
            recorder,
        })
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        // Stop recorder FIRST so it can drain Ring B
        if let Some(rec) = self.recorder.take() {
            rec.request_stop();
            drop(rec); // joins the thread
        }

        self.encode_stop.store(true, Ordering::Relaxed);
        self.refill_stop.store(true, Ordering::Relaxed);

        if let Some(t) = self.encode_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.refill_thread.take() {
            let _ = t.join();
        }
        log::info!("AudioPipeline dropped");
    }
}
