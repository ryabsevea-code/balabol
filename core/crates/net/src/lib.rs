use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

#[derive(Error, Debug)]
pub enum NetError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO network error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Peer not found: {0}")]
    PeerNotFound(String),
    #[error("Connection timeout")]
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerGossipEntry {
    pub user_id: String,
    pub public_key_hex: String,
    pub endpoint: String,
    pub last_seen_ms: u64,
    pub nat_type: String,
}

/// Discovered peer on local network (Wi-Fi or LAN) without any central servers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredLanPeer {
    pub user_id: String,
    pub display_name: String,
    pub endpoint: String,
    pub public_key_hex: String,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LanBeaconMessage {
    magic: String,
    user_id: String,
    display_name: String,
    listening_port: u16,
    public_key_hex: String,
}

pub mod stun;
pub mod upnp;
pub use stun::discover_public_address;
pub use upnp::UpnpManager;

/// Unified Balabol P2P packet format multiplexed over QUIC and UDP streams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PacketPayload {
    /// Latency & jitter probing for SFU elections and ping indicators
    Ping {
        seq: u64,
        client_timestamp_ms: u64,
    },
    Pong {
        seq: u64,
        client_timestamp_ms: u64,
        server_timestamp_ms: u64,
    },
    /// Periodic Gossip table of known peer IP addresses
    GossipTable {
        entries: Vec<PeerGossipEntry>,
    },
    /// CRDT Delta sync (Loro updates)
    CrdtSync {
        channel_id: String,
        delta_bytes_base64: String,
    },
    /// End-to-End Encrypted direct text message
    EncryptedDirectMessage {
        recipient_user_id: String,
        ciphertext_hex: String,
    },
    /// Low-latency real-time voice packet (PCM / Opus)
    VoiceAudio {
        channel_id: String,
        speaker_id: String,
        seq: u32,
        timestamp_ms: u32,
        samples: Vec<f32>,
    },
    /// Video / Screen sharing frame packet (JPEG-compressed)
    ScreenVideo {
        channel_id: String,
        streamer_id: String,
        frame_seq: u32,
        is_keyframe: bool,
        payload: Vec<u8>,
    },
    /// SFU Heartbeat
    SfuHeartbeat {
        term: u64,
        leader_id: String,
        timestamp_ms: u64,
    },
    /// Forward relayed packet through Supernode for symmetric CGNAT clients
    RelayForward {
        target_user_id: String,
        inner_packet_bytes: Vec<u8>,
    },
    /// Host role advertisement and clustering status
    HostHeartbeat {
        host_id: String,
        channel_id: String,
        is_primary: bool,
        participant_count: usize,
        timestamp_ms: u64,
    },
    /// Text message broadcast to channel
    TextMessage {
        channel_id: String,
        message_id: String,
        author_id: String,
        content: String,
        timestamp_ms: i64,
        signature_hex: Option<String>,
    },
    /// Bidirectional P2P handshake — carries the sender's own public endpoint
    /// so the receiver can register the sender and reply symmetrically.
    InviteExchange {
        my_pubkey: String,
        my_endpoint: String,   // "ip:port" of the sender (public or LAN)
        display_name: String,
    },
    /// Invite a peer to join a room
    RoomInvite {
        room_id: String,
        room_name: String,
        room_emoji: String,
        inviter_id: String,
        inviter_name: String,
    },
    /// Room voice state update (when someone joins or leaves a room's voice call)
    RoomVoiceState {
        room_id: String,
        user_id: String,
        is_joined: bool,
        is_muted: bool,
        is_deafened: bool,
    },
}

/// Determine the reachable local LAN IP address of this computer (e.g. 192.168.x.x)
/// using UDP socket routing without sending external packets.
pub fn detect_local_lan_ip() -> std::net::IpAddr {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
                    return addr.ip();
                }
            }
        }
    }
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalabolPacket {
    pub sender_id: String,
    pub signature_hex: Option<String>,
    pub payload: PacketPayload,
}

