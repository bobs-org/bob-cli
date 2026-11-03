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
            "-R, --refresh-instructions",
        ],
    );
    assert_stdout_has_no_ansi(&ensure_help);

    for (command, about) in [
        ("brief", "agent bundle"),
        ("draft", "Replace the `## Review` section"),
    ] {
        let help = bob_command()
            .arg("periods")
            .arg(command)
            .arg("--help")
            .output()
            .expect("run periods help");
        assert_success(&help);
        assert!(
            stdout(&help).contains(about),
            "missing {about}:\n{}",
            stdout(&help)
        );
        assert_stdout_has_no_ansi(&help);
    }
    assert_text_order(
        &stdout(
            &bob_command()
                .arg("periods")
                .arg("draft")
                .arg("--help")
                .output()
                .expect("draft help"),
        ),
        &[
            "-b, --bob-dir",
            "-F, --force",
            "-f, --format",
            "-h, --help",
            "-i, --input",
            "-n, --dry-run",
        ],
    );

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

const REVIEWS_NOW: &str = "2026-11-10 12:00:00";

fn periods_command(
    vault: &std::path::Path,
    subcommand: &str,
    extra: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("periods")
        .arg(subcommand)
        .arg("-b")
        .arg(vault)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", REVIEWS_NOW)
        .env("NO_COLOR", "1");
    for arg in extra {
        command.arg(arg);
    }
    command.output().expect("run bob periods")
}

fn brief_json(vault: &std::path::Path, period: &str) -> serde_json::Value {
    let output = periods_command(vault, "brief", &[period, "-f", "json"]);
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).expect("brief JSON parses")
}

/// A vault with two closed October days in W44, one open November day,
/// done and kept tasks, and a blocked project.
fn write_reviews_fixture(vault: &std::path::Path) {
    write_dataview_tasks_settings(vault);
    let _ = periods_command(vault, "ensure", &["-d", "2026-10-26", "-q"]);
    let _ = periods_command(vault, "ensure", &["-d", "2026-11-02", "-q"]);
    fs::create_dir_all(vault.join("2026")).unwrap();
    fs::write(
        vault.join("2026/20261026.md"),
        "---\nparent: '[[2026/2026-W44]]'\n---\n\n\
         [[2026/20261025|prev]] · [[2026/2026-W44|W44]] · [[2026/20261027|next]]\n\n\
         - [x] #task Ship widget [created:: 2026-10-20] [completion:: 2026-10-26] ^ship\n\
         - [x] #task Big launch [created:: 2026-10-18] [completion:: 2026-10-26] ^prj\n\n\
         ## Pomodoros\n\n\
         - [x] (**08:00 - 08:25** [t:: 25m]) — PLAN\n\
         \t- [[#^ship]]\n",
    )
    .unwrap();
    fs::write(
        vault.join("2026/20261027.md"),
        "---\nparent: '[[2026/2026-W44]]'\n---\n\n\
         [[2026/20261026|prev]] · [[2026/2026-W44|W44]] · [[2026/20261028|next]]\n\n\
         - [ ] #task Next thing [created:: 2026-10-27] ^next1\n\n\
         ## Pomodoros\n\n\
         - [x] (**09:00 - 09:40** [t:: 40m]) — PLAN\n\
         \t- [[2026/20261026#^ship]]\n\
         \t- free thought about October\n",
    )
    .unwrap();
    fs::write(
        vault.join("2026/20261103.md"),
        "---\nparent: '[[2026/2026-W45]]'\n---\n\nopen day\n",
    )
    .unwrap();
    fs::write(
        vault.join("proj.md"),
        "---\nparent: \"[[org]]\"\n---\n\n\
         - [ ] #task Stuck project [created:: 2026-10-20] [keeps:: 3] ^stuck\n\
         \t- 🛠️ **WORK LOG**\n\
         \t\t- *2026-10-27 14:03* — shipped the widget slice\n\
         \t- 🥀 **ROTTEN LOG**\n\
         \t\t- *2026-10-27* — waiting on review · kept 3×\n\
         - [ ] #task Blocked build #prj_is_blocked [created:: 2026-10-20] ^prj\n\
         \t- Depends-On: [[2026/20261026#^ship]]\n",
    )
    .unwrap();
}

