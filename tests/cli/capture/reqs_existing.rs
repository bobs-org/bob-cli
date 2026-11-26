//! Requirement capture into existing tasks: req-zone insertion, the
//! first-requirement scoop, same-line reqs, and Task Link refusals.

use crate::support::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Output};

const NOW: &str = "2026-06-15";

fn capture(vault: &Path, args: &[&str], draft: &str, json: bool) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault).args(args);
    if json {
        command.arg("-f").arg("json");
    } else {
        command.env("NO_COLOR", "1");
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture")
}

fn capture_json(vault: &Path, args: &[&str], draft: &str) -> Value {
    let output = capture(vault, args, draft, true);
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn assert_requirement(
    json: &Value,
    owner: &str,
    added: u64,
    notes_filed: u64,
    notes_created: bool,
    dry_run: bool,
) {
    let entries = json["requirements"].as_array().expect("requirements");
    assert_eq!(entries.len(), 1, "{json}");
    let entry = &entries[0];
    assert_eq!(entry["owner"], owner, "{json}");
    assert_eq!(entry["added"], added, "{json}");
    assert_eq!(entry["notes_filed"], notes_filed, "{json}");
    assert_eq!(entry["notes_created"], notes_created, "{json}");
    let verb = if dry_run { "adds" } else { "added" };
    let req = if added == 1 {
        "requirement"
    } else {
        "requirements"
    };
    let mut summary = format!("◇ {verb} {added} {req} to “{owner}”");
    if notes_filed > 0 {
        let filed = if dry_run { "files" } else { "filed" };
        let note = if notes_filed == 1 { "note" } else { "notes" };
        summary
            .push_str(&format!(" · {filed} {notes_filed} {note} under NOTES"));
    }
    assert_eq!(entry["summary"], summary, "{json}");
}

fn normalize_dry_run(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if key == "dry_run" {
                    *child = Value::Bool(false);
                } else if key == "summary"
                    && let Some(summary) = child.as_str()
                {
                    *child = Value::String(
                        summary
                            .replace(" adds ", " added ")
                            .replace(" files ", " filed "),
                    );
                } else {
                    normalize_dry_run(child);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_dry_run(item);
            }
        }
        _ => {}
    }
}

#[test]
fn first_req_scoops_only_preexisting_notes_before_planning_new_notes() {
    let temp = TempDir::new("bob-cli-capture-reqs-existing-scoop");
    let vault = temp.path().join("vault");
    let target = vault.join("work.md");
    let before = concat!(
        "- [ ] #task Existing [created::2026-01-01] ^ex\n",
        "\t- old note one\n",
        "\t- old note two\n",
    );
    write_file(&target, before);
    let draft = concat!(
        "@work+ex\n",
        "- opening note\n",
        "- [ ] Add the export checksum\n",
        "- note between requirements\n",
        "- [ ] Verify the manifest\n",
        "- follow-up note",
    );

    let dry = capture(&vault, &["--dry-run"], draft, true);
    assert_success(&dry);
    let dry_json: Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry-run JSON");
    assert_requirement(&dry_json, "Existing", 2, 2, true, true);
    assert_eq!(
        fs::read_to_string(&target).expect("read after dry run"),
        before,
        "dry run must not write"
    );

    let human = capture(&vault, &["--dry-run"], draft, false);
    assert_success(&human);
    assert!(
        stdout(&human).contains(
            "◇ adds 2 requirements to “Existing” · files 2 notes under NOTES"
        ),
        "{}",
        format_output(&human)
    );

    let json = capture_json(&vault, &[], draft);
    assert_requirement(&json, "Existing", 2, 2, true, false);
    assert_eq!(json["notes"].as_array().expect("notes").len(), 3, "{json}");
    let mut normalized_dry = dry_json;
    normalize_dry_run(&mut normalized_dry);
    assert_eq!(normalized_dry, json, "dry-run and commit previews diverged");
    assert_eq!(
        fs::read_to_string(&target).expect("read target"),
        concat!(
            "- [ ] #task Existing [created::2026-01-01] ^ex\n",
            "\t- [ ] Add the export checksum\n",
            "\t- [ ] Verify the manifest\n",
            "\t- **Notes**\n",
            "\t\t- old note one\n",
            "\t\t- old note two\n",
            "\t\t- opening note\n",
            "\t\t- note between requirements\n",
            "\t\t- follow-up note\n",
        )
    );
}

