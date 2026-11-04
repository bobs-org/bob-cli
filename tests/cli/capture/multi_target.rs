//! End-to-end capture tests for target runs.

use crate::support::*;
use std::{fs, path::Path, process::Output};

const NOW: &str = "2026-07-31 10:11:12";

fn setup(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] Unrelated placeholder\n");
    (temp, vault, day)
}

fn capture_output(
    vault: &Path,
    day: &Path,
    draft: &str,
    json: bool,
    extra_env: &[(&str, &Path)],
) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if json {
        command.arg("-f").arg("json");
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().expect("run bob capture")
}

fn capture_json(vault: &Path, day: &Path, draft: &str) -> serde_json::Value {
    let output = capture_output(vault, day, draft, true, &[]);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!("parse capture JSON: {error}\n{}", format_output(&output))
    })
}

fn write_priority_config(path: &Path) {
    write_file(
        path,
        concat!(
            "properties:\n",
            "  - name: scheduled\n",
            "    values: date\n",
            "  - name: dependsOn\n",
            "    values: local_task_id\n",
            "  - name: priority\n",
            "    values: priority\n",
            "    schedules: scheduled\n",
            "    levels:\n",
            "      - label: P1\n",
            "        value: high\n",
            "        min_days: 2\n",
            "        max_days: 7\n",
            "      - label: P2\n",
            "        value: medium\n",
            "        min_days: 8\n",
            "        max_days: 30\n",
            "      - label: P3\n",
            "        value: low\n",
            "        min_days: 31\n",
            "        max_days: 90\n",
            "      - label: P4\n",
            "        value: lowest\n",
            "        min_days: 91\n",
            "        max_days: 365\n",
        ),
    );
}

fn write_relation_settings(vault: &Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "taskFormat": "dataview",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
              {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"}
            ]
          }
        }"##,
    );
}

#[test]
fn target_run_captures_each_task_in_one_note_and_reports_the_group() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-basic");
    let note = vault.join("cash.md");
    write_file(
        &note,
        concat!(
            "- [ ] #task Parent ^parent\n",
            "  - [ ] #task Child ^child\n",
            "    - existing\n",
        ),
    );

    let json = capture_json(
        &vault,
        &day,
        "@cash+parent @cash+child Shared note\n- nested note",
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read capture note"),
        concat!(
            "- [ ] #task Parent ^parent\n",
            "  - [ ] #task Child ^child\n",
            "    - existing\n",
            "    - Shared note\n",
            "      - nested note\n",
            "  - Shared note\n",
            "    - nested note\n",
        )
    );
    assert_eq!(json["fanout"]["line"], 1);
    assert_eq!(json["fanout"]["position"], 1);
    assert_eq!(json["fanout"]["count"], 2);
    assert_eq!(json["fanout"]["marker"], "@cash+parent");
    assert_eq!(json["captures"][0]["fanout"]["position"], 1);
    assert_eq!(json["captures"][1]["fanout"]["position"], 2);
    assert_eq!(json["captures"][1]["fanout"]["marker"], "@cash+child");
    assert_eq!(json["kind"], "sub_bullet");
    assert_eq!(json["captures"].as_array().unwrap().len(), 2);

    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-human");
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Parent ^parent\n- [ ] #task Child ^child\n",
    );
    let output = capture_output(
        &vault,
        &day,
        "@cash+parent @cash+child Shared note",
        false,
        &[],
    );
    assert_success(&output);
    assert!(
        stdout(&output).contains("targets  @cash+parent · @cash+child"),
        "missing target-run header:\n{}",
        format_output(&output)
    );
    assert!(
        !stdout(&output).contains('\u{1b}'),
        "NO_COLOR preserves plain output:\n{}",
        format_output(&output)
    );

    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-dim-header");
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Parent ^parent\n- [ ] #task Child ^child\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("@cash+parent @cash+child Shared note")
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("CLICOLOR_FORCE", "1")
        .env_remove("NO_COLOR")
        .output()
        .expect("run colored capture");
    assert_success(&output);
    assert!(
        stdout(&output)
            .contains("\u{1b}[2mtargets  @cash+parent · @cash+child\u{1b}[0m"),
        "target-run header must use dim styling:\n{}",
        format_output(&output)
    );
}

