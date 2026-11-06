//! Merge-in (`<`) transitional execution refusal: dry-run and commit write
//! no files, including a later merge-in in a bulk draft.

use crate::support::*;
use std::path::Path;

const NOW: &str = "2026-11-05 09:30:00";
const TODAY: &str = "2026-11-05";

fn area_with_tasks(tasks: &str) -> String {
    format!("---\ntype: \"[[area]]\"\n---\n## Tasks\n{tasks}")
}

fn setup_merge_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("inbox.md"),
        &area_with_tasks("- [ ] #task Existing target ^t\n"),
    );
    write_file(
        &vault.join("a.md"),
        &area_with_tasks("- [ ] #task Fix the flaky login test ^f\n"),
    );
    write_file(
        &vault.join("b.md"),
        &area_with_tasks("- [ ] #task Login timeout too short ^b\n"),
    );
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    (temp, vault, day_file)
}

fn snapshot(vault: &Path) -> Vec<(String, String)> {
    let mut files: Vec<_> = std::fs::read_dir(vault)
        .expect("read vault")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .map(|entry| {
            let path = entry.path();
            let name = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .to_string();
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            (name, body)
        })
        .collect();
    files.sort();
    files
}

fn capture_json(
    vault: &Path,
    day_file: &Path,
    draft: &str,
    dry_run: bool,
) -> std::process::Output {
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
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture")
}

#[test]
fn merge_in_is_refused_in_dry_run_and_commit_without_writes() {
    for dry_run in [true, false] {
        let (_temp, vault, day_file) = setup_merge_vault("merge_in_refusal");
        let before = snapshot(&vault);
        let output =
            capture_json(&vault, &day_file, "New task < @a+f @b+b", dry_run);
        assert!(
            !output.status.success(),
            "merge-in should refuse: {}",
            format_output(&output)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stdout}\n{stderr}");
        assert!(
            combined.contains("merge_in_unavailable"),
            "expected merge_in_unavailable, got: {combined}"
        );
        assert_eq!(
            snapshot(&vault),
            before,
            "vault must be unchanged (dry_run={dry_run})"
        );
    }
}

#[test]
fn later_merge_in_bulk_item_leaves_vault_unchanged() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_bulk");
    let before = snapshot(&vault);
    let output = capture_json(
        &vault,
        &day_file,
        "First task @inbox\n\nSecond < @a+f",
        false,
    );
    assert!(
        !output.status.success(),
        "bulk merge-in should refuse: {}",
        format_output(&output)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("merge_in_unavailable"),
        "expected merge_in_unavailable, got: {combined}"
    );
    assert_eq!(snapshot(&vault), before, "bulk vault must be unchanged");
}

#[test]
fn incomplete_merge_in_is_refused_without_writes() {
    let (_temp, vault, day_file) = setup_merge_vault("merge_in_incomplete");
    let before = snapshot(&vault);
    let output = capture_json(&vault, &day_file, "New task < @a", false);
    assert!(
        !output.status.success(),
        "incomplete should refuse: {}",
        format_output(&output)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("incomplete_merge_in"),
        "expected incomplete_merge_in, got: {combined}"
    );
    assert_eq!(snapshot(&vault), before);
}
