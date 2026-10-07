//! Vault sync, conflicts, renamed commands, nightly, stubs.

use crate::support::*;
use fs2::FileExt;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

#[test]
fn vault_sync_no_change_cycle_writes_status_without_committing() {
    let temp = TempDir::new("bob-cli-vault-sync-no-change");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    let head_before = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run no-change vault-sync");

    assert_success(&output);
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "HEAD"])),
        head_before,
        "no-change cycle should not create a commit"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["status", "--porcelain"])),
        "",
        "vault should stay clean"
    );

    let status = read_vault_sync_status(&temp);
    assert_eq!(status["files_committed"], 0);
    assert_eq!(status["last_error"], serde_json::Value::Null);
    assert!(
        status["last_success_at"].is_string(),
        "successful cycle should set last_success_at:\n{status}"
    );

    let status_output = vault_sync_command(&vault, &temp)
        .arg("status")
        .arg("--json")
        .output()
        .expect("run vault-sync status --json");
    assert_success(&status_output);
    let printed_status: serde_json::Value =
        serde_json::from_str(&stdout(&status_output))
            .expect("status --json should print JSON");
    assert_eq!(printed_status["last_error"], serde_json::Value::Null);
}

#[test]
fn vault_sync_local_only_change_commits_and_pushes() {
    let temp = TempDir::new("bob-cli-vault-sync-local");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("extra.md"), "- [ ] local #task\n");

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "Athena Test")
        .output()
        .expect("run local-only vault-sync");

    assert_success(&output);
    let subject = stdout(&git_in(&vault, ["log", "-1", "--format=%s"]));
    assert!(
        subject.starts_with("vault(athena-test): 1 file - extra.md"),
        "generated commit subject should summarize paths:\n{subject}"
    );
    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:extra.md"
        ])),
        "- [ ] local #task\n",
        "push should update remote"
    );

    let status = read_vault_sync_status(&temp);
    assert_eq!(status["files_committed"], 1);
}

#[test]
fn vault_sync_remote_only_change_fast_forwards() {
    let temp = TempDir::new("bob-cli-vault-sync-remote");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
    write_file(&peer.join("remote.md"), "- [ ] remote #task\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote change"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run remote-only vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("remote.md"))
            .expect("read fast-forwarded file"),
        "- [ ] remote #task\n"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "HEAD"])),
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "rev-parse",
            "master"
        ])),
        "local HEAD should match remote after fast-forward"
    );
}

#[test]
fn vault_sync_non_overlapping_edits_merge_cleanly() {
    let temp = TempDir::new("bob-cli-vault-sync-clean-merge");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("shared.md"), "one\nmiddle\ntwo\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add shared"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);

    write_file(&vault.join("shared.md"), "local one\nmiddle\ntwo\n");
    write_file(&peer.join("shared.md"), "one\nmiddle\nremote two\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote shared"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run clean-merge vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("shared.md")).expect("read merged file"),
        "local one\nmiddle\nremote two\n"
    );
    assert!(
        quarantined_conflict_files(&vault).is_empty(),
        "clean merge should not create conflict copies"
    );
}

#[test]
fn vault_sync_same_line_edit_quarantines_local_copy_and_keeps_remote() {
    let temp = TempDir::new("bob-cli-vault-sync-text-conflict");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("note.md"), "base\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add note"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);

    write_file(&vault.join("note.md"), "local\n");
    write_file(&peer.join("note.md"), "remote\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote note"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "athena")
        .output()
        .expect("run text-conflict vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("note.md")).expect("read remote winner"),
        "remote\n"
    );
    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read_to_string(&copies[0]).expect("read conflict copy"),
        "local\n"
    );
    assert_no_conflict_markers(&vault);

    let status = read_vault_sync_status(&temp);
    assert_eq!(
        status["conflicts"]
            .as_array()
            .expect("conflicts array")
            .len(),
        1
    );
}

