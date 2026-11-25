//! Public `bob ref meet` CLI: list, preview, errors, and one filing.
//!
//! Synthetic fixtures only: invented people and companies, never real
//! meeting notes.

use crate::support::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

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

fn calendar_url(event_id: &str) -> String {
    let payload = URL_SAFE_NO_PAD.encode(format!("{event_id} calendar-id"));
    format!("https://calendar.google.com/event?eid={payload}")
}

fn cedar_row(root: &str) -> String {
    format!(
        "- [*] #task **1400-1430** Cedar Labs quarterly review [📅]({} \"Open in Google Calendar\") {root} ^cedar-review",
        calendar_url("cedar-event")
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
            "-L, --lib-dir",
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

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn assert_unique_json_keys(raw: &str) {
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut stack: Vec<(bool, HashSet<String>)> = Vec::new();
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() || byte == b':' || byte == b',' {
            index += 1;
            continue;
        }
        if byte == b'{' {
            stack.push((true, HashSet::new()));
            index += 1;
            continue;
        }
        if byte == b'[' {
            stack.push((false, HashSet::new()));
            index += 1;
            continue;
        }
        if byte == b'}' || byte == b']' {
            stack.pop();
            index += 1;
            continue;
        }
        if byte == b'"' {
            let start = index + 1;
            let mut cursor = start;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'\\' => cursor += 2,
                    b'"' => break,
                    _ => cursor += 1,
                }
            }
            let parsed =
                String::from_utf8_lossy(&bytes[start..cursor]).into_owned();
            let mut next = cursor + 1;
            while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                next += 1;
            }
            if next < bytes.len()
                && bytes[next] == b':'
                && stack.last().is_some_and(|(is_object, _)| *is_object)
            {
                let keys = &mut stack.last_mut().expect("object").1;
                assert!(
                    keys.insert(parsed.clone()),
                    "duplicate JSON key {parsed:?} in {raw}"
                );
            }
            index = cursor + 1;
            continue;
        }
        if raw[index..].starts_with("true") {
            index += 4;
            continue;
        }
        if raw[index..].starts_with("false") {
            index += 5;
            continue;
        }
        if raw[index..].starts_with("null") {
            index += 4;
            continue;
        }
        index += 1;
    }
}

fn write_renderer_stubs(dir: &Path) -> (PathBuf, PathBuf) {
    let pdf = dir.join("meet-fixture.pdf");
    write_highlights_pdf_pages(&pdf, &[&[]]);
    let pandoc = dir.join("fake-pandoc.sh");
    write_executable(
        &pandoc,
        r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
printf '\\documentclass{article}\\begin{document}Hi\\end{document}\n' > "$out"
"#,
    );
    let xelatex = dir.join("fake-xelatex.sh");
    write_executable(
        &xelatex,
        r#"#!/bin/sh
cp "$FAKE_XELATEX_PDF" note.pdf
printf 'fake log\n' > note.log
exit 0
"#,
    );
    (pandoc, xelatex)
}

fn seed_synced_vault(temp: &TempDir) -> (PathBuf, PathBuf) {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    seed_parent(&vault);
    seed_config(&vault);
    (vault, temp.path().join("meet-fixture.pdf"))
}

