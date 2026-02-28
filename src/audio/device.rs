use cpal::traits::{DeviceTrait, HostTrait};

pub fn log_all_devices() {
    let host = cpal::default_host();
    log::info!("Audio host: {}", host.id().name());

    log::info!("--- Input devices ---");
    match host.input_devices() {
        Ok(devices) => {
            for dev in devices {
                let name = dev.name().unwrap_or_else(|_| "<unknown>".into());
                log::info!("  input: {name}");
            }
        }
        Err(e) => log::warn!("Failed to enumerate input devices: {e}"),
    }

    log::info!("--- Output devices ---");
    match host.output_devices() {
        Ok(devices) => {
            for dev in devices {
                let name = dev.name().unwrap_or_else(|_| "<unknown>".into());
                log::info!("  output: {name}");
            }
        }
        Err(e) => log::warn!("Failed to enumerate output devices: {e}"),
    }
}

pub fn default_input() -> Result<cpal::Device, String> {
    cpal::default_host()
        .default_input_device()
        .ok_or_else(|| "No default input device found. Is a microphone connected?".into())
}

pub fn default_output() -> Result<cpal::Device, String> {
    cpal::default_host()
        .default_output_device()
        .ok_or_else(|| "No default output device found. Are speakers/headphones connected?".into())
}
