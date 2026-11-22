//! `bob capture-complete --links-only --task-links` (bob-cli-oz.1).
//!
//! Plain-prose `((` task links for Bob's prose prompts: a `((query` field
//! returns the same `wikilink_task` response a capture draft does, with no
//! capture-grammar checks; `[[` fields still win, and rows group against
//! `--source-note` when one is given. Completion is read-only.

use crate::support::*;
use std::collections::BTreeMap;
use std::fs;

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn task_vault(name: &str) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("projects/api.md"),
        concat!(
            "# API\n",
            "- [ ] #task API docs overhaul ^api-docs\n",
            "- [ ] #task API doc lint\n",
        ),
    );
    write_file(
        &vault.join("home.md"),
        concat!(
            "- [ ] #task Update API docs ^home-api\n",
            "- [ ] #task Buy milk\n"
        ),
    );
    write_file(
        &vault.join("twin.md"),
        concat!(
            "- [ ] #task API docs twin ^twin\n",
            "- [ ] #task API docs twin again ^twin\n",
        ),
    );
    write_file(&vault.join("done/old.md"), "- [x] #task Old API doc pass\n");
    write_file(&vault.join("sase.md"), "# Design\n# Decision Log\n");
    (temp, vault)
}

fn run_complete(
    vault: &std::path::Path,
    flags: &[&str],
    cursor: usize,
    text: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("capture-complete").arg("-b").arg(vault);
    for flag in flags {
        command.arg(flag);
    }
    command
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run capture-complete")
}

fn complete(
    vault: &std::path::Path,
    flags: &[&str],
    cursor: usize,
    text: &str,
) -> serde_json::Value {
    let output = run_complete(vault, flags, cursor, text);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!("complete JSON: {error}\n{}", format_output(&output))
    })
}

fn snapshot(vault: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.to_string_lossy().into_owned(),
                    fs::read(&path).expect("read file"),
                );
            }
        }
    }
    files
}

fn candidates(json: &serde_json::Value) -> &Vec<serde_json::Value> {
    json["candidates"].as_array().expect("candidates array")
}

fn candidate<'a>(
    json: &'a serde_json::Value,
    text: &str,
) -> &'a serde_json::Value {
    candidates(json)
        .iter()
        .find(|row| row["text"] == text)
        .unwrap_or_else(|| panic!("missing candidate {text:?}: {json}"))
}

fn groups(json: &serde_json::Value) -> Vec<&str> {
    candidates(json)
        .iter()
        .map(|row| row["group"].as_str().expect("group"))
        .collect()
}

#[test]
fn task_links_requires_links_only() {
    let (_temp, vault) = task_vault("bob-cli-task-links-requires");
    let text = "Superseded by ((api";
    for flags in [
        &["-T"][..],
        &["--task-links"][..],
        &["-T", "-S", "sase.md"][..],
    ] {
        let output = run_complete(&vault, flags, text.len(), text);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{flags:?}: {}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains("--links-only"),
            "{flags:?}: {}",
            format_output(&output)
        );
    }
}

#[test]
fn task_links_field_returns_wikilink_task_with_guards() {
    let (_temp, vault) = task_vault("bob-cli-task-links-field");
    let before = snapshot(&vault);
    let text = "Superseded by ((api do";
    let json =
        complete(&vault, &["-L", "-T", "-S", "projects/api.md"], 22, text);
    assert_eq!(json["context"], "wikilink_task", "{json}");
    assert_eq!(json["query"], "api do", "{json}");
    assert_eq!(
        json["replacement"],
        serde_json::json!({ "start": 14, "end": 22 }),
        "{json}"
    );
    assert_eq!(json["picker"]["kind"], "wikilink_task", "{json}");
    assert_eq!(json["picker"]["scope"], "vault", "{json}");
    assert_eq!(json["picker"]["scope_token"], "((", "{json}");
    assert_eq!(
        json["picker"]["marker_range"],
        serde_json::json!({ "start": 14, "end": 22 }),
        "{json}"
    );
    assert_eq!(
        json["picker"]["trigger_removal_range"],
        serde_json::json!({ "start": 15, "end": 22 }),
        "{json}"
    );

    let identified = candidate(&json, "API docs overhaul");
    assert_eq!(identified["group"], "in_this_note", "{json}");
    assert_eq!(identified["replacement"], "[[api#^api-docs]]", "{json}");
    assert_eq!(identified["link_target"], "api", "{json}");
    assert_eq!(
        identified["cursor_after"],
        14 + "[[api#^api-docs]]".len(),
        "{json}"
    );

    let id_less = candidate(&json, "API doc lint");
    assert_eq!(id_less["requires_block_id"], true, "{json}");
    assert_eq!(id_less["replacement"], "", "{json}");
    assert!(id_less.get("cursor_after").is_none(), "{json}");
    assert!(id_less.get("disabled_reason").is_none(), "{json}");

    let duplicate = candidate(&json, "API docs twin");
    assert_eq!(
        duplicate["disabled_reason"], "duplicate block ID ^twin in twin.md",
        "{json}"
    );
    assert_eq!(duplicate["replacement"], "", "{json}");

    let archived = candidate(&json, "Old API doc pass");
    assert_eq!(
        archived["disabled_reason"], "archived tasks can't get an id",
        "{json}"
    );

    assert!(
        candidates(&json)
            .iter()
            .all(|row| row["text"] != "Buy milk"),
        "multi-term query filters every term: {json}"
    );
    assert_eq!(snapshot(&vault), before, "completion must not write");
}

