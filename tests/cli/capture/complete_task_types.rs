//! `task_type` capture-complete candidates and their zsh-facing values.
//!
//! The lead tests below cover phase `lead_complete`: the optional editor
//! `lead` object on `task_type` candidates, its exact wire bytes, the
//! eligibility gates that keep it off, and the lead-free shell, write, and
//! related paths.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path};

const NOW: &str = "2026-10-30 09:00:00";

fn fixture(
    root: &Path,
    configured: bool,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let vault = root.join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("dev.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "task_type: BUILD\n",
            "---\n",
            "# Dev\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Current fix [type:: FIX] ^current\n",
            "- [ ] #task Inherited type ^inherited\n",
        ),
    );
    let day_file = vault.join("2026/20261030.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [x] (0800-0830) — FIX\n",
            "- [x] (0830-0900) — BUILD\n",
        ),
    );
    write_file(
        &vault.join("2026/20261029.md"),
        "## Pomodoros\n- [x] () — DEEP WORK\n",
    );
    let mut config = String::from("plan:\n  default_type: BUILD\n");
    if configured {
        config.push_str(concat!(
            "types:\n",
            "  BUILD: yellow\n",
            "  FIX: blue\n",
            "  FIXUP: red\n",
            "  PREFIX: magenta\n",
            "  \"DEEP WORK\": violet\n",
        ));
    }
    let config_path = root.join("config.yml");
    write_file(&config_path, &config);
    (vault, config_path)
}