#[test]
fn vault_sync_both_added_file_quarantines_local_copy() {
    let temp = TempDir::new("bob-cli-vault-sync-both-added");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("2026/20260827.md"), "local daily\n");
    write_file(&peer.join("2026/20260827.md"), "remote daily\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote daily"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run both-added vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("2026/20260827.md"))
            .expect("read remote winner"),
        "remote daily\n"
    );
    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read_to_string(&copies[0]).expect("read conflict copy"),
        "local daily\n"
    );
}

#[test]
fn vault_sync_delete_modify_conflict_keeps_the_file() {
    let temp = TempDir::new("bob-cli-vault-sync-delete-modify");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("keep.md"), "base\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add keep"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);

    write_file(&vault.join("keep.md"), "local edit\n");
    fs::remove_file(peer.join("keep.md")).expect("delete peer file");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote delete"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run delete-modify vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("keep.md")).expect("read kept file"),
        "local edit\n",
        "delete/modify conflict should keep the file"
    );
}

#[test]
fn vault_sync_binary_conflict_quarantines_uncorrupted_local_copy() {
    let temp = TempDir::new("bob-cli-vault-sync-binary-conflict");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    write_bytes(&vault.join("image.bin"), b"base\0bytes\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add binary"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);

    write_bytes(&vault.join("image.bin"), b"local\0bytes\n");
    write_bytes(&peer.join("image.bin"), b"remote\0bytes\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote binary"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run binary-conflict vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read(vault.join("image.bin")).expect("read binary winner"),
        b"remote\0bytes\n"
    );
    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read(&copies[0]).expect("read binary conflict copy"),
        b"local\0bytes\n"
    );
}

#[test]
fn vault_sync_refuses_95_mib_file_before_staging() {
    let temp = TempDir::new("bob-cli-vault-sync-large-file");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    let large = vault.join("large.bin");
    let file = fs::File::create(&large).expect("create sparse large file");
    file.set_len(95 * 1024 * 1024)
        .expect("size sparse large file");

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run large-file vault-sync");

    assert_eq!(
        output.status.code(),
        Some(1),
        "large file should fail locally:\n{}",
        format_output(&output)
    );
    assert_eq!(
        stdout(&git_in(&vault, ["diff", "--cached", "--name-only"])),
        "",
        "large file must not be staged"
    );
    let status = read_vault_sync_status(&temp);
    assert!(
        status["last_error"]
            .as_str()
            .is_some_and(|error| error.contains("refusing to stage large.bin")),
        "status should record large-file preflight error:\n{status}"
    );
}

#[test]
fn vault_sync_recovers_interrupted_merge_and_finishes_cycle() {
    let temp = TempDir::new("bob-cli-vault-sync-interrupted-merge");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("note.md"), "base\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add note"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);

    write_file(&vault.join("note.md"), "local after interrupted merge\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "local note"]);
    write_file(&peer.join("note.md"), "remote after interrupted merge\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote note"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);
    git_in(&vault, ["fetch", "-q", "origin", "master"]);

    let merge = git_maybe_in(&vault, ["merge", "--no-edit", "origin/master"]);
    assert!(
        !merge.status.success(),
        "manual merge should leave conflict state:\n{}",
        format_output(&merge)
    );
    assert!(vault.join(".git/MERGE_HEAD").exists());

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run interrupted-merge vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("note.md")).expect("read resolved note"),
        "remote after interrupted merge\n"
    );
    assert!(
        !vault.join(".git/MERGE_HEAD").exists(),
        "cycle should clear interrupted merge state"
    );
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["interrupted_merge_recovered"], true);
}

