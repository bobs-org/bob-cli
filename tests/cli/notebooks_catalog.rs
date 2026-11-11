//! Notebook catalogs, `sync`, `create`, section protection, and nightly.

use crate::support::*;
use fs2::FileExt;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const NOW: &str = "2026-11-09 09:14:00";
const BEGIN: &str = "<!-- bob:notebooks:begin v1 -->";
const END: &str = "<!-- bob:notebooks:end -->";

fn host_note(kind: &str, parent: &str, extra: &str) -> String {
    format!(
        "---\nparent: \"[[{parent}]]\"\ntype: \"[[{kind}]]\"\n{extra}---\n\n# Host\n\n## Tasks\n\n- [ ] #task Keep going ^keep\n"
    )
}

fn notebook_note(parent: &str, summary: &str, title: &str) -> String {
    format!(
        "---\nparent: \"[[{parent}]]\"\ntemplate: \"[[new_notebook]]\"\ntype: \"[[notebook]]\"\nsummary: {summary}\ncreated: 2026-11-09T09:14:00-05:00\n---\n\n# {title}\n\n- A note [created::2026-11-09]\n"
    )
}

/// dev (area) > proj (outermost project) > sub (non-root) > deep
/// (`root: true`), plus home (an area beneath dev).
fn write_tree(vault: &Path) {
    let files = [
        ("dev.md", host_note("area", "area", "")),
        ("home.md", host_note("area", "dev", "")),
        ("proj.md", host_note("project", "dev", "")),
        ("sub.md", host_note("project", "proj", "")),
        ("deep.md", host_note("project", "sub", "root: true\n")),
        (
            "dev_tools.md",
            notebook_note("dev", "Command-line tools.", "Developer tools"),
        ),
        ("git.md", notebook_note("dev_tools", "Git tips.", "Git")),
        ("zed.md", notebook_note("dev", "Zed notes.", "zed")),
        ("alpha.md", notebook_note("dev", "Sorts first.", "Alpha")),
        (
            "proj_log.md",
            notebook_note("proj", "Project log.", "Project log"),
        ),
        (
            "sub_notes.md",
            notebook_note("sub", "Sub notes.", "Sub notes"),
        ),
        (
            "deep_notes.md",
            notebook_note("deep", "Deep notes.", "Deep notes"),
        ),
        (
            "home_recipes.md",
            notebook_note("home", "Recipes.", "Recipes"),
        ),
    ];
    for (name, contents) in files {
        write_file(&vault.join(name), &contents);
    }
}

const HOSTS: [&str; 5] = ["deep.md", "dev.md", "home.md", "proj.md", "sub.md"];

fn plain_vault(temp: &TempDir) -> PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_tree(&vault);
    vault
}

/// A Git vault with a local remote and the fixture tree committed.
fn online_vault(temp: &TempDir) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    write_tree(&vault);
    commit_all(&vault, "notebook fixture");
    vault
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q"]);
}

fn nb(vault: &Path, temp: &TempDir, args: &[&str]) -> Command {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("create home");
    let mut command = bob_command();
    command
        .arg("notebooks")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
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
    nb(vault, temp, args).output().expect("run bob notebooks")
}

fn json(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!(
            "one JSON envelope expected ({error}):\n{}",
            format_output(output)
        )
    })
}

fn head(vault: &Path) -> String {
    stdout(&git_in(vault, ["rev-parse", "HEAD"]))
        .trim()
        .to_string()
}

fn subject(vault: &Path) -> String {
    stdout(&git_in(vault, ["log", "-1", "--format=%s"]))
        .trim()
        .to_string()
}

fn commits_since(vault: &Path, base: &str) -> usize {
    stdout(&git_in(
        vault,
        ["rev-list", "--count", &format!("{base}..HEAD")],
    ))
    .trim()
    .parse()
    .expect("count")
}

fn read(vault: &Path, name: &str) -> String {
    fs::read_to_string(vault.join(name)).expect("read note")
}

fn snapshot(vault: &Path) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = fs::read_dir(vault)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.path().is_file())
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read_to_string(entry.path()).unwrap_or_default(),
            )
        })
        .collect();
    files.sort();
    files
}

fn dev_with_catalog(rows: &str) -> String {
    format!(
        "---\nparent: \"[[area]]\"\ntype: \"[[area]]\"\n---\n\n# Host\n\n## Notebooks\n\n> [!notebooks]\n>\n{rows}\n\n## Tasks\n\n- [ ] #task Keep going ^keep\n"
    )
}

const DEV_ROWS: &str = "\
> - **[[alpha|Alpha]]** — Sorts first.
> - **[[dev_tools|Developer tools]]** — Command-line tools.
>   - **[[git|Git]]** — Git tips.
> - **[[zed|zed]]** — Zed notes.";

