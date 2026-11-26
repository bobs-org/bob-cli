//! Whole-draft `@route^id` caret Go To: parse, refusal, resolver ladder.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};

const BOB_NOW: &str = "2026-11-14 10:00:00";

const BOB_MD: &str = "\
---
type: [[area]]
parent: \"[[life]]\"
done_tasks: \"[[done/bob_custom]]\"
---

# Tasks

- [ ] #task Allow rollovers of intentions [created::2026-11-14] ^intention-rollovers
- [x] #task Finished already [completion:: 2026-01-02] ^done-one

## Heading title ^heading-id

- Plain bullet ^bullet-id

A paragraph with an id ^para-id

```
- Fenced ignored ^fence-id
```

- [ ] #task Dup one [created::2026-11-14] ^dup
- [ ] #task Dup two [created::2026-11-14] ^dup

- [ ] #task Mixed case id [created::2026-11-14] ^MixedCase
- [ ] #task Outline work [created::2026-11-14] ^outline
";

fn t5(
    token: &str,
    route_label: &str,
    block_id: &str,
    notebook: bool,
) -> String {
    let kind = if notebook { "note" } else { "task" };
    format!(
        "`{token}` opens {route_label} at ^{block_id} in Obsidian — it never captures; add {kind} text after the ID to create a {kind} with this ID"
    )
}

fn write_settings(vault: &Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ],
            "customStatuses": [
              {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
}

fn write_vault(vault: &Path) {
    write_settings(vault);
    write_file(&vault.join("mac_inbox.md"), "# inbox\n");
    write_file(
        &vault.join("life.md"),
        "---\ntype: [[area]]\ncolor: green\n---\n\n# Tasks\n\n- [ ] #task Living only [created::2026-11-14] ^living-only\n",
    );
    write_file(&vault.join("bob.md"), BOB_MD);
    write_file(
        &vault.join("cash.md"),
        "---\ntype: [[area]]\nparent: \"[[life]]\"\n---\n\n# Tasks\n\n- [ ] #task Shared elsewhere [created::2026-11-14] ^shared-id\n",
    );
    write_file(
        &vault.join("taxes.md"),
        "---\nparent: \"[[life]]\"\ntype: \"[[notebook]]\"\nsummary: Taxes.\n---\n\n# Taxes\n\n- File 2025 return [created::2026-04-14] ^z-250414-15\n",
    );
    write_file(
        &vault.join("done/bob_custom.md"),
        "# Archived bob\n\n- [x] #task Archived task [completion:: 2025-12-01] ^archived-one\n",
    );
    write_file(
        &vault.join("done/life_done.md"),
        "# Archived life\n\n- [x] #task Canonical archived [completion:: 2025-11-01] ^canonical-one\n",
    );
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

fn parse_json(vault: &Path, text: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob capture-parse json");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture-parse JSON")
}

fn parse_human(vault: &Path, text: &str) -> String {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg(text)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob capture-parse human");
    assert_success(&output);
    stdout(&output)
}

fn line_of(contents: &str, needle: &str) -> usize {
    contents
        .lines()
        .position(|line| line.contains(needle))
        .expect("needle in note")
        + 1
}

fn assert_claimed_parse(json: &serde_json::Value, route: &str, block_id: &str) {
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["schema_version"], 1, "{json}");
    assert_eq!(json["grammar"], 33, "{json}");
    assert_eq!(json["mode"], "task_goto", "{json}");
    assert_eq!(json["needs"], serde_json::json!([]), "{json}");
    assert_eq!(json["body"], "", "{json}");
    assert_eq!(json["route"], route, "{json}");
    assert_eq!(json["block_id"], block_id, "{json}");
    assert_eq!(json["diagnostics"], serde_json::json!([]), "{json}");
    let spans = json["spans"].as_array().expect("spans");
    assert_eq!(spans.len(), 2, "{json}");
    assert_eq!(spans[0]["kind"], "task_block_id_route", "{json}");
    assert_eq!(spans[1]["kind"], "task_block_id", "{json}");
}

fn assert_refusal(
    vault: &Path,
    token: &str,
    route_label: &str,
    block_id: &str,
    notebook: bool,
) {
    let before = snapshot_vault(vault);
    let output = capture_output(vault, &[token]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a caret Go To is a usage error ({token}):\n{}",
        format_output(&output)
    );
    assert_eq!(
        stderr(&output).trim(),
        format!(
            "bob capture: capture item 1 starting on line 1: {}",
            t5(token, route_label, block_id, notebook)
        ),
        "unexpected teaching error ({token}):\n{}",
        format_output(&output)
    );
    let output = capture_output(vault, &["--dry-run", token]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a dry run refuses too ({token}):\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains(&t5(token, route_label, block_id, notebook)),
        "dry run teaches caret Go To ({token}):\n{}",
        format_output(&output)
    );
    assert_eq!(
        snapshot_vault(vault),
        before,
        "a refusal must not change vault bytes ({token})"
    );
}