#[test]
fn vault_sync_push_race_retries_and_succeeds() {
    let temp = TempDir::new("bob-cli-vault-sync-push-race");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("local.md"), "local\n");
    let marker = temp.path().join("race-marker");
    write_executable(
        &vault.join(".git/hooks/pre-push"),
        r#"#!/bin/sh
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
if [ ! -f "$RACE_MARKER" ]; then
  : > "$RACE_MARKER"
  printf 'race\n' > "$RACE_PEER/race.md"
  git -C "$RACE_PEER" add race.md
  git -C "$RACE_PEER" commit -q -m "race remote"
  git -C "$RACE_PEER" push -q origin master
  printf 'non-fast-forward race\n' >&2
  exit 1
fi
exit 0
"#,
    );

    let output = vault_sync_command(&vault, &temp)
        .env("RACE_MARKER", &marker)
        .env("RACE_PEER", &peer)
        .output()
        .expect("run push-race vault-sync");

    assert_success(&output);
    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:local.md"
        ])),
        "local\n"
    );
    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:race.md"
        ])),
        "race\n"
    );
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["push_retries"], 1);
}

#[test]
fn vault_sync_concurrent_invocation_exits_zero_silently() {
    let temp = TempDir::new("bob-cli-vault-sync-lock");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    let lock_path = vault_sync_lock_file(&temp);
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold lock");

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run locked vault-sync");

    assert_success(&output);
    assert_eq!(stdout(&output), "", "locked run should be silent on stdout");
    assert_eq!(stderr(&output), "", "locked run should be silent on stderr");
}

#[test]
fn conflict_directory_is_skipped_by_vault_walkers() {
    let temp = TempDir::new("bob-cli-conflicts-excluded");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_file(
        &vault.join("_conflicts/bad.md"),
        &format!("{}\n#project\n", done_tasks_source(12)),
    );

    let output = bob_command()
        .args(["task", "archive"])
        .arg("--threshold")
        .arg("10")
        .env("BOB_DIR", &vault)
        .env("HOME", temp.path().join("home"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob task archive against conflict-only vault");

    assert_success(&output);
    assert!(
        stdout(&output).contains("markdown files: 0"),
        "_conflicts markdown files should not be scanned:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("done").exists(),
        "_conflicts tasks must not be archived"
    );

    let dataview = bob_command()
        .arg("query")
        .arg("-b")
        .arg(&vault)
        .arg("-S")
        .arg("-q")
        .arg("LIST FROM #project")
        .output()
        .expect("run native Dataview query against conflict-only vault");
    assert_success(&dataview);
    assert_eq!(
        stdout(&dataview),
        "",
        "_conflicts notes must not be visible to native Dataview"
    );

    let tasks = bob_command()
        .arg("query")
        .arg("-b")
        .arg(&vault)
        .arg("--tasks")
        .arg("")
        .output()
        .expect("run native Tasks query against conflict-only vault");
    assert_success(&tasks);
    assert_eq!(
        stdout(&tasks),
        "",
        "_conflicts tasks must not be visible to native Tasks"
    );
}

#[test]
fn renamed_old_top_level_commands_are_unknown() {
    for command in ["move-done-tasks", "query", "vault-sync"] {
        let output = bob_command()
            .arg(command)
            .arg("--help")
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob {command} --help: {error}")
            });
        assert_success(&output);
    }

    for command in [
        "bulk-git-commit",
        "collect-done",
        "cronjob",
        "dataview",
        "sync",
    ] {
        let output = bob_command()
            .arg(command)
            .arg("--help")
            .output()
            .unwrap_or_else(|error| {
                panic!("run bob {command} --help: {error}")
            });
        assert_eq!(
            output.status.code(),
            Some(2),
            "old top-level command should be rejected:\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains("unrecognized subcommand"),
            "expected clap unknown-command error:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn nightly_runs_vault_sync_move_done_tasks_vault_sync_in_order() {
    let temp = TempDir::new("bob-cli-nightly-happy");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    let home = temp.path().join("home");
    let source = vault.join("obsidian.md");
    let archive = vault.join("done/obsidian_done.md");
    let extra = vault.join("extra.md");
    fs::create_dir_all(&home).expect("create home");
    write_file(&source, &done_tasks_source(12));
    write_file(&extra, "- [ ] extra #task\n");

    let output = bob_command()
        .arg("nightly")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("HOME", &home)
        .env("HOSTNAME", "Athena Test")
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob nightly");

    assert_success(&output);
    let out = stdout(&output);

    let subjects = stdout(&git_in(&vault, ["log", "--format=%s"]));
    let lines: Vec<&str> = subjects.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some("bob task archive 2026-06-02"),
        "bob task archive should be the newest commit:\n{subjects}"
    );
    assert!(
        lines
            .get(1)
            .is_some_and(|subject| subject
                .starts_with("vault(athena-test): 2 files - extra.md, obsidian.md")),
        "leading vault-sync should commit loose vault changes first:\n{subjects}"
    );

    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:extra.md"
        ])),
        "- [ ] extra #task\n",
        "vault-sync should publish the loose extra file"
    );
    let remote_head =
        stdout(&git(["--git-dir", path_str(&remote), "rev-parse", "HEAD"]));
    let local_head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(
        remote_head, local_head,
        "nightly should leave remote current"
    );

    let archive_contents = fs::read_to_string(&archive).expect("read archive");
    assert!(
        archive_contents.contains("parent: \"[[obsidian]]\"")
            && archive_contents.contains("- [x] done 1 #task"),
        "expected archived tasks:\n{archive_contents}"
    );
    assert!(
        fs::read_to_string(&source)
            .expect("read source")
            .contains("done_tasks: \"[[done/obsidian_done]]\""),
        "expected source link in {}",
        source.display()
    );

    assert!(
        out.contains("bob nightly")
            && out.contains("step 1/3")
            && out.contains("step 2/3")
            && out.contains("step 3/3")
            && out.contains("vault-sync")
            && out.contains("task archive")
            && out.contains("All steps passed"),
        "expected a structured nightly summary:\n{}",
        format_output(&output)
    );
    assert!(
        !out.contains("Obsidian sync") && !out.contains("ob sync"),
        "nightly output must not mention the retired Obsidian sync gate:\n{out}"
    );
    assert!(
        !output.stdout.contains(&0x1b),
        "piped nightly output must not contain ANSI escape codes:\n{out}"
    );
}

