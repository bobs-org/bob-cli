//! CLI fixtures and write-safety coverage for note-pull capture.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path, path::PathBuf, process::Output};

const NOW: &str = "2026-10-05 09:30:00";
const FIXTURES: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/note_pull");

fn fixture(case: &str) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let root = Path::new(FIXTURES).join(case);
    let temp = TempDir::new(&format!("bob-cli-note-pull-{case}"));
    let vault = temp.path().join("vault");
    copy_tree(&Path::new(FIXTURES).join("shared"), &vault);
    copy_tree(&root.join("vault"), &vault);
    let day = vault.join("20261005.md");
    (temp, vault, day, root)
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create fixture vault copy");
    for entry in fs::read_dir(source).expect("read fixture vault") {
        let entry = entry.expect("fixture entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().expect("fixture entry type").is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path)
                .expect("copy fixture input");
        }
    }
}

fn run_json(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
    dry_run: bool,
) -> Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .args(flags)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW);
    command.output().expect("run bob capture JSON")
}

fn run_human(
    vault: &Path,
    day: &Path,
    flags: &[&str],
    draft: &str,
    dry_run: bool,
) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .args(flags)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    command.output().expect("run bob capture human")
}

fn normalized_fixture_text(
    root: &Path,
    vault: &Path,
    day: &Path,
    name: &str,
) -> String {
    fs::read_to_string(root.join(name))
        .expect("read note-pull CLI expectation")
        .replace("<VAULT>", &vault.display().to_string())
        .replace("<DAY_FILE>", &day.display().to_string())
}

fn assert_json_fixture(
    root: &Path,
    vault: &Path,
    day: &Path,
    output: &Output,
    name: &str,
    exit_code: i32,
) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "unexpected exit status:\n{}",
        format_output(output)
    );
    let actual: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!("capture stdout is not JSON: {error}\n{}", stdout(output))
        });
    let expected: Value =
        serde_json::from_str(&normalized_fixture_text(root, vault, day, name))
            .expect("fixture expectation is JSON");
    assert_eq!(actual, expected, "CLI output differs from {name}");
    actual
}

fn assert_human_fixture(
    root: &Path,
    vault: &Path,
    day: &Path,
    output: &Output,
    name: &str,
) {
    assert_success(output);
    let actual = stdout(output);
    let expected = normalized_fixture_text(root, vault, day, name);
    assert_eq!(actual, expected, "human output differs from {name}");
}

fn snapshot(
    vault: &Path,
    day: &Path,
    notes: &[&str],
) -> Vec<(PathBuf, Vec<u8>)> {
    notes
        .iter()
        .map(|note| {
            let path = if *note == "<day>" {
                day.to_path_buf()
            } else {
                vault.join(note)
            };
            let bytes = fs::read(&path).unwrap_or_default();
            (path, bytes)
        })
        .collect()
}

fn assert_snapshot_unchanged(before: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in before {
        assert_eq!(
            fs::read(path).unwrap_or_default(),
            *bytes,
            "{} changed",
            path.display()
        );
    }
}

#[test]
fn np1_no_pick_matches_cli_json_and_human_fixtures() {
    let (_temp, vault, day, root) = fixture("np1");
    let before = snapshot(&vault, &day, &["sase.md", "<day>"]);
    let json = run_json(&vault, &day, &[], "@sase", true);
    let report =
        assert_json_fixture(&root, &vault, &day, &json, "stdout.json", 0);
    assert_eq!(report["note_pull"]["route"], "sase");
    assert_snapshot_unchanged(&before);

    let human = run_human(&vault, &day, &[], "@sase", true);
    assert_human_fixture(&root, &vault, &day, &human, "human.txt");
}

