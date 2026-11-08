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

    assert_eq!(json["grammar"], 18);
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

// ---------------------------------------------------------------------------
// The drawer menu at a first-level bullet's leading `#`.
// ---------------------------------------------------------------------------

fn complete_at(
    temp: &TempDir,
    vault: &Path,
    draft: &str,
    cursor: usize,
) -> Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
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

fn write_prj_note(vault: &Path) {
    write_file(
        &vault.join("proj.md"),
        concat!(
            "---\ntype: \"[[project]]\"\n---\n\n",
            "# Project\n\n",
            "- [ ] #task #prj Ship it ^prj\n\n",
            "## Requirements\n\n",
            "- first\n\n",
            "## Pomodoros\n\n",
            "- [ ] (0900-0930) Work\n",
        ),
    );
}

#[test]
fn bullet_menu_json_pins_the_target_the_bare_titles_and_the_continuation() {
    let temp = TempDir::new("bob-cli-bullet-menu-wire");
    let vault = drawer_vault(&temp);
    let draft = "@foo+bar\n- #";
    let json = complete(&temp, &vault, draft);

    assert_eq!(json["grammar"], 18);
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
            "replacement": "REQUIREMENTS\n  - ",
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
            "cursor_after": draft.len() - 1 + "REQUIREMENTS\n  - ".len(),
        })
    );
    assert_eq!(
        row(&json, "recent", "MOTIVATION")["consequence"],
        "Creates MOTIVATION under ^bar"
    );
}

#[test]
fn bullet_menu_continuation_depends_on_what_follows_the_token() {
    let temp = TempDir::new("bob-cli-bullet-menu-continuation-wire");
    let vault = drawer_vault(&temp);

    let end = "@foo+bar\n- #req";
    let json = complete(&temp, &vault, end);
    assert_eq!(
        row(&json, "task", "REQUIREMENTS")["replacement"],
        "REQUIREMENTS\n  - "
    );

    let nested = "@foo+bar\n- #req\n  - already";
    let json = complete_at(&temp, &vault, nested, end.len());
    let requirements = row(&json, "task", "REQUIREMENTS");
    assert_eq!(requirements["replacement"], "REQUIREMENTS");
    assert_eq!(
        requirements["cursor_after"],
        "@foo+bar\n- ".len() + "REQUIREMENTS".len()
    );
}

#[test]
fn bullet_menu_on_new_tasks_and_project_notes_carries_the_new_target() {
    let temp = TempDir::new("bob-cli-bullet-menu-new-wire");
    let vault = drawer_vault(&temp);

    let task = complete(&temp, &vault, "Ship it @foo\n- #");
    assert_eq!(
        task["target"],
        serde_json::json!({
            "route": "foo.md",
            "block_id": null,
            "text": "Ship it",
        })
    );
    assert_eq!(shape(&task), ["recent:MOTIVATION", "recent:ACCEPTANCE"]);
    assert_eq!(
        row(&task, "recent", "MOTIVATION")["consequence"],
        "Starts MOTIVATION on the new task"
    );
    let inbox = complete(&temp, &vault, "Ship it\n- #");
    assert_eq!(inbox["target"]["route"], "mac_inbox.md");

    let note = complete(&temp, &vault, "@foo^fresh@ Plan\n- #");
    assert_eq!(
        note["target"],
        serde_json::json!({
            "route": "foo_fresh.md",
            "block_id": "prj",
            "text": "Plan",
        })
    );
    assert_eq!(shape(&note), ["recent:Motivation", "recent:Acceptance"]);
    let motivation = row(&note, "recent", "Motivation");
    assert_eq!(motivation["replacement"], "MOTIVATION\n  - ");
    assert_eq!(motivation["consequence"], "Creates ## Motivation");

    write_prj_note(&vault);
    let prj = complete(&temp, &vault, "@proj+prj\n- #");
    assert_eq!(prj["target"]["block_id"], "prj");
    assert_eq!(
        row(&prj, "task", "Requirements")["replacement"],
        "REQUIREMENTS\n  - "
    );
    let structural = complete(&temp, &vault, "@proj+prj\n- #pomodoros");
    assert_eq!(
        row(&structural, "new", "POMODOROS")["refused"],
        "use `@proj#<Heading>` to capture into a project `##` section"
    );
}

#[test]
fn bullet_starts_that_cannot_name_a_drawer_are_empty_successes() {
    let temp = TempDir::new("bob-cli-bullet-menu-positions-wire");
    let vault = drawer_vault(&temp);
    for draft in [
        "@foo+bar fix it\n- #",
        "@foo+bar#requirements\n- #",
        "%n\n- #",
        "x\n- #",
        "Ship it\n- \\#",
    ] {
        let json = complete(&temp, &vault, draft);
        assert_eq!(json["context"], Value::Null, "{draft:?}: {json}");
        assert_eq!(rows(&json).len(), 0, "{draft:?}: {json}");
    }

    // A nested `  - #` is still a task-type position.
    let nested = complete(&temp, &vault, "Ship it\n- check\n  - #");
    assert_eq!(nested["context"], "task_type");
}

