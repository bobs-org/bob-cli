//! `bob ref scan --intake-only` drain tests.

use super::ingest::{create_command, create_fixture, seed_synced_vault};
use crate::support::*;
use fs2::FileExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// `bob ref scan <args>` against `vault`, isolated like `create_command`.
pub(super) fn scan_command(
    temp: &TempDir,
    vault: &Path,
    args: &[&str],
) -> Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("scan")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
        .env("BOB_REF_SCAN_LOCK_WAIT_SECONDS", "0")
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"));
    command
}

fn head(vault: &Path) -> String {
    String::from_utf8(git_in(vault, ["rev-parse", "HEAD"]).stdout)
        .expect("head is utf8")
        .trim()
        .to_string()
}

fn subjects_since(vault: &Path, base: &str) -> Vec<String> {
    let log = git_in(vault, ["log", "--format=%s", &format!("{base}..HEAD")]);
    String::from_utf8_lossy(&log.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

pub(super) fn queue_pdf(
    temp: &TempDir,
    vault: &Path,
    fixture: &Path,
    pandoc: &Path,
    name: &str,
) {
    let source = temp.path().join(format!("{name}.md"));
    write_file(&source, &format!("# {name}\n\nQueued reference.\n"));
    let output = create_command(temp, vault, &source, fixture, pandoc)
        .arg("-k")
        .arg("-N")
        .arg(name)
        .output()
        .expect("queue PDF with create -k");
    assert_success(&output);
    assert!(
        vault.join(format!("xlib/chat/{name}.pdf")).is_file(),
        "create -k must leave the PDF queued"
    );
}

fn assert_scan_json_key_order(output: &Output, start: &str, keys: &[&str]) {
    let text = stdout(output);
    let mut cursor = text
        .find(start)
        .unwrap_or_else(|| panic!("expected `{start}` in:\n{text}"));
    for key in keys {
        let needle = format!("\"{key}\":");
        let relative = text[cursor..].find(&needle).unwrap_or_else(|| {
            panic!("expected `{needle}` after `{start}`:\n{text}")
        });
        cursor += relative + needle.len();
    }
}

#[test]
fn intake_only_drains_a_queued_pdf_into_one_pushed_commit() {
    let temp = TempDir::new("bob-cli-scan-intake-one");
    let (vault, peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "queued_one");
    let before = head(&vault);

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "ingested: lib/chat/queued_one.pdf → ref/chat/queued_one.md · task → sase.md ^ref-queued-one"
        ),
        "{report}"
    );
    assert!(report.contains("sync: committed "), "{report}");
    assert!(report.contains("sync: pushed"), "{report}");
    assert!(report.contains("intake: filed 1 · queued 0"), "{report}");

    assert!(vault.join("lib/chat/queued_one.pdf").is_file());
    assert!(vault.join("ref/chat/queued_one.md").is_file());
    assert!(!vault.join("xlib/chat/queued_one.pdf").exists());
    assert_eq!(
        subjects_since(&vault, &before),
        ["bob ref scan: ingest chat/queued_one"]
    );
    assert!(
        git_in(&vault, ["status", "--porcelain"]).stdout.is_empty(),
        "vault must be clean after the drain"
    );
    let changed = git_in(
        &vault,
        ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    );
    let mut changed = String::from_utf8_lossy(&changed.stdout)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    changed.sort();
    assert_eq!(
        changed,
        [
            "lib/chat/queued_one.pdf",
            "ref/chat/queued_one.md",
            "sase.md"
        ]
    );

    git_in(&peer, ["pull", "-q", "--ff-only"]);
    assert!(peer.join("lib/chat/queued_one.pdf").is_file());
    assert!(peer.join("ref/chat/queued_one.md").is_file());

    let again = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("rerun scan --intake-only");
    assert_success(&again);
    assert_eq!(stdout(&again), "intake: nothing queued\n");
}

#[test]
fn intake_only_files_each_queued_pdf_in_its_own_commit() {
    let temp = TempDir::new("bob-cli-scan-intake-two");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "alpha");
    queue_pdf(&temp, &vault, &fixture, &pandoc, "beta");
    let before = head(&vault);

    let output = scan_command(&temp, &vault, &["--intake-only"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("intake: filed 2 · queued 0"), "{report}");
    assert_eq!(
        subjects_since(&vault, &before),
        [
            "bob ref scan: ingest chat/beta",
            "bob ref scan: ingest chat/alpha"
        ]
    );
    assert!(vault.join("ref/chat/alpha.md").is_file());
    assert!(vault.join("ref/chat/beta.md").is_file());
    assert!(git_in(&vault, ["status", "--porcelain"]).stdout.is_empty());
}

