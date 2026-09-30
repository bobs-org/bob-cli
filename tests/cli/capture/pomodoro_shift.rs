//! Pomodoro shift execution.

use crate::support::*;
use std::fs;

#[test]
fn capture_pomodoro_shift_moves_and_reports_json() {
    let temp = TempDir::new("bob-cli-capture-shift-moves");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**0900-0925** [t:: 25m]) — FOCUS\n",
            "  - existing context\n",
            "## Later\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("++3")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run shift capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("shift JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_shift");
    assert_eq!(json["text"], "++3");
    assert_eq!(json["placement"], "toggled");
    assert_eq!(json["dry_run"], false);
    assert!(json["route"].is_null(), "{json}");
    assert!(json.get("pomodoro_adjust").is_none(), "{json}");
    let shift = &json["pomodoro_shift"];
    assert_eq!(shift["direction"], "later");
    assert_eq!(shift["requested_units"], 3);
    assert_eq!(shift["delta_minutes"], 15);
    assert_eq!(shift["before_start"], "0900");
    assert_eq!(shift["before_end"], "0925");
    assert_eq!(shift["after_start"], "0915");
    assert_eq!(shift["after_end"], "0940");
    assert_eq!(shift["duration_minutes"], 25);
    assert_eq!(shift["pomodoro_line"], 3);
    assert_eq!(shift["pomodoro_name"], "FOCUS");
    assert_eq!(shift["time_range"], "(**0915-0940** [t:: 25m])");
    assert_eq!(json["pomodoro_name"], "FOCUS");
    assert_eq!(json["task_line"], "- [ ] (**0915-0940** [t:: 25m]) — FOCUS");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read shifted day"),
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**0915-0940** [t:: 25m]) — FOCUS\n",
            "  - existing context\n",
            "## Later\n",
        )
    );

    // An earlier shift reports a signed delta.
    let earlier = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("--2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run earlier shift");
    assert_success(&earlier);
    let earlier_json: serde_json::Value =
        serde_json::from_str(stdout(&earlier).trim()).expect("earlier JSON");
    assert_eq!(earlier_json["kind"], "pomodoro_shift");
    assert_eq!(earlier_json["pomodoro_shift"]["direction"], "earlier");
    assert_eq!(earlier_json["pomodoro_shift"]["requested_units"], 2);
    assert_eq!(earlier_json["pomodoro_shift"]["delta_minutes"], -10);
    assert_eq!(earlier_json["pomodoro_shift"]["before_start"], "0915");
    assert_eq!(earlier_json["pomodoro_shift"]["after_start"], "0905");
    assert_eq!(earlier_json["pomodoro_shift"]["after_end"], "0930");

    // Human output names what shifted and where.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("++1")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run shift human");
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("would shift")
            && out.contains("FOCUS")
            && out.contains("0905-0930 to 0910-0935")
            && out.contains("5m later"),
        "{out}"
    );
    assert_stdout_has_no_ansi(&human);

    // An unnamed entry reads as the current session.
    let temp2 = TempDir::new("bob-cli-capture-shift-unnamed");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(&day2, "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m])\n");
    let unnamed = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("--dry-run")
        .arg("++1")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run unnamed shift");
    assert_success(&unnamed);
    let unnamed_out = stdout(&unnamed);
    assert!(unnamed_out.contains("current session"), "{unnamed_out}");
    let unnamed_json = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("++1")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run unnamed shift json");
    assert_success(&unnamed_json);
    let unnamed_value: serde_json::Value =
        serde_json::from_str(stdout(&unnamed_json).trim())
            .expect("unnamed JSON");
    assert!(
        unnamed_value.get("pomodoro_name").is_none(),
        "{unnamed_value}"
    );
    assert!(
        unnamed_value["pomodoro_shift"]
            .get("pomodoro_name")
            .is_none(),
        "{unnamed_value}"
    );
}

