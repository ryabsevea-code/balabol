use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

const MAGIC_HEADER: &[u8; 4] = b"BALA";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const MIN_PACKET_LEN: usize = MAGIC_HEADER.len() + NONCE_LEN + TAG_LEN;

/// Derive a 256-bit symmetric session key from the room code using domain-separated SHA-256.
pub fn derive_room_key(room_code: &str) -> [u8; 32] {
    let clean = room_code.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(b"balabol_e2ee_room_key_salt_v1:");
    hasher.update(clean.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&hasher.finalize());
    key
}

/// Derive an opaque topic tag for Nostr signaling from the room code.
/// This prevents eavesdroppers on public Nostr relays from seeing or enumerating room codes.
pub fn derive_nostr_topic(room_code: &str) -> String {
    let clean = room_code.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(b"balabol_nostr_topic_salt_v1:");
    hasher.update(clean.as_bytes());
    let hex_str = hex::encode(hasher.finalize());
    format!("balabol_t_{}", &hex_str[..24])
}

/// Encrypt and authenticate a packet payload using ChaCha20-Poly1305 AEAD.
/// Wire format: [Magic: 4 bytes "BALA"] [Nonce: 12 bytes] [Ciphertext + Poly1305 Tag: N + 16 bytes]
pub fn encrypt_packet(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("Encryption error: {:?}", e))?;

    let mut out = Vec::with_capacity(MAGIC_HEADER.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC_HEADER);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt and verify authentication of an incoming UDP packet.
/// Rejects packets that fail MAC authentication or have an invalid header.
pub fn decrypt_packet(wire_bytes: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if wire_bytes.len() < MIN_PACKET_LEN {
        return Err("Packet too short for authenticated container".into());
    }

    if &wire_bytes[0..4] != MAGIC_HEADER {
        return Err("Invalid magic packet header".into());
    }

    let nonce_bytes = &wire_bytes[4..16];
    let ciphertext = &wire_bytes[16..];

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Packet authentication/decryption failed (invalid MAC or wrong room key)".into())
}

/// Encrypt a string (e.g. JSON announce payload) to Base64 ciphertext.
pub fn encrypt_string(plaintext: &str, key: &[u8; 32]) -> Result<String, String> {
    use base64::Engine;
    let enc = encrypt_packet(plaintext.as_bytes(), key)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&enc))
}

/// Decrypt Base64 ciphertext to a string.
pub fn decrypt_string(b64: &str, key: &[u8; 32]) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("Base64 decode error: {:?}", e))?;
    let decrypted = decrypt_packet(&bytes, key)?;
    String::from_utf8(decrypted).map_err(|e| format!("UTF-8 decode error: {:?}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_encryption_and_decryption() {
        let key = derive_room_key("123456");
        let payload = b"Hello, Balabol secure encrypted packet!";
        let encrypted = encrypt_packet(payload, &key).expect("encryption succeeds");

        assert_ne!(&encrypted[16..], payload);
        assert_eq!(&encrypted[0..4], b"BALA");

        let decrypted = decrypt_packet(&encrypted, &key).expect("decryption succeeds");
        assert_eq!(&decrypted, payload);
    }

    #[test]
    fn test_packet_authentication_fails_with_wrong_key() {
        let key_a = derive_room_key("123456");
        let key_b = derive_room_key("654321");
        let payload = b"Secret voice/text data";
        let encrypted = encrypt_packet(payload, &key_a).unwrap();

        let result = decrypt_packet(&encrypted, &key_b);
        assert!(result.is_err(), "Decryption must fail with wrong key");
    }

    #[test]
    fn test_packet_tamper_detection() {
        let key = derive_room_key("123456");
        let payload = b"Original data";
        let mut encrypted = encrypt_packet(payload, &key).unwrap();

        // Tamper with one byte in the ciphertext
        let len = encrypted.len();
        encrypted[len - 1] ^= 0x01;

        let result = decrypt_packet(&encrypted, &key);
        assert!(result.is_err(), "Tampered packet must fail authentication");
    }

    #[test]
    fn test_string_encryption_roundtrip() {
        let key = derive_room_key("test_room");
        let plain = r#"{"peer_id": 42, "peer_name": "Alice"}"#;
        let enc = encrypt_string(plain, &key).unwrap();
        let dec = decrypt_string(&enc, &key).unwrap();
        assert_eq!(plain, dec);
    }

    #[test]
    fn test_nostr_topic_derivation_is_opaque_and_deterministic() {
        let topic1 = derive_nostr_topic("123456");
        let topic2 = derive_nostr_topic("123456");
        let topic_other = derive_nostr_topic("123457");

        assert_eq!(topic1, topic2);
        assert_ne!(topic1, topic_other);
        assert!(topic1.starts_with("balabol_t_"));
        assert!(!topic1.contains("123456"));
    }
}