#[test]
fn np2_pick_all_matches_cli_json_and_human_fixtures() {
    let (_temp, vault, day, root) = fixture("np2");
    let json = run_json(&vault, &day, &["-p", "all"], "@sase", false);
    let report =
        assert_json_fixture(&root, &vault, &day, &json, "stdout.json", 0);
    assert_eq!(
        report["note_pull"]["successors"].as_array().unwrap().len(),
        3
    );

    let (_human_temp, human_vault, human_day, human_root) = fixture("np2");
    let human =
        run_human(&human_vault, &human_day, &["-p", "all"], "@sase", false);
    assert_human_fixture(
        &human_root,
        &human_vault,
        &human_day,
        &human,
        "human.txt",
    );
}

#[test]
fn np10_empty_and_folded_np12_missing_source_match_write_free_fixtures() {
    for (case, draft, notes) in [
        ("np10-empty", "@sase", vec!["sase.md", "<day>"]),
        ("np10-folded", "@sase", vec!["sase.md", "<day>"]),
        ("np12", "@nosuch", vec!["<day>"]),
    ] {
        let (_temp, vault, day, root) = fixture(case);
        let before = snapshot(&vault, &day, &notes);
        let output = run_json(&vault, &day, &[], draft, false);
        let report =
            assert_json_fixture(&root, &vault, &day, &output, "stdout.json", 2);
        assert!(report.get("code").is_some(), "{report}");
        assert_snapshot_unchanged(&before);
    }
}

#[test]
fn np13_required_choice_matches_refusal_fixture_without_writes() {
    let (_temp, vault, day, root) = fixture("np13");
    let before = snapshot(&vault, &day, &["sase.md", "<day>"]);
    let output =
        run_json(&vault, &day, &["--require-pull-in-choice"], "@sase", false);
    let report =
        assert_json_fixture(&root, &vault, &day, &output, "stdout.json", 1);
    assert_eq!(report["code"], "pull_in_choice_required");
    assert_snapshot_unchanged(&before);
}

#[test]
fn np16_inbox_choice_and_routed_pick_match_cli_fixtures() {
    let (_temp, vault, day, root) = fixture("np16");
    let choice_before =
        snapshot(&vault, &day, &["mac_inbox.md", "health.md", "<day>"]);
    let choice = run_json(
        &vault,
        &day,
        &["-p", "2", "--require-inbox-route-choice"],
        "@mac_inbox",
        false,
    );
    let report =
        assert_json_fixture(&root, &vault, &day, &choice, "choice.json", 1);
    assert_eq!(report["code"], "inbox_route_choice_required");
    assert_snapshot_unchanged(&choice_before);

    let routed = run_json(
        &vault,
        &day,
        &["-p", "2", "-i", "1=health"],
        "@mac_inbox",
        false,
    );
    let report =
        assert_json_fixture(&root, &vault, &day, &routed, "routed.json", 0);
    assert_eq!(report["inbox_route"]["routed_count"], 1);
}

#[test]
fn np28_dry_run_parity_and_refusal_paths_are_write_free() {
    let (_temp, vault, day, _) = fixture("np1");
    let before = snapshot(&vault, &day, &["sase.md", "<day>"]);

    let preview = run_json(&vault, &day, &[], "@sase", true);
    assert_success(&preview);
    assert_snapshot_unchanged(&before);

    let choice =
        run_json(&vault, &day, &["--require-pull-in-choice"], "@sase", false);
    assert_eq!(choice.status.code(), Some(1));
    assert_snapshot_unchanged(&before);

    let invalid = run_json(&vault, &day, &["-p", "9"], "@sase", false);
    assert_eq!(invalid.status.code(), Some(2));
    assert_snapshot_unchanged(&before);

    let failed_batch =
        run_json(&vault, &day, &["-p", "all"], "@sase\n\n@nosuch", false);
    assert_eq!(failed_batch.status.code(), Some(2));
    assert_snapshot_unchanged(&before);

    let (_parity_temp, parity_vault, parity_day, _) = fixture("np2");
    let real = capture_json_dry_run_matches_real_flags(
        &parity_vault,
        &parity_day,
        NOW,
        &["-p", "all"],
        &["@sase"],
    );
    assert_eq!(real["dry_run"], false);
}
