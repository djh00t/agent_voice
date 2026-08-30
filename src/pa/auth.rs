//! HMAC authentication for internal personal-assistant requests.
//!
//! Authentication is deliberately independent of an HTTP framework. Callers
//! provide the request method, path and query, raw body, authentication header
//! values, current UNIX time, and a replay guard.

use std::collections::HashMap;
use std::fmt;

use ring::{digest, hmac};

/// The timestamp accepted on an authentication request.
pub const X_AV_TIMESTAMP: &str = "X-AV-Timestamp";
/// The nonce accepted on an authentication request.
pub const X_AV_NONCE: &str = "X-AV-Nonce";
/// The HMAC signature accepted on an authentication request.
pub const X_AV_SIGNATURE: &str = "X-AV-Signature";

/// Maximum amount of clock skew accepted for an authenticated request.
pub const MAX_CLOCK_SKEW_SECONDS: i64 = 60;
/// Amount of time a successfully consumed nonce remains reserved.
pub const REPLAY_RETENTION_SECONDS: i64 = 5 * 60;
/// Default bound for the in-memory replay guard.
pub const DEFAULT_REPLAY_CAPACITY: usize = 4_096;

/// Errors raised while signing or verifying an internal request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// The HMAC secret was empty.
    EmptySecret,
    /// A required header was not valid UTF-8.
    NonUtf8Header { header: &'static str },
    /// The timestamp header was not a signed UNIX-seconds integer.
    InvalidTimestamp,
    /// The timestamp was outside the allowed clock-skew window.
    TimestampOutsideAllowedSkew,
    /// The nonce was not 16-128 allowed ASCII bytes.
    InvalidNonce,
    /// The signature was not lowercase hexadecimal of a SHA-256 tag.
    InvalidSignatureEncoding,
    /// The decoded signature did not authenticate the canonical request.
    InvalidSignature,
    /// The nonce has already been consumed within its retention window.
    NonceReplay,
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySecret => formatter.write_str("authentication secret must not be empty"),
            Self::NonUtf8Header { header } => write!(formatter, "{header} is not valid UTF-8"),
            Self::InvalidTimestamp => formatter.write_str("authentication timestamp is invalid"),
            Self::TimestampOutsideAllowedSkew => {
                formatter.write_str("authentication timestamp is outside allowed clock skew")
            }
            Self::InvalidNonce => formatter.write_str("authentication nonce is invalid"),
            Self::InvalidSignatureEncoding => {
                formatter.write_str("authentication signature encoding is invalid")
            }
            Self::InvalidSignature => formatter.write_str("authentication signature is invalid"),
            Self::NonceReplay => formatter.write_str("authentication nonce was already used"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Result returned by request signing and verification.
pub type AuthResult<T> = Result<T, AuthError>;

/// A store that atomically checks whether a nonce is new and consumes it.
pub trait ReplayGuard {
    /// Returns `true` and consumes `nonce` when it has not been seen recently.
    fn check_and_record(&mut self, nonce: &str, now: i64) -> bool;
}

/// Bounded process-local replay protection.
#[derive(Debug, Clone)]
pub struct InMemoryReplayGuard {
    accepted: HashMap<String, i64>,
    capacity: usize,
}

impl InMemoryReplayGuard {
    /// Constructs a guard with the given maximum number of live entries.
    ///
    /// A zero capacity is normalized to one so that the guard always provides
    /// replay protection rather than silently accepting every request.
    pub fn new(capacity: usize) -> Self {
        Self {
            accepted: HashMap::new(),
            capacity: capacity.max(1),
        }
    }

    /// Returns the configured maximum number of entries.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the number of retained nonces.
    pub fn len(&self) -> usize {
        self.accepted.len()
    }

    /// Returns whether no nonces are retained.
    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty()
    }

    /// Removes entries whose five-minute retention window has elapsed.
    pub fn evict_expired(&mut self, now: i64) -> usize {
        let before = self.accepted.len();
        self.accepted
            .retain(|_, accepted_at| accepted_at.saturating_add(REPLAY_RETENTION_SECONDS) > now);
        before - self.accepted.len()
    }

    /// Checks and consumes a nonce, evicting expired entries first.
    pub fn check_and_record(&mut self, nonce: &str, now: i64) -> bool {
        self.evict_expired(now);
        if self.accepted.contains_key(nonce) || self.accepted.len() >= self.capacity {
            return false;
        }
        self.accepted.insert(nonce.to_owned(), now);
        true
    }
}

impl Default for InMemoryReplayGuard {
    fn default() -> Self {
        Self::new(DEFAULT_REPLAY_CAPACITY)
    }
}

impl ReplayGuard for InMemoryReplayGuard {
    fn check_and_record(&mut self, nonce: &str, now: i64) -> bool {
        Self::check_and_record(self, nonce, now)
    }
}

/// Builds the canonical text authenticated by the HMAC.
pub fn canonical_request(
    method: &str,
    path_and_query: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path_and_query,
        timestamp,
        nonce,
        lower_hex(digest::digest(&digest::SHA256, body).as_ref()),
    )
}

