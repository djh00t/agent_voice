//! Cross-provider public-contract matrix for deterministic PA fakes.

use super::*;
use crate::pa::availability::BusyInterval;
use crate::pa::domain::TaskKind;
use crate::pa::providers::*;
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use tokio::sync::Barrier;

fn now() -> DateTime<Utc> {
    "2026-08-29T12:34:56Z".parse().expect("fixed instant")
}

fn session() -> ProviderSession {
    ProviderSession::new("contract-account", "contract-token", None).expect("session")
}

fn range() -> TimeRange {
    TimeRange::new(
        "2026-08-29T10:00:00Z".parse().expect("start"),
        "2026-08-29T11:00:00Z".parse().expect("end"),
    )
    .expect("range")
}

fn calendar_request(cursor: Option<String>, limit: usize) -> CalendarSyncRequest {
    CalendarSyncRequest::new(range(), cursor, limit).expect("calendar request")
}

fn message(id: &str) -> MailMessage {
    MailMessage::new(
        id,
        MailAddress::new("contract@example.test").expect("sender"),
        "contract title",
        "contract body",
        now(),
        ["inbox"],
    )
    .expect("message")
}

fn sentinel_message(id: &str) -> MailMessage {
    MailMessage::new(
        id,
        MailAddress::new("sentinel-sender@example.test").expect("sender"),
        "sentinel-title",
        "sentinel-body",
        now(),
        ["sentinel-label"],
    )
    .expect("sentinel message")
}

fn triage_input(id: &str) -> TriageInput {
    TriageInput::new(
        MailMessageId::new(id).expect("id"),
        MailAddress::new("contract@example.test").expect("sender"),
        "contract title",
        "contract body",
    )
    .expect("triage input")
}

fn snapshot() -> EncryptedSnapshot {
    EncryptedSnapshot::new(
        "backup-sentinel-object-key",
        b"backup-sentinel-ciphertext".to_vec(),
        "2e46e41589939dd28a664c8d40d852b3800c01cdb32e87f3d7a6422f745c42e9",
        26,
        "backup-sentinel-encryption-format",
        "backup-sentinel-key-metadata",
        "backup-sentinel-encryption-metadata",
    )
    .expect("snapshot")
}

fn proposal() -> GoogleProposalDraft {
    GoogleProposalDraft::from_owner(
        "google-operation",
        "pending title",
        range(),
        "Australia/Sydney",
        CalendarAttendee::needs_action(MailAddress::new("owner@example.test").expect("owner")),
    )
    .expect("proposal")
}

fn owner_draft() -> OwnerEventDraft {
    OwnerEventDraft::new(
        "owner-operation",
        "owner title",
        range(),
        "Australia/Sydney",
    )
    .expect("owner draft")
}

fn throttled() -> ProviderError {
    ProviderError::throttled(RetryAfter::new(Duration::seconds(1)).expect("retry after"))
}

fn assert_contract_debug_redacted(value: impl std::fmt::Debug) {
    let debug = format!("{value:?}");
    for sentinel in [
        "contract-token",
        "contract@example.test",
        "contract body",
        "contract title",
        "backup-sentinel-object-key",
        "backup-sentinel-ciphertext",
        "a3b7c87d9b77dc909f571008227641e45b11b4e2369ebcd57e87c714fd8b5fe5",
        "backup-sentinel-encryption-format",
        "backup-sentinel-key-metadata",
        "backup-sentinel-encryption-metadata",
        "fake-calendar:0",
        "fake-outlook-mail:0",
        "fake-gmail:0",
        "sentinel-calendar-change",
        "sentinel-event-id",
        "sentinel-message-id",
        "sentinel-sender@example.test",
        "sentinel-title",
        "sentinel-body",
        "sentinel-label",
        "calendar-one",
        "calendar-two",
        "mail-one",
        "mail-two",
        "mail-three",
        "mail-four",
        "bad-cursor",
        "fake-calendar",
        "fake-outlook-mail",
        "fake-gmail",
        "sentinel-owner-op",
        "sentinel-google-op",
        "sentinel-send-op",
        "fake-outlook-owner-event-1",
        "fake-google-proposal-event-1",
        "fake-gmail-sent-1",
        "fake-s3-version-1",
    ] {
        assert!(
            !debug.contains(sentinel),
            "debug leaked {sentinel}: {debug}"
        );
    }
}

async fn assert_gmail_send_common_failures(session: &ProviderSession) {
    let control = FakeControl::new(now());
    let gmail = FakeGmail::new(control.clone(), Vec::<MailMessage>::new());
    let provider: &dyn GmailProvider = &gmail;
    let second = OutboundMail::new(
        "send-persistent",
        MailAddress::new("to@example.test").expect("recipient"),
        "subject",
        "body",
    )
    .expect("outbound");
    let mut expected_count = 0;
    let mut expected_receipts = Vec::new();

    for (index, failure) in [
        ProviderError::TokenExpired,
        throttled(),
        ProviderError::Unavailable,
    ]
    .into_iter()
    .enumerate()
    {
        let mail = OutboundMail::new(
            format!("send-failure-{index}"),
            MailAddress::new("to@example.test").expect("recipient"),
            "subject",
            "body",
        )
        .expect("outbound");
        let receipt = SentMail::new(format!("fake-gmail-sent-{}", index + 1), now())
            .expect("recovered receipt");
        let before = gmail.sent_mail_receipts().expect("pre-failure sent state");
        control
            .queue_failure(FakeOperation::MailSend, failure)
            .expect("queue failure");
        assert_eq!(provider.send_mail(session, &mail).await, Err(failure));
        expected_count += 1;
        assert_eq!(
            control.invocation_count(FakeOperation::MailSend),
            Ok(expected_count)
        );
        assert_eq!(gmail.sent_mail_receipts(), Ok(before));
        assert_eq!(
            provider.send_mail(session, &mail).await,
            Ok(receipt.clone())
        );
        expected_count += 1;
        assert_eq!(
            control.invocation_count(FakeOperation::MailSend),
            Ok(expected_count)
        );
        expected_receipts.push(receipt);
        assert_eq!(gmail.sent_mail_receipts(), Ok(expected_receipts.clone()));
    }

    let before = gmail
        .sent_mail_receipts()
        .expect("pre-persistent sent state");
    control
        .set_failure(FakeOperation::MailSend, ProviderError::Unavailable)
        .expect("persistent failure");
    assert_eq!(
        provider.send_mail(session, &second).await,
        Err(ProviderError::Unavailable)
    );
    expected_count += 1;
    assert_eq!(
        control.invocation_count(FakeOperation::MailSend),
        Ok(expected_count)
    );
    assert_eq!(gmail.sent_mail_receipts(), Ok(before));
    control
        .clear_failure(FakeOperation::MailSend)
        .expect("clear failure");
    let second_receipt = SentMail::new("fake-gmail-sent-4", now()).expect("persistent receipt");
    assert_eq!(
        provider.send_mail(session, &second).await,
        Ok(second_receipt.clone())
    );
    expected_count += 1;
    assert_eq!(
        control.invocation_count(FakeOperation::MailSend),
        Ok(expected_count)
    );
    assert_eq!(
        gmail.sent_mail_receipts(),
        Ok({
            expected_receipts.push(second_receipt);
            expected_receipts
        })
    );
}

