//! Record-language CLI tests (phase `record-language`).
//!
//! Claimed record notes refuse fail-closed through `bob capture`
//! (never creating an inbox task) and report `record_note` through
//! `bob capture-parse`. Creation itself lands with `record-writes`.

use crate::support::*;
use std::fs;

const NOW: &str = "2026-10-07 13:40:00";

const RECORDS_CONFIG: &str = r#"records:
  restaurant:
    dir: eat
    label: Restaurant
    fields:
      status: {required: true}
      location: {}
"#;

fn capture(
    vault: &std::path::Path,
    config: &std::path::Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command.env("BOB_NOW", NOW);
    command.env("BOB_CONFIG_FILE", config);
    command.arg("capture").arg("-b").arg(vault);
    for arg in args {
        command.arg(arg);
    }
    command
}

fn write_records_config(temp: &TempDir) -> std::path::PathBuf {
    let config = temp.path().join("config.yml");
    write_file(&config, RECORDS_CONFIG);
    config
}

#[test]
fn capture_record_creation_refuses_fail_closed() {
    let temp = TempDir::new("bob-cli-capture-record-fail-closed");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = write_records_config(&temp);

    let output = capture(&vault, &config, &["-f", "json", "Ramen Nagi @eat/"])
        .output()
        .expect("run record capture");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("record creation is not wired yet"),
        "temporary refusal message: {json}"
    );
    // Nothing was staged or written: no inbox note, no record folder.
    assert!(!vault.join("mac_inbox.md").exists(), "no inbox task");
    assert!(!vault.join("eat").exists(), "no record folder");
    assert!(!vault.join("eat/ramen-nagi.md").exists(), "no record note");
}

#[test]
fn capture_record_human_dry_run_refuses_without_writes() {
    let temp = TempDir::new("bob-cli-capture-record-dry");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = write_records_config(&temp);

    let output = capture(&vault, &config, &["--dry-run", "@eat/ Ramen Nagi"])
        .output()
        .expect("run record dry run");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(!vault.join("mac_inbox.md").exists(), "no inbox task");
}

#[test]
fn capture_parse_reports_record_note_wire() {
    let temp = TempDir::new("bob-cli-capture-record-parse");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let config = write_records_config(&temp);

    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_DIR", &vault)
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@eat/ Ramen Nagi")
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["grammar"], 20);
    assert_eq!(json["mode"], "record_note");
    assert_eq!(json["route"], "eat");
    assert_eq!(json["body"], "Ramen Nagi");
    let kinds: Vec<&str> = json["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .map(|span| span["kind"].as_str().unwrap_or_default())
        .collect();
    assert!(kinds.contains(&"record_route"), "{kinds:?}");
}