#[test]
fn case_distinct_block_ids_are_distinct_fanout_targets() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-case-id");
    let note = vault.join("cash.md");
    write_file(&note, "- [ ] #task Upper ^A\n- [ ] #task Lower ^a\n");

    let json = capture_json(&vault, &day, "@cash+A @cash+a Shared note");
    assert_eq!(json["captures"].as_array().unwrap().len(), 2, "{json}");
    assert_eq!(json["captures"][0]["fanout"]["marker"], "@cash+A");
    assert_eq!(json["captures"][1]["fanout"]["marker"], "@cash+a");
    assert_eq!(json["captures"][0]["block_id"], "A", "{json}");
    assert_eq!(json["captures"][1]["block_id"], "a", "{json}");
    assert_eq!(
        fs::read_to_string(&note).expect("read fanout note"),
        concat!(
            "- [ ] #task Upper ^A\n",
            "\t- Shared note\n",
            "- [ ] #task Lower ^a\n",
            "\t- Shared note\n",
        )
    );
}

#[test]
fn duplicate_target_suffixes_still_refuse_without_changing_vault_files() {
    let (_temp, vault, day) =
        setup("bob-cli-capture-multi-target-suffix-duplicates");
    let note = vault.join("cash.md");
    let before = "- [ ] #task Parent ^parent\n  - REQUIREMENTS\n";
    write_file(&note, before);
    let alias = vault.join("alias.md");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(&note, &alias).expect("create task-note alias");
    }
    let mut before_files = vec![
        (note.clone(), fs::read(&note).expect("read task note")),
        (day.clone(), fs::read(&day).expect("read day note")),
        (
            vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            fs::read(
                vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            )
            .expect("read task settings"),
        ),
    ];
    #[cfg(unix)]
    before_files.push((alias.clone(), fs::read(&alias).expect("read alias")));

    for draft in [
        "@cash+parent @cash+parent#requirements Shared note",
        "@alias+parent#requirements @cash+parent Shared note",
        "@alias+parent @cash+parent#requirements Shared note",
    ] {
        if draft.contains("@alias") {
            #[cfg(not(unix))]
            continue;
        }
        let output = capture_output(&vault, &day, draft, true, &[]);
        assert!(!output.status.success(), "duplicate should refuse: {draft}");
        assert!(
            format_output(&output).contains("duplicate_capture_target"),
            "wrong duplicate refusal for {draft}:\n{}",
            format_output(&output)
        );
        for (path, contents) in &before_files {
            assert_eq!(
                fs::read(path).expect("read unchanged vault file"),
                *contents,
                "duplicate changed {} for {draft}",
                path.display()
            );
        }
    }
}

#[test]
fn a_late_member_refusal_names_the_target_and_rolls_back_the_group() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-rollback");
    let note = vault.join("cash.md");
    let before = concat!(
        "- [ ] #task Parent ^parent\n",
        "  - [ ] #task Child ^child\n",
    );
    write_file(&note, before);

    let output = capture_output(
        &vault,
        &day,
        "@cash+parent @cash+child#schedule-log Shared note",
        true,
        &[],
    );
    assert!(!output.status.success(), "reserved drawer should refuse");
    let combined = format_output(&output);
    assert!(
        combined.contains("@cash+child#schedule-log"),
        "error must name the failing target:\n{combined}"
    );
    assert_eq!(fs::read_to_string(&note).expect("read note"), before);
}

