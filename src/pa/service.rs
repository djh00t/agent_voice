//! Application services for the personal-assistant availability workflow.
//!
//! This module contains the provider-free availability and appointment-
//! preparation boundaries. Submission, messages, and owner tasks are separate
//! service packages.

use std::fmt;

use chrono::{DateTime, Utc};
use ring::digest;
use time::{OffsetDateTime, UtcOffset};

use super::availability::{AvailabilityError, AvailabilityPolicy};
use super::domain::{
    AppointmentDraft, AppointmentKind, AppointmentSlot, CallerIdentity, DomainError,
    IdempotencyKey, ProposalState, Quote, QuoteId,
};
use super::providers::{
    CalendarAttendee, CalendarEvent, GoogleCalendarProvider, GoogleProposalDraft, MailAddress,
    OutlookCalendarProvider, ProviderError, ProviderSession, TimeRange,
};
use super::store::{
    AuditEntityType, AuditEventType, MAX_APPOINTMENT_QUOTE_SLOTS, MessageProvider, MessageSummary,
    NotificationKind, NotificationRecipient, NotificationTemplateData, PaStore, StoreError,
    StoredAppointmentQuoteState, validate_message_idempotency_key, validate_message_source_id,
    validate_provider_message_id,
};

const PENDING_PROPOSAL_TITLE: &str = "Pending assistant request";

/// The result type returned by personal-assistant services.
pub type ServiceResult<T> = Result<T, ServiceError>;

/// Closed failures exposed by the availability service facade.
pub enum ServiceError {
    /// The service input was outside its bounded contract.
    InvalidInput { field: &'static str },
    /// Availability calculation or time conversion failed.
    Availability(AvailabilityError),
    /// Outlook calendar access failed.
    OutlookCalendar(ProviderError),
    /// Google calendar access failed.
    GoogleCalendar(ProviderError),
    /// Quote persistence failed.
    Store(StoreError),
    /// Appointment draft validation failed.
    Domain(DomainError),
    /// A message recording requires the explicitly configured owner address.
    OwnerNotConfigured,
    /// Neither calendar had a slot satisfying the policy.
    NoAvailability,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field } => write!(formatter, "{field} is invalid"),
            Self::Availability(_) => formatter.write_str("availability calculation failed"),
            Self::OutlookCalendar(_) => formatter.write_str("outlook calendar operation failed"),
            Self::GoogleCalendar(_) => formatter.write_str("google calendar operation failed"),
            Self::Store(_) => formatter.write_str("appointment quote store operation failed"),
            Self::Domain(_) => formatter.write_str("appointment request validation failed"),
            Self::OwnerNotConfigured => formatter.write_str("owner address is not configured"),
            Self::NoAvailability => formatter.write_str("no appointment slots are available"),
        }
    }
}

impl fmt::Debug for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field } => formatter
                .debug_struct("InvalidInput")
                .field("field", field)
                .finish(),
            Self::Availability(error) => formatter
                .debug_struct("Availability")
                .field("category", &availability_category(error))
                .finish(),
            Self::OutlookCalendar(_) => formatter.write_str("OutlookCalendar"),
            Self::GoogleCalendar(_) => formatter.write_str("GoogleCalendar"),
            Self::Store(_) => formatter.write_str("Store"),
            Self::Domain(_) => formatter.write_str("Domain"),
            Self::OwnerNotConfigured => formatter.write_str("OwnerNotConfigured"),
            Self::NoAvailability => formatter.write_str("NoAvailability"),
        }
    }
}

impl std::error::Error for ServiceError {}

fn availability_category(error: &AvailabilityError) -> &'static str {
    match error {
        AvailabilityError::InvalidBusyInterval { .. } => "invalid_busy_interval",
        AvailabilityError::InvalidWorkingWindow { .. } => "invalid_working_window",
        AvailabilityError::InvalidTimezone { .. } => "invalid_timezone",
        AvailabilityError::InvalidDuration { .. } => "invalid_duration",
        AvailabilityError::DateTimeOverflow => "date_time_overflow",
    }
}

/// The durable result of an availability search.
pub struct AvailabilitySearch {
    quote: Quote,
    appointment_kind: AppointmentKind,
    timezone: String,
    offered_slots: Vec<AppointmentSlot>,
}

impl AvailabilitySearch {
    /// Returns the opaque quote and its five-minute validity interval.
    pub fn quote(&self) -> &Quote {
        &self.quote
    }

    /// Returns the appointment kind frozen into the quote.
    pub const fn appointment_kind(&self) -> AppointmentKind {
        self.appointment_kind
    }

    /// Returns the owner's validated IANA timezone.
    pub fn timezone(&self) -> &str {
        &self.timezone
    }

    /// Returns the ordered slots frozen into the quote.
    pub fn offered_slots(&self) -> &[AppointmentSlot] {
        &self.offered_slots
    }
}

impl fmt::Debug for AvailabilitySearch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AvailabilitySearch")
            .field("quote", &"<redacted>")
            .field("appointment_kind", &self.appointment_kind)
            .field("timezone", &"<redacted>")
            .field("offered_slot_count", &self.offered_slots.len())
            .finish()
    }
}

/// The durable, provider-free result of preparing an appointment request.
///
/// The recap is intentionally available through an explicit accessor because
/// it contains caller identity data. The ordinary debug representation keeps
/// that data, the quote identity, and the stored timestamps redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    draft_id: i64,
    quote_id: QuoteId,
    source_id: String,
    idempotency_key: IdempotencyKey,
    caller: CallerIdentity,
    kind: AppointmentKind,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    timezone: String,
    requester_included: bool,
    recap: String,
}

/// A capability minted by the trusted affirmative voice/HTTP boundary.
///
/// The constructor is crate-private so transcripts, booleans, and external
/// request payloads cannot manufacture confirmation outside the application.
pub struct ExplicitConfirmation(());

impl ExplicitConfirmation {
    /// Mints one confirmation capability after the affirmative boundary has
    /// already made its own policy decision.
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self(())
    }
}

impl fmt::Debug for ExplicitConfirmation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExplicitConfirmation(<redacted>)")
    }
}

/// The exact prepared request that crossed the explicit confirmation gate.
#[derive(Clone, PartialEq, Eq)]
pub struct ConfirmedPreparedRequest(PreparedRequest);

impl ConfirmedPreparedRequest {
    /// Consumes the exact prepared recap and a trusted affirmative capability.
    pub fn new(
        prepared: PreparedRequest,
        _confirmation: ExplicitConfirmation,
    ) -> ServiceResult<Self> {
        Ok(Self(prepared))
    }
}

impl fmt::Debug for ConfirmedPreparedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConfirmedPreparedRequest(<redacted>)")
    }
}

/// The durable result of submitting one confirmed request.
#[derive(Clone, PartialEq, Eq)]
pub struct SubmittedRequest {
    proposal_id: i64,
    event_mapping_id: i64,
    owner_notification_id: i64,
    requester_notification_id: Option<i64>,
    state: ProposalState,
}

impl SubmittedRequest {
    /// Returns the durable pending proposal identity.
    pub const fn proposal_id(&self) -> i64 {
        self.proposal_id
    }

    /// Returns the durable event-mapping identity.
    pub const fn event_mapping_id(&self) -> i64 {
        self.event_mapping_id
    }

    /// Returns the durable owner-notification identity.
    pub const fn owner_notification_id(&self) -> i64 {
        self.owner_notification_id
    }

    /// Returns the requester notification identity when policy includes it.
    pub const fn requester_notification_id(&self) -> Option<i64> {
        self.requester_notification_id
    }

    /// Returns the durable proposal lifecycle state.
    pub const fn state(&self) -> ProposalState {
        self.state
    }

    /// Submission always means pending/requested, never booked.
    pub const fn is_pending(&self) -> bool {
        matches!(self.state, ProposalState::Pending)
    }
}

impl fmt::Debug for SubmittedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SubmittedRequest { ids: <redacted>, state: Pending }")
    }
}

impl PreparedRequest {
    /// Returns the durable appointment-draft database ID.
    pub const fn draft_id(&self) -> i64 {
        self.draft_id
    }

    /// Alias emphasizing that the ID belongs to an appointment draft.
    pub const fn appointment_draft_id(&self) -> i64 {
        self.draft_id()
    }

    /// Returns the quote identity used for preparation.
    pub const fn quote_id(&self) -> QuoteId {
        self.quote_id
    }

    /// Returns the caller identity through an explicit accessor.
    pub fn caller(&self) -> &CallerIdentity {
        &self.caller
    }

    /// Returns the caller's validated name.
    pub fn caller_name(&self) -> &str {
        self.caller.name()
    }

    /// Returns the caller's confirmed email.
    pub fn caller_email(&self) -> &str {
        self.caller.email()
    }

    /// Returns the frozen appointment kind.
    pub const fn kind(&self) -> AppointmentKind {
        self.kind
    }

    /// Alias emphasizing that this is the appointment kind.
    pub const fn appointment_kind(&self) -> AppointmentKind {
        self.kind()
    }

    /// Returns the selected start instant.
    pub const fn starts_at(&self) -> OffsetDateTime {
        self.starts_at
    }

    /// Alias emphasizing that this is the selected slot start.
    pub const fn selected_starts_at(&self) -> OffsetDateTime {
        self.starts_at()
    }

    /// Returns the selected exclusive end instant.
    pub const fn ends_at(&self) -> OffsetDateTime {
        self.ends_at
    }

    /// Alias emphasizing that this is the selected slot end.
    pub const fn selected_ends_at(&self) -> OffsetDateTime {
        self.ends_at()
    }

    /// Returns the quote's validated IANA timezone.
    pub fn timezone(&self) -> &str {
        &self.timezone
    }

    /// Returns whether the requester is included in the appointment.
    pub const fn requester_included(&self) -> bool {
        self.requester_included
    }

    /// Alias for [`Self::requester_included`].
    pub const fn includes_requester(&self) -> bool {
        self.requester_included()
    }

    /// Returns the exact deterministic recap intended for spoken confirmation.
    pub fn recap(&self) -> &str {
        &self.recap
    }

    /// Alias for [`Self::recap`].
    pub fn spoken_recap(&self) -> &str {
        self.recap()
    }
}

impl fmt::Debug for PreparedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedRequest")
            .field("draft_id", &self.draft_id)
            .field("quote_id", &"<redacted>")
            .field("caller", &"<redacted>")
            .field("kind", &self.kind)
            .field("starts_at", &"<redacted>")
            .field("ends_at", &"<redacted>")
            .field("timezone", &"<redacted>")
            .field("requester_included", &self.requester_included)
            .field("recap", &"<redacted>")
            .finish()
    }
}

/// The durable result of recording one voice-call summary.
///
/// The associated message and notification values remain in the store. This
/// result exposes only their database identities; summary, owner, source, and
/// timestamps are intentionally unavailable through ordinary formatting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RecordedMessage {
    message_id: i64,
    notification_id: i64,
}

impl RecordedMessage {
    /// Returns the durable message database identity.
    pub const fn message_id(self) -> i64 {
        self.message_id
    }

    /// Returns the durable notification database identity.
    pub const fn notification_id(self) -> i64 {
        self.notification_id
    }
}

impl fmt::Debug for RecordedMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("RecordedMessage { message_id: <redacted>, notification_id: <redacted> }")
    }
}

/// Coordinates availability policy, calendar reads, and durable quote writes.
pub struct PaService<'a> {
    store: &'a PaStore,
    outlook: &'a dyn OutlookCalendarProvider,
    outlook_session: &'a ProviderSession,
    google: &'a dyn GoogleCalendarProvider,
    google_session: &'a ProviderSession,
    policy: AvailabilityPolicy,
    owner: Option<MailAddress>,
}

impl<'a> PaService<'a> {
    /// Creates a facade over one store, both calendar capabilities, and their
    /// explicit sessions. The policy is cloned for service-owned stability.
    pub fn new(
        store: &'a PaStore,
        outlook: &'a dyn OutlookCalendarProvider,
        outlook_session: &'a ProviderSession,
        google: &'a dyn GoogleCalendarProvider,
        google_session: &'a ProviderSession,
        policy: &AvailabilityPolicy,
    ) -> Self {
        Self {
            store,
            outlook,
            outlook_session,
            google,
            google_session,
            policy: policy.clone(),
            owner: None,
        }
    }

    /// Creates a message-capable facade with its validated owner address.
    #[allow(clippy::too_many_arguments)]
    pub fn with_owner(
        store: &'a PaStore,
        outlook: &'a dyn OutlookCalendarProvider,
        outlook_session: &'a ProviderSession,
        google: &'a dyn GoogleCalendarProvider,
        google_session: &'a ProviderSession,
        policy: &AvailabilityPolicy,
        owner: MailAddress,
    ) -> Self {
        Self {
            store,
            outlook,
            outlook_session,
            google,
            google_session,
            policy: policy.clone(),
            owner: Some(owner),
        }
    }

