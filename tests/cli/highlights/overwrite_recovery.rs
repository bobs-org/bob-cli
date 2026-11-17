//! Pending overwrite-intent recovery through `ref create` and `ref scan`.
//!
//! The intent markers are written by hand (the transaction phase owns the
//! writer); these tests prove the entry hooks recover, refuse dry runs,
//! and keep JSON output parseable.

use crate::support::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const STEM: &str = "half-written";
pub(crate) const SUBJECT: &str = "bob ref create: overwrite blogs/half-written";

pub(crate) fn fixture(prefix: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    for directory in ["lib/blogs", "ref/blogs", "xlib"] {
        fs::create_dir_all(vault.join(directory)).expect("vault dirs");
    }
    write_file(
        &vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nbody\n",
    );
    let git = |args: &[&str]| {
        let output = git_in(&vault, args);
        assert!(
            output.status.success(),
            "git {args:?} failed:\n{}",
            format_output(&output)
        );
        output
    };
    git(&["init", "-q"]);
    configure_test_git_identity(&vault);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "fixture"]);
    let state = temp.path().join("state");
    (temp, vault, state)
}

pub(crate) fn head(vault: &Path) -> String {
    let output = git_in(vault, ["rev-parse", "HEAD"]);
    assert!(output.status.success(), "rev-parse HEAD failed");
    stdout(&output).trim().to_string()
}

pub(crate) fn intent_path(state: &Path, vault: &Path) -> PathBuf {
    let canonical = fs::canonicalize(vault)
        .expect("canonical vault")
        .to_string_lossy()
        .into_owned();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let name = format!("overwrite-{}.json", hex::encode(hasher.finalize()));
    state.join("bob-cli/ref/overwrite-intents").join(name)
}

pub(crate) fn write_intent_v2(
    state: &Path,
    vault: &Path,
    base: &str,
    expected_head: Option<&str>,
    touched: &[&str],
    created: &[&str],
    moves: serde_json::Value,
    writes: serde_json::Value,
) {
    let canonical = fs::canonicalize(vault)
        .expect("canonical vault")
        .to_string_lossy()
        .into_owned();
    let path = intent_path(state, vault);
    fs::create_dir_all(path.parent().expect("intent parent"))
        .expect("intent dirs");
    let document = serde_json::json!({
        "schema_version": 2,
        "vault": canonical,
        "stem": STEM,
        "ref_type": "blogs",
        "base_head": base,
        "expected_subject": SUBJECT,
        "expected_head": expected_head,
        "touched": touched,
        "created": created,
        "moves": moves,
        "writes": writes,
    });
    fs::write(
        &path,
        serde_json::to_string(&document).expect("intent json"),
    )
    .expect("write intent");
}

pub(crate) fn write_intent(
    state: &Path,
    vault: &Path,
    base: &str,
    expected_head: Option<&str>,
    touched: &[&str],
    created: &[&str],
) {
    let canonical = fs::canonicalize(vault)
        .expect("canonical vault")
        .to_string_lossy()
        .into_owned();
    let path = intent_path(state, vault);
    fs::create_dir_all(path.parent().expect("intent parent"))
        .expect("intent dirs");
    let document = serde_json::json!({
        "schema_version": 1,
        "vault": canonical,
        "stem": STEM,
        "ref_type": "blogs",
        "base_head": base,
        "expected_subject": SUBJECT,
        "expected_head": expected_head,
        "touched": touched,
        "created": created,
    });
    fs::write(
        &path,
        serde_json::to_string(&document).expect("intent json"),
    )
    .expect("write intent");
}

fn note_body(vault: &Path) -> String {
    fs::read_to_string(vault.join("ref/blogs/half-written.md"))
        .expect("read note")
}

#[test]
fn overwrite_recovery_blocks_dry_run_create() {
    let (_temp, vault, state) = fixture("bob-cli-overwrite-recovery-create");
    let base = head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );

    let output = bob_command()
        .arg("ref")
        .arg("create")
        .arg("-d")
        .arg("anything")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("dry-run create with pending intent");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a pending intent blocks dry-run create:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("recovery required"),
        "the refusal names recovery:\n{}",
        format_output(&output)
    );
    assert!(
        note_body(&vault).contains("CRASHED"),
        "dry-run create restores nothing"
    );
    assert!(
        intent_path(&state, &vault).exists(),
        "dry-run create clears nothing"
    );
}

