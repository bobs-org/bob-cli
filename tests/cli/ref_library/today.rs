//! `bob ref today` links a v2 reading task under today's Pomodoro.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-10-16 09:15:00";
const GENERATED_AT: &str = "2026-10-16T09:15:00";

const DEFAULT_DAY: &str = concat!(
    "## Pomodoros\n",
    "- [ ] (**0900-0930**) — CURRENT\n",
    "  - context\n",
    "- [ ] () — READING\n",
);

const ATTENTION: &str = concat!(
    "---\n",
    "status: ready\n",
    "title: Attention Is All You Need\n",
    "id: attention\n",
    "---\n",
    "# Attention Is All You Need\n",
);

fn area(tasks: &str) -> String {
    format!(
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "title: Sase\n",
            "---\n",
            "# Sase\n",
            "\n",
            "## Tasks\n",
            "\n",
            "{tasks}"
        ),
        tasks = tasks
    )
}

fn ready_task() -> &'static str {
    "- [ ] #task #ref [[ref/papers/attention]] ^ref-attention\n"
}

struct TodayVault {
    _temp: TempDir,
    vault: PathBuf,
    day_file: PathBuf,
}

impl TodayVault {
    fn new(prefix: &str, task: &str, day: &str) -> Self {
        let temp = TempDir::new(prefix);
        let vault = temp.path().join("vault");
        write_toggle_task_settings(&vault);
        write_file(
            &temp.path().join("config.yml"),
            "plan:\n  max_themes: 30\n",
        );
        write_file(&vault.join("ref/papers/attention.md"), ATTENTION);
        write_file(&vault.join("sase.md"), &area(task));
        let day_file = vault.join("day.md");
        write_file(&day_file, day);
        Self {
            _temp: temp,
            vault,
            day_file,
        }
    }

    fn with_notes(
        prefix: &str,
        notes: &[(&str, &str)],
        day: Option<&str>,
    ) -> Self {
        let temp = TempDir::new(prefix);
        let vault = temp.path().join("vault");
        write_toggle_task_settings(&vault);
        write_file(
            &temp.path().join("config.yml"),
            "plan:\n  max_themes: 30\n",
        );
        for (relative, contents) in notes {
            write_file(&vault.join(relative), contents);
        }
        let day_file = vault.join("day.md");
        if let Some(day) = day {
            write_file(&day_file, day);
        }
        Self {
            _temp: temp,
            vault,
            day_file,
        }
    }
}

fn run_today(vault: &TodayVault, args: &[&str]) -> Output {
    bob_command()
        .arg("ref")
        .arg("today")
        .args(args)
        .env("BOB_DIR", &vault.vault)
        .env("BOB_DAY_FILE", &vault.day_file)
        .env("BOB_NOW", NOW)
        .env(
            "BOB_CONFIG_FILE",
            vault.vault.parent().unwrap().join("config.yml"),
        )
        .output()
        .expect("run bob ref today")
}

#[test]
fn today_listing_marks_the_task_theme_suggestion() {
    let vault = TodayVault::new(
        "bob-cli-ref-today-theme-suggestion",
        ready_task(),
        "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n- [ ] () — READ\n",
    );
    write_file(
        &vault._temp.path().join("config.yml"),
        "plan:\n  max_themes: 30\nthemes:\n  READ: auto\n",
    );
    let (output, json) = run_today_json(&vault, &["attention", "-d"]);
    assert_success(&output);
    assert_envelope(&json, true);
    assert_eq!(json["theme_suggestion"]["source"], "reading");
    assert_eq!(json["theme_suggestion"]["label"], "reading task");
    let suggested = json["pomodoros"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["suggested"] == true)
        .expect("suggested open Pomodoro");
    assert_eq!(suggested["name"], "READ");
    assert_eq!(suggested["suggestion_label"], "reading task");
}

fn run_today_json(vault: &TodayVault, args: &[&str]) -> (Output, Value) {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_today(vault, &full);
    let parsed = serde_json::from_str(stdout(&output).trim());
    let json = parsed.unwrap_or_else(|error| {
        panic!("parse today JSON: {error}\n{}", format_output(&output))
    });
    (output, json)
}

fn assert_envelope(json: &Value, ok: bool) {
    assert_eq!(json["ok"], ok, "{json}");
    assert_eq!(json["schema_version"], 1, "{json}");
    assert_eq!(json["command"], "ref today", "{json}");
    assert_eq!(json["generated_at"], GENERATED_AT, "{json}");
}