fn synced_vault(temp: &TempDir) -> PathBuf {
    let vault = online_vault(temp);
    let output = run(&vault, temp, &["sync"]);
    assert_success(&output);
    vault
}

// ----------------------------------------------------------- reporting

#[test]
fn list_and_check_report_catalog_states_and_filter_by_root() {
    let temp = TempDir::new("nb-catalog-report");
    let vault = plain_vault(&temp);

    let output = run(&vault, &temp, &["list", "-f", "json"]);
    assert_success(&output);
    assert!(!stdout(&output).contains('\u{1b}'));
    let payload = json(&output);
    assert_eq!(payload["schema_version"], 1);
    let catalogs = payload["catalogs"].as_array().unwrap();
    let summary: Vec<(String, String, u64)> = catalogs
        .iter()
        .map(|record| {
            (
                record["path"].as_str().unwrap().to_string(),
                record["state"].as_str().unwrap().to_string(),
                record["notebooks"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("deep.md".to_string(), "missing".to_string(), 1),
            ("dev.md".to_string(), "missing".to_string(), 4),
            ("home.md".to_string(), "missing".to_string(), 1),
            ("proj.md".to_string(), "missing".to_string(), 2),
            ("sub.md".to_string(), "missing".to_string(), 1),
        ]
    );
    assert_eq!(catalogs[1]["host"], "dev");
    assert_eq!(catalogs[1]["issues"][0]["code"], "notebook_catalog_stale");
    // Findings count once, as host warnings, never on a notebook.
    assert_eq!(payload["counts"]["warnings"], 5);
    assert_eq!(payload["counts"]["errors"], 0);
    assert!(payload["notebooks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["issues"].as_array().unwrap().is_empty()));

    // A root filter keeps that root's host catalogs and recounts.
    let output = run(&vault, &temp, &["list", "-r", "proj", "-f", "json"]);
    let payload = json(&output);
    let hosts: Vec<&str> = payload["catalogs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["path"].as_str().unwrap())
        .collect();
    assert_eq!(hosts, ["proj.md", "sub.md"]);
    assert_eq!(payload["counts"]["notebooks"], 2);
    assert_eq!(payload["counts"]["warnings"], 2);

    // `check` prints host findings under the host path and exits 0.
    let output = run(&vault, &temp, &["check"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("dev.md — catalog"), "{text}");
    assert!(text.contains("notebook_catalog_stale"), "{text}");
    assert!(text.contains("bob notebooks sync"), "{text}");
    assert!(text.contains("0 errors, 5 warnings"), "{text}");

    // Human list says what sync would do.
    let text = stdout(&run(&vault, &temp, &[]));
    assert!(text.contains("catalog dev.md — missing"), "{text}");
}

// ---------------------------------------------------------------- sync

#[test]
fn dry_run_prints_diffs_and_changes_nothing_even_under_the_lock() {
    let temp = TempDir::new("nb-sync-dry-run");
    let vault = online_vault(&temp);
    let head_before = head(&vault);
    let files_before = snapshot(&vault);
    // A held maintenance lock never blocks a read-only preview.
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(vault_sync_lock_file(&temp))
        .unwrap();
    lock.lock_exclusive().unwrap();

    let output = run(&vault, &temp, &["sync", "-d"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(text.contains("+++ b/dev.md"), "{text}");
    assert!(
        text.contains("+>   - **[[git|Git]]** — Git tips."),
        "{text}"
    );
    assert!(text.contains("dry run: 5 catalogs would change"), "{text}");

    let output = run(&vault, &temp, &["sync", "--dry-run", "-f", "json"]);
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["commit"], Value::Null);
    let changes = payload["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 5);
    assert!(changes[1]["diff"]
        .as_str()
        .unwrap()
        .contains("+++ b/dev.md"));

    drop(lock);
    assert_eq!(head(&vault), head_before);
    assert_eq!(snapshot(&vault), files_before);
    assert_eq!(stdout(&git_in(&vault, ["status", "--porcelain"])), "");
    assert_eq!(
        stdout(&git_in(&vault, ["diff", "--cached", "--name-only"])),
        ""
    );
}

#[test]
fn sync_makes_one_scoped_commit_then_a_second_run_changes_nothing() {
    let temp = TempDir::new("nb-sync-once");
    let vault = online_vault(&temp);
    let base = head(&vault);

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    assert!(!stdout(&output).contains('\u{1b}'));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["changed"], serde_json::json!(HOSTS));
    assert_eq!(
        payload["commit"]["subject"],
        "notebooks: sync catalogs (5 notes)"
    );
    assert_eq!(payload["sync"]["pre"], "ok");
    assert_eq!(payload["sync"]["post"], "ok");
    assert_eq!(commits_since(&vault, &base), 1, "one catalog commit");
    assert_eq!(subject(&vault), "notebooks: sync catalogs (5 notes)");
    assert_eq!(
        read(&vault, "dev.md"),
        dev_with_catalog(DEV_ROWS),
        "exact generated bytes"
    );
    let proj = read(&vault, "proj.md");
    assert!(
        proj.contains(
            "> - **[[proj_log|Project log]]** — Project log.\n> - 🧩 [[sub]]\n>   - **[[sub_notes|Sub notes]]** — Sub notes."
        ),
        "{proj}"
    );
    let changed = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=", "HEAD"],
    ));
    assert_eq!(changed.lines().collect::<Vec<_>>(), HOSTS);
    let remote_head = stdout(&git_in(&vault, ["rev-parse", "origin/master"]));
    assert_eq!(remote_head.trim(), head(&vault), "post-sync pushed it");

    // The second run is a byte-for-byte no-op with no commit.
    let files = snapshot(&vault);
    let synced_head = head(&vault);
    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["changed"], serde_json::json!([]));
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"]["post"], "skipped");
    assert_eq!(head(&vault), synced_head);
    assert_eq!(snapshot(&vault), files);
    let text = stdout(&run(&vault, &temp, &["sync"]));
    assert!(text.contains("notebook catalogs are current"), "{text}");
}

#[test]
fn later_syncs_rewrite_in_place_after_the_block_moves_and_on_reparent() {
    let temp = TempDir::new("nb-sync-moved");
    let vault = synced_vault(&temp);

    // Bryan moves the block below Tasks and adds prose; a title changes.
    let synced = read(&vault, "dev.md");
    let start = synced.find("## Notebooks").unwrap();
    let end = synced.find("## Tasks").unwrap();
    let block = synced[start..end].to_string();
    let moved = format!(
        "{}Closing prose.\n\n{block}",
        synced.replacen(&block, "", 1)
    );
    write_file(&vault.join("dev.md"), &moved);
    write_file(
        &vault.join("git.md"),
        &notebook_note("dev_tools", "Git tips.", "Git!"),
    );
    commit_all(&vault, "bryan edits");
    let base = head(&vault);

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    let after = read(&vault, "dev.md");
    let prefix = moved.split("> [!notebooks]").next().unwrap();
    assert!(
        after.starts_with(prefix),
        "bytes before the callout unchanged"
    );
    assert!(after.contains("**[[git|Git!]]** — Git tips."), "{after}");
    assert!(
        after.find("## Tasks").unwrap() < after.find("## Notebooks").unwrap(),
        "block stays where Bryan put it:\n{after}"
    );
    assert_eq!(commits_since(&vault, &base), 1);
    assert_eq!(subject(&vault), "notebooks: sync catalogs (1 notes)");

    // Reparenting updates the old and the new host in one commit.
    write_file(
        &vault.join("zed.md"),
        &notebook_note("home", "Zed notes.", "zed"),
    );
    commit_all(&vault, "reparent");
    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    assert_eq!(
        json(&output)["changed"],
        serde_json::json!(["dev.md", "home.md"])
    );
    assert!(!read(&vault, "dev.md").contains("[[zed|"));
    assert!(read(&vault, "home.md").contains("[[zed|zed]]"));
}

#[test]
fn an_emptied_host_loses_its_region_but_authored_prose_survives() {
    let temp = TempDir::new("nb-sync-delete");
    let vault = synced_vault(&temp);
    fs::remove_file(vault.join("home_recipes.md")).unwrap();
    fs::remove_file(vault.join("deep_notes.md")).unwrap();
    // `deep.md` keeps an authored paragraph under the heading.
    let deep = read(&vault, "deep.md").replace(
        "## Notebooks\n\n",
        "## Notebooks\n\nMy own words about notebooks.\n\n",
    );
    write_file(&vault.join("deep.md"), &deep);
    commit_all(&vault, "remove notebooks");

    assert_success(&run(&vault, &temp, &["sync"]));
    assert_eq!(read(&vault, "home.md"), host_note("area", "dev", ""));
    let deep = read(&vault, "deep.md");
    assert!(
        deep.contains("## Notebooks\n\nMy own words about notebooks."),
        "{deep}"
    );
    assert!(!deep.contains("bob:notebooks:"), "{deep}");
    assert!(!deep.contains("> [!notebooks]"), "{deep}");
    assert_eq!(subject(&vault), "notebooks: sync catalogs (2 notes)");
}

#[test]
fn legacy_marked_host_reports_stale_and_migrates_to_bare_callout() {
    let temp = TempDir::new("nb-sync-legacy");
    let vault = online_vault(&temp);
    // Seed one host with the old v1 marker pair around a current callout.
    let host = host_note("area", "area", "");
    let legacy = format!(
        "{host}## Notebooks\n\n{BEGIN}\n\n> [!notebooks]\n>\n> - old\n\n{END}\n"
    );
    write_file(&vault.join("dev.md"), &legacy);
    write_file(
        &vault.join("old_nb.md"),
        &notebook_note("dev", "Old.", "Old"),
    );
    commit_all(&vault, "legacy dev");
    let output = run(&vault, &temp, &["check", "-f", "json"]);
    let payload = json(&output);
    let dev = payload["catalogs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["path"] == "dev.md")
        .expect("dev catalog");
    assert_eq!(dev["state"], "stale");
    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    let synced = read(&vault, "dev.md");
    assert!(synced.contains("> [!notebooks]"), "{synced}");
    assert!(!synced.contains("bob:notebooks:"), "{synced}");
    let files = snapshot(&vault);
    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    assert_eq!(json(&output)["changed"], serde_json::json!([]));
    assert_eq!(snapshot(&vault), files, "second sync is a no-op");
}

#[test]
fn damaged_catalog_is_skipped_reported_and_exits_one() {
    let temp = TempDir::new("nb-sync-damaged");
    let vault = online_vault(&temp);
    let damaged = format!("{}\n{BEGIN}\n", host_note("area", "area", ""));
    write_file(&vault.join("dev.md"), &damaged);
    commit_all(&vault, "damage dev");
    let base = head(&vault);

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], false);
    let finding = payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["code"] == "notebook_catalog_damaged")
        .expect("damaged finding");
    assert_eq!(finding["path"], "dev.md");
    assert_eq!(finding["severity"], "error");
    assert_eq!(finding["line"], 12);
    assert_eq!(read(&vault, "dev.md"), damaged, "damaged host untouched");
    // Healthy hosts are still applied together.
    assert_eq!(
        payload["changed"],
        serde_json::json!(["deep.md", "home.md", "proj.md", "sub.md"])
    );
    assert_eq!(commits_since(&vault, &base), 1);
    assert_eq!(subject(&vault), "notebooks: sync catalogs (4 notes)");

    let output = run(&vault, &temp, &["check"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("dev.md — catalog"),
        "{}",
        stdout(&output)
    );
    let output = run(&vault, &temp, &["list", "-f", "json"]);
    assert_success(&output);
    assert_eq!(json(&output)["catalogs"][1]["state"], "damaged");
}

#[test]
fn marker_variants_are_each_damage_not_guesses() {
    let temp = TempDir::new("nb-sync-markers");
    let vault = plain_vault(&temp);
    for (damage, label) in [
        (format!("{END}\n{BEGIN}\n"), "reversed"),
        (format!("{BEGIN}\n{END}\n{BEGIN}\n{END}\n"), "duplicate"),
        (
            "<!-- bob:notebooks:begin v2 -->\n".to_string(),
            "unsupported",
        ),
        (format!("{END}\n"), "lone end"),
    ] {
        let body = format!("{}\n{damage}", host_note("area", "area", ""));
        write_file(&vault.join("dev.md"), &body);
        let output = run(&vault, &temp, &["list", "-f", "json"]);
        assert_success(&output);
        assert_eq!(json(&output)["catalogs"][1]["state"], "damaged", "{label}");
    }
    // Fenced examples of the markers are inert.
    let fenced =
        format!("{}\n```md\n{BEGIN}\n```\n", host_note("area", "area", ""));
    write_file(&vault.join("dev.md"), &fenced);
    let output = run(&vault, &temp, &["list", "-f", "json"]);
    assert_eq!(json(&output)["catalogs"][1]["state"], "missing");
}

#[test]
fn an_incomplete_scan_never_deletes_or_writes_catalogs() {
    let temp = TempDir::new("nb-sync-io");
    let vault = synced_vault(&temp);
    // Not UTF-8: the scan cannot read it, so the view is partial.
    fs::write(vault.join("junk.md"), [0xff, 0xfe, 0xfd]).unwrap();
    commit_all(&vault, "junk");
    fs::remove_file(vault.join("home_recipes.md")).unwrap();
    commit_all(&vault, "remove");
    let base = head(&vault);
    let files = snapshot(&vault);

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["error"]["code"], "io");
    assert_eq!(head(&vault), base);
    assert_eq!(
        snapshot(&vault)
            .into_iter()
            .filter(|(name, _)| name != "junk.md")
            .collect::<Vec<_>>(),
        files
            .into_iter()
            .filter(|(name, _)| name != "junk.md")
            .collect::<Vec<_>>()
    );
    let output = run(&vault, &temp, &["sync", "-d"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn lock_contention_fails_truthfully_without_writing() {
    let temp = TempDir::new("nb-sync-lock");
    let vault = online_vault(&temp);
    let base = head(&vault);
    let files = snapshot(&vault);
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(vault_sync_lock_file(&temp))
        .unwrap();
    lock.lock_exclusive().unwrap();

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["error"]["code"], "lock_busy");
    assert!(payload["error"]["hint"].as_str().unwrap().contains("retry"));
    assert!(
        stderr(&output).contains("waiting for another vault maintenance run")
    );
    drop(lock);
    assert_eq!(head(&vault), base);
    assert_eq!(snapshot(&vault), files);
}

#[test]
fn wrong_vault_refuses_before_locking_or_writing() {
    let temp = TempDir::new("nb-sync-wrong-vault");
    let vault = online_vault(&temp);
    let mut command = bob_command();
    command.args(["notebooks", "sync"]);
    assert_vault_override_refused(&mut command, &vault);
    let mut command = bob_command();
    command.args(["notebooks", "create", "dev", "x", "-s", "Summary."]);
    assert_vault_override_refused(&mut command, &vault);
}

#[cfg(unix)]
#[test]
fn commit_failure_rolls_the_catalogs_back() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new("nb-sync-commit-fail");
    let vault = online_vault(&temp);
    let hook = vault.join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let base = head(&vault);
    let files = snapshot(&vault);

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["error"]["code"], "verify_failed");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("rolled back"));
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(head(&vault), base);
    assert_eq!(snapshot(&vault), files);
    assert_eq!(
        stdout(&git_in(&vault, ["diff", "--cached", "--name-only"])),
        ""
    );
}

#[cfg(unix)]
#[test]
fn post_sync_failure_is_reported_apart_from_an_unapplied_run() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new("nb-sync-post-fail");
    let vault = online_vault(&temp);
    // Refuse only the push that would publish the catalog commit.
    let hook = vault.join(".git/hooks/pre-push");
    fs::write(
        &hook,
        "#!/bin/sh\ngit log -1 --format=%s | grep -q '^notebooks:' && exit 1\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let base = head(&vault);

    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["sync"]["post"], "failed");
    assert_eq!(
        payload["commit"]["subject"],
        "notebooks: sync catalogs (5 notes)"
    );
    let warning = payload["warnings"][0].as_str().unwrap();
    assert!(
        warning.contains("committed locally")
            && warning.contains("bob vault-sync"),
        "{warning}"
    );
    assert_eq!(commits_since(&vault, &base), 1, "the local commit stands");
}

