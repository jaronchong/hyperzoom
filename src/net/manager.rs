use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

use super::control::{self, Hello, Heartbeat, Welcome};
use super::protocol::{Packet, PacketHeader, PacketType};
use super::session::{PeerState, SharedSessionState, SessionState};
use super::socket::{InboundEvent, UdpTransport};
use super::upnp::PortMapping;
use crate::audio::codec;
use crate::audio::jitter::JitterBuffer;

/// Result of a host or join attempt, sent back to the UI via oneshot.
/// Does NOT contain AudioPipeline (cpal::Stream is !Send).
/// The app creates the pipeline from the returned components.
pub enum ConnectResult {
    Ready {
        state: SharedSessionState,
        socket: Arc<UdpSocket>,
        transport: Arc<UdpTransport>,
        jitter: Arc<Mutex<JitterBuffer>>,
        heartbeat_stop: tokio::sync::watch::Sender<bool>,
        inbound_stop: tokio::sync::watch::Sender<bool>,
        upnp: Option<PortMapping>,
        local_port: u16,
        video_rx: mpsc::UnboundedReceiver<InboundEvent>,
    },
    Error(String),
}

// ConnectResult contains PortMapping which has a gateway with non-Send internals.
// We send it from a tokio task back to the UI thread via oneshot.
// The PortMapping is only ever accessed from one thread at a time, so this is safe.
unsafe impl Send for ConnectResult {}

/// Static methods for launching host/join flows.
pub struct NetworkManager;

impl NetworkManager {
    /// Host a new session on the given port.
    pub fn host(
        handle: Handle,
        name: String,
        port: u16,
        result_tx: oneshot::Sender<ConnectResult>,
    ) {
        let h = handle.clone();
        handle.spawn(async move {
            let result = Self::do_host(h, name, port).await;
            let _ = result_tx.send(result);
        });
    }

