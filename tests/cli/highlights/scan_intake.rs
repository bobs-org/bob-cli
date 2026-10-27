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
) -> String {
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
    stdout(&output)
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("source_markdown:")
                .map(|pointer| pointer.trim().trim_matches('"').to_string())
        })
        .unwrap_or_else(|| {
            panic!("no source_markdown in -k report:\n{}", stdout(&output))
        })
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
    let birth_pointer =
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
    // The `-i` drain must file the newborn note with the exact birth
    // pointer the `-k` create reported, exactly once.
    let filed_note = fs::read_to_string(vault.join("ref/chat/queued_one.md"))
        .expect("read drained note");
    assert!(
        filed_note.lines().any(|line| {
            line.strip_prefix("source_markdown:").is_some_and(|value| {
                value.trim().trim_matches('"') == birth_pointer
            })
        }),
        "drained note must carry the birth pointer:\n{filed_note}"
    );
    assert_eq!(
        filed_note.matches("source_markdown:").count(),
        1,
        "drained note must not duplicate the pointer:\n{filed_note}"
    );
    assert_eq!(
        fs::read(vault.join(&birth_pointer)).expect("read saved Markdown"),
        "# queued_one\n\nQueued reference.\n"
            .to_string()
            .into_bytes(),
        "the drain must commit the saved source bytes"
    );
    let subjects = subjects_since(&vault, &before);
    assert!(
        subjects.contains(&"bob ref scan: ingest chat/queued_one".to_string()),
        "drain must file an ingest commit: {subjects:?}"
    );
    // Phase store: `-k` left the write-once store untracked, so the drain's
    // pre-sync may commit it as a separate vault commit before the ingest.
    // Either the ingest or a vault commit in this range must carry the store.
    let log_files = git_in(
        &vault,
        [
            "log",
            "--name-only",
            "--format=%s",
            &format!("{before}..HEAD"),
        ],
    );
    let log_text = String::from_utf8_lossy(&log_files.stdout).to_string();
    assert!(
        log_text.contains(".bob/ref-sources/chat/queued_one--"),
        "store files must be committed in this range: {log_text}"
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
    assert!(
        changed.contains(&"lib/chat/queued_one.pdf".to_string())
            && changed.contains(&"ref/chat/queued_one.md".to_string())
            && changed.contains(&"sase.md".to_string()),
        "drain commit is missing core files: {changed:?}"
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
    let subjects = subjects_since(&vault, &before);
    assert!(
        subjects.contains(&"bob ref scan: ingest chat/beta".to_string())
            && subjects
                .contains(&"bob ref scan: ingest chat/alpha".to_string()),
        "drain must file both ingest commits: {subjects:?}"
    );
    let log_files = git_in(
        &vault,
        [
            "log",
            "--name-only",
            "--format=%s",
            &format!("{before}..HEAD"),
        ],
    );
    let log_text = String::from_utf8_lossy(&log_files.stdout).to_string();
    assert!(
        log_text.contains(".bob/ref-sources/chat/alpha--")
            && log_text.contains(".bob/ref-sources/chat/beta--"),
        "both stores must be committed in this range: {log_text}"
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

#[test]
fn intake_only_json_drain_writes_matching_receipt() {
    let temp = TempDir::new("bob-cli-scan-intake-json-receipt");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "receipt_drain");

    let output = scan_command(&temp, &vault, &["-i", "-f", "json"])
        .output()
        .expect("run scan --intake-only -f json");
    assert_eq!(
        output.status.code(),
        Some(0),
        "json intake-only drain should succeed:\n{}",
        format_output(&output)
    );
    let text = stdout(&output);
    let receipt_path = temp.path().join("state/bob-cli/ref/last-scan.json");
    let bytes = fs::read(&receipt_path)
        .expect("a writing drain must write the receipt");
    assert_eq!(
        bytes,
        text.to_string().into_bytes(),
        "the receipt must be the exact envelope line from stdout"
    );
    let document: serde_json::Value =
        serde_json::from_str(text.trim()).expect("stdout parses as JSON");
    assert_eq!(document["ok"], true);
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["command"], "ref scan");
    assert_eq!(document["mode"], "write");
    assert_eq!(document["intake_only"], true);
    assert_eq!(document["summary"]["held"], 0);
    assert_eq!(document["held"], serde_json::json!([]));
    assert_eq!(
        document["intake"],
        serde_json::json!([{
            "from": "xlib/chat/receipt_drain.pdf",
            "to": "lib/chat/receipt_drain.pdf",
        }])
    );
}

#[test]
fn intake_only_human_drain_writes_receipt() {
    let temp = TempDir::new("bob-cli-scan-intake-human-receipt");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    queue_pdf(&temp, &vault, &fixture, &pandoc, "human_drain");

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_eq!(
        output.status.code(),
        Some(0),
        "human intake-only drain should succeed:\n{}",
        format_output(&output)
    );
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            temp.path().join("state/bob-cli/ref/last-scan.json"),
        )
        .expect("a writing drain must write the receipt"),
    )
    .expect("receipt parses as JSON");
    assert_eq!(receipt["ok"], true);
    assert_eq!(receipt["command"], "ref scan");
    assert_eq!(receipt["mode"], "write");
    assert_eq!(receipt["intake_only"], true);
    assert_eq!(receipt["summary"]["held"], 0);
    assert_eq!(receipt["held"], serde_json::json!([]));
}

