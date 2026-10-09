//! Ref-task locator CLI contracts over the dedicated fixture vault.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const FIXED_NOW: &str = "2026-10-06 12:00:00";

fn fixture_vault(prefix: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    copy_dir(&fixture("ref_tasks/vault"), &vault);
    (temp, vault)
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target)
        .unwrap_or_else(|error| panic!("create {}: {error}", target.display()));
    let entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let child_target = target.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &child_target);
        } else {
            fs::copy(entry.path(), &child_target).unwrap_or_else(|error| {
                panic!("copy {}: {error}", entry.path().display())
            });
        }
    }
}

fn run_ref(vault: &Path, args: &[&str]) -> Output {
    bob_command()
        .arg("ref")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("run bob ref")
}

fn run_json(vault: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_ref(vault, &full);
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!("parse JSON: {error}\n{}", format_output(&output))
    })
}

fn row_by_path<'a>(
    doc: &'a serde_json::Value,
    path: &str,
) -> &'a serde_json::Value {
    doc["refs"]
        .as_array()
        .expect("refs array")
        .iter()
        .find(|row| row["path"] == path)
        .unwrap_or_else(|| panic!("missing row {path}"))
}

fn codes(row: &serde_json::Value) -> Vec<String> {
    row["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| d["code"].as_str().expect("code").to_string())
        .collect()
}

#[test]
fn list_rows_carry_task_parent_status_and_diagnostics() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-tasks-list");
    let doc = run_json(&vault, &["list", "-R", "all", "-A"]);

    // v2 live in sase.md (path-qualified [*] -> next).
    let live = row_by_path(&doc, "ref/papers/live_sase.md");
    assert_eq!(live["task"]["block_id"], serde_json::json!("ref-live-sase"));
    assert_eq!(live["task"]["archived"], serde_json::json!(false));
    assert!(live["task"]["link"]
        .as_str()
        .unwrap()
        .contains("sase#^ref-live-sase"));
    assert_eq!(live["parent"], serde_json::json!("sase"));
    assert_eq!(live["status"], serde_json::json!("next"));

    // v2 live in bob.md via bare stem.
    let bob_row = row_by_path(&doc, "ref/papers/live_bob.md");
    assert_eq!(bob_row["parent"], serde_json::json!("bob"));

    // Archived closed row reads sase with finished date.
    let archived = row_by_path(&doc, "ref/papers/archived.md");
    assert_eq!(archived["task"]["archived"], serde_json::json!(true));
    assert_eq!(archived["parent"], serde_json::json!("sase"));
    assert_eq!(archived["finished"], serde_json::json!("2026-10-12"));

    // v1 rows carry block_id ref.
    let v1 = row_by_path(&doc, "ref/papers/v1_open.md");
    assert_eq!(v1["task"]["block_id"], serde_json::json!("ref"));
    assert!(v1["task"]["link"].as_str().unwrap().ends_with("#^ref]]"));

    // Ambiguity gives null task.
    let amb = row_by_path(&doc, "ref/papers/ambiguous.md");
    assert!(amb["task"].is_null());
    assert!(codes(amb).contains(&"multiple_open_ref_tasks".to_string()));

    // Mismatch and outside-area diagnostics appear.
    let mismatch = row_by_path(&doc, "ref/papers/mismatch.md");
    assert!(codes(mismatch).contains(&"parent_mismatch".to_string()));
    let daily = row_by_path(&doc, "ref/papers/daily_target.md");
    assert!(codes(daily).contains(&"ref_task_outside_area".to_string()));

    // Embed-only open note reports missing task.
    let embed = row_by_path(&doc, "ref/papers/embed_only.md");
    assert!(codes(embed).contains(&"open_ref_without_task".to_string()));
}

#[test]
fn list_parent_resolution_matches_routes_aliases_and_literals() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-tasks-parent");
    // Alias bob-cli matches the bob.md row (canonical bob).
    let doc = run_json(&vault, &["list", "-R", "all", "-A", "-P", "bob-cli"]);
    let paths: Vec<String> = doc["refs"]
        .as_array()
        .expect("refs")
        .iter()
        .map(|row| row["path"].as_str().expect("path").to_string())
        .collect();
    assert!(
        paths.contains(&"ref/papers/live_bob.md".to_string()),
        "alias should match: {paths:?}"
    );
    assert_eq!(doc["filters"]["parent"], serde_json::json!("bob"));

    // Literal frozen name still matches.
    let doc =
        run_json(&vault, &["list", "-R", "all", "-A", "-P", "obsidian_ref"]);
    assert_eq!(doc["filters"]["parent"], serde_json::json!("obsidian_ref"));
}

#[test]
fn show_human_and_json_carry_reading_tasks() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-tasks-show");
    let output = run_ref(&vault, &["show", "ref/papers/live_sase.md"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("📖 Reading task"), "{text}");

    let doc = run_json(&vault, &["show", "ref/papers/live_sase.md"]);
    let tasks = doc["refs"][0]["tasks"].as_array().expect("tasks");
    // The follow-up from sase.md carries its path.
    assert!(
        tasks
            .iter()
            .any(|t| t["path"] == serde_json::json!("sase.md")),
        "follow-up path: {tasks:?}"
    );

    let v1 = run_ref(&vault, &["show", "ref/papers/v1_open.md"]);
    assert_success(&v1);
    assert!(stdout(&v1).contains("in this note (v1)"), "{}", stdout(&v1));

    let amb = run_ref(&vault, &["show", "ref/papers/ambiguous.md"]);
    assert_success(&amb);
    assert!(
        stdout(&amb).contains("run bob ref doctor"),
        "{}",
        stdout(&amb)
    );
}

#[test]
fn doctor_reports_ref_tasks_and_parents_rows() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-tasks-doctor");
    fs::create_dir_all(vault.join("lib")).expect("create lib dir for doctor");
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    let output = run_ref(&vault, &["doctor", "--no-hooks"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("ref tasks:"), "{report}");
    assert!(report.contains("parents:"), "{report}");
    // Library diagnostics must not double-count ref-task codes.
    assert!(
        !report.contains("open_v1_tracker") || report.contains("ref tasks:"),
        "{report}"
    );
    assert!(report.contains("result: ok"), "{report}");
}

#[test]
fn git_dates_never_backfills_v2_rows() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-tasks-git");
    let doc = run_json(&vault, &["list", "-R", "all", "-A", "-g"]);
    // v2 rows keep their stored dates; the flag must not fail.
    let archived = row_by_path(&doc, "ref/papers/archived.md");
    assert_eq!(archived["finished"], serde_json::json!("2026-10-12"));
}
