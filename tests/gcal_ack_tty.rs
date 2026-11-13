//! `bob gcal ack` terminal session under a real PTY: raw/cooked transitions
//! and terminal restoration after quit, Ctrl-C, and failures.
//!
//! Piped-input tests live in `gcal_ack.rs`; only a real terminal exercises
//! termios, so this binary forks and `exec`s `bob` onto a PTY slave.

#![cfg(unix)]

mod gcal_support;

use gcal_support::{FakeGog, GcalEnv, BOB_BIN};
use serde_json::json;
use std::{
    ffi::{CStr, CString},
    fs,
    io::Write as _,
    os::fd::{AsRawFd as _, FromRawFd as _},
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(30);

fn prepare(env: &GcalEnv) {
    env.prepare_healthy_vault();
    env.write_config(
        "timezone: America/New_York\ngcal:\n  account: you@example.com\n  writer: test-host\n",
    );
}

fn session_events() -> String {
    json!({"events": [
        {
            "id": "task-1", "summary": "Budget review", "status": "confirmed",
            "start": {"dateTime": "2026-10-31T13:00:00-04:00"},
            "end": {"dateTime": "2026-10-31T13:30:00-04:00"},
            "htmlLink": "https://calendar.google.com/event?eid=task-1",
            "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
            "attendees": [{"self": true, "responseStatus": "accepted"}],
        },
        {
            "id": "task-2", "summary": "Hiring sync", "status": "confirmed",
            "start": {"dateTime": "2026-11-02T13:00:00-05:00"},
            "end": {"dateTime": "2026-11-02T13:30:00-05:00"},
            "htmlLink": "https://calendar.google.com/event?eid=task-2",
            "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
            "attendees": [{"self": true, "responseStatus": "accepted"}],
        },
    ], "nextPageToken": null})
    .to_string()
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct TermAttrs {
    iflag: u64,
    oflag: u64,
    cflag: u64,
    lflag: u64,
    cc: Vec<u8>,
}

fn attrs_of(fd: i32) -> TermAttrs {
    // SAFETY: tcgetattr fully writes `raw` on success, which is asserted.
    unsafe {
        let mut raw: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(fd, &mut raw), 0, "tcgetattr");
        TermAttrs {
            iflag: raw.c_iflag as u64,
            oflag: raw.c_oflag as u64,
            cflag: raw.c_cflag as u64,
            lflag: raw.c_lflag as u64,
            cc: raw.c_cc.to_vec(),
        }
    }
}

fn open_master() -> (fs::File, String) {
    // SAFETY: the master fd is owned by the returned File; the slave path
    // is a fresh NUL-terminated buffer filled by ptsname_r.
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(fd >= 0, "posix_openpt");
        assert_eq!(libc::grantpt(fd), 0, "grantpt");
        assert_eq!(libc::unlockpt(fd), 0, "unlockpt");
        let mut name = vec![0 as libc::c_char; 128];
        assert_eq!(
            libc::ptsname_r(fd, name.as_mut_ptr(), name.len()),
            0,
            "ptsname_r"
        );
        let path = CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
        (fs::File::from_raw_fd(fd), path)
    }
}

struct PtyChild {
    master: fs::File,
    pid: libc::pid_t,
    output: Vec<u8>,
    before: TermAttrs,
    reaped: bool,
}