#[test]
fn intake_only_empty_queue_writes_no_receipt() {
    let temp = TempDir::new("bob-cli-scan-intake-empty-receipt");
    let (vault, _peer) = seed_synced_vault(&temp);

    let output = scan_command(&temp, &vault, &["-i", "-f", "json"])
        .output()
        .expect("run scan --intake-only on empty queue");
    assert_eq!(
        output.status.code(),
        Some(0),
        "empty intake-only drain should succeed:\n{}",
        format_output(&output)
    );
    assert!(
        !temp
            .path()
            .join("state/bob-cli/ref/last-scan.json")
            .exists(),
        "an empty drain never takes the lock and writes no receipt"
    );
}

/// Stamp a library PDF whose marker carries URL identity, then file its
/// v1 note and task with `bob ref sync` (the same plan/execute path inline
/// intake uses after the move).
fn sync_library_pdf(temp: &TempDir, vault: &Path, rel: &str, marker: &str) {
    let pdf = vault.join(rel);
    write_highlights_pdf(&pdf, marker);
    let output = bob_command()
        .arg("ref")
        .arg("sync")
        .arg(&pdf)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run ref sync on library PDF");
    assert_success(&output);
}

fn alpha_marker() -> &'static str {
    "- status: ready\n- parent: sase\n- title: Alpha Paper\n- source_url: https://example.com/alpha\n- captured: 2026-09-01\n"
}

/// File v1 of the alpha family and leave its reread queued in `xlib/`.
fn seed_alpha_reread(temp: &TempDir, vault: &Path) {
    sync_library_pdf(temp, vault, "lib/blogs/alpha.pdf", alpha_marker());
    assert!(
        vault.join("ref/blogs/alpha.md").is_file(),
        "ref sync must file the v1 note"
    );
    fs::create_dir_all(vault.join("xlib/blogs")).expect("queue dir");
    fs::copy(
        vault.join("lib/blogs/alpha.pdf"),
        vault.join("xlib/blogs/alpha__v2.pdf"),
    )
    .expect("queue the reread-shaped PDF");
}

