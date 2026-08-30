//! Dependency-free contracts shared by personal-assistant provider adapters.
//!
//! These values deliberately contain only validated identifiers, timestamps,
//! cursors, and closed error categories. Provider response text must not cross
//! this boundary.

use std::{fmt, future::Future, pin::Pin};

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use ring::digest;

use crate::pa::availability::BusyInterval;
use crate::pa::domain::TaskKind;

/// Maximum UTF-8 byte length accepted for provider identifiers and cursors.
pub const MAX_PROVIDER_ID_LENGTH: usize = 256;

/// Maximum UTF-8 byte length accepted for a mail address.
pub const MAX_MAIL_ADDRESS_LENGTH: usize = 320;

/// Maximum UTF-8 byte length accepted for mail subjects and operation keys.
pub const MAX_MAIL_TEXT_LENGTH: usize = MAX_PROVIDER_ID_LENGTH;

/// Maximum UTF-8 byte length accepted for calendar titles.
pub const MAX_CALENDAR_TITLE_LENGTH: usize = MAX_PROVIDER_ID_LENGTH;

/// Maximum UTF-8 byte length accepted for a structured triage task title.
pub const MAX_TRIAGE_TASK_TITLE_LENGTH: usize = MAX_PROVIDER_ID_LENGTH;

/// Largest permitted explicit structured triage duration, in minutes.
pub const MAX_TRIAGE_DURATION_MINUTES: u16 = 24 * 60;

/// Maximum UTF-8 byte length accepted for encrypted backup metadata.
pub const MAX_BACKUP_METADATA_LENGTH: usize = MAX_PROVIDER_ID_LENGTH;

/// Maximum UTF-8 byte length accepted for IANA timezone text.
pub const MAX_TIMEZONE_LENGTH: usize = MAX_PROVIDER_ID_LENGTH;

/// Maximum number of messages returned by one mail sync call.
pub const MAX_MAIL_SYNC_LIMIT: usize = 100;

/// Alias for the bounded mail page size used by provider adapters.
pub const MAX_PROVIDER_PAGE_LIMIT: usize = MAX_MAIL_SYNC_LIMIT;

/// Maximum number of calendar changes returned by one sync call.
pub const MAX_CALENDAR_SYNC_LIMIT: usize = MAX_PROVIDER_PAGE_LIMIT;

/// The finite set of input fields validated at the provider boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderInputField {
    /// Provider account identifier.
    AccountId,
    /// Provider access token.
    AccessToken,
    /// Source collection identifier.
    SourceId,
    /// Item identifier within a source collection.
    ItemId,
    /// Continuation cursor for the next page.
    NextCursor,
    /// UTC time range.
    TimeRange,
    /// Retry delay requested by a throttled provider.
    RetryAfter,
    /// A mail address.
    EmailAddress,
    /// A mail subject.
    Subject,
    /// An idempotent operation key.
    OperationKey,
    /// A mail label.
    Label,
    /// A requested page size.
    Limit,
    /// An incoming message timestamp.
    ReceivedAt,
    /// An outgoing message timestamp.
    SentAt,
    /// A calendar event title.
    Title,
    /// An explicit extracted task duration.
    TaskDuration,
    /// An extracted UTC task due timestamp.
    DueAt,
    /// A provider calendar event identifier.
    EventId,
    /// An IANA timezone identifier.
    Timezone,
    /// A calendar event's last-modified timestamp.
    LastModifiedAt,
    /// A calendar change timestamp.
    ChangedAt,
    /// An attendee RSVP value.
    AttendeeRsvp,
    /// An encrypted backup object key.
    BackupObjectKey,
    /// Encrypted backup ciphertext bytes or size.
    Ciphertext,
    /// An encrypted backup integrity checksum.
    Checksum,
    /// Encryption format or key metadata.
    EncryptionMetadata,
    /// A provider version or ETag.
    ProviderVersion,
    /// An encrypted backup upload timestamp.
    UploadedAt,
    /// The stored encrypted byte count.
    StoredByteCount,
}

impl fmt::Display for ProviderInputField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field = match self {
            Self::AccountId => "account_id",
            Self::AccessToken => "access_token",
            Self::SourceId => "source_id",
            Self::ItemId => "item_id",
            Self::NextCursor => "next_cursor",
            Self::TimeRange => "time_range",
            Self::RetryAfter => "retry_after",
            Self::EmailAddress => "email_address",
            Self::Subject => "subject",
            Self::OperationKey => "operation_key",
            Self::Label => "label",
            Self::Limit => "limit",
            Self::ReceivedAt => "received_at",
            Self::SentAt => "sent_at",
            Self::Title => "title",
            Self::TaskDuration => "task_duration",
            Self::DueAt => "due_at",
            Self::EventId => "event_id",
            Self::Timezone => "timezone",
            Self::LastModifiedAt => "last_modified_at",
            Self::ChangedAt => "changed_at",
            Self::AttendeeRsvp => "attendee_rsvp",
            Self::BackupObjectKey => "backup_object_key",
            Self::Ciphertext => "ciphertext",
            Self::Checksum => "checksum",
            Self::EncryptionMetadata => "encryption_metadata",
            Self::ProviderVersion => "provider_version",
            Self::UploadedAt => "uploaded_at",
            Self::StoredByteCount => "stored_byte_count",
        };
        formatter.write_str(field)
    }
}

/// A strictly positive provider-request retry delay.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RetryAfter(Duration);

impl RetryAfter {
    /// Constructs a retry delay, rejecting zero and negative durations.
    pub fn new(duration: Duration) -> ProviderResult<Self> {
        if duration <= Duration::zero() {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::RetryAfter,
            });
        }
        Ok(Self(duration))
    }

    /// Returns the validated retry delay.
    pub const fn duration(self) -> Duration {
        self.0
    }

    /// Alias for [`Self::duration`].
    pub const fn as_duration(self) -> Duration {
        self.duration()
    }
}

impl fmt::Debug for RetryAfter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RetryAfter(<redacted>)")
    }
}

/// Errors exposed by provider adapters.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProviderError {
    /// A caller supplied a blank or over-sized boundary value.
    InvalidInput { field: ProviderInputField },
    /// The provider access token is no longer usable.
    TokenExpired,
    /// The provider asked the caller to retry after a positive delay.
    Throttled { retry_after: RetryAfter },
    /// The provider's incremental cursor can no longer be used.
    CursorExpired,
    /// The requested provider item does not exist.
    NotFound,
    /// The provider rejected the operation because of a conflicting state.
    Conflict,
    /// The provider is currently unavailable.
    Unavailable,
}

impl ProviderError {
    /// Constructs a throttling error from a validated retry delay.
    pub const fn throttled(retry_after: RetryAfter) -> Self {
        Self::Throttled { retry_after }
    }

    /// Returns the retry delay for a throttling error.
    pub const fn retry_after(&self) -> Option<RetryAfter> {
        match self {
            Self::Throttled { retry_after } => Some(*retry_after),
            _ => None,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { .. } => formatter.write_str("provider input is invalid"),
            Self::TokenExpired => formatter.write_str("provider token expired"),
            Self::Throttled { .. } => formatter.write_str("provider request throttled"),
            Self::CursorExpired => formatter.write_str("provider cursor expired"),
            Self::NotFound => formatter.write_str("provider item not found"),
            Self::Conflict => formatter.write_str("provider request conflicted"),
            Self::Unavailable => formatter.write_str("provider unavailable"),
        }
    }
}

impl fmt::Debug for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field } => formatter
                .debug_struct("InvalidInput")
                .field("field", field)
                .finish(),
            Self::Throttled { .. } => formatter
                .debug_struct("Throttled")
                .field("retry_after", &"<redacted>")
                .finish(),
            Self::TokenExpired => formatter.write_str("TokenExpired"),
            Self::CursorExpired => formatter.write_str("CursorExpired"),
            Self::NotFound => formatter.write_str("NotFound"),
            Self::Conflict => formatter.write_str("Conflict"),
            Self::Unavailable => formatter.write_str("Unavailable"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// Result returned by provider boundary constructors.
pub type ProviderResult<T> = Result<T, ProviderError>;

/// A provider operation future that can be returned from an object-safe trait.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>;

/// An already-encrypted database snapshot ready for provider upload.
#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedSnapshot {
    object_key: String,
    ciphertext: Vec<u8>,
    checksum: String,
    ciphertext_size: u64,
    encryption_format: String,
    key_metadata: String,
    encryption_metadata: String,
}

impl EncryptedSnapshot {
    /// Constructs an encrypted snapshot without accepting or retaining plaintext.
    pub fn new(
        object_key: impl Into<String>,
        ciphertext: Vec<u8>,
        checksum: impl Into<String>,
        ciphertext_size: u64,
        encryption_format: impl Into<String>,
        key_metadata: impl Into<String>,
        encryption_metadata: impl Into<String>,
    ) -> ProviderResult<Self> {
        if ciphertext.is_empty()
            || ciphertext_size == 0
            || ciphertext_size != ciphertext.len() as u64
        {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::Ciphertext,
            });
        }
        let checksum = validate_sha256_checksum(checksum.into())?;
        if checksum != lowercase_hex(digest::digest(&digest::SHA256, &ciphertext).as_ref()) {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::Checksum,
            });
        }
        Ok(Self {
            object_key: validate_backup_text(
                object_key.into(),
                ProviderInputField::BackupObjectKey,
            )?,
            ciphertext,
            checksum,
            ciphertext_size,
            encryption_format: validate_backup_text(
                encryption_format.into(),
                ProviderInputField::EncryptionMetadata,
            )?,
            key_metadata: validate_backup_text(
                key_metadata.into(),
                ProviderInputField::EncryptionMetadata,
            )?,
            encryption_metadata: validate_backup_text(
                encryption_metadata.into(),
                ProviderInputField::EncryptionMetadata,
            )?,
        })
    }

    /// Returns the validated destination key for this encrypted object.
    pub fn object_key(&self) -> &str {
        &self.object_key
    }

    /// Returns the already-encrypted bytes for the explicitly initiated upload.
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// Returns the canonical SHA-256 checksum of the ciphertext.
    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    /// Returns the positive encrypted byte count, independent of plaintext size.
    pub const fn ciphertext_size(&self) -> u64 {
        self.ciphertext_size
    }

    /// Returns the validated encryption format/version.
    pub fn encryption_format(&self) -> &str {
        &self.encryption_format
    }

    /// Returns validated key metadata for the upload adapter.
    pub fn key_metadata(&self) -> &str {
        &self.key_metadata
    }

    /// Returns validated encryption metadata for the upload adapter.
    pub fn encryption_metadata(&self) -> &str {
        &self.encryption_metadata
    }
}

impl fmt::Debug for EncryptedSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedSnapshot")
            .field("object_key", &"<redacted>")
            .field("ciphertext", &"<redacted>")
            .field("checksum", &"<redacted>")
            .field("ciphertext_size", &"<redacted>")
            .field("encryption_format", &"<redacted>")
            .field("key_metadata", &"<redacted>")
            .field("encryption_metadata", &"<redacted>")
            .finish()
    }
}

/// Provider metadata confirming one encrypted snapshot upload.
#[derive(Clone, PartialEq, Eq)]
pub struct BackupReceipt {
    object_key: String,
    provider_version: String,
    checksum: String,
    uploaded_at: DateTime<Utc>,
    stored_byte_count: u64,
}

impl BackupReceipt {
    /// Constructs validated metadata for one successfully stored encrypted object.
    pub fn new(
        object_key: impl Into<String>,
        provider_version: impl Into<String>,
        checksum: impl Into<String>,
        uploaded_at: DateTime<Utc>,
        stored_byte_count: u64,
    ) -> ProviderResult<Self> {
        if stored_byte_count == 0 {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::StoredByteCount,
            });
        }
        Ok(Self {
            object_key: validate_backup_text(
                object_key.into(),
                ProviderInputField::BackupObjectKey,
            )?,
            provider_version: validate_backup_text(
                provider_version.into(),
                ProviderInputField::ProviderVersion,
            )?,
            checksum: validate_sha256_checksum(checksum.into())?,
            uploaded_at: validate_timestamp(uploaded_at, ProviderInputField::UploadedAt)?,
            stored_byte_count,
        })
    }

    /// Returns the validated destination key.
    pub fn object_key(&self) -> &str {
        &self.object_key
    }

    /// Returns the provider version or ETag.
    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }

    /// Returns the canonical SHA-256 checksum of the stored ciphertext.
    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    /// Returns the UTC time at which the provider stored the object.
    pub fn uploaded_at(&self) -> DateTime<Utc> {
        self.uploaded_at
    }

    /// Returns the positive encrypted byte count stored by the provider.
    pub const fn stored_byte_count(&self) -> u64 {
        self.stored_byte_count
    }
}

impl fmt::Debug for BackupReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BackupReceipt")
            .field("object_key", &"<redacted>")
            .field("provider_version", &"<redacted>")
            .field("checksum", &"<redacted>")
            .field("uploaded_at", &self.uploaded_at)
            .field("stored_byte_count", &"<redacted>")
            .finish()
    }
}

/// Capability for storing one already-encrypted snapshot in S3-compatible storage.
pub trait EncryptedS3BackupProvider: Send + Sync {
    /// Uploads exactly one typed encrypted snapshot.
    fn put_snapshot<'a>(
        &'a self,
        session: &'a ProviderSession,
        snapshot: &'a EncryptedSnapshot,
    ) -> ProviderFuture<'a, BackupReceipt>;
}

fn validate_backup_text(value: String, field: ProviderInputField) -> ProviderResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_BACKUP_METADATA_LENGTH
        || value.chars().any(char::is_control)
    {
        return Err(ProviderError::InvalidInput { field });
    }
    Ok(value)
}

fn validate_sha256_checksum(value: String) -> ProviderResult<String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ProviderError::InvalidInput {
            field: ProviderInputField::Checksum,
        });
    }
    Ok(value)
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

/// Credentials for one validated provider account.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderSession {
    account_id: String,
    access_token: String,
    expires_at: Option<DateTime<Utc>>,
}

