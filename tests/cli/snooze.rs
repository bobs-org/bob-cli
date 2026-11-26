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
        .env("BOB_SNOOZE_NOTIFY_SINK", temp.path().join("notify.json"))
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
fn add_settles_a_promoted_task_into_its_status_group() {
    // Writers settle note-local structure (docs/task-status-hooks.md):
    // the Ready→Next stamp lands grouped, as a capture link would.
    let (temp, _daily_path) = snooze_vault(
        "bob-cli-snooze-add-settle",
        "# 2026-10-08\n\n## Pomodoros\n\n## Tasks\n",
        "---\ntype: [[area]]\n---\n## Tasks\n\n- [ ] #task Review the launch plan ^job\n",
    );
    let task_path = temp.path().join("vault/jobs.md");
    let added =
        snooze_command(&temp, &["add", "jobs:job", "15:30", "-f", "json"])
            .output()
            .expect("run snooze add");
    assert_success(&added);
    assert_eq!(json_output(&added)["lane"], "next");

    let task = fs::read_to_string(&task_path).unwrap();
    let group = task
        .find("### Next & In Progress")
        .unwrap_or_else(|| panic!("settled Next group:\n{task}"));
    let promoted = task
        .find("- [*] #task Review the launch plan")
        .unwrap_or_else(|| panic!("promoted task:\n{task}"));
    assert!(group < promoted, "task sits in its Next group:\n{task}");
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
    assert_eq!(report["notified"], "desktop", "{report}");
    let wake_id = report["wake_id"].as_str().expect("wake_id");
    assert!(!wake_id.is_empty());
    let receipt_path = temp.path().join("state/bob-cli/snooze/last-wake.json");
    let receipt: Value = serde_json::from_str(
        &fs::read_to_string(&receipt_path).expect("read wake receipt"),
    )
    .expect("parse wake receipt");
    assert_eq!(receipt["wake_id"], wake_id);
    assert_eq!(receipt["schema_version"], 1);
    assert_eq!(receipt["tasks"][0]["title"], "Review the launch plan");
    assert_eq!(receipt["tasks"][0]["block_id"], "job");
    let sink: Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join("notify.json"))
            .expect("notify sink"),
    )
    .expect("parse notify sink");
    assert_eq!(sink["title"], "bob snooze");
    assert!(
        sink["body"]
            .as_str()
            .unwrap()
            .starts_with("Ready now: Review the launch plan (woke 12:00"),
        "{sink}"
    );
    let daily = fs::read_to_string(&daily_path).unwrap();
    assert!(daily.contains("- ~~**1100** [[jobs#^job]]~~ · woke 12:00"));
    assert!(daily.contains("- Keep this note"));
}

