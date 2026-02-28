use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use eframe::egui;
use tokio::sync::oneshot;

use crate::audio::AudioPipeline;
use crate::audio::jitter::JitterBuffer;
use crate::net::manager::{ConnectResult, NetworkManager};
use crate::net::session::{PeerState, SharedSessionState};
use crate::recording;
use crate::video::VideoPipeline;

#[derive(Debug, Clone, PartialEq)]
enum AppScreen {
    PreCall,
    InCall,
    PostCall,
}

pub struct HyperZoomApp {
    runtime: tokio::runtime::Runtime,
    screen: AppScreen,

    // PreCall fields
    name_input: String,
    port_input: String,
    host_addr_input: String,
    status_message: String,

    // Connection in progress
    connect_rx: Option<oneshot::Receiver<ConnectResult>>,

    // InCall state
    session_state: Option<SharedSessionState>,
    // Keep pipeline alive by holding a reference
    _audio_pipeline: Option<AudioPipeline>,
    video_pipeline: Option<VideoPipeline>,
    camera_on: bool,
    // Keep manager pieces alive
    manager_transport: Option<Arc<crate::net::socket::UdpTransport>>,
    manager_jitter: Option<Arc<Mutex<JitterBuffer>>>,
    manager_heartbeat_stop: Option<tokio::sync::watch::Sender<bool>>,
    manager_inbound_stop: Option<tokio::sync::watch::Sender<bool>>,
    manager_upnp: Option<crate::net::upnp::PortMapping>,

    // PostCall fields
    end_reason: String,

    // Recording state
    session_dir: Option<PathBuf>,
    session_start_time: Option<chrono::DateTime<Utc>>,
    recording_path_display: Option<String>,
}

impl HyperZoomApp {
    pub fn new(runtime: tokio::runtime::Runtime) -> Self {
        Self {
            runtime,
            screen: AppScreen::PreCall,
            name_input: "User".into(),
            port_input: "9000".into(),
            host_addr_input: "127.0.0.1:9000".into(),
            status_message: String::new(),
            connect_rx: None,
            session_state: None,
            _audio_pipeline: None,
            video_pipeline: None,
            camera_on: true,
            manager_transport: None,
            manager_jitter: None,
            manager_heartbeat_stop: None,
            manager_inbound_stop: None,
            manager_upnp: None,
            end_reason: String::new(),
            session_dir: None,
            session_start_time: None,
            recording_path_display: None,
        }
    }

    fn handle_host(&mut self) {
        let port: u16 = match self.port_input.parse() {
            Ok(p) => p,
            Err(_) => {
                self.status_message = "Invalid port number".into();
                return;
            }
        };

        self.status_message = format!("Hosting on port {port}...");
        let (tx, rx) = oneshot::channel();
        self.connect_rx = Some(rx);

        let handle = self.runtime.handle().clone();
        let name = self.name_input.clone();
        NetworkManager::host(handle, name, port, tx);
    }

    fn handle_join(&mut self) {
        let host_addr: SocketAddr = match self.host_addr_input.parse() {
            Ok(a) => a,
            Err(_) => {
                self.status_message = "Invalid host address (use IP:port)".into();
                return;
            }
        };

        // Use a different local port for the guest
        let local_port: u16 = match self.port_input.parse::<u16>() {
            Ok(p) => p,
            Err(_) => {
                self.status_message = "Invalid port number".into();
                return;
            }
        };

        self.status_message = format!("Joining {host_addr}...");
        let (tx, rx) = oneshot::channel();
        self.connect_rx = Some(rx);

        let handle = self.runtime.handle().clone();
        let name = self.name_input.clone();
        NetworkManager::join(handle, name, host_addr, local_port, tx);
    }

