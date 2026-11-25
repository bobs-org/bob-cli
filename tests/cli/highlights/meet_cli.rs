//! Public `bob ref meet` CLI: list, preview, errors, and one filing.
//!
//! Synthetic fixtures only: invented people and companies, never real
//! meeting notes.

use crate::support::*;
use std::{fs, path::Path};

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn seed_parent(vault: &Path) {
    write_file(&vault.join("work.md"), "---\ntype: [[area]]\n---\n# Work\n");
    write_file(
        &vault.join("job.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[work]]\n---\n# Job\n## Tasks\n",
    );
    write_file(&vault.join(".gitignore"), "/xlib/\n");
}

fn seed_config(vault: &Path) {
    write_file(
        &vault.join("config.yml"),
        "meetings:\n  producer: VoiceQuill\n  relation_label: notes from\n  status: next\n  window_days: 7\n",
    );
}

fn seed_day(vault: &Path, rows: &str) {
    write_file(
        &vault.join("2026/20261124.md"),
        &format!("# Day\n## Events\n{rows}\n## Pomodoros\n"),
    );
}

fn cedar_row(root: &str) -> String {
    format!(
        "- [*] #task **1400-1430** Cedar Labs quarterly review [📅](https://calendar.google.com/event?eid=cedar-event \"Open in Google Calendar\") {root} ^cedar-review"
    )
}

fn meet_command(
    temp: &TempDir,
    vault: &Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command.arg("ref").arg("meet");
    for arg in args {
        command.arg(arg);
    }
    command
        .env("BOB_DIR", vault)
        .env("BOB_CONFIG_FILE", vault.join("config.yml"))
        .env("BOB_NOW", "2026-11-24 15:00:00")
        .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
        .env("BOB_REF_SCAN_LOCK_WAIT_SECONDS", "0")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("BOB_GCAL_ASSUME_TTY", "0")
        .env("XDG_STATE_HOME", temp.path().join("state"));
    command
}

fn notes_text() -> String {
    "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship the connector\n"
        .to_string()
}

#[test]
fn bare_list_shows_header_and_event_token() {
    let temp = TempDir::new("bob-cli-ref-meet-list");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let output = meet_command(&temp, &vault, &[]).output().expect("run meet");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Recent meetings"), "{out}");
    assert!(out.contains("need notes"), "{out}");
    assert!(out.contains("Cedar Labs quarterly review"), "{out}");
    assert!(out.contains("20261124#^cedar-review"), "{out}");
}

#[test]
fn explicit_list_filters_and_json_envelope() {
    let temp = TempDir::new("bob-cli-ref-meet-list-json");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(
        &vault,
        &format!(
            "{}\n- [*] #task **1500-1530** Driftwood planning [root:: job] ^driftwood",
            cedar_row("[root:: job]")
        ),
    );
    let output = meet_command(&temp, &vault, &["-l", "cedar", "-f", "json"])
        .output()
        .expect("run meet list json");
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("meet list json");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "ref meet");
    let meetings = json["meetings"].as_array().expect("meetings");
    assert_eq!(meetings.len(), 1, "{json}");
    assert_eq!(meetings[0]["title"], "Cedar Labs quarterly review");
    assert!(meetings[0]["ref"]
        .as_str()
        .unwrap()
        .contains("cedar-review"));
}

