use crate::invite::{
    create_answer_link, create_offer_link, parse_answer_link, parse_offer_link,
};
use crate::logger::AppLogger;
use crate::signaling::MqttSignaling;
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
pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Packet {
    Punch {
        nonce: u64,
        sender_name: String,
    },
    PunchAck {
        nonce: u64,
        sender_name: String,
    },
    ConnectRequest {
        client_name: String,
        protocol_version: u32,
        client_nonce: u64,
    },
    ConnectAccept {
        assigned_id: u32,
        host_name: String,
        room_name: String,
    },
    ConnectReject {
        reason: String,
    },
    Ping {
        seq: u64,
        timestamp_ms: u64,
    },
    Pong {
        seq: u64,
        timestamp_ms: u64,
    },
    TextMessage {
        id: u64,
        text: String,
        sender_name: String,
        timestamp_ms: u64,
    },
    TextAck {
        id: u64,
    },
    Disconnect {
        reason: String,
    },
    AudioData {
        seq: u64,
        data: Vec<u8>,
    },
    VideoChunk {
        frame_id: u32,
        chunk_idx: u16,
        chunk_total: u16,
        is_screen: bool,
        data: Vec<u8>,
    },
    MediaState {
        mic_muted: bool,
        deafened: bool,
        video_active: bool,
        screen_active: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: u32,
    pub name: String,
    pub addr: String,
    pub ping_ms: u32,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteVideoFrame {
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
        peer_name: String,
        peer_addr: String,
        room_name: String,
        ping_ms: u32,
    },
    Failed {
        error: String,
    },
}

pub struct P2pNode {
    logger: AppLogger,
    status: Arc<RwLock<ConnectionStatus>>,
    peers: Arc<RwLock<HashMap<SocketAddr, PeerInfo>>>,
    active_socket: Arc<RwLock<Option<Arc<UdpSocket>>>>,
    active_peer_addr: Arc<RwLock<Option<SocketAddr>>>,
    relay_sender: Arc<RwLock<Option<tokio::sync::mpsc::Sender<Packet>>>>,
    chat_messages: Arc<RwLock<Vec<ChatMessage>>>,
    seen_msg_ids: Arc<RwLock<HashSet<u64>>>,
    pending_acks: Arc<RwLock<HashMap<u64, oneshot::Sender<()>>>>,
    session_abort: Arc<RwLock<Option<tokio::task::AbortHandle>>>,
    shutdown_signal: Arc<AtomicBool>,
    next_peer_id: Arc<AtomicU32>,
    audio_engine: Arc<crate::audio::AudioEngine>,
    latest_remote_video: Arc<RwLock<Option<RemoteVideoFrame>>>,
    remote_media_state: Arc<RwLock<RemoteMediaState>>,
    video_frame_seq: Arc<AtomicU32>,
}

impl P2pNode {
    pub fn new(logger: AppLogger) -> Self {
        Self {
            logger,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            peers: Arc::new(RwLock::new(HashMap::new())),
            active_socket: Arc::new(RwLock::new(None)),
            active_peer_addr: Arc::new(RwLock::new(None)),
            relay_sender: Arc::new(RwLock::new(None)),
            chat_messages: Arc::new(RwLock::new(Vec::new())),
            seen_msg_ids: Arc::new(RwLock::new(HashSet::new())),
            pending_acks: Arc::new(RwLock::new(HashMap::new())),
            session_abort: Arc::new(RwLock::new(None)),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            next_peer_id: Arc::new(AtomicU32::new(1)),
            audio_engine: crate::audio::AudioEngine::new(),
            latest_remote_video: Arc::new(RwLock::new(None)),
            remote_media_state: Arc::new(RwLock::new(RemoteMediaState::default())),
            video_frame_seq: Arc::new(AtomicU32::new(1)),
        }
    }

    pub fn get_status(&self) -> ConnectionStatus {
        self.status.read().clone()
    }

    pub fn get_peers(&self) -> Vec<PeerInfo> {
        self.peers.read().values().cloned().collect()
    }

    pub fn get_chat_messages(&self) -> Vec<ChatMessage> {
        self.chat_messages.read().clone()
    }

    pub fn disconnect(&self) {
        if let Some(handle) = self.session_abort.write().take() {
            handle.abort();
        }
        self.shutdown_signal.store(true, Ordering::SeqCst);

        // Send disconnect packet to connected peers
        if let Some(sock) = self.active_socket.read().as_ref() {
            let disc = Packet::Disconnect {
                reason: "Пользователь вышел из комнаты".into(),
            };
            if let Ok(bytes) = disc.encode() {
                let addrs: Vec<SocketAddr> = self.peers.read().keys().cloned().collect();
                for addr in addrs {
                    let s = sock.clone();
                    let b = bytes.clone();
                    tokio::spawn(async move {
                        let _ = s.send_to(&b, addr).await;
                    });
                }
            }
        }
        if let Some(relay_tx) = self.relay_sender.read().clone() {
            let disc = Packet::Disconnect {
                reason: "Пользователь вышел из комнаты".into(),
            };
            tokio::spawn(async move {
                let _ = relay_tx.send(disc).await;
            });
        }

        self.logger.info("Отключение P2P соединения...");
        self.audio_engine.stop();
        *self.latest_remote_video.write() = None;
        *self.remote_media_state.write() = RemoteMediaState::default();
        *self.active_socket.write() = None;
        *self.active_peer_addr.write() = None;
        *self.relay_sender.write() = None;
        self.peers.write().clear();
        self.seen_msg_ids.write().clear();
        self.pending_acks.write().clear();
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

        let sock_opt = self.active_socket.read().clone();
        let peer_addr_opt = *self.active_peer_addr.read();
        let relay_tx_opt = self.relay_sender.read().clone();

        if peer_addr_opt.is_none() && relay_tx_opt.is_none() {
            return Ok(());
        }

        let frame_id = self.video_frame_seq.fetch_add(1, Ordering::SeqCst);
        const CHUNK_SIZE: usize = 1200;
        let chunk_total = ((bytes.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u16;

        for (idx, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
            let pkt = Packet::VideoChunk {
                frame_id,
                chunk_idx: idx as u16,
                chunk_total,
                is_screen,
                data: chunk.to_vec(),
            };

            if let (Some(sock), Some(peer_addr)) = (&sock_opt, peer_addr_opt) {
                if let Ok(b) = pkt.encode() {
                    let _ = sock.send_to(&b, peer_addr).await;
                }
            } else if let Some(relay_tx) = &relay_tx_opt {
                let _ = relay_tx.send(pkt).await;
            }

            // Pacing between UDP chunks to avoid buffer bloat and packet drops
            tokio::time::sleep(Duration::from_micros(200)).await;
        }

        Ok(())
    }

    pub async fn send_media_state(&self, video_active: bool, screen_active: bool) -> Result<(), String> {
        let pkt = Packet::MediaState {
            mic_muted: self.audio_engine.is_mic_muted(),
            deafened: self.audio_engine.is_deafened(),
            video_active,
            screen_active,
        };

        let sock_opt = self.active_socket.read().clone();
        let peer_addr_opt = *self.active_peer_addr.read();
        let relay_tx_opt = self.relay_sender.read().clone();

        if let (Some(sock), Some(peer_addr)) = (sock_opt, peer_addr_opt) {
            if let Ok(b) = pkt.encode() {
                let _ = sock.send_to(&b, peer_addr).await;
            }
        } else if let Some(relay_tx) = relay_tx_opt {
            let _ = relay_tx.send(pkt).await;
        }

        Ok(())
    }

    fn broadcast_media_state(&self) -> Result<(), String> {
        let pkt = Packet::MediaState {
            mic_muted: self.audio_engine.is_mic_muted(),
            deafened: self.audio_engine.is_deafened(),
            video_active: false,
            screen_active: false,
        };
        let sock_opt = self.active_socket.read().clone();
        let peer_addr_opt = *self.active_peer_addr.read();
        let relay_tx_opt = self.relay_sender.read().clone();

        tokio::spawn(async move {
            if let (Some(sock), Some(peer_addr)) = (sock_opt, peer_addr_opt) {
                if let Ok(b) = pkt.encode() {
                    let _ = sock.send_to(&b, peer_addr).await;
                }
            } else if let Some(relay_tx) = relay_tx_opt {
                let _ = relay_tx.send(pkt).await;
            }
        });

        Ok(())
    }

    /// Send a text message to the connected peer over P2P/Relay with guaranteed delivery (ACK retry loop)
    pub async fn send_chat_message(&self, text: String, my_name: String) -> Result<ChatMessage, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("Сообщение не может быть пустым".into());
        }

        let sock_opt = self.active_socket.read().clone();
        let peer_addr_opt = *self.active_peer_addr.read();
        let relay_tx_opt = self.relay_sender.read().clone();

        if peer_addr_opt.is_none() && relay_tx_opt.is_none() {
            return Err("Нет подключенного собеседника".to_string());
        }

        // Generate globally unique message ID using random 63-bit integer to prevent any peer collisions
        let id = (rand::random::<u64>() & 0x7FFF_FFFF_FFFF_FFFF).max(1);
        let timestamp_ms = now_ms();

        // Mark as seen so sender never treats own message as duplicate if bounced
        self.seen_msg_ids.write().insert(id);

        let packet = Packet::TextMessage {
            id,
            text: trimmed.to_string(),
            sender_name: my_name.clone(),
            timestamp_ms,
        };

        if let (Some(sock), Some(peer_addr)) = (sock_opt, peer_addr_opt) {
            let encoded = packet
                .encode()
                .map_err(|e| format!("Ошибка кодирования сообщения: {}", e))?;

            // Register ACK oneshot
            let (ack_tx, mut ack_rx) = oneshot::channel();
            self.pending_acks.write().insert(id, ack_tx);

            // Spawn retry loop in background: sends up to 6 times (every 200ms) until ACK is received
            let sock_clone = sock.clone();
            let enc_clone = encoded.clone();
            let pending_clone = self.pending_acks.clone();
            tokio::spawn(async move {
                for _ in 0..6 {
                    let _ = sock_clone.send_to(&enc_clone, peer_addr).await;
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {},
                        _ = &mut ack_rx => {
                            return;
                        }
                    }
                }
                pending_clone.write().remove(&id);
            });
        } else if let Some(relay_tx) = relay_tx_opt {
            let (ack_tx, mut ack_rx) = oneshot::channel();
            self.pending_acks.write().insert(id, ack_tx);

            let pending_clone = self.pending_acks.clone();
            let pkt_clone = packet.clone();
            tokio::spawn(async move {
                for _ in 0..4 {
                    let _ = relay_tx.send(pkt_clone.clone()).await;
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(400)) => {},
                        _ = &mut ack_rx => {
                            return;
                        }
                    }
                }
                pending_clone.write().remove(&id);
            });
        }

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

    /// Mode: Host starts room with a 6-digit code
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
            "Создание комнаты '{}' (Код: {}), никнейм '{}'",
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
        let pub_str = candidates.iter().find(|a| !a.ip().is_loopback() && a.to_string() != local_str.clone().unwrap_or_default()).map(|a| a.to_string());

        let offer_link = create_offer_link(&nickname, &room_code, candidates.clone());

        *self.status.write() = ConnectionStatus::Hosting {
            room_name: room_code.clone(),
            room_code: Some(clean_code.clone()),
            local_addr: local_str,
            public_addr: pub_str,
            connected_count: 0,
        };

        // 3. Spawn background task to wait for Answer on signaling broker
        let sock_clone = arc_socket.clone();
        let logger_clone = self.logger.clone();
        let status_clone = self.status.clone();
        let peers_clone = self.peers.clone();
        let active_peer_clone = self.active_peer_addr.clone();
        let relay_sender_clone = self.relay_sender.clone();
        let chat_clone = self.chat_messages.clone();
        let seen_msg_clone = self.seen_msg_ids.clone();
        let pending_clone = self.pending_acks.clone();
        let shutdown_clone = self.shutdown_signal.clone();
        let host_name = nickname.clone();
        let room_name = room_code.clone();
        let next_id = self.next_peer_id.clone();
        let my_candidates = candidates.clone();
        let code_for_task = clean_code.clone();
        let audio_engine_clone = self.audio_engine.clone();
        let latest_video_clone = self.latest_remote_video.clone();
        let remote_media_clone = self.remote_media_state.clone();

        let task = tokio::spawn(async move {
            logger_clone.info(&format!("Ожидание подключения гостя в комнате {} через брокер...", code_for_task));
            
            let answer_res = MqttSignaling::host_exchange(&code_for_task, &offer_link, Duration::from_secs(90), &logger_clone).await;
            
            if shutdown_clone.load(Ordering::Relaxed) {
                return;
            }

            match answer_res {
                Ok(answer_str) => {
                    logger_clone.info("Получен Answer гостя через брокер. Начинаем пробитие NAT...");
                    match parse_answer_link(&answer_str) {
                        Ok(answer) => {
                            run_simultaneous_punching_flow(
                                sock_clone,
                                host_name,
                                room_name,
                                code_for_task,
                                answer.name,
                                answer.candidates,
                                my_candidates,
                                status_clone,
                                peers_clone,
                                active_peer_clone,
                                relay_sender_clone,
                                chat_clone,
                                seen_msg_clone,
                                pending_clone,
                                shutdown_clone,
                                logger_clone,
                                next_id,
                                audio_engine_clone,
                                latest_video_clone,
                                remote_media_clone,
                            ).await;
                        }
                        Err(e) => {
                            logger_clone.error(&format!("Ошибка разбора Answer гостя: {}", e));
                            *status_clone.write() = ConnectionStatus::Failed { error: e };
                        }
                    }
                }
                Err(e) => {
                    logger_clone.warn(&format!("Сигналинг комнаты завершился: {}", e));
                }
            }
        });
        *self.session_abort.write() = Some(task.abort_handle());

        Ok(clean_code)
    }

    /// Mode: Client joins room with a 6-digit code
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
            "Вход в комнату '{}' (Код: {}), никнейм '{}'",
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

        // 2. Gather candidates (LAN + STUN)
        let candidates = gather_candidates(arc_socket.clone(), actual_port, opt_physical_ip, &self.logger).await;
        let answer_link = create_answer_link(&nickname, candidates.clone());

        // 3. Exchange via Signaling broker
        let logger_clone = self.logger.clone();
        let sock_clone = arc_socket.clone();
        let status_clone = self.status.clone();
        let peers_clone = self.peers.clone();
        let active_peer_clone = self.active_peer_addr.clone();
        let relay_sender_clone = self.relay_sender.clone();
        let chat_clone = self.chat_messages.clone();
        let seen_msg_clone = self.seen_msg_ids.clone();
        let pending_clone = self.pending_acks.clone();
        let shutdown_clone = self.shutdown_signal.clone();
        let client_name = nickname.clone();
        let next_id = self.next_peer_id.clone();
        let my_candidates = candidates.clone();
        let code_for_task = clean_code.clone();
        let audio_engine_clone = self.audio_engine.clone();
        let latest_video_clone = self.latest_remote_video.clone();
        let remote_media_clone = self.remote_media_state.clone();

        let task = tokio::spawn(async move {
            let offer_res = MqttSignaling::client_exchange(&code_for_task, &answer_link, Duration::from_secs(45), &logger_clone).await;

            if shutdown_clone.load(Ordering::Relaxed) {
                return;
            }

            match offer_res {
                Ok(offer_str) => {
                    match parse_offer_link(&offer_str) {
                        Ok(offer) => {
                            logger_clone.info(&format!("Offer от хоста '{}' получен! Начинаем встречное пробитие NAT...", offer.name));
                            run_simultaneous_punching_flow(
                                sock_clone,
                                client_name,
                                offer.room,
                                code_for_task,
                                offer.name,
                                offer.candidates,
                                my_candidates,
                                status_clone,
                                peers_clone,
                                active_peer_clone,
                                relay_sender_clone,
                                chat_clone,
                                seen_msg_clone,
                                pending_clone,
                                shutdown_clone,
                                logger_clone,
                                next_id,
                                audio_engine_clone,
                                latest_video_clone,
                                remote_media_clone,
                            ).await;
                        }
                        Err(e) => {
                            logger_clone.error(&format!("Ошибка разбора Offer: {}", e));
                            *status_clone.write() = ConnectionStatus::Failed { error: e };
                        }
                    }
                }
                Err(e) => {
                    logger_clone.error(&format!("Не удалось подключиться: {}", e));
                    *status_clone.write() = ConnectionStatus::Failed { error: e };
                }
            }
        });
        *self.session_abort.write() = Some(task.abort_handle());

        Ok(())
    }
}

