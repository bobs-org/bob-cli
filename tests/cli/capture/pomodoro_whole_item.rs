//! Capture pomodoro whole-item start tests.

use super::pomodoro_close::close_worked_vault;
use crate::support::*;
use std::fs;

#[test]
fn capture_pomodoro_whole_item_start_reports_json_and_human() {
    // Design clock: BOB_NOW 09:42 pins every suffix's range.
    let cases = [
        ("=", "0945", "1010", 25, 0),
        ("=3", "0945", "1000", 15, 0),
        ("=-", "0940", "1005", 25, 1),
        ("=-2", "0935", "1000", 25, 2),
        ("=3-", "0940", "0955", 15, 1),
        ("=2-1", "0940", "0950", 10, 1),
        ("=0", "0945", "0945", 0, 0),
        ("=03", "0945", "1000", 15, 0),
    ];
    for (token, expect_start, expect_end, expect_dur, expect_off) in cases {
        let temp = TempDir::new("bob-cli-capture-start-item");
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_file(
            &day_file,
            concat!(
                "## Pomodoros\n",
                "- [ ] () — CAPTURE\n",
                "  - queued note\n",
                "- [ ] () — SASE\n",
            ),
        );
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(token)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:42:00")
            .output()
            .expect("run start capture");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("start JSON");
        assert_eq!(json["ok"], true, "{token}");
        assert_eq!(json["kind"], "pomodoro_start", "{token}");
        assert_eq!(json["text"], token, "{token}");
        assert_eq!(json["placement"], "started", "{token}");
        assert_eq!(json["routed"], false, "{token}");
        assert!(json["route"].is_null(), "{token}: {json}");
        assert_eq!(json["route_label"], "", "{token}");
        assert_eq!(json["relative_target"], "day.md", "{token}");
        assert_eq!(json["created"], "2026-09-28", "{token}");
        assert_eq!(json["pomodoro_name"], "CAPTURE", "{token}");
        let expect_line = format!(
            "- [ ] (**{expect_start}-{expect_end}** [t:: {expect_dur}m]) — CAPTURE"
        );
        assert_eq!(json["task_line"], expect_line, "{token}");
        let start = &json["pomodoro_start"];
        assert_eq!(start["start"], expect_start, "{token}");
        assert_eq!(start["end"], expect_end, "{token}");
        assert_eq!(start["duration_minutes"], expect_dur, "{token}");
        assert_eq!(start["offset_units"], expect_off, "{token}");
        assert_eq!(start["pomodoro_name"], "CAPTURE", "{token}");
        assert_eq!(start["pomodoro_line"], 2, "{token}");
        assert_eq!(start["created_pomodoro"], false, "{token}");
        assert_eq!(
            start["time_range"],
            format!("(**{expect_start}-{expect_end}** [t:: {expect_dur}m])"),
            "{token}"
        );
        // No Task Links under the started entry: the lineup is present
        // but empty.
        assert_eq!(start["tasks"], serde_json::json!([]), "{token}: {json}");
        assert_eq!(
            fs::read_to_string(&day_file).expect("read started day"),
            format!(
                "## Pomodoros\n{expect_line}\n  - queued note\n- [ ] () — SASE\n"
            ),
            "{token}"
        );
    }

    // Human output mirrors the other operators, then the started line.
    let temp = TempDir::new("bob-cli-capture-start-human");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let before = "## Pomodoros\n- [ ] () — CAPTURE\n";
    write_file(&day_file, before);
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=3")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run start human");
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("would start")
            && out.contains("day.md")
            && out.contains("CAPTURE 0945-1000 (15m) at line 2")
            && out.contains("- [ ] (**0945-1000** [t:: 15m]) — CAPTURE")
            && out.contains("nothing queued"),
        "{out}"
    );
    assert_stdout_has_no_ansi(&human);
    assert_eq!(
        fs::read_to_string(&day_file).expect("dry-run writes nothing"),
        before
    );

    // An unnamed entry reads as the next session with no name keys.
    let temp2 = TempDir::new("bob-cli-capture-start-unnamed");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(&day2, "## Pomodoros\n- [ ] ()\n");
    let unnamed = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("--dry-run")
        .arg("=")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run unnamed start");
    assert_success(&unnamed);
    let unnamed_out = stdout(&unnamed);
    assert!(
        unnamed_out.contains("would start")
            && unnamed_out.contains("next session 0945-1010 (25m) at line 2"),
        "{unnamed_out}"
    );
    let unnamed_json = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("=")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run unnamed start json");
    assert_success(&unnamed_json);
    let unnamed_value: serde_json::Value =
        serde_json::from_str(stdout(&unnamed_json).trim())
            .expect("unnamed JSON");
    assert!(unnamed_value["pomodoro_name"].is_null(), "{unnamed_value}");
    assert!(
        unnamed_value["pomodoro_start"]
            .get("pomodoro_name")
            .is_none(),
        "{unnamed_value}"
    );
}

