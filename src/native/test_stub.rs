//! Fork-safe executable-stub writer for lib tests (bead `bob-cli-5l`).
//!
//! Every lib-test helper that materializes an executable stub (fake curl,
//! fake listen renderers, fake clip adapters, hook scripts, ...) must route
//! through [`write_executable_stub`]. Writing a stub in place with
//! `fs::write` keeps a writable descriptor open while the file is filled,
//! and `cargo test` runs every test in this one process. A child that
//! another test thread forks inside that window inherits the descriptor —
//! `O_CLOEXEC` only drops it once the child reaches `execve` — and until
//! then every attempt to execute the stub fails with `ETXTBSY` ("Text file
//! busy", os error 26), even from an unrelated child such as `bob`
//! spawning its `ob` shim. Writing the payload to a scratch file that is
//! never executed and letting a short-lived `cp` child create the stub
//! keeps the writable descriptor out of this process, so no fork can
//! capture it.
//!
//! This mirrors the `tests/cli/support.rs::write_executable` pattern that
//! bead `bob-cli-o` established for the CLI-side helpers. Test-only code:
//! production paths must never call this.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static STUB_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Materialize `contents` at `path` as an executable stub without ever
/// holding a writable descriptor on the file the tests execute.
///
/// The payload lands on a uniquely named scratch sibling that is never
/// executed; a short-lived `cp` child copies it onto a fresh inode at
/// `path` (any previous stub is removed first so a rewrite cannot disturb
/// a copy that is still executing). The scratch file is removed before
/// returning, and the stub is left mode `0o755` on unix.
///
/// The parent directory must already exist, except callers that create it
/// keep doing so before calling this.
pub(crate) fn write_executable_stub(path: &Path, contents: &str) {
    let unique = STUB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path.file_name().unwrap_or_else(|| {
        panic!("stub path has no file name: {}", path.display())
    });
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".{unique}.payload"));
    let payload: PathBuf = path.with_file_name(name);
    fs::write(&payload, contents).unwrap_or_else(|error| {
        panic!("write stub payload {}: {error}", payload.display())
    });
    // Copy onto a fresh inode so rewriting a stub cannot disturb a copy
    // that is still executing.
    let _ = fs::remove_file(path);
    let output = Command::new("cp")
        .arg("--")
        .arg(&payload)
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
    fs::remove_file(&payload).unwrap_or_else(|error| {
        panic!("remove stub payload {}: {error}", payload.display())
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| {
                panic!("chmod stub {}: {error}", path.display())
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsString,
        sync::atomic::{AtomicBool, Ordering},
        sync::Arc,
        thread,
    };

    #[cfg(unix)]
    #[test]
    fn executable_stubs_stay_executable_while_other_threads_fork() {
        // Guards the `Text file busy` flake (bead `bob-cli-5l`): a stub
        // must never be written through a descriptor this process holds,
        // because any child forked during the write inherits it and makes
        // `execve` of the stub fail with ETXTBSY until that child execs.
        // Ports `tests/cli/vault_sync.rs`'s `bob-cli-o` regression for the
        // shared lib helper every lib-test stub writer routes through.
        let dir = std::env::temp_dir().join(format!(
            "bob-lib-etxtbsy-stub-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(&dir).expect("create stub test dir");
        let stub = dir.join("stub");
        let padding = "#".repeat(256 * 1024);
        let payload = format!("#!/bin/sh\nexit 7\n{padding}");

        let stop = Arc::new(AtomicBool::new(false));
        let lurkers: Vec<_> = (0..4)
            .map(|_| {
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = Command::new("true").output();
                    }
                })
            })
            .collect();

        for iteration in 0..60 {
            write_executable_stub(&stub, &payload);
            if iteration == 0 {
                assert_eq!(
                    fs::read_to_string(&stub).expect("read stub"),
                    payload,
                    "stub contents must match the payload"
                );
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = fs::metadata(&stub)
                        .expect("stat stub")
                        .permissions()
                        .mode()
                        & 0o777;
                    assert_eq!(mode, 0o755, "unexpected stub mode");
                }
                let leftovers: Vec<_> = fs::read_dir(&dir)
                    .expect("read stub dir")
                    .map(|entry| entry.expect("dir entry").file_name())
                    .collect();
                assert_eq!(
                    leftovers,
                    [OsString::from("stub")],
                    "stub directory must contain only the stub, no leftover payload"
                );
            }
            match Command::new(&stub).output() {
                Err(error) if error.raw_os_error() == Some(26) => {
                    panic!(
                        "Text file busy (ETXTBSY, os error 26) executing stub {} — \
                         write_executable_stub leaked a writable descriptor (bead bob-cli-5l)",
                        stub.display()
                    );
                }
                Err(error) => {
                    panic!("execute stub {}: {error}", stub.display());
                }
                Ok(output) => {
                    assert_eq!(
                        output.status.code(),
                        Some(7),
                        "stub should exit 7:\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
        }

        stop.store(true, Ordering::Relaxed);
        for lurker in lurkers {
            lurker.join().expect("lurker thread");
        }
        fs::remove_dir_all(&dir).expect("remove stub test dir");
    }
}
