use std::collections::BTreeMap;

use super::codec::OPUS_FRAME_SAMPLES;

/// Maximum number of frames the jitter buffer can hold before eviction.
const MAX_BUFFER_FRAMES: usize = 60; // 300ms at 5ms/frame

/// Adaptive jitter buffer for incoming audio frames.
///
/// Frames are keyed by sequence number (u16). The buffer adapts its target
/// depth between 1 and 6 frames (5–30ms) based on underrun/overrun patterns.
pub struct JitterBuffer {
    frames: BTreeMap<u16, [f32; OPUS_FRAME_SAMPLES]>,
    next_seq: Option<u16>,
    target_depth: usize,
    consecutive_ok: usize,
    consecutive_underruns: usize,
}

impl JitterBuffer {
    pub fn new() -> Self {
        Self {
            frames: BTreeMap::new(),
            next_seq: None,
            target_depth: 2, // start at 10ms (2 frames × 5ms)
            consecutive_ok: 0,
            consecutive_underruns: 0,
        }
    }

    /// Insert a decoded audio frame keyed by sequence number.
    pub fn push(&mut self, seq: u16, samples: [f32; OPUS_FRAME_SAMPLES]) {
        self.frames.insert(seq, samples);

        // Evict stale entries if buffer is too large
        while self.frames.len() > MAX_BUFFER_FRAMES {
            self.frames.pop_first();
        }
    }

    /// Pull the next frame in sequence order.
    /// Returns the audio samples, or silence if the buffer is empty/starved.
    pub fn pull(&mut self) -> [f32; OPUS_FRAME_SAMPLES] {
        // Wait until we have at least target_depth frames before starting
        if self.next_seq.is_none() {
            if self.frames.len() >= self.target_depth {
                // Start playback from the earliest frame
                if let Some(&first_seq) = self.frames.keys().next() {
                    self.next_seq = Some(first_seq);
                }
            }
            if self.next_seq.is_none() {
                return [0.0; OPUS_FRAME_SAMPLES];
            }
        }

        let seq = self.next_seq.unwrap();

        if let Some(frame) = self.frames.remove(&seq) {
            self.next_seq = Some(seq.wrapping_add(1));
            self.consecutive_underruns = 0;
            self.consecutive_ok += 1;

            // Shrink depth after 200 consecutive OK pulls (~1 second)
            if self.consecutive_ok >= 200 && self.target_depth > 1 {
                self.target_depth -= 1;
                self.consecutive_ok = 0;
                log::debug!("Jitter buffer: shrink depth to {} frames", self.target_depth);
            }

            frame
        } else {
            // Underrun: frame not available yet
            self.next_seq = Some(seq.wrapping_add(1));
            self.consecutive_ok = 0;
            self.consecutive_underruns += 1;

            // Grow depth on 2 consecutive underruns
            if self.consecutive_underruns >= 2 && self.target_depth < 6 {
                self.target_depth += 1;
                self.consecutive_underruns = 0;
                log::debug!("Jitter buffer: grow depth to {} frames", self.target_depth);
            }

            [0.0; OPUS_FRAME_SAMPLES]
        }
    }

    /// Current number of buffered frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Current target depth in frames.
    pub fn target_depth(&self) -> usize {
        self.target_depth
    }

    /// Reset the buffer state (e.g., on peer reconnect).
    pub fn reset(&mut self) {
        self.frames.clear();
        self.next_seq = None;
        self.target_depth = 2;
        self.consecutive_ok = 0;
        self.consecutive_underruns = 0;
    }
}
