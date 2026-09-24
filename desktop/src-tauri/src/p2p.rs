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
    chat_messages: Arc<RwLock<Vec<ChatMessage>>>,
    seen_msg_ids: Arc<RwLock<HashSet<u64>>>,
    pending_acks: Arc<RwLock<HashMap<u64, oneshot::Sender<()>>>>,
    session_abort: Arc<RwLock<Option<tokio::task::AbortHandle>>>,
    shutdown_signal: Arc<AtomicBool>,
    next_peer_id: Arc<AtomicU32>,
}

impl P2pNode {
    pub fn new(logger: AppLogger) -> Self {
        Self {
            logger,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            peers: Arc::new(RwLock::new(HashMap::new())),
            active_socket: Arc::new(RwLock::new(None)),
            active_peer_addr: Arc::new(RwLock::new(None)),
            chat_messages: Arc::new(RwLock::new(Vec::new())),
            seen_msg_ids: Arc::new(RwLock::new(HashSet::new())),
            pending_acks: Arc::new(RwLock::new(HashMap::new())),
            session_abort: Arc::new(RwLock::new(None)),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            next_peer_id: Arc::new(AtomicU32::new(1)),
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

        self.logger.info("Отключение P2P соединения...");
        *self.active_socket.write() = None;
        *self.active_peer_addr.write() = None;
        self.peers.write().clear();
        self.seen_msg_ids.write().clear();
        self.pending_acks.write().clear();
        *self.status.write() = ConnectionStatus::Disconnected;
        self.logger.info("Состояние сброшено в Disconnected");
    }

    /// Send a text message to the connected peer over P2P with guaranteed delivery (ACK retry loop)
    pub async fn send_chat_message(&self, text: String, my_name: String) -> Result<ChatMessage, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("Сообщение не может быть пустым".into());
        }

        let sock = self
            .active_socket
            .read()
            .clone()
            .ok_or_else(|| "Нет активного сокета (вы не подключены)".to_string())?;

        let peer_addr = self
            .active_peer_addr
            .read()
            .ok_or_else(|| "Нет подключенного собеседника".to_string())?;

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
        configure_socket_physical_if(&socket, &self.logger);
        let actual_port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
        let arc_socket = Arc::new(socket);
        *self.active_socket.write() = Some(arc_socket.clone());

        // 2. Gather candidates (LAN + STUN)
        let candidates = gather_candidates(arc_socket.clone(), actual_port, &self.logger).await;

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
        let chat_clone = self.chat_messages.clone();
        let seen_msg_clone = self.seen_msg_ids.clone();
        let pending_clone = self.pending_acks.clone();
        let shutdown_clone = self.shutdown_signal.clone();
        let host_name = nickname.clone();
        let room_name = room_code.clone();
        let next_id = self.next_peer_id.clone();
        let my_candidates = candidates.clone();
        let code_for_task = clean_code.clone();

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
                                answer.name,
                                answer.candidates,
                                my_candidates,
                                status_clone,
                                peers_clone,
                                active_peer_clone,
                                chat_clone,
                                seen_msg_clone,
                                pending_clone,
                                shutdown_clone,
                                logger_clone,
                                next_id,
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
        configure_socket_physical_if(&socket, &self.logger);
        let actual_port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
        let arc_socket = Arc::new(socket);
        *self.active_socket.write() = Some(arc_socket.clone());

        // 2. Gather candidates (LAN + STUN)
        let candidates = gather_candidates(arc_socket.clone(), actual_port, &self.logger).await;
        let answer_link = create_answer_link(&nickname, candidates.clone());

        // 3. Exchange via Signaling broker
        let logger_clone = self.logger.clone();
        let sock_clone = arc_socket.clone();
        let status_clone = self.status.clone();
        let peers_clone = self.peers.clone();
        let active_peer_clone = self.active_peer_addr.clone();
        let chat_clone = self.chat_messages.clone();
        let seen_msg_clone = self.seen_msg_ids.clone();
        let pending_clone = self.pending_acks.clone();
        let shutdown_clone = self.shutdown_signal.clone();
        let client_name = nickname.clone();
        let next_id = self.next_peer_id.clone();
        let my_candidates = candidates.clone();

