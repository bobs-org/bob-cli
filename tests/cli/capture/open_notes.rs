//! Batch-level `open_notes` on `bob capture --format json`.

use super::pomodoro_close::close_worked_vault;
use crate::support::*;
use std::fs;
use std::path::Path;

const NOW: &str = "2026-10-17 10:11:12";

fn open_vault(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day = vault.join("day.md");
    write_file(
        &day,
        "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) — WORK\n",
    );
    (temp, vault, day)
}

fn capture_json(vault: &Path, day: &Path, args: &[&str]) -> serde_json::Value {
    capture_json_dry_run_matches_real(vault, day, NOW, args)
}

fn capture_json_once(
    vault: &Path,
    day: &Path,
    args: &[&str],
    dry_run: bool,
) -> serde_json::Value {
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
    command.arg("--").args(args);
    let output = command
        .env("BOB_DAY_FILE", day)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

fn open_notes(json: &serde_json::Value) -> &[serde_json::Value] {
    json["open_notes"].as_array().expect("open_notes array")
}

fn assert_paths_absolute_and_refs_match(json: &serde_json::Value) {
    for note in open_notes(json) {
        let path = note["path"].as_str().expect("path");
        assert!(
            Path::new(path).is_absolute(),
            "path must be absolute: {note}"
        );
        let Some(line) = note["line"].as_u64() else {
            assert!(
                note.get("ref").is_none(),
                "ref is present only with line: {note}"
            );
            continue;
        };
        let contents = fs::read_to_string(path).expect("read opened note");
        let physical = contents
            .lines()
            .nth(line as usize - 1)
            .unwrap_or_else(|| panic!("line {line} missing in {path}"));
        assert_eq!(
            note["ref"].as_str().expect("ref"),
            capture_pomodoro_ref(physical, line as usize),
            "digest mismatch: {note}"
        );
        let expected_block = physical.rsplit_once('^').and_then(|(_, id)| {
            id.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                .then_some(id)
        });
        match expected_block {
            Some(id)
                if physical.contains(&format!(" ^{id}"))
                    || physical.ends_with(&format!("^{id}")) =>
            {
                assert_eq!(note["block_id"].as_str(), Some(id), "{note}");
            }
            _ => {
                assert!(
                    note.get("block_id").is_none(),
                    "no block_id on {note}"
                );
            }
        }
    }
}

fn first_live_pomodoro_line(json: &serde_json::Value) -> u64 {
    json["pomodoro_blocks"]
        .as_array()
        .expect("pomodoro_blocks")
        .iter()
        .find(|block| {
            block["removed"] != true
                && block["roles"].as_array().is_none_or(|roles| {
                    roles.iter().all(|role| role != "removed")
                })
        })
        .and_then(|block| block["line"].as_u64())
        .expect("live pomodoro line")
}

fn assert_note(
    note: &serde_json::Value,
    vault: &Path,
    relative: &str,
    role: &str,
    label: &str,
) {
    assert_eq!(note["path"], vault.join(relative).display().to_string());
    assert_eq!(note["relative_path"], relative);
    assert_eq!(note["role"], role);
    assert_eq!(note["label"], label);
}

#[test]
fn open_notes_new_inbox_task() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-inbox");
    let json = capture_json(&vault, &day, &["buy milk"]);
    assert_eq!(json["kind"], "task");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_note(&notes[0], &vault, "mac_inbox.md", "note", "mac_inbox");
    assert_eq!(notes[0]["line"], 1);
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_routed_task() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-routed");
    let json = capture_json(&vault, &day, &["buy milk @groceries"]);
    assert_eq!(json["kind"], "task");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_note(&notes[0], &vault, "groceries.md", "note", "groceries.md");
    assert_eq!(notes[0]["line"], 1);
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_section_bullet() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-bullet");
    write_file(
        &vault.join("foo.md"),
        "# Foo\n## Ideas\nnotes\n## Idea\nnotes\n",
    );
    let json = capture_json(&vault, &day, &["Some bullet @foo#Idea"]);
    assert_eq!(json["kind"], "bullet");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_note(&notes[0], &vault, "foo.md", "note", "foo.md");
    let contents = fs::read_to_string(vault.join("foo.md")).expect("foo");
    let line = contents
        .lines()
        .position(|line| line.contains("Some bullet"))
        .expect("bullet line")
        + 1;
    assert_eq!(notes[0]["line"], line as u64);
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_sub_bullet() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-sub");
    write_file(
        &vault.join("cash.md"),
        "## Tasks\n- [*] #task Parent [created::2026-07-01] ^parent\n\t- first child\n",
    );
    let json = capture_json(&vault, &day, &["new note @cash+parent"]);
    assert_eq!(json["kind"], "sub_bullet");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_note(&notes[0], &vault, "cash.md", "note", "cash.md");
    let contents = fs::read_to_string(vault.join("cash.md")).expect("cash");
    let line = contents
        .lines()
        .position(|line| line.contains("new note"))
        .expect("child line")
        + 1;
    assert_eq!(notes[0]["line"], line as u64);
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_ensure_next_toggle_opens_task_and_day() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-ensure-next");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Finish packet [dependsOn::root] ^goog-exit\n",
    );
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — LATER\n",
            "  - [[cash#^goog-exit]]\n",
            "    - review notes\n",
        ),
    );
    let json = capture_json(&vault, &day, &["@cash+goog-exit"]);
    assert_eq!(json["kind"], "task_toggle");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 2, "{json}");
    assert_note(&notes[0], &vault, "cash.md", "note", "cash.md");
    assert_eq!(notes[0]["block_id"], "goog-exit");
    assert_note(&notes[1], &vault, "day.md", "day", "Today");
    assert_eq!(notes[1]["line"], first_live_pomodoro_line(&json));
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_pomodoro_start() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-start");
    write_file(&day, "## Pomodoros\n- [ ] () — WORK\n");
    let json = capture_json(&vault, &day, &["="]);
    assert_eq!(json["kind"], "pomodoro_start");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_note(&notes[0], &vault, "day.md", "day", "Today");
    assert_eq!(notes[0]["line"], first_live_pomodoro_line(&json));
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_pomodoro_close_opens_only_the_day_file() {
    let (_temp, vault, day) = close_worked_vault("bob-cli-open-notes-close");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day,
        "2026-09-28 09:37:00",
        &["=x"],
    );
    assert_eq!(json["kind"], "pomodoro_close");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_eq!(notes[0]["role"], "day");
    assert_eq!(notes[0]["label"], "Today");
    assert_eq!(notes[0]["path"], day.display().to_string(), "{json}");
    for note in notes {
        assert_ne!(
            note["relative_path"].as_str().unwrap_or_default(),
            "bob.md",
            "closed-task notes stay closed: {json}"
        );
    }
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_pomodoro_reset() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-reset");
    write_toggle_task_settings(&vault);
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
            "- [ ] () — SASE\n",
        ),
    );
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day,
        "2026-10-17 09:37:00",
        &["=x0"],
    );
    assert_eq!(json["kind"], "pomodoro_reset", "{json}");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_eq!(notes[0]["role"], "day");
    assert_eq!(notes[0]["line"], first_live_pomodoro_line(&json));
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_pomodoro_adjust() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-adjust");
    let json = capture_json(&vault, &day, &["+5"]);
    assert_eq!(json["kind"], "pomodoro_adjust");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_note(&notes[0], &vault, "day.md", "day", "Today");
    assert_eq!(notes[0]["line"], first_live_pomodoro_line(&json));
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_task_complete_with_ledger() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-complete");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("sase.md"),
        "- [*] #task Fix flaky gkeep test ^fix-flaky\n",
    );
    write_file(
        &day,
        "## Pomodoros\n- [ ] (**0920-0950**) — CAPTURE\n  - [[sase#^fix-flaky]]\n",
    );
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day,
        "2026-10-05 09:30:00",
        &["!sase:fix-flaky"],
    );
    assert_eq!(json["kind"], "task_complete");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 2, "{json}");
    assert_note(&notes[0], &vault, "sase.md", "note", "sase.md");
    assert_eq!(notes[0]["block_id"], "fix-flaky");
    assert_note(&notes[1], &vault, "day.md", "day", "Today");
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_project_note_with_task_links() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-project");
    write_file(&vault.join("cash.md"), "---\ntype: \"[[area]]\"\n---\n");
    write_file(&day, "## Pomodoros\n- [ ] (1330-1400) Work\n");
    let input = "Finish the Google exit packet! @cash^goog-exit+#admin\n\
         - Draft the resignation memo :draft-memo\n";
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", "2026-09-20 14:31:07")
        .env("TZ", "UTC0");
    let output = run_with_stdin(&mut command, input);
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["kind"], "project_note");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 2, "{json}");
    assert_note(
        &notes[0],
        &vault,
        "cash_goog_exit.md",
        "note",
        "cash_goog_exit.md",
    );
    assert_eq!(notes[0]["block_id"], "prj");
    assert_note(&notes[1], &vault, "day.md", "day", "Today");
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_queued_link_is_empty() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-queued");
    let dry =
        capture_json_once(&vault, &day, &["https://example.com/post"], true);
    let real =
        capture_json_once(&vault, &day, &["https://example.com/post"], false);
    assert_eq!(dry["kind"], "ref");
    assert_eq!(dry["placement"], "queued");
    assert_eq!(open_notes(&dry).len(), 0, "{dry}");
    assert_eq!(real["kind"], "ref");
    assert_eq!(real["placement"], "queued");
    assert_eq!(open_notes(&real).len(), 0, "{real}");
    assert_eq!(dry["open_notes"], real["open_notes"]);
}

