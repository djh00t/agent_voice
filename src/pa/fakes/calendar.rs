//! Deterministic calendar provider fakes for shared reads and calendar writes.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use crate::pa::availability::BusyInterval;
use crate::pa::providers::{
    CalendarAttendee, CalendarChange, CalendarEvent, CalendarReadProvider, CalendarSyncRequest,
    GoogleCalendarProvider, GoogleProposalDraft, GoogleProposalPromotion, OutlookCalendarProvider,
    OwnerEventDraft, ProviderError, ProviderEventId, ProviderFuture, ProviderItemFailure,
    ProviderResult, ProviderSession, Rsvp, SyncPage, TimeRange,
};

use super::control::{FakeControl, FakeOperation};

const CURSOR_PREFIX: &str = "fake-calendar:";
const FAILURE_SOURCE_ID: &str = "fake-calendar";
const OUTLOOK_OWNER_EVENT_PREFIX: &str = "fake-outlook-owner-event-";
const GOOGLE_PROPOSAL_EVENT_PREFIX: &str = "fake-google-proposal-event-";

struct CalendarState {
    /// Materialized busy values retained for the existing read/debug surface.
    /// Google proposal contributions are rebuilt from their identity map.
    busy: Vec<BusyInterval>,
    /// Seeded and Outlook-owned busy values have no Google proposal identity.
    unkeyed_busy: Vec<BusyInterval>,
    changes: Vec<CalendarChange>,
    emitted_cursors: BTreeSet<usize>,
    owner_events: Vec<CalendarEvent>,
    /// Immutable create inputs, kept separate from mutable proposal events.
    ///
    /// A proposal's RSVP and promoted title/attendees can change after
    /// creation, so retries must compare against this original draft rather
    /// than the current event representation.
    google_proposal_create_drafts: Vec<GoogleProposalDraft>,
    google_proposal_events: Vec<CalendarEvent>,
    next_google_event_sequence: u64,
    /// Google busy contributions are keyed by provider event ID. This keeps
    /// event mutations/deletions independent of event-vector ordering.
    google_proposal_busy: BTreeMap<String, BusyInterval>,
    google_promotion_requests: Vec<Option<GoogleProposalPromotion>>,
    deleted_google_proposal_ids: BTreeSet<String>,
    /// Operation keys for deleted proposals remain tombstoned so a delayed
    /// exact create cannot be mistaken for a new remote event.
    deleted_google_proposal_operation_keys: BTreeSet<String>,
    ambiguous_google_create_failures: VecDeque<ProviderError>,
    google_create_response_overrides: VecDeque<CalendarEvent>,
    ambiguous_owner_create_failures: VecDeque<ProviderError>,
    owner_create_response_overrides: VecDeque<CalendarEvent>,
    owner_find_response_overrides: VecDeque<CalendarEvent>,
}

/// Cloneable deterministic implementation of the read-only calendar contract.
///
/// The seeded values are held behind one shared mutex so clones observe the
/// same cursor metadata. The seed itself never changes; cursor positions are
/// stable indexes into the supplied change sequence.
#[derive(Clone)]
pub struct FakeCalendarRead {
    control: FakeControl,
    state: Arc<Mutex<CalendarState>>,
}

impl FakeCalendarRead {
    /// Creates a fake from validated busy intervals and calendar changes.
    ///
    /// Inputs can be supplied by value or through borrowed collections. The
    /// values are cloned into the shared fake state; change ordering is kept
    /// exactly as supplied.
    pub fn new<C, BI, CI>(control: C, busy: BI, changes: CI) -> Self
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        let seeded_busy: Vec<_> = busy.into_iter().map(|value| *value.borrow()).collect();
        Self {
            control: control.borrow().clone(),
            state: Arc::new(Mutex::new(CalendarState {
                busy: seeded_busy.clone(),
                unkeyed_busy: seeded_busy,
                changes: changes
                    .into_iter()
                    .map(|value| value.borrow().clone())
                    .collect(),
                emitted_cursors: BTreeSet::new(),
                owner_events: Vec::new(),
                google_proposal_create_drafts: Vec::new(),
                google_proposal_events: Vec::new(),
                next_google_event_sequence: 1,
                google_proposal_busy: BTreeMap::new(),
                google_promotion_requests: Vec::new(),
                deleted_google_proposal_ids: BTreeSet::new(),
                deleted_google_proposal_operation_keys: BTreeSet::new(),
                ambiguous_google_create_failures: VecDeque::new(),
                google_create_response_overrides: VecDeque::new(),
                ambiguous_owner_create_failures: VecDeque::new(),
                owner_create_response_overrides: VecDeque::new(),
                owner_find_response_overrides: VecDeque::new(),
            })),
        }
    }

    /// Validating-constructor alias for [`Self::new`].
    pub fn try_new<C, BI, CI>(control: C, busy: BI, changes: CI) -> ProviderResult<Self>
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        Ok(Self::new(control, busy, changes))
    }

    /// Seed-constructor alias for [`Self::new`].
    pub fn from_seed<C, BI, CI>(control: C, busy: BI, changes: CI) -> Self
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        Self::new(control, busy, changes)
    }

    /// Returns the shared control plane used by this fake.
    pub fn control(&self) -> &FakeControl {
        &self.control
    }

    fn create_owner_event(&self, draft: &OwnerEventDraft) -> ProviderResult<CalendarEvent> {
        let mut state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        if let Some(existing) = state
            .owner_events
            .iter()
            .find(|event| event.operation_key() == draft.operation_key())
        {
            if owner_draft_matches_event(draft, existing) {
                return Ok(existing.clone());
            }
            return Err(ProviderError::Conflict);
        }

        let provider_event_id = format!(
            "{OUTLOOK_OWNER_EVENT_PREFIX}{}",
            state.owner_events.len().saturating_add(1)
        );
        let event = CalendarEvent::new(
            provider_event_id,
            draft.operation_key(),
            draft.title(),
            draft.time_range().clone(),
            draft.timezone(),
            std::iter::empty::<CalendarAttendee>(),
            self.control.now(),
        )?;
        let busy = busy_for_range(draft.time_range())?;
        let change = CalendarChange::upsert(event.clone())?;

        state.owner_events.push(event.clone());
        state.unkeyed_busy.push(busy);
        rebuild_busy(&mut state);
        state.changes.push(change);
        if let Some(error) = state.ambiguous_owner_create_failures.pop_front() {
            return Err(error);
        }
        if let Some(response) = state.owner_create_response_overrides.pop_front() {
            return Ok(response);
        }
        Ok(event)
    }

    fn create_google_proposal(&self, draft: &GoogleProposalDraft) -> ProviderResult<CalendarEvent> {
        self.control.begin(FakeOperation::CalendarProposalCreate)?;
        let mut state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        if state
            .deleted_google_proposal_operation_keys
            .contains(draft.operation_key())
        {
            return Err(ProviderError::Conflict);
        }
        if let Some(event_index) = state
            .google_proposal_create_drafts
            .iter()
            .position(|stored| stored.operation_key() == draft.operation_key())
        {
            let stored_draft = &state.google_proposal_create_drafts[event_index];
            let existing = state
                .google_proposal_events
                .get(event_index)
                .ok_or(ProviderError::Conflict)?;
            if stored_draft == draft {
                return Ok(existing.clone());
            }
            return Err(ProviderError::Conflict);
        }

        let mut sequence = state.next_google_event_sequence;
        let provider_event_id = loop {
            let candidate = format!("{GOOGLE_PROPOSAL_EVENT_PREFIX}{sequence}");
            let already_used = state
                .changes
                .iter()
                .any(|change| change.provider_event_id() == candidate.as_str())
                || state
                    .google_proposal_events
                    .iter()
                    .any(|event| event.provider_event_id() == candidate.as_str());
            if !already_used {
                break candidate;
            }
            sequence = sequence.checked_add(1).ok_or(ProviderError::Unavailable)?;
        };
        let next_sequence = sequence.checked_add(1).ok_or(ProviderError::Unavailable)?;
        let event = CalendarEvent::new(
            provider_event_id,
            draft.operation_key(),
            draft.pending_title(),
            draft.time_range().clone(),
            draft.timezone(),
            draft.attendees().iter().cloned(),
            self.control.now(),
        )?;
        let busy = busy_for_range(draft.time_range())?;
        let change = CalendarChange::upsert(event.clone())?;

        state.google_proposal_create_drafts.push(draft.clone());
        state.google_proposal_events.push(event.clone());
        state.next_google_event_sequence = next_sequence;
        state
            .google_proposal_busy
            .insert(event.provider_event_id().to_owned(), busy);
        rebuild_busy(&mut state);
        state.google_promotion_requests.push(None);
        state.changes.push(change);
        if let Some(error) = state.ambiguous_google_create_failures.pop_front() {
            return Err(error);
        }
        if let Some(response) = state.google_create_response_overrides.pop_front() {
            return Ok(response);
        }
        Ok(event)
    }

    fn set_google_owner_rsvp(
        &self,
        provider_event_id: &ProviderEventId,
        rsvp: Rsvp,
    ) -> ProviderResult<CalendarEvent> {
        // This is a fake-only setup fixture, but it still participates in the
        // shared control gate so an unavailable or poisoned control can never
        // mutate calendar state behind the provider boundary.
        self.control.begin(FakeOperation::CalendarProposalFind)?;
        let mut state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        let event_index = state
            .google_proposal_events
            .iter()
            .position(|event| event.provider_event_id() == provider_event_id.as_str())
            .ok_or(ProviderError::NotFound)?;
        if matches!(
            state.google_promotion_requests.get(event_index),
            Some(Some(_))
        ) {
            return Err(ProviderError::Conflict);
        }
        let existing = &state.google_proposal_events[event_index];
        let owner = existing
            .attendees()
            .first()
            .ok_or(ProviderError::Conflict)?;
        if owner.rsvp() == rsvp {
            return Ok(existing.clone());
        }

        let mut attendees = existing.attendees().to_vec();
        attendees[0] = CalendarAttendee::new(owner.address().clone(), rsvp)?;
        let event = CalendarEvent::new(
            existing.provider_event_id(),
            existing.operation_key(),
            existing.title(),
            existing.time_range().clone(),
            existing.timezone(),
            attendees,
            self.control.now(),
        )?;
        let change = CalendarChange::upsert(event.clone())?;
        state.google_proposal_events[event_index] = event.clone();
        state.changes.push(change);
        Ok(event)
    }

    fn promote_google_proposal(
        &self,
        promotion: &GoogleProposalPromotion,
    ) -> ProviderResult<CalendarEvent> {
        self.control.begin(FakeOperation::CalendarPromote)?;
        let mut state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        let event_index = state
            .google_proposal_events
            .iter()
            .position(|event| event.provider_event_id() == promotion.provider_event_id())
            .ok_or(ProviderError::NotFound)?;
        let existing = &state.google_proposal_events[event_index];
        let stored_promotion = state
            .google_promotion_requests
            .get(event_index)
            .ok_or(ProviderError::Conflict)?;
        if let Some(stored_promotion) = stored_promotion {
            if stored_promotion == promotion {
                return Ok(existing.clone());
            }
            return Err(ProviderError::Conflict);
        }
        if !promotion.expected_owner_acceptance() || existing.attendees().len() != 1 {
            return Err(ProviderError::Conflict);
        }
        let owner = &existing.attendees()[0];
        if owner.rsvp() != Rsvp::Accepted {
            return Err(ProviderError::Conflict);
        }
        if let Some(requester) = promotion.requester()
            && requester.address() == owner.address()
        {
            return Err(ProviderError::Conflict);
        }

        let mut attendees = Vec::with_capacity(
            1 + if promotion.requester().is_some() {
                1
            } else {
                0
            },
        );
        attendees.push(owner.clone());
        if let Some(requester) = promotion.requester() {
            attendees.push(requester.clone());
        }
        let event = CalendarEvent::new(
            existing.provider_event_id(),
            existing.operation_key(),
            promotion.final_title(),
            existing.time_range().clone(),
            existing.timezone(),
            attendees,
            self.control.now(),
        )?;
        let busy = busy_for_range(existing.time_range())?;
        let change = CalendarChange::upsert(event.clone())?;
        let provider_event_id = existing.provider_event_id().to_owned();
        if !state.google_proposal_busy.contains_key(&provider_event_id) {
            return Err(ProviderError::Conflict);
        }

        state.google_proposal_events[event_index] = event.clone();
        state.google_proposal_busy.insert(provider_event_id, busy);
        rebuild_busy(&mut state);
        state.changes.push(change);
        state.google_promotion_requests[event_index] = Some(promotion.clone());
        Ok(event)
    }

    fn delete_google_proposal(&self, provider_event_id: &ProviderEventId) -> ProviderResult<()> {
        self.control.begin(FakeOperation::CalendarDelete)?;
        let mut state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        let event_index = match state
            .google_proposal_events
            .iter()
            .position(|event| event.provider_event_id() == provider_event_id.as_str())
        {
            Some(index) => index,
            None if state
                .deleted_google_proposal_ids
                .contains(provider_event_id.as_str()) =>
            {
                return Ok(());
            }
            None => return Err(ProviderError::NotFound),
        };
        if !state
            .google_proposal_busy
            .contains_key(provider_event_id.as_str())
        {
            return Err(ProviderError::Conflict);
        }
        let operation_key = state
            .google_proposal_create_drafts
            .get(event_index)
            .ok_or(ProviderError::Conflict)?
            .operation_key()
            .to_owned();
        let change = CalendarChange::deleted(provider_event_id.as_str(), self.control.now())?;

        state.google_proposal_events.remove(event_index);
        state.google_proposal_create_drafts.remove(event_index);
        state.google_promotion_requests.remove(event_index);
        state
            .google_proposal_busy
            .remove(provider_event_id.as_str());
        rebuild_busy(&mut state);
        state
            .deleted_google_proposal_ids
            .insert(provider_event_id.as_str().to_owned());
        state
            .deleted_google_proposal_operation_keys
            .insert(operation_key);
        state.changes.push(change);
        Ok(())
    }
}