    /// Searches both calendars and persists the exact ordered offered slots.
    ///
    /// Every invocation issues a fresh opaque quote. A failed provider read,
    /// an invalid range, an empty result, or a failed store write leaves no
    /// quote behind.
    pub async fn search_slots(
        &self,
        appointment_kind: AppointmentKind,
        now: OffsetDateTime,
        limit: usize,
    ) -> ServiceResult<AvailabilitySearch> {
        if limit == 0 || limit > MAX_APPOINTMENT_QUOTE_SLOTS {
            return Err(ServiceError::InvalidInput { field: "limit" });
        }

        let now = now.to_offset(UtcOffset::UTC);
        let horizon_end =
            now.checked_add(self.policy.booking_horizon())
                .ok_or(ServiceError::Availability(
                    AvailabilityError::DateTimeOverflow,
                ))?;
        let provider_start =
            now.checked_sub(self.policy.meeting_buffer())
                .ok_or(ServiceError::Availability(
                    AvailabilityError::DateTimeOverflow,
                ))?;
        let provider_end = horizon_end
            .checked_add(self.policy.meeting_buffer())
            .ok_or(ServiceError::Availability(
                AvailabilityError::DateTimeOverflow,
            ))?;
        let quote_expiry = now
            .checked_add(Quote::VALID_FOR)
            .ok_or(ServiceError::Availability(
                AvailabilityError::DateTimeOverflow,
            ))?;
        let range = TimeRange::new(to_chrono_utc(provider_start)?, to_chrono_utc(provider_end)?)
            .map_err(|_| ServiceError::InvalidInput {
                field: "time_range",
            })?;

        let outlook_busy = self
            .outlook
            .list_busy(self.outlook_session, &range)
            .await
            .map_err(ServiceError::OutlookCalendar)?;
        let google_busy = self
            .google
            .list_busy(self.google_session, &range)
            .await
            .map_err(ServiceError::GoogleCalendar)?;

        let starts = self
            .policy
            .available_slots(
                now,
                appointment_kind.duration(),
                &outlook_busy,
                &google_busy,
                limit,
            )
            .map_err(ServiceError::Availability)?;
        if starts.is_empty() {
            return Err(ServiceError::NoAvailability);
        }

        let mut offered_slots = Vec::with_capacity(starts.len());
        for starts_at in starts {
            let ends_at = starts_at.checked_add(appointment_kind.duration()).ok_or(
                ServiceError::Availability(AvailabilityError::DateTimeOverflow),
            )?;
            let slot = AppointmentSlot::new(starts_at, ends_at).map_err(|_| {
                ServiceError::InvalidInput {
                    field: "appointment_slot",
                }
            })?;
            offered_slots.push(slot);
        }

        // Checked above so Quote::new cannot overflow its fixed five-minute
        // expiry calculation.
        debug_assert_eq!(quote_expiry, now + Quote::VALID_FOR);
        let quote = Quote::new(now);
        let stored = self
            .store
            .save_appointment_quote(
                &quote,
                appointment_kind,
                self.policy.timezone(),
                &offered_slots,
            )
            .map_err(ServiceError::Store)?;

        Ok(AvailabilitySearch {
            quote: stored.quote().clone(),
            appointment_kind: stored.appointment_kind(),
            timezone: stored.timezone().to_owned(),
            offered_slots: stored.offered_slots().to_vec(),
        })
    }

    /// Prepares an immutable appointment request from a frozen quote.
    ///
    /// This boundary performs no calendar-provider calls. The store owns the
    /// atomic quote selection, draft persistence, exact-retry behavior, and
    /// immutable conflict checks.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_request(
        &self,
        quote_id: QuoteId,
        slot_index: u32,
        caller: CallerIdentity,
        expected_kind: AppointmentKind,
        requester_included: Option<bool>,
        source_id: impl AsRef<str>,
        idempotency_key: IdempotencyKey,
        now: OffsetDateTime,
    ) -> ServiceResult<PreparedRequest> {
        let quoted = self
            .store
            .load_appointment_quote_by_id(quote_id)
            .map_err(ServiceError::Store)?;
        if quoted.appointment_kind() != expected_kind {
            return Err(ServiceError::Store(StoreError::Conflict {
                resource: "appointment quote",
            }));
        }

        let slot = quoted
            .offered_slots()
            .get(usize::try_from(slot_index).map_err(|_| {
                if quoted.appointment_draft().is_some() {
                    ServiceError::Store(StoreError::Conflict {
                        resource: "appointment quote",
                    })
                } else {
                    ServiceError::Store(StoreError::InvalidInput {
                        field: "slot_index",
                    })
                }
            })?)
            .ok_or_else(|| {
                if quoted.appointment_draft().is_some() {
                    ServiceError::Store(StoreError::Conflict {
                        resource: "appointment quote",
                    })
                } else {
                    ServiceError::Store(StoreError::InvalidInput {
                        field: "slot_index",
                    })
                }
            })?;
        let requester_included =
            requester_included.unwrap_or_else(|| expected_kind.requester_included_by_default());
        let draft = AppointmentDraft::new_with_requester_inclusion(
            expected_kind,
            caller,
            slot.starts_at(),
            quote_id,
            idempotency_key,
            requester_included,
        )
        .map_err(ServiceError::Domain)?;
        let prepared = self
            .store
            .prepare_appointment_draft_from_quote(
                quote_id,
                slot_index,
                source_id,
                &draft,
                now.to_offset(UtcOffset::UTC),
            )
            .map_err(ServiceError::Store)?;
        let stored_draft_record = prepared.appointment_draft().ok_or(ServiceError::Store(
            StoreError::StoredRecordInvalid {
                resource: "appointment quote",
            },
        ))?;
        let stored_draft = stored_draft_record.draft();
        let draft_id = prepared.appointment_draft_id().ok_or(ServiceError::Store(
            StoreError::StoredRecordInvalid {
                resource: "appointment quote",
            },
        ))?;
        let recap = appointment_recap(stored_draft, prepared.timezone())?;

        Ok(PreparedRequest {
            draft_id,
            quote_id: prepared.quote_id(),
            source_id: stored_draft_record.source_id().to_owned(),
            idempotency_key: stored_draft.idempotency_key().clone(),
            caller: stored_draft.caller().clone(),
            kind: stored_draft.kind(),
            starts_at: stored_draft.starts_at(),
            ends_at: stored_draft.ends_at(),
            timezone: prepared.timezone().to_owned(),
            requester_included: stored_draft.requester_included(),
            recap,
        })
    }

    /// Records one validated voice-call summary and queues its owner-only
    /// call-summary notification.
    ///
    /// The validated source identity is persisted as `voice:<source>` so it
    /// cannot collide with Outlook or Gmail source identities. The raw source
    /// remains the input to message, notification, and audit idempotency/
    /// provider identities. The summary is a validated structured value; raw
    /// transcripts and message bodies cannot cross this boundary. Each local
    /// write is independently idempotent, so retries resume after any durable
    /// prefix left by a failed tail write.
    pub fn record_message(
        &self,
        summary: MessageSummary,
        source_id: impl AsRef<str>,
        received_at: OffsetDateTime,
    ) -> ServiceResult<RecordedMessage> {
        let owner = self
            .owner
            .as_ref()
            .ok_or(ServiceError::OwnerNotConfigured)?;
        let source_id = validate_message_source_id(source_id.as_ref().to_owned())
            .map_err(ServiceError::Store)?;
        let stored_source_id = validate_message_source_id(format!("voice:{source_id}"))
            .map_err(ServiceError::Store)?;
        let message_key = format!("pa-voice-message-recorded-{source_id}");
        let provider_message_id = format!("pa-voice-provider-message-{source_id}");
        let notification_key = format!("pa-voice-call-summary-notification-{source_id}");
        let message_audit_key = format!("pa-voice-message-recorded-audit-{source_id}");
        let notification_audit_key = format!("pa-voice-notification-enqueued-audit-{source_id}");

        // Validate every derived identity before the first write. This keeps
        // overlong source values from producing a durable partial prefix.
        validate_message_idempotency_key(message_key.clone()).map_err(ServiceError::Store)?;
        validate_provider_message_id(provider_message_id.clone()).map_err(ServiceError::Store)?;
        validate_message_idempotency_key(notification_key.clone()).map_err(ServiceError::Store)?;
        validate_message_idempotency_key(message_audit_key.clone()).map_err(ServiceError::Store)?;
        validate_message_idempotency_key(notification_audit_key.clone())
            .map_err(ServiceError::Store)?;

        let received_at = canonicalize_utc_second(received_at, "received_at")?;
        let message = self
            .store
            .record_message(
                message_key,
                &stored_source_id,
                MessageProvider::Voice,
                provider_message_id,
                summary.clone(),
                None,
                None,
                received_at,
            )
            .map_err(ServiceError::Store)?;
        let recipient =
            NotificationRecipient::new(owner.as_str().to_owned()).map_err(ServiceError::Store)?;
        let template = NotificationTemplateData::new(
            Some(summary.as_str().to_owned()),
            None,
            None,
            None,
            None,
            None,
        )
        .map_err(ServiceError::Store)?;
        let notification = self
            .store
            .enqueue_notification(
                notification_key,
                None,
                None,
                NotificationKind::CallSummary,
                recipient,
                template,
                received_at,
            )
            .map_err(ServiceError::Store)?;
        self.store
            .append_audit_event(
                message_audit_key,
                AuditEventType::MessageRecorded,
                AuditEntityType::Message,
                message.id().to_string(),
                received_at,
            )
            .map_err(ServiceError::Store)?;
        self.store
            .append_audit_event(
                notification_audit_key,
                AuditEventType::NotificationEnqueued,
                AuditEntityType::Notification,
                notification.id().to_string(),
                received_at,
            )
            .map_err(ServiceError::Store)?;
        Ok(RecordedMessage {
            message_id: message.id(),
            notification_id: notification.id(),
        })
    }

    /// Submits one explicitly confirmed prepared request as a pending Google
    /// proposal. Calendar availability is rechecked immediately before a new
    /// external create, while every local tail write is independently
    /// idempotent and therefore repairable after a partial failure.
    pub async fn submit_request(
        &self,
        confirmed: ConfirmedPreparedRequest,
        now: OffsetDateTime,
    ) -> ServiceResult<SubmittedRequest> {
        let owner = self
            .owner
            .as_ref()
            .ok_or(ServiceError::OwnerNotConfigured)?;
        let prepared = confirmed.0;
        let validation_now = now.to_offset(UtcOffset::UTC);
        let quote = self
            .store
            .load_appointment_quote_by_draft_id(prepared.draft_id)
            .map_err(ServiceError::Store)?;
        let stored_draft = quote.appointment_draft().ok_or(ServiceError::Store(
            StoreError::StoredRecordInvalid {
                resource: "appointment quote",
            },
        ))?;
        validate_prepared_request(&prepared, &quote, stored_draft)?;
        match quote.state() {
            StoredAppointmentQuoteState::Prepared if validation_now < quote.quote().issued_at() => {
                return Err(ServiceError::Store(StoreError::AppointmentQuoteNotYetValid));
            }
            StoredAppointmentQuoteState::Prepared
                if validation_now >= quote.quote().expires_at() =>
            {
                return Err(ServiceError::Store(StoreError::AppointmentQuoteExpired));
            }
            StoredAppointmentQuoteState::Prepared | StoredAppointmentQuoteState::Consumed => {}
            StoredAppointmentQuoteState::Issued => {
                return Err(ServiceError::Store(StoreError::Conflict {
                    resource: "appointment quote",
                }));
            }
        }
        let durable_now = canonicalize_submission_timestamp(
            validation_now,
            quote.quote().issued_at(),
            quote.quote().expires_at(),
        )?;

        let draft = stored_draft.draft();
        let draft_id = stored_draft.id();
        let proposal_key = format!("pa-proposal-draft-{draft_id}");
        let proposal_source = format!("pa-proposal-source-draft-{draft_id}");
        let mapping_source = |proposal_id| event_mapping_source(proposal_id, owner);

        let existing_proposal = match quote.proposal_id() {
            Some(proposal_id) => {
                let proposal = self
                    .store
                    .load_proposal_by_id(proposal_id)
                    .map_err(ServiceError::Store)?;
                validate_existing_proposal(&proposal, draft_id, &proposal_key, &proposal_source)?;
                Some(proposal)
            }
            None => None,
        };

        let existing_mapping = if let Some(proposal) = &existing_proposal {
            match self.store.load_event_mapping_by_proposal_id(proposal.id()) {
                Ok(mapping) => {
                    validate_existing_mapping(
                        &mapping,
                        proposal.id(),
                        &mapping_source(proposal.id()),
                        draft,
                    )?;
                    Some(mapping)
                }
                Err(StoreError::NotFound {
                    resource: "event mapping",
                }) => None,
                Err(error) => return Err(ServiceError::Store(error)),
            }
        } else {
            None
        };

        let (proposal, mapping) = if let Some(mapping) = existing_mapping {
            (existing_proposal.expect("mapping has a proposal"), mapping)
        } else {
            if existing_proposal.is_none() {
                let recheck_start = draft
                    .starts_at()
                    .checked_sub(self.policy.meeting_buffer())
                    .ok_or(ServiceError::Availability(
                        AvailabilityError::DateTimeOverflow,
                    ))?;
                let recheck_end = draft
                    .ends_at()
                    .checked_add(self.policy.meeting_buffer())
                    .ok_or(ServiceError::Availability(
                        AvailabilityError::DateTimeOverflow,
                    ))?;
                let recheck =
                    TimeRange::new(to_chrono_utc(recheck_start)?, to_chrono_utc(recheck_end)?)
                        .map_err(|_| ServiceError::InvalidInput {
                            field: "time_range",
                        })?;
                let outlook_busy = self
                    .outlook
                    .list_busy(self.outlook_session, &recheck)
                    .await
                    .map_err(ServiceError::OutlookCalendar)?;
                let google_busy = self
                    .google
                    .list_busy(self.google_session, &recheck)
                    .await
                    .map_err(ServiceError::GoogleCalendar)?;
                if busy_overlaps(&outlook_busy, recheck_start, recheck_end)
                    || busy_overlaps(&google_busy, recheck_start, recheck_end)
                {
                    return Err(ServiceError::NoAvailability);
                }
            }
            let time_range = TimeRange::new(
                to_chrono_utc(draft.starts_at())?,
                to_chrono_utc(draft.ends_at())?,
            )
            .map_err(|_| ServiceError::InvalidInput {
                field: "time_range",
            })?;
            let proposal_draft = GoogleProposalDraft::from_owner(
                proposal_operation_key(draft_id),
                PENDING_PROPOSAL_TITLE,
                time_range,
                quote.timezone(),
                CalendarAttendee::needs_action(owner.clone()),
            )
            .map_err(ServiceError::GoogleCalendar)?;

            let found = match self
                .google
                .find_proposal(self.google_session, &proposal_draft)
                .await
            {
                Ok(event) => {
                    validate_pending_google_event(&event, &proposal_draft, owner)?;
                    Some(event)
                }
                Err(ProviderError::NotFound) => None,
                Err(error) => return Err(ServiceError::GoogleCalendar(error)),
            };

            let (proposal, event) = if let Some(event) = found {
                let proposal = match existing_proposal {
                    Some(proposal) => proposal,
                    None => self
                        .store
                        .submit_appointment_quote(
                            quote.quote_id(),
                            draft_id,
                            &proposal_key,
                            &proposal_source,
                            durable_now,
                        )
                        .map_err(ServiceError::Store)?,
                };
                (proposal, event)
            } else {
                let proposal = match existing_proposal {
                    Some(proposal) => proposal,
                    None => self
                        .store
                        .submit_appointment_quote(
                            quote.quote_id(),
                            draft_id,
                            &proposal_key,
                            &proposal_source,
                            durable_now,
                        )
                        .map_err(ServiceError::Store)?,
                };
                let event = self
                    .google
                    .create_proposal(self.google_session, &proposal_draft)
                    .await
                    .map_err(ServiceError::GoogleCalendar)?;
                validate_pending_google_event(&event, &proposal_draft, owner)?;
                (proposal, event)
            };

            let mapping = self
                .store
                .attach_event_mapping(
                    proposal.id(),
                    "google_calendar",
                    event.provider_event_id(),
                    mapping_source(proposal.id()),
                    Some(draft.starts_at()),
                    Some(draft.ends_at()),
                )
                .map_err(ServiceError::Store)?;
            (proposal, mapping)
        };

        let consumed_at = self
            .store
            .load_appointment_quote_by_id(quote.quote_id())
            .map_err(ServiceError::Store)?
            .consumed_at()
            .ok_or(ServiceError::Store(StoreError::StoredRecordInvalid {
                resource: "appointment quote",
            }))?;
        let template = NotificationTemplateData::new(
            Some(PENDING_PROPOSAL_TITLE.to_owned()),
            Some(draft.starts_at()),
            Some(draft.ends_at()),
            Some(quote.timezone().to_owned()),
            Some(draft.kind()),
            Some(ProposalState::Pending),
        )
        .map_err(ServiceError::Store)?;
        let owner_recipient =
            NotificationRecipient::new(owner.as_str().to_owned()).map_err(ServiceError::Store)?;
        let owner_notification = self
            .store
            .enqueue_notification(
                format!("pa-notify-owner-{}", proposal.id()),
                Some(proposal.id()),
                Some(mapping.id()),
                NotificationKind::ProposalRequested,
                owner_recipient,
                template.clone(),
                consumed_at,
            )
            .map_err(ServiceError::Store)?;
        let requester_notification = if draft.requester_included() {
            let requester = NotificationRecipient::new(draft.caller().email().to_owned())
                .map_err(ServiceError::Store)?;
            Some(
                self.store
                    .enqueue_notification(
                        format!("pa-notify-requester-{}", proposal.id()),
                        Some(proposal.id()),
                        Some(mapping.id()),
                        NotificationKind::ProposalRequested,
                        requester,
                        template,
                        consumed_at,
                    )
                    .map_err(ServiceError::Store)?,
            )
        } else {
            None
        };

        append_submission_audit(
            self.store,
            format!("pa-audit-request-submitted-{draft_id}"),
            AuditEventType::RequestSubmitted,
            AuditEntityType::AppointmentRequest,
            draft_id.to_string(),
            consumed_at,
        )?;
        append_submission_audit(
            self.store,
            format!("pa-audit-proposal-created-{}", proposal.id()),
            AuditEventType::ProposalCreated,
            AuditEntityType::Proposal,
            proposal.id().to_string(),
            consumed_at,
        )?;
        append_submission_audit(
            self.store,
            format!("pa-audit-notification-enqueued-owner-{}", proposal.id()),
            AuditEventType::NotificationEnqueued,
            AuditEntityType::Notification,
            owner_notification.id().to_string(),
            consumed_at,
        )?;
        if let Some(requester_notification) = &requester_notification {
            append_submission_audit(
                self.store,
                format!("pa-audit-notification-enqueued-requester-{}", proposal.id()),
                AuditEventType::NotificationEnqueued,
                AuditEntityType::Notification,
                requester_notification.id().to_string(),
                consumed_at,
            )?;
        }

        Ok(SubmittedRequest {
            proposal_id: proposal.id(),
            event_mapping_id: mapping.id(),
            owner_notification_id: owner_notification.id(),
            requester_notification_id: requester_notification.map(|notification| notification.id()),
            state: ProposalState::Pending,
        })
    }
}

