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

#[test]
fn mixed_selectors_dedup_and_keep_source_order() {
    let temp = TempDir::new("bob-task-move-mixed-selectors");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Alpha ^a\n- [ ] #task Bravo ^b\n- [ ] #task Charlie ^c\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    let output = run_json(
        &temp,
        &vault,
        &[
            "-f", "json", "source", "dest", "-b", "c", "-m", "Alpha", "-b",
            "a", "-b", "b",
        ],
    );
    assert_success(&output);
    let json = parse_json(&output);
    let titles: Vec<&str> = json["moves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Alpha", "Bravo", "Charlie"]);
    let dest =
        fs::read_to_string(vault.join("dest.md")).expect("read destination");
    let alpha = dest.find("Alpha").expect("alpha");
    let bravo = dest.find("Bravo").expect("bravo");
    let charlie = dest.find("Charlie").expect("charlie");
    assert!(alpha < bravo && bravo < charlie, "{dest}");
}

#[test]
fn git_dry_run_changes_no_notes_or_git_state() {
    let temp = TempDir::new("bob-task-move-git-dry-run");
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
    git_in(&vault, ["commit", "-q", "-m", "dry-run fixture"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    let head_before = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    let status_before = stdout(&git_in(&vault, ["status", "--porcelain"]));
    let remote_before = git([
        "--git-dir",
        path_str(&remote),
        "rev-parse",
        "refs/heads/master",
    ]);
    let source_before = fs::read(vault.join("source.md")).expect("read source");
    let dest_before = fs::read(vault.join("dest.md")).expect("read dest");

    let output = run_json(
        &temp,
        &vault,
        &["--dry-run", "-f", "json", "source", "dest", "-b", "move-me"],
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["git"]["mode"], "dry_run");
    assert!(json["git"]["pre_sync"].is_null());
    assert!(json["git"]["commit"].is_null());
    assert!(json["git"]["post_sync"].is_null());
    assert_eq!(
        fs::read(vault.join("source.md")).expect("read source after"),
        source_before
    );
    assert_eq!(
        fs::read(vault.join("dest.md")).expect("read dest after"),
        dest_before
    );
    assert_eq!(stdout(&git_in(&vault, ["rev-parse", "HEAD"])), head_before);
    assert_eq!(
        stdout(&git_in(&vault, ["status", "--porcelain"])),
        status_before
    );
    let remote_after = git([
        "--git-dir",
        path_str(&remote),
        "rev-parse",
        "refs/heads/master",
    ]);
    assert_eq!(stdout(&remote_after), stdout(&remote_before));
}

#[test]
fn identity_refusals_write_nothing() {
    let temp = TempDir::new("bob-task-move-identity-refusals");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Anchored [id:: source__x] ^x\n",
    );
    write_file(
        &vault.join("dest space.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    let source_before = fs::read(vault.join("source.md")).expect("read source");
    let dest_before = fs::read(vault.join("dest space.md")).expect("read dest");
    let output = run_json(
        &temp,
        &vault,
        &["-f", "json", "source", "dest space", "-b", "x"],
    );
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["stage"], "plan");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("cannot encode identity"));
    assert_eq!(
        fs::read(vault.join("source.md")).expect("read source after"),
        source_before
    );
    assert_eq!(
        fs::read(vault.join("dest space.md")).expect("read dest after"),
        dest_before
    );

    let collision = temp.path().join("collision");
    write_file(
        &collision.join("a/b.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Nested [id:: a__b__x] ^x\n",
    );
    write_file(
        &collision.join("a__b.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Flat [id:: a__b__x] ^x\n",
    );
    write_file(
        &collision.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    let nested_before =
        fs::read(collision.join("a/b.md")).expect("read nested");
    let output =
        run_json(&temp, &collision, &["-f", "json", "a/b", "dest", "-b", "x"]);
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["error"]["stage"], "plan");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("encoded identity"));
    assert_eq!(
        fs::read(collision.join("a/b.md")).expect("read nested after"),
        nested_before
    );

    let stale = temp.path().join("stale");
    write_file(
        &stale.join("src.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Nested [id:: src__a] [id:: stale] ^a\n",
    );
    write_file(
        &stale.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    let src_before = fs::read(stale.join("src.md")).expect("read src");
    let output =
        run_json(&temp, &stale, &["-f", "json", "src", "dest", "-b", "a"]);
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["error"]["stage"], "plan");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("non-canonical"));
    assert_eq!(
        fs::read(stale.join("src.md")).expect("read src after"),
        src_before
    );
}

#[test]
fn ordinary_commit_failure_keeps_the_write_without_post_sync() {
    let temp = TempDir::new("bob-task-move-commit-failure");
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
    git_in(&vault, ["commit", "-q", "-m", "commit-failure fixture"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    let hook = vault.join(".git/hooks/commit-msg");
    write_file(&hook, "#!/bin/sh\nexit 1\n");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
        .expect("make rejecting commit-msg hook executable");
    let remote_before = git([
        "--git-dir",
        path_str(&remote),
        "rev-parse",
        "refs/heads/master",
    ]);

    let output = run_json(
        &temp,
        &vault,
        &["-o", "-f", "json", "source", "dest", "-b", "move-me"],
    );
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["stage"], "commit");
    assert!(json["git"]["commit"].is_null());
    assert!(json["git"]["post_sync"].is_null());
    assert!(fs::read_to_string(vault.join("dest.md"))
        .unwrap()
        .contains("Move this task"));
    assert!(!fs::read_to_string(vault.join("source.md"))
        .unwrap()
        .contains("Move this task"));
    let subject = git_in(&vault, ["log", "-1", "--format=%s"]);
    assert_eq!(stdout(&subject).trim(), "commit-failure fixture");
    let remote_after = git([
        "--git-dir",
        path_str(&remote),
        "rev-parse",
        "refs/heads/master",
    ]);
    assert_eq!(stdout(&remote_after), stdout(&remote_before));
}

#[test]
fn pre_sync_uses_the_peer_snapshot_and_commits_pending_local_edits() {
    let temp = TempDir::new("bob-task-move-pre-sync-peer");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move this task ^move-me\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n",
    );
    write_file(
        &vault.join("2026/20261019.md"),
        "## Pomodoros\n- [ ] ()\n  - [[source#^move-me|task]]\n",
    );
    git_in(&vault, ["add", "source.md", "dest.md", "2026/20261019.md"]);
    git_in(&vault, ["commit", "-q", "-m", "pre-sync fixture"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);

    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\nlocal dest note\n",
    );
    write_file(
        &peer.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Peer retitled task ^move-me\n",
    );
    git_in(&peer, ["add", "source.md"]);
    git_in(&peer, ["commit", "-q", "-m", "peer retitle"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = run_json(
        &temp,
        &vault,
        &["-f", "json", "source", "dest", "-b", "move-me"],
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true, "{}", format_output(&output));
    assert_eq!(json["git"]["mode"], "sync");
    assert_eq!(json["git"]["pre_sync"]["ok"], true);
    assert!(json["git"]["pre_sync"]["files_committed"].as_u64().unwrap() >= 1);
    assert_eq!(
        json["git"]["commit"]["subject"],
        "bob task move source.md -> dest.md (1 task)"
    );
    assert_eq!(json["git"]["post_sync"]["ok"], true);
    assert_eq!(json["git"]["post_sync"]["pushed"], true);
    assert!(json["git"]["post_sync"]["conflicts"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(json["moves"][0]["title"], "Peer retitled task");
    let dest =
        fs::read_to_string(vault.join("dest.md")).expect("read destination");
    assert!(dest.contains("Peer retitled task"), "{dest}");
    assert!(dest.contains("local dest note"), "{dest}");
    let log = stdout(&git_in(&vault, ["log", "--format=%s"]));
    assert!(
        log.contains("bob task move source.md -> dest.md (1 task)"),
        "{log}"
    );
    assert!(
        log.lines()
            .any(|line| line != "bob task move source.md -> dest.md (1 task)"
                && line != "pre-sync fixture"
                && line != "initial vault"),
        "missing separate pre-sync commit:\n{log}"
    );
    let remote_head = git([
        "--git-dir",
        path_str(&remote),
        "rev-parse",
        "refs/heads/master",
    ]);
    assert_eq!(
        stdout(&remote_head),
        stdout(&git_in(&vault, ["rev-parse", "HEAD"]))
    );
}

#[test]
fn post_sync_conflict_keeps_the_local_commit_and_reports_remote_wins() {
    let temp = TempDir::new("bob-task-move-post-sync-conflict");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move this task ^move-me\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\nSee [[source#^move-me]].\n",
    );
    git_in(&vault, ["add", "source.md", "dest.md"]);
    git_in(&vault, ["commit", "-q", "-m", "conflict fixture"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);
    git_in(
        &vault,
        [
            "config",
            "core.hooksPath",
            path_str(&vault.join(".git/hooks")),
        ],
    );

    let hook = vault.join(".git/hooks/post-commit");
    write_file(
        &hook,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX\n",
                "rm -f \"{hook}\"\n",
                "printf '%s\\n' '---' 'type: [[area]]' '---' '' '## Tasks' '' 'See [[source#^move-me|peer]].' > \"{peer}/dest.md\"\n",
                "git -C \"{peer}\" add dest.md\n",
                "git -C \"{peer}\" commit -q -m \"peer conflicting dest\"\n",
                "git -C \"{peer}\" push -q origin master\n",
                "exit 0\n",
            ),
            hook = path_str(&hook),
            peer = path_str(&peer),
        ),
    );
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
        .expect("make post-commit hook executable");

    let output = run_json(
        &temp,
        &vault,
        &["-f", "json", "source", "dest", "-b", "move-me"],
    );
    assert_exit(&output, 1);
    let json = parse_json(&output);
    assert_eq!(json["ok"], false, "{}", format_output(&output));
    assert_eq!(json["error"]["stage"], "post_sync");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("conflict"),
        "{}",
        json["error"]
    );
    assert!(!json["git"]["post_sync"]["conflicts"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(json["git"]["commit"]["sha"].as_str().is_some());
    assert!(
        vault.join("_conflicts").is_dir(),
        "expected _conflicts/ copy"
    );
    let dest =
        fs::read_to_string(vault.join("dest.md")).expect("read destination");
    assert!(
        dest.contains("peer"),
        "remote copy should win in place:\n{dest}"
    );
    let log = stdout(&git_in(&vault, ["log", "--format=%s"]));
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("bob task move "))
            .count(),
        1,
        "local commit must stand:\n{log}"
    );
}

#[test]
fn co_move_then_reconcile_keeps_canonical_identities() {
    let temp = TempDir::new("bob-task-move-co-move-reconcile");
    let vault = temp.path().join("vault");
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("source.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Parent [id:: source__move-me] [fresh:: 2026-10-01] [keeps:: 2] [unblocked:: 2026-10-02] ^move-me\n  - ⛓️ **DEPENDS ON:** [[#^child]]\n  - [ ] #task Child [id:: source__child] ^child\n- [ ] #task Other [dependsOn:: source__move-me, source__child] ^other\n",
    );
    write_file(
        &vault.join("dest.md"),
        "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Existing destination block ^move-me\n",
    );
    write_file(
        &vault.join("2026/20261019.md"),
        "## Pomodoros\n- [ ] ()\n  - [[source#^move-me|task]]\n",
    );
    let output = run_json(
        &temp,
        &vault,
        &["-f", "json", "source", "dest", "-b", "move-me"],
    );
    assert_success(&output);
    let dest =
        fs::read_to_string(vault.join("dest.md")).expect("read destination");
    assert!(dest.contains("[id:: dest__move-me-1]"), "{dest}");
    assert!(dest.contains("[id:: dest__child]"), "{dest}");
    assert!(dest.contains("[fresh:: 2026-10-01]"), "{dest}");
    assert!(dest.contains("[keeps:: 2]"), "{dest}");
    assert!(dest.contains("[unblocked:: 2026-10-02]"), "{dest}");
    assert!(dest.contains("[[#^child]]"), "{dest}");
    let source =
        fs::read_to_string(vault.join("source.md")).expect("read source");
    assert!(
        source.contains("[dependsOn:: dest__move-me-1, dest__child]"),
        "{source}"
    );
    assert!(fs::read_to_string(vault.join("2026/20261019.md"))
        .unwrap()
        .contains("[[dest#^move-me-1|task]]"));

    let daily = vault.join("2026/20261019.md");
    let reconciled = bob_command()
        .args(["task", "reconcile", "-r", "0"])
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_NOW", "2026-10-19")
        .env("HOME", temp.path().join("home"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob task reconcile after move");
    assert_success(&reconciled);
    let dest =
        fs::read_to_string(vault.join("dest.md")).expect("read dest after");
    assert!(dest.contains("^move-me-1"), "{dest}");
    assert!(dest.contains("[id:: dest__child]"), "{dest}");
    assert!(dest.contains("[fresh:: 2026-10-01]"), "{dest}");
    assert!(dest.contains("[keeps:: 2]"), "{dest}");
    let source =
        fs::read_to_string(vault.join("source.md")).expect("read source after");
    assert!(
        source.contains("dest__move-me-1")
            || source.contains("[[dest#^move-me-1]]"),
        "{source}"
    );
}