#[test]
fn capture_pomodoro_shift_bare_defaults_and_midnight_wrap() {
    // Every session operator's count is optional and defaults to 1.
    let temp = TempDir::new("bob-cli-capture-shift-bare");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) — FOCUS\n",
    );
    for (text, kind, units, delta) in [
        ("++", "pomodoro_shift", 1, 5),
        ("--", "pomodoro_shift", 1, -5),
        ("+", "pomodoro_adjust", 1, 5),
        ("-", "pomodoro_adjust", 1, -5),
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--dry-run")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:40:00")
            .output()
            .expect("run bare operator");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("bare JSON");
        assert_eq!(json["kind"], kind, "{text}");
        assert_eq!(json["text"], text, "{text}");
        if kind == "pomodoro_shift" {
            assert_eq!(
                json["pomodoro_shift"]["requested_units"], units,
                "{text}"
            );
            assert_eq!(
                json["pomodoro_shift"]["delta_minutes"], delta,
                "{text}"
            );
        } else {
            assert_eq!(
                json["pomodoro_adjust"]["requested_units"], units,
                "{text}"
            );
            assert_eq!(
                json["pomodoro_adjust"]["delta_minutes"], delta,
                "{text}"
            );
        }
    }
    // Leading zeros are accepted.
    let padded = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("++03")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run padded shift");
    assert_success(&padded);
    let padded_json: serde_json::Value =
        serde_json::from_str(stdout(&padded).trim()).expect("padded JSON");
    assert_eq!(padded_json["pomodoro_shift"]["requested_units"], 3);
    assert_eq!(padded_json["pomodoro_shift"]["delta_minutes"], 15);

    // Shifts wrap across midnight like Obsidian's N\o / N\O.
    let temp2 = TempDir::new("bob-cli-capture-shift-wrap");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(
        &day2,
        "## Pomodoros\n- [ ] (**0005-0030** [t:: 25m]) — FOCUS\n",
    );
    let wrap_earlier = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("--2")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 00:40:00")
        .output()
        .expect("run wrap earlier");
    assert_success(&wrap_earlier);
    let wrap_json: serde_json::Value =
        serde_json::from_str(stdout(&wrap_earlier).trim()).expect("wrap JSON");
    assert_eq!(wrap_json["pomodoro_shift"]["after_start"], "2355");
    assert_eq!(wrap_json["pomodoro_shift"]["after_end"], "0020");
    assert_eq!(wrap_json["pomodoro_shift"]["duration_minutes"], 25);
    assert_eq!(
        wrap_json["task_line"],
        "- [ ] (**2355-0020** [t:: 25m]) — FOCUS"
    );

    write_file(
        &day2,
        "## Pomodoros\n- [ ] (**2350-0015** [t:: 25m]) — FOCUS\n",
    );
    let wrap_later = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("++3")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 00:40:00")
        .output()
        .expect("run wrap later");
    assert_success(&wrap_later);
    let wrap_later_json: serde_json::Value =
        serde_json::from_str(stdout(&wrap_later).trim()).expect("wrap JSON");
    assert_eq!(wrap_later_json["pomodoro_shift"]["after_start"], "0005");
    assert_eq!(wrap_later_json["pomodoro_shift"]["after_end"], "0030");
    assert_eq!(
        wrap_later_json["task_line"],
        "- [ ] (**0005-0030** [t:: 25m]) — FOCUS"
    );
}

