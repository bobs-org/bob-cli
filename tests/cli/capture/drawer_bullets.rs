//! First-level drawer bullets on new tasks and new project notes.

use crate::support::*;
use std::fs;

fn capture_json(vault: &std::path::Path, draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn capture_error(vault: &std::path::Path, draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run refused capture");
    assert!(!output.status.success(), "{}", format_output(&output));
    serde_json::from_str(stdout(&output).trim()).expect("error JSON")
}

#[test]
fn new_task_shortcut_renders_the_all_caps_drawer() {
    let temp = TempDir::new("bob-cli-drawer-bullet-new-task");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let json = capture_json(
        &vault,
        "@work Ship Postgres\n- #future-work\n  - try logical replication",
    );
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["\t- FUTURE WORK", "\t\t- try logical replication"]),
        "{json}"
    );
    let note = fs::read_to_string(vault.join("work.md")).expect("work note");
    assert!(note.contains("\t- FUTURE WORK\n"), "{note}");
    assert!(note.contains("\t\t- try logical replication\n"), "{note}");
}

#[test]
fn catalog_spelling_wins_for_a_slug() {
    let temp = TempDir::new("bob-cli-drawer-bullet-catalog");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("seed.md"),
        concat!(
            "- [ ] #task Seed drawers [created::2026-01-01] ^seed\n",
            "\t- NON-GOALS\n",
            "\t\t- skip the rewrite\n",
        ),
    );
    let json = capture_json(
        &vault,
        "@work Cut the scope\n- #non-goals\n  - no rewrite pass",
    );
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["\t- NON-GOALS", "\t\t- no rewrite pass"]),
        "{json}"
    );
}

#[test]
fn repeated_keys_merge_in_draft_order() {
    let temp = TempDir::new("bob-cli-drawer-bullet-merge");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let json = capture_json(
        &vault,
        "@work Ship it\n- #future-work\n  - first\n- FUTURE WORK\n  - second",
    );
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["\t- FUTURE WORK", "\t\t- first", "\t\t- second"]),
        "{json}"
    );
}

#[test]
fn reserved_childless_and_text_refusals() {
    let temp = TempDir::new("bob-cli-drawer-bullet-refuse");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let reserved = capture_error(
        &vault,
        "@work Ship it\n- #schedule-log\n  - a managed line",
    );
    assert_eq!(reserved["code"], "reserved_drawer", "{reserved}");
    let empty = capture_error(&vault, "@work Ship it\n- #future-work");
    assert_eq!(empty["code"], "empty_drawer_bullet", "{empty}");
    let extra =
        capture_error(&vault, "@work Ship it\n- #requirements Postgres 17");
    assert_eq!(extra["code"], "drawer_bullet_text", "{extra}");
}

#[test]
fn same_line_note_is_not_under_task() {
    let temp = TempDir::new("bob-cli-drawer-bullet-position");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("foo.md"),
        "- [ ] #task Existing [created::2026-01-01] ^bar\n",
    );
    let json = capture_error(
        &vault,
        "@foo+bar a same-line note\n- #future-work\n  - nested",
    );
    assert_eq!(json["code"], "drawer_bullet_position", "{json}");
}

#[test]
fn childless_all_caps_stays_a_plain_bullet() {
    let temp = TempDir::new("bob-cli-drawer-bullet-nyc");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let json = capture_json(&vault, "@work Plan trip\n- NYC");
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["\t- NYC"]),
        "{json}"
    );
}

#[test]
fn project_note_shortcut_becomes_a_section() {
    let temp = TempDir::new("bob-cli-drawer-bullet-prj");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash^auth1@ Parent\n- #future-work\n  - nested detail")
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .output()
        .expect("run project-note capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(
        json["project_note"]["sections"],
        serde_json::json!(["Future Work"]),
        "{json}"
    );
    let contents =
        fs::read_to_string(vault.join("cash_auth1.md")).expect("note");
    assert!(contents.contains("## Future Work"), "{contents}");
    assert!(contents.contains("- nested detail"), "{contents}");
}

// ---------------------------------------------------------------------------
// Reserved drawer names: capture-parse, the dry-run preview, and the commit
// agree (the editor's lexical diagnostic uses the resolver's code and copy).
// ---------------------------------------------------------------------------

