//! `bob albums create`: guarded creation, refusals, and coverage.

use crate::support::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const NOW: &str = "2026-11-09 09:14:00";

const EXPECTED_ALBUM: &str = "---\nparent: \"[[body]]\"\ntype: \"[[album]]\"\nsummary: Places to eat.\ncreated: 2026-11-09T09:14:00+0000\nrecord:\n  noun: restaurant\n  label: Restaurant\n---\n\n# Eat\n\nTrack restaurants with `Name @eat/`; browse every record in [[eat.base]].\n\n## Records\n\n> [!records] No restaurants yet\n>\n> Add one with `Name @eat/`.\n";

const EXPECTED_BASE: &str = "filters:\n  and:\n    - file.inFolder(\"eat\")\n    - note.type == link(\"record\")\nformulas:\n  name: if(title, file.asLink(title), file.asLink())\nproperties:\n  formula.name:\n    displayName: Restaurant\nviews:\n  - type: table\n    name: \"All Restaurants\"\n    groupBy:\n      property: status\n      direction: ASC\n    order:\n      - formula.name\n      - status\n    sort:\n      - property: formula.name\n        direction: ASC\n    summaries:\n      formula.name: Count\n";

// Areas and nested projects, committed. Refusal tests layer collisions
// on top of this vault.
fn clean_vault(temp: &TempDir) -> PathBuf {
    let (vault, _remote, _peer) = init_vault_sync_pair(temp);
    write(&vault, "body.md", "---\ntype: [[area]]\n---\n\n# Body\n");
    write(&vault, "mind.md", "---\ntype: [[area]]\n---\n\n# Mind\n");
    write(
        &vault,
        "proj.md",
        "---\ntype: [[project]]\nparent: \"[[body]]\"\n---\n\n# Project\n",
    );
    write(
        &vault,
        "sub.md",
        "---\ntype: [[project]]\nparent: \"[[proj]]\"\n---\n\n# Sub\n",
    );
    commit_all(&vault, "create fixture");
    vault
}

fn fixture(temp: &TempDir) -> PathBuf {
    let vault = clean_vault(temp);
    write(&vault, "plain.md", "# Plain\n");
    write(
        &vault,
        "areas/twin.md",
        "---\ntype: [[area]]\n---\n\n# Twin\n",
    );
    write(
        &vault,
        "other/twin.md",
        "---\ntype: [[area]]\n---\n\n# Twin\n",
    );
    write(
        &vault,
        "cyc_a.md",
        "---\ntype: [[project]]\nparent: \"[[cyc_b]]\"\n---\n\n# Cyc A\n",
    );
    write(
        &vault,
        "cyc_b.md",
        "---\ntype: [[project]]\nparent: \"[[cyc_a]]\"\n---\n\n# Cyc B\n",
    );
    write(&vault, "taken.md", "# Taken\n");
    write(&vault, "Mixed.md", "# Mixed\n");
    write(&vault, "bbase.base", "filters: {}\n");
    write(&vault, "UPPER.base", "filters: {}\n");
    fs::create_dir_all(vault.join("exactdir.md")).expect("dir destination");
    fs::create_dir_all(vault.join("CASEBASE.base")).expect("base dir");
    #[cfg(unix)]
    std::os::unix::fs::symlink("missing-target", vault.join("DANGLE.md"))
        .expect("dangling symlink");
    write(
        &vault,
        "eat.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Places.\nrecord:\n  noun: restaurant\n---\n\n# Eat\n",
    );
    write(
        &vault,
        "broken.md",
        "---\ntype: [[album]]\nparent: \"[[body]]\"\nsummary: Broken.\nrecord:\n  fields: {}\n---\n\n# Broken\n\n## Keep this text\n",
    );
    fs::create_dir_all(vault.join("full")).expect("record folder");
    write(&vault, "full/note.md", "# Occupant\n");
    commit_all(&vault, "create fixture");
    vault
}

fn write(vault: &Path, path: &str, contents: &str) {
    write_file(&vault.join(path), contents);
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q"]);
}

