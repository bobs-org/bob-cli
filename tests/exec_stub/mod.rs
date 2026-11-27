//! Fork-safe executable-stub writers for integration tests (bead `bob-cli-5l`).
//!
//! Every integration-test helper that materializes an executable stub (fake
//! `gog`, fake browsers and adapters, token commands, editor scripts, git
//! hooks, ...) must route through this module. Writing or copying a stub in
//! place with `fs::write`/`fs::copy` keeps a writable descriptor open while
//! the file is filled, and `cargo test` runs every test in one process. A
//! child that another test thread forks inside that window inherits the
//! descriptor — `O_CLOEXEC` only drops it once the child reaches `execve` —
//! and until then every attempt to execute the stub fails with `ETXTBSY`
//! ("Text file busy", os error 26), even from an unrelated child such as
//! `bob` spawning a hook or `git` executing a hook. Writing the payload to a
//! scratch file that is never executed and letting a short-lived `cp` child
//! create the stub keeps the writable descriptor out of this process, so no
//! fork can capture it.
//!
//! This mirrors the `tests/cli/support.rs::write_executable` pattern that
//! bead `bob-cli-o` established for the CLI-side helpers, and the
//! `src/native/test_stub.rs::write_executable_stub` helper that covers the
//! lib tests. Integration-test binaries cannot use either of those (one
//! lives in a different test binary, the other is not public outside the
//! library), so this module carries the same pattern for every other
//! `tests/*.rs` binary and `tests/*_support` helper. Test-only code:
//! production paths must never call this.

#![allow(dead_code)]

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static STUB_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn scratch_payload_path(path: &Path) -> PathBuf {
    let file_name = path.file_name().unwrap_or_else(|| {
        panic!("stub path has no file name: {}", path.display())
    });
    let unique = STUB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".{unique}.payload"));
    path.with_file_name(name)
}

fn child_copy(src: &Path, path: &Path) {
    // Copy onto a fresh inode so rewriting a stub cannot disturb a copy
    // that is still executing.
    let _ = fs::remove_file(path);
    let output = Command::new("cp")
        .arg("--")
        .arg(src)
        .arg(path)
        .output()
        .unwrap_or_else(|error| {
            panic!("copy executable stub {}: {error}", path.display())
        });
    assert!(
        output.status.success(),
        "copy executable stub {}:\n{}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| {
                panic!("chmod stub {}: {error}", path.display())
            });
    }
}

/// Materialize `contents` at `path` as an executable stub without ever
/// holding a writable descriptor on the file the tests execute.
///
/// The payload lands on a uniquely named scratch sibling that is never
/// executed; a short-lived `cp` child copies it onto the stub path. The
/// parent directory must already exist.
pub(crate) fn write_executable_stub(path: &Path, contents: &str) {
    let payload = scratch_payload_path(path);
    fs::write(&payload, contents).unwrap_or_else(|error| {
        panic!("write stub payload {}: {error}", payload.display())
    });
    child_copy(&payload, path);
    fs::remove_file(&payload).unwrap_or_else(|error| {
        panic!("remove stub payload {}: {error}", payload.display())
    });
}

/// Copy `src` onto `path` as an executable stub without ever holding a
/// writable descriptor on the file the tests execute.
///
/// Unlike `fs::copy`, which opens the destination for writing in this
/// process, a short-lived `cp` child performs the copy. The parent
/// directory must already exist.
pub(crate) fn copy_executable_stub(src: &Path, path: &Path) {
    child_copy(src, path);
}
