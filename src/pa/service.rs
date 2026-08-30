//! Application services for the personal-assistant availability workflow.
//!
//! This module contains the provider-free availability and appointment-
//! preparation boundaries. Submission, messages, and owner tasks are separate
//! service packages.

use std::fmt;

use chrono::{DateTime, Utc};
use time::{OffsetDateTime, UtcOffset};

use super::availability::{AvailabilityError, AvailabilityPolicy};
use super::domain::{
    AppointmentDraft, AppointmentKind, AppointmentSlot, CallerIdentity, DomainError,
    IdempotencyKey, Quote, QuoteId,
};
use super::providers::{
    GoogleCalendarProvider, OutlookCalendarProvider, ProviderError, ProviderSession, TimeRange,
};
use super::store::{MAX_APPOINTMENT_QUOTE_SLOTS, PaStore, StoreError};

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
    caller: CallerIdentity,
    kind: AppointmentKind,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    timezone: String,
    requester_included: bool,
    recap: String,
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

/// Coordinates availability policy, calendar reads, and durable quote writes.
pub struct PaService<'a> {
    store: &'a PaStore,
    outlook: &'a dyn OutlookCalendarProvider,
    outlook_session: &'a ProviderSession,
    google: &'a dyn GoogleCalendarProvider,
    google_session: &'a ProviderSession,
    policy: AvailabilityPolicy,
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
        let quote_expiry = now
            .checked_add(Quote::VALID_FOR)
            .ok_or(ServiceError::Availability(
                AvailabilityError::DateTimeOverflow,
            ))?;
        let range =
            TimeRange::new(to_chrono_utc(now)?, to_chrono_utc(horizon_end)?).map_err(|_| {
                ServiceError::InvalidInput {
                    field: "time_range",
                }
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
        let stored_draft =
            prepared
                .draft()
                .ok_or(ServiceError::Store(StoreError::StoredRecordInvalid {
                    resource: "appointment quote",
                }))?;
        let draft_id = prepared.appointment_draft_id().ok_or(ServiceError::Store(
            StoreError::StoredRecordInvalid {
                resource: "appointment quote",
            },
        ))?;
        let recap = appointment_recap(stored_draft, prepared.timezone())?;

        Ok(PreparedRequest {
            draft_id,
            quote_id: prepared.quote_id(),
            caller: stored_draft.caller().clone(),
            kind: stored_draft.kind(),
            starts_at: stored_draft.starts_at(),
            ends_at: stored_draft.ends_at(),
            timezone: prepared.timezone().to_owned(),
            requester_included: stored_draft.requester_included(),
            recap,
        })
    }
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

#[cfg(test)]
mod tests {
    use super::{PaService, ServiceError};
    use crate::pa::availability::{
        AvailabilityError, AvailabilityPolicy, BusyInterval, default_working_windows,
    };
    use crate::pa::domain::{
        AppointmentKind, AppointmentSlot, CallerIdentity, ConfirmedEmail, IdempotencyKey, Quote,
        QuoteId,
    };
    use crate::pa::fakes::{FakeControl, FakeGoogleCalendar, FakeOperation, FakeOutlookCalendar};
    use crate::pa::providers::{CalendarChange, ProviderError, ProviderSession, RetryAfter};
    use crate::pa::store::{PaStore, StoreError};
    use chrono::{DateTime, Duration as ChronoDuration, Utc};
    use time::{Date, Duration, OffsetDateTime, Time, format_description::well_known::Rfc3339};

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
                "search debug leaked {secret}"
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
                assert!(!display.contains(secret), "display leaked {secret}");
                assert!(!debug.contains(secret), "debug leaked {secret}");
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
            assert!(!debug.contains(secret), "prepared debug leaked {secret}");
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
}
