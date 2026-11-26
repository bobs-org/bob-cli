//! End-to-end settle acceptance: every wired bob-cli writer leaves
//! `bob task reconcile --dry-run -f json` with nothing note-local to plan
//! and `bob task tidy` at zero.
//!
//! Phase acceptance for epic `bob-cli-q4` (`plan:202611/better_o_gesture_settle.md`
//! § acceptance). Each test builds a small fixture vault with grouped area and
//! project notes, a future-scheduled Blocked task, a requirement-owning task
//! with a filed NOTES drawer, and a daily ledger; settles the baseline with
//! one real `bob task reconcile`; drives one wired writer; then asserts:
//!
//! - reconcile dry-run plans no status-group move, badge refresh, or local
//!   `[?]` flip, and strips no `LINKED FROM (💬 mentions)` phantoms;
//! - `bob task tidy` reports zero moves;
//! - a live `bob task reconcile` is a byte-level no-op on every `*.md` note.
//!
//! The Obsidian gesture harness (tier-1/tier-2 settles, `o`/`O` scaffolds)
//! runs in the plugin repo; the JS twin's byte parity with this behavior is
//! pinned by the `task_settle` vectors (`tests/fixtures/task_settle/`).

use crate::support::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-11-25 09:30:00";
const TIMEZONE_CONFIG: &str = "timezone: America/New_York\n";

fn harness(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(&vault.join("bob.yml"), TIMEZONE_CONFIG);
    let daily = vault.join("2026/20261125.md");
    write_file(
        &daily,
        concat!(
            "# 2026-11-25\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (0900-1000) Morning work\n",
            "  - [[work#^active-work|task]]\n",
        ),
    );
    write_file(
        &vault.join("work.md"),
        concat!(
            "---\n",
            "type: [[area]]\n",
            "---\n",
            "\n",
            "## Tasks\n",
            "\n",
            "- [ ] #task Intake idea ^intake-idea\n",
            "\n",
            "### Next & In Progress\n",
            "\n",
            "- [*] #task Active work ^active-work\n",
            "\n",
            "### Blocked\n",
            "\n",
            "- [?] #task Waiting on vendor [scheduled:: 2026-12-10] ^waiting-vendor\n",
            "\n",
            "### Done & Canceled\n",
            "\n",
            "- [x] #task Finished thing [completion:: 2026-11-20] ^finished-thing\n",
        ),
    );
    write_file(
        &vault.join("alpha.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "\n",
            "## Tasks\n",
            "\n",
            "- [ ] #task Alpha intake ^alpha-intake\n",
            "  - [ ] First requirement\n",
            "  - **Notes**\n",
            "    - Filed note\n",
            "- [ ] #task Alpha second ^alpha-second\n",
            "- [ ] #task Alpha third ^alpha-third\n",
        ),
    );
    // Settle the baseline once so every writer step starts from a vault
    // where reconcile already has nothing note-local to plan.
    let settled =
        reconcile_command(&temp, &vault, &daily, &["--format", "json"])
            .output()
            .expect("settle acceptance baseline reconcile");
    assert_success(&settled);
    let dry = reconcile_command(
        &temp,
        &vault,
        &daily,
        &["--dry-run", "--format", "json", "-r", "0"],
    )
    .output()
    .expect("verify settled baseline");
    assert_success(&dry);
    assert_nothing_note_local("baseline", &json_of(&dry));
    (temp, vault, daily)
}

fn home_env(temp: &TempDir, command: &mut std::process::Command) {
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("create isolated home");
    command
        .env("HOME", home)
        .env("BOB_DIR", temp.path().join("vault"))
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("vault-sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"));
}

fn reconcile_command(
    temp: &TempDir,
    vault: &Path,
    daily: &Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command.args(["task", "reconcile"]).args(args);
    home_env(temp, &mut command);
    command.arg("--bob-dir").arg(vault);
    command.env("BOB_DAY_FILE", daily);
    command
}

fn tidy_command(temp: &TempDir, args: &[&str]) -> std::process::Command {
    let mut command = bob_command();
    command.args(["task", "tidy"]).args(args);
    home_env(temp, &mut command);
    command
}

fn capture_json(temp: &TempDir, daily: &Path, draft: &str) -> Value {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(temp.path().join("vault"))
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft);
    home_env(temp, &mut command);
    command.env("BOB_DAY_FILE", daily);
    let output = command.output().expect("run bob capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "capture stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn writer_command(
    temp: &TempDir,
    daily: &Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command.args(args);
    home_env(temp, &mut command);
    command.env("BOB_DAY_FILE", daily);
    command
}

fn snooze_command(temp: &TempDir, daily: &Path, args: &[&str]) -> Output {
    let config = temp.path().join("vault/bob.yml");
    let mut command = bob_command();
    command.arg("snooze").args(args);
    home_env(temp, &mut command);
    command
        .env("BOB_DAY_FILE", daily)
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_SNOOZE_NOTIFY_SINK", temp.path().join("notify.json"));
    command.output().expect("run bob snooze")
}

fn json_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("expected JSON ({error}):\n{}", format_output(output))
    })
}

