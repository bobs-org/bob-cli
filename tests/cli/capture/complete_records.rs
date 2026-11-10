//! Public JSON for record field/value completion and section create rows.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Output;

const NOW: &str = "2026-10-07 13:40:00";

const RECORDS_CONFIG: &str = r#"records:
  restaurant:
    dir: eat
    label: Restaurant
    emoji: "🍽️"
    dated_notes: true
    fields:
      status:
        required: true
        values:
          to_try: "To try"
          liked: "Liked"
      category:
        required: true
        suggest: true
      location:
        required: true
        suggest: true
      visited:
        values: date
      companions:
        values: link
        list: true
        default: ["[[kelly]]"]
"#;

fn write_records_config(temp: &TempDir) -> std::path::PathBuf {
    let config = temp.path().join("config.yml");
    write_file(&config, RECORDS_CONFIG);
    config
}

fn fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new("bob-cli-capture-complete-records");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("eat")).expect("create eat");
    let config = write_records_config(&temp);
    write_file(
        &vault.join("eat/nagi.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: Nagi\nstatus: liked\ncategory: Ramen\nlocation: Clark\n---\n# Nagi\n",
    );
    write_file(
        &vault.join("eat/west_1.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: West 1\nstatus: liked\ncategory: Pizza\nlocation: Westfield\n---\n# West 1\n",
    );
    write_file(
        &vault.join("eat/west_2.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: West 2\nstatus: liked\ncategory: Pizza\nlocation: Westfield\n---\n# West 2\n",
    );
    write_file(
        &vault.join("eat/west_link.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: West Link\nstatus: liked\ncategory: Pizza\n\
         location: \"[[nj_towns#^town|westfield]]\"\n---\n# West Link\n",
    );
    write_file(
        &vault.join("eat/cheese_1.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: Cheese 1\nstatus: liked\ncategory: Cheesesteaks\nlocation: Clark\n---\n# Cheese 1\n",
    );
    write_file(
        &vault.join("eat/cheese_2.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: Cheese 2\nstatus: liked\ncategory: Cheesesteaks\nlocation: Clark\n---\n# Cheese 2\n",
    );
    write_file(
        &vault.join("eat/cheese_3.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: Cheese 3\nstatus: liked\ncategory: Cheesesteaks\nlocation: Clark\n---\n# Cheese 3\n",
    );
    write_file(&vault.join("eat.md"), "# Ideas\n## Tasks\n");
    (temp, vault, config)
}

fn complete(
    vault: &Path,
    config: &Path,
    cursor: usize,
    draft: &str,
    extra: &[&str],
) -> Output {
    let mut command = bob_command();
    command
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", config)
        .env("NO_COLOR", "1")
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json");
    for arg in extra {
        command.arg(arg);
    }
    command.arg("--").arg(draft).output().expect("complete")
}

fn parse(vault: &Path, config: &Path, draft: &str) -> Output {
    bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_DIR", vault)
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("parse")
}

fn json_output(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("parse JSON ({error}):\n{}", format_output(output))
    })
}

fn apply(raw: &str, json: &Value, replacement: &str) -> String {
    let start = json["replacement"]["start"].as_u64().unwrap() as usize;
    let end = json["replacement"]["end"].as_u64().unwrap() as usize;
    format!("{}{}{}", &raw[..start], replacement, &raw[end..])
}

fn find_candidate<'a>(json: &'a Value, value: &str) -> &'a Value {
    json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|row| row["value"] == value)
        .unwrap_or_else(|| panic!("missing {value} in {}", json["candidates"]))
}

#[test]
fn capture_complete_record_route_offers_new_restaurant() {
    let (_temp, vault, config) = fixture();
    let output = complete(&vault, &config, 1, "@", &[]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["grammar"], 24);
    assert_eq!(json["context"], "route");
    let eat = json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|row| row["route"] == "eat/")
        .expect("eat/");
    assert_eq!(eat["kind"], "record");
    assert_eq!(eat["replacement"], "eat/ ");
    assert_eq!(eat["type"], "restaurant");
    assert_eq!(eat["emoji"], "🍽️");
    assert_eq!(eat["continuation"]["next_missing_field"], true);
    assert!(eat["label"]
        .as_str()
        .unwrap()
        .starts_with("🍽️ New restaurant · "));
}

