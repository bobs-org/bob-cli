//! Numbered start lineup tests: every whole-item start reports its
//! queued Task Links with `tasks[].index`/`tasks[].now` and a numbered
//! human index column. The `~<K>` drop list itself lands in
//! start-drop-grammar; here the engine only ever sees an empty list.

use crate::support::*;
use std::fs;

fn start_lineup_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260930.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t- [[bob#^web-capture]]\n",
            "\t  - remember the URL parser\n",
            "\t- [[sase#^axe-restart]]\n",
            "- [ ] () — SASE\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [*] #task Stop capture from the panel [created::2026-09-20] ^capture-stop\n",
            "- [*] #task Capture support for web URLs #now [created::2026-09-21] ^web-capture\n",
        ),
    );
    write_file(
        &vault.join("sase.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [*] #task Restart axe [created::2026-09-20] ^axe-restart\n",
        ),
    );
    (temp, vault, day_file)
}

fn run_start_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    token: &str,
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(token)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", "2026-09-30 09:42:00")
        .output()
        .expect("run start capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("start JSON")
}

#[test]
fn start_lineup_numbered_human_and_json() {
    let (_temp, vault, day_file) = start_lineup_vault("bob-cli-start-lineup");
    // Human dry run first: the JSON capture below starts the session.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-30 09:42:00")
        .output()
        .expect("run lineup human");
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
    let out = stdout(&human);
    assert!(out.contains("would start"), "{out}");
    assert!(out.contains("CAPTURE 0945-1010 (25m) at line 5"), "{out}");
    // Numbered index column in ledger order, bold with no outcome color
    // (plain here, so just the digits).
    for line in [
        "  1 [*] Stop capture from the panel bob.md ^capture-stop",
        "  2 [*] Capture support for web URLs #now bob.md ^web-capture",
        "  3 [*] Restart axe sase.md ^axe-restart",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(!out.contains("nothing queued"), "{out}");

    let json = run_start_json(&vault, &day_file, "=");
    assert_eq!(json["kind"], "pomodoro_start");
    let start = &json["pomodoro_start"];
    assert_eq!(start["start"], "0945");
    assert_eq!(start["end"], "1010");
    assert_eq!(start["duration_minutes"], 25);
    // No drop list typed: both fields stay omitted.
    assert!(start.get("drop").is_none(), "{json}");
    assert!(start.get("dropped").is_none(), "{json}");
    let tasks = start["tasks"].as_array().expect("tasks array").clone();
    assert_eq!(tasks.len(), 3, "{json}");
    let indices: Vec<u64> = tasks
        .iter()
        .map(|row| row["index"].as_u64().expect("index"))
        .collect();
    assert_eq!(indices, vec![1, 2, 3], "{json}");
    // Only the `#now` task carries `now: true`; it is omitted otherwise.
    assert!(tasks[0].get("now").is_none(), "{json}");
    assert_eq!(tasks[1]["now"], true, "{json}");
    assert!(tasks[2].get("now").is_none(), "{json}");
    assert_eq!(tasks[0]["ledger_line"], 6, "{json}");
    assert_eq!(tasks[1]["ledger_line"], 7, "{json}");
    assert_eq!(tasks[2]["ledger_line"], 9, "{json}");
    // Starting never touches task notes.
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert!(bob_after.contains("#now [created::2026-09-21] ^web-capture"));
}

#[test]
fn start_lineup_counted_and_named() {
    // Each token commits its own start, so each gets a fresh vault.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-lineup-counted");
    let counted = run_start_json(&vault, &day_file, "=3");
    let start = &counted["pomodoro_start"];
    assert_eq!(start["start"], "0945");
    assert_eq!(start["end"], "1000");
    assert_eq!(start["duration_minutes"], 15);
    let indices: Vec<u64> = start["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .map(|row| row["index"].as_u64().expect("index"))
        .collect();
    assert_eq!(indices, vec![1, 2, 3], "{counted}");
    assert_eq!(start["tasks"][1]["now"], true, "{counted}");

    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-lineup-named");
    let named = run_start_json(&vault, &day_file, "=#capture");
    let start = &named["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "CAPTURE", "{named}");
    assert_eq!(start["created_pomodoro"], false, "{named}");
    let indices: Vec<u64> = start["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .map(|row| row["index"].as_u64().expect("index"))
        .collect();
    assert_eq!(indices, vec![1, 2, 3], "{named}");
}

#[test]
fn link_and_task_starts_stay_byte_stable() {
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-lineup-stable");
    write_file(
        &vault.join("bob.md"),
        &format!(
            "{}- [ ] #task Spare work [created::2026-09-20] ^spare\n",
            fs::read_to_string(vault.join("bob.md")).expect("read bob"),
        ),
    );
    // Link into the now-running session, as in pomodoro_whole_item.
    let _started = run_start_json(&vault, &day_file, "=");
    let link = run_start_json(&vault, &day_file, "^bob:spare");
    assert_eq!(link["kind"], "pomodoro_link", "{link}");
    assert!(link["pomodoro_start"].get("tasks").is_none(), "{link}");
    assert!(link["pomodoro_start"].get("drop").is_none(), "{link}");
    assert!(link["pomodoro_start"].get("dropped").is_none(), "{link}");

    let temp = TempDir::new("bob-cli-start-task-stable");
    let vault_t = temp.path().join("vault");
    let day_t = vault_t.join("day.md");
    write_toggle_task_settings(&vault_t);
    write_file(&day_t, "## Pomodoros\n- [ ] () — SASE\n");
    let task = run_start_json(&vault_t, &day_t, "More @bob:tasked=");
    assert_eq!(task["kind"], "pomodoro_task", "{task}");
    assert!(task["pomodoro_start"].get("tasks").is_none(), "{task}");
    assert!(task["pomodoro_start"].get("drop").is_none(), "{task}");
    assert!(task["pomodoro_start"].get("dropped").is_none(), "{task}");
}