#[cfg(unix)]
#[test]
fn canonical_path_aliases_are_refused_as_duplicate_targets() {
    use std::os::unix::fs::symlink;

    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-alias");
    let note = vault.join("cash.md");
    let alias = vault.join("alias.md");
    let before = "- [ ] #task Parent ^parent\n";
    write_file(&note, before);
    symlink(&note, &alias).expect("create note alias");

    let output = capture_output(
        &vault,
        &day,
        "@alias+parent @cash+parent Shared note",
        true,
        &[],
    );
    assert!(!output.status.success(), "alias duplicate should refuse");
    let combined = format_output(&output);
    assert!(
        combined.contains("duplicate_capture_target"),
        "wrong duplicate refusal:\n{combined}"
    );
    assert_eq!(fs::read_to_string(&note).expect("read note"), before);
}

#[test]
fn note_less_bullets_and_drawers_are_filed_to_each_member() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-bullets");
    let note = vault.join("cash.md");
    write_file(
        &note,
        concat!(
            "- [ ] #task Alpha ^a\n",
            "  - REQUIREMENTS\n",
            "    - existing\n",
            "- [ ] #task Beta ^b\n",
        ),
    );

    let json = capture_json(
        &vault,
        &day,
        "@cash+a @cash+b\n- Ping Alice\n- REQUIREMENTS\n  - Postgres 17",
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        concat!(
            "- [ ] #task Alpha ^a\n",
            "  - REQUIREMENTS\n",
            "    - existing\n",
            "    - Postgres 17\n",
            "  - Ping Alice\n",
            "- [ ] #task Beta ^b\n",
            "  - Ping Alice\n",
            "  - REQUIREMENTS\n",
            "    - Postgres 17\n",
        )
    );
    assert_eq!(json["captures"][0]["notes"].as_array().unwrap().len(), 2);
    assert_eq!(json["captures"][1]["notes"].as_array().unwrap().len(), 2);
    assert_eq!(json["captures"][0]["drawers"][0]["key"], "REQUIREMENTS");
    assert_eq!(json["captures"][1]["drawers"][0]["created"], true);
}

#[test]
fn a_glued_drawer_suffix_stays_local_to_its_target() {
    let (_temp, vault, day) =
        setup("bob-cli-capture-multi-target-local-drawer");
    let note = vault.join("cash.md");
    write_file(
        &note,
        concat!(
            "- [ ] #task Alpha ^a\n",
            "  - REQUIREMENTS\n",
            "    - existing\n",
            "- [ ] #task Beta ^b\n",
        ),
    );

    let _json =
        capture_json(&vault, &day, "@cash+a#requirements @cash+b Shared note");
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        concat!(
            "- [ ] #task Alpha ^a\n",
            "  - REQUIREMENTS\n",
            "    - existing\n",
            "    - Shared note\n",
            "- [ ] #task Beta ^b\n",
            "  - Shared note\n",
        )
    );
}

#[test]
fn repeated_near_catalog_warnings_are_reported_once_per_target_run() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-warning");
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n",
    );
    write_file(
        &vault.join("one.md"),
        "- [ ] #task One ^one\n  - REQUIREMENTS\n",
    );
    write_file(
        &vault.join("two.md"),
        "- [ ] #task Two ^two\n  - REQUIREMENTS\n",
    );

    let json = capture_json(
        &vault,
        &day,
        "@cash+a#requirement @cash+b#requirement Note",
    );
    let warnings = json["warnings"].as_array().expect("warnings");
    assert_eq!(
        warnings.len(),
        1,
        "duplicate warning must be folded: {json}"
    );
    assert!(warnings[0]["message"]
        .as_str()
        .unwrap()
        .contains("REQUIREMENTS"));
}

#[test]
fn type_marker_retypes_every_target_in_the_run() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-retype");
    let note = vault.join("cash.md");
    let config = vault.join("config.yml");
    write_file(&note, "- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n");
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — OLD\n",
            "  - [[cash#^a]]\n",
            "  - [[cash#^b]]\n",
        ),
    );
    write_file(&config, "types:\n  FIX: blue\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+a @cash+b #fix")
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("capture retype target run");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["captures"][0]["task_theme"]["theme"]["name"], "FIX");
    assert_eq!(json["captures"][1]["task_theme"]["theme"]["name"], "FIX");
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        concat!(
            "- [*] #task Alpha [type:: FIX] [fresh:: 2026-07-31] ^a\n",
            "- [*] #task Beta [type:: FIX] [fresh:: 2026-07-31] ^b\n",
        )
    );
}

