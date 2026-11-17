//! Deterministic adversarial interleavings for the narrow scan lock window.
//!
//! Pause points are handshake-bounded (`BOB_REF_SCAN_TEST_*`): the child
//! writes `<seq>:<stage>` and waits until the go file contains that seq.
//! Subprocesses are reaped on drop. Lock paths and hooks stay inside the
//! temporary vault.

use super::overwrite_recovery;
use crate::support::*;
use fs2::FileExt;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const SCAN_JSON_NOW: &str = "2026-11-17 12:00:00";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

struct Handshake {
    ready: PathBuf,
    go: PathBuf,
}

impl Handshake {
    fn new(dir: &Path, name: &str) -> Self {
        let root = dir.join(name);
        fs::create_dir_all(&root).expect("handshake dir");
        Self {
            ready: root.join("ready"),
            go: root.join("go"),
        }
    }
}

struct SpawnedScan {
    child: Option<Child>,
}

impl SpawnedScan {
    fn wait(mut self) -> Output {
        self.child
            .take()
            .expect("spawned scan")
            .wait_with_output()
            .expect("wait for scan")
    }
}

impl Drop for SpawnedScan {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct ScanEnv {
    vault: PathBuf,
    state: PathBuf,
    lock: PathBuf,
}

fn wait_until(timeout: Duration, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what} after {timeout:?}");
}

fn wait_pause(hs: &Handshake, seq: usize) -> String {
    wait_until(HANDSHAKE_TIMEOUT, "scan handshake", || {
        fs::read_to_string(&hs.ready)
            .ok()
            .is_some_and(|body| body.starts_with(&format!("{seq}:")))
    });
    fs::read_to_string(&hs.ready).expect("read ready")
}

fn resume(hs: &Handshake, seq: usize) {
    fs::write(&hs.go, seq.to_string()).expect("write go");
}

fn scan_receipt_path(state: &Path) -> PathBuf {
    state.join("bob-cli/ref/last-scan.json")
}

fn open_lock_file(path: &Path) -> std::fs::File {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("lock dir");
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("open lock {}: {error}", path.display()))
}

fn hold_exclusive_lock(path: &Path) -> std::fs::File {
    let file = open_lock_file(path);
    wait_until(
        HANDSHAKE_TIMEOUT,
        &format!("exclusive lock {}", path.display()),
        || file.try_lock_exclusive().is_ok(),
    );
    file
}

fn hold_scan_writer_lock(state: &Path) -> std::fs::File {
    hold_exclusive_lock(&state.join("bob-cli/ref/scan.lock"))
}

fn hold_vault_maintenance_lock(path: &Path) -> std::fs::File {
    hold_exclusive_lock(path)
}

fn try_lock_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()?;
    Ok(file)
}

fn scan_json_stdout(output: &Output) -> Value {
    let text = stdout(output);
    assert_eq!(
        text.lines().count(),
        1,
        "JSON stdout must be exactly one line:\n{}",
        format_output(output)
    );
    serde_json::from_str(text.trim()).expect("stdout must parse as JSON")
}

fn write_marker_pdf(path: &Path, title: &str) {
    write_highlights_pdf(
        path,
        &format!("- status: wip\n- parent: obsidian\n- title: {title}\n"),
    );
}

fn apply_scan_env<'a>(
    command: &'a mut std::process::Command,
    env: &ScanEnv,
) -> &'a mut std::process::Command {
    command
        .env("BOB_DIR", &env.vault)
        .env("BOB_NOW", SCAN_JSON_NOW)
        .env("XDG_STATE_HOME", &env.state)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &env.lock)
        .env("BOB_REF_SCAN_LOCK_WAIT_SECONDS", "0")
}

fn run_scan(env: &ScanEnv, extra: &[&str]) -> Output {
    let mut command = bob_command();
    command.arg("ref").arg("scan");
    for arg in extra {
        command.arg(arg);
    }
    apply_scan_env(&mut command, env)
        .output()
        .expect("run ref scan")
}