// -------------------------------------------------------------- create

fn create_args<'a>(
    parent: &'a str,
    id: &'a str,
    summary: &'a str,
) -> Vec<&'a str> {
    vec!["create", parent, id, "-s", summary, "-f", "json"]
}

#[test]
fn create_under_an_area_writes_notebook_and_catalog_in_one_commit() {
    let temp = TempDir::new("nb-create-area");
    let vault = synced_vault(&temp);
    let base = head(&vault);

    let output = run(
        &vault,
        &temp,
        &create_args("dev", "nvim", "Neovim config notes."),
    );
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["notebook"]["path"], "dev_nvim.md");
    assert_eq!(payload["notebook"]["title"], "Nvim");
    assert_eq!(payload["notebook"]["parent"], "dev");
    assert_eq!(
        payload["commit"]["subject"],
        "notebooks: create dev_nvim.md"
    );
    assert_eq!(
        payload["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["dev_nvim.md", "dev.md"]
    );
    assert_eq!(
        read(&vault, "dev_nvim.md"),
        "---\nparent: \"[[dev]]\"\ntemplate: \"[[new_notebook]]\"\ntype: \"[[notebook]]\"\nsummary: Neovim config notes.\ncreated: 2026-11-09T09:14:00+0000\n---\n\n# Nvim\n"
    );
    assert!(read(&vault, "dev.md")
        .contains("> - **[[dev_nvim|Nvim]]** — Neovim config notes."));
    assert_eq!(commits_since(&vault, &base), 1);
    let changed = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=", "HEAD"],
    ));
    assert_eq!(
        changed.lines().collect::<Vec<_>>(),
        ["dev.md", "dev_nvim.md"]
    );

    // File and catalogs are consistent: nothing left to sync, no findings.
    let output = run(&vault, &temp, &["check", "-f", "json"]);
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["counts"]["warnings"], 0);
    assert_eq!(payload["counts"]["errors"], 0);
}

