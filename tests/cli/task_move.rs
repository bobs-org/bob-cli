//! CLI coverage for task-move command routing, dry-run isolation, and writes.

use std::{
    fs,
    fs::OpenOptions,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use fs2::FileExt;

use crate::support::*;

fn configured_move(temp: &TempDir, vault: &Path, args: &[&str]) -> Command {
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("create isolated home");
    let mut command = bob_command();
    command
        .args(["task", "move"])
        .args(args)
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
    command
}

fn basic_vault(temp: &TempDir) -> PathBuf {
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move this task [id:: source__move-me] [fresh:: 2026-10-01] [keeps:: 2] [unblocked:: 2026-10-02] ^move-me\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    write_file(
        &vault.join("2026/20261019.md"),
        "## Pomodoros\n- [ ] ()\n  - [[source#^move-me|task]]\n",
    );
    write_file(
        &vault.join("ref/design.md"),
        "Related: [[source#^move-me|design]].\n",
    );
    vault
}

fn run_json(temp: &TempDir, vault: &Path, args: &[&str]) -> Output {
    configured_move(temp, vault, args)
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
fn help_lists_sorted_options_and_short_aliases() {
    let output = bob_command()
        .args(["task", "move", "--help"])
        .output()
        .expect("run bob task move --help");
    assert_success(&output);
    let help = stdout(&output);
    let expected = [
        "-b, --block",
        "-d, --dry-run",
        "-f, --format",
        "-h, --help",
        "-m, --match",
        "-o, --offline",
        "-r, --retry-timeout",
    ];
    let mut previous = 0;
    for option in expected {
        let position = help
            .find(option)
            .unwrap_or_else(|| panic!("missing {option}:\n{help}"));
        assert!(position >= previous, "options are out of order:\n{help}");
        previous = position;
    }
    assert!(help.contains("<SOURCE> <DEST>"), "{help}");
    assert!(help.contains("literal text"), "{help}");
}

#[test]
fn dry_run_works_while_lock_is_held_and_writes_no_state_or_notes() {
    let temp = TempDir::new("bob-task-move-dry-run");
    let vault = basic_vault(&temp);
    let source_before = fs::read(vault.join("source.md")).expect("read source");
    let dest_before =
        fs::read(vault.join("dest.md")).expect("read destination");
    let lock_path = temp.path().join("bob_sync.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .expect("open held lock file");
    lock.lock_exclusive().expect("hold maintenance lock");

    let output = run_json(
        &temp,
        &vault,
        &["--dry-run", "-f", "json", "source", "dest", "-b", "move-me"],
    );
    FileExt::unlock(&lock).expect("release maintenance lock");
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["moves"].as_array().unwrap().len(), 1);
    assert_eq!(json["moves"][0]["title"], "Move this task");
    assert_eq!(json["git"]["mode"], "dry_run");
    assert!(json["git"]["pre_sync"].is_null());
    assert!(json["git"]["commit"].is_null());
    assert!(json["git"]["post_sync"].is_null());
    assert!(json["applied_paths"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read(vault.join("source.md")).expect("read source after"),
        source_before
    );
    assert_eq!(
        fs::read(vault.join("dest.md")).expect("read destination after"),
        dest_before
    );
    assert!(
        !temp.path().join("state").exists(),
        "dry run created state home"
    );
    assert!(
        !temp.path().join("vault-sync.json").exists(),
        "dry run wrote sync state"
    );
    assert_eq!(fs::metadata(&lock_path).expect("lock metadata").len(), 0);
    assert!(stderr(&output).is_empty(), "{}", format_output(&output));
}

#[test]
fn dry_run_reports_operational_plan_errors_as_json() {
    let temp = TempDir::new("bob-task-move-plan-error");
    let vault = basic_vault(&temp);
    let output = run_json(
        &temp,
        &vault,
        &["--dry-run", "-f", "json", "source", "dest", "-b", "missing"],
    );
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["error"]["stage"], "plan");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("matched no movable task"));
}

#[test]
fn live_zero_budget_refuses_a_held_lock_before_writing() {
    let temp = TempDir::new("bob-task-move-held-lock");
    let vault = basic_vault(&temp);
    let source_before = fs::read(vault.join("source.md")).expect("read source");
    let lock_path = temp.path().join("bob_sync.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .expect("open held lock file");
    lock.lock_exclusive().expect("hold maintenance lock");
    let output = run_json(
        &temp,
        &vault,
        &["-r", "0", "-f", "json", "source", "dest", "-b", "move-me"],
    );
    FileExt::unlock(&lock).expect("release maintenance lock");
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["error"]["stage"], "lock");
    assert_eq!(
        fs::read(vault.join("source.md")).expect("read source after"),
        source_before
    );
    assert!(!fs::read_to_string(vault.join("dest.md"))
        .unwrap()
        .contains("Move this task"));
}

#[test]
fn pre_sync_failure_does_not_write_move_data() {
    let temp = TempDir::new("bob-task-move-pre-sync-failure");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move this task ^move-me\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    git_in(&vault, ["add", "source.md", "dest.md"]);
    git_in(&vault, ["commit", "-q", "-m", "pre-sync fixture"]);
    let source_before = fs::read(vault.join("source.md")).expect("read source");
    let invalid_state_path = temp.path().join("state-is-a-directory");
    fs::create_dir_all(&invalid_state_path)
        .expect("create invalid state destination");

    let output = configured_move(
        &temp,
        &vault,
        &["-f", "json", "source", "dest", "-b", "move-me"],
    )
    .env("BOB_VAULT_SYNC_STATE_FILE", &invalid_state_path)
    .output()
    .expect("run pre-sync failure case");
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["error"]["stage"], "pre_sync");
    assert_eq!(json["git"]["pre_sync"]["ok"], false);
    assert!(json["git"]["commit"].is_null());
    assert!(json["git"]["post_sync"].is_null());
    assert_eq!(
        fs::read(vault.join("source.md")).expect("read source after"),
        source_before
    );
    assert!(!fs::read_to_string(vault.join("dest.md"))
        .unwrap()
        .contains("Move this task"));
}

