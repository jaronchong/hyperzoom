use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use igd_next::aio::tokio::Tokio;

/// Holds a UPnP port mapping that can be removed on shutdown.
pub struct PortMapping {
    gateway: igd_next::aio::Gateway<Tokio>,
    external_port: u16,
}

impl PortMapping {
    /// Attempt to create a UPnP port mapping for the given local port.
    /// Returns None (with a warning log) if UPnP is unavailable.
    pub async fn create(local_port: u16) -> Option<Self> {
        let gateway = match igd_next::aio::tokio::search_gateway(Default::default()).await {
            Ok(gw) => {
                log::info!("UPnP gateway found: {}", gw.addr);
                gw
            }
            Err(e) => {
                log::warn!("UPnP gateway discovery failed: {e}");
                return None;
            }
        };

        let local_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, local_port));

        let external_addr = match gateway
            .get_any_address(
                igd_next::PortMappingProtocol::UDP,
                local_addr,
                60,
                "HyperZoom audio",
            )
            .await
        {
            Ok(addr) => {
                log::info!("UPnP mapped external {addr} → local port {local_port}");
                addr
            }
            Err(e) => {
                log::warn!("UPnP port mapping failed: {e}");
                return None;
            }
        };

        Some(Self {
            gateway,
            external_port: external_addr.port(),
        })
    }

    /// Remove the UPnP port mapping. Best-effort.
    pub async fn remove(self) {
        match self
            .gateway
            .remove_port(igd_next::PortMappingProtocol::UDP, self.external_port)
            .await
        {
            Ok(()) => log::info!("UPnP mapping removed for port {}", self.external_port),
            Err(e) => log::warn!("UPnP mapping removal failed: {e}"),
        }
    }

    pub fn external_port(&self) -> u16 {
        self.external_port
    }
}
