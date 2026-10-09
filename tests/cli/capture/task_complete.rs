//! Whole-item `!note:block-id` execution through the shared engine.

use crate::support::*;
use std::fs;

const NOW: &str = "2026-10-05 09:30:00";

fn vault_with_settings(name: &str) -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    (temp, vault)
}

fn capture(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW);
    command.output().expect("run bob capture")
}

fn capture_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
) -> serde_json::Value {
    let mut full = vec!["-f", "json"];
    full.extend(args.iter().copied());
    let output = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(vault)
            .args(full)
            .env("BOB_DAY_FILE", day_file)
            .env("BOB_NOW", NOW);
        command.output().expect("run bob capture json")
    };
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

#[test]
fn next_completes_with_running_session_strike() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-strike");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [*] #task Fix flaky gkeep test ^fix-flaky\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^fix-flaky]]\n",
    );

    let json = capture_json(&vault, &day_file, &["!sase:fix-flaky"]);
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "task_complete");
    assert_eq!(json["placement"], "completed");
    assert_eq!(json["routed"], true);
    assert!(json["route"].is_null());
    assert_eq!(json["route_label"], "sase.md");
    assert_eq!(json["relative_target"], "sase.md");
    assert_eq!(json["text"], "");
    assert_eq!(json["block_id"], "fix-flaky");
    assert_eq!(
        json["task_line"],
        "- [x] #task Fix flaky gkeep test  [completion:: 2026-10-05] ^fix-flaky"
    );
    assert_eq!(
        json["previous_task_line"],
        "- [*] #task Fix flaky gkeep test ^fix-flaky"
    );
    assert_eq!(json["previous_status_symbol"], "*");
    assert_eq!(json["previous_status_name"], "Next");
    assert_eq!(json["status_symbol"], "x");
    assert_eq!(json["status_name"], "Done");
    assert_eq!(json["status_changed"], true);
    assert_eq!(json["created"], "2026-10-05");
    let complete = &json["task_complete"];
    assert_eq!(complete["raw"], "!sase:fix-flaky");
    assert_eq!(complete["note"], "sase");
    assert_eq!(complete["note_path"], "sase.md");
    assert_eq!(complete["block_id"], "fix-flaky");
    assert_eq!(complete["action"], "completed");
    assert_eq!(complete["completion_date"], "2026-10-05");
    assert_eq!(complete["subtasks"], serde_json::json!([]));
    assert_eq!(complete["subtasks_left_open"], serde_json::json!([]));
    assert_eq!(complete["unblocked"], serde_json::json!([]));
    assert_eq!(complete["ledger"]["struck"], 1);
    assert_eq!(complete["ledger"]["moved"], serde_json::json!([]));
    // The completed root is reported once with a cumulative diff.
    let blocks = json["task_blocks"].as_array().expect("task_blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["block_id"], "fix-flaky");
    assert_eq!(blocks[0]["roles"], serde_json::json!(["completed"]));

    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read note"),
        "- [x] #task Fix flaky gkeep test  [completion:: 2026-10-05] ^fix-flaky\n"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - ~~[[sase#^fix-flaky]]~~\n"
    );
}

#[test]
fn placeholder_link_moves_to_running_entry_and_placeholder_is_removed() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-move");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Other live ^other\n- [ ] #task Move me ^mv1\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^other]]\n- [ ] () \u{2014} SASE\n  - [[sase#^mv1]]\n",
    );

    let json = capture_json(&vault, &day_file, &["!sase:mv1"]);
    let ledger = &json["task_complete"]["ledger"];
    assert_eq!(ledger["struck"], 1);
    assert_eq!(ledger["moved"].as_array().expect("moved").len(), 1);
    assert_eq!(ledger["moved"][0]["from"]["name"], "SASE");
    assert_eq!(ledger["moved"][0]["to"]["name"], "CAPTURE");
    assert_eq!(ledger["removed_placeholders"][0]["name"], "SASE");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^other]]\n  - ~~[[sase#^mv1]]~~\n"
    );
}

