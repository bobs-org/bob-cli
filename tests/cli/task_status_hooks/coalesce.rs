//! Coalescing continuous same-type Pomodoro sessions through reconcile.

use crate::support::*;
use std::fs;

fn reconcile_json(
    vault: &std::path::Path,
    daily: &std::path::Path,
    dry_run: bool,
) -> serde_json::Value {
    let mut command = bob_command();
    command
        .args(["task", "reconcile"])
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(vault)
        .env("BOB_DAY_FILE", daily)
        .env("BOB_NOW", "2026-07-16 12:00")
        .arg("-r")
        .arg("0");
    if dry_run {
        command.arg("--dry-run");
    }
    let output = command.output().expect("run reconcile");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("reconcile JSON")
}

#[test]
fn coalescing_dry_run_live_write_and_second_run_are_reported() {
    let temp = TempDir::new("bob-cli-coalesce");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260716.md");
    let before = concat!(
        "# Daily\n\n",
        "## Pomodoros\n",
        "- [x] (**0840-0845** [t:: 5m]) — FIX\n",
        "  - 🍅 [[tasks#^a]]\n",
        "- [x] (**0845-0850** [t:: 5m]) — FIX\n",
        "  - 🍅 [[tasks#^b]]\n",
        "- [x] (**0850-0855** [t:: 5m]) — FIX\n",
        "  - 🍅 [[tasks#^c]]\n",
    );
    write_file(&daily, before);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task A ^a\n- [ ] #task B ^b\n- [ ] #task C ^c\n",
    );

    let dry = reconcile_json(&vault, &daily, true);
    let items = dry["coalesced_pomodoros"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["span"], "0840-0855");
    assert_eq!(items[0]["minutes"], 15);
    assert_eq!(items[0]["sources"].as_array().unwrap().len(), 3);
    assert_eq!(fs::read_to_string(&daily).unwrap(), before);

    let human = bob_command()
        .args(["task", "reconcile", "--dry-run", "--bob-dir"])
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_NOW", "2026-07-16 12:00")
        .arg("-r")
        .arg("0")
        .output()
        .expect("run human dry-run");
    assert_success(&human);
    assert!(stdout(&human).contains("would coalesce continuous Pomodoros"));
    assert!(stdout(&human).contains("⧉ 0840-0855 FIX 15m"));

    let live = reconcile_json(&vault, &daily, false);
    assert_eq!(live["coalesced_pomodoros"].as_array().unwrap().len(), 1);
    let after = fs::read_to_string(&daily).unwrap();
    assert!(after.contains("- [x] (**0840-0855** [t:: 15m]) — FIX"));
    assert_eq!(after.matches("- [x] (**084").count(), 1);

    let second = reconcile_json(&vault, &daily, false);
    assert!(second["coalesced_pomodoros"].as_array().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&daily).unwrap(), after);
}
