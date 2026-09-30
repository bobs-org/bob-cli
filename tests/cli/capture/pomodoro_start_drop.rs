//! Numbered start lineup and `~<K>` start-drop grammar tests: every
//! whole-item start reports its queued Task Links with
//! `tasks[].index`/`tasks[].now` and a numbered human index column, and a
//! trailing `~<K>` drop list starts the next session without those links.

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

fn run_start_expect_error(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
) -> String {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", "2026-09-30 09:42:00")
        .output()
        .expect("run start capture");
    assert!(
        !output.status.success(),
        "expected failure:\n{}",
        format_output(&output)
    );
    stdout(&output).trim().to_string()
}

#[test]
fn start_drop_bare_removes_link_with_nested_note() {
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-bare");
    // Human dry run first: the JSON capture below starts the session.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=~2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-30 09:42:00")
        .output()
        .expect("run drop human");
    assert_success(&human);
    assert_stdout_has_no_ansi(&human);
    let out = stdout(&human);
    assert!(out.contains("[dry-run] ok would start"), "{out}");
    assert!(out.contains("CAPTURE 0945-1010 (25m) at line 5"), "{out}");
    assert!(out.contains("  1 [*]"), "{out}");
    assert!(
        out.contains(
            "dropped 2 [[bob#^web-capture]] · stays in NOW · +1 nested line"
        ),
        "{out}"
    );
    assert!(out.contains("  3 [*]"), "{out}");
    assert!(out.contains("Dropped 2"), "{out}");
    assert!(!out.contains("nothing queued"), "{out}");
    // Dry run writes nothing.
    let day_before = fs::read_to_string(&day_file).expect("day before");
    assert!(day_before.contains("- [ ] () — CAPTURE"), "{day_before}");

    let json = run_start_json(&vault, &day_file, "=~2");
    assert_eq!(json["kind"], "pomodoro_start", "{json}");
    assert_eq!(json["text"], "=~2", "{json}");
    let start = &json["pomodoro_start"];
    assert_eq!(start["drop"], serde_json::json!([2]), "{json}");
    let tasks = start["tasks"].as_array().expect("tasks").clone();
    let indices: Vec<u64> = tasks
        .iter()
        .map(|row| row["index"].as_u64().expect("index"))
        .collect();
    assert_eq!(indices, vec![1, 3], "{json}");
    let dropped = start["dropped"].as_array().expect("dropped").clone();
    assert_eq!(dropped.len(), 1, "{json}");
    assert_eq!(dropped[0]["index"], 2, "{json}");
    assert_eq!(dropped[0]["now"], true, "{json}");
    assert_eq!(dropped[0]["block_link"], "[[bob#^web-capture]]", "{json}");
    assert_eq!(dropped[0]["nested_lines"], 1, "{json}");
    assert_eq!(dropped[0]["ledger_line"], 7, "{json}");
    // Kept rows carry no `nested_lines`.
    assert!(tasks[0].get("nested_lines").is_none(), "{json}");

    let day_after = fs::read_to_string(&day_file).expect("day after");
    assert!(
        day_after.contains("- [ ] (**0945-1010** [t:: 25m]) — CAPTURE"),
        "{day_after}"
    );
    assert!(!day_after.contains("[[bob#^web-capture]]"), "{day_after}");
    assert!(
        !day_after.contains("remember the URL parser"),
        "{day_after}"
    );
    assert!(day_after.contains("[[bob#^capture-stop]]"), "{day_after}");
    assert!(day_after.contains("[[sase#^axe-restart]]"), "{day_after}");
    // Starting never touches task notes.
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert!(bob_after.contains("^web-capture"), "{bob_after}");
}