#[test]
fn dependency_and_blocking_relationships_apply_to_each_target() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-edges");
    write_relation_settings(&vault);
    let note = vault.join("cash.md");
    let root = vault.join("root.md");
    let job = vault.join("job.md");
    write_file(&note, "- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n");
    write_file(&root, "- [ ] #task Prerequisite ^root\n");
    write_file(&job, "- [ ] #task Cleanup ^cleanup\n");

    let dependency = capture_json(&vault, &day, "@cash+a @cash+b &root:root");
    assert_eq!(dependency["captures"].as_array().unwrap().len(), 2);
    let targets = fs::read_to_string(&note).expect("read targets");
    assert!(targets.contains("[dependsOn:: root__root]"), "{targets}");
    assert_eq!(targets.matches("[[root#^root]]").count(), 2, "{targets}");

    let blocked = capture_json(&vault, &day, "@cash+a @cash+b *job:cleanup");
    assert_eq!(blocked["captures"].as_array().unwrap().len(), 2);
    let job_text = fs::read_to_string(&job).expect("read blocking task");
    assert!(
        job_text.contains("[dependsOn:: cash__a, cash__b]"),
        "{job_text}"
    );
    let targets_after_block =
        fs::read_to_string(&note).expect("read targets after block");
    assert!(
        targets_after_block.contains("[dependsOn:: root__root]"),
        "the blocker edge leaves each target's own prerequisite intact: {targets_after_block}"
    );
    assert_eq!(targets_after_block.matches("[[job#^cleanup]]").count(), 2);
}