fn snapshot_files(root: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    walk_text(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn walk_text(root: &Path, path: &Path, files: &mut Vec<(String, String)>) {
    let entries = fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let child = entry.path();
        if child
            .file_name()
            .is_some_and(|name| name == ".obsidian" || name == ".git")
        {
            continue;
        }
        if child.is_dir() {
            walk_text(root, &child, files);
            continue;
        }
        if child.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let relative = child
            .strip_prefix(root)
            .expect("child under root")
            .to_string_lossy()
            .into_owned();
        let contents = fs::read_to_string(&child).expect("read file");
        files.push((relative, contents));
    }
}

fn strip_dry_run(value: &Value) -> Value {
    let mut cloned = value.clone();
    if let Some(object) = cloned.as_object_mut() {
        object.remove("dry_run");
    }
    cloned
}

#[test]
fn today_matches_unnamed_capture_and_named_capture_remembers_theme() {
    let left = TodayVault::new(
        "bob-cli-ref-today-parity-left",
        ready_task(),
        DEFAULT_DAY,
    );
    let right = TodayVault::new(
        "bob-cli-ref-today-parity-right",
        ready_task(),
        DEFAULT_DAY,
    );

    let today = run_today(&left, &["attention"]);
    assert_success(&today);
    let capture = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&right.vault)
        .arg("--")
        .arg("^sase:ref-attention")
        .env("BOB_DAY_FILE", &right.day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture parity");
    assert_success(&capture);

    assert_eq!(
        fs::read_to_string(left.vault.join("sase.md")).expect("left sase"),
        fs::read_to_string(right.vault.join("sase.md")).expect("right sase")
    );
    assert_eq!(
        fs::read_to_string(&left.day_file).expect("left day"),
        fs::read_to_string(&right.day_file).expect("right day")
    );

    let named_left = TodayVault::new(
        "bob-cli-ref-today-parity-named-left",
        ready_task(),
        DEFAULT_DAY,
    );
    let named_right = TodayVault::new(
        "bob-cli-ref-today-parity-named-right",
        ready_task(),
        DEFAULT_DAY,
    );
    assert_success(&run_today(&named_left, &["attention", "-p", "bugs"]));
    let named_capture = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&named_right.vault)
        .arg("--")
        .arg("^sase:ref-attention#bugs")
        .env("BOB_DAY_FILE", &named_right.day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run named capture parity");
    assert_success(&named_capture);
    let left_task =
        fs::read_to_string(named_left.vault.join("sase.md")).expect("left");
    let right_task =
        fs::read_to_string(named_right.vault.join("sase.md")).expect("right");
    assert!(!left_task.contains("[theme::"), "{left_task}");
    assert!(right_task.contains("[type:: BUGS]"), "{right_task}");
    assert_eq!(
        fs::read_to_string(&named_left.day_file).expect("left day"),
        fs::read_to_string(&named_right.day_file).expect("right day")
    );
}

#[test]
fn dry_run_capture_object_matches_write_apart_from_dry_run() {
    let vault = TodayVault::new(
        "bob-cli-ref-today-dry-eq-write",
        ready_task(),
        DEFAULT_DAY,
    );
    let before = snapshot_files(&vault.vault);
    let (dry_out, dry) = run_today_json(&vault, &["attention", "-d"]);
    assert_success(&dry_out);
    assert_eq!(snapshot_files(&vault.vault), before, "dry-run wrote files");
    assert_envelope(&dry, true);
    assert_eq!(dry["mode"], "dry_run");
    assert_eq!(dry["changed"], true);
    assert_eq!(dry["capture"]["kind"], "pomodoro_link");

    let (write_out, write) = run_today_json(&vault, &["attention"]);
    assert_success(&write_out);
    assert_eq!(write["mode"], "write");
    assert_eq!(
        strip_dry_run(&dry["capture"]),
        strip_dry_run(&write["capture"]),
        "dry={}\nwrite={}",
        dry["capture"],
        write["capture"]
    );
}

#[test]
fn default_with_q_is_already_current() {
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - context\n",
        "- [ ] () — READING\n",
        "  - [[sase#^ref-attention]]\n",
    );
    let vault = TodayVault::new(
        "bob-cli-ref-today-q",
        "- [*] #task #ref [[ref/papers/attention]] [type:: BUILD] ^ref-attention\n",
        day,
    );
    let before_day = fs::read_to_string(&vault.day_file).expect("day");
    let before_sase =
        fs::read_to_string(vault.vault.join("sase.md")).expect("sase");
    let (output, json) = run_today_json(&vault, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["pomodoro_link_action"], "already_current");
    assert_eq!(json["changed"], false);
    assert_eq!(
        fs::read_to_string(&vault.day_file).expect("day"),
        before_day
    );
    assert_eq!(
        fs::read_to_string(vault.vault.join("sase.md")).expect("sase"),
        before_sase
    );
}

