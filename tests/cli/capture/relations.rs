//! Capture `~` / `~~` execute vectors LX1–LX16
//! (`plan:202610/task_relations.md` capture-execute).

use crate::support::*;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-10-14 10:20:00";
const DAY: &str = "## Pomodoros\n- [ ] Unrelated placeholder\n";

fn setup(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, DAY);
    (temp, vault, day)
}

fn capture_json(vault: &Path, day: &Path, draft: &str) -> serde_json::Value {
    capture_json_dry_run_matches_real(vault, day, NOW, &[draft])
}

fn capture_json_env(
    vault: &Path,
    day: &Path,
    draft: &str,
    extra_env: &[(&str, &str)],
) -> serde_json::Value {
    let run = |dry_run: bool| {
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
        command.arg("--").arg(draft);
        command.env("BOB_DAY_FILE", day).env("BOB_NOW", NOW);
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let output = command.output().expect("run capture");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };
    let dry: serde_json::Value = run(true);
    assert_eq!(dry["dry_run"], true);
    let real: serde_json::Value = run(false);
    assert_eq!(real["dry_run"], false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    if let Some(serde_json::Value::Array(items)) = masked.get_mut("captures") {
        for item in items {
            item["dry_run"] = serde_json::Value::Bool(false);
        }
    }
    assert_eq!(
        masked, real,
        "dry-run JSON must equal real-run JSON except for dry_run"
    );
    real
}

fn capture_fail(vault: &Path, day: &Path, draft: &str) -> std::process::Output {
    bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture")
}

fn capture_human(
    vault: &Path,
    day: &Path,
    draft: &str,
) -> std::process::Output {
    bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture human")
}

fn read(vault: &Path, relative: &str) -> String {
    fs::read_to_string(vault.join(relative)).expect("read note")
}

fn snapshot(vault: &Path, relatives: &[&str]) -> Vec<(String, String)> {
    relatives
        .iter()
        .map(|relative| ((*relative).to_string(), read(vault, relative)))
        .collect()
}

fn assert_unchanged(vault: &Path, before: &[(String, String)]) {
    for (relative, contents) in before {
        assert_eq!(
            read(vault, relative),
            *contents,
            "{relative} must stay byte-identical"
        );
    }
}

fn assert_day_untouched(day: &Path) {
    assert_eq!(
        fs::read_to_string(day).expect("day"),
        DAY,
        "day file must stay untouched"
    );
}

fn assert_no_fresh_or_lane_change(text: &str, previous: &str) {
    let strip_id = |line: &str| {
        line.split(" ^")
            .next()
            .unwrap_or(line)
            .trim_end()
            .to_string()
    };
    let previous_task = previous
        .lines()
        .find(|line| line.contains("#task"))
        .map(strip_id);
    let current_task = text
        .lines()
        .find(|line| line.contains("#task"))
        .map(strip_id);
    assert_eq!(
        current_task, previous_task,
        "relation must not change status, lane, or freshness:\n{text}"
    );
}

// LX1 + LX12: new N ~ archived T. Mirror skipped; N is not minted.
#[test]
fn lx1_new_task_to_archived_skips_mirror_without_mint() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx1");
    write_file(
        &vault.join("done/bob_done.md"),
        "- [x] #task Capture * blocks existing tasks ^capture-star\n",
    );

    let json = capture_json(
        &vault,
        &day,
        r#"Fix retries ~bob_done:capture-star|"follow-up of""#,
    );
    assert_eq!(json["kind"], "task");
    assert_eq!(json["relation_updates"][0]["direction"], "to");
    assert_eq!(json["relation_updates"][0]["action"], "added");
    assert_eq!(json["relation_updates"][0]["label"], "follow-up of");
    assert_eq!(json["relation_updates"][0]["target"]["archived"], true);
    assert_eq!(json["mirror_skipped"][0]["reason"], "archived_target");
    assert_eq!(json["mirror_skipped"][0]["block_id"], "capture-star");
    assert!(json.get("minted_relation_block_ids").is_none(), "{json}");
    assert!(json.get("linked_from_line_updates").is_none(), "{json}");

    let inbox = read(&vault, "mac_inbox.md");
    assert_eq!(
        inbox,
        concat!(
            "- [ ] #task Fix retries [created::2026-10-14]\n",
            "\t- 🪢 **LINKS:** follow-up of [[done/bob_done#^capture-star]]\n",
        ),
        "N exact bytes:\n{inbox}"
    );
    assert!(
        !inbox.contains(" ^"),
        "archived target must not mint N:\n{inbox}"
    );
    assert_eq!(
        read(&vault, "done/bob_done.md"),
        "- [x] #task Capture * blocks existing tasks ^capture-star\n",
        "archived T is not written"
    );
    assert_day_untouched(&day);

    let human = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", &day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("--dry-run")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg(r#"Fix retries ~bob_done:capture-star|"follow-up of""#)
        .output()
        .expect("dry-run human");
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("🪢 would link to") && text.contains("archived"),
        "{text}"
    );
}

