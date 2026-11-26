//! Bank hub task-home refusal (`bank_holds_no_tasks`).

use crate::support::*;
use std::fs;

const NOW: &str = "2026-11-12T12:00:00";

const HUB: &str = "---\nparent: \"[[cash]]\"\nbank:\n  accounts:\n    checking:\n      label: WF Checking\n      kind: checking\n      start: 2026-11-12\n  categories:\n    groceries: Groceries\n---\n\n# Bank\n";

fn bank_vault(temp: &TempDir, hub: &str) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("vault");
    write_file(&vault.join("area.md"), "# Areas\n");
    write_file(
        &vault.join("cash.md"),
        "---\ntype: \"[[area]]\"\nparent: \"[[area]]\"\n---\n\n# Cash\n",
    );
    write_file(&vault.join("bank.md"), hub);
    vault
}

fn capture_json(
    vault: &std::path::Path,
    args: &[&str],
) -> (std::process::Output, serde_json::Value) {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .expect("capture");
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    (output, json)
}

#[test]
fn bank_task_home_refuses_and_writes_nothing() {
    let temp = TempDir::new("bob-cli-capture-bank-hub");
    let vault = bank_vault(&temp, HUB);
    let before = fs::read_to_string(vault.join("bank.md")).expect("hub");
    let (output, json) = capture_json(&vault, &["Buy stamps @bank"]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json["code"], "bank_holds_no_tasks");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("holds no tasks"),
        "{json}"
    );
    assert_eq!(
        fs::read_to_string(vault.join("bank.md")).expect("hub unchanged"),
        before
    );
}

#[test]
fn bank_dry_run_and_parse_report_the_same_code() {
    let temp = TempDir::new("bob-cli-capture-bank-dry-parse");
    let vault = bank_vault(&temp, HUB);
    let (output, json) = capture_json(&vault, &["-d", "Buy stamps @bank"]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json["code"], "bank_holds_no_tasks");

    let parse = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .arg("--")
        .arg("Buy stamps @bank")
        .output()
        .expect("capture-parse");
    let value: serde_json::Value = serde_json::from_str(stdout(&parse).trim())
        .unwrap_or_else(|error| {
            panic!("parse JSON: {error}\n{}", format_output(&parse))
        });
    let diagnostics = value["diagnostics"].as_array().unwrap();
    let refusal = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == "bank_holds_no_tasks")
        .expect("bank preview diagnostic");
    assert!(
        refusal["message"]
            .as_str()
            .unwrap_or_default()
            .contains("holds no tasks"),
        "{refusal}"
    );
}

#[test]
fn area_typed_hub_is_still_refused() {
    let temp = TempDir::new("bob-cli-capture-bank-typed");
    let typed = HUB.replace(
        "parent: \"[[cash]]\"\nbank:",
        "parent: \"[[cash]]\"\ntype: \"[[area]]\"\nbank:",
    );
    let vault = bank_vault(&temp, &typed);
    let (output, json) = capture_json(&vault, &["Buy stamps @bank"]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json["code"], "bank_holds_no_tasks");
}
