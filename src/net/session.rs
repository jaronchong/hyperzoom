use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Shared session state accessed by UI, recv task, heartbeat task, and encode thread.
pub type SharedSessionState = Arc<Mutex<SessionState>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Guest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub participant_id: u8,
    pub name: String,
    pub addr: SocketAddr,
    pub state: PeerState,
    pub last_seen: Instant,
}

pub struct SessionState {
    pub role: Role,
    pub session_id: u32,
    pub my_participant_id: u8,
    pub my_name: String,
    pub peers: HashMap<u8, Peer>,
    pub next_participant_id: u8,
    pub seq_counter: u16,
    pub start_time: Instant,
    pub ended: bool,
}

impl SessionState {
    /// Create state for a host starting a new session.
    pub fn new_host(name: String) -> Self {
        let session_id = rand_session_id();
        log::info!("Created host session {session_id:#010X}");
        Self {
            role: Role::Host,
            session_id,
            my_participant_id: 1,
            my_name: name,
            peers: HashMap::new(),
            next_participant_id: 2, // host is 1, guests start at 2
            seq_counter: 0,
            start_time: Instant::now(),
            ended: false,
        }
    }

    /// Create state for a guest joining a session.
    /// participant_id and session_id are assigned after receiving Welcome.
    pub fn new_guest(name: String) -> Self {
        Self {
            role: Role::Guest,
            session_id: 0,
            my_participant_id: 0,
            my_name: name,
            peers: HashMap::new(),
            next_participant_id: 0,
            seq_counter: 0,
            start_time: Instant::now(),
            ended: false,
        }
    }

    /// Host assigns the next participant ID to a new guest.
    pub fn assign_participant_id(&mut self) -> u8 {
        let id = self.next_participant_id;
        self.next_participant_id = self.next_participant_id.wrapping_add(1);
        id
    }

    /// Get the next sequence number and increment.
    pub fn next_seq(&mut self) -> u16 {
        let seq = self.seq_counter;
        self.seq_counter = self.seq_counter.wrapping_add(1);
        seq
    }

    /// Milliseconds elapsed since session start.
    pub fn elapsed_ms(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }

    /// Update last_seen for a peer. Sets state to Connected if Connecting.
    pub fn touch_peer(&mut self, participant_id: u8) {
        if let Some(peer) = self.peers.get_mut(&participant_id) {
            peer.last_seen = Instant::now();
            if peer.state == PeerState::Connecting {
                peer.state = PeerState::Connected;
                log::info!("Peer {} ({}) is now Connected", peer.name, participant_id);
            }
        }
    }

    /// Check for peers that have timed out (>5 seconds since last_seen).
    /// Returns list of participant IDs that timed out.
    pub fn check_timeouts(&mut self) -> Vec<u8> {
        let timeout = std::time::Duration::from_secs(5);
        let now = Instant::now();
        let mut timed_out = Vec::new();

        for (id, peer) in &mut self.peers {
            if peer.state != PeerState::Disconnected && now.duration_since(peer.last_seen) > timeout
            {
                log::warn!("Peer {} ({}) timed out", peer.name, id);
                peer.state = PeerState::Disconnected;
                timed_out.push(*id);
            }
        }

        timed_out
    }

    /// Add a new peer to the session.
    pub fn add_peer(&mut self, participant_id: u8, name: String, addr: SocketAddr) {
        log::info!("Adding peer: {name} (id={participant_id}) at {addr}");
        self.peers.insert(
            participant_id,
            Peer {
                participant_id,
                name,
                addr,
                state: PeerState::Connecting,
                last_seen: Instant::now(),
            },
        );
    }

    /// Get addresses of all active (non-disconnected) peers.
    /// Includes both Connecting and Connected peers so audio/heartbeats
    /// flow immediately after handshake.
    pub fn connected_peer_addrs(&self) -> Vec<SocketAddr> {
        self.peers
            .values()
            .filter(|p| p.state != PeerState::Disconnected)
            .map(|p| p.addr)
            .collect()
    }
}

fn rand_session_id() -> u32 {
    // Simple pseudo-random from system time
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (t.as_nanos() & 0xFFFF_FFFF) as u32
}