impl ProviderSession {
    /// Constructs a session from a non-empty account ID and access token.
    pub fn new(
        account_id: impl Into<String>,
        access_token: impl Into<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> ProviderResult<Self> {
        let account_id = validate_identifier(account_id.into(), ProviderInputField::AccountId)?;
        let access_token = access_token.into();
        if access_token.trim().is_empty() || access_token.chars().any(char::is_control) {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::AccessToken,
            });
        }
        Ok(Self {
            account_id,
            access_token,
            expires_at,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        account_id: impl Into<String>,
        access_token: impl Into<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> ProviderResult<Self> {
        Self::new(account_id, access_token, expires_at)
    }

    /// Returns the validated account identifier.
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Returns the access token for an explicitly initiated provider call.
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// Returns the optional UTC access-token expiry.
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
    }
}

impl fmt::Debug for ProviderSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderSession")
            .field("account_id", &self.account_id)
            .field("access_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// An ordered UTC interval used in provider calendar operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

impl TimeRange {
    /// Constructs a range with a strictly earlier start.
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> ProviderResult<Self> {
        let start = validate_timestamp(start, ProviderInputField::TimeRange)?;
        let end = validate_timestamp(end, ProviderInputField::TimeRange)?;
        if start >= end {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::TimeRange,
            });
        }
        Ok(Self { start, end })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(start: DateTime<Utc>, end: DateTime<Utc>) -> ProviderResult<Self> {
        Self::new(start, end)
    }

    /// Returns the inclusive start instant in UTC.
    pub fn start(&self) -> DateTime<Utc> {
        self.start
    }

    /// Returns the exclusive end instant in UTC.
    pub fn end(&self) -> DateTime<Utc> {
        self.end
    }

    /// Alias for [`Self::start`].
    pub fn starts_at(&self) -> DateTime<Utc> {
        self.start()
    }

    /// Alias for [`Self::end`].
    pub fn ends_at(&self) -> DateTime<Utc> {
        self.end()
    }
}

/// A provider item that failed while a page was otherwise read successfully.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderItemFailure {
    source_id: String,
    item_id: String,
    error: ProviderError,
}

impl ProviderItemFailure {
    /// Constructs a failure with validated source and item identifiers.
    pub fn new(
        source_id: impl Into<String>,
        item_id: impl Into<String>,
        error: ProviderError,
    ) -> ProviderResult<Self> {
        Ok(Self {
            source_id: validate_identifier(source_id.into(), ProviderInputField::SourceId)?,
            item_id: validate_identifier(item_id.into(), ProviderInputField::ItemId)?,
            error,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        source_id: impl Into<String>,
        item_id: impl Into<String>,
        error: ProviderError,
    ) -> ProviderResult<Self> {
        Self::new(source_id, item_id, error)
    }

    /// Returns the source identifier.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Returns the item identifier.
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    /// Returns the closed provider error category.
    pub const fn error(&self) -> ProviderError {
        self.error
    }

    /// Alias for [`Self::error`].
    pub const fn provider_error(&self) -> ProviderError {
        self.error()
    }
}

impl fmt::Debug for ProviderItemFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderItemFailure")
            .field("source_id", &"<redacted>")
            .field("item_id", &"<redacted>")
            .field("error", &self.error)
            .finish()
    }
}

/// One ordered page of provider items, including item-level failures.
#[derive(Clone, PartialEq, Eq)]
pub struct SyncPage<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
    item_failures: Vec<ProviderItemFailure>,
}

impl<T> SyncPage<T> {
    /// Constructs a page and validates its optional continuation cursor.
    pub fn new(
        items: Vec<T>,
        next_cursor: Option<String>,
        item_failures: Vec<ProviderItemFailure>,
    ) -> ProviderResult<Self> {
        let next_cursor = next_cursor
            .map(|cursor| validate_identifier(cursor, ProviderInputField::NextCursor))
            .transpose()?;
        Ok(Self {
            items,
            next_cursor,
            item_failures,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        items: Vec<T>,
        next_cursor: Option<String>,
        item_failures: Vec<ProviderItemFailure>,
    ) -> ProviderResult<Self> {
        Self::new(items, next_cursor, item_failures)
    }

    /// Returns items in provider response order.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Returns the validated continuation cursor, when another page exists.
    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }

    /// Returns item failures in provider response order.
    pub fn item_failures(&self) -> &[ProviderItemFailure] {
        &self.item_failures
    }

    /// Alias for [`Self::item_failures`].
    pub fn failures(&self) -> &[ProviderItemFailure] {
        self.item_failures()
    }
}

impl<T> fmt::Debug for SyncPage<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncPage")
            .field("item_count", &self.items.len())
            .field("has_next_cursor", &self.next_cursor.is_some())
            .field("failure_count", &self.item_failures.len())
            .finish()
    }
}

/// The closed set of RSVP states accepted at a calendar provider boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rsvp {
    /// The attendee has not responded.
    NeedsAction,
    /// The attendee accepted the event.
    Accepted,
    /// The attendee declined the event.
    Declined,
    /// The attendee tentatively accepted the event.
    Tentative,
}

impl Rsvp {
    /// Returns whether this RSVP is an explicit acceptance.
    pub const fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
    }

    /// Returns whether this RSVP still needs an attendee response.
    pub const fn is_needs_action(self) -> bool {
        matches!(self, Self::NeedsAction)
    }

    /// Alias for [`Self::is_needs_action`].
    pub const fn needs_action(self) -> bool {
        self.is_needs_action()
    }
}

/// A calendar attendee with a validated address and closed RSVP state.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CalendarAttendee {
    address: MailAddress,
    rsvp: Rsvp,
}

impl CalendarAttendee {
    /// Constructs an attendee from a validated address and RSVP.
    pub fn new(address: MailAddress, rsvp: Rsvp) -> ProviderResult<Self> {
        Ok(Self { address, rsvp })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(address: MailAddress, rsvp: Rsvp) -> ProviderResult<Self> {
        Self::new(address, rsvp)
    }

    /// Constructs an attendee that has not yet responded.
    pub fn needs_action(address: MailAddress) -> Self {
        Self {
            address,
            rsvp: Rsvp::NeedsAction,
        }
    }

    /// Returns the validated attendee address.
    pub fn address(&self) -> &MailAddress {
        &self.address
    }

    /// Alias for [`Self::address`].
    pub fn mail_address(&self) -> &MailAddress {
        self.address()
    }

    /// Returns the attendee RSVP.
    pub const fn rsvp(&self) -> Rsvp {
        self.rsvp
    }

    /// Alias for [`Self::rsvp`].
    pub const fn status(&self) -> Rsvp {
        self.rsvp()
    }
}

impl From<MailAddress> for CalendarAttendee {
    fn from(address: MailAddress) -> Self {
        Self::needs_action(address)
    }
}

impl fmt::Debug for CalendarAttendee {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalendarAttendee")
            .field("address", &"<redacted>")
            .field("rsvp", &self.rsvp)
            .finish()
    }
}

/// A validated provider calendar event identifier.
///
/// Provider adapters receive this type for destructive proposal operations so
/// an unchecked event ID cannot cross the capability boundary.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProviderEventId(String);

impl ProviderEventId {
    /// Constructs a validated provider event identifier.
    pub fn new(value: impl Into<String>) -> ProviderResult<Self> {
        Ok(Self(validate_identifier(
            value.into(),
            ProviderInputField::EventId,
        )?))
    }

    /// Alias for [`Self::new`].
    pub fn try_new(value: impl Into<String>) -> ProviderResult<Self> {
        Self::new(value)
    }

    /// Returns the validated provider event identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Alias for [`Self::as_str`].
    pub fn event_id(&self) -> &str {
        self.as_str()
    }
}

/// Alias emphasizing that this is a calendar event identity.
pub type CalendarEventId = ProviderEventId;

impl fmt::Debug for ProviderEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderEventId(<redacted>)")
    }
}

impl PartialEq<str> for ProviderEventId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

/// A validated UTC timestamp used by a calendar deletion tombstone.
///
/// The private payload prevents a public `CalendarChange::Deleted` variant
/// from accepting an unchecked timestamp through direct construction.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CalendarChangedAt(DateTime<Utc>);

impl CalendarChangedAt {
    /// Constructs a changed-at value after validating its UTC representation.
    pub fn new(value: DateTime<Utc>) -> ProviderResult<Self> {
        Ok(Self(validate_timestamp(
            value,
            ProviderInputField::ChangedAt,
        )?))
    }

    /// Alias for [`Self::new`].
    pub fn try_new(value: DateTime<Utc>) -> ProviderResult<Self> {
        Self::new(value)
    }

    /// Returns the validated UTC timestamp.
    pub fn as_datetime(self) -> DateTime<Utc> {
        self.0
    }
}

impl fmt::Debug for CalendarChangedAt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Accepted attendee inputs for a Google proposal's exact owner-only list.
///
/// This conversion wrapper lets callers provide one owner or a collection
/// while the proposal constructor still enforces the one-attendee invariant.
#[derive(Clone, PartialEq, Eq)]
pub struct CalendarProposalAttendees(Vec<CalendarAttendee>);

impl CalendarProposalAttendees {
    /// Returns the supplied attendees in their original order.
    pub fn as_slice(&self) -> &[CalendarAttendee] {
        &self.0
    }
}

impl From<CalendarAttendee> for CalendarProposalAttendees {
    fn from(attendee: CalendarAttendee) -> Self {
        Self(vec![attendee])
    }
}

impl From<MailAddress> for CalendarProposalAttendees {
    fn from(address: MailAddress) -> Self {
        Self::from(CalendarAttendee::from(address))
    }
}

impl From<Vec<CalendarAttendee>> for CalendarProposalAttendees {
    fn from(attendees: Vec<CalendarAttendee>) -> Self {
        Self(attendees)
    }
}

impl<const N: usize> From<[CalendarAttendee; N]> for CalendarProposalAttendees {
    fn from(attendees: [CalendarAttendee; N]) -> Self {
        Self(attendees.into_iter().collect())
    }
}

impl From<&[CalendarAttendee]> for CalendarProposalAttendees {
    fn from(attendees: &[CalendarAttendee]) -> Self {
        Self(attendees.to_vec())
    }
}

impl fmt::Debug for CalendarProposalAttendees {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.0.iter()).finish()
    }
}

/// The only direct Outlook calendar-write input: an owner-only event draft.
///
/// There is intentionally no attendee field or accessor. External meetings
/// use the Google proposal lifecycle instead.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnerEventDraft {
    operation_key: String,
    title: String,
    time_range: TimeRange,
    timezone: String,
}

impl OwnerEventDraft {
    /// Constructs an owner-only event draft with validated metadata.
    pub fn new(
        operation_key: impl Into<String>,
        title: impl Into<String>,
        time_range: TimeRange,
        timezone: impl Into<String>,
    ) -> ProviderResult<Self> {
        Ok(Self {
            operation_key: validate_identifier(
                operation_key.into(),
                ProviderInputField::OperationKey,
            )?,
            title: validate_calendar_title(title.into())?,
            time_range,
            timezone: validate_timezone(timezone.into())?,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        operation_key: impl Into<String>,
        title: impl Into<String>,
        time_range: TimeRange,
        timezone: impl Into<String>,
    ) -> ProviderResult<Self> {
        Self::new(operation_key, title, time_range, timezone)
    }

    /// Returns the stable operation key.
    pub fn operation_key(&self) -> &str {
        &self.operation_key
    }

    /// Returns the event title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the UTC event interval.
    pub fn time_range(&self) -> &TimeRange {
        &self.time_range
    }

    /// Alias for [`Self::time_range`].
    pub fn range(&self) -> &TimeRange {
        self.time_range()
    }

    /// Returns the validated IANA timezone identifier.
    pub fn timezone(&self) -> &str {
        &self.timezone
    }
}

impl fmt::Debug for OwnerEventDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnerEventDraft")
            .field("operation_key", &"<redacted>")
            .field("title", &"<redacted>")
            .field("time_range", &self.time_range)
            .field("timezone", &"<redacted>")
            .finish()
    }
}

/// A pending Google Calendar proposal containing exactly one owner attendee.
#[derive(Clone, PartialEq, Eq)]
pub struct GoogleProposalDraft {
    operation_key: String,
    pending_title: String,
    time_range: TimeRange,
    timezone: String,
    owner: CalendarAttendee,
}

impl GoogleProposalDraft {
    /// Constructs a pending proposal from exactly one owner `needsAction`
    /// attendee. A collection input is accepted only to validate its exact
    /// cardinality before it crosses the provider boundary.
    pub fn new<A>(
        operation_key: impl Into<String>,
        pending_title: impl Into<String>,
        time_range: TimeRange,
        timezone: impl Into<String>,
        attendees: A,
    ) -> ProviderResult<Self>
    where
        A: Into<CalendarProposalAttendees>,
    {
        let attendees = attendees.into().0;
        if attendees.len() != 1 || !attendees[0].rsvp.is_needs_action() {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::AttendeeRsvp,
            });
        }
        Ok(Self {
            operation_key: validate_identifier(
                operation_key.into(),
                ProviderInputField::OperationKey,
            )?,
            pending_title: validate_calendar_title(pending_title.into())?,
            time_range,
            timezone: validate_timezone(timezone.into())?,
            owner: attendees
                .into_iter()
                .next()
                .expect("proposal attendee count validated"),
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new<A>(
        operation_key: impl Into<String>,
        pending_title: impl Into<String>,
        time_range: TimeRange,
        timezone: impl Into<String>,
        attendees: A,
    ) -> ProviderResult<Self>
    where
        A: Into<CalendarProposalAttendees>,
    {
        Self::new(
            operation_key,
            pending_title,
            time_range,
            timezone,
            attendees,
        )
    }

    /// Constructs a pending proposal from one owner attendee.
    pub fn from_owner(
        operation_key: impl Into<String>,
        pending_title: impl Into<String>,
        time_range: TimeRange,
        timezone: impl Into<String>,
        owner: CalendarAttendee,
    ) -> ProviderResult<Self> {
        Self::new(operation_key, pending_title, time_range, timezone, owner)
    }

    /// Returns the stable operation key.
    pub fn operation_key(&self) -> &str {
        &self.operation_key
    }

    /// Returns the pending event title.
    pub fn pending_title(&self) -> &str {
        &self.pending_title
    }

    /// Alias for [`Self::pending_title`].
    pub fn title(&self) -> &str {
        self.pending_title()
    }

    /// Returns the UTC event interval.
    pub fn time_range(&self) -> &TimeRange {
        &self.time_range
    }

    /// Alias for [`Self::time_range`].
    pub fn range(&self) -> &TimeRange {
        self.time_range()
    }

    /// Returns the validated IANA timezone identifier.
    pub fn timezone(&self) -> &str {
        &self.timezone
    }

    /// Returns the sole owner attendee.
    pub fn owner(&self) -> &CalendarAttendee {
        &self.owner
    }

    /// Returns the exact one-attendee owner list.
    pub fn attendees(&self) -> &[CalendarAttendee] {
        std::slice::from_ref(&self.owner)
    }
}

impl fmt::Debug for GoogleProposalDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GoogleProposalDraft")
            .field("operation_key", &"<redacted>")
            .field("pending_title", &"<redacted>")
            .field("time_range", &self.time_range)
            .field("timezone", &"<redacted>")
            .field("owner", &self.owner)
            .finish()
    }
}

/// A request to promote an existing Google proposal event.
///
/// The provider event ID is mandatory and there is no create identity or
/// second-event field, so promotion can only target the existing event.
#[derive(Clone, PartialEq, Eq)]
pub struct GoogleProposalPromotion {
    provider_event_id: String,
    final_title: String,
    requester: Option<CalendarAttendee>,
    expected_owner_acceptance: bool,
}

