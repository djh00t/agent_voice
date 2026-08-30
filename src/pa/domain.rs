//! Validated values used by the personal-assistant workflow.
//!
//! These types intentionally contain only scheduling and workflow metadata. In
//! particular, they do not carry email bodies or call transcripts.

use std::fmt;

use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// The result type used by constructors and state transitions in this module.
pub type DomainResult<T> = Result<T, DomainError>;

/// Validation and state-transition failures for PA domain values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// A required string contained no non-whitespace characters.
    BlankRequiredField { field: &'static str },
    /// An email address did not have the basic `local@domain.tld` shape.
    InvalidEmail,
    /// A caller email must be constructed through the confirmed-email value.
    UnconfirmedEmail,
    /// A duration supplied to a constructor was zero.
    ZeroDuration { field: &'static str },
    /// A quote expiry was not exactly five minutes after issuance.
    InvalidQuoteExpiry,
    /// An appointment slot had equal or reversed interval bounds.
    InvalidAppointmentSlot,
    /// A quote could no longer be consumed at the supplied instant.
    QuoteExpired,
    /// A quote was consumed before it was issued.
    QuoteNotYetValid,
    /// A proposal state cannot leave a terminal state.
    TerminalProposalState { state: ProposalState },
    /// The requested proposal transition is not allowed.
    InvalidProposalTransition {
        from: ProposalState,
        to: ProposalState,
    },
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankRequiredField { field } => {
                write!(formatter, "{field} must not be blank")
            }
            Self::InvalidEmail => formatter.write_str("email has an invalid shape"),
            Self::UnconfirmedEmail => formatter.write_str("email must be confirmed"),
            Self::ZeroDuration { field } => {
                write!(formatter, "{field} must be greater than zero")
            }
            Self::InvalidQuoteExpiry => {
                formatter.write_str("quote expiry must be exactly five minutes after issuance")
            }
            Self::InvalidAppointmentSlot => {
                formatter.write_str("appointment slot interval is invalid")
            }
            Self::QuoteExpired => formatter.write_str("quote has expired"),
            Self::QuoteNotYetValid => formatter.write_str("quote is not yet valid"),
            Self::TerminalProposalState { state } => {
                write!(formatter, "proposal state {state:?} is terminal")
            }
            Self::InvalidProposalTransition { from, to } => {
                write!(
                    formatter,
                    "proposal cannot transition from {from:?} to {to:?}"
                )
            }
        }
    }
}

impl std::error::Error for DomainError {}

/// The appointment options offered by the assistant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppointmentKind {
    /// A short owner callback; the requester is not included by default.
    #[default]
    Callback,
    /// A meeting; the requester is included by default.
    Meeting,
}

impl AppointmentKind {
    /// Returns the fixed default length of this appointment kind.
    pub const fn duration_minutes(self) -> u16 {
        match self {
            Self::Callback => 15,
            Self::Meeting => 30,
        }
    }

    /// Returns the fixed default length as a `time` duration.
    pub const fn duration(self) -> Duration {
        Duration::minutes(self.duration_minutes() as i64)
    }

    /// Alias for [`Self::duration`].
    pub const fn default_duration(self) -> Duration {
        self.duration()
    }

    /// Whether a requester should be included in the created event by default.
    pub const fn requester_included_by_default(self) -> bool {
        matches!(self, Self::Meeting)
    }

    /// Alias for [`Self::requester_included_by_default`].
    pub const fn includes_requester_by_default(self) -> bool {
        self.requester_included_by_default()
    }

    /// Whether this appointment is owner-only by default.
    pub const fn owner_only_by_default(self) -> bool {
        !self.requester_included_by_default()
    }
}

/// The task categories produced by email triage or direct owner requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// A bill or invoice task.
    Bill,
    /// A callback task.
    #[default]
    Callback,
    /// A reading task.
    Reading,
    /// A response-to-email task.
    EmailReply,
    /// A preparation task.
    Preparation,
}

impl TaskKind {
    /// Returns this task kind's default duration in minutes.
    pub const fn duration_minutes(self) -> u16 {
        match self {
            Self::Bill | Self::Callback => 15,
            Self::Reading | Self::EmailReply => 30,
            Self::Preparation => 60,
        }
    }

    /// Returns this task kind's default duration.
    pub const fn duration(self) -> Duration {
        Duration::minutes(self.duration_minutes() as i64)
    }

