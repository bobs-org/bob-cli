//! `bob areas list` and `check` contract tests.

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

fn areas_command(vault: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("areas")
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1");
    command
}

fn write_note(vault: &Path, name: &str, contents: &str) {
    write_file(&vault.join(name), contents);
}

fn area_note(parent: &str, extra: &str, title: &str) -> String {
    format!(
        "---\ntype: \"[[area]]\"\nparent: \"[[{parent}]]\"\n{extra}---\n\n# {title}\n"
    )
}

fn tree_vault(vault: &Path) {
    write_note(vault, "area.md", "# Area root\n");
    write_note(vault, "body.md", &area_note("area", "", "Body"));
    write_note(
        vault,
        "cash.md",
        &area_note(
            "area",
            "color: magenta\nsummary: Money and bills.\n",
            "Cash",
        ),
    );
    write_note(vault, "bank.md", &area_note("cash", "", "Bank"));
}

#[test]
fn areas_help_lists_list_and_check() {
    let output = bob_command()
        .args(["areas", "--help"])
        .output()
        .expect("run bob areas --help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Bare `bob areas` runs the read-only list")
            && help.contains("docs/areas.md")
            && help.contains("check")
            && help.contains("list")
            && help.contains("(default)"),
        "expected areas group help:\n{help}"
    );
    assert_text_order(&help, &["\n  check ", "\n  list "]);

    let list = bob_command()
        .args(["areas", "list", "--help"])
        .output()
        .expect("run bob areas list --help");
    assert_success(&list);
    let list_help = stdout(&list);
    assert!(
        list_help.contains("-b, --bob-dir")
            && list_help.contains("-f, --format")
            && list_help.contains("-h, --help")
            && list_help.contains("docs/areas.md")
            && list_help.contains("always exits 0"),
        "expected list help:\n{list_help}"
    );
    assert_text_order(
        &list_help,
        &["-b, --bob-dir", "-f, --format", "-h, --help"],
    );

    let check = bob_command()
        .args(["areas", "check", "--help"])
        .output()
        .expect("run bob areas check --help");
    assert_success(&check);
    let check_help = stdout(&check);
    assert!(
        check_help.contains("Exits 1 on any error finding")
            && check_help.contains("docs/areas.md"),
        "expected check help:\n{check_help}"
    );
}

#[test]
fn areas_bare_defaults_to_list() {
    let (_temp, vault) = vault_fixture("areas-bare");
    tree_vault(&vault);
    let bare = areas_command(&vault).output().expect("bare");
    let explicit = areas_command(&vault)
        .arg("list")
        .output()
        .expect("explicit");
    assert!(bare.status.success(), "{}", format_output(&bare));
    assert!(explicit.status.success(), "{}", format_output(&explicit));
    assert_eq!(stdout(&bare), stdout(&explicit));
    let text = stdout(&bare);
    assert!(
        text.contains("Areas · 3 areas · 2 catalogs")
            && text.contains("body")
            && text.contains("cash")
            && text.contains("bank")
            && text.contains("├─")
            && text.contains("└─"),
        "text:\n{text}"
    );
}

#[test]
fn areas_list_json_golden_shape() {
    let (_temp, vault) = vault_fixture("areas-json");
    tree_vault(&vault);
    let output = areas_command(&vault)
        .args(["list", "-f", "json"])
        .output()
        .expect("run list json");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse json");
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "list");
    assert_eq!(payload["root"]["path"], "area.md");
    assert_eq!(payload["root"]["stem"], "area");
    assert_eq!(payload["root"]["state"], "found");
    let areas = payload["areas"].as_array().unwrap();
    assert_eq!(areas.len(), 3);
    assert_eq!(areas[0]["stem"], "body");
    assert_eq!(areas[0]["depth"], 1);
    assert_eq!(areas[1]["stem"], "cash");
    assert_eq!(areas[1]["path"], "cash.md");
    assert_eq!(areas[1]["link"], "cash");
    assert_eq!(areas[1]["title"], "Cash");
    assert_eq!(areas[1]["titled"], true);
    assert_eq!(areas[1]["summary"], "Money and bills.");
    assert_eq!(areas[1]["parent"], "area.md");
    assert_eq!(areas[1]["depth"], 1);
    assert_eq!(areas[1]["top"], "cash");
    assert_eq!(areas[1]["hue"], "magenta");
    assert_eq!(areas[1]["hue_source"], "color");
    assert_eq!(areas[1]["children"], serde_json::json!(["bank.md"]));
    assert_eq!(areas[2]["stem"], "bank");
    assert_eq!(areas[2]["parent"], "cash.md");
    assert_eq!(areas[2]["depth"], 2);
    assert_eq!(areas[2]["top"], "cash");
    let catalogs = payload["catalogs"].as_array().unwrap();
    assert_eq!(catalogs.len(), 2);
    assert_eq!(catalogs[0]["host"], "area");
    assert_eq!(catalogs[0]["kind"], "root");
    assert_eq!(catalogs[0]["state"], "missing");
    assert_eq!(catalogs[1]["host"], "cash");
    assert_eq!(catalogs[1]["kind"], "area");
    assert_eq!(catalogs[1]["state"], "missing");
    assert_eq!(payload["counts"]["areas"], 3);
    assert_eq!(payload["counts"]["top_level"], 2);
    assert_eq!(payload["counts"]["hosts"], 2);
    assert_eq!(payload["counts"]["errors"], 0);
    assert!(payload["counts"]["warnings"].as_u64().unwrap() >= 2);
}

