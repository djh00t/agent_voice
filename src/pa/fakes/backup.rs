//! Deterministic encrypted-backup provider fake.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::pa::providers::{
    BackupObjectInfo, BackupObjectKey, BackupReceipt, EncryptedS3BackupProvider, EncryptedSnapshot,
    ProviderError, ProviderFuture, ProviderResult, ProviderSession,
};

use super::control::{FakeControl, FakeOperation};

struct StoredObject {
    snapshot: EncryptedSnapshot,
    receipt: BackupReceipt,
}

struct BackupState {
    objects: BTreeMap<String, StoredObject>,
    next_provider_version: u64,
}

/// Cloneable deterministic encrypted-object backup fake.
///
/// Clones share a mutex-protected transient object map. The fake accepts only
/// validated encrypted snapshots and delegates operation accounting and fault
/// injection to its shared control plane.
#[derive(Clone)]
pub struct FakeEncryptedS3Backup {
    control: FakeControl,
    state: Arc<Mutex<BackupState>>,
}

impl FakeEncryptedS3Backup {
    /// Creates an empty fake using the supplied shared deterministic control.
    pub fn new<C>(control: C) -> Self
    where
        C: Borrow<FakeControl>,
    {
        Self {
            control: control.borrow().clone(),
            state: Arc::new(Mutex::new(BackupState {
                objects: BTreeMap::new(),
                next_provider_version: 1,
            })),
        }
    }

    /// Returns the shared fake control plane.
    pub fn control(&self) -> &FakeControl {
        &self.control
    }

    /// Returns cloned stored receipts in deterministic object-key order.
    ///
    /// This fake-only inspection reads shared state without invoking the
    /// provider control plane or exposing stored snapshot data.
    pub fn stored_receipts(&self) -> ProviderResult<Vec<BackupReceipt>> {
        let state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        Ok(state
            .objects
            .values()
            .map(|stored| stored.receipt.clone())
            .collect())
    }

    fn store(&self, snapshot: EncryptedSnapshot) -> ProviderResult<BackupReceipt> {
        let mut state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        if let Some(existing) = state.objects.get(snapshot.object_key()) {
            return if existing.snapshot == snapshot {
                Ok(existing.receipt.clone())
            } else {
                Err(ProviderError::Conflict)
            };
        }

        let sequence = state.next_provider_version;
        let next_provider_version = sequence.checked_add(1).ok_or(ProviderError::Unavailable)?;
        let receipt = BackupReceipt::new(
            snapshot.object_key(),
            format!("fake-s3-version-{sequence}"),
            snapshot.checksum(),
            self.control.now(),
            snapshot.ciphertext_size(),
        )?;
        state.next_provider_version = next_provider_version;
        state.objects.insert(
            snapshot.object_key().to_owned(),
            StoredObject {
                snapshot,
                receipt: receipt.clone(),
            },
        );
        Ok(receipt)
    }

    fn list(&self, prefix: &BackupObjectKey) -> ProviderResult<Vec<BackupObjectInfo>> {
        let state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        state
            .objects
            .iter()
            .filter(|(key, _)| key_matches_prefix(key, prefix.as_str()))
            .map(|(key, stored)| {
                BackupObjectInfo::new(
                    BackupObjectKey::new(key.clone())?,
                    stored.receipt.provider_version(),
                    stored.receipt.checksum(),
                    stored.receipt.uploaded_at(),
                    stored.receipt.stored_byte_count(),
                )
            })
            .collect()
    }

    fn load(&self, key: &BackupObjectKey) -> ProviderResult<EncryptedSnapshot> {
        let state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        state
            .objects
            .get(key.as_str())
            .map(|stored| stored.snapshot.clone())
            .ok_or(ProviderError::NotFound)
    }

    fn remove(&self, key: &BackupObjectKey) -> ProviderResult<()> {
        let mut state = self.state.lock().map_err(|_| ProviderError::Unavailable)?;
        state.objects.remove(key.as_str());
        Ok(())
    }
}

