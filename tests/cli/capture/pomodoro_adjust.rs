//! Pomodoro adjust execution.

use crate::support::*;
use std::fs;

#[test]
fn capture_pomodoro_adjust_extends_and_reports_json() {
    let temp = TempDir::new("bob-cli-capture-adjust-extends");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — FOCUS\n",
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
        .arg("+5")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run adjustment capture");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("adjustment JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_adjust");
    assert_eq!(json["text"], "+5");
    assert_eq!(json["placement"], "toggled");
    assert_eq!(json["dry_run"], false);
    let adjust = &json["pomodoro_adjust"];
    assert_eq!(adjust["direction"], "plus");
    assert_eq!(adjust["requested_units"], 5);
    assert_eq!(adjust["requested_minutes"], 25);
    assert_eq!(adjust["delta_minutes"], 25);
    assert_eq!(adjust["before_start"], "0900");
    assert_eq!(adjust["before_end"], "0930");
    assert_eq!(adjust["before_duration_minutes"], 30);
    assert_eq!(adjust["after_start"], "0900");
    assert_eq!(adjust["after_end"], "0955");
    assert_eq!(adjust["after_duration_minutes"], 55);
    assert_eq!(adjust["pomodoro_line"], 3);
    assert_eq!(adjust["pomodoro_name"], "FOCUS");
    assert_eq!(adjust["time_range"], "(**0900-0955** [t:: 55m])");
    assert_eq!(adjust["clamped"], false);
    assert_eq!(json["task_line"], "- [ ] (**0900-0955** [t:: 55m]) — FOCUS");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read adjusted day"),
        concat!(
            "# 2026-07-10\n",
            "## Pomodoros\n",
            "- [ ] (**0900-0955** [t:: 55m]) — FOCUS\n",
            "  - existing context\n",
            "## Later\n",
        )
    );

    // Human output says what changed and where.
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("+1")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:40:00")
        .output()
        .expect("run adjustment human");
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("would adjust")
            && out.contains("FOCUS")
            && out.contains("0900-0955"),
        "{out}"
    );
    assert_stdout_has_no_ansi(&human);
}

#[test]
fn capture_pomodoro_adjust_clamps_and_crosses_midnight() {
    // Subtraction clamps at zero and reports requested vs applied.
    let temp = TempDir::new("bob-cli-capture-adjust-clamp");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0900-0910** [t:: 10m]) — SHORT\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:05:00")
        .output()
        .expect("run clamped adjustment");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("clamp JSON");
    assert_eq!(json["kind"], "pomodoro_adjust");
    assert_eq!(json["pomodoro_adjust"]["requested_minutes"], -10);
    assert_eq!(json["pomodoro_adjust"]["delta_minutes"], -10);
    assert_eq!(json["pomodoro_adjust"]["after_duration_minutes"], 0);
    assert_eq!(json["pomodoro_adjust"]["after_end"], "0900");
    assert_eq!(
        json["pomodoro_adjust"]["time_range"],
        "(**0900-0900** [t:: 0m])"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read clamped day"),
        "## Pomodoros\n- [ ] (**0900-0900** [t:: 0m]) — SHORT\n",
    );

    // Deep clamp: -5 requests -25m against a 10m session.
    let temp2 = TempDir::new("bob-cli-capture-adjust-deep-clamp");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(
        &day2,
        "## Pomodoros\n- [ ] (**0900-0910** [t:: 10m]) — SHORT\n",
    );
    let output2 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("-5")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:05:00")
        .output()
        .expect("run deep clamp");
    assert_success(&output2);
    let json2: serde_json::Value =
        serde_json::from_str(stdout(&output2).trim()).expect("deep clamp JSON");
    assert_eq!(json2["pomodoro_adjust"]["requested_minutes"], -25);
    assert_eq!(json2["pomodoro_adjust"]["delta_minutes"], -10);
    assert_eq!(json2["pomodoro_adjust"]["clamped"], true);
    assert_eq!(json2["pomodoro_adjust"]["after_duration_minutes"], 0);

    // Midnight crossing keeps the start fixed and wraps the end.
    let temp3 = TempDir::new("bob-cli-capture-adjust-midnight");
    let vault3 = temp3.path().join("vault");
    let day3 = vault3.join("day.md");
    write_file(
        &day3,
        "## Pomodoros\n- [ ] (**2330-2355** [t:: 25m]) — LATE\n",
    );
    let output3 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault3)
        .arg("-f")
        .arg("json")
        .arg("+2")
        .env("BOB_DAY_FILE", &day3)
        .env("BOB_NOW", "2026-07-10 23:40:00")
        .output()
        .expect("run midnight adjustment");
    assert_success(&output3);
    let json3: serde_json::Value =
        serde_json::from_str(stdout(&output3).trim()).expect("midnight JSON");
    assert_eq!(json3["pomodoro_adjust"]["after_start"], "2330");
    assert_eq!(json3["pomodoro_adjust"]["after_end"], "0005");
    assert_eq!(json3["pomodoro_adjust"]["after_duration_minutes"], 35);
    assert_eq!(
        fs::read_to_string(&day3).expect("read midnight day"),
        "## Pomodoros\n- [ ] (**2330-0005** [t:: 35m]) — LATE\n",
    );
}

