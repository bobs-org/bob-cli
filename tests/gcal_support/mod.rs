//! Shared `bob gcal` integration-test harness: an isolated vault plus
//! config builder and a fake gog executable.
//!
//! Pointing `BOB_GCAL_GOG` at the fake replaces `gog` on PATH. Tests
//! never touch Google. `BOB_GCAL_GOG` is removed by default so a missing
//! override is a real `gog_missing` failure.

#![allow(dead_code)]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
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
    "/tests/gcal_support/fake_gog.py"
);
const FAKE_BROWSER_SRC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/gcal_support/fake_browser.py"
);

pub(crate) const ACCOUNT: &str = "you@example.com";
pub(crate) const FAKE_AUTH_CODE: &str = "fake/code+1";
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
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_TTY")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("BOB_GCAL_BROWSER")
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
    event_dir: PathBuf,
    home: PathBuf,
    stdin_log: PathBuf,
    scenario: String,
}

impl FakeGog {
    pub(crate) fn new(env: &GcalEnv, name: &str) -> Self {
        let root = env.root().join(name);
        fs::create_dir_all(&root).expect("create fake gog dir");
        let path = root.join("gog");
        fs::copy(FAKE_GOG_SRC, &path).expect("copy fake gog");
        chmod_exec(&path);
        let event_dir = root.join("event-lookup");
        fs::create_dir_all(&event_dir).expect("create event lookup dir");
        let home = root.join("home");
        fs::create_dir_all(&home).expect("create fake home dir");
        Self {
            path,
            log: root.join("calls.jsonl"),
            state: root.join("state.json"),
            events: root.join("events.json"),
            event_dir,
            stdin_log: root.join("stdin.log"),
            home,
            scenario: "ok".to_string(),
        }
    }

    pub(crate) fn home(&self) -> &Path {
        &self.home
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

    /// Answer one `calendar event <calendar> <id>` lookup with this raw
    /// response body (a `{"event": {...}}` envelope, or
    /// `{"_fail": {"stderr": ..., "code": ...}}`).
    pub(crate) fn event_lookup(&self, event_id: &str, body: &str) {
        fs::write(self.event_dir.join(format!("{event_id}.json")), body)
            .expect("write fake event lookup");
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
            .env("BOB_GCAL_FAKE_EVENT_DIR", &self.event_dir)
            .env("BOB_GCAL_FAKE_HOME", &self.home)
            .env("BOB_GCAL_FAKE_STDIN_LOG", &self.stdin_log)
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

/// A fake `$BOB_GCAL_BROWSER` that GETs the loopback callback.
pub(crate) struct FakeBrowser {
    path: PathBuf,
    out: PathBuf,
}

impl FakeBrowser {
    pub(crate) fn new(env: &GcalEnv) -> Self {
        let root = env.root().join("browser");
        fs::create_dir_all(&root).expect("create fake browser dir");
        let path = root.join("browser");
        fs::copy(FAKE_BROWSER_SRC, &path).expect("copy fake browser");
        chmod_exec(&path);
        Self {
            path,
            out: root.join("page.html"),
        }
    }

    pub(crate) fn install<'a>(
        &self,
        command: &'a mut Command,
    ) -> &'a mut Command {
        command
            .env("BOB_GCAL_BROWSER", &self.path)
            .env("BOB_GCAL_FAKE_BROWSER_OUT", &self.out)
            .env("BOB_GCAL_CONSENT_SECS", "30")
    }

    pub(crate) fn deny<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        self.install(command)
            .env("BOB_GCAL_FAKE_BROWSER_ERROR", "access_denied")
    }

    /// Deny the first browser visit, then succeed on the next.
    pub(crate) fn deny_once<'a>(
        &self,
        command: &'a mut Command,
    ) -> &'a mut Command {
        self.install(command).env(
            "BOB_GCAL_FAKE_BROWSER_DENY_ONCE",
            self.out.with_extension("deny"),
        )
    }

    pub(crate) fn page(&self) -> String {
        fs::read_to_string(&self.out).unwrap_or_default()
    }
}