#[test]
fn create_under_projects_and_notebooks_with_default_and_custom_titles() {
    let temp = TempDir::new("nb-create-parents");
    let vault = online_vault(&temp);
    write_file(
        &vault.join("old.md"),
        &host_note("project", "dev", "status: done\n"),
    );
    commit_all(&vault, "closed project");

    let output = run(
        &vault,
        &temp,
        &create_args("old", "log", "A closed project's log."),
    );
    assert_success(&output);
    assert_eq!(json(&output)["notebook"]["path"], "old_log.md");
    assert!(read(&vault, "old.md").contains("[[old_log|Log]]"));

    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "dev_tools",
            "my-tool",
            "-s",
            "A nested one.",
            "-t",
            "My Tool",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["notebook"]["path"], "dev_tools_my_tool.md");
    assert_eq!(payload["notebook"]["title"], "My Tool");
    assert!(read(&vault, "dev_tools_my_tool.md")
        .contains("parent: \"[[dev_tools]]\""));
    let dev = read(&vault, "dev.md");
    assert!(
        dev.contains(">   - **[[dev_tools_my_tool|My Tool]]** — A nested one."),
        "{dev}"
    );
}

#[test]
fn create_summary_rules_quote_warn_and_require() {
    let temp = TempDir::new("nb-create-summary");
    let vault = online_vault(&temp);

    // A blank summary is a refusal; a missing -s is a usage error.
    let output = run(&vault, &temp, &create_args("dev", "a", "  "));
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json(&output)["error"]["code"], "notebook_summary_required");
    let output = run(&vault, &temp, &["create", "dev", "a"]);
    assert_eq!(output.status.code(), Some(2));
    let output = run(&vault, &temp, &create_args("dev", "a", "two\nlines"));
    assert_eq!(output.status.code(), Some(2));

    // YAML-sensitive summaries are quoted and round-trip as strings.
    let output = run(&vault, &temp, &create_args("dev", "yes-note", "yes"));
    assert_success(&output);
    let text = read(&vault, "dev_yes_note.md");
    assert!(text.contains("\nsummary: \"yes\"\n"), "{text}");
    let output =
        run(&vault, &temp, &create_args("dev", "colon", "Tips: a & b"));
    assert_success(&output);
    assert!(
        read(&vault, "dev_colon.md").contains("\nsummary: \"Tips: a & b\"\n")
    );

    // Over 140 characters only warns, in JSON and on the notebook itself.
    let long = "x".repeat(150);
    let output = run(&vault, &temp, &create_args("dev", "long", &long));
    assert_success(&output);
    let payload = json(&output);
    assert!(payload["warnings"][0]
        .as_str()
        .unwrap()
        .starts_with("notebook_summary_long"));
    let output = run(&vault, &temp, &["check", "-f", "json"]);
    let codes = json(&output)["notebooks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|entry| entry["issues"].as_array().unwrap().clone())
        .map(|issue| issue["code"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"notebook_summary_long".to_string()),
        "{codes:?}"
    );
}

