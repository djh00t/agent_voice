//! Shared deterministic controls for personal-assistant provider fakes.
//!
//! A control has no provider payloads or wall-clock access. It only records
//! operation metadata and lets a test schedule closed [`ProviderError`]
//! outcomes for a named operation.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};

use crate::pa::providers::{ProviderError, ProviderResult};

/// The closed set of operations that deterministic PA fakes may execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FakeOperation {
    /// Incremental mail synchronization.
    MailSync,
    /// Sending one PA Gmail message.
    MailSend,
    /// Adding or removing Gmail labels.
    MailLabels,
    /// Reading calendar busy intervals.
    CalendarBusy,
    /// Incremental calendar synchronization.
    CalendarSync,
    /// Creating an owner-only Outlook calendar event.
    CalendarOwnerCreate,
    /// Looking up an owner-only Outlook calendar event.
    CalendarOwnerFind,
    /// Creating a pending Google Calendar proposal.
    CalendarProposalCreate,
    /// Looking up a pending Google proposal by its idempotency key.
    CalendarProposalFind,
    /// Promoting an existing Google Calendar proposal.
    CalendarPromote,
    /// Deleting an existing Google Calendar proposal.
    CalendarDelete,
    /// Classifying one message through the structured triage provider.
    TriageClassify,
    /// Uploading one encrypted backup object.
    BackupPut,
}

impl FakeOperation {
    /// Every supported operation in stable debug and state-array order.
    pub const ALL: [Self; 13] = [
        Self::MailSync,
        Self::MailSend,
        Self::MailLabels,
        Self::CalendarBusy,
        Self::CalendarSync,
        Self::CalendarOwnerCreate,
        Self::CalendarOwnerFind,
        Self::CalendarProposalCreate,
        Self::CalendarProposalFind,
        Self::CalendarPromote,
        Self::CalendarDelete,
        Self::TriageClassify,
        Self::BackupPut,
    ];

    const fn index(self) -> usize {
        match self {
            Self::MailSync => 0,
            Self::MailSend => 1,
            Self::MailLabels => 2,
            Self::CalendarBusy => 3,
            Self::CalendarSync => 4,
            Self::CalendarOwnerCreate => 5,
            Self::CalendarOwnerFind => 6,
            Self::CalendarProposalCreate => 7,
            Self::CalendarProposalFind => 8,
            Self::CalendarPromote => 9,
            Self::CalendarDelete => 10,
            Self::TriageClassify => 11,
            Self::BackupPut => 12,
        }
    }

    /// Alias for the mail label mutation name used by some fake adapters.
    #[allow(non_upper_case_globals)]
    pub const MailModifyLabels: Self = Self::MailLabels;

    /// Alias for the direct owner event operation name.
    #[allow(non_upper_case_globals)]
    pub const CalendarCreateOwner: Self = Self::CalendarOwnerCreate;

    /// Alias for the pending proposal creation operation name.
    #[allow(non_upper_case_globals)]
    pub const CalendarCreateProposal: Self = Self::CalendarProposalCreate;

    /// Alias for the proposal promotion operation name.
    #[allow(non_upper_case_globals)]
    pub const CalendarPromoteProposal: Self = Self::CalendarPromote;

    /// Alias for the proposal deletion operation name.
    #[allow(non_upper_case_globals)]
    pub const CalendarDeleteProposal: Self = Self::CalendarDelete;
}

#[derive(Clone, Default)]
struct OperationState {
    count: u64,
    queued_failures: VecDeque<ProviderError>,
    persistent_failure: Option<ProviderError>,
    partial_failure_after: Option<usize>,
}

struct FakeState {
    operations: [OperationState; FakeOperation::ALL.len()],
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            operations: std::array::from_fn(|_| OperationState::default()),
        }
    }
}

/// Cloneable, deterministic state shared by provider fakes.
///
/// The supplied instant is copied into every clone and is never obtained from
/// the wall clock. Mutable operation state is protected by one standard
/// mutex; lock poisoning is converted to [`ProviderError::Unavailable`].
#[derive(Clone)]
pub struct FakeControl {
    now: DateTime<Utc>,
    state: Arc<Mutex<FakeState>>,
}

