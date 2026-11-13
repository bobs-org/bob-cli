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

fn changed_paths(payload: &Value) -> Vec<String> {
    payload["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

fn diff_for<'a>(payload: &'a Value, path: &str) -> &'a str {
    payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == path)
        .and_then(|change| change["diff"].as_str())
        .unwrap_or_else(|| panic!("missing diff for {path}: {payload}"))
}

#[test]
fn dry_run_shows_index_and_catalog_diffs_without_writing() {
    let temp = TempDir::new("album-sync-dry");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(&vault, "eat.md", ALBUM);
    write(&vault, "eat/one.md", RECORD);
    let before_album = fs::read(vault.join("eat.md")).unwrap();
    let before_body = fs::read(vault.join("body.md")).unwrap();
    let output = run(&vault, &temp, &["sync", "-d", "-f", "json"]);

    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["dry_run"], true);
    assert_eq!(changed_paths(&payload), ["body.md", "eat.md"]);
    assert!(diff_for(&payload, "eat.md").contains("> [!records] 1 restaurant"));
    assert!(diff_for(&payload, "body.md").contains("> [!albums]"));
    assert_eq!(payload["catalogs"][0]["path"], "body.md");
    assert_eq!(payload["catalogs"][0]["state"], "missing");
    assert_eq!(fs::read(vault.join("eat.md")).unwrap(), before_album);
    assert_eq!(fs::read(vault.join("body.md")).unwrap(), before_body);
    assert!(!vault.join(".git").exists());
}

#[test]
fn real_sync_commits_indexes_and_catalogs_then_becomes_a_noop() {
    let temp = TempDir::new("album-sync-real");
    let vault = fixture(&temp, false);
    let before = head(&vault);

    let first = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(first.status.code(), Some(0), "{}", format_output(&first));
    let payload = json(&first);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "sync");
    assert_eq!(payload["commit"]["subject"], "albums: sync (2 notes)");
    assert_eq!(changed_paths(&payload), ["body.md", "eat.md"]);
    assert_eq!(payload["albums"][0]["index"]["state"], "current");
    assert_eq!(payload["catalogs"][0]["state"], "current");
    assert_ne!(head(&vault), before);
    assert_eq!(subject(&vault), "albums: sync (2 notes)");
    let body = fs::read_to_string(vault.join("body.md")).unwrap();
    assert!(body.contains("> [!albums]"), "{body}");
    assert!(body.contains("[[eat|Restaurants]]"), "{body}");
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        ))
        .trim(),
        "body.md\neat.md"
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
    assert_eq!(payload["commit"]["subject"], "albums: sync (2 notes)");
    assert_eq!(changed_paths(&payload), ["body.md", "eat.md"]);
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
    assert_eq!(changed_paths(&payload), ["body.md", "eat.md"]);
    assert_eq!(payload["albums"][0]["index"]["state"], "current");
    let repaired = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert_eq!(repaired.matches("> [!records]").count(), 1, "{repaired}");
    assert_eq!(repaired.matches("## Records").count(), 1, "{repaired}");
    assert!(repaired.contains("> [!records] 1 restaurant"), "{repaired}");
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
    assert!(text.contains("step 6/8 · notebooks"), "{text}");
    assert!(text.contains("step 7/8 · albums"), "{text}");
    assert!(text.contains("step 8/8 · vault-sync"), "{text}");
    assert!(text.contains("albums failed"), "{text}");
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "origin/master"])).trim(),
        head(&vault),
        "nightly's final vault-sync still publishes the healthy album index"
    );
    let body = fs::read_to_string(vault.join("body.md")).unwrap();
    assert!(body.contains("> [!albums]"), "{body}");
    assert!(
        stdout(&git_in(
            &vault,
            ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        ))
        .contains("body.md"),
        "nightly also published the owner catalog"
    );
}

#[test]
fn reparenting_updates_both_owner_catalogs_in_one_commit() {
    let temp = TempDir::new("album-sync-reparent");
    let vault = fixture(&temp, false);
    write(&vault, "mind.md", "---\ntype: [[area]]\n---\n\n# Mind\n");
    commit_all(&vault, "add mind");
    assert_eq!(run(&vault, &temp, &["sync"]).status.code(), Some(0));

    let eat = fs::read_to_string(vault.join("eat.md")).unwrap();
    write(&vault, "eat.md", &eat.replace("[[body]]", "[[mind]]"));
    commit_all(&vault, "reparent eat");
    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(changed_paths(&payload), ["body.md", "mind.md"]);
    let body = fs::read_to_string(vault.join("body.md")).unwrap();
    assert!(!body.contains("> [!albums]"), "{body}");
    let mind = fs::read_to_string(vault.join("mind.md")).unwrap();
    assert!(mind.contains("[[eat|Restaurants]]"), "{mind}");
}

