use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, backup::Backup};

use crate::pa::store::{PaStore, StoreError, StoreResult};

impl PaStore {
    /// Copies the live SQLCipher database to a new attempt-local path.
    ///
    /// The caller-supplied key is applied to the destination before the
    /// backup starts. The destination is checked after the complete backup
    /// for SQLCipher support, integrity, and the current PA schema.
    pub fn backup_to_path<P, K>(&self, destination: P, database_key: K) -> StoreResult<()>
    where
        P: AsRef<Path>,
        K: AsRef<[u8]>,
    {
        let key = database_key.as_ref();
        reject_empty_database_key(key)?;

        let expected_schema_version = schema_version(self.connection())?;
        let mut destination_guard = DestinationGuard::create(destination.as_ref())?;
        let mut destination_connection =
            Connection::open(destination_guard.path()).map_err(|_| backup_error())?;

        apply_database_key(&destination_connection, key)?;
        verify_destination_cipher(&destination_connection)?;

        {
            let backup = Backup::new(self.connection(), &mut destination_connection)
                .map_err(|_| backup_error())?;
            backup
                .run_to_completion(5, Duration::from_millis(50), None)
                .map_err(|_| backup_error())?;
        }

        validate_destination(&destination_connection, expected_schema_version)?;
        destination_guard.persist();
        Ok(())
    }
}

fn reject_empty_database_key(key: &[u8]) -> StoreResult<()> {
    if key.is_empty() {
        return Err(StoreError::EmptyDatabaseKey);
    }
    Ok(())
}

fn apply_database_key(connection: &Connection, key: &[u8]) -> StoreResult<()> {
    let encoded_key = hex_encode(key);
    connection
        .pragma_update(None, "key", format!("x'{encoded_key}'"))
        .map_err(|_| backup_error())?;
    Ok(())
}

