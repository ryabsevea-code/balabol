use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tracing::{info, warn};

pub struct UpnpManager {
    local_port: u16,
    is_mapped: bool,
    external_addr: Option<SocketAddr>,
}

impl UpnpManager {
    pub fn new(local_port: u16) -> Self {
        Self {
            local_port,
            is_mapped: false,
            external_addr: None,
        }
    }

    /// Attempt to map the local UDP port on the gateway router via UPnP IGD.
    /// Runs synchronously inside spawn_blocking to avoid blocking the async event loop.
    pub async fn try_map_port(&mut self) -> Result<SocketAddr, String> {
        let port = self.local_port;
        let result = tokio::task::spawn_blocking(move || {
            let options = igd_next::SearchOptions {
                timeout: Some(Duration::from_millis(2000)),
                ..Default::default()
            };

            let gateway = igd_next::search_gateway(options)
                .map_err(|e| format!("UPnP search gateway failed: {}", e))?;

            let ext_ip = gateway
                .get_external_ip()
                .map_err(|e| format!("UPnP get_external_ip failed: {}", e))?;

            let local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
            gateway
                .add_port(
                    igd_next::PortMappingProtocol::UDP,
                    port,
                    local_addr,
                    7200, // 2-hour lease
                    "Balabol P2P Voice",
                )
                .map_err(|e| format!("UPnP add_port failed: {}", e))?;

            Ok::<SocketAddr, String>(SocketAddr::new(ext_ip, port))

        })
        .await
        .map_err(|e| format!("Join error: {}", e))?;

        match result {
            Ok(addr) => {
                info!("UPnP successfully mapped port {} (External: {})", self.local_port, addr);
                self.is_mapped = true;
                self.external_addr = Some(addr);
                Ok(addr)
            }
            Err(e) => {
                warn!("UPnP auto-mapping failed (normal if UPnP disabled on router): {}", e);
                Err(e)
            }
        }
    }

    pub fn is_mapped(&self) -> bool {
        self.is_mapped
    }

    pub fn external_addr(&self) -> Option<SocketAddr> {
        self.external_addr
    }
}