#[test]
fn capture_complete_record_target_replaces_only_the_route() {
    let (_temp, vault, config) = fixture();
    let draft = "Went back @eat/nag\n- Sushi was good";
    let cursor = "Went back @eat/nag".len();
    let output = complete(&vault, &config, cursor, draft, &[]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["context"], "record_target", "{json:#}");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 10, "end": 18})
    );
    let target = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["stem"] == "nagi")
        .expect("Nagi target");
    assert_eq!(target["replacement"], "@eat/nagi");
    assert_eq!(target["title"], "Nagi");
    assert_eq!(target["status"], "Liked");
    assert_eq!(target["fields"][0]["key"], "category");
    assert_eq!(target["fields"][0]["value"], "Ramen");
    assert_eq!(target["fields"][1]["key"], "location");
    assert_eq!(target["fields"][1]["value"], "Clark");
    assert_eq!(target["continuation"]["next_missing_field"], true);

    let rewritten =
        apply(draft, &json, target["replacement"].as_str().unwrap());
    assert_eq!(rewritten, "Went back @eat/nagi\n- Sushi was good");
    let parsed = json_output(&parse(&vault, &config, &rewritten));
    assert_eq!(parsed["record"]["action"], "update");
    assert_eq!(parsed["record"]["notes"], 2);

    let explicit_new = "@eat/^nagi";
    let explicit =
        complete(&vault, &config, explicit_new.len(), explicit_new, &[]);
    assert_success(&explicit);
    assert_ne!(json_output(&explicit)["context"], "record_target");
}

#[test]
fn capture_complete_next_missing_walks_required_fields() {
    let (_temp, vault, config) = fixture();
    let mut draft = "Ramen Nagi @eat/".to_string();
    let status = complete(&vault, &config, draft.len(), &draft, &["-n"]);
    assert_success(&status);
    let status_json = json_output(&status);
    assert_eq!(status_json["context"], "record_value");
    assert_eq!(status_json["record"]["field"], "status");
    let liked = find_candidate(&status_json, "liked");
    assert_eq!(liked["display"], "Liked");
    assert_eq!(liked["continuation"]["next_missing_field"], true);
    draft = apply(&draft, &status_json, liked["replacement"].as_str().unwrap());
    assert_eq!(draft, "Ramen Nagi @eat/\n- status:: liked");

    let cursor = liked["continuation"]["cursor"].as_u64().unwrap() as usize;
    let category = complete(&vault, &config, cursor, &draft, &["-n"]);
    assert_success(&category);
    let category_json = json_output(&category);
    let ramen = find_candidate(&category_json, "Ramen");
    assert_eq!(ramen["continuation"]["next_missing_field"], true);
    draft = apply(
        &draft,
        &category_json,
        ramen["replacement"].as_str().unwrap(),
    );
    assert_eq!(
        draft,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen"
    );

    let cursor = ramen["continuation"]["cursor"].as_u64().unwrap() as usize;
    let location = complete(&vault, &config, cursor, &draft, &["-n"]);
    assert_success(&location);
    let location_json = json_output(&location);
    let clark = find_candidate(&location_json, "Clark");
    assert!(clark.get("continuation").is_none(), "{clark}");
    draft = apply(
        &draft,
        &location_json,
        clark["replacement"].as_str().unwrap(),
    );
    assert_eq!(
        draft,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark"
    );
}

