//! Same-line Pomodoro session-operator chain integration tests.

use super::pomodoro_close::close_worked_vault;
use crate::support::*;
use std::fs;

fn run_capture_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
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
        .expect("run chain capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("chain JSON")
}

#[test]
fn chain_matches_blank_line_batch_file_by_file() {
    let (_temp_a, vault_a, day_a) = close_worked_vault("bob-cli-chain-equiv-a");
    let (_temp_b, vault_b, day_b) = close_worked_vault("bob-cli-chain-equiv-b");
    let now = "2026-09-28 09:37:00";

    let chain = run_capture_json(&vault_a, &day_a, now, &["+2 =x"]);
    let blank_output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_b)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_b)
            .env("BOB_NOW", now),
        "+2\n\n=x\n",
    );
    assert_success(&blank_output);
    let blank: serde_json::Value =
        serde_json::from_str(stdout(&blank_output).trim()).expect("blank JSON");

    assert_eq!(
        fs::read_to_string(&day_a).expect("read chain day"),
        fs::read_to_string(&day_b).expect("read blank day")
    );
    assert_eq!(
        fs::read_to_string(vault_a.join("bob.md")).expect("read chain bob"),
        fs::read_to_string(vault_b.join("bob.md")).expect("read blank bob")
    );
    assert_eq!(
        fs::read_to_string(vault_a.join("sase.md")).expect("read chain sase"),
        fs::read_to_string(vault_b.join("sase.md")).expect("read blank sase")
    );
    assert_eq!(chain["captures"][0]["kind"], "pomodoro_adjust");
    assert_eq!(chain["captures"][1]["kind"], "pomodoro_close");
    assert_eq!(
        chain["captures"][0]["pomodoro_adjust"],
        blank["captures"][0]["pomodoro_adjust"]
    );
    assert_eq!(
        chain["captures"][1]["pomodoro_close"],
        blank["captures"][1]["pomodoro_close"]
    );
    assert_eq!(chain["captures"][0]["kind"], blank["captures"][0]["kind"]);
    assert_eq!(chain["captures"][1]["kind"], blank["captures"][1]["kind"]);
}

#[test]
fn chain_switches_sessions_like_a_blank_line_batch() {
    let (_temp_a, vault_a, day_a) =
        close_worked_vault("bob-cli-chain-switch-a");
    let (_temp_b, vault_b, day_b) =
        close_worked_vault("bob-cli-chain-switch-b");
    let now = "2026-09-28 09:37:00";

    let chain = run_capture_json(&vault_a, &day_a, now, &["=x ="]);
    assert_eq!(chain["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(chain["captures"][1]["kind"], "pomodoro_start");
    let blank_output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_b)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_b)
            .env("BOB_NOW", now),
        "=x\n\n=\n",
    );
    assert_success(&blank_output);
    assert_eq!(
        fs::read_to_string(&day_a).expect("read chain day"),
        fs::read_to_string(&day_b).expect("read blank day")
    );
}

#[test]
fn chain_positional_args_form_works() {
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-chain-positional");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("+2")
        .arg("=x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("run positional chain");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("chain JSON");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_adjust");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_close");
}

#[test]
fn chain_dry_run_writes_nothing() {
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-chain-dry");
    let before_day = fs::read_to_string(&day_file).expect("read day");
    let before_bob =
        fs::read_to_string(vault.join("bob.md")).expect("read bob");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg("+2 =x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("run dry chain");
    assert_success(&output);
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), before_day);
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("read bob"),
        before_bob
    );
}

#[test]
fn chain_failure_rolls_back_with_item_two_prefix() {
    for args in [["+2 +0"], ["=x -2"]] {
        let (_temp, vault, day_file) =
            close_worked_vault("bob-cli-chain-rollback");
        let before = fs::read_to_string(&day_file).expect("read day");
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .args(args)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-28 09:37:00")
            .output()
            .expect("run failing chain");
        assert!(!output.status.success(), "{}", format_output(&output));
        assert!(
            stdout(&output).contains("capture item 2 starting on line 1"),
            "{}",
            format_output(&output)
        );
        assert_eq!(fs::read_to_string(&day_file).expect("read day"), before);
    }
}

