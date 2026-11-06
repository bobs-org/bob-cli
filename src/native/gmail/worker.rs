//! Host-local queue and detached, single-flight Gmail link worker.

use super::config::GmailConfig;
use crate::native::env as bob_env;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const WORKER_ENV: &str = "BOB_GMAIL_LINK_WORKER";

fn root() -> PathBuf {
    bob_env::bob_cli_state_dir().join("gmail")
}

// A non-secret local receipt lets capture check login without invoking gog
// or probing a keychain. Login refreshes it and logout removes it. It is
// a hint only: the worker still checks the actual grant.
pub(super) fn remember_login(account: &str) {
    let directory = root().join("accounts");
    if fs::create_dir_all(&directory).is_ok() {
        let _ = fs::write(
            directory.join(super::save::sha256(
                account.to_ascii_lowercase().as_bytes(),
            )),
            account.to_ascii_lowercase(),
        );
    }
}

pub(super) fn forget_login(account: &str) {
    let _ =
        fs::remove_file(root().join("accounts").join(super::save::sha256(
            account.to_ascii_lowercase().as_bytes(),
        )));
}

pub(super) fn login_account() -> Option<String> {
    let mut accounts: Vec<_> = fs::read_dir(root().join("accounts"))
        .ok()?
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .filter(|value| value.contains('@'))
        .collect();
    accounts.sort();
    accounts.dedup();
    (accounts.len() == 1).then(|| accounts.remove(0))
}

fn eligible(config: &GmailConfig) -> bool {
    if config.writer().is_some_and(|writer| {
        writer != crate::native::vault_sync::hostname_slug()
    }) {
        return false;
    }
    match config.account() {
        Some(account) => root()
            .join("accounts")
            .join(super::save::sha256(account.to_ascii_lowercase().as_bytes()))
            .is_file(),
        None => login_account().is_some(),
    }
}

#[derive(Serialize, Deserialize)]
struct Job {
    vault: PathBuf,
    notes: Vec<PathBuf>,
}

pub(crate) fn kick(vault: &Path, notes: &[PathBuf]) -> Result<(), String> {
    if matches!(
        bob_env::var("BOB_GMAIL_KICK").ok().as_deref(),
        Some("off" | "0" | "false")
    ) {
        return Ok(());
    }
    let Ok(config) = GmailConfig::resolve() else {
        return Ok(());
    };
    if !eligible(&config) || notes.is_empty() {
        return Ok(());
    }
    let exe = std::env::current_exe()
        .map_err(|_| "could not locate Gmail worker".to_string())?;
    kick_with(&exe, vault, notes)
        .map_err(|_| "could not start Gmail worker; URLs kept".to_string())
}

fn kick_with(exe: &Path, vault: &Path, notes: &[PathBuf]) -> io::Result<()> {
    let root = root();
    enqueue(&root, vault, notes)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let out = options.open(root.join("worker.log"))?;
    let err = out.try_clone()?;
    let mut command = Command::new(exe);
    bob_env::inherit_overrides(&mut command);
    command
        .args(["gmail", "link", "-q", "-b"])
        .arg(vault)
        .arg("--")
        .args(notes)
        .env(WORKER_ENV, "1")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command.spawn()?;
    Ok(())
}