#[test]
fn req_only_capture_keeps_notes_drawer_nonempty_and_existing_reqs_append() {
    let temp = TempDir::new("bob-cli-capture-reqs-existing-zone");
    let vault = temp.path().join("vault");
    let target = vault.join("work.md");
    write_file(
        &target,
        "- [ ] #task Empty owner ^empty\n- [ ] #task Has req ^has\n\t- [ ] Existing req\n\t- **Notes**\n\t\t- Existing context\n",
    );

    let empty_json = capture_json(&vault, &[], "@work+empty\n- [ ] First req");
    assert_requirement(&empty_json, "Empty owner", 1, 0, false, false);
    let after_empty = fs::read_to_string(&target).expect("read work note");
    assert!(
        after_empty.contains(
            "- [ ] #task Empty owner ^empty\n\t- [ ] First req\n- [ ] #task Has req"
        ),
        "{after_empty}"
    );
    let empty_owner = after_empty
        .split("- [ ] #task Has req")
        .next()
        .expect("empty owner prefix");
    assert!(!empty_owner.contains("NOTES\n"), "{empty_owner}");

    let json =
        capture_json(&vault, &[], "@work+has\n- note to file\n- [ ] Added req");
    assert_requirement(&json, "Has req", 1, 0, false, false);
    assert_eq!(json["notes"].as_array().expect("notes").len(), 1, "{json}");
    assert_eq!(
        fs::read_to_string(&target).expect("read work note"),
        concat!(
            "- [ ] #task Empty owner ^empty\n",
            "\t- [ ] First req\n",
            "- [ ] #task Has req ^has\n",
            "\t- [ ] Existing req\n",
            "\t- [ ] Added req\n",
            "\t- **Notes**\n",
            "\t\t- Existing context\n",
            "\t\t- note to file\n",
        )
    );
}

#[test]
fn same_line_req_and_its_children_insert_as_one_block() {
    let temp = TempDir::new("bob-cli-capture-reqs-existing-same-line");
    let vault = temp.path().join("vault");
    let target = vault.join("work.md");
    write_file(&target, "- [ ] #task Owner ^ex\n\t- existing loose note\n");
    let json = capture_json(
        &vault,
        &[],
        "@work+ex [ ] Handles CRLF\n- [ ] Windows line endings\n  - Fixture: windows.csv",
    );
    assert_requirement(&json, "Owner", 2, 1, true, false);
    assert_eq!(
        fs::read_to_string(&target).expect("read work note"),
        concat!(
            "- [ ] #task Owner ^ex\n",
            "\t- [ ] Handles CRLF\n",
            "\t\t- [ ] Windows line endings\n",
            "\t\t\t- Fixture: windows.csv\n",
            "\t- **Notes**\n",
            "\t\t- existing loose note\n",
        )
    );
}

#[test]
fn nested_requirements_count_at_every_depth_like_new_blocks() {
    let temp = TempDir::new("bob-cli-capture-reqs-existing-count");
    let vault = temp.path().join("vault");
    let target = vault.join("work.md");
    write_file(&target, "- [ ] #task Owner ^ex\n");
    let draft = concat!(
        "@work+ex\n",
        "- [ ] Outer\n",
        "  - [ ] Inner\n",
        "    - [ ] Deepest\n",
        "  - note of outer\n",
        "- [ ] Second",
    );
    let json = capture_json(&vault, &["--dry-run"], draft);
    assert_requirement(&json, "Owner", 4, 0, false, true);
    let json = capture_json(&vault, &[], draft);
    assert_requirement(&json, "Owner", 4, 0, false, false);
    assert_eq!(
        fs::read_to_string(&target).expect("read work note"),
        concat!(
            "- [ ] #task Owner ^ex\n",
            "\t- [ ] Outer\n",
            "\t\t- [ ] Inner\n",
            "\t\t\t- [ ] Deepest\n",
            "\t\t- note of outer\n",
            "\t- [ ] Second\n",
        )
    );
}

#[test]
fn forced_task_and_task_ref_targets_write_same_line_requirements() {
    let temp = TempDir::new("bob-cli-capture-reqs-existing-forced");
    let vault = temp.path().join("vault");
    let target = vault.join("work.md");
    let parent = "- [ ] #task ID owner ^ex";
    let idless = "- [ ] #task Ref owner";
    write_file(&target, &format!("{parent}\n{idless}\n"));

    let id_capture = capture(
        &vault,
        &["--route", "work", "--task", "ex"],
        "[ ] Forced requirement\n- [ ] Forced child",
        true,
    );
    assert_success(&id_capture);

    let digest = hex::encode(Sha256::digest(idless.as_bytes()));
    let task_ref = format!("2:{}", &digest[..8]);
    let ref_capture = capture(
        &vault,
        &["--route", "work", "--task-ref", &task_ref],
        "[ ] Ref requirement",
        true,
    );
    assert_success(&ref_capture);
    let note = fs::read_to_string(&target).expect("read work note");
    assert!(
        note.contains("\t- [ ] Forced requirement\n\t\t- [ ] Forced child\n"),
        "{note}"
    );
    assert!(note.contains("\t- [ ] Ref requirement\n"), "{note}");
}

