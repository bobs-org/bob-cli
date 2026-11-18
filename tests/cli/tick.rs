//! Hermetic tick engine tests. BOB_TICK_NOW fixes only the scheduler clock;
//! BOB_TICK_STEP_EXE replaces bob children, and BOB_TICK_STEP_TIMEOUT_SECS
//! shortens ceilings. None of these hooks appear in public help.
#![cfg(unix)]
use crate::support::*;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    dir: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let this = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        fs::create_dir(this.path("vault")).unwrap();
        this.config("");
        fs::write(this.path("step.py"), r#"#!/usr/bin/env python3
import json, os, pathlib, signal, subprocess, sys, time
name = {'ref':'refs', 'task':'tasks'}.get(sys.argv[1], sys.argv[1])
root = pathlib.Path(os.environ['TICK_FIXTURE'])
with (root/'calls').open('a') as f:
    f.write(json.dumps(sys.argv[1:]) + '\n')
if os.environ.get('FAKE_SLEEP') == name:
    child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
    def stop(sig, frame):
        child.terminate()
        child.wait()
        sys.exit(128+sig)
    signal.signal(signal.SIGTERM, stop)
    (root/'ready').write_text(json.dumps([os.getpid(), child.pid]))
    time.sleep(60)
if os.environ.get('FAKE_FAIL') == name:
    for i in range(12): print('detail '+str(i), file=sys.stderr)
    sys.exit(7)
print('{}')
"#).unwrap();
        fs::set_permissions(
            this.path("step.py"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        this
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }
    fn config(&self, text: &str) {
        fs::write(self.path("config.yml"), format!("timezone: UTC\n{text}"))
            .unwrap();
    }
    fn command(&self, at: &str) -> Command {
        let mut c = bob_command();
        c.arg("tick")
            .env("BOB_CONFIG_FILE", self.path("config.yml"))
            .env("BOB_DIR", self.path("vault"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("BOB_TICK_STEP_EXE", self.path("step.py"))
            .env("BOB_TICK_NOW", at)
            .env("TICK_FIXTURE", self.dir.path())
            .env("HOSTNAME", "test-host")
            .env("NO_COLOR", "1")
            .env("TZ", "UTC")
            .env_remove("BOB_TICK_STEP_TIMEOUT_SECS")
            .env_remove("FAKE_SLEEP")
            .env_remove("FAKE_FAIL");
        c
    }
    fn json(&self, at: &str, args: &[&str]) -> Value {
        let out = self
            .command(at)
            .args(args)
            .args(["-f", "json"])
            .output()
            .unwrap();
        assert_success(&out);
        json(&out)
    }
    fn saved(&self, name: &str) -> Vec<u8> {
        fs::read(self.path(&format!("state/bob-cli/tick/{name}.json"))).unwrap()
    }
    fn ready(&self) -> Vec<i32> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(b) = fs::read(self.path("ready"))
                && let Ok(pids) = serde_json::from_slice(&b)
            {
                return pids;
            }
            assert!(Instant::now() < deadline, "fake child did not start");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", format_output(out)))
}
fn ran(receipt: &Value) -> Vec<&str> {
    receipt["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| !s["started_at"].is_null())
        .map(|s| s["name"].as_str().unwrap())
        .collect()
}
const T0: &str = "2026-11-18T10:00:58Z";

#[test]
fn five_minute_ticks_preserve_cadence_and_floor_jitter() {
    let f = Fixture::new();
    for minute in (0..=60).step_by(5) {
        let at = format!(
            "2026-11-18T{:02}:{:02}:{:02}Z",
            10 + minute / 60,
            minute % 60,
            if minute == 0 { 58 } else { 1 }
        );
        let r = f.json(&at, &[]);
        let expected = if minute % 60 == 0 {
            vec!["gcal", "gmail", "refs", "projects", "tasks"]
        } else if minute % 15 == 0 {
            vec!["refs", "projects", "tasks"]
        } else {
            vec![]
        };
        assert_eq!(ran(&r), expected, "{at}");
        assert_eq!(r["schema_version"], 1);
        assert_eq!(r["command"], "tick");
        assert_eq!(r["trigger"], "scheduled");
        assert_eq!(
            serde_json::from_slice::<Value>(&f.saved("last")).unwrap(),
            r
        );
    }
    let calls = fs::read_to_string(f.path("calls")).unwrap();
    assert!(calls.contains("\"--retry-timeout\", \"120\""));
    assert!(calls.lines().all(|line| line.contains("--bob-dir")));
    assert_unix_mode(&f.path("state/bob-cli/tick"), 0o700);
    assert_unix_mode(&f.path("state/bob-cli/tick/state.json"), 0o600);
    assert_unix_mode(&f.path("state/bob-cli/tick/last.json"), 0o600);
    assert!(!f.path("state/bob-cli/tick/running.json").exists());
}
#[test]
fn failure_restarts_clock_and_keeps_last_success() {
    let f = Fixture::new();
    f.json(T0, &[]);
    let out = f
        .command("2026-11-18T11:00:00Z")
        .env("FAKE_FAIL", "gcal")
        .args(["-f", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let r = json(&out);
    assert_eq!(r["steps"][0]["reason"], "exit_nonzero");
    assert_eq!(r["steps"][0]["exit_code"], 7);
    assert_eq!(r["steps"][0]["last_success_at"], T0);
    assert!(!ran(&f.json("2026-11-18T11:05:00Z", &[])).contains(&"gcal"));
    assert!(ran(&f.json("2026-11-18T12:00:00Z", &[])).contains(&"gcal"));
}
#[test]
fn named_all_off_and_follow_up_are_explicit() {
    let f = Fixture::new();
    f.json(T0, &[]);
    let r = f.json("2026-11-18T10:05:00Z", &["gmail", "gcal"]);
    assert_eq!(ran(&r), ["gcal", "gmail"]);
    let state: Value = serde_json::from_slice(&f.saved("state")).unwrap();
    assert_eq!(state["steps"]["tasks"]["follow_up_pending"], true);
    let r = f.json("2026-11-18T10:06:00Z", &[]);
    assert_eq!(ran(&r), ["projects", "tasks"]);
    assert_eq!(r["steps"][4]["reason"], "follow_up");
    assert!(ran(&f.json("2026-11-18T10:07:00Z", &["-a"])).contains(&"gkeep"));
    f.config("tick: {steps: {gcal: off}}");
    let before = fs::read(f.path("calls")).unwrap();
    let out = f.command(T0).args(["gmail", "gcal"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(before, fs::read(f.path("calls")).unwrap());
    assert!(!ran(&f.json(T0, &["-a"])).contains(&"gcal"));
}
#[test]
fn intake_pulls_derive_even_when_their_intervals_are_longer() {
    let f = Fixture::new();
    f.config("tick: {steps: {projects: 1d, tasks: 1d}}");
    f.json(T0, &[]);
    let r = f.json("2026-11-18T10:15:00Z", &[]);
    assert_eq!(ran(&r), ["refs", "projects", "tasks"]);
    assert_eq!(r["steps"][4]["reason"], "follows_intake");
}
#[test]
fn read_only_views_and_wrong_writer_never_write_or_spawn() {
    let f = Fixture::new();
    f.config("gcal: {writer: elsewhere}");
    for arg in ["-d", "-s"] {
        let r = f.json(T0, &[arg]);
        assert!(r["banner"]
            .as_str()
            .unwrap()
            .contains("tick.writer is elsewhere"));
    }
    let out = f.command(T0).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("this host is test-host"));
    assert!(!f.path("state").exists());
    assert!(!f.path("calls").exists());
    f.config("");
    f.json(T0, &[]);
    let state = f.saved("state");
    let receipt = f.saved("last");
    let calls = fs::read(f.path("calls")).unwrap();
    for arg in ["-d", "-s"] {
        f.json(T0, &[arg]);
    }
    assert_eq!(state, f.saved("state"));
    assert_eq!(receipt, f.saved("last"));
    assert_eq!(calls, fs::read(f.path("calls")).unwrap());
    assert_eq!(
        f.json(T0, &["-s", "gmail"])["steps"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn bad_tick_block_does_not_break_another_command() {
    let f = Fixture::new();
    for (config, key) in [
        ("tick: {bad: 1}", "tick.bad"),
        ("tick: {steps: {bad: 1h}}", "tick.steps.bad"),
        ("tick: {steps: {gcal: 1hr}}", "tick.steps.gcal"),
        ("tick: {steps: {gcal: 0m}}", "tick.steps.gcal"),
        ("tick: {steps: {gcal: 8d}}", "tick.steps.gcal"),
    ] {
        f.config(config);
        let out = f.command(T0).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(stderr(&out).contains(key));
        assert!(!f.path("state").exists());
        assert!(!f.path("calls").exists());
        let plan = bob_command()
            .args(["plan", "-b"])
            .arg(f.path("vault"))
            .env("BOB_CONFIG_FILE", f.path("config.yml"))
            .output()
            .unwrap();
        assert_success(&plan);
    }
}
#[test]
fn corrupt_state_recovers_and_scheduler_ignores_bob_now() {
    let f = Fixture::new();
    fs::create_dir_all(f.path("state/bob-cli/tick")).unwrap();
    fs::write(f.path("state/bob-cli/tick/state.json"), "bad").unwrap();
    let out = f
        .command(T0)
        .env("BOB_NOW", "1999-01-01")
        .args(["-f", "json"])
        .output()
        .unwrap();
    assert_success(&out);
    assert!(stderr(&out).contains("ignoring state"));
    assert_eq!(json(&out)["started_at"], T0);
    let _: Value = serde_json::from_slice(&f.saved("state")).unwrap();
}
#[test]
fn lock_contention_preserves_state_and_receipt() {
    use fs2::FileExt;
    let f = Fixture::new();
    f.json(T0, &[]);
    let state = f.saved("state");
    let receipt = f.saved("last");
    let lock = fs::File::open(f.path("state/bob-cli/tick/run.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let out = f.command(T0).output().unwrap();
    assert_success(&out);
    assert!(stdout(&out).contains("already running"));
    assert_eq!(state, f.saved("state"));
    assert_eq!(receipt, f.saved("last"));
    assert!(f.command(T0).arg("-q").output().unwrap().stdout.is_empty());
}
fn assert_gone(pids: &[i32]) {
    let deadline = Instant::now() + Duration::from_secs(3);
    for pid in pids {
        while unsafe { libc::kill(*pid, 0) } == 0 {
            assert!(Instant::now() < deadline, "process {pid} survived");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
#[test]
fn timeout_kills_group_and_continues_later_steps() {
    let f = Fixture::new();
    let out = f
        .command(T0)
        .env("FAKE_SLEEP", "gcal")
        .env("BOB_TICK_STEP_TIMEOUT_SECS", "1.5")
        .args(["-f", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let r = json(&out);
    assert_eq!(r["steps"][0]["reason"], "timeout");
    assert_eq!(r["steps"][5]["outcome"], "ok");
    assert_gone(&f.ready());
}
#[test]
fn termination_forwards_to_group_and_persists_interruption() {
    let f = Fixture::new();
    let child = f
        .command(T0)
        .env("FAKE_SLEEP", "gcal")
        .args(["-f", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pids = f.ready();
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(143));
    assert_eq!(json(&out)["steps"][0]["reason"], "interrupted");
    assert_eq!(
        serde_json::from_slice::<Value>(&f.saved("state")).unwrap()["steps"]
            ["gcal"]["last_reason"],
        "interrupted"
    );
    assert_gone(&pids);
}
#[test]
fn abrupt_death_keeps_follow_up_intent() {
    let f = Fixture::new();
    f.json(T0, &[]);
    let mut child = f
        .command("2026-11-18T10:05:00Z")
        .arg("gcal")
        .env("FAKE_SLEEP", "gcal")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pids = f.ready();
    unsafe {
        libc::kill(child.id() as i32, libc::SIGKILL);
    }
    child.wait().unwrap();
    // SIGKILL is intentionally uncatchable; clean up the orphaned test group.
    unsafe {
        libc::kill(-pids[0], libc::SIGTERM);
    }
    let r = f.json("2026-11-18T10:06:00Z", &[]);
    assert_eq!(ran(&r), ["projects", "tasks"]);
    assert_eq!(r["steps"][4]["reason"], "follow_up");
}
#[test]
fn spawn_failure_has_no_interval_and_quiet_shows_only_errors() {
    let f = Fixture::new();
    let out = f
        .command(T0)
        .arg("gcal")
        .arg("-f")
        .arg("json")
        .env("BOB_TICK_STEP_EXE", f.path("missing"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out)["steps"][0]["reason"], "spawn_failed");
    assert!(json(&out)["steps"][0]["started_at"].is_null());
    let out = f.command(T0).arg("-q").output().unwrap();
    assert_success(&out);
    assert!(out.stdout.is_empty());
    let out = f
        .command(T0)
        .args(["-q", "gcal"])
        .env("FAKE_FAIL", "gcal")
        .output()
        .unwrap();
    let text = stdout(&out);
    assert!(text.contains("✗ gcal"));
    assert!(text.contains("failed · 1 ran"));
    assert!(!text.contains("detail 3"));
    assert!(text.contains("detail 4"));
    assert!(!text.contains("✓"));
}
#[test]
fn help_conflicts_order_and_completion() {
    let out = bob_command().args(["tick", "--help"]).output().unwrap();
    assert_success(&out);
    let help = stdout(&out);
    assert!(help.contains("[STEP]..."));
    assert!(help.contains("never holds the shared vault lock"));
    assert!(!help.contains("BOB_TICK_"));
    assert_text_order(
        &help,
        &[
            "-a, --all",
            "-b, --bob-dir",
            "-d, --dry-run",
            "-f, --format",
            "-h, --help",
            "-q, --quiet",
            "-s, --status",
        ],
    );
    assert_stdout_has_no_ansi(&out);
    let f = Fixture::new();
    for args in [
        vec!["-s", "-a"],
        vec!["-s", "-d"],
        vec!["-a", "gcal"],
        vec!["unknown"],
    ] {
        assert_eq!(
            f.command(T0).args(args).output().unwrap().status.code(),
            Some(2)
        );
    }
    assert!(!f.path("state").exists());
    let out = bob_completion_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "tick",
            "",
        ])
        .output()
        .unwrap();
    assert_success(&out);
    let choices: Vec<_> = stdout(&out)
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').next().unwrap().to_owned())
        .collect();
    for step in ["gcal", "gmail", "gkeep", "refs", "projects", "tasks"] {
        assert!(choices.contains(&step.to_string()), "{choices:?}");
    }
}

#[test]
fn human_views_match_colorless_snapshots() {
    fn snapshot(name: &str, output: &Output) {
        assert_stdout_has_no_ansi(output);
        let text = stdout(output);
        let elapsed = regex::Regex::new(r"\b\d+\.\ds\b").unwrap();
        let normalized = elapsed
            .replace_all(&text, "<time>")
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tick")
            .join(format!("{name}.txt"));
        if std::env::var("BOB_UPDATE_GOLDENS").ok().as_deref() == Some("1") {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &normalized).unwrap();
        }
        assert_eq!(
            fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("missing {}", path.display())),
            normalized,
            "{name}"
        );
    }
    let f = Fixture::new();
    snapshot("run", &f.command(T0).output().unwrap());
    snapshot(
        "nothing_due",
        &f.command("2026-11-18T10:05:00Z").output().unwrap(),
    );
    snapshot(
        "plan",
        &f.command("2026-11-18T10:05:00Z")
            .arg("-d")
            .output()
            .unwrap(),
    );
    snapshot(
        "status",
        &f.command("2026-11-18T10:05:00Z")
            .arg("-s")
            .output()
            .unwrap(),
    );
    snapshot(
        "quiet_failure",
        &f.command("2026-11-18T10:10:00Z")
            .args(["-q", "gcal"])
            .env("FAKE_FAIL", "gcal")
            .output()
            .unwrap(),
    );
}
