//! `project_task_move` completion rows for sealed project-note drafts.

use crate::support::*;
use std::fs;

const BOB_NOW: &str = "2026-11-03 09:02:00";

fn write_move_fixture(vault: &std::path::Path) -> std::path::PathBuf {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"}
            ],
            "customStatuses": [
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "## Work\n",
            "- [?] #task Call Morgan Stanley about the 401k ^call-ms\n",
            "- [ ] #task Draft resignation memo\n",
            "- [*] #task Review benefits ^review-benefits\n",
            "- [ ] #task Draft the agreement ^dup\n",
            "- [ ] #task Archive the agreement ^dup\n",
            "- [ ] #task Project task #prj ^prj\n",
            "- [x] #task Closed task ^closed\n",
        ),
    );
    let day_file = vault.join("2026/20261103.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — WORK\n",
            "  - [[cash#^call-ms]]\n",
            "- [ ] () — ADMIN\n",
            "  - [[cash#^review-benefits]]\n",
        ),
    );
    day_file
}

fn complete_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
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
        .env("BOB_NOW", BOB_NOW)
        .env("BOB_DAY_FILE", day_file)
        .output()
        .expect("run project-task-move completion");
    assert_success(&output);
    serde_json::from_slice(stdout(&output).as_bytes()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn rows(value: &serde_json::Value) -> &[serde_json::Value] {
    value["candidates"].as_array().expect("candidate array")
}

fn row<'a>(value: &'a serde_json::Value, text: &str) -> &'a serde_json::Value {
    rows(value)
        .iter()
        .find(|candidate| candidate["text"].as_str() == Some(text))
        .unwrap_or_else(|| panic!("missing {text:?}: {value}"))
}