#[test]
fn carried_link_dedupes_into_morning_entry_when_nothing_runs() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-dedupe");
    let day_file = vault.join("20261005.md");
    write_file(&vault.join("sase.md"), "- [*] #task Carried ^carry\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [x] (0800-0830) \u{2014} MORNING\n  - ~~[[sase#^carry]]~~\n- [ ] () \u{2014} SASE\n  - [[sase#^carry]]\n",
    );

    let json = capture_json(&vault, &day_file, &["!sase:carry"]);
    let ledger = &json["task_complete"]["ledger"];
    assert_eq!(ledger["deduplicated"], 1);
    assert_eq!(ledger["moved"], serde_json::json!([]));
    assert_eq!(ledger["removed_placeholders"][0]["name"], "SASE");
    // Exactly one struck link survives, in the morning entry.
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        "## Pomodoros\n- [x] (0800-0830) \u{2014} MORNING\n  - ~~[[sase#^carry]]~~\n"
    );
}

#[test]
fn embedded_subtasks_close_and_blocked_descendant_stays_open() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-subtasks");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [?] #task Blocked root ^root\n\t- ![[sase#^sub1]]\n\t- ![[sase#^sub2]]\n- [/] #task Sub one ^sub1\n- [?] #task Sub two ^sub2\n\t- ![[sase#^sub3]]\n- [ ] #task Sub three ^sub3\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );

    let json = capture_json(&vault, &day_file, &["!sase:root"]);
    assert_eq!(json["status_symbol"], "x");
    let complete = &json["task_complete"];
    assert_eq!(complete["subtasks"].as_array().expect("subtasks").len(), 1);
    assert_eq!(complete["subtasks"][0]["block_id"], "sub1");
    assert_eq!(complete["subtasks"][0]["text"], "Sub one");
    assert_eq!(complete["subtasks"][0]["previous_status_symbol"], "/");
    assert_eq!(complete["subtasks"][0]["status_symbol"], "x");
    assert_eq!(
        complete["subtasks_left_open"]
            .as_array()
            .expect("left")
            .len(),
        1
    );
    assert_eq!(complete["subtasks_left_open"][0]["block_id"], "sub2");
    assert_eq!(complete["subtasks_left_open"][0]["reason"], "blocked");
    // The Blocked subtask is not descended: the open task below it stays
    // open and is never reported.
    assert!(
        fs::read_to_string(vault.join("sase.md"))
            .expect("read note")
            .contains("- [ ] #task Sub three ^sub3\n"),
        "{complete}"
    );
    // Ledger untouched: no links to retire.
    assert!(complete.get("ledger").is_none(), "{complete}");
}

#[test]
fn blocked_root_completes_and_dependent_unblocks() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-unblock");
    let day_file = vault.join("20261005.md");
    write_file(&vault.join("sase.md"), "- [?] #task Blocked root ^root\n");
    write_file(
        &vault.join("travel.md"),
        "- [ ] #task Dep root [id:: dep-root] ^dep\n- [?] #task Waiter [dependsOn:: dep-root] [id:: waiter] ^waiter\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );

    let root = capture_json(&vault, &day_file, &["!sase:root"]);
    assert_eq!(root["previous_status_symbol"], "?");
    assert_eq!(root["status_symbol"], "x");

    let json = capture_json(&vault, &day_file, &["!travel:dep"]);
    let unblocked = json["task_complete"]["unblocked"]
        .as_array()
        .expect("unblocked");
    assert_eq!(unblocked.len(), 1);
    assert_eq!(unblocked[0]["block_id"], "waiter");
    assert_eq!(unblocked[0]["text"], "Waiter");
    assert_eq!(unblocked[0]["previous_status_symbol"], "?");
    assert_eq!(unblocked[0]["status_symbol"], " ");
    assert_eq!(
        fs::read_to_string(vault.join("travel.md")).expect("read travel"),
        "- [x] #task Dep root [id:: dep-root]  [completion:: 2026-10-05] ^dep\n- [ ] #task Waiter [dependsOn:: dep-root] [id:: waiter] ^waiter\n"
    );
    // The recovered dependent is reported with the unblocked role.
    let blocks = json["task_blocks"].as_array().expect("task_blocks");
    let roles: Vec<&str> = blocks
        .iter()
        .flat_map(|block| {
            block["roles"]
                .as_array()
                .expect("roles")
                .iter()
                .filter_map(|role| role.as_str())
        })
        .collect();
    assert!(roles.contains(&"completed"), "{roles:?}");
    assert!(roles.contains(&"unblocked"), "{roles:?}");
}

