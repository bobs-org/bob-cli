//! Whole-draft `//query` Go To Notes: parse, refusal, rollback, candidates.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};

const BOB_NOW: &str = "2026-11-14 10:00:00";

fn t4(token: &str) -> String {
    format!(
        "`{token}` opens the Go To Notes picker; pick a note to open it in Obsidian — it never captures"
    )
}

fn write_catalog_vault(vault: &Path) {
    write_file(&vault.join("mac_inbox.md"), "# inbox\n");
    write_file(
        &vault.join("life.md"),
        "---\ntype: [[area]]\ncolor: green\n---\n",
    );
    write_file(
        &vault.join("cash.md"),
        "---\ntype: [[area]]\nparent: \"[[life]]\"\naliases:\n  - money\n---\n",
    );
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[project]]\nstatus: wip\nparent: \"[[life]]\"\n---\n\n- [ ] Ship it ^prj\n",
    );
    write_file(
        &vault.join("ideas.md"),
        "---\ntitle: Ideas\naliases:\n  - sparks\n---\n",
    );
    write_file(&vault.join("2026/20261114.md"), "# plain daily\n");
    write_file(
        &vault.join("done/a_done.md"),
        "---\ntitle: A\nparent: \"[[areas/a]]\"\n---\n",
    );
    write_file(&vault.join("ref_history/old.md"), "# old\n");
    write_file(&vault.join("2026/20261114_habit.md"), "# old satellite\n");
    write_file(&vault.join("x.base"), "---\ntitle: Base\n---\n");
    write_file(&vault.join("_meta/hidden.md"), "# machinery\n");
}

fn snapshot_vault(vault: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut names: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("read vault dir")
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        names.sort();
        for path in names {
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path).expect("read vault file");
                entries.push((path, bytes));
            }
        }
    }
    entries.sort();
    entries
}

fn capture_output(vault: &Path, args: &[&str]) -> std::process::Output {
    bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .args(args)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob capture")
}

fn complete_json(vault: &Path, cursor: usize, text: &str) -> serde_json::Value {
    complete_json_with_flags(vault, cursor, text, &[])
}

fn complete_json_with_flags(
    vault: &Path,
    cursor: usize,
    text: &str,
    flags: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .args(flags)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run note_goto completion");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn parse_json(text: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run bob capture-parse json");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

fn assert_note_goto_picker(json: &serde_json::Value, end: usize) {
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["schema_version"], 1, "{json}");
    assert_eq!(json["grammar"], 33, "{json}");
    assert_eq!(json["context"], "note_goto", "{json}");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 0, "end": end}),
        "{json}"
    );
    assert!(json.get("query").is_none(), "{json}");
    let picker = json["picker"].as_object().expect("picker");
    assert_eq!(picker["kind"], "note_goto", "{json}");
    assert_eq!(picker["scope"], "vault", "{json}");
    assert_eq!(picker["scope_token"], "//", "{json}");
    assert_eq!(
        picker["marker_range"],
        serde_json::json!({"start": 0, "end": end}),
        "{json}"
    );
    assert_eq!(
        picker["trigger_removal_range"],
        serde_json::json!({"start": 1, "end": end}),
        "{json}"
    );
    assert!(picker.get("action_continuation_keys").is_none(), "{json}");
}

#[test]
fn capture_note_goto_query_fails_with_the_t4_teaching_error() {
    let temp = TempDir::new("bob-cli-capture-note-goto");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("//cash")
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run note_goto query capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "a picker query is a usage error:\n{}",
        format_output(&output)
    );
    assert_eq!(
        stderr(&output).trim(),
        format!(
            "bob capture: capture item 1 starting on line 1: {}",
            t4("//cash")
        ),
        "unexpected teaching error:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "an unfinished query must not create a junk inbox task"
    );
}

#[test]
fn capture_note_goto_refusals_cover_slash_forms_and_leave_the_vault_untouched()
{
    let temp = TempDir::new("bob-cli-capture-note-goto-refusals");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);
    let before = snapshot_vault(&vault);

    let cases: Vec<Vec<&str>> =
        vec![vec!["//"], vec!["//cash"], vec!["-r", "inbox", "//cash"]];
    for args in &cases {
        let output = capture_output(&vault, args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "a picker query is a usage error ({args:?}):\n{}",
            format_output(&output)
        );
        let token = args.last().expect("token");
        assert_eq!(
            stderr(&output).trim(),
            format!(
                "bob capture: capture item 1 starting on line 1: {}",
                t4(token)
            ),
            "unexpected teaching error ({args:?}):\n{}",
            format_output(&output)
        );
        let dry_run: Vec<&str> = std::iter::once("--dry-run")
            .chain(args.iter().copied())
            .collect();
        let output = capture_output(&vault, &dry_run);
        assert_eq!(
            output.status.code(),
            Some(2),
            "a dry run refuses too ({args:?}):\n{}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(&t4(token)),
            "dry run teaches Go To Notes ({args:?}):\n{}",
            format_output(&output)
        );
        assert_eq!(
            snapshot_vault(&vault),
            before,
            "a refusal must not change vault bytes ({args:?})"
        );
    }
}

