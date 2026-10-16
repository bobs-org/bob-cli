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

    let saved = run_with_stdin(
        &mut command(&file).args(["save", "-f", "json"]),
        DEFINITION,
    );
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
        &mut command(&file).args([
            "save",
            "--template-origin",
            id,
            "-f",
            "json",
        ]),
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
        &mut command(&file).args(["save", "-f", "json"]),
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
        &mut command(&disabled_file)
            .arg("save")
            .args(["-f", "json"])
            .env("BOB_CONFIG_FILE", config),
        DEFINITION,
    );
    assert_eq!(disabled.status.code(), Some(2));
    assert_eq!(json(&disabled)["code"], "saved_templates_disabled");
    assert!(!disabled_file.exists());
}