#[test]
fn capture_pomodoro_adjust_preserves_metadata_crlf_and_fallbacks() {
    // Extra parenthetical metadata and CRLF newlines survive a rewrite.
    let temp = TempDir::new("bob-cli-capture-adjust-metadata");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(
        &day_file,
        "- [ ] (**0900-0930** [t:: 30m] focus) — DEEP\r\n  - child link\r\n",
    );
    // Prepend the section with LF so the file mixes endings; the edit must
    // keep the target line's CRLF.
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
        .arg(" +1 ")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run metadata adjustment");
    assert_success(&output);
    let after = fs::read(&day_file).expect("read metadata day");
    let after_text = String::from_utf8(after).expect("utf8 day");
    assert!(
        after_text.contains(
            "(**0900-0935** [t:: 35m] focus) — DEEP\r\n  - child link\r\n"
        ),
        "{after_text}"
    );

    // Legacy stopwatch duration is honored like the keymap.
    let temp2 = TempDir::new("bob-cli-capture-adjust-legacy");
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
        .arg("+1")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run legacy adjustment");
    assert_success(&output2);
    let json2: serde_json::Value =
        serde_json::from_str(stdout(&output2).trim()).expect("legacy JSON");
    assert_eq!(json2["pomodoro_adjust"]["before_duration_minutes"], 20);
    assert_eq!(json2["pomodoro_adjust"]["after_duration_minutes"], 25);
    assert_eq!(
        fs::read_to_string(&day2).expect("read legacy day"),
        "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) — LEGACY\n",
    );

    // No duration metadata falls back to the displayed range.
    let temp3 = TempDir::new("bob-cli-capture-adjust-range-fallback");
    let vault3 = temp3.path().join("vault");
    let day3 = vault3.join("day.md");
    write_file(&day3, "## Pomodoros\n- [ ] (**0900-0930**) — PLAIN\n");
    let output3 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault3)
        .arg("-f")
        .arg("json")
        .arg("+1")
        .env("BOB_DAY_FILE", &day3)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run fallback adjustment");
    assert_success(&output3);
    let json3: serde_json::Value =
        serde_json::from_str(stdout(&output3).trim()).expect("fallback JSON");
    assert_eq!(json3["pomodoro_adjust"]["before_duration_minutes"], 30);
    assert_eq!(json3["pomodoro_adjust"]["after_duration_minutes"], 35);
}

#[test]
fn capture_pomodoro_adjust_batch_atomicity_and_dry_run() {
    // Later items observe earlier staged edits: two adjustments accumulate.
    let temp = TempDir::new("bob-cli-capture-adjust-batch");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let before = "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) — FOCUS\n";
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
        "+1\n\n+2\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("batch JSON");
    assert_eq!(json["captures"].as_array().expect("captures").len(), 2);
    assert_eq!(
        json["captures"][0]["pomodoro_adjust"]["after_duration_minutes"],
        35
    );
    assert_eq!(
        json["captures"][1]["pomodoro_adjust"]["after_duration_minutes"],
        45
    );
    assert_eq!(
        json["captures"][1]["pomodoro_adjust"]["before_duration_minutes"],
        35
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read batch day"),
        "## Pomodoros\n- [ ] (**0900-0945** [t:: 45m]) — FOCUS\n",
    );

    // Dry-run computes the same result without committing.
    let temp2 = TempDir::new("bob-cli-capture-adjust-dry-run");
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
        .arg("+5")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run adjust dry-run");
    assert_success(&dry);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry-run JSON");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["pomodoro_adjust"]["after_duration_minutes"], 55);
    assert_eq!(
        fs::read_to_string(&day2).expect("untouched dry-run day"),
        before
    );

    // A later invalid item rolls back the earlier staged adjustment.
    let temp3 = TempDir::new("bob-cli-capture-adjust-rollback");
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
        "+1\n\n+0\n",
    );
    assert_eq!(bad.status.code(), Some(2), "{}", format_output(&bad));
    assert_eq!(fs::read_to_string(&day3).expect("rolled back day"), before);
}