#[test]
fn capture_batch_with_a_double_slash_item_captures_both_items_as_usual() {
    let temp = TempDir::new("bob-cli-capture-note-goto-batch");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-15");
    let output = run_with_stdin(&mut command, "Buy milk\n\n//x\n");

    assert_success(&output);
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox");
    assert!(inbox.contains("Buy milk"), "{inbox}");
    assert!(inbox.contains("//x"), "{inbox}");
}

#[test]
fn capture_parse_note_goto_query_reports_incomplete_note_goto() {
    for (text, end) in [("//", 2), ("//cash", 6)] {
        let json = parse_json(text);
        assert_eq!(json["ok"], true);
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["grammar"], 33);
        assert_eq!(json["mode"], "incomplete");
        assert_eq!(json["needs"], serde_json::json!(["note_goto"]));
        assert_eq!(json["body"], "");
        assert!(json["route"].is_null(), "{json}");
        assert!(json["section"].is_null(), "{json}");
        assert!(json["block_id"].is_null(), "{json}");
        assert_eq!(
            json["spans"],
            serde_json::json!([{"start": 0, "end": end, "kind": "interactive_placeholder"}]),
            "{json}"
        );
        assert_eq!(json["diagnostics"], serde_json::json!([]), "{json}");
    }
}

#[test]
fn capture_parse_note_goto_prose_rows_keep_their_current_parse() {
    for text in [
        "///", "///x", "/Users/x", "/r/rust", "/a/b", "// cash", "//a b",
        "Buy //x",
    ] {
        let json = parse_json(text);
        assert_ne!(
            json["needs"],
            serde_json::json!(["note_goto"]),
            "{text}: {json}"
        );
        assert_ne!(
            json["needs"],
            serde_json::json!(["task_goto"]),
            "{text}: {json}"
        );
    }
    let json = parse_json("// cash");
    assert_eq!(json["body"], "// cash", "{json}");

    for text in ["//x\n- x", "//x\n\nBuy milk", "@@work\n//x"] {
        let json = parse_json(text);
        let needs = json.get("needs").cloned().unwrap_or_default();
        assert_ne!(needs, serde_json::json!(["note_goto"]), "{text:?}: {json}");
    }
}

#[test]
fn capture_complete_note_goto_reports_shape_without_query() {
    let temp = TempDir::new("bob-cli-capture-note-goto-complete");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    // `vault_root` is canonicalized by the catalog, so compare canonical
    // forms: on macOS the temp vault sits under the `/var` → `/private/var`
    // symlink and the as-passed path never equals the reported root.
    let expected_root = std::fs::canonicalize(&vault)
        .unwrap_or_else(|_| vault.clone())
        .display()
        .to_string();
    for (text, cursor, end) in [("//", 2, 2), ("//ca", 4, 4)] {
        let json = complete_json(&vault, cursor, text);
        assert_note_goto_picker(&json, end);
        assert_eq!(json["note_goto"]["vault_root"], expected_root, "{json}");
        let listed =
            json["note_goto"]["listed"].as_u64().expect("listed count");
        assert!(listed >= 1, "{json}");
        let candidates =
            json["candidates"].as_array().expect("candidates array");
        if text == "//" {
            assert_eq!(listed, candidates.len() as u64, "{json}");
            assert_eq!(
                candidates[0]["note_path"], "2026/20261114.md",
                "{json}"
            );
            assert_eq!(candidates[0]["kind"], "daily", "{json}");
            assert_eq!(candidates[0]["group"], "today", "{json}");
            let inbox = candidates
                .iter()
                .find(|row| row["note_path"] == "mac_inbox.md")
                .expect("inbox row");
            assert_eq!(inbox["kind"], "inbox", "{inbox}");
            assert_eq!(inbox["group"], "inbox", "{inbox}");
            assert!(
                candidates.iter().all(|row| row["replacement"] == ""),
                "{json}"
            );
            assert!(
                candidates
                    .iter()
                    .all(|row| row["note_path"] != "_meta/hidden.md"),
                "hidden machinery must not list: {json}"
            );
            let view = candidates
                .iter()
                .find(|row| row["note_path"] == "x.base")
                .expect("view row");
            assert_eq!(view["kind"], "view", "{view}");
        } else {
            assert_eq!(listed, 7, "{json}");
            assert!(
                candidates.iter().any(|row| row["note_path"] == "cash.md"),
                "query ca should match cash: {json}"
            );
            assert!(
                candidates.iter().all(|row| row["replacement"] == ""),
                "{json}"
            );
        }
    }
}