#[test]
fn already_done_is_a_noop() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-done");
    let day_file = vault.join("20261005.md");
    write_file(&vault.join("sase.md"), "- [x] #task Finished ^fin\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );

    let json = capture_json(&vault, &day_file, &["!sase:fin"]);
    assert_eq!(json["task_complete"]["action"], "already_done");
    assert!(
        json["task_complete"].get("completion_date").is_none(),
        "{json}"
    );
    assert_eq!(json["status_changed"], false);
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read note"),
        "- [x] #task Finished ^fin\n"
    );
}

#[test]
fn refusals_leave_the_vault_intact() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-refusals");
    let day_file = vault.join("20261005.md");
    let before_note = "- [-] #task Gone ^cx\n- [>] #task Weird ^wx\n- [ ] #task Water [repeat:: every day] ^water\n- [x] #task Done ^dn\n- [ ] #task Flaky ^fix-flaky\nplain line ^nt1\n";
    write_file(&vault.join("sase.md"), before_note);
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let cases = [
        ("!sase:cx", "is Canceled; reopen it before completing it"),
        (
            "!sase:wx",
            "has status `[>]`; only Ready, Blocked, Next, and In Progress tasks can be completed",
        ),
        ("!sase:water", "repeats; complete recurring tasks in Obsidian"),
        ("!sase:missing", "no task with block ID ^missing in sase.md"),
        ("!nosuchnote:xx", "no such note: nosuchnote"),
        (
            "!sase:nt1",
            "is not a task",
        ),
    ];
    for (draft, needle) in cases {
        let output = capture(&vault, &day_file, &[draft]);
        assert!(
            !output.status.success(),
            "{draft} must fail:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(needle),
            "{draft}: expected `{needle}`:\n{}",
            format_output(&output)
        );
    }
    // Close-match suggestions name the intended task.
    let output = capture(&vault, &day_file, &["!sase:fix-flak"]);
    assert!(stderr(&output).contains("did you mean ^fix-flaky?"));

    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read note"),
        before_note,
        "refusals must not write"
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "refusals must not create inbox tasks"
    );
}

#[test]
fn ambiguous_note_and_duplicate_id_and_forced_flags_fail() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-ambiguous");
    let day_file = vault.join("20261005.md");
    fs::create_dir_all(vault.join("a")).expect("mkdir a");
    fs::create_dir_all(vault.join("b")).expect("mkdir b");
    write_file(&vault.join("a/dup.md"), "- [ ] #task A ^x1\n");
    write_file(&vault.join("b/dup.md"), "- [ ] #task B ^x1\n");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task One ^dup\n- [ ] #task Two ^dup\n- [ ] #task Solo ^solo\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );

    let output = capture(&vault, &day_file, &["!dup:x1"]);
    assert!(stderr(&output).contains("ambiguous note"));

    let output = capture(&vault, &day_file, &["!sase:dup"]);
    assert!(stderr(&output).contains("appears 2 times"));

    let mut command = bob_command();
    let output = command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("sase")
        .arg("--")
        .arg("!sase:solo")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run forced-route capture");
    assert!(stderr(&output).contains("must be the whole capture item"));
}

#[test]
fn batch_of_two_completions_reports_already_done_for_repeats() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-batch");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task One ^one\n- [ ] #task Two ^two\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );

    let json = capture_json(&vault, &day_file, &["!sase:one\n\n!sase:two"]);
    let actions: Vec<&str> = json["captures"]
        .as_array()
        .expect("captures")
        .iter()
        .map(|item| item["task_complete"]["action"].as_str().expect("action"))
        .collect();
    assert_eq!(actions, vec!["completed", "completed"]);

    // The same task twice reports the second as already done.
    let (_temp, vault2) = vault_with_settings("bob-cli-task-complete-repeat");
    let day2 = vault2.join("20261005.md");
    write_file(&vault2.join("sase.md"), "- [ ] #task One ^one\n");
    write_file(
        &day2,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let json = capture_json(&vault2, &day2, &["!sase:one\n\n!sase:one"]);
    let actions: Vec<&str> = json["captures"]
        .as_array()
        .expect("captures")
        .iter()
        .map(|item| item["task_complete"]["action"].as_str().expect("action"))
        .collect();
    assert_eq!(actions, vec!["completed", "already_done"]);
}