    /// Alias for [`Self::duration`].
    pub const fn default_duration(self) -> Duration {
        self.duration()
    }
}

/// A positive duration expressed in whole minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DurationMinutes(u32);

impl DurationMinutes {
    /// Constructs a positive minute duration.
    pub fn new(minutes: u32) -> DomainResult<Self> {
        if minutes == 0 {
            return Err(DomainError::ZeroDuration { field: "duration" });
        }
        Ok(Self(minutes))
    }

    /// Returns the number of minutes.
    pub const fn minutes(self) -> u32 {
        self.0
    }

    /// Alias for [`Self::minutes`].
    pub const fn as_minutes(self) -> u32 {
        self.minutes()
    }

    /// Converts the value to a `time` duration.
    pub const fn as_duration(self) -> Duration {
        Duration::minutes(self.0 as i64)
    }
}

impl<'de> Deserialize<'de> for DurationMinutes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(u32::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl TryFrom<u32> for DurationMinutes {
    type Error = DomainError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// A confirmed, shape-validated email address.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ConfirmedEmail {
    value: String,
    confirmed: bool,
}

impl fmt::Debug for ConfirmedEmail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfirmedEmail")
            .field("value", &"<redacted>")
            .field("confirmed", &self.confirmed)
            .finish()
    }
}

impl ConfirmedEmail {
    /// Records an email address after the caller has explicitly confirmed it.
    pub fn confirm(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        if !is_valid_email(&value) {
            return Err(DomainError::InvalidEmail);
        }
        Ok(Self {
            value,
            confirmed: true,
        })
    }

    /// Returns the validated email text.
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Caller identity emails are confirmed by construction.
    pub const fn is_confirmed(&self) -> bool {
        self.confirmed
    }
}

/// Generic serde inputs cannot attest that a caller confirmed an email address.
impl<'de> Deserialize<'de> for ConfirmedEmail {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(D::Error::custom(DomainError::UnconfirmedEmail))
    }
}

impl From<ConfirmedEmail> for String {
    fn from(value: ConfirmedEmail) -> Self {
        value.value
    }
}

/// Alias emphasizing that the email is a validated address.
pub type EmailAddress = ConfirmedEmail;

/// A caller identity containing only a name and confirmed email.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CallerIdentity {
    name: String,
    email: ConfirmedEmail,
}

impl CallerIdentity {
    /// Validates a non-empty name and requires a previously confirmed email.
    pub fn new(name: impl Into<String>, email: ConfirmedEmail) -> DomainResult<Self> {
        Ok(Self {
            name: non_blank(name.into(), "name")?,
            email,
        })
    }

    /// Returns the caller's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the caller's confirmed email.
    pub fn email(&self) -> &str {
        self.email.as_str()
    }

    /// Returns the typed confirmed email.
    pub fn confirmed_email(&self) -> &ConfirmedEmail {
        &self.email
    }

    /// Always true because unconfirmed emails cannot construct this value.
    pub const fn email_confirmed(&self) -> bool {
        true
    }
}

impl<'de> Deserialize<'de> for CallerIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            name: String,
            email: ConfirmedEmail,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.name, wire.email).map_err(D::Error::custom)
    }
}

/// An opaque identifier for a five-minute quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuoteId(Uuid);

impl QuoteId {
    /// Creates a new opaque random identifier.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Wraps an existing UUID without exposing its semantics as a quote ID.
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Returns the wrapped UUID for provider/storage adapters.
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    /// Returns the UUID by value.
    pub const fn into_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for QuoteId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for QuoteId {
    fn from(value: Uuid) -> Self {
        Self::from_uuid(value)
    }
}

impl fmt::Display for QuoteId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A quote consumable only during its half-open five-minute validity interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Quote {
    id: QuoteId,
    issued_at: OffsetDateTime,
    expires_at: OffsetDateTime,
}

impl Quote {
    /// The quote validity period.
    pub const VALID_FOR: Duration = Duration::minutes(5);

    /// Issues a quote at the supplied UTC instant.
    pub fn new(issued_at: OffsetDateTime) -> Self {
        Self {
            id: QuoteId::new(),
            issued_at,
            expires_at: issued_at + Self::VALID_FOR,
        }
    }

