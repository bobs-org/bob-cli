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
        .args(["pomodoro", "tmux"])
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro tmux");

    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "[<65m] 0945-1015 Review crate skeleton · plan 0/10 | "
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
        .args(["pomodoro", "tmux"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro tmux with a named ledger");

    assert_success(&output);
    assert_eq!(stdout(&output), "plan 3/10 | ");
}

#[test]
fn tmux_pomodoro_reverses_over_cap_meter() {
    // The link cap drives `over`: eleven links on one entry with the
    // default cap of ten render reversed.
    let temp = TempDir::new("bob-cli-tmux-over");
    let vault = temp.path().join("vault");
    let mut note =
        String::from("# Daily\n\n## Pomodoros\n\n- [ ] () — GOALS\n");
    for index in 0..11 {
        note.push_str(&format!("  - [[tasks#^{index:04}]]\n"));
    }
    write_file(&vault.join("2026/20260601.md"), &note);
    write_file(&vault.join("tasks.md"), "- [ ] #task One ^0000\n");

    let output = bob_command()
        .args(["pomodoro", "tmux"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro tmux over the link cap");

    assert_success(&output);
    assert_eq!(stdout(&output), "#[reverse]plan 11/10#[noreverse] | ");
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
        .args(["pomodoro", "tmux"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro tmux without a section");

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

#[test]
fn pomodoro_worked_reports_closed_ledger_total_only_when_status_is_empty() {
    for args in [
        &["--worked"][..],
        &["-w"][..],
        &["--show-stale", "--worked"][..],
    ] {
        let output = run_pomodoro_fixture(
            args,
            "pomodoro/day_with_worked_pomodoros.md",
            "2026-06-01 12:00:00",
            false,
        );
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            "[WORKED 50m]\n",
            "tracked DEEP WORK 50m counts; untracked GTD and LATE NIGHT do not"
        );
        assert!(stderr(&output).is_empty());
    }

    let output = run_pomodoro_fixture(
        &["--worked"],
        "pomodoro/day_with_open_pomodoro.md",
        "2026-06-01 09:10:01",
        false,
    );
    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");

    let output = run_pomodoro_fixture(
        &["--show-stale", "--worked"],
        "pomodoro/day_with_open_pomodoro.md",
        "2026-06-01 10:25:00",
        false,
    );
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "[OVERDUE by 10m] 0945-1015 Review crate skeleton\n"
    );

    let output = run_pomodoro_fixture(
        &["--worked"],
        "pomodoro/day_with_open_pomodoro.md",
        "2026-06-01 10:25:00",
        false,
    );
    assert_success(&output);
    assert_eq!(stdout(&output), "[WORKED 0m]\n");

    let temp = TempDir::new("bob-cli-pomodoro-worked-missing");
    let output = bob_command()
        .args(["pomodoro", "--worked"])
        .env("BOB_DAY_FILE", temp.path().join("missing-day.md"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro --worked with missing day file");
    assert_success(&output);
    assert_eq!(stdout(&output), "[WORKED 0m]\n");
    assert!(stderr(&output).is_empty());

    let output = run_pomodoro_fixture(
        &["--worked"],
        "pomodoro/day_with_worked_pomodoros.md",
        "2026-06-01 12:00:00",
        true,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("Unexpected argument: --worked"));
}

#[test]
fn pomodoro_last_reports_latest_closed_end() {
    for args in [&["--worked", "--last"][..], &["-w", "-l"][..]] {
        let output = run_pomodoro_fixture(
            args,
            "pomodoro/day_with_last_closed_pomodoros.md",
            "2026-06-02 17:00:00",
            false,
        );
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            "[WORKED 80m] [LAST 1430-1530] — GYM\n",
            "off-clock GYM end is the last end; worked total omits it"
        );
        assert!(stderr(&output).is_empty());
    }

    let output = run_pomodoro_fixture(
        &["--last"],
        "pomodoro/day_with_last_closed_pomodoros.md",
        "2026-06-02 17:00:00",
        false,
    );
    assert_success(&output);
    assert_eq!(stdout(&output), "[LAST 1430-1530] — GYM\n");
    assert!(stderr(&output).is_empty());

    let output = run_pomodoro_fixture(
        &["--worked", "--last"],
        "pomodoro/day_with_worked_pomodoros.md",
        "2026-06-01 12:00:00",
        false,
    );
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "[WORKED 50m] [LAST 2350-0010] — LATE NIGHT\n",
        "midnight wrap sorts after every same-day end"
    );

    let output = run_pomodoro_fixture(
        &["--worked", "--last"],
        "pomodoro/day_with_open_pomodoro.md",
        "2026-06-01 09:10:01",
        false,
    );
    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton\n");

    let output = run_pomodoro_fixture(
        &["--worked", "--last"],
        "pomodoro/day_with_open_pomodoro.md",
        "2026-06-01 10:25:00",
        false,
    );
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "[WORKED 0m]\n",
        "trailing-only closed row has no leading range, so no LAST"
    );

    let temp = TempDir::new("bob-cli-pomodoro-last-missing");
    let output = bob_command()
        .args(["pomodoro", "--worked", "--last"])
        .env("BOB_DAY_FILE", temp.path().join("missing-day.md"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro --worked --last with missing day file");
    assert_success(&output);
    assert_eq!(stdout(&output), "[WORKED 0m]\n");
    assert!(stderr(&output).is_empty());

    let output = bob_command()
        .args(["pomodoro", "--last"])
        .env("BOB_DAY_FILE", temp.path().join("missing-day.md"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro --last with missing day file");
    assert_success(&output);
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).is_empty());

    let output = run_pomodoro_fixture(
        &["--last"],
        "pomodoro/day_with_last_closed_pomodoros.md",
        "2026-06-02 17:00:00",
        true,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("Unexpected argument: --last"));
}

#[test]
fn pomodoro_last_prints_derived_chip_for_unnamed_closed_session() {
    let temp = TempDir::new("bob-cli-pomodoro-last-homes-unnamed");
    let vault = temp.path().join("vault");
    write_homes_vault(
        &vault,
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [x] (0950-1015)\n",
            "  - [[sase#^one]]\n",
        ),
    );

    let output = bob_command()
        .args(["pomodoro", "--last"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 12:00:00")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro --last unnamed homes");

    assert_success(&output);
    assert_eq!(stdout(&output), "[LAST 0950-1015] sase\n");
}

#[test]
fn pomodoro_prints_derived_chip_for_unnamed_linked_session() {
    let temp = TempDir::new("bob-cli-pomodoro-homes-unnamed");
    let vault = temp.path().join("vault");
    write_homes_vault(
        &vault,
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] (0950-1015)\n",
            "  - [[sase#^one]]\n",
        ),
    );

    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:52:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro unnamed homes");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<23m] 0950-1015 sase\n");
}

#[test]
fn tmux_pomodoro_prints_derived_chip_for_unnamed_linked_session() {
    let temp = TempDir::new("bob-cli-tmux-homes-unnamed");
    let vault = temp.path().join("vault");
    write_homes_vault(
        &vault,
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] (0950-1015)\n",
            "  - [[sase#^one]]\n",
        ),
    );

    let output = bob_command()
        .args(["pomodoro", "tmux"])
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:52:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro tmux unnamed homes");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<23m] 0950-1015 sase · plan 1/10 | ");
}