async fn assert_backup_common_failures(session: &ProviderSession) {
    let control = FakeControl::new(now());
    let backup = FakeEncryptedS3Backup::new(control.clone());
    let provider: &dyn EncryptedS3BackupProvider = &backup;
    let snapshot = snapshot();
    let receipt = BackupReceipt::new(
        snapshot.object_key(),
        "fake-s3-version-1",
        snapshot.checksum(),
        now(),
        snapshot.ciphertext_size(),
    )
    .expect("receipt");
    let mut expected_count = 0;

    for failure in [
        ProviderError::TokenExpired,
        throttled(),
        ProviderError::Unavailable,
    ] {
        let before = backup.stored_receipts().expect("pre-failure stored state");
        control
            .queue_failure(FakeOperation::BackupPut, failure)
            .expect("queue failure");
        assert_eq!(
            provider.put_snapshot(session, &snapshot).await,
            Err(failure)
        );
        expected_count += 1;
        assert_eq!(
            control.invocation_count(FakeOperation::BackupPut),
            Ok(expected_count)
        );
        assert_eq!(backup.stored_receipts(), Ok(before));
        assert_eq!(
            provider.put_snapshot(session, &snapshot).await,
            Ok(receipt.clone())
        );
        expected_count += 1;
        assert_eq!(
            control.invocation_count(FakeOperation::BackupPut),
            Ok(expected_count)
        );
        assert_eq!(backup.stored_receipts(), Ok(vec![receipt.clone()]));
    }

    let before = backup
        .stored_receipts()
        .expect("pre-persistent stored state");
    control
        .set_failure(FakeOperation::BackupPut, ProviderError::Unavailable)
        .expect("persistent failure");
    assert_eq!(
        provider.put_snapshot(session, &snapshot).await,
        Err(ProviderError::Unavailable)
    );
    expected_count += 1;
    assert_eq!(
        control.invocation_count(FakeOperation::BackupPut),
        Ok(expected_count)
    );
    assert_eq!(backup.stored_receipts(), Ok(before));
    control
        .clear_failure(FakeOperation::BackupPut)
        .expect("clear failure");
    assert_eq!(
        provider.put_snapshot(session, &snapshot).await,
        Ok(receipt.clone())
    );
    expected_count += 1;
    assert_eq!(
        control.invocation_count(FakeOperation::BackupPut),
        Ok(expected_count)
    );
    assert_eq!(backup.stored_receipts(), Ok(vec![receipt]));
}

async fn calendar_history(
    provider: &dyn CalendarReadProvider,
    session: &ProviderSession,
) -> Vec<CalendarChange> {
    provider
        .sync_calendar(session, &calendar_request(None, 16))
        .await
        .expect("calendar state")
        .items()
        .to_vec()
}

fn expected_busy_interval() -> BusyInterval {
    let range = range();
    BusyInterval::new(
        time::OffsetDateTime::from_unix_timestamp(range.start().timestamp()).expect("start"),
        time::OffsetDateTime::from_unix_timestamp(range.end().timestamp()).expect("end"),
    )
    .expect("busy interval")
}

async fn calendar_busy(
    provider: &dyn CalendarReadProvider,
    session: &ProviderSession,
) -> Vec<BusyInterval> {
    provider
        .list_busy(session, &range())
        .await
        .expect("calendar busy state")
}

async fn mail_state(
    provider: &dyn IncomingMailProvider,
    session: &ProviderSession,
) -> SyncPage<MailMessage> {
    provider
        .sync_mail(
            session,
            &MailSyncRequest::new(None, 16).expect("mail state request"),
        )
        .await
        .expect("mail state")
}

fn assert_exact_mail_items(page: &SyncPage<MailMessage>, expected: &[(&str, &str, &str, &[&str])]) {
    assert_eq!(page.items().len(), expected.len());
    for (message, (id, subject, body, labels)) in page.items().iter().zip(expected) {
        assert_eq!(message.source_id().as_str(), *id);
        assert_eq!(message.subject(), *subject);
        assert_eq!(message.body(), *body);
        assert_eq!(message.labels(), *labels);
    }
}