fn spawn_scan(
    env: &ScanEnv,
    extra: &[&str],
    hs: &Handshake,
    pause: &str,
    pause_every: bool,
    plan_log: Option<&Path>,
) -> SpawnedScan {
    let mut command = bob_command();
    command.arg("ref").arg("scan");
    for arg in extra {
        command.arg(arg);
    }
    apply_scan_env(&mut command, env)
        .env("BOB_REF_SCAN_TEST_PAUSE", pause)
        .env("BOB_REF_SCAN_TEST_READY", &hs.ready)
        .env("BOB_REF_SCAN_TEST_GO", &hs.go)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if pause_every {
        command.env("BOB_REF_SCAN_TEST_PAUSE_EVERY", "1");
    }
    if let Some(log) = plan_log {
        command.env("BOB_REF_SCAN_TEST_PLAN_LOG", log);
    }
    SpawnedScan {
        child: Some(command.spawn().expect("spawn ref scan")),
    }
}

fn assert_no_lock_wait(output: &Output) {
    let err = stderr(output);
    assert!(
        !err.contains("waiting for another bob ref scan to finish")
            && !err.contains("waiting for another vault maintenance run"),
        "scan must not wait on either lock:\n{}",
        format_output(output)
    );
}

fn write_textbundle_with_asset(pdf: &Path, title: &str, asset_bytes: &[u8]) {
    write_marker_pdf(pdf, title);
    let bundle = pdf.with_extension("textbundle");
    let sidecar = bundle.join("text.md");
    let asset = bundle.join("assets/figure.png");
    write_file(
        &sidecar,
        &format!(
            "\
## Page 1

- status: wip
- parent: obsidian

---

![{title} figure](assets/figure.png)
"
        ),
    );
    fs::create_dir_all(asset.parent().expect("asset parent"))
        .expect("asset dir");
    fs::write(&asset, asset_bytes).expect("write asset");
}

// --- 1. lock-free no-work under held locks ---

