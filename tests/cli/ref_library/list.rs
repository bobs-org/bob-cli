//! `bob ref list` filtered views: the default queue, every filter,
//! ordering, since, limits, legacy collapse, git dates, formats, errors,
//! and side-effect freedom over the fixture vault.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const FIXED_NOW: &str = "2026-10-06 12:00:00";

fn fixture_vault(prefix: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    copy_dir(&fixture("ref_library/vault"), &vault);
    (temp, vault)
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target)
        .unwrap_or_else(|error| panic!("create {}: {error}", target.display()));
    let entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let child_target = target.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &child_target);
        } else {
            fs::copy(entry.path(), &child_target).unwrap_or_else(|error| {
                panic!("copy {}: {error}", entry.path().display())
            });
        }
    }
}

fn run_list(vault: &Path, args: &[&str]) -> Output {
    bob_command()
        .arg("ref")
        .arg("list")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("run bob ref list")
}

fn run_list_json(vault: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_list(vault, &full);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!("parse list JSON: {error}\n{}", format_output(&output))
    })
}

fn list_paths(document: &serde_json::Value) -> Vec<String> {
    document["refs"]
        .as_array()
        .expect("refs array")
        .iter()
        .map(|row| row["path"].as_str().expect("path string").to_string())
        .collect()
}

fn vault_snapshot(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    walk_snapshot(vault, vault, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn walk_snapshot(root: &Path, path: &Path, files: &mut Vec<(String, Vec<u8>)>) {
    let entries = fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let child = entry.path();
        if child.is_dir() {
            walk_snapshot(root, &child, files);
        } else {
            let relative = child
                .strip_prefix(root)
                .expect("child under root")
                .to_string_lossy()
                .into_owned();
            let contents = fs::read(&child).expect("read file");
            files.push((relative, contents));
        }
    }
}

#[test]
fn list_default_view_is_the_queue_in_every_format() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-default");
    let human = run_list(&vault, &[]);
    assert_success(&human);
    let text = stdout(&human);
    assert!(
        text.contains("reading queue") && text.contains("14 open"),
        "default header:\n{text}"
    );
    assert!(
        text.contains("STARTED") && text.contains("QUEUED"),
        "state groups:\n{text}"
    );
    assert!(
        text.contains("zorg-era legacy note hidden (1 started)"),
        "legacy collapse:\n{text}"
    );

    let document = run_list_json(&vault, &[]);
    assert_eq!(document["matched"], 15);
    assert_eq!(document["returned"], 15);
    assert_eq!(document["truncated"], false);
    assert_eq!(document["hidden"]["superseded"], 1);
    assert_eq!(
        document["filters"]["reading_state"],
        serde_json::json!(["queued", "started"])
    );
    assert_eq!(document["filters"]["reading_state_defaulted"], true);
    assert_eq!(document["filters"]["limit"], 50);
    for row in document["refs"].as_array().expect("refs array") {
        let state = row["reading_state"].as_str().expect("state string");
        assert!(
            state == "queued" || state == "started",
            "queue-only row: {}",
            row["path"]
        );
    }
    // Queue order: started first (modern before legacy), then queued
    // with NEXT before READY. JSON carries the collapsed legacy row.
    let paths = list_paths(&document);
    assert_eq!(paths[0], "ref/papers/two_trackers.md");
    assert_eq!(paths[1], "ref/ai/legacy_collect.md");
    assert_eq!(paths[2], "ref/papers/blocked_mark.md");
    assert_eq!(paths[3], "ref/chat/chat_next.md");

    let markdown = run_list(&vault, &["-f", "markdown"]);
    assert_success(&markdown);
    let body = stdout(&markdown);
    assert!(
        body.starts_with(
            "| State | Status | Date | Title | Type | Note |\n| --- | --- | --- | --- | --- | --- |\n"
        ),
        "markdown table header and six-cell separator:\n{body}"
    );
    assert!(
        body.contains(
            "\n\n15 of 15 matching notes shown · coverage: ref/ only\n"
        ),
        "blank line before the markdown summary:\n{body}"
    );
}