impl FakeControl {
    /// Creates a control plane with a fixed UTC instant.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now,
            state: Arc::new(Mutex::new(FakeState::default())),
        }
    }

    /// Returns the fixed instant supplied at construction.
    pub const fn now(&self) -> DateTime<Utc> {
        self.now
    }

    /// Queues one one-shot closed provider failure for `operation`.
    ///
    /// Queued failures are consumed in insertion order by [`Self::begin`].
    pub fn queue_failure(
        &self,
        operation: FakeOperation,
        failure: ProviderError,
    ) -> ProviderResult<()> {
        let mut state = self.lock_state()?;
        state.operations[operation.index()]
            .queued_failures
            .push_back(failure);
        Ok(())
    }

    /// Sets or replaces the persistent failure for `operation`.
    pub fn set_failure(
        &self,
        operation: FakeOperation,
        failure: ProviderError,
    ) -> ProviderResult<()> {
        let mut state = self.lock_state()?;
        state.operations[operation.index()].persistent_failure = Some(failure);
        Ok(())
    }

    /// Clears all injected failures for `operation`.
    pub fn clear_failure(&self, operation: FakeOperation) -> ProviderResult<()> {
        let mut state = self.lock_state()?;
        let operation = &mut state.operations[operation.index()];
        operation.queued_failures.clear();
        operation.persistent_failure = None;
        Ok(())
    }

    /// Clears both queued and persistent failures for `operation`.
    pub fn clear_all_failures(&self, operation: FakeOperation) -> ProviderResult<()> {
        self.clear_failure(operation)
    }

    /// Clears only the persistent failure, retaining queued FIFO failures.
    pub fn clear_persistent_failure(&self, operation: FakeOperation) -> ProviderResult<()> {
        let mut state = self.lock_state()?;
        state.operations[operation.index()].persistent_failure = None;
        Ok(())
    }

    /// Sets a non-negative partial-page failure boundary for `operation`.
    ///
    /// A fake can use the boundary to emit the first `after_items` items and
    /// then report its closed partial-page failure. A zero boundary reports a
    /// failure before any item succeeds and is useful for retrying from the
    /// initial cursor position.
    pub fn set_partial_failure(
        &self,
        operation: FakeOperation,
        after_items: usize,
    ) -> ProviderResult<()> {
        let mut state = self.lock_state()?;
        state.operations[operation.index()].partial_failure_after = Some(after_items);
        Ok(())
    }

    /// Returns the operation-scoped partial-page failure boundary, if set.
    pub fn partial_failure_after(&self, operation: FakeOperation) -> ProviderResult<Option<usize>> {
        let state = self.lock_state()?;
        Ok(state.operations[operation.index()].partial_failure_after)
    }

    /// Clears the partial-page failure boundary for `operation`.
    pub fn clear_partial_failure(&self, operation: FakeOperation) -> ProviderResult<()> {
        let mut state = self.lock_state()?;
        state.operations[operation.index()].partial_failure_after = None;
        Ok(())
    }

    /// Begins one operation atomically.
    ///
    /// The invocation count is incremented exactly once while holding the
    /// mutex. A queued failure wins over a persistent failure; when neither
    /// exists the call succeeds. A poisoned mutex returns only the fixed
    /// closed unavailable error.
    pub fn begin(&self, operation: FakeOperation) -> ProviderResult<()> {
        let mut state = self.lock_state()?;
        let operation = &mut state.operations[operation.index()];
        operation.count = operation.count.saturating_add(1);
        if let Some(failure) = operation.queued_failures.pop_front() {
            return Err(failure);
        }
        if let Some(failure) = operation.persistent_failure {
            return Err(failure);
        }
        Ok(())
    }

    /// Returns the total number of calls begun for `operation`.
    pub fn invocation_count(&self, operation: FakeOperation) -> ProviderResult<u64> {
        let state = self.lock_state()?;
        Ok(state.operations[operation.index()].count)
    }

    /// Alias for [`Self::invocation_count`].
    pub fn count(&self, operation: FakeOperation) -> ProviderResult<u64> {
        self.invocation_count(operation)
    }

    /// Alias for [`Self::set_failure`].
    pub fn set_persistent_failure(
        &self,
        operation: FakeOperation,
        failure: ProviderError,
    ) -> ProviderResult<()> {
        self.set_failure(operation, failure)
    }

    /// Alias for [`Self::set_partial_failure`].
    pub fn set_partial_page_failure(
        &self,
        operation: FakeOperation,
        after_items: usize,
    ) -> ProviderResult<()> {
        self.set_partial_failure(operation, after_items)
    }

    /// Alias for [`Self::partial_failure_after`].
    pub fn partial_page_failure_after(
        &self,
        operation: FakeOperation,
    ) -> ProviderResult<Option<usize>> {
        self.partial_failure_after(operation)
    }

    /// Alias for [`Self::clear_partial_failure`].
    pub fn clear_partial_page_failure(&self, operation: FakeOperation) -> ProviderResult<()> {
        self.clear_partial_failure(operation)
    }

    fn lock_state(&self) -> ProviderResult<MutexGuard<'_, FakeState>> {
        self.state.lock().map_err(|_| ProviderError::Unavailable)
    }

    #[cfg(test)]
    fn poison_for_test(&self) {
        let _guard = self.state.lock().expect("state is not already poisoned");
        panic!("intentional test poison");
    }
}