#[test]
fn areas_check_exits_zero_when_clean() {
    let (_temp, vault) = vault_fixture("areas-clean");
    write_note(&vault, "area.md", "# Area root\n");
    let output = areas_command(&vault)
        .arg("check")
        .output()
        .expect("check clean");
    assert!(output.status.success(), "{}", format_output(&output));
    let text = stdout(&output);
    assert!(text.contains("areas match the contract"), "text:\n{text}");
}

#[test]
fn areas_check_exits_one_on_error_finding() {
    let (_temp, vault) = vault_fixture("areas-error");
    write_note(&vault, "area.md", "# Area root\n");
    write_note(
        &vault,
        "orphan.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Orphan\n",
    );
    let output = areas_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check error");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["command"], "check");
    let codes: Vec<&str> = payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"area_parent_missing"), "codes: {codes:?}");
}

#[test]
fn areas_check_exits_one_on_damaged_catalog() {
    let (_temp, vault) = vault_fixture("areas-damaged");
    write_note(
        &vault,
        "area.md",
        "# Area root\n\n## Areas\n\n> [!areas\n> - partial\n",
    );
    write_note(&vault, "cash.md", &area_note("area", "", "Cash"));
    let output = areas_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check damaged");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["ok"], false);
    let catalogs = payload["catalogs"].as_array().unwrap();
    assert!(
        catalogs.iter().any(|catalog| catalog["state"] == "damaged"
            && catalog["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["code"] == "area_catalog_damaged")),
        "catalogs: {catalogs:?}"
    );
}

#[test]
fn areas_check_exits_zero_with_only_warnings() {
    let (_temp, vault) = vault_fixture("areas-warnings");
    tree_vault(&vault);
    let output = areas_command(&vault)
        .args(["check", "-f", "json"])
        .output()
        .expect("check warnings");
    assert!(output.status.success(), "{}", format_output(&output));
    let payload: Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse");
    assert_eq!(payload["ok"], true);
    assert!(payload["counts"]["errors"].as_u64().unwrap() == 0);
    assert!(payload["counts"]["warnings"].as_u64().unwrap() > 0);
    let codes: Vec<&str> = payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"area_catalog_stale"), "codes: {codes:?}");
}

#[test]
fn areas_list_exits_zero_with_error_findings() {
    let (_temp, vault) = vault_fixture("areas-list-errors");
    write_note(&vault, "area.md", "# Area root\n");
    write_note(
        &vault,
        "orphan.md",
        "---\ntype: \"[[area]]\"\n---\n\n# Orphan\n",
    );
    let output = areas_command(&vault)
        .arg("list")
        .output()
        .expect("list with errors");
    assert!(output.status.success(), "{}", format_output(&output));
    let text = stdout(&output);
    assert!(text.contains("Unplaced"), "text:\n{text}");
    assert!(text.contains("area_parent_missing"), "text:\n{text}");
}

#[test]
fn areas_vault_bytes_unchanged_by_list_and_check() {
    let (_temp, vault) = vault_fixture("areas-readonly");
    tree_vault(&vault);
    let before: Vec<(PathBuf, String)> = fs::read_dir(&vault)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let contents = fs::read_to_string(&path).unwrap();
            (path, contents)
        })
        .collect();
    for args in [&["list"][..], &["check"][..], &["list", "-f", "json"][..]] {
        let mut command = areas_command(&vault);
        command.args(args);
        let output = command.output().expect("run");
        assert!(
            output.status.success() || args[0] == "check",
            "{}",
            format_output(&output)
        );
    }
    for (path, contents) in before {
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }
}
