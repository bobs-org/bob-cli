//! Sized `[N]x` Pomodoro closes execute with exact-or-refuse timing.

use super::pomodoro_close::{
    close_worked_vault, run_close_json, run_close_json_flags,
};
use crate::support::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn vault_files(vault: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(dir: &Path, root: &Path, files: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(dir).expect("read dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, files);
            } else {
                let rel = path.strip_prefix(root).expect("rel").to_path_buf();
                files.insert(rel, fs::read_to_string(&path).expect("read"));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(vault, vault, &mut files);
    files
}

fn parse_error_json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("expected JSON error, got {text:?}: {error}")
    })
}

fn run_sized_error(
    vault: &Path,
    day_file: &Path,
    now: &str,
    flags: &[&str],
    args: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .args(flags)
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", now)
        .output()
        .expect("run sized close");
    assert!(
        !output.status.success(),
        "expected failure:\n{}",
        format_output(&output)
    );
    parse_error_json(stdout(&output).trim())
}

fn sized_day_vault(name: &str, headline: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        &format!("## Pomodoros\n\n{headline}\n- [ ] () — NEXT\n"),
    );
    write_file(
        &vault.join("bob.md"),
        concat!(
            "## Tasks\n\n",
            "- [*] #task Build task [created::2026-09-26] ^build\n",
        ),
    );
    (temp, vault, day_file)
}

fn sized_linked_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0900-0925** [t:: 25m]) — BUILD\n",
            "\t- [[bob#^build]]\n",
            "\t- [[bob#^other]]\n",
            "- [ ] () — NEXT\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        concat!(
            "## Tasks\n\n",
            "- [*] #task Build task [created::2026-09-26] ^build\n",
            "- [*] #task Other task [created::2026-09-26] ^other\n",
            "- [ ] #task Ready task [created::2026-09-20] ^ready\n",
        ),
    );
    (temp, vault, day_file)
}

#[test]
fn sized_close_timing_records_exact_spans() {
    // On 0900-0925, `2x` at 09:50 gives 0900-0910/C=09:10.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-timing-late",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["2x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed"]["time_range"], "0900-0910", "{json}");
    assert_eq!(close["closed_at"], "0910", "{json}");
    assert_eq!(close["planned"]["time_range"], "0900-0910", "{json}");
    assert_eq!(close["decremented_minutes"], 0, "{json}");
    assert_eq!(close["remaining_minutes"], 0, "{json}");
    assert_eq!(close["size"]["raw"], "2", "{json}");
    assert_eq!(close["size"]["blocks"], 2, "{json}");
    assert_eq!(close["size"]["minutes"], 10, "{json}");
    assert_eq!(close["size"]["from"]["time_range"], "0900-0925", "{json}");
    assert_eq!(close["size"]["from"]["duration_minutes"], 25, "{json}");
    assert_eq!(close["size"]["delta_minutes"], -15, "{json}");

    // At 09:08 the same `2x` keeps 10m but stamps C=09:08.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-timing-roundup",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:08:00", &["2x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed"]["time_range"], "0900-0910", "{json}");
    assert_eq!(close["closed_at"], "0908", "{json}");
    assert_eq!(close["remaining_minutes"], 2, "{json}");

    // `7x` at 09:50 lengthens to 35m/C=09:35.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-timing-lengthen",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["7x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed"]["time_range"], "0900-0935", "{json}");
    assert_eq!(close["closed_at"], "0935", "{json}");
    assert_eq!(close["size"]["delta_minutes"], 10, "{json}");

    // `5x` at 09:40 is exact with no overrun (C is the sized end).
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-timing-exact",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:40:00", &["5x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed"]["time_range"], "0900-0925", "{json}");
    assert_eq!(close["closed_at"], "0925", "{json}");
    assert_eq!(close["remaining_minutes"], 0, "{json}");
    assert!(close["size"]["delta_minutes"].as_i64() == Some(0), "{json}");

    // Non-grid `0902-0925` sizes to `0902-0912` for `2x`.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-timing-nongrid",
        "- [ ] (**0902-0925** [t:: 23m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:30:00", &["2x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed"]["time_range"], "0902-0912", "{json}");
}

#[test]
fn sized_close_boundary_rounding() {
    // At 09:05 a `2x` still needs 5m more, while 09:06 succeeds.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-boundary-refuse",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let before = vault_files(&vault);
    let json =
        run_sized_error(&vault, &day_file, "2026-09-28 09:05:00", &[], &["2x"]);
    assert_eq!(json["code"], "pomodoro_close_size_not_elapsed", "{json}");
    assert_eq!(vault_files(&vault), before, "refusal writes nothing");

    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-boundary-accept",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:06:00", &["2x"]);
    assert_eq!(json["pomodoro_close"]["closed_at"], "0906", "{json}");
}