    /// Issues a quote with a caller-supplied opaque identifier.
    pub fn with_id(id: QuoteId, issued_at: OffsetDateTime) -> Self {
        Self {
            id,
            issued_at,
            expires_at: issued_at + Self::VALID_FOR,
        }
    }

    /// Returns the quote identifier.
    pub const fn id(&self) -> QuoteId {
        self.id
    }

    /// Alias for [`Self::id`].
    pub const fn quote_id(&self) -> QuoteId {
        self.id()
    }

    /// Returns when the quote was issued.
    pub const fn issued_at(&self) -> OffsetDateTime {
        self.issued_at
    }

    /// Returns the exclusive expiry instant.
    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    /// Returns whether this quote is expired at the supplied instant.
    pub fn is_expired(&self, at: OffsetDateTime) -> bool {
        at >= self.expires_at
    }

    /// Returns whether this quote may be consumed at the supplied instant.
    pub fn is_valid_at(&self, at: OffsetDateTime) -> bool {
        at >= self.issued_at && !self.is_expired(at)
    }

    /// Consumes the quote identifier only while it is valid.
    pub fn consume(&self, at: OffsetDateTime) -> DomainResult<QuoteId> {
        if at < self.issued_at {
            return Err(DomainError::QuoteNotYetValid);
        }
        if self.is_expired(at) {
            return Err(DomainError::QuoteExpired);
        }
        Ok(self.id)
    }

    /// Consumes the quote against the current UTC clock.
    pub fn consume_now(&self) -> DomainResult<QuoteId> {
        self.consume(OffsetDateTime::now_utc())
    }
}

impl<'de> Deserialize<'de> for Quote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            id: QuoteId,
            issued_at: OffsetDateTime,
            expires_at: OffsetDateTime,
        }

        let wire = Wire::deserialize(deserializer)?;
        let expected = wire.issued_at + Self::VALID_FOR;
        if wire.expires_at != expected {
            return Err(D::Error::custom(DomainError::InvalidQuoteExpiry));
        }
        Ok(Self {
            id: wire.id,
            issued_at: wire.issued_at,
            expires_at: wire.expires_at,
        })
    }
}

/// A validated half-open interval offered for an appointment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct AppointmentSlot {
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
}

impl AppointmentSlot {
    /// Constructs a slot whose end instant is strictly after its start.
    pub fn new(starts_at: OffsetDateTime, ends_at: OffsetDateTime) -> DomainResult<Self> {
        if ends_at <= starts_at {
            return Err(DomainError::InvalidAppointmentSlot);
        }
        Ok(Self { starts_at, ends_at })
    }

    /// Returns the inclusive start boundary of this half-open interval.
    pub const fn starts_at(&self) -> OffsetDateTime {
        self.starts_at
    }

    /// Returns the exclusive end boundary of this half-open interval.
    pub const fn ends_at(&self) -> OffsetDateTime {
        self.ends_at
    }

    /// Returns the elapsed duration of this interval.
    pub fn duration(&self) -> Duration {
        self.ends_at - self.starts_at
    }
}

impl<'de> Deserialize<'de> for AppointmentSlot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            starts_at: OffsetDateTime,
            ends_at: OffsetDateTime,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.starts_at, wire.ends_at).map_err(D::Error::custom)
    }
}

/// An idempotency key for one logical PA operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Validates and constructs a non-empty key.
    pub fn new(value: impl Into<String>) -> DomainResult<Self> {
        Ok(Self(non_blank(value.into(), "idempotency_key")?))
    }

    /// Returns the key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl From<IdempotencyKey> for String {
    fn from(value: IdempotencyKey) -> Self {
        value.0
    }
}

/// An immutable appointment request assembled from a quote and caller choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppointmentDraft {
    quote_id: QuoteId,
    caller: CallerIdentity,
    kind: AppointmentKind,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    requester_included: bool,
    idempotency_key: IdempotencyKey,
}

impl AppointmentDraft {
    /// Builds an appointment draft using the kind's duration and inclusion default.
    pub fn new(
        kind: AppointmentKind,
        caller: CallerIdentity,
        starts_at: OffsetDateTime,
        quote_id: QuoteId,
        idempotency_key: IdempotencyKey,
    ) -> DomainResult<Self> {
        Self::new_with_requester_inclusion(
            kind,
            caller,
            starts_at,
            quote_id,
            idempotency_key,
            kind.requester_included_by_default(),
        )
    }

