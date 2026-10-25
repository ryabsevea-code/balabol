use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

pub const HEADER_LEN: usize = 13; // 1B packet_type + 4B sender_id + 8B seq_number
pub const TAG_LEN: usize = 16;
pub const MIN_WIRE_PACKET_LEN: usize = HEADER_LEN + TAG_LEN; // 29 bytes minimum (e.g. KeepAlive)

// Legacy aliases
pub const MAGIC_HEADER: &[u8; 4] = b"BALA";
pub const MIN_PACKET_LEN: usize = MIN_WIRE_PACKET_LEN;

/// Splits a 6-digit room code into prefix (3 digits for routing) and secret pin (3 digits for PAKE).
pub fn split_room_code(code: &str) -> (String, String) {
    let clean: String = code.chars().filter(|c| c.is_alphanumeric()).collect();
    if clean.len() >= 6 {
        let (p, s) = clean.split_at(3);
        (p.to_string(), s.to_string())
    } else if clean.len() > 1 {
        let mid = clean.len() / 2;
        let (p, s) = clean.split_at(mid);
        (p.to_string(), s.to_string())
    } else {
        (clean.clone(), clean)
    }
}

/// Derives a blind Nostr topic tag based only on prefix and hourly UTC slot.
/// This prevents public relay observers from enumerating or brute-forcing 6-digit codes.
pub fn derive_blind_nostr_topic(prefix: &str, time_slot: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"balabol_v2_route_");
    hasher.update(prefix.as_bytes());
    hasher.update(&time_slot.to_be_bytes());
    let hex_str = hex::encode(hasher.finalize());
    format!("balabol_v2_{}", &hex_str[..24])
}

/// Nostr topic derivation using current UTC hour slot for blind routing
pub fn derive_nostr_topic(room_code: &str) -> String {
    let (prefix, _) = split_room_code(room_code);
    let now_hour = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 3600)
        .unwrap_or(0);
    derive_blind_nostr_topic(&prefix, now_hour)
}

/// Derives Argon2id PAKE seed from full code and dynamic room salt.
/// Memory: 16 MiB, Iterations: 3, Parallelism: 1.
pub fn derive_pake_seed(room_code: &str, salt: &[u8; 32]) -> [u8; 32] {
    let clean: String = room_code.chars().filter(|c| c.is_alphanumeric()).collect();
    let params = Params::new(16 * 1024, 3, 1, Some(32)).unwrap_or_else(|_| Params::default());
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    let _ = argon2.hash_password_into(clean.as_bytes(), salt, &mut key);
    key
}

/// Derives a 256-bit symmetric session key using fixed salt for default rooms
pub fn derive_room_key(room_code: &str) -> [u8; 32] {
    let salt = b"balabol_e2ee_argon2id_salt_v2___"; // 32 bytes
    let mut s32 = [0u8; 32];
    s32.copy_from_slice(salt);
    derive_pake_seed(room_code, &s32)
}

/// Curve25519 Password-Authenticated Key Exchange (PAKE) helper.
/// Blinds ephemeral public points with Argon2id seed stream.
#[derive(Clone)]
pub struct PakeKeyExchange {
    secret: StaticSecret,
    public_key: PublicKey,
}

impl Default for PakeKeyExchange {
    fn default() -> Self {
        Self::new()
    }
}