#[test]
fn capture_pomodoro_whole_item_start_selects_first_placeholder_and_moves() {
    // Several futures: the first placeholder starts, left in place when a
    // completed entry already precedes it.
    let temp = TempDir::new("bob-cli-capture-start-first");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — DONE\n",
            "- [ ] () — FIRST\n",
            "- [ ] () — SECOND\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run first-placeholder start");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("start JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "FIRST");
    assert_eq!(json["pomodoro_start"]["pomodoro_line"], 3);
    assert_eq!(json["task_line"], "- [ ] (**0945-1010** [t:: 25m]) — FIRST");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read moved day"),
        concat!(
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — DONE\n",
            "- [ ] (**0945-1010** [t:: 25m]) — FIRST\n",
            "- [ ] () — SECOND\n",
        )
    );

    // A completed entry after the placeholder moves the started block,
    // with its children, to the current slot.
    let temp2 = TempDir::new("bob-cli-capture-start-move");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(
        &day2,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — FIRST\n",
            "  - keep me\n",
            "- [x] (**0830-0855** [t:: 25m]) — DONE\n",
            "- [ ] () — SECOND\n",
        ),
    );
    let moved = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run moving start");
    assert_success(&moved);
    let moved_json: serde_json::Value =
        serde_json::from_str(stdout(&moved).trim()).expect("move JSON");
    assert_eq!(moved_json["pomodoro_start"]["pomodoro_line"], 3);
    assert_eq!(
        fs::read_to_string(&day2).expect("read moved day"),
        concat!(
            "## Pomodoros\n",
            "- [x] (**0830-0855** [t:: 25m]) — DONE\n",
            "- [ ] (**0945-1010** [t:: 25m]) — FIRST\n",
            "  - keep me\n",
            "- [ ] () — SECOND\n",
        )
    );

    // CRLF and a missing final newline survive the rewrite byte for byte.
    let temp3 = TempDir::new("bob-cli-capture-start-crlf");
    let vault3 = temp3.path().join("vault");
    let day3 = vault3.join("day.md");
    write_file(
        &day3,
        "## Pomodoros\r\n- [ ] () — CAPTURE\r\n- [x] (**0830-0855** [t:: 25m]) — DONE",
    );
    let crlf = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault3)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &day3)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run crlf start");
    assert_success(&crlf);
    let day_after = fs::read(&day3).expect("read day bytes");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\r\n",
            "- [x] (**0830-0855** [t:: 25m]) — DONE\r\n",
            "- [ ] (**0945-1010** [t:: 25m]) — CAPTURE",
        )
        .as_bytes()
    );
    assert!(!day_after.ends_with(b"\n"));
}