#[test]
fn offline_git_move_commits_only_repaired_paths_and_preserves_task_state() {
    let temp = TempDir::new("bob-task-move-offline");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move this task [id:: source__move-me] [fresh:: 2026-10-01] [keeps:: 2] [unblocked:: 2026-10-02] ^move-me\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Existing destination block ^move-me\n\nSee its local link [[#^move-me]].\n",
    );
    write_file(
        &vault.join("2026/20261019.md"),
        "## Pomodoros\n- [ ] ()\n  - [[source#^move-me|task]]\n",
    );
    write_file(
        &vault.join("ref/design.md"),
        "Related: [[source#^move-me|design]].\n",
    );
    write_file(
        &vault.join("dependent.md"),
        "- [ ] #task Needs moved task [dependsOn:: source__move-me] ^dependent\n  - ⛓️ **DEPENDS ON:** [[source#^move-me]]\n",
    );
    git_in(
        &vault,
        [
            "add",
            "source.md",
            "dest.md",
            "2026/20261019.md",
            "ref/design.md",
            "dependent.md",
        ],
    );
    git_in(&vault, ["commit", "-q", "-m", "move fixture"]);
    let remote_before = git([
        "--git-dir",
        path_str(&remote),
        "rev-parse",
        "refs/heads/master",
    ]);
    write_file(&vault.join("unrelated-staged.md"), "keep this staged\n");
    write_file(&vault.join("unrelated-dirty.md"), "leave me dirty\n");
    git_in(&vault, ["add", "unrelated-staged.md"]);

    let output = run_json(
        &temp,
        &vault,
        &["-o", "-f", "json", "source", "dest", "-b", "move-me"],
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true, "{}", format_output(&output));
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["git"]["mode"], "offline");
    assert!(json["git"]["pre_sync"].is_null());
    assert!(json["git"]["post_sync"].is_null());
    assert!(json["git"]["commit"]["sha"].as_str().is_some());
    assert_eq!(
        json["git"]["commit"]["subject"],
        "bob task move source.md -> dest.md (1 task)"
    );
    let paths = json["git"]["commit"]["paths"].as_array().unwrap();
    for path in [
        "dest.md",
        "2026/20261019.md",
        "dependent.md",
        "ref/design.md",
        "source.md",
    ] {
        assert!(
            paths.iter().any(|value| value == path),
            "missing {path}: {paths:?}"
        );
    }
    assert_eq!(
        paths.first().and_then(|value| value.as_str()),
        Some("dest.md")
    );
    assert_eq!(
        paths.last().and_then(|value| value.as_str()),
        Some("source.md")
    );
    assert_eq!(
        fs::read_to_string(vault.join("source.md"))
            .unwrap()
            .contains("Move this task"),
        false
    );
    let dest =
        fs::read_to_string(vault.join("dest.md")).expect("read destination");
    assert!(dest.contains("[id:: dest__move-me-1]"), "{dest}");
    assert!(dest.contains("^move-me-1"), "{dest}");
    assert!(
        dest.contains("[[#^move-me]]"),
        "destination-local link changed: {dest}"
    );
    for preserved in [
        "[fresh:: 2026-10-01]",
        "[keeps:: 2]",
        "[unblocked:: 2026-10-02]",
    ] {
        assert!(
            dest.contains(preserved),
            "missing preserved field {preserved}: {dest}"
        );
    }
    assert!(fs::read_to_string(vault.join("dependent.md"))
        .unwrap()
        .contains("[dependsOn:: dest__move-me-1]"));
    assert!(fs::read_to_string(vault.join("dependent.md"))
        .unwrap()
        .contains("[[dest#^move-me-1]]"));
    assert!(fs::read_to_string(vault.join("ref/design.md"))
        .unwrap()
        .contains("[[dest#^move-me-1|design]]"));
    assert!(fs::read_to_string(vault.join("2026/20261019.md"))
        .unwrap()
        .contains("[[dest#^move-me-1|task]]"));
    assert!(fs::read_to_string(vault.join("unrelated-dirty.md"))
        .unwrap()
        .contains("leave me dirty"));
    assert!(fs::read_to_string(vault.join("unrelated-staged.md"))
        .unwrap()
        .contains("keep this staged"));
    let status = git_in(&vault, ["status", "--short"]);
    assert!(
        stdout(&status).contains("A  unrelated-staged.md")
            && stdout(&status).contains("?? unrelated-dirty.md"),
        "{}",
        stdout(&status)
    );
    let subject = git_in(&vault, ["log", "-1", "--format=%s"]);
    assert_eq!(
        stdout(&subject).trim(),
        "bob task move source.md -> dest.md (1 task)"
    );
    let remote_after = git([
        "--git-dir",
        path_str(&remote),
        "rev-parse",
        "refs/heads/master",
    ]);
    assert_eq!(
        stdout(&remote_after),
        stdout(&remote_before),
        "offline move pushed to remote"
    );
}