#[test]
fn overwrite_recovery_rolls_back_on_writing_scan() {
    let (_temp, vault, state) = fixture("bob-cli-overwrite-recovery-scan");
    let base = head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    fs::create_dir_all(vault.join("ref_history/blogs")).expect("archive dir");
    fs::write(
        vault.join("ref_history/blogs/half-written__2026-10-26.md"),
        "frozen\n",
    )
    .expect("archive");
    write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &["ref_history/blogs/half-written__2026-10-26.md"],
    );

    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("writing scan with pending intent");
    assert_eq!(
        output.status.code(),
        Some(0),
        "rollback then a clean scan exits 0:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains(
            "recovered: rolled back an interrupted overwrite of half-written"
        ),
        "the rollback notice is exact:\n{}",
        format_output(&output)
    );
    assert!(
        note_body(&vault).contains("body"),
        "the note is restored to the base"
    );
    assert!(
        !vault
            .join("ref_history/blogs/half-written__2026-10-26.md")
            .exists(),
        "the recorded creation is removed"
    );
    assert!(
        !intent_path(&state, &vault).exists(),
        "the intent is cleared after a proved rollback"
    );
}

#[test]
fn overwrite_recovery_reports_pending_in_json_dry_run() {
    let (_temp, vault, state) = fixture("bob-cli-overwrite-recovery-json");
    let base = head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );

    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("json dry-run scan with pending intent");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a pending intent fails json dry-run scan:\n{}",
        format_output(&output)
    );
    let document: serde_json::Value = serde_json::from_str(&stdout(&output))
        .unwrap_or_else(|error| panic!("scan stdout stays parseable: {error}"));
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "recovery_required");
    assert!(
        note_body(&vault).contains("CRASHED"),
        "json dry-run restores nothing"
    );
    assert!(
        intent_path(&state, &vault).exists(),
        "json dry-run clears nothing"
    );
}

#[test]
fn overwrite_recovery_keeps_committed_overwrite_on_scan() {
    let (_temp, vault, state) = fixture("bob-cli-overwrite-recovery-kept");
    let base = head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nreplaced\n",
    )
    .expect("new body");
    let committed = git_in(&vault, ["add", "-A"]);
    assert!(committed.status.success());
    let committed = git_in(&vault, ["commit", "-q", "-m", SUBJECT]);
    assert!(committed.status.success());
    let committed_head = head(&vault);
    write_intent(
        &state,
        &vault,
        &base,
        Some(&committed_head),
        &["ref/blogs/half-written.md"],
        &[],
    );

    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("writing scan with committed intent");
    assert_eq!(
        output.status.code(),
        Some(0),
        "a committed overwrite keeps the scan green:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains(
            "recovered: kept the completed overwrite of half-written"
        ),
        "the retain notice is exact:\n{}",
        format_output(&output)
    );
    assert!(
        note_body(&vault).contains("replaced"),
        "the committed state is kept"
    );
    assert!(
        !intent_path(&state, &vault).exists(),
        "the intent is cleared once the commit is proved"
    );
}

#[test]
fn vault_sync_recovers_a_planted_overwrite_intent() {
    let temp = TempDir::new("bob-cli-overwrite-vault-sync-recover");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    fs::create_dir_all(vault.join("ref/blogs")).expect("ref dir");
    write_file(
        &vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nbody\n",
    );
    let committed = git_in(&vault, ["add", "-A"]);
    assert!(committed.status.success(), "{}", format_output(&committed));
    let committed = git_in(&vault, ["commit", "-q", "-m", "file reference"]);
    assert!(committed.status.success(), "{}", format_output(&committed));
    let pushed = git_in(&vault, ["push", "-q", "origin", "master"]);
    assert!(pushed.status.success(), "{}", format_output(&pushed));
    let base = head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    let state = temp.path().join("state");
    write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );

    let output = vault_sync_command(&vault, &temp)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("standalone vault-sync with pending overwrite intent");
    assert_eq!(
        output.status.code(),
        Some(0),
        "standalone vault-sync recovers then finishes:\n{}",
        format_output(&output)
    );
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains(
            "recovered: rolled back an interrupted overwrite of half-written"
        ),
        "vault-sync must print the rollback notice:\n{}",
        format_output(&output)
    );
    assert!(
        note_body(&vault).contains("body"),
        "vault-sync restores the note to the base"
    );
    assert!(
        !intent_path(&state, &vault).exists(),
        "vault-sync clears the intent after a proved rollback"
    );
}