#[test]
fn capture_complete_section_create_row_is_first_on_an_empty_query() {
    let (_temp, vault, config) = fixture();
    let draft = "@eat#";
    let output = complete(&vault, &config, draft.len(), draft, &[]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["context"], "section");
    assert_eq!(json["replacement"]["start"], 0);
    assert_eq!(json["replacement"]["end"], 5);
    let rows = json["candidates"].as_array().expect("candidates");
    assert_eq!(rows[0]["kind"], "record_create");
    assert_eq!(rows[0]["replacement"], "@eat/ ");
    assert_eq!(rows[0]["label"], "🍽️ New restaurant · eat/");
    assert_eq!(rows[0]["continuation"]["next_missing_field"], true);
    assert!(
        rows.iter().any(|row| row["replacement"] == "@eat#Ideas"),
        "{rows:?}"
    );
}

#[test]
fn capture_complete_westfield_collapses_the_wikilink_alias() {
    let (_temp, vault, config) = fixture();
    let draft = "Slice @eat/\n- location:: ";
    let output = complete(&vault, &config, draft.len(), draft, &[]);
    assert_success(&output);
    let json = json_output(&output);
    let westfield: Vec<&Value> = json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter(|row| {
            row["value"] == "Westfield"
                || row["display"].as_str().is_some_and(|display| {
                    display.eq_ignore_ascii_case("westfield")
                })
        })
        .collect();
    assert_eq!(westfield.len(), 1, "{}", json["candidates"]);
    assert_eq!(westfield[0]["value"], "Westfield");
    assert_eq!(westfield[0]["count"], 3);
}

#[test]
fn capture_parse_and_complete_share_the_near_value_hint() {
    let (_temp, vault, config) = fixture();
    let draft = "Pat @eat/\n- category:: Cheeseteaks";
    let parsed = parse(&vault, &config, draft);
    assert_success(&parsed);
    let parse_json = json_output(&parsed);
    let parse_hint = parse_json["diagnostics"]
        .as_array()
        .expect("parse diagnostics")
        .iter()
        .find(|diag| diag["code"] == "record_value_near")
        .unwrap_or_else(|| panic!("parse hint missing: {parse_json}"));
    let message = parse_hint["message"].as_str().expect("message");
    assert!(message.contains("Cheeseteaks"), "{message}");
    assert!(message.contains("Cheesesteaks"), "{message}");
    assert!(parse_hint.get("rewrite").is_none());

    let complete_out = complete(&vault, &config, draft.len(), draft, &[]);
    assert_success(&complete_out);
    let complete_json = json_output(&complete_out);
    let complete_hint = complete_json["diagnostics"]
        .as_array()
        .expect("complete diagnostics")
        .iter()
        .find(|diag| diag["code"] == "record_value_near")
        .expect("complete hint");
    assert_eq!(complete_hint["authored"], "Cheeseteaks");
    assert_eq!(complete_hint["proposed"], "Cheesesteaks");
    assert_eq!(
        complete_hint["message"].as_str().expect("complete message"),
        message
    );
}

#[test]
fn capture_complete_next_missing_on_a_non_record_is_empty_success() {
    let (_temp, vault, config) = fixture();
    let draft = "Buy milk";
    let output = complete(&vault, &config, draft.len(), draft, &["-n"]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["ok"], true);
    assert!(json["context"].is_null(), "{json}");
    assert_eq!(json["candidates"], serde_json::json!([]));
}

fn apply_edit(raw: &str, json: &Value) -> String {
    let edit = &json["record"]["edit"];
    let start = edit["replacement_range"]["start"].as_u64().unwrap() as usize;
    let end = edit["replacement_range"]["end"].as_u64().unwrap() as usize;
    format!(
        "{}{}{}",
        &raw[..start],
        edit["replacement"].as_str().unwrap(),
        &raw[end..]
    )
}

