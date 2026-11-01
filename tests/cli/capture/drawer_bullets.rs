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
