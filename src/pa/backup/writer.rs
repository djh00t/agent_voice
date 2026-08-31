use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::{Arc, Barrier, Mutex, OnceLock};

const TEMP_FILE_PREFIX: &str = ".agent-voice-snapshot-";
const TEMP_COLLISION_LIMIT: usize = 32;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Publishes an already-encoded snapshot with a durable temporary file.
pub struct AtomicSnapshotWriter;

/// Fixed, redacted errors returned by the atomic snapshot writer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WriterError {
    /// The caller-selected destination already exists.
    DestinationExists,
    /// The destination or its parent cannot be used for publication.
    InvalidDestination,
    /// The encoded snapshot is empty.
    InvalidInput,
    /// The temporary snapshot could not be written.
    Write,
    /// The temporary snapshot could not be synchronized.
    Sync,
    /// The temporary snapshot could not be renamed.
    Rename,
    /// The destination parent could not be synchronized.
    DirectorySync,
}

impl fmt::Debug for WriterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DestinationExists => "WriterError::DestinationExists",
            Self::InvalidDestination => "WriterError::InvalidDestination",
            Self::InvalidInput => "WriterError::InvalidInput",
            Self::Write => "WriterError::Write",
            Self::Sync => "WriterError::Sync",
            Self::Rename => "WriterError::Rename",
            Self::DirectorySync => "WriterError::DirectorySync",
        })
    }
}

impl fmt::Display for WriterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DestinationExists => "snapshot writer error: destination exists",
            Self::InvalidDestination => "snapshot writer error: invalid destination",
            Self::InvalidInput => "snapshot writer error: invalid input",
            Self::Write => "snapshot writer error: write failed",
            Self::Sync => "snapshot writer error: file sync failed",
            Self::Rename => "snapshot writer error: rename failed",
            Self::DirectorySync => "snapshot writer error: directory sync failed",
        })
    }
}