fn run(vault: &Path, temp: &TempDir, args: &[&str]) -> Output {
    let home = temp.path().join("home-dir");
    fs::create_dir_all(&home).expect("create home");
    let mut command = bob_command();
    command
        .arg("albums")
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
    command.output().expect("run bob albums")
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

fn read(vault: &Path, name: &str) -> String {
    fs::read_to_string(vault.join(name))
        .unwrap_or_else(|error| panic!("read {name}: {error}"))
}

fn snapshot(vault: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut children: Vec<PathBuf> = fs::read_dir(&directory)
            .unwrap_or_else(|error| {
                panic!("read {}: {error}", directory.display())
            })
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        children.sort();
        for child in children {
            if child
                .file_name()
                .is_some_and(|name| name == ".git" || name == ".bob")
            {
                continue;
            }
            let metadata =
                fs::symlink_metadata(&child).unwrap_or_else(|error| {
                    panic!("stat {}: {error}", child.display())
                });
            if metadata.is_dir() {
                stack.push(child);
            } else {
                let relative = child
                    .strip_prefix(vault)
                    .expect("vault-relative")
                    .to_path_buf();
                let bytes = if metadata.file_type().is_symlink() {
                    fs::read_link(&child)
                        .expect("read link")
                        .to_string_lossy()
                        .into_owned()
                        .into_bytes()
                } else {
                    fs::read(&child).expect("read file")
                };
                entries.push((relative, bytes));
            }
        }
    }
    entries.sort();
    entries
}

// ---------------------------------------------------------- success paths

#[test]
fn create_writes_album_base_and_catalog_in_one_commit() {
    let temp = TempDir::new("album-create-minimal");
    let vault = clean_vault(&temp);
    let base = head(&vault);

    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "eat",
            "-n",
            "restaurant",
            "-s",
            "Places to eat.",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["command"], "create");
    assert_eq!(payload["dry_run"], false);
    assert_eq!(
        payload["album"],
        serde_json::json!({
            "path": "eat.md",
            "stem": "eat",
            "title": "Eat",
            "summary": "Places to eat.",
            "parent": "body",
            "noun": "restaurant",
            "label": "Restaurant",
        })
    );
    assert_eq!(
        payload["changed"],
        serde_json::json!(["eat.md", "eat.base", "body.md"])
    );
    let changes: Vec<&str> = payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect();
    assert_eq!(changes, ["eat.md", "eat.base", "body.md"]);
    assert_eq!(payload["changes"][0]["action"], "create", "{payload}");
    assert_eq!(payload["changes"][2]["action"], "update");
    assert!(payload["changes"][0].get("diff").is_none(), "{payload}");
    assert_eq!(payload["commit"]["subject"], "albums: create eat.md");
    assert_eq!(payload["sync"]["pre"], "ok");
    assert_eq!(payload["sync"]["post"], "ok");
    assert_eq!(payload["warnings"], serde_json::json!([]));
    assert_eq!(payload["catalogs"][0]["path"], "body.md");
    assert_eq!(payload["catalogs"][0]["root"], "body");
    assert_eq!(payload["catalogs"][0]["albums"], 1);

    assert_eq!(read(&vault, "eat.md"), EXPECTED_ALBUM);
    assert_eq!(read(&vault, "eat.base"), EXPECTED_BASE);
    assert!(
        !vault.join("eat").exists(),
        "create must not invent the empty record folder"
    );
    let body = read(&vault, "body.md");
    assert!(
        body.contains("> - **[[eat|Eat]]** — Places to eat."),
        "{body}"
    );
    let log = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=", "HEAD"],
    ));
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["body.md", "eat.base", "eat.md"]
    );

    // The new album is clean: check passes and a following sync is a no-op.
    let output = run(&vault, &temp, &["check", "-f", "json"]);
    assert_success(&output);
    assert_eq!(json(&output)["counts"]["errors"], 0);
    let sync_base = head(&vault);
    let output = run(&vault, &temp, &["sync", "-f", "json"]);
    assert_success(&output);
    assert_eq!(json(&output)["ok"], true);
    assert_eq!(head(&vault), sync_base, "sync after create is a no-op");
    assert_ne!(head(&vault), base, "create commits once");
}

