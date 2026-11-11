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

const UNION_BASE: &str = "---\n\
parent: \"[[dev]]\"\n\
---\n\
\n\
- [ ] #task Successor deps ^successor-deps\n\
\t- 🥀 **ROTTEN LOG**\n\
\t\t- *2026-11-06* — 🍂 Next → Ready\n\
\n\
### Next & In Progress\n\
\n\
- [?] #task Make gtd_daily grey [scheduled:: 2026-11-08] ^grey-gtd-daily\n\
\t- IMPL: Add a color property\n\
\t- 🗓️ **SCHEDULE LOG**\n";

const UNION_LOCAL_TASK: &str = "- [*] #task Change parent of gtd [created::2026-11-07] ^gtd-parent-is-gtd\n\
\t- 🪢 **LINKED FROM:** [[#^grey-gtd-daily]] (merged into)\n\
\t- Make gtd_daily grey\n\
\t\t- IMPL: Add a color property\n";

const UNION_REMOTE_TASK: &str = "- [ ] #task #ref [[ref/chat/gmail_thread_note_sync|Gmail sync]] [created::2026-11-07] ^ref-gmail-thread-note-sync\n";

/// Local side of the incident: a new task at the end of the open list plus a
/// non-conflicting merge cancellation further down the note.
fn union_local_note() -> String {
    UNION_BASE
        .replace(
            "\t\t- *2026-11-06* — 🍂 Next → Ready\n",
            &format!(
                "\t\t- *2026-11-06* — 🍂 Next → Ready\n{UNION_LOCAL_TASK}"
            ),
        )
        .replace(
            "- [?] #task Make gtd_daily grey [scheduled:: 2026-11-08] ^grey-gtd-daily\n\
\t- IMPL: Add a color property\n",
            "- [-] #task Make gtd_daily grey [scheduled:: 2026-11-08] [cancelled:: 2026-11-07] ^grey-gtd-daily\n\
\t- 🪢 **LINKS:** merged into [[#^gtd-parent-is-gtd]]\n",
        )
}

fn union_remote_note() -> String {
    UNION_BASE.replace(
        "\t\t- *2026-11-06* — 🍂 Next → Ready\n",
        &format!("\t\t- *2026-11-06* — 🍂 Next → Ready\n{UNION_REMOTE_TASK}"),
    )
}

fn union_merged_note() -> String {
    union_local_note().replace(
        UNION_LOCAL_TASK,
        &format!("{UNION_LOCAL_TASK}{UNION_REMOTE_TASK}"),
    )
}

/// Commit `files` as the shared base, then write the local edits into the
/// vault's working tree and push the remote edits from the peer.
fn seed_vault_sync_conflict(
    vault: &Path,
    peer: &Path,
    files: &[(&str, &str, &str, &str)],
) {
    for (path, base, _, _) in files {
        write_file(&vault.join(path), base);
    }
    git_in(vault, ["add", "."]);
    git_in(vault, ["commit", "-q", "-m", "add shared notes"]);
    git_in(vault, ["push", "-q", "origin", "master"]);
    git_in(peer, ["pull", "-q", "--ff-only", "origin", "master"]);
    for (path, _, local, remote) in files {
        write_file(&vault.join(path), local);
        write_file(&peer.join(path), remote);
    }
    git_in(peer, ["add", "."]);
    git_in(peer, ["commit", "-q", "-m", "remote edits"]);
    git_in(peer, ["push", "-q", "origin", "master"]);
}

fn conflict_log(vault: &Path) -> String {
    fs::read_to_string(vault.join("_conflicts/sync_conflicts.md"))
        .expect("read conflict log")
}

fn status_strings(status: &serde_json::Value, field: &str) -> Vec<String> {
    status[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} array in {status}"))
        .iter()
        .map(|value| value.as_str().expect("string entry").to_string())
        .collect()
}

#[test]
fn vault_sync_same_point_insertions_auto_merge_without_quarantine() {
    let temp = TempDir::new("bob-cli-vault-sync-union");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
    let (local_note, remote_note) = (union_local_note(), union_remote_note());
    seed_vault_sync_conflict(
        &vault,
        &peer,
        &[(
            "bob.md",
            UNION_BASE,
            local_note.as_str(),
            remote_note.as_str(),
        )],
    );

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "mac")
        .output()
        .expect("run union vault-sync");

    assert_success(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "auto-merged bob.md: kept both sides of 1 same-point insertion hunk(s)"
        ),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("read merged note"),
        union_merged_note()
    );
    assert!(
        quarantined_conflict_files(&vault).is_empty(),
        "auto-merge should not write a conflict copy"
    );
    assert_no_conflict_markers(&vault);
    let log = conflict_log(&vault);
    assert!(
        log.contains(
            " bob.md auto-merged 1 same-point insertion hunk(s); nothing dropped\n"
        ),
        "{log}"
    );
    assert!(!log.contains(" -> _conflicts/"), "{log}");

    let status = read_vault_sync_status(&temp);
    assert_eq!(status_strings(&status, "auto_merged"), ["bob.md"]);
    assert!(status_strings(&status, "conflicts").is_empty());
    assert!(status["last_error"].is_null(), "{status}");

    let head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(
        head,
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "rev-parse",
            "master"
        ])),
        "the merge should reach the remote"
    );
    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:bob.md"
        ])),
        union_merged_note()
    );
    assert!(stdout(&git_in(&vault, ["status", "--porcelain"])).is_empty());

    let panel = vault_sync_command(&vault, &temp)
        .arg("status")
        .output()
        .expect("run vault-sync status");
    assert_success(&panel);
    assert!(
        stdout(&panel).contains("auto_merged:\n  - bob.md\n"),
        "{}",
        format_output(&panel)
    );
}

