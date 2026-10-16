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
fn leading_dependency_run_skips_a_destination_marker() {
    let json = parse_json("@cash &foo:bar Buy milk");
    assert_eq!(json["body"], "Buy milk");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["dependencies"][0]["raw"], "&foo:bar");
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
fn caret_alias_reports_existing_task_owner() {
    let existing = serde_json::json!({
        "kind": "existing_task",
        "route": "body",
        "block_id": "excercise",
    });
    for draft in ["^body:excercise &foo:bar", "&foo:bar ^body:excercise"] {
        let json = parse_json(draft);
        assert_eq!(json["mode"], "task_dependency", "{draft}");
        assert_eq!(json["body"], "", "{draft}");
        assert_eq!(json["dependency_target"], existing, "{draft}");
        assert_eq!(json["dependencies"][0]["raw"], "&foo:bar", "{draft}");
        assert_eq!(json["diagnostics"], serde_json::json!([]), "{draft}");
        assert_eq!(json["needs"], serde_json::json!([]), "{draft}");
    }
}

#[test]
fn plus_selector_beside_modifiers_needs_task_parent() {
    for draft in ["+ &foo:bar", "+ex &foo:bar", "&foo:bar +"] {
        let json = parse_json(draft);
        assert_eq!(json["mode"], "incomplete", "{draft}");
        assert_eq!(
            json["needs"],
            serde_json::json!(["task_parent"]),
            "{draft}"
        );
        assert_eq!(json["dependencies"][0]["raw"], "&foo:bar", "{draft}");
        assert!(json["dependency_target"].is_null(), "{draft}");
        assert_eq!(json["diagnostics"], serde_json::json!([]), "{draft}");
    }
}

#[test]
fn relationship_only_owner_reports_needs_text() {
    let expected = "`$2` needs task text: `@body+excercise` with no text only adds task relationships; add text to write a sub-bullet (`Note @body+excercise &foo:bar $2`)";
    let json = parse_json("@body+excercise &foo:bar $2");
    let diagnostics =
        json["diagnostics"].as_array().expect("diagnostics array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "relationship_needs_text");
    assert_eq!(diagnostics[0]["severity"], "error");
    assert_eq!(diagnostics[0]["message"], expected);
    assert_eq!(
        json["dependency_target"],
        serde_json::json!({
            "kind": "existing_task",
            "route": "body",
            "block_id": "excercise",
        })
    );

    let children = parse_json("@body+excercise &foo:bar\n- note");
    assert_eq!(
        children["diagnostics"][0]["code"],
        "relationship_needs_text"
    );
    assert!(
        children["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("`child bullets` needs task text"),
        "{}",
        children["diagnostics"][0]["message"]
    );
}

#[test]
fn suffixed_plus_owner_keeps_must_be_bare() {
    let json = parse_json("@body+excercise#bugs &foo:bar");
    let diagnostics =
        json["diagnostics"].as_array().expect("diagnostics array");
    assert_eq!(diagnostics[0]["code"], "invalid_dependency_target");
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("must be bare"),
        "{}",
        diagnostics[0]["message"]
    );
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
    assert_eq!(
        json["owner"],
        serde_json::json!({ "kind": "new_task", "text": "Buy Groceries!" })
    );
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
        stdout(&repeat).contains("already depends on"),
        "repeat reports the relationship already exists:\n{}",
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
        stdout(&second).contains("now depends on"),
        "append reports the new prerequisite:\n{}",
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

fn write_project_note(vault: &std::path::Path, name: &str, body: &str) {
    write_file(
        &vault.join(name),
        &format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}"),
    );
}

fn capture_prj_with_day_file(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    draft: &str,
) -> std::process::Output {
    bob_command()
        .env("BOB_NOW", DEPENDENCY_NOW)
        .env("BOB_DAY_FILE", day_file)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture")
}

fn capture_json_with_day_file(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    draft: &str,
) -> std::process::Output {
    bob_command()
        .env("BOB_NOW", DEPENDENCY_NOW)
        .env("BOB_DAY_FILE", day_file)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--format")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture JSON")
}