#[test]
fn capture_pomodoro_shift_preserves_metadata_and_matches_plugin() {
    // Metadata, name, children, and CRLF survive the rewrite.
    let temp = TempDir::new("bob-cli-capture-shift-metadata");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "- [ ] (**0900-0925** [t:: 25m] focus) — DEEP\r\n  - child link\r\n",
    );
    let before = fs::read(&day_file).expect("read before");
    let mut mixed = b"## Pomodoros\n".to_vec();
    mixed.extend_from_slice(&before);
    fs::write(&day_file, &mixed).expect("write mixed endings");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("++2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run metadata shift");
    assert_success(&output);
    let after = fs::read(&day_file).expect("read metadata day");
    let after_text = String::from_utf8(after).expect("utf8 day");
    assert!(
        after_text.contains(
            "(**0910-0935** [t:: 25m] focus) — DEEP\r\n  - child link\r\n"
        ),
        "{after_text}"
    );

    // Legacy stopwatch duration is honored; the duration is unchanged.
    let temp2 = TempDir::new("bob-cli-capture-shift-legacy");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(
        &day2,
        "## Pomodoros\n- [ ] (**0900-0930** \u{23F1} 20m) — LEGACY\n",
    );
    let output2 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("++1")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run legacy shift");
    assert_success(&output2);
    let json2: serde_json::Value =
        serde_json::from_str(stdout(&output2).trim()).expect("legacy JSON");
    assert_eq!(json2["pomodoro_shift"]["duration_minutes"], 20);
    assert_eq!(
        fs::read_to_string(&day2).expect("read legacy day"),
        "## Pomodoros\n- [ ] (**0905-0935** [t:: 20m]) — LEGACY\n",
    );

    // No duration metadata falls back to the displayed range span.
    let temp3 = TempDir::new("bob-cli-capture-shift-range-fallback");
    let vault3 = temp3.path().join("vault");
    let day3 = vault3.join("day.md");
    write_file(&day3, "## Pomodoros\n- [ ] (**0900-0930**) — PLAIN\n");
    let output3 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault3)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("--1")
        .env("BOB_DAY_FILE", &day3)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run fallback shift");
    assert_success(&output3);
    let json3: serde_json::Value =
        serde_json::from_str(stdout(&output3).trim()).expect("fallback JSON");
    assert_eq!(json3["pomodoro_shift"]["duration_minutes"], 30);
    assert_eq!(
        fs::read_to_string(&day3).expect("read fallback day"),
        "## Pomodoros\n- [ ] (**0855-0925** [t:: 30m]) — PLAIN\n",
    );

    // Byte-for-byte with bob-plugins `offsetPomodoroLineRange` on canonical
    // lines: both translate both endpoints mod 1440 and keep the duration
    // (plugins/bob-ledger-tools/main.js `offsetPomodoroLineRange`).
    let temp4 = TempDir::new("bob-cli-capture-shift-plugin-parity");
    let vault4 = temp4.path().join("vault");
    let day4 = vault4.join("day.md");
    write_file(
        &day4,
        "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) — FOCUS\n",
    );
    let plugin_case = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault4)
        .arg("-f")
        .arg("json")
        .arg("++3")
        .env("BOB_DAY_FILE", &day4)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run plugin parity shift");
    assert_success(&plugin_case);
    let plugin_json: serde_json::Value =
        serde_json::from_str(stdout(&plugin_case).trim()).expect("parity JSON");
    assert_eq!(
        plugin_json["task_line"],
        "- [ ] (**0915-0940** [t:: 25m]) — FOCUS"
    );
}

#[test]
fn capture_pomodoro_shift_batch_atomicity_and_dry_run() {
    // Later items observe earlier staged edits: shifts compose in order.
    let temp = TempDir::new("bob-cli-capture-shift-batch");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let before = "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) — FOCUS\n";
    write_file(&day_file, before);
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:10:00"),
        "++1\n\n++2\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    assert_eq!(json["captures"].as_array().expect("captures").len(), 2);
    assert_eq!(json["captures"][0]["kind"], "pomodoro_shift");
    assert_eq!(json["captures"][0]["pomodoro_shift"]["after_start"], "0905");
    assert_eq!(
        json["captures"][1]["pomodoro_shift"]["before_start"],
        "0905"
    );
    assert_eq!(json["captures"][1]["pomodoro_shift"]["after_start"], "0915");
    assert_eq!(json["captures"][1]["pomodoro_shift"]["after_end"], "0940");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read batch day"),
        "## Pomodoros\n- [ ] (**0915-0940** [t:: 25m]) — FOCUS\n",
    );

    // A shift then a resize composes through the staged file.
    let temp_mix = TempDir::new("bob-cli-capture-shift-mix");
    let vault_mix = temp_mix.path().join("vault");
    let day_mix = vault_mix.join("day.md");
    write_file(&day_mix, before);
    let mixed = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_mix)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_mix)
            .env("BOB_NOW", "2026-07-10 09:10:00"),
        "--2\n\n+\n",
    );
    assert_success(&mixed);
    let mixed_json: serde_json::Value =
        serde_json::from_str(stdout(&mixed).trim()).expect("mixed JSON");
    assert_eq!(mixed_json["captures"][0]["kind"], "pomodoro_shift");
    assert_eq!(mixed_json["captures"][1]["kind"], "pomodoro_adjust");
    assert_eq!(
        mixed_json["captures"][1]["pomodoro_adjust"]["after_duration_minutes"],
        30
    );
    assert_eq!(
        fs::read_to_string(&day_mix).expect("read mixed day"),
        "## Pomodoros\n- [ ] (**0850-0920** [t:: 30m]) — FOCUS\n",
    );

    // A shift plus an ordinary @@-routed task applies in order.
    let temp_ord = TempDir::new("bob-cli-capture-shift-ordinary");
    let vault_ord = temp_ord.path().join("vault");
    let day_ord = vault_ord.join("day.md");
    write_file(&day_ord, before);
    let with_task = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_ord)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_ord)
            .env("BOB_NOW", "2026-07-10 09:10:00"),
        "++1\n\n@@work\nBuy milk\n",
    );
    assert_success(&with_task);
    let task_json: serde_json::Value =
        serde_json::from_str(stdout(&with_task).trim()).expect("task JSON");
    assert_eq!(task_json["captures"][0]["kind"], "pomodoro_shift");
    assert_eq!(task_json["captures"][1]["kind"], "task");
    assert_eq!(task_json["captures"][1]["route"], "work");
    assert!(task_json["captures"][0]["route"].is_null());

    // Dry-run computes the same result without committing.
    let temp2 = TempDir::new("bob-cli-capture-shift-dry-run");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(&day2, before);
    let dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("++3")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run shift dry-run");
    assert_success(&dry);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry-run JSON");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["pomodoro_shift"]["after_start"], "0915");
    assert_eq!(
        fs::read_to_string(&day2).expect("untouched dry-run day"),
        before
    );

    // A later invalid item rolls back the earlier staged shift.
    let temp3 = TempDir::new("bob-cli-capture-shift-rollback");
    let vault3 = temp3.path().join("vault");
    let day3 = vault3.join("day.md");
    write_file(&day3, before);
    let bad = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault3)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day3)
            .env("BOB_NOW", "2026-07-10 09:10:00"),
        "++1\n\n++0\n",
    );
    assert_eq!(bad.status.code(), Some(2), "{}", format_output(&bad));
    assert_eq!(fs::read_to_string(&day3).expect("rolled back day"), before);
}

