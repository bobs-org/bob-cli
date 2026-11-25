//! Public `bob snooze` list, guarded mutations, and command metadata.

use crate::support::*;
use serde_json::Value;
use std::fs;

const NOW: &str = "2026-10-08T12:00:00-04:00";

fn snooze_vault(
    prefix: &str,
    daily: &str,
    task: &str,
) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    let daily_path = vault.join("2026/20261008.md");
    write_file(&daily_path, daily);
    write_file(&vault.join("jobs.md"), task);
    (temp, daily_path)
}

fn snooze_command(temp: &TempDir, args: &[&str]) -> std::process::Command {
    let config = temp.path().join("bob.yml");
    write_file(&config, "timezone: America/New_York\n");
    let mut command = bob_command();
    command
        .arg("snooze")
        .env("BOB_DIR", temp.path().join("vault"))
        .env("BOB_DAY_FILE", temp.path().join("vault/2026/20261008.md"))
        .env("BOB_CONFIG_FILE", config)
        .env("BOB_VAULT_SYNC_LOCK_FILE", temp.path().join("vault.lock"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("BOB_NOW", NOW);
    command.args(args);
    command
}

fn json_output(output: &std::process::Output) -> Value {
    serde_json::from_str(stdout(output).trim()).expect("snooze JSON")
}

#[test]
fn bare_command_lists_read_only_with_stable_json() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-list",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1530** [[jobs#^job]]\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let before = fs::read_to_string(&daily_path).unwrap();
    let output = snooze_command(&temp, &["-f", "json"])
        .output()
        .expect("run bob snooze list");

    assert_success(&output);
    let report = json_output(&output);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["date"], "2026-10-08");
    assert_eq!(report["rows"][0]["state"], "asleep");
    assert_eq!(report["rows"][0]["target"], "1530");
    assert_eq!(report["rows"][0]["title"], "Review the launch plan");
    assert!(!report["next_wake_at"].as_str().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before);
}

#[test]
fn add_dry_run_and_drop_leave_expected_postimages() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-add-drop",
        "# 2026-10-08\n\n## Pomodoros\n\n## Tasks\n",
        "- [ ] #task Review the launch plan ^job\n",
    );
    let task_path = temp.path().join("vault/jobs.md");
    let before_day = fs::read_to_string(&daily_path).unwrap();
    let before_task = fs::read_to_string(&task_path).unwrap();
    let preview = snooze_command(
        &temp,
        &["add", "jobs:job", "15:30", "--dry-run", "-f", "json"],
    )
    .output()
    .expect("preview snooze add");
    assert_success(&preview);
    assert_eq!(json_output(&preview)["status"], "planned");
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before_day);
    assert_eq!(fs::read_to_string(&task_path).unwrap(), before_task);

    let added =
        snooze_command(&temp, &["add", "jobs:job", "15:30", "-f", "json"])
            .output()
            .expect("run snooze add");
    assert_success(&added);
    assert_eq!(json_output(&added)["status"], "added");
    assert!(fs::read_to_string(&daily_path)
        .unwrap()
        .contains("- **1530** [[jobs#^job]]"));
    assert!(fs::read_to_string(&task_path)
        .unwrap()
        .contains("- [*] #task Review the launch plan"));

    let dropped = snooze_command(&temp, &["drop", "jobs:job", "-f", "json"])
        .output()
        .expect("run snooze drop");
    assert_success(&dropped);
    assert_eq!(json_output(&dropped)["status"], "dropped");
    assert!(!fs::read_to_string(&daily_path)
        .unwrap()
        .contains("**1530**"));
}

#[test]
fn wake_moves_due_subtree_and_records_history() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-wake",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1100** [[jobs#^job]]\n  - Keep this note\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let output = snooze_command(&temp, &["wake", "-f", "json"])
        .output()
        .expect("run snooze wake");

    assert_success(&output);
    let report = json_output(&output);
    assert_eq!(report["status"], "woke", "{report}");
    assert_eq!(report["woken"].as_array().unwrap().len(), 1);
    assert_eq!(report["notified"], "none");
    assert!(report["wake_id"].is_null());
    let daily = fs::read_to_string(&daily_path).unwrap();
    assert!(daily.contains("- ~~**1100** [[jobs#^job]]~~ · woke 12:00"));
    assert!(daily.contains("- Keep this note"));
}