// PDG7 (§13.15): `&` on `^prj` adding an open target blocks the whole
// project note in the same write: `^prj` and every open resident flip to
// `[?]`, residents gain exactly one `#prj_is_blocked`, their live links
// leave today's open Pomodoros, and commitment transfers onto the open
// prerequisite (never onto a `^prj` target).
#[test]
fn capture_prj_dependency_blocks_tags_and_unlinks_residents() {
    let temp = TempDir::new("bob-cli-capture-prj-deps");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("cash.md"), "- [ ] #task Fund ^fund\n");
    write_project_note(
        &vault,
        "proj.md",
        concat!(
            "- [ ] #task Ship #hide ^prj\n",
            "```md\n",
            "- [ ] #task Fenced example ^fenced\n",
            "```\n",
            "- [ ] #task Work ^work\n",
            "- [/] #task Grind ^grind\n",
            "- [ ] #task Id-less resident\n",
        ),
    );
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) Morning\n",
            "  - [[proj#^work]]\n",
            "  - [[proj#^grind]]\n",
            "  - other context\n",
        ),
    );

    let output =
        capture_prj_with_day_file(&vault, &day_file, "&cash:fund @proj+prj");
    assert_success(&output);
    let after =
        fs::read_to_string(vault.join("proj.md")).expect("read project");
    let prj_line = after
        .lines()
        .find(|line| line.contains("^prj"))
        .expect("one ^prj line");
    assert!(
        prj_line.starts_with("- [?] #task Ship"),
        "^prj flips to Blocked:\n{after}"
    );
    assert!(
        !after.contains("#prj_is_blocked ^prj")
            && !after.lines().any(|line| line.contains("^prj")
                && line.contains("#prj_is_blocked")),
        "^prj never carries the tag:\n{after}"
    );
    for block in ["work", "grind"] {
        let lines: Vec<&str> = after
            .lines()
            .filter(|line| line.contains(&format!("^{block}")))
            .collect();
        assert_eq!(lines.len(), 1, "one ^{block} line:\n{after}");
        assert!(
            lines[0].starts_with("- [?] "),
            "resident ^{block} flips to Blocked:\n{after}"
        );
        assert_eq!(
            lines[0].matches("#prj_is_blocked").count(),
            1,
            "resident ^{block} carries exactly one tag:\n{after}"
        );
    }
    assert!(
        after.contains("- [?] #task Id-less resident #prj_is_blocked"),
        "id-less resident still receives the status/tag write:\n{after}"
    );
    assert!(
        after.contains("- [ ] #task Fenced example ^fenced"),
        "fenced tasks stay untouched after Depends-On insertion:\n{after}"
    );
    assert!(
        after.contains("⛓️ **DEPENDS ON:** [[cash#^fund]]"),
        "^prj keeps the ordinary managed line:\n{after}"
    );
    assert!(
        after.contains("[dependsOn:: cash__fund]"),
        "^prj keeps the ordinary field:\n{after}"
    );
    let day = fs::read_to_string(&day_file).expect("read day file");
    assert!(
        !day.contains("[[proj#^work]]") && !day.contains("[[proj#^grind]]"),
        "resident live links are consumed:\n{day}"
    );
    assert!(
        day.contains("other context"),
        "the entry survives on its other children:\n{day}"
    );
    let cash = fs::read_to_string(vault.join("cash.md")).expect("read cash");
    assert!(
        cash.contains("- [/] #task Fund"),
        "commitment transfers the strongest resident lane:\n{cash}"
    );

    // Repeating the same prerequisite is idempotent: no duplicate tags.
    let repeat =
        capture_prj_with_day_file(&vault, &day_file, "&cash:fund @proj+prj");
    assert_success(&repeat);
    let again =
        fs::read_to_string(vault.join("proj.md")).expect("read project");
    assert_eq!(
        again.matches("#prj_is_blocked").count(),
        3,
        "repeat adds no duplicate tags; id-less open residents stay tagged:\n{again}"
    );
}

// §13.6: a `^prj` dependent refuses same-note targets: a project cannot
// wait on its own tasks.
#[test]
fn capture_prj_dependency_refuses_same_note_targets() {
    let temp = TempDir::new("bob-cli-capture-prj-internal");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    let before = concat!(
        "---\ntype: [[project]]\nstatus: wip\n---\n",
        "- [ ] #task Ship #hide ^prj\n",
        "- [ ] #task Work ^work\n",
    );
    write_file(&vault.join("proj.md"), before);

    let output = capture_with_now(&vault, "&proj:work @proj+prj");
    assert!(
        !output.status.success(),
        "same-note ^prj targets must fail:\n{}",
        format_output(&output)
    );
    assert!(
        !stderr(&output).trim().is_empty(),
        "the refusal explains itself"
    );
    assert_eq!(
        fs::read_to_string(vault.join("proj.md")).expect("read project"),
        before,
        "refusals leave the vault intact"
    );
}

// PDG13 (§13.15): capture `&` on `^prj` targeting a descendant is refused.
#[test]
fn capture_prj_dependency_refuses_descendant_targets() {
    let temp = TempDir::new("bob-cli-capture-prj-descendant");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_project_note(&vault, "parent.md", "- [ ] #task Parent #hide ^prj\n");
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [ ] #task Child #hide ^prj\n",
            "- [ ] #task Work ^work\n",
        ),
    );
    let before =
        fs::read_to_string(vault.join("parent.md")).expect("read parent");

    let output = capture_with_now(&vault, "&child:work @parent+prj");
    assert!(
        !output.status.success(),
        "descendant ^prj targets must fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("inside a sub-project of this project"),
        "the refusal names the sub-project:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("parent.md")).expect("read parent"),
        before,
        "refusals leave the vault intact"
    );
}

