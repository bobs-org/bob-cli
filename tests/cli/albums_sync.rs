//! Album records-index maintenance, guarded writes, and nightly integration.

use crate::support::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const ALBUM: &str = "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places.\nrecord:\n  noun: restaurant\n  fields:\n    status:\n      values: {to_try: To try, liked: Liked}\n---\n\n# Restaurants\n";
const RECORD: &str = "---\ntype: [[record]]\nparent: \"[[eat]]\"\ntitle: One\nstatus: to_try\n---\n\n# One\n";
const BROKEN_ALBUM: &str = "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken.\nrecord:\n  fields: {}\n---\n\n# Broken\n\n## Keep this text\n";

fn write(vault: &Path, path: &str, contents: &str) {
    write_file(&vault.join(path), contents);
}

fn fixture(temp: &TempDir, with_broken: bool) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(&vault, "eat.md", ALBUM);
    write(&vault, "eat/one.md", RECORD);
    if with_broken {
        fs::create_dir_all(vault.join("broken")).expect("broken folder");
        write(&vault, "broken.md", BROKEN_ALBUM);
        write(
            &vault,
            "broken/record.md",
            &RECORD.replace("[[eat]]", "[[broken]]"),
        );
    }
    commit_all(&vault, "album fixture");
    vault
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q"]);
}

fn albums_command(vault: &Path, temp: &TempDir, args: &[&str]) -> Command {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("home");
    let mut command = bob_command();
    command
        .arg("albums")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", "2026-11-09 09:14:00")
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    command
}

fn run(vault: &Path, temp: &TempDir, args: &[&str]) -> Output {
    albums_command(vault, temp, args)
        .output()
        .expect("run bob albums")
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("expected JSON ({error}):\n{}", format_output(output))
    })
}

fn head(vault: &Path) -> String {
    stdout(&git_in(vault, ["rev-parse", "HEAD"]))
        .trim()
        .to_owned()
}

fn subject(vault: &Path) -> String {
    stdout(&git_in(vault, ["log", "-1", "--format=%s"]))
        .trim()
        .to_owned()
}

#[test]
fn dry_run_shows_index_diff_without_writing() {
    let temp = TempDir::new("album-sync-dry");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(&vault, "eat.md", ALBUM);
    write(&vault, "eat/one.md", RECORD);
    let before_album = fs::read(vault.join("eat.md")).unwrap();
    let output = run(&vault, &temp, &["sync", "-d", "-f", "json"]);

    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["changed"], serde_json::json!(["eat.md"]));
    assert!(payload["changes"][0]["diff"]
        .as_str()
        .unwrap()
        .contains("> [!records] 1 restaurants"));
    assert_eq!(fs::read(vault.join("eat.md")).unwrap(), before_album);
    assert!(!vault.join(".git").exists());
}

#[test]
fn real_sync_commits_only_changed_albums_and_then_becomes_a_noop() {
    let temp = TempDir::new("album-sync-real");
    let vault = fixture(&temp, false);
    let before = head(&vault);
    let body_before = fs::read(vault.join("body.md")).unwrap();

    let first = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(first.status.code(), Some(0), "{}", format_output(&first));
    let payload = json(&first);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "sync");
    assert_eq!(payload["commit"]["subject"], "albums: sync (1 notes)");
    assert_eq!(payload["changed"], serde_json::json!(["eat.md"]));
    assert_eq!(payload["albums"][0]["index"]["state"], "current");
    assert_ne!(head(&vault), before);
    assert_eq!(subject(&vault), "albums: sync (1 notes)");
    assert_eq!(fs::read(vault.join("body.md")).unwrap(), body_before);
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        ))
        .trim(),
        "eat.md"
    );

    let after_first = head(&vault);
    let second = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(second.status.code(), Some(0), "{}", format_output(&second));
    let payload = json(&second);
    assert_eq!(payload["changed"], serde_json::json!([]));
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"]["post"], "skipped");
    assert_eq!(head(&vault), after_first);
}

#[test]
fn damaged_album_stays_unchanged_while_healthy_indexes_commit() {
    let temp = TempDir::new("album-sync-partial");
    let vault = fixture(&temp, true);
    let broken_before = fs::read(vault.join("broken.md")).unwrap();

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["commit"]["subject"], "albums: sync (1 notes)");
    assert_eq!(payload["changed"], serde_json::json!(["eat.md"]));
    let broken = payload["albums"]
        .as_array()
        .unwrap()
        .iter()
        .find(|album| album["stem"] == "broken")
        .unwrap();
    assert_eq!(broken["records"], 1);
    assert_eq!(broken["index"]["state"], "damaged");
    assert_eq!(fs::read(vault.join("broken.md")).unwrap(), broken_before);
    assert!(vault.join("eat.md").exists());
}

#[test]
fn hand_edited_and_duplicate_index_regions_are_repaired() {
    let temp = TempDir::new("album-sync-edited");
    let vault = fixture(&temp, false);
    let stale = format!(
        "{ALBUM}\n## Records\n\n> [!records] stale\n> - old\n\n## Records\n\n> [!records] duplicate\n> - old\n"
    );
    write(&vault, "eat.md", &stale);
    commit_all(&vault, "hand edit records index");

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["changed"], serde_json::json!(["eat.md"]));
    assert_eq!(payload["albums"][0]["index"]["state"], "current");
    let repaired = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert_eq!(repaired.matches("> [!records]").count(), 1, "{repaired}");
    assert_eq!(repaired.matches("## Records").count(), 1, "{repaired}");
    assert!(
        repaired.contains("> [!records] 1 restaurants"),
        "{repaired}"
    );
}

#[test]
fn nightly_continues_after_album_failure_and_publishes_healthy_indexes() {
    let temp = TempDir::new("album-nightly");
    let vault = fixture(&temp, true);
    // Put an invalid album schema in the vault; nightly reports this step as
    // failed but still runs its final publish step.
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("home");
    let mut command = bob_command();
    command
        .arg("nightly")
        .env("BOB_DIR", &vault)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(&temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(&temp))
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    let output = command.output().expect("run nightly");
    let text = format_output(&output);
    assert_ne!(output.status.code(), Some(0), "{text}");
    assert!(text.contains("step 5/7 · notebooks"), "{text}");
    assert!(text.contains("step 6/7 · albums"), "{text}");
    assert!(text.contains("step 7/7 · vault-sync"), "{text}");
    assert!(text.contains("albums failed"), "{text}");
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "origin/master"])).trim(),
        head(&vault),
        "nightly's final vault-sync still publishes the healthy album index"
    );
}
