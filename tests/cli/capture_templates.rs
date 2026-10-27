use crate::support::*;
use serde_json::Value;
use std::{fs, process::Command};

const DEFINITION: &str =
    "@@template:\nApply to {0}! @job^apply-{1}\n- **URL**: {2}\n\n";

fn command(file: &std::path::Path) -> Command {
    let mut command = bob_command();
    command
        .arg("capture-templates")
        .env("BOB_CAPTURE_TEMPLATES_FILE", file)
        .env("BOB_NOW", "2026-10-16 09:40:02");
    command
}

fn json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("stdout should be JSON: {error}\n{}", format_output(output))
    })
}

#[test]
fn list_save_show_and_delete_follow_the_template_protocol() {
    let temp = TempDir::new("bob-cli-capture-templates-protocol");
    let file = temp.path().join("capture/templates.json");

    let listed = command(&file)
        .args(["-f", "json"])
        .output()
        .expect("run empty template list");
    assert_success(&listed);
    let listed_json = json(&listed);
    assert_eq!(listed_json["ok"], true);
    assert_eq!(listed_json["capacity"], 36);
    assert_eq!(listed_json["saving"], true);
    assert_eq!(listed_json["templates"].as_array().unwrap().len(), 0);
    assert!(!file.exists(), "list must not create the shelf file");

    let saved =
        run_with_stdin(command(&file).args(["save", "-f", "json"]), DEFINITION);
    assert_success(&saved);
    let saved_json = json(&saved);
    assert_eq!(saved_json["saved"]["status"], "new");
    assert_eq!(
        saved_json["template"]["text"],
        "Apply to {0}! @job^apply-{1}\n- **URL**: {2}"
    );
    assert_eq!(
        saved_json["template"]["draft"],
        "@@template:\nApply to {0}! @job^apply-{1}\n- **URL**: {2}\n\n"
    );
    assert_eq!(
        saved_json["template"]["cursor"],
        saved_json["template"]["draft"].as_str().unwrap().len()
    );
    assert_eq!(saved_json["template"]["field_count"], 3);
    assert_eq!(saved_json["template"]["route"], "job");
    assert_eq!(saved_json["template"]["use_count"], 0);
    assert!(saved_json["template"]["spans"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |span| span["kind"] == "template_placeholder" && span["field"] == 0
        ));
    let id = saved_json["saved"]["id"].as_str().unwrap();
    let first_saved_at = saved_json["template"]["first_saved_at"].clone();

    let shown = command(&file)
        .args(["show", id])
        .output()
        .expect("show template draft");
    assert_success(&shown);
    assert_eq!(
        stdout(&shown),
        "@@template:\nApply to {0}! @job^apply-{1}\n- **URL**: {2}\n\n"
    );

    let edited_definition =
        "@@template:\nApply for {0}! @job^apply-{1}\n\ninvalid row arity is ignored\n";
    let replaced = run_with_stdin(
        command(&file).args(["save", "--template-origin", id, "-f", "json"]),
        edited_definition,
    );
    assert_success(&replaced);
    let replaced_json = json(&replaced);
    assert_eq!(replaced_json["saved"]["status"], "replaced");
    assert_ne!(replaced_json["saved"]["id"], id);
    assert_eq!(replaced_json["template"]["use_count"], 0);
    assert_eq!(replaced_json["template"]["first_saved_at"], first_saved_at);

    let deleted = command(&file)
        .args(["delete", "1", "missing", "-f", "json"])
        .output()
        .expect("delete template");
    assert_success(&deleted);
    let deleted_json = json(&deleted);
    assert_eq!(deleted_json["deleted"].as_array().unwrap().len(), 1);
    assert_eq!(deleted_json["missing"], serde_json::json!(["missing"]));
    assert!(
        !file.exists(),
        "deleting the last template removes the shelf"
    );
}

#[test]
fn definition_errors_and_newer_schema_have_stable_recovery_paths() {
    let temp = TempDir::new("bob-cli-capture-templates-errors");
    let file = temp.path().join("templates.json");
    let invalid = run_with_stdin(
        command(&file).args(["save", "-f", "json"]),
        "@@template:\nA task\n\n",
    );
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(json(&invalid)["code"], "template_missing_placeholder");

    fs::write(
        &file,
        r#"{"schema_version":2,"templates":[{"id":"abc123","text":"Task {0}"}]}"#,
    )
    .expect("write newer shelf schema");
    let listed = command(&file)
        .args(["-f", "json"])
        .output()
        .expect("read newer shelf schema");
    assert_success(&listed);
    assert_eq!(json(&listed)["templates"].as_array().unwrap().len(), 1);
    assert!(!json(&listed)["warnings"].as_array().unwrap().is_empty());

    let save = command(&file)
        .arg("save")
        .args(["-f", "json"])
        .arg("@@template:\nTask {0}\n\n")
        .output()
        .expect("attempt newer-schema save");
    assert_eq!(save.status.code(), Some(1));
    assert_eq!(json(&save)["code"], "saved_templates_newer_schema");

    let cleared = command(&file)
        .args(["clear", "-f", "json"])
        .output()
        .expect("clear newer shelf schema");
    assert_success(&cleared);
    assert!(!file.exists());

    let disabled_file = temp.path().join("disabled.json");
    let config = temp.path().join("config.yml");
    fs::write(&config, "capture:\n  max_saved_templates: 0\n")
        .expect("write disabled shelf config");
    let disabled = run_with_stdin(
        command(&disabled_file)
            .arg("save")
            .args(["-f", "json"])
            .env("BOB_CONFIG_FILE", config),
        DEFINITION,
    );
    assert_eq!(disabled.status.code(), Some(2));
    assert_eq!(json(&disabled)["code"], "saved_templates_disabled");
    assert!(!disabled_file.exists());
}