#[test]
fn capture_pomodoro_shift_rejects_bad_grammar_and_targets() {
    let day_contents =
        "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) — FOCUS\n";
    let run = |text: &str, day: &str| {
        let temp = TempDir::new("bob-cli-capture-shift-reject");
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
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("run rejection");
        let after = fs::read_to_string(&day_file).expect("read after");
        (output, before, after)
    };

    for text in [
        "++0", "--0", "++00", "++3 more", "++3@work", "--2 s:1", "++3++",
    ] {
        let (output, before, after) = run(text, day_contents);
        assert_ne!(output.status.code(), Some(0), "{text} should fail");
        assert_eq!(before, after, "{text} must not write");
    }
    let (shape_out, _, _) = run("++3 more", day_contents);
    let shape_err = String::from_utf8_lossy(&shape_out.stderr).to_string()
        + &String::from_utf8_lossy(&shape_out.stdout).to_string();
    assert!(shape_err.contains("only the operator"), "{shape_err}");
    // Oversized magnitudes fail checked parsing before any write.
    let (output, before, after) =
        run("++99999999999999999999999", day_contents);
    assert_ne!(output.status.code(), Some(0), "overflow should fail");
    assert_eq!(before, after, "overflow must not write");

    // Child lines never become tasks.
    let temp = TempDir::new("bob-cli-capture-shift-child");
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
            .env("BOB_NOW", "2026-07-10 09:10:00"),
        "++3\n- detail\n",
    );
    assert_ne!(child.status.code(), Some(0), "child should fail");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read after child"),
        before
    );

    // Forced destination and clipboard flags are rejected.
    for args in [vec!["--route", "work"], vec!["--clip"]] {
        let temp_f = TempDir::new("bob-cli-capture-shift-forced");
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
        command.arg("++3");
        let forced = command
            .env("BOB_DAY_FILE", &day_f)
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("run forced");
        assert_ne!(
            forced.status.code(),
            Some(0),
            "{args:?} should fail: {}",
            format_output(&forced)
        );
    }

    // Prose that must stay prose: bare runs with text, long/mixed runs,
    // and mid-body tokens never claim an item.
    for text in [
        "- foo", "+ idea", "-- aside", "++ plan", "+++", "---", "+-", "-+3",
        "Plan ++3", "C++",
    ] {
        let temp_p = TempDir::new("bob-cli-capture-shift-prose");
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
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("run prose capture");
        assert_success(&prose);
        let prose_json: serde_json::Value =
            serde_json::from_str(stdout(&prose).trim()).expect("prose JSON");
        assert_eq!(prose_json["kind"], "task", "{text}");
        assert!(
            prose_json.get("pomodoro_shift").is_none(),
            "{text}: {prose_json}"
        );
        assert_eq!(
            fs::read_to_string(&day_p).expect("day untouched by prose"),
            day_contents,
            "{text}"
        );
    }

    // A @@ declaration routes ordinary items but never a shift.
    let temp3 = TempDir::new("bob-cli-capture-shift-global");
    let vault3 = temp3.path().join("vault");
    let day3 = vault3.join("day.md");
    write_file(&day3, day_contents);
    let global = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault3)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day3)
            .env("BOB_NOW", "2026-07-10 09:10:00"),
        "@@work\n++3\n",
    );
    assert_success(&global);
    let global_json: serde_json::Value =
        serde_json::from_str(stdout(&global).trim()).expect("global JSON");
    assert_eq!(global_json["kind"], "pomodoro_shift");
    assert!(global_json["route"].is_null(), "{global_json}");

    // CLI spellings: --3 and --1 carry shift text through the dispatcher.
    for (args, text, delta) in
        [(vec!["--3"], "--3", -15), (vec!["--", "--1"], "--1", -5)]
    {
        let temp_a = TempDir::new("bob-cli-capture-shift-argv");
        let vault_a = temp_a.path().join("vault");
        let day_a = vault_a.join("day.md");
        write_file(&day_a, day_contents);
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault_a)
            .arg("-f")
            .arg("json");
        for arg in &args {
            command.arg(arg);
        }
        let argv = command
            .env("BOB_DAY_FILE", &day_a)
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("run argv spelling");
        assert_success(&argv);
        let argv_json: serde_json::Value =
            serde_json::from_str(stdout(&argv).trim()).expect("argv JSON");
        assert_eq!(argv_json["kind"], "pomodoro_shift", "{args:?}");
        assert_eq!(argv_json["text"], text, "{args:?}");
        assert_eq!(
            argv_json["pomodoro_shift"]["delta_minutes"], delta,
            "{args:?}"
        );
    }
    // A bare -- carries no text.
    {
        let temp_e = TempDir::new("bob-cli-capture-shift-empty");
        let vault_e = temp_e.path().join("vault");
        let day_e = vault_e.join("day.md");
        write_file(&day_e, day_contents);
        let empty = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_e)
            .arg("-f")
            .arg("json")
            .arg("--")
            .env("BOB_DAY_FILE", &day_e)
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("run empty capture");
        assert_ne!(empty.status.code(), Some(0), "bare -- should fail");
    }

    // Bad ledger targets fail without writing.
    for (name, contents) in [
        (
            "missing-section",
            "## Notes\n- [ ] (**0900-0925** [t:: 25m])\n",
        ),
        (
            "no-open",
            "## Pomodoros\n- [x] (**0900-0925** [t:: 25m]) Done\n",
        ),
        (
            "multiple-open",
            "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) One\n- [ ] (**1000-1030** [t:: 30m]) Two\n",
        ),
        ("placeholder", "## Pomodoros\n- [ ] () — PLANNED\n"),
    ] {
        let temp_t = TempDir::new(&format!("bob-cli-capture-shift-{name}"));
        let vault_t = temp_t.path().join("vault");
        let day_t = vault_t.join("day.md");
        write_file(&day_t, contents);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_t)
            .arg("-f")
            .arg("json")
            .arg("++1")
            .env("BOB_DAY_FILE", &day_t)
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("run bad target");
        assert_ne!(output.status.code(), Some(0), "{name} should fail");
        assert_eq!(
            fs::read_to_string(&day_t).expect("target untouched"),
            contents,
            "{name} must not write"
        );
    }
}