impl std::error::Error for WriterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl AtomicSnapshotWriter {
    /// Writes exact encoded snapshot bytes to a new destination atomically.
    pub fn write(
        destination: &std::path::Path,
        encoded_snapshot: &[u8],
    ) -> Result<(), WriterError> {
        write_inner(destination, encoded_snapshot, FaultStage::None)
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterFault {
    Write,
    FileSync,
    Rename,
    DirectorySync,
}

#[cfg(test)]
pub(crate) fn write_with_fault(
    destination: &std::path::Path,
    encoded_snapshot: &[u8],
    fault: WriterFault,
) -> Result<(), WriterError> {
    write_inner(destination, encoded_snapshot, fault.into())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FaultStage {
    None,
    Write,
    FileSync,
    Rename,
    DirectorySync,
}

#[cfg(test)]
impl From<WriterFault> for FaultStage {
    fn from(fault: WriterFault) -> Self {
        match fault {
            WriterFault::Write => Self::Write,
            WriterFault::FileSync => Self::FileSync,
            WriterFault::Rename => Self::Rename,
            WriterFault::DirectorySync => Self::DirectorySync,
        }
    }
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn renameat2(
        old_directory: std::os::raw::c_int,
        old_path: *const std::os::raw::c_char,
        new_directory: std::os::raw::c_int,
        new_path: *const std::os::raw::c_char,
        flags: std::os::raw::c_uint,
    ) -> std::os::raw::c_int;
}

#[cfg(test)]
static PUBLICATION_BARRIER: OnceLock<Mutex<Option<Arc<Barrier>>>> = OnceLock::new();

#[cfg(test)]
fn set_publication_barrier(barrier: Option<Arc<Barrier>>) {
    *PUBLICATION_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("publication barrier is not poisoned") = barrier;
}

#[cfg(test)]
fn wait_for_publication_race() {
    let barrier = PUBLICATION_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("publication barrier is not poisoned")
        .clone();
    if let Some(barrier) = barrier {
        barrier.wait();
        barrier.wait();
    }
}

fn write_inner(
    destination: &Path,
    encoded_snapshot: &[u8],
    fault: FaultStage,
) -> Result<(), WriterError> {
    if encoded_snapshot.is_empty() {
        return Err(WriterError::InvalidInput);
    }

    let parent = destination_parent(destination)?;
    ensure_destination_absent(destination)?;
    let mut temporary = TemporaryFile::create(parent, destination)?;

    if fault == FaultStage::Write {
        return Err(WriterError::Write);
    }
    temporary
        .file_mut()
        .write_all(encoded_snapshot)
        .map_err(|_| WriterError::Write)?;

    if fault == FaultStage::FileSync {
        return Err(WriterError::Sync);
    }
    temporary
        .file_mut()
        .sync_all()
        .map_err(|_| WriterError::Sync)?;

    ensure_destination_absent(destination)?;
    temporary.ensure_owned()?;
    #[cfg(test)]
    wait_for_publication_race();
    temporary.ensure_owned()?;
    if fault == FaultStage::Rename {
        return Err(WriterError::Rename);
    }
    match publish_without_replacement_owned(temporary.path(), destination, temporary.identity()) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(WriterError::DestinationExists);
        }
        Err(_) => return Err(WriterError::Rename),
    }
    temporary.disarm();

    if fault == FaultStage::DirectorySync {
        return Err(WriterError::DirectorySync);
    }
    sync_parent_directory(parent).map_err(|_| WriterError::DirectorySync)
}

fn publish_without_replacement(source: &Path, destination: &Path) -> io::Result<()> {
    let source_identity = fs::symlink_metadata(source)?;
    publish_without_replacement_owned(source, destination, &source_identity)
}

fn publish_without_replacement_owned(
    source: &Path,
    destination: &Path,
    source_identity: &fs::Metadata,
) -> io::Result<()> {
    if !path_matches_identity(source, source_identity) {
        return Err(io::Error::other("temporary file identity changed"));
    }

    #[cfg(target_os = "linux")]
    {
        match rename_without_replacement_linux(source, destination) {
            Ok(()) => return Ok(()),
            Err(error) if rename_noreplace_unavailable(&error) => {}
            Err(error) => return Err(error),
        }
    }

    fs::hard_link(source, destination)?;
    if !path_matches_identity(source, source_identity) {
        remove_if_owned(destination, source_identity);
        return Err(io::Error::other("temporary file identity changed"));
    }
    if let Err(error) = fs::remove_file(source) {
        remove_if_owned(destination, source_identity);
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn rename_without_replacement_linux(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "source path contains NUL"))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "destination path contains NUL"))?;
    const AT_FDCWD: std::os::raw::c_int = -100;
    const RENAME_NOREPLACE: std::os::raw::c_uint = 1;

    let result = unsafe {
        renameat2(
            AT_FDCWD,
            source.as_ptr(),
            AT_FDCWD,
            destination.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_noreplace_unavailable(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(22 | 38 | 95))
}

fn remove_if_owned(path: &Path, identity: &fs::Metadata) {
    let Ok(current) = fs::symlink_metadata(path) else {
        return;
    };
    if same_file(identity, &current) {
        let _ = fs::remove_file(path);
    }
}

fn path_matches_identity(path: &Path, identity: &fs::Metadata) -> bool {
    fs::symlink_metadata(path)
        .map(|current| same_file(identity, &current))
        .unwrap_or(false)
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(windows)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.volume_serial_number() == right.volume_serial_number()
        && left.file_index() == right.file_index()
}

#[cfg(not(any(unix, windows)))]
fn same_file(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    false
}

fn destination_parent(destination: &Path) -> Result<&Path, WriterError> {
    if destination.file_name().is_none() {
        return Err(WriterError::InvalidDestination);
    }
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::symlink_metadata(parent).map_err(|_| WriterError::InvalidDestination)?;
    if !metadata.is_dir() {
        return Err(WriterError::InvalidDestination);
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o022 != 0 {
        return Err(WriterError::InvalidDestination);
    }
    Ok(parent)
}

fn ensure_destination_absent(destination: &Path) -> Result<(), WriterError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(WriterError::DestinationExists),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err(WriterError::InvalidDestination),
    }
}

fn temporary_path(parent: &Path, sequence: u64) -> PathBuf {
    parent.join(format!(
        "{TEMP_FILE_PREFIX}{}-{sequence}.tmp",
        std::process::id()
    ))
}

struct TemporaryFile {
    path: PathBuf,
    file: Option<File>,
    identity: fs::Metadata,
    armed: bool,
}