fn assert_ref_shape(value: &serde_json::Value, line: usize) {
    let text = value.as_str().expect("ref string");
    let (got_line, digest) = text.split_once(':').expect("line:digest");
    assert_eq!(got_line.parse::<usize>().ok(), Some(line), "{text}");
    assert_eq!(digest.len(), 8, "{text}");
    assert!(
        digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{text}"
    );
}

#[test]
fn capture_caret_goto_refusals_leave_the_vault_untouched() {
    let temp = TempDir::new("bob-cli-capture-caret-goto-refusals");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_vault(&vault);

    assert_refusal(
        &vault,
        "@bob^intention-rollovers",
        "bob.md",
        "intention-rollovers",
        false,
    );
    assert_refusal(
        &vault,
        "@taxes^z-250414-15",
        "taxes.md",
        "z-250414-15",
        true,
    );
    let before = snapshot_vault(&vault);
    let output =
        capture_output(&vault, &["-r", "inbox", "@bob^intention-rollovers"]);
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("never captures"),
        "{}",
        format_output(&output)
    );
    assert_eq!(snapshot_vault(&vault), before);
}

#[test]
fn capture_parse_open_task_is_found() {
    let temp = TempDir::new("bob-cli-capture-caret-goto-open");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_vault(&vault);
    let before = snapshot_vault(&vault);

    let json = parse_json(&vault, "@bob^intention-rollovers");
    assert_claimed_parse(&json, "bob", "intention-rollovers");
    let line = line_of(BOB_MD, "^intention-rollovers");
    assert_eq!(json["goto"]["status"], "found");
    assert_eq!(json["goto"]["route"], "bob");
    assert_eq!(json["goto"]["block_id"], "intention-rollovers");
    assert_eq!(json["goto"]["note_path"], "bob.md");
    assert!(!json["goto"]["archived"].as_bool().unwrap_or(false));
    let landing = &json["goto"]["landing"];
    assert_eq!(landing["target_kind"], "task");
    assert_eq!(landing["text"], "Allow rollovers of intentions");
    assert_eq!(landing["status_symbol"], " ");
    assert_eq!(landing["status_name"], "Ready");
    assert_eq!(landing["status_type"], "TODO");
    assert_eq!(landing["line"], line);
    assert_eq!(landing["note_path"], "bob.md");
    assert_ref_shape(&landing["ref"], line);
    let human = parse_human(&vault, "@bob^intention-rollovers");
    assert!(human.contains("go to"), "{human}");
    assert!(human.contains("bob.md"), "{human}");
    assert!(human.contains("Allow rollovers of intentions"), "{human}");
    assert_eq!(snapshot_vault(&vault), before);
}

#[test]
fn capture_parse_closed_heading_bullet_paragraph_and_fence() {
    let temp = TempDir::new("bob-cli-capture-caret-goto-kinds");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_vault(&vault);

    let closed = parse_json(&vault, "@bob^done-one");
    assert_claimed_parse(&closed, "bob", "done-one");
    assert_eq!(closed["goto"]["status"], "found");
    assert_eq!(closed["goto"]["landing"]["target_kind"], "task");
    assert_eq!(closed["goto"]["landing"]["status_type"], "DONE");
    assert_eq!(closed["goto"]["landing"]["closed"], "2026-01-02");
    assert_eq!(closed["goto"]["landing"]["text"], "Finished already");

    let heading = parse_json(&vault, "@bob^heading-id");
    assert_eq!(heading["goto"]["status"], "found");
    assert_eq!(heading["goto"]["landing"]["target_kind"], "heading");
    assert_eq!(heading["goto"]["landing"]["text"], "Heading title");

    let bullet = parse_json(&vault, "@bob^bullet-id");
    assert_eq!(bullet["goto"]["status"], "found");
    assert_eq!(bullet["goto"]["landing"]["target_kind"], "bullet");
    assert_eq!(bullet["goto"]["landing"]["text"], "Plain bullet");

    let paragraph = parse_json(&vault, "@bob^para-id");
    assert_eq!(paragraph["goto"]["status"], "found");
    assert_eq!(paragraph["goto"]["landing"]["target_kind"], "paragraph");
    assert_eq!(
        paragraph["goto"]["landing"]["text"],
        "A paragraph with an id"
    );

    let fenced = parse_json(&vault, "@bob^fence-id");
    assert_eq!(fenced["goto"]["status"], "missing_block_id");
    assert!(fenced["goto"].get("landing").is_none());
}