// LX2 + LX12: ~~ writes T's LINKS, mints N, and births N with LINKED FROM.
#[test]
fn lx2_from_relation_mints_new_task_and_writes_both_sides() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx2");
    write_file(
        &vault.join("sase_blog.md"),
        "- [ ] #task Ship the sase blog ^prj\n",
    );

    let json = capture_json(
        &vault,
        &day,
        r#"Write outline ~~sase_blog:prj|"next step""#,
    );
    assert_eq!(json["kind"], "task");
    let minted = json["minted_relation_block_ids"][0]["block_id"]
        .as_str()
        .expect("minted target id");
    assert_eq!(json["minted_relation_block_ids"][0]["role"], "target");
    assert_eq!(json["minted_relation_block_ids"][0]["path"], "mac_inbox.md");
    assert_eq!(json["relation_updates"][0]["direction"], "from");
    assert_eq!(json["relation_updates"][0]["action"], "added");
    assert_eq!(json["relation_updates"][0]["label"], "next step");
    assert_eq!(json["linked_from_line_updates"][0]["path"], "mac_inbox.md");
    assert!(json.get("mirror_skipped").is_none(), "{json}");

    let inbox = read(&vault, "mac_inbox.md");
    assert_eq!(
        inbox,
        format!(
            "- [ ] #task Write outline [created::2026-10-14] ^{minted}\n\t- 🪢 **LINKED FROM:** [[sase_blog#^prj]] (next step)\n"
        ),
        "N exact bytes:\n{inbox}"
    );
    assert_eq!(
        read(&vault, "sase_blog.md"),
        format!(
            "- [ ] #task Ship the sase blog ^prj\n\t- 🪢 **LINKS:** next step [[mac_inbox#^{minted}]]\n"
        ),
        "T LINKS"
    );
    assert_day_untouched(&day);

    let roles: Vec<Vec<String>> = json["task_blocks"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|block| {
            block["roles"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .filter_map(|role| role.as_str().map(str::to_string))
                .collect()
        })
        .collect();
    assert!(
        roles
            .iter()
            .any(|row| row.iter().any(|role| role == "relation_source")),
        "existing T whose LINKS changed is a relation_source:\n{json}"
    );

    let human = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", &day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("--dry-run")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg(r#"Write outline ~~sase_blog:prj|"next step""#)
        .output()
        .expect("dry-run human");
    assert_success(&human);
    assert!(
        stdout(&human).contains("would now link here"),
        "{}",
        stdout(&human)
    );
}