fn verify_destination_cipher(connection: &Connection) -> StoreResult<()> {
    let cipher_version = connection
        .pragma_query_value(None, "cipher_version", |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(|_| backup_error())?;
    if cipher_version
        .as_deref()
        .is_none_or(|version| version.trim().is_empty())
    {
        return Err(StoreError::SqlCipherUnavailable);
    }
    Ok(())
}

fn schema_version(connection: &Connection) -> StoreResult<i64> {
    let (minimum, maximum, count): (Option<i64>, Option<i64>, i64) = connection
        .query_row(
            "SELECT MIN(version), MAX(version), count(*) FROM schema_migrations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| backup_error())?;
    let Some(maximum) = maximum else {
        return Err(backup_error());
    };
    if minimum != Some(1) || maximum <= 0 || count != maximum {
        return Err(backup_error());
    }
    Ok(maximum)
}

fn validate_destination(connection: &Connection, expected_schema_version: i64) -> StoreResult<()> {
    let quick_check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| backup_error())?;
    if !quick_check.eq_ignore_ascii_case("ok") {
        return Err(backup_error());
    }

    if schema_version(connection)? != expected_schema_version {
        return Err(backup_error());
    }

    let required_tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master
             WHERE type = 'table' AND name IN (
                 'schema_migrations', 'configuration', 'oauth_credentials',
                 'provider_cursors', 'appointment_drafts', 'owner_task_drafts',
                 'messages', 'tasks', 'proposals', 'event_mappings',
                 'notification_outbox', 'replay_nonces', 'audit_events'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|_| backup_error())?;
    if required_tables != 13 {
        return Err(backup_error());
    }

    let configuration_rows: i64 = connection
        .query_row("SELECT count(*) FROM configuration", [], |row| row.get(0))
        .map_err(|_| backup_error())?;
    if configuration_rows != 1 {
        return Err(backup_error());
    }
    Ok(())
}

fn backup_error() -> StoreError {
    StoreError::StoredRecordInvalid { resource: "backup" }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

struct DestinationGuard {
    path: PathBuf,
    persisted: bool,
}

impl DestinationGuard {
    fn create(path: &Path) -> StoreResult<Self> {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => {
                drop(file);
                Ok(Self {
                    path: path.to_owned(),
                    persisted: false,
                })
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(StoreError::Conflict {
                resource: "backup destination",
            }),
            Err(_) => Err(backup_error()),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn persist(&mut self) {
        self.persisted = true;
    }
}

impl Drop for DestinationGuard {
    fn drop(&mut self) {
        if !self.persisted {
            remove_database_files(&self.path);
        }
    }
}

fn remove_database_files(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(sidecar_path(path, "-wal"));
    let _ = fs::remove_file(sidecar_path(path, "-shm"));
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use crate::pa::store::{MessageProvider, MessageSummary, PaStore, StoreError};

    use super::apply_database_key;

    const DATABASE_KEY: &[u8] = b"source-boundary-test-key";
    const WRONG_DATABASE_KEY: &[u8] = b"source-boundary-wrong-key";
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDestination {
        path: PathBuf,
    }

    impl TempDestination {
        fn new(label: &str) -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "agent_voice_source_backup_{}_{}_{}.db",
                std::process::id(),
                sequence,
                label
            ));
            remove_database_files(&path);
            Self { path }
        }
    }

    impl Drop for TempDestination {
        fn drop(&mut self) {
            remove_database_files(&self.path);
        }
    }

    fn remove_database_files(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));
    }

    fn fixture_store() -> PaStore {
        let store = PaStore::open_in_memory(DATABASE_KEY).expect("open fixture store");
        store
            .record_message(
                "source-fixture-message",
                "source-fixture-source",
                MessageProvider::Voice,
                "source-fixture-provider-message",
                MessageSummary::new("fixed fixture summary").expect("fixture summary"),
                None,
                None,
                OffsetDateTime::parse("2025-01-02T03:04:05Z", &Rfc3339)
                    .expect("fixture received time"),
            )
            .expect("record fixture message");
        store
    }

    fn open_keyed(path: &Path, key: &[u8]) -> Connection {
        let connection = Connection::open(path).expect("open copied database");
        apply_database_key(&connection, key).expect("apply copied database key");
        connection
    }

    fn schema_snapshot(connection: &Connection) -> (i64, i64, i64, i64) {
        let migrations = connection
            .query_row(
                "SELECT COALESCE(MAX(version), 0), count(*) FROM schema_migrations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("schema migration snapshot");
        let message_count = connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .expect("message count");
        let highest_message_id = connection
            .query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |row| {
                row.get(0)
            })
            .expect("message identity snapshot");
        (
            migrations.0,
            migrations.1,
            message_count,
            highest_message_id,
        )
    }

    #[test]
    fn live_backup_boundary() {
        let store = fixture_store();
        let destination = TempDestination::new("success");
        let source_snapshot = schema_snapshot(store.connection());

        store
            .backup_to_path(&destination.path, DATABASE_KEY)
            .expect("backup succeeds to a fresh destination");

        let copied = open_keyed(&destination.path, DATABASE_KEY);
        let cipher_version: String = copied
            .query_row("PRAGMA cipher_version", [], |row| row.get(0))
            .expect("copied cipher version");
        let quick_check: String = copied
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .expect("copied quick check");
        assert!(!cipher_version.trim().is_empty());
        assert_eq!(quick_check, "ok");
        assert_eq!(schema_snapshot(&copied), source_snapshot);
        assert_eq!(
            copied
                .query_row(
                    "SELECT count(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'configuration'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("configuration schema probe"),
            1
        );
    }

    #[test]
    fn copied_database_rejects_wrong_key_without_secret_or_row_content_in_error() {
        let store = fixture_store();
        let destination = TempDestination::new("wrong-key-reopen");
        store
            .backup_to_path(&destination.path, DATABASE_KEY)
            .expect("create encrypted backup");
        let error = PaStore::open(&destination.path, WRONG_DATABASE_KEY)
            .expect_err("wrong key must fail closed when reopening");

        assert!(matches!(error, StoreError::Sqlite(_)));
        let display = error.to_string();
        let debug = format!("{error:?}");
        for secret in [
            String::from_utf8_lossy(WRONG_DATABASE_KEY),
            "fixed fixture summary".into(),
            destination.path.display().to_string().into(),
        ] {
            assert!(!display.contains(secret.as_ref()));
            assert!(!debug.contains(secret.as_ref()));
        }
        assert!(destination.path.exists());
    }

    #[test]
    fn empty_key_fails_before_destination_activity() {
        let store = fixture_store();
        let destination = TempDestination::new("empty-key");
        let error = store
            .backup_to_path(&destination.path, [])
            .expect_err("empty key must fail");

        assert!(matches!(error, StoreError::EmptyDatabaseKey));
        assert!(!destination.path.exists());
    }

    #[test]
    fn existing_destination_is_unchanged_and_requires_a_new_attempt_path() {
        let store = fixture_store();
        let existing = TempDestination::new("existing");
        let sentinel = b"existing-backup-sentinel";
        fs::write(&existing.path, sentinel).expect("write existing destination");

        let error = store
            .backup_to_path(&existing.path, DATABASE_KEY)
            .expect_err("existing destination must conflict");

        assert!(matches!(
            error,
            StoreError::Conflict {
                resource: "backup destination"
            }
        ));
        assert_eq!(
            fs::read(&existing.path).expect("read existing destination"),
            sentinel
        );

        let retry = TempDestination::new("retry");
        store
            .backup_to_path(&retry.path, DATABASE_KEY)
            .expect("fresh retry destination succeeds");
        assert!(retry.path.exists());
    }
}