#[test]
fn source_note_adds_the_this_note_group() {
    let (_temp, vault) = task_vault("bob-cli-task-links-source");
    let text = "See ((";
    let with_source =
        complete(&vault, &["-L", "-T", "-S", "projects/api.md"], 6, text);
    assert_eq!(with_source["context"], "wikilink_task", "{with_source}");
    let grouped = groups(&with_source);
    assert_eq!(
        &grouped[..2],
        &["in_this_note", "in_this_note"],
        "{with_source}"
    );

    let without = complete(&vault, &["-L", "-T"], 6, text);
    assert_eq!(without["context"], "wikilink_task", "{without}");
    assert!(
        !groups(&without).contains(&"in_this_note"),
        "no source note means no this-note group: {without}"
    );
    assert_eq!(
        candidates(&without).len(),
        candidates(&with_source).len(),
        "{without}"
    );
}

#[test]
fn wikilink_fields_still_win() {
    let (_temp, vault) = task_vault("bob-cli-task-links-wikilink-wins");
    let note = "Read [[ap";
    let json = complete(&vault, &["-L", "-T"], note.len(), note);
    assert_eq!(json["context"], "wikilink_note", "{json}");

    let nested = "See [[api ((x";
    let json = complete(&vault, &["-L", "-T"], nested.len(), nested);
    assert_eq!(json["context"], "wikilink_note", "{json}");

    // A same-note `[[#` field without `-S` stays an empty success even
    // with `-T`.
    let heading = "See [[#De";
    let json = complete(&vault, &["-L", "-T"], heading.len(), heading);
    assert!(json["context"].is_null(), "{json}");
}

#[test]
fn links_only_without_task_links_leaves_parens_literal() {
    let (_temp, vault) = task_vault("bob-cli-task-links-off");
    let text = "Superseded by ((api";
    let json = complete(&vault, &["-L"], text.len(), text);
    assert!(json["context"].is_null(), "{json}");
    assert_eq!(
        json["replacement"],
        serde_json::json!({ "start": text.len(), "end": text.len() }),
        "{json}"
    );
}

#[test]
fn prose_opens_where_capture_grammar_would_not() {
    let (_temp, vault) = task_vault("bob-cli-task-links-prose-grammar");
    let text = "x\n- Follow up ((api";
    let capture = complete(&vault, &[], text.len(), text);
    assert!(capture["context"].is_null(), "{capture}");

    let prose = complete(&vault, &["-L", "-T"], text.len(), text);
    assert_eq!(prose["context"], "wikilink_task", "{prose}");
    assert_eq!(prose["query"], "api", "{prose}");
    let opener = text.find("((").expect("opener");
    assert_eq!(
        prose["replacement"],
        serde_json::json!({ "start": opener, "end": text.len() }),
        "{prose}"
    );

    let triple = "See (((api";
    let json = complete(&vault, &["-L", "-T"], triple.len(), triple);
    assert!(json["context"].is_null(), "{json}");
}

#[test]
fn capture_path_is_unchanged_and_matches_prose_rows() {
    let (_temp, vault) = task_vault("bob-cli-task-links-capture");
    let text = "Superseded by ((api do";
    let capture = complete(&vault, &[], 22, text);
    assert_eq!(capture["context"], "wikilink_task", "{capture}");
    let prose = complete(&vault, &["-L", "-T"], 22, text);
    for key in ["replacement", "query", "picker", "candidates"] {
        assert_eq!(prose[key], capture[key], "{key}: {prose}\n{capture}");
    }
}