/// Run `bob gcal login` and paste a callback built from the printed auth URL.
pub(crate) fn run_gcal_paste(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
) -> Output {
    run_gcal_paste_script(env, fake, args, "{callback}\n")
}

/// Run `bob gcal login` with a stdin script. `{callback}`, `{callback_encoded}`,
/// and `{denied}` are filled from the latest printed Google auth URL.
pub(crate) fn run_gcal_paste_script(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    script: &str,
) -> Output {
    run_gcal_paste_script_with(env, fake, args, script, |_| {})
}

/// Like [`run_gcal_paste_script`], with extra command configuration.
pub(crate) fn run_gcal_paste_script_with(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    script: &str,
    configure: impl FnOnce(&mut Command),
) -> Output {
    let mut command = env.command();
    fake.install(&mut command);
    command.arg("gcal");
    for arg in args {
        command.arg(arg);
    }
    command.env("BOB_GCAL_ASSUME_TTY", "1");
    configure(&mut command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bob gcal");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stderr = child.stderr.take().expect("piped stderr");
    let watch = AuthWatch::spawn(stderr);

    let mut last_auth: Option<String> = None;
    for raw_line in script.split_inclusive('\n') {
        let has_placeholder = raw_line.contains("{callback}")
            || raw_line.contains("{callback_encoded}")
            || raw_line.contains("{denied}");
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
    let status = child.wait().expect("wait for bob gcal");
    let stderr = watch.finish();
    Output {
        status,
        stdout,
        stderr,
    }
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
    let denied = format!(
        "{redirect}?state={}&error=access_denied",
        percent_encode(&state)
    );
    line.replace("{callback}", &callback)
        .replace("{callback_encoded}", &callback)
        .replace("{denied}", &denied)
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

/// GET the loopback callback printed in `auth_url` with an encoded code.
pub(crate) fn hit_oauth_callback(auth_url: &str) {
    let redirect = query_param(auth_url, "redirect_uri")
        .unwrap_or_else(|| panic!("auth URL missing redirect_uri: {auth_url}"));
    let state = query_param(auth_url, "state")
        .unwrap_or_else(|| panic!("auth URL missing state: {auth_url}"));
    let rest = redirect
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("redirect is not http: {redirect}"));
    let (host_port, path) = rest
        .split_once('/')
        .map(|(host, path)| (host, format!("/{path}")))
        .unwrap_or((rest, "/".to_string()));
    let (host, port) = host_port
        .rsplit_once(':')
        .and_then(|(host, port)| {
            port.parse::<u16>().ok().map(|port| (host, port))
        })
        .unwrap_or_else(|| panic!("redirect missing port: {redirect}"));
    let query = format!(
        "state={}&code={}",
        percent_encode(&state),
        percent_encode(FAKE_AUTH_CODE)
    );
    let request = format!(
        "GET {path}?{query} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_error = None;
    while Instant::now() < deadline {
        match TcpStream::connect((host, port)) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                stream
                    .write_all(request.as_bytes())
                    .expect("write callback");
                let mut buf = Vec::new();
                let _ = stream.read_to_end(&mut buf);
                return;
            }
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
    panic!(
        "could not reach loopback callback {redirect}: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "no connection".to_string())
    );
}

/// Live stderr watcher that extracts the printed Google auth URL.
pub(crate) struct PrintedAuth {
    pub url: String,
    watch: AuthWatch,
}

impl PrintedAuth {
    /// Remaining stderr after the child exits.
    pub(crate) fn finish(self) -> Vec<u8> {
        self.watch.finish()
    }
}

/// Wait until stderr contains a Google auth URL.
pub(crate) fn wait_printed_auth_url(
    stderr: std::process::ChildStderr,
) -> PrintedAuth {
    let watch = AuthWatch::spawn(stderr);
    let url = watch.wait_auth_url(None);
    PrintedAuth { url, watch }
}