#[test]
fn capture_pomodoro_whole_item_start_guards_fail_write_free() {
    let run = |text: &str, day: &str, now: &str| {
        let temp = TempDir::new("bob-cli-capture-start-guard");
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_file(&day_file, day);
        let before = fs::read_to_string(&day_file).expect("read before");
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", now)
            .output()
            .expect("run guarded start");
        let after = fs::read_to_string(&day_file).expect("read after");
        (output, before, after)
    };
    let running_named = concat!(
        "## Pomodoros\n",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
        "- [ ] () — SASE\n",
    );

    // A named running session names its range and line and teaches the
    // switch idiom with the user's own token.
    let (output, before, after) =
        run("=3", running_named, "2026-09-28 09:42:00");
    assert!(!output.status.success());
    let error = stdout(&output);
    assert!(
        error.contains(
            "cannot start the next Pomodoro: CAPTURE 0920-0950 is still running at line 3; close it with `=x` first, or capture `=x`, a blank line, then `=3` to switch sessions"
        ),
        "{error}"
    );
    assert_eq!(before, after);

    // The bare token teaches the same idiom with `=`.
    let (output, before, after) =
        run("=", running_named, "2026-09-28 09:42:00");
    assert!(!output.status.success());
    let error = stdout(&output);
    assert!(error.contains("then `=` to switch sessions"), "{error}");
    assert_eq!(before, after);

    // An unnamed running session reads as the current session, including
    // one past its nominal end.
    let running_unnamed = "## Pomodoros\n- [ ] (**0920-0950** [t:: 30m])\n";
    for now in ["2026-09-28 09:42:00", "2026-09-28 10:30:00"] {
        let (output, before, after) = run("=", running_unnamed, now);
        assert!(!output.status.success(), "{now}");
        let error = stdout(&output);
        assert!(
            error.contains(
                "the current session 0920-0950 is still running at line 2"
            ),
            "{now}: {error}"
        );
        assert_eq!(before, after, "{now}");
    }

    // More than one open timed entry names the ledger fix.
    let (output, before, after) = run(
        "=",
        "## Pomodoros\n- [ ] (**0920-0950** [t:: 30m]) — A\n- [ ] (**1000-1025** [t:: 25m]) — B\n",
        "2026-09-28 09:42:00",
    );
    assert!(!output.status.success());
    let error = stdout(&output);
    assert!(
        error.contains(
            "today's ledger has multiple open timed Pomodoros; finish all but one first"
        ),
        "{error}"
    );
    assert_eq!(before, after);

    // No future Pomodoro: empty ledger, only completed entries, and only a
    // non-placeholder open line all name the `^route:block-id=` escape.
    for day in [
        "## Pomodoros\n",
        "## Pomodoros\n- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
        "## Pomodoros\n- [ ] review notes\n",
    ] {
        let (output, before, after) = run("=", day, "2026-09-28 09:42:00");
        assert!(!output.status.success(), "{day}");
        let error = stdout(&output);
        assert!(
            error.contains(
                "no future Pomodoro to start: today's ledger (`day.md`) has no open `- [ ] ()` placeholder (start a new named session with `=#<name>`, or a task's session with `^route:block-id=`)"
            ),
            "{day}: {error}"
        );
        assert_eq!(before, after, "{day}");
    }

    // A missing day file names the note; a missing section reuses the
    // existing copy.
    let temp = TempDir::new("bob-cli-capture-start-missing-day");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    let missing = vault.join("2026").join("20260928.md");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=")
        .env("BOB_DAY_FILE", &missing)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run missing day");
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains(
            "no future Pomodoro to start: today's daily note `2026/20260928.md` does not exist"
        ),
        "{}",
        format_output(&output)
    );
    let (output, before, after) =
        run("=", "# 2026-09-28\n", "2026-09-28 09:42:00");
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("has no Pomodoros section"),
        "{}",
        format_output(&output)
    );
    assert_eq!(before, after);

    // An oversized value fails the shared start overflow before any write.
    let (output, before, after) = run(
        "=99999999999999999999999",
        "## Pomodoros\n- [ ] () — CAPTURE\n",
        "2026-09-28 09:42:00",
    );
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("too large"),
        "{}",
        format_output(&output)
    );
    assert_eq!(before, after);
}

