//! New-task, list, Pomodoro-created, and project-note requirement writes:
//! reqs zone, NOTES rule, additive JSON and human summary.

use crate::support::*;
use serde_json::Value;
use std::{fs, path::Path};

const NOW: &str = "2026-06-15";

fn vault() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new("bob-cli-capture-reqs-new");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("work.md"),
        "- [ ] #task Existing [created::2026-01-01] ^ex\n\t- loose note\n",
    );
    (temp, vault)
}

fn capture(
    vault: &Path,
    args: &[&str],
    draft: &str,
    json: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if json {
        command.arg("-f").arg("json");
    } else {
        command.env("NO_COLOR", "1");
    }
    command
        .args(args)
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

fn mask_requirement_summaries(value: &mut Value) {
    if let Some(entries) =
        value.get_mut("requirements").and_then(Value::as_array_mut)
    {
        for entry in entries {
            if let Some(summary) = entry
                .get("summary")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                entry["summary"] = Value::String(
                    summary
                        .replace(" adds ", " added ")
                        .replace(" files ", " filed "),
                );
            }
        }
    }
    if let Some(Value::Array(items)) = value.get_mut("captures") {
        for item in items {
            mask_requirement_summaries(item);
        }
    }
}

fn assert_dry_matches_real(dry: &Value, real: &Value) {
    let mut masked = dry.clone();
    masked["dry_run"] = Value::Bool(false);
    if let Some(Value::Array(items)) = masked.get_mut("captures") {
        for item in items {
            item["dry_run"] = Value::Bool(false);
        }
    }
    mask_requirement_summaries(&mut masked);
    assert_eq!(
        masked, *real,
        "dry-run JSON must equal real-run JSON except dry_run and summary verbs"
    );
}

fn assert_req_entry(
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
    let added_verb = if dry_run { "adds" } else { "added" };
    let req_word = if added == 1 {
        "requirement"
    } else {
        "requirements"
    };
    let mut expected =
        format!("◇ {added_verb} {added} {req_word} to “{owner}”");
    if notes_filed > 0 {
        let filed_verb = if dry_run { "files" } else { "filed" };
        let note_word = if notes_filed == 1 { "note" } else { "notes" };
        expected.push_str(&format!(
            " · {filed_verb} {notes_filed} {note_word} under NOTES"
        ));
    }
    assert_eq!(entry["summary"], expected, "{json}");
}

fn read_work(vault: &Path) -> String {
    fs::read_to_string(vault.join("work.md")).expect("work note")
}

fn tidy_reports_nothing(vault: &Path) {
    let output = bob_command()
        .args(["task", "tidy", "-f", "json"])
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob task tidy");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("tidy JSON");
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["counts"]["notes_changed"], 0, "{json}");
    assert_eq!(json["counts"]["tasks_changed"], 0, "{json}");
    assert_eq!(json["changes"], serde_json::json!([]), "{json}");
}

#[test]
fn a_new_task_files_notes_into_notes_and_reports_the_write() {
    let (_temp, vault) = vault();
    let draft = "@work Fix export\n- [ ] Handles CRLF\n  - [ ] Windows\n    - [ ] Mixed files\n      - seen in March\n- see the ticket";
    let dry = capture_json(&vault, &["--dry-run"], draft);
    assert_req_entry(&dry, "Fix export", 3, 1, true, true);
    assert_eq!(
        dry["sub_bullets"],
        serde_json::json!([
            "\t- [ ] Handles CRLF",
            "\t\t- [ ] Windows",
            "\t\t\t- [ ] Mixed files",
            "\t\t\t\t- seen in March",
            "\t- see the ticket",
        ]),
        "{dry}"
    );

    let json = capture_json(&vault, &[], draft);
    assert_dry_matches_real(&dry, &json);
    assert_req_entry(&json, "Fix export", 3, 1, true, false);
    let note = read_work(&vault);
    assert!(
        note.contains(concat!(
            "- [ ] #task Fix export [created::2026-06-15]\n",
            "\t- [ ] Handles CRLF\n",
            "\t\t- [ ] Windows\n",
            "\t\t\t- [ ] Mixed files\n",
            "\t\t\t\t- seen in March\n",
            "\t- NOTES\n",
            "\t\t- see the ticket\n",
        )),
        "{note}"
    );
    tidy_reports_nothing(&vault);
}

