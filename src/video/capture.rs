use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use ringbuf::traits::Producer;
use ringbuf::HeapProd;

use super::frame::VideoFrame;

/// Camera capture thread. Produces RGB frames from the default webcam.
pub struct CameraCapture {
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl CameraCapture {
    /// Start capturing from the default camera.
    /// Pushes VideoFrames into the ring producer at ~30fps.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn start(mut producer: HeapProd<VideoFrame>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();

        let thread = thread::Builder::new()
            .name("video-capture".into())
            .spawn(move || {
                Self::capture_loop(&stop_flag, &mut producer);
            })
            .map_err(|e| format!("Failed to spawn capture thread: {e}"))?;

        Ok(Self {
            thread: Some(thread),
            stop,
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub fn start(_producer: HeapProd<VideoFrame>) -> Result<Self, String> {
        log::warn!("Camera capture not supported on this platform");
        let stop = Arc::new(AtomicBool::new(false));
        Ok(Self { thread: None, stop })
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn capture_loop(stop: &AtomicBool, producer: &mut HeapProd<VideoFrame>) {
        use nokhwa::pixel_format::RgbFormat;
        use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
        use nokhwa::Camera;

        // Request camera permission (required on macOS, no-op on Windows)
        #[cfg(target_os = "macos")]
        {
            let (perm_tx, perm_rx) = std::sync::mpsc::channel();
            nokhwa::nokhwa_initialize(move |granted| {
                let _ = perm_tx.send(granted);
            });
            match perm_rx.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(true) => log::info!("Camera permission granted"),
                Ok(false) => {
                    log::warn!("Camera permission denied");
                    return;
                }
                Err(_) => {
                    log::warn!("Camera permission timeout");
                    return;
                }
            }
        }

        // Open default camera at highest framerate
        let requested =
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
        let mut camera = match Camera::new(CameraIndex::Index(0), requested) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Failed to open camera: {e}");
                return;
            }
        };

        if let Err(e) = camera.open_stream() {
            log::warn!("Failed to open camera stream: {e}");
            return;
        }

        let res = camera.resolution();
        log::info!(
            "Camera opened: {}x{} @ native fps",
            res.width_x, res.height_y
        );

        let target_interval = std::time::Duration::from_millis(33); // ~30fps

        while !stop.load(Ordering::Relaxed) {
            let start = std::time::Instant::now();

            match camera.frame() {
                Ok(buffer) => {
                    let resolution = buffer.resolution();
                    match buffer.decode_image::<RgbFormat>() {
                        Ok(decoded) => {
                            let frame = VideoFrame {
                                width: resolution.width_x,
                                height: resolution.height_y,
                                data: decoded.into_raw(),
                            };
                            // Push to ring; if full, drop this frame (encode is behind)
                            let _ = producer.try_push(frame);
                        }
                        Err(e) => log::debug!("Frame decode error: {e}"),
                    }
                }
                Err(e) => {
                    log::debug!("Camera frame error: {e}");
                    thread::sleep(std::time::Duration::from_millis(10));
                }
            }

            // Pace to ~30fps
            let elapsed = start.elapsed();
            if elapsed < target_interval {
                thread::sleep(target_interval - elapsed);
            }
        }

        let _ = camera.stop_stream();
        log::info!("Camera capture stopped");
    }
}

impl Drop for CameraCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
