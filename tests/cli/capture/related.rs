//! Draft-mode `bob capture-related` contract goldens and gates.

use crate::support::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

const GOLDEN_ROOT: &str = "/Users/test/bob";
const PINNED_NOW: &str = "2026-10-20 12:00:00";

fn write_related_vault(vault: &Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": [
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
    write_file(
        &vault.join("dev.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "- [ ] #task Install new UPS battery ^ups\n",
            "- [ ] #task Order a replacement UPS battery ^ups-order\n",
            "- [?] #task Replace server UPS battery this weekend\n",
            "- [/] #task Validate JSON output for capture related command ^json-output\n",
            "- [ ] #task Use `openssl` command safely ^openssl\n",
            "- [ ] #task Read `%d` reference notes ^percent-reference\n",
            "- [x] #task Retire old printer from office [completion:: 2026-10-19] ^closed-printer\n",
        ),
    );
    write_file(
        &vault.join("health.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "- [ ] #task Install new UPS battery ^ups-health\n",
            "- [ ] #task Install new UPS battery the ^ups-lab\n",
            "- [ ] #task Replace old UPS battery before winter ^ups-winter\n",
        ),
    );
    write_file(
        &vault.join("ship.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task Install new UPS battery ^prj\n",
        ),
    );
}

fn related_json(vault: &Path, draft: &str, extra: &[&str]) -> Value {
    let mut command = bob_command();
    command
        .arg("capture-related")
        .arg("--bob-dir")
        .arg(vault)
        .arg("--format")
        .arg("json");
    for arg in extra {
        command.arg(arg);
    }
    let output = command
        .env("BOB_NOW", PINNED_NOW)
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-related draft mode");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("related draft JSON")
}

fn capture_parse_json(draft: &str) -> Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--format")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run bob capture-parse for an action splice");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

fn splice_at(draft: &str, at: usize, insertion: &str) -> String {
    format!("{}{}{}", &draft[..at], insertion, &draft[at..])
}

fn replace_span(
    draft: &str,
    start: usize,
    end: usize,
    replacement: &str,
) -> String {
    format!("{}{}{}", &draft[..start], replacement, &draft[end..])
}

fn normalize(value: Value, vault: &Path) -> Value {
    let vault = vault.to_string_lossy().to_string();
    fn walk(value: Value, vault: &str) -> Value {
        match value {
            Value::String(text) => {
                Value::String(text.replace(vault, GOLDEN_ROOT))
            }
            Value::Array(items) => Value::Array(
                items.into_iter().map(|item| walk(item, vault)).collect(),
            ),
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, item)| (key, walk(item, vault)))
                    .collect(),
            ),
            scalar => scalar,
        }
    }
    walk(value, &vault)
}

fn check_golden(name: &str, value: Value, vault: &Path) {
    let value = normalize(value, vault);
    let pretty = format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("pretty JSON")
    );
    let golden = fixture(&format!("capture_related/{name}.json"));
    if std::env::var("BOB_UPDATE_GOLDENS").is_ok() {
        fs::create_dir_all(golden.parent().expect("golden parent"))
            .expect("create golden dir");
        fs::write(&golden, &pretty).expect("write related golden");
        return;
    }
    let expected = fs::read_to_string(&golden).expect("read related golden");
    assert_eq!(pretty, expected, "capture-related golden {name} changed");
}

fn fixture_vault(temp: &TempDir) -> PathBuf {
    let vault = temp.path().join("bob");
    write_related_vault(&vault);
    vault
}

