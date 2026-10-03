//! `task_link` completion candidates and accept-then-capture round trips.
//!
//! The worked-example vault mirrors the picker contract: 8 candidates in
//! canonical order behind a one-entry ledger, with the capture clock pinned
//! so pull-forward flags are deterministic.

use crate::support::*;
use std::fs;

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn write_worked_example(vault: &std::path::Path) -> std::path::PathBuf {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": [
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
    write_file(
        &vault.join("mac_inbox.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Call the bank [created::2026-09-29]\n",
    );
    write_file(
        &vault.join("health.md"),
        "---\ntype: [[area]]\n---\n## Errands\n- [?] #task Book dentist [scheduled::2026-10-03]\n",
    );
    write_file(
        &vault.join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Polish capture picker ^polish\n\t- [ ] #task Tune fuzzy weights\n",
    );
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n## Bugs\n- [*] #task Fix deep bug ^deep-fix\n- [ ] #task Fix flaky gkeep test\n- [x] #task Old fix ^old-fix\n## Writing\n- [/] #task Draft outline ^outline\n- [ ] #task Ship blog post #now ^blog\n- [-] #task Dropped idea\n",
    );
    write_file(
        &vault.join("archive.md"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Leftover ^leftover\n",
    );
    write_file(&vault.join("scratch.md"), "- [ ] #task Loose end ^loose\n");
    let day_file = vault.join("2026/20260930.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n\t- [[sase#^deep-fix]]\n",
    );
    day_file
}

fn complete_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    cursor: usize,
    text: &str,
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run task-link completion");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

#[test]
fn complete_task_link_lists_the_worked_example_in_json_and_human() {
    let temp = TempDir::new("bob-cli-complete-task-link-outputs");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    let json = complete_json(&vault, &day_file, 1, ":");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["context"], "task_link");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 0, "end": 1})
    );
    let replacements: Vec<&str> = json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .map(|candidate| {
            candidate["replacement"]
                .as_str()
                .expect("replacement string")
        })
        .collect();
    assert_eq!(
        replacements,
        vec![
            "@sase:deep-fix",
            "@sase:outline",
            "",
            "",
            "@bob:polish",
            "",
            "",
            "@sase:blog",
        ]
    );
    assert_eq!(json["candidates"][3]["pulls_forward"], true);

    let human = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("1")
        .arg("--")
        .arg(":")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run human task-link completion");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("@sase:deep-fix"), "{out}");
    assert!(out.contains("[*] Fix deep bug  · BUGS"), "{out}");
    assert!(
        out.contains(
            "[ ] Fix flaky gkeep test  · sase.md · needs ID (^fix-flaky-gkeep)"
        ),
        "{out}"
    );
    assert!(out.contains("8 candidates"), "{out}");
}

#[test]
fn complete_task_link_round_trip_for_an_identified_task() {
    let temp = TempDir::new("bob-cli-complete-task-link-identified");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    // Accept row 1 by splicing its replacement over its range.
    let json = complete_json(&vault, &day_file, 4, ":dee");
    assert_eq!(json["candidates"].as_array().expect("array").len(), 1);
    assert_eq!(json["candidates"][0]["replacement"], "@sase:deep-fix");
    let start = json["replacement"]["start"].as_u64().expect("start") as usize;
    let end = json["replacement"]["end"].as_u64().expect("end") as usize;
    let mut draft = ":dee".to_string();
    draft.replace_range(
        start..end,
        json["candidates"][0]["replacement"].as_str().expect("link"),
    );
    assert_eq!(draft, "@sase:deep-fix");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(&draft)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("capture the accepted link");
    assert_success(&output);
    let captured: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!(
                "capture stdout should be JSON: {error}\n{}",
                format_output(&output)
            )
        });
    assert_eq!(captured["ok"], true);
    assert_eq!(captured["kind"], "pomodoro_link");
    assert_eq!(captured["pomodoro_link_action"], "already_current");
}