impl fmt::Debug for FakeCalendarRead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return formatter.write_str("FakeCalendarRead { state: unavailable }"),
        };
        formatter
            .debug_struct("FakeCalendarRead")
            .field("busy_count", &state.busy.len())
            .field("change_count", &state.changes.len())
            .field("emitted_cursor_count", &state.emitted_cursors.len())
            .finish()
    }
}

impl CalendarReadProvider for FakeCalendarRead {
    fn list_busy<'a>(
        &'a self,
        _session: &'a ProviderSession,
        time_range: &'a TimeRange,
    ) -> ProviderFuture<'a, Vec<BusyInterval>> {
        let control = self.control.clone();
        let state = Arc::clone(&self.state);
        let range_start = chrono_key(time_range.start());
        let range_end = chrono_key(time_range.end());
        Box::pin(async move {
            control.begin(FakeOperation::CalendarBusy)?;
            let state = state.lock().map_err(|_| ProviderError::Unavailable)?;
            let mut intervals: Vec<_> = state
                .busy
                .iter()
                .copied()
                .filter(|interval| {
                    let starts_at = offset_key(interval.starts_at());
                    let ends_at = offset_key(interval.ends_at());
                    starts_at < range_end && ends_at > range_start
                })
                .collect();
            intervals.sort_by(|left, right| {
                left.starts_at()
                    .cmp(&right.starts_at())
                    .then_with(|| left.ends_at().cmp(&right.ends_at()))
            });
            Ok(intervals)
        })
    }

    fn sync_calendar<'a>(
        &'a self,
        _session: &'a ProviderSession,
        request: &'a CalendarSyncRequest,
    ) -> ProviderFuture<'a, SyncPage<CalendarChange>> {
        let control = self.control.clone();
        let state = Arc::clone(&self.state);
        let cursor = request.cursor().map(str::to_owned);
        let limit = request.limit();
        let time_range = request.time_range().clone();
        Box::pin(async move {
            control.begin(FakeOperation::CalendarSync)?;
            let partial_after = control.partial_failure_after(FakeOperation::CalendarSync)?;
            let mut state = state.lock().map_err(|_| ProviderError::Unavailable)?;
            let start = match cursor {
                None => 0,
                Some(cursor) => parse_cursor(&cursor, &state)?,
            };

            let mut prior_event_ranges = BTreeMap::new();
            for change in state.changes.iter().take(start) {
                remember_change_range(change, &mut prior_event_ranges);
            }
            let matching_positions: Vec<_> = state
                .changes
                .iter()
                .enumerate()
                .skip(start)
                .filter_map(|(position, change)| {
                    change_matches_time_range(change, &time_range, &mut prior_event_ranges)
                        .then_some(position)
                })
                .collect();
            let available = matching_positions.len();
            let page_count = available.min(limit);
            let failure_after = partial_after.filter(|after| *after < page_count);
            let successful_count = failure_after.unwrap_or(page_count);
            let items = matching_positions[..successful_count]
                .iter()
                .map(|position| state.changes[*position].clone())
                .collect();
            let item_failures = match failure_after {
                Some(_) => {
                    let failed = &state.changes[matching_positions[successful_count]];
                    vec![ProviderItemFailure::new(
                        FAILURE_SOURCE_ID,
                        failed.provider_event_id(),
                        ProviderError::Unavailable,
                    )?]
                }
                None => Vec::new(),
            };
            let next_position = failure_after
                .map(|_| matching_positions[successful_count])
                .or_else(|| {
                    (page_count < available)
                        .then(|| matching_positions[page_count - 1].saturating_add(1))
                });
            let next_cursor = next_position
                .filter(|position| *position < state.changes.len())
                .map(cursor_for);
            let page = SyncPage::new(items, next_cursor, item_failures)?;
            if let Some(next_position) = next_position
                && next_position < state.changes.len()
            {
                state.emitted_cursors.insert(next_position);
            }
            Ok(page)
        })
    }
}

/// Cloneable deterministic Outlook calendar fake.
///
/// Read operations delegate to the shared [`FakeCalendarRead`] core. The only
/// write capability is creation of owner-only events; all mutations are kept
/// in the same mutex-protected state as reads so retries and concurrent clones
/// observe one coherent sequence.
#[derive(Clone)]
pub struct FakeOutlookCalendar {
    read: FakeCalendarRead,
}

/// Cloneable deterministic Google calendar fake for the proposal lifecycle.
///
/// Read operations delegate to the shared [`FakeCalendarRead`] core. The
/// proposal lifecycle shares one mutex-protected state with reads. The
/// original create draft is retained separately from mutable event state, so
/// a retry remains idempotent after an owner RSVP or proposal promotion.
#[derive(Clone)]
pub struct FakeGoogleCalendar {
    read: FakeCalendarRead,
}

impl FakeGoogleCalendar {
    /// Creates a Google fake from seeded read data and shared fake control.
    pub fn new<C, BI, CI>(control: C, busy: BI, changes: CI) -> Self
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        Self {
            read: FakeCalendarRead::new(control, busy, changes),
        }
    }

    /// Validating-constructor alias for [`Self::new`].
    pub fn try_new<C, BI, CI>(control: C, busy: BI, changes: CI) -> ProviderResult<Self>
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        Ok(Self::new(control, busy, changes))
    }

    /// Seed-constructor alias for [`Self::new`].
    pub fn from_seed<C, BI, CI>(control: C, busy: BI, changes: CI) -> Self
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        Self::new(control, busy, changes)
    }

    /// Wraps an already-seeded read fake while preserving its shared state.
    pub fn from_read(read: FakeCalendarRead) -> Self {
        Self { read }
    }

    /// Returns the shared fake control plane.
    pub fn control(&self) -> &FakeControl {
        self.read.control()
    }

    /// Returns the delegated read core.
    pub fn read(&self) -> &FakeCalendarRead {
        &self.read
    }

    /// Arranges one create response that persists the event before returning a
    /// closed provider failure, modeling an ambiguous remote timeout.
    pub fn queue_ambiguous_create_failure(&self, error: ProviderError) -> ProviderResult<()> {
        let mut state = self
            .read
            .state
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        state.ambiguous_google_create_failures.push_back(error);
        Ok(())
    }

    /// Arranges one typed response event after the real proposal has been
    /// persisted. This is test-only support for validating untrusted provider
    /// responses at the service boundary.
    pub fn queue_create_response_override(&self, event: CalendarEvent) -> ProviderResult<()> {
        let mut state = self
            .read
            .state
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        state.google_create_response_overrides.push_back(event);
        Ok(())
    }

    fn create_proposal(&self, draft: &GoogleProposalDraft) -> ProviderResult<CalendarEvent> {
        self.read.create_google_proposal(draft)
    }

    fn find_proposal(&self, draft: &GoogleProposalDraft) -> ProviderResult<CalendarEvent> {
        self.read
            .control
            .begin(FakeOperation::CalendarProposalFind)?;
        let state = self
            .read
            .state
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        state
            .google_proposal_events
            .iter()
            .find(|event| event.operation_key() == draft.operation_key())
            .cloned()
            .ok_or(ProviderError::NotFound)
    }

    /// Applies a typed owner RSVP fixture before promoting a fake proposal.
    ///
    /// This exists only on the deterministic fake; it does not extend the
    /// production Google provider capability.
    pub fn set_owner_rsvp(
        &self,
        provider_event_id: &ProviderEventId,
        rsvp: Rsvp,
    ) -> ProviderResult<CalendarEvent> {
        self.read.set_google_owner_rsvp(provider_event_id, rsvp)
    }

    fn promote_proposal(
        &self,
        promotion: &GoogleProposalPromotion,
    ) -> ProviderResult<CalendarEvent> {
        self.read.promote_google_proposal(promotion)
    }

    fn delete_proposal(&self, provider_event_id: &ProviderEventId) -> ProviderResult<()> {
        self.read.delete_google_proposal(provider_event_id)
    }
}

impl fmt::Debug for FakeGoogleCalendar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.read.state.lock() {
            Ok(state) => state,
            Err(_) => return formatter.write_str("FakeGoogleCalendar { state: unavailable }"),
        };
        formatter
            .debug_struct("FakeGoogleCalendar")
            .field("busy_count", &state.busy.len())
            .field("change_count", &state.changes.len())
            .field("proposal_event_count", &state.google_proposal_events.len())
            .field("emitted_cursor_count", &state.emitted_cursors.len())
            .finish()
    }
}

impl CalendarReadProvider for FakeGoogleCalendar {
    fn list_busy<'a>(
        &'a self,
        session: &'a ProviderSession,
        time_range: &'a TimeRange,
    ) -> ProviderFuture<'a, Vec<BusyInterval>> {
        self.read.list_busy(session, time_range)
    }

    fn sync_calendar<'a>(
        &'a self,
        session: &'a ProviderSession,
        request: &'a CalendarSyncRequest,
    ) -> ProviderFuture<'a, SyncPage<CalendarChange>> {
        self.read.sync_calendar(session, request)
    }
}

impl GoogleCalendarProvider for FakeGoogleCalendar {
    fn find_proposal<'a>(
        &'a self,
        _session: &'a ProviderSession,
        draft: &'a GoogleProposalDraft,
    ) -> ProviderFuture<'a, CalendarEvent> {
        let fake = self.clone();
        let draft = draft.clone();
        Box::pin(async move { fake.find_proposal(&draft) })
    }

    fn create_proposal<'a>(
        &'a self,
        _session: &'a ProviderSession,
        draft: &'a GoogleProposalDraft,
    ) -> ProviderFuture<'a, CalendarEvent> {
        let fake = self.clone();
        let draft = draft.clone();
        Box::pin(async move { fake.create_proposal(&draft) })
    }

    fn promote_proposal<'a>(
        &'a self,
        _session: &'a ProviderSession,
        promotion: &'a GoogleProposalPromotion,
    ) -> ProviderFuture<'a, CalendarEvent> {
        let fake = self.clone();
        let promotion = promotion.clone();
        Box::pin(async move { fake.promote_proposal(&promotion) })
    }

    fn delete_proposal<'a>(
        &'a self,
        _session: &'a ProviderSession,
        provider_event_id: &'a ProviderEventId,
    ) -> ProviderFuture<'a, ()> {
        let fake = self.clone();
        let provider_event_id = provider_event_id.clone();
        Box::pin(async move { fake.delete_proposal(&provider_event_id) })
    }
}

