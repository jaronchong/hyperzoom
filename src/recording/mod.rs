use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde::Serialize;

/// Create a session recording directory: ~/HyperZoom/recordings/YYYY-MM-DD_HH-MM-SS/
/// Returns the directory path.
pub fn create_session_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let dir = home
        .join("HyperZoom")
        .join("recordings")
        .join(&timestamp);

    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create session directory: {e}"))?;

    log::info!("Session directory created: {}", dir.display());
    Ok(dir)
}

/// The filename for the local audio recording within the session directory.
pub fn recording_filename() -> &'static str {
    "local_recording.mp4"
}

#[derive(Serialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub start_time: String,
    pub end_time: String,
    pub duration_seconds: f64,
    pub participants: Vec<ParticipantInfo>,
    pub recording: RecordingInfo,
}

#[derive(Serialize)]
pub struct ParticipantInfo {
    pub id: u8,
    pub name: String,
}

#[derive(Serialize)]
pub struct RecordingInfo {
    pub file: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate_kbps: u32,
}

/// Write session_metadata.json to the session directory.
pub fn write_metadata(dir: &PathBuf, metadata: &SessionMetadata) -> Result<(), String> {
    let path = dir.join("session_metadata.json");
    let json = serde_json::to_string_pretty(metadata)
        .map_err(|e| format!("Failed to serialize metadata: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write metadata: {e}"))?;
    log::info!("Session metadata written: {}", path.display());
    Ok(())
}
