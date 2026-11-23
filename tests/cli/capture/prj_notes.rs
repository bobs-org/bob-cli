//! Implicit NOTES filing on a req-owning `^prj` task lands in `## Notes`.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path, process::Output};

const NOW: &str = "2026-06-15";

const PK: &str = concat!(
    "---\n",
    "type: \"[[project]]\"\n",
    "---\n",
    "- [ ] #task #prj Pk root ^prj\n",
    "\t- [ ] hand req\n",
    "\n",
    "## Tasks\n",
    "- [ ] #task Row a\n",
);

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

fn assert_notes_drawer(json: &Value, title: &str, created: bool) {
    assert_eq!(json["kind"], "sub_bullet", "{json}");
    assert_eq!(json["drawer"]["title"], title, "{json}");
    assert_eq!(json["drawer"]["key"], "NOTES", "{json}");
    assert_eq!(json["drawer"]["created"], created, "{json}");
    let notes = json["notes"].as_array().expect("notes array");
    assert!(!notes.is_empty(), "{json}");
    for note in notes {
        assert_eq!(note["drawer"]["title"], title, "{json}");
        assert_eq!(note["drawer"]["key"], "NOTES", "{json}");
    }
}

#[test]
fn two_note_captures_create_one_title_case_notes_section() {
    let temp = TempDir::new("bob-cli-prj-notes-two");
    let vault = temp.path().join("vault");
    let target = vault.join("pk.md");
    write_file(&target, PK);

    let first = capture_json(&vault, &[], "@pk+prj\n- first note");
    assert_notes_drawer(&first, "Notes", true);
    assert_eq!(first["notes"][0]["drawer"]["created"], true, "{first}");
    assert_eq!(
        fs::read_to_string(&target).expect("read after first"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "- [ ] #task #prj Pk root ^prj\n",
            "\t- [ ] hand req\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Row a\n",
            "\n",
            "## Notes\n",
            "\n",
            "- first note\n",
        )
    );

    let second = capture_json(&vault, &[], "@pk+prj\n- second note");
    assert_notes_drawer(&second, "Notes", false);
    assert_eq!(
        fs::read_to_string(&target).expect("read after second"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "- [ ] #task #prj Pk root ^prj\n",
            "\t- [ ] hand req\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Row a\n",
            "\n",
            "## Notes\n",
            "\n",
            "- first note\n",
            "- second note\n",
        )
    );
}

#[test]
fn first_req_scoops_a_loose_prj_note_into_notes() {
    let temp = TempDir::new("bob-cli-prj-notes-scoop");
    let vault = temp.path().join("vault");
    let target = vault.join("pm.md");
    write_file(
        &target,
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "- [ ] #task #prj Pm root ^prj\n",
            "\t- loose\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Row a\n",
        ),
    );
    let json = capture_json(&vault, &[], "@pm+prj\n- [ ] only req");
    let entries = json["requirements"].as_array().expect("requirements");
    assert_eq!(entries.len(), 1, "{json}");
    assert_eq!(entries[0]["added"], 1, "{json}");
    assert_eq!(entries[0]["notes_filed"], 1, "{json}");
    assert_eq!(entries[0]["notes_created"], true, "{json}");
    assert_eq!(
        fs::read_to_string(&target).expect("read after scoop"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "- [ ] #task #prj Pm root ^prj\n",
            "\t- [ ] only req\n",
            "\n",
            "## Tasks\n",
            "- [ ] #task Row a\n",
            "\n",
            "## Notes\n",
            "\n",
            "- loose\n",
        )
    );
}

#[test]
fn implicit_prj_notes_dry_run_matches_commit_and_human_names_the_section() {
    let temp = TempDir::new("bob-cli-prj-notes-preview");
    let vault = temp.path().join("vault");
    let target = vault.join("pk.md");
    write_file(&target, PK);

    let dry = capture(&vault, &["--dry-run"], "@pk+prj\n- first note", true);
    assert_success(&dry);
    let mut dry_json: Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry-run JSON");
    assert_eq!(dry_json["dry_run"], true);
    assert_notes_drawer(&dry_json, "Notes", true);
    assert_eq!(
        fs::read_to_string(&target).expect("read after dry run"),
        PK,
        "dry run must not write"
    );

    let human = capture(&vault, &["--dry-run"], "@pk+prj\n- first note", false);
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("Notes"), "{text}");
    assert!(text.contains("new drawer"), "{text}");
    assert!(text.contains("first note"), "{text}");

    let committed = capture_json(&vault, &[], "@pk+prj\n- first note");
    dry_json["dry_run"] = Value::Bool(false);
    normalize_dry_run(&mut dry_json);
    let mut committed_cmp = committed.clone();
    normalize_dry_run(&mut committed_cmp);
    assert_eq!(dry_json, committed_cmp, "dry run equals commit");
}