#[test]
fn complete_task_link_round_trip_for_a_task_without_an_id() {
    let temp = TempDir::new("bob-cli-complete-task-link-id-less");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    // Row 7 is ID-less: name it with its first suggestion, then link and
    // start it with `=`.
    let json = complete_json(&vault, &day_file, 1, ":");
    let row = &json["candidates"][6];
    assert_eq!(row["replacement"], "");
    assert_eq!(row["route"], "sase");
    assert_eq!(row["requires_block_id"], true);
    let task_ref = row["ref"].as_str().expect("row ref").to_string();
    let suggestion = row["block_id_suggestions"][0]
        .as_str()
        .expect("first suggestion")
        .to_string();
    assert_eq!(suggestion, "fix-flaky-gkeep");

    let named = bob_command()
        .arg("capture-task-id")
        .arg("--route")
        .arg("sase")
        .arg("--task-ref")
        .arg(&task_ref)
        .arg("--block-id")
        .arg(&suggestion)
        .arg("-b")
        .arg(&vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("name the ID-less task");
    assert_success(&named);

    let draft = format!("@sase:{suggestion}=");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(&draft)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("capture the named link with a start");
    assert_success(&output);
    let captured: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!(
                "capture stdout should be JSON: {error}\n{}",
                format_output(&output)
            )
        });
    assert_eq!(captured["ok"], true);
    assert_eq!(captured["kind"], "pomodoro_link");

    // The task is Next with its new ID, and BUGS started.
    // Linking stamps freshness (creation never does).
    let sase = fs::read_to_string(vault.join("sase.md")).expect("read sase");
    assert!(
        sase.contains(&format!(
            "- [*] #task Fix flaky gkeep test [fresh:: 2026-09-30] ^{suggestion}"
        )),
        "named task should be Next with its ID:\n{sase}"
    );
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("(**0905-0930** [t:: 25m]) — BUGS"), "{day}");
    assert!(day.contains(&format!("[[sase#^{suggestion}]]")), "{day}");
}

#[test]
fn complete_task_link_pull_forward_agrees_with_the_scheduled_retire() {
    let temp = TempDir::new("bob-cli-complete-task-link-forward");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    // The health row carries a future scheduled date it would retire.
    let json = complete_json(&vault, &day_file, 1, ":");
    let row = &json["candidates"][3];
    assert_eq!(row["route"], "health");
    assert_eq!(row["scheduled"], "2026-10-03");
    assert_eq!(row["pulls_forward"], true);
    let task_ref = row["ref"].as_str().expect("row ref").to_string();
    let suggestion = row["block_id_suggestions"][0]
        .as_str()
        .expect("first suggestion")
        .to_string();

    let named = bob_command()
        .arg("capture-task-id")
        .arg("--route")
        .arg("health")
        .arg("--task-ref")
        .arg(&task_ref)
        .arg("--block-id")
        .arg(&suggestion)
        .arg("-b")
        .arg(&vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("name the health task");
    assert_success(&named);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(format!("@health:{suggestion}"))
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("link the health task");
    assert_success(&output);
    let health =
        fs::read_to_string(vault.join("health.md")).expect("read health");
    assert!(
        !health.contains("[scheduled::2026-10-03]"),
        "linking should retire the future scheduled field:\n{health}"
    );
    assert!(
        health.contains(&format!("^{suggestion}")),
        "the named ID should persist:\n{health}"
    );
}

#[test]
fn complete_task_link_batch_accepts_two_queries() {
    let temp = TempDir::new("bob-cli-complete-task-link-batch");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    // Accept each `:` item against the original draft, then splice from
    // the end so the first range stays valid.
    let draft = ":dee\n\n:blog";
    let first = complete_json(&vault, &day_file, 4, draft);
    assert_eq!(first["candidates"][0]["replacement"], "@sase:deep-fix");
    let second = complete_json(&vault, &day_file, 11, draft);
    assert_eq!(second["candidates"][0]["replacement"], "@sase:blog");
    let mut accepted = draft.to_string();
    for result in [&second, &first] {
        let start =
            result["replacement"]["start"].as_u64().expect("start") as usize;
        let end = result["replacement"]["end"].as_u64().expect("end") as usize;
        let link = result["candidates"][0]["replacement"]
            .as_str()
            .expect("link")
            .to_string();
        accepted.replace_range(start..end, &link);
    }
    assert_eq!(accepted, "@sase:deep-fix\n\n@sase:blog");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(&accepted)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("capture both accepted links");
    assert_success(&output);
    let captured: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!(
                "capture stdout should be JSON: {error}\n{}",
                format_output(&output)
            )
        });
    assert_eq!(captured["ok"], true);
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("[[sase#^deep-fix]]"), "{day}");
    assert!(day.contains("[[sase#^blog]]"), "{day}");
}

