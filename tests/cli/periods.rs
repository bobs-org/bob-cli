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
            "-p, --problems",
            "-y, --year",
        ],
    );
    assert_stdout_has_no_ansi(&list_help);

    let migrate_help = bob_command()
        .arg("periods")
        .arg("migrate")
        .arg("--help")
        .output()
        .expect("run bob periods migrate --help");
    assert_success(&migrate_help);
    assert_text_order(
        &stdout(&migrate_help),
        &[
            "-a, --apply",
            "-b, --bob-dir",
            "-f, --format",
            "-h, --help",
            "-o, --offline",
            "-s, --since",
        ],
    );
    assert_stdout_has_no_ansi(&migrate_help);

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

const MIGRATE_NOW: &str = "2026-11-03 12:00:00";

fn zorg_year() -> &'static str {
    "---\nparent: [[journal]]\ntype: [[day]]\nzorg_id: 2026\n---\n\n# 2026\n\nIndex of 2026.\n"
}

fn write_dataview_tasks_settings(vault: &std::path::Path) {
    let settings =
        vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(settings, "{\"taskFormat\":\"dataview\"}\n").unwrap();
}

fn write_adoption_fixture(vault: &std::path::Path) {
    write_dataview_tasks_settings(vault);
    fs::create_dir_all(vault.join("2026")).unwrap();
    fs::create_dir_all(vault.join("2025")).unwrap();
    fs::write(vault.join("2026.md"), zorg_year()).unwrap();
    fs::write(
        vault.join("2024.md"),
        "---\nparent: [[journal]]\n---\n\n# 2024\n",
    )
    .unwrap();
    fs::write(
        vault.join("2025.md"),
        "---\nparent: [[journal]]\n---\n\n# 2025\n",
    )
    .unwrap();
    fs::write(
        vault.join("2026/202611.md"),
        "---\nparent: [[2026]]\n---\n\n# November\n",
    )
    .unwrap();
    fs::write(
        vault.join("2026/20261102.md"),
        "---\nparent: '[[2026/202611]]'\n---\n\n[[2026/20261101|prev]]  | [[2026/20261103|next]]\n\n## Pomodoros\n- [x] (**08:00 - 08:25** [t:: 25m]) — PLAN\n",
    )
    .unwrap();
    fs::write(
        vault.join("2026/20261103.md"),
        "---\nparent: '[[2026/202611]]'\n---\n\nsee you tomorrow\n",
    )
    .unwrap();
    fs::write(
        vault.join("2025/20251229.md"),
        "---\nparent: '[[2025/202512]]'\n---\n\n[[2025/20251228|prev]]  | [[2025/20251230|next]]\n",
    )
    .unwrap();
    fs::write(vault.join("2026/2026-W44.md"), "   \n").unwrap();
    fs::write(
        vault.join("2026/2026-W43.md"),
        "Bryan already wrote this week.\n",
    )
    .unwrap();
}

fn init_git_vault(vault: &std::path::Path) {
    git_in(vault, ["init", "-q"]);
    git_in(vault, ["config", "user.email", "t@t"]);
    git_in(vault, ["config", "user.name", "t"]);
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-qm", "init"]);
}

fn migrate(vault: &std::path::Path, extra: &[&str]) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("periods")
        .arg("migrate")
        .arg("-b")
        .arg(vault)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", MIGRATE_NOW)
        .env("NO_COLOR", "1");
    for arg in extra {
        command.arg(arg);
    }
    command.output().expect("run bob periods migrate")
}