#[test]
fn vault_sync_insertion_plus_edit_conflict_quarantines_whole_file() {
    let temp = TempDir::new("bob-cli-vault-sync-union-mixed");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    let base = "# Note\n\nAlpha paragraph stays put.\nBravo paragraph stays put.\nCharlie paragraph stays put.\nDelta paragraph stays put.\n\n- [ ] #task Edited on both hosts\n";
    let local = "# Note\n- [ ] #task Local insertion\n\nAlpha paragraph stays put.\nBravo paragraph stays put.\nCharlie paragraph stays put.\nDelta paragraph stays put.\n\n- [x] #task Edited on both hosts\n";
    let remote = "# Note\n- [ ] #task Remote insertion\n\nAlpha paragraph stays put.\nBravo paragraph stays put.\nCharlie paragraph stays put.\nDelta paragraph stays put.\n\n- [-] #task Edited on both hosts\n";
    seed_vault_sync_conflict(
        &vault,
        &peer,
        &[("note.md", base, local, remote)],
    );

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "athena")
        .output()
        .expect("run mixed-conflict vault-sync");

    assert_success(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("(not auto-merged: hunk 2 changes existing lines"),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("note.md")).expect("read remote winner"),
        remote
    );
    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read_to_string(&copies[0]).expect("read conflict copy"),
        local,
        "the conflict copy should hold the complete local file"
    );
    assert_no_conflict_markers(&vault);
    assert!(!conflict_log(&vault).contains("auto-merged"));

    let status = read_vault_sync_status(&temp);
    assert!(status_strings(&status, "auto_merged").is_empty());
    assert_eq!(status_strings(&status, "conflicts").len(), 1);
}

#[test]
fn vault_sync_canvas_both_insert_still_quarantines() {
    let temp = TempDir::new("bob-cli-vault-sync-union-canvas");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    let base = "{\n\"nodes\":[\n]\n}\n";
    let local = "{\n\"nodes\":[\n{\"id\":\"local\"}\n]\n}\n";
    let remote = "{\n\"nodes\":[\n{\"id\":\"remote\"}\n]\n}\n";
    seed_vault_sync_conflict(
        &vault,
        &peer,
        &[("board.canvas", base, local, remote)],
    );

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run canvas-conflict vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("board.canvas")).expect("read canvas"),
        remote
    );
    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read_to_string(&copies[0]).expect("read canvas copy"),
        local
    );
    let status = read_vault_sync_status(&temp);
    assert!(status_strings(&status, "auto_merged").is_empty());
}

#[test]
fn vault_sync_auto_merge_and_quarantine_share_one_merge_commit() {
    let temp = TempDir::new("bob-cli-vault-sync-union-two-paths");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
    let (local_note, remote_note) = (union_local_note(), union_remote_note());
    seed_vault_sync_conflict(
        &vault,
        &peer,
        &[
            (
                "bob.md",
                UNION_BASE,
                local_note.as_str(),
                remote_note.as_str(),
            ),
            ("note.md", "base\n", "local\n", "remote\n"),
        ],
    );
    let peer_sha = stdout(&git_in(&peer, ["rev-parse", "HEAD"]));

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "apollo")
        .output()
        .expect("run two-path vault-sync");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("bob.md")).expect("read merged note"),
        union_merged_note()
    );
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
    let log = conflict_log(&vault);
    assert!(log.contains(" bob.md auto-merged 1 same-point"), "{log}");
    assert!(log.contains(" note.md -> _conflicts/note.apollo-"), "{log}");

    let status = read_vault_sync_status(&temp);
    assert_eq!(status_strings(&status, "auto_merged"), ["bob.md"]);
    assert_eq!(status_strings(&status, "conflicts").len(), 1);

    let head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(
        head,
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "rev-parse",
            "master"
        ]))
    );
    assert_eq!(stdout(&git_in(&vault, ["rev-parse", "HEAD^2"])), peer_sha);
    let mut resolved =
        stdout(&git_in(&vault, ["diff", "--name-only", "HEAD^2", "HEAD"]))
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
    resolved.sort();
    assert_eq!(resolved.len(), 3, "{resolved:?}");
    assert!(
        resolved[0].starts_with("_conflicts/note.apollo-"),
        "{resolved:?}"
    );
    assert_eq!(
        resolved[1..],
        ["_conflicts/sync_conflicts.md", "bob.md"],
        "one merge commit should carry the auto-merge and the quarantine"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["rev-list", "--count", "HEAD^1..HEAD"])),
        "2\n",
        "the merge commit and the peer commit are the only new commits"
    );
}

#[test]
fn vault_sync_conflict_copy_on_ignored_path_still_commits_merge() {
    let temp = TempDir::new("bob-cli-vault-sync-ignored-copy");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
    let journal = Path::new(".bob/stats/review/2026-10.peer.jsonl");
    write_file(
        &vault.join(".gitignore"),
        "*\n!.gitignore\n!.gitattributes\n!*/\n!*.md\n!/.bob/stats/**/*.jsonl\n",
    );
    write_file(&vault.join(journal), "base\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add review journal"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);

    write_file(&vault.join(journal), "local journal line\n");
    write_file(&peer.join(journal), "remote journal line\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote journal edit"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);
    let peer_sha = stdout(&git_in(&peer, ["rev-parse", "HEAD"]));

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "athena")
        .output()
        .expect("run ignored-copy vault-sync");

    assert_success(&output);
    assert!(!vault.join(".git/MERGE_HEAD").exists());
    let head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(
        head,
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "rev-parse",
            "master"
        ])),
        "local HEAD should match the pushed merge"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "HEAD^2"])),
        peer_sha,
        "the merge's second parent should be the peer commit"
    );
    assert_eq!(
        fs::read_to_string(vault.join(journal)).expect("read journal"),
        "remote journal line\n"
    );
    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read_to_string(&copies[0]).expect("read conflict copy"),
        "local journal line\n"
    );
    let copy_relative = copies[0]
        .strip_prefix(&vault)
        .expect("copy under vault")
        .to_string_lossy()
        .into_owned();
    assert!(copy_relative.starts_with("_conflicts/.bob/stats/review/"));
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["ls-files", "--error-unmatch", &copy_relative]
        )),
        format!("{copy_relative}\n"),
        "ignored conflict copy should be force-added"
    );
    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            &format!("master:{copy_relative}")
        ])),
        "local journal line\n",
        "conflict copy should reach the remote"
    );

    let second = vault_sync_command(&vault, &temp)
        .output()
        .expect("run second ignored-copy vault-sync");
    assert_success(&second);
    assert_eq!(stdout(&git_in(&vault, ["rev-parse", "HEAD"])), head);
    assert_eq!(quarantined_conflict_files(&vault), copies);
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
    assert_eq!(status["aborted_merge_head"], serde_json::Value::Null);
}

