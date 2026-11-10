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
fn ensure_fills_a_frontmatter_only_stub_from_a_clicked_link() {
    // Clicking a dangling period link makes Templater write the vault's
    // folder template: frontmatter only, no body.
    let vault = vault_dir("stub");
    let stub = vault.path().join("2026/2026-W47.md");
    fs::create_dir_all(stub.parent().unwrap()).unwrap();
    fs::write(
        &stub,
        "---\nparent: \"[[2026/2026-W46|2026/2026-W46]]\"\ncreated: 2026-11-03T16:30:00-05:00\n---\n",
    )
    .unwrap();
    let output = ensure(vault.path(), "2026-11-16", &["-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("ensure JSON");
    assert!(
        body["filled"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "2026/2026-W47.md"),
        "the stub is reported as filled: {body}"
    );
    let filled = fs::read_to_string(&stub).unwrap();
    assert!(filled.contains("# 2026-W47"), "stub filled:\n{filled}");
    assert!(filled.contains("parent: \"[[2026/2026-Q4]]\""));
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
        adopted.contains("Agent instructions · yearly v2"),
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
            "6. repair generated notebook catalogs",
            "7. run vault-sync against the vault",
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
         \t\t- *2026-10-27* — first keep\n\
         \t\t- *2026-10-28* — second keep\n\
         \t\t- *2026-10-29* — waiting on review\n\
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
    assert_eq!(body["brief_version"], "brief v2");
    assert_eq!(body["schema_version"], 2);
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
    let parts = body["parts"].as_array().expect("parts");
    assert_eq!(parts.len(), 7, "seven day rows: {}", body["parts"]);
    assert!(
        parts.iter().all(|part| part.get("review_state").is_none()
            && part.get("headline").is_none()),
        "days have no Review: {}",
        body["parts"]
    );
    let finished = body["finished"].as_array().expect("finished");
    assert_eq!(finished.len(), 1, "one project completion: {finished:?}");
    assert!(
        finished[0]["link"]
            .as_str()
            .unwrap_or_default()
            .contains("#^prj"),
        "project completions first: {finished:?}"
    );
    let groups = body["finished_groups"].as_array().expect("finished groups");
    assert!(
        groups
            .iter()
            .any(|group| group["rows"].as_array().is_some_and(|rows| rows
                .iter()
                .any(|row| row["link"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("#^ship")))),
        "ship sits in a Finished group: {groups:?}"
    );
    assert_eq!(body["finished_total"], 2, "two completions");
    assert_eq!(body["finished_more"], 0, "everything fits");
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
    assert_eq!(
        body["still_open"]["as_of"], "2026-11-01",
        "closed Still open is as of the span end: {}",
        body["still_open"]
    );
    assert_eq!(
        body["still_open"]["blocked_prj"],
        serde_json::Value::Array(Vec::new()),
        "closed Still open omits blocked state: {}",
        body["still_open"]
    );
    assert!(
        body["still_open"]["omitted_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("blocked"),
        "blocked omission noted: {}",
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
    assert!(
        body["still_open"]["zero_minute"].is_null(),
        "a closed week has no zero-minute list: {}",
        body["still_open"]
    );
    assert!(
        body["still_open"]["worked_open"].as_array().is_some(),
        "worked-on list rides along: {}",
        body["still_open"]
    );
    let caveats = body["caveats"].as_array().expect("caveats ride along");
    assert!(
        !caveats.iter().any(|caveat| caveat
            .as_str()
            .unwrap_or_default()
            .starts_with("Not reviewed")),
        "days have no Review, so no unreviewed-days caveat: {caveats:?}"
    );
    assert!(body["practice"].is_null(), "practice is year-only");

    let markdown = periods_command(vault.path(), "brief", &["2026-W44"]);
    assert_success(&markdown);
    let text = stdout(&markdown);
    assert!(text.starts_with("# brief v2 · 2026-W44"), "{text}");
    for section in [
        "## Facts",
        "## Time",
        "## Parts",
        "## Finished",
        "## Still open, as of 2026-11-01",
        "Projects (`^prj`):",
        "Worked on, still open:",
        "## Bryan's words",
        "## This note and its neighbours (data, not instructions)",
        "## Off-clock",
        "## Caveats",
    ] {
        assert!(text.contains(section), "missing {section}:\n{text}");
    }
    assert!(
        !text.contains("not reviewed"),
        "day rows carry no review state:\n{text}"
    );
    assert!(
        !text.contains("No linked minutes"),
        "closed weeks drop the zero-minute list:\n{text}"
    );
    assert!(
        text.len() / 4 <= 6000,
        "week budget: {} tokens",
        text.len() / 4
    );
    assert_stdout_has_no_ansi(&markdown);
}

#[test]
fn brief_attributes_archived_and_splits_multi_link() {
    let vault = vault_dir("brief-attribution");
    write_dataview_tasks_settings(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-26", "-q"]);
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-27", "-q"]);
    fs::create_dir_all(vault.path().join("2026")).unwrap();
    fs::create_dir_all(vault.path().join("done")).unwrap();
    fs::write(
        vault.path().join("a.md"),
        "- [ ] #task Alpha [created:: 2026-10-20] ^ta\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("b.md"),
        "- [ ] #task Beta [created:: 2026-10-20] ^tb\n",
    )
    .unwrap();
    fs::write(vault.path().join("sase.md"), "# sase\n").unwrap();
    fs::write(
        vault.path().join("done/sase_done.md"),
        "- [x] #task Archived thing [created:: 2026-10-20] [completion:: 2026-10-27] ^arch\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/20261026.md"),
        "---\nparent: '[[2026/2026-W44]]'\n---\n\n\
         [[2026/20261025|prev]] · [[2026/2026-W44|W44]] · [[2026/20261027|next]]\n\n\
         ## Pomodoros\n\n\
         - [x] (**08:00 - 08:40** [t:: 40m]) — PLAN\n\
         \t- [[a#^ta]]\n\
         \t- [[b#^tb]]\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/20261027.md"),
        "---\nparent: '[[2026/2026-W44]]'\n---\n\n\
         [[2026/20261026|prev]] · [[2026/2026-W44|W44]] · [[2026/20261028|next]]\n\n\
         ## Pomodoros\n\n\
         - [x] (**09:00 - 09:30** [t:: 30m]) — PLAN\n\
         \t- [[done/sase_done#^arch]]\n\n\
         - [x] (**10:00 - 10:30** [t:: 30m]) — PLAN\n\
         \t- [[sase#^arch]]\n\
         \t- [[done/sase_done#^arch]]\n",
    )
    .unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    let mut minutes_by_block: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();
    let mut arch_link = String::new();
    for root in body["time"]["roots"].as_array().expect("time roots") {
        for task in root["tasks"].as_array().expect("root tasks") {
            let link = task["link"].as_str().unwrap_or_default().to_string();
            let minutes = task["minutes"].as_u64().unwrap_or(0);
            if link.contains("#^ta") {
                minutes_by_block.insert("ta".to_string(), minutes);
            } else if link.contains("#^tb") {
                minutes_by_block.insert("tb".to_string(), minutes);
            } else if link.contains("#^arch") {
                minutes_by_block.insert("arch".to_string(), minutes);
                arch_link = link;
            }
        }
    }
    assert_eq!(
        minutes_by_block.get("ta").copied().unwrap_or(0),
        20,
        "40m across two links splits 20/20: {}",
        body["time"]
    );
    assert_eq!(
        minutes_by_block.get("tb").copied().unwrap_or(0),
        20,
        "40m across two links splits 20/20: {}",
        body["time"]
    );
    assert_eq!(
        minutes_by_block.get("arch").copied().unwrap_or(0),
        60,
        "archived task gets 30m plus a once-counted 30m duplicate: {}",
        body["time"]
    );
    assert!(
        arch_link.contains("done/sase_done"),
        "archived task renders its done/ home: {arch_link}"
    );
    let groups = body["finished_groups"].as_array().expect("groups");
    let group = groups
        .iter()
        .find(|group| {
            group["rows"].as_array().is_some_and(|rows| {
                rows.iter().any(|row| {
                    row["link"].as_str().unwrap_or_default().contains("#^arch")
                })
            })
        })
        .expect("archived completion in a Finished group");
    assert_eq!(
        group["note"], "sase",
        "the done/ archive groups under its source note: {group}"
    );
    let archived_row = group["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["link"].as_str().unwrap_or_default().contains("#^arch"))
        .expect("archived row");
    assert!(
        archived_row["link"]
            .as_str()
            .unwrap_or_default()
            .contains("done/sase_done"),
        "finished renders the done/ link: {group}"
    );
    assert_eq!(
        archived_row["minutes"].as_u64().unwrap_or(0),
        60,
        "finished ranks by split minutes: {group}"
    );
}

#[test]
fn brief_finished_and_dropped_report_totals_behind_the_cap() {
    // bob-cli-gv: the weekly Dropped cap (7) hides rows without reporting
    // the cut, so the brief carries totals and +N more. brief v2: Finished
    // lists every non-routine completion grouped by note, on the Done
    // fact's basis, and names the routine rows it leaves out.
    let vault = vault_dir("brief-totals");
    write_dataview_tasks_settings(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-26", "-q"]);
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-11-02", "-q"]);
    fs::create_dir_all(vault.path().join("2026")).unwrap();
    let mut tasks = String::from(
        "- [x] #task Big launch [created:: 2026-10-18] [completion:: 2026-10-27] ^prj\n",
    );
    for n in 1..=8 {
        tasks.push_str(&format!(
            "- [x] #task Done task {n:02} [created:: 2026-10-18] [completion:: 2026-10-27] ^done{n:02}\n"
        ));
    }
    for n in 1..=8 {
        tasks.push_str(&format!(
            "- [-] #task Dropped task {n:02} [created:: 2026-10-18] [cancelled:: 2026-10-27] ^drop{n:02}\n"
        ));
    }
    // Routine rows: a recurring task and a daily `^gtd` review.
    tasks.push_str(
        "- [x] #task #gtd Water plants [repeat:: every day] [created:: 2026-10-18] [completion:: 2026-10-27] ^water\n",
    );
    fs::write(vault.path().join("tasks.md"), tasks).unwrap();
    fs::write(
        vault.path().join("2026/20261027.md"),
        "---\nparent: '[[2026/2026-W44]]'\n---\n\n\
         - [x] #task [[gtd_daily]] [root:: gtd] [created:: 2026-10-27] [completion:: 2026-10-27] ^gtd\n",
    )
    .unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    let finished = body["finished"].as_array().expect("project rows");
    assert_eq!(
        body["finished_total"], 9,
        "nine non-routine completions: {}",
        body["finished_total"]
    );
    assert_eq!(finished.len(), 1, "one project completion: {finished:?}");
    assert!(
        finished[0]["link"]
            .as_str()
            .unwrap_or_default()
            .contains("#^prj"),
        "project completions first: {finished:?}"
    );
    let groups = body["finished_groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "one note: {groups:?}");
    assert_eq!(groups[0]["note"], "tasks");
    assert_eq!(groups[0]["count"], 8, "eight grouped rows: {groups:?}");
    assert_eq!(
        groups[0]["rows"].as_array().map_or(0, Vec::len),
        8,
        "every row fits the week budget: {groups:?}"
    );
    assert_eq!(groups[0]["more"], 0);
    assert_eq!(
        body["finished_more"], 0,
        "nothing cut: {}",
        body["finished_more"]
    );
    assert_eq!(
        body["finished_routine_omitted"], 2,
        "recurring row and daily review omitted: {body}"
    );
    assert_eq!(body["finished_daily_review_omitted"], 1);
    let all_links = serde_json::to_string(&body["finished_groups"]).unwrap();
    assert!(!all_links.contains("#^water"), "routine row hidden");
    assert!(!all_links.contains("#^gtd"), "daily review hidden");
    let dropped = body["dropped"].as_array().expect("dropped rows");
    assert_eq!(
        body["dropped_total"], 8,
        "eight cancellations: {}",
        body["dropped_total"]
    );
    assert_eq!(dropped.len(), 7, "weekly cap still applies: {dropped:?}");
    assert_eq!(
        body["dropped_more"], 1,
        "one row cut: {}",
        body["dropped_more"]
    );

    let output = periods_command(vault.path(), "brief", &["2026-W44"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("9 completions"), "finished basis:\n{text}");
    assert!(
        text.contains("Omitted as routine: 2"),
        "routine line:\n{text}"
    );
    assert!(text.contains("tasks · 8 done:"), "group line:\n{text}");
    assert!(text.contains("+1 more"), "dropped tail: ...{text}");
}

#[test]
fn brief_finished_groups_share_the_budget_round_robin() {
    // A heavy week: grouped Finished rows fill the room the other
    // sections leave, one row per group per round, and every cut carries a
    // bob-computed `+N more`.
    let vault = vault_dir("brief-finished-fit");
    write_dataview_tasks_settings(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-26", "-q"]);
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-11-02", "-q"]);
    let mut alpha = String::new();
    for n in 0..400 {
        alpha.push_str(&format!(
            "- [x] #task Alpha completion number {n:03} with a long descriptive title [created:: 2026-10-18] [completion:: 2026-10-27] ^alpha{n:03}\n"
        ));
    }
    fs::write(vault.path().join("alpha.md"), alpha).unwrap();
    let mut beta = String::new();
    for n in 0..5 {
        beta.push_str(&format!(
            "- [x] #task Beta item {n} [created:: 2026-10-18] [completion:: 2026-10-27] ^beta{n}\n"
        ));
    }
    fs::write(vault.path().join("beta.md"), beta).unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    assert_eq!(body["finished_total"], 405);
    let groups = body["finished_groups"].as_array().expect("groups");
    let group = |note: &str| {
        groups
            .iter()
            .find(|group| group["note"] == note)
            .unwrap_or_else(|| panic!("{note} group: {groups:?}"))
            .clone()
    };
    let alpha = group("alpha");
    let beta = group("beta");
    assert_eq!(alpha["count"], 400);
    let alpha_shown = alpha["rows"].as_array().map_or(0, Vec::len) as u64;
    assert!(
        alpha_shown > 5,
        "alpha gets the leftover room: {alpha_shown}"
    );
    assert_eq!(
        alpha["more"].as_u64().unwrap_or(0),
        400 - alpha_shown,
        "alpha's tail is bob-computed"
    );
    assert_eq!(
        beta["rows"].as_array().map_or(0, Vec::len),
        5,
        "the small group shows every row: {beta}"
    );
    assert_eq!(
        body["finished_more"].as_u64().unwrap_or(0),
        400 - alpha_shown,
        "finished_more counts every row not shown"
    );
    let output = periods_command(vault.path(), "brief", &["2026-W44"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains(&format!("- +{} more", 400 - alpha_shown)),
        "alpha tail:\n{text}"
    );
    assert!(text.len() / 4 <= 6000, "week budget: {}", text.len() / 4);
}

#[test]
fn brief_bryans_words_dedups_orders_and_sanitizes_links() {
    // bob-cli-gw: mirrored WORK LOG lines duplicate Pomodoro children,
    // same-day rows sort by link text, checkbox trims eat leading `x`,
    // link-only children render as mangled aliases, and nested links nest.
    let vault = vault_dir("brief-words");
    write_dataview_tasks_settings(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-26", "-q"]);
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-11-02", "-q"]);
    fs::create_dir_all(vault.path().join("2026")).unwrap();
    fs::write(
        vault.path().join("proj.md"),
        "---\nparent: \"[[org]]\"\n---\n\n\
         - [ ] #task Stuck project [created:: 2026-10-20] ^stuck\n\
         \t- 🛠️ **WORK LOG**\n\
         \t\t- *2026-10-27 14:03* — shipped the widget slice\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/20261027.md"),
        "---\nparent: '[[2026/2026-W44]]'\n---\n\n\
         [[2026/20261026|prev]] · [[2026/2026-W44|W44]] · [[2026/20261028|next]]\n\n\
         ## Pomodoros\n\n\
         - [x] (**09:00 - 09:40** [t:: 40m]) — PLAN\n\
         \t- zebra thought\n\
         \t- apple thought\n\
         \t- shipped the widget slice\n\
         \t- xprompt follow-up\n\
         \t- [[sase_better_tasks]]\n\
         \t- review [[sase]] notes\n\
         \t- apollo agent\n\
         \t- athena agents\n\
         \t- athena agent to add songs\n",
    )
    .unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    let words = body["bryans_words"].as_array().expect("words");
    let texts: Vec<&str> = words
        .iter()
        .map(|word| word["text"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !texts.contains(&"apollo agent") && !texts.contains(&"athena agents"),
        "bare host labels are not Bryan's words: {texts:?}"
    );
    assert!(
        texts.contains(&"athena agent to add songs"),
        "a sentence that names a host stays: {texts:?}"
    );
    let shipped: Vec<&serde_json::Value> = words
        .iter()
        .filter(|word| {
            word["text"].as_str().unwrap_or_default()
                == "shipped the widget slice"
        })
        .collect();
    assert_eq!(
        shipped.len(),
        1,
        "work log mirror deduped to one entry: {words:?}"
    );
    assert!(
        shipped[0]["link"]
            .as_str()
            .unwrap_or_default()
            .contains("20261027"),
        "daily-note link wins the duplicate: {}",
        shipped[0]["link"]
    );
    let day: Vec<String> = words
        .iter()
        .filter(|word| word["date"] == "2026-10-27")
        .map(|word| word["text"].as_str().unwrap_or_default().to_string())
        .collect();
    let zebra = day
        .iter()
        .position(|text| text == "zebra thought")
        .expect("zebra");
    let apple = day
        .iter()
        .position(|text| text == "apple thought")
        .expect("apple");
    assert!(
        zebra < apple,
        "date-only stable order keeps insertion before alphabetic link: {day:?}"
    );
    assert!(
        words.iter().any(|word| {
            word["text"].as_str().unwrap_or_default() == "xprompt follow-up"
        }),
        "leading x survives the checkbox trim: {words:?}"
    );
    assert!(
        !words.iter().any(|word| {
            word["text"]
                .as_str()
                .unwrap_or_default()
                .contains("sase_better_tasks")
        }),
        "link-only child is skipped: {words:?}"
    );
    for word in words {
        let link = word["link"].as_str().unwrap_or_default();
        assert_eq!(
            link.matches("[[").count(),
            1,
            "no nested link in alias: {link}"
        );
        assert_eq!(
            link.matches("]]").count(),
            1,
            "no nested link in alias: {link}"
        );
    }
}

#[test]
fn brief_parts_skip_empty_dates_and_still_open_cleans_aliases() {
    // bob-cli-gx: week Part lines print an empty `·  ·` dates segment,
    // and Still open aliases leak raw block ids and inline fields.
    let vault = vault_dir("brief-alias");
    write_dataview_tasks_settings(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-26", "-q"]);
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-11-02", "-q"]);
    fs::create_dir_all(vault.path().join("2026")).unwrap();
    fs::write(
        vault.path().join("proj.md"),
        "---\nparent: \"[[org]]\"\n---\n\n\
         - [*] #task Better Obsidian image support! [created:: 2026-10-26] ^better-obsidian-image\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/20261027.md"),
        "---\nparent: '[[2026/2026-W44]]'\n---\n\n\
         ## Pomodoros\n\n\
         - [x] (**09:00 - 09:25** [t:: 25m]) — BUILD\n\
         \t- [[proj#^better-obsidian-image]]\n",
    )
    .unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    let worked = body["still_open"]["worked_open"]
        .as_array()
        .expect("worked_open");
    let row = worked
        .iter()
        .find(|row| {
            row["link"]
                .as_str()
                .unwrap_or_default()
                .contains("Better Obsidian")
        })
        .expect("worked-on open task listed");
    assert_eq!(row["display"], "25m", "worked-on rows carry minutes: {row}");
    let link = row["link"].as_str().unwrap_or_default();
    let alias = link.split('|').nth(1).unwrap_or(link);
    assert!(!alias.contains("^better"), "no raw block id: {link}");
    assert!(!alias.contains("created::"), "no inline field: {link}");
    assert!(!alias.contains("…"), "no mid-id truncation: {link}");

    let output = periods_command(vault.path(), "brief", &["2026-W44"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(!text.contains("·  ·"), "no empty dates segment:\n{text}");
    for line in text.lines().filter(|line| line.contains("Better Obsidian")) {
        let alias = line.split('|').nth(1).unwrap_or(line);
        assert!(!alias.contains("^better"), "md alias clean: {line}");
    }
}

#[test]
fn brief_still_open_rebuilds_past_period_as_of_span_end() {
    // bob-cli-gu / brief-still-open (rebuild): a closed period's Still open
    // must not report today's vault. Kept counts dated Rotten Log entries
    // on or before the span end, and Blocked projects is omitted with a
    // note. bob-cli-iy: Worked on, still open lists tasks open at the end
    // with linked minutes in the span; the closed period has no
    // zero-minute list (lane history is gone, and no minutes is no stall).
    let vault = vault_dir("brief-still-open");
    write_dataview_tasks_settings(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-26", "-q"]);
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-11-02", "-q"]);
    fs::create_dir_all(vault.path().join("2026")).unwrap();
    fs::write(
        vault.path().join("proj.md"),
        "---\nparent: \"[[org]]\"\n---\n\n\
         - [*] #task Past open [created:: 2026-10-20] ^pastopen\n\
         - [*] #task Future thing [created:: 2026-11-05] ^future1\n\
         - [x] #task Old done [created:: 2026-10-20] [completion:: 2026-11-05] ^olddone\n\
         - [x] #task Done in span [created:: 2026-10-20] [completion:: 2026-10-28] ^donein\n\
         - [*] #task Never touched [created:: 2026-10-20] ^untouched\n\
         - [ ] #task Past kept [created:: 2026-10-20] [keeps:: 3] ^pastkept\n\
         \t- 🥀 **ROTTEN LOG**\n\
         \t\t- *2026-10-27* — first keep\n\
         \t\t- *2026-10-28* — second keep\n\
         \t\t- *2026-10-29* — third keep\n\
         - [ ] #task Inflated keeps [created:: 2026-10-20] [keeps:: 5] ^inflated\n\
         \t- 🥀 **ROTTEN LOG**\n\
         \t\t- *2026-10-27* — first keep\n\
         \t\t- *2026-10-28* — second keep\n\
         - [ ] #task Future kept [created:: 2026-11-05] [keeps:: 3] ^futurekept\n\
         \t- 🥀 **ROTTEN LOG**\n\
         \t\t- *2026-11-05* — first keep\n\
         \t\t- *2026-11-06* — second keep\n\
         \t\t- *2026-11-07* — third keep\n\
         - [ ] #task Blocked build #prj_is_blocked [created:: 2026-10-20] ^prj\n\
         \t- Depends-On: [[proj#^pastopen]]\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/20261027.md"),
        "---\nparent: '[[2026/2026-W44]]'\n---\n\n\
         ## Pomodoros\n\n\
         - [x] (**09:00 - 09:30** [t:: 30m]) — BUILD\n\
         \t- [[proj#^pastopen]]\n\n\
         - [x] (**10:00 - 10:20** [t:: 20m]) — BUILD\n\
         \t- [[proj#^olddone]]\n\n\
         - [x] (**11:00 - 11:10** [t:: 10m]) — BUILD\n\
         \t- [[proj#^donein]]\n",
    )
    .unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    assert_eq!(body["header"]["state"], "closed");
    let still = &body["still_open"];
    assert_eq!(
        still["as_of"], "2026-11-01",
        "past Still open is as of the span end: {still}"
    );
    assert!(
        still["zero_minute"].is_null(),
        "closed periods have no zero-minute list: {still}"
    );
    let worked = still["worked_open"].as_array().expect("worked_open");
    let has = |block: &str| {
        worked
            .iter()
            .any(|row| row["link"].as_str().unwrap_or_default().contains(block))
    };
    assert!(has("#^pastopen"), "worked-on open task listed: {worked:?}");
    assert_eq!(
        worked[0]["display"], "30m",
        "ranked by linked minutes: {worked:?}"
    );
    assert!(
        has("#^olddone"),
        "task open at span end but done now still listed: {worked:?}"
    );
    assert!(
        !has("#^donein"),
        "task finished inside the span is not open: {worked:?}"
    );
    assert!(
        !has("#^untouched") && !has("#^future1"),
        "no linked minutes, not listed: {worked:?}"
    );
    let kept = still["kept"].as_array().expect("kept");
    let kept_has = |block: &str| {
        kept.iter()
            .any(|row| row["link"].as_str().unwrap_or_default().contains(block))
    };
    assert!(kept_has("#^pastkept"), "3 dated keeps listed: {kept:?}");
    let past_row = kept
        .iter()
        .find(|row| {
            row["link"]
                .as_str()
                .unwrap_or_default()
                .contains("#^pastkept")
        })
        .expect("past kept row");
    assert_eq!(
        past_row["keeps"], 3,
        "kept counts dated entries, not live keeps: {past_row}"
    );
    assert!(
        !kept_has("#^inflated"),
        "only 2 dated keeps on or before span end: {kept:?}"
    );
    assert!(
        !kept_has("#^futurekept"),
        "keeps after the span end omitted: {kept:?}"
    );
    assert_eq!(
        still["blocked_prj"],
        serde_json::Value::Array(Vec::new()),
        "blocked state has no history: {}",
        still["blocked_prj"]
    );
    assert!(
        still["omitted_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("blocked"),
        "blocked omission noted in JSON: {still}"
    );
    let output = periods_command(vault.path(), "brief", &["2026-W44"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("## Still open, as of 2026-11-01"),
        "past title carries the span end:\n{text}"
    );
    assert!(
        text.contains("Omitted"),
        "past Blocked projects omitted with a note:\n{text}"
    );
    assert!(
        !text.contains("Next or WIP with no linked minutes"),
        "closed periods drop the zero-minute list:\n{text}"
    );
}

#[test]
fn brief_carries_the_previous_review() {
    // brief v2: the previous same-kind Review's Headline (whole first
    // paragraph on one line), Carried forward (Still open in v1), and
    // Questions ride next to its Reflection.
    let vault = vault_dir("brief-prev-review");
    write_reviews_fixture(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-19", "-q"]);
    let w43 = vault.path().join("2026/2026-W43.md");
    let scaffold = fs::read_to_string(&w43).expect("W43 scaffold");
    let review = "## Review\n\n\
        **Headline:** Shipped the widget and the launch,\n\
        then planned the next slice.\n\n\
        **Shipped:**\n\n\
        - [[proj#^ship|Widget]]\n\n\
        **Carried forward:**\n\n\
        - [[proj#^stuck|Stuck project]] · kept 3×\n\n\
        **Questions:**\n\n\
        1. Is the stuck project waiting on a decision?\n\n\
        ## Reflection\n";
    let updated = scaffold.replacen("## Review\n\n## Reflection\n", review, 1);
    assert_ne!(updated, scaffold, "scaffold carries an empty Review");
    fs::write(&w43, updated).unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    let prev = &body["neighbours"]["prev_review"];
    assert_eq!(
        prev["headline"],
        "Shipped the widget and the launch, then planned the next slice.",
        "{prev}"
    );
    assert_eq!(prev["carried_label"], "Carried forward");
    assert_eq!(prev["carried"], "- [[proj#^stuck|Stuck project]] · kept 3×");
    assert_eq!(
        prev["questions"],
        "1. Is the stuck project waiting on a decision?"
    );
    let output = periods_command(vault.path(), "brief", &["2026-W44"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains(
            "### Previous Review: Headline\nShipped the widget and the launch, then planned the next slice.\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("### Previous Review: Carried forward (≤150 words)\n"),
        "{text}"
    );
    assert!(
        text.contains("### Previous Review: Questions (≤150 words)\n"),
        "{text}"
    );
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
fn brief_markdown_format_matches_the_agent_callouts() {
    // Every agent callout tells the agent to run `bob periods brief <id> -f md`.
    let vault = vault_dir("brief-md-format");
    write_reviews_fixture(vault.path());
    let default = periods_command(vault.path(), "brief", &["2026-W44"]);
    assert_success(&default);
    for format in ["md", "human"] {
        let output =
            periods_command(vault.path(), "brief", &["2026-W44", "-f", format]);
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            stdout(&default),
            "-f {format} prints the Markdown bundle"
        );
    }
    assert!(
        stdout(&default).starts_with('#'),
        "the Markdown bundle opens with a heading"
    );
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
    let has_caveat = |body: &serde_json::Value, prefix: &str| {
        body["caveats"].as_array().is_some_and(|caveats| {
            caveats.iter().any(|caveat| {
                caveat.as_str().unwrap_or_default().starts_with(prefix)
            })
        })
    };
    assert!(
        has_caveat(&quarter, "Not reviewed weeks:"),
        "quarter names its unreviewed weeks: {}",
        quarter["caveats"]
    );
    assert!(
        has_caveat(&year, "Not reviewed quarters:"),
        "year names its unreviewed quarters: {}",
        year["caveats"]
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

#[test]
fn relative_period_ids_resolve_like_canonical_ids() {
    // REVIEWS_NOW is 2026-11-10 (in 2026-W46), so last-week is 2026-W45.
    let vault = vault_dir("relative-ids");
    write_reviews_fixture(vault.path());

    // `show` takes no -b/--bob-dir flag; the vault travels via BOB_DIR.
    let show_in = |period: &str, extra: &[&str]| {
        let mut command = bob_command();
        command
            .arg("periods")
            .arg("show")
            .env("BOB_DIR", vault.path())
            .env("BOB_NOW", REVIEWS_NOW)
            .env("NO_COLOR", "1");
        command.arg(period);
        for arg in extra {
            command.arg(arg);
        }
        command.output().expect("run bob periods show")
    };
    let show_relative = show_in("last-week", &["-f", "json"]);
    assert_success(&show_relative);
    let relative_body: serde_json::Value =
        serde_json::from_str(&stdout(&show_relative)).expect("show JSON");
    assert_eq!(relative_body["period"]["id"], "2026-W45");
    assert_eq!(relative_body["note"]["path"], "2026/2026-W45.md");

    let show_canonical = show_in("2026-W45", &["-f", "json"]);
    assert_success(&show_canonical);
    let canonical_body: serde_json::Value =
        serde_json::from_str(&stdout(&show_canonical)).expect("show JSON");
    assert_eq!(
        relative_body["period"]["id"],
        canonical_body["period"]["id"]
    );
    assert_eq!(
        relative_body["note"]["path"],
        canonical_body["note"]["path"]
    );

    // PI5: case- and `_`/`-`-insensitive.
    for alias in ["LAST-WEEK", "last_week"] {
        let output = show_in(alias, &["-f", "json"]);
        assert_success(&output);
        let body: serde_json::Value =
            serde_json::from_str(&stdout(&output)).expect("show JSON");
        assert_eq!(body["period"]["id"], "2026-W45", "{alias}");
    }

    // Human `show` names the canonical id and the vault-relative path.
    let human = show_in("last-week", &[]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("2026-W45"), "human names the id:\n{text}");
    assert!(
        text.contains("2026/2026-W45.md"),
        "human names the note path:\n{text}"
    );

    // `brief last-week` matches `brief 2026-W45`.
    let brief_body = brief_json(vault.path(), "last-week");
    assert_eq!(brief_body["id"], "2026-W45");

    // `draft last-week` drafts exactly the canonical note (PI7).
    let input = write_review(
        vault.path(),
        "relative.md",
        "**Headline:** Relative week\n\nShipped it.\n",
    );
    let output =
        draft(vault.path(), "last-week", &input, &["-F", "-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("draft JSON");
    assert_eq!(body["id"], "2026-W45");
    assert!(
        fs::read_to_string(vault.path().join("2026/2026-W45.md"))
            .unwrap()
            .contains("**Headline:** Relative week"),
        "draft last-week writes the canonical note"
    );

    // PI6: unknown relative words stay usage errors whose message lists
    // the accepted relative forms.
    let output = show_in("last-month", &["-f", "json"]);
    assert_eq!(output.status.code(), Some(2), "show exit");
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error JSON");
    assert_eq!(body["error"]["code"], "invalid_period");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    for accepted in ["last-week", "last-quarter", "last-year"] {
        assert!(
            message.contains(accepted),
            "show message lists {accepted}: {message}"
        );
    }
    for (subcommand, args) in [("brief", vec!["next-week", "-f", "json"])] {
        let output = periods_command(vault.path(), subcommand, &args);
        assert_eq!(output.status.code(), Some(2), "{subcommand} exit");
        let body: serde_json::Value =
            serde_json::from_str(&stdout(&output)).expect("error JSON");
        assert_eq!(body["error"]["code"], "invalid_period");
        let message = body["error"]["message"].as_str().unwrap_or_default();
        for accepted in ["last-week", "last-quarter", "last-year"] {
            assert!(
                message.contains(accepted),
                "{subcommand} message lists {accepted}: {message}"
            );
        }
    }
    let draft_input = write_review(
        vault.path(),
        "relative-bad.md",
        "**Headline:** Bad\n\nWords.\n",
    );
    let output =
        draft(vault.path(), "last-weeks", &draft_input, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(2), "draft exit");
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error JSON");
    assert_eq!(body["error"]["code"], "invalid_period");
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
fn draft_joins_a_wrapped_headline_and_parents_read_it_whole() {
    let vault = vault_dir("draft-wrapped-headline");
    write_reviews_fixture(vault.path());
    let input = write_review(
        vault.path(),
        "review.md",
        "**Headline:** Shipped the widget slice and the big launch, and\n\
         closed the first weekly review.\n\n**Shipped:**\n\n- [[2026/20261026#^ship|Widget]]\n",
    );
    let output = draft(vault.path(), "2026-W44", &input, &["-f", "json"]);
    assert_success(&output);
    let after = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("drafted note");
    assert!(
        after.contains(
            "## Review\n\n**Headline:** Shipped the widget slice and the big launch, and closed the first weekly review.\n\n**Shipped:**\n"
        ),
        "Headline joined onto one physical line:\n{after}"
    );
    let list = periods_command(vault.path(), "list", &["-f", "json"]);
    assert_success(&list);
    let rows: serde_json::Value =
        serde_json::from_str(&stdout(&list)).expect("list json");
    let w44 = rows["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == "2026-W44")
        .expect("W44 row");
    assert_eq!(w44["review_state"], "drafted");
    assert_eq!(
        w44["headline"],
        "Shipped the widget slice and the big launch, and closed the first weekly review."
    );

    // The 25-word cap counts the whole paragraph, not its first line.
    let long = write_review(
        vault.path(),
        "long.md",
        "**Headline:** one two three four five six seven eight nine ten\n\
         eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen\n\
         nineteen twenty twenty-one twenty-two twenty-three twenty-four twenty-five twenty-six\n",
    );
    let output = draft(vault.path(), "2026-W44", &long, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error json");
    assert_eq!(body["error"]["code"], "over_cap");
    assert_eq!(body["error"]["count"], 26);
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
        "> [!agent]- Agent instructions · weekly v2\n",
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
            "Agent instructions · weekly v2",
            "Agent instructions · weekly v1",
        ),
    )
    .unwrap();
    let reflected = vault.join("2026/2026-W46.md");
    let reflected_before = fs::read_to_string(&reflected).expect("note");
    fs::write(
        &reflected,
        reflected_before
            .replace(
                "Agent instructions · weekly v2",
                "Agent instructions · weekly v1",
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
        Some("periods: refresh agent instructions to v2"),
        "one refresh commit:\n{log}"
    );
    assert_eq!(log.lines().count(), 4, "exactly one commit:\n{log}");
    let refreshed = fs::read_to_string(&stale).expect("refreshed note");
    assert!(
        refreshed.contains("Agent instructions · weekly v2"),
        "callout refreshed:\n{refreshed}"
    );
    assert!(
        !refreshed.contains("weekly v1"),
        "no stale callout left:\n{refreshed}"
    );
    assert_eq!(
        fs::read_to_string(&reflected).expect("reflected note"),
        reflected_before
            .replace(
                "Agent instructions · weekly v2",
                "Agent instructions · weekly v1",
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

fn write_area(vault: &std::path::Path, stem: &str, title: &str) {
    write_file(
        &vault.join(format!("{stem}.md")),
        &format!(
            "---\ntype: \"[[area]]\"\nparent: \"[[area]]\"\n---\n# {title}\n\n## Tasks\n- [ ] #task {title} work ^{stem}\n"
        ),
    );
}

fn write_daily(vault: &std::path::Path, date: &str, pomodoros: &str) {
    let compact = date.replace('-', "");
    let year = &date[..4];
    write_file(
        &vault.join(format!("{year}/{compact}.md")),
        &format!("# {date}\n\n## Pomodoros\n\n{pomodoros}"),
    );
}

fn write_charts_config(path: &std::path::Path, extra_stats: &str) {
    write_file(
        path,
        &format!(
            "types:\n  BUILD: yellow\n  FIX: red\n  PLAN: auto\nstats:\n  period_strip:\n    top_roots: 5\n{extra_stats}"
        ),
    );
}

fn write_charts_vault(vault: &std::path::Path) {
    write_dataview_tasks_settings(vault);
    for (stem, title) in [
        ("bob", "Bob"),
        ("sase", "SASE"),
        ("job", "Job"),
        ("rest", "Rest"),
        ("other", "Other"),
        ("cash", "Cash"),
    ] {
        write_area(vault, stem, title);
    }
    // 2026-W45 (closed): mixed roots, types, labels, unnamed, unlinked.
    write_daily(
        vault,
        "2026-11-02",
        "- [x] (**08:00 - 08:30** [t:: 30m]) — BUILD\n\t- [[bob#^bob]]\n\
         - [x] (**09:00 - 09:30** [t:: 30m]) — BUILD + SASE\n\t- [[sase#^sase]]\n\
         - [x] (**10:00 - 10:20** [t:: 20m]) \n\t- [[bob#^bob]]\n",
    );
    write_daily(
        vault,
        "2026-11-03",
        "- [x] (**08:00 - 08:25** [t:: 25m]) — BUILD + FIX\n\t- [[job#^job]]\n",
    );
    write_daily(
        vault,
        "2026-11-04",
        "- [x] (**08:00 - 08:40** [t:: 40m]) — BUILD\n",
    );
    write_daily(
        vault,
        "2026-11-05",
        "- [x] (**08:00 - 08:15** [t:: 15m]) — BUILD\n\t- [[rest#^rest]]\n\
         - [x] (**09:00 - 09:10** [t:: 10m]) — BUILD\n\t- [[other#^other]]\n\
         - [x] (**10:00 - 10:08** [t:: 8m]) — BUILD\n\t- [[cash#^cash]]\n",
    );
    // Current week W46 so coverage copy has Mon–Tue.
    write_daily(
        vault,
        "2026-11-09",
        "- [x] (**08:00 - 08:30** [t:: 30m]) — BUILD\n\t- [[bob#^bob]]\n",
    );
    write_daily(
        vault,
        "2026-11-10",
        "- [x] (**08:00 - 09:00** [t:: 60m]) — PLAN\n\t- [[bob#^bob]]\n",
    );
}

fn show_json(
    vault: &std::path::Path,
    config: &std::path::Path,
    period: &str,
) -> serde_json::Value {
    let mut command = bob_command();
    command
        .args(["periods", "show", period, "-f", "json"])
        .env("BOB_DIR", vault)
        .env("BOB_NOW", REVIEWS_NOW)
        .env("BOB_CONFIG_FILE", config)
        .env("NO_COLOR", "1");
    let output = command.output().expect("run bob periods show");
    assert_success(&output);
    serde_json::from_str(&stdout(&output)).expect("show JSON")
}

fn series_keys(report: &serde_json::Value, chart: &str) -> Vec<String> {
    report["charts"][chart]["series"]
        .as_array()
        .expect("series")
        .iter()
        .map(|row| row["key"].as_str().unwrap().to_string())
        .collect()
}

fn sum_minutes(rows: &serde_json::Value) -> i64 {
    rows.as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|row| row["minutes"].as_i64().unwrap_or(0))
        .sum()
}

fn assert_chart_invariant(report: &serde_json::Value) {
    let period_roots = sum_minutes(&report["charts"]["roots"]["series"]);
    let period_types = sum_minutes(&report["charts"]["types"]["series"]);
    assert_eq!(period_roots, period_types, "period series sums");
    for collection in ["columns", "children"] {
        for row in report[collection].as_array().unwrap() {
            if row["state"] == "future" {
                assert_eq!(row["stacks"]["roots"], serde_json::json!([]));
                assert_eq!(row["stacks"]["types"], serde_json::json!([]));
                continue;
            }
            let Some(worked) = row["worked"]["value"].as_i64() else {
                continue;
            };
            let roots = sum_minutes(&row["stacks"]["roots"]);
            let types = sum_minutes(&row["stacks"]["types"]);
            assert_eq!(roots, types, "{collection} {} types", row["id"]);
            assert_eq!(roots, worked, "{collection} {} worked", row["id"]);
        }
    }
    if period_roots > 0 {
        let facts_worked = report["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|fact| fact["metric"] == "worked")
            .and_then(|fact| fact["value"].as_i64());
        if let Some(worked) = facts_worked {
            assert_eq!(period_roots, worked, "charts match the Worked fact");
        }
    }
}

#[test]
fn periods_show_emits_charts_stacks_and_coverage_for_each_kind() {
    let vault = vault_dir("charts-kinds");
    write_charts_vault(vault.path());
    let config = vault.path().join("bob-config.yml");
    write_charts_config(&config, "");

    let week = show_json(vault.path(), &config, "2026-W45");
    assert_eq!(week["ok"], true);
    assert_eq!(week["schema_version"], 1);
    assert_eq!(week["charts"]["version"], 1);
    assert_eq!(week["charts"]["roots"]["limit"], 5);
    assert_eq!(week["charts"]["roots"]["dimension"], "root_parent");
    assert_eq!(week["charts"]["types"]["policy"], "configured");
    assert_eq!(week["columns"].as_array().unwrap().len(), 7);
    assert!(
        week["period"]["coverage"]["display"]
            .as_str()
            .unwrap()
            .contains("of 7 days logged"),
        "closed week with gaps: {}",
        week["period"]["coverage"]["display"]
    );
    let keys = series_keys(&week, "roots");
    assert_eq!(keys.last().map(String::as_str), Some("@unattributed"));
    assert!(keys.contains(&"bob.md".to_string()));
    assert!(keys.contains(&"rest.md".to_string()));
    assert!(keys.contains(&"other.md".to_string()));
    assert!(
        keys.contains(&"@rest".to_string()),
        "N=5 folds cash: {keys:?}"
    );
    let rest = week["charts"]["roots"]["series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "@rest")
        .expect("rest series");
    assert_eq!(rest["label"], "1 more");
    assert_eq!(week["charts"]["types"]["series"][0]["key"], "build");
    assert_eq!(week["charts"]["types"]["series"][0]["label"], "BUILD");
    let type_keys = series_keys(&week, "types");
    assert!(type_keys.contains(&"fix".to_string()));
    assert!(type_keys.contains(&"@unattributed".to_string()));
    assert!(!type_keys.contains(&"sase".to_string()), "SASE is a label");
    assert_eq!(
        week["columns"][0]["worked"]["display_short"],
        week["columns"][0]["worked"]["display"]
    );
    assert!(week["columns"][0]["stack"].is_array());
    assert!(week["lanes"]["root_parent"].is_array());
    assert_chart_invariant(&week);

    let current_week = show_json(vault.path(), &config, "2026-W46");
    assert_eq!(
        current_week["period"]["coverage"]["display"],
        "so far · Mon–Tue"
    );
    assert_eq!(current_week["columns"].as_array().unwrap().len(), 7);
    assert_eq!(current_week["columns"][2]["state"], "future");
    assert_eq!(
        current_week["columns"][2]["stacks"],
        serde_json::json!({ "roots": [], "types": [] })
    );
    assert_chart_invariant(&current_week);

    let quarter = show_json(vault.path(), &config, "2026-Q4");
    assert_eq!(quarter["columns"].as_array().unwrap().len(), 14);
    assert_eq!(quarter["period"]["coverage"]["display"], "so far · W40–W46");
    assert_chart_invariant(&quarter);

    let year = show_json(vault.path(), &config, "2026");
    assert_eq!(year["columns"].as_array().unwrap().len(), 53);
    assert_eq!(year["children"].as_array().unwrap().len(), 4);
    assert_eq!(year["period"]["coverage"]["display"], "so far · W01–W46");
    assert!(year["children"][3]["stacks"]["roots"].is_array());
    assert_chart_invariant(&year);

    let all_time = show_json(vault.path(), &config, "all");
    assert!(!all_time["columns"].as_array().unwrap().is_empty());
    assert!(all_time["period"]["coverage"]["display"]
        .as_str()
        .unwrap()
        .starts_with("since "));
    assert_chart_invariant(&all_time);
}

#[test]
fn periods_show_honors_top_roots_and_warns_on_invalid_values() {
    let vault = vault_dir("charts-top-roots");
    write_charts_vault(vault.path());

    let n1 = vault.path().join("n1.yml");
    write_file(
        &n1,
        "types:\n  BUILD: yellow\n  FIX: red\n  PLAN: auto\nstats:\n  period_strip:\n    top_roots: 1\n",
    );
    let report = show_json(vault.path(), &n1, "2026-W45");
    let keys = series_keys(&report, "roots");
    assert_eq!(keys[0], "bob.md");
    assert_eq!(keys[1], "@rest");
    assert!(report["charts"]["roots"]["series"][1]["label"]
        .as_str()
        .unwrap()
        .ends_with(" more"));
    assert_eq!(report["charts"]["roots"]["limit"], 1);
    assert_chart_invariant(&report);

    let n8 = vault.path().join("n8.yml");
    write_file(
        &n8,
        "types:\n  BUILD: yellow\n  FIX: red\n  PLAN: auto\nstats:\n  period_strip:\n    top_roots: 8\n",
    );
    let report = show_json(vault.path(), &n8, "2026-W45");
    assert_eq!(report["charts"]["roots"]["limit"], 8);
    assert!(!series_keys(&report, "roots").contains(&"@rest".to_string()));
    assert_chart_invariant(&report);

    for (name, body, expected_limit) in [
        ("five", "top_roots: five\n", 5),
        ("zero", "top_roots: 0\n", 1),
        ("twelve", "top_roots: 12\n", 8),
    ] {
        let path = vault.path().join(format!("{name}.yml"));
        write_file(
            &path,
            &format!(
                "types:\n  BUILD: yellow\n  FIX: red\n  PLAN: auto\nstats:\n  period_strip:\n    {body}"
            ),
        );
        let report = show_json(vault.path(), &path, "2026-W45");
        assert_eq!(
            report["charts"]["roots"]["limit"], expected_limit,
            "{name}"
        );
        let warnings = report["warnings"].as_array().expect("warnings");
        assert!(
            warnings.iter().any(|warning| {
                warning["code"] == "period_strip_top_roots_invalid"
                    && warning["message"]
                        .as_str()
                        .unwrap_or("")
                        .contains("stats.period_strip.top_roots")
                    && warning["message"]
                        .as_str()
                        .unwrap_or("")
                        .contains(&path.display().to_string())
            }),
            "{name} warnings: {warnings:?}"
        );
        assert_chart_invariant(&report);
    }
}

#[test]
fn periods_show_charts_hidden_worked_and_unattributed_only() {
    let vault = vault_dir("charts-hidden");
    write_charts_vault(vault.path());
    let config = vault.path().join("hide.yml");
    write_charts_config(&config, "  hide: [worked]\n");
    let report = show_json(vault.path(), &config, "2026-W45");
    let facts = report["facts"].as_array().expect("facts");
    assert!(
        facts.iter().all(|fact| fact["metric"] != "worked"),
        "Worked fact stays hidden: {facts:?}"
    );
    assert!(
        !report["charts"]["roots"]["series"]
            .as_array()
            .unwrap()
            .is_empty(),
        "charts still compute"
    );
    assert_chart_invariant(&report);

    let only = vault_dir("charts-unattributed");
    write_dataview_tasks_settings(only.path());
    write_daily(
        only.path(),
        "2026-11-02",
        "- [x] (**08:00 - 08:30** [t:: 30m])\n",
    );
    let config = only.path().join("bob-config.yml");
    write_charts_config(&config, "");
    let report = show_json(only.path(), &config, "2026-W45");
    assert_eq!(series_keys(&report, "roots"), ["@unattributed"]);
    assert_eq!(report["charts"]["roots"]["series"][0]["label"], "unlinked");
    assert_eq!(series_keys(&report, "types"), ["@unattributed"]);
    assert_eq!(report["charts"]["types"]["series"][0]["label"], "untyped");
    assert_chart_invariant(&report);
}

#[test]
fn periods_show_human_output_is_unchanged_by_charts() {
    let vault = vault_dir("charts-human");
    write_charts_vault(vault.path());
    let config = vault.path().join("bob-config.yml");
    write_charts_config(&config, "");
    let mut command = bob_command();
    command
        .args(["periods", "show", "2026-W45"])
        .env("BOB_DIR", vault.path())
        .env("BOB_NOW", REVIEWS_NOW)
        .env("BOB_CONFIG_FILE", &config)
        .env("NO_COLOR", "1");
    let output = command.output().expect("human show");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("2026-W45"), "{text}");
    assert!(!text.contains("\"charts\""), "{text}");
    assert_stdout_has_no_ansi(&output);
}
