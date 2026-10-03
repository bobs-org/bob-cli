//! `bob capture` freshness stamps: the existing tasks it rewrites.

use super::pomodoro_close::{close_worked_vault, run_close_json};
use crate::support::*;
use std::fs;

fn toggle_vault(
    name: &str,
    target_body: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let target = vault.join("cash.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&target, target_body);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0900-0930**) — CURRENT\n",
            "  - context\n",
            "- [ ] () — LATER\n",
        ),
    );
    (temp, vault, day_file)
}

#[test]
fn capture_link_stamps_ready_and_blocked_to_next() {
    for (symbol, name) in [(" ", "ready"), ("?", "blocked")] {
        let body = format!("- [{symbol}] #task Work ^t\n");
        let (_temp, vault, day_file) =
            toggle_vault(&format!("bob-cli-fresh-link-{name}"), &body);
        let target = vault.join("cash.md");
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg("@cash+t!")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 10:00:00")
            .output()
            .expect("link stamps");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        assert_eq!(json["toggle_direction"], "link", "{name}");
        assert_eq!(json["status_symbol"], "*", "{name}");
        assert_eq!(
            fs::read_to_string(&target).expect("target"),
            "- [*] #task Work [fresh:: 2026-07-10] ^t\n",
            "{name}"
        );
        assert!(
            !json["task_line"]
                .as_str()
                .unwrap_or_default()
                .contains("[fresh:: 2026-07-10] [fresh::"),
            "{json}"
        );
    }
}

#[test]
fn capture_link_clears_keep_streak() {
    // A capture rewrite stamps through the generic default, which
    // clears `keeps`; no keep metadata leaks into the rewritten line
    // or its clean description.
    let body = "- [ ] #task Work [fresh:: 2026-07-01] [keeps:: 2] ^t\n";
    let (_temp, vault, day_file) =
        toggle_vault("bob-cli-fresh-link-keeps", body);
    let target = vault.join("cash.md");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+t!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("link clears keeps");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&target).expect("target"),
        "- [*] #task Work [fresh:: 2026-07-10] ^t\n",
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert!(
        !json["task_line"]
            .as_str()
            .unwrap_or_default()
            .contains("[keeps::"),
        "{json}"
    );
}

#[test]
fn capture_link_leaves_next_untouched_without_future_schedule() {
    for (symbol, arg) in [("*", "@cash+t!"), ("/", "@cash+t!")] {
        let body = format!("- [{symbol}] #task Work ^t\n");
        let (_temp, vault, day_file) =
            toggle_vault("bob-cli-fresh-link-noop", &body);
        let target = vault.join("cash.md");
        let before = fs::read_to_string(&target).expect("before");
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(arg)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 10:00:00")
            .output()
            .expect("link no-op");
        assert_success(&output);
        assert_eq!(
            fs::read_to_string(&target).expect("target"),
            before,
            "{symbol} without a future schedule must not rewrite the note"
        );
        assert!(
            !fs::read_to_string(&target)
                .expect("target")
                .contains("[fresh::"),
            "{symbol} must not stamp"
        );
    }
}

#[test]
fn capture_link_stamps_next_when_it_retires_future_schedule() {
    let (_temp, vault, day_file) = toggle_vault(
        "bob-cli-fresh-link-retire",
        "- [*] #task Work [scheduled::2026-07-20] ^t\n",
    );
    let target = vault.join("cash.md");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+t!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("link retires schedule");
    assert_success(&output);
    let after = fs::read_to_string(&target).expect("target");
    assert!(
        after.contains("[fresh:: 2026-07-10]"),
        "retiring a future schedule stamps: {after}"
    );
    assert!(!after.contains("[scheduled::"), "{after}");
}

#[test]
fn capture_link_never_stamps_recurring_tasks() {
    let (_temp, vault, day_file) = toggle_vault(
        "bob-cli-fresh-link-recurring",
        "- [ ] #task Water [repeat:: every week] [created::2026-07-01] ^w\n",
    );
    let target = vault.join("cash.md");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+w!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("link recurring");
    assert_success(&output);
    let after = fs::read_to_string(&target).expect("target");
    assert!(
        !after.contains("[fresh::"),
        "recurring tasks are never stamped: {after}"
    );
    assert!(after.starts_with("- [*] #task Water"), "{after}");
}

#[test]
fn capture_link_stamping_is_idempotent_same_day() {
    let (_temp, vault, day_file) =
        toggle_vault("bob-cli-fresh-link-idempotent", "- [ ] #task Work ^t\n");
    let target = vault.join("cash.md");
    let day = day_file.clone();
    let run = |now: &str| {
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg("@cash+t!")
            .env("BOB_DAY_FILE", &day)
            .env("BOB_NOW", now)
            .output()
            .expect("link")
    };
    let first = run("2026-07-10 10:00:00");
    assert_success(&first);
    let after_first = fs::read_to_string(&target).expect("first");
    // Unlink (ledger only), then link again the same day.
    let unlink = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+t!")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:05:00")
        .output()
        .expect("unlink");
    assert_success(&unlink);
    assert_eq!(
        fs::read_to_string(&target).expect("unlink target"),
        after_first,
        "unlink must not touch the task note"
    );
}

#[test]
fn capture_close_to_in_progress_stamps_but_complete_does_not() {
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-fresh-close");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(json["kind"], "pomodoro_close");
    let bob = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert!(
        bob.contains(
            "- [/] #task Add support for `=x` syntax! [fresh:: 2026-09-28] [created::2026-09-26] ^capture-stop"
        ),
        "=x rows that set [/] stamp: {bob}"
    );
    // Complete rows never stamp (checked in the selection test below).
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-fresh-close-complete");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x!2"]);
    assert_eq!(json["kind"], "pomodoro_close");
    let bob = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert!(
        !bob.contains(
            "[fresh:: 2026-09-28] [created::2026-09-21]  [completion::"
        ),
        "=x complete must not stamp: {bob}"
    );
}

#[test]
fn capture_new_tasks_carry_no_fresh() {
    let temp = TempDir::new("bob-cli-fresh-creation");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, "## Pomodoros\n- [ ] () — CURRENT\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Fresh idea @cash")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 10:00:00")
        .output()
        .expect("routed capture");
    assert_success(&output);
    let cash = fs::read_to_string(vault.join("cash.md")).expect("cash");
    assert!(
        !cash.contains("[fresh::"),
        "new captures never stamp: {cash}"
    );
}
