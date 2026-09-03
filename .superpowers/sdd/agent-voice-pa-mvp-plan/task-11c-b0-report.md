# Task 11c-b0 decision: directory-handle SQLite capability

## Decision

The current repository has no safe cross-platform capability to open a
SQLCipher database relative to the pinned `LifecycleWorkspace` directory
handle. Do not implement #387 with a re-resolved workspace pathname,
`/proc/self/fd`, `/dev/fd`, or process-wide `chdir`.

The conditional candidate is `sqlite-vfs` version `0.2.0`. Dependency Advisor
approved that version under the conservative Rust policy with a 720-hour
minimum release age. It must not be added until the user authorizes the direct
dependency and a bounded implementation package proves compatibility with the
repository's bundled SQLCipher SQLite and all database/journal sidecars.

The no-file redesign is rejected. SQLCipher treats `:memory:`, URI
`mode=memory`, and the built-in `memdb` VFS as memory databases and disables
their page codec. `sqlite3_serialize` consequently returns plaintext logical
pages for those destinations, not an encrypted SQLCipher snapshot. Serializing
an on-disk keyed database is also not an encrypted-byte handoff guarantee.

## Evidence

- `Cargo.toml` pins `rusqlite` 0.40.2 with `backup`,
  `bundled-sqlcipher-vendored-openssl`, and `chrono`; it has no
  descriptor-open API.
- `rusqlite` 0.40.2 `Connection::open_with_flags` accepts `AsRef<Path>` and
  delegates to `sqlite3_open_v2` with a path.
- `rustix` supplies `openat`, but it cannot govern SQLite's later pathname
  opens for the main database, WAL, shared-memory, journal, or temporary files.
- `sqlite-vfs` 0.2.0 exposes a Rust VFS trait with explicit main-database,
  journal, WAL, and temporary-file open kinds. Its own README marks it as a
  prototype, not production-ready, and lists WAL and memory-mapping support as
  unavailable. It uses unreviewed unsafe Rust. A VFS implementation could bind
  names to the retained directory handle, but neither its sidecar behavior nor
  SQLCipher compatibility is established here.
- The bundled SQLCipher source sets the pager codec to null for memory
  databases; its built-in `memdb` VFS and URI memory mode both set
  `SQLITE_OPEN_MEMORY`. Its serialize implementation returns memory-store pages
  directly. Therefore an in-memory backup plus `serialize` cannot meet the
  encrypted-byte requirement.

## Required follow-on contract

Before #387 resumes, an authorized package must add only the approved VFS
dependency, register a private non-default VFS, and prove that every SQLite
open/delete/access request is mapped through the lifecycle-owned directory
handle. It must reject absolute paths, traversal, and unrecognized sidecar
names; pin ownership/identity; preserve redacted errors; and test Linux and
macOS-supported behavior without exposing a path, descriptor, cleanup handle,
or key to callers.

## Rollback

No runtime change exists to roll back. If this decision record is superseded,
revert this one documentation commit and keep #387 blocked until a replacement
decision proves an equivalent or stronger directory-handle boundary.

## Non-claims

This is a local/static decision only. It does not add a dependency, register a
VFS, create a backup artifact, prove SQLCipher compatibility, perform OAuth or
provider actions, deploy, or provide live-UAT evidence.