#[test]
fn vault_sync_halts_on_repeated_interrupted_merge_of_same_remote() {
    let temp = TempDir::new("bob-cli-vault-sync-halted-merge");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
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
    let remote_sha = stdout(&git_in(&peer, ["rev-parse", "HEAD"]));
    write_executable(
        &vault.join(".git/hooks/pre-commit"),
        "#!/bin/sh\nmerge_head=$(git rev-parse --git-path MERGE_HEAD) || exit 1\nif test -f \"$merge_head\"; then\n  echo 'reject interrupted merge commit for test' >&2\n  exit 1\nfi\nexit 0\n",
    );

    let first = vault_sync_command(&vault, &temp)
        .output()
        .expect("run first interrupted-merge cycle");
    assert_eq!(
        first.status.code(),
        Some(1),
        "first commit must fail:\n{}",
        format_output(&first)
    );
    assert!(vault.join(".git/MERGE_HEAD").exists());

    let second = vault_sync_command(&vault, &temp)
        .output()
        .expect("run second interrupted-merge cycle");
    assert_eq!(
        second.status.code(),
        Some(1),
        "second merge commit must fail:\n{}",
        format_output(&second)
    );
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["interrupted_merge_recovered"], true);
    assert_eq!(
        status["aborted_merge_head"].as_str().unwrap(),
        remote_sha.trim()
    );
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "MERGE_HEAD"])),
        remote_sha
    );
    assert!(vault.join(".git/MERGE_HEAD").exists());

    let head_before_halt = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    let merge_head_before_halt =
        stdout(&git_in(&vault, ["rev-parse", "MERGE_HEAD"]));
    let copies_before_halt = quarantined_conflict_files(&vault);
    for cycle in 3..=4 {
        let halted = vault_sync_command(&vault, &temp).output().unwrap_or_else(
            |error| panic!("run halted cycle {cycle}: {error}"),
        );
        assert_eq!(halted.status.code(), Some(1));
        let status = read_vault_sync_status(&temp);
        assert!(
            status["last_error"]
                .as_str()
                .is_some_and(|error| error.starts_with("halted:")),
            "cycle {cycle} should record the halt: {status}"
        );
        assert_eq!(
            status["halted_merge_head"].as_str().unwrap(),
            remote_sha.trim()
        );
        assert_eq!(
            stdout(&git_in(&vault, ["rev-parse", "HEAD"])),
            head_before_halt
        );
        assert_eq!(
            stdout(&git_in(&vault, ["rev-parse", "MERGE_HEAD"])),
            merge_head_before_halt
        );
        assert_eq!(quarantined_conflict_files(&vault), copies_before_halt);
    }

    fs::remove_file(vault.join(".git/hooks/pre-commit"))
        .expect("remove rejecting hook");
    git_in(&vault, ["commit", "--no-edit"]);
    let fifth = vault_sync_command(&vault, &temp)
        .output()
        .expect("run recovery cycle");
    assert_success(&fifth);
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["aborted_merge_head"], serde_json::Value::Null);
    assert_eq!(status["halted_merge_head"], serde_json::Value::Null);
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "HEAD"])),
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "rev-parse",
            "master"
        ]))
    );
}

#[test]
fn vault_sync_completes_fast_forward_left_by_head_lock() {
    let temp = TempDir::new("bob-cli-vault-sync-interrupted-ff");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("note.md"), "base\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add note"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);

    write_file(&peer.join("note.md"), "remote first\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "first remote edit"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);
    git_in(&vault, ["fetch", "-q", "origin", "master"]);
    write_file(&vault.join(".git/HEAD.lock"), "held by test\n");
    let interrupted =
        git_maybe_in(&vault, ["merge", "--ff-only", "origin/master"]);
    assert!(
        !interrupted.status.success(),
        "ff should fail while HEAD.lock exists:\n{}",
        format_output(&interrupted)
    );
    fs::remove_file(vault.join(".git/HEAD.lock")).expect("remove HEAD.lock");

    write_file(&peer.join("note.md"), "remote second\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "second remote edit"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run interrupted-ff recovery cycle");
    assert_success(&output);
    let head = stdout(&git_in(&vault, ["rev-parse", "HEAD"]));
    assert_eq!(
        head,
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "rev-parse",
            "master"
        ]))
    );
    assert_eq!(
        stdout(&git_in(&vault, ["log", "-1", "--format=%s"])),
        "second remote edit\n",
        "recovery should not create a local or merge commit"
    );
    assert!(quarantined_conflict_files(&vault).is_empty());
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["interrupted_fast_forward_recovered"], true);
    assert_eq!(status["files_committed"], 0);
}