// PDG13 (§13.6): a hand-authored descendant link on `^prj` stays on the
// line and is not projected into `[dependsOn::]`.
#[test]
fn capture_prj_dependency_deprojects_existing_descendant_links() {
    let temp = TempDir::new("bob-cli-capture-prj-deproject-descendant");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("job.md"), "- [ ] #task Prep ^prep\n");
    write_file(
        &vault.join("parent.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\n---\n",
            "- [ ] #task Parent #hide [dependsOn:: child__work] ^prj\n",
            "\t- ⛓️ **DEPENDS ON:** [[child#^work]]\n",
        ),
    );
    write_file(
        &vault.join("child.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n",
            "- [ ] #task Child #hide ^prj\n",
            "- [ ] #task Work [id:: child__work] ^work\n",
        ),
    );

    let output = bob_command()
        .env("BOB_NOW", DEPENDENCY_NOW)
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("&job:prep @parent+prj")
        .output()
        .expect("run capture with existing descendant link");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().is_some_and(|text| text
                .contains("project_internal_dependency")
                && text.contains("inside a sub-project of this project"))),
        "{json}"
    );
    let parent =
        fs::read_to_string(vault.join("parent.md")).expect("read parent");
    assert!(
        parent.contains("[[child#^work]]"),
        "the descendant link stays on the line:\n{parent}"
    );
    assert!(
        parent.contains("[[job#^prep]]"),
        "the new external link is added:\n{parent}"
    );
    assert!(
        !parent.contains("child__work"),
        "the descendant id is not projected:\n{parent}"
    );
}

// §13.5: `&` that newly Blocks an existing linked dependent consumes its
// live links in the same write.
#[test]
fn capture_dependency_consumes_links_when_newly_blocked() {
    let temp = TempDir::new("bob-cli-capture-dep-consume");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("cash.md"), "- [ ] #task Fund ^fund\n");
    write_file(&vault.join("body.md"), "- [*] #task Exercise ^excercise\n");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — Morning\n",
            "  - [[body#^excercise]]\n",
            "  - other context\n",
        ),
    );

    let output = capture_json_with_day_file(
        &vault,
        &day_file,
        "&cash:fund @body+excercise",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let body = fs::read_to_string(vault.join("body.md")).expect("read body");
    assert!(
        body.contains("- [?] #task Exercise"),
        "the dependent flips to Blocked:\n{body}"
    );
    let day = fs::read_to_string(&day_file).expect("read day file");
    assert!(
        !day.contains("[[body#^excercise]]"),
        "the newly Blocked dependent is unlinked:\n{day}"
    );
    assert!(
        day.contains("other context"),
        "the entry survives on its other children:\n{day}"
    );
    assert_eq!(json["consumed_links"].as_array().unwrap().len(), 1);
    assert_eq!(json["consumed_links"][0]["action"], "removed");
    assert_eq!(json["consumed_links"][0]["pomodoro"]["status"], "running");
    assert_eq!(json["consumed_links"][0]["pomodoro"]["removed"], false);
    assert_eq!(json["consumed_links"][0]["line"], 3);
    assert_eq!(
        json["warnings"][0],
        "removed 1 live Pomodoro link(s) to newly Blocked task(s)"
    );
}

#[test]
fn capture_consumption_reports_and_deletes_only_the_emptied_running_entry() {
    let temp = TempDir::new("bob-cli-capture-dep-consume-running");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("cash.md"), "- [ ] #task Fund ^fund\n");
    write_file(&vault.join("body.md"), "- [*] #task Exercise ^excercise\n");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Unrelated placeholder\n",
            "- [ ] (0900-0930) — Morning\n",
            "  - [[body#^excercise]]\n",
        ),
    );

    let output = capture_json_with_day_file(
        &vault,
        &day_file,
        "&cash:fund @body+excercise",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(day, "## Pomodoros\n\n- [ ] Unrelated placeholder\n");
    assert_eq!(json["consumed_links"][0]["pomodoro"]["name"], "Morning");
    assert_eq!(
        json["consumed_links"][0]["pomodoro"]["time_range"],
        "0900-0930"
    );
    assert_eq!(json["consumed_links"][0]["pomodoro"]["removed"], true);
    assert_eq!(json["pomodoro_blocks"].as_array().unwrap().len(), 1);
    assert_eq!(json["pomodoro_blocks"][0]["removed"], true);
    assert_eq!(
        json["pomodoro_blocks"][0]["roles"],
        serde_json::json!(["removed"])
    );
    assert_eq!(json["pomodoro_blocks"][0]["status"], "running");
    assert!(json["pomodoro_blocks"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .all(|line| line["change"] == "removed"));
}

#[test]
fn capture_consumption_keeps_authored_notes_under_plain_task_text() {
    let temp = TempDir::new("bob-cli-capture-dep-consume-notes");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("cash.md"), "- [ ] #task Fund ^fund\n");
    write_file(
        &vault.join("body.md"),
        "- [*] #task Exercise with [[steps#^walk|the walk]] ^excercise\n",
    );
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n\n",
            "- [ ] (0900-0930) — Morning\n",
            "  - [[body#^excercise]]\n",
            "    - authored work note\n",
            "- [ ] Unrelated placeholder\n",
        ),
    );

    let output = capture_json_with_day_file(
        &vault,
        &day_file,
        "&cash:fund @body+excercise",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("  - Exercise with the walk\n"), "{day}");
    assert!(day.contains("    - authored work note\n"), "{day}");
    assert!(day.contains("- [ ] Unrelated placeholder\n"), "{day}");
    assert_eq!(json["consumed_links"][0]["text"], "Exercise with the walk");
    assert_eq!(json["consumed_links"][0]["action"], "notes_kept");
    assert_eq!(json["consumed_links"][0]["pomodoro"]["removed"], false);
}

