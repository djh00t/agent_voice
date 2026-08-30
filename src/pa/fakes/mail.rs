//! Deterministic Outlook and Gmail mail provider fakes.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::pa::providers::{
    GmailProvider, IncomingMailProvider, LabelChanges, MailMessage, MailMessageId, MailSyncRequest,
    OutboundMail, OutlookMailProvider, ProviderError, ProviderFuture, ProviderItemFailure,
    ProviderResult, ProviderSession, SentMail, SyncPage,
};

use super::control::{FakeControl, FakeOperation};

const CURSOR_PREFIX: &str = "fake-outlook-mail:";
const FAILURE_SOURCE_ID: &str = "fake-outlook-mail";
const GMAIL_CURSOR_PREFIX: &str = "fake-gmail:";
const GMAIL_FAILURE_SOURCE_ID: &str = "fake-gmail";
const GMAIL_SENT_MESSAGE_PREFIX: &str = "fake-gmail-sent-";

struct MailReadState {
    messages: Vec<MailMessage>,
    emitted_cursors: BTreeSet<usize>,
}

struct GmailSentMail {
    mail: OutboundMail,
    sent: SentMail,
}

struct GmailSendState {
    sent_by_operation_key: BTreeMap<String, GmailSentMail>,
    next_sent_message_sequence: u64,
}

/// Cloneable deterministic implementation of the read-only Outlook contract.
///
/// Seed order is retained as the stable incremental synchronization order.
/// Clones share both the message seed and emitted cursor metadata.
#[derive(Clone)]
pub struct FakeOutlookMail {
    control: FakeControl,
    state: Arc<Mutex<MailReadState>>,
}

impl FakeOutlookMail {
    /// Creates an Outlook fake from validated messages and shared fake control.
    pub fn new<C, MI>(control: C, messages: MI) -> Self
    where
        C: Borrow<FakeControl>,
        MI: IntoIterator,
        MI::Item: Borrow<MailMessage>,
    {
        Self {
            control: control.borrow().clone(),
            state: Arc::new(Mutex::new(MailReadState {
                messages: messages
                    .into_iter()
                    .map(|message| message.borrow().clone())
                    .collect(),
                emitted_cursors: BTreeSet::new(),
            })),
        }
    }

    /// Validating-constructor alias for [`Self::new`].
    pub fn try_new<C, MI>(control: C, messages: MI) -> ProviderResult<Self>
    where
        C: Borrow<FakeControl>,
        MI: IntoIterator,
        MI::Item: Borrow<MailMessage>,
    {
        Ok(Self::new(control, messages))
    }

    /// Seed-constructor alias for [`Self::new`].
    pub fn from_seed<C, MI>(control: C, messages: MI) -> Self
    where
        C: Borrow<FakeControl>,
        MI: IntoIterator,
        MI::Item: Borrow<MailMessage>,
    {
        Self::new(control, messages)
    }

    /// Returns the shared control plane used by this fake.
    pub fn control(&self) -> &FakeControl {
        &self.control
    }
}

impl fmt::Debug for FakeOutlookMail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return formatter.write_str("FakeOutlookMail { state: unavailable }"),
        };
        formatter
            .debug_struct("FakeOutlookMail")
            .field("message_count", &state.messages.len())
            .field("emitted_cursor_count", &state.emitted_cursors.len())
            .finish()
    }
}

impl IncomingMailProvider for FakeOutlookMail {
    fn sync_mail<'a>(
        &'a self,
        _session: &'a ProviderSession,
        request: &'a MailSyncRequest,
    ) -> ProviderFuture<'a, SyncPage<MailMessage>> {
        let control = self.control.clone();
        let state = Arc::clone(&self.state);
        let cursor = request.cursor().map(str::to_owned);
        let limit = request.limit();
        Box::pin(async move {
            sync_mail_page(
                &control,
                &state,
                cursor.as_deref(),
                limit,
                CURSOR_PREFIX,
                FAILURE_SOURCE_ID,
            )
        })
    }
}

impl OutlookMailProvider for FakeOutlookMail {}