#[test]
fn failure_in_item_two_rolls_item_one_back() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-rollback");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task One ^one\n- [ ] #task Two ^two\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );

    let output = capture(&vault, &day_file, &["!sase:one\n\n!sase:missing"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("capture item 2"),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read note"),
        "- [ ] #task One ^one\n- [ ] #task Two ^two\n",
        "a later failure rolls the whole batch back"
    );
}

#[test]
fn dry_run_json_matches_real() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-dryrun");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Other live ^other\n- [*] #task Fix me ^fix\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^other]]\n- [ ] () \u{2014} SASE\n  - [[sase#^fix]]\n",
    );
    capture_json_dry_run_matches_real(&vault, &day_file, NOW, &["!sase:fix"]);
}

#[test]
fn human_output_names_transition_ledger_and_unblocked() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-human");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Other live ^other\n- [*] #task Fix me ^fix\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^other]]\n- [ ] () \u{2014} SASE\n  - [[sase#^fix]]\n",
    );

    let output = capture(&vault, &day_file, &["!sase:fix"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains(
            "\u{2713} completed [*] \u{2192} [x] Fix me  sase.md ^fix"
        ),
        "{out}"
    );
    assert!(
        out.contains("Task Link moved SASE \u{2192} CAPTURE (struck)"),
        "{out}"
    );
    assert!(out.contains("removed empty SASE"), "{out}");

    // Dry run says what would change.
    let mut command = bob_command();
    let dry = command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("!sase:other")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("dry run");
    assert_success(&dry);
    assert!(
        stdout(&dry).contains("would complete"),
        "{}",
        format_output(&dry)
    );

    // Already done prints the idempotent line.
    let done = capture(&vault, &day_file, &["!sase:fix"]);
    assert_success(&done);
    assert!(
        stdout(&done).contains("already done [x] Fix me  sase.md ^fix"),
        "{}",
        format_output(&done)
    );
    assert!(
        stdout(&done).contains("nothing to change"),
        "{}",
        format_output(&done)
    );
}

#[test]
fn task_living_in_todays_daily_note_completes_in_place() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-daynote");
    let day_file = vault.join("20261005.md");
    write_file(&vault.join("sase.md"), "- [ ] #task Elsewhere ^else\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[20261005#^dailytask]]\n## Tasks\n- [ ] #task Daily thing ^dailytask\n",
    );

    let json = capture_json(&vault, &day_file, &["!20261005:dailytask"]);
    assert_eq!(json["task_complete"]["action"], "completed");
    assert_eq!(json["route_label"], "20261005.md");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day.contains(
            "- [x] #task Daily thing  [completion:: 2026-10-05] ^dailytask"
        ),
        "{day}"
    );
    assert!(day.contains("~~[[20261005#^dailytask]]~~"), "{day}");
}

fn capture_human(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
    dry_run: bool,
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(vault);
    if dry_run {
        command.arg("--dry-run");
    }
    command
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    command.output().expect("run bob capture human")
}

fn vault_bytes(vault: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = std::fs::read_dir(&dir).expect("read vault dir");
        for entry in read {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(vault)
                    .expect("relative")
                    .to_string_lossy()
                    .to_string();
                let bytes = std::fs::read(&path).expect("read file");
                entries.push((relative, bytes));
            }
        }
    }
    entries.sort();
    entries
}