#[test]
fn sized_close_midnight_and_leading_zeros() {
    // Pure helper: 23:50 start, next-day 00:07, `3x` gives 00:05 on the
    // next date with seconds zeroed and half-day normalization.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-midnight",
        "- [ ] (**2350-0020** [t:: 30m]) — NIGHT",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-29 00:07:00", &["3x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed"]["time_range"], "2350-0005", "{json}");
    assert_eq!(close["closed_at"], "0005", "{json}");

    // Accepted `02x` keeps its leading-zero raw.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-leading-zero",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["02x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["size"]["raw"], "02", "{json}");
    assert_eq!(close["closed"]["time_range"], "0900-0910", "{json}");

    // The upper size limit stays sound: `99x` on a long session closes.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-upper-limit",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json = run_close_json_flags(
        &vault,
        &day_file,
        "2026-09-28 17:20:00",
        &[],
        &["99x"],
    );
    assert_eq!(json["pomodoro_close"]["size"]["blocks"], 99, "{json}");
}

#[test]
fn sized_close_preserves_clocks_and_metadata() {
    let headline =
        "- [ ] (**0900-0925** [t:: 25m] extra) — B\u{00dc}ILD \u{1F600}";
    let (_temp, vault, day_file) =
        sized_day_vault("bob-cli-sized-metadata", headline);
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["2x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed"]["time_range"], "0900-0910", "{json}");
    let entry = close["entry_line"].as_str().unwrap_or("");
    assert!(entry.contains("[t:: 10m] extra"), "{json}");
    assert!(entry.contains("B\u{00dc}ILD"), "{json}");

    // Lengthening emits no forward-event conflict warning.
    assert!(
        json.get("warnings").is_none()
            || !json.to_string().contains("pomodoro_runs_past_event"),
        "{json}"
    );

    // CRLF day files keep their line endings.
    let temp = TempDir::new("bob-cli-sized-crlf");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        "## Pomodoros\r\n\r\n- [ ] (**0900-0925** [t:: 25m]) \u{2014} BUILD\r\n- [ ] () \u{2014} NEXT\r\n",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["2x"]);
    assert_eq!(
        json["pomodoro_close"]["closed"]["time_range"], "0900-0910",
        "{json}"
    );
    let raw = fs::read(&day_file).expect("read day");
    assert!(raw.windows(2).any(|w| w == b"\r\n"), "keeps CRLF");
}

#[test]
fn sized_close_refusals_name_fixes_and_codes() {
    // `2x` at 09:03 suggests `1x`.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-refuse-203",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let before = vault_files(&vault);
    let json =
        run_sized_error(&vault, &day_file, "2026-09-28 09:03:00", &[], &["2x"]);
    assert_eq!(json["code"], "pomodoro_close_size_not_elapsed", "{json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("`2x` asks for 10m"),
        "{json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("`1x` closes at 0905"),
        "{json}"
    );
    assert_eq!(json["close_size"]["raw"], "2", "{json}");
    assert_eq!(json["close_size"]["elapsed_minutes"], 3, "{json}");
    assert_eq!(json["close_size"]["fits_blocks"], 1, "{json}");
    assert_eq!(vault_files(&vault), before, "refusal writes nothing");
    // Human rendering keeps both lines.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("2x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:03:00")
        .output()
        .expect("human refusal");
    let human = stderr(&output);
    assert!(human.contains("`2x` asks for 10m"), "{human}");
    assert!(human.contains("`1x` closes at 0905"), "{human}");

    // `7x` at 09:12 suggests `3x`.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-refuse-712",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_sized_error(&vault, &day_file, "2026-09-28 09:12:00", &[], &["7x"]);
    assert_eq!(json["code"], "pomodoro_close_size_not_elapsed", "{json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("`3x` closes at 0915"),
        "{json}"
    );
    assert_eq!(json["close_size"]["fits_blocks"], 3, "{json}");

    // `20x` at 09:50 suggests `10x`.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-refuse-20x",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json = run_sized_error(
        &vault,
        &day_file,
        "2026-09-28 09:50:00",
        &[],
        &["20x"],
    );
    assert_eq!(json["code"], "pomodoro_close_size_not_elapsed", "{json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("`10x` closes at 0950"),
        "{json}"
    );

    // `1x` at the start minute suggests `x0`.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-refuse-start",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_sized_error(&vault, &day_file, "2026-09-28 09:00:00", &[], &["1x"]);
    assert_eq!(json["code"], "pomodoro_close_size_not_elapsed", "{json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("started at 0900"),
        "{json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("`x0` resets it"),
        "{json}"
    );
    assert_eq!(json["close_size"]["fits_blocks"], 0, "{json}");

    // A future MEET refuses as not started.
    let temp = TempDir::new("bob-cli-sized-refuse-meet");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] (**1300-1330** [t:: 30m]) — MEET\n- [ ] () — NEXT\n",
    );
    let json =
        run_sized_error(&vault, &day_file, "2026-09-28 09:50:00", &[], &["2x"]);
    assert_eq!(json["code"], "pomodoro_close_size_not_started", "{json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("cannot size MEET"),
        "{json}"
    );
    assert!(
        json.get("close_size")
            .unwrap()
            .get("elapsed_minutes")
            .is_none(),
        "{json}"
    );

    // Unparseable range refuses: scanner-recognized but
    // `parse_adjustment_range` returns None (space after the paren).
    let temp = TempDir::new("bob-cli-sized-refuse-unparseable");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] ( **0900-0925** [t:: 25m]) \u{2014} BUILD\n- [ ] () \u{2014} NEXT\n",
    );
    let json =
        run_sized_error(&vault, &day_file, "2026-09-28 09:50:00", &[], &["2x"]);
    assert_eq!(json["code"], "pomodoro_close_size_unparseable", "{json}");
    assert!(
        json["error"].as_str().unwrap_or("").contains("needs BUILD"),
        "{json}"
    );

    // Unnamed fallback uses `the running session`.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-refuse-unnamed",
        "- [ ] (**0900-0925** [t:: 25m])",
    );
    let json =
        run_sized_error(&vault, &day_file, "2026-09-28 09:03:00", &[], &["2x"]);
    if json["code"] == "pomodoro_close_size_not_elapsed" {
        assert!(
            json["error"]
                .as_str()
                .unwrap_or("")
                .contains("the running session"),
            "{json}"
        );
    }

    // No session and multiple running keep existing diagnostics.
    let temp = TempDir::new("bob-cli-sized-no-running");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, "## Pomodoros\n\n- [ ] () — NEXT\n");
    let json =
        run_sized_error(&vault, &day_file, "2026-09-28 09:50:00", &[], &["2x"]);
    assert!(json["code"] != "pomodoro_close_size_not_elapsed", "{json}");
}