#[test]
fn empty_move_field_serves_parent_rows_and_json_picker_metadata() {
    let temp = TempDir::new("bob-cli-complete-project-task-move");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_move_fixture(&vault);

    let draft = "Create packet @cash^exit@#admin\n- ^";
    let value = complete_json(&vault, &day_file, draft, draft.len());
    assert_eq!(value["context"], "project_task_move");
    assert_eq!(value["grammar"], 25);
    assert_eq!(value["query"], "");
    assert_eq!(
        value["replacement"],
        serde_json::json!({"start": draft.len(), "end": draft.len()})
    );
    assert_eq!(value["project_task_move"]["parent"], "cash.md");
    assert_eq!(value["project_task_move"]["parent_route"], "cash");
    assert_eq!(value["project_task_move"]["destination"], "cash_exit.md");
    assert_eq!(value["project_task_move"]["sigil"], "^");
    assert_eq!(value["project_task_move"]["open_count"], 5);
    assert_eq!(value["project_task_move"]["pomodoro_name"], "admin");

    assert_eq!(value["picker"]["kind"], "project_task_move");
    assert_eq!(value["picker"]["scope"], "note");
    assert_eq!(value["picker"]["scope_token"], "^");
    assert_eq!(value["picker"]["note_target"], "cash.md");
    assert_eq!(value["picker"]["destination"], "cash_exit.md");
    assert_eq!(value["picker"]["continuation"], "\n- ^");
    assert_eq!(
        value["picker"]["marker_range"],
        serde_json::json!({"start": draft.len() - 1, "end": draft.len()})
    );

    let texts: Vec<&str> = rows(&value)
        .iter()
        .map(|candidate| candidate["text"].as_str().expect("row text"))
        .collect();
    assert_eq!(
        texts,
        [
            "Call Morgan Stanley about the 401k",
            "Draft resignation memo",
            "Review benefits",
            "Draft the agreement",
            "Archive the agreement",
        ]
    );
    assert!(!texts.iter().any(|text| text.contains("Project task")));
    assert!(!texts.iter().any(|text| text.contains("Closed task")));

    let call = row(&value, "Call Morgan Stanley about the 401k");
    assert_eq!(call["ref"].as_str().unwrap().split(':').count(), 2);
    assert_eq!(call["pomodoro"]["name"], "WORK");
    assert!(call["consequence"]
        .as_str()
        .unwrap()
        .contains("its WORK link follows"));
    assert_eq!(call["section"], "Work");

    let idless = row(&value, "Draft resignation memo");
    assert_eq!(idless["block_id"], serde_json::Value::Null);
    assert_eq!(idless["requires_block_id"], true);
    assert_eq!(idless["replacement"], "");
    assert!(!idless["block_id_suggestions"]
        .as_array()
        .expect("ID suggestions")
        .is_empty());
    assert!(idless["consequence"]
        .as_str()
        .unwrap()
        .starts_with("Needs an ID first · ⊕ ^"));

    let duplicates: Vec<_> = rows(&value)
        .iter()
        .filter(|candidate| candidate["block_id"] == "dup")
        .collect();
    assert_eq!(duplicates.len(), 2);
    assert!(duplicates.iter().all(|candidate| {
        candidate["replacement"] == ""
            && candidate["disabled_reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("2 tasks end ^dup"))
    }));
}

#[test]
fn query_ranking_add_block_id_ref_and_colon_consequence() {
    let temp = TempDir::new("bob-cli-complete-project-task-move-query");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_move_fixture(&vault);

    let draft = "Create packet @cash^exit@#admin\n- ^call";
    let value = complete_json(&vault, &day_file, draft, draft.len());
    assert_eq!(value["query"], "call");
    assert_eq!(
        value["replacement"],
        serde_json::json!({"start": draft.len() - 4, "end": draft.len()})
    );
    assert_eq!(
        rows(&value)[0]["text"],
        "Call Morgan Stanley about the 401k",
        "ID prefix ranks ahead of word matches"
    );

    let empty_draft = "Create packet @cash^exit@#admin\n- ^";
    let all = complete_json(&vault, &day_file, empty_draft, empty_draft.len());
    let idless = row(&all, "Draft resignation memo");
    let assigned = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("cash")
        .arg("--task-ref")
        .arg(idless["ref"].as_str().expect("stale-safe ref"))
        .arg("--block-id")
        .arg("draft-memo")
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("round-trip project task move ref through capture-task-id");
    assert_success(&assigned);
    let assigned: serde_json::Value =
        serde_json::from_slice(stdout(&assigned).as_bytes())
            .expect("capture-task-id JSON");
    assert_eq!(assigned["dry_run"], true);
    assert_eq!(assigned["route"], "cash");
    assert_eq!(assigned["block_id"], "draft-memo");

    let colon_draft = "Create packet @cash^exit@#admin\n- :";
    let colon =
        complete_json(&vault, &day_file, colon_draft, colon_draft.len());
    assert_eq!(colon["context"], "project_task_move");
    assert_eq!(colon["picker"]["scope_token"], ":");
    assert_eq!(colon["picker"]["continuation"], "\n- :");
    assert_eq!(
        row(&colon, "Call Morgan Stanley about the 401k")["consequence"],
        "Moves in · keeps its WORK link"
    );
    assert_eq!(
        row(&colon, "Review benefits")["consequence"],
        "Moves in · keeps its ADMIN link"
    );
}

#[test]
fn move_picker_uses_sticky_lane_and_blocked_status_copy() {
    let temp = TempDir::new("bob-cli-complete-project-task-move-lanes");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_move_fixture(&vault);
    let parent = vault.join("cash.md");
    let mut contents = fs::read_to_string(&parent).expect("parent note");
    contents.push_str(
        "- [/] #task Active task ^active\n- [?] #task Blocked task ^blocked\n",
    );
    write_file(&parent, &contents);

    let colon_draft = "Create @cash^exit@#admin\n- :";
    let colon =
        complete_json(&vault, &day_file, colon_draft, colon_draft.len());
    assert_eq!(
        row(&colon, "Active task")["consequence"],
        "Moves in · links under ADMIN"
    );
    assert_eq!(
        row(&colon, "Blocked task")["consequence"],
        "Moves in · links only if Bob can unblock it"
    );

    let caret_draft = "Create @cash^exit@\n- ^";
    let caret =
        complete_json(&vault, &day_file, caret_draft, caret_draft.len());
    assert_eq!(
        row(&caret, "Blocked task")["consequence"],
        "Moves into cash_exit.md · stays Blocked"
    );
}

#[test]
fn move_rows_report_draft_conflicts_future_start_and_missing_parent() {
    let temp = TempDir::new("bob-cli-complete-project-task-move-guards");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_move_fixture(&vault);

    let draft = concat!(
        "Create first @cash^first@\n",
        "- ^call-ms\n\n",
        "Create second @cash^second@\n",
        "- ^call",
    );
    let value = complete_json(&vault, &day_file, draft, draft.len());
    assert_eq!(value["context"], "project_task_move");
    let moved = row(&value, "Call Morgan Stanley about the 401k");
    assert_eq!(moved["replacement"], "");
    assert_eq!(moved["disabled_reason"], "already moved in item 1");

    let scheduled = "Create future s:3 @cash^future@\n- ^";
    let value = complete_json(&vault, &day_file, scheduled, scheduled.len());
    assert!(
        row(&value, "Draft resignation memo")["disabled_reason"]
            .as_str()
            .unwrap()
            .contains("project starts Nov 6 — only Blocked tasks can move in"),
        "{value}"
    );
    assert_eq!(
        row(&value, "Call Morgan Stanley about the 401k")["disabled_reason"],
        serde_json::Value::Null,
        "Blocked rows can move into a future project"
    );

    let no_parent = "Create @^orphan@\n- ^";
    let no_parent =
        complete_json(&vault, &day_file, no_parent, no_parent.len());
    assert_eq!(no_parent["context"], "project_task_move");
    assert!(rows(&no_parent).is_empty());
    assert_eq!(no_parent["initial_selection"], "none");
    assert_eq!(no_parent["warnings"][0], "choose the parent note first");

    let missing = "Create @missing^orphan@\n- ^";
    let missing = complete_json(&vault, &day_file, missing, missing.len());
    assert_eq!(missing["context"], "project_task_move");
    assert!(rows(&missing).is_empty());
    assert_eq!(
        missing["warnings"][0],
        "missing.md does not exist (run 'bob capture-targets' to list routable notes)"
    );
}

#[test]
fn choice_continuation_key_only_on_lone_caret_and_no_context_on_choice() {
    // cli-grammar MQ4: a lone `- ^` offers `^` as a continuation key; the
    // key is absent for `- :`, for a non-empty query, and at `- ^^` there
    // is no `project_task_move` context at all.
    let temp = TempDir::new("bob-cli-choice-continuation");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_move_fixture(&vault);

    let lone = "Create packet @cash^exit@\n- ^";
    let value = complete_json(&vault, &day_file, lone, lone.len());
    assert_eq!(value["context"], "project_task_move");
    assert_eq!(
        value["picker"]["action_continuation_keys"],
        serde_json::json!(["^"])
    );

    let link = "Create packet @cash^exit@\n- :";
    let value = complete_json(&vault, &day_file, link, link.len());
    assert_eq!(value["context"], "project_task_move");
    assert!(value["picker"]["action_continuation_keys"].is_null());

    let query = "Create packet @cash^exit@\n- ^dr";
    let value = complete_json(&vault, &day_file, query, query.len());
    assert_eq!(value["context"], "project_task_move");
    assert!(value["picker"]["action_continuation_keys"].is_null());

    let choice = "Create packet @cash^exit@\n- ^^";
    for cursor in [choice.len() - 1, choice.len()] {
        let value = complete_json(&vault, &day_file, choice, cursor);
        assert!(value["context"].is_null(), "{cursor}");
        assert!(
            value.get("project_task_move").is_none()
                || value["project_task_move"].is_null()
        );
    }
}

#[test]
fn offer_seal_twins_serve_same_rows_and_picker() {
    let temp = TempDir::new("bob-cli-complete-project-task-move-offer");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_move_fixture(&vault);

    // Twin of MC1 under the offer seal.
    let draft = "Create packet @cash:exit@#admin\n- ^";
    let value = complete_json(&vault, &day_file, draft, draft.len());
    assert_eq!(value["context"], "project_task_move");
    assert_eq!(value["project_task_move"]["parent"], "cash.md");
    assert_eq!(value["project_task_move"]["parent_route"], "cash");
    assert_eq!(value["project_task_move"]["destination"], "cash_exit.md");
    assert_eq!(value["project_task_move"]["sigil"], "^");
    assert_eq!(value["project_task_move"]["pomodoro_name"], "admin");
    assert_eq!(value["picker"]["kind"], "project_task_move");
    assert_eq!(value["picker"]["scope"], "note");
    assert_eq!(value["picker"]["scope_token"], "^");
    assert_eq!(value["picker"]["note_target"], "cash.md");
    assert_eq!(value["picker"]["destination"], "cash_exit.md");
    assert_eq!(value["picker"]["continuation"], "\n- ^");
    let texts: Vec<&str> = rows(&value)
        .iter()
        .map(|candidate| candidate["text"].as_str().expect("row text"))
        .collect();
    assert_eq!(
        texts,
        [
            "Call Morgan Stanley about the 401k",
            "Draft resignation memo",
            "Review benefits",
            "Draft the agreement",
            "Archive the agreement",
        ]
    );
    // A lone `- ^` under an offer seal still offers the `^` continuation key.
    assert_eq!(
        value["picker"]["action_continuation_keys"],
        serde_json::json!(["^"])
    );

    // Twin of MC4 under the offer seal.
    let colon_draft = "Create packet @cash:exit@#admin\n- :";
    let colon =
        complete_json(&vault, &day_file, colon_draft, colon_draft.len());
    assert_eq!(colon["context"], "project_task_move");
    assert_eq!(colon["picker"]["scope_token"], ":");
    assert_eq!(colon["picker"]["continuation"], "\n- :");
    assert_eq!(
        row(&colon, "Call Morgan Stanley about the 401k")["consequence"],
        "Moves in · keeps its WORK link"
    );
}

#[test]
fn move_picker_continuation_aliases_only_on_lone_caret() {
    let temp = TempDir::new("bob-cli-move-continuation-aliases");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_move_fixture(&vault);

    let lone = "Create packet @cash^exit@\n- ^";
    let value = complete_json(&vault, &day_file, lone, lone.len());
    assert_eq!(value["context"], "project_task_move");
    assert_eq!(
        value["picker"]["action_continuation_aliases"],
        serde_json::json!({".": "^"})
    );

    let link = "Create packet @cash^exit@\n- :";
    let value = complete_json(&vault, &day_file, link, link.len());
    assert_eq!(value["context"], "project_task_move");
    assert!(
        value["picker"].get("action_continuation_aliases").is_none()
            || value["picker"]["action_continuation_aliases"].is_null()
    );

    let query = "Create packet @cash^exit@\n- ^dr";
    let value = complete_json(&vault, &day_file, query, query.len());
    assert_eq!(value["context"], "project_task_move");
    assert!(
        value["picker"].get("action_continuation_aliases").is_none()
            || value["picker"]["action_continuation_aliases"].is_null()
    );
}