#[test]
fn default_without_q_places_by_the_reading_task_type() {
    let vault =
        TodayVault::new("bob-cli-ref-today-running", ready_task(), DEFAULT_DAY);
    let (output, json) = run_today_json(&vault, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["pomodoro_link_action"], "linked");
    assert_eq!(json["capture"]["creates_pomodoro"], true, "{json}");
    assert_eq!(json["capture"]["pomodoro_name"], "READ");
    let day = fs::read_to_string(&vault.day_file).expect("day");
    assert!(day.contains("[[sase#^ref-attention]]"), "{day}");
    assert!(day.contains("— READ\n"), "{day}");
    let read = day.find("— READ\n").expect("typed entry");
    let linked = day.find("[[sase#^ref-attention]]").expect("link");
    assert!(read < linked, "{day}");
}

#[test]
fn typed_task_uses_matching_open_entry_for_implicit_today() {
    let task = "- [ ] #task #ref [[ref/papers/attention]] [type:: BUILD] ^ref-attention\n";
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "- [ ] () — WRITE\n",
        "- [ ] () — BUILD\n",
    );
    let vault = TodayVault::new("bob-cli-ref-today-typed-entry", task, day);
    let (output, json) = run_today_json(&vault, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["pomodoro_name"], "BUILD");
    assert_eq!(
        json["capture"]["pomodoro_link_destination"]["name"],
        "BUILD"
    );
    assert_eq!(
        json["capture"]["pomodoro_link_destination"]["type"]["theme"]["name"],
        "BUILD"
    );
    assert!(json["capture"].get("task_theme").is_none(), "{json}");
    let day_after = fs::read_to_string(&vault.day_file).expect("day");
    let build = day_after.find("— BUILD").expect("BUILD entry");
    let link = day_after
        .find("[[sase#^ref-attention]]")
        .expect("task link");
    assert!(build < link, "{day_after}");
}

#[test]
fn typed_task_creates_named_entry_when_no_open_type_matches() {
    let task = "- [ ] #task #ref [[ref/papers/attention]] [type:: BUILD] ^ref-attention\n";
    let vault = TodayVault::new(
        "bob-cli-ref-today-typed-create",
        task,
        "## Pomodoros\n- [ ] (**0900-0930**) — CURRENT\n- [ ] () — WRITE\n",
    );
    let (output, json) = run_today_json(&vault, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["creates_pomodoro"], true, "{json}");
    assert_eq!(json["capture"]["pomodoro_name"], "BUILD");
    assert_eq!(
        json["capture"]["pomodoro_link_destination"]["name"],
        "BUILD"
    );
    assert_eq!(
        json["capture"]["pomodoro_link_destination"]["role"],
        "created"
    );
    assert_eq!(
        json["capture"]["pomodoro_link_destination"]["type"]["theme"]["name"],
        "BUILD"
    );
    assert!(json["capture"].get("task_theme").is_none(), "{json}");
    let day_after = fs::read_to_string(&vault.day_file).expect("day");
    assert!(day_after.contains("— BUILD\n"), "{day_after}");
    assert!(day_after.contains("[[sase#^ref-attention]]"), "{day_after}");
}

#[test]
fn named_prefix_selects_bugs() {
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "- [ ] () — BUGS\n",
    );
    let vault = TodayVault::new("bob-cli-ref-today-prefix", ready_task(), day);
    let (output, json) = run_today_json(&vault, &["attention", "-p", "bu"]);
    assert_success(&output);
    assert_eq!(json["capture"]["pomodoro_name"], "BUGS");
    assert_eq!(json["capture"]["creates_pomodoro"], false);
}