#[test]
fn intake_only_files_queued_reread_with_lineage_and_stable_second_scan() {
    let temp = TempDir::new("bob-cli-scan-intake-reread-birth");
    let (vault, _peer) = seed_synced_vault(&temp);
    seed_alpha_reread(&temp, &vault);
    let v1_note =
        fs::read_to_string(vault.join("ref/blogs/alpha.md")).expect("v1 note");
    let v1_pdf = fs::read(vault.join("lib/blogs/alpha.pdf")).expect("v1 pdf");
    let v1_task = fs::read_to_string(vault.join("sase.md"))
        .expect("parent")
        .lines()
        .find(|line| line.contains("ref/blogs/alpha|"))
        .expect("v1 task line")
        .to_string();

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "ingested: lib/blogs/alpha__v2.pdf → ref/blogs/alpha__v2.md"
        ),
        "{report}"
    );
    assert!(report.contains("task → sase.md ^ref-alpha-v2"), "{report}");
    assert!(report.contains("intake: filed 1 · queued 0"), "{report}");

    let note = fs::read_to_string(vault.join("ref/blogs/alpha__v2.md"))
        .expect("read reread note");
    assert!(
        note.contains("earlier:\n  - \"[[ref/blogs/alpha|v1 · 2026-09-01]]\""),
        "birth writes block-style earlier: for the live family member:\n{note}"
    );
    assert!(
        note.contains("![[sase#^ref-alpha-v2]]"),
        "the managed embed names the actual reading task:\n{note}"
    );
    let parent =
        fs::read_to_string(vault.join("sase.md")).expect("read parent");
    assert!(
        parent.contains("[[ref/blogs/alpha__v2|Alpha Paper (v2)]]"),
        "the reading task alias carries the version:\n{parent}"
    );
    assert!(parent.contains("^ref-alpha-v2"), "{parent}");
    assert!(parent.contains(&v1_task), "the v1 task line is untouched");

    // Earlier notes, PDFs, and the v1 task keep their bytes.
    assert_eq!(
        fs::read_to_string(vault.join("ref/blogs/alpha.md")).expect("v1 note"),
        v1_note,
        "no backlinks are written into old notes"
    );
    assert_eq!(
        fs::read(vault.join("lib/blogs/alpha.pdf")).expect("v1 pdf"),
        v1_pdf
    );
    assert!(
        !vault.join("xlib/blogs/alpha__v2.pdf").exists(),
        "the queued source is consumed"
    );

    // A second scan is byte-identical: same task line, same lineage.
    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("rerun scan --intake-only");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("ref/blogs/alpha__v2.md"))
            .expect("reread note"),
        note,
        "repeat sync preserves lineage verbatim"
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("parent"),
        parent,
        "no duplicate reading task"
    );
    assert_eq!(
        parent.matches("^ref-alpha-v2").count(),
        1,
        "exactly one v2 task:\n{parent}"
    );
}

#[test]
fn intake_only_refuses_fabricated_reread_occupants_and_recovers() {
    let temp = TempDir::new("bob-cli-scan-intake-reread-collision");
    let (vault, _peer) = seed_synced_vault(&temp);
    seed_alpha_reread(&temp, &vault);

    // Another host takes the name under a sibling type directory.
    write_file(
        &vault.join("ref/papers/alpha__v2.md"),
        "---\ntitle: Squatter\nstatus: ready\n---\n\n# Squatter\n",
    );
    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a fabricated occupant must keep the item queued:\n{}",
        format_output(&output)
    );
    let report = stdout(&output);
    assert!(
        report.contains("queued: xlib/blogs/alpha__v2.pdf (library collision: reread alpha__v2 is already filed (")
            && report.contains("nothing filed, queued source preserved"),
        "{report}"
    );
    assert!(
        vault.join("xlib/blogs/alpha__v2.pdf").is_file(),
        "the queued source is preserved for recovery"
    );
    assert!(
        !vault.join("lib/blogs/alpha__v2.pdf").exists()
            && !vault.join("ref/blogs/alpha__v2.md").exists(),
        "nothing is filed over the occupant"
    );

    // An id-only collision (same reference id, different filename) also
    // refuses instead of re-using the occupant's name.
    fs::remove_file(vault.join("ref/papers/alpha__v2.md"))
        .expect("clear the other-type occupant");
    write_file(
        &vault.join("ref/blogs/squatter.md"),
        "---\ntitle: Squatter\nid: alpha__v2\nstatus: ready\n---\n\n# Squatter\n",
    );
    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("rerun scan --intake-only");
    assert_eq!(
        output.status.code(),
        Some(1),
        "an id-only occupant must keep the item queued:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).contains("reread alpha__v2 is already filed ("),
        "{}",
        format_output(&output)
    );
    assert!(
        vault.join("xlib/blogs/alpha__v2.pdf").is_file(),
        "the queued source is still preserved"
    );

    // Clearing the occupant recovers with the same lineage as a clean run.
    fs::remove_file(vault.join("ref/blogs/squatter.md"))
        .expect("clear the id-only occupant");
    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("rerun scan --intake-only");
    assert_success(&output);
    let note = fs::read_to_string(vault.join("ref/blogs/alpha__v2.md"))
        .expect("read reread note");
    assert!(
        note.contains("[[ref/blogs/alpha|v1 · 2026-09-01]]"),
        "{note}"
    );
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .expect("parent")
            .contains("^ref-alpha-v2"),
        "recovery files the same task"
    );
}