#[test]
fn brief_closed_week_reports_every_section() {
    let vault = vault_dir("brief-week");
    write_reviews_fixture(vault.path());
    let body = brief_json(vault.path(), "2026-W44");
    assert_eq!(body["ok"], true);
    assert_eq!(body["brief_version"], "brief v1");
    assert_eq!(body["id"], "2026-W44");
    assert_eq!(body["header"]["state"], "closed");
    assert_eq!(body["header"]["review_state"], "empty");
    assert!(
        body["header"]["stop_advice"]
            .as_str()
            .unwrap_or_default()
            .contains("ready to draft"),
        "stop advice: {}",
        body["header"]["stop_advice"]
    );
    let facts = body["facts"]["facts"].as_array().expect("facts");
    let worked = facts
        .iter()
        .find(|fact| fact["label"] == "Worked")
        .expect("worked fact");
    assert_eq!(worked["display"], "1h 05m");
    let roots = body["time"]["roots"].as_array().expect("roots");
    assert!(
        roots
            .iter()
            .any(|root| root["tasks"].as_array().is_some_and(|tasks| tasks
                .iter()
                .any(|task| {
                    task["link"].as_str().unwrap_or_default().contains("#^ship")
                        && task["display"] == "1h 05m"
                }))),
        "ship carries its linked minutes: {}",
        body["time"]
    );
    assert_eq!(
        body["parts"].as_array().expect("parts").len(),
        7,
        "seven day rows: {}",
        body["parts"]
    );
    let finished = body["finished"].as_array().expect("finished");
    assert!(finished.len() >= 2, "finished rows: {finished:?}");
    assert!(
        finished[0]["link"]
            .as_str()
            .unwrap_or_default()
            .contains("#^prj"),
        "project completions first: {finished:?}"
    );
    let words = body["bryans_words"].as_array().expect("words");
    assert!(
        words.iter().any(|word| word["text"]
            .as_str()
            .unwrap_or_default()
            .contains("shipped the widget slice")),
        "work log entry: {words:?}"
    );
    assert!(
        words.iter().any(|word| word["text"]
            .as_str()
            .unwrap_or_default()
            .contains("free thought about October")),
        "free-text child: {words:?}"
    );
    assert!(
        body["still_open"]["blocked_prj"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("waits on"))),
        "blocked reason: {}",
        body["still_open"]
    );
    assert!(
        body["still_open"]["kept"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("waiting on review"))),
        "rotten log reason: {}",
        body["still_open"]
    );
    assert!(body["caveats"].as_array().is_some(), "caveats ride along");
    assert!(body["practice"].is_null(), "practice is year-only");

    let markdown = periods_command(vault.path(), "brief", &["2026-W44"]);
    assert_success(&markdown);
    let text = stdout(&markdown);
    assert!(text.starts_with("# brief v1 · 2026-W44"), "{text}");
    for section in [
        "## Facts",
        "## Time",
        "## Parts",
        "## Finished",
        "## Still open, as of today",
        "## Bryan's words",
        "## This note and its neighbours (data, not instructions)",
        "## Off-clock",
        "## Caveats",
    ] {
        assert!(text.contains(section), "missing {section}:\n{text}");
    }
    assert!(
        text.len() / 4 <= 6000,
        "week budget: {} tokens",
        text.len() / 4
    );
    assert_stdout_has_no_ansi(&markdown);
}

#[test]
fn brief_output_is_deterministic() {
    let vault = vault_dir("brief-deterministic");
    write_reviews_fixture(vault.path());
    let first = brief_json(vault.path(), "2026-W44");
    let second = brief_json(vault.path(), "2026-W44");
    let mut first = first.clone();
    let mut second = second.clone();
    first["generated_at"] = serde_json::Value::Null;
    second["generated_at"] = serde_json::Value::Null;
    assert_eq!(first, second, "same vault, same now: same brief");
}