#[test]
fn exact_name_creates_when_only_prefix_matches() {
    let vault = TodayVault::new(
        "bob-cli-ref-today-exact-create",
        ready_task(),
        DEFAULT_DAY,
    );
    let (output, json) =
        run_today_json(&vault, &["attention", "-p", "read", "-e"]);
    assert_success(&output);
    assert_eq!(json["capture"]["creates_pomodoro"], true);
    assert_eq!(json["capture"]["pomodoro_name"], "READ");
    let day = fs::read_to_string(&vault.day_file).expect("day");
    assert!(day.contains("— READ\n"), "{day}");
    assert!(day.contains("— READING\n"), "{day}");
}

#[test]
fn exact_name_reuses_whole_slug() {
    let vault = TodayVault::new(
        "bob-cli-ref-today-exact-reuse",
        ready_task(),
        DEFAULT_DAY,
    );
    let (output, json) =
        run_today_json(&vault, &["attention", "-p", "reading", "-e"]);
    assert_success(&output);
    assert_eq!(json["capture"]["creates_pomodoro"], false);
    assert_eq!(json["capture"]["pomodoro_name"], "READING");
}

#[test]
fn pomodoro_ref_targets_an_unnamed_entry() {
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**)\n",
        "- [ ] () — READING\n",
    );
    let vault = TodayVault::new("bob-cli-ref-today-entry", ready_task(), day);
    let unnamed = fs::read_to_string(&vault.day_file)
        .expect("day")
        .lines()
        .nth(1)
        .expect("unnamed line")
        .to_string();
    let pomodoro_ref = capture_pomodoro_ref(&unnamed, 2);
    let (output, json) =
        run_today_json(&vault, &["attention", "-P", &pomodoro_ref]);
    assert_success(&output);
    assert_eq!(json["capture"]["pomodoro_link_action"], "linked");
    let day_after = fs::read_to_string(&vault.day_file).expect("day");
    let unnamed_pos = day_after.find("(**0900-0930**)").expect("unnamed");
    let link_pos = day_after.find("[[sase#^ref-attention]]").expect("link");
    let reading_pos = day_after.find("— READING").expect("reading");
    assert!(
        unnamed_pos < link_pos && link_pos < reading_pos,
        "{day_after}"
    );
}

#[test]
fn moving_to_duplicate_unnamed_entry_reports_its_post_move_line() {
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] () — SOURCE\n",
        "  - [[sase#^ref-attention]]\n",
        "- [ ] ()\n",
        "- [ ] ()\n",
    );
    let vault =
        TodayVault::new("bob-cli-ref-today-duplicate-entry", ready_task(), day);
    let selected = fs::read_to_string(&vault.day_file)
        .expect("day")
        .lines()
        .nth(4)
        .expect("second unnamed entry")
        .to_string();
    let selected_ref = capture_pomodoro_ref(&selected, 5);
    let (output, json) =
        run_today_json(&vault, &["attention", "-P", &selected_ref]);
    assert_success(&output);

    let day_after = fs::read_to_string(&vault.day_file).expect("day after");
    let destination_line = json["capture"]["pomodoro_link_destination"]["line"]
        .as_u64()
        .expect("destination line") as usize;
    let lines = day_after.lines().collect::<Vec<_>>();
    assert_eq!(destination_line, 4);
    assert_eq!(lines[destination_line - 1], "- [ ] ()");
    assert_eq!(lines[destination_line], "  - [[sase#^ref-attention]]");
}

#[test]
fn stale_pomodoro_ref_writes_nothing() {
    let vault =
        TodayVault::new("bob-cli-ref-today-stale", ready_task(), DEFAULT_DAY);
    let before = snapshot_files(&vault.vault);
    let (output, json) =
        run_today_json(&vault, &["attention", "-P", "99:deadbeef"]);
    assert_eq!(output.status.code(), Some(1));
    assert_envelope(&json, false);
    assert_eq!(json["error"]["code"], "stale_pomodoro");
    assert_eq!(snapshot_files(&vault.vault), before);
}

