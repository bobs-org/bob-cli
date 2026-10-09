//! Capture targets tests.

use crate::support::*;
use std::fs;

#[test]
fn capture_targets_json_lists_picker_targets_in_order() {
    let temp = TempDir::new("bob-cli-capture-targets-json");
    let vault = temp.path().join("vault");

    write_file(&vault.join("mac_inbox.md"), "---\ntype: [[area]]\n---\n");
    write_file(&vault.join("dev.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&vault.join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n",
    );
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[project]]\nstatus: waiting\n---\n",
    );
    write_file(
        &vault.join("odd.md"),
        "---\ntype: [[project]]\nstatus: blocked\n---\n",
    );
    write_file(
        &vault.join("done.md"),
        "---\ntype: [[project]]\nstatus: done\n---\n",
    );
    write_file(
        &vault.join("canceled.md"),
        "---\ntype: [[project]]\nstatus: canceled\n---\n",
    );
    write_file(&vault.join("ref.md"), "---\ntype: [[ref]]\n---\n");
    write_file(&vault.join("nested/child.md"), "---\ntype: [[area]]\n---\n");
    write_file(&vault.join("Foo.md"), "---\ntype: [[area]]\n---\n");

    let output = bob_command()
        .arg("capture-targets")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-targets json");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "default capture-targets should keep skip warnings off stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["bob_dir"], vault.display().to_string());
    assert_eq!(json["count"], 6);

    let targets = json["targets"].as_array().expect("targets array");
    let routes = targets
        .iter()
        .map(|target| target["route"].as_str().expect("route string"))
        .collect::<Vec<_>>();
    assert_eq!(routes, ["mac_inbox", "cash", "dev", "bob", "odd", "sase"]);

    assert_eq!(targets[0]["name"], "mac_inbox");
    assert_eq!(targets[0]["label"], "mac_inbox.md");
    assert_eq!(targets[0]["kind"], "inbox");
    assert_eq!(targets[0]["is_default"], true);
    assert!(targets[0]["status"].is_null());
    assert_eq!(targets[0]["relative_path"], "mac_inbox.md");
    assert_eq!(targets[1]["kind"], "area");
    assert!(targets[1]["status"].is_null());
    assert_eq!(targets[3]["kind"], "project");
    assert_eq!(targets[3]["status"], "wip");
    assert_eq!(targets[4]["status"], "blocked");
    assert_eq!(targets[5]["status"], "waiting");
    for target in targets {
        assert!(
            target
                .get("project_name_aliases")
                .is_some_and(|aliases| aliases.as_array().is_some()),
            "every target carries project_name_aliases: {target}"
        );
    }
}

#[test]
fn capture_targets_json_reports_aliases_and_human_shows_aka() {
    let temp = TempDir::new("bob-cli-capture-targets-aliases");
    let vault = temp.path().join("vault");

    write_file(&vault.join("mac_inbox.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\nproject_name_aliases: [\"bob-cli\"]\n---\n",
    );
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[area]]\nproject_name_aliases:\n  - sase-alias\n---\n",
    );

    let output = bob_command()
        .arg("capture-targets")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-targets json with aliases");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    let targets = json["targets"].as_array().expect("targets array");
    let bob = targets
        .iter()
        .find(|target| target["route"] == "bob")
        .expect("bob target");
    assert_eq!(bob["project_name_aliases"], serde_json::json!(["bob-cli"]));
    let sase = targets
        .iter()
        .find(|target| target["route"] == "sase")
        .expect("sase target");
    assert_eq!(
        sase["project_name_aliases"],
        serde_json::json!(["sase-alias"])
    );

    let human = bob_command()
        .arg("capture-targets")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob capture-targets human with aliases");

    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("aka bob-cli") && out.contains("aka sase-alias"),
        "human output shows aliases:\n{out}"
    );

    let verbose = bob_command()
        .arg("capture-targets")
        .arg("-b")
        .arg(&vault)
        .arg("-v")
        .output()
        .expect("run bob capture-targets verbose with aliases");

    assert_success(&verbose);
    assert!(
        stderr(&verbose).is_empty(),
        "clean aliases warn about nothing:\n{}",
        format_output(&verbose)
    );
}

#[test]
fn capture_targets_verbose_emits_skip_warnings() {
    let temp = TempDir::new("bob-cli-capture-targets-verbose");
    let vault = temp.path().join("vault");

    write_file(&vault.join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(&vault.join("Foo.md"), "---\ntype: [[area]]\n---\n");

    for flag in ["--verbose", "-v"] {
        let output = bob_command()
            .arg("capture-targets")
            .arg("-b")
            .arg(&vault)
            .arg(flag)
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob capture-targets {flag}: {error}")
            });

        assert_success(&output);
        assert!(
            stderr(&output).contains("Foo.md")
                && stderr(&output).contains("skipping non-routable note"),
            "expected `{flag}` to emit skip warning:\n{}",
            format_output(&output)
        );
    }

    let quiet = bob_command()
        .arg("capture-targets")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob capture-targets default");

    assert_success(&quiet);
    assert!(
        stderr(&quiet).is_empty(),
        "default capture-targets should keep skip warnings off stderr:\n{}",
        format_output(&quiet)
    );
}

#[test]
fn capture_targets_human_groups_and_summarizes_without_ansi() {
    let temp = TempDir::new("bob-cli-capture-targets-human");
    let vault = temp.path().join("vault");

    write_file(&vault.join("cash.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n",
    );
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[project]]\nstatus: waiting\n---\n",
    );

    let output = bob_command()
        .arg("capture-targets")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob capture-targets human");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-targets stderr:\n{}",
        format_output(&output)
    );
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        out.contains("Capture targets - ")
            && out.contains("  Inbox")
            && out.contains("* mac_inbox")
            && out.contains("default")
            && out.contains("  Areas")
            && out.contains("cash.md")
            && out.contains("  Active projects")
            && out.contains("bob.md")
            && out.contains("waiting")
            && out.contains("4 targets - 1 inbox - 1 area - 2 active projects"),
        "unexpected capture-targets human output:\n{out}"
    );
    assert_text_order(&out, &["mac_inbox.md", "cash.md", "bob.md", "sase.md"]);
}

#[test]
fn capture_targets_empty_vault_still_lists_inbox_default() {
    let temp = TempDir::new("bob-cli-capture-targets-empty");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture-targets")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-targets empty vault");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected empty-vault stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["count"], 1);
    assert_eq!(json["targets"][0]["route"], "mac_inbox");
    assert_eq!(json["targets"][0]["kind"], "inbox");
    assert_eq!(json["targets"][0]["is_default"], true);
}

#[test]
fn capture_targets_json_failure_prints_error_object() {
    let temp = TempDir::new("bob-cli-capture-targets-json-failure");
    let missing_vault = temp.path().join("missing-vault");

    let output = bob_command()
        .arg("capture-targets")
        .arg("-b")
        .arg(&missing_vault)
        .arg("-f")
        .arg("json")
        .output()
        .expect("run bob capture-targets json failure");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing vault should be an IO failure:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "json failure should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("failed to read directory")),
        "unexpected json failure object: {json}"
    );
}