#[test]
fn list_human_rows_align_dated_and_undated_columns() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-align");
    let output = bob_command()
        .arg("ref")
        .arg("list")
        .args(["-R", "queued", "-A"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("COLUMNS", "300")
        .output()
        .expect("run wide bob ref list");
    assert_success(&output);
    let text = stdout(&output);
    let dated = text
        .lines()
        .find(|line| line.ends_with("ref/papers/created_note.md"))
        .unwrap_or_else(|| panic!("dated row:\n{text}"));
    let undated = text
        .lines()
        .find(|line| line.ends_with("ref/papers/wiki_parent.md"))
        .unwrap_or_else(|| panic!("undated row:\n{text}"));
    assert!(undated.contains("—"), "undated row keeps `—`:\n{undated}");
    // Compare display columns (char counts), not byte indices: `—` is
    // multibyte. The date column pads to 10, so titles start together.
    let columns = |line: &str, needle: &str| {
        line.find(needle)
            .map(|index| line[..index].chars().count())
            .unwrap_or_else(|| panic!("missing {needle}:\n{line}"))
    };
    assert_eq!(
        columns(dated, "created note"),
        columns(undated, "Wiki Parent Note"),
        "title columns:\n{dated}\n{undated}",
    );
    // The type column pads to the widest value, so paths start together.
    assert_eq!(
        columns(dated, "ref/papers/created_note.md"),
        columns(undated, "ref/papers/wiki_parent.md"),
        "path columns:\n{dated}\n{undated}",
    );
}

#[test]
fn list_filters_or_within_and_across_options() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-filters");
    // OR within one option: finished plus dropped, every era.
    let document = run_list_json(&vault, &["-R", "finished,dropped"]);
    assert_eq!(document["matched"], 13);
    assert_eq!(document["filters"]["reading_state_defaulted"], false);

    // OR within status, across modern and legacy.
    let document = run_list_json(&vault, &["-s", "read,abandoned"]);
    assert_eq!(document["matched"], 8);

    // AND across options: finished agent reports about chat.
    let document = run_list_json(&vault, &["-R", "finished", "-t", "chat"]);
    assert_eq!(list_paths(&document), vec!["ref/chat/chat_read.md"]);

    // Origin searches every state once it is a filter.
    let document = run_list_json(&vault, &["-o", "agent-report"]);
    assert_eq!(document["matched"], 4);

    // Parent matches the bare note name.
    let document = run_list_json(&vault, &["-P", "sase_ref"]);
    assert_eq!(document["matched"], 3);

    // `-R all` is every state, with no defaulting.
    let document = run_list_json(&vault, &["-R", "all"]);
    assert_eq!(document["matched"], 33);
    assert_eq!(document["filters"]["reading_state_defaulted"], false);
}

#[test]
fn list_unknown_and_conflict_states() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-unknown");
    let document = run_list_json(&vault, &["-R", "unknown"]);
    let mut paths = list_paths(&document);
    paths.sort();
    assert_eq!(
        paths,
        vec![
            "ref/ai/legacy_book.md",
            "ref/ai/legacy_missing.md",
            "ref/papers/conflict_note.md",
            "ref/papers/two_trackers_blocked.md",
            "ref/toplevel_note.md",
        ]
    );
    // `-s conflict` is the tracker/frontmatter conflict row only, while
    // `-s unknown` is the row with no status at all.
    let document = run_list_json(&vault, &["-s", "conflict"]);
    assert_eq!(list_paths(&document), vec!["ref/papers/conflict_note.md"]);
    let document = run_list_json(&vault, &["-s", "unknown"]);
    assert_eq!(
        list_paths(&document),
        vec!["ref/toplevel_note.md", "ref/papers/two_trackers_blocked.md"]
    );
}