#[test]
fn complete_note_goto_candidates_carry_contract_keys() {
    let temp = TempDir::new("bob-cli-complete-note-goto-keys");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    let json = complete_json(&vault, 2, "//");
    assert_eq!(json["note_goto"]["archived"], false, "{json}");
    let raw_output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("2")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("//")
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run note_goto completion");
    assert_success(&raw_output);
    let raw = stdout(&raw_output);
    let candidates = json["candidates"].as_array().expect("candidates array");
    assert!(!candidates.is_empty(), "{json}");

    let required = [
        "replacement",
        "note_path",
        "kind",
        "title",
        "match_keys",
        "rank",
    ];
    let body = raw
        .split_once("\"candidates\":")
        .expect("candidates array")
        .1;
    let mut cursor = 0;
    for key in required {
        let needle = format!("\"{key}\":");
        let at = body[cursor..].find(&needle).unwrap_or_else(|| {
            panic!("missing key {key} in candidate order: {body}")
        });
        cursor += at + needle.len();
    }

    for candidate in candidates {
        for key in required {
            assert!(
                candidate.get(key).is_some(),
                "missing {key} in {candidate}"
            );
        }
        assert_eq!(candidate["replacement"], "", "{candidate}");
        let keys = candidate["match_keys"]
            .as_array()
            .expect("match_keys array");
        assert!(!keys.is_empty(), "{candidate}");
        assert_eq!(keys[0], candidate["title"], "{candidate}");
    }

    let cash = candidates
        .iter()
        .find(|row| row["note_path"] == "cash.md")
        .expect("cash row");
    assert_eq!(cash["kind"], "area");
    assert_eq!(cash["title"], "cash");
    assert_eq!(cash["match_keys"][0], "cash");
    assert!(
        cash["match_keys"]
            .as_array()
            .expect("keys")
            .iter()
            .any(|key| key == "money"),
        "{cash}"
    );
    assert_eq!(cash["group"], "areas");
}