impl PakeKeyExchange {
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let secret = StaticSecret::random_from_rng(&mut rng);
        let public_key = PublicKey::from(&secret);
        Self { secret, public_key }
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.public_key.to_bytes()
    }

    pub fn blinded_public_point(&self, pake_seed: &[u8; 32]) -> [u8; 32] {
        let mut mask = Sha256::new();
        mask.update(b"balabol_pake_point_mask:");
        mask.update(pake_seed);
        let mask_bytes = mask.finalize();
        let mut out = self.public_key.to_bytes();
        for i in 0..32 {
            out[i] ^= mask_bytes[i];
        }
        out
    }

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

    pub fn compute_pake_session_key(
        &self,
        remote_blinded: &[u8; 32],
        pake_seed: &[u8; 32],
        my_id: u32,
        peer_id: u32,
    ) -> [u8; 32] {
        let mut mask = Sha256::new();
        mask.update(b"balabol_pake_point_mask:");
        mask.update(pake_seed);
        let mask_bytes = mask.finalize();
        let mut unblinded = [0u8; 32];
        for i in 0..32 {
            unblinded[i] = remote_blinded[i] ^ mask_bytes[i];
        }
        let remote_public = PublicKey::from(unblinded);
        let shared = self.secret.diffie_hellman(&remote_public);

        let mut kdf = Sha256::new();
        kdf.update(b"balabol_pake_session_key_v2:");
        let (min_id, max_id) = if my_id <= peer_id {
            (my_id, peer_id)
        } else {
            (peer_id, my_id)
        };
        kdf.update(&min_id.to_be_bytes());
        kdf.update(&max_id.to_be_bytes());
        kdf.update(pake_seed);
        kdf.update(shared.as_bytes());

        let mut key = [0u8; 32];
        key.copy_from_slice(&kdf.finalize());
        key
    }

    pub fn compute_confirmation_tag(session_key: &[u8; 32], sender_id: u32) -> [u8; 16] {
        let mut h = Sha256::new();
        h.update(b"balabol_pake_confirm_tag:");
        h.update(session_key);
        h.update(&sender_id.to_be_bytes());
        let digest = h.finalize();
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&digest[0..16]);
        tag
    }
}

// Alias for EphemeralKeyExchange
pub type EphemeralKeyExchange = PakeKeyExchange;

/// RFC 6479 Sliding Anti-Replay Window (128 packets) with Two-Phase Verification.
/// Phase 1 (pre-decrypt, read-only): checks if seq is acceptable before running heavy crypto.
/// Phase 2 (post-decrypt, commit): updates last_seq and bitmap strictly after Poly1305 MAC succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntiReplayWindow {
    pub last_seq: u64,
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
            last_seq: 0,
            bitmap: 0,
        }
    }

    /// Phase 1: Pre-decryption check (read-only).
    /// Returns true if packet sequence is acceptable.
    pub fn check_seq_pre(&self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }
        if seq > self.last_seq {
            return true; // Packet is ahead (even if jump +5000), allow to Poly1305 verification
        }
        let diff = self.last_seq - seq;
        if diff >= 128 {
            return false; // Too old, outside 128-packet window
        }
        let bit = 1u128 << diff;
        (self.bitmap & bit) == 0 // Reject if already seen
    }

    /// Phase 2: Post-decryption commit.
    /// Called ONLY after Poly1305 MAC has successfully verified packet authenticity.
    pub fn commit_seq_post(&mut self, seq: u64) {
        if seq > self.last_seq {
            let diff = seq - self.last_seq;
            if diff >= 128 {
                self.bitmap = 1;
            } else {
                self.bitmap = (self.bitmap << diff) | 1;
            }
            self.last_seq = seq;
        } else {
            let diff = self.last_seq - seq;
            if diff < 128 {
                self.bitmap |= 1u128 << diff;
            }
        }
    }

    /// Single-step helper for backwards-compatibility
    pub fn check_and_update(&mut self, seq: u64) -> bool {
        if self.check_seq_pre(seq) {
            self.commit_seq_post(seq);
            true
        } else {
            false
        }
    }
}

