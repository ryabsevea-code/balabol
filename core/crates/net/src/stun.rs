use crate::NetError;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tracing::{debug, info};

pub const PUBLIC_STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
    "stun.sipgate.net:3478",
];

/// Create an RFC 5389 STUN Binding Request message (20 bytes).
pub fn create_binding_request(transaction_id: &[u8; 12]) -> [u8; 20] {
    let mut buf = [0u8; 20];
    buf[0..2].copy_from_slice(&0x0001u16.to_be_bytes()); // Binding Request
    buf[2..4].copy_from_slice(&0x0000u16.to_be_bytes()); // Message Length = 0
    buf[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes()); // Magic Cookie
    buf[8..20].copy_from_slice(transaction_id);
    buf
}

/// Parse an RFC 5389 STUN Binding Success Response and extract the public reflexive address.
pub fn parse_binding_response(buf: &[u8], transaction_id: &[u8; 12]) -> Option<SocketAddr> {
    if buf.len() < 20 {
        return None;
    }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != 0x0101 {
        // Not a Binding Success Response
        return None;
    }
    let msg_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let magic = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if magic != 0x2112A442 {
        return None;
    }
    if &buf[8..20] != transaction_id {
        return None;
    }

    let mut offset = 20;
    let end = (20 + msg_len).min(buf.len());
    let mut mapped_addr = None;

    while offset + 4 <= end {
        let attr_type = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let attr_len = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
        offset += 4;
        if offset + attr_len > end {
            break;
        }
        let attr_val = &buf[offset..offset + attr_len];

        if attr_type == 0x0020 && attr_len >= 8 {
            // XOR-MAPPED-ADDRESS
            let family = attr_val[1];
            let x_port = u16::from_be_bytes([attr_val[2], attr_val[3]]);
            let port = x_port ^ 0x2112;

            if family == 0x01 && attr_len >= 8 {
                // IPv4
                let x_ip = u32::from_be_bytes([attr_val[4], attr_val[5], attr_val[6], attr_val[7]]);
                let ip = Ipv4Addr::from(x_ip ^ 0x2112A442);
                return Some(SocketAddr::new(IpAddr::V4(ip), port));
            } else if family == 0x02 && attr_len >= 20 {
                // IPv6
                let mut xor_key = [0u8; 16];
                xor_key[0..4].copy_from_slice(&0x2112A442u32.to_be_bytes());
                xor_key[4..16].copy_from_slice(transaction_id);
                let mut ip_bytes = [0u8; 16];
                for i in 0..16 {
                    ip_bytes[i] = attr_val[4 + i] ^ xor_key[i];
                }
                let ip = Ipv6Addr::from(ip_bytes);
                return Some(SocketAddr::new(IpAddr::V6(ip), port));
            }
        } else if attr_type == 0x0001 && attr_len >= 8 {
            // MAPPED-ADDRESS (fallback)
            let family = attr_val[1];
            let port = u16::from_be_bytes([attr_val[2], attr_val[3]]);
            if family == 0x01 {
                let ip = Ipv4Addr::new(attr_val[4], attr_val[5], attr_val[6], attr_val[7]);
                mapped_addr = Some(SocketAddr::new(IpAddr::V4(ip), port));
            }
        }

        // Attributes are padded to multiple of 4 bytes
        let padding = (4 - (attr_len % 4)) % 4;
        offset += attr_len + padding;
    }

    mapped_addr
}

/// Query a single STUN server over a UDP socket.
pub async fn query_stun(
    socket: &tokio::net::UdpSocket,
    stun_server: &str,
    timeout: Duration,
) -> Result<SocketAddr, NetError> {
    use rand::RngCore;
    let mut transaction_id = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut transaction_id);

    let req = create_binding_request(&transaction_id);

    let addrs = tokio::net::lookup_host(stun_server).await?;
    let target = addrs.into_iter().next().ok_or_else(|| {
        NetError::PeerNotFound(format!("Could not resolve STUN server {}", stun_server))
    })?;

    socket.send_to(&req, target).await?;

    let mut buf = [0u8; 1024];
    let res = tokio::time::timeout(timeout, socket.recv_from(&mut buf))
        .await
        .map_err(|_| NetError::Timeout)??;

    parse_binding_response(&buf[..res.0], &transaction_id)
        .ok_or_else(|| NetError::PeerNotFound("Invalid or empty STUN response".to_string()))
}

/// Automatically query public STUN servers to discover global public IP and reflexive port.
pub async fn discover_public_address(
    socket: &tokio::net::UdpSocket,
) -> Result<SocketAddr, NetError> {
    for &server in PUBLIC_STUN_SERVERS {
        debug!("Querying STUN server {}", server);
        match query_stun(socket, server, Duration::from_millis(1500)).await {
            Ok(addr) => {
                info!("Public internet endpoint discovered via {}: {}", server, addr);
                return Ok(addr);
            }
            Err(e) => {
                debug!("STUN server {} query failed: {}", server, e);
            }
        }
    }
    Err(NetError::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stun_request_format() {
        let tid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let req = create_binding_request(&tid);
        assert_eq!(req.len(), 20);
        assert_eq!(u16::from_be_bytes([req[0], req[1]]), 0x0001); // Binding Request
        assert_eq!(u16::from_be_bytes([req[2], req[3]]), 0x0000); // Length = 0
        assert_eq!(u32::from_be_bytes([req[4], req[5], req[6], req[7]]), 0x2112A442); // Cookie
        assert_eq!(&req[8..20], &tid);
    }

    #[test]
    fn test_stun_parse_xor_mapped_ipv4() {
        let tid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let mut resp = Vec::new();
        // Header
        resp.extend_from_slice(&0x0101u16.to_be_bytes()); // Binding Success
        resp.extend_from_slice(&12u16.to_be_bytes());   // Length = 12
        resp.extend_from_slice(&0x2112A442u32.to_be_bytes()); // Cookie
        resp.extend_from_slice(&tid);

        // Attribute XOR-MAPPED-ADDRESS (0x0020, len 8)
        let port: u16 = 42420;
        let x_port = port ^ 0x2112;
        let ip_u32 = u32::from(Ipv4Addr::new(85, 140, 20, 10));
        let x_ip = ip_u32 ^ 0x2112A442;

        resp.extend_from_slice(&0x0020u16.to_be_bytes());
        resp.extend_from_slice(&8u16.to_be_bytes());
        resp.push(0); // Reserved
        resp.push(1); // IPv4
        resp.extend_from_slice(&x_port.to_be_bytes());
        resp.extend_from_slice(&x_ip.to_be_bytes());

        let parsed = parse_binding_response(&resp, &tid).unwrap();
        assert_eq!(parsed.ip(), IpAddr::V4(Ipv4Addr::new(85, 140, 20, 10)));
        assert_eq!(parsed.port(), 42420);
    }
}
