//! `bob notebooks list` and `check` contract tests.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn vault_fixture(name: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    (temp, vault)
}

fn notebooks_command(vault: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("notebooks")
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1");
    command
}

fn write_note(vault: &Path, name: &str, contents: &str) {
    write_file(&vault.join(name), contents);
}

fn area_note(parent: &str) -> String {
    format!(
        "---\nparent: \"[[{parent}]]\"\ntype: \"[[area]]\"\n---\n\n# Area\n"
    )
}

fn notebook_note(parent: &str, summary: &str, body: &str) -> String {
    format!(
        "---\nparent: \"[[{parent}]]\"\ntemplate: \"[[new_notebook]]\"\ntype: \"[[notebook]]\"\nsummary: {summary}\ncreated: 2026-11-09T09:14:00-05:00\n---\n\n{body}"
    )
}

fn basic_vault(vault: &Path) {
    write_note(vault, "dev.md", &area_note("area"));
    write_note(vault,
        "dev_tools.md",
        &notebook_note(
            "dev",
            "Command-line tools.",
            "# Developer tools\n\n- Use tool [created::2026-11-09]\n\n## Git tips\n\n- Check status [created::2026-11-08]\n",
        ),
    );
    write_note(
        vault,
        "git.md",
        &notebook_note(
            "dev_tools",
            "Git commands.",
            "# Git\n\n- Basic [created::2026-11-09]\n",
        ),
    );
}

#[test]
fn notebooks_list_empty_vault_reports_empty() {
    let (_temp, vault) = vault_fixture("notebooks-empty");
    let output = notebooks_command(&vault)
        .arg("list")
        .output()
        .expect("run notebooks list");
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("No notebooks"), "text:\n{text}");
}

#[test]
fn notebooks_list_and_check_json_shape() {
    let (_temp, vault) = vault_fixture("notebooks-json");
    basic_vault(&vault);
    let output = notebooks_command(&vault)
        .arg("list")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run list json");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse json");
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["ok"], true);
    assert!(payload["catalogs"].as_array().unwrap().is_empty());
    assert_eq!(payload["notebooks"].as_array().unwrap().len(), 2);
    let git = payload["notebooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["stem"] == "git")
        .expect("git notebook");
    assert_eq!(git["owner"], "dev");
    assert_eq!(git["root"], "dev");
    assert_eq!(git["parent"], "dev_tools");
    assert_eq!(git["parent_kind"], "notebook");
    assert_eq!(git["last_note"], "2026-11-09");

    let output = notebooks_command(&vault)
        .arg("check")
        .arg("-f")
        .arg("json")
        .output()
        .expect("run check json");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse check");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["counts"]["errors"], 0);
}

#[test]
fn notebooks_bare_defaults_to_list() {
    let (_temp, vault) = vault_fixture("notebooks-bare");
    basic_vault(&vault);
    let bare = notebooks_command(&vault).output().expect("bare");
    let explicit = notebooks_command(&vault)
        .arg("list")
        .output()
        .expect("explicit");
    assert!(bare.status.success());
    assert!(explicit.status.success());
    assert_eq!(stdout(&bare), stdout(&explicit));
}