fn proposal_operation_key(draft_id: i64) -> String {
    format!("pa-google-proposal-draft-{draft_id}")
}

fn event_mapping_source(proposal_id: i64, owner: &MailAddress) -> String {
    format!(
        "pa-event-source-{proposal_id}-{}",
        owner_fingerprint(owner.as_str())
    )
}

fn owner_fingerprint(owner: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = digest::digest(&digest::SHA256, owner.as_bytes());
    let mut fingerprint = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        fingerprint.push(char::from(HEX[usize::from(byte >> 4)]));
        fingerprint.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    fingerprint
}

fn validate_prepared_request(
    prepared: &PreparedRequest,
    quote: &super::store::StoredAppointmentQuote,
    stored_draft: &super::store::StoredAppointmentDraft,
) -> ServiceResult<()> {
    let draft = stored_draft.draft();
    let selected_slot = quote
        .selected_slot_index()
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| quote.offered_slots().get(index))
        .ok_or(ServiceError::Store(StoreError::StoredRecordInvalid {
            resource: "appointment quote",
        }))?;
    let expected_recap = appointment_recap(draft, quote.timezone())?;
    if prepared.draft_id != stored_draft.id()
        || prepared.quote_id != quote.quote_id()
        || prepared.source_id != stored_draft.source_id()
        || prepared.idempotency_key != *draft.idempotency_key()
        || prepared.caller != *draft.caller()
        || prepared.kind != draft.kind()
        || prepared.starts_at != draft.starts_at()
        || prepared.ends_at != draft.ends_at()
        || prepared.timezone != quote.timezone()
        || prepared.requester_included != draft.requester_included()
        || selected_slot.starts_at() != draft.starts_at()
        || selected_slot.ends_at() != draft.ends_at()
        || draft.starts_at().offset() != UtcOffset::UTC
        || draft.ends_at().offset() != UtcOffset::UTC
        || expected_recap != prepared.recap
        || draft.quote_id() != quote.quote_id()
        || draft.ends_at() <= draft.starts_at()
    {
        return Err(ServiceError::Store(StoreError::Conflict {
            resource: "prepared appointment request",
        }));
    }
    Ok(())
}

fn validate_existing_proposal(
    proposal: &super::store::StoredProposal,
    draft_id: i64,
    proposal_key: &str,
    proposal_source: &str,
) -> ServiceResult<()> {
    if proposal.source().appointment_draft_id() != Some(draft_id)
        || proposal.idempotency_key() != proposal_key
        || proposal.source_id() != proposal_source
        || proposal.state() != ProposalState::Pending
    {
        return Err(ServiceError::Store(StoreError::Conflict {
            resource: "appointment request proposal",
        }));
    }
    Ok(())
}

fn validate_existing_mapping(
    mapping: &super::store::StoredEventMapping,
    proposal_id: i64,
    expected_source: &str,
    draft: &AppointmentDraft,
) -> ServiceResult<()> {
    if mapping.proposal_id() != proposal_id
        || mapping.provider() != "google_calendar"
        || mapping.source_id() != expected_source
        || mapping.starts_at() != Some(draft.starts_at())
        || mapping.ends_at() != Some(draft.ends_at())
    {
        return Err(ServiceError::Store(StoreError::Conflict {
            resource: "appointment request mapping",
        }));
    }
    Ok(())
}

fn validate_pending_google_event(
    event: &CalendarEvent,
    draft: &GoogleProposalDraft,
    owner: &MailAddress,
) -> ServiceResult<()> {
    let valid = event.operation_key() == draft.operation_key()
        && event.title() == draft.pending_title()
        && event.time_range() == draft.time_range()
        && event.timezone() == draft.timezone()
        && event.attendees().len() == 1
        && event.attendees()[0].address() == owner
        && event.attendees()[0].rsvp().is_needs_action();
    if valid {
        Ok(())
    } else {
        Err(ServiceError::GoogleCalendar(ProviderError::Conflict))
    }
}

fn busy_overlaps(
    intervals: &[super::availability::BusyInterval],
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
) -> bool {
    intervals
        .iter()
        .any(|interval| interval.starts_at() < ends_at && interval.ends_at() > starts_at)
}

fn append_submission_audit(
    store: &PaStore,
    key: String,
    event_type: AuditEventType,
    entity_type: AuditEntityType,
    entity_id: String,
    occurred_at: OffsetDateTime,
) -> ServiceResult<()> {
    store
        .append_audit_event(key, event_type, entity_type, entity_id, occurred_at)
        .map(|_| ())
        .map_err(ServiceError::Store)
}

fn appointment_recap(draft: &AppointmentDraft, timezone: &str) -> ServiceResult<String> {
    let timezone = timezone.parse::<chrono_tz::Tz>().map_err(|_| {
        ServiceError::Store(StoreError::StoredRecordInvalid {
            resource: "appointment quote",
        })
    })?;
    let local_start = to_chrono_utc(draft.starts_at())?.with_timezone(&timezone);
    let kind = match draft.kind() {
        AppointmentKind::Callback => "Callback",
        AppointmentKind::Meeting => "Meeting",
    };
    let inclusion = if draft.requester_included() {
        "requester included"
    } else {
        "requester not included"
    };
    Ok(format!(
        "{kind} for {} <{}> on {} at {} ({}); {inclusion}.",
        draft.caller().name(),
        draft.caller().email(),
        local_start.format("%Y-%m-%d"),
        local_start.format("%H:%M"),
        timezone.name(),
    ))
}

fn to_chrono_utc(value: OffsetDateTime) -> ServiceResult<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(value.unix_timestamp(), value.nanosecond()).ok_or(
        ServiceError::Availability(AvailabilityError::DateTimeOverflow),
    )
}

fn canonicalize_utc_second(
    value: OffsetDateTime,
    field: &'static str,
) -> ServiceResult<OffsetDateTime> {
    value
        .to_offset(UtcOffset::UTC)
        .replace_nanosecond(0)
        .map_err(|_| ServiceError::Store(StoreError::InvalidInput { field }))
}