#[test]
fn recapture_contract_golden_includes_actions_defaults_and_cross_note_matches()
{
    let temp = TempDir::new("bob-cli-capture-related-recapture");
    let vault = fixture_vault(&temp);
    let result = related_json(&vault, "Install new UPS battery @dev", &[]);
    let item = &result["items"][0];
    assert_eq!(item["index"], 0);
    assert_eq!(item["mode"], "task");
    assert_eq!(item["matches"][0]["band"], "same_title");
    assert_eq!(item["matches"][0]["default_action"], "discard");
    assert_eq!(item["matches"][0]["actions"][0], "relate", "{result}");
    assert!(item["matches"][0].get("score").is_none());
    assert!(item["matches"].as_array().unwrap().iter().any(|row| {
        row["note_path"] == "health.md" && row["band"] == "same_title"
    }));
    let draft = "Install new UPS battery @dev";
    let first = &item["matches"][0];
    let marker_at = item["marker_at"].as_u64().unwrap() as usize;
    let replacements = &first["replacements"];
    for (action, field) in [
        ("relate", "relation_to"),
        ("needs", "dependency"),
        ("blocks", "blocked_task"),
    ] {
        assert!(first["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == action));
        let edited = splice_at(
            draft,
            marker_at,
            &format!(" {}", replacements[field].as_str().unwrap()),
        );
        let parsed = capture_parse_json(&edited);
        assert_eq!(parsed["mode"], "task", "{action}: {parsed}");
        assert_eq!(
            parsed["diagnostics"],
            serde_json::json!([]),
            "{action}: {parsed}"
        );
    }
    assert!(first["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "note"));
    let route_range = item["route_range"].as_object().unwrap();
    let edited = replace_span(
        draft,
        route_range["start"].as_u64().unwrap() as usize,
        route_range["end"].as_u64().unwrap() as usize,
        replacements["parent"].as_str().unwrap(),
    );
    let parsed = capture_parse_json(&edited);
    assert_eq!(parsed["mode"], "sub_bullet", "note: {parsed}");
    assert_eq!(
        parsed["diagnostics"],
        serde_json::json!([]),
        "note: {parsed}"
    );
    check_golden("recapture", result, &vault);
}

#[test]
fn draft_gates_short_titles_templates_and_noncreating_items_before_vault_io() {
    let temp = TempDir::new("bob-cli-capture-related-gates");
    let vault = fixture_vault(&temp);
    let short = related_json(&vault, "Milk @dev", &[]);
    assert_eq!(short["items"][0]["matches"], serde_json::json!([]));
    check_golden("short", short, &vault);

    let missing = temp.path().join("does-not-exist");
    let noncreating =
        related_json(&missing, "Remember a thought @dev#Ideas", &[]);
    assert_eq!(noncreating["items"], serde_json::json!([]));
    assert_eq!(noncreating["warnings"], serde_json::json!([]));
    check_golden("noncreating", noncreating, &missing);

    let template =
        related_json(&missing, "@@template:\n- [ ] #task {title}", &[]);
    assert_eq!(template["items"], serde_json::json!([]));
    assert_eq!(template["warnings"], serde_json::json!([]));
    check_golden("template", template, &missing);
}

#[test]
fn bulk_draft_anchors_dedupe_linked_tasks_and_project_modes() {
    let temp = TempDir::new("bob-cli-capture-related-bulk");
    let vault = fixture_vault(&temp);
    let draft = concat!(
        "Install new UPS battery @dev\n\n",
        "Order a replacement UPS battery @dev\n\n",
        "Read `unique-inline-code` safely @dev\n\n",
        "Project heading to check @ship^generated-plan+\n\n",
        "Start Pomodoro task title @dev:new-pomodoro-task",
    );
    let result = related_json(&vault, draft, &[]);
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(items[3]["mode"], "project_note");
    assert_eq!(items[4]["mode"], "pomodoro_task");
    assert_eq!(items[0]["range"]["start"], 0);
    assert_eq!(items[0]["discard_range"]["start"], 0);
    assert_eq!(
        items[1]["discard_range"]["start"],
        items[1]["range"]["start"]
    );
    assert_eq!(items[4]["discard_range"]["end"], draft.len());
    for item in items {
        let start = item["range"]["start"].as_u64().unwrap() as usize;
        let end = item["range"]["end"].as_u64().unwrap() as usize;
        let marker_at = item["marker_at"].as_u64().unwrap() as usize;
        assert!(draft.is_char_boundary(start) && draft.is_char_boundary(end));
        assert!(draft.is_char_boundary(marker_at));
        assert!(start <= marker_at && marker_at <= end);
    }
    assert!(items
        .iter()
        .flat_map(|item| item["matches"].as_array().unwrap())
        .any(|row| {
            row["band"] == "same_title" && row["kind"] == "project"
        }));
    assert!(
        items
            .iter()
            .flat_map(|item| item["matches"].as_array().unwrap())
            .any(|row| {
                row["band"] == "possible_duplicate"
                    && row["default_action"] == "note"
                    && row["replacements"]["parent"].is_string()
            }),
        "{result}"
    );
    assert!(result["items"].as_array().unwrap().iter().all(|item| {
        item["matches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("score").is_none())
    }));
    check_golden("bulk", result, &vault);
}

#[test]
fn linked_candidates_are_reported_and_excluded_and_closed_rows_use_split_defaults(
) {
    let temp = TempDir::new("bob-cli-capture-related-linked-closed");
    let vault = fixture_vault(&temp);
    let linked_draft = "Install new UPS battery ~dev:ups @dev";
    let linked = related_json(&vault, linked_draft, &[]);
    let linked_item = &linked["items"][0];
    assert!(linked_item["matches"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| { row["key"] != "dev.md#ups" }));
    assert_eq!(linked_item["linked"][0]["key"], "dev.md#ups");
    assert_eq!(linked_item["linked"][0]["link"], "relation");
    check_golden("linked", linked, &vault);

    let closed =
        related_json(&vault, "Retire old printer from office @dev", &[]);
    let closed_row = &closed["items"][0]["matches"][0];
    assert_eq!(closed_row["band"], "closed_duplicate");
    assert_eq!(closed_row["closed"]["status"], "done");
    assert_eq!(closed_row["default_action"], "discard");
    assert_eq!(
        closed_row["actions"],
        serde_json::json!(["open", "discard"])
    );
    check_golden("closed", closed, &vault);
}

#[test]
fn idless_rows_offer_suggestions_and_atomic_code_spans_stay_distinct() {
    let temp = TempDir::new("bob-cli-capture-related-idless-code");
    let vault = fixture_vault(&temp);
    let idless = related_json(
        &vault,
        "Replace server UPS battery this weekend @dev",
        &[],
    );
    let row = idless["items"][0]["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["requires_block_id"] == true)
        .expect("ID-less blocked row");
    assert_eq!(row["block_id"], Value::Null);
    assert!(!row["block_id_suggestions"].as_array().unwrap().is_empty());
    assert!(row.get("replacements").is_none());
    check_golden("idless", idless, &vault);

    let code = related_json(&vault, "`%dispatch` `%proc` @dev", &[]);
    assert_eq!(code["items"][0]["matches"], serde_json::json!([]));
    check_golden("code-spans", code, &vault);
}

#[test]
fn draft_related_stays_under_the_debug_perf_gate() {
    let temp = TempDir::new("bob-cli-capture-related-perf");
    let vault = temp.path().join("bob");
    write_capture_task_settings(&vault);
    for note in 0..40 {
        let tasks = (0..38)
            .map(|task| {
                format!(
                    "- [ ] #task Analyze pipeline event system note {note} task {task} ^event-{note}-{task}\n"
                )
            })
            .collect::<String>();
        write_file(
            &vault.join(format!("area-{note:02}.md")),
            &format!("---\ntype: [[area]]\n---\n{tasks}"),
        );
    }
    // Trivial baseline over the same query so process startup and machine
    // load cancel out of the gated quantity.
    let tiny = temp.path().join("tiny");
    write_capture_task_settings(&tiny);
    write_file(
        &tiny.join("area-05.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Analyze pipeline event system note 5 task 12 ^event-5-12\n",
    );
    let draft = "Analyze pipeline event system note 5 task 12 @area-05";
    // Warm the binary and the page cache once, untimed.
    let warm = related_json(&vault, draft, &[]);
    assert!(warm["ok"] == true);
    let warm_tiny = related_json(&tiny, draft, &[]);
    assert!(warm_tiny["ok"] == true);
    // Take the minimum of several paired timed runs so a scheduling spike
    // from parallel `cargo test` load falls out instead of failing the gate.
    let mut big_samples = Vec::with_capacity(7);
    let mut tiny_samples = Vec::with_capacity(7);
    let mut result = serde_json::Value::Null;
    for _ in 0..7 {
        let start = std::time::Instant::now();
        result = related_json(&vault, draft, &[]);
        big_samples.push(start.elapsed());
        let start = std::time::Instant::now();
        let tiny_result = related_json(&tiny, draft, &[]);
        tiny_samples.push(start.elapsed());
        assert!(tiny_result["ok"] == true);
    }
    assert!(result["ok"] == true);
    let min_big = *big_samples.iter().min().expect("big samples");
    let min_tiny = *tiny_samples.iter().min().expect("tiny samples");
    let min_diff = big_samples
        .iter()
        .zip(tiny_samples.iter())
        .map(|(big, tiny)| big.saturating_sub(*tiny))
        .min()
        .expect("paired samples");
    assert!(
        min_diff.as_millis() < 500,
        "draft query cost above baseline took {min_diff:?} \
         (big min {min_big:?} in {big_samples:?}; \
         tiny min {min_tiny:?} in {tiny_samples:?})"
    );
    assert!(
        min_big.as_millis() < 2000,
        "draft query gross bound took {min_big:?} in {big_samples:?}"
    );
}

#[test]
fn calibration_excludes_self_before_closed_suppression_and_limits() {
    let temp = TempDir::new("bob-cli-related-calibration");
    write_related_vault(temp.path());
    write_file(
        &temp.path().join("calibration.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task Recycle obsolete calibration fixture ^open\n",
            "- [x] #task Recycle obsolete calibration fixture [completion:: 2026-10-19] ^closed\n",
        ),
    );
    for (extra, band, line) in [
        (vec![], "same_title", 4),
        (vec!["--exclude", "calibration.md:4"], "closed_duplicate", 5),
        (vec!["-e", "dev.md:4"], "same_title", 4),
        (vec!["-e", "calibration.md:5"], "same_title", 4),
    ] {
        let output = bob_command()
            .args(["capture-related", "--bob-dir"])
            .arg(temp.path())
            .args([
                "--query",
                "Recycle obsolete calibration fixture",
                "--limit",
                "1",
                "--format",
                "json",
            ])
            .args(extra)
            .env("BOB_NOW", PINNED_NOW)
            .output()
            .expect("query calibration fixture");
        assert_success(&output);
        let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
        let matches = value["items"][0]["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1, "{value}");
        assert_eq!(matches[0]["band"], band);
        assert_eq!(matches[0]["line"], line);
    }
    for excluded in [
        "calibration.md",
        "calibration.md:0",
        "calibration.md:-1",
        "calibration.md:no",
        "/calibration.md:4",
        "../calibration.md:4",
        ":4",
    ] {
        let output = bob_command()
            .args([
                "capture-related",
                "--query",
                "Example",
                "--exclude",
                excluded,
            ])
            .output()
            .expect("reject invalid exclusion");
        assert_eq!(output.status.code(), Some(2), "{excluded}");
    }
    let output = bob_command()
        .args([
            "capture-related",
            "--exclude",
            "calibration.md:4",
            "--",
            "Example @dev",
        ])
        .output()
        .expect("reject exclusion in draft mode");
    assert_eq!(output.status.code(), Some(2));
}