// LX3: existing owner ~ T; rerun unchanged; new label relabeled.
#[test]
fn lx3_existing_owner_relabel_and_unchanged_rerun() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx3");
    write_file(
        &vault.join("bob.md"),
        "- [ ] #task Fix retries ^fix-retries\n",
    );
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Retry design ^retry-design\n",
    );

    let first =
        capture_json(&vault, &day, "@bob+fix-retries ~sase:retry-design|see");
    assert_eq!(first["kind"], "task_dependency");
    assert_eq!(first["relation_updates"][0]["action"], "added");
    assert_eq!(first["relation_updates"][0]["label"], "see");
    assert_eq!(first["linked_from_line_updates"][0]["path"], "sase.md");
    let bob = read(&vault, "bob.md");
    assert!(
        bob.contains("- 🪢 **LINKS:** see [[sase#^retry-design]]\n"),
        "{bob}"
    );
    let sase = read(&vault, "sase.md");
    assert!(
        sase.contains("- 🪢 **LINKED FROM:** [[bob#^fix-retries]] (see)\n"),
        "{sase}"
    );

    let rerun =
        capture_json(&vault, &day, "@bob+fix-retries ~sase:retry-design|see");
    assert_eq!(rerun["relation_updates"][0]["action"], "unchanged");
    assert_eq!(read(&vault, "bob.md"), bob);
    assert_eq!(read(&vault, "sase.md"), sase);

    let relabel =
        capture_json(&vault, &day, "@bob+fix-retries ~sase:retry-design|next");
    assert_eq!(relabel["relation_updates"][0]["action"], "relabeled");
    assert_eq!(relabel["relation_updates"][0]["previous_label"], "see");
    assert_eq!(relabel["relation_updates"][0]["label"], "next");
    assert!(
        read(&vault, "bob.md")
            .contains("- 🪢 **LINKS:** next [[sase#^retry-design]]\n"),
        "{}",
        read(&vault, "bob.md")
    );
    assert!(
        read(&vault, "sase.md")
            .contains("- 🪢 **LINKED FROM:** [[bob#^fix-retries]] (next)\n"),
        "{}",
        read(&vault, "sase.md")
    );
    assert_day_untouched(&day);

    let human =
        capture_human(&vault, &day, "@bob+fix-retries ~sase:retry-design|next");
    assert_success(&human);
    assert!(
        stdout(&human).contains("🪢 unchanged"),
        "{}",
        stdout(&human)
    );
}

// LX4 + LX13: `&` still blocks; `~` changes no status, freshness, or Pomodoro.
#[test]
fn lx4_ampersand_blocks_and_relation_stays_inert() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx4");
    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Order parts [fresh:: 2026-10-01] ^parts\n",
            "- [*] #task Do cleanup [fresh:: 2026-10-01] ^cleanup\n",
        ),
    );
    let cleanup_before = "- [*] #task Do cleanup [fresh:: 2026-10-01] ^cleanup";

    let json =
        capture_json(&vault, &day, "Ask plumber &job:parts ~job:cleanup");
    assert_eq!(json["kind"], "task");
    assert_eq!(json["dependency_update"]["role"], "dependent");
    assert_eq!(json["relation_updates"][0]["direction"], "to");
    assert_eq!(json["relation_updates"][0]["target"]["block_id"], "cleanup");

    let job = read(&vault, "job.md");
    assert!(
        job.contains("- [ ] #task Order parts [fresh:: 2026-10-01]"),
        "& still writes parts as the prerequisite:\n{job}"
    );
    assert!(
        job.contains("- 🧱 **BLOCKS:**"),
        "& still writes the BLOCKS mirror:\n{job}"
    );
    assert!(
        job.contains(cleanup_before),
        "cleanup status/freshness unchanged:\n{job}"
    );
    assert!(
        job.contains("- 🪢 **LINKED FROM:**"),
        "cleanup gained a mirror:\n{job}"
    );
    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        inbox.contains("- ⛓️ **DEPENDS ON:** [[job#^parts]]"),
        "{inbox}"
    );
    assert!(
        inbox.contains("- 🪢 **LINKS:** [[job#^cleanup]]"),
        "{inbox}"
    );
    assert!(!inbox.contains("[fresh::]"), "{inbox}");
    assert_day_untouched(&day);
}

