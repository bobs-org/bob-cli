//! CLI coverage for `bob task merge` and `bob task move -t`.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::support::*;

fn configured(
    temp: &TempDir,
    vault: &Path,
    args: &[&str],
    day: Option<&Path>,
) -> Command {
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("create isolated home");
    let mut command = bob_command();
    command.args(["task"]).args(args);
    command
        .env("BOB_DIR", vault)
        .env("HOME", home)
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("NO_COLOR", "1");
    if let Some(day) = day {
        command.env("BOB_DAY_FILE", day);
    }
    command
}

fn merge_vault(temp: &TempDir) -> PathBuf {
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("inbox.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Bench task ^bench\n\t- Try pgbench at 10k TPS\n",
    );
    write_file(
        &vault.join("db.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Upgrade Postgres ^upgrade\n\t- FUTURE WORK\n\t\t- Try logical replication\n",
    );
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\n---\n\n## Future Work\n\n- existing note\n\n## Tasks\n\n- [ ] #task Dev task ^devtask\n",
    );
    vault
}

fn run_merge(
    temp: &TempDir,
    vault: &Path,
    args: &[&str],
    day: Option<&Path>,
) -> Output {
    let mut full = vec!["merge"];
    full.extend_from_slice(args);
    configured(temp, vault, &full, day)
        .output()
        .unwrap_or_else(|error| panic!("run bob task merge {args:?}: {error}"))
}

fn run_move_t(temp: &TempDir, vault: &Path, args: &[&str]) -> Output {
    let mut full = vec!["move"];
    full.extend_from_slice(args);
    configured(temp, vault, &full, None)
        .output()
        .unwrap_or_else(|error| panic!("run bob task move {args:?}: {error}"))
}

fn parse_json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("parse command JSON: {error}\n{}", format_output(output))
    })
}

fn assert_exit(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        format_output(output)
    );
}

#[test]
fn merge_into_task_reports_canonical_human_and_json() {
    let temp = TempDir::new("bob-task-merge-task");
    let vault = merge_vault(&temp);
    let output =
        run_merge(&temp, &vault, &["-i", "db:upgrade", "inbox:bench"], None);
    assert_exit(&output, 0);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(stdout.contains("⤵"), "{stdout}");
    assert!(stdout.contains("Upgrade Postgres"), "{stdout}");
    assert!(stdout.contains("Bench task"), "{stdout}");

    let json_output = run_merge(
        &temp,
        &vault,
        &["-o", "-f", "json", "-i", "db:upgrade", "inbox:bench"],
        None,
    );
    // Second run sees the already-merged tombstone and reports unchanged.
    assert_exit(&json_output, 0);
    let json = parse_json(&json_output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["merges"][0]["outcome"], "unchanged");
}

#[test]
fn merge_into_task_writes_payload_and_retires_source() {
    let temp = TempDir::new("bob-task-merge-write");
    let vault = merge_vault(&temp);
    let output = run_merge(
        &temp,
        &vault,
        &["-o", "-i", "db:upgrade", "inbox:bench"],
        None,
    );
    assert_exit(&output, 0);
    let target = fs::read_to_string(vault.join("db.md")).expect("read db");
    assert!(target.contains("Bench task"), "{target}");
    assert!(target.contains("Try pgbench"), "{target}");
    let source =
        fs::read_to_string(vault.join("inbox.md")).expect("read inbox");
    assert!(source.contains("[-]"), "{source}");
    assert!(source.contains("merged into"), "{source}");
}

#[test]
fn merge_into_drawer_uses_existing_drawer() {
    let temp = TempDir::new("bob-task-merge-drawer");
    let vault = merge_vault(&temp);
    let output = run_merge(
        &temp,
        &vault,
        &["-o", "-i", "db:upgrade", "-D", "FUTURE WORK", "inbox:bench"],
        None,
    );
    assert_exit(&output, 0);
    let target = fs::read_to_string(vault.join("db.md")).expect("read db");
    assert!(target.contains("Bench task"), "{target}");
    assert!(target.contains("FUTURE WORK"), "{target}");
    let json_output = run_merge(
        &temp,
        &vault,
        &[
            "-o",
            "-f",
            "json",
            "-i",
            "db:upgrade",
            "-D",
            "FUTURE WORK",
            "inbox:other-missing",
        ],
        None,
    );
    assert_exit(&json_output, 1);
}

#[test]
fn merge_into_created_drawer() {
    let temp = TempDir::new("bob-task-merge-new-drawer");
    let vault = merge_vault(&temp);
    let output = run_merge(
        &temp,
        &vault,
        &["-o", "-i", "db:upgrade", "-D", "RESEARCH", "inbox:bench"],
        None,
    );
    assert_exit(&output, 0);
    let target = fs::read_to_string(vault.join("db.md")).expect("read db");
    assert!(target.contains("RESEARCH"), "{target}");
    assert!(target.contains("Bench task"), "{target}");
}