fn commit_vault(vault: &Path, message: &str) {
    git_in(vault, ["add", "."]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q", "origin", "HEAD:master"]);
}

fn meet_write_command(
    temp: &TempDir,
    vault: &Path,
    pandoc: &Path,
    xelatex: &Path,
    pdf: &Path,
    args: &[&str],
) -> Command {
    let mut command = meet_command(temp, vault, args);
    command
        .env("BOB_PANDOC_COMMAND", pandoc)
        .env("BOB_XELATEX_COMMAND", xelatex)
        .env("FAKE_XELATEX_PDF", pdf);
    command
}

#[test]
fn clipboard_hashes_raw_bytes_before_normalization() {
    let temp = TempDir::new("bob-cli-ref-meet-clip-hash");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let raw = b"\xEF\xBB\xBFCedar Labs quarterly review\r\n\r\n## Next Steps\r\n- (Ari) Ship\r\n";
    let payload = temp.path().join("clip.bin");
    fs::write(&payload, raw).expect("write clip bytes");
    let clipboard = temp.path().join("clipboard.sh");
    write_executable(
        &clipboard,
        &format!("#!/bin/sh\ncat '{}'\n", payload.display()),
    );
    let output = meet_command(
        &temp,
        &vault,
        &["-c", "-d", "20261124#^cedar-review", "-f", "json"],
    )
    .env("BOB_CLIPBOARD_CMD", &clipboard)
    .output()
    .expect("run meet clipboard hash");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("json");
    assert_eq!(json["notes"]["sha256"], sha256_bytes(raw));
}

#[test]
fn empty_clipboard_is_no_notes() {
    let temp = TempDir::new("bob-cli-ref-meet-clip-empty");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let clipboard = temp.path().join("clipboard.sh");
    write_executable(&clipboard, "#!/bin/sh\nprintf '   \\n'\n");
    let output = meet_command(
        &temp,
        &vault,
        &["-c", "20261124#^cedar-review", "-f", "json"],
    )
    .env("BOB_CLIPBOARD_CMD", &clipboard)
    .output()
    .expect("run meet empty clipboard");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("json");
    assert_eq!(json["error"]["code"], "no_notes");
}

#[test]
fn clipboard_command_failure_is_notes_unreadable() {
    let temp = TempDir::new("bob-cli-ref-meet-clip-fail");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let clipboard = temp.path().join("clipboard.sh");
    write_executable(&clipboard, "#!/bin/sh\necho fail >&2\nexit 1\n");
    let output = meet_command(
        &temp,
        &vault,
        &["-c", "20261124#^cedar-review", "-f", "json"],
    )
    .env("BOB_CLIPBOARD_CMD", &clipboard)
    .output()
    .expect("run meet clipboard fail");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("json");
    assert_eq!(json["error"]["code"], "notes_unreadable");
}

#[test]
fn expect_sha256_without_a_source_is_notes_changed() {
    let temp = TempDir::new("bob-cli-ref-meet-sha-no-source");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let output = meet_command(
        &temp,
        &vault,
        &[
            "-e",
            "--expect-sha256",
            &"ab".repeat(32),
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("run meet sha without source");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("json");
    assert_eq!(json["error"]["code"], "notes_changed");
}

#[test]
fn dry_run_never_launches_the_editor() {
    let temp = TempDir::new("bob-cli-ref-meet-dry-edit");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let sentinel = temp.path().join("editor-launched");
    let editor = temp.path().join("editor.sh");
    write_executable(
        &editor,
        &format!("#!/bin/sh\nprintf launched > '{}'\n", sentinel.display()),
    );
    let started = Instant::now();
    let output = meet_command(
        &temp,
        &vault,
        &[
            "-i",
            path_str(&notes),
            "-e",
            "-d",
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .env("VISUAL", &editor)
    .env("EDITOR", &editor)
    .output()
    .expect("run meet dry-run edit");
    let elapsed_ms = started.elapsed().as_millis();
    eprintln!("bob ref meet dry-run elapsed {elapsed_ms}ms (target <300ms)");
    assert_success(&output);
    assert!(!sentinel.exists(), "dry-run must not launch VISUAL");
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("json");
    assert_eq!(json["outcome"], "would_file");
}

#[test]
fn editor_json_keeps_one_stdout_envelope() {
    let temp = TempDir::new("bob-cli-ref-meet-editor-json");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let editor = temp.path().join("editor.sh");
    write_executable(
        &editor,
        r#"#!/bin/sh
printf 'NOISY EDITOR STDOUT\n'
printf 'Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship\n' > "$1"
"#,
    );
    let output = meet_command(
        &temp,
        &vault,
        &["-e", "-d", "20261124#^cedar-review", "-f", "json"],
    )
    .env("VISUAL", &editor)
    .env("EDITOR", &editor)
    .output()
    .expect("run meet editor json");
    assert_eq!(
        output.status.code(),
        Some(1),
        "dry-run editor-only is no_notes: {}",
        format_output(&output)
    );
    let stdout_text = stdout(&output);
    assert!(
        !stdout_text.contains("NOISY EDITOR STDOUT"),
        "{stdout_text}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&stdout_text).expect("json");
    assert_eq!(json["error"]["code"], "no_notes");
}

#[test]
fn editor_success_files_preview_without_noisy_stdout() {
    let temp = TempDir::new("bob-cli-ref-meet-editor-success");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    seed_config(&vault);
    seed_day(&vault, &cedar_row("[root:: job]"));
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let editor = temp.path().join("editor.sh");
    write_executable(
        &editor,
        r#"#!/bin/sh
printf 'NOISY\n'
body=$(sed '/------------------------ >8 ------------------------/,$d' "$1")
printf '%s\n\n(edited)\n' "$body" > "$1"
"#,
    );
    let output = meet_command(
        &temp,
        &vault,
        &[
            "-i",
            path_str(&notes),
            "-e",
            "-d",
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .env("VISUAL", &editor)
    .env("EDITOR", &editor)
    .output()
    .expect("run meet editor success");
    assert_success(&output);
    let stdout_text = stdout(&output);
    assert_eq!(stdout_text.lines().count(), 1, "{stdout_text}");
    assert!(!stdout_text.contains("NOISY"), "{stdout_text}");
    let json: serde_json::Value =
        serde_json::from_str(&stdout_text).expect("json");
    assert_eq!(json["outcome"], "would_file");
    assert_eq!(
        json["notes"]["sha256"],
        sha256_bytes(notes_text().as_bytes()),
        "editor keeps the original source digest"
    );
}

#[test]
fn stdin_notes_are_never_used_as_picker_input() {
    let temp = TempDir::new("bob-cli-ref-meet-stdin-picker");
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
    let mut child = meet_command(&temp, &vault, &["-i", "-", "-P", "job"])
        .env("BOB_GCAL_ASSUME_TTY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn meet stdin");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"weekly sync notes\n\n- hello\n")
        .expect("write stdin notes");
    let output = child.wait_with_output().expect("wait meet stdin");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let err = stderr(&output);
    assert!(err.contains("do not identify one meeting"), "{err}");
    assert!(
        err.contains("choose one of the evidenced meetings"),
        "{err}"
    );
    assert!(
        !err.contains("Which?"),
        "stdin notes must not open the meeting picker: {err}"
    );
}

#[test]
fn list_explicit_identity_and_url_filter_without_title_fallback() {
    let temp = TempDir::new("bob-cli-ref-meet-list-identity");
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
    let explicit = meet_command(
        &temp,
        &vault,
        &["-l", "20261124#^cedar-review", "-f", "json"],
    )
    .output()
    .expect("list explicit");
    assert_success(&explicit);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&explicit)).expect("json");
    let meetings = json["meetings"].as_array().expect("meetings");
    assert_eq!(meetings.len(), 1, "{json}");
    assert_eq!(meetings[0]["block_id"], "cedar-review");

    let url = meet_command(
        &temp,
        &vault,
        &["-l", &calendar_url("cedar-event"), "-f", "json"],
    )
    .output()
    .expect("list url");
    assert_success(&url);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&url)).expect("json");
    assert_eq!(json["meetings"].as_array().expect("meetings").len(), 1);

    let miss = meet_command(
        &temp,
        &vault,
        &["-l", "20261124#^missing-event", "-f", "json"],
    )
    .output()
    .expect("list miss");
    assert_success(&miss);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&miss)).expect("json");
    assert!(json["meetings"].as_array().expect("meetings").is_empty());
    assert_ne!(json["error"]["code"], "no_meeting");

    let root_word = meet_command(&temp, &vault, &["-l", "job", "-f", "json"])
        .output()
        .expect("list title-only");
    assert_success(&root_word);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&root_word)).expect("json");
    assert!(
        json["meetings"].as_array().expect("meetings").is_empty(),
        "title-query must not match root path {json}"
    );
}