fn enqueue(root: &Path, vault: &Path, notes: &[PathBuf]) -> io::Result<()> {
    fs::create_dir_all(root.join("pending"))?;
    let job = Job {
        vault: fs::canonicalize(vault)?,
        notes: notes
            .iter()
            .map(fs::canonicalize)
            .collect::<Result<_, _>>()?,
    };
    // create_new prevents collisions even within one timestamp. Rename
    // publishes a complete job, so a live worker never reads partial JSON.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = root
        .join("pending")
        .join(format!("{}-{stamp}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    serde_json::to_writer(&file, &job)?;
    file.sync_all()?;
    fs::rename(&path, path.with_extension("json"))
}

pub(super) fn is_worker() -> bool {
    bob_env::var(WORKER_ENV).ok().as_deref() == Some("1")
}

pub(super) fn run() -> i32 {
    drain(&root(), |job| {
        let args = super::cli::LinkArgs {
            all: false,
            bob_dir: Some(job.vault.clone()),
            dry_run: false,
            format: super::cli::HumanFormat::Human,
            notes: job
                .notes
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            quiet: true,
        };
        super::link::run_direct(&args)
    })
    .unwrap_or(1)
}

fn pending(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(root.join("pending"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

fn drain(root: &Path, mut process: impl FnMut(&Job) -> i32) -> io::Result<i32> {
    fs::create_dir_all(root)?;
    let mut code = 0;
    loop {
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("worker.lock"))?;
        match lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Ok(code)
            }
            Err(error) => return Err(error),
        }
        trim_log(root);
        loop {
            let files = pending(root);
            if files.is_empty() {
                break;
            }
            for path in files {
                if let Ok(job) = fs::read(&path).and_then(|bytes| {
                    serde_json::from_slice(&bytes).map_err(io::Error::other)
                }) {
                    code = code.max(process(&job));
                } else {
                    code = 1;
                }
                // URL remains in its note on failure; explicit sweeps retry.
                fs::remove_file(path)?;
            }
        }
        drop(lock);
        // A kick that lost the lock after the last scan must not strand work.
        if pending(root).is_empty() {
            return Ok(code);
        }
    }
}

fn trim_log(root: &Path) {
    let path = root.join("worker.log");
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > 1024 * 1024)
        && let Ok(bytes) = fs::read(&path)
    {
        let start = bytes.len().saturating_sub(256 * 1024);
        let start = bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(start, |offset| start + offset + 1);
        let _ = fs::write(path, &bytes[start..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::env::TestEnvGuard;

    #[test]
    fn gmail_queue_drains_arrivals_mid_run_and_excludes_another_worker() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        let note = temp.path().join("note.md");
        fs::write(&note, "synthetic task").unwrap();
        enqueue(&root, temp.path(), std::slice::from_ref(&note)).unwrap();
        let mut count = 0;
        assert_eq!(
            drain(&root, |job| {
                count += 1;
                assert_eq!(job.notes, [note.clone()]);
                assert_eq!(
                    drain(&root, |_| panic!("second worker acquired lock"))
                        .unwrap(),
                    0
                );
                if count == 1 {
                    enqueue(&root, temp.path(), std::slice::from_ref(&note))
                        .unwrap();
                }
                0
            })
            .unwrap(),
            0
        );
        assert_eq!(count, 2);
        assert!(pending(&root).is_empty());
    }

    #[test]
    fn gmail_worker_disable_writer_and_login_gates_are_offline() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.yml");
        let _env = TestEnvGuard::set(&[
            ("XDG_STATE_HOME", Some(temp.path().as_os_str())),
            ("BOB_CONFIG_FILE", Some(config.as_os_str())),
        ]);
        fs::write(&config, "gmail:\n  account: me@example.com\n").unwrap();
        assert!(!eligible(&GmailConfig::resolve().unwrap()));
        remember_login("ME@example.com");
        assert_eq!(login_account().as_deref(), Some("me@example.com"));
        assert!(eligible(&GmailConfig::resolve().unwrap()));
        for disabled in ["off", "0", "false"] {
            let _env = TestEnvGuard::set(&[(
                "BOB_GMAIL_KICK",
                Some(std::ffi::OsStr::new(disabled)),
            )]);
            kick(temp.path(), &[temp.path().join("missing.md")]).unwrap();
            assert!(!root().join("pending").exists());
        }
        fs::write(&config, "gmail:\n  account: me@example.com\n  writer: another-host-that-is-not-this-one\n").unwrap();
        assert!(!eligible(&GmailConfig::resolve().unwrap()));
        forget_login("me@example.com");
        assert!(login_account().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn gmail_kick_spawns_scoped_command_and_keeps_job_on_spawn_failure() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::set(&[(
            "XDG_STATE_HOME",
            Some(temp.path().as_os_str()),
        )]);
        let note = temp.path().join("a note.md");
        fs::write(&note, "synthetic task").unwrap();
        let exe = temp.path().join("fake-bob");
        let args = temp.path().join("args");
        let _args_env =
            TestEnvGuard::set(&[("GMAIL_TEST_ARGS", Some(args.as_os_str()))]);
        fs::write(&exe, "#!/bin/sh\nprintf '%s\\n' \"$BOB_GMAIL_LINK_WORKER\" \"$@\" > \"$GMAIL_TEST_ARGS\"\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        kick_with(&exe, temp.path(), std::slice::from_ref(&note)).unwrap();
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(5);
        let expected = format!(
            "1\ngmail\nlink\n-q\n-b\n{}\n--\n{}\n",
            temp.path().display(),
            note.display()
        );
        while fs::read_to_string(&args).unwrap_or_default() != expected {
            assert!(
                std::time::Instant::now() < deadline,
                "detached worker argv"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(pending(&root()).len(), 1);
        let job: Job =
            serde_json::from_slice(&fs::read(&pending(&root())[0]).unwrap())
                .unwrap();
        assert_eq!(job.notes, [note.clone()]);
        assert!(kick_with(
            &temp.path().join("missing-bob"),
            temp.path(),
            &[note]
        )
        .is_err());
        assert_eq!(pending(&root()).len(), 2);
    }
}