#[test]
fn merge_into_section_and_created_section() {
    let temp = TempDir::new("bob-task-merge-section");
    let vault = merge_vault(&temp);
    let output = run_merge(
        &temp,
        &vault,
        &["-o", "-i", "dev", "-s", "Future Work", "inbox:bench"],
        None,
    );
    assert_exit(&output, 0);
    let target = fs::read_to_string(vault.join("dev.md")).expect("read dev");
    assert!(target.contains("Bench task"), "{target}");

    let temp = TempDir::new("bob-task-merge-new-section");
    let vault = merge_vault(&temp);
    let output = run_merge(
        &temp,
        &vault,
        &["-o", "-i", "dev", "-s", "Someday", "inbox:bench"],
        None,
    );
    assert_exit(&output, 0);
    let target = fs::read_to_string(vault.join("dev.md")).expect("read dev");
    assert!(target.contains("## Someday"), "{target}");
    assert!(target.contains("Bench task"), "{target}");
}

#[test]
fn merge_into_prj_h2_drawer() {
    let temp = TempDir::new("bob-task-merge-prj");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("inbox.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Bench task ^bench\n",
    );
    write_file(
        &vault.join("proj.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n\n## Tasks\n\n- [ ] #task Project work ^prj\n",
    );
    let output = run_merge(
        &temp,
        &vault,
        &["-o", "-i", "proj:prj", "-D", "Plans", "inbox:bench"],
        None,
    );
    assert_exit(&output, 0);
    let target = fs::read_to_string(vault.join("proj.md")).expect("read proj");
    assert!(target.contains("Bench task"), "{target}");
}

#[test]
fn merge_teaching_refusals_have_stable_codes() {
    let temp = TempDir::new("bob-task-merge-teaching");
    let vault = merge_vault(&temp);
    let output = run_merge(
        &temp,
        &vault,
        &["-f", "json", "-i", "dev", "inbox:bench"],
        None,
    );
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["error"]["code"], "merge_into_note_without_section");

    let output = run_move_t(
        &temp,
        &vault,
        &["-f", "json", "-t", "db:upgrade", "inbox:bench"],
    );
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["error"]["code"], "move_to_task");
}

#[test]
fn merge_operational_refusals_refuse_the_whole_operation() {
    let temp = TempDir::new("bob-task-merge-refusals");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("src.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [-] #task Closed source ^src\n\n- [ ] #task Open parent ^parent\n\t- [ ] #task Nested child ^child\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [-] #task Closed target ^dst\n\n- [ ] #task Live target ^live\n",
    );
    let before_src =
        fs::read_to_string(vault.join("src.md")).expect("read src");
    let before_dest =
        fs::read_to_string(vault.join("dest.md")).expect("read dest");

    for args in [
        vec!["-f", "json", "-i", "dest:live", "src:src"],
        vec!["-f", "json", "-i", "dest:dst", "src:parent"],
        vec!["-f", "json", "-i", "dest:live", "src:parent"],
        vec!["-f", "json", "-i", "src:child", "src:parent"],
    ] {
        let output = run_merge(&temp, &vault, &args, None);
        assert_exit(&output, 1);
        let json = parse_json(&output);
        assert!(json["error"]["code"].is_string(), "{json}");
    }
    assert_eq!(
        fs::read_to_string(vault.join("src.md")).expect("read src"),
        before_src
    );
    assert_eq!(
        fs::read_to_string(vault.join("dest.md")).expect("read dest"),
        before_dest
    );
}