impl TemporaryFile {
    fn create(parent: &Path, destination: &Path) -> Result<Self, WriterError> {
        for _ in 0..TEMP_COLLISION_LIMIT {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = temporary_path(parent, sequence);
            if path == destination {
                continue;
            }
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;

                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    let identity = match file.metadata() {
                        Ok(identity) => identity,
                        Err(_) => {
                            drop(file);
                            return Err(WriterError::Write);
                        }
                    };
                    return Ok(Self {
                        path,
                        file: Some(file),
                        identity,
                        armed: true,
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(WriterError::Write),
            }
        }
        Err(WriterError::Write)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn identity(&self) -> &fs::Metadata {
        &self.identity
    }

    fn ensure_owned(&self) -> Result<(), WriterError> {
        if path_matches_identity(&self.path, &self.identity) {
            Ok(())
        } else {
            Err(WriterError::Rename)
        }
    }

    fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("temporary file remains open until publication")
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if self.armed {
            remove_if_owned(&self.path, &self.identity);
        }
        self.file.take();
    }
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "directory synchronization is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier, Mutex, OnceLock};

    use super::{AtomicSnapshotWriter, WriterError, WriterFault, write_with_fault};

    const SNAPSHOT: &[u8] = b"agent-voice-encoded-snapshot-v1";
    const TEST_PAYLOAD: &[u8] =
        b"payload-sentinel:key-sentinel:header-sentinel:sql-sentinel:object-key-sentinel:checksum-sentinel";

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let root = std::env::temp_dir();
            loop {
                let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let path = root.join(format!(
                    "agent-voice-writer-contract-{}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("failed to create test directory: {error}"),
                }
            }
        }

        fn destination(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }

        fn temporary_entries(&self) -> Vec<PathBuf> {
            fs::read_dir(&self.path)
                .expect("test directory remains readable")
                .map(|entry| entry.expect("test entry is readable").path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(super::TEMP_FILE_PREFIX))
                })
                .collect()
        }

        fn temporary_entries_except(&self, excluded: &Path) -> Vec<PathBuf> {
            self.temporary_entries()
                .into_iter()
                .filter(|path| path != excluded)
                .collect()
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            if let Ok(entries) = fs::read_dir(&self.path) {
                for entry in entries.flatten() {
                    let _ = fs::remove_file(entry.path());
                }
            }
            let _ = fs::remove_dir(&self.path);
        }
    }

    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("writer test lock is not poisoned")
    }

    fn published_or_directory_sync(result: Result<(), WriterError>) {
        #[cfg(unix)]
        assert_eq!(result, Ok(()));
        #[cfg(not(unix))]
        assert_eq!(result, Err(WriterError::DirectorySync));
    }

    #[test]
    fn atomic_writer_contract() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("snapshot.bin");

        published_or_directory_sync(AtomicSnapshotWriter::write(&destination, SNAPSHOT));
        assert_eq!(
            fs::read(&destination).expect("published snapshot"),
            SNAPSHOT
        );
        assert!(directory.temporary_entries().is_empty());

        let existing = directory.destination("existing.bin");
        fs::write(&existing, b"prior snapshot bytes").expect("existing snapshot");
        assert_eq!(
            AtomicSnapshotWriter::write(&existing, SNAPSHOT),
            Err(WriterError::DestinationExists)
        );
        assert_eq!(
            fs::read(&existing).expect("existing snapshot remains readable"),
            b"prior snapshot bytes"
        );

        let empty_existing = directory.destination("empty-existing.bin");
        fs::File::create(&empty_existing).expect("empty existing snapshot");
        assert_eq!(
            AtomicSnapshotWriter::write(&empty_existing, SNAPSHOT),
            Err(WriterError::DestinationExists)
        );
        assert!(
            fs::read(&empty_existing)
                .expect("empty destination remains readable")
                .is_empty()
        );

        let empty_input = directory.destination("empty-input.bin");
        assert_eq!(
            AtomicSnapshotWriter::write(&empty_input, &[]),
            Err(WriterError::InvalidInput)
        );
        assert!(!empty_input.exists());
        assert!(directory.temporary_entries().is_empty());

        let invalid_parent = directory.destination("missing").join("snapshot.bin");
        assert_eq!(
            AtomicSnapshotWriter::write(&invalid_parent, SNAPSHOT),
            Err(WriterError::InvalidDestination)
        );
        assert!(directory.temporary_entries().is_empty());
    }

    #[test]
    fn pre_rename_faults_remove_only_their_temporary_sibling() {
        let _lock = lock_tests();
        for (fault, expected) in [
            (WriterFault::Write, WriterError::Write),
            (WriterFault::FileSync, WriterError::Sync),
            (WriterFault::Rename, WriterError::Rename),
        ] {
            let directory = TestDirectory::new();
            let destination = directory.destination("snapshot.bin");
            let unrelated = directory.destination("unrelated.bin");
            fs::write(&unrelated, b"unrelated bytes").expect("unrelated sibling");

            assert_eq!(
                write_with_fault(&destination, SNAPSHOT, fault),
                Err(expected)
            );
            assert!(!destination.exists());
            assert_eq!(
                fs::read(&unrelated).expect("unrelated sibling remains"),
                b"unrelated bytes"
            );
            assert!(directory.temporary_entries().is_empty());
        }
    }

    #[test]
    fn directory_sync_fault_leaves_complete_bytes_without_a_temporary_sibling() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("snapshot.bin");

        assert_eq!(
            write_with_fault(&destination, SNAPSHOT, WriterFault::DirectorySync),
            Err(WriterError::DirectorySync)
        );
        assert_eq!(
            fs::read(&destination).expect("final bytes remain complete"),
            SNAPSHOT
        );
        assert!(directory.temporary_entries().is_empty());
        assert_eq!(
            AtomicSnapshotWriter::write(&destination, SNAPSHOT),
            Err(WriterError::DestinationExists)
        );
    }

    #[test]
    fn a_failed_attempt_can_retry_the_same_bytes_at_a_new_destination() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let first_destination = directory.destination("first.bin");
        let retry_destination = directory.destination("retry.bin");

        assert_eq!(
            write_with_fault(&first_destination, SNAPSHOT, WriterFault::Rename),
            Err(WriterError::Rename)
        );
        assert!(!first_destination.exists());
        assert!(directory.temporary_entries().is_empty());

        published_or_directory_sync(AtomicSnapshotWriter::write(&retry_destination, SNAPSHOT));
        assert_eq!(
            fs::read(&retry_destination).expect("retry snapshot"),
            SNAPSHOT
        );
        assert!(directory.temporary_entries().is_empty());
    }

    #[test]
    fn create_new_collision_preserves_the_foreign_sibling() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("snapshot.bin");
        let sequence = super::TEMP_SEQUENCE.load(Ordering::Relaxed);
        let collision = super::temporary_path(&directory.path, sequence);
        fs::write(&collision, b"foreign temporary bytes").expect("foreign sibling");

        published_or_directory_sync(AtomicSnapshotWriter::write(&destination, SNAPSHOT));
        assert_eq!(
            fs::read(&destination).expect("published snapshot"),
            SNAPSHOT
        );
        assert_eq!(
            fs::read(&collision).expect("foreign sibling remains"),
            b"foreign temporary bytes"
        );
        assert!(directory.temporary_entries().len() == 1);
        fs::remove_file(&collision).expect("remove test-owned collision");
        assert!(directory.temporary_entries().is_empty());
    }

    #[test]
    fn no_replace_publication_preserves_an_existing_destination() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let source = directory.destination("source.tmp");
        let destination = directory.destination("destination.bin");
        fs::write(&source, SNAPSHOT).expect("source snapshot");
        fs::write(&destination, b"prior destination bytes").expect("existing destination");

        let error = super::publish_without_replacement(&source, &destination)
            .expect_err("existing destination must reject publication");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&source).expect("source remains"), SNAPSHOT);
        assert_eq!(
            fs::read(&destination).expect("destination remains"),
            b"prior destination bytes"
        );
    }

    #[test]
    fn concurrent_destination_creation_is_not_replaced() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("destination.bin");
        let barrier = Arc::new(Barrier::new(2));
        super::set_publication_barrier(Some(Arc::clone(&barrier)));

        let writer_destination = destination.clone();
        let writer =
            std::thread::spawn(move || AtomicSnapshotWriter::write(&writer_destination, SNAPSHOT));
        barrier.wait();
        fs::write(&destination, b"concurrent destination bytes")
            .expect("concurrent writer creates destination");
        barrier.wait();

        let result = writer.join().expect("writer thread completes");
        super::set_publication_barrier(None);
        assert_eq!(result, Err(WriterError::DestinationExists));
        assert_eq!(
            fs::read(&destination).expect("concurrent destination remains"),
            b"concurrent destination bytes"
        );
        assert!(directory.temporary_entries().is_empty());
    }

    #[test]
    fn generated_temporary_sibling_never_equals_the_destination() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let sequence = super::TEMP_SEQUENCE.load(Ordering::Relaxed);
        let destination = super::temporary_path(&directory.path, sequence);

        published_or_directory_sync(AtomicSnapshotWriter::write(&destination, SNAPSHOT));
        assert_eq!(
            fs::read(&destination).expect("published snapshot"),
            SNAPSHOT
        );
        assert!(directory.temporary_entries_except(&destination).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn writable_parent_is_rejected_before_temporary_creation() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("destination.bin");
        fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o777))
            .expect("make test parent writable");

        let result = AtomicSnapshotWriter::write(&destination, SNAPSHOT);

        fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
            .expect("restore private test parent");
        assert_eq!(result, Err(WriterError::InvalidDestination));
        assert!(!destination.exists());
        assert!(directory.temporary_entries().is_empty());
    }

    #[test]
    fn replaced_temporary_sibling_is_not_published_or_deleted() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("destination.bin");
        let barrier = Arc::new(Barrier::new(2));
        super::set_publication_barrier(Some(Arc::clone(&barrier)));

        let writer_destination = destination.clone();
        let writer =
            std::thread::spawn(move || AtomicSnapshotWriter::write(&writer_destination, SNAPSHOT));
        barrier.wait();
        let entries = directory.temporary_entries();
        assert_eq!(entries.len(), 1, "writer should own one temporary sibling");
        let temporary = entries
            .into_iter()
            .next()
            .expect("writer temporary sibling exists");
        fs::remove_file(&temporary).expect("remove writer temporary sibling");
        fs::write(&temporary, b"foreign temporary bytes")
            .expect("replace writer temporary sibling");
        barrier.wait();

        let result = writer.join().expect("writer thread completes");
        super::set_publication_barrier(None);
        assert_eq!(result, Err(WriterError::Rename));
        assert!(!destination.exists());
        assert_eq!(
            fs::read(&temporary).expect("foreign temporary sibling remains"),
            b"foreign temporary bytes"
        );
        assert_eq!(directory.temporary_entries().len(), 1);
    }

    #[test]
    fn writer_errors_are_fixed_redacted_and_have_no_source() {
        let cases = [
            (
                WriterError::DestinationExists,
                "WriterError::DestinationExists",
                "snapshot writer error: destination exists",
            ),
            (
                WriterError::InvalidDestination,
                "WriterError::InvalidDestination",
                "snapshot writer error: invalid destination",
            ),
            (
                WriterError::InvalidInput,
                "WriterError::InvalidInput",
                "snapshot writer error: invalid input",
            ),
            (
                WriterError::Write,
                "WriterError::Write",
                "snapshot writer error: write failed",
            ),
            (
                WriterError::Sync,
                "WriterError::Sync",
                "snapshot writer error: file sync failed",
            ),
            (
                WriterError::Rename,
                "WriterError::Rename",
                "snapshot writer error: rename failed",
            ),
            (
                WriterError::DirectorySync,
                "WriterError::DirectorySync",
                "snapshot writer error: directory sync failed",
            ),
        ];

        for (error, debug, display) in cases {
            assert_eq!(format!("{error:?}"), debug);
            assert_eq!(error.to_string(), display);
            assert!(error.source().is_none());
            for sentinel in [
                "payload-sentinel",
                "key-sentinel",
                "header-sentinel",
                "sql-sentinel",
                "object-key-sentinel",
                "checksum-sentinel",
            ] {
                assert!(!debug.contains(sentinel));
                assert!(!display.contains(sentinel));
            }
        }

        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("sentinel.bin");
        let error = write_with_fault(&destination, TEST_PAYLOAD, WriterFault::Write)
            .expect_err("fault seam returns a typed error");
        let debug = format!("{error:?}");
        let display = error.to_string();
        assert!(!debug.contains("sentinel"));
        assert!(!display.contains("sentinel"));
        assert!(directory.temporary_entries().is_empty());
    }
}
