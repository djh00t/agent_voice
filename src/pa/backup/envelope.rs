//! Frozen, self-describing encrypted-backup payload values.
//!
//! The envelope is deliberately a byte/value boundary. It validates and
//! serializes only metadata and opaque ciphertext; it does not perform I/O,
//! decrypt data, or calculate checksums.

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// Exact magic prefix for version-one backup envelopes.
pub const BACKUP_MAGIC: &[u8] = b"agent_voice_backup_v1\n";

/// Exact format value carried by a version-one backup header.
pub const BACKUP_FORMAT: &str = "agent_voice_backup_v1";

/// Maximum encoded JSON header size accepted by the envelope parser.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Validated metadata carried by one encrypted backup envelope.
#[derive(Clone, PartialEq, Eq)]
pub struct SnapshotHeader {
    /// Frozen envelope format identifier.
    pub format: String,
    /// Lowercase hexadecimal checksum shape for the encrypted database bytes.
    pub checksum: String,
    /// Exact number of opaque ciphertext bytes in the payload tail.
    pub ciphertext_size: u64,
    /// Canonical UTC RFC3339 creation timestamp.
    pub created_at: String,
    /// Opaque destination object key.
    pub object_key: String,
    /// Encryption format/version identifier.
    pub encryption_format: String,
    /// Opaque key-management metadata.
    pub key_metadata: String,
    /// Opaque encryption metadata.
    pub encryption_metadata: String,
}

impl fmt::Debug for SnapshotHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotHeader")
            .field("format", &"<redacted>")
            .field("checksum", &"<redacted>")
            .field("ciphertext_size", &"<redacted>")
            .field("created_at", &"<redacted>")
            .field("object_key", &"<redacted>")
            .field("encryption_format", &"<redacted>")
            .field("key_metadata", &"<redacted>")
            .field("encryption_metadata", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for SnapshotHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SnapshotHeader(<redacted>)")
    }
}

/// One frozen backup payload containing a validated header and opaque bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct SnapshotEnvelope {
    header: SnapshotHeader,
    ciphertext: Vec<u8>,
}

impl fmt::Debug for SnapshotEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotEnvelope")
            .field("header", &self.header)
            .field("ciphertext", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for SnapshotEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SnapshotEnvelope(<redacted>)")
    }
}

/// Fail-closed errors returned by backup-envelope encoding and parsing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeError {
    /// The prefix, length field, or declared JSON header was incomplete.
    Truncated,
    /// The payload did not begin with [`BACKUP_MAGIC`].
    BadMagic,
    /// The declared JSON header exceeded [`MAX_HEADER_BYTES`].
    HeaderTooLarge,
    /// The header was not a strict eight-field JSON object.
    InvalidHeader,
    /// The header JSON did not match its canonical serialization.
    NonCanonicalHeader,
    /// A header value violated its bounded value contract.
    InvalidValue,
    /// The declared ciphertext size did not equal the opaque tail length.
    SizeMismatch,
}

impl fmt::Debug for EnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Truncated => "Truncated",
            Self::BadMagic => "BadMagic",
            Self::HeaderTooLarge => "HeaderTooLarge",
            Self::InvalidHeader => "InvalidHeader",
            Self::NonCanonicalHeader => "NonCanonicalHeader",
            Self::InvalidValue => "InvalidValue",
            Self::SizeMismatch => "SizeMismatch",
        })
    }
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Truncated => "backup envelope is truncated",
            Self::BadMagic => "backup envelope magic is invalid",
            Self::HeaderTooLarge => "backup envelope header is too large",
            Self::InvalidHeader => "backup envelope header is invalid",
            Self::NonCanonicalHeader => "backup envelope header is not canonical",
            Self::InvalidValue => "backup envelope value is invalid",
            Self::SizeMismatch => "backup envelope ciphertext size does not match",
        })
    }
}

impl std::error::Error for EnvelopeError {}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireHeader {
    format: String,
    checksum: String,
    ciphertext_size: u64,
    created_at: String,
    object_key: String,
    encryption_format: String,
    key_metadata: String,
    encryption_metadata: String,
}

