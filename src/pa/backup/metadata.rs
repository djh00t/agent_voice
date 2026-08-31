use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use ring::digest;

use crate::pa::backup::envelope::{BACKUP_FORMAT, SnapshotHeader};

/// Encryption format of the opaque SQLCipher ciphertext.
pub const ENCRYPTION_FORMAT: &str = "sqlcipher";
/// Key-management metadata for externally supplied key material.
pub const KEY_METADATA: &str = "external";
/// Versioned encryption metadata for the inner ciphertext.
pub const ENCRYPTION_METADATA: &str = "sqlcipher-v1";

/// Metadata describing one opaque encrypted snapshot.
///
/// The checksum covers only the inner SQLCipher ciphertext. The provider
/// value checksum for the complete encoded envelope remains a separate
/// boundary owned by the provider adapter.
#[derive(Clone, PartialEq, Eq)]
pub struct SnapshotMetadata {
    pub format: String,
    pub checksum: String,
    pub ciphertext_size: u64,
    pub created_at: DateTime<Utc>,
    pub object_key: String,
    pub encryption_format: String,
    pub key_metadata: String,
    pub encryption_metadata: String,
}

impl fmt::Debug for SnapshotMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SnapshotMetadata { redacted: true }")
    }
}

impl fmt::Display for SnapshotMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("snapshot metadata (redacted)")
    }
}

impl SnapshotMetadata {
    /// Converts metadata into the frozen envelope header representation.
    pub fn to_header(&self) -> SnapshotHeader {
        SnapshotHeader {
            format: self.format.clone(),
            checksum: self.checksum.clone(),
            ciphertext_size: self.ciphertext_size,
            created_at: self.created_at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
            object_key: self.object_key.clone(),
            encryption_format: self.encryption_format.clone(),
            key_metadata: self.key_metadata.clone(),
            encryption_metadata: self.encryption_metadata.clone(),
        }
    }
}

/// Fail-closed errors for snapshot metadata derivation and verification.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MetadataError {
    EmptyCiphertext,
    InvalidTimestamp,
    InvalidObjectKey,
    InvalidMetadata,
    ChecksumMismatch,
    SizeMismatch,
}

impl fmt::Debug for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyCiphertext => "MetadataError::EmptyCiphertext",
            Self::InvalidTimestamp => "MetadataError::InvalidTimestamp",
            Self::InvalidObjectKey => "MetadataError::InvalidObjectKey",
            Self::InvalidMetadata => "MetadataError::InvalidMetadata",
            Self::ChecksumMismatch => "MetadataError::ChecksumMismatch",
            Self::SizeMismatch => "MetadataError::SizeMismatch",
        })
    }
}

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyCiphertext => "snapshot metadata error: empty ciphertext",
            Self::InvalidTimestamp => "snapshot metadata error: invalid timestamp",
            Self::InvalidObjectKey => "snapshot metadata error: invalid object key",
            Self::InvalidMetadata => "snapshot metadata error: invalid metadata",
            Self::ChecksumMismatch => "snapshot metadata error: checksum mismatch",
            Self::SizeMismatch => "snapshot metadata error: size mismatch",
        })
    }
}

impl std::error::Error for MetadataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// Derives deterministic metadata for opaque SQLCipher ciphertext.
pub fn derive_metadata(
    ciphertext: &[u8],
    created_at: DateTime<Utc>,
    object_key: impl Into<String>,
) -> Result<SnapshotMetadata, MetadataError> {
    if ciphertext.is_empty() {
        return Err(MetadataError::EmptyCiphertext);
    }

    let object_key = object_key.into();
    if !valid_object_key(&object_key) {
        return Err(MetadataError::InvalidObjectKey);
    }

    let checksum = checksum(ciphertext);
    let ciphertext_size =
        u64::try_from(ciphertext.len()).map_err(|_| MetadataError::SizeMismatch)?;
    Ok(SnapshotMetadata {
        format: BACKUP_FORMAT.to_owned(),
        checksum,
        ciphertext_size,
        created_at,
        object_key,
        encryption_format: ENCRYPTION_FORMAT.to_owned(),
        key_metadata: KEY_METADATA.to_owned(),
        encryption_metadata: ENCRYPTION_METADATA.to_owned(),
    })
}

