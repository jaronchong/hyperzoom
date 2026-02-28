use std::net::{Ipv4Addr, SocketAddrV4};

/// Control message sub-types carried inside a Control packet's payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControlType {
    Hello = 0x01,
    Welcome = 0x02,
    PeerJoined = 0x03,
    Heartbeat = 0x04,
    Nack = 0x05,
}

impl ControlType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x01 => Some(Self::Hello),
            0x02 => Some(Self::Welcome),
            0x03 => Some(Self::PeerJoined),
            0x04 => Some(Self::Heartbeat),
            0x05 => Some(Self::Nack),
            _ => None,
        }
    }
}

// --- Wire helpers for SocketAddrV4 (4 + 2 = 6 bytes) ---

fn write_addr(addr: &SocketAddrV4, buf: &mut Vec<u8>) {
    buf.extend_from_slice(&addr.ip().octets());
    buf.extend_from_slice(&addr.port().to_be_bytes());
}

fn read_addr(buf: &[u8], offset: usize) -> Option<(SocketAddrV4, usize)> {
    if buf.len() < offset + 6 {
        return None;
    }
    let ip = Ipv4Addr::new(buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]);
    let port = u16::from_be_bytes([buf[offset + 4], buf[offset + 5]]);
    Some((SocketAddrV4::new(ip, port), offset + 6))
}

// --- Hello: guest → host ---
// Wire: [ctrl_type=0x01] [name_len: u8] [name: utf8...]

#[derive(Debug, Clone)]
pub struct Hello {
    pub name: String,
}

impl Hello {
    pub fn to_bytes(&self) -> Vec<u8> {
        let name_bytes = self.name.as_bytes();
        let mut buf = Vec::with_capacity(2 + name_bytes.len());
        buf.push(ControlType::Hello as u8);
        buf.push(name_bytes.len() as u8);
        buf.extend_from_slice(name_bytes);
        buf
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 2 {
            return None;
        }
        let name_len = buf[1] as usize;
        if buf.len() < 2 + name_len {
            return None;
        }
        let name = String::from_utf8(buf[2..2 + name_len].to_vec()).ok()?;
        Some(Self { name })
    }
}

// --- Welcome: host → guest ---
// Wire: [ctrl_type=0x02] [session_id: u32 BE] [assigned_participant_id: u8]

#[derive(Debug, Clone)]
pub struct Welcome {
    pub session_id: u32,
    pub assigned_participant_id: u8,
}

impl Welcome {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(6);
        buf.push(ControlType::Welcome as u8);
        buf.extend_from_slice(&self.session_id.to_be_bytes());
        buf.push(self.assigned_participant_id);
        buf
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 6 {
            return None;
        }
        let session_id = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
        let assigned_participant_id = buf[5];
        Some(Self {
            session_id,
            assigned_participant_id,
        })
    }
}

// --- PeerJoined: host → existing peers ---
// Wire: [ctrl_type=0x03] [participant_id: u8] [addr: 6 bytes] [name_len: u8] [name: utf8...]

#[derive(Debug, Clone)]
pub struct PeerJoined {
    pub participant_id: u8,
    pub addr: SocketAddrV4,
    pub name: String,
}

impl PeerJoined {
    pub fn to_bytes(&self) -> Vec<u8> {
        let name_bytes = self.name.as_bytes();
        let mut buf = Vec::with_capacity(8 + name_bytes.len());
        buf.push(ControlType::PeerJoined as u8);
        buf.push(self.participant_id);
        write_addr(&self.addr, &mut buf);
        buf.push(name_bytes.len() as u8);
        buf.extend_from_slice(name_bytes);
        buf
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 3 {
            return None;
        }
        let participant_id = buf[1];
        let (addr, offset) = read_addr(buf, 2)?;
        if buf.len() < offset + 1 {
            return None;
        }
        let name_len = buf[offset] as usize;
        if buf.len() < offset + 1 + name_len {
            return None;
        }
        let name = String::from_utf8(buf[offset + 1..offset + 1 + name_len].to_vec()).ok()?;
        Some(Self {
            participant_id,
            addr,
            name,
        })
    }
}

// --- Heartbeat ---
// Wire: [ctrl_type=0x04]

#[derive(Debug, Clone)]
pub struct Heartbeat;

impl Heartbeat {
    pub fn to_bytes(&self) -> Vec<u8> {
        vec![ControlType::Heartbeat as u8]
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None;
        }
        Some(Self)
    }
}

// --- Nack: request retransmission ---
// Wire: [ctrl_type=0x05] [seq_start: u16 BE] [count: u8]

#[derive(Debug, Clone)]
pub struct Nack {
    pub seq_start: u16,
    pub count: u8,
}

impl Nack {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4);
        buf.push(ControlType::Nack as u8);
        buf.extend_from_slice(&self.seq_start.to_be_bytes());
        buf.push(self.count);
        buf
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 4 {
            return None;
        }
        let seq_start = u16::from_be_bytes([buf[1], buf[2]]);
        let count = buf[3];
        Some(Self { seq_start, count })
    }
}

/// Parse a control payload's first byte to determine its type.
pub fn parse_control_type(payload: &[u8]) -> Option<ControlType> {
    payload.first().and_then(|&b| ControlType::from_u8(b))
}