    /// Alias for [`Self::new`].
    pub fn try_new(
        kind: AppointmentKind,
        caller: CallerIdentity,
        starts_at: OffsetDateTime,
        quote_id: QuoteId,
        idempotency_key: IdempotencyKey,
    ) -> DomainResult<Self> {
        Self::new(kind, caller, starts_at, quote_id, idempotency_key)
    }

    /// Builds an appointment draft with an explicit requester-inclusion choice.
    pub fn new_with_requester_inclusion(
        kind: AppointmentKind,
        caller: CallerIdentity,
        starts_at: OffsetDateTime,
        quote_id: QuoteId,
        idempotency_key: IdempotencyKey,
        requester_included: bool,
    ) -> DomainResult<Self> {
        let duration = kind.duration();
        if duration <= Duration::ZERO {
            return Err(DomainError::ZeroDuration { field: "duration" });
        }
        Ok(Self {
            quote_id,
            caller,
            kind,
            starts_at,
            ends_at: starts_at + duration,
            requester_included,
            idempotency_key,
        })
    }

    /// Returns the quote identifier used to prepare this draft.
    pub const fn quote_id(&self) -> QuoteId {
        self.quote_id
    }

    /// Returns the caller identity.
    pub fn caller(&self) -> &CallerIdentity {
        &self.caller
    }

    /// Returns the appointment kind.
    pub const fn kind(&self) -> AppointmentKind {
        self.kind
    }

    /// Returns the selected start instant.
    pub const fn starts_at(&self) -> OffsetDateTime {
        self.starts_at
    }

    /// Returns the exclusive end instant.
    pub const fn ends_at(&self) -> OffsetDateTime {
        self.ends_at
    }

    /// Returns the selected duration.
    pub fn duration(&self) -> Duration {
        self.ends_at - self.starts_at
    }

    /// Returns whether the requester is included in the appointment.
    pub const fn requester_included(&self) -> bool {
        self.requester_included
    }

    /// Returns the operation idempotency key.
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }
}

impl<'de> Deserialize<'de> for AppointmentDraft {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            quote_id: QuoteId,
            caller: CallerIdentity,
            kind: AppointmentKind,
            starts_at: OffsetDateTime,
            ends_at: OffsetDateTime,
            requester_included: bool,
            idempotency_key: IdempotencyKey,
        }

        let wire = Wire::deserialize(deserializer)?;
        let draft = Self::new_with_requester_inclusion(
            wire.kind,
            wire.caller,
            wire.starts_at,
            wire.quote_id,
            wire.idempotency_key,
            wire.requester_included,
        )
        .map_err(D::Error::custom)?;
        if draft.ends_at != wire.ends_at {
            return Err(D::Error::custom(
                "appointment draft end does not match kind duration",
            ));
        }
        Ok(draft)
    }
}

/// An immutable owner task request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerTaskDraft {
    kind: TaskKind,
    title: String,
    duration: DurationMinutes,
    due_at: Option<OffsetDateTime>,
    idempotency_key: IdempotencyKey,
}

impl OwnerTaskDraft {
    /// Builds an owner task using its task-kind default duration.
    pub fn new(
        kind: TaskKind,
        title: impl Into<String>,
        idempotency_key: IdempotencyKey,
    ) -> DomainResult<Self> {
        Self::with_duration(
            kind,
            title,
            kind.duration_minutes() as u32,
            None,
            idempotency_key,
        )
    }

    /// Builds an owner task with an explicit positive duration.
    pub fn with_duration(
        kind: TaskKind,
        title: impl Into<String>,
        duration_minutes: u32,
        due_at: Option<OffsetDateTime>,
        idempotency_key: IdempotencyKey,
    ) -> DomainResult<Self> {
        Ok(Self {
            kind,
            title: non_blank(title.into(), "title")?,
            duration: DurationMinutes::new(duration_minutes)?,
            due_at,
            idempotency_key,
        })
    }

    /// Alias for [`Self::with_duration`] without a due date.
    pub fn new_with_duration(
        kind: TaskKind,
        title: impl Into<String>,
        duration_minutes: u32,
        idempotency_key: IdempotencyKey,
    ) -> DomainResult<Self> {
        Self::with_duration(kind, title, duration_minutes, None, idempotency_key)
    }

