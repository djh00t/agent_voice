//! Deterministic structured-email triage provider fake.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::pa::providers::{
    ProviderError, ProviderFuture, ProviderResult, ProviderSession, StructuredTriageProvider,
    TriageDecision, TriageInput,
};

use super::control::{FakeControl, FakeOperation};

/// One exact transient-input and typed-decision fixture.
pub type TriageFixture = (TriageInput, TriageDecision);

/// Cloneable deterministic structured-triage fake.
///
/// Fixtures are indexed by source identity in an immutable shared map. Every
/// call is still counted and failure-injected through the shared control plane;
/// no fixture is consumed or changed by classification.
#[derive(Clone)]
pub struct FakeStructuredTriage {
    control: FakeControl,
    fixtures: Arc<BTreeMap<String, TriageFixture>>,
}

impl FakeStructuredTriage {
    /// Creates a fake from unique exact fixtures.
    ///
    /// Use [`Self::try_new`] when fixture validation errors must be handled by
    /// the caller. This constructor panics only when duplicate source
    /// identities are supplied.
    pub fn new<C, FI, F>(control: C, fixtures: FI) -> Self
    where
        C: Borrow<FakeControl>,
        FI: IntoIterator<Item = F>,
        F: Borrow<TriageFixture>,
    {
        Self::try_new(control, fixtures).expect("triage fixtures must have unique source IDs")
    }

    /// Validates and creates a fake from unique exact fixtures.
    pub fn try_new<C, FI, F>(control: C, fixtures: FI) -> ProviderResult<Self>
    where
        C: Borrow<FakeControl>,
        FI: IntoIterator<Item = F>,
        F: Borrow<TriageFixture>,
    {
        let mut indexed = BTreeMap::new();
        for fixture in fixtures {
            let (input, decision) = fixture.borrow();
            let source_id = input.source_id().as_str().to_owned();
            if indexed
                .insert(source_id, (input.clone(), decision.clone()))
                .is_some()
            {
                return Err(ProviderError::Conflict);
            }
        }
        Ok(Self {
            control: control.borrow().clone(),
            fixtures: Arc::new(indexed),
        })
    }

    /// Seed-constructor alias for [`Self::new`].
    pub fn from_seed<C, FI, F>(control: C, fixtures: FI) -> Self
    where
        C: Borrow<FakeControl>,
        FI: IntoIterator<Item = F>,
        F: Borrow<TriageFixture>,
    {
        Self::new(control, fixtures)
    }

    /// Returns the shared fake control plane.
    pub fn control(&self) -> &FakeControl {
        &self.control
    }
}

impl fmt::Debug for FakeStructuredTriage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("FakeStructuredTriage");
        debug.field("fixture_count", &self.fixtures.len());
        match self.control.invocation_count(FakeOperation::TriageClassify) {
            Ok(count) => debug.field("classify_call_count", &count).finish(),
            Err(_) => debug
                .field("classify_call_count", &"<unavailable>")
                .finish(),
        }
    }
}