#[test]
fn list_and_wake_share_dedicated_today_link_membership() {
    let cases = [
        ("plain", "  - [[jobs#^job]]\n", true),
        ("embedded", "  - ![[jobs#^job]]\n", true),
        ("aliased", "  - [[jobs#^job|Job alias]]\n", true),
        ("prose", "  - Discuss [[jobs#^job]] next week\n", false),
        ("nested", "  - A parent note\n    - [[jobs#^job]]\n", false),
        ("fenced", "  ```md\n  - [[jobs#^job]]\n  ```\n", false),
        ("struck", "  - ~~[[jobs#^job]]~~\n", false),
    ];

    for (name, ledger_children, dedicated) in cases {
        let daily = format!(
            "# 2026-10-08\n\n## Pomodoros\n- [ ] () — FOCUS\n{ledger_children}\n## Snoozed\n- **1100** [[jobs#^job]]\n"
        );
        let (temp, daily_path) = snooze_vault(
            &format!("bob-cli-snooze-membership-{name}"),
            &daily,
            "- [*] #task Job ^job\n",
        );

        let listed = snooze_command(&temp, &["-f", "json"])
            .output()
            .expect("list snoozed rows");
        assert_success(&listed);
        let list = json_output(&listed);
        if dedicated {
            assert_eq!(list["rows"][0]["state"], "ignored", "{name}: {list}");
            assert_eq!(
                list["rows"][0]["reason"], "already_linked",
                "{name}: {list}"
            );
        } else {
            assert_eq!(list["rows"][0]["state"], "due", "{name}: {list}");
        }

        let vault = temp.path().join("vault");
        let plan = bob_command()
            .args(["plan", "-f", "json"])
            .env("BOB_DIR", &vault)
            .env("BOB_DAY_FILE", &daily_path)
            .env("BOB_CONFIG_FILE", temp.path().join("config.yml"))
            .env("BOB_NOW", NOW)
            .output()
            .expect("read Today and Snoozed plan meters");
        assert_success(&plan);
        let plan: Value = serde_json::from_str(stdout(&plan).trim()).unwrap();
        assert_eq!(
            plan["today_tasks"].as_array().unwrap().len(),
            dedicated as usize,
            "{name}: {plan}"
        );
        assert_eq!(
            plan["snoozed"]["count"],
            (!dedicated) as usize,
            "{name}: {plan}"
        );

        let agenda = bob_command()
            .args(["capture-pomodoros", "--tasks", "-f", "json", "-b"])
            .arg(&vault)
            .env("BOB_DAY_FILE", &daily_path)
            .env("BOB_CONFIG_FILE", temp.path().join("config.yml"))
            .env("BOB_NOW", NOW)
            .output()
            .expect("read Pomodoro and Snoozed agenda");
        assert_success(&agenda);
        let agenda: Value =
            serde_json::from_str(stdout(&agenda).trim()).unwrap();
        assert_eq!(
            agenda["snoozed"]["rows"].as_array().unwrap().len(),
            (!dedicated) as usize,
            "{name}: {agenda}"
        );
        // The agenda still shows any task link authored anywhere in the
        // Pomodoro block (including nested notes). Only its separate
        // `snoozed` projection follows dedicated Today membership.

        let woke = snooze_command(&temp, &["wake", "-f", "json"])
            .output()
            .expect("wake due snoozed rows");
        assert_success(&woke);
        let wake = json_output(&woke);
        if dedicated {
            assert!(wake["woken"].as_array().unwrap().is_empty(), "{wake}");
            assert!(
                wake["dropped"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| { row["reason"] == "already_linked" }),
                "{wake}"
            );
        } else {
            assert_eq!(wake["woken"].as_array().unwrap().len(), 1, "{wake}");
            assert_eq!(wake["woken"][0]["block_id"], "job");
            assert!(
                fs::read_to_string(&daily_path)
                    .unwrap()
                    .contains("[[jobs#^job]]"),
                "the due row moved to a Pomodoro: {}",
                fs::read_to_string(&daily_path).unwrap()
            );
        }
    }

    // Empty-target links resolve against their own daily note, just like
    // the plan's Today projection.
    let temp = TempDir::new("bob-cli-snooze-membership-same-note");
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    let daily_path = vault.join("2026/20261008.md");
    write_file(
        &daily_path,
        concat!(
            "# 2026-10-08\n\n",
            "## Pomodoros\n",
            "- [ ] () — FOCUS\n",
            "  - [[#^job]]\n\n",
            "## Snoozed\n",
            "- **1100** [[#^job]]\n\n",
            "## Tasks\n",
            "- [*] #task Local job ^job\n",
        ),
    );
    write_file(&vault.join("jobs.md"), "- [*] #task Other job ^other\n");
    let list = snooze_command(&temp, &["-f", "json"])
        .output()
        .expect("list same-note snoozed row");
    assert_success(&list);
    assert_eq!(json_output(&list)["rows"][0]["reason"], "already_linked");
    let wake = snooze_command(&temp, &["wake", "-f", "json"])
        .output()
        .expect("wake same-note snoozed row");
    assert_success(&wake);
    assert!(json_output(&wake)["woken"].as_array().unwrap().is_empty());
    assert!(json_output(&wake)["dropped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| { row["reason"] == "already_linked" }));
    let day = fs::read_to_string(&daily_path).unwrap();
    assert!(!day.contains("- **1100** [[#^job]]"));
    assert!(day.contains("  - [[#^job]]"));
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

fn receipt_path(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("state/bob-cli/snooze/last-wake.json")
}

#[test]
fn dry_run_and_no_work_leave_the_prior_receipt_untouched() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-receipt-dry",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1100** [[jobs#^job]]\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let dry =
        snooze_command(&temp, &["wake", "jobs:job", "--dry-run", "-f", "json"])
            .output()
            .expect("dry-run wake");
    assert_success(&dry);
    let dry_report = json_output(&dry);
    assert_eq!(dry_report["status"], "woke");
    assert_eq!(dry_report["notified"], "none");
    assert!(dry_report["wake_id"].is_null());
    assert!(!receipt_path(&temp).exists());

    let woke = snooze_command(&temp, &["wake", "-f", "json"])
        .output()
        .expect("wake once");
    assert_success(&woke);
    let prior = fs::read(receipt_path(&temp)).expect("receipt after wake");
    let daily = fs::read_to_string(&daily_path).unwrap();

    let noop = snooze_command(&temp, &["wake", "-f", "json"])
        .output()
        .expect("no-op wake");
    assert_success(&noop);
    assert_eq!(json_output(&noop)["status"], "nothing_due");
    assert_eq!(fs::read(receipt_path(&temp)).unwrap(), prior);
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), daily);
}