#[test]
fn intake_only_queues_a_stem_taken_by_a_peer_and_exits_nonzero() {
    let temp = TempDir::new("bob-cli-scan-intake-collision");
    let (vault, peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "taken");
    queue_pdf(&temp, &vault, &fixture, &pandoc, "free");
    write_file(&peer.join("lib/chat/taken.pdf"), "peer bytes");
    git_in(&peer, ["add", "lib/chat/taken.pdf"]);
    git_in(&peer, ["commit", "-q", "-m", "peer takes the stem"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a stuck item must fail the run:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains("queued: xlib/chat/taken.pdf (library collision:"),
        "{report}"
    );
    assert!(report.contains("ingested: lib/chat/free.pdf"), "{report}");
    assert!(report.contains("intake: filed 1 · queued 1"), "{report}");
    assert!(vault.join("xlib/chat/taken.pdf").is_file());
    assert_eq!(
        fs::read_to_string(vault.join("lib/chat/taken.pdf"))
            .expect("read peer PDF"),
        "peer bytes",
        "the peer's PDF must never be overwritten"
    );
    assert!(!vault.join("ref/chat/taken.md").exists());
    assert!(vault.join("ref/chat/free.md").is_file());
}

#[test]
fn intake_only_with_an_empty_queue_takes_no_lock_and_exits_zero() {
    let temp = TempDir::new("bob-cli-scan-intake-empty");
    let (vault, _peer) = seed_synced_vault(&temp);
    let sync_lock = temp.path().join("bob_sync.lock");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&sync_lock)
        .expect("open shared lock");
    held.lock_exclusive().expect("hold shared lock");
    let before = head(&vault);

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);
    assert_eq!(stdout(&output), "intake: nothing queued\n");
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    assert_eq!(head(&vault), before);

    let json = scan_command(&temp, &vault, &["-i", "-f", "json"])
        .output()
        .expect("run scan --intake-only json");
    assert_success(&json);
    let document: serde_json::Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse JSON");
    assert_eq!(document["ok"], true);
    assert_eq!(document["intake_only"], true);
    assert_eq!(document["queued"], serde_json::json!([]));
    assert_eq!(document["hook"]["status"], "skipped");
}

#[test]
fn intake_only_fails_with_lock_busy_when_work_waits_behind_the_lock() {
    let temp = TempDir::new("bob-cli-scan-intake-lock");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "behind_lock");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(temp.path().join("bob_sync.lock"))
        .expect("open shared lock");
    held.lock_exclusive().expect("hold shared lock");

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("lock busy"),
        "{}",
        format_output(&output)
    );
    assert!(vault.join("xlib/chat/behind_lock.pdf").is_file());
    assert!(!vault.join("lib/chat/behind_lock.pdf").exists());

    let json = scan_command(&temp, &vault, &["-i", "-f", "json"])
        .output()
        .expect("run scan --intake-only json");
    assert_eq!(json.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse JSON");
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "lock_busy");
    assert_eq!(document["intake_only"], true);
    assert_eq!(document["queued"], serde_json::json!([]));
    assert_scan_json_key_order(
        &json,
        "{",
        &[
            "ok",
            "schema_version",
            "command",
            "generated_at",
            "mode",
            "write_pdfs",
            "intake",
            "error",
            "intake_only",
            "queued",
        ],
    );
}

#[test]
fn intake_only_hard_fails_when_pre_sync_cannot_pull() {
    let temp = TempDir::new("bob-cli-scan-intake-offline");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "offline");
    git_in(
        &vault,
        [
            "remote",
            "set-url",
            "origin",
            path_str(&temp.path().join("unreachable.git")),
        ],
    );

    let output = scan_command(&temp, &vault, &["-i", "-f", "json"])
        .output()
        .expect("run scan --intake-only json");
    assert_eq!(output.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(document["error"]["code"], "pre_sync_failed");
    assert!(vault.join("xlib/chat/offline.pdf").is_file());
    assert!(!vault.join("lib/chat/offline.pdf").exists());
}