fn sync_mail_page(
    control: &FakeControl,
    state: &Arc<Mutex<MailReadState>>,
    cursor: Option<&str>,
    limit: usize,
    cursor_prefix: &str,
    failure_source_id: &str,
) -> ProviderResult<SyncPage<MailMessage>> {
    control.begin(FakeOperation::MailSync)?;
    let partial_after = control.partial_failure_after(FakeOperation::MailSync)?;
    let mut state = state.lock().map_err(|_| ProviderError::Unavailable)?;
    let start = match cursor {
        None => 0,
        Some(cursor) => parse_cursor(cursor, &state, cursor_prefix)?,
    };
    let page_count = state.messages.len().saturating_sub(start).min(limit);
    let failure_after = partial_after.filter(|after| *after < page_count);
    let successful_count = failure_after.unwrap_or(page_count);
    let items = state.messages[start..start + successful_count].to_vec();
    let item_failures = match failure_after {
        Some(_) => {
            let failed = &state.messages[start + successful_count];
            vec![ProviderItemFailure::new(
                failure_source_id,
                failed.source_id().as_str(),
                ProviderError::Unavailable,
            )?]
        }
        None => Vec::new(),
    };
    let next_position = start + successful_count;
    let next_cursor =
        (next_position < state.messages.len()).then(|| cursor_for(cursor_prefix, next_position));
    let page = SyncPage::new(items, next_cursor, item_failures)?;
    if next_position < state.messages.len() {
        state.emitted_cursors.insert(next_position);
    }
    Ok(page)
}

fn cursor_for(prefix: &str, position: usize) -> String {
    format!("{prefix}{position}")
}

fn parse_cursor(cursor: &str, state: &MailReadState, prefix: &str) -> ProviderResult<usize> {
    let suffix = cursor
        .strip_prefix(prefix)
        .ok_or(ProviderError::CursorExpired)?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProviderError::CursorExpired);
    }
    let position = suffix
        .parse::<usize>()
        .map_err(|_| ProviderError::CursorExpired)?;
    if position >= state.messages.len()
        || cursor_for(prefix, position) != cursor
        || !state.emitted_cursors.contains(&position)
    {
        return Err(ProviderError::CursorExpired);
    }
    Ok(position)
}

/// Cloneable deterministic Gmail read fake with internal label mutation state.
#[derive(Clone)]
pub struct FakeGmail {
    control: FakeControl,
    state: Arc<Mutex<MailReadState>>,
    send_state: Arc<Mutex<GmailSendState>>,
}

impl FakeGmail {
    /// Creates a Gmail fake from validated messages and shared fake control.
    pub fn new<C, MI>(control: C, messages: MI) -> Self
    where
        C: Borrow<FakeControl>,
        MI: IntoIterator,
        MI::Item: Borrow<MailMessage>,
    {
        Self {
            control: control.borrow().clone(),
            state: Arc::new(Mutex::new(MailReadState {
                messages: messages
                    .into_iter()
                    .map(|message| message.borrow().clone())
                    .collect(),
                emitted_cursors: BTreeSet::new(),
            })),
            send_state: Arc::new(Mutex::new(GmailSendState {
                sent_by_operation_key: BTreeMap::new(),
                next_sent_message_sequence: 1,
            })),
        }
    }

    /// Returns the shared control plane used by this fake.
    pub fn control(&self) -> &FakeControl {
        &self.control
    }

    /// Returns cloned sent receipts in deterministic operation-key order.
    ///
    /// This read-only fake inspection helper exposes no outbound message
    /// payload or operation key and does not invoke or count a provider
    /// operation.
    pub fn sent_mail_receipts(&self) -> ProviderResult<Vec<SentMail>> {
        let state = self
            .send_state
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        Ok(state
            .sent_by_operation_key
            .values()
            .map(|record| record.sent.clone())
            .collect())
    }