impl GoogleProposalPromotion {
    /// Constructs a promotion request targeting an existing provider event.
    pub fn new(
        provider_event_id: impl Into<String>,
        final_title: impl Into<String>,
        requester: Option<CalendarAttendee>,
        expected_owner_acceptance: bool,
    ) -> ProviderResult<Self> {
        Ok(Self {
            provider_event_id: validate_identifier(
                provider_event_id.into(),
                ProviderInputField::EventId,
            )?,
            final_title: validate_calendar_title(final_title.into())?,
            requester,
            expected_owner_acceptance,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        provider_event_id: impl Into<String>,
        final_title: impl Into<String>,
        requester: Option<CalendarAttendee>,
        expected_owner_acceptance: bool,
    ) -> ProviderResult<Self> {
        Self::new(
            provider_event_id,
            final_title,
            requester,
            expected_owner_acceptance,
        )
    }

    /// Returns the existing provider event identifier to update.
    pub fn provider_event_id(&self) -> &str {
        &self.provider_event_id
    }

    /// Alias for [`Self::provider_event_id`].
    pub fn event_id(&self) -> &str {
        self.provider_event_id()
    }

    /// Returns the final promoted event title.
    pub fn final_title(&self) -> &str {
        &self.final_title
    }

    /// Alias for [`Self::final_title`].
    pub fn title(&self) -> &str {
        self.final_title()
    }

    /// Returns the optional requester attendee to add to the existing event.
    pub fn requester(&self) -> Option<&CalendarAttendee> {
        self.requester.as_ref()
    }

    /// Returns whether the provider operation must observe owner acceptance.
    pub const fn expected_owner_acceptance(&self) -> bool {
        self.expected_owner_acceptance
    }

    /// Alias for [`Self::expected_owner_acceptance`].
    pub const fn owner_acceptance_expected(&self) -> bool {
        self.expected_owner_acceptance()
    }
}

impl fmt::Debug for GoogleProposalPromotion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GoogleProposalPromotion")
            .field("provider_event_id", &"<redacted>")
            .field("final_title", &"<redacted>")
            .field("requester", &self.requester)
            .field("expected_owner_acceptance", &self.expected_owner_acceptance)
            .finish()
    }
}

/// A validated calendar event returned by a provider.
#[derive(Clone, PartialEq, Eq)]
pub struct CalendarEvent {
    provider_event_id: String,
    operation_key: String,
    title: String,
    time_range: TimeRange,
    timezone: String,
    attendees: Vec<CalendarAttendee>,
    last_modified_at: DateTime<Utc>,
}

impl CalendarEvent {
    /// Constructs a calendar event and rejects duplicate attendee addresses.
    pub fn new<I>(
        provider_event_id: impl Into<String>,
        operation_key: impl Into<String>,
        title: impl Into<String>,
        time_range: TimeRange,
        timezone: impl Into<String>,
        attendees: I,
        last_modified_at: DateTime<Utc>,
    ) -> ProviderResult<Self>
    where
        I: IntoIterator<Item = CalendarAttendee>,
    {
        let attendees: Vec<_> = attendees.into_iter().collect();
        let mut addresses = std::collections::HashSet::with_capacity(attendees.len());
        if attendees
            .iter()
            .any(|attendee| !addresses.insert(attendee.address()))
        {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::EmailAddress,
            });
        }
        Ok(Self {
            provider_event_id: validate_identifier(
                provider_event_id.into(),
                ProviderInputField::EventId,
            )?,
            operation_key: validate_identifier(
                operation_key.into(),
                ProviderInputField::OperationKey,
            )?,
            title: validate_calendar_title(title.into())?,
            time_range,
            timezone: validate_timezone(timezone.into())?,
            attendees,
            last_modified_at: validate_timestamp(
                last_modified_at,
                ProviderInputField::LastModifiedAt,
            )?,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new<I>(
        provider_event_id: impl Into<String>,
        operation_key: impl Into<String>,
        title: impl Into<String>,
        time_range: TimeRange,
        timezone: impl Into<String>,
        attendees: I,
        last_modified_at: DateTime<Utc>,
    ) -> ProviderResult<Self>
    where
        I: IntoIterator<Item = CalendarAttendee>,
    {
        Self::new(
            provider_event_id,
            operation_key,
            title,
            time_range,
            timezone,
            attendees,
            last_modified_at,
        )
    }

    /// Returns the provider event identifier.
    pub fn provider_event_id(&self) -> &str {
        &self.provider_event_id
    }

    /// Alias for [`Self::provider_event_id`].
    pub fn event_id(&self) -> &str {
        self.provider_event_id()
    }

    /// Returns the associated stable operation key.
    pub fn operation_key(&self) -> &str {
        &self.operation_key
    }

    /// Returns the event title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the UTC event interval.
    pub fn time_range(&self) -> &TimeRange {
        &self.time_range
    }

    /// Alias for [`Self::time_range`].
    pub fn range(&self) -> &TimeRange {
        self.time_range()
    }

    /// Returns the validated IANA timezone identifier.
    pub fn timezone(&self) -> &str {
        &self.timezone
    }

    /// Returns attendees in provider response order.
    pub fn attendees(&self) -> &[CalendarAttendee] {
        &self.attendees
    }

    /// Returns the provider's UTC last-modified timestamp.
    pub fn last_modified_at(&self) -> DateTime<Utc> {
        self.last_modified_at
    }

    /// Alias for [`Self::last_modified_at`].
    pub fn changed_at(&self) -> DateTime<Utc> {
        self.last_modified_at()
    }
}

impl fmt::Debug for CalendarEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalendarEvent")
            .field("provider_event_id", &"<redacted>")
            .field("operation_key", &"<redacted>")
            .field("title", &"<redacted>")
            .field("time_range", &self.time_range)
            .field("timezone", &"<redacted>")
            .field("attendees", &self.attendees)
            .field("last_modified_at", &self.last_modified_at)
            .finish()
    }
}

/// An incremental calendar change: either a complete upsert or a deletion
/// tombstone containing no event body.
#[derive(Clone, PartialEq, Eq)]
pub enum CalendarChange {
    /// A complete event replacement or insertion.
    Upsert { event: CalendarEvent },
    /// A deletion containing only validated provider ID and UTC change time.
    ///
    /// Both fields use private-payload value types, so direct construction
    /// cannot bypass the constructor validation.
    Deleted {
        provider_event_id: ProviderEventId,
        changed_at: CalendarChangedAt,
    },
}

impl CalendarChange {
    /// Wraps a validated event as an upsert change.
    pub fn upsert(event: CalendarEvent) -> ProviderResult<Self> {
        Ok(Self::Upsert { event })
    }

    /// Alias for [`Self::upsert`].
    pub fn new_upsert(event: CalendarEvent) -> ProviderResult<Self> {
        Self::upsert(event)
    }

    /// Wraps an existing event ID and change timestamp as a tombstone.
    pub fn deleted(
        provider_event_id: impl Into<String>,
        changed_at: DateTime<Utc>,
    ) -> ProviderResult<Self> {
        Ok(Self::Deleted {
            provider_event_id: ProviderEventId::new(provider_event_id.into())?,
            changed_at: CalendarChangedAt::new(changed_at)?,
        })
    }

    /// Alias for [`Self::deleted`].
    pub fn deletion(
        provider_event_id: impl Into<String>,
        changed_at: DateTime<Utc>,
    ) -> ProviderResult<Self> {
        Self::deleted(provider_event_id, changed_at)
    }

    /// Returns the changed provider event identifier.
    pub fn provider_event_id(&self) -> &str {
        match self {
            Self::Upsert { event } => event.provider_event_id(),
            Self::Deleted {
                provider_event_id, ..
            } => provider_event_id.as_str(),
        }
    }

    /// Alias for [`Self::provider_event_id`].
    pub fn event_id(&self) -> &str {
        self.provider_event_id()
    }

    /// Returns the UTC time at which this change was observed.
    pub fn changed_at(&self) -> DateTime<Utc> {
        match self {
            Self::Upsert { event } => event.last_modified_at(),
            Self::Deleted { changed_at, .. } => changed_at.as_datetime(),
        }
    }

    /// Returns the upserted event, or `None` for a deletion tombstone.
    pub fn event(&self) -> Option<&CalendarEvent> {
        match self {
            Self::Upsert { event } => Some(event),
            Self::Deleted { .. } => None,
        }
    }

    /// Returns whether this change is an upsert.
    pub const fn is_upsert(&self) -> bool {
        matches!(self, Self::Upsert { .. })
    }

    /// Returns whether this change is a deletion tombstone.
    pub const fn is_deleted(&self) -> bool {
        matches!(self, Self::Deleted { .. })
    }
}

impl fmt::Debug for CalendarChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Upsert { event } => formatter
                .debug_struct("CalendarChange::Upsert")
                .field("event", event)
                .finish(),
            Self::Deleted {
                provider_event_id: _,
                changed_at,
            } => formatter
                .debug_struct("CalendarChange::Deleted")
                .field("provider_event_id", &"<redacted>")
                .field("changed_at", &changed_at.as_datetime())
                .finish(),
        }
    }
}

fn validate_identifier(value: String, field: ProviderInputField) -> ProviderResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_PROVIDER_ID_LENGTH
        || !value.is_ascii()
        || value.chars().any(char::is_control)
    {
        return Err(ProviderError::InvalidInput { field });
    }
    Ok(value)
}

fn validate_mail_text(value: String, field: ProviderInputField) -> ProviderResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_MAIL_TEXT_LENGTH
        || value.chars().any(char::is_control)
    {
        return Err(ProviderError::InvalidInput { field });
    }
    Ok(value)
}

fn validate_calendar_title(value: String) -> ProviderResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_CALENDAR_TITLE_LENGTH
        || value.chars().any(char::is_control)
    {
        return Err(ProviderError::InvalidInput {
            field: ProviderInputField::Title,
        });
    }
    Ok(value)
}

fn validate_timezone(value: String) -> ProviderResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_TIMEZONE_LENGTH
        || value.trim() != value
        || value.parse::<Tz>().is_err()
    {
        return Err(ProviderError::InvalidInput {
            field: ProviderInputField::Timezone,
        });
    }
    Ok(value)
}

fn validate_timestamp(
    value: DateTime<Utc>,
    field: ProviderInputField,
) -> ProviderResult<DateTime<Utc>> {
    if value.timestamp_nanos_opt().is_none() {
        return Err(ProviderError::InvalidInput { field });
    }
    Ok(value)
}

fn is_valid_mail_address(value: &str) -> bool {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_whitespace)
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '@' | '.' | '_' | '-' | '+')
        })
    {
        return false;
    }

    let mut parts = value.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || local.is_empty()
        || domain.is_empty()
        || local.starts_with('.')
        || local.ends_with('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || !domain.contains('.')
    {
        return false;
    }
    !domain.split('.').any(str::is_empty)
}

fn validate_label(value: String) -> ProviderResult<String> {
    validate_mail_text(value, ProviderInputField::Label)
}

fn validate_labels<I, L>(labels: I) -> ProviderResult<Vec<String>>
where
    I: IntoIterator<Item = L>,
    L: Into<String>,
{
    let mut validated = Vec::new();
    for label in labels {
        let label = validate_label(label.into())?;
        if validated.iter().any(|existing| existing == &label) {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::Label,
            });
        }
        validated.push(label);
    }
    Ok(validated)
}

/// A validated mail address.
///
/// The address is intentionally not serializable and its debug form is
/// redacted because provider payloads may contain personal information.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MailAddress(String);

impl MailAddress {
    /// Constructs a basic shape-validated mail address.
    pub fn new(value: impl Into<String>) -> ProviderResult<Self> {
        let value = value.into();
        if value.len() > MAX_MAIL_ADDRESS_LENGTH || !is_valid_mail_address(&value) {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::EmailAddress,
            });
        }
        Ok(Self(value))
    }

    /// Alias for [`Self::new`].
    pub fn try_new(value: impl Into<String>) -> ProviderResult<Self> {
        Self::new(value)
    }

    /// Returns the validated address for an explicitly initiated provider call.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Alias for [`Self::as_str`].
    pub fn address(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for MailAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MailAddress(<redacted>)")
    }
}

/// A validated provider message identifier accepted by mail mutations.
///
/// Keeping this identifier private-field and validated makes it impossible for
/// a provider adapter to receive an unchecked message ID through the trait.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MailMessageId(String);

/// Alias for callers that use the provider-neutral identifier terminology.
pub type ProviderItemId = MailMessageId;

impl MailMessageId {
    /// Constructs a validated provider message identifier.
    pub fn new(value: impl Into<String>) -> ProviderResult<Self> {
        Ok(Self(validate_identifier(
            value.into(),
            ProviderInputField::SourceId,
        )?))
    }

    /// Alias for [`Self::new`].
    pub fn try_new(value: impl Into<String>) -> ProviderResult<Self> {
        Self::new(value)
    }

    /// Returns the identifier for an explicitly initiated provider call.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MailMessageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MailMessageId(<redacted>)")
    }
}

/// An incoming provider message whose body remains transient and redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct MailMessage {
    source_id: MailMessageId,
    sender: MailAddress,
    subject: String,
    body: String,
    received_at: DateTime<Utc>,
    labels: Vec<String>,
}