#[test]
fn capture_complete_named_field_inserts_and_reparses() {
    let (_temp, vault, config) = fixture();
    let draft = "Ramen Nagi @eat/";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-F", "status"]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["context"], "record_value");
    assert_eq!(json["record"]["field"], "status");
    assert_eq!(json["record"]["assist_version"], 1);
    let liked = find_candidate(&json, "liked");
    assert_eq!(liked["continuation"]["next_missing_field"], true);
    let applied = apply(draft, &json, liked["replacement"].as_str().unwrap());
    assert_eq!(applied, "Ramen Nagi @eat/\n- status:: liked");
    let parsed = parse(&vault, &config, &applied);
    assert_success(&parsed);
    let parsed_json = json_output(&parsed);
    assert_eq!(parsed_json["record"]["assist_version"], 1);
    let status = parsed_json["record"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["key"] == "status")
        .expect("status");
    assert_eq!(status["state"], "set");
    assert_eq!(status["value"], "liked");
}

#[test]
fn capture_complete_named_field_edit_authors_empty_optional_and_reparses() {
    let (_temp, vault, config) = fixture();
    let draft = "Ramen Nagi @eat/\n- leftover note";
    let output = complete(
        &vault,
        &config,
        draft.len(),
        draft,
        &["--record-field", "visited"],
    );
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(
        json["record"]["edit"]["continuation"]["next_missing_field"],
        false
    );
    let applied = apply_edit(draft, &json);
    assert_eq!(applied, "Ramen Nagi @eat/\n- visited:: \n- leftover note");
    let parsed = parse(&vault, &config, &applied);
    assert_success(&parsed);
    let parsed_json = json_output(&parsed);
    let visited = parsed_json["record"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["key"] == "visited")
        .expect("visited");
    assert_eq!(visited["state"], "none");
}

#[test]
fn capture_complete_named_field_edit_materializes_default() {
    let (_temp, vault, config) = fixture();
    let draft = "Ramen Nagi @eat/";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-F", "companions"]);
    assert_success(&output);
    let json = json_output(&output);
    let applied = apply_edit(draft, &json);
    assert_eq!(applied, "Ramen Nagi @eat/\n- companions:: [[kelly]]");
    let parsed = parse(&vault, &config, &applied);
    assert_success(&parsed);
    let parsed_json = json_output(&parsed);
    let companions = parsed_json["record"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["key"] == "companions")
        .expect("companions");
    assert_eq!(companions["state"], "set");
    assert_eq!(companions["source"], "typed");
}

#[test]
fn capture_complete_named_field_conflicts_with_next_missing() {
    let (_temp, vault, config) = fixture();
    let draft = "Ramen Nagi @eat/";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-n", "-F", "status"]);
    assert_ne!(output.status.code(), Some(0));
    let err = format!("{}{}", stdout(&output), stderr(&output));
    assert!(err.contains("cannot be used with"), "{err}");
}

#[test]
fn capture_complete_named_field_on_a_non_record_is_empty_success() {
    let (_temp, vault, config) = fixture();
    let draft = "Buy milk";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-F", "status"]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["ok"], true);
    assert!(json["context"].is_null(), "{json}");
    assert!(json.get("record").is_none(), "{json}");
    assert_eq!(json["candidates"], serde_json::json!([]));
}

#[test]
fn capture_parse_record_advertises_assist_version() {
    let (_temp, vault, config) = fixture();
    let parsed = parse(&vault, &config, "Ramen Nagi @eat/");
    assert_success(&parsed);
    let json = json_output(&parsed);
    assert_eq!(json["record"]["assist_version"], 1);
    assert!(json.get("assist_version").is_none());
}

#[test]
fn record_target_skips_grammar_unaddressable_stems() {
    let (_temp, vault, config) = fixture();
    write_file(
        &vault.join("eat/Ramen Nagi.md"),
        "---\ntitle: Ramen Nagi\nstatus: liked\ncategory: Ramen\nlocation: Clark\n---\n# Ramen Nagi\n",
    );
    let draft = "@eat/";
    let output = complete(&vault, &config, draft.len(), draft, &[]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["context"], "record_target", "{json}");
    let stems: Vec<&str> = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["stem"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !stems.contains(&"Ramen Nagi"),
        "spaced stem is not offered: {stems:?}"
    );
    assert!(
        stems.contains(&"nagi"),
        "addressable stems remain: {stems:?}"
    );
    // Row keys required by the landing plan.
    let row = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["stem"] == "nagi")
        .expect("nagi row");
    for key in [
        "route",
        "stem",
        "title",
        "replacement",
        "cursor_after",
        "continuation",
    ] {
        assert!(row.get(key).is_some(), "row has {key}: {row}");
    }
    assert!(row.get("status").is_some(), "row has status: {row}");
    assert!(row.get("fields").is_some(), "row has fields: {row}");
}