#[tokio::test]
async fn mutation_failures_preserve_public_state_until_exact_recovery() {
    let s = session();
    let failures = [
        ProviderError::TokenExpired,
        throttled(),
        ProviderError::Unavailable,
    ];

    let owner_control = FakeControl::new(now());
    let outlook = FakeOutlookCalendar::new(
        owner_control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let oc: &dyn OutlookCalendarProvider = &outlook;
    let mut expected_owner_history = Vec::new();
    let expected_owner_busy = vec![expected_busy_interval()];
    for failure in failures {
        let before_history = calendar_history(oc, &s).await;
        let before_busy = calendar_busy(oc, &s).await;
        assert_eq!(before_history, expected_owner_history);
        owner_control
            .queue_failure(FakeOperation::CalendarOwnerCreate, failure)
            .expect("queue owner failure");
        assert_eq!(
            oc.create_owner_event(&s, &owner_draft()).await,
            Err(failure)
        );
        assert_eq!(calendar_history(oc, &s).await, before_history);
        assert_eq!(calendar_busy(oc, &s).await, before_busy);
        let recovered = oc
            .create_owner_event(&s, &owner_draft())
            .await
            .expect("owner recovery");
        assert_eq!(recovered.operation_key(), "owner-operation");
        assert_eq!(recovered.provider_event_id(), "fake-outlook-owner-event-1");
        expected_owner_history = vec![CalendarChange::upsert(recovered).expect("owner change")];
        assert_eq!(calendar_history(oc, &s).await, expected_owner_history);
        assert_eq!(calendar_busy(oc, &s).await, expected_owner_busy);
    }

    let google_control = FakeControl::new(now());
    let google = FakeGoogleCalendar::new(
        google_control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let gc: &dyn GoogleCalendarProvider = &google;
    let mut expected_google_history = Vec::new();
    let mut expected_google_busy = Vec::new();
    let mut created = None;
    for failure in failures {
        let before_history = calendar_history(gc, &s).await;
        let before_busy = calendar_busy(gc, &s).await;
        assert_eq!(before_history, expected_google_history);
        google_control
            .queue_failure(FakeOperation::CalendarProposalCreate, failure)
            .expect("queue proposal failure");
        assert_eq!(gc.create_proposal(&s, &proposal()).await, Err(failure));
        assert_eq!(calendar_history(gc, &s).await, before_history);
        assert_eq!(calendar_busy(gc, &s).await, before_busy);
        let recovered = gc
            .create_proposal(&s, &proposal())
            .await
            .expect("proposal recovery");
        assert_eq!(recovered.operation_key(), "google-operation");
        assert_eq!(
            recovered.provider_event_id(),
            "fake-google-proposal-event-1"
        );
        expected_google_history = vec![CalendarChange::upsert(recovered.clone()).expect("create")];
        if expected_google_busy.is_empty() {
            expected_google_busy.push(expected_busy_interval());
        }
        assert_eq!(calendar_history(gc, &s).await, expected_google_history);
        assert_eq!(calendar_busy(gc, &s).await, expected_google_busy);
        created = Some(recovered);
    }
    let created = created.expect("created proposal");
    let id = ProviderEventId::new(created.provider_event_id()).expect("proposal id");
    let accepted = google.set_owner_rsvp(&id, Rsvp::Accepted).expect("accept");
    expected_google_history.push(CalendarChange::upsert(accepted).expect("accept change"));
    let promotion =
        GoogleProposalPromotion::new(id.as_str(), "final", None, true).expect("promotion");
    let mut promoted = None;
    for failure in failures {
        let before_history = calendar_history(gc, &s).await;
        let before_busy = calendar_busy(gc, &s).await;
        assert_eq!(before_history, expected_google_history);
        assert_eq!(before_busy, expected_google_busy);
        google_control
            .queue_failure(FakeOperation::CalendarPromote, failure)
            .expect("queue promotion failure");
        assert_eq!(gc.promote_proposal(&s, &promotion).await, Err(failure));
        assert_eq!(calendar_history(gc, &s).await, before_history);
        assert_eq!(calendar_busy(gc, &s).await, before_busy);
        let recovered = gc
            .promote_proposal(&s, &promotion)
            .await
            .expect("promotion recovery");
        assert_eq!(recovered.operation_key(), "google-operation");
        assert_eq!(
            recovered.provider_event_id(),
            "fake-google-proposal-event-1"
        );
        if promoted.is_none() {
            expected_google_history
                .push(CalendarChange::upsert(recovered.clone()).expect("promotion"));
        }
        assert_eq!(calendar_history(gc, &s).await, expected_google_history);
        assert_eq!(calendar_busy(gc, &s).await, expected_google_busy);
        promoted = Some(recovered);
    }
    assert_eq!(promoted.expect("promoted").title(), "final");
    for failure in failures {
        let before_history = calendar_history(gc, &s).await;
        let before_busy = calendar_busy(gc, &s).await;
        assert_eq!(before_history, expected_google_history);
        assert_eq!(before_busy, expected_google_busy);
        google_control
            .queue_failure(FakeOperation::CalendarDelete, failure)
            .expect("queue delete failure");
        assert_eq!(gc.delete_proposal(&s, &id).await, Err(failure));
        assert_eq!(calendar_history(gc, &s).await, before_history);
        assert_eq!(calendar_busy(gc, &s).await, before_busy);
        gc.delete_proposal(&s, &id).await.expect("delete recovery");
        if !matches!(
            expected_google_history.last(),
            Some(CalendarChange::Deleted { .. })
        ) {
            expected_google_history
                .push(CalendarChange::deleted(id.as_str(), now()).expect("delete change"));
        }
        expected_google_busy.clear();
        assert_eq!(calendar_history(gc, &s).await, expected_google_history);
        assert_eq!(calendar_busy(gc, &s).await, expected_google_busy);
    }

    let mail_control = FakeControl::new(now());
    let outlook_mail = FakeOutlookMail::new(mail_control.clone(), [message("mail-state")]);
    let incoming: &dyn IncomingMailProvider = &outlook_mail;
    let expected_mail = [(
        "mail-state",
        "contract title",
        "contract body",
        &["inbox"][..],
    )];
    for failure in failures {
        let before = mail_state(incoming, &s).await;
        assert_exact_mail_items(&before, &expected_mail);
        mail_control
            .queue_failure(FakeOperation::MailSync, failure)
            .expect("queue mail failure");
        assert_eq!(
            incoming
                .sync_mail(&s, &MailSyncRequest::new(None, 16).expect("mail request"))
                .await,
            Err(failure)
        );
        let after = mail_state(incoming, &s).await;
        assert_eq!(after, before);
        assert_exact_mail_items(&after, &expected_mail);
        let recovered = mail_state(incoming, &s).await;
        assert_eq!(recovered, before);
        assert_exact_mail_items(&recovered, &expected_mail);
    }

    let gmail_control = FakeControl::new(now());
    let gmail = FakeGmail::new(gmail_control.clone(), [message("label-state")]);
    let gm: &dyn GmailProvider = &gmail;
    let label_id = MailMessageId::new("label-state").expect("label id");
    let labels = LabelChanges::new(["done"], ["inbox"]).expect("labels");
    let expected_unlabelled = [(
        "label-state",
        "contract title",
        "contract body",
        &["inbox"][..],
    )];
    let expected_labelled = [
        (
            "label-state",
            "contract title",
            "contract body",
            &["inbox"][..],
        ),
        (
            "label-state",
            "contract title",
            "contract body",
            &["done"][..],
        ),
    ];
    for failure in failures {
        let before = mail_state(gm, &s).await;
        gmail_control
            .queue_failure(FakeOperation::MailLabels, failure)
            .expect("queue label failure");
        assert_eq!(gm.modify_labels(&s, &label_id, &labels).await, Err(failure));
        let after = mail_state(gm, &s).await;
        assert_eq!(after, before);
        if before.items().len() == 1 {
            assert_exact_mail_items(&after, &expected_unlabelled);
        } else {
            assert_exact_mail_items(&after, &expected_labelled);
        }
        gm.modify_labels(&s, &label_id, &labels)
            .await
            .expect("label recovery");
        let recovered = mail_state(gm, &s).await;
        assert_exact_mail_items(&recovered, &expected_labelled);
    }

    let triage_control = FakeControl::new(now());
    let input = triage_input("triage-state");
    let decision = TriageDecision::Ambiguous(AmbiguousReason::UnclearAction);
    let triage =
        FakeStructuredTriage::new(triage_control.clone(), [(input.clone(), decision.clone())]);
    let tp: &dyn StructuredTriageProvider = &triage;
    for failure in failures {
        let before = tp.classify(&s, &input).await.expect("triage state");
        assert_eq!(before, decision);
        triage_control
            .queue_failure(FakeOperation::TriageClassify, failure)
            .expect("queue triage failure");
        assert_eq!(tp.classify(&s, &input).await, Err(failure));
        assert_eq!(tp.classify(&s, &input).await, Ok(before.clone()));
        assert_eq!(tp.classify(&s, &input).await, Ok(decision.clone()));
    }
}

#[tokio::test]
async fn failed_label_and_delete_leave_durable_content_unchanged_before_exact_recovery() {
    let s = session();
    let control = FakeControl::new(now());
    let gmail = FakeGmail::new(control.clone(), [message("label-source")]);
    let gm: &dyn GmailProvider = &gmail;
    let label_id = MailMessageId::new("label-source").expect("mail id");
    let changes = LabelChanges::new(["done"], ["inbox"]).expect("labels");
    control
        .queue_failure(FakeOperation::MailLabels, ProviderError::TokenExpired)
        .expect("queue label failure");
    assert_eq!(
        gm.modify_labels(&s, &label_id, &changes).await,
        Err(ProviderError::TokenExpired)
    );
    let before = gm
        .sync_mail(&s, &MailSyncRequest::new(None, 2).expect("request"))
        .await
        .expect("unchanged mail");
    assert_eq!(before.items().len(), 1);
    assert_eq!(before.items()[0].labels(), ["inbox"]);
    gm.modify_labels(&s, &label_id, &changes)
        .await
        .expect("label recovery");
    let after = gm
        .sync_mail(&s, &MailSyncRequest::new(None, 2).expect("request"))
        .await
        .expect("label state");
    assert_eq!(after.items().len(), 2);
    assert_eq!(after.items()[1].source_id().as_str(), "label-source");
    assert_eq!(after.items()[1].labels(), ["done"]);

    let google = FakeGoogleCalendar::new(
        control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let gc: &dyn GoogleCalendarProvider = &google;
    let created = gc.create_proposal(&s, &proposal()).await.expect("proposal");
    let id = ProviderEventId::new(created.provider_event_id()).expect("event id");
    control
        .queue_failure(FakeOperation::CalendarDelete, ProviderError::TokenExpired)
        .expect("queue delete failure");
    assert_eq!(
        gc.delete_proposal(&s, &id).await,
        Err(ProviderError::TokenExpired)
    );
    assert_eq!(
        gc.list_busy(&s, &range())
            .await
            .expect("retained busy")
            .len(),
        1
    );
    gc.delete_proposal(&s, &id).await.expect("delete recovery");
    assert!(
        gc.list_busy(&s, &range())
            .await
            .expect("deleted busy")
            .is_empty()
    );
    let changes = gc
        .sync_calendar(&s, &calendar_request(None, 8))
        .await
        .expect("calendar history");
    assert!(matches!(
        changes.items().last(),
        Some(CalendarChange::Deleted { provider_event_id, .. }) if provider_event_id == id.as_str()
    ));
}

#[tokio::test]
async fn trait_objects_cover_happy_paths_including_google_gmail_and_triage_variants() {
    let control = FakeControl::new(now());
    let s = session();
    let changes = [CalendarChange::deleted("google-read-change", now()).expect("change")];
    let outlook =
        FakeOutlookCalendar::new(control.clone(), Vec::<BusyInterval>::new(), changes.clone());
    let google = FakeGoogleCalendar::new(control.clone(), Vec::<BusyInterval>::new(), changes);
    let oc: &dyn OutlookCalendarProvider = &outlook;
    let gc: &dyn GoogleCalendarProvider = &google;
    let outlook_busy = oc.list_busy(&s, &range()).await.expect("outlook busy");
    assert!(outlook_busy.is_empty());
    assert_contract_debug_redacted(outlook_busy);
    let outlook_page = oc
        .sync_calendar(&s, &calendar_request(None, 1))
        .await
        .expect("outlook read");
    assert_eq!(outlook_page.items().len(), 1);
    assert_contract_debug_redacted(outlook_page);
    let google_page = gc
        .sync_calendar(&s, &calendar_request(None, 1))
        .await
        .expect("google read");
    assert_eq!(google_page.items().len(), 1);
    assert_contract_debug_redacted(google_page);
    let owner = oc
        .create_owner_event(&s, &owner_draft())
        .await
        .expect("owner create");
    assert_eq!(owner.title(), "owner title");
    assert_contract_debug_redacted(owner);
    let created = gc
        .create_proposal(&s, &proposal())
        .await
        .expect("proposal create");
    assert_contract_debug_redacted(created.clone());
    let id = ProviderEventId::new(created.provider_event_id()).expect("event id");
    google.set_owner_rsvp(&id, Rsvp::Accepted).expect("accept");
    let promoted = gc
        .promote_proposal(
            &s,
            &GoogleProposalPromotion::new(id.as_str(), "final title", None, true)
                .expect("promotion"),
        )
        .await
        .expect("promotion");
    assert_eq!(promoted.title(), "final title");
    assert_contract_debug_redacted(promoted);
    gc.delete_proposal(&s, &id).await.expect("delete");

    let outlook_mail = FakeOutlookMail::new(control.clone(), [message("outlook-mail")]);
    let gmail = FakeGmail::new(control.clone(), [message("gmail-mail")]);
    let om: &dyn OutlookMailProvider = &outlook_mail;
    let gm: &dyn GmailProvider = &gmail;
    let outlook_mail_page = om
        .sync_mail(&s, &MailSyncRequest::new(None, 1).expect("request"))
        .await
        .expect("outlook mail");
    assert_eq!(outlook_mail_page.items().len(), 1);
    assert_contract_debug_redacted(outlook_mail_page);
    let gmail_mail_page = gm
        .sync_mail(&s, &MailSyncRequest::new(None, 1).expect("request"))
        .await
        .expect("gmail mail");
    assert_eq!(gmail_mail_page.items().len(), 1);
    assert_contract_debug_redacted(gmail_mail_page);
    let labels = LabelChanges::new(["done"], ["inbox"]).expect("labels");
    gm.modify_labels(
        &s,
        &MailMessageId::new("gmail-mail").expect("mail id"),
        &labels,
    )
    .await
    .expect("labels");
    let outbound = OutboundMail::new(
        "mail-operation",
        MailAddress::new("to@example.test").expect("recipient"),
        "subject",
        "body",
    )
    .expect("outbound");
    let sent = gm.send_mail(&s, &outbound).await.expect("send");
    assert_eq!(gm.send_mail(&s, &outbound).await, Ok(sent.clone()));
    assert_contract_debug_redacted(sent);

    let actionable = TriageDecision::Actionable(
        ActionableTaskExtraction::new(TaskKind::Callback, "call", 15, Some(now())).expect("task"),
    );
    let ambiguous = TriageDecision::Ambiguous(AmbiguousReason::UnclearAction);
    let ignore = TriageDecision::Ignore;
    let triage = FakeStructuredTriage::new(
        control.clone(),
        [
            (triage_input("actionable"), actionable.clone()),
            (triage_input("ambiguous"), ambiguous.clone()),
            (triage_input("ignore"), ignore.clone()),
        ],
    );
    let tp: &dyn StructuredTriageProvider = &triage;
    let actionable_result = tp
        .classify(&s, &triage_input("actionable"))
        .await
        .expect("actionable");
    assert_eq!(actionable_result, actionable);
    assert_contract_debug_redacted(actionable_result);
    let ambiguous_result = tp
        .classify(&s, &triage_input("ambiguous"))
        .await
        .expect("ambiguous");
    assert_eq!(ambiguous_result, ambiguous);
    assert_contract_debug_redacted(ambiguous_result);
    let ignore_result = tp
        .classify(&s, &triage_input("ignore"))
        .await
        .expect("ignore");
    assert_eq!(ignore_result, ignore);
    assert_contract_debug_redacted(ignore_result);

    let backup = FakeEncryptedS3Backup::new(control.clone());
    let bp: &dyn EncryptedS3BackupProvider = &backup;
    let receipt = bp.put_snapshot(&s, &snapshot()).await.expect("backup");
    assert_eq!(bp.put_snapshot(&s, &snapshot()).await, Ok(receipt.clone()));
    assert_contract_debug_redacted(receipt);
}

#[tokio::test]
async fn every_sentinelized_return_value_formats_without_private_payloads() {
    let s = session();
    let control = FakeControl::new(now());
    let changes = [CalendarChange::deleted("sentinel-calendar-change", now()).expect("change")];
    let outlook =
        FakeOutlookCalendar::new(control.clone(), Vec::<BusyInterval>::new(), changes.clone());
    let google = FakeGoogleCalendar::new(control.clone(), Vec::<BusyInterval>::new(), changes);
    let oc: &dyn OutlookCalendarProvider = &outlook;
    let gc: &dyn GoogleCalendarProvider = &google;
    assert_contract_debug_redacted(oc.list_busy(&s, &range()).await.expect("busy"));
    assert_contract_debug_redacted(
        oc.sync_calendar(&s, &calendar_request(None, 1))
            .await
            .expect("outlook page"),
    );
    assert_contract_debug_redacted(&outlook);
    assert_contract_debug_redacted(
        gc.sync_calendar(&s, &calendar_request(None, 1))
            .await
            .expect("google page"),
    );
    let owner = OwnerEventDraft::new(
        "sentinel-owner-op",
        "sentinel-title",
        range(),
        "Australia/Sydney",
    )
    .expect("owner");
    let owner_event = oc
        .create_owner_event(&s, &owner)
        .await
        .expect("owner event");
    assert_eq!(owner_event.operation_key(), "sentinel-owner-op");
    assert_eq!(
        owner_event.provider_event_id(),
        "fake-outlook-owner-event-1"
    );
    assert_contract_debug_redacted(owner_event);
    let draft = GoogleProposalDraft::from_owner(
        "sentinel-google-op",
        "sentinel-title",
        range(),
        "Australia/Sydney",
        CalendarAttendee::needs_action(
            MailAddress::new("sentinel-sender@example.test").expect("owner"),
        ),
    )
    .expect("proposal");
    let created = gc.create_proposal(&s, &draft).await.expect("created");
    assert_eq!(created.operation_key(), "sentinel-google-op");
    assert_eq!(created.provider_event_id(), "fake-google-proposal-event-1");
    assert_contract_debug_redacted(created.clone());
    let id = ProviderEventId::new(created.provider_event_id()).expect("id");
    google.set_owner_rsvp(&id, Rsvp::Accepted).expect("accept");
    let promotion =
        GoogleProposalPromotion::new(id.as_str(), "sentinel-title", None, true).expect("promotion");
    let promoted = gc.promote_proposal(&s, &promotion).await.expect("promoted");
    assert_eq!(promoted.operation_key(), "sentinel-google-op");
    assert_eq!(promoted.provider_event_id(), "fake-google-proposal-event-1");
    assert_contract_debug_redacted(promoted);
    assert_contract_debug_redacted(&google);

    let outlook_mail =
        FakeOutlookMail::new(control.clone(), [sentinel_message("sentinel-message-id")]);
    let gmail = FakeGmail::new(control.clone(), [sentinel_message("sentinel-message-id")]);
    let outlook_page = (&outlook_mail as &dyn IncomingMailProvider)
        .sync_mail(&s, &MailSyncRequest::new(None, 1).expect("request"))
        .await
        .expect("outlook mail");
    assert_eq!(
        outlook_page.items()[0].source_id().as_str(),
        "sentinel-message-id"
    );
    assert_contract_debug_redacted(outlook_page);
    let gmail_page = (&gmail as &dyn IncomingMailProvider)
        .sync_mail(&s, &MailSyncRequest::new(None, 1).expect("request"))
        .await
        .expect("gmail mail");
    assert_eq!(
        gmail_page.items()[0].source_id().as_str(),
        "sentinel-message-id"
    );
    assert_contract_debug_redacted(gmail_page);
    let sent = (&gmail as &dyn GmailProvider)
        .send_mail(
            &s,
            &OutboundMail::new(
                "sentinel-send-op",
                MailAddress::new("sentinel-sender@example.test").expect("recipient"),
                "sentinel-title",
                "sentinel-body",
            )
            .expect("outbound"),
        )
        .await
        .expect("sent");
    assert_contract_debug_redacted(sent);
    let gmail_debug = format!("{gmail:?}");
    assert!(!gmail_debug.is_empty());
    assert!(gmail_debug.contains("sent_mail_count: 1"));
    assert_contract_debug_redacted(gmail_debug);

    let actionable = TriageDecision::Actionable(
        ActionableTaskExtraction::new(TaskKind::Callback, "sentinel-title", 15, Some(now()))
            .expect("actionable"),
    );
    let ambiguous = TriageDecision::Ambiguous(AmbiguousReason::UnclearAction);
    let ignore = TriageDecision::Ignore;
    let triage = FakeStructuredTriage::new(
        control.clone(),
        [
            (triage_input("sentinel-triage-action"), actionable),
            (triage_input("sentinel-triage-ambiguous"), ambiguous),
            (triage_input("sentinel-triage-ignore"), ignore),
        ],
    );
    let tp: &dyn StructuredTriageProvider = &triage;
    assert_contract_debug_redacted(
        tp.classify(&s, &triage_input("sentinel-triage-action"))
            .await
            .expect("actionable"),
    );
    assert_contract_debug_redacted(
        tp.classify(&s, &triage_input("sentinel-triage-ambiguous"))
            .await
            .expect("ambiguous"),
    );
    assert_contract_debug_redacted(
        tp.classify(&s, &triage_input("sentinel-triage-ignore"))
            .await
            .expect("ignore"),
    );

    let backup = FakeEncryptedS3Backup::new(control.clone());
    let receipt = (&backup as &dyn EncryptedS3BackupProvider)
        .put_snapshot(&s, &snapshot())
        .await
        .expect("receipt");
    assert_eq!(receipt.object_key(), "backup-sentinel-object-key");
    assert_eq!(
        receipt.checksum(),
        "2e46e41589939dd28a664c8d40d852b3800c01cdb32e87f3d7a6422f745c42e9"
    );
    assert_eq!(receipt.provider_version(), "fake-s3-version-1");
    assert_contract_debug_redacted(receipt);
    assert_contract_debug_redacted(&backup);
}

macro_rules! assert_common_failures {
    ($control:expr, $operation:expr, $call:expr, $recover:expr) => {{
        let control = &$control;
        let mut expected_count = control.invocation_count($operation).expect("initial count");
        for failure in [
            ProviderError::TokenExpired,
            throttled(),
            ProviderError::Unavailable,
        ] {
            control
                .queue_failure($operation, failure)
                .expect("queue failure");
            assert_eq!($call.await, Err(failure));
            expected_count += 1;
            assert_eq!(control.invocation_count($operation), Ok(expected_count));
            $recover;
            expected_count += 1;
            assert_eq!(control.invocation_count($operation), Ok(expected_count));
        }
        control
            .set_failure($operation, ProviderError::Unavailable)
            .expect("persistent failure");
        assert_eq!($call.await, Err(ProviderError::Unavailable));
        expected_count += 1;
        control.clear_failure($operation).expect("clear failure");
        $recover;
        expected_count += 1;
        assert_eq!(control.invocation_count($operation), Ok(expected_count));
    }};
}

#[tokio::test]
async fn every_operation_reports_common_failures_with_exact_counts_and_recovers() {
    let s = session();
    let control = FakeControl::new(now());
    let outlook = FakeOutlookCalendar::new(
        control.clone(),
        Vec::<BusyInterval>::new(),
        [CalendarChange::deleted("c", now()).expect("change")],
    );
    let calendar: &dyn OutlookCalendarProvider = &outlook;
    assert_common_failures!(
        control,
        FakeOperation::CalendarBusy,
        calendar.list_busy(&s, &range()),
        {
            assert!(
                calendar
                    .list_busy(&s, &range())
                    .await
                    .expect("recovery")
                    .is_empty()
            )
        }
    );
    assert_common_failures!(
        control,
        FakeOperation::CalendarSync,
        calendar.sync_calendar(&s, &calendar_request(None, 1)),
        {
            assert_eq!(
                calendar
                    .sync_calendar(&s, &calendar_request(None, 1))
                    .await
                    .expect("recovery")
                    .items()
                    .len(),
                1
            )
        }
    );
    assert_common_failures!(
        control,
        FakeOperation::CalendarOwnerCreate,
        calendar.create_owner_event(&s, &owner_draft()),
        {
            assert_eq!(
                calendar
                    .create_owner_event(&s, &owner_draft())
                    .await
                    .expect("recovery")
                    .title(),
                "owner title"
            )
        }
    );
    calendar
        .create_owner_event(&s, &owner_draft())
        .await
        .expect("owner lookup fixture");
    assert_common_failures!(
        control,
        FakeOperation::CalendarOwnerFind,
        calendar.find_owner_event(&s, &owner_draft()),
        {
            assert_eq!(
                calendar
                    .find_owner_event(&s, &owner_draft())
                    .await
                    .expect("owner lookup recovery")
                    .title(),
                "owner title"
            )
        }
    );

    let google = FakeGoogleCalendar::new(
        control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let gc: &dyn GoogleCalendarProvider = &google;
    assert_common_failures!(
        control,
        FakeOperation::CalendarProposalCreate,
        gc.create_proposal(&s, &proposal()),
        {
            assert_eq!(
                gc.create_proposal(&s, &proposal())
                    .await
                    .expect("recovery")
                    .title(),
                "pending title"
            )
        }
    );
    let created = gc.create_proposal(&s, &proposal()).await.expect("created");
    assert_common_failures!(
        control,
        FakeOperation::CalendarProposalFind,
        gc.find_proposal(&s, &proposal()),
        {
            assert_eq!(
                gc.find_proposal(&s, &proposal())
                    .await
                    .expect("proposal lookup recovery")
                    .title(),
                "pending title"
            )
        }
    );
    let id = ProviderEventId::new(created.provider_event_id()).expect("id");
    google
        .set_owner_rsvp(&id, Rsvp::Accepted)
        .expect("accepted");
    let promotion =
        GoogleProposalPromotion::new(id.as_str(), "final", None, true).expect("promotion");
    assert_common_failures!(
        control,
        FakeOperation::CalendarPromote,
        gc.promote_proposal(&s, &promotion),
        {
            assert_eq!(
                gc.promote_proposal(&s, &promotion)
                    .await
                    .expect("recovery")
                    .title(),
                "final"
            )
        }
    );
    assert_common_failures!(
        control,
        FakeOperation::CalendarDelete,
        gc.delete_proposal(&s, &id),
        { gc.delete_proposal(&s, &id).await.expect("recovery") }
    );

    let outlook_mail = FakeOutlookMail::new(control.clone(), [message("outlook")]);
    let incoming: &dyn IncomingMailProvider = &outlook_mail;
    assert_common_failures!(
        control,
        FakeOperation::MailSync,
        incoming.sync_mail(&s, &MailSyncRequest::new(None, 1).expect("request")),
        {
            assert_eq!(
                incoming
                    .sync_mail(&s, &MailSyncRequest::new(None, 1).expect("request"))
                    .await
                    .expect("recovery")
                    .items()
                    .len(),
                1
            )
        }
    );
    let gmail = FakeGmail::new(control.clone(), [message("gmail")]);
    let gm: &dyn GmailProvider = &gmail;
    let mail_id = MailMessageId::new("gmail").expect("id");
    let labels = LabelChanges::new(["done"], ["inbox"]).expect("labels");
    assert_common_failures!(
        control,
        FakeOperation::MailLabels,
        gm.modify_labels(&s, &mail_id, &labels),
        {
            gm.modify_labels(&s, &mail_id, &labels)
                .await
                .expect("recovery")
        }
    );
    assert_gmail_send_common_failures(&s).await;
    let input = triage_input("triage");
    let decision = TriageDecision::Ignore;
    let triage = FakeStructuredTriage::new(control.clone(), [(input.clone(), decision.clone())]);
    let tp: &dyn StructuredTriageProvider = &triage;
    assert_common_failures!(
        control,
        FakeOperation::TriageClassify,
        tp.classify(&s, &input),
        { assert_eq!(tp.classify(&s, &input).await, Ok(decision.clone())) }
    );
    assert_backup_common_failures(&s).await;
}

#[tokio::test]
async fn calendar_and_mail_cursor_expiry_and_partial_retries_cover_prefix_and_zero_success() {
    let s = session();
    let calendar_control = FakeControl::new(now());
    let calendar = FakeGoogleCalendar::new(
        calendar_control.clone(),
        Vec::<BusyInterval>::new(),
        [
            CalendarChange::deleted("calendar-one", now()).expect("change"),
            CalendarChange::deleted("calendar-two", now()).expect("change"),
        ],
    );
    let cp: &dyn CalendarReadProvider = &calendar;
    assert_eq!(
        cp.sync_calendar(&s, &calendar_request(Some("bad-cursor".to_owned()), 2))
            .await,
        Err(ProviderError::CursorExpired)
    );
    calendar_control
        .set_partial_failure(FakeOperation::CalendarSync, 1)
        .expect("partial");
    let partial = cp
        .sync_calendar(&s, &calendar_request(None, 2))
        .await
        .expect("partial page");
    assert_eq!(partial.items().len(), 1);
    assert_contract_debug_redacted(&partial);
    assert_eq!(
        partial.item_failures()[0].error(),
        ProviderError::Unavailable
    );
    assert_eq!(partial.item_failures()[0].source_id(), "fake-calendar");
    assert_eq!(partial.item_failures()[0].item_id(), "calendar-two");
    calendar_control
        .clear_partial_failure(FakeOperation::CalendarSync)
        .expect("clear");
    let retry = cp
        .sync_calendar(
            &s,
            &calendar_request(partial.next_cursor().map(str::to_owned), 2),
        )
        .await
        .expect("retry");
    assert_eq!(retry.items()[0].provider_event_id(), "calendar-two");
    assert_contract_debug_redacted(retry);
    calendar_control
        .set_partial_failure(FakeOperation::CalendarSync, 0)
        .expect("zero partial");
    let zero = cp
        .sync_calendar(&s, &calendar_request(None, 2))
        .await
        .expect("zero page");
    assert!(zero.items().is_empty());
    assert_contract_debug_redacted(&zero);
    assert_eq!(zero.next_cursor(), Some("fake-calendar:0"));
    calendar_control
        .clear_partial_failure(FakeOperation::CalendarSync)
        .expect("clear");
    let zero_retry = cp
        .sync_calendar(
            &s,
            &calendar_request(zero.next_cursor().map(str::to_owned), 2),
        )
        .await
        .expect("zero retry");
    assert_eq!(
        zero_retry
            .items()
            .iter()
            .map(CalendarChange::provider_event_id)
            .collect::<Vec<_>>(),
        ["calendar-one", "calendar-two"]
    );
    assert_contract_debug_redacted(zero_retry);

    let outlook_mail = FakeOutlookMail::new(
        FakeControl::new(now()),
        [message("mail-one"), message("mail-two")],
    );
    let gmail_mail = FakeGmail::new(
        FakeControl::new(now()),
        [message("mail-three"), message("mail-four")],
    );
    for mp in [
        &outlook_mail as &dyn IncomingMailProvider,
        &gmail_mail as &dyn IncomingMailProvider,
    ] {
        let control = if std::ptr::eq(mp, &outlook_mail as &dyn IncomingMailProvider) {
            outlook_mail.control().clone()
        } else {
            gmail_mail.control().clone()
        };
        assert_eq!(
            mp.sync_mail(
                &s,
                &MailSyncRequest::new(Some("bad-cursor".to_owned()), 2).expect("request")
            )
            .await,
            Err(ProviderError::CursorExpired)
        );
        control
            .set_partial_failure(FakeOperation::MailSync, 1)
            .expect("partial");
        let partial = mp
            .sync_mail(&s, &MailSyncRequest::new(None, 2).expect("request"))
            .await
            .expect("partial page");
        let (first_id, second_id) = if std::ptr::eq(mp, &outlook_mail as &dyn IncomingMailProvider)
        {
            ("mail-one", "mail-two")
        } else {
            ("mail-three", "mail-four")
        };
        assert_exact_mail_items(
            &partial,
            &[(first_id, "contract title", "contract body", &["inbox"][..])],
        );
        assert_eq!(
            partial.item_failures()[0].source_id(),
            if std::ptr::eq(mp, &outlook_mail as &dyn IncomingMailProvider) {
                "fake-outlook-mail"
            } else {
                "fake-gmail"
            }
        );
        assert_eq!(
            partial.item_failures()[0].item_id(),
            if std::ptr::eq(mp, &outlook_mail as &dyn IncomingMailProvider) {
                "mail-two"
            } else {
                "mail-four"
            }
        );
        assert_contract_debug_redacted(&partial);
        control
            .clear_partial_failure(FakeOperation::MailSync)
            .expect("clear");
        let retry = mp
            .sync_mail(
                &s,
                &MailSyncRequest::new(partial.next_cursor().map(str::to_owned), 2)
                    .expect("request"),
            )
            .await
            .expect("retry");
        assert_exact_mail_items(
            &retry,
            &[(second_id, "contract title", "contract body", &["inbox"][..])],
        );
        assert_eq!(retry.item_failures(), []);
        assert_contract_debug_redacted(retry);
        control
            .set_partial_failure(FakeOperation::MailSync, 0)
            .expect("zero partial");
        let zero = mp
            .sync_mail(&s, &MailSyncRequest::new(None, 2).expect("request"))
            .await
            .expect("zero page");
        assert!(zero.items().is_empty());
        assert_contract_debug_redacted(&zero);
        let expected_cursor = if std::ptr::eq(mp, &outlook_mail as &dyn IncomingMailProvider) {
            "fake-outlook-mail:0"
        } else {
            "fake-gmail:0"
        };
        assert_eq!(zero.next_cursor(), Some(expected_cursor));
        control
            .clear_partial_failure(FakeOperation::MailSync)
            .expect("clear");
        let zero_retry = mp
            .sync_mail(
                &s,
                &MailSyncRequest::new(zero.next_cursor().map(str::to_owned), 2).expect("request"),
            )
            .await
            .expect("zero retry");
        assert_exact_mail_items(
            &zero_retry,
            &[
                (first_id, "contract title", "contract body", &["inbox"][..]),
                (second_id, "contract title", "contract body", &["inbox"][..]),
            ],
        );
        assert_eq!(zero_retry.item_failures(), []);
        assert_contract_debug_redacted(zero_retry);
    }
}

#[tokio::test]
async fn cloned_fakes_share_one_durable_idempotent_result_for_every_mutation() {
    let s = session();
    let control = FakeControl::new(now());
    let outlook = FakeOutlookCalendar::new(
        control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let owner_gate = Arc::new(Barrier::new(3));
    let owner_one = outlook.clone();
    let owner_two = outlook.clone();
    let owner_one_gate = owner_gate.clone();
    let owner_two_gate = owner_gate.clone();
    let owner_one_session = s.clone();
    let owner_two_session = s.clone();
    let owner_draft = owner_draft();
    let owner_one_draft = owner_draft.clone();
    let owner_two_draft = owner_draft.clone();
    let owner = tokio::spawn(async move {
        owner_one_gate.wait().await;
        owner_one
            .create_owner_event(&owner_one_session, &owner_one_draft)
            .await
    });
    let owner_retry = tokio::spawn(async move {
        owner_two_gate.wait().await;
        owner_two
            .create_owner_event(&owner_two_session, &owner_two_draft)
            .await
    });
    owner_gate.wait().await;
    let owner = owner.await.expect("owner task");
    let owner_retry = owner_retry.await.expect("owner retry task");
    assert_eq!(owner, owner_retry);
    let owner = owner.expect("owner create");
    assert_eq!(owner.operation_key(), owner_draft.operation_key());
    assert_eq!(
        outlook
            .sync_calendar(&s, &calendar_request(None, 8))
            .await
            .expect("owner history")
            .items(),
        [CalendarChange::upsert(owner.clone()).expect("owner change")]
    );
    let google = FakeGoogleCalendar::new(
        control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let create_gate = Arc::new(Barrier::new(3));
    let create_one = google.clone();
    let create_two = google.clone();
    let create_one_gate = create_gate.clone();
    let create_two_gate = create_gate.clone();
    let create_one_session = s.clone();
    let create_two_session = s.clone();
    let proposal = proposal();
    let create_one_draft = proposal.clone();
    let create_two_draft = proposal.clone();
    let created = tokio::spawn(async move {
        create_one_gate.wait().await;
        create_one
            .create_proposal(&create_one_session, &create_one_draft)
            .await
    });
    let created_retry = tokio::spawn(async move {
        create_two_gate.wait().await;
        create_two
            .create_proposal(&create_two_session, &create_two_draft)
            .await
    });
    create_gate.wait().await;
    let created = created.await.expect("create task");
    let created_retry = created_retry.await.expect("create retry task");
    assert_eq!(created, created_retry);
    let created = created.expect("create");
    let id = ProviderEventId::new(created.provider_event_id()).expect("id");
    let accepted = google.set_owner_rsvp(&id, Rsvp::Accepted).expect("accept");
    let promotion =
        GoogleProposalPromotion::new(id.as_str(), "final", None, true).expect("promotion");
    let promote_gate = Arc::new(Barrier::new(3));
    let promote_one = google.clone();
    let promote_two = google.clone();
    let promote_one_gate = promote_gate.clone();
    let promote_two_gate = promote_gate.clone();
    let promote_one_session = s.clone();
    let promote_two_session = s.clone();
    let promote_one_input = promotion.clone();
    let promote_two_input = promotion.clone();
    let promoted = tokio::spawn(async move {
        promote_one_gate.wait().await;
        promote_one
            .promote_proposal(&promote_one_session, &promote_one_input)
            .await
    });
    let promoted_retry = tokio::spawn(async move {
        promote_two_gate.wait().await;
        promote_two
            .promote_proposal(&promote_two_session, &promote_two_input)
            .await
    });
    promote_gate.wait().await;
    let promoted = promoted.await.expect("promote task");
    let promoted_retry = promoted_retry.await.expect("promote retry task");
    assert_eq!(promoted, promoted_retry);
    let promoted = promoted.expect("promotion");
    let delete_gate = Arc::new(Barrier::new(3));
    let delete_one = google.clone();
    let delete_two = google.clone();
    let delete_one_gate = delete_gate.clone();
    let delete_two_gate = delete_gate.clone();
    let delete_one_session = s.clone();
    let delete_two_session = s.clone();
    let delete_one_id = id.clone();
    let delete_two_id = id.clone();
    let delete_one = tokio::spawn(async move {
        delete_one_gate.wait().await;
        delete_one
            .delete_proposal(&delete_one_session, &delete_one_id)
            .await
    });
    let delete_two = tokio::spawn(async move {
        delete_two_gate.wait().await;
        delete_two
            .delete_proposal(&delete_two_session, &delete_two_id)
            .await
    });
    delete_gate.wait().await;
    let deleted = delete_one.await.expect("delete task");
    let deleted_retry = delete_two.await.expect("delete retry task");
    assert_eq!(deleted, deleted_retry);
    assert_eq!(deleted, Ok(()));
    assert_eq!(
        google
            .sync_calendar(&s, &calendar_request(None, 8))
            .await
            .expect("proposal lifecycle history")
            .items(),
        [
            CalendarChange::upsert(created.clone()).expect("create change"),
            CalendarChange::upsert(accepted).expect("accept change"),
            CalendarChange::upsert(promoted).expect("promote change"),
            CalendarChange::deleted(id.as_str(), now()).expect("single delete tombstone"),
        ]
    );
    assert!(
        google
            .list_busy(&s, &range())
            .await
            .expect("deleted busy")
            .is_empty()
    );
    let gmail = FakeGmail::new(control.clone(), [message("gmail")]);
    let labels = LabelChanges::new(["done"], ["inbox"]).expect("labels");
    let gm: &dyn GmailProvider = &gmail;
    let gmail_id = MailMessageId::new("gmail").expect("id");
    let label_gate = Arc::new(Barrier::new(3));
    let label_one = gmail.clone();
    let label_two = gmail.clone();
    let label_one_gate = label_gate.clone();
    let label_two_gate = label_gate.clone();
    let label_one_session = s.clone();
    let label_two_session = s.clone();
    let label_one_id = gmail_id.clone();
    let label_two_id = gmail_id.clone();
    let label_one_changes = labels.clone();
    let label_two_changes = labels.clone();
    let labels_first = tokio::spawn(async move {
        label_one_gate.wait().await;
        GmailProvider::modify_labels(
            &label_one,
            &label_one_session,
            &label_one_id,
            &label_one_changes,
        )
        .await
    });
    let labels_retry = tokio::spawn(async move {
        label_two_gate.wait().await;
        GmailProvider::modify_labels(
            &label_two,
            &label_two_session,
            &label_two_id,
            &label_two_changes,
        )
        .await
    });
    label_gate.wait().await;
    let labels_first = labels_first.await.expect("label task");
    let labels_retry = labels_retry.await.expect("label retry task");
    assert_eq!(labels_first, labels_retry);
    let labelled = gm
        .sync_mail(&s, &MailSyncRequest::new(None, 3).expect("request"))
        .await
        .expect("labelled mail");
    assert_eq!(labelled.items().len(), 2);
    assert_eq!(labelled.items()[1].labels(), ["done"]);
    let outbound = OutboundMail::new(
        "send",
        MailAddress::new("to@example.test").expect("recipient"),
        "subject",
        "body",
    )
    .expect("outbound");
    let send_gate = Arc::new(Barrier::new(3));
    let send_one = gmail.clone();
    let send_two = gmail.clone();
    let send_one_gate = send_gate.clone();
    let send_two_gate = send_gate.clone();
    let send_one_session = s.clone();
    let send_two_session = s.clone();
    let send_one_mail = outbound.clone();
    let send_two_mail = outbound.clone();
    let sent = tokio::spawn(async move {
        send_one_gate.wait().await;
        GmailProvider::send_mail(&send_one, &send_one_session, &send_one_mail).await
    });
    let sent_retry = tokio::spawn(async move {
        send_two_gate.wait().await;
        GmailProvider::send_mail(&send_two, &send_two_session, &send_two_mail).await
    });
    send_gate.wait().await;
    let sent = sent.await.expect("send task");
    let sent_retry = sent_retry.await.expect("send retry task");
    assert_eq!(sent, sent_retry);
    let sent = sent.expect("send");
    assert_eq!(gmail.sent_mail_receipts(), Ok(vec![sent]));
    let input = triage_input("triage");
    let triage =
        FakeStructuredTriage::new(control.clone(), [(input.clone(), TriageDecision::Ignore)]);
    let triage_gate = Arc::new(Barrier::new(3));
    let triage_one = triage.clone();
    let triage_two = triage.clone();
    let triage_one_gate = triage_gate.clone();
    let triage_two_gate = triage_gate.clone();
    let triage_one_session = s.clone();
    let triage_two_session = s.clone();
    let triage_one_input = input.clone();
    let triage_two_input = input.clone();
    let triage_first = tokio::spawn(async move {
        triage_one_gate.wait().await;
        triage_one
            .classify(&triage_one_session, &triage_one_input)
            .await
    });
    let triage_retry = tokio::spawn(async move {
        triage_two_gate.wait().await;
        triage_two
            .classify(&triage_two_session, &triage_two_input)
            .await
    });
    triage_gate.wait().await;
    let triage_first = triage_first.await.expect("triage task");
    let triage_retry = triage_retry.await.expect("triage retry task");
    assert_eq!(triage_first, triage_retry);
    let backup = FakeEncryptedS3Backup::new(control.clone());
    let backup_gate = Arc::new(Barrier::new(3));
    let backup_one = backup.clone();
    let backup_two = backup.clone();
    let backup_one_gate = backup_gate.clone();
    let backup_two_gate = backup_gate.clone();
    let backup_one_session = s.clone();
    let backup_two_session = s.clone();
    let snapshot = snapshot();
    let backup_one_snapshot = snapshot.clone();
    let backup_two_snapshot = snapshot.clone();
    let backup_first = tokio::spawn(async move {
        backup_one_gate.wait().await;
        backup_one
            .put_snapshot(&backup_one_session, &backup_one_snapshot)
            .await
    });
    let backup_retry = tokio::spawn(async move {
        backup_two_gate.wait().await;
        backup_two
            .put_snapshot(&backup_two_session, &backup_two_snapshot)
            .await
    });
    backup_gate.wait().await;
    let backup_first = backup_first.await.expect("backup task");
    let backup_retry = backup_retry.await.expect("backup retry task");
    assert_eq!(backup_first, backup_retry);
    let backup_first = backup_first.expect("backup");
    assert_eq!(backup.stored_receipts(), Ok(vec![backup_first]));
    assert_eq!(
        control.invocation_count(FakeOperation::CalendarOwnerCreate),
        Ok(2)
    );
    assert_eq!(
        control.invocation_count(FakeOperation::CalendarProposalCreate),
        Ok(2)
    );
    assert_eq!(
        control.invocation_count(FakeOperation::CalendarPromote),
        Ok(2)
    );
    assert_eq!(
        control.invocation_count(FakeOperation::CalendarDelete),
        Ok(2)
    );
    assert_eq!(control.invocation_count(FakeOperation::MailLabels), Ok(2));
    assert_eq!(control.invocation_count(FakeOperation::MailSend), Ok(1));
    assert_eq!(
        control.invocation_count(FakeOperation::TriageClassify),
        Ok(2)
    );
    assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(2));
}

#[test]
fn every_fake_debug_and_every_trait_future_is_send_without_sentinel_leaks() {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send<T: Send>(_: T) {}
    assert_send_sync::<FakeControl>();
    assert_send_sync::<FakeCalendarRead>();
    assert_send_sync::<FakeOutlookCalendar>();
    assert_send_sync::<FakeGoogleCalendar>();
    assert_send_sync::<FakeOutlookMail>();
    assert_send_sync::<FakeGmail>();
    assert_send_sync::<FakeStructuredTriage>();
    assert_send_sync::<FakeEncryptedS3Backup>();
    let control = FakeControl::new(now());
    let s = session();
    let r = range();
    let cr = calendar_request(None, 1);
    let mr = MailSyncRequest::new(None, 1).expect("request");
    let owner = owner_draft();
    let proposal = proposal();
    let promotion =
        GoogleProposalPromotion::new("sentinel-event-id", "sentinel-title", None, false)
            .expect("promotion");
    let event_id = ProviderEventId::new("sentinel-event-id").expect("id");
    let mail_id = MailMessageId::new("sentinel-message-id").expect("id");
    let labels = LabelChanges::new(["sentinel-label"], ["inbox"]).expect("labels");
    let outbound = OutboundMail::new(
        "sentinel-operation",
        MailAddress::new("sentinel@example.test").expect("recipient"),
        "sentinel-title",
        "sentinel-body",
    )
    .expect("outbound");
    let input = triage_input("sentinel-message-id");
    let read = FakeCalendarRead::new(
        control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let outlook = FakeOutlookCalendar::new(
        control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let google = FakeGoogleCalendar::new(
        control.clone(),
        Vec::<BusyInterval>::new(),
        Vec::<CalendarChange>::new(),
    );
    let outlook_mail = FakeOutlookMail::new(control.clone(), [message("sentinel-message-id")]);
    let gmail = FakeGmail::new(control.clone(), [message("sentinel-message-id")]);
    let triage =
        FakeStructuredTriage::new(control.clone(), [(input.clone(), TriageDecision::Ignore)]);
    let backup = FakeEncryptedS3Backup::new(control.clone());
    assert_send((&read as &dyn CalendarReadProvider).list_busy(&s, &r));
    assert_send((&read as &dyn CalendarReadProvider).sync_calendar(&s, &cr));
    assert_send((&outlook as &dyn OutlookCalendarProvider).list_busy(&s, &r));
    assert_send((&outlook as &dyn OutlookCalendarProvider).sync_calendar(&s, &cr));
    assert_send((&outlook as &dyn OutlookCalendarProvider).find_owner_event(&s, &owner));
    assert_send((&outlook as &dyn OutlookCalendarProvider).create_owner_event(&s, &owner));
    assert_send((&google as &dyn GoogleCalendarProvider).list_busy(&s, &r));
    assert_send((&google as &dyn GoogleCalendarProvider).sync_calendar(&s, &cr));
    assert_send((&google as &dyn GoogleCalendarProvider).find_proposal(&s, &proposal));
    assert_send((&google as &dyn GoogleCalendarProvider).create_proposal(&s, &proposal));
    assert_send((&google as &dyn GoogleCalendarProvider).promote_proposal(&s, &promotion));
    assert_send((&google as &dyn GoogleCalendarProvider).delete_proposal(&s, &event_id));
    assert_send((&outlook_mail as &dyn IncomingMailProvider).sync_mail(&s, &mr));
    assert_send((&gmail as &dyn GmailProvider).sync_mail(&s, &mr));
    assert_send((&gmail as &dyn GmailProvider).modify_labels(&s, &mail_id, &labels));
    assert_send((&gmail as &dyn GmailProvider).send_mail(&s, &outbound));
    assert_send((&triage as &dyn StructuredTriageProvider).classify(&s, &input));
    assert_send((&backup as &dyn EncryptedS3BackupProvider).put_snapshot(&s, &snapshot()));
    let sentinels = [
        "contract-token",
        "contract@example.test",
        "sentinel@example.test",
        "contract body",
        "contract title",
        "sentinel-body",
        "sentinel-title",
        "sentinel-message-id",
        "sentinel-event-id",
        "sentinel-send-op",
        "contract-object-id",
        "contract-ciphertext",
        "a3b7c87d9b77dc909f571008227641e45b11b4e2369ebcd57e87c714fd8b5fe5",
        "contract-encryption",
        "contract-key-metadata",
        "contract-metadata",
        "fake-calendar:0",
        "fake-outlook-mail:0",
        "fake-gmail:0",
        "fake-gmail-sent-1",
        "sentinel-owner-op",
        "sentinel-google-op",
        "fake-outlook-owner-event-1",
        "fake-google-proposal-event-1",
        "fake-s3-version-1",
    ];
    for debug in [
        format!("{control:?}"),
        format!("{read:?}"),
        format!("{outlook:?}"),
        format!("{google:?}"),
        format!("{outlook_mail:?}"),
        format!("{gmail:?}"),
        format!("{triage:?}"),
        format!("{backup:?}"),
        format!("{:?}", snapshot()),
    ] {
        for sentinel in sentinels {
            assert!(
                !debug.contains(sentinel),
                "debug leaked {sentinel}: {debug}"
            );
        }
    }
}