#[test]
fn relation_labels_apply_to_each_target_and_complete_a_picked_source_once() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-relations");
    write_relation_settings(&vault);
    let note = vault.join("cash.md");
    let source = vault.join("other.md");
    let config = vault.join("config.yml");
    write_file(&note, "- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n");
    write_file(&source, "- [ ] #task Other ^other\n");
    write_file(
        &config,
        "relations:\n  complete_labels:\n    - implemented by\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(r#"@cash+a @cash+b ~other:other|"related to""#)
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .output()
        .expect("capture relation target run");
    assert_success(&output);
    let relation: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(relation["captures"].as_array().unwrap().len(), 2);
    let targets = fs::read_to_string(&note).expect("read targets");
    assert!(targets.contains("[[other#^other]]"), "{targets}");
    assert_eq!(targets.matches("[[other#^other]]").count(), 2, "{targets}");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(r#"@cash+a @cash+b ~~other:other|"implemented by""#)
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("capture complete relation target run");
    assert_success(&output);
    let completed: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(completed["relation_completes"].as_array().unwrap().len(), 1);
    assert_eq!(
        fs::read_to_string(&source).expect("read completed source"),
        concat!(
            "- [x] #task Other  [completion:: 2026-07-31] ^other\n",
            "\t- 🪢 **LINKS:** implemented by [[cash#^a]] • implemented by [[cash#^b]]\n",
            "\t- 🪢 **LINKED FROM:** [[cash#^a]] (related to) • [[cash#^b]] (related to)\n",
        )
    );
    let targets = fs::read_to_string(&note).expect("read relation targets");
    assert_eq!(
        targets.matches("related to [[other#^other]]").count(),
        2,
        "{targets}"
    );
}

#[test]
fn clipboard_runs_share_each_read_and_reuse_one_snippet_file() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-clipboard");
    let note = vault.join("cash.md");
    let clipboard = vault.join("clipboard");
    let calls = vault.join("clipboard-calls");
    let clip_text = "long clipboard payload ".repeat(60);
    write_file(&note, "- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n");
    write_executable(
        &clipboard,
        &format!(
            "#!/bin/sh\nprintf x >> {}\nprintf '%s\\n' {}\n",
            shell_single_quote(calls.to_str().expect("calls path")),
            shell_single_quote(&clip_text),
        ),
    );

    let output = capture_output(
        &vault,
        &day,
        "@cash+a @cash+b Recap %c\n\n@cash+a @cash+b %c",
        true,
        &[("BOB_CLIPBOARD_CMD", &clipboard)],
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let first_snippet = json["captures"][0]["clip"]["snippet"]
        .as_str()
        .expect("first snippet");
    assert_eq!(json["captures"][1]["clip"]["snippet"], first_snippet);
    assert_eq!(json["captures"][2]["clip"]["snippet"], first_snippet);
    assert_eq!(json["captures"][3]["clip"]["snippet"], first_snippet);
    assert_eq!(fs::read_to_string(&calls).expect("read call count"), "xx");
    assert_eq!(
        fs::read_dir(vault.join("file"))
            .expect("read file dir")
            .count(),
        1,
        "both target runs reuse the same snippet reservation"
    );
    assert_eq!(
        fs::read_to_string(vault.join(first_snippet)).expect("read snippet"),
        format!("{clip_text}\n")
    );
    let written = fs::read_to_string(&note).expect("read note");
    let link = format!("[[{}]]", first_snippet.trim_end_matches(".md"));
    assert_eq!(
        written,
        format!(
            "- [ ] #task Alpha ^a\n\t- Recap\n\t\t- {link}\n\t- {link}\n- [ ] #task Beta ^b\n\t- Recap\n\t\t- {link}\n\t- {link}\n"
        )
    );
}

#[test]
fn counted_clipboard_reads_share_live_and_history_values_within_the_run() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-history");
    let note = vault.join("cash.md");
    let clipboard = vault.join("clipboard");
    let history = vault.join("history");
    let calls = vault.join("clipboard-calls");
    let history_calls = vault.join("history-calls");
    write_file(&note, "- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n");
    write_executable(
        &clipboard,
        &format!(
            "#!/bin/sh\nprintf x >> {}\nprintf 'live value\\n'\n",
            shell_single_quote(calls.to_str().expect("clipboard calls path")),
        ),
    );
    write_executable(
        &history,
        &format!(
            "#!/bin/sh\nprintf x >> {}\nprintf '%s\\n' '[\"live value\",\"older value\"]'\n",
            shell_single_quote(history_calls.to_str().expect("history calls path")),
        ),
    );

    let output = capture_output(
        &vault,
        &day,
        "@cash+a @cash+b %c2",
        true,
        &[
            ("BOB_CLIPBOARD_CMD", &clipboard),
            ("BOB_CLIPBOARD_HISTORY_CMD", &history),
        ],
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(
        json["captures"][0]["clip"]["lines"],
        json["captures"][1]["clip"]["lines"]
    );
    assert_eq!(
        fs::read_to_string(&calls).expect("live clipboard calls"),
        "x"
    );
    assert_eq!(
        fs::read_to_string(&history_calls).expect("history calls"),
        "x"
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        concat!(
            "- [ ] #task Alpha ^a\n",
            "\t- live value\n",
            "\t- older value\n",
            "- [ ] #task Beta ^b\n",
            "\t- live value\n",
            "\t- older value\n",
        )
    );
}

#[test]
fn schedule_and_priority_rolls_are_shared_within_each_target_run() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-rolls");
    let note = vault.join("cash.md");
    let config = vault.join("config.yml");
    write_file(&note, "- [ ] #task Alpha ^a\n- [ ] #task Beta ^b\n");
    write_priority_config(&config);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+a @cash+b Explicit s:3\n\n@cash+a @cash+b Rolled $1")
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_PRIORITY_ROLL_SEED", "1")
        .output()
        .expect("capture scheduled target runs");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 4, "{json}");
    assert_eq!(captures[0]["scheduled"], captures[1]["scheduled"]);
    assert_eq!(captures[2]["scheduled"], captures[3]["scheduled"]);
    assert_eq!(captures[0]["scheduled"], "2026-08-03");
    assert_eq!(captures[0]["priority"], serde_json::Value::Null);
    assert_eq!(captures[2]["priority"], "high");
    let rolled = captures[2]["scheduled"].as_str().expect("rolled date");
    assert_eq!(rolled, "2026-08-03");
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        format!(
            "- [ ] #task Alpha ^a\n\t- Explicit [scheduled::{date}]\n\t- Rolled [priority::high] [scheduled::{rolled}]\n\t\t- 🗓️ **SCHEDULE LOG**\n\t\t\t- *{rolled}* — 🎲 P0 → P1 · in **3** (2–7) days\n- [ ] #task Beta ^b\n\t- Explicit [scheduled::{date}]\n\t- Rolled [priority::high] [scheduled::{rolled}]\n\t\t- 🗓️ **SCHEDULE LOG**\n\t\t\t- *{rolled}* — 🎲 P0 → P1 · in **3** (2–7) days\n",
            date = captures[0]["scheduled"].as_str().unwrap(),
            rolled = rolled,
        )
    );
}