#[test]
fn create_refusals_exit_two_and_leave_the_vault_alone() {
    let temp = TempDir::new("nb-create-refusals");
    let vault = online_vault(&temp);
    write_file(&vault.join("plain.md"), "# Plain\n");
    write_file(&vault.join("areas/twin.md"), &host_note("area", "area", ""));
    write_file(&vault.join("other/twin.md"), &host_note("area", "area", ""));
    write_file(
        &vault.join("loop_a.md"),
        &notebook_note("loop_b", "A.", "A"),
    );
    write_file(
        &vault.join("loop_b.md"),
        &notebook_note("loop_a", "B.", "B"),
    );
    write_file(
        &vault.join("Dev_Mixed.md"),
        &notebook_note("dev", "Mixed case.", "Mixed"),
    );
    commit_all(&vault, "refusal fixtures");
    let base = head(&vault);
    let files = snapshot(&vault);

    for (parent, id, code) in [
        ("missing", "x", "notebook_parent_missing"),
        ("plain", "x", "notebook_parent_unsupported"),
        ("twin", "x", "notebook_parent_ambiguous"),
        ("loop_a", "x", "notebook_parent_invalid"),
        ("dev", "bad id", "notebook_id_invalid"),
        ("dev", "mixed", "notebook_exists"),
    ] {
        let output = run(&vault, &temp, &create_args(parent, id, "A summary."));
        assert_eq!(
            output.status.code(),
            Some(2),
            "{parent}/{id}: {}",
            format_output(&output)
        );
        let payload = json(&output);
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["error"]["code"], code, "{parent}/{id}");
    }
    assert_eq!(head(&vault), base);
    assert_eq!(snapshot(&vault), files);
    assert!(!vault.join("dev_x.md").exists());
}