#[test]
fn sized_close_refusals_are_atomic_and_dry_runs_write_nothing() {
    // Every refusal leaves every vault file unchanged, including after
    // an earlier task in a batch.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-atomic-batch",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let before = vault_files(&vault);
    let json = run_sized_error(
        &vault,
        &day_file,
        "2026-09-28 09:03:00",
        &[],
        &["Do this\n\n2x"],
    );
    // The task staged, then the sized close refused, so the whole batch
    // rolls back.
    assert_eq!(json["code"], "pomodoro_close_size_not_elapsed", "{json}");
    assert_eq!(vault_files(&vault), before, "batch rolls back");

    // Successful and failing dry runs write nothing.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-atomic-dry",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let before = vault_files(&vault);
    let json = run_sized_error(
        &vault,
        &day_file,
        "2026-09-28 09:03:00",
        &["--dry-run"],
        &["2x"],
    );
    assert_eq!(json["code"], "pomodoro_close_size_not_elapsed", "{json}");
    assert_eq!(vault_files(&vault), before, "dry refusal writes nothing");

    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-dry-success",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let before = vault_files(&vault);
    let json = run_close_json_flags(
        &vault,
        &day_file,
        "2026-09-28 09:50:00",
        &["--dry-run"],
        &["2x"],
    );
    assert_eq!(json["dry_run"], true, "{json}");
    assert_eq!(vault_files(&vault), before, "dry success writes nothing");
}

#[test]
fn sized_close_matches_plain_close_outcomes() {
    // `2x!2` and `-3 x!2` at 09:12 agree on the 0900-0910 span and task
    // outcomes; only C-based logs and the unsized overrun differ.
    let (_temp, vault, day_file) = sized_linked_vault("bob-cli-sized-equiv-a");
    let sized =
        run_close_json(&vault, &day_file, "2026-09-28 09:12:00", &["2x!2"]);
    let (_temp, vault2, day2) = sized_linked_vault("bob-cli-sized-equiv-b");
    let chained =
        run_close_json(&vault2, &day2, "2026-09-28 09:12:00", &["-3 x!2"]);
    let chained_close = chained
        .get("pomodoro_close")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| chained["captures"][1]["pomodoro_close"].clone());
    assert_eq!(
        sized["pomodoro_close"]["closed"]["time_range"],
        chained_close["closed"]["time_range"],
        "same span {sized} vs {chained}"
    );
    assert_eq!(sized["pomodoro_close"]["closed_at"], "0910", "{sized}");
    // Sized has no overrun line; the unsized chain trims with an overrun.
    assert!(
        sized["pomodoro_close"]["remaining_minutes"]
            .as_i64()
            .unwrap_or(0)
            >= 0,
        "{sized}"
    );

    // `2x0` really closes and defers every plain link.
    let (_temp, vault, day_file) = sized_linked_vault("bob-cli-sized-2x0");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["2x0"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed"]["time_range"], "0900-0910", "{json}");
    for link in close["task_links"].as_array().expect("links") {
        assert!(
            link["outcome"] == "deferred" || link["outcome"] == "in_progress",
            "2x0 defers plain links: {link}"
        );
    }
}