#[test]
fn capture_pomodoro_whole_item_start_rejects_shape_and_forced_flags() {
    let day_contents = "## Pomodoros\n- [ ] () — CAPTURE\n";
    let run = |text: &str| {
        let temp = TempDir::new("bob-cli-capture-start-reject");
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_file(&day_file, day_contents);
        let before = fs::read_to_string(&day_file).expect("read before");
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:42:00")
            .output()
            .expect("run rejection");
        let after = fs::read_to_string(&day_file).expect("read after");
        (output, before, after)
    };

    // Counted tokens with extra text echo the typed token and point at the
    // task-session start instead. Never tasks, never writes.
    for (text, token, suffix) in [
        ("=3 more", "=3", "3"),
        ("=3x", "=3", "3"),
        ("=3s:1", "=3", "3"),
        ("=2-1-", "=2-1", "2-1"),
        ("=-2 @work", "=-2", "-2"),
    ] {
        let (output, before, after) = run(text);
        assert_ne!(output.status.code(), Some(0), "{text} should fail");
        let error = stdout(&output);
        assert!(
            error.contains(&format!(
                "Pomodoro start `{token}` must be the whole capture item"
            )) && error.contains(&format!("`^route:block-id={suffix}`")),
            "{text}: {error}"
        );
        assert_eq!(before, after, "{text} must not write");
    }

    // Exact tokens with child lines fail bare or counted.
    for (stdin_text, token) in [("=\n- child\n", "="), ("=2\n- child\n", "=2")]
    {
        let temp = TempDir::new("bob-cli-capture-start-child");
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_file(&day_file, day_contents);
        let before = fs::read_to_string(&day_file).expect("read before");
        let child = run_with_stdin(
            bob_command()
                .arg("capture")
                .arg("-b")
                .arg(&vault)
                .arg("-f")
                .arg("json")
                .env("BOB_DAY_FILE", &day_file)
                .env("BOB_NOW", "2026-09-28 09:42:00"),
            stdin_text,
        );
        assert_ne!(child.status.code(), Some(0), "{token} should fail");
        assert!(
            stdout(&child).contains(&format!(
                "Pomodoro start `{token}` must be the whole capture item"
            )),
            "{}",
            format_output(&child)
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("read after child"),
            before
        );
    }

    // Every forced destination, task, and clipboard flag is rejected with
    // the start copy.
    for args in [
        vec!["--route", "work"],
        vec!["--route", "work", "--section", "Ideas"],
        vec!["--route", "work", "--task", "some-id"],
        vec!["--route", "work", "--task-ref", "1:abcdef12"],
        vec![
            "--route",
            "work",
            "--task",
            "some-id",
            "--task-section",
            "TITLE",
        ],
        vec!["--clip"],
    ] {
        let temp_f = TempDir::new("bob-cli-capture-start-forced");
        let vault_f = temp_f.path().join("vault");
        let day_f = vault_f.join("day.md");
        write_file(&day_f, day_contents);
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault_f)
            .arg("-f")
            .arg("json");
        for arg in &args {
            command.arg(arg);
        }
        command.arg("=");
        let forced = command
            .env("BOB_DAY_FILE", &day_f)
            .env("BOB_NOW", "2026-09-28 09:42:00")
            .output()
            .expect("run forced");
        assert_ne!(
            forced.status.code(),
            Some(0),
            "{args:?} should fail: {}",
            format_output(&forced)
        );
        assert!(
            stdout(&forced).contains(
                "Pomodoro start `=<X>` cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the start alone"
            ),
            "{args:?}: {}",
            format_output(&forced)
        );
    }

    // Prose that must stay prose: bare tokens with text and mid-body
    // tokens never claim an item.
    for text in ["= foo", "=- foo", "==", "Plan =3"] {
        let temp_p = TempDir::new("bob-cli-capture-start-prose");
        let vault_p = temp_p.path().join("vault");
        let day_p = vault_p.join("day.md");
        write_file(&day_p, day_contents);
        let prose = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_p)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .env("BOB_DAY_FILE", &day_p)
            .env("BOB_NOW", "2026-09-28 09:42:00")
            .output()
            .expect("run prose capture");
        assert_success(&prose);
        let prose_json: serde_json::Value =
            serde_json::from_str(stdout(&prose).trim()).expect("prose JSON");
        assert_eq!(prose_json["kind"], "task", "{text}");
        assert!(
            prose_json.get("pomodoro_start").is_none(),
            "{text}: {prose_json}"
        );
        assert_eq!(
            fs::read_to_string(&day_p).expect("day untouched by prose"),
            day_contents,
            "{text}"
        );
    }

    // Argv spellings reach the parser verbatim: no shell expands here, so
    // the binary must accept the raw `=` forms itself.
    for (args, text) in [
        (vec!["="], "="),
        (vec!["=3"], "=3"),
        (vec!["--", "=-2"], "=-2"),
    ] {
        let temp_a = TempDir::new("bob-cli-capture-start-argv");
        let vault_a = temp_a.path().join("vault");
        let day_a = vault_a.join("day.md");
        write_file(&day_a, day_contents);
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault_a)
            .arg("--dry-run")
            .arg("-f")
            .arg("json");
        for arg in &args {
            command.arg(arg);
        }
        let output = command
            .env("BOB_DAY_FILE", &day_a)
            .env("BOB_NOW", "2026-09-28 09:42:00")
            .output()
            .expect("run argv start");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("argv JSON");
        assert_eq!(json["kind"], "pomodoro_start", "{args:?}");
        assert_eq!(json["text"], text, "{args:?}");
    }

    // The nothing-running errors on `=x`, `+`, and `++` point at `=` when a
    // future Pomodoro exists.
    let temp_h = TempDir::new("bob-cli-capture-start-hints");
    let vault_h = temp_h.path().join("vault");
    let day_h = vault_h.join("day.md");
    write_file(&day_h, day_contents);
    for (args, verb) in [
        (vec!["=x"], "close"),
        (vec!["+5"], "adjust"),
        (vec!["++3"], "shift"),
    ] {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault_h)
            .arg("-f")
            .arg("json")
            .arg("--");
        for arg in &args {
            command.arg(arg);
        }
        let output = command
            .env("BOB_DAY_FILE", &day_h)
            .env("BOB_NOW", "2026-09-28 09:42:00")
            .output()
            .expect("run hint capture");
        assert!(!output.status.success(), "{verb}");
        let error = stdout(&output);
        assert!(
            error.contains("next up is CAPTURE at line 2 (start it with `=`)"),
            "{verb}: {error}"
        );
    }
    // Without a future Pomodoro the hint stays off.
    write_file(
        &day_h,
        "## Pomodoros\n- [x] (**0830-0855** [t:: 25m]) — DONE\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault_h)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("++3")
        .env("BOB_DAY_FILE", &day_h)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run hintless shift");
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("has no open timed Pomodoro to shift")
            && !stdout(&output).contains("start it with"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn capture_pomodoro_whole_item_start_batches_compose_and_roll_back() {
    // `=x`, blank line, `=` switches sessions atomically: the close's
    // carried placeholder starts.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-start-switch");
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00"),
        "=x\n\n=\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("switch JSON");
    assert_eq!(json["captures"].as_array().expect("captures").len(), 2);
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_start");
    assert_eq!(json["captures"][1]["placement"], "started");
    assert_eq!(
        json["captures"][1]["pomodoro_start"]["created_pomodoro"],
        false
    );
    assert_eq!(
        json["captures"][1]["task_line"],
        "- [ ] (**0940-1005** [t:: 25m]) — CAPTURE"
    );
    assert!(
        fs::read_to_string(&day_file)
            .expect("read switched day")
            .contains("- [ ] (**0940-1005** [t:: 25m]) — CAPTURE"),
        "{}",
        fs::read_to_string(&day_file).expect("read switched day")
    );

    // `=`, blank line, `+2` starts then extends through the staged file.
    let temp = TempDir::new("bob-cli-capture-start-extend");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n- [ ] () — CAPTURE\n");
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:42:00"),
        "=\n\n+2\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("extend JSON");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_start");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_adjust");
    assert_eq!(
        json["captures"][1]["pomodoro_adjust"]["after_duration_minutes"],
        35
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read extended day"),
        "## Pomodoros\n- [ ] (**0945-1020** [t:: 35m]) — CAPTURE\n"
    );

    // `=`, blank line, `^bob:ready` starts then links a task into the
    // now-running session.
    let temp_l = TempDir::new("bob-cli-capture-start-link");
    let vault_l = temp_l.path().join("vault");
    let day_l = vault_l.join("day.md");
    write_toggle_task_settings(&vault_l);
    write_file(&day_l, "## Pomodoros\n- [ ] () — SASE\n");
    write_file(
        &vault_l.join("bob.md"),
        "## Tasks\n\n- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
    );
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_l)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_l)
            .env("BOB_NOW", "2026-09-28 09:42:00"),
        "=\n\n^bob:ready\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("link JSON");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_start");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_link");
    assert!(
        fs::read_to_string(&day_l)
            .expect("read linked day")
            .contains("[[bob#^ready]]"),
        "{}",
        fs::read_to_string(&day_l).expect("read linked day")
    );

    // A start plus an ordinary `@@`-routed task applies in order: the `@@`
    // declaration never turns the start into a task.
    let temp_o = TempDir::new("bob-cli-capture-start-ordinary");
    let vault_o = temp_o.path().join("vault");
    let day_o = vault_o.join("day.md");
    write_file(&day_o, "## Pomodoros\n- [ ] () — SASE\n");
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_o)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_o)
            .env("BOB_NOW", "2026-09-28 09:42:00"),
        "@@bob\n=\n\nplain second\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("ordinary JSON");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_start");
    assert_eq!(json["captures"][1]["kind"], "task");
    assert_eq!(json["captures"][1]["route"], "bob");
    let routed_note = vault_o.join("bob.md");
    assert!(
        fs::read_to_string(&routed_note)
            .expect("read routed note")
            .contains("plain second"),
        "{}",
        fs::read_to_string(&routed_note).unwrap_or_default()
    );

    // Dry-run computes without committing.
    let temp_d = TempDir::new("bob-cli-capture-start-dry-run");
    let vault_d = temp_d.path().join("vault");
    let day_d = vault_d.join("day.md");
    let before = "## Pomodoros\n- [ ] () — CAPTURE\n";
    write_file(&day_d, before);
    let dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault_d)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("=")
        .env("BOB_DAY_FILE", &day_d)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run start dry-run");
    assert_success(&dry);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry-run JSON");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(
        dry_json["task_line"],
        "- [ ] (**0945-1010** [t:: 25m]) — CAPTURE"
    );
    assert_eq!(
        fs::read_to_string(&day_d).expect("untouched dry-run day"),
        before
    );

    // A late failing item rolls back the staged start.
    let temp_r = TempDir::new("bob-cli-capture-start-rollback");
    let vault_r = temp_r.path().join("vault");
    let day_r = vault_r.join("day.md");
    write_file(&day_r, before);
    write_file(
        &vault_r.join("bob.md"),
        "## Tasks\n\n- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
    );
    let bad = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_r)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_r)
            .env("BOB_NOW", "2026-09-28 09:42:00"),
        "=\n\n^bob:missing\n",
    );
    assert!(!bad.status.success(), "{}", format_output(&bad));
    assert_eq!(fs::read_to_string(&day_r).expect("rolled back day"), before);
}