#[test]
fn live_mac_claim_reports_mac_capture_without_desktop() {
    let (temp, _) = snooze_vault(
        "bob-cli-snooze-mac-claim",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1100** [[jobs#^job]]\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let claim_dir = temp.path().join("state/bob-cli/snooze");
    fs::create_dir_all(&claim_dir).unwrap();
    let claim = serde_json::json!({
        "schema_version": 1,
        "pid": std::process::id(),
        "app": "Bob Mac Capture",
        "version": "1.0.0",
        "started_at": "2026-10-08T12:00:00Z",
    });
    write_file(&claim_dir.join("notifier.json"), &claim.to_string());
    let output = snooze_command(&temp, &["wake", "-f", "json"])
        .output()
        .expect("wake with live claim");
    assert_success(&output);
    let report = json_output(&output);
    assert_eq!(report["notified"], "mac_capture", "{report}");
    assert!(report["wake_id"].as_str().is_some());
    assert!(!temp.path().join("notify.json").exists());
}

#[test]
fn notify_off_still_writes_receipt_and_desktop_failure_keeps_the_wake() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-notify-off",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1100** [[jobs#^job]]\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let config = temp.path().join("off.yml");
    write_file(
        &config,
        "timezone: America/New_York\nsnooze:\n  notify: off\n",
    );
    let output = snooze_command(&temp, &["wake", "-f", "json"])
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("wake with notify off");
    assert_success(&output);
    let report = json_output(&output);
    assert_eq!(report["status"], "woke");
    assert_eq!(report["notified"], "none");
    assert!(receipt_path(&temp).exists());
    assert!(!temp.path().join("notify.json").exists());
    assert!(fs::read_to_string(&daily_path)
        .unwrap()
        .contains("~~**1100**"));

    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-notify-fail",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1100** [[jobs#^job]]\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let output = snooze_command(&temp, &["wake", "-f", "json"])
        .env("BOB_SNOOZE_NOTIFY_SINK", "fail")
        .output()
        .expect("wake with failed desktop");
    assert_success(&output);
    let report = json_output(&output);
    assert_eq!(report["status"], "woke", "{report}");
    assert_eq!(report["notified"], "none");
    assert!(report["wake_id"].as_str().is_some());
    assert!(fs::read_to_string(daily_path)
        .unwrap()
        .contains("~~**1100**"));
}

#[test]
fn invalid_notify_exits_two_before_mutating() {
    let (temp, daily_path) = snooze_vault(
        "bob-cli-snooze-notify-invalid",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1100** [[jobs#^job]]\n",
        "- [*] #task Review the launch plan ^job\n",
    );
    let before = fs::read_to_string(&daily_path).unwrap();
    let config = temp.path().join("bad.yml");
    write_file(
        &config,
        "timezone: America/New_York\nsnooze:\n  notify: banner\n",
    );
    let output = snooze_command(&temp, &["wake", "-f", "json"])
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("wake with invalid notify");
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    let report = json_output(&output);
    assert_eq!(report["error"]["kind"], "invalid_config");
    assert!(
        report["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("snooze.notify"),
        "{report}"
    );
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before);
    assert!(!receipt_path(&temp).exists());
}

#[test]
fn wake_refuses_typed_destination_with_two_running_pomodoros() {
    let daily = "# 2026-10-08\n\n## Pomodoros\n- [ ] (**1100-1200** [t:: 60m]) — WORK\n- [ ] (**1130-1230** [t:: 60m]) — BUILD\n\n## Snoozed\n- **1130** [[jobs#^job]]\n  - keep this note\n";
    let task = "- [*] #task Review the launch plan [type:: WRITE] ^job\n";
    let (temp, daily_path) =
        snooze_vault("bob-cli-snooze-wake-refuse-timed", daily, task);
    let before = fs::read_to_string(&daily_path).unwrap();
    let output = snooze_command(&temp, &["wake", "-f", "json"])
        .output()
        .expect("run refused wake");
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    let report = json_output(&output);
    assert_eq!(report["error"]["kind"], "multiple_open_timed", "{report}");
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before);
    assert!(!receipt_path(&temp).exists());
    assert!(!temp.path().join("notify.json").exists());

    let dry = snooze_command(&temp, &["wake", "--dry-run", "-f", "json"])
        .output()
        .expect("dry-run refused wake");
    assert_eq!(dry.status.code(), Some(2), "{}", stdout(&dry));
    assert_eq!(json_output(&dry)["error"]["kind"], "multiple_open_timed");
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before);

    let explicit = snooze_command(&temp, &["wake", "jobs:job", "-f", "json"])
        .output()
        .expect("explicit refused wake");
    assert_eq!(explicit.status.code(), Some(2), "{}", stdout(&explicit));
    assert_eq!(
        json_output(&explicit)["error"]["kind"],
        "multiple_open_timed"
    );
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before);
    assert!(!receipt_path(&temp).exists());
    assert!(!temp.path().join("notify.json").exists());
}

#[test]
fn wake_refuses_when_daily_note_has_no_pomodoros_section() {
    let daily = "# 2026-10-08\n\n## Snoozed\n- **1130** [[jobs#^job]]\n  - keep this note\n";
    let task = "- [*] #task Review the launch plan [type:: WRITE] ^job\n";
    let (temp, daily_path) =
        snooze_vault("bob-cli-snooze-wake-refuse-nopomo", daily, task);
    let before = fs::read_to_string(&daily_path).unwrap();
    let output = snooze_command(&temp, &["wake", "-f", "json"])
        .output()
        .expect("run refused wake");
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    let report = json_output(&output);
    assert_eq!(report["error"]["kind"], "no_pomodoros", "{report}");
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before);
    assert!(!receipt_path(&temp).exists());
    assert!(!temp.path().join("notify.json").exists());

    let dry = snooze_command(&temp, &["wake", "--dry-run", "-f", "json"])
        .output()
        .expect("dry-run refused wake");
    assert_eq!(dry.status.code(), Some(2), "{}", stdout(&dry));
    assert_eq!(json_output(&dry)["error"]["kind"], "no_pomodoros");
    assert_eq!(fs::read_to_string(&daily_path).unwrap(), before);
}

#[test]
fn skipped_sweep_does_not_write_a_receipt() {
    let (temp, _) = snooze_vault(
        "bob-cli-snooze-skipped-receipt",
        "# 2026-10-08\n\n## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1100** [[jobs#^job]]\n",
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
        .expect("skipped sweep");
    assert_success(&sweep);
    assert_eq!(json_output(&sweep)["status"], "skipped");
    assert!(!receipt_path(&temp).exists());
}
