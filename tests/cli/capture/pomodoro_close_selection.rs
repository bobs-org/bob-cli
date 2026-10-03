//! Pomodoro close `=x<N>!<M>` selection tests on the worked example.

use super::pomodoro_close::{
    close_worked_vault, run_close_expect_error, run_close_json,
};
use crate::support::*;
use std::fs;
use std::path::PathBuf;

fn close_entry_range(line: &str) -> bool {
    line.contains("(**0920-0940** [t:: 20m])")
}

fn worked_prefix() -> &'static str {
    concat!(
        "## Pomodoros\n",
        "\n",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
        "\t- \u{1F345} [[bob#^capture-stop]]\n",
    )
}

fn worked_suffix() -> &'static str {
    concat!("- [ ] () — SASE\n", "\t- [[sase#^recovery-panel]]\n",)
}

fn plain_bob_work_log() -> &'static str {
    concat!(
        "\t- \u{1F6E0}\u{FE0F} **WORK LOG**\n",
        "\t\t- *2026-09-28* — Designed the `=x` grammar\n",
        "\t\t\t- chose `x` for done\n",
        "\t\t- *2026-09-28* — Wrote the plan\n",
    )
}

fn plain_sase_after() -> String {
    concat!(
        "## Tasks\n",
        "\n",
        "- [x] #task Restart axe [created::2026-09-27] [completion:: 2026-09-28] ^axe-restart\n",
        "  - \u{1F6E0}\u{FE0F} **WORK LOG**\n",
        "    - *2026-09-28* — Restarted axe\n",
        "    - _2026-09-27_ — Diagnosed the hang\n",
        "- [ ] #task Recovery panel [created::2026-09-25] ^recovery-panel\n",
    )
    .to_string()
}

#[test]
fn capture_pomodoro_close_selection_defer_rest() {
    // `=x2`: task 1 deferred (unlisted), task 2 in progress (listed).
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-2");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x2"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x2");
    assert_eq!(close["in_progress"], serde_json::json!([2]));
    assert_eq!(close["complete"], serde_json::json!([]));
    assert_eq!(
        close["task_links"],
        serde_json::json!([
            {
                "index": 1,
                "ledger_line": 6,
                "block_link": "[[bob#^capture-stop]]",
                "block_id": "capture-stop",
                "marker": "plain",
                "outcome": "deferred",
                "source": "unlisted"
            },
            {
                "index": 2,
                "ledger_line": 10,
                "block_link": "[[bob#^web-capture]]",
                "block_id": "web-capture",
                "marker": "deferred",
                "outcome": "in_progress",
                "source": "listed"
            }
        ])
    );
    assert_eq!(close["tasks"][0]["role"], "deferred");
    assert_eq!(close["tasks"][0]["index"], 1);
    assert_eq!(close["tasks"][0]["status_symbol"], "*");
    assert_eq!(close["tasks"][0]["status_changed"], false);
    assert_eq!(close["tasks"][0]["work_log"].as_array().unwrap().len(), 2);
    assert_eq!(close["tasks"][1]["role"], "worked");
    assert_eq!(close["tasks"][1]["index"], 2);
    assert_eq!(close["tasks"][1]["status_symbol"], "/");
    assert_eq!(close["tasks"][1]["status_changed"], true);
    assert_eq!(close["tasks"][2]["index"], serde_json::Value::Null);
    assert_eq!(
        close["carried"],
        serde_json::json!([
            {"kind": "worked", "text": "[[bob#^web-capture]]"},
            {"kind": "deferred", "text": "[[bob#^capture-stop]]"}
        ])
    );
    assert_eq!(close["next_pomodoro"]["line"], 13);
    assert_eq!(close["next_pomodoro"]["created"], true);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        format!(
            concat!(
                "{}",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
                "\t\t- Designed the `=x` grammar\n",
                "\t\t\t- chose `x` for done\n",
                "\t\t- Wrote the plan\n",
                "\t- \u{1F345} [[bob#^web-capture]]\n",
                "\t- ~~[[sase#^axe-restart]]~~\n",
                "\t\t- Restarted axe\n",
                "\t- quick note\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^web-capture]]\n",
                "\t- [[bob#^capture-stop]]\n",
                "{}",
            ),
            worked_prefix(),
            worked_suffix(),
        )
    );
    assert!(close_entry_range(&day_after));
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert_eq!(
        bob_after,
        format!(
            concat!(
                "## Tasks\n",
                "\n",
                "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
                "{}",
                "- [/] #task Add capture support for web URLs! [fresh:: 2026-09-28] [created::2026-09-21] ^web-capture\n",
                "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
            ),
            plain_bob_work_log(),
        )
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("sase"),
        plain_sase_after(),
    );
}

#[test]
fn capture_pomodoro_close_selection_in_progress_and_complete() {
    // `=x1!2`: task 1 in progress (listed), task 2 complete (listed).
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-12");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x1!2"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x1!2");
    assert_eq!(close["in_progress"], serde_json::json!([1]));
    assert_eq!(close["complete"], serde_json::json!([2]));
    assert_eq!(
        close["task_links"],
        serde_json::json!([
            {
                "index": 1,
                "ledger_line": 6,
                "block_link": "[[bob#^capture-stop]]",
                "block_id": "capture-stop",
                "marker": "plain",
                "outcome": "in_progress",
                "source": "listed"
            },
            {
                "index": 2,
                "ledger_line": 10,
                "block_link": "[[bob#^web-capture]]",
                "block_id": "web-capture",
                "marker": "deferred",
                "outcome": "complete",
                "source": "listed"
            }
        ])
    );
    assert_eq!(close["tasks"][0]["role"], "worked");
    assert_eq!(close["tasks"][0]["index"], 1);
    assert_eq!(close["tasks"][0]["status_symbol"], "/");
    assert_eq!(close["tasks"][0]["status_changed"], true);
    assert_eq!(close["tasks"][0]["work_log"].as_array().unwrap().len(), 2);
    assert_eq!(close["tasks"][1]["role"], "embedded");
    assert_eq!(close["tasks"][1]["index"], 2);
    assert_eq!(close["tasks"][1]["status_symbol"], "x");
    assert_eq!(close["tasks"][1]["status_changed"], true);
    assert_eq!(close["tasks"][2]["index"], serde_json::Value::Null);
    assert_eq!(
        close["carried"],
        serde_json::json!([
            {"kind": "worked", "text": "[[bob#^capture-stop]]"}
        ])
    );
    assert_eq!(close["next_pomodoro"]["line"], 14);
    assert_eq!(close["next_pomodoro"]["created"], true);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        format!(
            concat!(
                "{}",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
                "\t- \u{1F345} [[bob#^capture-stop]]\n",
                "\t\t- Designed the `=x` grammar\n",
                "\t\t\t- chose `x` for done\n",
                "\t\t- Wrote the plan\n",
                "\t- ~~[[bob#^web-capture]]~~\n",
                "\t- ~~[[sase#^axe-restart]]~~\n",
                "\t\t- Restarted axe\n",
                "\t- quick note\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^capture-stop]]\n",
                "{}",
            ),
            worked_prefix(),
            worked_suffix(),
        )
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert_eq!(
        bob_after,
        format!(
            concat!(
                "## Tasks\n",
                "\n",
                "- [/] #task Add support for `=x` syntax! [fresh:: 2026-09-28] [created::2026-09-26] ^capture-stop\n",
                "{}",
                "- [x] #task Add capture support for web URLs! [created::2026-09-21]  [completion:: 2026-09-28] ^web-capture\n",
                "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
            ),
            plain_bob_work_log(),
        )
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("sase"),
        plain_sase_after(),
    );
}

