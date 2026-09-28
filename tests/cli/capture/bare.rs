//! Bare terminal and bare hash tests.

use crate::support::*;
use std::fs;

#[test]
fn capture_bare_terminal_marker_writes_pomodoro_note_under_current_pomodoro() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-current");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] Current (0900-0930)\n  - existing child\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("jot")
        .arg("this")
        .arg("#")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture");

    assert_success(&output);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_note");
    assert_eq!(json["routed"], false);
    assert!(json["route"].is_null());
    assert_eq!(json["text"], "jot this");
    assert_eq!(json["task_line"], "- jot this");
    assert_eq!(json["placement"], "appended");
    assert_eq!(json["day_file"], day_file.display().to_string());
    assert_eq!(json["parent_line"], 3);
    assert_eq!(json["parent_text"], "Current (0900-0930)");
    assert!(json["block_id"].is_null());
    assert!(json["block_link"].is_null());
    assert!(json["pomodoro_link_placement"].is_null());
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - existing child\n",
            "  - jot this\n",
        )
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "Pomodoro-note capture must not create the default inbox"
    );
}

#[test]
fn capture_bare_terminal_marker_human_output_shows_the_ledger_entry() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-human");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("jot")
        .arg("this")
        .arg("#")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("captured  day.md"), "{out}");
    assert!(out.contains("under Current (0900-0930)"), "{out}");
    assert!(out.contains("- jot this"), "{out}");
}

#[test]
fn capture_bare_terminal_marker_falls_back_to_the_last_completed_pomodoro() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-completed");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [x] Done (0900-0930)\n- [ ] Next ()\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("note")
        .arg("this")
        .arg("#")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture with no current entry");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("Pomodoro-note JSON");
    assert_eq!(json["parent_text"], "Done (0900-0930)");
    assert_eq!(json["parent_line"], 3);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n\n",
            "- [x] Done (0900-0930)\n",
            "  - note this\n",
            "- [ ] Next ()\n",
        )
    );
}

#[test]
fn capture_bare_terminal_marker_falls_back_to_the_first_future_pomodoro() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-future");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Next ()\n- [ ] Later ()\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("note")
        .arg("this")
        .arg("#")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture with only future entries");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("Pomodoro-note JSON");
    assert_eq!(json["parent_text"], "Next ()");
    assert_eq!(json["parent_line"], 3);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Next ()\n",
            "  - note this\n",
            "- [ ] Later ()\n",
        )
    );
}

#[test]
fn capture_bare_terminal_marker_selects_the_last_of_two_completed_pomodoros() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-last-completed");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [x] First (0900-0930)\n- [x] Second (1000-1030)\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("note")
        .arg("this")
        .arg("#")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture with two completed entries");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("Pomodoro-note JSON");
    assert_eq!(json["parent_text"], "Second (1000-1030)");
    assert_eq!(json["parent_line"], 4);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n\n",
            "- [x] First (0900-0930)\n",
            "- [x] Second (1000-1030)\n",
            "  - note this\n",
        )
    );
}

#[test]
fn capture_bare_terminal_marker_multiple_open_timed_pomodoros_is_io_error() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-multiple-timed");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let day_before =
        "## Pomodoros\n\n- [ ] One (0900-0930)\n- [ ] Two (1000-1030)\n";
    write_file(&day_file, day_before);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("note")
        .arg("this")
        .arg("#")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture with two timed entries");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("multiple open timed Pomodoros"),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read untouched day"),
        day_before
    );
}

#[test]
fn capture_bare_terminal_marker_preflight_failures_leave_daily_note_untouched()
{
    let cases = [
        (
            "missing-section",
            Some("# Daily\n\nNo Pomodoros heading here.\n"),
            "Bob daily note has no Pomodoros section",
        ),
        (
            "no-eligible",
            Some("## Pomodoros\n\n- [-] Cancelled ()\n"),
            "Bob daily note has no eligible Pomodoro",
        ),
        (
            "empty-section",
            Some("## Pomodoros\n"),
            "Bob daily note has no eligible Pomodoro",
        ),
        ("missing-daily-note", None, "Bob daily note does not exist"),
    ];

    for (name, day_before, expected_error) in cases {
        let temp =
            TempDir::new(&format!("bob-cli-capture-pomodoro-note-{name}"));
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        let day_file = vault.join("day.md");
        if let Some(day_before) = day_before {
            write_file(&day_file, day_before);
        }

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("note")
            .arg("this")
            .arg("#")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00")
            .output()
            .expect("run failing Pomodoro-note capture");

        assert_eq!(
            output.status.code(),
            Some(1),
            "{name}: {}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(expected_error),
            "{name}: {}",
            format_output(&output)
        );
        match day_before {
            Some(day_before) => assert_eq!(
                fs::read_to_string(&day_file).expect("read untouched day"),
                day_before,
                "{name}"
            ),
            None => assert!(
                !day_file.exists(),
                "{name}: daily note should not be created"
            ),
        }
    }
}

#[test]
fn capture_bare_terminal_marker_dry_run_reports_plan_and_writes_nothing() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-dry-run");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let day_before = "## Pomodoros\n\n- [ ] Current (0900-0930)\n";
    write_file(&day_file, day_before);

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("note")
        .arg("this")
        .arg("#")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run dry-run Pomodoro-note capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["kind"], "pomodoro_note");
    assert_eq!(json["task_line"], "- note this");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read untouched day"),
        day_before
    );
}

#[test]
fn capture_bare_terminal_marker_batch_items_land_under_the_same_pomodoro_in_order(
) {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-batch");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");

    let draft = concat!("first note #", "\n", "\n", "second note #");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg(draft)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run batched Pomodoro-note capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - first note\n",
            "  - second note\n",
        )
    );
}

#[test]
fn capture_bare_terminal_marker_batch_items_stack_under_the_same_completed_pomodoro(
) {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-batch-completed");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [x] Done (0900-0930)\n");

    let draft = concat!("first note #", "\n", "\n", "second note #");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg(draft)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run batched Pomodoro-note capture on a completed ledger");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n\n",
            "- [x] Done (0900-0930)\n",
            "  - first note\n",
            "  - second note\n",
        )
    );
}

#[test]
fn capture_bare_terminal_marker_keeps_crlf_line_endings() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-crlf");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "## Pomodoros\r\n\r\n- [ ] Current (0900-0930)\r\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("note")
        .arg("this")
        .arg("#")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture on a CRLF daily note");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        "## Pomodoros\r\n\r\n- [ ] Current (0900-0930)\r\n  - note this\r\n"
    );
}