#[test]
fn catalog_only_changes_commit_without_touching_indexes() {
    let temp = TempDir::new("album-sync-catalog-only");
    let vault = fixture(&temp, false);
    assert_eq!(run(&vault, &temp, &["sync"]).status.code(), Some(0));
    let eat_before = fs::read(vault.join("eat.md")).unwrap();
    write(
        &vault,
        "body.md",
        "---\ntype: [[area]]\n---\n\n# Body\n\n## Albums\n\n> [!albums]\n>\n> - stale\n",
    );
    commit_all(&vault, "stale catalog");

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(changed_paths(&payload), ["body.md"]);
    assert_eq!(payload["catalogs"][0]["state"], "current");
    assert_eq!(fs::read(vault.join("eat.md")).unwrap(), eat_before);
    let body = fs::read_to_string(vault.join("body.md")).unwrap();
    assert!(body.contains("[[eat|Restaurants]]"), "{body}");
}

#[test]
fn damaged_host_catalog_is_skipped_while_healthy_hosts_commit() {
    let temp = TempDir::new("album-sync-damaged-host");
    let vault = fixture(&temp, false);
    write(&vault, "mind.md", "---\ntype: [[area]]\n---\n\n# Mind\n");
    write(
        &vault,
        "movies.md",
        "---\ntype: [[album]]\nparent: \"[[mind]]\"\nsummary: Films.\nrecord:\n  noun: movie\n---\n\n# Movies\n",
    );
    write(
        &vault,
        "body.md",
        "---\ntype: [[area]]\n---\n\n# Body\n\n## Albums\n\n> [!albums\n> - partial\n",
    );
    commit_all(&vault, "damage body catalog");

    let damaged_before = fs::read(vault.join("body.md")).unwrap();
    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], false);
    assert_eq!(fs::read(vault.join("body.md")).unwrap(), damaged_before);
    let catalogs = payload["catalogs"].as_array().unwrap();
    let body = catalogs
        .iter()
        .find(|catalog| catalog["path"] == "body.md")
        .unwrap();
    assert_eq!(body["state"], "damaged");
    assert_eq!(body["issues"][0]["code"], "album_catalog_damaged");
    let changed = changed_paths(&payload);
    assert!(changed.contains(&"eat.md".to_owned()), "{changed:?}");
    assert!(changed.contains(&"mind.md".to_owned()), "{changed:?}");
    assert!(!changed.contains(&"body.md".to_owned()), "{changed:?}");
    let mind = fs::read_to_string(vault.join("mind.md")).unwrap();
    assert!(mind.contains("[[movies|Movies]]"), "{mind}");
}

#[test]
fn capture_cannot_target_the_generated_albums_section() {
    let temp = TempDir::new("album-capture-protect");
    let vault = fixture(&temp, false);
    write(
        &vault,
        "body.md",
        "---\ntype: [[area]]\n---\n\n# Body\n\n## Albums\n\n> [!albums]\n>\n> - 🍽️ **[[eat|Restaurants]]** — Places.\n\n## Ideas\n\n- keep\n",
    );
    commit_all(&vault, "seed catalog");
    let before = fs::read_to_string(vault.join("body.md")).unwrap();

    for target in ["@body#albums", "@body#album", "@body#Albums", "@body#al"] {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg(format!("an idea {target}"))
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", "2026-11-09")
            .env("NO_COLOR", "1");
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "{target}: {}",
            format_output(&output)
        );
        let text = format_output(&output);
        assert!(text.contains("generated album catalog"), "{target}: {text}");
        assert!(
            text.contains("albums_section_generated")
                || text.contains("generated album catalog"),
            "{target}: {text}"
        );
        assert_eq!(
            fs::read_to_string(vault.join("body.md")).unwrap(),
            before,
            "{target} wrote nothing"
        );
    }

    let mut dry = bob_command();
    dry.arg("capture")
        .arg("--dry-run")
        .arg("an idea @body#albums")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-11-09")
        .env("NO_COLOR", "1");
    let dry_output = dry.output().unwrap();
    assert!(
        !dry_output.status.success(),
        "{}",
        format_output(&dry_output)
    );
    assert_eq!(fs::read_to_string(vault.join("body.md")).unwrap(), before);

    let mut neighbor = bob_command();
    neighbor
        .arg("capture")
        .arg("an idea @body#Ideas")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-11-09")
        .env("NO_COLOR", "1");
    let neighbor_output = neighbor.output().unwrap();
    assert!(
        neighbor_output.status.success(),
        "{}",
        format_output(&neighbor_output)
    );
    let after = fs::read_to_string(vault.join("body.md")).unwrap();
    assert!(after.contains("- an idea"), "{after}");
    assert!(after.contains("> [!albums]"), "{after}");

    let draft = "@body#";
    let complete = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("capture-complete");
    assert_eq!(
        complete.status.code(),
        Some(0),
        "{}",
        format_output(&complete)
    );
    let payload: Value =
        serde_json::from_str(stdout(&complete).trim()).expect("json");
    let labels = payload["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| {
            row["title"]
                .as_str()
                .or_else(|| row["replacement"].as_str())
                .or_else(|| row["label"].as_str())
        })
        .collect::<Vec<_>>();
    assert!(
        !labels
            .iter()
            .any(|label| label.to_lowercase().contains("albums")),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.contains("Ideas")),
        "{labels:?}"
    );
}