impl From<WireHeader> for SnapshotHeader {
    fn from(header: WireHeader) -> Self {
        Self {
            format: header.format,
            checksum: header.checksum,
            ciphertext_size: header.ciphertext_size,
            created_at: header.created_at,
            object_key: header.object_key,
            encryption_format: header.encryption_format,
            key_metadata: header.key_metadata,
            encryption_metadata: header.encryption_metadata,
        }
    }
}

impl From<&SnapshotHeader> for WireHeader {
    fn from(header: &SnapshotHeader) -> Self {
        Self {
            format: header.format.clone(),
            checksum: header.checksum.clone(),
            ciphertext_size: header.ciphertext_size,
            created_at: header.created_at.clone(),
            object_key: header.object_key.clone(),
            encryption_format: header.encryption_format.clone(),
            key_metadata: header.key_metadata.clone(),
            encryption_metadata: header.encryption_metadata.clone(),
        }
    }
}

impl SnapshotEnvelope {
    /// Encodes one validated header and opaque ciphertext into frozen bytes.
    pub fn encode(header: &SnapshotHeader, ciphertext: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
        validate_header(header)?;
        let ciphertext_len =
            u64::try_from(ciphertext.len()).map_err(|_| EnvelopeError::SizeMismatch)?;
        if header.ciphertext_size == 0 || header.ciphertext_size != ciphertext_len {
            return Err(EnvelopeError::SizeMismatch);
        }

        let wire = WireHeader::from(header);
        let header_bytes = serde_json::to_vec(&wire).map_err(|_| EnvelopeError::InvalidHeader)?;
        let header_len =
            u32::try_from(header_bytes.len()).map_err(|_| EnvelopeError::HeaderTooLarge)?;
        if header_bytes.len() > MAX_HEADER_BYTES {
            return Err(EnvelopeError::HeaderTooLarge);
        }

        let total_len = BACKUP_MAGIC
            .len()
            .checked_add(4)
            .and_then(|len| len.checked_add(header_bytes.len()))
            .and_then(|len| len.checked_add(ciphertext.len()))
            .ok_or(EnvelopeError::SizeMismatch)?;
        let mut bytes = Vec::with_capacity(total_len);
        bytes.extend_from_slice(BACKUP_MAGIC);
        bytes.extend_from_slice(&header_len.to_be_bytes());
        bytes.extend_from_slice(&header_bytes);
        bytes.extend_from_slice(ciphertext);
        Ok(bytes)
    }

    /// Parses one complete frozen envelope without performing any I/O.
    pub fn parse(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        if bytes.len() < BACKUP_MAGIC.len() {
            return Err(EnvelopeError::Truncated);
        }
        if &bytes[..BACKUP_MAGIC.len()] != BACKUP_MAGIC {
            return Err(EnvelopeError::BadMagic);
        }

        let length_start = BACKUP_MAGIC.len();
        let length_end = length_start + 4;
        if bytes.len() < length_end {
            return Err(EnvelopeError::Truncated);
        }
        let header_len = u32::from_be_bytes(
            bytes[length_start..length_end]
                .try_into()
                .map_err(|_| EnvelopeError::Truncated)?,
        ) as usize;
        if header_len > MAX_HEADER_BYTES {
            return Err(EnvelopeError::HeaderTooLarge);
        }

        let header_end = length_end
            .checked_add(header_len)
            .ok_or(EnvelopeError::Truncated)?;
        if bytes.len() < header_end {
            return Err(EnvelopeError::Truncated);
        }
        let header_bytes = &bytes[length_end..header_end];
        let wire: WireHeader =
            serde_json::from_slice(header_bytes).map_err(|_| EnvelopeError::InvalidHeader)?;
        let canonical = serde_json::to_vec(&wire).map_err(|_| EnvelopeError::InvalidHeader)?;
        if canonical != header_bytes {
            return Err(EnvelopeError::NonCanonicalHeader);
        }

        let header = SnapshotHeader::from(wire);
        validate_header(&header)?;
        let ciphertext = &bytes[header_end..];
        let ciphertext_len =
            u64::try_from(ciphertext.len()).map_err(|_| EnvelopeError::SizeMismatch)?;
        if header.ciphertext_size != ciphertext_len {
            return Err(EnvelopeError::SizeMismatch);
        }

        Ok(Self {
            header,
            ciphertext: ciphertext.to_vec(),
        })
    }