/// Encrypt and authenticate a packet payload with 13-byte open header (passed as AAD).
/// Wire format:
/// [packet_type: 1B] [sender_id: 4B] [seq_number: 8B] [Ciphertext (ChaCha20-Poly1305)] [Poly1305 Tag: 16B]
/// Nonce (12B): [sender_id: 4B] [seq_number: 8B] (strictly unique per packet).
pub fn encrypt_wire_packet(
    packet_type: u8,
    sender_id: u32,
    seq: u64,
    payload: &[u8],
    key: &[u8; 32],
) -> Result<Vec<u8>, String> {
    let mut header = [0u8; HEADER_LEN];
    header[0] = packet_type;
    header[1..5].copy_from_slice(&sender_id.to_be_bytes());
    header[5..13].copy_from_slice(&seq.to_be_bytes());

    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[0..4].copy_from_slice(&sender_id.to_be_bytes());
    nonce_bytes[4..12].copy_from_slice(&seq.to_be_bytes());
    let nonce = Nonce::from_slice(&nonce_bytes);

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let ciphertext = cipher
        .encrypt(nonce, Payload { msg: payload, aad: &header })
        .map_err(|e| format!("Encryption error: {:?}", e))?;

    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Parse the 13-byte plaintext header from incoming wire bytes in O(1) without crypto.
pub fn parse_wire_header(wire: &[u8]) -> Result<(u8, u32, u64), String> {
    if wire.len() < HEADER_LEN {
        return Err("Packet too short for wire header".into());
    }
    let packet_type = wire[0];
    let sender_id = u32::from_be_bytes(wire[1..5].try_into().unwrap());
    let seq = u64::from_be_bytes(wire[5..13].try_into().unwrap());
    Ok((packet_type, sender_id, seq))
}

/// Decrypt and authenticate an incoming UDP packet using the 13-byte header as AAD.
pub fn decrypt_wire_packet(
    wire: &[u8],
    key: &[u8; 32],
) -> Result<(u8, u32, u64, Vec<u8>), String> {
    if wire.len() < MIN_WIRE_PACKET_LEN {
        return Err("Packet too short for authenticated container".into());
    }
    let (packet_type, sender_id, seq) = parse_wire_header(wire)?;
    let header = &wire[0..HEADER_LEN];
    let ciphertext = &wire[HEADER_LEN..];

    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[0..4].copy_from_slice(&sender_id.to_be_bytes());
    nonce_bytes[4..12].copy_from_slice(&seq.to_be_bytes());
    let nonce = Nonce::from_slice(&nonce_bytes);

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let plaintext = cipher
        .decrypt(nonce, Payload { msg: ciphertext, aad: header })
        .map_err(|_| "Poly1305 MAC authentication failed (tampered packet or wrong key)".to_string())?;

    Ok((packet_type, sender_id, seq, plaintext))
}

// Compatibility wrappers for existing code
pub fn encrypt_packet(plaintext: &[u8], key: &[u8; 32], seq: u64) -> Result<Vec<u8>, String> {
    encrypt_wire_packet(1, 0, seq, plaintext, key)
}

pub fn decrypt_packet(wire_bytes: &[u8], key: &[u8; 32]) -> Result<(u64, Vec<u8>), String> {
    let (_ptype, _sender_id, seq, plaintext) = decrypt_wire_packet(wire_bytes, key)?;
    Ok((seq, plaintext))
}

pub fn encrypt_string(plaintext: &str, key: &[u8; 32]) -> Result<String, String> {
    use base64::Engine;
    let enc = encrypt_packet(plaintext.as_bytes(), key, 1)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&enc))
}

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
    fn test_split_room_code() {
        let (p1, s1) = split_room_code("757805");
        assert_eq!(p1, "757");
        assert_eq!(s1, "805");

        let (p2, s2) = split_room_code("12-34-56");
        assert_eq!(p2, "123");
        assert_eq!(s2, "456");
    }

    #[test]
    fn test_blind_nostr_topic_derivation() {
        let topic1 = derive_blind_nostr_topic("757", 500000);
        let topic2 = derive_blind_nostr_topic("757", 500000);
        let topic_other_hour = derive_blind_nostr_topic("757", 500001);
        let topic_other_prefix = derive_blind_nostr_topic("758", 500000);

        assert_eq!(topic1, topic2);
        assert_ne!(topic1, topic_other_hour);
        assert_ne!(topic1, topic_other_prefix);
        assert!(topic1.starts_with("balabol_v2_"));
    }

    #[test]
    fn test_pake_key_exchange_with_argon2id_seed() {
        let salt = [0x5au8; 32];
        let seed_alice = derive_pake_seed("757805", &salt);
        let seed_bob = derive_pake_seed("757805", &salt);
        let seed_eve = derive_pake_seed("757806", &salt);
        assert_eq!(seed_alice, seed_bob);
        assert_ne!(seed_alice, seed_eve);

        let alice_kx = PakeKeyExchange::new();
        let bob_kx = PakeKeyExchange::new();

        let alice_blinded = alice_kx.blinded_public_point(&seed_alice);
        let bob_blinded = bob_kx.blinded_public_point(&seed_bob);

        let alice_id = 100u32;
        let bob_id = 200u32;

        let alice_key = alice_kx.compute_pake_session_key(&bob_blinded, &seed_alice, alice_id, bob_id);
        let bob_key = bob_kx.compute_pake_session_key(&alice_blinded, &seed_bob, bob_id, alice_id);

        assert_eq!(alice_key, bob_key);
        assert_ne!(alice_key, [0u8; 32]);

        // Tag confirmation
        let tag_alice = PakeKeyExchange::compute_confirmation_tag(&alice_key, alice_id);
        let tag_bob = PakeKeyExchange::compute_confirmation_tag(&bob_key, alice_id);
        assert_eq!(tag_alice, tag_bob);
    }

    #[test]
    fn test_wire_packet_encryption_and_aad_tamper_detection() {
        let key = derive_room_key("757805");
        let payload = b"Voice PCM payload 24kHz";
        let packet_type = 1u8;
        let sender_id = 42u32;
        let seq = 100u64;

        let wire = encrypt_wire_packet(packet_type, sender_id, seq, payload, &key).unwrap();
        assert_eq!(wire.len(), HEADER_LEN + payload.len() + TAG_LEN);

        // Header parsing
        let (ptype, sid, s) = parse_wire_header(&wire).unwrap();
        assert_eq!(ptype, packet_type);
        assert_eq!(sid, sender_id);
        assert_eq!(s, seq);

        // Decrypt
        let (d_ptype, d_sid, d_s, d_payload) = decrypt_wire_packet(&wire, &key).unwrap();
        assert_eq!(d_ptype, packet_type);
        assert_eq!(d_sid, sender_id);
        assert_eq!(d_s, seq);
        assert_eq!(d_payload, payload);

        // Tamper with plaintext header byte (e.g. sender_id) -> must fail MAC because header is in AAD!
        let mut tampered = wire.clone();
        tampered[2] ^= 0x01;
        assert!(decrypt_wire_packet(&tampered, &key).is_err(), "Header tampering must fail MAC");

        // Tamper with ciphertext -> must fail MAC
        let mut tampered_cipher = wire.clone();
        let last = tampered_cipher.len() - 1;
        tampered_cipher[last] ^= 0x01;
        assert!(decrypt_wire_packet(&tampered_cipher, &key).is_err(), "Ciphertext tampering must fail MAC");
    }

    #[test]
    fn test_rfc6479_anti_replay_window() {
        let mut window = AntiReplayWindow::new();

        // seq 0 must be rejected
        assert!(!window.check_seq_pre(0));

        // In-order packets
        assert!(window.check_seq_pre(1));
        window.commit_seq_post(1);
        assert!(window.check_seq_pre(2));
        window.commit_seq_post(2);
        assert!(window.check_seq_pre(3));
        window.commit_seq_post(3);

        // Duplicate replay of seq 2 must be rejected in Phase 1
        assert!(!window.check_seq_pre(2));
        assert!(!window.check_seq_pre(1));

        // Jump ahead to seq 100
        assert!(window.check_seq_pre(100));
        window.commit_seq_post(100);

        // Older packet seq 50 (within 128 window of 100: 100 - 50 = 50 < 128) must be accepted
        assert!(window.check_seq_pre(50));
        window.commit_seq_post(50);

        // Duplicate seq 50 rejected
        assert!(!window.check_seq_pre(50));

        // Large jump forward after 5-10 seconds network pause (+5000)
        assert!(window.check_seq_pre(5100));
        window.commit_seq_post(5100);

        // seq 50 is now too old (5100 - 50 = 5050 >= 128) -> rejected
        assert!(!window.check_seq_pre(50));

        // Packet 5050 is within 128 window of 5100 (5100 - 5050 = 50 < 128) -> accepted
        assert!(window.check_seq_pre(5050));
        window.commit_seq_post(5050);
        assert!(!window.check_seq_pre(5050)); // Duplicate rejected
    }

    #[test]
    fn test_string_encryption_roundtrip() {
        let key = derive_room_key("test_room");
        let plain = r#"{"peer_id": 42, "peer_name": "Alice"}"#;
        let enc = encrypt_string(plain, &key).unwrap();
        let dec = decrypt_string(&enc, &key).unwrap();
        assert_eq!(plain, dec);
    }
}
