//! SQLCipher-backed persistence boundary for personal-assistant state.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::time::Duration as StdDuration;

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use chrono_tz::Tz;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::de::Error as DeError;
use serde::{Deserialize, Serialize};
use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use super::auth::{REPLAY_RETENTION_SECONDS, ReplayGuard};
use super::crypto::{CryptoError, EncryptedSecret, TokenCipher};
use super::domain::{
    AppointmentDraft, AppointmentKind, AppointmentSlot, CallerIdentity, ConfirmedEmail,
    DurationMinutes, IdempotencyKey, OwnerTaskDraft, ProposalState, Quote, QuoteId, TaskKind,
};

/// Maximum UTF-8 byte length accepted for an audit entity identifier.
pub const MAX_AUDIT_ENTITY_ID_LENGTH: usize = 256;
/// Maximum UTF-8 byte length accepted for an audit idempotency key.
pub const MAX_AUDIT_IDEMPOTENCY_KEY_LENGTH: usize = 256;
/// Maximum number of audit events returned by one listing operation.
pub const MAX_AUDIT_LIST_LIMIT: usize = 100;

/// Maximum UTF-8 byte length accepted for message machine identifiers.
pub const MAX_MESSAGE_ID_LENGTH: usize = 256;
const MAX_PROVIDER_CURSOR_LENGTH: usize = MAX_MESSAGE_ID_LENGTH;
/// Maximum UTF-8 byte length accepted for a structured message summary.
pub const MAX_MESSAGE_SUMMARY_LENGTH: usize = 4096;
/// Maximum UTF-8 byte length accepted for an extracted message subject.
pub const MAX_MESSAGE_SUBJECT_LENGTH: usize = 256;
/// Maximum UTF-8 byte length accepted for an extracted message sender.
pub const MAX_MESSAGE_SENDER_LENGTH: usize = 320;
/// Maximum number of messages returned by one listing operation.
pub const MAX_MESSAGE_LIST_LIMIT: usize = 100;

/// Maximum UTF-8 byte length accepted for task machine identifiers.
pub const MAX_TASK_ID_LENGTH: usize = MAX_MESSAGE_ID_LENGTH;
/// Maximum UTF-8 byte length accepted for an extracted task title.
pub const MAX_TASK_TITLE_LENGTH: usize = MAX_MESSAGE_ID_LENGTH;
/// Maximum duration accepted for an extracted task, in whole minutes.
pub const MAX_TASK_DURATION_MINUTES: i64 = 24 * 60;
/// Maximum number of tasks returned by one state-listing operation.
pub const MAX_TASK_LIST_LIMIT: usize = 100;

/// Maximum number of frozen appointment slots retained for one quote.
pub const MAX_APPOINTMENT_QUOTE_SLOTS: usize = 100;

/// The closed set of providers that can produce persisted messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageProvider {
    /// A message summary produced from a voice call.
    Voice,
    /// A message read from Outlook.
    Outlook,
    /// A message read from Gmail.
    Gmail,
}

impl MessageProvider {
    /// Returns the canonical persisted representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Voice => "voice",
            Self::Outlook => "outlook",
            Self::Gmail => "gmail",
        }
    }

    /// Converts a persisted name, rejecting values outside the closed set.
    pub fn from_storage(value: &str) -> StoreResult<Self> {
        match value {
            "voice" => Ok(Self::Voice),
            "outlook" => Ok(Self::Outlook),
            "gmail" => Ok(Self::Gmail),
            _ => Err(stored_record_invalid("message provider")),
        }
    }
}

impl<'de> Deserialize<'de> for MessageProvider {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::from_storage(String::deserialize(deserializer)?.as_str()).map_err(D::Error::custom)
    }
}

impl std::str::FromStr for MessageProvider {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_storage(value)
    }
}

impl TryFrom<&str> for MessageProvider {
    type Error = StoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_storage(value)
    }
}

impl TryFrom<String> for MessageProvider {
    type Error = StoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_storage(&value)
    }
}

impl fmt::Display for MessageProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The closed set of persisted message triage states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageTriageState {
    /// A voice summary awaiting triage.
    Recorded,
    /// An email awaiting classification.
    Unprocessed,
    /// The message produced an actionable task.
    Actionable,
    /// The message requires owner clarification.
    Ambiguous,
    /// The message was intentionally ignored.
    Ignored,
    /// The message produced a scheduled task or appointment.
    Scheduled,
}

impl MessageTriageState {
    /// Returns the canonical persisted representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Unprocessed => "unprocessed",
            Self::Actionable => "actionable",
            Self::Ambiguous => "ambiguous",
            Self::Ignored => "ignored",
            Self::Scheduled => "scheduled",
        }
    }

    /// Converts a persisted name, rejecting values outside the closed set.
    pub fn from_storage(value: &str) -> StoreResult<Self> {
        match value {
            "recorded" => Ok(Self::Recorded),
            "unprocessed" => Ok(Self::Unprocessed),
            "actionable" => Ok(Self::Actionable),
            "ambiguous" => Ok(Self::Ambiguous),
            "ignored" => Ok(Self::Ignored),
            "scheduled" => Ok(Self::Scheduled),
            _ => Err(stored_record_invalid("message triage state")),
        }
    }
}

impl<'de> Deserialize<'de> for MessageTriageState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::from_storage(String::deserialize(deserializer)?.as_str()).map_err(D::Error::custom)
    }
}

impl std::str::FromStr for MessageTriageState {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_storage(value)
    }
}

impl TryFrom<&str> for MessageTriageState {
    type Error = StoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_storage(value)
    }
}

impl TryFrom<String> for MessageTriageState {
    type Error = StoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_storage(&value)
    }
}

impl fmt::Display for MessageTriageState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The closed set of persisted actionable-task states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredTaskState {
    /// The task has been extracted but has not yet been proposed.
    Pending,
    /// A calendar placement proposal exists for the task.
    Proposed,
    /// The task has been placed on the owner's calendar.
    Scheduled,
    /// No valid calendar slot was available for the task.
    NoSlot,
}

impl StoredTaskState {
    /// Returns the canonical persisted representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Proposed => "proposed",
            Self::Scheduled => "scheduled",
            Self::NoSlot => "no_slot",
        }
    }

    /// Converts a persisted name, rejecting values outside the closed set.
    pub fn from_storage(value: &str) -> StoreResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "proposed" => Ok(Self::Proposed),
            "scheduled" => Ok(Self::Scheduled),
            "no_slot" => Ok(Self::NoSlot),
            _ => Err(stored_record_invalid("task state")),
        }
    }
}

impl<'de> Deserialize<'de> for StoredTaskState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::from_storage(String::deserialize(deserializer)?.as_str()).map_err(D::Error::custom)
    }
}

impl std::str::FromStr for StoredTaskState {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_storage(value)
    }
}

impl TryFrom<&str> for StoredTaskState {
    type Error = StoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_storage(value)
    }
}

impl TryFrom<String> for StoredTaskState {
    type Error = StoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_storage(&value)
    }
}

impl fmt::Display for StoredTaskState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A validated task title produced by structured model output.
///
/// The value is private and redacted from ordinary debug output so model
/// content cannot accidentally enter logs.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TaskTitle(String);

impl TaskTitle {
    /// Constructs a validated task title.
    pub fn new(value: impl Into<String>) -> StoreResult<Self> {
        Ok(Self(validate_task_title(value.into())?))
    }

    /// Alias for [`Self::new`].
    pub fn try_new(value: impl Into<String>) -> StoreResult<Self> {
        Self::new(value)
    }

    /// Returns the validated title text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for TaskTitle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TaskTitle(<redacted>)")
    }
}

impl<'de> Deserialize<'de> for TaskTitle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl TryFrom<String> for TaskTitle {
    type Error = StoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TaskTitle> for String {
    fn from(value: TaskTitle) -> Self {
        value.0
    }
}

impl From<&TaskTitle> for TaskTitle {
    fn from(value: &TaskTitle) -> Self {
        value.clone()
    }
}

/// A validated, structured summary suitable for message persistence.
///
/// This value intentionally cannot contain a raw transcript or complete
/// email body. It rejects controls, embedded NULs, blank text, and values over
/// the byte bound used by the SQLite schema.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MessageSummary(String);

impl MessageSummary {
    /// Constructs a validated message summary.
    pub fn new(value: impl Into<String>) -> StoreResult<Self> {
        Ok(Self(validate_message_summary(value.into())?))
    }

    /// Alias for [`Self::new`].
    pub fn try_new(value: impl Into<String>) -> StoreResult<Self> {
        Self::new(value)
    }

    /// Returns the validated summary text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MessageSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MessageSummary(<redacted>)")
    }
}

impl<'de> Deserialize<'de> for MessageSummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl TryFrom<String> for MessageSummary {
    type Error = StoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MessageSummary> for String {
    fn from(value: MessageSummary) -> Self {
        value.0
    }
}

impl From<&MessageSummary> for MessageSummary {
    fn from(value: &MessageSummary) -> Self {
        value.clone()
    }
}

/// A strictly reconstructed provider message summary.
///
/// Raw email bodies and call transcripts are intentionally absent. Sensitive
/// identifiers and summary fields are redacted from the debug representation.
/// There are deliberately no `body` or `transcript` accessors:
///
/// ```compile_fail
/// use agent_voice::pa::StoredMessage;
///
/// let message: StoredMessage = todo!();
/// let _raw_body = message.body();
/// ```
///
/// ```compile_fail
/// use agent_voice::pa::StoredMessage;
///
/// let message: StoredMessage = todo!();
/// let _transcript = message.transcript();
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct StoredMessage {
    id: i64,
    idempotency_key: String,
    source_id: String,
    provider: MessageProvider,
    provider_message_id: String,
    summary: MessageSummary,
    subject: Option<String>,
    sender: Option<String>,
    received_at: OffsetDateTime,
    triage_state: MessageTriageState,
    created_at: String,
    updated_at: String,
}

impl StoredMessage {
    /// Returns the SQLite database ID.
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Alias for [`Self::id`] emphasizing that this is a database identity.
    pub const fn database_id(&self) -> i64 {
        self.id()
    }

    /// Returns the immutable operation idempotency key.
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    /// Returns the immutable source identity.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Returns the closed provider kind.
    pub const fn provider(&self) -> MessageProvider {
        self.provider
    }

    /// Returns the provider's immutable message identity.
    pub fn provider_message_id(&self) -> &str {
        &self.provider_message_id
    }

    /// Returns the validated structured summary.
    pub fn summary(&self) -> &MessageSummary {
        &self.summary
    }

    /// Returns the optional extracted subject.
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    /// Returns the optional extracted sender.
    pub fn sender(&self) -> Option<&str> {
        self.sender.as_deref()
    }

    /// Returns the canonical UTC whole-second receive time.
    pub const fn received_at(&self) -> OffsetDateTime {
        self.received_at
    }

    /// Returns the current message triage state.
    pub const fn triage_state(&self) -> MessageTriageState {
        self.triage_state
    }

    /// Alias for [`Self::triage_state`].
    pub const fn state(&self) -> MessageTriageState {
        self.triage_state()
    }

    /// Returns the stored creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Returns the stored last-update timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

impl fmt::Debug for StoredMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredMessage")
            .field("id", &self.id)
            .field("idempotency_key", &"<redacted>")
            .field("source_id", &"<redacted>")
            .field("provider", &self.provider)
            .field("provider_message_id", &"<redacted>")
            .field("summary", &"<redacted>")
            .field("subject", &"<redacted>")
            .field("sender", &"<redacted>")
            .field("received_at", &self.received_at)
            .field("triage_state", &self.triage_state)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// A strictly reconstructed actionable task extracted from one email.
///
/// The title and immutable identities are deliberately redacted from `Debug`.
/// Raw message content is not part of this value; the required message
/// reference is checked against the actionable email row at every storage
/// boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredTask {
    id: i64,
    idempotency_key: String,
    source_id: String,
    message_id: i64,
    title: TaskTitle,
    kind: TaskKind,
    duration: DurationMinutes,
    due_at: Option<OffsetDateTime>,
    state: StoredTaskState,
    created_at: String,
    updated_at: String,
}

impl StoredTask {
    /// Returns the SQLite database ID.
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Alias for [`Self::id`] emphasizing that this is a database identity.
    pub const fn database_id(&self) -> i64 {
        self.id()
    }

    /// Returns the immutable operation idempotency key.
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    /// Returns the immutable extraction source identity.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Returns the required source message database ID.
    pub const fn message_id(&self) -> i64 {
        self.message_id
    }

    /// Returns the validated structured task title.
    pub fn title(&self) -> &TaskTitle {
        &self.title
    }

    /// Returns the task category.
    pub const fn kind(&self) -> TaskKind {
        self.kind
    }

    /// Returns the validated task duration.
    pub const fn duration(&self) -> DurationMinutes {
        self.duration
    }

    /// Returns the task duration in whole minutes.
    pub const fn duration_minutes(&self) -> u32 {
        self.duration.minutes()
    }

    /// Returns the optional canonical UTC due time.
    pub const fn due_at(&self) -> Option<OffsetDateTime> {
        self.due_at
    }

    /// Returns the current task lifecycle state.
    pub const fn state(&self) -> StoredTaskState {
        self.state
    }

    /// Alias for [`Self::state`].
    pub const fn status(&self) -> StoredTaskState {
        self.state()
    }

    /// Returns the stored creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Returns the stored last-update timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

impl fmt::Debug for StoredTask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredTask")
            .field("id", &self.id)
            .field("idempotency_key", &"<redacted>")
            .field("source_id", &"<redacted>")
            .field("message_id", &"<redacted>")
            .field("title", &"<redacted>")
            .field("kind", &self.kind)
            .field("duration", &self.duration)
            .field("due_at", &self.due_at)
            .field("state", &self.state)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

pub(crate) const CURRENT_SCHEMA_VERSION: i64 = 15;
const BUSY_TIMEOUT: StdDuration = StdDuration::from_secs(5);

struct Migration {
    version: i64,
    apply: fn(&Transaction<'_>) -> StoreResult<()>,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        apply: apply_schema_v1,
    },
    Migration {
        version: 2,
        apply: apply_schema_v2,
    },
    Migration {
        version: 3,
        apply: apply_schema_v3,
    },
    Migration {
        version: 4,
        apply: apply_schema_v4,
    },
    Migration {
        version: 5,
        apply: apply_schema_v5,
    },
    Migration {
        version: 6,
        apply: apply_schema_v6,
    },
    Migration {
        version: 7,
        apply: apply_schema_v7,
    },
    Migration {
        version: 8,
        apply: apply_schema_v8,
    },
    Migration {
        version: 9,
        apply: apply_schema_v9,
    },
    Migration {
        version: 10,
        apply: apply_schema_v10,
    },
    Migration {
        version: 11,
        apply: apply_schema_v11,
    },
    Migration {
        version: 12,
        apply: apply_schema_v12,
    },
    Migration {
        version: 13,
        apply: apply_schema_v13,
    },
    Migration {
        version: 14,
        apply: apply_schema_v14,
    },
    Migration {
        version: CURRENT_SCHEMA_VERSION,
        apply: apply_schema_v15,
    },
];

/// Errors raised while opening or migrating the PA store.
#[derive(Debug)]
pub enum StoreError {
    /// The database key was empty and therefore could not protect the store.
    EmptyDatabaseKey,
    /// The linked SQLite library did not expose SQLCipher support.
    SqlCipherUnavailable,
    /// The database contains a migration newer than this binary understands.
    UnsupportedSchemaVersion(i64),
    /// A caller supplied a blank or otherwise invalid value.
    InvalidInput { field: &'static str },
    /// The requested record does not exist.
    NotFound { resource: &'static str },
    /// An appointment quote was used before its validity interval began.
    AppointmentQuoteNotYetValid,
    /// An appointment quote was used at or after its exclusive expiry instant.
    AppointmentQuoteExpired,
    /// A compare-and-set cursor write observed a different current value.
    CursorConflict { resource: &'static str },
    /// An immutable draft identity was already used for different content.
    Conflict { resource: &'static str },
    /// A credential envelope could not be serialized or deserialized safely.
    StoredValueInvalid,
    /// A persisted domain record failed strict reconstruction.
    StoredRecordInvalid { resource: &'static str },
    /// A credential cryptographic operation failed.
    Crypto(CryptoError),
    /// An underlying SQLite operation failed.
    Sqlite(rusqlite::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDatabaseKey => formatter.write_str("database key must not be empty"),
            Self::SqlCipherUnavailable => formatter.write_str("SQLCipher support is unavailable"),
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported database schema version {version}")
            }
            Self::InvalidInput { field } => write!(formatter, "{field} is invalid"),
            Self::NotFound { resource } => write!(formatter, "{resource} was not found"),
            Self::AppointmentQuoteNotYetValid => {
                formatter.write_str("appointment quote is not yet valid")
            }
            Self::AppointmentQuoteExpired => formatter.write_str("appointment quote has expired"),
            Self::CursorConflict { resource } => write!(formatter, "{resource} update conflicted"),
            Self::Conflict { resource } => {
                write!(formatter, "{resource} conflicts with an existing record")
            }
            Self::StoredValueInvalid => formatter.write_str("stored credential is invalid"),
            Self::StoredRecordInvalid { resource } => {
                write!(formatter, "stored {resource} is invalid")
            }
            Self::Crypto(_) => formatter.write_str("credential cryptographic operation failed"),
            Self::Sqlite(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::EmptyDatabaseKey
            | Self::SqlCipherUnavailable
            | Self::UnsupportedSchemaVersion(_)
            | Self::InvalidInput { .. }
            | Self::NotFound { .. }
            | Self::AppointmentQuoteNotYetValid
            | Self::AppointmentQuoteExpired
            | Self::CursorConflict { .. }
            | Self::Conflict { .. }
            | Self::StoredValueInvalid => None,
            Self::StoredRecordInvalid { .. } => None,
            Self::Crypto(error) => Some(error),
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// Result returned by PA store operations.
pub type StoreResult<T> = Result<T, StoreError>;

/// Validated OAuth credentials held by the PA store.
///
/// Access and refresh tokens are accepted here only as transient input. The
/// store encrypts them before writing and redacts both values from `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthCredential {
    /// Provider identity, for example `google` or `microsoft`.
    provider: String,
    /// Provider account identity.
    account_id: String,
    /// Short-lived or long-lived access token supplied by the provider.
    access_token: String,
    /// Optional refresh token supplied by the provider.
    refresh_token: Option<String>,
    /// Optional UTC expiry instant for the access token.
    expires_at: Option<DateTime<Utc>>,
    /// Required, normalized OAuth scopes.
    scopes: Vec<String>,
}

impl OAuthCredential {
    /// Constructs and validates an OAuth credential.
    pub fn new<I, S>(
        provider: impl Into<String>,
        account_id: impl Into<String>,
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        expires_at: Option<DateTime<Utc>>,
        scopes: I,
    ) -> StoreResult<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            provider: provider.into(),
            account_id: account_id.into(),
            access_token: access_token.into(),
            refresh_token,
            expires_at,
            scopes: scopes.into_iter().map(Into::into).collect(),
        }
        .validate_and_normalize()
    }

    /// Alias for [`Self::new`].
    pub fn try_new<I, S>(
        provider: impl Into<String>,
        account_id: impl Into<String>,
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        expires_at: Option<DateTime<Utc>>,
        scopes: I,
    ) -> StoreResult<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(
            provider,
            account_id,
            access_token,
            refresh_token,
            expires_at,
            scopes,
        )
    }

    /// Returns the validated provider identity.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Returns the validated provider account identity.
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Returns the access token for transient provider use.
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// Returns the optional refresh token for transient provider use.
    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    /// Returns the optional UTC expiry instant.
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
    }

    /// Returns the normalized, sorted, deduplicated scopes.
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    fn validate_and_normalize(mut self) -> StoreResult<Self> {
        self.provider = validate_oauth_identity(self.provider, "provider")?;
        self.account_id = validate_oauth_identity(self.account_id, "account_id")?;
        if self.access_token.trim().is_empty() {
            return Err(StoreError::InvalidInput {
                field: "access_token",
            });
        }
        if let Some(refresh_token) = &self.refresh_token
            && refresh_token.trim().is_empty()
        {
            return Err(StoreError::InvalidInput {
                field: "refresh_token",
            });
        }
        self.scopes = normalize_scopes(self.scopes)?;
        Ok(self)
    }
}

impl From<&OAuthCredential> for OAuthCredential {
    fn from(value: &OAuthCredential) -> Self {
        value.clone()
    }
}

impl fmt::Debug for OAuthCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthCredential")
            .field("provider", &self.provider)
            .field("account_id", &self.account_id)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// An appointment draft together with its database identity and source.
///
/// The contained draft is intentionally redacted from `Debug`: callers may
/// inspect it through [`Self::draft`] when they explicitly need the values,
/// but logging a stored record must not disclose caller identity data.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredAppointmentDraft {
    id: i64,
    source_id: String,
    draft: AppointmentDraft,
}

impl StoredAppointmentDraft {
    /// Returns the SQLite database ID.
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Alias for [`Self::id`] emphasizing that this is a database identity.
    pub const fn database_id(&self) -> i64 {
        self.id()
    }

    /// Returns the structured source identity supplied by the caller.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Returns the immutable appointment draft.
    pub fn draft(&self) -> &AppointmentDraft {
        &self.draft
    }

    /// Consumes this wrapper and returns its immutable appointment draft.
    pub fn into_draft(self) -> AppointmentDraft {
        self.draft
    }
}

impl fmt::Debug for StoredAppointmentDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredAppointmentDraft")
            .field("id", &self.id)
            .field("source_id", &self.source_id)
            .field("draft", &"<redacted>")
            .finish()
    }
}

/// The durable lifecycle state of an appointment quote.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StoredAppointmentQuoteState {
    /// The quote has been issued with its frozen offered slots.
    Issued,
    /// A slot was selected and an appointment draft was prepared.
    Prepared,
    /// The prepared appointment was consumed into a proposal.
    Consumed,
}

impl StoredAppointmentQuoteState {
    fn storage_name(self) -> &'static str {
        match self {
            Self::Issued => "issued",
            Self::Prepared => "prepared",
            Self::Consumed => "consumed",
        }
    }
}

impl fmt::Debug for StoredAppointmentQuoteState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Issued => "Issued",
            Self::Prepared => "Prepared",
            Self::Consumed => "Consumed",
        })
    }
}

/// A durable appointment quote and its optional prepared appointment state.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredAppointmentQuote {
    quote: Quote,
    appointment_kind: AppointmentKind,
    timezone: String,
    offered_slots: Vec<AppointmentSlot>,
    state: StoredAppointmentQuoteState,
    selected_slot_index: Option<u32>,
    appointment_draft: Option<StoredAppointmentDraft>,
    consumed_at: Option<OffsetDateTime>,
    proposal_id: Option<i64>,
}

impl StoredAppointmentQuote {
    /// Returns the immutable quote.
    pub fn quote(&self) -> &Quote {
        &self.quote
    }

    /// Returns the opaque quote identifier.
    pub const fn quote_id(&self) -> QuoteId {
        self.quote.id()
    }

    /// Returns the appointment kind whose slots were frozen.
    pub const fn appointment_kind(&self) -> AppointmentKind {
        self.appointment_kind
    }

    /// Returns the validated IANA timezone used when issuing the quote.
    pub fn timezone(&self) -> &str {
        &self.timezone
    }

    /// Returns the ordered frozen appointment slots.
    pub fn offered_slots(&self) -> &[AppointmentSlot] {
        &self.offered_slots
    }

    /// Returns the quote lifecycle state.
    pub const fn state(&self) -> StoredAppointmentQuoteState {
        self.state
    }

    /// Returns the selected slot's index when an appointment was prepared.
    pub const fn selected_slot_index(&self) -> Option<u32> {
        self.selected_slot_index
    }

    /// Returns the stored appointment draft wrapper, if one was prepared.
    pub fn appointment_draft(&self) -> Option<&StoredAppointmentDraft> {
        self.appointment_draft.as_ref()
    }

    /// Returns the prepared appointment draft database ID, if any.
    pub const fn appointment_draft_id(&self) -> Option<i64> {
        match &self.appointment_draft {
            Some(draft) => Some(draft.id()),
            None => None,
        }
    }

    /// Returns the immutable prepared appointment draft, if any.
    pub fn draft(&self) -> Option<&AppointmentDraft> {
        self.appointment_draft
            .as_ref()
            .map(StoredAppointmentDraft::draft)
    }

    /// Returns when the quote was consumed into a proposal, if it was consumed.
    pub const fn consumed_at(&self) -> Option<OffsetDateTime> {
        self.consumed_at
    }

    /// Returns the proposal database ID produced by consumption, if any.
    pub const fn proposal_id(&self) -> Option<i64> {
        self.proposal_id
    }
}

impl fmt::Debug for StoredAppointmentQuote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredAppointmentQuote")
            .field("offered_slot_count", &self.offered_slots.len())
            .field("state", &self.state)
            .field("selected_slot_index", &self.selected_slot_index)
            .field("appointment_draft_id", &self.appointment_draft_id())
            .field("proposal_id", &self.proposal_id)
            .finish()
    }
}

/// An owner task draft together with its database identity and optional
/// source. The draft is redacted from `Debug` to keep persisted records safe
/// for ordinary structured logging.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredOwnerTaskDraft {
    id: i64,
    source_id: Option<String>,
    draft: OwnerTaskDraft,
}

impl StoredOwnerTaskDraft {
    /// Returns the SQLite database ID.
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Alias for [`Self::id`] emphasizing that this is a database identity.
    pub const fn database_id(&self) -> i64 {
        self.id()
    }

    /// Returns the optional structured source identity.
    pub fn source_id(&self) -> Option<&str> {
        self.source_id.as_deref()
    }

    /// Returns the immutable owner task draft.
    pub fn draft(&self) -> &OwnerTaskDraft {
        &self.draft
    }

    /// Consumes this wrapper and returns its immutable owner task draft.
    pub fn into_draft(self) -> OwnerTaskDraft {
        self.draft
    }
}

impl fmt::Debug for StoredOwnerTaskDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredOwnerTaskDraft")
            .field("id", &self.id)
            .field("source_id", &self.source_id)
            .field("draft", &"<redacted>")
            .finish()
    }
}

/// Immutable direct-owner placement and its durable submission state.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredOwnerTaskPlacement {
    owner_task_draft_id: i64,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    timezone: String,
    operation_key: String,
    owner_fingerprint: String,
    provider_event_id: Option<String>,
}

impl StoredOwnerTaskPlacement {
    pub const fn owner_task_draft_id(&self) -> i64 {
        self.owner_task_draft_id
    }
    pub const fn starts_at(&self) -> OffsetDateTime {
        self.starts_at
    }
    pub const fn ends_at(&self) -> OffsetDateTime {
        self.ends_at
    }
    pub fn timezone(&self) -> &str {
        &self.timezone
    }
    pub fn operation_key(&self) -> &str {
        &self.operation_key
    }
    /// Returns the opaque caller fingerprint bound at preparation time.
    pub fn owner_fingerprint(&self) -> &str {
        &self.owner_fingerprint
    }
    pub fn provider_event_id(&self) -> Option<&str> {
        self.provider_event_id.as_deref()
    }
    pub const fn is_submitted(&self) -> bool {
        self.provider_event_id.is_some()
    }
}

impl fmt::Debug for StoredOwnerTaskPlacement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoredOwnerTaskPlacement(<redacted>)")
    }
}

/// The immutable draft that originated a proposal.
///
/// A proposal always has exactly one source. The source IDs are SQLite row
/// identities rather than user-controlled text, so they are checked for a
/// positive value at every storage boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProposalSource {
    /// A requester-involved appointment draft.
    AppointmentDraft { id: i64 },
    /// An owner task draft.
    OwnerTaskDraft { id: i64 },
}

impl ProposalSource {
    /// Constructs an appointment-draft source.
    pub const fn appointment_draft(id: i64) -> Self {
        Self::AppointmentDraft { id }
    }

    /// Constructs an owner-task-draft source.
    pub const fn owner_task_draft(id: i64) -> Self {
        Self::OwnerTaskDraft { id }
    }

    /// Constructs a source from nullable storage columns, rejecting neither
    /// and both source IDs as well as non-positive IDs.
    pub fn from_ids(
        appointment_draft_id: Option<i64>,
        owner_task_draft_id: Option<i64>,
    ) -> StoreResult<Self> {
        match (appointment_draft_id, owner_task_draft_id) {
            (Some(id), None) if id > 0 => Ok(Self::AppointmentDraft { id }),
            (None, Some(id)) if id > 0 => Ok(Self::OwnerTaskDraft { id }),
            _ => Err(StoreError::InvalidInput { field: "source" }),
        }
    }

    /// Returns the appointment-draft row ID, if this is an appointment source.
    pub const fn appointment_draft_id(self) -> Option<i64> {
        match self {
            Self::AppointmentDraft { id } => Some(id),
            Self::OwnerTaskDraft { .. } => None,
        }
    }

    /// Returns the owner-task-draft row ID, if this is an owner-task source.
    pub const fn owner_task_draft_id(self) -> Option<i64> {
        match self {
            Self::AppointmentDraft { .. } => None,
            Self::OwnerTaskDraft { id } => Some(id),
        }
    }
}

/// A proposal together with its immutable identity and current lifecycle
/// state. The record contains no caller message or transcript content.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredProposal {
    id: i64,
    idempotency_key: String,
    source_id: String,
    source: ProposalSource,
    state: ProposalState,
    created_at: String,
    updated_at: String,
}

impl StoredProposal {
    /// Returns the SQLite database ID.
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Alias for [`Self::id`].
    pub const fn database_id(&self) -> i64 {
        self.id()
    }

    /// Returns the immutable operation idempotency key.
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    /// Returns the immutable proposal source identity.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Returns the immutable draft source.
    pub const fn source(&self) -> ProposalSource {
        self.source
    }

    /// Returns the current lifecycle state.
    pub const fn state(&self) -> ProposalState {
        self.state
    }

    /// Returns the stored creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Returns the stored last-update timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

impl fmt::Debug for StoredProposal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredProposal")
            .field("id", &self.id)
            .field("idempotency_key", &"<redacted>")
            .field("source_id", &"<redacted>")
            .field("source", &"<redacted>")
            .field("state", &self.state)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// A provider event identity attached to one proposal.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredEventMapping {
    id: i64,
    proposal_id: i64,
    provider: String,
    provider_event_id: String,
    source_id: String,
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
    created_at: String,
    updated_at: String,
}

/// The closed set of auditable lifecycle events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// A provider message was recorded for later processing.
    MessageRecorded,
    /// A requester submitted an appointment request.
    RequestSubmitted,
    /// The owner submitted a task.
    OwnerTaskSubmitted,
    /// A proposal was created.
    ProposalCreated,
    /// A proposal was accepted.
    ProposalAccepted,
    /// A proposal was declined.
    ProposalDeclined,
    /// A proposal expired.
    ProposalExpired,
    /// A proposal was promoted to a provider event.
    ProposalPromoted,
    /// A notification was enqueued.
    NotificationEnqueued,
    /// A notification was sent.
    NotificationSent,
    /// A notification retry was scheduled.
    NotificationRetryScheduled,
    /// A provider cursor was advanced.
    ProviderCursorAdvanced,
}

impl AuditEventType {
    /// Returns the canonical persisted representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MessageRecorded => "message_recorded",
            Self::RequestSubmitted => "request_submitted",
            Self::OwnerTaskSubmitted => "owner_task_submitted",
            Self::ProposalCreated => "proposal_created",
            Self::ProposalAccepted => "proposal_accepted",
            Self::ProposalDeclined => "proposal_declined",
            Self::ProposalExpired => "proposal_expired",
            Self::ProposalPromoted => "proposal_promoted",
            Self::NotificationEnqueued => "notification_enqueued",
            Self::NotificationSent => "notification_sent",
            Self::NotificationRetryScheduled => "notification_retry_scheduled",
            Self::ProviderCursorAdvanced => "provider_cursor_advanced",
        }
    }

    /// Converts a persisted name, rejecting values outside the closed set.
    pub fn from_storage(value: &str) -> StoreResult<Self> {
        match value {
            "message_recorded" => Ok(Self::MessageRecorded),
            "request_submitted" => Ok(Self::RequestSubmitted),
            "owner_task_submitted" => Ok(Self::OwnerTaskSubmitted),
            "proposal_created" => Ok(Self::ProposalCreated),
            "proposal_accepted" => Ok(Self::ProposalAccepted),
            "proposal_declined" => Ok(Self::ProposalDeclined),
            "proposal_expired" => Ok(Self::ProposalExpired),
            "proposal_promoted" => Ok(Self::ProposalPromoted),
            "notification_enqueued" => Ok(Self::NotificationEnqueued),
            "notification_sent" => Ok(Self::NotificationSent),
            "notification_retry_scheduled" => Ok(Self::NotificationRetryScheduled),
            "provider_cursor_advanced" => Ok(Self::ProviderCursorAdvanced),
            _ => Err(stored_record_invalid("audit event")),
        }
    }
}

impl<'de> Deserialize<'de> for AuditEventType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::from_storage(String::deserialize(deserializer)?.as_str()).map_err(D::Error::custom)
    }
}

impl std::str::FromStr for AuditEventType {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_storage(value)
    }
}

impl TryFrom<&str> for AuditEventType {
    type Error = StoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_storage(value)
    }
}

impl TryFrom<String> for AuditEventType {
    type Error = StoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_storage(&value)
    }
}

impl fmt::Display for AuditEventType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The closed set of entity kinds that can appear in an audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEntityType {
    /// A recorded provider message.
    Message,
    /// An appointment request.
    AppointmentRequest,
    /// An owner task.
    OwnerTask,
    /// A proposal.
    Proposal,
    /// A notification.
    Notification,
    /// A provider cursor.
    ProviderCursor,
}

impl AuditEntityType {
    /// Returns the canonical persisted representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::AppointmentRequest => "appointment_request",
            Self::OwnerTask => "owner_task",
            Self::Proposal => "proposal",
            Self::Notification => "notification",
            Self::ProviderCursor => "provider_cursor",
        }
    }

    /// Converts a persisted name, rejecting values outside the closed set.
    pub fn from_storage(value: &str) -> StoreResult<Self> {
        match value {
            "message" => Ok(Self::Message),
            "appointment_request" => Ok(Self::AppointmentRequest),
            "owner_task" => Ok(Self::OwnerTask),
            "proposal" => Ok(Self::Proposal),
            "notification" => Ok(Self::Notification),
            "provider_cursor" => Ok(Self::ProviderCursor),
            _ => Err(stored_record_invalid("audit entity")),
        }
    }
}

impl<'de> Deserialize<'de> for AuditEntityType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::from_storage(String::deserialize(deserializer)?.as_str()).map_err(D::Error::custom)
    }
}

impl std::str::FromStr for AuditEntityType {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_storage(value)
    }
}

impl TryFrom<&str> for AuditEntityType {
    type Error = StoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_storage(value)
    }
}

impl TryFrom<String> for AuditEntityType {
    type Error = StoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_storage(&value)
    }
}

impl fmt::Display for AuditEntityType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A strictly reconstructed, immutable audit event.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredAuditEvent {
    id: i64,
    idempotency_key: String,
    event_type: AuditEventType,
    entity_type: AuditEntityType,
    entity_id: String,
    occurred_at: OffsetDateTime,
    created_at: String,
}

impl StoredAuditEvent {
    /// Returns the SQLite database ID.
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Alias for [`Self::id`].
    pub const fn database_id(&self) -> i64 {
        self.id()
    }

    /// Returns the immutable operation idempotency key.
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    /// Returns the closed event kind.
    pub const fn event_type(&self) -> AuditEventType {
        self.event_type
    }

    /// Alias for [`Self::event_type`].
    pub const fn event_kind(&self) -> AuditEventType {
        self.event_type()
    }

    /// Returns the closed entity kind.
    pub const fn entity_type(&self) -> AuditEntityType {
        self.entity_type
    }

    /// Alias for [`Self::entity_type`].
    pub const fn entity_kind(&self) -> AuditEntityType {
        self.entity_type()
    }

    /// Returns the validated entity identifier.
    pub fn entity_id(&self) -> &str {
        &self.entity_id
    }

    /// Returns the UTC occurrence time supplied by the caller.
    pub const fn occurred_at(&self) -> OffsetDateTime {
        self.occurred_at
    }

    /// Returns the stored creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

impl fmt::Debug for StoredAuditEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredAuditEvent")
            .field("id", &self.id)
            .field("idempotency_key", &"<redacted>")
            .field("event_type", &self.event_type)
            .field("entity_type", &self.entity_type)
            .field("entity_id", &"<redacted>")
            .field("occurred_at", &self.occurred_at)
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// Alias emphasizing that the stored row is an audit entry.
pub type StoredAuditEntry = StoredAuditEvent;

/// Compatibility alias emphasizing that audit values are closed kinds.
pub type AuditEventKind = AuditEventType;
/// Compatibility alias emphasizing that audit values are closed kinds.
pub type AuditEntityKind = AuditEntityType;

impl StoredEventMapping {
    /// Returns the SQLite database ID.
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Returns the owning proposal database ID.
    pub const fn proposal_id(&self) -> i64 {
        self.proposal_id
    }

    /// Returns the provider name.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Returns the provider's immutable event ID.
    pub fn provider_event_id(&self) -> &str {
        &self.provider_event_id
    }

    /// Returns the immutable event source identity.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Returns the optional event start instant.
    pub const fn starts_at(&self) -> Option<OffsetDateTime> {
        self.starts_at
    }

    /// Alias for [`Self::starts_at`].
    pub const fn start_at(&self) -> Option<OffsetDateTime> {
        self.starts_at()
    }

    /// Returns the optional event end instant.
    pub const fn ends_at(&self) -> Option<OffsetDateTime> {
        self.ends_at
    }

    /// Alias for [`Self::ends_at`].
    pub const fn end_at(&self) -> Option<OffsetDateTime> {
        self.ends_at()
    }

    /// Returns the stored creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Returns the stored last-update timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

impl fmt::Debug for StoredEventMapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredEventMapping")
            .field("id", &self.id)
            .field("proposal_id", &self.proposal_id)
            .field("provider", &"<redacted>")
            .field("provider_event_id", &"<redacted>")
            .field("source_id", &"<redacted>")
            .field("starts_at", &self.starts_at)
            .field("ends_at", &self.ends_at)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// The only notification templates supported by the personal assistant.
///
/// The enum is deliberately closed: callers select one of these templates and
/// provide structured values through [`NotificationTemplateData`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// A summary of one completed voice call.
    CallSummary,
    /// A requester-involved proposal was created.
    ProposalRequested,
    /// A requester accepted a proposal.
    ProposalAccepted,
    /// A requester declined a proposal.
    ProposalDeclined,
    /// A proposal expired before the requester responded.
    ProposalExpired,
}

impl NotificationKind {
    fn storage_name(self) -> &'static str {
        match self {
            Self::CallSummary => "call_summary",
            Self::ProposalRequested => "proposal_requested",
            Self::ProposalAccepted => "proposal_accepted",
            Self::ProposalDeclined => "proposal_declined",
            Self::ProposalExpired => "proposal_expired",
        }
    }

    fn requires_proposal(self) -> bool {
        !matches!(self, Self::CallSummary)
    }
}

/// A validated recipient address for a notification.
///
/// This value is intentionally separate from an arbitrary string so that the
/// outbox boundary cannot accidentally persist an unvalidated recipient.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct NotificationRecipient(String);

impl NotificationRecipient {
    /// Validates and constructs a recipient email address.
    pub fn new(value: impl Into<String>) -> StoreResult<Self> {
        let value = value.into();
        ConfirmedEmail::confirm(value)
            .map(|email| Self(email.as_str().to_owned()))
            .map_err(|_| StoreError::InvalidInput { field: "recipient" })
    }

    /// Alias for [`Self::new`].
    pub fn try_new(value: impl Into<String>) -> StoreResult<Self> {
        Self::new(value)
    }

    /// Alias for [`Self::new`] emphasizing the confirmation boundary.
    pub fn confirm(value: impl Into<String>) -> StoreResult<Self> {
        Self::new(value)
    }

    /// Returns the validated recipient address.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NotificationRecipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("NotificationRecipient")
            .field(&"<redacted>")
            .finish()
    }
}

impl<'de> Deserialize<'de> for NotificationRecipient {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(D::Error::custom(
            "notification recipient must be constructed through validation",
        ))
    }
}

impl From<ConfirmedEmail> for NotificationRecipient {
    fn from(value: ConfirmedEmail) -> Self {
        Self(value.as_str().to_owned())
    }
}

impl From<NotificationRecipient> for String {
    fn from(value: NotificationRecipient) -> Self {
        value.0
    }
}

/// Structured values consumed by a notification template.
///
/// This type intentionally has no message body, transcript, token, provider
/// payload, or free-form JSON field. Appointment times, when present, must be
/// UTC and must be supplied as an ordered pair.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NotificationTemplateData {
    title: Option<String>,
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
    timezone: Option<String>,
    appointment_kind: Option<AppointmentKind>,
    proposal_state: Option<ProposalState>,
}

impl NotificationTemplateData {
    /// Constructs validated structured template values.
    pub fn new(
        title: Option<String>,
        starts_at: Option<OffsetDateTime>,
        ends_at: Option<OffsetDateTime>,
        timezone: Option<String>,
        appointment_kind: Option<AppointmentKind>,
        proposal_state: Option<ProposalState>,
    ) -> StoreResult<Self> {
        if title.as_ref().is_some_and(|value| value.trim().is_empty()) {
            return Err(StoreError::InvalidInput { field: "title" });
        }
        if timezone
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(StoreError::InvalidInput { field: "timezone" });
        }
        if starts_at.is_some() != ends_at.is_some() {
            return Err(StoreError::InvalidInput {
                field: "template_times",
            });
        }
        if let Some(start) = starts_at
            && start.offset() != time::UtcOffset::UTC
        {
            return Err(StoreError::InvalidInput { field: "starts_at" });
        }
        if let Some(end) = ends_at
            && end.offset() != time::UtcOffset::UTC
        {
            return Err(StoreError::InvalidInput { field: "ends_at" });
        }
        if matches!((starts_at, ends_at), (Some(start), Some(end)) if start >= end) {
            return Err(StoreError::InvalidInput {
                field: "template_times",
            });
        }
        Ok(Self {
            title: title.map(|value| value.trim().to_owned()),
            starts_at,
            ends_at,
            timezone: timezone.map(|value| value.trim().to_owned()),
            appointment_kind,
            proposal_state,
        })
    }

    /// Returns the optional appointment or task title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Alias for [`Self::title`].
    pub fn appointment_or_task_title(&self) -> Option<&str> {
        self.title()
    }

    /// Returns the optional UTC start instant.
    pub const fn starts_at(&self) -> Option<OffsetDateTime> {
        self.starts_at
    }

    /// Alias for [`Self::starts_at`].
    pub const fn start_at(&self) -> Option<OffsetDateTime> {
        self.starts_at()
    }

    /// Returns the optional UTC end instant.
    pub const fn ends_at(&self) -> Option<OffsetDateTime> {
        self.ends_at
    }

    /// Alias for [`Self::ends_at`].
    pub const fn end_at(&self) -> Option<OffsetDateTime> {
        self.ends_at()
    }

    /// Returns the optional display timezone.
    pub fn timezone(&self) -> Option<&str> {
        self.timezone.as_deref()
    }

    /// Returns the optional appointment kind.
    pub const fn appointment_kind(&self) -> Option<AppointmentKind> {
        self.appointment_kind
    }

    /// Returns the optional proposal lifecycle state.
    pub const fn proposal_state(&self) -> Option<ProposalState> {
        self.proposal_state
    }
}

impl<'de> Deserialize<'de> for NotificationTemplateData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            title: Option<String>,
            starts_at: Option<OffsetDateTime>,
            ends_at: Option<OffsetDateTime>,
            timezone: Option<String>,
            appointment_kind: Option<AppointmentKind>,
            proposal_state: Option<ProposalState>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(
            wire.title,
            wire.starts_at,
            wire.ends_at,
            wire.timezone,
            wire.appointment_kind,
            wire.proposal_state,
        )
        .map_err(D::Error::custom)
    }
}

impl fmt::Debug for NotificationTemplateData {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NotificationTemplateData")
            .field("title", &"<redacted>")
            .field("starts_at", &self.starts_at)
            .field("ends_at", &self.ends_at)
            .field("timezone", &self.timezone)
            .field("appointment_kind", &self.appointment_kind)
            .field("proposal_state", &self.proposal_state)
            .finish()
    }
}

/// The closed delivery state for a notification outbox record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationStatus {
    /// The notification is eligible for a future delivery claim.
    Pending,
    /// A worker owns the current attempt until its lease expires.
    Delivering,
    /// The notification was delivered and is terminal.
    Sent,
}

impl NotificationStatus {
    /// Returns the stable storage and wire representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivering => "delivering",
            Self::Sent => "sent",
        }
    }
}

impl fmt::Display for NotificationStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A notification outbox row with all persisted values reconstructed into
/// typed values. Recipient and template data are redacted by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredNotification {
    id: i64,
    idempotency_key: String,
    proposal_id: Option<i64>,
    event_mapping_id: Option<i64>,
    kind: NotificationKind,
    recipient: NotificationRecipient,
    template_data: NotificationTemplateData,
    status: NotificationStatus,
    available_at: OffsetDateTime,
    lease_until: Option<OffsetDateTime>,
    sent_at: Option<OffsetDateTime>,
    last_error_code: Option<String>,
    attempts: i64,
    created_at: String,
    updated_at: String,
}

impl StoredNotification {
    /// Returns the SQLite database ID.
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Returns the immutable notification idempotency key.
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    /// Returns the optional owning proposal ID.
    pub const fn proposal_id(&self) -> Option<i64> {
        self.proposal_id
    }

    /// Returns the optional owning event mapping ID.
    pub const fn event_mapping_id(&self) -> Option<i64> {
        self.event_mapping_id
    }

    /// Returns the selected notification kind.
    pub const fn kind(&self) -> NotificationKind {
        self.kind
    }

    /// Returns the validated recipient address.
    pub fn recipient(&self) -> &NotificationRecipient {
        &self.recipient
    }

    /// Returns the structured template data.
    pub fn template_data(&self) -> &NotificationTemplateData {
        &self.template_data
    }

    /// Returns the current delivery status.
    pub const fn status(&self) -> NotificationStatus {
        self.status
    }

    /// Returns the next available delivery instant.
    pub const fn available_at(&self) -> OffsetDateTime {
        self.available_at
    }

    /// Returns the expiry of the active delivery lease, when one exists.
    pub const fn lease_until(&self) -> Option<OffsetDateTime> {
        self.lease_until
    }

    /// Returns the stable delivery timestamp for a sent notification.
    pub const fn sent_at(&self) -> Option<OffsetDateTime> {
        self.sent_at
    }

    /// Returns the validated machine-readable error code from the last retry.
    pub fn last_error_code(&self) -> Option<&str> {
        self.last_error_code.as_deref()
    }

    /// Returns the number of delivery attempts recorded so far.
    pub const fn attempts(&self) -> i64 {
        self.attempts
    }

    /// Returns the stored creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Returns the stored last-update timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

impl fmt::Debug for StoredNotification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredNotification")
            .field("id", &self.id)
            .field("idempotency_key", &"<redacted>")
            .field("proposal_id", &self.proposal_id)
            .field("event_mapping_id", &self.event_mapping_id)
            .field("kind", &self.kind)
            .field("recipient", &"<redacted>")
            .field("payload", &"<redacted>")
            .field("status", &self.status)
            .field("available_at", &self.available_at)
            .field("lease_until", &self.lease_until)
            .field("sent_at", &self.sent_at)
            .field("last_error_code", &self.last_error_code)
            .field("attempts", &self.attempts)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Alias emphasizing that the stored row belongs to the notification outbox.
pub type StoredNotificationOutbox = StoredNotification;

/// Short alias for the validated notification recipient.
pub type RecipientEmail = NotificationRecipient;

/// Alias for callers that name the recipient by its notification role.
pub type NotificationRecipientEmail = NotificationRecipient;

/// An encrypted SQLite connection containing the PA schema.
#[derive(Debug)]
pub struct PaStore {
    connection: Connection,
}

impl PaStore {
    /// Opens or creates an encrypted file-backed store and runs migrations.
    pub fn open<P, K>(path: P, database_key: K) -> StoreResult<Self>
    where
        P: AsRef<Path>,
        K: AsRef<[u8]>,
    {
        let key = database_key.as_ref();
        reject_empty_key(key)?;
        let mut connection = Connection::open(path)?;
        initialize(&mut connection, key, true, true)?;
        Ok(Self { connection })
    }

    /// Opens an existing encrypted file-backed store without running migrations.
    #[allow(dead_code)]
    pub(crate) fn open_existing<P, K>(path: P, database_key: K) -> StoreResult<Self>
    where
        P: AsRef<Path>,
        K: AsRef<[u8]>,
    {
        let key = database_key.as_ref();
        reject_empty_key(key)?;
        let path = path.as_ref();
        if !path.is_file() || rollback_journal_is_hot(&rollback_journal_path(path)) {
            return Err(StoreError::NotFound {
                resource: "database",
            });
        }
        let immutable_uri = immutable_read_only_uri(path)?;
        let mut connection = Connection::open_with_flags(
            immutable_uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|_| StoreError::NotFound {
            resource: "database",
        })?;
        initialize(&mut connection, key, false, false)?;
        connection.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(Self { connection })
    }

    /// Opens an encrypted in-memory store and runs migrations.
    pub fn open_in_memory<K>(database_key: K) -> StoreResult<Self>
    where
        K: AsRef<[u8]>,
    {
        let key = database_key.as_ref();
        reject_empty_key(key)?;
        let mut connection = Connection::open_in_memory()?;
        initialize(&mut connection, key, false, true)?;
        Ok(Self { connection })
    }

    /// Returns the underlying connection for the later repository layer.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Atomically checks and consumes a validated replay nonce.
    ///
    /// Expired rows are removed before the insert. The immediate transaction
    /// serializes callers across independently opened stores, while the
    /// unique nonce constraint turns a duplicate into a normal `Ok(false)`.
    pub fn consume_replay_nonce(&self, nonce: &str, now: i64) -> StoreResult<bool> {
        validate_replay_nonce(nonce)?;
        let (consumed_at, expires_at) = replay_timestamps(now)?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        transaction.execute(
            "DELETE FROM replay_nonces WHERE expires_at <= ?1",
            [&consumed_at],
        )?;
        let inserted = transaction.execute(
            "INSERT INTO replay_nonces (nonce, consumed_at, expires_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(nonce) DO NOTHING",
            params![nonce, consumed_at, expires_at],
        )?;
        transaction.commit()?;
        Ok(inserted == 1)
    }

    /// Encrypts and upserts one OAuth credential by provider/account pair.
    pub fn save_oauth_credential<C>(&self, cipher: &TokenCipher, credential: C) -> StoreResult<()>
    where
        C: Into<OAuthCredential>,
    {
        let credential = credential.into().validate_and_normalize()?;
        let access_context = oauth_context(&credential.provider, &credential.account_id, "access");
        let access_envelope = cipher
            .encrypt(&credential.access_token, access_context.as_bytes())
            .map_err(StoreError::Crypto)?;
        let access_ciphertext = serialize_envelope(&access_envelope)?;
        let refresh_ciphertext = credential
            .refresh_token
            .as_ref()
            .map(|refresh_token| {
                let refresh_context =
                    oauth_context(&credential.provider, &credential.account_id, "refresh");
                cipher
                    .encrypt(refresh_token, refresh_context.as_bytes())
                    .map_err(StoreError::Crypto)
                    .and_then(|envelope| serialize_envelope(&envelope))
            })
            .transpose()?;
        let expires_at = credential
            .expires_at
            .as_ref()
            .map(|expires_at| expires_at.to_rfc3339_opts(SecondsFormat::AutoSi, true));
        let scopes = serde_json::to_string(&credential.scopes)
            .map_err(|_| StoreError::StoredValueInvalid)?;

        self.connection.execute(
            "INSERT INTO oauth_credentials (
                 provider, account_id, access_token_ciphertext,
                 refresh_token_ciphertext, expires_at, scopes
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(provider, account_id) DO UPDATE SET
                 access_token_ciphertext = excluded.access_token_ciphertext,
                 refresh_token_ciphertext = excluded.refresh_token_ciphertext,
                 expires_at = excluded.expires_at,
                 scopes = excluded.scopes,
                 updated_at = CURRENT_TIMESTAMP",
            params![
                credential.provider,
                credential.account_id,
                access_ciphertext,
                refresh_ciphertext,
                expires_at,
                scopes,
            ],
        )?;
        Ok(())
    }

    /// Atomically updates OAuth tokens while preserving an omitted refresh token.
    #[allow(clippy::too_many_arguments)]
    pub fn update_oauth_tokens(
        &self,
        cipher: &TokenCipher,
        provider: &str,
        account_id: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: DateTime<Utc>,
        scopes: &[String],
    ) -> StoreResult<()> {
        let provider = validate_oauth_identity(provider.to_owned(), "provider")?;
        let account_id = validate_oauth_identity(account_id.to_owned(), "account_id")?;
        if access_token.trim().is_empty() {
            return Err(StoreError::InvalidInput {
                field: "access_token",
            });
        }
        if let Some(refresh_token) = refresh_token
            && refresh_token.trim().is_empty()
        {
            return Err(StoreError::InvalidInput {
                field: "refresh_token",
            });
        }
        let scopes = normalize_scopes(scopes.to_vec())?;
        let expires_at = expires_at.to_rfc3339_opts(SecondsFormat::AutoSi, true);
        let scopes = serde_json::to_string(&scopes).map_err(|_| StoreError::StoredValueInvalid)?;

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let access_context = oauth_context(&provider, &account_id, "access");
        let access_ciphertext = cipher
            .encrypt(access_token, access_context.as_bytes())
            .map_err(StoreError::Crypto)
            .and_then(|envelope| serialize_envelope(&envelope))?;
        let refresh_ciphertext = match refresh_token {
            Some(refresh_token) => {
                let refresh_context = oauth_context(&provider, &account_id, "refresh");
                Some(
                    cipher
                        .encrypt(refresh_token, refresh_context.as_bytes())
                        .map_err(StoreError::Crypto)
                        .and_then(|envelope| serialize_envelope(&envelope))?,
                )
            }
            None => transaction
                .query_row(
                    "SELECT refresh_token_ciphertext FROM oauth_credentials
                     WHERE provider = ?1 AND account_id = ?2",
                    params![&provider, &account_id],
                    |row| row.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()?
                .flatten(),
        };

        transaction.execute(
            "INSERT INTO oauth_credentials (
                 provider, account_id, access_token_ciphertext,
                 refresh_token_ciphertext, expires_at, scopes
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(provider, account_id) DO UPDATE SET
                 access_token_ciphertext = excluded.access_token_ciphertext,
                 refresh_token_ciphertext = CASE
                     WHEN excluded.refresh_token_ciphertext IS NULL
                     THEN oauth_credentials.refresh_token_ciphertext
                     ELSE excluded.refresh_token_ciphertext
                 END,
                 expires_at = excluded.expires_at,
                 scopes = excluded.scopes,
                 updated_at = CURRENT_TIMESTAMP",
            params![
                provider,
                account_id,
                access_ciphertext,
                refresh_ciphertext,
                expires_at,
                scopes,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Decrypts and returns one OAuth credential by provider/account pair.
    pub fn load_oauth_credential(
        &self,
        cipher: &TokenCipher,
        provider: impl AsRef<str>,
        account_id: impl AsRef<str>,
    ) -> StoreResult<OAuthCredential> {
        let provider = validate_oauth_identity(provider.as_ref().to_owned(), "provider")?;
        let account_id = validate_oauth_identity(account_id.as_ref().to_owned(), "account_id")?;
        let row = self
            .connection
            .query_row(
                "SELECT access_token_ciphertext, refresh_token_ciphertext, expires_at, scopes
                 FROM oauth_credentials WHERE provider = ?1 AND account_id = ?2",
                params![&provider, &account_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StoreError::NotFound {
                resource: "oauth credential",
            })?;

        let access_envelope = deserialize_envelope(&row.0)?;
        let access_context = oauth_context(&provider, &account_id, "access");
        let access_token = cipher
            .decrypt(&access_envelope, access_context.as_bytes())
            .map_err(StoreError::Crypto)
            .and_then(|token| {
                String::from_utf8(token).map_err(|_| StoreError::StoredValueInvalid)
            })?;
        let refresh_token = row
            .1
            .as_deref()
            .map(deserialize_envelope)
            .transpose()?
            .map(|envelope| {
                let refresh_context = oauth_context(&provider, &account_id, "refresh");
                cipher
                    .decrypt(&envelope, refresh_context.as_bytes())
                    .map_err(StoreError::Crypto)
                    .and_then(|token| {
                        String::from_utf8(token).map_err(|_| StoreError::StoredValueInvalid)
                    })
            })
            .transpose()?;
        let expires_at = row
            .2
            .as_deref()
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .map_err(|_| StoreError::StoredValueInvalid)?
            .map(|value| value.with_timezone(&Utc));
        let scopes = serde_json::from_str::<Vec<String>>(&row.3)
            .map_err(|_| StoreError::StoredValueInvalid)?;

        OAuthCredential::new(
            provider,
            account_id,
            access_token,
            refresh_token,
            expires_at,
            scopes,
        )
    }

    /// Deletes one OAuth credential by provider/account pair.
    pub fn delete_oauth_credential(
        &self,
        provider: impl AsRef<str>,
        account_id: impl AsRef<str>,
    ) -> StoreResult<()> {
        let provider = validate_oauth_identity(provider.as_ref().to_owned(), "provider")?;
        let account_id = validate_oauth_identity(account_id.as_ref().to_owned(), "account_id")?;
        let deleted = self.connection.execute(
            "DELETE FROM oauth_credentials WHERE provider = ?1 AND account_id = ?2",
            params![provider, account_id],
        )?;
        if deleted == 0 {
            return Err(StoreError::NotFound {
                resource: "oauth credential",
            });
        }
        Ok(())
    }

    /// Loads the opaque cursor for one validated provider stream.
    ///
    /// Both an absent row and a present nullable cursor are represented as
    /// `None`; an invalid stored value fails closed as a redacted corruption
    /// error.
    pub fn load_provider_cursor(&self, stream_id: impl AsRef<str>) -> StoreResult<Option<String>> {
        let stream_id =
            validate_provider_stream_identifier(stream_id.as_ref().to_owned(), "stream_id")?;
        let cursor = self
            .connection
            .query_row(
                "SELECT cursor FROM provider_cursors WHERE provider = ?1",
                params![&stream_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(provider_cursor_sqlite_error)?;
        match cursor {
            None | Some(None) => Ok(None),
            Some(Some(cursor)) => validate_provider_cursor_identifier(cursor, "cursor")
                .map(Some)
                .map_err(|_| stored_record_invalid("provider cursor")),
        }
    }

    /// Atomically compares and advances one opaque provider cursor.
    ///
    /// Validation completes before the immediate transaction begins. The
    /// transaction fences concurrent store handles, while equal retries avoid
    /// touching the row timestamp and stale callers receive a redacted
    /// conflict without mutation.
    pub fn advance_provider_cursor(
        &self,
        stream_id: &str,
        expected: Option<&str>,
        next: &str,
    ) -> StoreResult<()> {
        let stream_id = validate_provider_stream_identifier(stream_id.to_owned(), "stream_id")?;
        let expected = expected
            .map(|value| validate_provider_cursor_identifier(value.to_owned(), "expected"))
            .transpose()?;
        let next = validate_provider_cursor_identifier(next.to_owned(), "next")?;

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
                .map_err(provider_cursor_sqlite_error)?;
        let current = load_provider_cursor_row(&transaction, &stream_id)?;
        let current = match current {
            None => None,
            Some(None) => Some(None),
            Some(Some(cursor)) => Some(Some(
                validate_provider_cursor_identifier(cursor, "cursor")
                    .map_err(|_| stored_record_invalid("provider cursor"))?,
            )),
        };

        match current {
            None => {
                if expected.is_some() {
                    return Err(StoreError::CursorConflict {
                        resource: "provider cursor",
                    });
                }
                let inserted = transaction
                    .execute(
                        "INSERT INTO provider_cursors(provider, cursor) VALUES (?1, ?2)
                         ON CONFLICT(provider) DO NOTHING",
                        params![&stream_id, &next],
                    )
                    .map_err(provider_cursor_sqlite_error)?;
                if inserted != 1 {
                    return Err(StoreError::CursorConflict {
                        resource: "provider cursor",
                    });
                }
            }
            Some(current) => {
                if expected.as_deref() != current.as_deref() {
                    return Err(StoreError::CursorConflict {
                        resource: "provider cursor",
                    });
                }
                if current.as_deref() == Some(next.as_str()) {
                    transaction.commit().map_err(provider_cursor_sqlite_error)?;
                    return Ok(());
                }

                let updated = match current {
                    Some(current) => transaction
                        .execute(
                            "UPDATE provider_cursors
                             SET cursor = ?1, updated_at = CURRENT_TIMESTAMP
                             WHERE provider = ?2 AND cursor = ?3",
                            params![&next, &stream_id, &current],
                        )
                        .map_err(provider_cursor_sqlite_error)?,
                    None => transaction
                        .execute(
                            "UPDATE provider_cursors
                             SET cursor = ?1, updated_at = CURRENT_TIMESTAMP
                             WHERE provider = ?2 AND cursor IS NULL",
                            params![&next, &stream_id],
                        )
                        .map_err(provider_cursor_sqlite_error)?,
                };
                if updated != 1 {
                    return Err(StoreError::CursorConflict {
                        resource: "provider cursor",
                    });
                }
            }
        }
        transaction.commit().map_err(provider_cursor_sqlite_error)?;
        Ok(())
    }

    /// Appends one immutable, details-free audit event.
    ///
    /// An identical retry returns the original row, including its database ID
    /// and timestamps. Reusing the idempotency key for different content
    /// returns a redacted conflict and leaves the original row untouched.
    pub fn append_audit_event(
        &self,
        idempotency_key: impl AsRef<str>,
        event_type: AuditEventType,
        entity_type: AuditEntityType,
        entity_id: impl AsRef<str>,
        occurred_at: OffsetDateTime,
    ) -> StoreResult<StoredAuditEvent> {
        let idempotency_key = validate_audit_idempotency_key(idempotency_key.as_ref().to_owned())?;
        let entity_id = validate_audit_entity_id(entity_id.as_ref().to_owned())?;
        let (occurred_at, occurred_at_text) = format_audit_timestamp(occurred_at, "occurred_at")?;

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let inserted = transaction.execute(
            "INSERT INTO audit_events (
                 idempotency_key, event_type, entity_type, entity_id, details, occurred_at
             ) VALUES (?1, ?2, ?3, ?4, NULL, ?5)
             ON CONFLICT(idempotency_key) DO NOTHING",
            params![
                &idempotency_key,
                event_type.as_str(),
                entity_type.as_str(),
                &entity_id,
                occurred_at_text,
            ],
        )?;
        let row = transaction
            .query_row(
                AUDIT_EVENT_SELECT_BY_IDEMPOTENCY,
                params![&idempotency_key],
                read_audit_event_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("audit event"))?
            .ok_or_else(|| stored_record_invalid("audit event"))?;
        let stored = decode_audit_event_row(row)?;

        if inserted == 1 {
            transaction.commit()?;
            return Ok(stored);
        }
        if stored.event_type == event_type
            && stored.entity_type == entity_type
            && stored.entity_id == entity_id
            && stored.occurred_at == occurred_at
        {
            transaction.commit()?;
            return Ok(stored);
        }
        Err(StoreError::Conflict {
            resource: "audit event",
        })
    }

    /// Loads one audit event by its immutable idempotency key.
    pub fn load_audit_event_by_idempotency_key(
        &self,
        idempotency_key: impl AsRef<str>,
    ) -> StoreResult<StoredAuditEvent> {
        let idempotency_key = validate_audit_idempotency_key(idempotency_key.as_ref().to_owned())?;
        let row = self
            .connection
            .query_row(
                AUDIT_EVENT_SELECT_BY_IDEMPOTENCY,
                params![idempotency_key],
                read_audit_event_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("audit event"))?
            .ok_or(StoreError::NotFound {
                resource: "audit event",
            })?;
        decode_audit_event_row(row)
    }

    /// Returns audit events in database-ID order after an optional cursor.
    pub fn list_audit_events(
        &self,
        after_id: Option<i64>,
        limit: usize,
    ) -> StoreResult<Vec<StoredAuditEvent>> {
        if after_id.is_some_and(|id| id <= 0) {
            return Err(StoreError::InvalidInput { field: "cursor" });
        }
        if limit == 0 || limit > MAX_AUDIT_LIST_LIMIT {
            return Err(StoreError::InvalidInput { field: "limit" });
        }
        let limit =
            i64::try_from(limit).map_err(|_| StoreError::InvalidInput { field: "limit" })?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, idempotency_key, event_type, entity_type, entity_id,
                        details, occurred_at, created_at
                 FROM audit_events
                 WHERE (?1 IS NULL OR id > ?1)
                 ORDER BY id ASC
                 LIMIT ?2",
            )
            .map_err(|_| stored_record_invalid("audit event"))?;
        let rows = statement
            .query_map(params![after_id, limit], read_audit_event_row)
            .map_err(|_| stored_record_invalid("audit event"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| stored_record_invalid("audit event"))?;
        rows.into_iter().map(decode_audit_event_row).collect()
    }

    /// Records one immutable provider message summary.
    ///
    /// The initial triage state is derived from the provider. An exact retry
    /// returns the original row; reusing any immutable identity with changed
    /// content returns a generic conflict without changing the row.
    /// A raw body or transcript is not accepted by this API:
    ///
    /// ```compile_fail
    /// use agent_voice::pa::{MessageProvider, MessageSummary, PaStore};
    /// use time::OffsetDateTime;
    ///
    /// let store: PaStore = todo!();
    /// let summary = MessageSummary::new("safe summary").unwrap();
    /// let received_at = OffsetDateTime::now_utc();
    /// let _ = store.record_message(
    ///     "key", "source", MessageProvider::Voice, "provider-id", summary,
    ///     None, None, "raw transcript", received_at,
    /// );
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn record_message<S>(
        &self,
        idempotency_key: impl AsRef<str>,
        source_id: impl AsRef<str>,
        provider: MessageProvider,
        provider_message_id: impl AsRef<str>,
        summary: S,
        subject: Option<String>,
        sender: Option<String>,
        received_at: OffsetDateTime,
    ) -> StoreResult<StoredMessage>
    where
        S: Into<MessageSummary>,
    {
        let idempotency_key =
            validate_message_idempotency_key(idempotency_key.as_ref().to_owned())?;
        let source_id = validate_message_source_id(source_id.as_ref().to_owned())?;
        let provider_message_id =
            validate_provider_message_id(provider_message_id.as_ref().to_owned())?;
        let summary = summary.into();
        let summary = MessageSummary::new(summary.as_str().to_owned())?;
        let subject = validate_message_subject(subject)?;
        let sender = validate_message_sender(sender)?;
        let (received_at, received_at_text) = format_message_timestamp(received_at)?;
        let triage_state = initial_message_state(provider);

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let inserted = transaction.execute(
            "INSERT INTO messages (
                 idempotency_key, source_id, provider, provider_message_id, summary,
                 subject, sender, received_at, triage_state
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT DO NOTHING",
            params![
                &idempotency_key,
                &source_id,
                provider.as_str(),
                &provider_message_id,
                summary.as_str(),
                &subject,
                &sender,
                received_at_text,
                triage_state.as_str(),
            ],
        )?;

        if inserted == 1 {
            let id = transaction.last_insert_rowid();
            let row = transaction
                .query_row(MESSAGE_SELECT, params![id], read_message_row)
                .map_err(|_| stored_record_invalid("message"))?;
            let stored = decode_message_row(row)?;
            transaction.commit()?;
            return Ok(stored);
        }

        let candidates = query_message_rows_by_identity(
            &transaction,
            &idempotency_key,
            &source_id,
            provider,
            &provider_message_id,
        )?;
        let mut exact_retry = None;
        for candidate in candidates {
            let stored = decode_message_row(candidate)?;
            if stored.idempotency_key == idempotency_key
                && stored.source_id == source_id
                && stored.provider == provider
                && stored.provider_message_id == provider_message_id
                && stored.summary == summary
                && stored.subject == subject
                && stored.sender == sender
                && stored.received_at == received_at
            {
                if exact_retry.is_some() {
                    return Err(stored_record_invalid("message"));
                }
                exact_retry = Some(stored);
            } else {
                return Err(StoreError::Conflict {
                    resource: "message",
                });
            }
        }
        let stored = exact_retry.ok_or_else(|| stored_record_invalid("message"))?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Loads one message by SQLite database ID.
    pub fn load_message_by_id(&self, id: i64) -> StoreResult<StoredMessage> {
        let row = self
            .connection
            .query_row(MESSAGE_SELECT, params![id], read_message_row)
            .optional()
            .map_err(|_| stored_record_invalid("message"))?
            .ok_or(StoreError::NotFound {
                resource: "message",
            })?;
        decode_message_row(row)
    }

    /// Loads one message by immutable idempotency key.
    pub fn load_message_by_idempotency_key(
        &self,
        idempotency_key: impl AsRef<str>,
    ) -> StoreResult<StoredMessage> {
        let idempotency_key =
            validate_message_idempotency_key(idempotency_key.as_ref().to_owned())?;
        let row = self
            .connection
            .query_row(
                MESSAGE_SELECT_BY_IDEMPOTENCY,
                params![idempotency_key],
                read_message_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("message"))?
            .ok_or(StoreError::NotFound {
                resource: "message",
            })?;
        decode_message_row(row)
    }

    /// Loads one message by immutable source identity.
    pub fn load_message_by_source_id(
        &self,
        source_id: impl AsRef<str>,
    ) -> StoreResult<StoredMessage> {
        let source_id = validate_message_source_id(source_id.as_ref().to_owned())?;
        let row = self
            .connection
            .query_row(
                MESSAGE_SELECT_BY_SOURCE,
                params![source_id],
                read_message_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("message"))?
            .ok_or(StoreError::NotFound {
                resource: "message",
            })?;
        decode_message_row(row)
    }

    /// Loads one message by provider and provider message identity.
    pub fn load_message_by_provider_message_id(
        &self,
        provider: MessageProvider,
        provider_message_id: impl AsRef<str>,
    ) -> StoreResult<StoredMessage> {
        let provider_message_id =
            validate_provider_message_id(provider_message_id.as_ref().to_owned())?;
        let row = self
            .connection
            .query_row(
                MESSAGE_SELECT_BY_PROVIDER_MESSAGE,
                params![provider.as_str(), provider_message_id],
                read_message_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("message"))?
            .ok_or(StoreError::NotFound {
                resource: "message",
            })?;
        decode_message_row(row)
    }

    /// Alias for [`Self::load_message_by_provider_message_id`].
    pub fn load_message_by_provider_and_message_id(
        &self,
        provider: MessageProvider,
        provider_message_id: impl AsRef<str>,
    ) -> StoreResult<StoredMessage> {
        self.load_message_by_provider_message_id(provider, provider_message_id)
    }

    /// Records one immutable task extracted from an actionable email.
    ///
    /// The referenced message is checked inside the same immediate
    /// transaction as the insert. An exact retry returns the original row;
    /// reusing either immutable identity for changed content returns a
    /// redacted conflict. The API accepts only a validated [`TaskTitle`], so
    /// raw email bodies, transcripts, and prompt text cannot be persisted.
    #[allow(clippy::too_many_arguments)]
    pub fn record_task<T>(
        &self,
        idempotency_key: impl AsRef<str>,
        source_id: impl AsRef<str>,
        message_id: i64,
        title: T,
        kind: TaskKind,
        duration_minutes: Option<u32>,
        due_at: Option<OffsetDateTime>,
    ) -> StoreResult<StoredTask>
    where
        T: Into<TaskTitle>,
    {
        let idempotency_key = validate_task_idempotency_key(idempotency_key.as_ref().to_owned())?;
        let source_id = validate_task_source_id(source_id.as_ref().to_owned())?;
        if message_id <= 0 {
            return Err(StoreError::InvalidInput {
                field: "message_id",
            });
        }
        let title: TaskTitle = title.into();
        let title = TaskTitle::new(title.as_str().to_owned())?;
        let duration_minutes = duration_minutes.unwrap_or(kind.duration_minutes() as u32);
        let duration = validate_task_duration(duration_minutes)?;
        let (due_at, due_at_text) = due_at
            .map(|value| format_task_timestamp(value, "due_at"))
            .transpose()?
            .map(|(value, text)| (Some(value), Some(text)))
            .unwrap_or((None, None));

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let message_row = transaction
            .query_row(MESSAGE_SELECT, params![message_id], read_message_row)
            .optional()
            .map_err(|_| stored_record_invalid("message"))?
            .ok_or_else(|| stored_record_invalid("message"))?;
        let message =
            decode_message_row(message_row).map_err(|_| stored_record_invalid("message"))?;
        validate_actionable_message(&message, "message")?;

        let inserted = transaction.execute(
            "INSERT INTO tasks (
                 idempotency_key, source_id, message_id, title, kind,
                 duration_minutes, due_at, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending')
             ON CONFLICT DO NOTHING",
            params![
                &idempotency_key,
                &source_id,
                message_id,
                title.as_str(),
                task_kind_storage_name(kind),
                i64::from(duration.minutes()),
                due_at_text,
            ],
        )?;

        if inserted == 1 {
            let id = transaction.last_insert_rowid();
            let row = transaction
                .query_row(TASK_SELECT, params![id], read_task_row)
                .map_err(|_| stored_record_invalid("task"))?;
            let stored = decode_task_row(row)?;
            validate_task_message_in_transaction(&transaction, stored.message_id)?;
            transaction.commit()?;
            return Ok(stored);
        }

        let candidates = query_task_rows_by_identity(&transaction, &idempotency_key, &source_id)?;
        let mut exact_retry = None;
        for candidate in candidates {
            let stored = decode_task_row(candidate)?;
            validate_task_message_in_transaction(&transaction, stored.message_id)?;
            if stored.idempotency_key == idempotency_key
                && stored.source_id == source_id
                && stored.message_id == message_id
                && stored.title == title
                && stored.kind == kind
                && stored.duration == duration
                && stored.due_at == due_at
            {
                if exact_retry.is_some() {
                    return Err(stored_record_invalid("task"));
                }
                exact_retry = Some(stored);
            } else {
                return Err(StoreError::Conflict { resource: "task" });
            }
        }
        let stored = exact_retry.ok_or_else(|| stored_record_invalid("task"))?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Loads one task by SQLite database ID.
    pub fn load_task_by_id(&self, id: i64) -> StoreResult<StoredTask> {
        if id <= 0 {
            return Err(StoreError::InvalidInput { field: "id" });
        }
        let row = self
            .connection
            .query_row(TASK_SELECT, params![id], read_task_row)
            .optional()
            .map_err(|_| stored_record_invalid("task"))?
            .ok_or(StoreError::NotFound { resource: "task" })?;
        let task = decode_task_row(row)?;
        validate_task_message_on_connection(&self.connection, task.message_id)?;
        Ok(task)
    }

    /// Loads one task by its immutable idempotency key.
    pub fn load_task_by_idempotency_key(
        &self,
        idempotency_key: impl AsRef<str>,
    ) -> StoreResult<StoredTask> {
        let idempotency_key = validate_task_idempotency_key(idempotency_key.as_ref().to_owned())?;
        let row = self
            .connection
            .query_row(
                TASK_SELECT_BY_IDEMPOTENCY,
                params![idempotency_key],
                read_task_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("task"))?
            .ok_or(StoreError::NotFound { resource: "task" })?;
        let task = decode_task_row(row)?;
        validate_task_message_on_connection(&self.connection, task.message_id)?;
        Ok(task)
    }

    /// Loads one task by its immutable extraction source identity.
    pub fn load_task_by_source_id(&self, source_id: impl AsRef<str>) -> StoreResult<StoredTask> {
        let source_id = validate_task_source_id(source_id.as_ref().to_owned())?;
        let row = self
            .connection
            .query_row(TASK_SELECT_BY_SOURCE, params![source_id], read_task_row)
            .optional()
            .map_err(|_| stored_record_invalid("task"))?
            .ok_or(StoreError::NotFound { resource: "task" })?;
        let task = decode_task_row(row)?;
        validate_task_message_on_connection(&self.connection, task.message_id)?;
        Ok(task)
    }

    /// Compare-and-set one actionable email task through its closed lifecycle.
    ///
    /// Tasks may move from `pending` to `proposed` or `no_slot`, and from
    /// `proposed` to `scheduled` or `no_slot`. Terminal states cannot move.
    /// The referenced message is reconstructed and revalidated while the
    /// immediate transaction is held, so a retry cannot bypass corruption or
    /// a concurrent state change.
    pub fn transition_task(
        &self,
        source_id: impl AsRef<str>,
        expected_state: StoredTaskState,
        next_state: StoredTaskState,
        updated_at: OffsetDateTime,
    ) -> StoreResult<StoredTask> {
        let source_id = validate_task_source_id(source_id.as_ref().to_owned())?;
        let (_, updated_at_text) = format_task_updated_timestamp(updated_at)?;
        if !valid_task_transition(expected_state, next_state) {
            return Err(StoreError::Conflict {
                resource: "task transition",
            });
        }

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let current_row = transaction
            .query_row(TASK_SELECT_BY_SOURCE, params![&source_id], read_task_row)
            .optional()
            .map_err(|_| stored_record_invalid("task"))?
            .ok_or(StoreError::NotFound { resource: "task" })?;
        let current = decode_task_row(current_row)?;
        validate_task_message_in_lifecycle_transaction(&transaction, current.message_id)?;

        if current.state == next_state && current.updated_at == updated_at_text {
            transaction.commit()?;
            return Ok(current);
        }
        if current.state != expected_state {
            return Err(StoreError::Conflict {
                resource: "task transition",
            });
        }

        let updated = transaction.execute(
            "UPDATE tasks
             SET status = ?1, updated_at = ?2
             WHERE source_id = ?3 AND status = ?4",
            params![
                next_state.as_str(),
                &updated_at_text,
                &source_id,
                expected_state.as_str()
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::Conflict {
                resource: "task transition",
            });
        }
        let row = transaction
            .query_row(TASK_SELECT_BY_SOURCE, params![&source_id], read_task_row)
            .map_err(|_| stored_record_invalid("task"))?;
        let stored = decode_task_row(row)?;
        validate_task_message_in_lifecycle_transaction(&transaction, stored.message_id)?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Lists tasks in ascending database-ID order for one lifecycle state.
    ///
    /// Every task and referenced message is reconstructed before applying the
    /// state filter and page limit. This makes corruption fail closed even if
    /// the bad row would otherwise fall outside the returned page.
    pub fn list_tasks_by_state(
        &self,
        state: StoredTaskState,
        after_id: Option<i64>,
        limit: usize,
    ) -> StoreResult<Vec<StoredTask>> {
        if after_id.is_some_and(|id| id <= 0) {
            return Err(StoreError::InvalidInput { field: "cursor" });
        }
        if limit == 0 || limit > MAX_TASK_LIST_LIMIT {
            return Err(StoreError::InvalidInput { field: "limit" });
        }

        let mut statement = self
            .connection
            .prepare(TASK_SELECT_ALL_ORDERED)
            .map_err(|_| stored_record_invalid("task"))?;
        let rows = statement
            .query_map([], read_task_row)
            .map_err(|_| stored_record_invalid("task"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| stored_record_invalid("task"))?;
        let tasks = rows
            .into_iter()
            .map(|row| {
                let task = decode_task_row(row)?;
                validate_task_message_on_connection(&self.connection, task.message_id)
                    .map_err(|_| stored_record_invalid("task"))?;
                Ok(task)
            })
            .collect::<StoreResult<Vec<_>>>()?;
        Ok(tasks
            .into_iter()
            .filter(|task| {
                task.state() == state && after_id.is_none_or(|cursor| task.id() > cursor)
            })
            .take(limit)
            .collect())
    }

    /// Returns messages in ascending database-ID order after an optional
    /// cursor.
    pub fn list_messages(
        &self,
        after_id: Option<i64>,
        limit: usize,
    ) -> StoreResult<Vec<StoredMessage>> {
        if after_id.is_some_and(|id| id <= 0) {
            return Err(StoreError::InvalidInput { field: "cursor" });
        }
        if limit == 0 || limit > MAX_MESSAGE_LIST_LIMIT {
            return Err(StoreError::InvalidInput { field: "limit" });
        }
        let limit =
            i64::try_from(limit).map_err(|_| StoreError::InvalidInput { field: "limit" })?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, idempotency_key, source_id, provider, provider_message_id,
                        summary, subject, sender, received_at, triage_state, created_at,
                        updated_at
                 FROM messages
                 WHERE (?1 IS NULL OR id > ?1)
                 ORDER BY id ASC
                 LIMIT ?2",
            )
            .map_err(|_| stored_record_invalid("message"))?;
        let rows = statement
            .query_map(params![after_id, limit], read_message_row)
            .map_err(|_| stored_record_invalid("message"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| stored_record_invalid("message"))?;
        rows.into_iter().map(decode_message_row).collect()
    }

    /// Compare-and-set the triage state of one Outlook or Gmail message.
    ///
    /// The transition graph is deliberately small: an unprocessed email may
    /// become actionable, ambiguous, or ignored, and an actionable email may
    /// become scheduled. Voice summaries remain recorded and cannot be
    /// changed through this API. Retrying an already-applied transition with
    /// the same timestamp returns the unchanged row.
    pub fn transition_message(
        &self,
        source_id: impl AsRef<str>,
        expected_state: MessageTriageState,
        next_state: MessageTriageState,
        updated_at: OffsetDateTime,
    ) -> StoreResult<StoredMessage> {
        let source_id = validate_message_source_id(source_id.as_ref().to_owned())?;
        let (_, updated_at_text) = format_message_updated_timestamp(updated_at)?;
        if !valid_message_transition(expected_state, next_state) {
            return Err(StoreError::Conflict {
                resource: "message transition",
            });
        }

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let current_row = transaction
            .query_row(
                MESSAGE_SELECT_BY_SOURCE,
                params![&source_id],
                read_message_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("message"))?
            .ok_or(StoreError::NotFound {
                resource: "message",
            })?;
        let current = decode_message_row(current_row)?;

        if current.provider == MessageProvider::Voice {
            return Err(StoreError::Conflict {
                resource: "message transition",
            });
        }
        if current.triage_state == next_state && current.updated_at == updated_at_text {
            transaction.commit()?;
            return Ok(current);
        }
        if current.triage_state != expected_state {
            return Err(StoreError::Conflict {
                resource: "message transition",
            });
        }

        let updated = transaction.execute(
            "UPDATE messages
             SET triage_state = ?1, updated_at = ?2
             WHERE source_id = ?3 AND provider IN ('outlook', 'gmail')
               AND triage_state = ?4",
            params![
                next_state.as_str(),
                updated_at_text,
                &source_id,
                expected_state.as_str()
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::Conflict {
                resource: "message transition",
            });
        }
        let row = transaction
            .query_row(
                MESSAGE_SELECT_BY_SOURCE,
                params![&source_id],
                read_message_row,
            )
            .map_err(|_| stored_record_invalid("message"))?;
        let stored = decode_message_row(row)?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Alias for [`Self::transition_message`].
    pub fn transition_message_state(
        &self,
        source_id: impl AsRef<str>,
        expected_state: MessageTriageState,
        next_state: MessageTriageState,
        updated_at: OffsetDateTime,
    ) -> StoreResult<StoredMessage> {
        self.transition_message(source_id, expected_state, next_state, updated_at)
    }

    /// Alias for [`Self::transition_message`], naming the source lookup.
    pub fn transition_message_by_source_id(
        &self,
        source_id: impl AsRef<str>,
        expected_state: MessageTriageState,
        next_state: MessageTriageState,
        updated_at: OffsetDateTime,
    ) -> StoreResult<StoredMessage> {
        self.transition_message(source_id, expected_state, next_state, updated_at)
    }

    /// Returns messages in ascending database-ID order for one triage state.
    pub fn list_messages_by_triage_state(
        &self,
        triage_state: MessageTriageState,
        after_id: Option<i64>,
        limit: usize,
    ) -> StoreResult<Vec<StoredMessage>> {
        if after_id.is_some_and(|id| id <= 0) {
            return Err(StoreError::InvalidInput { field: "cursor" });
        }
        if limit == 0 || limit > MAX_MESSAGE_LIST_LIMIT {
            return Err(StoreError::InvalidInput { field: "limit" });
        }
        let limit =
            i64::try_from(limit).map_err(|_| StoreError::InvalidInput { field: "limit" })?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, idempotency_key, source_id, provider, provider_message_id,
                        summary, subject, sender, received_at, triage_state, created_at,
                        updated_at
                 FROM messages
                 WHERE (
                           triage_state = ?1
                           OR triage_state NOT IN (
                               'recorded', 'unprocessed', 'actionable',
                               'ambiguous', 'ignored', 'scheduled'
                           )
                       )
                   AND (?2 IS NULL OR id > ?2)
                 ORDER BY id ASC
                 LIMIT ?3",
            )
            .map_err(|_| stored_record_invalid("message"))?;
        let rows = statement
            .query_map(
                params![triage_state.as_str(), after_id, limit],
                read_message_row,
            )
            .map_err(|_| stored_record_invalid("message"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| stored_record_invalid("message"))?;
        rows.into_iter().map(decode_message_row).collect()
    }

    /// Saves one immutable appointment draft under a structured source ID.
    ///
    /// An exact retry returns the existing stored record. Reusing an
    /// idempotency, source, or quote identity for different immutable content
    /// returns [`StoreError::Conflict`] and leaves the database unchanged.
    pub fn save_appointment_draft(
        &self,
        source_id: impl AsRef<str>,
        draft: &AppointmentDraft,
    ) -> StoreResult<StoredAppointmentDraft> {
        let source_id = validate_source_id(source_id.as_ref().to_owned())?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let stored = save_appointment_draft_in_transaction(&transaction, &source_id, draft)?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Selects a frozen quote slot and atomically stores its appointment draft.
    ///
    /// The quote can only be prepared during its half-open validity interval.
    /// Exact retries of an already prepared or consumed quote return its
    /// current aggregate unchanged, including after expiry.
    pub fn prepare_appointment_draft_from_quote(
        &self,
        quote_id: QuoteId,
        slot_index: u32,
        source_id: impl AsRef<str>,
        draft: &AppointmentDraft,
        now: OffsetDateTime,
    ) -> StoreResult<StoredAppointmentQuote> {
        let source_id = validate_source_id(source_id.as_ref().to_owned())?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let current = decode_appointment_quote_by_id(&transaction, quote_id)?;

        match current.state() {
            StoredAppointmentQuoteState::Prepared | StoredAppointmentQuoteState::Consumed => {
                if current.selected_slot_index() == Some(slot_index)
                    && current.appointment_draft().is_some_and(|stored| {
                        stored.source_id() == source_id && stored.draft() == draft
                    })
                {
                    transaction.commit()?;
                    return Ok(current);
                }
                return Err(StoreError::Conflict {
                    resource: "appointment quote",
                });
            }
            StoredAppointmentQuoteState::Issued => {}
        }

        if draft.quote_id() != quote_id || draft.kind() != current.appointment_kind() {
            return Err(StoreError::Conflict {
                resource: "appointment quote",
            });
        }
        if now < current.quote().issued_at() {
            return Err(StoreError::AppointmentQuoteNotYetValid);
        }
        if now >= current.quote().expires_at() {
            return Err(StoreError::AppointmentQuoteExpired);
        }
        let selected_slot_index =
            usize::try_from(slot_index).map_err(|_| StoreError::InvalidInput {
                field: "slot_index",
            })?;
        let selected_slot =
            current
                .offered_slots()
                .get(selected_slot_index)
                .ok_or(StoreError::InvalidInput {
                    field: "slot_index",
                })?;
        if draft.starts_at() != selected_slot.starts_at()
            || draft.ends_at() != selected_slot.ends_at()
        {
            return Err(StoreError::Conflict {
                resource: "appointment quote",
            });
        }

        let stored_draft = save_appointment_draft_in_transaction(&transaction, &source_id, draft)?;
        let updated = transaction.execute(
            "UPDATE appointment_quotes
             SET state = 'prepared', appointment_draft_id = ?1, selected_slot_index = ?2
             WHERE quote_id = ?3 AND state = 'issued'",
            params![
                stored_draft.id(),
                i64::from(slot_index),
                quote_id.to_string(),
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::Conflict {
                resource: "appointment quote",
            });
        }
        let prepared = decode_appointment_quote_by_id(&transaction, quote_id)?;
        transaction.commit()?;
        Ok(prepared)
    }

    /// Saves one immutable appointment quote and its ordered frozen slots.
    pub fn save_appointment_quote(
        &self,
        quote: &Quote,
        appointment_kind: AppointmentKind,
        timezone: impl AsRef<str>,
        offered_slots: &[AppointmentSlot],
    ) -> StoreResult<StoredAppointmentQuote> {
        let timezone = validate_appointment_quote_timezone(timezone.as_ref())?;
        validate_appointment_quote_slots(appointment_kind, offered_slots)?;
        let quote_id = quote.id().to_string();
        let issued_at = format_appointment_quote_datetime(quote.issued_at())?;
        let expires_at = format_appointment_quote_datetime(quote.expires_at())?;
        let transaction = self.connection.unchecked_transaction()?;
        let inserted = transaction.execute(
            "INSERT INTO appointment_quotes (
                 quote_id, appointment_kind, timezone, issued_at, expires_at, slot_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(quote_id) DO NOTHING",
            params![
                &quote_id,
                appointment_kind_storage_name(appointment_kind),
                &timezone,
                &issued_at,
                &expires_at,
                i64::try_from(offered_slots.len()).map_err(|_| StoreError::InvalidInput {
                    field: "offered_slots"
                })?,
            ],
        )?;
        if inserted == 1 {
            for (slot_index, slot) in offered_slots.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO appointment_quote_slots (
                         quote_id, slot_index, starts_at, ends_at
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        &quote_id,
                        i64::try_from(slot_index).map_err(|_| StoreError::InvalidInput {
                            field: "offered_slots"
                        })?,
                        format_appointment_quote_datetime(slot.starts_at())?,
                        format_appointment_quote_datetime(slot.ends_at())?,
                    ],
                )?;
            }
        }

        let stored = decode_appointment_quote_by_id(&transaction, quote.id())?;
        if stored.quote() != quote
            || stored.appointment_kind() != appointment_kind
            || stored.timezone() != timezone
            || stored.offered_slots() != offered_slots
        {
            return Err(StoreError::Conflict {
                resource: "appointment quote",
            });
        }
        transaction.commit()?;
        Ok(stored)
    }

    /// Loads one appointment quote by its opaque identity.
    pub fn load_appointment_quote_by_id(
        &self,
        quote_id: QuoteId,
    ) -> StoreResult<StoredAppointmentQuote> {
        decode_appointment_quote_by_id(&self.connection, quote_id)
    }

    /// Loads one appointment quote by its linked appointment draft database ID.
    pub fn load_appointment_quote_by_draft_id(
        &self,
        appointment_draft_id: i64,
    ) -> StoreResult<StoredAppointmentQuote> {
        if appointment_draft_id <= 0 {
            return Err(StoreError::InvalidInput {
                field: "appointment_draft_id",
            });
        }
        let quote_id: Option<String> = self
            .connection
            .query_row(
                "SELECT quote_id FROM appointment_quotes WHERE appointment_draft_id = ?1",
                params![appointment_draft_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| stored_record_invalid("appointment quote"))?;
        let quote_id = quote_id.ok_or(StoreError::NotFound {
            resource: "appointment quote",
        })?;
        let quote_id = parse_quote_id(&quote_id)?;
        decode_appointment_quote_by_id(&self.connection, quote_id)
    }

    /// Loads an appointment draft by SQLite database ID.
    pub fn load_appointment_draft_by_id(&self, id: i64) -> StoreResult<StoredAppointmentDraft> {
        let row = self
            .connection
            .query_row(APPOINTMENT_DRAFT_SELECT, params![id], read_appointment_row)
            .optional()
            .map_err(|_| stored_record_invalid("appointment draft"))?
            .ok_or(StoreError::NotFound {
                resource: "appointment draft",
            })?;
        decode_appointment_row(row)
    }

    /// Loads an appointment draft by its immutable idempotency key.
    pub fn load_appointment_draft_by_idempotency_key(
        &self,
        idempotency_key: impl AsRef<str>,
    ) -> StoreResult<StoredAppointmentDraft> {
        let idempotency_key =
            validate_non_empty(idempotency_key.as_ref().to_owned(), "idempotency_key")?;
        let row = self
            .connection
            .query_row(
                APPOINTMENT_DRAFT_SELECT_BY_IDEMPOTENCY,
                params![idempotency_key],
                read_appointment_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("appointment draft"))?
            .ok_or(StoreError::NotFound {
                resource: "appointment draft",
            })?;
        decode_appointment_row(row)
    }

    /// Deletes one appointment draft by SQLite database ID.
    pub fn delete_appointment_draft_by_id(&self, id: i64) -> StoreResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let deleted = transaction.execute(
            "DELETE FROM appointment_drafts
             WHERE id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM proposals WHERE appointment_draft_id = ?1
               )",
            params![id],
        )?;
        if deleted == 0 {
            let exists: Option<i64> = transaction
                .query_row(
                    "SELECT id FROM appointment_drafts WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?;
            return if exists.is_some() {
                Err(StoreError::Conflict {
                    resource: "appointment draft",
                })
            } else {
                Err(StoreError::NotFound {
                    resource: "appointment draft",
                })
            };
        }
        transaction.commit()?;
        Ok(())
    }

    /// Saves one immutable owner task draft with an optional structured source
    /// ID. Exact retries return the existing record; all other identity reuse
    /// returns a redacted [`StoreError::Conflict`].
    pub fn save_owner_task_draft(
        &self,
        source_id: Option<&str>,
        draft: &OwnerTaskDraft,
    ) -> StoreResult<StoredOwnerTaskDraft> {
        let source_id = source_id
            .map(|source_id| validate_source_id(source_id.to_owned()))
            .transpose()?;
        let idempotency_key = draft.idempotency_key().as_str().to_owned();
        let kind = task_kind_storage_name(draft.kind());
        let duration_minutes = i64::from(draft.duration().minutes());
        let due_at = draft.due_at().map(format_offset_datetime).transpose()?;

        let transaction = self.connection.unchecked_transaction()?;
        let inserted = transaction.execute(
            "INSERT INTO owner_task_drafts (
                 idempotency_key, source_id, title, kind, duration_minutes, due_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT DO NOTHING",
            params![
                idempotency_key,
                source_id,
                draft.title(),
                kind,
                duration_minutes,
                due_at,
            ],
        )?;
        if inserted == 1 {
            let id = transaction.last_insert_rowid();
            transaction.commit()?;
            return Ok(StoredOwnerTaskDraft {
                id,
                source_id,
                draft: draft.clone(),
            });
        }

        let candidates = query_owner_task_rows_by_identity(
            &transaction,
            &idempotency_key,
            source_id.as_deref(),
        )?;
        let mut exact_retry = None;
        for candidate in candidates {
            let stored = decode_owner_task_row(candidate)?;
            if stored.source_id.as_deref() == source_id.as_deref() && stored.draft == *draft {
                exact_retry = Some(stored);
            } else {
                return Err(StoreError::Conflict {
                    resource: "owner task draft",
                });
            }
        }
        let stored = exact_retry.ok_or_else(|| stored_record_invalid("owner task draft"))?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Loads an owner task draft by SQLite database ID.
    pub fn load_owner_task_draft_by_id(&self, id: i64) -> StoreResult<StoredOwnerTaskDraft> {
        let row = self
            .connection
            .query_row(OWNER_TASK_DRAFT_SELECT, params![id], read_owner_task_row)
            .optional()
            .map_err(|_| stored_record_invalid("owner task draft"))?
            .ok_or(StoreError::NotFound {
                resource: "owner task draft",
            })?;
        decode_owner_task_row(row)
    }

    /// Loads an owner task draft by its immutable idempotency key.
    pub fn load_owner_task_draft_by_idempotency_key(
        &self,
        idempotency_key: impl AsRef<str>,
    ) -> StoreResult<StoredOwnerTaskDraft> {
        let idempotency_key =
            validate_non_empty(idempotency_key.as_ref().to_owned(), "idempotency_key")?;
        let row = self
            .connection
            .query_row(
                OWNER_TASK_DRAFT_SELECT_BY_IDEMPOTENCY,
                params![idempotency_key],
                read_owner_task_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("owner task draft"))?
            .ok_or(StoreError::NotFound {
                resource: "owner task draft",
            })?;
        decode_owner_task_row(row)
    }

    /// Deletes one owner task draft by SQLite database ID.
    pub fn delete_owner_task_draft_by_id(&self, id: i64) -> StoreResult<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let deleted = transaction.execute(
            "DELETE FROM owner_task_drafts
             WHERE id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM proposals WHERE owner_task_draft_id = ?1
               )",
            params![id],
        )?;
        if deleted == 0 {
            let exists: Option<i64> = transaction
                .query_row(
                    "SELECT id FROM owner_task_drafts WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?;
            return if exists.is_some() {
                Err(StoreError::Conflict {
                    resource: "owner task draft",
                })
            } else {
                Err(StoreError::NotFound {
                    resource: "owner task draft",
                })
            };
        }
        transaction.commit()?;
        Ok(())
    }

    /// Binds an immutable owner task to its exact direct-calendar interval.
    /// Exact retries return the existing placement; altered values conflict.
    pub fn save_owner_task_placement(
        &self,
        owner_task_draft_id: i64,
        starts_at: OffsetDateTime,
        ends_at: OffsetDateTime,
        timezone: impl AsRef<str>,
        operation_key: impl AsRef<str>,
        owner_fingerprint: impl AsRef<str>,
    ) -> StoreResult<StoredOwnerTaskPlacement> {
        if owner_task_draft_id <= 0 || starts_at >= ends_at {
            return Err(StoreError::InvalidInput {
                field: "owner_task_placement",
            });
        }
        let timezone = validate_owner_task_timezone(timezone.as_ref())?;
        let operation_key =
            validate_machine_identifier(operation_key.as_ref().to_owned(), "operation_key")?;
        let owner_fingerprint = validate_machine_identifier(
            owner_fingerprint.as_ref().to_owned(),
            "owner_fingerprint",
        )?;
        let starts_at_text = format_owner_task_placement_datetime(starts_at)?;
        let ends_at_text = format_owner_task_placement_datetime(ends_at)?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        ensure_owner_task_draft_exists(&transaction, owner_task_draft_id)?;
        transaction.execute(
            "INSERT INTO owner_task_placements (owner_task_draft_id, starts_at, ends_at, timezone, operation_key, owner_fingerprint, state) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared') ON CONFLICT DO NOTHING",
            params![owner_task_draft_id, starts_at_text, ends_at_text, timezone, operation_key, owner_fingerprint],
        )?;
        let placement = transaction
            .query_row(
                OWNER_TASK_PLACEMENT_SELECT,
                params![owner_task_draft_id],
                read_owner_task_placement_row,
            )
            .optional()?;
        let placement = match placement {
            Some(placement) => placement,
            None => transaction
                .query_row(
                    OWNER_TASK_PLACEMENT_SELECT_BY_OPERATION_KEY,
                    params![operation_key],
                    read_owner_task_placement_row,
                )
                .optional()?
                .ok_or(StoreError::NotFound {
                    resource: "owner task placement",
                })?,
        };
        let placement = decode_owner_task_placement(placement)?;
        if placement.owner_task_draft_id != owner_task_draft_id {
            return Err(StoreError::Conflict {
                resource: "owner task placement",
            });
        }
        if placement.starts_at != starts_at
            || placement.ends_at != ends_at
            || placement.timezone != timezone
            || placement.operation_key != operation_key
            || placement.owner_fingerprint != owner_fingerprint
        {
            return Err(StoreError::Conflict {
                resource: "owner task placement",
            });
        }
        transaction.commit()?;
        Ok(placement)
    }

    /// Atomically creates the owner draft and its caller-bound placement.
    #[allow(clippy::too_many_arguments)]
    pub fn save_prepared_owner_task(
        &self,
        source_id: Option<&str>,
        draft: &OwnerTaskDraft,
        starts_at: OffsetDateTime,
        ends_at: OffsetDateTime,
        timezone: impl AsRef<str>,
        operation_key: impl AsRef<str>,
        owner_fingerprint: impl AsRef<str>,
    ) -> StoreResult<(StoredOwnerTaskDraft, StoredOwnerTaskPlacement)> {
        if starts_at >= ends_at {
            return Err(StoreError::InvalidInput {
                field: "owner_task_placement",
            });
        }
        let source_id = source_id
            .map(|s| validate_source_id(s.to_owned()))
            .transpose()?;
        let timezone = validate_owner_task_timezone(timezone.as_ref())?;
        let operation_key =
            validate_machine_identifier(operation_key.as_ref().to_owned(), "operation_key")?;
        let owner_fingerprint = validate_machine_identifier(
            owner_fingerprint.as_ref().to_owned(),
            "owner_fingerprint",
        )?;
        let starts_at_text = format_owner_task_placement_datetime(starts_at)?;
        let ends_at_text = format_owner_task_placement_datetime(ends_at)?;
        // Acquire the write lock before examining either retry identity.
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let draft_key = draft.idempotency_key().as_str().to_owned();
        transaction.execute(
            "INSERT INTO owner_task_drafts (idempotency_key, source_id, title, kind, duration_minutes, due_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT DO NOTHING",
            params![draft_key, source_id, draft.title(), task_kind_storage_name(draft.kind()), i64::from(draft.duration().minutes()), draft.due_at().map(format_offset_datetime).transpose()?],
        )?;
        let candidate = query_owner_task_rows_by_identity(
            &transaction,
            draft.idempotency_key().as_str(),
            source_id.as_deref(),
        )?
        .into_iter()
        .map(decode_owner_task_row)
        .collect::<StoreResult<Vec<_>>>()?;
        let stored = candidate
            .into_iter()
            .find(|item| item.source_id.as_deref() == source_id.as_deref() && item.draft == *draft)
            .ok_or(StoreError::Conflict {
                resource: "owner task draft",
            })?;
        transaction.execute(
            "INSERT INTO owner_task_placements (owner_task_draft_id, starts_at, ends_at, timezone, operation_key, owner_fingerprint, state) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared') ON CONFLICT DO NOTHING",
            params![stored.id(), starts_at_text, ends_at_text, timezone, operation_key, owner_fingerprint],
        )?;
        let row = transaction
            .query_row(
                OWNER_TASK_PLACEMENT_SELECT,
                params![stored.id()],
                read_owner_task_placement_row,
            )
            .optional()?;
        let row = match row {
            Some(row) => row,
            None => transaction
                .query_row(
                    OWNER_TASK_PLACEMENT_SELECT_BY_OPERATION_KEY,
                    params![operation_key],
                    read_owner_task_placement_row,
                )
                .optional()?
                .ok_or(StoreError::NotFound {
                    resource: "owner task placement",
                })?,
        };
        let placement = decode_owner_task_placement(row)?;
        if placement.owner_task_draft_id != stored.id() {
            return Err(StoreError::Conflict {
                resource: "owner task placement",
            });
        }
        if placement.starts_at != starts_at
            || placement.ends_at != ends_at
            || placement.timezone != timezone
            || placement.operation_key != operation_key
            || placement.owner_fingerprint != owner_fingerprint
        {
            return Err(StoreError::Conflict {
                resource: "owner task placement",
            });
        }
        transaction.commit()?;
        Ok((stored, placement))
    }

    /// Loads a direct-owner placement by its immutable task draft ID.
    pub fn load_owner_task_placement(
        &self,
        owner_task_draft_id: i64,
    ) -> StoreResult<StoredOwnerTaskPlacement> {
        let placement = self
            .connection
            .query_row(
                OWNER_TASK_PLACEMENT_SELECT,
                params![owner_task_draft_id],
                read_owner_task_placement_row,
            )
            .optional()?
            .ok_or(StoreError::NotFound {
                resource: "owner task placement",
            })?;
        let placement = decode_owner_task_placement(placement)?;
        validate_owner_task_placement_reference(&self.connection, &placement)?;
        Ok(placement)
    }

    /// Records the sole Outlook event that completed a direct owner task.
    pub fn submit_owner_task_placement(
        &self,
        owner_task_draft_id: i64,
        provider_event_id: impl AsRef<str>,
    ) -> StoreResult<StoredOwnerTaskPlacement> {
        let provider_event_id = validate_provider_event_id(provider_event_id.as_ref().to_owned())?;
        // Serialize the read-then-write transition so concurrent confirmed
        // calls converge instead of one failing on a deferred lock upgrade.
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let placement = transaction
            .query_row(
                OWNER_TASK_PLACEMENT_SELECT,
                params![owner_task_draft_id],
                read_owner_task_placement_row,
            )
            .optional()?
            .ok_or(StoreError::NotFound {
                resource: "owner task placement",
            })?;
        let placement = decode_owner_task_placement(placement)?;
        validate_owner_task_placement_reference(&transaction, &placement)?;
        if let Some(existing) = placement.provider_event_id() {
            if existing != provider_event_id {
                return Err(StoreError::Conflict {
                    resource: "owner task placement",
                });
            }
            transaction.commit()?;
            return Ok(placement);
        }
        if let Some(existing) = transaction
            .query_row(
                OWNER_TASK_PLACEMENT_SELECT_BY_PROVIDER_EVENT,
                params![provider_event_id],
                read_owner_task_placement_row,
            )
            .optional()?
        {
            let existing = decode_owner_task_placement(existing)?;
            if existing.owner_task_draft_id != owner_task_draft_id {
                return Err(StoreError::Conflict {
                    resource: "owner task placement",
                });
            }
        }
        transaction.execute(
            "UPDATE owner_task_placements SET state = 'submitted', provider_event_id = ?2, updated_at = CURRENT_TIMESTAMP WHERE owner_task_draft_id = ?1",
            params![owner_task_draft_id, provider_event_id],
        )?;
        let submitted = transaction
            .query_row(
                OWNER_TASK_PLACEMENT_SELECT,
                params![owner_task_draft_id],
                read_owner_task_placement_row,
            )
            .optional()?
            .ok_or(StoreError::NotFound {
                resource: "owner task placement",
            })?;
        let submitted = decode_owner_task_placement(submitted)?;
        validate_owner_task_placement_reference(&transaction, &submitted)?;
        transaction.commit()?;
        Ok(submitted)
    }

    /// Creates one immutable proposal from exactly one existing draft.
    ///
    /// An identical retry returns the existing row. Reusing either immutable
    /// identity with a different source returns a redacted conflict and does
    /// not modify the database.
    pub fn create_proposal(
        &self,
        idempotency_key: impl AsRef<str>,
        source_id: impl AsRef<str>,
        source: ProposalSource,
    ) -> StoreResult<StoredProposal> {
        let idempotency_key =
            validate_non_empty(idempotency_key.as_ref().to_owned(), "idempotency_key")?;
        let source_id = validate_source_id(source_id.as_ref().to_owned())?;
        validate_proposal_source(source)?;

        let transaction = self.connection.unchecked_transaction()?;
        let stored =
            create_proposal_in_transaction(&transaction, &idempotency_key, &source_id, source)?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Atomically consumes a prepared appointment quote into its pending
    /// proposal. Exact retries return the linked proposal unchanged.
    pub fn submit_appointment_quote(
        &self,
        quote_id: QuoteId,
        appointment_draft_id: i64,
        proposal_idempotency_key: impl AsRef<str>,
        proposal_source_id: impl AsRef<str>,
        now: OffsetDateTime,
    ) -> StoreResult<StoredProposal> {
        if appointment_draft_id <= 0 {
            return Err(StoreError::InvalidInput {
                field: "appointment_draft_id",
            });
        }
        let proposal_idempotency_key = validate_non_empty(
            proposal_idempotency_key.as_ref().to_owned(),
            "idempotency_key",
        )?;
        let proposal_source_id = validate_source_id(proposal_source_id.as_ref().to_owned())?;
        let source = ProposalSource::appointment_draft(appointment_draft_id);
        let consumed_at = format_appointment_quote_datetime(now)?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let current = decode_appointment_quote_by_id(&transaction, quote_id)?;

        match current.state() {
            StoredAppointmentQuoteState::Consumed => {
                let linked_draft_id = current
                    .appointment_draft_id()
                    .ok_or_else(|| stored_record_invalid("appointment quote"))?;
                let proposal_id = current
                    .proposal_id()
                    .ok_or_else(|| stored_record_invalid("appointment quote"))?;
                let proposal_row = transaction
                    .query_row(PROPOSAL_SELECT, params![proposal_id], read_proposal_row)
                    .map_err(|_| stored_record_invalid("appointment quote"))?;
                let proposal = decode_proposal_row(proposal_row)
                    .map_err(|_| stored_record_invalid("appointment quote"))?;
                if linked_draft_id != appointment_draft_id
                    || proposal.idempotency_key() != proposal_idempotency_key
                    || proposal.source_id() != proposal_source_id
                    || proposal.source() != source
                {
                    return Err(StoreError::Conflict {
                        resource: "appointment quote",
                    });
                }
                transaction.commit()?;
                return Ok(proposal);
            }
            StoredAppointmentQuoteState::Issued => {
                return Err(StoreError::Conflict {
                    resource: "appointment quote",
                });
            }
            StoredAppointmentQuoteState::Prepared => {}
        }

        if current.appointment_draft_id() != Some(appointment_draft_id) {
            return Err(StoreError::Conflict {
                resource: "appointment quote",
            });
        }
        if now < current.quote().issued_at() {
            return Err(StoreError::AppointmentQuoteNotYetValid);
        }
        if now >= current.quote().expires_at() {
            return Err(StoreError::AppointmentQuoteExpired);
        }
        let proposal = create_proposal_in_transaction(
            &transaction,
            &proposal_idempotency_key,
            &proposal_source_id,
            source,
        )?;
        if proposal.state() != ProposalState::Pending {
            return Err(StoreError::Conflict {
                resource: "appointment quote",
            });
        }
        let updated = transaction.execute(
            "UPDATE appointment_quotes
             SET state = 'consumed', consumed_at = ?1, proposal_id = ?2
             WHERE quote_id = ?3 AND state = 'prepared' AND appointment_draft_id = ?4",
            params![
                consumed_at,
                proposal.id(),
                quote_id.to_string(),
                appointment_draft_id
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::Conflict {
                resource: "appointment quote",
            });
        }
        let quote = decode_appointment_quote_by_id(&transaction, quote_id)?;
        if quote.proposal_id() != Some(proposal.id()) || quote.consumed_at() != Some(now) {
            return Err(stored_record_invalid("appointment quote"));
        }
        let stored = transaction
            .query_row(PROPOSAL_SELECT, params![proposal.id()], read_proposal_row)
            .map_err(|_| stored_record_invalid("proposal"))
            .and_then(decode_proposal_row)?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Loads a proposal by SQLite database ID.
    pub fn load_proposal_by_id(&self, id: i64) -> StoreResult<StoredProposal> {
        let row = self
            .connection
            .query_row(PROPOSAL_SELECT, params![id], read_proposal_row)
            .optional()
            .map_err(|_| stored_record_invalid("proposal"))?
            .ok_or(StoreError::NotFound {
                resource: "proposal",
            })?;
        decode_proposal_row(row)
    }

    /// Loads a proposal by immutable idempotency key.
    pub fn load_proposal_by_idempotency_key(
        &self,
        idempotency_key: impl AsRef<str>,
    ) -> StoreResult<StoredProposal> {
        let idempotency_key =
            validate_non_empty(idempotency_key.as_ref().to_owned(), "idempotency_key")?;
        let row = self
            .connection
            .query_row(
                PROPOSAL_SELECT_BY_IDEMPOTENCY,
                params![idempotency_key],
                read_proposal_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("proposal"))?
            .ok_or(StoreError::NotFound {
                resource: "proposal",
            })?;
        decode_proposal_row(row)
    }

    /// Loads a proposal by immutable source identity.
    pub fn load_proposal_by_source_id(
        &self,
        source_id: impl AsRef<str>,
    ) -> StoreResult<StoredProposal> {
        let source_id = validate_source_id(source_id.as_ref().to_owned())?;
        let row = self
            .connection
            .query_row(
                PROPOSAL_SELECT_BY_SOURCE,
                params![source_id],
                read_proposal_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("proposal"))?
            .ok_or(StoreError::NotFound {
                resource: "proposal",
            })?;
        decode_proposal_row(row)
    }

    /// Compare-and-set transitions a pending proposal to one terminal state.
    /// An identical terminal retry returns the unchanged row.
    pub fn transition_proposal(
        &self,
        id: i64,
        next_state: ProposalState,
    ) -> StoreResult<StoredProposal> {
        let transaction = self.connection.unchecked_transaction()?;
        let current_row = transaction
            .query_row(PROPOSAL_SELECT, params![id], read_proposal_row)
            .optional()
            .map_err(|_| stored_record_invalid("proposal"))?
            .ok_or(StoreError::NotFound {
                resource: "proposal",
            })?;
        let current = decode_proposal_row(current_row)?;

        if current.state == next_state && current.state.is_terminal() {
            transaction.commit()?;
            return Ok(current);
        }
        if !current.state.can_transition_to(next_state) {
            return Err(StoreError::Conflict {
                resource: "proposal transition",
            });
        }

        let state = proposal_state_storage_name(next_state);
        let updated = transaction.execute(
            "UPDATE proposals
             SET state = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2 AND state = 'pending'",
            params![state, id],
        )?;
        if updated != 1 {
            return Err(StoreError::Conflict {
                resource: "proposal transition",
            });
        }
        let row = transaction
            .query_row(PROPOSAL_SELECT, params![id], read_proposal_row)
            .map_err(|_| stored_record_invalid("proposal"))?;
        let stored = decode_proposal_row(row)?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Alias for [`Self::transition_proposal`].
    pub fn transition_proposal_state(
        &self,
        id: i64,
        next_state: ProposalState,
    ) -> StoreResult<StoredProposal> {
        self.transition_proposal(id, next_state)
    }

    /// Deletes one proposal and its event mappings by database ID.
    pub fn delete_proposal_by_id(&self, id: i64) -> StoreResult<()> {
        let deleted = self
            .connection
            .execute("DELETE FROM proposals WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StoreError::NotFound {
                resource: "proposal",
            });
        }
        Ok(())
    }

    /// Attaches one immutable provider event mapping to a proposal.
    ///
    /// Exact retries return the existing row. Any reuse of a provider/event,
    /// source, or proposal identity with different content returns a redacted
    /// conflict and leaves the database unchanged.
    pub fn attach_event_mapping(
        &self,
        proposal_id: i64,
        provider: impl AsRef<str>,
        provider_event_id: impl AsRef<str>,
        source_id: impl AsRef<str>,
        starts_at: Option<OffsetDateTime>,
        ends_at: Option<OffsetDateTime>,
    ) -> StoreResult<StoredEventMapping> {
        let provider = validate_non_empty(provider.as_ref().to_owned(), "provider")?;
        let provider_event_id =
            validate_non_empty(provider_event_id.as_ref().to_owned(), "provider_event_id")?;
        let source_id = validate_source_id(source_id.as_ref().to_owned())?;
        validate_event_times(starts_at, ends_at)?;

        let transaction = self.connection.unchecked_transaction()?;
        let proposal_exists: Option<i64> = transaction
            .query_row(
                "SELECT id FROM proposals WHERE id = ?1",
                params![proposal_id],
                |row| row.get(0),
            )
            .optional()?;
        if proposal_exists.is_none() {
            return Err(StoreError::NotFound {
                resource: "proposal",
            });
        }

        let starts_at_text = starts_at.map(format_offset_datetime).transpose()?;
        let ends_at_text = ends_at.map(format_offset_datetime).transpose()?;
        let inserted = transaction.execute(
            "INSERT INTO event_mappings (
                 proposal_id, provider, provider_event_id, source_id,
                 starts_at, ends_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT DO NOTHING",
            params![
                proposal_id,
                provider,
                provider_event_id,
                source_id,
                starts_at_text,
                ends_at_text,
            ],
        )?;
        if inserted == 1 {
            let id = transaction.last_insert_rowid();
            let row = transaction
                .query_row(EVENT_MAPPING_SELECT, params![id], read_event_mapping_row)
                .map_err(|_| stored_record_invalid("event mapping"))?;
            let stored = decode_event_mapping_row(row)?;
            transaction.commit()?;
            return Ok(stored);
        }

        let candidates = query_event_mapping_rows_by_identity(
            &transaction,
            proposal_id,
            &provider,
            &provider_event_id,
            &source_id,
        )?;
        let mut exact_retry = None;
        for candidate in candidates {
            let stored = decode_event_mapping_row(candidate)?;
            if stored.proposal_id == proposal_id
                && stored.provider == provider
                && stored.provider_event_id == provider_event_id
                && stored.source_id == source_id
                && stored.starts_at == starts_at
                && stored.ends_at == ends_at
            {
                exact_retry = Some(stored);
            } else {
                return Err(StoreError::Conflict {
                    resource: "event mapping",
                });
            }
        }
        let stored = exact_retry.ok_or_else(|| stored_record_invalid("event mapping"))?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Loads an event mapping by database ID.
    pub fn load_event_mapping_by_id(&self, id: i64) -> StoreResult<StoredEventMapping> {
        let row = self
            .connection
            .query_row(EVENT_MAPPING_SELECT, params![id], read_event_mapping_row)
            .optional()
            .map_err(|_| stored_record_invalid("event mapping"))?
            .ok_or(StoreError::NotFound {
                resource: "event mapping",
            })?;
        decode_event_mapping_row(row)
    }

    /// Loads an event mapping by its proposal ID.
    pub fn load_event_mapping_by_proposal_id(
        &self,
        proposal_id: i64,
    ) -> StoreResult<StoredEventMapping> {
        let row = self
            .connection
            .query_row(
                EVENT_MAPPING_SELECT_BY_PROPOSAL,
                params![proposal_id],
                read_event_mapping_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("event mapping"))?
            .ok_or(StoreError::NotFound {
                resource: "event mapping",
            })?;
        decode_event_mapping_row(row)
    }

    /// Loads an event mapping by provider and provider event ID.
    pub fn load_event_mapping_by_provider_event(
        &self,
        provider: impl AsRef<str>,
        provider_event_id: impl AsRef<str>,
    ) -> StoreResult<StoredEventMapping> {
        let provider = validate_non_empty(provider.as_ref().to_owned(), "provider")?;
        let provider_event_id =
            validate_non_empty(provider_event_id.as_ref().to_owned(), "provider_event_id")?;
        let row = self
            .connection
            .query_row(
                EVENT_MAPPING_SELECT_BY_PROVIDER_EVENT,
                params![provider, provider_event_id],
                read_event_mapping_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("event mapping"))?
            .ok_or(StoreError::NotFound {
                resource: "event mapping",
            })?;
        decode_event_mapping_row(row)
    }

    /// Loads an event mapping by immutable source identity.
    pub fn load_event_mapping_by_source_id(
        &self,
        source_id: impl AsRef<str>,
    ) -> StoreResult<StoredEventMapping> {
        let source_id = validate_source_id(source_id.as_ref().to_owned())?;
        let row = self
            .connection
            .query_row(
                EVENT_MAPPING_SELECT_BY_SOURCE,
                params![source_id],
                read_event_mapping_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("event mapping"))?
            .ok_or(StoreError::NotFound {
                resource: "event mapping",
            })?;
        decode_event_mapping_row(row)
    }

    /// Deletes one event mapping by database ID.
    pub fn delete_event_mapping_by_id(&self, id: i64) -> StoreResult<()> {
        let deleted = self
            .connection
            .execute("DELETE FROM event_mappings WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StoreError::NotFound {
                resource: "event mapping",
            });
        }
        Ok(())
    }

    /// Enqueues one immutable, structured notification.
    ///
    /// The reference checks and insert happen in one transaction. An exact
    /// retry returns the existing row, while reusing the key for any changed
    /// immutable input returns a redacted conflict without changing it.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_notification<R>(
        &self,
        idempotency_key: impl AsRef<str>,
        proposal_id: Option<i64>,
        event_mapping_id: Option<i64>,
        kind: NotificationKind,
        recipient: R,
        template_data: NotificationTemplateData,
        available_at: OffsetDateTime,
    ) -> StoreResult<StoredNotification>
    where
        R: Into<NotificationRecipient>,
    {
        let idempotency_key =
            IdempotencyKey::new(idempotency_key.as_ref().to_owned()).map_err(|_| {
                StoreError::InvalidInput {
                    field: "idempotency_key",
                }
            })?;
        let idempotency_key = idempotency_key.as_str().to_owned();
        let recipient = recipient.into();
        let available_at = normalize_notification_time(available_at)?;
        let available_at_text = format_offset_datetime(available_at)?;
        let persisted_at = format_offset_datetime(OffsetDateTime::now_utc())?;
        let payload =
            serde_json::to_string(&template_data).map_err(|_| StoreError::StoredValueInvalid)?;

        let transaction = self.connection.unchecked_transaction()?;
        validate_notification_references(&transaction, proposal_id, event_mapping_id, kind)?;

        let inserted = transaction.execute(
            "INSERT INTO notification_outbox (
                 idempotency_key, proposal_id, event_mapping_id, notification_kind,
                 recipient, payload, status, available_at, sent_at, attempts,
                 created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, NULL, 0, ?8, ?8)
             ON CONFLICT DO NOTHING",
            params![
                idempotency_key,
                proposal_id,
                event_mapping_id,
                kind.storage_name(),
                recipient.as_str(),
                payload,
                available_at_text,
                persisted_at,
            ],
        )?;

        if inserted == 1 {
            let id = transaction.last_insert_rowid();
            let row = transaction
                .query_row(NOTIFICATION_SELECT, params![id], read_notification_row)
                .map_err(|_| stored_record_invalid("notification"))?;
            let stored = decode_notification_row(row)?;
            validate_stored_notification_references(&transaction, &stored)?;
            transaction.commit()?;
            return Ok(stored);
        }

        let row = transaction
            .query_row(
                NOTIFICATION_SELECT_BY_IDEMPOTENCY,
                params![idempotency_key],
                read_notification_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("notification"))?
            .ok_or_else(|| stored_record_invalid("notification"))?;
        let stored = decode_notification_row(row)?;
        validate_stored_notification_references(&transaction, &stored)?;
        if stored.proposal_id == proposal_id
            && stored.event_mapping_id == event_mapping_id
            && stored.kind == kind
            && stored.recipient == recipient
            && stored.template_data == template_data
            && stored.available_at == available_at
        {
            transaction.commit()?;
            return Ok(stored);
        }
        Err(StoreError::Conflict {
            resource: "notification",
        })
    }

    /// Loads one notification by SQLite database ID.
    pub fn load_notification_by_id(&self, id: i64) -> StoreResult<StoredNotification> {
        let row = self
            .connection
            .query_row(NOTIFICATION_SELECT, params![id], read_notification_row)
            .optional()
            .map_err(|_| stored_record_invalid("notification"))?
            .ok_or(StoreError::NotFound {
                resource: "notification",
            })?;
        let notification = decode_notification_row(row)?;
        validate_stored_notification_references(&self.connection, &notification)?;
        Ok(notification)
    }

    /// Loads one notification by immutable idempotency key.
    pub fn load_notification_by_idempotency_key(
        &self,
        idempotency_key: impl AsRef<str>,
    ) -> StoreResult<StoredNotification> {
        let idempotency_key =
            IdempotencyKey::new(idempotency_key.as_ref().to_owned()).map_err(|_| {
                StoreError::InvalidInput {
                    field: "idempotency_key",
                }
            })?;
        let row = self
            .connection
            .query_row(
                NOTIFICATION_SELECT_BY_IDEMPOTENCY,
                params![idempotency_key.as_str()],
                read_notification_row,
            )
            .optional()
            .map_err(|_| stored_record_invalid("notification"))?
            .ok_or(StoreError::NotFound {
                resource: "notification",
            })?;
        let notification = decode_notification_row(row)?;
        validate_stored_notification_references(&self.connection, &notification)?;
        Ok(notification)
    }

    /// Returns pending notifications ordered by availability and database ID.
    pub fn list_pending_notifications(&self) -> StoreResult<Vec<StoredNotification>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, idempotency_key, proposal_id, event_mapping_id,
                        notification_kind, recipient, payload, status, available_at,
                        lease_until, sent_at, last_error_code, attempts, created_at,
                        updated_at
                 FROM notification_outbox
                 WHERE status = 'pending'
                 ORDER BY available_at ASC, id ASC",
            )
            .map_err(|_| stored_record_invalid("notification"))?;
        let rows = statement
            .query_map([], read_notification_row)
            .map_err(|_| stored_record_invalid("notification"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| stored_record_invalid("notification"))?;
        rows.into_iter()
            .map(|row| {
                let notification = decode_notification_row(row)?;
                validate_stored_notification_references(&self.connection, &notification)?;
                Ok(notification)
            })
            .collect()
    }

    /// Alias for [`Self::list_pending_notifications`].
    pub fn list_pending_notification_outbox(&self) -> StoreResult<Vec<StoredNotification>> {
        self.list_pending_notifications()
    }

    /// Atomically claims eligible notification attempts for a bounded lease.
    ///
    /// Pending records are eligible once available; abandoned delivering
    /// records become eligible again after their lease expires. A write
    /// transaction is acquired before selection so separate store instances
    /// cannot claim the same attempt.
    pub fn claim_notifications(
        &self,
        now: OffsetDateTime,
        limit: usize,
        lease_duration: TimeDuration,
    ) -> StoreResult<Vec<StoredNotification>> {
        if limit == 0 || i64::try_from(limit).is_err() {
            return Err(StoreError::InvalidInput { field: "limit" });
        }
        if !lease_duration.is_positive() {
            return Err(StoreError::InvalidInput {
                field: "lease_duration",
            });
        }
        let now = normalize_notification_time(now)?;
        let lease_until = now
            .checked_add(lease_duration)
            .ok_or(StoreError::InvalidInput {
                field: "lease_duration",
            })?;
        let now_key = format_notification_comparison_datetime(now)?;
        let lease_until_text = format_offset_datetime(lease_until)?;
        let updated_at = format_offset_datetime(OffsetDateTime::now_utc())?;
        let limit =
            i64::try_from(limit).map_err(|_| StoreError::InvalidInput { field: "limit" })?;

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let available_at_key = notification_timestamp_comparison_sql("available_at");
        let lease_until_key = notification_timestamp_comparison_sql("lease_until");
        let candidate_ids = {
            let query = format!(
                "SELECT id
                 FROM notification_outbox
                 WHERE (status = 'pending' AND {available_at_key} <= ?1)
                    OR (status = 'delivering' AND lease_until IS NOT NULL
                        AND {lease_until_key} <= ?1)
                 ORDER BY {available_at_key} ASC, id ASC
                 LIMIT ?2"
            );
            let mut statement = transaction.prepare(&query)?;
            statement
                .query_map(params![now_key, limit], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut claimed = Vec::with_capacity(candidate_ids.len());
        for id in candidate_ids {
            let query = format!(
                "UPDATE notification_outbox
                 SET status = 'delivering',
                     attempts = attempts + 1,
                     lease_until = ?1,
                     sent_at = NULL,
                     last_error_code = NULL,
                     updated_at = ?4
                 WHERE id = ?2
                   AND (
                       (status = 'pending' AND {available_at_key} <= ?3)
                       OR (status = 'delivering' AND lease_until IS NOT NULL
                           AND {lease_until_key} <= ?3)
                   )"
            );
            let updated =
                transaction.execute(&query, params![lease_until_text, id, now_key, updated_at])?;
            if updated != 1 {
                return Err(StoreError::CursorConflict {
                    resource: "notification",
                });
            }
            claimed.push(load_notification_from_connection(&transaction, id)?);
        }
        transaction.commit()?;
        Ok(claimed)
    }

    /// Marks one owned delivery attempt as sent.
    ///
    /// Retrying the same completion cursor and timestamp returns the original
    /// terminal row without changing its timestamp.
    pub fn mark_notification_sent(
        &self,
        id: i64,
        expected_attempt: i64,
        sent_at: OffsetDateTime,
    ) -> StoreResult<StoredNotification> {
        validate_notification_delivery_cursor(id, expected_attempt)?;
        let sent_at = normalize_notification_time(sent_at)?;
        let sent_at_text = format_offset_datetime(sent_at)?;
        let updated_at = format_offset_datetime(OffsetDateTime::now_utc())?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let existing = load_notification_from_connection(&transaction, id)?;
        if existing.status == NotificationStatus::Sent
            && existing.attempts == expected_attempt
            && existing.sent_at == Some(sent_at)
        {
            transaction.commit()?;
            return Ok(existing);
        }
        if existing.status != NotificationStatus::Delivering
            || existing.attempts != expected_attempt
        {
            return Err(StoreError::CursorConflict {
                resource: "notification",
            });
        }
        let updated = transaction.execute(
            "UPDATE notification_outbox
             SET status = 'sent',
                 lease_until = NULL,
                 sent_at = ?1,
                 last_error_code = NULL,
                 updated_at = ?4
             WHERE id = ?2 AND status = 'delivering' AND attempts = ?3",
            params![sent_at_text, id, expected_attempt, updated_at],
        )?;
        if updated != 1 {
            return Err(StoreError::CursorConflict {
                resource: "notification",
            });
        }
        let sent = load_notification_from_connection(&transaction, id)?;
        transaction.commit()?;
        Ok(sent)
    }

    /// Releases one owned delivery attempt for a later retry with a safe code.
    pub fn reschedule_notification(
        &self,
        id: i64,
        expected_attempt: i64,
        available_at: OffsetDateTime,
        error_code: impl AsRef<str>,
    ) -> StoreResult<StoredNotification> {
        validate_notification_delivery_cursor(id, expected_attempt)?;
        let available_at = normalize_notification_time(available_at)?;
        let available_at_text = format_offset_datetime(available_at)?;
        let updated_at = format_offset_datetime(OffsetDateTime::now_utc())?;
        let error_code = validate_notification_error_code(error_code.as_ref().to_owned())?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let existing = load_notification_from_connection(&transaction, id)?;
        if existing.status != NotificationStatus::Delivering
            || existing.attempts != expected_attempt
        {
            return Err(StoreError::CursorConflict {
                resource: "notification",
            });
        }
        let updated = transaction.execute(
            "UPDATE notification_outbox
             SET status = 'pending',
                 available_at = ?1,
                 lease_until = NULL,
                 sent_at = NULL,
                 last_error_code = ?2,
                 updated_at = ?5
             WHERE id = ?3 AND status = 'delivering' AND attempts = ?4",
            params![
                available_at_text,
                error_code,
                id,
                expected_attempt,
                updated_at
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::CursorConflict {
                resource: "notification",
            });
        }
        let pending = load_notification_from_connection(&transaction, id)?;
        transaction.commit()?;
        Ok(pending)
    }

    /// Deletes one notification by database ID.
    pub fn delete_notification_by_id(&self, id: i64) -> StoreResult<()> {
        let deleted = self
            .connection
            .execute("DELETE FROM notification_outbox WHERE id = ?1", params![id])?;
        if deleted == 0 {
            return Err(StoreError::NotFound {
                resource: "notification",
            });
        }
        Ok(())
    }

    /// Alias for [`Self::delete_notification_by_id`].
    pub fn delete_notification_outbox_by_id(&self, id: i64) -> StoreResult<()> {
        self.delete_notification_by_id(id)
    }
}

fn save_appointment_draft_in_transaction(
    transaction: &Transaction<'_>,
    source_id: &str,
    draft: &AppointmentDraft,
) -> StoreResult<StoredAppointmentDraft> {
    let idempotency_key = draft.idempotency_key().as_str().to_owned();
    let quote_id = draft.quote_id().to_string();
    let starts_at = format_offset_datetime(draft.starts_at())?;
    let ends_at = format_offset_datetime(draft.ends_at())?;
    let kind = appointment_kind_storage_name(draft.kind());
    let requester_included = i64::from(draft.requester_included());
    let caller_name = draft.caller().name().to_owned();
    let caller_email = draft.caller().email().to_owned();
    let inserted = transaction.execute(
        "INSERT INTO appointment_drafts (
             idempotency_key, source_id, quote_id, caller_name, caller_email,
             kind, starts_at, ends_at, requester_included
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT DO NOTHING",
        params![
            idempotency_key,
            source_id,
            quote_id,
            caller_name,
            caller_email,
            kind,
            starts_at,
            ends_at,
            requester_included,
        ],
    )?;
    if inserted == 1 {
        return Ok(StoredAppointmentDraft {
            id: transaction.last_insert_rowid(),
            source_id: source_id.to_owned(),
            draft: draft.clone(),
        });
    }

    let candidates =
        query_appointment_rows_by_identity(transaction, &idempotency_key, source_id, &quote_id)?;
    let mut exact_retry = None;
    for candidate in candidates {
        let stored = decode_appointment_row(candidate)?;
        if stored.source_id == source_id && stored.draft == *draft {
            exact_retry = Some(stored);
        } else {
            return Err(StoreError::Conflict {
                resource: "appointment draft",
            });
        }
    }
    exact_retry.ok_or_else(|| stored_record_invalid("appointment draft"))
}

impl ReplayGuard for PaStore {
    fn check_and_record(&mut self, nonce: &str, now: i64) -> bool {
        self.consume_replay_nonce(nonce, now).unwrap_or(false)
    }
}

const APPOINTMENT_DRAFT_SELECT: &str = "SELECT id, idempotency_key, source_id, quote_id, caller_name, caller_email, kind, \
            starts_at, ends_at, requester_included \
     FROM appointment_drafts WHERE id = ?1";
const APPOINTMENT_DRAFT_SELECT_BY_IDEMPOTENCY: &str = "SELECT id, idempotency_key, source_id, quote_id, caller_name, caller_email, kind, \
            starts_at, ends_at, requester_included \
     FROM appointment_drafts WHERE idempotency_key = ?1";
const OWNER_TASK_DRAFT_SELECT: &str = "SELECT id, idempotency_key, source_id, title, kind, duration_minutes, due_at \
     FROM owner_task_drafts WHERE id = ?1";
const OWNER_TASK_DRAFT_SELECT_BY_IDEMPOTENCY: &str = "SELECT id, idempotency_key, source_id, title, kind, duration_minutes, due_at \
     FROM owner_task_drafts WHERE idempotency_key = ?1";
const OWNER_TASK_PLACEMENT_SELECT: &str = "SELECT owner_task_draft_id, starts_at, ends_at, timezone, operation_key, owner_fingerprint, provider_event_id, state \
     FROM owner_task_placements WHERE owner_task_draft_id = ?1";
const OWNER_TASK_PLACEMENT_SELECT_BY_OPERATION_KEY: &str = "SELECT owner_task_draft_id, starts_at, ends_at, timezone, operation_key, owner_fingerprint, provider_event_id, state \
     FROM owner_task_placements WHERE operation_key = ?1";
const OWNER_TASK_PLACEMENT_SELECT_BY_PROVIDER_EVENT: &str = "SELECT owner_task_draft_id, starts_at, ends_at, timezone, operation_key, owner_fingerprint, provider_event_id, state \
     FROM owner_task_placements WHERE provider_event_id = ?1";
const PROPOSAL_SELECT: &str = "SELECT id, idempotency_key, source_id, appointment_draft_id, owner_task_draft_id, state, created_at, updated_at \
     FROM proposals WHERE id = ?1";
const PROPOSAL_SELECT_BY_IDEMPOTENCY: &str = "SELECT id, idempotency_key, source_id, appointment_draft_id, owner_task_draft_id, state, created_at, updated_at \
     FROM proposals WHERE idempotency_key = ?1";
const PROPOSAL_SELECT_BY_SOURCE: &str = "SELECT id, idempotency_key, source_id, appointment_draft_id, owner_task_draft_id, state, created_at, updated_at \
     FROM proposals WHERE source_id = ?1";
const EVENT_MAPPING_SELECT: &str = "SELECT id, proposal_id, provider, provider_event_id, source_id, starts_at, ends_at, created_at, updated_at \
     FROM event_mappings WHERE id = ?1";
const EVENT_MAPPING_SELECT_BY_PROPOSAL: &str = "SELECT id, proposal_id, provider, provider_event_id, source_id, starts_at, ends_at, created_at, updated_at \
     FROM event_mappings WHERE proposal_id = ?1";
const EVENT_MAPPING_SELECT_BY_PROVIDER_EVENT: &str = "SELECT id, proposal_id, provider, provider_event_id, source_id, starts_at, ends_at, created_at, updated_at \
     FROM event_mappings WHERE provider = ?1 AND provider_event_id = ?2";
const EVENT_MAPPING_SELECT_BY_SOURCE: &str = "SELECT id, proposal_id, provider, provider_event_id, source_id, starts_at, ends_at, created_at, updated_at \
     FROM event_mappings WHERE source_id = ?1";
const AUDIT_EVENT_SELECT_BY_IDEMPOTENCY: &str = "SELECT id, idempotency_key, event_type, entity_type, entity_id, details, occurred_at, created_at \
     FROM audit_events WHERE idempotency_key = ?1";
const MESSAGE_SELECT: &str = "SELECT id, idempotency_key, source_id, provider, provider_message_id, \
     summary, subject, sender, received_at, triage_state, created_at, updated_at \
     FROM messages WHERE id = ?1";
const MESSAGE_SELECT_BY_IDEMPOTENCY: &str = "SELECT id, idempotency_key, source_id, provider, provider_message_id, \
     summary, subject, sender, received_at, triage_state, created_at, updated_at \
     FROM messages WHERE idempotency_key = ?1";
const MESSAGE_SELECT_BY_SOURCE: &str = "SELECT id, idempotency_key, source_id, provider, provider_message_id, \
     summary, subject, sender, received_at, triage_state, created_at, updated_at \
     FROM messages WHERE source_id = ?1";
const MESSAGE_SELECT_BY_PROVIDER_MESSAGE: &str = "SELECT id, idempotency_key, source_id, provider, provider_message_id, \
     summary, subject, sender, received_at, triage_state, created_at, updated_at \
     FROM messages WHERE provider = ?1 AND provider_message_id = ?2";
const TASK_SELECT: &str = "SELECT id, idempotency_key, source_id, message_id, title, kind, \
     duration_minutes, due_at, status, created_at, updated_at \
     FROM tasks WHERE id = ?1";
const TASK_SELECT_BY_IDEMPOTENCY: &str = "SELECT id, idempotency_key, source_id, message_id, title, kind, \
     duration_minutes, due_at, status, created_at, updated_at \
     FROM tasks WHERE idempotency_key = ?1";
const TASK_SELECT_BY_SOURCE: &str = "SELECT id, idempotency_key, source_id, message_id, title, kind, \
     duration_minutes, due_at, status, created_at, updated_at \
     FROM tasks WHERE source_id = ?1";
const TASK_SELECT_ALL_ORDERED: &str = "SELECT id, idempotency_key, source_id, message_id, title, kind, \
     duration_minutes, due_at, status, created_at, updated_at \
     FROM tasks ORDER BY id ASC";
const NOTIFICATION_SELECT: &str = "SELECT id, idempotency_key, proposal_id, event_mapping_id, notification_kind, recipient, payload, status, available_at, lease_until, sent_at, last_error_code, attempts, created_at, updated_at \
     FROM notification_outbox WHERE id = ?1";
const NOTIFICATION_SELECT_BY_IDEMPOTENCY: &str = "SELECT id, idempotency_key, proposal_id, event_mapping_id, notification_kind, recipient, payload, status, available_at, lease_until, sent_at, last_error_code, attempts, created_at, updated_at \
     FROM notification_outbox WHERE idempotency_key = ?1";

struct AppointmentStoredRow {
    id: i64,
    idempotency_key: String,
    source_id: String,
    quote_id: String,
    caller_name: String,
    caller_email: String,
    kind: String,
    starts_at: String,
    ends_at: String,
    requester_included: i64,
}

struct AppointmentQuoteStoredRow {
    quote_id: String,
    appointment_kind: String,
    timezone: String,
    issued_at: String,
    expires_at: String,
    slot_count: i64,
    state: String,
    appointment_draft_id: Option<i64>,
    selected_slot_index: Option<i64>,
    consumed_at: Option<String>,
    proposal_id: Option<i64>,
}

struct AppointmentQuoteSlotStoredRow {
    slot_index: i64,
    starts_at: String,
    ends_at: String,
}

struct OwnerTaskStoredRow {
    id: i64,
    idempotency_key: String,
    source_id: Option<String>,
    title: String,
    kind: String,
    duration_minutes: i64,
    due_at: Option<String>,
}

struct OwnerTaskPlacementStoredRow {
    owner_task_draft_id: i64,
    starts_at: String,
    ends_at: String,
    timezone: String,
    operation_key: String,
    owner_fingerprint: String,
    provider_event_id: Option<String>,
    state: String,
}

struct ProposalStoredRow {
    id: i64,
    idempotency_key: String,
    source_id: String,
    appointment_draft_id: Option<i64>,
    owner_task_draft_id: Option<i64>,
    state: String,
    created_at: String,
    updated_at: String,
}

struct EventMappingStoredRow {
    id: i64,
    proposal_id: i64,
    provider: String,
    provider_event_id: String,
    source_id: String,
    starts_at: Option<String>,
    ends_at: Option<String>,
    created_at: String,
    updated_at: String,
}

struct AuditEventStoredRow {
    id: i64,
    idempotency_key: String,
    event_type: String,
    entity_type: String,
    entity_id: String,
    details: Option<String>,
    occurred_at: String,
    created_at: String,
}

struct MessageStoredRow {
    id: i64,
    idempotency_key: String,
    source_id: String,
    provider: String,
    provider_message_id: String,
    summary: String,
    subject: Option<String>,
    sender: Option<String>,
    received_at: String,
    triage_state: String,
    created_at: String,
    updated_at: String,
}

struct TaskStoredRow {
    id: i64,
    idempotency_key: String,
    source_id: String,
    message_id: i64,
    title: String,
    kind: String,
    duration_minutes: i64,
    due_at: Option<String>,
    state: String,
    created_at: String,
    updated_at: String,
}

struct NotificationStoredRow {
    id: i64,
    idempotency_key: String,
    proposal_id: Option<i64>,
    event_mapping_id: Option<i64>,
    notification_kind: String,
    recipient: String,
    payload: Option<String>,
    status: String,
    available_at: String,
    lease_until: Option<String>,
    sent_at: Option<String>,
    last_error_code: Option<String>,
    attempts: i64,
    created_at: String,
    updated_at: String,
}

struct LegacyNotificationOutboxRow {
    id: i64,
    idempotency_key: String,
    proposal_id: Option<i64>,
    event_mapping_id: Option<i64>,
    notification_kind: String,
    recipient: String,
    payload: Option<String>,
    status: String,
    available_at: String,
    sent_at: Option<String>,
    attempts: i64,
    created_at: String,
    updated_at: String,
}

fn read_appointment_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppointmentStoredRow> {
    Ok(AppointmentStoredRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        source_id: row.get(2)?,
        quote_id: row.get(3)?,
        caller_name: row.get(4)?,
        caller_email: row.get(5)?,
        kind: row.get(6)?,
        starts_at: row.get(7)?,
        ends_at: row.get(8)?,
        requester_included: row.get(9)?,
    })
}

fn read_appointment_quote_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AppointmentQuoteStoredRow> {
    Ok(AppointmentQuoteStoredRow {
        quote_id: row.get(0)?,
        appointment_kind: row.get(1)?,
        timezone: row.get(2)?,
        issued_at: row.get(3)?,
        expires_at: row.get(4)?,
        slot_count: row.get(5)?,
        state: row.get(6)?,
        appointment_draft_id: row.get(7)?,
        selected_slot_index: row.get(8)?,
        consumed_at: row.get(9)?,
        proposal_id: row.get(10)?,
    })
}

fn read_appointment_quote_slot_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AppointmentQuoteSlotStoredRow> {
    Ok(AppointmentQuoteSlotStoredRow {
        slot_index: row.get(0)?,
        starts_at: row.get(1)?,
        ends_at: row.get(2)?,
    })
}

fn read_owner_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OwnerTaskStoredRow> {
    Ok(OwnerTaskStoredRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        source_id: row.get(2)?,
        title: row.get(3)?,
        kind: row.get(4)?,
        duration_minutes: row.get(5)?,
        due_at: row.get(6)?,
    })
}

fn read_owner_task_placement_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<OwnerTaskPlacementStoredRow> {
    Ok(OwnerTaskPlacementStoredRow {
        owner_task_draft_id: row.get(0)?,
        starts_at: row.get(1)?,
        ends_at: row.get(2)?,
        timezone: row.get(3)?,
        operation_key: row.get(4)?,
        owner_fingerprint: row.get(5)?,
        provider_event_id: row.get(6)?,
        state: row.get(7)?,
    })
}

fn read_proposal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProposalStoredRow> {
    Ok(ProposalStoredRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        source_id: row.get(2)?,
        appointment_draft_id: row.get(3)?,
        owner_task_draft_id: row.get(4)?,
        state: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn read_event_mapping_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventMappingStoredRow> {
    Ok(EventMappingStoredRow {
        id: row.get(0)?,
        proposal_id: row.get(1)?,
        provider: row.get(2)?,
        provider_event_id: row.get(3)?,
        source_id: row.get(4)?,
        starts_at: row.get(5)?,
        ends_at: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn read_audit_event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEventStoredRow> {
    Ok(AuditEventStoredRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        event_type: row.get(2)?,
        entity_type: row.get(3)?,
        entity_id: row.get(4)?,
        details: row.get(5)?,
        occurred_at: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn read_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageStoredRow> {
    Ok(MessageStoredRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        source_id: row.get(2)?,
        provider: row.get(3)?,
        provider_message_id: row.get(4)?,
        summary: row.get(5)?,
        subject: row.get(6)?,
        sender: row.get(7)?,
        received_at: row.get(8)?,
        triage_state: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn read_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskStoredRow> {
    Ok(TaskStoredRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        source_id: row.get(2)?,
        message_id: row.get(3)?,
        title: row.get(4)?,
        kind: row.get(5)?,
        duration_minutes: row.get(6)?,
        due_at: row.get(7)?,
        state: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn read_notification_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NotificationStoredRow> {
    Ok(NotificationStoredRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        proposal_id: row.get(2)?,
        event_mapping_id: row.get(3)?,
        notification_kind: row.get(4)?,
        recipient: row.get(5)?,
        payload: row.get(6)?,
        status: row.get(7)?,
        available_at: row.get(8)?,
        lease_until: row.get(9)?,
        sent_at: row.get(10)?,
        last_error_code: row.get(11)?,
        attempts: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn load_notification_from_connection(
    connection: &Connection,
    id: i64,
) -> StoreResult<StoredNotification> {
    let row = connection
        .query_row(NOTIFICATION_SELECT, params![id], read_notification_row)
        .optional()
        .map_err(|_| stored_record_invalid("notification"))?
        .ok_or(StoreError::NotFound {
            resource: "notification",
        })?;
    let notification = decode_notification_row(row)?;
    validate_stored_notification_references(connection, &notification)?;
    Ok(notification)
}

fn query_appointment_rows_by_identity(
    connection: &Connection,
    idempotency_key: &str,
    source_id: &str,
    quote_id: &str,
) -> StoreResult<Vec<AppointmentStoredRow>> {
    let mut statement = connection
        .prepare(
            "SELECT id, idempotency_key, source_id, quote_id, caller_name, caller_email, kind, \
                    starts_at, ends_at, requester_included \
             FROM appointment_drafts \
             WHERE idempotency_key = ?1 OR source_id = ?2 OR quote_id = ?3",
        )
        .map_err(|_| stored_record_invalid("appointment draft"))?;
    let rows = statement
        .query_map(
            params![idempotency_key, source_id, quote_id],
            read_appointment_row,
        )
        .map_err(|_| stored_record_invalid("appointment draft"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| stored_record_invalid("appointment draft"))?;
    Ok(rows)
}

fn query_owner_task_rows_by_identity(
    connection: &Connection,
    idempotency_key: &str,
    source_id: Option<&str>,
) -> StoreResult<Vec<OwnerTaskStoredRow>> {
    let mut statement = connection
        .prepare(
            "SELECT id, idempotency_key, source_id, title, kind, duration_minutes, due_at \
             FROM owner_task_drafts \
             WHERE idempotency_key = ?1 OR (?2 IS NOT NULL AND source_id = ?2)",
        )
        .map_err(|_| stored_record_invalid("owner task draft"))?;
    let rows = statement
        .query_map(params![idempotency_key, source_id], read_owner_task_row)
        .map_err(|_| stored_record_invalid("owner task draft"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| stored_record_invalid("owner task draft"))?;
    Ok(rows)
}

fn query_proposal_rows_by_identity(
    connection: &Connection,
    idempotency_key: &str,
    source_id: &str,
) -> StoreResult<Vec<ProposalStoredRow>> {
    let mut statement = connection
        .prepare(
            "SELECT id, idempotency_key, source_id, appointment_draft_id, \
                    owner_task_draft_id, state, created_at, updated_at
             FROM proposals
             WHERE idempotency_key = ?1 OR source_id = ?2",
        )
        .map_err(|_| stored_record_invalid("proposal"))?;
    let rows = statement
        .query_map(params![idempotency_key, source_id], read_proposal_row)
        .map_err(|_| stored_record_invalid("proposal"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| stored_record_invalid("proposal"))?;
    Ok(rows)
}

fn query_event_mapping_rows_by_identity(
    connection: &Connection,
    proposal_id: i64,
    provider: &str,
    provider_event_id: &str,
    source_id: &str,
) -> StoreResult<Vec<EventMappingStoredRow>> {
    let mut statement = connection
        .prepare(
            "SELECT id, proposal_id, provider, provider_event_id, source_id, \
                    starts_at, ends_at, created_at, updated_at
             FROM event_mappings
             WHERE proposal_id = ?1
                OR (provider = ?2 AND provider_event_id = ?3)
                OR source_id = ?4",
        )
        .map_err(|_| stored_record_invalid("event mapping"))?;
    let rows = statement
        .query_map(
            params![proposal_id, provider, provider_event_id, source_id],
            read_event_mapping_row,
        )
        .map_err(|_| stored_record_invalid("event mapping"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| stored_record_invalid("event mapping"))?;
    Ok(rows)
}

fn query_message_rows_by_identity(
    connection: &Connection,
    idempotency_key: &str,
    source_id: &str,
    provider: MessageProvider,
    provider_message_id: &str,
) -> StoreResult<Vec<MessageStoredRow>> {
    let mut statement = connection
        .prepare(
            "SELECT id, idempotency_key, source_id, provider, provider_message_id,
                    summary, subject, sender, received_at, triage_state, created_at,
                    updated_at
             FROM messages
             WHERE idempotency_key = ?1 OR source_id = ?2
                OR (provider = ?3 AND provider_message_id = ?4)",
        )
        .map_err(|_| stored_record_invalid("message"))?;
    let rows = statement
        .query_map(
            params![
                idempotency_key,
                source_id,
                provider.as_str(),
                provider_message_id
            ],
            read_message_row,
        )
        .map_err(|_| stored_record_invalid("message"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| stored_record_invalid("message"))?;
    Ok(rows)
}

fn query_task_rows_by_identity(
    transaction: &Transaction<'_>,
    idempotency_key: &str,
    source_id: &str,
) -> StoreResult<Vec<TaskStoredRow>> {
    let mut statement = transaction
        .prepare(
            "SELECT id, idempotency_key, source_id, message_id, title, kind,
                    duration_minutes, due_at, status, created_at, updated_at
             FROM tasks
             WHERE idempotency_key = ?1 OR source_id = ?2",
        )
        .map_err(|_| stored_record_invalid("task"))?;
    statement
        .query_map(params![idempotency_key, source_id], read_task_row)
        .map_err(|_| stored_record_invalid("task"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| stored_record_invalid("task"))
}

fn validate_proposal_source(source: ProposalSource) -> StoreResult<()> {
    match source {
        ProposalSource::AppointmentDraft { id } | ProposalSource::OwnerTaskDraft { id }
            if id > 0 =>
        {
            Ok(())
        }
        _ => Err(StoreError::InvalidInput { field: "source" }),
    }
}

fn ensure_proposal_source_exists(
    transaction: &Transaction<'_>,
    source: ProposalSource,
) -> StoreResult<()> {
    let (table, id, resource) = match source {
        ProposalSource::AppointmentDraft { id } => ("appointment_drafts", id, "appointment draft"),
        ProposalSource::OwnerTaskDraft { id } => ("owner_task_drafts", id, "owner task draft"),
    };
    let sql = format!("SELECT id FROM {table} WHERE id = ?1");
    let exists = transaction
        .query_row(&sql, params![id], |row| row.get::<_, i64>(0))
        .optional()?;
    if exists.is_none() {
        return Err(StoreError::NotFound { resource });
    }
    Ok(())
}

fn create_proposal_in_transaction(
    transaction: &Transaction<'_>,
    idempotency_key: &str,
    source_id: &str,
    source: ProposalSource,
) -> StoreResult<StoredProposal> {
    ensure_proposal_source_exists(transaction, source)?;
    let inserted = transaction.execute(
        "INSERT INTO proposals (
             idempotency_key, source_id, appointment_draft_id,
             owner_task_draft_id, state
         ) VALUES (?1, ?2, ?3, ?4, 'pending')
         ON CONFLICT DO NOTHING",
        params![
            idempotency_key,
            source_id,
            source.appointment_draft_id(),
            source.owner_task_draft_id(),
        ],
    )?;
    if inserted == 1 {
        let id = transaction.last_insert_rowid();
        let row = transaction
            .query_row(PROPOSAL_SELECT, params![id], read_proposal_row)
            .map_err(|_| stored_record_invalid("proposal"))?;
        return decode_proposal_row(row);
    }

    let candidates = query_proposal_rows_by_identity(transaction, idempotency_key, source_id)?;
    let mut exact_retry = None;
    for candidate in candidates {
        let stored = decode_proposal_row(candidate)?;
        if stored.idempotency_key == idempotency_key
            && stored.source_id == source_id
            && stored.source == source
        {
            exact_retry = Some(stored);
        } else {
            return Err(StoreError::Conflict {
                resource: "proposal",
            });
        }
    }
    exact_retry.ok_or_else(|| stored_record_invalid("proposal"))
}

fn decode_proposal_row(row: ProposalStoredRow) -> StoreResult<StoredProposal> {
    if row.id <= 0 {
        return Err(stored_record_invalid("proposal"));
    }
    let idempotency_key =
        IdempotencyKey::new(row.idempotency_key).map_err(|_| stored_record_invalid("proposal"))?;
    let source_id =
        validate_source_id(row.source_id).map_err(|_| stored_record_invalid("proposal"))?;
    let source = ProposalSource::from_ids(row.appointment_draft_id, row.owner_task_draft_id)
        .map_err(|_| stored_record_invalid("proposal"))?;
    let state = parse_proposal_state(&row.state)?;
    let created_at = validate_stored_timestamp(row.created_at, "proposal")?;
    let updated_at = validate_stored_timestamp(row.updated_at, "proposal")?;
    Ok(StoredProposal {
        id: row.id,
        idempotency_key: idempotency_key.into(),
        source_id,
        source,
        state,
        created_at,
        updated_at,
    })
}

fn decode_event_mapping_row(row: EventMappingStoredRow) -> StoreResult<StoredEventMapping> {
    if row.id <= 0 || row.proposal_id <= 0 {
        return Err(stored_record_invalid("event mapping"));
    }
    let provider = validate_non_empty(row.provider, "provider")
        .map_err(|_| stored_record_invalid("event mapping"))?;
    let provider_event_id = validate_non_empty(row.provider_event_id, "provider_event_id")
        .map_err(|_| stored_record_invalid("event mapping"))?;
    let source_id =
        validate_source_id(row.source_id).map_err(|_| stored_record_invalid("event mapping"))?;
    let starts_at = row
        .starts_at
        .as_deref()
        .map(parse_offset_datetime)
        .transpose()
        .map_err(|_| stored_record_invalid("event mapping"))?;
    let ends_at = row
        .ends_at
        .as_deref()
        .map(parse_offset_datetime)
        .transpose()
        .map_err(|_| stored_record_invalid("event mapping"))?;
    if starts_at.is_some() != ends_at.is_some()
        || matches!((starts_at, ends_at), (Some(start), Some(end)) if start >= end)
    {
        return Err(stored_record_invalid("event mapping"));
    }
    let created_at = validate_stored_timestamp(row.created_at, "event mapping")?;
    let updated_at = validate_stored_timestamp(row.updated_at, "event mapping")?;
    Ok(StoredEventMapping {
        id: row.id,
        proposal_id: row.proposal_id,
        provider,
        provider_event_id,
        source_id,
        starts_at,
        ends_at,
        created_at,
        updated_at,
    })
}

fn decode_audit_event_row(row: AuditEventStoredRow) -> StoreResult<StoredAuditEvent> {
    if row.id <= 0 || row.details.is_some() {
        return Err(stored_record_invalid("audit event"));
    }
    let idempotency_key = validate_audit_idempotency_key(row.idempotency_key)
        .map_err(|_| stored_record_invalid("audit event"))?;
    let event_type = AuditEventType::from_storage(&row.event_type)
        .map_err(|_| stored_record_invalid("audit event"))?;
    let entity_type = AuditEntityType::from_storage(&row.entity_type)
        .map_err(|_| stored_record_invalid("audit event"))?;
    let entity_id = validate_audit_entity_id(row.entity_id)
        .map_err(|_| stored_record_invalid("audit event"))?;
    let occurred_at = parse_audit_timestamp(row.occurred_at, "audit event")?;
    let created_at = parse_audit_timestamp(row.created_at, "audit event")?
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid("audit event"))?;
    Ok(StoredAuditEvent {
        id: row.id,
        idempotency_key,
        event_type,
        entity_type,
        entity_id,
        occurred_at,
        created_at,
    })
}

fn decode_message_row(row: MessageStoredRow) -> StoreResult<StoredMessage> {
    if row.id <= 0 {
        return Err(stored_record_invalid("message"));
    }
    let idempotency_key = validate_message_idempotency_key(row.idempotency_key)
        .map_err(|_| stored_record_invalid("message"))?;
    let source_id =
        validate_message_source_id(row.source_id).map_err(|_| stored_record_invalid("message"))?;
    let provider = MessageProvider::from_storage(&row.provider)
        .map_err(|_| stored_record_invalid("message"))?;
    let provider_message_id = validate_provider_message_id(row.provider_message_id)
        .map_err(|_| stored_record_invalid("message"))?;
    let summary = MessageSummary::new(row.summary).map_err(|_| stored_record_invalid("message"))?;
    let subject =
        validate_message_subject(row.subject).map_err(|_| stored_record_invalid("message"))?;
    let sender =
        validate_message_sender(row.sender).map_err(|_| stored_record_invalid("message"))?;
    let received_at = parse_message_timestamp(row.received_at)?;
    let triage_state = MessageTriageState::from_storage(&row.triage_state)
        .map_err(|_| stored_record_invalid("message"))?;
    if !valid_message_state(provider, triage_state) {
        return Err(stored_record_invalid("message"));
    }
    let created_at = parse_message_timestamp_text(row.created_at)?;
    let updated_at = parse_message_timestamp_text(row.updated_at)?;
    Ok(StoredMessage {
        id: row.id,
        idempotency_key,
        source_id,
        provider,
        provider_message_id,
        summary,
        subject,
        sender,
        received_at,
        triage_state,
        created_at,
        updated_at,
    })
}

fn decode_task_row(row: TaskStoredRow) -> StoreResult<StoredTask> {
    if row.id <= 0 || row.message_id <= 0 {
        return Err(stored_record_invalid("task"));
    }
    let idempotency_key = validate_task_idempotency_key(row.idempotency_key)
        .map_err(|_| stored_record_invalid("task"))?;
    let source_id =
        validate_task_source_id(row.source_id).map_err(|_| stored_record_invalid("task"))?;
    let title = TaskTitle::new(row.title).map_err(|_| stored_record_invalid("task"))?;
    let kind = parse_task_kind(&row.kind).map_err(|_| stored_record_invalid("task"))?;
    let duration = validate_task_duration_i64(row.duration_minutes)
        .map_err(|_| stored_record_invalid("task"))?;
    let due_at = row
        .due_at
        .map(|value| parse_task_timestamp(value, "task"))
        .transpose()?;
    let state =
        StoredTaskState::from_storage(&row.state).map_err(|_| stored_record_invalid("task"))?;
    let created_at = parse_task_timestamp_text(row.created_at)?;
    let updated_at = parse_task_timestamp_text(row.updated_at)?;
    Ok(StoredTask {
        id: row.id,
        idempotency_key,
        source_id,
        message_id: row.message_id,
        title,
        kind,
        duration,
        due_at,
        state,
        created_at,
        updated_at,
    })
}

fn validate_actionable_message(message: &StoredMessage, resource: &'static str) -> StoreResult<()> {
    if !matches!(
        message.provider,
        MessageProvider::Outlook | MessageProvider::Gmail
    ) || message.triage_state != MessageTriageState::Actionable
    {
        return Err(stored_record_invalid(resource));
    }
    Ok(())
}

fn validate_task_message_in_transaction(
    transaction: &Transaction<'_>,
    message_id: i64,
) -> StoreResult<()> {
    let row = transaction
        .query_row(MESSAGE_SELECT, params![message_id], read_message_row)
        .optional()
        .map_err(|_| stored_record_invalid("task"))?
        .ok_or_else(|| stored_record_invalid("task"))?;
    let message = decode_message_row(row).map_err(|_| stored_record_invalid("task"))?;
    validate_actionable_message(&message, "task")
}

fn validate_task_message_in_lifecycle_transaction(
    transaction: &Transaction<'_>,
    message_id: i64,
) -> StoreResult<()> {
    let row = transaction
        .query_row(MESSAGE_SELECT, params![message_id], read_message_row)
        .optional()
        .map_err(|_| stored_record_invalid("task"))?
        .ok_or_else(|| stored_record_invalid("task"))?;
    let message = decode_message_row(row).map_err(|_| stored_record_invalid("task"))?;
    validate_task_message_lifecycle(&message, "task")
}

fn validate_task_message_on_connection(
    connection: &Connection,
    message_id: i64,
) -> StoreResult<()> {
    let row = connection
        .query_row(MESSAGE_SELECT, params![message_id], read_message_row)
        .optional()
        .map_err(|_| stored_record_invalid("task"))?
        .ok_or_else(|| stored_record_invalid("task"))?;
    let message = decode_message_row(row).map_err(|_| stored_record_invalid("task"))?;
    validate_task_message_lifecycle(&message, "task")
}

fn validate_task_message_lifecycle(
    message: &StoredMessage,
    resource: &'static str,
) -> StoreResult<()> {
    if !matches!(
        (message.provider, message.triage_state),
        (
            MessageProvider::Outlook | MessageProvider::Gmail,
            MessageTriageState::Actionable | MessageTriageState::Scheduled
        )
    ) {
        return Err(stored_record_invalid(resource));
    }
    Ok(())
}

fn decode_notification_row(row: NotificationStoredRow) -> StoreResult<StoredNotification> {
    if row.id <= 0 || row.attempts < 0 {
        return Err(stored_record_invalid("notification"));
    }
    let idempotency_key = IdempotencyKey::new(row.idempotency_key)
        .map_err(|_| stored_record_invalid("notification"))?;
    let proposal_id = validate_stored_reference(row.proposal_id, "notification")?;
    let event_mapping_id = validate_stored_reference(row.event_mapping_id, "notification")?;
    let kind = parse_notification_kind(&row.notification_kind)?;
    if kind.requires_proposal() != proposal_id.is_some()
        || (!kind.requires_proposal() && event_mapping_id.is_some())
    {
        return Err(stored_record_invalid("notification"));
    }
    let recipient = NotificationRecipient::new(row.recipient)
        .map_err(|_| stored_record_invalid("notification"))?;
    let payload = row
        .payload
        .ok_or_else(|| stored_record_invalid("notification"))?;
    let template_data = serde_json::from_str::<NotificationTemplateData>(&payload)
        .map_err(|_| stored_record_invalid("notification"))?;
    let status = parse_notification_status(&row.status)?;
    let available_at = parse_stored_notification_timestamp(&row.available_at)?;
    let lease_until = row
        .lease_until
        .as_deref()
        .map(parse_stored_notification_timestamp)
        .transpose()?;
    let sent_at = row
        .sent_at
        .as_deref()
        .map(parse_stored_notification_timestamp)
        .transpose()?;
    let last_error_code = row
        .last_error_code
        .map(validate_notification_error_code)
        .transpose()
        .map_err(|_| stored_record_invalid("notification"))?;
    match status {
        NotificationStatus::Pending if lease_until.is_none() && sent_at.is_none() => {}
        NotificationStatus::Delivering
            if row.attempts > 0
                && lease_until.is_some()
                && sent_at.is_none()
                && last_error_code.is_none() => {}
        NotificationStatus::Sent
            if row.attempts > 0
                && lease_until.is_none()
                && sent_at.is_some()
                && last_error_code.is_none() => {}
        _ => return Err(stored_record_invalid("notification")),
    }
    let created_at = validate_stored_notification_metadata_timestamp(row.created_at)?;
    let updated_at = validate_stored_notification_metadata_timestamp(row.updated_at)?;
    Ok(StoredNotification {
        id: row.id,
        idempotency_key: idempotency_key.into(),
        proposal_id,
        event_mapping_id,
        kind,
        recipient,
        template_data,
        status,
        available_at,
        lease_until,
        sent_at,
        last_error_code,
        attempts: row.attempts,
        created_at,
        updated_at,
    })
}

fn validate_stored_notification_references(
    connection: &Connection,
    notification: &StoredNotification,
) -> StoreResult<()> {
    if let Some(proposal_id) = notification.proposal_id {
        let exists = connection
            .query_row(
                "SELECT id FROM proposals WHERE id = ?1",
                params![proposal_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| stored_record_invalid("notification"))?;
        if exists.is_none() {
            return Err(stored_record_invalid("notification"));
        }
    }
    if let Some(event_mapping_id) = notification.event_mapping_id {
        let mapping_proposal_id = connection
            .query_row(
                "SELECT proposal_id FROM event_mappings WHERE id = ?1",
                params![event_mapping_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| stored_record_invalid("notification"))?
            .ok_or_else(|| stored_record_invalid("notification"))?;
        if Some(mapping_proposal_id) != notification.proposal_id {
            return Err(stored_record_invalid("notification"));
        }
    }
    Ok(())
}

fn validate_stored_reference(
    value: Option<i64>,
    resource: &'static str,
) -> StoreResult<Option<i64>> {
    if value.is_some_and(|id| id <= 0) {
        return Err(stored_record_invalid(resource));
    }
    Ok(value)
}

fn parse_notification_kind(value: &str) -> StoreResult<NotificationKind> {
    match value {
        "call_summary" => Ok(NotificationKind::CallSummary),
        "proposal_requested" => Ok(NotificationKind::ProposalRequested),
        "proposal_accepted" => Ok(NotificationKind::ProposalAccepted),
        "proposal_declined" => Ok(NotificationKind::ProposalDeclined),
        "proposal_expired" => Ok(NotificationKind::ProposalExpired),
        _ => Err(stored_record_invalid("notification")),
    }
}

fn parse_notification_status(value: &str) -> StoreResult<NotificationStatus> {
    match value {
        "pending" => Ok(NotificationStatus::Pending),
        "delivering" => Ok(NotificationStatus::Delivering),
        "sent" => Ok(NotificationStatus::Sent),
        _ => Err(stored_record_invalid("notification")),
    }
}

fn validate_notification_error_code(value: String) -> StoreResult<String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(StoreError::InvalidInput {
            field: "error_code",
        });
    }
    Ok(value)
}

fn validate_notification_delivery_cursor(id: i64, expected_attempt: i64) -> StoreResult<()> {
    if id <= 0 {
        return Err(StoreError::InvalidInput { field: "id" });
    }
    if expected_attempt <= 0 {
        return Err(StoreError::InvalidInput {
            field: "expected_attempt",
        });
    }
    Ok(())
}

fn parse_stored_notification_timestamp(value: &str) -> StoreResult<OffsetDateTime> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| stored_record_invalid("notification"))?
        .to_offset(time::UtcOffset::UTC);
    let canonical =
        format_offset_datetime(parsed).map_err(|_| stored_record_invalid("notification"))?;
    if canonical != value {
        return Err(stored_record_invalid("notification"));
    }
    Ok(parsed)
}

fn validate_stored_notification_metadata_timestamp(value: String) -> StoreResult<String> {
    let parsed = parse_stored_notification_timestamp(&value)?;
    format_offset_datetime(parsed).map_err(|_| stored_record_invalid("notification"))
}

fn parse_legacy_notification_timestamp(value: &str) -> Option<OffsetDateTime> {
    if let Ok(parsed) = OffsetDateTime::parse(value, &Rfc3339) {
        return Some(parsed.to_offset(time::UtcOffset::UTC));
    }
    let parsed = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S"))
        .ok()?
        .and_utc();
    let seconds = OffsetDateTime::from_unix_timestamp(parsed.timestamp()).ok()?;
    seconds.checked_add(TimeDuration::nanoseconds(i64::from(
        parsed.timestamp_subsec_nanos(),
    )))
}

fn normalize_legacy_notification_timestamp(value: &str) -> StoreResult<String> {
    let parsed = parse_legacy_notification_timestamp(value)
        .ok_or_else(|| stored_record_invalid("notification"))?;
    format_offset_datetime(parsed.to_offset(time::UtcOffset::UTC))
        .map_err(|_| stored_record_invalid("notification"))
}

fn normalize_notification_time(value: OffsetDateTime) -> StoreResult<OffsetDateTime> {
    Ok(value.to_offset(time::UtcOffset::UTC))
}

/// Formats a notification timestamp as a fixed-width UTC comparison key.
///
/// Persisted notification timestamps retain their RFC3339 representation,
/// which omits trailing fractional zeroes. SQL comparisons therefore use a
/// fixed nine-digit fractional representation instead of comparing the raw
/// variable-width text.
fn format_notification_comparison_datetime(value: OffsetDateTime) -> StoreResult<String> {
    let text = format_offset_datetime(normalize_notification_time(value)?)?;
    let without_offset = text
        .strip_suffix('Z')
        .ok_or_else(|| stored_record_invalid("notification"))?;
    let (whole, fraction) = without_offset
        .split_once('.')
        .unwrap_or((without_offset, ""));
    if fraction.len() > 9 {
        return Err(stored_record_invalid("notification"));
    }
    let mut key = String::with_capacity(whole.len() + 11);
    key.push_str(whole);
    key.push('.');
    key.push_str(fraction);
    for _ in fraction.len()..9 {
        key.push('0');
    }
    key.push('Z');
    Ok(key)
}

/// Returns the SQL expression that normalizes a persisted notification time
/// to the same fixed-width comparison key as
/// [`format_notification_comparison_datetime`].
fn notification_timestamp_comparison_sql(column: &str) -> String {
    format!(
        "CASE WHEN instr({column}, '.') = 0 \
         THEN replace({column}, 'Z', '.000000000Z') \
         ELSE substr({column}, 1, instr({column}, '.')) \
              || replace(printf('%-9s', substr({column}, instr({column}, '.') + 1, \
                 length({column}) - instr({column}, '.') - 1)), ' ', '0') \
              || 'Z' END"
    )
}

fn validate_notification_references(
    transaction: &Transaction<'_>,
    proposal_id: Option<i64>,
    event_mapping_id: Option<i64>,
    kind: NotificationKind,
) -> StoreResult<()> {
    if proposal_id.is_some_and(|id| id <= 0) {
        return Err(StoreError::InvalidInput {
            field: "proposal_id",
        });
    }
    if event_mapping_id.is_some_and(|id| id <= 0) {
        return Err(StoreError::InvalidInput {
            field: "event_mapping_id",
        });
    }
    if kind.requires_proposal() && proposal_id.is_none() {
        return Err(StoreError::InvalidInput {
            field: "proposal_id",
        });
    }
    if !kind.requires_proposal() {
        if proposal_id.is_some() {
            return Err(StoreError::InvalidInput {
                field: "proposal_id",
            });
        }
        if event_mapping_id.is_some() {
            return Err(StoreError::InvalidInput {
                field: "event_mapping_id",
            });
        }
    }

    if let Some(proposal_id) = proposal_id {
        let exists = transaction
            .query_row(
                "SELECT id FROM proposals WHERE id = ?1",
                params![proposal_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(StoreError::NotFound {
                resource: "proposal",
            });
        }
    }
    if let Some(event_mapping_id) = event_mapping_id {
        let mapping_proposal_id = transaction
            .query_row(
                "SELECT proposal_id FROM event_mappings WHERE id = ?1",
                params![event_mapping_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let mapping_proposal_id = mapping_proposal_id.ok_or(StoreError::NotFound {
            resource: "event mapping",
        })?;
        if Some(mapping_proposal_id) != proposal_id {
            return Err(StoreError::Conflict {
                resource: "notification",
            });
        }
    }
    Ok(())
}

fn parse_proposal_state(value: &str) -> StoreResult<ProposalState> {
    match value {
        "pending" => Ok(ProposalState::Pending),
        "accepted" => Ok(ProposalState::Accepted),
        "declined" => Ok(ProposalState::Declined),
        "expired" => Ok(ProposalState::Expired),
        _ => Err(stored_record_invalid("proposal")),
    }
}

fn proposal_state_storage_name(state: ProposalState) -> &'static str {
    match state {
        ProposalState::Pending => "pending",
        ProposalState::Accepted => "accepted",
        ProposalState::Declined => "declined",
        ProposalState::Expired => "expired",
    }
}

fn validate_event_times(
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
) -> StoreResult<()> {
    match (starts_at, ends_at) {
        (None, None) => Ok(()),
        (Some(start), Some(end)) if start < end => Ok(()),
        (Some(_), None) => Err(StoreError::InvalidInput { field: "ends_at" }),
        (None, Some(_)) => Err(StoreError::InvalidInput { field: "starts_at" }),
        (Some(_), Some(_)) => Err(StoreError::InvalidInput {
            field: "event_times",
        }),
    }
}

fn validate_stored_timestamp(value: String, resource: &'static str) -> StoreResult<String> {
    let parsed = NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| stored_record_invalid(resource))?;
    if parsed.format("%Y-%m-%d %H:%M:%S").to_string() != value {
        return Err(stored_record_invalid(resource));
    }
    Ok(value)
}

fn decode_appointment_row(row: AppointmentStoredRow) -> StoreResult<StoredAppointmentDraft> {
    if row.id <= 0 {
        return Err(stored_record_invalid("appointment draft"));
    }
    let source_id = validate_source_id(row.source_id)
        .map_err(|_| stored_record_invalid("appointment draft"))?;
    let idempotency_key = IdempotencyKey::new(row.idempotency_key)
        .map_err(|_| stored_record_invalid("appointment draft"))?;
    let quote_uuid =
        Uuid::parse_str(&row.quote_id).map_err(|_| stored_record_invalid("appointment draft"))?;
    if quote_uuid.to_string() != row.quote_id {
        return Err(stored_record_invalid("appointment draft"));
    }
    let quote_id = QuoteId::from_uuid(quote_uuid);
    let kind = parse_appointment_kind(&row.kind)?;
    let starts_at = parse_offset_datetime(&row.starts_at)?;
    let ends_at = parse_offset_datetime(&row.ends_at)?;
    let requester_included = match row.requester_included {
        0 => false,
        1 => true,
        _ => return Err(stored_record_invalid("appointment draft")),
    };
    // This is the explicit storage-adapter trust boundary: a database string
    // becomes a caller email only after the domain confirmation constructor
    // accepts it.
    let email = ConfirmedEmail::confirm(row.caller_email)
        .map_err(|_| stored_record_invalid("appointment draft"))?;
    let caller = CallerIdentity::new(row.caller_name, email)
        .map_err(|_| stored_record_invalid("appointment draft"))?;
    let draft = AppointmentDraft::new_with_requester_inclusion(
        kind,
        caller,
        starts_at,
        quote_id,
        idempotency_key,
        requester_included,
    )
    .map_err(|_| stored_record_invalid("appointment draft"))?;
    if draft.ends_at() != ends_at {
        return Err(stored_record_invalid("appointment draft"));
    }
    Ok(StoredAppointmentDraft {
        id: row.id,
        source_id,
        draft,
    })
}

fn decode_appointment_quote_by_id(
    connection: &Connection,
    quote_id: QuoteId,
) -> StoreResult<StoredAppointmentQuote> {
    let row = connection
        .query_row(
            "SELECT quote_id, appointment_kind, timezone, issued_at, expires_at, slot_count, state,
                    appointment_draft_id, selected_slot_index, consumed_at, proposal_id
             FROM appointment_quotes WHERE quote_id = ?1",
            params![quote_id.to_string()],
            read_appointment_quote_row,
        )
        .optional()
        .map_err(|_| stored_record_invalid("appointment quote"))?;
    let row = match row {
        Some(row) => row,
        None => {
            let case_variant_matches = connection
                .prepare(
                    "SELECT quote_id FROM appointment_quotes
                     WHERE quote_id = ?1 COLLATE NOCASE LIMIT 2",
                )
                .map_err(|_| stored_record_invalid("appointment quote"))?
                .query_map(params![quote_id.to_string()], |row| row.get::<_, String>(0))
                .map_err(|_| stored_record_invalid("appointment quote"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| stored_record_invalid("appointment quote"))?;
            if case_variant_matches.is_empty() {
                return Err(StoreError::NotFound {
                    resource: "appointment quote",
                });
            }
            return Err(stored_record_invalid("appointment quote"));
        }
    };
    decode_appointment_quote_row(connection, row)
}

fn decode_appointment_quote_row(
    connection: &Connection,
    row: AppointmentQuoteStoredRow,
) -> StoreResult<StoredAppointmentQuote> {
    let quote_id = parse_quote_id(&row.quote_id)?;
    let appointment_kind = parse_appointment_quote_kind(&row.appointment_kind)?;
    let timezone = validate_appointment_quote_timezone(&row.timezone)
        .map_err(|_| stored_record_invalid("appointment quote"))?;
    let issued_at = parse_appointment_quote_datetime(&row.issued_at)?;
    let expires_at = parse_appointment_quote_datetime(&row.expires_at)?;
    let expected_expires_at = issued_at
        .checked_add(Quote::VALID_FOR)
        .ok_or_else(|| stored_record_invalid("appointment quote"))?;
    if expected_expires_at != expires_at {
        return Err(stored_record_invalid("appointment quote"));
    }
    let quote = Quote::with_id(quote_id, issued_at);

    let slots = connection
        .prepare(
            "SELECT slot_index, starts_at, ends_at
             FROM appointment_quote_slots
             WHERE quote_id = ?1 ORDER BY slot_index ASC LIMIT ?2",
        )
        .map_err(|_| stored_record_invalid("appointment quote"))?
        .query_map(
            params![
                &row.quote_id,
                i64::try_from(MAX_APPOINTMENT_QUOTE_SLOTS + 1)
                    .expect("slot limit fits SQLite integer"),
            ],
            read_appointment_quote_slot_row,
        )
        .map_err(|_| stored_record_invalid("appointment quote"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| stored_record_invalid("appointment quote"))?;
    let slot_count = usize::try_from(row.slot_count)
        .ok()
        .filter(|count| (1..=MAX_APPOINTMENT_QUOTE_SLOTS).contains(count))
        .ok_or_else(|| stored_record_invalid("appointment quote"))?;
    if slots.len() != slot_count {
        return Err(stored_record_invalid("appointment quote"));
    }
    let mut intervals = BTreeSet::new();
    let mut offered_slots = Vec::with_capacity(slots.len());
    for (expected_index, slot) in slots.into_iter().enumerate() {
        if slot.slot_index != i64::try_from(expected_index).expect("slot index is bounded") {
            return Err(stored_record_invalid("appointment quote"));
        }
        let starts_at = parse_appointment_quote_datetime(&slot.starts_at)?;
        let ends_at = parse_appointment_quote_datetime(&slot.ends_at)?;
        let appointment_slot = AppointmentSlot::new(starts_at, ends_at)
            .map_err(|_| stored_record_invalid("appointment quote"))?;
        if appointment_slot.duration() != appointment_kind.duration()
            || !intervals.insert((starts_at, ends_at))
        {
            return Err(stored_record_invalid("appointment quote"));
        }
        offered_slots.push(appointment_slot);
    }

    let state = parse_appointment_quote_state(&row.state)?;
    let (selected_slot_index, appointment_draft, consumed_at, proposal_id) = match state {
        StoredAppointmentQuoteState::Issued => {
            if row.appointment_draft_id.is_some()
                || row.selected_slot_index.is_some()
                || row.consumed_at.is_some()
                || row.proposal_id.is_some()
            {
                return Err(stored_record_invalid("appointment quote"));
            }
            (None, None, None, None)
        }
        StoredAppointmentQuoteState::Prepared | StoredAppointmentQuoteState::Consumed => {
            let draft_id = row
                .appointment_draft_id
                .filter(|id| *id > 0)
                .ok_or_else(|| stored_record_invalid("appointment quote"))?;
            let selected_slot_index = row
                .selected_slot_index
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < offered_slots.len())
                .ok_or_else(|| stored_record_invalid("appointment quote"))?;
            let draft = connection
                .query_row(
                    APPOINTMENT_DRAFT_SELECT,
                    params![draft_id],
                    read_appointment_row,
                )
                .optional()
                .map_err(|_| stored_record_invalid("appointment quote"))?
                .ok_or_else(|| stored_record_invalid("appointment quote"))?;
            let draft = decode_appointment_row(draft)
                .map_err(|_| stored_record_invalid("appointment quote"))?;
            let selected_slot = offered_slots[selected_slot_index];
            if draft.draft().quote_id() != quote_id
                || draft.draft().kind() != appointment_kind
                || draft.draft().starts_at() != selected_slot.starts_at()
                || draft.draft().ends_at() != selected_slot.ends_at()
            {
                return Err(stored_record_invalid("appointment quote"));
            }
            let selected_slot_index = u32::try_from(selected_slot_index)
                .map_err(|_| stored_record_invalid("appointment quote"))?;
            match state {
                StoredAppointmentQuoteState::Prepared => {
                    if row.consumed_at.is_some() || row.proposal_id.is_some() {
                        return Err(stored_record_invalid("appointment quote"));
                    }
                    (Some(selected_slot_index), Some(draft), None, None)
                }
                StoredAppointmentQuoteState::Consumed => {
                    let consumed_at = row
                        .consumed_at
                        .as_deref()
                        .ok_or_else(|| stored_record_invalid("appointment quote"))
                        .and_then(parse_appointment_quote_datetime)?;
                    let proposal_id = row
                        .proposal_id
                        .filter(|id| *id > 0)
                        .ok_or_else(|| stored_record_invalid("appointment quote"))?;
                    let proposal = connection
                        .query_row(PROPOSAL_SELECT, params![proposal_id], read_proposal_row)
                        .optional()
                        .map_err(|_| stored_record_invalid("appointment quote"))?
                        .ok_or_else(|| stored_record_invalid("appointment quote"))?;
                    let proposal = decode_proposal_row(proposal)
                        .map_err(|_| stored_record_invalid("appointment quote"))?;
                    if proposal.source().appointment_draft_id() != Some(draft_id) {
                        return Err(stored_record_invalid("appointment quote"));
                    }
                    (
                        Some(selected_slot_index),
                        Some(draft),
                        Some(consumed_at),
                        Some(proposal_id),
                    )
                }
                StoredAppointmentQuoteState::Issued => unreachable!(),
            }
        }
    };
    Ok(StoredAppointmentQuote {
        quote,
        appointment_kind,
        timezone,
        offered_slots,
        state,
        selected_slot_index,
        appointment_draft,
        consumed_at,
        proposal_id,
    })
}

fn validate_appointment_quote_timezone(value: &str) -> StoreResult<String> {
    if value.trim().is_empty() {
        return Err(StoreError::InvalidInput { field: "timezone" });
    }
    let timezone: Tz = value
        .parse()
        .map_err(|_| StoreError::InvalidInput { field: "timezone" })?;
    if timezone.name() != value {
        return Err(StoreError::InvalidInput { field: "timezone" });
    }
    Ok(timezone.name().to_owned())
}

fn validate_appointment_quote_slots(
    appointment_kind: AppointmentKind,
    offered_slots: &[AppointmentSlot],
) -> StoreResult<()> {
    if offered_slots.is_empty() || offered_slots.len() > MAX_APPOINTMENT_QUOTE_SLOTS {
        return Err(StoreError::InvalidInput {
            field: "offered_slots",
        });
    }
    let mut intervals = BTreeSet::new();
    for slot in offered_slots {
        if slot.duration() != appointment_kind.duration()
            || !intervals.insert((slot.starts_at(), slot.ends_at()))
        {
            return Err(StoreError::InvalidInput {
                field: "offered_slots",
            });
        }
    }
    Ok(())
}

fn parse_quote_id(value: &str) -> StoreResult<QuoteId> {
    let parsed = Uuid::parse_str(value).map_err(|_| stored_record_invalid("appointment quote"))?;
    if parsed.to_string() != value {
        return Err(stored_record_invalid("appointment quote"));
    }
    Ok(QuoteId::from_uuid(parsed))
}

fn parse_appointment_quote_kind(value: &str) -> StoreResult<AppointmentKind> {
    match value {
        "callback" => Ok(AppointmentKind::Callback),
        "meeting" => Ok(AppointmentKind::Meeting),
        _ => Err(stored_record_invalid("appointment quote")),
    }
}

fn parse_appointment_quote_state(value: &str) -> StoreResult<StoredAppointmentQuoteState> {
    match value {
        "issued" => Ok(StoredAppointmentQuoteState::Issued),
        "prepared" => Ok(StoredAppointmentQuoteState::Prepared),
        "consumed" => Ok(StoredAppointmentQuoteState::Consumed),
        _ => Err(stored_record_invalid("appointment quote")),
    }
}

fn format_appointment_quote_datetime(value: OffsetDateTime) -> StoreResult<String> {
    value
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput {
            field: "appointment_quote_time",
        })
}

fn parse_appointment_quote_datetime(value: &str) -> StoreResult<OffsetDateTime> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| stored_record_invalid("appointment quote"))?
        .to_offset(time::UtcOffset::UTC);
    let canonical = parsed
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid("appointment quote"))?;
    if value != canonical {
        return Err(stored_record_invalid("appointment quote"));
    }
    Ok(parsed)
}

fn decode_owner_task_row(row: OwnerTaskStoredRow) -> StoreResult<StoredOwnerTaskDraft> {
    if row.id <= 0 {
        return Err(stored_record_invalid("owner task draft"));
    }
    let source_id = row
        .source_id
        .map(validate_source_id)
        .transpose()
        .map_err(|_| stored_record_invalid("owner task draft"))?;
    let kind = parse_task_kind(&row.kind)?;
    let duration_minutes = u32::try_from(row.duration_minutes)
        .map_err(|_| stored_record_invalid("owner task draft"))?;
    let due_at = row
        .due_at
        .as_deref()
        .map(parse_offset_datetime)
        .transpose()?;
    let idempotency_key = IdempotencyKey::new(row.idempotency_key)
        .map_err(|_| stored_record_invalid("owner task draft"))?;
    let draft =
        OwnerTaskDraft::with_duration(kind, row.title, duration_minutes, due_at, idempotency_key)
            .map_err(|_| stored_record_invalid("owner task draft"))?;
    Ok(StoredOwnerTaskDraft {
        id: row.id,
        source_id,
        draft,
    })
}

fn decode_owner_task_placement(
    row: OwnerTaskPlacementStoredRow,
) -> StoreResult<StoredOwnerTaskPlacement> {
    if row.owner_task_draft_id <= 0
        || validate_owner_task_timezone(&row.timezone).is_err()
        || validate_machine_identifier(row.operation_key.clone(), "operation_key").is_err()
        || validate_machine_identifier(row.owner_fingerprint.clone(), "owner_fingerprint").is_err()
        || row
            .provider_event_id
            .as_deref()
            .is_some_and(|value| validate_provider_event_id(value.to_owned()).is_err())
    {
        return Err(stored_record_invalid("owner task placement"));
    }
    let starts_at = parse_owner_task_placement_datetime(&row.starts_at)?;
    let ends_at = parse_owner_task_placement_datetime(&row.ends_at)?;
    if starts_at >= ends_at {
        return Err(stored_record_invalid("owner task placement"));
    }
    if !matches!(
        (row.state.as_str(), row.provider_event_id.is_some()),
        ("prepared", false) | ("submitted", true)
    ) {
        return Err(stored_record_invalid("owner task placement"));
    }
    Ok(StoredOwnerTaskPlacement {
        owner_task_draft_id: row.owner_task_draft_id,
        starts_at,
        ends_at,
        timezone: row.timezone,
        operation_key: row.operation_key,
        owner_fingerprint: row.owner_fingerprint,
        provider_event_id: row.provider_event_id,
    })
}

fn validate_owner_task_timezone(value: &str) -> StoreResult<String> {
    validate_appointment_quote_timezone(value)
        .map_err(|_| StoreError::InvalidInput { field: "timezone" })
}

fn format_owner_task_placement_datetime(value: OffsetDateTime) -> StoreResult<String> {
    let value = value.to_offset(time::UtcOffset::UTC);
    if value.nanosecond() != 0 {
        return Err(StoreError::InvalidInput {
            field: "owner_task_placement_time",
        });
    }
    value
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput {
            field: "owner_task_placement_time",
        })
}

fn parse_owner_task_placement_datetime(value: &str) -> StoreResult<OffsetDateTime> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| stored_record_invalid("owner task placement"))?
        .to_offset(time::UtcOffset::UTC);
    let canonical = format_owner_task_placement_datetime(parsed)
        .map_err(|_| stored_record_invalid("owner task placement"))?;
    if canonical != value {
        return Err(stored_record_invalid("owner task placement"));
    }
    Ok(parsed)
}

fn ensure_owner_task_draft_exists(
    connection: &Connection,
    owner_task_draft_id: i64,
) -> StoreResult<()> {
    let row = connection
        .query_row(
            OWNER_TASK_DRAFT_SELECT,
            params![owner_task_draft_id],
            read_owner_task_row,
        )
        .optional()?;
    match row {
        Some(row) => {
            decode_owner_task_row(row)?;
            Ok(())
        }
        None => Err(StoreError::NotFound {
            resource: "owner task draft",
        }),
    }
}

fn validate_owner_task_placement_reference(
    connection: &Connection,
    placement: &StoredOwnerTaskPlacement,
) -> StoreResult<()> {
    ensure_owner_task_draft_exists(connection, placement.owner_task_draft_id)
        .map_err(|_| stored_record_invalid("owner task placement"))
}

fn parse_appointment_kind(value: &str) -> StoreResult<AppointmentKind> {
    match value {
        "callback" => Ok(AppointmentKind::Callback),
        "meeting" => Ok(AppointmentKind::Meeting),
        _ => Err(stored_record_invalid("appointment draft")),
    }
}

fn parse_task_kind(value: &str) -> StoreResult<TaskKind> {
    match value {
        "bill" => Ok(TaskKind::Bill),
        "callback" => Ok(TaskKind::Callback),
        "reading" => Ok(TaskKind::Reading),
        "email_reply" => Ok(TaskKind::EmailReply),
        "preparation" => Ok(TaskKind::Preparation),
        _ => Err(stored_record_invalid("owner task draft")),
    }
}

fn appointment_kind_storage_name(kind: AppointmentKind) -> &'static str {
    match kind {
        AppointmentKind::Callback => "callback",
        AppointmentKind::Meeting => "meeting",
    }
}

fn task_kind_storage_name(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Bill => "bill",
        TaskKind::Callback => "callback",
        TaskKind::Reading => "reading",
        TaskKind::EmailReply => "email_reply",
        TaskKind::Preparation => "preparation",
    }
}

fn format_offset_datetime(value: OffsetDateTime) -> StoreResult<String> {
    value
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid("draft"))
}

fn format_audit_timestamp(
    value: OffsetDateTime,
    field: &'static str,
) -> StoreResult<(OffsetDateTime, String)> {
    let value = value.to_offset(time::UtcOffset::UTC);
    if value.nanosecond() != 0 {
        return Err(StoreError::InvalidInput { field });
    }
    let text = value
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput { field })?;
    Ok((value, text))
}

fn parse_audit_timestamp(value: String, resource: &'static str) -> StoreResult<OffsetDateTime> {
    let parsed =
        OffsetDateTime::parse(&value, &Rfc3339).map_err(|_| stored_record_invalid(resource))?;
    let canonical = OffsetDateTime::from_unix_timestamp(parsed.unix_timestamp())
        .map_err(|_| stored_record_invalid(resource))?
        .to_offset(time::UtcOffset::UTC);
    let canonical_text = canonical
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid(resource))?;
    if value != canonical_text {
        return Err(stored_record_invalid(resource));
    }
    Ok(canonical)
}

fn parse_offset_datetime(value: &str) -> StoreResult<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| stored_record_invalid("draft"))
}

/// Validates a message idempotency/source/provider identifier.
pub fn validate_message_id(value: impl Into<String>) -> StoreResult<String> {
    validate_message_identifier(value.into(), "message_id")
}

/// Validates a message idempotency key for the persisted message contract.
pub fn validate_message_idempotency_key(value: impl Into<String>) -> StoreResult<String> {
    validate_message_identifier(value.into(), "idempotency_key")
}

/// Validates a message source identifier for the persisted message contract.
pub fn validate_message_source_id(value: impl Into<String>) -> StoreResult<String> {
    validate_message_identifier(value.into(), "source_id")
}

/// Validates a provider message identifier for the persisted message contract.
pub fn validate_provider_message_id(value: impl Into<String>) -> StoreResult<String> {
    validate_message_identifier(value.into(), "provider_message_id")
}

/// Validates a non-blank, control-free, byte-bounded structured summary.
pub fn validate_message_summary(value: impl Into<String>) -> StoreResult<String> {
    validate_message_text(value.into(), "summary", MAX_MESSAGE_SUMMARY_LENGTH)
}

/// Validates an optional extracted subject.
pub fn validate_message_subject(value: Option<String>) -> StoreResult<Option<String>> {
    value
        .map(|value| validate_message_text(value, "subject", MAX_MESSAGE_SUBJECT_LENGTH))
        .transpose()
}

/// Validates an optional extracted sender.
pub fn validate_message_sender(value: Option<String>) -> StoreResult<Option<String>> {
    value
        .map(|value| validate_message_text(value, "sender", MAX_MESSAGE_SENDER_LENGTH))
        .transpose()
}

/// Validates a task idempotency key for the persisted task contract.
pub fn validate_task_idempotency_key(value: impl Into<String>) -> StoreResult<String> {
    validate_task_identifier(value.into(), "idempotency_key")
}

/// Validates a task source identifier for the persisted task contract.
pub fn validate_task_source_id(value: impl Into<String>) -> StoreResult<String> {
    validate_task_identifier(value.into(), "source_id")
}

/// Validates a non-blank, control-free, byte-bounded task title.
pub fn validate_task_title(value: impl Into<String>) -> StoreResult<String> {
    let value = value.into();
    if value.trim().is_empty()
        || value.len() > MAX_TASK_TITLE_LENGTH
        || value.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidInput { field: "title" });
    }
    Ok(value)
}

fn validate_task_duration(minutes: u32) -> StoreResult<DurationMinutes> {
    if minutes == 0 || i64::from(minutes) > MAX_TASK_DURATION_MINUTES {
        return Err(StoreError::InvalidInput {
            field: "duration_minutes",
        });
    }
    DurationMinutes::new(minutes).map_err(|_| StoreError::InvalidInput {
        field: "duration_minutes",
    })
}

fn validate_task_duration_i64(minutes: i64) -> StoreResult<DurationMinutes> {
    let minutes = u32::try_from(minutes).map_err(|_| StoreError::InvalidInput {
        field: "duration_minutes",
    })?;
    validate_task_duration(minutes)
}

fn format_task_timestamp(
    value: OffsetDateTime,
    field: &'static str,
) -> StoreResult<(OffsetDateTime, String)> {
    let value = value.to_offset(time::UtcOffset::UTC);
    if value.nanosecond() != 0 {
        return Err(StoreError::InvalidInput { field });
    }
    let text = value
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput { field })?;
    Ok((value, text))
}

fn format_task_updated_timestamp(value: OffsetDateTime) -> StoreResult<(OffsetDateTime, String)> {
    if value.offset() != time::UtcOffset::UTC || value.nanosecond() != 0 {
        return Err(StoreError::InvalidInput {
            field: "updated_at",
        });
    }
    let text = value
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput {
            field: "updated_at",
        })?;
    Ok((value, text))
}

fn parse_task_timestamp(value: String, resource: &'static str) -> StoreResult<OffsetDateTime> {
    let parsed = OffsetDateTime::parse(&value, &Rfc3339)
        .map_err(|_| stored_record_invalid(resource))?
        .to_offset(time::UtcOffset::UTC);
    if parsed.nanosecond() != 0 {
        return Err(stored_record_invalid(resource));
    }
    let canonical = parsed
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid(resource))?;
    if canonical != value {
        return Err(stored_record_invalid(resource));
    }
    Ok(parsed)
}

fn parse_task_timestamp_text(value: String) -> StoreResult<String> {
    let parsed = parse_task_timestamp(value, "task")?;
    parsed
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid("task"))
}

/// Validates a canonical UTC message timestamp as stored in SQLite.
pub fn validate_message_timestamp(value: impl Into<String>) -> StoreResult<String> {
    let value = value.into();
    let parsed = OffsetDateTime::parse(&value, &Rfc3339)
        .map_err(|_| StoreError::InvalidInput { field: "timestamp" })?;
    let canonical = parsed
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput { field: "timestamp" })?;
    if canonical != value || parsed.nanosecond() != 0 {
        return Err(StoreError::InvalidInput { field: "timestamp" });
    }
    Ok(value)
}

fn initial_message_state(provider: MessageProvider) -> MessageTriageState {
    match provider {
        MessageProvider::Voice => MessageTriageState::Recorded,
        MessageProvider::Outlook | MessageProvider::Gmail => MessageTriageState::Unprocessed,
    }
}

fn valid_message_state(provider: MessageProvider, state: MessageTriageState) -> bool {
    match provider {
        MessageProvider::Voice => state == MessageTriageState::Recorded,
        MessageProvider::Outlook | MessageProvider::Gmail => {
            matches!(
                state,
                MessageTriageState::Unprocessed
                    | MessageTriageState::Actionable
                    | MessageTriageState::Ambiguous
                    | MessageTriageState::Ignored
                    | MessageTriageState::Scheduled
            )
        }
    }
}

fn valid_message_transition(
    expected_state: MessageTriageState,
    next_state: MessageTriageState,
) -> bool {
    matches!(
        (expected_state, next_state),
        (
            MessageTriageState::Unprocessed,
            MessageTriageState::Actionable
                | MessageTriageState::Ambiguous
                | MessageTriageState::Ignored
        ) | (
            MessageTriageState::Actionable,
            MessageTriageState::Scheduled
        )
    )
}

fn valid_task_transition(expected_state: StoredTaskState, next_state: StoredTaskState) -> bool {
    matches!(
        (expected_state, next_state),
        (
            StoredTaskState::Pending,
            StoredTaskState::Proposed | StoredTaskState::NoSlot
        ) | (
            StoredTaskState::Proposed,
            StoredTaskState::Scheduled | StoredTaskState::NoSlot
        )
    )
}

fn format_message_timestamp(value: OffsetDateTime) -> StoreResult<(OffsetDateTime, String)> {
    if value.offset() != time::UtcOffset::UTC {
        return Err(StoreError::InvalidInput {
            field: "received_at",
        });
    }
    format_audit_timestamp(value, "received_at")
}

fn format_message_updated_timestamp(
    value: OffsetDateTime,
) -> StoreResult<(OffsetDateTime, String)> {
    if value.offset() != time::UtcOffset::UTC {
        return Err(StoreError::InvalidInput {
            field: "updated_at",
        });
    }
    if value.nanosecond() != 0 {
        return Err(StoreError::InvalidInput {
            field: "updated_at",
        });
    }
    let text = value
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput {
            field: "updated_at",
        })?;
    Ok((value, text))
}

fn parse_message_timestamp(value: String) -> StoreResult<OffsetDateTime> {
    let parsed =
        OffsetDateTime::parse(&value, &Rfc3339).map_err(|_| stored_record_invalid("message"))?;
    if parsed.nanosecond() != 0 {
        return Err(stored_record_invalid("message"));
    }
    let canonical = parsed.to_offset(time::UtcOffset::UTC);
    let canonical_text = canonical
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid("message"))?;
    if value != canonical_text {
        return Err(stored_record_invalid("message"));
    }
    Ok(canonical)
}

fn parse_message_timestamp_text(value: String) -> StoreResult<String> {
    let parsed = parse_message_timestamp(value)?;
    parsed
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid("message"))
}

fn validate_message_identifier(value: String, field: &'static str) -> StoreResult<String> {
    validate_machine_identifier_with_limit(value, field, MAX_MESSAGE_ID_LENGTH)
}

fn validate_provider_cursor_identifier(value: String, field: &'static str) -> StoreResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_PROVIDER_CURSOR_LENGTH
        || !value.is_ascii()
        || value.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(value)
}

fn validate_provider_stream_identifier(value: String, field: &'static str) -> StoreResult<String> {
    validate_machine_identifier_with_limit(value, field, MAX_PROVIDER_CURSOR_LENGTH)
}

fn validate_machine_identifier_with_limit(
    value: String,
    field: &'static str,
    maximum_length: usize,
) -> StoreResult<String> {
    if value.is_empty()
        || value.len() > maximum_length
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(value)
}

fn load_provider_cursor_row(
    transaction: &Transaction<'_>,
    stream_id: &str,
) -> StoreResult<Option<Option<String>>> {
    transaction
        .query_row(
            "SELECT cursor FROM provider_cursors WHERE provider = ?1",
            params![stream_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(provider_cursor_sqlite_error)
}

fn provider_cursor_sqlite_error(_: rusqlite::Error) -> StoreError {
    StoreError::Sqlite(rusqlite::Error::InvalidQuery)
}

fn validate_task_identifier(value: String, field: &'static str) -> StoreResult<String> {
    if value.is_empty()
        || value.len() > MAX_TASK_ID_LENGTH
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(value)
}

fn validate_machine_identifier(value: String, field: &'static str) -> StoreResult<String> {
    if value.is_empty()
        || value.len() > MAX_TASK_ID_LENGTH
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(value)
}

fn validate_provider_event_id(value: String) -> StoreResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_TASK_ID_LENGTH
        || !value.is_ascii()
        || value.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidInput {
            field: "provider_event_id",
        });
    }
    Ok(value)
}

fn validate_message_text(
    value: String,
    field: &'static str,
    maximum_length: usize,
) -> StoreResult<String> {
    if value.trim().is_empty()
        || value.len() > maximum_length
        || value.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(value)
}

fn validate_source_id(value: String) -> StoreResult<String> {
    let value = validate_non_empty(value, "source_id")?;
    if value.chars().any(char::is_control) {
        return Err(StoreError::InvalidInput { field: "source_id" });
    }
    Ok(value)
}

fn stored_record_invalid(resource: &'static str) -> StoreError {
    StoreError::StoredRecordInvalid { resource }
}

fn validate_non_empty(value: String, field: &'static str) -> StoreResult<String> {
    if value.trim().is_empty() {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(value)
}

fn validate_replay_nonce(nonce: &str) -> StoreResult<()> {
    if !(16..=128).contains(&nonce.len())
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(StoreError::InvalidInput { field: "nonce" });
    }
    Ok(())
}

fn replay_timestamps(now: i64) -> StoreResult<(String, String)> {
    let consumed_at = OffsetDateTime::from_unix_timestamp(now)
        .map_err(|_| StoreError::InvalidInput { field: "now" })?
        .to_offset(time::UtcOffset::UTC);
    let expires_at = consumed_at
        .checked_add(TimeDuration::seconds(REPLAY_RETENTION_SECONDS))
        .ok_or(StoreError::InvalidInput { field: "now" })?;
    let consumed_at = consumed_at
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput { field: "now" })?;
    let expires_at = expires_at
        .format(&Rfc3339)
        .map_err(|_| StoreError::InvalidInput { field: "now" })?;
    Ok((consumed_at, expires_at))
}

fn validate_oauth_identity(value: String, field: &'static str) -> StoreResult<String> {
    let value = validate_non_empty(value, field)?;
    if value.contains(':') {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(value)
}

fn normalize_scopes(scopes: Vec<String>) -> StoreResult<Vec<String>> {
    let mut normalized = BTreeSet::new();
    for scope in scopes {
        let scope = scope.trim().to_owned();
        if scope.is_empty() {
            return Err(StoreError::InvalidInput { field: "scopes" });
        }
        normalized.insert(scope);
    }
    if normalized.is_empty() {
        return Err(StoreError::InvalidInput { field: "scopes" });
    }
    Ok(normalized.into_iter().collect())
}

fn oauth_context(provider: &str, account_id: &str, token_kind: &str) -> String {
    format!("oauth:{provider}:{account_id}:{token_kind}")
}

fn serialize_envelope(envelope: &EncryptedSecret) -> StoreResult<Vec<u8>> {
    serde_json::to_vec(envelope).map_err(|_| StoreError::StoredValueInvalid)
}

fn deserialize_envelope(bytes: &[u8]) -> StoreResult<EncryptedSecret> {
    serde_json::from_slice(bytes).map_err(|_| StoreError::StoredValueInvalid)
}

fn reject_empty_key(key: &[u8]) -> StoreResult<()> {
    if key.is_empty() {
        return Err(StoreError::EmptyDatabaseKey);
    }
    Ok(())
}

fn rollback_journal_path(path: &Path) -> std::path::PathBuf {
    let mut journal_path = path.as_os_str().to_os_string();
    journal_path.push("-journal");
    journal_path.into()
}

fn rollback_journal_is_hot(path: &Path) -> bool {
    const JOURNAL_MAGIC: &[u8; 8] = b"\xd9\xd5\x05\xf9\x20\xa1\x63\xd7";

    std::fs::read(path).is_ok_and(|bytes| bytes.len() > 512 && bytes.starts_with(JOURNAL_MAGIC))
}

fn immutable_read_only_uri(path: &Path) -> StoreResult<String> {
    #[cfg(unix)]
    let path_bytes = {
        use std::os::unix::ffi::OsStrExt as _;

        path.as_os_str().as_bytes()
    };
    #[cfg(windows)]
    let normalized_path = path
        .to_str()
        .ok_or(StoreError::NotFound {
            resource: "database",
        })?
        .replace('\\', "/");
    #[cfg(windows)]
    let path_bytes = normalized_path.as_bytes();
    #[cfg(not(any(unix, windows)))]
    let path_text = path.to_str().ok_or(StoreError::NotFound {
        resource: "database",
    })?;
    #[cfg(not(any(unix, windows)))]
    let path_bytes = path_text.as_bytes();

    #[cfg(windows)]
    let is_windows_path = true;
    #[cfg(not(windows))]
    let is_windows_path = false;

    Ok(immutable_read_only_uri_for_path_bytes(
        path_bytes,
        is_windows_path,
        path.is_absolute(),
    ))
}

fn immutable_read_only_uri_for_path_bytes(
    path_bytes: &[u8],
    is_windows_path: bool,
    is_absolute: bool,
) -> String {
    let mut uri = String::from("file:");
    if is_windows_path && is_absolute {
        uri.push_str("///");
    }
    append_uri_path_bytes(&mut uri, path_bytes);
    uri.push_str("?immutable=1");
    uri
}

fn append_uri_path_bytes(uri: &mut String, path_bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    for &byte in path_bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'.' | b'_' | b'~') {
            uri.push(char::from(byte));
        } else {
            uri.push('%');
            uri.push(char::from(HEX[(byte >> 4) as usize]));
            uri.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
}

fn initialize(
    connection: &mut Connection,
    key: &[u8],
    file_store: bool,
    migrate: bool,
) -> StoreResult<()> {
    apply_sqlcipher_key(connection, key)?;
    verify_sqlcipher(connection)?;
    connection.pragma_update(None, "recursive_triggers", true)?;

    connection.pragma_update(None, "foreign_keys", true)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    if file_store {
        let journal_mode =
            connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| {
                row.get::<_, String>(0)
            })?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Sqlite(rusqlite::Error::InvalidQuery));
        }
    }

    if migrate {
        run_migrations(connection)?;
    }
    Ok(())
}

fn apply_sqlcipher_key(connection: &Connection, key: &[u8]) -> StoreResult<()> {
    let encoded_key = hex_encode(key);
    // The value contains only hexadecimal ASCII, so it cannot alter the
    // pragma statement even when callers supply arbitrary key bytes.
    connection.pragma_update(None, "key", format!("x'{encoded_key}'"))?;
    Ok(())
}

fn verify_sqlcipher(connection: &Connection) -> StoreResult<()> {
    let cipher_version = connection
        .pragma_query_value(None, "cipher_version", |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(StoreError::Sqlite)?;
    if cipher_version.is_none_or(|version| version.trim().is_empty()) {
        return Err(StoreError::SqlCipherUnavailable);
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn run_migrations(connection: &mut Connection) -> StoreResult<()> {
    run_migrations_with(connection, MIGRATIONS)
}

fn run_migrations_with(connection: &mut Connection, migrations: &[Migration]) -> StoreResult<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
            version INTEGER PRIMARY KEY,\
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP\
        )",
    )?;
    let current_version: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    let latest_version = migrations
        .last()
        .map(|migration| migration.version)
        .unwrap_or_default();
    if current_version > latest_version {
        return Err(StoreError::UnsupportedSchemaVersion(current_version));
    }
    for migration in migrations {
        if migration.version > current_version {
            (migration.apply)(&transaction)?;
            transaction.execute(
                "INSERT INTO schema_migrations(version) VALUES (?1)",
                [migration.version],
            )?;
        }
    }
    transaction.commit()?;
    Ok(())
}

fn apply_schema_v1(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS configuration (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    owner_timezone TEXT,
    owner_email TEXT,
    owner_phone TEXT,
    working_days TEXT NOT NULL DEFAULT 'monday,tuesday,wednesday,thursday,friday',
    working_window_start TEXT NOT NULL DEFAULT '08:00',
    working_window_end TEXT NOT NULL DEFAULT '18:00',
    minimum_notice_minutes INTEGER NOT NULL DEFAULT 60 CHECK (minimum_notice_minutes >= 0),
    booking_horizon_days INTEGER NOT NULL DEFAULT 60 CHECK (booking_horizon_days > 0),
    meeting_buffer_minutes INTEGER NOT NULL DEFAULT 0 CHECK (meeting_buffer_minutes >= 0),
    retention_days INTEGER NOT NULL DEFAULT 90 CHECK (retention_days > 0),
    task_duration_bill_minutes INTEGER NOT NULL DEFAULT 15 CHECK (task_duration_bill_minutes > 0),
    task_duration_callback_minutes INTEGER NOT NULL DEFAULT 15 CHECK (task_duration_callback_minutes > 0),
    task_duration_reading_minutes INTEGER NOT NULL DEFAULT 30 CHECK (task_duration_reading_minutes > 0),
    task_duration_email_reply_minutes INTEGER NOT NULL DEFAULT 30 CHECK (task_duration_email_reply_minutes > 0),
    task_duration_preparation_minutes INTEGER NOT NULL DEFAULT 60 CHECK (task_duration_preparation_minutes > 0),
    email_triage_model TEXT NOT NULL DEFAULT 'gpt-5.6-luna',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS oauth_credentials (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    account_id TEXT NOT NULL,
    access_token_ciphertext BLOB,
    refresh_token_ciphertext BLOB,
    expires_at TEXT,
    scopes TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (provider, account_id)
);

CREATE TABLE IF NOT EXISTS provider_cursors (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL UNIQUE,
    cursor TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS appointment_drafts (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    source_id TEXT NOT NULL UNIQUE,
    quote_id TEXT NOT NULL UNIQUE,
    caller_name TEXT NOT NULL,
    caller_email TEXT NOT NULL,
    kind TEXT NOT NULL,
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    requester_included INTEGER NOT NULL CHECK (requester_included IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS owner_task_drafts (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    source_id TEXT UNIQUE,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,
    duration_minutes INTEGER NOT NULL CHECK (duration_minutes > 0),
    due_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    source_id TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL,
    provider_message_id TEXT NOT NULL,
    subject TEXT,
    sender TEXT,
    received_at TEXT,
    triage_state TEXT NOT NULL DEFAULT 'unprocessed',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (provider, provider_message_id)
);

CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    source_id TEXT NOT NULL UNIQUE,
    message_id INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,
    duration_minutes INTEGER NOT NULL CHECK (duration_minutes > 0),
    due_at TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS proposals (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    source_id TEXT NOT NULL UNIQUE,
    appointment_draft_id INTEGER REFERENCES appointment_drafts(id) ON DELETE SET NULL,
    owner_task_draft_id INTEGER REFERENCES owner_task_drafts(id) ON DELETE SET NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'accepted', 'declined', 'expired')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS event_mappings (
    id INTEGER PRIMARY KEY,
    proposal_id INTEGER NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    provider_event_id TEXT NOT NULL,
    source_id TEXT NOT NULL UNIQUE,
    starts_at TEXT,
    ends_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (provider, provider_event_id)
);

CREATE TABLE IF NOT EXISTS notification_outbox (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    proposal_id INTEGER REFERENCES proposals(id) ON DELETE CASCADE,
    event_mapping_id INTEGER REFERENCES event_mappings(id) ON DELETE CASCADE,
    notification_kind TEXT NOT NULL,
    recipient TEXT NOT NULL,
    payload TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    available_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    sent_at TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS replay_nonces (
    id INTEGER PRIMARY KEY,
    nonce TEXT NOT NULL UNIQUE,
    consumed_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS audit_events (
    id INTEGER PRIMARY KEY,
    event_type TEXT NOT NULL,
    entity_type TEXT,
    entity_id TEXT,
    details TEXT,
    occurred_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_appointment_drafts_starts_at
    ON appointment_drafts(starts_at);
CREATE INDEX IF NOT EXISTS idx_owner_task_drafts_due_at
    ON owner_task_drafts(due_at);
CREATE INDEX IF NOT EXISTS idx_messages_received_at
    ON messages(received_at);
CREATE INDEX IF NOT EXISTS idx_tasks_status_due_at
    ON tasks(status, due_at);
CREATE INDEX IF NOT EXISTS idx_proposals_state
    ON proposals(state);
CREATE INDEX IF NOT EXISTS idx_notification_outbox_delivery
    ON notification_outbox(status, available_at);
CREATE INDEX IF NOT EXISTS idx_replay_nonces_expires_at
    ON replay_nonces(expires_at);
CREATE INDEX IF NOT EXISTS idx_audit_events_occurred_at
    ON audit_events(occurred_at);

INSERT INTO configuration (
    id,
    owner_timezone,
    owner_email,
    owner_phone,
    working_days,
    working_window_start,
    working_window_end,
    minimum_notice_minutes,
    booking_horizon_days,
    meeting_buffer_minutes,
    retention_days,
    task_duration_bill_minutes,
    task_duration_callback_minutes,
    task_duration_reading_minutes,
    task_duration_email_reply_minutes,
    task_duration_preparation_minutes,
    email_triage_model
) VALUES (
    1,
    NULL,
    NULL,
    NULL,
    'monday,tuesday,wednesday,thursday,friday',
    '08:00',
    '18:00',
    60,
    60,
    0,
    90,
    15,
    15,
    30,
    30,
    60,
    'gpt-5.6-luna'
)
ON CONFLICT(id) DO NOTHING;
            "#,
    )?;
    Ok(())
}

fn apply_schema_v2(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_event_mappings_proposal_id \
         ON event_mappings(proposal_id)",
        [],
    )?;
    Ok(())
}

fn apply_schema_v3(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE proposals_rebuilt (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    source_id TEXT NOT NULL UNIQUE,
    appointment_draft_id INTEGER REFERENCES appointment_drafts(id) ON DELETE RESTRICT,
    owner_task_draft_id INTEGER REFERENCES owner_task_drafts(id) ON DELETE RESTRICT,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'accepted', 'declined', 'expired')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK ((appointment_draft_id IS NOT NULL) <> (owner_task_draft_id IS NOT NULL))
);

INSERT INTO proposals_rebuilt (
    id, idempotency_key, source_id, appointment_draft_id, owner_task_draft_id,
    state, created_at, updated_at
)
SELECT
    id, idempotency_key, source_id, appointment_draft_id, owner_task_draft_id,
    state, created_at, updated_at
FROM proposals;

CREATE TABLE event_mappings_rebuilt (
    id INTEGER PRIMARY KEY,
    proposal_id INTEGER NOT NULL REFERENCES proposals_rebuilt(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    provider_event_id TEXT NOT NULL,
    source_id TEXT NOT NULL UNIQUE,
    starts_at TEXT,
    ends_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (provider, provider_event_id)
);

INSERT INTO event_mappings_rebuilt (
    id, proposal_id, provider, provider_event_id, source_id, starts_at, ends_at,
    created_at, updated_at
)
SELECT
    id, proposal_id, provider, provider_event_id, source_id, starts_at, ends_at,
    created_at, updated_at
FROM event_mappings;

CREATE TABLE notification_outbox_rebuilt (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    proposal_id INTEGER REFERENCES proposals_rebuilt(id) ON DELETE CASCADE,
    event_mapping_id INTEGER REFERENCES event_mappings_rebuilt(id) ON DELETE CASCADE,
    notification_kind TEXT NOT NULL,
    recipient TEXT NOT NULL,
    payload TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    available_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    sent_at TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO notification_outbox_rebuilt (
    id, idempotency_key, proposal_id, event_mapping_id, notification_kind,
    recipient, payload, status, available_at, sent_at, attempts, created_at,
    updated_at
)
SELECT
    id, idempotency_key, proposal_id, event_mapping_id, notification_kind,
    recipient, payload, status, available_at, sent_at, attempts, created_at,
    updated_at
FROM notification_outbox;

DROP TABLE notification_outbox;
DROP TABLE event_mappings;
DROP TABLE proposals;

ALTER TABLE proposals_rebuilt RENAME TO proposals;
ALTER TABLE event_mappings_rebuilt RENAME TO event_mappings;
ALTER TABLE notification_outbox_rebuilt RENAME TO notification_outbox;

CREATE INDEX idx_proposals_state ON proposals(state);
CREATE UNIQUE INDEX idx_event_mappings_proposal_id ON event_mappings(proposal_id);
CREATE INDEX idx_notification_outbox_delivery ON notification_outbox(status, available_at);
        "#,
    )?;
    Ok(())
}

fn apply_schema_v4(transaction: &Transaction<'_>) -> StoreResult<()> {
    let legacy_rows = {
        let mut statement = transaction
            .prepare(
                "SELECT id, idempotency_key, proposal_id, event_mapping_id, notification_kind,
                    recipient, payload, status, available_at, sent_at, attempts, created_at,
                    updated_at
             FROM notification_outbox
             ORDER BY id ASC",
            )
            .map_err(|_| stored_record_invalid("notification"))?;
        statement
            .query_map([], |row| {
                Ok(LegacyNotificationOutboxRow {
                    id: row.get(0)?,
                    idempotency_key: row.get(1)?,
                    proposal_id: row.get(2)?,
                    event_mapping_id: row.get(3)?,
                    notification_kind: row.get(4)?,
                    recipient: row.get(5)?,
                    payload: row.get(6)?,
                    status: row.get(7)?,
                    available_at: row.get(8)?,
                    sent_at: row.get(9)?,
                    attempts: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            })
            .map_err(|_| stored_record_invalid("notification"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| stored_record_invalid("notification"))?
    };

    let normalized_rows = legacy_rows
        .into_iter()
        .map(|row| {
            let available_at = normalize_legacy_notification_timestamp(&row.available_at)?;
            let created_at = normalize_legacy_notification_timestamp(&row.created_at)?;
            let updated_at = normalize_legacy_notification_timestamp(&row.updated_at)?;
            let normalized_sent_at = row
                .sent_at
                .as_deref()
                .map(normalize_legacy_notification_timestamp)
                .transpose()?;
            let (status, sent_at) = match (row.status.as_str(), row.attempts, normalized_sent_at) {
                ("sent", attempts, Some(sent_at)) if attempts > 0 => {
                    (NotificationStatus::Sent.as_str(), Some(sent_at))
                }
                _ => (NotificationStatus::Pending.as_str(), None),
            };
            Ok((row, status, available_at, sent_at, created_at, updated_at))
        })
        .collect::<StoreResult<Vec<_>>>()?;

    transaction.execute_batch(
        r#"
CREATE TABLE notification_outbox_rebuilt (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    proposal_id INTEGER REFERENCES proposals(id) ON DELETE CASCADE,
    event_mapping_id INTEGER REFERENCES event_mappings(id) ON DELETE CASCADE,
    notification_kind TEXT NOT NULL,
    recipient TEXT NOT NULL,
    payload TEXT,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'delivering', 'sent')),
    available_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    lease_until TEXT,
    sent_at TEXT,
    last_error_code TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
        "#,
    )?;

    for (row, status, available_at, sent_at, created_at, updated_at) in normalized_rows {
        transaction.execute(
            "INSERT INTO notification_outbox_rebuilt (
                 id, idempotency_key, proposal_id, event_mapping_id, notification_kind,
                 recipient, payload, status, available_at, lease_until, sent_at,
                 last_error_code, attempts, created_at, updated_at
             ) VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, NULL, ?11, ?12, ?13
             )",
            params![
                row.id,
                row.idempotency_key,
                row.proposal_id,
                row.event_mapping_id,
                row.notification_kind,
                row.recipient,
                row.payload,
                status,
                available_at,
                sent_at,
                row.attempts,
                created_at,
                updated_at,
            ],
        )?;
    }

    transaction.execute_batch(
        "DROP TABLE notification_outbox;
         ALTER TABLE notification_outbox_rebuilt RENAME TO notification_outbox;
         CREATE INDEX idx_notification_outbox_delivery ON notification_outbox(status, available_at);",
    )?;
    Ok(())
}

fn apply_schema_v5(transaction: &Transaction<'_>) -> StoreResult<()> {
    let legacy_rows = {
        let mut statement = transaction
            .prepare(
                "SELECT id, event_type, entity_type, entity_id, details, occurred_at,
                        created_at
                 FROM audit_events
                 ORDER BY id ASC",
            )
            .map_err(|_| stored_record_invalid("audit event"))?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|_| stored_record_invalid("audit event"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| stored_record_invalid("audit event"))?
    };

    let normalized_rows = legacy_rows
        .into_iter()
        .map(
            |(id, event_type, entity_type, entity_id, details, occurred_at, created_at)| {
                if id <= 0 || details.is_some() {
                    return Err(stored_record_invalid("audit event"));
                }
                let event_type = AuditEventType::from_storage(&event_type)
                    .map_err(|_| stored_record_invalid("audit event"))?;
                let entity_type = entity_type
                    .as_deref()
                    .ok_or_else(|| stored_record_invalid("audit event"))
                    .and_then(AuditEntityType::from_storage)
                    .map_err(|_| stored_record_invalid("audit event"))?;
                let entity_id = entity_id
                    .ok_or_else(|| stored_record_invalid("audit event"))
                    .and_then(validate_audit_entity_id)
                    .map_err(|_| stored_record_invalid("audit event"))?;
                let occurred_at = normalize_legacy_audit_timestamp(&occurred_at)?;
                let created_at = normalize_legacy_audit_timestamp(&created_at)?;
                Ok((
                    id,
                    validate_audit_idempotency_key(format!("legacy-audit-{id}"))
                        .map_err(|_| stored_record_invalid("audit event"))?,
                    event_type.as_str(),
                    entity_type.as_str(),
                    entity_id,
                    occurred_at,
                    created_at,
                ))
            },
        )
        .collect::<StoreResult<Vec<_>>>()?;

    transaction.execute_batch(
        r#"
CREATE TABLE audit_events_rebuilt (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        typeof(idempotency_key) = 'text'
        AND length(CAST(idempotency_key AS BLOB)) BETWEEN 1 AND 256
        AND instr(CAST(idempotency_key AS BLOB), x'00') = 0
        AND idempotency_key NOT GLOB '*[^A-Za-z0-9._:-]*'
    ),
    event_type TEXT NOT NULL CHECK (event_type IN (
        'message_recorded', 'request_submitted', 'owner_task_submitted',
        'proposal_created', 'proposal_accepted', 'proposal_declined',
        'proposal_expired', 'proposal_promoted', 'notification_enqueued',
        'notification_sent', 'notification_retry_scheduled',
        'provider_cursor_advanced'
    )),
    entity_type TEXT NOT NULL CHECK (entity_type IN (
        'message', 'appointment_request', 'owner_task', 'proposal',
        'notification', 'provider_cursor'
    )),
    entity_id TEXT NOT NULL CHECK (
        typeof(entity_id) = 'text'
        AND length(CAST(entity_id AS BLOB)) BETWEEN 1 AND 256
        AND instr(CAST(entity_id AS BLOB), x'00') = 0
        AND entity_id NOT GLOB '*[^A-Za-z0-9._:-]*'
    ),
    details TEXT CHECK (details IS NULL),
    occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        strftime('%Y-%m-%dT%H:%M:%SZ', occurred_at) IS NOT NULL
        AND occurred_at = strftime('%Y-%m-%dT%H:%M:%SZ', occurred_at)
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        strftime('%Y-%m-%dT%H:%M:%SZ', created_at) IS NOT NULL
        AND created_at = strftime('%Y-%m-%dT%H:%M:%SZ', created_at)
    )
);
        "#,
    )?;

    for (id, idempotency_key, event_type, entity_type, entity_id, occurred_at, created_at) in
        normalized_rows
    {
        transaction.execute(
            "INSERT INTO audit_events_rebuilt (
                 id, idempotency_key, event_type, entity_type, entity_id, details,
                 occurred_at, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
            params![
                id,
                idempotency_key,
                event_type,
                entity_type,
                entity_id,
                occurred_at,
                created_at,
            ],
        )?;
    }

    transaction.execute_batch(
        "DROP TABLE audit_events;
         ALTER TABLE audit_events_rebuilt RENAME TO audit_events;
         CREATE INDEX idx_audit_events_occurred_at ON audit_events(occurred_at);",
    )?;
    Ok(())
}

fn apply_schema_v6(transaction: &Transaction<'_>) -> StoreResult<()> {
    let legacy_count: i64 =
        transaction.query_row("SELECT count(*) FROM messages", [], |row| row.get(0))?;
    if legacy_count != 0 {
        // The old table has no trustworthy summary or idempotency contract.
        // Refuse the rebuild before making any schema or data change.
        return Err(stored_record_invalid("message"));
    }

    transaction.execute_batch(
        r#"
CREATE TABLE messages_rebuilt (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        typeof(idempotency_key) = 'text'
        AND length(CAST(idempotency_key AS BLOB)) BETWEEN 1 AND 256
        AND instr(CAST(idempotency_key AS BLOB), x'00') = 0
        AND idempotency_key NOT GLOB '*[^A-Za-z0-9._:-]*'
    ),
    source_id TEXT NOT NULL UNIQUE CHECK (
        typeof(source_id) = 'text'
        AND length(CAST(source_id AS BLOB)) BETWEEN 1 AND 256
        AND instr(CAST(source_id AS BLOB), x'00') = 0
        AND source_id NOT GLOB '*[^A-Za-z0-9._:-]*'
    ),
    provider TEXT NOT NULL CHECK (
        typeof(provider) = 'text'
        AND provider IN ('voice', 'outlook', 'gmail')
    ),
    provider_message_id TEXT NOT NULL CHECK (
        typeof(provider_message_id) = 'text'
        AND length(CAST(provider_message_id AS BLOB)) BETWEEN 1 AND 256
        AND instr(CAST(provider_message_id AS BLOB), x'00') = 0
        AND provider_message_id NOT GLOB '*[^A-Za-z0-9._:-]*'
    ),
    summary TEXT NOT NULL CHECK (
        typeof(summary) = 'text'
        AND length(CAST(summary AS BLOB)) BETWEEN 1 AND 4096
        AND instr(CAST(summary AS BLOB), x'00') = 0
        AND length(trim(summary, char(
            9, 10, 11, 12, 13, 32, 133, 160, 5760,
            8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202,
            8232, 8233, 8239, 8287, 12288
        ))) > 0
        AND summary NOT GLOB '*[' || char(1) || '-' || char(31) || char(127) || '-' || char(159) || ']*'
    ),
    subject TEXT CHECK (
        subject IS NULL OR (
            typeof(subject) = 'text'
            AND length(CAST(subject AS BLOB)) BETWEEN 1 AND 256
            AND instr(CAST(subject AS BLOB), x'00') = 0
            AND length(trim(subject, char(
                9, 10, 11, 12, 13, 32, 133, 160, 5760,
                8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202,
                8232, 8233, 8239, 8287, 12288
            ))) > 0
            AND subject NOT GLOB '*[' || char(1) || '-' || char(31) || char(127) || '-' || char(159) || ']*'
        )
    ),
    sender TEXT CHECK (
        sender IS NULL OR (
            typeof(sender) = 'text'
            AND length(CAST(sender AS BLOB)) BETWEEN 1 AND 320
            AND instr(CAST(sender AS BLOB), x'00') = 0
            AND length(trim(sender, char(
                9, 10, 11, 12, 13, 32, 133, 160, 5760,
                8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202,
                8232, 8233, 8239, 8287, 12288
            ))) > 0
            AND sender NOT GLOB '*[' || char(1) || '-' || char(31) || char(127) || '-' || char(159) || ']*'
        )
    ),
    received_at TEXT NOT NULL CHECK (
        typeof(received_at) = 'text'
        AND length(CAST(received_at AS BLOB)) = 20
        AND instr(CAST(received_at AS BLOB), x'00') = 0
        AND received_at GLOB '????-??-??T??:??:??Z'
        AND received_at = strftime('%Y-%m-%dT%H:%M:%SZ', received_at)
    ),
    triage_state TEXT NOT NULL CHECK (
        typeof(triage_state) = 'text'
        AND triage_state IN (
            'recorded', 'unprocessed', 'actionable', 'ambiguous', 'ignored', 'scheduled'
        )
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        typeof(created_at) = 'text'
        AND length(CAST(created_at AS BLOB)) = 20
        AND instr(CAST(created_at AS BLOB), x'00') = 0
        AND created_at GLOB '????-??-??T??:??:??Z'
        AND created_at = strftime('%Y-%m-%dT%H:%M:%SZ', created_at)
    ),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        typeof(updated_at) = 'text'
        AND length(CAST(updated_at AS BLOB)) = 20
        AND instr(CAST(updated_at AS BLOB), x'00') = 0
        AND updated_at GLOB '????-??-??T??:??:??Z'
        AND updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', updated_at)
    ),
    UNIQUE (provider, provider_message_id),
    CHECK (
        (provider = 'voice' AND triage_state = 'recorded')
        OR (provider IN ('outlook', 'gmail') AND triage_state = 'unprocessed')
        OR triage_state IN ('actionable', 'ambiguous', 'ignored', 'scheduled')
    )
);
        "#,
    )?;

    transaction.execute_batch(
        "DROP TABLE messages;
         ALTER TABLE messages_rebuilt RENAME TO messages;
         CREATE INDEX idx_messages_received_at ON messages(received_at);",
    )?;
    Ok(())
}

fn apply_schema_v7(transaction: &Transaction<'_>) -> StoreResult<()> {
    let legacy_count: i64 =
        transaction.query_row("SELECT count(*) FROM tasks", [], |row| row.get(0))?;
    if legacy_count != 0 {
        // The old table has no trustworthy message reference, title, or state
        // contract. Refuse the rebuild before making any schema or data change.
        return Err(stored_record_invalid("task"));
    }

    transaction.execute_batch(
        r#"
CREATE TABLE tasks_rebuilt (
    id INTEGER PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        typeof(idempotency_key) = 'text'
        AND length(CAST(idempotency_key AS BLOB)) BETWEEN 1 AND 256
        AND instr(CAST(idempotency_key AS BLOB), x'00') = 0
        AND idempotency_key NOT GLOB '*[^A-Za-z0-9._:-]*'
    ),
    source_id TEXT NOT NULL UNIQUE CHECK (
        typeof(source_id) = 'text'
        AND length(CAST(source_id AS BLOB)) BETWEEN 1 AND 256
        AND instr(CAST(source_id AS BLOB), x'00') = 0
        AND source_id NOT GLOB '*[^A-Za-z0-9._:-]*'
    ),
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE RESTRICT,
    title TEXT NOT NULL CHECK (
        typeof(title) = 'text'
        AND length(CAST(title AS BLOB)) BETWEEN 1 AND 256
        AND instr(CAST(title AS BLOB), x'00') = 0
        AND length(trim(title, char(
            9, 10, 11, 12, 13, 32, 133, 160, 5760,
            8192, 8193, 8194, 8195, 8196, 8197, 8198, 8199, 8200, 8201, 8202,
            8232, 8233, 8239, 8287, 12288
        ))) > 0
        AND title NOT GLOB '*[' || char(1) || '-' || char(31) || char(127) || '-' || char(159) || ']*'
    ),
    kind TEXT NOT NULL CHECK (
        typeof(kind) = 'text'
        AND kind IN ('bill', 'callback', 'reading', 'email_reply', 'preparation')
    ),
    duration_minutes INTEGER NOT NULL CHECK (
        typeof(duration_minutes) = 'integer'
        AND duration_minutes BETWEEN 1 AND 1440
    ),
    due_at TEXT CHECK (
        due_at IS NULL OR (
            typeof(due_at) = 'text'
            AND length(CAST(due_at AS BLOB)) = 20
            AND instr(CAST(due_at AS BLOB), x'00') = 0
            AND due_at GLOB '????-??-??T??:??:??Z'
            AND due_at = strftime('%Y-%m-%dT%H:%M:%SZ', due_at)
        )
    ),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        typeof(status) = 'text'
        AND status IN ('pending', 'proposed', 'scheduled', 'no_slot')
    ),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        typeof(created_at) = 'text'
        AND length(CAST(created_at AS BLOB)) = 20
        AND instr(CAST(created_at AS BLOB), x'00') = 0
        AND created_at GLOB '????-??-??T??:??:??Z'
        AND created_at = strftime('%Y-%m-%dT%H:%M:%SZ', created_at)
    ),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        typeof(updated_at) = 'text'
        AND length(CAST(updated_at AS BLOB)) = 20
        AND instr(CAST(updated_at AS BLOB), x'00') = 0
        AND updated_at GLOB '????-??-??T??:??:??Z'
        AND updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', updated_at)
    )
);
        "#,
    )?;

    transaction.execute_batch(
        "DROP TABLE tasks;
         ALTER TABLE tasks_rebuilt RENAME TO tasks;
         CREATE INDEX idx_tasks_status_due_at ON tasks(status, due_at);",
    )?;
    Ok(())
}

fn apply_schema_v8(transaction: &Transaction<'_>) -> StoreResult<()> {
    let issued = StoredAppointmentQuoteState::Issued.storage_name();
    transaction.execute_batch(&format!(
        r#"
CREATE UNIQUE INDEX IF NOT EXISTS idx_appointment_drafts_id_quote_id
    ON appointment_drafts(id, quote_id);

CREATE TABLE appointment_quotes (
    quote_id TEXT NOT NULL PRIMARY KEY,
    appointment_kind TEXT NOT NULL CHECK (appointment_kind IN ('callback', 'meeting')),
    timezone TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT '{issued}' CHECK (state IN ('issued', 'prepared', 'consumed')),
    appointment_draft_id INTEGER UNIQUE,
    selected_slot_index INTEGER,
    consumed_at TEXT,
    proposal_id INTEGER,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        typeof(created_at) = 'text'
        AND length(CAST(created_at AS BLOB)) = 20
        AND instr(CAST(created_at AS BLOB), x'00') = 0
        AND created_at GLOB '????-??-??T??:??:??Z'
        AND created_at = strftime('%Y-%m-%dT%H:%M:%SZ', created_at)
    ),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        typeof(updated_at) = 'text'
        AND length(CAST(updated_at AS BLOB)) = 20
        AND instr(CAST(updated_at AS BLOB), x'00') = 0
        AND updated_at GLOB '????-??-??T??:??:??Z'
        AND updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', updated_at)
    ),
    FOREIGN KEY (appointment_draft_id, quote_id)
        REFERENCES appointment_drafts(id, quote_id) ON DELETE RESTRICT,
    FOREIGN KEY (quote_id, selected_slot_index)
        REFERENCES appointment_quote_slots(quote_id, slot_index),
    FOREIGN KEY (proposal_id) REFERENCES proposals(id) ON DELETE RESTRICT,
    CHECK (
        (state = 'issued'
            AND appointment_draft_id IS NULL
            AND selected_slot_index IS NULL
            AND consumed_at IS NULL
            AND proposal_id IS NULL)
        OR (state = 'prepared'
            AND appointment_draft_id IS NOT NULL
            AND selected_slot_index IS NOT NULL
            AND consumed_at IS NULL
            AND proposal_id IS NULL)
        OR (state = 'consumed'
            AND appointment_draft_id IS NOT NULL
            AND selected_slot_index IS NOT NULL
            AND consumed_at IS NOT NULL
            AND proposal_id IS NOT NULL)
    )
);

CREATE TABLE appointment_quote_slots (
    quote_id TEXT NOT NULL REFERENCES appointment_quotes(quote_id) ON DELETE RESTRICT,
    slot_index INTEGER NOT NULL CHECK (
        typeof(slot_index) = 'integer'
        AND slot_index BETWEEN 0 AND 99
    ),
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    PRIMARY KEY (quote_id, slot_index),
    UNIQUE (quote_id, starts_at, ends_at),
    CHECK (ends_at > starts_at)
);

CREATE INDEX idx_appointment_quotes_state_expires_at
    ON appointment_quotes(state, expires_at);
CREATE INDEX idx_appointment_quotes_proposal_id
    ON appointment_quotes(proposal_id);
        "#,
    ))?;
    Ok(())
}

fn apply_schema_v9(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(
        r#"
ALTER TABLE appointment_quotes ADD COLUMN slot_count INTEGER NOT NULL DEFAULT 0 CHECK (
    typeof(slot_count) = 'integer'
    AND slot_count BETWEEN 0 AND 100
);
UPDATE appointment_quotes
SET slot_count = (
    SELECT count(*) FROM appointment_quote_slots
    WHERE appointment_quote_slots.quote_id = appointment_quotes.quote_id
);
        "#,
    )?;
    Ok(())
}

fn apply_schema_v10(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE owner_task_placements (
    owner_task_draft_id INTEGER PRIMARY KEY REFERENCES owner_task_drafts(id) ON DELETE RESTRICT,
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    timezone TEXT NOT NULL,
    operation_key TEXT NOT NULL UNIQUE,
    provider_event_id TEXT UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'submitted')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK ((state = 'prepared' AND provider_event_id IS NULL) OR (state = 'submitted' AND provider_event_id IS NOT NULL))
);
        "#,
    )?;
    Ok(())
}

fn apply_schema_v11(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(r#"
        ALTER TABLE owner_task_placements ADD COLUMN owner_fingerprint TEXT NOT NULL DEFAULT 'legacy';
        CREATE TRIGGER owner_task_placement_fingerprint_insert
        BEFORE INSERT ON owner_task_placements
        WHEN length(NEW.owner_fingerprint) = 0 OR NEW.owner_fingerprint GLOB '*[^A-Za-z0-9._:-]*'
        BEGIN SELECT RAISE(ABORT, 'invalid owner fingerprint'); END;
    "#)?;
    Ok(())
}

fn apply_schema_v12(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(
        r#"
CREATE TRIGGER IF NOT EXISTS audit_events_append_only_update
BEFORE UPDATE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events are append-only');
END;

CREATE TRIGGER IF NOT EXISTS audit_events_append_only_delete
BEFORE DELETE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events are append-only');
END;
        "#,
    )?;
    Ok(())
}

fn apply_schema_v13(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(
        r#"
ALTER TABLE configuration ADD COLUMN version INTEGER NOT NULL DEFAULT 1
  CHECK (typeof(version) = 'integer' AND version >= 1);
        "#,
    )?;
    Ok(())
}

fn apply_schema_v14(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS http_idempotency_records (
    id INTEGER PRIMARY KEY,
    scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('in_progress', 'completed')),
    lease_generation INTEGER NOT NULL CHECK (lease_generation > 0),
    lease_until INTEGER NOT NULL CHECK (
        typeof(lease_until) = 'integer' AND lease_until >= 0
    ),
    response_status INTEGER,
    response_content_type TEXT,
    response_body BLOB,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        typeof(created_at) = 'text'
        AND length(CAST(created_at AS BLOB)) = 20
        AND instr(CAST(created_at AS BLOB), x'00') = 0
        AND created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9]Z'
        AND substr(created_at, 6, 2) BETWEEN '01' AND '12'
        AND substr(created_at, 9, 2) BETWEEN '01' AND '31'
        AND substr(created_at, 12, 2) BETWEEN '00' AND '23'
        AND substr(created_at, 15, 2) BETWEEN '00' AND '59'
        AND substr(created_at, 18, 2) BETWEEN '00' AND '59'
        AND created_at = strftime('%Y-%m-%dT%H:%M:%SZ', created_at)
    ),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) CHECK (
        typeof(updated_at) = 'text'
        AND length(CAST(updated_at AS BLOB)) = 20
        AND instr(CAST(updated_at AS BLOB), x'00') = 0
        AND updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9]Z'
        AND substr(updated_at, 6, 2) BETWEEN '01' AND '12'
        AND substr(updated_at, 9, 2) BETWEEN '01' AND '31'
        AND substr(updated_at, 12, 2) BETWEEN '00' AND '23'
        AND substr(updated_at, 15, 2) BETWEEN '00' AND '59'
        AND substr(updated_at, 18, 2) BETWEEN '00' AND '59'
        AND updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', updated_at)
    ),
    CHECK (
      typeof(id) = 'integer'
      AND typeof(scope) = 'text'
      AND length(CAST(scope AS BLOB)) BETWEEN 1 AND 64
      AND instr(CAST(scope AS BLOB), x'00') = 0
      AND scope NOT GLOB '*[^A-Za-z0-9._:-]*'
    ),
    CHECK (
      typeof(idempotency_key) = 'text'
      AND length(CAST(idempotency_key AS BLOB)) BETWEEN 1 AND 128
      AND instr(CAST(idempotency_key AS BLOB), x'00') = 0
      AND idempotency_key NOT GLOB '*[^A-Za-z0-9._~-]*'
    ),
    CHECK (
      typeof(fingerprint) = 'text'
      AND length(CAST(fingerprint AS BLOB)) = 64
      AND instr(CAST(fingerprint AS BLOB), x'00') = 0
      AND fingerprint NOT GLOB '*[^0-9a-f]*'
    ),
    CHECK (typeof(state) = 'text'),
    CHECK (typeof(lease_generation) = 'integer'),
    CHECK (typeof(lease_until) = 'integer'),
    CHECK (typeof(created_at) = 'text'),
    CHECK (typeof(updated_at) = 'text'),
    CHECK (
      response_status IS NULL
      OR (typeof(response_status) = 'integer' AND response_status BETWEEN 200 AND 599)
    ),
    CHECK (response_content_type IS NULL OR typeof(response_content_type) = 'text'),
    CHECK (response_body IS NULL OR typeof(response_body) = 'blob'),
    UNIQUE (scope, idempotency_key),
    CHECK (
      (state = 'in_progress'
       AND response_status IS NULL
       AND response_content_type IS NULL
       AND response_body IS NULL)
      OR
      (state = 'completed'
       AND response_status IS NOT NULL
       AND response_status BETWEEN 200 AND 599
       AND response_content_type IS NOT NULL
       AND response_content_type = 'application/json'
       AND response_body IS NOT NULL)
    )
);
CREATE INDEX IF NOT EXISTS idx_http_idempotency_records_lease_until
    ON http_idempotency_records(lease_until);
        "#,
    )?;
    Ok(())
}

fn apply_schema_v15(transaction: &Transaction<'_>) -> StoreResult<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE backup_operation_attempts (
    id INTEGER PRIMARY KEY,
    attempt_key TEXT NOT NULL UNIQUE,
    operation TEXT NOT NULL CHECK (
      operation IN ('snapshot_create', 'upload', 'restore_verify', 'retention')
    ),
    state TEXT NOT NULL CHECK (state IN ('running', 'succeeded', 'failed')),
    started_at TEXT NOT NULL,
    completed_at TEXT,
    error_code TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    CHECK (
      (state = 'running' AND completed_at IS NULL AND error_code IS NULL)
      OR
      (state = 'succeeded' AND completed_at IS NOT NULL AND error_code IS NULL)
      OR
      (state = 'failed' AND completed_at IS NOT NULL AND error_code IS NOT NULL)
    )
);
CREATE INDEX idx_backup_operation_attempts_operation_started
    ON backup_operation_attempts(operation, started_at DESC, id DESC);
        "#,
    )?;
    Ok(())
}

/// Fixed scope for HTTP idempotency records.
pub const HTTP_IDEMPOTENCY_SCOPE: &str = "pa-http-v1";
/// Maximum byte length accepted for an HTTP idempotency scope.
pub const MAX_HTTP_IDEMPOTENCY_SCOPE_LENGTH: usize = 64;
/// Maximum byte length accepted for an HTTP idempotency key.
pub const MAX_HTTP_IDEMPOTENCY_KEY_LENGTH: usize = 128;
/// Exact byte length required for an HTTP idempotency fingerprint.
pub const MAX_HTTP_IDEMPOTENCY_FINGERPRINT_LENGTH: usize = 64;
/// Lease duration for a newly reserved HTTP idempotency record.
pub const HTTP_IDEMPOTENCY_RESERVATION_SECONDS: i64 = 300;
/// Maximum cached response body size for an HTTP idempotency record.
pub const MAX_HTTP_IDEMPOTENCY_RESPONSE_BYTES: usize = 64 * 1024;

/// A validated, byte-preserving HTTP idempotency response.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpIdempotencyResponse {
    status: u16,
    body: Vec<u8>,
}

impl HttpIdempotencyResponse {
    /// Constructs a response with a bounded status and JSON body.
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> StoreResult<Self> {
        let body = body.into();
        if !(200..=599).contains(&status)
            || body.is_empty()
            || body.len() > MAX_HTTP_IDEMPOTENCY_RESPONSE_BYTES
        {
            return Err(StoreError::InvalidInput {
                field: "http idempotency response",
            });
        }

        let text = std::str::from_utf8(&body).map_err(|_| StoreError::InvalidInput {
            field: "http idempotency response",
        })?;
        let mut deserializer = serde_json::Deserializer::from_str(text);
        serde::de::IgnoredAny::deserialize(&mut deserializer).map_err(|_| {
            StoreError::InvalidInput {
                field: "http idempotency response",
            }
        })?;
        deserializer.end().map_err(|_| StoreError::InvalidInput {
            field: "http idempotency response",
        })?;

        Ok(Self { status, body })
    }

    /// Returns the original HTTP status.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Returns the fixed response content type.
    pub fn content_type(&self) -> &'static str {
        "application/json"
    }

    /// Returns the exact validated response bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

impl fmt::Debug for HttpIdempotencyResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpIdempotencyResponse")
            .field("status", &self.status)
            .field("body", &"<redacted>")
            .finish()
    }
}

/// Validates the bounded ASCII grammar used for an HTTP idempotency scope.
#[allow(dead_code)]
pub(crate) fn validate_http_idempotency_scope(value: &str) -> StoreResult<()> {
    validate_http_idempotency_value(
        value,
        1..=MAX_HTTP_IDEMPOTENCY_SCOPE_LENGTH,
        "http idempotency scope",
        |byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'),
    )
}

/// Validates the bounded ASCII grammar used for an HTTP idempotency key.
#[allow(dead_code)]
pub(crate) fn validate_http_idempotency_key(value: &str) -> StoreResult<()> {
    validate_http_idempotency_value(
        value,
        1..=MAX_HTTP_IDEMPOTENCY_KEY_LENGTH,
        "http idempotency key",
        |byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-'),
    )
}

/// Validates the exact lowercase hexadecimal HTTP idempotency fingerprint.
#[allow(dead_code)]
pub(crate) fn validate_http_idempotency_fingerprint(value: &str) -> StoreResult<()> {
    validate_http_idempotency_value(
        value,
        MAX_HTTP_IDEMPOTENCY_FINGERPRINT_LENGTH..=MAX_HTTP_IDEMPOTENCY_FINGERPRINT_LENGTH,
        "http idempotency fingerprint",
        |byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'),
    )
}

#[allow(dead_code)]
fn validate_http_idempotency_value(
    value: &str,
    length: std::ops::RangeInclusive<usize>,
    field: &'static str,
    is_allowed: fn(u8) -> bool,
) -> StoreResult<()> {
    if !length.contains(&value.len()) || !value.bytes().all(is_allowed) {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(())
}

fn normalize_legacy_audit_timestamp(value: &str) -> StoreResult<String> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map(|value| value.to_offset(time::UtcOffset::UTC))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .map_err(|_| ())
                .and_then(|value| {
                    OffsetDateTime::from_unix_timestamp(value.and_utc().timestamp()).map_err(|_| ())
                })
        })
        .map_err(|_| stored_record_invalid("audit event"))?;
    OffsetDateTime::from_unix_timestamp(parsed.unix_timestamp())
        .map_err(|_| stored_record_invalid("audit event"))?
        .format(&Rfc3339)
        .map_err(|_| stored_record_invalid("audit event"))
}

fn validate_audit_entity_id(value: String) -> StoreResult<String> {
    validate_audit_machine_identifier(value, "entity_id", MAX_AUDIT_ENTITY_ID_LENGTH)
}

fn validate_audit_idempotency_key(value: String) -> StoreResult<String> {
    validate_audit_machine_identifier(value, "idempotency_key", MAX_AUDIT_IDEMPOTENCY_KEY_LENGTH)
}

/// Validates ASCII-only audit machine identifiers accepted by the SQL schema.
///
/// The restricted alphabet (`A-Z`, `a-z`, `0-9`, `.`, `_`, `:`, `-`) keeps
/// audit identifiers safe for stable storage and makes byte-length validation
/// identical in Rust and SQLite.
fn validate_audit_machine_identifier(
    value: String,
    field: &'static str,
    maximum_length: usize,
) -> StoreResult<String> {
    if !(1..=maximum_length).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(StoreError::InvalidInput { field });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use ring::rand::{SecureRandom, SystemRandom};
    use rusqlite::{Connection, Transaction, types::Value};
    use time::{
        Duration as TimeDuration, OffsetDateTime, UtcOffset,
        format_description::well_known::Rfc3339,
    };

    use crate::pa::domain::{
        AppointmentDraft, AppointmentKind, AppointmentSlot, CallerIdentity, ConfirmedEmail,
        IdempotencyKey, OwnerTaskDraft, ProposalState, Quote, QuoteId, TaskKind,
    };

    use super::{
        AuditEntityType, AuditEventType, BUSY_TIMEOUT, CURRENT_SCHEMA_VERSION,
        MAX_APPOINTMENT_QUOTE_SLOTS, MAX_AUDIT_ENTITY_ID_LENGTH, MAX_AUDIT_LIST_LIMIT,
        MAX_MESSAGE_ID_LENGTH, MAX_MESSAGE_LIST_LIMIT, MAX_MESSAGE_SENDER_LENGTH,
        MAX_MESSAGE_SUBJECT_LENGTH, MAX_MESSAGE_SUMMARY_LENGTH, MAX_TASK_DURATION_MINUTES,
        MAX_TASK_ID_LENGTH, MAX_TASK_TITLE_LENGTH, MIGRATIONS, MessageProvider, MessageSummary,
        MessageTriageState, Migration, NotificationKind, NotificationRecipient, NotificationStatus,
        NotificationTemplateData, OAuthCredential, PaStore, ProposalSource, StoreError,
        StoreResult, StoredAppointmentDraft, StoredAppointmentQuote, StoredAppointmentQuoteState,
        StoredMessage, StoredProposal, StoredTask, StoredTaskState, TaskTitle, apply_sqlcipher_key,
        run_migrations_with, validate_audit_entity_id, validate_audit_idempotency_key,
        validate_message_id, validate_message_sender, validate_message_subject,
        validate_message_summary, verify_sqlcipher,
    };

    const DATABASE_KEY: &[u8] = b"task-4a-test-key";
    const VALID_HTTP_FINGERPRINT: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const STREAM_A: &str = "microsoft.mail:account-a";
    const STREAM_B: &str = "google.mail:account-a";
    const FIRST_CURSOR: &str = "cursor-a-1";
    const NEXT_CURSOR: &str = "cursor-a-2";
    const STALE_CURSOR: &str = "cursor-a-0";
    const REDACTION_SENTINEL: &str = "cursor-secret-sentinel-9c-a";
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDatabase {
        path: PathBuf,
    }

    impl TempDatabase {
        fn new() -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "agent_voice_pa_store_{}_{}.db",
                std::process::id(),
                sequence
            ));
            remove_database_files(&path);
            Self { path }
        }
    }

    impl Drop for TempDatabase {
        fn drop(&mut self) {
            remove_database_files(&self.path);
        }
    }

    fn remove_database_files(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-journal", path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));
    }

    fn database_file_snapshots(path: &Path) -> [Option<Vec<u8>>; 3] {
        [
            fs::read(path).ok(),
            fs::read(PathBuf::from(format!("{}-wal", path.display()))).ok(),
            fs::read(PathBuf::from(format!("{}-shm", path.display()))).ok(),
        ]
    }

    fn random_replay_nonce() -> String {
        let mut bytes = [0_u8; 24];
        SystemRandom::new()
            .fill(&mut bytes)
            .expect("runtime replay nonce generation");
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn disable_audit_append_only_triggers_for_fixture(connection: &Connection) {
        connection
            .execute_batch(
                "DROP TRIGGER audit_events_append_only_update;
                 DROP TRIGGER audit_events_append_only_delete;",
            )
            .expect("disable audit append-only triggers for corruption fixture");
    }

    fn enable_audit_append_only_triggers_after_fixture(connection: &Connection) {
        connection
            .execute_batch(
                r#"
CREATE TRIGGER audit_events_append_only_update
BEFORE UPDATE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events are append-only');
END;

CREATE TRIGGER audit_events_append_only_delete
BEFORE DELETE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit_events are append-only');
END;
                "#,
            )
            .expect("restore audit append-only triggers after corruption fixture");
    }

    fn table_names(store: &PaStore) -> Vec<String> {
        let mut statement = store
            .connection()
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
                 ORDER BY name",
            )
            .expect("table query");
        statement
            .query_map([], |row| row.get(0))
            .expect("table rows")
            .collect::<rusqlite::Result<Vec<String>>>()
            .expect("table names")
    }

    fn message_snapshot(store: &PaStore) -> Vec<Vec<Value>> {
        let mut statement = store
            .connection()
            .prepare(
                "SELECT id, idempotency_key, source_id, provider, provider_message_id,
                        summary, subject, sender, received_at, triage_state, created_at,
                        updated_at
                 FROM messages ORDER BY id ASC",
            )
            .expect("message snapshot query");
        statement
            .query_map([], |row| {
                (0..12)
                    .map(|column| row.get::<_, Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .expect("message snapshot rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("message snapshot")
    }

    fn task_snapshot(store: &PaStore) -> Vec<Vec<Value>> {
        let mut statement = store
            .connection()
            .prepare(
                "SELECT id, idempotency_key, source_id, message_id, title, kind,
                        duration_minutes, due_at, status, created_at, updated_at
                 FROM tasks ORDER BY id ASC",
            )
            .expect("task snapshot query");
        statement
            .query_map([], |row| {
                (0..11)
                    .map(|column| row.get::<_, Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .expect("task snapshot rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("task snapshot")
    }

    fn appointment_quote_snapshot(store: &PaStore, quote_id: QuoteId) -> Vec<Value> {
        let mut statement = store
            .connection()
            .prepare(
                "SELECT quote_id, appointment_kind, timezone, issued_at, expires_at, state,
                        appointment_draft_id, selected_slot_index, consumed_at, proposal_id,
                        created_at, updated_at, slot_count
                 FROM appointment_quotes WHERE quote_id = ?1",
            )
            .expect("appointment quote snapshot query");
        statement
            .query_row([quote_id.to_string()], |row| {
                (0..13)
                    .map(|column| row.get::<_, Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .expect("appointment quote snapshot")
    }

    fn actionable_task_message(store: &PaStore, suffix: &str) -> StoredMessage {
        let message = store
            .record_message(
                format!("task-message-{suffix}-key"),
                format!("task-message-{suffix}-source"),
                MessageProvider::Gmail,
                format!("task-message-{suffix}-provider-id"),
                MessageSummary::new("Invoice needs review").expect("summary"),
                Some("Invoice".to_owned()),
                Some("sender@example.com".to_owned()),
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
            )
            .expect("message");
        store
            .connection()
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .expect("allow corruption fixture");
        store
            .connection()
            .execute(
                "UPDATE messages SET triage_state = 'actionable' WHERE id = ?1",
                [message.id()],
            )
            .expect("actionable message");
        message
    }

    fn assert_invalid_message_write<F>(store: &PaStore, expected_field: &'static str, operation: F)
    where
        F: FnOnce(&PaStore) -> StoreResult<StoredMessage>,
    {
        let before = message_snapshot(store);
        let error = operation(store).expect_err("invalid message input must fail");
        assert!(matches!(
            error,
            StoreError::InvalidInput { field } if field == expected_field
        ));
        assert_eq!(
            message_snapshot(store),
            before,
            "invalid write mutated messages"
        );
    }

    fn unique_index_columns(store: &PaStore, table: &str) -> Vec<Vec<String>> {
        let mut statement = store
            .connection()
            .prepare(&format!("PRAGMA index_list({table})"))
            .expect("index list query");
        let indexes = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
            })
            .expect("index list rows")
            .collect::<rusqlite::Result<Vec<(String, bool)>>>()
            .expect("index list");
        let mut unique_columns = indexes
            .into_iter()
            .filter_map(|(name, is_unique)| is_unique.then_some(name))
            .map(|name| index_columns(store, &name))
            .collect::<Vec<_>>();
        unique_columns.sort();
        unique_columns
    }

    fn index_columns(store: &PaStore, index: &str) -> Vec<String> {
        let mut statement = store
            .connection()
            .prepare(&format!("PRAGMA index_info({index})"))
            .expect("index info query");
        statement
            .query_map([], |row| row.get(2))
            .expect("index info rows")
            .collect::<rusqlite::Result<Vec<String>>>()
            .expect("index columns")
    }

    fn assert_unique_index_columns(store: &PaStore, table: &str, expected: &[Vec<&str>]) {
        let mut expected = expected
            .iter()
            .map(|columns| columns.iter().map(|column| (*column).to_owned()).collect())
            .collect::<Vec<Vec<String>>>();
        expected.sort();
        assert_eq!(unique_index_columns(store, table), expected, "{table}");
    }

    fn assert_named_index(store: &PaStore, index: &str, table: &str, columns: &[&str]) {
        let indexed_table: String = store
            .connection()
            .query_row(
                "SELECT tbl_name FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [index],
                |row| row.get(0),
            )
            .expect("named index exists");
        assert_eq!(indexed_table, table, "{index} table");
        assert_eq!(
            index_columns(store, index),
            columns
                .iter()
                .map(|column| (*column).to_owned())
                .collect::<Vec<_>>(),
            "{index} columns"
        );
    }

    fn failing_multi_statement_migration(transaction: &Transaction<'_>) -> StoreResult<()> {
        transaction.execute_batch(
            "CREATE TABLE partial_migration_effect (id INTEGER PRIMARY KEY); \
             INSERT INTO migration_failure_marker (id) VALUES (1);",
        )?;
        Ok(())
    }

    fn keyed_connection_for_migration_test() -> Connection {
        let connection = Connection::open_in_memory().expect("open connection");
        apply_sqlcipher_key(&connection, DATABASE_KEY).expect("apply SQLCipher key");
        verify_sqlcipher(&connection).expect("verify SQLCipher");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("enable foreign keys");
        connection
    }

    #[derive(Debug)]
    struct ConfigurationDefaults {
        owner_timezone: Option<String>,
        owner_email: Option<String>,
        owner_phone: Option<String>,
        working_days: String,
        working_window_start: String,
        working_window_end: String,
        minimum_notice_minutes: i64,
        booking_horizon_days: i64,
        meeting_buffer_minutes: i64,
        retention_days: i64,
        task_duration_bill_minutes: i64,
        task_duration_callback_minutes: i64,
        task_duration_reading_minutes: i64,
        task_duration_email_reply_minutes: i64,
        task_duration_preparation_minutes: i64,
        email_triage_model: String,
    }

    #[test]
    fn empty_database_key_is_rejected() {
        let error = PaStore::open_in_memory([]).expect_err("empty key must fail");
        assert!(error.to_string().contains("database key"));
    }

    #[test]
    fn restore_open_existing_contract() {
        assert_eq!(
            MIGRATIONS.last().map(|migration| migration.version),
            Some(CURRENT_SCHEMA_VERSION)
        );

        let absent = TempDatabase::new();
        let absent_error = PaStore::open_existing(&absent.path, DATABASE_KEY)
            .expect_err("an absent restore database must not be created");
        assert!(!absent.path.exists(), "absent restore database was created");
        assert!(!absent_error.to_string().contains("agent_voice_pa_store_"));

        let removed = TempDatabase::new();
        let removed_store =
            PaStore::open(&removed.path, DATABASE_KEY).expect("seed removable store");
        drop(removed_store);
        remove_database_files(&removed.path);
        let removed_error = PaStore::open_existing(&removed.path, DATABASE_KEY)
            .expect_err("a removed restore database must not be recreated");
        assert!(
            !removed.path.exists(),
            "removed restore database was recreated"
        );
        assert!(!removed_error.to_string().contains("agent_voice_pa_store_"));

        let current = TempDatabase::new();
        let seeded = PaStore::open(&current.path, DATABASE_KEY).expect("seed current fixture");
        let schema_version: i64 = seeded
            .connection()
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("current schema version");
        assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);
        drop(seeded);

        let restored = PaStore::open_existing(&current.path, DATABASE_KEY)
            .expect("current SQLCipher fixture opens without migration");
        let foreign_keys: i64 = restored
            .connection()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign key setting");
        assert_eq!(foreign_keys, 1);
        let busy_timeout: i64 = restored
            .connection()
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy timeout setting");
        assert_eq!(busy_timeout, BUSY_TIMEOUT.as_millis() as i64);
        assert_eq!(
            restored
                .connection()
                .query_row("SELECT count(*) FROM configuration", [], |row| row
                    .get::<_, i64>(0))
                .expect("current fixture query"),
            1
        );
        drop(restored);

        let wrong_key = b"restore-open-existing-wrong-key";
        let wrong_key_error = PaStore::open_existing(&current.path, wrong_key)
            .expect_err("wrong restore key must fail");
        assert!(
            !wrong_key_error
                .to_string()
                .contains("restore-open-existing-wrong-key")
        );

        let older = TempDatabase::new();
        let mut older_connection = Connection::open(&older.path).expect("open older fixture");
        apply_sqlcipher_key(&older_connection, DATABASE_KEY).expect("apply SQLCipher key");
        verify_sqlcipher(&older_connection).expect("verify SQLCipher");
        let older_schema_version = CURRENT_SCHEMA_VERSION - 1;
        run_migrations_with(
            &mut older_connection,
            &MIGRATIONS[..older_schema_version as usize],
        )
        .expect("apply older schema");
        let older_journal_mode: String = older_connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("older journal mode");
        drop(older_connection);

        let older_restored = PaStore::open_existing(&older.path, DATABASE_KEY)
            .expect("older schema opens without migration");
        let older_version: i64 = older_restored
            .connection()
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("older schema version");
        assert_eq!(older_version, older_schema_version);
        assert_ne!(older_version, CURRENT_SCHEMA_VERSION);
        let restored_journal_mode: String = older_restored
            .connection()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("restored journal mode");
        assert_eq!(restored_journal_mode, older_journal_mode);
    }

    #[test]
    fn restore_open_existing_preserves_pending_wal_sidecars() {
        let database = TempDatabase::new();
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("seed WAL fixture");
        seed.connection()
            .execute(
                "UPDATE configuration SET owner_email = 'restore@example.com' WHERE id = 1",
                [],
            )
            .expect("write pending WAL frame");

        let before = database_file_snapshots(&database.path);
        assert!(before.iter().all(Option::is_some), "seeded WAL sidecars");

        let restored = PaStore::open_existing(&database.path, DATABASE_KEY)
            .expect("current SQLCipher WAL fixture opens");
        drop(restored);

        assert!(
            database_file_snapshots(&database.path) == before,
            "restore candidate inspection must preserve database and WAL sidecar bytes"
        );
        drop(seed);
    }

    #[test]
    fn restore_open_existing_escapes_reserved_path_characters() {
        let mut database = TempDatabase::new();
        database.path = database
            .path
            .with_file_name("agent_voice_restore?reserved#percent%.db");
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("seed reserved-path fixture");
        drop(seed);

        let restored = PaStore::open_existing(&database.path, DATABASE_KEY)
            .expect("reserved-path SQLCipher fixture opens");
        assert_eq!(
            restored
                .connection()
                .query_row("SELECT count(*) FROM configuration", [], |row| row
                    .get::<_, i64>(0))
                .expect("reserved-path fixture query"),
            1
        );
    }

    #[test]
    fn immutable_uri_builder_distinguishes_windows_relative_and_absolute_paths() {
        assert_eq!(
            super::immutable_read_only_uri_for_path_bytes(b"relative.db", true, false),
            "file:relative.db?immutable=1"
        );
        assert_eq!(
            super::immutable_read_only_uri_for_path_bytes(b"C:/restore.db", true, true),
            "file:///C:/restore.db?immutable=1"
        );
    }

    #[test]
    fn restore_open_existing_preserves_empty_quiescent_rollback_journal() {
        let database = TempDatabase::new();
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("seed empty journal fixture");
        drop(seed);

        let journal_path = PathBuf::from(format!("{}-journal", database.path.display()));
        fs::write(&journal_path, []).expect("create empty quiescent rollback journal");
        let before = [
            fs::read(&database.path).expect("snapshot database"),
            fs::read(&journal_path).expect("snapshot empty rollback journal"),
        ];

        let restored = PaStore::open_existing(&database.path, DATABASE_KEY)
            .expect("empty quiescent rollback journal opens");
        drop(restored);

        assert!(
            [
                fs::read(&database.path).expect("database remains"),
                fs::read(&journal_path).expect("empty rollback journal remains"),
            ] == before,
            "restore candidate inspection must preserve empty rollback journal bytes"
        );
    }

    #[test]
    fn restore_open_existing_preserves_zero_header_quiescent_rollback_journal() {
        let database = TempDatabase::new();
        let seed =
            PaStore::open(&database.path, DATABASE_KEY).expect("seed zero-header journal fixture");
        drop(seed);

        let journal_path = PathBuf::from(format!("{}-journal", database.path.display()));
        fs::write(&journal_path, [0_u8; 512]).expect("create zero-header rollback journal");
        let before = [
            fs::read(&database.path).expect("snapshot database"),
            fs::read(&journal_path).expect("snapshot zero-header rollback journal"),
        ];

        let restored = PaStore::open_existing(&database.path, DATABASE_KEY)
            .expect("zero-header quiescent rollback journal opens");
        drop(restored);

        assert!(
            [
                fs::read(&database.path).expect("database remains"),
                fs::read(&journal_path).expect("zero-header rollback journal remains"),
            ] == before,
            "restore candidate inspection must preserve zero-header rollback journal bytes"
        );
    }

    #[test]
    fn restore_open_existing_rejects_genuine_interrupted_rollback_journal() {
        let source = TempDatabase::new();
        let seed = PaStore::open(&source.path, DATABASE_KEY).expect("seed journal fixture");
        drop(seed);

        let connection = Connection::open(&source.path).expect("open rollback journal fixture");
        apply_sqlcipher_key(&connection, DATABASE_KEY).expect("apply SQLCipher key");
        connection
            .execute_batch(
                "PRAGMA journal_mode = DELETE;
                 PRAGMA cache_size = 1;
                 BEGIN IMMEDIATE;
                 CREATE TABLE interrupted_fixture (id INTEGER PRIMARY KEY, value BLOB);
                 INSERT INTO interrupted_fixture VALUES (1, zeroblob(4096));
                 INSERT INTO interrupted_fixture VALUES (2, zeroblob(4096));
                 INSERT INTO interrupted_fixture VALUES (3, zeroblob(4096));
                 INSERT INTO interrupted_fixture VALUES (4, zeroblob(4096));",
            )
            .expect("create rollback journal");

        let interrupted = TempDatabase::new();
        let source_journal_path = PathBuf::from(format!("{}-journal", source.path.display()));
        let interrupted_journal_path =
            PathBuf::from(format!("{}-journal", interrupted.path.display()));
        let source_journal = fs::read(&source_journal_path).expect("read source rollback journal");
        assert_eq!(
            &source_journal[..8],
            b"\xd9\xd5\x05\xf9\x20\xa1\x63\xd7",
            "fixture must contain a hot rollback journal header"
        );
        fs::copy(&source.path, &interrupted.path).expect("copy interrupted database");
        fs::copy(&source_journal_path, &interrupted_journal_path)
            .expect("copy interrupted rollback journal");
        connection
            .execute_batch("ROLLBACK;")
            .expect("restore fixture");
        drop(connection);

        let before = [
            fs::read(&interrupted.path).expect("snapshot interrupted database"),
            fs::read(&interrupted_journal_path).expect("snapshot interrupted rollback journal"),
        ];
        let error = PaStore::open_existing(&interrupted.path, DATABASE_KEY)
            .expect_err("interrupted rollback journal must fail closed");
        assert!(matches!(
            error,
            StoreError::NotFound {
                resource: "database"
            }
        ));

        assert!(
            [
                fs::read(&interrupted.path).expect("interrupted database remains"),
                fs::read(&interrupted_journal_path).expect("interrupted rollback journal remains"),
            ] == before,
            "restore candidate inspection must preserve database and rollback journal bytes"
        );
    }

    #[test]
    fn task_types_expose_closed_states_and_redacted_validated_titles() {
        for (text, state) in [
            ("pending", StoredTaskState::Pending),
            ("proposed", StoredTaskState::Proposed),
            ("scheduled", StoredTaskState::Scheduled),
            ("no_slot", StoredTaskState::NoSlot),
        ] {
            assert_eq!(StoredTaskState::from_storage(text).expect("state"), state);
            assert_eq!(state.as_str(), text);
        }
        assert!(StoredTaskState::from_storage("task-secret-state").is_err());

        let title = TaskTitle::new("Review the invoice").expect("title");
        assert_eq!(title.as_str(), "Review the invoice");
        assert!(TaskTitle::new("\u{2003}").is_err());
        assert!(TaskTitle::new("title\nsecret").is_err());
        assert!(TaskTitle::new("x".repeat(MAX_TASK_TITLE_LENGTH + 1)).is_err());
        assert!(TaskTitle::new("é".repeat(MAX_TASK_TITLE_LENGTH / 2 + 1)).is_err());
        let debug = format!("{title:?}");
        assert!(!debug.contains("Review the invoice"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn task_repository_records_and_reads_actionable_email_task() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let message = store
            .record_message(
                "task-message-key",
                "task-message-source",
                MessageProvider::Gmail,
                "task-provider-message",
                MessageSummary::new("Invoice needs review").expect("summary"),
                Some("Invoice".to_owned()),
                Some("sender@example.com".to_owned()),
                received_at,
            )
            .expect("message");
        store
            .connection()
            .execute(
                "UPDATE messages SET triage_state = 'actionable' WHERE id = ?1",
                [message.id()],
            )
            .expect("actionable message");
        let task = store
            .record_task(
                "task-key",
                "task-source",
                message.id(),
                TaskTitle::new("Review the invoice").expect("title"),
                TaskKind::Bill,
                None,
                None,
            )
            .expect("task");
        assert_eq!(task.state(), StoredTaskState::Pending);
        assert_eq!(
            store
                .load_task_by_idempotency_key("task-key")
                .expect("task by key"),
            task
        );
    }

    #[test]
    fn task_retry_by_both_identities_returns_original_row_after_every_later_state() {
        for state in [
            StoredTaskState::Proposed,
            StoredTaskState::Scheduled,
            StoredTaskState::NoSlot,
        ] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let message = actionable_task_message(&store, state.as_str());
            let task = store
                .record_task(
                    format!("task-retry-{}-key", state.as_str()),
                    format!("task-retry-{}-source", state.as_str()),
                    message.id(),
                    TaskTitle::new("Review the invoice").expect("title"),
                    TaskKind::Bill,
                    Some(30),
                    Some(
                        OffsetDateTime::parse("2025-01-03T03:04:05Z", &Rfc3339).expect("due time"),
                    ),
                )
                .expect("task");
            store
                .connection()
                .execute(
                    "UPDATE tasks SET status = ?1 WHERE id = ?2",
                    rusqlite::params![state.as_str(), task.id()],
                )
                .expect("later lifecycle state fixture");

            let retry = store
                .record_task(
                    task.idempotency_key(),
                    task.source_id(),
                    message.id(),
                    TaskTitle::new("Review the invoice").expect("title"),
                    TaskKind::Bill,
                    Some(30),
                    Some(
                        OffsetDateTime::parse("2025-01-03T03:04:05Z", &Rfc3339).expect("due time"),
                    ),
                )
                .expect("exact retry after later state");
            assert_eq!(retry.state(), state);
            assert_eq!(retry, store.load_task_by_id(task.id()).expect("task by id"));
            assert_eq!(
                retry,
                store
                    .load_task_by_idempotency_key(task.idempotency_key())
                    .expect("task by idempotency key")
            );
            assert_eq!(
                retry,
                store
                    .load_task_by_source_id(task.source_id())
                    .expect("task by source identity")
            );
        }
    }

    #[test]
    fn task_retries_conflict_for_crossed_or_changed_immutable_inputs_without_mutation() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let first_message = actionable_task_message(&store, "first");
        let second_message = actionable_task_message(&store, "second");
        let task = store
            .record_task(
                "task-conflict-key",
                "task-conflict-source",
                first_message.id(),
                TaskTitle::new("Review the invoice").expect("title"),
                TaskKind::Bill,
                Some(30),
                Some(OffsetDateTime::parse("2025-01-03T03:04:05Z", &Rfc3339).expect("due time")),
            )
            .expect("task");
        let before = task_snapshot(&store);

        let attempts = [
            (
                "task-conflict-key",
                "other-source",
                first_message.id(),
                "Review the invoice",
                TaskKind::Bill,
                Some(30),
                Some("2025-01-03T03:04:05Z"),
            ),
            (
                "other-key",
                "task-conflict-source",
                first_message.id(),
                "Review the invoice",
                TaskKind::Bill,
                Some(30),
                Some("2025-01-03T03:04:05Z"),
            ),
            (
                "task-conflict-key",
                "task-conflict-source",
                second_message.id(),
                "Review the invoice",
                TaskKind::Bill,
                Some(30),
                Some("2025-01-03T03:04:05Z"),
            ),
            (
                "task-conflict-key",
                "task-conflict-source",
                first_message.id(),
                "Changed title",
                TaskKind::Bill,
                Some(30),
                Some("2025-01-03T03:04:05Z"),
            ),
            (
                "task-conflict-key",
                "task-conflict-source",
                first_message.id(),
                "Review the invoice",
                TaskKind::Callback,
                Some(30),
                Some("2025-01-03T03:04:05Z"),
            ),
            (
                "task-conflict-key",
                "task-conflict-source",
                first_message.id(),
                "Review the invoice",
                TaskKind::Bill,
                Some(15),
                Some("2025-01-03T03:04:05Z"),
            ),
            (
                "task-conflict-key",
                "task-conflict-source",
                first_message.id(),
                "Review the invoice",
                TaskKind::Bill,
                Some(30),
                None,
            ),
        ];
        for (key, source, message_id, title, kind, duration, due_at) in attempts {
            let due_at =
                due_at.map(|value| OffsetDateTime::parse(value, &Rfc3339).expect("due time"));
            assert!(matches!(
                store.record_task(
                    key,
                    source,
                    message_id,
                    TaskTitle::new(title).expect("title"),
                    kind,
                    duration,
                    due_at,
                ),
                Err(StoreError::Conflict { resource: "task" })
            ));
            assert_eq!(task_snapshot(&store), before);
        }
        assert_eq!(
            store
                .load_task_by_id(task.id())
                .expect("unchanged task")
                .created_at(),
            task.created_at()
        );
        assert_eq!(
            store
                .load_task_by_id(task.id())
                .expect("unchanged task")
                .updated_at(),
            task.updated_at()
        );
    }

    #[test]
    fn concurrent_identical_task_records_return_the_single_stored_row() {
        let database = TempDatabase::new();
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("open seed store");
        let message = actionable_task_message(&seed, "concurrent");
        let message_id = message.id();
        drop(seed);

        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.record_task(
                "task-concurrent-key",
                "task-concurrent-source",
                message_id,
                TaskTitle::new("Review the invoice").expect("title"),
                TaskKind::Bill,
                None,
                None,
            )
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.record_task(
                "task-concurrent-key",
                "task-concurrent-source",
                message_id,
                TaskTitle::new("Review the invoice").expect("title"),
                TaskKind::Bill,
                None,
                None,
            )
        });
        let first = first_handle
            .join()
            .expect("first record thread")
            .expect("first task");
        let second = second_handle
            .join()
            .expect("second record thread")
            .expect("second task");
        assert_eq!(first, second);
        assert_eq!(first.id(), second.id());
        let store = PaStore::open(&database.path, DATABASE_KEY).expect("reopen store");
        assert_eq!(
            store
                .load_task_by_idempotency_key("task-concurrent-key")
                .expect("single task")
                .id(),
            first.id()
        );
        assert_eq!(task_snapshot(&store).len(), 1);
    }

    #[test]
    fn task_record_rejects_missing_voice_non_actionable_and_malformed_messages_without_mutation() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let voice = store
            .record_message(
                "task-voice-key",
                "task-voice-source",
                MessageProvider::Voice,
                "task-voice-provider-id",
                MessageSummary::new("Voice summary").expect("summary"),
                None,
                None,
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
            )
            .expect("voice message");
        let email = store
            .record_message(
                "task-email-key",
                "task-email-source",
                MessageProvider::Outlook,
                "task-email-provider-id",
                MessageSummary::new("Email summary").expect("summary"),
                None,
                None,
                OffsetDateTime::parse("2025-01-02T03:04:06Z", &Rfc3339).expect("received time"),
            )
            .expect("email message");
        let malformed = actionable_task_message(&store, "malformed");
        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .expect("allow corruption fixture");
        store
            .connection()
            .execute(
                "UPDATE messages SET summary = '   ' WHERE id = ?1",
                [malformed.id()],
            )
            .expect("corrupt referenced message");

        for message_id in [999_999, voice.id(), email.id(), malformed.id()] {
            let before = task_snapshot(&store);
            assert!(matches!(
                store.record_task(
                    format!("rejected-task-{message_id}-key"),
                    format!("rejected-task-{message_id}-source"),
                    message_id,
                    TaskTitle::new("Review the invoice").expect("title"),
                    TaskKind::Bill,
                    None,
                    None,
                ),
                Err(StoreError::StoredRecordInvalid {
                    resource: "message" | "task"
                })
            ));
            assert_eq!(task_snapshot(&store), before);
        }
    }

    #[test]
    fn task_lookups_fail_closed_for_every_selected_task_field_corruption() {
        enum CorruptValue {
            Integer(i64),
            Text(&'static str),
        }

        for (column, value) in [
            ("id", CorruptValue::Integer(0)),
            ("idempotency_key", CorruptValue::Text("invalid task key")),
            ("source_id", CorruptValue::Text("invalid task source")),
            ("message_id", CorruptValue::Integer(0)),
            ("title", CorruptValue::Text("   ")),
            ("kind", CorruptValue::Text("invalid-kind")),
            ("duration_minutes", CorruptValue::Integer(0)),
            ("due_at", CorruptValue::Text("invalid-time")),
            ("status", CorruptValue::Text("invalid-status")),
            ("created_at", CorruptValue::Text("invalid-time")),
            ("updated_at", CorruptValue::Text("invalid-time")),
        ] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let message = actionable_task_message(&store, &format!("corrupt-{column}"));
            let task = store
                .record_task(
                    format!("task-corrupt-{column}-key"),
                    format!("task-corrupt-{column}-source"),
                    message.id(),
                    TaskTitle::new("Review the invoice").expect("title"),
                    TaskKind::Bill,
                    None,
                    None,
                )
                .expect("task");
            store
                .connection()
                .execute_batch("PRAGMA ignore_check_constraints = ON;")
                .expect("allow corruption fixture");
            if column == "message_id" {
                store
                    .connection()
                    .execute_batch("PRAGMA foreign_keys = OFF;")
                    .expect("allow corrupt message reference fixture");
            }
            match value {
                CorruptValue::Integer(value) => store
                    .connection()
                    .execute(
                        &format!("UPDATE tasks SET {column} = ?1 WHERE id = ?2"),
                        rusqlite::params![value, task.id()],
                    )
                    .expect("corrupt integer task field"),
                CorruptValue::Text(value) => store
                    .connection()
                    .execute(
                        &format!("UPDATE tasks SET {column} = ?1 WHERE id = ?2"),
                        rusqlite::params![value, task.id()],
                    )
                    .expect("corrupt text task field"),
            };
            let before = task_snapshot(&store);

            let assert_stored_record_error = |error: StoreError| {
                assert!(matches!(
                    error,
                    StoreError::StoredRecordInvalid { resource: "task" }
                ));
                if let CorruptValue::Text(value) = value {
                    assert!(!error.to_string().contains(value));
                    assert!(!format!("{error:?}").contains(value));
                }
            };
            let assert_unchanged = || assert_eq!(task_snapshot(&store), before, "{column}");

            match column {
                "id" => {
                    let error = store
                        .load_task_by_id(task.id())
                        .expect_err("the original task identity no longer resolves");
                    assert!(matches!(error, StoreError::NotFound { resource: "task" }));
                    assert_unchanged();
                    let error = store
                        .load_task_by_id(0)
                        .expect_err("the malformed task identity is rejected at the boundary");
                    assert!(matches!(error, StoreError::InvalidInput { field: "id" }));
                    assert_unchanged();
                    for result in [
                        store.load_task_by_idempotency_key(task.idempotency_key()),
                        store.load_task_by_source_id(task.source_id()),
                    ] {
                        assert_stored_record_error(
                            result.expect_err("alternate lookup rejects corrupt id"),
                        );
                        assert_unchanged();
                    }
                }
                "idempotency_key" => {
                    for result in [
                        store.load_task_by_id(task.id()),
                        store.load_task_by_source_id(task.source_id()),
                    ] {
                        assert_stored_record_error(
                            result.expect_err("alternate lookup rejects corrupt idempotency key"),
                        );
                        assert_unchanged();
                    }
                    let error = store
                        .load_task_by_idempotency_key("invalid task key")
                        .expect_err("malformed idempotency lookup input is rejected");
                    assert!(matches!(
                        error,
                        StoreError::InvalidInput {
                            field: "idempotency_key"
                        }
                    ));
                    assert!(!error.to_string().contains("invalid task key"));
                    assert!(!format!("{error:?}").contains("invalid task key"));
                    assert_unchanged();
                }
                "source_id" => {
                    for result in [
                        store.load_task_by_id(task.id()),
                        store.load_task_by_idempotency_key(task.idempotency_key()),
                    ] {
                        assert_stored_record_error(
                            result.expect_err("alternate lookup rejects corrupt source identity"),
                        );
                        assert_unchanged();
                    }
                    let error = store
                        .load_task_by_source_id("invalid task source")
                        .expect_err("malformed source lookup input is rejected");
                    assert!(matches!(
                        error,
                        StoreError::InvalidInput { field: "source_id" }
                    ));
                    assert!(!error.to_string().contains("invalid task source"));
                    assert!(!format!("{error:?}").contains("invalid task source"));
                    assert_unchanged();
                }
                _ => {
                    for result in [
                        store.load_task_by_id(task.id()),
                        store.load_task_by_idempotency_key(task.idempotency_key()),
                        store.load_task_by_source_id(task.source_id()),
                    ] {
                        assert_stored_record_error(
                            result.expect_err("corrupt task rows must not load"),
                        );
                        assert_unchanged();
                    }
                }
            }
        }
    }

    #[test]
    fn stored_task_debug_redacts_sensitive_fields_and_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<StoredTask>();
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let message = actionable_task_message(&store, "debug");
        let task = store
            .record_task(
                "task-debug-key",
                "task-debug-source",
                message.id(),
                TaskTitle::new("Secret invoice review").expect("title"),
                TaskKind::Bill,
                None,
                None,
            )
            .expect("task");
        let debug = format!("{task:?}");
        for secret in [
            "task-debug-key",
            "task-debug-source",
            "Secret invoice review",
        ] {
            assert!(!debug.contains(secret));
        }
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn sqlcipher_is_available_before_schema_access() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open SQLCipher store");
        let cipher_version: String = store
            .connection()
            .query_row("PRAGMA cipher_version", [], |row| row.get(0))
            .expect("cipher version");
        assert!(!cipher_version.trim().is_empty());
    }

    #[test]
    fn migrations_are_transactional_and_idempotent() {
        let database = TempDatabase::new();
        let store = PaStore::open(&database.path, DATABASE_KEY).expect("initial open");
        let migration_count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration count");
        let migration_version: i64 = store
            .connection()
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration version");
        assert_eq!(migration_count, CURRENT_SCHEMA_VERSION);
        assert_eq!(migration_version, CURRENT_SCHEMA_VERSION);
        drop(store);

        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen");
        let reopened_count: i64 = reopened
            .connection()
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("reopened migration count");
        assert_eq!(reopened_count, migration_count);
    }

    #[test]
    fn migration_v15_adds_backup_attempt_schema() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open v15 store");
        let schema_version: i64 = store
            .connection()
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("schema version");
        assert_eq!(schema_version, 15);

        let columns = store
            .connection()
            .prepare("PRAGMA table_info('backup_operation_attempts')")
            .expect("backup attempts table info")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .expect("backup attempts columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("backup attempts column rows");
        assert_eq!(
            columns,
            vec![
                ("id".to_owned(), "INTEGER".to_owned(), 0, None, 1),
                ("attempt_key".to_owned(), "TEXT".to_owned(), 1, None, 0),
                ("operation".to_owned(), "TEXT".to_owned(), 1, None, 0),
                ("state".to_owned(), "TEXT".to_owned(), 1, None, 0),
                ("started_at".to_owned(), "TEXT".to_owned(), 1, None, 0),
                ("completed_at".to_owned(), "TEXT".to_owned(), 0, None, 0),
                ("error_code".to_owned(), "TEXT".to_owned(), 0, None, 0),
                (
                    "created_at".to_owned(),
                    "TEXT".to_owned(),
                    1,
                    Some("strftime('%Y-%m-%dT%H:%M:%SZ', 'now')".to_owned()),
                    0
                ),
                (
                    "updated_at".to_owned(),
                    "TEXT".to_owned(),
                    1,
                    Some("strftime('%Y-%m-%dT%H:%M:%SZ', 'now')".to_owned()),
                    0
                ),
            ]
        );

        assert_named_index(
            &store,
            "idx_backup_operation_attempts_operation_started",
            "backup_operation_attempts",
            &["operation", "started_at", "id"],
        );
        let index_columns = store
            .connection()
            .prepare("PRAGMA index_xinfo('idx_backup_operation_attempts_operation_started')")
            .expect("backup attempts index info")
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, i64>(5)? != 0,
                ))
            })
            .expect("backup attempts index columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("backup attempts index column rows")
            .into_iter()
            .filter_map(|(name, is_descending, is_key)| {
                is_key.then(|| (name.expect("key index column name"), is_descending))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            index_columns,
            vec![
                ("operation".to_owned(), false),
                ("started_at".to_owned(), true),
                ("id".to_owned(), true),
            ]
        );
    }

    #[test]
    fn backup_attempt_schema_defaults_are_canonical_utc() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open v15 store");
        store
            .connection()
            .execute(
                "INSERT INTO backup_operation_attempts (
                     attempt_key, operation, state, started_at
                 ) VALUES ('default-timestamps', 'upload', 'running',
                           '2026-09-01T00:00:00Z')",
                [],
            )
            .expect("insert row using timestamp defaults");
        let defaults: (String, String) = store
            .connection()
            .query_row(
                "SELECT created_at, updated_at
                 FROM backup_operation_attempts
                 WHERE attempt_key = 'default-timestamps'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read timestamp defaults");
        for value in [defaults.0, defaults.1] {
            assert_eq!(value.len(), 20, "canonical timestamps use whole seconds");
            assert!(value.ends_with('Z'), "canonical timestamps are UTC");
            let parsed =
                OffsetDateTime::parse(&value, &Rfc3339).expect("default timestamp is RFC3339");
            assert_eq!(parsed.offset(), UtcOffset::UTC);
            assert_eq!(parsed.nanosecond(), 0);
            assert_eq!(
                parsed.format(&Rfc3339).expect("format timestamp"),
                value,
                "default timestamp is strict canonical RFC3339"
            );
        }
    }

    #[test]
    fn migration_v15_preserves_v14_rows_and_reopens_once() {
        let database = TempDatabase::new();
        let v14_migrations = &MIGRATIONS[..MIGRATIONS
            .iter()
            .position(|migration| migration.version == 15)
            .expect("v15 migration")];
        let table_row_counts = |connection: &Connection| {
            let table_names = connection
                .prepare(
                    "SELECT name FROM sqlite_master
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                     ORDER BY name",
                )
                .expect("v14 table names query")
                .query_map([], |row| row.get::<_, String>(0))
                .expect("v14 table name rows")
                .collect::<rusqlite::Result<Vec<_>>>()
                .expect("v14 table names");
            table_names
                .into_iter()
                .map(|table| {
                    let count: i64 = connection
                        .query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |row| {
                            row.get(0)
                        })
                        .expect("v14 table row count");
                    (table, count)
                })
                .collect::<Vec<_>>()
        };
        let v14_snapshot = {
            let mut connection = Connection::open(&database.path).expect("open v14 database");
            apply_sqlcipher_key(&connection, DATABASE_KEY).expect("apply SQLCipher key");
            verify_sqlcipher(&connection).expect("verify SQLCipher");
            connection
                .pragma_update(None, "foreign_keys", true)
                .expect("enable foreign keys");
            run_migrations_with(&mut connection, v14_migrations).expect("apply v14 schema");
            connection
                .execute(
                    "UPDATE configuration SET owner_email = ?1 WHERE id = 1",
                    ["owner-v14@example.test"],
                )
                .expect("seed v14 configuration");
            connection
                .execute(
                    "INSERT INTO http_idempotency_records (
                         scope, idempotency_key, fingerprint, state, lease_generation,
                         lease_until
                     ) VALUES (?1, ?2, ?3, 'in_progress', 1, ?4)",
                    rusqlite::params![
                        "scope-v14",
                        "key-v14",
                        VALID_HTTP_FINGERPRINT,
                        1_700_000_000_i64
                    ],
                )
                .expect("seed v14 idempotency row");
            let configuration: (Option<String>, String) = connection
                .query_row(
                    "SELECT owner_email, email_triage_model FROM configuration WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("v14 configuration snapshot");
            let idempotency: (String, String, String, String, i64, i64) = connection
                .query_row(
                    "SELECT scope, idempotency_key, fingerprint, state,
                            lease_generation, lease_until
                     FROM http_idempotency_records",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .expect("v14 idempotency snapshot");
            (table_row_counts(&connection), configuration, idempotency)
        };
        let assert_v14_table_counts = |connection: &Connection| {
            let actual = table_row_counts(connection);
            for (table, before_count) in &v14_snapshot.0 {
                let expected = if table == "schema_migrations" {
                    *before_count + 1
                } else {
                    *before_count
                };
                assert_eq!(
                    actual
                        .iter()
                        .find(|(name, _)| name == table)
                        .map(|(_, count)| *count),
                    Some(expected),
                    "{table} row count changed"
                );
            }
            assert_eq!(
                actual
                    .iter()
                    .find(|(name, _)| name == "backup_operation_attempts")
                    .map(|(_, count)| *count),
                Some(0),
                "new backup-attempt table must start empty"
            );
        };

        {
            let store = PaStore::open(&database.path, DATABASE_KEY).expect("migrate v14 store");
            assert_v14_table_counts(store.connection());
            assert_eq!(
                store
                    .connection()
                    .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .expect("migrated schema version"),
                15
            );
            assert_eq!(
                store
                    .connection()
                    .query_row(
                        "SELECT owner_email, email_triage_model
                         FROM configuration WHERE id = 1",
                        [],
                        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
                    )
                    .expect("preserved configuration"),
                v14_snapshot.1
            );
            assert_eq!(
                store
                    .connection()
                    .query_row(
                        "SELECT scope, idempotency_key, fingerprint, state,
                                lease_generation, lease_until
                         FROM http_idempotency_records",
                        [],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, i64>(4)?,
                                row.get::<_, i64>(5)?,
                            ))
                        },
                    )
                    .expect("preserved idempotency row"),
                v14_snapshot.2
            );
            assert_eq!(
                store
                    .connection()
                    .query_row(
                        "SELECT count(*) FROM schema_migrations WHERE version = 15",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("v15 migration count"),
                1
            );
        }

        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen v15 store");
        assert_v14_table_counts(reopened.connection());
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("reopened migration count"),
            15
        );
        assert_eq!(
            reopened
                .connection()
                .query_row(
                    "SELECT count(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'backup_operation_attempts'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("backup attempts table count"),
            1
        );
        assert_eq!(
            reopened
                .connection()
                .query_row(
                    "SELECT owner_email, email_triage_model
                     FROM configuration WHERE id = 1",
                    [],
                    |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
                )
                .expect("reopened preserved configuration"),
            v14_snapshot.1
        );
        assert_eq!(
            reopened
                .connection()
                .query_row(
                    "SELECT scope, idempotency_key, fingerprint, state,
                            lease_generation, lease_until
                     FROM http_idempotency_records",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .expect("reopened preserved idempotency row"),
            v14_snapshot.2
        );
    }

    #[test]
    fn backup_attempt_schema_rejects_invalid_state_rows() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open v15 store");
        let insert = |attempt_key: &str,
                      operation: &str,
                      state: &str,
                      completed_at: Option<&str>,
                      error_code: Option<&str>| {
            store.connection().execute(
                "INSERT INTO backup_operation_attempts (
                     attempt_key, operation, state, started_at, completed_at, error_code
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    attempt_key,
                    operation,
                    state,
                    "2026-09-01T00:00:00Z",
                    completed_at,
                    error_code
                ],
            )
        };

        for (index, operation) in ["snapshot_create", "upload", "restore_verify", "retention"]
            .into_iter()
            .enumerate()
        {
            insert(
                &format!("valid-running-{index}"),
                operation,
                "running",
                None,
                None,
            )
            .expect("valid running row");
            insert(
                &format!("valid-succeeded-{index}"),
                operation,
                "succeeded",
                Some("2026-09-01T00:01:00Z"),
                None,
            )
            .expect("valid succeeded row");
            insert(
                &format!("valid-failed-{index}"),
                operation,
                "failed",
                Some("2026-09-01T00:02:00Z"),
                Some("provider_error"),
            )
            .expect("valid failed row");
        }

        for (attempt_key, operation, state, completed_at, error_code) in [
            ("invalid-operation", "unknown", "running", None, None),
            ("invalid-state", "upload", "paused", None, None),
            (
                "running-completed",
                "upload",
                "running",
                Some("2026-09-01T00:01:00Z"),
                None,
            ),
            (
                "running-error",
                "upload",
                "running",
                None,
                Some("provider_error"),
            ),
            ("succeeded-incomplete", "upload", "succeeded", None, None),
            (
                "succeeded-error",
                "upload",
                "succeeded",
                Some("2026-09-01T00:01:00Z"),
                Some("provider_error"),
            ),
            (
                "failed-incomplete",
                "upload",
                "failed",
                None,
                Some("provider_error"),
            ),
            (
                "failed-without-error",
                "upload",
                "failed",
                Some("2026-09-01T00:01:00Z"),
                None,
            ),
        ] {
            assert!(
                insert(attempt_key, operation, state, completed_at, error_code).is_err(),
                "{attempt_key} must be rejected"
            );
        }
        assert!(insert("duplicate-attempt-key", "upload", "running", None, None).is_ok());
        assert!(insert("duplicate-attempt-key", "upload", "running", None, None).is_err());

        assert_eq!(
            store
                .connection()
                .query_row(
                    "SELECT count(*) FROM backup_operation_attempts",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .expect("valid backup attempts remain"),
            13
        );
    }

    #[test]
    fn migrations_record_every_version_once() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, MIGRATIONS).expect("apply v15 schema");
        run_migrations_with(&mut connection, MIGRATIONS).expect("reapply v15 schema");

        let versions: Vec<i64> = connection
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .expect("migration versions query")
            .query_map([], |row| row.get(0))
            .expect("migration version rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("migration versions");
        assert_eq!(versions, (1..=15).collect::<Vec<_>>());
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("migration count"),
            15
        );
    }

    #[test]
    fn http_idempotency_v14_migration_creates_schema() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let schema_version: i64 = store
            .connection()
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("schema version");
        assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);

        let table_sql: String = store
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'http_idempotency_records'",
                [],
                |row| row.get(0),
            )
            .expect("idempotency table");
        assert!(table_sql.contains("UNIQUE (scope, idempotency_key)"));
        assert!(table_sql.contains("state IN ('in_progress', 'completed')"));
        let columns = store
            .connection()
            .prepare("PRAGMA table_info('http_idempotency_records')")
            .expect("idempotency table info")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .expect("idempotency columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("idempotency column rows");
        assert_eq!(
            columns,
            vec![
                ("id".to_owned(), "INTEGER".to_owned(), 0, None, 1),
                ("scope".to_owned(), "TEXT".to_owned(), 1, None, 0),
                ("idempotency_key".to_owned(), "TEXT".to_owned(), 1, None, 0),
                ("fingerprint".to_owned(), "TEXT".to_owned(), 1, None, 0),
                ("state".to_owned(), "TEXT".to_owned(), 1, None, 0),
                (
                    "lease_generation".to_owned(),
                    "INTEGER".to_owned(),
                    1,
                    None,
                    0
                ),
                ("lease_until".to_owned(), "INTEGER".to_owned(), 1, None, 0),
                (
                    "response_status".to_owned(),
                    "INTEGER".to_owned(),
                    0,
                    None,
                    0
                ),
                (
                    "response_content_type".to_owned(),
                    "TEXT".to_owned(),
                    0,
                    None,
                    0
                ),
                ("response_body".to_owned(), "BLOB".to_owned(), 0, None, 0),
                (
                    "created_at".to_owned(),
                    "TEXT".to_owned(),
                    1,
                    Some("strftime('%Y-%m-%dT%H:%M:%SZ', 'now')".to_owned()),
                    0
                ),
                (
                    "updated_at".to_owned(),
                    "TEXT".to_owned(),
                    1,
                    Some("strftime('%Y-%m-%dT%H:%M:%SZ', 'now')".to_owned()),
                    0
                ),
            ]
        );
        let indexes = store
            .connection()
            .prepare("PRAGMA index_list('http_idempotency_records')")
            .expect("idempotency index list")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .expect("idempotency indexes")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("idempotency index rows");
        let lease_index = indexes
            .iter()
            .find(|(name, _, _, _)| name == "idx_http_idempotency_records_lease_until")
            .expect("lease index metadata");
        assert_eq!(
            lease_index,
            &(
                "idx_http_idempotency_records_lease_until".to_owned(),
                0,
                "c".to_owned(),
                0
            )
        );
        assert_named_index(
            &store,
            "idx_http_idempotency_records_lease_until",
            "http_idempotency_records",
            &["lease_until"],
        );

        store
            .connection()
            .execute(
                "INSERT INTO http_idempotency_records (
                     scope, idempotency_key, fingerprint, state, lease_generation,
                     lease_until
                 ) VALUES (?1, ?2, ?3, 'in_progress', 1, ?4)",
                rusqlite::params![
                    "scope",
                    "default-timestamps",
                    VALID_HTTP_FINGERPRINT,
                    1700000000_i64
                ],
            )
            .expect("insert row using timestamp defaults");
        let defaults: (String, String) = store
            .connection()
            .query_row(
                "SELECT created_at, updated_at
                 FROM http_idempotency_records
                 WHERE idempotency_key = 'default-timestamps'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read timestamp defaults");
        for value in [defaults.0, defaults.1] {
            assert_eq!(value.len(), 20, "canonical timestamps use whole seconds");
            assert!(value.ends_with('Z'), "canonical timestamps are UTC");
            let parsed =
                OffsetDateTime::parse(&value, &Rfc3339).expect("default timestamp is RFC3339");
            assert_eq!(parsed.offset(), UtcOffset::UTC);
            assert_eq!(parsed.nanosecond(), 0);
            assert_eq!(
                parsed.format(&Rfc3339).expect("format timestamp"),
                value,
                "default timestamp is strict canonical RFC3339"
            );
        }

        for (key, lease_until) in [
            ("numeric-9", 9_i64),
            ("numeric-10", 10),
            ("numeric-large", 1_700_000_000),
        ] {
            store
                .connection()
                .execute(
                    "INSERT INTO http_idempotency_records (
                         scope, idempotency_key, fingerprint, state, lease_generation,
                         lease_until
                     ) VALUES ('numeric', ?1, ?2, 'in_progress', 1, ?3)",
                    rusqlite::params![key, VALID_HTTP_FINGERPRINT, lease_until],
                )
                .expect("numeric lease row");
        }
        let ordered: Vec<i64> = store
            .connection()
            .prepare(
                "SELECT lease_until FROM http_idempotency_records
                 WHERE scope = 'numeric' ORDER BY lease_until",
            )
            .expect("numeric lease order query")
            .query_map([], |row| row.get(0))
            .expect("numeric lease order rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("numeric lease order");
        assert_eq!(ordered, vec![9, 10, 1_700_000_000]);
        let query_plan: Vec<String> = store
            .connection()
            .prepare(
                "EXPLAIN QUERY PLAN SELECT id FROM http_idempotency_records
                 WHERE lease_until <= 10 ORDER BY lease_until",
            )
            .expect("lease query plan")
            .query_map([], |row| row.get(3))
            .expect("lease query plan rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("lease query plan");
        assert!(
            query_plan
                .iter()
                .any(|detail| { detail.contains("idx_http_idempotency_records_lease_until") })
        );
    }

    #[test]
    fn http_idempotency_v14_migration_is_idempotent() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, MIGRATIONS).expect("apply v14 schema");
        run_migrations_with(&mut connection, MIGRATIONS).expect("reapply v14 schema");

        let migration_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM schema_migrations WHERE version = 14",
                [],
                |row| row.get(0),
            )
            .expect("v14 migration count");
        assert_eq!(migration_count, 1);
        let table_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'http_idempotency_records'",
                [],
                |row| row.get(0),
            )
            .expect("idempotency table count");
        assert_eq!(table_count, 1);
        let index_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_http_idempotency_records_lease_until'",
                [],
                |row| row.get(0),
            )
            .expect("lease index count");
        assert_eq!(index_count, 1);
    }

    #[test]
    fn http_idempotency_v14_constraints_reject_invalid_rows() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let insert = |scope: Option<&str>,
                      key: Option<&str>,
                      fingerprint: Option<&str>,
                      state: &str,
                      lease_generation: i64,
                      lease_until: Option<i64>,
                      response_status: Option<i64>,
                      response_content_type: Option<&str>,
                      response_body: Option<&[u8]>| {
            store.connection().execute(
                "INSERT INTO http_idempotency_records (
                     scope, idempotency_key, fingerprint, state, lease_generation,
                     lease_until, response_status, response_content_type, response_body
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    scope,
                    key,
                    fingerprint,
                    state,
                    lease_generation,
                    lease_until,
                    response_status,
                    response_content_type,
                    response_body,
                ],
            )
        };

        insert(
            Some("scope"),
            Some("in-progress"),
            Some(VALID_HTTP_FINGERPRINT),
            "in_progress",
            1,
            Some(1_700_000_000_i64),
            None,
            None,
            None,
        )
        .expect("valid in-progress row");
        insert(
            Some("scope"),
            Some("completed"),
            Some(VALID_HTTP_FINGERPRINT),
            "completed",
            1,
            Some(1_700_000_000_i64),
            Some(200),
            Some("application/json"),
            Some(b"{}"),
        )
        .expect("valid completed row");

        assert!(
            insert(
                Some("scope"),
                Some("bad-state"),
                Some(VALID_HTTP_FINGERPRINT),
                "unknown",
                1,
                Some(1_700_000_000_i64),
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("in-progress-status-only"),
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                1,
                Some(1_700_000_000_i64),
                Some(200),
                None,
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("in-progress-content-type-only"),
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                1,
                Some(1_700_000_000_i64),
                None,
                Some("application/json"),
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("in-progress-body-only"),
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                1,
                Some(1_700_000_000_i64),
                None,
                None,
                Some(b"{}"),
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("bad-generation"),
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                0,
                Some(1_700_000_000_i64),
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                None,
                Some("missing-scope"),
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                1,
                Some(1_700_000_000_i64),
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                None,
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                1,
                Some(1_700_000_000_i64),
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("missing-fingerprint"),
                None,
                "in_progress",
                1,
                Some(1_700_000_000_i64),
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("missing-lease"),
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                1,
                None,
                None,
                None,
                None,
            )
            .is_err()
        );
        for (label, scope, key, fingerprint) in [
            (
                "empty scope",
                Some(""),
                Some("empty-scope-key"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "whitespace-only scope",
                Some("   "),
                Some("whitespace-scope-key"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "empty idempotency key",
                Some("scope"),
                Some(""),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "whitespace-only idempotency key",
                Some("scope"),
                Some("   "),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "empty fingerprint",
                Some("empty-fingerprint-scope"),
                Some("empty-fingerprint-key"),
                Some(""),
            ),
            (
                "whitespace-only fingerprint",
                Some("whitespace-fingerprint-scope"),
                Some("whitespace-fingerprint-key"),
                Some("   "),
            ),
            (
                "tab-only scope",
                Some("\t"),
                Some("tab-scope-key"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "newline-only scope",
                Some("\n"),
                Some("newline-scope-key"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "non-breaking-space-only scope",
                Some("\u{00a0}"),
                Some("nbsp-scope-key"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "tab-only idempotency key",
                Some("scope"),
                Some("\t"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "newline-only idempotency key",
                Some("scope"),
                Some("\n"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "non-breaking-space-only idempotency key",
                Some("scope"),
                Some("\u{00a0}"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "tab-only fingerprint",
                Some("tab-fingerprint-scope"),
                Some("tab-fingerprint-key"),
                Some("\t"),
            ),
            (
                "newline-only fingerprint",
                Some("newline-fingerprint-scope"),
                Some("newline-fingerprint-key"),
                Some("\n"),
            ),
            (
                "non-breaking-space-only fingerprint",
                Some("nbsp-fingerprint-scope"),
                Some("nbsp-fingerprint-key"),
                Some("\u{00a0}"),
            ),
        ] {
            assert!(
                insert(
                    scope,
                    key,
                    fingerprint,
                    "in_progress",
                    1,
                    Some(1_700_000_000_i64),
                    None,
                    None,
                    None,
                )
                .is_err(),
                "{label} was accepted"
            );
        }
        assert!(
            insert(
                Some("scope"),
                Some("in-progress-response"),
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                1,
                Some(1_700_000_000_i64),
                Some(200),
                Some("application/json"),
                Some(b"{}"),
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("completed-status-null"),
                Some(VALID_HTTP_FINGERPRINT),
                "completed",
                1,
                Some(1_700_000_000_i64),
                None,
                Some("application/json"),
                Some(b"{}"),
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("completed-type-null"),
                Some(VALID_HTTP_FINGERPRINT),
                "completed",
                1,
                Some(1_700_000_000_i64),
                Some(200),
                None,
                Some(b"{}"),
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("completed-body-null"),
                Some(VALID_HTTP_FINGERPRINT),
                "completed",
                1,
                Some(1_700_000_000_i64),
                Some(200),
                Some("application/json"),
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("completed-status-low"),
                Some(VALID_HTTP_FINGERPRINT),
                "completed",
                1,
                Some(1_700_000_000_i64),
                Some(199),
                Some("application/json"),
                Some(b"{}"),
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("completed-status-high"),
                Some(VALID_HTTP_FINGERPRINT),
                "completed",
                1,
                Some(1_700_000_000_i64),
                Some(600),
                Some("application/json"),
                Some(b"{}"),
            )
            .is_err()
        );
        assert!(
            insert(
                Some("scope"),
                Some("completed-type-invalid"),
                Some(VALID_HTTP_FINGERPRINT),
                "completed",
                1,
                Some(1_700_000_000_i64),
                Some(200),
                Some("text/plain"),
                Some(b"{}"),
            )
            .is_err()
        );
        let nul_fingerprint = format!("{}\0!", "a".repeat(62));
        assert_eq!(nul_fingerprint.len(), 64);
        for (label, scope, key, fingerprint) in [
            (
                "embedded NUL in scope",
                Some("ok\0!"),
                Some("nul-scope-key"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "embedded NUL in idempotency key",
                Some("scope"),
                Some("ok\0!"),
                Some(VALID_HTTP_FINGERPRINT),
            ),
            (
                "embedded NUL with invalid fingerprint suffix",
                Some("nul-fingerprint-scope"),
                Some("nul-fingerprint-key"),
                Some(nul_fingerprint.as_str()),
            ),
        ] {
            assert!(
                insert(
                    scope,
                    key,
                    fingerprint,
                    "in_progress",
                    1,
                    Some(1_700_000_000_i64),
                    None,
                    None,
                    None,
                )
                .is_err(),
                "{label} was accepted"
            );
        }
        assert!(
            insert(
                Some("scope"),
                Some("in-progress"),
                Some(VALID_HTTP_FINGERPRINT),
                "in_progress",
                1,
                Some(1_700_000_000_i64),
                None,
                None,
                None,
            )
            .is_err()
        );

        let row_count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM http_idempotency_records", [], |row| {
                row.get(0)
            })
            .expect("row count");
        assert_eq!(row_count, 2);
    }

    #[test]
    fn http_idempotency_v14_constraints_reject_wrong_storage_classes() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let insert = |values: [Value; 11]| {
            store.connection().execute(
                "INSERT INTO http_idempotency_records (
                     scope, idempotency_key, fingerprint, state, lease_generation,
                     lease_until, response_status, response_content_type, response_body,
                     created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    values[0].clone(),
                    values[1].clone(),
                    values[2].clone(),
                    values[3].clone(),
                    values[4].clone(),
                    values[5].clone(),
                    values[6].clone(),
                    values[7].clone(),
                    values[8].clone(),
                    values[9].clone(),
                    values[10].clone(),
                ],
            )
        };
        let in_progress = || {
            [
                Value::Text("scope".to_owned()),
                Value::Text("key".to_owned()),
                Value::Text(VALID_HTTP_FINGERPRINT.to_owned()),
                Value::Text("in_progress".to_owned()),
                Value::Integer(1),
                Value::Integer(1_700_000_000),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Text("2025-01-01T00:00:00Z".to_owned()),
                Value::Text("2025-01-01T00:00:00Z".to_owned()),
            ]
        };
        let completed = || {
            [
                Value::Text("scope".to_owned()),
                Value::Text("key".to_owned()),
                Value::Text(VALID_HTTP_FINGERPRINT.to_owned()),
                Value::Text("completed".to_owned()),
                Value::Integer(1),
                Value::Integer(1_700_000_000),
                Value::Integer(200),
                Value::Text("application/json".to_owned()),
                Value::Blob(b"{}".to_vec()),
                Value::Text("2025-01-01T00:00:00Z".to_owned()),
                Value::Text("2025-01-01T00:00:00Z".to_owned()),
            ]
        };

        for (label, index, value) in [
            ("scope", 0, Value::Blob(b"scope".to_vec())),
            ("idempotency key", 1, Value::Blob(b"key".to_vec())),
            (
                "fingerprint",
                2,
                Value::Blob(VALID_HTTP_FINGERPRINT.as_bytes().to_vec()),
            ),
            ("state", 3, Value::Blob(b"in_progress".to_vec())),
            ("lease generation", 4, Value::Text("abc".to_owned())),
            ("lease until", 5, Value::Blob(b"1700000000".to_vec())),
            (
                "created at",
                9,
                Value::Blob(b"2025-01-01T00:00:00Z".to_vec()),
            ),
            (
                "updated at",
                10,
                Value::Blob(b"2025-01-01T00:00:00Z".to_vec()),
            ),
        ] {
            let mut values = in_progress();
            values[index] = value;
            assert!(insert(values).is_err(), "{label} accepted the wrong type");
        }

        for (label, index, value) in [
            ("response status", 6, Value::Real(200.5)),
            (
                "response content type",
                7,
                Value::Blob(b"application/json".to_vec()),
            ),
            ("response body", 8, Value::Text("{}".to_owned())),
        ] {
            let mut values = completed();
            values[index] = value;
            assert!(insert(values).is_err(), "{label} accepted the wrong type");
        }
    }

    #[test]
    fn http_idempotency_v14_constraints_reject_noncanonical_timestamps() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let insert = |key: &str, created_at: &str, updated_at: &str| {
            store.connection().execute(
                "INSERT INTO http_idempotency_records (
                     scope, idempotency_key, fingerprint, state, lease_generation,
                     lease_until, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'in_progress', 1, ?4, ?5, ?6)",
                rusqlite::params![
                    "scope",
                    key,
                    VALID_HTTP_FINGERPRINT,
                    1_700_000_000_i64,
                    created_at,
                    updated_at,
                ],
            )
        };
        let canonical = "2025-01-01T00:00:00Z";
        for (index, (label, created_at, updated_at)) in [
            ("legacy created_at", "2025-01-01 00:00:00", canonical),
            (
                "fractional created_at",
                "2025-01-01T00:00:00.000Z",
                canonical,
            ),
            ("legacy updated_at", canonical, "2025-01-01 00:00:00"),
            ("offset updated_at", canonical, "2025-01-01T00:00:00+00:00"),
            ("hour 24 created_at", "2025-01-01T24:00:00Z", canonical),
            ("month zero created_at", "2025-00-01T00:00:00Z", canonical),
            ("month 13 created_at", "2025-13-01T00:00:00Z", canonical),
            ("day zero created_at", "2025-01-00T00:00:00Z", canonical),
            ("day 32 created_at", "2025-01-32T00:00:00Z", canonical),
            (
                "day outside month created_at",
                "2025-02-29T00:00:00Z",
                canonical,
            ),
            ("minute 60 created_at", "2025-01-01T00:60:00Z", canonical),
            ("second 60 created_at", "2025-01-01T00:00:60Z", canonical),
            ("hour 24 updated_at", canonical, "2025-01-01T24:00:00Z"),
            ("minute 60 updated_at", canonical, "2025-01-01T00:60:00Z"),
            ("second 60 updated_at", canonical, "2025-01-01T00:00:60Z"),
        ]
        .into_iter()
        .enumerate()
        {
            if label.contains("hour")
                || label.contains("month")
                || label.contains("day")
                || label.contains("minute")
                || label.contains("second")
            {
                let value = if created_at != canonical {
                    created_at
                } else {
                    updated_at
                };
                assert!(
                    OffsetDateTime::parse(value, &Rfc3339).is_err(),
                    "{label} must be rejected by strict RFC3339 parsing"
                );
            }
            let key = format!("timestamp-{index}");
            assert!(
                insert(&key, created_at, updated_at).is_err(),
                "{label} was accepted"
            );
        }
        let row_count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM http_idempotency_records", [], |row| {
                row.get(0)
            })
            .expect("row count");
        assert_eq!(row_count, 0);
    }

    #[test]
    fn http_idempotency_v14_constraints_reject_invalid_lease_until() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let insert = |key: &str, lease_until: Value| {
            store.connection().execute(
                "INSERT INTO http_idempotency_records (
                     scope, idempotency_key, fingerprint, state, lease_generation,
                     lease_until
                 ) VALUES ('scope', ?1, ?2, 'in_progress', 1, ?3)",
                rusqlite::params![key, VALID_HTTP_FINGERPRINT, lease_until],
            )
        };

        for (index, (label, lease_until)) in [
            ("negative", Value::Integer(-1)),
            ("real", Value::Real(1_700_000_000.5)),
            ("non-numeric text", Value::Text("1700000000x".to_owned())),
            ("blob", Value::Blob(b"1700000000".to_vec())),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                insert(&format!("invalid-lease-{index}"), lease_until).is_err(),
                "{label} lease_until was accepted"
            );
        }

        for (key, lease_until) in [
            ("zero", 0_i64),
            ("positive", 1_700_000_000),
            ("maximum", i64::MAX),
        ] {
            insert(key, Value::Integer(lease_until)).expect("valid lease_until was rejected");
        }

        let stored: Vec<i64> = store
            .connection()
            .prepare(
                "SELECT lease_until FROM http_idempotency_records
                 ORDER BY idempotency_key",
            )
            .expect("lease values query")
            .query_map([], |row| row.get(0))
            .expect("lease values")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("lease values rows");
        assert_eq!(stored, vec![i64::MAX, 1_700_000_000, 0]);
    }

    #[test]
    fn http_idempotency_v14_reopen_preserves_rows() {
        let database = TempDatabase::new();
        let v13_migrations = &MIGRATIONS[..MIGRATIONS
            .iter()
            .position(|migration| migration.version == 14)
            .expect("v14 migration")];
        {
            let mut connection = Connection::open(&database.path).expect("open v13 database");
            apply_sqlcipher_key(&connection, DATABASE_KEY).expect("apply SQLCipher key");
            verify_sqlcipher(&connection).expect("verify SQLCipher");
            run_migrations_with(&mut connection, v13_migrations).expect("apply v13 schema");
            connection
                .execute(
                    "UPDATE configuration SET owner_email = ?1 WHERE id = 1",
                    ["owner@example.test"],
                )
                .expect("seed v13 configuration");
            connection
                .execute(
                    "INSERT INTO replay_nonces (nonce, consumed_at, expires_at)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![
                        "reopen-nonce",
                        "2025-01-01T00:00:00Z",
                        "2025-01-01T00:01:00Z"
                    ],
                )
                .expect("seed v13 replay nonce");
        }

        let v13_table_count: i64 = {
            let connection = Connection::open(&database.path).expect("open v13 snapshot");
            apply_sqlcipher_key(&connection, DATABASE_KEY).expect("apply SQLCipher key");
            verify_sqlcipher(&connection).expect("verify SQLCipher");
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get(0),
                )
                .expect("v13 table count")
        };
        let snapshot = |store: &PaStore| {
            let table_count: i64 = store
                .connection()
                .query_row(
                    "SELECT count(*) FROM sqlite_master
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get(0),
                )
                .expect("table count");
            let index_count: i64 = store
                .connection()
                .query_row(
                    "SELECT count(*) FROM sqlite_master
                     WHERE type = 'index'",
                    [],
                    |row| row.get(0),
                )
                .expect("index count");
            let configuration_rows: i64 = store
                .connection()
                .query_row("SELECT count(*) FROM configuration", [], |row| row.get(0))
                .expect("configuration row count");
            let replay_rows: i64 = store
                .connection()
                .query_row("SELECT count(*) FROM replay_nonces", [], |row| row.get(0))
                .expect("replay row count");
            let idempotency_rows: i64 = store
                .connection()
                .query_row("SELECT count(*) FROM http_idempotency_records", [], |row| {
                    row.get(0)
                })
                .expect("idempotency row count");
            let migration_rows: i64 = store
                .connection()
                .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .expect("migration row count");
            (
                table_count,
                index_count,
                configuration_rows,
                replay_rows,
                idempotency_rows,
                migration_rows,
            )
        };

        let mut snapshots = Vec::new();
        for reopen in 0..3 {
            let store = PaStore::open(&database.path, DATABASE_KEY).expect("open v14 store");
            let owner_email: String = store
                .connection()
                .query_row(
                    "SELECT owner_email FROM configuration WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .expect("preserved configuration");
            assert_eq!(owner_email, "owner@example.test");
            let nonce: String = store
                .connection()
                .query_row(
                    "SELECT nonce FROM replay_nonces WHERE nonce = 'reopen-nonce'",
                    [],
                    |row| row.get(0),
                )
                .expect("preserved replay nonce");
            assert_eq!(nonce, "reopen-nonce");
            if reopen == 0 {
                store
                    .connection()
                    .execute(
                        "INSERT INTO http_idempotency_records (
                             scope, idempotency_key, fingerprint, state, lease_generation,
                             lease_until
                         ) VALUES (?1, ?2, ?3, 'in_progress', 1, ?4)",
                        rusqlite::params![
                            "scope",
                            "key",
                            VALID_HTTP_FINGERPRINT,
                            1_700_000_000_i64
                        ],
                    )
                    .expect("insert idempotency row");
            }
            let idempotency_row: (String, String, String) = store
                .connection()
                .query_row(
                    "SELECT scope, idempotency_key, fingerprint
                     FROM http_idempotency_records",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("preserved idempotency row");
            assert_eq!(
                idempotency_row,
                (
                    "scope".to_owned(),
                    "key".to_owned(),
                    VALID_HTTP_FINGERPRINT.to_owned()
                )
            );
            snapshots.push(snapshot(&store));
        }

        assert_eq!(snapshots[0].0, v13_table_count + 2);
        assert_eq!(snapshots[0].2, 1);
        assert_eq!(snapshots[0].3, 1);
        assert_eq!(snapshots[0].4, 1);
        assert_eq!(snapshots[0].5, CURRENT_SCHEMA_VERSION);
        assert_eq!(snapshots[0], snapshots[1]);
        assert_eq!(snapshots[1], snapshots[2]);
    }

    #[test]
    fn configuration_version_survives_file_reopen() {
        let database = TempDatabase::new();
        let v12_migrations = &MIGRATIONS[..MIGRATIONS
            .iter()
            .position(|migration| migration.version == 13)
            .unwrap_or(MIGRATIONS.len())];
        let expected_configuration = {
            let mut connection = Connection::open(&database.path).expect("open v12 database");
            apply_sqlcipher_key(&connection, DATABASE_KEY).expect("apply SQLCipher key");
            verify_sqlcipher(&connection).expect("verify SQLCipher");
            run_migrations_with(&mut connection, v12_migrations).expect("apply v12 schema");
            connection
                .execute(
                    "UPDATE configuration
                     SET owner_timezone = 'Australia/Sydney',
                         owner_email = 'owner@example.com',
                         working_days = 'monday,wednesday',
                         meeting_buffer_minutes = 15
                     WHERE id = 1",
                    [],
                )
                .expect("seed existing configuration values");
            let mut statement = connection
                .prepare(
                    "SELECT owner_timezone, owner_email, owner_phone, working_days,
                            working_window_start, working_window_end, minimum_notice_minutes,
                            booking_horizon_days, meeting_buffer_minutes, retention_days,
                            task_duration_bill_minutes, task_duration_callback_minutes,
                            task_duration_reading_minutes, task_duration_email_reply_minutes,
                            task_duration_preparation_minutes, email_triage_model,
                            created_at, updated_at
                     FROM configuration WHERE id = 1",
                )
                .expect("configuration snapshot query");
            statement
                .query_row([], |row| {
                    (0..18)
                        .map(|column| row.get::<_, Value>(column))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .expect("configuration snapshot")
        };

        let store = PaStore::open(&database.path, DATABASE_KEY).expect("upgrade v12 store");
        let schema_version: i64 = store
            .connection()
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("schema version");
        assert_eq!(schema_version, CURRENT_SCHEMA_VERSION);
        let migration_count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration count");
        assert_eq!(migration_count, CURRENT_SCHEMA_VERSION);
        let version: i64 = store
            .connection()
            .query_row(
                "SELECT version FROM configuration WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("configuration version");
        assert_eq!(version, 1);
        let actual_configuration = store
            .connection()
            .prepare(
                "SELECT owner_timezone, owner_email, owner_phone, working_days,
                        working_window_start, working_window_end, minimum_notice_minutes,
                        booking_horizon_days, meeting_buffer_minutes, retention_days,
                        task_duration_bill_minutes, task_duration_callback_minutes,
                        task_duration_reading_minutes, task_duration_email_reply_minutes,
                        task_duration_preparation_minutes, email_triage_model,
                        created_at, updated_at
                 FROM configuration WHERE id = 1",
            )
            .expect("configuration snapshot query")
            .query_row([], |row| {
                (0..18)
                    .map(|column| row.get::<_, Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .expect("configuration snapshot");
        assert_eq!(actual_configuration, expected_configuration);

        store
            .connection()
            .execute("UPDATE configuration SET version = 7 WHERE id = 1", [])
            .expect("set configuration version");
        drop(store);

        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen store");
        let reopened_version: i64 = reopened
            .connection()
            .query_row(
                "SELECT version FROM configuration WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("reopened configuration version");
        assert_eq!(reopened_version, 7);
    }

    #[test]
    fn configuration_version_migration_is_idempotent() {
        let mut connection = keyed_connection_for_migration_test();
        let v12_migrations = &MIGRATIONS[..MIGRATIONS
            .iter()
            .position(|migration| migration.version == 13)
            .unwrap_or(MIGRATIONS.len())];
        run_migrations_with(&mut connection, v12_migrations).expect("apply v12 schema");
        run_migrations_with(&mut connection, MIGRATIONS).expect("apply v13 schema");
        run_migrations_with(&mut connection, MIGRATIONS).expect("reapply v13 schema");

        let migration_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM schema_migrations WHERE version = 13",
                [],
                |row| row.get(0),
            )
            .expect("v13 migration count");
        assert_eq!(migration_count, 1);
        let version_column_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM pragma_table_info('configuration') WHERE name = 'version'",
                [],
                |row| row.get(0),
            )
            .expect("configuration version column count");
        assert_eq!(version_column_count, 1);

        connection
            .execute("UPDATE configuration SET version = 7 WHERE id = 1", [])
            .expect("set configuration version");
        for invalid in [
            Value::Integer(0),
            Value::Integer(-1),
            Value::Real(1.5),
            Value::Text("text".to_owned()),
        ] {
            assert!(
                connection
                    .execute(
                        "UPDATE configuration SET version = ?1 WHERE id = 1",
                        [&invalid],
                    )
                    .is_err()
            );
            let version: i64 = connection
                .query_row(
                    "SELECT version FROM configuration WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .expect("valid configuration version remains");
            assert_eq!(version, 7);
        }
    }

    #[test]
    fn message_types_validate_closed_values_and_safe_summary_text() {
        for (text, provider) in [
            ("voice", MessageProvider::Voice),
            ("outlook", MessageProvider::Outlook),
            ("gmail", MessageProvider::Gmail),
        ] {
            assert_eq!(
                MessageProvider::from_storage(text).expect("provider"),
                provider
            );
            assert_eq!(provider.as_str(), text);
        }
        for (text, state) in [
            ("recorded", MessageTriageState::Recorded),
            ("unprocessed", MessageTriageState::Unprocessed),
            ("actionable", MessageTriageState::Actionable),
            ("ambiguous", MessageTriageState::Ambiguous),
            ("ignored", MessageTriageState::Ignored),
            ("scheduled", MessageTriageState::Scheduled),
        ] {
            assert_eq!(
                MessageTriageState::from_storage(text).expect("triage state"),
                state
            );
            assert_eq!(state.as_str(), text);
        }
        assert!(MessageProvider::from_storage("provider-secret").is_err());
        assert!(MessageTriageState::from_storage("state-secret").is_err());

        let summary = MessageSummary::new("Caller's requested follow-up").expect("summary");
        assert_eq!(summary.as_str(), "Caller's requested follow-up");
        assert!(validate_message_summary(" ".to_owned()).is_err());
        assert!(validate_message_summary("\u{2003}".to_owned()).is_err());
        assert!(validate_message_summary(" \u{2003}\u{00a0}\u{3000}".to_owned()).is_err());
        assert!(validate_message_summary("Résumé \u{2003}follow-up".to_owned()).is_ok());
        assert!(validate_message_summary("summary\nsecret".to_owned()).is_err());
        assert!(validate_message_summary("x".repeat(MAX_MESSAGE_SUMMARY_LENGTH + 1)).is_err());
        assert!(validate_message_summary("é".repeat(MAX_MESSAGE_SUMMARY_LENGTH / 2 + 1)).is_err());
        assert!(validate_message_id("id with spaces".to_owned()).is_err());
        assert!(validate_message_id("id\0secret".to_owned()).is_err());
        assert!(validate_message_id("x".repeat(MAX_MESSAGE_ID_LENGTH + 1)).is_err());
        assert!(validate_message_subject(Some(" ".to_owned())).is_err());
        assert!(validate_message_subject(Some("\u{2003}".to_owned())).is_err());
        assert!(validate_message_subject(Some(" \u{2003}\u{00a0}\u{3000}".to_owned())).is_err());
        assert!(validate_message_subject(Some("Réunion \u{2003}tomorrow".to_owned())).is_ok());
        assert!(validate_message_subject(Some("subject\tvalue".to_owned())).is_err());
        assert!(
            validate_message_subject(Some("x".repeat(MAX_MESSAGE_SUBJECT_LENGTH + 1))).is_err()
        );
        assert!(validate_message_sender(Some(" ".to_owned())).is_err());
        assert!(validate_message_sender(Some("\u{2003}".to_owned())).is_err());
        assert!(validate_message_sender(Some(" \u{2003}\u{00a0}\u{3000}".to_owned())).is_err());
        assert!(validate_message_sender(Some("José <jose@example.com>".to_owned())).is_ok());
        assert!(validate_message_sender(Some("sender\0value".to_owned())).is_err());
        assert!(validate_message_sender(Some("x".repeat(MAX_MESSAGE_SENDER_LENGTH + 1))).is_err());
    }

    #[test]
    fn empty_legacy_messages_rebuild_to_safe_schema_and_migration_is_idempotent() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..5]).expect("apply legacy schema");
        run_migrations_with(&mut connection, MIGRATIONS).expect("rebuild messages");
        run_migrations_with(&mut connection, MIGRATIONS).expect("idempotent rebuild");

        let version: i64 = connection
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("schema version");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let columns = connection
            .prepare("PRAGMA table_info(messages)")
            .expect("messages schema")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("messages columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("messages column names");
        assert_eq!(
            columns,
            vec![
                "id",
                "idempotency_key",
                "source_id",
                "provider",
                "provider_message_id",
                "summary",
                "subject",
                "sender",
                "received_at",
                "triage_state",
                "created_at",
                "updated_at",
            ]
        );
        assert!(
            !columns
                .iter()
                .any(|column| column == "body" || column == "transcript")
        );
    }

    #[test]
    fn legacy_message_rows_abort_rebuild_atomically_without_echoing_values() {
        const SECRET: &str = "legacy-message-provider-secret";
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..5]).expect("apply legacy schema");
        connection
            .execute(
                "INSERT INTO messages (
                    source_id, provider, provider_message_id, subject, sender, received_at,
                    triage_state
                 ) VALUES ('legacy-source', 'voice', 'legacy-provider-id', 'legacy subject',
                    'legacy@example.com', '2025-01-02 03:04:05', 'recorded')",
                [],
            )
            .expect("seed legacy message");
        connection
            .execute("UPDATE messages SET provider_message_id = ?1", [SECRET])
            .expect("set sentinel");

        let error = run_migrations_with(&mut connection, MIGRATIONS)
            .expect_err("legacy message rows must fail closed");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "message"
            }
        ));
        assert!(!error.to_string().contains(SECRET));
        assert!(!format!("{error:?}").contains(SECRET));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("version unchanged"),
            5
        );
        let stored: String = connection
            .query_row("SELECT provider_message_id FROM messages", [], |row| {
                row.get(0)
            })
            .expect("legacy row preserved");
        assert_eq!(stored, SECRET);
        assert!(connection.prepare("SELECT summary FROM messages").is_err());
    }

    #[test]
    fn empty_legacy_tasks_rebuild_to_safe_schema_and_migration_is_idempotent() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..6]).expect("apply legacy schema");
        run_migrations_with(&mut connection, MIGRATIONS).expect("rebuild tasks");
        run_migrations_with(&mut connection, MIGRATIONS).expect("idempotent rebuild");

        let version: i64 = connection
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("schema version");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let columns = connection
            .prepare("PRAGMA table_info(tasks)")
            .expect("tasks schema")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("tasks columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("task column names");
        assert_eq!(
            columns,
            vec![
                "id",
                "idempotency_key",
                "source_id",
                "message_id",
                "title",
                "kind",
                "duration_minutes",
                "due_at",
                "status",
                "created_at",
                "updated_at",
            ]
        );
        assert!(!columns.iter().any(|column| {
            column == "body" || column == "email_body" || column == "transcript"
        }));
        assert_named_index(
            &PaStore { connection },
            "idx_tasks_status_due_at",
            "tasks",
            &["status", "due_at"],
        );
    }

    #[test]
    fn legacy_task_rows_abort_rebuild_atomically_without_echoing_values() {
        const SECRET: &str = "legacy-task-title-secret";
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..6]).expect("apply legacy schema");
        connection
            .execute(
                "INSERT INTO tasks (
                    idempotency_key, source_id, title, kind, duration_minutes
                 ) VALUES ('legacy-task-key', 'legacy-task-source', ?1, 'callback', 15)",
                [SECRET],
            )
            .expect("seed legacy task");

        let error = run_migrations_with(&mut connection, MIGRATIONS)
            .expect_err("legacy task rows must fail closed");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid { resource: "task" }
        ));
        assert!(!error.to_string().contains(SECRET));
        assert!(!format!("{error:?}").contains(SECRET));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("version unchanged"),
            6
        );
        let stored: String = connection
            .query_row("SELECT title FROM tasks", [], |row| row.get(0))
            .expect("legacy row preserved");
        assert_eq!(stored, SECRET);
        assert!(connection.prepare("SELECT message_id FROM tasks").is_ok());
    }

    #[test]
    fn task_schema_accepts_every_kind_and_rejects_invalid_contract_values() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let message = store
            .record_message(
                "task-message-key",
                "task-message-source",
                MessageProvider::Outlook,
                "task-message-provider-id",
                MessageSummary::new("Invoice summary").expect("summary"),
                Some("Invoice".to_owned()),
                Some("sender@example.com".to_owned()),
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
            )
            .expect("message");
        let insert = |idempotency_key: &str,
                      source_id: &str,
                      message_id: i64,
                      title: &str,
                      kind: &str,
                      duration_minutes: i64,
                      due_at: Option<&str>,
                      status: Option<&str>| {
            store.connection().execute(
                "INSERT INTO tasks (
                    idempotency_key, source_id, message_id, title, kind,
                    duration_minutes, due_at, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, COALESCE(?8, 'pending'))",
                rusqlite::params![
                    idempotency_key,
                    source_id,
                    message_id,
                    title,
                    kind,
                    duration_minutes,
                    due_at,
                    status,
                ],
            )
        };

        for (index, kind) in ["bill", "callback", "reading", "email_reply", "preparation"]
            .into_iter()
            .enumerate()
        {
            insert(
                &format!("task-key-{index}"),
                &format!("task-source-{index}"),
                message.id(),
                "Review the invoice",
                kind,
                [15, 15, 30, 30, 60][index],
                (index == 4).then_some("2025-01-03T03:04:05Z"),
                None,
            )
            .expect("valid task");
        }
        insert(
            "task-explicit-key",
            "task-explicit-source",
            message.id(),
            "Prepare a reply",
            "email_reply",
            120,
            None,
            Some("proposed"),
        )
        .expect("explicit duration/state task");

        for (key, source, message_id, title, kind, duration, due_at, status) in [
            (
                "missing-message",
                "missing-message-source",
                999_999,
                "Task",
                "callback",
                15,
                None,
                None,
            ),
            (
                "bad-state",
                "bad-state-source",
                message.id(),
                "Task",
                "callback",
                15,
                None,
                Some("unknown"),
            ),
            (
                "bad-kind",
                "bad-kind-source",
                message.id(),
                "Task",
                "unknown",
                15,
                None,
                None,
            ),
            (
                "zero-duration",
                "zero-duration-source",
                message.id(),
                "Task",
                "callback",
                0,
                None,
                None,
            ),
            (
                "large-duration",
                "large-duration-source",
                message.id(),
                "Task",
                "callback",
                MAX_TASK_DURATION_MINUTES + 1,
                None,
                None,
            ),
            (
                "fractional-due",
                "fractional-due-source",
                message.id(),
                "Task",
                "callback",
                15,
                Some("2025-01-03T03:04:05.000Z"),
                None,
            ),
            (
                "bad-title-control",
                "bad-title-control-source",
                message.id(),
                "Task\nsecret",
                "callback",
                15,
                None,
                None,
            ),
            (
                "bad-title-blank",
                "bad-title-blank-source",
                message.id(),
                "\u{2003}",
                "callback",
                15,
                None,
                None,
            ),
        ] {
            assert!(
                insert(
                    key, source, message_id, title, kind, duration, due_at, status
                )
                .is_err()
            );
        }
        assert!(
            insert(
                "task-key-0",
                "new-source-for-duplicate-key",
                message.id(),
                "Task",
                "callback",
                15,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                "new-key-for-duplicate-source",
                "task-source-0",
                message.id(),
                "Task",
                "callback",
                15,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            insert(
                "blob-id-key",
                "blob-id-source",
                message.id(),
                "Task",
                "callback",
                15,
                None,
                None,
            )
            .is_ok()
        );
        store
            .connection()
            .execute(
                "DELETE FROM tasks WHERE idempotency_key = 'blob-id-key'",
                [],
            )
            .expect("remove temporary task");
        assert!(
            store
                .connection()
                .execute(
                    "INSERT INTO tasks (
                    idempotency_key, source_id, message_id, title, kind, duration_minutes
                 ) VALUES (CAST(?1 AS BLOB), 'blob-task-source', ?2, 'Task', 'callback', 15)",
                    rusqlite::params!["blob-task-key", message.id()],
                )
                .is_err()
        );
        assert!(
            store
                .connection()
                .execute(
                    "INSERT INTO tasks (
                    idempotency_key, source_id, message_id, title, kind, duration_minutes
                 ) VALUES ('nul-task-key', 'nul-task-source', ?1, 'Task', 'callback', 15)",
                    rusqlite::params![message.id()],
                )
                .is_ok()
        );
        store
            .connection()
            .execute(
                "DELETE FROM tasks WHERE idempotency_key = 'nul-task-key'",
                [],
            )
            .expect("remove temporary task");
        assert!(
            store
                .connection()
                .execute(
                    "INSERT INTO tasks (
                    idempotency_key, source_id, message_id, title, kind, duration_minutes
                 ) VALUES ('nul-task-key', 'nul-task-source', ?1, 'Task\0secret', 'callback', 15)",
                    rusqlite::params![message.id()],
                )
                .is_err()
        );
        assert!(
            store
                .connection()
                .execute("DELETE FROM messages WHERE id = ?1", [message.id()],)
                .is_err()
        );
    }

    #[test]
    fn messages_accept_valid_initial_rows_and_reject_invalid_contract_values() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let connection = store.connection();
        let insert = |idempotency_key: &str,
                      source_id: &str,
                      provider: &str,
                      provider_message_id: &str,
                      summary: &str,
                      subject: Option<&str>,
                      sender: Option<&str>,
                      received_at: &str,
                      triage_state: &str| {
            connection.execute(
                "INSERT INTO messages (
                    idempotency_key, source_id, provider, provider_message_id, summary,
                    subject, sender, received_at, triage_state
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    idempotency_key,
                    source_id,
                    provider,
                    provider_message_id,
                    summary,
                    subject,
                    sender,
                    received_at,
                    triage_state,
                ],
            )
        };

        insert(
            "voice-key",
            "voice-source",
            "voice",
            "voice-message",
            "Caller requested a callback",
            None,
            None,
            "2025-01-02T03:04:05Z",
            "recorded",
        )
        .expect("valid voice message");
        insert(
            "outlook-key",
            "outlook-source",
            "outlook",
            "outlook-message",
            "Invoice needs review",
            Some("Invoice"),
            Some("sender@example.com"),
            "2025-01-02T03:04:06Z",
            "unprocessed",
        )
        .expect("valid email message");

        assert!(
            insert(
                "bad-provider-key",
                "bad-provider-source",
                "slack",
                "bad-provider-message",
                "Summary",
                None,
                None,
                "2025-01-02T03:04:07Z",
                "unprocessed"
            )
            .is_err()
        );
        assert!(
            insert(
                "bad-state-key",
                "bad-state-source",
                "voice",
                "bad-state-message",
                "Summary",
                None,
                None,
                "2025-01-02T03:04:08Z",
                "unprocessed"
            )
            .is_err()
        );
        assert!(
            insert(
                "bad-email-state-key",
                "bad-email-state-source",
                "gmail",
                "bad-email-state-message",
                "Summary",
                None,
                None,
                "2025-01-02T03:04:09Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "voice-key",
                "other-source",
                "voice",
                "other-message",
                "Summary",
                None,
                None,
                "2025-01-02T03:04:10Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "other-key",
                "other-source",
                "voice",
                "voice-message",
                "Summary",
                None,
                None,
                "2025-01-02T03:04:11Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "blank-summary-key",
                "blank-summary-source",
                "voice",
                "blank-summary-message",
                " ",
                None,
                None,
                "2025-01-02T03:04:12Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "control-summary-key",
                "control-summary-source",
                "voice",
                "control-summary-message",
                "Summary\nsecret",
                None,
                None,
                "2025-01-02T03:04:13Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "oversized-summary-key",
                "oversized-summary-source",
                "voice",
                "oversized-summary-message",
                &"x".repeat(MAX_MESSAGE_SUMMARY_LENGTH + 1),
                None,
                None,
                "2025-01-02T03:04:14Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "nul-summary-key",
                "nul-summary-source",
                "voice",
                "nul-summary-message",
                "Summary\0secret",
                None,
                None,
                "2025-01-02T03:04:15Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "subject-key",
                "subject-source",
                "voice",
                "subject-message",
                "Summary",
                Some(" "),
                None,
                "2025-01-02T03:04:16Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "sender-key",
                "sender-source",
                "voice",
                "sender-message",
                "Summary",
                None,
                Some("sender\0value"),
                "2025-01-02T03:04:17Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "timestamp-key",
                "timestamp-source",
                "voice",
                "timestamp-message",
                "Summary",
                None,
                None,
                "2025-01-02T03:04:05.000Z",
                "recorded"
            )
            .is_err()
        );
        assert!(
            insert(
                "blob-key",
                "blob-source",
                "voice",
                "blob-message",
                "Summary",
                None,
                None,
                "2025-01-02T03:04:18Z",
                "recorded"
            )
            .is_ok()
        );
        connection
            .execute(
                "DELETE FROM messages WHERE idempotency_key = 'blob-key'",
                [],
            )
            .expect("remove temporary valid row");
        assert!(
            connection
                .execute(
                    "INSERT INTO messages (
                    idempotency_key, source_id, provider, provider_message_id, summary,
                    received_at, triage_state
                 ) VALUES (CAST(?1 AS BLOB), 'blob-source-2', 'voice', 'blob-message-2',
                    'Summary', '2025-01-02T03:04:19Z', 'recorded')",
                    ["blob-key-2"],
                )
                .is_err()
        );
        let row_count: i64 = connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .expect("valid rows remain");
        assert_eq!(row_count, 2);
    }

    #[test]
    fn message_schema_rejects_unicode_whitespace_only_fields_but_accepts_unicode_text() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let connection = store.connection();
        let insert = |suffix: &str, summary: &str, subject: &str, sender: &str| {
            connection.execute(
                "INSERT INTO messages (
                    idempotency_key, source_id, provider, provider_message_id, summary,
                    subject, sender, received_at, triage_state
                 ) VALUES (?1, ?2, 'outlook', ?3, ?4, ?5, ?6, '2025-01-02T03:04:05Z', 'unprocessed')",
                rusqlite::params![
                    format!("unicode-{suffix}-key"),
                    format!("unicode-{suffix}-source"),
                    format!("unicode-{suffix}-message"),
                    summary,
                    subject,
                    sender,
                ],
            )
        };

        assert!(
            insert(
                "valid",
                "Résumé \u{2003}follow-up",
                "Réunion \u{2003}tomorrow",
                "José <jose@example.com>",
            )
            .is_ok()
        );
        assert!(insert("summary-only", "\u{2003}", "Subject", "sender@example.com",).is_err());
        assert!(
            insert(
                "summary-mixed",
                " \u{2003}\u{00a0}\u{3000}",
                "Subject",
                "sender@example.com",
            )
            .is_err()
        );
        assert!(insert("subject-only", "Summary", "\u{2003}", "sender@example.com",).is_err());
        assert!(
            insert(
                "subject-mixed",
                "Summary",
                " \u{2003}\u{00a0}\u{3000}",
                "sender@example.com",
            )
            .is_err()
        );
        assert!(insert("sender-only", "Summary", "Subject", "\u{2003}",).is_err());
        assert!(
            insert(
                "sender-mixed",
                "Summary",
                "Subject",
                " \u{2003}\u{00a0}\u{3000}",
            )
            .is_err()
        );
        let row_count: i64 = connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .expect("valid Unicode row remains");
        assert_eq!(row_count, 1);
    }

    #[test]
    fn message_repository_records_derived_state_and_retries_stably() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let summary = MessageSummary::new("Caller requested a callback").expect("summary");

        let voice = store
            .record_message(
                "voice-key",
                "voice-source",
                MessageProvider::Voice,
                "voice-message",
                summary.clone(),
                None,
                None,
                received_at,
            )
            .expect("voice message");
        assert_eq!(voice.triage_state(), MessageTriageState::Recorded);
        assert_eq!(voice.summary(), &summary);
        assert_eq!(voice.received_at(), received_at);

        let email = store
            .record_message(
                "email-key",
                "email-source",
                MessageProvider::Outlook,
                "email-message",
                MessageSummary::new("Invoice needs review").expect("summary"),
                Some("Invoice".to_owned()),
                Some("sender@example.com".to_owned()),
                received_at,
            )
            .expect("email message");
        assert_eq!(email.triage_state(), MessageTriageState::Unprocessed);
        assert_eq!(email.subject(), Some("Invoice"));
        assert_eq!(email.sender(), Some("sender@example.com"));

        assert_eq!(
            store
                .record_message(
                    "voice-key",
                    "voice-source",
                    MessageProvider::Voice,
                    "voice-message",
                    summary,
                    None,
                    None,
                    received_at,
                )
                .expect("identical retry"),
            voice
        );
        assert_eq!(store.list_messages(None, 10).expect("list").len(), 2);
    }

    #[test]
    fn message_repository_conflicts_preserve_rows_and_support_equivalent_lookups() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let first = store
            .record_message(
                "message-key",
                "message-source",
                MessageProvider::Gmail,
                "provider-message",
                MessageSummary::new("Invoice needs review").expect("summary"),
                Some("Invoice".to_owned()),
                Some("sender@example.com".to_owned()),
                received_at,
            )
            .expect("message");
        let original_created_at = first.created_at().to_owned();
        let original_updated_at = first.updated_at().to_owned();

        for (key, source, provider, provider_message_id, summary) in [
            (
                "message-key",
                "changed-source",
                MessageProvider::Gmail,
                "provider-message",
                "Invoice needs review",
            ),
            (
                "changed-key",
                "message-source",
                MessageProvider::Gmail,
                "provider-message",
                "Invoice needs review",
            ),
            (
                "changed-key-2",
                "changed-source-2",
                MessageProvider::Gmail,
                "provider-message",
                "Changed summary",
            ),
        ] {
            let error = store
                .record_message(
                    key,
                    source,
                    provider,
                    provider_message_id,
                    MessageSummary::new(summary).expect("summary"),
                    Some("Invoice".to_owned()),
                    Some("sender@example.com".to_owned()),
                    received_at,
                )
                .expect_err("identity reuse must conflict");
            assert!(matches!(
                error,
                StoreError::Conflict {
                    resource: "message"
                }
            ));
            assert!(!error.to_string().contains("Changed summary"));
        }

        assert_eq!(store.load_message_by_id(first.id()).expect("id"), first);
        assert_eq!(
            store
                .load_message_by_idempotency_key("message-key")
                .expect("key"),
            first
        );
        assert_eq!(
            store
                .load_message_by_source_id("message-source")
                .expect("source"),
            first
        );
        assert_eq!(
            store
                .load_message_by_provider_message_id(MessageProvider::Gmail, "provider-message")
                .expect("provider identity"),
            first
        );
        assert_eq!(first.created_at(), original_created_at);
        assert_eq!(first.updated_at(), original_updated_at);
        assert_eq!(store.list_messages(None, 10).expect("list").len(), 1);
    }

    #[test]
    fn message_listing_is_deterministic_and_rejects_invalid_pagination() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        for index in 1..=3 {
            store
                .record_message(
                    format!("page-key-{index}"),
                    format!("page-source-{index}"),
                    MessageProvider::Voice,
                    format!("page-message-{index}"),
                    MessageSummary::new(format!("Summary {index}")).expect("summary"),
                    None,
                    None,
                    received_at,
                )
                .expect("message");
        }
        let all = store.list_messages(None, 10).expect("all");
        assert_eq!(all.len(), 3);
        let page = store.list_messages(Some(all[0].id()), 2).expect("page");
        assert_eq!(
            page.iter().map(StoredMessage::id).collect::<Vec<_>>(),
            vec![all[1].id(), all[2].id()]
        );
        assert!(store.list_messages(Some(0), 2).is_err());
        assert!(store.list_messages(None, 0).is_err());
        assert!(
            store
                .list_messages(None, MAX_MESSAGE_LIST_LIMIT + 1)
                .is_err()
        );
        assert_eq!(store.list_messages(None, 10).expect("unchanged").len(), 3);
    }

    #[test]
    fn message_reads_fail_closed_for_corrupt_values_and_redact_content() {
        const PRIVATE_SUMMARY: &str = "private-message-summary";
        const PRIVATE_SOURCE: &str = "private-message-source";
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let message = store
            .record_message(
                "corruption-key",
                PRIVATE_SOURCE,
                MessageProvider::Voice,
                "corruption-provider-message",
                MessageSummary::new(PRIVATE_SUMMARY).expect("summary"),
                Some("private subject".to_owned()),
                Some("private@example.com".to_owned()),
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
            )
            .expect("message");
        store
            .connection()
            .pragma_update(None, "ignore_check_constraints", true)
            .expect("disable checks");
        store
            .connection()
            .execute(
                "UPDATE messages SET summary = 'summary-secret\nvalue' WHERE id = ?1",
                [message.id()],
            )
            .expect("corrupt summary");

        for result in [
            store.load_message_by_id(message.id()),
            store.load_message_by_idempotency_key("corruption-key"),
            store.load_message_by_source_id(PRIVATE_SOURCE),
            store.load_message_by_provider_message_id(
                MessageProvider::Voice,
                "corruption-provider-message",
            ),
        ] {
            let error = result.expect_err("corruption must fail closed");
            assert!(matches!(
                error,
                StoreError::StoredRecordInvalid {
                    resource: "message"
                }
            ));
            assert!(!error.to_string().contains(PRIVATE_SUMMARY));
            assert!(!format!("{error:?}").contains(PRIVATE_SOURCE));
        }
        let list_error = store
            .list_messages(None, 10)
            .expect_err("list corruption must fail closed");
        assert!(matches!(
            list_error,
            StoreError::StoredRecordInvalid {
                resource: "message"
            }
        ));
        let debug = format!("{message:?}");
        for private_value in [
            "corruption-key",
            PRIVATE_SOURCE,
            "corruption-provider-message",
            PRIVATE_SUMMARY,
            "private subject",
            "private@example.com",
        ] {
            assert!(
                !debug.contains(private_value),
                "debug leaked {private_value}"
            );
        }
    }

    #[test]
    fn message_corruption_matrix_fails_generically_through_all_read_paths() {
        const SENTINEL: &str = "message-corruption-sentinel";
        let corruptions: [(&str, &str, bool); 19] = [
            ("idempotency_key", SENTINEL, true),
            ("source_id", SENTINEL, true),
            ("provider", SENTINEL, false),
            ("provider", SENTINEL, true),
            ("provider_message_id", SENTINEL, true),
            ("summary", "summary-corruption\nvalue", false),
            ("summary", SENTINEL, true),
            ("subject", "subject-corruption\nvalue", false),
            ("subject", SENTINEL, true),
            ("sender", "sender-corruption\nvalue", false),
            ("sender", SENTINEL, true),
            ("received_at", "2025-01-02T03:04:05.000Z", false),
            ("received_at", SENTINEL, true),
            ("created_at", "2025-01-02T03:04:05.000Z", false),
            ("created_at", SENTINEL, true),
            ("updated_at", "2025-01-02T03:04:05.000Z", false),
            ("updated_at", SENTINEL, true),
            ("triage_state", SENTINEL, false),
            ("triage_state", SENTINEL, true),
        ];

        for (column, value, as_blob) in corruptions {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let message = store
                .record_message(
                    "matrix-key",
                    "matrix-source",
                    MessageProvider::Voice,
                    "matrix-provider-message",
                    MessageSummary::new("matrix summary").expect("summary"),
                    Some("matrix subject".to_owned()),
                    Some("matrix@example.com".to_owned()),
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
                )
                .expect("message");
            store
                .connection()
                .pragma_update(None, "ignore_check_constraints", true)
                .expect("disable checks");
            let sql = if as_blob {
                format!("UPDATE messages SET {column} = CAST(?1 AS BLOB) WHERE id = ?2")
            } else {
                format!("UPDATE messages SET {column} = ?1 WHERE id = ?2")
            };
            store
                .connection()
                .execute(&sql, rusqlite::params![value, message.id()])
                .expect("corrupt message column");

            let error = store
                .load_message_by_id(message.id())
                .expect_err("corruption must fail by ID");
            assert!(matches!(
                error,
                StoreError::StoredRecordInvalid {
                    resource: "message"
                }
            ));
            assert!(!error.to_string().contains(value));
            assert!(!format!("{error:?}").contains(value));
        }

        for path in ["idempotency", "source", "provider_message", "alias"] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let message = store
                .record_message(
                    "path-key",
                    "path-source",
                    MessageProvider::Voice,
                    "path-provider-message",
                    MessageSummary::new("path summary").expect("summary"),
                    None,
                    None,
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
                )
                .expect("message");
            store
                .connection()
                .pragma_update(None, "ignore_check_constraints", true)
                .expect("disable checks");
            store
                .connection()
                .execute(
                    "UPDATE messages SET summary = 'path-summary-corruption\nvalue' WHERE id = ?1",
                    [message.id()],
                )
                .expect("corrupt summary");
            let result = match path {
                "idempotency" => store.load_message_by_idempotency_key("path-key"),
                "source" => store.load_message_by_source_id("path-source"),
                "provider_message" => store.load_message_by_provider_message_id(
                    MessageProvider::Voice,
                    "path-provider-message",
                ),
                "alias" => store.load_message_by_provider_and_message_id(
                    MessageProvider::Voice,
                    "path-provider-message",
                ),
                _ => unreachable!("known path"),
            };
            let error = result.expect_err("corruption must fail by identity");
            assert!(matches!(
                error,
                StoreError::StoredRecordInvalid {
                    resource: "message"
                }
            ));
            assert!(!error.to_string().contains("path-summary-corruption"));
            assert!(!format!("{error:?}").contains("path-summary-corruption"));
        }

        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        store
            .record_message(
                "list-key",
                "list-source",
                MessageProvider::Voice,
                "list-provider-message",
                MessageSummary::new("list summary").expect("summary"),
                None,
                None,
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
            )
            .expect("message");
        store
            .connection()
            .pragma_update(None, "ignore_check_constraints", true)
            .expect("disable checks");
        store
            .connection()
            .execute(
                "UPDATE messages SET summary = 'list-summary-corruption\nvalue'",
                [],
            )
            .expect("corrupt summary");
        let error = store
            .list_messages(None, 10)
            .expect_err("corruption must fail by list");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "message"
            }
        ));
        assert!(!error.to_string().contains("list-summary-corruption"));
        assert!(!format!("{error:?}").contains("list-summary-corruption"));
    }

    #[test]
    fn message_id_corruption_fails_through_a_reachable_identity_lookup() {
        const SENTINEL: &str = "message-id-corruption-sentinel";
        for corrupt_id in [0_i64, -7_i64] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            store
                .record_message(
                    "id-corruption-key",
                    "id-corruption-source",
                    MessageProvider::Voice,
                    "id-corruption-provider-message",
                    MessageSummary::new("id corruption summary").expect("summary"),
                    None,
                    None,
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
                )
                .expect("message");
            store
                .connection()
                .pragma_update(None, "ignore_check_constraints", true)
                .expect("disable checks");
            store
                .connection()
                .execute(
                    "UPDATE messages SET id = ?1 WHERE source_id = 'id-corruption-source'",
                    [corrupt_id],
                )
                .expect("corrupt message ID");

            let error = store
                .load_message_by_source_id("id-corruption-source")
                .expect_err("non-positive ID must fail strict reconstruction");
            assert!(matches!(
                error,
                StoreError::StoredRecordInvalid {
                    resource: "message"
                }
            ));
            assert!(!error.to_string().contains(SENTINEL));
            assert!(!format!("{error:?}").contains(SENTINEL));
        }
    }

    #[test]
    fn message_invalid_inputs_leave_the_full_table_unchanged() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        store
            .record_message(
                "valid-key",
                "valid-source",
                MessageProvider::Voice,
                "valid-provider-message",
                MessageSummary::new("valid summary").expect("summary"),
                Some("Valid subject".to_owned()),
                Some("valid@example.com".to_owned()),
                received_at,
            )
            .expect("valid message");
        store
            .record_message(
                "valid-key-2",
                "valid-source-2",
                MessageProvider::Gmail,
                "valid-provider-message-2",
                MessageSummary::new("valid summary 2").expect("summary"),
                None,
                None,
                received_at,
            )
            .expect("valid message");

        let invalid_ids: Vec<(&'static str, String)> = vec![
            ("idempotency_key", String::new()),
            ("idempotency_key", "bad key".to_owned()),
            ("idempotency_key", "bad\nkey".to_owned()),
            ("idempotency_key", "x".repeat(MAX_MESSAGE_ID_LENGTH + 1)),
            ("idempotency_key", "é".repeat(MAX_MESSAGE_ID_LENGTH / 2 + 1)),
            ("source_id", String::new()),
            ("source_id", "bad source".to_owned()),
            ("source_id", "bad\nsource".to_owned()),
            ("source_id", "x".repeat(MAX_MESSAGE_ID_LENGTH + 1)),
            ("source_id", "é".repeat(MAX_MESSAGE_ID_LENGTH / 2 + 1)),
            ("provider_message_id", String::new()),
            ("provider_message_id", "bad message".to_owned()),
            ("provider_message_id", "bad\nmessage".to_owned()),
            ("provider_message_id", "x".repeat(MAX_MESSAGE_ID_LENGTH + 1)),
            (
                "provider_message_id",
                "é".repeat(MAX_MESSAGE_ID_LENGTH / 2 + 1),
            ),
        ];
        for (field, value) in invalid_ids {
            assert_invalid_message_write(&store, field, |store| {
                store.record_message(
                    if field == "idempotency_key" {
                        value.as_str()
                    } else {
                        "new-key"
                    },
                    if field == "source_id" {
                        value.as_str()
                    } else {
                        "new-source"
                    },
                    MessageProvider::Voice,
                    if field == "provider_message_id" {
                        value.as_str()
                    } else {
                        "new-provider-message"
                    },
                    MessageSummary::new("new summary").expect("summary"),
                    None,
                    None,
                    received_at,
                )
            });
        }

        let invalid_texts: Vec<(&'static str, String)> = vec![
            ("subject", String::new()),
            ("subject", " ".to_owned()),
            ("subject", "bad\nsubject".to_owned()),
            ("subject", "x".repeat(MAX_MESSAGE_SUBJECT_LENGTH + 1)),
            ("subject", "é".repeat(MAX_MESSAGE_SUBJECT_LENGTH / 2 + 1)),
            ("sender", String::new()),
            ("sender", " ".to_owned()),
            ("sender", "bad\nsender".to_owned()),
            ("sender", "x".repeat(MAX_MESSAGE_SENDER_LENGTH + 1)),
            ("sender", "é".repeat(MAX_MESSAGE_SENDER_LENGTH / 2 + 1)),
        ];
        for (field, value) in invalid_texts {
            assert_invalid_message_write(&store, field, |store| {
                store.record_message(
                    "new-key",
                    "new-source",
                    MessageProvider::Voice,
                    "new-provider-message",
                    MessageSummary::new("new summary").expect("summary"),
                    if field == "subject" {
                        Some(value.clone())
                    } else {
                        None
                    },
                    if field == "sender" { Some(value) } else { None },
                    received_at,
                )
            });
        }

        let fractional = received_at.replace_nanosecond(1).expect("fractional time");
        assert_invalid_message_write(&store, "received_at", |store| {
            store.record_message(
                "new-time-key",
                "new-time-source",
                MessageProvider::Voice,
                "new-time-provider-message",
                MessageSummary::new("new summary").expect("summary"),
                None,
                None,
                fractional,
            )
        });
        let non_utc = received_at.to_offset(UtcOffset::from_hms(2, 0, 0).expect("offset"));
        assert_invalid_message_write(&store, "received_at", |store| {
            store.record_message(
                "new-offset-key",
                "new-offset-source",
                MessageProvider::Voice,
                "new-offset-provider-message",
                MessageSummary::new("new summary").expect("summary"),
                None,
                None,
                non_utc,
            )
        });

        for (after_id, limit) in [(Some(0), 1), (None, 0), (None, MAX_MESSAGE_LIST_LIMIT + 1)] {
            let before = message_snapshot(&store);
            let error = store
                .list_messages(after_id, limit)
                .expect_err("invalid pagination must fail");
            assert!(matches!(error, StoreError::InvalidInput { .. }));
            assert_eq!(message_snapshot(&store), before);
        }
    }

    #[test]
    fn proposal_rebuild_migration_preserves_rows_references_mappings_and_indexes() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..2]).expect("apply v2 schema");
        connection
            .execute_batch(
                r#"
                INSERT INTO appointment_drafts (
                    idempotency_key, source_id, quote_id, caller_name, caller_email,
                    kind, starts_at, ends_at, requester_included
                ) VALUES (
                    'legacy-appointment-key', 'legacy:appointment',
                    '123e4567-e89b-12d3-a456-426614174000', 'Ada', 'ada@example.com',
                    'callback', '2023-11-14T22:13:20Z', '2023-11-14T22:28:20Z', 0
                );
                INSERT INTO owner_task_drafts (
                    idempotency_key, source_id, title, kind, duration_minutes
                ) VALUES (
                    'legacy-owner-key', 'legacy:owner', 'Call supplier', 'callback', 15
                );
                INSERT INTO proposals (
                    idempotency_key, source_id, appointment_draft_id, state
                ) VALUES (
                    'legacy-proposal-appointment-key', 'legacy:proposal:appointment', 1, 'accepted'
                );
                INSERT INTO proposals (
                    idempotency_key, source_id, owner_task_draft_id, state
                ) VALUES (
                    'legacy-proposal-owner-key', 'legacy:proposal:owner', 1, 'declined'
                );
                INSERT INTO event_mappings (
                    proposal_id, provider, provider_event_id, source_id
                ) VALUES (
                    1, 'google', 'legacy-event', 'legacy:event'
                );
                INSERT INTO notification_outbox (
                    idempotency_key, proposal_id, event_mapping_id, notification_kind, recipient
                ) VALUES (
                    'legacy-outbox', 1, 1, 'reminder', 'owner@example.com'
                );
                "#,
            )
            .expect("seed v2 proposal graph");

        run_migrations_with(&mut connection, MIGRATIONS).expect("upgrade proposal schema");

        let version: i64 = connection
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration version");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        for (table, expected) in [
            ("proposals", 2_i64),
            ("event_mappings", 1_i64),
            ("notification_outbox", 1_i64),
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("preserved row count");
            assert_eq!(count, expected, "{table}");
        }
        let mut proposal_statement = connection
            .prepare(
                "SELECT source_id, appointment_draft_id, owner_task_draft_id, state \
                 FROM proposals ORDER BY source_id",
            )
            .expect("proposal reference query");
        let proposal_sources = proposal_statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .expect("proposal reference rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("proposal references");
        assert_eq!(
            proposal_sources,
            vec![
                (
                    "legacy:proposal:appointment".to_owned(),
                    Some(1),
                    None,
                    "accepted".to_owned(),
                ),
                (
                    "legacy:proposal:owner".to_owned(),
                    None,
                    Some(1),
                    "declined".to_owned(),
                ),
            ]
        );
        let mapping_reference = connection
            .query_row(
                "SELECT proposal_id, provider, provider_event_id, source_id FROM event_mappings",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .expect("preserved mapping reference");
        assert_eq!(
            mapping_reference,
            (
                1,
                "google".to_owned(),
                "legacy-event".to_owned(),
                "legacy:event".to_owned(),
            )
        );
        let outbox_reference = connection
            .query_row(
                "SELECT proposal_id, event_mapping_id, notification_kind FROM notification_outbox",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .expect("preserved outbox reference");
        assert_eq!(outbox_reference, (Some(1), Some(1), "reminder".to_owned()));
        let mut foreign_key_statement = connection
            .prepare("PRAGMA foreign_key_list(proposals)")
            .expect("proposal foreign key query");
        let mut delete_actions = foreign_key_statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(2)?, row.get::<_, String>(6)?))
            })
            .expect("proposal foreign key rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("proposal foreign key actions");
        delete_actions.sort();
        assert_eq!(
            delete_actions,
            vec![
                ("appointment_drafts".to_owned(), "RESTRICT".to_owned()),
                ("owner_task_drafts".to_owned(), "RESTRICT".to_owned()),
            ]
        );
        for index in [
            "idx_proposals_state",
            "idx_event_mappings_proposal_id",
            "idx_notification_outbox_delivery",
        ] {
            let count: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [index],
                    |row| row.get(0),
                )
                .expect("preserved named index");
            assert_eq!(count, 1, "{index}");
        }
        let foreign_key_violations: i64 = connection
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("foreign key check");
        assert_eq!(foreign_key_violations, 0);

        drop(proposal_statement);
        drop(foreign_key_statement);
        run_migrations_with(&mut connection, MIGRATIONS).expect("idempotent reopen migration");
        let migration_count: i64 = connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("idempotent migration count");
        assert_eq!(migration_count, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn notification_outbox_delivery_migration_preserves_rows_and_constrains_status() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..3]).expect("apply v3 schema");
        connection
            .execute(
                "INSERT INTO notification_outbox (\
                    idempotency_key, notification_kind, recipient, payload, status, available_at, attempts\
                 ) VALUES (\
                    'legacy-delivery', 'call_summary', 'owner@example.com', '{}', 'pending', \
                    '2023-11-14T22:13:20Z', 2\
                 )",
                [],
            )
            .expect("seed legacy notification");

        run_migrations_with(&mut connection, MIGRATIONS).expect("upgrade delivery schema");

        let preserved = connection
            .query_row(
                "SELECT idempotency_key, status, attempts, lease_until, last_error_code \
                 FROM notification_outbox WHERE idempotency_key = 'legacy-delivery'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .expect("preserved notification");
        assert_eq!(
            preserved,
            (
                "legacy-delivery".to_owned(),
                "pending".to_owned(),
                2,
                None,
                None
            )
        );
        assert!(
            connection
                .execute(
                    "UPDATE notification_outbox SET status = 'unexpected' \
                 WHERE idempotency_key = 'legacy-delivery'",
                    [],
                )
                .is_err()
        );
        let mut index_statement = connection
            .prepare("PRAGMA index_info(idx_notification_outbox_delivery)")
            .expect("delivery index info query");
        let index_columns = index_statement
            .query_map([], |row| row.get::<_, String>(2))
            .expect("delivery index rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("delivery index columns");
        assert_eq!(index_columns, vec!["status", "available_at"]);
        drop(index_statement);

        run_migrations_with(&mut connection, MIGRATIONS).expect("idempotent reopen migration");
        let migration_count: i64 = connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration count");
        assert_eq!(migration_count, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn notification_delivery_migration_normalizes_future_sqlite_timestamps_before_claiming() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..3]).expect("apply v3 schema");
        connection
            .execute(
                "INSERT INTO notification_outbox (\
                    idempotency_key, notification_kind, recipient, payload, status, available_at\
                 ) VALUES (\
                    'legacy-future-default', 'call_summary', 'owner@example.com', '{}', 'pending', \
                    datetime('now', '+2 hours')\
                 )",
                [],
            )
            .expect("seed future v3 notification");
        run_migrations_with(&mut connection, MIGRATIONS).expect("upgrade delivery schema");
        let store = PaStore { connection };
        let now = OffsetDateTime::now_utc();

        let stored = store
            .load_notification_by_idempotency_key("legacy-future-default")
            .expect("normalized future notification");
        assert_eq!(stored.status(), NotificationStatus::Pending);
        assert!(stored.available_at() > now);
        assert!(OffsetDateTime::parse(stored.created_at(), &Rfc3339).is_ok());
        assert!(OffsetDateTime::parse(stored.updated_at(), &Rfc3339).is_ok());
        assert!(
            store
                .claim_notifications(now, 1, TimeDuration::minutes(5))
                .expect("future notification is not claimed early")
                .is_empty()
        );

        let claimed = store
            .claim_notifications(now + TimeDuration::hours(3), 1, TimeDuration::minutes(5))
            .expect("future notification becomes claimable");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id(), stored.id());
    }

    #[test]
    fn notification_delivery_migration_recovers_legacy_statuses_and_normalizes_sent_time() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..3]).expect("apply v3 schema");
        connection
            .execute_batch(
                "INSERT INTO notification_outbox (\
                    idempotency_key, notification_kind, recipient, payload, status, available_at, attempts\
                 ) VALUES (\
                    'legacy-delivering', 'call_summary', 'owner@example.com', '{}', 'delivering', \
                    datetime('now', '-2 hours'), 3\
                 );\
                 INSERT INTO notification_outbox (\
                    idempotency_key, notification_kind, recipient, payload, status, available_at, sent_at, attempts\
                 ) VALUES (\
                    'legacy-sent', 'call_summary', 'owner@example.com', '{}', 'sent', \
                    datetime('now', '-2 hours'), '2025-01-02 03:04:05', 2\
                 );\
                 INSERT INTO notification_outbox (\
                    idempotency_key, notification_kind, recipient, payload, status, available_at, sent_at, attempts\
                 ) VALUES (\
                    'legacy-unknown', 'call_summary', 'owner@example.com', '{}', 'unknown', \
                    datetime('now', '-2 hours'), '2025-01-02 03:04:05', 4\
                 );",
            )
            .expect("seed unconstrained v3 statuses");
        run_migrations_with(&mut connection, MIGRATIONS).expect("upgrade delivery schema");
        let store = PaStore { connection };

        let delivering = store
            .load_notification_by_idempotency_key("legacy-delivering")
            .expect("legacy delivering is recovered");
        assert_eq!(delivering.status(), NotificationStatus::Pending);
        assert_eq!(delivering.attempts(), 3);
        assert_eq!(delivering.lease_until(), None);
        assert_eq!(delivering.sent_at(), None);

        let sent = store
            .load_notification_by_idempotency_key("legacy-sent")
            .expect("valid sent row survives");
        assert_eq!(sent.status(), NotificationStatus::Sent);
        assert_eq!(sent.attempts(), 2);
        assert_eq!(
            sent.sent_at(),
            Some(
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339)
                    .expect("expected sent timestamp"),
            )
        );
        assert!(OffsetDateTime::parse(sent.created_at(), &Rfc3339).is_ok());
        assert!(OffsetDateTime::parse(sent.updated_at(), &Rfc3339).is_ok());

        let unknown = store
            .load_notification_by_idempotency_key("legacy-unknown")
            .expect("unknown status is recovered");
        assert_eq!(unknown.status(), NotificationStatus::Pending);
        assert_eq!(unknown.attempts(), 4);
        assert_eq!(unknown.lease_until(), None);
        assert_eq!(unknown.sent_at(), None);

        let claimed = store
            .claim_notifications(OffsetDateTime::now_utc(), 10, TimeDuration::minutes(5))
            .expect("recovered legacy rows are claimable");
        assert_eq!(
            claimed
                .iter()
                .map(|notification| (notification.id(), notification.attempts()))
                .collect::<Vec<_>>(),
            vec![(delivering.id(), 4), (unknown.id(), 5)]
        );
    }

    #[test]
    fn notification_delivery_migration_rejects_malformed_legacy_timestamps_atomically() {
        const MALFORMED_TIMESTAMP: &str = "malformed-legacy-timestamp-secret";
        const PRIVATE_RECIPIENT: &str = "migration-private-recipient@example.com";
        const PRIVATE_PAYLOAD: &str = "migration-private-payload-secret";

        for column in ["available_at", "created_at", "updated_at", "sent_at"] {
            let mut connection = keyed_connection_for_migration_test();
            run_migrations_with(&mut connection, &MIGRATIONS[..3]).expect("apply v3 schema");
            connection
                .execute(
                    "INSERT INTO notification_outbox (\
                        idempotency_key, notification_kind, recipient, payload, status, available_at, sent_at\
                     ) VALUES (?1, 'call_summary', ?2, ?3, 'pending', '2025-01-02 03:04:05', NULL)",
                    rusqlite::params![format!("legacy-malformed-{column}"), PRIVATE_RECIPIENT, PRIVATE_PAYLOAD],
                )
                .expect("seed legacy notification");
            connection
                .execute(
                    &format!(
                        "UPDATE notification_outbox SET {column} = ?1 \
                         WHERE idempotency_key = ?2"
                    ),
                    rusqlite::params![MALFORMED_TIMESTAMP, format!("legacy-malformed-{column}")],
                )
                .expect("corrupt one legacy timestamp");

            let error = run_migrations_with(&mut connection, MIGRATIONS)
                .expect_err("malformed legacy timestamps must abort v4 migration");
            assert!(matches!(
                error,
                StoreError::StoredRecordInvalid {
                    resource: "notification"
                }
            ));
            let error_text = error.to_string();
            let error_debug = format!("{error:?}");
            for private_value in [MALFORMED_TIMESTAMP, PRIVATE_RECIPIENT, PRIVATE_PAYLOAD] {
                assert!(!error_text.contains(private_value));
                assert!(!error_debug.contains(private_value));
            }

            let version: i64 = connection
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |row| row.get(0),
                )
                .expect("schema version after rejected migration");
            assert_eq!(version, 3, "{column} leaves the schema at v3");
            let persisted: String = connection
                .query_row(
                    &format!(
                        "SELECT {column} FROM notification_outbox \
                         WHERE idempotency_key = ?1"
                    ),
                    [format!("legacy-malformed-{column}")],
                    |row| row.get(0),
                )
                .expect("legacy row remains intact");
            assert_eq!(persisted, MALFORMED_TIMESTAMP);
            let unchanged: (String, String, String, String) = connection
                .query_row(
                    "SELECT idempotency_key, status, recipient, payload FROM notification_outbox \
                     WHERE idempotency_key = ?1",
                    [format!("legacy-malformed-{column}")],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("legacy row data remains intact");
            assert_eq!(
                unchanged,
                (
                    format!("legacy-malformed-{column}"),
                    "pending".to_owned(),
                    PRIVATE_RECIPIENT.to_owned(),
                    PRIVATE_PAYLOAD.to_owned(),
                )
            );
            assert!(
                connection
                    .prepare("SELECT lease_until FROM notification_outbox")
                    .is_err(),
                "v4 claim state is unavailable after rejected migration"
            );
        }
    }

    #[test]
    fn audit_types_convert_known_storage_names_and_reject_unknown_names_generically() {
        assert_eq!(
            AuditEventType::from_storage("message_recorded")
                .expect("known event")
                .as_str(),
            "message_recorded"
        );
        assert_eq!(
            AuditEntityType::from_storage("appointment_request")
                .expect("known entity")
                .as_str(),
            "appointment_request"
        );

        let unknown_event = "event-value-that-must-not-escape";
        let event_error = AuditEventType::from_storage(unknown_event)
            .expect_err("unknown event must fail closed");
        assert!(matches!(
            event_error,
            StoreError::StoredRecordInvalid {
                resource: "audit event"
            }
        ));
        assert!(!event_error.to_string().contains(unknown_event));

        let unknown_entity = "entity-value-that-must-not-escape";
        let entity_error = AuditEntityType::from_storage(unknown_entity)
            .expect_err("unknown entity must fail closed");
        assert!(matches!(
            entity_error,
            StoreError::StoredRecordInvalid {
                resource: "audit entity"
            }
        ));
        assert!(!entity_error.to_string().contains(unknown_entity));
    }

    #[test]
    fn audit_append_retries_stably_and_conflicting_keys_preserve_the_original() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let occurred_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("occurrence time");

        let first = store
            .append_audit_event(
                "audit-retry",
                AuditEventType::MessageRecorded,
                AuditEntityType::Message,
                "message-1",
                occurred_at,
            )
            .expect("first append");
        let retry = store
            .append_audit_event(
                "audit-retry",
                AuditEventType::MessageRecorded,
                AuditEntityType::Message,
                "message-1",
                occurred_at,
            )
            .expect("identical retry");
        assert_eq!(retry, first);

        let error = store
            .append_audit_event(
                "audit-retry",
                AuditEventType::MessageRecorded,
                AuditEntityType::Message,
                "message-2",
                occurred_at,
            )
            .expect_err("different content must conflict");
        assert!(matches!(
            error,
            StoreError::Conflict {
                resource: "audit event"
            }
        ));
        assert!(!error.to_string().contains("message-2"));
        assert_eq!(
            store
                .load_audit_event_by_idempotency_key("audit-retry")
                .expect("original remains"),
            first
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM audit_events", [], |row| row
                    .get::<_, i64>(0))
                .expect("row count"),
            1
        );
    }

    #[test]
    fn audit_events_are_database_enforced_append_only() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let event = store
            .append_audit_event(
                "audit-append-only",
                AuditEventType::MessageRecorded,
                AuditEntityType::Message,
                "message-append-only",
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("occurrence time"),
            )
            .expect("append valid event");

        assert!(
            store
                .connection()
                .execute(
                    "UPDATE audit_events SET entity_id = ?1 WHERE id = ?2",
                    rusqlite::params!["message-mutated", event.id()],
                )
                .is_err(),
            "raw audit updates must be rejected"
        );
        assert_eq!(
            store
                .load_audit_event_by_idempotency_key("audit-append-only")
                .expect("original event after rejected update"),
            event
        );

        assert!(
            store
                .connection()
                .execute("DELETE FROM audit_events WHERE id = ?1", [event.id()],)
                .is_err(),
            "raw audit deletes must be rejected"
        );
        assert_eq!(
            store
                .load_audit_event_by_idempotency_key("audit-append-only")
                .expect("original event after rejected delete"),
            event
        );

        let recursive_triggers: i64 = store
            .connection()
            .pragma_query_value(None, "recursive_triggers", |row| row.get(0))
            .expect("read recursive trigger setting");
        assert_eq!(recursive_triggers, 1, "recursive triggers must be enabled");

        for (statement, replacement_entity_id) in [
            ("INSERT OR REPLACE", "message-replaced-by-insert"),
            ("REPLACE", "message-replaced-by-replace"),
        ] {
            let sql = format!(
                "{statement} INTO audit_events (
                     idempotency_key, event_type, entity_type, entity_id, details,
                     occurred_at, created_at
                 ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)"
            );
            assert!(
                store
                    .connection()
                    .execute(
                        &sql,
                        rusqlite::params![
                            "audit-append-only",
                            replacement_entity_id,
                            "2025-01-02T03:04:05Z"
                        ],
                    )
                    .is_err(),
                "{statement} must not bypass audit immutability"
            );
            assert_eq!(
                store
                    .load_audit_event_by_idempotency_key("audit-append-only")
                    .expect("original event after rejected replacement"),
                event
            );
        }
    }

    #[test]
    fn audit_listing_is_ordered_and_cursor_paginated_with_strict_inputs() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let occurred_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("occurrence time");
        let first = store
            .append_audit_event(
                "audit-page-1",
                AuditEventType::MessageRecorded,
                AuditEntityType::Message,
                "message-1",
                occurred_at,
            )
            .expect("first append");
        let second = store
            .append_audit_event(
                "audit-page-2",
                AuditEventType::ProposalCreated,
                AuditEntityType::Proposal,
                "proposal-1",
                occurred_at,
            )
            .expect("second append");
        assert_eq!(
            store
                .list_audit_events(None, 10)
                .expect("ordered list")
                .iter()
                .map(|event| event.id())
                .collect::<Vec<_>>(),
            vec![first.id(), second.id()]
        );
        assert_eq!(
            store
                .list_audit_events(Some(first.id()), 1)
                .expect("cursor page"),
            vec![second.clone()]
        );

        let before = store
            .connection()
            .query_row("SELECT count(*) FROM audit_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("row count");
        for result in [
            store.list_audit_events(Some(0), 1),
            store.list_audit_events(None, 0),
            store.list_audit_events(None, 101),
        ] {
            assert!(matches!(
                result,
                Err(StoreError::InvalidInput {
                    field: "cursor" | "limit"
                })
            ));
        }
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM audit_events", [], |row| row
                    .get::<_, i64>(0))
                .expect("row count after invalid input"),
            before
        );
    }

    #[test]
    fn audit_append_rejects_invalid_inputs_without_mutation() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let valid_time =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("valid time");
        let fractional_time =
            OffsetDateTime::parse("2025-01-02T03:04:05.123Z", &Rfc3339).expect("fractional time");
        for (key, entity_id, time, field) in [
            ("", "message-1", valid_time, "idempotency_key"),
            ("audit-invalid-entity", "", valid_time, "entity_id"),
            (
                "audit-invalid-time",
                "message-1",
                fractional_time,
                "occurred_at",
            ),
        ] {
            let error = store
                .append_audit_event(
                    key,
                    AuditEventType::MessageRecorded,
                    AuditEntityType::Message,
                    entity_id,
                    time,
                )
                .expect_err("invalid input must fail");
            assert!(matches!(error, StoreError::InvalidInput { field: actual } if actual == field));
        }
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM audit_events", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("row count"),
            0
        );
    }

    #[test]
    fn audit_lookup_and_listing_fail_generically_for_corrupted_rows() {
        for (column, value) in [
            ("event_type", "corrupt-event-value"),
            ("entity_type", "corrupt-entity-value"),
            ("entity_id", "corrupt entity value"),
            ("occurred_at", "corrupt-occurrence-value"),
            ("created_at", "corrupt-creation-value"),
            ("details", "corrupt-details-value"),
        ] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let key = format!("audit-corrupt-{column}");
            store
                .append_audit_event(
                    &key,
                    AuditEventType::MessageRecorded,
                    AuditEntityType::Message,
                    "message-1",
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339)
                        .expect("occurrence time"),
                )
                .expect("append valid row");
            store
                .connection()
                .pragma_update(None, "ignore_check_constraints", true)
                .expect("disable checks for corruption fixture");
            disable_audit_append_only_triggers_for_fixture(store.connection());
            store
                .connection()
                .execute(
                    &format!("UPDATE audit_events SET {column} = ?1 WHERE idempotency_key = ?2"),
                    rusqlite::params![value, &key],
                )
                .expect("corrupt fixture row");
            enable_audit_append_only_triggers_after_fixture(store.connection());
            store
                .connection()
                .pragma_update(None, "ignore_check_constraints", false)
                .expect("restore checks");

            for result in [
                store.load_audit_event_by_idempotency_key(&key),
                store
                    .list_audit_events(None, MAX_AUDIT_LIST_LIMIT)
                    .map(|_| unreachable!("corrupted row must fail")),
            ] {
                let error = result.expect_err("corrupted row must fail closed");
                assert!(matches!(
                    error,
                    StoreError::StoredRecordInvalid {
                        resource: "audit event"
                    }
                ));
                assert!(!error.to_string().contains(value));
                assert!(!format!("{error:?}").contains(value));
            }
        }

        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        store
            .append_audit_event(
                "audit-corrupt-id",
                AuditEventType::MessageRecorded,
                AuditEntityType::Message,
                "message-1",
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("occurrence time"),
            )
            .expect("append valid row");
        store
            .connection()
            .pragma_update(None, "ignore_check_constraints", true)
            .expect("disable checks for corruption fixture");
        disable_audit_append_only_triggers_for_fixture(store.connection());
        store
            .connection()
            .execute(
                "UPDATE audit_events SET id = -1 WHERE idempotency_key = 'audit-corrupt-id'",
                [],
            )
            .expect("corrupt ID fixture");
        enable_audit_append_only_triggers_after_fixture(store.connection());
        let error = store
            .load_audit_event_by_idempotency_key("audit-corrupt-id")
            .expect_err("corrupted ID must fail closed");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "audit event"
            }
        ));
        assert!(matches!(
            store
                .list_audit_events(None, MAX_AUDIT_LIST_LIMIT)
                .expect_err("corrupted ID must fail through list"),
            StoreError::StoredRecordInvalid {
                resource: "audit event"
            }
        ));
    }

    #[test]
    fn audit_schema_migration_rebuilds_legacy_rows_with_stable_keys() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..3]).expect("apply legacy schema");
        connection
            .execute(
                "INSERT INTO audit_events (
                    id, event_type, entity_type, entity_id, details, occurred_at, created_at
                 ) VALUES (
                    42, 'message_recorded', 'message', 'message-42', NULL,
                    '2025-01-02T14:04:05.987+11:00', '2025-01-02T03:04:06.123Z'
                 )",
                [],
            )
            .expect("seed valid legacy audit row");

        run_migrations_with(&mut connection, MIGRATIONS).expect("apply audit schema");
        let row: (
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        ) = connection
            .query_row(
                "SELECT idempotency_key, event_type, entity_type, entity_id, details,
                        occurred_at, created_at
                 FROM audit_events WHERE id = 42",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .expect("read migrated audit row");
        assert_eq!(
            row,
            (
                "legacy-audit-42".to_owned(),
                "message_recorded".to_owned(),
                "message".to_owned(),
                "message-42".to_owned(),
                None,
                "2025-01-02T03:04:05Z".to_owned(),
                "2025-01-02T03:04:06Z".to_owned(),
            )
        );

        run_migrations_with(&mut connection, MIGRATIONS).expect("reopen migration");
        let version: i64 = connection
            .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("schema version");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn audit_machine_identifiers_reject_non_ascii_whitespace_and_byte_overflows() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let connection = store.connection();
        let canonical = "2025-01-02T03:04:05Z";
        let valid = "machine-id_01:part.2";
        let too_many_multibyte_bytes = "é".repeat((MAX_AUDIT_ENTITY_ID_LENGTH / 2) + 1);

        assert_eq!(
            validate_audit_entity_id(valid.to_owned()).expect("valid entity identifier"),
            valid
        );
        assert_eq!(
            validate_audit_idempotency_key(valid.to_owned()).expect("valid idempotency identifier"),
            valid
        );

        for value in [
            "\t",
            "\n",
            "\u{2003}",
            "machine\0identifier",
            too_many_multibyte_bytes.as_str(),
        ] {
            assert!(validate_audit_entity_id(value.to_owned()).is_err());
            assert!(validate_audit_idempotency_key(value.to_owned()).is_err());
            assert!(
                connection
                    .execute(
                        "INSERT INTO audit_events (
                            idempotency_key, event_type, entity_type, entity_id, details,
                            occurred_at, created_at
                         ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)",
                        rusqlite::params![format!("key-{}", value.len()), value, canonical],
                    )
                    .is_err(),
                "schema must reject invalid entity identifier"
            );
            assert!(
                connection
                    .execute(
                        "INSERT INTO audit_events (
                            idempotency_key, event_type, entity_type, entity_id, details,
                            occurred_at, created_at
                         ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)",
                        rusqlite::params![value, valid, canonical],
                    )
                    .is_err(),
                "schema must reject invalid idempotency identifier"
            );
        }
    }

    #[test]
    fn audit_schema_rejects_blob_machine_identifiers_without_damaging_valid_rows() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let connection = store.connection();
        let canonical = "2025-01-02T03:04:05Z";
        let valid_key = "audit-valid-text";
        let valid_entity_id = "message-valid-text";

        connection
            .execute(
                "INSERT INTO audit_events (
                    idempotency_key, event_type, entity_type, entity_id, details,
                    occurred_at, created_at
                 ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)",
                rusqlite::params![valid_key, valid_entity_id, canonical],
            )
            .expect("insert valid text audit row");

        assert!(
            connection
                .execute(
                    "INSERT INTO audit_events (
                        idempotency_key, event_type, entity_type, entity_id, details,
                        occurred_at, created_at
                     ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)",
                    rusqlite::params![b"audit-blob-key".as_slice(), "message-blob-key", canonical],
                )
                .is_err(),
            "BLOB idempotency keys must not bypass the text contract"
        );
        assert!(
            connection
                .execute(
                    "INSERT INTO audit_events (
                        idempotency_key, event_type, entity_type, entity_id, details,
                        occurred_at, created_at
                     ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)",
                    rusqlite::params![
                        "audit-blob-entity",
                        b"message-blob-entity".as_slice(),
                        canonical
                    ],
                )
                .is_err(),
            "BLOB entity identifiers must not bypass the text contract"
        );

        let row: (String, String, String, String) = connection
            .query_row(
                "SELECT typeof(idempotency_key), idempotency_key, typeof(entity_id), entity_id
                 FROM audit_events WHERE idempotency_key = ?1",
                [valid_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("valid audit row remains intact");
        assert_eq!(
            row,
            (
                "text".to_owned(),
                valid_key.to_owned(),
                "text".to_owned(),
                valid_entity_id.to_owned(),
            )
        );
    }

    #[test]
    fn audit_schema_rejects_nul_text_machine_identifiers_without_damaging_valid_rows() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let connection = store.connection();
        let canonical = "2025-01-02T03:04:05Z";
        let valid_key = "audit-valid-nul-check";
        let valid_entity_id = "message-valid-nul-check";

        connection
            .execute(
                "INSERT INTO audit_events (
                    idempotency_key, event_type, entity_type, entity_id, details,
                    occurred_at, created_at
                 ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)",
                rusqlite::params![valid_key, valid_entity_id, canonical],
            )
            .expect("insert valid text audit row");

        assert!(
            connection
                .execute(
                    "INSERT INTO audit_events (
                        idempotency_key, event_type, entity_type, entity_id, details,
                        occurred_at, created_at
                     ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)",
                    rusqlite::params!["audit\0nul-key", "message-nul-key", canonical],
                )
                .is_err(),
            "NUL idempotency keys must not bypass the text contract"
        );
        assert!(
            connection
                .execute(
                    "INSERT INTO audit_events (
                        idempotency_key, event_type, entity_type, entity_id, details,
                        occurred_at, created_at
                     ) VALUES (?1, 'message_recorded', 'message', ?2, NULL, ?3, ?3)",
                    rusqlite::params!["audit-nul-entity", "message\0nul-entity", canonical],
                )
                .is_err(),
            "NUL entity identifiers must not bypass the text contract"
        );

        let row: (String, String) = connection
            .query_row(
                "SELECT idempotency_key, entity_id FROM audit_events WHERE idempotency_key = ?1",
                [valid_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("valid audit row remains intact");
        assert_eq!(row, (valid_key.to_owned(), valid_entity_id.to_owned()));
    }

    #[test]
    fn audit_schema_rejects_invalid_values_and_noncanonical_rows() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let connection = store.connection();
        let insert = |key: &str,
                      event_type: &str,
                      entity_type: &str,
                      entity_id: &str,
                      details: Option<&str>,
                      occurred_at: &str,
                      created_at: &str| {
            connection.execute(
                "INSERT INTO audit_events (
                    idempotency_key, event_type, entity_type, entity_id, details,
                    occurred_at, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    key,
                    event_type,
                    entity_type,
                    entity_id,
                    details,
                    occurred_at,
                    created_at,
                ],
            )
        };
        let canonical = "2025-01-02T03:04:05Z";
        assert!(
            insert(
                "audit-valid",
                "message_recorded",
                "message",
                "message-1",
                None,
                canonical,
                canonical,
            )
            .is_ok()
        );
        assert!(
            insert(
                "audit-invalid-event",
                "unknown_event",
                "message",
                "message-2",
                None,
                canonical,
                canonical,
            )
            .is_err()
        );
        assert!(
            insert(
                "audit-invalid-entity",
                "message_recorded",
                "unknown_entity",
                "message-3",
                None,
                canonical,
                canonical,
            )
            .is_err()
        );
        assert!(
            insert(
                "audit-blank-id",
                "message_recorded",
                "message",
                " ",
                None,
                canonical,
                canonical,
            )
            .is_err()
        );
        assert!(
            insert(
                "audit-long-id",
                "message_recorded",
                "message",
                &"x".repeat(MAX_AUDIT_ENTITY_ID_LENGTH + 1),
                None,
                canonical,
                canonical,
            )
            .is_err()
        );
        assert!(
            insert(
                "audit-details",
                "message_recorded",
                "message",
                "message-4",
                Some("arbitrary details"),
                canonical,
                canonical,
            )
            .is_err()
        );
        assert!(
            insert(
                "audit-occurred-fraction",
                "message_recorded",
                "message",
                "message-5",
                None,
                "2025-01-02T03:04:05.000Z",
                canonical,
            )
            .is_err()
        );
        assert!(
            insert(
                "audit-created-fraction",
                "message_recorded",
                "message",
                "message-6",
                None,
                canonical,
                "2025-01-02T03:04:05.000Z",
            )
            .is_err()
        );
        assert!(
            insert(
                "audit-valid",
                "message_recorded",
                "message",
                "message-7",
                None,
                canonical,
                canonical,
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_legacy_audit_rows_abort_migration_and_version_advance() {
        for (column, value) in [
            ("event_type", "legacy-event-value"),
            ("entity_type", "legacy-entity-value"),
            ("entity_id", " "),
            ("details", "legacy-details-value"),
            ("occurred_at", "legacy-timestamp-value"),
            ("created_at", "legacy-timestamp-value"),
        ] {
            let mut connection = keyed_connection_for_migration_test();
            run_migrations_with(&mut connection, &MIGRATIONS[..3]).expect("apply legacy schema");
            connection
                .execute(
                    "INSERT INTO audit_events (
                        id, event_type, entity_type, entity_id, details, occurred_at, created_at
                     ) VALUES (
                        9, 'message_recorded', 'message', 'message-9', NULL,
                        '2025-01-02 03:04:05', '2025-01-02 03:04:06'
                     )",
                    [],
                )
                .expect("seed legacy audit row");
            connection
                .execute(
                    &format!("UPDATE audit_events SET {column} = ?1 WHERE id = 9"),
                    [value],
                )
                .expect("corrupt legacy audit row");

            let error = run_migrations_with(&mut connection, MIGRATIONS)
                .expect_err("malformed legacy audit rows must fail closed");
            assert!(matches!(
                error,
                StoreError::StoredRecordInvalid {
                    resource: "audit event"
                } | StoreError::StoredRecordInvalid {
                    resource: "audit entity"
                } | StoreError::StoredRecordInvalid { resource: "audit" }
            ));
            let text = error.to_string();
            let debug = format!("{error:?}");
            if !value.trim().is_empty() {
                assert!(!text.contains(value));
                assert!(!debug.contains(value));
            }
            let version: i64 = connection
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |row| row.get(0),
                )
                .expect("schema version");
            assert_eq!(version, 3, "{column} leaves the schema at v3");
            assert!(
                connection
                    .prepare("SELECT idempotency_key FROM audit_events")
                    .is_err(),
                "{column} must leave the legacy table intact"
            );
        }
    }

    #[test]
    fn null_legacy_audit_entity_fields_abort_migration_and_version_advance() {
        for column in ["entity_type", "entity_id"] {
            let mut connection = keyed_connection_for_migration_test();
            run_migrations_with(&mut connection, &MIGRATIONS[..3]).expect("apply legacy schema");
            connection
                .execute(
                    "INSERT INTO audit_events (
                        id, event_type, entity_type, entity_id, details, occurred_at, created_at
                     ) VALUES (
                        10, 'message_recorded', 'message', 'message-10', NULL,
                        '2025-01-02 03:04:05', '2025-01-02 03:04:06'
                     )",
                    [],
                )
                .expect("seed legacy audit row");
            connection
                .execute(
                    &format!("UPDATE audit_events SET {column} = NULL WHERE id = 10"),
                    [],
                )
                .expect("null legacy audit field");

            let error = run_migrations_with(&mut connection, MIGRATIONS)
                .expect_err("NULL legacy audit entity fields must fail closed");
            assert!(matches!(error, StoreError::StoredRecordInvalid { .. }));
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("schema version"),
                3,
                "{column} leaves the schema at v3"
            );
            assert!(
                connection
                    .prepare("SELECT idempotency_key FROM audit_events")
                    .is_err()
            );
        }
    }

    #[test]
    fn notification_delivery_defaults_are_canonical_and_strictly_loadable() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let payload = serde_json::to_string(&notification_template(None, None))
            .expect("serialize valid notification template");
        store
            .connection()
            .execute(
                "INSERT INTO notification_outbox (\
                    idempotency_key, notification_kind, recipient, payload\
                 ) VALUES (?1, 'call_summary', ?2, ?3)",
                rusqlite::params![
                    "notification-default-timestamps",
                    notification_recipient().as_str(),
                    payload,
                ],
            )
            .expect("insert a row that relies on v4 timestamp defaults");

        let notification = store
            .load_notification_by_idempotency_key("notification-default-timestamps")
            .expect("defaulted notification loads through strict decoding");
        assert_eq!(notification.status(), NotificationStatus::Pending);
        let defaults: (String, String, String) = store
            .connection()
            .query_row(
                "SELECT available_at, created_at, updated_at FROM notification_outbox \
                 WHERE idempotency_key = 'notification-default-timestamps'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read default timestamps");
        for value in [defaults.0, defaults.1, defaults.2] {
            assert_eq!(value.len(), 20, "canonical timestamps use whole seconds");
            assert!(!value.contains('.'));
            let parsed =
                OffsetDateTime::parse(&value, &Rfc3339).expect("default timestamp is RFC3339");
            assert_eq!(
                parsed.format(&Rfc3339).expect("format timestamp"),
                value,
                "default timestamp is strict canonical RFC3339"
            );
        }
    }

    #[test]
    fn required_tables_unique_contracts_and_lookup_indexes_exist() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        assert_eq!(
            table_names(&store),
            vec![
                "appointment_drafts",
                "appointment_quote_slots",
                "appointment_quotes",
                "audit_events",
                "backup_operation_attempts",
                "configuration",
                "event_mappings",
                "http_idempotency_records",
                "messages",
                "notification_outbox",
                "oauth_credentials",
                "owner_task_drafts",
                "owner_task_placements",
                "proposals",
                "provider_cursors",
                "replay_nonces",
                "schema_migrations",
                "tasks",
            ]
        );

        for (table, expected) in [
            ("oauth_credentials", vec![vec!["provider", "account_id"]]),
            ("provider_cursors", vec![vec!["provider"]]),
            (
                "appointment_drafts",
                vec![
                    vec!["id", "quote_id"],
                    vec!["idempotency_key"],
                    vec!["quote_id"],
                    vec!["source_id"],
                ],
            ),
            (
                "appointment_quotes",
                vec![vec!["appointment_draft_id"], vec!["quote_id"]],
            ),
            (
                "appointment_quote_slots",
                vec![
                    vec!["quote_id", "slot_index"],
                    vec!["quote_id", "starts_at", "ends_at"],
                ],
            ),
            (
                "owner_task_drafts",
                vec![vec!["idempotency_key"], vec!["source_id"]],
            ),
            (
                "messages",
                vec![
                    vec!["idempotency_key"],
                    vec!["provider", "provider_message_id"],
                    vec!["source_id"],
                ],
            ),
            ("tasks", vec![vec!["idempotency_key"], vec!["source_id"]]),
            (
                "proposals",
                vec![vec!["idempotency_key"], vec!["source_id"]],
            ),
            (
                "event_mappings",
                vec![
                    vec!["proposal_id"],
                    vec!["provider", "provider_event_id"],
                    vec!["source_id"],
                ],
            ),
            ("notification_outbox", vec![vec!["idempotency_key"]]),
            ("replay_nonces", vec![vec!["nonce"]]),
            ("audit_events", vec![vec!["idempotency_key"]]),
        ] {
            assert_unique_index_columns(&store, table, &expected);
        }

        for (index, table, columns) in [
            (
                "idx_appointment_drafts_starts_at",
                "appointment_drafts",
                &["starts_at"][..],
            ),
            (
                "idx_owner_task_drafts_due_at",
                "owner_task_drafts",
                &["due_at"][..],
            ),
            ("idx_messages_received_at", "messages", &["received_at"][..]),
            (
                "idx_tasks_status_due_at",
                "tasks",
                &["status", "due_at"][..],
            ),
            ("idx_proposals_state", "proposals", &["state"][..]),
            (
                "idx_event_mappings_proposal_id",
                "event_mappings",
                &["proposal_id"][..],
            ),
            (
                "idx_notification_outbox_delivery",
                "notification_outbox",
                &["status", "available_at"][..],
            ),
            (
                "idx_replay_nonces_expires_at",
                "replay_nonces",
                &["expires_at"][..],
            ),
            (
                "idx_audit_events_occurred_at",
                "audit_events",
                &["occurred_at"][..],
            ),
        ] {
            assert_named_index(&store, index, table, columns);
        }
    }

    #[test]
    fn proposal_event_and_outbox_foreign_keys_reject_missing_parents() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let connection = store.connection();

        assert!(
            connection
                .execute(
                    "INSERT INTO proposals(idempotency_key, source_id, appointment_draft_id) \
                 VALUES ('proposal-appointment', 'proposal-appointment-source', 999)",
                    [],
                )
                .is_err()
        );
        assert!(
            connection
                .execute(
                    "INSERT INTO proposals(idempotency_key, source_id, owner_task_draft_id) \
                 VALUES ('proposal-owner-task', 'proposal-owner-task-source', 999)",
                    [],
                )
                .is_err()
        );
        assert!(connection
            .execute(
                "INSERT INTO event_mappings(proposal_id, provider, provider_event_id, source_id) \
                 VALUES (999, 'outlook', 'missing-proposal-event', 'missing-proposal-source')",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO notification_outbox(idempotency_key, proposal_id, notification_kind, recipient) \
                 VALUES ('missing-outbox-proposal', 999, 'reminder', 'owner@example.com')",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO notification_outbox(idempotency_key, event_mapping_id, notification_kind, recipient) \
                 VALUES ('missing-outbox-event', 999, 'reminder', 'owner@example.com')",
                [],
            )
            .is_err());
    }

    #[test]
    fn failing_multi_statement_migration_rolls_back_schema_and_version() {
        let mut connection = keyed_connection_for_migration_test();
        let migrations = [Migration {
            version: 1,
            apply: failing_multi_statement_migration,
        }];

        assert!(run_migrations_with(&mut connection, &migrations).is_err());
        for table in ["schema_migrations", "partial_migration_effect"] {
            let count: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("table presence query");
            assert_eq!(count, 0, "{table} must roll back");
        }
    }

    #[test]
    fn file_stores_use_wal_journal_mode() {
        let database = TempDatabase::new();
        let store = PaStore::open(&database.path, DATABASE_KEY).expect("open file store");
        let journal_mode: String = store
            .connection()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    }

    #[test]
    fn configuration_is_seeded_with_exact_safe_defaults() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let defaults = store
            .connection()
            .query_row(
                "SELECT owner_timezone, owner_email, owner_phone, working_days, \
                        working_window_start, working_window_end, minimum_notice_minutes, \
                        booking_horizon_days, meeting_buffer_minutes, retention_days, \
                        task_duration_bill_minutes, task_duration_callback_minutes, \
                        task_duration_reading_minutes, task_duration_email_reply_minutes, \
                        task_duration_preparation_minutes, email_triage_model \
                 FROM configuration WHERE id = 1",
                [],
                |row| {
                    Ok(ConfigurationDefaults {
                        owner_timezone: row.get(0)?,
                        owner_email: row.get(1)?,
                        owner_phone: row.get(2)?,
                        working_days: row.get(3)?,
                        working_window_start: row.get(4)?,
                        working_window_end: row.get(5)?,
                        minimum_notice_minutes: row.get(6)?,
                        booking_horizon_days: row.get(7)?,
                        meeting_buffer_minutes: row.get(8)?,
                        retention_days: row.get(9)?,
                        task_duration_bill_minutes: row.get(10)?,
                        task_duration_callback_minutes: row.get(11)?,
                        task_duration_reading_minutes: row.get(12)?,
                        task_duration_email_reply_minutes: row.get(13)?,
                        task_duration_preparation_minutes: row.get(14)?,
                        email_triage_model: row.get(15)?,
                    })
                },
            )
            .expect("configuration defaults");
        assert_eq!(defaults.owner_timezone, None);
        assert_eq!(defaults.owner_email, None);
        assert_eq!(defaults.owner_phone, None);
        assert_eq!(
            defaults.working_days,
            "monday,tuesday,wednesday,thursday,friday"
        );
        assert_eq!(defaults.working_window_start, "08:00");
        assert_eq!(defaults.working_window_end, "18:00");
        assert_eq!(defaults.minimum_notice_minutes, 60);
        assert_eq!(defaults.booking_horizon_days, 60);
        assert_eq!(defaults.meeting_buffer_minutes, 0);
        assert_eq!(defaults.retention_days, 90);
        assert_eq!(defaults.task_duration_bill_minutes, 15);
        assert_eq!(defaults.task_duration_callback_minutes, 15);
        assert_eq!(defaults.task_duration_reading_minutes, 30);
        assert_eq!(defaults.task_duration_email_reply_minutes, 30);
        assert_eq!(defaults.task_duration_preparation_minutes, 60);
        assert_eq!(defaults.email_triage_model, "gpt-5.6-luna");
    }

    #[test]
    fn fresh_store_starts_configuration_version_at_one() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open current store");
        let version: i64 = store
            .connection()
            .query_row(
                "SELECT version FROM configuration WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("fresh configuration version");
        assert_eq!(version, 1);
    }

    #[test]
    fn wrong_key_fails_without_echoing_the_key() {
        let database = TempDatabase::new();
        let store = PaStore::open(&database.path, DATABASE_KEY).expect("create store");
        store
            .connection()
            .execute(
                "INSERT INTO audit_events(
                    idempotency_key, event_type, entity_type, entity_id
                 ) VALUES ('wrong-key-check', 'message_recorded', 'message', 'message-1')",
                [],
            )
            .expect("write encrypted row");
        drop(store);

        let wrong_key = b"definitely-the-wrong-key";
        let error = PaStore::open(&database.path, wrong_key).expect_err("wrong key must fail");
        let message = error.to_string();
        assert!(!message.contains(std::str::from_utf8(wrong_key).unwrap()));
        assert!(!message.contains("definitely-the-wrong-key"));
    }

    #[test]
    fn connection_exposes_only_the_open_database() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let _: &Connection = store.connection();
    }

    #[test]
    fn oauth_credentials_are_validated_normalized_and_round_trip_encrypted() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let cipher = crate::pa::crypto::TokenCipher::new([0x41; 32]).expect("cipher");
        let expires_at =
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).expect("expiry");
        let credential = OAuthCredential::new(
            "google",
            "account-a",
            "access-token-value",
            Some("refresh-token-value".to_owned()),
            Some(expires_at),
            ["Mail.Read", "Calendars.Read", "Mail.Read"],
        )
        .expect("valid credential");

        assert_eq!(
            credential.scopes(),
            ["Calendars.Read".to_owned(), "Mail.Read".to_owned()]
        );
        store
            .save_oauth_credential(&cipher, credential.clone())
            .expect("save credential");

        let row = store
            .connection()
            .query_row(
                "SELECT access_token_ciphertext, refresh_token_ciphertext \
                 FROM oauth_credentials WHERE provider = 'google' AND account_id = 'account-a'",
                [],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .expect("ciphertext row");
        assert!(
            !row.0
                .windows(b"access-token-value".len())
                .any(|window| { window == b"access-token-value" })
        );
        assert!(
            !row.1
                .windows(b"refresh-token-value".len())
                .any(|window| { window == b"refresh-token-value" })
        );
        let access_envelope: crate::pa::crypto::EncryptedSecret =
            serde_json::from_slice(&row.0).expect("access envelope");
        let refresh_envelope: crate::pa::crypto::EncryptedSecret =
            serde_json::from_slice(&row.1).expect("refresh envelope");
        assert_eq!(
            cipher
                .decrypt(&access_envelope, b"oauth:google:account-a:access")
                .expect("access AAD round trip"),
            b"access-token-value"
        );
        assert_eq!(
            cipher
                .decrypt(&refresh_envelope, b"oauth:google:account-a:refresh")
                .expect("refresh AAD round trip"),
            b"refresh-token-value"
        );
        assert!(matches!(
            cipher.decrypt(&access_envelope, b"oauth:google:account-a:refresh"),
            Err(crate::pa::crypto::CryptoError::DecryptionFailed)
        ));
        let wrong_cipher = crate::pa::crypto::TokenCipher::new([0x42; 32]).expect("cipher");
        assert!(matches!(
            wrong_cipher.decrypt(&access_envelope, b"oauth:google:account-a:access"),
            Err(crate::pa::crypto::CryptoError::DecryptionFailed)
        ));
        let wrong_key_error = store
            .load_oauth_credential(&wrong_cipher, "google", "account-a")
            .expect_err("wrong key must fail");
        let wrong_key_text = wrong_key_error.to_string();
        assert!(!wrong_key_text.contains("access-token-value"));
        assert!(!wrong_key_text.contains("refresh-token-value"));
        assert_eq!(
            store
                .load_oauth_credential(&cipher, "google", "account-a")
                .expect("load credential"),
            credential
        );
    }

    #[test]
    fn oauth_credential_validation_errors_are_distinct_from_not_found() {
        for (provider, account_id, access_token, refresh_token, scopes) in [
            (
                " ",
                "account-a",
                "access-token-value",
                None,
                vec!["Mail.Read"],
            ),
            ("google", " ", "access-token-value", None, vec!["Mail.Read"]),
            ("google", "account-a", "", None, vec!["Mail.Read"]),
            (
                "google",
                "account-a",
                "access-token-value",
                Some(String::new()),
                vec!["Mail.Read"],
            ),
            ("google", "account-a", "access-token-value", None, vec![]),
            ("google", "account-a", "access-token-value", None, vec![" "]),
        ] {
            let invalid = OAuthCredential::new(
                provider,
                account_id,
                access_token,
                refresh_token,
                None,
                scopes,
            )
            .expect_err("invalid credential must fail");
            assert!(matches!(invalid, StoreError::InvalidInput { .. }));
        }

        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let missing = store
            .load_oauth_credential(
                &crate::pa::crypto::TokenCipher::new([0x41; 32]).expect("cipher"),
                "google",
                "missing",
            )
            .expect_err("missing credential");
        assert!(matches!(missing, StoreError::NotFound { .. }));
        assert!(matches!(
            store.load_provider_cursor(" "),
            Err(StoreError::InvalidInput { .. })
        ));
        assert!(matches!(
            store.advance_provider_cursor("stream", None, " "),
            Err(StoreError::InvalidInput { .. })
        ));
    }

    #[test]
    fn oauth_credentials_isolate_accounts_upsert_without_duplicates_and_delete_exactly_one() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let cipher = crate::pa::crypto::TokenCipher::new([0x41; 32]).expect("cipher");
        let make = |account_id: &str, access_token: &str| {
            OAuthCredential::new(
                "google",
                account_id,
                access_token,
                None,
                None,
                ["Mail.Read"],
            )
            .expect("valid credential")
        };

        store
            .save_oauth_credential(&cipher, make("account-a", "first"))
            .expect("first save");
        store
            .save_oauth_credential(&cipher, make("account-a", "second"))
            .expect("upsert");
        store
            .save_oauth_credential(&cipher, make("account-b", "other"))
            .expect("isolated save");

        let count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM oauth_credentials WHERE provider = 'google'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(count, 2);
        assert_eq!(
            store
                .load_oauth_credential(&cipher, "google", "account-a")
                .expect("load updated")
                .access_token(),
            "second"
        );
        store
            .delete_oauth_credential("google", "account-a")
            .expect("delete account a");
        assert!(matches!(
            store.load_oauth_credential(&cipher, "google", "account-a"),
            Err(StoreError::NotFound { .. })
        ));
        assert_eq!(
            store
                .load_oauth_credential(&cipher, "google", "account-b")
                .expect("account b remains")
                .access_token(),
            "other"
        );
    }

    #[test]
    fn provider_cursor_cas_rejects_stale_and_equal_retry() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        store
            .advance_provider_cursor(STREAM_A, None, FIRST_CURSOR)
            .expect("first cursor");
        store
            .advance_provider_cursor(STREAM_A, Some(FIRST_CURSOR), NEXT_CURSOR)
            .expect("cursor advance");
        let before_equal_count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| {
                row.get(0)
            })
            .expect("cursor count before equal retry");
        let before_equal_timestamp: String = store
            .connection()
            .query_row(
                "SELECT updated_at FROM provider_cursors WHERE provider = ?1",
                [STREAM_A],
                |row| row.get(0),
            )
            .expect("timestamp before equal retry");
        store
            .advance_provider_cursor(STREAM_A, Some(NEXT_CURSOR), NEXT_CURSOR)
            .expect("equal retry");
        let after_equal_count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| {
                row.get(0)
            })
            .expect("cursor count after equal retry");
        let after_equal_timestamp: String = store
            .connection()
            .query_row(
                "SELECT updated_at FROM provider_cursors WHERE provider = ?1",
                [STREAM_A],
                |row| row.get(0),
            )
            .expect("timestamp after equal retry");
        assert!(before_equal_count == after_equal_count);
        assert!(before_equal_timestamp == after_equal_timestamp);
        assert!(matches!(
            store.load_provider_cursor(STREAM_A),
            Ok(Some(value)) if value == NEXT_CURSOR
        ));
        assert!(matches!(
            store.advance_provider_cursor(STREAM_A, Some(FIRST_CURSOR), STALE_CURSOR),
            Err(StoreError::CursorConflict {
                resource: "provider cursor"
            })
        ));
        assert!(matches!(
            store.advance_provider_cursor(STREAM_A, Some(FIRST_CURSOR), NEXT_CURSOR),
            Err(StoreError::CursorConflict {
                resource: "provider cursor"
            })
        ));
        assert!(matches!(
            store.load_provider_cursor(STREAM_A),
            Ok(Some(value)) if value == NEXT_CURSOR
        ));
    }

    #[test]
    fn provider_cursor_first_write_and_restart() {
        let null_store = PaStore::open_in_memory(DATABASE_KEY).expect("open null store");
        null_store
            .connection()
            .execute(
                "INSERT INTO provider_cursors(provider, cursor) VALUES (?1, NULL)",
                [STREAM_A],
            )
            .expect("insert nullable cursor row");
        assert!(matches!(
            null_store.load_provider_cursor(STREAM_A),
            Ok(None)
        ));
        null_store
            .advance_provider_cursor(STREAM_A, None, FIRST_CURSOR)
            .expect("replace nullable cursor");
        assert!(matches!(
            null_store.load_provider_cursor(STREAM_A),
            Ok(Some(value)) if value == FIRST_CURSOR
        ));

        let database = TempDatabase::new();
        let store = PaStore::open(&database.path, DATABASE_KEY).expect("open file store");
        assert!(matches!(store.load_provider_cursor(STREAM_A), Ok(None)));
        store
            .advance_provider_cursor(STREAM_A, None, FIRST_CURSOR)
            .expect("first file cursor");
        assert!(matches!(
            store.load_provider_cursor(STREAM_A),
            Ok(Some(value)) if value == FIRST_CURSOR
        ));
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| {
                row.get(0)
            })
            .expect("cursor row count");
        assert!(count == 1);
        drop(store);

        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen file store");
        assert!(matches!(
            reopened.load_provider_cursor(STREAM_A),
            Ok(Some(value)) if value == FIRST_CURSOR
        ));
        assert!(matches!(
            reopened.advance_provider_cursor(STREAM_A, None, NEXT_CURSOR),
            Err(StoreError::CursorConflict {
                resource: "provider cursor"
            })
        ));
        let reopened_count: i64 = reopened
            .connection()
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| {
                row.get(0)
            })
            .expect("reopened cursor row count");
        assert!(reopened_count == 1);
    }

    #[test]
    fn provider_cursor_two_handles_have_one_winner() {
        let database = TempDatabase::new();
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("open seed store");
        seed.advance_provider_cursor(STREAM_A, None, FIRST_CURSOR)
            .expect("seed cursor");
        drop(seed);

        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.advance_provider_cursor(STREAM_A, Some(FIRST_CURSOR), NEXT_CURSOR)
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.advance_provider_cursor(STREAM_A, Some(FIRST_CURSOR), STALE_CURSOR)
        });
        let results = [
            first_handle.join().expect("first cursor thread"),
            second_handle.join().expect("second cursor thread"),
        ];
        assert!(results.iter().filter(|result| result.is_ok()).count() == 1);
        assert!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::CursorConflict {
                        resource: "provider cursor"
                    })
                ))
                .count()
                == 1
        );

        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen race store");
        let current = reopened
            .load_provider_cursor(STREAM_A)
            .expect("load race winner");
        assert!(matches!(
            current.as_deref(),
            Some(value) if value == NEXT_CURSOR || value == STALE_CURSOR
        ));
        assert!(matches!(
            reopened.advance_provider_cursor(STREAM_A, Some(FIRST_CURSOR), NEXT_CURSOR),
            Err(StoreError::CursorConflict {
                resource: "provider cursor"
            })
        ));
        let winner = current.as_deref().expect("race winner value");
        assert!(
            reopened
                .advance_provider_cursor(STREAM_A, Some(winner), winner)
                .is_ok()
        );
    }

    #[test]
    fn provider_cursor_invalid_inputs_are_atomic_and_redacted() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let invalid_streams = vec![
            String::new(),
            " ".to_owned(),
            "stream id".to_owned(),
            "stream\tid".to_owned(),
            "stream\nid".to_owned(),
            "é".to_owned(),
            "x".repeat(257),
            format!("{REDACTION_SENTINEL}\n"),
        ];
        for stream in invalid_streams {
            let error = store
                .load_provider_cursor(&stream)
                .expect_err("invalid stream must fail");
            assert!(matches!(error, StoreError::InvalidInput { .. }));
            assert!(!error.to_string().contains(REDACTION_SENTINEL));
            assert!(!format!("{error:?}").contains(REDACTION_SENTINEL));
        }
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| {
                row.get(0)
            })
            .expect("invalid stream row count");
        assert!(count == 0);

        let printable_stream = "microsoft.mail:account-printable";
        let printable_cursor = "page=abc/def+ 1";
        store
            .advance_provider_cursor(printable_stream, None, printable_cursor)
            .expect("provider-compatible cursor must persist");
        assert!(matches!(
            store.load_provider_cursor(printable_stream),
            Ok(Some(value)) if value == printable_cursor
        ));

        store
            .advance_provider_cursor(STREAM_A, None, FIRST_CURSOR)
            .expect("seed cursor");
        let invalid_expected = vec![
            Some(String::new()),
            Some(" ".to_owned()),
            Some("stream\tid".to_owned()),
            Some("stream\nid".to_owned()),
            Some("é".to_owned()),
            Some("x".repeat(257)),
            Some(format!("{REDACTION_SENTINEL}\n")),
        ];
        for expected in invalid_expected {
            let error = store
                .advance_provider_cursor(STREAM_A, expected.as_deref(), NEXT_CURSOR)
                .expect_err("invalid expected cursor must fail");
            assert!(matches!(error, StoreError::InvalidInput { .. }));
            assert!(!error.to_string().contains(REDACTION_SENTINEL));
            assert!(!format!("{error:?}").contains(REDACTION_SENTINEL));
            assert!(matches!(
                store.load_provider_cursor(STREAM_A),
                Ok(Some(value)) if value == FIRST_CURSOR
            ));
        }
        let invalid_next = vec![
            String::new(),
            " ".to_owned(),
            "stream\tid".to_owned(),
            "stream\nid".to_owned(),
            "é".to_owned(),
            "x".repeat(257),
            format!("{REDACTION_SENTINEL}\n"),
        ];
        for next in invalid_next {
            let error = store
                .advance_provider_cursor(STREAM_A, Some(FIRST_CURSOR), &next)
                .expect_err("invalid next cursor must fail");
            assert!(matches!(error, StoreError::InvalidInput { .. }));
            assert!(!error.to_string().contains(REDACTION_SENTINEL));
            assert!(!format!("{error:?}").contains(REDACTION_SENTINEL));
        }
        assert!(matches!(
            store.load_provider_cursor(STREAM_A),
            Ok(Some(value)) if value == FIRST_CURSOR
        ));

        let corrupt = PaStore::open_in_memory(DATABASE_KEY).expect("open corrupt store");
        corrupt
            .advance_provider_cursor(STREAM_A, None, FIRST_CURSOR)
            .expect("seed corrupt store");
        corrupt
            .connection()
            .execute(
                "UPDATE provider_cursors SET cursor = ?1 WHERE provider = ?2",
                rusqlite::params![format!("{REDACTION_SENTINEL}\n"), STREAM_A],
            )
            .expect("corrupt cursor fixture");
        let error = corrupt
            .load_provider_cursor(STREAM_A)
            .expect_err("corrupt cursor must fail closed");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "provider cursor"
            }
        ));
        assert!(!error.to_string().contains(REDACTION_SENTINEL));
        assert!(!format!("{error:?}").contains(REDACTION_SENTINEL));
    }

    #[test]
    fn provider_cursor_streams_are_isolated() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        assert!(matches!(store.load_provider_cursor(STREAM_A), Ok(None)));
        assert!(matches!(store.load_provider_cursor(STREAM_B), Ok(None)));
        store
            .advance_provider_cursor(STREAM_A, None, FIRST_CURSOR)
            .expect("stream A cursor");
        store
            .advance_provider_cursor(STREAM_B, None, FIRST_CURSOR)
            .expect("stream B cursor");

        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM provider_cursors", [], |row| {
                row.get(0)
            })
            .expect("cursor count");
        assert!(count == 2);
        assert!(matches!(
            store.load_provider_cursor(STREAM_A),
            Ok(Some(value)) if value == FIRST_CURSOR
        ));
        assert!(matches!(
            store.load_provider_cursor(STREAM_B),
            Ok(Some(value)) if value == FIRST_CURSOR
        ));
        assert!(matches!(
            store.advance_provider_cursor(STREAM_A, None, NEXT_CURSOR),
            Err(StoreError::CursorConflict { .. })
        ));
        assert!(
            matches!(store.load_provider_cursor(STREAM_B), Ok(Some(value)) if value == FIRST_CURSOR)
        );
    }

    #[test]
    fn oauth_credential_debug_redacts_tokens() {
        let credential = OAuthCredential::new(
            "google",
            "account-a",
            "access-token-value",
            Some("refresh-token-value".to_owned()),
            None,
            ["Mail.Read"],
        )
        .expect("valid credential");
        let debug = format!("{credential:?}");
        assert!(!debug.contains("access-token-value"));
        assert!(!debug.contains("refresh-token-value"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn oauth_identity_colons_are_rejected_to_prevent_aad_collisions() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let cipher = crate::pa::crypto::TokenCipher::new([0x41; 32]).expect("cipher");
        for (provider, account_id) in [("a:b", "c"), ("a", "b:c")] {
            let error = OAuthCredential::new(
                provider,
                account_id,
                "access-token-value",
                None,
                None,
                ["Mail.Read"],
            )
            .expect_err("colon in OAuth identity must fail");
            assert!(matches!(error, StoreError::InvalidInput { .. }));

            let raw = OAuthCredential {
                provider: provider.to_owned(),
                account_id: account_id.to_owned(),
                access_token: "access-token-value".to_owned(),
                refresh_token: None,
                expires_at: None,
                scopes: vec!["Mail.Read".to_owned()],
            };
            assert!(matches!(
                store.save_oauth_credential(&cipher, raw),
                Err(StoreError::InvalidInput { .. })
            ));
            assert!(matches!(
                store.load_oauth_credential(&cipher, provider, account_id),
                Err(StoreError::InvalidInput { .. })
            ));
        }
    }

    fn draft_time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid timestamp")
    }

    fn caller() -> CallerIdentity {
        CallerIdentity::new(
            "Ada Lovelace",
            ConfirmedEmail::confirm("ada.storage@example.com").expect("confirmed email"),
        )
        .expect("caller")
    }

    fn appointment(
        kind: AppointmentKind,
        starts_at: OffsetDateTime,
        quote_id: QuoteId,
        idempotency_key: &str,
        requester_included: bool,
    ) -> AppointmentDraft {
        AppointmentDraft::new_with_requester_inclusion(
            kind,
            caller(),
            starts_at,
            quote_id,
            IdempotencyKey::new(idempotency_key).expect("idempotency key"),
            requester_included,
        )
        .expect("appointment draft")
    }

    fn appointment_quote() -> (Quote, Vec<AppointmentSlot>) {
        let issued_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("quote issue time");
        let quote = Quote::with_id(
            QuoteId::from_uuid(
                uuid::Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").expect("quote id"),
            ),
            issued_at,
        );
        let slots = vec![
            AppointmentSlot::new(
                issued_at + TimeDuration::hours(1),
                issued_at + TimeDuration::hours(1) + AppointmentKind::Callback.duration(),
            )
            .expect("first slot"),
            AppointmentSlot::new(
                issued_at + TimeDuration::hours(3),
                issued_at + TimeDuration::hours(3) + AppointmentKind::Callback.duration(),
            )
            .expect("second slot"),
        ];
        (quote, slots)
    }

    fn strict_quote_fixture(consumed: bool) -> (PaStore, Quote, Vec<AppointmentSlot>) {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let draft = store
            .save_appointment_draft(
                "appointment:strict-quote",
                &appointment(
                    AppointmentKind::Callback,
                    slots[0].starts_at(),
                    quote.id(),
                    "strict-quote-draft",
                    false,
                ),
            )
            .expect("save matching draft");
        store
            .save_appointment_draft(
                "appointment:strict-quote-other",
                &appointment(
                    AppointmentKind::Callback,
                    slots[1].starts_at(),
                    QuoteId::new(),
                    "strict-quote-other-draft",
                    false,
                ),
            )
            .expect("save other draft");
        store
            .connection()
            .execute(
                "UPDATE appointment_quotes
                 SET state = 'prepared', appointment_draft_id = ?1, selected_slot_index = 0
                 WHERE quote_id = ?2",
                rusqlite::params![draft.id(), quote.id().to_string()],
            )
            .expect("prepare quote");

        if consumed {
            let proposal = store
                .create_proposal(
                    "strict-quote-proposal",
                    "proposal:strict-quote",
                    ProposalSource::appointment_draft(draft.id()),
                )
                .expect("create matching proposal");
            store
                .connection()
                .execute(
                    "UPDATE appointment_quotes
                     SET state = 'consumed', consumed_at = '2025-01-02T03:05:05Z', proposal_id = ?1
                     WHERE quote_id = ?2",
                    rusqlite::params![proposal.id(), quote.id().to_string()],
                )
                .expect("consume quote");
        }

        (store, quote, slots)
    }

    fn prepared_file_submit_fixture() -> (TempDatabase, Quote, i64) {
        let database = TempDatabase::new();
        let (quote, slots) = appointment_quote();
        let store = PaStore::open(&database.path, DATABASE_KEY).expect("open seed store");
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "appointment:submit-concurrent",
                &appointment(
                    AppointmentKind::Callback,
                    slots[0].starts_at(),
                    quote.id(),
                    "submit-concurrent-draft",
                    false,
                ),
                quote.issued_at(),
            )
            .expect("prepare quote");
        let draft_id = prepared.appointment_draft_id().expect("prepared draft id");
        drop(store);
        (database, quote, draft_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn concurrent_file_submissions(
        database: &TempDatabase,
        quote_id: QuoteId,
        first_draft_id: i64,
        first_key: &'static str,
        first_source: &'static str,
        second_draft_id: i64,
        second_key: &'static str,
        second_source: &'static str,
        now: OffsetDateTime,
    ) -> (StoreResult<StoredProposal>, StoreResult<StoredProposal>) {
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.submit_appointment_quote(quote_id, first_draft_id, first_key, first_source, now)
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.submit_appointment_quote(
                quote_id,
                second_draft_id,
                second_key,
                second_source,
                now,
            )
        });
        (
            first_handle.join().expect("first submit thread"),
            second_handle.join().expect("second submit thread"),
        )
    }

    #[test]
    fn appointment_quote_retries_and_loads_strict_prepared_and_consumed_aggregates() {
        for (consumed, expected_state) in [
            (false, StoredAppointmentQuoteState::Prepared),
            (true, StoredAppointmentQuoteState::Consumed),
        ] {
            let (store, quote, slots) = strict_quote_fixture(consumed);
            let expected = store
                .load_appointment_quote_by_id(quote.id())
                .expect("load strict aggregate");
            assert_eq!(expected.state(), expected_state);
            let draft_id = expected.appointment_draft_id().expect("linked draft");

            assert_eq!(
                store
                    .save_appointment_quote(
                        &quote,
                        AppointmentKind::Callback,
                        "Australia/Sydney",
                        &slots,
                    )
                    .expect("exact retry"),
                expected
            );
            assert_eq!(
                store
                    .load_appointment_quote_by_id(quote.id())
                    .expect("load by quote id"),
                expected
            );
            assert_eq!(
                store
                    .load_appointment_quote_by_draft_id(draft_id)
                    .expect("load by draft id"),
                expected
            );
        }
    }

    #[test]
    fn prepare_appointment_draft_from_quote_obeys_the_quote_validity_interval() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-validity",
            false,
        );

        for (now, expected) in [
            (
                quote.issued_at() - TimeDuration::seconds(1),
                StoreError::AppointmentQuoteNotYetValid,
            ),
            (quote.expires_at(), StoreError::AppointmentQuoteExpired),
        ] {
            let error = store
                .prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    "prepare:validity",
                    &draft,
                    now,
                )
                .expect_err("quote validity boundary rejects preparation");
            assert_eq!(error.to_string(), expected.to_string());
            assert!(!error.to_string().contains(&quote.id().to_string()));
            assert!(!format!("{error:?}").contains(&quote.id().to_string()));
            let drafts: i64 = store
                .connection()
                .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                    row.get(0)
                })
                .expect("count drafts");
            assert_eq!(drafts, 0);
        }

        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "prepare:validity",
                &draft,
                quote.expires_at() - TimeDuration::seconds(1),
            )
            .expect("one instant before expiry is valid");
        assert_eq!(prepared.state(), StoredAppointmentQuoteState::Prepared);
        assert_eq!(prepared.selected_slot_index(), Some(0));
        assert_eq!(prepared.draft(), Some(&draft));
    }

    #[test]
    fn prepare_appointment_draft_from_quote_rolls_back_when_draft_identity_conflicts() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let existing = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            QuoteId::new(),
            "prepare-existing",
            false,
        );
        store
            .save_appointment_draft("prepare:identity", &existing)
            .expect("existing draft");
        let requested = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-requested",
            false,
        );

        assert!(matches!(
            store.prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "prepare:identity",
                &requested,
                quote.issued_at(),
            ),
            Err(StoreError::Conflict {
                resource: "appointment draft"
            })
        ));
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("quote remains issued")
                .state(),
            StoredAppointmentQuoteState::Issued
        );
        let drafts: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                row.get(0)
            })
            .expect("count drafts");
        assert_eq!(drafts, 1);
    }

    #[test]
    fn prepare_appointment_draft_from_quote_rejects_unknown_and_invalid_slots_without_drafts() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-invalid-slot",
            false,
        );

        assert!(matches!(
            store.prepare_appointment_draft_from_quote(
                QuoteId::new(),
                0,
                "prepare:missing",
                &draft,
                quote.issued_at(),
            ),
            Err(StoreError::NotFound {
                resource: "appointment quote"
            })
        ));
        assert!(matches!(
            store.prepare_appointment_draft_from_quote(
                quote.id(),
                u32::MAX,
                "prepare:invalid-slot",
                &draft,
                quote.issued_at(),
            ),
            Err(StoreError::InvalidInput {
                field: "slot_index"
            })
        ));
        let drafts: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                row.get(0)
            })
            .expect("count drafts");
        assert_eq!(drafts, 0);
    }

    #[test]
    fn prepare_appointment_draft_from_quote_rejects_mismatched_drafts_without_orphans() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let mismatches = [
            appointment(
                AppointmentKind::Callback,
                slots[0].starts_at(),
                QuoteId::new(),
                "prepare-other-quote",
                false,
            ),
            appointment(
                AppointmentKind::Meeting,
                slots[0].starts_at(),
                quote.id(),
                "prepare-kind",
                false,
            ),
            appointment(
                AppointmentKind::Callback,
                slots[1].starts_at(),
                quote.id(),
                "prepare-time",
                false,
            ),
        ];

        for draft in mismatches {
            assert!(matches!(
                store.prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    "prepare:mismatch",
                    &draft,
                    quote.issued_at(),
                ),
                Err(StoreError::Conflict {
                    resource: "appointment quote"
                })
            ));
        }
        let drafts: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                row.get(0)
            })
            .expect("count drafts");
        assert_eq!(drafts, 0);
    }

    #[test]
    fn prepare_appointment_draft_from_quote_retries_prepared_and_consumed_aggregates_exactly() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-retry",
            true,
        );
        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "prepare:retry",
                &draft,
                quote.issued_at(),
            )
            .expect("prepare quote");
        let prepared_retry = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "prepare:retry",
                &draft,
                quote.expires_at(),
            )
            .expect("expired exact retry");
        assert_eq!(prepared_retry, prepared);

        let draft_id = prepared.appointment_draft_id().expect("prepared draft");
        let proposal = store
            .create_proposal(
                "prepare-retry-proposal",
                "proposal:prepare-retry",
                ProposalSource::appointment_draft(draft_id),
            )
            .expect("proposal");
        store
            .connection()
            .execute(
                "UPDATE appointment_quotes
                 SET state = 'consumed', consumed_at = '2025-01-02T03:05:05Z', proposal_id = ?1
                 WHERE quote_id = ?2",
                rusqlite::params![proposal.id(), quote.id().to_string()],
            )
            .expect("synthetic consumed quote");
        let consumed = store
            .load_appointment_quote_by_id(quote.id())
            .expect("load consumed quote");
        assert_eq!(
            store
                .prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    "prepare:retry",
                    &draft,
                    quote.expires_at(),
                )
                .expect("consumed exact retry"),
            consumed
        );
    }

    #[test]
    fn prepare_appointment_draft_from_quote_rejects_different_prepared_inputs_without_changes() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-conflict",
            false,
        );
        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "prepare:original",
                &draft,
                quote.issued_at(),
            )
            .expect("prepare quote");
        let changed_inclusion = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-conflict",
            true,
        );
        for (slot_index, source_id, candidate) in [
            (1, "prepare:original", draft.clone()),
            (0, "prepare:other", draft.clone()),
            (0, "prepare:original", changed_inclusion),
        ] {
            let error = store
                .prepare_appointment_draft_from_quote(
                    quote.id(),
                    slot_index,
                    source_id,
                    &candidate,
                    quote.expires_at(),
                )
                .expect_err("different retry conflicts");
            assert!(matches!(
                error,
                StoreError::Conflict {
                    resource: "appointment quote"
                }
            ));
            assert!(!error.to_string().contains("prepare:"));
            assert!(!format!("{error:?}").contains("prepare:"));
        }
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("prepared unchanged"),
            prepared
        );
    }

    #[test]
    fn prepare_appointment_draft_from_quote_conflicts_on_changed_caller_or_time() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-caller",
            false,
        );
        store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "prepare:caller",
                &draft,
                quote.issued_at(),
            )
            .expect("prepare quote");
        let changed_caller = AppointmentDraft::new_with_requester_inclusion(
            AppointmentKind::Callback,
            CallerIdentity::new(
                "Grace Hopper",
                ConfirmedEmail::confirm("grace.storage@example.com").expect("confirmed email"),
            )
            .expect("caller"),
            slots[0].starts_at(),
            quote.id(),
            IdempotencyKey::new("prepare-caller").expect("idempotency key"),
            false,
        )
        .expect("draft");
        let changed_time = appointment(
            AppointmentKind::Callback,
            slots[1].starts_at(),
            quote.id(),
            "prepare-caller",
            false,
        );
        for candidate in [changed_caller, changed_time] {
            assert!(matches!(
                store.prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    "prepare:caller",
                    &candidate,
                    quote.expires_at(),
                ),
                Err(StoreError::Conflict {
                    resource: "appointment quote"
                })
            ));
        }
    }

    #[test]
    fn prepare_appointment_draft_from_quote_prepared_retry_survives_store_reopen() {
        let database = TempDatabase::new();
        let (quote, slots) = appointment_quote();
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-reopen",
            false,
        );
        let prepared = {
            let store = PaStore::open(&database.path, DATABASE_KEY).expect("open file store");
            store
                .save_appointment_quote(
                    &quote,
                    AppointmentKind::Callback,
                    "Australia/Sydney",
                    &slots,
                )
                .expect("save quote");
            store
                .prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    "prepare:reopen",
                    &draft,
                    quote.issued_at(),
                )
                .expect("prepare quote")
        };
        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen file store");
        assert_eq!(
            reopened
                .prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    "prepare:reopen",
                    &draft,
                    quote.expires_at(),
                )
                .expect("reopened exact retry"),
            prepared
        );
    }

    #[test]
    fn prepare_appointment_draft_from_quote_identical_file_store_race_returns_one_prepared_aggregate()
     {
        let database = TempDatabase::new();
        let (quote, slots) = appointment_quote();
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "prepare-file-identical",
            false,
        );
        let before = {
            let store = PaStore::open(&database.path, DATABASE_KEY).expect("open seed store");
            store
                .save_appointment_quote(
                    &quote,
                    AppointmentKind::Callback,
                    "Australia/Sydney",
                    &slots,
                )
                .expect("save quote");
            appointment_quote_snapshot(&store, quote.id())
        };

        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let first_quote_id = quote.id();
        let second_quote_id = quote.id();
        let issued_at = quote.issued_at();
        let first_draft = draft.clone();
        let second_draft = draft.clone();
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.prepare_appointment_draft_from_quote(
                first_quote_id,
                0,
                "prepare:file-identical",
                &first_draft,
                issued_at,
            )
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.prepare_appointment_draft_from_quote(
                second_quote_id,
                0,
                "prepare:file-identical",
                &second_draft,
                issued_at,
            )
        });
        let first = first_handle
            .join()
            .expect("first prepare thread")
            .expect("first prepare");
        let second = second_handle
            .join()
            .expect("second prepare thread")
            .expect("second prepare");

        assert_eq!(first, second);
        assert_eq!(first.state(), StoredAppointmentQuoteState::Prepared);
        assert_eq!(first.selected_slot_index(), Some(0));
        assert_eq!(first.draft(), Some(&draft));
        let check = PaStore::open(&database.path, DATABASE_KEY).expect("open check store");
        let mut expected_after = before;
        expected_after[5] = Value::Text("prepared".to_owned());
        expected_after[6] = Value::Integer(first.appointment_draft_id().expect("draft id"));
        expected_after[7] = Value::Integer(0);
        assert_eq!(
            appointment_quote_snapshot(&check, quote.id()),
            expected_after
        );
        assert_eq!(
            check
                .connection()
                .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("appointment draft count"),
            1
        );
        assert_eq!(
            check
                .connection()
                .query_row(
                    "SELECT count(*) FROM appointment_quotes
                     WHERE quote_id = ?1 AND appointment_draft_id IS NOT NULL",
                    [quote.id().to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .expect("appointment quote binding count"),
            1
        );
        assert_eq!(
            check
                .load_appointment_quote_by_id(quote.id())
                .expect("load prepared aggregate"),
            first
        );
    }

    #[test]
    fn prepare_appointment_draft_from_quote_different_file_store_races_keep_one_winner_and_no_orphan()
     {
        for different_slot in [true, false] {
            let database = TempDatabase::new();
            let (quote, slots) = appointment_quote();
            let first_draft = appointment(
                AppointmentKind::Callback,
                slots[0].starts_at(),
                quote.id(),
                if different_slot {
                    "prepare-file-slot"
                } else {
                    "prepare-file-caller"
                },
                false,
            );
            let first_source = if different_slot {
                "prepare:file-slot"
            } else {
                "prepare:file-caller"
            };
            let (second_slot_index, second_source, second_draft) = if different_slot {
                (
                    1,
                    "prepare:file-slot",
                    appointment(
                        AppointmentKind::Callback,
                        slots[1].starts_at(),
                        quote.id(),
                        "prepare-file-slot",
                        false,
                    ),
                )
            } else {
                let caller = CallerIdentity::new(
                    "Grace Race Caller",
                    ConfirmedEmail::confirm("grace.race@example.com").expect("caller email"),
                )
                .expect("caller");
                (
                    0,
                    "prepare:file-caller-secret-source",
                    AppointmentDraft::new_with_requester_inclusion(
                        AppointmentKind::Callback,
                        caller,
                        slots[0].starts_at(),
                        quote.id(),
                        IdempotencyKey::new("prepare-file-caller-secret-key")
                            .expect("idempotency key"),
                        false,
                    )
                    .expect("appointment draft"),
                )
            };
            let seed = PaStore::open(&database.path, DATABASE_KEY).expect("open seed store");
            seed.save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
            drop(seed);

            let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
            let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let first_barrier = std::sync::Arc::clone(&barrier);
            let second_barrier = std::sync::Arc::clone(&barrier);
            let quote_id = quote.id();
            let issued_at = quote.issued_at();
            let first_draft_for_assert = first_draft.clone();
            let second_draft_for_assert = second_draft.clone();
            let first_handle = std::thread::spawn({
                let first_draft = first_draft.clone();
                move || {
                    first_barrier.wait();
                    first.prepare_appointment_draft_from_quote(
                        quote_id,
                        0,
                        first_source,
                        &first_draft,
                        issued_at,
                    )
                }
            });
            let second_handle = std::thread::spawn({
                let second_draft = second_draft.clone();
                move || {
                    second_barrier.wait();
                    second.prepare_appointment_draft_from_quote(
                        quote_id,
                        second_slot_index,
                        second_source,
                        &second_draft,
                        issued_at,
                    )
                }
            });
            let results = [
                first_handle.join().expect("first prepare thread"),
                second_handle.join().expect("second prepare thread"),
            ];
            assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
            assert_eq!(
                results
                    .iter()
                    .filter(|result| matches!(
                        result,
                        Err(StoreError::Conflict {
                            resource: "appointment quote"
                        })
                    ))
                    .count(),
                1
            );
            let winner = results
                .iter()
                .find_map(|result| result.as_ref().ok())
                .expect("one prepare winner");
            assert!(
                winner.draft() == Some(&first_draft_for_assert)
                    || winner.draft() == Some(&second_draft_for_assert),
                "quote binds one raced candidate"
            );

            let conflict = results
                .iter()
                .find_map(|result| result.as_ref().err())
                .expect("one prepare conflict");
            let display = conflict.to_string();
            let debug = format!("{conflict:?}");
            let first_start = first_draft_for_assert
                .starts_at()
                .format(&Rfc3339)
                .expect("first start");
            let second_start = second_draft_for_assert
                .starts_at()
                .format(&Rfc3339)
                .expect("second start");
            let quote_id = quote.id().to_string();
            for secret in [
                quote_id.as_str(),
                first_source,
                second_source,
                first_draft_for_assert.idempotency_key().as_str(),
                second_draft_for_assert.idempotency_key().as_str(),
                first_draft_for_assert.caller().name(),
                first_draft_for_assert.caller().email(),
                second_draft_for_assert.caller().name(),
                second_draft_for_assert.caller().email(),
                first_start.as_str(),
                second_start.as_str(),
            ] {
                assert!(!display.contains(secret), "display leaked sensitive value");
                assert!(!debug.contains(secret), "debug leaked sensitive value");
            }

            let check = PaStore::open(&database.path, DATABASE_KEY).expect("reopen winner store");
            assert_eq!(
                check
                    .load_appointment_quote_by_id(quote.id())
                    .expect("load winner quote"),
                *winner
            );
            assert_eq!(
                check
                    .connection()
                    .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .expect("one draft, no loser orphan"),
                1
            );
            assert_eq!(
                check
                    .connection()
                    .query_row(
                        "SELECT count(*) FROM appointment_quotes
                         WHERE quote_id = ?1 AND appointment_draft_id IS NOT NULL",
                        [quote.id().to_string()],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("one quote binding"),
                1
            );
        }
    }

    #[test]
    fn prepare_appointment_draft_from_quote_conflicts_on_every_changed_prepared_and_consumed_field()
    {
        for consumed in [false, true] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let (quote, slots) = appointment_quote();
            store
                .save_appointment_quote(
                    &quote,
                    AppointmentKind::Callback,
                    "Australia/Sydney",
                    &slots,
                )
                .expect("save quote");
            let base_source = "prepare:conflict-table";
            let base_draft = appointment(
                AppointmentKind::Callback,
                slots[0].starts_at(),
                quote.id(),
                "prepare-conflict-table",
                false,
            );
            let prepared = store
                .prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    base_source,
                    &base_draft,
                    quote.issued_at(),
                )
                .expect("prepare baseline quote");
            if consumed {
                let draft_id = prepared.appointment_draft_id().expect("prepared draft");
                let proposal = store
                    .create_proposal(
                        "prepare-conflict-table-proposal",
                        "proposal:prepare-conflict-table",
                        ProposalSource::appointment_draft(draft_id),
                    )
                    .expect("create synthetic proposal");
                store
                    .connection()
                    .execute(
                        "UPDATE appointment_quotes
                         SET state = 'consumed', consumed_at = '2025-01-02T03:05:05Z',
                             proposal_id = ?1
                         WHERE quote_id = ?2",
                        rusqlite::params![proposal.id(), quote.id().to_string()],
                    )
                    .expect("mark synthetic consumed quote");
            }
            let before = store
                .load_appointment_quote_by_id(quote.id())
                .expect("load baseline aggregate");
            let before_draft = before.draft().expect("baseline draft").clone();
            let before_draft_id = before.appointment_draft_id().expect("baseline draft id");
            let before_sql = appointment_quote_snapshot(&store, quote.id());

            struct ConflictCase {
                label: &'static str,
                slot_index: u32,
                source_id: &'static str,
                draft: AppointmentDraft,
            }
            let changed_name = CallerIdentity::new(
                "Grace Conflict Name",
                ConfirmedEmail::confirm("ada.storage@example.com").expect("email"),
            )
            .expect("caller");
            let changed_email = CallerIdentity::new(
                "Ada Lovelace",
                ConfirmedEmail::confirm("changed.conflict@example.com").expect("email"),
            )
            .expect("caller");
            let cases = [
                ConflictCase {
                    label: "slot",
                    slot_index: 1,
                    source_id: base_source,
                    draft: appointment(
                        AppointmentKind::Callback,
                        slots[1].starts_at(),
                        quote.id(),
                        "prepare-conflict-table",
                        false,
                    ),
                },
                ConflictCase {
                    label: "source",
                    slot_index: 0,
                    source_id: "prepare:changed-conflict-source",
                    draft: base_draft.clone(),
                },
                ConflictCase {
                    label: "idempotency key",
                    slot_index: 0,
                    source_id: base_source,
                    draft: appointment(
                        AppointmentKind::Callback,
                        slots[0].starts_at(),
                        quote.id(),
                        "prepare-changed-conflict-key",
                        false,
                    ),
                },
                ConflictCase {
                    label: "caller name",
                    slot_index: 0,
                    source_id: base_source,
                    draft: AppointmentDraft::new_with_requester_inclusion(
                        AppointmentKind::Callback,
                        changed_name,
                        slots[0].starts_at(),
                        quote.id(),
                        IdempotencyKey::new("prepare-conflict-table").expect("key"),
                        false,
                    )
                    .expect("appointment draft"),
                },
                ConflictCase {
                    label: "caller email",
                    slot_index: 0,
                    source_id: base_source,
                    draft: AppointmentDraft::new_with_requester_inclusion(
                        AppointmentKind::Callback,
                        changed_email,
                        slots[0].starts_at(),
                        quote.id(),
                        IdempotencyKey::new("prepare-conflict-table").expect("key"),
                        false,
                    )
                    .expect("appointment draft"),
                },
                ConflictCase {
                    label: "kind",
                    slot_index: 0,
                    source_id: base_source,
                    draft: appointment(
                        AppointmentKind::Meeting,
                        slots[0].starts_at(),
                        quote.id(),
                        "prepare-conflict-table",
                        false,
                    ),
                },
                ConflictCase {
                    label: "start",
                    slot_index: 0,
                    source_id: base_source,
                    draft: appointment(
                        AppointmentKind::Callback,
                        slots[0].starts_at() + TimeDuration::minutes(1),
                        quote.id(),
                        "prepare-conflict-table",
                        false,
                    ),
                },
                ConflictCase {
                    label: "requester inclusion",
                    slot_index: 0,
                    source_id: base_source,
                    draft: appointment(
                        AppointmentKind::Callback,
                        slots[0].starts_at(),
                        quote.id(),
                        "prepare-conflict-table",
                        true,
                    ),
                },
            ];

            for case in cases {
                let error = store
                    .prepare_appointment_draft_from_quote(
                        quote.id(),
                        case.slot_index,
                        case.source_id,
                        &case.draft,
                        quote.expires_at(),
                    )
                    .expect_err("changed prepared/consumed input conflicts");
                assert!(
                    matches!(
                        error,
                        StoreError::Conflict {
                            resource: "appointment quote"
                        }
                    ),
                    "{}: unexpected error {error:?}",
                    case.label
                );
                let display = error.to_string();
                let debug = format!("{error:?}");
                let quote_id = quote.id().to_string();
                let starts_at = case
                    .draft
                    .starts_at()
                    .format(&Rfc3339)
                    .expect("candidate start");
                for secret in [
                    quote_id.as_str(),
                    case.source_id,
                    case.draft.idempotency_key().as_str(),
                    case.draft.caller().name(),
                    case.draft.caller().email(),
                    starts_at.as_str(),
                ] {
                    assert!(
                        !display.contains(secret),
                        "{}: display leaked sensitive value",
                        case.label
                    );
                    assert!(
                        !debug.contains(secret),
                        "{}: debug leaked sensitive value",
                        case.label
                    );
                }
                let after = store
                    .load_appointment_quote_by_id(quote.id())
                    .expect("load unchanged aggregate");
                assert_eq!(after, before, "{}: aggregate changed", case.label);
                assert_eq!(after.draft(), Some(&before_draft));
                assert_eq!(after.appointment_draft_id(), Some(before_draft_id));
                assert_eq!(
                    store
                        .load_appointment_draft_by_id(before_draft_id)
                        .expect("load unchanged draft")
                        .draft(),
                    &before_draft
                );
                assert_eq!(appointment_quote_snapshot(&store, quote.id()), before_sql);
                assert_eq!(
                    store
                        .connection()
                        .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                            row.get::<_, i64>(0)
                        })
                        .expect("draft count"),
                    1
                );
            }
        }
    }

    #[test]
    fn submit_appointment_quote_consumes_a_live_prepared_quote_and_retries_unchanged_after_expiry()
    {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "submit-quote-draft",
            false,
        );
        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "appointment:submit-quote",
                &draft,
                quote.issued_at(),
            )
            .expect("prepare quote");
        let draft_id = prepared.appointment_draft_id().expect("prepared draft");
        let consumed_at = quote.expires_at() - TimeDuration::seconds(1);

        let proposal = store
            .submit_appointment_quote(
                quote.id(),
                draft_id,
                "submit-quote-key",
                "proposal:submit-quote",
                consumed_at,
            )
            .expect("consume quote");
        assert_eq!(proposal.state(), ProposalState::Pending);
        assert_eq!(
            proposal.source(),
            ProposalSource::appointment_draft(draft_id)
        );
        let consumed = store
            .load_appointment_quote_by_id(quote.id())
            .expect("load consumed quote");
        assert_eq!(consumed.state(), StoredAppointmentQuoteState::Consumed);
        assert_eq!(consumed.consumed_at(), Some(consumed_at));
        assert_eq!(consumed.proposal_id(), Some(proposal.id()));
        let before_quote = consumed.clone();
        let before_proposal = proposal.clone();

        assert_eq!(
            store
                .submit_appointment_quote(
                    quote.id(),
                    draft_id,
                    "submit-quote-key",
                    "proposal:submit-quote",
                    quote.expires_at(),
                )
                .expect("exact retry after expiry"),
            proposal
        );
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("quote unchanged"),
            before_quote
        );
        assert_eq!(
            store
                .load_proposal_by_id(proposal.id())
                .expect("proposal unchanged"),
            before_proposal
        );
    }

    #[test]
    fn submit_appointment_quote_rejects_an_unknown_quote_without_a_proposal() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        assert!(matches!(
            store.submit_appointment_quote(
                QuoteId::new(),
                1,
                "submit-unknown-key",
                "proposal:submit-unknown",
                draft_time(),
            ),
            Err(StoreError::NotFound {
                resource: "appointment quote"
            })
        ));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| row
                    .get::<_, i64>(0))
                .expect("proposal count"),
            0
        );
    }

    #[test]
    fn submit_appointment_quote_retries_before_expiry_without_changing_consumption() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "appointment:submit-before-expiry",
                &appointment(
                    AppointmentKind::Callback,
                    slots[0].starts_at(),
                    quote.id(),
                    "submit-before-expiry-draft",
                    false,
                ),
                quote.issued_at(),
            )
            .expect("prepare quote");
        let draft_id = prepared.appointment_draft_id().expect("prepared draft");
        let consumed_at = quote.issued_at() + TimeDuration::seconds(1);
        let proposal = store
            .submit_appointment_quote(
                quote.id(),
                draft_id,
                "submit-before-expiry-key",
                "proposal:submit-before-expiry",
                consumed_at,
            )
            .expect("consume quote");
        let before_quote = store
            .load_appointment_quote_by_id(quote.id())
            .expect("load consumed quote");
        let before_proposal = store
            .load_proposal_by_id(proposal.id())
            .expect("load proposal");

        assert_eq!(
            store
                .submit_appointment_quote(
                    quote.id(),
                    draft_id,
                    "submit-before-expiry-key",
                    "proposal:submit-before-expiry",
                    quote.issued_at() + TimeDuration::seconds(2),
                )
                .expect("exact retry before expiry"),
            proposal
        );
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("quote unchanged"),
            before_quote
        );
        assert_eq!(
            store
                .load_proposal_by_id(proposal.id())
                .expect("proposal unchanged"),
            before_proposal
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| row
                    .get::<_, i64>(0))
                .expect("proposal count"),
            1
        );
    }

    #[test]
    fn submit_appointment_quote_persists_consumption_and_exact_retry_across_reopen() {
        let database = TempDatabase::new();
        let (quote, slots) = appointment_quote();
        let (draft_id, proposal) = {
            let store = PaStore::open(&database.path, DATABASE_KEY).expect("open file store");
            store
                .save_appointment_quote(
                    &quote,
                    AppointmentKind::Callback,
                    "Australia/Sydney",
                    &slots,
                )
                .expect("save quote");
            let prepared = store
                .prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    "appointment:submit-reopen",
                    &appointment(
                        AppointmentKind::Callback,
                        slots[0].starts_at(),
                        quote.id(),
                        "submit-reopen-draft",
                        false,
                    ),
                    quote.issued_at(),
                )
                .expect("prepare quote");
            let draft_id = prepared.appointment_draft_id().expect("prepared draft");
            let proposal = store
                .submit_appointment_quote(
                    quote.id(),
                    draft_id,
                    "submit-reopen-key",
                    "proposal:submit-reopen",
                    quote.issued_at(),
                )
                .expect("consume quote");
            (draft_id, proposal)
        };

        let consumed = {
            let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen store");
            let quote_after_reopen = reopened
                .load_appointment_quote_by_id(quote.id())
                .expect("load consumed quote");
            assert_eq!(
                quote_after_reopen.state(),
                StoredAppointmentQuoteState::Consumed
            );
            assert_eq!(quote_after_reopen.proposal_id(), Some(proposal.id()));
            assert_eq!(
                reopened
                    .load_proposal_by_id(proposal.id())
                    .expect("load linked proposal"),
                proposal
            );
            assert_eq!(
                reopened
                    .submit_appointment_quote(
                        quote.id(),
                        draft_id,
                        "submit-reopen-key",
                        "proposal:submit-reopen",
                        quote.expires_at(),
                    )
                    .expect("reopened exact retry"),
                proposal
            );
            assert_eq!(
                reopened
                    .connection()
                    .query_row("SELECT count(*) FROM proposals", [], |row| row
                        .get::<_, i64>(0))
                    .expect("proposal count"),
                1
            );
            quote_after_reopen
        };
        let reopened_again = PaStore::open(&database.path, DATABASE_KEY).expect("reopen again");
        assert_eq!(
            reopened_again
                .load_appointment_quote_by_id(quote.id())
                .expect("strict quote after second reopen"),
            consumed
        );
    }

    #[test]
    fn submit_appointment_quote_identical_file_store_race_returns_one_stable_proposal() {
        let (database, quote, draft_id) = prepared_file_submit_fixture();
        let consumed_at = quote.issued_at() + TimeDuration::seconds(1);
        let (first, second) = concurrent_file_submissions(
            &database,
            quote.id(),
            draft_id,
            "submit-concurrent-key",
            "proposal:submit-concurrent",
            draft_id,
            "submit-concurrent-key",
            "proposal:submit-concurrent",
            consumed_at,
        );
        let first = first.expect("first identical submit");
        let second = second.expect("second identical submit");
        assert_eq!(first, second);
        assert_eq!(first.source(), ProposalSource::appointment_draft(draft_id));
        assert_eq!(first.state(), ProposalState::Pending);

        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen race store");
        let consumed = reopened
            .load_appointment_quote_by_id(quote.id())
            .expect("load consumed quote");
        assert_eq!(consumed.state(), StoredAppointmentQuoteState::Consumed);
        assert_eq!(consumed.appointment_draft_id(), Some(draft_id));
        assert_eq!(consumed.consumed_at(), Some(consumed_at));
        assert_eq!(consumed.proposal_id(), Some(first.id()));
        let quote_before_retry = appointment_quote_snapshot(&reopened, quote.id());
        let proposal_before_retry = reopened
            .load_proposal_by_id(first.id())
            .expect("load raced proposal");
        assert_eq!(proposal_before_retry, first);
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("one proposal row"),
            1
        );

        assert_eq!(
            reopened
                .submit_appointment_quote(
                    quote.id(),
                    draft_id,
                    "submit-concurrent-key",
                    "proposal:submit-concurrent",
                    quote.expires_at(),
                )
                .expect("exact retry after race and expiry"),
            first
        );
        assert_eq!(
            appointment_quote_snapshot(&reopened, quote.id()),
            quote_before_retry,
            "exact retry changed quote consumption or timestamps"
        );
        assert_eq!(
            reopened
                .load_proposal_by_id(first.id())
                .expect("proposal unchanged after retry"),
            proposal_before_retry,
            "exact retry changed proposal state or timestamps"
        );
        drop(reopened);

        let reopened_again = PaStore::open(&database.path, DATABASE_KEY).expect("reopen again");
        assert_eq!(
            reopened_again
                .load_appointment_quote_by_id(quote.id())
                .expect("quote persists after second reopen"),
            consumed
        );
        assert_eq!(
            reopened_again
                .load_proposal_by_id(first.id())
                .expect("proposal persists after second reopen"),
            proposal_before_retry
        );
    }

    #[test]
    fn submit_appointment_quote_conflicting_key_file_store_race_has_one_redacted_winner() {
        let (database, quote, draft_id) = prepared_file_submit_fixture();
        let consumed_at = quote.issued_at() + TimeDuration::seconds(1);
        let (first, second) = concurrent_file_submissions(
            &database,
            quote.id(),
            draft_id,
            "submit-concurrent-key-a",
            "proposal:submit-concurrent-key",
            draft_id,
            "submit-concurrent-key-b",
            "proposal:submit-concurrent-key",
            consumed_at,
        );
        let results = [first, second];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::Conflict {
                        resource: "appointment quote"
                    })
                ))
                .count(),
            1
        );
        let winner = results
            .iter()
            .find_map(|result| result.as_ref().ok())
            .expect("one proposal winner")
            .clone();
        let conflict = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("one proposal conflict");
        let display = conflict.to_string();
        let debug = format!("{conflict:?}");
        for secret in [
            "submit-concurrent-key-a",
            "submit-concurrent-key-b",
            "proposal:submit-concurrent-key",
        ] {
            assert!(!display.contains(secret), "display leaked sensitive value");
            assert!(!debug.contains(secret), "debug leaked sensitive value");
        }

        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen key race store");
        let consumed = reopened
            .load_appointment_quote_by_id(quote.id())
            .expect("load winning quote");
        assert_eq!(consumed.state(), StoredAppointmentQuoteState::Consumed);
        assert_eq!(consumed.consumed_at(), Some(consumed_at));
        assert_eq!(consumed.proposal_id(), Some(winner.id()));
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("one winning proposal row"),
            1
        );
        let quote_before_retry = appointment_quote_snapshot(&reopened, quote.id());
        let proposal_before_retry = reopened
            .load_proposal_by_id(winner.id())
            .expect("load winning proposal");
        assert_eq!(proposal_before_retry, winner);
        assert_eq!(
            reopened
                .submit_appointment_quote(
                    quote.id(),
                    draft_id,
                    winner.idempotency_key(),
                    winner.source_id(),
                    quote.expires_at(),
                )
                .expect("exact winning retry after expiry"),
            winner
        );
        assert_eq!(
            appointment_quote_snapshot(&reopened, quote.id()),
            quote_before_retry
        );
        assert_eq!(
            reopened
                .load_proposal_by_id(winner.id())
                .expect("winning proposal unchanged"),
            proposal_before_retry
        );
        drop(reopened);

        let reopened_again = PaStore::open(&database.path, DATABASE_KEY).expect("reopen key store");
        assert_eq!(
            reopened_again
                .load_appointment_quote_by_id(quote.id())
                .expect("quote after second reopen"),
            consumed
        );
        assert_eq!(
            reopened_again
                .load_proposal_by_id(winner.id())
                .expect("winner after second reopen"),
            proposal_before_retry
        );
    }

    #[test]
    fn submit_appointment_quote_conflicting_source_file_store_race_has_one_redacted_winner() {
        let (database, quote, draft_id) = prepared_file_submit_fixture();
        let consumed_at = quote.issued_at() + TimeDuration::seconds(1);
        let (first, second) = concurrent_file_submissions(
            &database,
            quote.id(),
            draft_id,
            "submit-concurrent-source-key",
            "proposal:submit-concurrent-source-a",
            draft_id,
            "submit-concurrent-source-key",
            "proposal:submit-concurrent-source-b",
            consumed_at,
        );
        let results = [first, second];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::Conflict {
                        resource: "appointment quote"
                    })
                ))
                .count(),
            1
        );
        let winner = results
            .iter()
            .find_map(|result| result.as_ref().ok())
            .expect("one proposal winner")
            .clone();
        let conflict = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("one proposal conflict");
        let display = conflict.to_string();
        let debug = format!("{conflict:?}");
        for secret in [
            "submit-concurrent-source-key",
            "proposal:submit-concurrent-source-a",
            "proposal:submit-concurrent-source-b",
        ] {
            assert!(!display.contains(secret), "display leaked sensitive value");
            assert!(!debug.contains(secret), "debug leaked sensitive value");
        }

        let reopened =
            PaStore::open(&database.path, DATABASE_KEY).expect("reopen source race store");
        let consumed = reopened
            .load_appointment_quote_by_id(quote.id())
            .expect("load winning quote");
        assert_eq!(consumed.state(), StoredAppointmentQuoteState::Consumed);
        assert_eq!(consumed.consumed_at(), Some(consumed_at));
        assert_eq!(consumed.proposal_id(), Some(winner.id()));
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("one winning proposal row"),
            1
        );
        let quote_before_retry = appointment_quote_snapshot(&reopened, quote.id());
        let proposal_before_retry = reopened
            .load_proposal_by_id(winner.id())
            .expect("load winning proposal");
        assert_eq!(proposal_before_retry, winner);
        assert_eq!(
            reopened
                .submit_appointment_quote(
                    quote.id(),
                    draft_id,
                    winner.idempotency_key(),
                    winner.source_id(),
                    quote.expires_at(),
                )
                .expect("exact winning retry after expiry"),
            winner
        );
        assert_eq!(
            appointment_quote_snapshot(&reopened, quote.id()),
            quote_before_retry
        );
        assert_eq!(
            reopened
                .load_proposal_by_id(winner.id())
                .expect("winning proposal unchanged"),
            proposal_before_retry
        );
        drop(reopened);

        let reopened_again =
            PaStore::open(&database.path, DATABASE_KEY).expect("reopen source store");
        assert_eq!(
            reopened_again
                .load_appointment_quote_by_id(quote.id())
                .expect("quote after second reopen"),
            consumed
        );
        assert_eq!(
            reopened_again
                .load_proposal_by_id(winner.id())
                .expect("winner after second reopen"),
            proposal_before_retry
        );
    }

    #[test]
    fn submit_appointment_quote_mismatched_draft_file_store_race_has_one_redacted_conflict() {
        let (database, quote, draft_id) = prepared_file_submit_fixture();
        let consumed_at = quote.issued_at() + TimeDuration::seconds(1);
        let (first, second) = concurrent_file_submissions(
            &database,
            quote.id(),
            draft_id,
            "submit-concurrent-draft-key",
            "proposal:submit-concurrent-draft",
            draft_id + 1,
            "submit-concurrent-draft-key",
            "proposal:submit-concurrent-draft",
            consumed_at,
        );
        let results = [first, second];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::Conflict {
                        resource: "appointment quote"
                    })
                ))
                .count(),
            1
        );
        let winner = results
            .iter()
            .find_map(|result| result.as_ref().ok())
            .expect("one proposal winner")
            .clone();
        let conflict = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("one proposal conflict");
        assert!(!conflict.to_string().contains("submit-concurrent-draft"));
        assert!(!format!("{conflict:?}").contains("submit-concurrent-draft"));

        let reopened =
            PaStore::open(&database.path, DATABASE_KEY).expect("reopen draft race store");
        let consumed = reopened
            .load_appointment_quote_by_id(quote.id())
            .expect("load winning quote");
        assert_eq!(consumed.state(), StoredAppointmentQuoteState::Consumed);
        assert_eq!(consumed.appointment_draft_id(), Some(draft_id));
        assert_eq!(consumed.consumed_at(), Some(consumed_at));
        assert_eq!(consumed.proposal_id(), Some(winner.id()));
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("one winning proposal row"),
            1
        );
        let quote_before_retry = appointment_quote_snapshot(&reopened, quote.id());
        let proposal_before_retry = reopened
            .load_proposal_by_id(winner.id())
            .expect("load winning proposal");
        assert_eq!(proposal_before_retry, winner);
        assert_eq!(
            reopened
                .submit_appointment_quote(
                    quote.id(),
                    draft_id,
                    winner.idempotency_key(),
                    winner.source_id(),
                    quote.expires_at(),
                )
                .expect("exact winning retry after expiry"),
            winner
        );
        assert_eq!(
            appointment_quote_snapshot(&reopened, quote.id()),
            quote_before_retry
        );
        assert_eq!(
            reopened
                .load_proposal_by_id(winner.id())
                .expect("winning proposal unchanged"),
            proposal_before_retry
        );
        drop(reopened);

        let reopened_again =
            PaStore::open(&database.path, DATABASE_KEY).expect("reopen draft store");
        assert_eq!(
            reopened_again
                .load_appointment_quote_by_id(quote.id())
                .expect("quote after second reopen"),
            consumed
        );
        assert_eq!(
            reopened_again
                .load_proposal_by_id(winner.id())
                .expect("winner after second reopen"),
            proposal_before_retry
        );
    }

    #[test]
    fn submit_appointment_quote_rejects_invalid_or_conflicting_inputs_without_proposals() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "submit-invalid-draft",
            false,
        );
        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "appointment:submit-invalid",
                &draft,
                quote.issued_at(),
            )
            .expect("prepare quote");
        let draft_id = prepared.appointment_draft_id().expect("prepared draft");

        for (draft_id, now, expected) in [
            (0, quote.issued_at(), "invalid"),
            (draft_id + 1, quote.issued_at(), "conflict"),
            (
                draft_id,
                quote.issued_at() - TimeDuration::seconds(1),
                "not yet valid",
            ),
            (draft_id, quote.expires_at(), "expired"),
        ] {
            let error = store
                .submit_appointment_quote(
                    quote.id(),
                    draft_id,
                    "submit-invalid-key",
                    "proposal:submit-invalid",
                    now,
                )
                .expect_err(expected);
            assert!(
                matches!(
                    error,
                    StoreError::InvalidInput {
                        field: "appointment_draft_id"
                    } | StoreError::Conflict {
                        resource: "appointment quote"
                    } | StoreError::AppointmentQuoteNotYetValid
                        | StoreError::AppointmentQuoteExpired
                ),
                "unexpected error: {error:?}"
            );
            assert!(!error.to_string().contains("proposal:submit-invalid"));
            assert!(!format!("{error:?}").contains("proposal:submit-invalid"));
        }
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("prepared quote remains")
                .state(),
            StoredAppointmentQuoteState::Prepared
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| row
                    .get::<_, i64>(0))
                .expect("proposal count"),
            0
        );
    }

    #[test]
    fn submit_appointment_quote_rejects_issued_and_changed_consumed_identities() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        assert!(matches!(
            store.submit_appointment_quote(
                quote.id(),
                1,
                "submit-issued-key",
                "proposal:submit-issued",
                quote.issued_at(),
            ),
            Err(StoreError::Conflict {
                resource: "appointment quote"
            })
        ));
        let draft = appointment(
            AppointmentKind::Callback,
            slots[0].starts_at(),
            quote.id(),
            "submit-conflict-draft",
            false,
        );
        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "appointment:submit-conflict",
                &draft,
                quote.issued_at(),
            )
            .expect("prepare quote");
        let draft_id = prepared.appointment_draft_id().expect("prepared draft");
        let proposal = store
            .submit_appointment_quote(
                quote.id(),
                draft_id,
                "submit-conflict-key",
                "proposal:submit-conflict",
                quote.issued_at(),
            )
            .expect("consume quote");
        let before_quote = store
            .load_appointment_quote_by_id(quote.id())
            .expect("load consumed quote");
        for (key, source) in [
            ("submit-conflict-changed-key", "proposal:submit-conflict"),
            (
                "submit-conflict-key",
                "proposal:submit-conflict-changed-source",
            ),
        ] {
            let error = store
                .submit_appointment_quote(quote.id(), draft_id, key, source, quote.expires_at())
                .expect_err("changed consumed identity conflicts");
            assert!(matches!(
                error,
                StoreError::Conflict {
                    resource: "appointment quote"
                }
            ));
            assert!(!error.to_string().contains(key));
            assert!(!error.to_string().contains(source));
        }
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("quote unchanged"),
            before_quote
        );
        assert_eq!(
            store
                .load_proposal_by_id(proposal.id())
                .expect("proposal unchanged"),
            proposal
        );
    }

    #[test]
    fn submit_appointment_quote_rolls_back_proposal_conflicts_and_quote_cas_failures() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        let prepared = store
            .prepare_appointment_draft_from_quote(
                quote.id(),
                0,
                "appointment:submit-rollback",
                &appointment(
                    AppointmentKind::Callback,
                    slots[0].starts_at(),
                    quote.id(),
                    "submit-rollback-draft",
                    false,
                ),
                quote.issued_at(),
            )
            .expect("prepare quote");
        let draft_id = prepared.appointment_draft_id().expect("prepared draft");
        let other_draft = store
            .save_appointment_draft(
                "appointment:submit-rollback-other",
                &appointment(
                    AppointmentKind::Callback,
                    slots[1].starts_at(),
                    QuoteId::new(),
                    "submit-rollback-other-draft",
                    false,
                ),
            )
            .expect("save other draft");
        store
            .create_proposal(
                "submit-rollback-key",
                "proposal:submit-rollback-other",
                ProposalSource::appointment_draft(other_draft.id()),
            )
            .expect("reserve conflicting proposal key");
        assert!(matches!(
            store.submit_appointment_quote(
                quote.id(),
                draft_id,
                "submit-rollback-key",
                "proposal:submit-rollback",
                quote.issued_at(),
            ),
            Err(StoreError::Conflict {
                resource: "proposal"
            })
        ));
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("quote remains prepared")
                .state(),
            StoredAppointmentQuoteState::Prepared
        );

        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER reject_quote_consumption
                 BEFORE UPDATE OF state ON appointment_quotes
                 WHEN NEW.state = 'consumed'
                 BEGIN SELECT RAISE(ABORT, 'forced quote CAS failure'); END;",
            )
            .expect("install failure trigger");
        let error = store
            .submit_appointment_quote(
                quote.id(),
                draft_id,
                "submit-rollback-cas-key",
                "proposal:submit-rollback-cas",
                quote.issued_at(),
            )
            .expect_err("quote CAS failure must roll back proposal");
        assert!(!error.to_string().contains("submit-rollback-cas"));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| row
                    .get::<_, i64>(0))
                .expect("no orphan proposal"),
            1
        );
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("quote remains prepared")
                .state(),
            StoredAppointmentQuoteState::Prepared
        );
    }

    #[test]
    fn submit_appointment_quote_only_binds_a_pending_exact_identity_proposal() {
        for terminal_state in [
            Some(ProposalState::Accepted),
            Some(ProposalState::Declined),
            Some(ProposalState::Expired),
            None,
        ] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let (quote, slots) = appointment_quote();
            store
                .save_appointment_quote(
                    &quote,
                    AppointmentKind::Callback,
                    "Australia/Sydney",
                    &slots,
                )
                .expect("save quote");
            let prepared = store
                .prepare_appointment_draft_from_quote(
                    quote.id(),
                    0,
                    "appointment:submit-terminal",
                    &appointment(
                        AppointmentKind::Callback,
                        slots[0].starts_at(),
                        quote.id(),
                        "submit-terminal-draft",
                        false,
                    ),
                    quote.issued_at(),
                )
                .expect("prepare quote");
            let draft_id = prepared.appointment_draft_id().expect("prepared draft");
            let proposal = store
                .create_proposal(
                    "submit-terminal-key",
                    "proposal:submit-terminal",
                    ProposalSource::appointment_draft(draft_id),
                )
                .expect("create proposal");
            let proposal = match terminal_state {
                Some(state) => store
                    .transition_proposal(proposal.id(), state)
                    .expect("make terminal proposal"),
                None => proposal,
            };
            let before_proposal = proposal.clone();

            let result = store.submit_appointment_quote(
                quote.id(),
                draft_id,
                "submit-terminal-key",
                "proposal:submit-terminal",
                quote.issued_at(),
            );
            match terminal_state {
                Some(_) => assert!(matches!(
                    result,
                    Err(StoreError::Conflict {
                        resource: "appointment quote"
                    })
                )),
                None => assert_eq!(result.expect("pending proposal binds"), proposal),
            }
            assert_eq!(
                store
                    .load_proposal_by_id(proposal.id())
                    .expect("proposal preserved"),
                before_proposal
            );
            let current_quote = store
                .load_appointment_quote_by_id(quote.id())
                .expect("load quote");
            assert_eq!(
                current_quote.state(),
                if terminal_state.is_some() {
                    StoredAppointmentQuoteState::Prepared
                } else {
                    StoredAppointmentQuoteState::Consumed
                }
            );
            assert_eq!(
                store
                    .connection()
                    .query_row("SELECT count(*) FROM proposals", [], |row| row
                        .get::<_, i64>(0))
                    .expect("one proposal"),
                1
            );
        }
    }

    #[test]
    fn appointment_quote_strict_read_rejects_in_range_slot_count_mismatch() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .expect("allow corruption fixture");
        store
            .connection()
            .execute(
                "UPDATE appointment_quotes SET slot_count = 1 WHERE quote_id = ?1",
                [quote.id().to_string()],
            )
            .expect("corrupt in-range slot count");

        assert!(matches!(
            store.load_appointment_quote_by_id(quote.id()),
            Err(StoreError::StoredRecordInvalid {
                resource: "appointment quote"
            })
        ));
    }

    #[test]
    fn appointment_quote_strict_reads_redact_each_sqlite_corruption_shape() {
        let cases = [
            (
                "expiry",
                "UPDATE appointment_quotes SET expires_at = '2025-01-02T03:09:06Z' WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "timezone",
                "UPDATE appointment_quotes SET timezone = 'secret/timezone' WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "unknown kind",
                "UPDATE appointment_quotes SET appointment_kind = 'secret-kind' WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "unknown state",
                "UPDATE appointment_quotes SET state = 'secret-state' WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "issued draft link",
                "UPDATE appointment_quotes SET state = 'issued', appointment_draft_id = 1, selected_slot_index = NULL, consumed_at = NULL, proposal_id = NULL WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "issued selected link",
                "UPDATE appointment_quotes SET state = 'issued', appointment_draft_id = NULL, selected_slot_index = 0, consumed_at = NULL, proposal_id = NULL WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "issued consumed link",
                "UPDATE appointment_quotes SET state = 'issued', appointment_draft_id = NULL, selected_slot_index = NULL, consumed_at = '2025-01-02T03:05:05Z', proposal_id = NULL WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "issued proposal link",
                "UPDATE appointment_quotes SET state = 'issued', appointment_draft_id = NULL, selected_slot_index = NULL, consumed_at = NULL, proposal_id = 1 WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "prepared missing draft",
                "UPDATE appointment_quotes SET state = 'prepared', appointment_draft_id = NULL, selected_slot_index = 0, consumed_at = NULL, proposal_id = NULL WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "prepared missing selection",
                "UPDATE appointment_quotes SET state = 'prepared', appointment_draft_id = 1, selected_slot_index = NULL, consumed_at = NULL, proposal_id = NULL WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "prepared consumed link",
                "UPDATE appointment_quotes SET state = 'prepared', appointment_draft_id = 1, selected_slot_index = 0, consumed_at = '2025-01-02T03:05:05Z', proposal_id = NULL WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "prepared proposal link",
                "UPDATE appointment_quotes SET state = 'prepared', appointment_draft_id = 1, selected_slot_index = 0, consumed_at = NULL, proposal_id = 1 WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "consumed missing draft",
                "UPDATE appointment_quotes SET appointment_draft_id = NULL, selected_slot_index = 0, consumed_at = '2025-01-02T03:05:05Z', proposal_id = 1 WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "consumed missing selection",
                "UPDATE appointment_quotes SET appointment_draft_id = 1, selected_slot_index = NULL, consumed_at = '2025-01-02T03:05:05Z', proposal_id = 1 WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "consumed missing timestamp",
                "UPDATE appointment_quotes SET appointment_draft_id = 1, selected_slot_index = 0, consumed_at = NULL, proposal_id = 1 WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "consumed missing proposal",
                "UPDATE appointment_quotes SET appointment_draft_id = 1, selected_slot_index = 0, consumed_at = '2025-01-02T03:05:05Z', proposal_id = NULL WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "selected out of range",
                "UPDATE appointment_quotes SET state = 'prepared', proposal_id = NULL, consumed_at = NULL, selected_slot_index = 99 WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "selected fractional",
                "UPDATE appointment_quotes SET state = 'prepared', proposal_id = NULL, consumed_at = NULL, selected_slot_index = 0.5 WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'",
            ),
            (
                "noncontiguous slots",
                "DELETE FROM appointment_quote_slots WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa' AND slot_index = 0",
            ),
            (
                "malformed slot",
                "UPDATE appointment_quote_slots SET starts_at = 'secret-time' WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa' AND slot_index = 0",
            ),
            (
                "equal slot",
                "UPDATE appointment_quote_slots SET ends_at = starts_at WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa' AND slot_index = 0",
            ),
            (
                "reversed slot",
                "UPDATE appointment_quote_slots SET ends_at = '2025-01-02T03:59:05Z' WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa' AND slot_index = 0",
            ),
            (
                "wrong slot duration",
                "UPDATE appointment_quote_slots SET ends_at = '2025-01-02T04:20:05Z' WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa' AND slot_index = 0",
            ),
            (
                "extra slot",
                "INSERT INTO appointment_quote_slots (quote_id, slot_index, starts_at, ends_at) VALUES ('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 2, '2025-01-02T08:04:05Z', '2025-01-02T08:19:05Z')",
            ),
            (
                "draft quote mismatch",
                "UPDATE appointment_drafts SET quote_id = 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb' WHERE id = 1",
            ),
            (
                "draft kind mismatch",
                "UPDATE appointment_drafts SET kind = 'meeting' WHERE id = 1",
            ),
            (
                "draft start mismatch",
                "UPDATE appointment_drafts SET starts_at = '2025-01-02T05:04:05Z', ends_at = '2025-01-02T05:19:05Z' WHERE id = 1",
            ),
            (
                "draft end mismatch",
                "UPDATE appointment_drafts SET ends_at = '2025-01-02T04:20:05Z' WHERE id = 1",
            ),
            ("proposal missing", "DELETE FROM proposals WHERE id = 1"),
            (
                "proposal wrong source draft",
                "UPDATE proposals SET appointment_draft_id = 2 WHERE id = 1",
            ),
        ];

        for (name, corruption) in cases {
            let (store, quote, _) = strict_quote_fixture(true);
            store
                .connection()
                .execute_batch("PRAGMA foreign_keys = OFF; PRAGMA ignore_check_constraints = ON;")
                .expect("allow corruption fixture");
            store
                .connection()
                .execute_batch(corruption)
                .unwrap_or_else(|error| panic!("{name}: corrupt fixture: {error}"));
            let error = store
                .load_appointment_quote_by_id(quote.id())
                .expect_err("corruption must fail");
            assert!(
                matches!(
                    error,
                    StoreError::StoredRecordInvalid {
                        resource: "appointment quote"
                    }
                ),
                "{name}: unexpected error {error:?}"
            );
            assert!(
                !error.to_string().contains("secret"),
                "{name}: display leaked corruption data"
            );
            assert!(
                !format!("{error:?}").contains("secret"),
                "{name}: debug leaked corruption data"
            );
        }
    }

    #[test]
    fn appointment_quote_near_maximum_issued_time_fails_closed_without_panicking() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        store
            .connection()
            .execute_batch(
                "PRAGMA ignore_check_constraints = ON;
                 UPDATE appointment_quotes
                 SET issued_at = '9999-12-31T23:59:59Z',
                     expires_at = '9999-12-31T23:59:59Z'
                 WHERE quote_id = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa';",
            )
            .expect("install canonical overflow corruption");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.load_appointment_quote_by_id(quote.id())
        }));
        assert!(result.is_ok(), "overflow corruption must not panic");
        let error = result
            .expect("no panic")
            .expect_err("overflow corruption must fail");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "appointment quote"
            }
        ));
        assert!(!error.to_string().contains("9999-12-31T23:59:59Z"));
        assert!(!format!("{error:?}").contains("9999-12-31T23:59:59Z"));
    }

    #[test]
    fn appointment_quote_repository_persists_and_strictly_loads_ordered_slots() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();

        let saved = store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");

        assert_eq!(saved.quote(), &quote);
        assert_eq!(saved.timezone(), "Australia/Sydney");
        assert_eq!(saved.offered_slots(), slots);
        assert_eq!(saved.state(), StoredAppointmentQuoteState::Issued);
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("load quote"),
            saved
        );
    }

    #[test]
    fn appointment_quote_repository_retries_immutably_and_validates_inputs() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        let first = store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("first save");
        assert_eq!(
            store
                .save_appointment_quote(
                    &quote,
                    AppointmentKind::Callback,
                    "Australia/Sydney",
                    &slots,
                )
                .expect("exact retry"),
            first
        );
        let meeting_slots = vec![
            AppointmentSlot::new(
                slots[0].starts_at(),
                slots[0].starts_at() + AppointmentKind::Meeting.duration(),
            )
            .expect("meeting slot"),
        ];
        for (kind, timezone, offered_slots) in [
            (AppointmentKind::Meeting, "Australia/Sydney", meeting_slots),
            (AppointmentKind::Callback, "UTC", slots.clone()),
            (
                AppointmentKind::Callback,
                "Australia/Sydney",
                vec![slots[1], slots[0]],
            ),
            (
                AppointmentKind::Callback,
                "Australia/Sydney",
                vec![slots[0]],
            ),
            (
                AppointmentKind::Callback,
                "Australia/Sydney",
                vec![
                    AppointmentSlot::new(
                        slots[0].starts_at() + TimeDuration::hours(6),
                        slots[0].ends_at() + TimeDuration::hours(6),
                    )
                    .expect("changed interval"),
                    slots[1],
                ],
            ),
        ] {
            assert!(matches!(
                store.save_appointment_quote(&quote, kind, timezone, &offered_slots),
                Err(StoreError::Conflict {
                    resource: "appointment quote"
                })
            ));
            assert_eq!(
                store
                    .load_appointment_quote_by_id(quote.id())
                    .expect("original remains"),
                first
            );
        }
        let changed_time = Quote::with_id(quote.id(), quote.issued_at() + TimeDuration::seconds(1));
        assert!(matches!(
            store.save_appointment_quote(
                &changed_time,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            ),
            Err(StoreError::Conflict {
                resource: "appointment quote"
            })
        ));
        assert_eq!(
            store
                .load_appointment_quote_by_id(quote.id())
                .expect("original remains"),
            first
        );
        let duplicate = vec![slots[0], slots[0]];
        let wrong_duration = vec![
            AppointmentSlot::new(
                slots[0].starts_at(),
                slots[0].ends_at() + TimeDuration::minutes(1),
            )
            .expect("wrong duration slot"),
        ];
        let too_many = vec![slots[0]; MAX_APPOINTMENT_QUOTE_SLOTS + 1];
        for (timezone, offered_slots) in [
            (" ", Vec::new()),
            ("Not/A_Timezone", slots.clone()),
            ("Australia/Sydney", Vec::new()),
            ("Australia/Sydney", duplicate),
            ("Australia/Sydney", wrong_duration),
            ("Australia/Sydney", too_many),
        ] {
            assert!(matches!(
                store.save_appointment_quote(
                    &Quote::new(quote.issued_at() + TimeDuration::hours(10)),
                    AppointmentKind::Callback,
                    timezone,
                    &offered_slots,
                ),
                Err(StoreError::InvalidInput { .. })
            ));
            assert_eq!(
                store
                    .load_appointment_quote_by_id(quote.id())
                    .expect("original remains"),
                first
            );
        }
        assert!(matches!(
            store.load_appointment_quote_by_draft_id(0),
            Err(StoreError::InvalidInput { .. })
        ));
        assert!(matches!(
            store.load_appointment_quote_by_draft_id(42),
            Err(StoreError::NotFound {
                resource: "appointment quote"
            })
        ));
    }

    #[test]
    fn appointment_quote_repository_rejects_corrupt_rows_without_disclosing_values() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .expect("allow corruption fixture");
        store
            .connection()
            .execute(
                "UPDATE appointment_quotes SET expires_at = '2025-01-02T03:09:06Z', timezone = 'secret/timezone' WHERE quote_id = ?1",
                [quote.id().to_string()],
            )
            .expect("corrupt quote");
        let error = store
            .load_appointment_quote_by_id(quote.id())
            .expect_err("corrupt quote must fail");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "appointment quote"
            }
        ));
        assert!(!error.to_string().contains("secret/timezone"));
        assert!(!format!("{error:?}").contains("secret/timezone"));
    }

    #[test]
    fn appointment_quote_lookup_rejects_case_variant_stored_identity_as_corruption() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            )
            .expect("save quote");
        store
            .connection()
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .expect("allow corruption fixture");
        store
            .connection()
            .execute(
                "UPDATE appointment_quotes SET quote_id = upper(quote_id) WHERE quote_id = ?1",
                [quote.id().to_string()],
            )
            .expect("corrupt identity");

        assert!(matches!(
            store.load_appointment_quote_by_id(quote.id()),
            Err(StoreError::StoredRecordInvalid {
                resource: "appointment quote"
            })
        ));
    }

    #[test]
    fn appointment_quote_repository_persists_across_file_store_reopen() {
        let database = TempDatabase::new();
        let (quote, slots) = appointment_quote();
        let saved = {
            let store = PaStore::open(&database.path, DATABASE_KEY).expect("open file store");
            store
                .save_appointment_quote(
                    &quote,
                    AppointmentKind::Callback,
                    "Australia/Sydney",
                    &slots,
                )
                .expect("save quote")
        };
        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen file store");
        assert_eq!(
            reopened
                .load_appointment_quote_by_id(quote.id())
                .expect("load reopened quote"),
            saved
        );
    }

    #[test]
    fn appointment_quote_identical_file_store_race_returns_one_aggregate() {
        let database = TempDatabase::new();
        let (quote, slots) = appointment_quote();
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let first_quote = quote.clone();
        let first_slots = slots.clone();
        let second_quote = quote.clone();
        let second_slots = slots.clone();
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.save_appointment_quote(
                &first_quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &first_slots,
            )
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.save_appointment_quote(
                &second_quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &second_slots,
            )
        });
        let first = first_handle
            .join()
            .expect("first save panicked")
            .expect("first save");
        let second = second_handle
            .join()
            .expect("second save panicked")
            .expect("second save");
        assert_eq!(first, second);
        let check = PaStore::open(&database.path, DATABASE_KEY).expect("open check store");
        assert_eq!(
            check
                .connection()
                .query_row("SELECT count(*) FROM appointment_quotes", [], |row| row
                    .get::<_, i64>(0))
                .expect("parent count"),
            1
        );
        assert_eq!(
            check
                .connection()
                .query_row("SELECT count(*) FROM appointment_quote_slots", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("slot count"),
            i64::try_from(slots.len()).expect("slot count fits")
        );
    }

    #[test]
    fn appointment_quote_conflicting_file_store_race_has_one_winner_without_mixed_slots() {
        let database = TempDatabase::new();
        let (quote, slots) = appointment_quote();
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let first_quote = quote.clone();
        let first_slots = slots.clone();
        let second_quote = quote.clone();
        let second_slots = slots.clone();
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.save_appointment_quote(
                &first_quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &first_slots,
            )
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.save_appointment_quote(
                &second_quote,
                AppointmentKind::Callback,
                "UTC",
                &second_slots,
            )
        });
        let results = [
            first_handle.join().expect("first save panicked"),
            second_handle.join().expect("second save panicked"),
        ];
        let winner = results
            .iter()
            .find_map(|result| result.as_ref().ok())
            .expect("one winner");
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::Conflict {
                        resource: "appointment quote"
                    })
                ))
                .count(),
            1
        );
        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen store");
        assert_eq!(
            reopened
                .load_appointment_quote_by_id(quote.id())
                .expect("winner row"),
            *winner
        );
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM appointment_quotes", [], |row| row
                    .get::<_, i64>(0))
                .expect("parent count"),
            1
        );
    }

    #[test]
    fn appointment_quote_slot_constraint_failure_rolls_back_parent_and_slots() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (quote, slots) = appointment_quote();
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER reject_second_quote_slot
                 BEFORE INSERT ON appointment_quote_slots
                 WHEN NEW.slot_index = 1
                 BEGIN SELECT RAISE(ABORT, 'quote slot constraint'); END;",
            )
            .expect("install constraint fixture");
        assert!(matches!(
            store.save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "Australia/Sydney",
                &slots,
            ),
            Err(StoreError::Sqlite(_))
        ));
        for table in ["appointment_quotes", "appointment_quote_slots"] {
            assert_eq!(
                store
                    .connection()
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .expect("row count"),
                0,
                "{table} rolls back"
            );
        }
    }

    fn owner_task(
        kind: TaskKind,
        title: &str,
        duration_minutes: u32,
        due_at: Option<OffsetDateTime>,
        idempotency_key: &str,
    ) -> OwnerTaskDraft {
        OwnerTaskDraft::with_duration(
            kind,
            title,
            duration_minutes,
            due_at,
            IdempotencyKey::new(idempotency_key).expect("idempotency key"),
        )
        .expect("owner task draft")
    }

    #[test]
    fn appointment_draft_repository_round_trips_all_immutable_fields() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let callback = appointment(
            AppointmentKind::Callback,
            draft_time(),
            QuoteId::new(),
            "appointment-callback",
            false,
        );
        let meeting = appointment(
            AppointmentKind::Meeting,
            draft_time() + TimeDuration::hours(1),
            QuoteId::new(),
            "appointment-meeting",
            false,
        );

        for (source_id, draft) in [("phone:callback-1", callback), ("phone:meeting-1", meeting)] {
            let stored = store
                .save_appointment_draft(source_id, &draft)
                .expect("save appointment draft");
            assert_eq!(stored.source_id(), source_id);
            assert_eq!(stored.draft(), &draft);
            assert_eq!(
                store
                    .load_appointment_draft_by_id(stored.id())
                    .expect("load by id")
                    .draft(),
                &draft
            );
            assert_eq!(
                store
                    .load_appointment_draft_by_idempotency_key(draft.idempotency_key().as_str())
                    .expect("load by idempotency key")
                    .draft(),
                &draft
            );
        }

        let override_draft = appointment(
            AppointmentKind::Meeting,
            draft_time() + TimeDuration::hours(2),
            QuoteId::new(),
            "appointment-override",
            false,
        );
        assert!(!override_draft.requester_included());
        let stored = store
            .save_appointment_draft("phone:meeting-override", &override_draft)
            .expect("save requester override");
        assert!(!stored.draft().requester_included());
    }

    #[test]
    fn appointment_retry_is_idempotent_and_conflicts_do_not_change_rows() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let original = appointment(
            AppointmentKind::Callback,
            draft_time(),
            QuoteId::new(),
            "appointment-idempotency",
            false,
        );
        let first = store
            .save_appointment_draft("phone:original", &original)
            .expect("first save");
        let retry = store
            .save_appointment_draft("phone:original", &original)
            .expect("identical retry");
        assert_eq!(retry, first);
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("appointment row count"),
            1
        );

        let idempotency_conflict = appointment(
            AppointmentKind::Meeting,
            draft_time() + TimeDuration::hours(1),
            QuoteId::new(),
            "appointment-idempotency",
            true,
        );
        assert!(matches!(
            store.save_appointment_draft("phone:different", &idempotency_conflict),
            Err(StoreError::Conflict { .. })
        ));

        let source_conflict = appointment(
            AppointmentKind::Meeting,
            draft_time() + TimeDuration::hours(2),
            QuoteId::new(),
            "appointment-source-conflict",
            true,
        );
        assert!(matches!(
            store.save_appointment_draft("phone:original", &source_conflict),
            Err(StoreError::Conflict { .. })
        ));

        let quote_conflict = appointment(
            AppointmentKind::Meeting,
            draft_time() + TimeDuration::hours(3),
            original.quote_id(),
            "appointment-quote-conflict",
            true,
        );
        assert!(matches!(
            store.save_appointment_draft("phone:quote-conflict", &quote_conflict),
            Err(StoreError::Conflict { .. })
        ));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("appointment row count after conflicts"),
            1
        );
        assert_eq!(
            store
                .load_appointment_draft_by_id(first.id())
                .expect("original remains")
                .draft(),
            &original
        );
    }

    #[test]
    fn appointment_retry_returns_the_row_when_an_identical_write_wins_the_race() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        // This fixture injects a competing insert from another trigger. Keep
        // recursion disabled only for the fixture; production connections
        // enable it so REPLACE cannot bypass audit append-only guards.
        store
            .connection()
            .pragma_update(None, "recursive_triggers", false)
            .expect("disable recursive triggers for race fixture");
        store
            .connection()
            .execute_batch(
                r#"
                CREATE TRIGGER inject_appointment_retry_race
                BEFORE INSERT ON appointment_drafts
                BEGIN
                    INSERT OR IGNORE INTO appointment_drafts (
                        idempotency_key, source_id, quote_id, caller_name, caller_email,
                        kind, starts_at, ends_at, requester_included
                    ) VALUES (
                        NEW.idempotency_key, NEW.source_id, NEW.quote_id, NEW.caller_name, NEW.caller_email,
                        NEW.kind, NEW.starts_at, NEW.ends_at, NEW.requester_included
                    );
                END;
                "#,
            )
            .expect("install deterministic retry-race trigger");
        let draft = appointment(
            AppointmentKind::Callback,
            draft_time(),
            QuoteId::new(),
            "appointment-retry-race",
            false,
        );

        let stored = store
            .save_appointment_draft("phone:retry-race", &draft)
            .expect("an exact raced retry returns the stored row");
        store
            .connection()
            .pragma_update(None, "recursive_triggers", true)
            .expect("restore recursive triggers after race fixture");

        assert_eq!(stored.source_id(), "phone:retry-race");
        assert_eq!(stored.draft(), &draft);
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("one appointment row"),
            1
        );
    }

    #[test]
    fn appointment_repository_validates_source_and_detects_duration_corruption() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = appointment(
            AppointmentKind::Callback,
            draft_time(),
            QuoteId::new(),
            "appointment-corruption",
            false,
        );
        assert!(matches!(
            store.save_appointment_draft("  ", &draft),
            Err(StoreError::InvalidInput { .. })
        ));
        let stored = store
            .save_appointment_draft("phone:corruption", &draft)
            .expect("save draft");
        store
            .connection()
            .execute(
                "UPDATE appointment_drafts SET ends_at = '2023-11-14T23:13:20Z' WHERE id = ?1",
                [stored.id()],
            )
            .expect("corrupt end time");
        let error = store
            .load_appointment_draft_by_id(stored.id())
            .expect_err("duration corruption must fail");
        assert!(matches!(error, StoreError::StoredRecordInvalid { .. }));
        assert!(!error.to_string().contains("ada.storage@example.com"));
        assert!(!format!("{error:?}").contains("ada.storage@example.com"));
    }

    #[test]
    fn appointment_repository_rejects_noncanonical_stored_quote_uuid() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = appointment(
            AppointmentKind::Callback,
            draft_time(),
            QuoteId::new(),
            "appointment-noncanonical-quote",
            false,
        );
        let stored = store
            .save_appointment_draft("phone:noncanonical-quote", &draft)
            .expect("save draft");
        store
            .connection()
            .execute(
                "UPDATE appointment_drafts SET quote_id = upper(quote_id) WHERE id = ?1",
                [stored.id()],
            )
            .expect("rewrite quote UUID to a noncanonical representation");

        assert!(matches!(
            store.load_appointment_draft_by_id(stored.id()),
            Err(StoreError::StoredRecordInvalid {
                resource: "appointment draft"
            })
        ));
    }

    #[test]
    fn appointment_repository_reports_missing_and_deleted_rows_and_redacts_debug() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        assert!(matches!(
            store.load_appointment_draft_by_id(999),
            Err(StoreError::NotFound { .. })
        ));
        let draft = appointment(
            AppointmentKind::Callback,
            draft_time(),
            QuoteId::new(),
            "appointment-delete",
            false,
        );
        let stored = store
            .save_appointment_draft("phone:delete", &draft)
            .expect("save draft");
        let debug = format!("{stored:?}");
        assert!(!debug.contains("ada.storage@example.com"));
        store
            .delete_appointment_draft_by_id(stored.id())
            .expect("delete draft");
        assert!(matches!(
            store.load_appointment_draft_by_id(stored.id()),
            Err(StoreError::NotFound { .. })
        ));
        assert!(matches!(
            store.delete_appointment_draft_by_id(stored.id()),
            Err(StoreError::NotFound { .. })
        ));
    }

    #[test]
    fn owner_task_repository_round_trips_each_kind_custom_duration_and_due_date() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let due_at = draft_time() + TimeDuration::days(2);
        for (index, kind) in [
            TaskKind::Bill,
            TaskKind::Callback,
            TaskKind::Reading,
            TaskKind::EmailReply,
            TaskKind::Preparation,
        ]
        .into_iter()
        .enumerate()
        {
            let draft = owner_task(
                kind,
                "Prepare a concise agenda",
                kind.duration_minutes() as u32 + 5,
                (index == 4).then_some(due_at),
                &format!("owner-task-{index}"),
            );
            let source_id = format!("owner:task-{index}");
            let stored = store
                .save_owner_task_draft(Some(source_id.as_str()), &draft)
                .expect("save owner task");
            assert_eq!(stored.source_id(), Some(source_id.as_str()));
            assert_eq!(stored.draft(), &draft);
            assert_eq!(
                store
                    .load_owner_task_draft_by_id(stored.id())
                    .expect("load owner task by id")
                    .draft(),
                &draft
            );
            assert_eq!(
                store
                    .load_owner_task_draft_by_idempotency_key(draft.idempotency_key().as_str())
                    .expect("load owner task by key")
                    .draft(),
                &draft
            );
        }
    }

    #[test]
    fn owner_task_retry_conflicts_and_optional_source_are_immutable() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let original = owner_task(
            TaskKind::Callback,
            "Call the supplier",
            25,
            None,
            "owner-retry",
        );
        let first = store
            .save_owner_task_draft(None, &original)
            .expect("owner save");
        let retry = store
            .save_owner_task_draft(None, &original)
            .expect("owner retry");
        assert_eq!(retry, first);

        let changed = owner_task(
            TaskKind::Preparation,
            "Prepare another task",
            60,
            None,
            "owner-retry",
        );
        assert!(matches!(
            store.save_owner_task_draft(Some("owner:different"), &changed),
            Err(StoreError::Conflict { .. })
        ));
        let source_conflict = owner_task(
            TaskKind::Preparation,
            "Prepare another task",
            60,
            None,
            "owner-source-conflict",
        );
        assert!(
            store
                .save_owner_task_draft(Some("owner:source"), &source_conflict)
                .is_ok()
        );
        let source_reuse = owner_task(TaskKind::Bill, "Pay a bill", 15, None, "owner-source-reuse");
        assert!(matches!(
            store.save_owner_task_draft(Some("owner:source"), &source_reuse),
            Err(StoreError::Conflict { .. })
        ));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM owner_task_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("owner task row count"),
            2
        );
        let debug = format!("{first:?}");
        assert!(!debug.contains("Call the supplier"));

        store
            .delete_owner_task_draft_by_id(first.id())
            .expect("delete owner task");
        assert!(matches!(
            store.load_owner_task_draft_by_id(first.id()),
            Err(StoreError::NotFound { .. })
        ));
    }

    #[test]
    fn owner_task_retry_returns_the_row_when_an_identical_write_wins_the_race() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        // This fixture injects a competing insert from another trigger. Keep
        // recursion disabled only for the fixture; production connections
        // enable it so REPLACE cannot bypass audit append-only guards.
        store
            .connection()
            .pragma_update(None, "recursive_triggers", false)
            .expect("disable recursive triggers for race fixture");
        store
            .connection()
            .execute_batch(
                r#"
                CREATE TRIGGER inject_owner_task_retry_race
                BEFORE INSERT ON owner_task_drafts
                BEGIN
                    INSERT OR IGNORE INTO owner_task_drafts (
                        idempotency_key, source_id, title, kind, duration_minutes, due_at
                    ) VALUES (
                        NEW.idempotency_key, NEW.source_id, NEW.title, NEW.kind, NEW.duration_minutes, NEW.due_at
                    );
                END;
                "#,
            )
            .expect("install deterministic retry-race trigger");
        let draft = owner_task(
            TaskKind::Callback,
            "Call the supplier",
            25,
            None,
            "owner-retry-race",
        );

        let stored = store
            .save_owner_task_draft(None, &draft)
            .expect("an exact raced retry returns the stored row");
        store
            .connection()
            .pragma_update(None, "recursive_triggers", true)
            .expect("restore recursive triggers after race fixture");

        assert_eq!(stored.source_id(), None);
        assert_eq!(stored.draft(), &draft);
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM owner_task_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("one owner task row"),
            1
        );
    }

    #[test]
    fn owner_task_repository_rejects_blank_source_and_redacts_corruption_errors() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = owner_task(
            TaskKind::Reading,
            "Read the report",
            30,
            None,
            "owner-corruption",
        );
        assert!(matches!(
            store.save_owner_task_draft(Some("  "), &draft),
            Err(StoreError::InvalidInput { .. })
        ));
        let stored = store
            .save_owner_task_draft(None, &draft)
            .expect("save owner task");
        store
            .connection()
            .execute(
                "UPDATE owner_task_drafts SET kind = 'unknown' WHERE id = ?1",
                [stored.id()],
            )
            .expect("corrupt task kind");
        let error = store
            .load_owner_task_draft_by_id(stored.id())
            .expect_err("corrupt task kind must fail");
        assert!(matches!(error, StoreError::StoredRecordInvalid { .. }));
        assert!(!error.to_string().contains("Read the report"));
    }

    #[test]
    fn owner_task_placement_v10_upgrade_backfills_legacy_fingerprint_safely() {
        let database = TempDatabase::new();
        {
            let mut connection = Connection::open(&database.path).expect("open v10 fixture");
            apply_sqlcipher_key(&connection, DATABASE_KEY).expect("apply key");
            verify_sqlcipher(&connection).expect("verify cipher");
            connection
                .pragma_update(None, "foreign_keys", true)
                .expect("foreign keys");
            run_migrations_with(&mut connection, &MIGRATIONS[..10]).expect("apply v10");
            connection
                .execute(
                    "INSERT INTO owner_task_drafts(idempotency_key, source_id, title, kind, duration_minutes) VALUES (?1, ?2, ?3, 'callback', 15)",
                    rusqlite::params!["owner-v10-key", "owner:v10", "Call supplier"],
                )
                .expect("seed owner draft");
            connection
                .execute(
                    "INSERT INTO owner_task_placements(owner_task_draft_id, starts_at, ends_at, timezone, operation_key, state) VALUES (1, '2025-01-02T03:04:05Z', '2025-01-02T03:19:05Z', 'Australia/Sydney', 'owner-v10-operation', 'prepared')",
                    [],
                )
                .expect("seed v10 placement");
            run_migrations_with(&mut connection, MIGRATIONS).expect("upgrade to v11");
            assert_eq!(
                connection
                    .query_row(
                        "SELECT owner_fingerprint FROM owner_task_placements WHERE owner_task_draft_id = 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .expect("legacy fingerprint"),
                "legacy"
            );
            let store = PaStore { connection };
            let placement = store
                .load_owner_task_placement(1)
                .expect("migrated placement remains loadable");
            assert_eq!(placement.owner_fingerprint(), "legacy");
            assert_eq!(placement.operation_key(), "owner-v10-operation");
        }
        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen v11 store");
        assert_eq!(
            reopened
                .load_owner_task_placement(1)
                .expect("reopened placement")
                .owner_fingerprint(),
            "legacy"
        );
    }

    #[test]
    fn owner_task_placement_reopen_retry_and_strict_corruption_fail_closed() {
        let database = TempDatabase::new();
        let draft = owner_task(
            TaskKind::Callback,
            "Call the supplier",
            25,
            None,
            "owner-placement-adversarial",
        );
        let starts_at = OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start");
        let ends_at = starts_at + TimeDuration::minutes(25);
        let (stored_draft, first) = {
            let store = PaStore::open(&database.path, DATABASE_KEY).expect("open");
            store
                .save_prepared_owner_task(
                    Some("owner:placement-adversarial"),
                    &draft,
                    starts_at,
                    ends_at,
                    "Australia/Sydney",
                    "owner-placement-operation",
                    "owner-fingerprint-a",
                )
                .expect("atomic prepare")
        };
        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen");
        let (retry_draft, retry) = reopened
            .save_prepared_owner_task(
                Some("owner:placement-adversarial"),
                &draft,
                starts_at,
                ends_at,
                "Australia/Sydney",
                "owner-placement-operation",
                "owner-fingerprint-a",
            )
            .expect("exact retry after reopen");
        assert_eq!(retry_draft, stored_draft);
        assert_eq!(retry, first);

        for (starts_at, ends_at, timezone, operation_key, fingerprint) in [
            (
                starts_at + TimeDuration::minutes(1),
                ends_at + TimeDuration::minutes(1),
                "Australia/Sydney",
                "owner-placement-operation",
                "owner-fingerprint-a",
            ),
            (
                starts_at,
                ends_at,
                "UTC",
                "owner-placement-operation",
                "owner-fingerprint-a",
            ),
            (
                starts_at,
                ends_at,
                "Australia/Sydney",
                "owner-placement-operation-changed",
                "owner-fingerprint-a",
            ),
            (
                starts_at,
                ends_at,
                "Australia/Sydney",
                "owner-placement-operation",
                "owner-fingerprint-b",
            ),
        ] {
            assert!(matches!(
                reopened.save_prepared_owner_task(
                    Some("owner:placement-adversarial"),
                    &draft,
                    starts_at,
                    ends_at,
                    timezone,
                    operation_key,
                    fingerprint,
                ),
                Err(StoreError::Conflict {
                    resource: "owner task placement"
                })
            ));
        }
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM owner_task_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("draft count"),
            1
        );
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM owner_task_placements", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("placement count"),
            1
        );

        for (column, value) in [
            ("starts_at", "2025-01-02T03:04:05+00:00"),
            ("ends_at", "2025-01-02T03:29:05.000Z"),
            ("timezone", "secret/timezone"),
            ("operation_key", "bad operation key"),
            ("owner_fingerprint", "bad fingerprint"),
        ] {
            reopened
                .connection()
                .execute_batch("PRAGMA ignore_check_constraints = ON")
                .expect("allow corruption fixture");
            reopened
                .connection()
                .execute(
                    &format!("UPDATE owner_task_placements SET {column} = ?1"),
                    [value],
                )
                .expect("corrupt placement");
            let error = reopened
                .load_owner_task_placement(stored_draft.id())
                .expect_err("corrupt placement must fail closed");
            assert!(matches!(error, StoreError::StoredRecordInvalid { .. }));
            assert!(!error.to_string().contains(value));
            assert!(!format!("{error:?}").contains(value));
            reopened
                .connection()
                .execute(
                    &format!("UPDATE owner_task_placements SET {column} = ?1"),
                    [match column {
                        "starts_at" => "2025-01-02T03:04:05Z",
                        "ends_at" => "2025-01-02T03:29:05Z",
                        "timezone" => "Australia/Sydney",
                        "operation_key" => "owner-placement-operation",
                        "owner_fingerprint" => "owner-fingerprint-a",
                        _ => unreachable!(),
                    }],
                )
                .expect("restore fixture");
            reopened
                .connection()
                .execute_batch("PRAGMA ignore_check_constraints = OFF")
                .expect("restore checks");
        }

        reopened
            .connection()
            .execute_batch("PRAGMA foreign_keys = OFF; DELETE FROM owner_task_drafts;")
            .expect("remove draft for FK corruption fixture");
        let error = reopened
            .load_owner_task_placement(stored_draft.id())
            .expect_err("orphan placement must fail closed");
        assert!(matches!(error, StoreError::StoredRecordInvalid { .. }));
    }

    #[test]
    fn owner_task_placement_submission_is_immutable_and_provider_ids_are_unique() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open");
        let first_draft = owner_task(
            TaskKind::Callback,
            "First task",
            15,
            None,
            "owner-submit-first",
        );
        let second_draft = owner_task(
            TaskKind::Callback,
            "Second task",
            15,
            None,
            "owner-submit-second",
        );
        let starts_at = OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start");
        let first = store
            .save_prepared_owner_task(
                Some("owner:submit:first"),
                &first_draft,
                starts_at,
                starts_at + TimeDuration::minutes(15),
                "Australia/Sydney",
                "owner-submit-op-first",
                "owner-submit-fingerprint",
            )
            .expect("first prepare");
        let second = store
            .save_prepared_owner_task(
                Some("owner:submit:second"),
                &second_draft,
                starts_at + TimeDuration::minutes(30),
                starts_at + TimeDuration::minutes(45),
                "Australia/Sydney",
                "owner-submit-op-second",
                "owner-submit-fingerprint",
            )
            .expect("second prepare");

        let submitted = store
            .submit_owner_task_placement(first.0.id(), "outlook/event+opaque==")
            .expect("submit");
        assert_eq!(
            submitted.provider_event_id(),
            Some("outlook/event+opaque==")
        );
        assert_eq!(
            store
                .submit_owner_task_placement(first.0.id(), "outlook/event+opaque==")
                .expect("exact submit retry"),
            submitted
        );
        assert!(matches!(
            store.submit_owner_task_placement(first.0.id(), "outlook-event-2"),
            Err(StoreError::Conflict {
                resource: "owner task placement"
            })
        ));
        assert!(matches!(
            store.submit_owner_task_placement(second.0.id(), "outlook/event+opaque=="),
            Err(StoreError::Conflict {
                resource: "owner task placement"
            })
        ));
        assert_eq!(
            store
                .load_owner_task_placement(second.0.id())
                .expect("second remains prepared")
                .provider_event_id(),
            None
        );
    }

    #[test]
    fn owner_task_provider_event_id_submission_rejects_invalid_inputs_without_mutation() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open");
        let draft = owner_task(
            TaskKind::Callback,
            "Reject invalid event IDs",
            15,
            None,
            "owner-submit-invalid-event-id",
        );
        let starts_at = OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start");
        let stored = store
            .save_prepared_owner_task(
                Some("owner:submit:invalid-event-id"),
                &draft,
                starts_at,
                starts_at + TimeDuration::minutes(15),
                "Australia/Sydney",
                "owner-submit-invalid-operation",
                "owner-submit-invalid-fingerprint",
            )
            .expect("prepare");

        for value in ["", "   ", "A\nB", "A\0B", "é"] {
            assert!(matches!(
                store.submit_owner_task_placement(stored.0.id(), value),
                Err(StoreError::InvalidInput {
                    field: "provider_event_id"
                })
            ));
        }
        let oversized = "a".repeat(MAX_TASK_ID_LENGTH + 1);
        assert!(matches!(
            store.submit_owner_task_placement(stored.0.id(), oversized),
            Err(StoreError::InvalidInput {
                field: "provider_event_id"
            })
        ));
        assert_eq!(
            store
                .load_owner_task_placement(stored.0.id())
                .expect("placement remains prepared")
                .provider_event_id(),
            None
        );
    }

    #[test]
    fn owner_task_provider_event_id_corruption_fails_closed_without_disclosure() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open");
        let draft = owner_task(
            TaskKind::Callback,
            "Reject corrupt event IDs",
            15,
            None,
            "owner-corrupt-event-id",
        );
        let starts_at = OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start");
        let stored = store
            .save_prepared_owner_task(
                Some("owner:corrupt:event-id"),
                &draft,
                starts_at,
                starts_at + TimeDuration::minutes(15),
                "Australia/Sydney",
                "owner-corrupt-operation",
                "owner-corrupt-fingerprint",
            )
            .expect("prepare");
        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints = ON")
            .expect("allow corruption fixture");

        for value in ["", "   ", "A\nB", "A\0B", "é"] {
            store
                .connection()
                .execute(
                    "UPDATE owner_task_placements SET state = 'submitted', provider_event_id = ?1",
                    [value],
                )
                .expect("corrupt provider event ID");
            let error = store
                .load_owner_task_placement(stored.0.id())
                .expect_err("corrupt provider event ID must fail closed");
            assert!(matches!(error, StoreError::StoredRecordInvalid { .. }));
            if !value.is_empty() {
                assert!(!error.to_string().contains(value));
                assert!(!format!("{error:?}").contains(value));
            }
        }
        let oversized = "a".repeat(MAX_TASK_ID_LENGTH + 1);
        store
            .connection()
            .execute(
                "UPDATE owner_task_placements SET state = 'submitted', provider_event_id = ?1",
                [&oversized],
            )
            .expect("corrupt oversized provider event ID");
        let error = store
            .load_owner_task_placement(stored.0.id())
            .expect_err("oversized provider event ID must fail closed");
        assert!(matches!(error, StoreError::StoredRecordInvalid { .. }));
        assert!(!error.to_string().contains(&oversized));
        assert!(!format!("{error:?}").contains(&oversized));
    }

    #[test]
    fn owner_task_prepared_file_store_races_keep_one_stable_aggregate() {
        use std::sync::{Arc, Barrier};

        let database = TempDatabase::new();
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("second store");
        let barrier = Arc::new(Barrier::new(2));
        let draft = owner_task(
            TaskKind::Callback,
            "Call supplier",
            15,
            None,
            "owner-race-key",
        );
        let run = |store: PaStore, barrier: Arc<Barrier>| {
            let draft = draft.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.save_prepared_owner_task(
                    Some("owner:race"),
                    &draft,
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start"),
                    OffsetDateTime::parse("2025-01-02T03:19:05Z", &Rfc3339).expect("end"),
                    "Australia/Sydney",
                    "owner-race-operation",
                    "owner-race-fingerprint",
                )
            })
        };
        let first_handle = run(first, Arc::clone(&barrier));
        let second_handle = run(second, barrier);
        let results = [
            first_handle.join().expect("first race thread"),
            second_handle.join().expect("second race thread"),
        ];
        assert!(
            results.iter().all(Result::is_ok),
            "identical retries succeed"
        );
        assert_eq!(results[0].as_ref().ok(), results[1].as_ref().ok());
        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen race store");
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM owner_task_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("draft count"),
            1
        );
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM owner_task_placements", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("placement count"),
            1
        );

        let database = TempDatabase::new();
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("first conflict store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("second conflict store");
        let barrier = Arc::new(Barrier::new(2));
        let first_draft = owner_task(
            TaskKind::Callback,
            "First title",
            15,
            None,
            "owner-conflict-race",
        );
        let second_draft = owner_task(
            TaskKind::Callback,
            "Second title",
            15,
            None,
            "owner-conflict-race",
        );
        let first_handle = std::thread::spawn({
            let barrier = Arc::clone(&barrier);
            move || {
                barrier.wait();
                first.save_prepared_owner_task(
                    Some("owner:conflict-race"),
                    &first_draft,
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start"),
                    OffsetDateTime::parse("2025-01-02T03:19:05Z", &Rfc3339).expect("end"),
                    "Australia/Sydney",
                    "owner-conflict-operation-a",
                    "owner-conflict-fingerprint",
                )
            }
        });
        let second_handle = std::thread::spawn({
            let barrier = Arc::clone(&barrier);
            move || {
                barrier.wait();
                second.save_prepared_owner_task(
                    Some("owner:conflict-race"),
                    &second_draft,
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start"),
                    OffsetDateTime::parse("2025-01-02T03:19:05Z", &Rfc3339).expect("end"),
                    "Australia/Sydney",
                    "owner-conflict-operation-b",
                    "owner-conflict-fingerprint",
                )
            }
        });
        let results = [
            first_handle.join().expect("first conflict thread"),
            second_handle.join().expect("second conflict thread"),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::Conflict {
                        resource: "owner task draft"
                    })
                ))
                .count(),
            1
        );
        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen conflict store");
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM owner_task_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("conflict draft count"),
            1
        );
        assert_eq!(
            reopened
                .connection()
                .query_row("SELECT count(*) FROM owner_task_placements", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("conflict placement count"),
            1
        );
    }

    #[test]
    fn owner_task_placement_file_store_races_converge_or_conflict_without_busy_errors() {
        use std::sync::{Arc, Barrier};

        let database = TempDatabase::new();
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("seed store");
        let stored = seed
            .save_owner_task_draft(
                Some("owner:placement-race"),
                &owner_task(
                    TaskKind::Callback,
                    "Call supplier",
                    15,
                    None,
                    "owner-placement-race-key",
                ),
            )
            .expect("seed draft");
        let draft_id = stored.id();
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("second store");
        let barrier = Arc::new(Barrier::new(2));
        let run = |store: PaStore, barrier: Arc<Barrier>| {
            std::thread::spawn(move || {
                barrier.wait();
                store.save_owner_task_placement(
                    draft_id,
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start"),
                    OffsetDateTime::parse("2025-01-02T03:19:05Z", &Rfc3339).expect("end"),
                    "Australia/Sydney",
                    "owner-placement-race-operation",
                    "owner-placement-race-fingerprint",
                )
            })
        };
        let first_handle = run(first, Arc::clone(&barrier));
        let second_handle = run(second, barrier);
        let results = [
            first_handle.join().expect("first race thread"),
            second_handle.join().expect("second race thread"),
        ];
        assert!(results.iter().all(Result::is_ok), "exact calls converge");
        assert_eq!(results[0].as_ref().ok(), results[1].as_ref().ok());

        let conflict_database = TempDatabase::new();
        let conflict_seed =
            PaStore::open(&conflict_database.path, DATABASE_KEY).expect("conflict seed store");
        let conflict_draft_id = conflict_seed
            .save_owner_task_draft(
                Some("owner:placement-conflict-race"),
                &owner_task(
                    TaskKind::Callback,
                    "Call supplier",
                    15,
                    None,
                    "owner-placement-conflict-race-key",
                ),
            )
            .expect("conflict seed draft")
            .id();
        let first =
            PaStore::open(&conflict_database.path, DATABASE_KEY).expect("first conflict store");
        let second =
            PaStore::open(&conflict_database.path, DATABASE_KEY).expect("second conflict store");
        let barrier = Arc::new(Barrier::new(2));
        let run = |store: PaStore,
                   barrier: Arc<Barrier>,
                   operation_key: &'static str,
                   fingerprint: &'static str| {
            std::thread::spawn(move || {
                barrier.wait();
                store.save_owner_task_placement(
                    conflict_draft_id,
                    OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("start"),
                    OffsetDateTime::parse("2025-01-02T03:19:05Z", &Rfc3339).expect("end"),
                    "Australia/Sydney",
                    operation_key,
                    fingerprint,
                )
            })
        };
        let first_handle = run(
            first,
            Arc::clone(&barrier),
            "different-placement-operation-a",
            "different-placement-fingerprint-a",
        );
        let second_handle = run(
            second,
            barrier,
            "different-placement-operation-b",
            "different-placement-fingerprint-b",
        );
        let results = [
            first_handle.join().expect("first conflict thread"),
            second_handle.join().expect("second conflict thread"),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::Conflict {
                        resource: "owner task placement"
                    })
                ))
                .count(),
            1
        );
    }

    #[test]
    fn proposal_repository_creates_from_an_existing_appointment_draft() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = appointment(
            AppointmentKind::Meeting,
            draft_time(),
            QuoteId::new(),
            "proposal-appointment-draft",
            true,
        );
        let stored_draft = store
            .save_appointment_draft("appointment:proposal", &draft)
            .expect("save appointment draft");

        let proposal = store
            .create_proposal(
                "proposal-key",
                "proposal:appointment",
                ProposalSource::AppointmentDraft {
                    id: stored_draft.id(),
                },
            )
            .expect("create proposal");

        assert_eq!(proposal.state(), ProposalState::Pending);
        assert_eq!(proposal.source_id(), "proposal:appointment");
        assert_eq!(
            proposal.source(),
            ProposalSource::AppointmentDraft {
                id: stored_draft.id(),
            }
        );
    }

    #[test]
    fn proposal_source_requires_exactly_one_positive_draft_id() {
        for (appointment_id, owner_task_id) in [
            (None, None),
            (Some(1), Some(2)),
            (Some(0), None),
            (None, Some(0)),
            (Some(-1), None),
            (None, Some(-1)),
        ] {
            assert!(matches!(
                ProposalSource::from_ids(appointment_id, owner_task_id),
                Err(StoreError::InvalidInput { field: "source" })
            ));
        }
    }

    #[test]
    fn proposal_creation_rejects_missing_source_before_insertion() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let error = store
            .create_proposal(
                "missing-proposal",
                "proposal:missing",
                ProposalSource::AppointmentDraft { id: 999 },
            )
            .expect_err("missing source must fail");
        assert!(matches!(
            error,
            StoreError::NotFound {
                resource: "appointment draft"
            }
        ));
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM proposals", [], |row| row.get(0))
            .expect("proposal count");
        assert_eq!(count, 0);
    }

    #[test]
    fn proposal_schema_rejects_neither_or_both_draft_sources_even_for_direct_sql() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        assert!(
            store
                .connection()
                .execute(
                    "INSERT INTO proposals(idempotency_key, source_id) \
                 VALUES ('proposal-neither-key', 'proposal:neither')",
                    [],
                )
                .is_err()
        );

        let appointment = store
            .save_appointment_draft(
                "appointment:proposal-xor",
                &appointment(
                    AppointmentKind::Callback,
                    draft_time(),
                    QuoteId::new(),
                    "proposal-xor-appointment",
                    false,
                ),
            )
            .expect("appointment draft");
        let owner_task = store
            .save_owner_task_draft(
                Some("owner:proposal-xor"),
                &owner_task(
                    TaskKind::Callback,
                    "Call the supplier",
                    15,
                    None,
                    "proposal-xor-owner",
                ),
            )
            .expect("owner task draft");
        assert!(
            store
                .connection()
                .execute(
                    "INSERT INTO proposals(
                     idempotency_key, source_id, appointment_draft_id, owner_task_draft_id
                 ) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        "proposal-both-key",
                        "proposal:both",
                        appointment.id(),
                        owner_task.id(),
                    ],
                )
                .is_err()
        );
    }

    #[test]
    fn deleting_appointment_proposal_source_returns_conflict_and_preserves_retry() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = appointment(
            AppointmentKind::Callback,
            draft_time(),
            QuoteId::new(),
            "proposal-source-delete-appointment-draft",
            false,
        );
        let stored_draft = store
            .save_appointment_draft("appointment:proposal-source-delete", &draft)
            .expect("appointment draft");
        let source = ProposalSource::AppointmentDraft {
            id: stored_draft.id(),
        };
        let proposal = store
            .create_proposal(
                "proposal-source-delete-appointment-key",
                "proposal:source-delete:appointment",
                source,
            )
            .expect("proposal");

        let error = store
            .delete_appointment_draft_by_id(stored_draft.id())
            .expect_err("referenced appointment draft deletion must fail");
        assert!(matches!(
            error,
            StoreError::Conflict {
                resource: "appointment draft"
            }
        ));
        assert!(
            !error
                .to_string()
                .contains("proposal:source-delete:appointment")
        );
        assert_eq!(
            store
                .load_proposal_by_id(proposal.id())
                .expect("proposal remains loadable"),
            proposal
        );
        assert_eq!(
            store
                .create_proposal(
                    "proposal-source-delete-appointment-key",
                    "proposal:source-delete:appointment",
                    source,
                )
                .expect("exact retry remains valid"),
            proposal
        );
    }

    #[test]
    fn deleting_owner_task_proposal_source_returns_conflict_and_preserves_retry() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = owner_task(
            TaskKind::Callback,
            "Call the supplier",
            15,
            None,
            "proposal-source-delete-owner-draft",
        );
        let stored_draft = store
            .save_owner_task_draft(Some("owner:proposal-source-delete"), &draft)
            .expect("owner task draft");
        let source = ProposalSource::OwnerTaskDraft {
            id: stored_draft.id(),
        };
        let proposal = store
            .create_proposal(
                "proposal-source-delete-owner-key",
                "proposal:source-delete:owner",
                source,
            )
            .expect("proposal");

        let error = store
            .delete_owner_task_draft_by_id(stored_draft.id())
            .expect_err("referenced owner task draft deletion must fail");
        assert!(matches!(
            error,
            StoreError::Conflict {
                resource: "owner task draft"
            }
        ));
        assert!(!error.to_string().contains("proposal:source-delete:owner"));
        assert_eq!(
            store
                .load_proposal_by_id(proposal.id())
                .expect("proposal remains loadable"),
            proposal
        );
        assert_eq!(
            store
                .create_proposal(
                    "proposal-source-delete-owner-key",
                    "proposal:source-delete:owner",
                    source,
                )
                .expect("exact retry remains valid"),
            proposal
        );
    }

    #[test]
    fn proposal_retry_conflicts_and_lookups_are_immutable() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let appointment = store
            .save_appointment_draft(
                "appointment:proposal-retry",
                &appointment(
                    AppointmentKind::Meeting,
                    draft_time(),
                    QuoteId::new(),
                    "proposal-retry-draft",
                    true,
                ),
            )
            .expect("appointment draft");
        let owner_task = store
            .save_owner_task_draft(
                Some("owner:proposal-retry"),
                &owner_task(
                    TaskKind::Callback,
                    "Call the supplier",
                    15,
                    None,
                    "proposal-retry-task",
                ),
            )
            .expect("owner task draft");
        let source = ProposalSource::AppointmentDraft {
            id: appointment.id(),
        };
        let first = store
            .create_proposal("proposal-retry-key", "proposal:retry", source)
            .expect("proposal");
        assert_eq!(
            store
                .create_proposal("proposal-retry-key", "proposal:retry", source)
                .expect("exact retry"),
            first
        );

        assert!(matches!(
            store.create_proposal(
                "proposal-retry-key",
                "proposal:changed-key-content",
                ProposalSource::OwnerTaskDraft {
                    id: owner_task.id()
                },
            ),
            Err(StoreError::Conflict {
                resource: "proposal"
            })
        ));
        assert!(matches!(
            store.create_proposal(
                "proposal-other-key",
                "proposal:retry",
                ProposalSource::OwnerTaskDraft {
                    id: owner_task.id()
                },
            ),
            Err(StoreError::Conflict {
                resource: "proposal"
            })
        ));
        assert!(matches!(
            store.create_proposal("  ", "proposal:blank-key", source),
            Err(StoreError::InvalidInput {
                field: "idempotency_key"
            })
        ));

        assert_eq!(
            store.load_proposal_by_id(first.id()).expect("load by id"),
            first
        );
        assert_eq!(
            store
                .load_proposal_by_idempotency_key("proposal-retry-key")
                .expect("load by key"),
            first
        );
        assert_eq!(
            store
                .load_proposal_by_source_id("proposal:retry")
                .expect("load by source"),
            first
        );
        assert!(matches!(
            store.load_proposal_by_id(999),
            Err(StoreError::NotFound {
                resource: "proposal"
            })
        ));
        store
            .delete_proposal_by_id(first.id())
            .expect("delete proposal");
        assert!(matches!(
            store.load_proposal_by_id(first.id()),
            Err(StoreError::NotFound {
                resource: "proposal"
            })
        ));
    }

    #[test]
    fn proposal_transitions_compare_and_set_and_preserve_terminal_retry_timestamp() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = store
            .save_appointment_draft(
                "appointment:proposal-transition",
                &appointment(
                    AppointmentKind::Callback,
                    draft_time(),
                    QuoteId::new(),
                    "proposal-transition-draft",
                    false,
                ),
            )
            .expect("appointment draft");
        let pending = store
            .create_proposal(
                "proposal-transition-key",
                "proposal:transition",
                ProposalSource::AppointmentDraft { id: draft.id() },
            )
            .expect("proposal");
        assert!(matches!(
            store.transition_proposal(pending.id(), ProposalState::Pending),
            Err(StoreError::Conflict {
                resource: "proposal transition"
            })
        ));

        store
            .connection()
            .execute(
                "UPDATE proposals SET updated_at = '2000-01-01 00:00:00' WHERE id = ?1",
                [pending.id()],
            )
            .expect("set deterministic timestamp");
        let accepted = store
            .transition_proposal(pending.id(), ProposalState::Accepted)
            .expect("accept proposal");
        assert_eq!(accepted.state(), ProposalState::Accepted);
        let timestamp = accepted.updated_at().to_owned();
        let retry = store
            .transition_proposal(pending.id(), ProposalState::Accepted)
            .expect("terminal retry");
        assert_eq!(retry.updated_at(), timestamp);
        for state in [
            ProposalState::Pending,
            ProposalState::Declined,
            ProposalState::Expired,
        ] {
            assert!(matches!(
                store.transition_proposal(pending.id(), state),
                Err(StoreError::Conflict {
                    resource: "proposal transition"
                })
            ));
        }

        for (index, terminal) in [ProposalState::Declined, ProposalState::Expired]
            .into_iter()
            .enumerate()
        {
            let draft = store
                .save_owner_task_draft(
                    Some(&format!("owner:proposal-terminal-{index}")),
                    &owner_task(
                        TaskKind::Callback,
                        "Terminal transition",
                        15,
                        None,
                        &format!("proposal-terminal-{index}"),
                    ),
                )
                .expect("owner task");
            let proposal = store
                .create_proposal(
                    format!("proposal-terminal-key-{index}"),
                    format!("proposal:terminal-{index}"),
                    ProposalSource::OwnerTaskDraft { id: draft.id() },
                )
                .expect("proposal");
            store
                .connection()
                .execute(
                    "UPDATE proposals SET updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![format!("2001-01-01 00:00:0{index}"), proposal.id()],
                )
                .expect("set deterministic terminal timestamp");
            let transitioned = store
                .transition_proposal(proposal.id(), terminal)
                .expect("terminal transition");
            assert_eq!(transitioned.state(), terminal);
            let timestamp = transitioned.updated_at().to_owned();
            let retry = store
                .transition_proposal(proposal.id(), terminal)
                .expect("identical terminal retry");
            assert_eq!(retry.updated_at(), timestamp, "{terminal:?}");
        }
    }

    #[test]
    fn proposal_corruption_is_strict_and_redacted() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = store
            .save_appointment_draft(
                "appointment:proposal-corrupt",
                &appointment(
                    AppointmentKind::Callback,
                    draft_time(),
                    QuoteId::new(),
                    "proposal-corrupt-draft",
                    false,
                ),
            )
            .expect("appointment draft");
        let proposal = store
            .create_proposal(
                "proposal-corrupt-key",
                "proposal:corrupt-sensitive",
                ProposalSource::AppointmentDraft { id: draft.id() },
            )
            .expect("proposal");
        store
            .connection()
            .execute("PRAGMA ignore_check_constraints = ON", [])
            .expect("ignore checks");
        store
            .connection()
            .execute(
                "UPDATE proposals SET state = 'broken', appointment_draft_id = NULL, owner_task_draft_id = NULL WHERE id = ?1",
                [proposal.id()],
            )
            .expect("corrupt proposal");
        let error = store
            .load_proposal_by_id(proposal.id())
            .expect_err("corrupt proposal must fail");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "proposal"
            }
        ));
        assert!(!error.to_string().contains("proposal:corrupt-sensitive"));
        assert!(!format!("{error:?}").contains("proposal:corrupt-sensitive"));
    }

    #[test]
    fn proposal_rejects_malformed_storage_timestamp_without_leaking_source() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = store
            .save_appointment_draft(
                "appointment:proposal-timestamp-corrupt",
                &appointment(
                    AppointmentKind::Callback,
                    draft_time(),
                    QuoteId::new(),
                    "proposal-timestamp-corrupt-draft",
                    false,
                ),
            )
            .expect("appointment draft");
        let proposal = store
            .create_proposal(
                "proposal-timestamp-corrupt-key",
                "proposal:timestamp-sensitive",
                ProposalSource::AppointmentDraft { id: draft.id() },
            )
            .expect("proposal");

        store
            .connection()
            .execute(
                "UPDATE proposals SET updated_at = 'not-a-timestamp' WHERE id = ?1",
                [proposal.id()],
            )
            .expect("corrupt proposal timestamp");
        let error = store
            .load_proposal_by_id(proposal.id())
            .expect_err("malformed proposal timestamp must fail");

        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "proposal"
            }
        ));
        assert!(!error.to_string().contains("proposal:timestamp-sensitive"));
        assert!(!format!("{error:?}").contains("proposal:timestamp-sensitive"));
    }

    #[test]
    fn event_mapping_validates_retries_uniqueness_lookups_and_cascade_delete() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = store
            .save_appointment_draft(
                "appointment:event-mapping",
                &appointment(
                    AppointmentKind::Meeting,
                    draft_time(),
                    QuoteId::new(),
                    "event-mapping-draft",
                    true,
                ),
            )
            .expect("appointment draft");
        let proposal = store
            .create_proposal(
                "event-proposal-key",
                "proposal:event",
                ProposalSource::AppointmentDraft { id: draft.id() },
            )
            .expect("proposal");
        let start = draft_time() + TimeDuration::hours(1);
        let end = start + TimeDuration::minutes(30);
        for args in [
            (Some(start), None),
            (None, Some(end)),
            (Some(end), Some(start)),
        ] {
            assert!(matches!(
                store.attach_event_mapping(
                    proposal.id(),
                    "google",
                    "event-invalid",
                    "event:invalid",
                    args.0,
                    args.1,
                ),
                Err(StoreError::InvalidInput { .. })
            ));
        }
        for (provider, provider_event_id, source_id) in [
            ("", "event-empty-provider", "event:empty-provider"),
            ("google", "", "event:empty-event"),
            ("google", "event-empty-source", " "),
        ] {
            assert!(matches!(
                store.attach_event_mapping(
                    proposal.id(),
                    provider,
                    provider_event_id,
                    source_id,
                    None,
                    None,
                ),
                Err(StoreError::InvalidInput { .. })
            ));
        }

        let mapping = store
            .attach_event_mapping(
                proposal.id(),
                "google",
                "event-1",
                "event:source-1",
                Some(start),
                Some(end),
            )
            .expect("event mapping");
        assert_eq!(
            store
                .attach_event_mapping(
                    proposal.id(),
                    "google",
                    "event-1",
                    "event:source-1",
                    Some(start),
                    Some(end),
                )
                .expect("exact retry"),
            mapping
        );
        assert!(matches!(
            store.attach_event_mapping(
                proposal.id(),
                "google",
                "event-1",
                "event:source-changed",
                Some(start),
                Some(end),
            ),
            Err(StoreError::Conflict {
                resource: "event mapping"
            })
        ));
        assert!(matches!(
            store.attach_event_mapping(
                proposal.id(),
                "google",
                "event-2",
                "event:source-1",
                Some(start),
                Some(end),
            ),
            Err(StoreError::Conflict {
                resource: "event mapping"
            })
        ));
        assert!(matches!(
            store.attach_event_mapping(
                proposal.id(),
                "google",
                "event-2",
                "event:source-2",
                None,
                None,
            ),
            Err(StoreError::Conflict {
                resource: "event mapping"
            })
        ));

        assert_eq!(
            store
                .load_event_mapping_by_id(mapping.id())
                .expect("load by id"),
            mapping
        );
        assert_eq!(
            store
                .load_event_mapping_by_proposal_id(proposal.id())
                .expect("load by proposal"),
            mapping
        );
        assert_eq!(
            store
                .load_event_mapping_by_provider_event("google", "event-1")
                .expect("load by provider event"),
            mapping
        );
        assert_eq!(
            store
                .load_event_mapping_by_source_id("event:source-1")
                .expect("load by source"),
            mapping
        );

        store
            .delete_proposal_by_id(proposal.id())
            .expect("cascade proposal delete");
        assert!(matches!(
            store.load_event_mapping_by_id(mapping.id()),
            Err(StoreError::NotFound {
                resource: "event mapping"
            })
        ));
    }

    #[test]
    fn delete_event_mapping_by_id_removes_only_the_mapping() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = store
            .save_appointment_draft(
                "appointment:event-mapping-delete",
                &appointment(
                    AppointmentKind::Callback,
                    draft_time(),
                    QuoteId::new(),
                    "event-mapping-delete-draft",
                    false,
                ),
            )
            .expect("appointment draft");
        let proposal = store
            .create_proposal(
                "event-mapping-delete-proposal-key",
                "proposal:event-mapping-delete",
                ProposalSource::AppointmentDraft { id: draft.id() },
            )
            .expect("proposal");
        let mapping = store
            .attach_event_mapping(
                proposal.id(),
                "google",
                "event-delete",
                "event:delete",
                None,
                None,
            )
            .expect("mapping");

        store
            .delete_event_mapping_by_id(mapping.id())
            .expect("delete mapping");
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM event_mappings", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("mapping count"),
            0_i64
        );
        assert!(matches!(
            store.load_event_mapping_by_id(mapping.id()),
            Err(StoreError::NotFound {
                resource: "event mapping"
            })
        ));
        assert!(matches!(
            store.delete_event_mapping_by_id(mapping.id()),
            Err(StoreError::NotFound {
                resource: "event mapping"
            })
        ));
        assert_eq!(
            store
                .load_proposal_by_id(proposal.id())
                .expect("proposal remains"),
            proposal
        );
    }

    #[test]
    fn event_mapping_corruption_is_strict_and_redacted() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = store
            .save_appointment_draft(
                "appointment:event-corrupt",
                &appointment(
                    AppointmentKind::Callback,
                    draft_time(),
                    QuoteId::new(),
                    "event-corrupt-draft",
                    false,
                ),
            )
            .expect("appointment draft");
        let proposal = store
            .create_proposal(
                "event-corrupt-proposal",
                "proposal:event-corrupt",
                ProposalSource::AppointmentDraft { id: draft.id() },
            )
            .expect("proposal");
        let mapping = store
            .attach_event_mapping(
                proposal.id(),
                "google",
                "event-sensitive",
                "event:source-sensitive",
                None,
                None,
            )
            .expect("mapping");
        store
            .connection()
            .execute(
                "UPDATE event_mappings SET provider = '', starts_at = 'not-a-time', ends_at = NULL WHERE id = ?1",
                [mapping.id()],
            )
            .expect("corrupt event mapping");
        let error = store
            .load_event_mapping_by_id(mapping.id())
            .expect_err("corrupt mapping must fail");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "event mapping"
            }
        ));
        assert!(!error.to_string().contains("event-sensitive"));
        assert!(!format!("{error:?}").contains("event-sensitive"));
    }

    #[test]
    fn event_mapping_rejects_malformed_storage_timestamp_without_leaking_identifier() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let draft = store
            .save_appointment_draft(
                "appointment:event-timestamp-corrupt",
                &appointment(
                    AppointmentKind::Callback,
                    draft_time(),
                    QuoteId::new(),
                    "event-timestamp-corrupt-draft",
                    false,
                ),
            )
            .expect("appointment draft");
        let proposal = store
            .create_proposal(
                "event-timestamp-corrupt-proposal",
                "proposal:event-timestamp-corrupt",
                ProposalSource::AppointmentDraft { id: draft.id() },
            )
            .expect("proposal");
        let mapping = store
            .attach_event_mapping(
                proposal.id(),
                "google",
                "event-timestamp-sensitive",
                "event:timestamp-sensitive",
                None,
                None,
            )
            .expect("mapping");

        store
            .connection()
            .execute(
                "UPDATE event_mappings SET created_at = 'not-a-timestamp' WHERE id = ?1",
                [mapping.id()],
            )
            .expect("corrupt event mapping timestamp");
        let error = store
            .load_event_mapping_by_id(mapping.id())
            .expect_err("malformed event mapping timestamp must fail");

        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "event mapping"
            }
        ));
        assert!(!error.to_string().contains("event-timestamp-sensitive"));
        assert!(!format!("{error:?}").contains("event-timestamp-sensitive"));
    }

    fn notification_time(hours: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000 + hours * 3_600)
            .expect("valid notification timestamp")
    }

    fn notification_template(
        title: Option<&str>,
        proposal_state: Option<ProposalState>,
    ) -> NotificationTemplateData {
        NotificationTemplateData::new(
            title.map(str::to_owned),
            Some(notification_time(1)),
            Some(notification_time(2)),
            Some("Australia/Sydney".to_owned()),
            Some(AppointmentKind::Meeting),
            proposal_state,
        )
        .expect("valid notification template")
    }

    fn notification_recipient() -> NotificationRecipient {
        NotificationRecipient::new("owner.notification@example.com")
            .expect("valid notification recipient")
    }

    fn notification_proposal_graph_with_suffix(
        store: &PaStore,
        suffix: &str,
    ) -> (super::StoredProposal, super::StoredEventMapping) {
        let source_id = format!("appointment:notification:{suffix}");
        let draft_key = format!("notification-draft-{suffix}");
        let draft = store
            .save_appointment_draft(
                source_id,
                &appointment(
                    AppointmentKind::Meeting,
                    draft_time(),
                    QuoteId::new(),
                    &draft_key,
                    true,
                ),
            )
            .expect("appointment draft");
        let proposal_key = format!("notification-proposal-{suffix}");
        let proposal_source = format!("proposal:notification:{suffix}");
        let proposal = store
            .create_proposal(
                proposal_key,
                proposal_source,
                ProposalSource::AppointmentDraft { id: draft.id() },
            )
            .expect("proposal");
        let event_id = format!("notification-event-{suffix}");
        let event_source = format!("event:notification:{suffix}");
        let mapping = store
            .attach_event_mapping(proposal.id(), "google", event_id, event_source, None, None)
            .expect("event mapping");
        (proposal, mapping)
    }

    fn notification_proposal_graph(
        store: &PaStore,
    ) -> (super::StoredProposal, super::StoredEventMapping) {
        notification_proposal_graph_with_suffix(store, "one")
    }

    #[test]
    fn notification_kinds_are_closed_and_use_stable_wire_names() {
        let kinds = [
            (NotificationKind::CallSummary, "call_summary"),
            (NotificationKind::ProposalRequested, "proposal_requested"),
            (NotificationKind::ProposalAccepted, "proposal_accepted"),
            (NotificationKind::ProposalDeclined, "proposal_declined"),
            (NotificationKind::ProposalExpired, "proposal_expired"),
        ];
        for (kind, wire_name) in kinds {
            assert_eq!(
                serde_json::to_string(&kind).expect("kind JSON"),
                format!("\"{wire_name}\"")
            );
        }
        assert!(serde_json::from_str::<NotificationKind>("\"reminder\"").is_err());
    }

    #[test]
    fn notification_template_data_round_trips_only_structured_fields() {
        let data = notification_template(Some("Planning meeting"), Some(ProposalState::Pending));
        let encoded = serde_json::to_string(&data).expect("template JSON");
        let value: serde_json::Value = serde_json::from_str(&encoded).expect("template object");
        let object = value.as_object().expect("template object map");
        assert_eq!(object.len(), 6);
        for field in [
            "title",
            "starts_at",
            "ends_at",
            "timezone",
            "appointment_kind",
            "proposal_state",
        ] {
            assert!(
                object.contains_key(field),
                "missing structured field {field}"
            );
        }
        for forbidden in [
            "body",
            "message",
            "transcript",
            "token",
            "provider",
            "payload",
        ] {
            assert!(!encoded.contains(forbidden), "raw field {forbidden} leaked");
        }
        assert_eq!(
            serde_json::from_str::<NotificationTemplateData>(&encoded).expect("template decode"),
            data
        );
        assert!(serde_json::from_str::<NotificationTemplateData>(
            r#"{"title":null,"starts_at":null,"ends_at":null,"timezone":null,"appointment_kind":null,"proposal_state":null,"body":"raw"}"#
        )
        .is_err());
    }

    #[test]
    fn notification_recipient_is_validated_and_redacted() {
        assert!(NotificationRecipient::new("  ").is_err());
        assert!(NotificationRecipient::new("not-an-email").is_err());
        let recipient = notification_recipient();
        let debug = format!("{recipient:?}");
        assert!(!debug.contains("owner.notification@example.com"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn notification_enqueue_round_trips_all_kinds_with_pending_zero_attempts() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (proposal, _) = notification_proposal_graph(&store);
        let cases = [
            (NotificationKind::CallSummary, None, None, None),
            (
                NotificationKind::ProposalRequested,
                Some(proposal.id()),
                None,
                Some(ProposalState::Pending),
            ),
            (
                NotificationKind::ProposalAccepted,
                Some(proposal.id()),
                None,
                Some(ProposalState::Accepted),
            ),
            (
                NotificationKind::ProposalDeclined,
                Some(proposal.id()),
                None,
                Some(ProposalState::Declined),
            ),
            (
                NotificationKind::ProposalExpired,
                Some(proposal.id()),
                None,
                Some(ProposalState::Expired),
            ),
        ];
        for (index, (kind, proposal_id, mapping_id, state)) in cases.into_iter().enumerate() {
            let stored = store
                .enqueue_notification(
                    format!("notification-key-{index}"),
                    proposal_id,
                    mapping_id,
                    kind,
                    notification_recipient(),
                    notification_template(Some("Planning meeting"), state),
                    notification_time(index as i64),
                )
                .expect("enqueue notification");
            assert_eq!(stored.kind(), kind);
            assert_eq!(stored.proposal_id(), proposal_id);
            assert_eq!(stored.event_mapping_id(), mapping_id);
            assert_eq!(
                stored.recipient().as_str(),
                "owner.notification@example.com"
            );
            assert_eq!(stored.attempts(), 0);
            assert_eq!(stored.status(), NotificationStatus::Pending);
            assert_eq!(
                stored.template_data(),
                &notification_template(Some("Planning meeting"), state)
            );
            assert_eq!(
                store
                    .load_notification_by_id(stored.id())
                    .expect("load by ID"),
                stored
            );
            assert_eq!(
                store
                    .load_notification_by_idempotency_key(stored.idempotency_key())
                    .expect("load by key"),
                stored
            );
        }
    }

    #[test]
    fn notification_enqueue_validates_references_and_kind_consistency() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (proposal, mapping) = notification_proposal_graph(&store);
        let (_, other_mapping) = notification_proposal_graph_with_suffix(&store, "two");
        let data = notification_template(None, Some(ProposalState::Pending));
        let recipient = notification_recipient();
        assert!(matches!(
            store.enqueue_notification(
                "  ",
                None,
                None,
                NotificationKind::CallSummary,
                recipient.clone(),
                data.clone(),
                notification_time(0)
            ),
            Err(StoreError::InvalidInput {
                field: "idempotency_key"
            })
        ));
        assert!(matches!(
            store.enqueue_notification(
                "missing-proposal",
                Some(999),
                None,
                NotificationKind::ProposalRequested,
                recipient.clone(),
                data.clone(),
                notification_time(0)
            ),
            Err(StoreError::NotFound {
                resource: "proposal"
            })
        ));
        assert!(matches!(
            store.enqueue_notification(
                "missing-event",
                Some(proposal.id()),
                Some(999),
                NotificationKind::ProposalRequested,
                recipient.clone(),
                data.clone(),
                notification_time(0)
            ),
            Err(StoreError::NotFound {
                resource: "event mapping"
            })
        ));
        assert!(matches!(
            store.enqueue_notification(
                "mapping-without-proposal",
                None,
                Some(mapping.id()),
                NotificationKind::ProposalRequested,
                recipient.clone(),
                data.clone(),
                notification_time(0)
            ),
            Err(StoreError::InvalidInput {
                field: "proposal_id"
            })
        ));
        assert!(matches!(
            store.enqueue_notification(
                "mapping-mismatch",
                Some(proposal.id()),
                Some(other_mapping.id()),
                NotificationKind::ProposalRequested,
                recipient.clone(),
                data.clone(),
                notification_time(0)
            ),
            Err(StoreError::Conflict {
                resource: "notification"
            })
        ));
        assert!(matches!(
            store.enqueue_notification(
                "call-summary-proposal",
                Some(proposal.id()),
                None,
                NotificationKind::CallSummary,
                recipient.clone(),
                data.clone(),
                notification_time(0)
            ),
            Err(StoreError::InvalidInput {
                field: "proposal_id"
            })
        ));
        assert!(matches!(
            store.enqueue_notification(
                "call-summary-event",
                None,
                Some(mapping.id()),
                NotificationKind::CallSummary,
                recipient,
                data,
                notification_time(0)
            ),
            Err(StoreError::InvalidInput {
                field: "event_mapping_id"
            })
        ));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM notification_outbox", [], |row| row
                    .get::<_, i64>(0))
                .expect("count"),
            0
        );
    }

    #[test]
    fn notification_retry_is_exact_and_conflicts_leave_row_unchanged() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (proposal, mapping) = notification_proposal_graph(&store);
        let (other_proposal, _) = notification_proposal_graph_with_suffix(&store, "retry-other");
        let stored = store
            .enqueue_notification(
                "notification-retry",
                Some(proposal.id()),
                None,
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(Some("Original"), Some(ProposalState::Pending)),
                notification_time(0),
            )
            .expect("first notification");
        let retry = store
            .enqueue_notification(
                "notification-retry",
                Some(proposal.id()),
                None,
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(Some("Original"), Some(ProposalState::Pending)),
                notification_time(0),
            )
            .expect("exact retry");
        assert_eq!(retry, stored);
        let original_created_at = stored.created_at().to_owned();
        let original_updated_at = stored.updated_at().to_owned();
        let mut conflicts = vec![
            (
                Some(proposal.id()),
                None,
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(Some("Changed"), Some(ProposalState::Pending)),
                notification_time(0),
            ),
            (
                Some(proposal.id()),
                None,
                NotificationKind::ProposalRequested,
                NotificationRecipient::new("other@example.com").expect("recipient"),
                notification_template(Some("Original"), Some(ProposalState::Pending)),
                notification_time(0),
            ),
            (
                Some(proposal.id()),
                None,
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(Some("Original"), Some(ProposalState::Pending)),
                notification_time(1),
            ),
            (
                Some(other_proposal.id()),
                None,
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(Some("Original"), Some(ProposalState::Pending)),
                notification_time(0),
            ),
            (
                Some(proposal.id()),
                Some(mapping.id()),
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(Some("Original"), Some(ProposalState::Pending)),
                notification_time(0),
            ),
            (
                Some(proposal.id()),
                None,
                NotificationKind::ProposalAccepted,
                notification_recipient(),
                notification_template(Some("Original"), Some(ProposalState::Accepted)),
                notification_time(0),
            ),
        ];
        for (proposal_id, event_mapping_id, kind, recipient, data, available_at) in
            conflicts.drain(..)
        {
            let error = store
                .enqueue_notification(
                    "notification-retry",
                    proposal_id,
                    event_mapping_id,
                    kind,
                    recipient,
                    data,
                    available_at,
                )
                .expect_err("changed immutable input must conflict");
            assert!(matches!(
                error,
                StoreError::Conflict {
                    resource: "notification"
                }
            ));
            assert!(!error.to_string().contains("notification-retry"));
        }
        assert_eq!(
            store
                .load_notification_by_id(stored.id())
                .expect("original remains"),
            stored
        );
        let unchanged = store
            .load_notification_by_id(stored.id())
            .expect("unchanged original");
        assert_eq!(unchanged.created_at(), original_created_at);
        assert_eq!(unchanged.updated_at(), original_updated_at);
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM notification_outbox", [], |row| row
                    .get::<_, i64>(0))
                .expect("count"),
            1
        );
    }

    #[test]
    fn notification_loader_rejects_a_mapping_that_is_corrupted_to_another_proposal() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let (proposal, mapping) = notification_proposal_graph(&store);
        let other_draft = store
            .save_owner_task_draft(
                Some("owner:notification-corruption"),
                &owner_task(
                    TaskKind::Callback,
                    "Call supplier",
                    15,
                    None,
                    "notification-corruption-draft",
                ),
            )
            .expect("other proposal draft");
        let other_proposal = store
            .create_proposal(
                "notification-corruption-proposal",
                "proposal:notification-corruption",
                ProposalSource::OwnerTaskDraft {
                    id: other_draft.id(),
                },
            )
            .expect("other proposal");
        let notification = store
            .enqueue_notification(
                "notification-corruption",
                Some(proposal.id()),
                Some(mapping.id()),
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(None, Some(ProposalState::Pending)),
                notification_time(0),
            )
            .expect("notification");

        store
            .connection()
            .execute(
                "UPDATE notification_outbox SET proposal_id = ?1 WHERE id = ?2",
                rusqlite::params![other_proposal.id(), notification.id()],
            )
            .expect("corrupt notification reference");

        assert!(matches!(
            store.load_notification_by_id(notification.id()),
            Err(StoreError::StoredRecordInvalid {
                resource: "notification"
            })
        ));
        let by_key_error = store
            .load_notification_by_idempotency_key("notification-corruption")
            .expect_err("corrupt notification must fail by key");
        assert!(matches!(
            &by_key_error,
            StoreError::StoredRecordInvalid {
                resource: "notification"
            }
        ));
        let pending_error = store
            .list_pending_notifications()
            .expect_err("corrupt notification must fail in pending list");
        assert!(matches!(
            &pending_error,
            StoreError::StoredRecordInvalid {
                resource: "notification"
            }
        ));
        for error in [by_key_error, pending_error] {
            assert!(!error.to_string().contains("notification-corruption"));
            assert!(!format!("{error:?}").contains("notification-corruption"));
        }
    }

    #[test]
    fn pending_notifications_are_ordered_by_available_at_then_id_and_delete_cascades() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let first = store
            .enqueue_notification(
                "order-a",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(2),
            )
            .expect("first");
        let second = store
            .enqueue_notification(
                "order-b",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(1),
            )
            .expect("second");
        let third = store
            .enqueue_notification(
                "order-c",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(2),
            )
            .expect("third");
        let pending = store
            .list_pending_notifications()
            .expect("pending notifications");
        assert_eq!(
            pending
                .iter()
                .map(|notification| notification.id())
                .collect::<Vec<_>>(),
            vec![second.id(), first.id(), third.id()]
        );
        store
            .delete_notification_by_id(second.id())
            .expect("delete notification");
        assert!(matches!(
            store.load_notification_by_id(second.id()),
            Err(StoreError::NotFound {
                resource: "notification"
            })
        ));
        assert!(matches!(
            store.delete_notification_by_id(second.id()),
            Err(StoreError::NotFound {
                resource: "notification"
            })
        ));

        let (proposal, mapping) = notification_proposal_graph(&store);
        let by_proposal = store
            .enqueue_notification(
                "cascade-proposal",
                Some(proposal.id()),
                None,
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(None, Some(ProposalState::Pending)),
                notification_time(0),
            )
            .expect("proposal notification");
        let by_mapping = store
            .enqueue_notification(
                "cascade-mapping",
                Some(proposal.id()),
                Some(mapping.id()),
                NotificationKind::ProposalRequested,
                notification_recipient(),
                notification_template(None, Some(ProposalState::Pending)),
                notification_time(0),
            )
            .expect("mapping notification");
        store
            .delete_event_mapping_by_id(mapping.id())
            .expect("delete mapping");
        assert!(matches!(
            store.load_notification_by_id(by_mapping.id()),
            Err(StoreError::NotFound {
                resource: "notification"
            })
        ));
        assert_eq!(
            store
                .load_notification_by_id(by_proposal.id())
                .expect("proposal notification remains"),
            by_proposal
        );
        store
            .delete_proposal_by_id(proposal.id())
            .expect("delete proposal");
        assert!(matches!(
            store.load_notification_by_id(by_proposal.id()),
            Err(StoreError::NotFound {
                resource: "notification"
            })
        ));
    }

    #[test]
    fn corrupted_notification_rows_fail_generically_without_redaction_leaks() {
        for (column, value) in [
            ("notification_kind", "not-a-kind"),
            ("payload", "not-json"),
            ("available_at", "not-a-time"),
            ("recipient", "not-an-email"),
        ] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let notification = store
                .enqueue_notification(
                    "corrupt-key",
                    None,
                    None,
                    NotificationKind::CallSummary,
                    notification_recipient(),
                    notification_template(Some("sensitive-template"), None),
                    notification_time(0),
                )
                .expect("notification");
            store
                .connection()
                .execute(
                    &format!("UPDATE notification_outbox SET {column} = ?1 WHERE id = ?2"),
                    rusqlite::params![value, notification.id()],
                )
                .expect("corrupt row");
            let error = store
                .load_notification_by_id(notification.id())
                .expect_err("corrupt row must fail");
            assert!(matches!(
                error,
                StoreError::StoredRecordInvalid {
                    resource: "notification"
                }
            ));
            assert!(!error.to_string().contains("sensitive-template"));
            assert!(!format!("{error:?}").contains("sensitive-template"));
            assert!(!format!("{notification:?}").contains("owner.notification@example.com"));
            assert!(!format!("{notification:?}").contains("sensitive-template"));
        }
    }

    #[test]
    fn claim_notifications_orders_limits_and_leases_each_attempt_once() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let first = store
            .enqueue_notification(
                "claim-first",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(0),
            )
            .expect("first notification");
        let second = store
            .enqueue_notification(
                "claim-second",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(1),
            )
            .expect("second notification");
        let third = store
            .enqueue_notification(
                "claim-third",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(2),
            )
            .expect("third notification");
        let now = notification_time(3);

        let claimed = store
            .claim_notifications(now, 2, TimeDuration::minutes(5))
            .expect("claim eligible notifications");

        assert_eq!(
            claimed
                .iter()
                .map(|notification| notification.id())
                .collect::<Vec<_>>(),
            vec![first.id(), second.id()]
        );
        for notification in &claimed {
            assert_eq!(notification.status(), NotificationStatus::Delivering);
            assert_eq!(notification.attempts(), 1);
            assert_eq!(
                notification.lease_until(),
                Some(now + TimeDuration::minutes(5))
            );
            assert_eq!(notification.sent_at(), None);
            assert_eq!(notification.last_error_code(), None);
        }
        assert_eq!(
            store
                .load_notification_by_id(third.id())
                .expect("unclaimed notification")
                .status(),
            NotificationStatus::Pending
        );
    }

    #[test]
    fn notification_claim_rejects_invalid_limits_and_leases_without_mutation() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let notification = store
            .enqueue_notification(
                "claim-invalid-inputs",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(0),
            )
            .expect("notification");
        let now = notification_time(1);

        for lease_duration in [TimeDuration::ZERO, TimeDuration::seconds(-1)] {
            assert!(matches!(
                store.claim_notifications(now, 1, lease_duration),
                Err(StoreError::InvalidInput {
                    field: "lease_duration"
                })
            ));
        }
        assert!(matches!(
            store.claim_notifications(now, 0, TimeDuration::seconds(1)),
            Err(StoreError::InvalidInput { field: "limit" })
        ));
        assert_eq!(
            store
                .load_notification_by_id(notification.id())
                .expect("unchanged notification"),
            notification
        );
    }

    #[test]
    fn notification_claim_recovers_expired_leases_but_excludes_active_ones() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let notification = store
            .enqueue_notification(
                "claim-recovery",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(0),
            )
            .expect("notification");
        let first_now = notification_time(1);
        let first = store
            .claim_notifications(first_now, 1, TimeDuration::minutes(5))
            .expect("first claim");
        assert_eq!(first[0].attempts(), 1);
        assert!(
            store
                .claim_notifications(
                    first_now + TimeDuration::minutes(4),
                    1,
                    TimeDuration::minutes(5)
                )
                .expect("active lease claim")
                .is_empty()
        );

        let recovered = store
            .claim_notifications(
                first_now + TimeDuration::minutes(5),
                1,
                TimeDuration::minutes(5),
            )
            .expect("expired lease recovery");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id(), notification.id());
        assert_eq!(recovered[0].status(), NotificationStatus::Delivering);
        assert_eq!(recovered[0].attempts(), 2);
        assert_eq!(
            recovered[0].lease_until(),
            Some(first_now + TimeDuration::minutes(10))
        );
    }

    #[test]
    fn notification_claim_does_not_treat_fractional_future_time_as_due() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let base = notification_time(0);
        let notification = store
            .enqueue_notification(
                "claim-fractional-available-at",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                base + TimeDuration::milliseconds(100),
            )
            .expect("notification");

        assert!(
            store
                .claim_notifications(base, 1, TimeDuration::minutes(5))
                .expect("future notification is not claimed early")
                .is_empty()
        );
        let claimed = store
            .claim_notifications(
                base + TimeDuration::milliseconds(100),
                1,
                TimeDuration::minutes(5),
            )
            .expect("fractional notification becomes due");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id(), notification.id());
    }

    #[test]
    fn notification_claim_does_not_reclaim_fractional_lease_before_expiry() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let base = notification_time(0);
        let notification = store
            .enqueue_notification(
                "claim-fractional-lease-unexpired",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                base - TimeDuration::seconds(1),
            )
            .expect("notification");
        let first_now = base + TimeDuration::milliseconds(100);
        let first = store
            .claim_notifications(first_now, 1, TimeDuration::minutes(5))
            .expect("first claim");
        assert_eq!(first.len(), 1);
        assert_eq!(
            first[0].lease_until(),
            Some(first_now + TimeDuration::minutes(5))
        );

        assert!(
            store
                .claim_notifications(base + TimeDuration::minutes(5), 1, TimeDuration::minutes(5))
                .expect("lease is still active")
                .is_empty()
        );
        let recovered = store
            .claim_notifications(
                first_now + TimeDuration::minutes(5),
                1,
                TimeDuration::minutes(5),
            )
            .expect("lease becomes reclaimable at expiry");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id(), notification.id());
        assert_eq!(recovered[0].attempts(), 2);
    }

    #[test]
    fn notification_claims_are_atomic_across_file_backed_store_instances() {
        let database = TempDatabase::new();
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("open seed store");
        let notification = seed
            .enqueue_notification(
                "claim-race",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(0),
            )
            .expect("notification");
        drop(seed);

        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let now = notification_time(1);
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.claim_notifications(now, 1, TimeDuration::minutes(5))
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.claim_notifications(now, 1, TimeDuration::minutes(5))
        });
        let first_claim = first_handle.join().expect("first claimant panicked");
        let second_claim = second_handle.join().expect("second claimant panicked");
        let claimed = [
            first_claim.expect("first claim"),
            second_claim.expect("second claim"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id(), notification.id());
        assert_eq!(claimed[0].attempts(), 1);
    }

    #[test]
    fn notification_completion_is_idempotent_only_for_the_same_delivery_cursor() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let notification = store
            .enqueue_notification(
                "mark-sent",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(0),
            )
            .expect("notification");
        let claimed = store
            .claim_notifications(notification_time(1), 1, TimeDuration::minutes(5))
            .expect("claim");
        let sent_at = notification_time(2);

        let sent = store
            .mark_notification_sent(notification.id(), claimed[0].attempts(), sent_at)
            .expect("mark sent");
        assert_eq!(sent.status(), NotificationStatus::Sent);
        assert_eq!(sent.sent_at(), Some(sent_at));
        assert_eq!(sent.lease_until(), None);
        assert_eq!(sent.last_error_code(), None);
        let retry = store
            .mark_notification_sent(notification.id(), claimed[0].attempts(), sent_at)
            .expect("idempotent sent retry");
        assert_eq!(retry, sent);
        assert!(matches!(
            store.mark_notification_sent(
                notification.id(),
                claimed[0].attempts(),
                sent_at + TimeDuration::seconds(1)
            ),
            Err(StoreError::CursorConflict {
                resource: "notification"
            })
        ));
        assert_eq!(
            store
                .load_notification_by_id(notification.id())
                .expect("sent row remains"),
            sent
        );
        assert!(
            store
                .claim_notifications(notification_time(100), 1, TimeDuration::minutes(5))
                .expect("sent rows are terminal")
                .is_empty()
        );
    }

    #[test]
    fn notification_reschedule_validates_safe_codes_and_stale_cursors_leave_rows_unchanged() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let notification = store
            .enqueue_notification(
                "reschedule",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(None, None),
                notification_time(0),
            )
            .expect("notification");
        let claimed = store
            .claim_notifications(notification_time(1), 1, TimeDuration::minutes(5))
            .expect("claim");
        let unsafe_provider_text = "provider response: recipient@example.com payload=private";
        let invalid = store
            .reschedule_notification(
                notification.id(),
                claimed[0].attempts(),
                notification_time(2),
                unsafe_provider_text,
            )
            .expect_err("provider text must be rejected");
        assert!(matches!(
            invalid,
            StoreError::InvalidInput {
                field: "error_code"
            }
        ));
        assert!(!invalid.to_string().contains(unsafe_provider_text));
        assert!(!format!("{invalid:?}").contains(unsafe_provider_text));
        assert_eq!(
            store
                .load_notification_by_id(notification.id())
                .expect("row remains delivering"),
            claimed[0]
        );

        let pending = store
            .reschedule_notification(
                notification.id(),
                claimed[0].attempts(),
                notification_time(3),
                "retryable_timeout-1",
            )
            .expect("reschedule");
        assert_eq!(pending.status(), NotificationStatus::Pending);
        assert_eq!(pending.lease_until(), None);
        assert_eq!(pending.sent_at(), None);
        assert_eq!(pending.last_error_code(), Some("retryable_timeout-1"));
        assert_eq!(pending.available_at(), notification_time(3));

        for invalid_code in ["", "contains space", "contains/slash", "é", &"x".repeat(65)] {
            assert!(matches!(
                store.reschedule_notification(
                    notification.id(),
                    claimed[0].attempts(),
                    notification_time(4),
                    invalid_code,
                ),
                Err(StoreError::InvalidInput {
                    field: "error_code"
                })
            ));
        }
        let recovered = store
            .claim_notifications(notification_time(3), 1, TimeDuration::minutes(5))
            .expect("claim rescheduled row");
        assert_eq!(recovered[0].attempts(), 2);
        let unchanged = recovered[0].clone();
        assert!(matches!(
            store.reschedule_notification(notification.id(), 1, notification_time(4), "stale"),
            Err(StoreError::CursorConflict {
                resource: "notification"
            })
        ));
        assert_eq!(
            store
                .load_notification_by_id(notification.id())
                .expect("stale cursor leaves row unchanged"),
            unchanged
        );
    }

    #[test]
    fn notification_delivery_rejects_invalid_cursors_and_corrupted_state_without_leaks() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let notification = store
            .enqueue_notification(
                "delivery-corruption",
                None,
                None,
                NotificationKind::CallSummary,
                notification_recipient(),
                notification_template(Some("sensitive body-like title"), None),
                notification_time(0),
            )
            .expect("notification");
        let claimed = store
            .claim_notifications(notification_time(1), 1, TimeDuration::minutes(5))
            .expect("claim");

        for (id, attempt) in [(0, 1), (notification.id(), 0)] {
            assert!(matches!(
                store.mark_notification_sent(id, attempt, notification_time(2)),
                Err(StoreError::InvalidInput { .. })
            ));
            assert!(matches!(
                store.reschedule_notification(id, attempt, notification_time(2), "safe_code"),
                Err(StoreError::InvalidInput { .. })
            ));
        }
        store
            .connection()
            .execute(
                "UPDATE notification_outbox \
                 SET lease_until = 'provider error recipient@example.com payload=private' \
                 WHERE id = ?1",
                [claimed[0].id()],
            )
            .expect("corrupt lease");
        let error = store
            .load_notification_by_id(notification.id())
            .expect_err("corrupted delivery state must fail");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "notification"
            }
        ));
        let rendered = error.to_string();
        let debug = format!("{error:?}");
        for secret in [
            "provider error recipient@example.com payload=private",
            "owner.notification@example.com",
            "sensitive body-like title",
        ] {
            assert!(!rendered.contains(secret));
            assert!(!debug.contains(secret));
        }
    }

    #[test]
    fn replay_nonce_is_persistent_and_reusable_at_exact_expiry() {
        let database = TempDatabase::new();
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let now = 1_700_000_000;
        let nonce = random_replay_nonce();

        assert!(
            first
                .consume_replay_nonce(&nonce, now)
                .expect("first consume")
        );
        assert!(
            !first
                .consume_replay_nonce(&nonce, now)
                .expect("duplicate consume")
        );
        drop(first);

        let reopened = PaStore::open(&database.path, DATABASE_KEY).expect("reopen store");
        assert!(
            !reopened
                .consume_replay_nonce(&nonce, now)
                .expect("reopened duplicate consume")
        );
        assert!(
            reopened
                .consume_replay_nonce(&nonce, now + crate::pa::auth::REPLAY_RETENTION_SECONDS,)
                .expect("expiry-boundary consume")
        );

        let stored: (String, String) = reopened
            .connection()
            .query_row(
                "SELECT consumed_at, expires_at FROM replay_nonces WHERE nonce = ?1",
                [nonce.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("stored replay row");
        assert_eq!(stored.0, "2023-11-14T22:18:20Z");
        assert_eq!(stored.1, "2023-11-14T22:23:20Z");
    }

    #[test]
    fn replay_nonce_purges_expired_rows_but_keeps_live_rows() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let now = 1_700_000_000;
        let expired_nonce = random_replay_nonce();
        let live_nonce = random_replay_nonce();
        let new_nonce = random_replay_nonce();
        store
            .connection()
            .execute(
                "INSERT INTO replay_nonces (nonce, consumed_at, expires_at)
                 VALUES (?1, ?2, ?3), (?4, ?5, ?6)",
                rusqlite::params![
                    &expired_nonce,
                    "2023-11-14T22:08:20Z",
                    "2023-11-14T22:13:20Z",
                    &live_nonce,
                    "2023-11-14T22:08:21Z",
                    "2023-11-14T22:13:21Z",
                ],
            )
            .expect("seed replay rows");

        assert!(
            store
                .consume_replay_nonce(&new_nonce, now)
                .expect("new consume")
        );
        let rows = store
            .connection()
            .prepare("SELECT nonce FROM replay_nonces ORDER BY nonce")
            .expect("prepare replay rows")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query replay rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect replay rows");
        let mut expected = vec![live_nonce, new_nonce];
        expected.sort();
        assert_eq!(rows, expected);
    }

    #[test]
    fn replay_nonce_file_stores_accept_exactly_once_under_race() {
        let database = TempDatabase::new();
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let nonce = random_replay_nonce();
        let first_nonce = nonce.clone();
        let second_nonce = nonce.clone();
        let now = 1_700_000_000;

        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.consume_replay_nonce(&first_nonce, now)
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.consume_replay_nonce(&second_nonce, now)
        });
        let results = [
            first_handle.join().expect("first replay claimant panicked"),
            second_handle
                .join()
                .expect("second replay claimant panicked"),
        ];
        assert!(results.iter().all(|result| result.is_ok()));
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().ok())
                .filter(|accepted| **accepted)
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().ok())
                .filter(|accepted| !**accepted)
                .count(),
            1
        );

        let verifier = PaStore::open(&database.path, DATABASE_KEY).expect("reopen verifier");
        let count: i64 = verifier
            .connection()
            .query_row(
                "SELECT count(*) FROM replay_nonces WHERE nonce = ?1",
                [nonce.as_str()],
                |row| row.get(0),
            )
            .expect("count retained replay row");
        assert_eq!(count, 1);
    }

    #[test]
    fn replay_nonce_rejects_invalid_input_without_mutating_rows_or_leaking_it() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let expired_nonce = random_replay_nonce();
        let live_nonce = random_replay_nonce();
        let mut short_nonce = random_replay_nonce();
        let short_length = usize::from(random_replay_nonce().as_bytes()[0] & 0x0f);
        short_nonce.truncate(short_length);
        let mut invalid_character_nonce = random_replay_nonce();
        invalid_character_nonce.push('!');
        let mut secret_nonce = random_replay_nonce();
        secret_nonce.push('!');
        store
            .connection()
            .execute(
                "INSERT INTO replay_nonces (nonce, consumed_at, expires_at)
                 VALUES (?1, ?2, ?3), (?4, ?5, ?6)",
                rusqlite::params![
                    &expired_nonce,
                    "2023-11-14T22:08:20Z",
                    "2023-11-14T22:13:20Z",
                    &live_nonce,
                    "2023-11-14T22:13:19Z",
                    "2023-11-14T22:13:21Z",
                ],
            )
            .expect("seed replay rows");
        let snapshot = || {
            let mut statement = store
                .connection()
                .prepare(
                    "SELECT id, nonce, consumed_at, expires_at, created_at
                     FROM replay_nonces ORDER BY id",
                )
                .expect("prepare replay snapshot");
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .expect("query replay snapshot")
                .collect::<rusqlite::Result<Vec<_>>>()
                .expect("collect replay snapshot")
        };

        for nonce in [&short_nonce, &invalid_character_nonce, &secret_nonce] {
            let before = snapshot();
            let error = store
                .consume_replay_nonce(nonce, 1_700_000_000)
                .expect_err("invalid nonce");
            assert!(matches!(error, StoreError::InvalidInput { field: "nonce" }));
            assert!(!error.to_string().contains(&secret_nonce));
            assert!(!format!("{error:?}").contains(&secret_nonce));
            assert_eq!(snapshot(), before);
        }
        let valid_nonce = random_replay_nonce();
        for now in [i64::MIN, i64::MAX] {
            let before = snapshot();
            let error = store
                .consume_replay_nonce(&valid_nonce, now)
                .expect_err("unrepresentable time");
            assert!(matches!(error, StoreError::InvalidInput { field: "now" }));
            assert_eq!(snapshot(), before);
        }
    }

    #[test]
    fn message_triage_transitions_follow_email_graph_and_exact_retries() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let updated_at =
            OffsetDateTime::parse("2025-01-02T03:04:06Z", &Rfc3339).expect("updated time");
        let message = store
            .record_message(
                "triage-key",
                "triage-source",
                MessageProvider::Outlook,
                "triage-provider-message",
                MessageSummary::new("triage summary").expect("summary"),
                None,
                None,
                received_at,
            )
            .expect("message");

        let actionable = store
            .transition_message(
                message.source_id(),
                MessageTriageState::Unprocessed,
                MessageTriageState::Actionable,
                updated_at,
            )
            .expect("actionable transition");
        assert_eq!(actionable.triage_state(), MessageTriageState::Actionable);
        assert_eq!(actionable.updated_at(), "2025-01-02T03:04:06Z");
        assert_eq!(
            store
                .transition_message(
                    message.source_id(),
                    MessageTriageState::Unprocessed,
                    MessageTriageState::Actionable,
                    updated_at,
                )
                .expect("exact retry"),
            actionable
        );
        let different_retry_time =
            OffsetDateTime::parse("2025-01-02T03:04:07Z", &Rfc3339).expect("retry time");
        assert!(matches!(
            store.transition_message(
                message.source_id(),
                MessageTriageState::Unprocessed,
                MessageTriageState::Actionable,
                different_retry_time,
            ),
            Err(StoreError::Conflict {
                resource: "message transition"
            })
        ));
        assert_eq!(
            store
                .load_message_by_id(message.id())
                .expect("unchanged actionable row"),
            actionable
        );

        let scheduled_at =
            OffsetDateTime::parse("2025-01-02T03:04:08Z", &Rfc3339).expect("scheduled time");
        let scheduled = store
            .transition_message(
                message.source_id(),
                MessageTriageState::Actionable,
                MessageTriageState::Scheduled,
                scheduled_at,
            )
            .expect("scheduled transition");
        assert_eq!(scheduled.triage_state(), MessageTriageState::Scheduled);

        let states = store
            .list_messages_by_triage_state(MessageTriageState::Scheduled, None, 10)
            .expect("state list");
        assert_eq!(
            states.iter().map(StoredMessage::id).collect::<Vec<_>>(),
            vec![message.id()]
        );
    }

    #[test]
    fn message_triage_rejects_forbidden_edges_and_invalid_inputs_without_mutation() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let message = store
            .record_message(
                "triage-invalid-key",
                "triage-invalid-source",
                MessageProvider::Gmail,
                "triage-invalid-provider-message",
                MessageSummary::new("triage summary").expect("summary"),
                None,
                None,
                received_at,
            )
            .expect("message");
        let before = message_snapshot(&store);
        let updated_at =
            OffsetDateTime::parse("2025-01-02T03:04:06Z", &Rfc3339).expect("updated time");
        for (expected, next) in [
            (
                MessageTriageState::Unprocessed,
                MessageTriageState::Scheduled,
            ),
            (
                MessageTriageState::Actionable,
                MessageTriageState::Scheduled,
            ),
            (MessageTriageState::Ambiguous, MessageTriageState::Scheduled),
            (
                MessageTriageState::Unprocessed,
                MessageTriageState::Unprocessed,
            ),
        ] {
            let error = store
                .transition_message(message.source_id(), expected, next, updated_at)
                .expect_err("forbidden transition");
            assert!(matches!(
                error,
                StoreError::Conflict {
                    resource: "message transition"
                }
            ));
            assert_eq!(message_snapshot(&store), before);
        }
        for (source_id, timestamp) in [
            ("missing-source", updated_at),
            (
                message.source_id(),
                updated_at.replace_nanosecond(1).expect("fractional"),
            ),
        ] {
            let error = store
                .transition_message(
                    source_id,
                    MessageTriageState::Unprocessed,
                    MessageTriageState::Actionable,
                    timestamp,
                )
                .expect_err("invalid transition input");
            assert!(matches!(
                error,
                StoreError::NotFound { .. } | StoreError::InvalidInput { .. }
            ));
            assert_eq!(message_snapshot(&store), before);
        }
    }

    #[test]
    fn message_triage_list_fails_closed_for_unknown_stored_state() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let message = store
            .record_message(
                "triage-corrupt-key",
                "triage-corrupt-source",
                MessageProvider::Outlook,
                "triage-corrupt-provider-message",
                MessageSummary::new("triage summary").expect("summary"),
                None,
                None,
                received_at,
            )
            .expect("message");
        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .expect("allow corruption fixture");
        store
            .connection()
            .execute(
                "UPDATE messages SET triage_state = 'corrupt-state' WHERE id = ?1",
                [message.id()],
            )
            .expect("corrupt stored state");

        let error = store
            .list_messages_by_triage_state(MessageTriageState::Unprocessed, None, 10)
            .expect_err("unknown state must not be silently filtered out");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "message"
            }
        ));
        assert!(!error.to_string().contains("corrupt-state"));
    }

    #[test]
    fn message_triage_preserves_voice_rows_and_rejects_provider_state_mismatches() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let updated_at =
            OffsetDateTime::parse("2025-01-02T03:04:06Z", &Rfc3339).expect("updated time");
        let voice = store
            .record_message(
                "triage-voice-key",
                "triage-voice-source",
                MessageProvider::Voice,
                "triage-voice-provider-message",
                MessageSummary::new("voice summary").expect("summary"),
                None,
                None,
                received_at,
            )
            .expect("voice");
        let before = message_snapshot(&store);
        assert!(matches!(
            store.transition_message(
                voice.source_id(),
                MessageTriageState::Unprocessed,
                MessageTriageState::Actionable,
                updated_at,
            ),
            Err(StoreError::Conflict {
                resource: "message transition"
            })
        ));
        assert_eq!(message_snapshot(&store), before);

        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .expect("allow corruption fixture");
        store
            .connection()
            .execute(
                "UPDATE messages SET triage_state = 'unprocessed' WHERE id = ?1",
                [voice.id()],
            )
            .expect("corrupt voice state");
        let error = store
            .transition_message(
                voice.source_id(),
                MessageTriageState::Unprocessed,
                MessageTriageState::Actionable,
                updated_at,
            )
            .expect_err("provider/state mismatch");
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid {
                resource: "message"
            }
        ));
        assert!(!error.to_string().contains("triage-voice-source"));
    }

    #[test]
    fn message_triage_list_orders_paginates_and_validates_inputs_without_mutation() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let updated_at =
            OffsetDateTime::parse("2025-01-02T03:04:06Z", &Rfc3339).expect("updated time");
        let mut ids = Vec::new();
        for suffix in ["first", "second", "third"] {
            let message = store
                .record_message(
                    format!("triage-list-{suffix}-key"),
                    format!("triage-list-{suffix}-source"),
                    MessageProvider::Gmail,
                    format!("triage-list-{suffix}-provider-message"),
                    MessageSummary::new("email summary").expect("summary"),
                    None,
                    None,
                    received_at,
                )
                .expect("message");
            let actionable = store
                .transition_message(
                    message.source_id(),
                    MessageTriageState::Unprocessed,
                    MessageTriageState::Actionable,
                    updated_at,
                )
                .expect("actionable");
            ids.push(actionable.id());
        }
        assert_eq!(
            store
                .list_messages_by_triage_state(MessageTriageState::Actionable, None, 2)
                .expect("first page")
                .iter()
                .map(StoredMessage::id)
                .collect::<Vec<_>>(),
            ids[..2]
        );
        assert_eq!(
            store
                .list_messages_by_triage_state(MessageTriageState::Actionable, Some(ids[1]), 2,)
                .expect("second page")
                .iter()
                .map(StoredMessage::id)
                .collect::<Vec<_>>(),
            ids[2..]
        );
        let before = message_snapshot(&store);
        for (cursor, limit) in [(Some(0), 1), (Some(-1), 1), (None, 0), (None, 101)] {
            assert!(matches!(
                store.list_messages_by_triage_state(MessageTriageState::Actionable, cursor, limit),
                Err(StoreError::InvalidInput { .. })
            ));
            assert_eq!(message_snapshot(&store), before);
        }
    }

    #[test]
    fn message_triage_compare_and_set_has_one_file_store_winner() {
        let database = TempDatabase::new();
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("open seed store");
        let message = seed
            .record_message(
                "triage-race-key",
                "triage-race-source",
                MessageProvider::Outlook,
                "triage-race-provider-message",
                MessageSummary::new("email summary").expect("summary"),
                None,
                None,
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time"),
            )
            .expect("message");
        drop(seed);
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let source_id = message.source_id().to_owned();
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.transition_message(
                &source_id,
                MessageTriageState::Unprocessed,
                MessageTriageState::Actionable,
                OffsetDateTime::parse("2025-01-02T03:04:06Z", &Rfc3339).expect("first time"),
            )
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.transition_message(
                "triage-race-source",
                MessageTriageState::Unprocessed,
                MessageTriageState::Actionable,
                OffsetDateTime::parse("2025-01-02T03:04:07Z", &Rfc3339).expect("second time"),
            )
        });
        let results = [
            first_handle.join().expect("first transition thread"),
            second_handle.join().expect("second transition thread"),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(StoreError::Conflict { .. })))
                .count(),
            1
        );
    }

    #[test]
    fn message_triage_transition_and_list_redact_corrupt_rows() {
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let updated_at =
            OffsetDateTime::parse("2025-01-02T03:04:06Z", &Rfc3339).expect("updated time");
        for (suffix, column, value) in [
            ("provider", "provider", "secret-corrupt-provider"),
            ("state", "triage_state", "secret-corrupt-state"),
            ("content", "summary", "   "),
            ("time", "received_at", "secret-corrupt-time"),
        ] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let message = store
                .record_message(
                    format!("triage-corrupt-{suffix}-key"),
                    format!("triage-corrupt-{suffix}-source"),
                    MessageProvider::Gmail,
                    format!("triage-corrupt-{suffix}-provider-message"),
                    MessageSummary::new("email summary").expect("summary"),
                    None,
                    None,
                    received_at,
                )
                .expect("message");
            store
                .connection()
                .execute_batch("PRAGMA ignore_check_constraints = ON;")
                .expect("allow corruption fixture");
            store
                .connection()
                .execute(
                    &format!("UPDATE messages SET {column} = ?1 WHERE id = ?2"),
                    rusqlite::params![value, message.id()],
                )
                .expect("corrupt stored row");
            let before = message_snapshot(&store);
            for error in [
                store
                    .transition_message(
                        message.source_id(),
                        MessageTriageState::Unprocessed,
                        MessageTriageState::Actionable,
                        updated_at,
                    )
                    .expect_err("transition must reject corruption"),
                store
                    .list_messages_by_triage_state(MessageTriageState::Unprocessed, None, 10)
                    .expect_err("list must reject corruption"),
            ] {
                assert!(matches!(
                    error,
                    StoreError::StoredRecordInvalid {
                        resource: "message"
                    }
                ));
                assert!(!error.to_string().contains(value));
                assert!(!format!("{error:?}").contains(value));
                assert_eq!(message_snapshot(&store), before);
            }
        }
    }

    #[test]
    fn message_triage_allows_each_unprocessed_email_outcome() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let received_at =
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("received time");
        let updated_at =
            OffsetDateTime::parse("2025-01-02T03:04:06Z", &Rfc3339).expect("updated time");
        for (suffix, next_state) in [
            ("actionable", MessageTriageState::Actionable),
            ("ambiguous", MessageTriageState::Ambiguous),
            ("ignored", MessageTriageState::Ignored),
        ] {
            let message = store
                .record_message(
                    format!("triage-outcome-{suffix}-key"),
                    format!("triage-outcome-{suffix}-source"),
                    MessageProvider::Outlook,
                    format!("triage-outcome-{suffix}-provider-message"),
                    MessageSummary::new("email summary").expect("summary"),
                    None,
                    None,
                    received_at,
                )
                .expect("message");
            assert_eq!(
                store
                    .transition_message(
                        message.source_id(),
                        MessageTriageState::Unprocessed,
                        next_state,
                        updated_at,
                    )
                    .expect("allowed transition")
                    .triage_state(),
                next_state
            );
        }
    }

    fn lifecycle_task(store: &PaStore, suffix: &str) -> StoredTask {
        let message = actionable_task_message(store, suffix);
        store
            .record_task(
                format!("task-lifecycle-{suffix}-key"),
                format!("task-lifecycle-{suffix}-source"),
                message.id(),
                TaskTitle::new("Review the invoice").expect("title"),
                TaskKind::Bill,
                None,
                None,
            )
            .expect("task")
    }

    fn task_transition_time(second: u8) -> OffsetDateTime {
        OffsetDateTime::parse(&format!("2025-01-02T03:04:{second:02}Z"), &Rfc3339)
            .expect("updated time")
    }

    fn assert_redacted_task_error(error: StoreError, secret: &str) {
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid { resource: "task" }
                | StoreError::Conflict {
                    resource: "task transition"
                }
        ));
        assert!(!error.to_string().contains(secret));
        assert!(!format!("{error:?}").contains(secret));
    }

    #[test]
    fn task_lifecycle_allows_both_no_slot_edges_and_rejects_forbidden_terminal_edges() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let pending_no_slot = lifecycle_task(&store, "pending-no-slot");
        let proposed_no_slot = lifecycle_task(&store, "proposed-no-slot");
        let terminal_scheduled = lifecycle_task(&store, "terminal-scheduled");

        assert_eq!(
            store
                .transition_task(
                    pending_no_slot.source_id(),
                    StoredTaskState::Pending,
                    StoredTaskState::NoSlot,
                    task_transition_time(6),
                )
                .expect("pending may become no-slot")
                .state(),
            StoredTaskState::NoSlot
        );
        let proposed = store
            .transition_task(
                proposed_no_slot.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::Proposed,
                task_transition_time(7),
            )
            .expect("proposed transition");
        assert_eq!(
            store
                .transition_task(
                    proposed.source_id(),
                    StoredTaskState::Proposed,
                    StoredTaskState::NoSlot,
                    task_transition_time(8),
                )
                .expect("proposed may become no-slot")
                .state(),
            StoredTaskState::NoSlot
        );

        store
            .transition_task(
                terminal_scheduled.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::Proposed,
                task_transition_time(9),
            )
            .expect("proposed transition");
        let scheduled = store
            .transition_task(
                terminal_scheduled.source_id(),
                StoredTaskState::Proposed,
                StoredTaskState::Scheduled,
                task_transition_time(10),
            )
            .expect("scheduled transition");
        assert_eq!(scheduled.state(), StoredTaskState::Scheduled);
        let before = task_snapshot(&store);
        for (source_id, expected, next) in [
            (
                pending_no_slot.source_id(),
                StoredTaskState::NoSlot,
                StoredTaskState::Proposed,
            ),
            (
                scheduled.source_id(),
                StoredTaskState::Scheduled,
                StoredTaskState::NoSlot,
            ),
            (
                proposed_no_slot.source_id(),
                StoredTaskState::NoSlot,
                StoredTaskState::Scheduled,
            ),
            (
                terminal_scheduled.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::Scheduled,
            ),
        ] {
            assert!(matches!(
                store.transition_task(source_id, expected, next, task_transition_time(11)),
                Err(StoreError::Conflict {
                    resource: "task transition"
                })
            ));
            assert_eq!(task_snapshot(&store), before);
        }
    }

    #[test]
    fn task_lifecycle_retries_validate_timestamps_and_inputs_without_mutation() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let task = lifecycle_task(&store, "retry");
        let updated_at = task_transition_time(6);
        let proposed = store
            .transition_task(
                task.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::Proposed,
                updated_at,
            )
            .expect("initial transition");
        assert_eq!(proposed.updated_at(), "2025-01-02T03:04:06Z");
        assert_eq!(
            store
                .transition_task(
                    task.source_id(),
                    StoredTaskState::Pending,
                    StoredTaskState::Proposed,
                    updated_at,
                )
                .expect("exact retry"),
            proposed
        );
        let before = task_snapshot(&store);
        for result in [
            store.transition_task(
                task.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::Proposed,
                task_transition_time(7),
            ),
            store.transition_task(
                task.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::Scheduled,
                task_transition_time(8),
            ),
            store.transition_task(
                "invalid task source",
                StoredTaskState::Proposed,
                StoredTaskState::Scheduled,
                task_transition_time(8),
            ),
            store.transition_task(
                task.source_id(),
                StoredTaskState::Proposed,
                StoredTaskState::Scheduled,
                task_transition_time(8)
                    .to_offset(time::UtcOffset::from_hms(1, 0, 0).expect("offset")),
            ),
            store.transition_task(
                task.source_id(),
                StoredTaskState::Proposed,
                StoredTaskState::Scheduled,
                task_transition_time(8) + TimeDuration::nanoseconds(1),
            ),
        ] {
            assert!(result.is_err());
            assert_eq!(task_snapshot(&store), before);
        }
    }

    #[test]
    fn task_state_listing_orders_filters_and_validates_page_boundaries() {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
        let first = lifecycle_task(&store, "list-first");
        let second = lifecycle_task(&store, "list-second");
        let third = lifecycle_task(&store, "list-third");
        store
            .transition_task(
                first.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::Proposed,
                task_transition_time(6),
            )
            .expect("first proposed");
        store
            .transition_task(
                second.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::NoSlot,
                task_transition_time(7),
            )
            .expect("second no slot");
        store
            .transition_task(
                third.source_id(),
                StoredTaskState::Pending,
                StoredTaskState::Proposed,
                task_transition_time(8),
            )
            .expect("third proposed");
        assert_eq!(
            store
                .list_tasks_by_state(StoredTaskState::Proposed, None, 2)
                .expect("proposed page")
                .iter()
                .map(StoredTask::id)
                .collect::<Vec<_>>(),
            vec![first.id(), third.id()]
        );
        assert_eq!(
            store
                .list_tasks_by_state(StoredTaskState::Proposed, Some(first.id()), 1)
                .expect("cursor page")
                .iter()
                .map(StoredTask::id)
                .collect::<Vec<_>>(),
            vec![third.id()]
        );
        assert_eq!(
            store
                .list_tasks_by_state(StoredTaskState::NoSlot, None, 1)
                .expect("filtered page")
                .iter()
                .map(StoredTask::id)
                .collect::<Vec<_>>(),
            vec![second.id()]
        );
        let before = task_snapshot(&store);
        for (cursor, limit) in [(Some(0), 1), (Some(-1), 1), (None, 0), (None, 101)] {
            assert!(matches!(
                store.list_tasks_by_state(StoredTaskState::Pending, cursor, limit),
                Err(StoreError::InvalidInput { .. })
            ));
            assert_eq!(task_snapshot(&store), before);
        }
    }

    #[test]
    fn task_transition_and_listing_fail_closed_for_task_and_message_corruption() {
        for (suffix, table, column, secret) in [
            ("task-state", "tasks", "status", "secret-task-state"),
            ("task-time", "tasks", "updated_at", "secret-task-time"),
            ("task-title", "tasks", "title", "   "),
            (
                "message-provider",
                "messages",
                "provider",
                "secret-message-provider",
            ),
            ("message-state", "messages", "triage_state", "ignored"),
            ("message-summary", "messages", "summary", "   "),
            (
                "message-time",
                "messages",
                "received_at",
                "secret-message-time",
            ),
        ] {
            let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
            let task = lifecycle_task(&store, suffix);
            store
                .connection()
                .execute_batch("PRAGMA ignore_check_constraints = ON;")
                .expect("allow corruption fixture");
            let target_id = if table == "tasks" {
                task.id()
            } else {
                task.message_id()
            };
            store
                .connection()
                .execute(
                    &format!("UPDATE {table} SET {column} = ?1 WHERE id = ?2"),
                    rusqlite::params![secret, target_id],
                )
                .expect("corrupt stored row");
            let task_before = task_snapshot(&store);
            let message_before = message_snapshot(&store);
            for error in [
                store
                    .transition_task(
                        task.source_id(),
                        StoredTaskState::Pending,
                        StoredTaskState::Proposed,
                        task_transition_time(6),
                    )
                    .expect_err("transition rejects corruption"),
                store
                    .list_tasks_by_state(StoredTaskState::Pending, None, 1)
                    .expect_err("listing rejects corruption"),
            ] {
                assert_redacted_task_error(error, secret);
                assert_eq!(task_snapshot(&store), task_before);
                assert_eq!(message_snapshot(&store), message_before);
            }
        }
    }

    #[test]
    fn task_transition_compare_and_set_has_one_file_store_winner() {
        let database = TempDatabase::new();
        let seed = PaStore::open(&database.path, DATABASE_KEY).expect("open seed store");
        let task = lifecycle_task(&seed, "race");
        drop(seed);
        let first = PaStore::open(&database.path, DATABASE_KEY).expect("open first store");
        let second = PaStore::open(&database.path, DATABASE_KEY).expect("open second store");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let source_id = task.source_id().to_owned();
        let first_handle = std::thread::spawn(move || {
            first_barrier.wait();
            first.transition_task(
                &source_id,
                StoredTaskState::Pending,
                StoredTaskState::Proposed,
                task_transition_time(6),
            )
        });
        let second_handle = std::thread::spawn(move || {
            second_barrier.wait();
            second.transition_task(
                "task-lifecycle-race-source",
                StoredTaskState::Pending,
                StoredTaskState::NoSlot,
                task_transition_time(7),
            )
        });
        let results = [
            first_handle.join().expect("first transition thread"),
            second_handle.join().expect("second transition thread"),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(StoreError::Conflict {
                        resource: "task transition"
                    })
                ))
                .count(),
            1
        );
        let store = PaStore::open(&database.path, DATABASE_KEY).expect("reopen store");
        let final_task = store.load_task_by_id(task.id()).expect("final task");
        assert!(matches!(
            final_task.state(),
            StoredTaskState::Proposed | StoredTaskState::NoSlot
        ));
    }

    #[test]
    fn appointment_quote_upgrade_preserves_v7_rows_and_enforces_quote_contract() {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..7]).expect("apply v7 schema");
        connection
            .execute(
                "INSERT INTO appointment_drafts (\
                    idempotency_key, source_id, quote_id, caller_name, caller_email, kind,\
                    starts_at, ends_at, requester_included\
                 ) VALUES (\
                    'legacy-draft-key', 'legacy-draft-source', 'legacy-draft-quote',\
                    'Legacy Caller', 'legacy@example.com', 'callback',\
                    '2025-01-02T03:04:05Z', '2025-01-02T03:19:05Z', 0\
                 )",
                [],
            )
            .expect("seed v7 appointment draft");

        run_migrations_with(&mut connection, MIGRATIONS).expect("upgrade to current schema");
        run_migrations_with(&mut connection, MIGRATIONS).expect("idempotent current reopen");

        assert_eq!(
            connection
                .query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("schema version"),
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM appointment_drafts", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("preserved appointment draft count"),
            1
        );
        for table in ["appointment_quotes", "appointment_quote_slots"] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                        [table],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("table presence"),
                1,
                "{table} exists"
            );
        }

        let store = PaStore { connection };
        assert_named_index(
            &store,
            "idx_appointment_drafts_id_quote_id",
            "appointment_drafts",
            &["id", "quote_id"],
        );
        assert_named_index(
            &store,
            "idx_appointment_quotes_state_expires_at",
            "appointment_quotes",
            &["state", "expires_at"],
        );
        assert_named_index(
            &store,
            "idx_appointment_quotes_proposal_id",
            "appointment_quotes",
            &["proposal_id"],
        );

        let insert_quote = |quote_id: &str,
                            state: &str,
                            draft_id: Option<i64>,
                            slot_index: Option<i64>,
                            consumed_at: Option<&str>,
                            proposal_id: Option<i64>| {
            store.connection().execute(
                "INSERT INTO appointment_quotes (\
                    quote_id, appointment_kind, timezone, issued_at, expires_at, state,\
                    appointment_draft_id, selected_slot_index, consumed_at, proposal_id\
                 ) VALUES (?1, 'callback', 'Australia/Sydney',\
                    '2025-01-02T03:04:05Z', '2025-01-02T03:09:05Z', ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    quote_id,
                    state,
                    draft_id,
                    slot_index,
                    consumed_at,
                    proposal_id
                ],
            )
        };
        insert_quote("legacy-draft-quote", "issued", None, None, None, None)
            .expect("issued quote shape");
        assert!(insert_quote("bad-kind", "issued", None, None, None, None).is_ok());
        assert!(store
            .connection()
            .execute(
                "UPDATE appointment_quotes SET appointment_kind = 'unknown' WHERE quote_id = 'bad-kind'",
                [],
            )
            .is_err());
        assert!(insert_quote("bad-state-shape", "issued", Some(1), None, None, None).is_err());
        assert!(
            insert_quote(
                "bad-slot-reference",
                "prepared",
                Some(1),
                Some(0),
                None,
                None
            )
            .is_err()
        );
        assert!(
            store
                .connection()
                .execute(
                    "INSERT INTO appointment_quotes (\
                    quote_id, appointment_kind, timezone, issued_at, expires_at\
                 ) VALUES (NULL, 'callback', 'Australia/Sydney',\
                    '2025-01-02T03:04:05Z', '2025-01-02T03:09:05Z')",
                    [],
                )
                .is_err()
        );
        assert!(store
            .connection()
            .execute(
                "INSERT INTO appointment_quote_slots(quote_id, slot_index, starts_at, ends_at) \
                 VALUES ('legacy-draft-quote', 100, '2025-01-02T03:04:05Z', '2025-01-02T03:19:05Z')",
                [],
            )
            .is_err());
        assert!(store
            .connection()
            .execute(
                "INSERT INTO appointment_quote_slots(quote_id, slot_index, starts_at, ends_at) \
                 VALUES ('legacy-draft-quote', 0.5, '2025-01-02T03:04:05Z', '2025-01-02T03:19:05Z')",
                [],
            )
            .is_err());
        store
            .connection()
            .execute(
                "INSERT INTO appointment_quote_slots(quote_id, slot_index, starts_at, ends_at) \
                 VALUES ('legacy-draft-quote', 0, '2025-01-02T03:04:05Z', '2025-01-02T03:19:05Z')",
                [],
            )
            .expect("valid frozen slot");
        store
            .connection()
            .execute(
                "UPDATE appointment_quotes
                 SET state = 'prepared', appointment_draft_id = 1, selected_slot_index = 0
                 WHERE quote_id = 'legacy-draft-quote'",
                [],
            )
            .expect("prepared quote references matching draft and slot");
        assert!(
            store
                .connection()
                .execute(
                    "DELETE FROM appointment_quotes WHERE quote_id = 'legacy-draft-quote'",
                    []
                )
                .is_err()
        );
    }

    #[test]
    fn appointment_quote_v9_upgrade_backfills_v8_slot_count_and_enforces_bounds() {
        let database = TempDatabase::new();
        {
            let mut connection = Connection::open(&database.path).expect("open v8 fixture");
            apply_sqlcipher_key(&connection, DATABASE_KEY).expect("apply SQLCipher key");
            verify_sqlcipher(&connection).expect("verify SQLCipher");
            connection
                .pragma_update(None, "foreign_keys", true)
                .expect("enable foreign keys");
            run_migrations_with(&mut connection, &MIGRATIONS[..8]).expect("apply v8 schema");
            connection
                .execute(
                    "INSERT INTO appointment_quotes (\
                        quote_id, appointment_kind, timezone, issued_at, expires_at\
                     ) VALUES (\
                        '11111111-1111-1111-1111-111111111111', 'callback', 'Australia/Sydney',\
                        '2025-01-02T03:04:05Z', '2025-01-02T03:09:05Z'\
                     )",
                    [],
                )
                .expect("seed valid v8 quote");
            for (slot_index, starts_at, ends_at) in [
                (0, "2025-01-02T04:04:05Z", "2025-01-02T04:19:05Z"),
                (1, "2025-01-02T06:04:05Z", "2025-01-02T06:19:05Z"),
            ] {
                connection
                    .execute(
                        "INSERT INTO appointment_quote_slots (quote_id, slot_index, starts_at, ends_at) \
                         VALUES ('11111111-1111-1111-1111-111111111111', ?1, ?2, ?3)",
                        rusqlite::params![slot_index, starts_at, ends_at],
                    )
                    .expect("seed ordered v8 slot");
            }
        }

        let quote = Quote::with_id(
            QuoteId::from_uuid(
                uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").expect("quote id"),
            ),
            OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("issued at"),
        );
        let expected_slots = [
            AppointmentSlot::new(
                OffsetDateTime::parse("2025-01-02T04:04:05Z", &Rfc3339).expect("first start"),
                OffsetDateTime::parse("2025-01-02T04:19:05Z", &Rfc3339).expect("first end"),
            )
            .expect("first slot"),
            AppointmentSlot::new(
                OffsetDateTime::parse("2025-01-02T06:04:05Z", &Rfc3339).expect("second start"),
                OffsetDateTime::parse("2025-01-02T06:19:05Z", &Rfc3339).expect("second end"),
            )
            .expect("second slot"),
        ];
        let assert_migrated_store = |store: &PaStore, version_context: &str| {
            assert_eq!(
                store
                    .connection()
                    .query_row(
                        "SELECT max(version), count(*) FROM schema_migrations",
                        [],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .expect("schema version and migration count"),
                (CURRENT_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION),
                "schema version/count after {version_context}"
            );
            assert_eq!(
                store
                    .connection()
                    .query_row(
                        "SELECT slot_count FROM appointment_quotes WHERE quote_id = '11111111-1111-1111-1111-111111111111'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("backfilled slot count"),
                2
            );
            let stored = store
                .load_appointment_quote_by_id(quote.id())
                .expect("strictly load migrated quote");
            assert_eq!(stored.quote(), &quote);
            assert_eq!(stored.appointment_kind(), AppointmentKind::Callback);
            assert_eq!(stored.timezone(), "Australia/Sydney");
            assert_eq!(stored.offered_slots(), expected_slots);
            assert_eq!(stored.state(), StoredAppointmentQuoteState::Issued);
        };

        {
            let store = PaStore::open(&database.path, DATABASE_KEY)
                .expect("open v8 fixture through PaStore and migrate to v9");
            assert_eq!(
                store
                    .connection()
                    .query_row(
                        "SELECT count(*) FROM pragma_table_info('appointment_quotes') WHERE name = 'slot_count'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("slot count column presence"),
                1
            );
            for (label, sql) in [
                (
                    "negative",
                    "UPDATE appointment_quotes SET slot_count = -1 \
                     WHERE quote_id = '11111111-1111-1111-1111-111111111111'",
                ),
                (
                    "fractional",
                    "UPDATE appointment_quotes SET slot_count = 1.5 \
                     WHERE quote_id = '11111111-1111-1111-1111-111111111111'",
                ),
                (
                    "over maximum",
                    "UPDATE appointment_quotes SET slot_count = 101 \
                     WHERE quote_id = '11111111-1111-1111-1111-111111111111'",
                ),
            ] {
                assert!(
                    store.connection().execute(sql, []).is_err(),
                    "{label} slot count must be rejected"
                );
            }
            assert_migrated_store(&store, "first PaStore open");
        }

        {
            let store = PaStore::open(&database.path, DATABASE_KEY)
                .expect("reopen migrated file store through PaStore");
            assert_migrated_store(&store, "second PaStore open");
        }
    }

    fn prepared_quote_store_for_foreign_key_test() -> PaStore {
        let mut connection = keyed_connection_for_migration_test();
        run_migrations_with(&mut connection, &MIGRATIONS[..7]).expect("apply v7 schema");
        connection
            .execute(
                "INSERT INTO appointment_drafts (\
                    idempotency_key, source_id, quote_id, caller_name, caller_email, kind,\
                    starts_at, ends_at, requester_included\
                 ) VALUES (\
                    'prepared-draft-key', 'prepared-draft-source', 'prepared-quote',\
                    'Prepared Caller', 'prepared@example.com', 'callback',\
                    '2025-01-02T03:04:05Z', '2025-01-02T03:19:05Z', 0\
                 )",
                [],
            )
            .expect("seed v7 appointment draft");
        run_migrations_with(&mut connection, MIGRATIONS).expect("upgrade to current schema");

        let store = PaStore { connection };
        store
            .connection()
            .execute(
                "INSERT INTO appointment_quotes (\
                    quote_id, appointment_kind, timezone, issued_at, expires_at\
                 ) VALUES (\
                    'prepared-quote', 'callback', 'Australia/Sydney',\
                    '2025-01-02T03:04:05Z', '2025-01-02T03:09:05Z'\
                 )",
                [],
            )
            .expect("issued quote");
        store
            .connection()
            .execute(
                "INSERT INTO appointment_quote_slots(quote_id, slot_index, starts_at, ends_at) \
                 VALUES ('prepared-quote', 0, '2025-01-02T03:04:05Z', '2025-01-02T03:19:05Z')",
                [],
            )
            .expect("frozen quote slot");
        store
            .connection()
            .execute(
                "UPDATE appointment_quotes
                 SET state = 'prepared', appointment_draft_id = 1, selected_slot_index = 0
                 WHERE quote_id = 'prepared-quote'",
                [],
            )
            .expect("prepared quote");
        store
    }

    #[test]
    fn appointment_quote_restricts_deleting_the_prepared_appointment_draft() {
        let store = prepared_quote_store_for_foreign_key_test();

        assert!(
            store
                .connection()
                .execute("DELETE FROM appointment_drafts WHERE id = 1", [])
                .is_err()
        );
    }

    #[test]
    fn consumed_appointment_quote_restricts_deleting_its_proposal() {
        let store = prepared_quote_store_for_foreign_key_test();
        store
            .connection()
            .execute(
                "INSERT INTO proposals(idempotency_key, source_id, appointment_draft_id) \
                 VALUES ('consumed-proposal-key', 'consumed-proposal-source', 1)",
                [],
            )
            .expect("proposal for consumed quote");
        store
            .connection()
            .execute(
                "UPDATE appointment_quotes
                 SET state = 'consumed', consumed_at = '2025-01-02T03:10:00Z', proposal_id = 1
                 WHERE quote_id = 'prepared-quote'",
                [],
            )
            .expect("consumed quote references proposal");

        assert!(
            store
                .connection()
                .execute("DELETE FROM proposals WHERE id = 1", [])
                .is_err()
        );
    }

    #[test]
    fn stored_appointment_quote_exposes_values_without_debugging_sensitive_content() {
        let issued_at = OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339).expect("issued at");
        let quote = Quote::with_id(
            QuoteId::from_uuid(
                uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").expect("quote id"),
            ),
            issued_at,
        );
        let slot =
            AppointmentSlot::new(issued_at, issued_at + TimeDuration::minutes(15)).expect("slot");
        let caller = CallerIdentity::new(
            "Private Caller",
            ConfirmedEmail::confirm("private@example.com").expect("email"),
        )
        .expect("caller");
        let draft = AppointmentDraft::new(
            AppointmentKind::Callback,
            caller,
            slot.starts_at(),
            quote.id(),
            IdempotencyKey::new("private-draft-key").expect("key"),
        )
        .expect("draft");
        let consumed_at =
            OffsetDateTime::parse("2025-01-02T03:07:08Z", &Rfc3339).expect("consumed at");
        let stored = StoredAppointmentQuote {
            quote: quote.clone(),
            appointment_kind: AppointmentKind::Callback,
            timezone: "Australia/Sydney".to_owned(),
            offered_slots: vec![slot],
            state: StoredAppointmentQuoteState::Consumed,
            selected_slot_index: Some(0),
            appointment_draft: Some(StoredAppointmentDraft {
                id: 42,
                source_id: "private-draft-source".to_owned(),
                draft,
            }),
            consumed_at: Some(consumed_at),
            proposal_id: Some(84),
        };

        assert_eq!(stored.quote(), &quote);
        assert_eq!(stored.quote_id(), quote.id());
        assert_eq!(stored.appointment_kind(), AppointmentKind::Callback);
        assert_eq!(stored.timezone(), "Australia/Sydney");
        assert_eq!(stored.offered_slots(), &[slot]);
        assert_eq!(stored.state(), StoredAppointmentQuoteState::Consumed);
        assert_eq!(stored.selected_slot_index(), Some(0));
        assert_eq!(stored.appointment_draft().expect("stored draft").id(), 42);
        assert_eq!(stored.appointment_draft_id(), Some(42));
        assert_eq!(stored.draft().expect("draft").quote_id(), quote.id());
        assert_eq!(stored.consumed_at(), Some(consumed_at));
        assert_eq!(stored.proposal_id(), Some(84));

        let debug = format!("{stored:?}");
        for secret in [
            "11111111-1111-1111-1111-111111111111",
            "2025-01-02T03:04:05Z",
            "2025-01-02T03:19:05Z",
            "2025-01-02T03:07:08Z",
            "Australia/Sydney",
            "Private Caller",
            "private@example.com",
            "private-draft-source",
        ] {
            assert!(!debug.contains(secret), "debug redacts sensitive value");
        }
        assert!(debug.contains("Consumed"));
        assert!(debug.contains("1"));
        assert!(debug.contains("42"));
    }

    #[test]
    fn http_idempotency_constants_and_validators() {
        assert_eq!(super::HTTP_IDEMPOTENCY_SCOPE, "pa-http-v1");
        assert_eq!(super::MAX_HTTP_IDEMPOTENCY_SCOPE_LENGTH, 64);
        assert_eq!(super::MAX_HTTP_IDEMPOTENCY_KEY_LENGTH, 128);
        assert_eq!(super::MAX_HTTP_IDEMPOTENCY_FINGERPRINT_LENGTH, 64);
        assert_eq!(super::HTTP_IDEMPOTENCY_RESERVATION_SECONDS, 300);
        assert_eq!(super::MAX_HTTP_IDEMPOTENCY_RESPONSE_BYTES, 64 * 1024);

        assert!(super::validate_http_idempotency_scope("a").is_ok());
        let scope_boundary = format!("A0._:-{}", "s".repeat(58));
        assert!(super::validate_http_idempotency_scope(&scope_boundary).is_ok());

        assert!(super::validate_http_idempotency_key("a").is_ok());
        let key_boundary = format!("A0._~-{}", "k".repeat(122));
        assert!(super::validate_http_idempotency_key(&key_boundary).is_ok());

        assert!(super::validate_http_idempotency_fingerprint(VALID_HTTP_FINGERPRINT).is_ok());
    }

    #[test]
    fn http_idempotency_response_new_accepts_valid_json() {
        let body = br#"{"ok":true,"items":[1,2]}"#.to_vec();
        let response =
            super::HttpIdempotencyResponse::new(201, body.clone()).expect("valid response");

        assert_eq!(response.status(), 201);
        assert_eq!(response.content_type(), "application/json");
        assert_eq!(response.body(), body.as_slice());
        assert_eq!(response, response.clone());
    }

    #[test]
    fn http_idempotency_response_boundary_matrix() {
        let shortest = b"0".to_vec();
        for status in [200, 599] {
            let response = super::HttpIdempotencyResponse::new(status, shortest.clone())
                .expect("status boundary is accepted");
            assert_eq!(response.status(), status);
            assert_eq!(response.body(), shortest.as_slice());
        }
        for status in [199, 600] {
            assert!(super::HttpIdempotencyResponse::new(status, shortest.clone()).is_err());
        }

        assert!(super::HttpIdempotencyResponse::new(200, Vec::new()).is_err());

        let mut exact_limit = Vec::with_capacity(super::MAX_HTTP_IDEMPOTENCY_RESPONSE_BYTES);
        exact_limit.push(b'"');
        exact_limit.extend(std::iter::repeat_n(
            b'a',
            super::MAX_HTTP_IDEMPOTENCY_RESPONSE_BYTES - 2,
        ));
        exact_limit.push(b'"');
        assert_eq!(
            exact_limit.len(),
            super::MAX_HTTP_IDEMPOTENCY_RESPONSE_BYTES
        );
        let response = super::HttpIdempotencyResponse::new(200, exact_limit.clone())
            .expect("exact byte limit is accepted");
        assert_eq!(response.body(), exact_limit.as_slice());

        let mut over_limit = exact_limit;
        over_limit.insert(1, b'a');
        assert_eq!(
            over_limit.len(),
            super::MAX_HTTP_IDEMPOTENCY_RESPONSE_BYTES + 1
        );
        assert!(super::HttpIdempotencyResponse::new(200, over_limit).is_err());
    }

    #[test]
    fn http_idempotency_response_preserves_exact_bytes() {
        let body = b" {\"z\":1e+2,\"message\":\"caf\xc3\xa9\",\"a\":[true,null]} \n".to_vec();
        let response =
            super::HttpIdempotencyResponse::new(202, body.clone()).expect("valid response");

        assert_eq!(response.status(), 202);
        assert_eq!(response.content_type(), "application/json");
        assert_eq!(response.body(), body.as_slice());
    }

    #[test]
    fn http_idempotency_response_rejects_invalid_utf8_and_json() {
        let invalid_bodies = [
            vec![0x7b, 0xff, 0x7d],
            br#"{"ok":}"#.to_vec(),
            b"{".to_vec(),
            b"0x".to_vec(),
            b" ".to_vec(),
        ];

        for body in invalid_bodies {
            assert!(super::HttpIdempotencyResponse::new(200, body).is_err());
        }
    }

    #[test]
    fn http_idempotency_response_debug_is_redacted() {
        let response = super::HttpIdempotencyResponse::new(
            418,
            br#"{"debug":"debug-body-sentinel-7d1b"} "#.to_vec(),
        )
        .expect("valid response");
        let debug = format!("{response:?}");

        assert!(debug.contains("HttpIdempotencyResponse"));
        assert!(debug.contains("418"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("debug-body-sentinel-7d1b"));
    }

    #[test]
    fn http_idempotency_response_errors_are_redacted() {
        let error = super::HttpIdempotencyResponse::new(
            199,
            br#"{"secret":"error-body-sentinel-7d1b"}"#.to_vec(),
        )
        .expect_err("invalid status");
        let display = error.to_string();
        let debug = format!("{error:?}");

        assert!(!display.contains("error-body-sentinel-7d1b"));
        assert!(!debug.contains("error-body-sentinel-7d1b"));
    }

    #[test]
    fn http_idempotency_validator_boundary_matrix() {
        let invalid_scope_values = [
            String::new(),
            "s".repeat(super::MAX_HTTP_IDEMPOTENCY_SCOPE_LENGTH + 1),
            "scope with space".to_owned(),
            "scope/slash".to_owned(),
            "scope\\backslash".to_owned(),
            "scope\0nul".to_owned(),
            "scope+other".to_owned(),
            "scope-π".to_owned(),
        ];
        for value in invalid_scope_values {
            let original = value.clone();
            assert!(super::validate_http_idempotency_scope(&value).is_err());
            assert!(value.as_bytes() == original.as_bytes());
        }

        let invalid_key_values = [
            String::new(),
            "k".repeat(super::MAX_HTTP_IDEMPOTENCY_KEY_LENGTH + 1),
            "key with space".to_owned(),
            "key/slash".to_owned(),
            "key\\backslash".to_owned(),
            "key\0nul".to_owned(),
            "key+other".to_owned(),
            "key:delimiter".to_owned(),
            "key-π".to_owned(),
        ];
        for value in invalid_key_values {
            let original = value.clone();
            assert!(super::validate_http_idempotency_key(&value).is_err());
            assert!(value.as_bytes() == original.as_bytes());
        }

        let invalid_fingerprint_values = [
            String::new(),
            "0".repeat(super::MAX_HTTP_IDEMPOTENCY_FINGERPRINT_LENGTH - 1),
            "0".repeat(super::MAX_HTTP_IDEMPOTENCY_FINGERPRINT_LENGTH + 1),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeF".to_owned(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg".to_owned(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde π".to_owned(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde ".to_owned(),
        ];
        for value in invalid_fingerprint_values {
            let original = value.clone();
            assert!(super::validate_http_idempotency_fingerprint(&value).is_err());
            assert!(value.as_bytes() == original.as_bytes());
        }
    }

    #[test]
    fn http_idempotency_validator_errors_are_redacted() {
        let scope_sentinel = "scope-secret-sentinel/with whitespace";
        let key_sentinel = "key-secret-sentinel/with whitespace";
        let fingerprint_sentinel =
            "fingerprint-secret-sentinel/with whitespace and more than 64 bytes";
        let errors = [
            super::validate_http_idempotency_scope(scope_sentinel).expect_err("invalid scope"),
            super::validate_http_idempotency_key(key_sentinel).expect_err("invalid key"),
            super::validate_http_idempotency_fingerprint(fingerprint_sentinel)
                .expect_err("invalid fingerprint"),
        ];

        for (error, sentinel) in [
            (&errors[0], scope_sentinel),
            (&errors[1], key_sentinel),
            (&errors[2], fingerprint_sentinel),
        ] {
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert!(!display.contains(sentinel));
            assert!(!debug.contains(sentinel));
        }
    }
}
