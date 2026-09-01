use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{Connection, OpenFlags, backup::Backup};
#[cfg(unix)]
use rustix::{fs as rustix_fs, process};

use crate::pa::store::{PaStore, StoreError, StoreResult};

const ATTEMPT_FILE_PREFIX: &str = ".agent-voice-backup-attempt-";
const ATTEMPT_COLLISION_LIMIT: usize = 32;
static ATTEMPT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Crate-private root for transient artifacts owned by the PA lifecycle.
///
/// Construction never creates a directory or chooses a system temporary path:
/// deployment supplies the already-isolated lifecycle mount.
pub(crate) struct LifecycleWorkspace {
    directory: File,
}

impl LifecycleWorkspace {
    pub(crate) fn from_private_root(root: impl AsRef<Path>) -> StoreResult<Self> {
        Ok(Self {
            directory: open_private_workspace(root.as_ref())?,
        })
    }

    fn allocate_attempt_name(&self) -> StoreResult<OsString> {
        let _ = &self.directory;
        opaque_attempt_name()
    }
}

impl PaStore {
    /// Copies the live SQLCipher database to a new caller-selected attempt path.
    ///
    /// On Unix, the destination's direct parent must not be group or
    /// other-writable. The method creates an opaque sibling artifact in that
    /// trusted parent, retains the exclusive creation handle, and verifies its
    /// device/inode identity before opening, publishing, or cleaning up the
    /// artifact. A mismatched path fails closed and is left untouched. This
    /// treats same-principal filesystem writers as part of the application
    /// trust boundary. Unsupported platforms fail closed until they have an
    /// equivalent capability and identity check. The later snapshot writer
    /// owns final encoded-snapshot fsync and rename behavior.
    pub fn backup_to_path<P, K>(&self, destination: P, database_key: K) -> StoreResult<()>
    where
        P: AsRef<Path>,
        K: AsRef<[u8]>,
    {
        let key = database_key.as_ref();
        reject_empty_database_key(key)?;

        let expected_schema_version = schema_version(self.connection())?;
        {
            let attempt = AttemptGuard::create(destination.as_ref())?;
            {
                let mut attempt_connection = attempt.open_connection()?;

                apply_database_key(&attempt_connection, key)?;
                verify_destination_cipher(&attempt_connection)?;

                {
                    let backup = Backup::new(self.connection(), &mut attempt_connection)
                        .map_err(|_| backup_error())?;
                    backup
                        .run_to_completion(5, Duration::from_millis(50), None)
                        .map_err(|_| backup_error())?;
                }

                validate_destination(&attempt_connection, expected_schema_version)?;
            }

            attempt.publish(destination.as_ref())?;
        }
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

struct AttemptGuard {
    path: PathBuf,
    file: File,
}

impl AttemptGuard {
    fn create(destination: &Path) -> StoreResult<Self> {
        if destination.file_name().is_none() {
            return Err(backup_error());
        }
        let parent = trusted_attempt_parent(destination)?;
        ensure_destination_absent(destination)?;

        for _ in 0..ATTEMPT_COLLISION_LIMIT {
            let attempt_path = opaque_sibling_attempt_path(parent)?;
            if attempt_path == destination {
                continue;
            }
            match create_owned_attempt_file(&attempt_path) {
                Ok(file) => {
                    return Ok(Self {
                        path: attempt_path,
                        file,
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(backup_error()),
            }
        }
        Err(backup_error())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn open_connection(&self) -> StoreResult<Connection> {
        self.require_owned_path()?;
        Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| backup_error())
    }

    fn publish(&self, destination: &Path) -> StoreResult<()> {
        self.require_owned_path()?;
        match fs::hard_link(&self.path, destination) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(StoreError::Conflict {
                resource: "backup destination",
            }),
            Err(_) => Err(backup_error()),
        }
    }

    fn require_owned_path(&self) -> StoreResult<()> {
        if self.path_matches_owned_file() {
            Ok(())
        } else {
            Err(backup_error())
        }
    }

    #[cfg(unix)]
    fn path_matches_owned_file(&self) -> bool {
        let Ok(file_metadata) = self.file.metadata() else {
            return false;
        };
        let Ok(path_metadata) = fs::symlink_metadata(&self.path) else {
            return false;
        };
        file_metadata.dev() == path_metadata.dev() && file_metadata.ino() == path_metadata.ino()
    }

    #[cfg(not(unix))]
    fn path_matches_owned_file(&self) -> bool {
        false
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if self.path_matches_owned_file() {
            remove_owned_attempt_artifacts(&self.path);
        }
    }
}

fn create_owned_attempt_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

#[cfg(unix)]
fn trusted_attempt_parent(destination: &Path) -> StoreResult<&Path> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::symlink_metadata(parent).map_err(|_| backup_error())?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o022 != 0 {
        return Err(backup_error());
    }
    Ok(parent)
}

#[cfg(not(unix))]
fn trusted_attempt_parent(_destination: &Path) -> StoreResult<&Path> {
    Err(backup_error())
}

#[cfg(unix)]
fn open_private_workspace(root: &Path) -> StoreResult<File> {
    let directory = File::open(root).map_err(|_| backup_error())?;
    let handle_metadata = directory.metadata().map_err(|_| backup_error())?;
    let path_metadata = fs::symlink_metadata(root).map_err(|_| backup_error())?;
    let stat = rustix_fs::fstat(&directory).map_err(|_| backup_error())?;
    if !handle_metadata.is_dir()
        || handle_metadata.dev() != path_metadata.dev()
        || handle_metadata.ino() != path_metadata.ino()
        || stat.st_mode & 0o022 != 0
        || !workspace_owner_matches_effective_uid(stat.st_uid, process::geteuid().as_raw())
    {
        return Err(backup_error());
    }
    Ok(directory)
}

#[cfg(not(unix))]
fn open_private_workspace(_root: &Path) -> StoreResult<File> {
    Err(backup_error())
}

#[cfg(unix)]
fn workspace_owner_matches_effective_uid(root_uid: u32, effective_uid: u32) -> bool {
    root_uid == effective_uid
}

fn ensure_destination_absent(destination: &Path) -> StoreResult<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(StoreError::Conflict {
            resource: "backup destination",
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err(backup_error()),
    }
}

fn opaque_sibling_attempt_path(parent: &Path) -> StoreResult<PathBuf> {
    Ok(parent.join(opaque_attempt_name()?))
}

fn opaque_attempt_name() -> StoreResult<OsString> {
    let mut nonce = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| backup_error())?;
    let sequence = ATTEMPT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(OsString::from(format!(
        "{ATTEMPT_FILE_PREFIX}{}-{sequence}-{}.db",
        std::process::id(),
        hex_encode(&nonce)
    )))
}

fn remove_owned_attempt_artifacts(path: &Path) {
    for artifact in [
        path.to_owned(),
        sidecar_path(path, "-wal"),
        sidecar_path(path, "-shm"),
        sidecar_path(path, "-journal"),
    ] {
        let _ = fs::remove_file(artifact);
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
    use std::path::{Path, PathBuf};

    use ring::rand::{SecureRandom, SystemRandom};
    use rusqlite::Connection;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use crate::pa::store::{MessageProvider, MessageSummary, PaStore, StoreError};

    use super::apply_database_key;

    const DATABASE_KEY: &[u8] = b"source-boundary-test-key";
    const WRONG_DATABASE_KEY: &[u8] = b"source-boundary-wrong-key";
    const TEST_DIRECTORY_PREFIX: &str = "agent-voice-source-boundary-";
    const TEST_DIRECTORY_COLLISION_LIMIT: usize = 32;

    struct TempDestination {
        directory: PathBuf,
        path: PathBuf,
    }

    impl TempDestination {
        fn new(label: &str) -> Self {
            let directory = create_test_directory();
            let path = directory.join(format!("{label}.db"));
            assert!(!path.exists());
            Self { directory, path }
        }

        fn sibling(&self, name: &str) -> PathBuf {
            self.directory.join(name)
        }
    }

    impl Drop for TempDestination {
        fn drop(&mut self) {
            // The fixture exclusively created this 0700 directory. Only its
            // known database artifacts are removed; unexpected files keep the
            // non-recursive directory removal from deleting them.
            remove_database_files(&self.path);
            let _ = fs::remove_dir(&self.directory);
        }
    }

    fn create_test_directory() -> PathBuf {
        for _ in 0..TEST_DIRECTORY_COLLISION_LIMIT {
            let mut nonce = [0_u8; 16];
            SystemRandom::new()
                .fill(&mut nonce)
                .expect("fill test directory nonce");
            let directory = std::env::temp_dir().join(format!(
                "{TEST_DIRECTORY_PREFIX}{}",
                super::hex_encode(&nonce)
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&directory) {
                Ok(()) => {
                    assert_eq!(
                        fs::metadata(&directory)
                            .expect("read private test directory metadata")
                            .permissions()
                            .mode()
                            & 0o077,
                        0
                    );
                    return directory;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create private test directory: {error}"),
            }
        }
        panic!("allocate private test directory");
    }

    fn remove_database_files(path: &Path) {
        for artifact in [
            path.to_owned(),
            super::sidecar_path(path, "-wal"),
            super::sidecar_path(path, "-shm"),
            super::sidecar_path(path, "-journal"),
        ] {
            let _ = fs::remove_file(artifact);
        }
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

    #[test]
    fn failed_backup_preserves_caller_sidecars() {
        let store = fixture_store();
        store
            .connection()
            .execute("DELETE FROM configuration", [])
            .expect("make the copied database fail its post-copy validation");
        let destination = TempDestination::new("failure-cleanup");
        let stale_wal = super::sidecar_path(&destination.path, "-wal");
        let stale_shm = super::sidecar_path(&destination.path, "-shm");
        fs::write(&stale_wal, b"caller-stale-wal").expect("write caller WAL sidecar");
        fs::write(&stale_shm, b"caller-stale-shm").expect("write caller SHM sidecar");

        let error = store
            .backup_to_path(&destination.path, DATABASE_KEY)
            .expect_err("invalid copied database must fail");

        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid { resource: "backup" }
        ));
        assert!(!destination.path.exists());
        assert_eq!(
            fs::read(&stale_wal).expect("caller WAL sidecar survives"),
            b"caller-stale-wal"
        );
        assert_eq!(
            fs::read(&stale_shm).expect("caller SHM sidecar survives"),
            b"caller-stale-shm"
        );
    }

    #[test]
    fn unsafe_parent_is_rejected_before_creating_an_attempt_artifact() {
        let destination = TempDestination::new("unsafe-parent");
        fs::set_permissions(&destination.directory, fs::Permissions::from_mode(0o777))
            .expect("make test parent group and other writable");

        let result = super::AttemptGuard::create(&destination.path);

        fs::set_permissions(&destination.directory, fs::Permissions::from_mode(0o700))
            .expect("restore private test parent");
        let error = match result {
            Ok(_) => panic!("untrusted parent must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid { resource: "backup" }
        ));
        assert!(!destination.path.exists());
        assert!(
            fs::read_dir(&destination.directory)
                .expect("read rejected parent")
                .next()
                .is_none()
        );
    }

    #[test]
    fn lifecycle_workspace_requires_an_existing_private_root() {
        let destination = TempDestination::new("lifecycle-root");
        let workspace = super::LifecycleWorkspace::from_private_root(&destination.directory)
            .expect("private fixture directory is a workspace root");
        let attempt = workspace
            .allocate_attempt_name()
            .expect("allocate opaque workspace attempt name");
        assert!(
            attempt
                .to_string_lossy()
                .starts_with(super::ATTEMPT_FILE_PREFIX)
        );

        let missing = destination.sibling("missing-secret-workspace-root");
        let error = match super::LifecycleWorkspace::from_private_root(&missing) {
            Ok(_) => panic!("missing root must fail closed"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid { resource: "backup" }
        ));
        assert!(!error.to_string().contains("secret-workspace-root"));

        let not_directory = destination.sibling("not-a-secret-workspace-root");
        fs::write(&not_directory, b"not a workspace root").expect("create non-directory root");
        let error = match super::LifecycleWorkspace::from_private_root(&not_directory) {
            Ok(_) => panic!("non-directory root must fail closed"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid { resource: "backup" }
        ));
        assert!(!error.to_string().contains("secret-workspace-root"));
        fs::remove_file(&not_directory).expect("remove non-directory root");

        let symlink_root = destination.sibling("workspace-root-symlink");
        symlink(&destination.directory, &symlink_root).expect("create workspace root symlink");
        let error = match super::LifecycleWorkspace::from_private_root(&symlink_root) {
            Ok(_) => panic!("symlink root must fail closed"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::StoredRecordInvalid { resource: "backup" }
        ));
        fs::remove_file(&symlink_root).expect("remove workspace root symlink");

        fs::set_permissions(&destination.directory, fs::Permissions::from_mode(0o777))
            .expect("make root unsafe");
        let unsafe_error =
            match super::LifecycleWorkspace::from_private_root(&destination.directory) {
                Ok(_) => panic!("writable root must fail closed"),
                Err(error) => error,
            };
        fs::set_permissions(&destination.directory, fs::Permissions::from_mode(0o700))
            .expect("restore fixture permissions");
        assert!(matches!(
            unsafe_error,
            StoreError::StoredRecordInvalid { resource: "backup" }
        ));
    }

    #[test]
    fn lifecycle_workspace_pins_the_validated_directory_handle() {
        let destination = TempDestination::new("lifecycle-pinned-root");
        let workspace = super::LifecycleWorkspace::from_private_root(&destination.directory)
            .expect("construct workspace from private root");
        let relocated = destination.directory.with_extension("relocated-root");
        fs::rename(&destination.directory, &relocated).expect("relocate private root");
        let replacement = fs::DirBuilder::new();
        replacement
            .create(&destination.directory)
            .expect("create replacement root");

        let pinned = workspace
            .directory
            .metadata()
            .expect("read pinned root metadata");
        let relocated_metadata = fs::metadata(&relocated).expect("read relocated root metadata");
        assert_eq!(pinned.dev(), relocated_metadata.dev());
        assert_eq!(pinned.ino(), relocated_metadata.ino());

        fs::remove_dir(&destination.directory).expect("remove replacement root");
        fs::rename(&relocated, &destination.directory).expect("restore fixture root");
    }

    #[test]
    fn lifecycle_workspace_requires_service_owner() {
        assert!(super::workspace_owner_matches_effective_uid(42, 42));
        assert!(!super::workspace_owner_matches_effective_uid(42, 41));
    }

    #[test]
    fn owned_attempt_identity_allows_database_open_and_publication() {
        let destination = TempDestination::new("owned-identity");
        let attempt = super::AttemptGuard::create(&destination.path).expect("create attempt");
        let attempt_path = attempt.path().to_owned();

        attempt
            .open_connection()
            .expect("open exclusively owned attempt");
        fs::write(&attempt_path, b"owned-attempt-bytes").expect("write owned attempt bytes");
        attempt
            .publish(&destination.path)
            .expect("publish exclusively owned attempt");

        assert_eq!(
            fs::read(&destination.path).expect("read published attempt"),
            b"owned-attempt-bytes"
        );
        drop(attempt);
        assert!(!attempt_path.exists());
    }

    #[test]
    fn replacement_after_attempt_creation_never_opens_publishes_or_deletes_it() {
        let destination = TempDestination::new("attempt-replacement");
        let attempt = super::AttemptGuard::create(&destination.path).expect("create attempt");
        let attempt_path = attempt.path().to_owned();
        let sentinel_source = destination.sibling("sentinel-source.db");
        let sentinel = b"replacement-sentinel";
        fs::write(&sentinel_source, sentinel).expect("write replacement source");
        fs::remove_file(&attempt_path).expect("unlink owned attempt path");
        fs::hard_link(&sentinel_source, &attempt_path).expect("replace attempt with sentinel link");

        let open_error = attempt
            .open_connection()
            .expect_err("replaced attempt must not be opened");
        let publish_error = attempt
            .publish(&destination.path)
            .expect_err("replaced attempt must not be published");
        assert!(matches!(
            open_error,
            StoreError::StoredRecordInvalid { resource: "backup" }
        ));
        assert!(matches!(
            publish_error,
            StoreError::StoredRecordInvalid { resource: "backup" }
        ));
        assert!(!destination.path.exists());

        drop(attempt);

        assert_eq!(
            fs::read(&attempt_path).expect("read replacement after attempt drop"),
            sentinel
        );
        assert_eq!(
            fs::read(&sentinel_source).expect("read sentinel source after attempt drop"),
            sentinel
        );
        fs::remove_file(&attempt_path).expect("remove test-created replacement link");
        fs::remove_file(&sentinel_source).expect("remove test-created sentinel source");
    }

    #[test]
    fn publication_race_preserves_the_replacement_without_overwrite() {
        let destination = TempDestination::new("publication-race");
        let attempt = super::AttemptGuard::create(&destination.path).expect("create attempt");
        let attempt_path = attempt.path().to_owned();
        fs::write(&attempt_path, b"attempt-bytes").expect("write attempt bytes");
        fs::write(&destination.path, b"racing-replacement").expect("write replacement");

        let error = attempt
            .publish(&destination.path)
            .expect_err("publication must not overwrite a racing replacement");

        assert!(matches!(
            error,
            StoreError::Conflict {
                resource: "backup destination"
            }
        ));
        assert_eq!(
            fs::read(&destination.path).expect("read racing replacement"),
            b"racing-replacement"
        );
        drop(attempt);
        assert!(!attempt_path.exists());
    }

    #[test]
    fn attempt_cleanup_removes_only_its_opaque_database_artifacts() {
        let destination = TempDestination::new("caller-secret-destination-name");
        let attempt = super::AttemptGuard::create(&destination.path).expect("create attempt");
        let attempt_path = attempt.path().to_owned();
        let attempt_wal = super::sidecar_path(&attempt_path, "-wal");
        let attempt_shm = super::sidecar_path(&attempt_path, "-shm");
        let attempt_journal = super::sidecar_path(&attempt_path, "-journal");
        let destination_wal = super::sidecar_path(&destination.path, "-wal");
        fs::write(&attempt_wal, b"attempt-wal").expect("write attempt WAL sidecar");
        fs::write(&attempt_shm, b"attempt-shm").expect("write attempt SHM sidecar");
        fs::write(&attempt_journal, b"attempt-journal").expect("write attempt journal");
        fs::write(&destination_wal, b"caller-wal").expect("write caller WAL sidecar");

        let attempt_name = attempt_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("opaque attempt name");
        assert!(attempt_name.starts_with(".agent-voice-backup-attempt-"));
        assert!(!attempt_name.contains("caller-secret-destination-name"));

        drop(attempt);

        assert!(!attempt_path.exists());
        assert!(!attempt_wal.exists());
        assert!(!attempt_shm.exists());
        assert!(!attempt_journal.exists());
        assert_eq!(
            fs::read(&destination_wal).expect("caller sidecar survives"),
            b"caller-wal"
        );
    }
}