fn reasons(item: &Value) -> Vec<String> {
    item.get("reason_kinds")
        .and_then(Value::as_array)
        .map(|reasons| {
            reasons
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Mirror `scripts/dev/reconcile-drift::classify_reconcile` for the
/// note-local categories: status-group moves, badge refreshes, local
/// `[?]` flips (recurring or plain), and mention-strip phantoms.
fn note_local_counts(payload: &Value) -> BTreeMap<&'static str, usize> {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::from([
        ("status_group_move", 0),
        ("badge_row", 0),
        ("local_schedule_flip", 0),
        ("recurring_spawn_flip", 0),
        ("linked_from_mention_strip", 0),
    ]);
    let sections = payload
        .get("grouped_task_sections")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for section in &sections {
        let moved = section
            .get("moved_block_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if moved > 0 {
            *counts.get_mut("status_group_move").expect("move count") +=
                moved as usize;
        } else {
            *counts.get_mut("badge_row").expect("badge count") += 1;
        }
    }
    let blocked = payload
        .get("marked_blocked")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in &blocked {
        if reasons(item)
            .iter()
            .any(|reason| reason == "local_schedule")
        {
            let blob = ["description", "detail", "line", "task"]
                .into_iter()
                .filter_map(|key| item.get(key).and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            let recurring = blob.contains("🔁")
                || item.get("description").and_then(Value::as_str).is_some_and(
                    |description| {
                        description.contains("[repeat::")
                            || description.contains("repeat:")
                    },
                );
            if recurring {
                *counts
                    .get_mut("recurring_spawn_flip")
                    .expect("recur count") += 1;
            } else {
                *counts
                    .get_mut("local_schedule_flip")
                    .expect("schedule count") += 1;
            }
        }
    }
    let linked = payload
        .get("linked_from_line_updates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in &linked {
        let removed = item
            .get("mentions_removed")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if removed > 0 {
            *counts
                .get_mut("linked_from_mention_strip")
                .expect("mention count") += 1;
        }
    }
    counts
}

fn assert_nothing_note_local(step: &str, payload: &Value) {
    let counts = note_local_counts(payload);
    let total: usize = counts.values().sum();
    assert_eq!(
        total,
        0,
        "{step}: reconcile still plans note-local work {counts:?}:\n{}",
        serde_json::to_string_pretty(payload).expect("pretty reconcile JSON"),
    );
}

fn md_snapshot(vault: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(
        root: &Path,
        current: &Path,
        files: &mut BTreeMap<String, Vec<u8>>,
    ) {
        for entry in fs::read_dir(current).expect("read vault tree") {
            let entry = entry.expect("vault entry");
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                files.insert(
                    path.strip_prefix(root)
                        .expect("relative note path")
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(&path).expect("read note bytes"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(vault, vault, &mut files);
    files
}

/// After each writer step: dry-run reconcile plans nothing note-local,
/// tidy reports zero moves, and a live reconcile changes no note bytes.
fn assert_settled(temp: &TempDir, vault: &Path, daily: &Path, step: &str) {
    let dry = reconcile_command(
        temp,
        vault,
        daily,
        &["--dry-run", "--format", "json", "-r", "0"],
    )
    .output()
    .expect("dry-run reconcile");
    assert_success(&dry);
    assert_nothing_note_local(step, &json_of(&dry));

    let tidy = tidy_command(temp, &["-f", "json"])
        .output()
        .expect("dry-run tidy");
    assert_success(&tidy);
    let tidy_json = json_of(&tidy);
    assert_eq!(
        tidy_json["counts"]["moves"].as_u64().unwrap_or(u64::MAX),
        0,
        "{step}: tidy still plans moves:\n{}",
        serde_json::to_string_pretty(&tidy_json).expect("pretty tidy JSON"),
    );
    assert_eq!(
        tidy_json["counts"]["tasks_changed"]
            .as_u64()
            .unwrap_or(u64::MAX),
        0,
        "{step}: tidy still plans task changes:\n{}",
        serde_json::to_string_pretty(&tidy_json).expect("pretty tidy JSON"),
    );

    let before = md_snapshot(vault);
    let live = reconcile_command(temp, vault, daily, &["-r", "0"])
        .output()
        .expect("live reconcile");
    assert_success(&live);
    assert_eq!(
        md_snapshot(vault),
        before,
        "{step}: live reconcile rewrote note bytes the writer should have settled",
    );
}

#[test]
fn capture_into_grouped_note_leaves_nothing_note_local() {
    let (temp, vault, daily) = harness("bob-cli-settle-acceptance-capture");
    let json =
        capture_json(&temp, &daily, "Polish the release notes @work s:15");
    assert_eq!(json["ok"], true, "{json}");
    let work = fs::read_to_string(vault.join("work.md")).expect("read work");
    assert!(
        work.contains("- [?] #task Polish the release notes"),
        "future-scheduled capture is born Blocked:\n{work}",
    );
    assert_settled(&temp, &vault, &daily, "capture");
}

#[test]
fn task_move_between_grouped_notes_leaves_nothing_note_local() {
    let (temp, vault, daily) = harness("bob-cli-settle-acceptance-move");
    let output = writer_command(
        &temp,
        &daily,
        &[
            "task",
            "move",
            "-f",
            "json",
            "alpha",
            "work",
            "-b",
            "alpha-second",
        ],
    )
    .output()
    .expect("run bob task move");
    assert_success(&output);
    let work = fs::read_to_string(vault.join("work.md")).expect("read work");
    assert!(
        work.contains("Alpha second"),
        "moved task lands in the destination note:\n{work}",
    );
    assert_settled(&temp, &vault, &daily, "task move");
}

#[test]
fn task_split_and_merge_leave_nothing_note_local() {
    let (temp, vault, daily) = harness("bob-cli-settle-acceptance-split");
    let split = writer_command(
        &temp,
        &daily,
        &[
            "task",
            "split",
            "-f",
            "json",
            "alpha:alpha-intake",
            "-m",
            "Filed note",
        ],
    )
    .output()
    .expect("run bob task split");
    assert_success(&split);
    assert_settled(&temp, &vault, &daily, "task split");

    // Merge between requirement-less tasks: the canceled tombstone and
    // the copied content settle into their groups in the same write.
    // Merging into a requirement-owning task without `-D` still leaves a
    // loose note for tidy to file; see the PROPOSED FOLLOW-UP on the bead.
    let merge = writer_command(
        &temp,
        &daily,
        &[
            "task",
            "merge",
            "-f",
            "json",
            "-i",
            "alpha:alpha-third",
            "alpha:alpha-second",
        ],
    )
    .output()
    .expect("run bob task merge");
    assert_success(&merge);
    let alpha = fs::read_to_string(vault.join("alpha.md")).expect("read alpha");
    assert!(
        alpha.contains("Alpha second"),
        "merged content lands in the destination task:\n{alpha}",
    );
    assert_settled(&temp, &vault, &daily, "task merge");
}

#[test]
fn ref_today_leaves_nothing_note_local() {
    let (temp, vault, daily) = harness("bob-cli-settle-acceptance-ref-today");
    // Reference plus its reading task, following the `bob ref today`
    // parity fixture: the reading task lives in the area note.
    write_file(
        &vault.join("ref/papers/attention.md"),
        concat!(
            "---\n",
            "status: ready\n",
            "title: Attention Is All You Need\n",
            "id: attention\n",
            "---\n",
            "# Attention Is All You Need\n",
        ),
    );
    let work_before =
        fs::read_to_string(vault.join("work.md")).expect("read work");
    let with_reading = format!(
        "{work_before}- [ ] #task #ref [[ref/papers/attention]] ^ref-attention\n"
    );
    write_file(&vault.join("work.md"), &with_reading);
    // Re-settle the baseline with the new reading task so the writer step
    // starts from nothing note-local.
    let resettled =
        reconcile_command(&temp, &vault, &daily, &["--format", "json"])
            .output()
            .expect("settle baseline with reading task");
    assert_success(&resettled);
    let output = writer_command(&temp, &daily, &["ref", "today", "attention"])
        .output()
        .expect("run bob ref today");
    assert_success(&output);
    let work = fs::read_to_string(vault.join("work.md")).expect("read work");
    assert!(
        work.contains("^ref-attention"),
        "reading task stays in its note:\n{work}",
    );
    assert_settled(&temp, &vault, &daily, "ref today");
}

#[test]
fn snooze_add_and_drop_leave_nothing_note_local() {
    let (temp, vault, daily) = harness("bob-cli-settle-acceptance-snooze");
    let added = snooze_command(
        &temp,
        &daily,
        &["add", "work:intake-idea", "15:30", "-f", "json"],
    );
    assert_success(&added);
    assert_eq!(json_of(&added)["status"], "added");
    assert_settled(&temp, &vault, &daily, "snooze add");

    let dropped = snooze_command(
        &temp,
        &daily,
        &["drop", "work:intake-idea", "-f", "json"],
    );
    assert_success(&dropped);
    assert_eq!(json_of(&dropped)["status"], "dropped");
    assert_settled(&temp, &vault, &daily, "snooze drop");
}
