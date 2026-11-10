//! Notebook creation through the guarded capture transaction.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-11-09 12:00:00";
const DATE: &str = "2026-11-09";
const CREATED: &str = "2026-11-09T12:00:00+0000";

fn vault_with_area(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: \"[[area]]\"\n---\n# Dev\n",
    );
    write_file(
        &vault.join("dev_tools.md"),
        "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n# Developer tools\n\n## Git\n\n- Existing note [created::2026-11-08] ^existing\n",
    );
    vault
}

fn run_capture(
    vault: &std::path::Path,
    draft: &str,
    extra: &[&str],
    write: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if !write {
        command.arg("--dry-run");
    }
    for arg in extra {
        command.arg(arg);
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC0");
    command.output().expect("run capture")
}

fn dry_json(vault: &std::path::Path, draft: &str) -> Value {
    let output = run_capture(vault, draft, &[], false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn refusal(vault: &std::path::Path, draft: &str) -> Value {
    let output = run_capture(vault, draft, &[], false);
    assert_eq!(output.status.code(), Some(2));
    serde_json::from_str(stdout(&output).trim()).expect("refusal JSON")
}

#[test]
fn test_create_under_area_writes_notebook_and_catalog() {
    let temp = TempDir::new("bob-cli-capture-notebook-create");
    let vault = vault_with_area(&temp);
    let draft = "Neovim config notes @dev^nvim@ %n";
    let json = dry_json(&vault, draft);
    assert_eq!(json["kind"], "notebook_create");
    assert_eq!(json["placement"], "created");
    assert_eq!(json["relative_target"], "dev_nvim.md");
    assert_eq!(json["notebook"]["route"], "dev_nvim");
    assert_eq!(json["notebook"]["title"], "Nvim");
    assert_eq!(json["notebook"]["breadcrumb"][0], "dev");
    let files = json["note_files"].as_array().expect("note_files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "dev_nvim.md");
    assert_eq!(files[0]["action"], "create");
    assert!(files[0].get("record_type").is_none());
    let markdown = files[0]["markdown"].as_str().expect("markdown");
    assert!(markdown.contains("parent: \"[[dev]]\""));
    assert!(markdown.contains("type: \"[[notebook]]\""));
    assert!(markdown.contains("summary: Neovim config notes"));
    assert!(markdown.contains(&format!("created: {CREATED}")));
    assert!(markdown.contains("# Nvim\n"));
    assert!(!vault.join("dev_nvim.md").exists());

    let output = run_capture(&vault, draft, &[], true);
    assert_success(&output);
    let committed: Value =
        serde_json::from_str(stdout(&output).trim()).expect("commit JSON");
    assert_eq!(
        committed["note_files"][0]["markdown"],
        json["note_files"][0]["markdown"]
    );
    let created = fs::read_to_string(vault.join("dev_nvim.md")).expect("read");
    assert_eq!(created, markdown);
    let host = fs::read_to_string(vault.join("dev.md")).expect("read host");
    assert!(host.contains("> - **[[dev_nvim|Nvim]]** — Neovim config notes"));
}

#[test]
fn test_seed_sections_and_later_same_draft_notes() {
    let temp = TempDir::new("bob-cli-capture-notebook-create-seed");
    let vault = vault_with_area(&temp);
    let draft = "Neovim config notes @dev^nvim@ %n\n- KEYBINDS\n  - leader is space\n\nA later note @dev_nvim";
    let json = dry_json(&vault, draft);
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures[0]["kind"], "notebook_create");
    assert_eq!(captures[1]["kind"], "notebook_note");
    assert_eq!(captures[1]["notebook"]["route"], "dev_nvim");
    let markdown = json["note_files"][0]["markdown"]
        .as_str()
        .expect("markdown");
    assert!(markdown.contains("## Keybinds"));
    assert!(markdown.contains(&format!("- leader is space [created::{DATE}]")));
    assert!(markdown.contains(&format!("- A later note [created::{DATE}]")));

    let output = run_capture(&vault, draft, &[], true);
    assert_success(&output);
    let committed: Value =
        serde_json::from_str(stdout(&output).trim()).expect("commit JSON");
    assert_eq!(
        committed["note_files"][0]["markdown"],
        json["note_files"][0]["markdown"]
    );
    let created = fs::read_to_string(vault.join("dev_nvim.md")).expect("read");
    assert_eq!(created, markdown);
}

#[test]
fn test_empty_summary_refuses_at_plan() {
    let temp = TempDir::new("bob-cli-capture-notebook-create-summary");
    let vault = vault_with_area(&temp);
    let json = refusal(&vault, "@dev^nvim@ %n");
    assert_eq!(json["code"], "notebook_summary_required");
}

#[test]
fn test_used_before_created_refuses() {
    let temp = TempDir::new("bob-cli-capture-notebook-create-order");
    let vault = vault_with_area(&temp);
    let json = refusal(
        &vault,
        "A note @dev_nvim\n\nNeovim config notes @dev^nvim@ %n",
    );
    assert_eq!(json["code"], "notebook_used_before_created");
}

#[test]
fn test_area_seal_without_n_stays_project_note() {
    let temp = TempDir::new("bob-cli-capture-notebook-create-project");
    let vault = vault_with_area(&temp);
    let json = dry_json(&vault, "A project @dev^nvim@");
    assert_eq!(json["kind"], "project_note");
}

#[test]
fn test_notebook_seal_without_n_stays_project_seal() {
    let temp = TempDir::new("bob-cli-capture-notebook-create-seal");
    let vault = vault_with_area(&temp);
    let json = refusal(&vault, "A note @dev_tools^foo@");
    assert_eq!(json["code"], "notebook_project_seal");
}

#[test]
fn test_colon_seal_plus_n_stays_conflict() {
    let temp = TempDir::new("bob-cli-capture-notebook-create-colon");
    let vault = vault_with_area(&temp);
    let json = refusal(&vault, "Keep @dev_tools:nvim@ %n");
    assert_eq!(json["code"], "pomodoro_note_conflict");
}