#[test]
fn non_git_live_move_writes_and_reports_null_git_stages() {
    let temp = TempDir::new("bob-task-move-non-git");
    let vault = basic_vault(&temp);
    let output = run_json(
        &temp,
        &vault,
        &["-f", "json", "source", "dest", "-b", "move-me"],
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true, "{}", format_output(&output));
    assert_eq!(json["git"]["mode"], "not_a_worktree");
    assert!(json["git"]["pre_sync"].is_null());
    assert!(json["git"]["commit"].is_null());
    assert!(json["git"]["post_sync"].is_null());
    assert!(json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .unwrap()
            .contains("not a git worktree")));
    assert!(fs::read_to_string(vault.join("dest.md"))
        .unwrap()
        .contains("Move this task"));
    assert!(!fs::read_to_string(vault.join("source.md"))
        .unwrap()
        .contains("Move this task"));
    assert!(
        stderr(&output).contains("warning:"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn post_sync_failure_keeps_the_scoped_local_commit_and_reports_failure() {
    let temp = TempDir::new("bob-task-move-post-sync-failure");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move this task ^move-me\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    git_in(&vault, ["add", "source.md", "dest.md"]);
    git_in(&vault, ["commit", "-q", "-m", "post-sync fixture"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    let hook = remote.join("hooks/pre-receive");
    write_file(&hook, "#!/bin/sh\nexit 1\n");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
        .expect("make rejecting hook executable");

    let output = run_json(
        &temp,
        &vault,
        &["-f", "json", "source", "dest", "-b", "move-me"],
    );
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["stage"], "post_sync");
    assert_eq!(json["git"]["post_sync"]["ok"], false);
    assert!(json["git"]["commit"]["sha"].as_str().is_some());
    assert!(fs::read_to_string(vault.join("dest.md"))
        .unwrap()
        .contains("Move this task"));
    assert!(!fs::read_to_string(vault.join("source.md"))
        .unwrap()
        .contains("Move this task"));
}
