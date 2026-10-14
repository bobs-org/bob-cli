//! Owner metadata and relationship-aware guards for the `+` pickers.

use crate::support::*;
use std::fs;

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn fixture() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new("bob-cli-complete-relationship-owner");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("body.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "- [/] #task Exercise ^excercise\n",
            "- [ ] #task Other owner ^other\n",
        ),
    );
    write_file(&vault.join("empty.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("foo.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "- [ ] #task Buy shoes ^bar\n",
            "  - ⛓️ **DEPENDS ON:** [[#^cycle]]\n",
            "- [ ] #task Cycle candidate ^cycle\n",
        ),
    );
    write_file(
        &vault.join("job.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Cleanup ^cleanup\n",
    );
    write_file(
        &vault.join("project.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "status: wip\n",
            "---\n",
            "- [ ] #task Project root ^prj\n",
        ),
    );
    write_file(
        &vault.join("resident.md"),
        concat!(
            "---\n",
            "parent: [[project]]\n",
            "---\n",
            "- [ ] #task Resident ^resident\n",
        ),
    );
    let day_file = vault.join("2026/20260930.md");
    write_file(&day_file, "## Pomodoros\n");
    (temp, day_file)
}

fn complete_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    text: &str,
    cursor: usize,
) -> serde_json::Value {
    serde_json::from_str(&complete_output(vault, day_file, text, cursor))
        .expect("completion JSON")
}

fn complete_output(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    text: &str,
    cursor: usize,
) -> String {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    stdout(&output).trim().to_string()
}

fn shell_values(vault: &std::path::Path, text_words: &[&str]) -> Vec<String> {
    let output = bob_command()
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .arg("bob")
        .arg("capture")
        .args(text_words)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob shell completion");
    assert_success(&output);
    stdout(&output)
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').next().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn owner_metadata_resolves_existing_alias_and_new_task_text() {
    let (temp, day_file) = fixture();
    let vault = temp.path().join("vault");

    for text in ["@body+excercise &", "^body:excercise &"] {
        let result = complete_json(&vault, &day_file, text, text.len());
        assert_eq!(result["context"], "task_dependency");
        assert_eq!(result["owner"]["kind"], "existing_task");
        assert_eq!(result["owner"]["text"], "Exercise");
        assert_eq!(result["owner"]["note_path"], "body.md");
        assert_eq!(result["owner"]["status_symbol"], "/");
        assert_eq!(result["owner"]["status_name"], "In Progress");
    }

    let result = complete_json(&vault, &day_file, "Buy milk &", 10);
    assert_eq!(result["owner"]["kind"], "new_task");
    assert_eq!(result["owner"]["text"], "Buy milk");
    assert!(result["owner"].get("note_path").is_none());
}