/// Verifies frozen metadata against only the supplied opaque ciphertext.
pub fn verify_header(header: &SnapshotHeader, ciphertext: &[u8]) -> Result<(), MetadataError> {
    if ciphertext.is_empty() {
        return Err(MetadataError::EmptyCiphertext);
    }

    let ciphertext_size =
        u64::try_from(ciphertext.len()).map_err(|_| MetadataError::SizeMismatch)?;
    if header.ciphertext_size != ciphertext_size {
        return Err(MetadataError::SizeMismatch);
    }
    if header.format != BACKUP_FORMAT
        || header.encryption_format != ENCRYPTION_FORMAT
        || header.key_metadata != KEY_METADATA
        || header.encryption_metadata != ENCRYPTION_METADATA
        || !valid_checksum_shape(&header.checksum)
    {
        return Err(MetadataError::InvalidMetadata);
    }
    if !valid_object_key(&header.object_key) {
        return Err(MetadataError::InvalidObjectKey);
    }
    if !valid_timestamp(&header.created_at) {
        return Err(MetadataError::InvalidTimestamp);
    }
    if header.checksum != checksum(ciphertext) {
        return Err(MetadataError::ChecksumMismatch);
    }
    Ok(())
}

fn checksum(ciphertext: &[u8]) -> String {
    digest::digest(&digest::SHA256, ciphertext)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_object_key(object_key: &str) -> bool {
    !object_key.trim().is_empty()
        && object_key.len() <= 256
        && !object_key.chars().any(char::is_control)
}

fn valid_checksum_shape(checksum: &str) -> bool {
    checksum.len() == 64
        && checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_timestamp(timestamp: &str) -> bool {
    timestamp.ends_with('Z')
        && DateTime::parse_from_rfc3339(timestamp)
            .map(|parsed| {
                parsed
                    .with_timezone(&Utc)
                    .to_rfc3339_opts(SecondsFormat::AutoSi, true)
                    == timestamp
            })
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use chrono::{TimeZone, Utc};

    use super::{
        ENCRYPTION_FORMAT, ENCRYPTION_METADATA, KEY_METADATA, MetadataError, SnapshotMetadata,
        derive_metadata, verify_header,
    };
    use crate::pa::backup::envelope::{BACKUP_FORMAT, SnapshotHeader};

    const CIPHERTEXT: &[u8] = b"fixture-sqlcipher-ciphertext-v1";
    const CREATED_AT: &str = "2026-01-02T03:04:05Z";
    const OBJECT_KEY: &str = "fixture/snapshot-001";
    const CHECKSUM: &str = "422e69d7f07ad3df305642920a8e0cb76424fab69fc0600dfaa46702b57d98d4";

    fn created_at() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
            .single()
            .expect("fixture timestamp")
    }

    fn metadata() -> SnapshotMetadata {
        derive_metadata(CIPHERTEXT, created_at(), OBJECT_KEY).expect("metadata")
    }

    fn header() -> SnapshotHeader {
        metadata().to_header()
    }

    #[test]
    fn metadata_checksum_contract() {
        let created_at = created_at();
        let metadata = metadata();
        let header = metadata.to_header();

        assert_eq!(metadata.format, BACKUP_FORMAT);
        assert_eq!(metadata.checksum, CHECKSUM);
        assert_eq!(metadata.ciphertext_size, 31);
        assert_eq!(metadata.created_at, created_at);
        assert_eq!(metadata.object_key, OBJECT_KEY);
        assert_eq!(metadata.encryption_format, ENCRYPTION_FORMAT);
        assert_eq!(metadata.key_metadata, KEY_METADATA);
        assert_eq!(metadata.encryption_metadata, ENCRYPTION_METADATA);
        assert_eq!(header.format, BACKUP_FORMAT);
        assert_eq!(header.checksum, CHECKSUM);
        assert_eq!(header.ciphertext_size, 31);
        assert_eq!(header.created_at, CREATED_AT);
        assert_eq!(header.object_key, OBJECT_KEY);
        assert_eq!(header.encryption_format, ENCRYPTION_FORMAT);
        assert_eq!(header.key_metadata, KEY_METADATA);
        assert_eq!(header.encryption_metadata, ENCRYPTION_METADATA);
    }

    #[test]
    fn metadata_derivation_is_deterministic() {
        assert_eq!(metadata(), metadata());
    }

    #[test]
    fn verifies_ciphertext_only_checksum() {
        let header = header();

        assert_eq!(verify_header(&header, CIPHERTEXT), Ok(()));

        let mut changed = CIPHERTEXT.to_vec();
        changed[0] ^= 1;
        assert_eq!(
            verify_header(&header, &changed),
            Err(MetadataError::ChecksumMismatch)
        );
    }

    #[test]
    fn rejects_changed_checksum() {
        let mut header = header();
        header.checksum.replace_range(..1, "0");

        assert_eq!(
            verify_header(&header, CIPHERTEXT),
            Err(MetadataError::ChecksumMismatch)
        );
    }

    #[test]
    fn rejects_changed_size() {
        let mut header = header();
        header.ciphertext_size += 1;

        assert_eq!(
            verify_header(&header, CIPHERTEXT),
            Err(MetadataError::SizeMismatch)
        );
    }

    #[test]
    fn rejects_changed_format_and_encryption_metadata() {
        let mut format_changed = header();
        format_changed.format = "other-format".to_owned();
        assert_eq!(
            verify_header(&format_changed, CIPHERTEXT),
            Err(MetadataError::InvalidMetadata)
        );

        let mut encryption_changed = header();
        encryption_changed.encryption_metadata = "other-version".to_owned();
        assert_eq!(
            verify_header(&encryption_changed, CIPHERTEXT),
            Err(MetadataError::InvalidMetadata)
        );
    }

    #[test]
    fn rejects_malformed_checksum() {
        let mut header = header();
        header.checksum = "not-a-checksum".to_owned();

        assert_eq!(
            verify_header(&header, CIPHERTEXT),
            Err(MetadataError::InvalidMetadata)
        );
    }

    #[test]
    fn rejects_wrong_timestamp_shape() {
        let mut offset = header();
        offset.created_at = "2026-01-02T03:04:05+00:00".to_owned();
        assert_eq!(
            verify_header(&offset, CIPHERTEXT),
            Err(MetadataError::InvalidTimestamp)
        );

        let mut fractional_zero = header();
        fractional_zero.created_at = "2026-01-02T03:04:05.000Z".to_owned();
        assert_eq!(
            verify_header(&fractional_zero, CIPHERTEXT),
            Err(MetadataError::InvalidTimestamp)
        );
    }

    #[test]
    fn rejects_invalid_object_keys() {
        assert_eq!(
            derive_metadata(CIPHERTEXT, created_at(), ""),
            Err(MetadataError::InvalidObjectKey)
        );
        assert_eq!(
            derive_metadata(CIPHERTEXT, created_at(), "   "),
            Err(MetadataError::InvalidObjectKey)
        );
        assert_eq!(
            derive_metadata(CIPHERTEXT, created_at(), "bad\nkey"),
            Err(MetadataError::InvalidObjectKey)
        );
        assert_eq!(
            derive_metadata(CIPHERTEXT, created_at(), "a".repeat(257)),
            Err(MetadataError::InvalidObjectKey)
        );

        let mut header = header();
        header.object_key = "bad\0key".to_owned();
        assert_eq!(
            verify_header(&header, CIPHERTEXT),
            Err(MetadataError::InvalidObjectKey)
        );
    }

    #[test]
    fn rejects_empty_ciphertext() {
        assert_eq!(
            derive_metadata(&[], created_at(), OBJECT_KEY),
            Err(MetadataError::EmptyCiphertext)
        );
        assert_eq!(
            verify_header(&header(), &[]),
            Err(MetadataError::EmptyCiphertext)
        );
    }

    #[test]
    fn redacts_metadata_and_errors() {
        let sentinel_key = "sentinel-object-key";
        let sentinel_ciphertext = b"sentinel-ciphertext";
        let metadata =
            derive_metadata(sentinel_ciphertext, created_at(), sentinel_key).expect("metadata");
        let debug = format!("{metadata:?}");
        let display = format!("{metadata}");

        assert_eq!(debug, "SnapshotMetadata { redacted: true }");
        assert_eq!(display, "snapshot metadata (redacted)");
        assert!(!debug.contains(sentinel_key));
        assert!(!debug.contains("sentinel-ciphertext"));
        assert!(!display.contains(sentinel_key));
        assert!(!display.contains("sentinel-ciphertext"));

        let errors = [
            (
                MetadataError::EmptyCiphertext,
                "MetadataError::EmptyCiphertext",
                "snapshot metadata error: empty ciphertext",
            ),
            (
                MetadataError::InvalidTimestamp,
                "MetadataError::InvalidTimestamp",
                "snapshot metadata error: invalid timestamp",
            ),
            (
                MetadataError::InvalidObjectKey,
                "MetadataError::InvalidObjectKey",
                "snapshot metadata error: invalid object key",
            ),
            (
                MetadataError::InvalidMetadata,
                "MetadataError::InvalidMetadata",
                "snapshot metadata error: invalid metadata",
            ),
            (
                MetadataError::ChecksumMismatch,
                "MetadataError::ChecksumMismatch",
                "snapshot metadata error: checksum mismatch",
            ),
            (
                MetadataError::SizeMismatch,
                "MetadataError::SizeMismatch",
                "snapshot metadata error: size mismatch",
            ),
        ];
        for (error, expected_debug, expected_display) in errors {
            assert_eq!(format!("{error:?}"), expected_debug);
            assert_eq!(format!("{error}"), expected_display);
            assert!(error.source().is_none());
        }
    }
}
