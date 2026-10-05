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