/// Gathers LAN and STUN candidates
async fn gather_candidates(
    socket: Arc<UdpSocket>,
    port: u16,
    opt_physical_ip: Option<Ipv4Addr>,
    logger: &AppLogger,
) -> Vec<SocketAddr> {
    let mut list = Vec::new();

    // 1. Local LAN IP: prioritize chosen physical interface IP, fallback to auto-detection
    let lan_ip = opt_physical_ip.map(IpAddr::V4).unwrap_or_else(detect_local_lan_ip);
    let local_addr = SocketAddr::new(lan_ip, port);
    list.push(local_addr);
    logger.info(&format!("Локальный адрес кандидата: {}", local_addr));

    // 2. STUN Public IP
    if let Some(pub_addr) = discover_public_ip_via_stun(socket, logger).await {
        logger.info(&format!("STUN публичный адрес кандидата: {}", pub_addr));
        if !list.contains(&pub_addr) {
            list.push(pub_addr);
        }
    } else {
        logger.warn("STUN: не удалось определить публичный адрес (будет использован LAN/VPN адрес)");
    }

    list
}

/// Expand candidates with delta ports for Symmetric NAT / CGNAT port drift
fn expand_candidates_with_port_spray(candidates: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut targets = Vec::new();
    for &addr in candidates {
        targets.push(addr);
        if let SocketAddr::V4(v4) = addr {
            let port = v4.port();
            for delta in [-5i32, -4, -3, -2, -1, 1, 2, 3, 4, 5] {
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

struct VideoReassembler {
    current_frame_id: u32,
    chunk_total: u16,
    is_screen: bool,
    chunks: Vec<Option<Vec<u8>>>,
    created_at: Instant,
}

impl VideoReassembler {
    fn new() -> Self {
        Self {
            current_frame_id: 0,
            chunk_total: 0,
            is_screen: false,
            chunks: Vec::new(),
            created_at: Instant::now(),
        }
    }

    fn handle_chunk(
        &mut self,
        frame_id: u32,
        chunk_idx: u16,
        chunk_total: u16,
        is_screen: bool,
        data: Vec<u8>,
        target: &Arc<RwLock<Option<RemoteVideoFrame>>>,
    ) {
        use base64::Engine;
        if frame_id > self.current_frame_id || self.created_at.elapsed() > Duration::from_millis(800) {
            self.current_frame_id = frame_id;
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

/// Core simultaneous hole punching and connected session loop
async fn run_simultaneous_punching_flow(
    socket: Arc<UdpSocket>,
    my_name: String,
    room_name: String,
    room_code: String,
    peer_name_expected: String,
    remote_candidates: Vec<SocketAddr>,
    _my_candidates: Vec<SocketAddr>,
    status: Arc<RwLock<ConnectionStatus>>,
    peers: Arc<RwLock<HashMap<SocketAddr, PeerInfo>>>,
    active_peer: Arc<RwLock<Option<SocketAddr>>>,
    relay_sender: Arc<RwLock<Option<tokio::sync::mpsc::Sender<Packet>>>>,
    chat_messages: Arc<RwLock<Vec<ChatMessage>>>,
    seen_msg_ids: Arc<RwLock<HashSet<u64>>>,
    pending_acks: Arc<RwLock<HashMap<u64, oneshot::Sender<()>>>>,
    shutdown: Arc<AtomicBool>,
    logger: AppLogger,
    next_peer_id: Arc<AtomicU32>,
    audio_engine: Arc<crate::audio::AudioEngine>,
    latest_remote_video: Arc<RwLock<Option<RemoteVideoFrame>>>,
    remote_media_state: Arc<RwLock<RemoteMediaState>>,
) {
    let targets = expand_candidates_with_port_spray(&remote_candidates);
    logger.info(&format!(
        "Встречное пробитие NAT: рассылка Punch пакетов на {} адресов собеседника '{}'...",
        targets.len(),
        peer_name_expected
    ));

    *status.write() = ConnectionStatus::Connecting {
        target_str: peer_name_expected.clone(),
        attempt: 1,
        max_attempts: 2,
        mode: "Пробитие прямого NAT / Рукопожатие...".into(),
    };

    let punch_pkt = Packet::Punch {
        nonce: rand::random(),
        sender_name: my_name.clone(),
    };
    let punch_bytes = match punch_pkt.encode() {
        Ok(b) => b,
        Err(e) => {
            logger.error(&format!("Ошибка кодирования Punch: {}", e));
            return;
        }
    };

    let mut buf = [0u8; 4096];
    let punch_start = Instant::now();
    let punch_timeout = Duration::from_secs(7);
    let mut last_blast = Instant::now() - Duration::from_secs(1);
    let mut connected_endpoint: Option<SocketAddr> = None;
    let mut resolved_peer_name = peer_name_expected.clone();

    // PHASE 1: SIMULTANEOUS HOLE PUNCHING (up to 7 seconds)
    while punch_start.elapsed() < punch_timeout && !shutdown.load(Ordering::Relaxed) {
        // Send punch packets to all targets every 80ms
        if last_blast.elapsed() >= Duration::from_millis(80) {
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
                                Packet::Punch { sender_name, nonce } => {
                                    logger.info(&format!(
                                        "УСПЕХ: получен встречный Punch от '{}' ({})! Отправляем PunchAck...",
                                        sender_name, sender_addr
                                    ));
                                    resolved_peer_name = sender_name.clone();
                                    let ack = Packet::PunchAck { nonce, sender_name: my_name.clone() };
                                    if let Ok(ack_b) = ack.encode() {
                                        for _ in 0..4 {
                                            let _ = socket.send_to(&ack_b, sender_addr).await;
                                        }
                                    }
                                    connected_endpoint = Some(sender_addr);
                                    break;
                                }
                                Packet::PunchAck { sender_name, .. } => {
                                    logger.info(&format!(
                                        "УСПЕХ: получен PunchAck от '{}' ({})! Канал P2P открыт!",
                                        sender_name, sender_addr
                                    ));
                                    resolved_peer_name = sender_name;
                                    connected_endpoint = Some(sender_addr);
                                    break;
                                }
                                Packet::TextMessage { .. } | Packet::Ping { .. } => {
                                    logger.info(&format!("Получен P2P пакет от {}! Соединение установлено.", sender_addr));
                                    connected_endpoint = Some(sender_addr);
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        if e.raw_os_error() == Some(10054) {
                            continue; // Ignore ICMP reset on Windows
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }

    if shutdown.load(Ordering::Relaxed) {
        return;
    }

    let active_addr = match connected_endpoint {
        Some(addr) => addr,
        None => {
            logger.warn(&format!(
                "Прямое UDP пробитие NAT не удалось после {}с (возможно, блокировка UDP провайдером или Symmetric NAT). Включаем резервный WSS релей...",
                punch_timeout.as_secs()
            ));

            *status.write() = ConnectionStatus::Connecting {
                target_str: peer_name_expected.clone(),
                attempt: 2,
                max_attempts: 2,
                mode: "Подключение через WSS релей...".into(),
            };

            match crate::signaling::MqttSignaling::start_relay_session(&room_code, &my_name, &logger).await {
                Ok(relay_session) => {
                    logger.info("Резервный релейный канал успешно активирован!");
                    *active_peer.write() = None;
                    *relay_sender.write() = Some(relay_session.outgoing_tx.clone());

                    let peer_id = next_peer_id.fetch_add(1, Ordering::SeqCst);
                    let relay_virtual_addr = "127.0.0.1:443".parse::<SocketAddr>().unwrap();

                    peers.write().insert(
                        relay_virtual_addr,
                        PeerInfo {
                            id: peer_id,
                            name: peer_name_expected.clone(),
                            addr: "Релей (WSS TLS/443)".into(),
                            ping_ms: 35,
                            last_seen_ms: now_ms(),
                        },
                    );

                    *status.write() = ConnectionStatus::Connected {
                        peer_name: peer_name_expected.clone(),
                        peer_addr: "Релей (WSS TLS/443)".into(),
                        room_name: room_name.clone(),
                        ping_ms: 35,
                    };

                    logger.info("============================================================");
                    logger.info(&format!(
                        "P2P СОЕДИНЕНИЕ УСТАНОВЛЕНО ЧЕРЕЗ WSS-РЕЛЕЙ! Собеседник: '{}'",
                        peer_name_expected
                    ));
                    logger.info("============================================================");
                    audio_engine.play_connect_chime();

                    run_relay_connected_loop(
                        relay_session,
                        my_name,
                        peer_name_expected,
                        status,
                        peers,
                        chat_messages,
                        seen_msg_ids,
                        pending_acks,
                        shutdown,
                        logger,
                        audio_engine.clone(),
                        latest_remote_video.clone(),
                        remote_media_state.clone(),
                    ).await;
                    return;
                }
                Err(e) => {
                    let err = format!(
                        "Не удалось установить ни прямое P2P соединение, ни подключение через релей: {}",
                        e
                    );
                    logger.error(&err);
                    *status.write() = ConnectionStatus::Failed { error: err };
                    return;
                }
            }
        }
    };

    // PHASE 2: DIRECT UDP CONNECTED STATE
    *active_peer.write() = Some(active_addr);
    *relay_sender.write() = None;
    let peer_id = next_peer_id.fetch_add(1, Ordering::SeqCst);

    peers.write().insert(
        active_addr,
        PeerInfo {
            id: peer_id,
            name: resolved_peer_name.clone(),
            addr: active_addr.to_string(),
            ping_ms: 1,
            last_seen_ms: now_ms(),
        },
    );

    *status.write() = ConnectionStatus::Connected {
        peer_name: resolved_peer_name.clone(),
        peer_addr: active_addr.to_string(),
        room_name: room_name.clone(),
        ping_ms: 1,
    };

    logger.info(&format!(
        "============================================================"
    ));
    logger.info(&format!(
        "P2P СОЕДИНЕНИЕ УСТАНОВЛЕНО НАПРЯМУЮ! Собеседник: '{}' ({})",
        resolved_peer_name, active_addr
    ));
    logger.info(&format!(
        "============================================================"
    ));

    // Start native audio engine for voice call
    if let Err(e) = audio_engine.start() {
        logger.error(&format!("Не удалось запустить аудио движок: {}", e));
    } else {
        logger.info("Аудио движок запущен для прямого P2P голосового звонка");
        audio_engine.play_connect_chime();
    }

    // PHASE 3: ACTIVE SESSION (Audio/Video + Ping/Pong + Text Chat + Disconnect + Auto Re-Punching)
    let mut last_ping_sent = Instant::now();
    let mut last_repunch = Instant::now() - Duration::from_secs(10);
    let mut ping_seq = 0u64;
    // Dedicated low-latency background audio transmitter (sends smoothly on 10ms ticks)
    let audio_sock = socket.clone();
    let audio_engine_tx = audio_engine.clone();
    let shutdown_tx = shutdown.clone();

    let audio_tx_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut audio_seq = 0u64;

        while !shutdown_tx.load(Ordering::Relaxed) {
            interval.tick().await;
            for audio_payload in audio_engine_tx.pop_outgoing_packets() {
                audio_seq += 1;
                let audio_pkt = Packet::AudioData {
                    seq: audio_seq,
                    data: audio_payload,
                };
                if let Ok(b) = audio_pkt.encode() {
                    let _ = audio_sock.send_to(&b, active_addr).await;
                }
            }
        }
    });

    let mut video_reassembler = VideoReassembler::new();

    while !shutdown.load(Ordering::Relaxed) {
        // Send Ping every 1.5 seconds to keep mobile CGNAT mappings alive
        if last_ping_sent.elapsed() >= Duration::from_millis(1500) {
            ping_seq += 1;
            let ping_pkt = Packet::Ping {
                seq: ping_seq,
                timestamp_ms: now_ms(),
            };
            if let Ok(b) = ping_pkt.encode() {
                let _ = socket.send_to(&b, active_addr).await;
            }
            last_ping_sent = Instant::now();

            let silence = {
                let p_table = peers.read();
                if let Some(p) = p_table.get(&active_addr) {
                    now_ms().saturating_sub(p.last_seen_ms)
                } else {
                    0
                }
            };

            // If no packets received for >3.5s, mobile carrier may have dropped NAT mapping
            // Immediately spray Punch packets to remote endpoints to re-open NAT
            if silence > 3500 && last_repunch.elapsed() >= Duration::from_millis(1500) {
                logger.warn(&format!(
                    "Пауза связи ({}мс), повторный залп Punch для восстановления мобильного CGNAT...",
                    silence
                ));
                for &t in &targets {
                    let _ = socket.send_to(&punch_bytes, t).await;
                }
                last_repunch = Instant::now();
            }

            if silence > 10000 {
                logger.warn("Таймаут P2P связи (собеседник не отвечает более 10с)");
            }
        }

        tokio::select! {
            recv_res = socket.recv_from(&mut buf) => {
                match recv_res {
                    Ok((len, sender_addr)) => {
                        if sender_addr == active_addr || targets.contains(&sender_addr) {
                            if let Ok(pkt) = Packet::decode(&buf[..len]) {
                                match pkt {
                                    Packet::AudioData { data, .. } => {
                                        audio_engine.feed_incoming_packet(&data);
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.last_seen_ms = now_ms();
                                        }
                                    }
                                    Packet::VideoChunk {
                                        frame_id,
                                        chunk_idx,
                                        chunk_total,
                                        is_screen,
                                        data,
                                    } => {
                                        video_reassembler.handle_chunk(
                                            frame_id,
                                            chunk_idx,
                                            chunk_total,
                                            is_screen,
                                            data,
                                            &latest_remote_video,
                                        );
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.last_seen_ms = now_ms();
                                        }
                                    }
                                    Packet::MediaState {
                                        mic_muted,
                                        deafened,
                                        video_active,
                                        screen_active,
                                    } => {
                                        *remote_media_state.write() = RemoteMediaState {
                                            mic_muted,
                                            deafened,
                                            video_active,
                                            screen_active,
                                        };
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.last_seen_ms = now_ms();
                                        }
                                    }
                                    Packet::Ping { seq, timestamp_ms } => {
                                        let pong = Packet::Pong { seq, timestamp_ms };
                                        if let Ok(b) = pong.encode() {
                                            let _ = socket.send_to(&b, active_addr).await;
                                        }
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.last_seen_ms = now_ms();
                                        }
                                    }
                                    Packet::Pong { timestamp_ms, .. } => {
                                        let rtt = (now_ms().saturating_sub(timestamp_ms) as u32).max(1);
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.ping_ms = rtt;
                                            p.last_seen_ms = now_ms();
                                        }
                                        let mut st = status.write();
                                        if let ConnectionStatus::Connected { ping_ms, .. } = &mut *st {
                                            *ping_ms = rtt;
                                        }
                                    }
                                    Packet::TextMessage { id, text, sender_name, timestamp_ms } => {
                                        // Immediately send TextAck (3 times to guarantee delivery over mobile cellular loss)
                                        let ack = Packet::TextAck { id };
                                        if let Ok(ack_b) = ack.encode() {
                                            let _ = socket.send_to(&ack_b, active_addr).await;
                                            let _ = socket.send_to(&ack_b, active_addr).await;
                                            let _ = socket.send_to(&ack_b, active_addr).await;
                                        }

                                        // Deduplicate incoming messages strictly by id
                                        let is_new = seen_msg_ids.write().insert(id);
                                        if is_new {
                                            logger.info(&format!("[ЧАТ] {}: {}", sender_name, text));
                                            chat_messages.write().push(ChatMessage {
                                                id,
                                                sender: sender_name,
                                                text,
                                                timestamp_ms,
                                                is_me: false,
                                            });
                                        }
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.last_seen_ms = now_ms();
                                        }
                                    }
                                    Packet::TextAck { id } => {
                                        if let Some(tx) = pending_acks.write().remove(&id) {
                                            let _ = tx.send(());
                                        }
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.last_seen_ms = now_ms();
                                        }
                                    }
                                    Packet::Punch { nonce, .. } => {
                                        // Peer is re-punching to recover NAT hole
                                        let ack = Packet::PunchAck { nonce, sender_name: my_name.clone() };
                                        if let Ok(ack_b) = ack.encode() {
                                            let _ = socket.send_to(&ack_b, active_addr).await;
                                        }
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.last_seen_ms = now_ms();
                                        }
                                    }
                                    Packet::PunchAck { .. } => {
                                        if let Some(p) = peers.write().get_mut(&active_addr) {
                                            p.last_seen_ms = now_ms();
                                        }
                                    }
                                    Packet::Disconnect { reason } => {
                                        logger.warn(&format!("Собеседник отключил связь: {}", reason));
                                        audio_engine.play_disconnect_chime();
                                        *status.write() = ConnectionStatus::Failed {
                                            error: format!("Собеседник отключился: {}", reason),
                                        };
                                        audio_tx_task.abort();
                                        audio_engine.stop();
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if e.raw_os_error() == Some(10054) {
                            continue;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    audio_tx_task.abort();
    audio_engine.stop();
}

/// Active session loop for Relay (Nostr WSS TLS/443) fallback
async fn run_relay_connected_loop(
    mut relay_session: crate::signaling::RelaySession,
    my_name: String,
    _peer_name_expected: String,
    status: Arc<RwLock<ConnectionStatus>>,
    peers: Arc<RwLock<HashMap<SocketAddr, PeerInfo>>>,
    chat_messages: Arc<RwLock<Vec<ChatMessage>>>,
    seen_msg_ids: Arc<RwLock<HashSet<u64>>>,
    pending_acks: Arc<RwLock<HashMap<u64, oneshot::Sender<()>>>>,
    shutdown: Arc<AtomicBool>,
    logger: AppLogger,
    audio_engine: Arc<crate::audio::AudioEngine>,
    latest_remote_video: Arc<RwLock<Option<RemoteVideoFrame>>>,
    remote_media_state: Arc<RwLock<RemoteMediaState>>,
) {
    let mut last_ping_sent = Instant::now();
    let mut ping_seq = 0u64;
    let mut video_reassembler = VideoReassembler::new();
    let relay_virtual_addr = "127.0.0.1:443".parse::<SocketAddr>().unwrap();

    // Start audio engine for relay voice call
    if let Err(e) = audio_engine.start() {
        logger.error(&format!("(Релей) Не удалось запустить аудио движок: {}", e));
    } else {
        logger.info("(Релей) Аудио движок запущен для WSS P2P голосового звонка");
    }

    // Send initial Punch packet through relay to guarantee mutual session activation
    let init_punch = Packet::Punch {
        nonce: rand::random(),
        sender_name: my_name.clone(),
    };
    let _ = relay_session.outgoing_tx.send(init_punch).await;

    // Dedicated low-latency background audio transmitter for relay (10ms cadence)
    let relay_tx = relay_session.outgoing_tx.clone();
    let audio_engine_tx = audio_engine.clone();
    let shutdown_tx = shutdown.clone();

    let audio_tx_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut audio_seq = 0u64;

        while !shutdown_tx.load(Ordering::Relaxed) {
            interval.tick().await;
            for audio_payload in audio_engine_tx.pop_outgoing_packets() {
                audio_seq += 1;
                let audio_pkt = Packet::AudioData {
                    seq: audio_seq,
                    data: audio_payload,
                };
                let _ = relay_tx.send(audio_pkt).await;
            }
        }
    });

    while !shutdown.load(Ordering::Relaxed) {
        // Send Ping every 2 seconds over relay to measure latency and keep alive
        if last_ping_sent.elapsed() >= Duration::from_millis(2000) {
            ping_seq += 1;
            let ping_pkt = Packet::Ping {
                seq: ping_seq,
                timestamp_ms: now_ms(),
            };
            let _ = relay_session.outgoing_tx.send(ping_pkt).await;
            last_ping_sent = Instant::now();
        }

        tokio::select! {
            recv_opt = relay_session.incoming_rx.recv() => {
                match recv_opt {
                    Some(pkt) => {
                        match pkt {
                            Packet::AudioData { data, .. } => {
                                audio_engine.feed_incoming_packet(&data);
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.last_seen_ms = now_ms();
                                }
                            }
                            Packet::VideoChunk {
                                frame_id,
                                chunk_idx,
                                chunk_total,
                                is_screen,
                                data,
                            } => {
                                video_reassembler.handle_chunk(
                                    frame_id,
                                    chunk_idx,
                                    chunk_total,
                                    is_screen,
                                    data,
                                    &latest_remote_video,
                                );
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.last_seen_ms = now_ms();
                                }
                            }
                            Packet::MediaState {
                                mic_muted,
                                deafened,
                                video_active,
                                screen_active,
                            } => {
                                *remote_media_state.write() = RemoteMediaState {
                                    mic_muted,
                                    deafened,
                                    video_active,
                                    screen_active,
                                };
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.last_seen_ms = now_ms();
                                }
                            }
                            Packet::Ping { seq, timestamp_ms } => {
                                let pong = Packet::Pong { seq, timestamp_ms };
                                let _ = relay_session.outgoing_tx.send(pong).await;
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.last_seen_ms = now_ms();
                                }
                            }
                            Packet::Pong { timestamp_ms, .. } => {
                                let rtt = (now_ms().saturating_sub(timestamp_ms) as u32).max(1);
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.ping_ms = rtt;
                                    p.last_seen_ms = now_ms();
                                }
                                let mut st = status.write();
                                if let ConnectionStatus::Connected { ping_ms, .. } = &mut *st {
                                    *ping_ms = rtt;
                                }
                            }
                            Packet::TextMessage { id, text, sender_name, timestamp_ms } => {
                                // Send TextAck back through relay
                                let ack = Packet::TextAck { id };
                                let _ = relay_session.outgoing_tx.send(ack).await;

                                let is_new = seen_msg_ids.write().insert(id);
                                if is_new {
                                    logger.info(&format!("[ЧАТ Релей] {}: {}", sender_name, text));
                                    chat_messages.write().push(ChatMessage {
                                        id,
                                        sender: sender_name,
                                        text,
                                        timestamp_ms,
                                        is_me: false,
                                    });
                                }
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.last_seen_ms = now_ms();
                                }
                            }
                            Packet::TextAck { id } => {
                                if let Some(tx) = pending_acks.write().remove(&id) {
                                    let _ = tx.send(());
                                }
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.last_seen_ms = now_ms();
                                }
                            }
                            Packet::Punch { nonce, .. } => {
                                let ack = Packet::PunchAck { nonce, sender_name: my_name.clone() };
                                let _ = relay_session.outgoing_tx.send(ack).await;
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.last_seen_ms = now_ms();
                                }
                            }
                            Packet::PunchAck { .. } => {
                                if let Some(p) = peers.write().get_mut(&relay_virtual_addr) {
                                    p.last_seen_ms = now_ms();
                                }
                            }
                            Packet::Disconnect { reason } => {
                                logger.warn(&format!("Собеседник отключил связь (релей): {}", reason));
                                audio_engine.play_disconnect_chime();
                                *status.write() = ConnectionStatus::Failed {
                                    error: format!("Собеседник отключился: {}", reason),
                                };
                                audio_tx_task.abort();
                                audio_engine.stop();
                                return;
                            }
                            _ => {}
                        }
                    }
                    None => {
                        logger.warn("Канал релея закрыт");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(15)) => {}
        }
    }
    audio_tx_task.abort();
    audio_engine.stop();
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

    // Build RFC 5389 Binding Request
    let mut req = [0u8; 20];
    req[0..2].copy_from_slice(&0x0001u16.to_be_bytes()); // Type: Binding Request
    req[2..4].copy_from_slice(&0x0000u16.to_be_bytes()); // Length: 0
    req[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes()); // Magic Cookie
    req[8..20].copy_from_slice(&tx_id);

    // 1. Resolve hostnames in parallel with short timeout
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
                // IMPORTANT: Filter out IPv6 to prevent WSAEAFNOSUPPORT error on IPv4 socket
                if let SocketAddr::V4(_) = addr {
                    if !targets.contains(&addr) {
                        targets.push(addr);
                    }
                }
            }
        }
    }

    if targets.is_empty() {
        logger.warn("STUN: Не удалось разрешить STUN-серверы (проверьте интернет)");
        return None;
    }

    targets.truncate(8);

    // 2. Parallel blast to all targets
    for &t in &targets {
        let _ = socket.send_to(&req, t).await;
    }

    let mut buf = [0u8; 512];
    let start_time = Instant::now();
    let mut last_reblast = Instant::now();
    let overall_timeout = Duration::from_millis(2500);

    // 3. Fast response collection with 400ms retry
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
                            logger.info(&format!("STUN: получен ответ от {} -> публичный адрес: {}", from, pub_addr));
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

        // XOR-MAPPED-ADDRESS (0x0020)
        if attr_type == 0x0020 && attr_len >= 8 {
            let family = buf[offset + 1];
            let x_port = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]);
            let port = x_port ^ 0x2112;

            if family == 0x01 {
                // IPv4
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
            // Standard MAPPED-ADDRESS (0x0001)
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

        // Expand socket buffers to 1 MB so burst video chunks never drop packets
        let buf_size: i32 = 1024 * 1024;
        let _ = setsockopt(raw_sock, 0xFFFF /* SOL_SOCKET */, 0x1002 /* SO_RCVBUF */, &buf_size as *const _ as *const u8, 4);
        let _ = setsockopt(raw_sock, 0xFFFF /* SOL_SOCKET */, 0x1001 /* SO_SNDBUF */, &buf_size as *const _ as *const u8, 4);

        if let Some((if_idx, opt_ip)) = find_best_physical_interface() {
            let if_index_be: u32 = if_idx.to_be(); // network byte order for Winsock IP_UNICAST_IF
            let res = setsockopt(
                raw_sock,
                0,  /* IPPROTO_IP */
                31, /* IP_UNICAST_IF */
                &if_index_be as *const _ as *const u8,
                4,
            );
            if res == 0 {
                logger.info(&format!(
                    "UDP сокет привязан к физическому интерфейсу #{} (обход VPN, LAN IP: {:?})",
                    if_idx,
                    opt_ip.unwrap_or(Ipv4Addr::UNSPECIFIED)
                ));
            } else {
                logger.warn(&format!(
                    "Не удалось установить IP_UNICAST_IF для интерфейса #{}",
                    if_idx
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
            let is_up = adapter.OperStatus == 1; // IfOperStatusUp
            let if_index = adapter.Anonymous1.Anonymous.IfIndex;

            if is_up && if_index > 0 {
                let mut score = 0i32;

                // Physical interface type: 6 = Ethernet, 71 = Wi-Fi, 243/244 = WWAN
                let is_physical = adapter.IfType == 6
                    || adapter.IfType == 71
                    || adapter.IfType == 243
                    || adapter.IfType == 244;

                if is_physical {
                    score += 200;
                }

                // Check default gateway (real physical internet interface has gateway)
                if !adapter.FirstGatewayAddress.is_null() {
                    score += 1000;
                }

                // Check blacklist
                let is_blacklisted = blacklist.iter().any(|&w| desc.contains(w) || friendly.contains(w));
                if is_blacklisted {
                    score -= 5000;
                }

                // Extract IPv4 address if available
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

        // Simulate simultaneous punching directly between two local ports
        let sock_a = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let sock_b = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());

        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        *node_a.active_socket.write() = Some(sock_a.clone());
        *node_b.active_socket.write() = Some(sock_b.clone());

        *node_a.active_peer_addr.write() = Some(addr_b);
        *node_b.active_peer_addr.write() = Some(addr_a);

        // Send a message with id 999
        let pkt = Packet::TextMessage {
            id: 999,
            text: "Тестовое сообщение".into(),
            sender_name: "Alice".into(),
            timestamp_ms: now_ms(),
        };
        let b = pkt.encode().unwrap();

        // Send it 3 times to simulate duplicate UDP arrivals
        sock_a.send_to(&b, addr_b).await.unwrap();
        sock_a.send_to(&b, addr_b).await.unwrap();
        sock_a.send_to(&b, addr_b).await.unwrap();

        // Simulate receiver loop on B for 3 packets
        let mut buf = [0u8; 1024];
        for _ in 0..3 {
            let (len, _) = sock_b.recv_from(&mut buf).await.unwrap();
            let decoded = Packet::decode(&buf[..len]).unwrap();
            if let Packet::TextMessage { id, text, sender_name, timestamp_ms } = decoded {
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

    #[tokio::test]
    async fn test_fast_parallel_stun() {
        let sock = Arc::new(UdpSocket::bind("0.0.0.0:0").await.unwrap());
        let logger = AppLogger::init();
        let start = Instant::now();
        let pub_addr = discover_public_ip_via_stun(sock, &logger).await;
        let elapsed = start.elapsed();
        println!("STUN discovered: {:?} in {:?}", pub_addr, elapsed);
        assert!(pub_addr.is_some(), "STUN must discover public IP");
    }
}
