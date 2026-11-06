//! Merge-in (`<`) transitional execution refusal: dry-run and commit write
//! no files, including a later merge-in in a bulk draft.
//!
//! Feedback coverage: dry-run `merge_in_pending` context and the stdin
//! `shell_ate_merge_in_arrow` guard. Complete `<` execution still refuses
//! with `merge_in_unavailable` until the execute phase lands.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::process::Stdio;

const NOW: &str = "2026-11-05 09:30:00";

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

fn pending_json(
    vault: &Path,
    day_file: &Path,
    draft: &str,
) -> serde_json::Value {
    let output = capture_json(vault, day_file, draft, true);
    assert!(
        !output.status.success(),
        "incomplete merge-in should refuse: {}",
        format_output(&output)
    );
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
            .expect("dry-run JSON");
    assert_eq!(json["code"], "incomplete_merge_in", "{json}");
    json
}

#[test]
fn merge_in_pending_reports_single_complete_source_with_mergeability() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-single");
    let before = snapshot(&vault);
    let json = pending_json(&vault, &day_file, "New task < @a+f @b");
    let pending = &json["merge_in_pending"];
    assert_eq!(pending["item"], 1, "{json}");
    assert_eq!(pending["need"], "merge_in_source", "{json}");
    let sources = pending["sources"].as_array().expect("sources array");
    assert_eq!(sources.len(), 1, "{json}");
    let first = &sources[0];
    assert_eq!(first["marker"], "@a+f", "{json}");
    assert_eq!(first["block_id"], "f", "{json}");
    assert_eq!(first["path"], "a.md", "{json}");
    assert_eq!(first["title"], "Fix the flaky login test", "{json}");
    assert_eq!(first["status_name"], "Ready", "{json}");
    assert_eq!(first["mergeable"], true, "{json}");
    assert_eq!(snapshot(&vault), before, "dry run writes nothing");
}

#[test]
fn merge_in_pending_preserves_typed_order_across_complete_sources() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-order");
    let json = pending_json(&vault, &day_file, "New task < @b+b @a+f @c");
    let sources = json["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(sources.len(), 2, "{json}");
    assert_eq!(sources[0]["marker"], "@b+b", "{json}");
    assert_eq!(sources[1]["marker"], "@a+f", "{json}");
    assert_eq!(sources[0]["path"], "b.md", "{json}");
    assert_eq!(sources[1]["path"], "a.md", "{json}");
}

#[test]
fn merge_in_pending_reports_source_level_errors() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-errors");
    let json = pending_json(&vault, &day_file, "New task < @a+typo @b");
    let sources = json["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(sources.len(), 1, "{json}");
    assert_eq!(sources[0]["marker"], "@a+typo", "{json}");
    assert!(
        sources[0]["error"]["code"].is_string(),
        "missing task reports code: {json}"
    );
    assert!(
        sources[0]["error"]["message"].is_string(),
        "missing task reports message: {json}"
    );

    let missing = pending_json(&vault, &day_file, "New task < @missing+f @b");
    let missing_sources = missing["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(missing_sources.len(), 1, "{missing}");
    assert!(missing_sources[0]["error"]["code"].is_string(), "{missing}");
}

#[test]
fn merge_in_pending_reports_closed_source_as_unmergeable() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-closed");
    write_file(
        &vault.join("a.md"),
        &area_with_tasks("- [x] #task Fix the flaky login test ^f\n"),
    );
    let json = pending_json(&vault, &day_file, "New task < @a+f @b");
    let sources = json["merge_in_pending"]["sources"]
        .as_array()
        .expect("sources array")
        .clone();
    assert_eq!(sources.len(), 1, "{json}");
    assert_eq!(sources[0]["mergeable"], false, "{json}");
    assert_eq!(
        sources[0]["mergeable_code"], "merge_source_closed",
        "{json}"
    );
}

