use crate::invite::{
    create_answer_link_with_id, create_offer_link_with_id, parse_answer_link, parse_offer_link,
};
use crate::logger::AppLogger;
use crate::signaling::{HostSignalingSession, MqttSignaling};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

#[allow(dead_code)]
pub const PROTOCOL_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Packet {
    Punch {
        nonce: u64,
        sender_name: String,
        sender_id: u32,
    },
    PunchAck {
        nonce: u64,
        sender_name: String,
        sender_id: u32,
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
    },
    TextAck {
        id: u64,
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
    },
    ClaimHost {
        candidate_id: u32,
        reason: String,
    },
    NicknameUpdate {
        sender_id: u32,
        new_nickname: String,
    },
}

impl Packet {
    pub fn encode(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(buf)
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub sender: String,
    pub text: String,
    pub timestamp_ms: u64,
    pub is_me: bool,
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
    pub peer_volume: f32,
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

        if let Ok(bytes) = disc.encode() {
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
        if let Some(relay_tx) = self.relay_sender.read().clone() {
            tokio::spawn(async move {
                let _ = relay_tx.send(disc).await;
            });
        }

        self.logger.info("Отключение P2P соединения...");
        self.audio_engine.stop();
        *self.latest_remote_video.write() = None;
        *self.remote_media_state.write() = RemoteMediaState::default();
        *self.active_socket.write() = None;
        *self.host_socket_addr.write() = None;
        *self.relay_sender.write() = None;
        self.client_endpoints.write().clear();
        self.peer_addrs.write().clear();
        self.peers.write().clear();
        self.seen_msg_ids.write().clear();
        self.pending_acks.write().clear();
        self.is_host.store(false, Ordering::SeqCst);
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

    pub fn set_peer_volume(&self, volume: f32) {
        self.audio_engine.set_peer_volume(volume);
    }

    pub fn get_voice_status(&self) -> VoiceStatus {
        let remote_media = self.remote_media_state.read().clone();
        let (out_name, in_name) = self.audio_engine.get_device_names();
        VoiceStatus {
            mic_muted: self.audio_engine.is_mic_muted(),
            deafened: self.audio_engine.is_deafened(),
            peer_volume: self.audio_engine.get_peer_volume(),
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

    pub async fn send_video_frame(&self, jpeg_base64: String, is_screen: bool) -> Result<(), String> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&jpeg_base64)
            .map_err(|e| format!("Base64 decode error: {}", e))?;

        if bytes.is_empty() {
            return Ok(());
        }

        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let frame_id = self.video_frame_seq.fetch_add(1, Ordering::SeqCst);
        const CHUNK_SIZE: usize = 1200;
        let chunk_total = ((bytes.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u16;

        for (idx, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
            let pkt = Packet::VideoChunk {
                sender_id: my_id,
                frame_id,
                chunk_idx: idx as u16,
                chunk_total,
                is_screen,
                data: chunk.to_vec(),
            };

            self.broadcast_packet(&pkt).await;
            tokio::time::sleep(Duration::from_micros(150)).await;
        }

        Ok(())
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
        let pkt = Packet::MediaState {
            sender_id: my_id,
            mic_muted: self.audio_engine.is_mic_muted(),
            deafened: self.audio_engine.is_deafened(),
            video_active: false,
            screen_active: false,
        };
        let self_clone = self.clone();
        tokio::spawn(async move {
            self_clone.broadcast_packet(&pkt).await;
        });
        Ok(())
    }

    pub async fn broadcast_packet(&self, packet: &Packet) {
        if let Ok(bytes) = packet.encode() {
            let sock_opt = self.active_socket.read().clone();
            if let Some(sock) = sock_opt {
                // If Host: send to all connected clients
                let client_addrs: Vec<SocketAddr> = self.client_endpoints.read().values().cloned().collect();
                for addr in client_addrs {
                    let _ = sock.send_to(&bytes, addr).await;
                }
                // If Client: send to Host
                let host_addr_opt = *self.host_socket_addr.read();
                if let Some(host_addr) = host_addr_opt {
                    let _ = sock.send_to(&bytes, host_addr).await;
                }
            }
            let relay_tx_opt = self.relay_sender.read().clone();
            if let Some(relay_tx) = relay_tx_opt {
                let _ = relay_tx.send(packet.clone()).await;
            }
        }
    }

    pub async fn send_chat_message(&self, text: String, my_name: String) -> Result<ChatMessage, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("Сообщение не может быть пустым".into());
        }

        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        let id = (rand::random::<u64>() & 0x7FFF_FFFF_FFFF_FFFF).max(1);
        let timestamp_ms = now_ms();
        self.seen_msg_ids.write().insert(id);

        let packet = Packet::TextMessage {
            id,
            sender_id: my_id,
            text: trimmed.to_string(),
            sender_name: my_name.clone(),
            timestamp_ms,
        };

        // Broadcast with retry
        self.broadcast_packet(&packet).await;

        let msg = ChatMessage {
            id,
            sender: my_name,
            text: trimmed.to_string(),
            timestamp_ms,
            is_me: true,
        };

        self.chat_messages.write().push(msg.clone());
        self.logger.info(&format!("[ЧАТ] Я: {}", trimmed));
        Ok(msg)
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

    /// Transfer host responsibility to another connected peer
    pub async fn transfer_host(&self, new_host_id: u32) -> Result<(), String> {
        if !self.is_host.load(Ordering::Relaxed) {
            return Err("Только текущий хост может передать роль хоста".into());
        }
        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        if new_host_id == my_id {
            return Ok(());
        }

        self.logger.info(&format!(
            "👑 Передача роли хоста от узла {} к узлу {}",
            my_id, new_host_id
        ));

        let handover_pkt = Packet::HostHandover {
            old_host_id: my_id,
            new_host_id,
        };

        self.broadcast_packet(&handover_pkt).await;
        self.is_host.store(false, Ordering::SeqCst);
        self.current_host_id.store(new_host_id, Ordering::SeqCst);

        // Point host_socket_addr to the new host so our outgoing audio/video goes directly to them
        if let Some(target_addr) = self.client_endpoints.read().get(&new_host_id).cloned() {
            *self.host_socket_addr.write() = Some(target_addr);
        }

        // Update local peer roster
        let new_host_name = {
            let mut peers_guard = self.peers.write();
            if let Some(target) = peers_guard.get_mut(&new_host_id) {
                target.is_host = true;
                target.name.clone()
            } else {
                "Организатор".into()
            }
        };

        // Update connection status
        let mut st = self.status.write();
        if let ConnectionStatus::Connected { is_host, host_name, .. } = &mut *st {
            *is_host = false;
            *host_name = new_host_name;
        }

        Ok(())
    }

    /// Promote this node to SFU Host (used on failover when host disconnects)
    pub async fn promote_to_host(&self) {
        let my_id = self.my_peer_id.load(Ordering::Relaxed);
        self.logger.warn(&format!(
            "👑 [Failover] Узел {} принимает на себя роль SFU Хоста комнаты!",
            my_id
        ));

        self.is_host.store(true, Ordering::SeqCst);
        self.current_host_id.store(my_id, Ordering::SeqCst);
        *self.host_socket_addr.write() = None;

        let claim_pkt = Packet::ClaimHost {
            candidate_id: my_id,
            reason: "Предыдущий хост отключился".into(),
        };
        self.broadcast_packet(&claim_pkt).await;

        let my_name = self.my_nickname.read().clone();
        let _room_code = self.current_room_code.read().clone().unwrap_or_default();

        let mut st = self.status.write();
        if let ConnectionStatus::Connected { is_host, host_name, .. } = &mut *st {
            *is_host = true;
            *host_name = my_name;
        }
    }

    /// Mode: Host starts Star-SFU room with a 6-digit code
    pub async fn start_room(
        &self,
        room_code: String,
        nickname: String,
    ) -> Result<String, String> {
        self.disconnect();
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.shutdown_signal.store(false, Ordering::SeqCst);

        let clean_code = crate::signaling::sanitize_room_code(&room_code);
        self.logger.info(&format!(
            "Создание Star-SFU комнаты '{}' (Код: {}), хост '{}'",
            room_code, clean_code, nickname
        ));

        // 1. Bind UDP socket
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| format!("Не удалось создать UDP сокет: {}", e))?;
        let opt_physical_ip = configure_socket_physical_if(&socket, &self.logger);
        let actual_port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
        let arc_socket = Arc::new(socket);
        *self.active_socket.write() = Some(arc_socket.clone());

        // 2. Gather candidates (LAN + STUN)
        let candidates = gather_candidates(arc_socket.clone(), actual_port, opt_physical_ip, &self.logger).await;

        let local_str = candidates.first().map(|a| a.to_string());
        let pub_str = candidates
            .iter()
            .find(|a| !a.ip().is_loopback() && a.to_string() != local_str.clone().unwrap_or_default())
            .map(|a| a.to_string());

        let offer_link = create_offer_link_with_id(&nickname, &room_code, candidates.clone(), Some(1));

        // 3. Node identity as Host (id = 1)
        self.my_peer_id.store(1, Ordering::SeqCst);
        *self.my_nickname.write() = nickname.clone();
        self.is_host.store(true, Ordering::SeqCst);
        self.current_host_id.store(1, Ordering::SeqCst);
        *self.backup_host_id.write() = None;
        *self.current_room_code.write() = Some(clean_code.clone());

        *self.status.write() = ConnectionStatus::Connected {
            my_id: 1,
            is_host: true,
            host_name: nickname.clone(),
            peer_name: "Ожидание гостей".into(),
            peer_addr: pub_str.clone().or(local_str.clone()).unwrap_or_default(),
            room_name: room_code.clone(),
            room_code: Some(clean_code.clone()),
            ping_ms: 0,
            connected_count: 0,
        };

        // 4. Start local audio engine
        if let Err(e) = self.audio_engine.start() {
            self.logger.error(&format!("Не удалось запустить аудио движок хоста: {}", e));
        } else {
            self.logger.info("Аудио движок хоста успешно запущен");
            self.audio_engine.play_connect_chime();
        }

        // 5. Start continuous Nostr signaling session for multi-guest intake
        let host_signaling = MqttSignaling::start_host_session(&clean_code, &offer_link, &self.logger).await?;

        // 6. Spawn Star-SFU Host routing and connection loop
        let node_clone = self.clone();
        let code_for_task = clean_code.clone();

        let task = tokio::spawn(async move {
            run_sfu_host_loop(
                arc_socket,
                host_signaling,
                code_for_task,
                node_clone,
            ).await;
        });

        *self.session_abort.write() = Some(task.abort_handle());
        Ok(clean_code)
    }

    /// Mode: Client joins Star-SFU room with a 6-digit code
    pub async fn join_room(
        &self,
        room_code: String,
        nickname: String,
    ) -> Result<(), String> {
        self.disconnect();
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.shutdown_signal.store(false, Ordering::SeqCst);

        let clean_code = crate::signaling::sanitize_room_code(&room_code);
        self.logger.info(&format!(
            "Вход в Star-SFU комнату '{}' (Код: {}), клиент '{}'",
            room_code, clean_code, nickname
        ));

        *self.status.write() = ConnectionStatus::Connecting {
            target_str: format!("Комната {}", clean_code),
            attempt: 1,
            max_attempts: 1,
            mode: "Поиск хоста через брокер...".into(),
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
        let candidates = gather_candidates(arc_socket.clone(), actual_port, opt_physical_ip, &self.logger).await;

        let my_nonce = rand::random::<u64>();
        let client_initial_id = (rand::random::<u32>() % 9000 + 1000).max(2);
        self.my_peer_id.store(client_initial_id, Ordering::SeqCst);
        *self.my_nickname.write() = nickname.clone();
        self.is_host.store(false, Ordering::SeqCst);
        *self.current_room_code.write() = Some(clean_code.clone());

        let answer_link = create_answer_link_with_id(
            &nickname,
            candidates.clone(),
            Some(client_initial_id),
            Some(my_nonce),
        );

        let node_clone = self.clone();
        let code_for_task = clean_code.clone();

        let task = tokio::spawn(async move {
            run_sfu_client_connection(
                arc_socket,
                code_for_task,
                answer_link,
                candidates,
                client_initial_id,
                my_nonce,
                node_clone,
            ).await;
        });

        *self.session_abort.write() = Some(task.abort_handle());
        Ok(())
    }
}

/// Builds full list of peers in the room including the host itself
fn build_host_peer_list(node: &P2pNode) -> Vec<PeerInfo> {
    let mut list = Vec::new();
    let my_id = node.my_peer_id.load(Ordering::Relaxed);
    let my_name = node.my_nickname.read().clone();
    list.push(PeerInfo {
        id: my_id,
        name: my_name,
        addr: "".into(),
        ping_ms: 0,
        last_seen_ms: now_ms(),
        is_host: node.is_host.load(Ordering::Relaxed),
        is_speaking: false,
        mic_muted: false,
        deafened: false,
        video_active: false,
        screen_active: false,
    });
    for p in node.peers.read().values() {
        if p.id != my_id {
            list.push(p.clone());
        }
    }
    list
}

/// Star-SFU Host Loop: receives packets from all guests and forwards them across the room
async fn run_sfu_host_loop(
    socket: Arc<UdpSocket>,
    mut host_signaling: HostSignalingSession,
    room_code: String,
    node: P2pNode,
) {
    node.logger.info("============================================================");
    node.logger.info(&format!("STAR-SFU ХОСТ АКТИВЕН ДЛЯ КОМНАТЫ '{}'", room_code));
    node.logger.info("============================================================");

    let mut buf = [0u8; 4096];
    let mut video_reassembler = VideoReassembler::new();
    let mut last_ping_broadcast = Instant::now();
    let mut ping_seq = 0u64;

    // Background Audio Transmitter for Host
    let audio_sock = socket.clone();
    let node_audio = node.clone();
    let audio_tx_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut audio_seq = 0u64;

        while !node_audio.shutdown_signal.load(Ordering::Relaxed) {
            interval.tick().await;
            let my_id = node_audio.my_peer_id.load(Ordering::Relaxed);
            for audio_payload in node_audio.audio_engine.pop_outgoing_packets() {
                audio_seq += 1;
                let audio_pkt = Packet::AudioData {
                    sender_id: my_id,
                    seq: audio_seq,
                    data: audio_payload,
                };
                if let Ok(b) = audio_pkt.encode() {
                    let opt_haddr = *node_audio.host_socket_addr.read();
                    if node_audio.is_host.load(Ordering::Relaxed) {
                        let client_addrs: Vec<SocketAddr> = node_audio.client_endpoints.read().values().cloned().collect();
                        for caddr in client_addrs {
                            let _ = audio_sock.send_to(&b, caddr).await;
                        }
                    } else if let Some(haddr) = opt_haddr {
                        let _ = audio_sock.send_to(&b, haddr).await;
                    }
                }
            }
        }
    });

    let mut last_diag_log = Instant::now();

    while !node.shutdown_signal.load(Ordering::Relaxed) {
        // Periodic audio diagnostics logging every 10s
        if last_diag_log.elapsed() >= Duration::from_secs(10) {
            let (rx, tx, starves, jb_samples) = node.audio_engine.get_audio_diagnostics();
            node.logger.info(&format!(
                "[AUDIO ДИАГНОСТИКА] Пакетов получено: {}, отправлено: {}, голоданий: {}, сэмплов в буфере: {}",
                rx, tx, starves, jb_samples
            ));
            last_diag_log = Instant::now();
        }

        // Periodic ping and peer list broadcast every 2.5s
        if last_ping_broadcast.elapsed() >= Duration::from_millis(2500) {
            ping_seq += 1;
            let my_id = node.my_peer_id.load(Ordering::Relaxed);
            let ping_pkt = Packet::Ping {
                sender_id: my_id,
                seq: ping_seq,
                timestamp_ms: now_ms(),
            };
            node.broadcast_packet(&ping_pkt).await;

            // Compute backup host (guest with lowest peer id)
            let backup_id = {
                let p_guard = node.peers.read();
                p_guard.keys().filter(|&&id| id != 1).min().cloned()
            };
            *node.backup_host_id.write() = backup_id;

            let peers_list = build_host_peer_list(&node);
            let update_pkt = Packet::PeerListUpdate {
                host_id: my_id,
                backup_host_id: backup_id,
                peers: peers_list,
            };
            node.broadcast_packet(&update_pkt).await;

            // Update connected count in Hosting/Connected status
            let connected_count = node.peers.read().len();
            let mut st = node.status.write();
            if let ConnectionStatus::Connected { connected_count: cc, peer_name, .. } = &mut *st {
                *cc = connected_count;
                *peer_name = if connected_count == 0 {
                    "Ожидание гостей".into()
                } else {
                    format!("Участников: {}", connected_count + 1)
                };
            }

            last_ping_broadcast = Instant::now();
        }

        tokio::select! {
            // 1. Incoming Answers from Nostr Signaling for new guests
            Some(answer_str) = host_signaling.incoming_answers.recv() => {
                if let Ok(answer) = parse_answer_link(&answer_str) {
                    node.logger.info(&format!(
                        "SFU Хост: получен запрос на подключение от гостя '{}'!",
                        answer.name
                    ));
                    let _assigned_id = answer.guest_id.unwrap_or_else(|| node.next_peer_id.fetch_add(1, Ordering::SeqCst));
                    let targets = expand_candidates_with_port_spray(&answer.candidates);

                    let sock_clone = socket.clone();
                    let my_name = node.my_nickname.read().clone();
                    let node_punch = node.clone();
                    let guest_id_opt = answer.guest_id;
                    tokio::spawn(async move {
                        let punch_pkt = Packet::Punch {
                            nonce: answer.nonce.unwrap_or_else(rand::random),
                            sender_name: my_name,
                            sender_id: 1,
                        };
                        if let Ok(b) = punch_pkt.encode() {
                            let punch_start = Instant::now();
                            while punch_start.elapsed() < Duration::from_secs(7) {
                                if let Some(gid) = guest_id_opt {
                                    if node_punch.client_endpoints.read().contains_key(&gid) {
                                        break;
                                    }
                                }
                                for &t in &targets {
                                    let _ = sock_clone.send_to(&b, t).await;
                                }
                                tokio::time::sleep(Duration::from_millis(70)).await;
                            }
                        }
                    });
                }
            }

            // 2. Incoming UDP packets from connected guests
            recv_res = socket.recv_from(&mut buf) => {
                match recv_res {
                    Ok((len, sender_addr)) => {
                        if let Ok(pkt) = Packet::decode(&buf[..len]) {
                            match pkt {
                                Packet::Punch { nonce, sender_name, sender_id } => {
                                    node.logger.info(&format!(
                                        "SFU Хост: подключился гость '{}' (id={}, addr={})!",
                                        sender_name, sender_id, sender_addr
                                    ));

                                    // Send PunchAck
                                    let ack = Packet::PunchAck {
                                        nonce,
                                        sender_name: node.my_nickname.read().clone(),
                                        sender_id: 1,
                                    };
                                    if let Ok(ack_b) = ack.encode() {
                                        for _ in 0..3 {
                                            let _ = socket.send_to(&ack_b, sender_addr).await;
                                        }
                                    }

                                    // Register in client endpoints and peer table
                                    node.client_endpoints.write().insert(sender_id, sender_addr);
                                    node.peer_addrs.write().insert(sender_addr, sender_id);
                                    node.peers.write().insert(
                                        sender_id,
                                        PeerInfo {
                                            id: sender_id,
                                            name: sender_name.clone(),
                                            addr: sender_addr.to_string(),
                                            ping_ms: 1,
                                            last_seen_ms: now_ms(),
                                            is_host: false,
                                            is_speaking: false,
                                            mic_muted: false,
                                            deafened: false,
                                            video_active: false,
                                            screen_active: false,
                                        },
                                    );

                                    // Send ConnectAccept with current peer list
                                    let accept = Packet::ConnectAccept {
                                        assigned_id: sender_id,
                                        host_id: node.my_peer_id.load(Ordering::Relaxed),
                                        host_name: node.my_nickname.read().clone(),
                                        room_name: room_code.clone(),
                                        peers: build_host_peer_list(&node),
                                    };
                                    if let Ok(b) = accept.encode() {
                                        let _ = socket.send_to(&b, sender_addr).await;
                                    }

                                    node.audio_engine.play_connect_chime();
                                }
                                Packet::PunchAck { sender_name, sender_id, .. } => {
                                    node.client_endpoints.write().insert(sender_id, sender_addr);
                                    node.peer_addrs.write().insert(sender_addr, sender_id);
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.last_seen_ms = now_ms();
                                        p.name = sender_name;
                                    }
                                }
                                Packet::AudioData { sender_id, seq, data } => {
                                    // Host feeds locally into mixer
                                    node.audio_engine.feed_incoming_packet(sender_id, &data);
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.last_seen_ms = now_ms();
                                    }

                                    // SFU FORWARDING: Relay audio to all OTHER clients if we are host
                                    if node.is_host.load(Ordering::Relaxed) {
                                        let forward_pkt = Packet::AudioData { sender_id, seq, data };
                                        if let Ok(forward_bytes) = forward_pkt.encode() {
                                            let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                            for caddr in endpoints {
                                                if caddr != sender_addr {
                                                    let _ = socket.send_to(&forward_bytes, caddr).await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Packet::VideoChunk {
                                    sender_id,
                                    frame_id,
                                    chunk_idx,
                                    chunk_total,
                                    is_screen,
                                    data,
                                } => {
                                    // Host feeds into local video reassembler
                                    video_reassembler.handle_chunk(
                                        sender_id,
                                        frame_id,
                                        chunk_idx,
                                        chunk_total,
                                        is_screen,
                                        data.clone(),
                                        &node.latest_remote_video,
                                    );
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.last_seen_ms = now_ms();
                                    }

                                    // SFU FORWARDING: Relay video chunk to all other clients if we are host
                                    if node.is_host.load(Ordering::Relaxed) {
                                        let forward_pkt = Packet::VideoChunk {
                                            sender_id,
                                            frame_id,
                                            chunk_idx,
                                            chunk_total,
                                            is_screen,
                                            data,
                                        };
                                        if let Ok(forward_bytes) = forward_pkt.encode() {
                                            let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                            for caddr in endpoints {
                                                if caddr != sender_addr {
                                                    let _ = socket.send_to(&forward_bytes, caddr).await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Packet::TextMessage { id, sender_id, text, sender_name, timestamp_ms } => {
                                    let ack = Packet::TextAck { id };
                                    if let Ok(ack_b) = ack.encode() {
                                        let _ = socket.send_to(&ack_b, sender_addr).await;
                                    }

                                    let is_new = node.seen_msg_ids.write().insert(id);
                                    if is_new {
                                        node.logger.info(&format!("[ЧАТ SFU] {}: {}", sender_name, text));
                                        node.chat_messages.write().push(ChatMessage {
                                             id,
                                             sender: sender_name.clone(),
                                             text: text.clone(),
                                             timestamp_ms,
                                             is_me: false,
                                        });

                                        // SFU FORWARDING: Relay text message to all other clients
                                        let forward_pkt = Packet::TextMessage {
                                            id,
                                            sender_id,
                                            text,
                                            sender_name,
                                            timestamp_ms,
                                        };
                                        if let Ok(fb) = forward_pkt.encode() {
                                            let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                            for caddr in endpoints {
                                                if caddr != sender_addr {
                                                    let _ = socket.send_to(&fb, caddr).await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Packet::MediaState { sender_id, mic_muted, deafened, video_active, screen_active } => {
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.mic_muted = mic_muted;
                                        p.deafened = deafened;
                                        p.video_active = video_active;
                                        p.screen_active = screen_active;
                                        p.last_seen_ms = now_ms();
                                    }

                                    // SFU FORWARDING: Relay media state to all other clients
                                    let forward_pkt = Packet::MediaState { sender_id, mic_muted, deafened, video_active, screen_active };
                                    if let Ok(fb) = forward_pkt.encode() {
                                        let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                        for caddr in endpoints {
                                            if caddr != sender_addr {
                                                let _ = socket.send_to(&fb, caddr).await;
                                            }
                                        }
                                    }
                                }
                                Packet::Ping { sender_id, seq, timestamp_ms } => {
                                    let pong = Packet::Pong { sender_id: 1, seq, timestamp_ms };
                                    if let Ok(b) = pong.encode() {
                                        let _ = socket.send_to(&b, sender_addr).await;
                                    }
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.last_seen_ms = now_ms();
                                    }
                                }
                                Packet::Pong { sender_id, timestamp_ms, .. } => {
                                    let rtt = (now_ms().saturating_sub(timestamp_ms) as u32).max(1);
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.ping_ms = rtt;
                                        p.last_seen_ms = now_ms();
                                    }
                                }
                                Packet::Disconnect { sender_id, reason } => {
                                    node.logger.warn(&format!("Гость (id={}) отключился: {}", sender_id, reason));
                                    node.client_endpoints.write().remove(&sender_id);
                                    node.peer_addrs.write().remove(&sender_addr);
                                    node.peers.write().remove(&sender_id);
                                    node.audio_engine.play_disconnect_chime();

                                    let my_id = node.my_peer_id.load(Ordering::Relaxed);
                                    let update = Packet::PeerListUpdate {
                                        host_id: my_id,
                                        backup_host_id: *node.backup_host_id.read(),
                                        peers: build_host_peer_list(&node),
                                    };
                                    node.broadcast_packet(&update).await;
                                }
                                Packet::NicknameUpdate { sender_id, new_nickname } => {
                                    node.logger.info(&format!("[SFU] Участник (id={}) обновил имя на '{}'", sender_id, new_nickname));
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.name = new_nickname.clone();
                                    }
                                    if node.is_host.load(Ordering::Relaxed) {
                                        let fwd = Packet::NicknameUpdate { sender_id, new_nickname };
                                        if let Ok(fb) = fwd.encode() {
                                            let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                            for caddr in endpoints {
                                                if caddr != sender_addr {
                                                    let _ = socket.send_to(&fb, caddr).await;
                                                }
                                            }
                                        }
                                        let my_id = node.my_peer_id.load(Ordering::Relaxed);
                                        let update = Packet::PeerListUpdate {
                                            host_id: my_id,
                                            backup_host_id: *node.backup_host_id.read(),
                                            peers: build_host_peer_list(&node),
                                        };
                                        node.broadcast_packet(&update).await;
                                    }
                                }
                                Packet::HostHandover { new_host_id, .. } => {
                                    node.current_host_id.store(new_host_id, Ordering::SeqCst);
                                    let my_id = node.my_peer_id.load(Ordering::Relaxed);
                                    if new_host_id != my_id {
                                        node.is_host.store(false, Ordering::SeqCst);
                                        let mut peers_guard = node.peers.write();
                                        for p in peers_guard.values_mut() {
                                            p.is_host = p.id == new_host_id;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        if e.raw_os_error() != Some(10054) {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    }
                }
            }
        }
    }

    audio_tx_task.abort();
    node.audio_engine.stop();
}

/// Star-SFU Client Connection & Connected Session
async fn run_sfu_client_connection(
    socket: Arc<UdpSocket>,
    room_code: String,
    answer_link: String,
    _my_candidates: Vec<SocketAddr>,
    client_initial_id: u32,
    my_nonce: u64,
    node: P2pNode,
) {
    node.logger.info(&format!("Поиск комнаты '{}' на WSS-релеях...", room_code));

    let offer_res = MqttSignaling::client_exchange(&room_code, &answer_link, Duration::from_secs(45), &node.logger).await;

    if node.shutdown_signal.load(Ordering::Relaxed) {
        return;
    }

    let offer = match offer_res {
        Ok(str_offer) => match parse_offer_link(&str_offer) {
            Ok(off) => off,
            Err(e) => {
                node.logger.error(&format!("Ошибка разбора Offer: {}", e));
                *node.status.write() = ConnectionStatus::Failed { error: e };
                return;
            }
        },
        Err(e) => {
            node.logger.error(&format!("Не удалось найти хоста: {}", e));
            *node.status.write() = ConnectionStatus::Failed { error: e };
            return;
        }
    };

    node.logger.info(&format!("Offer от хоста '{}' получен! Начинаем пробитие NAT...", offer.name));
    *node.status.write() = ConnectionStatus::Connecting {
        target_str: offer.name.clone(),
        attempt: 1,
        max_attempts: 2,
        mode: "Пробитие UDP к SFU Хосту...".into(),
    };

    let targets = expand_candidates_with_port_spray(&offer.candidates);
    let punch_pkt = Packet::Punch {
        nonce: my_nonce,
        sender_name: node.my_nickname.read().clone(),
        sender_id: client_initial_id,
    };
    let punch_bytes = match punch_pkt.encode() {
        Ok(b) => b,
        Err(e) => {
            node.logger.error(&format!("Ошибка кодирования Punch: {}", e));
            return;
        }
    };

    let mut buf = [0u8; 4096];
    let punch_start = Instant::now();
    let punch_timeout = Duration::from_secs(8);
    let mut last_blast = Instant::now() - Duration::from_secs(1);
    let mut connected_host_addr: Option<SocketAddr> = None;

    while punch_start.elapsed() < punch_timeout && !node.shutdown_signal.load(Ordering::Relaxed) {
        if last_blast.elapsed() >= Duration::from_millis(70) {
            for &t in &targets {
                let _ = socket.send_to(&punch_bytes, t).await;
            }
            last_blast = Instant::now();
        }

        tokio::select! {
            recv_res = socket.recv_from(&mut buf) => {
                match recv_res {
                    Ok((len, sender_addr)) => {
                        if let Ok(pkt) = Packet::decode(&buf[..len]) {
                            match pkt {
                                Packet::PunchAck { .. } | Packet::ConnectAccept { .. } => {
                                    node.logger.info(&format!("УСПЕХ: канал к SFU Хосту ({}) открыт!", sender_addr));
                                    connected_host_addr = Some(sender_addr);
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        if e.raw_os_error() != Some(10054) {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(15)) => {}
        }
    }

    if node.shutdown_signal.load(Ordering::Relaxed) {
        return;
    }

    let host_addr = match connected_host_addr {
        Some(a) => a,
        None => {
            node.logger.warn("Прямое UDP пробитие к хосту не удалось. Переключаемся на резервный WSS релей...");
            *node.status.write() = ConnectionStatus::Connecting {
                target_str: offer.name.clone(),
                attempt: 2,
                max_attempts: 2,
                mode: "Подключение через WSS релей...".into(),
            };

            let my_name = node.my_nickname.read().clone();
            match MqttSignaling::start_relay_session(&room_code, &my_name, &node.logger).await {
                Ok(relay_session) => {
                    node.logger.info("Резервный релейный канал активирован!");
                    run_relay_connected_loop(relay_session, room_code, node).await;
                    return;
                }
                Err(e) => {
                    let err = format!("Не удалось подключиться к комнате: {}", e);
                    node.logger.error(&err);
                    *node.status.write() = ConnectionStatus::Failed { error: err };
                    return;
                }
            }
        }
    };

    // Client connected to Host over UDP
    *node.host_socket_addr.write() = Some(host_addr);
    *node.status.write() = ConnectionStatus::Connected {
        my_id: client_initial_id,
        is_host: false,
        host_name: offer.name.clone(),
        peer_name: offer.name.clone(),
        peer_addr: host_addr.to_string(),
        room_name: offer.room.clone(),
        room_code: Some(room_code.clone()),
        ping_ms: 1,
        connected_count: 1,
    };

    if let Err(e) = node.audio_engine.start() {
        node.logger.error(&format!("Не удалось запустить аудио движок: {}", e));
    } else {
        node.audio_engine.play_connect_chime();
    }

    // Launch Audio Transmitter
    let audio_sock = socket.clone();
    let node_audio = node.clone();
    let audio_tx_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut audio_seq = 0u64;

        while !node_audio.shutdown_signal.load(Ordering::Relaxed) {
            interval.tick().await;
            let my_id = node_audio.my_peer_id.load(Ordering::Relaxed);
            let is_host = node_audio.is_host.load(Ordering::Relaxed);
            for audio_payload in node_audio.audio_engine.pop_outgoing_packets() {
                audio_seq += 1;
                let audio_pkt = Packet::AudioData {
                    sender_id: my_id,
                    seq: audio_seq,
                    data: audio_payload,
                };
                if let Ok(b) = audio_pkt.encode() {
                    let opt_haddr = *node_audio.host_socket_addr.read();
                    if is_host {
                        let endpoints: Vec<SocketAddr> = node_audio.client_endpoints.read().values().cloned().collect();
                        for caddr in endpoints {
                            let _ = audio_sock.send_to(&b, caddr).await;
                        }
                    } else if let Some(haddr) = opt_haddr {
                        let _ = audio_sock.send_to(&b, haddr).await;
                    } else {
                        let _ = audio_sock.send_to(&b, host_addr).await;
                    }
                }
            }
        }
    });

    let mut last_ping = Instant::now();
    let mut ping_seq = 0u64;
    let mut video_reassembler = VideoReassembler::new();
    let mut last_host_packet = Instant::now();
    let mut last_diag_log = Instant::now();

    while !node.shutdown_signal.load(Ordering::Relaxed) {
        // Periodic audio diagnostics logging every 10s
        if last_diag_log.elapsed() >= Duration::from_secs(10) {
            let (rx, tx, starves, jb_samples) = node.audio_engine.get_audio_diagnostics();
            node.logger.info(&format!(
                "[AUDIO ДИАГНОСТИКА] Пакетов получено: {}, отправлено: {}, голоданий: {}, сэмплов в буфере: {}",
                rx, tx, starves, jb_samples
            ));
            last_diag_log = Instant::now();
        }

        let is_promoted_host = node.is_host.load(Ordering::Relaxed);

        if last_ping.elapsed() >= Duration::from_millis(1500) {
            ping_seq += 1;
            let my_id = node.my_peer_id.load(Ordering::Relaxed);
            let ping_pkt = Packet::Ping {
                sender_id: my_id,
                seq: ping_seq,
                timestamp_ms: now_ms(),
            };

            if is_promoted_host {
                // If we became host, broadcast ping and peer list to all connected clients
                node.broadcast_packet(&ping_pkt).await;

                let update_pkt = Packet::PeerListUpdate {
                    host_id: my_id,
                    backup_host_id: *node.backup_host_id.read(),
                    peers: build_host_peer_list(&node),
                };
                node.broadcast_packet(&update_pkt).await;
            } else {
                if let Ok(b) = ping_pkt.encode() {
                    let target = node.host_socket_addr.read().unwrap_or(host_addr);
                    let _ = socket.send_to(&b, target).await;
                }
            }
            last_ping = Instant::now();

            // Failover check: if no packet from host for > 3.5s and we are not host yet
            if !is_promoted_host && last_host_packet.elapsed() > Duration::from_millis(3500) {
                let backup_id = *node.backup_host_id.read();
                let my_id = node.my_peer_id.load(Ordering::Relaxed);
                let should_promote = match backup_id {
                    Some(b_id) => b_id == my_id,
                    None => true,
                };

                if should_promote {
                    node.logger.warn("Хост перестал отвечать! Принимаем роль SFU Хоста без разрыва связи...");
                    node.is_host.store(true, Ordering::SeqCst);
                    node.current_host_id.store(my_id, Ordering::SeqCst);
                    *node.host_socket_addr.write() = None;
                    let my_name = node.my_nickname.read().clone();
                    {
                        let mut st = node.status.write();
                        if let ConnectionStatus::Connected { is_host, host_name, .. } = &mut *st {
                            *is_host = true;
                            *host_name = my_name;
                        }
                    }
                    let claim_pkt = Packet::ClaimHost {
                        candidate_id: my_id,
                        reason: "Предыдущий хост отключился".into(),
                    };
                    node.broadcast_packet(&claim_pkt).await;
                }
            }
        }

        tokio::select! {
            recv_res = socket.recv_from(&mut buf) => {
                match recv_res {
                    Ok((len, sender_addr)) => {
                        last_host_packet = Instant::now();
                        if let Ok(pkt) = Packet::decode(&buf[..len]) {
                            match pkt {
                                Packet::ConnectAccept { assigned_id, host_id, peers, .. } => {
                                    node.my_peer_id.store(assigned_id, Ordering::SeqCst);
                                    node.current_host_id.store(host_id, Ordering::SeqCst);
                                    let mut p_guard = node.peers.write();
                                    p_guard.clear();
                                    for p in peers {
                                        if p.id != assigned_id {
                                            p_guard.insert(p.id, p);
                                        }
                                    }
                                }
                                Packet::PeerListUpdate { host_id, backup_host_id, peers } => {
                                    node.current_host_id.store(host_id, Ordering::SeqCst);
                                    *node.backup_host_id.write() = backup_host_id;
                                    let my_id = node.my_peer_id.load(Ordering::Relaxed);
                                    let mut p_guard = node.peers.write();
                                    p_guard.clear();
                                    for p in peers {
                                        if p.id != my_id {
                                            p_guard.insert(p.id, p);
                                        }
                                    }
                                }
                                Packet::AudioData { sender_id, seq, data } => {
                                    node.audio_engine.feed_incoming_packet(sender_id, &data);
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.last_seen_ms = now_ms();
                                    }

                                    // If this client was promoted to SFU Host, relay audio to all other clients
                                    if node.is_host.load(Ordering::Relaxed) {
                                        let forward_pkt = Packet::AudioData { sender_id, seq, data };
                                        if let Ok(forward_bytes) = forward_pkt.encode() {
                                            let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                            for caddr in endpoints {
                                                if caddr != sender_addr {
                                                    let _ = socket.send_to(&forward_bytes, caddr).await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Packet::VideoChunk { sender_id, frame_id, chunk_idx, chunk_total, is_screen, data } => {
                                    video_reassembler.handle_chunk(
                                        sender_id,
                                        frame_id,
                                        chunk_idx,
                                        chunk_total,
                                        is_screen,
                                        data.clone(),
                                        &node.latest_remote_video,
                                    );
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.last_seen_ms = now_ms();
                                    }

                                    // If promoted to host, forward video chunk to other clients
                                    if node.is_host.load(Ordering::Relaxed) {
                                        let forward_pkt = Packet::VideoChunk {
                                            sender_id,
                                            frame_id,
                                            chunk_idx,
                                            chunk_total,
                                            is_screen,
                                            data,
                                        };
                                        if let Ok(forward_bytes) = forward_pkt.encode() {
                                            let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                            for caddr in endpoints {
                                                if caddr != sender_addr {
                                                    let _ = socket.send_to(&forward_bytes, caddr).await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Packet::TextMessage { id, sender_id, text, sender_name, timestamp_ms } => {
                                    let is_new = node.seen_msg_ids.write().insert(id);
                                    if is_new {
                                        node.logger.info(&format!("[ЧАТ] {}: {}", sender_name, text));
                                        node.chat_messages.write().push(ChatMessage {
                                            id,
                                            sender: sender_name.clone(),
                                            text: text.clone(),
                                            timestamp_ms,
                                            is_me: false,
                                        });

                                        // If promoted to host, forward text message to other clients
                                        if node.is_host.load(Ordering::Relaxed) {
                                            let forward_pkt = Packet::TextMessage {
                                                id,
                                                sender_id,
                                                text,
                                                sender_name,
                                                timestamp_ms,
                                            };
                                            if let Ok(fb) = forward_pkt.encode() {
                                                let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                                for caddr in endpoints {
                                                    if caddr != sender_addr {
                                                        let _ = socket.send_to(&fb, caddr).await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                Packet::MediaState { sender_id, mic_muted, deafened, video_active, screen_active } => {
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.mic_muted = mic_muted;
                                        p.deafened = deafened;
                                        p.video_active = video_active;
                                        p.screen_active = screen_active;
                                    }

                                    // If promoted to host, forward media state to other clients
                                    if node.is_host.load(Ordering::Relaxed) {
                                        let forward_pkt = Packet::MediaState { sender_id, mic_muted, deafened, video_active, screen_active };
                                        if let Ok(fb) = forward_pkt.encode() {
                                            let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                            for caddr in endpoints {
                                                if caddr != sender_addr {
                                                    let _ = socket.send_to(&fb, caddr).await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Packet::HostHandover { old_host_id, new_host_id } => {
                                    node.current_host_id.store(new_host_id, Ordering::SeqCst);
                                    let my_id = node.my_peer_id.load(Ordering::Relaxed);
                                    if new_host_id == my_id {
                                        node.logger.warn("👑 Вам передана роль SFU Хоста! Переключение режима без разрыва соединения...");
                                        node.is_host.store(true, Ordering::SeqCst);
                                        *node.host_socket_addr.write() = None;
                                        node.client_endpoints.write().insert(old_host_id, sender_addr);
                                        node.peer_addrs.write().insert(sender_addr, old_host_id);
                                        if let Some(p) = node.peers.write().get_mut(&old_host_id) {
                                            p.is_host = false;
                                        }
                                        let my_name = node.my_nickname.read().clone();
                                        {
                                            let mut st = node.status.write();
                                            if let ConnectionStatus::Connected { is_host, host_name, .. } = &mut *st {
                                                *is_host = true;
                                                *host_name = my_name;
                                            }
                                        }
                                        let update = Packet::PeerListUpdate {
                                            host_id: my_id,
                                            backup_host_id: *node.backup_host_id.read(),
                                            peers: build_host_peer_list(&node),
                                        };
                                        node.broadcast_packet(&update).await;
                                    } else {
                                        if let Some(p) = node.peers.write().get_mut(&new_host_id) {
                                            p.is_host = true;
                                            if let Ok(parsed) = p.addr.parse::<SocketAddr>() {
                                                *node.host_socket_addr.write() = Some(parsed);
                                            }
                                        }
                                        if let Some(p) = node.peers.write().get_mut(&old_host_id) {
                                            p.is_host = false;
                                        }
                                    }
                                }
                                Packet::NicknameUpdate { sender_id, new_nickname } => {
                                    node.logger.info(&format!("Участник (id={}) обновил имя на '{}'", sender_id, new_nickname));
                                    if sender_id == node.current_host_id.load(Ordering::Relaxed) {
                                        let mut st = node.status.write();
                                        if let ConnectionStatus::Connected { host_name, .. } = &mut *st {
                                            *host_name = new_nickname.clone();
                                        }
                                    }
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.name = new_nickname.clone();
                                    }
                                    if node.is_host.load(Ordering::Relaxed) {
                                        let fwd = Packet::NicknameUpdate { sender_id, new_nickname };
                                        if let Ok(fb) = fwd.encode() {
                                            let endpoints: Vec<SocketAddr> = node.client_endpoints.read().values().cloned().collect();
                                            for caddr in endpoints {
                                                if caddr != sender_addr {
                                                    let _ = socket.send_to(&fb, caddr).await;
                                                }
                                            }
                                        }
                                    }
                                }
                                Packet::Ping { sender_id, seq, timestamp_ms } => {
                                    let my_id = node.my_peer_id.load(Ordering::Relaxed);
                                    let pong = Packet::Pong { sender_id: my_id, seq, timestamp_ms };
                                    if let Ok(b) = pong.encode() {
                                        let _ = socket.send_to(&b, sender_addr).await;
                                    }
                                    if let Some(p) = node.peers.write().get_mut(&sender_id) {
                                        p.last_seen_ms = now_ms();
                                    }
                                }
                                Packet::Pong { timestamp_ms, .. } => {
                                    let rtt = (now_ms().saturating_sub(timestamp_ms) as u32).max(1);
                                    let mut st = node.status.write();
                                    if let ConnectionStatus::Connected { ping_ms, .. } = &mut *st {
                                        *ping_ms = rtt;
                                    }
                                }
                                Packet::Disconnect { sender_id, reason } => {
                                    if sender_id == node.current_host_id.load(Ordering::Relaxed) {
                                        node.logger.warn(&format!("Хост вышел: {}. Проверка смены хоста...", reason));
                                        let backup_id = *node.backup_host_id.read();
                                        let my_id = node.my_peer_id.load(Ordering::Relaxed);
                                        if backup_id == Some(my_id) {
                                            node.logger.warn("👑 Принимаем роль SFU Хоста вместо отключившегося хоста!");
                                            node.is_host.store(true, Ordering::SeqCst);
                                            node.current_host_id.store(my_id, Ordering::SeqCst);
                                            *node.host_socket_addr.write() = None;
                                            let my_name = node.my_nickname.read().clone();
                                            let mut st = node.status.write();
                                            if let ConnectionStatus::Connected { is_host, host_name, .. } = &mut *st {
                                                *is_host = true;
                                                *host_name = my_name;
                                            }
                                        }
                                    } else {
                                        node.peers.write().remove(&sender_id);
                                        node.client_endpoints.write().remove(&sender_id);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        if e.raw_os_error() != Some(10054) {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    }
                }
            }
        }
    }

    audio_tx_task.abort();
    node.audio_engine.stop();
}

/// Fallback WSS Relay loop for rooms behind strict CGNAT
async fn run_relay_connected_loop(
    mut relay_session: crate::signaling::RelaySession,
    room_code: String,
    node: P2pNode,
) {
    let my_id = node.my_peer_id.load(Ordering::Relaxed);
    let _my_name = node.my_nickname.read().clone();
    *node.relay_sender.write() = Some(relay_session.outgoing_tx.clone());

    *node.status.write() = ConnectionStatus::Connected {
        my_id,
        is_host: false,
        host_name: "WSS Релей".into(),
        peer_name: "WSS Релей".into(),
        peer_addr: "Релей TLS/443".into(),
        room_name: room_code.clone(),
        room_code: Some(room_code),
        ping_ms: 35,
        connected_count: 1,
    };

    if let Err(e) = node.audio_engine.start() {
        node.logger.error(&format!("(Релей) Не удалось запустить аудио: {}", e));
    } else {
        node.audio_engine.play_connect_chime();
    }

    let relay_tx = relay_session.outgoing_tx.clone();
    let node_audio = node.clone();
    let audio_tx_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        let mut audio_seq = 0u64;
        while !node_audio.shutdown_signal.load(Ordering::Relaxed) {
            interval.tick().await;
            let mid = node_audio.my_peer_id.load(Ordering::Relaxed);
            for p in node_audio.audio_engine.pop_outgoing_packets() {
                audio_seq += 1;
                let _ = relay_tx.send(Packet::AudioData { sender_id: mid, seq: audio_seq, data: p }).await;
            }
        }
    });

    let mut video_reassembler = VideoReassembler::new();

    while !node.shutdown_signal.load(Ordering::Relaxed) {
        tokio::select! {
            Some(pkt) = relay_session.incoming_rx.recv() => {
                match pkt {
                    Packet::AudioData { sender_id, data, .. } => {
                        node.audio_engine.feed_incoming_packet(sender_id, &data);
                    }
                    Packet::VideoChunk { sender_id, frame_id, chunk_idx, chunk_total, is_screen, data } => {
                        video_reassembler.handle_chunk(sender_id, frame_id, chunk_idx, chunk_total, is_screen, data, &node.latest_remote_video);
                    }
                    Packet::TextMessage { id, text, sender_name, timestamp_ms, .. } => {
                        let is_new = node.seen_msg_ids.write().insert(id);
                        if is_new {
                            node.chat_messages.write().push(ChatMessage {
                                id,
                                sender: sender_name,
                                text,
                                timestamp_ms,
                                is_me: false,
                            });
                        }
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(15)) => {}
        }
    }

    audio_tx_task.abort();
    node.audio_engine.stop();
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
        target: &Arc<RwLock<Option<RemoteVideoFrame>>>,
    ) {
        use base64::Engine;
        if frame_id > self.current_frame_id || sender_id != self.sender_id || self.created_at.elapsed() > Duration::from_millis(800) {
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
                *target.write() = Some(RemoteVideoFrame {
                    sender_id,
                    frame_id,
                    is_screen: self.is_screen,
                    jpeg_base64: base64_str,
                    timestamp_ms: now_ms(),
                });
                self.chunks.clear();
            }
        }
    }
}

async fn gather_candidates(
    socket: Arc<UdpSocket>,
    port: u16,
    opt_physical_ip: Option<Ipv4Addr>,
    logger: &AppLogger,
) -> Vec<SocketAddr> {
    let mut list = Vec::new();

    let lan_ip = opt_physical_ip.map(IpAddr::V4).unwrap_or_else(detect_local_lan_ip);
    let local_addr = SocketAddr::new(lan_ip, port);
    list.push(local_addr);
    logger.info(&format!("Локальный адрес кандидата: {}", local_addr));

    if let Some(pub_addr) = discover_public_ip_via_stun(socket, logger).await {
        logger.info(&format!("STUN публичный адрес кандидата: {}", pub_addr));
        if !list.contains(&pub_addr) {
            list.push(pub_addr);
        }
    } else {
        logger.warn("STUN: не удалось определить публичный адрес (будет использован LAN адрес)");
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
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
                    return addr.ip();
                }
            }
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
            let if_index_be: u32 = if_idx.to_be();
            let res = setsockopt(
                raw_sock,
                0,
                31,
                &if_index_be as *const _ as *const u8,
                4,
            );
            if res == 0 {
                logger.info(&format!(
                    "UDP сокет привязан к физическому интерфейсу #{} (LAN IP: {:?})",
                    if_idx,
                    opt_ip.unwrap_or(Ipv4Addr::UNSPECIFIED)
                ));
            }
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

#[cfg(not(windows))]
pub fn find_best_physical_interface() -> Option<(u32, Option<Ipv4Addr>)> {
    None
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
        };
        let b = pkt.encode().unwrap();

        sock_a.send_to(&b, addr_b).await.unwrap();
        sock_a.send_to(&b, addr_b).await.unwrap();
        sock_a.send_to(&b, addr_b).await.unwrap();

        let mut buf = [0u8; 1024];
        for _ in 0..3 {
            let (len, _) = sock_b.recv_from(&mut buf).await.unwrap();
            let decoded = Packet::decode(&buf[..len]).unwrap();
            if let Packet::TextMessage { id, text, sender_name, timestamp_ms, .. } = decoded {
                let is_new = node_b.seen_msg_ids.write().insert(id);
                if is_new {
                    node_b.chat_messages.write().push(ChatMessage {
                        id,
                        sender: sender_name,
                        text,
                        timestamp_ms,
                        is_me: false,
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
}