#[test]
fn two_completions_plus_close_commit_as_one_batch() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-two-plus");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task One ^one\n- [ ] #task Two ^two\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^one]]\n  - [[sase#^two]]\n",
    );

    let output = {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg("!sase:one\n\n!sase:two\n\n=x")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW);
        command.output().expect("run batch json")
    };
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    let captures = json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 3, "{json}");
    assert_eq!(captures[0]["kind"], "task_complete");
    assert_eq!(captures[0]["task_complete"]["action"], "completed");
    assert_eq!(captures[0]["task_complete"]["block_id"], "one");
    assert_eq!(captures[0]["task_complete"]["text"], "One");
    assert_eq!(captures[1]["kind"], "task_complete");
    assert_eq!(captures[1]["task_complete"]["action"], "completed");
    assert_eq!(captures[1]["task_complete"]["block_id"], "two");
    assert_eq!(captures[1]["task_complete"]["text"], "Two");
    assert_eq!(captures[2]["kind"], "pomodoro_close");

    let note = fs::read_to_string(vault.join("sase.md")).expect("read note");
    assert!(note.contains("- [x] #task One"), "{note}");
    assert!(note.contains("- [x] #task Two"), "{note}");
    let day = fs::read_to_string(&day_file).expect("read day");
    assert!(day.contains("~~[[sase#^one]]~~"), "{day}");
    assert!(day.contains("~~[[sase#^two]]~~"), "{day}");
}

#[test]
fn forced_flags_exit_2_with_exact_message_and_untouched_vault() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-forced");
    let day_file = vault.join("20261005.md");
    write_file(&vault.join("sase.md"), "- [ ] #task Solo ^solo\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let before = vault_bytes(&vault);
    let message = "`!sase:solo` completes an existing task and must be the whole capture item; remove its forced destination flags";
    let cases: Vec<Vec<&str>> = vec![
        vec!["--route", "sase"],
        vec!["--route", "sase", "--section", "Tasks"],
        vec!["--route", "sase", "--task", "foo"],
        vec!["--route", "sase", "--task", "foo", "--task-section", "Bar"],
        vec!["--clip"],
    ];
    for flags in cases {
        let mut command = bob_command();
        command.arg("capture").arg("-b").arg(&vault);
        for flag in &flags {
            command.arg(flag);
        }
        let output = command
            .arg("--")
            .arg("!sase:solo")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW)
            .output()
            .expect("run forced capture");
        assert_eq!(output.status.code(), Some(2), "{flags:?}");
        let expected = format!(
            "bob capture: capture item 1 starting on line 1: {message}\n"
        );
        assert_eq!(stderr(&output), expected, "{flags:?}");
        assert_eq!(vault_bytes(&vault), before, "{flags:?} must not write");
    }
}