#[test]
fn create_optional_fields_quote_warn_and_shape_the_base() {
    let temp = TempDir::new("album-create-optional");
    let vault = clean_vault(&temp);

    let output = run(
        &vault,
        &temp,
        &[
            "create", "fun", "movies", "-n", "movie", "-s", "yes", "-t",
            "Movies", "-l", "Diner", "-e", "🎬", "-f", "json",
        ],
    );
    // `fun` is not an area or project yet.
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json(&output)["error"]["code"], "album_parent_missing");

    write(&vault, "fun.md", "---\ntype: [[area]]\n---\n\n# Fun\n");
    commit_all(&vault, "add fun");
    let output = run(
        &vault,
        &temp,
        &[
            "create", "fun", "movies", "-n", "movie", "-s", "yes", "-t",
            "Movies", "-l", "Diner", "-e", "🎬", "-f", "json",
        ],
    );
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["album"]["title"], "Movies");
    assert_eq!(payload["album"]["label"], "Diner");
    // A YAML-sensitive summary is quoted and round-trips as a string.
    let album = read(&vault, "movies.md");
    assert!(album.contains("\nsummary: \"yes\"\n"), "{album}");
    assert!(album.contains("\n  label: Diner\n"), "{album}");
    assert!(album.contains("\nemoji: 🎬\n"), "{album}");
    assert!(album.contains("# Movies\n"), "{album}");
    assert!(
        album.contains("Track diners with `Name @movies/`"),
        "{album}"
    );
    let base = read(&vault, "movies.base");
    assert!(base.contains("file.inFolder(\"movies\")"), "{base}");
    assert!(base.contains("note.type == link(\"record\")"), "{base}");
    assert!(base.contains("displayName: Diner"), "{base}");
    assert!(base.contains("All Diners"), "{base}");

    // Over 140 characters only warns, in JSON and on the album itself.
    let long = "x".repeat(150);
    let output = run(
        &vault,
        &temp,
        &[
            "create", "fun", "shorts", "-n", "short", "-s", &long, "-f", "json",
        ],
    );
    assert_success(&output);
    let payload = json(&output);
    assert!(payload["warnings"][0]
        .as_str()
        .unwrap()
        .starts_with("album_summary_long"));
    let output = run(&vault, &temp, &["check", "-f", "json"]);
    let codes = json(&output)["albums"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|entry| entry["issues"].as_array().unwrap().clone())
        .map(|issue| issue["code"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        codes.contains(&"album_summary_long".to_string()),
        "{codes:?}"
    );
}

#[test]
fn create_renders_literal_markdown_metacharacters() {
    let temp = TempDir::new("album-create-meta");
    let vault = clean_vault(&temp);

    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "eat",
            "-n",
            "restaurant",
            "-s",
            "Read [[raw]] & stuff: a *real* guide",
            "-t",
            "Eat & Drink",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    let album = read(&vault, "eat.md");
    assert!(
        album.contains("summary: \"Read [[raw]] & stuff: a *real* guide\"\n"),
        "{album}"
    );
    assert!(album.contains("# Eat & Drink\n"), "{album}");
    let body = read(&vault, "body.md");
    assert!(
        body.contains("[[eat|Eat & Drink]]"),
        "catalog rows keep literal text:\n{body}"
    );
    assert!(body.contains("\\[\\[raw\\]\\]"), "{body}");
    assert!(body.contains("\\*real\\*"), "{body}");
    let listed = run(&vault, &temp, &["list", "-f", "json"]);
    assert_success(&listed);
    let payload = json(&listed);
    let entry = payload["albums"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["stem"] == "eat")
        .expect("eat listed");
    assert_eq!(entry["title"], "Eat & Drink");
    assert_eq!(entry["summary"], "Read [[raw]] & stuff: a *real* guide");
}

