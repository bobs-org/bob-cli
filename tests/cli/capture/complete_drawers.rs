//! The `drawer` completion menu on the wire: groups, bob-authored copy,
//! refusals that match the commit, and the `#`-covering replacement.

use crate::support::*;
use serde_json::Value;
use std::path::Path;

const FOO: &str = concat!(
    "---\ntype: [[area]]\n---\n",
    "- [ ] #task Upgrade Postgres ^bar\n",
    "\t- REQUIREMENTS\n",
    "\t\t- 2 GB RAM\n",
    "\t\t- fast disk\n",
    "\t- NOTES\n",
    "\t- NOTES\n",
    "\t- FUTURE WORK\n",
    "- [ ] #task Plain ^plain\n",
);

/// A vault whose catalog holds MOTIVATION (three notes), ACCEPTANCE (two),
/// and PERFORMANCE (one) besides the target note `foo.md`.
fn drawer_vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(&vault.join("foo.md"), FOO);
    for (name, drawers) in [
        ("other1", &["MOTIVATION", "ACCEPTANCE", "PERFORMANCE"][..]),
        ("other2", &["MOTIVATION", "ACCEPTANCE"][..]),
        ("other3", &["MOTIVATION"][..]),
    ] {
        let mut note =
            format!("---\ntype: [[area]]\n---\n- [ ] #task T ^{name}\n");
        for drawer in drawers {
            note.push_str(&format!("\t- {drawer}\n"));
        }
        write_file(&vault.join(format!("{name}.md")), &note);
    }
    vault
}

fn complete(temp: &TempDir, vault: &Path, draft: &str) -> Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run capture-complete");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("completion JSON")
}

fn rows(json: &Value) -> &Vec<Value> {
    json["candidates"].as_array().expect("candidates")
}

fn shape(json: &Value) -> Vec<String> {
    rows(json)
        .iter()
        .map(|row| {
            format!(
                "{}:{}",
                row["group"].as_str().expect("group"),
                row["title"].as_str().expect("title")
            )
        })
        .collect()
}

fn row<'a>(json: &'a Value, group: &str, title: &str) -> &'a Value {
    rows(json)
        .iter()
        .find(|row| row["group"] == group && row["title"] == title)
        .unwrap_or_else(|| panic!("no {group} row {title}: {:?}", shape(json)))
}

/// `draft` with the menu's replacement range swapped for a row's text.
fn accept(draft: &str, menu: &Value, replacement: &str) -> String {
    let start = menu["replacement"]["start"].as_u64().expect("start") as usize;
    let end = menu["replacement"]["end"].as_u64().expect("end") as usize;
    format!("{}{replacement}{}", &draft[..start], &draft[end..])
}

fn dry_run(vault: &Path, draft: &str) -> (bool, Value) {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--dry-run")
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .env("BOB_NOW", "2026-10-30 09:15:00")
        .output()
        .expect("run dry-run capture");
    let json =
        serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
            panic!("capture JSON: {error}\n{}", format_output(&output))
        });
    (output.status.success(), json)
}

