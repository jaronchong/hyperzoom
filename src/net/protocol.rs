/// 12-byte packet header, big-endian on the wire.
///
/// Wire layout:
///   byte 0:       version (2 bits) | padding (1 bit) | type (5 bits)
///   byte 1:       participant_id (u8)
///   bytes 2..4:   sequence number (u16 big-endian)
///   bytes 4..8:   timestamp_ms (u32 big-endian)
///   bytes 8..10:  payload_length (u16 big-endian)
///   byte 10:      fragment_id (u8)
///   byte 11:      fragment_total (u8)

pub const HEADER_SIZE: usize = 12;
pub const PROTOCOL_VERSION: u8 = 0; // 2-bit version field

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    Audio = 0x01,
    VideoKeyframe = 0x02,
    VideoDelta = 0x03,
    Control = 0x04,
    Bye = 0x05,
}

impl PacketType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x01 => Some(Self::Audio),
            0x02 => Some(Self::VideoKeyframe),
            0x03 => Some(Self::VideoDelta),
            0x04 => Some(Self::Control),
            0x05 => Some(Self::Bye),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PacketHeader {
    pub version: u8,
    pub packet_type: PacketType,
    pub participant_id: u8,
    pub sequence: u16,
    pub timestamp_ms: u32,
    pub payload_length: u16,
    pub fragment_id: u8,
    pub fragment_total: u8,
}

impl PacketHeader {
    pub fn new(packet_type: PacketType, participant_id: u8, sequence: u16, timestamp_ms: u32, payload_length: u16) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type,
            participant_id,
            sequence,
            timestamp_ms,
            payload_length,
            fragment_id: 0,
            fragment_total: 1,
        }
    }

    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        // byte 0: version(2) | padding(1) | type(5)
        buf[0] = ((self.version & 0x03) << 6) | (self.packet_type as u8 & 0x1F);
        buf[1] = self.participant_id;
        buf[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        buf[4..8].copy_from_slice(&self.timestamp_ms.to_be_bytes());
        buf[8..10].copy_from_slice(&self.payload_length.to_be_bytes());
        buf[10] = self.fragment_id;
        buf[11] = self.fragment_total;
        buf
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_SIZE {
            return None;
        }
        let version = (buf[0] >> 6) & 0x03;
        let type_val = buf[0] & 0x1F;
        let packet_type = PacketType::from_u8(type_val)?;
        let participant_id = buf[1];
        let sequence = u16::from_be_bytes([buf[2], buf[3]]);
        let timestamp_ms = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let payload_length = u16::from_be_bytes([buf[8], buf[9]]);
        let fragment_id = buf[10];
        let fragment_total = buf[11];

        Some(Self {
            version,
            packet_type,
            participant_id,
            sequence,
            timestamp_ms,
            payload_length,
            fragment_id,
            fragment_total,
        })
    }
}

/// A complete packet: header + payload bytes.
#[derive(Debug, Clone)]
pub struct Packet {
    pub header: PacketHeader,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(header: PacketHeader, payload: Vec<u8>) -> Self {
        Self { header, payload }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_SIZE + self.payload.len());
        buf.extend_from_slice(&self.header.to_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        let header = PacketHeader::from_bytes(buf)?;
        let payload_start = HEADER_SIZE;
        let payload_end = payload_start + header.payload_length as usize;
        if buf.len() < payload_end {
            return None;
        }
        let payload = buf[payload_start..payload_end].to_vec();
        Some(Self { header, payload })
    }
}
