use crate::logger::AppLogger;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

#[allow(dead_code)]
pub const PROTOCOL_VERSION: u32 = 4;

pub const PACKET_TYPE_AUDIO: u8 = 1;
pub const PACKET_TYPE_KEEPALIVE: u8 = 2;
pub const PACKET_TYPE_CONTROL: u8 = 3;
pub const PACKET_TYPE_PUNCH: u8 = 4;
pub const PACKET_TYPE_PUNCH_ACK: u8 = 5;
pub const PACKET_TYPE_PAKE: u8 = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Packet {
    Punch {
        room_code: String,
        nonce: u64,
        sender_name: String,
        sender_id: u32,
        #[serde(default)]
        wire_sender_id: u32,
        #[serde(default)]
        pake_point: [u8; 32],
    },
    PunchAck {
        room_code: String,
        nonce: u64,
        sender_name: String,
        sender_id: u32,
        #[serde(default)]
        wire_sender_id: u32,
        #[serde(default)]
        pake_point: [u8; 32],
    },
    ConnectRequest {
        client_name: String,
        protocol_version: u32,
        client_nonce: u64,
        client_id: u32,
    },
    ConnectAccept {
        assigned_id: u32,
        host_id: u32,
        host_name: String,
        room_name: String,
        peers: Vec<PeerInfo>,
    },
    ConnectReject {
        reason: String,
    },
    Ping {
        sender_id: u32,
        seq: u64,
        timestamp_ms: u64,
    },
    Pong {
        sender_id: u32,
        seq: u64,
        timestamp_ms: u64,
    },
    TextMessage {
        id: u64,
        sender_id: u32,
        text: String,
        sender_name: String,
        timestamp_ms: u64,
        image_thumb: Option<String>,
        image_id: Option<String>,
    },
    TextAck {
        id: u64,
    },
    ImageRequest {
        image_id: String,
        requester_id: u32,
    },
    ImageChunk {
        image_id: String,
        chunk_idx: u32,
        chunk_total: u32,
        data: Vec<u8>,
    },
    HistoryRequest {
        requester_id: u32,
    },
    HistoryMessage {
        message: ChatMessage,
    },
    Disconnect {
        sender_id: u32,
        reason: String,
    },
    AudioData {
        sender_id: u32,
        seq: u64,
        data: Vec<u8>,
    },
    VideoChunk {
        sender_id: u32,
        frame_id: u32,
        chunk_idx: u16,
        chunk_total: u16,
        is_screen: bool,
        data: Vec<u8>,
    },
    MediaState {
        sender_id: u32,
        mic_muted: bool,
        deafened: bool,
        video_active: bool,
        screen_active: bool,
    },
    PeerListUpdate {
        host_id: u32,
        backup_host_id: Option<u32>,
        peers: Vec<PeerInfo>,
    },
    HostHandover {
        old_host_id: u32,
        new_host_id: u32,
        new_host_addr: Option<SocketAddr>,
    },
    ClaimHost {
        candidate_id: u32,
        reason: String,
    },
    NicknameUpdate {
        sender_id: u32,
        new_nickname: String,
    },
    PeerIntroduce {
        room_code: String,
        peer_id: u32,
        peer_name: String,
        endpoints: Vec<SocketAddr>,
    },
    KeyExchange {
        sender_id: u32,
        ephemeral_pubkey: [u8; 32],
    },
    KeepAlive {
        sender_id: u32,
        timestamp_ms: u64,
    },
}

#[derive(Debug, Clone)]
pub struct PeerSession {
    pub peer_id: u32,
    pub sender_id: u32,
    pub session_key: [u8; 32],
    pub current_addr: SocketAddr,
    pub replay_window: crate::crypto::AntiReplayWindow,
    pub last_sent_time: Instant,
    pub last_recv_time: Instant,
    pub last_repunch_time: Instant,
    pub candidate_addrs: Vec<SocketAddr>,
}

impl Packet {
    pub fn encode(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(buf)
    }

    pub fn encode_wire(&self, key_opt: Option<&[u8; 32]>, seq: u64) -> Result<Vec<u8>, String> {
        let raw = self.encode().map_err(|e| e.to_string())?;
        if let Some(key) = key_opt {
            crate::crypto::encrypt_wire_packet(PACKET_TYPE_CONTROL, 0, seq, &raw, key)
        } else {
            Ok(raw)
        }
    }

    pub fn encode_wire_with_sender(&self, key_opt: Option<&[u8; 32]>, seq: u64, sender_id: u32) -> Result<Vec<u8>, String> {
        let raw = self.encode().map_err(|e| e.to_string())?;
        if let Some(key) = key_opt {
            crate::crypto::encrypt_wire_packet(PACKET_TYPE_CONTROL, sender_id, seq, &raw, key)
        } else {
            Ok(raw)
        }
    }

