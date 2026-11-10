//! Notebook note writes through the guarded capture transaction.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-11-09 12:00:00";
const DATE: &str = "2026-11-09";

fn vault_with_notebook(temp: &TempDir) -> std::path::PathBuf {
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

fn dry_single(vault: &std::path::Path, draft: &str) -> Value {
    let output = run_capture(vault, draft, &[], false);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn read_notebook(vault: &std::path::Path) -> String {
    fs::read_to_string(vault.join("dev_tools.md")).expect("read notebook")
}

#[test]
fn test_top_level_note_appends_before_first_h2() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let before = read_notebook(&vault);
    let json = dry_single(&vault, "Try mergiraf @dev_tools");
    assert_eq!(json["kind"], "notebook_note");
    assert_eq!(json["notebook"]["route"], "dev_tools");
    assert_eq!(json["notebook"]["title"], "Developer tools");
    assert_eq!(json["notebook"]["section"], Value::Null);
    assert_eq!(
        json["task_line"],
        format!("- Try mergiraf [created::{DATE}]")
    );
    // Dry run preserves bytes.
    assert_eq!(read_notebook(&vault), before);
    let output = run_capture(&vault, "Try mergiraf @dev_tools", &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(
        after.contains(&format!("- Try mergiraf [created::{DATE}]\n\n## Git"))
    );
}

#[test]
fn test_section_exact_and_near_miss_and_create() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let json = dry_single(&vault, "Try mergiraf @dev_tools#git");
    assert_eq!(json["notebook"]["section"], "Git");
    assert_eq!(json["notebook"]["section_action"], "existing");
    let output = run_capture(&vault, "Try mergiraf @dev_tools#gi", &[], false);
    assert_eq!(output.status.code(), Some(2));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("refusal JSON");
    assert_eq!(value["code"], "notebook_section_near_miss");
    let json = dry_single(&vault, "Try mergiraf @dev_tools#merge-tools");
    assert_eq!(json["notebook"]["section"], "Merge Tools");
    assert_eq!(json["notebook"]["section_action"], "create");
    let output =
        run_capture(&vault, "Try mergiraf @dev_tools#merge-tools", &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains("## Merge Tools"));
    assert!(after.contains(&format!("- Try mergiraf [created::{DATE}]")));
}

#[test]
fn test_named_note_and_section_named_note() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let json = dry_single(&vault, "Try mergiraf @dev_tools^mergiraf");
    assert_eq!(json["notebook"]["block_id"], "mergiraf");
    assert_eq!(
        json["task_line"],
        format!("- Try mergiraf [created::{DATE}] ^mergiraf")
    );
    let json = dry_single(&vault, "Try mergiraf @dev_tools#git^mergiraf");
    assert_eq!(json["notebook"]["section"], "Git");
    assert_eq!(json["notebook"]["block_id"], "mergiraf");
}

#[test]
fn test_sub_note_appends_unstamped_after_parent() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let json = dry_single(&vault, "Faster with -j @dev_tools+existing");
    assert_eq!(json["kind"], "notebook_sub_note");
    assert_eq!(json["notebook"]["parent_block_id"], "existing");
    let output =
        run_capture(&vault, "Faster with -j @dev_tools+existing", &[], true);
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains("- Faster with -j"));
    assert!(!after.contains("Faster with -j [created::"));
}

#[test]
fn test_forced_route_section_and_task() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let output = run_capture(&vault, "text", &["-r", "dev_tools"], false);
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["kind"], "notebook_note");
    let output = run_capture(
        &vault,
        "text",
        &["-r", "dev_tools", "--section", "Git"],
        false,
    );
    assert_success(&output);
    let output = run_capture(&vault, "parent @dev_tools^myparent", &[], true);
    assert_success(&output);
    let output = run_capture(
        &vault,
        "child text",
        &["-r", "dev_tools", "-t", "myparent"],
        false,
    );
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["kind"], "notebook_sub_note");
    assert_eq!(json["notebook"]["parent_block_id"], "myparent");
}

#[test]
fn test_list_and_global_inheritance() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let output = run_capture(&vault, "@dev_tools\n- a\n- b", &[], false);
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert!(captures.iter().all(|item| item["kind"] == "notebook_note"));
    let output =
        run_capture(&vault, "Idea A\n\nIdea B\n\n@@dev_tools", &[], false);
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
}

#[test]
fn test_url_is_plain_note_with_no_ref_job() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let json = dry_single(&vault, "https://example.com/post @dev_tools");
    assert_eq!(json["kind"], "notebook_note");
    assert_eq!(json["text"], "https://example.com/post");
}

#[test]
fn test_mixed_task_and_note_is_atomic() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let before_notebook = read_notebook(&vault);
    let before_dev =
        fs::read_to_string(vault.join("dev.md")).expect("read dev");
    let output = run_capture(
        &vault,
        "Good task @dev\n\nBad note @dev_tools#gi",
        &[],
        true,
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(read_notebook(&vault), before_notebook);
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        before_dev
    );
}

#[test]
fn test_duplicate_ids_refuse() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let output = run_capture(&vault, "dup @dev_tools^existing", &[], false);
    // Duplicate block IDs are IO-guarded, like ordinary captures.
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn test_later_items_see_earlier_additions() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = vault_with_notebook(&temp);
    let output = run_capture(
        &vault,
        "First @dev_tools^newid\n\nChild @dev_tools+newid",
        &[],
        true,
    );
    assert_success(&output);
    let after = read_notebook(&vault);
    assert!(after.contains("^newid"));
    assert!(after.contains("- Child"));
}

#[test]
fn test_fenced_ids_are_not_parents() {
    let temp = TempDir::new("bob-cli-capture-notebook-notes");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: \"[[area]]\"\n---\n# Dev\n",
    );
    write_file(
        &vault.join("dev_tools.md"),
        "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n# Developer tools\n\n```\n- Fake ^fenced\n```\n\n- Real [created::2026-11-08] ^real\n",
    );
    let output = run_capture(&vault, "child @dev_tools+fenced", &[], false);
    assert_eq!(output.status.code(), Some(2));
    let json = dry_single(&vault, "child @dev_tools+real");
    assert_eq!(json["notebook"]["parent_block_id"], "real");
}
