use balabol_audio::{
    AudioEngineConfig, AudioError, FilterMode, LiveAudioPipeline, SoftwareMixer, VadAnalyzer,
};
use balabol_capture::{
    capture_source_frame, compress_frame_to_jpeg, list_capture_sources, CaptureError, CaptureSource,
};
pub use balabol_crypto::ContactCard;
pub use balabol_storage::StoredRoom;
use balabol_crypto::Identity;
use balabol_net::{
    detect_local_lan_ip, discover_public_address, BalabolPacket, DiscoveredLanPeer, GossipManager,
    LanDiscovery, NetError, PacketPayload, UpnpManager,
};
use balabol_sfu::{NatType, PeerScoreReport, SfuElectionManager, SfuRouter};
use balabol_storage::{CrdtChannelDoc, LocalDatabase, StorageError, StoredChannel, StoredMessage};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::{info, warn};

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Capture error: {0}")]
    Capture(#[from] CaptureError),
    #[error("Audio error: {0}")]
    Audio(#[from] AudioError),
    #[error("Network error: {0}")]
    Net(#[from] NetError),
    #[error("Network request error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Engine initialization error: {0}")]
    Init(String),
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStatusInfo {
    pub public_endpoint: Option<String>,
    pub local_port: u16,
    pub is_upnp_mapped: bool,
    pub nat_type: String,
    pub is_sfu_host: bool,
    pub invite_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMemberCard {
    pub user_id: String,
    pub display_name: String,
    pub role: String,
    pub is_online: bool,
    pub is_in_voice: bool,
}

/// The unified Balabol Native Engine.
/// Houses all subsystems (crypto, local-first storage, audio engine, SFU, capture, network transport).
/// Acts as the single point of contact for the Tauri UI Shell via IPC.
pub struct BalabolEngine {
    pub identity: Identity,
    pub storage: Arc<LocalDatabase>,
    pub crdt_channels: Arc<RwLock<HashMap<String, CrdtChannelDoc>>>,
    pub mixer: Arc<SoftwareMixer>,
    pub vad: Arc<Mutex<VadAnalyzer>>,
    pub audio_config: Arc<RwLock<AudioEngineConfig>>,
    pub audio_pipeline: Arc<LiveAudioPipeline>,
    pub sfu_election: Arc<Mutex<SfuElectionManager>>,
    pub sfu_router: Arc<SfuRouter>,
    pub gossip: Arc<GossipManager>,
    pub lan_discovery: Arc<LanDiscovery>,
    pub udp_port: Arc<RwLock<u16>>,
    pub connected_peers: Arc<RwLock<HashMap<String, String>>>, // user_id -> "ip:port"
    pub http_client: reqwest::Client,
    pub voice_seq: Arc<AtomicU32>,
    pub active_voice_channel: Arc<RwLock<Option<String>>>,
    pub room_voice_participants: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub udp_socket: Arc<RwLock<Option<Arc<tokio::net::UdpSocket>>>>,
    pub public_endpoint: Arc<RwLock<Option<SocketAddr>>>,
    pub is_upnp_mapped: Arc<AtomicBool>,
    pub nat_type: Arc<RwLock<String>>,
    pub latest_screen_frame: Arc<RwLock<Option<String>>>,
    pub screen_streamer_id: Arc<RwLock<Option<String>>>,
    pub screen_capture_running: Arc<AtomicBool>,
}

impl BalabolEngine {
    /// Initialize the Balabol engine with an existing identity or fresh random keypair.
    pub fn init<P: AsRef<Path>>(
        identity: Option<Identity>,
        db_path: P,
    ) -> Result<Arc<Self>, EngineError> {
        let id = identity.unwrap_or_else(Identity::generate);
        let user_id = id.user_id();
        info!("Initializing Balabol Native Engine for user {}", user_id);

        let storage = Arc::new(LocalDatabase::open(db_path)?);
        let audio_config = Arc::new(RwLock::new(AudioEngineConfig::default()));
        let mixer = Arc::new(SoftwareMixer::new());
        let vad = Arc::new(Mutex::new(VadAnalyzer::new(audio_config.read().vad_threshold)));

        let audio_pipeline = Arc::new(LiveAudioPipeline::new(
            audio_config.clone(),
            mixer.clone(),
            vad.clone(),
        ));

        // Try initializing audio playback stream
        let _ = audio_pipeline.start_output_stream();
        let _ = audio_pipeline.start_input_stream();

        // Bind UDP socket for P2P transport
        let std_sock = match std::net::UdpSocket::bind("0.0.0.0:42420") {
            Ok(s) => s,
            Err(_) => std::net::UdpSocket::bind("0.0.0.0:0").map_err(NetError::Io)?,
        };
        let local_port = std_sock.local_addr().map_err(NetError::Io)?.port();
        let _ = std_sock.set_nonblocking(true);

        let lan_discovery = Arc::new(LanDiscovery::new(
            user_id.clone(),
            format!("Balabol-{}", &id.public_key_hex()[..4]),
            local_port,
            id.public_key_hex(),
        ));

        let engine = Arc::new(Self {
            identity: id.clone(),
            storage,
            crdt_channels: Arc::new(RwLock::new(HashMap::new())),
            mixer,
            vad,
            audio_config,
            audio_pipeline,
            sfu_election: Arc::new(Mutex::new(SfuElectionManager::new(user_id.clone()))),
            sfu_router: Arc::new(SfuRouter::new()),
            gossip: Arc::new(GossipManager::new(user_id.clone())),
            lan_discovery,
            udp_port: Arc::new(RwLock::new(local_port)),
            connected_peers: Arc::new(RwLock::new(HashMap::new())),
            http_client: reqwest::Client::new(),
            voice_seq: Arc::new(AtomicU32::new(0)),
            active_voice_channel: Arc::new(RwLock::new(None)),
            room_voice_participants: Arc::new(RwLock::new(HashMap::new())),
            udp_socket: Arc::new(RwLock::new(None)),
            public_endpoint: Arc::new(RwLock::new(None)),
            is_upnp_mapped: Arc::new(AtomicBool::new(false)),
            nat_type: Arc::new(RwLock::new("Detecting...".to_string())),
            latest_screen_frame: Arc::new(RwLock::new(None)),
            screen_streamer_id: Arc::new(RwLock::new(None)),
            screen_capture_running: Arc::new(AtomicBool::new(false)),
        });

        // Seed default channels if empty
        engine.seed_default_channels_if_needed()?;

        // If inside tokio runtime, setup async socket tasks & STUN / UPnP discovery
        if let Ok(_) = tokio::runtime::Handle::try_current() {
            if let Ok(tokio_sock) = tokio::net::UdpSocket::from_std(std_sock) {
                let sock_arc = Arc::new(tokio_sock);
                *engine.udp_socket.write() = Some(sock_arc.clone());

                engine.lan_discovery.start_background_tasks();
                engine.setup_voice_sender(sock_arc.clone());
                engine.setup_packet_receiver(sock_arc.clone());
                engine.setup_nat_traversal(sock_arc, local_port);
            }
        }

        Ok(engine)
    }

    /// Ensure network stack is running (can be called if engine was initialized before tokio runtime).
    pub fn ensure_network_started(&self) {
        if self.udp_socket.read().is_some() {
            return;
        }

        if let Ok(_) = tokio::runtime::Handle::try_current() {
            let std_sock = match std::net::UdpSocket::bind("0.0.0.0:42420") {
                Ok(s) => s,
                Err(_) => match std::net::UdpSocket::bind("0.0.0.0:0") {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Failed to bind UDP socket in ensure_network_started: {}", e);
                        return;
                    }
                },
            };
            let local_port = std_sock.local_addr().map(|a| a.port()).unwrap_or(42420);
            let _ = std_sock.set_nonblocking(true);
            *self.udp_port.write() = local_port;

            if let Ok(tokio_sock) = tokio::net::UdpSocket::from_std(std_sock) {
                let sock_arc = Arc::new(tokio_sock);
                *self.udp_socket.write() = Some(sock_arc.clone());

                self.lan_discovery.start_background_tasks();
                self.setup_voice_sender(sock_arc.clone());
                self.setup_packet_receiver(sock_arc.clone());
                self.setup_nat_traversal(sock_arc, local_port);
                info!("Balabol P2P network stack started on port {}", local_port);
            }
        }
    }

    /// In-memory engine for unit testing and ephemeral sessions.
    pub fn in_memory() -> Result<Arc<Self>, EngineError> {
        let id = Identity::generate();
        let user_id = id.user_id();
        let storage = Arc::new(LocalDatabase::in_memory()?);
        let audio_config = Arc::new(RwLock::new(AudioEngineConfig::default()));
        let mixer = Arc::new(SoftwareMixer::new());
        let vad = Arc::new(Mutex::new(VadAnalyzer::new(audio_config.read().vad_threshold)));

        let audio_pipeline = Arc::new(LiveAudioPipeline::new(
            audio_config.clone(),
            mixer.clone(),
            vad.clone(),
        ));

        let lan_discovery = Arc::new(LanDiscovery::new(
            user_id.clone(),
            "Balabol-Test".to_string(),
            42420,
            id.public_key_hex(),
        ));

        let engine = Arc::new(Self {
            identity: id.clone(),
            storage,
            crdt_channels: Arc::new(RwLock::new(HashMap::new())),
            mixer,
            vad,
            audio_config,
            audio_pipeline,
            sfu_election: Arc::new(Mutex::new(SfuElectionManager::new(user_id.clone()))),
            sfu_router: Arc::new(SfuRouter::new()),
            gossip: Arc::new(GossipManager::new(user_id)),
            lan_discovery,
            udp_port: Arc::new(RwLock::new(42420)),
            connected_peers: Arc::new(RwLock::new(HashMap::new())),
            http_client: reqwest::Client::new(),
            voice_seq: Arc::new(AtomicU32::new(0)),
            active_voice_channel: Arc::new(RwLock::new(None)),
            room_voice_participants: Arc::new(RwLock::new(HashMap::new())),
            udp_socket: Arc::new(RwLock::new(None)),
            public_endpoint: Arc::new(RwLock::new(None)),
            is_upnp_mapped: Arc::new(AtomicBool::new(false)),
            nat_type: Arc::new(RwLock::new("Local-Test".to_string())),
            latest_screen_frame: Arc::new(RwLock::new(None)),
            screen_streamer_id: Arc::new(RwLock::new(None)),
            screen_capture_running: Arc::new(AtomicBool::new(false)),
        });

        engine.seed_default_channels_if_needed()?;
        Ok(engine)
    }

    fn setup_nat_traversal(&self, sock: Arc<tokio::net::UdpSocket>, local_port: u16) {
        let pub_ep_lock = self.public_endpoint.clone();
        let upnp_flag = self.is_upnp_mapped.clone();
        let nat_type_lock = self.nat_type.clone();
        let election = self.sfu_election.clone();
        let my_id = self.user_id();

        tokio::spawn(async move {
            // 1. Try UPnP port mapping first
            let mut upnp = UpnpManager::new(local_port);
            if let Ok(addr) = upnp.try_map_port().await {
                *pub_ep_lock.write() = Some(addr);
                upnp_flag.store(true, Ordering::Relaxed);
                *nat_type_lock.write() = "Open (UPnP)".to_string();
                info!("Global internet endpoint mapped via UPnP: {}", addr);

                // Report Open NAT score for SFU leadership
                election.lock().update_peer_score(PeerScoreReport {
                    user_id: my_id.clone(),
                    avg_rtt_ms: 10.0,
                    jitter_ms: 1.0,
                    packet_loss_pct: 0.0,
                    nat_type: NatType::Open,
                    last_ping_ms: now_ms(),
                });
                return;
            }

            // 2. Fallback to public STUN discovery
            match discover_public_address(&sock).await {
                Ok(addr) => {
                    *pub_ep_lock.write() = Some(addr);
                    *nat_type_lock.write() = "Full Cone (STUN)".to_string();
                    info!("Global internet endpoint discovered via STUN: {}", addr);

                    election.lock().update_peer_score(PeerScoreReport {
                        user_id: my_id,
                        avg_rtt_ms: 25.0,
                        jitter_ms: 2.0,
                        packet_loss_pct: 0.0,
                        nat_type: NatType::FullCone,
                        last_ping_ms: now_ms(),
                    });
                }
                Err(e) => {
                    *nat_type_lock.write() = "Local / Restricted".to_string();
                    warn!("Public IP discovery failed: {}", e);
                }
            }
        });
    }

    fn setup_voice_sender(&self, sock: Arc<tokio::net::UdpSocket>) {
        let peers_lock = self.connected_peers.clone();
        let lan_lock = self.lan_discovery.clone();
        let my_user_id = self.user_id();
        let channel_lock = self.active_voice_channel.clone();
        let seq_counter = self.voice_seq.clone();
        let election = self.sfu_election.clone();

        self.audio_pipeline.set_audio_sender(move |samples| {
            let chan_opt = channel_lock.read().clone();
            let channel_id = match chan_opt {
                Some(c) => c,
                None => return,
            };

            let seq = seq_counter.fetch_add(1, Ordering::Relaxed);
            let packet = BalabolPacket::new(
                my_user_id.clone(),
                PacketPayload::VoiceAudio {
                    channel_id,
                    speaker_id: my_user_id.clone(),
                    seq,
                    timestamp_ms: (now_ms() & 0xFFFFFFFF) as u32,
                    samples,
                },
            );

            let bytes = match packet.to_bytes() {
                Ok(b) => b,
                Err(_) => return,
            };

            let mut targets = Vec::new();

            // Check if there is an elected SFU host that is not us
            let maybe_host = election.lock().current_leader().cloned();
            let is_host = election.lock().is_current_node_sfu();

            if let Some(host_id) = maybe_host {
                if !is_host && host_id != my_user_id {
                    // Send to the elected host
                    if let Some(ep) = peers_lock.read().get(&host_id) {
                        targets.push(ep.clone());
                    }
                }
            }

            // Fallback: send directly to connected peers + LAN discovered peers
            if targets.is_empty() {
                for ep in peers_lock.read().values() {
                    targets.push(ep.clone());
                }
                for lan_peer in lan_lock.get_active_peers() {
                    if !targets.contains(&lan_peer.endpoint) {
                        targets.push(lan_peer.endpoint);
                    }
                }
            }

            if !targets.is_empty() {
                let sock_clone = sock.clone();
                tokio::spawn(async move {
                    for target in targets {
                        let _ = sock_clone.send_to(&bytes, &target).await;
                    }
                });
            }
        });
    }

    fn setup_packet_receiver(&self, sock: Arc<tokio::net::UdpSocket>) {
        let mixer_for_recv = self.mixer.clone();
        let gossip_for_recv = self.gossip.clone();
        let my_id = self.user_id();
        let my_own_pubkey = self.public_key_hex();
        let sock_clone = sock.clone();
        let election = self.sfu_election.clone();
        let sfu_router = self.sfu_router.clone();
        let peers_lock = self.connected_peers.clone();
        let pub_ep_lock = self.public_endpoint.clone();
        let udp_port_lock = self.udp_port.clone();
        let storage_for_recv = self.storage.clone();
        let crdt_for_recv = self.crdt_channels.clone();
        let screen_frame_lock = self.latest_screen_frame.clone();
        let screen_streamer_lock = self.screen_streamer_id.clone();
        let room_voice_lock = self.room_voice_participants.clone();
        let active_voice_lock = self.active_voice_channel.clone();

        tokio::spawn(async move {
            let mut buf = [0u8; 65535];
            loop {
                match sock_clone.recv_from(&mut buf).await {
                    Ok((len, src)) => {
                        if let Ok(packet) = BalabolPacket::from_bytes(&buf[..len]) {
                            if packet.sender_id == my_id {
                                continue;
                            }
                            // Auto-register peer endpoint for bidirectional routing
                            peers_lock.write().insert(packet.sender_id.clone(), src.to_string());

                            match packet.payload {
                                PacketPayload::InviteExchange {
                                    ref my_pubkey,
                                    ref display_name,
                                    ..
                                } => {
                                    let peer_id = format!("bala:{}", my_pubkey);
                                    {
                                        let mut peers = peers_lock.write();
                                        peers.retain(|k, v| !(k.starts_with("peer:") && v == &src.to_string()));
                                        peers.insert(peer_id.clone(), src.to_string());
                                    }

                                    let _ = storage_for_recv.save_contact(&ContactCard {
                                        user_id: peer_id.clone(),
                                        public_key_hex: my_pubkey.clone(),
                                        display_name: display_name.clone(),
                                        avatar_url: None,
                                        bio: Some(src.to_string()),
                                    });

                                    // Reply with our own InviteExchange so remote side registers us
                                    let my_ep = if let Some(ext) = *pub_ep_lock.read() {
                                        ext.to_string()
                                    } else {
                                        let lan_ip = balabol_net::detect_local_lan_ip();
                                        format!("{}:{}", lan_ip, *udp_port_lock.read())
                                    };
                                    let reply = BalabolPacket::new(
                                        my_id.clone(),
                                        PacketPayload::InviteExchange {
                                            my_pubkey: my_own_pubkey.clone(),
                                            my_endpoint: my_ep,
                                            display_name: format!("Балабол-{}", &my_own_pubkey[..4.min(my_own_pubkey.len())]),
                                        },
                                    );
                                    if let Ok(reply_bytes) = reply.to_bytes() {
                                        let _ = sock_clone.send_to(&reply_bytes, src).await;
                                    }
                                }
                                PacketPayload::RoomInvite {
                                    ref room_id,
                                    ref room_name,
                                    ref room_emoji,
                                    ref inviter_id,
                                    ref inviter_name,
                                } => {
                                    let _ = storage_for_recv.save_room(&StoredRoom {
                                        id: room_id.clone(),
                                        name: room_name.clone(),
                                        emoji: room_emoji.clone(),
                                        invite_code: None,
                                        created_at: now_ms() as i64,
                                    });
                                    let _ = storage_for_recv.save_channel(&StoredChannel {
                                        id: format!("c-{}", room_id),
                                        server_id: Some(room_id.clone()),
                                        name: "общий-чат".to_string(),
                                        channel_type: "text".to_string(),
                                        topic: Some(format!("Чат беседы {}", room_name)),
                                        created_at: now_ms() as i64,
                                    });
                                    let _ = storage_for_recv.save_channel(&StoredChannel {
                                        id: format!("v-{}", room_id),
                                        server_id: Some(room_id.clone()),
                                        name: "Голосовой".to_string(),
                                        channel_type: "voice".to_string(),
                                        topic: Some(format!("Голосовой канал {}", room_name)),
                                        created_at: now_ms() as i64 + 1,
                                    });
                                    let _ = storage_for_recv.add_room_member(room_id, inviter_id, "owner");
                                    let _ = storage_for_recv.add_room_member(room_id, &my_id, "member");
                                    info!("Received room invite for '{}' from {}", room_name, inviter_name);
                                }
                                PacketPayload::RoomVoiceState {
                                    ref room_id,
                                    ref user_id,
                                    is_joined,
                                    ..
                                } => {
                                    let mut map = room_voice_lock.write();
                                    let set = map.entry(room_id.clone()).or_insert_with(HashSet::new);
                                    if is_joined {
                                        set.insert(user_id.clone());
                                    } else {
                                        set.remove(user_id);
                                    }
                                }
                                PacketPayload::TextMessage {
                                    ref channel_id,
                                    ref message_id,
                                    ref author_id,
                                    ref content,
                                    timestamp_ms,
                                    ref signature_hex,
                                } => {
                                    let msg = StoredMessage {
                                        id: message_id.clone(),
                                        channel_id: channel_id.clone(),
                                        author_id: author_id.clone(),
                                        content: content.clone(),
                                        attachments: Vec::new(),
                                        timestamp_ms,
                                        signature_hex: signature_hex.clone(),
                                    };
                                    let _ = storage_for_recv.save_message(&msg);
                                    let mut docs = crdt_for_recv.write();
                                    let doc = docs.entry(channel_id.clone()).or_insert_with(CrdtChannelDoc::new);
                                    let _ = doc.append_message(&msg);
                                }
                                PacketPayload::ScreenVideo {
                                    ref streamer_id,
                                    ref payload,
                                    ..
                                } => {
                                    use base64::prelude::*;
                                    let b64 = format!("data:image/jpeg;base64,{}", BASE64_STANDARD.encode(payload));
                                    *screen_frame_lock.write() = Some(b64);
                                    *screen_streamer_lock.write() = Some(streamer_id.clone());
                                }
                                PacketPayload::VoiceAudio {
                                    ref channel_id,
                                    ref speaker_id,
                                    ref samples,
                                    ..
                                } => {
                                    if active_voice_lock.read().as_deref() == Some(channel_id) {
                                        mixer_for_recv.feed_samples(speaker_id, samples);
                                    }

                                    if election.lock().is_current_node_sfu() {
                                        if let Ok(recipients) = sfu_router.route_media_packet(channel_id, speaker_id) {
                                            let targets: Vec<String> = {
                                                let peers = peers_lock.read();
                                                recipients.iter().filter_map(|r| peers.get(r).cloned()).collect()
                                            };
                                            for target_ep in targets {
                                                let _ = sock_clone.send_to(&buf[..len], &target_ep).await;
                                            }
                                        }
                                    }
                                }
                                PacketPayload::Ping { seq, client_timestamp_ms } => {
                                    peers_lock.write().insert(packet.sender_id.clone(), src.to_string());
                                    let pong = BalabolPacket::new(
                                        my_id.clone(),
                                        PacketPayload::Pong {
                                            seq,
                                            client_timestamp_ms,
                                            server_timestamp_ms: now_ms(),
                                        },
                                    );
                                    if let Ok(pong_bytes) = pong.to_bytes() {
                                        let _ = sock_clone.send_to(&pong_bytes, src).await;
                                    }
                                }
                                PacketPayload::Pong { .. } => {
                                    peers_lock.write().insert(packet.sender_id.clone(), src.to_string());
                                }
                                PacketPayload::GossipTable { entries } => {
                                    gossip_for_recv.merge_gossip_table(entries);
                                }
                                PacketPayload::SfuHeartbeat { term, leader_id, timestamp_ms } => {
                                    election.lock().handle_heartbeat(balabol_sfu::SfuHeartbeat {
                                        leader_id,
                                        term,
                                        timestamp_ms,
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!("UDP packet receiver error: {}", e);
                    }
                }
            }
        });
    }



    fn seed_default_channels_if_needed(&self) -> Result<(), EngineError> {
        // Seed rooms if none exist
        let existing_rooms = self.storage.list_rooms()?;
        if existing_rooms.is_empty() {
            let default_rooms = vec![
                StoredRoom {
                    id: "r-lounge".to_string(),
                    name: "Гостиная".to_string(),
                    emoji: "🎮".to_string(),
                    invite_code: None,
                    created_at: now_ms() as i64,
                },
                StoredRoom {
                    id: "r-gaming".to_string(),
                    name: "Игровая".to_string(),
                    emoji: "⚔️".to_string(),
                    invite_code: None,
                    created_at: now_ms() as i64 + 1,
                },
            ];
            for room in &default_rooms {
                self.storage.save_room(room)?;
            }
        }

        // Seed channels if none exist
        let existing = self.storage.list_channels(None)?;
        if existing.is_empty() {
            let default_channels = vec![
                // Гостиная
                StoredChannel {
                    id: "c-general".to_string(),
                    server_id: Some("r-lounge".to_string()),
                    name: "общий-чат".to_string(),
                    channel_type: "text".to_string(),
                    topic: Some("Общение и новости".to_string()),
                    created_at: now_ms() as i64,
                },
                StoredChannel {
                    id: "v-lounge".to_string(),
                    server_id: Some("r-lounge".to_string()),
                    name: "Голосовой".to_string(),
                    channel_type: "voice".to_string(),
                    topic: Some("Голосовой и видео-чат".to_string()),
                    created_at: now_ms() as i64 + 1,
                },
                // Игровая
                StoredChannel {
                    id: "c-gaming".to_string(),
                    server_id: Some("r-gaming".to_string()),
                    name: "стратегия".to_string(),
                    channel_type: "text".to_string(),
                    topic: Some("Обсуждение тактик".to_string()),
                    created_at: now_ms() as i64 + 2,
                },
                StoredChannel {
                    id: "v-gaming-1".to_string(),
                    server_id: Some("r-gaming".to_string()),
                    name: "Катка".to_string(),
                    channel_type: "voice".to_string(),
                    topic: Some("Голосовой для игры".to_string()),
                    created_at: now_ms() as i64 + 3,
                },
            ];
            for ch in default_channels {
                self.storage.save_channel(&ch)?;
            }
        }

        // Ensure local user is added to all existing rooms
        let my_id = self.user_id();
        for r in self.storage.list_rooms()? {
            let _ = self.storage.add_room_member(&r.id, &my_id, "owner");
        }

        Ok(())
    }

    // --- Identity & Profiles ---
    pub fn user_id(&self) -> String {
        self.identity.user_id()
    }

    pub fn public_key_hex(&self) -> String {
        self.identity.public_key_hex()
    }

    pub fn get_my_invite_code(&self) -> String {
        if let Some(ext) = *self.public_endpoint.read() {
            format!("bala://{}@{}", self.identity.public_key_hex(), ext)
        } else {
            let lan_ip = detect_local_lan_ip();
            format!(
                "bala://{}@{}:{}",
                self.identity.public_key_hex(),
                lan_ip,
                *self.udp_port.read()
            )
        }
    }

    pub fn get_network_status(&self) -> NetworkStatusInfo {
        let is_sfu = self.sfu_election.lock().is_current_node_sfu();
        NetworkStatusInfo {
            public_endpoint: self.public_endpoint.read().map(|a| a.to_string()),
            local_port: *self.udp_port.read(),
            is_upnp_mapped: self.is_upnp_mapped.load(Ordering::Relaxed),
            nat_type: self.nat_type.read().clone(),
            is_sfu_host: is_sfu,
            invite_code: self.get_my_invite_code(),
        }
    }

    // --- Audio Control & Live Streams ---
    pub fn set_user_volume(&self, peer_id: &str, volume: f32) -> Result<(), EngineError> {
        self.mixer.set_user_volume(peer_id, volume);
        self.storage.set_user_volume(peer_id, volume)?;
        Ok(())
    }

    pub fn get_user_volume(&self, peer_id: &str) -> f32 {
        self.mixer.get_user_volume(peer_id)
    }

    pub fn set_filter_mode(&self, mode: FilterMode) {
        self.audio_config.write().filter_mode = mode;
        self.audio_pipeline.filter.lock().set_mode(mode);
    }

    pub fn set_mute(&self, muted: bool) {
        self.audio_config.write().is_muted = muted;
    }

    pub fn set_deafen(&self, deafened: bool) {
        let mut cfg = self.audio_config.write();
        cfg.is_deafened = deafened;
        if deafened {
            cfg.is_muted = true;
        }
    }

    pub fn set_echo_test(&self, enabled: bool) {
        self.audio_pipeline.set_echo_test(enabled);
    }

    pub fn is_echo_test_active(&self) -> bool {
        self.audio_pipeline.is_echo_test_active()
    }

    pub fn get_mic_level(&self) -> f32 {
        self.audio_pipeline.get_mic_level()
    }

    pub fn start_voice_channel(&self, channel_id: &str) -> Result<(), EngineError> {
        *self.active_voice_channel.write() = Some(channel_id.to_string());
        self.audio_pipeline
            .set_active_channel(Some(channel_id.to_string()));
        self.sfu_router.join_room(channel_id, &self.user_id());
        let _ = self.audio_pipeline.start_output_stream();
        let _ = self.audio_pipeline.start_input_stream();
        Ok(())
    }

    pub fn stop_voice_channel(&self) {
        if let Some(ch) = self.active_voice_channel.read().as_ref() {
            self.sfu_router.leave_room(ch, &self.user_id());
        }
        *self.active_voice_channel.write() = None;
        self.audio_pipeline.set_active_channel(None);
    }

    pub fn join_room_voice(&self, room_id: &str) -> Result<(), EngineError> {
        let voice_channel_id = format!("v-{}", room_id);
        self.start_voice_channel(&voice_channel_id)?;

        let my_id = self.user_id();
        {
            let mut map = self.room_voice_participants.write();
            map.entry(room_id.to_string()).or_default().insert(my_id.clone());
        }

        let packet = BalabolPacket::new(
            my_id.clone(),
            PacketPayload::RoomVoiceState {
                room_id: room_id.to_string(),
                user_id: my_id,
                is_joined: true,
                is_muted: self.audio_config.read().is_muted,
                is_deafened: self.audio_config.read().is_deafened,
            },
        );
        self.broadcast_packet(&packet);
        Ok(())
    }

    pub fn leave_room_voice(&self, room_id: &str) -> Result<(), EngineError> {
        self.stop_voice_channel();

        let my_id = self.user_id();
        {
            let mut map = self.room_voice_participants.write();
            if let Some(set) = map.get_mut(room_id) {
                set.remove(&my_id);
            }
        }

        let packet = BalabolPacket::new(
            my_id.clone(),
            PacketPayload::RoomVoiceState {
                room_id: room_id.to_string(),
                user_id: my_id,
                is_joined: false,
                is_muted: false,
                is_deafened: false,
            },
        );
        self.broadcast_packet(&packet);
        Ok(())
    }

    pub fn get_room_voice_participants(&self, room_id: &str) -> Vec<String> {
        self.room_voice_participants
            .read()
            .get(room_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn invite_friend_to_room(&self, room_id: &str, friend_user_id: &str) -> Result<(), EngineError> {
        let rooms = self.storage.list_rooms()?;
        let room = match rooms.into_iter().find(|r| r.id == room_id) {
            Some(r) => r,
            None => return Err(EngineError::Init("Комната не найдена".to_string())),
        };

        // 1. Add to local room members
        self.storage.add_room_member(room_id, friend_user_id, "member")?;

        // 2. Lookup friend's endpoint in connected_peers or contacts
        let maybe_ep = {
            let peers = self.connected_peers.read();
            peers.get(friend_user_id).cloned()
        }.or_else(|| {
            self.storage.get_contact(friend_user_id).ok().flatten().and_then(|c| c.bio)
        });

        if let Some(ep) = maybe_ep {
            let my_id = self.user_id();
            let my_name = format!("Балабол-{}", &self.public_key_hex()[..4.min(self.public_key_hex().len())]);
            let packet = BalabolPacket::new(
                my_id.clone(),
                PacketPayload::RoomInvite {
                    room_id: room.id,
                    room_name: room.name,
                    room_emoji: room.emoji,
                    inviter_id: my_id,
                    inviter_name: my_name,
                },
            );
            if let Ok(bytes) = packet.to_bytes() {
                if let Some(sock) = self.udp_socket.read().as_ref() {
                    let sock_clone = sock.clone();
                    tokio::spawn(async move {
                        for _ in 0..3 {
                            let _ = sock_clone.send_to(&bytes, &ep).await;
                            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                        }
                    });
                }
            }
        }

        Ok(())
    }

    pub fn list_room_members(&self, room_id: &str) -> Result<Vec<RoomMemberCard>, EngineError> {
        let stored_members = self.storage.list_room_members(room_id)?;
        let connected = self.connected_peers.read();
        let voice_participants = self.get_room_voice_participants(room_id);
        let my_id = self.user_id();

        let mut cards = Vec::new();
        for m in stored_members {
            let is_self = m.user_id == my_id;
            let display_name = if is_self {
                "Вы (Локальный узел)".to_string()
            } else if let Ok(Some(contact)) = self.storage.get_contact(&m.user_id) {
                contact.display_name
            } else {
                format!("Участник-{}", &m.user_id[..4.min(m.user_id.len())])
            };

            let is_online = is_self || connected.contains_key(&m.user_id);
            let is_in_voice = voice_participants.contains(&m.user_id);

            cards.push(RoomMemberCard {
                user_id: m.user_id,
                display_name,
                role: m.role,
                is_online,
                is_in_voice,
            });
        }
        Ok(cards)
    }

    pub fn broadcast_packet(&self, packet: &BalabolPacket) {
        if let Ok(bytes) = packet.to_bytes() {
            if let Some(sock) = self.udp_socket.read().as_ref() {
                let targets: Vec<String> = self.connected_peers.read().values().cloned().collect();
                let sock_clone = sock.clone();
                tokio::spawn(async move {
                    for target in targets {
                        let _ = sock_clone.send_to(&bytes, &target).await;
                    }
                });
            }
        }
    }

    // --- LAN & P2P Peer Connectivity ---
    pub fn get_lan_peers(&self) -> Vec<DiscoveredLanPeer> {
        self.lan_discovery.get_active_peers()
    }

    pub fn connect_peer_direct(&self, endpoint_or_invite: &str) -> Result<String, EngineError> {
        self.ensure_network_started();

        let clean = endpoint_or_invite.trim();
        let (pubkey, endpoint) = if clean.starts_with("bala://") {
            let without_proto = &clean["bala://".len()..];
            if let Some((pk, ep)) = without_proto.split_once('@') {
                (pk.to_string(), ep.to_string())
            } else {
                ("".to_string(), without_proto.to_string())
            }
        } else {
            ("".to_string(), clean.to_string())
        };

        let peer_id = if !pubkey.is_empty() {
            format!("bala:{}", pubkey)
        } else {
            format!("peer:{}", endpoint.replace(':', "_"))
        };

        self.connected_peers
            .write()
            .insert(peer_id.clone(), endpoint.clone());

        // Also record in contacts
        let _ = self.storage.save_contact(&ContactCard {
            user_id: peer_id.clone(),
            public_key_hex: pubkey.clone(),
            display_name: if !pubkey.is_empty() {
                format!("Балабол-{}", &pubkey[..4.min(pubkey.len())])
            } else {
                endpoint.clone()
            },
            avatar_url: None,
            bio: Some(endpoint.clone()),
        });

        // Determine our own endpoint (public if known, else local LAN IP)
        let my_endpoint = if let Some(ext) = *self.public_endpoint.read() {
            ext.to_string()
        } else {
            let lan_ip = detect_local_lan_ip();
            format!("{}:{}", lan_ip, *self.udp_port.read())
        };

        if let Some(sock) = self.udp_socket.read().as_ref() {
            let exchange = BalabolPacket::new(
                self.user_id(),
                PacketPayload::InviteExchange {
                    my_pubkey: self.public_key_hex(),
                    my_endpoint: my_endpoint.clone(),
                    display_name: format!("Balabol-{}", &self.public_key_hex()[..4.min(self.public_key_hex().len())]),
                },
            );
            if let Ok(bytes) = exchange.to_bytes() {
                let sock_clone = sock.clone();
                let target = endpoint.clone();
                tokio::spawn(async move {
                    for _ in 0..5 {
                        let _ = sock_clone.send_to(&bytes, &target).await;
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                    }
                });
            }
        }

        Ok(peer_id)
    }

    // --- Friend & Contact Book Management ---
    pub fn list_contacts(&self) -> Result<Vec<ContactCard>, EngineError> {
        Ok(self.storage.list_contacts()?)
    }

    pub fn add_friend(&self, name: &str, invite_or_id: &str) -> Result<ContactCard, EngineError> {
        let clean = invite_or_id.trim();
        let (pubkey, endpoint) = if clean.starts_with("bala://") {
            let without_proto = &clean["bala://".len()..];
            if let Some((pk, ep)) = without_proto.split_once('@') {
                (pk.to_string(), Some(ep.to_string()))
            } else {
                ("".to_string(), Some(without_proto.to_string()))
            }
        } else if clean.starts_with("bala:") {
            (clean["bala:".len()..].to_string(), None)
        } else {
            (clean.to_string(), None)
        };

        let user_id = if pubkey.is_empty() {
            format!("bala:{}", &clean[..clean.len().min(16)])
        } else {
            format!("bala:{}", pubkey)
        };

        let card = ContactCard {
            user_id: user_id.clone(),
            public_key_hex: pubkey.clone(),
            display_name: name.to_string(),
            avatar_url: None,
            bio: endpoint.clone(),
        };

        self.storage.save_contact(&card)?;

        // If endpoint was provided in invite, register in connected_peers and probe
        if let Some(ref ep) = endpoint {
            let _ = self.connect_peer_direct(&format!("bala://{}@{}", pubkey, ep));
        }

        Ok(card)
    }

    pub fn delete_contact(&self, user_id: &str) -> Result<(), EngineError> {
        self.storage.delete_contact(user_id)?;
        self.connected_peers.write().remove(user_id);
        Ok(())
    }

    pub fn get_connected_peers(&self) -> Vec<String> {
        self.connected_peers.read().keys().cloned().collect()
    }

    // --- Messaging & Channels CRUD ---
    pub fn send_message(&self, channel_id: &str, content: &str) -> Result<StoredMessage, EngineError> {
        let msg_id = format!("m-{}-{}", now_ms(), &self.public_key_hex()[..6]);
        let signature = self.identity.sign_hex(content.as_bytes());

        let msg = StoredMessage {
            id: msg_id,
            channel_id: channel_id.to_string(),
            author_id: self.user_id(),
            content: content.to_string(),
            attachments: Vec::new(),
            timestamp_ms: now_ms() as i64,
            signature_hex: Some(signature),
        };

        // 1. Save to local SQLite
        self.storage.save_message(&msg)?;

        // 2. Append to CRDT doc for this channel
        {
            let mut channels = self.crdt_channels.write();
            let doc = channels
                .entry(channel_id.to_string())
                .or_insert_with(CrdtChannelDoc::new);
            doc.append_message(&msg)?;
        }

        // 3. Broadcast to all connected peers over UDP
        if let Some(sock) = self.udp_socket.read().as_ref() {
            let packet = BalabolPacket::new(
                self.user_id(),
                PacketPayload::TextMessage {
                    channel_id: channel_id.to_string(),
                    message_id: msg.id.clone(),
                    author_id: msg.author_id.clone(),
                    content: msg.content.clone(),
                    timestamp_ms: msg.timestamp_ms,
                    signature_hex: msg.signature_hex.clone(),
                },
            );
            if let Ok(bytes) = packet.to_bytes() {
                let sock_clone = sock.clone();
                let targets: Vec<String> = self.connected_peers.read().values().cloned().collect();
                tokio::spawn(async move {
                    for target in targets {
                        let _ = sock_clone.send_to(&bytes, &target).await;
                    }
                });
            }
        }

        Ok(msg)
    }

    pub fn get_messages(&self, channel_id: &str, limit: usize) -> Result<Vec<StoredMessage>, EngineError> {
        Ok(self.storage.get_messages(channel_id, limit)?)
    }

    pub fn list_channels(&self) -> Result<Vec<StoredChannel>, EngineError> {
        Ok(self.storage.list_channels(None)?)
    }

    pub fn list_channels_for_room(&self, room_id: &str) -> Result<Vec<StoredChannel>, EngineError> {
        Ok(self.storage.list_channels(Some(room_id))?)
    }

    // --- Rooms CRUD ---
    pub fn list_rooms(&self) -> Result<Vec<StoredRoom>, EngineError> {
        Ok(self.storage.list_rooms()?)
    }

    pub fn create_room(&self, name: &str, emoji: &str) -> Result<StoredRoom, EngineError> {
        let room = StoredRoom {
            id: format!("r-{}", now_ms()),
            name: name.to_string(),
            emoji: emoji.to_string(),
            invite_code: Some(self.get_my_invite_code()),
            created_at: now_ms() as i64,
        };
        self.storage.save_room(&room)?;
        // Auto-create default text + voice channels inside the room
        let text_ch = StoredChannel {
            id: format!("c-{}-txt", now_ms()),
            server_id: Some(room.id.clone()),
            name: "общий-чат".to_string(),
            channel_type: "text".to_string(),
            topic: None,
            created_at: now_ms() as i64,
        };
        let voice_ch = StoredChannel {
            id: format!("c-{}-vc", now_ms()),
            server_id: Some(room.id.clone()),
            name: "Голосовой".to_string(),
            channel_type: "voice".to_string(),
            topic: None,
            created_at: now_ms() as i64 + 1,
        };
        self.storage.save_channel(&text_ch)?;
        self.storage.save_channel(&voice_ch)?;
        Ok(room)
    }

    pub fn delete_room(&self, room_id: &str) -> Result<(), EngineError> {
        // Get channels to clean up CRDT state
        if let Ok(channels) = self.storage.list_channels(Some(room_id)) {
            for ch in channels {
                self.crdt_channels.write().remove(&ch.id);
                let is_active = self.active_voice_channel.read().as_ref() == Some(&ch.id);
                if is_active {
                    self.stop_voice_channel();
                }
            }
        }
        self.storage.delete_room(room_id)?;
        Ok(())
    }

    pub fn create_channel(&self, name: &str, channel_type: &str, room_id: Option<&str>) -> Result<StoredChannel, EngineError> {
        let ch = StoredChannel {
            id: format!("c-{}", now_ms()),
            server_id: room_id.map(|s| s.to_string()).or_else(|| Some("r-lounge".to_string())),
            name: name.to_string(),
            channel_type: channel_type.to_string(),
            topic: None,
            created_at: now_ms() as i64,
        };
        self.storage.save_channel(&ch)?;
        Ok(ch)
    }

    pub fn delete_channel(&self, channel_id: &str) -> Result<(), EngineError> {
        self.storage.delete_channel(channel_id)?;
        self.crdt_channels.write().remove(channel_id);
        if let Some(active) = self.active_voice_channel.read().as_ref() {
            if active == channel_id {
                self.stop_voice_channel();
            }
        }
        Ok(())
    }

    // --- Screen / Window Capture & Streaming ---
    pub fn list_capture_sources(&self) -> Result<Vec<CaptureSource>, EngineError> {
        Ok(list_capture_sources()?)
    }

    pub fn start_screen_share(&self, channel_id: &str, source_id: &str) -> Result<(), EngineError> {
        use base64::prelude::*;
        self.screen_capture_running.store(true, Ordering::Relaxed);
        *self.screen_streamer_id.write() = Some(self.user_id());

        let running = self.screen_capture_running.clone();
        let channel_id_str = channel_id.to_string();
        let source_id_str = source_id.to_string();
        let my_user_id = self.user_id();
        let udp_sock_opt = self.udp_socket.read().clone();
        let peers_lock = self.connected_peers.clone();
        let local_frame_lock = self.latest_screen_frame.clone();

        tokio::spawn(async move {
            let mut seq = 0u32;
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(66)); // ~15 FPS
            while running.load(Ordering::Relaxed) {
                interval.tick().await;
                if let Ok(raw) = capture_source_frame(&source_id_str) {
                    if let Ok(jpeg) = compress_frame_to_jpeg(&raw, 1280, 65) {
                        let b64 = format!("data:image/jpeg;base64,{}", BASE64_STANDARD.encode(&jpeg));
                        *local_frame_lock.write() = Some(b64);

                        if let Some(ref sock) = udp_sock_opt {
                            seq = seq.wrapping_add(1);
                            let packet = BalabolPacket::new(
                                my_user_id.clone(),
                                PacketPayload::ScreenVideo {
                                    channel_id: channel_id_str.clone(),
                                    streamer_id: my_user_id.clone(),
                                    frame_seq: seq,
                                    is_keyframe: true,
                                    payload: jpeg,
                                },
                            );
                            if let Ok(bytes) = packet.to_bytes() {
                                let targets: Vec<String> = peers_lock.read().values().cloned().collect();
                                for target in targets {
                                    let _ = sock.send_to(&bytes, &target).await;
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub fn stop_screen_share(&self) {
        self.screen_capture_running.store(false, Ordering::Relaxed);
        *self.screen_streamer_id.write() = None;
        *self.latest_screen_frame.write() = None;
    }

    pub fn get_latest_screen_frame(&self) -> Option<String> {
        self.latest_screen_frame.read().clone()
    }

    pub fn get_screen_streamer_id(&self) -> Option<String> {
        self.screen_streamer_id.read().clone()
    }

    // --- Rendezvous Announce ---
    pub async fn announce_to_rendezvous(
        &self,
        tracker_url: &str,
        endpoint: &str,
        nat_type: &str,
    ) -> Result<bool, EngineError> {
        let timestamp_ms = now_ms();
        let message = format!("{}:{}:{}", endpoint, timestamp_ms, nat_type);
        let signature_hex = self.identity.sign_hex(message.as_bytes());

        let body = serde_json::json!({
            "public_key_hex": self.identity.public_key_hex(),
            "endpoint": endpoint,
            "nat_type": nat_type,
            "timestamp_ms": timestamp_ms,
            "signature_hex": signature_hex,
        });

        let url = format!("{}/api/v1/announce", tracker_url.trim_end_matches('/'));
        let res = self.http_client.post(&url).json(&body).send().await?;
        Ok(res.status().is_success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_init_and_messaging() {
        let engine = BalabolEngine::in_memory().unwrap();
        assert!(engine.user_id().starts_with("bala:"));

        let channels = engine.list_channels().unwrap();
        assert!(!channels.is_empty());

        let msg = engine.send_message("c-general", "Hello from Balabol Core!").unwrap();
        assert_eq!(msg.content, "Hello from Balabol Core!");
        assert!(msg.signature_hex.is_some());

        let messages = engine.get_messages("c-general", 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello from Balabol Core!");
    }

    #[test]
    fn test_channel_crud_and_delete() {
        let engine = BalabolEngine::in_memory().unwrap();
        let new_ch = engine.create_channel("Новая комната", "voice", None).unwrap();
        assert_eq!(new_ch.name, "Новая комната");

        let chs = engine.list_channels().unwrap();
        assert!(chs.iter().any(|c| c.id == new_ch.id));

        engine.delete_channel(&new_ch.id).unwrap();
        let chs_after = engine.list_channels().unwrap();
        assert!(!chs_after.iter().any(|c| c.id == new_ch.id));
    }

    #[test]
    fn test_volume_and_audio_controls() {
        let engine = BalabolEngine::in_memory().unwrap();
        engine.set_user_volume("bala:friend1", 1.5).unwrap();
        assert_eq!(engine.get_user_volume("bala:friend1"), 1.5);

        engine.set_filter_mode(FilterMode::DeepFilterNet);
        assert_eq!(engine.audio_config.read().filter_mode, FilterMode::DeepFilterNet);

        engine.set_deafen(true);
        assert!(engine.audio_config.read().is_deafened);
        assert!(engine.audio_config.read().is_muted);

        engine.set_echo_test(true);
        assert!(engine.is_echo_test_active());
        engine.set_echo_test(false);
        assert!(!engine.is_echo_test_active());
    }

    #[tokio::test]
    async fn test_invite_code_and_direct_connect() {
        let engine = BalabolEngine::in_memory().unwrap();
        let invite = engine.get_my_invite_code();
        assert!(invite.starts_with("bala://"));

        let peer_id = engine.connect_peer_direct("bala://abcdef0123456789@192.168.1.100:42420").unwrap();
        assert!(peer_id.starts_with("bala:abcdef01"));
        assert_eq!(engine.connected_peers.read().get(&peer_id).unwrap(), "192.168.1.100:42420");
    }

    #[test]
    fn test_network_status_info() {
        let engine = BalabolEngine::in_memory().unwrap();
        let status = engine.get_network_status();
        assert_eq!(status.local_port, 42420);
        assert_eq!(status.nat_type, "Local-Test");
        assert!(status.invite_code.starts_with("bala://"));
    }
}
