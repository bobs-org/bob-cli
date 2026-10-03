//! Dependency capture grammar contract (`&note:block-id`): lexical
//! ownership, incomplete states, spans, the additive JSON contract, and
//! the staged dependency writer (managed `DEPENDS ON` lines plus derived
//! effects through the capture batch planner).

use crate::support::*;
use std::fs;

const DEPENDENCY_NOW: &str = "2026-09-30 09:02:00";

fn capture_with_now(
    vault: &std::path::Path,
    draft: &str,
) -> std::process::Output {
    bob_command()
        .env("BOB_NOW", DEPENDENCY_NOW)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture")
}

fn parse_json(draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-parse json");
    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

#[test]
fn dependency_new_task_reports_modifiers_and_target() {
    let json = parse_json("Buy Groceries! &foo:bar");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["body"], "Buy Groceries!");
    assert_eq!(json["mode"], "task");
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["dependencies"],
        serde_json::json!([{
            "raw": "&foo:bar",
            "note": "foo",
            "block_id": "bar",
            "quoted": false,
            "range": { "start": 15, "end": 23 },
        }])
    );
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({ "kind": "new_task" })
    );
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 15, "end": 16, "kind": "dependency_sigil" },
            { "start": 16, "end": 19, "kind": "dependency_note" },
            { "start": 20, "end": 23, "kind": "dependency_block_id" },
        ])
    );
    assert_eq!(json["diagnostics"], serde_json::json!([]));
}

#[test]
fn dependency_markers_interleave_with_destination_in_either_order() {
    let trailing = parse_json("Buy Groceries! @home &foo:bar &cash:budget");
    assert_eq!(trailing["body"], "Buy Groceries!");
    assert_eq!(trailing["mode"], "task");
    assert_eq!(trailing["route"], "home");
    assert_eq!(
        trailing["dependency_target"],
        serde_json::json!({ "kind": "new_task", "route": "home" })
    );
    assert_eq!(
        trailing["dependencies"],
        serde_json::json!([
            {
                "raw": "&foo:bar",
                "note": "foo",
                "block_id": "bar",
                "quoted": false,
                "range": { "start": 21, "end": 29 },
            },
            {
                "raw": "&cash:budget",
                "note": "cash",
                "block_id": "budget",
                "quoted": false,
                "range": { "start": 30, "end": 42 },
            },
        ])
    );

    let scheduled = parse_json("Buy milk s:1 &foo:bar");
    assert_eq!(scheduled["body"], "Buy milk");
    assert_eq!(scheduled["mode"], "task");
    assert_eq!(scheduled["dependencies"][0]["raw"], "&foo:bar");
    assert!(scheduled["spans"].as_array().expect("spans").contains(
        &serde_json::json!(
            { "start": 9, "end": 12, "kind": "schedule" }
        )
    ));
    let reversed = parse_json("Buy milk &foo:bar s:1");
    assert_eq!(reversed["body"], "Buy milk");
    assert_eq!(reversed["dependencies"][0]["raw"], "&foo:bar");

    let leading = parse_json("&cash:budget Buy Groceries! @home &foo:bar");
    assert_eq!(leading["body"], "Buy Groceries!");
    assert_eq!(leading["route"], "home");
    let raws: Vec<&str> = leading["dependencies"]
        .as_array()
        .expect("dependencies array")
        .iter()
        .map(|entry| entry["raw"].as_str().expect("raw"))
        .collect();
    assert_eq!(raws, vec!["&cash:budget", "&foo:bar"]);
}

#[test]
fn dependency_only_plus_target_reports_task_dependency() {
    let json = parse_json("&foo:bar @body+excercise");
    assert_eq!(json["body"], "");
    assert_eq!(json["mode"], "task_dependency");
    assert_eq!(json["route"], "body");
    assert_eq!(json["block_id"], "excercise");
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({
            "kind": "existing_task",
            "route": "body",
            "block_id": "excercise",
        })
    );
    assert_eq!(json["diagnostics"], serde_json::json!([]));
}

#[test]
fn dependency_only_colon_alias_reports_existing_target() {
    let json = parse_json("&foo:bar @body:excercise");
    assert_eq!(json["mode"], "task_dependency");
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({
            "kind": "existing_task",
            "route": "body",
            "block_id": "excercise",
        })
    );
    assert_eq!(json["diagnostics"], serde_json::json!([]));
}

