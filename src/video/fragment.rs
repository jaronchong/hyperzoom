use std::collections::HashMap;
use std::time::Instant;

/// Maximum payload per UDP fragment (fits within typical MTU).
pub const MAX_FRAGMENT_SIZE: usize = 1200;

/// Split an encoded video frame into MTU-sized fragments.
///
/// Returns a list of `(fragment_id, fragment_total, data)` tuples.
pub fn fragment_payload(encoded: &[u8]) -> Vec<(u8, u8, Vec<u8>)> {
    if encoded.len() <= MAX_FRAGMENT_SIZE {
        return vec![(0, 1, encoded.to_vec())];
    }

    let total = (encoded.len() + MAX_FRAGMENT_SIZE - 1) / MAX_FRAGMENT_SIZE;
    let total = total.min(255) as u8; // fragment_total is u8

    let mut fragments = Vec::with_capacity(total as usize);
    for i in 0..total {
        let start = i as usize * MAX_FRAGMENT_SIZE;
        let end = (start + MAX_FRAGMENT_SIZE).min(encoded.len());
        fragments.push((i, total, encoded[start..end].to_vec()));
    }
    fragments
}

/// Tracks fragments of a single frame being reassembled.
struct PendingFrame {
    fragments: HashMap<u8, Vec<u8>>,
    total: u8,
    is_keyframe: bool,
    created: Instant,
}

/// Reassembles fragmented video frames from multiple peers.
pub struct FragmentAssembler {
    /// Keyed by (participant_id, timestamp_ms)
    pending: HashMap<(u8, u32), PendingFrame>,
}

/// A fully reassembled frame ready for decoding.
pub struct ReassembledFrame {
    pub participant_id: u8,
    pub timestamp_ms: u32,
    pub data: Vec<u8>,
    pub is_keyframe: bool,
}

impl FragmentAssembler {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Push a fragment. Returns the complete frame if all fragments have arrived.
    pub fn push(
        &mut self,
        participant_id: u8,
        timestamp_ms: u32,
        fragment_id: u8,
        fragment_total: u8,
        data: &[u8],
        is_keyframe: bool,
    ) -> Option<ReassembledFrame> {
        if fragment_total == 0 {
            return None;
        }

        // Single-fragment frame — no assembly needed
        if fragment_total == 1 {
            return Some(ReassembledFrame {
                participant_id,
                timestamp_ms,
                data: data.to_vec(),
                is_keyframe,
            });
        }

        let key = (participant_id, timestamp_ms);
        let pending = self.pending.entry(key).or_insert_with(|| PendingFrame {
            fragments: HashMap::new(),
            total: fragment_total,
            is_keyframe,
            created: Instant::now(),
        });

        pending.fragments.insert(fragment_id, data.to_vec());

        if pending.fragments.len() == pending.total as usize {
            // All fragments received — reassemble in order
            let frame = self.pending.remove(&key).unwrap();
            let mut full_data = Vec::new();
            for i in 0..frame.total {
                if let Some(frag) = frame.fragments.get(&i) {
                    full_data.extend_from_slice(frag);
                }
            }
            Some(ReassembledFrame {
                participant_id,
                timestamp_ms,
                data: full_data,
                is_keyframe: frame.is_keyframe,
            })
        } else {
            None
        }
    }

    /// Drop incomplete frames older than the given duration.
    pub fn expire_stale(&mut self, max_age: std::time::Duration) {
        let now = Instant::now();
        self.pending
            .retain(|_, pf| now.duration_since(pf.created) < max_age);
    }
}