#[test]
fn vault_sync_retries_fast_forward_blocked_by_head_lock() {
    let temp = TempDir::new("bob-cli-vault-sync-retry-ff");
    let (vault, remote, peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join("note.md"), "base\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "add note"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);
    write_file(&peer.join("note.md"), "remote edit\n");
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote edit"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let marker = temp.path().join("reference-transaction-marker");
    // NOTE: git leaves GIT_DIR empty for reference-transaction hooks and runs
    // them with the worktree root as cwd, so use .git/HEAD.lock directly.
    write_executable(
        &vault.join(".git/hooks/reference-transaction"),
        "#!/bin/sh\n[ \"$1\" = committed ] || exit 0\nwhile read old new ref; do\n  [ \"$ref\" = ORIG_HEAD ] || continue\n  if [ ! -e \"$RACE_MARKER\" ]; then\n    : > \"$RACE_MARKER\"\n    : > \".git/HEAD.lock\"\n  else\n    rm -f \".git/HEAD.lock\"\n  fi\ndone\nexit 0\n",
    );

    let output = vault_sync_command(&vault, &temp)
        .env("RACE_MARKER", &marker)
        .output()
        .expect("run lock-retry vault-sync");

    assert_success(&output);
    assert!(
        stderr(&output)
            .contains("fast-forward blocked by a git lock; retrying (1/5)"),
        "expected retry warning:\n{}",
        stderr(&output)
    );
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "HEAD"])),
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "rev-parse",
            "master"
        ]))
    );
    assert!(quarantined_conflict_files(&vault).is_empty());
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["files_committed"], 0);
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
    assert!(
        !stderr(&output).contains("holds the lock"),
        "nightly owns the lock, so its archive step must not wait on it:\n{}",
        stderr(&output)
    );

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
            && out.contains("step 1/7")
            && out.contains("step 2/7")
            && out.contains("step 3/7")
            && out.contains("step 4/7")
            && out.contains("step 5/7")
            && out.contains("step 6/7")
            && out.contains("step 7/7")
            && out.contains("vault-sync")
            && out.contains("task archive")
            && out.contains("articles")
            && out.contains("period notes")
            && out.contains("notebooks")
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
        out.contains("step 7/7") && out.contains("1 step failed"),
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

#[test]
fn nightly_articles_step_reports_without_writing_and_still_passes() {
    let temp = TempDir::new("bob-cli-nightly-articles");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("create home");
    let article = "---\ntype: \"[[article]]\"\n---\n\n# Draft\n";
    write_file(&vault.join("draft.md"), article);

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
        .expect("run bob nightly with an unformatted article");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("step 3/7")
            && out.contains("articles")
            && out.contains("needs formatting")
            && out.contains("All steps passed"),
        "expected a report-only articles step:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("draft.md")).unwrap(),
        article,
        "nightly must not write articles"
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

const CLOBBER_SASE_CREATED: &str = "2026-06-01T03:30:02-04:00";
const CLOBBER_RECOVERY_CREATED: &str = "2026-10-19T19:32:43-0400";

fn clobber_recovery_note() -> String {
    format!(
        "---\ncreated: {CLOBBER_RECOVERY_CREATED}\nparent: \"[[sase]]\"\n---\n\
- [ ] #task Failed, killed, or interrupted agents can be recovered in one action without losing work! #prj ^prj\n\
- [ ] #task Recover the session pane ^time-all-of-turn\n\
- [ ] #task Preserve the transcript ^restart-survival\n\
- [ ] #task Keep the worktree ^worktree-keep\n\
- [ ] #task Unique recovery line one ^recovery-one\n\
- [ ] #task Unique recovery line two ^recovery-two\n\
- [ ] #task Unique recovery line three ^recovery-three\n\
- [ ] #task Unique recovery line four ^recovery-four\n"
    )
}

fn clobber_sase_note() -> String {
    format!(
        "---\ncreated: {CLOBBER_SASE_CREATED}\n---\n\
- [ ] #task Get ≥1000 stars #prj ^prj\n\
- [ ] #task Task alpha ^alpha\n\
- [ ] #task Task beta ^beta\n\
- [ ] #task Task gamma ^gamma\n\
- [ ] #task Task delta ^delta\n\
- [ ] #task Task epsilon ^epsilon\n"
    )
}

fn clobber_worktree(sase: &str) -> String {
    sase.replace(
        "- [ ] #task Task alpha ^alpha\n",
        "- [?] #task Task alpha [scheduled:: 2026-11-19] ^alpha\n\
- 🎲 P2 roll · in 22\n",
    )
}

fn seed_clobber_vault(vault: &Path) -> (String, String) {
    let recovery = clobber_recovery_note();
    let sase = clobber_sase_note();
    write_file(&vault.join("sase.md"), &sase);
    write_file(&vault.join("sase_agent_recovery.md"), &recovery);
    git_in(vault, ["add", "."]);
    git_in(vault, ["commit", "-q", "-m", "seed clobber notes"]);
    git_in(vault, ["push", "-q", "origin", "master"]);
    (recovery, sase)
}

#[test]
fn vault_sync_quarantines_incident_shaped_clobber() {
    let temp = TempDir::new("bob-cli-vault-sync-clobber");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    let (recovery, sase) = seed_clobber_vault(&vault);
    write_file(
        &vault.join("sase_agent_recovery.md"),
        &clobber_worktree(&sase),
    );

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "athena")
        .output()
        .expect("run clobber vault-sync");

    assert_success(&output);
    let stderr = stderr(&output);
    assert!(
        stderr.contains("quarantined clobber of sase_agent_recovery.md")
            && stderr.contains("matches sase.md"),
        "expected red clobber warning:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase_agent_recovery.md"))
            .expect("read restored note"),
        recovery,
        "HEAD bytes should be restored"
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read sase"),
        sase
    );

    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read_to_string(&copies[0]).expect("read conflict copy"),
        clobber_worktree(&sase)
    );
    let log = fs::read_to_string(vault.join("_conflicts/sync_conflicts.md"))
        .expect("read conflict log");
    assert!(
        log.contains("sase_agent_recovery.md")
            && log.contains("clobber")
            && log.contains("sase.md"),
        "conflict log should name the clobber:\n{log}"
    );

    let committed_recovery = stdout(&git([
        "--git-dir",
        path_str(&remote),
        "show",
        "master:sase_agent_recovery.md",
    ]));
    assert_eq!(
        committed_recovery, recovery,
        "clobbered bytes must not reach the remote"
    );
    let remote_log = stdout(&git([
        "--git-dir",
        path_str(&remote),
        "show",
        "master:_conflicts/sync_conflicts.md",
    ]));
    assert!(
        remote_log.contains("clobber"),
        "conflict log should be pushed:\n{remote_log}"
    );

    let status = read_vault_sync_status(&temp);
    assert_eq!(status["clobbers"].as_array().map(Vec::len), Some(1));
    assert_eq!(status["clobbers"][0]["path"], "sase_agent_recovery.md");
    assert_eq!(status["clobbers"][0]["matched_note"], "sase.md");
    assert_eq!(status["clobbers"][0]["action"], "quarantined");
    assert!(
        status["clobbers"][0]["conflict_copy"]
            .as_str()
            .is_some_and(|path| path.contains("_conflicts/")),
        "status should record the conflict copy:\n{status}"
    );
    assert_eq!(status["recent_clobbers"].as_array().map(Vec::len), Some(1));

    let panel = vault_sync_command(&vault, &temp)
        .args(["status"])
        .output()
        .expect("run vault-sync status");
    assert_success(&panel);
    let panel_out = stdout(&panel);
    assert!(
        panel_out.contains("sase_agent_recovery.md")
            && panel_out.contains("sase.md"),
        "human status should show the clobber:\n{panel_out}"
    );

    let follow_up = vault_sync_command(&vault, &temp)
        .output()
        .expect("run follow-up vault-sync");
    assert_success(&follow_up);
    let later = read_vault_sync_status(&temp);
    assert_eq!(later["clobbers"].as_array().map(Vec::len), Some(0));
    assert_eq!(later["recent_clobbers"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        later["recent_clobbers"][0]["path"],
        "sase_agent_recovery.md"
    );
}