#[test]
fn capture_pomodoro_close_selection_complete_listed() {
    // `=x!1`: task 1 complete (listed), task 2 deferred (ledger).
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-c1");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x!1"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x!1");
    assert!(close["in_progress"].is_null());
    assert_eq!(close["complete"], serde_json::json!([1]));
    assert_eq!(
        close["task_links"],
        serde_json::json!([
            {
                "index": 1,
                "ledger_line": 6,
                "block_link": "[[bob#^capture-stop]]",
                "block_id": "capture-stop",
                "marker": "plain",
                "outcome": "complete",
                "source": "listed"
            },
            {
                "index": 2,
                "ledger_line": 10,
                "block_link": "[[bob#^web-capture]]",
                "block_id": "web-capture",
                "marker": "deferred",
                "outcome": "deferred",
                "source": "ledger"
            }
        ])
    );
    assert_eq!(close["tasks"][0]["role"], "embedded");
    assert_eq!(close["tasks"][0]["index"], 1);
    assert_eq!(close["tasks"][0]["status_symbol"], "x");
    assert_eq!(close["tasks"][0]["status_name"], "Done");
    assert_eq!(close["tasks"][0]["status_changed"], true);
    assert_eq!(close["tasks"][1]["role"], "deferred");
    assert_eq!(close["tasks"][1]["index"], 2);
    assert_eq!(close["tasks"][2]["index"], serde_json::Value::Null);
    assert_eq!(
        close["carried"],
        serde_json::json!([
            {"kind": "deferred", "text": "[[bob#^web-capture]]"}
        ])
    );
    assert_eq!(close["next_pomodoro"]["line"], 13);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        format!(
            concat!(
                "{}",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
                "\t- ~~[[bob#^capture-stop]]~~\n",
                "\t\t- Designed the `=x` grammar\n",
                "\t\t\t- chose `x` for done\n",
                "\t\t- Wrote the plan\n",
                "\t- ~~[[sase#^axe-restart]]~~\n",
                "\t\t- Restarted axe\n",
                "\t- quick note\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^web-capture]]\n",
                "{}",
            ),
            worked_prefix(),
            worked_suffix(),
        )
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert_eq!(
        bob_after,
        format!(
            concat!(
                "## Tasks\n",
                "\n",
                "- [x] #task Add support for `=x` syntax! [created::2026-09-26]  [completion:: 2026-09-28] ^capture-stop\n",
                "{}",
                "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
                "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
            ),
            plain_bob_work_log(),
        )
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("sase"),
        plain_sase_after(),
    );
}

#[test]
fn capture_pomodoro_close_selection_defer_all() {
    // `=x0`: every numbered link deferred (unlisted).
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-0");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x0"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x0");
    assert_eq!(close["in_progress"], serde_json::json!([]));
    assert_eq!(close["complete"], serde_json::json!([]));
    assert_eq!(
        close["task_links"],
        serde_json::json!([
            {
                "index": 1,
                "ledger_line": 6,
                "block_link": "[[bob#^capture-stop]]",
                "block_id": "capture-stop",
                "marker": "plain",
                "outcome": "deferred",
                "source": "unlisted"
            },
            {
                "index": 2,
                "ledger_line": 10,
                "block_link": "[[bob#^web-capture]]",
                "block_id": "web-capture",
                "marker": "deferred",
                "outcome": "deferred",
                "source": "unlisted"
            }
        ])
    );
    assert_eq!(close["tasks"][0]["role"], "deferred");
    assert_eq!(close["tasks"][0]["index"], 1);
    assert_eq!(close["tasks"][0]["status_symbol"], "*");
    assert_eq!(close["tasks"][1]["role"], "deferred");
    assert_eq!(close["tasks"][1]["index"], 2);
    assert_eq!(
        close["carried"],
        serde_json::json!([
            {"kind": "deferred", "text": "[[bob#^capture-stop]]"},
            {"kind": "deferred", "text": "[[bob#^web-capture]]"}
        ])
    );
    assert_eq!(close["next_pomodoro"]["line"], 12);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        format!(
            concat!(
                "{}",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
                "\t\t- Designed the `=x` grammar\n",
                "\t\t\t- chose `x` for done\n",
                "\t\t- Wrote the plan\n",
                "\t- ~~[[sase#^axe-restart]]~~\n",
                "\t\t- Restarted axe\n",
                "\t- quick note\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^capture-stop]]\n",
                "\t- [[bob#^web-capture]]\n",
                "{}",
            ),
            worked_prefix(),
            worked_suffix(),
        )
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert_eq!(
        bob_after,
        format!(
            concat!(
                "## Tasks\n",
                "\n",
                "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n",
                "{}",
                "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n",
                "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
            ),
            plain_bob_work_log(),
        )
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("sase"),
        plain_sase_after(),
    );
}

#[test]
fn capture_pomodoro_close_selection_listed_matches_ledger() {
    // `=x1` equals plain `=x` on this fixture: task 2 was already deferred.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-1");
    let one =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x1"]);
    let (_temp, plain_vault, plain_day) =
        close_worked_vault("bob-cli-close-sel-1-plain");
    run_close_json(&plain_vault, &plain_day, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        fs::read_to_string(&plain_day).expect("plain day")
    );
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("bob"),
        fs::read_to_string(plain_vault.join("bob.md")).expect("plain bob")
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("sase"),
        fs::read_to_string(plain_vault.join("sase.md")).expect("plain sase")
    );
    let close = &one["pomodoro_close"];
    assert_eq!(close["raw"], "=x1");
    assert_eq!(close["in_progress"], serde_json::json!([1]));
    assert_eq!(close["tasks"][0]["index"], 1);
    assert_eq!(close["task_links"][0]["outcome"], "in_progress");
    assert_eq!(close["task_links"][0]["source"], "listed");
    assert_eq!(close["task_links"][1]["outcome"], "deferred");
    assert_eq!(close["task_links"][1]["source"], "unlisted");

    // `=x1,2` un-defers task 2: both links worked, both tasks started.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-12b");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x1,2"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x1,2");
    assert_eq!(close["in_progress"], serde_json::json!([1, 2]));
    assert_eq!(
        close["task_links"],
        serde_json::json!([
            {
                "index": 1,
                "ledger_line": 6,
                "block_link": "[[bob#^capture-stop]]",
                "block_id": "capture-stop",
                "marker": "plain",
                "outcome": "in_progress",
                "source": "listed"
            },
            {
                "index": 2,
                "ledger_line": 10,
                "block_link": "[[bob#^web-capture]]",
                "block_id": "web-capture",
                "marker": "deferred",
                "outcome": "in_progress",
                "source": "listed"
            }
        ])
    );
    assert_eq!(close["tasks"][0]["index"], 1);
    assert_eq!(close["tasks"][1]["index"], 2);
    assert_eq!(close["tasks"][0]["role"], "worked");
    assert_eq!(close["tasks"][1]["role"], "worked");
    assert_eq!(close["tasks"][1]["status_symbol"], "/");
    assert_eq!(close["tasks"][1]["status_changed"], true);
    assert_eq!(
        close["carried"],
        serde_json::json!([
            {"kind": "worked", "text": "[[bob#^capture-stop]]"},
            {"kind": "worked", "text": "[[bob#^web-capture]]"}
        ])
    );
    assert_eq!(close["next_pomodoro"]["line"], 14);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        format!(
            concat!(
                "{}",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
                "\t- \u{1F345} [[bob#^capture-stop]]\n",
                "\t\t- Designed the `=x` grammar\n",
                "\t\t\t- chose `x` for done\n",
                "\t\t- Wrote the plan\n",
                "\t- \u{1F345} [[bob#^web-capture]]\n",
                "\t- ~~[[sase#^axe-restart]]~~\n",
                "\t\t- Restarted axe\n",
                "\t- quick note\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^capture-stop]]\n",
                "\t- [[bob#^web-capture]]\n",
                "{}",
            ),
            worked_prefix(),
            worked_suffix(),
        )
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert_eq!(
        bob_after,
        format!(
            concat!(
                "## Tasks\n",
                "\n",
                "- [/] #task Add support for `=x` syntax! [fresh:: 2026-09-28] [created::2026-09-26] ^capture-stop\n",
                "{}",
                "- [/] #task Add capture support for web URLs! [fresh:: 2026-09-28] [created::2026-09-21] ^web-capture\n",
                "- [ ] #task Plain ready task [created::2026-09-20] ^ready\n",
            ),
            plain_bob_work_log(),
        )
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("sase"),
        plain_sase_after(),
    );
}

