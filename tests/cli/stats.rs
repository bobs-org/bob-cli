//! `bob stats` overview, show, and metrics.

use crate::support::*;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Instant;

const NOW: &str = "2026-10-11 12:00:00";
const RANGE: &str = "2026-10-08..2026-10-11";

fn fixture_vault() -> PathBuf {
    fixture("stats/vault")
}

fn stats_command() -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", fixture_vault())
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    command
}

#[test]
fn overview_human_names_metrics() {
    let output = stats_command()
        .args(["stats", "-r", RANGE])
        .output()
        .expect("run bob stats");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("Stats"), "missing Stats:\n{text}");
    assert!(text.contains("Worked"), "missing Worked:\n{text}");
    assert!(text.contains("today so far"), "missing today:\n{text}");
    assert!(
        text.contains("Review closed not yet"),
        "missing provisional boolean:\n{text}"
    );
    assert!(
        text.contains('◌'),
        "missing provisional boolean spark mark:\n{text}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn overview_json_is_ok_schema_one() {
    let output = stats_command()
        .args(["stats", "-r", RANGE, "-f", "json"])
        .output()
        .expect("run bob stats -f json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("overview JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["today"], "2026-10-11");
    let metrics = value["metrics"].as_array().expect("metrics");
    let worked = metrics
        .iter()
        .find(|metric| metric["id"] == "worked")
        .expect("worked");
    assert_eq!(worked["chart"]["group"], "day");
    assert!(worked.get("daily_points").is_none());
    assert_eq!(worked["points"][1]["parts_display"]["theme"]["PLAN"], "29m");
    let review_closed = metrics
        .iter()
        .find(|metric| metric["id"] == "review_closed")
        .expect("review_closed");
    assert_eq!(review_closed["points"][3]["display"], "not yet");
    assert!(!stdout(&output).trim().contains('\n'));
}

#[test]
fn overview_csv_has_tidy_header() {
    let output = stats_command()
        .args(["stats", "-r", RANGE, "-f", "csv"])
        .output()
        .expect("run bob stats -f csv");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.starts_with(
        "date,end,metric,value,display,quality,provisional,num,den\n"
    ));
    assert!(text.contains(",worked,"));
    assert!(text.contains(",review_closed,0,not yet,ok,true,,"));
}

#[test]
fn show_worked_json_includes_compare_points() {
    let output = stats_command()
        .args(["stats", "show", "worked", "-c", "-r", RANGE, "-f", "json"])
        .output()
        .expect("run bob stats show worked -c");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("show JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["metrics"][0]["id"], "worked");
    assert!(value["range"]["prior"].is_object());
}

#[test]
fn show_all_json_lists_visible_metrics() {
    let output = stats_command()
        .args(["stats", "show", "-a", "-r", RANGE, "-f", "json"])
        .output()
        .expect("run bob stats show -a");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("show -a JSON");
    let ids: Vec<&str> = value["metrics"]
        .as_array()
        .expect("metrics")
        .iter()
        .map(|metric| metric["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"worked"));
    assert!(ids.contains(&"done"));
    let habits = value["metrics"]
        .as_array()
        .expect("metrics")
        .iter()
        .find(|metric| metric["id"] == "habits")
        .expect("habits");
    assert_eq!(habits["points"][1]["checklist_state"], "missed");
    assert_eq!(habits["chores"][0]["display"], "0 of 2 days");
    assert_eq!(habits["chores"][1]["display"], "1 of 2 days");
}

#[test]
fn metrics_catalog_lists_worked() {
    let output = stats_command()
        .args(["stats", "metrics"])
        .output()
        .expect("run bob stats metrics");
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("worked"), "missing worked:\n{text}");
    assert!(text.contains("pomodoro"), "missing kind:\n{text}");
}

#[test]
fn metrics_json_omits_points() {
    let output = stats_command()
        .args(["stats", "metrics", "-f", "json"])
        .output()
        .expect("run bob stats metrics -f json");
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("metrics JSON");
    assert_eq!(value["ok"], true);
    assert!(value["metrics"][0].get("points").is_none());
    assert!(value["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|metric| metric["id"] == "worked"));
}

#[test]
fn unknown_metric_is_exit_two_json_envelope() {
    let output = stats_command()
        .args(["stats", "show", "nope", "-f", "json"])
        .output()
        .expect("run bob stats show nope");
    assert_eq!(output.status.code(), Some(2));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("error JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["error"]["code"], "unknown_metric");
}

#[test]
fn missing_vault_is_exit_one() {
    let output = stats_command()
        .args([
            "stats",
            "-b",
            "/definitely/missing/bob-stats-vault",
            "-f",
            "json",
        ])
        .output()
        .expect("run bob stats missing vault");
    assert_eq!(output.status.code(), Some(1));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("io JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "io");
}

#[test]
fn invalid_range_is_exit_two() {
    let output = stats_command()
        .args(["stats", "-r", "bogus", "-f", "json"])
        .output()
        .expect("run bob stats -r bogus");
    assert_eq!(output.status.code(), Some(2));
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("usage JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "usage");
}

#[test]
fn help_names_docs_and_exit_codes() {
    let output = bob_command()
        .args(["stats", "--help"])
        .output()
        .expect("run bob stats --help");
    assert_success(&output);
    let help = stdout(&output);
    assert!(help.contains("Usage: bob stats"));
    assert!(help.contains("overview"));
    assert!(help.contains("docs/stats.md"));
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn completion_includes_worked() {
    let output = bob_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "stats",
            "show",
            "",
        ])
        .output()
        .expect("complete bob stats show");
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.lines().any(|line| line.starts_with("worked\t")),
        "missing worked completion:\n{text}"
    );
}

/// Opt-in live-vault CLI path. The engine oracle lives in
/// `src/native/stats/compute.rs`; this covers `bob stats` itself.
#[test]
fn real_vault_parity_is_opt_in() {
    if std::env::var_os("BOB_STATS_REAL_VAULT_PARITY").is_none() {
        return;
    }
    let vault = std::env::var_os("BOB_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join("bob"))
        })
        .expect("BOB_STATS_REAL_VAULT_PARITY=1 requires BOB_DIR or HOME");
    let now = std::env::var("BOB_NOW").unwrap_or_else(|_| {
        panic!("BOB_STATS_REAL_VAULT_PARITY=1 requires BOB_NOW")
    });
    let started = Instant::now();
    let output = bob_command()
        .args(["stats", "show", "worked", "-c", "-r", "7d", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", &now)
        .output()
        .expect("run bob stats show worked against the real vault");
    let elapsed = started.elapsed();
    eprintln!(
        "bob stats show worked -c -f json took {}ms",
        elapsed.as_millis()
    );
    assert_success(&output);
    let value: Value =
        serde_json::from_str(stdout(&output).trim()).expect("real vault JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["metrics"][0]["id"], "worked");

    let overview = bob_command()
        .args(["stats", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", &now)
        .output()
        .expect("run bob stats -f json against the real vault");
    assert_success(&overview);
    let overview_value: Value = serde_json::from_str(stdout(&overview).trim())
        .expect("real vault overview JSON");
    assert_eq!(overview_value["ok"], true);

    let catalog = bob_command()
        .args(["stats", "metrics", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", &now)
        .output()
        .expect("run bob stats metrics against the real vault");
    assert_success(&catalog);
    let catalog_value: Value = serde_json::from_str(stdout(&catalog).trim())
        .expect("real vault catalog JSON");
    assert_eq!(catalog_value["ok"], true);
}