#[test]
fn no_work_wake_leaves_vault_and_state_directory_unchanged() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-no-work",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let before = fs::read_to_string(&daily_path).unwrap();
    let output = snooze_command(&temp, &["wake", "-f", "json"])
        .output()
        .expect("run empty wake");

    assert_success(&output);
    assert_eq!(json_output(&output)["status"], "nothing_due");
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before);
    assert!(!temp.path().join("state").exists());
}

#[test]
fn concurrent_wakes_move_one_row_in_one_successful_batch() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-concurrent",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1100** [[jobs#^job]]\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let mut first_command = snooze_command(&temp, &["wake", "-f", "json"]);
    first_command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let first = first_command.spawn().expect("start first wake");
    let mut second_command = snooze_command(&temp, &["wake", "-f", "json"]);
    second_command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let second = second_command.spawn().expect("start second wake");
    let first = first.wait_with_output().expect("first wake result");
    let second = second.wait_with_output().expect("second wake result");
    assert_success(&first);
    assert_success(&second);
    let mut statuses = [
        json_output(&first)["status"].clone(),
        json_output(&second)["status"].clone(),
    ];
    statuses
        .sort_by_key(|value| value.as_str().unwrap_or_default().to_string());
    assert_eq!(statuses, ["nothing_due", "woke"]);
    let daily = fs::read_to_string(daily_path).unwrap();
    assert_eq!(daily.matches("~~**1100**").count(), 1);
    assert!(daily.contains("- ~~**1100** [[jobs#^job]]~~ · woke 12:00"));
    assert!(daily.contains("\t- [[jobs#^job]]"), "{daily}");
}

#[test]
fn explicit_wake_works_when_automatic_writer_is_elsewhere() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-writer",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1700** [[jobs#^job]]\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        "timezone: America/New_York\ntick:\n  writer: another-host\n",
    );

    let sweep = snooze_command(&temp, &["wake", "-f", "json"])
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run pinned wake sweep");
    assert_success(&sweep);
    assert_eq!(json_output(&sweep)["status"], "skipped");
    assert!(!temp.path().join("state").exists());
    assert!(fs::read_to_string(&daily_path)
        .unwrap()
        .contains("- **1700**"));

    let explicit = snooze_command(&temp, &["wake", "jobs:job", "-f", "json"])
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run explicit wake on non-writer");
    assert_success(&explicit);
    let explicit_report = json_output(&explicit);
    assert_eq!(explicit_report["status"], "woke", "{explicit_report}");
}

#[test]
fn help_completion_and_catalog_expose_snooze_commands() {
    let help = bob_command()
        .args(["snooze", "--help"])
        .output()
        .expect("show snooze help");
    assert_success(&help);
    assert!(stdout(&help).contains("read-only `list`"));

    let list_help = bob_command()
        .args(["snooze", "list", "--help"])
        .output()
        .expect("show snooze list help");
    assert_success(&list_help);
    assert!(stdout(&list_help).contains("--bob-dir"));
    assert!(stdout(&list_help).contains("--format"));

    let completion = bob_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "snooze",
            "",
        ])
        .output()
        .expect("generate completion");
    assert_success(&completion);
    let completion = stdout(&completion);
    assert!(completion.contains("list"), "{completion}");
    assert!(completion.contains("wake"), "{completion}");

    let catalog = bob_command()
        .args(["__catalog", "--protocol", "1"])
        .output()
        .expect("read command catalog");
    assert_success(&catalog);
    let entries: Value = json_output(&catalog);
    for id in ["snooze-list", "snooze-add", "snooze-drop", "snooze-wake"] {
        assert!(
            entries["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == id),
            "missing catalog row {id}"
        );
    }
}