#[test]
fn task_move_refuses_while_overwrite_intent_is_pending() {
    let (temp, vault, state) = fixture("bob-cli-overwrite-task-move-refuse");
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move this task [id:: source__move-me] ^move-me\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    let committed = git_in(&vault, ["add", "-A"]);
    assert!(committed.status.success(), "{}", format_output(&committed));
    let committed = git_in(&vault, ["commit", "-q", "-m", "movable task"]);
    assert!(committed.status.success(), "{}", format_output(&committed));
    let base = head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );

    let output = bob_command()
        .args(["task", "move", "source", "dest", "-b", "move-me"])
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("HOME", temp.path().join("home"))
        .env("XDG_STATE_HOME", &state)
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .output()
        .expect("task move with pending overwrite intent");
    assert_ne!(
        output.status.code(),
        Some(0),
        "task move must refuse while an overwrite intent is pending:\n{}",
        format_output(&output)
    );
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("interrupted overwrite of half-written")
            && combined.contains("bob ref scan"),
        "the refusal names the stem and tells the operator to scan:\n{}",
        format_output(&output)
    );
    assert!(
        note_body(&vault).contains("CRASHED"),
        "task move must not recover or commit the crashed overwrite"
    );
    assert!(
        intent_path(&state, &vault).exists(),
        "task move must leave the pending intent in place"
    );
    assert!(
        fs::read_to_string(vault.join("source.md"))
            .expect("source")
            .contains("^move-me"),
        "the move must not apply"
    );
}

#[test]
fn overwrite_recovery_reverses_annotated_pdf_move_on_scan() {
    let (temp, vault, state) = fixture("bob-cli-overwrite-recovery-annotated");
    const FROZEN: &[u8] = b"frozen\n";
    const LIVE_PDF: &str = "lib/blogs/half-written.pdf";
    const ARCHIVE_PDF: &str =
        "ref_history/blogs/half-written__2026-10-26__annotated.pdf";
    const ARCHIVE_NOTE: &str = "ref_history/blogs/half-written__2026-10-26.md";
    // The managed region lets the post-recovery scan parse the note.
    write_file(
        &vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nbody\n\n<!-- highlights:begin -->\n\n<!-- highlights:end -->\n",
    );
    // Valid PDFs so the post-recovery scan can parse the restored file.
    let marker = "- status: ready\n- parent: sase\n- title: Half\n- id: half-written\n- captured: 2026-01-01\n";
    let new_marker = marker.replace("title: Half", "title: Half v2");
    write_highlights_pdf(&vault.join(LIVE_PDF), marker);
    let old_pdf = fs::read(vault.join(LIVE_PDF)).expect("old pdf bytes");
    write_highlights_pdf(&temp.path().join("new.pdf"), &new_marker);
    let new_pdf = fs::read(temp.path().join("new.pdf")).expect("new pdf bytes");
    let committed = git_in(&vault, ["add", "-A"]);
    assert!(committed.status.success(), "{}", format_output(&committed));
    let committed = git_in(&vault, ["commit", "-q", "-m", "file reference"]);
    assert!(committed.status.success(), "{}", format_output(&committed));
    let base = head(&vault);

    let mut hasher = Sha256::new();
    hasher.update(&old_pdf);
    let old_hash = hex::encode(hasher.finalize_reset());
    hasher.update(&new_pdf);
    let new_hash = hex::encode(hasher.finalize_reset());
    hasher.update(FROZEN);
    let frozen_hash = hex::encode(hasher.finalize());
    write_intent_v2(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md", LIVE_PDF],
        &[ARCHIVE_NOTE],
        serde_json::json!([{
            "source": LIVE_PDF,
            "dest": ARCHIVE_PDF,
            "source_sha256": old_hash,
            "replacement_sha256": new_hash,
        }]),
        serde_json::json!([{
            "path": ARCHIVE_NOTE,
            "sha256": frozen_hash,
        }]),
    );

    // Crash after the rename and the replacement publication: the archive
    // holds the only annotated copy and the live path holds new bytes.
    fs::create_dir_all(vault.join("ref_history/blogs")).expect("archive dir");
    fs::rename(vault.join(LIVE_PDF), vault.join(ARCHIVE_PDF))
        .expect("move pdf");
    fs::write(vault.join(LIVE_PDF), &new_pdf).expect("replacement");
    fs::write(vault.join(ARCHIVE_NOTE), FROZEN).expect("archive note");
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");

    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("writing scan with annotated intent");
    assert_eq!(
        output.status.code(),
        Some(0),
        "a proved annotated rollback keeps the scan green:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains(
            "recovered: rolled back an interrupted overwrite of half-written"
        ),
        "the rollback notice is exact:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read(vault.join(LIVE_PDF)).expect("live pdf"),
        old_pdf,
        "the only annotated copy returns to the live path"
    );
    assert!(
        !vault.join(ARCHIVE_PDF).exists(),
        "the proven rename reverses instead of copying"
    );
    assert!(
        !vault.join(ARCHIVE_NOTE).exists(),
        "the transaction-owned archive note is removed"
    );
    assert!(
        !intent_path(&state, &vault).exists(),
        "the intent is cleared after a proved rollback"
    );
}