    /// Returns the validated metadata for this envelope.
    pub fn header(&self) -> &SnapshotHeader {
        &self.header
    }

    /// Returns the opaque ciphertext tail without interpreting it.
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// Returns the number of bytes produced by [`Self::encode`].
    pub fn encoded_len(&self) -> usize {
        let header = WireHeader::from(&self.header);
        let header_len = serde_json::to_vec(&header)
            .expect("wire header serialization cannot fail")
            .len();
        BACKUP_MAGIC.len() + 4 + header_len + self.ciphertext.len()
    }
}

fn validate_header(header: &SnapshotHeader) -> Result<(), EnvelopeError> {
    for value in [
        header.format.as_str(),
        header.checksum.as_str(),
        header.created_at.as_str(),
        header.object_key.as_str(),
        header.encryption_format.as_str(),
        header.key_metadata.as_str(),
        header.encryption_metadata.as_str(),
    ] {
        if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(EnvelopeError::InvalidValue);
        }
    }
    if header.format != BACKUP_FORMAT
        || header.checksum.len() != 64
        || !header
            .checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || header.ciphertext_size == 0
        || !is_canonical_timestamp(&header.created_at)
    {
        return Err(EnvelopeError::InvalidValue);
    }
    Ok(())
}

fn is_canonical_timestamp(value: &str) -> bool {
    if !value.ends_with('Z') {
        return false;
    }
    let Ok(parsed) = DateTime::parse_from_rfc3339(value) else {
        return false;
    };
    parsed
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::AutoSi, true)
        == value
}

#[cfg(test)]
mod tests {
    use super::{
        BACKUP_FORMAT, BACKUP_MAGIC, EnvelopeError, MAX_HEADER_BYTES, SnapshotEnvelope,
        SnapshotHeader,
    };

    const CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OBJECT_KEY: &str = "backups/2026-08-30.snapshot";
    const CREATED_AT: &str = "2026-08-30T00:00:00Z";
    const CIPHERTEXT: &[u8] = &[0xde, 0xad, 0xbe, 0xef];

    fn header() -> SnapshotHeader {
        SnapshotHeader {
            format: BACKUP_FORMAT.to_owned(),
            checksum: CHECKSUM.to_owned(),
            ciphertext_size: CIPHERTEXT.len() as u64,
            created_at: CREATED_AT.to_owned(),
            object_key: OBJECT_KEY.to_owned(),
            encryption_format: "sqlcipher-v4".to_owned(),
            key_metadata: "key-id-01".to_owned(),
            encryption_metadata: "aes-256-gcm".to_owned(),
        }
    }

    fn canonical_header_json() -> String {
        format!(
            r#"{{"format":"{BACKUP_FORMAT}","checksum":"{CHECKSUM}","ciphertext_size":4,"created_at":"{CREATED_AT}","object_key":"{OBJECT_KEY}","encryption_format":"sqlcipher-v4","key_metadata":"key-id-01","encryption_metadata":"aes-256-gcm"}}"#
        )
    }

    fn raw_envelope(header_json: &str, ciphertext: &[u8]) -> Vec<u8> {
        let header_len = u32::try_from(header_json.len()).expect("test header fits u32");
        let mut bytes =
            Vec::with_capacity(BACKUP_MAGIC.len() + 4 + header_json.len() + ciphertext.len());
        bytes.extend_from_slice(BACKUP_MAGIC);
        bytes.extend_from_slice(&header_len.to_be_bytes());
        bytes.extend_from_slice(header_json.as_bytes());
        bytes.extend_from_slice(ciphertext);
        bytes
    }

