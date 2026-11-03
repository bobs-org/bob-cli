//! Core `bob periods ensure` / `list` contract.

use crate::support::*;
use std::fs;

fn vault_dir(tag: &str) -> TempDir {
    TempDir::new(&format!("bob-cli-periods-{tag}"))
}

fn ensure(
    vault: &std::path::Path,
    date: &str,
    extra: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("periods").arg("ensure").arg("-b").arg(vault);
    command.arg("-d").arg(date);
    for arg in extra {
        command.arg(arg);
    }
    command.output().expect("run bob periods ensure")
}

fn list(vault: &std::path::Path, extra: &[&str]) -> std::process::Output {
    let mut command = bob_command();
    command.arg("periods").arg("list").arg("-b").arg(vault);
    for arg in extra {
        command.arg(arg);
    }
    command.output().expect("run bob periods list")
}

#[test]
fn ensure_creates_the_look_ahead_set_and_reruns_cleanly() {
    let vault = vault_dir("ensure");
    // Mon 2026-12-28: the +7-day look-ahead reaches 2027-W01,
    // 2027-Q1, and 2027.
    let output = ensure(vault.path(), "2026-12-28", &[]);
    assert_success(&output);
    for expected in [
        "2026/2026-W53.md",
        "2026/2026-Q4.md",
        "2026.md",
        "2027/2027-W01.md",
        "2027/2027-Q1.md",
        "2027.md",
        "all_time.md",
    ] {
        assert!(
            vault.path().join(expected).is_file(),
            "missing {expected}:\n{}",
            format_output(&output)
        );
    }
    // A second run writes nothing and leaves bytes identical.
    let before: Vec<(String, String)> = [
        "2026/2026-W53.md",
        "2026/2026-Q4.md",
        "2026.md",
        "2027/2027-W01.md",
        "2027/2027-Q1.md",
        "2027.md",
        "all_time.md",
    ]
    .iter()
    .map(|relative| {
        (
            relative.to_string(),
            fs::read_to_string(vault.path().join(relative)).unwrap(),
        )
    })
    .collect();
    let rerun = ensure(vault.path(), "2026-12-28", &[]);
    assert_success(&rerun);
    assert!(
        stdout(&rerun).trim().is_empty(),
        "rerun should write nothing:\n{}",
        format_output(&rerun)
    );
    for (relative, contents) in before {
        assert_eq!(
            fs::read_to_string(vault.path().join(&relative)).unwrap(),
            contents,
            "rerun changed {relative}"
        );
    }
}

#[test]
fn ensure_never_clobbers_and_fills_empty_notes() {
    let vault = vault_dir("clobber");
    let target = vault.path().join("2026/2026-W45.md");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "Bryan's words\n").unwrap();
    let output = ensure(vault.path(), "2026-11-03", &[]);
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "Bryan's words\n",
        "ensure must never touch a note with content"
    );

    let empty = vault.path().join("2026/2026-W46.md");
    fs::create_dir_all(empty.parent().unwrap()).unwrap();
    fs::write(&empty, "   \n").unwrap();
    let output = ensure(vault.path(), "2026-11-03", &[]);
    assert_success(&output);
    let filled = fs::read_to_string(&empty).unwrap();
    assert!(
        filled.contains("# 2026-W46"),
        "empty notes are filled:\n{filled}"
    );
}

#[test]
fn ensure_dry_run_plans_without_writing() {
    let vault = vault_dir("dry-run");
    let output = ensure(vault.path(), "2026-11-03", &["-n"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("would create"),
        "dry run names its writes:\n{}",
        format_output(&output)
    );
    assert!(
        fs::read_dir(vault.path())
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true),
        "dry run must not write files"
    );
}

#[test]
fn list_shows_review_states_and_filters() {
    let vault = vault_dir("list");
    let output = ensure(vault.path(), "2026-11-03", &[]);
    assert_success(&output);

    let output = list(vault.path(), &["-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("list JSON parses");
    assert_eq!(body["ok"], true);
    assert_eq!(body["schema_version"], 1);
    let rows = body["rows"].as_array().expect("rows array");
    assert!(!rows.is_empty(), "list returns rows");
    for row in rows {
        assert!(row["id"].is_string());
        assert!(row["path"].is_string());
        assert!(row["review_state"].is_string());
    }

    let weeks = list(vault.path(), &["-k", "week", "-f", "json"]);
    assert_success(&weeks);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&weeks)).expect("weeks JSON parses");
    for row in body["rows"].as_array().expect("rows") {
        assert_eq!(row["kind"], "week");
    }

    let human = list(vault.path(), &[]);
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
}

#[test]
fn periods_help_lists_options_alphabetically() {
    let ensure_help = bob_command()
        .arg("periods")
        .arg("ensure")
        .arg("--help")
        .output()
        .expect("run bob periods ensure --help");
    assert_success(&ensure_help);
    assert_text_order(
        &stdout(&ensure_help),
        &[
            "-b, --bob-dir",
            "-d, --date",
            "-n, --dry-run",
            "-f, --format",
            "-h, --help",
            "-q, --quiet",
        ],
    );
    assert_stdout_has_no_ansi(&ensure_help);

    let list_help = bob_command()
        .arg("periods")
        .arg("list")
        .arg("--help")
        .output()
        .expect("run bob periods list --help");
    assert_success(&list_help);
    assert_text_order(
        &stdout(&list_help),
        &[
            "-b, --bob-dir",
            "-f, --format",
            "-h, --help",
            "-k, --kind",
            "-y, --year",
        ],
    );
    assert_stdout_has_no_ansi(&list_help);

    // Bare `bob periods` runs the read-only list.
    let bare = bob_command()
        .arg("periods")
        .arg("--help")
        .output()
        .expect("run bob periods --help");
    assert_success(&bare);
    let help = stdout(&bare);
    assert!(
        help.contains("Bare `bob periods` runs the read-only list."),
        "group help names its default:\n{help}"
    );
    assert!(
        help.contains("docs/periods.md"),
        "group help points at the contract:\n{help}"
    );
}