#[test]
fn capture_pomodoro_close_plain_reports_numbered_lineup() {
    // Plain `=x` keeps its files and reports the ledger lineup.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-x");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x");
    assert!(close["in_progress"].is_null());
    assert_eq!(close["complete"], serde_json::json!([]));
    assert_eq!(close["task_links"].as_array().unwrap().len(), 2);
    assert!(close["task_links"]
        .as_array()
        .unwrap()
        .iter()
        .all(|link| link["source"] == "ledger"));
    assert_eq!(close["tasks"][0]["index"], 1);
    assert_eq!(close["tasks"][1]["index"], 2);
    assert_eq!(close["tasks"][2]["index"], serde_json::Value::Null);
}

#[test]
fn capture_pomodoro_close_selection_human_output() {
    // `=x1!2` human rows carry a numbered index column and the singular
    // `carries 1 link`; dry-run prints the same rows with `would close`.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-hu");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("=x1!2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("human close");
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        vec![
            "\u{2713} closed CAPTURE 0920-0950 \u{2192} 0920-0940 (20m, \u{2212}10m) · 2026/20260928.md line 5",
            "  1 [*] \u{2192} [/] Add support for `=x` syntax! bob.md ^capture-stop +2 Work Log",
            "      *2026-09-28* — Designed the `=x` grammar",
            "      *2026-09-28* — Wrote the plan",
            "  2 [*] \u{2192} [x] Add capture support for web URLs! bob.md ^web-capture",
            "    [x] Restart axe sase.md ^axe-restart +1 Work Log",
            "      *2026-09-28* — Restarted axe",
            "  next: CAPTURE (created) at line 14 · carries 1 link",
            "plan 2/3 themes · 2/10 links",
        ]
    );

    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-hd");
    let dry = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("=x1!2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("dry human");
    assert_success(&dry);
    assert_stdout_has_no_ansi(&dry);
    let out = stdout(&dry);
    assert!(out.contains("would close CAPTURE 0920-0950"), "{out}");
    assert!(
        out.contains(
            "  1 [*] \u{2192} [/] Add support for `=x` syntax! bob.md ^capture-stop +2 Work Log"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "  2 [*] \u{2192} [x] Add capture support for web URLs! bob.md ^web-capture"
        ),
        "{out}"
    );
    assert!(out.contains("carries 1 link"), "{out}");
    assert!(!out.contains("carries 1 links"), "{out}");
}

#[test]
fn capture_pomodoro_close_selection_dry_run_matches_real_run() {
    // A mixed wildcard preview is identical to its real result apart from
    // `dry_run`, and writes nothing.
    let (_temp, vault, day_file) = drop_worked_vault("bob-cli-close-sel-dr");
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
        .arg("=*!2")
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
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=*!2"]);
    assert_eq!(real_json["pomodoro_close"]["park_all"], true);
    assert_eq!(
        real_json["pomodoro_close"]["park"],
        serde_json::json!([1, 3])
    );
    assert_eq!(
        real_json["pomodoro_close"]["complete"],
        serde_json::json!([2])
    );
    dry_json["dry_run"] = serde_json::json!(false);
    real_json["dry_run"] = serde_json::json!(false);
    assert_eq!(dry_json, real_json);
    let day_after = fs::read_to_string(&day_file).expect("read day after");
    assert_pomodoro_blocks_cover_changes(&before_day, &day_after, &real_json);
}