// LX5: T in today's daily note gets the same-batch mirror; N is minted.
#[test]
fn lx5_todays_daily_target_gets_mirror_and_mints_source() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx5");
    write_file(
        &vault.join("2026/20261014.md"),
        "- [ ] #task Daily chore ^gtd\n",
    );

    let json = capture_json(&vault, &day, "Plan walk ~2026/20261014:gtd");
    assert_eq!(json["relation_updates"][0]["action"], "added");
    let minted = json["minted_relation_block_ids"][0]["block_id"]
        .as_str()
        .expect("source mint");
    assert_eq!(json["minted_relation_block_ids"][0]["role"], "source");
    assert_eq!(
        json["linked_from_line_updates"][0]["path"],
        "2026/20261014.md"
    );
    assert!(json.get("mirror_skipped").is_none(), "{json}");

    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        inbox.contains(&format!("^{minted}\n")),
        "N minted:\n{inbox}"
    );
    assert!(
        inbox.contains("- 🪢 **LINKS:** [[2026/20261014#^gtd]]")
            || inbox.contains("- 🪢 **LINKS:** [[20261014#^gtd]]"),
        "{inbox}"
    );
    let daily = read(&vault, "2026/20261014.md");
    assert!(
        daily.contains(&format!("[[mac_inbox#^{minted}]]")),
        "{daily}"
    );
    assert_day_untouched(&day);
}

// LX6: kill switch off skips the mirror; `~` does not mint; `~~` still mints N.
#[test]
fn lx6_kill_switch_skips_mirror_and_source_mint() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx6");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");
    let config = vault.join("bob-config.yml");
    write_file(&config, "relations:\n  linked_from_lines: false\n");
    let env = [("BOB_CONFIG_FILE", config.to_str().unwrap())];

    let to = capture_json_env(&vault, &day, "Fix it ~work:foo", &env);
    assert_eq!(to["mirror_skipped"][0]["reason"], "disabled");
    assert!(to.get("minted_relation_block_ids").is_none(), "{to}");
    assert!(to.get("linked_from_line_updates").is_none(), "{to}");
    let inbox = read(&vault, "mac_inbox.md");
    assert!(inbox.contains("- 🪢 **LINKS:** [[work#^foo]]"), "{inbox}");
    assert!(
        !inbox.contains(" ^"),
        "kill switch must not mint N for ~:\n{inbox}"
    );
    assert_eq!(
        read(&vault, "work.md"),
        "- [ ] #task Design ^foo\n",
        "T untouched"
    );

    fs::remove_file(vault.join("mac_inbox.md")).ok();
    let from = capture_json_env(&vault, &day, "Write outline ~~work:foo", &env);
    assert_eq!(from["mirror_skipped"][0]["reason"], "disabled");
    let minted = from["minted_relation_block_ids"][0]["block_id"]
        .as_str()
        .expect("~~ still mints N");
    assert_eq!(from["minted_relation_block_ids"][0]["role"], "target");
    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        inbox.contains(&format!("^{minted}")),
        "~~ still mints N:\n{inbox}"
    );
    assert!(
        !inbox.contains("LINKED FROM"),
        "kill switch skips N's mirror:\n{inbox}"
    );
    assert!(
        read(&vault, "work.md")
            .contains(&format!("- 🪢 **LINKS:** [[mac_inbox#^{minted}]]")),
        "{}",
        read(&vault, "work.md")
    );
}

// LX7: ~~ target in a daily note, done/, _templates, or fenced code is refused.
#[test]
fn lx7_from_target_path_refusals_leave_vault_untouched() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx7");
    write_file(
        &vault.join("job.md"),
        concat!(
            "- [ ] #task Cleanup ^cleanup\n",
            "```md\n",
            "- [ ] #task Fenced example ^fenced\n",
            "```\n",
        ),
    );
    write_file(&vault.join("2026/20261014.md"), "- [ ] #task Daily ^gtd\n");
    write_file(&vault.join("done/old.md"), "- [ ] #task Archived ^old\n");
    write_file(
        &vault.join("_templates/t.md"),
        "- [ ] #task Template ^tmpl\n",
    );
    let before = snapshot(
        &vault,
        &[
            "job.md",
            "2026/20261014.md",
            "done/old.md",
            "_templates/t.md",
            "day.md",
        ],
    );

    let cases: &[(&str, &str)] = &[
        ("Ask Kelly ~~2026/20261014:gtd", "daily note"),
        ("Ask Kelly ~~done/old:old", "done/"),
        ("Ask Kelly ~~_templates/t:tmpl", "_templates"),
        ("Ask Kelly ~~job:fenced", "fenced"),
    ];
    for (draft, needle) in cases {
        let output = capture_fail(&vault, &day, draft);
        assert!(
            !output.status.success(),
            "{draft} should refuse:\n{}",
            format_output(&output)
        );
        let message = format!("{}{}", stdout(&output), stderr(&output));
        assert!(
            message
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
                || message.contains(needle),
            "{draft} ({needle}): {message}"
        );
        assert_unchanged(&vault, &before);
        assert!(!vault.join("mac_inbox.md").exists());
    }
}

