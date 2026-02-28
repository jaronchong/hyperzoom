use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use ringbuf::traits::Consumer;

use super::aac::{self, AacEncoder, AAC_FRAME_SAMPLES};
use super::fmp4::FragmentedMp4Writer;

/// How many AAC frames per fragment (~1 second at 48kHz).
const FRAMES_PER_FRAGMENT: usize = 47;

/// BufWriter capacity for the recording file.
const BUF_WRITER_SIZE: usize = 64 * 1024;

type RingConsumer = ringbuf::HeapCons<f32>;

/// Manages the recorder thread that consumes from Ring B, encodes AAC,
/// and writes fragmented MP4 to disk.
pub struct AudioRecorder {
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl AudioRecorder {
    /// Start the recorder thread.
    /// `consumer` is the Ring B consumer end.
    /// `path` is the full path to the output .mp4 file.
    pub fn start(consumer: RingConsumer, path: PathBuf) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();

        let path_display = path.display().to_string();
        let thread = thread::Builder::new()
            .name("audio-recorder".into())
            .spawn(move || {
                if let Err(e) = recorder_loop(consumer, &path, &stop_flag) {
                    log::error!("Recorder thread error: {e}");
                }
            })
            .map_err(|e| format!("Failed to spawn recorder thread: {e}"))?;

        log::info!("AudioRecorder started: {}", path_display);

        Ok(Self {
            thread: Some(thread),
            stop,
        })
    }

    /// Signal the recorder thread to stop. Does not block.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        log::info!("AudioRecorder dropped");
    }
}

fn recorder_loop(
    mut consumer: RingConsumer,
    path: &PathBuf,
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut encoder = AacEncoder::new()?;
    let asc = encoder.audio_specific_config().to_vec();

    let file = File::create(path).map_err(|e| format!("Failed to create recording file: {e}"))?;
    let writer = BufWriter::with_capacity(BUF_WRITER_SIZE, file);
    let mut muxer = FragmentedMp4Writer::new(writer, &asc)?;

    let mut i16_buf = [0i16; AAC_FRAME_SAMPLES];
    let mut buf_pos = 0usize;
    let mut frames_in_fragment = 0usize;

    log::info!("Recorder thread running");

    while !stop.load(Ordering::Relaxed) {
        match consumer.try_pop() {
            Some(sample) => {
                i16_buf[buf_pos] = aac::f32_to_i16(sample);
                buf_pos += 1;

                if buf_pos == AAC_FRAME_SAMPLES {
                    let aac_data = encoder.encode_frame(&i16_buf)?;
                    buf_pos = 0;

                    if !aac_data.is_empty() {
                        muxer.push_frame(&aac_data);
                        frames_in_fragment += 1;

                        if frames_in_fragment >= FRAMES_PER_FRAGMENT {
                            muxer.flush_fragment()?;
                            frames_in_fragment = 0;
                        }
                    }
                }
            }
            None => {
                thread::sleep(std::time::Duration::from_micros(500));
            }
        }
    }

    // Drain remaining samples from ring
    loop {
        match consumer.try_pop() {
            Some(sample) => {
                i16_buf[buf_pos] = aac::f32_to_i16(sample);
                buf_pos += 1;
                if buf_pos == AAC_FRAME_SAMPLES {
                    let aac_data = encoder.encode_frame(&i16_buf)?;
                    buf_pos = 0;
                    if !aac_data.is_empty() {
                        muxer.push_frame(&aac_data);
                    }
                }
            }
            None => break,
        }
    }

    // Pad last partial frame with silence and encode
    if buf_pos > 0 {
        for i in buf_pos..AAC_FRAME_SAMPLES {
            i16_buf[i] = 0;
        }
        let aac_data = encoder.encode_frame(&i16_buf)?;
        if !aac_data.is_empty() {
            muxer.push_frame(&aac_data);
        }
    }

    // Finalize: flush remaining fragment + write finalization moov
    muxer.finalize()?;

    log::info!("Recorder thread finalized: {}", path.display());
    Ok(())
}