#[test]
fn capture_pomodoro_close_selection_link_forms() {
    // `^bob:ready=x3`: the link step appends Ready as number 3, then the
    // selection defers 1 and 2 (unlisted) and works 3 (listed).
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-l3");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:ready=x3"],
    );
    assert_eq!(json["kind"], "pomodoro_link");
    assert_eq!(json["pomodoro_link_action"], "linked");
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x3");
    assert_eq!(close["in_progress"], serde_json::json!([3]));
    assert_eq!(close["complete"], serde_json::json!([]));
    assert_eq!(
        close["task_links"],
        serde_json::json!([
            {
                "index": 1,
                "ledger_line": 6,
                "block_link": "[[bob#^capture-stop]]",
                "block_id": "capture-stop",
                "marker": "plain",
                "outcome": "deferred",
                "source": "unlisted"
            },
            {
                "index": 2,
                "ledger_line": 10,
                "block_link": "[[bob#^web-capture]]",
                "block_id": "web-capture",
                "marker": "deferred",
                "outcome": "deferred",
                "source": "unlisted"
            },
            {
                "index": 3,
                "ledger_line": 14,
                "block_link": "[[bob#^ready]]",
                "block_id": "ready",
                "marker": "plain",
                "outcome": "in_progress",
                "source": "listed"
            }
        ])
    );
    assert_eq!(
        close["carried"],
        serde_json::json!([
            {"kind": "worked", "text": "[[bob#^ready]]"},
            {"kind": "deferred", "text": "[[bob#^capture-stop]]"},
            {"kind": "deferred", "text": "[[bob#^web-capture]]"}
        ])
    );
    assert_eq!(close["next_pomodoro"]["line"], 13);
    assert_eq!(close["next_pomodoro"]["created"], true);
    let task_by_id = |id: &str| {
        close["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["block_id"] == id)
            .unwrap_or_else(|| panic!("missing task {id}"))
            .clone()
    };
    assert_eq!(task_by_id("capture-stop")["index"], 1);
    assert_eq!(task_by_id("capture-stop")["role"], "deferred");
    assert_eq!(task_by_id("capture-stop")["status_symbol"], "*");
    assert_eq!(task_by_id("web-capture")["index"], 2);
    assert_eq!(task_by_id("web-capture")["role"], "deferred");
    assert_eq!(task_by_id("web-capture")["status_symbol"], "*");
    assert_eq!(task_by_id("ready")["index"], 3);
    assert_eq!(task_by_id("ready")["role"], "worked");
    assert_eq!(task_by_id("ready")["status_symbol"], "/");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        format!(
            concat!(
                "{}",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
                "\t\t- Designed the `=x` grammar\n",
                "\t\t\t- chose `x` for done\n",
                "\t\t- Wrote the plan\n",
                "\t- ~~[[sase#^axe-restart]]~~\n",
                "\t\t- Restarted axe\n",
                "\t- quick note\n",
                "\t- \u{1F345} [[bob#^ready]]\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^ready]]\n",
                "\t- [[bob#^capture-stop]]\n",
                "\t- [[bob#^web-capture]]\n",
                "{}",
            ),
            worked_prefix(),
            worked_suffix(),
        )
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert!(bob_after.contains(
        "- [/] #task Plain ready task [fresh:: 2026-09-28] [created::2026-09-20] ^ready\n"
    ));
    assert!(bob_after.contains(
        "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n"
    ));
    assert!(bob_after.contains(
        "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n"
    ));
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("sase"),
        plain_sase_after(),
    );

    // `^bob:capture-stop=x!1`: already current, completing it matches the
    // `=x!1` post-images byte for byte.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-lc");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^bob:capture-stop=x!1"],
    );
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert_eq!(json["pomodoro_close"]["raw"], "=x!1");
    assert_eq!(json["pomodoro_close"]["complete"], serde_json::json!([1]));
    let day_after = fs::read_to_string(&day_file).expect("read day");
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    let sase_after = fs::read_to_string(vault.join("sase.md")).expect("sase");
    let (_temp, plain_vault, plain_day) =
        close_worked_vault("bob-cli-close-sel-lc-once");
    run_close_json(&plain_vault, &plain_day, "2026-09-28 09:37:00", &["=x!1"]);
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

    // `Draft docs @bob:draft-docs=x0`: the new task lands last as number 3
    // and is deferred with the rest.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-nt");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["Draft docs @bob:draft-docs=x0"],
    );
    assert_eq!(json["kind"], "pomodoro_task");
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x0");
    assert_eq!(close["in_progress"], serde_json::json!([]));
    let task_by_id = |id: &str| {
        close["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["block_id"] == id)
            .unwrap_or_else(|| panic!("missing task {id}"))
            .clone()
    };
    assert_eq!(task_by_id("capture-stop")["index"], 1);
    assert_eq!(task_by_id("web-capture")["index"], 2);
    assert_eq!(task_by_id("draft-docs")["index"], 3);
    assert_eq!(task_by_id("draft-docs")["role"], "deferred");
    assert_eq!(close["task_links"].as_array().unwrap().len(), 3);
    assert_eq!(close["task_links"][2]["index"], 3);
    assert_eq!(close["task_links"][2]["block_id"], "draft-docs");
    assert_eq!(close["task_links"][2]["outcome"], "deferred");
    assert_eq!(close["task_links"][2]["source"], "unlisted");
    assert_eq!(
        close["carried"],
        serde_json::json!([
            {"kind": "deferred", "text": "[[bob#^capture-stop]]"},
            {"kind": "deferred", "text": "[[bob#^web-capture]]"},
            {"kind": "deferred", "text": "[[bob#^draft-docs]]"}
        ])
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        format!(
            concat!(
                "{}",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n",
                "\t\t- Designed the `=x` grammar\n",
                "\t\t\t- chose `x` for done\n",
                "\t\t- Wrote the plan\n",
                "\t- ~~[[sase#^axe-restart]]~~\n",
                "\t\t- Restarted axe\n",
                "\t- quick note\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^capture-stop]]\n",
                "\t- [[bob#^web-capture]]\n",
                "\t- [[bob#^draft-docs]]\n",
                "{}",
            ),
            worked_prefix(),
            worked_suffix(),
        )
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    // The new task is created Next and deferred, so it is never started.
    assert!(bob_after.contains(
        "- [*] #task Draft docs [created::2026-09-28] ^draft-docs\n"
    ));
    assert!(bob_after.contains(
        "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop\n"
    ));
    assert!(bob_after.contains(
        "- [*] #task Add capture support for web URLs! [created::2026-09-21] ^web-capture\n"
    ));
}

#[test]
fn capture_pomodoro_close_selection_batches() {
    // `-2`, blank, `=x1!2` decrements once, then closes with the selection.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-b1");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-2\n\n=x1!2")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("batch");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_close");
    assert_eq!(json["captures"][1]["pomodoro_close"]["raw"], "=x1!2");
    assert_eq!(
        json["captures"][1]["pomodoro_close"]["in_progress"],
        serde_json::json!([1])
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("(**0920-0940** [t:: 20m])"),
        "{day_after}"
    );
    assert!(
        day_after.contains("\t- ~~[[bob#^web-capture]]~~"),
        "{day_after}"
    );
    assert!(
        fs::read_to_string(vault.join("bob.md"))
            .expect("bob")
            .contains("[completion:: 2026-09-28] ^web-capture"),
        "batch completes web-capture"
    );

    // `=x0`, blank, `^sase:recovery-panel=` closes, then switches sessions.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-b2");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("=x0\n\n^sase:recovery-panel=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("switch");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["captures"][0]["kind"], "pomodoro_close");
    assert_eq!(json["captures"][0]["pomodoro_close"]["raw"], "=x0");
    assert_eq!(json["captures"][1]["kind"], "pomodoro_link");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("(**0920-0940** [t:: 20m]) — CAPTURE"),
        "{day_after}"
    );
    assert!(
        fs::read_to_string(vault.join("bob.md"))
            .expect("bob")
            .contains("^capture-stop"),
        "batch keeps bob tasks"
    );

    // `-2`, blank, `=x5` fails out of range and rolls back the `-2`.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-b3");
    let before = fs::read_to_string(&day_file).expect("read");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-2\n\n=x5")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("rollback");
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains(
            "`=x5` names task 5, but CAPTURE has 2 numbered Task Links"
        ),
        "{}",
        format_output(&output)
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
}