#[test]
fn create_dry_run_matches_the_real_run() {
    let temp = TempDir::new("album-create-dry");
    let vault = clean_vault(&temp);
    let base = head(&vault);
    let files = snapshot(&vault);
    let args = [
        "create",
        "body",
        "eat",
        "-n",
        "restaurant",
        "-s",
        "Places to eat.",
    ];

    let dry = run(
        &vault,
        &temp,
        &[args.as_slice(), &["-d", "-f", "json"]].concat(),
    );
    assert_success(&dry);
    let dry_payload = json(&dry);
    assert_eq!(dry_payload["dry_run"], true);
    assert_eq!(dry_payload["commit"], Value::Null);
    assert_eq!(
        dry_payload["changed"],
        serde_json::json!(["eat.md", "eat.base", "body.md"])
    );
    let dry_diffs: Vec<(&str, &str)> = dry_payload["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| {
            (
                change["path"].as_str().unwrap(),
                change["diff"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        dry_diffs.iter().map(|(path, _)| *path).collect::<Vec<_>>(),
        ["eat.md", "eat.base", "body.md"]
    );
    assert!(
        dry_diffs[0].1.contains("--- /dev/null"),
        "{}",
        dry_diffs[0].1
    );
    assert!(
        dry_diffs[0].1.contains("+parent: \"[[body]]\""),
        "{}",
        dry_diffs[0].1
    );
    assert!(
        dry_diffs[2].1.contains("+> - **[[eat|Eat]]**"),
        "{}",
        dry_diffs[2].1
    );
    assert!(!dry_payload["catalogs"].as_array().unwrap().is_empty());
    assert_eq!(head(&vault), base);
    assert_eq!(snapshot(&vault), files, "dry run writes nothing");

    let real = run(&vault, &temp, &[args.as_slice(), &["-f", "json"]].concat());
    assert_success(&real);
    let real_payload = json(&real);
    assert_eq!(real_payload["dry_run"], false);
    assert_eq!(real_payload["album"], dry_payload["album"]);
    assert_eq!(real_payload["changed"], dry_payload["changed"]);
    assert_eq!(real_payload["catalogs"], dry_payload["catalogs"]);
    assert_eq!(read(&vault, "eat.md"), EXPECTED_ALBUM);
    assert_ne!(head(&vault), base, "real run commits");

    let human = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "late",
            "-n",
            "late_noun",
            "-s",
            "Late.",
            "-d",
        ],
    );
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("dry run: would create late.md"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");

    let human = run(
        &vault,
        &temp,
        &["create", "body", "late", "-n", "late_noun", "-s", "Late."],
    );
    assert_success(&human);
    let text = stdout(&human);
    assert!(text.contains("created late.md in body"), "{text}");
    assert!(text.contains("created late.base"), "{text}");
    assert!(text.contains("updated body.md"), "{text}");
    assert!(text.contains("noun: late_noun"), "{text}");
    assert!(text.contains("fields:"), "{text}");
    assert!(text.contains("bob albums check"), "{text}");
}

// --------------------------------------------------------------- refusals

fn refusal_args(
    stem: &str,
    noun: &str,
    parent: &str,
    summary: &str,
    extra: &[&str],
) -> Vec<String> {
    let mut args = vec![
        "create".to_string(),
        parent.to_string(),
        stem.to_string(),
        "-n".to_string(),
        noun.to_string(),
        "-s".to_string(),
        summary.to_string(),
        "-f".to_string(),
        "json".to_string(),
    ];
    args.extend(extra.iter().map(|arg| arg.to_string()));
    args
}

fn run_owned(vault: &Path, temp: &TempDir, args: &[String]) -> Output {
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    run(vault, temp, &borrowed)
}

#[test]
fn create_refusals_exit_two_and_leave_the_vault_alone() {
    let temp = TempDir::new("album-create-refusals");
    let vault = fixture(&temp);

    let cases: Vec<(Vec<String>, &str)> = vec![
        (
            refusal_args("bad stem", "ok", "body", "A summary.", &[]),
            "album_stem_invalid",
        ),
        (
            refusal_args("done", "ok", "body", "A summary.", &[]),
            "album_stem_invalid",
        ),
        (
            refusal_args("ok1", "", "body", "A summary.", &[]),
            "album_noun_invalid",
        ),
        (
            refusal_args("ok2", "Bad", "body", "A summary.", &[]),
            "album_noun_invalid",
        ),
        (
            refusal_args("ok3", "album", "body", "A summary.", &[]),
            "album_noun_reserved",
        ),
        (
            refusal_args("ok4", "record", "body", "A summary.", &[]),
            "album_noun_reserved",
        ),
        (
            refusal_args("ok5", "ok", "body", "  ", &[]),
            "album_summary_required",
        ),
        (
            refusal_args("ok15", "ok", "body", "two\nlines", &[]),
            "album_summary_multiline",
        ),
        (
            refusal_args("ok6", "ok", "body", "A summary.", &["-t", " "]),
            "album_title_invalid",
        ),
        (
            refusal_args("ok7", "ok", "body", "A summary.", &["-l", "a\nb"]),
            "album_label_invalid",
        ),
        (
            refusal_args("ok8", "ok", "body", "A summary.", &["-e", "a\nb"]),
            "album_emoji_invalid",
        ),
        (
            refusal_args("ok9", "ok", "missing", "A summary.", &[]),
            "album_parent_missing",
        ),
        (
            refusal_args("ok10", "ok", "twin", "A summary.", &[]),
            "album_parent_ambiguous",
        ),
        (
            refusal_args("ok11", "ok", "plain", "A summary.", &[]),
            "album_parent_unsupported",
        ),
        (
            refusal_args("ok12", "ok", "cyc_a", "A summary.", &[]),
            "album_parent_invalid",
        ),
        (
            refusal_args("taken", "ok", "body", "A summary.", &[]),
            "album_exists",
        ),
        (
            refusal_args("bbase", "ok", "body", "A summary.", &[]),
            "album_exists",
        ),
        (
            refusal_args("mixed", "ok", "body", "A summary.", &[]),
            "album_exists",
        ),
        (
            refusal_args("exactdir", "ok", "body", "A summary.", &[]),
            "album_exists",
        ),
        (
            refusal_args("upper", "ok", "body", "A summary.", &[]),
            "album_exists",
        ),
        (
            refusal_args("casebase", "ok", "body", "A summary.", &[]),
            "album_exists",
        ),
        #[cfg(unix)]
        (
            refusal_args("dangle", "ok", "body", "A summary.", &[]),
            "album_exists",
        ),
        (
            refusal_args("restaurant", "ok", "body", "A summary.", &[]),
            "album_noun_duplicate",
        ),
        (
            refusal_args("ok13", "eat", "body", "A summary.", &[]),
            "album_noun_duplicate",
        ),
        (
            refusal_args("ok14", "broken", "body", "A summary.", &[]),
            "album_noun_duplicate",
        ),
        (
            refusal_args("full", "ok", "body", "A summary.", &[]),
            "album_folder_exists",
        ),
    ];

    for (args, code) in &cases {
        let base = head(&vault);
        let files = snapshot(&vault);
        let output = run_owned(&vault, &temp, args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{code}: {}",
            format_output(&output)
        );
        let payload = json(&output);
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["error"]["code"], *code, "{args:?}");
        assert_eq!(head(&vault), base, "{code} commits nothing");
        assert_eq!(snapshot(&vault), files, "{code} writes nothing");
    }

    // Missing flags are usage errors, still exit 2.
    let output = run(&vault, &temp, &["create", "body", "nos", "-s", "S."]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let output =
        run(&vault, &temp, &["create", "body", "nos", "-n", "nos_noun"]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}

#[test]
fn create_normalizes_mixed_case_stems_and_takes_path_qualified_parents() {
    let temp = TempDir::new("album-create-case");
    let vault = clean_vault(&temp);

    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "Eat",
            "-n",
            "restaurant",
            "-s",
            "Places.",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["album"]["stem"], "eat");
    assert_eq!(payload["album"]["path"], "eat.md");
    assert!(vault.join("eat.md").exists());
    assert!(!vault.join("Eat.md").exists());

    // Ambiguous stems need the path-qualified parent.
    write(
        &vault,
        "areas/twin.md",
        "---\ntype: [[area]]\n---\n\n# Twin\n",
    );
    write(
        &vault,
        "other/twin.md",
        "---\ntype: [[area]]\n---\n\n# Twin\n",
    );
    commit_all(&vault, "twins");
    let output = run(
        &vault,
        &temp,
        &[
            "create", "twin", "tea", "-n", "tea", "-s", "Teas.", "-f", "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json(&output)["error"]["code"], "album_parent_ambiguous");
    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "areas/twin",
            "tea",
            "-n",
            "tea",
            "-s",
            "Teas.",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    assert_eq!(json(&output)["album"]["parent"], "twin");
    assert!(read(&vault, "tea.md").contains("parent: \"[[areas/twin]]\""));
}

#[test]
fn create_under_a_nested_project_updates_every_owner_host() {
    let temp = TempDir::new("album-create-nested");
    let vault = clean_vault(&temp);

    let output = run(
        &vault,
        &temp,
        &[
            "create", "sub", "movies", "-n", "movie", "-s", "Films.", "-f",
            "json",
        ],
    );
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["album"]["parent"], "sub");
    // Rows stop at the project root: the area above proj gains nothing.
    assert_eq!(
        payload["changed"],
        serde_json::json!(["movies.md", "movies.base", "sub.md", "proj.md"])
    );
    assert_eq!(payload["commit"]["subject"], "albums: create movies.md");
    let hosts: Vec<&str> = payload["catalogs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["path"].as_str().unwrap())
        .collect();
    assert_eq!(hosts, ["sub.md", "proj.md"]);
    let sub = read(&vault, "sub.md");
    assert!(sub.contains("> - **[[movies|Movies]]** — Films."), "{sub}");
    let proj = read(&vault, "proj.md");
    assert!(proj.contains("> - 🧩 [[sub]]"), "{proj}");
    assert!(
        !read(&vault, "body.md").contains("## Albums"),
        "rows stop at the project root"
    );
    let log = stdout(&git_in(
        &vault,
        ["show", "--name-only", "--format=", "HEAD"],
    ));
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["movies.base", "movies.md", "proj.md", "sub.md"]
    );
}

#[test]
fn create_replans_when_the_destination_appears_between_commands() {
    let temp = TempDir::new("album-create-concurrent");
    let vault = clean_vault(&temp);
    let files = snapshot(&vault);

    let dry = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "eat",
            "-n",
            "restaurant",
            "-s",
            "Places.",
            "-d",
            "-f",
            "json",
        ],
    );
    assert_success(&dry);
    assert_eq!(json(&dry)["ok"], true);
    assert_eq!(snapshot(&vault), files, "dry run writes nothing");

    // Another writer claims the destination before a separately planned
    // real run; this is replan coverage, not same-plan race proof.
    write(&vault, "eat.md", "# Racing writer\n");
    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "eat",
            "-n",
            "restaurant",
            "-s",
            "Places.",
            "-f",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert_eq!(json(&output)["error"]["code"], "album_exists");
    assert_eq!(
        read(&vault, "eat.md"),
        "# Racing writer\n",
        "refusal does not replace the planted note"
    );

    // A later command on a still-unclaimed noun replans and succeeds.
    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "meals",
            "-n",
            "restaurant",
            "-s",
            "Places.",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    assert_eq!(json(&output)["ok"], true);
}