#[test]
fn brief_quarter_and_year_shape() {
    let vault = vault_dir("brief-shapes");
    write_reviews_fixture(vault.path());
    let quarter = brief_json(vault.path(), "2026-Q4");
    assert_eq!(
        quarter["parts"].as_array().expect("parts").len(),
        14,
        "a 14-week quarter: {}",
        quarter["parts"]
    );
    assert_eq!(
        quarter
            .get("columns")
            .and_then(|columns| columns.as_array())
            .map_or(0, Vec::len),
        0,
        "brief carries no strip columns"
    );
    assert!(quarter["time"]["roots"].as_array().is_some());
    let year = brief_json(vault.path(), "2026");
    assert_eq!(
        year["parts"].as_array().expect("parts").len(),
        4,
        "four quarters: {}",
        year["parts"]
    );
    assert_eq!(
        year["time"]["quarters"].as_array().expect("quarters").len(),
        4,
        "root shares by quarter: {}",
        year["time"]
    );
    assert!(
        year["practice"].is_object(),
        "practice section: {}",
        year["practice"]
    );
    for (period, budget) in [("2026-Q4", 10_000), ("2026", 12_000)] {
        let output = periods_command(vault.path(), "brief", &[period]);
        assert_success(&output);
        assert!(stdout(&output).len() / 4 <= budget, "{period} budget");
    }
}

#[test]
fn brief_refuses_all_time_and_bad_periods() {
    let vault = vault_dir("brief-refusals");
    write_reviews_fixture(vault.path());
    for (period, code) in [
        ("all_time", "no_review_section"),
        ("2026-W99", "invalid_period"),
    ] {
        let output =
            periods_command(vault.path(), "brief", &[period, "-f", "json"]);
        assert!(!output.status.success(), "{period} must fail");
        let body: serde_json::Value =
            serde_json::from_str(&stdout(&output)).expect("error JSON");
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], code);
        assert_eq!(output.status.code(), Some(2), "usage exit");
    }
    let human = periods_command(vault.path(), "brief", &["2026-W99"]);
    assert!(!human.status.success());
    assert!(
        String::from_utf8_lossy(&human.stderr).contains("bob periods brief"),
        "human errors name the command"
    );
}

fn write_review(
    vault: &std::path::Path,
    name: &str,
    text: &str,
) -> std::path::PathBuf {
    let path = vault.join(name);
    fs::write(&path, text).unwrap();
    path
}

fn draft(
    vault: &std::path::Path,
    period: &str,
    input: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    let mut args = vec![period, "-i", input.to_str().unwrap()];
    args.extend(extra);
    periods_command(vault, "draft", &args)
}

#[test]
fn draft_accepts_a_valid_text_and_a_second_draft() {
    let vault = vault_dir("draft-accept");
    write_reviews_fixture(vault.path());
    let input = write_review(
        vault.path(),
        "review.md",
        "**Headline:** A steady build week\n\nShipped the widget slice.\n",
    );
    let before = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("week note");
    let output = draft(vault.path(), "2026-W44", &input, &[]);
    assert_success(&output);
    assert!(
        stdout(&output).starts_with("Drafted 2026-W44 Review · "),
        "success line: {}",
        stdout(&output)
    );
    let after = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("drafted note");
    assert!(
        after.contains("## Review\n\n**Headline:** A steady build week\n\nShipped the widget slice.\n\n## Reflection"),
        "section replaced with blank lines:\n{after}"
    );
    assert!(
        after.contains("review_drafted: 2026-11-10"),
        "draft day stamped:\n{after}"
    );
    let digest = after
        .lines()
        .find(|line| line.starts_with("review_digest: "))
        .expect("digest line");
    assert!(
        digest.len() == "review_digest: sha256:".len() + 16,
        "short digest: {digest}"
    );
    for kept in [
        "[[2026/2026-Q4|↑ 2026-Q4]]",
        "> [!agent]- Agent instructions · weekly v1",
    ] {
        assert!(
            after.contains(kept),
            "untouched bytes survive ({kept}):\n{after}"
        );
    }
    assert!(
        !before.contains("review_digest:"),
        "fixture starts undrafted"
    );
    // A second draft is allowed.
    let again = write_review(
        vault.path(),
        "review2.md",
        "**Headline:** Revised take\n\nNew words.\n",
    );
    let output = draft(vault.path(), "2026-W44", &again, &[]);
    assert_success(&output);
    let redrafted = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("redrafted note");
    assert!(redrafted.contains("**Headline:** Revised take"));
    let list = periods_command(vault.path(), "list", &["-f", "json"]);
    assert_success(&list);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&list)).expect("list JSON");
    let row = body["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == "2026-W44")
        .expect("week row");
    assert_eq!(row["review_state"], "drafted");
    assert_eq!(row["headline"], "Revised take");
}

