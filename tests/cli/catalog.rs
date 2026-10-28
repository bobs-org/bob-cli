//! Hidden `bob __catalog --protocol 1` handshake.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::time::Instant;

fn catalog() -> (std::process::Output, Value) {
    catalog_with(&["--protocol", "1"])
}

fn catalog_with(args: &[&str]) -> (std::process::Output, Value) {
    let mut command = bob_command();
    command.arg("__catalog");
    for arg in args {
        command.arg(arg);
    }
    let output = command.output().expect("run bob __catalog");
    let parsed = if output.status.success() {
        serde_json::from_str(&stdout(&output)).expect("catalog json")
    } else {
        Value::Null
    };
    (output, parsed)
}

#[test]
fn protocol_1_parses_with_schema_version_one() {
    let (output, json) = catalog();
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    assert_stdout_has_no_ansi(&output);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], true);
    assert_eq!(json["protocol"], 1);
    assert_eq!(json["bob_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        json["features"],
        serde_json::json!(["clicolor-force", "progress-env", "run-receipt"])
    );
    let entries = json["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 55);
    assert!(entries.iter().any(|entry| {
        entry["id"] == "themes-split"
            && entry["path"] == serde_json::json!(["themes", "split"])
    }));
}

#[test]
fn protocol_equals_form_matches_space_form() {
    let (space, space_json) = catalog_with(&["--protocol", "1"]);
    let (equals, equals_json) = catalog_with(&["--protocol=1"]);
    assert_success(&space);
    assert_success(&equals);
    assert_eq!(space_json, equals_json);
}

#[test]
fn protocol_2_returns_json_error_and_exit_2() {
    let (output, _) = catalog_with(&["--protocol", "2"]);
    assert_eq!(output.status.code(), Some(2));
    let json: Value =
        serde_json::from_str(&stdout(&output)).expect("error json");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], false);
    assert_eq!(json["code"], "unsupported_protocol");
    assert_eq!(json["min"], 1);
    assert_eq!(json["max"], 1);
    assert!(json["message"]
        .as_str()
        .is_some_and(|text| !text.is_empty()));
}

#[test]
fn missing_protocol_exits_2_with_usage() {
    let mut command = bob_command();
    command.arg("__catalog");
    let output = command.output().expect("run bare __catalog");
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("missing --protocol"), "{err}");
    assert!(err.contains("usage: bob __catalog --protocol"), "{err}");
}

#[test]
fn catalog_is_read_only() {
    let probe = TempDir::new("bob-cli-catalog-readonly");
    let cache = probe.path().join("cache");
    let state = probe.path().join("state");
    fs::create_dir_all(&cache).expect("cache");
    fs::create_dir_all(&state).expect("state");
    let mut command = bob_command();
    command
        .arg("__catalog")
        .arg("--protocol")
        .arg("1")
        .env("BOB_DIR", "/definitely/missing/bob-cli-catalog-vault")
        .env("XDG_CACHE_HOME", &cache)
        .env("XDG_STATE_HOME", &state);
    let output = command.output().expect("run bob __catalog");
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    let json: Value =
        serde_json::from_str(&stdout(&output)).expect("catalog json");
    assert_eq!(json["schema_version"], 1);
    assert_empty_tree(&cache);
    assert_empty_tree(&state);
}

#[test]
fn catalog_is_fast() {
    let started = Instant::now();
    let (output, _) = catalog();
    assert_success(&output);
    let elapsed_ms = started.elapsed().as_millis();
    eprintln!("bob __catalog --protocol 1 took {elapsed_ms}ms");
}

fn assert_empty_tree(root: &std::path::Path) {
    let mut files = Vec::new();
    visit(root, &mut files);
    assert!(
        files.is_empty(),
        "expected no files under {}: {files:?}",
        root.display()
    );
}

fn visit(path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            visit(&child, files);
        } else {
            files.push(child);
        }
    }
}