#[test]
fn intake_only_files_version_looking_pdf_without_predecessor_as_ordinary() {
    let temp = TempDir::new("bob-cli-scan-intake-natural-version");
    let (vault, _peer) = seed_synced_vault(&temp);
    write_highlights_pdf(
        &vault.join("xlib/blogs/protocol__v2.pdf"),
        "- status: ready\n- parent: sase\n- title: Protocol Paper\n- source_url: https://example.com/protocol\n- captured: 2026-09-01\n",
    );

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);
    let note = fs::read_to_string(vault.join("ref/blogs/protocol__v2.md"))
        .expect("read version-looking note");
    assert!(
        !note.contains("earlier:"),
        "a version-looking stem without lineage stays an ordinary birth:\n{note}"
    );
    let parent =
        fs::read_to_string(vault.join("sase.md")).expect("read parent");
    assert!(
        parent.contains("[[ref/blogs/protocol__v2|Protocol Paper]]"),
        "the natural suffix is not family lineage:\n{parent}"
    );
    assert!(parent.contains("^ref-protocol-v2"), "{parent}");
}

#[test]
fn intake_only_preserves_version_in_long_stem_task_ids() {
    let temp = TempDir::new("bob-cli-scan-intake-reread-long-stem");
    let (vault, _peer) = seed_synced_vault(&temp);
    let root = "a-very-long-ref-note-stem-that-keeps-going";
    let title = "A Very Long Ref Note Stem That Keeps Going And Going On And On Past One Hundred Characters For Sure";
    sync_library_pdf(
        &temp,
        &vault,
        &format!("lib/blogs/{root}.pdf"),
        &format!(
            "- status: ready\n- parent: sase\n- title: {title}\n- source_url: https://example.com/long\n- captured: 2026-09-01\n"
        ),
    );
    // An unrelated task already holds the would-be base ID: the reread
    // must suffix after the version, not drop it.
    write_file(
        &vault.join("sase.md"),
        &format!(
            "{}\n- [ ] #task #ref [[ref/blogs/unrelated|Unrelated]] [created::2026-09-01] ^ref-a-very-long-ref-note-stem-that-keeps-v2\n",
            fs::read_to_string(vault.join("sase.md")).expect("parent")
        ),
    );
    fs::create_dir_all(vault.join("xlib/blogs")).expect("queue dir");
    fs::copy(
        vault.join(format!("lib/blogs/{root}.pdf")),
        vault.join(format!("xlib/blogs/{root}__v2.pdf")),
    )
    .expect("queue the long-stem reread");

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("run scan --intake-only");
    assert_success(&output);
    let parent =
        fs::read_to_string(vault.join("sase.md")).expect("read parent");
    let line = parent
        .lines()
        .find(|line| line.contains(&format!("{root}__v2|")))
        .expect("reread task line");
    let id = line
        .rsplit('^')
        .next()
        .expect("task line carries a block id");
    assert!(id.len() <= 44, "task id fits the bound: {id}");
    assert!(
        id.ends_with("-v2-2"),
        "the collision suffix follows the version: {id}"
    );
    assert!(
        line.contains(&" (v2)]] [created::".to_string()),
        "the long-title alias keeps its suffix: {line}"
    );
    let alias = line
        .split('|')
        .nth(1)
        .and_then(|rest| rest.split("]]").next())
        .expect("task alias");
    assert!(
        alias.chars().count() <= 100,
        "alias fits the bound: {alias}"
    );
    let note =
        fs::read_to_string(vault.join(format!("ref/blogs/{root}__v2.md")))
            .expect("read reread note");
    assert!(
        note.contains(&format!("![[sase#^{id}]]")),
        "the embed names the actual task id:\n{note}"
    );
}

