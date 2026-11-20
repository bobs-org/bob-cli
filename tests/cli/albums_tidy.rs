//! `bob albums tidy`: dry-run-first title moves that change no derived output.

use crate::support::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const EAT: &str = "---\nparent: \"[[body]]\"\ntype: \"[[album]]\"\nsummary: Places.\nrecord:\n  noun: restaurant\n  fields:\n    status:\n      values: {to_try: To try, liked: Liked}\n---\n\n# Restaurants\n";
const MOVIES: &str = "---\nparent: \"[[body]]\"\ntype: \"[[album]]\"\nsummary: Films.\nrecord:\n  noun: movie\n---\n\nFilms worth watching.\n";
const RECORDS: &[(&str, &str, &str)] = &[
    ("eat/apple.md", "Apple", "to_try"),
    ("eat/mango.md", "Mango", "liked"),
    ("eat/pear.md", "Pear", "to_try"),
];

fn write(vault: &Path, path: &str, contents: &str) {
    write_file(&vault.join(path), contents);
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q"]);
}

/// An owner area, `eat.md` (H1 `# Restaurants`, not its stem) with three
/// records and a `status` choice field, and `movies.md` with no H1.
fn fixture(temp: &TempDir) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    fs::create_dir_all(vault.join("eat")).expect("eat folder");
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(&vault, "eat.md", EAT);
    write(&vault, "movies.md", MOVIES);
    for (path, title, status) in RECORDS {
        write(
            &vault,
            path,
            &format!(
                "---\ntype: \"[[record]]\"\nparent: \"[[eat]]\"\ntitle: {title}\nstatus: {status}\n---\n"
            ),
        );
    }
    commit_all(&vault, "album fixture");
    vault
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

fn head_paths(vault: &Path) -> String {
    stdout(&git_in(
        vault,
        ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    ))
    .trim()
    .to_owned()
}

fn read(vault: &Path, path: &str) -> String {
    fs::read_to_string(vault.join(path)).expect("read vault file")
}

fn change_paths(payload: &Value) -> Vec<String> {
    payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap().to_owned())
        .collect()
}

