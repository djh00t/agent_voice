#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{AdminConfigPatch, AdminConfigStore, TaskDurationsPatch, WorkingDay};
use crate::pa::store::{PaStore, StoreError};
use serde_json::json;

const DATABASE_KEY: &[u8] = b"fixed-admin-config-test-key";

fn in_memory_store() -> Arc<PaStore> {
    Arc::new(PaStore::open_in_memory(DATABASE_KEY).expect("open in-memory store"))
}

fn temporary_database_path(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("agent-voice-admin-config-{label}-{nonce}.db"))
}

fn model_patch(model: &str) -> AdminConfigPatch {
    AdminConfigPatch {
        model: Some(model.to_owned()),
        ..AdminConfigPatch::default()
    }
}

#[test]
fn missing_admin_config_cas() {
    let admin = AdminConfigStore::new(in_memory_store());
    assert_eq!(admin.read().expect("read default config").version, 1);
}

#[test]
fn admin_config_exact_json_and_validation() {
    let store = in_memory_store();
    let admin = AdminConfigStore::new(Arc::clone(&store));
    let config = admin.read().expect("read default config");

    let expected = json!({
        "version": 1,
        "owner_timezone": null,
        "working_days": ["monday", "tuesday", "wednesday", "thursday", "friday"],
        "working_window_start": "08:00",
        "working_window_end": "18:00",
        "minimum_notice_minutes": 60,
        "booking_horizon_days": 60,
        "meeting_buffer_minutes": 0,
        "retention_days": 90,
        "task_duration_minutes": {
            "bill": 15,
            "callback": 15,
            "reading": 30,
            "email_reply": 30,
            "preparation": 60
        },
        "model": "gpt-5.6-luna",
        "updated_at": config.updated_at,
    });
    assert_eq!(
        serde_json::to_value(&config).expect("serialize config"),
        expected
    );
    let serialized = serde_json::to_string(&config).expect("serialize config");
    assert_eq!(
        serialized,
        format!(
            "{{\"version\":1,\"owner_timezone\":null,\"working_days\":[\"monday\",\"tuesday\",\"wednesday\",\"thursday\",\"friday\"],\"working_window_start\":\"08:00\",\"working_window_end\":\"18:00\",\"minimum_notice_minutes\":60,\"booking_horizon_days\":60,\"meeting_buffer_minutes\":0,\"retention_days\":90,\"task_duration_minutes\":{{\"bill\":15,\"callback\":15,\"reading\":30,\"email_reply\":30,\"preparation\":60}},\"model\":\"gpt-5.6-luna\",\"updated_at\":\"{}\"}}",
            config.updated_at
        )
    );
    assert!(!serialized.contains("owner_email"));
    assert!(!serialized.contains("owner_phone"));
    assert!(config.updated_at.ends_with('Z'));
    assert_eq!(config.working_days.len(), 5);

    let valid_patch = AdminConfigPatch {
        owner_timezone: Some("Australia/Sydney".to_owned()),
        working_days: Some(vec![WorkingDay::Monday, WorkingDay::Wednesday]),
        working_window_start: Some("09:15".to_owned()),
        working_window_end: Some("17:45".to_owned()),
        minimum_notice_minutes: Some(0),
        booking_horizon_days: Some(1),
        meeting_buffer_minutes: Some(15),
        retention_days: Some(1),
        task_duration_minutes: Some(TaskDurationsPatch {
            bill: Some(1),
            callback: Some(2),
            reading: Some(3),
            email_reply: Some(4),
            preparation: Some(5),
        }),
        model: Some("gpt-test".to_owned()),
    };
    let updated = admin
        .update_config(config.version, valid_patch)
        .expect("valid patch");
    assert_eq!(updated.version, 2);
    assert_eq!(updated.owner_timezone.as_deref(), Some("Australia/Sydney"));

    for value in [
        json!({}),
        json!({"owner_timezone": null}),
        json!({"owner_email": "sentinel-owner@example.com"}),
        json!({"model": null}),
        json!({"task_duration_minutes": {"unknown": 1}}),
    ] {
        let error = serde_json::from_value::<AdminConfigPatch>(value)
            .expect_err("invalid patch should be rejected");
        assert!(!error.to_string().contains("sentinel-owner@example.com"));
    }
    let duplicate =
        serde_json::from_str::<AdminConfigPatch>(r#"{"model":"first","model":"second"}"#)
            .expect_err("duplicate patch field should be rejected");
    assert!(!duplicate.to_string().contains("second"));

    let invalid_working_day =
        serde_json::from_str::<AdminConfigPatch>(r#"{"working_days":["sentinel-working-day"]}"#)
            .expect_err("unknown working day should be rejected");
    assert!(
        invalid_working_day
            .to_string()
            .starts_with("working_days contains an unsupported value")
    );
    assert!(
        !invalid_working_day
            .to_string()
            .contains("sentinel-working-day")
    );
    assert!(!format!("{invalid_working_day:?}").contains("sentinel-working-day"));

    for patch in [
        AdminConfigPatch {
            working_days: Some(vec![WorkingDay::Monday, WorkingDay::Monday]),
            ..AdminConfigPatch::default()
        },
        AdminConfigPatch {
            working_window_start: Some("8:00".to_owned()),
            ..AdminConfigPatch::default()
        },
        AdminConfigPatch {
            minimum_notice_minutes: Some(-1),
            ..AdminConfigPatch::default()
        },
        AdminConfigPatch {
            booking_horizon_days: Some(0),
            ..AdminConfigPatch::default()
        },
        AdminConfigPatch {
            retention_days: Some(0),
            ..AdminConfigPatch::default()
        },
        AdminConfigPatch {
            task_duration_minutes: Some(TaskDurationsPatch {
                preparation: Some(0),
                ..TaskDurationsPatch::default()
            }),
            ..AdminConfigPatch::default()
        },
        AdminConfigPatch {
            owner_timezone: Some("not/a-timezone".to_owned()),
            ..AdminConfigPatch::default()
        },
    ] {
        let error = admin
            .update_config(updated.version, patch)
            .expect_err("invalid patch should fail before update");
        assert!(!format!("{error:?}").contains("not/a-timezone"));
    }
}

#[test]
fn configuration_cas_commits_once() {
    let store = in_memory_store();
    let admin = AdminConfigStore::new(Arc::clone(&store));
    let before = admin.read().expect("read default config");

    let after = admin
        .update_config(
            before.version,
            AdminConfigPatch {
                meeting_buffer_minutes: Some(15),
                model: Some("gpt-test".to_owned()),
                ..AdminConfigPatch::default()
            },
        )
        .expect("commit config patch");

    assert_eq!(after.version, before.version + 1);
    assert_eq!(after.meeting_buffer_minutes, 15);
    assert_eq!(after.model, "gpt-test");
    assert!(after.updated_at.ends_with('Z'));
    let persisted_version: i64 = store
        .connection()
        .query_row(
            "SELECT version FROM configuration WHERE id = ?1",
            [1_i64],
            |row| row.get(0),
        )
        .expect("read committed version");
    assert_eq!(persisted_version, 2);
}

#[test]
fn stale_version_is_atomic_and_redacted() {
    let store = in_memory_store();
    let admin = AdminConfigStore::new(Arc::clone(&store));
    let before = admin.read().expect("read default config");
    let patch = model_patch("sentinel-model-value");
    assert!(!format!("{patch:?}").contains("sentinel-model-value"));
    let error = admin
        .update_config(before.version - 1, patch)
        .expect_err("stale revision should conflict");

    assert!(matches!(
        error,
        StoreError::CursorConflict {
            resource: "configuration"
        }
    ));
    assert_eq!(error.to_string(), "configuration update conflicted");
    assert!(!format!("{error:?}").contains("sentinel-model-value"));
    assert_eq!(admin.read().expect("read unchanged config"), before);
}

#[test]
fn rollback_on_constraint_failure() {
    let store = in_memory_store();
    let admin = AdminConfigStore::new(Arc::clone(&store));
    let before = admin.read().expect("read default config");
    store
        .connection()
        .execute_batch(
            "CREATE TRIGGER admin_config_test_abort
             BEFORE UPDATE OF email_triage_model ON configuration
             BEGIN SELECT RAISE(ABORT, 'sentinel SQL diagnostic'); END;",
        )
        .expect("install test abort trigger");

    let error = admin
        .update_config(before.version, model_patch("sentinel-model-value"))
        .expect_err("constraint failure should roll back");
    assert!(!format!("{error}").contains("sentinel"));
    assert!(!format!("{error:?}").contains("sentinel"));
    assert_eq!(admin.read().expect("read rolled-back config"), before);
    store
        .connection()
        .execute_batch("DROP TRIGGER admin_config_test_abort")
        .expect("remove test abort trigger");
}

#[test]
fn configuration_cas_survives_restart() {
    let path = temporary_database_path("restart");
    let committed = {
        let store = Arc::new(PaStore::open(&path, DATABASE_KEY).expect("open file store"));
        let admin = AdminConfigStore::new(store);
        admin
            .update_config(1, model_patch("gpt-restart-test"))
            .expect("commit file-backed patch")
    };
    assert_eq!(committed.version, 2);

    let reopened = {
        let store = Arc::new(PaStore::open(&path, DATABASE_KEY).expect("reopen file store"));
        let admin = AdminConfigStore::new(store);
        admin.read().expect("read persisted config")
    };
    assert_eq!(reopened.version, 2);
    assert_eq!(reopened.model, "gpt-restart-test");
    let _ = std::fs::remove_file(path);
}