#[test]
fn a_named_new_task_and_a_linked_pomodoro_task_write_the_same_layout() {
    let (_temp, vault) = vault();
    let day = vault.join("day.md");
    write_file(&day, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    let named = capture_json(
        &vault,
        &[],
        "@work^export Fix named\n- [ ] Handles CRLF\n- see the ticket",
    );
    assert_eq!(named["kind"], "task", "{named}");
    assert_eq!(named["block_id"], "export", "{named}");
    assert_req_entry(&named, "Fix named", 1, 1, true, false);

    let linked = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@work:linked Fix linked\n- [ ] Handles CRLF\n- see the ticket")
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", &day)
        .output()
        .expect("run linked capture");
    assert_success(&linked);
    let json: Value =
        serde_json::from_str(stdout(&linked).trim()).expect("linked JSON");
    assert_eq!(json["kind"], "pomodoro_task", "{json}");
    assert_req_entry(&json, "Fix linked", 1, 1, true, false);

    let note = read_work(&vault);
    assert!(
        note.contains(concat!(
            "- [ ] #task Fix named [created::2026-06-15] ^export\n",
            "\t- [ ] Handles CRLF\n",
            "\t- NOTES\n",
            "\t\t- see the ticket\n",
        )),
        "{note}"
    );
    assert!(
        note.contains(
            "\t- [ ] Handles CRLF\n\t- NOTES\n\t\t- see the ticket\n"
        ),
        "linked row files NOTES:\n{note}"
    );
    assert!(
        note.contains("^linked") || note.contains("Fix linked"),
        "{note}"
    );
}

#[test]
fn a_task_list_canonicalizes_each_owner_and_leaves_zero_req_rows() {
    let (_temp, vault) = vault();
    let draft = concat!(
        "@work\n",
        "- Ship the parser\n",
        "  - [ ] Handles CRLF\n",
        "  - see the ticket\n",
        "- Just a note task\n",
        "  - leftover\n",
    );
    let json = capture_json(&vault, &[], draft);
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2, "{json}");
    assert_req_entry(&captures[0], "Ship the parser", 1, 1, true, false);
    assert!(captures[1].get("requirements").is_none(), "{json}");
    let note = read_work(&vault);
    assert!(
        note.contains(concat!(
            "- [ ] #task Ship the parser [created::2026-06-15]\n",
            "\t- [ ] Handles CRLF\n",
            "\t- NOTES\n",
            "\t\t- see the ticket\n",
        )),
        "{note}"
    );
    assert!(
        note.contains(concat!(
            "- [ ] #task Just a note task [created::2026-06-15]\n",
            "\t- leftover\n",
        )),
        "{note}"
    );
    assert!(
        !note.contains("- NOTES\n\t\t- leftover"),
        "zero-req row stays byte-identical:\n{note}"
    );
}

#[test]
fn a_project_note_row_files_its_loose_note_into_notes() {
    let temp = TempDir::new("bob-cli-capture-reqs-new-project");
    let vault = temp.path().join("vault");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    let draft = concat!(
        "Ship it @cash^ship@\n",
        "- Row one\n",
        "  - [ ] req\n",
        "    - [ ] deeper\n",
        "      - why\n",
        "  - Ask about fees\n",
        "- Book the flight\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0")
        .output()
        .expect("run capture");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "project_note", "{json}");
    assert_req_entry(&json, "Row one", 2, 1, true, false);
    let note =
        fs::read_to_string(vault.join("cash_ship.md")).expect("project note");
    assert!(
        note.contains(concat!(
            "- [ ] #task Row one [created::2026-09-20]\n",
            "\t- [ ] req\n",
            "\t\t- [ ] deeper\n",
            "\t\t\t- why\n",
            "\t- NOTES\n",
            "\t\t- Ask about fees\n",
            "- [ ] #task Book the flight [created::2026-09-20]\n",
        )),
        "{note}"
    );
}