/// Album titles from `bob albums list -f json`, keyed by path.
fn listed_titles(vault: &Path, temp: &TempDir) -> Vec<(String, String)> {
    let output = run(vault, temp, &["list", "-f", "json"]);
    assert_success(&output);
    json(&output)["albums"]
        .as_array()
        .unwrap()
        .iter()
        .map(|album| {
            (
                album["path"].as_str().unwrap().to_owned(),
                album["title"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The album's generated `## Records` section through EOF.
fn records_region(album: &str) -> String {
    let start = album.find("## Records").expect("records region");
    album[start..].to_owned()
}

#[test]
fn bare_run_is_a_dry_run_with_diffs_and_counts() {
    let temp = TempDir::new("album-tidy-dry");
    let vault = fixture(&temp);
    let before_head = head(&vault);
    let before_eat = fs::read(vault.join("eat.md")).unwrap();
    let before_movies = fs::read(vault.join("movies.md")).unwrap();

    let human = run(&vault, &temp, &["tidy"]);
    assert_eq!(human.status.code(), Some(0), "{}", format_output(&human));
    let text = stdout(&human);
    assert!(text.contains("+title: Restaurants"), "{text}");
    assert!(text.contains("-# Restaurants"), "{text}");
    assert!(text.contains("+title: movies"), "{text}");
    assert!(
        text.contains(
            "! dry run: 2 albums would change (2 titles added; 1 redundant H1); nothing was written. Rerun with -a to apply"
        ),
        "{text}"
    );

    let output = run(&vault, &temp, &["tidy", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "tidy");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["counts"]["scanned"], 2);
    assert_eq!(payload["counts"]["changed"], 2);
    assert_eq!(payload["counts"]["title_added"], 2);
    assert_eq!(payload["counts"]["h1_removed"], 1);
    assert_eq!(payload["counts"]["h1_kept"], 0);
    assert_eq!(change_paths(&payload), ["eat.md", "movies.md"]);
    assert_eq!(payload["changes"][0]["title"], "Restaurants");
    assert_eq!(payload["changes"][0]["h1_removed"], true);
    assert!(payload["changes"][0]["h1_kept_reason"].is_null());
    assert_eq!(payload["changes"][1]["title"], "movies");
    assert_eq!(payload["changes"][1]["h1_removed"], false);
    assert!(payload["changes"][0]["diff"].is_string());
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"], Value::Null);

    assert_eq!(head(&vault), before_head);
    assert_eq!(fs::read(vault.join("eat.md")).unwrap(), before_eat);
    assert_eq!(fs::read(vault.join("movies.md")).unwrap(), before_movies);
}

#[test]
fn apply_commits_once_and_a_rerun_is_a_noop() {
    let temp = TempDir::new("album-tidy-apply");
    let vault = fixture(&temp);
    let before = head(&vault);

    let output = run(&vault, &temp, &["tidy", "-a", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["dry_run"], false);
    assert_eq!(payload["commit"]["subject"], "albums: tidy (2 notes)");
    assert_eq!(payload["sync"]["pre"], "ok");
    assert_eq!(payload["sync"]["post"], "ok");
    assert!(payload["changes"][0].get("diff").is_none());
    assert_ne!(head(&vault), before);
    assert_eq!(subject(&vault), "albums: tidy (2 notes)");
    assert_eq!(
        stdout(&git_in(
            &vault,
            ["rev-list", "--count", &format!("{before}..HEAD")]
        ))
        .trim(),
        "1"
    );
    assert_eq!(head_paths(&vault), "eat.md\nmovies.md");
    assert!(read(&vault, "eat.md").contains("\ntitle: Restaurants\n"));
    assert!(!read(&vault, "eat.md").contains("# Restaurants"));

    let after = head(&vault);
    let rerun = run(&vault, &temp, &["tidy", "-a", "-f", "json"]);
    assert_eq!(rerun.status.code(), Some(0), "{}", format_output(&rerun));
    let payload = json(&rerun);
    assert_eq!(payload["changes"], serde_json::json!([]));
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(payload["sync"]["post"], "skipped");
    assert_eq!(head(&vault), after);

    let human = run(&vault, &temp, &["tidy"]);
    assert_success(&human);
    assert!(
        stdout(&human).contains("✓ albums are tidy: 2 scanned, 0 tidied"),
        "{}",
        format_output(&human)
    );
}

#[test]
fn tidy_changes_no_derived_output() {
    let temp = TempDir::new("album-tidy-churn");
    let vault = fixture(&temp);

    // A real sync writes every index, walk line, and catalog first.
    let synced = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(synced.status.code(), Some(0), "{}", format_output(&synced));
    assert_eq!(json(&synced)["commit"]["subject"], "albums: sync (6 notes)");

    let records_before: Vec<Vec<u8>> = RECORDS
        .iter()
        .map(|(path, _, _)| fs::read(vault.join(path)).unwrap())
        .collect();
    let eat_before = read(&vault, "eat.md");
    let movies_before = read(&vault, "movies.md");
    let body_before = read(&vault, "body.md");
    for bytes in &records_before {
        let record = String::from_utf8_lossy(bytes);
        assert!(record.contains("[[eat|↑ Restaurants]]"), "{record}");
    }
    assert!(
        eat_before.contains("> [!records] 3 restaurants"),
        "{eat_before}"
    );
    assert!(body_before.contains("[[eat|Restaurants]]"), "{body_before}");
    assert!(body_before.contains("[[movies|movies]]"), "{body_before}");
    let titles_before = listed_titles(&vault, &temp);
    assert_eq!(
        titles_before,
        [
            ("eat.md".to_owned(), "Restaurants".to_owned()),
            ("movies.md".to_owned(), "movies".to_owned()),
        ]
    );

    let tidied = run(&vault, &temp, &["tidy", "-a", "-f", "json"]);
    assert_eq!(tidied.status.code(), Some(0), "{}", format_output(&tidied));
    assert_eq!(json(&tidied)["commit"]["subject"], "albums: tidy (2 notes)");
    let after_tidy = head(&vault);

    let resync = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(resync.status.code(), Some(0), "{}", format_output(&resync));
    let payload = json(&resync);
    assert_eq!(payload["changed"], serde_json::json!([]), "{payload}");
    assert_eq!(payload["changes"], serde_json::json!([]), "{payload}");
    assert_eq!(payload["commit"], Value::Null);
    assert_eq!(head(&vault), after_tidy);

    for ((path, _, _), before) in RECORDS.iter().zip(&records_before) {
        assert_eq!(&fs::read(vault.join(path)).unwrap(), before, "{path}");
    }
    let eat_after = read(&vault, "eat.md");
    assert_eq!(records_region(&eat_after), records_region(&eat_before));
    assert_eq!(read(&vault, "body.md"), body_before);
    assert_eq!(listed_titles(&vault, &temp), titles_before);

    // Only the inserted `title:` line and the removed H1 (plus its blank
    // line) differ.
    assert_eq!(
        eat_after,
        eat_before
            .replacen(
                "\ntype: \"[[album]]\"\n",
                "\ntype: \"[[album]]\"\ntitle: Restaurants\n",
                1
            )
            .replacen("---\n\n# Restaurants\n\n", "---\n\n", 1)
    );
    assert_eq!(
        read(&vault, "movies.md"),
        movies_before.replacen(
            "\ntype: \"[[album]]\"\n",
            "\ntype: \"[[album]]\"\ntitle: movies\n",
            1
        )
    );
}

#[test]
fn stem_selects_one_album_and_unknown_stems_exit_2() {
    let temp = TempDir::new("album-tidy-stem");
    let vault = fixture(&temp);
    let before = head(&vault);
    let movies_before = fs::read(vault.join("movies.md")).unwrap();

    let unknown =
        run(&vault, &temp, &["tidy", "restaurant", "-a", "-f", "json"]);
    assert_eq!(
        unknown.status.code(),
        Some(2),
        "{}",
        format_output(&unknown)
    );
    let payload = json(&unknown);
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["error"]["code"], "unknown_album");
    assert_eq!(
        payload["error"]["hint"],
        "use an album stem from `bob albums list`"
    );
    assert_eq!(head(&vault), before);

    let dry_unknown = run(&vault, &temp, &["tidy", "nope"]);
    assert_eq!(dry_unknown.status.code(), Some(2));
    assert!(stderr(&dry_unknown).contains("unknown album `nope`"));

    let output = run(&vault, &temp, &["tidy", "eat", "-a", "-f", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["counts"]["scanned"], 1);
    assert_eq!(payload["commit"]["subject"], "albums: tidy (1 notes)");
    assert_eq!(head_paths(&vault), "eat.md");
    assert_eq!(fs::read(vault.join("movies.md")).unwrap(), movies_before);
}