#[test]
fn empty_drawer_menu_json_pins_groups_fields_and_the_unselected_open() {
    let temp = TempDir::new("bob-cli-drawer-menu-wire");
    let vault = drawer_vault(&temp);
    let draft = "Postgres 17 minimum @foo+bar#";
    let json = complete(&temp, &vault, draft);

    assert_eq!(json["grammar"], 4);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["context"], "drawer");
    assert_eq!(json["initial_selection"], "none");
    assert_eq!(json["query"], "");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": draft.len() - 1, "end": draft.len()})
    );
    assert_eq!(
        json["target"],
        serde_json::json!({
            "route": "foo.md",
            "block_id": "bar",
            "text": "Upgrade Postgres",
        })
    );
    assert_eq!(
        shape(&json),
        [
            "task:REQUIREMENTS",
            "task:NOTES",
            "task:NOTES",
            "task:FUTURE WORK",
            "recent:MOTIVATION",
            "recent:ACCEPTANCE",
        ]
    );

    let requirements = row(&json, "task", "REQUIREMENTS");
    assert_eq!(
        *requirements,
        serde_json::json!({
            "replacement": "#requirements",
            "title": "REQUIREMENTS",
            "key": "REQUIREMENTS",
            "slug": "requirements",
            "route": "foo",
            "block_id": "bar",
            "text": "Upgrade Postgres",
            "line": 5,
            "entry_count": 2,
            "group": "task",
            "detail": "2 notes",
            "creates": false,
            "consequence": "Adds after \"fast disk\" in REQUIREMENTS",
            "cursor_after": draft.len() - 1 + "#requirements".len(),
        })
    );
    assert_eq!(row(&json, "task", "FUTURE WORK")["detail"], "empty");

    let motivation = row(&json, "recent", "MOTIVATION");
    assert_eq!(motivation["creates"], true);
    assert_eq!(motivation["note_count"], 3);
    assert_eq!(motivation["detail"], "used in 3 notes · today");
    assert_eq!(motivation["consequence"], "Creates MOTIVATION under ^bar");
    assert_eq!(motivation["replacement"], "#motivation");
    assert!(motivation["last_seen"].as_u64().unwrap() > 0);
    assert!(motivation.get("refused").is_none());
}

#[test]
fn typed_drawer_menu_filters_groups_and_drops_the_unselected_open() {
    let temp = TempDir::new("bob-cli-drawer-menu-typed-wire");
    let vault = drawer_vault(&temp);

    let json = complete(&temp, &vault, "note @foo+bar#perf");
    assert!(json.get("initial_selection").is_none());
    assert_eq!(json["query"], "perf");
    assert_eq!(shape(&json), ["recent:PERFORMANCE", "new:PERF"]);
    assert_eq!(
        row(&json, "recent", "PERFORMANCE")["detail"],
        "used in 1 note · today"
    );
    let new = row(&json, "new", "PERF");
    assert_eq!(new["creates"], true);
    assert_eq!(new["replacement"], "#perf");
    assert_eq!(new["consequence"], "Creates PERF under ^bar");

    // A near-miss of a popular key is a hint, not a refusal.
    let typo = complete(&temp, &vault, "note @foo+bar#motivaton");
    let new = row(&typo, "new", "MOTIVATON");
    assert_eq!(new["detail"], "did you mean MOTIVATION?");
    assert!(new.get("refused").is_none());
}

#[test]
fn quoted_drawer_literals_decode_the_query_and_cover_the_quotes() {
    let temp = TempDir::new("bob-cli-drawer-menu-quoted");
    let vault = drawer_vault(&temp);

    let draft = "note @foo+bar#\"FUTURE W";
    let json = complete(&temp, &vault, draft);
    assert_eq!(json["query"], "FUTURE W");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 13, "end": draft.len()})
    );
    assert_eq!(
        row(&json, "task", "FUTURE WORK")["replacement"],
        "#future-work"
    );

    // A title no slug can spell is offered as a quoted literal.
    let closed = complete(&temp, &vault, "note @foo+bar#\"Q&A\"");
    assert_eq!(closed["query"], "Q&A");
    let new = row(&closed, "new", "Q&A");
    assert_eq!(new["replacement"], "#\"Q&A\"");
    assert_eq!(new["cursor_after"], 13 + "#\"Q&A\"".len());
}

#[test]
fn refused_rows_name_the_commits_reason() {
    let temp = TempDir::new("bob-cli-drawer-menu-refused-wire");
    let vault = drawer_vault(&temp);

    let reserved = complete(&temp, &vault, "note @foo+bar#tasks");
    assert_eq!(
        row(&reserved, "new", "TASKS")["refused"],
        "`TASKS` is managed by Bob; name the drawer something else"
    );

    let near = complete(&temp, &vault, "note @foo+bar#req");
    assert_eq!(
        row(&near, "new", "REQ")["refused"],
        "did you mean `#requirements`? write `#\"REQ\"` to create REQ"
    );

    let ambiguous = complete(&temp, &vault, "note @foo+bar#notes");
    let notes: Vec<&Value> = rows(&ambiguous)
        .iter()
        .filter(|row| row["group"] == "task")
        .collect();
    assert_eq!(notes.len(), 2);
    for note in notes {
        assert_eq!(
            note["refused"],
            "drawer `NOTES` matches more than one drawer (lines 8, 9); rename one"
        );
    }
}