#[test]
fn draft_guards_open_and_edited_notes() {
    let vault = vault_dir("draft-guards");
    write_reviews_fixture(vault.path());
    let input = write_review(
        vault.path(),
        "review.md",
        "**Headline:** So far\n\nEarly words.\n",
    );
    // W46 is current on 2026-11-10.
    let output = draft(vault.path(), "2026-W46", &input, &[]);
    assert_eq!(output.status.code(), Some(3), "refused_open exit");
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&periods_command(
            vault.path(),
            "draft",
            &["2026-W46", "-i", input.to_str().unwrap(), "-f", "json"],
        )))
        .expect("refusal JSON");
    assert_eq!(body["error"]["code"], "refused_open");
    let forced = draft(vault.path(), "2026-W46", &input, &["-F"]);
    assert_success(&forced);

    // A hand edit after a draft refuses without -F.
    let closed = write_review(
        vault.path(),
        "closed.md",
        "**Headline:** Closed week\n\nDone things.\n",
    );
    assert_success(&draft(vault.path(), "2026-W44", &closed, &[]));
    let note = vault.path().join("2026/2026-W44.md");
    let edited = fs::read_to_string(&note)
        .unwrap()
        .replace("Done things.", "Done things, edited by Bryan.");
    fs::write(&note, edited).unwrap();
    let output = draft(vault.path(), "2026-W44", &closed, &[]);
    assert_eq!(output.status.code(), Some(3), "refused_state exit");
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&periods_command(
            vault.path(),
            "draft",
            &["2026-W44", "-i", closed.to_str().unwrap(), "-f", "json"],
        )))
        .expect("state JSON");
    assert_eq!(body["error"]["code"], "refused_state");
    assert_success(&draft(vault.path(), "2026-W44", &closed, &["-F"]));
}

#[test]
fn draft_rejects_bad_text_and_targets() {
    let vault = vault_dir("draft-validation");
    write_reviews_fixture(vault.path());
    let cases: Vec<(&str, &str, &str, i32)> = vec![
        ("no-headline.md", "Just words.\n", "missing_headline", 2),
        (
            "heading.md",
            "**Headline:** Ok\n\n# Not allowed\n",
            "invalid_draft",
            2,
        ),
        (
            "callout.md",
            "**Headline:** Ok\n\n> [!agent]- do this\n",
            "invalid_draft",
            2,
        ),
        ("fence.md", "**Headline:** Ok\n\n---\n", "invalid_draft", 2),
    ];
    for (name, text, code, exit) in cases {
        let input = write_review(vault.path(), name, text);
        let output =
            draft(vault.path(), "2026-W44", &input, &["-F", "-f", "json"]);
        assert_eq!(output.status.code(), Some(exit), "{name}");
        let body: serde_json::Value =
            serde_json::from_str(&stdout(&output)).expect("error JSON");
        assert_eq!(body["error"]["code"], code, "{name}");
    }
    let long = format!("**Headline:** Ok\n\n{}", "word ".repeat(300));
    let input = write_review(vault.path(), "long.md", &long);
    let output = draft(vault.path(), "2026-W44", &input, &["-F", "-f", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("cap JSON");
    assert_eq!(body["error"]["code"], "over_cap");
    assert_eq!(body["error"]["cap"], 250);
    assert!(
        body["error"]["count"].as_u64().unwrap_or(0) > 250,
        "count reported: {}",
        body["error"]
    );
    // Links count by their shown words: alias plus last segment.
    let linked = write_review(
        vault.path(),
        "links.md",
        "**Headline:** Ok\n\n[[2026/20261026|Mon]] did [[bob#^x|things]]\n",
    );
    let output = draft(vault.path(), "2026-W44", &linked, &["-F"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("· 5 words ·"),
        "link words reduced: {}",
        stdout(&output)
    );
    // all_time and missing notes have no Review.
    for period in ["all_time", "2026-W40"] {
        let output =
            draft(vault.path(), period, &linked, &["-F", "-f", "json"]);
        assert_eq!(output.status.code(), Some(2), "{period}");
        let body: serde_json::Value =
            serde_json::from_str(&stdout(&output)).expect("target JSON");
        assert_eq!(body["error"]["code"], "no_review_section", "{period}");
    }
}

#[test]
fn draft_dry_run_prints_the_planned_section() {
    let vault = vault_dir("draft-dry-run");
    write_reviews_fixture(vault.path());
    let input = write_review(
        vault.path(),
        "review.md",
        "**Headline:** Planned\n\nNot yet written.\n",
    );
    let before = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("week note");
    let output = draft(vault.path(), "2026-W44", &input, &["-n"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output).trim_end(),
        "## Review\n\n**Headline:** Planned\n\nNot yet written."
    );
    assert_eq!(
        fs::read_to_string(vault.path().join("2026/2026-W44.md"))
            .expect("untouched note"),
        before,
        "dry run writes nothing"
    );
}

#[test]
fn draft_enforces_quarter_and_year_caps() {
    let vault = vault_dir("draft-caps");
    write_reviews_fixture(vault.path());
    // 300 words: over the week cap, under the quarter cap.
    let words300 = format!("**Headline:** Ok\n\n{}", "word ".repeat(300));
    let input = write_review(vault.path(), "w300.md", &words300);
    let week = draft(vault.path(), "2026-W44", &input, &["-F", "-f", "json"]);
    assert_eq!(week.status.code(), Some(2));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&week)).expect("week cap JSON");
    assert_eq!(body["error"]["code"], "over_cap");
    assert_eq!(body["error"]["cap"], 250);
    let quarter = draft(vault.path(), "2026-Q4", &input, &["-F"]);
    assert_success(&quarter);

    // 450 words: over the quarter cap, under the year cap.
    let words450 = format!("**Headline:** Ok\n\n{}", "word ".repeat(450));
    let input = write_review(vault.path(), "w450.md", &words450);
    let quarter = draft(vault.path(), "2026-Q4", &input, &["-F", "-f", "json"]);
    assert_eq!(quarter.status.code(), Some(2));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&quarter)).expect("quarter cap JSON");
    assert_eq!(body["error"]["cap"], 400);
    let year = draft(vault.path(), "2026", &input, &["-F"]);
    assert_success(&year);
}

