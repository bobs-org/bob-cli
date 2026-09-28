//! Pomodoro close execution and close helpers.

use crate::support::*;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

pub(crate) fn close_worked_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t\t- Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- Wrote the plan\n",
            "\t- [[bob#^web-capture]]#\n",
            "\t- ~~[[sase#^axe-restart]]~~\n",
            "\t\t- Restarted axe\n",
            "\t- quick note\n",
            "- [ ] () — SASE\n",
            "\t- [[sase#^recovery-panel]]\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
            "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
        ),
    );
    write_file(
        &vault.join("sase.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [x] #task Restart axe [created::2026-09-27] [completion:: 2026-09-28] ^axe-restart\n",
            "  - \u{1F6E0}\u{FE0F} **WORK LOG**\n",
            "    - _2026-09-27_ — Diagnosed the hang\n",
            "- [ ] #task Recovery panel [created::2026-09-25] ^recovery-panel\n",
        ),
    );
    (temp, vault, day_file)
}

pub(crate) fn run_close_json(
    vault: &Path,
    day_file: &Path,
    now: &str,
    args: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", now)
        .output()
        .expect("run close capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("close json")
}

pub(crate) fn run_close_expect_error(
    vault: &Path,
    day_file: &Path,
    now: &str,
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
        .env("BOB_NOW", now)
        .output()
        .expect("run close capture");
    assert!(
        !output.status.success(),
        "expected failure:\n{}",
        format_output(&output)
    );
    stdout(&output).trim().to_string()
}

#[test]
fn capture_pomodoro_close_worked_example() {
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-worked");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_close");
    assert_eq!(json["routed"], false);
    assert_eq!(json["route_label"], "");
    assert_eq!(json["relative_target"], "2026/20260928.md");
    assert_eq!(json["text"], "=x");
    assert_eq!(json["placement"], "closed");
    assert_eq!(
        json["task_line"],
        "- [x] (**0920-0940** [t:: 20m]) — CAPTURE"
    );
    assert_eq!(json["pomodoro_name"], "CAPTURE");
    assert_eq!(json["created"], "2026-09-28");
    assert!(json["scheduled"].is_null());
    assert!(json["route"].is_null());
    let close = &json["pomodoro_close"];
    // Full `pomodoro_close` object, byte for byte.
    assert_eq!(
        close,
        &serde_json::json!({
            "raw": "=x",
            "pomodoro_line": 5,
            "pomodoro_name": "CAPTURE",
            "day_relative": "2026/20260928.md",
            "entry_line": "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
            "planned": {
                "start": "0920", "end": "0950",
                "duration_minutes": 30, "time_range": "0920-0950"
            },
            "closed": {
                "start": "0920", "end": "0940",
                "duration_minutes": 20, "time_range": "0920-0940"
            },
            "closed_at": "0937",
            "remaining_minutes": 13,
            "decremented_minutes": 10,
            "tasks": [
                {
                    "role": "worked",
                    "block_link": "[[bob#^capture-stop]]",
                    "ledger_line": 6,
                    "resolved": true,
                    "relative_target": "bob.md",
                    "block_id": "capture-stop",
                    "text": "Add support for `=x` syntax!",
                    "previous_status_symbol": "*",
                    "previous_status_name": "Next",
                    "status_symbol": "/",
                    "status_name": "In Progress",
                    "status_changed": true,
                    "carried": true,
                    "work_log": [
                        "*2026-09-28* — Designed the `=x` grammar",
                        "*2026-09-28* — Wrote the plan"
                    ],
                    "work_log_created": true,
                    "warning": null
                },
                {
                    "role": "deferred",
                    "block_link": "[[bob#^web-capture]]",
                    "ledger_line": 10,
                    "resolved": true,
                    "relative_target": "bob.md",
                    "block_id": "web-capture",
                    "text": "Add capture support for web URLs!",
                    "previous_status_symbol": "*",
                    "previous_status_name": "Next",
                    "status_symbol": "*",
                    "status_name": "Next",
                    "status_changed": false,
                    "carried": true,
                    "work_log": [],
                    "work_log_created": false,
                    "warning": null
                },
                {
                    "role": "struck",
                    "block_link": "[[sase#^axe-restart]]",
                    "ledger_line": 11,
                    "resolved": true,
                    "relative_target": "sase.md",
                    "block_id": "axe-restart",
                    "text": "Restart axe",
                    "previous_status_symbol": "x",
                    "previous_status_name": "Done",
                    "status_symbol": "x",
                    "status_name": "Done",
                    "status_changed": false,
                    "carried": false,
                    "work_log": ["*2026-09-28* — Restarted axe"],
                    "work_log_created": true,
                    "warning": null
                }
            ],
            "carried": [
                {"kind": "worked", "text": "[[bob#^capture-stop]]"},
                {"kind": "deferred", "text": "[[bob#^web-capture]]"}
            ],
            "notes": ["quick note"],
            "next_pomodoro": {
                "line": 13, "name": "CAPTURE",
                "time_range": null, "created": true
            }
        })
    );
    // Ledger post-image, byte for byte.
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "\t\t- Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- Wrote the plan\n",
            "\t- ~~[[sase#^axe-restart]]~~\n",
            "\t\t- Restarted axe\n",
            "\t- quick note\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t- [[bob#^web-capture]]\n",
            "- [ ] () — SASE\n",
            "\t- [[sase#^recovery-panel]]\n",
        )
    );
    // Task notes, byte for byte.
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert_eq!(
        bob_after,
        concat!(
            "## Tasks\n",
            "\n",
            "- [/] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
            "\t- \u{1F6E0}\u{FE0F} **WORK LOG**\n",
            "\t\t- *2026-09-28* — Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- *2026-09-28* — Wrote the plan\n",
            "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
        )
    );
    let sase_after = fs::read_to_string(vault.join("sase.md")).expect("sase");
    assert_eq!(
        sase_after,
        concat!(
            "## Tasks\n",
            "\n",
            "- [x] #task Restart axe [created::2026-09-27] [completion:: 2026-09-28] ^axe-restart\n",
            "  - \u{1F6E0}\u{FE0F} **WORK LOG**\n",
            "    - *2026-09-28* — Restarted axe\n",
            "    - _2026-09-27_ — Diagnosed the hang\n",
            "- [ ] #task Recovery panel [created::2026-09-25] ^recovery-panel\n",
        )
    );

    // 09:49 leaves the range untouched (less than 5 minutes remain).
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-no-decrement");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:49:00", &["=x"]);
    assert_eq!(json["pomodoro_close"]["closed"]["end"], "0950");
    assert_eq!(json["pomodoro_close"]["decremented_minutes"], 0);
    assert_eq!(
        json["task_line"],
        "- [x] (**0920-0950** [t:: 30m]) — CAPTURE"
    );
    assert_eq!(json["pomodoro_close"]["remaining_minutes"], 1);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "- [x] (**0920-0950** [t:: 30m]) — CAPTURE\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "\t\t- Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- Wrote the plan\n",
            "\t- ~~[[sase#^axe-restart]]~~\n",
            "\t\t- Restarted axe\n",
            "\t- quick note\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t- [[bob#^web-capture]]\n",
            "- [ ] () — SASE\n",
            "\t- [[sase#^recovery-panel]]\n",
        )
    );

    // Human output names the session, the range change, and the next entry.
    // Dry-run first so the human close below still has a running session.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-human");
    let dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("dry human");
    assert_success(&dry);
    assert!(stdout(&dry).contains("would close"));
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("human close");
    assert_success(&output);
    let out = stdout(&output);
    assert_text_order(
        &out,
        &[
            "closed CAPTURE 0920-0950",
            "0920-0940",
            "2026/20260928.md line 5",
            "next: CAPTURE",
        ],
    );
    // Task rows print the locator once with the Work Log count.
    assert!(out.contains("bob.md ^capture-stop +2 Work Log"), "{out}");
    assert!(out.contains("bob.md ^web-capture"), "{out}");
    assert!(out.contains("sase.md ^axe-restart +1 Work Log"), "{out}");
    assert!(!out.contains("note ^capture-stop"), "{out}");
    assert_stdout_has_no_ansi(&output);

    // Link forms name the day file in the header, not the route note.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-human-link");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("^bob:ready=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("human link close");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("2026/20260928.md line 5"), "{out}");
    assert!(!out.contains("bob.md line 5"), "{out}");
    assert!(out.contains("bob.md ^ready"), "{out}");
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_pomodoro_close_diagnostics() {
    // Missing day file.
    let temp = TempDir::new("bob-cli-close-missing-day");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    write_toggle_task_settings(&vault);
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x"],
    );
    assert!(
        error.contains("today's daily note `2026/20260928.md` does not exist"),
        "{error}"
    );

    // Missing section.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-no-section");
    write_file(&day_file, "# 2026-09-28\n");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x"],
    );
    assert!(error.contains("has no Pomodoros section"), "{error}");

    // None running with a next placeholder.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-none-running");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — CAPTURE\n  - [[bob#^capture-stop]]\n",
    );
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x"],
    );
    assert!(error.contains("has no open timed entry"), "{error}");
    assert!(error.contains("next up is CAPTURE at line"), "{error}");
    assert!(error.contains("(start it with `=`)"), "{error}");
    // Link forms point at the start syntax.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:capture-stop=x"],
    );
    assert!(
        error.contains("to start a session with this task instead"),
        "{error}"
    );

    // Multiple open timed entries.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-multiple");
    let contents = fs::read_to_string(&day_file).expect("read");
    write_file(
        &day_file,
        &contents.replace(
            "- [ ] () — SASE",
            "- [ ] (**1000-1030** [t:: 30m]) — SASE",
        ),
    );
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x"],
    );
    assert!(error.contains("multiple open timed Pomodoros"), "{error}");

    // Near misses and conflicts.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-near-miss");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x more"],
    );
    assert!(error.contains("must be the whole capture item"), "{error}");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x")
        .arg("s:2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("close with child");
    assert!(!output.status.success());
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:ready#capture=x"],
    );
    assert!(error.contains("remove `#capture`"), "{error}");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["Text @bob:draft=x s:2"],
    );
    assert!(error.contains("cannot be combined with `s:<N>`"), "{error}");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("bob")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("forced");
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("cannot be combined with --route"),
        "{}",
        stdout(&output)
    );
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["Do @bob:new+=x"],
    );
    assert!(error.contains("not project-note"), "{error}");

    // @@ never turns a close into a task.
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00"),
        "@@bob\n=x\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["kind"], "pomodoro_close");

    // Prose lookalikes stay prose; =X is accepted. `=3` is a start, not
    // a task, and is covered by the start tests below.
    for (input, kind) in [("=xx", "task"), ("Plan =x", "task")] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--dry-run")
            .arg("--")
            .arg(input)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00")
            .output()
            .expect("prose");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        assert_eq!(json["kind"], kind, "{input}");
    }
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-upper");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=X"]);
    assert_eq!(json["kind"], "pomodoro_close");
    assert_eq!(json["pomodoro_close"]["raw"], "=X");

    // None running without a placeholder names no next session.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-none-at-all");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
    );
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x"],
    );
    assert!(error.contains("has no open timed entry"), "{error}");
    assert!(!error.contains("next up"), "{error}");

    // A second `=x` names the new placeholder.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-second");
    run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x"],
    );
    assert!(error.contains("next up is CAPTURE at line 13"), "{error}");

    // Link forms use the no-running contract: link keeps the next-up tail
    // and the start hint, missing day files name the note, missing
    // sections reuse the whole-item phrasing, and multiple entries report
    // pre-image lines.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-link-missing-day");
    fs::remove_file(&day_file).expect("remove day");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:ready=x"],
    );
    assert!(
        error.contains("today's daily note `2026/20260928.md` does not exist"),
        "{error}"
    );
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-link-no-section");
    write_file(&day_file, "# 2026-09-28\n");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:ready=x"],
    );
    assert!(error.contains("has no Pomodoros section"), "{error}");
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-link-multiple");
    let contents = fs::read_to_string(&day_file).expect("read");
    write_file(
        &day_file,
        &contents.replace(
            "- [ ] () — SASE",
            "- [ ] (**1000-1030** [t:: 30m]) — SASE",
        ),
    );
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:ready=x"],
    );
    assert!(error.contains("multiple open timed Pomodoros"), "{error}");
    assert!(error.contains("SASE at line 14"), "{error}");

    // Body-bearing hints name the item's own spelling.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-body-hint");
    write_file(&day_file, "## Pomodoros\n- [ ] () — CAPTURE\n");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["Draft docs @bob:draft-docs=x"],
    );
    assert!(error.contains("next up is CAPTURE at line 2"), "{error}");
    assert!(
        error.contains("use `Draft docs @bob:draft-docs=`"),
        "{error}"
    );

    // An unparseable range still closes without decrementing, with a
    // warning, leaving the range bytes untouched.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-bad-range");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] ( **0920-0950** [t:: 30m]) — BROKEN\n",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(json["kind"], "pomodoro_close");
    assert_eq!(json["pomodoro_close"]["decremented_minutes"], 0);
    assert_eq!(
        json["task_line"],
        "- [x] ( **0920-0950** [t:: 30m]) — BROKEN"
    );
    assert!(
        json["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .is_some_and(|warning| warning
                    .contains("time range was left as written"))),
        "{}",
        json["warnings"]
    );

    // `=x p:1` and strict `=` fail without writing.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-prio");
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x p:1"],
    );
    assert!(error.contains("=x"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    // A bare `=` is a start, not an incomplete close: with CAPTURE
    // running it names the running session and teaches the switch idiom.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["="],
    );
    assert!(error.contains("is still running at line 5"), "{error}");
    assert!(error.contains("then `=` to switch sessions"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // A real `=x` item with an authored child line fails the same way.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-child-line");
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00"),
        "=x\n- detail\n",
    );
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("must be the whole capture item"),
        "{}",
        format_output(&output)
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Forced flags beyond `--route` fail on whole-item closes.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-forced");
    let clipboard = _temp.path().join("clipboard");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'forced text'\n");
    for args in [
        vec!["--route", "bob"],
        vec!["--route", "bob", "--section", "Tasks"],
        vec!["--route", "bob", "--task", "ready"],
        vec!["--route", "bob", "--task-ref", "1:deadbeef"],
        vec!["--route", "bob", "--task", "ready", "--task-section", "FOO"],
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .args(&args)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg("=x")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00")
            .output()
            .expect("forced close");
        assert!(!output.status.success());
        assert!(
            stdout(&output).contains("cannot be combined with"),
            "{args:?}: {}",
            stdout(&output)
        );
    }
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--clip")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .output()
        .expect("clip close");
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("cannot be combined with --clip"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn capture_pomodoro_close_links_batches_and_files() {
    // Solo link: `^bob:ready=x` appends after `quick note`, closes as
    // `linked` with the non-close placement and `=x` raw, and carries
    // second (capture-stop, ready, then web-capture).
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-linked");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:ready=x"],
    );
    assert_eq!(json["kind"], "pomodoro_link");
    assert_eq!(json["placement"], "linked");
    assert_eq!(json["pomodoro_link_action"], "linked");
    assert_eq!(json["previous_status_symbol"], " ");
    assert_eq!(json["status_symbol"], "/");
    assert_eq!(json["pomodoro_close"]["raw"], "=x");
    assert_eq!(
        json["pomodoro_close"]["carried"],
        serde_json::json!([
            {"kind": "worked", "text": "[[bob#^capture-stop]]"},
            {"kind": "worked", "text": "[[bob#^ready]]"},
            {"kind": "deferred", "text": "[[bob#^web-capture]]"}
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "\t\t- Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- Wrote the plan\n",
            "\t- ~~[[sase#^axe-restart]]~~\n",
            "\t\t- Restarted axe\n",
            "\t- quick note\n",
            "\t- \u{1F345} [[bob#^ready]]\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t- [[bob#^ready]]\n",
            "\t- [[bob#^web-capture]]\n",
            "- [ ] () — SASE\n",
            "\t- [[sase#^recovery-panel]]\n",
        )
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert!(bob_after.contains(
        "- [/] #task Plain ready task [created::2026-09-20] ^ready\n"
    ));

    // Moved: the subtree leaves SASE for CAPTURE with the pre-image
    // source line, and the task ends In Progress.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-moved");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^sase:recovery-panel=x"],
    );
    assert_eq!(json["kind"], "pomodoro_link");
    assert_eq!(json["placement"], "linked");
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["pomodoro_link_source"]["line"], 14);
    assert_eq!(json["pomodoro_link_source"]["name"], "SASE");
    assert_eq!(json["pomodoro_close"]["raw"], "=x");
    assert_eq!(json["status_symbol"], "/");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "\t\t- Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- Wrote the plan\n",
            "\t- ~~[[sase#^axe-restart]]~~\n",
            "\t\t- Restarted axe\n",
            "\t- quick note\n",
            "\t- \u{1F345} [[sase#^recovery-panel]]\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t- [[sase#^recovery-panel]]\n",
            "\t- [[bob#^web-capture]]\n",
            "- [ ] () — SASE\n",
        )
    );
    let sase_after = fs::read_to_string(vault.join("sase.md")).expect("sase");
    assert!(sase_after.contains(
        "- [/] #task Recovery panel [created::2026-09-25] ^recovery-panel\n"
    ));

    // Already current: file post-images are byte-identical to plain `=x`.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-current");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:capture-stop=x"],
    );
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert_eq!(json["placement"], "linked");
    assert_eq!(json["pomodoro_close"]["raw"], "=x");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    let sase_after = fs::read_to_string(vault.join("sase.md")).expect("sase");
    let (_temp, plain_vault, plain_day) =
        close_worked_vault("bob-cli-close-current-plain");
    run_close_json(&plain_vault, &plain_day, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(
        day_after,
        fs::read_to_string(&plain_day).expect("plain day")
    );
    assert_eq!(
        bob_after,
        fs::read_to_string(plain_vault.join("bob.md")).expect("plain bob")
    );
    assert_eq!(
        sase_after,
        fs::read_to_string(plain_vault.join("sase.md")).expect("plain sase")
    );

    // New task while closing keeps the new-task placement.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-new-task");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["Draft docs @bob:draft-docs=x"],
    );
    assert_eq!(json["kind"], "pomodoro_task");
    assert_eq!(json["placement"], "inserted");
    assert_eq!(json["status_symbol"], "/");
    assert_eq!(json["pomodoro_close"]["raw"], "=x");
    assert_eq!(json["pomodoro_link_action"], "linked");
    let created = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert_eq!(
        created,
        concat!(
            "## Tasks\n",
            "\n",
            "- [/] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
            "\t- \u{1F6E0}\u{FE0F} **WORK LOG**\n",
            "\t\t- *2026-09-28* — Designed the `=x` grammar\n",
            "\t\t\t- chose `x` for done\n",
            "\t\t- *2026-09-28* — Wrote the plan\n",
            "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
            "- [/] #task Draft docs [created::2026-09-28] ^draft-docs\n",
        )
    );

    // Solo-link closes across statuses: Ready, Blocked, Next, and In
    // Progress all end In Progress in both JSON and file.
    for (name, line, previous) in [
        ("ready", "- [ ] #task Ready ^ready\n", " "),
        ("blocked", "- [?] #task Blocked ^blocked\n", "?"),
        ("next", "- [*] #task Next ^next\n", "*"),
        ("progress", "- [/] #task Progress ^progress\n", "/"),
    ] {
        let (_temp, vault, day_file) =
            close_worked_vault(&format!("bob-cli-close-transition-{name}"));
        write_file(&vault.join("bob.md"), &format!("## Tasks\n\n{line}"));
        let json = run_close_json(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[&format!("^bob:{name}=x")],
        );
        assert_eq!(json["kind"], "pomodoro_link", "{name}");
        assert_eq!(json["previous_status_symbol"], previous, "{name}");
        assert_eq!(json["status_symbol"], "/", "{name}");
        let task_line = json["task_line"].as_str().expect("task line");
        assert!(task_line.starts_with("- [/] #task"), "{name}: {task_line}");
        assert!(task_line.contains(&format!("^{name}")), "{task_line}");
        let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
        assert!(bob_after.contains(task_line), "{name}: {bob_after}");
    }

    // Done tasks fail; missing IDs hint at creation.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-gates");
    write_file(
        &vault.join("bob.md"),
        concat!("## Tasks\n", "\n", "- [x] #task Done ^done\n",),
    );
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:done=x"],
    );
    assert!(error.contains("only Ready, Blocked, Next"), "{error}");
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-missing");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:nope=x"],
    );
    assert!(error.contains("no task with block ID ^nope"), "{error}");

    // Batches compose; failures roll back.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-batch-adjust");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-2\n\n=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("batch");
    assert_success(&output);
    let before = fs::read_to_string(&day_file).expect("read");
    assert!(before.contains("(**0920-0940** [t:: 20m])"));

    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-batch-switch");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("=x\n\n^sase:recovery-panel=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("switch");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_link");

    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-batch-rollback");
    let before = fs::read_to_string(&day_file).expect("read");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("=x\n\n=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("rollback");
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Dry-run prints identical JSON apart from `dry_run`, and writes
    // nothing.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-dry");
    let before_day = fs::read_to_string(&day_file).expect("read");
    let before_bob = fs::read_to_string(vault.join("bob.md")).expect("read");
    let before_sase = fs::read_to_string(vault.join("sase.md")).expect("read");
    let dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("dry");
    assert_success(&dry);
    let mut dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("json");
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before_day);
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("read"),
        before_bob
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read"),
        before_sase
    );
    let mut real_json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    dry_json["dry_run"] = serde_json::json!(false);
    real_json["dry_run"] = serde_json::json!(false);
    assert_eq!(dry_json, real_json);

    // A relative `-b` applies every task effect instead of skipping them
    // with unreadable-target warnings, for both `=x` and `^bob:ready=x`
    // (where the link step and the close both edit `bob.md`).
    for (name, args) in [
        ("bob-cli-close-relative-b", "=x"),
        ("bob-cli-close-relative-link", "^bob:ready=x"),
    ] {
        let (_temp, vault, day_file) = close_worked_vault(name);
        let parent = vault.parent().expect("vault parent").to_path_buf();
        let vault_name = vault
            .file_name()
            .expect("vault name")
            .to_string_lossy()
            .into_owned();
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_name)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(args)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00")
            .current_dir(&parent)
            .output()
            .expect("relative close");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        assert!(json["ok"].as_bool().unwrap_or(false), "{args}");
        assert!(
            json["warnings"].as_array().is_none_or(Vec::is_empty),
            "{args}: {:?}",
            json["warnings"]
        );
        let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
        assert!(bob_after.contains(
            "- [/] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop"
        ), "{args}");
        if args != "=x" {
            assert!(bob_after.contains(
                "- [/] #task Plain ready task [created::2026-09-20] ^ready"
            ));
        }
    }

    // CRLF and missing final newline survive the close.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-crlf");
    let contents = fs::read_to_string(&day_file).expect("read");
    fs::write(&day_file, contents.replace('\n', "\r\n")).expect("crlf");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(json["kind"], "pomodoro_close");
    let raw = fs::read(&day_file).expect("read");
    assert!(raw.windows(2).any(|pair| pair == b"\r\n"));

    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-no-newline");
    let contents = fs::read_to_string(&day_file).expect("read");
    let trimmed = contents.trim_end_matches('\n').to_string();
    fs::write(&day_file, &trimmed).expect("trim");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(json["kind"], "pomodoro_close");
    let raw = fs::read(&day_file).expect("read");
    assert!(!raw.ends_with(b"\n"));

    // CRLF and a missing final newline survive in a task note too, with
    // task effects still applied.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-note-crlf");
    let note = vault.join("bob.md");
    let contents = fs::read_to_string(&note).expect("read");
    fs::write(&note, contents.replace('\n', "\r\n")).expect("crlf");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(json["kind"], "pomodoro_close");
    let raw = fs::read(&note).expect("read");
    assert!(raw.windows(2).any(|pair| pair == b"\r\n"));
    assert!(String::from_utf8_lossy(&raw).contains(
        "- [/] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop"
    ));

    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-note-no-newline");
    let note = vault.join("bob.md");
    let contents = fs::read_to_string(&note).expect("read");
    let trimmed = contents.trim_end_matches('\n').to_string();
    fs::write(&note, &trimmed).expect("trim");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(json["kind"], "pomodoro_close");
    let raw = fs::read(&note).expect("read");
    assert!(!raw.ends_with(b"\n"));
    assert!(String::from_utf8_lossy(&raw).contains(
        "- [/] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop"
    ));
}
