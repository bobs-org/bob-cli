//! Shared `bob gmail` integration-test harness: an isolated vault plus
//! config builder and a fake gog executable.
//!
//! Pointing `BOB_GMAIL_GOG` at the fake replaces `gog` on PATH. Tests
//! never touch Google.

#![allow(dead_code)]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(crate) static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const FAKE_GOG_SRC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/gmail_support/fake_gog.py"
);

pub(crate) const ACCOUNT: &str = "you@example.com";
pub(crate) const FAKE_AUTH_CODE: &str = "fake/code+1";
pub(crate) const HOST: &str = "test-host";
pub(crate) const NOW: &str = "2026-11-05 12:00:00";

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

/// An isolated vault with a `gmail:` config section.
pub(crate) struct GmailEnv {
    dir: TempDir,
    vault: PathBuf,
    config: PathBuf,
}

impl GmailEnv {
    /// Create the vault and a config file with `gmail.account` set.
    pub(crate) fn new(prefix: &str) -> Self {
        let dir = TempDir::new(prefix);
        let vault = dir.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault dir");
        fs::create_dir_all(dir.path().join("home")).expect("create home dir");
        let config = dir.path().join("config.yml");
        fs::write(
            &config,
            format!("gmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n"),
        )
        .expect("write gmail config");
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
        fs::write(&self.config, text).expect("write gmail config");
    }

    /// A `bob` command isolated to this vault and config.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(BOB_BIN);
        let path = std::env::var_os("PATH");
        let tmpdir = std::env::var_os("TMPDIR");
        let ld_library_path = std::env::var_os("LD_LIBRARY_PATH");
        command.env_clear();
        if let Some(path) = path {
            command.env("PATH", path);
        }
        if let Some(tmpdir) = tmpdir {
            command.env("TMPDIR", tmpdir);
        }
        if let Some(ld_library_path) = ld_library_path {
            command.env("LD_LIBRARY_PATH", ld_library_path);
        }
        command
            .env("BOB_DIR", self.vault())
            .env("BOB_CONFIG_FILE", self.config())
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
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        command
    }

    /// Write the journal gitignore and gitattributes rules doctor checks.
    pub(crate) fn write_journal_rules(&self) {
        fs::write(
            self.vault.join(".gitignore"),
            ".bob/\n!.bob/\n!.bob/gmail/\n!/.bob/gmail/*.jsonl\n",
        )
        .expect("write gitignore");
        fs::write(
            self.vault.join(".gitattributes"),
            ".bob/gmail/*.jsonl merge=union\n",
        )
        .expect("write gitattributes");
    }
}

/// A fake `gog` executable that records argv and answers from a scenario.
pub(crate) struct FakeGog {
    path: PathBuf,
    log: PathBuf,
    state: PathBuf,
    home: PathBuf,
    stdin_log: PathBuf,
    scenario: String,
}

impl FakeGog {
    pub(crate) fn new(env: &GmailEnv, name: &str) -> Self {
        let root = env.root().join(name);
        fs::create_dir_all(&root).expect("create fake gog dir");
        let path = root.join("gog");
        fs::copy(FAKE_GOG_SRC, &path).expect("copy fake gog");
        chmod_exec(&path);
        let home = root.join("home");
        fs::create_dir_all(&home).expect("create fake home dir");
        Self {
            path,
            log: root.join("calls.jsonl"),
            state: root.join("state.json"),
            home,
            stdin_log: root.join("stdin.log"),
            scenario: "ok".to_string(),
        }
    }

    pub(crate) fn scenario(mut self, name: &str) -> Self {
        self.scenario = name.to_string();
        self
    }

    /// Replace the thread response and attachment API bodies with explicit,
    /// deterministic synthetic data for transaction tests.
    pub(crate) fn set_thread(
        &self,
        thread: serde_json::Value,
        attachments: serde_json::Value,
    ) {
        fs::write(
            &self.state,
            serde_json::to_vec(&serde_json::json!({
                "thread": thread,
                "attachments": attachments,
            }))
            .expect("serialize fake thread state"),
        )
        .expect("write fake thread state");
    }

    pub(crate) fn install<'a>(
        &self,
        command: &'a mut Command,
    ) -> &'a mut Command {
        command
            .env("BOB_GMAIL_GOG", &self.path)
            .env("BOB_GMAIL_FAKE_LOG", &self.log)
            .env("BOB_GMAIL_FAKE_STATE", &self.state)
            .env("BOB_GMAIL_FAKE_HOME", &self.home)
            .env("BOB_GMAIL_FAKE_STDIN_LOG", &self.stdin_log)
            .env("BOB_GMAIL_FAKE_SCENARIO", &self.scenario)
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

    pub(crate) fn stdin_payloads(&self) -> Vec<serde_json::Value> {
        let text = fs::read_to_string(&self.stdin_log).unwrap_or_default();
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("stdin payload is not JSON ({error}):\n{line}")
                })
            })
            .collect()
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

/// Run `bob gmail <args>` with the fake gog installed.
pub(crate) fn run_gmail(
    env: &GmailEnv,
    fake: &FakeGog,
    args: &[&str],
) -> Output {
    let mut command = env.command();
    fake.install(&mut command);
    command.arg("gmail");
    for arg in args {
        command.arg(arg);
    }
    command.output().expect("run bob gmail")
}

