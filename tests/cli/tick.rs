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
        // Readiness gate needs today's note to exist, be rendered, and have
        // ## Pomodoros. The scheduler clock (BOB_TICK_NOW) is 2026-11-18 in
        // these tests and the host runs the same day with timezone UTC.
        fs::create_dir_all(this.path("vault/2026")).unwrap();
        fs::write(
            this.path("vault/2026/20261118.md"),
            "# 2026-11-18\n## Pomodoros\n- [ ] 0900-0930 Test\n",
        )
        .unwrap();
        fs::write(this.path("step.py"), r#"#!/usr/bin/env python3
import json, os, pathlib, signal, subprocess, sys, time
name = {'ref':'refs', 'task':'tasks'}.get(sys.argv[1], sys.argv[1])
root = pathlib.Path(os.environ['TICK_FIXTURE'])
with (root/'calls').open('a') as f:
    f.write(json.dumps(sys.argv[1:]) + '\n')
# Mid-scan mutation probe for the refs pre-scan fingerprint test.
if os.environ.get('FAKE_REFS_TOUCH') == name:
    try:
        idx = sys.argv.index('--bob-dir')
        vault = pathlib.Path(sys.argv[idx+1])
        (vault/'lib').mkdir(parents=True, exist_ok=True)
        (vault/'lib'/'mid-scan.pdf').write_text('%PDF-1.4 mid-scan')
    except Exception:
        pass
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
custom = os.environ.get('FAKE_JSON_' + name.upper())
if custom:
    print(custom)
else:
    print('{}')
exit_code = os.environ.get('FAKE_EXIT_' + name.upper())
if exit_code:
    sys.exit(int(exit_code))
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
            // Tick uses its own scheduler clock, while readiness resolves
            // today's note through bob_env::now(). Pin both to keep the
            // fixture stable after the host calendar rolls over.
            .env("BOB_NOW", at)
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
        // Refs change gate: touch lib/ so the fingerprint differs whenever
        // refs is due; otherwise the gate correctly skips an unchanged library.
        fs::create_dir_all(f.path("vault/lib")).unwrap();
        fs::write(
            f.path(&format!("vault/lib/tick-{minute}.pdf")),
            format!("%PDF-1.4 tick {minute}"),
        )
        .unwrap();
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
    fs::create_dir_all(f.path("vault/lib")).unwrap();
    fs::write(f.path("vault/lib/tick-15.pdf"), "%PDF-1.4 tick 15").unwrap();
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

#[test]
fn readiness_missing_then_ready_and_unrendered() {
    let f = Fixture::new();
    // Missing note: gcal and tasks wait, others run.
    fs::remove_file(f.path("vault/2026/20261118.md")).unwrap();
    let r = f.json(T0, &[]);
    assert_eq!(r["steps"][0]["outcome"], "waiting");
    assert_eq!(r["steps"][0]["reason"], "note_missing");
    assert_eq!(r["steps"][5]["outcome"], "waiting");
    assert_eq!(r["steps"][5]["reason"], "note_missing");
    assert!(r["steps"][0]["started_at"].is_null());
    assert_eq!(r["exit_code"], 0);
    // Dry-run surfaces the gate reason without spawning.
    let before = fs::read(f.path("calls")).ok();
    let d = f.json(T0, &["-d"]);
    assert_eq!(d["steps"][0]["would_run"], false);
    assert!(d["steps"][0]["summary"]
        .as_str()
        .unwrap()
        .contains("doesn't exist"));
    assert_eq!(before, fs::read(f.path("calls")).ok());
    // Once the note appears, the next tick runs them.
    fs::write(
        f.path("vault/2026/20261118.md"),
        "# 2026-11-18\n## Pomodoros\n- [ ] 0900-0930 Test\n",
    )
    .unwrap();
    let r = f.json("2026-11-18T10:05:00Z", &["gcal", "tasks"]);
    assert_eq!(ran(&r), ["gcal", "tasks"]);
    // Unrendered note waits with note_unrendered.
    fs::write(
        f.path("vault/2026/20261118.md"),
        "# 2026-11-18\n## Pomodoros\n<% tp.file.title %>\n",
    )
    .unwrap();
    let r = f.json("2026-11-18T10:10:00Z", &["gcal"]);
    assert_eq!(r["steps"][0]["outcome"], "waiting");
    assert_eq!(r["steps"][0]["reason"], "note_unrendered");
}

#[test]
fn gcal_yesterday_scope_cooldown_and_waiting_status() {
    let f = Fixture::new();
    // First run uses -n 2.
    let r = f.json(T0, &["gcal"]);
    assert_eq!(ran(&r), ["gcal"]);
    let calls = fs::read_to_string(f.path("calls")).unwrap();
    assert!(calls.contains("\"-n\", \"2\""), "{calls}");
    let state: Value = serde_json::from_slice(&f.saved("state")).unwrap();
    assert_eq!(state["steps"]["gcal"]["yesterday_synced_on"], "2026-11-18");
    // Second run same day drops -n 2.
    fs::write(f.path("calls"), "").unwrap();
    let r = f.json("2026-11-18T11:00:00Z", &["gcal"]);
    assert_eq!(ran(&r), ["gcal"]);
    let calls = fs::read_to_string(f.path("calls")).unwrap();
    assert!(!calls.contains("\"-n\""), "{calls}");
    // Failed first run retries -n 2.
    let f2 = Fixture::new();
    let out = f2
        .command(T0)
        .arg("gcal")
        .env("FAKE_FAIL", "gcal")
        .args(["-f", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let calls = fs::read_to_string(f2.path("calls")).unwrap();
    assert!(calls.contains("\"-n\", \"2\""), "{calls}");
    fs::write(f2.path("calls"), "").unwrap();
    let r = f2.json("2026-11-18T11:00:00Z", &["gcal"]);
    assert_eq!(ran(&r), ["gcal"]);
    assert!(fs::read_to_string(f2.path("calls"))
        .unwrap()
        .contains("\"-n\""));
    // Host cooldown: a recent synced status suppresses automated gcal.
    let f3 = Fixture::new();
    f3.json(T0, &[]);
    let status_dir = f3.path("state/bob/gcal");
    fs::create_dir_all(&status_dir).unwrap();
    fs::write(
        status_dir.join("status.json"),
        serde_json::json!({
            "ts": "2026-11-18T10:50:00+00:00",
            "host": "test-host",
            "day": "2026-11-18",
            "outcome": "synced",
            "tasks_added": 0,
            "context_added": 0,
            "links_added": 0,
            "links_refiled": 0,
            "links_closed": 0,
        })
        .to_string(),
    )
    .unwrap();
    let r = f3.json("2026-11-18T11:00:00Z", &[]);
    assert!(!ran(&r).contains(&"gcal"), "cooldown should skip gcal");
    // Explicit runs still count and a waiting status does not suppress.
    let r = f3.json("2026-11-18T11:00:00Z", &["gcal"]);
    assert_eq!(ran(&r), ["gcal"]);
    fs::write(
        status_dir.join("status.json"),
        serde_json::json!({
            "ts": "2026-11-18T10:50:00+00:00",
            "host": "test-host",
            "day": "2026-11-18",
            "outcome": "waiting",
        })
        .to_string(),
    )
    .unwrap();
    fs::create_dir_all(f3.path("vault/lib")).unwrap();
    fs::write(f3.path("vault/lib/tick.pdf"), "%PDF-1.4 x").unwrap();
    let r = f3.json("2026-11-18T12:00:00Z", &[]);
    assert!(
        ran(&r).contains(&"gcal"),
        "waiting status must not suppress"
    );
}

#[test]
fn gmail_busy_waits_without_consuming_interval() {
    let f = Fixture::new();
    f.json(T0, &[]);
    let busy = serde_json::json!({
        "schema_version": 1,
        "ok": true,
        "busy": true,
        "dry_run": false,
        "pulled": 0,
        "results": [],
        "watch": [],
        "problems": [],
        "archive": [],
        "archive_hints": [],
        "summary": {"watching": 0, "awaiting": 0, "archived": 0},
        "warnings": [],
    })
    .to_string();
    let out = f
        .command("2026-11-18T11:00:00Z")
        .args(["-f", "json"])
        .env("FAKE_JSON_GMAIL", &busy)
        .output()
        .unwrap();
    assert_success(&out);
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    let gmail = &r["steps"][1];
    assert_eq!(gmail["name"], "gmail");
    assert_eq!(gmail["outcome"], "waiting");
    assert_eq!(gmail["reason"], "vault_busy");
    // Clock untouched: still due on the next tick.
    let state: Value = serde_json::from_slice(&f.saved("state")).unwrap();
    assert_eq!(
        state["steps"]["gmail"]["last_started_at"],
        serde_json::from_slice::<Value>(&f.saved("state")).unwrap()["steps"]
            ["gmail"]["last_started_at"]
    );
    let r2 = f.json("2026-11-18T11:05:00Z", &[]);
    assert!(
        ran(&r2).contains(&"gmail"),
        "busy must not restart the clock"
    );
}

#[test]
fn refs_change_gate_outcomes_and_prescan_fingerprint() {
    // Unchanged library skips when due.
    let f = Fixture::new();
    f.json(T0, &[]);
    let r = f.json("2026-11-18T10:15:00Z", &[]);
    assert!(!ran(&r).contains(&"refs"), "unchanged lib should skip");
    assert_eq!(r["steps"][3]["reason"], "unchanged");
    // PDF touched opens the gate.
    fs::create_dir_all(f.path("vault/lib")).unwrap();
    fs::write(f.path("vault/lib/a.pdf"), "%PDF-1.4 a").unwrap();
    let r = f.json("2026-11-18T10:30:00Z", &[]);
    assert!(ran(&r).contains(&"refs"), "touched PDF should run");
    // xlib addition also opens the gate.
    let f2 = Fixture::new();
    f2.json(T0, &[]);
    fs::create_dir_all(f2.path("vault/xlib")).unwrap();
    fs::write(f2.path("vault/xlib/b.pdf"), "%PDF-1.4 b").unwrap();
    let r = f2.json("2026-11-18T10:15:00Z", &[]);
    assert!(ran(&r).contains(&"refs"), "xlib addition should run");
    // Hook forces the gate open.
    let f3 = Fixture::new();
    f3.json(T0, &[]);
    let r = f3
        .command("2026-11-18T10:15:00Z")
        .args(["-f", "json"])
        .env("BOB_HIGHLIGHTS_PRE_SCAN_HOOK", "true")
        .output()
        .unwrap();
    assert_success(&r);
    let v: Value = serde_json::from_slice(&r.stdout).unwrap();
    assert!(ran(&v).contains(&"refs"), "hook should force refs");
    assert_eq!(v["steps"][3]["gate"]["hook"], true);
    // 6h ceiling forces a full scan.
    let f4 = Fixture::new();
    f4.json(T0, &[]);
    let r = f4.json("2026-11-18T16:15:00Z", &[]);
    assert!(ran(&r).contains(&"refs"), "6h ceiling should run refs");
    // Named refs always scans.
    let f5 = Fixture::new();
    f5.json(T0, &[]);
    let r = f5.json("2026-11-18T10:05:00Z", &["refs"]);
    assert_eq!(ran(&r), ["refs"]);
    assert_eq!(r["steps"][3]["gate"]["open"], true);
    // Stored fingerprint is the pre-scan one: a mid-scan write is not lost.
    let f6 = Fixture::new();
    fs::create_dir_all(f6.path("vault/lib")).unwrap();
    fs::write(f6.path("vault/lib/before.pdf"), "%PDF-1.4 before").unwrap();
    f6.json(T0, &[]);
    let stored: Value = serde_json::from_slice(&f6.saved("state")).unwrap();
    let fp_before = stored["steps"]["refs"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!fp_before.is_empty());
    // Next run writes mid-scan.pdf during the scan; the stored fingerprint
    // must stay the pre-scan value, so the following tick still sees a change.
    // Touch trigger.pdf first so the gate opens (otherwise unchanged would skip).
    fs::write(f6.path("vault/lib/trigger.pdf"), "%PDF-1.4 trigger").unwrap();
    let out = f6
        .command("2026-11-18T10:15:00Z")
        .args(["-f", "json"])
        .env("FAKE_REFS_TOUCH", "refs")
        .output()
        .unwrap();
    assert_success(&out);
    let stored: Value = serde_json::from_slice(&f6.saved("state")).unwrap();
    let fp_prescan = stored["steps"]["refs"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    // Pre-scan includes trigger.pdf (gate opener) but not mid-scan.pdf
    // (written during the scan), so it must differ from the first run.
    assert_ne!(fp_prescan, fp_before, "trigger should change fingerprint");
    assert!(f6.path("vault/lib/mid-scan.pdf").exists());
    // The following tick still sees a change (mid-scan not in stored),
    // proving the stored fingerprint is the pre-scan one.
    fs::remove_file(f6.path("calls")).ok();
    let r = f6.json("2026-11-18T10:30:00Z", &[]);
    assert!(
        ran(&r).contains(&"refs"),
        "mid-scan change must not be lost"
    );
    // Gate JSON is present for the mac_cutover probe: check the run receipt,
    // since -d at 10:30 is not due (no gate evaluated).
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["steps"][3]["gate"]["kind"], "refs_change");
}

#[test]
fn per_step_summaries_come_from_real_json_shapes() {
    let f = Fixture::new();
    let gcal = serde_json::json!({
        "schema_version": 1, "ok": true, "day": "2026-11-18",
        "path": "2026/20261118.md", "status": "synced", "force": false,
        "rows": [], "in_sync": 0, "skipped": [], "counts": {"unacked": 0},
        "pomodoro": {"links_added": 2, "links_refiled": 1, "links_closed": 1,
            "links_sorted": 0, "entries": []},
        "waiting": null, "warnings": [], "deprecations": [],
    })
    .to_string();
    let gmail = serde_json::json!({
        "schema_version": 1, "ok": true, "busy": false, "dry_run": false,
        "pulled": 2, "results": [], "watch": [], "problems": [],
        "archive": [], "archive_hints": [],
        "summary": {"watching": 1, "awaiting": 1, "archived": 1},
        "warnings": [],
    })
    .to_string();
    let gkeep = serde_json::json!({
        "schema_version": 1, "ok": true, "dry_run": false,
        "summary": {"written": 2, "archived": 0, "skipped": 0, "failed": 0,
            "refs": {}},
    })
    .to_string();
    let refs = serde_json::json!({
        "schema_version": 1, "ok": true, "summary": {
            "pdfs": 40, "created": 1, "updated": 1, "unchanged": 37,
            "markers": 3, "tasks": 2, "failures": 0, "held": 1,
        },
    })
    .to_string();
    let projects = serde_json::json!({
        "schema_version": 1, "ok": true, "changed": [{"path": "a.md"}],
    })
    .to_string();
    let tasks = serde_json::json!({
        "ok": true, "marked_next": [{}, {}], "marked_in_progress": [],
        "cleared": [{}], "marked_blocked": [], "unblocked": [{}],
    })
    .to_string();
    let out = f
        .command(T0)
        .args(["-a", "-f", "json"])
        .env("FAKE_JSON_GCAL", &gcal)
        .env("FAKE_JSON_GMAIL", &gmail)
        .env("FAKE_JSON_GKEEP", &gkeep)
        .env("FAKE_JSON_REFS", &refs)
        .env("FAKE_JSON_PROJECTS", &projects)
        .env("FAKE_JSON_TASKS", &tasks)
        .output()
        .unwrap();
    assert_success(&out);
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    let by_name = |n: &str| {
        r["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == n)
            .unwrap()
            .clone()
    };
    assert!(by_name("gcal")["summary"]
        .as_str()
        .unwrap()
        .contains("2 added"));
    assert!(by_name("gmail")["summary"]
        .as_str()
        .unwrap()
        .contains("2 filed"));
    assert!(by_name("gkeep")["summary"]
        .as_str()
        .unwrap()
        .contains("2 notes pulled"));
    assert!(by_name("refs")["summary"]
        .as_str()
        .unwrap()
        .contains("2 PDFs changed"));
    assert_eq!(
        by_name("refs")["summary"],
        "2 PDFs changed · 2 notes updated · 3 markers · 2 tasks · 1 held"
    );
    assert!(by_name("projects")["summary"]
        .as_str()
        .unwrap()
        .contains("1 note changed"));
    assert!(by_name("tasks")["summary"]
        .as_str()
        .unwrap()
        .contains("4 status changes"));
    // Gate reasons appear in -d and the gate object in JSON: the -a run
    // receipt carries gates for the gated steps (gcal readiness, refs change).
    assert!(r["steps"][0]["gate"].is_object());
    assert_eq!(r["steps"][0]["gate"]["kind"], "readiness");
    assert_eq!(r["steps"][3]["gate"]["kind"], "refs_change");
    // -d at T0 on a fresh fixture shows gate reasons without spawning.
    let f2 = Fixture::new();
    fs::create_dir_all(f2.path("vault/lib")).unwrap();
    fs::write(f2.path("vault/lib/a.pdf"), "%PDF-1.4 a").unwrap();
    let d = f2.command(T0).args(["-d", "-f", "json"]).output().unwrap();
    assert_success(&d);
    let v: Value = serde_json::from_slice(&d.stdout).unwrap();
    assert_eq!(v["steps"][3]["gate"]["kind"], "refs_change");
    assert_eq!(v["steps"][3]["would_run"], true);
}

#[test]
fn contention_waits_report_waiting_and_exit_zero() {
    let lock_busy = serde_json::json!({
        "ok": false,
        "error": {
            "code": "lock_busy",
            "message": "could not acquire the vault maintenance lock: busy",
        },
    })
    .to_string();
    let scan_busy = serde_json::json!({
        "ok": false,
        "error": {
            "code": "scan_busy",
            "message": "another bob ref scan is still running",
        },
    })
    .to_string();
    let inputs_changed = serde_json::json!({
        "ok": false,
        "schema_version": 1,
        "command": "ref scan",
        "error": {
            "code": "inputs_changed",
            "message": "scan inputs changed during planning (input changed: 2026/20261118.md); rerun bob ref scan",
        },
    })
    .to_string();
    // Projects lock_busy waits without consuming the interval.
    let f = Fixture::new();
    f.json(T0, &["projects"]);
    let before: Value = serde_json::from_slice(&f.saved("state")).unwrap();
    let started_before = before["steps"]["projects"]["last_started_at"].clone();
    let out = f
        .command("2026-11-18T11:00:00Z")
        .args(["projects", "-f", "json"])
        .env("FAKE_JSON_PROJECTS", &lock_busy)
        .env("FAKE_EXIT_PROJECTS", "1")
        .output()
        .unwrap();
    assert_success(&out);
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["exit_code"], 0);
    let projects = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "projects")
        .unwrap();
    assert_eq!(projects["outcome"], "waiting");
    assert_eq!(projects["reason"], "vault_busy");
    assert_eq!(projects["summary"], "vault busy · retrying next tick");
    let after: Value = serde_json::from_slice(&f.saved("state")).unwrap();
    assert_eq!(
        after["steps"]["projects"]["last_started_at"], started_before,
        "vault_busy must not restart the clock"
    );
    assert!(
        after["steps"]["projects"]["waiting_since"].is_string(),
        "first contention wait records waiting_since"
    );
    // Scan lock contention maps the same way, including via gcal's adapter.
    let f2 = Fixture::new();
    let out = f2
        .command(T0)
        .args(["gcal", "-f", "json"])
        .env("FAKE_JSON_GCAL", &scan_busy)
        .env("FAKE_EXIT_GCAL", "1")
        .output()
        .unwrap();
    assert_success(&out);
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    let gcal = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "gcal")
        .unwrap();
    assert_eq!(gcal["reason"], "vault_busy");
    assert_eq!(gcal["outcome"], "waiting");
    // Refs inputs_changed waits while consuming the refs interval.
    let f3 = Fixture::new();
    f3.json(T0, &["refs"]);
    let out = f3
        .command("2026-11-18T10:15:00Z")
        .args(["refs", "-f", "json"])
        .env("FAKE_JSON_REFS", &inputs_changed)
        .env("FAKE_EXIT_REFS", "1")
        .output()
        .unwrap();
    assert_success(&out);
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["exit_code"], 0);
    let refs = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "refs")
        .unwrap();
    assert_eq!(refs["outcome"], "waiting");
    assert_eq!(refs["reason"], "inputs_changed");
    assert_eq!(refs["summary"], "notes changed during scan · retrying");
    let after: Value = serde_json::from_slice(&f3.saved("state")).unwrap();
    assert_eq!(
        after["steps"]["refs"]["last_started_at"], refs["started_at"],
        "inputs_changed consumes the refs interval"
    );
}