    /// Builds an owner task with its task-kind duration and a due date.
    pub fn with_due_at(
        kind: TaskKind,
        title: impl Into<String>,
        due_at: OffsetDateTime,
        idempotency_key: IdempotencyKey,
    ) -> DomainResult<Self> {
        Self::with_duration(
            kind,
            title,
            kind.duration_minutes() as u32,
            Some(due_at),
            idempotency_key,
        )
    }

    /// Returns the task kind.
    pub const fn kind(&self) -> TaskKind {
        self.kind
    }

    /// Returns the short task title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the validated task duration.
    pub const fn duration(&self) -> DurationMinutes {
        self.duration
    }

    /// Returns the optional due date.
    pub const fn due_at(&self) -> Option<OffsetDateTime> {
        self.due_at
    }

    /// Returns the operation idempotency key.
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }
}

impl<'de> Deserialize<'de> for OwnerTaskDraft {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            kind: TaskKind,
            title: String,
            duration: DurationMinutes,
            due_at: Option<OffsetDateTime>,
            idempotency_key: IdempotencyKey,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::with_duration(
            wire.kind,
            wire.title,
            wire.duration.minutes(),
            wire.due_at,
            wire.idempotency_key,
        )
        .map_err(D::Error::custom)
    }
}

/// The lifecycle state of a requester-involved proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    /// Awaiting requester response.
    #[default]
    Pending,
    /// Requester accepted.
    Accepted,
    /// Requester declined.
    Declined,
    /// The response window elapsed.
    Expired,
}

impl ProposalState {
    /// Returns whether this state is terminal.
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }

    /// Returns whether the state may move to `next`.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Accepted)
                | (Self::Pending, Self::Declined)
                | (Self::Pending, Self::Expired)
        )
    }

    /// Applies an allowed transition.
    pub fn transition_to(self, next: Self) -> DomainResult<Self> {
        if self.is_terminal() {
            return Err(DomainError::TerminalProposalState { state: self });
        }
        if !self.can_transition_to(next) {
            return Err(DomainError::InvalidProposalTransition {
                from: self,
                to: next,
            });
        }
        Ok(next)
    }

    /// Moves pending to accepted.
    pub fn accept(self) -> DomainResult<Self> {
        self.transition_to(Self::Accepted)
    }

    /// Moves pending to declined.
    pub fn decline(self) -> DomainResult<Self> {
        self.transition_to(Self::Declined)
    }

    /// Moves pending to expired.
    pub fn expire(self) -> DomainResult<Self> {
        self.transition_to(Self::Expired)
    }
}

fn non_blank(value: String, field: &'static str) -> DomainResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(DomainError::BlankRequiredField { field });
    }
    Ok(trimmed.to_owned())
}