#[test]
fn capture_pomodoro_whole_item_start_reports_queued_tasks() {
    // Design clock: BOB_NOW 09:42 starts `=` as 0945-1010.
    let temp = TempDir::new("bob-cli-capture-start-lineup");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^ready-task]]\n",
            "\t- [[bob#^next-task]]\n",
            "\t- [[bob#^progress-task]]\n",
            "\t- [[bob#^blocked-task]]\n",
            "\t- ![[bob#^embed-task]]\n",
            "\t- [[bob#^gone]]\n",
            "\t- [[missing#^nowhere]]\n",
            "\t\t- [[bob#^nested]]\n",
            "\t- a queued note\n",
            "\t- [[bob#^mixed]] plus extra text\n",
            "\t- ~~[[bob#^struck]]~~\n",
            "- [ ] () — SASE\n",
        ),
    );
    let bob_before = concat!(
        "## Tasks\n",
        "\n",
        "- [ ] #task Ready work [created::2026-09-20] ^ready-task\n",
        "- [*] #task Next work [created::2026-09-20] ^next-task\n",
        "- [/] #task Progress work [created::2026-09-20] ^progress-task\n",
        "- [?] #task Blocked work [created::2026-09-20] ^blocked-task\n",
        "- [ ] #task Embedded work [created::2026-09-20] ^embed-task\n",
    );
    write_file(&vault.join("bob.md"), bob_before);
    // Human output lists one row per queued task in the close's row style,
    // with the unchanged marker and a dim warning for unresolved rows. This
    // dry run goes first: the JSON capture below actually starts the
    // session, after which a start preview reports it as running.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run lineup human");
    assert_success(&human);
    let out = stdout(&human);
    for line in [
        "would start",
        "CAPTURE 0945-1010 (25m) at line 2",
        "[ ] Ready work bob.md ^ready-task",
        "[*] Next work bob.md ^next-task",
        "[/] Progress work bob.md ^progress-task",
        "[?] Blocked work bob.md ^blocked-task",
        "[ ] Embedded work bob.md ^embed-task",
        "warning: bob.md has no task with block ID ^gone",
        "warning: [[missing#^nowhere]] does not resolve to a vault note",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(!out.contains("nothing queued"), "{out}");
    assert_stdout_has_no_ansi(&human);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run lineup capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("lineup JSON");
    assert_eq!(json["kind"], "pomodoro_start");
    let tasks = json["pomodoro_start"]["tasks"]
        .as_array()
        .expect("tasks array")
        .clone();
    // Only direct-child bare links, in ledger order: notes, mixed lines,
    // struck links, and deeper descendants stay out.
    assert_eq!(tasks.len(), 7, "{json}");
    let row = |index: usize| &tasks[index];
    assert_eq!(
        row(0),
        &serde_json::json!({
            "index": 1,
            "block_link": "[[bob#^ready-task]]",
            "embedded": false,
            "ledger_line": 3,
            "resolved": true,
            "relative_target": "bob.md",
            "block_id": "ready-task",
            "text": "Ready work",
            "status_symbol": " ",
            "status_name": "Ready",
            "warning": null,
        }),
        "{json}"
    );
    assert_eq!(row(1)["status_symbol"], "*");
    assert_eq!(row(1)["status_name"], "Next");
    assert_eq!(row(1)["text"], "Next work");
    assert_eq!(row(1)["ledger_line"], 4);
    assert_eq!(row(2)["status_symbol"], "/");
    assert_eq!(row(2)["status_name"], "In Progress");
    assert_eq!(row(2)["text"], "Progress work");
    assert_eq!(row(3)["status_symbol"], "?");
    assert_eq!(row(3)["status_name"], "Blocked");
    assert_eq!(row(3)["text"], "Blocked work");
    assert_eq!(
        row(4),
        &serde_json::json!({
            "index": 5,
            "block_link": "[[bob#^embed-task]]",
            "embedded": true,
            "ledger_line": 7,
            "resolved": true,
            "relative_target": "bob.md",
            "block_id": "embed-task",
            "text": "Embedded work",
            "status_symbol": " ",
            "status_name": "Ready",
            "warning": null,
        }),
        "{json}"
    );
    // Unresolved rows carry explicit nulls, never omitted keys.
    for key in ["relative_target", "text", "status_symbol", "status_name"] {
        assert!(row(5).get(key).is_some(), "{key} omitted: {json}");
    }
    assert_eq!(
        row(5),
        &serde_json::json!({
            "index": 6,
            "block_link": "[[bob#^gone]]",
            "embedded": false,
            "ledger_line": 8,
            "resolved": false,
            "relative_target": "bob.md",
            "block_id": "gone",
            "text": null,
            "status_symbol": null,
            "status_name": null,
            "warning": "bob.md has no task with block ID ^gone",
        }),
        "{json}"
    );
    assert_eq!(row(6)["resolved"], false);
    assert!(row(6)["relative_target"].is_null(), "{json}");
    assert_eq!(
        row(6)["warning"],
        "[[missing#^nowhere]] does not resolve to a vault note",
        "{json}"
    );
    // Starting never touches task notes.
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("read bob.md"),
        bob_before
    );

    // A task captured earlier in the same draft resolves in the lineup.
    let temp_s = TempDir::new("bob-cli-capture-start-staged-task");
    let vault_s = temp_s.path().join("vault");
    let day_s = vault_s.join("day.md");
    write_toggle_task_settings(&vault_s);
    // The task capture inserts its own `[[bob#^fresh]]` ledger link under
    // the placeholder; the staged task note then resolves in the lineup.
    write_file(&day_s, "## Pomodoros\n- [ ] () — SASE\n");
    let staged = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_s)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_s)
            .env("BOB_NOW", "2026-09-28 09:42:00"),
        "Fresh work @bob:fresh\n\n=\n",
    );
    assert_success(&staged);
    let staged_json: serde_json::Value =
        serde_json::from_str(stdout(&staged).trim())
            .expect("staged lineup JSON");
    assert_eq!(
        staged_json["captures"][1]["pomodoro_start"]["tasks"],
        serde_json::json!([{
            "index": 1,
            "block_link": "[[bob#^fresh]]",
            "embedded": false,
            "ledger_line": 3,
            "resolved": true,
            "relative_target": "bob.md",
            "block_id": "fresh",
            "text": "Fresh work",
            "status_symbol": "*",
            "status_name": "Next",
            "warning": null,
        }]),
        "{staged_json}"
    );

    // Link and task starts stay byte-stable: they omit `tasks`. The link
    // check runs against the lineup vault's running session with a task
    // that is not already queued there.
    write_file(
        &vault.join("bob.md"),
        &format!(
            "{bob_before}- [ ] #task Spare work [created::2026-09-20] ^spare\n"
        ),
    );
    let link = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("^bob:spare")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run link start");
    assert_success(&link);
    let link_json: serde_json::Value =
        serde_json::from_str(stdout(&link).trim()).expect("link JSON");
    assert_eq!(link_json["kind"], "pomodoro_link");
    assert!(
        link_json["pomodoro_start"].get("tasks").is_none(),
        "{link_json}"
    );
    let temp_t = TempDir::new("bob-cli-capture-start-task-omits");
    let vault_t = temp_t.path().join("vault");
    let day_t = vault_t.join("day.md");
    write_toggle_task_settings(&vault_t);
    write_file(&day_t, "## Pomodoros\n- [ ] () — SASE\n");
    let task = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault_t)
        .arg("-f")
        .arg("json")
        .arg("More @bob:tasked=")
        .env("BOB_DAY_FILE", &day_t)
        .env("BOB_NOW", "2026-09-28 09:42:00")
        .output()
        .expect("run task start");
    assert_success(&task);
    let task_json: serde_json::Value =
        serde_json::from_str(stdout(&task).trim()).expect("task JSON");
    assert_eq!(task_json["kind"], "pomodoro_task");
    assert!(
        task_json["pomodoro_start"].get("tasks").is_none(),
        "{task_json}"
    );
}

#[test]
fn capture_pomodoro_whole_item_start_reports_moved_block() {
    let temp = TempDir::new("bob-cli-capture-start-blocks");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] () — GTD\n",
            "\t- [[#^gtd]]\n",
            "\t- more context\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
        ),
    );
    let day_before = fs::read_to_string(&day_file).expect("read day before");

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-09-28 09:42:00",
        &["="],
    );
    let day_after = fs::read_to_string(&day_file).expect("read day after");
    assert_pomodoro_blocks_cover_changes(&day_before, &day_after, &json);
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 3,
                "name": "GTD",
                "time_range": "0945-1010",
                "status": "running",
                "created": false,
                "roles": ["started"],
                "lines": [
                    {
                        "text": "- [ ] (**0945-1010** [t:: 25m]) — GTD",
                        "depth": 0,
                        "change": "changed",
                        "before": "- [ ] () — GTD",
                    },
                    {
                        "text": "\t- [[#^gtd]]",
                        "depth": 1,
                        "change": "unchanged",
                    },
                    {
                        "text": "\t- more context",
                        "depth": 1,
                        "change": "unchanged",
                    },
                ],
            },
        ])
    );
}