#[test]
fn merge_section_with_dependents_is_refused() {
    let temp = TempDir::new("bob-task-merge-section-dep");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("src.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Source ^src\n",
    );
    write_file(
        &vault.join("dep.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Dependent ^dep\n\t- ⛓️ **DEPENDS ON:** [[src#^src]]\n",
    );
    write_file(
        &vault.join("dev.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Dev ^dev\n",
    );
    let output = run_merge(
        &temp,
        &vault,
        &[
            "-f",
            "json",
            "-o",
            "-i",
            "dev",
            "-s",
            "Future Work",
            "src:src",
        ],
        None,
    );
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["error"]["code"], "merge_section_has_dependents");
}

#[test]
fn merge_pull_in_pick_links_the_target() {
    let temp = TempDir::new("bob-task-merge-pullin");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("inbox.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Bench task ^bench\n",
    );
    write_file(
        &vault.join("db.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Upgrade Postgres ^upgrade\n",
    );
    let day = vault.join("daily/2026-11-03.md");
    write_file(
        &day,
        "## Pomodoros\n- [ ] () — BUILD\n\t- [[inbox#^bench]]\n",
    );
    let output = run_merge(
        &temp,
        &vault,
        &[
            "-o",
            "-f",
            "json",
            "-p",
            "1",
            "-i",
            "db:upgrade",
            "inbox:bench",
        ],
        Some(&day),
    );
    assert_exit(&output, 0);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["pull_in"]["picked_count"], 1);
    let day_text = fs::read_to_string(&day).expect("read day");
    assert!(day_text.contains("db#^upgrade"), "{day_text}");
}

#[test]
fn merge_unpicked_offer_prints_a_capture_hint() {
    let temp = TempDir::new("bob-task-merge-hint");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("inbox.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Bench task ^bench\n",
    );
    write_file(
        &vault.join("db.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Upgrade Postgres ^upgrade\n",
    );
    let day = vault.join("daily/2026-11-03.md");
    write_file(
        &day,
        "## Pomodoros\n- [ ] () — BUILD\n\t- [[inbox#^bench]]\n",
    );
    let output = run_merge(
        &temp,
        &vault,
        &["-o", "-i", "db:upgrade", "inbox:bench"],
        Some(&day),
    );
    assert_exit(&output, 0);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(stdout.contains("bob capture"), "{stdout}");
}

#[test]
fn merge_rerun_is_unchanged_with_no_commit() {
    let temp = TempDir::new("bob-task-merge-rerun");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("inbox.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Bench task ^bench\n",
    );
    write_file(
        &vault.join("db.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Upgrade Postgres ^upgrade\n",
    );
    git_in(&vault, ["init"]);
    configure_test_git_identity(&vault);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-m", "seed"]);
    let first = run_merge(
        &temp,
        &vault,
        &["-o", "-i", "db:upgrade", "inbox:bench"],
        None,
    );
    assert_exit(&first, 0);
    let count = |output: Output| {
        String::from_utf8_lossy(&output.stdout).lines().count()
    };
    let _ = count;
    let log = git_in(&vault, ["rev-list", "--count", "HEAD"]);
    let before = String::from_utf8_lossy(&log.stdout).trim().to_string();
    let second = run_merge(
        &temp,
        &vault,
        &["-o", "-f", "json", "-i", "db:upgrade", "inbox:bench"],
        None,
    );
    assert_exit(&second, 0);
    let json = parse_json(&second);
    assert_eq!(json["merges"][0]["outcome"], "unchanged");
    let log = git_in(&vault, ["rev-list", "--count", "HEAD"]);
    let after = String::from_utf8_lossy(&log.stdout).trim().to_string();
    assert_eq!(before, after);
}

#[test]
fn merge_dry_run_matches_live_except_for_writes() {
    let temp = TempDir::new("bob-task-merge-parity");
    let vault = merge_vault(&temp);
    let dry = run_merge(
        &temp,
        &vault,
        &["-f", "json", "-d", "-i", "db:upgrade", "inbox:bench"],
        None,
    );
    assert_exit(&dry, 0);
    let dry_json = parse_json(&dry);
    assert_eq!(dry_json["dry_run"], true);
    assert!(!fs::read_to_string(vault.join("inbox.md"))
        .expect("read inbox")
        .contains("[-]"));
    let live = run_merge(
        &temp,
        &vault,
        &["-o", "-f", "json", "-i", "db:upgrade", "inbox:bench"],
        None,
    );
    assert_exit(&live, 0);
    let live_json = parse_json(&live);
    assert_eq!(live_json["dry_run"], false);
    assert_eq!(dry_json["merges"], live_json["merges"]);
}

#[test]
fn move_t_moves_many_tasks_in_typed_order() {
    let temp = TempDir::new("bob-task-move-t");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("a.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Alpha ^a\n",
    );
    write_file(
        &vault.join("b.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Bravo ^b\n",
    );
    write_file(
        &vault.join("db.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    let output = run_move_t(
        &temp,
        &vault,
        &["-o", "-f", "json", "-t", "db", "b:b", "a:a"],
    );
    assert_exit(&output, 0);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["moves"].as_array().map(Vec::len), Some(2));
    let target = fs::read_to_string(vault.join("db.md")).expect("read db");
    let alpha = target.find("Alpha").expect("alpha moved");
    let bravo = target.find("Bravo").expect("bravo moved");
    assert!(bravo < alpha, "typed order kept: {target}");
}

#[test]
fn move_t_same_note_is_refused() {
    let temp = TempDir::new("bob-task-move-t-same");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("db.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Alpha ^a\n",
    );
    let output =
        run_move_t(&temp, &vault, &["-o", "-f", "json", "-t", "db", "db:a"]);
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["error"]["code"], "move_same_note");
}
