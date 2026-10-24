//! Capture inbox route detection and the write-free choice guard.
use crate::support::*;
use std::{fs, path::Path};

const NOW: &str = "2026-10-24 09:30:00";

fn setup(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(&vault.join("inbox.md"), "# Inbox\n");
    write_file(
        &vault.join("mac_inbox.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task Route me ^route-task\n",
            "- [*] #task Already Next ^already-next\n",
        ),
    );
    write_file(
        &vault.join("gkeep_inbox.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [ ] #task GKeep task ^gkeep-task\n",
        ),
    );
    write_file(
        &vault.join("health.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n",
    );
    let day_file = vault.join("20261024.md");
    write_file(&day_file, "## Pomodoros\n- [ ] (**0900-0930**) — WORK\n");
    (temp, vault, day_file)
}

fn capture(
    vault: &Path,
    day_file: &Path,
    dry_run: bool,
    guard: bool,
    draft: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    if dry_run {
        command.arg("-d");
    }
    if guard {
        command.arg("--require-inbox-route-choice");
    }
    command
        .arg("--")
        .arg(draft)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture")
}

fn remove_dry_run(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.remove("dry_run");
            for value in object.values_mut() {
                remove_dry_run(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                remove_dry_run(value);
            }
        }
        _ => {}
    }
}

#[test]
fn ir_preview_report_matches_write_and_hidden_guard_refuses_write_free() {
    let (_temp, vault, day_file) = setup("bob-cli-inbox-route-report");
    let original_inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("original inbox");
    let original_day = fs::read_to_string(&day_file).expect("original day");

    // IR1: a Ready inbox task linked by capture becomes a route candidate.
    // IR2: the batch also links an already-Next task and creates a new task;
    // neither adds a route candidate.
    let draft = "@mac_inbox:route-task\n\n@mac_inbox:already-next\n\nNew task @mac_inbox";
    let preview = capture(&vault, &day_file, true, false, draft);
    assert_success(&preview);
    let preview_json: serde_json::Value =
        serde_json::from_str(stdout(&preview).trim()).expect("preview JSON");
    assert_eq!(preview_json["inbox_route"]["supported"], true);
    assert_eq!(preview_json["inbox_route"]["candidate_count"], 1);
    let candidate = &preview_json["inbox_route"]["candidates"][0];
    assert_eq!(candidate["key"], "mac_inbox.md#^route-task");
    assert_eq!(candidate["n"], 1);
    assert_eq!(candidate["note_path"], "mac_inbox.md");
    assert_eq!(candidate["inbox"], "mac_inbox");
    assert_eq!(candidate["previous_status_symbol"], " ");
    assert_eq!(candidate["status_symbol"], "*");
    assert_eq!(candidate["answer"], serde_json::Value::Null);
    assert_eq!(candidate["outcome"], "pending");
    let destinations = preview_json["inbox_route"]["destinations"]
        .as_array()
        .expect("destinations");
    assert!(destinations
        .iter()
        .any(|row| row["relative_path"] == "health.md"));
    assert!(!destinations
        .iter()
        .any(|row| row["relative_path"] == "mac_inbox.md"));
    assert!(!destinations
        .iter()
        .any(|row| row["relative_path"] == "gkeep_inbox.md"));
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("inbox unchanged"),
        original_inbox
    );

    let real = capture(&vault, &day_file, false, false, draft);
    assert_success(&real);
    let real_json: serde_json::Value =
        serde_json::from_str(stdout(&real).trim()).expect("real JSON");
    let mut preview_without_dry_run = preview_json;
    let mut real_without_dry_run = real_json;
    remove_dry_run(&mut preview_without_dry_run);
    remove_dry_run(&mut real_without_dry_run);
    assert_eq!(preview_without_dry_run, real_without_dry_run);

    // Restore the sandbox so the hidden guard is tested against the same
    // planned transition and can prove that neither note is touched.
    write_file(&vault.join("mac_inbox.md"), &original_inbox);
    write_file(&day_file, &original_day);
    // IR6: the client guard refuses before either note is written.
    let guarded = capture(&vault, &day_file, false, true, draft);
    assert_eq!(guarded.status.code(), Some(1));
    let error_json: serde_json::Value =
        serde_json::from_str(stdout(&guarded).trim()).expect("guard JSON");
    assert_eq!(error_json["code"], "inbox_route_choice_required");
    assert_eq!(error_json["inbox_route"]["candidate_count"], 1);
    assert!(error_json["error"]
        .as_str()
        .expect("error message")
        .contains("pass --inbox-route"));
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("guard inbox"),
        original_inbox
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("guard day"),
        original_day
    );
}

#[test]
fn ir_gkeep_inbox_child_is_classified_from_its_live_parent() {
    let (_temp, vault, day_file) = setup("bob-cli-inbox-route-gkeep");
    let output =
        capture(&vault, &day_file, true, false, "@gkeep_inbox:gkeep-task");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    // IR3: a second direct area child of inbox.md is an inbox too.
    assert_eq!(json["inbox_route"]["candidate_count"], 1);
    assert_eq!(
        json["inbox_route"]["candidates"][0]["note_path"],
        "gkeep_inbox.md"
    );
}

#[test]
fn ir_pull_in_choice_refusal_precedes_inbox_route_choice_refusal() {
    let (_temp, vault, day_file) = setup("bob-cli-inbox-route-guard-order");
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n- [ ] #task Finish prerequisite [id:: prerequisite] ^prerequisite\n",
    );
    write_file(
        &vault.join("mac_inbox.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "parent: [[inbox]]\n",
            "---\n",
            "## Tasks\n",
            "- [?] #task Recovered inbox task [dependsOn:: prerequisite] [id:: dependent] ^dependent\n",
            "- [ ] #task Route me ^route-task\n",
        ),
    );
    let before_inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox before");
    let before_dev =
        fs::read_to_string(vault.join("dev.md")).expect("dev before");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--require-inbox-route-choice")
        .arg("--require-pull-in-choice")
        .arg("--")
        .arg("!dev:prerequisite")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture");
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("refusal JSON");
    // IR7: a recovery would also change an inbox task, but the pull-in card
    // remains the first required choice in the submit sequence.
    assert_eq!(json["code"], "pull_in_choice_required");
    assert!(json.get("inbox_route").is_none());
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("inbox after"),
        before_inbox
    );
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("dev after"),
        before_dev
    );
}

#[test]
fn inbox_route_choice_guard_stays_hidden_from_help() {
    let output = bob_command()
        .arg("capture")
        .arg("--help")
        .output()
        .expect("capture help");
    assert_success(&output);
    assert!(!stdout(&output).contains("require-inbox-route-choice"));
}
