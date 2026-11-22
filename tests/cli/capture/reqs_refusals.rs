//! `bob capture` requirement bullets: refusal codes, dry-run parity with
//! `capture-parse`, forced targets, and depth-preserving writes of levels
//! past two.

use crate::support::*;
use std::{fs, path::Path};

const NOW: &str = "2026-06-15";

fn vault() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new("bob-cli-capture-reqs");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("work.md"),
        "- [ ] #task Existing [created::2026-01-01] ^ex\n\t- loose note\n",
    );
    (temp, vault)
}

fn capture(vault: &Path, args: &[&str], draft: &str) -> std::process::Output {
    bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .args(args)
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture")
}

fn refusal(vault: &Path, args: &[&str], draft: &str) -> serde_json::Value {
    let output = capture(vault, args, draft);
    assert!(!output.status.success(), "{}", format_output(&output));
    serde_json::from_str(stdout(&output).trim()).expect("error JSON")
}

fn parse(text: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

#[test]
fn refusals_report_the_shared_codes_and_copy() {
    let (_temp, vault) = vault();
    for (draft, code, copy) in [
        (
            "Fix\n- note\n  - [ ] bad",
            "req_parent",
            "A requirement needs a task or requirement directly above it",
        ),
        (
            "Fix\n- [x] done",
            "req_state",
            "Requirements start open — they're met when the task closes",
        ),
        (
            "Fix\n- #future-work\n  - [ ] bad",
            "req_in_drawer",
            "Drawers hold notes — put the requirement directly under the task",
        ),
        (
            "@work+ex#notes [ ] bad",
            "req_in_drawer",
            "Drawers hold notes — put the requirement directly under the task",
        ),
        (
            "@work+ex [x] done",
            "req_state",
            "Requirements start open — they're met when the task closes",
        ),
        (
            "@work+ex Call Morgan\n- [ ] bad",
            "req_parent",
            "A requirement needs a task or requirement directly above it",
        ),
    ] {
        let json = refusal(&vault, &["--dry-run"], draft);
        assert_eq!(json["code"], code, "{draft:?}: {json}");
        assert!(
            json["error"].as_str().unwrap_or_default().ends_with(copy),
            "{draft:?}: {json}"
        );
    }
}

#[test]
fn a_third_level_note_keeps_invalid_child_line_with_the_requirement_hint() {
    let (_temp, vault) = vault();
    let json = refusal(
        &vault,
        &["--dry-run"],
        "Fix\n- note\n  - nested\n    - deep",
    );
    assert_eq!(json["code"], "invalid_child_line", "{json}");
    assert!(
        json["error"].as_str().unwrap_or_default().ends_with(
            "deeper bullets nest only beneath a requirement (`- [ ] …`)"
        ),
        "{json}"
    );
}

#[test]
fn the_depth_bound_admits_sixteen_and_refuses_seventeen() {
    let (_temp, vault) = vault();
    let chain = |levels: usize| {
        let mut draft = String::from("Deep @work");
        for level in 0..levels {
            draft.push('\n');
            draft.push_str(&"  ".repeat(level));
            draft.push_str(&format!("- [ ] level {}", level + 1));
        }
        draft
    };
    let output = capture(&vault, &["--dry-run"], &chain(16));
    assert_success(&output);
    let json = refusal(&vault, &["--dry-run"], &chain(17));
    assert_eq!(json["code"], "req_depth", "{json}");
}

#[test]
fn forced_targets_re_decide_requirement_positions() {
    let (_temp, vault) = vault();
    let same_line = capture(
        &vault,
        &["--dry-run", "-r", "work", "-t", "ex"],
        "[ ] Handles CRLF\n- [ ] sub",
    );
    assert_success(&same_line);
    let json = refusal(
        &vault,
        &["--dry-run", "-r", "work", "-t", "ex"],
        "Call Morgan\n- [ ] bad",
    );
    assert_eq!(json["code"], "req_parent", "{json}");
    let json = refusal(
        &vault,
        &["--dry-run", "-r", "work", "-t", "ex", "-D", "NOTES"],
        "[ ] bad",
    );
    assert_eq!(json["code"], "req_in_drawer", "{json}");
}

/// The `bob capture --dry-run` refusal code equals the first
/// `capture-parse` diagnostic code over one corpus, and accepted drafts
/// agree on requirement depths.
#[test]
fn dry_run_and_capture_parse_agree_on_codes_and_depths() {
    let (_temp, vault) = vault();
    for draft in [
        "Fix\n- note\n  - [ ] bad",
        "Fix\n- [x] done",
        "Fix\n- NOTES\n  - [ ] bad",
        "Fix\n- note\n  - nested\n    - deep",
        "Fix\n- [ ] a\n      - jump",
        "@work+ex#notes\n- [ ] bad",
        "@work+ex Call Morgan\n- [ ] bad",
        "@work+ex [x] done",
        "@work\n- Task A\n  - note\n    - deep",
    ] {
        let json = refusal(&vault, &["--dry-run"], draft);
        let parsed = parse(draft);
        let first = &parsed["diagnostics"][0]["code"];
        assert_eq!(&json["code"], first, "{draft:?}: {json} vs {parsed}");
    }
    for draft in [
        "@work Fix\n- [ ] a\n  - [ ] b\n    - [ ] c\n      - note\n- plain",
        "@work+ex\n- [ ] a\n  - [ ] b\n    - [ ] c",
        "@work+ex [ ] Handles CRLF\n- [ ] sub\n  - [ ] subsub",
    ] {
        let output = capture(&vault, &["--dry-run"], draft);
        assert_success(&output);
        let parsed = parse(draft);
        assert!(
            parsed.get("diagnostics").is_none()
                || parsed["diagnostics"]
                    .as_array()
                    .is_some_and(|items| items.is_empty()),
            "{draft:?}: {parsed}"
        );
        let depths: Vec<u64> = parsed["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .filter(|span| span["kind"] == "req_marker")
            .map(|span| span["depth"].as_u64().expect("depth"))
            .collect();
        assert!(
            depths.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{draft:?}: {depths:?}"
        );
        assert_eq!(depths.len(), 3, "{draft:?}: {parsed}");
    }
}

#[test]
fn a_new_task_writes_requirements_past_two_levels_at_their_depth() {
    let (_temp, vault) = vault();
    let output = capture(
        &vault,
        &[],
        "@work Fix export\n- [ ] Handles CRLF\n  - [ ] Windows\n    - [ ] Mixed files\n      - seen in March\n- see the ticket",
    );
    assert_success(&output);
    let note = fs::read_to_string(vault.join("work.md")).expect("work note");
    assert!(
        note.contains(
            "- [ ] #task Fix export [created::2026-06-15]\n\t- [ ] Handles CRLF\n\t\t- [ ] Windows\n\t\t\t- [ ] Mixed files\n\t\t\t\t- seen in March\n\t- NOTES\n\t\t- see the ticket\n"
        ),
        "{note}"
    );
}

#[test]
fn a_multi_note_existing_task_writes_requirements_at_their_depth() {
    let (_temp, vault) = vault();
    let draft = "@work+ex\n- [ ] a\n  - [ ] b\n    - [ ] c\n      - note c";
    let dry = capture(&vault, &["--dry-run"], draft);
    assert_success(&dry);
    let output = capture(&vault, &[], draft);
    assert_success(&output);
    let note = fs::read_to_string(vault.join("work.md")).expect("work note");
    assert!(
        note.contains(
            "\t- [ ] a\n\t\t- [ ] b\n\t\t\t- [ ] c\n\t\t\t\t- note c\n"
        ),
        "{note}"
    );
}

#[test]
fn the_same_line_form_writes_its_requirement_chain_at_depth() {
    let (_temp, vault) = vault();
    let output = capture(
        &vault,
        &[],
        "@work+ex [ ] Handles CRLF\n- [ ] sub\n  - [ ] subsub\n    - why",
    );
    assert_success(&output);
    let note = fs::read_to_string(vault.join("work.md")).expect("work note");
    assert!(
        note.contains(
            "\t- [ ] Handles CRLF\n\t\t- [ ] sub\n\t\t\t- [ ] subsub\n\t\t\t\t- why\n"
        ),
        "{note}"
    );
}

#[test]
fn a_project_note_row_writes_its_requirements_at_depth() {
    let temp = TempDir::new("bob-cli-capture-reqs-project");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Ship it @cash^ship@\n- Row one\n  - [ ] req\n    - [ ] deeper\n      - why")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .output()
        .expect("run capture");
    assert_success(&output);
    let note =
        fs::read_to_string(vault.join("cash_ship.md")).expect("project note");
    assert!(
        note.contains("\t- [ ] req\n\t\t- [ ] deeper\n\t\t\t- why\n"),
        "{note}"
    );
}

#[test]
fn a_placeholder_requirement_writes_nothing() {
    let (_temp, vault) = vault();
    let output = capture(&vault, &[], "@work Fix\n- [ ] \n- note");
    assert_success(&output);
    let note = fs::read_to_string(vault.join("work.md")).expect("work note");
    assert!(
        note.contains("- [ ] #task Fix [created::2026-06-15]\n\t- note\n"),
        "{note}"
    );
}