#[test]
fn nightly_failed_step_still_runs_later_steps_and_exits_nonzero() {
    let temp = TempDir::new("bob-cli-nightly-step-fail");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    let home = temp.path().join("home");
    let source = vault.join("obsidian.md");
    let blocking_done_path = vault.join("done");
    let extra = vault.join("extra.md");
    fs::create_dir_all(&home).expect("create home");
    write_file(&source, &done_tasks_source(12));
    write_file(
        &blocking_done_path,
        "regular file blocking done/ archive directory\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "blocked archive"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    write_file(&extra, "- [ ] later step still runs #task\n");

    let output = bob_command()
        .arg("nightly")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-06-02")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("HOME", &home)
        .env("HOSTNAME", "Athena Test")
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob nightly with a failing wrapped step");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected the failing step's exit code:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("\u{2717} task archive"),
        "expected a failed task archive marker:\n{}",
        format_output(&output)
    );
    assert!(
        out.contains("step 3/3") && out.contains("1 step failed"),
        "expected the trailing vault-sync step and failure count:\n{}",
        format_output(&output)
    );

    assert!(
        stdout(&git_in(&vault, ["log", "-1", "--format=%s"]))
            .starts_with("vault(athena-test): 1 file - extra.md"),
        "leading vault-sync should commit the unrelated file"
    );
    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:extra.md"
        ])),
        "- [ ] later step still runs #task\n",
        "vault-sync should push the unrelated file"
    );
}

