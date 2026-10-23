use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

pub const MAGIC_HEADER: &[u8; 4] = b"BALA";
pub const SEQ_LEN: usize = 8;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
pub const MIN_PACKET_LEN: usize = MAGIC_HEADER.len() + SEQ_LEN + NONCE_LEN + TAG_LEN; // 40 bytes

/// Derive a 256-bit symmetric session key from the room code using Argon2id.
/// Argon2id provides state-of-the-art resistance against GPU/ASIC-accelerated offline
/// brute-force and dictionary attacks (Memory: 16 MiB, Iterations: 3, Parallelism: 1).
pub fn derive_room_key(room_code: &str) -> [u8; 32] {
    let clean = room_code
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    let salt = b"balabol_e2ee_argon2id_salt_v2__";

    let params = Params::new(16 * 1024, 3, 1, Some(32)).unwrap_or_else(|_| Params::default());
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = [0u8; 32];
    let _ = argon2.hash_password_into(clean.as_bytes(), salt, &mut key);
    key
}

/// Derive an opaque topic tag for Nostr signaling from the room code.
/// This prevents eavesdroppers on public Nostr relays from seeing or enumerating room codes.
pub fn derive_nostr_topic(room_code: &str) -> String {
    let clean = room_code
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(b"balabol_nostr_topic_salt_v1:");
    hasher.update(clean.as_bytes());
    let hex_str = hex::encode(hasher.finalize());
    format!("balabol_t_{}", &hex_str[..24])
}

/// RFC 4303 128-packet Anti-Replay Sliding Window.
/// Tracks packet sequence numbers and drops duplicate or delayed packets outside the window.
#[derive(Debug, Clone)]
pub struct AntiReplayWindow {
    pub highest_seq: u64,
    pub bitmap: u128,
}

impl Default for AntiReplayWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl AntiReplayWindow {
    pub fn new() -> Self {
        Self {
            highest_seq: 0,
            bitmap: 0,
        }
    }

    /// Check and update the replay window for an incoming packet sequence.
    /// Returns true if packet is accepted, false if it is a duplicate or too old.
    pub fn check_and_update(&mut self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }

        if seq > self.highest_seq {
            let diff = seq - self.highest_seq;
            if diff >= 128 {
                self.bitmap = 1;
            } else {
                self.bitmap = (self.bitmap << diff) | 1;
            }
            self.highest_seq = seq;
            true
        } else {
            let diff = self.highest_seq - seq;
            if diff >= 128 {
                // Packet too old, outside the 128-packet window
                false
            } else {
                let mask = 1u128 << diff;
                if self.bitmap & mask != 0 {
                    // Replay detected!
                    false
                } else {
                    self.bitmap |= mask;
                    true
                }
            }
        }
    }
}

/// Ephemeral X25519 Diffie-Hellman Key Exchange for Perfect Forward Secrecy (PFS).
/// Each peer generates an ephemeral keypair upon joining the room.
#[derive(Clone)]
pub struct EphemeralKeyExchange {
    secret: StaticSecret,
    public_key: PublicKey,
}

impl Default for EphemeralKeyExchange {
    fn default() -> Self {
        Self::new()
    }
}

impl EphemeralKeyExchange {
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let secret = StaticSecret::random_from_rng(&mut rng);
        let public_key = PublicKey::from(&secret);
        Self { secret, public_key }
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.public_key.to_bytes()
    }

    /// Computes shared secret with remote peer's public key
    /// and derives a pair-specific 256-bit symmetric session key with domain separation.
    pub fn compute_session_key(
        &self,
        remote_pub_bytes: &[u8; 32],
        local_peer_id: u32,
        remote_peer_id: u32,
    ) -> [u8; 32] {
        let remote_public = PublicKey::from(*remote_pub_bytes);
        let shared = self.secret.diffie_hellman(&remote_public);

        let mut hasher = Sha256::new();
        hasher.update(b"balabol_x25519_pfs_session_key_v1:");
        let (min_id, max_id) = if local_peer_id <= remote_peer_id {
            (local_peer_id, remote_peer_id)
        } else {
            (remote_peer_id, local_peer_id)
        };
        hasher.update(&min_id.to_be_bytes());
        hasher.update(&max_id.to_be_bytes());
        hasher.update(shared.as_bytes());

        let mut key = [0u8; 32];
        key.copy_from_slice(&hasher.finalize());
        key
    }
}

/// Encrypt and authenticate a packet payload using ChaCha20-Poly1305 AEAD with sequence number.
/// Wire format: [Magic: 4B "BALA"] [Seq: 8B u64] [Nonce: 12B] [Ciphertext + Poly1305 Tag: N + 16B]
pub fn encrypt_packet(plaintext: &[u8], key: &[u8; 32], seq: u64) -> Result<Vec<u8>, String> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("Encryption error: {:?}", e))?;

    let mut out = Vec::with_capacity(MAGIC_HEADER.len() + SEQ_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC_HEADER);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt and verify authentication of an incoming UDP packet.