/// Recovery that mutates settles `last-scan.json` while both writer locks
/// are held. The follow-on empty-library scan is lock-free no-work and
/// must leave that receipt untouched.
#[test]
fn overwrite_recovery_receipt_survives_lock_free_no_work_scan() {
    let (temp, vault, state) = fixture("bob-cli-overwrite-recovery-receipt");
    let base = head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    let lock_path = temp.path().join("bob_sync.lock");

    let recovered = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .output()
        .expect("json scan with pending overwrite intent");
    assert_eq!(
        recovered.status.code(),
        Some(0),
        "rollback then a lock-free empty-lib scan exits 0:\n{}",
        format_output(&recovered)
    );
    assert!(
        stderr(&recovered).contains(
            "recovered: rolled back an interrupted overwrite of half-written"
        ),
        "the rollback notice is exact:\n{}",
        format_output(&recovered)
    );
    assert!(
        note_body(&vault).contains("body"),
        "the note is restored to the base"
    );
    assert!(
        !intent_path(&state, &vault).exists(),
        "the intent is cleared after a proved rollback"
    );
    let receipt = scan_receipt_path(&state);
    let receipt_after_recovery =
        fs::read(&receipt).expect("recovery writes a serialized receipt");

    let _scan_lock = hold_scan_writer_lock(&state);
    let _maint_lock = hold_vault_maintenance_lock(&lock_path);
    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", &state)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .env("BOB_REF_SCAN_LOCK_WAIT_SECONDS", "0")
        .output()
        .expect("no-work json scan after recovery");
    assert_eq!(
        output.status.code(),
        Some(0),
        "post-recovery no-work must finish without either lock:\n{}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        !err.contains("waiting for another bob ref scan to finish")
            && !err.contains("waiting for another vault maintenance run"),
        "no-work scans must not wait on either lock:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read(&receipt).expect("receipt remains"),
        receipt_after_recovery,
        "a lock-free no-work scan must leave the recovery receipt untouched"
    );
}

fn scan_receipt_path(state_dir: &Path) -> PathBuf {
    state_dir.join("bob-cli/ref/last-scan.json")
}

fn hold_scan_writer_lock(state_dir: &Path) -> std::fs::File {
    let lock_dir = state_dir.join("bob-cli/ref");
    fs::create_dir_all(&lock_dir).expect("create scan lock dir");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_dir.join("scan.lock"))
        .expect("open scan lock");
    fs2::FileExt::try_lock_exclusive(&file).expect("take scan lock");
    file
}

fn hold_vault_maintenance_lock(path: &Path) -> std::fs::File {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create vault lock dir");
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .expect("open vault maintenance lock");
    fs2::FileExt::try_lock_exclusive(&file)
        .expect("take vault maintenance lock");
    file
}
