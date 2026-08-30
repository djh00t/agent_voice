//! Envelope encryption for OAuth token bytes.
//!
//! The caller supplies associated data that identifies the provider and
//! account. It is authenticated by AES-GCM but is intentionally not stored in
//! the envelope, so moving an envelope to a different account cannot decrypt
//! it successfully.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

/// The current serialized encrypted-secret envelope version.
pub const TOKEN_CIPHER_VERSION: u8 = 1;
/// Alias for the encrypted-secret envelope version.
pub const ENCRYPTED_SECRET_VERSION: u8 = TOKEN_CIPHER_VERSION;
/// The AES-GCM nonce size in bytes.
pub const NONCE_LEN: usize = aead::NONCE_LEN;
const AES_GCM_TAG_LEN: usize = 16;

/// Errors returned by token-envelope encryption and decryption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// The supplied master key was not exactly 32 bytes.
    InvalidKeyLength { expected: usize, actual: usize },
    /// The token to encrypt was empty.
    EmptyToken,
    /// The associated-data context to authenticate was empty.
    EmptyContext,
    /// Secure randomness or AES-GCM sealing failed.
    EncryptionFailed,
    /// The envelope could not be authenticated or decoded.
    DecryptionFailed,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKeyLength { .. } => {
                formatter.write_str("master key must be exactly 32 bytes")
            }
            Self::EmptyToken => formatter.write_str("token must not be empty"),
            Self::EmptyContext => formatter.write_str("associated-data context must not be empty"),
            Self::EncryptionFailed => formatter.write_str("token encryption failed"),
            Self::DecryptionFailed => formatter.write_str("token decryption failed"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Result returned by token-envelope operations.
pub type CryptoResult<T> = Result<T, CryptoError>;

/// An authenticated, serialized OAuth token envelope.
///
/// `nonce` and `ciphertext` are URL-safe, no-padding base64. The ciphertext
/// includes the AES-GCM authentication tag appended by `ring`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncryptedSecret {
    /// Envelope format version.
    pub version: u8,
    /// URL-safe, no-padding base64 AES-GCM nonce.
    pub nonce: String,
    /// URL-safe, no-padding base64 ciphertext followed by its authentication tag.
    pub ciphertext: String,
}

impl fmt::Debug for EncryptedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedSecret")
            .field("version", &self.version)
            .field("nonce", &"<redacted>")
            .field("ciphertext", &"<redacted>")
            .finish()
    }
}

/// AES-256-GCM token envelope cipher.
pub struct TokenCipher {
    key: aead::LessSafeKey,
    random: SystemRandom,
}

impl TokenCipher {
    /// Constructs a cipher from exactly 32 master-key bytes.
    pub fn new(master_key: impl AsRef<[u8]>) -> CryptoResult<Self> {
        let master_key = master_key.as_ref();
        if master_key.len() != 32 {
            return Err(CryptoError::InvalidKeyLength {
                expected: 32,
                actual: master_key.len(),
            });
        }

        let unbound_key = aead::UnboundKey::new(&aead::AES_256_GCM, master_key).map_err(|_| {
            CryptoError::InvalidKeyLength {
                expected: 32,
                actual: master_key.len(),
            }
        })?;
        Ok(Self {
            key: aead::LessSafeKey::new(unbound_key),
            random: SystemRandom::new(),
        })
    }

    /// Alias for [`Self::new`].
    pub fn from_master_key(master_key: impl AsRef<[u8]>) -> CryptoResult<Self> {
        Self::new(master_key)
    }

    /// Encrypts non-empty token bytes and authenticates a non-empty context.
    ///
    /// The context should contain a stable provider/account identity binding,
    /// such as `provider.as_bytes()` joined to `account_id.as_bytes()` with a
    /// caller-defined separator.
    pub fn encrypt(
        &self,
        token: impl AsRef<[u8]>,
        context: impl AsRef<[u8]>,
    ) -> CryptoResult<EncryptedSecret> {
        let token = token.as_ref();
        if token.is_empty() {
            return Err(CryptoError::EmptyToken);
        }
        let context = context.as_ref();
        if context.is_empty() {
            return Err(CryptoError::EmptyContext);
        }

        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.random
            .fill(&mut nonce_bytes)
            .map_err(|_| CryptoError::EncryptionFailed)?;
        let nonce = aead::Nonce::assume_unique_for_key(nonce_bytes);

        let mut ciphertext = token.to_vec();
        self.key
            .seal_in_place_append_tag(nonce, aead::Aad::from(context), &mut ciphertext)
            .map_err(|_| CryptoError::EncryptionFailed)?;

        Ok(EncryptedSecret {
            version: TOKEN_CIPHER_VERSION,
            nonce: URL_SAFE_NO_PAD.encode(nonce_bytes),
            ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
        })
    }