// LX8: self, non-task, and missing block id refuse with a teaching error.
#[test]
fn lx8_self_non_task_and_missing_id_refuse() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx8");
    write_file(
        &vault.join("job.md"),
        concat!("- [ ] #task Cleanup ^cleanup\n", "- not a task ^plain\n",),
    );
    let before = snapshot(&vault, &["job.md", "day.md"]);

    let cases: &[(&str, &str)] = &[
        ("@job+cleanup ~job:cleanup", "itself"),
        ("Ask Kelly ~job:plain", "not a task"),
        ("Ask Kelly ~job:missing", "no task"),
    ];
    for (draft, needle) in cases {
        let output = capture_fail(&vault, &day, draft);
        assert!(
            !output.status.success(),
            "{draft} should refuse:\n{}",
            format_output(&output)
        );
        let message = format!("{}{}", stdout(&output), stderr(&output));
        assert!(
            message
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
                || message.contains(needle),
            "{draft} ({needle}): {message}"
        );
        assert_unchanged(&vault, &before);
    }
}

// LX9: malformed LINKS on S refuses; vault byte-identical.
#[test]
fn lx9_malformed_links_refuses() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx9");
    write_file(
        &vault.join("bob.md"),
        concat!(
            "- [ ] #task Fix retries ^fix-retries\n",
            "\t- 🪢 **LINKS:** [[#^a]] until Friday\n",
        ),
    );
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Retry design ^retry-design\n",
    );
    let before = snapshot(&vault, &["bob.md", "sase.md", "day.md"]);

    let output =
        capture_fail(&vault, &day, "@bob+fix-retries ~sase:retry-design");
    assert!(!output.status.success(), "{}", format_output(&output));
    let message = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        message.to_ascii_lowercase().contains("malformed"),
        "{message}"
    );
    assert_unchanged(&vault, &before);
}

// LX10: malformed LINKED FROM on T succeeds; mirror skipped with a warning.
#[test]
fn lx10_malformed_linked_from_skips_mirror() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx10");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");
    write_file(
        &vault.join("sase.md"),
        concat!(
            "- [ ] #task Retry design ^retry-design\n",
            "\t- 🪢 **LINKED FROM:** [[#^b]] extra\n",
        ),
    );

    let json = capture_json(&vault, &day, "Fix it ~sase:retry-design");
    assert_eq!(json["relation_updates"][0]["action"], "added");
    assert_eq!(json["mirror_skipped"][0]["reason"], "malformed_line");
    assert!(json.get("linked_from_line_updates").is_none(), "{json}");
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings.iter().any(|warning| {
            warning
                .as_str()
                .is_some_and(|text| text.contains("malformed LINKED FROM"))
        }),
        "{json}"
    );
    assert_eq!(
        read(&vault, "sase.md"),
        concat!(
            "- [ ] #task Retry design ^retry-design\n",
            "\t- 🪢 **LINKED FROM:** [[#^b]] extra\n",
        )
    );
    assert!(
        read(&vault, "mac_inbox.md")
            .contains("- 🪢 **LINKS:** [[sase#^retry-design]]"),
        "{}",
        read(&vault, "mac_inbox.md")
    );
}

// LX11: a relation after `x` is an operator_relation_error.
#[test]
fn lx11_operator_relation_error() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx11");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    let output = capture_fail(&vault, &day, "x ~job:cleanup");
    assert!(!output.status.success(), "{}", format_output(&output));
    let message = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        message.contains("task relationships need a task owner")
            || message.contains("Pomodoro operator"),
        "{message}"
    );
    assert_day_untouched(&day);
}