#[test]
fn damaged_affected_host_refuses_the_create_but_an_unrelated_one_does_not() {
    let temp = TempDir::new("nb-create-damaged");
    let vault = synced_vault(&temp);
    let home = format!("{}\n{BEGIN}\n", read(&vault, "home.md"));
    write_file(&vault.join("home.md"), &home);
    commit_all(&vault, "damage home");
    let base = head(&vault);

    let output = run(&vault, &temp, &create_args("home", "tea", "Tea notes."));
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["error"]["code"], "notebook_catalog_damaged");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("home.md"));
    assert!(
        !vault.join("home_tea.md").exists(),
        "no half-created notebook"
    );
    assert_eq!(head(&vault), base);

    let output =
        run(&vault, &temp, &create_args("dev", "nvim", "Neovim notes."));
    assert_success(&output);
    assert_eq!(
        read(&vault, "home.md"),
        home,
        "unrelated damage is never repaired"
    );
}

#[test]
fn create_dry_run_matches_the_real_run() {
    let temp = TempDir::new("nb-create-dry");
    let vault = synced_vault(&temp);
    let base = head(&vault);
    let files = snapshot(&vault);

    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "dev",
            "nvim",
            "-s",
            "Neovim config notes.",
            "-d",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    let dry = json(&output);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["commit"], Value::Null);
    let dry_paths: Vec<&str> = dry["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect();
    assert_eq!(dry_paths, ["dev_nvim.md", "dev.md"]);
    assert!(dry["changes"][0]["diff"]
        .as_str()
        .unwrap()
        .contains("--- /dev/null"));
    assert!(dry["changes"][1]["diff"]
        .as_str()
        .unwrap()
        .contains("+> - **[[dev_nvim|Nvim]]**"));
    assert_eq!(head(&vault), base);
    assert_eq!(snapshot(&vault), files, "dry run writes nothing");

    let human = run(
        &vault,
        &temp,
        &["create", "dev", "nvim", "-s", "Neovim config notes.", "-d"],
    );
    assert_success(&human);
    assert!(
        stdout(&human).contains("dry run: would create dev_nvim.md under dev")
    );

    let real = run(
        &vault,
        &temp,
        &create_args("dev", "nvim", "Neovim config notes."),
    );
    assert_success(&real);
    let real = json(&real);
    let real_paths: Vec<&str> = real["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect();
    assert_eq!(real_paths, dry_paths);
    assert_eq!(real["notebook"], dry["notebook"]);
}

#[cfg(unix)]
#[test]
fn open_runs_only_after_a_real_create_and_failing_to_open_is_a_warning() {
    let temp = TempDir::new("nb-create-open");
    let vault = synced_vault(&temp);
    let bin = temp.path().join("bin");
    let log = temp.path().join("opened.log");
    fs::create_dir_all(&bin).unwrap();
    let stub = format!(
        "#!/bin/sh\necho \"$1\" >> {}\n",
        shell_single_quote(path_str(&log))
    );
    write_executable(&bin.join("xdg-open"), &stub);
    write_executable(&bin.join("open"), &stub);

    // A dry run never opens.
    let mut command = nb(
        &vault,
        &temp,
        &["create", "dev", "nvim", "-s", "S.", "-d", "-o"],
    );
    command.env("PATH", path_with_prefix(&bin));
    assert_success(&command.output().unwrap());
    assert!(!log.exists());

    let mut command =
        nb(&vault, &temp, &["create", "dev", "nvim", "-s", "S.", "-o"]);
    command.env("PATH", path_with_prefix(&bin));
    assert_success(&command.output().unwrap());
    // The opener is spawned, never awaited: poll for the stub's write.
    let mut opened = String::new();
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&log)
            && text.contains('\n')
        {
            opened = text;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!opened.is_empty(), "the opener stub never ran");
    assert!(
        opened.contains("obsidian://open?vault=vault&file=dev_nvim.md"),
        "{opened}"
    );

    // With no opener on PATH the open fails: a warning, never a rollback.
    let git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let git = String::from_utf8_lossy(&git.stdout).trim().to_string();
    let only_git = temp.path().join("only-git");
    fs::create_dir_all(&only_git).unwrap();
    std::os::unix::fs::symlink(git, only_git.join("git")).unwrap();
    let mut command = nb(
        &vault,
        &temp,
        &["create", "dev", "lua", "-s", "S.", "-o", "-f", "json"],
    );
    command.env("PATH", &only_git);
    let output = command.output().unwrap();
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert!(payload["warnings"][0]
        .as_str()
        .unwrap()
        .contains("could not open"));
    assert!(vault.join("dev_lua.md").exists(), "committed file stays");
}