#[test]
fn migrate_dry_run_is_a_pure_plan() {
    let temp = TempDir::new("bob-cli-periods-migrate-dry");
    let vault = temp.path().join("vault");
    write_adoption_fixture(&vault);
    init_git_vault(&vault);
    let before = fs::read_to_string(vault.join("2026.md")).unwrap();
    let monthly = fs::read_to_string(vault.join("2026/202611.md")).unwrap();

    let output = migrate(&vault, &["-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("migrate JSON");
    assert_eq!(body["ok"], true);
    assert_eq!(body["dry_run"], true);
    assert_eq!(
        body["commit_subject"],
        "periods: adopt the period-note tree for ISO 2026"
    );
    let created = body["created"].as_array().expect("created");
    assert!(created.iter().any(|p| p == "all_time.md"), "{created:?}");
    assert!(
        body["adopted"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "2026.md"),
        "{}",
        stdout(&output)
    );
    assert!(
        body["nav_skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "2026/20261103.md"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("2026.md")).unwrap(),
        before,
        "dry run must not write"
    );
    assert_eq!(
        fs::read_to_string(vault.join("2026/202611.md")).unwrap(),
        monthly
    );
    assert!(
        !vault.join("all_time.md").exists(),
        "dry run must not create files"
    );
}

#[test]
fn migrate_apply_commits_the_tree_and_reruns_cleanly() {
    let temp = TempDir::new("bob-cli-periods-migrate-apply");
    let vault = temp.path().join("vault");
    write_adoption_fixture(&vault);
    init_git_vault(&vault);
    let monthly = fs::read_to_string(vault.join("2026/202611.md")).unwrap();
    let week43 = fs::read_to_string(vault.join("2026/2026-W43.md")).unwrap();

    let output = migrate(&vault, &["-a", "-o", "-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("apply JSON");
    assert_eq!(body["ok"], true);
    assert_eq!(body["dry_run"], false);
    assert!(body["sha"].as_str().is_some_and(|sha| !sha.is_empty()));
    assert_eq!(
        body["commit_subject"],
        "periods: adopt the period-note tree for ISO 2026"
    );

    let adopted = fs::read_to_string(vault.join("2026.md")).unwrap();
    assert!(adopted.contains("parent: \"[[all_time]]\""), "{adopted}");
    assert!(adopted.contains("zorg_id: 2026"), "{adopted}");
    assert!(adopted.contains("## Archive"), "{adopted}");
    assert!(adopted.contains("# 2026\n\nIndex of 2026."), "{adopted}");
    assert!(
        adopted.contains("Agent instructions · yearly v1"),
        "{adopted}"
    );
    let archive_at = adopted.find("## Archive").expect("archive");
    let callout_at = adopted.find("> [!agent]").expect("callout");
    assert!(callout_at < archive_at, "Archive follows the callout");

    assert!(vault.join("all_time.md").is_file());
    assert!(vault.join("2026/2026-W45.md").is_file());
    assert!(vault.join("2026/2026-Q4.md").is_file());
    let filled = fs::read_to_string(vault.join("2026/2026-W44.md")).unwrap();
    assert!(filled.contains("# 2026-W44"), "{filled}");
    assert_eq!(
        fs::read_to_string(vault.join("2026/2026-W43.md")).unwrap(),
        week43,
        "notes with content stay untouched"
    );
    assert_eq!(
        fs::read_to_string(vault.join("2026/202611.md")).unwrap(),
        monthly,
        "monthly notes stay untouched"
    );

    let daily = fs::read_to_string(vault.join("2026/20261102.md")).unwrap();
    assert!(daily.contains("parent: '[[2026/2026-W45]]'"), "{daily}");
    assert!(
        daily.contains(
            "[[2026/20261101|prev]] · [[2026/2026-W45|W45]] · [[2026/20261103|next]]"
        ),
        "{daily}"
    );
    let skipped = fs::read_to_string(vault.join("2026/20261103.md")).unwrap();
    assert!(skipped.contains("parent: '[[2026/2026-W45]]'"), "{skipped}");
    assert!(skipped.contains("see you tomorrow"), "{skipped}");
    let dec29 = fs::read_to_string(vault.join("2025/20251229.md")).unwrap();
    assert!(dec29.contains("parent: '[[2026/2026-W01]]'"), "{dec29}");

    let year2024 = fs::read_to_string(vault.join("2024.md")).unwrap();
    assert!(year2024.contains("parent: \"[[all_time]]\""), "{year2024}");
    assert!(year2024.contains("# 2024"), "{year2024}");
    assert!(!year2024.contains("## Archive"), "{year2024}");

    let log = git_in(&vault, ["log", "--format=%s"]);
    let subjects = stdout(&log);
    let lines: Vec<&str> = subjects.lines().collect();
    assert_eq!(lines[0], "periods: adopt the period-note tree for ISO 2026");
    assert_eq!(lines.len(), 2, "exactly one adoption commit:\n{subjects}");

    let after = snapshot_tree(&vault);
    let rerun = migrate(&vault, &["-a", "-o", "-f", "json"]);
    assert_success(&rerun);
    let rerun_body: serde_json::Value =
        serde_json::from_str(&stdout(&rerun)).expect("rerun JSON");
    assert!(
        rerun_body["sha"].is_null(),
        "re-run must not commit:\n{}",
        stdout(&rerun)
    );
    assert_eq!(snapshot_tree(&vault), after, "re-run is a no-op");
    let log_after = stdout(&git_in(&vault, ["log", "--format=%s"]));
    assert_eq!(log_after, subjects);
}

fn snapshot_tree(vault: &std::path::Path) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        rows: &mut Vec<(String, String)>,
    ) {
        let mut entries: Vec<_> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, rows);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                rows.push((
                    relative,
                    fs::read_to_string(&path).unwrap_or_default(),
                ));
            }
        }
    }
    walk(vault, vault, &mut rows);
    rows
}

#[test]
fn list_problems_reports_each_kind() {
    let vault = vault_dir("problems");
    fs::create_dir_all(vault.path().join("2026")).unwrap();
    fs::write(
        vault.path().join("2026/2026-W45.md"),
        "---\nparent: \"[[journal]]\"\n---\n\n# 2026-W45\n\n- [ ] do it #task\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/20261103.md"),
        "---\nparent: '[[2026/202611]]'\n---\n\n# day\n",
    )
    .unwrap();
    let output = bob_command()
        .arg("periods")
        .arg("list")
        .arg("-b")
        .arg(vault.path())
        .arg("-p")
        .arg("-f")
        .arg("json")
        .env("BOB_NOW", MIGRATE_NOW)
        .output()
        .expect("list -p");
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("problems JSON");
    let kinds: Vec<&str> = body["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    for expected in [
        "dangling_daily_parent",
        "missing_period_note",
        "unscaffolded",
        "wrong_parent",
        "task_rows",
        "stale_instructions",
    ] {
        assert!(
            kinds.contains(&expected),
            "missing {expected} in {kinds:?}\n{}",
            stdout(&output)
        );
    }
}

#[test]
fn nightly_help_names_the_period_notes_step() {
    let output = bob_command()
        .arg("nightly")
        .arg("--help")
        .output()
        .expect("nightly help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("period notes"),
        "nightly help names the step:\n{help}"
    );
    assert!(
        help.contains("report period-note problems"),
        "nightly help says the step is report-only:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "2. run vault-sync against the vault",
            "3. run task archive against the vault",
            "4. report article notes that need formatting (write nothing)",
            "5. report period-note problems (write nothing)",
            "6. run vault-sync against the vault",
        ],
    );
}