#[test]
fn accepting_a_row_makes_the_commit_agree_with_the_menu() {
    let temp = TempDir::new("bob-cli-drawer-menu-roundtrip");
    let vault = drawer_vault(&temp);

    for query in ["", "req", "motivaton", "perf", "tasks", "notes", "future"] {
        let draft = format!("Postgres 17 minimum @foo+bar#{query}");
        let menu = complete(&temp, &vault, &draft);
        for row in rows(&menu) {
            let replacement = row["replacement"].as_str().expect("replacement");
            let accepted = accept(&draft, &menu, replacement);
            let (ok, json) = dry_run(&vault, &accepted);
            let label = format!("{draft:?} → {accepted:?}: {row}");
            match row.get("refused") {
                Some(reason) => {
                    assert!(!ok, "{label}: {json}");
                    // The commit prefixes the item position to the very
                    // reason the menu shows.
                    let error = json["error"].as_str().expect("error");
                    assert!(
                        error.ends_with(reason.as_str().expect("reason")),
                        "{label}: {json}"
                    );
                    assert!(json["code"].is_string(), "{label}: {json}");
                }
                None => {
                    assert!(ok, "{label}: {json}");
                    assert_eq!(
                        json["drawer"]["created"], row["creates"],
                        "{label}: {json}"
                    );
                    assert_eq!(
                        json["drawer"]["key"], row["key"],
                        "{label}: {json}"
                    );
                    // The cursor lands just after the accepted selector.
                    let after = row["cursor_after"].as_u64().unwrap() as usize;
                    assert_eq!(
                        after,
                        draft.rfind('#').unwrap() + replacement.len()
                    );
                }
            }
        }
    }
}

#[test]
fn unresolvable_drawer_targets_return_a_diagnostic_and_no_rows() {
    let temp = TempDir::new("bob-cli-drawer-menu-diagnostics-wire");
    let vault = drawer_vault(&temp);
    write_file(
        &vault.join("dup.md"),
        concat!(
            "---\ntype: [[area]]\n---\n",
            "- [ ] #task One ^same\n",
            "- [ ] #task Two ^same\n",
            "- plain bullet ^notatask\n",
        ),
    );

    for (draft, warning) in [
        ("note @foo+nope#", "no task with block ID ^nope in foo.md"),
        ("note @missing+bar#", "note does not exist: missing.md"),
        (
            "note @dup+same#",
            "block ID ^same appears 2 times in dup.md",
        ),
        ("note @dup+notatask#", "^notatask in dup.md is not a task"),
        ("note @foo+nope#mo", "no task with block ID ^nope in foo.md"),
    ] {
        let json = complete(&temp, &vault, draft);
        assert_eq!(json["context"], "drawer", "{draft}");
        assert_eq!(rows(&json).len(), 0, "{draft}: {json}");
        assert_eq!(json["warnings"][0], warning, "{draft}");
    }
}

#[test]
fn drawer_menu_human_output_lists_bobs_detail_and_refusals() {
    let temp = TempDir::new("bob-cli-drawer-menu-human");
    let vault = drawer_vault(&temp);
    let draft = "note @foo+bar#tasks";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(draft.len().to_string())
        .arg("--")
        .arg(draft)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run human capture-complete");
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        text.contains("TASKS  refused: `TASKS` is managed by Bob"),
        "{text}"
    );

    let listing = "note @foo+bar#";
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(&vault)
        .arg("-c")
        .arg(listing.len().to_string())
        .arg("--")
        .arg(listing)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run human capture-complete");
    let text = stdout(&output);
    assert!(text.contains("REQUIREMENTS  2 notes  · task"), "{text}");
    assert!(
        text.contains("MOTIVATION  used in 3 notes · today  · recent"),
        "{text}"
    );
    assert!(!text.contains(" items"), "{text}");
}