#[test]
fn ambiguous_and_duplicate_refusals_state_exit_code_and_leave_vault_untouched()
{
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-ambdup");
    let day_file = vault.join("20261005.md");
    fs::create_dir_all(vault.join("a")).expect("mkdir a");
    fs::create_dir_all(vault.join("b")).expect("mkdir b");
    write_file(&vault.join("a/dup.md"), "- [ ] #task A ^x1\n");
    write_file(&vault.join("b/dup.md"), "- [ ] #task B ^x1\n");
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task One ^dup\n- [ ] #task Two ^dup\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let before = vault_bytes(&vault);

    let output = capture(&vault, &day_file, &["!dup:x1"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        stderr(&output),
        "bob capture: capture item 1 starting on line 1: ambiguous note \"dup\": matches a/dup.md, b/dup.md; use the full relative path\n"
    );

    let output = capture(&vault, &day_file, &["!sase:dup"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "bob capture: capture item 1 starting on line 1: block ID ^dup appears 2 times in sase.md; make it unique before capturing\n"
    );

    assert_eq!(vault_bytes(&vault), before, "refusals must not write");
}

#[test]
fn ledger_json_carries_text_struck_in_and_dropped() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-ledger");
    let day_file = vault.join("20261005.md");

    // Running-session strike.
    write_file(
        &vault.join("sase.md"),
        "- [*] #task Fix flaky gkeep test ^fix-flaky\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^fix-flaky]]\n",
    );
    let json = capture_json(&vault, &day_file, &["!sase:fix-flaky"]);
    assert_eq!(json["task_complete"]["text"], "Fix flaky gkeep test");
    let ledger = &json["task_complete"]["ledger"];
    assert_eq!(ledger["struck"], 1);
    assert_eq!(
        ledger["struck_in"],
        serde_json::json!([{"line": 2, "name": "CAPTURE", "status": "running"}])
    );
    assert_eq!(ledger["moved"], serde_json::json!([]));
    assert_eq!(ledger["dropped"], serde_json::json!([]));

    // Placeholder move with removal.
    write_file(
        &vault.join("sase.md"),
        "- [ ] #task Other live ^other\n- [ ] #task Move me ^mv1\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^other]]\n- [ ] () \u{2014} SASE\n  - [[sase#^mv1]]\n",
    );
    let json = capture_json(&vault, &day_file, &["!sase:mv1"]);
    assert_eq!(json["task_complete"]["text"], "Move me");
    let ledger = &json["task_complete"]["ledger"];
    assert_eq!(ledger["struck"], 1);
    assert_eq!(
        ledger["struck_in"],
        serde_json::json!([{"line": 4, "name": "SASE", "status": "queued"}])
    );
    assert_eq!(
        ledger["moved"],
        serde_json::json!([{"from": {"line": 4, "name": "SASE", "status": "queued"}, "to": {"line": 2, "name": "CAPTURE", "status": "running"}}])
    );

    // Dedupe.
    write_file(&vault.join("sase.md"), "- [*] #task Carried ^carry\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [x] (0800-0830) \u{2014} MORNING\n  - ~~[[sase#^carry]]~~\n- [ ] () \u{2014} SASE\n  - [[sase#^carry]]\n",
    );
    let json = capture_json(&vault, &day_file, &["!sase:carry"]);
    assert_eq!(json["task_complete"]["text"], "Carried");
    let ledger = &json["task_complete"]["ledger"];
    assert_eq!(ledger["deduplicated"], 1);
    assert_eq!(
        ledger["dropped"],
        serde_json::json!([{"from": {"line": 4, "name": "SASE", "status": "queued"}, "to": {"line": 2, "name": "MORNING", "status": "completed"}}])
    );

    // Already done carries clean text and no ledger.
    write_file(&vault.join("sase.md"), "- [x] #task Finished ^fin\n");
    let json = capture_json(&vault, &day_file, &["!sase:fin"]);
    assert_eq!(json["task_complete"]["action"], "already_done");
    assert_eq!(json["task_complete"]["text"], "Finished");
}

#[test]
fn exact_human_output_for_strike_move_dedupe_and_done() {
    // Running-session strike, real and dry run.
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-exact1");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [*] #task Fix flaky gkeep test ^fix-flaky\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^fix-flaky]]\n",
    );
    let output = capture_human(&vault, &day_file, &["!sase:fix-flaky"], false);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "\u{2713} completed [*] \u{2192} [x] Fix flaky gkeep test  sase.md ^fix-flaky\n  ledger  Task Link struck in CAPTURE \u{00B7} 20261005.md\nplan 1/3 themes \u{00B7} 0/10 links\n"
    );

    // Strike under a completed entry.
    let (_temp, vault2) = vault_with_settings("bob-cli-task-complete-exact2");
    let day2 = vault2.join("20261005.md");
    write_file(&vault2.join("sase.md"), "- [*] #task Plan task ^plantask\n");
    write_file(
        &day2,
        "## Pomodoros\n- [x] (0800-0830) \u{2014} PLAN\n  - [[sase#^plantask]]\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let output = capture_human(&vault2, &day2, &["!sase:plantask"], false);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "\u{2713} completed [*] \u{2192} [x] Plan task  sase.md ^plantask\n  ledger  Task Link struck in PLAN (completed) \u{00B7} removed empty CAPTURE \u{00B7} 20261005.md\nplan 0/3 themes \u{00B7} 0/10 links\n"
    );

    // Placeholder move with removal.
    let (_temp, vault3) = vault_with_settings("bob-cli-task-complete-exact3");
    let day3 = vault3.join("20261005.md");
    write_file(
        &vault3.join("sase.md"),
        "- [ ] #task Other live ^other\n- [ ] #task Move me ^mv1\n",
    );
    write_file(
        &day3,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^other]]\n- [ ] () \u{2014} SASE\n  - [[sase#^mv1]]\n",
    );
    let output = capture_human(&vault3, &day3, &["!sase:mv1"], false);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "\u{2713} completed [ ] \u{2192} [x] Move me  sase.md ^mv1\n  ledger  Task Link struck in SASE \u{00B7} Task Link moved SASE \u{2192} CAPTURE (struck) \u{00B7} removed empty SASE \u{00B7} 20261005.md\nplan 1/3 themes \u{00B7} 1/10 links\n"
    );

    // Dedupe.
    let (_temp, vault4) = vault_with_settings("bob-cli-task-complete-exact4");
    let day4 = vault4.join("20261005.md");
    write_file(&vault4.join("sase.md"), "- [*] #task Carried ^carry\n");
    write_file(
        &day4,
        "## Pomodoros\n- [x] (0800-0830) \u{2014} MORNING\n  - ~~[[sase#^carry]]~~\n- [ ] () \u{2014} SASE\n  - [[sase#^carry]]\n",
    );
    let output = capture_human(&vault4, &day4, &["!sase:carry"], false);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "\u{2713} completed [*] \u{2192} [x] Carried  sase.md ^carry\n  ledger  Task Link already in MORNING; dropped the SASE copy \u{00B7} removed empty SASE \u{00B7} 20261005.md\nplan 0/3 themes \u{00B7} 0/10 links\n"
    );

    // Already done.
    let (_temp, vault5) = vault_with_settings("bob-cli-task-complete-exact5");
    let day5 = vault5.join("20261005.md");
    write_file(&vault5.join("sase.md"), "- [x] #task Finished ^fin\n");
    write_file(
        &day5,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let output = capture_human(&vault5, &day5, &["!sase:fin"], false);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "\u{2713} already done [x] Finished  sase.md ^fin \u{00B7} nothing to change\n"
    );
}

