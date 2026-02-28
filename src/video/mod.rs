pub mod frame;
pub mod vp8_encode;
pub mod vp8_decode;
pub mod fragment;
pub mod capture;
pub mod display;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ringbuf::traits::{Consumer, Split};
use ringbuf::HeapRb;
use tokio::net::UdpSocket;
use tokio::runtime::Handle;

use crate::net::protocol::{Packet, PacketHeader, PacketType};
use crate::net::session::SharedSessionState;
use crate::net::socket::InboundEvent;

use capture::CameraCapture;
use display::VideoDisplay;
use frame::{downscale_rgb, i420_to_rgb, rgb_to_i420, VideoFrame};
use fragment::{fragment_payload, FragmentAssembler};
use vp8_decode::Vp8Decoder;
use vp8_encode::Vp8Encoder;

/// Target encode resolution (480p, 16:9)
const ENCODE_WIDTH: u32 = 854;
const ENCODE_HEIGHT: u32 = 480;

/// The video pipeline: camera capture → VP8 encode → UDP send,
/// and inbound VP8 decode → display frames.
pub struct VideoPipeline {
    _capture: Option<CameraCapture>,
    encode_thread: Option<JoinHandle<()>>,
    encode_stop: Arc<AtomicBool>,
    decode_stop: Option<tokio::sync::watch::Sender<bool>>,
    camera_enabled: Arc<AtomicBool>,
    /// Latest local camera frame (downscaled to 480p) for preview.
    pub local_frame: Arc<Mutex<Option<VideoFrame>>>,
    /// Latest decoded remote frames, keyed by participant_id.
    pub remote_frames: Arc<Mutex<HashMap<u8, VideoFrame>>>,
    /// Texture manager for egui rendering.
    pub display: VideoDisplay,
}

impl VideoPipeline {
    /// Create and start the video pipeline.
    ///
    /// - `camera_enabled`: whether to start capturing from the camera
    /// - `state`: shared session state for peer info
    /// - `socket`: UDP socket for sending video packets
    /// - `handle`: tokio runtime handle for async sends
    /// - `video_rx`: channel receiving inbound video events from the network
    pub fn new(
        camera_enabled: bool,
        state: SharedSessionState,
        socket: Arc<UdpSocket>,
        handle: Handle,
        video_rx: tokio::sync::mpsc::UnboundedReceiver<InboundEvent>,
    ) -> Result<Self, String> {
        let camera_flag = Arc::new(AtomicBool::new(camera_enabled));
        let local_frame: Arc<Mutex<Option<VideoFrame>>> = Arc::new(Mutex::new(None));
        let remote_frames: Arc<Mutex<HashMap<u8, VideoFrame>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Ring buffer: capture thread → encode thread (4 frames)
        let ring = HeapRb::<VideoFrame>::new(4);
        let (producer, consumer) = ring.split();

        // Start camera capture (pushes frames to ring)
        let capture = if camera_enabled {
            match CameraCapture::start(producer) {
                Ok(c) => {
                    log::info!("Camera capture started");
                    Some(c)
                }
                Err(e) => {
                    log::warn!("Camera capture failed: {e}");
                    None
                }
            }
        } else {
            log::info!("Camera disabled, no capture");
            None
        };

        // Spawn VP8 encode thread
        let encode_stop = Arc::new(AtomicBool::new(false));
        let encode_thread = Self::spawn_encode_thread(
            encode_stop.clone(),
            camera_flag.clone(),
            consumer,
            local_frame.clone(),
            state,
            socket,
            handle.clone(),
        )?;

        // Spawn video decode task (tokio)
        let (decode_stop_tx, decode_stop_rx) = tokio::sync::watch::channel(false);
        Self::spawn_decode_task(
            handle,
            decode_stop_rx,
            video_rx,
            remote_frames.clone(),
        );

        Ok(Self {
            _capture: capture,
            encode_thread: Some(encode_thread),
            encode_stop,
            decode_stop: Some(decode_stop_tx),
            camera_enabled: camera_flag,
            local_frame,
            remote_frames,
            display: VideoDisplay::new(),
        })
    }