// LX13: a relation capture leaves freshness, lane, status, and the day file.
#[test]
fn lx13_relation_is_inert() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx13");
    write_file(
        &vault.join("work.md"),
        "- [*] #task Design [fresh:: 2026-10-01] ^foo\n",
    );
    let before = read(&vault, "work.md");
    let _ = capture_json(&vault, &day, "Fix it ~work:foo");
    let after = read(&vault, "work.md");
    assert!(after.contains("- [*] #task Design [fresh:: 2026-10-01] ^foo"));
    assert_no_fresh_or_lane_change(&after, &before);
    assert_day_untouched(&day);
}

// LX14: duplicate same-label is a no-op; different labels refuse before write.
#[test]
fn lx14_duplicate_noop_and_conflicting_labels_refuse() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx14");
    write_file(
        &vault.join("job.md"),
        concat!("- [ ] #task One ^t1\n", "- [ ] #task Two ^t2\n",),
    );

    let json = capture_json(&vault, &day, "Plan work ~job:t1 ~job:t2 ~job:t1");
    let actions: Vec<&str> = json["relation_updates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["added", "added", "unchanged"], "{json}");
    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        inbox.contains("[[job#^t1]]") && inbox.contains("[[job#^t2]]"),
        "{inbox}"
    );
    let t1_count = inbox.matches("[[job#^t1]]").count();
    assert_eq!(t1_count, 1, "duplicate is a no-op:\n{inbox}");

    let before = snapshot(&vault, &["job.md", "mac_inbox.md", "day.md"]);
    let output = capture_fail(&vault, &day, "Other ~job:t1|a ~job:t1|b");
    assert!(!output.status.success(), "{}", format_output(&output));
    let message = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        message.contains("different labels") || message.contains("twice"),
        "{message}"
    );
    assert_unchanged(&vault, &before);
}

// LX15: `N ~T ~~T` writes two independent edges.
#[test]
fn lx15_to_and_from_same_target_are_independent() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx15");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");

    let json = capture_json(&vault, &day, "Fix it ~work:foo ~~work:foo");
    assert_eq!(json["relation_updates"].as_array().unwrap().len(), 2);
    assert_eq!(json["relation_updates"][0]["direction"], "to");
    assert_eq!(json["relation_updates"][1]["direction"], "from");
    let inbox = read(&vault, "mac_inbox.md");
    assert!(inbox.contains("- 🪢 **LINKS:** [[work#^foo]]"), "{inbox}");
    assert!(
        inbox.contains("- 🪢 **LINKED FROM:** [[work#^foo]]"),
        "{inbox}"
    );
    let work = read(&vault, "work.md");
    assert!(work.contains("- 🪢 **LINKS:** [[mac_inbox#^"), "{work}");
    assert!(
        work.contains("- 🪢 **LINKED FROM:** [[mac_inbox#^"),
        "{work}"
    );
}

// LX16: same-note S and T compose against staged text in one note write.
#[test]
fn lx16_same_note_pair_composes() {
    let (_temp, vault, day) = setup("bob-cli-rel-lx16");
    write_file(&vault.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");

    let json = capture_json(&vault, &day, "Ask plumber @job ~job:cleanup");
    assert_eq!(json["relative_target"], "job.md");
    let job = read(&vault, "job.md");
    assert!(
        job.contains("- 🪢 **LINKS:** [[#^cleanup]]"),
        "S LINKS uses a same-note link:\n{job}"
    );
    assert!(
        job.contains("- 🪢 **LINKED FROM:** [[#^"),
        "T LINKED FROM composed in the same note:\n{job}"
    );
    assert_day_untouched(&day);
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "routed to job.md, not inbox"
    );
}

// Previous-daily T skips the mirror (skip reason coverage beside LX1/LX6/LX10).
#[test]
fn previous_daily_target_skips_mirror() {
    let (_temp, vault, day) = setup("bob-cli-rel-prev-daily");
    write_file(
        &vault.join("2026/20261013.md"),
        "- [ ] #task Yesterday ^gtd\n",
    );

    let json = capture_json(&vault, &day, "Follow up ~2026/20261013:gtd");
    assert_eq!(json["mirror_skipped"][0]["reason"], "previous_daily_target");
    assert!(json.get("minted_relation_block_ids").is_none(), "{json}");
    assert_eq!(
        read(&vault, "2026/20261013.md"),
        "- [ ] #task Yesterday ^gtd\n"
    );
    assert!(
        read(&vault, "mac_inbox.md").contains("- 🪢 **LINKS:**"),
        "{}",
        read(&vault, "mac_inbox.md")
    );
}