    #[test]
    fn frozen_envelope_values() {
        let header = header();
        let header_json = canonical_header_json();
        let encoded = SnapshotEnvelope::encode(&header, CIPHERTEXT).expect("encode");

        let mut expected = Vec::new();
        expected.extend_from_slice(BACKUP_MAGIC);
        expected.extend_from_slice(&(header_json.len() as u32).to_be_bytes());
        expected.extend_from_slice(header_json.as_bytes());
        expected.extend_from_slice(CIPHERTEXT);
        assert_eq!(encoded, expected);

        let parsed = SnapshotEnvelope::parse(&encoded).expect("parse");
        assert_eq!(parsed.header(), &header);
        assert_eq!(parsed.ciphertext(), CIPHERTEXT);
        assert_eq!(parsed.encoded_len(), encoded.len());
        assert_eq!(
            SnapshotEnvelope::encode(parsed.header(), parsed.ciphertext()),
            Ok(encoded)
        );
    }

    #[test]
    fn rejects_bad_prefix_and_truncation() {
        assert_eq!(SnapshotEnvelope::parse(b""), Err(EnvelopeError::Truncated));
        assert_eq!(
            SnapshotEnvelope::parse(b"wrong"),
            Err(EnvelopeError::Truncated)
        );

        let mut bad_magic = raw_envelope(&canonical_header_json(), CIPHERTEXT);
        bad_magic[0] ^= 1;
        assert_eq!(
            SnapshotEnvelope::parse(&bad_magic),
            Err(EnvelopeError::BadMagic)
        );

        let mut truncated_length = BACKUP_MAGIC.to_vec();
        truncated_length.push(0);
        assert_eq!(
            SnapshotEnvelope::parse(&truncated_length),
            Err(EnvelopeError::Truncated)
        );

        let mut truncated_header = BACKUP_MAGIC.to_vec();
        truncated_header.extend_from_slice(&10u32.to_be_bytes());
        truncated_header.extend_from_slice(b"{}");
        assert_eq!(
            SnapshotEnvelope::parse(&truncated_header),
            Err(EnvelopeError::Truncated)
        );
    }

    #[test]
    fn rejects_oversized_or_ambiguous_headers() {
        let mut oversized = BACKUP_MAGIC.to_vec();
        oversized.extend_from_slice(&((MAX_HEADER_BYTES as u32) + 1).to_be_bytes());
        assert_eq!(
            SnapshotEnvelope::parse(&oversized),
            Err(EnvelopeError::HeaderTooLarge)
        );

        let unknown = canonical_header_json().replacen("}", ",\"unknown\":true}", 1);
        assert_eq!(
            SnapshotEnvelope::parse(&raw_envelope(&unknown, CIPHERTEXT)),
            Err(EnvelopeError::InvalidHeader)
        );

        let duplicate =
            canonical_header_json().replacen("}", ",\"format\":\"agent_voice_backup_v1\"}", 1);
        assert_eq!(
            SnapshotEnvelope::parse(&raw_envelope(&duplicate, CIPHERTEXT)),
            Err(EnvelopeError::InvalidHeader)
        );

        let whitespace = canonical_header_json().replacen('{', "{ ", 1);
        assert_eq!(
            SnapshotEnvelope::parse(&raw_envelope(&whitespace, CIPHERTEXT)),
            Err(EnvelopeError::NonCanonicalHeader)
        );

        let reordered = format!(
            r#"{{"checksum":"{CHECKSUM}","format":"{BACKUP_FORMAT}","ciphertext_size":4,"created_at":"{CREATED_AT}","object_key":"{OBJECT_KEY}","encryption_format":"sqlcipher-v4","key_metadata":"key-id-01","encryption_metadata":"aes-256-gcm"}}"#
        );
        assert_eq!(
            SnapshotEnvelope::parse(&raw_envelope(&reordered, CIPHERTEXT)),
            Err(EnvelopeError::NonCanonicalHeader)
        );
    }