#[test]
fn bare_delete_is_a_usage_error_and_leaves_the_store_absent() {
    let temp = TempDir::new("bob-cli-capture-templates-bare-delete");
    let file = temp.path().join("templates.json");
    let output = command(&file)
        .args(["delete", "-f", "json"])
        .output()
        .expect("run bare delete");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(!file.exists(), "bare delete must not create the store");
}

#[test]
fn newer_schema_with_changed_field_type_stays_readable_and_refuses_writes() {
    let temp = TempDir::new("bob-cli-capture-templates-newer-types");
    let file = temp.path().join("templates.json");
    let bytes = br#"{"schema_version": 2, "templates": [{"id": "9c1f0e2a7b3d", "text": "Task {0}", "use_count": 1.5, "name": "future"}]}"#;
    fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    fs::write(&file, bytes).expect("write newer schema with bad type");

    let listed = command(&file)
        .args(["list", "-f", "json"])
        .output()
        .expect("list newer schema with bad type");
    assert_success(&listed);
    let listed_json = json(&listed);
    assert_eq!(listed_json["templates"].as_array().unwrap().len(), 1);
    assert_eq!(listed_json["templates"][0]["id"], "9c1f0e2a7b3d");
    assert!(!listed_json["warnings"].as_array().unwrap().is_empty());
    assert!(
        stderr(&listed).contains("newer schema"),
        "{}",
        format_output(&listed)
    );

    let save = command(&file)
        .arg("save")
        .args(["-f", "json"])
        .arg("@@template:\nTask {0}\n\n")
        .output()
        .expect("save against newer schema");
    assert_eq!(save.status.code(), Some(1));
    assert_eq!(json(&save)["code"], "saved_templates_newer_schema");
    assert_eq!(
        fs::read(&file).expect("read after refused save"),
        bytes,
        "refused save must leave the bytes untouched"
    );

    let cleared = command(&file)
        .args(["clear", "-f", "json"])
        .output()
        .expect("clear newer schema");
    assert_success(&cleared);
    assert!(!file.exists());
}

#[test]
fn human_list_covers_empty_populated_and_saving_off() {
    let temp = TempDir::new("bob-cli-capture-templates-human");
    let file = temp.path().join("templates.json");

    let empty = command(&file)
        .env("NO_COLOR", "1")
        .arg("list")
        .output()
        .expect("human empty list");
    assert_success(&empty);
    assert!(
        stdout(&empty).contains("No saved templates yet"),
        "{}",
        format_output(&empty)
    );

    let saved =
        run_with_stdin(command(&file).args(["save", "-f", "json"]), DEFINITION);
    assert_success(&saved);
    let id = json(&saved)["saved"]["id"]
        .as_str()
        .expect("saved id")
        .to_string();

    let populated = command(&file)
        .env("NO_COLOR", "1")
        .arg("list")
        .output()
        .expect("human populated list");
    assert_success(&populated);
    let text = stdout(&populated);
    assert!(
        text.contains("1  Apply to"),
        "{}",
        format_output(&populated)
    );
    assert!(text.contains(&id), "{}", format_output(&populated));
    assert!(
        !text.contains("\u{1b}["),
        "NO_COLOR output must have no escapes: {}",
        format_output(&populated)
    );

    let config = temp.path().join("config.yml");
    fs::write(&config, "capture:\n  max_saved_templates: 0\n")
        .expect("write off config");
    let off = command(&file)
        .env("NO_COLOR", "1")
        .env("BOB_CONFIG_FILE", &config)
        .arg("list")
        .output()
        .expect("human list with saving off");
    assert_success(&off);
    assert!(
        stdout(&off).contains("saving is off"),
        "{}",
        format_output(&off)
    );
}

#[test]
fn show_by_index_prints_the_insertable_draft() {
    let temp = TempDir::new("bob-cli-capture-templates-show-index");
    let file = temp.path().join("templates.json");
    let saved =
        run_with_stdin(command(&file).args(["save", "-f", "json"]), DEFINITION);
    assert_success(&saved);
    let shown = command(&file)
        .args(["show", "1"])
        .output()
        .expect("show by index");
    assert_success(&shown);
    assert_eq!(
        stdout(&shown),
        "@@template:\nApply to {0}! @job^apply-{1}\n- **URL**: {2}\n\n"
    );
}

#[test]
fn save_of_headerless_draft_is_not_a_template() {
    let temp = TempDir::new("bob-cli-capture-templates-headerless");
    let file = temp.path().join("templates.json");
    let output = run_with_stdin(
        command(&file).args(["save", "-f", "json"]),
        "just a task\n",
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json(&output)["code"], "saved_template_not_template");
}

#[test]
fn clear_also_removes_the_corrupt_quarantine() {
    let temp = TempDir::new("bob-cli-capture-templates-clear-quarantine");
    let file = temp.path().join("templates.json");
    let saved =
        run_with_stdin(command(&file).args(["save", "-f", "json"]), DEFINITION);
    assert_success(&saved);
    let quarantine = temp.path().join("templates.corrupt.json");
    fs::write(&quarantine, b"old quarantine").expect("write quarantine");
    let cleared = command(&file)
        .args(["clear", "-f", "json"])
        .output()
        .expect("clear shelf");
    assert_success(&cleared);
    assert!(!file.exists());
    assert!(
        !quarantine.exists(),
        "clear must remove the quarantine copy"
    );
}