impl MailMessage {
    /// Constructs an incoming message with validated provider metadata.
    pub fn new<I, L>(
        source_id: impl Into<String>,
        sender: MailAddress,
        subject: impl Into<String>,
        body: impl Into<String>,
        received_at: DateTime<Utc>,
        labels: I,
    ) -> ProviderResult<Self>
    where
        I: IntoIterator<Item = L>,
        L: Into<String>,
    {
        Ok(Self {
            source_id: MailMessageId::new(source_id)?,
            sender,
            subject: validate_mail_text(subject.into(), ProviderInputField::Subject)?,
            body: body.into(),
            received_at: validate_timestamp(received_at, ProviderInputField::ReceivedAt)?,
            labels: validate_labels(labels)?,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new<I, L>(
        source_id: impl Into<String>,
        sender: MailAddress,
        subject: impl Into<String>,
        body: impl Into<String>,
        received_at: DateTime<Utc>,
        labels: I,
    ) -> ProviderResult<Self>
    where
        I: IntoIterator<Item = L>,
        L: Into<String>,
    {
        Self::new(source_id, sender, subject, body, received_at, labels)
    }

    /// Returns the provider's source message identifier.
    pub fn source_id(&self) -> &MailMessageId {
        &self.source_id
    }

    /// Alias for [`Self::source_id`].
    pub fn message_id(&self) -> &MailMessageId {
        self.source_id()
    }

    /// Returns the typed sender address.
    pub fn sender(&self) -> &MailAddress {
        &self.sender
    }

    /// Returns the sender address text for an explicitly initiated call.
    pub fn sender_address(&self) -> &str {
        self.sender.as_str()
    }

    /// Returns the untrusted subject text.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Returns the untrusted body text without serializing or persisting it.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Alias for [`Self::body`].
    pub fn body_text(&self) -> &str {
        self.body()
    }

    /// Returns the received UTC timestamp.
    pub fn received_at(&self) -> DateTime<Utc> {
        self.received_at
    }

    /// Returns labels in provider response order.
    pub fn labels(&self) -> &[String] {
        &self.labels
    }
}

impl fmt::Debug for MailMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailMessage")
            .field("source_id", &"<redacted>")
            .field("sender", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("body", &"<redacted>")
            .field("received_at", &self.received_at)
            .field("labels", &"<redacted>")
            .finish()
    }
}

/// A validated add/remove label set for one Gmail message.
#[derive(Clone, PartialEq, Eq)]
pub struct LabelChanges {
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
}

impl LabelChanges {
    /// Constructs disjoint, duplicate-free add and remove label sets.
    pub fn new<AI, AL, RI, RL>(add_labels: AI, remove_labels: RI) -> ProviderResult<Self>
    where
        AI: IntoIterator<Item = AL>,
        AL: Into<String>,
        RI: IntoIterator<Item = RL>,
        RL: Into<String>,
    {
        let add_labels = validate_labels(add_labels)?;
        let remove_labels = validate_labels(remove_labels)?;
        if add_labels
            .iter()
            .any(|label| remove_labels.iter().any(|removed| removed == label))
        {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::Label,
            });
        }
        Ok(Self {
            add_labels,
            remove_labels,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new<AI, AL, RI, RL>(add_labels: AI, remove_labels: RI) -> ProviderResult<Self>
    where
        AI: IntoIterator<Item = AL>,
        AL: Into<String>,
        RI: IntoIterator<Item = RL>,
        RL: Into<String>,
    {
        Self::new(add_labels, remove_labels)
    }

    /// Returns labels to add.
    pub fn add_labels(&self) -> &[String] {
        &self.add_labels
    }

    /// Returns labels to remove.
    pub fn remove_labels(&self) -> &[String] {
        &self.remove_labels
    }

    /// Alias for [`Self::add_labels`].
    pub fn add(&self) -> &[String] {
        self.add_labels()
    }

    /// Alias for [`Self::remove_labels`].
    pub fn remove(&self) -> &[String] {
        self.remove_labels()
    }
}

impl fmt::Debug for LabelChanges {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LabelChanges")
            .field("add_labels", &"<redacted>")
            .field("remove_labels", &"<redacted>")
            .finish()
    }
}

/// Validated input for one incremental mail synchronization request.
#[derive(Clone, PartialEq, Eq)]
pub struct MailSyncRequest {
    cursor: Option<String>,
    limit: usize,
}

impl MailSyncRequest {
    /// Constructs a request with an optional cursor and bounded positive limit.
    pub fn new(cursor: Option<String>, limit: usize) -> ProviderResult<Self> {
        if limit == 0 || limit > MAX_MAIL_SYNC_LIMIT {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::Limit,
            });
        }
        if let Some(cursor) = cursor.as_deref() {
            validate_identifier(cursor.to_owned(), ProviderInputField::NextCursor)?;
        }
        Ok(Self { cursor, limit })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(cursor: Option<String>, limit: usize) -> ProviderResult<Self> {
        Self::new(cursor, limit)
    }

    /// Returns the optional incremental cursor.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Returns the positive bounded page limit.
    pub const fn limit(&self) -> usize {
        self.limit
    }
}

impl fmt::Debug for MailSyncRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailSyncRequest")
            .field("cursor", &self.cursor.as_deref().map(|_| "<redacted>"))
            .field("limit", &self.limit)
            .finish()
    }
}

/// An incoming mail provider contract shared by Outlook and Gmail adapters.
pub trait IncomingMailProvider: Send + Sync {
    /// Reads one validated incremental page of mail.
    fn sync_mail<'a>(
        &'a self,
        session: &'a ProviderSession,
        request: &'a MailSyncRequest,
    ) -> ProviderFuture<'a, SyncPage<MailMessage>>;
}

/// Read-only Outlook mail capability. No send or mutation operation is exposed.
pub trait OutlookMailProvider: IncomingMailProvider {}

/// Gmail mail capability, including the only permitted PA mail mutations.
pub trait GmailProvider: IncomingMailProvider {
    /// Applies explicit, validated label additions/removals to one source item.
    fn modify_labels<'a>(
        &'a self,
        session: &'a ProviderSession,
        source_id: &'a MailMessageId,
        changes: &'a LabelChanges,
    ) -> ProviderFuture<'a, ()>;

    /// Sends one typed outbound message and returns its provider result.
    fn send_mail<'a>(
        &'a self,
        session: &'a ProviderSession,
        mail: &'a OutboundMail,
    ) -> ProviderFuture<'a, SentMail>;
}

/// A validated, typed outbound mail command.
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundMail {
    operation_key: String,
    recipient: MailAddress,
    subject: String,
    body: String,
}

impl OutboundMail {
    /// Constructs an outbound message with a stable operation key.
    pub fn new(
        operation_key: impl Into<String>,
        recipient: MailAddress,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> ProviderResult<Self> {
        Ok(Self {
            operation_key: validate_mail_text(
                operation_key.into(),
                ProviderInputField::OperationKey,
            )?,
            recipient,
            subject: validate_mail_text(subject.into(), ProviderInputField::Subject)?,
            body: body.into(),
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        operation_key: impl Into<String>,
        recipient: MailAddress,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> ProviderResult<Self> {
        Self::new(operation_key, recipient, subject, body)
    }

    /// Returns the stable idempotency key.
    pub fn operation_key(&self) -> &str {
        &self.operation_key
    }

    /// Returns the typed recipient address.
    pub fn recipient(&self) -> &MailAddress {
        &self.recipient
    }

    /// Returns the recipient address text for an explicitly initiated call.
    pub fn recipient_address(&self) -> &str {
        self.recipient.as_str()
    }

    /// Returns the untrusted subject text.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Returns the transient body text.
    pub fn body(&self) -> &str {
        &self.body
    }
}

impl fmt::Debug for OutboundMail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundMail")
            .field("operation_key", &"<redacted>")
            .field("recipient", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("body", &"<redacted>")
            .finish()
    }
}

/// The provider identifier and UTC timestamp returned after sending mail.
#[derive(Clone, PartialEq, Eq)]
pub struct SentMail {
    provider_message_id: String,
    sent_at: DateTime<Utc>,
}

impl fmt::Debug for SentMail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SentMail")
            .field("provider_message_id", &"<redacted>")
            .field("sent_at", &self.sent_at)
            .finish()
    }
}

/// Validated input for one incremental calendar synchronization request.
#[derive(Clone, PartialEq, Eq)]
pub struct CalendarSyncRequest {
    time_range: TimeRange,
    cursor: Option<String>,
    limit: usize,
}

impl CalendarSyncRequest {
    /// Constructs a request with a validated range, optional cursor, and
    /// bounded positive page limit.
    pub fn new(
        time_range: TimeRange,
        cursor: Option<String>,
        limit: usize,
    ) -> ProviderResult<Self> {
        if limit == 0 || limit > MAX_CALENDAR_SYNC_LIMIT {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::Limit,
            });
        }
        let cursor = cursor
            .map(|cursor| validate_identifier(cursor, ProviderInputField::NextCursor))
            .transpose()?;
        Ok(Self {
            time_range,
            cursor,
            limit,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        time_range: TimeRange,
        cursor: Option<String>,
        limit: usize,
    ) -> ProviderResult<Self> {
        Self::new(time_range, cursor, limit)
    }

    /// Returns the validated UTC synchronization interval.
    pub fn time_range(&self) -> &TimeRange {
        &self.time_range
    }

    /// Alias for [`Self::time_range`].
    pub fn range(&self) -> &TimeRange {
        self.time_range()
    }

    /// Returns the optional incremental synchronization cursor.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Returns the positive bounded page limit.
    pub const fn limit(&self) -> usize {
        self.limit
    }
}

impl fmt::Debug for CalendarSyncRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalendarSyncRequest")
            .field("time_range", &self.time_range)
            .field("cursor", &self.cursor.as_deref().map(|_| "<redacted>"))
            .field("limit", &self.limit)
            .finish()
    }
}

/// Read-only calendar capabilities shared by Outlook and Google adapters.
pub trait CalendarReadProvider: Send + Sync {
    /// Reads busy intervals for one validated UTC range.
    fn list_busy<'a>(
        &'a self,
        session: &'a ProviderSession,
        time_range: &'a TimeRange,
    ) -> ProviderFuture<'a, Vec<BusyInterval>>;

    /// Reads one validated incremental page of calendar changes.
    fn sync_calendar<'a>(
        &'a self,
        session: &'a ProviderSession,
        request: &'a CalendarSyncRequest,
    ) -> ProviderFuture<'a, SyncPage<CalendarChange>>;
}

/// Outlook calendar capability: read calendar data and create owner-only
/// events. No attendee, update, delete, or proposal operation is exposed.
pub trait OutlookCalendarProvider: CalendarReadProvider {
    /// Read-only lookup by the application-owned direct-event operation key.
    /// `NotFound` means no successful create is observable. This must not
    /// mutate state and is used after ambiguous create outcomes.
    fn find_owner_event<'a>(
        &'a self,
        session: &'a ProviderSession,
        draft: &'a OwnerEventDraft,
    ) -> ProviderFuture<'a, CalendarEvent>;

    /// Creates one direct owner-only event from a typed draft.
    /// Implementations must return the same event for exact retries,
    /// including an ambiguous timeout after a remote mutation.
    fn create_owner_event<'a>(
        &'a self,
        session: &'a ProviderSession,
        draft: &'a OwnerEventDraft,
    ) -> ProviderFuture<'a, CalendarEvent>;
}

/// Google Calendar capability: read data and manage the proposal lifecycle.
pub trait GoogleCalendarProvider: CalendarReadProvider {
    /// Finds the existing proposal created with this exact application-owned
    /// operation key. This operation is read-only: it must not create, alter,
    /// or delete calendar state. `NotFound` means no create has completed yet.
    /// This is required to recover a process crash after a provider create
    /// succeeds but before its durable event mapping commits.
    fn find_proposal<'a>(
        &'a self,
        session: &'a ProviderSession,
        draft: &'a GoogleProposalDraft,
    ) -> ProviderFuture<'a, CalendarEvent>;

    /// Creates one pending owner-only proposal event.
    ///
    /// Adapters must treat `draft.operation_key()` as an idempotency key and
    /// return the same event for an exact retry, including after an ambiguous
    /// timeout or connection failure where the remote create may have won.
    fn create_proposal<'a>(
        &'a self,
        session: &'a ProviderSession,
        draft: &'a GoogleProposalDraft,
    ) -> ProviderFuture<'a, CalendarEvent>;

    /// Promotes one existing proposal event using its typed promotion input.
    fn promote_proposal<'a>(
        &'a self,
        session: &'a ProviderSession,
        promotion: &'a GoogleProposalPromotion,
    ) -> ProviderFuture<'a, CalendarEvent>;

    /// Deletes one existing proposal event by validated typed provider ID.
    fn delete_proposal<'a>(
        &'a self,
        session: &'a ProviderSession,
        provider_event_id: &'a ProviderEventId,
    ) -> ProviderFuture<'a, ()>;
}

impl SentMail {
    /// Constructs a sent result with a validated provider message identifier.
    pub fn new(
        provider_message_id: impl Into<String>,
        sent_at: DateTime<Utc>,
    ) -> ProviderResult<Self> {
        Ok(Self {
            provider_message_id: validate_identifier(
                provider_message_id.into(),
                ProviderInputField::ItemId,
            )?,
            sent_at: validate_timestamp(sent_at, ProviderInputField::SentAt)?,
        })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        provider_message_id: impl Into<String>,
        sent_at: DateTime<Utc>,
    ) -> ProviderResult<Self> {
        Self::new(provider_message_id, sent_at)
    }

    /// Returns the provider's sent-message identifier.
    pub fn provider_message_id(&self) -> &str {
        &self.provider_message_id
    }

    /// Returns the UTC send timestamp.
    pub fn sent_at(&self) -> DateTime<Utc> {
        self.sent_at
    }
}

/// Transient, validated input for one explicitly initiated email triage call.
///
/// This value is intentionally not serializable or persistable. Its message
/// content may only be read by an adapter while performing the requested call.
#[derive(Clone, PartialEq, Eq)]
pub struct TriageInput {
    source_id: MailMessageId,
    sender: MailAddress,
    subject: String,
    body: String,
}

impl TriageInput {
    /// Constructs one transient triage input from validated mail identity.
    pub fn new(
        source_id: MailMessageId,
        sender: MailAddress,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> ProviderResult<Self> {
        Ok(Self {
            source_id,
            sender,
            subject: validate_mail_text(subject.into(), ProviderInputField::Subject)?,
            body: body.into(),
        })
    }

    /// Returns the validated source ID for the explicitly initiated call.
    pub fn source_id(&self) -> &MailMessageId {
        &self.source_id
    }

    /// Returns the validated sender for the explicitly initiated call.
    pub fn sender(&self) -> &MailAddress {
        &self.sender
    }

    /// Returns the untrusted subject for the explicitly initiated call.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Returns the untrusted transient body for the explicitly initiated call.
    pub fn body(&self) -> &str {
        &self.body
    }
}

impl fmt::Debug for TriageInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TriageInput")
            .field("source_id", &"<redacted>")
            .field("sender", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("body", &"<redacted>")
            .finish()
    }
}

/// A validated actionable task extracted from one email.
#[derive(Clone, PartialEq, Eq)]
pub struct ActionableTaskExtraction {
    kind: TaskKind,
    title: String,
    duration_minutes: u16,
    due_at: Option<DateTime<Utc>>,
}

impl ActionableTaskExtraction {
    /// Constructs a task using an explicit provider-supplied duration.
    ///
    /// Callers must apply any category defaults before invoking this boundary.
    pub fn new(
        kind: TaskKind,
        title: impl Into<String>,
        duration_minutes: u16,
        due_at: Option<DateTime<Utc>>,
    ) -> ProviderResult<Self> {
        if duration_minutes == 0 || duration_minutes > MAX_TRIAGE_DURATION_MINUTES {
            return Err(ProviderError::InvalidInput {
                field: ProviderInputField::TaskDuration,
            });
        }
        Ok(Self {
            kind,
            title: validate_triage_task_title(title.into())?,
            duration_minutes,
            due_at: due_at
                .map(|due_at| validate_timestamp(due_at, ProviderInputField::DueAt))
                .transpose()?,
        })
    }