#[test]
fn ensure_next_reuses_one_typed_placeholder_for_the_group() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-next");
    let note = vault.join("cash.md");
    let config = vault.join("config.yml");
    write_file(
        &note,
        concat!(
            "- [ ] #task Alpha [type:: T] ^a\n",
            "- [ ] #task Beta [type:: T] ^b\n",
        ),
    );
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — OLD\n",
            "  - [[cash#^a]]\n",
            "  - [[cash#^b]]\n",
        ),
    );
    write_file(&config, "plan:\n  themes:\n    T: blue\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+a @cash+b")
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("ensure both targets Next");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures[0]["creates_pomodoro"], true, "{json}");
    assert_eq!(captures[1]["creates_pomodoro"], false, "{json}");
    assert_eq!(captures[0]["pomodoro_name"], "T");
    assert_eq!(captures[1]["pomodoro_name"], "T");
    assert_eq!(
        fs::read_to_string(&day).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] () — T\n",
            "  - [[cash#^a]]\n",
            "  - [[cash#^b]]\n",
            "- [ ] () — OLD\n",
        )
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        concat!(
            "- [*] #task Alpha [type:: T] [fresh:: 2026-07-31] ^a\n",
            "- [*] #task Beta [type:: T] [fresh:: 2026-07-31] ^b\n",
        )
    );
}

#[test]
fn bang_targets_toggle_links_independently() {
    let (_temp, vault, day) = setup("bob-cli-capture-multi-target-bang");
    let note = vault.join("cash.md");
    write_file(&note, "- [*] #task Alpha ^a\n- [*] #task Beta ^b\n");
    let before_day = concat!(
        "## Pomodoros\n",
        "- [ ] () — ONE\n",
        "  - [[cash#^a]]\n",
        "- [ ] () — TWO\n",
        "  - [[cash#^b]]\n",
    );
    write_file(&day, before_day);

    let json = capture_json(&vault, &day, "@cash+a! @cash+b!");
    assert_eq!(json["captures"][0]["toggle_direction"], "unlink");
    assert_eq!(json["captures"][1]["toggle_direction"], "unlink");
    assert_eq!(json["captures"][0]["removed_pomodoro_links"], 1);
    assert_eq!(json["captures"][1]["removed_pomodoro_links"], 1);
    assert_eq!(
        fs::read_to_string(&day).expect("read day"),
        "## Pomodoros\n- [ ] () — ONE\n- [ ] () — TWO\n"
    );
    assert_eq!(
        fs::read_to_string(&note).expect("read note"),
        "- [*] #task Alpha ^a\n- [*] #task Beta ^b\n"
    );
}