#[test]
fn vault_sync_dry_run_reports_clobber_without_writing() {
    let temp = TempDir::new("bob-cli-vault-sync-clobber-dry");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    let (recovery, sase) = seed_clobber_vault(&vault);
    let work = clobber_worktree(&sase);
    write_file(&vault.join("sase_agent_recovery.md"), &work);

    let output = vault_sync_command(&vault, &temp)
        .arg("--dry-run")
        .output()
        .expect("run clobber dry-run");

    assert_success(&output);
    let stderr = stderr(&output);
    assert!(
        stderr.contains(
            "would quarantine sase_agent_recovery.md (matches sase.md:"
        ),
        "dry-run should name the would-quarantine action:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase_agent_recovery.md"))
            .expect("read untouched note"),
        work
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase.md")).expect("read sase"),
        sase
    );
    assert!(
        quarantined_conflict_files(&vault).is_empty(),
        "dry-run must not write a conflict copy"
    );
    assert!(
        !vault.join("_conflicts/sync_conflicts.md").exists(),
        "dry-run must not write the conflict log"
    );
    assert!(
        !vault_sync_state_file(&temp).exists(),
        "dry-run must not write the status file"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["show", "HEAD:sase_agent_recovery.md"])),
        recovery
    );
}

#[test]
fn vault_sync_ignores_legitimate_rewrites() {
    let temp = TempDir::new("bob-cli-vault-sync-clobber-negatives");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    let recovery = clobber_recovery_note();
    write_file(&vault.join("sase_agent_recovery.md"), &recovery);
    write_file(&vault.join("sase.md"), &clobber_sase_note());
    write_file(&vault.join("done/archive.md"), &recovery);
    write_file(&vault.join("2026/20261028.md"), &recovery);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed negatives"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    write_file(
        &vault.join("sase_agent_recovery.md"),
        "---\ncreated: 2026-10-19T19:32:43-0400\n---\nleft\n",
    );
    write_file(
        &vault.join("done/archive.md"),
        &clobber_worktree(&clobber_sase_note()),
    );
    write_file(
        &vault.join("2026/20261028.md"),
        &clobber_worktree(&clobber_sase_note()),
    );
    write_file(
        &vault.join("brand_new.md"),
        &clobber_worktree(&clobber_sase_note()),
    );
    write_file(
        &vault.join("sase.md"),
        &format!(
            "{}- [ ] #task Extra copy ^docs-like-zen\n",
            clobber_sase_note()
        ),
    );

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "athena")
        .output()
        .expect("run negative clobber vault-sync");

    assert_success(&output);
    assert!(
        !stderr(&output).contains("quarantine"),
        "legitimate rewrites should not quarantine:\n{}",
        format_output(&output)
    );
    assert!(
        quarantined_conflict_files(&vault).is_empty(),
        "no conflict copies for negatives"
    );
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["clobbers"].as_array().map(Vec::len), Some(0));
}