    /// Applies validated Gmail label changes without exposing the provider trait.
    pub(crate) fn modify_labels<'a>(
        &'a self,
        source_id: &'a MailMessageId,
        changes: &'a LabelChanges,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            self.control.begin(FakeOperation::MailLabels)?;
            let mut state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
            let current = state
                .messages
                .iter()
                .rev()
                .find(|message| message.source_id() == source_id)
                .cloned()
                .ok_or(ProviderError::NotFound)?;
            let labels = current
                .labels()
                .iter()
                .filter(|label| !changes.remove_labels().contains(*label))
                .chain(changes.add_labels().iter())
                .cloned()
                .collect::<BTreeSet<_>>();
            let current_labels = current.labels().iter().collect::<BTreeSet<_>>();
            if labels.iter().eq(current_labels.iter().copied()) {
                return Ok(());
            }
            state.messages.push(MailMessage::new(
                current.source_id().as_str(),
                current.sender().clone(),
                current.subject(),
                current.body(),
                current.received_at(),
                labels,
            )?);
            Ok(())
        })
    }

    /// Sends one validated Gmail message with deterministic idempotency.
    pub(crate) fn send_mail<'a>(&'a self, mail: &'a OutboundMail) -> ProviderFuture<'a, SentMail> {
        Box::pin(async move {
            let seeded_message_ids = {
                let read_state = self
                    .state
                    .lock()
                    .map_err(|_| ProviderError::Unavailable)?;
                read_state
                    .messages
                    .iter()
                    .map(|message| message.source_id().as_str().to_owned())
                    .collect::<BTreeSet<_>>()
            };
            let mut state = self
                .send_state
                .lock()
                .map_err(|_| ProviderError::Unavailable)?;
            if let Some(existing) = state.sent_by_operation_key.get(mail.operation_key()) {
                return if existing.mail == *mail {
                    Ok(existing.sent.clone())
                } else {
                    Err(ProviderError::Conflict)
                };
            }

            self.control.begin(FakeOperation::MailSend)?;
            let mut sequence = state.next_sent_message_sequence;
            let provider_message_id = loop {
                let candidate = format!("{GMAIL_SENT_MESSAGE_PREFIX}{sequence}");
                if !seeded_message_ids.contains(&candidate) {
                    break candidate;
                }
                sequence = sequence.checked_add(1).ok_or(ProviderError::Unavailable)?;
            };
            let next_sequence = sequence.checked_add(1).ok_or(ProviderError::Unavailable)?;
            let sent = SentMail::new(
                provider_message_id,
                self.control.now(),
            )?;
            state.sent_by_operation_key.insert(
                mail.operation_key().to_owned(),
                GmailSentMail {
                    mail: mail.clone(),
                    sent: sent.clone(),
                },
            );
            state.next_sent_message_sequence = next_sequence;
            Ok(sent)
        })
    }
}

impl fmt::Debug for FakeGmail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return formatter.write_str("FakeGmail { state: unavailable }"),
        };
        let send_state = match self.send_state.lock() {
            Ok(state) => state,
            Err(_) => return formatter.write_str("FakeGmail { state: unavailable }"),
        };
        formatter
            .debug_struct("FakeGmail")
            .field("message_count", &state.messages.len())
            .field("emitted_cursor_count", &state.emitted_cursors.len())
            .field("sent_mail_count", &send_state.sent_by_operation_key.len())
            .finish()
    }
}

impl IncomingMailProvider for FakeGmail {
    fn sync_mail<'a>(
        &'a self,
        _session: &'a ProviderSession,
        request: &'a MailSyncRequest,
    ) -> ProviderFuture<'a, SyncPage<MailMessage>> {
        let control = self.control.clone();
        let state = Arc::clone(&self.state);
        let cursor = request.cursor().map(str::to_owned);
        let limit = request.limit();
        Box::pin(async move {
            sync_mail_page(
                &control,
                &state,
                cursor.as_deref(),
                limit,
                GMAIL_CURSOR_PREFIX,
                GMAIL_FAILURE_SOURCE_ID,
            )
        })
    }
}

impl GmailProvider for FakeGmail {
    fn modify_labels<'a>(
        &'a self,
        _session: &'a ProviderSession,
        source_id: &'a MailMessageId,
        changes: &'a LabelChanges,
    ) -> ProviderFuture<'a, ()> {
        self.modify_labels(source_id, changes)
    }

    fn send_mail<'a>(
        &'a self,
        _session: &'a ProviderSession,
        mail: &'a OutboundMail,
    ) -> ProviderFuture<'a, SentMail> {
        self.send_mail(mail)
    }
}

#[cfg(test)]
mod tests {
    use super::{FakeGmail, FakeOutlookMail};
    use crate::pa::fakes::{FakeControl, FakeOperation};
    use crate::pa::providers::{
        GmailProvider, IncomingMailProvider, LabelChanges, MailAddress, MailMessage, MailMessageId,
        MailSyncRequest, OutboundMail, ProviderError, ProviderSession, RetryAfter, SyncPage,
    };
    use chrono::{DateTime, Duration, Utc};

    fn assert_send_sync<T: Send + Sync>() {}

    fn now() -> DateTime<Utc> {
        "2026-08-29T12:34:56Z".parse().expect("fixed timestamp")
    }

    fn session() -> ProviderSession {
        ProviderSession::new("account", "token", None).expect("session")
    }

    fn message(id: &str) -> MailMessage {
        MailMessage::new(
            id,
            MailAddress::new("sender@example.com").expect("sender"),
            "subject",
            "body",
            now(),
            ["inbox"],
        )
        .expect("message")
    }

    fn request(cursor: Option<String>, limit: usize) -> MailSyncRequest {
        MailSyncRequest::new(cursor, limit).expect("request")
    }

