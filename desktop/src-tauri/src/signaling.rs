use crate::logger::AppLogger;
use crate::p2p::Packet;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use secp256k1::Keypair;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Robust pool of public, decentralized WSS Nostr relays operating on Port 443 with TLS.
/// Traffic is fully encrypted via TLS, indistinguishable from normal HTTPS traffic.
pub const RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://nostr.mom",
    "wss://relay.snort.social",
    "wss://relay.primal.net",
    "wss://nostr.oxtr.dev",
    "wss://eden.nostr.land",
    "wss://relay.nostr.bg",
];

pub struct RelaySession {
    pub outgoing_tx: tokio::sync::mpsc::Sender<Packet>,
    pub incoming_rx: tokio::sync::mpsc::Receiver<Packet>,
    pub abort_handle: tokio::task::AbortHandle,
}

impl Drop for RelaySession {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

pub struct HostSignalingSession {
    pub incoming_answers: tokio::sync::mpsc::Receiver<String>,
    pub abort_handle: tokio::task::AbortHandle,
}

impl Drop for HostSignalingSession {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MeshAnnounce {
    pub peer_id: u32,
    pub session_id: String,
    pub peer_name: String,
    pub candidates: Vec<std::net::SocketAddr>,
    pub nonce: u64,
}

pub struct MeshSignalingSession {
    pub incoming_announces: tokio::sync::mpsc::Receiver<MeshAnnounce>,
    pub shutdown: Arc<AtomicBool>,
    pub abort_handle: tokio::task::AbortHandle,
}

impl Drop for MeshSignalingSession {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.abort_handle.abort();
    }
}

pub struct Signaling;

pub type MqttSignaling = Signaling; // Backward compatibility alias

impl Signaling {
    /// Starts symmetric decentralized Full Mesh signaling for a room.
    /// Every peer in the room periodically broadcasts its candidates and receives candidates from all other peers.
    pub async fn start_mesh_session(
        room_code: &str,
        my_announce: MeshAnnounce,
        logger: &AppLogger,
    ) -> Result<MeshSignalingSession, String> {
        let clean_code = sanitize_room_code(room_code);
        let room_tag = format!("balabol_mesh_v5_{}", clean_code);

        logger.info(&format!(
            "Mesh сигналинг: параллельный запуск пула из {} WSS-релеев для комнаты '{}'...",
            RELAYS.len(),
            clean_code
        ));

        let (tx, rx) = tokio::sync::mpsc::channel::<MeshAnnounce>(64);
        let shutdown = Arc::new(AtomicBool::new(false));
        let last_announced = Arc::new(parking_lot::Mutex::new(
            std::collections::HashMap::<u32, (u64, tokio::time::Instant)>::new(),
        ));

        let keypair = {
            let mut rng = secp256k1::rand::rng();
            Keypair::new(&mut rng)
        };

        let announce_json = serde_json::to_string(&my_announce).unwrap_or_default();
        let mut join_handles = Vec::new();

        for &relay in RELAYS {
            let tx = tx.clone();
            let shutdown = shutdown.clone();
            let room_tag = room_tag.clone();
            let announce_payload = announce_json.clone();
            let keypair = keypair;
            let last_announced = last_announced.clone();
            let my_id = my_announce.peer_id;
            let logger = logger.clone();

            let handle = tokio::spawn(async move {
                let mut retry_count = 0u32;
                while !shutdown.load(Ordering::Relaxed) {
                    let connect_fut = connect_async(relay);
                    let (mut ws, _) = match tokio::time::timeout(Duration::from_secs(4), connect_fut).await {
                        Ok(Ok(pair)) => {
                            logger.info(&format!("Mesh сигналинг: подключен к WSS-релею {}", relay));
                            retry_count = 0;
                            pair
                        }
                        _ => {
                            if retry_count == 0 {
                                logger.warn(&format!("Mesh сигналинг: релей {} временно недоступен", relay));
                            }
                            retry_count = retry_count.saturating_add(1);
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            continue;
                        }
                    };

                    let sub_id = format!("sub_mesh_{}", rand::random::<u32>());
                    let sub_filter = json!(["REQ", sub_id, {
                        "#t": [room_tag],
                        "limit": 30
                    }]);
                    if ws.send(WsMessage::Text(sub_filter.to_string().into())).await.is_err() {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }

                    // Send initial announce immediately on connect
                    let initial_evt = create_signed_event(
                        &keypair,
                        1,
                        vec![
                            vec!["t".into(), room_tag.clone()],
                            vec!["type".into(), "mesh_announce".into()],
                        ],
                        &announce_payload,
                    );
                    let _ = ws.send(WsMessage::Text(json!(["EVENT", initial_evt]).to_string().into())).await;

                    let mut last_pub = tokio::time::Instant::now();
                    let mut last_ping = tokio::time::Instant::now();

                    while !shutdown.load(Ordering::Relaxed) {
                        // Periodic announce broadcast every 2.0s
                        if last_pub.elapsed() >= Duration::from_millis(2000) {
                            let evt = create_signed_event(
                                &keypair,
                                1,
                                vec![
                                    vec!["t".into(), room_tag.clone()],
                                    vec!["type".into(), "mesh_announce".into()],
                                ],
                                &announce_payload,
                            );
                            let pub_msg = json!(["EVENT", evt]).to_string();
                            if ws.send(WsMessage::Text(pub_msg.into())).await.is_err() {
                                break;
                            }
                            last_pub = tokio::time::Instant::now();
                        }

                        if last_ping.elapsed() >= Duration::from_secs(12) {
                            if ws.send(WsMessage::Ping(vec![].into())).await.is_err() {
                                break;
                            }
                            last_ping = tokio::time::Instant::now();
                        }

                        let read_fut = tokio::time::timeout(Duration::from_millis(150), ws.next());
                        if let Ok(Some(Ok(msg))) = read_fut.await {
                            if let WsMessage::Text(txt) = msg {
                                if let Ok(parsed) = serde_json::from_str::<Value>(&txt) {
                                    if let Some(arr) = parsed.as_array() {
                                        if arr.len() >= 3 && arr[0] == "EVENT" {
                                            if let Some(content) = arr[2].get("content").and_then(|c| c.as_str()) {
                                                if let Ok(incoming) = serde_json::from_str::<MeshAnnounce>(content) {
                                                    if incoming.peer_id != my_id {
                                                        let should_emit = {
                                                            let mut map = last_announced.lock();
                                                            match map.get_mut(&incoming.peer_id) {
                                                                Some((last_nonce, last_time)) => {
                                                                    if *last_nonce != incoming.nonce
                                                                        || last_time.elapsed() >= Duration::from_millis(2500)
                                                                    {
                                                                        *last_nonce = incoming.nonce;
                                                                        *last_time = tokio::time::Instant::now();
                                                                        true
                                                                    } else {
                                                                        false
                                                                    }
                                                                }
                                                                None => {
                                                                    map.insert(
                                                                        incoming.peer_id,
                                                                        (incoming.nonce, tokio::time::Instant::now()),
                                                                    );
                                                                    true
                                                                }
                                                            }
                                                        };
                                                        if should_emit {
                                                            logger.info(&format!(
                                                                "📡 [Mesh Сигналинг] Получен анонс от пира {} ('{}') через {}, кандидаты: {:?}",
                                                                incoming.peer_id,
                                                                incoming.peer_name,
                                                                relay,
                                                                incoming.candidates
                                                            ));
                                                            let _ = tx.send(incoming).await;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            });
            join_handles.push(handle);
        }

        let shutdown_clone = shutdown.clone();
        let supervisor = tokio::spawn(async move {
            futures_util::future::join_all(join_handles).await;
            shutdown_clone.store(true, Ordering::Relaxed);
        });

        Ok(MeshSignalingSession {
            incoming_announces: rx,
            shutdown,
            abort_handle: supervisor.abort_handle(),
        })
    }
    /// Starts continuous multi-guest host signaling session for Star-SFU
    pub async fn start_host_session(
        room_code: &str,
        offer_payload: &str,
        logger: &AppLogger,
    ) -> Result<HostSignalingSession, String> {
        let clean_code = sanitize_room_code(room_code);
        let room_tag = format!("balabol_v4_{}", clean_code);

        logger.info(&format!(
            "Защищенный SFU сигналинг (Хост): параллельный запуск пула из {} WSS-релеев для комнаты '{}'...",
            RELAYS.len(),
            clean_code
        ));

        let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
        let shutdown = Arc::new(AtomicBool::new(false));
        let seen_answers = Arc::new(parking_lot::Mutex::new(std::collections::HashSet::<String>::new()));

        let keypair = {
            let mut rng = secp256k1::rand::rng();
            Keypair::new(&mut rng)
        };

        let mut join_handles = Vec::new();

        for &relay in RELAYS {
            let tx = tx.clone();
            let shutdown = shutdown.clone();
            let room_tag = room_tag.clone();
            let offer_payload = offer_payload.to_string();
            let logger = logger.clone();
            let keypair = keypair;
            let seen_answers = seen_answers.clone();

            let handle = tokio::spawn(async move {
                let connect_fut = connect_async(relay);
                let Ok(Ok((mut ws, _))) = tokio::time::timeout(Duration::from_secs(4), connect_fut).await else {
                    return;
                };

                logger.info(&format!("SFU Хост подключен к WSS-релею {}, непрерывное ожидание гостей...", relay));

                let sub_id = format!("sub_sfu_{}", rand::random::<u32>());
                let sub_filter = json!(["REQ", sub_id, {
                    "#t": [room_tag],
                    "limit": 20
                }]);
                if ws.send(WsMessage::Text(sub_filter.to_string().into())).await.is_err() {
                    return;
                }

                let mut last_pub = tokio::time::Instant::now() - Duration::from_secs(10);
                let mut last_ping = tokio::time::Instant::now();

                while !shutdown.load(Ordering::Relaxed) {
                    // Periodic broadcast of Offer so new guests can find the room
                    if last_pub.elapsed() >= Duration::from_millis(2500) {
                        let evt = create_signed_event(
                            &keypair,
                            1,
                            vec![
                                vec!["t".into(), room_tag.clone()],
                                vec!["type".into(), "offer".into()],
                            ],
                            &offer_payload,
                        );
                        let pub_msg = json!(["EVENT", evt]).to_string();
                        if ws.send(WsMessage::Text(pub_msg.into())).await.is_err() {
                            break;
                        }
                        last_pub = tokio::time::Instant::now();
                    }

                    if last_ping.elapsed() >= Duration::from_secs(12) {
                        let _ = ws.send(WsMessage::Ping(vec![].into())).await;
                        last_ping = tokio::time::Instant::now();
                    }

                    let read_fut = tokio::time::timeout(Duration::from_millis(150), ws.next());
                    if let Ok(Some(Ok(msg))) = read_fut.await {
                        if let WsMessage::Text(txt) = msg {
                            if let Ok(parsed) = serde_json::from_str::<Value>(&txt) {
                                if let Some(arr) = parsed.as_array() {
                                    if arr.len() >= 3 && arr[0] == "EVENT" {
                                        if let Some(content) = arr[2].get("content").and_then(|c| c.as_str()) {
                                            if content.starts_with("bala://answer/") {
                                                let is_new = seen_answers.lock().insert(content.to_string());
                                                if is_new {
                                                    logger.info(&format!(
                                                        "SFU Хост: получен новый гость через WSS-релей {}!",
                                                        relay
                                                    ));
                                                    let _ = tx.send(content.to_string()).await;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
            join_handles.push(handle);
        }

        let shutdown_clone = shutdown.clone();
        let supervisor_task = tokio::spawn(async move {
            futures_util::future::join_all(join_handles).await;
            shutdown_clone.store(true, Ordering::Relaxed);
        });

        Ok(HostSignalingSession {
            incoming_answers: rx,
            abort_handle: supervisor_task.abort_handle(),
        })
    }
    /// Host starts room: connects in parallel to all WSS relays,
    /// publishes Offer every 2.5s, and waits for Answer from guest.
    pub async fn host_exchange(
        room_code: &str,
        offer_payload: &str,
        timeout: Duration,
        logger: &AppLogger,
    ) -> Result<String, String> {
        let clean_code = sanitize_room_code(room_code);
        let room_tag = format!("balabol_v4_{}", clean_code);

        logger.info(&format!(
            "Защищенный сигналинг (Хост): параллельный запуск пула из {} WSS-релеев (TLS/443) для комнаты '{}'...",
            RELAYS.len(),
            clean_code
        ));

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(10);
        let shutdown = Arc::new(AtomicBool::new(false));

        let keypair = {
            let mut rng = secp256k1::rand::rng();
            Keypair::new(&mut rng)
        };

        for &relay in RELAYS {
            let tx = tx.clone();
            let shutdown = shutdown.clone();
            let room_tag = room_tag.clone();
            let offer_payload = offer_payload.to_string();
            let logger = logger.clone();
            let keypair = keypair;

            tokio::spawn(async move {
                let connect_fut = connect_async(relay);
                let Ok(Ok((mut ws, _))) = tokio::time::timeout(Duration::from_secs(4), connect_fut).await else {
                    return;
                };

                logger.info(&format!("Хост подключился к WSS-релею {}, ожидаем гостя...", relay));

                // Subscribe to incoming Answer events
                let sub_id = format!("sub_h_{}", rand::random::<u32>());
                let sub_filter = json!(["REQ", sub_id, {
                    "#t": [room_tag],
                    "limit": 10
                }]);
                if ws.send(WsMessage::Text(sub_filter.to_string().into())).await.is_err() {
                    return;
                }

                let mut last_pub = tokio::time::Instant::now() - Duration::from_secs(10);
                let mut last_ping = tokio::time::Instant::now();

                while !shutdown.load(Ordering::Relaxed) {
                    // Periodic broadcast of Offer
                    if last_pub.elapsed() >= Duration::from_millis(2500) {
                        let evt = create_signed_event(
                            &keypair,
                            1,
                            vec![
                                vec!["t".into(), room_tag.clone()],
                                vec!["type".into(), "offer".into()],
                            ],
                            &offer_payload,
                        );
                        let pub_msg = json!(["EVENT", evt]).to_string();
                        if ws.send(WsMessage::Text(pub_msg.into())).await.is_err() {
                            break;
                        }
                        last_pub = tokio::time::Instant::now();
                    }

                    // Keep-alive WebSocket ping to prevent cellular CGNAT timeout
                    if last_ping.elapsed() >= Duration::from_secs(12) {
                        let _ = ws.send(WsMessage::Ping(vec![].into())).await;
                        last_ping = tokio::time::Instant::now();
                    }

                    // Read next message with small timeout
                    let read_fut = tokio::time::timeout(Duration::from_millis(150), ws.next());
                    if let Ok(Some(Ok(msg))) = read_fut.await {
                        if let WsMessage::Text(txt) = msg {
                            if let Ok(parsed) = serde_json::from_str::<Value>(&txt) {
                                if let Some(arr) = parsed.as_array() {
                                    if arr.len() >= 3 && arr[0] == "EVENT" {
                                        if let Some(content) = arr[2].get("content").and_then(|c| c.as_str()) {
                                            if content.starts_with("bala://answer/") {
                                                logger.info(&format!(
                                                    "Сигналинг: ответ (Answer) от гостя получен через WSS-релей {}!",
                                                    relay
                                                ));
                                                let _ = tx.send(content.to_string()).await;
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }

        let answer_opt = tokio::time::timeout(timeout, rx.recv()).await;
        shutdown.store(true, Ordering::SeqCst);

        match answer_opt {
            Ok(Some(answer)) => Ok(answer),
            _ => Err("Время ожидания подключения гостя в комнате истекло (таймаут)".into()),
        }
    }

    /// Client searches for host Offer across all WSS relays in parallel (Happy Eyeballs).
    /// As soon as Offer arrives on ANY relay, immediately sends Answer and returns.
    pub async fn client_exchange(
        room_code: &str,
        answer_payload: &str,
        timeout: Duration,
        logger: &AppLogger,
    ) -> Result<String, String> {
        let clean_code = sanitize_room_code(room_code);
        let room_tag = format!("balabol_v4_{}", clean_code);

        logger.info(&format!(
            "Защищенный сигналинг (Клиент): параллельный поиск комнаты '{}' на {} WSS-релеях (TLS/443)...",
            clean_code,
            RELAYS.len()
        ));

        let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, String)>(10);
        let shutdown = Arc::new(AtomicBool::new(false));

        let keypair = {
            let mut rng = secp256k1::rand::rng();
            Keypair::new(&mut rng)
        };

        for &relay in RELAYS {
            let tx = tx.clone();
            let shutdown = shutdown.clone();
            let room_tag = room_tag.clone();
            let answer_payload = answer_payload.to_string();
            let logger = logger.clone();
            let keypair = keypair;

            tokio::spawn(async move {
                let connect_fut = connect_async(relay);
                let Ok(Ok((mut ws, _))) = tokio::time::timeout(Duration::from_secs(4), connect_fut).await else {
                    return;
                };

                logger.info(&format!("Клиент подключен к WSS-релею {}, ожидаем Offer...", relay));

                let sub_id = format!("sub_c_{}", rand::random::<u32>());
                let sub_filter = json!(["REQ", sub_id, {
                    "#t": [room_tag],
                    "limit": 10
                }]);
                if ws.send(WsMessage::Text(sub_filter.to_string().into())).await.is_err() {
                    return;
                }

                let mut last_ping = tokio::time::Instant::now();

                while !shutdown.load(Ordering::Relaxed) {
                    if last_ping.elapsed() >= Duration::from_secs(12) {
                        let _ = ws.send(WsMessage::Ping(vec![].into())).await;
                        last_ping = tokio::time::Instant::now();
                    }

                    let read_fut = tokio::time::timeout(Duration::from_millis(150), ws.next());
                    if let Ok(Some(Ok(msg))) = read_fut.await {
                        if let WsMessage::Text(txt) = msg {
                            if let Ok(parsed) = serde_json::from_str::<Value>(&txt) {
                                if let Some(arr) = parsed.as_array() {
                                    if arr.len() >= 3 && arr[0] == "EVENT" {
                                        if let Some(content) = arr[2].get("content").and_then(|c| c.as_str()) {
                                            if content.starts_with("bala://offer/") {
                                                logger.info(&format!(
                                                    "Сигналинг: Offer от хоста найден на WSS-релею {}! Отправляем Answer...",
                                                    relay
                                                ));

                                                // Broadcast Answer event
                                                let ans_evt = create_signed_event(
                                                    &keypair,
                                                    1,
                                                    vec![
                                                        vec!["t".into(), room_tag.clone()],
                                                        vec!["type".into(), "answer".into()],
                                                    ],
                                                    &answer_payload,
                                                );
                                                let pub_msg = json!(["EVENT", ans_evt]).to_string();
                                                for _ in 0..2 {
                                                    let _ = ws.send(WsMessage::Text(pub_msg.clone().into())).await;
                                                    tokio::time::sleep(Duration::from_millis(30)).await;
                                                }

                                                let _ = tx.send((relay.to_string(), content.to_string())).await;
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }

        let offer_opt = tokio::time::timeout(timeout, rx.recv()).await;
        shutdown.store(true, Ordering::SeqCst);

        match offer_opt {
            Ok(Some((relay, offer))) => {
                logger.info(&format!(
                    "Сигналинг: согласование успешно завершено через WSS-релей {}",
                    relay
                ));
                Ok(offer)
            }
            _ => Err("Хост с таким кодом комнаты не найден в сети. Убедитесь, что комната создана.".into()),
        }
    }

    /// Starts a real-time Relay session across connected WSS relays for fallback communication.
    pub async fn start_relay_session(
        room_code: &str,
        my_name: &str,
        logger: &AppLogger,
    ) -> Result<RelaySession, String> {
        let clean_code = sanitize_room_code(room_code);
        let relay_tag = format!("balabol_p2p_{}", clean_code);

        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Packet>(128);
        let (in_tx, in_rx) = tokio::sync::mpsc::channel::<Packet>(128);

        let keypair = {
            let mut rng = secp256k1::rand::rng();
            Keypair::new(&mut rng)
        };
        let (my_xonly, _) = keypair.x_only_public_key();
        let my_pubkey_hex = hex::encode(my_xonly.to_byte_array());

        let logger_clone = logger.clone();
        let my_name = my_name.to_string();

        let task = tokio::spawn(async move {
            let mut writers = Vec::new();

            for &relay in &RELAYS[..5.min(RELAYS.len())] {
                let connect_fut = connect_async(relay);
                if let Ok(Ok((ws, _))) = tokio::time::timeout(Duration::from_millis(3000), connect_fut).await {
                    let (mut ws_write, mut ws_read) = ws.split();

                    let sub_id = format!("sub_r_{}", rand::random::<u32>());
                    let sub_filter = json!(["REQ", sub_id, {
                        "#t": [relay_tag],
                        "limit": 0
                    }]);
                    if ws_write.send(WsMessage::Text(sub_filter.to_string().into())).await.is_ok() {
                        let ws_write_arc = Arc::new(tokio::sync::Mutex::new(ws_write));
                        writers.push(ws_write_arc);

                        let in_tx_ws = in_tx.clone();
                        let my_pubkey_ws = my_pubkey_hex.clone();

                        tokio::spawn(async move {
                            while let Some(Ok(msg)) = ws_read.next().await {
                                if let WsMessage::Text(txt) = msg {
                                    if let Ok(parsed) = serde_json::from_str::<Value>(&txt) {
                                        if let Some(arr) = parsed.as_array() {
                                            if arr.len() >= 3 && arr[0] == "EVENT" {
                                                let sender_pubkey = arr[2]
                                                    .get("pubkey")
                                                    .and_then(|p| p.as_str())
                                                    .unwrap_or_default();
                                                if sender_pubkey != my_pubkey_ws {
                                                    if let Some(content) =
                                                        arr[2].get("content").and_then(|c| c.as_str())
                                                    {
                                                        if let Some(stripped) = content.strip_prefix("bala://pkt/") {
                                                            if let Ok(bytes) = BASE64.decode(stripped) {
                                                                if let Ok(pkt) = Packet::decode(&bytes) {
                                                                    let _ = in_tx_ws.send(pkt).await;
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        });
                    }
                }
            }

            if writers.is_empty() {
                logger_clone.error("Релей-сессия: не удалось подключиться ни к одному WSS-релею");
                return;
            }

            logger_clone.info(&format!(
                "Релей-сессия активна через {} WSS-релеев (комната '{}')",
                writers.len(),
                clean_code
            ));

            let writers_arc = Arc::new(writers);
            let mut last_ping = tokio::time::Instant::now();

            loop {
                // Keep-alive ping every 10s
                if last_ping.elapsed() >= Duration::from_secs(10) {
                    for w in writers_arc.iter() {
                        let w_clone = w.clone();
                        tokio::spawn(async move {
                            let mut lock = w_clone.lock().await;
                            let _ = lock.send(WsMessage::Ping(vec![].into())).await;
                        });
                    }
                    last_ping = tokio::time::Instant::now();
                }

                tokio::select! {
                    Some(packet) = out_rx.recv() => {
                        if let Ok(bytes) = packet.encode() {
                            let b64 = BASE64.encode(&bytes);
                            let payload = format!("bala://pkt/{}", b64);
                            let evt = create_signed_event(
                                &keypair,
                                20001,
                                vec![
                                    vec!["t".into(), relay_tag.clone()],
                                    vec!["s".into(), my_name.clone()],
                                ],
                                &payload,
                            );
                            let pub_msg = json!(["EVENT", evt]).to_string();
                            for w in writers_arc.iter() {
                                let w_clone = w.clone();
                                let msg = pub_msg.clone();
                                tokio::spawn(async move {
                                    let mut lock = w_clone.lock().await;
                                    let _ = lock.send(WsMessage::Text(msg.into())).await;
                                });
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }
        });

        Ok(RelaySession {
            outgoing_tx: out_tx,
            incoming_rx: in_rx,
            abort_handle: task.abort_handle(),
        })
    }
}

pub fn sanitize_room_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

pub fn create_signed_event(
    keypair: &Keypair,
    kind: u64,
    tags: Vec<Vec<String>>,
    content: &str,
) -> Value {
    let (xonly, _) = keypair.x_only_public_key();
    let pubkey_hex = hex::encode(xonly.to_byte_array());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let serialized = json!([0, pubkey_hex, now, kind, tags, content]).to_string();
    let id_bytes = Sha256::digest(serialized.as_bytes());
    let id_hex = hex::encode(id_bytes);

    let sig = secp256k1::schnorr::sign(&id_bytes, keypair);
    let sig_hex = hex::encode(sig.as_ref());

    json!({
        "id": id_hex,
        "pubkey": pubkey_hex,
        "created_at": now,
        "kind": kind,
        "tags": tags,
        "content": content,
        "sig": sig_hex
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_code_sanitization() {
        assert_eq!(sanitize_room_code("123-456"), "123456");
        assert_eq!(sanitize_room_code(" Room #42! "), "room42");
    }

    #[test]
    fn test_event_signing() {
        let mut rng = secp256k1::rand::rng();
        let keypair = Keypair::new(&mut rng);
        let evt = create_signed_event(
            &keypair,
            1,
            vec![vec!["t".into(), "test_tag".into()]],
            "test_content",
        );

        assert!(evt.get("id").is_some());
        assert!(evt.get("sig").is_some());
        assert_eq!(evt.get("content").unwrap(), "test_content");
    }
}