impl StructuredTriageProvider for FakeStructuredTriage {
    fn classify<'a>(
        &'a self,
        _session: &'a ProviderSession,
        input: &'a TriageInput,
    ) -> ProviderFuture<'a, TriageDecision> {
        let control = self.control.clone();
        let fixtures = Arc::clone(&self.fixtures);
        let input = input.clone();
        Box::pin(async move {
            control.begin(FakeOperation::TriageClassify)?;
            let fixture = fixtures
                .get(input.source_id().as_str())
                .ok_or(ProviderError::NotFound)?;
            if fixture.0 == input {
                Ok(fixture.1.clone())
            } else {
                Err(ProviderError::Conflict)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::FakeStructuredTriage;
    use crate::pa::domain::TaskKind;
    use crate::pa::fakes::{FakeControl, FakeOperation};
    use crate::pa::providers::{
        ActionableTaskExtraction, AmbiguousReason, MailAddress, MailMessageId, ProviderError,
        ProviderFuture, ProviderSession, RetryAfter, StructuredTriageProvider, TriageDecision,
        TriageInput,
    };
    use chrono::{DateTime, Duration, Utc};

    fn now() -> DateTime<Utc> {
        "2026-08-29T12:34:56Z".parse().expect("fixed UTC instant")
    }

    fn input(source: &str, sender: &str, subject: &str, body: &str) -> TriageInput {
        TriageInput::new(
            MailMessageId::new(source).expect("source"),
            MailAddress::new(sender).expect("sender"),
            subject,
            body,
        )
        .expect("input")
    }

    fn actionable(kind: TaskKind, title: &str, due_at: DateTime<Utc>) -> TriageDecision {
        TriageDecision::Actionable(
            ActionableTaskExtraction::new(kind, title, kind.duration_minutes(), Some(due_at))
                .expect("extraction"),
        )
    }

    fn fixture(source: &str, body: &str) -> (TriageInput, TriageDecision) {
        (
            input(source, "sender@example.test", "subject", body),
            actionable(TaskKind::Callback, "title", now()),
        )
    }

    fn session() -> ProviderSession {
        ProviderSession::new("account", "session-token", None).expect("session")
    }

    #[tokio::test]
    async fn exact_input_returns_seeded_decision_and_repeats_across_clones() {
        let control = FakeControl::new(now());
        let (input, decision) = fixture("source", "body");
        let fake = FakeStructuredTriage::new(control.clone(), [(input.clone(), decision.clone())]);

        assert_eq!(
            fake.classify(&session(), &input).await,
            Ok(decision.clone())
        );
        assert_eq!(
            fake.clone().classify(&session(), &input).await,
            Ok(decision)
        );
        assert_eq!(
            control.invocation_count(FakeOperation::TriageClassify),
            Ok(2)
        );
    }

    #[tokio::test]
    async fn all_closed_decisions_are_returned_exactly_with_non_empty_due_instants() {
        let due_at = now() + Duration::minutes(45);
        let fixtures = [
            (
                input("bill", "bill@example.test", "Pay", "Invoice"),
                actionable(TaskKind::Bill, "Pay invoice", due_at),
            ),
            (
                input("callback", "callback@example.test", "Call", "Please call"),
                actionable(TaskKind::Callback, "Call Alex", due_at),
            ),
            (
                input("reading", "reading@example.test", "Read", "Document"),
                actionable(TaskKind::Reading, "Read document", due_at),
            ),
            (
                input("reply", "reply@example.test", "Reply", "Question"),
                actionable(TaskKind::EmailReply, "Reply to Alex", due_at),
            ),
            (
                input("prepare", "prepare@example.test", "Prepare", "Agenda"),
                actionable(TaskKind::Preparation, "Prepare agenda", due_at),
            ),
            (
                input("unclear-action", "one@example.test", "One", "One"),
                TriageDecision::Ambiguous(AmbiguousReason::UnclearAction),
            ),
            (
                input("unclear-timing", "two@example.test", "Two", "Two"),
                TriageDecision::Ambiguous(AmbiguousReason::UnclearTiming),
            ),
            (
                input("unclear-duration", "three@example.test", "Three", "Three"),
                TriageDecision::Ambiguous(AmbiguousReason::UnclearDuration),
            ),
            (
                input("unsafe", "four@example.test", "Four", "Four"),
                TriageDecision::Ambiguous(AmbiguousReason::UnsafeInstruction),
            ),
            (
                input("ignore", "ignore@example.test", "Ignore", "Ignore"),
                TriageDecision::Ignore,
            ),
        ];
        let control = FakeControl::new(now());
        let fake = FakeStructuredTriage::new(control.clone(), fixtures.clone());

        for (input, decision) in fixtures {
            assert_eq!(fake.classify(&session(), &input).await, Ok(decision));
        }
        assert_eq!(
            control.invocation_count(FakeOperation::TriageClassify),
            Ok(10)
        );
    }

    #[tokio::test]
    async fn unknown_and_each_changed_source_field_have_closed_errors_and_exact_counts() {
        let control = FakeControl::new(now());
        let (seeded_input, decision) = fixture("source", "body");
        let fake = FakeStructuredTriage::new(control.clone(), [(seeded_input.clone(), decision)]);
        let (unknown, _) = fixture("unknown", "body");
        assert_eq!(
            fake.classify(&session(), &unknown).await,
            Err(ProviderError::NotFound)
        );
        for changed in [
            input("source", "changed@example.test", "subject", "body"),
            input("source", "sender@example.test", "changed subject", "body"),
            input("source", "sender@example.test", "subject", "changed body"),
        ] {
            assert_eq!(
                fake.classify(&session(), &changed).await,
                Err(ProviderError::Conflict)
            );
        }
        assert_eq!(
            control.invocation_count(FakeOperation::TriageClassify),
            Ok(4)
        );
    }

    #[test]
    fn exact_and_conflicting_duplicate_sources_are_rejected() {
        let first = fixture("duplicate", "one");
        let exact_duplicate = first.clone();
        let conflicting_duplicate = fixture("duplicate", "two");
        assert!(matches!(
            FakeStructuredTriage::try_new(
                FakeControl::new(now()),
                [first.clone(), exact_duplicate]
            ),
            Err(ProviderError::Conflict)
        ));
        assert!(matches!(
            FakeStructuredTriage::try_new(FakeControl::new(now()), [first, conflicting_duplicate]),
            Err(ProviderError::Conflict)
        ));
    }

    #[tokio::test]
    async fn repeated_cloned_and_concurrent_exact_calls_are_stable_with_exact_counts() {
        let control = FakeControl::new(now());
        let (input, decision) = fixture("concurrent", "body");
        let fake = FakeStructuredTriage::new(control.clone(), [(input.clone(), decision.clone())]);

        assert_eq!(
            fake.classify(&session(), &input).await,
            Ok(decision.clone())
        );
        assert_eq!(
            fake.clone().classify(&session(), &input).await,
            Ok(decision.clone())
        );

        let mut calls = Vec::new();
        for _ in 0..12 {
            let fake = fake.clone();
            let input = input.clone();
            let decision = decision.clone();
            calls.push(tokio::spawn(async move {
                assert_eq!(fake.classify(&session(), &input).await, Ok(decision));
            }));
        }
        for call in calls {
            call.await.expect("concurrent call completes");
        }
        assert_eq!(
            control.invocation_count(FakeOperation::TriageClassify),
            Ok(14)
        );
    }

    #[tokio::test]
    async fn queued_and_persistent_failures_are_exact_non_mutating_and_recover() {
        let failures = [
            ProviderError::TokenExpired,
            ProviderError::throttled(RetryAfter::new(Duration::seconds(30)).expect("retry")),
            ProviderError::Unavailable,
        ];
        for failure in failures {
            let control = FakeControl::new(now());
            let (input, decision) = fixture("failure", "body");
            let fake =
                FakeStructuredTriage::new(control.clone(), [(input.clone(), decision.clone())]);

            control
                .queue_failure(FakeOperation::TriageClassify, failure)
                .expect("queue");
            assert_eq!(fake.classify(&session(), &input).await, Err(failure));
            assert_eq!(
                fake.classify(&session(), &input).await,
                Ok(decision.clone())
            );

            control
                .set_failure(FakeOperation::TriageClassify, failure)
                .expect("persist");
            assert_eq!(fake.classify(&session(), &input).await, Err(failure));
            control
                .clear_persistent_failure(FakeOperation::TriageClassify)
                .expect("clear");
            assert_eq!(fake.classify(&session(), &input).await, Ok(decision));
            assert_eq!(
                control.invocation_count(FakeOperation::TriageClassify),
                Ok(4)
            );
        }
    }

    #[tokio::test]
    async fn poisoned_control_does_not_expose_fixtures_and_fresh_replacement_recovers() {
        struct PanicWriter;

        impl std::fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> std::fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let (input, decision) = fixture("poisoned", "body");
        let fake = FakeStructuredTriage::new(control.clone(), [(input.clone(), decision.clone())]);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = std::fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());
        assert_eq!(
            fake.classify(&session(), &input).await,
            Err(ProviderError::Unavailable)
        );
        assert_eq!(
            format!("{fake:?}"),
            "FakeStructuredTriage { fixture_count: 1, classify_call_count: \"<unavailable>\" }"
        );

        let fresh =
            FakeStructuredTriage::new(FakeControl::new(now()), [(input.clone(), decision.clone())]);
        assert_eq!(fresh.classify(&session(), &input).await, Ok(decision));
    }

    #[tokio::test]
    async fn debug_trait_object_and_future_are_safe_and_redacted() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        fn assert_send<T: Send>(_: &T) {}

        let input = input(
            "sentinel-source-id",
            "sentinel.sender@example.test",
            "sentinel subject",
            "sentinel body",
        );
        let decision = actionable(TaskKind::Preparation, "sentinel actionable title", now());
        let fake =
            FakeStructuredTriage::new(FakeControl::new(now()), [(input.clone(), decision.clone())]);
        let current_session =
            ProviderSession::new("account", "sentinel-session-token", None).expect("session");
        let triage: &dyn StructuredTriageProvider = &fake;
        let future: ProviderFuture<'_, TriageDecision> = triage.classify(&current_session, &input);

        assert_send_sync::<FakeStructuredTriage>();
        assert_send_sync::<dyn StructuredTriageProvider>();
        assert_send(&future);
        assert_eq!(future.await, Ok(decision));

        let debug = format!("{fake:?}");
        assert!(debug.contains("fixture_count: 1"));
        assert!(debug.contains("classify_call_count: 1"));
        for sentinel in [
            "sentinel-source-id",
            "sentinel.sender@example.test",
            "sentinel subject",
            "sentinel body",
            "sentinel actionable title",
            "sentinel-session-token",
            "2026-08-29T12:34:56",
        ] {
            assert!(!debug.contains(sentinel), "debug leaked {sentinel}");
        }
    }
}