fn key_matches_prefix(key: &str, prefix: &str) -> bool {
    key == prefix
        || key
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

impl fmt::Debug for FakeEncryptedS3Backup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let object_count = match self.state.lock() {
            Ok(state) => state.objects.len(),
            Err(_) => return formatter.write_str("FakeEncryptedS3Backup { state: unavailable }"),
        };
        let mut debug = formatter.debug_struct("FakeEncryptedS3Backup");
        debug.field("object_count", &object_count);
        match self.control.invocation_count(FakeOperation::BackupPut) {
            Ok(count) => debug.field("put_call_count", &count).finish(),
            Err(_) => debug.field("put_call_count", &"<unavailable>").finish(),
        }
    }
}

impl EncryptedS3BackupProvider for FakeEncryptedS3Backup {
    fn put_snapshot<'a>(
        &'a self,
        _session: &'a ProviderSession,
        snapshot: &'a EncryptedSnapshot,
    ) -> ProviderFuture<'a, BackupReceipt> {
        let fake = self.clone();
        let snapshot = snapshot.clone();
        Box::pin(async move {
            fake.control.begin(FakeOperation::BackupPut)?;
            fake.store(snapshot)
        })
    }

    fn list_snapshots<'a>(
        &'a self,
        _session: &'a ProviderSession,
        prefix: &'a BackupObjectKey,
    ) -> ProviderFuture<'a, Vec<BackupObjectInfo>> {
        let fake = self.clone();
        let prefix = prefix.clone();
        Box::pin(async move {
            fake.control.begin(FakeOperation::BackupPut)?;
            fake.list(&prefix)
        })
    }

    fn get_snapshot<'a>(
        &'a self,
        _session: &'a ProviderSession,
        key: &'a BackupObjectKey,
    ) -> ProviderFuture<'a, EncryptedSnapshot> {
        let fake = self.clone();
        let key = key.clone();
        Box::pin(async move {
            fake.control.begin(FakeOperation::BackupPut)?;
            fake.load(&key)
        })
    }

    fn delete_snapshot<'a>(
        &'a self,
        _session: &'a ProviderSession,
        key: &'a BackupObjectKey,
    ) -> ProviderFuture<'a, ()> {
        let fake = self.clone();
        let key = key.clone();
        Box::pin(async move {
            fake.control.begin(FakeOperation::BackupPut)?;
            fake.remove(&key)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::FakeEncryptedS3Backup;
    use crate::pa::fakes::{FakeControl, FakeOperation};
    use crate::pa::providers::{
        BackupObjectKey, BackupReceipt, EncryptedS3BackupProvider, EncryptedSnapshot,
        ProviderError, ProviderSession, RetryAfter,
    };
    use chrono::{DateTime, Duration, Utc};
    use std::fmt;

    const NOW: &str = "2026-08-29T12:34:56Z";

    fn now() -> DateTime<Utc> {
        NOW.parse().expect("valid instant")
    }

    fn session() -> ProviderSession {
        ProviderSession::new("backup-account", "backup-session-token", None).expect("session")
    }

    fn snapshot(
        object_key: &str,
        ciphertext: &[u8],
        checksum: &str,
        encryption_format: &str,
        key_metadata: &str,
        encryption_metadata: &str,
    ) -> EncryptedSnapshot {
        EncryptedSnapshot::new(
            object_key,
            ciphertext.to_vec(),
            checksum,
            ciphertext.len() as u64,
            encryption_format,
            key_metadata,
            encryption_metadata,
        )
        .expect("snapshot")
    }

    fn first_snapshot() -> EncryptedSnapshot {
        snapshot(
            "backup-sentinel-object-key",
            b"backup-sentinel-ciphertext",
            "2e46e41589939dd28a664c8d40d852b3800c01cdb32e87f3d7a6422f745c42e9",
            "backup-sentinel-encryption-format",
            "backup-sentinel-key-metadata",
            "backup-sentinel-encryption-metadata",
        )
    }

    #[tokio::test]
    async fn upload_returns_a_stable_exact_retry_receipt() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let snapshot = first_snapshot();

        let receipt = fake
            .put_snapshot(&session(), &snapshot)
            .await
            .expect("first receipt");
        assert_eq!(receipt.object_key(), snapshot.object_key());
        assert_eq!(receipt.provider_version(), "fake-s3-version-1");
        assert_eq!(receipt.checksum(), snapshot.checksum());
        assert_eq!(receipt.uploaded_at(), now());
        assert_eq!(receipt.stored_byte_count(), snapshot.ciphertext_size());
        assert_eq!(fake.put_snapshot(&session(), &snapshot).await, Ok(receipt));
        assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(2));
    }

    #[tokio::test]
    async fn list_download_and_delete_are_prefix_scoped_deterministic_and_idempotent() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let provider: &dyn EncryptedS3BackupProvider = &fake;
        let values = [
            snapshot(
                "backup-sentinel/z/key",
                b"backup-sentinel-z-ciphertext",
                "089d76f2759d5f8dc10b541e00875a601890d4fc89dae748261c8f1d64f2a492",
                "backup-sentinel-encryption-format",
                "backup-sentinel-key-metadata",
                "backup-sentinel-encryption-metadata",
            ),
            snapshot(
                "backup-sentinel/a/key",
                b"backup-sentinel-a-ciphertext",
                "92762cbb819f1bceef362f2400603f484cee20321497c97f504c8838fc72e5c2",
                "backup-sentinel-encryption-format",
                "backup-sentinel-key-metadata",
                "backup-sentinel-encryption-metadata",
            ),
            snapshot(
                "outside/key",
                b"backup-sentinel-outside-ciphertext",
                "20bae8d48a542dd08810d2d96bbb60ad2c0c552d549c845f629f1494770c391b",
                "backup-sentinel-encryption-format",
                "backup-sentinel-key-metadata",
                "backup-sentinel-encryption-metadata",
            ),
        ];
        for value in &values {
            provider
                .put_snapshot(&session(), value)
                .await
                .expect("receipt");
        }

        let prefix = BackupObjectKey::new("backup-sentinel").expect("prefix");
        let listed = provider
            .list_snapshots(&session(), &prefix)
            .await
            .expect("metadata");
        assert_eq!(
            listed
                .iter()
                .map(|info| info.object_key())
                .collect::<Vec<_>>(),
            vec!["backup-sentinel/a/key", "backup-sentinel/z/key"]
        );
        assert_eq!(listed[0].checksum(), values[1].checksum());
        assert_eq!(listed[0].byte_count(), values[1].ciphertext_size());
        assert!(!format!("{:?}", listed[0]).contains("backup-sentinel-a"));

        let key = BackupObjectKey::new(values[1].object_key()).expect("object key");
        assert_eq!(
            provider.get_snapshot(&session(), &key).await,
            Ok(values[1].clone())
        );
        assert_eq!(
            provider
                .get_snapshot(
                    &session(),
                    &BackupObjectKey::new("backup-sentinel-missing").expect("key")
                )
                .await,
            Err(ProviderError::NotFound)
        );
        assert_eq!(provider.delete_snapshot(&session(), &key).await, Ok(()));
        assert_eq!(provider.delete_snapshot(&session(), &key).await, Ok(()));
        assert_eq!(
            fake.stored_receipts()
                .expect("remaining receipts")
                .iter()
                .map(BackupReceipt::object_key)
                .collect::<Vec<_>>(),
            vec!["backup-sentinel/z/key", "outside/key"]
        );
        assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(8));
    }

    #[tokio::test]
    async fn list_download_and_delete_fail_before_mutation_when_control_fails() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let snapshot = first_snapshot();
        fake.put_snapshot(&session(), &snapshot)
            .await
            .expect("receipt");
        let key = BackupObjectKey::new(snapshot.object_key()).expect("key");
        let prefix = BackupObjectKey::new("backup-sentinel").expect("prefix");

        control
            .queue_failure(FakeOperation::BackupPut, ProviderError::Unavailable)
            .expect("list failure");
        assert_eq!(
            fake.list_snapshots(&session(), &prefix).await,
            Err(ProviderError::Unavailable)
        );
        control
            .queue_failure(FakeOperation::BackupPut, ProviderError::Unavailable)
            .expect("get failure");
        assert_eq!(
            fake.get_snapshot(&session(), &key).await,
            Err(ProviderError::Unavailable)
        );
        control
            .queue_failure(FakeOperation::BackupPut, ProviderError::Unavailable)
            .expect("delete failure");
        assert_eq!(
            fake.delete_snapshot(&session(), &key).await,
            Err(ProviderError::Unavailable)
        );
        assert_eq!(fake.get_snapshot(&session(), &key).await, Ok(snapshot));
    }

    #[test]
    fn stored_receipts_is_empty_without_provider_calls() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());

        assert_eq!(fake.stored_receipts(), Ok(Vec::<BackupReceipt>::new()));
        assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(0));
    }

    #[tokio::test]
    async fn stored_receipts_is_unchanged_after_upload_failure() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        control
            .set_failure(FakeOperation::BackupPut, ProviderError::Unavailable)
            .expect("persistent failure");

        assert_eq!(
            fake.put_snapshot(&session(), &first_snapshot()).await,
            Err(ProviderError::Unavailable)
        );
        assert_eq!(fake.stored_receipts(), Ok(Vec::<BackupReceipt>::new()));
        assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(1));
    }

    #[tokio::test]
    async fn stored_receipts_has_one_receipt_after_exact_retry() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let snapshot = first_snapshot();

        let receipt = fake
            .put_snapshot(&session(), &snapshot)
            .await
            .expect("first receipt");
        assert_eq!(fake.stored_receipts(), Ok(vec![receipt.clone()]));
        assert_eq!(
            fake.put_snapshot(&session(), &snapshot).await,
            Ok(receipt.clone())
        );
        assert_eq!(fake.stored_receipts(), Ok(vec![receipt]));
        assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(2));
    }

    #[tokio::test]
    async fn cloned_fakes_observe_the_same_stored_receipts() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control);
        let clone = fake.clone();
        let snapshot = first_snapshot();

        let receipt = fake
            .put_snapshot(&session(), &snapshot)
            .await
            .expect("receipt");
        assert_eq!(clone.stored_receipts(), Ok(vec![receipt.clone()]));
        assert_eq!(
            clone.put_snapshot(&session(), &snapshot).await,
            Ok(receipt.clone())
        );
        assert_eq!(fake.stored_receipts(), Ok(vec![receipt]));
    }

    #[tokio::test]
    async fn stored_receipts_are_sorted_by_object_key() {
        let fake = FakeEncryptedS3Backup::new(FakeControl::new(now()));
        let snapshots = [
            snapshot(
                "backup-sentinel-z-key",
                b"backup-sentinel-z-ciphertext",
                "089d76f2759d5f8dc10b541e00875a601890d4fc89dae748261c8f1d64f2a492",
                "backup-sentinel-encryption-format",
                "backup-sentinel-key-metadata",
                "backup-sentinel-encryption-metadata",
            ),
            snapshot(
                "backup-sentinel-a-key",
                b"backup-sentinel-a-ciphertext",
                "92762cbb819f1bceef362f2400603f484cee20321497c97f504c8838fc72e5c2",
                "backup-sentinel-encryption-format",
                "backup-sentinel-key-metadata",
                "backup-sentinel-encryption-metadata",
            ),
            snapshot(
                "backup-sentinel-m-key",
                b"backup-sentinel-m-ciphertext",
                "a52c71b8fb5b070fd320e7397716b3b6984e0cab9884476744d9e616841ce475",
                "backup-sentinel-encryption-format",
                "backup-sentinel-key-metadata",
                "backup-sentinel-encryption-metadata",
            ),
        ];

        for value in snapshots {
            fake.put_snapshot(&session(), &value)
                .await
                .expect("receipt");
        }

        let receipts = fake.stored_receipts().expect("stored receipts");
        assert_eq!(
            receipts
                .iter()
                .map(BackupReceipt::object_key)
                .collect::<Vec<_>>(),
            vec![
                "backup-sentinel-a-key",
                "backup-sentinel-m-key",
                "backup-sentinel-z-key",
            ]
        );
    }

    #[test]
    fn stored_receipts_fails_closed_when_state_is_poisoned() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let invocation_count_before = control
            .invocation_count(FakeOperation::BackupPut)
            .expect("invocation count");
        let state = fake.state.clone();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.lock().expect("state is not already poisoned");
            panic!("intentional test poison");
        }));
        assert!(poisoned.is_err());

        assert_eq!(fake.stored_receipts(), Err(ProviderError::Unavailable));
        assert_eq!(
            control.invocation_count(FakeOperation::BackupPut),
            Ok(invocation_count_before)
        );
    }

    #[tokio::test]
    async fn changed_immutable_snapshot_values_conflict_without_mutation() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let first = first_snapshot();
        let receipt = fake
            .put_snapshot(&session(), &first)
            .await
            .expect("first receipt");

        for changed in [
            snapshot(
                first.object_key(),
                b"backup-sentinel-changed-ct",
                "982778ff9a64f092682d0984705397b9b2c8d2bfccc1f21c42f9ea3ff0321ed6",
                first.encryption_format(),
                first.key_metadata(),
                first.encryption_metadata(),
            ),
            snapshot(
                first.object_key(),
                b"backup-sentinel-checksum-change",
                "8284e5182240e764e6d4d63b8aaea66bf77e68b8e2b4392d5ced51ef06c34df5",
                first.encryption_format(),
                first.key_metadata(),
                first.encryption_metadata(),
            ),
            snapshot(
                first.object_key(),
                b"backup-sentinel-size-change",
                "23c37cb167ad81ffd2eeff823155b6724d8891c4600c02e98d1b04eb4b00aa7a",
                first.encryption_format(),
                first.key_metadata(),
                first.encryption_metadata(),
            ),
            snapshot(
                first.object_key(),
                first.ciphertext(),
                first.checksum(),
                "backup-sentinel-other-encryption-format",
                first.key_metadata(),
                first.encryption_metadata(),
            ),
            snapshot(
                first.object_key(),
                first.ciphertext(),
                first.checksum(),
                first.encryption_format(),
                "backup-sentinel-other-key-metadata",
                first.encryption_metadata(),
            ),
            snapshot(
                first.object_key(),
                first.ciphertext(),
                first.checksum(),
                first.encryption_format(),
                first.key_metadata(),
                "backup-sentinel-other-encryption-metadata",
            ),
        ] {
            assert_eq!(
                fake.put_snapshot(&session(), &changed).await,
                Err(ProviderError::Conflict)
            );
        }

        assert_eq!(fake.put_snapshot(&session(), &first).await, Ok(receipt));
        assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(8));
    }

    #[tokio::test]
    async fn failures_record_nothing_and_do_not_consume_provider_versions() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let first = first_snapshot();
        let second = snapshot(
            "backup-sentinel-second-key",
            b"backup-sentinel-second-ciphertext",
            "e9ae061c75c5e9341b78da76ba3eba59e525609feaa04dd2ec06022fce38c66c",
            "backup-sentinel-encryption-format",
            "backup-sentinel-key-metadata",
            "backup-sentinel-encryption-metadata",
        );

        for failure in [
            ProviderError::TokenExpired,
            ProviderError::throttled(RetryAfter::new(Duration::seconds(1)).expect("retry")),
        ] {
            control
                .queue_failure(FakeOperation::BackupPut, failure)
                .expect("queued failure");
            assert_eq!(fake.put_snapshot(&session(), &first).await, Err(failure));
        }
        control
            .set_failure(FakeOperation::BackupPut, ProviderError::Unavailable)
            .expect("persistent failure");
        assert_eq!(
            fake.put_snapshot(&session(), &first).await,
            Err(ProviderError::Unavailable)
        );
        control
            .clear_failure(FakeOperation::BackupPut)
            .expect("clear failure");

        let first_receipt = fake
            .put_snapshot(&session(), &first)
            .await
            .expect("recovered first receipt");
        let second_receipt = fake
            .put_snapshot(&session(), &second)
            .await
            .expect("second receipt");
        assert_eq!(first_receipt.provider_version(), "fake-s3-version-1");
        assert_eq!(second_receipt.provider_version(), "fake-s3-version-2");
        assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(5));
    }

    #[tokio::test]
    async fn cloned_concurrent_exact_uploads_share_one_object_and_receipt() {
        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let clone = fake.clone();
        let snapshot = first_snapshot();
        let session = session();

        let (left, right) = tokio::join!(
            fake.put_snapshot(&session, &snapshot),
            clone.put_snapshot(&session, &snapshot),
        );
        assert_eq!(left, right);
        assert_eq!(
            left.expect("receipt").provider_version(),
            "fake-s3-version-1"
        );
        assert_eq!(control.invocation_count(FakeOperation::BackupPut), Ok(2));
    }

    #[tokio::test]
    async fn concurrent_debug_and_upload_complete() {
        let fake = FakeEncryptedS3Backup::new(FakeControl::new(now()));
        let debug_fake = fake.clone();
        let snapshot = first_snapshot();
        let session = session();

        let (debug, receipt) = tokio::join!(
            async move { format!("{debug_fake:?}") },
            fake.put_snapshot(&session, &snapshot),
        );
        assert!(debug.contains("FakeEncryptedS3Backup"));
        assert!(receipt.is_ok());
    }

    #[tokio::test]
    async fn poisoned_control_fails_closed_and_replacement_recovers() {
        struct PanicWriter;

        impl fmt::Write for PanicWriter {
            fn write_str(&mut self, _value: &str) -> fmt::Result {
                panic!("intentional formatter panic for mutex poisoning");
            }
        }

        let control = FakeControl::new(now());
        let fake = FakeEncryptedS3Backup::new(control.clone());
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut writer = PanicWriter;
            let _ = fmt::write(&mut writer, format_args!("{control:?}"));
        }));
        assert!(poisoned.is_err());
        assert_eq!(
            fake.put_snapshot(&session(), &first_snapshot()).await,
            Err(ProviderError::Unavailable)
        );

        let replacement = FakeEncryptedS3Backup::new(FakeControl::new(now()));
        assert_eq!(
            replacement
                .put_snapshot(&session(), &first_snapshot())
                .await
                .expect("replacement receipt")
                .provider_version(),
            "fake-s3-version-1"
        );
    }

    #[tokio::test]
    async fn trait_object_round_trip_preserves_receipt_integrity_and_future_is_send() {
        fn assert_send<T: Send>(_: T) {}
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<FakeEncryptedS3Backup>();
        let fake = FakeEncryptedS3Backup::new(FakeControl::new(now()));
        let provider: &dyn EncryptedS3BackupProvider = &fake;
        assert_send(provider.put_snapshot(&session(), &first_snapshot()));
        let receipt: BackupReceipt = provider
            .put_snapshot(&session(), &first_snapshot())
            .await
            .expect("receipt");
        assert_eq!(receipt.object_key(), "backup-sentinel-object-key");
        assert_eq!(receipt.checksum(), first_snapshot().checksum());
        assert_eq!(receipt.uploaded_at(), now());
        assert_eq!(receipt.stored_byte_count(), 26);
    }

    #[tokio::test]
    async fn debug_exposes_counts_only_and_source_has_no_unencrypted_input_or_state() {
        let fake = FakeEncryptedS3Backup::new(FakeControl::new(now()));
        fake.put_snapshot(&session(), &first_snapshot())
            .await
            .expect("receipt");
        let debug = format!("{fake:?}");
        let receipts_debug = format!("{:?}", fake.stored_receipts().expect("receipts"));
        assert!(debug.contains("object_count: 1"));
        assert!(debug.contains("put_call_count: 1"));
        assert!(receipts_debug.contains("BackupReceipt"));
        for sentinel in [
            "backup-sentinel-object-key",
            "backup-sentinel-ciphertext",
            "2e46e41589939dd28a664c8d40d852b3800c01cdb32e87f3d7a6422f745c42e9",
            "backup-sentinel-encryption-format",
            "backup-sentinel-key-metadata",
            "backup-sentinel-encryption-metadata",
            "fake-s3-version-1",
            "backup-session-token",
        ] {
            assert!(!debug.contains(sentinel));
            assert!(!receipts_debug.contains(sentinel));
        }
        assert!(!include_str!("backup.rs").contains(&["plain", "text"].concat()));
    }
}