// ------------------------------------------- sections, projects, nightly

#[test]
fn capture_cannot_target_the_generated_catalog_section() {
    let temp = TempDir::new("nb-capture-protect");
    let vault = synced_vault(&temp);
    let before = read(&vault, "dev.md");
    for target in ["@dev#notebooks", "@dev#note", "@dev#Notebooks"] {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg(format!("an idea {target}"))
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", NOW)
            .env("NO_COLOR", "1");
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "{target}: {}",
            format_output(&output)
        );
        assert!(
            format_output(&output).contains("generated notebook catalog"),
            "{target}: {}",
            format_output(&output)
        );
        assert_eq!(read(&vault, "dev.md"), before, "{target} wrote nothing");
    }
}

#[test]
fn projects_sync_never_lists_a_notebook_as_a_sub_project() {
    let temp = TempDir::new("nb-projects-sync");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Parent.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Finish parent #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("Child.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: [[Parent]]\n---\n- [ ] #task Finish child #hide ^prj\n- [ ] #task Needs priority\n",
    );
    write_file(
        &vault.join("Parent_log.md"),
        &notebook_note("Parent", "A notebook, not a project.", "Log"),
    );
    let output = bob_command()
        .args(["projects", "sync", "--bob-dir"])
        .arg(&vault)
        .output()
        .expect("projects sync");
    assert_success(&output);
    let parent = read(&vault, "Parent.md");
    assert!(parent.contains("[[Child]]"), "{parent}");
    assert!(!parent.contains("Parent_log"), "{parent}");
}