    async fn do_host(_handle: Handle, name: String, port: u16) -> ConnectResult {
        let transport = match UdpTransport::bind(port).await {
            Ok(t) => Arc::new(t),
            Err(e) => return ConnectResult::Error(e),
        };

        let upnp = PortMapping::create(port).await;

        let state = Arc::new(Mutex::new(SessionState::new_host(name)));
        let jitter = Arc::new(Mutex::new(JitterBuffer::new()));

        let mut inbound_rx = transport.spawn_recv_loop();

        let mut decoder = match codec::create_decoder() {
            Ok(d) => d,
            Err(e) => return ConnectResult::Error(e),
        };

        // Video event channel
        let (video_tx, video_rx) = mpsc::unbounded_channel();

        // Spawn inbound processing task
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let state_clone = state.clone();
        let jitter_clone = jitter.clone();
        let transport_clone = transport.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    event = inbound_rx.recv() => {
                        match event {
                            Some(InboundEvent::Audio { participant_id, sequence, payload, .. }) => {
                                let samples = codec::decode_frame(&mut decoder, Some(&payload));
                                if let Ok(mut jb) = jitter_clone.lock() {
                                    jb.push(sequence, samples);
                                }
                                if let Ok(mut s) = state_clone.lock() {
                                    s.touch_peer(participant_id);
                                }
                            }
                            Some(ref ev @ InboundEvent::Video { participant_id, .. }) => {
                                if let Ok(mut s) = state_clone.lock() {
                                    s.touch_peer(participant_id);
                                }
                                // Forward to video pipeline
                                let _ = video_tx.send(ev.clone());
                            }
                            Some(InboundEvent::Control { from, payload, .. }) => {
                                Self::handle_control_host(
                                    &state_clone,
                                    &transport_clone,
                                    from,
                                    &payload,
                                ).await;
                            }
                            Some(InboundEvent::Bye { participant_id }) => {
                                if let Ok(mut s) = state_clone.lock() {
                                    if let Some(peer) = s.peers.get_mut(&participant_id) {
                                        log::info!("Peer {} sent BYE", peer.name);
                                        peer.state = PeerState::Disconnected;
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        // Start heartbeat
        let (hb_stop_tx, hb_stop_rx) = tokio::sync::watch::channel(false);
        Self::start_heartbeat(state.clone(), transport.clone(), hb_stop_rx);

        ConnectResult::Ready {
            state,
            socket: transport.socket.clone(),
            transport,
            jitter,
            heartbeat_stop: hb_stop_tx,
            inbound_stop: stop_tx,
            upnp,
            local_port: port,
            video_rx,
        }
    }

    /// Join an existing session.
    pub fn join(
        handle: Handle,
        name: String,
        host_addr: SocketAddr,
        local_port: u16,
        result_tx: oneshot::Sender<ConnectResult>,
    ) {
        let h = handle.clone();
        handle.spawn(async move {
            let result = Self::do_join(h, name, host_addr, local_port).await;
            let _ = result_tx.send(result);
        });
    }

    async fn do_join(
        _handle: Handle,
        name: String,
        host_addr: SocketAddr,
        local_port: u16,
    ) -> ConnectResult {
        let transport = match UdpTransport::bind(local_port).await {
            Ok(t) => Arc::new(t),
            Err(e) => return ConnectResult::Error(e),
        };

        let state = Arc::new(Mutex::new(SessionState::new_guest(name.clone())));
        let jitter = Arc::new(Mutex::new(JitterBuffer::new()));

        let mut inbound_rx = transport.spawn_recv_loop();

        // Send Hello to host
        let hello_payload = Hello { name: name.clone() }.to_bytes();
        let header = PacketHeader::new(PacketType::Control, 0, 0, 0, hello_payload.len() as u16);
        let packet = Packet::new(header, hello_payload).to_bytes();
        if let Err(e) = transport.send_to(&packet, host_addr).await {
            return ConnectResult::Error(format!("Failed to send Hello: {e}"));
        }
        log::info!("Sent Hello to {host_addr}");

        // Wait for Welcome (with timeout)
        let welcome = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match inbound_rx.recv().await {
                    Some(InboundEvent::Control { payload, from, .. }) => {
                        if let Some(ctrl_type) = control::parse_control_type(&payload) {
                            if ctrl_type == control::ControlType::Welcome {
                                if let Some(welcome) = Welcome::from_bytes(&payload) {
                                    return Ok((welcome, from));
                                }
                            }
                        }
                    }
                    None => return Err("Channel closed while waiting for Welcome".to_string()),
                    _ => continue,
                }
            }
        })
        .await;

        let (welcome, _) = match welcome {
            Ok(Ok(w)) => w,
            Ok(Err(e)) => return ConnectResult::Error(e),
            Err(_) => return ConnectResult::Error("Timeout waiting for Welcome from host".into()),
        };

        log::info!(
            "Received Welcome: session={:#010X}, my_id={}",
            welcome.session_id, welcome.assigned_participant_id
        );

        {
            let mut s = state.lock().unwrap();
            s.session_id = welcome.session_id;
            s.my_participant_id = welcome.assigned_participant_id;
            s.add_peer(1, "Host".into(), host_addr);
            s.touch_peer(1);
        }

        let mut decoder = match codec::create_decoder() {
            Ok(d) => d,
            Err(e) => return ConnectResult::Error(e),
        };

        // Video event channel
        let (video_tx, video_rx) = mpsc::unbounded_channel();

        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let state_clone = state.clone();
        let jitter_clone = jitter.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    event = inbound_rx.recv() => {
                        match event {
                            Some(InboundEvent::Audio { participant_id, sequence, payload, .. }) => {
                                let samples = codec::decode_frame(&mut decoder, Some(&payload));
                                if let Ok(mut jb) = jitter_clone.lock() {
                                    jb.push(sequence, samples);
                                }
                                if let Ok(mut s) = state_clone.lock() {
                                    s.touch_peer(participant_id);
                                }
                            }
                            Some(ref ev @ InboundEvent::Video { participant_id, .. }) => {
                                if let Ok(mut s) = state_clone.lock() {
                                    s.touch_peer(participant_id);
                                }
                                let _ = video_tx.send(ev.clone());
                            }
                            Some(InboundEvent::Control { .. }) => {}
                            Some(InboundEvent::Bye { participant_id }) => {
                                if let Ok(mut s) = state_clone.lock() {
                                    if let Some(peer) = s.peers.get_mut(&participant_id) {
                                        log::info!("Peer {} sent BYE", peer.name);
                                        peer.state = PeerState::Disconnected;
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        let (hb_stop_tx, hb_stop_rx) = tokio::sync::watch::channel(false);
        Self::start_heartbeat(state.clone(), transport.clone(), hb_stop_rx);

        ConnectResult::Ready {
            state,
            socket: transport.socket.clone(),
            transport,
            jitter,
            heartbeat_stop: hb_stop_tx,
            inbound_stop: stop_tx,
            upnp: None,
            local_port,
            video_rx,
        }
    }

    async fn handle_control_host(
        state: &SharedSessionState,
        transport: &Arc<UdpTransport>,
        from: SocketAddr,
        payload: &[u8],
    ) {
        let ctrl_type = match control::parse_control_type(payload) {
            Some(t) => t,
            None => return,
        };

        match ctrl_type {
            control::ControlType::Hello => {
                let hello = match Hello::from_bytes(payload) {
                    Some(h) => h,
                    None => return,
                };
                log::info!("Received Hello from {} at {from}", hello.name);

                let (session_id, assigned_id, my_id) = {
                    let mut s = state.lock().unwrap();
                    let assigned_id = s.assign_participant_id();
                    s.add_peer(assigned_id, hello.name.clone(), from);
                    s.touch_peer(assigned_id);
                    (s.session_id, assigned_id, s.my_participant_id)
                };

                let welcome = Welcome {
                    session_id,
                    assigned_participant_id: assigned_id,
                };
                let welcome_payload = welcome.to_bytes();
                let header = PacketHeader::new(
                    PacketType::Control,
                    my_id,
                    0,
                    0,
                    welcome_payload.len() as u16,
                );
                let packet = Packet::new(header, welcome_payload).to_bytes();
                if let Err(e) = transport.send_to(&packet, from).await {
                    log::warn!("Failed to send Welcome to {from}: {e}");
                }
                log::info!("Sent Welcome to {} (id={})", hello.name, assigned_id);
            }
            control::ControlType::Heartbeat => {
                let mut s = state.lock().unwrap();
                let peer_id = s
                    .peers
                    .values()
                    .find(|p| p.addr == from)
                    .map(|p| p.participant_id);
                if let Some(id) = peer_id {
                    s.touch_peer(id);
                }
            }
            _ => {}
        }
    }

    fn start_heartbeat(
        state: SharedSessionState,
        transport: Arc<UdpTransport>,
        mut stop_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    _ = interval.tick() => {
                        let (my_id, ts, peer_addrs) = {
                            let mut s = state.lock().unwrap();
                            let _timed_out = s.check_timeouts();
                            let addrs = s.connected_peer_addrs();
                            (s.my_participant_id, s.elapsed_ms(), addrs)
                        };

                        let hb_payload = Heartbeat.to_bytes();
                        let header = PacketHeader::new(
                            PacketType::Control,
                            my_id,
                            0,
                            ts,
                            hb_payload.len() as u16,
                        );
                        let packet = Packet::new(header, hb_payload).to_bytes();

                        for addr in peer_addrs {
                            if let Err(e) = transport.send_to(&packet, addr).await {
                                log::debug!("Heartbeat send failed to {addr}: {e}");
                            }
                        }
                    }
                }
            }
        });
    }
}