#[test]
fn exact_human_output_for_subtasks_and_unblocked() {
    // Subtasks plus left open: no ledger line, no plan meter.
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-exact6");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join("sase.md"),
        "- [?] #task Blocked root ^root\n\t- ![[sase#^sub1]]\n\t- ![[sase#^sub2]]\n- [/] #task Sub one ^sub1\n- [?] #task Sub two ^sub2\n\t- ![[sase#^sub3]]\n- [ ] #task Sub three ^sub3\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let output = capture_human(&vault, &day_file, &["!sase:root"], false);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "\u{2713} completed [?] \u{2192} [x] Blocked root  sase.md ^root\n    [/] \u{2192} [x] Sub one  sase.md ^sub1\n    left Blocked [?] Sub two  sase.md ^sub2\n"
    );

    // Unblocked dependent: one unblocked row, no ledger line.
    let (_temp, vault2) = vault_with_settings("bob-cli-task-complete-exact7");
    let day2 = vault2.join("20261005.md");
    write_file(&vault2.join("sase.md"), "- [?] #task Blocked root ^root\n");
    write_file(
        &vault2.join("travel.md"),
        "- [ ] #task Dep root [id:: dep-root] ^dep\n- [?] #task Waiter [dependsOn:: dep-root] [id:: waiter] ^waiter\n",
    );
    write_file(
        &day2,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let _ = capture_json(&vault2, &day2, &["!sase:root"]);
    let output = capture_human(&vault2, &day2, &["!travel:dep"], false);
    assert_success(&output);
    assert_eq!(
        stdout(&output),
        "\u{2713} completed [ ] \u{2192} [x] Dep root  travel.md ^dep\n  unblocked [?] \u{2192} [ ] Waiter  travel.md ^waiter\n"
    );
}