#[test]
fn list_rows_carry_always_present_blocked() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-blocked");
    let document = run_list_json(&vault, &["-R", "all", "-A"]);
    let rows = document["refs"].as_array().expect("refs array");
    assert!(!rows.is_empty());
    for row in rows {
        assert!(
            row.get("blocked").is_some_and(|value| value.is_boolean()),
            "always-present blocked boolean: {}",
            row["path"]
        );
    }
    let blocked = rows
        .iter()
        .find(|row| row["path"] == "ref/papers/blocked_mark.md")
        .expect("blocked row");
    assert_eq!(blocked["blocked"], true);
    let synced = rows
        .iter()
        .find(|row| row["path"] == "ref/papers/synced_paper.md")
        .expect("synced row");
    assert_eq!(synced["blocked"], false);
    // Several trackers with one `[?]` still leave `blocked` false.
    let pair = rows
        .iter()
        .find(|row| row["path"] == "ref/papers/two_trackers_blocked.md")
        .expect("two-tracker blocked row");
    assert_eq!(pair["blocked"], false);
}

#[test]
fn list_hides_superseded_notes_everywhere() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-superseded");
    for args in [vec!["-R", "all"], vec!["-s", "legacy"]] {
        let document = run_list_json(&vault, &args);
        assert_eq!(
            document["hidden"]["superseded"], 1,
            "superseded counted: {:?}",
            args
        );
        assert!(
            !list_paths(&document)
                .contains(&"ref/ai/legacy_unread.md".to_string()),
            "superseded legacy twin never lists: {:?}",
            args
        );
    }
}

#[test]
fn list_orders_states_era_status_dates_and_titles() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-order");
    // Finished externals (legacy ai notes are external too): modern
    // first, newest row date first, undated (tracker without a completion
    // date, or a legacy note) last by title.
    let document = run_list_json(&vault, &["-R", "finished", "-o", "external"]);
    assert_eq!(
        list_paths(&document),
        vec![
            "ref/papers/arxiv_pdf.md",
            "ref/papers/dates_done.md",
            "ref/blogs/clipped_blog.md",
            "ref/papers/dates_emoji.md",
            "ref/papers/pending_note.md",
            "ref/ai/legacy_read.md",
            "ref/ai/legacy_review_fleeting.md",
            "ref/ai/legacy_review_lit.md",
            "ref/ai/zorg_block.md",
        ]
    );
    // Modern finished rows precede legacy finished rows.
    let document = run_list_json(&vault, &["-R", "finished"]);
    let paths = list_paths(&document);
    assert_eq!(paths.len(), 10);
    assert!(
        paths[..6].iter().all(|path| !path.starts_with("ref/ai/")),
        "modern first: {paths:?}"
    );
    assert!(
        paths[6..].iter().all(|path| path.starts_with("ref/ai/")),
        "legacy last: {paths:?}"
    );
}

#[test]
fn list_since_filters_by_row_date() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-since");
    let document =
        run_list_json(&vault, &["-R", "finished", "-S", "2026-09-01"]);
    assert_eq!(document["matched"], 2);
    assert_eq!(document["filters"]["undated_excluded"], 6);
    assert_eq!(
        list_paths(&document),
        vec!["ref/papers/arxiv_pdf.md", "ref/papers/dates_done.md"]
    );
    // Relative dates resolve against the clock (`BOB_NOW`): sixty days
    // back from 2026-10-06 keeps the four dated August/September rows.
    let document = run_list_json(&vault, &["-R", "finished", "-S", "60d"]);
    assert_eq!(document["matched"], 4);

    let invalid = run_list(&vault, &["-S", "someday"]);
    assert_eq!(
        invalid.status.code(),
        Some(2),
        "bad --since is a usage error:\n{}",
        format_output(&invalid)
    );
}

#[test]
fn list_limit_all_and_truncation() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-limit");
    let capped = run_list(&vault, &["-n", "2"]);
    assert_success(&capped);
    assert!(
        stdout(&capped).contains("… 12 more · -n N or -A to show more"),
        "truncation line:\n{}",
        stdout(&capped)
    );

    let document = run_list_json(&vault, &["-n", "2"]);
    assert_eq!(document["matched"], 15);
    assert_eq!(document["returned"], 2);
    assert_eq!(document["truncated"], true);
    assert_eq!(list_paths(&document).len(), 2);

    let document = run_list_json(&vault, &["-A"]);
    assert_eq!(document["matched"], 15);
    assert_eq!(document["returned"], 15);
    assert_eq!(document["truncated"], false);
    assert_eq!(document["filters"]["limit"], serde_json::Value::Null);

    let conflict = run_list(&vault, &["-A", "-n", "5"]);
    assert_eq!(
        conflict.status.code(),
        Some(2),
        "-A with -n is a usage error:\n{}",
        format_output(&conflict)
    );
}