fn canonicalize_submission_timestamp(
    validation_now: OffsetDateTime,
    issued_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> ServiceResult<OffsetDateTime> {
    let durable_now = canonicalize_utc_second(validation_now, "now")?;
    if durable_now >= issued_at {
        return Ok(durable_now);
    }

    // A fractional issue instant can be later than the truncated second even
    // though validation_now is already within the quote's validity window.
    // Carry the durable timestamp to the next whole second while retaining
    // the full-precision boundary decision above.
    let next_second =
        durable_now
            .checked_add(time::Duration::seconds(1))
            .ok_or(ServiceError::Availability(
                AvailabilityError::DateTimeOverflow,
            ))?;
    if next_second >= expires_at {
        return Err(ServiceError::Store(StoreError::Conflict {
            resource: "appointment quote",
        }));
    }
    Ok(next_second)
}

#[cfg(test)]
mod tests {
    use super::{ConfirmedPreparedRequest, ExplicitConfirmation, PaService, ServiceError};
    use crate::pa::availability::{
        AvailabilityError, AvailabilityPolicy, BusyInterval, default_working_windows,
    };
    use crate::pa::domain::{
        AppointmentKind, AppointmentSlot, CallerIdentity, ConfirmedEmail, IdempotencyKey, Quote,
        QuoteId,
    };
    use crate::pa::fakes::{FakeControl, FakeGoogleCalendar, FakeOperation, FakeOutlookCalendar};
    use crate::pa::providers::{
        CalendarAttendee, CalendarChange, CalendarEvent, GoogleCalendarProvider,
        GoogleProposalDraft, MailAddress, ProviderError, ProviderSession, RetryAfter, TimeRange,
    };
    use crate::pa::store::{
        AuditEntityType, AuditEventType, MessageProvider, MessageSummary, PaStore, StoreError,
    };
    use chrono::{DateTime, Duration as ChronoDuration, Utc};
    use time::{
        Date, Duration, OffsetDateTime, Time, UtcOffset, format_description::well_known::Rfc3339,
    };

    #[test]
    fn flat_pa_module_exports_service_facade() {
        fn accepts_service(_: &crate::pa::PaService<'_>) {}
        let _ = accepts_service;
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-08-31T08:00:00Z", &Rfc3339).expect("valid now")
    }

    fn session() -> ProviderSession {
        ProviderSession::new("calendar-account", "calendar-access-token", None)
            .expect("valid session")
    }

    fn control(now: OffsetDateTime) -> FakeControl {
        FakeControl::new(
            DateTime::<Utc>::from_timestamp(now.unix_timestamp(), now.nanosecond())
                .expect("chrono now"),
        )
    }

    fn fixture(
        outlook_control: &FakeControl,
        google_control: &FakeControl,
        outlook_busy: Vec<BusyInterval>,
        google_busy: Vec<BusyInterval>,
    ) -> (
        PaStore,
        FakeOutlookCalendar,
        FakeGoogleCalendar,
        ProviderSession,
        ProviderSession,
    ) {
        (
            PaStore::open_in_memory(b"service-test-key").expect("store"),
            FakeOutlookCalendar::new(outlook_control, outlook_busy, Vec::<CalendarChange>::new()),
            FakeGoogleCalendar::new(google_control, google_busy, Vec::<CalendarChange>::new()),
            session(),
            session(),
        )
    }

    fn appointment_quote_row_count(store: &PaStore) -> i64 {
        store
            .connection()
            .query_row("SELECT count(*) FROM appointment_quotes", [], |row| {
                row.get(0)
            })
            .expect("quote row count")
    }

    fn assert_no_calendar_operations(control: &FakeControl) {
        for operation in [
            FakeOperation::CalendarBusy,
            FakeOperation::CalendarSync,
            FakeOperation::CalendarOwnerCreate,
            FakeOperation::CalendarOwnerFind,
            FakeOperation::CalendarProposalCreate,
            FakeOperation::CalendarProposalFind,
            FakeOperation::CalendarPromote,
            FakeOperation::CalendarDelete,
        ] {
            assert_eq!(
                control.invocation_count(operation).expect("calendar count"),
                0,
                "message recording called {operation:?}"
            );
        }
    }

    #[tokio::test]
    async fn search_slots_persists_and_returns_the_frozen_quote() {
        let now = now();
        let control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&control, &control, Vec::new(), Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
        );

        let result = service
            .search_slots(AppointmentKind::Callback, now, 2)
            .await
            .expect("search");

        assert_eq!(result.appointment_kind(), AppointmentKind::Callback);
        assert_eq!(result.timezone(), "UTC");
        assert_eq!(result.offered_slots().len(), 2);
        let stored = store
            .load_appointment_quote_by_id(result.quote().id())
            .expect("stored quote");
        assert_eq!(stored.offered_slots(), result.offered_slots());
    }

    #[tokio::test]
    async fn invalid_limits_do_not_call_either_provider_or_write_a_quote() {
        for limit in [0, 101] {
            let now = now();
            let outlook_control = control(now);
            let google_control = control(now);
            let (store, outlook, google, outlook_session, google_session) =
                fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
            let service = PaService::new(
                &store,
                &outlook,
                &outlook_session,
                &google,
                &google_session,
                &AvailabilityPolicy::default(),
            );

            let error = service
                .search_slots(AppointmentKind::Callback, now, limit)
                .await
                .expect_err("limit must be rejected");

            assert!(matches!(
                error,
                ServiceError::InvalidInput { field: "limit" }
            ));
            assert_eq!(
                outlook_control
                    .invocation_count(FakeOperation::CalendarBusy)
                    .expect("outlook count"),
                0
            );
            assert_eq!(
                google_control
                    .invocation_count(FakeOperation::CalendarBusy)
                    .expect("google count"),
                0
            );
            assert_eq!(appointment_quote_row_count(&store), 0);
        }
    }

    #[tokio::test]
    async fn outlook_failures_stop_before_google_and_write_no_quote() {
        let failures = [
            ProviderError::TokenExpired,
            ProviderError::throttled(
                RetryAfter::new(ChronoDuration::seconds(1)).expect("positive retry delay"),
            ),
            ProviderError::Unavailable,
        ];

        for failure in failures {
            let now = now();
            let outlook_control = control(now);
            let google_control = control(now);
            outlook_control
                .queue_failure(FakeOperation::CalendarBusy, failure)
                .expect("queue outlook failure");
            let (store, outlook, google, outlook_session, google_session) =
                fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
            let service = PaService::new(
                &store,
                &outlook,
                &outlook_session,
                &google,
                &google_session,
                &AvailabilityPolicy::default(),
            );

            let error = service
                .search_slots(AppointmentKind::Callback, now, 1)
                .await
                .expect_err("outlook failure must stop search");

            assert!(matches!(error, ServiceError::OutlookCalendar(_)));
            assert_eq!(
                outlook_control
                    .invocation_count(FakeOperation::CalendarBusy)
                    .expect("outlook count"),
                1
            );
            assert_eq!(
                google_control
                    .invocation_count(FakeOperation::CalendarBusy)
                    .expect("google count"),
                0
            );
            assert_eq!(appointment_quote_row_count(&store), 0);
        }
    }

    #[tokio::test]
    async fn google_failures_follow_outlook_and_write_no_quote() {
        let throttled = ProviderError::throttled(
            RetryAfter::new(ChronoDuration::seconds(1)).expect("positive retry delay"),
        );
        for failure in [
            ProviderError::TokenExpired,
            throttled,
            ProviderError::Unavailable,
        ] {
            let now = now();
            let outlook_control = control(now);
            let google_control = control(now);
            google_control
                .queue_failure(FakeOperation::CalendarBusy, failure)
                .expect("queue google failure");
            let (store, outlook, google, outlook_session, google_session) =
                fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
            let service = PaService::new(
                &store,
                &outlook,
                &outlook_session,
                &google,
                &google_session,
                &AvailabilityPolicy::default(),
            );

            let error = service
                .search_slots(AppointmentKind::Callback, now, 1)
                .await
                .expect_err("google failure must fail search");

            assert!(matches!(error, ServiceError::GoogleCalendar(_)));
            assert_eq!(
                outlook_control
                    .invocation_count(FakeOperation::CalendarBusy)
                    .expect("outlook count"),
                1
            );
            assert_eq!(
                google_control
                    .invocation_count(FakeOperation::CalendarBusy)
                    .expect("google count"),
                1
            );
            assert_eq!(appointment_quote_row_count(&store), 0);
        }
    }

    #[tokio::test]
    async fn union_busy_intervals_skip_each_calendar_conflict_and_return_the_third_callback_start()
    {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let outlook_busy = BusyInterval::new(
            now + Duration::hours(1),
            now + Duration::hours(1) + Duration::minutes(15),
        )
        .expect("outlook busy interval");
        let google_busy = BusyInterval::new(
            now + Duration::hours(1) + Duration::minutes(15),
            now + Duration::hours(1) + Duration::minutes(30),
        )
        .expect("google busy interval");
        let (store, outlook, google, outlook_session, google_session) = fixture(
            &outlook_control,
            &google_control,
            vec![outlook_busy],
            vec![google_busy],
        );
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
        );

        let result = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("union availability");

        assert_eq!(
            result.offered_slots()[0].starts_at(),
            now + Duration::hours(1) + Duration::minutes(30)
        );
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            1
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            1
        );
    }

    #[tokio::test]
    async fn either_calendar_busy_interval_blocks_the_conflicting_start() {
        let now = now();
        let first_start = now + Duration::hours(1);
        let busy = BusyInterval::new(first_start, first_start + Duration::minutes(15))
            .expect("busy interval");

        for (outlook_busy, google_busy) in [(vec![busy], Vec::new()), (Vec::new(), vec![busy])] {
            let outlook_control = control(now);
            let google_control = control(now);
            let (store, outlook, google, outlook_session, google_session) =
                fixture(&outlook_control, &google_control, outlook_busy, google_busy);
            let service = PaService::new(
                &store,
                &outlook,
                &outlook_session,
                &google,
                &google_session,
                &AvailabilityPolicy::default(),
            );

            let result = service
                .search_slots(AppointmentKind::Callback, now, 1)
                .await
                .expect("availability after one-calendar conflict");

            assert_eq!(
                result.offered_slots()[0].starts_at(),
                first_start + Duration::minutes(15)
            );
        }
    }

    #[tokio::test]
    async fn empty_calendars_return_ordered_literal_starts_up_to_requested_limit() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
        );

        let result = service
            .search_slots(AppointmentKind::Callback, now, 3)
            .await
            .expect("empty calendars");

        assert_eq!(
            result
                .offered_slots()
                .iter()
                .map(|slot| slot.starts_at())
                .collect::<Vec<_>>(),
            vec![
                OffsetDateTime::parse("2026-08-31T09:00:00Z", &Rfc3339).expect("start"),
                OffsetDateTime::parse("2026-08-31T09:15:00Z", &Rfc3339).expect("start"),
                OffsetDateTime::parse("2026-08-31T09:30:00Z", &Rfc3339).expect("start"),
            ]
        );
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            1
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            1
        );
    }

    #[tokio::test]
    async fn busy_intervals_outside_provider_range_do_not_change_slots() {
        let now = now();
        let policy = AvailabilityPolicy::new(
            "UTC",
            default_working_windows(),
            Duration::hours(1),
            Duration::hours(2),
            Duration::ZERO,
        )
        .expect("short horizon policy");
        let outlook_control = control(now);
        let google_control = control(now);
        let before_now = BusyInterval::new(now - Duration::minutes(30), now).expect("before");
        let at_horizon = BusyInterval::new(
            now + Duration::hours(2),
            now + Duration::hours(2) + Duration::minutes(15),
        )
        .expect("at horizon");
        let (store, outlook, google, outlook_session, google_session) = fixture(
            &outlook_control,
            &google_control,
            vec![before_now],
            vec![at_horizon],
        );
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &policy,
        );

        let result = service
            .search_slots(AppointmentKind::Callback, now, 3)
            .await
            .expect("out-of-range busy intervals");

        assert_eq!(
            result
                .offered_slots()
                .iter()
                .map(|slot| slot.starts_at())
                .collect::<Vec<_>>(),
            vec![
                OffsetDateTime::parse("2026-08-31T09:00:00Z", &Rfc3339).expect("start"),
                OffsetDateTime::parse("2026-08-31T09:15:00Z", &Rfc3339).expect("start"),
                OffsetDateTime::parse("2026-08-31T09:30:00Z", &Rfc3339).expect("start"),
            ]
        );
    }

    #[tokio::test]
    async fn meeting_buffer_busy_interval_before_now_blocks_buffered_candidate() {
        let now = now();
        let policy = AvailabilityPolicy::new(
            "UTC",
            default_working_windows(),
            Duration::ZERO,
            Duration::hours(2),
            Duration::minutes(30),
        )
        .expect("buffered policy");
        let outlook_control = control(now);
        let google_control = control(now);
        let busy = BusyInterval::new(now - Duration::minutes(15), now - Duration::minutes(5))
            .expect("busy interval before now");
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, vec![busy], Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &policy,
        );

        let result = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("buffered availability");

        assert_eq!(
            result.offered_slots()[0].starts_at(),
            now + Duration::minutes(30)
        );
    }

    #[tokio::test]
    async fn meeting_buffer_busy_interval_after_horizon_blocks_buffered_candidate() {
        let now = OffsetDateTime::parse("2026-08-31T09:45:00Z", &Rfc3339).expect("now");
        let policy = AvailabilityPolicy::new(
            "UTC",
            default_working_windows(),
            Duration::ZERO,
            Duration::minutes(15),
            Duration::minutes(30),
        )
        .expect("buffered policy");
        let outlook_control = control(now);
        let google_control = control(now);
        let horizon_end = now + Duration::minutes(15);
        let busy = BusyInterval::new(
            horizon_end + Duration::minutes(15),
            horizon_end + Duration::minutes(20),
        )
        .expect("busy interval after horizon");
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, vec![busy], Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &policy,
        );

        let error = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect_err("buffered candidate must be blocked");

        assert!(matches!(error, ServiceError::NoAvailability));
    }

    #[tokio::test]
    async fn file_backed_search_quote_survives_service_and_store_reopen() {
        let now = now();
        let path = std::env::temp_dir().join(format!(
            "agent_voice_pa_service_{}_{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let expected_slots = [
            OffsetDateTime::parse("2026-08-31T22:00:00Z", &Rfc3339).expect("start"),
            OffsetDateTime::parse("2026-08-31T22:15:00Z", &Rfc3339).expect("start"),
        ];
        let (quote_id, expected_quote) = {
            let store = PaStore::open(&path, b"service-test-key").expect("file store");
            let outlook_control = control(now);
            let google_control = control(now);
            let outlook = FakeOutlookCalendar::new(
                outlook_control,
                Vec::<BusyInterval>::new(),
                Vec::<CalendarChange>::new(),
            );
            let google = FakeGoogleCalendar::new(
                google_control,
                Vec::<BusyInterval>::new(),
                Vec::<CalendarChange>::new(),
            );
            let outlook_session = session();
            let google_session = session();
            let policy = AvailabilityPolicy::for_timezone("Australia/Sydney").expect("policy");
            let service = PaService::new(
                &store,
                &outlook,
                &outlook_session,
                &google,
                &google_session,
                &policy,
            );

            let result = service
                .search_slots(AppointmentKind::Meeting, now, 2)
                .await
                .expect("file-backed search");
            assert_eq!(result.offered_slots()[0].starts_at(), expected_slots[0]);
            assert_eq!(result.offered_slots()[1].starts_at(), expected_slots[1]);
            assert_eq!(result.appointment_kind(), AppointmentKind::Meeting);
            assert_eq!(result.timezone(), "Australia/Sydney");
            assert_eq!(result.quote().expires_at(), now + Quote::VALID_FOR);
            (result.quote().id(), result.quote().clone())
        };

        let reopened = PaStore::open(&path, b"service-test-key").expect("reopen file store");
        let stored = reopened
            .load_appointment_quote_by_id(quote_id)
            .expect("load reopened quote");
        assert_eq!(stored.quote(), &expected_quote);
        assert_eq!(stored.appointment_kind(), AppointmentKind::Meeting);
        assert_eq!(stored.timezone(), "Australia/Sydney");
        assert_eq!(
            stored
                .offered_slots()
                .iter()
                .map(|slot| slot.starts_at())
                .collect::<Vec<_>>(),
            expected_slots
        );
        assert_eq!(
            stored.quote().expires_at() - stored.quote().issued_at(),
            Duration::minutes(5)
        );
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[tokio::test]
    async fn store_failure_after_both_reads_returns_closed_store_error() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        store
            .connection()
            .execute("DROP TABLE appointment_quote_slots", [])
            .expect("drop quote slots table");
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
        );

        let error = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect_err("store failure must be surfaced");

        assert!(matches!(error, ServiceError::Store(StoreError::Sqlite(_))));
        assert_eq!(appointment_quote_row_count(&store), 0);
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            1
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            1
        );
        assert!(!matches!(error, ServiceError::NoAvailability));
    }

    #[tokio::test]
    async fn no_available_slots_does_not_write_an_empty_quote() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let busy = BusyInterval::new(now, now + Duration::days(61)).expect("busy interval");
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, vec![busy], Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
        );

        let error = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect_err("fully busy horizon has no availability");

        assert!(matches!(error, ServiceError::NoAvailability));
        assert_eq!(appointment_quote_row_count(&store), 0);
    }

    #[tokio::test]
    async fn prepare_request_returns_spoken_recap_for_frozen_slot_without_provider_calls() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let policy = AvailabilityPolicy::for_timezone("Australia/Sydney").expect("policy");
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &policy,
        );
        let quote = Quote::new(now);
        let slot_start =
            OffsetDateTime::parse("2026-08-31T22:00:00Z", &Rfc3339).expect("slot start");
        let slot =
            AppointmentSlot::new(slot_start, slot_start + Duration::minutes(30)).expect("slot");
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Meeting,
                "Australia/Sydney",
                &[slot],
            )
            .expect("quote");
        let caller = CallerIdentity::new(
            "Ada Lovelace",
            ConfirmedEmail::confirm("ada@example.test").expect("email"),
        )
        .expect("caller");

        let request = service
            .prepare_request(
                quote.id(),
                0,
                caller,
                AppointmentKind::Meeting,
                None,
                "voice:call-1",
                IdempotencyKey::new("appointment:call-1").expect("key"),
                now,
            )
            .expect("prepare");

        assert_eq!(request.draft_id(), 1);
        assert_eq!(request.quote_id(), quote.id());
        assert_eq!(request.kind(), AppointmentKind::Meeting);
        assert_eq!(request.starts_at(), slot.starts_at());
        assert_eq!(request.ends_at(), slot.ends_at());
        assert_eq!(request.timezone(), "Australia/Sydney");
        assert!(request.requester_included());
        assert_eq!(
            request.recap(),
            "Meeting for Ada Lovelace <ada@example.test> on 2026-09-01 at 08:00 (Australia/Sydney); requester included."
        );
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
    }

    #[tokio::test]
    async fn prepare_request_applies_inclusion_defaults_and_explicit_override() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
        );
        let caller = || {
            CallerIdentity::new(
                "Ada Lovelace",
                ConfirmedEmail::confirm("ada@example.test").expect("email"),
            )
            .expect("caller")
        };

        let callback_quote = Quote::new(now);
        let callback_slot = AppointmentSlot::new(
            now + Duration::hours(1),
            now + Duration::hours(1) + Duration::minutes(15),
        )
        .expect("callback slot");
        store
            .save_appointment_quote(
                &callback_quote,
                AppointmentKind::Callback,
                "UTC",
                &[callback_slot],
            )
            .expect("callback quote");
        let callback_request = service
            .prepare_request(
                callback_quote.id(),
                0,
                caller(),
                AppointmentKind::Callback,
                None,
                "voice:callback-default",
                IdempotencyKey::new("appointment:callback-default").expect("key"),
                now,
            )
            .expect("callback prepare");
        assert!(!callback_request.requester_included());
        assert!(callback_request.recap().contains("requester not included"));

        let meeting_quote = Quote::new(now);
        let meeting_slot = AppointmentSlot::new(
            now + Duration::hours(2),
            now + Duration::hours(2) + Duration::minutes(30),
        )
        .expect("meeting slot");
        store
            .save_appointment_quote(
                &meeting_quote,
                AppointmentKind::Meeting,
                "UTC",
                &[meeting_slot],
            )
            .expect("meeting quote");
        let meeting_request = service
            .prepare_request(
                meeting_quote.id(),
                0,
                caller(),
                AppointmentKind::Meeting,
                Some(false),
                "voice:meeting-override",
                IdempotencyKey::new("appointment:meeting-override").expect("key"),
                now,
            )
            .expect("meeting prepare");
        assert!(!meeting_request.requester_included());
        assert!(meeting_request.recap().contains("requester not included"));
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
    }

    #[tokio::test]
    async fn prepare_request_retries_exactly_after_expiry_and_conflicts_on_changes() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
        );
        let quote = Quote::new(now);
        let first_slot = AppointmentSlot::new(
            now + Duration::hours(1),
            now + Duration::hours(1) + Duration::minutes(30),
        )
        .expect("first slot");
        let second_slot = AppointmentSlot::new(
            now + Duration::hours(2),
            now + Duration::hours(2) + Duration::minutes(30),
        )
        .expect("second slot");
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Meeting,
                "UTC",
                &[first_slot, second_slot],
            )
            .expect("quote");
        let caller = CallerIdentity::new(
            "Ada Lovelace",
            ConfirmedEmail::confirm("ada@example.test").expect("email"),
        )
        .expect("caller");
        let key = IdempotencyKey::new("appointment:retry").expect("key");
        let first = service
            .prepare_request(
                quote.id(),
                0,
                caller.clone(),
                AppointmentKind::Meeting,
                None,
                "voice:retry",
                key.clone(),
                now,
            )
            .expect("first prepare");
        let retry = service
            .prepare_request(
                quote.id(),
                0,
                caller.clone(),
                AppointmentKind::Meeting,
                None,
                "voice:retry",
                key.clone(),
                quote.expires_at(),
            )
            .expect("retry after expiry");
        assert_eq!(retry, first);

        let changed_caller = CallerIdentity::new(
            "Grace Hopper",
            ConfirmedEmail::confirm("grace@example.test").expect("email"),
        )
        .expect("caller");
        for (slot_index, caller, inclusion, source_id, key) in [
            (0, changed_caller, None, "voice:retry", key.clone()),
            (0, caller.clone(), Some(false), "voice:retry", key.clone()),
            (0, caller.clone(), None, "voice:changed", key.clone()),
            (
                0,
                caller.clone(),
                None,
                "voice:retry",
                IdempotencyKey::new("appointment:changed").expect("key"),
            ),
            (1, caller.clone(), None, "voice:retry", key.clone()),
        ] {
            let error = service
                .prepare_request(
                    quote.id(),
                    slot_index,
                    caller,
                    AppointmentKind::Meeting,
                    inclusion,
                    source_id,
                    key,
                    now,
                )
                .expect_err("changed immutable input must conflict");
            assert!(matches!(
                error,
                ServiceError::Store(StoreError::Conflict { .. })
            ));
        }

        let error = service
            .prepare_request(
                quote.id(),
                0,
                caller,
                AppointmentKind::Callback,
                None,
                "voice:retry",
                IdempotencyKey::new("appointment:retry").expect("key"),
                now,
            )
            .expect_err("changed kind must conflict");
        assert!(matches!(
            error,
            ServiceError::Store(StoreError::Conflict { .. })
        ));
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
    }

    #[tokio::test]
    async fn prepare_request_rejects_unknown_invalid_and_temporally_invalid_quotes() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let policy = AvailabilityPolicy::for_timezone("UTC").expect("policy");
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &policy,
        );
        let caller = CallerIdentity::new(
            "Ada Lovelace",
            ConfirmedEmail::confirm("ada@example.test").expect("email"),
        )
        .expect("caller");
        let key = || IdempotencyKey::new("appointment:invalid").expect("key");
        let unknown = service
            .prepare_request(
                QuoteId::new(),
                0,
                caller.clone(),
                AppointmentKind::Callback,
                None,
                "voice:unknown",
                key(),
                now,
            )
            .expect_err("unknown quote must fail closed");
        assert!(matches!(
            unknown,
            ServiceError::Store(StoreError::NotFound { .. })
        ));

        let quote = Quote::with_id(QuoteId::new(), now);
        let slot = AppointmentSlot::new(
            now + Duration::hours(1),
            now + Duration::hours(1) + Duration::minutes(15),
        )
        .expect("slot");
        store
            .save_appointment_quote(&quote, AppointmentKind::Callback, "UTC", &[slot])
            .expect("quote");
        let invalid_slot = service
            .prepare_request(
                quote.id(),
                1,
                caller.clone(),
                AppointmentKind::Callback,
                None,
                "voice:invalid-slot",
                key(),
                now,
            )
            .expect_err("unknown slot must fail closed");
        assert!(matches!(
            invalid_slot,
            ServiceError::Store(StoreError::InvalidInput {
                field: "slot_index"
            })
        ));
        let expired = service
            .prepare_request(
                quote.id(),
                0,
                caller.clone(),
                AppointmentKind::Callback,
                None,
                "voice:expired",
                key(),
                quote.expires_at(),
            )
            .expect_err("expired quote must fail closed");
        assert!(matches!(
            expired,
            ServiceError::Store(StoreError::AppointmentQuoteExpired)
        ));

        let future_quote = Quote::with_id(QuoteId::new(), now + Duration::hours(1));
        let future_slot = AppointmentSlot::new(
            future_quote.issued_at() + Duration::hours(1),
            future_quote.issued_at() + Duration::hours(1) + Duration::minutes(15),
        )
        .expect("future slot");
        store
            .save_appointment_quote(
                &future_quote,
                AppointmentKind::Callback,
                "UTC",
                &[future_slot],
            )
            .expect("future quote");
        let not_yet_valid = service
            .prepare_request(
                future_quote.id(),
                0,
                caller,
                AppointmentKind::Callback,
                None,
                "voice:not-yet-valid",
                key(),
                now,
            )
            .expect_err("future quote must fail closed");
        assert!(matches!(
            not_yet_valid,
            ServiceError::Store(StoreError::AppointmentQuoteNotYetValid)
        ));
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
    }

    #[tokio::test]
    async fn confirmed_submission_creates_one_owner_only_pending_proposal_and_outbox_rows() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let owner = MailAddress::new("owner@example.test").expect("owner");
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            owner,
        );
        let caller = CallerIdentity::new(
            "Ada Lovelace",
            ConfirmedEmail::confirm("ada@example.test").expect("email"),
        )
        .expect("caller");
        let search = service
            .search_slots(AppointmentKind::Meeting, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                caller,
                AppointmentKind::Meeting,
                None,
                "voice:submit-1",
                IdempotencyKey::new("appointment:submit-1").expect("key"),
                now,
            )
            .expect("prepare");

        let submitted = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect("submit");

        assert_eq!(submitted.state(), crate::pa::domain::ProposalState::Pending);
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM event_mappings", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("mapping count"),
            1
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM notification_outbox", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("notification count"),
            2
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM audit_events", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("audit count"),
            4
        );
    }

    #[tokio::test]
    async fn callback_submission_notifies_only_the_owner() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:callback-submit",
                IdempotencyKey::new("appointment:callback-submit").expect("key"),
                now,
            )
            .expect("prepare");

        let result = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect("submit");

        assert!(result.is_pending());
        assert_eq!(result.requester_notification_id(), None);
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 1);
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 3);
    }

    #[tokio::test]
    async fn newly_busy_slot_fails_before_local_proposal_creation() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let owner = MailAddress::new("owner@example.test").expect("owner");
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            owner.clone(),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:busy-submit",
                IdempotencyKey::new("appointment:busy-submit").expect("key"),
                now,
            )
            .expect("prepare");
        let range = TimeRange::new(
            super::to_chrono_utc(prepared.starts_at()).expect("start"),
            super::to_chrono_utc(prepared.ends_at()).expect("end"),
        )
        .expect("range");
        let occupied = GoogleProposalDraft::from_owner(
            "unrelated-proposal",
            super::PENDING_PROPOSAL_TITLE,
            range,
            prepared.timezone(),
            CalendarAttendee::needs_action(owner),
        )
        .expect("occupied draft");
        google
            .create_proposal(&google_session, &occupied)
            .await
            .expect("occupy slot");

        let error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect_err("busy slot must fail closed");
        assert!(matches!(error, ServiceError::NoAvailability));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("proposal count"),
            0
        );
        assert!(
            store
                .list_audit_events(None, 10)
                .expect("audits")
                .is_empty()
        );
        assert!(
            store
                .list_pending_notifications()
                .expect("outbox")
                .is_empty()
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
    }

    #[tokio::test]
    async fn submission_recheck_enforces_pre_and_post_buffer_but_zero_buffer_is_unchanged() {
        for (name, pre_buffer, buffer, expected_blocked) in [
            ("pre", true, Duration::minutes(15), true),
            ("post", false, Duration::minutes(15), true),
            ("zero", true, Duration::ZERO, false),
        ] {
            let now = now();
            let outlook_control = control(now);
            let google_control = control(now);
            let (store, outlook, google, outlook_session, google_session) =
                fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
            let owner = MailAddress::new("owner@example.test").expect("owner");
            let policy = AvailabilityPolicy::new(
                "UTC",
                default_working_windows(),
                Duration::ZERO,
                Duration::hours(2),
                buffer,
            )
            .expect("policy");
            let service = PaService::with_owner(
                &store,
                &outlook,
                &outlook_session,
                &google,
                &google_session,
                &policy,
                owner.clone(),
            );
            let search = service
                .search_slots(AppointmentKind::Callback, now, 1)
                .await
                .expect("search");
            let prepared = service
                .prepare_request(
                    search.quote().id(),
                    0,
                    CallerIdentity::new(
                        "Ada Lovelace",
                        ConfirmedEmail::confirm("ada@example.test").expect("email"),
                    )
                    .expect("caller"),
                    AppointmentKind::Callback,
                    None,
                    format!("voice:buffer-{name}"),
                    IdempotencyKey::new(format!("appointment:buffer-{name}")).expect("key"),
                    now,
                )
                .expect("prepare");
            let (busy_start, busy_end) = if pre_buffer {
                (
                    prepared.starts_at() - Duration::minutes(10),
                    prepared.starts_at() - Duration::minutes(5),
                )
            } else {
                (
                    prepared.ends_at() + Duration::minutes(5),
                    prepared.ends_at() + Duration::minutes(10),
                )
            };
            let occupied_range = TimeRange::new(
                super::to_chrono_utc(busy_start).expect("busy start"),
                super::to_chrono_utc(busy_end).expect("busy end"),
            )
            .expect("busy range");
            let occupied = GoogleProposalDraft::from_owner(
                format!("unrelated-buffer-{name}"),
                super::PENDING_PROPOSAL_TITLE,
                occupied_range,
                prepared.timezone(),
                CalendarAttendee::needs_action(owner),
            )
            .expect("occupied draft");
            google
                .create_proposal(&google_session, &occupied)
                .await
                .expect("occupy buffer only");

            let submitted = service
                .submit_request(
                    ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                        .expect("confirmation"),
                    now,
                )
                .await;
            if expected_blocked {
                assert!(matches!(submitted, Err(ServiceError::NoAvailability)));
                assert_eq!(store.list_pending_notifications().expect("outbox").len(), 0);
                assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 0);
                assert_eq!(
                    google_control
                        .invocation_count(FakeOperation::CalendarProposalCreate)
                        .expect("create count"),
                    1
                );
            } else {
                assert!(submitted.expect("zero-buffer submission").is_pending());
                assert_eq!(
                    google_control
                        .invocation_count(FakeOperation::CalendarProposalCreate)
                        .expect("create count"),
                    2
                );
            }
        }
    }

    #[tokio::test]
    async fn submission_buffer_expansion_overflow_fails_before_provider_calls() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let policy = AvailabilityPolicy::new(
            "UTC",
            default_working_windows(),
            Duration::ZERO,
            Duration::hours(2),
            Duration::MAX,
        )
        .expect("policy");
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &policy,
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let starts_at = now + Duration::hours(1);
        let ends_at = starts_at
            .checked_add(AppointmentKind::Callback.duration())
            .expect("callback end");
        let quote = Quote::new(now);
        store
            .save_appointment_quote(
                &quote,
                AppointmentKind::Callback,
                "UTC",
                &[AppointmentSlot::new(starts_at, ends_at).expect("slot")],
            )
            .expect("quote");
        let prepared = service
            .prepare_request(
                quote.id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:buffer-overflow",
                IdempotencyKey::new("appointment:buffer-overflow").expect("key"),
                now,
            )
            .expect("prepare");

        let error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect_err("buffer expansion overflow must fail");
        assert!(matches!(
            error,
            ServiceError::Availability(AvailabilityError::DateTimeOverflow)
        ));
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalFind)
                .expect("find count"),
            0
        );
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook busy count"),
            0
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google busy count"),
            0
        );
    }

    #[tokio::test]
    async fn expired_prepared_request_fails_before_provider_reads() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:expired-submit",
                IdempotencyKey::new("appointment:expired-submit").expect("key"),
                now,
            )
            .expect("prepare");
        let expires_at = search.quote().expires_at();

        let error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                expires_at,
            )
            .await
            .expect_err("expired quote must fail");
        assert!(matches!(
            error,
            ServiceError::Store(StoreError::AppointmentQuoteExpired)
        ));
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook busy count"),
            1
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalFind)
                .expect("find count"),
            0
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM proposals", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("proposal count"),
            0
        );
    }

    #[tokio::test]
    async fn non_utc_durable_interval_fails_before_submission_side_effects_and_exact_retry_converges()
     {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:non-utc-submit",
                IdempotencyKey::new("appointment:non-utc-submit").expect("key"),
                now,
            )
            .expect("prepare");
        let non_utc_offset = UtcOffset::from_hms(10, 0, 0).expect("offset");
        let non_utc_start = prepared
            .starts_at()
            .to_offset(non_utc_offset)
            .format(&Rfc3339)
            .expect("start text");
        let non_utc_end = prepared
            .ends_at()
            .to_offset(non_utc_offset)
            .format(&Rfc3339)
            .expect("end text");
        store
            .connection()
            .execute(
                "UPDATE appointment_drafts SET starts_at = ?1, ends_at = ?2 WHERE id = ?3",
                (&non_utc_start, &non_utc_end, prepared.draft_id()),
            )
            .expect("corrupt durable offset");

        let error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared.clone(), ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect_err("non-UTC durable interval must fail closed");
        assert!(matches!(
            error,
            ServiceError::Store(StoreError::Conflict { .. })
        ));
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalFind)
                .expect("find count"),
            0
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            0
        );
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook busy count"),
            1
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google busy count"),
            1
        );
        for table in [
            "proposals",
            "event_mappings",
            "notification_outbox",
            "audit_events",
        ] {
            let count = store
                .connection()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("side-effect count");
            assert_eq!(count, 0, "{table} must remain unchanged");
        }

        let starts_at = prepared
            .starts_at()
            .to_offset(UtcOffset::UTC)
            .format(&Rfc3339)
            .expect("canonical start text");
        let ends_at = prepared
            .ends_at()
            .to_offset(UtcOffset::UTC)
            .format(&Rfc3339)
            .expect("canonical end text");
        store
            .connection()
            .execute(
                "UPDATE appointment_drafts SET starts_at = ?1, ends_at = ?2 WHERE id = ?3",
                (&starts_at, &ends_at, prepared.draft_id()),
            )
            .expect("restore durable interval");

        let first = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared.clone(), ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect("valid submission");
        let retry = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect("exact valid retry");
        assert_eq!(retry, first);
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 1);
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 3);
    }

    #[tokio::test]
    async fn exact_retry_repairs_a_missing_audit_without_provider_calls() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:retry-submit",
                IdempotencyKey::new("appointment:retry-submit").expect("key"),
                now,
            )
            .expect("prepare");
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER fail_submission_audit
                 BEFORE INSERT ON audit_events
                 BEGIN SELECT RAISE(ABORT, 'forced audit failure'); END;",
            )
            .expect("install audit failure");
        let first_error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared.clone(), ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect_err("audit tail failure must be returned");
        assert!(matches!(first_error, ServiceError::Store(_)));
        store
            .connection()
            .execute_batch("DROP TRIGGER fail_submission_audit")
            .expect("remove audit failure");
        google_control
            .set_failure(
                FakeOperation::CalendarProposalFind,
                ProviderError::Unavailable,
            )
            .expect("google find failure");
        google_control
            .set_failure(
                FakeOperation::CalendarProposalCreate,
                ProviderError::Unavailable,
            )
            .expect("google create failure");

        let retry = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now + Duration::minutes(1),
            )
            .await
            .expect("retry");
        assert!(retry.is_pending());
        assert_eq!(retry.requester_notification_id(), None);
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 1);
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 3);
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalFind)
                .expect("find count"),
            1
        );
    }

    #[tokio::test]
    async fn exact_retry_repairs_a_missing_mapping_without_duplicate_provider_create() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:mapping-retry",
                IdempotencyKey::new("appointment:mapping-retry").expect("key"),
                now,
            )
            .expect("prepare");
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER fail_submission_mapping
                 BEFORE INSERT ON event_mappings
                 BEGIN SELECT RAISE(ABORT, 'forced mapping failure'); END;",
            )
            .expect("install mapping failure");

        let first_error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared.clone(), ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect_err("mapping tail failure must be returned");
        assert!(matches!(first_error, ServiceError::Store(_)));
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM event_mappings", [], |row| row
                    .get::<_, i64>(0))
                .expect("mapping count"),
            0
        );
        store
            .connection()
            .execute_batch("DROP TRIGGER fail_submission_mapping")
            .expect("remove mapping failure");
        google_control
            .set_failure(
                FakeOperation::CalendarProposalCreate,
                ProviderError::Unavailable,
            )
            .expect("google create failure");

        let retry = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now + Duration::minutes(1),
            )
            .await
            .expect("retry repairs mapping");
        assert!(retry.is_pending());
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 1);
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 3);
    }

    #[tokio::test]
    async fn exact_retry_repairs_a_missing_notification_without_provider_calls() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:notification-retry",
                IdempotencyKey::new("appointment:notification-retry").expect("key"),
                now,
            )
            .expect("prepare");
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER fail_submission_notification
                 BEFORE INSERT ON notification_outbox
                 BEGIN SELECT RAISE(ABORT, 'forced notification failure'); END;",
            )
            .expect("install notification failure");

        let first_error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared.clone(), ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect_err("notification tail failure must be returned");
        assert!(matches!(first_error, ServiceError::Store(_)));
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 0);
        store
            .connection()
            .execute_batch("DROP TRIGGER fail_submission_notification")
            .expect("remove notification failure");
        for operation in [
            FakeOperation::CalendarBusy,
            FakeOperation::CalendarProposalFind,
            FakeOperation::CalendarProposalCreate,
        ] {
            google_control
                .set_failure(operation, ProviderError::Unavailable)
                .expect("google failure");
        }
        outlook_control
            .set_failure(FakeOperation::CalendarBusy, ProviderError::Unavailable)
            .expect("outlook failure");

        let retry = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now + Duration::minutes(1),
            )
            .await
            .expect("retry repairs notification");
        assert!(retry.is_pending());
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 1);
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 3);
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
    }

    #[tokio::test]
    async fn owner_change_after_mapping_fails_closed_without_provider_calls_or_misrouting() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let original_owner = MailAddress::new("owner@example.test").expect("original owner");
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            original_owner,
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:owner-change-retry",
                IdempotencyKey::new("appointment:owner-change-retry").expect("key"),
                now,
            )
            .expect("prepare");
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER fail_owner_notification
                 BEFORE INSERT ON notification_outbox
                 BEGIN SELECT RAISE(ABORT, 'forced notification failure'); END;",
            )
            .expect("install notification failure");
        let first_error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared.clone(), ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect_err("notification tail failure must be returned");
        assert!(matches!(first_error, ServiceError::Store(_)));
        store
            .connection()
            .execute_batch("DROP TRIGGER fail_owner_notification")
            .expect("remove notification failure");
        let changed_owner_service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("replacement@example.test").expect("replacement owner"),
        );
        let proposal_creates = google_control
            .invocation_count(FakeOperation::CalendarProposalCreate)
            .expect("create count");
        let proposal_finds = google_control
            .invocation_count(FakeOperation::CalendarProposalFind)
            .expect("find count");
        let outlook_busy = outlook_control
            .invocation_count(FakeOperation::CalendarBusy)
            .expect("outlook busy count");
        let google_busy = google_control
            .invocation_count(FakeOperation::CalendarBusy)
            .expect("google busy count");
        for operation in [
            FakeOperation::CalendarBusy,
            FakeOperation::CalendarProposalFind,
            FakeOperation::CalendarProposalCreate,
        ] {
            google_control
                .set_failure(operation, ProviderError::Unavailable)
                .expect("google failure");
        }
        outlook_control
            .set_failure(FakeOperation::CalendarBusy, ProviderError::Unavailable)
            .expect("outlook failure");

        let error = changed_owner_service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now + Duration::minutes(1),
            )
            .await
            .expect_err("owner change must fail before tail repair");
        assert!(matches!(
            error,
            ServiceError::Store(StoreError::Conflict { .. })
        ));
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 0);
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            proposal_creates
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalFind)
                .expect("find count"),
            proposal_finds
        );
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook busy count"),
            outlook_busy
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google busy count"),
            google_busy
        );
    }

    #[tokio::test]
    async fn nonzero_nanosecond_submission_retries_without_partial_state() {
        let now = now();
        let submission_now = now
            .replace_nanosecond(123_456_789)
            .expect("valid nanosecond");
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:nanosecond-submit",
                IdempotencyKey::new("appointment:nanosecond-submit").expect("key"),
                now,
            )
            .expect("prepare");

        let first = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared.clone(), ExplicitConfirmation::new())
                    .expect("confirmation"),
                submission_now,
            )
            .await
            .expect("nonzero nanoseconds must be canonicalized");
        assert!(first.is_pending());
        assert_eq!(first.requester_notification_id(), None);
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 1);
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 3);
        assert_eq!(
            store
                .load_appointment_quote_by_id(search.quote().id())
                .expect("quote")
                .consumed_at()
                .expect("consumed timestamp")
                .nanosecond(),
            0
        );

        for operation in [
            FakeOperation::CalendarBusy,
            FakeOperation::CalendarProposalFind,
            FakeOperation::CalendarProposalCreate,
        ] {
            let failure = if operation == FakeOperation::CalendarBusy {
                ProviderError::Unavailable
            } else {
                ProviderError::Conflict
            };
            google_control
                .set_failure(operation, failure)
                .expect("provider failure");
            if operation == FakeOperation::CalendarBusy {
                outlook_control
                    .set_failure(operation, ProviderError::Unavailable)
                    .expect("outlook failure");
            }
        }

        let retry = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                submission_now,
            )
            .await
            .expect("exact retry");
        assert_eq!(retry, first);
        assert_eq!(store.list_pending_notifications().expect("outbox").len(), 1);
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 3);
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
    }

    #[tokio::test]
    async fn fractional_issued_at_is_valid_at_exact_issue() {
        let now = now()
            .replace_nanosecond(123_456_789)
            .expect("valid fractional time");
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        assert_eq!(search.quote().issued_at().nanosecond(), 123_456_789);
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:fractional-issued",
                IdempotencyKey::new("appointment:fractional-issued").expect("key"),
                now,
            )
            .expect("prepare");

        let submitted = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect("exact fractional issue must be valid");
        assert!(submitted.is_pending());
        assert_eq!(
            store
                .load_appointment_quote_by_id(search.quote().id())
                .expect("quote")
                .consumed_at()
                .expect("consumed timestamp")
                .nanosecond(),
            0
        );
    }

    #[tokio::test]
    async fn fractional_expires_at_remains_exclusive() {
        let now = now()
            .replace_nanosecond(123_456_789)
            .expect("valid fractional time");
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            MailAddress::new("owner@example.test").expect("owner"),
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        assert_eq!(search.quote().expires_at().nanosecond(), 123_456_789);
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:fractional-expiry",
                IdempotencyKey::new("appointment:fractional-expiry").expect("key"),
                now,
            )
            .expect("prepare");

        let error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                search.quote().expires_at(),
            )
            .await
            .expect_err("fractional expiry must be exclusive");
        assert!(matches!(
            error,
            ServiceError::Store(StoreError::AppointmentQuoteExpired)
        ));
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarProposalFind)
                .expect("find count"),
            0
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

    #[tokio::test]
    async fn mismatched_provider_event_is_rejected_before_mapping() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let owner = MailAddress::new("owner@example.test").expect("owner");
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
            owner,
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("search");
        let prepared = service
            .prepare_request(
                search.quote().id(),
                0,
                CallerIdentity::new(
                    "Ada Lovelace",
                    ConfirmedEmail::confirm("ada@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:mismatch-submit",
                IdempotencyKey::new("appointment:mismatch-submit").expect("key"),
                now,
            )
            .expect("prepare");
        let range = TimeRange::new(
            super::to_chrono_utc(prepared.starts_at()).expect("start"),
            super::to_chrono_utc(prepared.ends_at()).expect("end"),
        )
        .expect("range");
        google
            .queue_create_response_override(
                CalendarEvent::new(
                    "mismatched-event",
                    super::proposal_operation_key(prepared.draft_id()),
                    "Wrong title",
                    range,
                    prepared.timezone(),
                    [CalendarAttendee::needs_action(
                        MailAddress::new("other@example.test").expect("other owner"),
                    )],
                    google_control.now(),
                )
                .expect("response"),
            )
            .expect("queue response");

        let error = service
            .submit_request(
                ConfirmedPreparedRequest::new(prepared, ExplicitConfirmation::new())
                    .expect("confirmation"),
                now,
            )
            .await
            .expect_err("mismatched event must fail");
        assert!(matches!(
            error,
            ServiceError::GoogleCalendar(ProviderError::Conflict)
        ));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM event_mappings", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("mapping count"),
            0
        );
        assert!(
            store
                .list_pending_notifications()
                .expect("outbox")
                .is_empty()
        );
        assert!(
            store
                .list_audit_events(None, 10)
                .expect("audits")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn horizon_and_quote_expiry_overflow_fail_before_provider_calls() {
        let horizon_overflow_now = Date::MAX.with_time(Time::MAX).assume_utc();
        let horizon_outlook_control = control(horizon_overflow_now);
        let horizon_google_control = control(horizon_overflow_now);
        let (
            horizon_store,
            horizon_outlook,
            horizon_google,
            horizon_outlook_session,
            horizon_google_session,
        ) = fixture(
            &horizon_outlook_control,
            &horizon_google_control,
            Vec::new(),
            Vec::new(),
        );
        let horizon_service = PaService::new(
            &horizon_store,
            &horizon_outlook,
            &horizon_outlook_session,
            &horizon_google,
            &horizon_google_session,
            &AvailabilityPolicy::default(),
        );

        let horizon_error = horizon_service
            .search_slots(AppointmentKind::Callback, horizon_overflow_now, 1)
            .await
            .expect_err("horizon overflow must fail");
        assert!(matches!(
            horizon_error,
            ServiceError::Availability(AvailabilityError::DateTimeOverflow)
        ));
        assert_eq!(
            horizon_outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            horizon_google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
        assert_eq!(appointment_quote_row_count(&horizon_store), 0);

        let quote_overflow_now = Date::MAX
            .with_time(Time::MAX)
            .assume_utc()
            .checked_sub(Duration::minutes(1))
            .expect("one minute before maximum");
        let quote_outlook_control = control(quote_overflow_now);
        let quote_google_control = control(quote_overflow_now);
        let (quote_store, quote_outlook, quote_google, quote_outlook_session, quote_google_session) =
            fixture(
                &quote_outlook_control,
                &quote_google_control,
                Vec::new(),
                Vec::new(),
            );
        let short_horizon_policy = AvailabilityPolicy::new(
            "UTC",
            default_working_windows(),
            Duration::ZERO,
            Duration::minutes(1),
            Duration::ZERO,
        )
        .expect("short policy");
        let quote_service = PaService::new(
            &quote_store,
            &quote_outlook,
            &quote_outlook_session,
            &quote_google,
            &quote_google_session,
            &short_horizon_policy,
        );

        let quote_error = quote_service
            .search_slots(AppointmentKind::Callback, quote_overflow_now, 1)
            .await
            .expect_err("quote expiry overflow must fail");
        assert!(matches!(
            quote_error,
            ServiceError::Availability(AvailabilityError::DateTimeOverflow)
        ));
        assert_eq!(
            quote_outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            quote_google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
        assert_eq!(appointment_quote_row_count(&quote_store), 0);
    }

    #[tokio::test]
    async fn meeting_buffer_range_overflow_fails_before_provider_calls() {
        let before_minimum = Date::MIN.with_time(Time::MIDNIGHT).assume_utc();
        let before_policy = AvailabilityPolicy::new(
            "UTC",
            default_working_windows(),
            Duration::ZERO,
            Duration::days(1),
            Duration::minutes(1),
        )
        .expect("before-boundary policy");
        let before_outlook_control = control(before_minimum);
        let before_google_control = control(before_minimum);
        let (
            before_store,
            before_outlook,
            before_google,
            before_outlook_session,
            before_google_session,
        ) = fixture(
            &before_outlook_control,
            &before_google_control,
            Vec::new(),
            Vec::new(),
        );
        let before_service = PaService::new(
            &before_store,
            &before_outlook,
            &before_outlook_session,
            &before_google,
            &before_google_session,
            &before_policy,
        );

        let before_error = before_service
            .search_slots(AppointmentKind::Callback, before_minimum, 1)
            .await
            .expect_err("provider start overflow must fail");

        assert!(matches!(
            before_error,
            ServiceError::Availability(AvailabilityError::DateTimeOverflow)
        ));
        assert_eq!(
            before_outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            before_google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
        assert_eq!(appointment_quote_row_count(&before_store), 0);

        let after_maximum = Date::MAX
            .with_time(Time::MAX)
            .assume_utc()
            .checked_sub(Duration::minutes(10))
            .expect("ten minutes before maximum");
        let after_policy = AvailabilityPolicy::new(
            "UTC",
            default_working_windows(),
            Duration::ZERO,
            Duration::minutes(5),
            Duration::minutes(10),
        )
        .expect("after-boundary policy");
        let after_outlook_control = control(after_maximum);
        let after_google_control = control(after_maximum);
        let (after_store, after_outlook, after_google, after_outlook_session, after_google_session) =
            fixture(
                &after_outlook_control,
                &after_google_control,
                Vec::new(),
                Vec::new(),
            );
        let after_service = PaService::new(
            &after_store,
            &after_outlook,
            &after_outlook_session,
            &after_google,
            &after_google_session,
            &after_policy,
        );

        let after_error = after_service
            .search_slots(AppointmentKind::Callback, after_maximum, 1)
            .await
            .expect_err("provider end overflow must fail");

        assert!(matches!(
            after_error,
            ServiceError::Availability(AvailabilityError::DateTimeOverflow)
        ));
        assert_eq!(
            after_outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            after_google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
        assert_eq!(appointment_quote_row_count(&after_store), 0);
    }

    #[tokio::test]
    async fn search_and_service_errors_redact_sensitive_values_from_display_and_debug() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let policy = AvailabilityPolicy::for_timezone("Australia/Sydney").expect("timezone");
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &policy,
        );
        let search = service
            .search_slots(AppointmentKind::Callback, now, 1)
            .await
            .expect("availability");
        let search_debug = format!("{search:?}");
        let quote_id = search.quote().id().to_string();
        let slot_timestamp = search.offered_slots()[0]
            .starts_at()
            .format(&Rfc3339)
            .expect("slot timestamp");
        assert!(search_debug.contains("AvailabilitySearch"));
        assert!(search_debug.contains("offered_slot_count: 1"));
        for secret in [
            quote_id.as_str(),
            slot_timestamp.as_str(),
            "Australia/Sydney",
            "calendar-account",
            "calendar-access-token",
        ] {
            assert!(
                !search_debug.contains(secret),
                "search debug redaction assertion failed"
            );
        }

        let errors = [
            ServiceError::Availability(AvailabilityError::InvalidTimezone {
                timezone: "timezone-sentinel".to_owned(),
            }),
            ServiceError::OutlookCalendar(ProviderError::TokenExpired),
            ServiceError::GoogleCalendar(ProviderError::throttled(
                RetryAfter::new(ChronoDuration::seconds(1)).expect("positive retry delay"),
            )),
            ServiceError::Store(StoreError::NotFound {
                resource: "quote-sentinel",
            }),
        ];
        for error in errors {
            let display = error.to_string();
            let debug = format!("{error:?}");
            for secret in [
                "timezone-sentinel",
                "quote-sentinel",
                "calendar-account",
                "calendar-access-token",
                "Australia/Sydney",
            ] {
                assert!(
                    !display.contains(secret),
                    "service display redaction assertion failed"
                );
                assert!(
                    !debug.contains(secret),
                    "service debug redaction assertion failed"
                );
            }
        }
    }

    #[tokio::test]
    async fn prepared_request_debug_redacts_caller_and_spoken_recap() {
        let now = now();
        let outlook_control = control(now);
        let google_control = control(now);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&outlook_control, &google_control, Vec::new(), Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::for_timezone("UTC").expect("policy"),
        );
        let quote = Quote::new(now);
        let slot = AppointmentSlot::new(
            now + Duration::hours(1),
            now + Duration::hours(1) + Duration::minutes(15),
        )
        .expect("slot");
        store
            .save_appointment_quote(&quote, AppointmentKind::Callback, "UTC", &[slot])
            .expect("quote");
        let request = service
            .prepare_request(
                quote.id(),
                0,
                CallerIdentity::new(
                    "Debug Sentinel",
                    ConfirmedEmail::confirm("debug-sentinel@example.test").expect("email"),
                )
                .expect("caller"),
                AppointmentKind::Callback,
                None,
                "voice:debug",
                IdempotencyKey::new("appointment:debug").expect("key"),
                now,
            )
            .expect("prepare");
        let debug = format!("{request:?}");
        for secret in [
            "Debug Sentinel",
            "debug-sentinel@example.test",
            "2026-08-31T09:00:00Z",
            "Callback for",
            "UTC",
        ] {
            assert!(
                !debug.contains(secret),
                "prepared request redaction assertion failed"
            );
        }
        assert!(debug.contains("PreparedRequest"));
        assert_eq!(
            outlook_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("outlook count"),
            0
        );
        assert_eq!(
            google_control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("google count"),
            0
        );
    }

    #[test]
    fn record_message_persists_one_voice_summary_and_owner_notification() {
        let canonical_received_at = now();
        let received_at = canonical_received_at
            .replace_nanosecond(123_456_789)
            .expect("valid timestamp");
        let control = control(received_at);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&control, &control, Vec::new(), Vec::new());
        let owner = crate::pa::providers::MailAddress::new("owner@example.com").expect("owner");
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
            owner,
        );

        let result = service
            .record_message(
                MessageSummary::new("Caller requested a callback").expect("summary"),
                "call-1",
                received_at,
            )
            .expect("recorded");

        assert_eq!(result.message_id(), 1);
        assert_eq!(result.notification_id(), 1);
        let message = store.load_message_by_id(1).expect("message");
        assert_eq!(message.provider(), crate::pa::store::MessageProvider::Voice);
        assert_eq!(message.source_id(), "voice:call-1");
        assert_eq!(message.summary().as_str(), "Caller requested a callback");
        assert_eq!(message.subject(), None);
        assert_eq!(message.sender(), None);
        assert_eq!(message.received_at(), canonical_received_at);
        let notifications = store.list_pending_notifications().expect("notifications");
        assert_eq!(notifications.len(), 1);
        assert_eq!(
            notifications[0].kind(),
            crate::pa::store::NotificationKind::CallSummary
        );
        assert_eq!(notifications[0].recipient().as_str(), "owner@example.com");
        assert_eq!(
            notifications[0].template_data().title(),
            Some("Caller requested a callback")
        );
        let audits = store.list_audit_events(None, 10).expect("audits");
        assert_eq!(audits.len(), 2);
        assert!(
            audits
                .iter()
                .all(|event| event.occurred_at() == canonical_received_at)
        );
        let debug = format!("{result:?}");
        assert_eq!(
            debug,
            "RecordedMessage { message_id: <redacted>, notification_id: <redacted> }"
        );
        assert_no_calendar_operations(&control);
    }

    #[test]
    fn record_message_exact_retry_is_stable_and_changed_inputs_conflict() {
        let canonical_received_at = now();
        let received_at = canonical_received_at
            .replace_nanosecond(987_654_321)
            .expect("valid timestamp");
        let control = control(received_at);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&control, &control, Vec::new(), Vec::new());
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
            crate::pa::providers::MailAddress::new("owner@example.com").expect("owner"),
        );
        let summary = MessageSummary::new("Stable call summary").expect("summary");
        let first = service
            .record_message(summary.clone(), "retry-call", received_at)
            .expect("first");
        assert_eq!(
            store
                .load_message_by_id(first.message_id())
                .expect("stored message")
                .source_id(),
            "voice:retry-call"
        );
        let retry = service
            .record_message(summary.clone(), "retry-call", canonical_received_at)
            .expect("exact source retry");
        assert_eq!(retry, first);
        assert!(matches!(
            service.record_message(
                MessageSummary::new("Changed call summary").expect("summary"),
                "retry-call",
                canonical_received_at,
            ),
            Err(ServiceError::Store(StoreError::Conflict {
                resource: "message"
            }))
        ));
        assert!(matches!(
            service.record_message(
                summary.clone(),
                "retry-call",
                canonical_received_at + Duration::seconds(1)
            ),
            Err(ServiceError::Store(StoreError::Conflict {
                resource: "message"
            }))
        ));
        assert!(
            service
                .record_message(summary, "retry-call-other", received_at)
                .is_ok()
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .expect("messages"),
            2
        );
        assert_eq!(
            store
                .list_pending_notifications()
                .expect("notifications")
                .len(),
            2
        );
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 4);
        assert_no_calendar_operations(&control);
    }

    #[test]
    fn record_message_owner_is_required_before_any_write_and_debug_is_redacted() {
        let received_at = now();
        let control = control(received_at);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&control, &control, Vec::new(), Vec::new());
        let service = PaService::new(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
        );
        let summary = MessageSummary::new("private caller summary").expect("summary");
        let error = service
            .record_message(summary, "private-call", received_at)
            .expect_err("owner is required");
        assert!(matches!(error, ServiceError::OwnerNotConfigured));
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .expect("messages"),
            0
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM notification_outbox", [], |row| row
                    .get::<_, i64>(0))
                .expect("notifications"),
            0
        );
        assert!(!format!("{error}").contains("private-call"));
        assert!(!format!("{error:?}").contains("private-call"));

        let owner_service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
            crate::pa::providers::MailAddress::new("private-owner@example.com").expect("owner"),
        );
        let result = owner_service
            .record_message(
                MessageSummary::new("private caller summary").expect("summary"),
                "private-call",
                received_at,
            )
            .expect("recorded");
        let debug = format!("{result:?}");
        for secret in [
            "private caller summary",
            "private-owner@example.com",
            "private-call",
            "message_id: 1",
            "notification_id: 1",
        ] {
            assert!(
                !debug.contains(secret),
                "recorded message redaction assertion failed"
            );
        }
        assert_no_calendar_operations(&control);
    }

    #[test]
    fn record_message_namespaces_source_and_submit_flow_identities() {
        let received_at = now();
        let control = control(received_at);
        let (store, outlook, google, outlook_session, google_session) =
            fixture(&control, &control, Vec::new(), Vec::new());
        store
            .record_message(
                "outlook-message-collision",
                "owner-1",
                MessageProvider::Outlook,
                "outlook-owner-1",
                MessageSummary::new("outlook summary").expect("summary"),
                None,
                None,
                received_at,
            )
            .expect("seed outlook message");
        store
            .record_message(
                "gmail-message-collision",
                "requester-1",
                MessageProvider::Gmail,
                "gmail-requester-1",
                MessageSummary::new("gmail summary").expect("summary"),
                None,
                None,
                received_at,
            )
            .expect("seed gmail message");
        for (key, entity_id) in [
            (
                "pa-audit-notification-enqueued-owner-1",
                "owner-notification-1",
            ),
            (
                "pa-audit-notification-enqueued-requester-1",
                "requester-notification-1",
            ),
        ] {
            store
                .append_audit_event(
                    key,
                    AuditEventType::NotificationEnqueued,
                    AuditEntityType::Notification,
                    entity_id,
                    received_at,
                )
                .expect("seed submit-flow audit identity");
        }
        let service = PaService::with_owner(
            &store,
            &outlook,
            &outlook_session,
            &google,
            &google_session,
            &AvailabilityPolicy::default(),
            crate::pa::providers::MailAddress::new("owner@example.com").expect("owner"),
        );

        let owner_result = service
            .record_message(
                MessageSummary::new("owner call summary").expect("summary"),
                "owner-1",
                received_at,
            )
            .expect("owner source must not collide");
        let requester_result = service
            .record_message(
                MessageSummary::new("requester call summary").expect("summary"),
                "requester-1",
                received_at,
            )
            .expect("requester source must not collide");
        assert_ne!(owner_result.message_id(), requester_result.message_id());
        assert_eq!(
            store
                .load_message_by_id(owner_result.message_id())
                .expect("owner message")
                .source_id(),
            "voice:owner-1"
        );
        assert_eq!(
            store
                .load_message_by_id(requester_result.message_id())
                .expect("requester message")
                .source_id(),
            "voice:requester-1"
        );
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .expect("messages"),
            4
        );
        assert_eq!(
            store
                .list_pending_notifications()
                .expect("notifications")
                .len(),
            2
        );
        assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 6);
        assert_no_calendar_operations(&control);
    }

    #[test]
    fn record_message_tail_failures_preserve_prefix_and_retry_to_complete_state() {
        struct Checkpoint {
            name: &'static str,
            table: &'static str,
            key: &'static str,
            notifications: usize,
            audits: usize,
        }
        let checkpoints = [
            Checkpoint {
                name: "notification",
                table: "notification_outbox",
                key: "pa-voice-call-summary-notification-tail-call",
                notifications: 0,
                audits: 0,
            },
            Checkpoint {
                name: "message audit",
                table: "audit_events",
                key: "pa-voice-message-recorded-audit-tail-call",
                notifications: 1,
                audits: 0,
            },
            Checkpoint {
                name: "notification audit",
                table: "audit_events",
                key: "pa-voice-notification-enqueued-audit-tail-call",
                notifications: 1,
                audits: 1,
            },
        ];
        for checkpoint in checkpoints {
            let received_at = now();
            let control = control(received_at);
            let (store, outlook, google, outlook_session, google_session) =
                fixture(&control, &control, Vec::new(), Vec::new());
            let service = PaService::with_owner(
                &store,
                &outlook,
                &outlook_session,
                &google,
                &google_session,
                &AvailabilityPolicy::default(),
                crate::pa::providers::MailAddress::new("owner@example.com").expect("owner"),
            );
            let trigger = format!("fail_record_message_{}", checkpoint.name.replace(' ', "_"));
            let condition = format!("NEW.idempotency_key = '{}'", checkpoint.key);
            store
                .connection()
                .execute_batch(&format!(
                    "CREATE TEMP TRIGGER {trigger} BEFORE INSERT ON {table} WHEN {condition} BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                    trigger = trigger,
                    table = checkpoint.table,
                    condition = condition,
                ))
                .expect("install trigger");
            let summary = MessageSummary::new("tail summary").expect("summary");
            assert!(matches!(
                service.record_message(summary.clone(), "tail-call", received_at),
                Err(ServiceError::Store(_))
            ));
            assert_eq!(
                store
                    .list_pending_notifications()
                    .expect("notifications")
                    .len(),
                checkpoint.notifications
            );
            assert_eq!(
                store.list_audit_events(None, 10).expect("audits").len(),
                checkpoint.audits
            );
            store
                .connection()
                .execute_batch(&format!("DROP TRIGGER {trigger}"))
                .expect("drop trigger");
            service
                .record_message(summary, "tail-call", received_at)
                .expect("retry");
            assert_eq!(
                store
                    .list_pending_notifications()
                    .expect("notifications")
                    .len(),
                1
            );
            assert_eq!(store.list_audit_events(None, 10).expect("audits").len(), 2);
            assert_eq!(
                store
                    .connection()
                    .query_row("SELECT count(*) FROM messages", [], |row| row
                        .get::<_, i64>(0))
                    .expect("messages"),
                1
            );
            assert_no_calendar_operations(&control);
        }
    }
}
