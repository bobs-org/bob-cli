//! Create-time intake integration tests.

use crate::support::*;
use fs2::FileExt;
use std::{fs, path::Path, process::Command};

pub(super) fn create_fixture(
    temp: &TempDir,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let pdf = temp.path().join("rendered.pdf");
    write_highlights_pdf_pages(&pdf, &[&[]]);
    let pandoc = temp.path().join("pandoc");
    write_executable(
        &pandoc,
        "#!/bin/sh\nout=\nprev=\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncp \"$BOB_TEST_RENDERED_PDF\" \"$out\"\n",
    );
    (pdf, pandoc)
}

pub(super) fn seed_parent(vault: &Path) {
    write_file(
        &vault.join("sase.md"),
        "---\ntype: \"[[area]]\"\n---\n\n# SASE\n",
    );
    write_file(&vault.join(".gitignore"), "/xlib/\n");
}

pub(super) fn create_command(
    temp: &TempDir,
    vault: &Path,
    source: &Path,
    fixture: &Path,
    pandoc: &Path,
) -> Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("create")
        .arg(source)
        .arg("-P")
        .arg("sase")
        .arg("-b")
        .arg(vault)
        .env("BOB_DIR", vault)
        .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "1")
        .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("BOB_PANDOC_COMMAND", pandoc)
        .env("BOB_TEST_RENDERED_PDF", fixture);
    command
}