        let task = tokio::spawn(async move {
            let offer_res = MqttSignaling::client_exchange(&clean_code, &answer_link, Duration::from_secs(45), &logger_clone).await;

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
                                offer.name,
                                offer.candidates,
                                my_candidates,
                                status_clone,
                                peers_clone,
                                active_peer_clone,
                                chat_clone,
                                seen_msg_clone,
                                pending_clone,
                                shutdown_clone,
                                logger_clone,
                                next_id,
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
    logger: &AppLogger,
) -> Vec<SocketAddr> {
    let mut list = Vec::new();

    // 1. Local LAN IP
    let lan_ip = detect_local_lan_ip();
    let local_addr = SocketAddr::new(lan_ip, port);
    list.push(local_addr);
    logger.info(&format!("Локальный адрес кандидата: {}", local_addr));

    // 2. STUN Public IP
    if let Some(pub_addr) = discover_public_ip_via_stun(socket).await {
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
            for delta in [-3i32, -2, -1, 1, 2, 3] {
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

/// Core simultaneous hole punching and connected session loop
async fn run_simultaneous_punching_flow(
    socket: Arc<UdpSocket>,
    my_name: String,
    room_name: String,
    peer_name_expected: String,
    remote_candidates: Vec<SocketAddr>,
    _my_candidates: Vec<SocketAddr>,
    status: Arc<RwLock<ConnectionStatus>>,
    peers: Arc<RwLock<HashMap<SocketAddr, PeerInfo>>>,
    active_peer: Arc<RwLock<Option<SocketAddr>>>,
    chat_messages: Arc<RwLock<Vec<ChatMessage>>>,
    seen_msg_ids: Arc<RwLock<HashSet<u64>>>,
    pending_acks: Arc<RwLock<HashMap<u64, oneshot::Sender<()>>>>,
    shutdown: Arc<AtomicBool>,
    logger: AppLogger,
    next_peer_id: Arc<AtomicU32>,
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
        max_attempts: 1,
        mode: "Пробитие NAT / Рукопожатие...".into(),
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
    let punch_timeout = Duration::from_secs(12); // Extended timeout for mobile networks
    let mut last_blast = Instant::now() - Duration::from_secs(1);
    let mut connected_endpoint: Option<SocketAddr> = None;
    let mut resolved_peer_name = peer_name_expected.clone();

    // PHASE 1: SIMULTANEOUS HOLE PUNCHING (up to 12 seconds)
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
            let err = format!(
                "Не удалось пробить NAT с собеседником '{}' после 12 секунд встречных попыток. Проверьте соединение и попробуйте снова.",
                peer_name_expected
            );
            logger.error(&err);
            *status.write() = ConnectionStatus::Failed { error: err };
            return;
        }
    };

    // PHASE 2: CONNECTED STATE
    *active_peer.write() = Some(active_addr);
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

    // PHASE 3: ACTIVE SESSION (Ping/Pong + Text Chat + Disconnect + Auto Re-Punching)
    let mut last_ping_sent = Instant::now();
    let mut last_repunch = Instant::now() - Duration::from_secs(10);
    let mut ping_seq = 0u64;

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
                                        *status.write() = ConnectionStatus::Failed {
                                            error: format!("Собеседник отключился: {}", reason),
                                        };
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
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
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

async fn discover_public_ip_via_stun(socket: Arc<UdpSocket>) -> Option<SocketAddr> {
    const STUN_SERVERS: &[&str] = &[
        "stun.l.google.com:19302",
        "stun1.l.google.com:19302",
        "stun.cloudflare.com:3478",
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

    let mut buf = [0u8; 512];

    for server in STUN_SERVERS {
        if let Ok(addrs) = tokio::net::lookup_host(server).await {
            for target in addrs {
                let _ = socket.send_to(&req, target).await;

                let timeout = tokio::time::sleep(Duration::from_millis(600));
                tokio::pin!(timeout);

                tokio::select! {
                    res = socket.recv_from(&mut buf) => {
                        if let Ok((len, from)) = res {
                            if from == target && len >= 20 {
                                if let Some(pub_addr) = parse_stun_response(&buf[..len], &tx_id) {
                                    return Some(pub_addr);
                                }
                            }
                        }
                    }
                    _ = &mut timeout => {}
                }
            }
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
        }

        offset += attr_len;
    }

    None
}

#[cfg(windows)]
pub fn configure_socket_physical_if(socket: &UdpSocket, logger: &AppLogger) {
    use std::os::windows::io::AsRawSocket;
    if let Some(if_idx) = find_best_physical_interface_index() {
        let raw_sock = socket.as_raw_socket() as usize;
        let if_index_be: u32 = if_idx.to_be(); // network byte order for Winsock IP_UNICAST_IF
        unsafe {
            extern "system" {
                fn setsockopt(s: usize, level: i32, optname: i32, optval: *const u8, optlen: i32) -> i32;
            }
            let res = setsockopt(
                raw_sock,
                0,  /* IPPROTO_IP */
                31, /* IP_UNICAST_IF */
                &if_index_be as *const _ as *const u8,
                4,
            );
            if res == 0 {
                logger.info(&format!(
                    "UDP сокет привязан к физическому интерфейсу #{} (обход VPN)",
                    if_idx
                ));
            } else {
                logger.warn(&format!(
                    "Не удалось установить IP_UNICAST_IF для интерфейса #{}",
                    if_idx
                ));
            }
        }
    }
}

#[cfg(not(windows))]
pub fn configure_socket_physical_if(_socket: &UdpSocket, _logger: &AppLogger) {}

#[cfg(windows)]
pub fn find_best_physical_interface_index() -> Option<u32> {
    use std::ptr::null_mut;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

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

    let mut curr = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    let mut best_index: Option<u32> = None;

    while !curr.is_null() {
        unsafe {
            let adapter = &*curr;
            // Filter: must be OperStatus Up (1 = IfOperStatusUp)
            // IfType: 6 = Ethernet, 71 = 802.11 WiFi, 243/244 = WWAN / 4G Cellular
            let is_physical_type = adapter.IfType == 6
                || adapter.IfType == 71
                || adapter.IfType == 243
                || adapter.IfType == 244;
            let is_up = adapter.OperStatus == 1;

            // Check description for common VPN keywords to avoid virtual adapters
            let mut desc_str = String::new();
            if !adapter.Description.is_null() {
                let mut p = adapter.Description;
                while *p != 0 {
                    desc_str.push((*p as u8) as char);
                    p = p.add(1);
                }
            }
            let desc_lower = desc_str.to_lowercase();
            let is_vpn = desc_lower.contains("tap")
                || desc_lower.contains("tun")
                || desc_lower.contains("wireguard")
                || desc_lower.contains("wintun")
                || desc_lower.contains("v2ray")
                || desc_lower.contains("sing-box")
                || desc_lower.contains("openvpn")
                || desc_lower.contains("tailscale");

            if is_physical_type && is_up && !is_vpn {
                let if_index = adapter.Anonymous1.Anonymous.IfIndex;
                if if_index > 0 {
                    best_index = Some(if_index);
                    break;
                }
            }

            curr = adapter.Next;
        }
    }

    best_index
}

#[cfg(not(windows))]
pub fn find_best_physical_interface_index() -> Option<u32> {
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
        let idx = find_best_physical_interface_index();
        println!("Found physical interface index: {:?}", idx);
        #[cfg(windows)]
        assert!(idx.is_some(), "Should find a physical interface on Windows");
    }

    #[test]
    fn test_text_ack_packet() {
        let ack = Packet::TextAck { id: 12345 };
        let enc = ack.encode().expect("Failed to encode TextAck");
        let dec = Packet::decode(&enc).expect("Failed to decode TextAck");
        assert_eq!(dec, Packet::TextAck { id: 12345 });
    }
}