#[test]
fn list_legacy_collapse_versus_status_filter() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-collapse");
    let collapsed = run_list(&vault, &["-R", "finished"]);
    assert_success(&collapsed);
    assert!(
        stdout(&collapsed)
            .contains("+ 4 zorg-era legacy notes hidden (4 finished)"),
        "collapse line:\n{}",
        stdout(&collapsed)
    );
    let listed = run_list(&vault, &["-R", "finished", "-s", "legacy"]);
    assert_success(&listed);
    let text = stdout(&listed);
    assert!(!text.contains("hidden"), "no collapse with -s:\n{text}");
    assert!(
        text.contains("Legacy Read Note"),
        "legacy rows list with -s:\n{text}"
    );
}

#[test]
fn list_empty_views_say_so() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-empty");
    let output = run_list(&vault, &["-P", "no_such_parent"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("No matching notes."),
        "filtered empty:\n{}",
        stdout(&output)
    );

    // A vault with nothing queued reports the empty queue, not an empty
    // table.
    let temp = TempDir::new("bob-cli-ref-list-empty-queue");
    let single = temp.path().join("vault");
    write_file(
        &single.join("ref/papers/done.md"),
        "---\nstatus: read\ntitle: Done Already\n---\n\n- [x] #task #ref [[lib/papers/done.pdf]] #hide ^ref\n",
    );
    let output = run_list(&single, &[]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Nothing queued ✓"),
        "empty queue:\n{}",
        stdout(&output)
    );
}

fn write_git_note(vault: &Path, relative: &str, contents: &str) {
    let path = vault.join(relative);
    fs::create_dir_all(path.parent().expect("note parent"))
        .expect("create note parent");
    write_file(&path, contents);
}