#[test]
fn pomodoro_named_session_skips_home_resolution() {
    let temp = TempDir::new("bob-cli-pomodoro-homes-named");
    let vault = temp.path().join("vault");
    write_homes_vault(
        &vault,
        concat!(
            "# Daily\n\n",
            "## Pomodoros\n\n",
            "- [ ] (0950-1015) — HACK\n",
            "  - [[sase#^one]]\n",
        ),
    );

    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:52:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro named homes");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<23m] 0950-1015 — HACK\n");
}

#[test]
fn pomodoro_unnamed_unlinked_session_prints_range_only() {
    let temp = TempDir::new("bob-cli-pomodoro-homes-unlinked");
    let vault = temp.path().join("vault");
    write_homes_vault(
        &vault,
        concat!("# Daily\n\n", "## Pomodoros\n\n", "- [ ] (0950-1015)\n"),
    );

    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_DIR", &vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260601.md"))
        .env("BOB_NOW", "2026-06-01 09:52:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro unlinked unnamed");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<23m] 0950-1015\n");
}

fn write_homes_vault(vault: &std::path::Path, daily: &str) {
    write_file(
        &vault.join("gtd.md"),
        "---\ntype: [[area]]\ncolor: orange\n---\n",
    );
    write_file(
        &vault.join("sase.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "parent: [[gtd]]\n",
            "---\n",
            "- [ ] #task One ^one\n",
        ),
    );
    write_file(&vault.join("2026/20260601.md"), daily);
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