#[test]
fn completed_pomodoro_ref_is_closed() {
    let day = concat!(
        "## Pomodoros\n",
        "- [x] (**0800-0830**) — DONE\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
    );
    let vault =
        TodayVault::new("bob-cli-ref-today-closed-p", ready_task(), day);
    let done = fs::read_to_string(&vault.day_file)
        .expect("day")
        .lines()
        .nth(1)
        .expect("done line")
        .to_string();
    let pomodoro_ref = capture_pomodoro_ref(&done, 2);
    let before = snapshot_files(&vault.vault);
    let (output, json) =
        run_today_json(&vault, &["attention", "-P", &pomodoro_ref]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "pomodoro_closed");
    assert_eq!(snapshot_files(&vault.vault), before);
}

#[test]
fn moving_q_creates_the_named_destination() {
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**) — CURRENT\n",
        "  - [[sase#^ref-attention]]\n",
        "    - notes\n",
        "- [ ] () — READING\n",
    );
    let vault = TodayVault::new("bob-cli-ref-today-move", ready_task(), day);
    let (output, json) = run_today_json(&vault, &["attention", "-p", "focus"]);
    assert_success(&output);
    assert_eq!(json["capture"]["pomodoro_link_action"], "moved");
    assert_eq!(json["capture"]["creates_pomodoro"], true);
    let day_after = fs::read_to_string(&vault.day_file).expect("day");
    assert!(day_after.contains("— FOCUS\n"), "{day_after}");
    assert!(day_after.contains("[[sase#^ref-attention]]"), "{day_after}");
    let current_children = day_after
        .split_once("— CURRENT\n")
        .and_then(|(_, rest)| rest.split_once("— FOCUS\n"))
        .map(|(children, _)| children)
        .expect("current children");
    assert!(
        !current_children.contains("[[sase#^ref-attention]]"),
        "{day_after}"
    );
    let focus_children = day_after
        .split_once("— FOCUS\n")
        .and_then(|(_, rest)| rest.split_once("— READING"))
        .map(|(children, _)| children)
        .expect("focus children");
    assert!(
        focus_children.contains("[[sase#^ref-attention]]"),
        "{day_after}"
    );
}

#[test]
fn ready_becomes_next_with_fresh_stamp() {
    let vault =
        TodayVault::new("bob-cli-ref-today-ready", ready_task(), DEFAULT_DAY);
    let (output, json) = run_today_json(&vault, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["status_symbol"], "*");
    let sase = fs::read_to_string(vault.vault.join("sase.md")).expect("sase");
    assert!(sase.contains("- [*] #task #ref"), "{sase}");
    assert!(sase.contains("[fresh:: 2026-10-16]"), "{sase}");
}

#[test]
fn next_and_in_progress_keep_their_lanes() {
    let next = TodayVault::new(
        "bob-cli-ref-today-next",
        "- [*] #task #ref [[ref/papers/attention]] [type:: BUILD] ^ref-attention\n",
        DEFAULT_DAY,
    );
    let (output, json) = run_today_json(&next, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["status_symbol"], "*");
    assert_eq!(json["capture"]["previous_status_symbol"], "*");
    let sase = fs::read_to_string(next.vault.join("sase.md")).expect("sase");
    assert!(sase.contains("- [*] #task #ref"), "{sase}");

    let wip = TodayVault::new(
        "bob-cli-ref-today-wip",
        "- [/] #task #ref [[ref/papers/attention]] ^ref-attention\n",
        DEFAULT_DAY,
    );
    let (output, json) = run_today_json(&wip, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["status_symbol"], "/");
    let sase = fs::read_to_string(wip.vault.join("sase.md")).expect("sase");
    assert!(sase.contains("- [/] #task #ref"), "{sase}");
}

#[test]
fn scheduled_blocked_task_is_pulled_forward() {
    let task = concat!(
        "- [?] #task #ref [[ref/papers/attention]] [scheduled:: 2026-11-02] ^ref-attention\n",
        "\t- 🗓️ **SCHEDULE LOG**\n",
        "\t\t- *2026-08-01 → 2026-07-01* — earlier move\n",
    );
    let vault = TodayVault::new("bob-cli-ref-today-sched", task, DEFAULT_DAY);
    let (output, json) = run_today_json(&vault, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["kind"], "pomodoro_link");
    assert_eq!(json["capture"]["removed_scheduled"], "2026-11-02");
    let sase = fs::read_to_string(vault.vault.join("sase.md")).expect("sase");
    assert!(sase.contains("- [*] #task #ref"), "{sase}");
    assert!(!sase.contains("[scheduled:: 2026-11-02]"), "{sase}");
    assert!(sase.contains("2026-11-02 → 2026-10-16"), "{sase}");
}