#[test]
fn capture_pomodoro_close_selection_diagnostics() {
    // Out of range writes nothing.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-o3");
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x3"],
    );
    assert!(
        error.contains(
            "`=x3` names task 3, but CAPTURE has 2 numbered Task Links (1–2)"
        ),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // With no numbered Task Links the error teaches plain `=x`.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-o0");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n  - quick note\n",
    );
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1"],
    );
    assert!(
        error.contains(
            "`=x1` names task 1, but CAPTURE has no numbered Task Links; close it with `=x`"
        ),
        "{error}"
    );

    // Conflicting duplicates fail; same-outcome duplicates pass.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-dup");
    let contents = fs::read_to_string(&day_file).expect("read");
    write_file(
        &day_file,
        &contents.replace(
            "\t- [[bob#^web-capture]]#\n",
            "\t- [[bob#^capture-stop]]\n",
        ),
    );
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1!2"],
    );
    assert!(
        error.contains(
            "tasks 1 and 2 both link `[[bob#^capture-stop]]` but get different outcomes; give them the same one"
        ),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x1,2"]);
    assert_eq!(json["pomodoro_close"]["raw"], "=x1,2");

    // A listed Blocked task warns instead of starting or completing.
    for (args, phrase) in [
        (vec!["=x1"], "so it was not started"),
        (vec!["=x!1"], "so it was not completed"),
    ] {
        let (_temp, vault, day_file) =
            close_worked_vault("bob-cli-close-sel-blocked");
        let bob = fs::read_to_string(vault.join("bob.md")).expect("bob");
        write_file(
            &vault.join("bob.md"),
            &bob.replace(
                "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop",
                "- [?] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop",
            ),
        );
        let json =
            run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &args);
        let warning =
            format!("task 1 `[[bob#^capture-stop]]` is Blocked, {phrase}");
        assert!(
            json["warnings"]
                .as_array()
                .expect("warnings")
                .iter()
                .any(|entry| entry.as_str() == Some(&warning)),
            "{args:?}: {}",
            json["warnings"]
        );
        assert_eq!(json["pomodoro_close"]["tasks"][0]["warning"], warning);
        assert_eq!(json["pomodoro_close"]["tasks"][0]["status_symbol"], "?");
    }

    // Every lexical row surfaces through `bob capture` without writing.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-lex");
    let before = fs::read_to_string(&day_file).expect("read");
    for (args, phrase) in [
        (vec!["=x1,1"], "task 1 is listed twice in `=x1,1`"),
        (
            vec!["=x1!1"],
            "task 1 cannot both stay in progress and complete in `=x1!1`",
        ),
        (
            vec!["=x0,2"],
            "`0` means no task stays in progress; use it alone, as `=x0`, `=x0*2`, `=x0!2`, or `=x0~2`",
        ),
        (
            vec!["=x0,"],
            "`0` means no task stays in progress; use it alone, as `=x0`, `=x0*2`, `=x0!2`, or `=x0~2`",
        ),
        (
            vec!["=x00"],
            "`0` means no task stays in progress; use it alone, as `=x0`, `=x0*2`, `=x0!2`, or `=x0~2`",
        ),
        (vec!["=x!0"], "task numbers start at 1"),
        (vec!["=x~0"], "task numbers start at 1"),
        (vec!["=x,1"], "expected a task number before `,`"),
        (vec!["=x1,,"], "expected a task number before `,`"),
        (vec!["=x1!2,,"], "expected a task number before `,`"),
        (vec!["=x1,!2"], "expected a task number after `,`"),
        (vec!["=x1!2!3"], "use one `!` list: `=x1!2,3`"),
        (vec!["=x1~2~3"], "use one `~` list: `=x1~2,3`"),
        (
            vec!["=x1~1"],
            "task 1 cannot both stay in progress and drop in `=x1~1`",
        ),
        (
            vec!["=x!2~2"],
            "task 2 cannot both complete and drop in `=x!2~2`",
        ),
        (
            vec!["=x1a"],
            "`=x1a` is not a task list: write `=x`, then comma-separated task numbers",
        ),
        (vec!["=x99999999999"], "task number 99999999999 is too large"),
        (
            vec!["=x 1,3"],
            "with no spaces (`=x1,3`)",
        ),
    ] {
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &args,
        );
        assert!(error.contains(phrase), "{args:?}: {error}");
    }
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // A dangling separator stays an incomplete editing state.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1,"],
    );
    assert!(
        error.contains("`=x1,` is incomplete: type a task number after `,`"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    let tilde = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1~"],
    );
    assert!(
        tilde.contains("`=x1~` is incomplete: type a task number after `~`"),
        "{tilde}"
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // A lone number on the close line is the dangling inline entry, never
    // the no-spaces hint (`=x 1` wants text, `=x1` keeps only task 1).
    for (args, phrase) in [
        ("=x 2", "type the Work Log text after task 2"),
        ("=x1 1", "type the Work Log text after task 1"),
        ("=x1,3 3", "type the Work Log text after task 3"),
    ] {
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[args],
        );
        assert!(error.contains(phrase), "{args}: {error}");
    }
    let dangling_plain = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x 2"],
    );
    assert!(dangling_plain.contains("`=x2`"), "{dangling_plain}");
    // A dangling Work Log bullet is an incomplete editing state.
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x\n- 1"],
    );
    assert!(
        error.contains(
            "`- 1` is incomplete: type the Work Log text after task 1"
        ),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // A `^` link close ending in `!` is a complete-all wildcard, so it
    // lexes as a close (execution then fails on the fixture block ID, not
    // on a dangling separator). A trailing `~` stays incomplete.
    for args in ["^r:id=x~", "^r:id=x1,"] {
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[args],
        );
        assert!(
            error.contains("is incomplete: type a task number after"),
            "{args}: {error}"
        );
    }
    // `^r:id!` without `=` is still the toggle error.
    let toggle = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["^r:id!"],
    );
    assert!(toggle.contains("toggle"), "{toggle}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Invalid link closes with conflicts report the conflict like the editor.
    for (args, phrase) in [
        (
            "^r:id=x1,1 s:2",
            "Pomodoro link capture cannot be combined with s:<N>",
        ),
        (
            "^r:id=x1, s:2",
            "Pomodoro link capture cannot be combined with s:<N>",
        ),
    ] {
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[args],
        );
        assert!(error.contains(phrase), "{args}: {error}");
    }
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
}

#[test]
fn capture_pomodoro_close_selection_link_and_new_task_wildcards() {
    // Existing @ and ^ links are appended to the staged session before the
    // wildcard expands; the newly linked row is therefore selected too.
    for link in ["@bob:ready=*", "^bob:ready=*"] {
        let (_temp, vault, day_file) =
            close_worked_vault("bob-cli-close-link-wildcard");
        let json =
            run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &[link]);
        assert_eq!(json["kind"], "pomodoro_link", "{link}");
        let close = &json["pomodoro_close"];
        assert_eq!(close["park_all"], true, "{link}");
        assert_eq!(close["park"], serde_json::json!([1, 2, 3]), "{link}");
        assert_eq!(close["task_links"].as_array().unwrap().len(), 3, "{link}");
        assert_eq!(close["task_links"][2]["block_id"], "ready", "{link}");
        assert_eq!(close["task_links"][2]["outcome"], "parked", "{link}");
        assert_eq!(close["task_links"][2]["source"], "listed", "{link}");
        assert_eq!(close["tasks"][0]["role"], "worked", "{link}");
        assert_eq!(close["tasks"][0]["status_symbol"], "/", "{link}");
        assert_eq!(close["tasks"][0]["carried"], false, "{link}");
    }

    // A body-bearing capture creates its task in the staged lineup, then
    // applies the same all-links outcome to both old links and the new row.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-new-task-wildcard");
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["Draft docs @bob:draft-docs=x*"],
    );
    assert_eq!(json["kind"], "pomodoro_task");
    let close = &json["pomodoro_close"];
    assert_eq!(close["park_all"], true);
    assert_eq!(close["park"], serde_json::json!([1, 2, 3]));
    assert_eq!(close["task_links"][2]["block_id"], "draft-docs");
    assert_eq!(close["task_links"][2]["outcome"], "parked");
    assert_eq!(close["task_links"][2]["source"], "listed");
    let created = close["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["block_id"] == "draft-docs")
        .unwrap();
    assert_eq!(created["role"], "worked");
    assert_eq!(created["status_symbol"], "/");
    assert_eq!(created["carried"], false);
}