#[test]
fn dependency_only_suffixed_colon_target_is_rejected() {
    let json = parse_json("&foo:bar @body:excercise#name");
    assert_eq!(json["mode"], "pomodoro_link");
    assert!(json["dependency_target"].is_null());
    let diagnostics =
        json["diagnostics"].as_array().expect("diagnostics array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "invalid_dependency_target");
    assert_eq!(diagnostics[0]["severity"], "error");
}

#[test]
fn ownerless_dependency_needs_its_target() {
    let json = parse_json("&foo:bar");
    assert_eq!(json["body"], "");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["dependency_target"]));
    assert_eq!(json["dependencies"][0]["raw"], "&foo:bar");
    assert!(json["dependency_target"].is_null());
    assert_eq!(json["diagnostics"], serde_json::json!([]));
}

#[test]
fn partial_queries_need_task_dependency() {
    for draft in ["&", "Buy Groceries! &fo", "Buy Groceries! &foo:"] {
        let json = parse_json(draft);
        let needs = json["needs"].as_array().expect("needs array");
        assert!(
            needs.iter().any(|need| need == "task_dependency"),
            "draft {draft:?} needs task_dependency, got {needs:?}"
        );
        assert_eq!(json["diagnostics"], serde_json::json!([]));
    }
    let owned = parse_json("Buy Groceries! &fo");
    assert_eq!(owned["body"], "Buy Groceries!");
    assert_eq!(owned["mode"], "task");
    assert_eq!(
        owned["dependency_target"],
        serde_json::json!({ "kind": "new_task" })
    );
}

#[test]
fn literal_ampersands_stay_prose() {
    for draft in [
        "R&D research",
        "Research & Development",
        "See [[foo &foo:bar]] for details",
        "Run `grep &foo:bar` loudly",
    ] {
        let json = parse_json(draft);
        assert_eq!(json["body"], draft, "draft {draft:?} keeps its body");
        assert!(
            json.get("dependencies").is_none(),
            "draft {draft:?} reports no dependencies"
        );
        assert!(json.get("dependency_target").is_none());
    }
    // A trailing bare `&` at the line end is the picker trigger, not
    // prose: it leaves the body and opens the prerequisite picker.
    let json = parse_json("Buy Groceries! &");
    assert_eq!(json["body"], "Buy Groceries!");
    assert_eq!(json["needs"], serde_json::json!(["task_dependency"]));
}

#[test]
fn escaped_ampersand_leaves_visible_token() {
    let json = parse_json("Buy Groceries! \\&foo:bar");
    assert_eq!(json["body"], "Buy Groceries! &foo:bar");
    assert_eq!(json["mode"], "task");
    assert!(json.get("dependencies").is_none());
    assert!(json.get("dependency_target").is_none());
}

#[test]
fn quoted_note_reports_decoded_identity_and_offsets() {
    let json = parse_json("Buy Groceries! &\"Shopping List\":bar");
    assert_eq!(json["body"], "Buy Groceries!");
    assert_eq!(
        json["dependencies"],
        serde_json::json!([{
            "raw": "&\"Shopping List\":bar",
            "note": "Shopping List",
            "block_id": "bar",
            "quoted": true,
            "range": { "start": 15, "end": 35 },
        }])
    );
    // The note span covers the quoted component exactly.
    assert!(json["spans"].as_array().expect("spans").contains(
        &serde_json::json!(
            { "start": 17, "end": 30, "kind": "dependency_note" }
        )
    ));
}

#[test]
fn malformed_modifiers_report_invalid_dependency() {
    for draft in ["Buy milk &../escape:id", "Buy milk &foo:bar!"] {
        let json = parse_json(draft);
        assert!(json.get("dependencies").is_none());
        let diagnostics =
            json["diagnostics"].as_array().expect("diagnostics array");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic["code"] == "invalid_dependency"),
            "draft {draft:?} reports invalid_dependency, got {diagnostics:?}"
        );
    }
}

#[test]
fn child_line_dependency_belongs_to_the_item() {
    let mut command = bob_command();
    command.arg("capture-parse").arg("-f").arg("json");
    let output = run_with_stdin(&mut command, "Buy milk\n- &foo:bar\n");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["body"], "Buy milk");
    assert_eq!(json["mode"], "task");
    assert!(
        json.get("sub_bullets").is_none(),
        "a modifiers-only child contributes no bullet"
    );
    assert_eq!(json["dependencies"][0]["raw"], "&foo:bar");
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({ "kind": "new_task" })
    );
    assert_eq!(json["diagnostics"], serde_json::json!([]));
}

