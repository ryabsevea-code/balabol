use crate::logger::AppLogger;
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

pub struct Signaling;

pub type MqttSignaling = Signaling; // Backward compatibility alias

impl Signaling {
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