#[test]
fn vault_sync_detects_clobber_when_shared_ids_sort_last() {
    let temp = TempDir::new("bob-cli-vault-sync-clobber-sort-last");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    let head: String = (0..10)
        .map(|i| format!("Original recovery line {i}\n"))
        .collect();
    let other: String = (0..3)
        .map(|i| format!("- [ ] #task Shared {i} ^z{i}\n"))
        .collect();
    write_file(&vault.join("victim.md"), &head);
    write_file(&vault.join("other.md"), &other);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed sort-last fixture"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    let mut work = String::new();
    for i in 0..5 {
        work.push_str(&format!("- [ ] #task Unique {i} ^a{i}\n"));
    }
    work.push_str(&other);
    write_file(&vault.join("victim.md"), &work);

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "athena")
        .output()
        .expect("run sort-last clobber vault-sync");

    assert_success(&output);
    let stderr = stderr(&output);
    assert!(
        stderr.contains("quarantined clobber of victim.md")
            && stderr.contains("matches other.md"),
        "full 5+3 case should quarantine:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("victim.md")).expect("read victim"),
        head,
        "HEAD bytes should be restored"
    );
    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read_to_string(&copies[0]).expect("read conflict copy"),
        work,
        "quarantined bytes should preserve the full worktree"
    );
    let committed = stdout(&git([
        "--git-dir",
        path_str(&remote),
        "show",
        "master:victim.md",
    ]));
    assert_eq!(committed, head, "clobbered bytes must not reach the remote");
}

#[test]
fn vault_sync_quarantine_with_ignored_conflicts_dir_commits_and_pushes() {
    let temp = TempDir::new("bob-cli-vault-sync-clobber-ignored");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    write_file(&vault.join(".gitignore"), "_conflicts/\n");
    let (recovery, sase) = seed_clobber_vault(&vault);
    let work = clobber_worktree(&sase);
    write_file(&vault.join("sase_agent_recovery.md"), &work);

    let dry = vault_sync_command(&vault, &temp)
        .arg("--dry-run")
        .output()
        .expect("run ignored dry-run");
    assert_success(&dry);
    assert!(
        stderr(&dry).contains("would quarantine sase_agent_recovery.md"),
        "dry-run should name the would-quarantine action:\n{}",
        format_output(&dry)
    );
    assert!(
        quarantined_conflict_files(&vault).is_empty(),
        "dry-run must not write a conflict copy"
    );
    assert!(
        !vault.join("_conflicts/sync_conflicts.md").exists(),
        "dry-run must not write the conflict log"
    );
    assert!(
        !vault_sync_state_file(&temp).exists(),
        "dry-run must not write the status file"
    );

    let output = vault_sync_command(&vault, &temp)
        .env("HOSTNAME", "athena")
        .output()
        .expect("run ignored clobber vault-sync");
    assert_success(&output);
    assert!(
        stderr(&output)
            .contains("quarantined clobber of sase_agent_recovery.md"),
        "expected quarantine warning:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase_agent_recovery.md"))
            .expect("read restored note"),
        recovery
    );
    let copies = quarantined_conflict_files(&vault);
    assert_eq!(copies.len(), 1, "expected one conflict copy: {copies:?}");
    assert_eq!(
        fs::read_to_string(&copies[0]).expect("read conflict copy"),
        work
    );
    let copy_relative = copies[0]
        .strip_prefix(&vault)
        .expect("copy under vault")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["ls-files", "--error-unmatch", &copy_relative]
        )),
        format!("{copy_relative}\n"),
        "ignored conflict copy should be force-added"
    );
    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:sase_agent_recovery.md"
        ])),
        recovery,
        "restored bytes should reach the remote"
    );
    assert!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:_conflicts/sync_conflicts.md"
        ]))
        .contains("clobber"),
        "conflict log should be pushed"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["status", "--porcelain"])),
        "",
        "worktree should be clean after quarantine commit"
    );
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["clobbers"].as_array().map(Vec::len), Some(1));
    assert_eq!(status["recent_clobbers"].as_array().map(Vec::len), Some(1));
}