#[test]
fn open_notes_library_hit_has_no_line() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-library");
    write_file(
        &vault.join("ref/papers/captured.md"),
        "---\ntitle: Captured Post\nstatus: ready\nsource_url: https://example.com/captured\nsource_pdf: lib/papers/captured.pdf\n---\n\n- [ ] ^ref\n",
    );
    let json = capture_json(&vault, &day, &["https://example.com/captured"]);
    assert_eq!(json["kind"], "ref");
    assert_eq!(json["placement"], "unchanged");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 1, "{json}");
    assert_note(
        &notes[0],
        &vault,
        "ref/papers/captured.md",
        "note",
        "ref/papers/captured",
    );
    assert!(notes[0].get("line").is_none(), "{json}");
    assert!(notes[0].get("ref").is_none(), "{json}");
    assert!(Path::new(notes[0]["path"].as_str().unwrap()).is_absolute());
}

#[test]
fn open_notes_ampersand_and_star_open_the_owner_only() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-deps");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("foo.md"), "- [ ] #task Buy shoes ^bar\n");
    write_file(&vault.join("body.md"), "- [ ] #task Exercise ^excercise\n");
    write_file(
        &vault.join("job.md"),
        "- [*] #task Do cleanup tasks ^cleanup\n",
    );

    let ampersand = capture_json(&vault, &day, &["&foo:bar @body+excercise"]);
    assert_eq!(ampersand["kind"], "task_dependency");
    let notes = open_notes(&ampersand);
    assert_eq!(notes.len(), 1, "{ampersand}");
    assert_note(&notes[0], &vault, "body.md", "note", "body.md");
    assert_eq!(notes[0]["block_id"], "excercise");
    assert_paths_absolute_and_refs_match(&ampersand);

    let (_temp2, vault2, day2) = open_vault("bob-cli-open-notes-star");
    write_blocked_tasks_settings(&vault2);
    write_file(&vault2.join("job.md"), "- [ ] #task Cleanup ^cleanup\n");
    let star = capture_json(&vault2, &day2, &["Ask Kelly *job:cleanup"]);
    assert_eq!(star["kind"], "task");
    let notes = open_notes(&star);
    assert_eq!(notes.len(), 1, "{star}");
    assert_note(&notes[0], &vault2, "mac_inbox.md", "note", "mac_inbox");
    assert_ne!(
        notes[0]["relative_path"].as_str().unwrap_or_default(),
        "job.md",
        "blocked-task notes stay closed: {star}"
    );
    assert_paths_absolute_and_refs_match(&star);
}