#[test]
fn custom_global_filter_yields_clean_text_and_human_output() {
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-filter");
    let day_file = vault.join("20261005.md");
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        "{\n  \"globalFilter\": \"#todo\",\n  \"statusSettings\": {\n    \"coreStatuses\": [\n      {\"symbol\":\" \",\"name\":\"Ready\",\"type\":\"TODO\"},\n      {\"symbol\":\"x\",\"name\":\"Done\",\"type\":\"DONE\"}\n    ],\n    \"customStatuses\": [\n      {\"symbol\":\"*\",\"name\":\"Next\",\"type\":\"ON_HOLD\"},\n      {\"symbol\":\"?\",\"name\":\"Blocked\",\"type\":\"TODO\"},\n      {\"symbol\":\"/\",\"name\":\"In Progress\",\"type\":\"IN_PROGRESS\"},\n      {\"symbol\":\"-\",\"name\":\"Canceled\",\"type\":\"CANCELLED\"}\n    ]\n  }\n}",
    );
    write_file(
        &vault.join("sase.md"),
        "- [*] #todo Fix with custom filter ^fixcustom\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^fixcustom]]\n",
    );
    // Dry run first: it reports without writing, so the task is still open.
    let output = capture_human(&vault, &day_file, &["!sase:fixcustom"], true);
    assert_success(&output);
    assert!(
        stdout(&output).starts_with(
            "[dry-run] ok would complete [*] \u{2192} [x] Fix with custom filter  sase.md ^fixcustom\n"
        ),
        "{}",
        format_output(&output)
    );
    assert!(
        !stdout(&output).contains("#todo"),
        "{}",
        format_output(&output)
    );
    let json = capture_json(&vault, &day_file, &["!sase:fixcustom"]);
    assert_eq!(json["task_complete"]["text"], "Fix with custom filter");
}

#[test]
fn hello_ignores_poisoned_note_only_discovery_would_touch() {
    // Lazy `DependencyContext`: a plain capture never scans note
    // contents, so an invalid-UTF-8 note that only a vault-wide scan
    // would touch succeeds silently.
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-poison");
    let day_file = vault.join("20261005.md");
    fs::write(
        vault.join("poison.md"),
        b"\xff\xfe invalid \xfe\xff [dependsOn:: ghost] [id:: poison]",
    )
    .expect("write poison note");
    let json = capture_json(&vault, &day_file, &["hello"]);
    assert_eq!(json["ok"], true);
    if let Some(warnings) = json.get("warnings") {
        assert!(!warnings.to_string().contains("poison"), "{warnings}");
    }
}

#[test]
fn close_completing_nothing_ignores_poisoned_note() {
    // Same laziness for `=x`: closing a session with no embedded tasks
    // completes nothing and never scans note contents.
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-poison-x");
    let day_file = vault.join("20261005.md");
    write_file(&vault.join("sase.md"), "- [ ] #task Open work ^work\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n  - [[sase#^work]]\n",
    );
    fs::write(
        vault.join("poison.md"),
        b"\xff\xfe invalid \xfe\xff [dependsOn:: ghost] [id:: poison]",
    )
    .expect("write poison note");
    let json = capture_json(&vault, &day_file, &["=x"]);
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_close");
    if let Some(warnings) = json.get("warnings") {
        assert!(!warnings.to_string().contains("poison"), "{warnings}");
    }
}

#[test]
fn complete_without_id_skips_dependent_lookup() {
    // `[id::]` gate: completing a task without `[id::]` neither reads
    // nor reports dependents, even when one names the canonical id.
    // (Successor-links gate edge: that stale dependent is left to the
    // hooks, which stamp the target's `[id::]` on their next run.)
    let (_temp, vault) = vault_with_settings("bob-cli-task-complete-gate");
    let day_file = vault.join("20261005.md");
    write_file(&vault.join("sase.md"), "- [?] #task Plain root ^root\n");
    write_file(
        &vault.join("travel.md"),
        "- [?] #task Canonical waiter [dependsOn:: sase__root] ^waiter\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] (**0920-0950**) \u{2014} CAPTURE\n",
    );
    let json = capture_json(&vault, &day_file, &["!sase:root"]);
    assert_eq!(json["status_symbol"], "x");
    assert_eq!(json["task_complete"]["unblocked"], serde_json::json!([]));
    assert!(
        fs::read_to_string(vault.join("travel.md"))
            .expect("read travel")
            .contains("- [?] #task Canonical waiter [dependsOn:: sase__root] ^waiter\n"),
        "the canonical-only dependent stays Blocked for the hooks"
    );
}
