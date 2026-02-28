/// Thin wrapper around `vpx-encode` for VP8 encoding.

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub pts: i64,
}

pub struct Vp8Encoder {
    inner: vpx_encode::Encoder,
}

impl Vp8Encoder {
    /// Create a VP8 encoder for the given resolution.
    ///
    /// Default config: 400 kbps VBR, timebase 1/24, keyframe every ~2s.
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let config = vpx_encode::Config {
            width,
            height,
            timebase: [1, 24],
            bitrate: 400,
            codec: vpx_encode::VideoCodecId::VP8,
        };
        let encoder = vpx_encode::Encoder::new(config)
            .map_err(|e| format!("VP8 encoder init failed: {e}"))?;
        Ok(Self { inner: encoder })
    }

    /// Encode a single I420 frame. Returns zero or more encoded packets.
    pub fn encode(&mut self, i420_data: &[u8], pts: i64) -> Result<Vec<EncodedFrame>, String> {
        let packets = self
            .inner
            .encode(pts, i420_data)
            .map_err(|e| format!("VP8 encode failed: {e}"))?;

        let frames: Vec<EncodedFrame> = packets
            .into_iter()
            .map(|pkt| EncodedFrame {
                data: pkt.data.to_vec(),
                is_keyframe: pkt.key,
                pts: pkt.pts,
            })
            .collect();

        Ok(frames)
    }

    /// Flush remaining packets from the encoder.
    pub fn finish(self) -> Result<Vec<EncodedFrame>, String> {
        let mut finish = self
            .inner
            .finish()
            .map_err(|e| format!("VP8 finish failed: {e}"))?;

        let mut frames = Vec::new();
        loop {
            match finish.next() {
                Ok(Some(pkt)) => {
                    frames.push(EncodedFrame {
                        data: pkt.data.to_vec(),
                        is_keyframe: pkt.key,
                        pts: pkt.pts,
                    });
                }
                Ok(None) => break,
                Err(e) => return Err(format!("VP8 finish packet error: {e}")),
            }
        }

        Ok(frames)
    }
}