    /// Returns the extracted closed task category.
    pub const fn kind(&self) -> TaskKind {
        self.kind
    }

    /// Returns the validated task title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the explicit extracted duration in whole minutes.
    pub const fn duration_minutes(&self) -> u16 {
        self.duration_minutes
    }

    /// Returns the optional validated UTC due instant.
    pub const fn due_at(&self) -> Option<DateTime<Utc>> {
        self.due_at
    }
}

impl fmt::Debug for ActionableTaskExtraction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActionableTaskExtraction")
            .field("kind", &self.kind)
            .field("title", &"<redacted>")
            .field("duration_minutes", &self.duration_minutes)
            .field("due_at", &self.due_at)
            .finish()
    }
}

/// Closed reasons that require manual handling rather than automated action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmbiguousReason {
    /// The email does not clearly request an action.
    UnclearAction,
    /// The email does not supply actionable timing.
    UnclearTiming,
    /// The email does not supply a clear duration.
    UnclearDuration,
    /// The email contains an instruction that is unsafe to follow.
    UnsafeInstruction,
}

impl AmbiguousReason {
    /// Returns the stable strict-structured-output wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnclearAction => "unclear_action",
            Self::UnclearTiming => "unclear_timing",
            Self::UnclearDuration => "unclear_duration",
            Self::UnsafeInstruction => "unsafe_instruction",
        }
    }
}

/// The closed triage result for one email.
#[derive(Clone, PartialEq, Eq)]
pub enum TriageDecision {
    /// An actionable task was extracted.
    Actionable(ActionableTaskExtraction),
    /// Manual handling is required for a closed reason.
    Ambiguous(AmbiguousReason),
    /// The email requires no task.
    Ignore,
}

impl TriageDecision {
    /// Returns the stable strict-structured-output wire name.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Actionable(_) => "actionable",
            Self::Ambiguous(_) => "ambiguous",
            Self::Ignore => "ignore",
        }
    }

    /// Returns the extracted task for an actionable decision.
    pub fn actionable(&self) -> Option<&ActionableTaskExtraction> {
        match self {
            Self::Actionable(extraction) => Some(extraction),
            Self::Ambiguous(_) | Self::Ignore => None,
        }
    }

    /// Returns the manual-handling reason for an ambiguous decision.
    pub const fn ambiguous_reason(&self) -> Option<AmbiguousReason> {
        match self {
            Self::Ambiguous(reason) => Some(*reason),
            Self::Actionable(_) | Self::Ignore => None,
        }
    }

    /// Returns whether this decision ignores the email.
    pub const fn is_ignore(&self) -> bool {
        matches!(self, Self::Ignore)
    }
}

impl fmt::Debug for TriageDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Actionable(extraction) => formatter
                .debug_tuple("TriageDecision::Actionable")
                .field(extraction)
                .finish(),
            Self::Ambiguous(reason) => formatter
                .debug_tuple("TriageDecision::Ambiguous")
                .field(reason)
                .finish(),
            Self::Ignore => formatter.write_str("TriageDecision::Ignore"),
        }
    }
}

/// Provider-neutral structured email triage capability.
pub trait StructuredTriageProvider: Send + Sync {
    /// Classifies one transient input during an explicitly initiated call.
    fn classify<'a>(
        &'a self,
        session: &'a ProviderSession,
        input: &'a TriageInput,
    ) -> ProviderFuture<'a, TriageDecision>;
}

