//! Settle ended off-clock Event Tasks into closed off-clock MEETs.

use crate::support::*;
use std::fs;

fn write_vault(daily: &std::path::Path, contents: &str) {
    write_file(daily, contents);
}

fn settle_json(
    vault: &std::path::Path,
    daily: &std::path::Path,
    now: &str,
) -> serde_json::Value {
    let output = bob_command()
        .args(["task", "reconcile"])
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .env("BOB_NOW", now)
        .arg("-r")
        .arg("0")
        .output()
        .expect("run settle reconciliation");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("settle JSON")
}

#[test]
fn settle_single_ended_off_clock_event() {
    let temp = TempDir::new("bob-cli-settle-single");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_vault(
        &daily,
        concat!(
            "# Daily\n\n",
            "## Events\n\n",
            "- [*] #task **1200-1300** Lunch with Kelly #offclock [root:: love] [type:: MEET] ^lunch-with-kelly\n",
            "\n## Pomodoros\n\n",
            "- [ ] () — MEET\n",
            "  - [[#^lunch-with-kelly]]\n",
        ),
    );

    let dry = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_NOW", "2026-07-16 13:00")
        .arg("-r")
        .arg("0")
        .output()
        .expect("dry-run settle");
    assert_success(&dry);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("dry JSON");
    assert_eq!(dry_json["settled"].as_array().unwrap().len(), 1);
    assert_eq!(dry_json["settled"][0]["title"], "Lunch with Kelly");
    assert_eq!(dry_json["settled"][0]["span"], "1200-1300");
    assert_eq!(dry_json["settled"][0]["minutes"], 60);
    // Dry-run writes nothing.
    assert!(fs::read_to_string(&daily)
        .unwrap()
        .contains("- [ ] () — MEET"));

    let json = settle_json(&vault, &daily, "2026-07-16 13:00");
    let settled = json["settled"].as_array().unwrap();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0]["title"], "Lunch with Kelly");
    assert_eq!(settled[0]["line_number"], 5);
    assert_eq!(settled[0]["span"], "1200-1300");
    assert_eq!(settled[0]["block_id"], "lunch-with-kelly");
    assert_eq!(settled[0]["minutes"], 60);

    let after = fs::read_to_string(&daily).unwrap();
    assert!(
        after.contains("- [x] (**1200-1300** [off:: 60m]) — MEET"),
        "closed off-clock MEET:\n{after}"
    );
    assert!(
        after.contains("🍅 ~~[[#^lunch-with-kelly]]~~"),
        "struck link under closed MEET:\n{after}"
    );
    assert!(
        after.contains("- [x] #task **1200-1300** Lunch with Kelly"),
        "completed event row:\n{after}"
    );
    assert!(
        after.contains("[completion:: 2026-07-16]"),
        "done date:\n{after}"
    );
    assert!(
        !after.contains("- [ ] () — MEET"),
        "placeholder gone:\n{after}"
    );

    // Idempotent rerun settles nothing.
    let second = settle_json(&vault, &daily, "2026-07-16 13:00");
    assert_eq!(second["settled"].as_array().unwrap().len(), 0);
}

#[test]
fn settled_meet_children_follow_the_note_indent_unit() {
    for (label, indent) in [("tabs", "\t"), ("two-spaces", "  ")] {
        let temp = TempDir::new(&format!("bob-cli-settle-indent-{label}"));
        let vault = temp.path().join("vault");
        let daily = vault.join("2026/20260716.md");
        write_vault(
            &daily,
            &format!(
                concat!(
                    "# Daily\n\n",
                    "## Events\n\n",
                    "- [*] #task **1200-1300** Lunch #offclock [type:: MEET] ^lunch\n",
                    "\n## Pomodoros\n\n",
                    "- [ ] () — MEET\n",
                    "{indent}- [[#^lunch]]\n",
                ),
                indent = indent,
            ),
        );

        settle_json(&vault, &daily, "2026-07-16 13:00");
        let after = fs::read_to_string(&daily).expect("settled daily note");
        let expected = format!("{indent}- 🍅 ~~[[#^lunch]]~~");
        assert!(after.contains(&expected), "expected {expected:?}:\n{after}");
        if indent == "\t" {
            assert!(!after.contains("  - 🍅 ~~[[#^lunch]]~~"), "{after}");
        }
    }
}

