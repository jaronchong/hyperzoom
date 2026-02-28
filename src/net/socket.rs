use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use super::protocol::{Packet, PacketType, HEADER_SIZE};

/// Events dispatched from the recv loop to consumers.
#[derive(Debug, Clone)]
pub enum InboundEvent {
    /// Opus-encoded audio frame with sender info.
    Audio {
        participant_id: u8,
        sequence: u16,
        timestamp_ms: u32,
        payload: Vec<u8>,
    },
    /// VP8 video frame fragment.
    Video {
        participant_id: u8,
        sequence: u16,
        timestamp_ms: u32,
        is_keyframe: bool,
        fragment_id: u8,
        fragment_total: u8,
        payload: Vec<u8>,
    },
    /// Control message payload (Hello, Welcome, PeerJoined, Heartbeat, Nack).
    Control {
        from: SocketAddr,
        participant_id: u8,
        payload: Vec<u8>,
    },
    /// Remote peer sent BYE.
    Bye {
        participant_id: u8,
    },
}

/// Thin wrapper around a tokio UdpSocket for send/recv.
pub struct UdpTransport {
    pub socket: Arc<UdpSocket>,
}

impl UdpTransport {
    /// Bind to `0.0.0.0:<port>`.
    pub async fn bind(port: u16) -> Result<Self, String> {
        let addr = format!("0.0.0.0:{port}");
        let socket = UdpSocket::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind UDP socket on {addr}: {e}"))?;
        log::info!("UDP socket bound on {addr}");
        Ok(Self {
            socket: Arc::new(socket),
        })
    }

    /// Send raw bytes to a specific address.
    pub async fn send_to(&self, buf: &[u8], target: SocketAddr) -> Result<(), String> {
        self.socket
            .send_to(buf, target)
            .await
            .map_err(|e| format!("UDP send_to failed: {e}"))?;
        Ok(())
    }

    /// Spawn a tokio task that receives packets and dispatches them as InboundEvents.
    /// Returns the mpsc receiver for the consumer.
    pub fn spawn_recv_loop(
        &self,
    ) -> mpsc::UnboundedReceiver<InboundEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let socket = self.socket.clone();

        tokio::spawn(async move {
            let mut buf = [0u8; 1500]; // MTU-sized buffer
            loop {
                let (len, from) = match socket.recv_from(&mut buf).await {
                    Ok(result) => result,
                    Err(e) => {
                        log::warn!("UDP recv error: {e}");
                        continue;
                    }
                };

                if len < HEADER_SIZE {
                    log::debug!("Ignoring undersized packet ({len} bytes) from {from}");
                    continue;
                }

                let packet = match Packet::from_bytes(&buf[..len]) {
                    Some(p) => p,
                    None => {
                        log::debug!("Failed to parse packet from {from}");
                        continue;
                    }
                };

                let event = match packet.header.packet_type {
                    PacketType::Audio => InboundEvent::Audio {
                        participant_id: packet.header.participant_id,
                        sequence: packet.header.sequence,
                        timestamp_ms: packet.header.timestamp_ms,
                        payload: packet.payload,
                    },
                    PacketType::VideoKeyframe | PacketType::VideoDelta => InboundEvent::Video {
                        participant_id: packet.header.participant_id,
                        sequence: packet.header.sequence,
                        timestamp_ms: packet.header.timestamp_ms,
                        is_keyframe: packet.header.packet_type == PacketType::VideoKeyframe,
                        fragment_id: packet.header.fragment_id,
                        fragment_total: packet.header.fragment_total,
                        payload: packet.payload,
                    },
                    PacketType::Control => InboundEvent::Control {
                        from,
                        participant_id: packet.header.participant_id,
                        payload: packet.payload,
                    },
                    PacketType::Bye => InboundEvent::Bye {
                        participant_id: packet.header.participant_id,
                    },
                };

                if tx.send(event).is_err() {
                    log::info!("Recv loop: channel closed, stopping");
                    break;
                }
            }
        });

        rx
    }
}