    #[test]
    fn rejects_invalid_values_and_size_mismatch() {
        let cases: [(&str, String, EnvelopeError); 4] = [
            ("format", "other".to_owned(), EnvelopeError::InvalidValue),
            ("checksum", "A".repeat(64), EnvelopeError::InvalidValue),
            (
                "created_at",
                "2026-08-30T00:00:00+00:00".to_owned(),
                EnvelopeError::InvalidValue,
            ),
            ("object_key", "   ".to_owned(), EnvelopeError::InvalidValue),
        ];
        for (field, value, expected) in cases {
            let json = canonical_header_json().replace(
                match field {
                    "format" => BACKUP_FORMAT,
                    "checksum" => CHECKSUM,
                    "created_at" => CREATED_AT,
                    "object_key" => OBJECT_KEY,
                    _ => unreachable!(),
                },
                &value,
            );
            assert_eq!(
                SnapshotEnvelope::parse(&raw_envelope(&json, CIPHERTEXT)),
                Err(expected),
                "field {field}"
            );
        }

        let zero_size =
            canonical_header_json().replace("\"ciphertext_size\":4", "\"ciphertext_size\":0");
        assert_eq!(
            SnapshotEnvelope::parse(&raw_envelope(&zero_size, CIPHERTEXT)),
            Err(EnvelopeError::InvalidValue)
        );

        let short_tail = raw_envelope(&canonical_header_json(), &CIPHERTEXT[..3]);
        assert_eq!(
            SnapshotEnvelope::parse(&short_tail),
            Err(EnvelopeError::SizeMismatch)
        );

        let long_tail = raw_envelope(&canonical_header_json(), &[0xde, 0xad, 0xbe, 0xef, 0x00]);
        assert_eq!(
            SnapshotEnvelope::parse(&long_tail),
            Err(EnvelopeError::SizeMismatch)
        );

        let encode_mismatch = SnapshotEnvelope::encode(&header(), &[0xde, 0xad, 0xbe]);
        assert_eq!(encode_mismatch, Err(EnvelopeError::SizeMismatch));
    }

    #[test]
    fn accepts_unicode_with_byte_bounds_and_rejects_controls_or_overflow() {
        let mut unicode = header();
        unicode.object_key = "backups/東京.snapshot".to_owned();
        unicode.key_metadata = "鍵-id".to_owned();
        let encoded = SnapshotEnvelope::encode(&unicode, CIPHERTEXT).expect("unicode encode");
        assert_eq!(
            SnapshotEnvelope::parse(&encoded)
                .expect("unicode parse")
                .header(),
            &unicode
        );

        macro_rules! assert_control_rejected {
            ($field:ident) => {{
                let mut invalid = unicode.clone();
                invalid.$field.push('\u{0000}');
                assert_eq!(
                    SnapshotEnvelope::encode(&invalid, CIPHERTEXT),
                    Err(EnvelopeError::InvalidValue)
                );
            }};
        }
        assert_control_rejected!(format);
        assert_control_rejected!(checksum);
        assert_control_rejected!(created_at);
        assert_control_rejected!(object_key);
        assert_control_rejected!(encryption_format);
        assert_control_rejected!(key_metadata);
        assert_control_rejected!(encryption_metadata);

        unicode.object_key = "é".repeat(128);
        assert_eq!(unicode.object_key.len(), 256);
        SnapshotEnvelope::encode(&unicode, CIPHERTEXT).expect("256 UTF-8 bytes accepted");
        unicode.object_key.push('é');
        assert_eq!(
            SnapshotEnvelope::encode(&unicode, CIPHERTEXT),
            Err(EnvelopeError::InvalidValue)
        );
    }

    #[test]
    fn redacts_values_in_debug_and_errors() {
        let encoded = SnapshotEnvelope::encode(&header(), CIPHERTEXT).expect("encode");
        let envelope = SnapshotEnvelope::parse(&encoded).expect("parse");
        let debug = format!("{envelope:?}");
        for sentinel in [OBJECT_KEY, CHECKSUM, "key-id-01", "deadbeef"] {
            assert!(!debug.contains(sentinel), "debug leaked {sentinel}");
        }
        assert!(debug.contains("<redacted>"));
        assert_eq!(
            format!("{}", envelope.header()),
            "SnapshotHeader(<redacted>)"
        );
        assert_eq!(format!("{envelope}"), "SnapshotEnvelope(<redacted>)");

        let error = SnapshotEnvelope::parse(b"bad").expect_err("bad magic");
        assert_eq!(format!("{error:?}"), "Truncated");
        assert_eq!(format!("{error}"), "backup envelope is truncated");
        assert!(!format!("{error}").contains(OBJECT_KEY));
    }
}