/// Signs an internal request and returns its lowercase hexadecimal HMAC.
pub fn sign_request(
    secret: impl AsRef<[u8]>,
    method: &str,
    path_and_query: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
) -> AuthResult<String> {
    let key = hmac_key(secret.as_ref())?;
    validate_nonce(nonce)?;
    let canonical = canonical_request(method, path_and_query, timestamp, nonce, body);
    Ok(lower_hex(hmac::sign(&key, canonical.as_bytes()).as_ref()))
}

/// Verifies an internal request and consumes its nonce only after HMAC success.
///
/// Header arguments accept any value that can expose raw bytes, including
/// `&str`, byte slices, and `http::HeaderValue`. This keeps malformed and
/// non-UTF-8 input distinguishable without coupling this module to HTTP.
#[allow(clippy::too_many_arguments)]
pub fn verify_request<S, T, N, G, R>(
    secret: S,
    method: &str,
    path_and_query: &str,
    timestamp_header: T,
    nonce_header: N,
    signature_header: G,
    body: &[u8],
    current_timestamp: i64,
    replay_guard: &mut R,
) -> AuthResult<()>
where
    S: AsRef<[u8]>,
    T: AsRef<[u8]>,
    N: AsRef<[u8]>,
    G: AsRef<[u8]>,
    R: ReplayGuard + ?Sized,
{
    let key = hmac_key(secret.as_ref())?;
    let timestamp_text =
        std::str::from_utf8(timestamp_header.as_ref()).map_err(|_| AuthError::NonUtf8Header {
            header: X_AV_TIMESTAMP,
        })?;
    let nonce = std::str::from_utf8(nonce_header.as_ref())
        .map_err(|_| AuthError::NonUtf8Header { header: X_AV_NONCE })?;
    let signature_text =
        std::str::from_utf8(signature_header.as_ref()).map_err(|_| AuthError::NonUtf8Header {
            header: X_AV_SIGNATURE,
        })?;

    let timestamp = timestamp_text
        .parse::<i64>()
        .map_err(|_| AuthError::InvalidTimestamp)?;
    if timestamp.abs_diff(current_timestamp) > MAX_CLOCK_SKEW_SECONDS as u64 {
        return Err(AuthError::TimestampOutsideAllowedSkew);
    }
    validate_nonce(nonce)?;

    let signature = decode_hex(signature_text).ok_or(AuthError::InvalidSignatureEncoding)?;
    if signature.len() != digest::SHA256_OUTPUT_LEN {
        return Err(AuthError::InvalidSignatureEncoding);
    }

    let canonical = canonical_request(method, path_and_query, timestamp, nonce, body);
    hmac::verify(&key, canonical.as_bytes(), &signature)
        .map_err(|_| AuthError::InvalidSignature)?;

    if !replay_guard.check_and_record(nonce, current_timestamp) {
        return Err(AuthError::NonceReplay);
    }
    Ok(())
}

fn hmac_key(secret: &[u8]) -> AuthResult<hmac::Key> {
    if secret.is_empty() {
        return Err(AuthError::EmptySecret);
    }
    Ok(hmac::Key::new(hmac::HMAC_SHA256, secret))
}