    /// Authenticates and decrypts an envelope with its caller-supplied context.
    ///
    /// Every malformed, unsupported, mismatched, or unauthenticated envelope
    /// returns the same error so decryption failures disclose no payload data.
    pub fn decrypt(
        &self,
        envelope: &EncryptedSecret,
        context: impl AsRef<[u8]>,
    ) -> CryptoResult<Vec<u8>> {
        let context = context.as_ref();
        if context.is_empty() || envelope.version != TOKEN_CIPHER_VERSION {
            return Err(CryptoError::DecryptionFailed);
        }

        let nonce_bytes = URL_SAFE_NO_PAD
            .decode(&envelope.nonce)
            .map_err(|_| CryptoError::DecryptionFailed)?;
        if nonce_bytes.len() != NONCE_LEN {
            return Err(CryptoError::DecryptionFailed);
        }
        let nonce = aead::Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| CryptoError::DecryptionFailed)?;

        let mut ciphertext = URL_SAFE_NO_PAD
            .decode(&envelope.ciphertext)
            .map_err(|_| CryptoError::DecryptionFailed)?;
        if ciphertext.len() <= AES_GCM_TAG_LEN {
            return Err(CryptoError::DecryptionFailed);
        }

        let plaintext = self
            .key
            .open_in_place(nonce, aead::Aad::from(context), &mut ciphertext)
            .map_err(|_| CryptoError::DecryptionFailed)?;
        if plaintext.is_empty() {
            return Err(CryptoError::DecryptionFailed);
        }
        Ok(plaintext.to_vec())
    }
}