#[test]
fn parent_picker_reports_resolved_relationships_and_guards_rows() {
    let (temp, day_file) = fixture();
    let vault = temp.path().join("vault");

    let text = "&foo:bar +";
    let result = complete_json(&vault, &day_file, text, text.len());
    assert_eq!(result["context"], "task_parent");
    assert_eq!(
        result["picker"]["relationship"]["dependencies"][0]["raw"],
        "&foo:bar"
    );
    assert_eq!(
        result["picker"]["relationship"]["dependencies"][0]["note_path"],
        "foo.md"
    );
    assert_eq!(
        result["picker"]["relationship"]["dependencies"][0]["text"],
        "Buy shoes"
    );
    assert_eq!(
        result["picker"]["relationship"]["blocked_tasks"],
        serde_json::json!([])
    );

    let candidates = result["candidates"].as_array().expect("candidate array");
    let candidate = |name: &str| {
        candidates
            .iter()
            .find(|row| row["text"] == name)
            .unwrap_or_else(|| {
                panic!("missing candidate {name}: {candidates:?}")
            })
    };
    assert_eq!(
        candidate("Buy shoes")["disabled_reason"],
        "the prerequisite itself"
    );
    assert_eq!(
        candidate("Cycle candidate")["disabled_reason"],
        "would create a dependency cycle"
    );
    assert_eq!(candidate("Buy shoes")["replacement"], "");
    assert_eq!(candidate("Cycle candidate")["replacement"], "");

    let shell = shell_values(&vault, &["&foo:bar", "+"]);
    assert!(!shell.iter().any(|value| value == "@foo+bar"));
    assert!(!shell.iter().any(|value| value == "@foo+cycle"));

    let text = "*job:cleanup +";
    let result = complete_json(&vault, &day_file, text, text.len());
    assert_eq!(
        result["picker"]["relationship"]["blocked_tasks"][0]["note_path"],
        "job.md"
    );
    assert_eq!(
        result["picker"]["relationship"]["blocked_tasks"][0]["text"],
        "Cleanup"
    );
    let candidates = result["candidates"].as_array().expect("candidate array");
    let cleanup = candidates
        .iter()
        .find(|row| row["text"] == "Cleanup")
        .expect("cleanup candidate");
    assert_eq!(cleanup["disabled_reason"], "the blocked task itself");

    let text = "&ghost:missing +";
    let result = complete_json(&vault, &day_file, text, text.len());
    let unresolved = &result["picker"]["relationship"]["dependencies"][0];
    assert_eq!(unresolved["raw"], "&ghost:missing");
    assert_eq!(unresolved["note_path"], "ghost");
    assert_eq!(unresolved["block_id"], "missing");
    assert!(unresolved.get("text").is_none());

    let leading = "+ &foo:bar";
    let result = complete_json(&vault, &day_file, leading, 1);
    assert_eq!(result["context"], "task_parent");
    assert_eq!(result["replacement"]["start"], 0);
    assert_eq!(result["replacement"]["end"], 1);
}

#[test]
fn scoped_parent_picker_applies_the_same_relationship_guards() {
    let (temp, day_file) = fixture();
    let vault = temp.path().join("vault");
    let text = "&foo:bar @body+";
    let result = complete_json(&vault, &day_file, text, text.len());
    assert_eq!(result["context"], "task");
    assert_eq!(result["picker"]["scope"], "note");
    assert_eq!(
        result["picker"]["relationship"]["dependencies"][0]["text"],
        "Buy shoes"
    );
    let exercise = result["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .find(|row| row["text"] == "Exercise")
        .expect("Exercise candidate");
    assert!(exercise.get("disabled_reason").is_none());

    let shell = shell_values(&vault, &["&body:excercise", "@body+"]);
    assert!(!shell.iter().any(|value| value == "excercise"));
}

#[test]
fn project_cannot_depend_on_its_resident_and_unmodified_shape_is_frozen() {
    let (temp, day_file) = fixture();
    let vault = temp.path().join("vault");
    let text = "&resident:resident +";
    let result = complete_json(&vault, &day_file, text, text.len());
    let project = result["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .find(|row| row["text"] == "Project root")
        .expect("project task candidate");
    assert_eq!(
        project["disabled_reason"],
        "a project cannot depend on its own resident"
    );

    let unmodified = complete_output(&vault, &day_file, "@empty+", 7);
    assert_eq!(
        unmodified,
        r#"{"ok":true,"schema_version":1,"cursor":7,"replacement":{"start":7,"end":7},"context":"task","candidates":[],"query":"","picker":{"kind":"parent_task","scope":"note","scope_token":"@empty+","note_target":"empty.md","marker_range":{"start":0,"end":7},"trigger_removal_range":{"start":6,"end":7}}}"#
    );
}

#[test]
fn capture_complete_help_documents_additive_owner_fields() {
    let output = bob_command()
        .arg("capture-complete")
        .arg("--help")
        .output()
        .expect("capture-complete help");
    assert_success(&output);
    let help = stdout(&output);
    for phrase in [
        "picker.relationship",
        "disabled_reason",
        "status_symbol",
        "status_name",
    ] {
        assert!(help.contains(phrase), "help is missing {phrase}");
    }
}