fn validate_triage_task_title(value: String) -> ProviderResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_TRIAGE_TASK_TITLE_LENGTH
        || value.chars().any(char::is_control)
    {
        return Err(ProviderError::InvalidInput {
            field: ProviderInputField::Title,
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};

    use super::{
        ActionableTaskExtraction, AmbiguousReason, BackupReceipt, CalendarAttendee, CalendarChange,
        CalendarChangedAt, CalendarEvent, CalendarReadProvider, CalendarSyncRequest,
        EncryptedS3BackupProvider, EncryptedSnapshot, GoogleCalendarProvider, GoogleProposalDraft,
        GoogleProposalPromotion, IncomingMailProvider, LabelChanges, MAX_CALENDAR_SYNC_LIMIT,
        MAX_CALENDAR_TITLE_LENGTH, MAX_MAIL_SYNC_LIMIT, MAX_MAIL_TEXT_LENGTH,
        MAX_PROVIDER_ID_LENGTH, MailAddress, MailMessage, MailMessageId, MailSyncRequest,
        OutboundMail, OutlookCalendarProvider, OutlookMailProvider, OwnerEventDraft, ProviderError,
        ProviderEventId, ProviderFuture, ProviderInputField, ProviderItemFailure, ProviderResult,
        ProviderSession, RetryAfter, Rsvp, SentMail, StructuredTriageProvider, SyncPage, TimeRange,
        TriageDecision, TriageInput, validate_calendar_title,
    };

    use crate::pa::availability::BusyInterval;
    use crate::pa::domain::TaskKind;
    use std::sync::{Arc, Mutex};

    const START: &str = "2026-01-01T00:00:00Z";
    const END: &str = "2026-01-01T01:00:00Z";
    const TOKEN: &str = "sentinel-provider-token";

    fn instant(value: &str) -> DateTime<Utc> {
        value.parse().expect("valid UTC instant")
    }

    fn calendar_owner() -> CalendarAttendee {
        CalendarAttendee::new(
            MailAddress::new("owner@example.test").expect("owner address"),
            Rsvp::NeedsAction,
        )
        .expect("owner attendee")
    }

    fn calendar_requester() -> CalendarAttendee {
        CalendarAttendee::new(
            MailAddress::new("requester@example.test").expect("requester address"),
            Rsvp::Accepted,
        )
        .expect("requester attendee")
    }

    fn failure(item_id: &str) -> ProviderItemFailure {
        ProviderItemFailure::new("mailbox-a", item_id, ProviderError::NotFound)
            .expect("valid item failure")
    }

    struct DebuglessPageItem;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn rejects_invalid_session_inputs_without_echoing_token() {
        assert!(matches!(
            ProviderSession::new(" ", TOKEN, None),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::AccountId
            })
        ));
        assert!(matches!(
            ProviderSession::new("a".repeat(MAX_PROVIDER_ID_LENGTH + 1), TOKEN, None),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::AccountId
            })
        ));
        let error = ProviderSession::new("account-a", " ", None).expect_err("blank token");
        assert_eq!(
            error,
            ProviderError::InvalidInput {
                field: ProviderInputField::AccessToken
            }
        );
        assert!(!format!("{error:?}").contains("rejected-access-token"));
        assert!(!error.to_string().contains("rejected-access-token"));

        let rejected_sentinel = format!("rejected-access-token-{}", "x".repeat(300));
        let error = ProviderSession::new(rejected_sentinel.clone(), "access-token", None)
            .expect_err("over-sized account ID");
        assert!(!format!("{error:?}").contains(&rejected_sentinel));
        assert!(!error.to_string().contains(&rejected_sentinel));
    }

    #[test]
    fn rejects_access_token_control_characters_but_preserves_arbitrary_length() {
        for rejected_token in ["access-token\n", "access-token\0"] {
            assert!(matches!(
                ProviderSession::new("account-a", rejected_token, None),
                Err(ProviderError::InvalidInput {
                    field: ProviderInputField::AccessToken
                })
            ));
        }

        let long_token = format!("access-token-{}", "x".repeat(MAX_PROVIDER_ID_LENGTH * 8));
        let session = ProviderSession::new("account-a", long_token.clone(), None)
            .expect("long token without controls remains valid");
        assert_eq!(session.access_token(), long_token);
    }

    #[test]
    fn session_and_page_debug_redact_tokens() {
        let session = ProviderSession::new("account-a", TOKEN, None).expect("session");
        assert!(!format!("{session:?}").contains(TOKEN));
        let page = SyncPage::new(vec![session], None, vec![failure("event-1")]).expect("page");
        assert!(!format!("{page:?}").contains(TOKEN));
    }

    #[test]
    fn provider_item_failure_debug_redacts_identifiers_and_keeps_error_category() {
        let failure = ProviderItemFailure::new(
            "source-id-sentinel",
            "item-id-sentinel",
            ProviderError::NotFound,
        )
        .expect("failure");
        let debug = format!("{failure:?}");

        assert!(!debug.contains("source-id-sentinel"));
        assert!(!debug.contains("item-id-sentinel"));
        assert!(debug.contains("NotFound"));
    }

    #[test]
    fn sync_page_debug_redacts_contents_and_does_not_require_item_debug() {
        let page = SyncPage::new(
            vec![DebuglessPageItem],
            Some("cursor-sentinel".to_owned()),
            vec![
                ProviderItemFailure::new(
                    "source-id-sentinel",
                    "item-id-sentinel",
                    ProviderError::NotFound,
                )
                .expect("failure"),
            ],
        )
        .expect("page");
        let debug = format!("{page:?}");

        assert!(!debug.contains("cursor-sentinel"));
        assert!(!debug.contains("source-id-sentinel"));
        assert!(!debug.contains("item-id-sentinel"));
        assert!(!debug.contains("NotFound"));
        assert!(debug.contains("item_count"));
        assert!(debug.contains("has_next_cursor"));
        assert!(debug.contains("failure_count"));
    }

    #[test]
    fn closed_errors_have_stable_display_and_valid_throttle() {
        assert_eq!(
            ProviderError::InvalidInput {
                field: ProviderInputField::NextCursor
            }
            .to_string(),
            "provider input is invalid"
        );
        assert_eq!(
            format!(
                "{:?}",
                ProviderError::InvalidInput {
                    field: ProviderInputField::NextCursor
                }
            ),
            "InvalidInput { field: NextCursor }"
        );
        assert_eq!(
            ProviderError::TokenExpired.to_string(),
            "provider token expired"
        );
        assert_eq!(
            ProviderError::CursorExpired.to_string(),
            "provider cursor expired"
        );
        assert_eq!(
            ProviderError::NotFound.to_string(),
            "provider item not found"
        );
        assert_eq!(
            ProviderError::Conflict.to_string(),
            "provider request conflicted"
        );
        assert_eq!(
            ProviderError::Unavailable.to_string(),
            "provider unavailable"
        );
        assert!(matches!(
            RetryAfter::new(Duration::zero()),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::RetryAfter
            })
        ));
        assert!(matches!(
            RetryAfter::new(Duration::seconds(-1)),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::RetryAfter
            })
        ));
        let retry_after = RetryAfter::new(Duration::seconds(7)).expect("positive delay");
        assert_eq!(retry_after.duration(), Duration::seconds(7));
        let throttled = ProviderError::throttled(retry_after);
        assert_eq!(throttled.retry_after(), Some(retry_after));
        assert_eq!(throttled.to_string(), "provider request throttled");
    }

    #[test]
    fn rejects_invalid_ranges_and_ids_or_cursors() {
        assert!(matches!(
            TimeRange::new(instant(START), instant(START)),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::TimeRange
            })
        ));
        assert!(matches!(
            TimeRange::new(instant(END), instant(START)),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::TimeRange
            })
        ));
        assert!(matches!(
            TimeRange::new(DateTime::<Utc>::MIN_UTC, instant(END)),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::TimeRange
            })
        ));
        assert!(matches!(
            TimeRange::new(instant(START), DateTime::<Utc>::MAX_UTC),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::TimeRange
            })
        ));
        assert!(matches!(
            ProviderItemFailure::new(" ", "event-1", ProviderError::NotFound),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::SourceId
            })
        ));
        assert!(matches!(
            ProviderItemFailure::new("source-a", " ", ProviderError::NotFound),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::ItemId
            })
        ));
        assert!(matches!(
            ProviderItemFailure::new(
                "s".repeat(MAX_PROVIDER_ID_LENGTH + 1),
                "event-1",
                ProviderError::NotFound
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::SourceId
            })
        ));
        assert!(matches!(
            ProviderItemFailure::new(
                "source-a",
                "i".repeat(MAX_PROVIDER_ID_LENGTH + 1),
                ProviderError::NotFound
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::ItemId
            })
        ));
        assert!(matches!(
            SyncPage::<u8>::new(vec![1], Some(" ".to_owned()), vec![]),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::NextCursor
            })
        ));
        assert!(matches!(
            SyncPage::<u8>::new(
                vec![1],
                Some("c".repeat(MAX_PROVIDER_ID_LENGTH + 1)),
                vec![]
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::NextCursor
            })
        ));
    }

    #[test]
    fn page_preserves_order_cursor_and_failures() -> ProviderResult<()> {
        let first = failure("event-1");
        let second = failure("event-2");
        let page = SyncPage::new(
            vec![3, 1, 2],
            Some("cursor-2".to_owned()),
            vec![first, second],
        )?;
        assert_eq!(page.items(), &[3, 1, 2]);
        assert_eq!(page.next_cursor(), Some("cursor-2"));
        assert_eq!(page.item_failures().len(), 2);
        assert_eq!(page.failures(), page.item_failures());
        assert_eq!(page.item_failures()[0].source_id(), "mailbox-a");
        assert_eq!(page.item_failures()[0].item_id(), "event-1");
        assert_eq!(page.item_failures()[0].error(), ProviderError::NotFound);
        assert_eq!(page.item_failures()[1].item_id(), "event-2");
        Ok(())
    }

    #[test]
    fn accessors_round_trip_session_and_time_range() -> ProviderResult<()> {
        let expiry = instant("2026-01-01T02:00:00Z");
        let session = ProviderSession::new("account-a", TOKEN, Some(expiry))?;
        assert_eq!(session.account_id(), "account-a");
        assert_eq!(session.access_token(), TOKEN);
        assert_eq!(session.expires_at(), Some(expiry));

        let range = TimeRange::new(instant(START), instant(END))?;
        assert_eq!(range.start(), instant(START));
        assert_eq!(range.end(), instant(END));
        assert_eq!(range.starts_at(), instant(START));
        assert_eq!(range.ends_at(), instant(END));
        Ok(())
    }

    #[test]
    fn primitive_types_are_send_and_sync() {
        assert_send_sync::<ProviderInputField>();
        assert_send_sync::<RetryAfter>();
        assert_send_sync::<ProviderError>();
        assert_send_sync::<ProviderSession>();
        assert_send_sync::<TimeRange>();
        assert_send_sync::<ProviderItemFailure>();
        assert_send_sync::<SyncPage<String>>();
        assert_send_sync::<ProviderEventId>();
        assert_send_sync::<CalendarChangedAt>();
        assert_send_sync::<MailAddress>();
        assert_send_sync::<MailMessage>();
        assert_send_sync::<LabelChanges>();
        assert_send_sync::<MailSyncRequest>();
        assert_send_sync::<OutboundMail>();
        assert_send_sync::<SentMail>();
    }

    #[test]
    fn mail_values_validate_inputs_and_round_trip_untrusted_body() -> ProviderResult<()> {
        let sender = MailAddress::new("sender@example.test")?;
        assert_eq!(sender.as_str(), "sender@example.test");
        assert_eq!(sender.address(), "sender@example.test");

        let body = "  do not trust this text\nwith exact spacing  ";
        let received_at = instant("2026-01-01T00:00:00Z");
        let message = MailMessage::new(
            "provider-message-1",
            sender.clone(),
            "Subject",
            body,
            received_at,
            vec!["INBOX".to_owned(), "UNREAD".to_owned()],
        )?;
        assert_eq!(message.source_id().as_str(), "provider-message-1");
        assert_eq!(message.sender(), &sender);
        assert_eq!(message.subject(), "Subject");
        assert_eq!(message.body(), body);
        assert_eq!(message.body_text(), body);
        assert_eq!(message.received_at(), received_at);
        assert_eq!(message.labels(), &["INBOX".to_owned(), "UNREAD".to_owned()]);

        let outbound = OutboundMail::new("operation-1", sender.clone(), "Reply", "outbound body")?;
        assert_eq!(outbound.operation_key(), "operation-1");
        assert_eq!(outbound.recipient(), &sender);
        assert_eq!(outbound.subject(), "Reply");
        assert_eq!(outbound.body(), "outbound body");

        let sent_at = instant("2026-01-01T00:01:00Z");
        let sent = SentMail::new("provider-sent-1", sent_at)?;
        assert_eq!(sent.provider_message_id(), "provider-sent-1");
        assert_eq!(sent.sent_at(), sent_at);
        Ok(())
    }

    #[test]
    fn mail_debug_redacts_addresses_and_bodies() -> ProviderResult<()> {
        let address = "sentinel-address@example.test";
        let body = "sentinel-mail-body-do-not-log";
        let sender = MailAddress::new(address)?;
        let message = MailMessage::new(
            "provider-message-1",
            sender.clone(),
            "subject",
            body,
            instant(START),
            vec!["INBOX".to_owned()],
        )?;
        let outbound = OutboundMail::new("operation-1", sender.clone(), "subject", body)?;
        let sent = SentMail::new(address, instant(END))?;
        let changes = LabelChanges::new(vec![address], Vec::<String>::new())?;

        for debug in [
            format!("{message:?}"),
            format!("{outbound:?}"),
            format!("{sender:?}"),
            format!("{sent:?}"),
            format!("{changes:?}"),
        ] {
            assert!(!debug.contains(address), "debug leaked address: {debug}");
            assert!(!debug.contains(body), "debug leaked body: {debug}");
        }
        Ok(())
    }

    #[test]
    fn mail_contract_helpers_reject_invalid_values() {
        for value in ["", " ", "missing-at.example", "a@b", "a@@example.test"] {
            assert!(matches!(
                MailAddress::new(value),
                Err(ProviderError::InvalidInput {
                    field: ProviderInputField::EmailAddress
                })
            ));
        }
        assert!(matches!(
            MailAddress::new("a".repeat(321)),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::EmailAddress
            })
        ));

        let address = MailAddress::new("sender@example.test").expect("address");
        let received_at = instant(START);
        assert!(matches!(
            MailMessage::new(
                " ",
                address.clone(),
                "subject",
                "body",
                received_at,
                Vec::<String>::new()
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::SourceId
            })
        ));
        assert!(matches!(
            MailMessage::new(
                "s".repeat(MAX_PROVIDER_ID_LENGTH + 1),
                address.clone(),
                "subject",
                "body",
                received_at,
                Vec::<String>::new()
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::SourceId
            })
        ));
        assert!(matches!(
            MailMessage::new(
                "source",
                address.clone(),
                "s".repeat(MAX_MAIL_TEXT_LENGTH + 1),
                "body",
                received_at,
                Vec::<String>::new()
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Subject
            })
        ));
        assert!(matches!(
            MailMessage::new(
                "source",
                address.clone(),
                "subject",
                "body",
                received_at,
                vec!["INBOX".to_owned(), "INBOX".to_owned()]
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Label
            })
        ));
        assert!(matches!(
            MailMessage::new(
                "source",
                address.clone(),
                "subject",
                "body",
                received_at,
                vec![" ".to_owned()]
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Label
            })
        ));
        assert!(matches!(
            OutboundMail::new(" ", address.clone(), "subject", "body"),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::OperationKey
            })
        ));
        assert!(matches!(
            OutboundMail::new(
                "o".repeat(MAX_MAIL_TEXT_LENGTH + 1),
                address.clone(),
                "subject",
                "body"
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::OperationKey
            })
        ));
        assert!(matches!(
            OutboundMail::new(
                "operation",
                address.clone(),
                "s".repeat(MAX_MAIL_TEXT_LENGTH + 1),
                "body"
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Subject
            })
        ));
        assert!(matches!(
            SentMail::new(" ", received_at),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::ItemId
            })
        ));
        assert!(matches!(
            MailMessage::new(
                "source",
                address.clone(),
                "subject",
                "body",
                DateTime::<Utc>::MAX_UTC,
                Vec::<String>::new()
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::ReceivedAt
            })
        ));
        assert!(matches!(
            SentMail::new("sent", DateTime::<Utc>::MAX_UTC),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::SentAt
            })
        ));

        assert!(matches!(
            LabelChanges::new(
                vec!["INBOX".to_owned(), "INBOX".to_owned()],
                Vec::<String>::new()
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Label
            })
        ));
        assert!(matches!(
            LabelChanges::new(vec!["INBOX".to_owned()], vec!["INBOX".to_owned()]),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Label
            })
        ));
        assert!(matches!(
            MailSyncRequest::new(None, 0),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Limit
            })
        ));
        assert!(matches!(
            MailSyncRequest::new(None, MAX_MAIL_SYNC_LIMIT + 1),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Limit
            })
        ));
        assert!(matches!(
            MailSyncRequest::new(Some(" ".to_owned()), 1),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::NextCursor
            })
        ));
        assert!(matches!(
            MailMessageId::new(" "),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::SourceId
            })
        ));
    }

    #[test]
    fn machine_text_rejects_injection_controls_and_unicode_identifiers() {
        assert!(matches!(
            validate_calendar_title("Injected\r\nTitle".to_owned()),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Title
            })
        ));
        assert!(matches!(
            MailMessageId::new("source\nid"),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::SourceId
            })
        ));
        assert!(matches!(
            MailMessageId::new("source\u{00e9}id"),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::SourceId
            })
        ));

        let address = MailAddress::new("sender@example.test").expect("address");
        let received_at = instant(START);
        assert!(matches!(
            MailMessage::new(
                "source-id",
                address.clone(),
                "Injected\r\nBcc: victim@example.test",
                "body",
                received_at,
                Vec::<String>::new()
            ),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Subject
            })
        ));
        assert!(matches!(
            OutboundMail::new("operation\r\nkey", address.clone(), "subject", "body"),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::OperationKey
            })
        ));
        assert!(matches!(
            LabelChanges::new(vec!["label\r\nInjected"], Vec::<String>::new()),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::Label
            })
        ));

        let transient_body = "body\r\nInjected: Bcc";
        let message = MailMessage::new(
            "source-id",
            address,
            "subject",
            transient_body,
            received_at,
            Vec::<String>::new(),
        )
        .expect("transient body remains opaque to machine-text validation");
        assert_eq!(message.body(), transient_body);
    }

    struct DummyProvider;

    impl IncomingMailProvider for DummyProvider {
        fn sync_mail<'a>(
            &'a self,
            _session: &'a ProviderSession,
            _request: &'a MailSyncRequest,
        ) -> ProviderFuture<'a, SyncPage<MailMessage>> {
            Box::pin(async { SyncPage::new(Vec::new(), None, Vec::new()) })
        }
    }

    impl OutlookMailProvider for DummyProvider {}

    impl super::GmailProvider for DummyProvider {
        fn modify_labels<'a>(
            &'a self,
            _session: &'a ProviderSession,
            _source_id: &'a MailMessageId,
            _changes: &'a LabelChanges,
        ) -> ProviderFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }

        fn send_mail<'a>(
            &'a self,
            _session: &'a ProviderSession,
            _mail: &'a OutboundMail,
        ) -> ProviderFuture<'a, SentMail> {
            Box::pin(async { SentMail::new("sent-1", instant("2026-01-01T00:00:00Z")) })
        }
    }

    fn assert_provider_object_safe(
        incoming: &dyn IncomingMailProvider,
        outlook: &dyn OutlookMailProvider,
        gmail: &dyn super::GmailProvider,
    ) {
        let session = ProviderSession::new("account", "token", None).expect("session");
        let request = MailSyncRequest::new(None, 1).expect("request");
        let future = incoming.sync_mail(&session, &request);
        let _outlook: &dyn IncomingMailProvider = outlook;
        let source_id = MailMessageId::new("message").expect("message ID");
        let changes = LabelChanges::new(Vec::<String>::new(), Vec::<String>::new())
            .expect("empty label changes");
        let modify = gmail.modify_labels(&session, &source_id, &changes);
        let recipient = MailAddress::new("recipient@example.test").expect("recipient");
        let mail =
            OutboundMail::new("operation", recipient, "subject", "body").expect("outbound mail");
        let send = gmail.send_mail(&session, &mail);
        assert_send::<ProviderFuture<'_, SyncPage<MailMessage>>>(&future);
        assert_send::<ProviderFuture<'_, ()>>(&modify);
        assert_send::<ProviderFuture<'_, SentMail>>(&send);
    }

    fn assert_send<T: Send>(_: &T) {}

    #[test]
    fn mail_provider_traits_are_object_safe_and_send() {
        let provider = DummyProvider;
        assert_provider_object_safe(&provider, &provider, &provider);
    }

    #[test]
    fn calendar_attendees_and_rsvp_round_trip_with_redacted_debug() -> ProviderResult<()> {
        let address = MailAddress::new("sentinel-calendar@example.test")?;
        let attendee = CalendarAttendee::new(address.clone(), Rsvp::Tentative)?;
        assert_eq!(attendee.address(), &address);
        assert_eq!(attendee.mail_address(), &address);
        assert_eq!(attendee.rsvp(), Rsvp::Tentative);
        assert_eq!(attendee.status(), Rsvp::Tentative);
        assert_eq!(Rsvp::NeedsAction, Rsvp::NeedsAction);
        for debug in [format!("{address:?}"), format!("{attendee:?}")] {
            assert!(!debug.contains("sentinel-calendar@example.test"));
        }
        Ok(())
    }

    #[test]
    fn owner_event_draft_has_no_attendee_capability_and_round_trips() -> ProviderResult<()> {
        let range = TimeRange::new(instant(START), instant(END))?;
        let draft = OwnerEventDraft::new("owner-op-1", "Focus time", range.clone(), "UTC")?;
        assert_eq!(draft.operation_key(), "owner-op-1");
        assert_eq!(draft.title(), "Focus time");
        assert_eq!(draft.time_range(), &range);
        assert_eq!(draft.range(), &range);
        assert_eq!(draft.timezone(), "UTC");
        Ok(())
    }

    #[test]
    fn proposal_requires_exactly_one_owner_needs_action_attendee() -> ProviderResult<()> {
        let range = TimeRange::new(instant(START), instant(END))?;
        let owner = calendar_owner();
        let proposal = GoogleProposalDraft::new(
            "proposal-op-1",
            "Pending meeting",
            range.clone(),
            "UTC",
            vec![owner.clone()],
        )?;
        assert_eq!(proposal.operation_key(), "proposal-op-1");
        assert_eq!(proposal.pending_title(), "Pending meeting");
        assert_eq!(proposal.title(), "Pending meeting");
        assert_eq!(proposal.time_range(), &range);
        assert_eq!(proposal.timezone(), "UTC");
        assert_eq!(proposal.attendees(), &[owner]);

        let requester = calendar_requester();
        assert!(
            GoogleProposalDraft::new(
                "proposal-op-2",
                "Pending meeting",
                range.clone(),
                "UTC",
                vec![calendar_owner(), requester],
            )
            .is_err()
        );
        assert!(
            GoogleProposalDraft::new(
                "proposal-op-3",
                "Pending meeting",
                range.clone(),
                "UTC",
                vec![calendar_owner(), calendar_owner()],
            )
            .is_err()
        );
        assert!(
            GoogleProposalDraft::new(
                "proposal-op-4",
                "Pending meeting",
                range,
                "UTC",
                vec![CalendarAttendee::new(
                    MailAddress::new("owner@example.test")?,
                    Rsvp::Accepted,
                )?],
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn promotion_targets_existing_event_and_preserves_optional_requester() -> ProviderResult<()> {
        let requester = calendar_requester();
        let promotion = GoogleProposalPromotion::new(
            "google-event-1",
            "Final meeting",
            Some(requester.clone()),
            true,
        )?;
        assert_eq!(promotion.provider_event_id(), "google-event-1");
        assert_eq!(promotion.event_id(), "google-event-1");
        assert_eq!(promotion.final_title(), "Final meeting");
        assert_eq!(promotion.title(), "Final meeting");
        assert_eq!(promotion.requester(), Some(&requester));
        assert!(promotion.expected_owner_acceptance());
        Ok(())
    }

    #[test]
    fn calendar_event_and_changes_round_trip_and_reject_duplicates() -> ProviderResult<()> {
        let owner = calendar_owner();
        let requester = calendar_requester();
        let range = TimeRange::new(instant(START), instant(END))?;
        let modified_at = instant("2026-01-01T00:30:00Z");
        let event = CalendarEvent::new(
            "google-event-1",
            "operation-1",
            "Calendar title",
            range.clone(),
            "UTC",
            vec![owner.clone(), requester.clone()],
            modified_at,
        )?;
        assert_eq!(event.provider_event_id(), "google-event-1");
        assert_eq!(event.operation_key(), "operation-1");
        assert_eq!(event.title(), "Calendar title");
        assert_eq!(event.time_range(), &range);
        assert_eq!(event.timezone(), "UTC");
        assert_eq!(event.attendees(), &[owner, requester]);
        assert_eq!(event.last_modified_at(), modified_at);
        assert_eq!(event.changed_at(), modified_at);
        assert!(
            CalendarEvent::new(
                "google-event-1",
                "operation-1",
                "Calendar title",
                range,
                "UTC",
                vec![calendar_owner(), calendar_owner()],
                modified_at,
            )
            .is_err()
        );

        let upsert = CalendarChange::upsert(event.clone())?;
        assert_eq!(upsert.provider_event_id(), "google-event-1");
        assert_eq!(upsert.changed_at(), modified_at);
        assert_eq!(upsert.event(), Some(&event));
        let deleted = CalendarChange::deleted("google-event-1", modified_at)?;
        assert_eq!(deleted.provider_event_id(), "google-event-1");
        assert_eq!(deleted.changed_at(), modified_at);
        assert_eq!(deleted.event(), None);
        assert!(matches!(deleted, CalendarChange::Deleted { .. }));
        Ok(())
    }

    #[test]
    fn deleted_variant_accepts_only_validated_payload_types() -> ProviderResult<()> {
        let provider_event_id = ProviderEventId::new("google-event-1")?;
        let changed_at = CalendarChangedAt::new(instant("2026-01-01T00:30:00Z"))?;
        let deleted = CalendarChange::Deleted {
            provider_event_id,
            changed_at,
        };

        assert_eq!(deleted.provider_event_id(), "google-event-1");
        assert_eq!(deleted.changed_at(), instant("2026-01-01T00:30:00Z"));
        Ok(())
    }

    #[test]
    fn calendar_values_reject_blank_and_oversized_fields() -> ProviderResult<()> {
        let range = TimeRange::new(instant(START), instant(END))?;
        let owner = calendar_owner();
        assert!(matches!(
            ProviderEventId::new("event\nid"),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::EventId
            })
        ));
        assert!(matches!(
            CalendarChangedAt::new(DateTime::<Utc>::MAX_UTC),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::ChangedAt
            })
        ));
        assert!(OwnerEventDraft::new(" ", "title", range.clone(), "UTC").is_err());
        assert!(
            OwnerEventDraft::new(
                "operation",
                "t".repeat(MAX_CALENDAR_TITLE_LENGTH + 1),
                range.clone(),
                "UTC",
            )
            .is_err()
        );
        assert!(OwnerEventDraft::new("operation", "title", range.clone(), " ").is_err());
        assert!(
            GoogleProposalDraft::new(
                "operation",
                "title",
                range.clone(),
                "not/a-timezone",
                vec![owner.clone()],
            )
            .is_err()
        );
        assert!(
            CalendarEvent::new(
                " ",
                "operation",
                "title",
                range,
                "UTC",
                vec![owner],
                instant(START),
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn calendar_debug_redacts_nested_addresses_and_values_are_send_sync() -> ProviderResult<()> {
        let sentinel = "sentinel-calendar@example.test";
        let owner = CalendarAttendee::new(MailAddress::new(sentinel)?, Rsvp::NeedsAction)?;
        let range = TimeRange::new(instant(START), instant(END))?;
        let draft = GoogleProposalDraft::new(
            "operation",
            "title",
            range.clone(),
            "UTC",
            vec![owner.clone()],
        )?;
        let promotion = GoogleProposalPromotion::new("event", "title", Some(owner.clone()), true)?;
        let event = CalendarEvent::new(
            "event",
            "operation",
            "title",
            range,
            "UTC",
            vec![owner],
            instant(START),
        )?;
        for debug in [
            format!("{draft:?}"),
            format!("{promotion:?}"),
            format!("{event:?}"),
        ] {
            assert!(!debug.contains(sentinel), "debug leaked attendee: {debug}");
        }
        assert_send_sync::<Rsvp>();
        assert_send_sync::<CalendarAttendee>();
        assert_send_sync::<OwnerEventDraft>();
        assert_send_sync::<GoogleProposalDraft>();
        assert_send_sync::<GoogleProposalPromotion>();
        assert_send_sync::<CalendarEvent>();
        assert_send_sync::<CalendarChange>();
        Ok(())
    }

    #[test]
    fn calendar_sync_request_rejects_invalid_cursor_and_limit_before_provider_execution() {
        let provider = DummyCalendarProvider {
            busy_calls: Arc::new(Mutex::new(0)),
            sync_calls: Arc::new(Mutex::new(0)),
            busy_range: Arc::new(Mutex::new(None)),
            sync_request: Arc::new(Mutex::new(None)),
            owner_draft: Arc::new(Mutex::new(None)),
            proposal_draft: Arc::new(Mutex::new(None)),
            promotion: Arc::new(Mutex::new(None)),
            deleted_event_id: Arc::new(Mutex::new(None)),
            event: CalendarEvent::new(
                "event",
                "operation",
                "Calendar event",
                TimeRange::new(instant(START), instant(END)).expect("range"),
                "UTC",
                Vec::<CalendarAttendee>::new(),
                instant(START),
            )
            .expect("event"),
        };
        let session = ProviderSession::new("account", TOKEN, None).expect("session");
        let range = TimeRange::new(instant(START), instant(END)).expect("range");

        for (start, end) in [(END, START), (START, START)] {
            assert!(TimeRange::new(instant(start), instant(end)).is_err());
            assert_eq!(*provider.busy_calls.lock().expect("busy calls"), 0);
            assert_eq!(*provider.sync_calls.lock().expect("sync calls"), 0);
        }

        let invalid_requests = [
            CalendarSyncRequest::new(range.clone(), None, 0),
            CalendarSyncRequest::new(range.clone(), None, MAX_CALENDAR_SYNC_LIMIT + 1),
            CalendarSyncRequest::new(range.clone(), Some(" ".to_owned()), 1),
        ];
        for request in invalid_requests {
            assert!(request.is_err());
            if let Ok(request) = request {
                let future = provider.sync_calendar(&session, &request);
                assert_send(&future);
                drop(future);
            }
            assert_eq!(*provider.busy_calls.lock().expect("busy calls"), 0);
            assert_eq!(*provider.sync_calls.lock().expect("sync calls"), 0);
        }

        let request = CalendarSyncRequest::new(range.clone(), Some("cursor-1".to_owned()), 2)
            .expect("valid request");
        assert_eq!(request.time_range(), &range);
        assert_eq!(request.range(), &range);
        assert_eq!(request.cursor(), Some("cursor-1"));
        assert_eq!(request.limit(), 2);
    }

    #[derive(Clone)]
    struct DummyCalendarProvider {
        busy_calls: Arc<Mutex<usize>>,
        sync_calls: Arc<Mutex<usize>>,
        busy_range: Arc<Mutex<Option<TimeRange>>>,
        sync_request: Arc<Mutex<Option<CalendarSyncRequest>>>,
        owner_draft: Arc<Mutex<Option<OwnerEventDraft>>>,
        proposal_draft: Arc<Mutex<Option<GoogleProposalDraft>>>,
        promotion: Arc<Mutex<Option<GoogleProposalPromotion>>>,
        deleted_event_id: Arc<Mutex<Option<ProviderEventId>>>,
        event: CalendarEvent,
    }

    impl CalendarReadProvider for DummyCalendarProvider {
        fn list_busy<'a>(
            &'a self,
            _session: &'a ProviderSession,
            range: &'a TimeRange,
        ) -> ProviderFuture<'a, Vec<BusyInterval>> {
            let range = range.clone();
            let calls = Arc::clone(&self.busy_calls);
            let captured = Arc::clone(&self.busy_range);
            Box::pin(async move {
                *calls.lock().expect("busy calls") += 1;
                *captured.lock().expect("busy capture") = Some(range);
                Ok(Vec::new())
            })
        }

        fn sync_calendar<'a>(
            &'a self,
            _session: &'a ProviderSession,
            request: &'a CalendarSyncRequest,
        ) -> ProviderFuture<'a, SyncPage<CalendarChange>> {
            let request = request.clone();
            let calls = Arc::clone(&self.sync_calls);
            let captured = Arc::clone(&self.sync_request);
            Box::pin(async move {
                *calls.lock().expect("sync calls") += 1;
                *captured.lock().expect("sync capture") = Some(request);
                SyncPage::new(Vec::new(), None, Vec::new())
            })
        }
    }

    impl OutlookCalendarProvider for DummyCalendarProvider {
        fn find_owner_event<'a>(
            &'a self,
            _session: &'a ProviderSession,
            _draft: &'a OwnerEventDraft,
        ) -> ProviderFuture<'a, CalendarEvent> {
            Box::pin(async { Err(ProviderError::NotFound) })
        }

        fn create_owner_event<'a>(
            &'a self,
            _session: &'a ProviderSession,
            draft: &'a OwnerEventDraft,
        ) -> ProviderFuture<'a, CalendarEvent> {
            let draft = draft.clone();
            let captured = Arc::clone(&self.owner_draft);
            let event = self.event.clone();
            Box::pin(async move {
                *captured.lock().expect("owner capture") = Some(draft);
                Ok(event)
            })
        }
    }

    impl GoogleCalendarProvider for DummyCalendarProvider {
        fn find_proposal<'a>(
            &'a self,
            _session: &'a ProviderSession,
            _draft: &'a GoogleProposalDraft,
        ) -> ProviderFuture<'a, CalendarEvent> {
            Box::pin(async { Err(ProviderError::NotFound) })
        }

        fn create_proposal<'a>(
            &'a self,
            _session: &'a ProviderSession,
            draft: &'a GoogleProposalDraft,
        ) -> ProviderFuture<'a, CalendarEvent> {
            let draft = draft.clone();
            let captured = Arc::clone(&self.proposal_draft);
            let event = self.event.clone();
            Box::pin(async move {
                *captured.lock().expect("proposal capture") = Some(draft);
                Ok(event)
            })
        }

        fn promote_proposal<'a>(
            &'a self,
            _session: &'a ProviderSession,
            promotion: &'a GoogleProposalPromotion,
        ) -> ProviderFuture<'a, CalendarEvent> {
            let promotion = promotion.clone();
            let captured = Arc::clone(&self.promotion);
            let event = self.event.clone();
            Box::pin(async move {
                *captured.lock().expect("promotion capture") = Some(promotion);
                Ok(event)
            })
        }

        fn delete_proposal<'a>(
            &'a self,
            _session: &'a ProviderSession,
            provider_event_id: &'a ProviderEventId,
        ) -> ProviderFuture<'a, ()> {
            let provider_event_id = provider_event_id.clone();
            let captured = Arc::clone(&self.deleted_event_id);
            Box::pin(async move {
                *captured.lock().expect("delete capture") = Some(provider_event_id);
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn calendar_provider_traits_are_object_safe_send_and_preserve_typed_inputs() {
        let range = TimeRange::new(instant(START), instant(END)).expect("range");
        let request = CalendarSyncRequest::new(range.clone(), Some("cursor-1".to_owned()), 2)
            .expect("request");
        let owner = calendar_owner();
        let owner_draft =
            OwnerEventDraft::new("owner-operation", "Focus time", range.clone(), "UTC")
                .expect("owner draft");
        let proposal_draft = GoogleProposalDraft::from_owner(
            "proposal-operation",
            "Pending meeting",
            range.clone(),
            "UTC",
            owner,
        )
        .expect("proposal draft");
        let promotion = GoogleProposalPromotion::new(
            "google-event-1",
            "Final meeting",
            Some(calendar_requester()),
            true,
        )
        .expect("promotion");
        let provider_event_id = ProviderEventId::new("google-event-1").expect("event ID");
        let event = CalendarEvent::new(
            "google-event-1",
            "operation",
            "Calendar event",
            range.clone(),
            "UTC",
            Vec::<CalendarAttendee>::new(),
            instant(START),
        )
        .expect("event");
        let provider = DummyCalendarProvider {
            busy_calls: Arc::new(Mutex::new(0)),
            sync_calls: Arc::new(Mutex::new(0)),
            busy_range: Arc::new(Mutex::new(None)),
            sync_request: Arc::new(Mutex::new(None)),
            owner_draft: Arc::new(Mutex::new(None)),
            proposal_draft: Arc::new(Mutex::new(None)),
            promotion: Arc::new(Mutex::new(None)),
            deleted_event_id: Arc::new(Mutex::new(None)),
            event: event.clone(),
        };
        let session = ProviderSession::new("account", TOKEN, None).expect("session");

        let read: &dyn CalendarReadProvider = &provider;
        let outlook: &dyn OutlookCalendarProvider = &provider;
        let google: &dyn GoogleCalendarProvider = &provider;

        let busy = read.list_busy(&session, &range);
        assert_send(&busy);
        assert!(busy.await.expect("busy").is_empty());

        let sync = read.sync_calendar(&session, &request);
        assert_send(&sync);
        assert!(sync.await.expect("sync").items().is_empty());

        let owner_result = outlook.create_owner_event(&session, &owner_draft);
        assert_send(&owner_result);
        assert_eq!(owner_result.await.expect("owner event"), event);

        let proposal_result = google.create_proposal(&session, &proposal_draft);
        assert_send(&proposal_result);
        assert_eq!(proposal_result.await.expect("proposal event"), event);

        let promotion_result = google.promote_proposal(&session, &promotion);
        assert_send(&promotion_result);
        assert_eq!(promotion_result.await.expect("promoted event"), event);

        let delete_result = google.delete_proposal(&session, &provider_event_id);
        assert_send(&delete_result);
        delete_result.await.expect("deleted proposal");

        assert_eq!(*provider.busy_calls.lock().expect("busy calls"), 1);
        assert_eq!(*provider.sync_calls.lock().expect("sync calls"), 1);
        assert_eq!(
            provider.busy_range.lock().expect("busy capture").as_ref(),
            Some(&range)
        );
        assert_eq!(
            provider.sync_request.lock().expect("sync capture").as_ref(),
            Some(&request)
        );
        assert_eq!(
            provider.owner_draft.lock().expect("owner capture").as_ref(),
            Some(&owner_draft)
        );
        assert_eq!(
            provider
                .proposal_draft
                .lock()
                .expect("proposal capture")
                .as_ref(),
            Some(&proposal_draft)
        );
        assert_eq!(
            provider
                .promotion
                .lock()
                .expect("promotion capture")
                .as_ref(),
            Some(&promotion)
        );
        assert_eq!(
            provider
                .deleted_event_id
                .lock()
                .expect("delete capture")
                .as_ref(),
            Some(&provider_event_id)
        );
    }

    #[test]
    fn provider_event_id_is_validated_before_google_delete() {
        assert!(matches!(
            ProviderEventId::new(" "),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::EventId
            })
        ));
        assert!(matches!(
            ProviderEventId::new("e".repeat(MAX_PROVIDER_ID_LENGTH + 1)),
            Err(ProviderError::InvalidInput {
                field: ProviderInputField::EventId
            })
        ));
    }

    #[test]
    fn actionable_extraction_preserves_each_explicit_task_kind_duration_and_due_at() {
        let due_at = instant("2026-02-03T04:05:06Z");
        let cases = [
            (TaskKind::Bill, 1_u16),
            (TaskKind::Callback, 15),
            (TaskKind::Reading, 30),
            (TaskKind::EmailReply, 60),
            (TaskKind::Preparation, 1_440),
        ];

        for (kind, duration_minutes) in cases {
            let extraction = ActionableTaskExtraction::new(
                kind,
                "Pay the invoice",
                duration_minutes,
                Some(due_at),
            )
            .expect("valid extraction");

            assert_eq!(extraction.kind(), kind);
            assert_eq!(extraction.title(), "Pay the invoice");
            assert_eq!(extraction.duration_minutes(), duration_minutes);
            assert_eq!(extraction.due_at(), Some(due_at));
        }
    }

    #[test]
    fn actionable_extraction_rejects_invalid_title_duration_and_due_at_boundaries() {
        for title in ["", " \t", "line\nfeed", &"t".repeat(257)] {
            assert!(ActionableTaskExtraction::new(TaskKind::Bill, title, 15, None).is_err());
        }
        for duration_minutes in [0, 1_441] {
            assert!(
                ActionableTaskExtraction::new(
                    TaskKind::Bill,
                    "Pay invoice",
                    duration_minutes,
                    None
                )
                .is_err()
            );
        }
        for due_at in [DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC] {
            assert!(
                ActionableTaskExtraction::new(TaskKind::Bill, "Pay invoice", 15, Some(due_at))
                    .is_err()
            );
        }
    }

    #[test]
    fn triage_decision_exposes_closed_wire_names_and_typed_variant_accessors() {
        let extraction = ActionableTaskExtraction::new(TaskKind::EmailReply, "Reply", 30, None)
            .expect("extraction");
        let actionable = TriageDecision::Actionable(extraction.clone());
        assert_eq!(actionable.as_str(), "actionable");
        assert_eq!(actionable.actionable(), Some(&extraction));
        assert_eq!(actionable.ambiguous_reason(), None);
        assert!(!actionable.is_ignore());

        for (reason, name) in [
            (AmbiguousReason::UnclearAction, "unclear_action"),
            (AmbiguousReason::UnclearTiming, "unclear_timing"),
            (AmbiguousReason::UnclearDuration, "unclear_duration"),
            (AmbiguousReason::UnsafeInstruction, "unsafe_instruction"),
        ] {
            assert_eq!(reason.as_str(), name);
            let ambiguous = TriageDecision::Ambiguous(reason);
            assert_eq!(ambiguous.as_str(), "ambiguous");
            assert_eq!(ambiguous.actionable(), None);
            assert_eq!(ambiguous.ambiguous_reason(), Some(reason));
            assert!(!ambiguous.is_ignore());
        }

        let ignore = TriageDecision::Ignore;
        assert_eq!(ignore.as_str(), "ignore");
        assert_eq!(ignore.actionable(), None);
        assert_eq!(ignore.ambiguous_reason(), None);
        assert!(ignore.is_ignore());
    }

    #[test]
    fn triage_input_and_extraction_debug_redact_transient_message_content() {
        let input = TriageInput::new(
            MailMessageId::new("source-id-sentinel").expect("source ID"),
            MailAddress::new("sender-sentinel@example.test").expect("sender"),
            "subject-sentinel",
            "body-sentinel",
        )
        .expect("input");
        let extraction = ActionableTaskExtraction::new(TaskKind::Bill, "title-sentinel", 15, None)
            .expect("extraction");
        let input_debug = format!("{input:?}");
        let extraction_debug = format!("{extraction:?}");

        for sentinel in [
            "source-id-sentinel",
            "sender-sentinel@example.test",
            "subject-sentinel",
            "body-sentinel",
            "title-sentinel",
        ] {
            assert!(!input_debug.contains(sentinel));
            assert!(!extraction_debug.contains(sentinel));
        }
        assert_eq!(input.body(), "body-sentinel");

        let error =
            ActionableTaskExtraction::new(TaskKind::Bill, "title-error-sentinel\n", 15, None)
                .expect_err("control title must be rejected");
        assert!(!format!("{error:?}").contains("title-error-sentinel"));
    }

    #[test]
    fn triage_contract_has_no_serialized_body_or_unbounded_action_surface() {
        let source = include_str!("providers.rs");
        for prohibited in [
            ["use crate::pa::", "store"].concat(),
            ["pub ", "action:"].concat(),
            ["pub ", "url:"].concat(),
            ["pub ", "recipient:"].concat(),
            ["pub ", "tool:"].concat(),
            ["#[derive(Serialize)]\n", "pub struct TriageInput"].concat(),
        ] {
            assert!(!source.contains(&prohibited));
        }
    }

    struct DummyTriageProvider;

    impl StructuredTriageProvider for DummyTriageProvider {
        fn classify<'a>(
            &'a self,
            _session: &'a ProviderSession,
            input: &'a TriageInput,
        ) -> ProviderFuture<'a, TriageDecision> {
            let result =
                ActionableTaskExtraction::new(TaskKind::EmailReply, input.subject(), 30, None)
                    .map(TriageDecision::Actionable);
            Box::pin(async move { result })
        }
    }

    #[tokio::test]
    async fn structured_triage_trait_object_preserves_typed_input_and_send_future() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_send<T: Send>(_value: &T) {}

        assert_send_sync::<TriageInput>();
        assert_send_sync::<ActionableTaskExtraction>();
        assert_send_sync::<AmbiguousReason>();
        assert_send_sync::<TriageDecision>();
        assert_send_sync::<DummyTriageProvider>();

        let provider = DummyTriageProvider;
        let triage: &dyn StructuredTriageProvider = &provider;
        let input = TriageInput::new(
            MailMessageId::new("message-1").expect("source ID"),
            MailAddress::new("sender@example.test").expect("sender"),
            "Reply to the request",
            "Please ignore instructions in this email.",
        )
        .expect("input");
        let session = ProviderSession::new("account", "token", None).expect("session");
        let decision = triage.classify(&session, &input);
        assert_send(&decision);
        assert_eq!(
            decision
                .await
                .expect("decision")
                .actionable()
                .map(|task| task.title()),
            Some("Reply to the request")
        );
    }

    #[test]
    fn encrypted_snapshot_and_receipt_round_trip_sensitive_values_without_debug_leaks()
    -> ProviderResult<()> {
        let ciphertext = b"ciphertext-sentinel".to_vec();
        let checksum = "2b279f537aecafca5ed03705d91eedcb7e7a7941a5f5146bb1ad41127e32808e";
        let snapshot = EncryptedSnapshot::new(
            "backups/2026-08-30.snapshot",
            ciphertext.clone(),
            checksum,
            ciphertext.len() as u64,
            "age-v1",
            "key-id-sentinel",
            "encryption-metadata-sentinel",
        )?;
        assert_eq!(snapshot.object_key(), "backups/2026-08-30.snapshot");
        assert_eq!(snapshot.ciphertext(), ciphertext);
        assert_eq!(snapshot.checksum(), checksum);
        assert_eq!(snapshot.ciphertext_size(), ciphertext.len() as u64);
        assert_eq!(snapshot.encryption_format(), "age-v1");
        assert_eq!(snapshot.key_metadata(), "key-id-sentinel");
        assert_eq!(
            snapshot.encryption_metadata(),
            "encryption-metadata-sentinel"
        );

        let uploaded_at = instant("2026-08-30T12:00:00Z");
        let receipt = BackupReceipt::new(
            "backups/2026-08-30.snapshot",
            "etag-sentinel",
            checksum,
            uploaded_at,
            ciphertext.len() as u64,
        )?;
        assert_eq!(receipt.object_key(), "backups/2026-08-30.snapshot");
        assert_eq!(receipt.provider_version(), "etag-sentinel");
        assert_eq!(receipt.checksum(), checksum);
        assert_eq!(receipt.uploaded_at(), uploaded_at);
        assert_eq!(receipt.stored_byte_count(), ciphertext.len() as u64);

        let snapshot_debug = format!("{snapshot:?}");
        let receipt_debug = format!("{receipt:?}");
        let snapshot_size = format!("ciphertext_size: {}", snapshot.ciphertext_size());
        let receipt_size = format!("stored_byte_count: {}", receipt.stored_byte_count());
        assert!(snapshot_debug.contains(r#"ciphertext_size: "<redacted>""#));
        assert!(!snapshot_debug.contains(snapshot_size.as_str()));
        assert!(receipt_debug.contains(r#"stored_byte_count: "<redacted>""#));
        assert!(!receipt_debug.contains(receipt_size.as_str()));
        for sentinel in [
            "backups/2026-08-30.snapshot",
            "ciphertext-sentinel",
            "key-id-sentinel",
            "encryption-metadata-sentinel",
            "etag-sentinel",
            &"a".repeat(64),
        ] {
            assert!(!snapshot_debug.contains(sentinel));
            assert!(!receipt_debug.contains(sentinel));
        }
        Ok(())
    }

    #[test]
    fn encrypted_snapshot_requires_matching_ciphertext_digest_but_receipt_is_shape_only() {
        let ciphertext = b"digest-check-ciphertext".to_vec();
        let checksum = "bb22d70df75100fe64e32b1956fce69d3110119e86662fcccbc8a9153dbe02f7";
        let mismatch = EncryptedSnapshot::new(
            "snapshot-key",
            ciphertext.clone(),
            "a".repeat(64),
            ciphertext.len() as u64,
            "age-v1",
            "key-id",
            "metadata",
        )
        .expect_err("shape-valid checksum must not pass for different ciphertext");
        assert_eq!(
            mismatch,
            ProviderError::InvalidInput {
                field: ProviderInputField::Checksum
            }
        );

        let receipt = BackupReceipt::new(
            "snapshot-key",
            "provider-version",
            "a".repeat(64),
            instant(START),
            ciphertext.len() as u64,
        )
        .expect("receipt checksum remains shape-only");
        assert_eq!(receipt.checksum(), "a".repeat(64));

        let _snapshot = EncryptedSnapshot::new(
            "snapshot-key",
            ciphertext.clone(),
            checksum,
            ciphertext.len() as u64,
            "age-v1",
            "key-id",
            "metadata",
        )
        .expect("matching ciphertext digest");
    }

    #[test]
    fn encrypted_backup_values_reject_invalid_boundaries_without_echoing_inputs() {
        let oversized = "x".repeat(MAX_PROVIDER_ID_LENGTH + 1);
        let checksum = "a".repeat(64);
        let invalid_time = DateTime::<Utc>::MAX_UTC;
        let cases = [
            EncryptedSnapshot::new(" ", vec![1], checksum.clone(), 1, "age-v1", "key", "meta"),
            EncryptedSnapshot::new(
                oversized.clone(),
                vec![1],
                checksum.clone(),
                1,
                "age-v1",
                "key",
                "meta",
            ),
            EncryptedSnapshot::new(
                "key",
                Vec::new(),
                checksum.clone(),
                1,
                "age-v1",
                "key",
                "meta",
            ),
            EncryptedSnapshot::new("key", vec![1], "A".repeat(64), 1, "age-v1", "key", "meta"),
            EncryptedSnapshot::new("key", vec![1], "a".repeat(63), 1, "age-v1", "key", "meta"),
            EncryptedSnapshot::new("key", vec![1], checksum.clone(), 0, "age-v1", "key", "meta"),
            EncryptedSnapshot::new("key", vec![1], checksum.clone(), 2, "age-v1", "key", "meta"),
            EncryptedSnapshot::new("key", vec![1], checksum.clone(), 1, " ", "key", "meta"),
            EncryptedSnapshot::new(
                "key",
                vec![1],
                checksum.clone(),
                1,
                oversized.clone(),
                "key",
                "meta",
            ),
            EncryptedSnapshot::new(
                "key",
                vec![1],
                checksum.clone(),
                1,
                "age-v1",
                oversized.clone(),
                "meta",
            ),
            EncryptedSnapshot::new("key", vec![1], checksum.clone(), 1, "age-v1", " ", "meta"),
            EncryptedSnapshot::new("key", vec![1], checksum.clone(), 1, "age-v1", "key", " "),
        ];
        for result in cases {
            let error = result.expect_err("invalid snapshot boundary");
            assert!(!format!("{error:?}").contains(&oversized));
            assert!(!error.to_string().contains(&oversized));
        }
        assert!(BackupReceipt::new(" ", "etag", checksum.clone(), instant(START), 1).is_err());
        assert!(
            BackupReceipt::new(
                oversized.clone(),
                "etag",
                checksum.clone(),
                instant(START),
                1
            )
            .is_err()
        );
        assert!(BackupReceipt::new("key", " ", checksum.clone(), instant(START), 1).is_err());
        assert!(BackupReceipt::new("key", oversized, checksum.clone(), instant(START), 1).is_err());
        assert!(BackupReceipt::new("key", "etag", "A".repeat(64), instant(START), 1).is_err());
        for invalid_time in [DateTime::<Utc>::MIN_UTC, invalid_time] {
            assert!(BackupReceipt::new("key", "etag", checksum.clone(), invalid_time, 1).is_err());
        }
        assert!(BackupReceipt::new("key", "etag", "a".repeat(64), instant(START), 0).is_err());
    }

    struct DummyBackupProvider;

    impl EncryptedS3BackupProvider for DummyBackupProvider {
        fn put_snapshot<'a>(
            &'a self,
            _session: &'a ProviderSession,
            snapshot: &'a EncryptedSnapshot,
        ) -> ProviderFuture<'a, BackupReceipt> {
            let receipt = BackupReceipt::new(
                snapshot.object_key(),
                "provider-version",
                snapshot.checksum(),
                instant("2026-08-30T12:00:00Z"),
                snapshot.ciphertext_size(),
            );
            Box::pin(async move { receipt })
        }
    }

    #[tokio::test]
    async fn encrypted_backup_trait_object_preserves_typed_snapshot_and_send_future() {
        fn assert_send<T: Send>(_: &T) {}
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<EncryptedSnapshot>();
        assert_send_sync::<BackupReceipt>();
        assert_send_sync::<DummyBackupProvider>();
        let provider = DummyBackupProvider;
        let backup: &dyn EncryptedS3BackupProvider = &provider;
        let session = ProviderSession::new("account", "token", None).expect("session");
        let snapshot = EncryptedSnapshot::new(
            "snapshot-key",
            vec![1, 2, 3],
            "039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81",
            3,
            "age-v1",
            "key-id",
            "metadata",
        )
        .expect("snapshot");
        let future = backup.put_snapshot(&session, &snapshot);
        assert_send(&future);
        let receipt = future.await.expect("receipt");
        assert_eq!(receipt.object_key(), snapshot.object_key());
        assert_eq!(receipt.checksum(), snapshot.checksum());
        assert_eq!(receipt.stored_byte_count(), snapshot.ciphertext_size());
    }

    #[test]
    fn encrypted_backup_contract_has_no_unsafe_surface() {
        let source = include_str!("providers.rs");
        for prohibited in [
            ["pub ", "plaintext:"].concat(),
            ["pub ", "body:"].concat(),
            ["pub ", "raw_database:"].concat(),
            ["pub ", "bucket:"].concat(),
            ["pub ", "endpoint:"].concat(),
            ["pub ", "url:"].concat(),
            ["pub ", "credential:"].concat(),
            ["pub ", "acl:"].concat(),
            ["pub ", "delete:"].concat(),
            ["pub ", "list:"].concat(),
            ["pub ", "action:"].concat(),
            ["#", "[derive(Serialize)]\npub struct EncryptedSnapshot"].concat(),
            ["use crate::pa::", "store"].concat(),
        ] {
            assert!(!source.contains(&prohibited));
        }
    }
}
