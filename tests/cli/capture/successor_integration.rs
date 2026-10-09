//! Successor integration with the newer capture grammar
//! (`plan:202610/successor_links_landing_repairs.md` §6): successor
//! completion combined with restart/swap/reset, later items consuming
//! the continuation's successor, and `#task #ref` successors.

use super::pomodoro_close::run_close_json;
use crate::support::*;
use std::fs;

const NOW: &str = "2026-10-05 09:30:00";

fn vault(name: &str) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    let day_file = vault.join("20261005.md");
    (temp, vault, day_file)
}

// A close that links a successor, followed in the same batch by an item
// completing that successor: the first row drops out (net) while the
// second item links what its own close unblocked. Final live-link and
// task truth agree with the committed bytes.
#[test]
fn close_successor_then_consume_net() {
    let (_temp, vault, day_file) = vault("bob-cli-successor-consume-net");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n- [?] #task E [dependsOn:: d] [id:: e] ^e\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^p]]\n- [ ] () \u{2014} FIX\n",
    );

    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW),
        "=!\n\n!sase:d\n",
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    // The consumed successor row is gone from the first item (net).
    assert!(
        captures[0]["pomodoro_close"]["unblocked"]
            .as_array()
            .expect("first unblocked")
            .is_empty(),
        "{}",
        captures[0]["pomodoro_close"]
    );
    let second = captures[1]["task_complete"]["unblocked"]
        .as_array()
        .expect("second unblocked");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0]["block_id"], "e");
    assert_eq!(second[0]["status_symbol"], "*");
    assert!(second[0]["link"].is_object());
    // Final truth: P and D done, E next, and the continuation holds E.
    let sase = fs::read_to_string(vault.join("sase.md")).expect("read note");
    assert!(sase.contains("- [x] #task P [id:: p]"), "{sase}");
    assert!(
        sase.contains("- [x] #task D [dependsOn:: p] [id:: d]"),
        "{sase}"
    );
    assert!(
        sase.contains("- [*] #task E [dependsOn:: d] [id:: e]"),
        "{sase}"
    );
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("[[sase#^e]]"), "{day}");
    assert!(!day.contains("[[sase#^d]]\n  - [[sase#^e]]"), "{day}");
    // The surviving successor keeps the unblocked task-block role; the
    // consumed one reports none.
    let task_blocks = json["task_blocks"].as_array().expect("task_blocks");
    assert!(
        task_blocks.iter().any(|block| block["block_id"] == "e"
            && block["roles"] == serde_json::json!(["unblocked"])),
        "{task_blocks:?}"
    );
    assert!(
        task_blocks
            .iter()
            .filter(|block| block["block_id"] == "d")
            .all(|block| block["roles"] != serde_json::json!(["unblocked"])),
        "{task_blocks:?}"
    );
}

// Restart and reset move the ledger without completing anything: open
// prerequisites stay open, dependents stay blocked, and no successor
// rows appear anywhere in the report.
#[test]
fn restart_and_reset_never_complete_prerequisites() {
    let setup = |name: &str| {
        let (_temp, vault, day_file) = vault(name);
        write_file(
            &vault.join("sase.md"),
            "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n",
        );
        write_file(
            &day_file,
            "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^p]]\n- [ ] () \u{2014} FIX\n",
        );
        (_temp, vault, day_file)
    };

    let (_temp, vault, day_file) = setup("bob-cli-successor-restart-preserve");
    let json = run_close_json(&vault, &day_file, NOW, &["=="]);
    assert_eq!(json["kind"], "pomodoro_start");
    assert_eq!(json["pomodoro_start"]["override"]["action"], "restart");
    assert!(json.get("task_complete").is_none(), "{json}");
    assert!(json.get("pomodoro_close").is_none(), "{json}");
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read note"),
        "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n"
    );

    let (_temp, vault, day_file) = setup("bob-cli-successor-reset-preserve");
    let json = run_close_json(&vault, &day_file, NOW, &["=x0"]);
    assert_eq!(json["kind"], "pomodoro_reset");
    assert!(json.get("task_complete").is_none(), "{json}");
    assert!(json.get("pomodoro_close").is_none(), "{json}");
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read note"),
        "- [ ] #task P [id:: p] ^p\n- [?] #task D [dependsOn:: p] [id:: d] ^d\n"
    );
}

// A normal `#task #ref` successor in a parent note is an ordinary task
// to the classifier: it links into the slot and goes Next.
#[test]
fn ref_task_successor_links_like_an_ordinary_task() {
    let (_temp, vault, day_file) = vault("bob-cli-successor-ref-task");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Predecessor [id:: p] ^p\n",
    );
    write_file(
        &vault.join("parent.md"),
        "---\ntype: [[area]]\n---\n- [?] #task #ref Read paper [dependsOn:: p] [id:: d] ^d\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} BOB\n  - [[sase#^p]]\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("!sase:p")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run bob capture json");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let unblocked = json["task_complete"]["unblocked"]
        .as_array()
        .expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    assert_eq!(unblocked[0]["note_path"], "parent.md");
    assert_eq!(unblocked[0]["block_id"], "d");
    assert_eq!(unblocked[0]["status_symbol"], "*");
    assert_eq!(unblocked[0]["link"]["block_link"], "[[parent#^d]]");
    assert_eq!(
        fs::read_to_string(vault.join("parent.md")).expect("read parent"),
        "---\ntype: [[area]]\n---\n- [*] #task #ref Read paper [dependsOn:: p] [id:: d] ^d\n"
    );
}
