use opus::{Application, Bitrate, Channels, Decoder, Encoder};

pub const OPUS_FRAME_SAMPLES: usize = 240; // 5ms at 48kHz
pub const MAX_ENCODED_SIZE: usize = 256;
pub const SAMPLE_RATE: u32 = 48_000;

/// Create an Opus encoder configured for low-delay voice.
/// 48kHz, mono, LowDelay application, 32kbps CBR, FEC enabled.
pub fn create_encoder() -> Result<Encoder, String> {
    let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::LowDelay)
        .map_err(|e| format!("Opus encoder creation failed: {e}"))?;

    encoder
        .set_bitrate(Bitrate::Bits(32_000))
        .map_err(|e| format!("Failed to set bitrate: {e}"))?;

    encoder
        .set_vbr(false)
        .map_err(|e| format!("Failed to set CBR mode: {e}"))?;

    encoder
        .set_inband_fec(true)
        .map_err(|e| format!("Failed to enable FEC: {e}"))?;

    log::info!("Opus encoder created: 48kHz mono, 32kbps CBR, FEC on");
    Ok(encoder)
}

/// Create an Opus decoder for 48kHz mono.
pub fn create_decoder() -> Result<Decoder, String> {
    let decoder = Decoder::new(SAMPLE_RATE, Channels::Mono)
        .map_err(|e| format!("Opus decoder creation failed: {e}"))?;
    log::info!("Opus decoder created: 48kHz mono");
    Ok(decoder)
}

/// Encode a frame of 240 mono f32 samples into Opus bytes.
/// Returns the encoded bytes or an error.
pub fn encode_frame(encoder: &mut Encoder, samples: &[f32; OPUS_FRAME_SAMPLES]) -> Result<Vec<u8>, String> {
    let mut output = [0u8; MAX_ENCODED_SIZE];
    let len = encoder
        .encode_float(samples, &mut output)
        .map_err(|e| format!("Opus encode failed: {e}"))?;
    Ok(output[..len].to_vec())
}

/// Decode Opus bytes into 240 mono f32 samples.
/// If `data` is None, performs packet loss concealment (PLC).
pub fn decode_frame(decoder: &mut Decoder, data: Option<&[u8]>) -> [f32; OPUS_FRAME_SAMPLES] {
    let mut output = [0.0f32; OPUS_FRAME_SAMPLES];
    match data {
        Some(bytes) => {
            if let Err(e) = decoder.decode_float(bytes, &mut output, false) {
                log::warn!("Opus decode failed, using silence: {e}");
                output = [0.0; OPUS_FRAME_SAMPLES];
            }
        }
        None => {
            // PLC: decode with no data
            if let Err(e) = decoder.decode_float(&[], &mut output, false) {
                log::debug!("Opus PLC failed: {e}");
                output = [0.0; OPUS_FRAME_SAMPLES];
            }
        }
    }
    output
}
