//! Pending overwrite-intent recovery through `ref create` and `ref scan`.
//!
//! The intent markers are written by hand (the transaction phase owns the
//! writer); these tests prove the entry hooks recover, refuse dry runs,
//! and keep JSON output parseable.

use crate::support::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const STEM: &str = "half-written";
const SUBJECT: &str = "bob ref create: overwrite blogs/half-written";

fn fixture(prefix: &str) -> (TempDir, PathBuf, PathBuf) {
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

fn head(vault: &Path) -> String {
    let output = git_in(&vault, &["rev-parse", "HEAD"]);
    assert!(output.status.success(), "rev-parse HEAD failed");
    stdout(&output).trim().to_string()
}

fn intent_path(state: &Path, vault: &Path) -> PathBuf {
    let canonical = fs::canonicalize(vault)
        .expect("canonical vault")
        .to_string_lossy()
        .into_owned();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let name = format!("overwrite-{}.json", hex::encode(hasher.finalize()));
    state.join("bob-cli/ref/overwrite-intents").join(name)
}

fn write_intent(
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
    let committed = git_in(&vault, &["add", "-A"]);
    assert!(committed.status.success());
    let committed = git_in(&vault, &["commit", "-q", "-m", SUBJECT]);
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