#[test]
fn inherited_parent_selects_the_existing_dependent() {
    let mut command = bob_command();
    command.arg("capture-parse").arg("-f").arg("json");
    let output =
        run_with_stdin(&mut command, "@@home+shop\n\nBuy milk &foo:bar\n");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({
            "kind": "existing_task",
            "route": "home",
            "block_id": "shop",
            "inherited": true,
        })
    );
    assert_eq!(json["dependencies"][0]["raw"], "&foo:bar");

    // A bare `@@route` never names the dependent: the item stays
    // ownerless.
    let mut bare = bob_command();
    bare.arg("capture-parse").arg("-f").arg("json");
    let output = run_with_stdin(&mut bare, "@@home\n\n&foo:bar\n");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["dependency_target"]));
    assert!(json.get("dependency_target").is_none());
}

#[test]
fn complete_reports_task_dependency_field() {
    let temp = TempDir::new("bob-cli-capture-dependency-complete");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let draft = "Buy Groceries! &foo";
    let cursor = draft.len();
    let mut command = bob_command();
    command
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft);
    let output = command.output().expect("run capture-complete");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["context"], "task_dependency");
    assert_eq!(
        json["replacement"],
        serde_json::json!({ "start": 15, "end": 19 })
    );
    assert_eq!(json["query"], "foo");
    assert_eq!(json["owner"], serde_json::json!({ "kind": "new_task" }));
    assert_eq!(json["candidates"], serde_json::json!([]));
}

#[test]
fn section_bullet_with_dependency_is_rejected() {
    let json = parse_json("Buy milk @notes#Ideas &foo:bar");
    assert_eq!(json["mode"], "bullet");
    assert!(json.get("dependency_target").is_none());
    let diagnostics =
        json["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic["code"] == "dependency_unsupported_target"
        }),
        "section bullets reject dependencies, got {diagnostics:?}"
    );
}

#[test]
fn capture_executes_new_task_dependencies() {
    let temp = TempDir::new("bob-cli-capture-dependency-new");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Confirm grocery budget ^budget\n",
    );
    write_file(
        &vault.join("mac_inbox.md"),
        "## Tasks\n\n- [ ] #task Existing ^keep\n",
    );

    let output = capture_with_now(&vault, "Buy Groceries! &cash:budget");
    assert_success(&output);
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox");
    assert!(
        inbox.contains("- [?] #task Buy Groceries!"),
        "new dependent with an open prerequisite is Blocked:\n{inbox}"
    );
    assert!(
        inbox.contains("[dependsOn:: cash__budget]"),
        "new dependent carries the derived field:\n{inbox}"
    );
    assert!(
        inbox.contains("\t- ⛓️ **DEPENDS ON:** [[cash#^budget]]"),
        "new dependent carries the managed line:\n{inbox}"
    );
}

#[test]
fn capture_executes_dependency_only_updates_idempotently() {
    let temp = TempDir::new("bob-cli-capture-dependency-only");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("cash.md"),
        concat!(
            "- [ ] #task Confirm grocery budget ^budget\n",
            "- [*] #task Plan meals ^meals\n",
        ),
    );
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^excercise\n");

    let first = capture_with_now(&vault, "&cash:budget @body+excercise");
    assert_success(&first);
    let after_first =
        fs::read_to_string(vault.join("body.md")).expect("read body");
    assert!(
        after_first.contains("- [?] #task Exercise"),
        "open prerequisite blocks the dependent:\n{after_first}"
    );
    assert!(
        after_first.contains("[dependsOn:: cash__budget]"),
        "dependency-only capture writes the derived field:\n{after_first}"
    );
    assert!(
        after_first.contains("⛓️ **DEPENDS ON:** [[cash#^budget]]"),
        "dependency-only capture writes the managed line:\n{after_first}"
    );

    // Repeating the same prerequisite is idempotent: the managed line
    // and field keep one entry and the run still succeeds.
    let repeat = capture_with_now(&vault, "&cash:budget @body+excercise");
    assert_success(&repeat);
    let after_repeat =
        fs::read_to_string(vault.join("body.md")).expect("read body");
    assert_eq!(
        after_repeat.matches("cash#^budget").count(),
        1,
        "repeat adds no duplicate links:\n{after_repeat}"
    );
    assert!(
        stdout(&repeat).contains("0 added"),
        "repeat reports nothing added:\n{}",
        format_output(&repeat)
    );

    // A second prerequisite appends: both links survive and the tally
    // counts only the newly added one.
    let second = capture_with_now(&vault, "&cash:meals @body+excercise");
    assert_success(&second);
    let after_second =
        fs::read_to_string(vault.join("body.md")).expect("read body");
    assert!(
        after_second.contains("[[cash#^budget]]")
            && after_second.contains("[[cash#^meals]]"),
        "both prerequisites survive the append:\n{after_second}"
    );
    assert!(
        after_second.contains("[dependsOn:: cash__budget, cash__meals]"),
        "the field accumulates both ids:\n{after_second}"
    );
    assert!(
        stdout(&second).contains("1 added"),
        "append tallies only the new prerequisite:\n{}",
        format_output(&second)
    );

    // Dry runs plan through the same planner without touching the vault.
    let snapshot =
        fs::read_to_string(vault.join("body.md")).expect("read body");
    let dry = bob_command()
        .env("BOB_NOW", DEPENDENCY_NOW)
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("&cash:budget @body+excercise")
        .output()
        .expect("run bob capture --dry-run");
    assert_success(&dry);
    assert_eq!(
        fs::read_to_string(vault.join("body.md")).expect("read body"),
        snapshot,
        "dry runs leave the vault intact"
    );
}

