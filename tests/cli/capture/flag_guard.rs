//! Unknown long flags where capture-style TEXT would begin (bob-cli-8a).
//!
//! `bob capture` used to file `--flag -- draft` as an ordinary task and exit
//! 0 because its TEXT positional swallows hyphen values. The guard rejects
//! an unknown alphabetic long option before `--` with exit 2 and writes
//! nothing; the same rule covers every other command sharing the
//! capture-style TEXT positional.

use crate::support::*;
use std::fs;

fn inbox_vault(name: &str) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("mac_inbox.md"), "# Tasks\n");
    (temp, vault)
}

#[test]
fn capture_unknown_long_flag_is_rejected_with_json_and_writes_nothing() {
    let (_temp, vault) = inbox_vault("bob-cli-capture-flag-guard-json");
    let inbox = vault.join("mac_inbox.md");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--format")
        .arg("json")
        .arg("--require-pull-in-choice-x")
        .arg("--")
        .arg("hi")
        .env("BOB_NOW", "2026-10-24")
        .output()
        .expect("run capture with unknown flag");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("guard rejection should be JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(
        json["error"],
        "unexpected argument '--require-pull-in-choice-x' found"
    );
    assert_eq!(
        fs::read_to_string(&inbox).expect("read inbox"),
        "# Tasks\n",
        "rejected capture must not write"
    );
}

#[test]
fn capture_unknown_long_flag_human_error_writes_nothing() {
    let (_temp, vault) = inbox_vault("bob-cli-capture-flag-guard-human");
    let inbox = vault.join("mac_inbox.md");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--bogus")
        .arg("hello")
        .env("BOB_NOW", "2026-10-24")
        .output()
        .expect("run capture with unknown flag");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("unexpected argument '--bogus' found"),
        "human error should use clap wording:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&inbox).expect("read inbox"),
        "# Tasks\n",
        "rejected capture must not write"
    );
}

#[test]
fn capture_delimiter_literal_and_trailing_flag_word_still_capture() {
    let (_temp, vault) = inbox_vault("bob-cli-capture-flag-guard-text");

    let literal = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("--anything")
        .env("BOB_NOW", "2026-10-24")
        .output()
        .expect("run capture with delimiter literal");
    assert_success(&literal);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&literal).trim()).expect("capture JSON");
    assert_eq!(json["text"], "--anything");

    let trailing = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("hello")
        .arg("--world")
        .env("BOB_NOW", "2026-10-24")
        .output()
        .expect("run capture with trailing flag word");
    assert_success(&trailing);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&trailing).trim()).expect("capture JSON");
    assert_eq!(json["text"], "hello --world");
}

#[test]
fn capture_parse_unknown_long_flag_is_rejected_with_json() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--format")
        .arg("json")
        .arg("--bogus")
        .output()
        .expect("run capture-parse with unknown flag");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("guard rejection should be JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], "unexpected argument '--bogus' found");
}

#[test]
fn capture_rewrite_unknown_long_flag_is_rejected_with_json() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-f")
        .arg("json")
        .arg("--bogus")
        .output()
        .expect("run capture-rewrite with unknown flag");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("guard rejection should be JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], "unexpected argument '--bogus' found");
}

#[test]
fn capture_templates_save_guard_json_carries_a_stable_code() {
    let output = bob_command()
        .arg("capture-templates")
        .arg("save")
        .arg("-f")
        .arg("json")
        .arg("--bogus")
        .output()
        .expect("run capture-templates save with unknown flag");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("guard rejection should be JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], "unexpected argument '--bogus' found");
    assert_eq!(json["code"], "saved_templates_unknown_flag");
}

#[test]
fn capture_mixed_short_cluster_unknown_long_is_rejected_with_json() {
    let (_temp, vault) = inbox_vault("bob-cli-capture-flag-guard-cluster");
    let inbox = vault.join("mac_inbox.md");

    for argv in [
        vec!["-nf", "json", "--bogus", "--", "hi"],
        vec!["-nfjson", "--bogus", "--", "hi"],
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .args(&argv)
            .env("BOB_NOW", "2026-10-24")
            .output()
            .expect("run capture with mixed short cluster and unknown flag");

        assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("guard rejection should be JSON");
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"], "unexpected argument '--bogus' found");
        assert_eq!(
            fs::read_to_string(&inbox).expect("read inbox"),
            "# Tasks\n",
            "rejected capture must not write"
        );
    }
}

#[test]
fn capture_complete_unknown_long_flag_is_rejected_with_json() {
    let output = bob_command()
        .arg("capture-complete")
        .arg("--cursor")
        .arg("0")
        .arg("--format")
        .arg("json")
        .arg("--bogus")
        .arg("--")
        .arg("hi")
        .output()
        .expect("run capture-complete with unknown flag");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("guard rejection should be JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], "unexpected argument '--bogus' found");
}

#[test]
fn capture_templates_save_unknown_flag_leaves_the_shelf_unchanged() {
    let temp = TempDir::new("bob-cli-capture-flag-guard-templates");
    let shelf = temp.path().join("templates.json");
    let original = "{\"schema_version\":1,\"templates\":[]}\n";
    fs::write(&shelf, original).expect("write shelf");

    let output = bob_command()
        .arg("capture-templates")
        .arg("save")
        .arg("-f")
        .arg("json")
        .arg("--bogus")
        .arg("--")
        .arg("@@template:\nApply to {0}\n\n")
        .env("BOB_CAPTURE_TEMPLATES_FILE", &shelf)
        .output()
        .expect("run capture-templates save with unknown flag");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("guard rejection should be JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"], "unexpected argument '--bogus' found");
    assert_eq!(
        fs::read_to_string(&shelf).expect("read shelf"),
        original,
        "rejected save must not rewrite the shelf"
    );
}