fn is_valid_email(value: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid timestamp")
    }

    fn caller() -> CallerIdentity {
        CallerIdentity::new(
            "Ada Lovelace",
            ConfirmedEmail::confirm("ada@example.com").expect("confirmed email"),
        )
        .expect("caller")
    }

    fn key() -> IdempotencyKey {
        IdempotencyKey::new("request-1").expect("key")
    }

    #[test]
    fn appointment_slot_accepts_half_open_interval_and_reports_duration() {
        let starts_at = now();
        let ends_at = OffsetDateTime::from_unix_timestamp(1_700_001_800).expect("valid timestamp");

        let slot = AppointmentSlot::new(starts_at, ends_at).expect("appointment slot");

        assert_eq!(slot.starts_at(), starts_at);
        assert_eq!(slot.ends_at(), ends_at);
        assert_eq!(slot.duration(), Duration::minutes(30));
    }

    #[test]
    fn appointment_slot_rejects_equal_bounds() {
        assert_eq!(
            AppointmentSlot::new(now(), now()).unwrap_err(),
            DomainError::InvalidAppointmentSlot
        );
    }

    #[test]
    fn appointment_slot_rejects_reversed_bounds_without_leaking_timestamps() {
        let starts_at = now() + Duration::minutes(30);
        let ends_at = now();
        let error = AppointmentSlot::new(starts_at, ends_at).unwrap_err();

        assert_eq!(error, DomainError::InvalidAppointmentSlot);
        let display = error.to_string();
        assert_eq!(display, "appointment slot interval is invalid");
        assert!(!display.contains(&starts_at.to_string()));
        assert!(!display.contains(&ends_at.to_string()));
    }

    #[test]
    fn appointment_slot_round_trips_through_serde() {
        let starts_at = now();
        let ends_at = OffsetDateTime::from_unix_timestamp(1_700_001_800).expect("valid timestamp");
        let slot = AppointmentSlot::new(starts_at, ends_at).expect("appointment slot");

        let encoded = serde_json::to_string(&slot).expect("serialize appointment slot");
        let decoded: AppointmentSlot =
            serde_json::from_str(&encoded).expect("deserialize appointment slot");

        assert_eq!(decoded, slot);
    }

    #[test]
    fn appointment_slot_deserialization_rejects_crafted_invalid_interval() {
        let starts_at = now();
        let crafted = serde_json::json!({
            "starts_at": starts_at,
            "ends_at": starts_at,
        });
        let encoded = serde_json::to_string(&crafted).expect("serialize crafted slot");

        let error = serde_json::from_str::<AppointmentSlot>(&encoded).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("appointment slot interval is invalid")
        );
    }

    #[test]
    fn appointment_kind_defaults_have_expected_durations_and_inclusion() {
        assert_eq!(AppointmentKind::Callback.duration_minutes(), 15);
        assert!(!AppointmentKind::Callback.requester_included_by_default());
        assert_eq!(AppointmentKind::Meeting.duration_minutes(), 30);
        assert!(AppointmentKind::Meeting.requester_included_by_default());
    }

    #[test]
    fn task_kind_defaults_have_expected_durations() {
        assert_eq!(TaskKind::Bill.duration_minutes(), 15);
        assert_eq!(TaskKind::Callback.duration_minutes(), 15);
        assert_eq!(TaskKind::Reading.duration_minutes(), 30);
        assert_eq!(TaskKind::EmailReply.duration_minutes(), 30);
        assert_eq!(TaskKind::Preparation.duration_minutes(), 60);
    }

    #[test]
    fn validation_rejects_blank_fields_bad_email_and_zero_duration() {
        assert!(matches!(
            CallerIdentity::new(
                "  ",
                ConfirmedEmail::confirm("ada@example.com").expect("confirmed email"),
            ),
            Err(DomainError::BlankRequiredField { field: "name" })
        ));
        assert_eq!(
            ConfirmedEmail::confirm("not-an-email").unwrap_err(),
            DomainError::InvalidEmail
        );
        assert_eq!(
            DurationMinutes::new(0).unwrap_err(),
            DomainError::ZeroDuration { field: "duration" }
        );
        assert!(IdempotencyKey::new("  ").is_err());
        assert!(OwnerTaskDraft::new_with_duration(TaskKind::Bill, "Pay", 0, key()).is_err());
    }

    #[test]
    fn confirmed_email_rejects_non_address_characters() {
        assert_eq!(
            ConfirmedEmail::confirm("ada()@example.com").unwrap_err(),
            DomainError::InvalidEmail
        );
    }

    #[test]
    fn confirmed_email_debug_redacts_through_domain_values() {
        const CALLER_EMAIL: &str = "ada.private@example.com";
        let confirmed = ConfirmedEmail::confirm(CALLER_EMAIL).expect("confirmed email");
        let caller = CallerIdentity::new("Ada", confirmed.clone()).expect("caller");
        let draft = AppointmentDraft::new(
            AppointmentKind::Callback,
            caller.clone(),
            now(),
            QuoteId::new(),
            key(),
        )
        .expect("appointment draft");

        for debug in [
            format!("{confirmed:?}"),
            format!("{caller:?}"),
            format!("{draft:?}"),
        ] {
            assert!(!debug.contains(CALLER_EMAIL));
            assert!(debug.contains("<redacted>"));
        }
    }

    #[test]
    fn caller_identity_deserialization_rejects_raw_or_asserted_confirmation() {
        let raw_email = r#"{"name":"Ada","email":"ada@example.com"}"#;
        assert!(serde_json::from_str::<CallerIdentity>(raw_email).is_err());

        let unconfirmed_email =
            r#"{"name":"Ada","email":{"value":"ada@example.com","confirmed":false}}"#;
        assert!(serde_json::from_str::<CallerIdentity>(unconfirmed_email).is_err());

        let asserted_confirmation =
            r#"{"name":"Ada","email":{"value":"ada@example.com","confirmed":true}}"#;
        assert!(serde_json::from_str::<CallerIdentity>(asserted_confirmation).is_err());

        let confirmed_marker = r#"{"value":"ada@example.com","confirmed":true}"#;
        assert!(serde_json::from_str::<ConfirmedEmail>(confirmed_marker).is_err());

        let bare_email = r#""ada@example.com""#;
        assert!(serde_json::from_str::<ConfirmedEmail>(bare_email).is_err());
    }

    #[test]
    fn appointment_draft_serialization_does_not_create_email_confirmation() {
        let draft = AppointmentDraft::new(
            AppointmentKind::Callback,
            caller(),
            now(),
            QuoteId::new(),
            key(),
        )
        .expect("appointment draft");
        let encoded = serde_json::to_string(&draft).expect("serialize draft");
        assert!(serde_json::from_str::<AppointmentDraft>(&encoded).is_err());
    }

    #[test]
    fn quote_expires_after_five_minutes_and_rejects_consumption_at_expiry() {
        let quote = Quote::with_id(QuoteId::new(), now());
        assert_eq!(quote.expires_at(), now() + Duration::minutes(5));
        assert_eq!(quote.consume(now() + Duration::minutes(4)), Ok(quote.id()));
        assert_eq!(
            quote.consume(now() + Duration::minutes(5)),
            Err(DomainError::QuoteExpired)
        );
    }

    #[test]
    fn quote_rejects_consumption_before_issuance() {
        let quote = Quote::with_id(QuoteId::new(), now());
        assert!(quote.consume(now() - Duration::seconds(1)).is_err());
    }

    #[test]
    fn drafts_are_constructed_with_private_immutable_fields() {
        let appointment = AppointmentDraft::new(
            AppointmentKind::Meeting,
            caller(),
            now(),
            QuoteId::new(),
            key(),
        )
        .expect("appointment draft");
        assert_eq!(appointment.duration(), Duration::minutes(30));
        assert!(appointment.requester_included());

        let task = OwnerTaskDraft::new(TaskKind::Preparation, "Prepare agenda", key())
            .expect("task draft");
        assert_eq!(task.duration().minutes(), 60);
        assert_eq!(task.title(), "Prepare agenda");
    }

    #[test]
    fn proposal_transitions_allow_pending_to_one_terminal_state_only() {
        assert_eq!(ProposalState::Pending.accept(), Ok(ProposalState::Accepted));
        assert_eq!(
            ProposalState::Pending.decline(),
            Ok(ProposalState::Declined)
        );
        assert_eq!(ProposalState::Pending.expire(), Ok(ProposalState::Expired));
        assert!(matches!(
            ProposalState::Accepted.decline(),
            Err(DomainError::TerminalProposalState {
                state: ProposalState::Accepted
            })
        ));
        assert!(
            ProposalState::Pending
                .transition_to(ProposalState::Pending)
                .is_err()
        );
    }

    #[test]
    fn declined_and_expired_proposals_reject_transitions() {
        assert!(matches!(
            ProposalState::Declined.accept(),
            Err(DomainError::TerminalProposalState {
                state: ProposalState::Declined
            })
        ));
        assert!(matches!(
            ProposalState::Expired.decline(),
            Err(DomainError::TerminalProposalState {
                state: ProposalState::Expired
            })
        ));
    }

    #[test]
    fn serde_serializes_confirmed_values_but_round_trips_safe_domain_values() {
        let draft = AppointmentDraft::new(
            AppointmentKind::Callback,
            caller(),
            now(),
            QuoteId::new(),
            key(),
        )
        .expect("appointment draft");
        let encoded = serde_json::to_string(&draft).expect("serialize draft");
        assert!(serde_json::from_str::<AppointmentDraft>(&encoded).is_err());

        let task = OwnerTaskDraft::new(TaskKind::Preparation, "Prepare agenda", key())
            .expect("owner task");
        let encoded = serde_json::to_string(&task).expect("serialize task");
        let decoded: OwnerTaskDraft = serde_json::from_str(&encoded).expect("deserialize task");
        assert_eq!(decoded, task);

        let quote = Quote::with_id(QuoteId::new(), now());
        let encoded = serde_json::to_string(&quote).expect("serialize quote");
        let decoded: Quote = serde_json::from_str(&encoded).expect("deserialize quote");
        assert_eq!(decoded, quote);

        assert!(serde_json::from_str::<IdempotencyKey>(r#""  ""#).is_err());
        assert!(serde_json::from_str::<DurationMinutes>("0").is_err());
    }
}