#[test]
fn notebooks_check_reports_all_finding_codes() {
    let (_temp, vault) = vault_fixture("notebooks-findings");
    write_note(&vault, "dev.md", &area_note("area"));
    // Missing summary + checkbox.
    write_note(&vault,
        "bad.md",
        "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\ncreated: 2026-11-09T09:14:00-05:00\n---\n\n# Bad\n\n- [ ] task should error\n",
    );
    let output = notebooks_command(&vault)
        .arg("check")
        .arg("-f")
        .arg("json")
        .output()
        .expect("check json");
    assert!(!output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    let issues = payload["notebooks"][0]["issues"].as_array().unwrap();
    let codes: Vec<&str> = issues
        .iter()
        .map(|issue| issue["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"notebook_summary_missing"));
    assert!(codes.contains(&"notebook_checkbox"));
}

#[test]
fn notebooks_refuses_unquoted_and_not_at_root() {
    let (_temp, vault) = vault_fixture("notebooks-unquoted");
    write_note(&vault, "dev.md", &area_note("area"));
    write_note(&vault,
        "plain.md",
        "---\nparent: [[dev]]\ntype: \"[[notebook]]\"\nsummary: Plain.\ncreated: 2026-11-09T09:14:00-05:00\n---\n\n# Plain\n\n- Note [created::2026-11-09]\n",
    );
    fs::create_dir_all(vault.join("sub")).expect("sub");
    write_note(
        &vault,
        "sub/nested.md",
        &notebook_note("dev", "Nested.", "# Nested\n\n- Note\n"),
    );
    let output = notebooks_command(&vault)
        .arg("check")
        .arg("-f")
        .arg("json")
        .output()
        .expect("check");
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    let by_stem = |stem: &str| {
        payload["notebooks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["stem"] == stem)
            .cloned()
    };
    let plain = by_stem("plain").expect("plain");
    assert!(plain["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "notebook_parent_unquoted"));
    let nested = by_stem("nested").expect("nested");
    assert!(nested["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "notebook_not_at_root"));
}

#[test]
fn notebooks_task_move_refuses_with_owner_hint() {
    let (_temp, vault) = vault_fixture("notebooks-move");
    basic_vault(&vault);
    write_note(&vault,
        "tasks.md",
        "---\nparent: \"[[dev]]\"\ntype: \"[[area]]\"\n---\n\n- [ ] #task test ^mine\n",
    );
    let mut command = bob_command();
    command
        .arg("task")
        .arg("move")
        .arg("tasks")
        .arg("dev_tools")
        .arg("-b")
        .arg("mine")
        .arg("-d")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1");
    let output = command.output().expect("task move");
    let text = format_output(&output);
    assert!(
        text.contains("is a notebook and holds no tasks"),
        "text:\n{text}"
    );
    assert!(text.contains("@dev"), "text:\n{text}");
}

#[test]
fn notebooks_vault_bytes_unchanged_by_list_and_check() {
    let (_temp, vault) = vault_fixture("notebooks-clean");
    basic_vault(&vault);
    let before: Vec<(PathBuf, String)> = fs::read_dir(&vault)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let contents = fs::read_to_string(&path).unwrap();
            (path, contents)
        })
        .collect();
    for args in [&["list"][..], &["check"][..]] {
        let mut command = notebooks_command(&vault);
        command.args(args);
        let output = command.output().expect("run");
        assert!(output.status.success(), "{}", format_output(&output));
    }
    // JSON also leaves bytes alone.
    for args in [&["list", "-f", "json"][..], &["check", "-f", "json"][..]] {
        let mut command = notebooks_command(&vault);
        command.args(args);
        let output = command.output().expect("run json");
        assert!(output.status.success(), "{}", format_output(&output));
    }
    for (path, contents) in before {
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }
}

#[test]
fn notebooks_root_filter_and_warning_only_check() {
    let (_temp, vault) = vault_fixture("notebooks-filter");
    basic_vault(&vault);
    // Warning-only notebook: long summary.
    write_note(
        &vault,
        "long.md",
        &notebook_note(
            "dev",
            &"x".repeat(150),
            "# Long\n\n- Note [created::2026-11-09]\n",
        ),
    );
    let output = notebooks_command(&vault)
        .arg("list")
        .arg("-r")
        .arg("dev")
        .arg("-f")
        .arg("json")
        .output()
        .expect("filtered");
    assert!(output.status.success());
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["notebooks"].as_array().unwrap().len(), 3);

    let output = notebooks_command(&vault)
        .arg("check")
        .output()
        .expect("check human");
    // Only warnings so far: long summary is a warning.
    assert!(output.status.success(), "{}", format_output(&output));
}