#[test]
fn merge_in_pending_finds_the_failing_bulk_item() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-bulk");
    let json = pending_json(
        &vault,
        &day_file,
        "First task @inbox\n\nSecond < @a+f @b",
    );
    assert_eq!(json["merge_in_pending"]["item"], 2, "{json}");
    assert_eq!(
        json["merge_in_pending"]["sources"][0]["marker"], "@a+f",
        "{json}"
    );
}

#[test]
fn merge_in_pending_finds_failing_list_bullet() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-list");
    let json = pending_json(&vault, &day_file, "@inbox\n- Second < @a+f @b");
    assert_eq!(json["merge_in_pending"]["item"], 1, "{json}");
    assert_eq!(
        json["merge_in_pending"]["sources"][0]["marker"], "@a+f",
        "{json}"
    );
}

#[test]
fn merge_in_pending_is_dry_run_json_only() {
    let (_temp, vault, day_file) = setup_merge_vault("merge-in-pending-commit");
    let real = capture_json(&vault, &day_file, "New task < @a+f @b", false);
    assert!(!real.status.success(), "{}", format_output(&real));
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&real.stdout).trim())
            .expect("commit JSON");
    assert_eq!(json["code"], "incomplete_merge_in", "{json}");
    assert!(
        json.get("merge_in_pending").is_none(),
        "commit failures carry no pending context: {json}"
    );

    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("New task < @a+f @b")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("human incomplete");
    assert!(!human.status.success(), "{}", format_output(&human));
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(stderr.contains("pick tasks to merge in"), "{stderr}");
    assert!(!stderr.contains("merge_in_pending"), "{stderr}");
}

#[test]
fn shell_ate_merge_in_arrow_refuses_argv_with_at_stdin_redirect() {
    let temp = TempDir::new("bob-cli-merge-in-shell-guard");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    let before = fs::read_to_string(&day_file).expect("read day");
    let at_file = temp.path().join("@a+x");
    fs::write(&at_file, "stale stdin").expect("write @ stdin");
    let stdin = fs::File::open(&at_file).expect("open @ stdin");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("New task")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .stdin(Stdio::from(stdin))
        .output()
        .expect("run capture with @ stdin");
    assert!(
        !output.status.success(),
        "guard must refuse: {}",
        format_output(&output)
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("your shell ate"), "{err}");
    assert!(err.contains("quote the draft"), "{err}");
    assert!(err.contains("New task < @a+x"), "{err}");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day after"),
        before,
        "vault unchanged when the shell ate `<`"
    );

    // JSON failures carry the machine code.
    let json_stdin = fs::File::open(&at_file).expect("reopen @ stdin");
    let json_out = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("New task")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .stdin(Stdio::from(json_stdin))
        .output()
        .expect("json guard");
    assert!(!json_out.status.success(), "{}", format_output(&json_out));
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&json_out.stdout).trim())
            .expect("guard JSON");
    assert_eq!(json["code"], "shell_ate_merge_in_arrow", "{json}");
}

#[test]
fn shell_ate_merge_in_allows_plain_files_pipes_and_stdin_capture() {
    let temp = TempDir::new("bob-cli-merge-in-shell-guard-ok");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");

    // A plain (non-`@`) regular stdin file with argv does not trigger.
    let plain = temp.path().join("plain.md");
    fs::write(&plain, "stale stdin").expect("write plain stdin");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("jot")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .stdin(Stdio::from(fs::File::open(&plain).expect("open plain")))
        .output()
        .expect("run capture with plain stdin");
    assert_success(&output);

    // Piped stdin (not a regular file) with argv does not trigger.
    let piped = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("pipe ok")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture with piped stdin");
    assert_success(&piped);

    // Normal stdin capture without argv reads the draft from stdin: an
    // `@`-named stdin file is the draft source, not a shell-eaten `<`.
    let draft_file = temp.path().join("@draft");
    fs::write(&draft_file, "stdin task").expect("write draft stdin");
    let from_stdin = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .stdin(Stdio::from(
            fs::File::open(&draft_file).expect("open draft"),
        ))
        .output()
        .expect("run stdin capture");
    assert_success(&from_stdin);
}
