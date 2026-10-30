//! Shared `bob gcal` integration-test harness: an isolated vault plus
//! config builder and a fake gog executable.
//!
//! Pointing `BOB_GCAL_GOG` at the fake replaces `gog` on PATH. Tests
//! never touch Google. `BOB_GCAL_GOG` is removed by default so a missing
//! override is a real `gog_missing` failure.

#![allow(dead_code)]

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(crate) static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const FAKE_GOG_SRC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/gcal_support/fake_gog.py"
);

pub(crate) const ACCOUNT: &str = "you@example.com";
pub(crate) const HOST: &str = "test-host";
pub(crate) const NOW: &str = "2026-10-30 12:00:00";

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

/// An isolated vault with a `gcal:` config section.
pub(crate) struct GcalEnv {
    dir: TempDir,
    vault: PathBuf,
    config: PathBuf,
}

impl GcalEnv {
    /// Create the vault and a config file with `gcal.account` set.
    pub(crate) fn new(prefix: &str) -> Self {
        let dir = TempDir::new(prefix);
        let vault = dir.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault dir");
        fs::create_dir_all(dir.path().join("home")).expect("create home dir");
        let config = dir.path().join("config.yml");
        fs::write(
            &config,
            format!("gcal:\n  account: {ACCOUNT}\n  writer: {HOST}\n"),
        )
        .expect("write gcal config");
        Self { dir, vault, config }
    }

    pub(crate) fn vault(&self) -> &Path {
        &self.vault
    }

    pub(crate) fn config(&self) -> &Path {
        &self.config
    }

    pub(crate) fn root(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn write_config(&self, text: &str) {
        fs::write(&self.config, text).expect("write gcal config");
    }

    /// A `bob` command isolated to this vault and config.
    ///
    /// `BOB_GCAL_GOG` is removed unless a test installs [`FakeGog`].
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(BOB_BIN);
        command
            .env("BOB_DIR", self.vault())
            .env("BOB_CONFIG_FILE", self.config())
            .env_remove("BOB_GCAL_GOG")
            .env_remove("BOB_GCAL_ASSUME_TTY")
            .env("HOSTNAME", HOST)
            .env("BOB_NOW", NOW)
            .env("NO_COLOR", "1")
            .env(
                "BOB_VAULT_SYNC_LOCK_FILE",
                self.dir.path().join("bob_sync.lock"),
            )
            .env(
                "BOB_VAULT_SYNC_STATE_FILE",
                self.dir.path().join("vault-sync.json"),
            )
            .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .env("HOME", self.dir.path().join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("BOB_HIGHLIGHTS_RESOLVE", "*=203.0.113.1")
            .env(
                "BOB_WEB_CLIP_ADAPTER",
                "/definitely/missing/bob-cli-test-web-clip-adapter",
            )
            .env("BOB_HIGHLIGHTS_CURL", "/bin/false");
        command
    }

    /// Write `meet.md` as an area so doctor can validate the default root.
    pub(crate) fn write_meet_area(&self) {
        fs::write(
            self.vault.join("meet.md"),
            "---\ntype: area\ncolor: violet\n---\n# Meet\n",
        )
        .expect("write meet.md");
    }

    /// Write today's daily note with a Pomodoros heading.
    pub(crate) fn write_today(&self) {
        let day = self.vault.join("2026");
        fs::create_dir_all(&day).expect("create 2026 dir");
        fs::write(
            day.join("20261030.md"),
            "# Friday 2026-10-30\n\n## Pomodoros\n",
        )
        .expect("write daily note");
    }

    /// Write the journal gitignore and gitattributes rules doctor checks.
    pub(crate) fn write_journal_rules(&self) {
        fs::write(
            self.vault.join(".gitignore"),
            ".bob/\n!.bob/\n!.bob/gcal/\n!/.bob/gcal/*.jsonl\n",
        )
        .expect("write gitignore");
        fs::write(
            self.vault.join(".gitattributes"),
            ".bob/gcal/*.jsonl merge=union\n",
        )
        .expect("write gitattributes");
    }

    pub(crate) fn prepare_healthy_vault(&self) {
        self.write_meet_area();
        self.write_today();
        self.write_journal_rules();
    }
}

/// A fake `gog` executable that records argv and answers from a scenario.
pub(crate) struct FakeGog {
    path: PathBuf,
    log: PathBuf,
    state: PathBuf,
    events: PathBuf,
    scenario: String,
}

impl FakeGog {
    pub(crate) fn new(env: &GcalEnv, name: &str) -> Self {
        let root = env.root().join(name);
        fs::create_dir_all(&root).expect("create fake gog dir");
        let path = root.join("gog");
        fs::copy(FAKE_GOG_SRC, &path).expect("copy fake gog");
        chmod_exec(&path);
        Self {
            path,
            log: root.join("calls.jsonl"),
            state: root.join("state.json"),
            events: root.join("events.json"),
            scenario: "ok".to_string(),
        }
    }

    pub(crate) fn scenario(mut self, name: &str) -> Self {
        self.scenario = name.to_string();
        self
    }

    pub(crate) fn events_json(&self, text: &str) {
        fs::write(&self.events, text).expect("write fake event snapshot");
    }

    pub(crate) fn install<'a>(
        &self,
        command: &'a mut Command,
    ) -> &'a mut Command {
        command
            .env("BOB_GCAL_GOG", &self.path)
            .env("BOB_GCAL_FAKE_LOG", &self.log)
            .env("BOB_GCAL_FAKE_STATE", &self.state)
            .env("BOB_GCAL_FAKE_EVENTS", &self.events)
            .env("BOB_GCAL_FAKE_SCENARIO", &self.scenario)
    }

    pub(crate) fn calls(&self) -> Vec<Vec<String>> {
        let text = fs::read_to_string(&self.log).unwrap_or_default();
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("parse argv log {line}: {error}")
                })
            })
            .collect()
    }

    pub(crate) fn called_with(&self, needles: &[&str]) -> bool {
        self.calls().iter().any(|call| {
            needles
                .iter()
                .all(|needle| call.iter().any(|arg| arg == needle))
        })
    }
}

fn chmod_exec(path: &Path) {
    #[cfg(unix)]
    {
        let mut permissions =
            fs::metadata(path).expect("stat fake gog").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod fake gog");
    }
}

pub(crate) fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub(crate) fn json_stdout(output: &Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("stdout is not JSON ({error}):\n{}", stdout(output))
    })
}

/// Run `bob gcal <args>` with the fake gog installed.
pub(crate) fn run_gcal(env: &GcalEnv, fake: &FakeGog, args: &[&str]) -> Output {
    let mut command = env.command();
    fake.install(&mut command);
    command.arg("gcal");
    for arg in args {
        command.arg(arg);
    }
    command.output().expect("run bob gcal")
}

/// Run `bob gcal <args>` with piped stdin, treating stdin as a TTY.
pub(crate) fn run_gcal_stdin(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    stdin_text: &str,
) -> Output {
    run_gcal_stdin_inner(env, fake, args, stdin_text, true)
}

/// Run `bob gcal <args>` with piped stdin (login reuse / logout confirm).
fn run_gcal_stdin_inner(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    stdin_text: &str,
    assume_tty: bool,
) -> Output {
    let mut command = env.command();
    fake.install(&mut command);
    command.arg("gcal");
    for arg in args {
        command.arg(arg);
    }
    if assume_tty {
        command.env("BOB_GCAL_ASSUME_TTY", "1");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bob gcal");
    let write_result = child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin_text.as_bytes());
    if let Err(error) = write_result {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "write stdin");
    }
    child.wait_with_output().expect("wait for bob gcal")
}