impl fmt::Debug for FakeControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return formatter.write_str("FakeControl { state: unavailable }"),
        };
        let entries: Vec<_> = FakeOperation::ALL
            .into_iter()
            .map(|operation| {
                let metadata = &state.operations[operation.index()];
                DebugOperation {
                    operation,
                    count: metadata.count,
                    mode: metadata.mode(),
                }
            })
            .collect();
        formatter
            .debug_struct("FakeControl")
            .field("operations", &entries)
            .finish()
    }
}

struct DebugOperation {
    operation: FakeOperation,
    count: u64,
    mode: &'static str,
}

impl fmt::Debug for DebugOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Operation")
            .field("operation", &self.operation)
            .field("count", &self.count)
            .field("mode", &self.mode)
            .finish()
    }
}

impl OperationState {
    fn mode(&self) -> &'static str {
        match (
            !self.queued_failures.is_empty(),
            self.persistent_failure.is_some(),
            self.partial_failure_after.is_some(),
        ) {
            (false, false, false) => "ready",
            (true, false, false) => "queued",
            (false, true, false) => "persistent",
            (true, true, false) => "queued+persistent",
            (false, false, true) => "partial",
            (true, false, true) => "queued+partial",
            (false, true, true) => "persistent+partial",
            (true, true, true) => "queued+persistent+partial",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FakeControl, FakeOperation};
    use crate::pa::providers::{ProviderError, ProviderInputField};
    use chrono::{DateTime, Duration, Utc};
    use std::sync::Arc;
    use std::thread;

    const NOW: &str = "2026-08-29T12:34:56Z";

    fn now() -> DateTime<Utc> {
        NOW.parse().expect("valid UTC instant")
    }

    #[test]
    fn fixed_time_is_stable_across_clones() {
        let control = FakeControl::new(now());
        let clone = control.clone();

        assert_eq!(control.now(), now());
        assert_eq!(clone.now(), now());
    }

    #[test]
    fn queued_failures_are_fifo_then_persistent_and_isolated() {
        let control = FakeControl::new(now());
        let queued_one = ProviderError::TokenExpired;
        let queued_two = ProviderError::CursorExpired;
        let persistent = ProviderError::Unavailable;

        control
            .queue_failure(FakeOperation::MailSync, queued_one)
            .expect("queue first");
        control
            .queue_failure(FakeOperation::MailSync, queued_two)
            .expect("queue second");
        control
            .set_failure(FakeOperation::MailSync, persistent)
            .expect("set persistent");

        assert_eq!(control.begin(FakeOperation::MailSync), Err(queued_one));
        assert_eq!(control.begin(FakeOperation::MailSync), Err(queued_two));
        assert_eq!(control.begin(FakeOperation::MailSync), Err(persistent));
        assert_eq!(control.begin(FakeOperation::MailSend), Ok(()));

        control
            .clear_failure(FakeOperation::MailSync)
            .expect("clear failure");
        assert_eq!(control.begin(FakeOperation::MailSync), Ok(()));
    }

    #[test]
    fn begin_counts_once_even_when_injected_failure_is_returned() {
        let control = FakeControl::new(now());
        control
            .queue_failure(FakeOperation::BackupPut, ProviderError::Conflict)
            .expect("queue failure");

        assert_eq!(
            control.begin(FakeOperation::BackupPut),
            Err(ProviderError::Conflict)
        );
        assert_eq!(
            control
                .invocation_count(FakeOperation::BackupPut)
                .expect("count"),
            1
        );
        assert_eq!(control.begin(FakeOperation::BackupPut), Ok(()));
        assert_eq!(
            control
                .invocation_count(FakeOperation::BackupPut)
                .expect("count"),
            2
        );
    }

    #[test]
    fn partial_page_boundaries_are_non_negative_and_operation_scoped() {
        let control = FakeControl::new(now());

        control
            .set_partial_failure(FakeOperation::MailSync, 0)
            .expect("zero-success boundary");
        assert_eq!(
            control
                .partial_failure_after(FakeOperation::MailSync)
                .expect("boundary"),
            Some(0)
        );
        control
            .set_partial_failure(FakeOperation::MailSync, 2)
            .expect("set boundary");
        assert_eq!(
            control
                .partial_failure_after(FakeOperation::MailSync)
                .expect("boundary"),
            Some(2)
        );
        assert_eq!(
            control
                .partial_failure_after(FakeOperation::MailSend)
                .expect("boundary"),
            None
        );

        control
            .clear_partial_failure(FakeOperation::MailSync)
            .expect("clear boundary");
        assert_eq!(
            control
                .partial_failure_after(FakeOperation::MailSync)
                .expect("boundary"),
            None
        );
    }