    fn handle_end_call(&mut self) {
        self.end_reason = "Call ended by user".into();

        // Stop heartbeat and inbound tasks
        if let Some(stop) = self.manager_heartbeat_stop.take() {
            let _ = stop.send(true);
        }
        if let Some(stop) = self.manager_inbound_stop.take() {
            let _ = stop.send(true);
        }

        // Send BYE packets
        if let (Some(state), Some(transport)) = (&self.session_state, &self.manager_transport) {
            let (my_id, ts, peer_addrs) = {
                let mut s = state.lock().unwrap();
                s.ended = true;
                (s.my_participant_id, s.elapsed_ms(), s.connected_peer_addrs())
            };

            use crate::net::protocol::{Packet, PacketHeader, PacketType};
            let header = PacketHeader::new(PacketType::Bye, my_id, 0, ts, 0);
            let packet = Packet::new(header, Vec::new()).to_bytes();

            let transport = transport.clone();
            self.runtime.spawn(async move {
                for _ in 0..3 {
                    for addr in &peer_addrs {
                        let _ = transport.send_to(&packet, *addr).await;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            });
        }

        // Remove UPnP mapping
        if let Some(mapping) = self.manager_upnp.take() {
            self.runtime.spawn(async move {
                mapping.remove().await;
            });
        }

        // Collect session info before dropping state
        let participants = if let Some(state) = &self.session_state {
            let s = state.lock().unwrap();
            let mut parts = vec![recording::ParticipantInfo {
                id: s.my_participant_id,
                name: s.my_name.clone(),
            }];
            for peer in s.peers.values() {
                parts.push(recording::ParticipantInfo {
                    id: peer.participant_id,
                    name: peer.name.clone(),
                });
            }
            parts
        } else {
            Vec::new()
        };

        // Drop video pipeline first (stops encode/decode threads)
        self.video_pipeline = None;

        // Drop audio pipeline (stops recorder first, then encode/refill threads)
        self._audio_pipeline = None;

        // Write session metadata
        if let (Some(dir), Some(start_time)) = (&self.session_dir, self.session_start_time) {
            let end_time = Utc::now();
            let duration = end_time
                .signed_duration_since(start_time)
                .num_milliseconds() as f64
                / 1000.0;

            let metadata = recording::SessionMetadata {
                session_id: self
                    .session_state
                    .as_ref()
                    .map(|s| format!("{:#010X}", s.lock().unwrap().session_id))
                    .unwrap_or_default(),
                start_time: start_time.to_rfc3339(),
                end_time: end_time.to_rfc3339(),
                duration_seconds: duration,
                participants,
                recording: recording::RecordingInfo {
                    file: recording::recording_filename().to_string(),
                    codec: "AAC-LC".to_string(),
                    sample_rate: 48000,
                    channels: 1,
                    bitrate_kbps: 192,
                },
            };

            if let Err(e) = recording::write_metadata(dir, &metadata) {
                log::error!("Failed to write session metadata: {e}");
            }
        }

        self.session_state = None;
        self.manager_transport = None;
        self.manager_jitter = None;
        self.session_start_time = None;

        self.screen = AppScreen::PostCall;
    }

    fn check_connection_result(&mut self) {
        let mut rx = match self.connect_rx.take() {
            Some(rx) => rx,
            None => return,
        };

        match rx.try_recv() {
            Ok(ConnectResult::Ready {
                state,
                socket,
                transport,
                jitter,
                heartbeat_stop,
                inbound_stop,
                upnp,
                local_port,
                video_rx,
            }) => {
                // Create session directory for recording
                let (session_dir, recording_path) = match recording::create_session_dir() {
                    Ok(dir) => {
                        let rec_path = dir.join(recording::recording_filename());
                        (Some(dir), Some(rec_path))
                    }
                    Err(e) => {
                        log::warn!("Failed to create session dir, recording disabled: {e}");
                        (None, None)
                    }
                };

                // Create AudioPipeline on the main thread (cpal::Stream is !Send)
                let handle = self.runtime.handle().clone();
                match AudioPipeline::new(
                    state.clone(),
                    socket.clone(),
                    handle.clone(),
                    jitter.clone(),
                    recording_path.clone(),
                ) {
                    Ok(audio_pipeline) => {
                        // Create VideoPipeline
                        let video_pipeline = match VideoPipeline::new(
                            self.camera_on,
                            state.clone(),
                            socket,
                            handle,
                            video_rx,
                        ) {
                            Ok(vp) => {
                                log::info!("VideoPipeline created");
                                Some(vp)
                            }
                            Err(e) => {
                                log::warn!("VideoPipeline failed: {e}");
                                None
                            }
                        };

                        self.status_message =
                            format!("Connected on port {local_port}");
                        self.session_state = Some(state);
                        self._audio_pipeline = Some(audio_pipeline);
                        self.video_pipeline = video_pipeline;
                        self.manager_transport = Some(transport);
                        self.manager_jitter = Some(jitter);
                        self.manager_heartbeat_stop = Some(heartbeat_stop);
                        self.manager_inbound_stop = Some(inbound_stop);
                        self.manager_upnp = upnp;
                        self.session_dir = session_dir;
                        self.session_start_time = Some(Utc::now());
                        self.recording_path_display =
                            recording_path.map(|p| p.display().to_string());
                        self.screen = AppScreen::InCall;
                    }
                    Err(e) => {
                        self.status_message = format!("Audio pipeline failed: {e}");
                    }
                }
            }
            Ok(ConnectResult::Error(e)) => {
                self.status_message = format!("Connection failed: {e}");
            }
            Err(oneshot::error::TryRecvError::Empty) => {
                // Not ready yet, put it back
                self.connect_rx = Some(rx);
            }
            Err(oneshot::error::TryRecvError::Closed) => {
                self.status_message = "Connection attempt failed unexpectedly".into();
            }
        }
    }

    fn check_peer_disconnects(&mut self) {
        if let Some(state) = &self.session_state {
            let all_disconnected = {
                let s = state.lock().unwrap();
                if s.ended {
                    true
                } else {
                    !s.peers.is_empty()
                        && s.peers.values().all(|p| p.state == PeerState::Disconnected)
                }
            };

            if all_disconnected {
                let s = state.lock().unwrap();
                if !s.peers.is_empty() {
                    drop(s);
                    self.end_reason = "All peers disconnected".into();
                    // Drop pipelines first
                    self.video_pipeline = None;
                    self._audio_pipeline = None;
                    self.session_state = None;
                    self.screen = AppScreen::PostCall;
                }
            }
        }
    }

    /// Update video textures from the latest frames and render the InCall screen.
    fn show_incall(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        self.check_peer_disconnects();

        let (my_name, my_id, peers_info, duration_s) = {
            if let Some(state) = &self.session_state {
                let s = state.lock().unwrap();
                let peers: Vec<(u8, String, PeerState)> = s
                    .peers
                    .values()
                    .map(|p| (p.participant_id, p.name.clone(), p.state))
                    .collect();
                (
                    s.my_name.clone(),
                    s.my_participant_id,
                    peers,
                    s.elapsed_ms() / 1000,
                )
            } else {
                return;
            }
        };

        // Header bar
        ui.horizontal(|ui| {
            ui.label(format!("You: {my_name} (ID {my_id})"));
            ui.separator();
            ui.label(format!("Duration: {duration_s}s"));
            ui.separator();

            // Camera toggle
            let cam_label = if self.camera_on { "Camera ON" } else { "Camera OFF" };
            if ui.button(cam_label).clicked() {
                self.camera_on = !self.camera_on;
                if let Some(ref vp) = self.video_pipeline {
                    vp.set_camera_enabled(self.camera_on);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("End Call").clicked() {
                    self.handle_end_call();
                }
            });
        });

        ui.separator();

        // Update textures from latest video frames
        if let Some(ref mut vp) = self.video_pipeline {
            // Local preview
            if let Ok(lf) = vp.local_frame.lock() {
                if let Some(ref frame) = *lf {
                    vp.display.update_local(ctx, frame);
                }
            }

            // Remote peers
            if let Ok(rf) = vp.remote_frames.lock() {
                for (&pid, frame) in rf.iter() {
                    vp.display.update_remote(ctx, pid, frame);
                }
            }

            // Build peer list for grid (sorted by id, only non-disconnected)
            let grid_peers: Vec<(u8, String)> = peers_info
                .iter()
                .filter(|(_, _, state)| *state != PeerState::Disconnected)
                .map(|(id, name, _)| (*id, name.clone()))
                .collect();

            // Render video grid
            vp.display.show_grid(ui, &my_name, &grid_peers);
        } else {
            // No video pipeline — show text-only peer list
            ui.label("Peers:");
            if peers_info.is_empty() {
                ui.label("  (waiting for peers to join)");
            } else {
                for (pid, name, state) in &peers_info {
                    let status = match state {
                        PeerState::Connecting => "connecting...",
                        PeerState::Connected => "connected",
                        PeerState::Disconnected => "disconnected",
                    };
                    ui.label(format!("  {name} (ID {pid}) -- {status}"));
                }
            }
        }

        // Repaint at ~30fps for smooth video
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

impl eframe::App for HyperZoomApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for async results
        self.check_connection_result();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("HyperZoom");
            ui.separator();

            match self.screen.clone() {
                AppScreen::PreCall => {
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut self.name_input);
                    });

                    ui.horizontal(|ui| {
                        ui.label("Local Port:");
                        ui.text_edit_singleline(&mut self.port_input);
                    });

                    ui.add_space(10.0);

                    let connecting = self.connect_rx.is_some();

                    if ui
                        .add_enabled(!connecting, egui::Button::new("Host"))
                        .clicked()
                    {
                        self.handle_host();
                    }

                    ui.add_space(5.0);
                    ui.separator();
                    ui.add_space(5.0);

                    ui.horizontal(|ui| {
                        ui.label("Host Address:");
                        ui.text_edit_singleline(&mut self.host_addr_input);
                    });

                    if ui
                        .add_enabled(!connecting, egui::Button::new("Join"))
                        .clicked()
                    {
                        self.handle_join();
                    }

                    if !self.status_message.is_empty() {
                        ui.add_space(10.0);
                        ui.label(&self.status_message);
                    }

                    // Poll for connection result while waiting
                    if connecting {
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                }

                AppScreen::InCall => {
                    self.show_incall(ctx, ui);
                }

                AppScreen::PostCall => {
                    ui.add_space(10.0);
                    ui.label("Call ended");
                    ui.label(&self.end_reason);

                    if let Some(ref path) = self.recording_path_display {
                        ui.add_space(5.0);
                        ui.label(format!("Recording saved: {path}"));
                    }
                    if let Some(ref dir) = self.session_dir {
                        ui.label(format!("Session: {}", dir.display()));
                    }

                    ui.add_space(10.0);

                    if ui.button("New Call").clicked() {
                        self.status_message.clear();
                        self.end_reason.clear();
                        self.session_dir = None;
                        self.recording_path_display = None;
                        self.screen = AppScreen::PreCall;
                    }
                }
            }
        });
    }
}
