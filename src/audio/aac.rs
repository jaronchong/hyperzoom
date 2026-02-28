use fdk_aac::enc::{
    AudioObjectType, BitRate, ChannelMode, EncodeInfo, Encoder, EncoderParams, Transport,
};

const SAMPLE_RATE: u32 = 48_000;
const BITRATE: u32 = 192_000;

/// AAC-LC frame size in samples.
pub const AAC_FRAME_SAMPLES: usize = 1024;

/// Wraps fdk-aac for AAC-LC encoding at 48 kHz mono, 192 kbps CBR.
/// Outputs raw AAC frames (no ADTS headers) suitable for MP4 muxing.
pub struct AacEncoder {
    encoder: Encoder,
    /// AudioSpecificConfig bytes for the esds box in MP4.
    asc: Vec<u8>,
    /// Persistent output buffer sized to encoder's max frame bytes.
    out_buf: Vec<u8>,
}

impl AacEncoder {
    pub fn new() -> Result<Self, String> {
        let params = EncoderParams {
            bit_rate: BitRate::Cbr(BITRATE),
            sample_rate: SAMPLE_RATE,
            transport: Transport::Raw,
            channels: ChannelMode::Mono,
            audio_object_type: AudioObjectType::Mpeg4LowComplexity,
        };

        let encoder =
            Encoder::new(params).map_err(|e| format!("Failed to create AAC encoder: {e:?}"))?;

        let info = encoder
            .info()
            .map_err(|e| format!("Failed to get AAC encoder info: {e:?}"))?;

        let asc = info.confBuf[..info.confSize as usize].to_vec();
        let out_buf = vec![0u8; info.maxOutBufBytes as usize];

        log::info!(
            "AAC encoder created: 48kHz mono, 192kbps CBR, frame_len={}, delay={} samples, ASC={} bytes",
            info.frameLength,
            info.nDelay,
            asc.len()
        );

        Ok(Self {
            encoder,
            asc,
            out_buf,
        })
    }

    /// Encode one AAC frame (1024 i16 PCM samples).
    /// Returns the raw AAC frame bytes, or empty if the encoder hasn't produced output yet
    /// (priming delay).
    pub fn encode_frame(&mut self, samples: &[i16; AAC_FRAME_SAMPLES]) -> Result<Vec<u8>, String> {
        let info: EncodeInfo = self
            .encoder
            .encode(samples, &mut self.out_buf)
            .map_err(|e| format!("AAC encode failed: {e:?}"))?;

        if info.output_size > 0 {
            Ok(self.out_buf[..info.output_size].to_vec())
        } else {
            Ok(Vec::new())
        }
    }

    /// The AudioSpecificConfig bytes (typically 2 bytes for AAC-LC mono 48kHz).
    /// Used in the esds box of the MP4 init segment.
    pub fn audio_specific_config(&self) -> &[u8] {
        &self.asc
    }
}

/// Convert f32 audio sample to i16 for AAC encoding.
#[inline]
pub fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}
