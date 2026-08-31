#[cfg(unix)]
use std::ffi::{CStr, CString};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(all(test, unix))]
use std::sync::{Arc, Barrier};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

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

#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe extern "C" {
    fn renameat2(
        old_directory: std::os::raw::c_int,
        old_path: *const std::os::raw::c_char,
        new_directory: std::os::raw::c_int,
        new_path: *const std::os::raw::c_char,
        flags: std::os::raw::c_uint,
    ) -> std::os::raw::c_int;
}

#[cfg(unix)]
unsafe extern "C" {
    fn openat(
        directory: std::os::raw::c_int,
        path: *const std::os::raw::c_char,
        flags: std::os::raw::c_int,
        mode: std::os::raw::c_uint,
    ) -> std::os::raw::c_int;
    fn unlinkat(
        directory: std::os::raw::c_int,
        path: *const std::os::raw::c_char,
        flags: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
unsafe extern "C" {
    fn renameatx_np(
        old_directory: std::os::raw::c_int,
        old_path: *const std::os::raw::c_char,
        new_directory: std::os::raw::c_int,
        new_path: *const std::os::raw::c_char,
        flags: std::os::raw::c_uint,
    ) -> std::os::raw::c_int;
}

#[cfg(all(test, unix))]
static PUBLICATION_BARRIER: OnceLock<Mutex<Option<Arc<Barrier>>>> = OnceLock::new();

#[cfg(all(test, unix))]
static PARENT_HANDOFF_BARRIER: OnceLock<Mutex<Option<Arc<Barrier>>>> = OnceLock::new();

#[cfg(all(test, unix))]
static TEMPORARY_CREATION_BARRIER: OnceLock<Mutex<Option<Arc<Barrier>>>> = OnceLock::new();

#[cfg(all(test, unix))]
fn set_publication_barrier(barrier: Option<Arc<Barrier>>) {
    *PUBLICATION_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("publication barrier is not poisoned") = barrier;
}

#[cfg(all(test, unix))]
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

#[cfg(all(test, unix))]
fn set_parent_handoff_barrier(barrier: Option<Arc<Barrier>>) {
    *PARENT_HANDOFF_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("parent handoff barrier is not poisoned") = barrier;
}

#[cfg(all(test, unix))]
fn wait_for_parent_handoff() {
    let barrier = PARENT_HANDOFF_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("parent handoff barrier is not poisoned")
        .clone();
    if let Some(barrier) = barrier {
        barrier.wait();
        barrier.wait();
    }
}

#[cfg(all(test, unix))]
fn set_temporary_creation_barrier(barrier: Option<Arc<Barrier>>) {
    *TEMPORARY_CREATION_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("temporary creation barrier is not poisoned") = barrier;
}

#[cfg(all(test, unix))]
fn wait_for_temporary_creation() {
    let barrier = TEMPORARY_CREATION_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("temporary creation barrier is not poisoned")
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
    let parent_directory = open_parent_directory(parent)?;
    ensure_destination_absent(destination)?;
    ensure_destination_absent_at(&parent_directory, destination)?;
    #[cfg(all(test, unix))]
    wait_for_temporary_creation();
    let mut temporary = TemporaryFile::create(parent, &parent_directory, destination)?;

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

    ensure_destination_absent_at(&parent_directory, destination)?;
    temporary.ensure_owned()?;
    #[cfg(all(test, unix))]
    wait_for_publication_race();
    temporary.ensure_owned()?;
    #[cfg(all(test, unix))]
    wait_for_parent_handoff();
    if fault == FaultStage::Rename {
        return Err(WriterError::Rename);
    }
    match publish_without_replacement_owned(
        &parent_directory,
        temporary.path(),
        destination,
        temporary.identity(),
    ) {
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
    sync_parent_directory(&parent_directory).map_err(|_| WriterError::DirectorySync)
}

#[cfg(unix)]
fn publish_without_replacement(source: &Path, destination: &Path) -> io::Result<()> {
    let source_identity = fs::symlink_metadata(source)?;
    let parent = source
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_directory = File::open(parent)?;
    publish_without_replacement_owned(&parent_directory, source, destination, &source_identity)
}

#[cfg(unix)]
fn publish_without_replacement_owned(
    parent: &File,
    source: &Path,
    destination: &Path,
    source_identity: &fs::Metadata,
) -> io::Result<()> {
    let source_name = path_name(source)?;
    let destination_name = path_name(destination)?;
    if !path_matches_identity_at(parent, &source_name, source_identity) {
        return Err(io::Error::other("temporary file identity changed"));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        rename_without_replacement_linux(parent, &source_name, &destination_name)
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        rename_without_replacement_darwin(parent, &source_name, &destination_name)
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "atomic no-replace publication is unavailable on this platform",
        ))
    }
}

#[cfg(not(unix))]
fn publish_without_replacement_owned(
    _parent: &File,
    _source: &Path,
    _destination: &Path,
    _source_identity: &fs::Metadata,
) -> io::Result<()> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "atomic publication is unavailable on this platform",
    ))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_without_replacement_linux(
    parent: &File,
    source: &CStr,
    destination: &CStr,
) -> io::Result<()> {
    const RENAME_NOREPLACE: std::os::raw::c_uint = 1;

    let result = unsafe {
        renameat2(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
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

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_without_replacement_darwin(
    parent: &File,
    source: &CStr,
    destination: &CStr,
) -> io::Result<()> {
    const RENAME_EXCL: std::os::raw::c_uint = 0x00000004;
    const RENAME_NOFOLLOW_ANY: std::os::raw::c_uint = 0x00000010;

    let result = unsafe {
        renameatx_np(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
            RENAME_EXCL | RENAME_NOFOLLOW_ANY,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn path_name(path: &Path) -> io::Result<CString> {
    use std::os::unix::ffi::OsStrExt;

    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "path has no file name"))?;
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "path contains NUL"))
}

#[cfg(unix)]
fn open_at(parent: &File, name: &CStr) -> io::Result<File> {
    let flags = no_follow_open_flags()?;
    open_at_with_flags(parent, name, flags, 0)
}

#[cfg(unix)]
fn open_at_with_flags(
    parent: &File,
    name: &CStr,
    flags: std::os::raw::c_int,
    mode: std::os::raw::c_uint,
) -> io::Result<File> {
    let descriptor = unsafe { openat(parent.as_raw_fd(), name.as_ptr(), flags, mode) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn no_follow_open_flags() -> io::Result<std::os::raw::c_int> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        Ok(0o400000 | 0o4000 | 0o2000000)
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        Ok(0x100 | 0x4 | 0x01000000)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "no-follow directory entry access is unavailable on this platform",
        ))
    }
}