#[test]
fn vault_sync_reports_confirmed_linked_task_loss_once_after_settle_window() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss");
    let (vault, remote, _peer) = init_vault_sync_pair(&temp);
    let old = "- [ ] #task Auto-label Kelly's emails with Bob ^old-id\n\
- [ ] #task Parent task ^parent\n\
- [ ] #task Today task ^today\n\
- [x] #task Archived task ^archived\n\
[[#^old-id]]\n";
    write_file(&vault.join("bob.md"), old);
    write_file(
        &vault.join("2026/20261109.md"),
        "- [ ] #task Today task [[bob#^old-id]] ^today-link\n",
    );
    write_file(
        &vault.join("done/bob_done.md"),
        "- [x] #task Archived task [[bob#^old-id]] ^done-link\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed loss replay"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    let renamed = old
        .replace(
            "Auto-label Kelly's emails with Bob ^old-id",
            "Auto-label Kelly's emails with Bob ^auto-label-kelly-emails",
        )
        .replace("^old-id", "^auto-label-kelly-emails");
    write_file(&vault.join("bob.md"), &renamed);
    write_file(
        &vault.join("2026/20261109.md"),
        "- [ ] #task Today task [[bob#^auto-label-kelly-emails]] ^today-link\n",
    );
    write_file(
        &vault.join("done/bob_done.md"),
        "- [x] #task Archived task [[bob#^auto-label-kelly-emails]] ^done-link\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "rename task ID"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    write_file(
        &vault.join("bob.md"),
        &renamed.replace(
            "Auto-label Kelly's emails with Bob ^auto-label-kelly-emails",
            "Auto-label Kelly's emails with Bob ^old-id",
        ),
    );
    let first = vault_sync_command(&vault, &temp)
        .output()
        .expect("run first loss-detection cycle");
    assert_success(&first);
    let first_status = read_vault_sync_status(&temp);
    assert_eq!(
        first_status["task_losses"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        first_status["pending_task_losses"].as_array().map(Vec::len),
        Some(1),
        "the first cycle should save one pending linked loss: {first_status}"
    );
    assert_eq!(
        first_status["pending_task_losses"][0]["id"],
        "auto-label-kelly-emails"
    );
    assert_eq!(first_status["pending_task_losses"][0]["kind"], "linked");

    make_pending_task_losses_old(&temp);
    let second = vault_sync_command(&vault, &temp)
        .output()
        .expect("run settle confirmation cycle");
    assert_success(&second);
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["task_losses"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        status["recent_task_losses"].as_array().map(Vec::len),
        Some(1)
    );
    let loss = &status["task_losses"][0];
    assert_eq!(loss["id"], "auto-label-kelly-emails");
    assert_eq!(loss["kind"], "linked");
    assert_eq!(loss["path"], "bob.md");
    assert_eq!(loss["inbound_links"].as_array().map(Vec::len), Some(3));
    assert!(loss["restore_hint"].as_str().unwrap().contains("git -C "));
    assert!(loss["restore_hint"]
        .as_str()
        .unwrap()
        .contains("sed -n '1p'"));
    let log = fs::read_to_string(vault.join("_conflicts/sync_conflicts.md"))
        .expect("read task-loss log");
    assert_eq!(log.matches("task loss (linked)").count(), 1, "{log}");
    assert!(log.contains("auto-label-kelly-emails"), "{log}");
    assert!(
        log.contains(loss["restore_hint"].as_str().unwrap()),
        "{log}"
    );
    assert_eq!(
        stdout(&git([
            "--git-dir",
            path_str(&remote),
            "show",
            "master:_conflicts/sync_conflicts.md",
        ])),
        log,
        "the report log should be committed and pushed"
    );

    let panel = vault_sync_command(&vault, &temp)
        .arg("status")
        .output()
        .expect("run vault-sync status panel");
    assert_success(&panel);
    assert!(stdout(&panel).contains("recent_task_losses:"));
    assert!(stdout(&panel).contains("auto-label-kelly-emails"));
}

#[test]
fn vault_sync_ignores_task_moves_archives_and_rewritten_hand_renames() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss-negatives");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("bob.md"),
        "- [ ] #task Move task ^move-id\n\
- [ ] #task Archive task ^archive-id\n\
- [ ] #task Rename task ^rename-old\n\
- [ ] #task Deleted open task ^unlinked-id\n\
[[#^move-id]] [[#^archive-id]] [[#^rename-old]]\n",
    );
    write_file(
        &vault.join("legacy.md"),
        "A pre-existing dangling link: [[missing#^legacy-dangling]].\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed task-loss negatives"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    write_file(
        &vault.join("bob.md"),
        "- [ ] #task Archive task ^archive-id\n\
- [ ] #task Rename task ^rename-new\n\
[[#^archive-id]] [[#^rename-new]]\n",
    );
    write_file(&vault.join("moved.md"), "- [ ] #task Move task ^move-id\n");
    write_file(
        &vault.join("done/archive.md"),
        "- [ ] #task Archive task ^archive-id\n",
    );
    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run move/archive/rename cycle");
    assert_success(&output);
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["task_losses"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        status["pending_task_losses"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(status["pending_task_losses"][0]["id"], "unlinked-id");
    assert_eq!(status["pending_task_losses"][0]["kind"], "unlinked");
}

#[test]
fn vault_sync_drops_pending_loss_when_task_reappears() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss-reappears");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Task to restore ^reappears\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed pending loss"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    write_file(&vault.join("tasks.md"), "- [ ] #task Other task ^other\n");

    let first = vault_sync_command(&vault, &temp)
        .output()
        .expect("run candidate cycle");
    assert_success(&first);
    assert_eq!(
        read_vault_sync_status(&temp)["pending_task_losses"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Task to restore ^reappears\n- [ ] #task Other task ^other\n",
    );
    let second = vault_sync_command(&vault, &temp)
        .output()
        .expect("run reappearance cycle");
    assert_success(&second);
    let status = read_vault_sync_status(&temp);
    assert_eq!(
        status["pending_task_losses"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(status["task_losses"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        status["recent_task_losses"].as_array().map(Vec::len),
        Some(0)
    );
    assert!(!vault.join("_conflicts/sync_conflicts.md").exists());
}

#[test]
fn vault_sync_dry_run_reports_task_loss_without_writes() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss-dry");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Task to lose ^dry-loss\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed dry-run task"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    write_file(&vault.join("tasks.md"), "- [ ] #task Other task ^other\n");

    let output = vault_sync_command(&vault, &temp)
        .arg("--dry-run")
        .output()
        .expect("run task-loss dry run");
    assert_success(&output);
    assert!(
        stderr(&output).contains("would report unlinked task loss dry-loss"),
        "dry run should explain the candidate:\n{}",
        format_output(&output)
    );
    assert!(!vault_sync_state_file(&temp).exists());
    assert!(!vault.join("_conflicts/sync_conflicts.md").exists());
    assert_eq!(
        stdout(&git_in(&vault, ["status", "--porcelain"]))
            .lines()
            .count(),
        1
    );
    assert_eq!(
        stdout(&git_in(&vault, ["show", "HEAD:tasks.md"])),
        "- [ ] #task Task to lose ^dry-loss\n"
    );
}

#[test]
fn vault_sync_legacy_same_id_danglers_are_unlinked_not_linked() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss-legacy-id");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Shared id task ^shared-id\n\
- [ ] #task Surviving task ^surviving\n",
    );
    write_file(
        &vault.join("legacy.md"),
        "A pre-existing local dangler: [[#^shared-id]]\n\
And an unrelated explicit target: [[other#^shared-id]]\n",
    );
    write_file(&vault.join("other.md"), "unrelated note\n");
    git_in(&vault, ["add", "."]);
    git_in(
        &vault,
        ["commit", "-q", "-m", "seed shared-id with legacy danglers"],
    );
    git_in(&vault, ["push", "-q", "origin", "master"]);

    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Surviving task ^surviving\n",
    );
    let first = vault_sync_command(&vault, &temp)
        .output()
        .expect("run legacy-dangler candidate cycle");
    assert_success(&first);
    let first_status = read_vault_sync_status(&temp);
    assert_eq!(
        first_status["pending_task_losses"].as_array().map(Vec::len),
        Some(1),
        "legacy danglers must not mint a linked loss: {first_status}"
    );
    assert_eq!(first_status["pending_task_losses"][0]["id"], "shared-id");
    assert_eq!(first_status["pending_task_losses"][0]["kind"], "unlinked");
    assert_eq!(
        first_status["pending_task_losses"][0]["inbound_links"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    make_pending_task_losses_old(&temp);
    let second = vault_sync_command(&vault, &temp)
        .output()
        .expect("run legacy-dangler settle cycle");
    assert_success(&second);
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["task_losses"].as_array().map(Vec::len), Some(1));
    assert_eq!(status["task_losses"][0]["kind"], "unlinked");
    assert_eq!(
        status["task_losses"][0]["inbound_links"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "unlinked losses record no inbound links and do not notify"
    );
}

#[test]
fn vault_sync_closed_task_with_legacy_dangler_is_not_a_loss() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss-closed-legacy");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("tasks.md"),
        "- [x] #task Closed shared id ^shared-id\n\
- [ ] #task Surviving task ^surviving\n",
    );
    write_file(&vault.join("legacy.md"), "[[#^shared-id]]\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed closed shared-id"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Surviving task ^surviving\n",
    );

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run closed-task legacy-dangler cycle");
    assert_success(&output);
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["task_losses"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        status["pending_task_losses"].as_array().map(Vec::len),
        Some(0),
        "a closed deletion with only a legacy dangler is not a loss: {status}"
    );
}

#[test]
fn vault_sync_counts_resolved_same_note_and_path_alias_links() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss-resolved-links");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Linked task ^keep-id\n[[#^keep-id]]\n",
    );
    write_file(
        &vault.join("daily.md"),
        "[[tasks#^keep-id]] [[tasks.md#^keep-id|the task]]\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(
        &vault,
        ["commit", "-q", "-m", "seed resolved inbound links"],
    );
    git_in(&vault, ["push", "-q", "origin", "master"]);
    write_file(&vault.join("tasks.md"), "[[#^keep-id]]\n");

    let first = vault_sync_command(&vault, &temp)
        .output()
        .expect("run resolved-link candidate cycle");
    assert_success(&first);
    let first_status = read_vault_sync_status(&temp);
    assert_eq!(
        first_status["pending_task_losses"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(first_status["pending_task_losses"][0]["kind"], "linked");
    let mut inbound = first_status["pending_task_losses"][0]["inbound_links"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    inbound.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    assert_eq!(
        inbound,
        vec![
            serde_json::Value::String("daily.md".to_string()),
            serde_json::Value::String("tasks.md".to_string()),
        ]
    );
}

#[test]
fn vault_sync_detects_whole_note_deletion_from_remaining_links() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss-whole-note");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("tasks.md"),
        "- [ ] #task Whole note task ^gone-id\n",
    );
    write_file(&vault.join("daily.md"), "[[tasks#^gone-id]]\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed whole-note deletion"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    fs::remove_file(vault.join("tasks.md")).expect("delete tasks.md");

    let first = vault_sync_command(&vault, &temp)
        .output()
        .expect("run whole-note deletion cycle");
    assert_success(&first);
    let first_status = read_vault_sync_status(&temp);
    assert_eq!(
        first_status["pending_task_losses"].as_array().map(Vec::len),
        Some(1),
        "deleting the note must still count remaining links: {first_status}"
    );
    assert_eq!(first_status["pending_task_losses"][0]["id"], "gone-id");
    assert_eq!(first_status["pending_task_losses"][0]["path"], "tasks.md");
    assert_eq!(first_status["pending_task_losses"][0]["kind"], "linked");
    assert_eq!(
        first_status["pending_task_losses"][0]["inbound_links"][0],
        "daily.md"
    );
}

#[test]
fn vault_sync_does_not_report_remote_only_task_removal() {
    let temp = TempDir::new("bob-cli-vault-sync-task-loss-remote");
    let (vault, _remote, peer) = init_vault_sync_pair(&temp);
    write_file(
        &vault.join("remote-task.md"),
        "- [ ] #task Remote task ^remote-only\n",
    );
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "seed remote-only case"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    git_in(&peer, ["pull", "-q", "--ff-only", "origin", "master"]);
    write_file(
        &peer.join("remote-task.md"),
        "- [ ] #task Changed remotely\n",
    );
    git_in(&peer, ["add", "."]);
    git_in(&peer, ["commit", "-q", "-m", "remote task removal"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    let output = vault_sync_command(&vault, &temp)
        .output()
        .expect("run remote-only removal cycle");
    assert_success(&output);
    let status = read_vault_sync_status(&temp);
    assert_eq!(status["task_losses"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        status["pending_task_losses"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        fs::read_to_string(vault.join("remote-task.md"))
            .expect("read remote task"),
        "- [ ] #task Changed remotely\n"
    );
}

fn make_pending_task_losses_old(temp: &TempDir) {
    let path = vault_sync_state_file(temp);
    let mut status = read_vault_sync_status(temp);
    for pending in status["pending_task_losses"]
        .as_array_mut()
        .expect("pending losses array")
    {
        pending["first_seen"] =
            serde_json::Value::String("2020-01-01T00:00:00Z".to_string());
    }
    write_file(
        &path,
        &serde_json::to_string_pretty(&status).expect("encode status JSON"),
    );
}

fn done_tasks_source(count: usize) -> String {
    let mut text = String::from("---\ntype: [[area]]\n---\n\n");
    for index in 1..=count {
        text.push_str(&format!(
            "- [x] done {index} #task [completion:: 2000-01-01]\n"
        ));
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