#[test]
fn capture_dependency_failures_leave_the_vault_intact() {
    let temp = TempDir::new("bob-cli-capture-dependency-gate");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Confirm grocery budget ^budget\n",
    );
    let before = "## Tasks\n\n- [ ] #task Existing ^keep\n";
    write_file(&vault.join("mac_inbox.md"), before);
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^excercise\n");

    // Missing prerequisite notes, missing targets, self-dependencies,
    // and ownerless modifiers all refuse with an actionable error.
    for draft in [
        "Buy Groceries! &foo:bar",
        "&foo:bar @body+excercise",
        "&cash:nope @body+excercise",
        "&body:excercise @body+excercise",
        "&cash:budget",
    ] {
        let output = capture_with_now(&vault, draft);
        assert!(
            !output.status.success(),
            "draft {draft:?} must fail:\n{}",
            format_output(&output)
        );
        assert!(
            !stderr(&output).trim().is_empty(),
            "draft {draft:?} explains the failure"
        );
    }
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        before,
        "failed dependency captures leave the inbox intact"
    );
    assert_eq!(
        fs::read_to_string(vault.join("body.md")).expect("read body"),
        "- [ ] #task Exercise ^excercise\n",
        "failed dependency captures leave the dependent intact"
    );
    assert!(
        !vault.join("foo.md").exists(),
        "no prerequisite note is created"
    );
}

#[test]
fn capture_keeps_literal_ampersand_prose() {
    let temp = TempDir::new("bob-cli-capture-dependency-literal");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("Research & Development")
        .output()
        .expect("run bob capture");
    assert_success(&output);
    let contents =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox");
    assert!(
        contents.contains("Research & Development"),
        "literal ampersands still capture:\n{contents}"
    );
}

fn write_block_id_settings(vault: &std::path::Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"}
            ],
            "customStatuses": []
          }
        }"##,
    );
}

fn complete_block_id_json(
    vault: &std::path::Path,
    draft: &str,
    cursor: usize,
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
        .arg(draft)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("complete JSON")
}

#[test]
fn block_id_intent_follows_dependency_ownership() {
    let temp = TempDir::new("bob-cli-capture-dependency-intent");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join(".obsidian/plugins/obsidian-tasks-plugin"))
        .expect("create vault");
    write_block_id_settings(&vault);
    write_file(&vault.join("body.md"), "- [ ] #task Shop ^excercise\n");

    // A dependency-only colon owner completes its existing dependent.
    let existing = complete_block_id_json(&vault, "&foo:bar @body:", 15);
    assert_eq!(existing["context"], "pomodoro_block_id");
    assert_eq!(existing["block_id"]["intent"], "link");

    // A new task with a trailing dependency keeps new-task ID behavior,
    // and the modifier never enters the suggestion title.
    let new = complete_block_id_json(&vault, "New task @body: &foo:bar", 15);
    assert_eq!(new["context"], "pomodoro_block_id");
    assert_eq!(new["block_id"]["intent"], "new");
    assert_eq!(new["block_id"]["body"], "New task");
}

#[test]
fn rewrite_never_absorbs_ampersands_or_the_colon_alias() {
    // A bare `@@` with a dependency-only item has no absorbable marker:
    // the `&` modifier is not a destination and the colon alias keeps its
    // existing-task ownership.
    let mut command = bob_command();
    command
        .arg("capture-rewrite")
        .arg("-c")
        .arg("0")
        .arg("-f")
        .arg("json");
    let output =
        run_with_stdin(&mut command, "@@\n\n&foo:bar @body+excercise\n");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("rewrite JSON");
    assert_eq!(json["rule"], serde_json::Value::Null);
    assert_eq!(
        json["text"], "@@\n\n&foo:bar @body+excercise\n",
        "rewrite leaves dependency drafts untouched"
    );
}