impl fmt::Debug for TokenCipher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenCipher")
            .field("key", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use super::{CryptoError, EncryptedSecret, TokenCipher};

    const KEY: [u8; 32] = [0x4b; 32];
    const OTHER_KEY: [u8; 32] = [0x52; 32];
    const CONTEXT: &[u8] = b"google\0owner@example.com";
    const OTHER_CONTEXT: &[u8] = b"google\0other@example.com";
    const TOKEN: &[u8] = b"refresh-token-with-sensitive-value";

    fn cipher() -> TokenCipher {
        TokenCipher::new(KEY).expect("32-byte key is accepted")
    }

    fn assert_decryption_failed(result: Result<Vec<u8>, CryptoError>) {
        assert!(matches!(result, Err(CryptoError::DecryptionFailed)));
    }

    #[test]
    fn constructor_requires_exactly_32_key_bytes() {
        for length in [0, 31, 33] {
            let key = vec![0x4b; length];
            let result = TokenCipher::new(key);
            assert!(matches!(
                result,
                Err(CryptoError::InvalidKeyLength {
                    expected: 32,
                    actual
                }) if actual == length
            ));
        }
    }

    #[test]
    fn encryption_rejects_empty_token() {
        let result = cipher().encrypt([], CONTEXT);
        assert_eq!(result, Err(CryptoError::EmptyToken));
    }

    #[test]
    fn encryption_rejects_empty_context() {
        let result = cipher().encrypt(TOKEN, []);
        assert_eq!(result, Err(CryptoError::EmptyContext));
    }

    #[test]
    fn encryption_and_decryption_round_trip_token_with_context() {
        let cipher = cipher();
        let envelope = cipher.encrypt(TOKEN, CONTEXT).expect("encrypt");

        assert_eq!(envelope.version, 1);
        assert_eq!(URL_SAFE_NO_PAD.decode(&envelope.nonce).unwrap().len(), 12);
        assert!(!envelope.ciphertext.is_empty());
        assert_eq!(cipher.decrypt(&envelope, CONTEXT).unwrap(), TOKEN);
    }

    #[test]
    fn encryptions_use_fresh_random_nonces_and_ciphertexts() {
        let cipher = cipher();
        let first = cipher.encrypt(TOKEN, CONTEXT).expect("first encrypt");
        let second = cipher.encrypt(TOKEN, CONTEXT).expect("second encrypt");

        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_eq!(cipher.decrypt(&first, CONTEXT).unwrap(), TOKEN);
        assert_eq!(cipher.decrypt(&second, CONTEXT).unwrap(), TOKEN);
    }

    #[test]
    fn decryption_rejects_wrong_key_and_context_with_generic_error() {
        let envelope = cipher().encrypt(TOKEN, CONTEXT).expect("encrypt");

        assert_decryption_failed(
            TokenCipher::new(OTHER_KEY)
                .unwrap()
                .decrypt(&envelope, CONTEXT),
        );
        assert_decryption_failed(cipher().decrypt(&envelope, OTHER_CONTEXT));
    }

    #[test]
    fn decryption_rejects_tampered_nonce_ciphertext_and_tag() {
        let cipher = cipher();
        let envelope = cipher.encrypt(TOKEN, CONTEXT).expect("encrypt");

        let mut tampered_nonce = envelope.clone();
        let mut nonce = URL_SAFE_NO_PAD.decode(&tampered_nonce.nonce).unwrap();
        nonce[0] ^= 1;
        tampered_nonce.nonce = URL_SAFE_NO_PAD.encode(nonce);
        assert_decryption_failed(cipher.decrypt(&tampered_nonce, CONTEXT));

        let mut tampered_ciphertext = envelope.clone();
        let mut ciphertext = URL_SAFE_NO_PAD
            .decode(&tampered_ciphertext.ciphertext)
            .unwrap();
        ciphertext[0] ^= 1;
        tampered_ciphertext.ciphertext = URL_SAFE_NO_PAD.encode(ciphertext);
        assert_decryption_failed(cipher.decrypt(&tampered_ciphertext, CONTEXT));

        let mut tampered_tag = envelope;
        let mut ciphertext = URL_SAFE_NO_PAD.decode(&tampered_tag.ciphertext).unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 1;
        tampered_tag.ciphertext = URL_SAFE_NO_PAD.encode(ciphertext);
        assert_decryption_failed(cipher.decrypt(&tampered_tag, CONTEXT));
    }

    #[test]
    fn decryption_rejects_malformed_base64_wrong_version_and_nonce_length() {
        let cipher = cipher();
        let envelope = cipher.encrypt(TOKEN, CONTEXT).expect("encrypt");

        let mut wrong_version = envelope.clone();
        wrong_version.version = 2;
        assert_decryption_failed(cipher.decrypt(&wrong_version, CONTEXT));

        let mut malformed_nonce = envelope.clone();
        malformed_nonce.nonce = "not*base64".to_owned();
        assert_decryption_failed(cipher.decrypt(&malformed_nonce, CONTEXT));

        let mut wrong_nonce_length = envelope.clone();
        wrong_nonce_length.nonce = URL_SAFE_NO_PAD.encode([0u8; 11]);
        assert_decryption_failed(cipher.decrypt(&wrong_nonce_length, CONTEXT));

        let mut malformed_ciphertext = envelope;
        malformed_ciphertext.ciphertext = "not*base64".to_owned();
        assert_decryption_failed(cipher.decrypt(&malformed_ciphertext, CONTEXT));
    }

    #[test]
    fn serde_representation_contains_version_and_encoded_envelope_only() {
        let envelope = cipher().encrypt(TOKEN, CONTEXT).expect("encrypt");
        let json = serde_json::to_value(&envelope).expect("serialize envelope");

        assert_eq!(json["version"], 1);
        assert_eq!(json["nonce"], envelope.nonce);
        assert_eq!(json["ciphertext"], envelope.ciphertext);
        let serialized = json.to_string();
        assert!(!serialized.contains(std::str::from_utf8(TOKEN).unwrap()));
        assert!(!serialized.contains(std::str::from_utf8(CONTEXT).unwrap()));

        let decoded: EncryptedSecret = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn debug_and_error_output_redact_sensitive_material() {
        let cipher = cipher();
        let envelope = cipher.encrypt(TOKEN, CONTEXT).expect("encrypt");
        let key_text = "4b".repeat(KEY.len());
        let ciphertext = envelope.ciphertext.clone();

        let cipher_debug = format!("{cipher:?}");
        let envelope_debug = format!("{envelope:?}");
        assert!(!cipher_debug.contains(&key_text));
        assert!(!cipher_debug.contains("KKKK"));
        assert!(!envelope_debug.contains(&ciphertext));
        assert!(!envelope_debug.contains(std::str::from_utf8(TOKEN).unwrap()));

        let wrong_key_error = TokenCipher::new(OTHER_KEY)
            .unwrap()
            .decrypt(&envelope, CONTEXT)
            .unwrap_err();
        let error_text = format!("{wrong_key_error}");
        assert_eq!(error_text, "token decryption failed");
        assert!(!error_text.contains(&key_text));
        assert!(!error_text.contains(std::str::from_utf8(TOKEN).unwrap()));
        assert!(!error_text.contains(&ciphertext));
        assert!(!error_text.contains(std::str::from_utf8(CONTEXT).unwrap()));
    }
}
