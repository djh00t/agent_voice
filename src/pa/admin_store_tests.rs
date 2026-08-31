#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use super::{AdminConnectionStatus, PaAdminStore};
use crate::pa::admin_config::AdminConfigPatch;
use crate::pa::store::PaStore;

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