#[test]
fn create_dry_run_refuses_mixed_case_non_file_destinations() {
    let temp = TempDir::new("album-create-namespace");
    let vault = fixture(&temp);
    for stem in ["upper", "casebase"] {
        let files = snapshot(&vault);
        let output = run(
            &vault,
            &temp,
            &[
                "create",
                "body",
                stem,
                "-n",
                "ok",
                "-s",
                "A summary.",
                "-d",
                "-f",
                "json",
            ],
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "{stem}: {}",
            format_output(&output)
        );
        assert_eq!(json(&output)["error"]["code"], "album_exists");
        assert_eq!(snapshot(&vault), files, "{stem} dry-run writes nothing");
    }
}

#[test]
fn create_damaged_catalog_refuses_the_whole_creation() {
    let temp = TempDir::new("album-create-damaged");
    let vault = clean_vault(&temp);
    write(
        &vault,
        "body.md",
        "---\ntype: [[area]]\n---\n\n# Body\n\n## Albums\n\n> [!albums\n> - partial\n",
    );
    commit_all(&vault, "damage body");
    let base = head(&vault);
    let files = snapshot(&vault);

    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "eat",
            "-n",
            "restaurant",
            "-s",
            "Places.",
            "-f",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let payload = json(&output);
    assert_eq!(payload["error"]["code"], "album_catalog_damaged");
    assert!(payload["error"]["message"]
        .as_str()
        .unwrap()
        .contains("body.md"));
    assert!(!vault.join("eat.md").exists(), "no half-created album");
    assert!(!vault.join("eat.base").exists(), "no half-created Base");
    assert_eq!(head(&vault), base);
    assert_eq!(snapshot(&vault), files);
}