#[test]
fn capture_pomodoro_close_selection_land_fixes() {
    // Mentioned-first then bare link to a Blocked task: the row carries
    // index 1 and the not-started warning.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-land-1");
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
        "\t- see [[bob#^capture-stop]] for context\n",
        "\t- [[bob#^capture-stop]]\n",
        "\t- [[bob#^web-capture]]#\n",
    );
    write_file(&day_file, day);
    let bob = fs::read_to_string(vault.join("bob.md")).expect("bob");
    write_file(
        &vault.join("bob.md"),
        &bob.replace(
            "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop",
            "- [?] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop",
        ),
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x1"]);
    assert_eq!(json["pomodoro_close"]["tasks"][0]["index"], 1);
    assert_eq!(
        json["pomodoro_close"]["tasks"][0]["warning"],
        "task 1 `[[bob#^capture-stop]]` is Blocked, so it was not started"
    );

    // `![[T]]` / `[[T]]` duplicate with `=x!2` on Blocked warns not completed.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-land-2");
    let day = concat!(
        "## Pomodoros\n",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
        "\t- ![[bob#^capture-stop]]\n",
        "\t- [[bob#^capture-stop]]\n",
    );
    write_file(&day_file, day);
    let bob = fs::read_to_string(vault.join("bob.md")).expect("bob");
    write_file(
        &vault.join("bob.md"),
        &bob.replace(
            "- [*] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop",
            "- [?] #task Add support for `=x` syntax! [created::2026-09-26] ^capture-stop",
        ),
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x!2"]);
    assert_eq!(json["pomodoro_close"]["tasks"][0]["index"], 1);
    assert!(
        json["pomodoro_close"]["tasks"][0]["warning"]
            .as_str()
            .expect("warning")
            .contains("so it was not completed"),
        "{}",
        json["pomodoro_close"]["tasks"][0]["warning"]
    );

    // Plain `=x` with no numbered rows is byte-identical to pre-epic output;
    // a fixture with no bare Task Links at all has an empty lineup.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-land-3");
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n\t- quick note\n",
    );
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x"]);
    assert_eq!(json["pomodoro_close"]["task_links"], serde_json::json!([]));
    assert!(json["pomodoro_close"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|task| task["index"].is_null()));
}

#[test]
fn capture_pomodoro_close_selection_crlf_day_file() {
    // A CRLF ledger closes with the selection and stays CRLF.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-close-sel-crlf");
    let contents = fs::read_to_string(&day_file).expect("read");
    write_file(&day_file, &contents.replace('\n', "\r\n"));
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x1!2"]);
    assert_eq!(json["pomodoro_close"]["raw"], "=x1!2");
    assert_eq!(json["pomodoro_close"]["tasks"][0]["status_symbol"], "/");
    assert_eq!(json["pomodoro_close"]["tasks"][1]["status_symbol"], "x");
    let day_after = fs::read_to_string(&day_file).expect("read");
    assert!(day_after.contains("\r\n"));
    assert!(!day_after.replace("\r\n", "").contains('\n'));
    assert!(day_after.contains("(**0920-0940** [t:: 20m])"));
    assert!(day_after.contains("\t- \u{1F345} [[bob#^capture-stop]]\r\n"));
}

fn drop_worked_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    // The worked fixture plus a third numbered link (`^ready`) so
    // `=x1!2~3` exercises all three close forms at once.
    let (temp, vault, day_file) = close_worked_vault(name);
    let day = fs::read_to_string(&day_file).expect("read day");
    write_file(
        &day_file,
        &day.replace(
            "\t- [[bob#^web-capture]]#\n",
            "\t- [[bob#^web-capture]]#\n\t- [[bob#^ready]]\n",
        ),
    );
    let bob = fs::read_to_string(vault.join("bob.md")).expect("read bob");
    write_file(
        &vault.join("bob.md"),
        &bob.replace(
            "- [ ] #task Plain ready task [created::2026-09-20] ^ready",
            "- [ ] #task Plain ready task #now [created::2026-09-20] ^ready",
        ),
    );
    (temp, vault, day_file)
}

fn empty_close_vault(name: &str) -> (TempDir, PathBuf, PathBuf) {
    let (temp, vault, day_file) = close_worked_vault(name);
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n\t- quick note\n",
    );
    (temp, vault, day_file)
}

#[test]
fn capture_pomodoro_close_selection_drop() {
    // `=x1!2~3`: task 1 in progress, task 2 complete, task 3 dropped.
    // The dropped link leaves the closed session, is not carried, and its
    // Ready task is untouched (stays `[ ]`, no Work Log).
    let (_temp, vault, day_file) = drop_worked_vault("bob-cli-close-sel-drop");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x1!2~3"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x1!2~3");
    assert_eq!(close["in_progress"], serde_json::json!([1]));
    assert_eq!(close["complete"], serde_json::json!([2]));
    assert_eq!(close["drop"], serde_json::json!([3]));
    assert_eq!(
        close["task_links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| (
                link["index"].clone(),
                link["outcome"].clone(),
                link["source"].clone()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                serde_json::json!(1),
                serde_json::json!("in_progress"),
                serde_json::json!("listed")
            ),
            (
                serde_json::json!(2),
                serde_json::json!("complete"),
                serde_json::json!("listed")
            ),
            (
                serde_json::json!(3),
                serde_json::json!("dropped"),
                serde_json::json!("listed")
            ),
        ]
    );
    let dropped = close["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["block_id"] == "ready")
        .expect("dropped task row");
    assert_eq!(dropped["role"], "dropped");
    assert_eq!(dropped["index"], 3);
    assert!(dropped.get("now").is_none());
    assert_eq!(dropped["carried"], false);
    assert_eq!(dropped["status_symbol"], " ");
    assert_eq!(dropped["status_changed"], false);
    assert_eq!(dropped["work_log"], serde_json::json!([]));
    assert!(dropped["warning"].is_null());
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(!day_after.contains("\t- ~[["), "{day_after}");
    assert!(!day_after.contains("[[bob#^ready]]"), "{day_after}");
    assert!(
        day_after.contains("~~[[bob#^web-capture]]~~"),
        "{day_after}"
    );
    assert!(
        day_after.contains("- [ ] () — CAPTURE\n\t- [[bob#^capture-stop]]\n"),
        "{day_after}"
    );
    let bob_after = fs::read_to_string(vault.join("bob.md")).expect("bob");
    assert!(
        bob_after.contains(
            "- [ ] #task Plain ready task #now [created::2026-09-20] ^ready\n"
        ),
        "{bob_after}"
    );
    assert!(
        bob_after.contains("[completion:: 2026-09-28] ^web-capture"),
        "{bob_after}"
    );
}