#[test]
fn complete_parent_task_plus_serves_json_and_human_candidates() {
    let temp = TempDir::new("bob-cli-complete-parent-task-output");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    let json = complete_json(&vault, &day_file, 1, "+");
    assert_eq!(json["context"], "task_parent");
    assert_eq!(json["query"], "");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 0, "end": 1})
    );
    assert_eq!(json["picker"]["kind"], "parent_task");
    assert_eq!(json["picker"]["scope"], "vault");
    assert_eq!(json["picker"]["scope_token"], "+");
    assert_eq!(json["picker"]["marker_range"], json["replacement"]);
    assert_eq!(json["picker"]["trigger_removal_range"], json["replacement"]);
    assert_eq!(json["picker"]["action_continuation_keys"][0], "0");
    assert_eq!(json["picker"]["action_continuation_keys"][10], "+");

    let replacements: Vec<&str> = json["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .map(|row| row["replacement"].as_str().expect("replacement"))
        .collect();
    assert_eq!(
        replacements,
        vec![
            "@sase+deep-fix",
            "@sase+outline",
            "",
            "",
            "@bob+polish",
            "",
            "",
            "@sase+blog",
        ]
    );
    assert!(json["candidates"][0].get("pulls_forward").is_none());
    assert_eq!(json["candidates"][6]["requires_block_id"], true);
    assert_eq!(
        json["candidates"][6]["block_id_suggestions"][0],
        "fix-flaky-gkeep"
    );

    let human = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("1")
        .arg("--")
        .arg("+")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run human parent-task completion");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("task_parent"), "{out}");
    assert!(out.contains("@sase+deep-fix"), "{out}");
    assert!(out.contains("Fix deep bug"), "{out}");
    assert!(out.contains("8 candidates"), "{out}");
}

#[test]
fn parent_picker_acceptance_round_trips_and_id_assignment_returns_replacement()
{
    let temp = TempDir::new("bob-cli-complete-parent-task-round-trip");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_worked_example(&vault);

    // A bare-plus accept uses the exact replacement from Bob. A solo
    // identified marker retains Ensure Next semantics.
    let completion = complete_json(&vault, &day_file, 1, "+");
    let mut solo = "+".to_string();
    let start = completion["replacement"]["start"].as_u64().unwrap() as usize;
    let end = completion["replacement"]["end"].as_u64().unwrap() as usize;
    solo.replace_range(
        start..end,
        completion["candidates"][0]["replacement"]
            .as_str()
            .expect("identified replacement"),
    );
    assert_eq!(solo, "@sase+deep-fix");
    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(&solo)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("dry-run accepted parent marker");
    assert_success(&dry_run);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).expect("dry-run JSON");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["kind"], "task_toggle");
    assert_eq!(dry_json["toggle_behavior"], "ensure_next");

    // A prose-terminal selector replaces only its plus token and keeps the
    // parent text as the new note body.
    let prose = "Call the bank +";
    let completion = complete_json(&vault, &day_file, prose.len(), prose);
    let selected = completion["candidates"][0]["replacement"]
        .as_str()
        .expect("selected marker");
    let mut accepted = prose.to_string();
    accepted.replace_range(
        completion["replacement"]["start"].as_u64().unwrap() as usize
            ..completion["replacement"]["end"].as_u64().unwrap() as usize,
        selected,
    );
    assert_eq!(accepted, "Call the bank @sase+deep-fix");
    let append = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(&accepted)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("dry-run accepted prose parent marker");
    assert_success(&append);
    let append_json: serde_json::Value =
        serde_json::from_str(stdout(&append).trim()).expect("append JSON");
    assert_eq!(append_json["dry_run"], true);
    assert_eq!(append_json["kind"], "sub_bullet");

    // An explicitly accepted ID-less vault row gets a safe ID, and Bob
    // returns the full parent marker so the client does not format it.
    let row = &completion["candidates"][6];
    assert_eq!(row["requires_block_id"], true);
    let task_ref = row["ref"].as_str().expect("stale-safe ref");
    let suggestion = row["block_id_suggestions"][0]
        .as_str()
        .expect("ID suggestion");
    let assigned = bob_command()
        .arg("capture-task-id")
        .arg("--route")
        .arg("sase")
        .arg("--task-ref")
        .arg(task_ref)
        .arg("--block-id")
        .arg(suggestion)
        .arg("--format")
        .arg("json")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("assign ID to selected task");
    assert_success(&assigned);
    let assigned_json: serde_json::Value =
        serde_json::from_str(stdout(&assigned).trim())
            .expect("assignment JSON");
    let parent_replacement = format!("@sase+{suggestion}");
    assert_eq!(assigned_json["parent_replacement"], parent_replacement);

    let accepted_idless = format!("Call the bank {parent_replacement}");
    let dry_run = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(&accepted_idless)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("dry-run accepted ID-less parent marker");
    assert_success(&dry_run);
    let accepted_json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).expect("capture JSON");
    assert_eq!(accepted_json["kind"], "sub_bullet");
    assert_eq!(accepted_json["block_id"], suggestion);
}