#[test]
fn no_work_scan_succeeds_while_each_lock_is_held() {
    let temp = TempDir::new("bob-cli-scan-interleave-nowork-locks");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/stable.pdf"), "Stable");
    assert_success(&run_scan(&env, &[]));
    let receipt_before =
        fs::read(scan_receipt_path(&env.state)).expect("initial receipt");

    let cases: &[(&str, bool, bool)] = &[
        ("both", true, true),
        ("scan-only", true, false),
        ("maint-only", false, true),
    ];
    for (label, hold_scan, hold_maint) in cases {
        let _scan = hold_scan.then(|| hold_scan_writer_lock(&env.state));
        let _maint = hold_maint.then(|| hold_vault_maintenance_lock(&env.lock));
        let output = run_scan(&env, &["-f", "json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{label} no-work json must succeed:\n{}",
            format_output(&output)
        );
        assert_no_lock_wait(&output);
        let document = scan_json_stdout(&output);
        assert_eq!(document["ok"], true, "{label}");
        assert_eq!(document["summary"]["unchanged"], 1, "{label}");
        assert_eq!(
            fs::read(scan_receipt_path(&env.state)).expect("receipt"),
            receipt_before,
            "{label} must leave the sentinel receipt untouched"
        );
    }
}

#[test]
fn no_work_empty_library_and_annotated_assets_ignore_held_locks() {
    let temp = TempDir::new("bob-cli-scan-interleave-nowork-shapes");
    let empty = ScanEnv {
        vault: temp.path().join("empty"),
        state: temp.path().join("empty-state"),
        lock: temp.path().join("empty.lock"),
    };
    fs::create_dir_all(empty.vault.join("lib")).expect("empty lib");
    let _scan = hold_scan_writer_lock(&empty.state);
    let _maint = hold_vault_maintenance_lock(&empty.lock);
    let output = run_scan(&empty, &["-f", "json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "empty library no-work:\n{}",
        format_output(&output)
    );
    assert_no_lock_wait(&output);
    let document = scan_json_stdout(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["summary"]["pdfs"], 0);
    assert!(
        !scan_receipt_path(&empty.state).exists(),
        "empty no-work must not publish a receipt"
    );

    let annotated = ScanEnv {
        vault: temp.path().join("annotated"),
        state: temp.path().join("annotated-state"),
        lock: temp.path().join("annotated.lock"),
    };
    let pdf = annotated.vault.join("lib/books/figures.pdf");
    write_textbundle_with_asset(&pdf, "Figures", b"synthetic png bytes");
    assert_success(&run_scan(&annotated, &[]));
    let note = annotated.vault.join("ref/books/figures.md");
    let assets = annotated.vault.join("ref/books/figures.assets");
    let note_before = fs::read(&note).expect("note");
    let receipt_before =
        fs::read(scan_receipt_path(&annotated.state)).expect("receipt");
    assert!(assets.is_dir(), "first scan copies image assets");
    let _scan = hold_scan_writer_lock(&annotated.state);
    let _maint = hold_vault_maintenance_lock(&annotated.lock);
    let output = run_scan(&annotated, &["-f", "json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "annotated no-work:\n{}",
        format_output(&output)
    );
    assert_no_lock_wait(&output);
    let document = scan_json_stdout(&output);
    assert_eq!(document["summary"]["unchanged"], 1);
    assert_eq!(fs::read(&note).expect("note after"), note_before);
    assert_eq!(
        fs::read(scan_receipt_path(&annotated.state)).expect("receipt after"),
        receipt_before
    );
}

// --- 2. planning stays unlocked; parser does not rerun under locks ---

#[test]
fn planning_pause_lets_maintenance_writer_finish_without_replanning() {
    let temp = TempDir::new("bob-cli-scan-interleave-planning");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/stable.pdf"), "Stable");
    write_marker_pdf(&env.vault.join("lib/chat/fresh.pdf"), "Fresh");
    assert_success(
        &bob_command()
            .arg("ref")
            .arg("sync")
            .arg(env.vault.join("lib/chat/stable.pdf"))
            .env("BOB_DIR", &env.vault)
            .env("XDG_STATE_HOME", &env.state)
            .env("BOB_VAULT_SYNC_LOCK_FILE", &env.lock)
            .output()
            .expect("converge stable pdf"),
    );
    let hs = Handshake::new(temp.path(), "hs");
    let plan_log = temp.path().join("plan.log");
    let sentinel = temp.path().join("maint-writer");
    let child = spawn_scan(
        &env,
        &["-f", "json"],
        &hs,
        "planning",
        false,
        Some(&plan_log),
    );
    wait_pause(&hs, 1);
    {
        let _held = hold_vault_maintenance_lock(&env.lock);
        fs::write(&sentinel, "wrote").expect("maintenance writer finished");
    }
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(0),
        "scan after unlocked planning:\n{}",
        format_output(&output)
    );
    assert!(sentinel.is_file(), "maintenance writer completed mid-plan");
    assert!(env.vault.join("ref/chat/fresh.md").is_file());
    let log = fs::read_to_string(&plan_log).expect("plan log");
    assert!(
        log.lines().any(|line| line.starts_with("locked=0 count=2")),
        "unlocked planning must cover both PDFs:\n{log}"
    );
    assert!(
        log.lines().all(|line| line.starts_with("locked=0")),
        "existing library PDFs must not be parsed again under the locks:\n{log}"
    );
}

// --- 3. lock_busy / scan_busy with real work: no mutation, no receipt ---

fn pending_intake_env(prefix: &str) -> (TempDir, ScanEnv, PathBuf, PathBuf) {
    let temp = TempDir::new(prefix);
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    let source = env.vault.join("xlib/chat/queued.pdf");
    write_marker_pdf(&source, "Queued");
    let note = env.vault.join("ref/chat/queued.md");
    (temp, env, source, note)
}

#[test]
fn pending_work_lock_busy_beats_scan_busy_and_writes_nothing() {
    let (_temp, env, source, note) =
        pending_intake_env("bob-cli-scan-interleave-lock-order");
    let _scan = hold_scan_writer_lock(&env.state);
    let _maint = hold_vault_maintenance_lock(&env.lock);
    let output = run_scan(&env, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let document = scan_json_stdout(&output);
    assert_eq!(document["error"]["code"], "lock_busy");
    assert_eq!(
        document["error"]["hint"],
        "vault-sync, nightly, or rename held the maintenance lock; wait for it to finish, then scan again"
    );
    assert!(
        document["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains(path_str(&env.lock)),
        "lock_busy names the lock path:\n{}",
        format_output(&output)
    );
    assert!(source.is_file(), "intake must not move on lock_busy");
    assert!(!note.exists(), "note must not be written on lock_busy");
    assert!(
        !scan_receipt_path(&env.state).exists(),
        "lock_busy must not write a receipt"
    );
}

#[test]
fn pending_work_scan_busy_writes_nothing() {
    let (_temp, env, source, note) =
        pending_intake_env("bob-cli-scan-interleave-scan-busy");
    let _scan = hold_scan_writer_lock(&env.state);
    let output = run_scan(&env, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let document = scan_json_stdout(&output);
    assert_eq!(document["error"]["code"], "scan_busy");
    assert_eq!(
        document["error"]["message"],
        "another bob ref scan is still running"
    );
    assert_eq!(
        document["error"]["hint"],
        "wait for it to finish, then scan again"
    );
    assert!(source.is_file());
    assert!(!note.exists());
    assert!(!scan_receipt_path(&env.state).exists());
}

#[test]
fn pending_work_human_lock_errors_match_json() {
    let (_temp, env, source, _note) =
        pending_intake_env("bob-cli-scan-interleave-human-busy");
    let _maint = hold_vault_maintenance_lock(&env.lock);
    let output = run_scan(&env, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let err = stderr(&output);
    assert!(
        err.contains("waiting for another vault maintenance run…")
            && err.contains("could not acquire the vault maintenance lock")
            && err.contains(path_str(&env.lock)),
        "human lock_busy shape:\n{}",
        format_output(&output)
    );
    assert!(source.is_file());
    assert!(!scan_receipt_path(&env.state).exists());
}

// --- 4. stale inputs between prepare and acquire ---

#[test]
fn stale_inputs_after_prepare_replans_or_refuses() {
    let temp = TempDir::new("bob-cli-scan-interleave-stale-inputs");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_area_note(&env.vault.join("obsidian.md"), "Obsidian");
    let pdf = env.vault.join("lib/chat/live.pdf");
    write_textbundle_with_asset(&pdf, "Live", b"asset-v1");
    assert_success(&run_scan(&env, &[]));
    let note = env.vault.join("ref/chat/live.md");
    let original = fs::read_to_string(&note).expect("note");
    write_file(&note, &original.replace("title: Live", "title: Live Edit"));

    let hs = Handshake::new(temp.path(), "hs");
    let hook_log = temp.path().join("hook.log");
    let script = temp.path().join("pre-scan");
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf hook >> {}\n",
            shell_single_quote(path_str(&hook_log))
        ),
    );
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let child = {
        let mut command = bob_command();
        command.arg("ref").arg("scan").arg("-f").arg("json");
        apply_scan_env(&mut command, &env)
            .env("BOB_CONFIG_FILE", &config)
            .env("BOB_REF_SCAN_TEST_PAUSE", "after_prepare")
            .env("BOB_REF_SCAN_TEST_READY", &hs.ready)
            .env("BOB_REF_SCAN_TEST_GO", &hs.go)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        SpawnedScan {
            child: Some(command.spawn().expect("spawn")),
        }
    };
    wait_pause(&hs, 1);
    write_marker_pdf(&pdf, "Live annotations changed");
    write_file(&note, &format!("{original}\n# extra heading\n"));
    write_file(
        &pdf.with_extension("textbundle").join("text.md"),
        "\
## Page 1

- status: wip
- parent: obsidian

---

> sidecar changed
",
    );
    write_marker_pdf(&env.vault.join("lib/chat/added.pdf"), "Added");
    let parent = fs::read_to_string(env.vault.join("obsidian.md"))
        .unwrap_or_else(|_| {
            "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n".into()
        });
    if let Some(task) = parent.lines().find(|line| line.contains("#ref")) {
        write_file(
            &env.vault.join("elsewhere.md"),
            &format!("---\ntype: \"[[area]]\"\n---\n\n# Elsewhere\n\n{task}\n"),
        );
        let stripped = parent
            .lines()
            .filter(|line| !line.contains("#ref"))
            .collect::<Vec<_>>()
            .join("\n");
        write_file(&env.vault.join("obsidian.md"), &format!("{stripped}\n"));
    }
    resume(&hs, 1);
    let output = child.wait();
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "stale inputs must replan or refuse, never hang:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    if document.get("error").is_some() {
        assert_eq!(
            document["error"]["code"],
            "scan_failed",
            "{}",
            format_output(&output)
        );
        assert!(
            document["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("scan inputs changed"),
            "persistent churn names a rerun:\n{}",
            format_output(&output)
        );
    } else {
        assert_eq!(
            document["ok"],
            output.status.success(),
            "report envelope ok must match the exit:\n{}",
            format_output(&output)
        );
        assert!(
            env.vault.join("ref/chat/live.md").is_file(),
            "fresh plan must keep a well-formed note:\n{}",
            format_output(&output)
        );
        let created = document["summary"]["created"].as_u64().unwrap_or(0);
        let updated = document["summary"]["updated"].as_u64().unwrap_or(0);
        let failures = document["summary"]["failures"].as_u64().unwrap_or(0);
        assert!(
            created + updated + failures > 0,
            "edits must not be classified stale no-work:\n{}",
            format_output(&output)
        );
    }
    let hook = fs::read_to_string(&hook_log).unwrap_or_default();
    assert_eq!(hook.matches("hook").count(), 1, "hook runs once: {hook}");
}

#[test]
fn repeated_churn_after_prepare_terminates() {
    let temp = TempDir::new("bob-cli-scan-interleave-churn");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/churn.pdf"), "Churn");
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "before_lock", true, None);
    wait_pause(&hs, 1);
    write_marker_pdf(&env.vault.join("lib/chat/churn.pdf"), "Churn v2");
    resume(&hs, 1);
    wait_pause(&hs, 2);
    write_marker_pdf(&env.vault.join("lib/chat/churn.pdf"), "Churn v3");
    resume(&hs, 2);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(1),
        "repeated churn must terminate:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    assert_eq!(document["error"]["code"], "scan_failed");
    assert!(
        document["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("rerun bob ref scan"),
        "{}",
        format_output(&output)
    );
    assert!(
        !scan_receipt_path(&env.state).exists(),
        "abandoned retry must not publish an intermediate receipt"
    );
}

// --- 5. two scans, one intake; paired note stays locked ---

#[test]
fn two_scans_one_queued_pdf_only_one_intakes() {
    let temp = TempDir::new("bob-cli-scan-interleave-two-scans");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    let source = env.vault.join("xlib/chat/race.pdf");
    write_marker_pdf(&source, "Race");
    write_file(
        &source.with_extension("md"),
        "\
## Page 1

- status: wip
- parent: obsidian

---

> sidecar quote.
",
    );
    write_file(&source.with_extension("mp3"), "fake-mp3");
    let hs_a = Handshake::new(temp.path(), "a");
    let hs_b = Handshake::new(temp.path(), "b");
    let a =
        spawn_scan(&env, &["-f", "json"], &hs_a, "after_prepare", false, None);
    let b =
        spawn_scan(&env, &["-f", "json"], &hs_b, "after_prepare", false, None);
    wait_pause(&hs_a, 1);
    wait_pause(&hs_b, 1);
    resume(&hs_a, 1);
    let out_a = a.wait();
    assert_eq!(
        out_a.status.code(),
        Some(0),
        "first scan:\n{}",
        format_output(&out_a)
    );
    resume(&hs_b, 1);
    let out_b = b.wait();
    assert_eq!(
        out_b.status.code(),
        Some(0),
        "second scan:\n{}",
        format_output(&out_b)
    );
    let dest = env.vault.join("lib/chat/race.pdf");
    let note = env.vault.join("ref/chat/race.md");
    assert!(dest.is_file());
    assert!(dest.with_extension("md").is_file());
    assert!(dest.with_extension("mp3").is_file());
    assert!(note.is_file());
    assert!(!source.exists());
    let doc_a = scan_json_stdout(&out_a);
    let doc_b = scan_json_stdout(&out_b);
    let created_a = doc_a["summary"]["created"].as_u64().unwrap_or(0);
    let created_b = doc_b["summary"]["created"].as_u64().unwrap_or(0);
    assert_eq!(
        created_a + created_b,
        1,
        "exactly one scan creates the note:\nA {}\nB {}",
        format_output(&out_a),
        format_output(&out_b)
    );
}

#[test]
fn two_scans_late_audio_moves_once() {
    let temp = TempDir::new("bob-cli-scan-interleave-late-audio");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/listen.pdf"), "Listen");
    assert_success(&run_scan(&env, &[]));
    write_file(&env.vault.join("xlib/chat/listen.mp3"), "late-audio");
    let hs_a = Handshake::new(temp.path(), "a");
    let hs_b = Handshake::new(temp.path(), "b");
    let a =
        spawn_scan(&env, &["-f", "json"], &hs_a, "after_prepare", false, None);
    let b =
        spawn_scan(&env, &["-f", "json"], &hs_b, "after_prepare", false, None);
    wait_pause(&hs_a, 1);
    wait_pause(&hs_b, 1);
    resume(&hs_a, 1);
    let out_a = a.wait();
    resume(&hs_b, 1);
    let out_b = b.wait();
    assert_eq!(out_a.status.code(), Some(0), "{}", format_output(&out_a));
    assert_eq!(out_b.status.code(), Some(0), "{}", format_output(&out_b));
    assert!(env.vault.join("lib/chat/listen.mp3").is_file());
    assert!(!env.vault.join("xlib/chat/listen.mp3").exists());
}

#[test]
fn intake_move_holds_maintenance_lock_until_note_exists() {
    let temp = TempDir::new("bob-cli-scan-interleave-paired-note");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/pair.pdf"), "Pair");
    let hs = Handshake::new(temp.path(), "hs");
    let child = spawn_scan(
        &env,
        &["-f", "json"],
        &hs,
        "after_intake_move",
        false,
        None,
    );
    wait_pause(&hs, 1);
    assert!(
        env.vault.join("lib/chat/pair.pdf").is_file(),
        "PDF has moved"
    );
    assert!(
        !env.vault.join("ref/chat/pair.md").exists(),
        "note is not filed yet"
    );
    assert!(
        try_lock_exclusive(&env.lock).is_err(),
        "vault-sync-like taker cannot enter until the paired note exists"
    );
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert!(env.vault.join("ref/chat/pair.md").is_file());
}

// --- 6. overwrite recovery before hook; intent during prepare ---

#[test]
fn recovery_runs_under_locks_before_hook() {
    let (temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-scan-interleave-recovery-hook");
    let base = overwrite_recovery::head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    let hook_log = temp.path().join("hook.log");
    let script = temp.path().join("pre-scan");
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\n\
             if grep -q CRASHED ref/blogs/half-written.md; then printf TOO_EARLY >> {log}; fi\n\
             printf HOOK >> {log}\n",
            log = shell_single_quote(path_str(&hook_log))
        ),
    );
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );
    let lock = temp.path().join("bob_sync.lock");
    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("XDG_STATE_HOME", &state)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("BOB_NOW", SCAN_JSON_NOW)
        .output()
        .expect("scan with pending recovery");
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert!(
        stderr(&output)
            .contains("recovered: rolled back an interrupted overwrite"),
        "{}",
        format_output(&output)
    );
    let hook = fs::read_to_string(&hook_log).expect("hook log");
    assert!(
        hook.contains("HOOK") && !hook.contains("TOO_EARLY"),
        "recovery must finish before the hook runs: {hook}"
    );
}

#[test]
fn intent_arriving_during_prepare_invalidates_the_plan() {
    let (temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-scan-interleave-intent-during");
    let base = overwrite_recovery::head(&vault);
    write_marker_pdf(&vault.join("lib/blogs/fresh.pdf"), "Fresh");
    let env = ScanEnv {
        vault: vault.clone(),
        state: state.clone(),
        lock: temp.path().join("bob_sync.lock"),
    };
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "after_prepare", false, None);
    wait_pause(&hs, 1);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("crash the note");
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(0),
        "invalidated plan recovers then continues:\n{}",
        format_output(&output)
    );
    let note = fs::read_to_string(vault.join("ref/blogs/half-written.md"))
        .expect("restored note");
    assert!(
        note.contains("body"),
        "intent arriving during prepare must recover: {note}"
    );
    assert!(
        !overwrite_recovery::intent_path(&state, &vault).exists(),
        "intent is cleared"
    );
    assert!(vault.join("ref/blogs/fresh.md").is_file());
}

// --- 7. human/json/receipt agreement ---

#[test]
fn human_verbose_and_json_no_work_agree_with_chatty_hook() {
    let temp = TempDir::new("bob-cli-scan-interleave-modes");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/stable.pdf"), "Stable");
    assert_success(&run_scan(&env, &[]));
    let receipt_before =
        fs::read(scan_receipt_path(&env.state)).expect("receipt");
    let script = temp.path().join("pre-scan");
    write_executable(
        &script,
        "#!/bin/sh\nprintf 'hook stdout line\\n'\nprintf 'hook stderr line\\n' >&2\n",
    );
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );
    let _scan = hold_scan_writer_lock(&env.state);
    let _maint = hold_vault_maintenance_lock(&env.lock);

    let json = {
        let mut command = bob_command();
        command.arg("ref").arg("scan").arg("-f").arg("json");
        apply_scan_env(&mut command, &env)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .expect("json")
    };
    assert_eq!(json.status.code(), Some(0), "{}", format_output(&json));
    assert_no_lock_wait(&json);
    let document = scan_json_stdout(&json);
    assert_eq!(document["ok"], true);
    assert_eq!(document["summary"]["unchanged"], 1);
    assert!(
        !stdout(&json).contains("hook stdout line"),
        "chatty hook must not corrupt JSON stdout:\n{}",
        format_output(&json)
    );
    assert!(
        stderr(&json).contains("hook stdout line")
            && stderr(&json).contains("hook stderr line"),
        "{}",
        format_output(&json)
    );

    let human = {
        let mut command = bob_command();
        command.arg("ref").arg("scan").arg("--verbose");
        apply_scan_env(&mut command, &env)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .expect("human")
    };
    assert_eq!(human.status.code(), Some(0), "{}", format_output(&human));
    assert_no_lock_wait(&human);
    assert!(
        stdout(&human).contains("notes_unchanged: 1"),
        "{}",
        format_output(&human)
    );
    assert_eq!(
        fs::read(scan_receipt_path(&env.state)).expect("receipt after"),
        receipt_before,
        "no-work modes must not replace the receipt"
    );
}