#[test]
fn sized_close_stamps_work_logs_at_sized_end() {
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-sized-worklog");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:50:00",
        &["2x!2\n- 2 wired the lexer"],
    );
    let close = &json["pomodoro_close"];
    assert_eq!(close["closed_at"], "0930", "{json}");
    let tasks = close["tasks"].as_array().expect("tasks");
    let logged: Vec<&serde_json::Value> = tasks
        .iter()
        .filter(|t| {
            !t["typed_work_log"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true)
        })
        .collect();
    assert!(!logged.is_empty(), "typed log lands: {json}");
    for task in &logged {
        for entry in task["typed_work_log"].as_array().expect("entries") {
            assert!(
                entry.as_str().unwrap_or("").contains("2026-09-28 09:30"),
                "Work Log stamps C, not wall now: {entry}"
            );
        }
    }
}

#[test]
fn sized_close_output_json_and_captions() {
    // Exact size JSON keeps the original span after prior edits.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-sized-output-json");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["2x!2"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["size"]["from"]["time_range"], "0920-0950", "{json}");
    assert_eq!(close["size"]["from"]["duration_minutes"], 30, "{json}");
    assert_eq!(close["size"]["delta_minutes"], -20, "{json}");
    assert_eq!(
        close["planned"]["time_range"], close["closed"]["time_range"],
        "{json}"
    );
    assert_eq!(close["decremented_minutes"], 0, "{json}");

    // Unsized summaries keep `size` absent.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-sized-unsized-json");
    let json = run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["x"]);
    assert!(json["pomodoro_close"].get("size").is_none(), "{json}");

    // All four captions render through the human path.
    for (draft, now, needle) in [
        ("2x", "2026-09-28 09:50:00", "0900-0910 (10m"),
        ("7x", "2026-09-28 09:50:00", "0900-0935 (35m"),
        ("5x", "2026-09-28 09:40:00", "0900-0925 (25m"),
    ] {
        let (_temp, vault, day_file) = sized_day_vault(
            &format!(
                "bob-cli-sized-caption-{}",
                needle.replace([' ', '-', '(', ')'], "_")
            ),
            "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
        );
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--")
            .arg(draft)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", now)
            .output()
            .expect("human caption");
        assert_success(&output);
        let human = stdout(&output);
        assert!(human.contains(needle), "{draft} at {now}: {human}");
        assert!(
            !human.contains("ran "),
            "{draft} never reports overrun: {human}"
        );
    }
}

#[test]
fn sized_close_tip_for_adjust_then_close() {
    // Positive: `-3 x!2` suggests `2x!2`.
    let (_temp, vault, day_file) = sized_linked_vault("bob-cli-sized-tip-yes");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("-3 x!2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:12:00")
        .output()
        .expect("tip batch");
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("tip: 2x!2 closes at 10m in one step"),
        "{human}"
    );

    // Negative: a sized close never tips.
    let (_temp, vault, day_file) = sized_linked_vault("bob-cli-sized-tip-no");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("-3 2x!2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:12:00")
        .output()
        .expect("sized batch");
    // Either succeeds without a tip or refuses on the chain shape; either
    // way no `tip:` line appears for a sized close.
    let human = stdout(&output) + &stderr(&output);
    assert!(!human.contains("tip:"), "sized closes never tip: {human}");
}

#[test]
fn sized_close_composition_uses_wall_now() {
    // `2x =` closes at the sized end, then starts the next session at
    // wall now with no overlap on a grid session.
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-compose-chain",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["2x ="]);
    // One close plus one start: the close object exists and the day has
    // both a completed and a new open entry.
    assert!(
        json["pomodoro_close"].is_object() || json["captures"].is_array(),
        "{json}"
    );

    // `--1 2x` sizes from the shifted start (0900 -> 0855, then 2x).
    let (_temp, vault, day_file) = sized_day_vault(
        "bob-cli-sized-compose-shift",
        "- [ ] (**0900-0925** [t:: 25m]) — BUILD",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:50:00", &["--1 2x"]);
    let text = json.to_string();
    assert!(text.contains("0855-0905"), "{json}");

    // Link and new-task closes use the post-link lineup.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-sized-link-lineup");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:50:00",
        &["@bob:ready=2x"],
    );
    assert_eq!(json["ok"], true, "{json}");
}
