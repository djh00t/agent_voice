#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use super::PaAdminStore;
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