fn nightly(vault: &Path, temp: &TempDir) -> Output {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).unwrap();
    bob_command()
        .arg("nightly")
        .env("BOB_DIR", vault)
        .env("BOB_NOW", "2026-06-02")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob nightly")
}

#[test]
fn nightly_has_seven_ordered_steps_and_syncs_catalogs_once() {
    let temp = TempDir::new("nb-nightly");
    let vault = online_vault(&temp);
    let base = head(&vault);

    let output = nightly(&vault, &temp);
    assert_success(&output);
    let out = stdout(&output);
    assert_text_order(
        &out,
        &[
            "step 1/7 · vault-sync",
            "step 2/7 · task archive",
            "step 3/7 · articles",
            "step 4/7 · period notes",
            "step 5/7 · notebooks — Repair generated notebook catalogs",
            "step 6/7 · albums — Repair generated records indexes",
            "step 7/7 · vault-sync",
            "All steps passed",
        ],
    );
    assert!(out.contains("synced 5 catalogs"), "{out}");
    let subjects = stdout(&git_in(
        &vault,
        ["log", &format!("{base}..HEAD"), "--format=%s"],
    ));
    assert_eq!(
        subjects
            .lines()
            .filter(|line| line.starts_with("notebooks: sync catalogs"))
            .collect::<Vec<_>>(),
        ["notebooks: sync catalogs (5 notes)"],
        "{subjects}"
    );
    assert!(
        !subjects
            .lines()
            .any(|line| line.starts_with("vault(") && line.contains("dev.md")),
        "vault-sync must not mislabel the catalog commit:\n{subjects}"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "origin/master"])).trim(),
        head(&vault)
    );

    // A current vault: the step changes and commits nothing.
    let synced = head(&vault);
    let output = nightly(&vault, &temp);
    assert_success(&output);
    assert!(stdout(&output).contains("notebook catalogs are current"));
    assert_eq!(head(&vault), synced);
}

#[test]
fn nightly_still_publishes_after_a_notebooks_step_failure() {
    let temp = TempDir::new("nb-nightly-fail");
    let vault = online_vault(&temp);
    write_file(
        &vault.join("dev.md"),
        &format!("{}\n{BEGIN}\n", host_note("area", "area", "")),
    );
    commit_all(&vault, "damage dev");

    let output = nightly(&vault, &temp);
    assert!(!output.status.success(), "{}", format_output(&output));
    let out = stdout(&output);
    assert!(out.contains("\u{2717} notebooks"), "{out}");
    assert!(
        out.contains("step 7/7") && out.contains("1 step failed"),
        "{out}"
    );
    assert_eq!(
        subject_of_notebooks(&vault),
        "notebooks: sync catalogs (4 notes)"
    );
    assert_eq!(
        stdout(&git_in(&vault, ["rev-parse", "origin/master"])).trim(),
        head(&vault),
        "the final vault-sync still published"
    );
}

fn subject_of_notebooks(vault: &Path) -> String {
    stdout(&git_in(
        vault,
        ["log", "--format=%s", "--grep=^notebooks:", "-1"],
    ))
    .trim()
    .to_string()
}
