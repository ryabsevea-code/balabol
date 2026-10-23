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
    pub relay_handles: Vec<tokio::task::AbortHandle>,
}

impl MeshSignalingSession {
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.abort_handle.abort();
        for handle in &self.relay_handles {
            handle.abort();
        }
    }
}

impl Drop for MeshSignalingSession {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct Signaling;


impl Signaling {
    /// Starts symmetric decentralized Full Mesh signaling for a room.
    /// Every peer in the room periodically broadcasts its candidates and receives candidates from all other peers.
    pub async fn start_mesh_session(
        room_code: &str,
        my_announce: MeshAnnounce,
        logger: &AppLogger,
    ) -> Result<MeshSignalingSession, String> {
        let clean_code = sanitize_room_code(room_code);
        let room_key = crate::crypto::derive_room_key(&clean_code);
        let room_tag = crate::crypto::derive_nostr_topic(&clean_code);

        logger.info(&format!(
            "Mesh сигналинг: защищенный запуск пула из {} WSS-релеев (тег: {}, шифрование: E2EE)...",
            RELAYS.len(),
            room_tag
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
        let encrypted_announce = crate::crypto::encrypt_string(&announce_json, &room_key)
            .unwrap_or(announce_json);
        let mut join_handles = Vec::new();
        let mut relay_handles = Vec::new();

        for &relay in RELAYS {
            let tx = tx.clone();
            let shutdown = shutdown.clone();
            let room_tag = room_tag.clone();
            let announce_payload = encrypted_announce.clone();
            let room_key = room_key;
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
                    let joined_at = tokio::time::Instant::now();

                    while !shutdown.load(Ordering::Relaxed) {
                        // Room creation / handshake phase: announce every 2.5s during first 12s.
                        // After initial handshake, back off to 45s to avoid flooding WSS relays.
                        let pub_interval = if joined_at.elapsed() < Duration::from_secs(12) {
                            Duration::from_millis(2500)
                        } else {
                            Duration::from_secs(45)
                        };

                        if last_pub.elapsed() >= pub_interval {
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
                                                if let Ok(raw_content) = crate::crypto::decrypt_string(content, &room_key) {
                                                    if let Ok(incoming) = serde_json::from_str::<MeshAnnounce>(&raw_content) {
                                                    if incoming.peer_id != my_id {
                                                        let should_emit = {
                                                            let mut map = last_announced.lock();
                                                            match map.get_mut(&incoming.peer_id) {
                                                                Some((last_nonce, last_time)) => {
                                                                    if *last_nonce != incoming.nonce
                                                                        || last_time.elapsed() >= Duration::from_secs(15)
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
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            });
            relay_handles.push(handle.abort_handle());
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
            relay_handles,
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