#[test]
fn draft_refuses_when_the_lock_is_busy() {
    use fs2::FileExt;
    let vault = vault_dir("draft-lock");
    write_reviews_fixture(vault.path());
    let input = write_review(
        vault.path(),
        "review.md",
        "**Headline:** Locked\n\nWaiting on the lock.\n",
    );
    let lock_path = vault.path().join("test-bob_sync.lock");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    held.try_lock_exclusive().expect("take lock");
    let output = bob_command()
        .arg("periods")
        .arg("draft")
        .arg("-b")
        .arg(vault.path())
        .arg("2026-W44")
        .arg("-i")
        .arg(&input)
        .arg("-F")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", vault.path())
        .env("BOB_NOW", REVIEWS_NOW)
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .output()
        .expect("locked draft");
    drop(held);
    assert_eq!(output.status.code(), Some(1), "lock_busy exit");
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("lock JSON");
    assert_eq!(body["error"]["code"], "lock_busy");
}

#[test]
fn draft_aborts_when_the_note_changes_under_it() {
    use fs2::FileExt;
    let vault = vault_dir("draft-stale");
    write_reviews_fixture(vault.path());
    let input = write_review(
        vault.path(),
        "review.md",
        "**Headline:** Stale\n\nRacy words.\n",
    );
    let note = vault.path().join("2026/2026-W44.md");
    let lock_path = vault.path().join("test-bob_sync.lock");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    held.try_lock_exclusive().expect("take lock");
    let child = bob_command()
        .arg("periods")
        .arg("draft")
        .arg("-b")
        .arg(vault.path())
        .arg("2026-W44")
        .arg("-i")
        .arg(&input)
        .arg("-F")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", vault.path())
        .env("BOB_NOW", REVIEWS_NOW)
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn racing draft");
    // The child blocks on the lock; change the note, then let it in.
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let changed = fs::read_to_string(&note)
        .unwrap()
        .replace("## Reflection\n", "## Reflection\n\nConcurrent edit.\n");
    fs::write(&note, &changed).unwrap();
    drop(held);
    let output = child.wait_with_output().expect("racing draft");
    assert_eq!(output.status.code(), Some(1), "stale_note exit");
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("stale JSON");
    assert_eq!(body["error"]["code"], "stale_note");
    assert!(
        fs::read_to_string(&note)
            .unwrap()
            .contains("Concurrent edit."),
        "the concurrent write wins"
    );
}