#[test]
fn capture_pomodoro_shift_reports_full_block() {
    let temp = TempDir::new("bob-cli-capture-shift-blocks");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260930.md");
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0620-0710** [t:: 50m]) — CLEANUP\n",
            "\t- [[sase#^re-launch-failed]]\n",
            "\t- [[sase#^clean-prompt-history]]\n",
            "\t- [[bob#^decision-web]]\n",
            "- [ ] () — GTD\n",
            "\t- [[#^gtd]]\n",
        ),
    );

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-09-30 07:20:00",
        &["++1"],
    );
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "2026/20260930.md",
                "line": 5,
                "name": "CLEANUP",
                "time_range": "0625-0715",
                "status": "running",
                "created": false,
                "roles": ["shifted"],
                "lines": [
                    {
                        "text": "- [ ] (**0625-0715** [t:: 50m]) — CLEANUP",
                        "depth": 0,
                        "change": "changed",
                        "before": "- [ ] (**0620-0710** [t:: 50m]) — CLEANUP",
                    },
                    {
                        "text": "\t- [[sase#^re-launch-failed]]",
                        "depth": 1,
                        "change": "unchanged",
                    },
                    {
                        "text": "\t- [[sase#^clean-prompt-history]]",
                        "depth": 1,
                        "change": "unchanged",
                    },
                    {
                        "text": "\t- [[bob#^decision-web]]",
                        "depth": 1,
                        "change": "unchanged",
                    },
                ],
            },
        ])
    );
}