#[test]
fn settle_back_to_back_cluster_and_trims_around_on_clock() {
    let temp = TempDir::new("bob-cli-settle-cluster-trim");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_vault(
        &daily,
        concat!(
            "# Daily\n\n",
            "## Events\n\n",
            "- [*] #task **1200-1230** A #offclock [type:: MEET] ^a\n",
            "- [*] #task **1230-1300** B #offclock [type:: MEET] ^b\n",
            "\n## Pomodoros\n\n",
            "- [ ] () — MEET\n",
            "  - [[#^a]]\n",
            "  - [[#^b]]\n",
            "- [x] (**1215-1245** [t:: 30m]) — BUILD\n",
            "  - [[tasks#^work]]\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Work ^work\n");

    let json = settle_json(&vault, &daily, "2026-07-16 14:00");
    let settled = json["settled"].as_array().unwrap();
    assert_eq!(settled.len(), 2);
    let after = fs::read_to_string(&daily).unwrap();
    assert!(
        after.contains("- [x] (**1200-1215** [off:: 15m]) — MEET"),
        "first trimmed piece:\n{after}"
    );
    assert!(
        after.contains("- [x] (**1245-1300** [off:: 15m]) — MEET"),
        "second trimmed piece:\n{after}"
    );
    // Each event links its overlapping piece.
    assert!(after.contains("~~[[#^a]]~~"), "A linked:\n{after}");
    assert!(after.contains("~~[[#^b]]~~"), "B linked:\n{after}");
}

#[test]
fn settle_fully_covered_cluster_does_nothing() {
    let temp = TempDir::new("bob-cli-settle-covered");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let before = concat!(
        "# Daily\n\n",
        "## Events\n\n",
        "- [*] #task **1200-1300** Lunch #offclock [type:: MEET] ^lunch\n",
        "\n## Pomodoros\n\n",
        "- [ ] () — MEET\n",
        "  - [[#^lunch]]\n",
        "- [x] (**1200-1300** [t:: 60m]) — BUILD\n",
        "  - [[tasks#^work]]\n",
    );
    write_vault(&daily, before);
    write_file(&vault.join("tasks.md"), "- [ ] #task Work ^work\n");

    let json = settle_json(&vault, &daily, "2026-07-16 13:00");
    assert_eq!(json["settled"].as_array().unwrap().len(), 0);
    let after = fs::read_to_string(&daily).unwrap();
    assert!(
        after.contains("- [ ] () — MEET"),
        "placeholder stays:\n{after}"
    );
    assert!(
        after.contains("- [*] #task **1200-1300** Lunch"),
        "event stays open:\n{after}"
    );
}

#[test]
fn settle_running_on_clock_session_trims_and_mixed_placeholder_keeps_future() {
    let temp = TempDir::new("bob-cli-settle-running-mixed");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_vault(
        &daily,
        concat!(
            "# Daily\n\n",
            "## Events\n\n",
            "- [*] #task **1100-1200** Lunch #offclock [type:: MEET] ^lunch\n",
            "- [*] #task **1400-1500** Later #offclock [type:: MEET] ^later\n",
            "\n## Pomodoros\n\n",
            "- [ ] () — MEET\n",
            "  - [[#^lunch]]\n",
            "  - [[#^later]]\n",
            "- [ ] (**1130-1230** [t:: 60m]) — BUILD\n",
            "  - [[tasks#^work]]\n",
        ),
    );
    write_file(&vault.join("tasks.md"), "- [ ] #task Work ^work\n");

    let json = settle_json(&vault, &daily, "2026-07-16 12:30");
    let settled = json["settled"].as_array().unwrap();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0]["title"], "Lunch");
    assert_eq!(settled[0]["minutes"], 30);
    let after = fs::read_to_string(&daily).unwrap();
    assert!(
        after.contains("- [x] (**1100-1130** [off:: 30m]) — MEET"),
        "trimmed around running session:\n{after}"
    );
    assert!(
        after.contains("- [ ] () — MEET\n  - [[#^later]]"),
        "future link stays under placeholder:\n{after}"
    );
    assert!(
        after.contains("- [*] #task **1400-1500** Later"),
        "future event stays open:\n{after}"
    );
}