#[test]
fn open_notes_three_items_dedupe_and_keep_first_landing() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-batch");
    write_file(&vault.join("job.md"), "# Job\n## Tasks\n");
    let json = capture_json(
        &vault,
        &day,
        &["first @job\n\nsecond @job\n\nthird @job:tid"],
    );
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 3, "{json}");
    let notes = open_notes(&json);
    assert_eq!(notes.len(), 2, "{json}");
    assert_note(&notes[0], &vault, "job.md", "note", "job.md");
    let contents = fs::read_to_string(vault.join("job.md")).expect("job");
    let first_line = contents
        .lines()
        .position(|line| line.contains("first"))
        .expect("first task")
        + 1;
    assert_eq!(notes[0]["line"], first_line as u64, "{json}");
    assert_note(&notes[1], &vault, "day.md", "day", "Today");
    assert_paths_absolute_and_refs_match(&json);
}

#[test]
fn open_notes_template_draft() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-template");
    fs::create_dir_all(&vault).expect("create vault");
    let draft = concat!(
        "@@template:\n",
        "Apply to {0}! @job^apply-{1}\n",
        "- **URL**: {2}\n",
        "\n",
        "Foobar foo https://foobar.example/job\n",
        "\"Two Sigma\" two-sigma https://twosigma.example/job\n",
    );
    let dry = capture_json_once(&vault, &day, &[draft], true);
    let real = capture_json_once(&vault, &day, &[draft], false);
    assert_eq!(dry["open_notes"], real["open_notes"], "{dry}\n{real}");
    let notes = open_notes(&real);
    assert_eq!(notes.len(), 1, "{real}");
    assert_note(&notes[0], &vault, "job.md", "note", "job.md");
    let contents = fs::read_to_string(vault.join("job.md")).expect("job");
    let first_line = contents
        .lines()
        .position(|line| line.contains("Apply to Foobar"))
        .expect("first template task")
        + 1;
    assert_eq!(notes[0]["line"], first_line as u64);
    assert_eq!(notes[0]["block_id"], "apply-foo");
    assert_paths_absolute_and_refs_match(&real);
}

#[test]
fn open_notes_human_output_is_unchanged() {
    let (_temp, vault, day) = open_vault("bob-cli-open-notes-human");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy milk")
        .env("BOB_DAY_FILE", &day)
        .env("BOB_NOW", NOW)
        .output()
        .expect("human capture");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("captured  mac_inbox.md"), "{out}");
    assert!(
        !out.contains("open_notes"),
        "human output must omit open_notes: {out}"
    );
}