impl BalabolPacket {
    pub fn new(sender_id: String, payload: PacketPayload) -> Self {
        Self {
            sender_id,
            signature_hex: None,
            payload,
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, NetError> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NetError> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Gossip manager that periodically exchanges known IP addresses and peer routes.
pub struct GossipManager {
    my_user_id: String,
    peer_table: Arc<RwLock<HashMap<String, PeerGossipEntry>>>,
}

impl GossipManager {
    pub fn new(my_user_id: String) -> Self {
        Self {
            my_user_id,
            peer_table: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add or update a peer in the local gossip table.
    pub fn update_peer(&self, entry: PeerGossipEntry) {
        if entry.user_id != self.my_user_id {
            self.peer_table.write().insert(entry.user_id.clone(), entry);
        }
    }

    /// Merge peer table received from a remote peer during gossip exchange.
    pub fn merge_gossip_table(&self, incoming: Vec<PeerGossipEntry>) -> usize {
        let mut table = self.peer_table.write();
        let mut updated_count = 0;

        for entry in incoming {
            if entry.user_id == self.my_user_id {
                continue;
            }

            match table.get_mut(&entry.user_id) {
                Some(existing) => {
                    if entry.last_seen_ms > existing.last_seen_ms {
                        *existing = entry;
                        updated_count += 1;
                    }
                }
                None => {
                    table.insert(entry.user_id.clone(), entry);
                    updated_count += 1;
                }
            }
        }

        updated_count
    }

    /// Export a random sample of known peers to transmit in a Gossip packet.
    pub fn sample_peers_for_gossip(&self, limit: usize) -> Vec<PeerGossipEntry> {
        let table = self.peer_table.read();
        table.values().take(limit).cloned().collect()
    }

    /// Total number of known peers in routing table.
    pub fn peer_count(&self) -> usize {
        self.peer_table.read().len()
    }
}

/// Serverless LAN Auto-Discovery: broadcasts and listens for UDP beacons on the local network.
pub struct LanDiscovery {
    my_user_id: String,
    display_name: String,
    listening_port: u16,
    public_key_hex: String,
    peers: Arc<RwLock<HashMap<String, DiscoveredLanPeer>>>,
}

impl LanDiscovery {
    const BEACON_PORT: u16 = 42424;
    const MAGIC: &'static str = "BALABOL_LAN_BEACON";

    pub fn new(
        my_user_id: String,
        display_name: String,
        listening_port: u16,
        public_key_hex: String,
    ) -> Self {
        Self {
            my_user_id,
            display_name,
            listening_port,
            public_key_hex,
            peers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get all actively discovered peers on the local network (seen in last 10 seconds).
    pub fn get_active_peers(&self) -> Vec<DiscoveredLanPeer> {
        let current = now_ms();
        let mut table = self.peers.write();
        // Evict peers unseen for > 10s
        table.retain(|_, p| current.saturating_sub(p.last_seen_ms) < 10_000);
        table.values().cloned().collect()
    }

    /// Add or update a discovered peer manually or from beacon.
    pub fn record_peer(&self, peer: DiscoveredLanPeer) {
        if peer.user_id != self.my_user_id {
            self.peers.write().insert(peer.user_id.clone(), peer);
        }
    }

    /// Start background broadcaster and listener tasks.
    pub fn start_background_tasks(&self) {
        let my_user_id = self.my_user_id.clone();
        let display_name = self.display_name.clone();
        let listening_port = self.listening_port;
        let public_key_hex = self.public_key_hex.clone();
        let peers_map = self.peers.clone();

        // 1. Broadcaster task: every 2.5s sends UDP broadcast beacon
        tokio::spawn(async move {
            let socket = match UdpSocket::bind("0.0.0.0:0").await {
                Ok(s) => {
                    let _ = s.set_broadcast(true);
                    s
                }
                Err(e) => {
                    warn!("Failed to bind LAN broadcast sender: {}", e);
                    return;
                }
            };

            let beacon = LanBeaconMessage {
                magic: Self::MAGIC.to_string(),
                user_id: my_user_id,
                display_name,
                listening_port,
                public_key_hex,
            };

            let beacon_bytes = match serde_json::to_vec(&beacon) {
                Ok(b) => b,
                Err(_) => return,
            };

            let target_addr = format!("255.255.255.255:{}", Self::BEACON_PORT);
            let mut interval = tokio::time::interval(Duration::from_millis(2500));

            loop {
                interval.tick().await;
                let _ = socket.send_to(&beacon_bytes, &target_addr).await;
            }
        });

        // 2. Listener task: binds BEACON_PORT and collects incoming beacons
        let my_id = self.my_user_id.clone();
        tokio::spawn(async move {
            let bind_addr = format!("0.0.0.0:{}", Self::BEACON_PORT);
            let socket = match UdpSocket::bind(&bind_addr).await {
                Ok(s) => {
                    let _ = s.set_broadcast(true);
                    s
                }
                Err(e) => {
                    debug!("LAN Beacon listener could not bind port {}: {}. Running in client-only mode.", Self::BEACON_PORT, e);
                    return;
                }
            };

            let mut buf = [0u8; 2048];
            loop {
                if let Ok((len, src_addr)) = socket.recv_from(&mut buf).await {
                    if let Ok(beacon) = serde_json::from_slice::<LanBeaconMessage>(&buf[..len]) {
                        if beacon.magic == Self::MAGIC && beacon.user_id != my_id {
                            let endpoint = format!("{}:{}", src_addr.ip(), beacon.listening_port);
                            peers_map.write().insert(
                                beacon.user_id.clone(),
                                DiscoveredLanPeer {
                                    user_id: beacon.user_id,
                                    display_name: beacon.display_name,
                                    endpoint,
                                    public_key_hex: beacon.public_key_hex,
                                    last_seen_ms: now_ms(),
                                },
                            );
                        }
                    }
                }
            }
        });
    }
}

/// Real-time UDP network transport for P2P audio, text, and CRDT synchronization.
pub struct P2pUdpTransport {
    socket: Arc<UdpSocket>,
    local_port: u16,
}

impl P2pUdpTransport {
    /// Bind UDP transport on preferred port (e.g. 42420).
    /// If preferred port is occupied, automatically falls back to OS-assigned ephemeral port (0.0.0.0:0).
    pub async fn bind(preferred_port: u16) -> Result<Self, NetError> {
        let socket = match UdpSocket::bind(format!("0.0.0.0:{}", preferred_port)).await {
            Ok(s) => s,
            Err(_) => UdpSocket::bind("0.0.0.0:0").await?,
        };

        let local_port = socket.local_addr()?.port();
        info!("P2P UDP Transport bound to port {}", local_port);

        Ok(Self {
            socket: Arc::new(socket),
            local_port,
        })
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub fn socket_ref(&self) -> Arc<UdpSocket> {
        self.socket.clone()
    }

    /// Send a Balabol packet directly to a peer's endpoint (e.g. "192.168.1.50:42420").
    pub async fn send_packet(&self, target_endpoint: &str, packet: &BalabolPacket) -> Result<(), NetError> {
        let bytes = packet.to_bytes()?;
        self.socket.send_to(&bytes, target_endpoint).await?;
        Ok(())
    }

    /// Send audio frame PCM samples directly to a target peer or SFU router.
    pub async fn send_audio_frame(
        &self,
        target_endpoint: &str,
        channel_id: &str,
        speaker_id: &str,
        seq: u32,
        samples: &[f32],
    ) -> Result<(), NetError> {
        let packet = BalabolPacket::new(
            speaker_id.to_string(),
            PacketPayload::VoiceAudio {
                channel_id: channel_id.to_string(),
                speaker_id: speaker_id.to_string(),
                seq,
                timestamp_ms: (now_ms() & 0xFFFFFFFF) as u32,
                samples: samples.to_vec(),
            },
        );
        self.send_packet(target_endpoint, &packet).await
    }

    /// Receive the next incoming packet from the UDP socket.
    pub async fn recv_packet(&self) -> Result<(BalabolPacket, SocketAddr), NetError> {
        let mut buf = [0u8; 65535];
        let (len, src) = self.socket.recv_from(&mut buf).await?;
        let packet = BalabolPacket::from_bytes(&buf[..len])?;
        Ok((packet, src))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_serialization_roundtrip() {
        let packet = BalabolPacket::new(
            "bala:alice".to_string(),
            PacketPayload::VoiceAudio {
                channel_id: "chan-lounge".to_string(),
                speaker_id: "bala:alice".to_string(),
                seq: 42,
                timestamp_ms: 12345,
                samples: vec![0.1, -0.2, 0.3],
            },
        );

        let bytes = packet.to_bytes().unwrap();
        let parsed = BalabolPacket::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.sender_id, "bala:alice");
        if let PacketPayload::VoiceAudio { seq, samples, .. } = parsed.payload {
            assert_eq!(seq, 42);
            assert_eq!(samples, vec![0.1, -0.2, 0.3]);
        } else {
            panic!("Wrong payload variant");
        }
    }

    #[test]
    fn test_lan_discovery_records_peers() {
        let discovery = LanDiscovery::new(
            "bala:my_node".to_string(),
            "My Device".to_string(),
            42420,
            "pub_hex_1".to_string(),
        );

        discovery.record_peer(DiscoveredLanPeer {
            user_id: "bala:friend_pc".to_string(),
            display_name: "Friend's Laptop".to_string(),
            endpoint: "192.168.1.105:42420".to_string(),
            public_key_hex: "pub_hex_2".to_string(),
            last_seen_ms: now_ms(),
        });

        let peers = discovery.get_active_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].display_name, "Friend's Laptop");
    }

    #[tokio::test]
    async fn test_udp_transport_send_and_recv() {
        let transport_a = P2pUdpTransport::bind(0).await.unwrap();
        let transport_b = P2pUdpTransport::bind(0).await.unwrap();

        let endpoint_b = format!("127.0.0.1:{}", transport_b.local_port());

        let ping_packet = BalabolPacket::new(
            "bala:alice".to_string(),
            PacketPayload::Ping {
                seq: 1,
                client_timestamp_ms: 1000,
            },
        );

        transport_a.send_packet(&endpoint_b, &ping_packet).await.unwrap();

        let (received_packet, src_addr) = transport_b.recv_packet().await.unwrap();
        assert_eq!(received_packet.sender_id, "bala:alice");
        assert_eq!(src_addr.port(), transport_a.local_port());
    }
}