// ---------------------------------------------------------------- opener

#[cfg(unix)]
#[test]
fn open_runs_only_after_a_real_create_and_failing_to_open_is_a_warning() {
    let temp = TempDir::new("album-create-open");
    let vault = clean_vault(&temp);
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
    let mut command = bob_command();
    command.arg("albums").args([
        "create",
        "body",
        "eat",
        "-n",
        "restaurant",
        "-s",
        "Places.",
        "-d",
        "-o",
    ]);
    command
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(&temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(&temp))
        .env("HOME", temp.path().join("home-dir"))
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("PATH", path_with_prefix(&bin));
    assert_success(&command.output().unwrap());
    assert!(!log.exists());

    // The opener is spawned, never awaited: poll for the stub's write.
    let mut command = bob_command();
    command.arg("albums").args([
        "create",
        "body",
        "eat",
        "-n",
        "restaurant",
        "-s",
        "Places.",
        "-o",
        "-f",
        "json",
    ]);
    command
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(&temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(&temp))
        .env("HOME", temp.path().join("home-dir"))
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("PATH", path_with_prefix(&bin));
    assert_success(&command.output().unwrap());
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
        opened.contains("obsidian://open?vault=vault&file=eat.md"),
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
    let mut command = bob_command();
    command.arg("albums").args([
        "create",
        "body",
        "late",
        "-n",
        "late_noun",
        "-s",
        "Late.",
        "-o",
        "-f",
        "json",
    ]);
    command
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(&temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(&temp))
        .env("HOME", temp.path().join("home-dir"))
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("PATH", &only_git);
    let output = command.output().unwrap();
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["ok"], true);
    assert!(payload["warnings"][0]
        .as_str()
        .unwrap()
        .contains("could not open"));
    assert!(vault.join("late.md").exists(), "committed file stays");
}