#[test]
fn capture_pomodoro_close_selection_drop_human_output() {
    // Dropped rows read `dropped <K> [[T]]` (with a `stays <status>`
    // lane caption) plus a `Dropped <K>` summary.
    let (_temp, vault, day_file) =
        drop_worked_vault("bob-cli-close-sel-drop-hu");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("=x1!2~3")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("human drop close");
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(out.contains("closed CAPTURE 0920-0950"), "{out}");
    assert!(
        out.contains("dropped 3 [[bob#^ready]] · stays Ready"),
        "{out}"
    );
    assert!(out.contains("Dropped 3"), "{out}");
}

#[test]
fn capture_pomodoro_close_selection_drop_only_forms() {
    // `=x~2` drops with unlisted links at their ledger outcome;
    // `=x0~2` drops while deferring everything else.
    let (_temp, vault, day_file) =
        drop_worked_vault("bob-cli-close-sel-drop-only");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x~2"]);
    let close = &json["pomodoro_close"];
    assert!(close["in_progress"].is_null());
    assert_eq!(close["complete"], serde_json::json!([]));
    assert_eq!(close["drop"], serde_json::json!([2]));
    assert_eq!(close["task_links"][0]["outcome"], "in_progress");
    assert_eq!(close["task_links"][0]["source"], "ledger");
    assert_eq!(close["task_links"][1]["outcome"], "dropped");

    let (_temp, vault, day_file) =
        drop_worked_vault("bob-cli-close-sel-drop-none");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x0~2"]);
    let close = &json["pomodoro_close"];
    assert_eq!(close["in_progress"], serde_json::json!([]));
    assert_eq!(close["drop"], serde_json::json!([2]));
    assert_eq!(close["task_links"][0]["outcome"], "deferred");
    assert_eq!(close["task_links"][0]["source"], "unlisted");
    assert_eq!(close["task_links"][1]["outcome"], "dropped");
    assert_eq!(close["task_links"][2]["outcome"], "deferred");
}