#[test]
fn capture_pomodoro_adjust_rejects_bad_grammar_and_targets() {
    let day_contents =
        "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) — FOCUS\n";
    let run = |text: &str, day: &str| {
        let temp = TempDir::new("bob-cli-capture-adjust-reject");
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
            .arg(text)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("run rejection");
        let after = fs::read_to_string(&day_file).expect("read after");
        (output, before, after)
    };

    for text in ["+0", "-0", "+5 foo", "+5 s:1", "+5 p:1", "+5 %"] {
        let (output, before, after) = run(text, day_contents);
        assert_ne!(output.status.code(), Some(0), "{text} should fail");
        assert_eq!(before, after, "{text} must not write");
    }
    // A bare sign is one unit, never incomplete.
    for text in ["+", "-"] {
        let (output, before, after) = run(text, day_contents);
        assert_success(&output);
        assert_ne!(before, after, "{text} should write");
    }
    // Oversized magnitudes fail checked parsing before any write.
    let (output, before, after) = run("+99999999999999999999999", day_contents);
    assert_ne!(output.status.code(), Some(0), "overflow should fail");
    assert_eq!(before, after, "overflow must not write");

    // Child lines never become tasks.
    let temp = TempDir::new("bob-cli-capture-adjust-child");
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
        "+5\n- detail\n",
    );
    assert_ne!(child.status.code(), Some(0), "child should fail");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read after child"),
        before
    );

    // Forced destination and clipboard flags are rejected.
    let forced = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(temp.path().join("vault"))
        .arg("--route")
        .arg("work")
        .arg("+5")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run forced route");
    assert_ne!(forced.status.code(), Some(0), "forced route should fail");

    // Ordinary prose containing a count stays a task.
    let temp2 = TempDir::new("bob-cli-capture-adjust-prose");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(&day2, day_contents);
    fs::create_dir_all(&vault2).expect("create vault");
    let prose = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("Plan +5")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:10:00")
        .output()
        .expect("run prose capture");
    assert_success(&prose);
    let prose_json: serde_json::Value =
        serde_json::from_str(stdout(&prose).trim()).expect("prose JSON");
    assert_eq!(prose_json["kind"], "task");
    assert_eq!(
        fs::read_to_string(&day2).expect("day untouched by prose"),
        day_contents
    );

    // A @@ declaration routes ordinary items but never an adjustment.
    let temp3 = TempDir::new("bob-cli-capture-adjust-global");
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
        "@@work\n+5\n",
    );
    assert_success(&global);
    let global_json: serde_json::Value =
        serde_json::from_str(stdout(&global).trim()).expect("global JSON");
    assert_eq!(global_json["kind"], "pomodoro_adjust");
    assert!(global_json["route"].is_null(), "{global_json}");

    // Bad ledger targets fail without writing.
    for (name, contents) in [
        (
            "missing-section",
            "## Notes\n- [ ] (**0900-0930** [t:: 30m])\n",
        ),
        (
            "no-open",
            "## Pomodoros\n- [x] (**0900-0930** [t:: 30m]) Done\n",
        ),
        (
            "multiple-open",
            "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) One\n- [ ] (**1000-1030** [t:: 30m]) Two\n",
        ),
        ("placeholder", "## Pomodoros\n- [ ] () — PLANNED\n"),
    ] {
        let temp = TempDir::new(&format!("bob-cli-capture-adjust-{name}"));
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_file(&day_file, contents);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("+1")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:10:00")
            .output()
            .expect("run bad target");
        assert_ne!(output.status.code(), Some(0), "{name} should fail");
        assert_eq!(
            fs::read_to_string(&day_file).expect("target untouched"),
            contents,
            "{name} must not write"
        );
    }
}

#[test]
fn capture_pomodoro_adjust_reports_full_block() {
    let temp = TempDir::new("bob-cli-capture-adjust-blocks");
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
        &["+5"],
    );
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "2026/20260930.md",
                "line": 5,
                "name": "CLEANUP",
                "time_range": "0620-0735",
                "status": "running",
                "created": false,
                "roles": ["adjusted"],
                "lines": [
                    {
                        "text": "- [ ] (**0620-0735** [t:: 75m]) — CLEANUP",
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