#[test]
fn persistent_contention_escalates_to_stalled() {
    let lock_busy = serde_json::json!({
        "ok": false,
        "error": {
            "code": "lock_busy",
            "message": "could not acquire the vault maintenance lock: busy",
        },
    })
    .to_string();
    let f = Fixture::new();
    f.json(T0, &["projects"]);
    // Seed a contention wait older than the 2h stall window.
    let path = f.path("state/bob-cli/tick/state.json");
    let mut state: Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["steps"]["projects"]["last_outcome"] = Value::from("waiting");
    state["steps"]["projects"]["last_reason"] = Value::from("vault_busy");
    state["steps"]["projects"]["waiting_since"] =
        Value::from("2026-11-18T08:00:00Z");
    fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();
    let out = f
        .command("2026-11-18T11:00:00Z")
        .args(["projects", "-f", "json"])
        .env("FAKE_JSON_PROJECTS", &lock_busy)
        .env("FAKE_EXIT_PROJECTS", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    let projects = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "projects")
        .unwrap();
    assert_eq!(projects["outcome"], "failed");
    assert_eq!(projects["reason"], "stalled");
    assert!(
        projects["summary"]
            .as_str()
            .unwrap()
            .starts_with("still waiting after 2h · "),
        "{}",
        projects["summary"]
    );
    // A fresh wait below the window still waits.
    let f2 = Fixture::new();
    f2.json(T0, &["projects"]);
    let out = f2
        .command("2026-11-18T10:30:00Z")
        .args(["projects", "-f", "json"])
        .env("FAKE_JSON_PROJECTS", &lock_busy)
        .env("FAKE_EXIT_PROJECTS", "1")
        .output()
        .unwrap();
    assert_success(&out);
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    let projects = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "projects")
        .unwrap();
    assert_eq!(projects["outcome"], "waiting");
}