fn validate_nonce(nonce: &str) -> AuthResult<()> {
    if !(16..=128).contains(&nonce.len())
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AuthError::InvalidNonce);
    }
    Ok(())
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        decoded.push((high << 4) | low);
    }
    Some(decoded)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ring::rand::{SecureRandom, SystemRandom};

    use crate::pa::store::PaStore;

    use super::{
        AuthError, InMemoryReplayGuard, ReplayGuard, X_AV_NONCE, X_AV_SIGNATURE, X_AV_TIMESTAMP,
        canonical_request, lower_hex, sign_request, verify_request,
    };

    const SECRET: &str = "internal-request-secret";
    const METHOD: &str = "post";
    const PATH_AND_QUERY: &str = "/internal/appointments?owner=ada";
    const NOW: i64 = 1_700_000_000;
    const BODY: &[u8] = b"hello";

    fn test_nonce() -> String {
        let mut bytes = [0_u8; 16];
        SystemRandom::new()
            .fill(&mut bytes)
            .expect("system random source is available for tests");
        lower_hex(&bytes)
    }

    fn test_non_utf8_header() -> Vec<u8> {
        let mut bytes = test_nonce().into_bytes();
        bytes[0] |= 0x80;
        bytes
    }

    const DATABASE_KEY: &[u8] = b"task-4a-test-key";
    fn signed_headers() -> (String, String, String) {
        let nonce = test_nonce();
        let signature = sign_request(SECRET, METHOD, PATH_AND_QUERY, NOW, &nonce, BODY).unwrap();
        (NOW.to_string(), nonce, signature)
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_with_headers(
        method: &str,
        path_and_query: &str,
        timestamp: &[u8],
        nonce: &[u8],
        signature: &[u8],
        body: &[u8],
        now: i64,
        replay_guard: &mut InMemoryReplayGuard,
    ) -> Result<(), AuthError> {
        verify_request(
            SECRET,
            method,
            path_and_query,
            timestamp,
            nonce,
            signature,
            body,
            now,
            replay_guard,
        )
    }

    #[test]
    fn canonical_request_uses_the_required_literal() {
        let nonce = test_nonce();
        assert_eq!(
            canonical_request(METHOD, PATH_AND_QUERY, NOW, &nonce, BODY),
            format!(
                "POST\n/internal/appointments?owner=ada\n1700000000\n{nonce}\n2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
            )
        );
    }

    #[test]
    fn valid_request_round_trips() {
        let (timestamp, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();

        assert!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            )
            .is_ok()
        );
    }

    #[test]
    fn method_tampering_is_rejected() {
        let (timestamp, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();

        assert_eq!(
            verify_with_headers(
                "GET",
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::InvalidSignature)
        );
    }

    #[test]
    fn path_and_query_tampering_is_rejected() {
        let (timestamp, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();

        assert_eq!(
            verify_with_headers(
                METHOD,
                "/internal/appointments?owner=grace",
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::InvalidSignature)
        );
    }

    #[test]
    fn body_tampering_is_rejected() {
        let (timestamp, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();

        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                b"tampered",
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::InvalidSignature)
        );
    }

    #[test]
    fn stale_and_future_timestamps_are_rejected() {
        let (_, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();

        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                (NOW - 61).to_string().as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::TimestampOutsideAllowedSkew)
        );

        let mut replay_guard = InMemoryReplayGuard::default();
        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                (NOW + 61).to_string().as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::TimestampOutsideAllowedSkew)
        );
    }

    #[test]
    fn malformed_nonce_is_rejected() {
        let valid_nonce = test_nonce();
        let signature =
            sign_request(SECRET, METHOD, PATH_AND_QUERY, NOW, &valid_nonce, BODY).unwrap();
        let mut replay_guard = InMemoryReplayGuard::default();
        let mut short_nonce = valid_nonce.clone();
        let short_length = usize::from(test_nonce().as_bytes()[0] & 0x0f);
        short_nonce.truncate(short_length);
        let mut invalid_character_nonce = valid_nonce;
        invalid_character_nonce.replace_range(0..1, "!");
        let mut too_long_nonce = test_nonce();
        while too_long_nonce.len() <= 128 {
            too_long_nonce.push_str(&test_nonce());
        }

        for nonce in [short_nonce, invalid_character_nonce, too_long_nonce] {
            assert_eq!(
                verify_with_headers(
                    METHOD,
                    PATH_AND_QUERY,
                    NOW.to_string().as_bytes(),
                    nonce.as_bytes(),
                    signature.as_bytes(),
                    BODY,
                    NOW,
                    &mut replay_guard,
                ),
                Err(AuthError::InvalidNonce)
            );
        }
    }

    #[test]
    fn invalid_signature_encoding_is_rejected() {
        let (_, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();

        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                NOW.to_string().as_bytes(),
                nonce.as_bytes(),
                b"not-hex",
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::InvalidSignatureEncoding)
        );

        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                NOW.to_string().as_bytes(),
                nonce.as_bytes(),
                signature.to_ascii_uppercase().as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::InvalidSignatureEncoding)
        );
    }

    #[test]
    fn malformed_and_non_utf8_headers_are_rejected() {
        let (_, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();

        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                b"not-a-timestamp",
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::InvalidTimestamp)
        );

        let mut replay_guard = InMemoryReplayGuard::default();
        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                &test_non_utf8_header(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::NonUtf8Header {
                header: X_AV_TIMESTAMP
            })
        );

        let mut replay_guard = InMemoryReplayGuard::default();
        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                NOW.to_string().as_bytes(),
                &test_non_utf8_header(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::NonUtf8Header { header: X_AV_NONCE })
        );

        let mut replay_guard = InMemoryReplayGuard::default();
        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                NOW.to_string().as_bytes(),
                nonce.as_bytes(),
                &test_non_utf8_header(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::NonUtf8Header {
                header: X_AV_SIGNATURE
            })
        );
    }

    #[test]
    fn empty_secret_is_rejected() {
        let nonce = test_nonce();
        assert_eq!(
            sign_request("", METHOD, PATH_AND_QUERY, NOW, &nonce, BODY),
            Err(AuthError::EmptySecret)
        );
    }

    #[test]
    fn replay_is_rejected_only_after_a_valid_signature() {
        let (timestamp, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();

        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                b"00",
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::InvalidSignatureEncoding)
        );
        assert_eq!(replay_guard.len(), 0);

        assert!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            )
            .is_ok()
        );

        assert_eq!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::NonceReplay)
        );
    }

    #[test]
    fn accepted_nonces_expire_after_five_minutes() {
        let (timestamp, nonce, signature) = signed_headers();
        let mut replay_guard = InMemoryReplayGuard::default();
        assert!(
            verify_with_headers(
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            )
            .is_ok()
        );

        assert!(!replay_guard.check_and_record(&nonce, NOW + 299));
        assert_eq!(replay_guard.len(), 1);
        assert!(replay_guard.check_and_record(&nonce, NOW + 300));
        assert_eq!(replay_guard.len(), 1);
    }

    #[test]
    fn replay_guard_is_bounded() {
        let mut replay_guard = InMemoryReplayGuard::new(2);
        let first_nonce = test_nonce();
        let second_nonce = test_nonce();
        let third_nonce = test_nonce();
        assert!(replay_guard.check_and_record(&first_nonce, NOW));
        assert!(replay_guard.check_and_record(&second_nonce, NOW));
        assert!(!replay_guard.check_and_record(&third_nonce, NOW));
        assert_eq!(replay_guard.len(), 2);
    }

    #[test]
    fn header_constants_have_the_wire_names() {
        assert_eq!(X_AV_TIMESTAMP, "X-AV-Timestamp");
        assert_eq!(X_AV_NONCE, "X-AV-Nonce");
        assert_eq!(X_AV_SIGNATURE, "X-AV-Signature");
    }

    #[test]
    fn persistent_replay_guard_verifies_once_and_invalid_hmac_does_not_consume() {
        let (timestamp, nonce, signature) = signed_headers();
        let mut replay_guard = PaStore::open_in_memory(DATABASE_KEY).expect("open store");

        assert!(
            verify_request(
                SECRET,
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            )
            .is_ok()
        );
        assert_eq!(
            verify_request(
                SECRET,
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                signature.as_bytes(),
                BODY,
                NOW,
                &mut replay_guard,
            ),
            Err(AuthError::NonceReplay)
        );

        let mut invalid_guard = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let mut altered_signature = signature.clone().into_bytes();
        altered_signature[0] = if altered_signature[0] == b'0' {
            b'1'
        } else {
            b'0'
        };
        assert_eq!(altered_signature.len(), 64);
        assert!(
            altered_signature
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        );
        assert_ne!(altered_signature, signature.as_bytes());
        assert_eq!(
            verify_request(
                SECRET,
                METHOD,
                PATH_AND_QUERY,
                timestamp.as_bytes(),
                nonce.as_bytes(),
                &altered_signature,
                BODY,
                NOW,
                &mut invalid_guard,
            ),
            Err(AuthError::InvalidSignature)
        );
        let count: i64 = invalid_guard
            .connection()
            .query_row("SELECT count(*) FROM replay_nonces", [], |row| row.get(0))
            .expect("count replay rows");
        assert_eq!(count, 0);
    }

    #[test]
    fn persistent_replay_guard_fails_closed_when_storage_fails() {
        let nonce = test_nonce();
        let mut replay_guard = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        replay_guard
            .connection()
            .execute("DROP TABLE replay_nonces", [])
            .expect("drop replay table");

        assert!(!ReplayGuard::check_and_record(
            &mut replay_guard,
            &nonce,
            NOW
        ));
    }
}