    fn outbound(key: &str, recipient: &str, subject: &str, body: &str) -> OutboundMail {
        OutboundMail::new(
            key,
            MailAddress::new(recipient).expect("recipient"),
            subject,
            body,
        )
        .expect("outbound mail")
    }

    #[tokio::test]
    async fn gmail_send_is_idempotent_and_rejects_every_changed_payload_field() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("seed")]);
        let first = outbound("mail-op", "recipient@example.test", "Subject", "Body");
        let sent = fake.send_mail(&first).await.expect("first send");
        assert_eq!(sent.provider_message_id(), "fake-gmail-sent-1");
        assert_eq!(sent.sent_at(), now());
        assert_eq!(fake.sent_mail_receipts(), Ok(vec![sent.clone()]));
        assert_eq!(fake.send_mail(&first).await, Ok(sent.clone()));
        assert_eq!(fake.sent_mail_receipts(), Ok(vec![sent.clone()]));

        for changed in [
            outbound("mail-op", "other@example.test", "Subject", "Body"),
            outbound("mail-op", "recipient@example.test", "Other subject", "Body"),
            outbound("mail-op", "recipient@example.test", "Subject", "Other body"),
        ] {
            assert_eq!(fake.send_mail(&changed).await, Err(ProviderError::Conflict));
        }
        assert_eq!(fake.sent_mail_receipts(), Ok(vec![sent]));
        assert_eq!(control.invocation_count(FakeOperation::MailSend), Ok(1));
    }

    #[tokio::test]
    async fn gmail_send_skips_seeded_sent_message_ids() {
        let fake = FakeGmail::new(
            FakeControl::new(now()),
            [message("fake-gmail-sent-1"), message("fake-gmail-sent-2")],
        );
        let sent = fake
            .send_mail(&outbound(
                "seeded-message-id-op",
                "recipient@example.test",
                "Subject",
                "Body",
            ))
            .await
            .expect("send");

        assert_eq!(sent.provider_message_id(), "fake-gmail-sent-3");
    }

    #[tokio::test]
    async fn gmail_sent_mail_receipts_are_empty_before_any_send() {
        let fake = FakeGmail::new(FakeControl::new(now()), [message("seed")]);

        assert_eq!(fake.sent_mail_receipts(), Ok(Vec::new()));
    }

    #[tokio::test]
    async fn gmail_sent_mail_receipts_are_unchanged_after_failed_send_and_reads_do_not_count() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("seed")]);
        let mail = outbound(
            "failed-receipt-op",
            "recipient@example.test",
            "Subject",
            "Body",
        );
        let before = fake.sent_mail_receipts().expect("empty receipts");
        let count_before = control
            .invocation_count(FakeOperation::MailSend)
            .expect("send count");

        control
            .queue_failure(FakeOperation::MailSend, ProviderError::Unavailable)
            .expect("queue failure");
        assert_eq!(fake.send_mail(&mail).await, Err(ProviderError::Unavailable));
        assert_eq!(fake.sent_mail_receipts(), Ok(before.clone()));
        let count_after_failure = control
            .invocation_count(FakeOperation::MailSend)
            .expect("send count");
        assert_eq!(count_after_failure, count_before + 1);

        assert_eq!(fake.sent_mail_receipts(), Ok(before));
        assert_eq!(
            control
                .invocation_count(FakeOperation::MailSend)
                .expect("send count"),
            count_after_failure
        );
    }

    #[tokio::test]
    async fn gmail_sent_mail_receipts_are_shared_and_snapshotted_across_clones() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control, [message("seed")]);
        let first = fake
            .send_mail(&outbound(
                "first-receipt-op",
                "recipient@example.test",
                "Subject",
                "Body",
            ))
            .await
            .expect("first send");
        let clone = fake.clone();
        let snapshot = fake.sent_mail_receipts().expect("first receipt");

        assert_eq!(snapshot, vec![first.clone()]);
        assert_eq!(clone.sent_mail_receipts(), Ok(snapshot.clone()));

        clone
            .send_mail(&outbound(
                "second-receipt-op",
                "recipient@example.test",
                "Subject",
                "Body",
            ))
            .await
            .expect("second send");
        assert_eq!(snapshot, vec![first]);
        assert_eq!(
            fake.sent_mail_receipts().expect("current receipts").len(),
            2
        );
    }

    #[tokio::test]
    async fn gmail_sent_mail_receipts_are_ordered_by_operation_key() {
        let fake = FakeGmail::new(FakeControl::new(now()), [message("seed")]);
        for key in ["zulu-receipt-op", "alpha-receipt-op", "middle-receipt-op"] {
            fake.send_mail(&outbound(key, "recipient@example.test", "Subject", "Body"))
                .await
                .expect("send");
        }

        let receipts = fake.sent_mail_receipts().expect("receipts");
        assert_eq!(
            receipts
                .iter()
                .map(|receipt| receipt.provider_message_id())
                .collect::<Vec<_>>(),
            [
                "fake-gmail-sent-2",
                "fake-gmail-sent-3",
                "fake-gmail-sent-1"
            ]
        );
    }

    #[tokio::test]
    async fn gmail_sent_mail_receipts_fail_closed_when_send_state_is_poisoned() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("seed")]);
        let count_before = control
            .invocation_count(FakeOperation::MailSend)
            .expect("send count");
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = fake
                .send_state
                .lock()
                .expect("send state is not already poisoned");
            panic!("intentional send-state mutex poisoning");
        }));
        assert!(poisoned.is_err());

        assert_eq!(fake.sent_mail_receipts(), Err(ProviderError::Unavailable));
        assert_eq!(
            control
                .invocation_count(FakeOperation::MailSend)
                .expect("send count"),
            count_before
        );
    }

    #[tokio::test]
    async fn gmail_send_failures_do_not_consume_ids_and_recover_deterministically() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("seed")]);
        let mail = outbound("failure-op", "recipient@example.test", "Subject", "Body");

        control
            .queue_failure(FakeOperation::MailSend, ProviderError::TokenExpired)
            .expect("queued expiration");
        assert_eq!(
            fake.send_mail(&mail).await,
            Err(ProviderError::TokenExpired)
        );
        control
            .set_failure(
                FakeOperation::MailSend,
                ProviderError::throttled(RetryAfter::new(Duration::seconds(1)).expect("retry")),
            )
            .expect("persistent throttle");
        assert!(matches!(
            fake.send_mail(&mail).await,
            Err(ProviderError::Throttled { .. })
        ));
        control
            .set_failure(FakeOperation::MailSend, ProviderError::Unavailable)
            .expect("persistent unavailable");
        assert_eq!(fake.send_mail(&mail).await, Err(ProviderError::Unavailable));
        control
            .clear_failure(FakeOperation::MailSend)
            .expect("clear failure");

        let sent = fake.send_mail(&mail).await.expect("recovered send");
        assert_eq!(sent.provider_message_id(), "fake-gmail-sent-1");
        assert_eq!(control.invocation_count(FakeOperation::MailSend), Ok(4));
    }

    #[tokio::test]
    async fn gmail_send_is_shared_and_idempotent_across_concurrent_clones() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("seed")]);
        let mail = outbound("concurrent-op", "recipient@example.test", "Subject", "Body");
        let cloned = fake.clone();

        let (first, second) = tokio::join!(fake.send_mail(&mail), cloned.send_mail(&mail));
        let first = first.expect("first send");
        assert_eq!(second, Ok(first.clone()));
        assert_eq!(first.provider_message_id(), "fake-gmail-sent-1");
        assert_eq!(control.invocation_count(FakeOperation::MailSend), Ok(1));
    }

    #[tokio::test]
    async fn gmail_provider_trait_object_runs_full_lifecycle_once_per_operation() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("one")]);
        let gmail: &dyn GmailProvider = &fake;
        let current_session = session();
        let source_id = MailMessageId::new("one").expect("source ID");
        let changes = LabelChanges::new(["starred"], Vec::<String>::new()).expect("changes");
        let mail = outbound("trait-op", "recipient@example.test", "Subject", "Body");

        assert_eq!(
            gmail
                .sync_mail(&current_session, &request(None, 1))
                .await
                .expect("read")
                .items(),
            [message("one")]
        );
        gmail
            .modify_labels(&current_session, &source_id, &changes)
            .await
            .expect("labels");
        let sent = gmail
            .send_mail(&current_session, &mail)
            .await
            .expect("send");
        assert_eq!(sent.provider_message_id(), "fake-gmail-sent-1");
        assert_eq!(control.invocation_count(FakeOperation::MailSync), Ok(1));
        assert_eq!(control.invocation_count(FakeOperation::MailLabels), Ok(1));
        assert_eq!(control.invocation_count(FakeOperation::MailSend), Ok(1));
    }

    #[tokio::test]
    async fn gmail_send_fails_closed_when_control_is_poisoned_without_recording_mail() {
        struct PanicWriter;

        impl std::fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> std::fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("seed")]);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = std::fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());

        let mail = outbound("poisoned-op", "recipient@example.test", "Subject", "Body");
        assert_eq!(fake.send_mail(&mail).await, Err(ProviderError::Unavailable));
        assert_eq!(
            format!("{fake:?}"),
            "FakeGmail { message_count: 1, emitted_cursor_count: 0, sent_mail_count: 0 }"
        );

        let fresh = FakeGmail::new(FakeControl::new(now()), [message("seed")]);
        assert_eq!(
            fresh
                .send_mail(&mail)
                .await
                .expect("fresh fake sends")
                .provider_message_id(),
            "fake-gmail-sent-1"
        );
    }

    #[test]
    fn outlook_mail_fake_is_send_sync() {
        assert_send_sync::<FakeOutlookMail>();
    }

    #[test]
    fn gmail_mail_fake_is_send_sync() {
        assert_send_sync::<FakeGmail>();
    }

    #[tokio::test]
    async fn gmail_label_mutation_updates_one_message_and_is_visible_incrementally() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("one"), message("two")]);
        let changes = LabelChanges::new(["starred"], ["inbox"]).expect("label changes");

        fake.modify_labels(&MailMessageId::new("one").expect("message ID"), &changes)
            .await
            .expect("labels applied");

        let page = fake
            .sync_mail(&session(), &request(None, 3))
            .await
            .expect("incremental page");
        assert_eq!(page.items().len(), 3);
        assert_eq!(page.items()[0], message("one"));
        assert_eq!(page.items()[1], message("two"));
        assert_eq!(page.items()[2].source_id().as_str(), "one");
        assert_eq!(page.items()[2].labels(), ["starred"]);
        assert_eq!(page.items()[2].body(), "body");
        assert_eq!(control.invocation_count(FakeOperation::MailLabels), Ok(1));
    }

    async fn assert_shared_cursor_contract<P: IncomingMailProvider>(provider: &P, prefix: &str) {
        assert_eq!(
            provider
                .sync_mail(&session(), &request(Some(format!("{prefix}1")), 1))
                .await,
            Err(ProviderError::CursorExpired)
        );
        let first = provider
            .sync_mail(&session(), &request(None, 1))
            .await
            .expect("first page");
        assert_eq!(first.next_cursor(), Some(format!("{prefix}1").as_str()));
        assert_eq!(
            provider
                .sync_mail(&session(), &request(Some(format!("{prefix}0")), 1))
                .await,
            Err(ProviderError::CursorExpired)
        );
    }

    #[tokio::test]
    async fn outlook_and_gmail_share_emitted_cursor_validation() {
        let control = FakeControl::new(now());
        let outlook = FakeOutlookMail::new(control.clone(), [message("one"), message("two")]);
        let gmail = FakeGmail::new(control, [message("one"), message("two")]);

        assert_shared_cursor_contract(&outlook, "fake-outlook-mail:").await;
        assert_shared_cursor_contract(&gmail, "fake-gmail:").await;
    }

    #[tokio::test]
    async fn gmail_label_mutation_is_idempotent_shared_by_clones_and_keeps_seed_order() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("one"), message("two")]);
        let changes =
            LabelChanges::new(["starred", "inbox"], Vec::<String>::new()).expect("label changes");
        let source_id = MailMessageId::new("one").expect("message ID");
        let cloned = fake.clone();

        let (first, second) = tokio::join!(
            fake.modify_labels(&source_id, &changes),
            cloned.modify_labels(&source_id, &changes)
        );
        assert_eq!(first, Ok(()));
        assert_eq!(second, Ok(()));

        let page = fake
            .sync_mail(&session(), &request(None, 4))
            .await
            .expect("incremental page");
        assert_eq!(
            page.items()
                .iter()
                .map(|message| message.source_id().as_str())
                .collect::<Vec<_>>(),
            ["one", "two", "one"]
        );
        assert_eq!(page.items()[2].labels(), ["inbox", "starred"]);
        assert_eq!(control.invocation_count(FakeOperation::MailLabels), Ok(2));
    }

    #[tokio::test]
    async fn gmail_label_failures_and_missing_message_leave_incremental_history_unchanged() {
        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("one")]);
        let changes = LabelChanges::new(["starred"], Vec::<String>::new()).expect("changes");
        let source_id = MailMessageId::new("one").expect("message ID");

        control
            .queue_failure(FakeOperation::MailLabels, ProviderError::TokenExpired)
            .expect("queue failure");
        assert_eq!(
            fake.modify_labels(&source_id, &changes).await,
            Err(ProviderError::TokenExpired)
        );
        control
            .set_failure(
                FakeOperation::MailLabels,
                ProviderError::throttled(RetryAfter::new(Duration::seconds(1)).expect("retry")),
            )
            .expect("persistent failure");
        assert!(matches!(
            fake.modify_labels(&source_id, &changes).await,
            Err(ProviderError::Throttled { .. })
        ));
        control
            .clear_failure(FakeOperation::MailLabels)
            .expect("clear failure");
        control
            .set_failure(FakeOperation::MailLabels, ProviderError::Unavailable)
            .expect("persistent unavailable");
        assert_eq!(
            fake.modify_labels(&source_id, &changes).await,
            Err(ProviderError::Unavailable)
        );
        control
            .clear_failure(FakeOperation::MailLabels)
            .expect("clear unavailable");
        assert_eq!(
            fake.modify_labels(
                &MailMessageId::new("missing").expect("message ID"),
                &changes
            )
            .await,
            Err(ProviderError::NotFound)
        );
        let unchanged = fake
            .sync_mail(&session(), &request(None, 2))
            .await
            .expect("unchanged history");
        assert_eq!(unchanged.items(), [message("one")]);

        fake.modify_labels(&source_id, &changes)
            .await
            .expect("recovered mutation");
        let changed = fake
            .sync_mail(&session(), &request(None, 2))
            .await
            .expect("changed history");
        assert_eq!(changed.items().len(), 2);
        assert_eq!(changed.items()[1].labels(), ["inbox", "starred"]);
    }

    #[test]
    fn gmail_debug_exposes_only_counts() {
        let fake = FakeGmail::new(
            FakeControl::new(now()),
            [MailMessage::new(
                "private-message-id",
                MailAddress::new("private.sender@example.com").expect("sender"),
                "private subject",
                "private body",
                now(),
                ["private-label"],
            )
            .expect("message")],
        );
        let debug = format!("{fake:?}");
        assert!(debug.contains("message_count"));
        for secret in [
            "private-message-id",
            "private.sender@example.com",
            "private subject",
            "private body",
            "private-label",
            "fake-gmail:",
        ] {
            assert!(!debug.contains(secret), "debug output exposed a seeded value");
        }
    }

    #[tokio::test]
    async fn gmail_debug_redacts_seeded_and_sent_mail_values() {
        let fake = FakeGmail::new(
            FakeControl::new(now()),
            [MailMessage::new(
                "sentinel-seed-id",
                MailAddress::new("sentinel.seed@example.test").expect("sender"),
                "sentinel seed subject",
                "sentinel seed body",
                now(),
                ["sentinel-seed-label"],
            )
            .expect("message")],
        );
        fake.send_mail(&outbound(
            "sentinel-operation-key",
            "sentinel.recipient@example.test",
            "sentinel sent subject",
            "sentinel sent body",
        ))
        .await
        .expect("sent mail");

        let debug = format!("{fake:?}");
        assert_eq!(fake.sent_mail_receipts().expect("receipt").len(), 1);
        assert_eq!(format!("{fake:?}"), debug);
        assert!(debug.contains("message_count: 1"));
        assert!(debug.contains("sent_mail_count: 1"));
        for secret in [
            "sentinel-seed-id",
            "sentinel.seed@example.test",
            "sentinel seed subject",
            "sentinel seed body",
            "sentinel-seed-label",
            "sentinel-operation-key",
            "sentinel.recipient@example.test",
            "sentinel sent subject",
            "sentinel sent body",
            "fake-gmail-sent-1",
        ] {
            assert!(!debug.contains(secret), "debug output exposed a seeded value");
        }
    }

    #[tokio::test]
    async fn gmail_poisoned_control_fails_closed_without_mutation_and_fresh_fake_recovers() {
        struct PanicWriter;

        impl std::fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> std::fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let fake = FakeGmail::new(control.clone(), [message("one")]);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = std::fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());

        let changes = LabelChanges::new(["starred"], Vec::<String>::new()).expect("changes");
        assert_eq!(
            fake.modify_labels(&MailMessageId::new("one").expect("message ID"), &changes)
                .await,
            Err(ProviderError::Unavailable)
        );
        assert_eq!(
            fake.sync_mail(&session(), &request(None, 2)).await,
            Err(ProviderError::Unavailable)
        );

        let fresh = FakeGmail::new(FakeControl::new(now()), [message("one")]);
        fresh
            .modify_labels(&MailMessageId::new("one").expect("message ID"), &changes)
            .await
            .expect("fresh fake recovers");
        assert_eq!(
            fresh
                .sync_mail(&session(), &request(None, 2))
                .await
                .expect("fresh history")
                .items()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn sync_pages_preserve_seed_order_and_clone_cursor_metadata() {
        let control = FakeControl::new(now());
        let fake = FakeOutlookMail::new(
            control.clone(),
            [message("one"), message("two"), message("three")],
        );
        let first = fake
            .sync_mail(&session(), &request(None, 1))
            .await
            .expect("first page");
        assert_eq!(first.items()[0].source_id().as_str(), "one");
        let cursor = first.next_cursor().expect("cursor").to_owned();

        let second = fake
            .clone()
            .sync_mail(&session(), &request(Some(cursor.clone()), 2))
            .await
            .expect("second page");
        assert_eq!(
            second
                .items()
                .iter()
                .map(|message| message.source_id().as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
        assert_eq!(second.next_cursor(), None);
        assert_eq!(
            fake.sync_mail(&session(), &request(Some(cursor), 2)).await,
            Ok(second)
        );
        assert_eq!(control.invocation_count(FakeOperation::MailSync), Ok(3));
    }

    #[tokio::test]
    async fn invalid_or_unemitted_cursor_is_expired_without_cursor_mutation() {
        let fake = FakeOutlookMail::new(FakeControl::new(now()), [message("one"), message("two")]);
        for cursor in ["fake-outlook-mail:1", "foreign", "fake-outlook-mail:0"] {
            assert_eq!(
                fake.sync_mail(&session(), &request(Some(cursor.to_owned()), 1))
                    .await,
                Err(ProviderError::CursorExpired)
            );
        }
        let first = fake
            .sync_mail(&session(), &request(None, 1))
            .await
            .expect("first page");
        assert_eq!(first.next_cursor(), Some("fake-outlook-mail:1"));
    }

    #[tokio::test]
    async fn partial_failure_returns_success_prefix_and_retries_failed_message() {
        let control = FakeControl::new(now());
        control
            .set_partial_failure(FakeOperation::MailSync, 1)
            .expect("partial failure");
        let fake = FakeOutlookMail::new(control.clone(), [message("one"), message("two")]);
        let partial = fake
            .sync_mail(&session(), &request(None, 2))
            .await
            .expect("partial page");
        assert_eq!(partial.items()[0].source_id().as_str(), "one");
        assert_eq!(partial.item_failures().len(), 1);
        assert_eq!(partial.item_failures()[0].item_id(), "two");
        assert_eq!(
            partial.item_failures()[0].error(),
            ProviderError::Unavailable
        );

        control
            .clear_partial_failure(FakeOperation::MailSync)
            .expect("clear partial failure");
        let retry = fake
            .sync_mail(
                &session(),
                &request(Some(partial.next_cursor().expect("cursor").to_owned()), 1),
            )
            .await
            .expect("retry");
        assert_eq!(retry.items()[0].source_id().as_str(), "two");
    }

    #[tokio::test]
    async fn zero_success_partial_page_retries_first_message_from_position_zero() {
        let control = FakeControl::new(now());
        control
            .set_partial_failure(FakeOperation::MailSync, 0)
            .expect("zero-success partial failure");
        let fake = FakeOutlookMail::new(control.clone(), [message("one"), message("two")]);

        let partial = fake
            .sync_mail(&session(), &request(None, 2))
            .await
            .expect("zero-success partial page");
        assert!(partial.items().is_empty());
        assert_eq!(partial.next_cursor(), Some("fake-outlook-mail:0"));
        assert_eq!(partial.item_failures().len(), 1);
        assert_eq!(partial.item_failures()[0].source_id(), "fake-outlook-mail");
        assert_eq!(partial.item_failures()[0].item_id(), "one");
        assert_eq!(
            partial.item_failures()[0].error(),
            ProviderError::Unavailable
        );

        control
            .clear_partial_failure(FakeOperation::MailSync)
            .expect("clear partial failure");
        let retry = fake
            .sync_mail(
                &session(),
                &request(Some("fake-outlook-mail:0".to_owned()), 2),
            )
            .await
            .expect("exact retry from position zero");
        assert_eq!(
            retry,
            SyncPage::new(vec![message("one"), message("two")], None, Vec::new(),)
                .expect("successful retry page")
        );
    }

    #[tokio::test]
    async fn injected_failure_does_not_emit_a_cursor() {
        let control = FakeControl::new(now());
        control
            .queue_failure(FakeOperation::MailSync, ProviderError::TokenExpired)
            .expect("queue failure");
        let fake = FakeOutlookMail::new(control, [message("one"), message("two")]);
        assert_eq!(
            fake.sync_mail(&session(), &request(None, 1)).await,
            Err(ProviderError::TokenExpired)
        );
        assert_eq!(
            fake.sync_mail(
                &session(),
                &request(Some("fake-outlook-mail:1".to_owned()), 1),
            )
            .await,
            Err(ProviderError::CursorExpired)
        );
    }
}
