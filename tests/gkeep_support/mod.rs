//! Shared `bob gkeep` integration-test harness: an isolated vault plus
//! config builder and the fake adapter.
//!
//! The fake adapter is an executable shell script speaking adapter
//! protocol v1; pointing `BOB_GKEEP_ADAPTER` at it replaces
//! `uv run --script …`, the same idea as `BOB_CLIPBOARD_CMD`. The
//! `adapter` phase extends this module; other phases put extra helpers
//! in their own test file.

#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(crate) static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");

pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(crate) fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{}-{}-{}-{}",
            prefix,
            std::process::id(),
            current_time_nanos(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("create temp dir {}: {error}", path.display())
        });
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn current_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}

/// An isolated vault with a `gkeep:` config section.
pub(crate) struct GkeepEnv {
    dir: TempDir,
    vault: PathBuf,
    config: PathBuf,
}

impl GkeepEnv {
    /// Create the vault and a config file with `gkeep.email` set.
    pub(crate) fn new(prefix: &str) -> Self {
        let dir = TempDir::new(prefix);
        let vault = dir.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault dir");
        let config = dir.path().join("config.yml");
        fs::write(&config, "gkeep:\n  email: bryanbugyi34@gmail.com\n")
            .expect("write gkeep config");
        Self { dir, vault, config }
    }

    /// The vault root.
    pub(crate) fn vault(&self) -> &Path {
        &self.vault
    }

    /// The config file path.
    pub(crate) fn config(&self) -> &Path {
        &self.config
    }

    /// Overwrite the config file.
    pub(crate) fn write_config(&self, text: &str) {
        fs::write(&self.config, text).expect("write gkeep config");
    }

    /// A `bob` command isolated to this vault and config, with the
    /// adapter override removed unless a test sets it.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(BOB_BIN);
        command
            .env("BOB_DIR", self.vault())
            .env("BOB_CONFIG_FILE", self.config())
            .env_remove("BOB_GKEEP_ADAPTER")
            .env(
                "BOB_VAULT_SYNC_LOCK_FILE",
                self.dir.path().join("bob_sync.lock"),
            )
            .env("XDG_STATE_HOME", self.dir.path().join("state"));
        command
    }

    /// Write the fake adapter and return its path; the caller opts in
    /// with `.env("BOB_GKEEP_ADAPTER", path)`.
    pub(crate) fn write_fake_adapter(&self) -> PathBuf {
        let path = self.dir.path().join("fake-gkeep-adapter.sh");
        fs::write(
            &path,
            r#"#!/bin/sh
# Fake gkeep adapter: one JSON request on stdin, one JSON response on
# stdout. Answers ping; every other op is a protocol error.
request=$(cat)
case "$request" in
  *'"op":"ping"'*|*'"op": "ping"'*)
    printf '{"ok":true,"protocol":1,"python":"3.12.3","gkeepapi":"0.17.1","gpsoauth":"2.0.0"}'
    ;;
  *)
    printf '{"ok":false,"error":{"kind":"protocol","message":"fake adapter: unsupported op"}}'
    ;;
esac
"#,
        )
        .expect("write fake adapter");
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&path)
                .expect("stat fake adapter")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions)
                .expect("chmod fake adapter");
        }
        path
    }
}

pub(crate) fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