#[cfg(unix)]
#[test]
fn executable_stubs_stay_executable_while_other_threads_fork() {
    // Guards the `Text file busy` flake (bead bob-cli-o): a stub must never be
    // written through a descriptor this process holds, because any child
    // forked during the write inherits it and makes `execve` of the stub fail
    // with ETXTBSY until that child execs.
    let temp = TempDir::new("bob-cli-etxtbsy-stub");
    let stub = temp.path().join("stub");
    let padding = "#".repeat(256 * 1024);
    let payload = format!("#!/bin/sh\nexit 7\n{padding}");

    let stop = Arc::new(AtomicBool::new(false));
    let lurkers: Vec<_> = (0..4)
        .map(|_| {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = Command::new("true").output();
                }
            })
        })
        .collect();

    for iteration in 0..60 {
        write_executable(&stub, &payload);
        if iteration == 0 {
            assert_eq!(
                fs::read_to_string(&stub).expect("read stub"),
                payload,
                "stub contents must match the payload"
            );
            assert_unix_mode(&stub, 0o755);
            let leftovers: Vec<_> = fs::read_dir(temp.path())
                .expect("read stub dir")
                .map(|entry| entry.expect("dir entry").file_name())
                .collect();
            assert_eq!(
                leftovers,
                [OsString::from("stub")],
                "stub directory must contain only the stub, no leftover payload"
            );
        }
        match Command::new(&stub).output() {
            Err(error) if error.raw_os_error() == Some(26) => {
                panic!(
                    "Text file busy (ETXTBSY, os error 26) executing stub {} — \
                     write_executable leaked a writable descriptor (bead bob-cli-o)",
                    stub.display()
                );
            }
            Err(error) => {
                panic!("execute stub {}: {error}", stub.display());
            }
            Ok(output) => {
                assert_eq!(
                    output.status.code(),
                    Some(7),
                    "stub should exit 7:\n{}",
                    format_output(&output)
                );
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    for lurker in lurkers {
        lurker.join().expect("lurker thread");
    }
}

fn done_tasks_source(count: usize) -> String {
    let mut text = String::new();
    for index in 1..=count {
        text.push_str(&format!("- [x] done {index} #task\n"));
    }
    text.push_str("- [ ] active #task\n");
    text
}

fn read_vault_sync_status(temp: &TempDir) -> serde_json::Value {
    let path = vault_sync_state_file(temp);
    let contents = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("read status {}: {error}", path.display())
    });
    serde_json::from_str(&contents).unwrap_or_else(|error| {
        panic!("parse status {}: {error}\n{contents}", path.display())
    })
}

fn git_maybe_in<I, S>(directory: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("color.status=false")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .expect("run git")
}

fn write_bytes(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn quarantined_conflict_files(vault: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_quarantined_conflict_files(&vault.join("_conflicts"), &mut files);
    files.sort();
    files
}

fn collect_quarantined_conflict_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries {
        let entry = entry.expect("read conflict entry");
        let path = entry.path();
        let file_type = entry.file_type().expect("read conflict file type");
        if file_type.is_dir() {
            collect_quarantined_conflict_files(&path, files);
        } else if file_type.is_file()
            && path.file_name().and_then(OsStr::to_str)
                != Some("sync_conflicts.md")
        {
            files.push(path);
        }
    }
}

fn assert_no_conflict_markers(vault: &Path) {
    let mut files = Vec::new();
    collect_vault_files(vault, vault, &mut files);
    for path in files {
        let contents = fs::read(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert!(
            !contents
                .windows(b"<<<<<<<".len())
                .any(|window| window == b"<<<<<<<"),
            "conflict marker found in {}",
            path.display()
        );
    }
}

fn collect_vault_files(
    vault: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) {
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        panic!("read {}: {error}", directory.display())
    });
    for entry in entries {
        let entry = entry.expect("read vault entry");
        if entry.file_name() == OsStr::new(".git") {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().expect("read vault file type");
        if file_type.is_dir() {
            collect_vault_files(vault, &path, files);
        } else if file_type.is_file() {
            let _ = path.strip_prefix(vault).expect("vault file path");
            files.push(path);
        }
    }
}