#[test]
fn accepting_a_bullet_row_makes_the_commit_agree_with_the_menu() {
    let temp = TempDir::new("bob-cli-bullet-menu-roundtrip");
    let vault = drawer_vault(&temp);
    write_prj_note(&vault);

    for (prefix, scope) in [
        ("@foo+bar\n- #", "task"),
        ("@proj+prj\n- #", "section"),
        ("Ship it\n- #", "new task"),
        ("@foo^fresh@ Plan\n- #", "new project note"),
    ] {
        for query in ["", "req", "motiv", "perf", "tasks", "notes", "future"] {
            let draft = format!("{prefix}{query}");
            let menu = complete(&temp, &vault, &draft);
            for row in rows(&menu) {
                let replacement =
                    row["replacement"].as_str().expect("replacement");
                // The accepted draft ends on the fresh nested bullet, so
                // the user's first note is the only thing left to type.
                assert!(replacement.ends_with("\n  - "), "{row}");
                let accepted =
                    format!("{}note", accept(&draft, &menu, replacement));
                let (ok, json) = dry_run(&vault, &accepted);
                let label = format!("{scope} {draft:?} → {accepted:?}: {row}");
                if let Some(reason) = row.get("refused") {
                    assert!(!ok, "{label}: {json}");
                    let error = json["error"].as_str().expect("error");
                    assert!(
                        error.ends_with(reason.as_str().expect("reason")),
                        "{label}: {json}"
                    );
                    continue;
                }
                assert!(ok, "{label}: {json}");
                let title = row["key"].as_str().expect("key");
                match scope {
                    "task" | "section" => assert_eq!(
                        json["drawer"]["created"], row["creates"],
                        "{label}: {json}"
                    ),
                    "new task" => assert_eq!(
                        json["sub_bullets"][0],
                        format!("\t- {title}"),
                        "{label}: {json}"
                    ),
                    // The template's own `## Tasks` is not a new section.
                    _ if row["creates"] == false => assert_eq!(
                        json["project_note"]["sections"],
                        serde_json::json!([]),
                        "{label}: {json}"
                    ),
                    _ => assert_eq!(
                        json["project_note"]["sections"][0]
                            .as_str()
                            .map(str::to_uppercase),
                        Some(title.to_string()),
                        "{label}: {json}"
                    ),
                }
                if scope == "task" || scope == "section" {
                    assert_eq!(json["drawer"]["key"], row["key"], "{label}");
                }
                // The caret lands past the title and the opened bullet.
                let after = row["cursor_after"].as_u64().unwrap() as usize;
                assert_eq!(
                    after,
                    draft.rfind('#').unwrap() + replacement.len(),
                    "{label}"
                );
            }
        }
    }
}

#[test]
fn bullet_menu_human_output_shows_the_title_line_only() {
    let temp = TempDir::new("bob-cli-bullet-menu-human");
    let vault = drawer_vault(&temp);
    let draft = "@foo+bar\n- #";
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
    assert!(text.contains("REQUIREMENTS  2 notes  · task"), "{text}");
    assert!(!text.contains("\n  - "), "{text}");
}

#[test]
fn run_members_complete_and_bullet_menu_keeps_first_members_drawers() {
    let temp = TempDir::new("bob-cli-run-complete-wire");
    let vault = drawer_vault(&temp);

    // Any run member completes through its own route and block ID: the
    // second member's drawer fragment offers that member's drawers.
    let draft = "@foo+bar @foo+plain#";
    let json = complete(&temp, &vault, draft);
    assert_eq!(json["context"], "drawer");

    // The bullet drawer menu below a note-less run keeps offering the first
    // member's drawers, matching the singleton `@foo+bar` menu (titles and
    // target match; `cursor_after` shifts with the longer run draft).
    let single = complete(&temp, &vault, "@foo+bar\n- #");
    let run = complete(&temp, &vault, "@foo+bar @foo+plain\n- #");
    assert_eq!(run["context"], "drawer");
    assert_eq!(run["target"], single["target"]);
    assert_eq!(shape(&run), shape(&single));
}

#[test]
fn target_run_completion_crosses_schedule_and_clipboard_tokens() {
    let temp = TempDir::new("bob-cli-run-completion-transparent-markers");
    let vault = drawer_vault(&temp);

    let task_draft = "@foo+bar s:3 @foo+pl note";
    let task_cursor = task_draft.find("@foo+pl").unwrap() + "@foo+pl".len();
    let task = complete_at(&temp, &vault, task_draft, task_cursor);
    assert_eq!(task["context"], "task", "{task}");
    assert_eq!(
        task["replacement"],
        serde_json::json!({
            "start": task_draft.find("@foo+pl").unwrap() + "@foo+".len(),
            "end": task_cursor,
        }),
        "{task}"
    );
    assert!(
        task["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["block_id"] == "plain"),
        "the second member owns its task candidates: {task}"
    );

    let drawer_draft = "@foo+plain %c @foo+bar#FUT note";
    let drawer_cursor = drawer_draft.find("#FUT").unwrap() + "#FUT".len();
    let drawer = complete_at(&temp, &vault, drawer_draft, drawer_cursor);
    assert_eq!(drawer["context"], "drawer", "{drawer}");
    assert_eq!(
        drawer["replacement"],
        serde_json::json!({
            "start": drawer_draft.find("#FUT").unwrap(),
            "end": drawer_cursor,
        }),
        "{drawer}"
    );
    assert!(
        drawer["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["title"] == "FUTURE WORK"),
        "the selected member owns its drawer candidates: {drawer}"
    );
}