#[test]
fn intake_only_conflicts_with_write_pdfs() {
    let temp = TempDir::new("bob-cli-scan-intake-usage");
    let (vault, _peer) = seed_synced_vault(&temp);
    for args in [["-i", "-w"], ["--intake-only", "--write-pdfs"]] {
        let output = scan_command(&temp, &vault, &args)
            .output()
            .expect("run conflicting flags");
        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        assert!(
            stderr(&output).contains("cannot be used with"),
            "{}",
            stderr(&output)
        );
    }
}

#[test]
fn intake_only_dry_run_previews_without_locks_or_writes() {
    let temp = TempDir::new("bob-cli-scan-intake-dry");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "preview");
    let before = head(&vault);
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(temp.path().join("bob_sync.lock"))
        .expect("open shared lock");
    held.lock_exclusive().expect("hold shared lock");

    let output = scan_command(&temp, &vault, &["-i", "-d"])
        .output()
        .expect("run scan --intake-only --dry-run");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "would ingest: xlib/chat/preview.pdf → lib/chat/preview.pdf → ref/chat/preview.md"
        ),
        "{report}"
    );
    assert!(
        report.contains("intake: would file 1 · would queue 0"),
        "{report}"
    );
    assert!(vault.join("xlib/chat/preview.pdf").is_file());
    assert!(!vault.join("lib/chat/preview.pdf").exists());
    assert_eq!(head(&vault), before);

    let json = scan_command(&temp, &vault, &["-i", "-d", "-f", "json"])
        .output()
        .expect("run scan --intake-only --dry-run json");
    assert_success(&json);
    let document: serde_json::Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse JSON");
    assert_eq!(document["mode"], "dry_run");
    assert_eq!(document["ok"], true);
    assert_eq!(document["intake_only"], true);
    assert_eq!(document["queued"], serde_json::json!([]));
    assert_eq!(
        document["intake"],
        serde_json::json!([
            {"from": "xlib/chat/preview.pdf", "to": "lib/chat/preview.pdf"}
        ])
    );
    assert_eq!(document["hook"]["status"], "skipped");
}

#[test]
fn intake_only_json_dry_run_lists_would_queue_items_with_ok_true() {
    let temp = TempDir::new("bob-cli-scan-intake-dry-queue");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "taken");
    write_file(&vault.join("lib/chat/taken.pdf"), "occupied");

    let json = scan_command(&temp, &vault, &["-i", "-d", "-f", "json"])
        .output()
        .expect("run scan --intake-only --dry-run json");
    assert_success(&json);
    let document: serde_json::Value =
        serde_json::from_str(stdout(&json).trim()).expect("parse JSON");
    assert_eq!(document["mode"], "dry_run");
    assert_eq!(document["ok"], true);
    assert_eq!(document["intake_only"], true);
    let queued = document["queued"].as_array().expect("queued array");
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0]["pdf"], "xlib/chat/taken.pdf");
    assert_eq!(queued[0]["reason"], "collision");
    assert!(vault.join("xlib/chat/taken.pdf").is_file());
}

#[test]
fn intake_only_json_reports_filed_notes_and_queued_items() {
    let temp = TempDir::new("bob-cli-scan-intake-json");
    let (vault, peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "filed");
    queue_pdf(&temp, &vault, &fixture, &pandoc, "stuck");
    write_file(&peer.join("lib/chat/stuck.pdf"), "peer bytes");
    git_in(&peer, ["add", "lib/chat/stuck.pdf"]);
    git_in(&peer, ["commit", "-q", "-m", "peer takes the stem"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = scan_command(&temp, &vault, &["-i", "-f", "json"])
        .output()
        .expect("run scan --intake-only json");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 1, "{text}");
    let document: serde_json::Value =
        serde_json::from_str(text.trim()).expect("parse JSON");
    assert_scan_json_key_order(
        &output,
        "{",
        &[
            "ok",
            "schema_version",
            "command",
            "generated_at",
            "mode",
            "write_pdfs",
            "hook",
            "intake",
            "summary",
            "notes",
            "failures",
            "intake_only",
            "queued",
        ],
    );
    assert_scan_json_key_order(
        &output,
        "\"queued\":[",
        &["pdf", "reason", "message"],
    );
    assert_eq!(document["ok"], false);
    assert_eq!(document["mode"], "write");
    assert_eq!(document["write_pdfs"], false);
    assert_eq!(
        document["hook"],
        serde_json::json!({"status": "skipped", "command": null})
    );
    assert_eq!(
        document["intake"],
        serde_json::json!([
            {"from": "xlib/chat/filed.pdf", "to": "lib/chat/filed.pdf"}
        ])
    );
    assert_eq!(document["notes"][0]["action"], "create");
    assert_eq!(document["notes"][0]["path"], "ref/chat/filed.md");
    assert_eq!(document["notes"][0]["source_pdf"], "lib/chat/filed.pdf");
    assert_eq!(document["failures"], serde_json::json!([]));
    assert_eq!(document["summary"]["pdfs"], 2);
    assert_eq!(document["summary"]["created"], 1);
    assert_eq!(document["summary"]["failures"], 0);
    let queued = document["queued"].as_array().expect("queued array");
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0]["pdf"], "xlib/chat/stuck.pdf");
    assert_eq!(queued[0]["reason"], "collision");
    assert!(
        queued[0]["message"]
            .as_str()
            .expect("queued message")
            .contains("destination already exists"),
        "{queued:?}"
    );
}