#[test]
fn files_meeting_notes_with_unique_json_keys_and_actual_paths() {
    let temp = TempDir::new("bob-cli-ref-meet-file");
    let (vault, pdf) = seed_synced_vault(&temp);
    write_highlights_pdf_pages(&pdf, &[&[]]);
    seed_day(&vault, &cedar_row("[root:: job]"));
    commit_vault(&vault, "seed meetings");
    let (pandoc, xelatex) = write_renderer_stubs(temp.path());
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let output = meet_write_command(
        &temp,
        &vault,
        &pandoc,
        &xelatex,
        &pdf,
        &[
            "-i",
            path_str(&notes),
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("run meet file");
    assert_success(&output);
    let stdout_text = stdout(&output);
    assert_eq!(stdout_text.lines().count(), 1, "{stdout_text}");
    assert_unique_json_keys(&stdout_text);
    let json: serde_json::Value =
        serde_json::from_str(&stdout_text).expect("json");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["outcome"], "filed");
    assert_eq!(json["match"]["method"], "explicit");
    assert!(
        json["filing"]["note"]
            .as_str()
            .is_some_and(|note| note.starts_with("ref/meetings/")
                && note.ends_with(".md")),
        "{json}"
    );
    assert!(
        json["filing"]["pdf"].as_str().is_some_and(|pdf| pdf
            .starts_with("lib/meetings/")
            && pdf.ends_with(".pdf")),
        "{json}"
    );
    assert_eq!(json["filing"]["task"]["destination"], "job");
    assert!(json["commit"].as_str().is_some_and(|sha| !sha.is_empty()));
    assert!(json["pages"].as_u64().is_some_and(|pages| pages >= 1));
    let note = vault.join(json["filing"]["note"].as_str().expect("note"));
    assert!(note.is_file(), "filed note {}", note.display());
}

#[test]
fn already_filed_refuses_without_overwrite() {
    let temp = TempDir::new("bob-cli-ref-meet-already");
    let (vault, pdf) = seed_synced_vault(&temp);
    write_highlights_pdf_pages(&pdf, &[&[]]);
    seed_day(&vault, &cedar_row("[root:: job]"));
    commit_vault(&vault, "seed meetings");
    let (pandoc, xelatex) = write_renderer_stubs(temp.path());
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let first = meet_write_command(
        &temp,
        &vault,
        &pandoc,
        &xelatex,
        &pdf,
        &["-i", path_str(&notes), "20261124#^cedar-review"],
    )
    .output()
    .expect("first file");
    assert_success(&first);
    let second = meet_write_command(
        &temp,
        &vault,
        &pandoc,
        &xelatex,
        &pdf,
        &[
            "-i",
            path_str(&notes),
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("second file");
    assert_eq!(second.status.code(), Some(2), "{}", format_output(&second));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&second)).expect("json");
    assert_eq!(json["error"]["code"], "already_filed");
}

#[test]
fn overwrite_keeps_custom_title_without_explicit_title_flag() {
    let temp = TempDir::new("bob-cli-ref-meet-overwrite-title");
    let (vault, pdf) = seed_synced_vault(&temp);
    write_highlights_pdf_pages(&pdf, &[&[]]);
    seed_day(&vault, &cedar_row("[root:: job]"));
    commit_vault(&vault, "seed meetings");
    let (pandoc, xelatex) = write_renderer_stubs(temp.path());
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let first = meet_write_command(
        &temp,
        &vault,
        &pandoc,
        &xelatex,
        &pdf,
        &[
            "-i",
            path_str(&notes),
            "-T",
            "Cedar Meeting",
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("file with custom title");
    assert_success(&first);
    let first_json: serde_json::Value =
        serde_json::from_str(&stdout(&first)).expect("json");
    assert_eq!(first_json["filing"]["title"], "Cedar Meeting");
    let revised = temp.path().join("revised.md");
    write_file(
        &revised,
        "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Follow up\n",
    );
    let replaced = meet_write_command(
        &temp,
        &vault,
        &pandoc,
        &xelatex,
        &pdf,
        &[
            "--overwrite",
            "-i",
            path_str(&revised),
            "20261124#^cedar-review",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("overwrite without title");
    assert_success(&replaced);
    let stdout_text = stdout(&replaced);
    assert_unique_json_keys(&stdout_text);
    let json: serde_json::Value =
        serde_json::from_str(&stdout_text).expect("json");
    assert_eq!(json["outcome"], "filed");
    assert_eq!(json["replaced"], true);
    assert_eq!(json["filing"]["title"], "Cedar Meeting");
    assert_eq!(
        json["filing"]["note"], first_json["filing"]["note"],
        "overwrite keeps the note path"
    );
    assert_eq!(
        json["filing"]["task"]["block_id"],
        first_json["filing"]["task"]["block_id"]
    );
}

#[test]
fn inbox_reroot_files_with_one_commit() {
    let temp = TempDir::new("bob-cli-ref-meet-reroot");
    let (vault, pdf) = seed_synced_vault(&temp);
    write_highlights_pdf_pages(&pdf, &[&[]]);
    seed_day(&vault, &cedar_row(""));
    commit_vault(&vault, "seed meetings");
    let (pandoc, xelatex) = write_renderer_stubs(temp.path());
    let notes = temp.path().join("notes.md");
    write_file(&notes, &notes_text());
    let output = meet_write_command(
        &temp,
        &vault,
        &pandoc,
        &xelatex,
        &pdf,
        &[
            "-i",
            path_str(&notes),
            "20261124#^cedar-review",
            "-P",
            "job",
            "-f",
            "json",
        ],
    )
    .output()
    .expect("run meet reroot");
    assert_success(&output);
    let stdout_text = stdout(&output);
    assert_unique_json_keys(&stdout_text);
    let json: serde_json::Value =
        serde_json::from_str(&stdout_text).expect("json");
    assert_eq!(json["outcome"], "filed");
    assert_eq!(json["filing"]["parent"], "job");
    let day = fs::read_to_string(vault.join("2026/20261124.md")).expect("day");
    assert!(day.contains("[root:: job]"), "{day}");
}

#[test]
fn json_parent_choice_errors_without_prompting() {
    let temp = TempDir::new("bob-cli-ref-meet-json-parent");
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
    .env("BOB_GCAL_ASSUME_TTY", "1")
    .output()
    .expect("run meet json parent");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("json");
    assert_eq!(json["error"]["code"], "needs_parent");
}