impl FakeOutlookCalendar {
    /// Creates an Outlook fake from seeded read data and shared fake control.
    pub fn new<C, BI, CI>(control: C, busy: BI, changes: CI) -> Self
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        Self {
            read: FakeCalendarRead::new(control, busy, changes),
        }
    }

    /// Validating-constructor alias for [`Self::new`].
    pub fn try_new<C, BI, CI>(control: C, busy: BI, changes: CI) -> ProviderResult<Self>
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        Ok(Self::new(control, busy, changes))
    }

    /// Seed-constructor alias for [`Self::new`].
    pub fn from_seed<C, BI, CI>(control: C, busy: BI, changes: CI) -> Self
    where
        C: Borrow<FakeControl>,
        BI: IntoIterator,
        BI::Item: Borrow<BusyInterval>,
        CI: IntoIterator,
        CI::Item: Borrow<CalendarChange>,
    {
        Self::new(control, busy, changes)
    }

    /// Wraps an already-seeded read fake while preserving its shared state.
    pub fn from_read(read: FakeCalendarRead) -> Self {
        Self { read }
    }

    /// Returns the shared fake control plane.
    pub fn control(&self) -> &FakeControl {
        self.read.control()
    }

    /// Returns the delegated read core.
    pub fn read(&self) -> &FakeCalendarRead {
        &self.read
    }

    /// Persists one owner event then returns the supplied failure, modeling a
    /// timeout after a remote side effect.
    pub fn queue_ambiguous_owner_create_failure(&self, error: ProviderError) -> ProviderResult<()> {
        let mut state = self
            .read
            .state
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        state.ambiguous_owner_create_failures.push_back(error);
        Ok(())
    }

    /// Returns this typed response after persisting the valid owner event.
    /// Tests use it to prove service-side response validation is fail-closed.
    pub fn queue_owner_create_response_override(&self, event: CalendarEvent) -> ProviderResult<()> {
        let mut state = self
            .read
            .state
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        state.owner_create_response_overrides.push_back(event);
        Ok(())
    }

    /// Returns the supplied typed response from the next owner-event lookup.
    /// Tests use this to prove service-side response validation is fail-closed.
    pub fn queue_owner_find_response_override(&self, event: CalendarEvent) -> ProviderResult<()> {
        let mut state = self
            .read
            .state
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        state.owner_find_response_overrides.push_back(event);
        Ok(())
    }
}

impl fmt::Debug for FakeOutlookCalendar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.read.state.lock() {
            Ok(state) => state,
            Err(_) => return formatter.write_str("FakeOutlookCalendar { state: unavailable }"),
        };
        formatter
            .debug_struct("FakeOutlookCalendar")
            .field("busy_count", &state.busy.len())
            .field("change_count", &state.changes.len())
            .field("owner_event_count", &state.owner_events.len())
            .field("emitted_cursor_count", &state.emitted_cursors.len())
            .finish()
    }
}

impl CalendarReadProvider for FakeOutlookCalendar {
    fn list_busy<'a>(
        &'a self,
        session: &'a ProviderSession,
        time_range: &'a TimeRange,
    ) -> ProviderFuture<'a, Vec<BusyInterval>> {
        self.read.list_busy(session, time_range)
    }

    fn sync_calendar<'a>(
        &'a self,
        session: &'a ProviderSession,
        request: &'a CalendarSyncRequest,
    ) -> ProviderFuture<'a, SyncPage<CalendarChange>> {
        self.read.sync_calendar(session, request)
    }
}

impl OutlookCalendarProvider for FakeOutlookCalendar {
    fn find_owner_event<'a>(
        &'a self,
        _session: &'a ProviderSession,
        draft: &'a OwnerEventDraft,
    ) -> ProviderFuture<'a, CalendarEvent> {
        let control = self.read.control.clone();
        let read = self.read.clone();
        let draft = draft.clone();
        Box::pin(async move {
            control.begin(FakeOperation::CalendarOwnerFind)?;
            let mut state = read.state.lock().map_err(|_| ProviderError::Unavailable)?;
            if let Some(response) = state.owner_find_response_overrides.pop_front() {
                return Ok(response);
            }
            state
                .owner_events
                .iter()
                .find(|event| event.operation_key() == draft.operation_key())
                .cloned()
                .ok_or(ProviderError::NotFound)
        })
    }

    fn create_owner_event<'a>(
        &'a self,
        _session: &'a ProviderSession,
        draft: &'a OwnerEventDraft,
    ) -> ProviderFuture<'a, CalendarEvent> {
        let control = self.read.control.clone();
        let read = self.read.clone();
        let draft = draft.clone();
        Box::pin(async move {
            control.begin(FakeOperation::CalendarOwnerCreate)?;
            read.create_owner_event(&draft)
        })
    }
}

fn owner_draft_matches_event(draft: &OwnerEventDraft, event: &CalendarEvent) -> bool {
    event.operation_key() == draft.operation_key()
        && event.title() == draft.title()
        && event.time_range() == draft.time_range()
        && event.timezone() == draft.timezone()
        && event.attendees().is_empty()
}

fn busy_for_range(range: &TimeRange) -> ProviderResult<BusyInterval> {
    let starts_at = offset_datetime(range.start())?;
    let ends_at = offset_datetime(range.end())?;
    BusyInterval::new(starts_at, ends_at).map_err(|_| ProviderError::InvalidInput {
        field: crate::pa::providers::ProviderInputField::TimeRange,
    })
}

fn rebuild_busy(state: &mut CalendarState) {
    state.busy.clear();
    state.busy.reserve(
        state
            .unkeyed_busy
            .len()
            .saturating_add(state.google_proposal_busy.len()),
    );
    state.busy.extend(state.unkeyed_busy.iter().copied());
    state
        .busy
        .extend(state.google_proposal_busy.values().copied());
}

fn offset_datetime(value: DateTime<Utc>) -> ProviderResult<time::OffsetDateTime> {
    time::OffsetDateTime::from_unix_timestamp(value.timestamp())
        .ok()
        .and_then(|base| base.replace_nanosecond(value.timestamp_subsec_nanos()).ok())
        .map(|value| value.to_offset(time::UtcOffset::UTC))
        .ok_or(ProviderError::InvalidInput {
            field: crate::pa::providers::ProviderInputField::TimeRange,
        })
}

fn cursor_for(position: usize) -> String {
    format!("{CURSOR_PREFIX}{position}")
}

fn parse_cursor(cursor: &str, state: &CalendarState) -> ProviderResult<usize> {
    let suffix = cursor
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(ProviderError::CursorExpired)?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProviderError::CursorExpired);
    }
    let position = suffix
        .parse::<usize>()
        .map_err(|_| ProviderError::CursorExpired)?;
    if position >= state.changes.len()
        || cursor_for(position) != cursor
        || !state.emitted_cursors.contains(&position)
    {
        return Err(ProviderError::CursorExpired);
    }
    Ok(position)
}

fn remember_change_range(change: &CalendarChange, event_ranges: &mut BTreeMap<String, TimeRange>) {
    if let Some(event) = change.event() {
        event_ranges.insert(
            event.provider_event_id().to_owned(),
            event.time_range().clone(),
        );
    } else {
        event_ranges.remove(change.provider_event_id());
    }
}

fn change_matches_time_range(
    change: &CalendarChange,
    requested_range: &TimeRange,
    event_ranges: &mut BTreeMap<String, TimeRange>,
) -> bool {
    if let Some(event) = change.event() {
        let matches = ranges_overlap(event.time_range(), requested_range);
        event_ranges.insert(
            event.provider_event_id().to_owned(),
            event.time_range().clone(),
        );
        return matches;
    }

    // A deletion tombstone has no event window. When its preceding upsert is
    // known, scope it to that event's window; otherwise retain it as an
    // unscoped invalidation signal so a consumer cannot miss the deletion.
    event_ranges
        .remove(change.provider_event_id())
        .map(|event_range| ranges_overlap(&event_range, requested_range))
        .unwrap_or(true)
}

fn ranges_overlap(left: &TimeRange, right: &TimeRange) -> bool {
    chrono_key(left.start()) < chrono_key(right.end())
        && chrono_key(left.end()) > chrono_key(right.start())
}

fn chrono_key(value: DateTime<Utc>) -> (i64, u32) {
    (value.timestamp(), value.timestamp_subsec_nanos())
}

fn offset_key(value: time::OffsetDateTime) -> (i64, u32) {
    (value.unix_timestamp(), value.nanosecond())
}

#[cfg(test)]
mod tests {
    use super::{FakeCalendarRead, FakeGoogleCalendar, FakeOutlookCalendar};
    use crate::pa::availability::BusyInterval;
    use crate::pa::fakes::{FakeControl, FakeOperation};
    use crate::pa::providers::{
        CalendarAttendee, CalendarChange, CalendarEvent, CalendarReadProvider, CalendarSyncRequest,
        GoogleCalendarProvider, GoogleProposalDraft, GoogleProposalPromotion, MailAddress,
        OutlookCalendarProvider, OwnerEventDraft, ProviderError, ProviderEventId, ProviderFuture,
        ProviderSession, RetryAfter, Rsvp, TimeRange,
    };
    use chrono::{DateTime, Duration, Utc};
    use futures_util::FutureExt;
    use std::fmt;
    use std::sync::Arc;
    use time::{OffsetDateTime, UtcOffset};

    const NOW: &str = "2026-08-29T12:34:56Z";

    fn now() -> DateTime<Utc> {
        NOW.parse().expect("valid UTC instant")
    }

    fn session() -> ProviderSession {
        ProviderSession::new("calendar-account", "sentinel-session-token", None)
            .expect("valid session")
    }

    fn range(start: &str, end: &str) -> TimeRange {
        TimeRange::new(start.parse().expect("start"), end.parse().expect("end"))
            .expect("valid range")
    }

    fn busy(start: i64, end: i64) -> BusyInterval {
        BusyInterval::new(
            OffsetDateTime::from_unix_timestamp(start)
                .expect("start")
                .to_offset(UtcOffset::UTC),
            OffsetDateTime::from_unix_timestamp(end)
                .expect("end")
                .to_offset(UtcOffset::UTC),
        )
        .expect("valid busy interval")
    }

    fn change(id: &str, changed_at: &str) -> CalendarChange {
        CalendarChange::deleted(id, changed_at.parse().expect("changed_at")).expect("change")
    }

    fn event_change(id: &str, title: &str) -> CalendarChange {
        let event = CalendarEvent::new(
            id,
            "sentinel-operation-key",
            title,
            range("2026-08-29T10:00:00Z", "2026-08-29T11:00:00Z"),
            "Australia/Sydney",
            [CalendarAttendee::new(
                MailAddress::new("sentinel-calendar-attendee@example.test")
                    .expect("attendee address"),
                Rsvp::Accepted,
            )
            .expect("attendee")],
            now(),
        )
        .expect("event");
        CalendarChange::upsert(event).expect("upsert")
    }