// LD: a `||` one-off on a new label writes the plain label and one
// `relation_label.hide` event, and dry-run JSON matches real-run JSON.
#[test]
fn ld_new_one_off_label_writes_plain_label_and_hides() {
    let (_temp, vault, day) = setup("bob-cli-rel-ld-new");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");

    let json = capture_json(&vault, &day, "Fix trip ~work:foo||oneoff");
    assert_eq!(
        json["relation_label_hides"],
        serde_json::json!([{
            "label": "oneoff",
            "key": "oneoff",
            "existing_count": 0,
            "was_hidden": false,
            "action": "hide",
        }]),
        "{json}"
    );
    assert!(json.get("warnings").is_none(), "{json}");

    // The stored Markdown is identical to the `|` form: no pipes survive
    // in the LINKS entry.
    let inbox = read(&vault, "mac_inbox.md");
    assert!(
        inbox.contains("- 🪢 **LINKS:** oneoff [[work#^foo]]"),
        "{inbox}"
    );
    assert!(!inbox.contains("||"), "{inbox}");

    // Exactly one hide event, from the real run only.
    let events = journal_lines(&vault);
    assert_eq!(events.len(), 1, "{events:?}");
    let event: serde_json::Value =
        serde_json::from_str(&events[0]).expect("journal JSON");
    assert_eq!(event["kind"], "relation_label.hide");
    assert_eq!(event["key"], "oneoff");
    assert_eq!(event["name"], "oneoff");
    assert_eq!(event["via"], "bob-cli");
}

// LD: an existing visible label warns in JSON and human output, with the
// pre-capture count, and hides on commit.
#[test]
fn ld_visible_existing_label_warns_and_hides() {
    let (_temp, vault, day) = setup("bob-cli-rel-ld-visible");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Target ^target\n  - 🪢 **LINKS:** see [[work#^foo]]\n  - 🪢 **LINKS:** look [[work#^foo]]\n",
    );

    let json = capture_json(&vault, &day, "Fix thing ~tasks:target||see");
    assert_eq!(
        json["relation_label_hides"],
        serde_json::json!([{
            "label": "see",
            "key": "see",
            "existing_count": 1,
            "was_hidden": false,
            "action": "hide",
        }]),
        "{json}"
    );
    let warnings = json["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| {
            warning["code"] == "hides_existing_relation_label"
                && warning["label"] == "see"
                && warning["count"] == 1
                && warning["message"].as_str().is_some_and(|message| {
                    message.contains(
                        "hides \"see\" from relation suggestions (1 link)",
                    )
                })
        }),
        "{json}"
    );

    // The human run hides a still-visible label, so the count is the
    // pre-capture one.
    let output = capture_human(&vault, &day, "Fix that ~tasks:target||look");
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("🪢 hides \"look\" from suggestions (1 link)"),
        "{text}"
    );
    let errors = stderr(&output);
    assert!(
        errors.contains("hides \"look\" from relation suggestions"),
        "{errors}"
    );

    let events = journal_lines(&vault);
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(events[0].contains("\"key\":\"see\""), "{}", events[0]);
    assert!(events[1].contains("\"key\":\"look\""), "{}", events[1]);
}