// ------------------------------------------------------------ integration

fn check_json(vault: &Path, temp: &TempDir, args: &[&str]) -> Value {
    let output = run(vault, temp, args);
    assert_success(&output);
    json(&output)
}

#[test]
fn created_album_accepts_records_and_stays_clean() {
    let temp = TempDir::new("album-create-consumers");
    let vault = clean_vault(&temp);

    let output = run(
        &vault,
        &temp,
        &[
            "create",
            "body",
            "eat",
            "-n",
            "restaurant",
            "-s",
            "Places.",
            "-f",
            "json",
        ],
    );
    assert_success(&output);
    let body_before = read(&vault, "body.md");

    // Capture and `records new` write through the album without touching
    // owner bytes.
    let output = run(&vault, &temp, &["check", "-f", "json"]);
    assert_success(&output);
    let home = temp.path().join("home-dir");
    let mut new = bob_command();
    new.arg("records")
        .args(["new", "restaurant", "Ninja Sushi", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    let output = new.output().expect("run bob records new");
    assert_success(&output);
    let payload = json(&output);
    assert_eq!(payload["status"], "written");
    assert_eq!(payload["source"], "album");
    assert_eq!(
        read(&vault, "body.md"),
        body_before,
        "owner bytes unchanged"
    );
    assert!(
        vault.join("eat").is_dir(),
        "first record creates the folder"
    );
    let album = read(&vault, "eat.md");
    assert!(album.contains("1 restaurants"), "{album}");
    assert!(album.contains("[[eat/ninja_sushi|Ninja Sushi]]"), "{album}");

    // Capture writes through the same album machinery.
    let mut capture_command = bob_command();
    capture_command
        .arg("capture")
        .args(["-b"])
        .arg(&vault)
        .args(["-f", "json", "Ramen Nagi @eat/\n- Spicy tuna was great."])
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("HOME", temp.path().join("home-dir"))
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    let output = capture_command.output().expect("run bob capture");
    assert_success(&output);
    let ramen = read(&vault, "eat/ramen_nagi.md");
    assert!(ramen.contains("# Ramen Nagi"), "{ramen}");
    assert_eq!(
        read(&vault, "body.md"),
        body_before,
        "owner bytes unchanged"
    );
    let album = read(&vault, "eat.md");
    assert!(album.contains("2 restaurants"), "{album}");

    // Update rebuilds the index without touching owner bytes either.
    let mut update = bob_command();
    update
        .arg("records")
        .args([
            "update",
            "restaurant",
            "ninja_sushi",
            "-n",
            "Went back.",
            "-f",
            "json",
        ])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", NOW)
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("HOME", temp.path().join("home-dir"))
        .env("HOSTNAME", "Athena Test")
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    let output = update.output().expect("run bob records update");
    assert_success(&output);
    assert_eq!(json(&output)["status"], "written");
    assert!(
        read(&vault, "eat/ninja_sushi.md").contains("Went back."),
        "note appended"
    );
    assert_eq!(
        read(&vault, "body.md"),
        body_before,
        "owner bytes unchanged"
    );

    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "first record"]);
    git_in(&vault, ["push", "-q"]);
    let payload = check_json(&vault, &temp, &["check", "-f", "json"]);
    assert_eq!(payload["counts"]["errors"], 0, "{payload}");
    assert_eq!(payload["counts"]["warnings"], 0, "{payload}");
    let sync_base = head(&vault);
    let payload = check_json(&vault, &temp, &["sync", "-f", "json"]);
    assert_eq!(payload["ok"], true);
    assert_eq!(head(&vault), sync_base, "sync after records is a no-op");
}

