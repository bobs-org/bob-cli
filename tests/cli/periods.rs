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
fn intentions_fields_reach_period_list_and_report_projections() {
    let vault = vault_dir("intentions-fields");
    write_dataview_tasks_settings(vault.path());
    let config = vault.path().join("bob-config.yml");
    write_charts_config(&config, "");

    write_file(
        &vault.path().join("2026/2026-W44.md"),
        "---\nintentions: [\"Close the week\"]\nsummary: \"Shipped [[bob|Bob]]\"\n---\n# W44\n",
    );
    write_file(
        &vault.path().join("2026/2026-W47.md"),
        "---\nintentions:\n  - \"Set the weekly direction\"\nsummary: \"Still in progress\"\n---\n# W47\n",
    );
    write_file(
        &vault.path().join("2026/20261116.md"),
        "---\nintentions: [\"Old plan\"]\n---\n# Monday\n",
    );
    write_file(
        &vault.path().join("2026/20261117.md"),
        "---\nintentions: [\"Old plan\"]\nsummary: \"Finished [[bob|Bob]]\"\n---\n# Tuesday\n",
    );
    write_file(
        &vault.path().join("2026/20261119.md"),
        "---\nintentions: [\"Write [[bob|the contract]]\", \"Ship it\"]\nsummary: \"Today's progress\"\n---\n# Thursday\n",
    );
    write_file(
        &vault.path().join("2026/20261120.md"),
        "---\nintentions: [\"Plan tomorrow\"]\nsummary: \"Future summary\"\n---\n# Friday\n",
    );

    let mut show = bob_command();
    show.args(["periods", "show", "2026-W47", "-f", "json"])
        .env("BOB_DIR", vault.path())
        .env("BOB_NOW", "2026-11-19 12:00:00")
        .env("BOB_CONFIG_FILE", &config)
        .env("NO_COLOR", "1");
    let output = show.output().expect("run intentions period show");
    assert_success(&output);
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("show JSON");
    assert_eq!(
        report["note"]["review"]["intentions"],
        serde_json::json!(["Set the weekly direction"])
    );
    assert_eq!(report["note"]["review"]["summary"], "Still in progress");
    let columns = report["columns"].as_array().unwrap();
    let column = |id: &str| columns.iter().find(|row| row["id"] == id).unwrap();
    assert_eq!(
        column("2026-11-16")["intentions"],
        serde_json::json!(["Old plan"])
    );
    assert!(
        column("2026-11-16")["headline"].is_null(),
        "closed day with no summary must not fall back to its intention"
    );
    assert_eq!(column("2026-11-17")["headline"], "Finished Bob");
    assert_eq!(
        column("2026-11-19")["headline"],
        "Write the contract · Ship it"
    );
    assert_eq!(column("2026-11-20")["headline"], "Plan tomorrow");
    let children = report["children"].as_array().unwrap();
    let child = |id: &str| children.iter().find(|row| row["id"] == id).unwrap();
    assert_eq!(child("2026-11-17")["summary"], "Finished [[bob|Bob]]");
    assert_eq!(
        child("2026-11-19")["intentions"],
        serde_json::json!(["Write the contract", "Ship it"])
    );

    let list_output = list(vault.path(), &["-f", "json"]);
    assert_success(&list_output);
    let list_json: serde_json::Value =
        serde_json::from_str(&stdout(&list_output)).unwrap();
    let week = list_json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "2026-W44")
        .unwrap();
    assert_eq!(week["summary"], "Shipped [[bob|Bob]]");
    let human = list(vault.path(), &[]);
    assert_success(&human);
    assert!(stdout(&human).contains("Summary:"));
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

    for (command, about) in
        [("brief", "agent bundle"), ("draft", "split a period draft")]
    {
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
        adopted.contains("Agent instructions · yearly v3"),
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

#[test]
fn migrate_intentions_moves_old_shapes_and_reports() {
    let temp = TempDir::new("bob-cli-periods-migrate-intentions");
    let vault = temp.path().join("vault");
    write_dataview_tasks_settings(&vault);
    fs::create_dir_all(vault.join("2026")).unwrap();
    fs::create_dir_all(vault.join("2025")).unwrap();
    // Adopted year with Theme + Outcomes (filled).
    fs::write(
        vault.join("2026.md"),
        "---\nparent: \"[[all_time]]\"\ntype: \"[[day]]\"\nid: \"2026\"\ndate: 2025-12-29\nend: 2027-01-03\naliases:\n  - \"2026\"\ntags:\n  - yearly\n---\n\n# 2026\n\n[[2025|‹ 2025]] · [[all_time|↑ All time]] · [[2027|2027 ›]]\n\n[[2026/2026-Q1|Q1]] · [[2026/2026-Q2|Q2]] · [[2026/2026-Q3|Q3]] · [[2026/2026-Q4|Q4]]\n\n```bob-stats-strip\n\n```\n\n## Theme\n\nMake room for deep work\n\n## Outcomes\n\n- Ship the album\n- Keep the notes\n\n## Quarters\n\n```bob-period-parts\n\n```\n\n## Review\n\nOld review.\n",
    )
    .unwrap();
    // Week with three intentions (block links) + reflection + drafted Review.
    // Drafted: Review opens with Headline and carries a matching digest.
    let review_body = "**Headline:** A calm week\n\nDid the work.\n";
    // Compute the old whole-Review digest in-test (same recipe as bob).
    let digest = {
        let normalized = review_body.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<String> = normalized
            .lines()
            .map(|l| l.trim_end().to_owned())
            .collect();
        let canonical = lines.join("\n").trim().to_owned();
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest as _;
        hasher.update(canonical.as_bytes());
        let hex = hex::encode(hasher.finalize());
        format!("sha256:{}", &hex[..16])
    };
    fs::write(
        vault.join("2026/2026-W45.md"),
        format!(
            "---\nparent: \"[[2026/2026-Q4]]\"\ntype: \"[[day]]\"\nid: 2026-W45\ndate: 2026-11-02\nend: 2026-11-08\naliases:\n  - \"2026-W45\"\ntags:\n  - weekly\nreview_digest: {digest}\nreview_drafted: 2026-11-03\n---\n\n# 2026-W45\n\n[[2026/2026-W44|‹ W44]] · [[2026/2026-Q4|↑ 2026-Q4]] · [[2026/2026-W46|W46 ›]]\n\n[[2026/20261102|Mon Nov 2]]\n\n```bob-stats-strip\n\n```\n\n## Intentions\n\n- Ship [[work#^a|the alias]]\n- Keep [[bob#^b]]\n- Third [[areas/work]]\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n\n{review_body}\n\n## Reflection\n\nFirst [[work#^a]]\n\nSecond paragraph.\n"
        ),
    )
    .unwrap();
    // Placeholder week: NOT SET stays, reflection with block link migrates.
    fs::write(
        vault.join("2026/2026-W46.md"),
        "---\nparent: \"[[2026/2026-Q4]]\"\ntype: \"[[day]]\"\nid: 2026-W46\n---\n\n# 2026-W46\n\n## Intentions\n\n- NOT SET\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n\nOld.\n\n## Reflection\n\nThinking about [[work#^r]]\n",
    )
    .unwrap();
    // Nested week: left in place.
    fs::write(
        vault.join("2026/2026-W44.md"),
        "---\nparent: \"[[2026/2026-Q4]]\"\ntype: \"[[day]]\"\nid: 2026-W44\n---\n\n# 2026-W44\n\n## Intentions\n\n- Ship it\n  - nested detail\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n",
    )
    .unwrap();
    // Old-scaffold week whose slots were never filled: the empty headings go.
    fs::write(
        vault.join("2026/2026-W43.md"),
        "---\nparent: \"[[2026/2026-Q4]]\"\ntype: \"[[day]]\"\nid: 2026-W43\n---\n\n# 2026-W43\n\n```bob-stats-strip\n\n```\n\n## Intentions\n\n## Days\n\n```bob-period-parts\n\n```\n\n## Review\n\n## Reflection\n\n> [!agent]- Agent instructions · weekly v2\n> Draft the Review.\n",
    )
    .unwrap();
    // Quarter with outcomes.
    fs::write(
        vault.join("2026/2026-Q4.md"),
        "---\nparent: \"[[2026]]\"\ntype: \"[[day]]\"\nid: 2026-Q4\n---\n\n# 2026-Q4\n\n## Outcomes\n\n- First [[work#^a]]\n- Second thing\n- Third thing\n\n## Weeks\n\n```bob-period-parts\n\n```\n\n## Review\n",
    )
    .unwrap();
    // Daily with Headline + Reflection.
    fs::write(
        vault.join("2026/20261102.md"),
        "---\nparent: '[[2026/202611]]'\n---\n\n[[2026/20261101|prev]]  | [[2026/20261103|next]]\n\n**Headline:** Ship the thing [[bob#^x]]\n\n**Reflection:** Tired but [[work#^y]] helped.\n\n## Pomodoros\n",
    )
    .unwrap();
    // Future daily with empty slot (in range, since range ends Nov 15).
    fs::write(
        vault.join("2026/20261104.md"),
        "---\nparent: '[[2026/202611]]'\n---\n\n**Headline:**\n\n## Pomodoros\n",
    )
    .unwrap();
    // Out-of-range daily: before range_start (2025-12-29), must stay untouched.
    fs::create_dir_all(vault.join("2025")).unwrap();
    fs::write(
        vault.join("2025/20251228.md"),
        "---\nparent: '[[2025/202512]]'\n---\n\n**Headline:** Out of range stays\n",
    )
    .unwrap();
    // Monthly note: never touched.
    fs::write(
        vault.join("2026/202611.md"),
        "---\nparent: [[2026]]\n---\n\n# November\n",
    )
    .unwrap();
    init_git_vault(&vault);
    let monthly_before =
        fs::read_to_string(vault.join("2026/202611.md")).unwrap();
    let ooo_before =
        fs::read_to_string(vault.join("2025/20251228.md")).unwrap();

    let output = migrate(&vault, &["-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("migrate JSON");
    let intentions = &body["intentions"];
    let migrated = intentions["migrated"].as_array().expect("migrated");
    assert!(
        migrated.iter().any(|p| p == "2026/20261102.md"),
        "daily Headline migrates: {body}"
    );
    assert!(
        migrated.iter().any(|p| p == "2026/2026-W45.md"),
        "week intentions migrate: {body}"
    );
    assert!(
        migrated.iter().any(|p| p == "2026/2026-Q4.md"),
        "quarter outcomes migrate: {body}"
    );
    assert!(
        migrated.iter().any(|p| p == "2026.md"),
        "year theme+outcomes migrate: {body}"
    );
    let placeholders =
        intentions["placeholders"].as_array().expect("placeholders");
    assert!(
        placeholders
            .iter()
            .any(|p| p.as_str().unwrap().contains("2026/2026-W46.md")),
        "W46 placeholder reported: {body}"
    );
    let skipped = intentions["skipped"].as_array().expect("skipped");
    assert!(
        skipped
            .iter()
            .any(|p| p.as_str().unwrap().contains("2026/2026-W44.md")),
        "nested week reported: {body}"
    );

    let apply = migrate(&vault, &["-a", "-o", "-f", "json"]);
    assert_success(&apply);
    let applied: serde_json::Value =
        serde_json::from_str(&stdout(&apply)).expect("apply JSON");
    assert!(applied["sha"].as_str().is_some_and(|s| !s.is_empty()));

    let daily = fs::read_to_string(vault.join("2026/20261102.md")).unwrap();
    assert!(daily.contains("intentions:"), "{daily}");
    assert!(!daily.contains("**Headline:**"), "{daily}");
    assert!(!daily.contains("**Reflection:**"), "{daily}");

    let future = fs::read_to_string(vault.join("2026/20261104.md")).unwrap();
    assert!(!future.contains("**Headline:**"), "{future}");

    let week = fs::read_to_string(vault.join("2026/2026-W45.md")).unwrap();
    assert!(week.contains("intentions:"), "{week}");
    assert!(!week.contains("## Intentions"), "{week}");
    assert!(!week.contains("## Reflection"), "{week}");
    assert!(week.contains("summary:"), "{week}");
    assert!(!week.contains("**Headline:**"), "{week}");
    assert!(week.contains("## Review"), "{week}");
    assert!(week.contains("## Days"), "{week}");

    let placeholder =
        fs::read_to_string(vault.join("2026/2026-W46.md")).unwrap();
    assert!(placeholder.contains("## Intentions"), "{placeholder}");
    assert!(placeholder.contains("NOT SET"), "{placeholder}");
    assert!(placeholder.contains("reflection:"), "{placeholder}");
    assert!(!placeholder.contains("## Reflection"), "{placeholder}");

    let nested = fs::read_to_string(vault.join("2026/2026-W44.md")).unwrap();
    assert!(nested.contains("## Intentions"), "{nested}");

    let empty = fs::read_to_string(vault.join("2026/2026-W43.md")).unwrap();
    assert!(!empty.contains("## Intentions"), "{empty}");
    assert!(!empty.contains("## Reflection"), "{empty}");
    assert!(empty.contains("## Days"), "{empty}");
    assert!(empty.contains("## Review"), "{empty}");
    assert!(empty.contains("> [!agent]- Agent instructions"), "{empty}");
    assert!(!empty.contains("intentions:"), "{empty}");

    let quarter = fs::read_to_string(vault.join("2026/2026-Q4.md")).unwrap();
    assert!(quarter.contains("intentions:"), "{quarter}");
    assert!(!quarter.contains("## Outcomes"), "{quarter}");

    let year = fs::read_to_string(vault.join("2026.md")).unwrap();
    assert!(year.contains("intentions:"), "{year}");
    assert!(!year.contains("## Theme"), "{year}");
    assert!(!year.contains("## Outcomes"), "{year}");

    assert_eq!(
        fs::read_to_string(vault.join("2026/202611.md")).unwrap(),
        monthly_before,
        "monthly notes stay untouched"
    );
    assert_eq!(
        fs::read_to_string(vault.join("2025/20251228.md")).unwrap(),
        ooo_before,
        "out-of-range dailies stay untouched"
    );

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
fn list_problems_reports_field_findings_in_human_and_json_without_writes() {
    let vault = vault_dir("field-problems");
    fs::create_dir_all(vault.path().join("2026")).unwrap();
    fs::write(
        vault.path().join("2026/2026-W45.md"),
        "---\nparent: [[2026/2026-Q4]]\nintentions: [one, two, three, four]\n---\n\
         ## Intentions\n\n- legacy item\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/20261103.md"),
        "---\nparent: [[2026/2026-W45]]\nintentions: \"Read [[missing]]\"\n\
         summary: \"[[missing-summary]]\"\n---\n\
         **Headline:** Old daily plan\n\n## Pomodoros\n",
    )
    .unwrap();
    let before = snapshot_tree(vault.path());

    let json_output = bob_command()
        .arg("periods")
        .arg("list")
        .arg("-b")
        .arg(vault.path())
        .arg("-p")
        .arg("-f")
        .arg("json")
        .env("BOB_NOW", MIGRATE_NOW)
        .output()
        .expect("list -p JSON");
    assert_success(&json_output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&json_output)).expect("problems JSON");
    let problems = body["problems"].as_array().expect("problem rows");
    for kind in [
        "dangling_period_link",
        "intentions_shape",
        "intentions_over_soft_cap",
        "legacy_period_text",
    ] {
        assert!(
            problems.iter().any(|problem| problem["kind"] == kind),
            "missing {kind}: {problems:#?}"
        );
    }
    assert!(problems.iter().any(|problem| {
        problem["kind"] == "intentions_over_soft_cap"
            && problem["level"] == "info"
    }));
    assert!(problems.iter().any(|problem| {
        problem["kind"] == "dangling_period_link"
            && problem["detail"]
                .as_str()
                .unwrap()
                .contains("intentions[1]")
    }));
    assert!(problems.iter().any(|problem| {
        problem["kind"] == "legacy_period_text"
            && problem["detail"].as_str().unwrap().contains("conflicts")
            && problem["detail"]
                .as_str()
                .unwrap()
                .contains("frontmatter wins")
    }));

    let human_output = bob_command()
        .arg("periods")
        .arg("list")
        .arg("-b")
        .arg(vault.path())
        .arg("-p")
        .env("BOB_NOW", MIGRATE_NOW)
        .output()
        .expect("list -p human");
    assert_success(&human_output);
    let human = stdout(&human_output);
    for kind in [
        "dangling_period_link",
        "intentions_shape",
        "intentions_over_soft_cap",
        "legacy_period_text",
    ] {
        assert!(human.contains(kind), "missing {kind}:\n{human}");
    }
    assert_eq!(
        snapshot_tree(vault.path()),
        before,
        "report wrote to the vault"
    );
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
            "4. report prose notes that need formatting (write nothing)",
            "5. report period-note problems (write nothing)",
            "6. repair generated area catalogs",
            "7. sync project lifecycles and sub-project catalogs",
            "8. repair generated notebook catalogs",
            "9. repair generated album records indexes",
            "10. remove empty record `## Notes` sections",
            "11. run vault-sync against the vault",
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
    assert_eq!(body["brief_version"], "brief v3");
    assert_eq!(body["schema_version"], 3);
    assert_eq!(body["id"], "2026-W44");
    assert_eq!(body["header"]["state"], "closed");
    assert_eq!(body["header"]["review_state"], "empty");
    assert_eq!(body["header"]["summary_state"], "empty");
    assert!(
        body["header"]["stop_advice"]
            .as_str()
            .unwrap_or_default()
            .contains("ready to draft"),
        "stop advice: {}",
        body["header"]["stop_advice"]
    );
    // Instructions ride first, in both formats, with the same substance.
    assert_eq!(body["instructions"]["version"], "weekly v3");
    assert!(
        body["instructions"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("at most 250 words"),
        "v3 instruction substance: {}",
        body["instructions"]
    );
    // v3 neighbour shape: no legacy spellings.
    let neighbours = &body["neighbours"];
    assert!(neighbours.get("own").is_none(), "{neighbours}");
    assert!(neighbours.get("parent_outcomes").is_none(), "{neighbours}");
    assert!(neighbours.get("parent_theme").is_none(), "{neighbours}");
    assert!(neighbours.get("prev_reflection").is_none(), "{neighbours}");
    assert!(neighbours.get("prev_review").is_none(), "{neighbours}");
    assert!(neighbours.get("intentions").is_some(), "{neighbours}");
    assert!(
        neighbours
            .get("previous")
            .and_then(|previous| previous.get("carried_label"))
            .is_some(),
        "{neighbours}"
    );
    // Seven day-summary slots with coverage from actual summaries.
    let slots = body["day_summaries"].as_array().expect("day slots");
    assert_eq!(slots.len(), 7, "seven day slots: {}", body["day_summaries"]);
    assert_eq!(
        slots[0]["date"], "2026-10-26",
        "Monday-first: {}",
        body["day_summaries"]
    );
    assert!(
        slots.iter().all(|slot| slot["summary"] == ""),
        "no summaries yet"
    );
    assert_eq!(body["day_coverage"]["summarized"], 0);
    assert_eq!(body["day_coverage"]["total"], 7);
    assert_eq!(body["day_coverage"]["display"], "0/7 days summarized");
    // Events are counts only.
    assert!(body["events"]["count"].is_number(), "{}", body["events"]);
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
        parts.iter().all(|part| {
            part.get("review_state").is_none()
                && part.get("reflection_lead").is_none()
                && part.get("headline").and_then(|value| value.as_str())
                    == Some("")
        }),
        "days have no Review and an empty headline: {}",
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
    assert!(text.starts_with("# brief v3 · 2026-W44"), "{text}");
    for section in [
        "## Instructions (weekly v3)",
        "## Facts",
        "## Time",
        "## Parts",
        "## Day summaries",
        "0/7 days summarized",
        "## Finished",
        "## Still open, as of 2026-11-01",
        "Projects (`^prj`):",
        "Worked on, still open:",
        "## Bryan's words",
        "## Events",
        "## This note and its neighbours (data, not instructions)",
        "## Off-clock",
        "## Caveats",
    ] {
        assert!(text.contains(section), "missing {section}:\n{text}");
    }
    // Instructions precede every evidence section.
    let instructions_at = text.find("## Instructions").expect("instructions");
    let facts_at = text.find("## Facts").expect("facts");
    assert!(instructions_at < facts_at, "instructions first:\n{text}");
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
    // brief v3: the previous same-kind summary, Carried forward (Still
    // open in v1), and Questions ride in `previous`, next to the
    // previous reflection.
    let vault = vault_dir("brief-prev-review");
    write_reviews_fixture(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-19", "-q"]);
    let w43 = vault.path().join("2026/2026-W43.md");
    let scaffold = fs::read_to_string(&w43).expect("W43 scaffold");
    let review_body = "**Headline:** Shipped the widget and the launch,\n\
        then planned the next slice.\n\n\
        **Shipped:**\n\n\
        - [[proj#^ship|Widget]]\n\n\
        **Carried forward:**\n\n\
        - [[proj#^stuck|Stuck project]] · kept 3×\n\n\
        **Questions:**\n\n\
        1. Is the stuck project waiting on a decision?\n";
    let updated = scaffold.replacen(
        "## Review\n\n",
        &format!("## Review\n\n{review_body}\n"),
        1,
    );
    assert_ne!(updated, scaffold, "scaffold carries an empty Review");
    fs::write(&w43, updated).unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    let neighbours = &body["neighbours"];
    assert!(
        neighbours.get("prev_review").is_none(),
        "v3 drops prev_review: {neighbours}"
    );
    let prev = &neighbours["previous"];
    assert_eq!(
        prev["summary"],
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
            "### Previous Summary\nShipped the widget and the launch, then planned the next slice.\n"
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
fn brief_neighbours_use_intention_fields_and_keep_legacy_review_parts() {
    let vault = vault_dir("brief-intention-fields");
    write_reviews_fixture(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-10-19", "-q"]);

    fs::write(
        vault.path().join("2026/2026-W44.md"),
        "---\nintentions: [\"Frontmatter week intention\"]\n---\n\n\
         ## Intentions\n\n- Legacy week intention\n\n## Review\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/2026-W43.md"),
        "---\nsummary: \"Frontmatter previous summary\"\n\
         reflection: \"Frontmatter previous reflection.\"\n---\n\n\
         ## Review\n\n**Headline:** Legacy review headline.\n\n\
         **Carried forward:**\n\n- Keep this detail.\n\n## Reflection\n\nLegacy reflection.\n",
    )
    .unwrap();
    fs::write(
        vault.path().join("2026/2026-Q4.md"),
        "---\nintentions: [\"Frontmatter parent intention\"]\n---\n\n\
         ## Theme\n\nLegacy theme.\n\n## Outcomes\n\n- Legacy outcome.\n",
    )
    .unwrap();

    let body = brief_json(vault.path(), "2026-W44");
    let neighbours = &body["neighbours"];
    assert_eq!(neighbours["intentions"], "- Frontmatter week intention");
    assert_eq!(neighbours["summary"], "");
    assert_eq!(
        neighbours["parent_intentions"],
        "- Frontmatter parent intention"
    );
    assert_eq!(
        neighbours["previous"]["summary"],
        "Frontmatter previous summary"
    );
    assert_eq!(neighbours["previous"]["carried"], "- Keep this detail.");
    assert_eq!(
        neighbours["previous"]["reflection"],
        "Frontmatter previous reflection."
    );
    // Legacy shapes stay readable: the legacy Review Headline projects
    // into the previous summary, and legacy text survives beside
    // frontmatter.
    assert!(neighbours.get("own").is_none(), "{neighbours}");
    assert!(neighbours.get("prev_review").is_none(), "{neighbours}");
    assert!(neighbours.get("parent_outcomes").is_none(), "{neighbours}");
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
        "**Summary:** Relative week\n\nShipped it.\n",
    );
    let output =
        draft(vault.path(), "last-week", &input, &["-F", "-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("draft JSON");
    assert_eq!(body["id"], "2026-W45");
    assert_eq!(body["summary_state"], "drafted");
    let written =
        fs::read_to_string(vault.path().join("2026/2026-W45.md")).unwrap();
    assert!(
        written.contains("summary: Relative week"),
        "draft last-week writes the summary to the canonical note"
    );
    assert!(
        written.contains("## Review\n\nShipped it."),
        "and the Review body:\n{written}"
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
        "**Summary:** Shipped the widget slice and the big launch, and\n\
         closed the first weekly review.\n\n**Shipped:**\n\n- [[2026/20261026#^ship|Widget]]\n",
    );
    let output = draft(vault.path(), "2026-W44", &input, &["-f", "json"]);
    assert_success(&output);
    let after = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("drafted note");
    assert!(
        after.contains(
            "## Review\n\n**Shipped:**\n\n- [[2026/20261026#^ship|Widget]]\n"
        ),
        "remaining text becomes only the Review body:\n{after}"
    );
    assert!(
        !after.contains("**Headline:**"),
        "no duplicate opener:\n{after}"
    );
    assert!(
        after.contains("summary: Shipped the widget slice and the big launch, and closed the first weekly review."),
        "summary split into frontmatter:\n{after}"
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

    // The 25-word cap counts the whole opener paragraph.
    let long = write_review(
        vault.path(),
        "long.md",
        "**Summary:** one two three four five six seven eight nine ten\n\
         eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen\n\
         nineteen twenty twenty-one twenty-two twenty-three twenty-four twenty-five twenty-six\n",
    );
    let output = draft(vault.path(), "2026-W44", &long, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error json");
    assert_eq!(body["error"]["code"], "over_cap");
    assert_eq!(body["error"]["count"], 26);
    // The one-release Headline alias still reads and joins.
    let alias = write_review(
        vault.path(),
        "alias.md",
        "**Headline:** Shipped the widget slice,\nand the launch.\n\nBody words.\n",
    );
    let output = draft(vault.path(), "2026-W44", &alias, &["-F", "-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("alias json");
    assert_eq!(body["summary_state"], "drafted");
}

#[test]
fn draft_accepts_a_valid_text_and_a_second_draft() {
    let vault = vault_dir("draft-accept");
    write_reviews_fixture(vault.path());
    let input = write_review(
        vault.path(),
        "review.md",
        "**Summary:** A steady build week\n\nShipped the widget slice.\n",
    );
    let before = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("week note");
    let output = draft(vault.path(), "2026-W44", &input, &[]);
    assert_success(&output);
    assert!(
        stdout(&output)
            .starts_with("Drafted 2026-W44 Review · 9 words · state drafted"),
        "success line: {}",
        stdout(&output)
    );
    let after = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("drafted note");
    assert!(
        after.contains("## Review\n\nShipped the widget slice.\n"),
        "remaining text becomes only the Review body:\n{after}"
    );
    assert!(
        after.contains("summary: A steady build week"),
        "summary split into frontmatter:\n{after}"
    );
    assert!(
        after.contains("review_drafted: \"2026-11-10\""),
        "draft day stamped:\n{after}"
    );
    assert!(
        after.contains("summary_drafted: \"2026-11-10\""),
        "summary day stamped:\n{after}"
    );
    let digest = after
        .lines()
        .find(|line| line.starts_with("review_digest: "))
        .expect("digest line");
    assert!(
        digest.len() == "review_digest: sha256:".len() + 16,
        "short digest: {digest}"
    );
    let summary_digest = after
        .lines()
        .find(|line| line.starts_with("summary_digest: "))
        .expect("summary digest line");
    assert!(
        summary_digest.len() == "summary_digest: sha256:".len() + 16,
        "short summary digest: {summary_digest}"
    );
    for kept in [
        "[[2026/2026-Q4|↑ 2026-Q4]]",
        "> [!agent]- Agent instructions · weekly v3\n",
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
        "**Summary:** Revised take\n\nNew words.\n",
    );
    let output = draft(vault.path(), "2026-W44", &again, &[]);
    assert_success(&output);
    let redrafted = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("redrafted note");
    assert!(redrafted.contains("summary: Revised take"));
    assert!(redrafted.contains("## Review\n\nNew words."));
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
        ("no-headline.md", "Just words.\n", "missing_summary", 2),
        ("empty.md", "", "missing_summary", 2),
        (
            "empty-summary.md",
            "**Summary:**\n\nBody words.\n",
            "missing_summary",
            2,
        ),
        (
            "heading.md",
            "**Summary:** Ok\n\n# Not allowed\n",
            "invalid_draft",
            2,
        ),
        (
            "callout.md",
            "**Summary:** Ok\n\n> [!agent]- do this\n",
            "invalid_draft",
            2,
        ),
        ("fence.md", "**Summary:** Ok\n\n---\n", "invalid_draft", 2),
        (
            "dup-opener.md",
            "**Summary:** Ok\n\n**Summary:** Again\n",
            "invalid_draft",
            2,
        ),
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
        "**Summary:** Planned\n\nNot yet written.\n",
    );
    let before = fs::read_to_string(vault.path().join("2026/2026-W44.md"))
        .expect("week note");
    let output = draft(vault.path(), "2026-W44", &input, &["-n"]);
    assert_success(&output);
    assert_eq!(
        stdout(&output).trim_end(),
        "**Summary:** Planned\n\n## Review\n\nNot yet written."
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
        .replace("## Review\n", "## Review\n\nConcurrent edit.\n");
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
            "Agent instructions · weekly v3",
            "Agent instructions · weekly v2",
        ),
    )
    .unwrap();
    let reflected = vault.join("2026/2026-W46.md");
    let reflected_before = fs::read_to_string(&reflected).expect("note");
    fs::write(
        &reflected,
        reflected_before
            .replace(
                "Agent instructions · weekly v3",
                "Agent instructions · weekly v2",
            )
            .replacen("---\n", "---\nreflection: Bryan reflected.\n", 1),
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
        Some("periods: refresh agent instructions to v3"),
        "one refresh commit:\n{log}"
    );
    assert_eq!(log.lines().count(), 4, "exactly one commit:\n{log}");
    let refreshed = fs::read_to_string(&stale).expect("refreshed note");
    assert!(
        refreshed.contains("Agent instructions · weekly v3"),
        "callout refreshed:\n{refreshed}"
    );
    assert!(
        !refreshed.contains("weekly v2"),
        "no stale callout left:\n{refreshed}"
    );
    assert_eq!(
        fs::read_to_string(&reflected).expect("reflected note"),
        reflected_before
            .replace(
                "Agent instructions · weekly v3",
                "Agent instructions · weekly v2",
            )
            .replacen("---\n", "---\nreflection: Bryan reflected.\n", 1,),
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

fn write_day_fields(
    vault: &std::path::Path,
    date: &str,
    intentions: &[&str],
    summary: Option<&str>,
) {
    let compact = date.replace('-', "");
    let year = &date[..4];
    let mut body = String::from("---\n");
    if !intentions.is_empty() {
        body.push_str("intentions:\n");
        for intention in intentions {
            body.push_str("  - ");
            body.push_str(&serde_json::to_string(intention).unwrap());
            body.push('\n');
        }
    }
    if let Some(summary) = summary {
        body.push_str("summary: ");
        body.push_str(&serde_json::to_string(summary).unwrap());
        body.push('\n');
    }
    body.push_str(&format!("---\n# {date}\n\n## Pomodoros\n"));
    write_file(&vault.join(format!("{year}/{compact}.md")), &body);
}

#[test]
fn week_report_and_brief_carry_day_intentions_and_summaries() {
    let vault = vault_dir("day-headlines");
    write_dataview_tasks_settings(vault.path());
    let config = vault.path().join("bob-config.yml");
    write_charts_config(&config, "");
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-11-10", "-q"]);
    // 2026-W46: Mon 9 – Sun 15 Nov. REVIEWS_NOW is Tue 10.
    write_day_fields(
        vault.path(),
        "2026-11-09",
        &[],
        Some("Capture merge-in arrow shipped"),
    );
    write_day_fields(
        vault.path(),
        "2026-11-10",
        &["Daily headline designed and planned"],
        None,
    );
    write_day_fields(
        vault.path(),
        "2026-11-11",
        &["Converge take-home submitted"],
        None,
    );
    write_day_fields(vault.path(), "2026-11-12", &[], None);

    let week = show_json(vault.path(), &config, "2026-W46");
    let by_id = |rows: &serde_json::Value, id: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("missing {id} in {rows}"))
    };
    for collection in ["columns", "children"] {
        let rows = &week[collection];
        assert_eq!(
            by_id(rows, "2026-11-09")["headline"],
            "Capture merge-in arrow shipped",
            "{collection}"
        );
        assert_eq!(
            by_id(rows, "2026-11-10")["headline"],
            "Daily headline designed and planned",
            "{collection}"
        );
        assert_eq!(
            by_id(rows, "2026-11-11")["headline"],
            "Converge take-home submitted",
            "{collection}"
        );
        assert!(
            by_id(rows, "2026-11-12")["headline"].is_null(),
            "empty slot is null in {collection}"
        );
        assert!(
            by_id(rows, "2026-11-13")["headline"].is_null(),
            "missing note is null in {collection}"
        );
    }

    let quarter = show_json(vault.path(), &config, "2026-Q4");
    assert!(
        quarter["children"]
            .as_array()
            .unwrap()
            .iter()
            .all(|child| child["headline"].is_null()),
        "quarter children have no summary or intentions headline: {}",
        quarter["children"]
    );

    let mut command = bob_command();
    command
        .args(["periods", "show", "2026-W46"])
        .env("BOB_DIR", vault.path())
        .env("BOB_NOW", REVIEWS_NOW)
        .env("BOB_CONFIG_FILE", &config)
        .env("NO_COLOR", "1");
    let human = command.output().expect("human show");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("Capture merge-in arrow shipped"),
        "past day prefers headline:\n{text}"
    );
    assert!(
        text.contains("Daily headline designed and planned"),
        "today prefers headline:\n{text}"
    );
    assert!(
        text.contains("Converge take-home submitted"),
        "future day appends headline:\n{text}"
    );

    let brief = brief_json(vault.path(), "2026-W46");
    let part = |id: &str| {
        brief["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|part| part["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("missing part {id}"))
    };
    assert_eq!(
        part("2026-11-09")["headline"],
        "Capture merge-in arrow shipped"
    );
    assert_eq!(
        part("2026-11-10")["headline"],
        "Daily headline designed and planned"
    );
    assert_eq!(
        part("2026-11-11")["headline"],
        "Converge take-home submitted"
    );
    assert_eq!(part("2026-11-12")["headline"], "");
    assert_eq!(part("2026-11-13")["headline"], "");
    assert!(part("2026-11-09").get("review_state").is_none());

    let markdown = periods_command(vault.path(), "brief", &["2026-W46"]);
    assert_success(&markdown);
    let md = stdout(&markdown);
    assert!(
        md.contains("Bryan's headline: “Capture merge-in arrow shipped”"),
        "past day brief:\n{md}"
    );
    assert!(
        md.contains("Bryan's headline: “Daily headline designed and planned”"),
        "today brief:\n{md}"
    );
    assert!(
        md.contains("(not started)")
            && md.contains("Bryan's headline: “Converge take-home submitted”"),
        "future day brief:\n{md}"
    );
}

fn draft_stdin(
    vault: &std::path::Path,
    subject: &str,
    text: &str,
    extra: &[&str],
) -> std::process::Output {
    use std::io::Write;
    let mut command = bob_command();
    command
        .arg("periods")
        .arg("draft")
        .arg("-b")
        .arg(vault)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", REVIEWS_NOW)
        .env("NO_COLOR", "1");
    command.arg(subject).arg("-i").arg("-");
    for arg in extra {
        command.arg(arg);
    }
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("spawn bob periods draft");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(text.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("draft stdin")
}

#[test]
fn brief_day_reports_evidence_and_instructions_first() {
    let vault = vault_dir("brief-day");
    write_reviews_fixture(vault.path());
    // Day intentions verbatim, plus one event whose title stays private.
    fs::write(
        vault.path().join("2026/20261026.md"),
        "---\nparent: '[[2026/2026-W44]]'\nintentions:\n  - \"Ship the widget\"\n---\n\n\
         [[2026/20261025|prev]] · [[2026/2026-W44|W44]] · [[2026/20261027|next]]\n\n\
         - [x] #task Ship widget [created:: 2026-10-20] [completion:: 2026-10-26] ^ship\n\
         - [x] #task Big launch [created:: 2026-10-18] [completion:: 2026-10-26] ^prj\n\n\
         ## Pomodoros\n\n\
         - [x] (**08:00 - 08:25** [t:: 25m]) — PLAN\n\
         \t- [[#^ship]]\n\n\
         ## Events\n\n\
         - [ ] #task **0800-0830** Dentist ^dentist\n",
    )
    .unwrap();
    // Previous calendar day's summary.
    fs::write(
        vault.path().join("2026/20261025.md"),
        "---\nparent: '[[2026/2026-W44]]'\nsummary: Shipped the prior slice\n---\n\nprev day\n",
    )
    .unwrap();
    // Week intentions project into the day brief as the parent.
    let w44 = vault.path().join("2026/2026-W44.md");
    let scaffold = fs::read_to_string(&w44).expect("W44 scaffold");
    fs::write(
        &w44,
        scaffold.replacen(
            "\n---\n\n# 2026-W44",
            "\nintentions: [\"Finish the widget slice\"]\n---\n\n# 2026-W44",
            1,
        ),
    )
    .unwrap();

    let body = brief_json(vault.path(), "2026-10-26");
    assert_eq!(body["ok"], true);
    assert_eq!(body["brief_version"], "brief v3");
    assert_eq!(body["schema_version"], 3);
    assert_eq!(body["id"], "2026-10-26");
    assert_eq!(body["kind"], "day");
    assert_eq!(body["header"]["state"], "closed");
    assert_eq!(body["header"]["note_path"], "2026/20261026.md");
    assert_eq!(body["header"]["prev_path"], "2026/20261025.md");
    assert_eq!(body["header"]["parent_path"], "2026/2026-W44.md");
    assert!(
        body["header"]["stop_advice"]
            .as_str()
            .unwrap_or_default()
            .contains("ready to draft"),
        "day stop advice: {}",
        body["header"]
    );
    // Day v1 instructions carry the canonical date and the day rules.
    assert_eq!(body["instructions"]["version"], "day v1");
    let text = body["instructions"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("2026-10-26"), "canonical date: {text}");
    assert!(text.contains("at most 60 words"), "day cap: {text}");
    // Intentions verbatim plus the week's, explicitly data.
    assert_eq!(body["neighbours"]["intentions"], "- Ship the widget");
    assert_eq!(
        body["neighbours"]["parent_intentions"],
        "- Finish the widget slice"
    );
    // Day facts come from the Stats machinery for that day.
    let facts: Vec<(String, String)> = body["facts"]["facts"]
        .as_array()
        .expect("facts")
        .iter()
        .map(|fact| {
            (
                fact["label"].as_str().unwrap_or_default().to_string(),
                fact["display"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        facts
            .iter()
            .any(|(label, display)| label == "Worked" && display == "25m"),
        "day Worked: {facts:?}"
    );
    assert!(
        facts
            .iter()
            .any(|(label, display)| label == "Pomodoros" && display == "1"),
        "day Pomodoros: {facts:?}"
    );
    assert!(
        facts
            .iter()
            .any(|(label, display)| label == "Done" && display == "2"),
        "day Done: {facts:?}"
    );
    // Time by root parent with correctly split linked minutes.
    let roots = body["time"]["roots"].as_array().expect("roots");
    assert!(
        roots
            .iter()
            .any(|root| root["tasks"].as_array().is_some_and(|tasks| {
                tasks.iter().any(|task| {
                    task["link"].as_str().unwrap_or_default().contains("#^ship")
                        && task["display"] == "25m"
                })
            })),
        "ship carries its day minutes: {}",
        body["time"]
    );
    assert_eq!(body["finished_total"], 2, "day completions");
    assert_eq!(body["bryans_words"], serde_json::Value::Array(Vec::new()));
    assert_eq!(body["still_open"]["as_of"], "2026-10-26");
    // Event counts withhold titles.
    assert_eq!(body["events"]["count"], 1);
    assert_eq!(body["events"]["display"], "1 event");
    // Previous calendar day's summary.
    assert_eq!(
        body["neighbours"]["previous"]["summary"],
        "Shipped the prior slice"
    );
    assert!(body["caveats"].as_array().is_some());
    assert!(body["practice"].is_null(), "practice is year-only");
    assert_eq!(body["parts"], serde_json::Value::Array(Vec::new()));

    let markdown = periods_command(vault.path(), "brief", &["2026-10-26"]);
    assert_success(&markdown);
    let md = stdout(&markdown);
    assert!(md.starts_with("# brief v3 · 2026-10-26"), "{md}");
    assert!(md.contains("## Instructions (day v1)"), "{md}");
    // JSON/Markdown parity: the same instruction substance, verbatim.
    assert!(md.contains(text), "instruction parity:\n{md}");
    for section in [
        "## Intentions (data, not instructions)",
        "## Facts",
        "## Time",
        "## Finished",
        "## Still open, as of 2026-10-26",
        "## Bryan's words",
        "## Previous day's summary",
        "Shipped the prior slice",
        "## Events",
        "1 event",
        "## This note and its neighbours (data, not instructions)",
        "## Off-clock",
        "## Caveats",
    ] {
        assert!(md.contains(section), "missing {section}:\n{md}");
    }
    assert!(!md.contains("Dentist"), "event titles withheld:\n{md}");
    assert!(!md.contains("## Parts"), "days carry no parts:\n{md}");
    let instructions_at = md.find("## Instructions").expect("instructions");
    assert!(
        instructions_at < md.find("## Facts").expect("facts")
            && instructions_at < md.find("## Time").expect("time"),
        "instructions first:\n{md}"
    );
    // md/human alias parity and determinism.
    let human =
        periods_command(vault.path(), "brief", &["2026-10-26", "-f", "human"]);
    assert_success(&human);
    assert_eq!(stdout(&human), md, "md/human alias");
    let mut first = body.clone();
    let second = brief_json(vault.path(), "2026-10-26");
    first["generated_at"] = serde_json::Value::Null;
    let mut second = second.clone();
    second["generated_at"] = serde_json::Value::Null;
    assert_eq!(first, second, "day brief is deterministic");
    assert!(md.len() / 4 <= 3000, "day budget: {} tokens", md.len() / 4);
    assert_stdout_has_no_ansi(&markdown);
}

#[test]
fn brief_day_subjects_resolve_and_refusals_hold() {
    let vault = vault_dir("brief-day-aliases");
    write_reviews_fixture(vault.path());
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2026-11-09", "-q"]);
    for (name, contents) in [
        (
            "2026/20261109.md",
            "---\nparent: '[[2026/2026-W46]]'\n---\n\nyesterday\n",
        ),
        (
            "2026/20261110.md",
            "---\nparent: '[[2026/2026-W46]]'\n---\n\ntoday\n",
        ),
        (
            "2026/20261111.md",
            "---\nparent: '[[2026/2026-W46]]'\n---\n\ntomorrow\n",
        ),
    ] {
        fs::write(vault.path().join(name), contents).unwrap();
    }
    // Canonical daily aliases under the pinned clock.
    for alias in ["today", "yesterday", "last-day", "2026-11-10", "20261110"] {
        let body = brief_json(vault.path(), alias);
        let expected = if alias == "yesterday" || alias == "last-day" {
            "2026-11-09"
        } else {
            "2026-11-10"
        };
        assert_eq!(body["id"], expected, "{alias}");
        assert_eq!(body["kind"], "day");
    }
    let by_path = brief_json(vault.path(), "2026/20261109.md");
    assert_eq!(by_path["id"], "2026-11-09");
    // Open and future states with stop advice.
    assert_eq!(brief_json(vault.path(), "today")["header"]["state"], "open");
    assert!(brief_json(vault.path(), "today")["header"]["stop_advice"]
        .as_str()
        .unwrap_or_default()
        .contains("so far"));
    assert_eq!(
        brief_json(vault.path(), "2026-11-11")["header"]["state"],
        "future"
    );
    // all_time refusal and bad ids.
    for (subject, code) in [
        ("all_time", "no_review_section"),
        ("2026-02-30", "invalid_period"),
    ] {
        let output =
            periods_command(vault.path(), "brief", &[subject, "-f", "json"]);
        assert_eq!(output.status.code(), Some(2), "{subject}");
        let body: serde_json::Value =
            serde_json::from_str(&stdout(&output)).expect("error JSON");
        assert_eq!(body["error"]["code"], code, "{subject}");
        assert_eq!(body["schema_version"], 1, "error envelope");
    }
    // A missing daily note names the Obsidian recovery hint.
    let missing =
        periods_command(vault.path(), "brief", &["2026-10-25", "-f", "json"]);
    assert_eq!(missing.status.code(), Some(2));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&missing)).expect("missing JSON");
    assert_eq!(body["error"]["code"], "missing_note");
    assert!(
        body["error"]["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("Obsidian"),
        "recovery hint: {}",
        body["error"]
    );
    // The brief is read-only.
    let before =
        fs::read_to_string(vault.path().join("2026/20261109.md")).unwrap();
    let _ = brief_json(vault.path(), "2026-11-09");
    assert_eq!(
        fs::read_to_string(vault.path().join("2026/20261109.md")).unwrap(),
        before,
        "brief writes nothing"
    );
}

#[test]
fn draft_day_summaries_enforce_boundaries() {
    let vault = vault_dir("draft-day");
    write_reviews_fixture(vault.path());
    // A valid day draft stores one line with stamps; stdin works too.
    let input = write_review(
        vault.path(),
        "summary.md",
        "**Summary:** Shipped the widget and closed the review queue.\n",
    );
    let output = draft(vault.path(), "2026-10-26", &input, &["-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("draft JSON");
    assert_eq!(body["id"], "2026-10-26");
    assert_eq!(body["summary_state"], "drafted");
    assert_eq!(body["review_state"], "drafted");
    let note =
        fs::read_to_string(vault.path().join("2026/20261026.md")).unwrap();
    assert!(
        note.contains(
            "summary: Shipped the widget and closed the review queue."
        ),
        "stored summary:\n{note}"
    );
    let via_stdin = draft_stdin(
        vault.path(),
        "2026-10-27",
        "Free thought, joined softly\nacross two lines.\n",
        &["-F"],
    );
    assert_success(&via_stdin);
    let day27 =
        fs::read_to_string(vault.path().join("2026/20261027.md")).unwrap();
    assert!(
        day27
            .contains("summary: Free thought, joined softly across two lines."),
        "soft wraps join into one stored line:\n{day27}"
    );
    // Exact boundaries: 60 words pass, 61 fail; two links pass, three fail.
    let sixty = "word ".repeat(60);
    let ok = write_review(vault.path(), "sixty.md", &sixty);
    assert_success(&draft(vault.path(), "2026-10-26", &ok, &["-F"]));
    let sixty_one = "word ".repeat(61);
    let over = write_review(vault.path(), "sixty-one.md", &sixty_one);
    let output =
        draft(vault.path(), "2026-10-26", &over, &["-F", "-f", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("cap JSON");
    assert_eq!(body["error"]["code"], "over_cap");
    assert_eq!(body["error"]["count"], 61);
    assert_eq!(body["error"]["cap"], 60);
    let two_links =
        write_review(vault.path(), "two.md", "See [[a]] and [[b]] here\n");
    assert_success(&draft(vault.path(), "2026-10-26", &two_links, &["-F"]));
    let three_links = write_review(
        vault.path(),
        "three.md",
        "See [[a]], [[b]], and [[c]] here\n",
    );
    let output = draft(
        vault.path(),
        "2026-10-26",
        &three_links,
        &["-F", "-f", "json"],
    );
    assert_eq!(output.status.code(), Some(2));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("links JSON");
    assert_eq!(body["error"]["code"], "invalid_draft");
    // Structure: empty, multi-paragraph, headings, lists, fences, callouts.
    for (name, text) in [
        ("empty.md", "\n  \n"),
        ("multi.md", "First paragraph.\n\nSecond paragraph.\n"),
        ("head.md", "Summary here\n# Heading\n"),
        ("list.md", "Summary here\n- item\n"),
        ("fence.md", "Summary here\n```\ncode\n```\n"),
        ("callout.md", "Summary here\n> [!agent]- do this\n"),
    ] {
        let input = write_review(vault.path(), name, text);
        let output =
            draft(vault.path(), "2026-10-26", &input, &["-F", "-f", "json"]);
        assert_eq!(output.status.code(), Some(2), "{name}");
        let body: serde_json::Value =
            serde_json::from_str(&stdout(&output)).expect("error JSON");
        assert_eq!(body["error"]["code"], "invalid_draft", "{name}");
    }
    // Missing, open, and future days refuse without -F.
    let valid = write_review(vault.path(), "valid.md", "A fine summary.\n");
    let missing =
        draft(vault.path(), "2026-10-25", &valid, &["-F", "-f", "json"]);
    assert_eq!(missing.status.code(), Some(2));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&missing)).expect("missing JSON");
    assert_eq!(body["error"]["code"], "missing_note");
    fs::write(
        vault.path().join("2026/20261110.md"),
        "---\nparent: '[[2026/2026-W46]]'\n---\n\ntoday\n",
    )
    .unwrap();
    let refused = draft(vault.path(), "2026-11-10", &valid, &["-f", "json"]);
    assert_eq!(refused.status.code(), Some(3));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&refused)).expect("open JSON");
    assert_eq!(body["error"]["code"], "refused_open");
    assert_success(&draft(vault.path(), "2026-11-10", &valid, &["-F"]));
}

#[test]
fn draft_eligibility_matrix_guards_both_kinds() {
    let vault = vault_dir("draft-eligibility");
    write_reviews_fixture(vault.path());
    let summary = |text: &str| {
        write_review(vault.path(), "s.md", &format!("**Summary:** {text}\n"))
    };
    // Day: a reflection never blocks a first draft; the reflection is
    // preserved while the summary state becomes drafted. The overall
    // state remains reflected.
    let day = vault.path().join("2026/20261026.md");
    let with_reflection = fs::read_to_string(&day).unwrap().replacen(
        "---\n\n[[2026/20261025|prev]]",
        "reflection: It meant a lot.\n---\n\n[[2026/20261025|prev]]",
        1,
    );
    fs::write(&day, with_reflection).unwrap();
    let planned = draft(
        vault.path(),
        "2026-10-26",
        &summary("Did the work."),
        &["-n", "-f", "json"],
    );
    assert_success(&planned);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&planned)).expect("dry JSON");
    assert_eq!(body["review_state"], "reflected");
    assert_eq!(body["summary_state"], "drafted");
    assert!(
        body["planned"]
            .as_str()
            .unwrap_or_default()
            .contains("Did the work."),
        "dry run shows the summary: {}",
        body
    );
    assert_success(&draft(
        vault.path(),
        "2026-10-26",
        &summary("Did the work."),
        &[],
    ));
    let note = fs::read_to_string(&day).unwrap();
    assert!(
        note.contains("reflection: It meant a lot."),
        "preserved:\n{note}"
    );
    assert!(note.contains("summary: Did the work."), "stamped:\n{note}");
    // Day: a reflection beside an existing summary refuses re-draft.
    let output = draft(
        vault.path(),
        "2026-10-26",
        &summary("Redo it."),
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(3));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("refused JSON");
    assert_eq!(body["error"]["code"], "refused_state");
    assert_success(&draft(
        vault.path(),
        "2026-10-26",
        &summary("Redo it."),
        &["-F"],
    ));
    // Day: an edited summary (digest mismatch) refuses without -F.
    let edited = fs::read_to_string(&day)
        .unwrap()
        .replace("summary: Redo it.", "summary: Bryan rewrote it.");
    fs::write(&day, edited).unwrap();
    let output = draft(
        vault.path(),
        "2026-10-26",
        &summary("Once more."),
        &["-f", "json"],
    );
    assert_eq!(output.status.code(), Some(3));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("edited JSON");
    assert_eq!(body["error"]["code"], "refused_state");

    // Period: an edited Review body refuses a first summary even when
    // the summary key is absent.
    let week = vault.path().join("2026/2026-W44.md");
    let edited_review = fs::read_to_string(&week).unwrap().replacen(
        "## Review\n\n",
        "## Review\n\nBryan wrote this.\n\n",
        1,
    );
    fs::write(&week, edited_review).unwrap();
    let first =
        write_review(vault.path(), "first.md", "**Summary:** New summary\n");
    let output = draft(vault.path(), "2026-W44", &first, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(3), "protect Bryan's Review");
    // Period: a legacy Headline Review stays protected without -F.
    let legacy = write_review(
        vault.path(),
        "legacy.md",
        "**Headline:** Legacy headline\n\nLegacy body.\n",
    );
    let legacy_note = "---\nparent: \"[[2026/2026-Q4]]\"\n---\n\n# 2026-W45\n\n## Review\n\n**Headline:** Legacy headline\n\nLegacy body.\n\n## Reflection\n";
    fs::write(vault.path().join("2026/2026-W45.md"), legacy_note).unwrap();
    let output = draft(vault.path(), "2026-W45", &legacy, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(3), "legacy protection");
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("legacy JSON");
    assert_eq!(body["error"]["code"], "refused_state");
    assert_success(&draft(vault.path(), "2026-W45", &legacy, &["-F"]));
    // -F never overrides invalid text.
    let over = write_review(
        vault.path(),
        "over.md",
        &format!("**Summary:** {}\n", "word ".repeat(60)),
    );
    let output = draft(vault.path(), "2026-W45", &over, &["-F", "-f", "json"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn draft_period_splitting_caps_and_reflected_result() {
    let vault = vault_dir("draft-splitting");
    write_reviews_fixture(vault.path());
    // Wrapped opener joins onto one line; exactly 25 words pass.
    let words25: Vec<String> = (1..=25).map(|n| format!("w{n}")).collect();
    let wrapped = write_review(
        vault.path(),
        "wrapped.md",
        &format!(
            "**Summary:** {}\n{}\n\nBody here.\n",
            words25[..12].join(" "),
            words25[12..].join(" ")
        ),
    );
    let output = draft(vault.path(), "2026-W44", &wrapped, &["-f", "json"]);
    assert_success(&output);
    let note =
        fs::read_to_string(vault.path().join("2026/2026-W44.md")).unwrap();
    assert!(
        note.contains(&format!("summary: {}", words25.join(" "))),
        "joined opener:\n{note}"
    );
    assert!(
        note.contains("## Review\n\nBody here."),
        "split body:\n{note}"
    );
    // 26 summary words refuse.
    let words26 = format!(
        "**Summary:** {}\n",
        (1..=26)
            .map(|n| format!("w{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let input = write_review(vault.path(), "s26.md", &words26);
    let output = draft(vault.path(), "2026-W44", &input, &["-F", "-f", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("cap JSON");
    assert_eq!(body["error"]["code"], "over_cap");
    assert_eq!(body["error"]["cap"], 25);
    // Total 250/251 boundary over the submitted summary-plus-body.
    let filler = format!("**Summary:** Ok\n\n{}", "word ".repeat(248));
    let input = write_review(vault.path(), "b250.md", &filler);
    assert_success(&draft(vault.path(), "2026-W44", &input, &["-F"]));
    let filler = format!("**Summary:** Ok\n\n{}", "word ".repeat(249));
    let input = write_review(vault.path(), "b251.md", &filler);
    let output = draft(vault.path(), "2026-W44", &input, &["-F", "-f", "json"]);
    assert_eq!(output.status.code(), Some(2));
    // Summary-only draft leaves an empty Review body with no Review stamps.
    let only = write_review(
        vault.path(),
        "only.md",
        "**Summary:** Just the summary\n",
    );
    assert_success(&draft(vault.path(), "2026-W44", &only, &["-F"]));
    let note =
        fs::read_to_string(vault.path().join("2026/2026-W44.md")).unwrap();
    assert!(note.contains("summary: Just the summary"), ":\n{note}");
    assert!(
        !note.contains("review_digest:"),
        "no Review stamps:\n{note}"
    );
    // A first draft after reflection remains reflected with a drafted summary.
    let reflected = fs::read_to_string(vault.path().join("2026/2026-Q4.md"))
        .unwrap()
        .replacen("---\n", "---\nreflection: Bryan reflected.\n", 1);
    fs::write(vault.path().join("2026/2026-Q4.md"), reflected).unwrap();
    let quarter = write_review(
        vault.path(),
        "q.md",
        "**Summary:** Quarter summary\n\nQuarter body.\n",
    );
    let output =
        draft(vault.path(), "2026-Q4", &quarter, &["-F", "-f", "json"]);
    assert_success(&output);
    let body: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("quarter JSON");
    assert_eq!(body["review_state"], "reflected");
    assert_eq!(body["summary_state"], "drafted");
}

#[test]
fn draft_preserves_bytes_and_fails_closed() {
    let vault = vault_dir("draft-preserve");
    write_reviews_fixture(vault.path());
    let note = vault.path().join("2026/2026-W44.md");
    // BOM, CRLF, comments, unrelated keys, intentions, reflection, callout.
    let contents = "\u{feff}---\r\nparent: \"[[2026/2026-Q4]]\" # keep me\r\ncustom: value\r\nintentions:\r\n  - Do the thing\r\n---\r\n\r\n# 2026-W44\r\n\r\n## Review\r\n\r\nOld body.\r\n\r\n## Reflection\r\n\r\nBryan reflected.\r\n\r\n> [!agent]- Agent instructions · weekly v3\r\n"
        .to_string();
    fs::write(&note, contents).unwrap();
    let input = write_review(
        vault.path(),
        "review.md",
        "**Summary:** New summary\n\nNew body.\n",
    );
    let output = draft(vault.path(), "2026-W44", &input, &["-F", "-f", "json"]);
    assert_success(&output);
    let after = fs::read_to_string(&note).unwrap();
    assert!(after.starts_with("\u{feff}---\r\n"), "BOM kept:\n{after:?}");
    assert!(
        after.contains("parent: \"[[2026/2026-Q4]]\" # keep me\r\n"),
        "comment kept"
    );
    assert!(after.contains("custom: value\r\n"), "unrelated key kept");
    assert!(after.contains("  - Do the thing\r\n"), "intentions kept");
    assert!(after.contains("Bryan reflected.\r\n"), "reflection kept");
    assert!(
        after.contains("> [!agent]- Agent instructions · weekly v3\r\n"),
        "callout kept"
    );
    assert!(
        after.contains("## Review\r\n\r\nNew body.\r\n"),
        "body replaced"
    );
    assert!(after.contains("summary: New summary"), "summary stamped");
    // Malformed and duplicate YAML fail closed, even with -F.
    for (name, contents, code) in [
        (
            "malformed.md",
            "---\nintentions: [\n---\n\n## Review\n",
            "frontmatter_invalid",
        ),
        (
            "duplicate.md",
            "---\nsummary: one\nsummary: two\n---\n\n## Review\n",
            "duplicate_key",
        ),
    ] {
        let target = vault.path().join("2026/2026-W44.md");
        fs::write(&target, contents).unwrap();
        let input =
            write_review(vault.path(), name, "**Summary:** Try\n\nBody.\n");
        let output =
            draft(vault.path(), "2026-W44", &input, &["-F", "-f", "json"]);
        assert_eq!(output.status.code(), Some(1), "{name}");
        let body: serde_json::Value =
            serde_json::from_str(&stdout(&output)).expect("error JSON");
        assert_eq!(body["error"]["code"], code, "{name}: {}", body["error"]);
    }
}

#[test]
fn brief_week_coverage_counts_actual_summaries() {
    let vault = vault_dir("brief-coverage");
    write_reviews_fixture(vault.path());
    // Partial coverage: two of seven days carry nonempty summaries.
    // One stamped summary via draft (drafted), one bare summary (edited).
    let stamped =
        write_review(vault.path(), "stamped.md", "Shipped the widget.\n");
    assert_success(&draft(vault.path(), "2026-10-26", &stamped, &["-F"]));
    let bare = vault.path().join("2026/20261028.md");
    fs::write(
        &bare,
        "---\nparent: '[[2026/2026-W44]]'\nsummary: \"Planned the launch.\"\n---\n\nnote\n",
    )
    .unwrap();
    let body = brief_json(vault.path(), "2026-W44");
    assert_eq!(body["day_coverage"]["summarized"], 2);
    assert_eq!(body["day_coverage"]["display"], "2/7 days summarized");
    let slots = body["day_summaries"].as_array().expect("slots");
    assert_eq!(slots.len(), 7);
    assert_eq!(slots[0]["summary"], "Shipped the widget.");
    assert_eq!(slots[0]["summary_state"], "drafted");
    assert_eq!(slots[2]["summary"], "Planned the launch.");
    assert_eq!(slots[2]["summary_state"], "edited");
    assert_eq!(slots[1]["summary"], "");
    assert_eq!(slots[1]["summary_state"], "empty");
    assert_eq!(slots[1]["path"], "2026/20261027.md");
    // Quarter parts read actual week summaries, never intentions.
    let week = vault.path().join("2026/2026-W44.md");
    let scaffold = fs::read_to_string(&week).unwrap();
    fs::write(
        &week,
        scaffold.replacen(
            "\n---\n\n# 2026-W44",
            "\nsummary: \"Week summary wins\"\nsummary_digest: \"sha256:0000000000000000\"\n---\n\n# 2026-W44",
            1,
        ),
    )
    .unwrap();
    let quarter = brief_json(vault.path(), "2026-Q4");
    let parts = quarter["parts"].as_array().expect("parts");
    let w44 = parts
        .iter()
        .find(|part| part["id"] == "2026-W44")
        .expect("W44 part");
    assert_eq!(w44["headline"], "Week summary wins");
    // ISO boundary: W53 spans the year boundary with per-day slots.
    let _ =
        periods_command(vault.path(), "ensure", &["-d", "2020-12-28", "-q"]);
    for date in [
        "20201228", "20201229", "20201230", "20201231", "20210101", "20210102",
        "20210103",
    ] {
        let path = vault.path().join(format!("2020/{date}.md"));
        if date.starts_with("2021") {
            let path = vault.path().join(format!("2021/{date}.md"));
            if !path.exists() {
                let year = "2021";
                fs::create_dir_all(vault.path().join(year)).unwrap();
                fs::write(
                    &path,
                    "---\nparent: '[[2020/2020-W53]]'\n---\n\nnote\n",
                )
                .unwrap();
            }
            continue;
        }
        if !path.exists() {
            fs::create_dir_all(vault.path().join("2020")).unwrap();
            fs::write(&path, "---\nparent: '[[2020/2020-W53]]'\n---\n\nnote\n")
                .unwrap();
        }
    }
    let boundary = brief_json(vault.path(), "2020-W53");
    assert_eq!(
        boundary["day_summaries"].as_array().expect("slots").len(),
        7
    );
    assert_eq!(boundary["day_summaries"][0]["date"], "2020-12-28");
    assert_eq!(boundary["day_summaries"][6]["date"], "2021-01-03");
}

#[test]
fn brief_heavy_day_stays_in_budget_without_private_titles() {
    let vault = vault_dir("brief-heavy-day");
    write_reviews_fixture(vault.path());
    // Many completions force honest omission totals behind the day cap.
    let mut tasks = String::new();
    for n in 1..=150 {
        tasks.push_str(&format!(
            "- [x] #task Heavy job {n:03} with substantial scope and follow-through [created:: 2026-10-20] [completion:: 2026-10-26] ^heavy{n:03}\n"
        ));
    }
    tasks.push_str("- [x] #task Daily review #gtd [created:: 2026-10-26] [completion:: 2026-10-26] ^gtd\n");
    fs::write(
        vault.path().join("2026/20261026.md"),
        format!(
            "---\nparent: '[[2026/2026-W44]]'\n---\n\ntasks\n\n{tasks}\n## Pomodoros\n\n\
             - [x] (**08:00 - 08:40** [t:: 40m]) — PLAN\n\
             \t- [[2026/20261026#^heavy001]]\n\
             \t- [[proj#^openheavy]]\n\
             - [x] (**09:00 - 09:20** [off:: 20m]) — REST\n\
             - [x] (**10:00 - 10:30** [t:: 30m]) — PLAN\n\n\
             ## Events\n\n\
             - [ ] #task **0800-0830** QRS Secret Sync Title ^evt1\n\
             - [ ] #task **0900-0930** Zebra Private Planning ^evt2\n"
        ),
    )
    .unwrap();
    fs::write(
        vault.path().join("proj.md"),
        "---\nparent: \"[[org]]\"\n---\n\n\
         - [x] #task Old job [created:: 2026-10-18] [completion:: 2026-10-26] ^oldjob\n\
         - [x] #task #gtd Water plants [repeat:: every day] [created:: 2026-10-18] [completion:: 2026-10-26] ^water\n\
         - [ ] #task Open heavy [created:: 2026-10-20] ^openheavy\n\
         \t- 🛠️ **WORK LOG**\n\
         \t\t- *2026-10-26 14:03* — heavy day words\n\
         \t\t- *2026-10-27 14:03* — adjacent day words\n",
    )
    .unwrap();
    let body = brief_json(vault.path(), "2026-10-26");
    assert_eq!(body["finished_total"], 151, "151 counted completions");
    assert!(
        body["finished_routine_omitted"].as_u64().unwrap_or(0) >= 2,
        "recurring and daily-review rows omitted: {}",
        body["finished_routine_omitted"]
    );
    let more = body["finished_more"].as_u64().unwrap_or(0);
    assert!(more > 0, "omission totals: {}", body["finished_more"]);
    assert_eq!(body["events"]["count"], 2);
    assert!(
        body["facts"]["facts"].as_array().is_some_and(|facts| facts
            .iter()
            .any(
                |fact| fact["label"] == "Off-clock" && fact["display"] != "0m"
            )),
        "off-clock counts: {}",
        body["facts"]
    );
    assert!(
        body["still_open"]["worked_open"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "historical open work: {}",
        body["still_open"]
    );
    let markdown = periods_command(vault.path(), "brief", &["2026-10-26"]);
    assert_success(&markdown);
    let md = stdout(&markdown);
    assert!(!md.contains("QRS Secret Sync Title"), "privacy:\n{md}");
    assert!(!md.contains("Zebra Private Planning"), "privacy:\n{md}");
    assert!(
        md.contains(&format!("- +{more} more")),
        "honest more counts:\n{md}"
    );
    assert!(md.contains("heavy day words"), "same-day words:\n{md}");
    assert!(
        md.len() / 4 <= 3000,
        "heavy day budget with instructions: {} tokens",
        md.len() / 4
    );
    assert_eq!(
        body["instructions"]["version"], "day v1",
        "budget includes instructions"
    );
}