    #[tokio::test]
    async fn list_busy_filters_half_open_range_orders_and_begins_once() {
        let control = FakeControl::new(now());
        let fake = FakeCalendarRead::new(
            control.clone(),
            [
                busy(200, 300),
                busy(100, 200),
                busy(250, 350),
                busy(50, 90),
                busy(300, 400),
            ],
            Vec::<CalendarChange>::new(),
        );

        let result = fake
            .list_busy(
                &session(),
                &range("1970-01-01T00:01:30Z", "1970-01-01T00:05:00Z"),
            )
            .await
            .expect("busy result");

        assert_eq!(result, vec![busy(100, 200), busy(200, 300), busy(250, 350)]);
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("count"),
            1
        );
    }

    #[tokio::test]
    async fn sync_pages_are_cursor_stable_and_retries_are_deterministic() {
        let control = FakeControl::new(now());
        let fake = FakeCalendarRead::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            [
                change("event-1", NOW),
                change("event-2", "2026-08-29T12:35:56Z"),
                change("event-3", "2026-08-29T12:36:56Z"),
            ],
        );
        let request = |cursor: Option<String>, limit| {
            CalendarSyncRequest::new(
                range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                cursor,
                limit,
            )
            .expect("request")
        };

        let first = fake
            .sync_calendar(&session(), &request(None, 1))
            .await
            .expect("first page");
        assert_eq!(first.items().len(), 1);
        let first_cursor = first.next_cursor().expect("next cursor").to_owned();

        let second = fake
            .sync_calendar(&session(), &request(Some(first_cursor.clone()), 1))
            .await
            .expect("second page");
        let second_retry = fake
            .sync_calendar(&session(), &request(Some(first_cursor), 1))
            .await
            .expect("same-cursor retry page");
        assert_eq!(second_retry, second);
        let second_cursor = second.next_cursor().expect("second cursor").to_owned();

        let end = fake
            .sync_calendar(&session(), &request(Some(second_cursor), 1))
            .await
            .expect("end page");
        assert_eq!(end.items().len(), 1);
        assert_eq!(end.next_cursor(), None);
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarSync)
                .expect("count"),
            4
        );
    }

    #[tokio::test]
    async fn sync_filters_upserts_to_the_requested_half_open_window() {
        let control = FakeControl::new(now());
        let fake = FakeCalendarRead::new(
            control,
            Vec::<BusyInterval>::new(),
            [
                CalendarChange::upsert(
                    CalendarEvent::new(
                        "outside-before",
                        "operation-outside-before",
                        "outside before",
                        range("2026-08-29T08:00:00Z", "2026-08-29T09:00:00Z"),
                        "Australia/Sydney",
                        std::iter::empty::<CalendarAttendee>(),
                        now(),
                    )
                    .expect("outside-before event"),
                )
                .expect("outside-before change"),
                CalendarChange::upsert(
                    CalendarEvent::new(
                        "inside-first",
                        "operation-inside-first",
                        "inside first",
                        range("2026-08-29T10:00:00Z", "2026-08-29T10:30:00Z"),
                        "Australia/Sydney",
                        std::iter::empty::<CalendarAttendee>(),
                        now(),
                    )
                    .expect("inside-first event"),
                )
                .expect("inside-first change"),
                CalendarChange::upsert(
                    CalendarEvent::new(
                        "outside-after",
                        "operation-outside-after",
                        "outside after",
                        range("2026-08-29T11:00:00Z", "2026-08-29T12:00:00Z"),
                        "Australia/Sydney",
                        std::iter::empty::<CalendarAttendee>(),
                        now(),
                    )
                    .expect("outside-after event"),
                )
                .expect("outside-after change"),
                CalendarChange::upsert(
                    CalendarEvent::new(
                        "inside-second",
                        "operation-inside-second",
                        "inside second",
                        range("2026-08-29T10:30:00Z", "2026-08-29T11:00:00Z"),
                        "Australia/Sydney",
                        std::iter::empty::<CalendarAttendee>(),
                        now(),
                    )
                    .expect("inside-second event"),
                )
                .expect("inside-second change"),
            ],
        );
        let request = |cursor: Option<String>, limit| {
            CalendarSyncRequest::new(
                range("2026-08-29T10:00:00Z", "2026-08-29T11:00:00Z"),
                cursor,
                limit,
            )
            .expect("request")
        };

        let first = fake
            .sync_calendar(&session(), &request(None, 1))
            .await
            .expect("first filtered page");
        assert_eq!(
            first
                .items()
                .iter()
                .map(CalendarChange::provider_event_id)
                .collect::<Vec<_>>(),
            vec!["inside-first"]
        );
        let cursor = first.next_cursor().expect("filtered cursor").to_owned();

        let second = fake
            .sync_calendar(&session(), &request(Some(cursor.clone()), 1))
            .await
            .expect("second filtered page");
        assert_eq!(
            second
                .items()
                .iter()
                .map(CalendarChange::provider_event_id)
                .collect::<Vec<_>>(),
            vec!["inside-second"]
        );
        assert_eq!(second.next_cursor(), None);

        let retry = fake
            .sync_calendar(&session(), &request(Some(cursor), 1))
            .await
            .expect("same filtered cursor retry");
        assert_eq!(retry, second);
    }

    #[tokio::test]
    async fn malformed_or_stale_cursors_fail_without_advancing() {
        let control = FakeControl::new(now());
        let fake = FakeCalendarRead::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            [
                change("event-1", NOW),
                change("event-2", "2026-08-29T12:35:56Z"),
            ],
        );
        let request = |cursor: Option<String>| {
            CalendarSyncRequest::new(
                range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                cursor,
                1,
            )
            .expect("request")
        };

        for cursor in [
            "not-a-fake-cursor",
            "fake-calendar:abc",
            "fake-calendar:0",
            "fake-calendar:1",
            "fake-calendar:2",
        ] {
            assert_eq!(
                fake.sync_calendar(&session(), &request(Some(cursor.to_owned())))
                    .await,
                Err(ProviderError::CursorExpired),
                "cursor {cursor:?} should fail closed"
            );
        }
        let first = fake
            .sync_calendar(&session(), &request(None))
            .await
            .expect("first page");
        assert_eq!(first.items().len(), 1);
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarSync)
                .expect("count"),
            6
        );
    }

    #[tokio::test]
    async fn partial_failure_returns_prefix_and_retryable_failed_change() {
        let control = FakeControl::new(now());
        control
            .set_partial_failure(FakeOperation::CalendarSync, 1)
            .expect("partial failure");
        let fake = FakeCalendarRead::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            [
                change("event-1", NOW),
                change("event-2", "2026-08-29T12:35:56Z"),
            ],
        );
        let request = CalendarSyncRequest::new(
            range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            None,
            2,
        )
        .expect("request");

        let partial = fake
            .sync_calendar(&session(), &request)
            .await
            .expect("partial page");
        assert_eq!(partial.items().len(), 1);
        assert_eq!(partial.item_failures().len(), 1);
        let failure = &partial.item_failures()[0];
        assert_eq!(failure.source_id(), "fake-calendar");
        assert_eq!(failure.item_id(), "event-2");
        assert_eq!(failure.error(), ProviderError::Unavailable);
        let cursor = partial.next_cursor().expect("prefix cursor").to_owned();

        control
            .clear_partial_failure(FakeOperation::CalendarSync)
            .expect("clear partial failure");
        let retry_request = CalendarSyncRequest::new(
            range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            Some(cursor),
            2,
        )
        .expect("retry request");
        let retry = fake
            .sync_calendar(&session(), &retry_request)
            .await
            .expect("retry failed item");
        assert_eq!(retry.items().len(), 1);
        assert_eq!(retry.items()[0].provider_event_id(), "event-2");
    }

    #[tokio::test]
    async fn zero_success_partial_page_retries_from_position_zero_without_skipping_items() {
        let control = FakeControl::new(now());
        control
            .set_partial_failure(FakeOperation::CalendarSync, 0)
            .expect("zero-success partial failure");
        let fake = FakeCalendarRead::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            [
                change("event-1", NOW),
                change("event-2", "2026-08-29T12:35:56Z"),
                change("event-3", "2026-08-29T12:36:56Z"),
            ],
        );
        let request = |cursor: Option<String>, limit| {
            CalendarSyncRequest::new(
                range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                cursor,
                limit,
            )
            .expect("request")
        };

        let partial = fake
            .sync_calendar(&session(), &request(None, 2))
            .await
            .expect("zero-success partial page");
        assert!(partial.items().is_empty());
        assert_eq!(partial.next_cursor(), Some("fake-calendar:0"));
        assert_eq!(partial.item_failures().len(), 1);
        assert_eq!(partial.item_failures()[0].item_id(), "event-1");

        control
            .clear_partial_failure(FakeOperation::CalendarSync)
            .expect("clear partial failure");
        let retry = fake
            .sync_calendar(&session(), &request(Some("fake-calendar:0".to_owned()), 2))
            .await
            .expect("retry from position zero");
        assert_eq!(
            retry
                .items()
                .iter()
                .map(CalendarChange::provider_event_id)
                .collect::<Vec<_>>(),
            vec!["event-1", "event-2"]
        );
        assert_eq!(retry.next_cursor(), Some("fake-calendar:2"));
    }

    #[tokio::test]
    async fn injected_failures_propagate_without_cursor_state_changes() {
        let control = FakeControl::new(now());
        control
            .queue_failure(FakeOperation::CalendarSync, ProviderError::TokenExpired)
            .expect("queue expiry");
        let fake = FakeCalendarRead::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            [
                change("event-1", NOW),
                change("event-2", "2026-08-29T12:35:56Z"),
            ],
        );
        let request = CalendarSyncRequest::new(
            range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            None,
            1,
        )
        .expect("request");
        assert_eq!(
            fake.sync_calendar(&session(), &request).await,
            Err(ProviderError::TokenExpired)
        );
        let page = fake
            .sync_calendar(&session(), &request)
            .await
            .expect("retry page");
        assert_eq!(page.items()[0].provider_event_id(), "event-1");
    }

    #[tokio::test]
    async fn list_busy_one_shot_and_persistent_failures_recover_without_mutation() {
        let control = FakeControl::new(now());
        control
            .queue_failure(FakeOperation::CalendarBusy, ProviderError::TokenExpired)
            .expect("queue token expiry");
        let fake = FakeCalendarRead::new(
            control.clone(),
            [busy(100, 200)],
            Vec::<CalendarChange>::new(),
        );
        let time_range = range("1970-01-01T00:00:00Z", "1970-01-01T00:10:00Z");

        assert_eq!(
            fake.list_busy(&session(), &time_range).await,
            Err(ProviderError::TokenExpired)
        );
        assert_eq!(
            fake.list_busy(&session(), &time_range).await,
            Ok(vec![busy(100, 200)])
        );

        let retry_after = RetryAfter::new(Duration::seconds(7)).expect("retry delay");
        let throttled = ProviderError::throttled(retry_after);
        control
            .set_failure(FakeOperation::CalendarBusy, throttled)
            .expect("set throttle");
        let throttle_error = fake
            .list_busy(&session(), &time_range)
            .await
            .expect_err("persistent throttle");
        assert_eq!(throttle_error, throttled);
        assert_eq!(
            throttle_error
                .retry_after()
                .expect("retry after")
                .duration(),
            Duration::seconds(7)
        );
        assert_eq!(
            fake.list_busy(&session(), &time_range).await,
            Err(throttled)
        );

        control
            .clear_failure(FakeOperation::CalendarBusy)
            .expect("clear throttle");
        assert_eq!(
            fake.list_busy(&session(), &time_range).await,
            Ok(vec![busy(100, 200)])
        );

        control
            .set_failure(FakeOperation::CalendarBusy, ProviderError::Unavailable)
            .expect("set unavailable");
        assert_eq!(
            fake.list_busy(&session(), &time_range).await,
            Err(ProviderError::Unavailable)
        );
        control
            .clear_failure(FakeOperation::CalendarBusy)
            .expect("clear unavailable");
        assert_eq!(
            fake.list_busy(&session(), &time_range).await,
            Ok(vec![busy(100, 200)])
        );
    }

    #[tokio::test]
    async fn sync_one_shot_and_persistent_failures_recover_without_cursor_mutation() {
        let control = FakeControl::new(now());
        control
            .queue_failure(FakeOperation::CalendarSync, ProviderError::TokenExpired)
            .expect("queue token expiry");
        let fake = FakeCalendarRead::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            [
                change("event-1", NOW),
                change("event-2", "2026-08-29T12:35:56Z"),
            ],
        );
        let request = |cursor: Option<String>| {
            CalendarSyncRequest::new(
                range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                cursor,
                1,
            )
            .expect("request")
        };

        assert_eq!(
            fake.sync_calendar(&session(), &request(None)).await,
            Err(ProviderError::TokenExpired)
        );
        let first = fake
            .sync_calendar(&session(), &request(None))
            .await
            .expect("first page after expiry");
        let cursor = first.next_cursor().expect("emitted cursor").to_owned();

        let retry_after = RetryAfter::new(Duration::seconds(11)).expect("retry delay");
        let throttled = ProviderError::throttled(retry_after);
        control
            .set_failure(FakeOperation::CalendarSync, throttled)
            .expect("set throttle");
        let throttle_error = fake
            .sync_calendar(&session(), &request(Some(cursor.clone())))
            .await
            .expect_err("persistent throttle");
        assert_eq!(throttle_error, throttled);
        assert_eq!(
            throttle_error
                .retry_after()
                .expect("retry after")
                .duration(),
            Duration::seconds(11)
        );
        assert_eq!(
            fake.sync_calendar(&session(), &request(Some(cursor.clone())))
                .await,
            Err(throttled)
        );

        control
            .clear_failure(FakeOperation::CalendarSync)
            .expect("clear throttle");
        let second = fake
            .sync_calendar(&session(), &request(Some(cursor.clone())))
            .await
            .expect("same cursor after throttle");
        assert_eq!(second.items()[0].provider_event_id(), "event-2");
        assert_eq!(second.next_cursor(), None);

        control
            .set_failure(FakeOperation::CalendarSync, ProviderError::Unavailable)
            .expect("set unavailable");
        assert_eq!(
            fake.sync_calendar(&session(), &request(None)).await,
            Err(ProviderError::Unavailable)
        );
        control
            .clear_failure(FakeOperation::CalendarSync)
            .expect("clear unavailable");
        let recovered = fake
            .sync_calendar(&session(), &request(None))
            .await
            .expect("recovered sync");
        assert_eq!(recovered.items()[0].provider_event_id(), "event-1");
    }

    #[tokio::test]
    async fn cloned_calendar_reads_share_cursor_state_during_concurrent_reads() {
        let control = FakeControl::new(now());
        let fake = FakeCalendarRead::new(
            control,
            Vec::<BusyInterval>::new(),
            [
                change("event-1", NOW),
                change("event-2", "2026-08-29T12:35:56Z"),
                change("event-3", "2026-08-29T12:36:56Z"),
            ],
        );
        let initial_request = CalendarSyncRequest::new(
            range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            None,
            1,
        )
        .expect("initial request");
        let first = fake
            .sync_calendar(&session(), &initial_request)
            .await
            .expect("initial page");
        let cursor = first.next_cursor().expect("initial cursor").to_owned();

        let mut joins = Vec::new();
        for _ in 0..8 {
            let worker = fake.clone();
            let request = CalendarSyncRequest::new(
                range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                Some(cursor.clone()),
                1,
            )
            .expect("clone request");
            joins.push(tokio::spawn(async move {
                worker
                    .sync_calendar(&session(), &request)
                    .await
                    .expect("clone page")
            }));
        }
        let expected = joins.remove(0).await.expect("first clone join");
        assert_eq!(expected.items()[0].provider_event_id(), "event-2");
        assert_eq!(expected.next_cursor(), Some("fake-calendar:2"));
        for join in joins {
            assert_eq!(join.await.expect("clone join"), expected);
        }
        let debug = format!("{fake:?}");
        assert!(debug.contains("emitted_cursor_count: 2"));
    }

    #[tokio::test]
    async fn poisoned_control_fails_closed_without_advancing_fake_cursor_state() {
        struct PanicWriter;

        impl fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let fake =
            FakeCalendarRead::new(control.clone(), [busy(100, 200)], [change("event-1", NOW)]);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());

        let request = CalendarSyncRequest::new(
            range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            None,
            1,
        )
        .expect("request");
        assert_eq!(
            futures_util::future::join(
                fake.sync_calendar(&session(), &request),
                fake.list_busy(
                    &session(),
                    &range("1970-01-01T00:00:00Z", "1970-01-01T00:10:00Z"),
                ),
            )
            .await,
            (
                Err(ProviderError::Unavailable),
                Err(ProviderError::Unavailable)
            )
        );
        let debug = format!("{fake:?}");
        assert!(debug.contains("change_count: 1"));
        assert!(debug.contains("emitted_cursor_count: 0"));
    }

    #[test]
    fn fake_debug_redacts_seeded_content_and_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FakeCalendarRead>();
        let control = FakeControl::new(now());
        let fake = FakeCalendarRead::new(
            control,
            [busy(100, 200)],
            [event_change(
                "sentinel-calendar-event-id",
                "sentinel-calendar-title",
            )],
        );
        let debug = format!("{fake:?}");
        assert!(debug.contains("busy_count: 1"));
        assert!(debug.contains("change_count: 1"));
        assert!(!debug.contains("sentinel-calendar-event-id"));
        assert!(!debug.contains("sentinel-calendar-title"));
        assert!(!debug.contains("sentinel-operation-key"));
        assert!(!debug.contains("sentinel-calendar-attendee@example.test"));
        assert!(!debug.contains("sentinel-session-token"));
    }

    #[tokio::test]
    async fn cloned_reads_share_deterministic_state() {
        let control = FakeControl::new(now());
        let fake = Arc::new(FakeCalendarRead::new(
            control.clone(),
            [busy(100, 200)],
            [change("event-1", NOW)],
        ));
        let request = CalendarSyncRequest::new(
            range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            None,
            1,
        )
        .expect("request");
        let mut joins = Vec::new();
        for _ in 0..8 {
            let worker = Arc::clone(&fake);
            let request = request.clone();
            joins.push(tokio::spawn(async move {
                worker
                    .sync_calendar(&session(), &request)
                    .await
                    .expect("sync")
            }));
        }
        let first = joins.remove(0).await.expect("join");
        for join in joins {
            assert_eq!(join.await.expect("join"), first);
        }
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarSync)
                .expect("count"),
            8
        );
    }

    fn owner_draft(key: &str, title: &str) -> OwnerEventDraft {
        OwnerEventDraft::new(
            key,
            title,
            range("2026-08-29T10:00:00Z", "2026-08-29T11:00:00Z"),
            "Australia/Sydney",
        )
        .expect("owner draft")
    }

    fn google_proposal_draft(key: &str, title: &str) -> GoogleProposalDraft {
        google_proposal_draft_with(
            key,
            title,
            range("2026-08-29T10:00:00Z", "2026-08-29T11:00:00Z"),
            "Australia/Sydney",
            "owner@example.test",
        )
    }

    fn google_proposal_draft_with(
        key: &str,
        title: &str,
        time_range: TimeRange,
        timezone: &str,
        owner: &str,
    ) -> GoogleProposalDraft {
        GoogleProposalDraft::from_owner(
            key,
            title,
            time_range,
            timezone,
            CalendarAttendee::needs_action(MailAddress::new(owner).expect("owner address")),
        )
        .expect("proposal draft")
    }

    fn provider_event_id(event: &CalendarEvent) -> ProviderEventId {
        ProviderEventId::new(event.provider_event_id()).expect("provider event ID")
    }

    fn promotion(
        event: &CalendarEvent,
        final_title: &str,
        requester: Option<CalendarAttendee>,
        expected_owner_acceptance: bool,
    ) -> GoogleProposalPromotion {
        GoogleProposalPromotion::new(
            event.provider_event_id(),
            final_title,
            requester,
            expected_owner_acceptance,
        )
        .expect("promotion")
    }

    #[tokio::test]
    async fn google_create_records_pending_event_for_reads() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );

        let event = fake
            .create_proposal(&google_proposal_draft("google-op-1", "Discuss"))
            .expect("pending proposal");

        assert!(
            event
                .provider_event_id()
                .starts_with("fake-google-proposal-event-")
        );
        assert_eq!(event.operation_key(), "google-op-1");
        assert_eq!(event.title(), "Discuss");
        assert_eq!(event.timezone(), "Australia/Sydney");
        assert_eq!(event.attendees().len(), 1);
        assert_eq!(event.attendees()[0].rsvp(), Rsvp::NeedsAction);
        assert_eq!(event.last_modified_at(), now());
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("count"),
            1
        );

        let busy = fake
            .list_busy(
                &session(),
                &range("2026-08-29T09:00:00Z", "2026-08-29T12:00:00Z"),
            )
            .await
            .expect("busy");
        assert_eq!(busy.len(), 1);
        let page = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync");
        assert_eq!(page.items().len(), 1);
        assert_eq!(page.items()[0].event(), Some(&event));
    }

    #[tokio::test]
    async fn google_create_skips_seeded_event_ids_and_deletes_only_created_identity() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control,
            Vec::<BusyInterval>::new(),
            [event_change("fake-google-proposal-event-1", "seeded")],
        );
        let draft = google_proposal_draft("google-seeded-id", "Discuss");

        let created = fake.create_proposal(&draft).expect("proposal");
        assert_eq!(created.provider_event_id(), "fake-google-proposal-event-2");

        let request = CalendarSyncRequest::new(
            range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            None,
            10,
        )
        .expect("sync request");
        let before_delete = fake
            .sync_calendar(&session(), &request)
            .await
            .expect("sync before delete");
        assert_eq!(
            before_delete
                .items()
                .iter()
                .map(CalendarChange::provider_event_id)
                .collect::<Vec<_>>(),
            vec![
                "fake-google-proposal-event-1",
                "fake-google-proposal-event-2"
            ]
        );

        let created_id = provider_event_id(&created);
        fake.delete_proposal(&created_id).expect("delete proposal");
        let after_delete = fake
            .sync_calendar(&session(), &request)
            .await
            .expect("sync after delete");
        assert_eq!(after_delete.items().len(), 3);
        assert_eq!(
            after_delete.items()[0].provider_event_id(),
            "fake-google-proposal-event-1"
        );
        assert_eq!(
            after_delete.items()[1].provider_event_id(),
            "fake-google-proposal-event-2"
        );
        assert_eq!(
            after_delete.items()[2].provider_event_id(),
            "fake-google-proposal-event-2"
        );
        assert!(after_delete.items()[2].event().is_none());
    }

    #[test]
    fn google_create_is_idempotent_and_changed_drafts_conflict_without_mutation() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let original = google_proposal_draft("google-op-retry", "Discuss");
        let first = fake.create_proposal(&original).expect("first proposal");
        let retry = fake
            .create_proposal(&google_proposal_draft("google-op-retry", "Discuss"))
            .expect("same proposal retry");
        assert_eq!(retry, first);

        let snapshot = format!("{fake:?}");
        let variants = [
            google_proposal_draft("google-op-retry", "Changed title"),
            google_proposal_draft_with(
                "google-op-retry",
                "Discuss",
                range("2026-08-29T11:00:00Z", "2026-08-29T12:00:00Z"),
                "Australia/Sydney",
                "owner@example.test",
            ),
            google_proposal_draft_with(
                "google-op-retry",
                "Discuss",
                range("2026-08-29T10:00:00Z", "2026-08-29T11:00:00Z"),
                "UTC",
                "owner@example.test",
            ),
            google_proposal_draft_with(
                "google-op-retry",
                "Discuss",
                range("2026-08-29T10:00:00Z", "2026-08-29T11:00:00Z"),
                "Australia/Sydney",
                "other-owner@example.test",
            ),
        ];
        for variant in variants {
            assert_eq!(fake.create_proposal(&variant), Err(ProviderError::Conflict));
            assert_eq!(format!("{fake:?}"), snapshot);
        }
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("count"),
            6
        );
    }

    #[tokio::test]
    async fn google_find_is_read_only_and_create_recovers_after_ambiguous_failure() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = google_proposal_draft("google-op-find", "Discuss");
        assert_eq!(
            GoogleCalendarProvider::find_proposal(&fake, &session(), &draft).await,
            Err(ProviderError::NotFound)
        );
        let first = GoogleCalendarProvider::create_proposal(&fake, &session(), &draft)
            .await
            .expect("create");
        let found = GoogleCalendarProvider::find_proposal(&fake, &session(), &draft)
            .await
            .expect("find");
        assert_eq!(found, first);
        let retry = GoogleCalendarProvider::create_proposal(&fake, &session(), &draft)
            .await
            .expect("ambiguous create retry");
        assert_eq!(retry, first);
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            2
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarProposalFind)
                .expect("find count"),
            2
        );
    }

    #[test]
    fn google_create_retry_uses_immutable_draft_after_rsvp_and_promotion() {
        let fake = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = google_proposal_draft("google-create-after-mutation", "Discuss");
        let pending = fake.create_proposal(&draft).expect("pending proposal");
        let id = provider_event_id(&pending);

        let accepted = fake
            .set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("owner acceptance");
        assert_eq!(fake.create_proposal(&draft), Ok(accepted.clone()));

        let promoted = fake
            .promote_proposal(&promotion(&pending, "Final", None, true))
            .expect("promotion");
        assert_eq!(fake.create_proposal(&draft), Ok(promoted.clone()));

        let changed = google_proposal_draft("google-create-after-mutation", "Changed");
        assert_eq!(fake.create_proposal(&changed), Err(ProviderError::Conflict));
        assert_eq!(
            fake.read
                .state
                .lock()
                .expect("state")
                .google_proposal_events,
            vec![promoted]
        );
    }

    #[tokio::test]
    async fn deleted_google_proposal_operation_key_cannot_be_recreated() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = google_proposal_draft("google-deleted-retry", "Discuss");
        let created = fake.create_proposal(&draft).expect("proposal");
        let id = provider_event_id(&created);

        fake.delete_proposal(&id).expect("delete proposal");
        let before_retry = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("delete history");
        assert_eq!(before_retry.items().len(), 2);

        assert_eq!(
            fake.create_proposal(&draft),
            Err(ProviderError::Conflict),
            "deleted operation keys must fail closed"
        );
        {
            let state = fake.read.state.lock().expect("state");
            assert!(state.google_proposal_events.is_empty());
            assert!(state.google_proposal_create_drafts.is_empty());
            assert_eq!(state.next_google_event_sequence, 2);
            assert!(
                state
                    .deleted_google_proposal_operation_keys
                    .contains(draft.operation_key())
            );
        }

        let after_retry = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("unchanged delete history");
        assert_eq!(after_retry, before_retry);
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            2
        );
    }

    #[tokio::test]
    async fn concurrent_identical_google_creates_share_one_event() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = google_proposal_draft("google-op-concurrent", "Discuss");
        let mut joins = Vec::new();
        for _ in 0..8 {
            let worker = fake.clone();
            let draft = draft.clone();
            joins.push(tokio::spawn(async move {
                worker.create_proposal(&draft).expect("concurrent proposal")
            }));
        }
        let first = joins.remove(0).await.expect("join");
        for join in joins {
            assert_eq!(join.await.expect("join"), first);
        }
        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("busy")
            .len(),
            1
        );
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request())
                .await
                .expect("sync")
                .items()
                .len(),
            1
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("count"),
            8
        );
    }

    #[tokio::test]
    async fn google_create_failures_leave_state_unchanged_and_recover() {
        let control = FakeControl::new(now());
        control
            .queue_failure(
                FakeOperation::CalendarProposalCreate,
                ProviderError::TokenExpired,
            )
            .expect("queue expiry");
        let retry_after = RetryAfter::new(Duration::seconds(13)).expect("retry delay");
        let throttled = ProviderError::throttled(retry_after);
        control
            .set_failure(FakeOperation::CalendarProposalCreate, throttled)
            .expect("set throttle");
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = google_proposal_draft("google-op-failures", "Discuss");

        assert_eq!(
            fake.create_proposal(&draft),
            Err(ProviderError::TokenExpired)
        );
        assert_eq!(fake.create_proposal(&draft), Err(throttled));
        assert_eq!(fake.create_proposal(&draft), Err(throttled));
        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("busy")
            .len(),
            0
        );
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request())
                .await
                .expect("sync")
                .items()
                .len(),
            0
        );

        control
            .clear_failure(FakeOperation::CalendarProposalCreate)
            .expect("clear throttle");
        let event = fake.create_proposal(&draft).expect("recovery");
        assert_eq!(event.last_modified_at(), now());
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request())
                .await
                .expect("recovered sync")
                .items()[0]
                .event(),
            Some(&event)
        );
    }

    #[tokio::test]
    async fn google_persistent_unavailable_leaves_state_unchanged_and_recovers() {
        let control = FakeControl::new(now());
        control
            .set_failure(
                FakeOperation::CalendarProposalCreate,
                ProviderError::Unavailable,
            )
            .expect("set unavailable");
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = google_proposal_draft("google-op-unavailable", "Unavailable");
        let busy_range = range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z");
        let debug_before = format!("{fake:?}");

        for expected_count in [1, 2] {
            assert_eq!(
                fake.create_proposal(&draft),
                Err(ProviderError::Unavailable)
            );
            assert_eq!(
                control
                    .invocation_count(FakeOperation::CalendarProposalCreate)
                    .expect("count"),
                expected_count
            );
            assert!(
                fake.list_busy(&session(), &busy_range)
                    .await
                    .expect("busy read")
                    .is_empty()
            );
            assert!(
                fake.sync_calendar(&session(), &sync_request())
                    .await
                    .expect("sync read")
                    .items()
                    .is_empty()
            );
            assert_eq!(format!("{fake:?}"), debug_before);
        }

        control
            .clear_failure(FakeOperation::CalendarProposalCreate)
            .expect("clear unavailable");
        let event = fake.create_proposal(&draft).expect("recovery");
        assert_eq!(event.last_modified_at(), now());
        assert_eq!(
            fake.list_busy(&session(), &busy_range)
                .await
                .expect("recovered busy")
                .len(),
            1
        );
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request())
                .await
                .expect("recovered sync")
                .items()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn google_poisoned_control_fails_closed_without_mutation_and_replacement_recovers() {
        struct PanicWriter;

        impl fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());

        let draft = google_proposal_draft("sentinel-poisoned-google-key", "Sentinel title");
        assert_eq!(
            fake.create_proposal(&draft),
            Err(ProviderError::Unavailable)
        );
        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await,
            Err(ProviderError::Unavailable)
        );
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request()).await,
            Err(ProviderError::Unavailable)
        );
        let debug = format!("{fake:?}");
        assert!(debug.contains("busy_count: 0"));
        assert!(debug.contains("change_count: 0"));
        assert!(debug.contains("proposal_event_count: 0"));
        assert!(!debug.contains("sentinel-poisoned-google-key"));
        assert!(!debug.contains("Sentinel title"));

        // Control mutex poisoning is terminal; a fresh fake is the explicit
        // replacement boundary and proves the draft itself remains usable.
        let replacement_control = FakeControl::new(now());
        let replacement = FakeGoogleCalendar::new(
            replacement_control,
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        assert!(replacement.create_proposal(&draft).is_ok());
    }

    #[tokio::test]
    async fn google_owner_rsvp_fixture_updates_existing_event_once_and_syncs() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control,
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let pending = fake
            .create_proposal(&google_proposal_draft("google-rsvp", "Discuss"))
            .expect("pending proposal");
        let id = provider_event_id(&pending);
        let accepted = fake
            .set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("owner acceptance");

        assert_eq!(accepted.provider_event_id(), pending.provider_event_id());
        assert_eq!(accepted.operation_key(), pending.operation_key());
        assert_eq!(accepted.title(), pending.title());
        assert_eq!(accepted.time_range(), pending.time_range());
        assert_eq!(accepted.timezone(), pending.timezone());
        assert_eq!(accepted.attendees().len(), 1);
        assert_eq!(accepted.attendees()[0].rsvp(), Rsvp::Accepted);
        assert_eq!(accepted.last_modified_at(), now());

        let retry = fake
            .set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("idempotent RSVP fixture retry");
        assert_eq!(retry, accepted);

        let page = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync");
        assert_eq!(page.items().len(), 2);
        assert_eq!(page.items()[1].event(), Some(&accepted));

        let missing = ProviderEventId::new("missing-google-event").expect("missing ID");
        assert_eq!(
            fake.set_owner_rsvp(&missing, Rsvp::Declined),
            Err(ProviderError::NotFound)
        );
        assert_eq!(page.items().len(), 2);
    }

    #[tokio::test]
    async fn google_owner_rsvp_fixture_rejects_every_post_promotion_mutation_without_changes() {
        let fake = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let pending = fake
            .create_proposal(&google_proposal_draft("google-rsvp-promoted", "Discuss"))
            .expect("pending proposal");
        let id = provider_event_id(&pending);
        fake.set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("owner acceptance");
        let promoted = fake
            .promote_proposal(&promotion(&pending, "Final", None, true))
            .expect("promotion");
        let busy_before = fake
            .list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("busy before RSVP mutation");
        let page_before = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync before RSVP mutation");

        for rsvp in [Rsvp::Accepted, Rsvp::Declined] {
            assert_eq!(fake.set_owner_rsvp(&id, rsvp), Err(ProviderError::Conflict));
            assert_eq!(
                fake.list_busy(
                    &session(),
                    &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                )
                .await
                .expect("busy after rejected RSVP mutation"),
                busy_before
            );
            assert_eq!(
                fake.sync_calendar(&session(), &sync_request())
                    .await
                    .expect("sync after rejected RSVP mutation"),
                page_before
            );
        }
        assert_eq!(
            page_before.items().last().and_then(|item| item.event()),
            Some(&promoted)
        );
    }

    #[test]
    fn google_owner_rsvp_fixture_fails_closed_when_control_is_poisoned() {
        struct PanicWriter;

        impl fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let pending = fake
            .create_proposal(&google_proposal_draft(
                "google-rsvp-poisoned-control",
                "Discuss",
            ))
            .expect("pending proposal");
        let id = provider_event_id(&pending);
        let before = {
            let state = fake.read.state.lock().expect("state before poison");
            (
                state.google_proposal_events.clone(),
                state.google_proposal_create_drafts.clone(),
                state.busy.clone(),
                state.changes.clone(),
            )
        };

        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());

        assert_eq!(
            fake.set_owner_rsvp(&id, Rsvp::Accepted),
            Err(ProviderError::Unavailable)
        );
        let state = fake.read.state.lock().expect("state after poison");
        assert_eq!(
            (
                state.google_proposal_events.clone(),
                state.google_proposal_create_drafts.clone(),
                state.busy.clone(),
                state.changes.clone(),
            ),
            before
        );
    }

    #[tokio::test]
    async fn google_promotion_queued_failures_do_not_mutate_and_retries_recover() {
        let throttled = ProviderError::throttled(
            RetryAfter::new(Duration::seconds(13)).expect("positive retry delay"),
        );
        for (key, failure) in [
            ("token-expired", ProviderError::TokenExpired),
            ("throttled", throttled),
            ("unavailable", ProviderError::Unavailable),
        ] {
            let control = FakeControl::new(now());
            let fake = FakeGoogleCalendar::new(
                control.clone(),
                Vec::<BusyInterval>::new(),
                Vec::<CalendarChange>::new(),
            );
            let pending = fake
                .create_proposal(&google_proposal_draft(
                    &format!("google-promote-queued-{key}"),
                    "Discuss",
                ))
                .expect("pending proposal");
            let id = provider_event_id(&pending);
            fake.set_owner_rsvp(&id, Rsvp::Accepted)
                .expect("owner acceptance");
            let request = promotion(&pending, "Final", None, true);
            let busy_before = fake
                .list_busy(
                    &session(),
                    &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                )
                .await
                .expect("busy before failed promotion");
            let page_before = fake
                .sync_calendar(&session(), &sync_request())
                .await
                .expect("sync before failed promotion");

            control
                .queue_failure(FakeOperation::CalendarPromote, failure)
                .expect("queue promotion failure");
            assert_eq!(fake.promote_proposal(&request), Err(failure));
            assert_eq!(
                fake.list_busy(
                    &session(),
                    &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                )
                .await
                .expect("busy after failed promotion"),
                busy_before
            );
            assert_eq!(
                fake.sync_calendar(&session(), &sync_request())
                    .await
                    .expect("sync after failed promotion"),
                page_before
            );

            let promoted = fake
                .promote_proposal(&request)
                .expect("recovered promotion");
            assert_eq!(promoted.provider_event_id(), pending.provider_event_id());
            assert_eq!(promoted.title(), "Final");
        }
    }

    #[tokio::test]
    async fn google_promotion_persistent_failures_do_not_mutate_and_clear_recovers() {
        let throttled = ProviderError::throttled(
            RetryAfter::new(Duration::seconds(13)).expect("positive retry delay"),
        );
        for (key, failure) in [
            ("token-expired", ProviderError::TokenExpired),
            ("throttled", throttled),
            ("unavailable", ProviderError::Unavailable),
        ] {
            let control = FakeControl::new(now());
            let fake = FakeGoogleCalendar::new(
                control.clone(),
                Vec::<BusyInterval>::new(),
                Vec::<CalendarChange>::new(),
            );
            let pending = fake
                .create_proposal(&google_proposal_draft(
                    &format!("google-promote-persistent-{key}"),
                    "Discuss",
                ))
                .expect("pending proposal");
            let id = provider_event_id(&pending);
            fake.set_owner_rsvp(&id, Rsvp::Accepted)
                .expect("owner acceptance");
            let request = promotion(&pending, "Final", None, true);
            let busy_before = fake
                .list_busy(
                    &session(),
                    &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                )
                .await
                .expect("busy before persistent promotion failure");
            let page_before = fake
                .sync_calendar(&session(), &sync_request())
                .await
                .expect("sync before persistent promotion failure");

            control
                .set_failure(FakeOperation::CalendarPromote, failure)
                .expect("set persistent promotion failure");
            for _ in 0..2 {
                assert_eq!(fake.promote_proposal(&request), Err(failure));
                assert_eq!(
                    fake.list_busy(
                        &session(),
                        &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                    )
                    .await
                    .expect("busy after persistent promotion failure"),
                    busy_before
                );
                assert_eq!(
                    fake.sync_calendar(&session(), &sync_request())
                        .await
                        .expect("sync after persistent promotion failure"),
                    page_before
                );
            }

            control
                .clear_failure(FakeOperation::CalendarPromote)
                .expect("clear persistent promotion failure");
            let promoted = fake
                .promote_proposal(&request)
                .expect("recovered promotion");
            assert_eq!(promoted.provider_event_id(), pending.provider_event_id());
            assert_eq!(promoted.title(), "Final");
        }
    }

    #[tokio::test]
    async fn google_promotion_poisoned_control_does_not_mutate_and_replacement_recovers() {
        struct PanicWriter;

        impl fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let pending = fake
            .create_proposal(&google_proposal_draft("google-promote-poisoned", "Discuss"))
            .expect("pending proposal");
        let id = provider_event_id(&pending);
        fake.set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("owner acceptance");
        let request = promotion(&pending, "Final", None, true);
        let busy_before = fake
            .list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("busy before poisoned promotion");
        let page_before = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync before poisoned promotion");
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());

        assert_eq!(
            fake.promote_proposal(&request),
            Err(ProviderError::Unavailable)
        );
        assert_eq!(
            fake.read
                .state
                .lock()
                .expect("calendar state remains available")
                .google_proposal_events[0],
            page_before
                .items()
                .last()
                .and_then(|item| item.event())
                .cloned()
                .expect("accepted event")
        );
        assert_eq!(
            fake.read
                .state
                .lock()
                .expect("calendar state remains available")
                .busy,
            busy_before
        );
        assert_eq!(
            fake.read
                .state
                .lock()
                .expect("calendar state remains available")
                .changes,
            page_before.items()
        );

        let replacement = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let replacement_pending = replacement
            .create_proposal(&google_proposal_draft("google-promote-poisoned", "Discuss"))
            .expect("replacement pending proposal");
        let replacement_id = provider_event_id(&replacement_pending);
        replacement
            .set_owner_rsvp(&replacement_id, Rsvp::Accepted)
            .expect("replacement owner acceptance");
        assert_eq!(
            replacement
                .promote_proposal(&promotion(&replacement_pending, "Final", None, true))
                .expect("replacement promotion")
                .title(),
            "Final"
        );
    }

    #[test]
    fn google_promotion_requires_one_accepted_owner_and_expected_acceptance() {
        for owner_rsvp in [Rsvp::NeedsAction, Rsvp::Declined, Rsvp::Tentative] {
            let fake = FakeGoogleCalendar::new(
                FakeControl::new(now()),
                Vec::<BusyInterval>::new(),
                Vec::<CalendarChange>::new(),
            );
            let pending = fake
                .create_proposal(&google_proposal_draft("google-gating", "Discuss"))
                .expect("pending proposal");
            let id = provider_event_id(&pending);
            if owner_rsvp != Rsvp::NeedsAction {
                fake.set_owner_rsvp(&id, owner_rsvp)
                    .expect("owner RSVP fixture");
            }
            let request = promotion(&pending, "Final", None, true);
            assert_eq!(
                fake.promote_proposal(&request),
                Err(ProviderError::Conflict)
            );
        }

        let fake = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let pending = fake
            .create_proposal(&google_proposal_draft(
                "google-false-expectation",
                "Discuss",
            ))
            .expect("pending proposal");
        let id = provider_event_id(&pending);
        fake.set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("owner acceptance");
        assert_eq!(
            fake.promote_proposal(&promotion(&pending, "Final", None, false)),
            Err(ProviderError::Conflict)
        );
    }

    #[test]
    fn google_promotion_rejects_missing_and_ambiguous_owner_state_without_mutation() {
        let fake = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let missing = GoogleProposalPromotion::new("missing-google-event", "Final", None, true)
            .expect("promotion");
        assert_eq!(
            fake.promote_proposal(&missing),
            Err(ProviderError::NotFound)
        );

        let pending = fake
            .create_proposal(&google_proposal_draft("google-ambiguous", "Discuss"))
            .expect("pending proposal");
        let ambiguous = CalendarEvent::new(
            pending.provider_event_id(),
            pending.operation_key(),
            pending.title(),
            pending.time_range().clone(),
            pending.timezone(),
            [
                CalendarAttendee::needs_action(
                    MailAddress::new("ambiguous-owner@example.test").expect("owner"),
                ),
                CalendarAttendee::needs_action(
                    MailAddress::new("ambiguous-second@example.test").expect("attendee"),
                ),
            ],
            now(),
        )
        .expect("ambiguous event");
        {
            let mut state = fake.read.state.lock().expect("state");
            state.google_proposal_events[0] = ambiguous.clone();
        }
        assert_eq!(
            fake.promote_proposal(&promotion(&ambiguous, "Final", None, true)),
            Err(ProviderError::Conflict)
        );
        assert_eq!(
            fake.read
                .state
                .lock()
                .expect("state")
                .google_proposal_events[0],
            ambiguous
        );
    }

    #[tokio::test]
    async fn google_callback_and_meeting_promotions_update_same_event_and_busy_once() {
        let callback = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let pending = callback
            .create_proposal(&google_proposal_draft("google-callback", "Discuss"))
            .expect("pending proposal");
        let id = provider_event_id(&pending);
        callback
            .set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("owner acceptance");
        let promoted = callback
            .promote_proposal(&promotion(&pending, "Callback", None, true))
            .expect("callback promotion");
        assert_eq!(promoted.provider_event_id(), pending.provider_event_id());
        assert_eq!(promoted.title(), "Callback");
        assert_eq!(promoted.attendees().len(), 1);
        assert_eq!(promoted.attendees()[0].rsvp(), Rsvp::Accepted);
        assert_eq!(
            callback
                .list_busy(
                    &session(),
                    &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                )
                .await
                .expect("busy")
                .len(),
            1
        );

        let meeting = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let pending = meeting
            .create_proposal(&google_proposal_draft("google-meeting", "Discuss"))
            .expect("pending proposal");
        let id = provider_event_id(&pending);
        meeting
            .set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("owner acceptance");
        let requester = CalendarAttendee::new(
            MailAddress::new("requester@example.test").expect("requester"),
            Rsvp::Tentative,
        )
        .expect("requester");
        let promoted = meeting
            .promote_proposal(&promotion(
                &pending,
                "Meeting",
                Some(requester.clone()),
                true,
            ))
            .expect("meeting promotion");
        assert_eq!(promoted.provider_event_id(), pending.provider_event_id());
        assert_eq!(promoted.title(), "Meeting");
        assert_eq!(promoted.attendees().len(), 2);
        assert_eq!(promoted.attendees()[0].rsvp(), Rsvp::Accepted);
        assert_eq!(promoted.attendees()[1], requester);
        assert_eq!(
            meeting
                .list_busy(
                    &session(),
                    &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                )
                .await
                .expect("busy")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn google_promotion_retry_and_changed_payloads_are_stable() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let pending = fake
            .create_proposal(&google_proposal_draft("google-promotion-retry", "Discuss"))
            .expect("pending proposal");
        let id = provider_event_id(&pending);
        fake.set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("owner acceptance");
        let requester = CalendarAttendee::new(
            MailAddress::new("retry-requester@example.test").expect("requester"),
            Rsvp::Accepted,
        )
        .expect("requester");
        let request = promotion(&pending, "Final", Some(requester.clone()), true);
        let promoted = fake.promote_proposal(&request).expect("promotion");
        let busy_before = fake
            .list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("busy before retry");
        let page_before = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync before retry");
        let debug_before = format!("{fake:?}");
        assert_eq!(fake.promote_proposal(&request), Ok(promoted.clone()));

        let changed_title = promotion(&pending, "Changed", Some(requester.clone()), true);
        let changed_requester = promotion(
            &pending,
            "Final",
            Some(
                CalendarAttendee::new(
                    MailAddress::new("other-requester@example.test").expect("requester"),
                    Rsvp::Accepted,
                )
                .expect("requester"),
            ),
            true,
        );
        for changed in [changed_title, changed_requester] {
            assert_eq!(
                fake.promote_proposal(&changed),
                Err(ProviderError::Conflict)
            );
            assert_eq!(
                fake.list_busy(
                    &session(),
                    &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                )
                .await
                .expect("busy after conflict"),
                busy_before
            );
            assert_eq!(
                fake.sync_calendar(&session(), &sync_request())
                    .await
                    .expect("sync after conflict"),
                page_before
            );
            assert_eq!(format!("{fake:?}"), debug_before);
        }
        assert_eq!(promoted.provider_event_id(), pending.provider_event_id());
        assert_eq!(promoted.attendees().len(), 2);
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarPromote)
                .expect("promote count"),
            4
        );
    }

    #[tokio::test]
    async fn google_delete_removes_only_its_identical_busy_contribution_and_syncs_one_tombstone() {
        let control = FakeControl::new(now());
        let identical = busy(1_787_997_600, 1_788_001_200);
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            [identical, identical],
            Vec::<CalendarChange>::new(),
        );
        let event = fake
            .create_proposal(&google_proposal_draft("google-delete", "Discuss"))
            .expect("proposal");
        let event_id = provider_event_id(&event);

        fake.delete_proposal(&event_id).expect("first delete");

        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T09:00:00Z", "2026-08-29T12:00:00Z"),
            )
            .await
            .expect("busy after delete"),
            vec![identical, identical]
        );
        let page = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync after delete");
        assert_eq!(page.items().len(), 2);
        assert_eq!(page.items()[0].event(), Some(&event));
        assert!(page.items()[1].is_deleted());
        assert_eq!(
            page.items()[1].provider_event_id(),
            event.provider_event_id()
        );
        assert_eq!(page.items()[1].changed_at(), now());
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarDelete)
                .expect("delete count"),
            1
        );

        fake.delete_proposal(&event_id).expect("idempotent delete");
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request())
                .await
                .expect("sync after repeat")
                .items(),
            page.items()
        );
        assert_eq!(
            fake.delete_proposal(&ProviderEventId::new("never-known").expect("ID")),
            Err(ProviderError::NotFound)
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarDelete)
                .expect("delete count"),
            3
        );
    }

    #[tokio::test]
    async fn google_busy_tracks_provider_identity_when_event_order_changes() {
        let fake = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let first = fake
            .create_proposal(&google_proposal_draft_with(
                "google-order-first",
                "First",
                range("2026-08-29T10:00:00Z", "2026-08-29T11:00:00Z"),
                "Australia/Sydney",
                "owner@example.test",
            ))
            .expect("first proposal");
        let second = fake
            .create_proposal(&google_proposal_draft_with(
                "google-order-second",
                "Second",
                range("2026-08-29T12:00:00Z", "2026-08-29T13:00:00Z"),
                "Australia/Sydney",
                "owner@example.test",
            ))
            .expect("second proposal");
        let first_id = provider_event_id(&first);
        let second_id = provider_event_id(&second);

        // A provider refresh can reorder event records. Keep the lifecycle
        // vectors aligned while deliberately leaving busy state independent
        // of that ordering; deletion must follow the provider event ID.
        {
            let mut state = fake.read.state.lock().expect("calendar state");
            state.google_proposal_events.swap(0, 1);
            state.google_proposal_create_drafts.swap(0, 1);
            state.google_promotion_requests.swap(0, 1);
        }

        fake.delete_proposal(&first_id)
            .expect("delete first proposal");
        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T09:00:00Z", "2026-08-29T14:00:00Z"),
            )
            .await
            .expect("busy after reordered delete"),
            vec![busy(1_788_004_800, 1_788_008_400)]
        );
        let page = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync after reordered delete");
        assert_eq!(page.items().len(), 3);
        assert_eq!(
            page.items()[0].provider_event_id(),
            first.provider_event_id()
        );
        assert_eq!(
            page.items()[1].provider_event_id(),
            second.provider_event_id()
        );
        assert_eq!(page.items()[2].provider_event_id(), first_id.as_str());
        assert!(page.items()[2].is_deleted());

        fake.delete_proposal(&second_id)
            .expect("delete remaining proposal");
        assert!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T09:00:00Z", "2026-08-29T14:00:00Z"),
            )
            .await
            .expect("busy after second delete")
            .is_empty()
        );
    }

    #[tokio::test]
    async fn google_trait_object_deletes_lifecycle_event_once_and_failures_do_not_mutate() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let google: &dyn GoogleCalendarProvider = &fake;
        let draft = google_proposal_draft("google-trait-delete", "Discuss");
        let event = google
            .create_proposal(&session(), &draft)
            .await
            .expect("trait create");
        let id = provider_event_id(&event);
        fake.set_owner_rsvp(&id, Rsvp::Accepted)
            .expect("acceptance");
        let requester = CalendarAttendee::new(
            MailAddress::new("requester@example.test").expect("requester address"),
            Rsvp::Accepted,
        )
        .expect("requester");
        let promoted = google
            .promote_proposal(
                &session(),
                &promotion(&event, "Final", Some(requester), true),
            )
            .await
            .expect("trait promotion");
        assert_eq!(promoted.provider_event_id(), event.provider_event_id());
        assert_eq!(promoted.attendees().len(), 2);

        control
            .queue_failure(FakeOperation::CalendarDelete, ProviderError::TokenExpired)
            .expect("delete failure");
        assert_eq!(
            google.delete_proposal(&session(), &id).await,
            Err(ProviderError::TokenExpired)
        );
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request())
                .await
                .expect("unchanged sync")
                .items()
                .last()
                .and_then(CalendarChange::event),
            Some(&promoted)
        );

        google
            .delete_proposal(&session(), &id)
            .await
            .expect("trait delete");
        let clone = fake.clone();
        clone.delete_proposal(&id).expect("clone idempotent delete");
        let page = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("tombstone sync");
        assert_eq!(page.items().len(), 4);
        assert!(page.items()[3].is_deleted());
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarProposalCreate)
                .expect("create count"),
            1
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarPromote)
                .expect("promote count"),
            1
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarDelete)
                .expect("delete count"),
            3
        );
    }

    #[tokio::test]
    async fn concurrent_identical_google_deletes_share_one_tombstone() {
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let event = fake
            .create_proposal(&google_proposal_draft(
                "google-concurrent-delete",
                "Discuss",
            ))
            .expect("proposal");
        let event_id = provider_event_id(&event);
        let mut joins = Vec::new();
        for _ in 0..8 {
            let worker = fake.clone();
            let event_id = event_id.clone();
            joins.push(tokio::spawn(
                async move { worker.delete_proposal(&event_id) },
            ));
        }
        for join in joins {
            assert_eq!(join.await.expect("join"), Ok(()));
        }
        let page = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync");
        assert_eq!(page.items().len(), 2);
        assert_eq!(
            page.items()
                .iter()
                .filter(|change| change.is_deleted())
                .count(),
            1
        );
        assert!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T09:00:00Z", "2026-08-29T12:00:00Z"),
            )
            .await
            .expect("busy")
            .is_empty()
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarDelete)
                .expect("delete count"),
            8
        );
    }

    #[tokio::test]
    async fn google_create_delete_create_never_reuses_a_tombstoned_provider_id() {
        let fake = FakeGoogleCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let first = fake
            .create_proposal(&google_proposal_draft("google-sequence-1", "First"))
            .expect("first proposal");
        let first_id = provider_event_id(&first);
        fake.delete_proposal(&first_id).expect("delete first");

        let second = fake
            .create_proposal(&google_proposal_draft("google-sequence-2", "Second"))
            .expect("second proposal");
        let second_id = provider_event_id(&second);
        assert_ne!(second.provider_event_id(), first.provider_event_id());
        fake.delete_proposal(&first_id)
            .expect("first tombstone remains idempotent");
        fake.delete_proposal(&second_id)
            .expect("second proposal deletes independently");
    }

    #[test]
    fn google_fake_debug_is_redacted_and_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FakeGoogleCalendar>();
        let control = FakeControl::new(now());
        let fake = FakeGoogleCalendar::new(
            control,
            Vec::<BusyInterval>::new(),
            [CalendarChange::upsert(
                CalendarEvent::new(
                    "sentinel-google-proposal-event-id",
                    "sentinel-google-proposal-operation-key",
                    "sentinel-google-proposal-title",
                    range("2026-08-29T10:00:00Z", "2026-08-29T11:00:00Z"),
                    "Australia/Sydney",
                    [CalendarAttendee::needs_action(
                        MailAddress::new("sentinel-google-owner@example.test")
                            .expect("owner address"),
                    )],
                    now(),
                )
                .expect("sentinel event"),
            )
            .expect("sentinel change")],
        );
        let debug = format!("{fake:?}");
        assert!(debug.contains("busy_count: 0"));
        assert!(debug.contains("change_count: 1"));
        assert!(debug.contains("proposal_event_count: 0"));
        assert!(!debug.contains("sentinel-google-proposal-event-id"));
        assert!(!debug.contains("sentinel-google-proposal-operation-key"));
        assert!(!debug.contains("sentinel-google-proposal-title"));
        assert!(!debug.contains("sentinel-google-owner@example.test"));
        assert!(!debug.contains("Australia/Sydney"));
        assert!(!debug.contains("sentinel-session-token"));
    }

    fn sync_request() -> CalendarSyncRequest {
        CalendarSyncRequest::new(
            range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            None,
            10,
        )
        .expect("sync request")
    }

    #[tokio::test]
    async fn outlook_create_reads_busy_and_syncs_one_owner_event() {
        let control = FakeControl::new(now());
        let fake = FakeOutlookCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let event = fake
            .create_owner_event(&session(), &owner_draft("outlook-op-1", "Focus time"))
            .await
            .expect("owner event");

        assert!(event.provider_event_id().starts_with("fake-outlook-owner-"));
        assert_eq!(event.operation_key(), "outlook-op-1");
        assert_eq!(event.title(), "Focus time");
        assert_eq!(event.timezone(), "Australia/Sydney");
        assert!(event.attendees().is_empty());
        assert_eq!(event.last_modified_at(), now());
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarOwnerCreate)
                .expect("count"),
            1
        );

        let busy = fake
            .list_busy(
                &session(),
                &range("2026-08-29T09:00:00Z", "2026-08-29T12:00:00Z"),
            )
            .await
            .expect("busy");
        assert_eq!(busy.len(), 1);
        let page = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync");
        assert_eq!(page.items().len(), 1);
        assert_eq!(page.items()[0].event(), Some(&event));
    }

    #[tokio::test]
    async fn outlook_create_is_idempotent_and_conflicts_without_mutation() {
        let control = FakeControl::new(now());
        let fake = FakeOutlookCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let first = fake
            .create_owner_event(&session(), &owner_draft("outlook-op-1", "Focus time"))
            .await
            .expect("first create");
        let retry = fake
            .create_owner_event(&session(), &owner_draft("outlook-op-1", "Focus time"))
            .await
            .expect("idempotent retry");
        assert_eq!(retry, first);

        let busy_before = fake
            .list_busy(
                &session(),
                &range("2026-08-29T09:00:00Z", "2026-08-29T12:00:00Z"),
            )
            .await
            .expect("busy before conflict");
        let page_before = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync before conflict");
        let debug_before = format!("{fake:?}");
        let owner_create_count_before = control
            .invocation_count(FakeOperation::CalendarOwnerCreate)
            .expect("owner-create count before conflict");
        let busy_count_before = control
            .invocation_count(FakeOperation::CalendarBusy)
            .expect("busy count before conflict");
        let sync_count_before = control
            .invocation_count(FakeOperation::CalendarSync)
            .expect("sync count before conflict");

        assert_eq!(
            fake.create_owner_event(&session(), &owner_draft("outlook-op-1", "Changed"))
                .await,
            Err(ProviderError::Conflict)
        );
        let busy_after = fake
            .list_busy(
                &session(),
                &range("2026-08-29T09:00:00Z", "2026-08-29T12:00:00Z"),
            )
            .await
            .expect("busy after conflict");
        let page_after = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync");
        let debug_after = format!("{fake:?}");

        assert_eq!(busy_after, busy_before);
        assert_eq!(page_after, page_before);
        assert_eq!(page_after.items(), page_before.items());
        assert_eq!(
            page_after.items()[0].provider_event_id(),
            first.provider_event_id()
        );
        assert_eq!(page_after.items()[0].changed_at(), first.last_modified_at());
        assert_eq!(page_after.items()[0].event(), Some(&first));
        assert_eq!(debug_after, debug_before);
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarOwnerCreate)
                .expect("count"),
            owner_create_count_before + 1
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarBusy)
                .expect("busy count"),
            busy_count_before + 1
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarSync)
                .expect("sync count"),
            sync_count_before + 1
        );
    }

    #[tokio::test]
    async fn outlook_find_returns_operation_match_for_service_side_validation() {
        let fake = FakeOutlookCalendar::new(
            FakeControl::new(now()),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let created = fake
            .create_owner_event(
                &session(),
                &owner_draft("outlook-find-operation", "Original"),
            )
            .await
            .expect("owner event");
        let response = fake
            .find_owner_event(
                &session(),
                &owner_draft("outlook-find-operation", "Changed"),
            )
            .await
            .expect("operation-key lookup");
        assert_eq!(response, created);
    }

    #[tokio::test]
    async fn concurrent_identical_outlook_creates_share_one_event() {
        let control = FakeControl::new(now());
        let fake = FakeOutlookCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = owner_draft("outlook-op-concurrent", "Focus time");
        let mut joins = Vec::new();
        for _ in 0..8 {
            let worker = fake.clone();
            let draft = draft.clone();
            joins.push(tokio::spawn(async move {
                worker
                    .create_owner_event(&session(), &draft)
                    .await
                    .expect("concurrent create")
            }));
        }
        let first = joins.remove(0).await.expect("join");
        for join in joins {
            assert_eq!(join.await.expect("join"), first);
        }
        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("busy")
            .len(),
            1
        );
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request())
                .await
                .expect("sync")
                .items()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn outlook_failures_leave_state_unchanged_and_recover() {
        let control = FakeControl::new(now());
        control
            .queue_failure(
                FakeOperation::CalendarOwnerCreate,
                ProviderError::TokenExpired,
            )
            .expect("queue expiry");
        let fake = FakeOutlookCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = owner_draft("outlook-op-failures", "Focus time");
        assert_eq!(
            fake.create_owner_event(&session(), &draft).await,
            Err(ProviderError::TokenExpired)
        );
        control
            .set_failure(
                FakeOperation::CalendarOwnerCreate,
                ProviderError::Unavailable,
            )
            .expect("set unavailable");
        assert_eq!(
            fake.create_owner_event(&session(), &draft).await,
            Err(ProviderError::Unavailable)
        );
        control
            .clear_failure(FakeOperation::CalendarOwnerCreate)
            .expect("clear failure");
        let event = fake
            .create_owner_event(&session(), &draft)
            .await
            .expect("recovery");
        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("busy")
            .len(),
            1
        );
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request())
                .await
                .expect("sync")
                .items()[0]
                .event(),
            Some(&event)
        );
    }

    #[tokio::test]
    async fn outlook_throttle_preserves_state_and_recovers_after_clear() {
        let control = FakeControl::new(now());
        let retry_after = RetryAfter::new(Duration::seconds(13)).expect("retry delay");
        let throttled = ProviderError::throttled(retry_after);
        control
            .set_failure(FakeOperation::CalendarOwnerCreate, throttled)
            .expect("set throttle");
        let fake = FakeOutlookCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let draft = owner_draft("outlook-op-throttle", "Throttle recovery");
        let busy_before = fake
            .list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("busy before throttle");
        let page_before = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("sync before throttle");
        let debug_before = format!("{fake:?}");

        for expected_count in [1, 2] {
            let error = fake
                .create_owner_event(&session(), &draft)
                .await
                .expect_err("persistent throttle");
            assert_eq!(error, throttled);
            assert_eq!(error.retry_after(), Some(retry_after));
            assert_eq!(
                error.retry_after().expect("retry metadata").duration(),
                Duration::seconds(13)
            );
            assert_eq!(
                control
                    .invocation_count(FakeOperation::CalendarOwnerCreate)
                    .expect("owner-create count"),
                expected_count
            );
            assert_eq!(
                fake.list_busy(
                    &session(),
                    &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
                )
                .await
                .expect("busy after throttle"),
                busy_before
            );
            assert_eq!(
                fake.sync_calendar(&session(), &sync_request())
                    .await
                    .expect("sync after throttle"),
                page_before
            );
            assert_eq!(format!("{fake:?}"), debug_before);
        }

        control
            .clear_failure(FakeOperation::CalendarOwnerCreate)
            .expect("clear throttle");
        let event = fake
            .create_owner_event(&session(), &draft)
            .await
            .expect("throttle recovery");
        assert_eq!(event.last_modified_at(), now());
        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarOwnerCreate)
                .expect("recovery count"),
            3
        );
        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await
            .expect("recovered busy")
            .len(),
            1
        );
        let recovered_page = fake
            .sync_calendar(&session(), &sync_request())
            .await
            .expect("recovered sync");
        assert_eq!(recovered_page.items().len(), 1);
        assert_eq!(recovered_page.items()[0].event(), Some(&event));
    }

    #[tokio::test]
    async fn poisoned_control_outlook_create_fails_closed_without_mutation() {
        struct PanicWriter;

        impl fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let fake = FakeOutlookCalendar::new(
            control.clone(),
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());

        let draft = owner_draft("poisoned-outlook-operation-key", "Poisoned title");
        let create = std::panic::AssertUnwindSafe(fake.create_owner_event(&session(), &draft))
            .catch_unwind()
            .await;
        assert!(matches!(create, Ok(Err(ProviderError::Unavailable))));
        assert_eq!(
            fake.list_busy(
                &session(),
                &range("2026-08-29T00:00:00Z", "2026-08-30T00:00:00Z"),
            )
            .await,
            Err(ProviderError::Unavailable)
        );
        assert_eq!(
            fake.sync_calendar(&session(), &sync_request()).await,
            Err(ProviderError::Unavailable)
        );
        let debug = format!("{fake:?}");
        assert!(debug.contains("busy_count: 0"));
        assert!(debug.contains("change_count: 0"));
        assert!(debug.contains("owner_event_count: 0"));
        assert!(!debug.contains("poisoned-outlook-operation-key"));
        assert!(!debug.contains("Poisoned title"));

        // A poisoned mutex is intentionally terminal for this control; a
        // fresh deterministic control/fake provides the explicit recovery
        // boundary without weakening the fail-closed behavior above.
        let fresh_control = FakeControl::new(now());
        let fresh_fake = FakeOutlookCalendar::new(
            fresh_control,
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        assert!(
            fresh_fake
                .create_owner_event(&session(), &draft)
                .await
                .is_ok()
        );
    }

    #[test]
    fn outlook_fake_debug_is_redacted_and_trait_surface_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_send(future: &ProviderFuture<'_, CalendarEvent>) {
            fn assert_inner<T: Send>(_: &T) {}
            assert_inner(future);
        }
        assert_send_sync::<FakeOutlookCalendar>();
        let control = FakeControl::new(now());
        let fake = FakeOutlookCalendar::new(
            control,
            Vec::<BusyInterval>::new(),
            Vec::<CalendarChange>::new(),
        );
        let outlook: &dyn OutlookCalendarProvider = &fake;
        let draft = owner_draft("sentinel-outlook-operation-key", "sentinel-title");
        let session = session();
        let future = outlook.create_owner_event(&session, &draft);
        assert_send(&future);
        drop(future);
        let debug = format!("{fake:?}");
        assert!(debug.contains("busy_count: 0"));
        assert!(debug.contains("change_count: 0"));
        assert!(!debug.contains("sentinel-outlook-operation-key"));
        assert!(!debug.contains("sentinel-title"));
        assert!(!debug.contains("sentinel-session-token"));
    }
}