#[test]
fn intake_only_attaches_late_audio_to_a_library_pdf() {
    let temp = TempDir::new("bob-cli-scan-intake-audio");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("report.md");
    write_file(&source, "# Report\n");
    let created = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .output()
        .expect("create and ingest report");
    assert_success(&created);
    assert!(vault.join("lib/chat/report.pdf").is_file());
    write_file(&vault.join("xlib/chat/report.mp3"), "late audio");
    let before = head(&vault);

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "attached audio: lib/chat/report.mp3 → ref/chat/report.md"
        ),
        "{report}"
    );
    assert!(report.contains("intake: filed 1 · queued 0"), "{report}");
    assert!(vault.join("lib/chat/report.mp3").is_file());
    assert!(!vault.join("xlib/chat/report.mp3").exists());
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read note");
    assert!(note.contains("audio:"), "{note}");
    assert_eq!(
        subjects_since(&vault, &before),
        ["bob ref scan: attach audio chat/report"]
    );
    assert!(git_in(&vault, ["status", "--porcelain"]).stdout.is_empty());
}

#[test]
fn intake_only_queues_late_audio_when_the_marker_needs_a_pdf_write() {
    let temp = TempDir::new("bob-cli-scan-intake-needs-write-pdfs");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("report.md");
    write_file(&source, "# Report\n");
    let created = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .output()
        .expect("create and ingest report");
    assert_success(&created);
    let note_path = vault.join("ref/chat/report.md");
    let note = fs::read_to_string(&note_path).expect("read note");
    let edited = note.replacen("status: ready", "status: next", 1);
    assert_ne!(edited, note, "note must contain status: ready to edit");
    write_file(&note_path, &edited);
    git_in(&vault, ["add", "ref/chat/report.md"]);
    git_in(&vault, ["commit", "-q", "-m", "edit status"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    write_file(&vault.join("xlib/chat/report.mp3"), "late audio");
    let before = head(&vault);

    let output = scan_command(&temp, &vault, &["-i", "-f", "json"])
        .output()
        .expect("run scan --intake-only json");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let document: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(document["ok"], false);
    let queued = document["queued"].as_array().expect("queued array");
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0]["pdf"], "xlib/chat/report.mp3");
    assert_eq!(queued[0]["reason"], "needs_write_pdfs");
    assert!(vault.join("xlib/chat/report.mp3").is_file());
    assert!(!vault.join("lib/chat/report.mp3").exists());
    assert!(git_in(&vault, ["status", "--porcelain"]).stdout.is_empty());
    assert_eq!(head(&vault), before);
}

#[test]
fn intake_only_leaves_a_library_pdf_without_a_note_alone() {
    let temp = TempDir::new("bob-cli-scan-intake-lib-only");
    let (vault, _peer) = seed_synced_vault(&temp);
    let orphan: PathBuf = vault.join("lib/chat/orphan.pdf");
    write_highlights_pdf_pages(&orphan, &[&[]]);
    git_in(&vault, ["add", "lib/chat/orphan.pdf"]);
    git_in(&vault, ["commit", "-q", "-m", "orphan PDF"]);
    let before = head(&vault);

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);
    assert_eq!(stdout(&output), "intake: nothing queued\n");
    assert!(!vault.join("ref/chat/orphan.md").exists());
    assert_eq!(head(&vault), before);
}