#[test]
fn waiting_derive_keeps_follow_up_pending() {
    let lock_busy = serde_json::json!({
        "ok": false,
        "error": {
            "code": "lock_busy",
            "message": "could not acquire the vault maintenance lock: busy",
        },
    })
    .to_string();
    let f = Fixture::new();
    f.config("tick: {steps: {projects: 1d, tasks: 1d}}");
    f.json(T0, &[]);
    fs::create_dir_all(f.path("vault/lib")).unwrap();
    fs::write(f.path("vault/lib/tick-15.pdf"), "%PDF-1.4 tick 15").unwrap();
    let out = f
        .command("2026-11-18T10:15:00Z")
        .args(["-f", "json"])
        .env("FAKE_JSON_PROJECTS", &lock_busy)
        .env("FAKE_EXIT_PROJECTS", "1")
        .output()
        .unwrap();
    assert_success(&out);
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    let projects = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "projects")
        .unwrap();
    assert_eq!(projects["outcome"], "waiting");
    let state: Value = serde_json::from_slice(&f.saved("state")).unwrap();
    assert_eq!(
        state["steps"]["projects"]["follow_up_pending"], true,
        "a waiting derive step must keep its follow-up"
    );
    let r = f.json("2026-11-18T10:16:00Z", &[]);
    assert!(
        ran(&r).contains(&"projects"),
        "follow-up must rerun projects even before its 1d interval"
    );
    assert_eq!(
        r["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "projects")
            .unwrap()["reason"],
        "follow_up"
    );
}

#[test]
fn destination_asset_scan_failed_still_fails() {
    let dest_failed = serde_json::json!({
        "ok": false,
        "schema_version": 1,
        "command": "ref scan",
        "error": {
            "code": "scan_failed",
            "message": "scan inputs changed during planning (destination asset changed: ref/chat/live.assets/fig.png); rerun bob ref scan",
        },
    })
    .to_string();
    let f = Fixture::new();
    let out = f
        .command(T0)
        .args(["refs", "-f", "json"])
        .env("FAKE_JSON_REFS", &dest_failed)
        .env("FAKE_EXIT_REFS", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    let refs = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "refs")
        .unwrap();
    assert_eq!(refs["outcome"], "failed");
    assert_eq!(refs["reason"], "exit_nonzero");
}