#[cfg(unix)]
fn temporary_open_flags() -> io::Result<std::os::raw::c_int> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        Ok(0o2 | 0o100 | 0o200 | 0o400000 | 0o2000000)
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        Ok(0x0002 | 0x0200 | 0x0800 | 0x0100 | 0x01000000)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "exclusive no-follow temporary creation is unavailable on this platform",
        ))
    }
}

#[cfg(unix)]
fn open_temporary_at(parent: &File, name: &CStr) -> io::Result<File> {
    open_at_with_flags(parent, name, temporary_open_flags()?, 0o600)
}

#[cfg(unix)]
fn unlink_at(parent: &File, name: &CStr) -> io::Result<()> {
    let result = unsafe { unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn remove_if_owned(path: &Path, identity: &fs::Metadata) {
    let Ok(current) = fs::symlink_metadata(path) else {
        return;
    };
    if same_file(identity, &current) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(not(unix))]
fn path_matches_identity(path: &Path, identity: &fs::Metadata) -> bool {
    fs::symlink_metadata(path)
        .map(|current| same_file(identity, &current))
        .unwrap_or(false)
}

#[cfg(unix)]
fn path_matches_identity_at(parent: &File, name: &CStr, identity: &fs::Metadata) -> bool {
    open_at(parent, name)
        .and_then(|file| file.metadata())
        .map(|current| same_file(identity, &current))
        .unwrap_or(false)
}

#[cfg(unix)]
fn remove_if_owned_at(parent: &File, name: &CStr, identity: &fs::Metadata) -> bool {
    let Ok(file) = open_at(parent, name) else {
        return false;
    };
    let Ok(current) = file.metadata() else {
        return false;
    };
    if !same_file(identity, &current) {
        return false;
    }
    unlink_at(parent, name).is_ok()
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

#[cfg(unix)]
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

#[cfg(not(unix))]
fn destination_parent(_destination: &Path) -> Result<&Path, WriterError> {
    Err(WriterError::InvalidDestination)
}

#[cfg(unix)]
fn open_parent_directory(parent: &Path) -> Result<File, WriterError> {
    let directory = File::open(parent).map_err(|_| WriterError::InvalidDestination)?;
    let handle_metadata = directory
        .metadata()
        .map_err(|_| WriterError::InvalidDestination)?;
    let path_metadata =
        fs::symlink_metadata(parent).map_err(|_| WriterError::InvalidDestination)?;
    if !handle_metadata.is_dir() || !same_file(&handle_metadata, &path_metadata) {
        return Err(WriterError::InvalidDestination);
    }
    if handle_metadata.permissions().mode() & 0o022 != 0 {
        return Err(WriterError::InvalidDestination);
    }
    Ok(directory)
}

#[cfg(not(unix))]
fn open_parent_directory(_parent: &Path) -> Result<File, WriterError> {
    Err(WriterError::InvalidDestination)
}

fn ensure_destination_absent(destination: &Path) -> Result<(), WriterError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(WriterError::DestinationExists),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err(WriterError::InvalidDestination),
    }
}

#[cfg(unix)]
fn ensure_destination_absent_at(parent: &File, destination: &Path) -> Result<(), WriterError> {
    let name = path_name(destination).map_err(|_| WriterError::InvalidDestination)?;
    match open_at(parent, &name) {
        Ok(_) => Err(WriterError::DestinationExists),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err(WriterError::InvalidDestination),
    }
}

#[cfg(not(unix))]
fn ensure_destination_absent_at(_parent: &File, _destination: &Path) -> Result<(), WriterError> {
    Err(WriterError::InvalidDestination)
}

fn temporary_path(parent: &Path, sequence: u64) -> PathBuf {
    parent.join(format!(
        "{TEMP_FILE_PREFIX}{}-{sequence}.tmp",
        std::process::id()
    ))
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.components()
        .filter(|component| !matches!(component, Component::CurDir))
        .eq(right
            .components()
            .filter(|component| !matches!(component, Component::CurDir)))
}

struct CreatedTemporary {
    path: PathBuf,
    parent: Option<File>,
    file: Option<File>,
    identity: Option<fs::Metadata>,
    armed: bool,
}

impl CreatedTemporary {
    fn new(path: PathBuf, parent: File, file: File) -> Self {
        Self {
            path,
            parent: Some(parent),
            file: Some(file),
            identity: None,
            armed: true,
        }
    }

    fn file(&self) -> &File {
        self.file
            .as_ref()
            .expect("created temporary file remains open during initialization")
    }

    fn into_parts(mut self) -> (PathBuf, File, File, fs::Metadata) {
        self.armed = false;
        (
            std::mem::take(&mut self.path),
            self.parent
                .take()
                .expect("created temporary parent remains open during initialization"),
            self.file
                .take()
                .expect("created temporary file remains open during initialization"),
            self.identity
                .take()
                .expect("created temporary identity is recorded before publication"),
        )
    }
}

impl Drop for CreatedTemporary {
    fn drop(&mut self) {
        if self.armed {
            #[cfg(unix)]
            {
                let Some(parent) = self.parent.as_ref() else {
                    return;
                };
                let Ok(name) = path_name(&self.path) else {
                    return;
                };
                if let Some(identity) = self.identity.as_ref() {
                    let _ = remove_if_owned_at(parent, &name, identity);
                } else {
                    let _ = unlink_at(parent, &name);
                }
            }
            #[cfg(not(unix))]
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

struct TemporaryFile {
    path: PathBuf,
    parent: File,
    file: Option<File>,
    identity: fs::Metadata,
    armed: bool,
}

impl TemporaryFile {
    fn create(
        parent: &Path,
        parent_directory: &File,
        destination: &Path,
    ) -> Result<Self, WriterError> {
        for _ in 0..TEMP_COLLISION_LIMIT {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = temporary_path(parent, sequence);
            if same_path(&path, destination) {
                continue;
            }
            let parent_handle = parent_directory
                .try_clone()
                .map_err(|_| WriterError::Write)?;
            #[cfg(unix)]
            let open_result = {
                let name = path_name(&path).map_err(|_| WriterError::Write)?;
                open_temporary_at(&parent_handle, &name)
            };
            #[cfg(not(unix))]
            let open_result: io::Result<File> = Err(io::Error::new(
                ErrorKind::Unsupported,
                "exclusive no-follow temporary creation is unavailable on this platform",
            ));
            match open_result {
                Ok(file) => {
                    let mut created = CreatedTemporary::new(path, parent_handle, file);
                    let identity = match created.file().metadata() {
                        Ok(identity) => identity,
                        Err(_) => return Err(WriterError::Write),
                    };
                    created.identity = Some(identity);
                    #[cfg(unix)]
                    if created
                        .file()
                        .set_permissions(fs::Permissions::from_mode(0o600))
                        .is_err()
                    {
                        return Err(WriterError::Write);
                    }
                    let (path, parent, file, identity) = created.into_parts();
                    return Ok(Self {
                        path,
                        parent,
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
        #[cfg(unix)]
        let owned = path_name(&self.path)
            .map(|name| path_matches_identity_at(&self.parent, &name, &self.identity))
            .unwrap_or(false);
        #[cfg(not(unix))]
        let owned = path_matches_identity(&self.path, &self.identity);
        if owned {
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
            #[cfg(unix)]
            {
                let removed = path_name(&self.path)
                    .map(|name| remove_if_owned_at(&self.parent, &name, &self.identity))
                    .unwrap_or(false);
                if !removed {
                    remove_if_owned(&self.path, &self.identity);
                }
            }
            #[cfg(not(unix))]
            remove_if_owned(&self.path, &self.identity);
        }
        self.file.take();
    }
}

#[cfg(unix)]
fn sync_parent_directory(parent: &File) -> io::Result<()> {
    parent.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &File) -> io::Result<()> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "directory synchronization is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    use std::os::unix::fs::symlink;
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    use std::os::fd::AsRawFd;
    #[cfg(unix)]
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    use std::sync::{Arc, Barrier};
    use std::sync::{Mutex, OnceLock};

    use super::{AtomicSnapshotWriter, WriterError, WriterFault, write_with_fault};

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    unsafe extern "C" {
        fn fcntl(
            descriptor: std::os::raw::c_int,
            command: std::os::raw::c_int,
            ...,
        ) -> std::os::raw::c_int;
    }

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
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                builder.mode(0o700);
                match builder.create(&path) {
                    Ok(()) => {
                        #[cfg(unix)]
                        {
                            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                                .expect("set private test directory permissions");
                            assert_eq!(
                                fs::symlink_metadata(&path)
                                    .expect("test directory metadata")
                                    .permissions()
                                    .mode()
                                    & 0o777,
                                0o700,
                                "test directory must be owner-only"
                            );
                        }
                        return Self { path };
                    }
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

    #[cfg(unix)]
    #[test]
    fn created_temporary_guard_removes_unpublished_sibling() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let path = directory.destination("guarded.tmp");
        let parent = fs::File::open(&directory.path).expect("test parent remains openable");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("created temporary sibling");

        let guard = super::CreatedTemporary::new(path.clone(), parent, file);
        drop(guard);

        assert!(!path.exists(), "failed initialization removes sibling");
    }

    #[cfg(unix)]
    #[test]
    fn created_temporary_guard_cleanup_uses_pinned_parent() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let path = directory.destination("guarded.tmp");
        let moved = directory.path.with_file_name(format!(
            "{}-moved-guard",
            directory
                .path
                .file_name()
                .expect("test directory has a name")
                .to_string_lossy()
        ));
        let parent = fs::File::open(&directory.path).expect("test parent remains openable");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("created temporary sibling");
        let identity = file.metadata().expect("created temporary identity");

        let mut guard = super::CreatedTemporary::new(path.clone(), parent, file);
        guard.identity = Some(identity);
        fs::rename(&directory.path, &moved).expect("rename pinned parent directory");
        fs::create_dir(&directory.path).expect("replace parent directory");
        fs::write(&path, b"foreign temporary bytes").expect("write replacement sibling");
        drop(guard);

        let pinned_path = moved.join("guarded.tmp");
        let pinned_path_exists = pinned_path.exists();
        assert_eq!(
            fs::read(&path).expect("replacement sibling remains"),
            b"foreign temporary bytes"
        );
        if pinned_path_exists {
            fs::remove_file(&pinned_path).expect("remove leaked pinned sibling after assertion");
        }
        fs::remove_file(&path).expect("remove replacement sibling");
        fs::remove_dir(&directory.path).expect("remove replacement parent");
        fs::remove_dir(&moved).expect("remove moved parent");
        assert!(!pinned_path_exists, "failed initialization removes pinned sibling");
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    #[test]
    fn raw_openat_descriptors_are_close_on_exec() {
        const F_GETFD: std::os::raw::c_int = 1;
        const FD_CLOEXEC: std::os::raw::c_int = 1;

        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("destination.bin");
        let parent = fs::File::open(&directory.path).expect("test parent remains openable");
        let temporary = super::TemporaryFile::create(&directory.path, &parent, &destination)
            .expect("create temporary sibling");
        let temporary_file = temporary
            .file
            .as_ref()
            .expect("temporary file remains open");
        let temporary_flags = unsafe { fcntl(temporary_file.as_raw_fd(), F_GETFD) };
        assert!(
            temporary_flags >= 0 && temporary_flags & FD_CLOEXEC != 0,
            "temporary openat descriptor must be close-on-exec"
        );

        let name = super::path_name(temporary.path()).expect("temporary sibling has a name");
        let probe = super::open_at(&temporary.parent, &name).expect("open temporary identity");
        let probe_flags = unsafe { fcntl(probe.as_raw_fd(), F_GETFD) };
        assert!(
            probe_flags >= 0 && probe_flags & FD_CLOEXEC != 0,
            "identity openat descriptor must be close-on-exec"
        );
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    fn published_or_directory_sync(result: Result<(), WriterError>) {
        assert_eq!(result, Ok(()));
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    #[test]
    fn source_symlink_is_not_published() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let source = directory.destination("source.tmp");
        let alias = directory.destination("source-alias.tmp");
        let destination = directory.destination("destination.bin");
        fs::write(&alias, SNAPSHOT).expect("source bytes");
        symlink(&alias, &source).expect("replace source with symlink");

        let result = super::publish_without_replacement(&source, &destination);

        assert!(result.is_err(), "source symlink must fail publication");
        assert!(!destination.exists());
        assert!(
            fs::symlink_metadata(&source)
                .expect("source symlink remains")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&alias).expect("aliased source remains"), SNAPSHOT);
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[test]
    fn temporary_path_comparison_normalizes_current_directory() {
        let _lock = lock_tests();
        let sequence = super::TEMP_SEQUENCE.load(Ordering::Relaxed);
        let bare_destination = PathBuf::from(format!(
            "{}{}-{sequence}.tmp",
            super::TEMP_FILE_PREFIX,
            std::process::id()
        ));
        let synthesized = super::temporary_path(Path::new("."), sequence);

        assert!(super::same_path(&bare_destination, &synthesized));
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
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

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    #[test]
    fn replaced_temporary_symlink_is_not_published_or_deleted() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("destination.bin");
        let alias = directory.destination("temporary-alias.tmp");
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
        fs::hard_link(&temporary, &alias).expect("retain temporary identity as an alias");
        fs::remove_file(&temporary).expect("remove writer temporary sibling");
        symlink(&alias, &temporary).expect("replace writer temporary sibling with symlink");
        barrier.wait();

        let result = writer.join().expect("writer thread completes");
        super::set_publication_barrier(None);
        assert_eq!(result, Err(WriterError::Rename));
        assert!(!destination.exists());
        assert!(
            fs::symlink_metadata(&temporary)
                .expect("foreign symlink remains")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(&alias).expect("aliased snapshot remains"),
            SNAPSHOT
        );
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    #[test]
    fn validated_parent_handle_pins_temporary_creation() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("destination.bin");
        let moved = directory.path.with_file_name(format!(
            "{}-moved-before-create",
            directory
                .path
                .file_name()
                .expect("test directory has a name")
                .to_string_lossy()
        ));
        assert!(!moved.exists(), "moved test directory must be unused");

        let barrier = Arc::new(Barrier::new(2));
        super::set_temporary_creation_barrier(Some(Arc::clone(&barrier)));
        let writer_destination = destination.clone();
        let writer =
            std::thread::spawn(move || AtomicSnapshotWriter::write(&writer_destination, SNAPSHOT));

        barrier.wait();
        fs::rename(&directory.path, &moved).expect("rename validated parent directory");
        fs::create_dir(&directory.path).expect("replace parent directory");
        let foreign_destination = directory.path.join("destination.bin");
        fs::write(&foreign_destination, b"foreign destination bytes")
            .expect("write replacement destination");
        barrier.wait();

        let result = writer.join().expect("writer thread completes");
        super::set_temporary_creation_barrier(None);
        assert_eq!(result, Ok(()));
        assert_eq!(
            fs::read(moved.join("destination.bin")).expect("published snapshot in pinned parent"),
            SNAPSHOT
        );
        assert_eq!(
            fs::read(&foreign_destination).expect("replacement destination remains"),
            b"foreign destination bytes"
        );
        assert!(directory.temporary_entries().is_empty());

        fs::remove_file(&foreign_destination).expect("remove replacement destination");
        fs::remove_dir(&directory.path).expect("remove replacement parent");
        fs::remove_file(moved.join("destination.bin")).expect("remove published snapshot");
        fs::remove_dir(&moved).expect("remove moved parent");
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    #[test]
    fn validated_parent_handle_pins_publication() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("destination.bin");
        let moved = directory.path.with_file_name(format!(
            "{}-moved",
            directory
                .path
                .file_name()
                .expect("test directory has a name")
                .to_string_lossy()
        ));
        assert!(!moved.exists(), "moved test directory must be unused");

        let barrier = Arc::new(Barrier::new(2));
        super::set_parent_handoff_barrier(Some(Arc::clone(&barrier)));
        let writer_destination = destination.clone();
        let writer =
            std::thread::spawn(move || AtomicSnapshotWriter::write(&writer_destination, SNAPSHOT));

        barrier.wait();
        let entries = directory.temporary_entries();
        assert_eq!(entries.len(), 1, "writer should own one temporary sibling");
        let temporary_name = entries[0]
            .file_name()
            .expect("temporary sibling has a name")
            .to_owned();
        fs::rename(&directory.path, &moved).expect("rename validated parent directory");
        fs::create_dir(&directory.path).expect("replace parent directory");
        let foreign_temporary = directory.path.join(&temporary_name);
        fs::write(&foreign_temporary, b"foreign temporary bytes")
            .expect("write replacement temporary sibling");
        barrier.wait();

        let result = writer.join().expect("writer thread completes");
        super::set_parent_handoff_barrier(None);
        assert_eq!(result, Ok(()));
        assert_eq!(
            fs::read(moved.join("destination.bin")).expect("published snapshot in pinned parent"),
            SNAPSHOT
        );
        assert_eq!(
            fs::read(&foreign_temporary).expect("replacement sibling remains"),
            b"foreign temporary bytes"
        );

        fs::remove_file(&foreign_temporary).expect("remove replacement sibling");
        fs::remove_dir(&directory.path).expect("remove replacement parent");
        fs::remove_file(moved.join("destination.bin")).expect("remove published snapshot");
        fs::remove_dir(&moved).expect("remove moved parent");
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    #[test]
    fn writer_fails_closed_before_temporary_creation_on_unsupported_platform() {
        let _lock = lock_tests();
        let directory = TestDirectory::new();
        let destination = directory.destination("destination.bin");

        assert_eq!(
            AtomicSnapshotWriter::write(&destination, SNAPSHOT),
            Err(WriterError::InvalidDestination)
        );
        assert!(!destination.exists());
        assert!(directory.temporary_entries().is_empty());
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