pub(super) fn seed_synced_vault(
    temp: &TempDir,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let (vault, _remote, peer) = init_vault_sync_pair(temp);
    seed_parent(&vault);
    git_in(&vault, ["add", ".gitignore", "sase.md"]);
    git_in(&vault, ["commit", "-q", "-m", "seed parent"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only"]);
    (vault, peer)
}

#[test]
fn create_ingests_pdf_audio_note_and_parent_as_one_scoped_pushed_commit() {
    let temp = TempDir::new("bob-cli-create-ingest-happy");
    let (vault, peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("report.md");
    let audio = temp.path().join("report.mp3");
    write_file(&source, "# Report\n\nCaptured report.\n");
    write_file(&audio, "mp3 bytes");
    let before =
        String::from_utf8(git_in(&vault, ["rev-parse", "HEAD"]).stdout)
            .expect("head is utf8");
    let before = before.trim();

    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("--audio")
        .arg(&audio)
        .output()
        .expect("run create with intake");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ingested: lib/chat/report.pdf → ref/chat/report.md"),
        "{report}"
    );
    assert!(report.contains("task → sase.md ^ref-report"), "{report}");
    assert!(
        report.contains("sync: committed ") && report.contains("pushed"),
        "{report}"
    );

    let library_pdf = vault.join("lib/chat/report.pdf");
    let library_audio = vault.join("lib/chat/report.mp3");
    let note = vault.join("ref/chat/report.md");
    assert!(library_pdf.is_file());
    assert!(library_audio.is_file());
    assert!(note.is_file());
    assert!(!vault.join("xlib/chat/report.pdf").exists());
    assert!(!vault.join("xlib/chat/report.mp3").exists());
    let parent_contents =
        fs::read_to_string(vault.join("sase.md")).expect("read parent note");
    assert!(
        parent_contents.contains("[[ref/chat/report|Report]]"),
        "{parent_contents}"
    );
    let status = git_in(&vault, ["status", "--porcelain"]);
    assert!(status.stdout.is_empty(), "vault must be clean after intake");

    let head = String::from_utf8(git_in(&vault, ["rev-parse", "HEAD"]).stdout)
        .expect("head is utf8");
    let head = head.trim();
    assert_ne!(head, before, "intake must create commits");
    let parents = git_in(
        &vault,
        ["rev-list", "--count", &format!("{before}..{head}")],
    );
    let count = String::from_utf8_lossy(&parents.stdout).trim().to_string();
    assert!(
        count == "1" || count == "2",
        "intake must create one ingest commit plus an optional pre-sync store commit: {count}"
    );
    let subject = git_in(&vault, ["log", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "bob ref create: ingest chat/report"
    );
    // Phase store: the scoped ingest commit (or pre-sync) contains both
    // store files. Check the whole range since pre-sync may have committed
    // them separately (adding them to ingest is then a no-op per spec).
    let range_files = git_in(
        &vault,
        [
            "log",
            "--name-only",
            "--format=",
            &format!("{before}..{head}"),
        ],
    );
    let range_text = String::from_utf8_lossy(&range_files.stdout).to_string();
    for required in [
        "lib/chat/report.mp3",
        "lib/chat/report.pdf",
        "ref/chat/report.md",
        "sase.md",
    ] {
        assert!(
            range_text.lines().any(|line| line.trim() == required),
            "range is missing {required}: {range_text}"
        );
    }
    assert!(
        range_text.contains(".bob/ref-sources/chat/report--")
            && range_text.contains("/source.md")
            && range_text.contains("/code.json"),
        "range must carry source.md + code.json: {range_text}"
    );

    git_in(&peer, ["pull", "-q", "--ff-only"]);
    for path in [
        "lib/chat/report.pdf",
        "lib/chat/report.mp3",
        "ref/chat/report.md",
    ] {
        assert!(peer.join(path).is_file(), "peer is missing {path}");
    }
    let peer_parent =
        fs::read_to_string(peer.join("sase.md")).expect("read peer parent");
    assert!(
        peer_parent.contains("[[ref/chat/report|Report]]"),
        "peer parent is missing the reading-task link: {peer_parent}"
    );
    let origin_head = git_in(&vault, ["rev-parse", "origin/master"]);
    assert_eq!(
        String::from_utf8_lossy(&origin_head.stdout).trim(),
        head,
        "origin HEAD must match the vault after the intake push"
    );
    let scan = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("--dry-run")
        .arg("--no-hooks")
        .env("BOB_DIR", &vault)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run follow-up dry scan");
    assert_success(&scan);
    let scan_report = stdout(&scan);
    assert!(
        !scan_report.contains("would create")
            && !scan_report.contains("note_action: create"),
        "follow-up scan must not plan a second note: {scan_report}"
    );
}

#[test]
fn create_queues_when_locks_or_pre_sync_prevent_inline_intake() {
    let temp = TempDir::new("bob-cli-create-ingest-queues");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let sync_lock = temp.path().join("bob_sync.lock");
    let held_sync = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&sync_lock)
        .expect("open shared lock");
    held_sync.lock_exclusive().expect("hold shared lock");
    let head_before =
        String::from_utf8(git_in(&vault, ["rev-parse", "HEAD"]).stdout)
            .expect("head is utf8");
    let sync_source = temp.path().join("sync-busy.md");
    write_file(&sync_source, "# Sync Busy\n");
    let sync_output =
        create_command(&temp, &vault, &sync_source, &fixture, &pandoc)
            .arg("-N")
            .arg("sync_busy")
            .output()
            .expect("run create with maintenance lock held");
    assert_success(&sync_output);
    assert!(
        stdout(&sync_output)
            .contains("queued: xlib/chat/sync_busy.pdf (lock busy:"),
        "{}",
        stdout(&sync_output)
    );
    assert!(vault.join("xlib/chat/sync_busy.pdf").is_file());
    let head_after =
        String::from_utf8(git_in(&vault, ["rev-parse", "HEAD"]).stdout)
            .expect("head is utf8");
    assert_eq!(
        head_after.trim(),
        head_before.trim(),
        "lock-busy queue must not commit"
    );
    drop(held_sync);

    let scan_lock_path = temp.path().join("state/bob-cli/ref/scan.lock");
    fs::create_dir_all(scan_lock_path.parent().expect("scan lock parent"))
        .expect("create scan lock directory");
    let held_scan = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&scan_lock_path)
        .expect("open scan lock");
    held_scan.lock_exclusive().expect("hold scan lock");
    let scan_source = temp.path().join("scan-busy.md");
    write_file(&scan_source, "# Scan Busy\n");
    let scan_output =
        create_command(&temp, &vault, &scan_source, &fixture, &pandoc)
            .arg("-N")
            .arg("scan_busy")
            .output()
            .expect("run create with scan lock held");
    assert_success(&scan_output);
    assert!(
        stdout(&scan_output)
            .contains("queued: xlib/chat/scan_busy.pdf (scan busy:"),
        "{}",
        stdout(&scan_output)
    );
    assert!(vault.join("xlib/chat/scan_busy.pdf").is_file());
    drop(held_scan);

    let remote_unreachable = temp.path().join("unreachable.git");
    git_in(
        &vault,
        ["remote", "set-url", "origin", path_str(&remote_unreachable)],
    );
    let offline_source = temp.path().join("offline.md");
    write_file(&offline_source, "# Offline\n");
    let offline_output =
        create_command(&temp, &vault, &offline_source, &fixture, &pandoc)
            .arg("-N")
            .arg("offline")
            .output()
            .expect("run create with unreachable origin");
    assert_success(&offline_output);
    assert!(
        stdout(&offline_output)
            .contains("queued: xlib/chat/offline.pdf (pre-sync failed:"),
        "{}",
        stdout(&offline_output)
    );
    assert!(vault.join("xlib/chat/offline.pdf").is_file());

    git_in(
        &vault,
        [
            "remote",
            "set-url",
            "origin",
            path_str(&temp.path().join("remote.git")),
        ],
    );
    let drain = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("--no-hooks")
        .env("BOB_DIR", &vault)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("drain queued PDF locally with scan");
    assert_success(&drain);
    assert!(vault.join("lib/chat/offline.pdf").is_file());
    assert!(vault.join("ref/chat/offline.md").is_file());
}

#[test]
fn create_keeps_intake_for_flag_config_env_and_dry_run() {
    let temp = TempDir::new("bob-cli-create-ingest-kept");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("kept.md");
    write_file(&source, "# Kept\n");
    let head_before =
        String::from_utf8(git_in(&vault, ["rev-parse", "HEAD"]).stdout)
            .expect("head is utf8");
    let head_before = head_before.trim();

    let keep = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-k")
        .output()
        .expect("run create with -k");
    assert_success(&keep);
    let legacy_output = stdout(&keep);
    assert!(
        legacy_output.contains("next: bob ref scan"),
        "{legacy_output}"
    );
    assert!(
        !legacy_output.contains("queued:")
            && !legacy_output.contains("ingested:")
    );
    assert!(vault.join("xlib/chat/kept.pdf").is_file());
    assert!(!vault.join("lib/chat/kept.pdf").exists());
    // Phase store: `-k` still writes the write-once source store (taking
    // the maintenance lock) but files nothing and commits nothing.
    assert!(
        legacy_output.contains("source_markdown: .bob/ref-sources/chat/kept--"),
        "{legacy_output}"
    );
    assert!(legacy_output.contains("code: "), "{legacy_output}");
    let _ = fs::remove_file(temp.path().join("bob_sync.lock"));
    // Clean the uncommitted store so later modes start clean.
    for entry in fs::read_dir(vault.join(".bob/ref-sources/chat"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry.file_name().to_string_lossy().starts_with("kept--") {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    fs::remove_file(vault.join("xlib/chat/kept.pdf"))
        .expect("remove kept PDF before next mode");

    let config_file = temp.path().join("config.yml");
    write_file(&config_file, "highlights:\n  ingest_on_create: false\n");
    let config_output =
        create_command(&temp, &vault, &source, &fixture, &pandoc)
            .env("BOB_CONFIG_FILE", &config_file)
            .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "")
            .output()
            .expect("run create with config switch disabled");
    assert_success(&config_output);
    assert_eq!(stdout(&config_output), legacy_output);
    assert!(vault.join("xlib/chat/kept.pdf").is_file());
    let _ = fs::remove_file(temp.path().join("bob_sync.lock"));
    for entry in fs::read_dir(vault.join(".bob/ref-sources/chat"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry.file_name().to_string_lossy().starts_with("kept--") {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    fs::remove_file(vault.join("xlib/chat/kept.pdf"))
        .expect("remove config-kept PDF before next mode");

    let env_output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "0")
        .output()
        .expect("run create with env switch disabled");
    assert_success(&env_output);
    assert_eq!(stdout(&env_output), legacy_output);
    assert!(vault.join("xlib/chat/kept.pdf").is_file());
    let _ = fs::remove_file(temp.path().join("bob_sync.lock"));
    for entry in fs::read_dir(vault.join(".bob/ref-sources/chat"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry.file_name().to_string_lossy().starts_with("kept--") {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    fs::remove_file(vault.join("xlib/chat/kept.pdf"))
        .expect("remove env-kept PDF before dry-run");

    let dry_source = temp.path().join("dry.md");
    write_file(&dry_source, "# Dry Run\n");
    let dry = create_command(&temp, &vault, &dry_source, &fixture, &pandoc)
        .arg("-d")
        .arg("-N")
        .arg("dry_run")
        .output()
        .expect("run create dry-run");
    assert_success(&dry);
    let dry_report = stdout(&dry);
    assert!(dry_report.contains("intake: would ingest into lib/chat/dry_run.pdf → ref/chat/dry_run.md"), "{dry_report}");
    assert!(dry_report.contains("writes: none"), "{dry_report}");
    assert!(!vault.join("xlib/chat/dry_run.pdf").exists());
    assert!(
        !temp.path().join("bob_sync.lock").exists(),
        "dry-run must not take the maintenance lock"
    );
    let head_after =
        String::from_utf8(git_in(&vault, ["rev-parse", "HEAD"]).stdout)
            .expect("head is utf8");
    assert_eq!(
        head_after.trim(),
        head_before,
        "keep modes and dry-run must not commit"
    );
}

#[test]
fn create_rejects_an_invalid_ingest_on_create_env_before_any_write() {
    let temp = TempDir::new("bob-cli-create-ingest-invalid-env");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("maybe.md");
    write_file(&source, "# Maybe\n");

    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "maybe")
        .output()
        .expect("run create with invalid ingest env");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let err = stderr(&output);
    assert!(
        err.contains("BOB_HIGHLIGHTS_INGEST_ON_CREATE")
            && err.contains("maybe")
            && err.contains("0, false, no, off, 1, true, yes, on"),
        "{err}"
    );
    assert!(!vault.join("xlib/chat/maybe.pdf").exists());
    assert!(!vault.join("lib/chat/maybe.pdf").exists());
    assert!(!vault.join("ref/chat/maybe.md").exists());
}

#[test]
fn create_queues_for_non_git_and_other_vaults_without_touching_bob_dir() {
    let temp = TempDir::new("bob-cli-create-ingest-vault-identity");
    let (fixture, pandoc) = create_fixture(&temp);
    let vault = temp.path().join("non-git");
    fs::create_dir_all(&vault).expect("create non-git vault");
    seed_parent(&vault);
    let source = temp.path().join("not-git.md");
    write_file(&source, "# Non Git\n");
    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .output()
        .expect("run create in non-git vault");
    assert_success(&output);
    assert!(
        stdout(&output).contains("(vault is not a Git worktree:"),
        "{}",
        stdout(&output)
    );
    assert!(vault.join("xlib/chat/not-git.pdf").is_file());

    let primary_temp = TempDir::new("bob-cli-create-ingest-primary");
    let (primary, _remote, _peer) = init_vault_sync_pair(&primary_temp);
    // Use a separate selected vault and leave BOB_DIR pointing at primary.
    let selected = temp.path().join("selected");
    fs::create_dir_all(&selected).expect("create selected vault");
    seed_parent(&selected);
    let other_source = temp.path().join("other-vault.md");
    write_file(&other_source, "# Other Vault\n");
    let other =
        create_command(&temp, &selected, &other_source, &fixture, &pandoc)
            .env("BOB_DIR", &primary)
            .output()
            .expect("run create with -b different from BOB_DIR");
    assert_success(&other);
    assert!(
        stdout(&other).contains("(different vault:"),
        "{}",
        stdout(&other)
    );
    assert!(selected.join("xlib/chat/other-vault.pdf").is_file());
    assert!(!primary.join("xlib/chat/other-vault.pdf").exists());
    assert!(!primary.join("lib/chat/other-vault.pdf").exists());
}

#[test]
fn create_rolls_back_pdf_when_birth_task_selection_refuses() {
    let temp = TempDir::new("bob-cli-create-ingest-rollback");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    write_file(
        &vault.join("sase.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n\n",
            "- [ ] #task #ref [[ref/chat/refused]] ^ref-one\n",
            "- [ ] #task #ref [[ref/chat/refused]] ^ref-two\n",
        ),
    );
    git_in(&vault, ["add", "sase.md"]);
    git_in(&vault, ["commit", "-q", "-m", "seed ambiguous ref tasks"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    let source = temp.path().join("refused.md");
    write_file(&source, "# Refused\n");
    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-N")
        .arg("refused")
        .output()
        .expect("run create with ambiguous birth tasks");
    assert_success(&output);
    assert!(
        stdout(&output)
            .contains("queued: xlib/chat/refused.pdf (ingest refused:"),
        "{}",
        stdout(&output)
    );
    assert!(
        vault.join("xlib/chat/refused.pdf").is_file(),
        "PDF must roll back to xlib"
    );
    assert!(!vault.join("lib/chat/refused.pdf").exists());
    let status = git_in(&vault, ["status", "--porcelain"]);
    assert!(
        status.stdout.is_empty(),
        "pre-sync should leave a clean tree after rollback"
    );
}

/// An inline-filing reread files the library PDF, a note with `earlier:`,
/// and a versioned reading task in one run, leaving the earlier bundle and
/// its task line byte-identical.
#[test]
fn create_reread_files_inline_with_lineage_and_versioned_task() {
    let temp = TempDir::new("bob-cli-create-reread-inline");
    let (vault, _peer) = seed_synced_vault(&temp);
    let (fixture, pandoc) = create_fixture(&temp);
    let source = temp.path().join("report.md");
    write_file(&source, "# Report\n\nCaptured report.\n");

    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .output()
        .expect("create v1 inline");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ingested: lib/chat/report.pdf → ref/chat/report.md"),
        "{report}"
    );
    assert!(report.contains("task → sase.md ^ref-report"), "{report}");

    let before_note =
        fs::read(vault.join("ref/chat/report.md")).expect("read v1 note");
    let before_pdf =
        fs::read(vault.join("lib/chat/report.pdf")).expect("read v1 PDF");
    let before_parent =
        fs::read_to_string(vault.join("sase.md")).expect("read parent");

    let output = create_command(&temp, &vault, &source, &fixture, &pandoc)
        .arg("-f")
        .output()
        .expect("reread inline");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("reread: report → report__v2 (v1"),
        "real run must report the reread:\n{report}"
    );
    assert!(
        report.contains(
            "ingested: lib/chat/report__v2.pdf → ref/chat/report__v2.md"
        ),
        "the reread must file inline:\n{report}"
    );
    assert!(
        vault.join("lib/chat/report__v2.pdf").is_file(),
        "the new reading files beside the earlier one"
    );
    assert!(
        !vault.join("xlib/chat/report__v2.pdf").exists(),
        "nothing waits in intake:\n{report}"
    );
    let note = fs::read_to_string(vault.join("ref/chat/report__v2.md"))
        .expect("read reread note");
    assert!(
        note.contains("earlier:") && note.contains("ref/chat/report"),
        "birth writes lineage for the inline reread:\n{note}"
    );
    assert!(
        note.contains("id: report__v2"),
        "the reread stamps id to its stem:\n{note}"
    );
    let parent =
        fs::read_to_string(vault.join("sase.md")).expect("reread parent");
    assert!(
        parent.contains("^ref-report-v2"),
        "the reread files a versioned reading task:\n{parent}"
    );
    assert_eq!(
        fs::read(vault.join("ref/chat/report.md")).expect("reread v1 note"),
        before_note,
        "the earlier note keeps its bytes"
    );
    assert_eq!(
        fs::read(vault.join("lib/chat/report.pdf")).expect("reread v1 PDF"),
        before_pdf,
        "the earlier PDF keeps its bytes"
    );
    let v1_line = before_parent
        .lines()
        .find(|line| {
            line.contains("^ref-report") && !line.contains("^ref-report-v")
        })
        .expect("v1 task line");
    assert!(
        parent.lines().any(|line| line == v1_line),
        "the earlier reading task line survives byte-identical:\n{parent}"
    );
}
