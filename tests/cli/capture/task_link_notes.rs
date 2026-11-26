//! Notes, drawers, and requirements on existing-task Pomodoro links.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-10-12 09:20:00";

fn write_link_fixture(
    vault: &std::path::Path,
) -> (std::path::PathBuf, String, String) {
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    let target_before =
        "# Cash\n## Tasks\n- [ ] #task Work ^work\n".to_string();
    let day_before =
        "## Pomodoros\n- [ ] () — CURRENT\n  - context\n".to_string();
    write_file(&target, &target_before);
    write_file(&day_file, &day_before);
    (day_file, target_before, day_before)
}

fn run_capture(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    draft: &str,
    flags: &[&str],
) -> std::process::Output {
    bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .args(flags)
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture")
}

fn parse_json(draft: &str) -> Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

#[test]
fn at_link_files_notes_and_dry_run_does_not_write() {
    let temp = TempDir::new("bob-cli-task-link-notes-at");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let (day_file, target_before, day_before) = write_link_fixture(&vault);
    let target = vault.join("cash.md");
    let draft = "@cash:work\n- foo\n  - detail";

    let dry =
        run_capture(&vault, &day_file, draft, &["--dry-run", "-f", "json"]);
    assert_success(&dry);
    let dry_json: Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry-run JSON");
    assert_eq!(dry_json["kind"], "pomodoro_link", "{dry_json}");
    assert_eq!(dry_json["notes"][0]["text"], "foo", "{dry_json}");
    assert_eq!(
        fs::read_to_string(&target).expect("target after dry-run"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("day after dry-run"),
        day_before
    );

    let human = run_capture(&vault, &day_file, draft, &["--dry-run"]);
    assert_success(&human);
    assert!(stdout(&human).contains("foo"), "{}", format_output(&human));

    let written = run_capture(&vault, &day_file, draft, &["-f", "json"]);
    assert_success(&written);
    let json: Value =
        serde_json::from_str(stdout(&written).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "pomodoro_link", "{json}");
    assert_eq!(json["notes"][0]["text"], "foo", "{json}");
    let target_after = fs::read_to_string(&target).expect("target after link");
    assert!(
        target_after.contains("- [*] #task Work ")
            && target_after.contains("^work"),
        "{target_after}"
    );
    assert!(
        target_after.contains("\t- foo\n\t\t- detail"),
        "{target_after}"
    );
    assert!(fs::read_to_string(&day_file)
        .expect("day after link")
        .contains("[[cash#^work]]"));
}

#[test]
fn caret_and_at_links_file_identical_note_blocks() {
    let mut snapshots = Vec::new();
    for (index, marker) in ["@cash:work", "^cash:work"].into_iter().enumerate()
    {
        let temp =
            TempDir::new(&format!("bob-cli-task-link-notes-spelling-{index}"));
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        let (day_file, _, _) = write_link_fixture(&vault);
        let draft = format!("{marker}\n- foo\n- **Future Work**\n  - detail");
        let output = run_capture(&vault, &day_file, &draft, &["-f", "json"]);
        assert_success(&output);
        let json: Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["kind"], "pomodoro_link", "{json}");
        assert_eq!(json["notes"][0]["text"], "foo", "{json}");
        assert_eq!(
            json["notes"][1]["drawer"]["title"], "Future Work",
            "{json}"
        );
        snapshots.push((
            fs::read_to_string(vault.join("cash.md")).expect("read task"),
            fs::read_to_string(day_file).expect("read day"),
        ));
    }
    assert_eq!(snapshots[0], snapshots[1]);
}

#[test]
fn link_close_forms_keep_refusing_child_notes_without_writes() {
    for (index, marker) in
        ["@cash:work=x", "^cash:work=x"].into_iter().enumerate()
    {
        let temp =
            TempDir::new(&format!("bob-cli-task-link-notes-close-{index}"));
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        let (day_file, target_before, day_before) = write_link_fixture(&vault);
        let draft = format!("{marker}\n- foo");
        let output = run_capture(&vault, &day_file, &draft, &["-f", "json"]);
        assert!(!output.status.success(), "{}", format_output(&output));
        assert!(
            stdout(&output).contains("Pomodoro link capture cannot be combined with authored child bullets"),
            "{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(vault.join("cash.md"))
                .expect("target after refusal"),
            target_before
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("day after refusal"),
            day_before
        );
    }
}

#[test]
fn capture_parse_and_dry_run_agree_for_notes_drawers_and_requirements() {
    for draft in [
        "@cash:work\n- foo",
        "^cash:work\n- foo",
        "@cash:work\n- **Future Work**\n  - detail",
        "@cash:work\n- [ ] first requirement",
    ] {
        let parse = parse_json(draft);
        assert_eq!(parse["mode"], "pomodoro_link", "{draft}: {parse}");
        assert_eq!(
            parse["diagnostics"],
            serde_json::json!([]),
            "{draft}: {parse}"
        );

        let temp = TempDir::new("bob-cli-task-link-notes-parity");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        let (day_file, _, _) = write_link_fixture(&vault);
        let dry =
            run_capture(&vault, &day_file, draft, &["--dry-run", "-f", "json"]);
        assert_success(&dry);
    }
}

#[test]
fn capture_complete_offers_existing_task_drawers_under_link_notes() {
    let temp = TempDir::new("bob-cli-task-link-notes-complete");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Work ^work\n  - **Future Work**\n",
    );
    let draft = "@cash:work\n- #";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("completion JSON");
    assert_eq!(json["context"], "drawer", "{json}");
    assert!(
        json["candidates"].as_array().is_some_and(|rows| {
            rows.iter().any(|row| row["title"] == "Future Work")
        }),
        "{json}"
    );
}