/// Run `bob gmail <args>` with piped stdin, treating stdin as a TTY.
pub(crate) fn run_gmail_stdin(
    env: &GmailEnv,
    fake: &FakeGog,
    args: &[&str],
    stdin_text: &str,
) -> Output {
    let mut command = env.command();
    fake.install(&mut command);
    command.arg("gmail");
    for arg in args {
        command.arg(arg);
    }
    command.env("BOB_GMAIL_ASSUME_TTY", "1");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bob gmail");
    let write_result = child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin_text.as_bytes());
    if let Err(error) = write_result {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "write stdin");
    }
    child.wait_with_output().expect("wait for bob gmail")
}

/// Run `bob gmail login` and paste a callback built from the printed auth URL.
pub(crate) fn run_gmail_paste(
    env: &GmailEnv,
    fake: &FakeGog,
    args: &[&str],
) -> Output {
    run_gmail_paste_script(env, fake, args, "{callback}\n")
}

/// Run `bob gmail login` with a stdin script.
pub(crate) fn run_gmail_paste_script(
    env: &GmailEnv,
    fake: &FakeGog,
    args: &[&str],
    script: &str,
) -> Output {
    let mut command = env.command();
    fake.install(&mut command);
    command.arg("gmail");
    for arg in args {
        command.arg(arg);
    }
    command.env("BOB_GMAIL_ASSUME_TTY", "1");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bob gmail");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stderr = child.stderr.take().expect("piped stderr");
    let watch = AuthWatch::spawn(stderr);

    let mut last_auth: Option<String> = None;
    for raw_line in script.split_inclusive('\n') {
        let has_placeholder = raw_line.contains("{callback}");
        let line = if has_placeholder {
            let auth_url = watch.wait_auth_url(last_auth.as_deref());
            last_auth = Some(auth_url.clone());
            fill_callback_placeholders(raw_line, &auth_url)
        } else {
            raw_line.to_string()
        };
        if let Err(error) = stdin.write_all(line.as_bytes()) {
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe,
                "write stdin"
            );
            break;
        }
        let _ = stdin.flush();
    }
    drop(stdin);

    let mut stdout = Vec::new();
    if let Some(mut handle) = child.stdout.take() {
        handle.read_to_end(&mut stdout).expect("read stdout");
    }
    let status = child.wait().expect("wait for bob gmail");
    let stderr = watch.finish();
    Output {
        status,
        stdout,
        stderr,
    }
}

const AUTH_WAIT: Duration = Duration::from_secs(30);

struct AuthInner {
    url: Option<String>,
    stderr: Vec<u8>,
    done: bool,
}

struct AuthWatch {
    inner: Arc<Mutex<AuthInner>>,
    cv: Arc<Condvar>,
    thread: Option<thread::JoinHandle<()>>,
}

impl AuthWatch {
    fn spawn(stderr: std::process::ChildStderr) -> Self {
        let inner = Arc::new(Mutex::new(AuthInner {
            url: None,
            stderr: Vec::new(),
            done: false,
        }));
        let cv = Arc::new(Condvar::new());
        let thread_inner = Arc::clone(&inner);
        let thread_cv = Arc::clone(&cv);
        let thread = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                let mut guard = thread_inner.lock().expect("auth watch");
                guard.stderr.extend_from_slice(line.as_bytes());
                guard.stderr.push(b'\n');
                if let Some(url) = google_auth_url_in(&line) {
                    guard.url = Some(url);
                }
                thread_cv.notify_all();
            }
            let mut guard = thread_inner.lock().expect("auth watch");
            guard.done = true;
            thread_cv.notify_all();
        });
        Self {
            inner,
            cv,
            thread: Some(thread),
        }
    }

    fn wait_auth_url(&self, previous: Option<&str>) -> String {
        let deadline = Instant::now() + AUTH_WAIT;
        let mut guard = self.inner.lock().expect("auth watch");
        loop {
            if let Some(url) = guard.url.clone()
                && previous.is_none_or(|prev| prev != url)
            {
                return url;
            }
            if guard.done {
                let stderr =
                    String::from_utf8_lossy(&guard.stderr).into_owned();
                panic!("process exited before a Google auth URL appeared; stderr:\n{stderr}");
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let stderr =
                    String::from_utf8_lossy(&guard.stderr).into_owned();
                panic!("timed out waiting for a Google auth URL; stderr:\n{stderr}");
            }
            let (next, timeout) = self
                .cv
                .wait_timeout(guard, remaining)
                .expect("auth watch wait");
            guard = next;
            if timeout.timed_out() && Instant::now() >= deadline {
                let stderr =
                    String::from_utf8_lossy(&guard.stderr).into_owned();
                panic!("timed out waiting for a Google auth URL; stderr:\n{stderr}");
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.inner.lock().expect("auth watch").stderr.clone()
    }
}

fn google_auth_url_in(line: &str) -> Option<String> {
    let trimmed = line.trim();
    trimmed
        .starts_with("https://accounts.google.com/o/oauth2/auth")
        .then(|| trimmed.to_string())
}

fn fill_callback_placeholders(line: &str, auth_url: &str) -> String {
    let redirect = query_param(auth_url, "redirect_uri")
        .unwrap_or_else(|| panic!("auth URL missing redirect_uri: {auth_url}"));
    let state = query_param(auth_url, "state")
        .unwrap_or_else(|| panic!("auth URL missing state: {auth_url}"));
    let callback = format!(
        "{redirect}?state={}&code={}",
        percent_encode(&state),
        percent_encode(FAKE_AUTH_CODE)
    );
    line.replace("{callback}", &callback)
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
        if name == key {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let raw = input.as_bytes();
    let mut bytes = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let Ok(value) = u8::from_str_radix(
                std::str::from_utf8(&raw[i + 1..i + 3]).unwrap_or(""),
                16,
            )
        {
            bytes.push(value);
            i += 3;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