fn parse_json(draft: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

fn diagnostic_codes(parse: &serde_json::Value) -> Vec<String> {
    parse["diagnostics"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row["code"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Run the draft through the dry-run preview and the real commit.
fn preview_and_commit(
    vault: &std::path::Path,
    draft: &str,
) -> (
    Result<serde_json::Value, serde_json::Value>,
    Result<serde_json::Value, serde_json::Value>,
) {
    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(vault)
            .arg("-f")
            .arg("json");
        if dry_run {
            command.arg("--dry-run");
        }
        let output = command
            .arg("--")
            .arg(draft)
            .env("BOB_NOW", "2026-06-15")
            .output()
            .expect("run capture");
        let value: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        if output.status.success() {
            Ok(value)
        } else {
            Err(value)
        }
    };
    (run(true), run(false))
}

fn vault_files(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read vault dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push((
                    path.strip_prefix(root)
                        .expect("relative")
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(&path).expect("read file"),
                ));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn reserved_new_task_drafts_agree_across_parse_preview_and_commit() {
    for draft in [
        "Ship it\n- #tasks\n  - note",
        "Ship it\n- TASKS\n  - note",
        "Ship it\n- #schedule-log\n  - note",
        "Ship it\n- #\"Work Log\"\n  - note",
        "Ship it\n- LINKS\n  - note",
        "Ship it\n- #future-work\n  - fine\n- #tasks\n  - not fine",
    ] {
        let temp = TempDir::new("bob-cli-drawer-reserved-new-task");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        let before = vault_files(&vault);

        let parse = parse_json(draft);
        assert!(
            diagnostic_codes(&parse).contains(&"reserved_drawer".to_string()),
            "{draft:?}: {parse}"
        );
        let (preview, commit) = preview_and_commit(&vault, draft);
        let preview = preview.expect_err("preview refuses");
        let commit = commit.expect_err("commit refuses");
        assert_eq!(preview["code"], "reserved_drawer", "{draft:?}: {preview}");
        assert_eq!(commit["code"], "reserved_drawer", "{draft:?}: {commit}");
        assert_eq!(vault_files(&vault), before, "{draft:?} wrote files");
    }
}

#[test]
fn reserved_diagnostic_copy_matches_the_refusal() {
    let temp = TempDir::new("bob-cli-drawer-reserved-copy");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let draft = "Ship it\n- #tasks\n  - note";
    let parse = parse_json(draft);
    let diagnostic = parse["diagnostics"]
        .as_array()
        .and_then(|rows| {
            rows.iter().find(|row| row["code"] == "reserved_drawer")
        })
        .expect("reserved diagnostic")
        .clone();
    let refusal = capture_error(&vault, draft);
    let error = refusal["error"].as_str().expect("refusal text");
    let message = diagnostic["message"].as_str().expect("diagnostic text");
    assert!(
        error.ends_with(message),
        "refusal {error:?} should carry the editor copy {message:?}"
    );
    // The range covers exactly the `#tasks` shortcut, byte for byte.
    let range = diagnostic["range"].as_array().expect("range");
    let (start, end) = (
        range[0].as_u64().expect("start") as usize,
        range[1].as_u64().expect("end") as usize,
    );
    assert_eq!(&draft[start..end], "#tasks");
}

#[test]
fn legal_drawer_controls_agree_across_parse_preview_and_commit() {
    for draft in [
        "Ship it\n- #future-work\n  - note",
        "Ship it\n- FUTURE WORK\n  - note",
        // A childless literal ALL-CAPS bullet stays a plain note, even for
        // a managed word.
        "Ship it\n- TASKS",
        "Ship it\n- NYC",
        // An escaped hash is prose.
        "Ship it\n- \\#tasks\n  - note",
    ] {
        let temp = TempDir::new("bob-cli-drawer-reserved-control");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        let parse = parse_json(draft);
        assert!(
            !diagnostic_codes(&parse).contains(&"reserved_drawer".to_string()),
            "{draft:?}: {parse}"
        );
        let (preview, commit) = preview_and_commit(&vault, draft);
        preview.unwrap_or_else(|error| panic!("{draft:?} preview: {error}"));
        commit.unwrap_or_else(|error| panic!("{draft:?} commit: {error}"));
    }
}

#[test]
fn new_project_note_structural_sections_stay_legal_everywhere() {
    for (draft, note) in [
        (
            "Parent @cash^p1@\n- TASKS\n  - merged\n- Real",
            "cash_p1.md",
        ),
        (
            "Parent @cash^p2@\n- #tasks\n  - merged\n- Real",
            "cash_p2.md",
        ),
    ] {
        let temp = TempDir::new("bob-cli-drawer-reserved-prj");
        let vault = temp.path().join("vault");
        write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
        let parse = parse_json(draft);
        assert_eq!(parse["mode"], "project_note", "{parse}");
        assert!(diagnostic_codes(&parse).is_empty(), "{draft:?}: {parse}");
        let (preview, commit) = preview_and_commit(&vault, draft);
        preview.unwrap_or_else(|error| panic!("{draft:?} preview: {error}"));
        commit.unwrap_or_else(|error| panic!("{draft:?} commit: {error}"));
        let contents = fs::read_to_string(vault.join(note)).expect("note");
        assert_eq!(contents.matches("## Tasks").count(), 1, "{contents}");
        assert!(contents.contains("- merged"), "{contents}");
    }
}
