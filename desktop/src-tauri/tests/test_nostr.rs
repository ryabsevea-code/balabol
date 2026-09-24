use futures_util::{SinkExt, StreamExt};
use secp256k1::Keypair;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn create_nostr_event(keypair: &Keypair, kind: u64, tags: Vec<Vec<String>>, content: &str) -> serde_json::Value {
    let (xonly, _) = keypair.x_only_public_key();
    let pubkey_hex = hex::encode(xonly.to_byte_array());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

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

#[tokio::test]
async fn test_nostr_end_to_end_exchange() {
    let relay = "wss://nos.lol";
    let room_code = format!("{:06}", rand::random::<u32>() % 1_000_000);
    let room_tag = format!("balabol_test_{}", room_code);

    let offer_payload = "bala://offer/TEST_OFFER_123456";
    let answer_payload = "bala://answer/TEST_ANSWER_654321";

    // 1. Host connects and subscribes to answers
    let (mut host_ws, _) = connect_async(relay).await.expect("Host connect");
    let host_sub = json!(["REQ", "sub_ans", {"#t": [room_tag], "limit": 10}]).to_string();
    host_ws.send(WsMessage::Text(host_sub.into())).await.expect("Host sub");

    // 2. Client connects and subscribes to offers
    let (mut client_ws, _) = connect_async(relay).await.expect("Client connect");
    let client_sub = json!(["REQ", "sub_off", {"#t": [room_tag], "limit": 10}]).to_string();
    client_ws.send(WsMessage::Text(client_sub.into())).await.expect("Client sub");

    // 3. Host publishes Offer
    let mut rng = secp256k1::rand::rng();
    let host_key = Keypair::new(&mut rng);
    let client_key = Keypair::new(&mut rng);

    let offer_evt = create_nostr_event(
        &host_key,
        1,
        vec![vec!["t".into(), room_tag.clone()], vec!["type".into(), "offer".into()]],
        offer_payload
    );
    let host_pub = json!(["EVENT", offer_evt]).to_string();
    host_ws.send(WsMessage::Text(host_pub.into())).await.expect("Host pub offer");

    // 4. Client receives Offer and sends Answer
    let mut client_received_offer = None;
    for _ in 0..10 {
        if let Ok(Some(Ok(msg))) = tokio::time::timeout(std::time::Duration::from_millis(800), client_ws.next()).await {
            let text = msg.to_string();
            if text.contains(offer_payload) {
                client_received_offer = Some(offer_payload.to_string());
                break;
            }
        }
    }
    assert_eq!(client_received_offer, Some(offer_payload.to_string()), "Client must receive Offer");

    let answer_evt = create_nostr_event(
        &client_key,
        1,
        vec![vec!["t".into(), room_tag.clone()], vec!["type".into(), "answer".into()]],
        answer_payload
    );
    let client_pub = json!(["EVENT", answer_evt]).to_string();
    client_ws.send(WsMessage::Text(client_pub.into())).await.expect("Client pub answer");

    // 5. Host receives Answer
    let mut host_received_answer = None;
    for _ in 0..10 {
        if let Ok(Some(Ok(msg))) = tokio::time::timeout(std::time::Duration::from_millis(800), host_ws.next()).await {
            let text = msg.to_string();
            if text.contains(answer_payload) {
                host_received_answer = Some(answer_payload.to_string());
                break;
            }
        }
    }
    assert_eq!(host_received_answer, Some(answer_payload.to_string()), "Host must receive Answer");
    println!("Nostr end-to-end exchange successfully verified!");
}