#[test]
fn full_scan_files_queued_reread_with_lineage() {
    let temp = TempDir::new("bob-cli-scan-full-reread-birth");
    let (vault, _peer) = seed_synced_vault(&temp);
    seed_alpha_reread(&temp, &vault);

    let output = scan_command(&temp, &vault, &[])
        .output()
        .expect("run full scan");
    assert_success(&output);
    let note = fs::read_to_string(vault.join("ref/blogs/alpha__v2.md"))
        .expect("read reread note");
    assert!(
        note.contains("earlier:\n  - \"[[ref/blogs/alpha|v1 · 2026-09-01]]\""),
        "{note}"
    );
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .expect("parent")
            .contains("[[ref/blogs/alpha__v2|Alpha Paper (v2)]]"),
        "full scan files the same reading task"
    );
}

#[test]
fn full_scan_aborts_before_moving_any_queued_reread_with_an_occupant() {
    let temp = TempDir::new("bob-cli-scan-full-reread-collision");
    let (vault, _peer) = seed_synced_vault(&temp);
    seed_alpha_reread(&temp, &vault);
    let unrelated = b"unrelated queued pdf bytes";
    write_file(
        &vault.join("xlib/chat/unrelated.pdf"),
        "unrelated queued pdf bytes",
    );
    write_file(
        &vault.join("ref/papers/alpha__v2.md"),
        "---\ntitle: Squatter\n---\n",
    );
    let queued = fs::read(vault.join("xlib/blogs/alpha__v2.pdf"))
        .expect("read queued reread bytes");

    let output = scan_command(&temp, &vault, &[])
        .output()
        .expect("run full scan");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let report = stderr(&output);
    assert!(
        report.contains("xlib intake collision(s) detected before writes:")
            && report.contains("ref/papers/alpha__v2.md"),
        "{report}"
    );
    assert_eq!(
        fs::read(vault.join("xlib/blogs/alpha__v2.pdf"))
            .expect("queued reread remains"),
        queued
    );
    assert_eq!(
        fs::read(vault.join("xlib/chat/unrelated.pdf"))
            .expect("unrelated queue remains"),
        unrelated
    );
    assert!(!vault.join("lib/chat/unrelated.pdf").exists());
    assert!(!vault.join("lib/blogs/alpha__v2.pdf").exists());
}

#[test]
fn intake_only_preview_reports_a_queued_reread_occupant() {
    let temp = TempDir::new("bob-cli-scan-intake-reread-preview-collision");
    let (vault, _peer) = seed_synced_vault(&temp);
    seed_alpha_reread(&temp, &vault);
    write_file(
        &vault.join("ref/papers/alpha__v2.md"),
        "---\ntitle: Squatter\n---\n",
    );

    let output = scan_command(&temp, &vault, &["-i", "-d"])
        .output()
        .expect("preview intake");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("would queue: xlib/blogs/alpha__v2.pdf")
            && report.contains("reread alpha__v2 is already filed")
            && report.contains("ref/papers/alpha__v2.md"),
        "{report}"
    );
    assert!(vault.join("xlib/blogs/alpha__v2.pdf").is_file());
    assert!(!vault.join("lib/blogs/alpha__v2.pdf").exists());
}

#[test]
fn intake_only_attaches_audio_to_an_existing_reread() {
    let temp = TempDir::new("bob-cli-scan-intake-reread-audio");
    let (vault, _peer) = seed_synced_vault(&temp);
    seed_alpha_reread(&temp, &vault);
    let filed = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("file queued reread");
    assert_success(&filed);
    write_file(&vault.join("xlib/blogs/alpha__v2.mp3"), "narrated v2");

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("attach audio to reread");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "attached audio: lib/blogs/alpha__v2.mp3 → ref/blogs/alpha__v2.md"
        ),
        "{report}"
    );
    assert!(vault.join("lib/blogs/alpha__v2.mp3").is_file());
    assert!(!vault.join("xlib/blogs/alpha__v2.mp3").exists());
}