fn plant_clash_destination(env: &ScanEnv) {
    write_marker_pdf(&env.vault.join("lib/chat/clash.pdf"), "Existing");
}

#[test]
fn prelock_intake_collision_writes_no_receipt() {
    let temp = TempDir::new("bob-cli-scan-interleave-prelock-receipt");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/clash.pdf"), "Clash");
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "after_prepare", false, None);
    wait_pause(&hs, 1);
    plant_clash_destination(&env);
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        !scan_receipt_path(&env.state).exists(),
        "a destination planted before lock acquisition is prelock:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(
        document["error"]["code"],
        "intake_collision",
        "{}",
        format_output(&output)
    );
}

#[test]
fn human_prelock_intake_collision_writes_no_receipt() {
    let temp = TempDir::new("bob-cli-scan-interleave-human-prelock");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/clash.pdf"), "Clash");
    let hs = Handshake::new(temp.path(), "hs");
    let child = spawn_scan(&env, &[], &hs, "after_prepare", false, None);
    wait_pause(&hs, 1);
    plant_clash_destination(&env);
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        !scan_receipt_path(&env.state).exists(),
        "human prelock collision must not write a receipt:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("xlib intake collision"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn post_lock_intake_collision_writes_error_receipt() {
    let temp = TempDir::new("bob-cli-scan-interleave-postlock-receipt");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/clash.pdf"), "Clash");
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "before_intake", false, None);
    wait_pause(&hs, 1);
    plant_clash_destination(&env);
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let text = stdout(&output);
    let receipt = fs::read_to_string(scan_receipt_path(&env.state))
        .expect("post-lock hard failure writes a receipt");
    assert_eq!(
        receipt, text,
        "JSON stdout and the post-lock error receipt must match"
    );
    let document: Value = serde_json::from_str(text.trim()).expect("json");
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "intake_collision");
}

#[test]
fn human_post_lock_intake_collision_writes_error_receipt() {
    let temp = TempDir::new("bob-cli-scan-interleave-human-postlock");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/clash.pdf"), "Clash");
    let hs = Handshake::new(temp.path(), "hs");
    let child = spawn_scan(&env, &[], &hs, "before_intake", false, None);
    wait_pause(&hs, 1);
    plant_clash_destination(&env);
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let receipt: Value = serde_json::from_str(
        &fs::read_to_string(scan_receipt_path(&env.state))
            .expect("human post-lock failure writes a receipt"),
    )
    .expect("receipt json");
    assert_eq!(receipt["ok"], false);
    assert_eq!(receipt["error"]["code"], "intake_collision");
    assert!(
        stderr(&output).contains("xlib intake collision"),
        "{}",
        format_output(&output)
    );
}