#[test]
fn settle_inherited_off_clock_event_settles_once_and_on_stays_open() {
    let temp = TempDir::new("bob-cli-settle-inherited");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_file(
        &vault.join("love.md"),
        "---\ntype: [[area]]\noffclock: true\n---\n# Love\n",
    );
    write_file(
        &vault.join("abby.md"),
        "---\ntype: [[area]]\nparent: [[love]]\n---\n# Abby\n",
    );
    write_vault(
        &daily,
        concat!(
            "# Daily\n\n",
            "## Events\n\n",
            "- [*] #task **1200-1300** Inherited [root:: abby] [type:: MEET] ^inherited\n",
            "- [*] #task **1200-1300** Override #on [root:: abby] [type:: MEET] ^override\n",
            "\n## Pomodoros\n\n",
            "- [ ] () — MEET\n",
            "  - [[#^inherited]]\n",
            "  - [[#^override]]\n",
        ),
    );

    let json = settle_json(&vault, &daily, "2026-07-16 13:00");
    let settled = json["settled"].as_array().unwrap();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0]["block_id"], "inherited");
    let after = fs::read_to_string(&daily).unwrap();
    assert!(
        after.contains("- [x] (**1200-1300** [off:: 60m]) — MEET"),
        "inherited closed MEET:\n{after}"
    );
    assert!(
        after.contains("- [x] #task **1200-1300** Inherited"),
        "inherited event completed:\n{after}"
    );
    assert!(
        after.contains("- [*] #task **1200-1300** Override #on"),
        "override stays open:\n{after}"
    );
    let second = settle_json(&vault, &daily, "2026-07-16 13:00");
    assert_eq!(second["settled"].as_array().unwrap().len(), 0);
}

#[test]
fn settle_ignores_cancelled_done_unlinked_and_on_clock_events() {
    let temp = TempDir::new("bob-cli-settle-never");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_vault(
        &daily,
        concat!(
            "# Daily\n\n",
            "## Events\n\n",
            "- [-] #task **1200-1300** Cancelled #offclock [type:: MEET] ^cancelled\n",
            "- [x] #task **1200-1300** Done #offclock [type:: MEET] ^done\n",
            "- [*] #task **1200-1300** Work meeting [type:: MEET] ^workmeet\n",
            "- [*] #task **1200-1300** Unlinked #offclock [type:: MEET] ^unlinked\n",
            "\n## Pomodoros\n\n",
            "- [ ] () — MEET\n",
            "  - [[#^cancelled]]\n",
            "  - [[#^done]]\n",
            "  - [[#^workmeet]]\n",
        ),
    );

    let json = settle_json(&vault, &daily, "2026-07-16 13:00");
    assert_eq!(json["settled"].as_array().unwrap().len(), 0);
}

#[test]
fn settle_human_output_names_events() {
    let temp = TempDir::new("bob-cli-settle-human");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    write_vault(
        &daily,
        concat!(
            "# Daily\n\n",
            "## Events\n\n",
            "- [*] #task **1200-1300** Lunch with Kelly #offclock [type:: MEET] ^lunch\n",
            "\n## Pomodoros\n\n",
            "- [ ] () — MEET\n",
            "  - [[#^lunch]]\n",
        ),
    );
    let output = bob_command()
        .args(["task", "reconcile"])
        .arg("--dry-run")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_NOW", "2026-07-16 13:00")
        .arg("-r")
        .arg("0")
        .output()
        .expect("dry-run human settle");
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains(
            "◌ would settle Lunch with Kelly 1200-1300 off the clock"
        ),
        "dry-run human:\n{text}"
    );
}