impl PtyChild {
    fn fd(&self) -> i32 {
        self.master.as_raw_fd()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output).replace("\r\n", "\n")
    }

    /// Poll for output, reading what is available. False means the child
    /// is gone (EOF or EIO on the master side).
    fn pump(&mut self, wait_ms: i32) -> bool {
        let mut waiting = libc::pollfd {
            fd: self.fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: polls the single master fd with no pointer lifetime.
        let ready = unsafe { libc::poll(&mut waiting, 1, wait_ms) };
        if ready <= 0 {
            return true;
        }
        let mut buf = [0u8; 4096];
        // SAFETY: `buf` is a live stack array; the kernel writes at most
        // its length on success.
        let read = unsafe {
            libc::read(
                self.fd(),
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len(),
            )
        };
        if read <= 0 {
            return false;
        }
        self.output.extend_from_slice(&buf[..read as usize]);
        true
    }

    fn wait_for(&mut self, needle: &str) {
        let start = Instant::now();
        while !self.text().contains(needle) {
            assert!(
                start.elapsed() < WAIT,
                "timed out waiting for {needle:?}; got:\n{}",
                self.text()
            );
            if !self.pump(500) {
                assert!(
                    self.text().contains(needle),
                    "EOF before {needle:?}; got:\n{}",
                    self.text()
                );
                return;
            }
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).expect("write to pty");
    }

    fn attrs(&self) -> TermAttrs {
        attrs_of(self.fd())
    }

    fn is_raw(attrs: &TermAttrs) -> bool {
        attrs.lflag & libc::ICANON as u64 == 0
    }

    fn is_cooked(attrs: &TermAttrs) -> bool {
        attrs.lflag & libc::ICANON as u64 != 0
    }

    fn kill_reap(&mut self) {
        if self.reaped {
            return;
        }
        self.reaped = true;
        // SAFETY: best-effort kill of a known-spawned test child.
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
            let mut status = 0;
            libc::waitpid(self.pid, &mut status, 0);
        }
    }

    fn wait_exit(&mut self) -> Option<i32> {
        let start = Instant::now();
        loop {
            let mut status = 0;
            // SAFETY: reaps only the known-spawned test child.
            let done =
                unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if done == self.pid {
                self.reaped = true;
                if libc::WIFEXITED(status) {
                    return Some(libc::WEXITSTATUS(status));
                }
                return None;
            }
            assert!(
                start.elapsed() < WAIT,
                "timed out waiting for child exit; output:\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for PtyChild {
    fn drop(&mut self) {
        self.kill_reap();
    }
}

/// Process environment is process-global, and `spawn_pty` stages the child's
/// environment there before `fork`. This lock serializes that handoff so two
/// parallel PTY tests can never swap vaults; it is held only across
/// set-var/fork/restore, never across child interaction.
static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn spawn_pty(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    extra: &[(std::ffi::OsString, std::ffi::OsString)],
) -> PtyChild {
    spawn_pty_mode(env, fake, args, extra, false)
}

fn spawn_pty_colored(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    extra: &[(std::ffi::OsString, std::ffi::OsString)],
) -> PtyChild {
    spawn_pty_mode(env, fake, args, extra, true)
}

fn spawn_pty_mode(
    env: &GcalEnv,
    fake: &FakeGog,
    args: &[&str],
    extra: &[(std::ffi::OsString, std::ffi::OsString)],
    colored: bool,
) -> PtyChild {
    let (master, slave_path) = open_master();
    let before = attrs_of(master.as_raw_fd());
    let slave_c = CString::new(slave_path).expect("slave path");
    let bob_c = CString::new(BOB_BIN).expect("bob path");
    let argv_owned: Vec<CString> = ["bob", "gcal"]
        .into_iter()
        .chain(args.iter().copied())
        .map(|arg| CString::new(arg).expect("argv"))
        .collect();
    // Pointer table and C strings are fully built before fork so the child
    // performs no allocation between fork and exec.
    let mut argv_ptrs: Vec<*const libc::c_char> =
        argv_owned.iter().map(|arg| arg.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());

    let mut pairs = env.env_pairs();
    pairs.extend(fake.env_pairs());
    pairs.extend(extra.iter().cloned());
    if colored {
        pairs.retain(|(key, _)| key != "NO_COLOR");
        pairs.push(("CLICOLOR_FORCE".into(), "1".into()));
    }
    let _spawn = SPAWN_LOCK.lock().expect("pty spawn lock");
    let saved: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = pairs
        .iter()
        .map(|(key, _)| (key.clone(), std::env::var_os(key)))
        .collect();
    let mut saved = saved;
    if colored {
        saved.push(("NO_COLOR".into(), std::env::var_os("NO_COLOR")));
        unsafe {
            std::env::remove_var("NO_COLOR");
        }
    }
    for (key, value) in &pairs {
        // SAFETY: scoped to the fork below; the harness isolates children
        // with env_clear, so sibling tests never observe these values.
        unsafe {
            std::env::set_var(key, value);
        }
    }
    // SAFETY: the child performs only async-signal-safe calls (close,
    // setsid, open, dup2, execv, _exit) before exec, using memory prepared
    // above; the parent restores every variable it set.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork");
    if pid == 0 {
        unsafe {
            libc::close(master.as_raw_fd());
            libc::setsid();
            // Opening the slave as a session leader without O_NOCTTY
            // acquires it as the controlling terminal, so the child is the
            // foreground process group and cooked-mode Ctrl-C reaches it.
            let slave =
                libc::open(slave_c.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
            if slave < 0 {
                libc::_exit(127);
            }
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            // Foreground the child's own process group so cooked-mode
            // VINTR (Ctrl-C) arrives as SIGINT. SIGTTOU is ignored only
            // around the call and restored before exec.
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(slave, libc::getpid());
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            if slave > 2 {
                libc::close(slave);
            }
            libc::execv(bob_c.as_ptr(), argv_ptrs.as_ptr());
            libc::_exit(127);
        }
    }
    for (key, old) in saved {
        // SAFETY: restores the pre-fork process environment.
        unsafe {
            match old {
                Some(value) => std::env::set_var(&key, &value),
                None => std::env::remove_var(&key),
            }
        }
    }
    PtyChild {
        master,
        pid,
        output: Vec::new(),
        before,
        reaped: false,
    }
}

fn spawn_ack_session(
    env: &GcalEnv,
    extra: &[(std::ffi::OsString, std::ffi::OsString)],
) -> (PtyChild, FakeGog) {
    let fake = FakeGog::new(env, "fake-gog");
    fake.events_json(&session_events());
    let mut child = spawn_pty(env, &fake, &["ack"], extra);
    child.wait_for("─ 1 of 2 ─");
    (child, fake)
}

fn spawn_ack_session_events(
    env: &GcalEnv,
    events: &str,
    colored: bool,
) -> (PtyChild, FakeGog) {
    let fake = FakeGog::new(env, "fake-gog");
    fake.events_json(events);
    let mut child = if colored {
        spawn_pty_colored(env, &fake, &["ack"], &[])
    } else {
        spawn_pty(env, &fake, &["ack"], &[])
    };
    child.wait_for("a ack");
    (child, fake)
}

#[test]
fn pty_quit_restores_terminal() {
    let env = GcalEnv::new("bob-cli-gcal-ack-pty-quit");
    prepare(&env);
    let (mut child, _fake) = spawn_ack_session(&env, &[]);
    assert!(PtyChild::is_raw(&child.attrs()), "session reads raw keys");
    child.write_bytes(b"q");
    child.wait_for("✓ 0 acked · 0 noted · 0 skipped · 0 series-covered");
    child.wait_for("→ 2 remaining");
    assert_eq!(child.wait_exit(), Some(0));
    assert_eq!(child.attrs(), child.before, "quit restores termios");
}

#[test]
fn pty_ctrl_c_in_raw_restores_terminal() {
    let env = GcalEnv::new("bob-cli-gcal-ack-pty-raw-int");
    prepare(&env);
    let (mut child, _fake) = spawn_ack_session(&env, &[]);
    child.write_bytes(b"a");
    child.wait_for("✓ Acked Budget review · event scope");
    child.write_bytes(b"\x03");
    child.wait_for("✓ 1 acked · 0 noted · 0 skipped");
    assert_eq!(child.wait_exit(), Some(130));
    assert_eq!(child.attrs(), child.before, "Ctrl-C restores termios");
    let journal =
        fs::read_to_string(env.vault().join(".bob/gcal/test-host.jsonl"))
            .expect("session journal");
    assert_eq!(journal.lines().count(), 1, "{journal}");
}

#[test]
fn pty_ctrl_c_in_cooked_line_restores_terminal() {
    let env = GcalEnv::new("bob-cli-gcal-ack-pty-cooked-int");
    prepare(&env);
    let (mut child, _fake) = spawn_ack_session(&env, &[]);
    child.write_bytes(b"n");
    child.wait_for("Note › ");
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        PtyChild::is_cooked(&child.attrs()),
        "note line reads cooked"
    );
    // Cooked-mode VINTR arrives as SIGINT to the foreground group; the
    // session reports saved answers and exits 130 with the terminal intact.
    child.write_bytes(b"\x03");
    child.wait_for("✓ 0 acked · 0 noted · 0 skipped");
    assert_eq!(child.wait_exit(), Some(130));
    assert_eq!(
        child.attrs(),
        child.before,
        "cooked Ctrl-C restores termios"
    );
}

#[test]
fn pty_raw_mode_returns_after_cooked_line() {
    let env = GcalEnv::new("bob-cli-gcal-ack-pty-transitions");
    prepare(&env);
    let (mut child, _fake) = spawn_ack_session(&env, &[]);
    child.write_bytes(b"n");
    child.wait_for("Note › ");
    std::thread::sleep(Duration::from_millis(400));
    assert!(PtyChild::is_cooked(&child.attrs()));
    child.write_bytes(b"Bring snacks\n");
    child.wait_for("✓ Noted Budget review · event scope · acked");
    std::thread::sleep(Duration::from_millis(400));
    assert!(PtyChild::is_raw(&child.attrs()), "session resumes raw mode");
    child.write_bytes(b"q");
    assert_eq!(child.wait_exit(), Some(0));
    assert_eq!(child.attrs(), child.before, "quit restores termios");
}

#[test]
fn pty_persistence_failure_restores_terminal() {
    let env = GcalEnv::new("bob-cli-gcal-ack-pty-error");
    prepare(&env);
    let journal_dir = env.vault().join(".bob/gcal");
    let (mut child, _fake) = spawn_ack_session(&env, &[]);
    child.write_bytes(b"a");
    child.wait_for("✓ Acked Budget review · event scope");
    let journal_file = journal_dir.join("test-host.jsonl");
    let first_answer = fs::read(&journal_file).expect("retain first answer");
    fs::remove_file(&journal_file).expect("remove journal file");
    fs::create_dir(&journal_file).expect("make journal unreadable");
    std::thread::sleep(Duration::from_millis(400));
    child.write_bytes(b"a");
    child.wait_for("read private-note evidence");
    assert_eq!(child.wait_exit(), Some(1));
    assert_eq!(child.attrs(), child.before, "failure restores termios");
    assert!(
        !child.text().contains("✓ Acked Hiring sync"),
        "failed answer must not be reported as successful"
    );
    fs::remove_dir(&journal_file).expect("remove unreadable journal fixture");
    fs::write(&journal_file, first_answer).expect("restore first answer");
    let journal =
        fs::read_to_string(journal_file).expect("read retained answer");
    assert_eq!(journal.lines().count(), 1, "only the first answer persists");
}

#[test]
fn pty_editor_and_adopt_stop_when_note_evidence_becomes_unreadable() {
    for key in ["e", "A"] {
        let env =
            GcalEnv::new(&format!("bob-cli-gcal-ack-pty-{key}-read-error"));
        prepare(&env);
        let mut event = json!({
            "id": "task-1",
            "summary": "Budget review",
            "status": "confirmed",
            "start": {"dateTime": "2026-10-31T13:00:00-04:00"},
            "end": {"dateTime": "2026-10-31T13:30:00-04:00"},
            "htmlLink": "https://calendar.google.com/event?eid=task-1",
            "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
            "attendees": [{"self": true, "responseStatus": "accepted"}],
        });
        if key == "A" {
            event["description"] = json!("-30 Drive to the airport");
        }
        let (mut child, _fake) = spawn_ack_session_events(
            &env,
            &json!({"events": [event], "nextPageToken": null}).to_string(),
            false,
        );
        let journal_file = env.vault().join(".bob/gcal/test-host.jsonl");
        fs::create_dir_all(journal_file.parent().unwrap())
            .expect("journal directory");
        fs::create_dir(&journal_file).expect("make journal unreadable");
        child.write_bytes(key.as_bytes());
        child.wait_for("read private-note evidence");
        assert_eq!(child.wait_exit(), Some(1), "key {key}");
        assert_eq!(child.attrs(), child.before, "key {key} restores termios");
        assert!(
            !child.text().contains("✓ Noted Budget review"),
            "key {key} must not report a note save"
        );
        fs::remove_dir(&journal_file).expect("remove failure fixture");
    }
}

#[test]
fn pty_card_wraps_unicode_rows_and_preserves_newlines_with_and_without_color() {
    let long_title = format!(
        "Interview 東京 {} https://calendar.google.com/event?eid={} root/{}",
        "Quarterly review with wide glyphs 界界界界界界界界界界".repeat(3),
        "event-id-with-a-long-url-fragment-".repeat(3),
        "long-root-name-".repeat(5),
    );
    let event = json!({
        "id": "wide-title",
        "summary": long_title,
        "status": "confirmed",
        "start": {"dateTime": "2026-10-31T13:00:00-04:00"},
        "end": {"dateTime": "2026-10-31T13:30:00-04:00"},
        "htmlLink": "https://calendar.google.com/event?eid=wide-title",
        "organizer": {"displayName": "Kelly", "email": "kelly@example.com", "self": false},
        "attendees": [{"self": true, "responseStatus": "accepted"}],
    });
    for colored in [false, true] {
        let env = GcalEnv::new(if colored {
            "bob-cli-gcal-ack-pty-card-color"
        } else {
            "bob-cli-gcal-ack-pty-card-no-color"
        });
        prepare(&env);
        let (mut child, _fake) = spawn_ack_session_events(
            &env,
            &json!({"events": [event.clone()], "nextPageToken": null})
                .to_string(),
            colored,
        );
        child.write_bytes(b"q");
        assert_eq!(child.wait_exit(), Some(0));
        assert!(child.output.windows(2).any(|pair| pair == b"\r\n"));
        for (index, byte) in child.output.iter().enumerate() {
            if *byte == b'\n' {
                assert_eq!(
                    index.checked_sub(1).map(|previous| child.output[previous]),
                    Some(b'\r'),
                    "PTY must process LF as CRLF in raw mode"
                );
            }
        }
        let plain = strip_ansi(&child.text());
        assert!(plain.contains("Interview 東京"), "{plain}");
        assert!(plain.contains("wide-title"), "{plain}");
        assert!(plain.contains("long-root-name-"), "{plain}");
        for line in plain.lines() {
            assert!(
                unicode_width::UnicodeWidthStr::width(line) <= 80,
                "card line exceeds 80 display columns ({}): {line:?}",
                unicode_width::UnicodeWidthStr::width(line)
            );
        }
        if colored {
            assert!(child.output.contains(&0x1b), "color styles should render");
        } else {
            assert!(
                !child.output.contains(&0x1b),
                "NO_COLOR suppresses escapes"
            );
        }
        assert_eq!(child.attrs(), child.before, "quit restores terminal");
    }
}

fn strip_ansi(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}
