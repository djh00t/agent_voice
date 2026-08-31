#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use super::{AdminConnectionStatus, PaAdminStore};
use crate::pa::admin_config::AdminConfigPatch;
use crate::pa::store::{PaStore, StoreError};

const DATABASE_KEY: &[u8] = b"fixed-admin-store-test-key";

fn in_memory_store() -> Arc<PaStore> {
    Arc::new(PaStore::open_in_memory(DATABASE_KEY).expect("open in-memory store"))
}

#[test]
fn missing_admin_read_models() {
    let admin = PaAdminStore::new(in_memory_store());
    admin.read_snapshot().expect("read admin snapshot");
}

#[test]
fn admin_snapshot_json_excludes_backup() {
    let snapshot = PaAdminStore::new(in_memory_store())
        .read_snapshot_at(time::macros::datetime!(2026-08-31 00:00 UTC))
        .expect("read snapshot");
    let value = serde_json::to_value(snapshot).expect("serialize snapshot");
    let object = value.as_object().expect("snapshot object");
    assert_eq!(object.len(), 5);
    assert!(!object.contains_key("backup"));
}

#[test]
fn projection_order_bounds_and_redaction() {
    let store = in_memory_store();
    store.connection().execute(
        "INSERT INTO oauth_credentials(provider, account_id, access_token_ciphertext, scopes) VALUES ('google', 'sentinel-account', X'00', 'sentinel-scope')",
        [],
    ).expect("seed credential");
    let snapshot = PaAdminStore::new(store)
        .read_snapshot_at(time::macros::datetime!(2026-08-31 00:00 UTC))
        .expect("read snapshot");
    assert_eq!(
        snapshot.connections[0].status,
        AdminConnectionStatus::Connected
    );
    let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
    assert!(!json.contains("sentinel-account"));
    assert!(!json.contains("sentinel-scope"));
}

#[test]
fn owner_task_proposals_fail_closed_in_appointment_projection() {
    let store = in_memory_store();
    store
        .connection()
        .execute(
            "INSERT INTO owner_task_drafts(idempotency_key, title, kind, duration_minutes) VALUES ('owner-task-draft', 'secret task', 'callback', 15)",
            [],
        )
        .expect("seed owner task draft");
    store
        .connection()
        .execute(
            "INSERT INTO proposals(idempotency_key, source_id, owner_task_draft_id) VALUES ('owner-task-proposal', 'owner-task-source', 1)",
            [],
        )
        .expect("seed owner task proposal");

    assert!(matches!(
        PaAdminStore::new(store).read_snapshot_at(time::macros::datetime!(2026-08-31 00:00 UTC)),
        Err(StoreError::StoredRecordInvalid {
            resource: "admin projection"
        })
    ));
}

#[test]
fn failure_projection_filters_before_the_bound() {
    let store = in_memory_store();
    for id in 0..100 {
        store
            .connection()
            .execute(
                "INSERT INTO audit_events(idempotency_key, event_type, entity_type, entity_id, occurred_at) VALUES (?1, 'message_recorded', 'message', ?1, ?2)",
                [
                    format!("non-failure-{id}"),
                    format!("2026-08-30T{:02}:{:02}:00Z", id / 60, id % 60),
                ],
            )
            .expect("seed non-failure audit event");
    }
    store
        .connection()
        .execute(
            "INSERT INTO audit_events(idempotency_key, event_type, entity_type, entity_id, occurred_at) VALUES ('failure-event', 'notification_retry_scheduled', 'notification', 'notification-1', '2026-08-31T00:00:00Z')",
            [],
        )
        .expect("seed failure audit event");

    let snapshot = PaAdminStore::new(store)
        .read_snapshot_at(time::macros::datetime!(2026-08-31 00:00 UTC))
        .expect("read failure projection");

    assert_eq!(snapshot.failures.len(), 1);
    assert!(snapshot.failures[0].retryable);
}

#[test]
fn connection_bound_keeps_real_supported_provider() {
    let store = in_memory_store();
    for id in 0..100 {
        store
            .connection()
            .execute(
                "INSERT INTO oauth_credentials(provider, account_id, access_token_ciphertext, scopes) VALUES ('google', ?1, X'00', 'scope')",
                [format!("google-account-{id:03}")],
            )
            .expect("seed google credential");
    }
    store
        .connection()
        .execute(
            "INSERT INTO oauth_credentials(provider, account_id, access_token_ciphertext, scopes) VALUES ('outlook', 'outlook-account', X'00', 'scope')",
            [],
        )
        .expect("seed outlook credential");

    let snapshot = PaAdminStore::new(store)
        .read_snapshot_at(time::macros::datetime!(2026-08-31 00:00 UTC))
        .expect("read connection projection");

    assert_eq!(snapshot.connections.len(), 100);
    assert!(snapshot.connections.iter().any(|connection| {
        connection.status == AdminConnectionStatus::Connected
            && connection.provider == super::AdminProvider::Outlook
    }));
}

#[test]
fn unknown_non_failure_audit_event_fails_closed() {
    let store = in_memory_store();
    store
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = ON;")
        .expect("relax audit event fixture constraint");
    store
        .connection()
        .execute(
            "INSERT INTO audit_events(idempotency_key, event_type, entity_type, entity_id, occurred_at) VALUES ('unknown-audit-event', 'unknown', 'message', 'message-1', '2026-08-31T00:00:00Z')",
            [],
        )
        .expect("seed unknown audit event");
    store
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = OFF;")
        .expect("restore audit event constraint");

    assert!(matches!(
        PaAdminStore::new(store).read_snapshot_at(time::macros::datetime!(2026-08-31 00:00 UTC)),
        Err(StoreError::StoredRecordInvalid {
            resource: "admin projection"
        })
    ));
}

#[test]
fn failure_projection_orders_by_ascending_audit_id() {
    let store = in_memory_store();
    for (idempotency_key, occurred_at) in [
        ("first-failure", "2026-08-30T00:00:00Z"),
        ("second-failure", "2026-08-31T00:00:00Z"),
    ] {
        store
            .connection()
            .execute(
                "INSERT INTO audit_events(idempotency_key, event_type, entity_type, entity_id, occurred_at) VALUES (?1, 'notification_retry_scheduled', 'notification', ?1, ?2)",
                [idempotency_key, occurred_at],
            )
            .expect("seed failure audit event");
    }

    let snapshot = PaAdminStore::new(store)
        .read_snapshot_at(time::macros::datetime!(2026-08-31 00:00 UTC))
        .expect("read failure projection");

    assert_eq!(
        snapshot
            .failures
            .iter()
            .map(|failure| failure.id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn corrupt_row_fails_closed() {
    let store = in_memory_store();
    store
        .connection()
        .execute(
            "INSERT INTO oauth_credentials(provider, account_id) VALUES ('unsupported', 'secret')",
            [],
        )
        .expect("seed corrupt credential");
    assert!(
        PaAdminStore::new(store)
            .read_snapshot_at(time::macros::datetime!(2026-08-31 00:00 UTC))
            .is_err()
    );
}

#[test]
fn config_cas_handoff_uses_one_seam() {
    let admin = PaAdminStore::new(in_memory_store());
    let patch = AdminConfigPatch {
        model: Some("gpt-5.6-luna".into()),
        ..AdminConfigPatch::default()
    };
    let updated = admin
        .update_config(1, patch)
        .expect("delegate config update");
    assert_eq!(updated.version, 2);
}