/// Returns Ok((seq, plaintext)) if magic header matches and MAC authentication succeeds.
pub fn decrypt_packet(wire_bytes: &[u8], key: &[u8; 32]) -> Result<(u64, Vec<u8>), String> {
    if wire_bytes.len() < MIN_PACKET_LEN {
        return Err("Packet too short for authenticated container".into());
    }

    if &wire_bytes[0..4] != MAGIC_HEADER {
        return Err("Invalid magic packet header".into());
    }

    let seq = u64::from_be_bytes(wire_bytes[4..12].try_into().unwrap());
    let nonce_bytes = &wire_bytes[12..24];
    let ciphertext = &wire_bytes[24..];

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Packet authentication/decryption failed (invalid MAC or wrong key)".to_string())?;

    Ok((seq, plaintext))
}

/// Encrypt a string (e.g. JSON announce payload) to Base64 ciphertext.
pub fn encrypt_string(plaintext: &str, key: &[u8; 32]) -> Result<String, String> {
    use base64::Engine;
    let enc = encrypt_packet(plaintext.as_bytes(), key, 1)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&enc))
}

/// Decrypt Base64 ciphertext to a string.
pub fn decrypt_string(b64: &str, key: &[u8; 32]) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("Base64 decode error: {:?}", e))?;
    let (_seq, decrypted) = decrypt_packet(&bytes, key)?;
    String::from_utf8(decrypted).map_err(|e| format!("UTF-8 decode error: {:?}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argon2id_room_key_derivation() {
        let key1 = derive_room_key("123456");
        let key2 = derive_room_key("123456");
        let key_other = derive_room_key("123457");

        assert_eq!(key1, key2);
        assert_ne!(key1, key_other);
        assert_ne!(key1, [0u8; 32]);
    }

    #[test]
    fn test_packet_encryption_and_decryption_with_seq() {
        let key = derive_room_key("123456");
        let payload = b"Hello, Balabol secure encrypted packet with sequence!";
        let encrypted = encrypt_packet(payload, &key, 42).expect("encryption succeeds");

        assert_ne!(&encrypted[24..], payload);
        assert_eq!(&encrypted[0..4], b"BALA");

        let (seq, decrypted) = decrypt_packet(&encrypted, &key).expect("decryption succeeds");
        assert_eq!(seq, 42);
        assert_eq!(&decrypted, payload);
    }

    #[test]
    fn test_anti_replay_window() {
        let mut window = AntiReplayWindow::new();

        // seq 0 must be rejected
        assert!(!window.check_and_update(0));

        // In-order packets
        assert!(window.check_and_update(1));
        assert!(window.check_and_update(2));
        assert!(window.check_and_update(3));

        // Duplicate replay of seq 2 must be rejected
        assert!(!window.check_and_update(2));
        assert!(!window.check_and_update(1));

        // Jump ahead to seq 100
        assert!(window.check_and_update(100));

        // Older packet seq 50 (within 128 window of 100) must be accepted
        assert!(window.check_and_update(50));

        // Replaying seq 50 must be rejected
        assert!(!window.check_and_update(50));

        // Jump to seq 300
        assert!(window.check_and_update(300));

        // seq 50 is now too old (300 - 50 = 250 >= 128) -> must be rejected
        assert!(!window.check_and_update(50));
    }

    #[test]
    fn test_x25519_pfs_key_exchange() {
        let alice_kx = EphemeralKeyExchange::new();
        let bob_kx = EphemeralKeyExchange::new();

        let alice_pub = alice_kx.public_key_bytes();
        let bob_pub = bob_kx.public_key_bytes();

        let alice_id = 100u32;
        let bob_id = 200u32;

        let alice_session_key = alice_kx.compute_session_key(&bob_pub, alice_id, bob_id);
        let bob_session_key = bob_kx.compute_session_key(&alice_pub, bob_id, alice_id);

        assert_eq!(alice_session_key, bob_session_key);
        assert_ne!(alice_session_key, [0u8; 32]);
    }

    #[test]
    fn test_packet_authentication_fails_with_wrong_key() {
        let key_a = derive_room_key("123456");
        let key_b = derive_room_key("654321");
        let payload = b"Secret voice/text data";
        let encrypted = encrypt_packet(payload, &key_a, 1).unwrap();

        let result = decrypt_packet(&encrypted, &key_b);
        assert!(result.is_err(), "Decryption must fail with wrong key");
    }

    #[test]
    fn test_packet_tamper_detection() {
        let key = derive_room_key("123456");
        let payload = b"Original data";
        let mut encrypted = encrypt_packet(payload, &key, 5).unwrap();

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