fn complete(
    root: &Path,
    vault: &Path,
    config: &Path,
    draft: &str,
    cursor: usize,
) -> Value {
    let output = bob_command()
        .args([
            "capture-complete",
            "-b",
            vault.to_str().expect("UTF-8 fixture path"),
            "-c",
            &cursor.to_string(),
            "-f",
            "json",
            "--",
            draft,
        ])
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", vault.join("2026/20261030.md"))
        .env("BOB_CONFIG_FILE", config)
        .env("XDG_CACHE_HOME", root.join("cache"))
        .output()
        .expect("run task-type capture completion");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion output should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn complete_end(
    root: &Path,
    vault: &Path,
    config: &Path,
    draft: &str,
) -> Value {
    complete(root, vault, config, draft, draft.len())
}

fn names(value: &Value) -> Vec<&str> {
    value["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .map(|row| row["name"].as_str().expect("candidate name"))
        .collect()
}

#[test]
fn configured_menu_uses_config_order_and_carries_task_effects() {
    let temp = TempDir::new("bob-cli-complete-task-types-config");
    let (vault, config) = fixture(temp.path(), true);
    let draft = "@dev #\"";
    let result = complete_end(temp.path(), &vault, &config, draft);

    assert_eq!(result["context"], "task_type");
    assert_eq!(result["subject"], "new");
    assert_eq!(result["query"], "");
    assert_eq!(
        result["replacement"],
        serde_json::json!({"start": 5, "end": 7})
    );
    assert_eq!(
        names(&result),
        vec!["BUILD", "FIX", "FIXUP", "PREFIX", "DEEP WORK"]
    );
    let build = &result["candidates"][0];
    assert_eq!(build["replacement"], "#build ");
    assert_eq!(build["origin"], "configured");
    assert_eq!(build["current"], true);
    assert_eq!(build["inherited"], true);
    assert_eq!(build["effect"], "keep");
    assert_eq!(build["cursor_after"], 12);
    assert_eq!(build["suggestion_label"], "default type");
    assert_eq!(build["themes"][0]["configured"], true);
    assert_eq!(result["theme_suggestion"]["theme"]["key"], "build");
}

#[test]
fn clock_tag_spellings_never_appear_as_type_completion_choices() {
    let temp = TempDir::new("bob-cli-complete-task-types-clock-tags");
    let (vault, config) = fixture(temp.path(), false);
    write_file(
        &vault.join("2026/20261029.md"),
        "## Pomodoros\n- [x] () — OFF\n- [x] () — OFFCLOCK\n- [x] () — UNTRACKED\n",
    );
    let result = complete_end(temp.path(), &vault, &config, "Fix it #o");
    let offered = names(&result);
    for reserved in ["OFF", "ON", "OFFCLOCK", "OFF-CLOCK", "UNTRACKED"] {
        assert!(!offered.contains(&reserved), "offered {reserved}: {result}");
    }
}

#[test]
fn query_orders_exact_prefix_substring_and_adds_typed_row() {
    let temp = TempDir::new("bob-cli-complete-task-types-rank");
    let (vault, config) = fixture(temp.path(), true);
    let result = complete_end(temp.path(), &vault, &config, "Fix it #fix");

    assert_eq!(names(&result), vec!["FIX", "FIXUP", "PREFIX"]);
    assert_eq!(result["candidates"][0]["replacement"], "#fix");
    assert_eq!(result["candidates"][0]["effect"], "set");

    let typed = complete_end(temp.path(), &vault, &config, "Fix it #fixs");
    let row = typed["candidates"]
        .as_array()
        .unwrap()
        .last()
        .expect("typed row");
    assert_eq!(row["name"], "FIXS");
    assert_eq!(row["origin"], "typed");
    assert_eq!(row["themes"][0]["configured"], false);
    assert_eq!(row["similar_to"][0], "FIX");
    assert_eq!(row["refused"], "type_not_configured");
}

#[test]
fn exact_hidden_type_remains_available_but_not_in_empty_menu() {
    let temp = TempDir::new("bob-cli-complete-task-types-hidden");
    let (vault, config) = fixture(temp.path(), true);
    write_file(
        &vault.join(".bob/themes/test.jsonl"),
        "{\"v\":1,\"ts\":\"2026-10-30T07:00:00Z\",\"host\":\"test\",\"kind\":\"theme.hide\",\"key\":\"fix\",\"name\":\"FIX\",\"via\":\"test\"}\n",
    );
    let journal = fs::read_to_string(vault.join(".bob/themes/test.jsonl"))
        .expect("theme journal");

    let empty = complete_end(temp.path(), &vault, &config, "#\"");
    assert!(!names(&empty).contains(&"FIX"));
    let exact = complete_end(temp.path(), &vault, &config, "#fix");
    let fix = exact["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "FIX")
        .expect("exact hidden row");
    assert_eq!(fix["hidden"], true);
    assert_eq!(
        fs::read_to_string(vault.join(".bob/themes/test.jsonl")).unwrap(),
        journal,
        "completion never changes theme preferences"
    );
}

#[test]
fn current_and_inherited_existing_types_drive_effects() {
    let temp = TempDir::new("bob-cli-complete-task-types-existing");
    let (vault, config) = fixture(temp.path(), true);

    let current =
        complete_end(temp.path(), &vault, &config, "@dev:current #\"");
    assert_eq!(current["subject"], "existing");
    let fix = current["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "FIX")
        .unwrap();
    assert_eq!(fix["current"], true);
    assert_eq!(fix["effect"], "keep");
    let build = current["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "BUILD")
        .unwrap();
    assert_eq!(build["inherited"], true);
    assert_eq!(build["effect"], "clear");

    let inherited =
        complete_end(temp.path(), &vault, &config, "@dev:inherited #\"");
    assert_eq!(inherited["subject"], "existing");
    let build = inherited["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "BUILD")
        .unwrap();
    assert_eq!(build["current"], true);
    assert_eq!(build["inherited"], true);
    assert_eq!(build["effect"], "keep");
}

#[test]
fn attached_ensure_next_type_uses_existing_subject_without_extra_space() {
    let temp = TempDir::new("bob-cli-complete-task-types-attached");
    let (vault, config) = fixture(temp.path(), true);

    // Glued `#` after `+id` is a drawer, not a type.
    let drawer = complete_end(temp.path(), &vault, &config, "@dev+current#");
    assert_eq!(drawer["context"], "drawer");

    let full_menu =
        complete_end(temp.path(), &vault, &config, "@dev+current #");
    assert_eq!(full_menu["context"], "task_type");
    assert_eq!(full_menu["subject"], "existing");
    assert_eq!(full_menu["query"], "");
    assert_eq!(
        full_menu["replacement"],
        serde_json::json!({"start": 13, "end": 14})
    );

    let typed = complete_end(temp.path(), &vault, &config, "@dev+current #fi");
    assert_eq!(typed["context"], "task_type");
    assert_eq!(typed["subject"], "existing");
    let fix = typed["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "FIX")
        .unwrap();
    assert_eq!(fix["replacement"], "#fix ");

    let body =
        complete_end(temp.path(), &vault, &config, "@dev+current #fi detail");
    assert_ne!(body["context"], "task_type");
}

#[test]
fn attached_project_task_empty_suffix_is_new_and_keeps_suffix_replacements() {
    let temp = TempDir::new("bob-cli-complete-task-types-project-attached");
    let (vault, config) = fixture(temp.path(), true);

    let empty = complete_end(
        temp.path(),
        &vault,
        &config,
        "Fix the parser @dev^new-id #",
    );
    assert_eq!(empty["context"], "task_type");
    assert_eq!(empty["subject"], "new");
    assert_eq!(empty["query"], "");
    assert_eq!(empty["candidates"][0]["replacement"], "#build");

    let draft = "@dev^new-id #f Fix the parser";
    let typed = complete(temp.path(), &vault, &config, draft, 14);
    assert_eq!(typed["context"], "task_type");
    assert_eq!(typed["subject"], "new");
    assert_eq!(
        typed["replacement"],
        serde_json::json!({"start": 12, "end": 14})
    );
    let fix = typed["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "FIX")
        .unwrap();
    assert_eq!(fix["replacement"], "#fix");

    for draft in [
        "Fix the parser @dev^current #\"DEEP W",
        "@dev+current #\"DEEP W",
    ] {
        let partial = complete_end(temp.path(), &vault, &config, draft);
        assert_eq!(partial["context"], "task_type", "{draft:?}");
    }
}

#[test]
fn literal_replacement_child_and_second_line_triggers_work() {
    let temp = TempDir::new("bob-cli-complete-task-types-literal");
    let (vault, config) = fixture(temp.path(), true);
    let literal = complete_end(temp.path(), &vault, &config, "#\"");
    let deep = literal["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "DEEP WORK")
        .unwrap();
    assert_eq!(deep["replacement"], "#\"DEEP WORK\" ");

    let child = "Parent task\n- check the lexer #fi";
    assert_eq!(
        complete_end(temp.path(), &vault, &config, child)["context"],
        "task_type"
    );
    let second_line = "Parent task\n #fi";
    assert_eq!(
        complete_end(temp.path(), &vault, &config, second_line)["context"],
        "task_type"
    );
    let mid_line = "Ask about #fix today";
    assert!(
        complete_end(temp.path(), &vault, &config, mid_line)["context"]
            .is_null()
    );
    assert!(
        complete_end(temp.path(), &vault, &config, "Fix it t:fix")["context"]
            .is_null(),
        "retired t: syntax is ordinary prose"
    );
}

#[test]
fn route_completion_wins_at_its_token_and_unconfigured_menu_uses_history() {
    let temp = TempDir::new("bob-cli-complete-task-types-context");
    let (vault, config) = fixture(temp.path(), false);
    let route = complete(temp.path(), &vault, &config, "@de #fix", 3);
    assert_eq!(route["context"], "route");

    let history = complete_end(temp.path(), &vault, &config, "#b");
    assert_eq!(history["context"], "task_type");
    assert_eq!(history["candidates"][0]["origin"], "history");
    assert_eq!(history["candidates"][0]["name"], "BUILD");
}

fn leads_fixture(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let (vault, config) = fixture(root, true);
    write_file(
        &config,
        concat!(
            "plan:\n",
            "  default_type: BUILD\n",
            "types:\n",
            "  BUILD: yellow\n",
            "  FIX: blue\n",
            "  PLAN: green\n",
            "  READ: violet\n",
            "  WRITE: orange\n",
            "  FIXUP: red\n",
            "  PREFIX: magenta\n",
            "  \"DEEP WORK\": violet\n",
            "  REPAIR: aqua\n",
            "capture:\n",
            "  type_leads:\n",
            "    BUILD: Build\n",
            "    FIX: Fix\n",
            "    PLAN: Plan\n",
            "    READ: Read\n",
            "    WRITE: Write\n",
            "    \"DEEP WORK\": Deep work\n",
            "    REPAIR: 'R\u{e9}parer'\n",
        ),
    );
    (vault, config)
}

fn row<'a>(result: &'a Value, name: &str) -> &'a Value {
    result["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .find(|candidate| candidate["name"] == name)
        .unwrap_or_else(|| panic!("missing {name} candidate: {result}"))
}

fn assert_no_leads(result: &Value) {
    for candidate in result["candidates"].as_array().expect("candidate array") {
        assert!(
            candidate.get("lead").is_none(),
            "unexpected lead on {}: {result}",
            candidate["name"]
        );
    }
}

#[test]
fn editor_lead_wire_bytes_cover_the_worked_table() {
    let temp = TempDir::new("bob-cli-complete-task-types-leads");
    let (vault, config) = leads_fixture(temp.path());

    // `#f` in an empty panel: the motivating case.
    let result = complete_end(temp.path(), &vault, &config, "#f");
    let fix = row(&result, "FIX");
    assert_eq!(fix["replacement"], "#fix ");
    assert_eq!(fix["cursor_after"], 5);
    assert_eq!(
        fix["lead"],
        serde_json::json!({
            "text": "Fix",
            "insert_at": 5,
            "insertion": "Fix ",
            "cursor_after": 9,
        }),
        "{result}"
    );
    // A refused row never carries one.
    let typed = row(&result, "F");
    assert_eq!(typed["refused"], "type_not_configured");
    assert!(typed.get("lead").is_none(), "{result}");

    // A bare `#` previews every configured lead.
    let bare = complete_end(temp.path(), &vault, &config, "#");
    for (name, text, insert_at, cursor_after) in [
        ("BUILD", "Build", 7, 13),
        ("FIX", "Fix", 5, 9),
        ("PLAN", "Plan", 6, 11),
        ("READ", "Read", 6, 11),
        ("WRITE", "Write", 7, 13),
    ] {
        let candidate = row(&bare, name);
        assert_eq!(
            candidate["lead"],
            serde_json::json!({
                "text": text,
                "insert_at": insert_at,
                "insertion": format!("{text} "),
                "cursor_after": cursor_after,
            }),
            "{name}: {bare}"
        );
    }

    // An authored space: the replacement keeps the typed space, so the
    // candidate `cursor_after` is 4 while the lead still starts at 5.
    let spaced = complete(temp.path(), &vault, &config, "#f ", 2);
    let fix = row(&spaced, "FIX");
    assert_eq!(fix["replacement"], "#fix");
    assert_eq!(fix["cursor_after"], 4);
    assert_eq!(
        fix["lead"],
        serde_json::json!({
            "text": "Fix",
            "insert_at": 5,
            "insertion": "Fix ",
            "cursor_after": 9,
        }),
        "{spaced}"
    );

    // A child below: the marker owns no trailing space, so the lead
    // brings its own leading one.
    let child = complete(temp.path(), &vault, &config, "#f\n  - child", 2);
    let fix = row(&child, "FIX");
    assert_eq!(
        fix["lead"],
        serde_json::json!({
            "text": "Fix",
            "insert_at": 4,
            "insertion": " Fix ",
            "cursor_after": 9,
        }),
        "{child}"
    );

    // A leading route stays transparent.
    let routed = complete_end(temp.path(), &vault, &config, "@cash #f");
    assert_eq!(
        row(&routed, "FIX")["lead"],
        serde_json::json!({
            "text": "Fix",
            "insert_at": 11,
            "insertion": "Fix ",
            "cursor_after": 15,
        }),
        "{routed}"
    );

    // A leading clock tag stays transparent.
    let clock = complete_end(temp.path(), &vault, &config, "#off #f");
    assert_eq!(
        row(&clock, "FIX")["lead"],
        serde_json::json!({
            "text": "Fix",
            "insert_at": 10,
            "insertion": "Fix ",
            "cursor_after": 14,
        }),
        "{clock}"
    );

    // Offsets in a later bulk item are absolute draft bytes.
    let bulk = complete_end(temp.path(), &vault, &config, "Buy milk\n\n#w");
    assert_eq!(
        row(&bulk, "WRITE")["lead"],
        serde_json::json!({
            "text": "Write",
            "insert_at": 17,
            "insertion": "Write ",
            "cursor_after": 23,
        }),
        "{bulk}"
    );

    // A quoted literal with a configured lead.
    let literal = complete_end(temp.path(), &vault, &config, "#\"DEEP W");
    let deep = row(&literal, "DEEP WORK");
    assert_eq!(
        deep["lead"],
        serde_json::json!({
            "text": "Deep work",
            "insert_at": 13,
            "insertion": "Deep work ",
            "cursor_after": 23,
        }),
        "{literal}"
    );

    // A multibyte lead measures bytes, not characters.
    let multibyte = complete_end(temp.path(), &vault, &config, "#repair");
    let repair = row(&multibyte, "REPAIR");
    assert_eq!(
        repair["lead"],
        serde_json::json!({
            "text": "R\u{e9}parer",
            "insert_at": 8,
            "insertion": "R\u{e9}parer ",
            "cursor_after": 17,
        }),
        "{multibyte}"
    );
}

#[test]
fn hidden_exact_type_keeps_its_lead() {
    let temp = TempDir::new("bob-cli-complete-task-types-lead-hidden");
    let (vault, config) = leads_fixture(temp.path());
    write_file(
        &vault.join(".bob/themes/test.jsonl"),
        "{\"v\":1,\"ts\":\"2026-10-30T07:00:00Z\",\"host\":\"test\",\"kind\":\"theme.hide\",\"key\":\"fix\",\"name\":\"FIX\",\"via\":\"test\"}\n",
    );
    let exact = complete_end(temp.path(), &vault, &config, "#fix");
    let fix = row(&exact, "FIX");
    assert_eq!(fix["hidden"], true);
    assert_eq!(fix["lead"]["text"], "Fix", "{exact}");
}

#[test]
fn lead_stays_off_everything_but_an_empty_new_task_start() {
    let temp = TempDir::new("bob-cli-complete-task-types-lead-gates");
    let (vault, config) = leads_fixture(temp.path());
    write_file(
        &vault.join("sase.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "---\n",
            "# Sase\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Existing foo ^foo\n",
        ),
    );

    // A title already exists, the marker trails, or text follows it.
    let titled = complete(temp.path(), &vault, &config, "#f Fix it", 2);
    assert_no_leads(&titled);
    let trailing = complete_end(temp.path(), &vault, &config, "Fix it #f");
    assert!(trailing["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .any(|candidate| {
            candidate["name"] == "FIX" && candidate.get("lead").is_none()
        }));
    assert_no_leads(&trailing);
    let followed = complete(temp.path(), &vault, &config, "#f @sase", 2);
    assert_no_leads(&followed);

    // Existing subjects retype only.
    for draft in ["@dev:current #f", "@dev+current #f"] {
        let existing = complete_end(temp.path(), &vault, &config, draft);
        assert_eq!(existing["subject"], "existing", "{draft}");
        let fix = row(&existing, "FIX");
        assert!(fix.get("lead").is_none(), "{draft}: {existing}");
    }
    for draft in ["@sase:foo #f", "@sase+foo #f"] {
        let existing = complete_end(temp.path(), &vault, &config, draft);
        assert_eq!(existing["subject"], "existing", "{draft}");
        assert_no_leads(&existing);
    }

    // The project-note caret seal reports no refusal today, and the
    // parser probe still keeps the lead off it.
    let seal = complete_end(temp.path(), &vault, &config, "@dev^foo@ #f");
    let fix = row(&seal, "FIX");
    assert!(fix["refused"].is_null(), "{seal}");
    assert!(fix.get("lead").is_none(), "{seal}");

    // Template rows never carry leads.
    let template = complete_end(temp.path(), &vault, &config, "%t #f");
    assert_no_leads(&template);

    // A child bullet line is never a task start, even though the marker
    // lexer marks it leading.
    let child = complete_end(temp.path(), &vault, &config, "Parent\n  - #f");
    assert_no_leads(&child);
}

#[test]
fn lead_stays_off_notebook_routes() {
    let temp = TempDir::new("bob-cli-complete-task-types-lead-notebook");
    let (vault, config) = leads_fixture(temp.path());
    write_file(
        &vault.join("dev_tools.md"),
        concat!(
            "---\n",
            "parent: \"[[dev]]\"\n",
            "type: \"[[notebook]]\"\n",
            "summary: Tools.\n",
            "---\n",
            "\n",
            "# Developer tools\n",
        ),
    );
    let notebook = complete_end(temp.path(), &vault, &config, "@dev_tools #f");
    assert_no_leads(&notebook);
    assert!(
        notebook["candidates"]
            .as_array()
            .expect("candidate array")
            .iter()
            .any(|candidate| candidate["name"] == "FIX"),
        "notebook menu still offers types, just lead-free: {notebook}"
    );
}

#[test]
fn lead_stays_off_record_routes() {
    let temp = TempDir::new("bob-cli-complete-task-types-lead-record");
    let (vault, config) = leads_fixture(temp.path());
    write_file(
        &config,
        concat!(
            "plan:\n",
            "  default_type: BUILD\n",
            "types:\n",
            "  BUILD: yellow\n",
            "  FIX: blue\n",
            "records:\n",
            "  restaurant:\n",
            "    dir: eat\n",
            "    label: Restaurant\n",
            "    fields:\n",
            "      status: {}\n",
            "capture:\n",
            "  type_leads:\n",
            "    BUILD: Build\n",
            "    FIX: Fix\n",
        ),
    );
    write_file(
        &vault.join("body.md"),
        "---\ntype: [[area]]\n---\n\n# Body\n",
    );
    write_file(
        &vault.join("eat.md"),
        concat!(
            "---\n",
            "type: [[album]]\n",
            "parent: \"[[body]]\"\n",
            "summary: Places to eat.\n",
            "record:\n",
            "  noun: restaurant\n",
            "  label: Restaurant\n",
            "  fields:\n",
            "    status: {}\n",
            "---\n\n",
            "# Ideas\n"
        ),
    );
    write_file(
        &vault.join("eat/nagi.md"),
        concat!(
            "---\n",
            "parent: \"[[eat]]\"\n",
            "type: \"[[restaurant]]\"\n",
            "title: Nagi\n",
            "status: liked\n",
            "---\n",
            "# Nagi\n"
        ),
    );
    let record = complete_end(temp.path(), &vault, &config, "@eat/nagi #f");
    assert_no_leads(&record);
    assert!(
        record["candidates"]
            .as_array()
            .expect("candidate array")
            .iter()
            .any(|candidate| candidate["name"] == "FIX"),
        "record menu still offers types, just lead-free: {record}"
    );
}

#[test]
fn lead_only_title_returns_no_lexical_band() {
    let temp = TempDir::new("bob-cli-complete-task-types-lead-related");
    let (vault, _config) = leads_fixture(temp.path());
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
        &vault.join("fix.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "- [ ] #task Fix the parser ^fix1\n",
        ),
    );
    // A lead-only title ("Fix") stays below the lexical band's bars: the
    // short query gates before vault IO.
    let output = bob_command()
        .arg("capture-related")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("--format")
        .arg("json")
        .arg("--")
        .arg("#fix Fix ")
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob capture-related draft mode");
    assert_success(&output);
    let json: Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!(
                "parse capture-related JSON: {error}\n{}",
                format_output(&output)
            )
        });
    assert_eq!(json["items"][0]["matches"], serde_json::json!([]), "{json}");
}

#[test]
fn without_leads_task_type_json_has_no_lead_key() {
    let temp = TempDir::new("bob-cli-complete-task-types-no-leads");
    let (vault, config) = fixture(temp.path(), true);
    for draft in ["#f", "#", "@cash #f"] {
        let result = complete_end(temp.path(), &vault, &config, draft);
        assert!(!result.to_string().contains("lead"), "{draft}: {result}");
        assert_no_leads(&result);
    }
}

#[test]
fn shell_completion_never_carries_leads() {
    let temp = TempDir::new("bob-cli-complete-task-types-lead-shell");
    let (vault, config) = leads_fixture(temp.path());
    let output = bob_completion_command()
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .arg("bob")
        .arg("capture")
        .arg("#f")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run configured bob __complete");
    assert_success(&output);
    let out = stdout(&output);
    let values: Vec<&str> = out
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').next().unwrap_or_default())
        .collect();
    assert!(values.contains(&"#fix"), "lead-free insert: {values:?}");
    assert!(
        !values.iter().any(|value| {
            value.starts_with("#fix ")
                || value.starts_with("#build ")
                || value.starts_with("#write ")
        }),
        "shell values never glue lead text onto the marker: {values:?}"
    );
    let descriptions: Vec<&str> = out
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').nth(1).unwrap_or_default())
        .collect();
    assert!(
        descriptions
            .iter()
            .any(|description| description.starts_with("your type ·")),
        "descriptions keep their pre-lead shape: {descriptions:?}"
    );
}

#[test]
fn accepted_lead_still_captures_like_hand_typed_text() {
    let temp = TempDir::new("bob-cli-complete-task-types-lead-write");
    let (vault, config) = leads_fixture(temp.path());
    let day_file = vault.join("2026/20261030.md");

    // A marker alone still needs a task: the write path never sees leads.
    let mut refused = bob_command();
    refused
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("#fix ")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", NOW);
    let refused = refused.output().expect("run refused dry-run capture");
    assert!(!refused.status.success(), "{}", format_output(&refused));
    let json: Value = serde_json::from_str(stdout(&refused).trim())
        .unwrap_or_else(|error| {
            panic!(
                "parse capture error JSON: {error}\n{}",
                format_output(&refused)
            )
        });
    assert_eq!(json["code"], "type_marker_needs_task", "{json}");

    // The simulated accept parses exactly like hand-typed text.
    let mut parsed = bob_command();
    parsed
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("#fix Fix ")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW);
    let parsed = parsed.output().expect("run capture-parse");
    assert_success(&parsed);
    let json: Value = serde_json::from_str(stdout(&parsed).trim())
        .unwrap_or_else(|error| {
            panic!(
                "parse capture-parse JSON: {error}\n{}",
                format_output(&parsed)
            )
        });
    assert_eq!(json["mode"], "task", "{json}");
    assert_eq!(json["body"], "Fix", "{json}");
}

#[test]
fn human_task_type_rows_preview_the_lead() {
    let temp = TempDir::new("bob-cli-complete-task-types-lead-human");
    let (vault, config) = leads_fixture(temp.path());
    let output = bob_command()
        .args([
            "capture-complete",
            "-b",
            vault.to_str().expect("UTF-8 fixture path"),
            "-c",
            "2",
            "-f",
            "human",
            "--",
            "#f",
        ])
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", vault.join("2026/20261030.md"))
        .env("BOB_CONFIG_FILE", &config)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run human task-type capture completion");
    assert_success(&output);
    assert!(
        stdout(&output).contains("· types \"Fix\""),
        "human row previews the lead:\n{}",
        format_output(&output)
    );
}

#[test]
fn type_marker_inside_task_wikilink_owns_the_caret() {
    let temp = TempDir::new("bob-cli-complete-task-types-wikilink");
    let (vault, config) = fixture(temp.path(), true);
    let draft = "Follow up ((foo #f";
    let at_type = complete(temp.path(), &vault, &config, draft, draft.len());
    assert_eq!(at_type["context"], "task_type", "{at_type}");
    assert_eq!(at_type["query"], "f", "{at_type}");
    let start = draft.rfind("#").expect("type marker");
    assert_eq!(
        at_type["replacement"],
        serde_json::json!({"start": start, "end": draft.len()}),
        "{at_type}"
    );

    let opener = draft.find("((").expect("wikilink opener");
    let at_opener = complete(temp.path(), &vault, &config, draft, opener);
    assert_eq!(at_opener["context"], "wikilink_task", "{at_opener}");
    assert_eq!(at_opener["query"], "", "{at_opener}");
    assert_eq!(
        at_opener["replacement"],
        serde_json::json!({"start": opener, "end": opener + 5}),
        "{at_opener}"
    );

    let inside_query =
        complete(temp.path(), &vault, &config, draft, opener + 4);
    assert_eq!(inside_query["context"], "wikilink_task", "{inside_query}");
    assert_eq!(inside_query["query"], "fo", "{inside_query}");
}
