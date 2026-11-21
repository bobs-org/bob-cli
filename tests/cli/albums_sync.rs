//! Album records-index maintenance, guarded writes, and nightly integration.

use crate::support::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const ALBUM: &str = "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places.\nrecord:\n  noun: restaurant\n  fields:\n    status:\n      values: {to_try: To try, liked: Liked}\n---\n\n# Restaurants\n";
const RECORD: &str = "---\ntype: [[record]]\nparent: \"[[eat]]\"\ntitle: One\nstatus: to_try\n---\n";
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

fn walk_fixture(temp: &TempDir, records: &[(&str, &str)]) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(&vault, "eat.md", ALBUM);
    for (stem, title) in records {
        let record = RECORD.replace("title: One", &format!("title: {title}"));
        write(&vault, &format!("eat/{stem}.md"), &record);
    }
    commit_all(&vault, "walk fixture");
    vault
}

fn walk_line(vault: &Path, relative: &str) -> String {
    fs::read_to_string(vault.join(relative))
        .expect("record")
        .lines()
        .find(|line| line.contains("|↑ "))
        .unwrap_or_else(|| panic!("missing walk line in {relative}"))
        .to_owned()
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
    assert_eq!(changed_paths(&payload), ["body.md", "eat.md", "eat/one.md"]);
    assert!(diff_for(&payload, "eat.md").contains("> [!records] 1 restaurant"));
    assert!(diff_for(&payload, "body.md").contains("> [!albums]"));
    assert!(diff_for(&payload, "eat/one.md").contains("[[eat|↑"));
    let kinds: Vec<(&str, &str)> = payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| {
            (
                change["path"].as_str().unwrap(),
                change["kind"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(kinds.contains(&("eat.md", "index")));
    assert!(kinds.contains(&("body.md", "catalog")));
    assert!(kinds.contains(&("eat/one.md", "walk")));
    assert_eq!(payload["counts"]["walk_lines"], 1);
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
    assert_eq!(payload["commit"]["subject"], "albums: sync (3 notes)");
    assert_eq!(changed_paths(&payload), ["body.md", "eat.md", "eat/one.md"]);
    assert_eq!(payload["counts"]["walk_lines"], 1);
    assert_eq!(payload["albums"][0]["index"]["state"], "current");
    assert_eq!(payload["albums"][0]["walk"]["state"], "current");
    assert_eq!(payload["catalogs"][0]["state"], "current");
    assert_ne!(head(&vault), before);
    assert_eq!(subject(&vault), "albums: sync (3 notes)");
    let body = fs::read_to_string(vault.join("body.md")).unwrap();
    assert!(body.contains("> [!albums]"), "{body}");
    assert!(body.contains("[[eat|Restaurants]]"), "{body}");
    let one = fs::read_to_string(vault.join("eat/one.md")).unwrap();
    assert!(one.contains("[[eat|↑"), "{one}");
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        ))
        .trim(),
        "body.md\neat.md\neat/one.md"
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
fn walk_sync_writes_two_and_three_member_rings() {
    let temp = TempDir::new("album-sync-walk-rings");
    let vault = walk_fixture(&temp, &[("apple", "Apple"), ("mango", "Mango")]);

    let first = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(first.status.code(), Some(0), "{}", format_output(&first));
    let payload = json(&first);
    assert_eq!(payload["counts"]["walk_lines"], 2);
    assert_eq!(
        walk_line(&vault, "eat/apple.md"),
        "[[eat/mango|‹ Mango]] · [[eat|↑ Restaurants]] · [[eat/mango|Mango ›]]"
    );
    assert_eq!(
        walk_line(&vault, "eat/mango.md"),
        "[[eat/apple|‹ Apple]] · [[eat|↑ Restaurants]] · [[eat/apple|Apple ›]]"
    );
    let lime = RECORD.replace("title: One", "title: Lime");
    write(&vault, "eat/lime.md", &lime);
    commit_all(&vault, "add third record");
    let second = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(second.status.code(), Some(0), "{}", format_output(&second));
    let payload = json(&second);
    assert_eq!(payload["counts"]["walk_lines"], 3);
    let changed_walk_lines = payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|change| change["kind"] == "walk")
        .count();
    assert_eq!(payload["counts"]["walk_lines"], changed_walk_lines);
    assert_eq!(
        walk_line(&vault, "eat/apple.md"),
        "[[eat/mango|‹ Mango]] · [[eat|↑ Restaurants]] · [[eat/lime|Lime ›]]"
    );
    assert_eq!(
        walk_line(&vault, "eat/lime.md"),
        "[[eat/apple|‹ Apple]] · [[eat|↑ Restaurants]] · [[eat/mango|Mango ›]]"
    );
    assert_eq!(
        walk_line(&vault, "eat/mango.md"),
        "[[eat/lime|‹ Lime]] · [[eat|↑ Restaurants]] · [[eat/apple|Apple ›]]"
    );

    let third = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(third.status.code(), Some(0), "{}", format_output(&third));
    let payload = json(&third);
    assert_eq!(changed_paths(&payload), Vec::<String>::new());
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"]["post"], "skipped");
}

#[test]
fn walk_sync_repairs_only_the_retitle_and_delete_radius() {
    let temp = TempDir::new("album-sync-walk-radius");
    let vault = walk_fixture(
        &temp,
        &[
            ("alpha", "Alpha"),
            ("bravo", "Bravo"),
            ("charlie", "Charlie"),
            ("delta", "Delta"),
            ("echo", "Echo"),
        ],
    );
    let initial = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(
        initial.status.code(),
        Some(0),
        "{}",
        format_output(&initial)
    );
    let charlie_before = fs::read(vault.join("eat/charlie.md")).unwrap();
    let echo_path = vault.join("eat/echo.md");
    let echo = fs::read_to_string(&echo_path).unwrap();
    write(
        &vault,
        "eat/echo.md",
        &echo.replace("title: Echo", "title: Between"),
    );
    commit_all(&vault, "retitle echo between alpha and bravo");
    let retitle = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(
        retitle.status.code(),
        Some(0),
        "{}",
        format_output(&retitle)
    );
    let payload = json(&retitle);
    assert!(payload["counts"]["walk_lines"].as_u64().unwrap() <= 5);
    assert_eq!(
        fs::read(vault.join("eat/charlie.md")).unwrap(),
        charlie_before,
        "Charlie is outside the old and new neighbourhoods"
    );

    fs::remove_file(&echo_path).expect("delete retitled record");
    commit_all(&vault, "delete between record");
    let delete = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(delete.status.code(), Some(0), "{}", format_output(&delete));
    let payload = json(&delete);
    assert_eq!(payload["counts"]["walk_lines"], 2);
    let walk_paths = payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|change| change["kind"] == "walk")
        .map(|change| change["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(walk_paths, ["eat/alpha.md", "eat/bravo.md"]);
}

#[test]
fn walk_sync_leaves_a_damaged_line_and_warns() {
    let temp = TempDir::new("album-sync-walk-damaged");
    let vault = walk_fixture(&temp, &[("apple", "Apple"), ("mango", "Mango")]);
    let damaged_line =
        "[[eat/mango|‹ Mango]] · [[eat|↑ Restaurants]] · [[eat/mango|Mango ›]] extra";
    let apple = format!(
        "{}\n{damaged_line}\n",
        RECORD.replace("title: One", "title: Apple")
    );
    write(&vault, "eat/apple.md", &apple);
    commit_all(&vault, "add damaged walk line");
    let apple_before = fs::read(vault.join("eat/apple.md")).unwrap();

    let check = run(&vault, &temp, &["check", "-f", "json"]);
    assert_eq!(check.status.code(), Some(0), "{}", format_output(&check));
    let payload = json(&check);
    let album = payload["albums"]
        .as_array()
        .unwrap()
        .iter()
        .find(|album| album["stem"] == "eat")
        .unwrap();
    assert_eq!(album["walk"]["state"], "damaged");
    assert_eq!(album["walk"]["counts"]["damaged"], 1);
    let warning = album["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "album_walk_damaged")
        .expect("damaged walk warning");
    assert!(warning["message"].as_str().unwrap().contains("1 records"));
    assert!(warning["message"]
        .as_str()
        .unwrap()
        .contains("eat/apple.md"));

    let sync = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(sync.status.code(), Some(0), "{}", format_output(&sync));
    let payload = json(&sync);
    assert_eq!(payload["counts"]["walk_lines"], 1);
    assert_eq!(fs::read(vault.join("eat/apple.md")).unwrap(), apple_before);
    assert_eq!(
        walk_line(&vault, "eat/mango.md"),
        "[[eat/apple|‹ Apple]] · [[eat|↑ Restaurants]] · [[eat/apple|Apple ›]]"
    );
}

#[test]
fn walk_sync_holds_an_album_with_an_unreadable_member() {
    let temp = TempDir::new("album-sync-walk-held");
    let vault = fixture(&temp, false);
    let held_member_before = fs::read(vault.join("eat/one.md")).unwrap();
    let unreadable = b"---\ntitle: \xff\n---\n";
    fs::write(vault.join("eat/unreadable.md"), unreadable).unwrap();
    fs::create_dir_all(vault.join("movies")).expect("movies folder");
    write(
        &vault,
        "movies.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Films.\nrecord:\n  noun: movie\n---\n\n# Movies\n",
    );
    write(
        &vault,
        "movies/film.md",
        &RECORD
            .replace("[[eat]]", "[[movies]]")
            .replace("title: One", "title: Film"),
    );
    commit_all(&vault, "add unreadable and healthy albums");

    let listed = run(&vault, &temp, &["list", "-f", "json"]);
    let payload = json(&listed);
    let held = payload["albums"]
        .as_array()
        .unwrap()
        .iter()
        .find(|album| album["stem"] == "eat")
        .unwrap();
    assert_eq!(held["walk"]["state"], "held");
    assert_eq!(held["walk"]["order"], serde_json::json!([]));

    let sync = run(&vault, &temp, &["sync", "-f", "json"]);
    let payload = json(&sync);
    assert_eq!(
        payload["counts"]["walk_lines"],
        1,
        "{}",
        format_output(&sync)
    );
    assert_eq!(
        fs::read(vault.join("eat/one.md")).unwrap(),
        held_member_before
    );
    assert_eq!(
        fs::read(vault.join("eat/unreadable.md")).unwrap(),
        unreadable
    );
    assert_eq!(walk_line(&vault, "movies/film.md"), "[[movies|↑ Movies]]");
    assert!(changed_paths(&payload).contains(&"movies/film.md".to_owned()));
    assert!(!changed_paths(&payload).contains(&"eat/one.md".to_owned()));
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
    assert_eq!(payload["commit"]["subject"], "albums: sync (4 notes)");
    assert_eq!(
        changed_paths(&payload),
        ["body.md", "broken/record.md", "eat.md", "eat/one.md"]
    );
    assert_eq!(payload["counts"]["walk_lines"], 2);
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
    assert_eq!(changed_paths(&payload), ["body.md", "eat.md", "eat/one.md"]);
    assert_eq!(payload["counts"]["walk_lines"], 1);
    assert_eq!(payload["albums"][0]["index"]["state"], "current");
    assert_eq!(payload["albums"][0]["walk"]["state"], "current");
    let repaired = fs::read_to_string(vault.join("eat.md")).unwrap();
    assert_eq!(repaired.matches("> [!records]").count(), 1, "{repaired}");
    assert_eq!(repaired.matches("## Records").count(), 1, "{repaired}");
    assert!(repaired.contains("> [!records] 1 restaurant"), "{repaired}");
}

#[test]
fn grouped_sync_orders_walk_lines_and_is_idempotent() {
    let temp = TempDir::new("album-sync-grouped");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    fs::create_dir_all(vault.join("zettel")).expect("zettel folder");
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(
        &vault,
        "zettel.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Notes.\nwalk_by: section\nrecord:\n  noun: note\n  fields:\n    section:\n      values: {intro: Intro, body: Body}\n    status:\n      values: {open: Open, done: Done}\n---\n\n# Zettel\n",
    );
    write(
        &vault,
        "zettel/zeta.md",
        "---\ntype: [[record]]\nparent: \"[[zettel]]\"\ntitle: Zeta\nsection: intro\nstatus: done\n---\n",
    );
    write(
        &vault,
        "zettel/alpha.md",
        "---\ntype: [[record]]\nparent: \"[[zettel]]\"\ntitle: Alpha\nsection: body\nstatus: open\n---\n",
    );
    write(
        &vault,
        "zettel/beta.md",
        "---\ntype: [[record]]\nparent: \"[[zettel]]\"\ntitle: Beta\nsection: body\nstatus: done\n---\n",
    );
    commit_all(&vault, "grouped walk fixture");

    let first = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(first.status.code(), Some(0), "{}", format_output(&first));
    let payload = json(&first);
    assert_eq!(payload["ok"], true);
    let album = &payload["albums"][0];
    assert_eq!(album["walk_by"], "section");
    assert_eq!(
        album["walk"]["order"],
        serde_json::json!(["zeta", "alpha", "beta"])
    );
    assert_eq!(album["walk"]["state"], "current");
    assert_eq!(album["walk"]["groups"][0]["value"], "intro");
    assert_eq!(album["walk"]["groups"][0]["head"]["stem"], "zeta");
    assert_eq!(album["walk"]["groups"][1]["value"], "body");
    assert_eq!(album["walk"]["groups"][1]["head"]["stem"], "alpha");
    assert_eq!(walk_line(&vault, "zettel/zeta.md"), "[[zettel/beta|‹ Beta]] · [[zettel|↑ Zettel]] · [[zettel/alpha|Alpha ›]]");
    assert_eq!(
        walk_line(&vault, "zettel/alpha.md"),
        "[[zettel/zeta|‹ Zeta]] · [[zettel|↑ Zettel]] · [[zettel/beta|Beta ›]]"
    );
    assert_eq!(walk_line(&vault, "zettel/beta.md"), "[[zettel/alpha|‹ Alpha]] · [[zettel|↑ Zettel]] · [[zettel/zeta|Zeta ›]]");

    let before = head(&vault);
    let second = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(second.status.code(), Some(0), "{}", format_output(&second));
    let again = json(&second);
    assert_eq!(again["ok"], true);
    assert!(changed_paths(&again).is_empty(), "{again}");
    assert_eq!(head(&vault), before);
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
    assert!(text.contains("step 7/10 · notebooks"), "{text}");
    assert!(text.contains("step 8/10 · albums"), "{text}");
    assert!(text.contains("step 9/10 · records"), "{text}");
    assert!(text.contains("step 10/10 · vault-sync"), "{text}");
    assert!(text.contains("albums failed"), "{text}");
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "origin/master"])).trim(),
        head(&vault),
        "nightly's final vault-sync still publishes the healthy album index"
    );
    let body = fs::read_to_string(vault.join("body.md")).unwrap();
    assert!(body.contains("> [!albums]"), "{body}");
    let record = fs::read_to_string(vault.join("eat/one.md")).unwrap();
    assert!(record.contains("[[eat|↑ Restaurants]]"), "{record}");
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