    /// Toggle camera on/off during a call.
    pub fn set_camera_enabled(&self, enabled: bool) {
        self.camera_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_camera_enabled(&self) -> bool {
        self.camera_enabled.load(Ordering::Relaxed)
    }

    fn spawn_encode_thread(
        stop: Arc<AtomicBool>,
        camera_enabled: Arc<AtomicBool>,
        mut consumer: ringbuf::HeapCons<VideoFrame>,
        local_frame: Arc<Mutex<Option<VideoFrame>>>,
        state: SharedSessionState,
        socket: Arc<UdpSocket>,
        handle: Handle,
    ) -> Result<JoinHandle<()>, String> {
        thread::Builder::new()
            .name("video-encode".into())
            .spawn(move || {
                let mut encoder = match Vp8Encoder::new(ENCODE_WIDTH, ENCODE_HEIGHT) {
                    Ok(e) => e,
                    Err(e) => {
                        log::error!("Failed to create VP8 encoder: {e}");
                        return;
                    }
                };

                let mut video_seq: u16 = 0;
                let mut frame_count: i64 = 0;

                while !stop.load(Ordering::Relaxed) {
                    // Skip encoding if camera is disabled
                    if !camera_enabled.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(33));
                        continue;
                    }

                    match consumer.try_pop() {
                        Some(raw_frame) => {
                            // Downscale to encode resolution
                            let scaled =
                                downscale_rgb(&raw_frame, ENCODE_WIDTH, ENCODE_HEIGHT);

                            // Convert to I420 for VP8 encoder
                            let i420 =
                                rgb_to_i420(&scaled.data, scaled.width, scaled.height);

                            // Store downscaled frame for local preview
                            if let Ok(mut lf) = local_frame.lock() {
                                *lf = Some(scaled);
                            }

                            // VP8 encode
                            let pts = frame_count;
                            frame_count += 1;

                            match encoder.encode(&i420, pts) {
                                Ok(packets) => {
                                    for pkt in packets {
                                        Self::send_video_packet(
                                            &pkt,
                                            &state,
                                            &socket,
                                            &handle,
                                            &mut video_seq,
                                        );
                                    }
                                }
                                Err(e) => log::warn!("VP8 encode error: {e}"),
                            }
                        }
                        None => {
                            thread::sleep(Duration::from_millis(5));
                        }
                    }
                }

                // Flush remaining encoder packets
                match encoder.finish() {
                    Ok(remaining) => {
                        for pkt in remaining {
                            Self::send_video_packet(
                                &pkt, &state, &socket, &handle, &mut video_seq,
                            );
                        }
                    }
                    Err(e) => log::debug!("VP8 finish error: {e}"),
                }

                log::info!("Video encode thread stopped");
            })
            .map_err(|e| format!("Failed to spawn video encode thread: {e}"))
    }

    fn send_video_packet(
        pkt: &vp8_encode::EncodedFrame,
        state: &SharedSessionState,
        socket: &Arc<UdpSocket>,
        handle: &Handle,
        video_seq: &mut u16,
    ) {
        let (my_id, ts, peer_addrs) = {
            let s = state.lock().unwrap();
            (s.my_participant_id, s.elapsed_ms(), s.connected_peer_addrs())
        };

        if peer_addrs.is_empty() {
            return;
        }

        let packet_type = if pkt.is_keyframe {
            PacketType::VideoKeyframe
        } else {
            PacketType::VideoDelta
        };

        let fragments = fragment_payload(&pkt.data);

        for (frag_id, frag_total, frag_data) in &fragments {
            let seq = *video_seq;
            *video_seq = video_seq.wrapping_add(1);

            let mut header = PacketHeader::new(
                packet_type,
                my_id,
                seq,
                ts,
                frag_data.len() as u16,
            );
            header.fragment_id = *frag_id;
            header.fragment_total = *frag_total;

            let packet_bytes = Packet::new(header, frag_data.clone()).to_bytes();

            for addr in &peer_addrs {
                let bytes = packet_bytes.clone();
                let sock = socket.clone();
                let target = *addr;
                let _ = handle.block_on(async move {
                    sock.send_to(&bytes, target).await
                });
            }
        }
    }

    fn spawn_decode_task(
        handle: Handle,
        mut stop_rx: tokio::sync::watch::Receiver<bool>,
        mut video_rx: tokio::sync::mpsc::UnboundedReceiver<InboundEvent>,
        remote_frames: Arc<Mutex<HashMap<u8, VideoFrame>>>,
    ) {
        handle.spawn(async move {
            let mut assembler = FragmentAssembler::new();
            let mut decoders: HashMap<u8, Vp8Decoder> = HashMap::new();
            let mut last_expire = tokio::time::Instant::now();

            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    event = video_rx.recv() => {
                        let Some(event) = event else { break };
                        if let InboundEvent::Video {
                            participant_id,
                            timestamp_ms,
                            is_keyframe,
                            fragment_id,
                            fragment_total,
                            payload,
                            ..
                        } = event {
                            if let Some(reassembled) = assembler.push(
                                participant_id,
                                timestamp_ms,
                                fragment_id,
                                fragment_total,
                                &payload,
                                is_keyframe,
                            ) {
                                // Get or create decoder for this peer
                                let decoder = decoders
                                    .entry(reassembled.participant_id)
                                    .or_insert_with(|| {
                                        Vp8Decoder::new().expect("VP8 decoder init failed")
                                    });

                                match decoder.decode(&reassembled.data) {
                                    Ok(Some(decoded)) => {
                                        let rgb = i420_to_rgb(
                                            &decoded.data,
                                            decoded.width,
                                            decoded.height,
                                        );
                                        let frame = VideoFrame {
                                            data: rgb,
                                            width: decoded.width,
                                            height: decoded.height,
                                        };
                                        if let Ok(mut rf) = remote_frames.lock() {
                                            rf.insert(
                                                reassembled.participant_id,
                                                frame,
                                            );
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        log::debug!(
                                            "VP8 decode error for peer {}: {e}",
                                            reassembled.participant_id
                                        );
                                    }
                                }
                            }

                            // Periodically expire stale fragments
                            if last_expire.elapsed() > Duration::from_millis(500) {
                                assembler.expire_stale(Duration::from_millis(200));
                                last_expire = tokio::time::Instant::now();
                            }
                        }
                    }
                }
            }

            log::info!("Video decode task stopped");
        });
    }
}

impl Drop for VideoPipeline {
    fn drop(&mut self) {
        // Stop decode task
        if let Some(stop) = self.decode_stop.take() {
            let _ = stop.send(true);
        }

        // Stop encode thread
        self.encode_stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.encode_thread.take() {
            let _ = t.join();
        }

        // CameraCapture is dropped automatically (its Drop stops the thread)

        log::info!("VideoPipeline dropped");
    }
}