#[test]
fn implicit_notes_on_a_req_owner_reports_the_drawer() {
    let temp = TempDir::new("bob-cli-capture-reqs-implicit-notes");
    let vault = temp.path().join("vault");
    let target = vault.join("work.md");
    let before =
        "- [ ] #task Existing [created::2026-01-01] ^ex\n\t- [ ] a req\n";
    write_file(&target, before);
    let draft = "@work+ex\n- first note";

    let dry = capture(&vault, &["--dry-run"], draft, true);
    assert_success(&dry);
    let mut dry_json: Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry-run JSON");
    assert_eq!(dry_json["dry_run"], true, "{dry_json}");
    assert_eq!(dry_json["drawer"]["title"], "Notes", "{dry_json}");
    assert_eq!(dry_json["drawer"]["key"], "NOTES", "{dry_json}");
    assert_eq!(dry_json["drawer"]["created"], true, "{dry_json}");
    assert_eq!(
        dry_json["notes"][0]["drawer"]["title"], "Notes",
        "{dry_json}"
    );
    assert_eq!(
        dry_json["notes"][0]["drawer"]["created"], true,
        "{dry_json}"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read after dry run"),
        before,
        "dry run must not write"
    );

    let human = capture(&vault, &["--dry-run"], draft, false);
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("· Notes ·"), "{text}");
    assert!(text.contains("new drawer"), "{text}");
    assert!(text.contains("first note"), "{text}");

    let committed = capture_json(&vault, &[], draft);
    dry_json["dry_run"] = Value::Bool(false);
    normalize_dry_run(&mut dry_json);
    let mut committed_cmp = committed.clone();
    normalize_dry_run(&mut committed_cmp);
    assert_eq!(dry_json, committed_cmp, "dry run equals commit");
    assert_eq!(
        fs::read_to_string(&target).expect("read after commit"),
        concat!(
            "- [ ] #task Existing [created::2026-01-01] ^ex\n",
            "\t- [ ] a req\n",
            "\t- **Notes**\n",
            "\t\t- first note\n",
        )
    );
}

#[test]
fn project_root_task_accepts_existing_requirement_groups() {
    let temp = TempDir::new("bob-cli-capture-reqs-existing-project");
    let vault = temp.path().join("vault");
    let target = vault.join("proj.md");
    write_file(
        &target,
        concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "- [ ] #task #prj Project root ^prj\n",
        ),
    );
    let json =
        capture_json(&vault, &[], "@proj+prj\n- [ ] First project requirement");
    assert_requirement(&json, "#prj Project root", 1, 0, false, false);
    let note = fs::read_to_string(target).expect("read project note");
    assert!(
        note.contains("\t- [ ] First project requirement\n"),
        "{note}"
    );
}

#[test]
fn task_links_to_requirement_anchors_refuse_with_owner_copy() {
    let temp = TempDir::new("bob-cli-capture-req-link");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("work.md"),
        "- [ ] #task Export owner ^owner\n\t- [ ] Preserve CRLF ^req\n",
    );
    for draft in ["@work:req", "@work:req=x"] {
        let output = capture(&vault, &[], draft, true);
        assert!(!output.status.success(), "{}", format_output(&output));
        let json: Value =
            serde_json::from_str(stdout(&output).trim()).expect("error JSON");
        assert_eq!(json["code"], "req_link", "{json}");
        assert!(
            json["error"].as_str().expect("error string").contains(
                "Requirements aren't planned — link “Export owner”, or Ctrl+Shift+T makes it a task"
            ),
            "{json}"
        );
    }
}

#[test]
fn failed_note_planning_rolls_back_the_req_insert_and_scoop() {
    let temp = TempDir::new("bob-cli-capture-reqs-existing-rollback");
    let vault = temp.path().join("vault");
    let target = vault.join("work.md");
    let before = "- [ ] #task Owner ^ex\n\t- loose context\n".to_string();
    write_file(&target, &before);
    let output = capture(
        &vault,
        &[],
        "@work+ex\n- [ ] New req\n- #tasks\n  - forbidden drawer item",
        true,
    );
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(
        fs::read_to_string(&target).expect("read after refusal"),
        before,
        "the staged req and scoop must roll back"
    );
}