// LD: an already-hidden label writes no event and no warning.
#[test]
fn ld_hidden_label_writes_no_event() {
    let (_temp, vault, day) = setup("bob-cli-rel-ld-hidden");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Target ^target\n  - 🪢 **LINKS:** see [[work#^foo]]\n",
    );
    let _ = capture_json(&vault, &day, "First ~tasks:target||see");
    assert_eq!(journal_lines(&vault).len(), 1);

    let json = capture_json(&vault, &day, "Second ~tasks:target||see");
    assert_eq!(
        json["relation_label_hides"],
        serde_json::json!([{
            "label": "see",
            "key": "see",
            "existing_count": 2,
            "was_hidden": true,
            "action": "already_hidden",
        }]),
        "{json}"
    );
    assert!(json.get("warnings").is_none(), "{json}");
    assert_eq!(journal_lines(&vault).len(), 1);

    let output = capture_human(&vault, &day, "Third ~tasks:target||see");
    assert_success(&output);
    assert!(
        !stdout(&output).contains("hides \"see\""),
        "{}",
        format_output(&output)
    );
}

// LD: mixing `|x` and `||x` hides `x` either way, and repeated hides of
// one key across items collapse to one journal event.
#[test]
fn ld_mixed_pipes_and_repeated_keys_collapse_to_one_event() {
    let (_temp, vault, day) = setup("bob-cli-rel-ld-mix");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Target ^target\n  - 🪢 **LINKS:** see [[work#^foo]]\n",
    );

    let json = capture_json_env(
        &vault,
        &day,
        "Mix ~tasks:target|see\n\nSecond ~tasks:target||see",
        &[],
    );
    assert!(json["relation_label_hides"].is_null(), "{json}");
    assert_eq!(
        json["captures"][1]["relation_label_hides"],
        serde_json::json!([{
            "label": "see",
            "key": "see",
            "existing_count": 1,
            "was_hidden": false,
            "action": "hide",
        }]),
        "{json}"
    );
    let events = journal_lines(&vault);
    assert_eq!(events.len(), 1, "{events:?}");
}

// LD: `-d` previews hides and warnings but writes nothing.
#[test]
fn ld_dry_run_previews_hides_without_writing() {
    let (_temp, vault, day) = setup("bob-cli-rel-ld-dry");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");
    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", &day)
        .arg("capture")
        .arg("--dry-run")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fix trip ~work:foo||oneoff")
        .output()
        .expect("dry-run json");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["relation_label_hides"][0]["action"], "hide");
    assert!(!vault.join("mac_inbox.md").exists());
    assert!(!vault.join(".bob/relations").exists());
}

// LD: a read-only journal directory still captures; the failure shows
// up as a `relation_label_hide_failed` warning with the retry command.
#[test]
#[cfg(unix)]
fn ld_read_only_journal_still_captures_with_retry_warning() {
    use std::os::unix::fs::PermissionsExt;

    let (_temp, vault, day) = setup("bob-cli-rel-ld-readonly");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");
    let journal_dir = vault.join(".bob/relations");
    fs::create_dir_all(&journal_dir).expect("journal dir");
    fs::set_permissions(&journal_dir, fs::Permissions::from_mode(0o555))
        .expect("read-only journal dir");

    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", &day)
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fix trip ~work:foo||oneoff")
        .output()
        .expect("capture with read-only journal");
    fs::set_permissions(&journal_dir, fs::Permissions::from_mode(0o755))
        .expect("restore journal dir");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["relation_label_hides"][0]["action"], "hide");
    let warnings = json["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| {
            warning["code"] == "relation_label_hide_failed"
                && warning["message"].as_str().is_some_and(|message| {
                    message.contains("bob relations hide")
                        && message.contains("--allow-unused")
                        && message.contains("oneoff")
                })
        }),
        "{json}"
    );
    assert!(
        read(&vault, "mac_inbox.md").contains("Fix trip"),
        "the capture itself still succeeds"
    );
}

/// Every `relation_label.hide` journal line in the scratch vault.
fn journal_lines(vault: &Path) -> Vec<String> {
    let dir = vault.join(".bob/relations");
    let mut lines = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return lines;
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();
    for path in paths {
        let body = fs::read_to_string(&path).expect("journal file");
        lines.extend(body.lines().map(str::to_string));
    }
    lines
}

// Dry-run human output says `would`.
#[test]
fn dry_run_human_says_would() {
    let (_temp, vault, day) = setup("bob-cli-rel-dry-human");
    write_file(&vault.join("work.md"), "- [ ] #task Design ^foo\n");
    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", &day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("--dry-run")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("Fix it ~work:foo")
        .output()
        .expect("dry-run human");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("🪢 would link to"), "{text}");
    assert!(!vault.join("mac_inbox.md").exists());
}