#[test]
fn human_consumption_line_replaces_the_legacy_warning() {
    let temp = TempDir::new("bob-cli-capture-dep-consume-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("cash.md"), "- [ ] #task Fund ^fund\n");
    write_file(&vault.join("body.md"), "- [*] #task Exercise ^excercise\n");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] (0900-0930) — Morning\n  - [[body#^excercise]]\n",
    );

    let output = capture_prj_with_day_file(
        &vault,
        &day_file,
        "&cash:fund @body+excercise",
    );
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(
            "− removed its Task Link from Morning 09:00–09:30 (running)"
        ),
        "{out}"
    );
    assert!(out.contains("deleted the empty Pomodoro"), "{out}");
    assert!(
        !stderr(&output).contains("removed 1 live Pomodoro link(s)"),
        "{}",
        stderr(&output)
    );
}

// PDB9 (§13.13): promotion never raises a `^prj` target into a lane; the
// walk continues through it to that project's open prerequisites.
#[test]
fn capture_promotion_walks_through_prj_without_raising_it() {
    let temp = TempDir::new("bob-cli-capture-prj-promotion");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("zeta.md"), "- [ ] #task Zed ^zed\n");
    write_project_note(
        &vault,
        "proj.md",
        concat!(
            "- [ ] #task Ship #hide [dependsOn:: zeta__zed] ^prj\n",
            "\t- ⛓️ **DEPENDS ON:** [[zeta#^zed]]\n",
        ),
    );
    write_file(&vault.join("why.md"), "- [*] #task Reason ^reason\n");

    let output = capture_with_now(&vault, "&proj:prj @why+reason");
    assert_success(&output);
    let proj = fs::read_to_string(vault.join("proj.md")).expect("read proj");
    assert!(
        proj.lines()
            .any(|line| line.starts_with("- [ ] #task Ship")),
        "^prj is never raised into a lane:\n{proj}"
    );
    let zeta = fs::read_to_string(vault.join("zeta.md")).expect("read zeta");
    assert!(
        zeta.contains("- [*] #task Zed"),
        "the walk continues to the project's prerequisites:\n{zeta}"
    );
}

// §13.6: cycle detection includes the implicit resident → `^prj`
// edges. Here `work` reaches `dee` only through its implicit edge to
// `^prj`'s target `tee`, so depending `dee` on `work` must refuse.
#[test]
fn capture_prj_dependency_sees_implicit_resident_cycles() {
    let temp = TempDir::new("bob-cli-capture-prj-cycle");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_blocked_tasks_settings(&vault);
    write_project_note(
        &vault,
        "proj.md",
        concat!(
            "- [ ] #task Ship #hide [dependsOn:: ext__tee] ^prj\n",
            "\t- ⛓️ **DEPENDS ON:** [[ext#^tee]]\n",
            "- [ ] #task Work ^work\n",
        ),
    );
    write_file(
        &vault.join("ext.md"),
        concat!(
            "- [ ] #task Tee [dependsOn:: dee__dee] ^tee\n",
            "\t- ⛓️ **DEPENDS ON:** [[dee#^dee]]\n",
        ),
    );
    let before = "- [ ] #task Dee ^dee\n";
    write_file(&vault.join("dee.md"), before);

    let output = capture_with_now(&vault, "&proj:work @dee+dee");
    assert!(
        !output.status.success(),
        "the implicit resident cycle must fail:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("cycle"),
        "the refusal names the cycle:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("dee.md")).expect("read dee"),
        before,
        "refusals leave the vault intact"
    );
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