fn git_commit(vault: &Path, message: &str, author_date: &str, files: &[&str]) {
    let mut add = std::process::Command::new("git");
    add.arg("-C")
        .arg(vault)
        .arg("add")
        .args(files)
        .output()
        .expect("git add");
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(vault)
        .arg("commit")
        .arg("-m")
        .arg(message)
        .env("GIT_AUTHOR_DATE", author_date)
        .env("GIT_COMMITTER_DATE", author_date)
        .output()
        .expect("git commit");
    assert!(
        status.status.success(),
        "git commit {message}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

#[test]
fn list_git_dates_backfills_modern_notes_only() {
    let temp = TempDir::new("bob-cli-ref-list-git");
    let vault = temp.path().join("vault");
    write_git_note(
        &vault,
        "ref/papers/modern_note.md",
        "---\nstatus: ready\ntitle: Modern Note\n---\n\n- [ ] #task #ref [[lib/papers/modern.pdf]] #hide ^ref\n",
    );
    write_git_note(
        &vault,
        "ref/ai/legacy_note.md",
        "---\nstatus: legacy\nlegacy_status: unread\ntitle: Legacy Note\nsource_block: ^z-260101-abcdef\n---\n\nMigrated.\n",
    );
    git_in(&vault, ["init"]);
    configure_test_git_identity(&vault);
    git_commit(
        &vault,
        "capture modern and legacy notes",
        "2026-08-01T10:00:00",
        &["ref/papers/modern_note.md", "ref/ai/legacy_note.md"],
    );
    write_git_note(
        &vault,
        "ref/papers/done_note.md",
        "---\nstatus: read\ntitle: Done Note\n---\n\n- [x] #task #ref [[lib/papers/done.pdf]] #hide ^ref\n",
    );
    git_commit(
        &vault,
        "finish one note",
        "2026-09-15T10:00:00",
        &["ref/papers/done_note.md"],
    );

    let document = run_list_json(&vault, &["-g", "-R", "all", "-A"]);
    assert!(
        document["coverage"].get("git_dates").is_none(),
        "no git_dates word on success: {}",
        document["coverage"]
    );
    let rows = document["refs"].as_array().expect("refs array");
    let row = rows
        .iter()
        .find(|row| row["path"] == "ref/papers/modern_note.md")
        .expect("modern row");
    assert_eq!(row["added"], "2026-08-01");
    assert_eq!(row["added_source"], "git");
    let row = rows
        .iter()
        .find(|row| row["path"] == "ref/papers/done_note.md")
        .expect("done row");
    assert_eq!(row["added"], "2026-09-15");
    assert_eq!(row["finished"], "2026-09-15");
    assert_eq!(row["finished_source"], "git");
    // The legacy note keeps its zorg block date, never Git history.
    let row = rows
        .iter()
        .find(|row| row["path"] == "ref/ai/legacy_note.md")
        .expect("legacy row");
    assert_eq!(row["added"], "2026-01-01");
    assert_eq!(row["added_source"], "zorg_block");
}

#[test]
fn list_git_dates_without_git_warns_and_continues() {
    let temp = TempDir::new("bob-cli-ref-list-no-git");
    let vault = temp.path().join("vault");
    write_git_note(
        &vault,
        "ref/papers/lonely_note.md",
        "---\nstatus: ready\ntitle: Lonely Note\n---\n\n- [ ] #task #ref [[lib/papers/lonely.pdf]] #hide ^ref\n",
    );
    assert!(
        !vault.join(".git").exists(),
        "temp vault must not be a git repo"
    );
    let output = run_list(&vault, &["-g", "-f", "json"]);
    assert_success(&output);
    let document: serde_json::Value = serde_json::from_str(&stdout(&output))
        .unwrap_or_else(|error| {
            panic!("parse list JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(document["coverage"]["git_dates"], "unavailable");
    let captured = stderr(&output);
    let warnings: Vec<&str> = captured
        .lines()
        .filter(|line| line.contains("warning"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "exactly one git warning:\n{}",
        format_output(&output)
    );
}

#[test]
fn list_markdown_covers_dropped_rows() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-markdown");
    let output = run_list(&vault, &["-R", "dropped", "-f", "markdown"]);
    assert_success(&output);
    let body = stdout(&output);
    assert!(
        body.contains("| Dropped | ABANDONED |"),
        "dropped rows:\n{body}"
    );
    assert!(
        body.contains("3 of 3 matching notes shown · coverage: ref/ only"),
        "markdown footer:\n{body}"
    );
}

#[test]
fn list_missing_ref_dir_fails_like_find() {
    let temp = TempDir::new("bob-cli-ref-list-missing-dir");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let human = run_list(&vault, &[]);
    assert_eq!(
        human.status.code(),
        Some(1),
        "missing ref dir exits 1:\n{}",
        format_output(&human)
    );
    assert!(
        stderr(&human).contains("reference directory not found"),
        "human error:\n{}",
        format_output(&human)
    );
    let json = run_list(&vault, &["-f", "json"]);
    assert_eq!(json.status.code(), Some(1));
    let document: serde_json::Value = serde_json::from_str(&stdout(&json))
        .unwrap_or_else(|error| {
            panic!("parse error JSON: {error}\n{}", format_output(&json))
        });
    assert_eq!(document["ok"], false);
    assert_eq!(document["command"], "ref list");
}

#[test]
fn list_leaves_the_vault_untouched() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-list-side-effects");
    let before = vault_snapshot(&vault);
    for args in [
        vec![],
        vec!["-R", "finished", "-f", "json"],
        vec!["-R", "all", "-f", "markdown", "-A"],
        vec!["-g"],
        vec!["-g", "-f", "json", "-S", "30d"],
    ] {
        let output = run_list(&vault, &args);
        assert_success(&output);
    }
    assert_eq!(
        vault_snapshot(&vault),
        before,
        "list must not write to the vault"
    );
}