/// The per-host intake status record under the test's `XDG_STATE_HOME`.
fn intake_status(temp: &TempDir) -> serde_json::Value {
    let path = temp.path().join("state/bob-cli/ref/intake-status.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).expect("parse intake status")
}

fn doctor_report(temp: &TempDir, vault: &Path) -> String {
    let output = bob_command()
        .args(["ref", "doctor", "--no-hooks"])
        .env("BOB_DIR", vault)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run bob ref doctor");
    stdout(&output)
}

#[test]
fn intake_status_records_drain_success_and_doctor_shows_it() {
    let temp = TempDir::new("bob-cli-scan-intake-status-ok");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let before = doctor_report(&temp, &vault);
    assert!(before.contains("intake_last: none recorded\n"), "{before}");
    queue_pdf(&temp, &vault, &fixture, &pandoc, "status_ok");

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);

    let status = intake_status(&temp);
    assert_eq!(status["schema_version"], 1);
    assert_eq!(status["last_success"]["command"], "bob ref scan -i");
    assert_eq!(status["last_success"]["filed"], 1);
    assert!(status.get("last_failure").is_none(), "{status}");
    let report = doctor_report(&temp, &vault);
    assert!(report.contains("xlib_pending: 0\n"), "{report}");
    assert!(report.contains("intake_last: ok "), "{report}");
    assert!(report.contains("(bob ref scan -i, 1 filed)"), "{report}");
}

#[test]
fn intake_status_records_a_queued_drain_as_a_failure() {
    let temp = TempDir::new("bob-cli-scan-intake-status-fail");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "status_busy");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(temp.path().join("bob_sync.lock"))
        .expect("open shared lock");
    held.lock_exclusive().expect("hold shared lock");

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_eq!(output.status.code(), Some(1));

    let status = intake_status(&temp);
    assert_eq!(status["last_failure"]["command"], "bob ref scan -i");
    assert_eq!(status["last_failure"]["reason"], "lock_busy");
    assert!(status.get("last_success").is_none(), "{status}");
    let report = doctor_report(&temp, &vault);
    assert!(report.contains("xlib_pending: 1 · oldest "), "{report}");
    assert!(report.contains("intake_last: failure "), "{report}");
    assert!(report.contains("(lock_busy: "), "{report}");
}

#[test]
fn intake_status_records_create_outcomes_and_keeps_the_other_field() {
    let temp = TempDir::new("bob-cli-scan-intake-status-create");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);

    let busy_source = temp.path().join("busy.md");
    write_file(&busy_source, "# busy\n\nBody.\n");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(temp.path().join("bob_sync.lock"))
        .expect("open shared lock");
    held.lock_exclusive().expect("hold shared lock");
    let output = create_command(&temp, &vault, &busy_source, &fixture, &pandoc)
        .arg("-N")
        .arg("busy")
        .output()
        .expect("run create while locked");
    drop(held);
    assert!(
        vault.join("xlib/chat/busy.pdf").is_file(),
        "{}",
        format_output(&output)
    );
    let status = intake_status(&temp);
    assert_eq!(status["last_failure"]["command"], "bob ref create");
    assert_eq!(status["last_failure"]["reason"], "lock_busy");

    let ok_source = temp.path().join("fine.md");
    write_file(&ok_source, "# fine\n\nBody.\n");
    let output = create_command(&temp, &vault, &ok_source, &fixture, &pandoc)
        .arg("-N")
        .arg("fine")
        .output()
        .expect("run create");
    assert_success(&output);
    let status = intake_status(&temp);
    assert_eq!(status["last_success"]["command"], "bob ref create");
    assert_eq!(status["last_success"]["filed"], 1);
    assert_eq!(
        status["last_failure"]["reason"], "lock_busy",
        "a success must not erase the recorded failure"
    );
}

#[test]
fn intake_status_records_a_writing_full_scan_but_not_a_dry_run() {
    let temp = TempDir::new("bob-cli-scan-intake-status-full");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "full_scan");

    let dry = scan_command(&temp, &vault, &["--dry-run", "--no-hooks"])
        .output()
        .expect("run dry-run scan");
    assert_success(&dry);
    assert!(
        !temp
            .path()
            .join("state/bob-cli/ref/intake-status.json")
            .exists(),
        "a dry run must record nothing"
    );

    let output = scan_command(&temp, &vault, &["--no-hooks"])
        .output()
        .expect("run full scan");
    assert_success(&output);
    let status = intake_status(&temp);
    assert_eq!(status["last_success"]["command"], "bob ref scan");
    assert_eq!(status["last_success"]["filed"], 1);
}