#[test]
fn named_field_on_update_prefills_current_value() {
    let (_temp, vault, config) = fixture();
    let draft = "@eat/nagi";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-F", "location"]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["record"]["field"], "location");
    assert_eq!(json["record"]["current"], "Clark");
    let edit = json["record"]["edit"].as_object().expect("record.edit");
    let replacement = edit["replacement"].as_str().unwrap_or_default();
    assert!(
        replacement.contains("- location:: Clark"),
        "pre-filled current: {replacement}"
    );
    let applied = apply_edit(draft, &json);
    assert_eq!(applied, "@eat/nagi\n- location:: Clark");
}

#[test]
fn named_field_on_update_without_current_keeps_empty_insert() {
    let (_temp, vault, config) = fixture();
    write_file(
        &vault.join("eat/bare.md"),
        "---\ntitle: Bare\nstatus: liked\ncategory: Ramen\n---\n# Bare\n",
    );
    let draft = "@eat/bare";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-F", "location"]);
    assert_success(&output);
    let json = json_output(&output);
    let replacement = json["record"]["edit"]["replacement"]
        .as_str()
        .unwrap_or_default();
    assert!(
        replacement.contains("- location:: "),
        "empty insert when no current: {replacement}"
    );
}

#[test]
fn named_field_on_update_prefills_list_and_authored_bullet() {
    let (_temp, vault, config) = fixture();
    write_file(
        &vault.join("eat/nagi.md"),
        "---\ntitle: Nagi\nstatus: liked\ncategory: Ramen\nlocation: Clark\ncompanions: [\"[[kelly]]\", \"[[morgan]]\"]\n---\n# Nagi\n",
    );
    let draft = "@eat/nagi";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-F", "companions"]);
    assert_success(&output);
    let json = json_output(&output);
    let replacement = json["record"]["edit"]["replacement"]
        .as_str()
        .unwrap_or_default();
    assert!(
        replacement.contains("[[kelly]]") && replacement.contains("[[morgan]]"),
        "list pre-filled: {replacement}"
    );
    // Already-authored bullet keeps the caret path (no insert).
    let authored = "@eat/nagi\n- location:: Cla";
    let output = complete(
        &vault,
        &config,
        authored.len(),
        authored,
        &["-F", "location"],
    );
    assert_success(&output);
    let json = json_output(&output);
    assert!(
        json["record"]["edit"].is_object(),
        "authored keeps edit: {json}"
    );
}

#[test]
fn named_field_on_update_with_comma_member_omits_edit() {
    let (_temp, vault, config) = fixture();
    write_file(
        &vault.join("eat/nagi.md"),
        "---\ntitle: Nagi\nstatus: liked\ncategory: Ramen\nlocation: Clark\ncompanions: [\"[[a,b]]\"]\n---\n# Nagi\n",
    );
    let draft = "@eat/nagi";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-F", "companions"]);
    assert_success(&output);
    let json = json_output(&output);
    assert!(
        json["record"]["edit"].is_null(),
        "comma member cannot be spelled losslessly: {json}"
    );
}

#[test]
fn update_completion_title_is_stored_not_parent_note() {
    let (_temp, vault, config) = fixture();
    let draft = "Went back @eat/nagi";
    let output =
        complete(&vault, &config, draft.len(), draft, &["-F", "location"]);
    assert_success(&output);
    let json = json_output(&output);
    assert_eq!(json["record"]["title"], "Nagi", "stored title: {json}");
}