    #[test]
    fn cloned_controls_update_counts_without_lost_updates() {
        let control = FakeControl::new(now());
        let workers = 8;
        let per_worker = 125;
        let mut joins = Vec::new();

        for _ in 0..workers {
            let worker = control.clone();
            joins.push(thread::spawn(move || {
                for _ in 0..per_worker {
                    worker.begin(FakeOperation::CalendarSync).expect("begin");
                }
            }));
        }
        for join in joins {
            join.join().expect("worker joined");
        }

        assert_eq!(
            control
                .invocation_count(FakeOperation::CalendarSync)
                .expect("count"),
            workers * per_worker
        );
    }

    #[test]
    fn all_operations_are_closed_and_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FakeOperation>();
        assert_send_sync::<FakeControl>();

        assert_eq!(FakeOperation::ALL.len(), 13);
        assert!(FakeOperation::ALL.contains(&FakeOperation::MailSync));
        assert!(FakeOperation::ALL.contains(&FakeOperation::MailSend));
        assert!(FakeOperation::ALL.contains(&FakeOperation::MailLabels));
        assert!(FakeOperation::ALL.contains(&FakeOperation::CalendarBusy));
        assert!(FakeOperation::ALL.contains(&FakeOperation::CalendarSync));
        assert!(FakeOperation::ALL.contains(&FakeOperation::CalendarOwnerCreate));
        assert!(FakeOperation::ALL.contains(&FakeOperation::CalendarOwnerFind));
        assert!(FakeOperation::ALL.contains(&FakeOperation::CalendarProposalCreate));
        assert!(FakeOperation::ALL.contains(&FakeOperation::CalendarProposalFind));
        assert!(FakeOperation::ALL.contains(&FakeOperation::CalendarPromote));
        assert!(FakeOperation::ALL.contains(&FakeOperation::CalendarDelete));
        assert!(FakeOperation::ALL.contains(&FakeOperation::TriageClassify));
        assert!(FakeOperation::ALL.contains(&FakeOperation::BackupPut));
    }

    #[test]
    fn debug_is_metadata_only_and_redacts_failure_details() {
        let control = FakeControl::new(now());
        control
            .queue_failure(
                FakeOperation::MailSend,
                ProviderError::InvalidInput {
                    field: ProviderInputField::AccessToken,
                },
            )
            .expect("queue failure");
        control
            .set_failure(FakeOperation::MailSend, ProviderError::Unavailable)
            .expect("set persistent failure");
        control.begin(FakeOperation::MailSend).expect_err("failure");

        let debug = format!("{control:?}");
        assert!(debug.contains("MailSend"));
        assert!(debug.contains("count: 1"));
        assert!(debug.contains("persistent"));
        assert!(!debug.contains("sentinel-provider-token"));
        assert!(!debug.contains("AccessToken"));
        assert!(!debug.contains("provider"));
        assert!(!debug.contains("body"));
        assert!(!debug.contains("attendee"));
        assert!(!debug.contains("ciphertext"));
    }

    #[test]
    fn poisoned_state_fails_closed_with_fixed_error() {
        let control = FakeControl::new(now());
        let poison_target = control.clone();
        let join = thread::spawn(move || poison_target.poison_for_test());
        assert!(
            join.join().is_err(),
            "poison worker must panic while holding lock"
        );

        assert_eq!(
            control.begin(FakeOperation::MailSync),
            Err(ProviderError::Unavailable)
        );
        assert_eq!(
            control.queue_failure(FakeOperation::MailSync, ProviderError::Conflict),
            Err(ProviderError::Unavailable)
        );
        assert_eq!(
            control.set_partial_failure(FakeOperation::MailSync, 1),
            Err(ProviderError::Unavailable)
        );
        let debug = format!("{control:?}");
        assert_eq!(debug, "FakeControl { state: unavailable }");
        assert!(!debug.contains("poison"));
    }

    #[test]
    fn fixed_time_does_not_depend_on_wall_clock() {
        let control = FakeControl::new(now());
        let before = control.now();
        std::thread::sleep(
            Duration::milliseconds(1)
                .to_std()
                .expect("positive duration"),
        );
        assert_eq!(before, control.now());
    }

    #[test]
    fn arc_clone_is_supported_for_harness_setup() {
        let control = Arc::new(FakeControl::new(now()));
        let clone = Arc::clone(&control);
        clone.begin(FakeOperation::TriageClassify).expect("begin");
        assert_eq!(
            control
                .invocation_count(FakeOperation::TriageClassify)
                .expect("count"),
            1
        );
    }
}