#[test]
fn start_drop_counted_offset_named_and_empty_session() {
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-counted");
    let counted = run_start_json(&vault, &day_file, "=3~2");
    assert_eq!(counted["kind"], "pomodoro_start", "{counted}");
    assert_eq!(
        counted["pomodoro_start"]["drop"],
        serde_json::json!([2]),
        "{counted}"
    );
    assert_eq!(
        counted["pomodoro_start"]["duration_minutes"], 15,
        "{counted}"
    );

    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-offset");
    let offset = run_start_json(&vault, &day_file, "=-2~1");
    assert_eq!(
        offset["pomodoro_start"]["drop"],
        serde_json::json!([1]),
        "{offset}"
    );
    assert_eq!(offset["pomodoro_start"]["offset_units"], 2, "{offset}");

    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-dur-off");
    let dur_off = run_start_json(&vault, &day_file, "=2-1~3");
    assert_eq!(
        dur_off["pomodoro_start"]["drop"],
        serde_json::json!([3]),
        "{dur_off}"
    );

    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-multi");
    let multi = run_start_json(&vault, &day_file, "=~2,3");
    let start = &multi["pomodoro_start"];
    assert_eq!(start["drop"], serde_json::json!([2, 3]), "{multi}");
    let indices: Vec<u64> = start["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|row| row["index"].as_u64().expect("index"))
        .collect();
    assert_eq!(indices, vec![1], "{multi}");

    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-empty");
    let empty = run_start_json(&vault, &day_file, "=~1,2,3");
    let start = &empty["pomodoro_start"];
    assert_eq!(start["drop"], serde_json::json!([1, 2, 3]), "{empty}");
    assert_eq!(start["tasks"], serde_json::json!([]), "{empty}");
    assert_eq!(
        start["dropped"].as_array().expect("dropped").len(),
        3,
        "{empty}"
    );

    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-named");
    let named = run_start_json(&vault, &day_file, "=#capture~1,3");
    let start = &named["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "CAPTURE", "{named}");
    assert_eq!(start["drop"], serde_json::json!([1, 3]), "{named}");
    let indices: Vec<u64> = start["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|row| row["index"].as_u64().expect("index"))
        .collect();
    assert_eq!(indices, vec![2], "{named}");
}

#[test]
fn start_drop_out_of_range_and_empty_lineup() {
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-range");
    let error = run_start_expect_error(&vault, &day_file, &["=~4"]);
    assert!(
        error.contains(
            "`=~4` names task 4, but CAPTURE has 3 queued Task Links (1–3)"
        ),
        "{error}"
    );

    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-sase");
    let error = run_start_expect_error(&vault, &day_file, &["=#sase~1"]);
    assert!(
        error.contains("`=#sase~1` names task 1, but SASE has no queued Task Links; start it with `=#sase`"),
        "{error}"
    );

    // A named start that creates its session has an empty lineup.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-created");
    let error = run_start_expect_error(&vault, &day_file, &["=#plan~1"]);
    assert!(
        error.contains("`=#plan~1` names task 1, but PLAN starts as a new session with no queued Task Links; start it with `=#plan`"),
        "{error}"
    );
}

#[test]
fn start_drop_lexical_errors() {
    let cases = [
        (
            "=~",
            "`=~` is incomplete: type a task number after `~`",
        ),
        ("=~2,", "`=~2,` is incomplete: type a task number after `,`"),
        ("=~0", "task numbers start at 1"),
        ("=~2,2", "task 2 is listed twice in `=~2,2`"),
        ("=~,2", "expected a task number before `,`"),
        ("=~2,,3", "expected a task number before `,`"),
        ("=~2~3", "use one `~` list: `=~2,3`"),
        (
            "=~2!3",
            "a start can only drop Task Links; `!` completes them when you close (`=x!3`)",
        ),
        (
            "=~2#bugs",
            "write the drop list after the name: `=#bugs~2` instead of `=~2#bugs`",
        ),
        (
            "=~2a",
            "`=~2a` is not a drop list: write `~`, then comma-separated task numbers (for example `=~2,3`)",
        ),
        ("=~99999999999", "task number 99999999999 is too large"),
        (
            "=~ 2",
            "write the task numbers right after `~`, with no spaces (for example `=~2,3`)",
        ),
        (
            "=~2, 3",
            "write the task numbers right after `~`, with no spaces (for example `=~2,3`)",
        ),
        (
            "=#~2",
            "`=#` is incomplete: type a Pomodoro name after `#`",
        ),
    ];
    for (token, message) in cases {
        let (_temp, vault, day_file) = start_lineup_vault(&format!(
            "bob-cli-start-drop-lex-{}",
            token
                .replace('~', "t")
                .replace(',', "c")
                .replace(' ', "s")
                .replace('#', "h")
                .replace('!', "b")
        ));
        let error = run_start_expect_error(&vault, &day_file, &[token]);
        assert!(error.contains(message), "{token}: {error}");
    }
    // Extra text and child lines report the existing shape error.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-shape");
    let error = run_start_expect_error(&vault, &day_file, &["=~2 more"]);
    assert!(
        error.contains("Pomodoro start `=~2` must be the whole capture item"),
        "{error}"
    );
    // `=~2` never becomes a task.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-notask");
    let before = fs::read_to_string(vault.join("bob.md")).expect("bob");
    let _ = run_start_json(&vault, &day_file, "=~2");
    let inbox = vault.join("mac_inbox.md");
    assert!(
        !inbox.exists()
            || !fs::read_to_string(&inbox)
                .unwrap_or_default()
                .contains("=~2")
    );
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("bob"),
        before
    );
}

#[test]
fn start_drop_batch_chain_and_rollback() {
    // Same-line chain: close the running session, then start the next one
    // without link 2 of that session's own lineup.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-chain");
    let _started = run_start_json(&vault, &day_file, "=");
    let chained = run_start_json(&vault, &day_file, "=x =~2");
    assert_eq!(chained["kind"], "pomodoro_close", "{chained}");
    let captures = chained["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2, "{chained}");
    assert_eq!(captures[1]["kind"], "pomodoro_start", "{chained}");
    assert_eq!(
        captures[1]["pomodoro_start"]["drop"],
        serde_json::json!([2]),
        "{chained}"
    );

    // Blank-line batch: `=x`, blank line, `=~2`.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-batch");
    let _started = run_start_json(&vault, &day_file, "=");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("=x\n\n=~2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-30 09:42:00")
        .output()
        .expect("run batch capture");
    assert_success(&output);
    let batch: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    assert_eq!(
        batch["captures"].as_array().expect("captures").len(),
        2,
        "{batch}"
    );

    // `=~2 +2` chain: start without link 2, then extend.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-extend");
    let started = run_start_json(&vault, &day_file, "=~2 +2");
    assert_eq!(started["kind"], "pomodoro_start", "{started}");
    let captures = started["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2, "{started}");
    assert_eq!(captures[1]["kind"], "pomodoro_adjust", "{started}");

    // Batch rollback: a first-item failure writes nothing.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-rollback");
    let before = fs::read_to_string(&day_file).expect("day before");
    let error = run_start_expect_error(&vault, &day_file, &["=~99\n\n="]);
    assert!(error.contains("names task 99"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("day"), before);

    // The running-session error echoes the full typed token.
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-running");
    let _started = run_start_json(&vault, &day_file, "=");
    let error = run_start_expect_error(&vault, &day_file, &["=~2"]);
    assert!(error.contains("then `=~2` to switch sessions"), "{error}");
}

#[test]
fn start_drop_crlf_day_file() {
    let (_temp, vault, day_file) =
        start_lineup_vault("bob-cli-start-drop-crlf");
    let before = fs::read_to_string(&day_file).expect("day");
    let crlf = before.replace('\n', "\r\n");
    fs::write(&day_file, &crlf).expect("write crlf");
    let _ = run_start_json(&vault, &day_file, "=~2");
    let after = fs::read_to_string(&day_file).expect("day after");
    assert!(after.contains("\r\n"), "{after:?}");
    assert!(!after.contains("[[bob#^web-capture]]"), "{after:?}");
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