#[test]
fn own_depends_on_is_dropped_and_linked() {
    let task = concat!(
        "- [ ] #task Prep [id:: sase__prep] ^prep\n",
        "- [?] #task #ref [[ref/papers/attention]] [dependsOn:: sase__prep] [id:: sase__ref-attention] ^ref-attention\n",
        "\t- ⛓️ **DEPENDS ON:** [[sase#^prep]]\n",
    );
    let vault = TodayVault::new("bob-cli-ref-today-deps", task, DEFAULT_DAY);
    let (output, json) = run_today_json(&vault, &["attention"]);
    assert_success(&output);
    assert_eq!(json["capture"]["kind"], "pomodoro_link");
    assert_eq!(
        json["capture"]["dropped_dependencies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let sase = fs::read_to_string(vault.vault.join("sase.md")).expect("sase");
    assert!(sase.contains("- [*] #task #ref"), "{sase}");
    assert!(!sase.contains("DEPENDS ON"), "{sase}");
    let day = fs::read_to_string(&vault.day_file).expect("day");
    assert!(day.contains("[[sase#^ref-attention]]"), "{day}");
}

#[test]
fn project_blocked_reading_task_is_link_guard() {
    let vault = TodayVault::with_notes(
        "bob-cli-ref-today-guard",
        &[
            ("ref/papers/attention.md", ATTENTION),
            (
                "job.md",
                "- [ ] #task Prep [id:: job__prep] ^prep\n",
            ),
            (
                "cash.md",
                concat!(
                    "---\n",
                    "type: \"[[project]]\"\n",
                    "status: wip\n",
                    "---\n\n",
                    "- [?] #task Ship #prj #hide [dependsOn:: job__prep] ^prj\n",
                    "\t- ⛓️ **DEPENDS ON:** [[job#^prep]]\n",
                    "- [ ] #task #ref [[ref/papers/attention]] ^ref-attention\n",
                ),
            ),
        ],
        Some(DEFAULT_DAY),
    );
    let before_day = fs::read_to_string(&vault.day_file).expect("day");
    let (output, json) = run_today_json(&vault, &["attention"]);
    assert_success(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["capture"]["kind"], "link_guard");
    let day = fs::read_to_string(&vault.day_file).expect("day");
    assert!(!day.contains("[[cash#^ref-attention]]"), "{day}");
    assert_eq!(day.matches("[[cash#^ref-attention]]").count(), 0);
    let job = fs::read_to_string(vault.vault.join("job.md")).expect("job");
    assert!(job.contains("- [*] #task Prep"), "{job}");
    assert!(!before_day.contains("[[cash#^ref-attention]]"));
}

#[test]
fn refusals_cover_task_and_ledger_codes() {
    let legacy = TodayVault::with_notes(
        "bob-cli-ref-today-legacy",
        &[(
            "ref/papers/legacy.md",
            "---\nstatus: ready\ntitle: Legacy Note\n---\n# Legacy Note\n",
        )],
        Some(DEFAULT_DAY),
    );
    let (output, json) = run_today_json(&legacy, &["legacy"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "no_reading_task");

    let v1 = TodayVault::with_notes(
        "bob-cli-ref-today-v1",
        &[(
            "ref/papers/v1.md",
            concat!(
                "---\nstatus: ready\ntitle: V1 Note\n---\n",
                "# V1 Note\n\n",
                "- [ ] #task #ref [[lib/papers/v1.pdf]] #hide ^ref\n",
            ),
        )],
        Some(DEFAULT_DAY),
    );
    let (output, json) = run_today_json(&v1, &["v1"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "v1_reading_task");

    let amb = TodayVault::with_notes(
        "bob-cli-ref-today-amb",
        &[
            (
                "ref/papers/split.md",
                "---\nstatus: ready\ntitle: Split\n---\n# Split\n",
            ),
            (
                "sase.md",
                &area("- [ ] #task #ref [[ref/papers/split]] ^ref-a\n"),
            ),
            (
                "bob.md",
                concat!(
                    "---\ntype: \"[[area]]\"\ntitle: Bob\n---\n",
                    "- [ ] #task #ref [[ref/papers/split]] ^ref-b\n",
                ),
            ),
        ],
        Some(DEFAULT_DAY),
    );
    let (output, json) = run_today_json(&amb, &["split"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "ambiguous_reading_task");

    let closed = TodayVault::with_notes(
        "bob-cli-ref-today-read",
        &[
            (
                "ref/papers/done.md",
                "---\nstatus: read\ntitle: Done Paper\n---\n# Done Paper\n",
            ),
            (
                "sase.md",
                &area(
                    "- [x] #task #ref [[ref/papers/done]] [completion:: 2026-10-01] ^ref-done\n",
                ),
            ),
        ],
        Some(DEFAULT_DAY),
    );
    let (output, json) = run_today_json(&closed, &["done"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "reading_task_closed");

    let abandoned = TodayVault::with_notes(
        "bob-cli-ref-today-abandoned",
        &[
            (
                "ref/papers/drop.md",
                "---\nstatus: abandoned\ntitle: Dropped\n---\n# Dropped\n",
            ),
            (
                "sase.md",
                &area("- [-] #task #ref [[ref/papers/drop]] ^ref-drop\n"),
            ),
        ],
        Some(DEFAULT_DAY),
    );
    let (output, json) = run_today_json(&abandoned, &["drop"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "reading_task_closed");

    let archived = TodayVault::with_notes(
        "bob-cli-ref-today-archived",
        &[
            (
                "ref/papers/archived.md",
                "---\nstatus: read\ntitle: Archived\n---\n# Archived\n",
            ),
            (
                "done/sase_done.md",
                concat!(
                    "---\ntype: \"[[area]]\"\ntitle: Sase Done\n---\n",
                    "- [x] #task #ref [[ref/papers/archived]] [completion:: 2026-10-12] ^ref-archived\n",
                ),
            ),
        ],
        Some(DEFAULT_DAY),
    );
    let (output, json) = run_today_json(&archived, &["archived"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "reading_task_closed");

    let nested = TodayVault::with_notes(
        "bob-cli-ref-today-unroutable",
        &[
            (
                "ref/papers/daily.md",
                "---\nstatus: ready\ntitle: Daily Target\n---\n# Daily Target\n",
            ),
            (
                "2026/20261016.md",
                "- [ ] #task #ref [[ref/papers/daily]] ^ref-daily\n",
            ),
        ],
        Some(DEFAULT_DAY),
    );
    let (output, json) = run_today_json(&nested, &["daily"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "reading_task_unroutable");
}

#[test]
fn ledger_and_resolution_refusals() {
    let missing_day = TodayVault::with_notes(
        "bob-cli-ref-today-no-day",
        &[
            ("ref/papers/attention.md", ATTENTION),
            ("sase.md", &area(ready_task())),
        ],
        None,
    );
    let (output, json) = run_today_json(&missing_day, &["attention"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "no_daily_note");

    let no_section = TodayVault::new(
        "bob-cli-ref-today-no-section",
        ready_task(),
        "# Just a note\n",
    );
    let (output, json) = run_today_json(&no_section, &["attention"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "no_pomodoros_section");

    let multi = TodayVault::new(
        "bob-cli-ref-today-multi",
        ready_task(),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — A\n",
            "- [ ] (**0930-1000**) — B\n",
        ),
    );
    let (output, json) = run_today_json(&multi, &["attention", "-d"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "multiple_running");
    assert!(json["pomodoros"].as_array().unwrap().len() >= 2, "{json}");

    let empty = TodayVault::new(
        "bob-cli-ref-today-empty",
        ready_task(),
        "## Pomodoros\n- [x] (**0800-0830**) — DONE\n",
    );
    let (output, json) = run_today_json(&empty, &["attention"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "no_open_pomodoro");
    let (created_out, created) =
        run_today_json(&empty, &["attention", "-p", "focus"]);
    assert_success(&created_out);
    assert_eq!(created["capture"]["creates_pomodoro"], true);
    assert_eq!(created["capture"]["pomodoro_name"], "FOCUS");

    let vault = TodayVault::new(
        "bob-cli-ref-today-bad-name",
        ready_task(),
        DEFAULT_DAY,
    );
    let (output, json) = run_today_json(&vault, &["attention", "-p", "@@@"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json["error"]["code"], "invalid_pomodoro_name");

    let (output, json) = run_today_json(&vault, &["Attention Is"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "unknown_reference");
    assert!(
        json["candidates"]
            .as_array()
            .is_some_and(|candidates| !candidates.is_empty()),
        "{json}"
    );

    let dup = TodayVault::with_notes(
        "bob-cli-ref-today-dup",
        &[
            (
                "ref/papers/dup.md",
                "---\nstatus: ready\ntitle: Dup Papers\n---\n# Dup Papers\n",
            ),
            (
                "ref/blogs/dup.md",
                "---\nstatus: ready\ntitle: Dup Blogs\n---\n# Dup Blogs\n",
            ),
        ],
        Some(DEFAULT_DAY),
    );
    let (output, json) = run_today_json(&dup, &["dup"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["error"]["code"], "ambiguous_reference");
    assert!(json["candidates"].as_array().unwrap().len() >= 2, "{json}");
}

#[test]
fn listing_flags_and_human_open_pomodoros() {
    let vault =
        TodayVault::new("bob-cli-ref-today-list", ready_task(), DEFAULT_DAY);
    let (output, json) = run_today_json(&vault, &["attention", "-d"]);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let rows = json["pomodoros"].as_array().expect("pomodoros");
    assert_eq!(rows.len(), 2, "{json}");
    assert_eq!(rows[0]["name"], "CURRENT");
    assert_eq!(rows[0]["running"], true);
    assert_eq!(rows[0]["default"], true);
    assert_eq!(rows[0]["linked"], false);
    assert!(rows[0]["ref"].as_str().unwrap().contains(':'), "{json}");
    assert_eq!(rows[0]["starts_at"], "2026-10-16T09:00");
    assert_eq!(rows[0]["ends_at"], "2026-10-16T09:30");
    assert_eq!(rows[1]["name"], "READING");
    assert_eq!(rows[1]["running"], false);
    assert_eq!(rows[1]["default"], false);
    assert_eq!(json["task"]["path"], "sase.md");
    assert_eq!(json["task"]["block_id"], "ref-attention");
    assert_eq!(json["ref"]["marker_id"], "attention");

    let human = run_today(&vault, &["attention", "-d"]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("would link"), "{text}");
    assert!(text.contains("📖 Attention Is All You Need"), "{text}");
    assert!(text.contains("Open Pomodoros"), "{text}");
    assert!(text.contains("●"), "{text}");
    assert!(text.contains("-P "), "{text}");
}

#[test]
fn unnamed_rows_carry_derived_homes_and_label() {
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0900-0930**)\n",
        "  - [[sase#^ref-attention]]\n",
        "- [ ] () — READING\n",
    );
    let vault = TodayVault::new("bob-cli-ref-today-homes", ready_task(), day);
    let (output, json) = run_today_json(&vault, &["attention", "-d"]);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let rows = json["pomodoros"].as_array().expect("pomodoros");
    assert_eq!(rows.len(), 2, "{json}");
    assert!(rows[0]["name"].is_null(), "{json}");
    assert_eq!(rows[0]["homes"]["chip"], "sase", "{json}");
    assert_eq!(rows[0]["homes"]["label"], "sase", "{json}");
    assert_eq!(rows[0]["homes"]["state"], "forecast", "{json}");
    assert_eq!(rows[1]["name"], "READING");
    assert!(rows[1].get("homes").is_none(), "{json}");

    let human = run_today(&vault, &["attention", "-d"]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("Open Pomodoros"), "{text}");
    assert!(!text.contains("(untitled)"), "{text}");
    assert!(text.contains("sase"), "{text}");
}

#[test]
fn help_lists_today_and_selectors() {
    let output = bob_command()
        .arg("ref")
        .arg("-h")
        .env("NO_COLOR", "1")
        .output()
        .expect("bob ref -h");
    assert_success(&output);
    let help = stdout(&output);
    assert!(help.contains("Library:"), "{help}");
    assert!(help.contains("\n  today"), "{help}");
    assert!(help.contains("bob ref today <REF> -p reading"), "{help}");

    let today = bob_command()
        .arg("ref")
        .arg("today")
        .arg("--help")
        .env("NO_COLOR", "1")
        .output()
        .expect("bob ref today --help");
    assert_success(&today);
    let help = stdout(&today);
    assert!(help.contains("-p NAME"), "{help}");
    assert!(help.contains("-p NAME -e"), "{help}");
    assert!(help.contains("-P LINE:DIGEST"), "{help}");
    assert!(help.contains("whole-slug"), "{help}");
}
