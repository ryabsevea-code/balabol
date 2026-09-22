use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Invalid key format or length: {0}")]
    InvalidKey(String),
    #[error("Signature verification failed")]
    SignatureVerificationFailed,
    #[error("Decryption failed (corrupt data or invalid key)")]
    DecryptionFailed,
    #[error("Hex decoding error: {0}")]
    HexError(#[from] hex::FromHexError),
    #[error("Payload too short for ciphertext")]
    PayloadTooShort,
}

/// Cryptographic identity of a Balabol node/user.
/// Uses Ed25519 for signature-based authentication without emails or passwords.
#[derive(Clone)]
pub struct Identity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl Identity {
    /// Generate a fresh random identity using cryptographically secure OS RNG.
    pub fn generate() -> Self {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
        }
    }

    /// Restore an identity from a 32-byte secret seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(seed);
        let verifying_key = signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
        }
    }

    /// Restore an identity from a 64-character hex seed string.
    pub fn from_seed_hex(hex_str: &str) -> Result<Self, CryptoError> {
        let bytes = hex::decode(hex_str)?;
        if bytes.len() != 32 {
            return Err(CryptoError::InvalidKey("Seed must be exactly 32 bytes (64 hex characters)".into()));
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes);
        Ok(Self::from_seed(&seed))
    }

    /// Export secret seed in hex format for backup.
    pub fn to_seed_hex(&self) -> String {
        hex::encode(self.signing_key.to_bytes())
    }

    /// Public key object.
    pub fn verifying_key(&self) -> VerifyingKey {
        self.verifying_key
    }

    /// Public key bytes (32 bytes).
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }

    /// Public key encoded as 64-character lowercase hex string.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public_key_bytes())
    }

    /// Canonical Balabol User ID format: `bala:<public_key_hex>`.
    pub fn user_id(&self) -> String {
        format!("bala:{}", self.public_key_hex())
    }

    /// Sign arbitrary binary data with the user's private key.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }

    /// Sign data and return 128-char hex signature string.
    pub fn sign_hex(&self, message: &[u8]) -> String {
        hex::encode(self.sign(message))
    }

    /// Verify signature for a message given a 32-byte public key.
    pub fn verify_signature(
        public_key_bytes: &[u8; 32],
        message: &[u8],
        signature_bytes: &[u8; 64],
    ) -> Result<(), CryptoError> {
        let verifying_key = VerifyingKey::from_bytes(public_key_bytes)
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
        let signature = Signature::from_bytes(signature_bytes);
        verifying_key
            .verify(message, &signature)
            .map_err(|_| CryptoError::SignatureVerificationFailed)
    }

    /// Verify signature where public key and signature are in hex format.
    pub fn verify_signature_hex(
        public_key_hex: &str,
        message: &[u8],
        signature_hex: &str,
    ) -> Result<(), CryptoError> {
        let pk_clean = public_key_hex.strip_prefix("bala:").unwrap_or(public_key_hex);
        let pk_bytes = hex::decode(pk_clean)?;
        if pk_bytes.len() != 32 {
            return Err(CryptoError::InvalidKey("Public key must be 32 bytes".into()));
        }
        let sig_bytes = hex::decode(signature_hex)?;
        if sig_bytes.len() != 64 {
            return Err(CryptoError::InvalidKey("Signature must be 64 bytes".into()));
        }

        let mut pk_arr = [0u8; 32];
        pk_arr.copy_from_slice(&pk_bytes);
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);

        Self::verify_signature(&pk_arr, message, &sig_arr)
    }
}

/// Symmetric E2EE encryption using XChaCha20-Poly1305.
/// Generates a random 24-byte nonce for every message and prepends it to the ciphertext.
pub struct E2eeCipher;

impl E2eeCipher {
    const NONCE_LEN: usize = 24;

    /// Encrypt plaintext with a 32-byte shared symmetric key.
    /// Output format: `[24 bytes nonce] + [ciphertext + 16 bytes auth tag]`.
    pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let cipher = XChaCha20Poly1305::new(key.into());
        let mut nonce_bytes = [0u8; Self::NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| CryptoError::DecryptionFailed)?;

        let mut result = Vec::with_capacity(Self::NONCE_LEN + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// Decrypt payload containing prepended 24-byte nonce.
    pub fn decrypt(key: &[u8; 32], payload: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if payload.len() < Self::NONCE_LEN + 16 {
            return Err(CryptoError::PayloadTooShort);
        }

        let (nonce_slice, ciphertext) = payload.split_at(Self::NONCE_LEN);
        let cipher = XChaCha20Poly1305::new(key.into());
        let nonce = XNonce::from_slice(nonce_slice);

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| CryptoError::DecryptionFailed)
    }
}

/// Contact entry identified by Ed25519 public key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContactCard {
    pub user_id: String,
    pub public_key_hex: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_generation_and_signatures() {
        let id = Identity::generate();
        assert!(id.user_id().starts_with("bala:"));
        assert_eq!(id.public_key_hex().len(), 64);

        let msg = b"Hello Balabol P2P Network";
        let sig = id.sign(msg);
        let pk = id.public_key_bytes();

        assert!(Identity::verify_signature(&pk, msg, &sig).is_ok());

        // Tampered message must fail
        let tampered = b"Hello Balabol P2P Network!";
        assert!(Identity::verify_signature(&pk, tampered, &sig).is_err());
    }

    #[test]
    fn test_seed_export_and_restore() {
        let id1 = Identity::generate();
        let seed_hex = id1.to_seed_hex();
        let id2 = Identity::from_seed_hex(&seed_hex).unwrap();

        assert_eq!(id1.user_id(), id2.user_id());
        assert_eq!(id1.public_key_bytes(), id2.public_key_bytes());
    }

    #[test]
    fn test_e2ee_cipher_roundtrip() {
        let key = [42u8; 32];
        let message = b"Confidential Discord voice channel state and message payload";

        let encrypted = E2eeCipher::encrypt(&key, message).unwrap();
        assert_ne!(encrypted, message);

        let decrypted = E2eeCipher::decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, message);

        // Decryption with wrong key must fail
        let wrong_key = [99u8; 32];
        assert!(E2eeCipher::decrypt(&wrong_key, &encrypted).is_err());
    }
}