#[test]
fn clip_notes_with_reqs_file_into_notes() {
    let temp = TempDir::new("bob-cli-capture-reqs-new-clip");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(&vault.join("work.md"), "# Work\n## Tasks\n");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'from clip\\n'\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fix export @work %c\n- [ ] Handles CRLF\n- see the ticket")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", NOW)
        .output()
        .expect("clip capture");
    assert_success(&output);
    let json: Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_req_entry(&json, "Fix export", 1, 2, true, false);
    let note = fs::read_to_string(vault.join("work.md")).expect("work note");
    assert!(
        note.contains(concat!(
            "- [ ] #task Fix export [created::2026-06-15]\n",
            "\t- [ ] Handles CRLF\n",
            "\t- NOTES\n",
        )),
        "{note}"
    );
    assert!(note.contains("\t\t- see the ticket\n"), "{note}");
    assert!(note.contains("\t\t- from clip\n"), "{note}");
}

#[test]
fn an_authored_notes_drawer_merges_scooped_notes() {
    let (_temp, vault) = vault();
    let json = capture_json(
        &vault,
        &[],
        "@work Fix export\n- [ ] Handles CRLF\n- NOTES\n  - already filed\n- see the ticket",
    );
    assert_req_entry(&json, "Fix export", 1, 1, false, false);
    let note = read_work(&vault);
    assert!(
        note.contains(concat!(
            "- [ ] #task Fix export [created::2026-06-15]\n",
            "\t- [ ] Handles CRLF\n",
            "\t- NOTES\n",
            "\t\t- already filed\n",
            "\t\t- see the ticket\n",
        )),
        "{note}"
    );
}

#[test]
fn interleaved_notes_reqs_and_drawers_canonicalize() {
    let (_temp, vault) = vault();
    let json = capture_json(
        &vault,
        &[],
        "@work Fix export\n- see the ticket\n- [ ] Handles CRLF\n- FUTURE WORK\n  - later",
    );
    assert_req_entry(&json, "Fix export", 1, 1, true, false);
    let note = read_work(&vault);
    assert!(
        note.contains(concat!(
            "- [ ] #task Fix export [created::2026-06-15]\n",
            "\t- [ ] Handles CRLF\n",
            "\t- NOTES\n",
            "\t\t- see the ticket\n",
            "\t- FUTURE WORK\n",
            "\t\t- later\n",
        )),
        "{note}"
    );
}

#[test]
fn a_zero_req_new_task_stays_byte_identical_and_omits_requirements() {
    let (_temp, vault) = vault();
    let json = capture_json(&vault, &[], "@work Fix nothing\n- just a note");
    assert!(json.get("requirements").is_none(), "{json}");
    let note = read_work(&vault);
    assert!(
        note.contains(
            "- [ ] #task Fix nothing [created::2026-06-15]\n\t- just a note\n"
        ),
        "{note}"
    );
}

#[test]
fn a_single_requirement_uses_singular_copy() {
    let (_temp, vault) = vault();
    let dry = capture_json(
        &vault,
        &["--dry-run"],
        "@work Fix export\n- [ ] Handles CRLF",
    );
    assert_req_entry(&dry, "Fix export", 1, 0, false, true);
    let human = capture(
        &vault,
        &["--dry-run"],
        "@work Fix export\n- [ ] Handles CRLF",
        false,
    );
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("◇ adds 1 requirement to “Fix export”"),
        "{text}"
    );
    assert!(!text.contains("under NOTES"), "{text}");
}