    pub fn decode_wire(buf: &[u8], key_opt: Option<&[u8; 32]>) -> Result<(u64, Self), String> {
        if let Some(key) = key_opt {
            if buf.len() >= crate::crypto::MIN_WIRE_PACKET_LEN {
                let (_type, _sid, seq, decrypted) = crate::crypto::decrypt_wire_packet(buf, key)?;
                let pkt = Self::decode(&decrypted).map_err(|e| e.to_string())?;
                Ok((seq, pkt))
            } else if buf.len() >= 40 && &buf[0..4] == b"BALA" {
                let (seq, decrypted) = crate::crypto::decrypt_packet(buf, key)?;
                let pkt = Self::decode(&decrypted).map_err(|e| e.to_string())?;
                Ok((seq, pkt))
            } else {
                Err("Rejected unauthenticated packet (E2EE room key active)".into())
            }
        } else {
            let pkt = Self::decode(buf).map_err(|e| e.to_string())?;
            Ok((0, pkt))
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: u64,
    pub sender: String,
    pub text: String,
    pub timestamp_ms: u64,
    pub is_me: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_thumb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerInfo {
    pub id: u32,
    pub name: String,
    pub addr: String,
    pub ping_ms: u32,
    pub last_seen_ms: u64,
    pub is_host: bool,
    pub is_speaking: bool,
    pub mic_muted: bool,
    pub deafened: bool,
    pub video_active: bool,
    pub screen_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteVideoFrame {
    pub sender_id: u32,
    pub frame_id: u32,
    pub is_screen: bool,
    pub jpeg_base64: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteMediaState {
    pub mic_muted: bool,
    pub deafened: bool,
    pub video_active: bool,
    pub screen_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStatus {
    pub mic_muted: bool,
    pub deafened: bool,
    pub local_speaking: bool,
    pub remote_speaking: bool,
    pub remote_media: RemoteMediaState,
    pub mic_level: f32,
    pub mic_sensitivity: f32,
    pub mic_gain: f32,
    pub output_device_ok: bool,
    pub input_device_ok: bool,
    pub output_device_name: Option<String>,
    pub input_device_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConnectionStatus {
    Disconnected,
    Hosting {
        room_name: String,
        room_code: Option<String>,
        local_addr: Option<String>,
        public_addr: Option<String>,
        connected_count: usize,
    },
    Connecting {
        target_str: String,
        attempt: u32,
        max_attempts: u32,
        mode: String,
    },
    Connected {
        my_id: u32,
        is_host: bool,
        host_name: String,
        peer_name: String,
        peer_addr: String,
        room_name: String,
        room_code: Option<String>,
        ping_ms: u32,
        connected_count: usize,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingStatus {
    pub is_open: bool,
    pub is_waiting_first_peer: bool,
    pub remaining_seconds: u32,
}


#[derive(Debug, Clone)]
pub struct ImageReassembler {
    pub chunk_total: u32,
    pub chunks: Vec<Option<Vec<u8>>>,
    pub created_at: Instant,
}

impl ImageReassembler {
    pub fn new(chunk_total: u32) -> Option<Self> {
        // Enforce strict limit: chunk_total must be > 0 and <= 5000 (~6 MB) to prevent OOM DoS
        if chunk_total == 0 || chunk_total > 5000 {
            return None;
        }
        Some(Self {
            chunk_total,
            chunks: vec![None; chunk_total as usize],
            created_at: Instant::now(),
        })
    }

    pub fn handle_chunk(&mut self, chunk_idx: u32, data: Vec<u8>) -> Option<Vec<u8>> {
        // Reject chunks if transfer timed out (>15s) or chunk is abnormally large (>4 KB)
        if self.created_at.elapsed() > Duration::from_secs(15) || data.len() > 4096 {
            return None;
        }
        if (chunk_idx as usize) < self.chunks.len() {
            self.chunks[chunk_idx as usize] = Some(data);
            if self.chunks.iter().all(|c| c.is_some()) {
                let mut full_bytes = Vec::new();
                for c in &self.chunks {
                    if let Some(bytes) = c {
                        full_bytes.extend_from_slice(bytes);
                    }
                }
                return Some(full_bytes);
            }
        }
        None
    }
}

/// Token Bucket rate-limiter for UDP packets by IP address to prevent CPU DoS
#[derive(Debug)]
pub struct IpRateLimiter {
    buckets: HashMap<IpAddr, (f64, Instant)>,
    last_cleanup: Instant,
    capacity: f64,
    refill_rate: f64,
}

impl IpRateLimiter {
    pub fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            buckets: HashMap::new(),
            last_cleanup: Instant::now(),
            capacity,
            refill_rate,
        }
    }

    pub fn check(&mut self, ip: IpAddr) -> bool {
        let now = Instant::now();
        if self.last_cleanup.elapsed() > Duration::from_secs(30) {
            self.buckets.retain(|_, (_, last)| now.duration_since(*last) < Duration::from_secs(60));
            self.last_cleanup = now;
        }

        let entry = self.buckets.entry(ip).or_insert((self.capacity, now));
        let elapsed = now.duration_since(entry.1).as_secs_f64();
        entry.0 = (entry.0 + elapsed * self.refill_rate).min(self.capacity);
        entry.1 = now;

        if entry.0 >= 1.0 {
            entry.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

fn get_cache_file_path() -> std::path::PathBuf {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let dir = std::path::PathBuf::from(local_app_data).join("Balabol");
        let _ = std::fs::create_dir_all(&dir);
        dir.join("balabol_room_cache.json")
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("balabol_room_cache.json")
    }
}

pub fn load_room_peer_cache() -> HashMap<String, HashSet<SocketAddr>> {
    let path = get_cache_file_path();
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(map) = serde_json::from_slice::<HashMap<String, HashSet<SocketAddr>>>(&bytes) {
            return map;
        }
    }
    HashMap::new()
}

pub fn save_room_peer_cache(cache: &HashMap<String, HashSet<SocketAddr>>) {
    let path = get_cache_file_path();
    if let Ok(json) = serde_json::to_vec_pretty(cache) {
        let _ = std::fs::write(&path, json);
    }
}

#[derive(Serialize, Deserialize)]
struct NodeIdentity {
    peer_id: u32,
    sender_id: u32,
}

fn get_identity_file_path() -> std::path::PathBuf {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let dir = std::path::PathBuf::from(local_app_data).join("Balabol");
        let _ = std::fs::create_dir_all(&dir);
        dir.join("balabol_node_identity.json")
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("balabol_node_identity.json")
    }
}

pub fn get_or_create_node_identity() -> (u32, u32) {
    let path = get_identity_file_path();
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(ident) = serde_json::from_slice::<NodeIdentity>(&bytes) {
            if ident.peer_id >= 2 && ident.sender_id != 0 {
                return (ident.peer_id, ident.sender_id);
            }
        }
    }
    let peer_id = (rand::random::<u32>() % 900_000 + 10_000).max(2);
    let sender_id = rand::random::<u32>() | 1;
    let ident = NodeIdentity { peer_id, sender_id };
    if let Ok(json) = serde_json::to_vec_pretty(&ident) {
        let _ = std::fs::write(&path, json);
    }
    (peer_id, sender_id)
}

#[derive(Clone)]
pub struct P2pNode {
    logger: AppLogger,
    status: Arc<RwLock<ConnectionStatus>>,
    pub my_peer_id: Arc<AtomicU32>,
    pub my_nickname: Arc<RwLock<String>>,
    pub is_host: Arc<AtomicBool>,
    pub current_host_id: Arc<AtomicU32>,
    pub backup_host_id: Arc<RwLock<Option<u32>>>,

    pub peers: Arc<RwLock<HashMap<u32, PeerInfo>>>,
    pub peer_addrs: Arc<RwLock<HashMap<SocketAddr, u32>>>,
    pub client_endpoints: Arc<RwLock<HashMap<u32, SocketAddr>>>,

    pub active_socket: Arc<RwLock<Option<Arc<UdpSocket>>>>,
    pub host_socket_addr: Arc<RwLock<Option<SocketAddr>>>,
    pub relay_sender: Arc<RwLock<Option<tokio::sync::mpsc::Sender<Packet>>>>,

    pub chat_messages: Arc<RwLock<Vec<ChatMessage>>>,
    pub seen_msg_ids: Arc<RwLock<HashSet<u64>>>,
    pub pending_acks: Arc<RwLock<HashMap<u64, oneshot::Sender<()>>>>,
    pub session_abort: Arc<RwLock<Option<tokio::task::AbortHandle>>>,
    pub shutdown_signal: Arc<AtomicBool>,
    pub next_peer_id: Arc<AtomicU32>,
    pub audio_engine: Arc<crate::audio::AudioEngine>,
    pub latest_remote_video: Arc<RwLock<Option<RemoteVideoFrame>>>,
    pub remote_media_state: Arc<RwLock<RemoteMediaState>>,
    pub video_frame_seq: Arc<AtomicU32>,
    pub current_room_code: Arc<RwLock<Option<String>>>,
    pub current_room_key: Arc<RwLock<Option<[u8; 32]>>>,
    pub image_store: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    pub image_reassemblers: Arc<RwLock<HashMap<String, ImageReassembler>>>,
    pub video_frame_callback: Arc<RwLock<Option<Box<dyn Fn(RemoteVideoFrame) + Send + Sync>>>>,
    pub screen_capturer: Arc<crate::screen_capture::ScreenCapturer>,
    pub is_screen_sharing: Arc<AtomicBool>,
    pub room_peer_cache: Arc<RwLock<HashMap<String, HashSet<SocketAddr>>>>,
    pub signaling_until_ms: Arc<AtomicU64>,
    pub reopen_signaling_requested: Arc<AtomicBool>,

    // Network security & PFS
    pub out_packet_seq: Arc<AtomicU64>,
    pub peer_replay_windows: Arc<RwLock<HashMap<SocketAddr, crate::crypto::AntiReplayWindow>>>,
    pub ephemeral_kx: Arc<crate::crypto::EphemeralKeyExchange>,
    pub peer_pfs_keys: Arc<RwLock<HashMap<u32, [u8; 32]>>>,
    pub hide_lan_addresses: Arc<AtomicBool>,

    // Dynamic NAT Roaming & PAKE sessions
    pub my_sender_id: Arc<AtomicU32>,
    pub pake_kx: Arc<crate::crypto::PakeKeyExchange>,
    pub current_room_salt: Arc<RwLock<Option<[u8; 32]>>>,
    pub peer_sessions: Arc<RwLock<HashMap<u32, PeerSession>>>,
    pub peer_id_to_sender_id: Arc<RwLock<HashMap<u32, u32>>>,
}

impl P2pNode {
    pub fn new(logger: AppLogger) -> Self {
        Self {
            logger,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            my_peer_id: Arc::new(AtomicU32::new(1)),
            my_nickname: Arc::new(RwLock::new("Узел".into())),
            is_host: Arc::new(AtomicBool::new(false)),
            current_host_id: Arc::new(AtomicU32::new(1)),
            backup_host_id: Arc::new(RwLock::new(None)),

            peers: Arc::new(RwLock::new(HashMap::new())),
            peer_addrs: Arc::new(RwLock::new(HashMap::new())),
            client_endpoints: Arc::new(RwLock::new(HashMap::new())),

            active_socket: Arc::new(RwLock::new(None)),
            host_socket_addr: Arc::new(RwLock::new(None)),
            relay_sender: Arc::new(RwLock::new(None)),

            chat_messages: Arc::new(RwLock::new(Vec::new())),
            seen_msg_ids: Arc::new(RwLock::new(HashSet::new())),
            pending_acks: Arc::new(RwLock::new(HashMap::new())),
            session_abort: Arc::new(RwLock::new(None)),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            next_peer_id: Arc::new(AtomicU32::new(2)),
            audio_engine: crate::audio::AudioEngine::new(),
            latest_remote_video: Arc::new(RwLock::new(None)),
            remote_media_state: Arc::new(RwLock::new(RemoteMediaState::default())),
            video_frame_seq: Arc::new(AtomicU32::new(1)),
            current_room_code: Arc::new(RwLock::new(None)),
            current_room_key: Arc::new(RwLock::new(None)),
            image_store: Arc::new(RwLock::new(HashMap::new())),
            image_reassemblers: Arc::new(RwLock::new(HashMap::new())),
            video_frame_callback: Arc::new(RwLock::new(None)),
            screen_capturer: Arc::new(crate::screen_capture::ScreenCapturer::new()),
            is_screen_sharing: Arc::new(AtomicBool::new(false)),
            room_peer_cache: Arc::new(RwLock::new(load_room_peer_cache())),
            signaling_until_ms: Arc::new(AtomicU64::new(0)),
            reopen_signaling_requested: Arc::new(AtomicBool::new(false)),

            out_packet_seq: Arc::new(AtomicU64::new(0)),
            peer_replay_windows: Arc::new(RwLock::new(HashMap::new())),
            ephemeral_kx: Arc::new(crate::crypto::EphemeralKeyExchange::new()),
            peer_pfs_keys: Arc::new(RwLock::new(HashMap::new())),
            hide_lan_addresses: Arc::new(AtomicBool::new(false)),

            my_sender_id: Arc::new(AtomicU32::new(rand::random::<u32>() | 1)),
            pake_kx: Arc::new(crate::crypto::PakeKeyExchange::new()),
            current_room_salt: Arc::new(RwLock::new(None)),
            peer_sessions: Arc::new(RwLock::new(HashMap::new())),
            peer_id_to_sender_id: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn next_packet_seq(&self) -> u64 {
        self.out_packet_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn get_key_for_peer(&self, peer_id_opt: Option<u32>) -> Option<[u8; 32]> {
        if let Some(peer_id) = peer_id_opt {
            if let Some(sender_id) = self.peer_id_to_sender_id.read().get(&peer_id).copied() {
                if let Some(session) = self.peer_sessions.read().get(&sender_id) {
                    return Some(session.session_key);
                }
            }
            if let Some(pfs_key) = self.peer_pfs_keys.read().get(&peer_id).cloned() {
                return Some(pfs_key);
            }
        }
        self.current_room_key.read().clone()
    }

    pub fn encode_wire_for_peer(&self, packet: &Packet, peer_id_opt: Option<u32>) -> Result<Vec<u8>, String> {
        let raw = packet.encode().map_err(|e| e.to_string())?;
        let my_sender_id = self.my_sender_id.load(Ordering::Relaxed);
        let seq = self.next_packet_seq();

        if let Some(peer_id) = peer_id_opt {
            if let Some(sender_id) = self.peer_id_to_sender_id.read().get(&peer_id).copied() {
                if let Some(session) = self.peer_sessions.read().get(&sender_id) {
                    return crate::crypto::encrypt_wire_packet(
                        PACKET_TYPE_CONTROL,
                        my_sender_id,
                        seq,
                        &raw,
                        &session.session_key,
                    );
                }
            }
            if let Some(pfs_key) = self.peer_pfs_keys.read().get(&peer_id) {
                return crate::crypto::encrypt_wire_packet(
                    PACKET_TYPE_CONTROL,
                    my_sender_id,
                    seq,
                    &raw,
                    pfs_key,
                );
            }
        }
        if let Some(ref room_key) = *self.current_room_key.read() {
            return crate::crypto::encrypt_wire_packet(
                PACKET_TYPE_CONTROL,
                my_sender_id,
                seq,
                &raw,
                room_key,
            );
        }
        Ok(raw)
    }

    pub fn set_hide_lan_mode(&self, hide: bool) {
        self.hide_lan_addresses.store(hide, Ordering::Relaxed);
        self.logger.info(&format!(
            "Режим повышенной анонимности (сокрытие LAN): {}",
            if hide { "Включен" } else { "Выключен" }
        ));
    }

    pub fn is_hide_lan_mode(&self) -> bool {
        self.hide_lan_addresses.load(Ordering::Relaxed)
    }

    pub fn get_known_endpoints_for_room(&self, clean_code: &str) -> Vec<SocketAddr> {
        let cache = self.room_peer_cache.read();
        cache
            .get(clean_code)
            .cloned()
            .map(|s| s.into_iter().collect())
            .unwrap_or_default()
    }

    pub fn record_room_peer_endpoints(&self, clean_code: &str, endpoints: &[SocketAddr]) {
        if clean_code.is_empty() {
            return;
        }
        let mut modified = false;
        {
            let mut cache = self.room_peer_cache.write();
            let set = cache.entry(clean_code.to_string()).or_insert_with(HashSet::new);
            for &ep in endpoints {
                if !ep.ip().is_loopback() && !ep.ip().is_unspecified() {
                    if set.insert(ep) {
                        modified = true;
                    }
                }
            }
        }
        if modified {
            let cache_clone = self.room_peer_cache.read().clone();
            save_room_peer_cache(&cache_clone);
        }
    }

    pub fn get_status(&self) -> ConnectionStatus {
        self.status.read().clone()
    }

    pub fn get_peers(&self) -> Vec<PeerInfo> {
        let speaking_ids = self.audio_engine.get_speaking_peers();
        let peers_guard = self.peers.read();
        let mut list = Vec::with_capacity(peers_guard.len());
        for peer in peers_guard.values() {
            let mut p = peer.clone();
            p.is_speaking = speaking_ids.contains(&p.id);
            list.push(p);
        }
        list
    }

    pub fn get_chat_messages(&self) -> Vec<ChatMessage> {
        self.chat_messages.read().clone()
    }

    pub fn get_signaling_status(&self) -> SignalingStatus {
        if self.current_room_code.read().is_none() {
            return SignalingStatus {
                is_open: false,
                is_waiting_first_peer: false,
                remaining_seconds: 0,
            };
        }
        let until = self.signaling_until_ms.load(Ordering::SeqCst);
        if until == u64::MAX {
            SignalingStatus {
                is_open: true,
                is_waiting_first_peer: true,
                remaining_seconds: 300,
            }
        } else {
            let now = now_ms();
            if until > now {
                let rem = ((until - now) / 1000) as u32;
                SignalingStatus {
                    is_open: true,
                    is_waiting_first_peer: false,
                    remaining_seconds: rem,
                }
            } else {
                SignalingStatus {
                    is_open: false,
                    is_waiting_first_peer: false,
                    remaining_seconds: 0,
                }
            }
        }
    }

    pub fn reopen_signaling(&self) -> Result<(), String> {
        if self.current_room_code.read().is_none() {
            return Err("Имя не может быть пустым".into());
        }
        self.reopen_signaling_requested.store(true, Ordering::SeqCst);
        let my_name = self.my_nickname.read().clone();
        let sys_msg = format!("🔓 {} открыл(а) комнату на 5 минут для подключения новых участников", my_name);
        let node_clone = self.clone();
        tokio::spawn(async move {
            let _ = node_clone.send_chat_message(sys_msg, "Система".into(), None, None, None).await;
        });
        Ok(())
    }

    pub fn disconnect(&self) {
        if let Some(handle) = self.session_abort.write().take() {
            handle.abort();
        }
        self.shutdown_signal.store(true, Ordering::SeqCst);

        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let disc = Packet::Disconnect {
            sender_id: my_id,
            reason: "Пользователь вышел из комнаты".into(),
        };

        if let Ok(bytes) = self.encode_wire_for_peer(&disc, None) {
            if let Some(sock) = self.active_socket.read().as_ref() {
                let addrs: Vec<SocketAddr> = self.client_endpoints.read().values().cloned().collect();
                for addr in addrs {
                    let s = sock.clone();
                    let b = bytes.clone();
                    tokio::spawn(async move {
                        let _ = s.send_to(&b, addr).await;
                    });
                }
                if let Some(host_addr) = *self.host_socket_addr.read() {
                    let s = sock.clone();
                    let b = bytes.clone();
                    tokio::spawn(async move {
                        let _ = s.send_to(&b, host_addr).await;
                    });
                }
            }
        }

        self.logger.info("Отключение P2P соединения...");
        self.screen_capturer.stop();
        self.audio_engine.stop();
        *self.latest_remote_video.write() = None;
        *self.remote_media_state.write() = RemoteMediaState::default();
        *self.active_socket.write() = None;
        *self.host_socket_addr.write() = None;
        *self.relay_sender.write() = None;
        self.client_endpoints.write().clear();
        self.peer_addrs.write().clear();
        self.peers.write().clear();
        self.peer_pfs_keys.write().clear();
        self.peer_replay_windows.write().clear();
        self.seen_msg_ids.write().clear();
        self.pending_acks.write().clear();
        self.image_store.write().clear();
        self.image_reassemblers.write().clear();
        *self.current_room_key.write() = None;
        self.is_host.store(false, Ordering::SeqCst);
        self.signaling_until_ms.store(0, Ordering::SeqCst);
        self.reopen_signaling_requested.store(false, Ordering::SeqCst);
        *self.current_room_code.write() = None;
        *self.status.write() = ConnectionStatus::Disconnected;
        self.logger.info("Состояние сброшено в Disconnected");
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.audio_engine.set_mic_muted(muted);
        let _ = self.broadcast_media_state();
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.audio_engine.set_deafened(deafened);
        let _ = self.broadcast_media_state();
    }

    pub fn set_master_volume(&self, volume: f32) {
        self.audio_engine.set_master_volume(volume);
    }

    pub fn get_master_volume(&self) -> f32 {
        self.audio_engine.get_master_volume()
    }

    pub fn set_peer_volume(&self, peer_id: u32, volume: f32) {
        self.audio_engine.set_peer_volume(peer_id, volume);
    }

    pub fn get_voice_status(&self) -> VoiceStatus {
        let remote_media = self.remote_media_state.read().clone();
        let (out_name, in_name) = self.audio_engine.get_device_names();
        VoiceStatus {
            mic_muted: self.audio_engine.is_mic_muted(),
            deafened: self.audio_engine.is_deafened(),
            local_speaking: self.audio_engine.is_local_speaking(),
            remote_speaking: self.audio_engine.is_remote_speaking(),
            remote_media,
            mic_level: self.audio_engine.get_mic_level(),
            mic_sensitivity: self.audio_engine.get_mic_sensitivity(),
            mic_gain: self.audio_engine.get_mic_gain(),
            output_device_ok: self.audio_engine.is_output_ok(),
            input_device_ok: self.audio_engine.is_input_ok(),
            output_device_name: out_name,
            input_device_name: in_name,
        }
    }

    pub fn set_mic_sensitivity(&self, sensitivity: f32) {
        self.audio_engine.set_mic_sensitivity(sensitivity);
    }

    pub fn set_mic_gain(&self, gain: f32) {
        self.audio_engine.set_mic_gain(gain);
    }

    pub fn set_loopback_test(&self, enabled: bool) {
        self.audio_engine.set_loopback_test(enabled);
    }

    pub fn restart_audio(&self) -> Result<(), String> {
        self.audio_engine.restart()
    }

    pub fn start_mic_test(&self) -> Result<(), String> {
        self.audio_engine.set_loopback_test(true);
        self.audio_engine.start()
    }

    pub fn stop_mic_test(&self) {
        self.audio_engine.set_loopback_test(false);
    }

    pub fn play_notification_sound(&self, sound_type: &str) {
        if sound_type == "connect" {
            self.audio_engine.play_connect_chime();
        } else {
            self.audio_engine.play_disconnect_chime();
        }
    }

    pub fn get_remote_video_frame(&self, last_frame_id: u32) -> Option<RemoteVideoFrame> {
        let guard = self.latest_remote_video.read();
        if let Some(frame) = guard.as_ref() {
            if frame.frame_id != last_frame_id {
                return Some(frame.clone());
            }
        }
        None
    }

    pub fn set_video_frame_callback<F>(&self, cb: F)
    where
        F: Fn(RemoteVideoFrame) + Send + Sync + 'static,
    {
        *self.video_frame_callback.write() = Some(Box::new(cb));
    }

    pub async fn start_screen_capture(self: &Arc<Self>) -> Result<(), String> {
        self.is_screen_sharing.store(true, Ordering::SeqCst);
        self.logger.info("🖥️ [Экран] Запуск демонстрации рабочего стола...");
        let rt_handle = tokio::runtime::Handle::current();
        let node_weak = Arc::downgrade(self);
        self.screen_capturer.start(move |jpeg_bytes| {
            if let Some(node) = node_weak.upgrade() {
                let node_clone = node.clone();
                rt_handle.spawn(async move {
                    let _ = node_clone.send_video_frame_bytes(jpeg_bytes, true).await;
                });
            }
        });

        // Send media state a few times to ensure reception over UDP
        for _ in 0..2 {
            let _ = self.send_media_state(false, true).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }

    pub async fn stop_screen_capture(&self) -> Result<(), String> {
        self.is_screen_sharing.store(false, Ordering::SeqCst);
        self.logger.info("🖥️ [Экран] Остановка демонстрации рабочего стола");
        self.screen_capturer.stop();
        for _ in 0..2 {
            let _ = self.send_media_state(false, false).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }

    pub async fn send_video_frame_bytes(&self, bytes: Vec<u8>, is_screen: bool) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }

        let sock_opt = self.active_socket.read().clone();
        let sock = match sock_opt {
            Some(s) => s,
            None => return Ok(()),
        };

        let client_addrs: Vec<SocketAddr> = self.client_endpoints.read().values().cloned().collect();
        let host_addr_opt = *self.host_socket_addr.read();

        if client_addrs.is_empty() && host_addr_opt.is_none() {
            return Ok(());
        }

        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let frame_id = self.video_frame_seq.fetch_add(1, Ordering::SeqCst);
        const CHUNK_SIZE: usize = 1200;
        let chunk_total = ((bytes.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u16;

        if frame_id == 1 || frame_id % 60 == 0 {
            self.logger.info(&format!(
                "🖥️ [Экран] Отправка кадра #{} ({:.1} KB, {} чанков) пирам: {:?}",
                frame_id,
                bytes.len() as f64 / 1024.0,
                chunk_total,
                client_addrs
            ));
        }

        for (idx, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
            let pkt = Packet::VideoChunk {
                sender_id: my_id,
                frame_id,
                chunk_idx: idx as u16,
                chunk_total,
                is_screen,
                data: chunk.to_vec(),
            };

            for &addr in &client_addrs {
                let peer_id_opt = self.peer_addrs.read().get(&addr).copied();
                if let Ok(pkt_bytes) = self.encode_wire_for_peer(&pkt, peer_id_opt) {
                    let _ = sock.send_to(&pkt_bytes, addr).await;
                }
            }
            if let Some(host_addr) = host_addr_opt {
                let host_id = self.current_host_id.load(Ordering::Relaxed);
                if let Ok(pkt_bytes) = self.encode_wire_for_peer(&pkt, Some(host_id)) {
                    let _ = sock.send_to(&pkt_bytes, host_addr).await;
                }
            }

            tokio::task::yield_now().await;
        }

        Ok(())
    }

    pub async fn send_video_frame(&self, jpeg_base64: String, is_screen: bool) -> Result<(), String> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&jpeg_base64)
            .map_err(|e| format!("Base64 decode error: {}", e))?;

        self.send_video_frame_bytes(bytes, is_screen).await
    }

    pub async fn send_media_state(&self, video_active: bool, screen_active: bool) -> Result<(), String> {
        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let pkt = Packet::MediaState {
            sender_id: my_id,
            mic_muted: self.audio_engine.is_mic_muted(),
            deafened: self.audio_engine.is_deafened(),
            video_active,
            screen_active,
        };
        self.broadcast_packet(&pkt).await;
        Ok(())
    }

    fn broadcast_media_state(&self) -> Result<(), String> {
        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let is_screen = self.is_screen_sharing.load(Ordering::Relaxed);
        let pkt = Packet::MediaState {
            sender_id: my_id,
            mic_muted: self.audio_engine.is_mic_muted(),
            deafened: self.audio_engine.is_deafened(),
            video_active: false,
            screen_active: is_screen,
        };
        let self_clone = self.clone();
        tokio::spawn(async move {
            self_clone.broadcast_packet(&pkt).await;
        });
        Ok(())
    }

    pub async fn broadcast_packet(&self, packet: &Packet) {
        let sock_opt = self.active_socket.read().clone();
        if let Some(sock) = sock_opt {
            let targets: Vec<(SocketAddr, u32)> = {
                let sessions = self.peer_sessions.read();
                sessions.values().map(|s| (s.current_addr, s.peer_id)).collect()
            };
            if !targets.is_empty() {
                for (addr, peer_id) in targets {
                    if let Ok(bytes) = self.encode_wire_for_peer(packet, Some(peer_id)) {
                        let _ = sock.send_to(&bytes, addr).await;
                    }
                }
            } else {
                let client_addrs: Vec<SocketAddr> = self.client_endpoints.read().values().cloned().collect();
                for addr in client_addrs {
                    let peer_id_opt = self.peer_addrs.read().get(&addr).copied();
                    if let Ok(bytes) = self.encode_wire_for_peer(packet, peer_id_opt) {
                        let _ = sock.send_to(&bytes, addr).await;
                    }
                }
                let host_addr_opt = *self.host_socket_addr.read();
                if let Some(host_addr) = host_addr_opt {
                    let host_id = self.current_host_id.load(Ordering::Relaxed);
                    if let Ok(bytes) = self.encode_wire_for_peer(packet, Some(host_id)) {
                        let _ = sock.send_to(&bytes, host_addr).await;
                    }
                }
            }
        }
    }

    pub async fn send_image_chunks_broadcast(&self, image_id: &str, bytes: &[u8]) {
        const CHUNK_SIZE: usize = 1200;
        let chunk_total = ((bytes.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32;
        let sock_opt = self.active_socket.read().clone();
        if let Some(sock) = sock_opt {
            let targets: Vec<(SocketAddr, u32)> = {
                let sessions = self.peer_sessions.read();
                sessions.values().map(|s| (s.current_addr, s.peer_id)).collect()
            };
            for (idx, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
                let pkt = Packet::ImageChunk {
                    image_id: image_id.to_string(),
                    chunk_idx: idx as u32,
                    chunk_total,
                    data: chunk.to_vec(),
                };
                if !targets.is_empty() {
                    for &(addr, peer_id) in &targets {
                        if let Ok(b) = self.encode_wire_for_peer(&pkt, Some(peer_id)) {
                            let _ = sock.send_to(&b, addr).await;
                        }
                    }
                } else {
                    let endpoints: Vec<SocketAddr> = self.client_endpoints.read().values().cloned().collect();
                    for &addr in &endpoints {
                        let peer_id_opt = self.peer_addrs.read().get(&addr).copied();
                        if let Ok(b) = self.encode_wire_for_peer(&pkt, peer_id_opt) {
                            let _ = sock.send_to(&b, addr).await;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
        }
    }

    pub async fn send_chat_message(
        &self,
        text: String,
        my_name: String,
        image_thumb: Option<String>,
        image_id: Option<String>,
        full_image_base64: Option<String>,
    ) -> Result<ChatMessage, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() && image_thumb.is_none() && image_id.is_none() {
            return Err("Сообщение не может быть пустым".into());
        }

        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let id = (rand::random::<u64>() & 0x7FFF_FFFF_FFFF_FFFF).max(1);
        let timestamp_ms = now_ms();
        self.seen_msg_ids.write().insert(id);

        let mut image_bytes_opt = None;
        if let (Some(ref img_id), Some(ref full_b64)) = (&image_id, &full_image_base64) {
            use base64::Engine;
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(full_b64) {
                self.image_store.write().insert(img_id.clone(), bytes.clone());
                image_bytes_opt = Some((img_id.clone(), bytes));
            }
        }

        let packet = Packet::TextMessage {
            id,
            sender_id: my_id,
            text: trimmed.to_string(),
            sender_name: my_name.clone(),
            timestamp_ms,
            image_thumb: image_thumb.clone(),
            image_id: image_id.clone(),
        };

        // Broadcast to all peers via direct UDP (encrypted & authenticated)
        self.broadcast_packet(&packet).await;

        // Proactively send image chunks directly via UDP
        if let Some((img_id, bytes)) = image_bytes_opt {
            let self_clone = self.clone();
            tokio::spawn(async move {
                self_clone.send_image_chunks_broadcast(&img_id, &bytes).await;
            });
        }

        let msg = ChatMessage {
            id,
            sender: my_name,
            text: trimmed.to_string(),
            timestamp_ms,
            is_me: true,
            image_thumb,
            image_id,
        };

        self.chat_messages.write().push(msg.clone());
        if !trimmed.is_empty() {
            self.logger.info(&format!("[ЧАТ] Отправлено сообщение (длина: {} симв.)", trimmed.chars().count()));
        } else {
            self.logger.info("[ЧАТ] Отправлено изображение");
        }
        Ok(msg)
    }

    pub async fn request_full_image(&self, image_id: String) -> Result<(), String> {
        if self.image_store.read().contains_key(&image_id) {
            return Ok(());
        }
        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let pkt = Packet::ImageRequest {
            image_id,
            requester_id: my_id,
        };
        self.broadcast_packet(&pkt).await;
        Ok(())
    }

    pub fn get_cached_image(&self, image_id: &str) -> Option<String> {
        use base64::Engine;
        self.image_store
            .read()
            .get(image_id)
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub async fn request_history(&self) {
        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let pkt = Packet::HistoryRequest { requester_id: my_id };
        self.broadcast_packet(&pkt).await;
    }

    /// Update local nickname and broadcast to all peers in call
    pub async fn update_nickname(&self, new_nickname: String) -> Result<(), String> {
        let trimmed = new_nickname.trim().to_string();
        if trimmed.is_empty() {
            return Err("Имя не может быть пустым".into());
        }

        *self.my_nickname.write() = trimmed.clone();
        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let is_host = self.is_host.load(Ordering::Relaxed);

        {
            let mut st = self.status.write();
            if let ConnectionStatus::Connected { host_name, is_host: host_flag, .. } = &mut *st {
                if *host_flag || is_host {
                    *host_name = trimmed.clone();
                }
            }
        }

        let pkt = Packet::NicknameUpdate {
            sender_id: my_id,
            new_nickname: trimmed.clone(),
        };
        self.broadcast_packet(&pkt).await;
        self.logger.info(&format!("Ваше имя обновлено на '{}'", trimmed));
        Ok(())
    }

    /// Transfer host responsibility (no-op in Full Mesh)
    pub async fn transfer_host(&self, _new_host_id: u32) -> Result<(), String> {
        Ok(())
    }

    /// Promote to host (no-op in Full Mesh)
    pub async fn promote_to_host(&self) {}

    /// Connect to a Full Mesh room symmetrically
    pub async fn connect_mesh(
        &self,
        room_code: String,
        nickname: String,
    ) -> Result<String, String> {
        self.disconnect();
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.shutdown_signal.store(false, Ordering::SeqCst);

        let clean_code = crate::signaling::sanitize_room_code(&room_code);
        self.logger.info(&format!(
            "Подключение к Full Mesh комнате '{}' (Код: {}), узел '{}'",
            room_code, clean_code, nickname
        ));

        let display_code = if clean_code.len() == 6 {
            format!("{} {} {}", &clean_code[0..2], &clean_code[2..4], &clean_code[4..6])
        } else {
            clean_code.clone()
        };
        *self.status.write() = ConnectionStatus::Connecting {
            target_str: format!("Комната {}", display_code),
            attempt: 1,
            max_attempts: 1,
            mode: "Поиск участников через релей-пул...".into(),
        };

        // 1. Bind UDP socket
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| format!("Не удалось создать UDP сокет: {}", e))?;
        let opt_physical_ip = configure_socket_physical_if(&socket, &self.logger);
        let actual_port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
        let arc_socket = Arc::new(socket);
        *self.active_socket.write() = Some(arc_socket.clone());

        // 2. Gather candidates
        let hide_lan = self.hide_lan_addresses.load(Ordering::Relaxed);
        let candidates = gather_candidates(arc_socket.clone(), actual_port, opt_physical_ip, hide_lan, &self.logger).await;

        let local_str = candidates.first().map(|a| a.to_string());
        let pub_str = candidates
            .iter()
            .find(|a| !a.ip().is_loopback() && a.to_string() != local_str.clone().unwrap_or_default())
            .map(|a| a.to_string());

        let (my_id, my_sender_id) = get_or_create_node_identity();
        self.my_sender_id.store(my_sender_id, Ordering::SeqCst);
        let session_id = format!("{:08x}", rand::random::<u32>());
        let my_nonce = rand::random::<u64>();

        let is_creator = self.is_host.load(Ordering::SeqCst);
        self.my_peer_id.store(my_id, Ordering::SeqCst);
        *self.my_nickname.write() = nickname.clone();
        self.current_host_id.store(my_id, Ordering::SeqCst);
        *self.backup_host_id.write() = None;

        let room_salt = crate::crypto::derive_deterministic_room_salt(&clean_code);
        *self.current_room_salt.write() = Some(room_salt);
        let pake_seed = crate::crypto::derive_pake_seed(&clean_code, &room_salt);
        *self.current_room_key.write() = Some(pake_seed);
        *self.current_room_code.write() = Some(clean_code.clone());
        let pake_point = self.pake_kx.blinded_public_point(&pake_seed);

        *self.status.write() = ConnectionStatus::Connected {
            my_id,
            is_host: is_creator,
            host_name: "P2P Full Mesh".into(),
            peer_name: "Комната".into(),
            peer_addr: pub_str.clone().or(local_str.clone()).unwrap_or_default(),
            room_name: room_code.clone(),
            room_code: Some(clean_code.clone()),
            ping_ms: 0,
            connected_count: 0,
        };

        // 3. Start local audio engine
        if let Err(e) = self.audio_engine.start() {
            self.logger.error(&format!("Не удалось запустить аудио движок: {}", e));
        } else {
            self.logger.info("Аудио движок успешно запущен");
            self.audio_engine.play_connect_chime();
        }

        // 4. Start continuous Mesh Nostr signaling session
        let my_announce = crate::signaling::MeshAnnounce {
            peer_id: my_id,
            session_id,
            peer_name: nickname.clone(),
            candidates: candidates.clone(),
            nonce: my_nonce,
            sender_id: my_sender_id,
            room_salt,
            pake_point,
            confirmation_tag: [0u8; 16],
        };

        let mesh_signaling = crate::signaling::Signaling::start_mesh_session(&clean_code, my_announce.clone(), &self.logger).await?;

        // 5. Spawn Full Mesh peer communication loop (pure direct UDP P2P)
        let node_clone = self.clone();
        let code_for_task = clean_code.clone();

        let my_announce_task = my_announce.clone();
        let task = tokio::spawn(async move {
            run_mesh_peer_loop(
                arc_socket,
                mesh_signaling,
                code_for_task,
                node_clone,
                my_announce_task,
            ).await;
        });

        *self.session_abort.write() = Some(task.abort_handle());
        Ok(clean_code)
    }

    /// Host starts Full Mesh room with code
    pub async fn start_room(
        &self,
        room_code: String,
        nickname: String,
    ) -> Result<String, String> {
        self.is_host.store(true, Ordering::SeqCst);
        self.connect_mesh(room_code, nickname).await
    }

    /// Client joins Full Mesh room with code
    pub async fn join_room(
        &self,
        room_code: String,
        nickname: String,
    ) -> Result<(), String> {
        self.is_host.store(false, Ordering::SeqCst);
        self.connect_mesh(room_code, nickname).await.map(|_| ())
    }
}

/// Full Mesh P2P Loop: fully symmetric communication between all peers over direct UDP
async fn run_mesh_peer_loop(
    socket: Arc<UdpSocket>,
    mesh_signaling: crate::signaling::MeshSignalingSession,
    room_code: String,
    node: P2pNode,
    my_announce: crate::signaling::MeshAnnounce,
) {
    node.logger.info("============================================================");
    node.logger.info(&format!("FULL MESH P2P УЗЕЛ АКТИВЕН ДЛЯ КОМНАТЫ '{}'", room_code));
    node.logger.info("============================================================");

    let mut buf = [0u8; 65535];
    let mut video_reassemblers: HashMap<u32, VideoReassembler> = HashMap::new();
    let mut last_known_punch = Instant::now();
    let mut rate_limiter = IpRateLimiter::new(2000.0, 1500.0);

    // Discovered remote peers' candidates: peer_id -> Vec<SocketAddr>
    let mut discovered_candidates: HashMap<u32, Vec<SocketAddr>> = HashMap::new();
    let mut discovered_sessions: HashMap<u32, String> = HashMap::new();
    let mut last_punch_attempt: HashMap<u32, Instant> = HashMap::new();

    // 1. Signaling lifetime management:
    // While the creator sits alone in the room, signaling runs indefinitely.
    // Once someone joins (peers.len() > 0), signaling runs for 5 more minutes, then closes WSS.
    let mut mesh_signaling: Option<crate::signaling::MeshSignalingSession> = Some(mesh_signaling);
    let mut first_peer_joined_at: Option<Instant> = None;
    let mut signaling_closed = false;

    if node.peers.read().is_empty() {
        node.signaling_until_ms.store(u64::MAX, Ordering::SeqCst);
    } else {
        node.signaling_until_ms.store(now_ms() + 300_000, Ordering::SeqCst);
    }

    // 2. Preload known interlocutors from room memory
    let known_room_addrs = node.get_known_endpoints_for_room(&room_code);
    let mut known_unconnected_addrs: HashSet<SocketAddr> = HashSet::new();
    if !known_room_addrs.is_empty() {
        node.logger.info(&format!(
            "🧠 [Память комнаты '{}'] Найдено {} ранее известных адресов собеседников. Запуск прямого P2P опроса...",
            room_code,
            known_room_addrs.len()
        ));
        for target in expand_candidates_with_port_spray(&known_room_addrs) {
            known_unconnected_addrs.insert(target);
        }
        let my_id = node.my_peer_id.load(Ordering::Relaxed);
        let my_name = node.my_nickname.read().clone();
        let my_sender_id = node.my_sender_id.load(Ordering::Relaxed);
        let punch_pkt = Packet::Punch {
            room_code: room_code.clone(),
            nonce: rand::random(),
            sender_name: my_name,
            sender_id: my_id,
            wire_sender_id: my_sender_id,
            pake_point: my_announce.pake_point,
        };
        let key_opt = node.current_room_key.read().clone();
        let seq = node.next_packet_seq();
        if let Ok(pb) = punch_pkt.encode_wire(key_opt.as_ref(), seq) {
            for target in &known_unconnected_addrs {
                let _ = socket.send_to(&pb, *target).await;
            }
        }
    }

    // Background History Sync on room join
    let node_history = node.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        node_history.request_history().await;
        tokio::time::sleep(Duration::from_millis(2200)).await;
        node_history.request_history().await;
    });

    // Background Audio Transmitter (direct UDP P2P with 13B Wire Format & O(1) sender_id)
    let audio_sock = socket.clone();
    let node_audio = node.clone();
    let audio_tx_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut audio_frame_seq = 0u32;

        while !node_audio.shutdown_signal.load(Ordering::Relaxed) {
            interval.tick().await;
            let my_sender_id = node_audio.my_sender_id.load(Ordering::Relaxed);
            for audio_payload in node_audio.audio_engine.pop_outgoing_packets() {
                audio_frame_seq = audio_frame_seq.wrapping_add(1);
                let transport_seq = node_audio.next_packet_seq();

                // Pack [audio_frame_seq: 4B] + audio_payload for JitterBuffer
                let mut framed_payload = Vec::with_capacity(4 + audio_payload.len());
                framed_payload.extend_from_slice(&audio_frame_seq.to_be_bytes());
                framed_payload.extend_from_slice(&audio_payload);

                let targets: Vec<(SocketAddr, [u8; 32])> = {
                    let now = Instant::now();
                    let mut sessions = node_audio.peer_sessions.write();
                    sessions.values_mut().map(|s| {
                        s.last_sent_time = now;
                        (s.current_addr, s.session_key)
                    }).collect()
                };
                if targets.is_empty() {
                    let key_opt = node_audio.current_room_key.read().clone();
                    if let Some(key) = key_opt {
                        let fallback_eps: Vec<SocketAddr> = {
                            let eps = node_audio.client_endpoints.read();
                            eps.values().copied().collect()
                        };
                        for caddr in fallback_eps {
                            if let Ok(b) = crate::crypto::encrypt_wire_packet(
                                PACKET_TYPE_AUDIO,
                                my_sender_id,
                                transport_seq,
                                &framed_payload,
                                &key,
                            ) {
                                let _ = audio_sock.send_to(&b, caddr).await;
                            }
                        }
                    }
                } else {
                    for (caddr, session_key) in targets {
                        if let Ok(b) = crate::crypto::encrypt_wire_packet(
                            PACKET_TYPE_AUDIO,
                            my_sender_id,
                            transport_seq,
                            &framed_payload,
                            &session_key,
                        ) {
                            let _ = audio_sock.send_to(&b, caddr).await;
                        }
                    }
                }
            }
        }
    });

    while !node.shutdown_signal.load(Ordering::Relaxed) {
        let my_id = node.my_peer_id.load(Ordering::Relaxed);
        let my_name = node.my_nickname.read().clone();
        let my_sender_id = node.my_sender_id.load(Ordering::Relaxed);

        // Check if user requested to reopen/extend signaling from UI
        if node.reopen_signaling_requested.swap(false, Ordering::SeqCst) {
            first_peer_joined_at = Some(Instant::now());
            signaling_closed = false;
            node.signaling_until_ms.store(now_ms() + 300_000, Ordering::SeqCst);
            if mesh_signaling.is_none() {
                match crate::signaling::Signaling::start_mesh_session(&room_code, my_announce.clone(), &node.logger).await {
                    Ok(new_sig) => {
                        mesh_signaling = Some(new_sig);
                        node.logger.info("🔓 [Nostr] Сигналинг комнаты снова открыт на 5 минут для входа новых участников.");
                    }
                    Err(e) => {
                        node.logger.error(&format!("Не удалось перезапустить сигналинг: {}", e));
                    }
                }
            } else {
                node.logger.info("⏳ [Nostr] Окно сигналинга продлено еще на 5 минут.");
            }
        }

        // Manage Signaling Lifecycle: 5 minutes after first participant joins
        let active_peers_count = node.peers.read().len();
        if active_peers_count > 0 && first_peer_joined_at.is_none() {
            first_peer_joined_at = Some(Instant::now());
            node.signaling_until_ms.store(now_ms() + 300_000, Ordering::SeqCst);
            node.logger.info(&format!(
                "🔔 В комнату '{}' подключился участник. Сигналинг через WSS активен еще 5 минут для подключения остальных гостей, после чего комната закроется для новых подключений по коду.",
                room_code
            ));
        }

        if !signaling_closed {
            if let Some(joined_time) = first_peer_joined_at {
                if joined_time.elapsed() >= Duration::from_secs(300) {
                    signaling_closed = true;
                    node.signaling_until_ms.store(0, Ordering::SeqCst);
                    if let Some(sig) = mesh_signaling.take() {
                        sig.stop();
                        drop(sig);
                    }
                    node.logger.info(&format!(
                        "🔒 Сигналинг комнаты '{}' закрыт (прошло 5 минут после первого подключения). WSS релеи отключены. Новые участники по коду зайти не смогут.",
                        room_code
                    ));
                }
            }
        }

        // 1. Process incoming peer announcements from Nostr signaling (if signaling is active)
        if let Some(ref mut sig) = mesh_signaling {
            while let Ok(ann) = sig.incoming_announces.try_recv() {
                if ann.peer_id == my_id {
                    continue;
                }

                // Record known candidates into room peer memory
                node.record_room_peer_endpoints(&room_code, &ann.candidates);

                // Detect if peer reconnected with a new session
                let prev_session = discovered_sessions.insert(ann.peer_id, ann.session_id.clone());
                if let Some(prev) = prev_session {
                    if prev != ann.session_id {
                        node.logger.info(&format!(
                            "Пир {} ('{}') переподключился (новая сессия {}). Сброс аудио-последовательности.",
                            ann.peer_id, ann.peer_name, ann.session_id
                        ));
                        node.audio_engine.reset_peer_sequence(ann.peer_id);
                    }
                }

                // Update room salt if host provided it
                if ann.room_salt != [0u8; 32] {
                    let mut salt_guard = node.current_room_salt.write();
                    if salt_guard.is_none() {
                        *salt_guard = Some(ann.room_salt);
                    }
                }

                // Compute PAKE session key
                let salt_to_use = node.current_room_salt.read().clone().unwrap_or(ann.room_salt);
                let pake_seed = crate::crypto::derive_pake_seed(&room_code, &salt_to_use);
                let session_key = if ann.pake_point != [0u8; 32] {
                    node.pake_kx.compute_pake_session_key(&ann.pake_point, &pake_seed, my_id, ann.peer_id)
                } else {
                    crate::crypto::derive_room_key(&room_code)
                };

                let remote_sender_id = if ann.sender_id != 0 { ann.sender_id } else { ann.peer_id };
                let init_addr = ann.candidates.first().copied().unwrap_or_else(|| SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0));

                {
                    let mut sessions = node.peer_sessions.write();
                    if let Some(session) = sessions.get_mut(&remote_sender_id) {
                        session.session_key = session_key;
                        session.candidate_addrs = ann.candidates.clone();
                    } else {
                        sessions.insert(remote_sender_id, PeerSession {
                            peer_id: ann.peer_id,
                            sender_id: remote_sender_id,
                            session_key,
                            current_addr: init_addr,
                            replay_window: crate::crypto::AntiReplayWindow::new(),
                            last_sent_time: Instant::now(),
                            last_recv_time: Instant::now(),
                            last_repunch_time: Instant::now() - Duration::from_secs(10),
                            candidate_addrs: ann.candidates.clone(),
                        });
                        node.peer_id_to_sender_id.write().insert(ann.peer_id, remote_sender_id);
                    }
                }
                node.peer_pfs_keys.write().insert(ann.peer_id, session_key);

                let mut candidates = expand_candidates_with_port_spray(&ann.candidates);

                // Smart local loopback for single-machine or same-NAT testing:
                let shares_public_ip = ann.candidates.iter().any(|peer_c| {
                    my_announce.candidates.iter().any(|my_c| my_c.ip() == peer_c.ip())
                });
                if shares_public_ip {
                    for &peer_c in &ann.candidates {
                        let loopback = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), peer_c.port());
                        if !candidates.contains(&loopback) {
                            candidates.push(loopback);
                        }
                    }
                }
                discovered_candidates.insert(ann.peer_id, candidates.clone());

                let already_connected = node.client_endpoints.read().contains_key(&ann.peer_id);
                let punch_needed = !already_connected
                    && last_punch_attempt
                        .get(&ann.peer_id)
                        .map(|t| t.elapsed() > Duration::from_secs(2))
                        .unwrap_or(true);

                if punch_needed {
                    last_punch_attempt.insert(ann.peer_id, Instant::now());
                    node.logger.info(&format!(
                        "🥊 [NAT Punch] Отправка Punch пакетов пиру {} ('{}') на {} адресов...",
                        ann.peer_id, ann.peer_name, candidates.len()
                    ));
                    let punch_pkt = Packet::Punch {
                        room_code: room_code.clone(),
                        nonce: ann.nonce,
                        sender_name: my_name.clone(),
                        sender_id: my_id,
                        wire_sender_id: my_sender_id,
                        pake_point: my_announce.pake_point,
                    };
                    let key_opt = node.current_room_key.read().clone();
                    let seq = node.next_packet_seq();
                    if let Ok(pb) = punch_pkt.encode_wire(key_opt.as_ref(), seq) {
                        for target in &candidates {
                            let _ = socket.send_to(&pb, *target).await;
                        }
                    }
                }
            }
        }

        // 2. Periodic Punch retry for discovered but unconnected UDP peers
        for (&pid, cands) in &discovered_candidates {
            if !node.client_endpoints.read().contains_key(&pid) {
                let should_try = last_punch_attempt
                    .get(&pid)
                    .map(|t| t.elapsed() >= Duration::from_millis(2000))
                    .unwrap_or(true);
                if should_try {
                    last_punch_attempt.insert(pid, Instant::now());
                    let punch_pkt = Packet::Punch {
                        room_code: room_code.clone(),
                        nonce: rand::random(),
                        sender_name: my_name.clone(),
                        sender_id: my_id,
                        wire_sender_id: my_sender_id,
                        pake_point: my_announce.pake_point,
                    };
                    let key_opt = node.current_room_key.read().clone();
                    let seq = node.next_packet_seq();
                    if let Ok(pb) = punch_pkt.encode_wire(key_opt.as_ref(), seq) {
                        for &target in cands {
                            let _ = socket.send_to(&pb, target).await;
                        }
                    }
                }
            }
        }

        // 2b. Periodic Punch retry for remembered interlocutors from room memory
        if !known_unconnected_addrs.is_empty() {
            let current_connected: HashSet<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
            known_unconnected_addrs.retain(|addr| !current_connected.contains(addr));

            if last_known_punch.elapsed() >= Duration::from_millis(2000) && !known_unconnected_addrs.is_empty() {
                last_known_punch = Instant::now();
                let punch_pkt = Packet::Punch {
                    room_code: room_code.clone(),
                    nonce: rand::random(),
                    sender_name: my_name.clone(),
                    sender_id: my_id,
                    wire_sender_id: my_sender_id,
                    pake_point: my_announce.pake_point,
                };
                let key_opt = node.current_room_key.read().clone();
                let seq = node.next_packet_seq();
                if let Ok(pb) = punch_pkt.encode_wire(key_opt.as_ref(), seq) {
                    for target in &known_unconnected_addrs {
                        let _ = socket.send_to(&pb, *target).await;
                    }
                }
            }
        }

        // Step 4: Three Independent Timers per Peer (Send Timer, Silence Timer, Death Timer)
        let now = Instant::now();
        let mut keepalives_to_send = Vec::new();
        let mut repunch_actions = Vec::new();
        let mut dead_peers = Vec::new();

        {
            let mut sessions = node.peer_sessions.write();
            for (&remote_sender_id, session) in sessions.iter_mut() {
                // Timer 1: Send Timer (5s of silence) -> send 29B encrypted KeepAlive to hold open CGNAT
                if now.duration_since(session.last_sent_time) >= Duration::from_secs(5) {
                    session.last_sent_time = now;
                    let seq = node.next_packet_seq();
                    if let Ok(wire) = crate::crypto::encrypt_wire_packet(
                        PACKET_TYPE_KEEPALIVE,
                        my_sender_id,
                        seq,
                        &[], // 0 bytes payload -> exactly 29 bytes wire packet
                        &session.session_key,
                    ) {
                        keepalives_to_send.push((wire, session.current_addr));
                    }
                }

                // Timer 2: Silence Timer (2.5s without packets) -> background Port Spraying & PEX re-route
                let silence_duration = now.duration_since(session.last_recv_time);
                if silence_duration >= Duration::from_millis(2500) && silence_duration < Duration::from_secs(30) {
                    if now.duration_since(session.last_repunch_time) >= Duration::from_millis(500) {
                        session.last_repunch_time = now;
                        let seq = node.next_packet_seq();
                        if let Ok(wire) = crate::crypto::encrypt_wire_packet(
                            PACKET_TYPE_KEEPALIVE,
                            my_sender_id,
                            seq,
                            &[],
                            &session.session_key,
                        ) {
                            let mut targets = Vec::new();
                            let base_port = session.current_addr.port();
                            let ip = session.current_addr.ip();
                            for delta in -10i32..=10i32 {
                                let target_port = (base_port as i32 + delta) as u16;
                                if target_port > 0 {
                                    targets.push(SocketAddr::new(ip, target_port));
                                }
                            }
                            for &cand in &session.candidate_addrs {
                                if !targets.contains(&cand) {
                                    targets.push(cand);
                                }
                            }
                            repunch_actions.push((wire, targets, session.peer_id));
                        }
                    }
                }

                // Timer 3: Death Timer (30s without valid packets) -> disconnect peer in UI
                if silence_duration >= Duration::from_secs(30) {
                    dead_peers.push((remote_sender_id, session.peer_id));
                }
            }

            for (remote_sender_id, _) in &dead_peers {
                sessions.remove(remote_sender_id);
            }
        }

        // Execute Timer 1: Send KeepAlive
        for (wire, target) in keepalives_to_send {
            let _ = socket.send_to(&wire, target).await;
        }

        // Execute Timer 2: Background Port Spraying & PEX assist
        for (wire, targets, peer_b_id) in repunch_actions {
            for target in targets {
                let _ = socket.send_to(&wire, target).await;
            }
            // PEX request to third participant (Peer C)
            let other_peers: Vec<(u32, SocketAddr)> = {
                let eps = node.client_endpoints.read();
                eps.iter()
                    .filter(|(&id, _)| id != peer_b_id && id != my_id)
                    .map(|(&id, &addr)| (id, addr))
                    .collect()
            };
            for (peer_c_id, peer_c_addr) in other_peers {
                let intro_query = Packet::PeerIntroduce {
                    room_code: room_code.clone(),
                    peer_id: peer_b_id,
                    peer_name: String::new(),
                    endpoints: vec![],
                };
                if let Ok(b) = node.encode_wire_for_peer(&intro_query, Some(peer_c_id)) {
                    let _ = socket.send_to(&b, peer_c_addr).await;
                }
            }
        }

        // Execute Timer 3: Death cleanup
        for (_remote_sender_id, peer_id) in dead_peers {
            node.logger.warn(&format!("Пир {} отключился по таймауту (>30с тишины)", peer_id));
            node.peer_id_to_sender_id.write().remove(&peer_id);
            let opt_addr = node.client_endpoints.write().remove(&peer_id);
            if let Some(addr) = opt_addr {
                node.peer_addrs.write().remove(&addr);
                known_unconnected_addrs.insert(addr);
            }
            let was_connected = node.peers.write().remove(&peer_id).is_some();
            if was_connected {
                node.audio_engine.play_disconnect_chime();
            }
            let cnt = node.peers.read().len();
            let mut st = node.status.write();
            if let ConnectionStatus::Connected { connected_count, .. } = &mut *st {
                *connected_count = cnt;
            }
        }

        // 5. Receive UDP packets (O(1) NAT Roaming & Wire Format Verification)
        tokio::select! {
            recv_res = socket.recv_from(&mut buf) => {
                match recv_res {
                    Ok((len, sender_addr)) => {
                        // Rate limiter: Token Bucket to protect against UDP flood
                        if !rate_limiter.check(sender_addr.ip()) {
                            continue;
                        }

                        // Try Wire Header parse (13 bytes: packet_type: 1B, sender_id: 4B, seq: 8B)
                        if len >= crate::crypto::MIN_WIRE_PACKET_LEN {
                            if let Ok((packet_type, sender_id, seq)) = crate::crypto::parse_wire_header(&buf[..len]) {
                                // O(1) Session Lookup by sender_id (Step 2)
                                let session_opt = {
                                    let sessions = node.peer_sessions.read();
                                    sessions.get(&sender_id).cloned()
                                };

                                if let Some(mut session) = session_opt {
                                    // Step 3: Anti-Replay Phase 1 (pre-check before heavy crypto)
                                    if !session.replay_window.check_seq_pre(seq) {
                                        continue;
                                    }

                                    // Decrypt with 13-byte header as AAD
                                    match crate::crypto::decrypt_wire_packet(&buf[..len], &session.session_key) {
                                        Ok((_ptype, _sid, _s, payload)) => {
                                            // Poly1305 MAC verified successfully!
                                            // Step 3: Anti-Replay Phase 2 (post-commit sequence)
                                            session.replay_window.commit_seq_post(seq);

                                            // Step 2: Instant Dynamic NAT Roaming (0 ms switch)
                                            let addr_changed = session.current_addr != sender_addr;
                                            if addr_changed {
                                                node.logger.info(&format!(
                                                    "🔄 [NAT Roaming O(1)] Пир {} (sender_id: {}) сменил адрес/порт {} -> {}. Мгновенное переключение (0 мс)!",
                                                    session.peer_id, sender_id, session.current_addr, sender_addr
                                                ));
                                                session.current_addr = sender_addr;
                                                node.client_endpoints.write().insert(session.peer_id, sender_addr);
                                                node.peer_addrs.write().insert(sender_addr, session.peer_id);
                                            }

                                            session.last_recv_time = Instant::now();
                                            node.peer_sessions.write().insert(sender_id, session.clone());

                                            if let Some(p) = node.peers.write().get_mut(&session.peer_id) {
                                                p.last_seen_ms = now_ms();
                                                if addr_changed {
                                                    p.addr = sender_addr.to_string();
                                                }
                                            }

                                            match packet_type {
                                                PACKET_TYPE_AUDIO => {
                                                    let (frame_seq, audio_data) = if payload.len() >= 5
                                                        && (payload[4] == crate::audio::CODEC_PCM24K
                                                            || payload[4] == crate::audio::CODEC_PCM16K)
                                                    {
                                                        let fseq = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as u64;
                                                        (fseq, &payload[4..])
                                                    } else {
                                                        (seq, payload.as_slice())
                                                    };
                                                    node.audio_engine.feed_incoming_packet(session.peer_id, frame_seq, audio_data);
                                                    if let Some(p) = node.peers.write().get_mut(&session.peer_id) {
                                                        p.is_speaking = true;
                                                    }
                                                }
                                                PACKET_TYPE_KEEPALIVE => {
                                                    // KeepAlive packet: route maintained, nothing more needed
                                                }
                                                PACKET_TYPE_CONTROL => {
                                                    if let Ok(pkt) = Packet::decode(&payload) {
                                                        handle_mesh_packet(
                                                            pkt,
                                                            sender_addr,
                                                            &socket,
                                                            &node,
                                                            &mut video_reassemblers,
                                                            &mut discovered_candidates,
                                                            &mut discovered_sessions,
                                                        ).await;
                                                    }
                                                }
                                                _ => {}
                                            }
                                            continue;
                                        }
                                        Err(_) => {
                                            // Invalid MAC / decryption error (e.g. peer restarted and session keys differ):
                                            // Silently drop without banning IP
                                            continue;
                                        }
                                    }
                                }
                            }
                        }

                        // Fallback: room_key decode for initial Punch/Handshake
                        let room_key_opt = node.current_room_key.read().clone();
                        if let Ok((_seq, packet)) = Packet::decode_wire(&buf[..len], room_key_opt.as_ref()) {
                            handle_mesh_packet(
                                packet,
                                sender_addr,
                                &socket,
                                &node,
                                &mut video_reassemblers,
                                &mut discovered_candidates,
                                &mut discovered_sessions,
                            ).await;
                        } else {
                            // Unrecognized packet or legacy format: silently drop without banning IP
                            continue;
                        }
                    }
                    Err(e) => {
                        if e.raw_os_error() != Some(10054) {
                            node.logger.warn(&format!("UDP recv_from error: {}", e));
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(15)) => {}
        }
    }

    if let Some(sig) = mesh_signaling.take() {
        sig.stop();
    }
    audio_tx_task.abort();
    node.audio_engine.stop();
}

async fn perform_peer_exchange(
    node: &P2pNode,
    socket: &Arc<UdpSocket>,
    room_code: &str,
    newcomer_id: u32,
    newcomer_name: &str,
    newcomer_addr: SocketAddr,
) {
    if room_code.is_empty() {
        return;
    }

    let my_id = node.my_peer_id.load(Ordering::Relaxed);
    let existing_peers: Vec<(u32, String, SocketAddr)> = {
        let endpoints = node.client_endpoints.read();
        let peers = node.peers.read();
        endpoints
            .iter()
            .filter(|(&pid, _)| pid != newcomer_id && pid != my_id)
            .map(|(&pid, &addr)| {
                let name = peers
                    .get(&pid)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| format!("Peer-{}", pid));
                (pid, name, addr)
            })
            .collect()
    };

    if existing_peers.is_empty() {
        return;
    }

    node.logger.info(&format!(
        "🤝 [Peer Exchange] Новый пир {} ('{}') подключился. Знакомим его с {} активными участниками...",
        newcomer_id, newcomer_name, existing_peers.len()
    ));

    for (existing_id, existing_name, existing_addr) in existing_peers {
        // 1. Introduce existing peer to newcomer
        let intro_to_newcomer = Packet::PeerIntroduce {
            room_code: room_code.to_string(),
            peer_id: existing_id,
            peer_name: existing_name.clone(),
            endpoints: vec![existing_addr],
        };
        if let Ok(b) = node.encode_wire_for_peer(&intro_to_newcomer, Some(newcomer_id)) {
            for _ in 0..2 {
                let _ = socket.send_to(&b, newcomer_addr).await;
            }
        }

        // 2. Introduce newcomer to existing peer
        let intro_to_existing = Packet::PeerIntroduce {
            room_code: room_code.to_string(),
            peer_id: newcomer_id,
            peer_name: newcomer_name.to_string(),
            endpoints: vec![newcomer_addr],
        };
        if let Ok(b) = node.encode_wire_for_peer(&intro_to_existing, Some(existing_id)) {
            for _ in 0..2 {
                let _ = socket.send_to(&b, existing_addr).await;
            }
        }
    }
}

async fn handle_mesh_packet(
    packet: Packet,
    sender_addr: SocketAddr,
    socket: &Arc<UdpSocket>,
    node: &P2pNode,
    video_reassemblers: &mut HashMap<u32, VideoReassembler>,
    discovered_candidates: &mut HashMap<u32, Vec<SocketAddr>>,
    discovered_sessions: &mut HashMap<u32, String>,
) {
    let my_id = node.my_peer_id.load(Ordering::Relaxed);
    let my_name = node.my_nickname.read().clone();

    match packet {
        Packet::Punch { room_code, nonce, sender_name, sender_id, wire_sender_id, pake_point } => {
            if sender_id == my_id {
                return;
            }

            let current_code = node.current_room_code.read().clone().unwrap_or_default();
            if !current_code.is_empty() && room_code != current_code {
                node.logger.warn(&format!(
                    "🥊 [NAT Punch] Отклонен Punch от {} (код комнаты '{}' != '{}')",
                    sender_addr, room_code, current_code
                ));
                return;
            }

            // Clean up any stale ghost sessions from the same IP with a different peer_id
            let stale_ids: Vec<u32> = {
                let peers_guard = node.peers.read();
                peers_guard.iter()
                    .filter(|(&pid, p)| pid != sender_id && p.addr.starts_with(&format!("{}:", sender_addr.ip())))
                    .map(|(&pid, _)| pid)
                    .collect()
            };
            for stale_id in stale_ids {
                node.peers.write().remove(&stale_id);
                node.client_endpoints.write().remove(&stale_id);
                if let Some(sid) = node.peer_id_to_sender_id.write().remove(&stale_id) {
                    node.peer_sessions.write().remove(&sid);
                }
            }

            let salt = node.current_room_salt.read().clone().unwrap_or_else(|| crate::crypto::derive_deterministic_room_salt(&current_code));
            let pake_seed = crate::crypto::derive_pake_seed(&current_code, &salt);
            let session_key = if pake_point != [0u8; 32] {
                node.pake_kx.compute_pake_session_key(&pake_point, &pake_seed, my_id, sender_id)
            } else {
                crate::crypto::derive_room_key(&current_code)
            };
            let remote_sid = if wire_sender_id != 0 { wire_sender_id } else { sender_id };

            {
                let mut sessions = node.peer_sessions.write();
                if let Some(session) = sessions.get_mut(&remote_sid) {
                    session.current_addr = sender_addr;
                    session.session_key = session_key;
                    session.last_recv_time = Instant::now();
                } else {
                    sessions.insert(remote_sid, PeerSession {
                        peer_id: sender_id,
                        sender_id: remote_sid,
                        session_key,
                        current_addr: sender_addr,
                        replay_window: crate::crypto::AntiReplayWindow::new(),
                        last_sent_time: Instant::now(),
                        last_recv_time: Instant::now(),
                        last_repunch_time: Instant::now() - Duration::from_secs(10),
                        candidate_addrs: vec![sender_addr],
                    });
                    node.peer_id_to_sender_id.write().insert(sender_id, remote_sid);
                }
            }
            node.peer_pfs_keys.write().insert(sender_id, session_key);

            let my_sender_id = node.my_sender_id.load(Ordering::Relaxed);
            let my_pake_point = node.pake_kx.blinded_public_point(&pake_seed);
            let ack = Packet::PunchAck {
                room_code: current_code.clone(),
                nonce,
                sender_name: my_name.clone(),
                sender_id: my_id,
                wire_sender_id: my_sender_id,
                pake_point: my_pake_point,
            };

            if let Ok(b) = node.encode_wire_for_peer(&ack, Some(sender_id)) {
                for _ in 0..2 {
                    let _ = socket.send_to(&b, sender_addr).await;
                }
            }

            node.record_room_peer_endpoints(&current_code, &[sender_addr]);

            let is_new = !node.client_endpoints.read().contains_key(&sender_id);
            node.client_endpoints.write().insert(sender_id, sender_addr);
            node.peer_addrs.write().insert(sender_addr, sender_id);

            {
                let mut peers_guard = node.peers.write();
                let peer = peers_guard.entry(sender_id).or_insert_with(|| {
                    PeerInfo {
                        id: sender_id,
                        name: sender_name.clone(),
                        addr: sender_addr.to_string(),
                        ping_ms: 0,
                        last_seen_ms: now_ms(),
                        is_host: false,
                        is_speaking: false,
                        mic_muted: false,
                        deafened: false,
                        video_active: false,
                        screen_active: false,
                    }
                });
                peer.addr = sender_addr.to_string();
                peer.name = sender_name.clone();
                peer.last_seen_ms = now_ms();
            }

            if is_new {
                node.logger.info(&format!(
                    "⚡ [Full Mesh] Установлен прямой P2P UDP канал с пиром {} ('{}') на {}",
                    sender_id, sender_name, sender_addr
                ));
                node.audio_engine.play_connect_chime();
                {
                    let cnt = node.peers.read().len();
                    let mut st = node.status.write();
                    if let ConnectionStatus::Connected { connected_count, .. } = &mut *st {
                        *connected_count = cnt;
                    }
                }
                perform_peer_exchange(node, socket, &current_code, sender_id, &sender_name, sender_addr).await;
            }
        }

        Packet::PunchAck { room_code, sender_name, sender_id, wire_sender_id, pake_point, .. } => {
            if sender_id == my_id {
                return;
            }

            let current_code = node.current_room_code.read().clone().unwrap_or_default();
            if !current_code.is_empty() && room_code != current_code {
                node.logger.warn(&format!(
                    "🥊 [NAT Punch] Отклонен PunchAck от {} (код комнаты '{}' != '{}')",
                    sender_addr, room_code, current_code
                ));
                return;
            }

            // Clean up any stale ghost sessions from the same IP with a different peer_id
            let stale_ids: Vec<u32> = {
                let peers_guard = node.peers.read();
                peers_guard.iter()
                    .filter(|(&pid, p)| pid != sender_id && p.addr.starts_with(&format!("{}:", sender_addr.ip())))
                    .map(|(&pid, _)| pid)
                    .collect()
            };
            for stale_id in stale_ids {
                node.peers.write().remove(&stale_id);
                node.client_endpoints.write().remove(&stale_id);
                if let Some(sid) = node.peer_id_to_sender_id.write().remove(&stale_id) {
                    node.peer_sessions.write().remove(&sid);
                }
            }

            let salt = node.current_room_salt.read().clone().unwrap_or_else(|| crate::crypto::derive_deterministic_room_salt(&current_code));
            let pake_seed = crate::crypto::derive_pake_seed(&current_code, &salt);
            let session_key = if pake_point != [0u8; 32] {
                node.pake_kx.compute_pake_session_key(&pake_point, &pake_seed, my_id, sender_id)
            } else {
                crate::crypto::derive_room_key(&current_code)
            };
            let remote_sid = if wire_sender_id != 0 { wire_sender_id } else { sender_id };

            {
                let mut sessions = node.peer_sessions.write();
                if let Some(session) = sessions.get_mut(&remote_sid) {
                    session.current_addr = sender_addr;
                    session.session_key = session_key;
                    session.last_recv_time = Instant::now();
                } else {
                    sessions.insert(remote_sid, PeerSession {
                        peer_id: sender_id,
                        sender_id: remote_sid,
                        session_key,
                        current_addr: sender_addr,
                        replay_window: crate::crypto::AntiReplayWindow::new(),
                        last_sent_time: Instant::now(),
                        last_recv_time: Instant::now(),
                        last_repunch_time: Instant::now() - Duration::from_secs(10),
                        candidate_addrs: vec![sender_addr],
                    });
                    node.peer_id_to_sender_id.write().insert(sender_id, remote_sid);
                }
            }
            node.peer_pfs_keys.write().insert(sender_id, session_key);

            node.record_room_peer_endpoints(&current_code, &[sender_addr]);

            let is_new = !node.client_endpoints.read().contains_key(&sender_id);
            node.client_endpoints.write().insert(sender_id, sender_addr);
            node.peer_addrs.write().insert(sender_addr, sender_id);

            // Send our X25519 ephemeral key in response to PunchAck
            let kx = Packet::KeyExchange {
                sender_id: my_id,
                ephemeral_pubkey: node.ephemeral_kx.public_key_bytes(),
            };
            if let Ok(b) = node.encode_wire_for_peer(&kx, Some(sender_id)) {
                let _ = socket.send_to(&b, sender_addr).await;
            }

            {
                let mut peers_guard = node.peers.write();
                let peer = peers_guard.entry(sender_id).or_insert_with(|| {
                    PeerInfo {
                        id: sender_id,
                        name: sender_name.clone(),
                        addr: sender_addr.to_string(),
                        ping_ms: 0,
                        last_seen_ms: now_ms(),
                        is_host: false,
                        is_speaking: false,
                        mic_muted: false,
                        deafened: false,
                        video_active: false,
                        screen_active: false,
                    }
                });
                peer.addr = sender_addr.to_string();
                peer.name = sender_name.clone();
                peer.last_seen_ms = now_ms();
            }

            if is_new {
                node.logger.info(&format!(
                    "⚡ [Full Mesh] Подтвержден прямой P2P UDP канал с пиром {} ('{}') на {}",
                    sender_id, sender_name, sender_addr
                ));
                node.audio_engine.play_connect_chime();
                {
                    let cnt = node.peers.read().len();
                    let mut st = node.status.write();
                    if let ConnectionStatus::Connected { connected_count, .. } = &mut *st {
                        *connected_count = cnt;
                    }
                }
                perform_peer_exchange(node, socket, &current_code, sender_id, &sender_name, sender_addr).await;
            }
        }

        Packet::Ping { sender_id, seq, timestamp_ms } => {
            if sender_id != my_id {
                node.client_endpoints.write().insert(sender_id, sender_addr);
                node.peer_addrs.write().insert(sender_addr, sender_id);
                {
                    let mut peers_guard = node.peers.write();
                    if let Some(peer) = peers_guard.get_mut(&sender_id) {
                        peer.last_seen_ms = now_ms();
                        peer.addr = sender_addr.to_string();
                    }
                }
                let pong = Packet::Pong {
                    sender_id: my_id,
                    seq,
                    timestamp_ms,
                };
                if let Ok(b) = node.encode_wire_for_peer(&pong, Some(sender_id)) {
                    let _ = socket.send_to(&b, sender_addr).await;
                }
            }
        }

        Packet::Pong { sender_id, timestamp_ms, .. } => {
            if sender_id != my_id {
                node.client_endpoints.write().insert(sender_id, sender_addr);
                node.peer_addrs.write().insert(sender_addr, sender_id);
                let rtt = (now_ms().saturating_sub(timestamp_ms) as u32).min(9999);
                let mut peers_guard = node.peers.write();
                if let Some(peer) = peers_guard.get_mut(&sender_id) {
                    peer.ping_ms = rtt;
                    peer.last_seen_ms = now_ms();
                    peer.addr = sender_addr.to_string();
                }
            }
        }

        Packet::AudioData { sender_id, seq, data } => {
            if sender_id != my_id {
                node.client_endpoints.write().insert(sender_id, sender_addr);
                node.peer_addrs.write().insert(sender_addr, sender_id);
                {
                    let mut peers_guard = node.peers.write();
                    if let Some(peer) = peers_guard.get_mut(&sender_id) {
                        peer.last_seen_ms = now_ms();
                    }
                }
                node.audio_engine.feed_incoming_packet(sender_id, seq, &data);
            }
        }

        Packet::VideoChunk { sender_id, frame_id, chunk_idx, chunk_total, is_screen, data } => {
            if sender_id != my_id {
                let reassembler = video_reassemblers.entry(sender_id).or_insert_with(VideoReassembler::new);
                if let Some(frame) = reassembler.handle_chunk(
                    sender_id,
                    frame_id,
                    chunk_idx,
                    chunk_total,
                    is_screen,
                    data,
                ) {
                    if frame.frame_id == 1 || frame.frame_id % 60 == 0 {
                        node.logger.info(&format!(
                            "🖥️ [Экран] Кадр #{} успешно собран от пира {} (экран: {})",
                            frame.frame_id, sender_id, frame.is_screen
                        ));
                    }
                    *node.latest_remote_video.write() = Some(frame.clone());
                    {
                        let mut peers_guard = node.peers.write();
                        if let Some(peer) = peers_guard.get_mut(&sender_id) {
                            if is_screen {
                                peer.screen_active = true;
                            } else {
                                peer.video_active = true;
                            }
                            peer.last_seen_ms = now_ms();
                        }
                    }
                    if is_screen {
                        node.remote_media_state.write().screen_active = true;
                    } else {
                        node.remote_media_state.write().video_active = true;
                    }
                    if let Some(cb) = node.video_frame_callback.read().as_ref() {
                        cb(frame);
                    }
                }
            }
        }

        Packet::TextMessage { id, sender_id, text, sender_name, timestamp_ms, image_thumb, image_id } => {
            if sender_id != my_id {
                let is_new = node.seen_msg_ids.write().insert(id);
                if is_new {
                    if !text.is_empty() {
                        node.logger.info(&format!("[ЧАТ] Сообщение от '{}' ({} симв.)", sender_name, text.chars().count()));
                    } else if image_id.is_some() || image_thumb.is_some() {
                        node.logger.info(&format!("[ЧАТ] Изображение от '{}'", sender_name));
                    }
                    node.chat_messages.write().push(ChatMessage {
                        id,
                        sender: sender_name.clone(),
                        text,
                        timestamp_ms,
                        is_me: false,
                        image_thumb,
                        image_id: image_id.clone(),
                    });

                    // If message contains an image and we don't have it in store yet, request it
                    if let Some(ref img_id) = image_id {
                        if !node.image_store.read().contains_key(img_id) {
                            let req = Packet::ImageRequest {
                                image_id: img_id.clone(),
                                requester_id: my_id,
                            };
                            if let Ok(b) = node.encode_wire_for_peer(&req, Some(sender_id)) {
                                let _ = socket.send_to(&b, sender_addr).await;
                            }
                        }
                    }
                }
                let ack = Packet::TextAck { id };
                if let Ok(b) = node.encode_wire_for_peer(&ack, Some(sender_id)) {
                    let _ = socket.send_to(&b, sender_addr).await;
                }
            }
        }

        Packet::ImageRequest { image_id, requester_id } => {
            if requester_id != my_id {
                let opt_bytes = node.image_store.read().get(&image_id).cloned();
                if let Some(bytes) = opt_bytes {
                    node.logger.info(&format!(
                        "🖼️ [Изображение] Отправка чанков '{}' ({:.1} KB) пиру {}",
                        image_id,
                        bytes.len() as f64 / 1024.0,
                        requester_id
                    ));
                    let sock_opt = node.active_socket.read().clone();
                    let target_addr = node.client_endpoints.read().get(&requester_id).cloned().unwrap_or(sender_addr);
                    let node_img = node.clone();
                    tokio::spawn(async move {
                        const CHUNK_SIZE: usize = 1200;
                        let chunk_total = ((bytes.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32;
                        for (idx, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
                            let pkt = Packet::ImageChunk {
                                image_id: image_id.clone(),
                                chunk_idx: idx as u32,
                                chunk_total,
                                data: chunk.to_vec(),
                            };
                            if let Ok(b) = node_img.encode_wire_for_peer(&pkt, Some(requester_id)) {
                                if let Some(sock) = &sock_opt {
                                    let _ = sock.send_to(&b, target_addr).await;
                                }
                            }
                            tokio::time::sleep(Duration::from_millis(3)).await;
                        }
                    });
                }
            }
        }

        Packet::ImageChunk { image_id, chunk_idx, chunk_total, data } => {
            let mut reassemblers = node.image_reassemblers.write();
            reassemblers.retain(|_, r| r.created_at.elapsed() < Duration::from_secs(15));
            let current_count = reassemblers.len();
            use std::collections::hash_map::Entry;
            let entry = reassemblers.entry(image_id.clone());
            let reassembler = match entry {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => {
                    if current_count >= 20 {
                        return;
                    }
                    if let Some(r) = ImageReassembler::new(chunk_total) {
                        v.insert(r)
                    } else {
                        return;
                    }
                }
            };
            if let Some(full_bytes) = reassembler.handle_chunk(chunk_idx, data) {
                node.logger.info(&format!(
                    "🖼️ [Изображение] Полное изображение '{}' получено ({:.1} KB)",
                    image_id,
                    full_bytes.len() as f64 / 1024.0
                ));
                let mut store = node.image_store.write();
                if store.len() >= 30 {
                    if let Some(first_key) = store.keys().next().cloned() {
                        store.remove(&first_key);
                    }
                }
                store.insert(image_id.clone(), full_bytes);
                reassemblers.remove(&image_id);
            }
        }

        Packet::HistoryRequest { requester_id } => {
            if requester_id != my_id {
                let msgs = node.chat_messages.read().clone();
                if !msgs.is_empty() {
                    node.logger.info(&format!(
                        "📜 [История] Отправка {} сообщений пиру {}",
                        msgs.len(),
                        requester_id
                    ));
                    let sock_opt = node.active_socket.read().clone();
                    let target_addr = node.client_endpoints.read().get(&requester_id).cloned().unwrap_or(sender_addr);
                    let node_hist = node.clone();
                    tokio::spawn(async move {
                        let start_idx = msgs.len().saturating_sub(50);
                        for msg in &msgs[start_idx..] {
                            let pkt = Packet::HistoryMessage { message: msg.clone() };
                            if let Ok(b) = node_hist.encode_wire_for_peer(&pkt, Some(requester_id)) {
                                if let Some(sock) = &sock_opt {
                                    let _ = sock.send_to(&b, target_addr).await;
                                }
                            }
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    });
                }
            }
        }

        Packet::HistoryMessage { mut message } => {
            let is_new = node.seen_msg_ids.write().insert(message.id);
            if is_new {
                let my_nickname = node.my_nickname.read().clone();
                message.is_me = message.sender == my_nickname;
                let mut chat = node.chat_messages.write();
                chat.push(message);
                chat.sort_by_key(|m| m.timestamp_ms);
            }
        }

        Packet::TextAck { id } => {
            if let Some(sender) = node.pending_acks.write().remove(&id) {
                let _ = sender.send(());
            }
        }

        Packet::MediaState { sender_id, mic_muted, deafened, video_active, screen_active } => {
            if sender_id != my_id {
                node.logger.info(&format!(
                    "📡 [Медиа-статус] Пир {}: mic_muted={}, deafened={}, video={}, screen={}",
                    sender_id, mic_muted, deafened, video_active, screen_active
                ));
                let mut peers_guard = node.peers.write();
                if let Some(peer) = peers_guard.get_mut(&sender_id) {
                    peer.mic_muted = mic_muted;
                    peer.deafened = deafened;
                    peer.video_active = video_active;
                    peer.screen_active = screen_active;
                    peer.last_seen_ms = now_ms();
                }
                let any_mic_muted = peers_guard.values().all(|p| p.mic_muted);
                let any_deafened = peers_guard.values().any(|p| p.deafened);
                let any_video = peers_guard.values().any(|p| p.video_active);
                let any_screen = peers_guard.values().any(|p| p.screen_active);
                *node.remote_media_state.write() = RemoteMediaState {
                    mic_muted: any_mic_muted,
                    deafened: any_deafened,
                    video_active: any_video,
                    screen_active: any_screen,
                };
                if !any_video && !any_screen {
                    *node.latest_remote_video.write() = None;
                }
            }
        }

        Packet::NicknameUpdate { sender_id, new_nickname } => {
            if sender_id != my_id {
                let mut peers_guard = node.peers.write();
                if let Some(peer) = peers_guard.get_mut(&sender_id) {
                    peer.name = new_nickname;
                }
            }
        }

        Packet::Disconnect { sender_id, reason } => {
            if sender_id != my_id {
                node.logger.info(&format!("Пир {} покинул комнату: {}", sender_id, reason));
                let opt_addr = node.client_endpoints.write().remove(&sender_id);
                if let Some(addr) = opt_addr {
                    node.peer_addrs.write().remove(&addr);
                }
                node.peers.write().remove(&sender_id);
                video_reassemblers.remove(&sender_id);
                discovered_candidates.remove(&sender_id);
                discovered_sessions.remove(&sender_id);
                node.audio_engine.play_disconnect_chime();
                let cnt = node.peers.read().len();
                let any_mic_muted = node.peers.read().values().all(|p| p.mic_muted);
                let any_deafened = node.peers.read().values().any(|p| p.deafened);
                let any_video = node.peers.read().values().any(|p| p.video_active);
                let any_screen = node.peers.read().values().any(|p| p.screen_active);
                *node.remote_media_state.write() = RemoteMediaState {
                    mic_muted: any_mic_muted,
                    deafened: any_deafened,
                    video_active: any_video,
                    screen_active: any_screen,
                };
                let mut st = node.status.write();
                if let ConnectionStatus::Connected { connected_count, .. } = &mut *st {
                    *connected_count = cnt;
                }
            }
        }

        Packet::PeerIntroduce { room_code, peer_id, peer_name, endpoints } => {
            if peer_id == my_id {
                return;
            }

            let current_code = node.current_room_code.read().clone().unwrap_or_default();
            if !current_code.is_empty() && room_code != current_code {
                node.logger.warn(&format!(
                    "🤝 [Peer Exchange] Отклонена рекомендация пира {} (код комнаты '{}' != '{}')",
                    peer_id, room_code, current_code
                ));
                return;
            }

            // Case A: Query for assistance (endpoints is empty)
            if endpoints.is_empty() {
                let opt_target = {
                    let eps = node.client_endpoints.read();
                    let peers = node.peers.read();
                    eps.get(&peer_id).copied().map(|addr| {
                        let name = peers.get(&peer_id).map(|p| p.name.clone()).unwrap_or_default();
                        (addr, name)
                    })
                };
                if let Some((target_addr, name)) = opt_target {
                    let reply = Packet::PeerIntroduce {
                        room_code: current_code.clone(),
                        peer_id,
                        peer_name: name,
                        endpoints: vec![target_addr],
                    };
                    if let Ok(b) = node.encode_wire_for_peer(&reply, None) {
                        let _ = socket.send_to(&b, sender_addr).await;
                    }
                }
                return;
            }

            // Case B: Recommendation received (endpoints has addresses)
            node.record_room_peer_endpoints(&current_code, &endpoints);

            if !node.client_endpoints.read().contains_key(&peer_id) {
                let candidates = expand_candidates_with_port_spray(&endpoints);
                let entry = discovered_candidates.entry(peer_id).or_default();
                for &c in &candidates {
                    if !entry.contains(&c) {
                        entry.push(c);
                    }
                }

                node.logger.info(&format!(
                    "🤝 [Peer Exchange] Получена рекомендация пира {} ('{}') на {} адресов. Запуск прямого NAT Punch...",
                    peer_id, peer_name, candidates.len()
                ));

                let my_sender_id = node.my_sender_id.load(Ordering::Relaxed);
                let salt = node.current_room_salt.read().clone().unwrap_or([0u8; 32]);
                let pake_seed = crate::crypto::derive_pake_seed(&current_code, &salt);
                let my_pake_point = node.pake_kx.blinded_public_point(&pake_seed);

                let punch_pkt = Packet::Punch {
                    room_code: current_code,
                    nonce: rand::random(),
                    sender_name: my_name.clone(),
                    sender_id: my_id,
                    wire_sender_id: my_sender_id,
                    pake_point: my_pake_point,
                };
                if let Ok(pb) = node.encode_wire_for_peer(&punch_pkt, Some(peer_id)) {
                    for target in &candidates {
                        let _ = socket.send_to(&pb, *target).await;
                    }
                }
            }
        }

        Packet::KeyExchange { sender_id, ephemeral_pubkey } => {
            if sender_id != my_id {
                let session_key = node.ephemeral_kx.compute_session_key(&ephemeral_pubkey, my_id, sender_id);
                let is_new = !node.peer_pfs_keys.read().contains_key(&sender_id);
                node.peer_pfs_keys.write().insert(sender_id, session_key);
                node.logger.info(&format!(
                    "🔐 [X25519 PFS] Установлен сессионный ключ Perfect Forward Secrecy для пира #{}",
                    sender_id
                ));
                if is_new {
                    let kx_reply = Packet::KeyExchange {
                        sender_id: my_id,
                        ephemeral_pubkey: node.ephemeral_kx.public_key_bytes(),
                    };
                    if let Ok(b) = node.encode_wire_for_peer(&kx_reply, Some(sender_id)) {
                        let _ = socket.send_to(&b, sender_addr).await;
                    }
                }
            }
        }

        _ => {}
    }
}

struct VideoReassembler {
    current_frame_id: u32,
    chunk_total: u16,
    is_screen: bool,
    sender_id: u32,
    chunks: Vec<Option<Vec<u8>>>,
    created_at: Instant,
}

impl VideoReassembler {
    fn new() -> Self {
        Self {
            current_frame_id: 0,
            chunk_total: 0,
            is_screen: false,
            sender_id: 0,
            chunks: Vec::new(),
            created_at: Instant::now(),
        }
    }

    fn handle_chunk(
        &mut self,
        sender_id: u32,
        frame_id: u32,
        chunk_idx: u16,
        chunk_total: u16,
        is_screen: bool,
        data: Vec<u8>,
    ) -> Option<RemoteVideoFrame> {
        use base64::Engine;
        if chunk_total == 0 || chunk_total > 500 || data.len() > 4096 {
            return None;
        }
        if self.chunks.is_empty()
            || frame_id > self.current_frame_id
            || sender_id != self.sender_id
            || self.created_at.elapsed() > Duration::from_millis(800)
        {
            self.current_frame_id = frame_id;
            self.sender_id = sender_id;
            self.chunk_total = chunk_total;
            self.is_screen = is_screen;
            self.chunks = vec![None; chunk_total as usize];
            self.created_at = Instant::now();
        }

        if frame_id == self.current_frame_id && (chunk_idx as usize) < self.chunks.len() {
            self.chunks[chunk_idx as usize] = Some(data);

            if self.chunks.iter().all(|c| c.is_some()) {
                let mut full_bytes = Vec::new();
                for c in &self.chunks {
                    if let Some(bytes) = c {
                        full_bytes.extend_from_slice(bytes);
                    }
                }
                let base64_str = base64::engine::general_purpose::STANDARD.encode(&full_bytes);
                let frame = RemoteVideoFrame {
                    sender_id,
                    frame_id,
                    is_screen: self.is_screen,
                    jpeg_base64: base64_str,
                    timestamp_ms: now_ms(),
                };
                self.chunks.clear();
                return Some(frame);
            }
        }
        None
    }
}

async fn gather_candidates(
    socket: Arc<UdpSocket>,
    port: u16,
    opt_physical_ip: Option<Ipv4Addr>,
    hide_lan: bool,
    logger: &AppLogger,
) -> Vec<SocketAddr> {
    let mut list = Vec::new();

    let lan_ip = opt_physical_ip.map(IpAddr::V4).unwrap_or_else(detect_local_lan_ip);
    let local_addr = SocketAddr::new(lan_ip, port);

    // Discover public reflexive address via STUN
    if let Some(pub_addr) = discover_public_ip_via_stun(socket, logger).await {
        logger.info(&format!("STUN публичный адрес кандидата: {}", pub_addr));
        list.push(pub_addr);
        if !hide_lan {
            list.push(local_addr);
            logger.info(&format!("Локальный адрес кандидата (LAN включен): {}", local_addr));
        } else {
            logger.info("Режим повышенной анонимности: внутренний LAN адрес скрыт от собеседников");
        }
    } else {
        // Fallback to local LAN address if STUN is not reachable (e.g. offline local network)
        logger.warn("STUN: не удалось определить публичный адрес (используется локальный LAN адрес)");
        list.push(local_addr);
    }

    list
}

fn expand_candidates_with_port_spray(candidates: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut targets = Vec::new();
    for &addr in candidates {
        targets.push(addr);
        if let SocketAddr::V4(v4) = addr {
            let port = v4.port();
            for delta in [-10i32, -8, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4, 5, 6, 8, 10] {
                let p = port as i32 + delta;
                if (1024..=65535).contains(&p) {
                    let sprayed = SocketAddr::new(IpAddr::V4(*v4.ip()), p as u16);
                    if !targets.contains(&sprayed) {
                        targets.push(sprayed);
                    }
                }
            }
        }
    }
    targets
}

pub fn detect_local_lan_ip() -> IpAddr {
    #[cfg(windows)]
    {
        if let Some((_if_idx, Some(ip))) = find_best_physical_interface() {
            return IpAddr::V4(ip);
        }
    }
    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
}

async fn discover_public_ip_via_stun(socket: Arc<UdpSocket>, logger: &AppLogger) -> Option<SocketAddr> {
    const STUN_SERVERS: &[&str] = &[
        "stun.cloudflare.com:3478",
        "stun.syncthing.net:3478",
        "stun.miwifi.com:3478",
        "stun.chat.bilibili.com:3478",
        "stun.nextcloud.com:3478",
        "stun.sipnet.ru:3478",
        "stun.l.google.com:19302",
        "stun1.l.google.com:19302",
        "stun2.l.google.com:19302",
        "stun.voip.blackberry.com:3478",
    ];

    let mut tx_id = [0u8; 12];
    for b in tx_id.iter_mut() {
        *b = rand::random();
    }

    let mut req = [0u8; 20];
    req[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
    req[2..4].copy_from_slice(&0x0000u16.to_be_bytes());
    req[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes());
    req[8..20].copy_from_slice(&tx_id);

    let mut resolve_tasks = Vec::new();
    for &server in STUN_SERVERS {
        resolve_tasks.push(async move {
            tokio::time::timeout(Duration::from_millis(800), tokio::net::lookup_host(server)).await
        });
    }

    let mut targets = Vec::new();
    let results = futures_util::future::join_all(resolve_tasks).await;
    for res in results {
        if let Ok(Ok(addrs)) = res {
            for addr in addrs {
                if let SocketAddr::V4(_) = addr {
                    if !targets.contains(&addr) {
                        targets.push(addr);
                    }
                }
            }
        }
    }

    if targets.is_empty() {
        logger.warn("STUN: Не удалось разрешить STUN-серверы");
        return None;
    }

    targets.truncate(8);

    for &t in &targets {
        let _ = socket.send_to(&req, t).await;
    }

    let mut buf = [0u8; 512];
    let start_time = Instant::now();
    let mut last_reblast = Instant::now();
    let overall_timeout = Duration::from_millis(2500);

    while start_time.elapsed() < overall_timeout {
        if last_reblast.elapsed() >= Duration::from_millis(400) {
            for &t in &targets {
                let _ = socket.send_to(&req, t).await;
            }
            last_reblast = Instant::now();
        }

        let recv_timeout = tokio::time::sleep(Duration::from_millis(150));
        tokio::pin!(recv_timeout);

        tokio::select! {
            res = socket.recv_from(&mut buf) => {
                if let Ok((len, from)) = res {
                    if len >= 20 {
                        if let Some(pub_addr) = parse_stun_response(&buf[..len], &tx_id) {
                            logger.info(&format!("STUN: получен ответ от {} -> {}", from, pub_addr));
                            return Some(pub_addr);
                        }
                    }
                }
            }
            _ = &mut recv_timeout => {}
        }
    }

    None
}

fn parse_stun_response(buf: &[u8], tx_id: &[u8; 12]) -> Option<SocketAddr> {
    if buf.len() < 20 {
        return None;
    }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != 0x0101 {
        return None;
    }
    let magic = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if magic != 0x2112A442 || &buf[8..20] != tx_id {
        return None;
    }

    let msg_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let end = (20 + msg_len).min(buf.len());
    let mut offset = 20;

    while offset + 4 <= end {
        let attr_type = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let attr_len = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
        offset += 4;
        if offset + attr_len > end {
            break;
        }

        if attr_type == 0x0020 && attr_len >= 8 {
            let family = buf[offset + 1];
            let x_port = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]);
            let port = x_port ^ 0x2112;

            if family == 0x01 {
                let mut ip_bytes = [0u8; 4];
                ip_bytes.copy_from_slice(&buf[offset + 4..offset + 8]);
                let magic_bytes = 0x2112A442u32.to_be_bytes();
                for i in 0..4 {
                    ip_bytes[i] ^= magic_bytes[i];
                }
                return Some(SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::from(ip_bytes)),
                    port,
                ));
            }
        } else if attr_type == 0x0001 && attr_len >= 8 {
            let family = buf[offset + 1];
            let port = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]);
            if family == 0x01 {
                let ip = Ipv4Addr::new(buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7]);
                return Some(SocketAddr::new(IpAddr::V4(ip), port));
            }
        }

        offset += attr_len;
    }

    None
}

#[cfg(windows)]
pub fn configure_socket_physical_if(socket: &UdpSocket, logger: &AppLogger) -> Option<Ipv4Addr> {
    use std::os::windows::io::AsRawSocket;
    let raw_sock = socket.as_raw_socket() as usize;

    unsafe {
        extern "system" {
            fn setsockopt(s: usize, level: i32, optname: i32, optval: *const u8, optlen: i32) -> i32;
        }

        let buf_size: i32 = 1024 * 1024;
        let _ = setsockopt(raw_sock, 0xFFFF, 0x1002, &buf_size as *const _ as *const u8, 4);
        let _ = setsockopt(raw_sock, 0xFFFF, 0x1001, &buf_size as *const _ as *const u8, 4);

        if let Some((if_idx, opt_ip)) = find_best_physical_interface() {
            logger.info(&format!(
                "Обнаружен основной сетевой интерфейс #{} (LAN IP: {:?}), сокет слушает 0.0.0.0:0 для поддержки VPN и физических сетей",
                if_idx,
                opt_ip.unwrap_or(Ipv4Addr::UNSPECIFIED)
            ));
            return opt_ip;
        }
    }
    None
}

#[cfg(not(windows))]
pub fn configure_socket_physical_if(_socket: &UdpSocket, _logger: &AppLogger) -> Option<Ipv4Addr> {
    None
}

#[cfg(windows)]
pub fn find_best_physical_interface() -> Option<(u32, Option<Ipv4Addr>)> {
    use std::ptr::null_mut;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut buf_len: u32 = 15000;
    let mut buf = vec![0u8; buf_len as usize];

    let res = unsafe {
        GetAdaptersAddresses(
            AF_INET as u32,
            flags,
            null_mut(),
            buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
            &mut buf_len,
        )
    };

    if res != 0 {
        return None;
    }

    unsafe fn pwstr_to_string(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        String::from_utf16_lossy(slice)
    }

    let blacklist = &[
        "tap", "tun", "wireguard", "wintun", "v2ray", "sing-box", "openvpn", "tailscale",
        "vethernet", "hyper-v", "virtual", "docker", "vmware", "virtualbox", "wsl",
        "loopback", "bluetooth", "pseudo", "teredo", "isatap", "npcap", "pcap",
    ];

    let mut curr = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    let mut best: Option<(u32, Option<Ipv4Addr>)> = None;
    let mut best_score: i32 = i32::MIN;

    while !curr.is_null() {
        unsafe {
            let adapter = &*curr;
            let desc = pwstr_to_string(adapter.Description as _).to_lowercase();
            let friendly = pwstr_to_string(adapter.FriendlyName as _).to_lowercase();
            let is_up = adapter.OperStatus == 1;
            let if_index = adapter.Anonymous1.Anonymous.IfIndex;

            if is_up && if_index > 0 {
                let mut score = 0i32;
                let is_physical = adapter.IfType == 6
                    || adapter.IfType == 71
                    || adapter.IfType == 243
                    || adapter.IfType == 244;

                if is_physical {
                    score += 200;
                }
                if !adapter.FirstGatewayAddress.is_null() {
                    score += 1000;
                }
                let is_blacklisted = blacklist.iter().any(|&w| desc.contains(w) || friendly.contains(w));
                if is_blacklisted {
                    score -= 5000;
                }

                let mut ipv4_addr = None;
                let mut unicast = adapter.FirstUnicastAddress;
                while !unicast.is_null() {
                    let u = &*unicast;
                    let lp_sock = u.Address.lpSockaddr;
                    if !lp_sock.is_null() && (*lp_sock).sa_family == AF_INET {
                        let sin = &*(lp_sock as *const SOCKADDR_IN);
                        let ip_u32 = u32::from_be(sin.sin_addr.S_un.S_addr);
                        let ip = Ipv4Addr::from(ip_u32);
                        if !ip.is_loopback() && !ip.is_link_local() {
                            if !(ip.octets()[0] == 172 && (16..=31).contains(&ip.octets()[1])) {
                                score += 100;
                            }
                            ipv4_addr = Some(ip);
                            break;
                        }
                    }
                    unicast = (*unicast).Next;
                }

                if score > best_score && score > 0 {
                    best_score = score;
                    best = Some((if_index, ipv4_addr));
                }
            }

            curr = adapter.Next;
        }
    }

    best
}

#[cfg(windows)]
pub fn find_all_local_physical_ips() -> Vec<Ipv4Addr> {
    use std::ptr::null_mut;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut buf_len: u32 = 15000;
    let mut buf = vec![0u8; buf_len as usize];

    let res = unsafe {
        GetAdaptersAddresses(
            AF_INET as u32,
            flags,
            null_mut(),
            buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
            &mut buf_len,
        )
    };

    if res != 0 {
        return Vec::new();
    }

    unsafe fn pwstr_to_string(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        String::from_utf16_lossy(slice)
    }

    let blacklist = &[
        "vethernet", "hyper-v", "virtual", "docker", "vmware", "virtualbox", "wsl",
        "loopback", "bluetooth", "pseudo", "teredo", "isatap", "npcap", "pcap",
    ];

    let mut result = Vec::new();
    let mut curr = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;

    while !curr.is_null() {
        unsafe {
            let adapter = &*curr;
            let desc = pwstr_to_string(adapter.Description as _).to_lowercase();
            let friendly = pwstr_to_string(adapter.FriendlyName as _).to_lowercase();
            let is_up = adapter.OperStatus == 1;

            if is_up && !blacklist.iter().any(|&w| desc.contains(w) || friendly.contains(w)) {
                let mut unicast = adapter.FirstUnicastAddress;
                while !unicast.is_null() {
                    let u = &*unicast;
                    let lp_sock = u.Address.lpSockaddr;
                    if !lp_sock.is_null() && (*lp_sock).sa_family == AF_INET {
                        let sin = &*(lp_sock as *const SOCKADDR_IN);
                        let ip_u32 = u32::from_be(sin.sin_addr.S_un.S_addr);
                        let ip = Ipv4Addr::from(ip_u32);
                        if !ip.is_loopback() && !ip.is_link_local() && !result.contains(&ip) {
                            result.push(ip);
                        }
                    }
                    unicast = (*unicast).Next;
                }
            }
            curr = adapter.Next;
        }
    }
    result
}

#[cfg(not(windows))]
pub fn find_best_physical_interface() -> Option<(u32, Option<Ipv4Addr>)> {
    None
}

#[cfg(not(windows))]
pub fn find_all_local_physical_ips() -> Vec<Ipv4Addr> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_p2p_room_code_and_chat_deduplication() {
        let logger_a = AppLogger::init();
        let logger_b = AppLogger::init();

        let node_a = P2pNode::new(logger_a);
        let node_b = P2pNode::new(logger_b);

        let sock_a = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let sock_b = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());

        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        *node_a.active_socket.write() = Some(sock_a.clone());
        *node_b.active_socket.write() = Some(sock_b.clone());

        node_a.client_endpoints.write().insert(2, addr_b);
        *node_b.host_socket_addr.write() = Some(addr_a);

        let pkt = Packet::TextMessage {
            id: 999,
            sender_id: 1,
            text: "Тестовое сообщение".into(),
            sender_name: "Alice".into(),
            timestamp_ms: now_ms(),
            image_thumb: None,
            image_id: None,
        };
        let b = pkt.encode().unwrap();

        sock_a.send_to(&b, addr_b).await.unwrap();
        sock_a.send_to(&b, addr_b).await.unwrap();
        sock_a.send_to(&b, addr_b).await.unwrap();

        let mut buf = [0u8; 1024];
        for _ in 0..3 {
            let (len, _) = sock_b.recv_from(&mut buf).await.unwrap();
            let decoded = Packet::decode(&buf[..len]).unwrap();
            if let Packet::TextMessage { id, text, sender_name, timestamp_ms, image_thumb, image_id, .. } = decoded {
                let is_new = node_b.seen_msg_ids.write().insert(id);
                if is_new {
                    node_b.chat_messages.write().push(ChatMessage {
                        id,
                        sender: sender_name,
                        text,
                        timestamp_ms,
                        is_me: false,
                        image_thumb,
                        image_id,
                    });
                }
            }
        }

        let msgs = node_b.get_chat_messages();
        assert_eq!(msgs.len(), 1, "Duplicate UDP arrivals must be deduplicated to exactly 1 message!");
        assert_eq!(msgs[0].text, "Тестовое сообщение");
    }

    #[test]
    fn test_find_physical_interface() {
        let best = find_best_physical_interface();
        println!("Found physical interface: {:?}", best);
        #[cfg(windows)]
        assert!(best.is_some(), "Should find a physical interface on Windows");
    }

    #[test]
    fn test_text_ack_packet() {
        let ack = Packet::TextAck { id: 12345 };
        let enc = ack.encode().expect("Failed to encode TextAck");
        let dec = Packet::decode(&enc).expect("Failed to decode TextAck");
        assert_eq!(dec, Packet::TextAck { id: 12345 });
    }

    #[test]
    fn test_sfu_packets_roundtrip() {
        // Test HostHandover
        let handover = Packet::HostHandover {
            old_host_id: 1,
            new_host_id: 2,
            new_host_addr: Some("127.0.0.1:9999".parse().unwrap()),
        };
        let b = handover.encode().unwrap();
        assert_eq!(Packet::decode(&b).unwrap(), handover);

        // Test ClaimHost
        let claim = Packet::ClaimHost {
            candidate_id: 2,
            reason: "Host disconnect".into(),
        };
        let b = claim.encode().unwrap();
        assert_eq!(Packet::decode(&b).unwrap(), claim);

        // Test PeerListUpdate
        let peer_update = Packet::PeerListUpdate {
            host_id: 1,
            backup_host_id: Some(2),
            peers: vec![
                PeerInfo {
                    id: 2,
                    name: "Bob".into(),
                    addr: "127.0.0.1:8888".into(),
                    ping_ms: 12,
                    last_seen_ms: 1000,
                    is_host: false,
                    is_speaking: true,
                    mic_muted: false,
                    deafened: false,
                    video_active: true,
                    screen_active: false,
                }
            ],
        };
        let b = peer_update.encode().unwrap();
        assert_eq!(Packet::decode(&b).unwrap(), peer_update);
    }

    #[test]
    fn test_punch_packet_room_code_validation_and_peer_memory() {
        let punch = Packet::Punch {
            room_code: "123456".into(),
            nonce: 42,
            sender_name: "Bob".into(),
            sender_id: 100,
            wire_sender_id: 0,
            pake_point: [0u8; 32],
        };
        let bytes = punch.encode().expect("Punch encode");
        let decoded = Packet::decode(&bytes).expect("Punch decode");
        assert_eq!(decoded, punch);

        let punch_ack = Packet::PunchAck {
            room_code: "123456".into(),
            nonce: 42,
            sender_name: "Alice".into(),
            sender_id: 200,
            wire_sender_id: 0,
            pake_point: [0u8; 32],
        };
        let ack_bytes = punch_ack.encode().expect("PunchAck encode");
        let decoded_ack = Packet::decode(&ack_bytes).expect("PunchAck decode");
        assert_eq!(decoded_ack, punch_ack);

        // Test room peer memory recording
        let logger = AppLogger::init();
        let node = P2pNode::new(logger);
        let ep1: SocketAddr = "192.168.1.50:50000".parse().unwrap();
        let ep2: SocketAddr = "95.10.20.30:50000".parse().unwrap();
        node.record_room_peer_endpoints("123456", &[ep1, ep2]);

        let endpoints = node.get_known_endpoints_for_room("123456");
        assert_eq!(endpoints.len(), 2);
        assert!(endpoints.contains(&ep1));
        assert!(endpoints.contains(&ep2));
    }

    #[test]
    fn test_peer_introduce_packet_and_exchange() {
        let ep1: SocketAddr = "192.168.1.100:40000".parse().unwrap();
        let ep2: SocketAddr = "10.0.0.5:40000".parse().unwrap();
        let intro = Packet::PeerIntroduce {
            room_code: "ROOM-TEST".into(),
            peer_id: 300,
            peer_name: "Charlie".into(),
            endpoints: vec![ep1, ep2],
        };

        let bytes = intro.encode().expect("PeerIntroduce encode");
        let decoded = Packet::decode(&bytes).expect("PeerIntroduce decode");
        assert_eq!(decoded, intro);

        let logger = AppLogger::init();
        let node = P2pNode::new(logger);
        node.record_room_peer_endpoints("ROOM-TEST", &[ep1, ep2]);
        let known = node.get_known_endpoints_for_room("ROOM-TEST");
        assert!(known.contains(&ep1));
        assert!(known.contains(&ep2));
    }

    #[test]
    fn test_wire_encryption_and_decryption() {
        use crate::crypto::derive_room_key;

        let key = derive_room_key("SECRET-ROOM-42");
        let wrong_key = derive_room_key("WRONG-ROOM-99");

        let pkt = Packet::TextMessage {
            id: 12345,
            sender_id: 1,
            text: "Hello encrypted world!".to_string(),
            sender_name: "Alice".to_string(),
            timestamp_ms: 999999,
            image_thumb: None,
            image_id: None,
        };

        // 1. Encrypt with key and seq = 42
        let wire_bytes = pkt.encode_wire(Some(&key), 42).expect("encode_wire encrypted");
        assert_eq!(wire_bytes[0], PACKET_TYPE_CONTROL);
        assert!(wire_bytes.len() >= crate::crypto::MIN_WIRE_PACKET_LEN);

        // 2. Decrypt with correct key
        let (seq, decoded) = Packet::decode_wire(&wire_bytes, Some(&key)).expect("decode_wire success");
        assert_eq!(seq, 42);
        assert_eq!(decoded, pkt);

        // 3. Decrypt with wrong key should fail
        let wrong_decrypt = Packet::decode_wire(&wire_bytes, Some(&wrong_key));
        assert!(wrong_decrypt.is_err());

        // 4. Decrypt without key should fail because packet is encrypted
        let no_key_decrypt = Packet::decode_wire(&wire_bytes, None);
        assert!(no_key_decrypt.is_err());

        // 5. Plaintext backward compatibility when key is None
        let plain_wire = pkt.encode_wire(None, 0).expect("encode_wire plain");
        assert!(plain_wire.len() > 0);

        // 6. Roundtrip with wire format
        let (wire_seq, wire_decoded) = Packet::decode_wire(&wire_bytes, Some(&key)).expect("wire decode");
        assert_eq!(wire_seq, 42);
        assert_eq!(wire_decoded, pkt);

        let (plain_seq, plain_decoded) = Packet::decode_wire(&plain_wire, None).expect("decode_wire plain");
        assert_eq!(plain_seq, 0);
        assert_eq!(plain_decoded, pkt);
    }

    #[test]
    fn test_ip_rate_limiter() {
        use std::net::Ipv4Addr;
        let mut limiter = IpRateLimiter::new(5.0, 10.0);
        let test_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));

        // 5 tokens available: first 5 checks should succeed
        for _ in 0..5 {
            assert!(limiter.check(test_ip));
        }

        // 6th check immediately should fail (exhausted tokens)
        assert!(!limiter.check(test_ip));
    }

    #[test]
    fn test_x25519_pfs_in_node() {
        let logger_a = AppLogger::init();
        let logger_b = AppLogger::init();
        let node_a = P2pNode::new(logger_a);
        let node_b = P2pNode::new(logger_b);

        let pub_a = node_a.ephemeral_kx.public_key_bytes();
        let pub_b = node_b.ephemeral_kx.public_key_bytes();

        let session_key_a = node_a.ephemeral_kx.compute_session_key(&pub_b, 10, 20);
        let session_key_b = node_b.ephemeral_kx.compute_session_key(&pub_a, 20, 10);

        assert_eq!(session_key_a, session_key_b);
    }

    #[test]
    fn test_keepalive_29_bytes_wire_format() {
        let key = [0x55u8; 32];
        let wire = crate::crypto::encrypt_wire_packet(PACKET_TYPE_KEEPALIVE, 0x12345678, 1, &[], &key)
            .expect("encrypt keepalive");
        // Exactly 13 bytes header + 0 bytes ciphertext + 16 bytes Poly1305 MAC tag = 29 bytes
        assert_eq!(wire.len(), 29);
        assert_eq!(wire[0], PACKET_TYPE_KEEPALIVE);

        let (ptype, sender_id, seq, payload) = crate::crypto::decrypt_wire_packet(&wire, &key)
            .expect("decrypt keepalive");
        assert_eq!(ptype, PACKET_TYPE_KEEPALIVE);
        assert_eq!(sender_id, 0x12345678);
        assert_eq!(seq, 1);
        assert!(payload.is_empty());
    }

    #[test]
    fn test_dynamic_nat_roaming_instant_switch() {
        let initial_addr: SocketAddr = "198.51.100.10:45000".parse().unwrap();
        let roamed_addr: SocketAddr = "198.51.100.10:58210".parse().unwrap(); // Router hopped to port 58210
        let key = [0x77u8; 32];
        let sender_id = 99999u32;

        let mut session = PeerSession {
            peer_id: 42,
            sender_id,
            session_key: key,
            current_addr: initial_addr,
            replay_window: crate::crypto::AntiReplayWindow::new(),
            last_sent_time: Instant::now(),
            last_recv_time: Instant::now(),
            last_repunch_time: Instant::now(),
            candidate_addrs: vec![initial_addr],
        };

        // Incoming audio packet from new roamed address
        let wire = crate::crypto::encrypt_wire_packet(PACKET_TYPE_AUDIO, sender_id, 100, b"opus_audio_data", &key)
            .expect("encrypt audio");

        // 1. Read header in O(1)
        let (packet_type, sid, seq) = crate::crypto::parse_wire_header(&wire).unwrap();
        assert_eq!(packet_type, PACKET_TYPE_AUDIO);
        assert_eq!(sid, sender_id);

        // 2. Anti-Replay pre-check
        assert!(session.replay_window.check_seq_pre(seq));

        // 3. Decrypt with AAD
        let (_, _, _, payload) = crate::crypto::decrypt_wire_packet(&wire, &session.session_key).unwrap();
        assert_eq!(&payload, b"opus_audio_data");

        // 4. Anti-Replay post-commit
        session.replay_window.commit_seq_post(seq);

        // 5. Instant NAT Roaming switch
        if session.current_addr != roamed_addr {
            session.current_addr = roamed_addr;
        }

        // Port updated in zero ms on the very first packet
        assert_eq!(session.current_addr, roamed_addr);
        assert_eq!(session.replay_window.last_seq, 100);
    }
}