#[test]
fn capture_pomodoro_close_selection_park() {
    // `=x1*2,3!4,5`: task 1 continues, tasks 2-3 park (worked but not
    // carried), tasks 4-5 complete. Only link 1 is carried.
    use crate::support::TempDir;
    let temp = TempDir::new("bob-cli-close-sel-park");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20260928.md");
    crate::support::write_toggle_task_settings(&vault);
    crate::support::write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
            "\t- [[tasks#^t1]]\n",
            "\t- [[tasks#^t2]]\n",
            "\t- [[tasks#^t3]]\n",
            "\t- [[tasks#^t4]]\n",
            "\t- [[tasks#^t5]]\n",
        ),
    );
    crate::support::write_file(
        &vault.join("tasks.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [*] #task T1 [created::2026-09-20] ^t1\n",
            "- [*] #task T2 [created::2026-09-20] ^t2\n",
            "- [*] #task T3 [created::2026-09-20] ^t3\n",
            "- [*] #task T4 [created::2026-09-20] ^t4\n",
            "- [*] #task T5 [created::2026-09-20] ^t5\n",
        ),
    );
    let json = run_close_json(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=x1*2,3!4,5"],
    );
    let close = &json["pomodoro_close"];
    assert_eq!(close["raw"], "=x1*2,3!4,5");
    assert_eq!(close["in_progress"], serde_json::json!([1]));
    assert_eq!(close["park"], serde_json::json!([2, 3]));
    assert_eq!(close["complete"], serde_json::json!([4, 5]));
    let outcomes: Vec<(u32, &str, &str)> = close["task_links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|link| {
            (
                link["index"].as_u64().unwrap() as u32,
                link["outcome"].as_str().unwrap(),
                link["source"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        vec![
            (1, "in_progress", "listed"),
            (2, "parked", "listed"),
            (3, "parked", "listed"),
            (4, "complete", "listed"),
            (5, "complete", "listed"),
        ]
    );
    // Parked rows stay worked with real transitions and no carry.
    for block in ["t1", "t2", "t3"] {
        let row = close["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["block_id"] == block)
            .expect("parked/continued row");
        assert_eq!(row["role"], "worked");
        assert_eq!(row["status_symbol"], "/");
        assert_eq!(row["status_changed"], true);
    }
    assert_eq!(close["tasks"][1]["carried"], false);
    assert_eq!(close["tasks"][2]["carried"], false);
    assert_eq!(close["tasks"][0]["carried"], true);
    assert_eq!(
        close["carried"],
        serde_json::json!([{"kind": "worked", "text": "[[tasks#^t1]]"}])
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(day_after.contains("\t- 🍅 [[tasks#^t1]]"), "{day_after}");
    assert!(day_after.contains("\t- 🍅 [[tasks#^t2]]"), "{day_after}");
    assert!(day_after.contains("\t- 🍅 [[tasks#^t3]]"), "{day_after}");
    assert!(
        day_after.contains("- [ ] () — CAPTURE\n\t- [[tasks#^t1]]\n"),
        "{day_after}"
    );
    assert!(
        !day_after.contains("[[tasks#^t2]]\n\t- [[tasks#^t2]]"),
        "{day_after}"
    );
    // Star-only defers unlisted plain links like ordinary closes.
    let (_temp, vault, day_file) =
        drop_worked_vault("bob-cli-close-sel-park-star-only");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x*2"]);
    let close = &json["pomodoro_close"];
    assert!(close["in_progress"].is_null());
    assert_eq!(close["park"], serde_json::json!([2]));
    assert_eq!(close["task_links"][0]["outcome"], "deferred");
    assert_eq!(close["task_links"][0]["source"], "unlisted");
    assert_eq!(close["task_links"][1]["outcome"], "parked");
}

#[test]
fn capture_pomodoro_close_short_alias_equivalence() {
    // Short aliases omit `x` before an initial `*`/`!`; both spellings
    // preserve the same wildcard or explicit scope against the same lineup.
    for (alias, long_form, three_link) in [
        ("=*", "=x*", true),
        ("=*1", "=x*1", false),
        ("=*!2", "=x*!2", true),
        ("=*!2", "=x!2*", true),
        ("=!", "=x!", true),
        ("=!1", "=x!1", false),
        ("=!~2", "=x!~2", true),
        ("=*2,3", "=x*2,3", true),
        ("=!2,3", "=x!2,3", true),
    ] {
        let (_temp_a, vault_a, day_a) = if three_link {
            drop_worked_vault("bob-cli-close-alias-a")
        } else {
            close_worked_vault("bob-cli-close-alias-a")
        };
        let (_temp_b, vault_b, day_b) = if three_link {
            drop_worked_vault("bob-cli-close-alias-b")
        } else {
            close_worked_vault("bob-cli-close-alias-b")
        };
        let json_a =
            run_close_json(&vault_a, &day_a, "2026-09-28 09:37:00", &[alias]);
        let json_b = run_close_json(
            &vault_b,
            &day_b,
            "2026-09-28 09:37:00",
            &[long_form],
        );
        assert_eq!(json_a["ok"], true, "{alias}");
        assert_eq!(json_b["ok"], true, "{long_form}");
        assert_eq!(json_a["pomodoro_close"]["raw"], alias, "{alias}");
        assert_eq!(json_b["pomodoro_close"]["raw"], long_form, "{long_form}");
        for field in [
            "in_progress",
            "park",
            "complete",
            "drop",
            "tasks",
            "task_links",
            "carried",
        ] {
            assert_eq!(
                json_a["pomodoro_close"][field],
                json_b["pomodoro_close"][field],
                "{alias} vs {long_form}: {field}"
            );
        }
        for field in ["park_all", "complete_all"] {
            assert_eq!(
                json_a["pomodoro_close"][field].as_bool().unwrap_or(false),
                json_b["pomodoro_close"][field].as_bool().unwrap_or(false),
                "{alias} vs {long_form}: {field}"
            );
        }
        if alias == "=*" {
            assert_eq!(
                json_a["pomodoro_close"]["park"],
                serde_json::json!([1, 2, 3])
            );
            assert_eq!(json_a["pomodoro_close"]["park_all"], true);
            assert!(json_a["pomodoro_close"]["task_links"]
                .as_array()
                .unwrap()
                .iter()
                .all(|link| link["outcome"] == "parked"
                    && link["source"] == "listed"));
        }
        if alias == "=!" {
            assert_eq!(
                json_a["pomodoro_close"]["complete"],
                serde_json::json!([1, 2, 3])
            );
            assert_eq!(json_a["pomodoro_close"]["complete_all"], true);
            assert!(json_a["pomodoro_close"]["task_links"]
                .as_array()
                .unwrap()
                .iter()
                .all(|link| link["outcome"] == "complete"
                    && link["source"] == "listed"));
        }
        let day_a_after = fs::read_to_string(&day_a).expect("read day");
        let day_b_after = fs::read_to_string(&day_b).expect("read day");
        assert_eq!(day_a_after, day_b_after, "{alias} vs {long_form}: day");
        for file in ["bob.md", "sase.md"] {
            assert_eq!(
                fs::read_to_string(vault_a.join(file)).expect("read"),
                fs::read_to_string(vault_b.join(file)).expect("read"),
                "{alias} vs {long_form}: {file}"
            );
        }
    }

    // A wildcard fills the remaining lineup after explicit exceptions.
    let (_temp, vault, day_file) =
        drop_worked_vault("bob-cli-close-all-exceptions");
    let mixed =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x1*~2"]);
    let close = &mixed["pomodoro_close"];
    assert_eq!(close["in_progress"], serde_json::json!([1]));
    assert_eq!(close["park"], serde_json::json!([3]));
    assert_eq!(close["park_all"], true);
    assert_eq!(close["drop"], serde_json::json!([2]));
    assert_eq!(close["task_links"][0]["outcome"], "in_progress");
    assert_eq!(close["task_links"][1]["outcome"], "dropped");
    assert_eq!(close["task_links"][2]["outcome"], "parked");

    // Explicit overlaps still fail; two empty wildcards have their own
    // order-independent diagnostic. Neither failure writes the day note.
    for args in ["=x1*1", "=x1!1", "=*1!1", "=x*1!1", "=x*1~1", "=x!1~1"] {
        let (_temp, vault, day_file) =
            close_worked_vault("bob-cli-close-alias-explicit-overlap");
        let before = fs::read_to_string(&day_file).expect("read");
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[args],
        );
        assert!(error.contains("cannot both"), "{args}: {error}");
        assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    }
    for args in ["=*!", "=!*", "=x*!", "=x!*"] {
        let (_temp, vault, day_file) =
            close_worked_vault("bob-cli-close-competing-wildcards");
        let before = fs::read_to_string(&day_file).expect("read");
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[args],
        );
        assert!(
            error.contains("competing park-all and complete-all"),
            "{args}: {error}"
        );
        assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    }

    // Malformed aliases claim close syntax, never task creation.
    for (args, phrase) in [
        ("=*abc", "is not a task list"),
        ("=!0", "task numbers start at 1"),
        ("=**2", "use one `*` list"),
        ("=!1,,2", "expected a task number"),
        ("=*,2", "expected a task number"),
        ("=!,", "expected a task number"),
    ] {
        let (_temp, vault, day_file) =
            close_worked_vault("bob-cli-close-alias-bad");
        let before = fs::read_to_string(&day_file).expect("read");
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[args],
        );
        assert!(error.contains(phrase), "{args}: {error}");
        assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    }

    // Empty comma elements never default; trailing comma/~ dangle.
    for args in ["=*1,", "=!2,", "=*~"] {
        let (_temp, vault, day_file) =
            close_worked_vault("bob-cli-close-alias-dangle");
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[args],
        );
        assert!(error.contains("is incomplete"), "{args}: {error}");
    }

    // Empty sessions accept both wildcard outcomes; a positive explicit
    // index remains out of range. Wildcard Work Log text still fails atomically.
    for args in ["=*", "=!", "=x*", "=x!"] {
        let (_temp, vault, day_file) =
            empty_close_vault("bob-cli-close-alias-empty");
        let json =
            run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &[args]);
        assert_eq!(json["ok"], true, "{args}: {json}");
        assert_eq!(json["pomodoro_close"]["task_links"], serde_json::json!([]));
        assert_eq!(
            json["pomodoro_close"]["park_all"]
                .as_bool()
                .unwrap_or(false),
            args.contains('*'),
            "{args}"
        );
        assert_eq!(
            json["pomodoro_close"]["complete_all"]
                .as_bool()
                .unwrap_or(false),
            args.contains('!'),
            "{args}"
        );
    }
    for args in ["=*1", "=!1"] {
        let (_temp, vault, day_file) =
            empty_close_vault("bob-cli-close-alias-empty-explicit");
        let before = fs::read_to_string(&day_file).expect("read");
        let error = run_close_expect_error(
            &vault,
            &day_file,
            "2026-09-28 09:37:00",
            &[args],
        );
        assert!(error.contains(args), "{args}: {error}");
        assert!(error.contains("no numbered Task Links"), "{args}: {error}");
        assert_eq!(fs::read_to_string(&day_file).expect("read"), before);
    }
    let (_temp, vault, day_file) =
        empty_close_vault("bob-cli-close-alias-empty-log");
    let before = fs::read_to_string(&day_file).expect("read");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-09-28 09:37:00",
        &["=* log it"],
    );
    assert!(error.contains("no numbered Task Links"), "{error}");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // A batch whose later shorthand close fails rolls back the earlier edit.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-alias-batch");
    let before = fs::read_to_string(&day_file).expect("read");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("-2\n\n=*abc")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("batch rollback");
    assert!(!output.status.success());
    assert!(stdout(&output).contains("is not a task list"));
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before);

    // Dry-run changes no vault files.
    let (_temp, vault, day_file) =
        close_worked_vault("bob-cli-close-alias-dry");
    let before_day = fs::read_to_string(&day_file).expect("read");
    let before_bob = fs::read_to_string(vault.join("bob.md")).expect("read");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=*")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-28 09:37:00")
        .output()
        .expect("dry run");
    assert_success(&output);
    assert_eq!(fs::read_to_string(&day_file).expect("read"), before_day);
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("read"),
        before_bob
    );
}