#[test]
fn complete_note_goto_query_ranks_match_keys_then_path() {
    let temp = TempDir::new("bob-cli-complete-note-goto-rank");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    let money = complete_json(&vault, 7, "//money");
    let money_paths: Vec<&str> = money["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|row| row["note_path"].as_str().expect("note_path"))
        .collect();
    assert_eq!(money_paths.first().copied(), Some("cash.md"), "{money}");

    let sparks = complete_json(&vault, 8, "//sparks");
    let sparks_paths: Vec<&str> = sparks["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|row| row["note_path"].as_str().expect("note_path"))
        .collect();
    assert_eq!(sparks_paths.first().copied(), Some("ideas.md"), "{sparks}");
}

#[test]
fn complete_note_goto_flags_do_not_change_the_catalog() {
    let temp = TempDir::new("bob-cli-complete-note-goto-flags");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    let plain = complete_json(&vault, 2, "//");
    assert_eq!(plain["note_goto"]["archived"], false, "{plain}");
    for flags in [
        &["-C"][..],
        &["-a"][..],
        &["-n"][..],
        &["-F", "visited"][..],
    ] {
        let flagged = complete_json_with_flags(&vault, 2, "//", flags);
        assert_eq!(flagged["context"], "note_goto", "{flags:?}");
        assert_eq!(flagged["candidates"], plain["candidates"], "{flags:?}");
        assert_eq!(flagged["note_goto"], plain["note_goto"], "{flags:?}");
    }
}

#[test]
fn complete_note_goto_archive_scope_is_explicit_and_ranked() {
    let temp = TempDir::new("bob-cli-complete-note-goto-archive");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    let living = complete_json(&vault, 2, "//");
    let archive = complete_json_with_flags(&vault, 2, "//", &["-A"]);
    assert_eq!(living["note_goto"]["archived"], false, "{living}");
    assert_eq!(archive["note_goto"]["archived"], true, "{archive}");
    assert!(
        !living["candidates"]
            .as_array()
            .expect("living candidates")
            .iter()
            .any(|row| row["note_path"] == "done/a_done.md"),
        "{living}"
    );
    let rows = archive["candidates"].as_array().expect("archive rows");
    assert_eq!(rows[0]["note_path"], "done/a_done.md", "{archive}");
    assert_eq!(rows[0]["kind"], "archive", "{archive}");
    assert_eq!(rows[0]["group"], "done", "{archive}");
    assert_eq!(rows[0]["title"], "A", "{archive}");
    assert!(rows.iter().any(|row| {
        row["note_path"] == "ref_history/old.md" && row["kind"] == "archive"
    }));
    assert!(rows.iter().any(|row| {
        row["note_path"] == "2026/20261114_habit.md"
            && row["kind"] == "satellite"
            && row["title"] == "2026-11-14 habit"
    }));

    let filtered =
        complete_json_with_flags(&vault, 3, "//a", &["--archived-notes"]);
    assert_eq!(filtered["note_goto"]["archived"], true, "{filtered}");
    assert_eq!(
        filtered["candidates"][0]["note_path"], "done/a_done.md",
        "{filtered}"
    );
}

#[test]
fn archived_notes_flag_is_ignored_outside_note_goto() {
    let temp = TempDir::new("bob-cli-complete-archived-notes-scope");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    for (text, cursor) in [("/", 1), (":", 1), ("[[cash", 6)] {
        let plain = complete_json(&vault, cursor, text);
        let flagged = complete_json_with_flags(&vault, cursor, text, &["-A"]);
        assert_eq!(flagged["context"], plain["context"], "{text}: {flagged}");
        assert_eq!(flagged["candidates"], plain["candidates"], "{text}");
        assert_eq!(flagged.get("note_goto"), plain.get("note_goto"), "{text}");
    }
}

#[test]
fn complete_note_goto_human_lines_show_kind_title_and_path() {
    let temp = TempDir::new("bob-cli-complete-note-goto-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    let human = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("2")
        .arg("--")
        .arg("//")
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run human note_goto completion");
    assert_success(&human);
    let out = stdout(&human);
    assert!(out.contains("note_goto"), "{out}");
    assert!(out.contains("cash  · cash.md"), "{out}");
    assert!(out.contains("area"), "{out}");
    assert!(
        out.contains("inbox  · mac_inbox.md") || out.contains("mac_inbox.md"),
        "{out}"
    );
    assert!(out.contains("candidates"), "{out}");
    assert!(!out.contains("_meta/hidden.md"), "{out}");
}

#[test]
fn complete_note_goto_archive_human_lines_keep_the_row_format() {
    let temp = TempDir::new("bob-cli-complete-note-goto-archive-human");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    let output = bob_command()
        .arg("capture-complete")
        .arg("-A")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg("2")
        .arg("--")
        .arg("//")
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run archived note_goto completion");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("archive A  · done/a_done.md"), "{out}");
    assert!(
        out.contains("satellite 2026-11-14 habit  · 2026/20261114_habit.md"),
        "{out}"
    );
}

#[test]
fn shell_completion_serves_no_note_goto_rows() {
    let temp = TempDir::new("bob-cli-complete-note-goto-shell");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    let output = bob_completion_command()
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .arg("bob")
        .arg("capture")
        .arg("//")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob __complete");
    assert_success(&output);
    let out = stdout(&output);
    assert!(!out.contains("cash.md"), "nothing is insertable: {out}");
    assert!(!out.contains("mac_inbox"), "nothing is insertable: {out}");
}

#[test]
fn capture_complete_bare_slash_still_advertises_notes_continuation() {
    let temp = TempDir::new("bob-cli-capture-note-goto-continuation");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_catalog_vault(&vault);

    let bare = complete_json(&vault, 1, "/");
    assert_eq!(bare["context"], "task_goto", "{bare}");
    assert_eq!(
        bare["picker"]["action_continuation_keys"],
        serde_json::json!(["/"]),
        "{bare}"
    );

    let typed = complete_json(&vault, 4, "/fix");
    assert_eq!(typed["context"], "task_goto", "{typed}");
    assert!(
        typed["picker"]
            .as_object()
            .expect("picker")
            .get("action_continuation_keys")
            .is_none(),
        "{typed}"
    );
}
