//! Pomodoro, tmux and script binaries, not help.

use crate::support::*;
use std::fs;
use std::process::Output;

#[test]
fn pomodoro_formats_native_pomodoro_status() {
    let temp = TempDir::new("bob-cli-pomodoro-path");
    let output = bob_command()
        .arg("pomodoro")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn tmux_pomodoro_formats_native_pomodoro_status() {
    let temp = TempDir::new("bob-cli-tmux-path");
    let output = bob_command()
        .arg("tmux-pomodoro")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob tmux-pomodoro");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "[<65m] 0945-1015 Review crate skeleton · plan 0/3 · 0/10 | "
    );
}

#[test]
fn tmux_pomodoro_appends_named_budget_meter() {
    let temp = TempDir::new("bob-cli-tmux-named");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("2026/20260601.md"),
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] () — GOALS\n",
            "  - [[tasks#^one]]\n",
            "  - [[tasks#^two]]\n",
            "- [ ] () — DECKS\n",
            "  - [[tasks#^three]]\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task One ^one\n");

    let output = bob_command()
        .arg("tmux-pomodoro")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob tmux-pomodoro with a named ledger");

    assert_success(&output);
    assert_eq!(stdout(&output), "plan 2/3 · 3/10 | ");
}

#[test]
fn tmux_pomodoro_reverses_over_cap_meter() {
    let temp = TempDir::new("bob-cli-tmux-over");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("2026/20260601.md"),
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] () — ONE\n",
            "- [ ] () — TWO\n",
            "- [ ] () — THREE\n",
            "- [ ] () — FOUR\n",
        ),
    );

    let output = bob_command()
        .arg("tmux-pomodoro")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob tmux-pomodoro over the theme cap");

    assert_success(&output);
    assert_eq!(stdout(&output), "#[reverse]plan 4/3 · 0/10#[noreverse] | ");
}

#[test]
fn tmux_pomodoro_omits_meter_without_pomodoros_section() {
    let temp = TempDir::new("bob-cli-tmux-no-section");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("2026/20260601.md"),
        "# Daily\n\nNo ledger here.\n",
    );

    let output = bob_command()
        .arg("tmux-pomodoro")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob tmux-pomodoro without a section");

    assert_success(&output);
    assert_eq!(stdout(&output), "");
}

#[test]
fn pomodoro_reads_default_bare_daily_file_from_bob_dir() {
    let temp = TempDir::new("bob-cli-pomodoro-default-bare");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("2026/20260601.md"),
        &fs::read_to_string(fixture("pomodoro/day_with_open_pomodoro.md"))
            .expect("read pomodoro fixture"),
    );

    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro with default bare daily path");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn script_pomodoro_reads_default_bare_daily_file_from_bob_dir() {
    let temp = TempDir::new("bob-cli-script-pomodoro-default-bare");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("2026/20260601.md"),
        &fs::read_to_string(fixture("pomodoro/day_with_open_pomodoro.md"))
            .expect("read pomodoro fixture"),
    );

    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run script bob pomodoro with default bare daily path");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn script_pomodoro_accepts_inline_duration_field_in_time_range() {
    let temp = TempDir::new("bob-cli-script-pomodoro");
    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run script bob pomodoro");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn pomodoro_accepts_legacy_unbolded_inline_duration_field_in_time_range() {
    let temp = TempDir::new("bob-cli-pomodoro-legacy");
    let output = bob_command()
        .arg("pomodoro")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_legacy_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro with legacy time range");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn script_pomodoro_accepts_legacy_unbolded_time_range() {
    let temp = TempDir::new("bob-cli-script-pomodoro-legacy");
    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_legacy_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run script bob pomodoro with legacy time range");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");
}

#[test]
fn pomodoro_stale_cutoff_is_empty_unless_requested() {
    for script_fallback in [false, true] {
        let output = run_pomodoro_fixture(
            &[],
            "pomodoro/day_with_open_pomodoro.md",
            "2026-06-01 10:25:00",
            script_fallback,
        );
        assert_success(&output);
        assert!(
            stdout(&output).is_empty(),
            "default stale Pomodoro output should be empty with script_fallback={script_fallback}:\n{}",
            format_output(&output)
        );

        let output = run_pomodoro_fixture(
            &["--show-stale"],
            "pomodoro/day_with_open_pomodoro.md",
            "2026-06-01 10:25:00",
            script_fallback,
        );
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            "[OVERDUE by 10m] 0945-1015 Review crate skeleton\n",
            "show-stale should report exact cutoff with script_fallback={script_fallback}"
        );

        let output = run_pomodoro_fixture(
            &["-s"],
            "pomodoro/day_with_open_pomodoro.md",
            "2026-06-01 10:31:01",
            script_fallback,
        );
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            "[OVERDUE by 16m] 0945-1015 Review crate skeleton\n",
            "short show-stale alias should report beyond cutoff with script_fallback={script_fallback}"
        );
    }
}

#[test]
fn pomodoro_show_stale_keeps_no_open_day_empty() {
    for script_fallback in [false, true] {
        let output = run_pomodoro_fixture(
            &["--show-stale"],
            "pomodoro/day_without_open_pomodoro.md",
            "2026-06-01 10:25:00",
            script_fallback,
        );
        assert_success(&output);
        assert!(
            stdout(&output).is_empty(),
            "show-stale should not invent an open Pomodoro with script_fallback={script_fallback}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn pomodoro_missing_day_file_is_a_successful_noop() {
    let temp = TempDir::new("bob-cli-pomodoro-missing");
    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_DAY_FILE", temp.path().join("missing-day.md"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro with missing day file");

    assert_success(&output);
    assert!(stdout(&output).is_empty(), "expected empty stdout");
    assert!(stderr(&output).is_empty(), "expected empty stderr");
}

fn run_pomodoro_fixture(
    args: &[&str],
    fixture_relative: &str,
    now: &str,
    script_fallback: bool,
) -> Output {
    let temp = TempDir::new("bob-cli-pomodoro-fixture");
    let mut command = bob_command();
    command
        .arg("pomodoro")
        .args(args)
        .env("BOB_DAY_FILE", fixture(fixture_relative))
        .env("BOB_NOW", now)
        .env("XDG_CACHE_HOME", temp.path().join("cache"));

    if script_fallback {
        command.env("BOB_CLI_USE_SCRIPT", "1");
    }

    command.output().unwrap_or_else(|error| {
        panic!("run bob pomodoro {args:?} with {fixture_relative}: {error}")
    })
}