#[test]
fn dry_run_preview_human_and_json_stdout() {
    let temp = TempDir::new("bob-cli-ref-meet-preview");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let output = meet_command(
        &temp,
        &vault,
        &["-i", path_str(&notes), "20261124#^cedar-review", "-d"],
    )
    .output()
    .expect("run meet preview");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Would file meeting notes"), "{out}");
    assert!(out.contains("261124"), "{out}");
    assert!(out.contains("notes from"), "{out}");

    let output = meet_command(
        &temp,
        &vault,
        &[
            "-i",
            path_str(&notes),
            "20261124#^cedar-review",
            "-d",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("run meet preview json");
    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    let stdout_text = stdout(&output);
    assert_eq!(stdout_text.lines().count(), 1, "{stdout_text}");
    let json: serde_json::Value =
        serde_json::from_str(&stdout_text).expect("meet preview json");
    assert_eq!(json["ok"], true);
    assert_eq!(json["outcome"], "would_file");
    assert_eq!(json["match"]["method"], "explicit");
    assert_eq!(json["filing"]["parent"], "job");
    assert!(json["notes"]["sha256"]
        .as_str()
        .is_some_and(|sha| !sha.is_empty()));
}

#[test]
fn no_meeting_exits_2_with_code() {
    let temp = TempDir::new("bob-cli-ref-meet-no-meeting");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    // Daily note without Event Tasks.
    seed_day(&vault, "- [ ] #task ordinary chore");
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let output =
        meet_command(&temp, &vault, &["-i", path_str(&notes), "-f", "json"])
            .output()
            .expect("run meet no meeting");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error json");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "no_meeting");
}

#[test]
fn ambiguous_meeting_exits_2_with_candidates() {
    let temp = TempDir::new("bob-cli-ref-meet-ambiguous");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(
        &vault,
        &format!(
            "{}\n- [*] #task **1500-1530** Cedar Labs follow-up [root:: job] ^cedar-followup",
            cedar_row("[root:: job]")
        ),
    );
    // Vague notes match neither meeting strongly.
    let notes = temp.path().join("notes.md");
    write_file(&notes, "weekly sync notes\n\n- hello\n");
    let output =
        meet_command(&temp, &vault, &["-i", path_str(&notes), "-f", "json"])
            .output()
            .expect("run meet ambiguous");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error json");
    assert_eq!(json["error"]["code"], "ambiguous_meeting");
    assert!(json["candidates"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty()));
}

#[test]
fn needs_parent_exits_2_with_candidates() {
    let temp = TempDir::new("bob-cli-ref-meet-needs-parent");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row(""));
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let output = meet_command(
        &temp,
        &vault,
        &[
            "-i",
            path_str(&notes),
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("run meet needs parent");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error json");
    assert_eq!(json["error"]["code"], "needs_parent");
    assert!(json["parent_candidates"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty()));
}

#[test]
fn expected_sha_mismatch_refuses_before_render() {
    let temp = TempDir::new("bob-cli-ref-meet-sha");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let output = meet_command(
        &temp,
        &vault,
        &[
            "-i",
            path_str(&notes),
            "20261124#^cedar-review",
            "--expect-sha256",
            &"0".repeat(64),
            "-f",
            "json",
        ],
    )
    .output()
    .expect("run meet sha mismatch");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error json");
    assert_eq!(json["error"]["code"], "notes_changed");
}

#[test]
fn empty_notes_refuse_with_no_notes() {
    let temp = TempDir::new("bob-cli-ref-meet-empty");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let notes = temp.path().join("empty.md");
    write_file(&notes, "   \n");
    let output = meet_command(
        &temp,
        &vault,
        &[
            "-i",
            path_str(&notes),
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("run meet empty notes");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error json");
    assert_eq!(json["error"]["code"], "no_notes");
}

#[test]
fn clipboard_and_input_conflict() {
    let temp = TempDir::new("bob-cli-ref-meet-conflict");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let output = meet_command(&temp, &vault, &["-c", "-i", path_str(&notes)])
        .output()
        .expect("run meet conflict");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}

#[test]
fn list_conflicts_with_filing_options() {
    let temp = TempDir::new("bob-cli-ref-meet-list-conflict");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let output = meet_command(&temp, &vault, &["-l", "-c"])
        .output()
        .expect("run meet list conflict");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}

#[test]
fn help_lists_options_alphabetically_with_contract() {
    let output = bob_command()
        .args(["ref", "meet", "--help"])
        .output()
        .expect("run bob ref meet --help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("List meetings and file meeting notes"),
        "{help}"
    );
    assert!(help.contains("With -c, -i, or -e it files"), "{help}");
    assert!(help.contains("docs/meeting-notes.md"), "{help}");
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-c, --clipboard",
            "-D, --day",
            "-d, --dry-run",
            "-e, --edit",
            "-f, --format",
            "-i, --input",
            "--lib-dir <PATH>",
            "-l, --list",
            "-O, --overwrite",
            "-P, --parent",
            "-p, --producer",
            "-r, --ref-dir",
            "-s, --status",
            "-T, --title",
            "-t, --transcript",
            "-w, --window",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}