#[test]
fn ensure_refresh_skips_reflected_and_commits_once() {
    let temp = TempDir::new("bob-cli-periods-refresh");
    let vault = temp.path().join("vault");
    write_dataview_tasks_settings(&vault);
    init_git_vault(&vault);
    let _ = periods_command(&vault, "ensure", &["-d", "2026-11-02", "-q"]);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "scaffold"]);
    // One stale note, one stale-but-reflected note, one current note.
    let stale = vault.join("2026/2026-W45.md");
    let stale_before = fs::read_to_string(&stale).expect("stale note");
    fs::write(
        &stale,
        stale_before.replace(
            "Agent instructions · weekly v1",
            "Agent instructions · weekly v0",
        ),
    )
    .unwrap();
    let reflected = vault.join("2026/2026-W46.md");
    let reflected_before = fs::read_to_string(&reflected).expect("note");
    fs::write(
        &reflected,
        reflected_before
            .replace(
                "Agent instructions · weekly v1",
                "Agent instructions · weekly v0",
            )
            .replace("## Reflection\n", "## Reflection\n\nBryan reflected.\n"),
    )
    .unwrap();
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-qm", "stale callouts"]);

    let dry = periods_command(&vault, "ensure", &["-R", "-n", "-f", "json"]);
    assert_success(&dry);
    let plan: serde_json::Value =
        serde_json::from_str(&stdout(&dry)).expect("refresh plan");
    assert_eq!(plan["ok"], true);
    assert_eq!(plan["dry_run"], true);
    assert!(
        plan["refreshed"]
            .as_array()
            .is_some_and(|refreshed| refreshed
                .iter()
                .any(|path| path == "2026/2026-W45.md")),
        "stale note planned: {}",
        plan["refreshed"]
    );
    assert!(
        plan["skipped_reflected"]
            .as_array()
            .is_some_and(|skipped| skipped
                .iter()
                .any(|path| path == "2026/2026-W46.md")),
        "reflected note skipped: {}",
        plan["skipped_reflected"]
    );
    let log = stdout(&git_in(&vault, ["log", "--format=%s"]));
    assert_eq!(log.lines().count(), 3, "dry run commits nothing");

    let output = periods_command(&vault, "ensure", &["-R"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("refreshed 2026/2026-W45.md"),
        "human names the write:\n{}",
        stdout(&output)
    );
    let log = stdout(&git_in(&vault, ["log", "--format=%s"]));
    assert_eq!(
        log.lines().next(),
        Some("periods: refresh agent instructions to v1"),
        "one refresh commit:\n{log}"
    );
    assert_eq!(log.lines().count(), 4, "exactly one commit:\n{log}");
    let refreshed = fs::read_to_string(&stale).expect("refreshed note");
    assert!(
        refreshed.contains("Agent instructions · weekly v1"),
        "callout refreshed:\n{refreshed}"
    );
    assert!(
        !refreshed.contains("weekly v0"),
        "no stale callout left:\n{refreshed}"
    );
    assert_eq!(
        fs::read_to_string(&reflected).expect("reflected note"),
        reflected_before
            .replace(
                "Agent instructions · weekly v1",
                "Agent instructions · weekly v0",
            )
            .replace("## Reflection\n", "## Reflection\n\nBryan reflected.\n"),
        "reflected notes stay untouched"
    );

    let rerun = periods_command(&vault, "ensure", &["-R", "-f", "json"]);
    assert_success(&rerun);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&rerun)).expect("rerun JSON");
    assert!(
        body["refreshed"].as_array().is_some_and(Vec::is_empty),
        "re-run refreshes nothing: {}",
        body["refreshed"]
    );
    assert!(body["sha"].is_null(), "re-run commits nothing");
    let log = stdout(&git_in(&vault, ["log", "--format=%s"]));
    assert_eq!(log.lines().count(), 4, "re-run is a no-op:\n{log}");
}
