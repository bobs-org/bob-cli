//! `bob capture-complete --links-only` (bob-cli-om.8).
//!
//! Plain-prose wikilink completion for the Mac Pull-In reason field: only
//! note, heading, and block fields complete, capture grammar stays literal,
//! same-note fields need `--source-note`, and the Work Log block-link
//! suppression does not apply.

use crate::support::*;

fn links_vault(name: &str) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("sase.md"),
        "# Design\n# Decision Log\n\nBody text ^abc123\n",
    );
    write_file(&vault.join("db.md"), "---\ntype: [[area]]\n---\n");
    (temp, vault)
}

fn complete(
    vault: &std::path::Path,
    extra: &[&str],
    cursor: usize,
    draft: &str,
) -> serde_json::Value {
    let mut command = bob_command();
    command.arg("capture-complete").arg("-b").arg(vault);
    for flag in extra {
        command.arg(flag);
    }
    let output = command
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("complete JSON")
}

#[test]
fn links_only_completes_note_field_with_trailing_prose() {
    let (_temp, vault) = links_vault("bob-cli-links-only-note");
    let draft = "Read [[sas because of the outage";
    let json = complete(&vault, &["-L"], 9, draft);
    assert_eq!(json["context"], "wikilink_note", "{json}");
    assert_eq!(
        json["replacement"],
        serde_json::json!({ "start": 7, "end": 9 }),
        "{json}"
    );
    let names: Vec<&str> = json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|candidate| candidate["name"].as_str().expect("name"))
        .collect();
    assert!(names.contains(&"sase"), "{json}");
}

#[test]
fn links_only_leaves_capture_grammar_literal() {
    let (_temp, vault) = links_vault("bob-cli-links-only-literal");
    // Each draft completes (or would complete) in normal mode; with
    // `--links-only` every caret stays an empty success.
    for draft in [
        "fix bug @db",
        "ask @db+fix",
        "note +",
        "Follow up ((s",
        "note %clip",
        "!",
        "plan #deep work",
        "ship it x",
        "start =deep work",
    ] {
        let json = complete(&vault, &["-L"], draft.len(), draft);
        assert!(json["context"].is_null(), "{draft}: {json}");
        assert_eq!(
            json["replacement"],
            serde_json::json!({
                "start": draft.len(),
                "end": draft.len(),
            }),
            "{draft}: {json}"
        );
    }
}

#[test]
fn links_only_same_note_field_needs_source_note() {
    let (_temp, vault) = links_vault("bob-cli-links-only-source");
    let draft = "See [[#De";
    let without = complete(&vault, &["-L"], draft.len(), draft);
    assert!(without["context"].is_null(), "{without}");

    let mut command = bob_command();
    let output = command
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .arg("-S")
        .arg("sase.md")
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run capture-complete with source note");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("complete JSON");
    assert_eq!(json["context"], "wikilink_heading", "{json}");
    let headings: Vec<&str> = json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|candidate| candidate["heading"].as_str().expect("heading"))
        .collect();
    assert_eq!(headings, vec!["Design", "Decision Log"], "{json}");
}

#[test]
fn links_only_ignores_work_log_block_suppression() {
    let (_temp, vault) = links_vault("bob-cli-links-only-work-log");
    let draft = "x\n- 1 see [[^^ab";
    let normal = complete(&vault, &[], draft.len(), draft);
    assert!(normal["context"].is_null(), "{normal}");

    let links = complete(&vault, &["-L"], draft.len(), draft);
    assert_eq!(links["context"], "wikilink_block", "{links}");
    let ids: Vec<&str> = links["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|candidate| candidate["block_id"].as_str().expect("block_id"))
        .collect();
    assert!(ids.contains(&"abc123"), "{links}");
}

#[test]
fn source_note_requires_links_only() {
    let (_temp, vault) = links_vault("bob-cli-links-only-requires");
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-S")
        .arg("sase.md")
        .arg("-c")
        .arg("9")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("See [[#De")
        .output()
        .expect("run capture-complete with source note only");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}