#[test]
fn capture_parse_duplicate_archive_notebook_and_missing_note() {
    let temp = TempDir::new("bob-cli-capture-caret-goto-ladder");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_vault(&vault);

    let duplicate = parse_json(&vault, "@bob^dup");
    assert_claimed_parse(&duplicate, "bob", "dup");
    assert_eq!(duplicate["goto"]["status"], "duplicate_block_id");
    let hits = duplicate["goto"]["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0]["text"], "Dup one");
    assert_eq!(hits[1]["text"], "Dup two");
    let human = parse_human(&vault, "@bob^dup");
    assert!(human.contains("appears 2"), "{human}");

    let archived = parse_json(&vault, "@bob^archived-one");
    assert_eq!(archived["goto"]["status"], "found");
    assert_eq!(archived["goto"]["archived"], true);
    assert_eq!(archived["goto"]["note_path"], "done/bob_custom.md");
    assert_eq!(archived["goto"]["landing"]["text"], "Archived task");
    assert_eq!(archived["goto"]["landing"]["archived"], true);

    let canonical = parse_json(&vault, "@life^canonical-one");
    assert_eq!(canonical["goto"]["status"], "found");
    assert_eq!(canonical["goto"]["archived"], true);
    assert_eq!(canonical["goto"]["note_path"], "done/life_done.md");
    assert_eq!(canonical["goto"]["landing"]["text"], "Canonical archived");

    let notebook = parse_json(&vault, "@taxes^z-250414-15");
    assert_claimed_parse(&notebook, "taxes", "z-250414-15");
    assert_eq!(notebook["goto"]["status"], "found");
    assert_eq!(notebook["goto"]["landing"]["target_kind"], "notebook_note");
    assert_eq!(
        notebook["goto"]["landing"]["text"],
        "File 2025 return [created::2026-04-14]"
    );

    let missing_note = parse_json(&vault, "@nope^id");
    assert_claimed_parse(&missing_note, "nope", "id");
    assert_eq!(missing_note["goto"]["status"], "missing_note");
    assert_eq!(missing_note["goto"]["note_path"], "nope.md");
    assert!(missing_note["goto"].get("target").is_none());
    let human = parse_human(&vault, "@nope^id");
    assert!(human.contains("no note named nope.md"), "{human}");
}

#[test]
fn capture_parse_suggestions_case_prefix_near_and_other_note() {
    let temp = TempDir::new("bob-cli-capture-caret-goto-suggest");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_vault(&vault);

    let case = parse_json(&vault, "@bob^mixedcase");
    assert_eq!(case["goto"]["status"], "missing_block_id");
    let suggestions = case["goto"]["suggestions"].as_array().expect("case");
    assert_eq!(suggestions[0]["reason"], "case");
    assert_eq!(suggestions[0]["draft"], "@bob^MixedCase");
    assert_eq!(suggestions[0]["block_id"], "MixedCase");

    let prefix = parse_json(&vault, "@bob^intention");
    assert_eq!(prefix["goto"]["status"], "missing_block_id");
    let suggestions = prefix["goto"]["suggestions"].as_array().expect("prefix");
    assert!(
        suggestions.iter().any(|row| {
            row["reason"] == "prefix"
                && row["draft"] == "@bob^intention-rollovers"
        }),
        "{prefix}"
    );

    let near = parse_json(&vault, "@bob^outlne");
    assert_eq!(near["goto"]["status"], "missing_block_id");
    let suggestions = near["goto"]["suggestions"].as_array().expect("near");
    assert!(
        suggestions.iter().any(
            |row| row["reason"] == "near" && row["draft"] == "@bob^outline"
        ),
        "{near}"
    );

    let other = parse_json(&vault, "@bob^shared-id");
    assert_eq!(other["goto"]["status"], "missing_block_id");
    let suggestions = other["goto"]["suggestions"].as_array().expect("other");
    assert_eq!(suggestions[0]["reason"], "other_note");
    assert_eq!(suggestions[0]["draft"], "@cash^shared-id");
    assert_eq!(suggestions[0]["note_path"], "cash.md");
}

#[test]
fn capture_caret_goto_prose_rows_keep_their_current_parse() {
    let temp = TempDir::new("bob-cli-capture-caret-goto-prose");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_vault(&vault);

    for text in [
        "@bob^x Fix it",
        "Fix it @bob^x",
        "Discuss @bob^x later",
        "@bob:x",
        "@bob+x",
        "@bob^",
    ] {
        let json = parse_json(&vault, text);
        assert_ne!(json["mode"], "task_goto", "{text}: {json}");
    }

    let mint = parse_json(&vault, "Fix it @bob^new-id");
    assert_eq!(mint["mode"], "task");
    assert_eq!(mint["block_id"], "new-id");

    let batch = parse_json(&vault, "Buy milk\n\nFix it @bob^x");
    assert_ne!(batch["mode"], "task_goto", "{batch}");
    let items = batch["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{batch}");
    assert_ne!(items[1]["mode"], "task_goto", "{batch}");
}

#[test]
fn capture_batch_with_a_mint_item_captures_both_items_as_usual() {
    let temp = TempDir::new("bob-cli-capture-caret-goto-batch");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_vault(&vault);

    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-15");
    let output =
        run_with_stdin(&mut command, "Buy milk\n\nFix it @bob^brand-new\n");
    assert_success(&output);
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox");
    assert!(inbox.contains("Buy milk"), "{inbox}");
    let bob = fs::read_to_string(vault.join("bob.md")).expect("read bob");
    assert!(bob.contains("Fix it"), "{bob}");
    assert!(bob.contains("^brand-new"), "{bob}");
}