// ------------------------------------------------------- help/completion

#[test]
fn create_help_lists_options_alphabetically() {
    let help = bob_command()
        .args(["albums", "create", "--help"])
        .output()
        .expect("create help");
    assert_success(&help);
    let text = stdout(&help);
    assert!(text.contains("guarded"), "{text}");
    assert!(!text.contains("later phase"), "{text}");
    assert_text_order(
        &text,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-e, --emoji",
            "-f, --format",
            "-h, --help",
            "-l, --label",
            "-n, --noun",
            "-o, --open",
            "-s, --summary",
            "-t, --title",
        ],
    );
}

#[test]
fn completion_offers_create_and_album_parents() {
    let temp = TempDir::new("album-create-complete");
    let vault = clean_vault(&temp);
    write(&vault, "plain.md", "# Plain\n");
    commit_all(&vault, "plain note");

    let members = bob_completion_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "albums",
            "",
        ])
        .env("BOB_DIR", &vault)
        .output()
        .expect("complete albums");
    assert_success(&members);
    let members_out = stdout(&members);
    let names: Vec<&str> = members_out
        .lines()
        .filter(|line| !line.starts_with('!'))
        .filter_map(|line| line.split('\t').next())
        .collect();
    assert!(names.contains(&"create"), "members: {names:?}");

    let parents = bob_completion_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "albums",
            "create",
            "",
        ])
        .env("BOB_DIR", &vault)
        .output()
        .expect("complete parent");
    assert_success(&parents);
    let text = stdout(&parents);
    let names: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with('!'))
        .filter_map(|line| line.split('\t').next())
        .collect();
    assert!(names.contains(&"body"), "parents: {names:?}\n{text}");
    assert!(names.contains(&"sub"), "parents: {names:?}\n{text}");
    assert!(
        !names.contains(&"plain"),
        "only areas and projects: {names:?}\n{text}"
    );
}