#[test]
fn chain_human_output_numbers_both_items() {
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-chain-human");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("+2 =x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("run human chain");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("1/2") && out.contains("2/2"), "{out}");
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn chain_capture_parse_reports_per_token_items() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("+2 =x")
        .output()
        .expect("run chain capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["schema_version"], 1);
    let items = json["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["mode"], "pomodoro_adjust");
    assert_eq!(items[1]["mode"], "pomodoro_close");
    assert_eq!(
        items[0]["range"],
        serde_json::json!({ "start": 0, "end": 2 })
    );
    assert_eq!(
        items[1]["range"],
        serde_json::json!({ "start": 3, "end": 5 })
    );
    assert_eq!(items[0]["line_start"], 1);
    assert_eq!(items[0]["line_end"], 1);
    assert_eq!(items[1]["line_start"], 1);
    assert_eq!(items[1]["line_end"], 1);
    assert_eq!(items[0]["index"], 1);
    assert_eq!(items[1]["index"], 2);
    assert!(
        json["spans"].as_array().expect("spans").iter().any(|span| {
            span["kind"] == "pomodoro_adjust"
                && span["start"] == 0
                && span["end"] == 2
        }),
        "{json}"
    );
    assert!(
        json["spans"].as_array().expect("spans").iter().any(|span| {
            span["kind"] == "pomodoro_close"
                && span["start"] == 3
                && span["end"] == 5
        }),
        "{json}"
    );
}

fn chain_blocks_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260930.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0620-0710** [t:: 50m]) — CLEANUP\n",
            "\t- quick note\n",
            "- [ ] () — GTD\n",
            "\t- [[#^gtd]]\n",
        ),
    );
    (temp, vault, day_file)
}

#[test]
fn chain_adjust_then_close_reports_one_cumulative_block() {
    let (_temp, vault, day_file) =
        chain_blocks_vault("bob-cli-chain-blocks-adjust-close");
    let day_before = fs::read_to_string(&day_file).expect("read day before");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-09-30 07:20:00",
        &["+2 =x"],
    );
    let day_after = fs::read_to_string(&day_file).expect("read day after");
    assert_pomodoro_blocks_cover_changes(&day_before, &day_after, &json);
    assert_eq!(json["captures"][0]["kind"], "pomodoro_adjust");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_close");
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "2026/20260930.md",
                "line": 2,
                "name": "CLEANUP",
                "time_range": "0620-0720",
                "status": "completed",
                "created": false,
                "roles": ["adjusted", "closed"],
                "lines": [
                    {
                        "text": "- [x] (**0620-0720** [t:: 60m]) — CLEANUP",
                        "depth": 0,
                        "change": "changed",
                        "before": "- [ ] (**0620-0710** [t:: 50m]) — CLEANUP",
                    },
                    {
                        "text": "\t- quick note",
                        "depth": 1,
                        "change": "unchanged",
                    },
                ],
            },
            {
                "relative_target": "2026/20260930.md",
                "line": 4,
                "name": "GTD",
                "status": "queued",
                "created": false,
                "roles": ["next"],
                "lines": [
                    {
                        "text": "- [ ] () — GTD",
                        "depth": 0,
                        "change": "unchanged",
                    },
                    {
                        "text": "\t- [[#^gtd]]",
                        "depth": 1,
                        "change": "unchanged",
                    },
                ],
            },
        ])
    );
}

#[test]
fn chain_close_then_start_reports_next_started_block() {
    let (_temp, vault, day_file) =
        chain_blocks_vault("bob-cli-chain-blocks-close-start");
    let day_before = fs::read_to_string(&day_file).expect("read day before");
    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-09-30 07:20:00",
        &["=x ="],
    );
    let day_after = fs::read_to_string(&day_file).expect("read day after");
    assert_pomodoro_blocks_cover_changes(&day_before, &day_after, &json);
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_start");
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "2026/20260930.md",
                "line": 2,
                "name": "CLEANUP",
                "time_range": "0620-0710",
                "status": "completed",
                "created": false,
                "roles": ["closed"],
                "lines": [
                    {
                        "text": "- [x] (**0620-0710** [t:: 50m]) — CLEANUP",
                        "depth": 0,
                        "change": "changed",
                        "before": "- [ ] (**0620-0710** [t:: 50m]) — CLEANUP",
                    },
                    {
                        "text": "\t- quick note",
                        "depth": 1,
                        "change": "unchanged",
                    },
                ],
            },
            {
                "relative_target": "2026/20260930.md",
                "line": 4,
                "name": "GTD",
                "time_range": "0720-0745",
                "status": "running",
                "created": false,
                "roles": ["next", "started"],
                "lines": [
                    {
                        "text": "- [ ] (**0720-0745** [t:: 25m]) — GTD",
                        "depth": 0,
                        "change": "changed",
                        "before": "- [ ] () — GTD",
                    },
                    {
                        "text": "\t- [[#^gtd]]",
                        "depth": 1,
                        "change": "unchanged",
                    },
                ],
            },
        ])
    );
}

#[test]
fn chain_capture_complete_returns_no_candidates() {
    for (text, cursor) in [("+2 =x", 1), ("+2 =x", 4)] {
        let output = bob_command()
            .arg("capture-complete")
            .arg("-c")
            .arg(cursor.to_string())
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .output()
            .expect("run chain capture-complete");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("complete JSON");
        assert_eq!(json["ok"], true, "{text} {cursor}");
        assert_eq!(
            json["candidates"],
            serde_json::json!([]),
            "{text} {cursor}"
        );
    }
}
