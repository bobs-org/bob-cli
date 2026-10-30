//! `wikilink_task` (`((query`) completion: vault-wide Task Wikilink
//! snapshot, backend-formatted `[[target#^id]]` replacements, picker
//! descriptor, guards, and `capture-task-id`'s `wikilink_replacement`.
//!
//! The fixture vault pins `BOB_NOW` so previous-daily and ordering are
//! deterministic. Completion is read-only: every test snapshots the
//! vault and the focused tests assert it is untouched.

use crate::support::*;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn write_fixture(vault: &std::path::Path) {
    write_toggle_task_settings(vault);
    write_file(
        &vault.join("mac_inbox.md"),
        concat!(
            "- [ ] #task Inbox chore ^inbox-chore\n",
            "- [ ] #task Inbox plain\n",
            "- [ ] #task Inbox hidden #hide ^inbox-hidden\n",
        ),
    );
    write_file(
        &vault.join("home.md"),
        concat!(
            "# Home\n",
            "- [ ] #task Buy milk ^milk\n",
            "- [/] #task Cook dinner ^dinner\n",
            "- [ ] #task No id yet\n",
            "- [ ] #task Hidden chore #hide ^hidden-chore\n",
            "- [x] #task Done deed ^done-deed\n",
            "- [-] #task Cancelled chore ^cancelled-chore\n",
            "```\n",
            "- [ ] #task Fenced task ^fenced\n",
            "```\n",
            "Plain line ^plain-anchor\n",
        ),
    );
    write_file(
        &vault.join("cash.md"),
        concat!(
            "- [ ] #task Confirm grocery budget ^budget\n",
            "- [*] #task Plan meals ^meals\n",
            "- [?] #task Blocked bill ^bill\n",
        ),
    );
    write_file(
        &vault.join("projects/alpha.md"),
        concat!(
            "- [ ] #task Alpha task ^alpha\n",
            "- [ ] #task Alpha clone\n",
        ),
    );
    write_file(
        &vault.join("Shopping List.md"),
        concat!("- [ ] #task Buy eggs ^eggs\n", "- [ ] #task Buy cheese\n",),
    );
    write_file(&vault.join("a/dup.md"), "- [ ] #task Dup A ^dup-a\n");
    write_file(&vault.join("b/dup.md"), "- [ ] #task Dup B ^dup-b\n");
    write_file(
        &vault.join("twin.md"),
        concat!(
            "- [ ] #task First twin ^twin\n",
            "- [ ] #task Second twin ^twin\n",
        ),
    );
    write_file(
        &vault.join("_templates/t.md"),
        "- [ ] #task Tmpl task ^tmpl\n",
    );
    write_file(
        &vault.join(".hidden/h.md"),
        "- [ ] #task Hidden note task ^hid\n",
    );
    write_file(
        &vault.join("done/old.md"),
        concat!("- [x] #task Old done\n", "- [x] #task Old kept ^kept\n",),
    );
    write_file(
        &vault.join("2026/20260930.md"),
        concat!(
            "- [ ] #task Today task ^today-task\n",
            "- [ ] #task Today plain\n",
        ),
    );
    write_file(
        &vault.join("2026/20260929.md"),
        concat!(
            "- [ ] #task Yesterday task\n",
            "- [ ] #task Yesterday kept ^yest-kept\n",
        ),
    );
    write_file(
        &vault.join("ref/papers/paper.md"),
        concat!(
            "---\n",
            "type: \"[[ref]]\"\n",
            "---\n",
            "- [ ] #task Read paper ^paper-read\n",
        ),
    );
    write_file(
        &vault.join("locked.md"),
        concat!(
            "- [ ] #task Locked plain\n",
            "- [ ] #task Locked kept ^locked-kept\n",
        ),
    );
}

fn complete_json(
    vault: &std::path::Path,
    cursor: usize,
    text: &str,
) -> serde_json::Value {
    complete_json_with(vault, cursor, text, false)
}

fn complete_json_with(
    vault: &std::path::Path,
    cursor: usize,
    text: &str,
    all_tasks: bool,
) -> serde_json::Value {
    let mut command = bob_command();
    command
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json");
    if all_tasks {
        command.arg("-a");
    }
    let output = command
        .arg("--")
        .arg(text)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run wikilink_task completion");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "completion stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn snapshot(vault: &std::path::Path) -> HashMap<String, String> {
    let mut files = HashMap::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut entries: Vec<_> =
            fs::read_dir(&directory).expect("read dir").collect();
        entries
            .sort_by_key(|entry| entry.as_ref().map(|entry| entry.path()).ok());
        for entry in entries {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if entry.file_type().expect("file type").is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(vault) {
                files.insert(
                    relative.to_string_lossy().replace('\\', "/"),
                    fs::read_to_string(&path).unwrap_or_default(),
                );
            }
        }
    }
    files
}

fn candidate_texts(json: &serde_json::Value) -> Vec<String> {
    json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .map(|candidate| candidate["text"].as_str().expect("text").to_string())
        .collect()
}

fn candidate_by_text(
    json: &serde_json::Value,
    text: &str,
) -> serde_json::Value {
    json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .find(|candidate| candidate["text"].as_str() == Some(text))
        .unwrap_or_else(|| panic!("missing candidate {text:?}"))
        .clone()
}

fn first_identified(json: &serde_json::Value) -> &serde_json::Value {
    json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .find(|candidate| {
            candidate["replacement"]
                .as_str()
                .is_some_and(|replacement| !replacement.is_empty())
        })
        .expect("identified candidate")
}

fn splice(draft: &str, json: &serde_json::Value, replacement: &str) -> String {
    let start = json["replacement"]["start"].as_u64().expect("start") as usize;
    let end = json["replacement"]["end"].as_u64().expect("end") as usize;
    format!("{}{}{}", &draft[..start], replacement, &draft[end..])
}

fn parse_json(text: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "parse stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn diagnostic_codes(json: &serde_json::Value) -> Vec<String> {
    json.get("diagnostics")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|diagnostic| {
                    diagnostic["code"].as_str().unwrap_or_default().to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn dry_run_json(vault: &std::path::Path, text: &str) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run capture dry-run");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "dry-run stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn task_id_json(
    vault: &std::path::Path,
    args: &[&str],
) -> (std::process::Output, serde_json::Value) {
    let output = bob_command()
        .arg("capture-task-id")
        .arg("-b")
        .arg(vault)
        .args(args)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run capture-task-id");
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!(
                "task-id stdout should be JSON: {error}\n{}",
                format_output(&output)
            )
        });
    (output, json)
}

#[test]
fn empty_query_orders_this_note_lanes_and_history() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-order");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);
    let before = snapshot(&vault);

    let text = "Follow up ((";
    let json = complete_json(&vault, text.len(), text);
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["context"], "wikilink_task");
    assert_eq!(json["query"], "");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 10, "end": 12})
    );
    assert_eq!(json["picker"]["kind"], "wikilink_task");
    assert_eq!(json["picker"]["scope"], "vault");
    assert_eq!(json["picker"]["scope_token"], "((");
    assert_eq!(
        json["picker"]["marker_range"],
        serde_json::json!({"start": 10, "end": 12})
    );
    assert_eq!(
        json["picker"]["trigger_removal_range"],
        serde_json::json!({"start": 11, "end": 12})
    );

    let texts = candidate_texts(&json);
    assert_eq!(texts[0], "Inbox chore");
    assert_eq!(texts[1], "Inbox plain");
    let inbox_hidden = texts
        .iter()
        .position(|text| text == "Inbox hidden #hide")
        .expect("hidden inbox");
    assert!(inbox_hidden > 1, "{texts:?}");
    let dinner = texts.iter().position(|text| text == "Cook dinner").unwrap();
    let meals = texts.iter().position(|text| text == "Plan meals").unwrap();
    let milk = texts.iter().position(|text| text == "Buy milk").unwrap();
    assert!(dinner < meals, "{texts:?}");
    assert!(meals < milk, "{texts:?}");
    let first_completed = texts
        .iter()
        .position(|text| {
            ["Done deed", "Cancelled chore", "Old done", "Old kept"]
                .contains(&text.as_str())
        })
        .expect("completed history");
    assert!(
        texts[..first_completed].iter().all(|text| ![
            "Done deed",
            "Cancelled chore",
            "Old done",
            "Old kept"
        ]
        .contains(&text.as_str())),
        "{texts:?}"
    );
    let hidden_home = texts
        .iter()
        .position(|text| text == "Hidden chore #hide")
        .unwrap();
    assert!(milk < hidden_home, "{texts:?}");
    for absent in ["Tmpl task", "Hidden note task", "Fenced task"] {
        assert!(!texts.contains(&absent.to_string()), "{texts:?}");
    }
    assert!(texts.contains(&"Read paper".to_string()), "{texts:?}");
    assert_eq!(snapshot(&vault), before, "completion writes nothing");
}

#[test]
fn rows_carry_link_targets_groups_and_guards() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-rows");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let text = "Follow up ((";
    let json = complete_json(&vault, text.len(), text);

    let inbox = candidate_by_text(&json, "Inbox chore");
    assert_eq!(inbox["group"], "in_this_note");
    assert_eq!(inbox["replacement"], "[[mac_inbox#^inbox-chore]]");
    assert_eq!(inbox["link_target"], "mac_inbox");
    assert_eq!(
        inbox["cursor_after"],
        10 + "[[mac_inbox#^inbox-chore]]".len()
    );
    assert_eq!(inbox["requires_block_id"], false);

    let dinner = candidate_by_text(&json, "Cook dinner");
    assert_eq!(dinner["group"], "in_progress");
    assert_eq!(dinner["replacement"], "[[home#^dinner]]");
    assert_eq!(dinner["link_target"], "home");

    let meals = candidate_by_text(&json, "Plan meals");
    assert_eq!(meals["group"], "next");
    assert_eq!(meals["replacement"], "[[cash#^meals]]");

    let milk = candidate_by_text(&json, "Buy milk");
    assert_eq!(milk["group"], "open");
    assert_eq!(milk["replacement"], "[[home#^milk]]");
    assert_eq!(milk["link_target"], "home");
    assert_eq!(milk["note_path"], "home.md");

    let eggs = candidate_by_text(&json, "Buy eggs");
    assert_eq!(eggs["replacement"], "[[Shopping List#^eggs]]");
    assert_eq!(eggs["link_target"], "Shopping List");

    let dup_a = candidate_by_text(&json, "Dup A");
    assert_eq!(dup_a["note_path"], "a/dup.md");
    assert_eq!(dup_a["replacement"], "[[a/dup#^dup-a]]");
    assert_eq!(dup_a["link_target"], "a/dup");

    let alpha = candidate_by_text(&json, "Alpha task");
    assert_eq!(alpha["replacement"], "[[alpha#^alpha]]");
    assert_eq!(alpha["link_target"], "alpha");

    let kept = candidate_by_text(&json, "Old kept");
    assert_eq!(kept["group"], "completed");
    assert_eq!(kept["replacement"], "[[done/old#^kept]]");
    assert_eq!(kept["link_target"], "done/old");

    let paper = candidate_by_text(&json, "Read paper");
    assert_eq!(paper["note_path"], "ref/papers/paper.md");
    assert_eq!(paper["replacement"], "[[paper#^paper-read]]");
    assert_eq!(paper["link_target"], "paper");

    let clone = candidate_by_text(&json, "Alpha clone");
    assert_eq!(clone["replacement"], "");
    assert_eq!(clone["requires_block_id"], true);
    assert!(clone.get("cursor_after").is_none(), "{clone}");
    assert!(
        !clone["block_id_suggestions"]
            .as_array()
            .expect("suggestions")
            .is_empty(),
        "{clone}"
    );

    let old_done = candidate_by_text(&json, "Old done");
    assert_eq!(old_done["replacement"], "");
    assert_eq!(
        old_done["disabled_reason"],
        "archived tasks can't get an id"
    );

    let yesterday = candidate_by_text(&json, "Yesterday task");
    assert_eq!(
        yesterday["disabled_reason"],
        "the previous daily note is read-only"
    );
    let yest_kept = candidate_by_text(&json, "Yesterday kept");
    assert_eq!(yest_kept["replacement"], "[[20260929#^yest-kept]]");
    assert_eq!(yest_kept["link_target"], "20260929");
    assert!(yest_kept.get("disabled_reason").is_none(), "{yest_kept}");

    let twin = candidate_by_text(&json, "First twin");
    assert_eq!(twin["replacement"], "");
    assert!(
        twin["disabled_reason"]
            .as_str()
            .expect("reason")
            .contains("duplicate block ID"),
        "{twin}"
    );

    let hidden = candidate_by_text(&json, "Hidden chore #hide");
    assert_eq!(hidden["hidden"], true);
    assert_eq!(hidden["group"], "open");
}

#[test]
fn triggers_ranges_and_non_triggers() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-triggers");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let line_start = "((";
    let json = complete_json(&vault, line_start.len(), line_start);
    assert_eq!(json["context"], "wikilink_task");
    assert_eq!(json["query"], "");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 0, "end": 2})
    );

    let after_space = "Follow up ((s";
    let json = complete_json(&vault, after_space.len(), after_space);
    assert_eq!(json["context"], "wikilink_task");
    assert_eq!(json["query"], "s");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 10, "end": 13})
    );
    assert_eq!(
        json["picker"]["trigger_removal_range"],
        serde_json::json!({"start": 11, "end": 13})
    );

    let closed = "Follow up ((foo)) more";
    let opener = closed.find("((").expect("opener");
    let json = complete_json(&vault, opener + 4, closed);
    assert_eq!(json["context"], "wikilink_task");
    assert_eq!(json["query"], "fo");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": opener, "end": opener + 7})
    );
    let at_start = complete_json(&vault, opener, closed);
    assert_eq!(at_start["query"], "");
    assert_eq!(at_start["replacement"], json["replacement"]);
    let after_closer = complete_json(&vault, opener + 7, closed);
    assert_eq!(after_closer["query"], "foo");
    assert_eq!(after_closer["replacement"], json["replacement"]);
    let beyond_closer = complete_json(&vault, opener + 8, closed);
    assert_ne!(beyond_closer["context"], "wikilink_task");

    let mid = "Follow up ((foobar baz";
    let mid_opener = mid.find("((").expect("opener");
    let json = complete_json(&vault, mid_opener + 5, mid);
    assert_eq!(json["query"], "foo");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": mid_opener, "end": mid_opener + 8})
    );

    let crlf = "Follow up ((s\r\nnext";
    let json = complete_json(&vault, 13, crlf);
    assert_eq!(json["query"], "s");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 10, "end": 13})
    );

    let cafe = "Café ((s";
    let cafe_opener = cafe.find("((").expect("opener");
    let json = complete_json(&vault, cafe.len(), cafe);
    assert_eq!(json["query"], "s");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": cafe_opener, "end": cafe.len()})
    );

    let bulk = "First item\n\nFollow up ((s";
    let json = complete_json(&vault, bulk.len(), bulk);
    assert_eq!(json["context"], "wikilink_task");
    let bulk_opener = bulk.find("((").expect("opener");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": bulk_opener, "end": bulk.len()})
    );

    let at_query = "((@foo";
    let json = complete_json(&vault, at_query.len(), at_query);
    assert_eq!(json["context"], "wikilink_task");
    assert_eq!(json["query"], "@foo");

    let after_link = "Do work @sase:id ((s";
    let json = complete_json(&vault, after_link.len(), after_link);
    assert_eq!(json["context"], "wikilink_task");

    let punctuation_suffix = "Follow up ((foo and [existing](url)";
    let json = complete_json(&vault, 15, punctuation_suffix);
    assert_eq!(json["context"], "wikilink_task");
    assert_eq!(json["query"], "foo");
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 10, "end": 15})
    );
    for draft in ["Follow up ((foo) bar", "Follow up ((foo`code`)"] {
        let json = complete_json(&vault, 15, draft);
        assert_eq!(json["context"], "wikilink_task", "{draft:?}: {json}");
        assert_eq!(json["query"], "foo");
        assert_eq!(json["replacement"]["end"], 15);
    }

    for (raw, cursor) in [
        ("f((x", 4usize),
        ("(((foo", 6),
        ("See [[note ((x", 14),
        ("Follow up ((fo(o", 16),
        ("=", 1),
        ("x", 1),
        ("+5", 2),
        ("--", 2),
        ("!", 1),
        (":", 1),
        ("/", 1),
        ("^sase:id", 8),
    ] {
        let json = complete_json(&vault, cursor, raw);
        assert_ne!(
            json["context"], "wikilink_task",
            "non-trigger {raw:?} at {cursor}: {json}"
        );
    }

    let work_log = "x\n- Follow up ((s";
    let json = complete_json(&vault, work_log.len(), work_log);
    assert_ne!(
        json["context"], "wikilink_task",
        "work log should not be a field: {json}"
    );
}

#[test]
fn all_tasks_does_not_change_the_pool() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-all-tasks");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let text = "Follow up ((";
    let without = complete_json_with(&vault, text.len(), text, false);
    let with = complete_json_with(&vault, text.len(), text, true);
    assert_eq!(
        without["candidates"].as_array().expect("array").len(),
        with["candidates"].as_array().expect("array").len()
    );
}

#[test]
fn query_ranks_open_matches_ahead_of_history() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-rank");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let text = "Follow up ((meals";
    let json = complete_json(&vault, text.len(), text);
    assert_eq!(json["query"], "meals");
    assert_eq!(candidate_texts(&json), vec!["Plan meals"]);

    let text = "Follow up ((kept";
    let json = complete_json(&vault, text.len(), text);
    let texts = candidate_texts(&json);
    assert!(texts.contains(&"Locked kept".to_string()), "{texts:?}");
    assert!(texts.contains(&"Old kept".to_string()), "{texts:?}");
    assert!(texts.contains(&"Yesterday kept".to_string()), "{texts:?}");
    let locked = texts.iter().position(|text| text == "Locked kept").unwrap();
    let old = texts.iter().position(|text| text == "Old kept").unwrap();
    assert!(locked < old, "open matches precede completed: {texts:?}");

    let text = "Follow up ((done-deed";
    let json = complete_json(&vault, text.len(), text);
    assert_eq!(candidate_texts(&json), vec!["Done deed"]);
    assert_eq!(json["candidates"][0]["group"], "completed");
}

#[test]
fn read_only_note_guards_id_less_rows_and_stays_untouched() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-readonly");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);
    let locked = vault.join("locked.md");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o444))
        .expect("lock note");
    let before = snapshot(&vault);

    let text = "Follow up ((";
    let json = complete_json(&vault, text.len(), text);
    let plain = candidate_by_text(&json, "Locked plain");
    assert_eq!(plain["replacement"], "");
    assert_eq!(plain["disabled_reason"], "read-only note");
    let kept = candidate_by_text(&json, "Locked kept");
    assert_eq!(kept["replacement"], "[[locked#^locked-kept]]");
    assert!(kept.get("disabled_reason").is_none(), "{kept}");
    assert_eq!(snapshot(&vault), before, "read-only vault stays untouched");

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644))
        .expect("unlock note");
}

#[test]
fn spliced_identified_link_stays_valid_capture() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-splice");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let drafts: Vec<(String, usize)> = vec![
        ("((".to_string(), 2),
        ("Follow up ((".to_string(), 12),
        ("Follow up ((s".to_string(), 13),
        ("Follow up ((foo)) more".to_string(), 10),
        ("Follow up ((foobar baz".to_string(), 15),
        ("Follow up ((foo and [existing](url)".to_string(), 15),
        ("Follow up ((s\r\nnext".to_string(), 13),
        ("Café ((s".to_string(), "Café ((s".len()),
        (
            "First item\n\nFollow up ((s".to_string(),
            "First item\n\nFollow up ((s".len(),
        ),
    ];
    for (draft, cursor) in drafts {
        let json = complete_json(&vault, cursor, &draft);
        assert_eq!(
            json["context"], "wikilink_task",
            "offered {draft:?} at {cursor}"
        );
        let snapshot = if json["candidates"].as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row["replacement"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            })
        }) {
            json.clone()
        } else {
            let opener = draft.find("((").expect("opener");
            complete_json(&vault, opener, &draft)
        };
        let identified = first_identified(&snapshot);
        let replacement = identified["replacement"].as_str().expect("link");
        let cursor_after = identified["cursor_after"].as_u64().expect("cursor");
        let start = json["replacement"]["start"].as_u64().expect("start");
        assert_eq!(
            cursor_after,
            start + replacement.len() as u64,
            "{draft:?} {replacement}"
        );
        let spliced = splice(&draft, &json, replacement);
        if draft.contains("[existing](url)") {
            let end =
                json["replacement"]["end"].as_u64().expect("end") as usize;
            assert!(
                spliced.ends_with(&draft[end..]),
                "suffix bytes were lost: {draft:?} -> {spliced:?}"
            );
            let captured = dry_run_json(&vault, &spliced);
            assert_eq!(
                captured["ok"], true,
                "dry-run refused suffix-preserving splice {spliced:?}: {captured}"
            );
        }
        let before_codes = diagnostic_codes(&parse_json(&draft));
        let after = parse_json(&spliced);
        let after_codes = diagnostic_codes(&after);
        for code in &after_codes {
            assert!(
                before_codes.contains(code),
                "new diagnostic {code} after splicing {draft:?} -> {spliced:?}\n{after}"
            );
        }
        let original_capture = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--dry-run")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(&draft)
            .env("BOB_NOW", BOB_NOW)
            .output()
            .expect("run original dry-run");
        if original_capture.status.success() {
            let captured = dry_run_json(&vault, &spliced);
            assert_eq!(
                captured["ok"], true,
                "dry-run refused {spliced:?}: {captured}"
            );
        }
    }
}

#[test]
fn capture_task_id_reports_wikilink_replacement_in_both_modes() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-id");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let json = complete_json(&vault, 12, "Follow up ((");
    let clone = candidate_by_text(&json, "Alpha clone");
    let task_ref = clone["ref"].as_str().expect("ref");
    let block_id = clone["block_id_suggestions"][0]
        .as_str()
        .expect("suggestion");

    let (output, assigned) = task_id_json(
        &vault,
        &[
            "-n",
            "projects/alpha.md",
            "-t",
            task_ref,
            "-i",
            block_id,
            "-f",
            "json",
            "-d",
        ],
    );
    assert_success(&output);
    assert_eq!(assigned["ok"], true);
    assert_eq!(
        assigned["wikilink_replacement"],
        format!("[[alpha#^{block_id}]]"),
        "note-path unique basename"
    );

    let cheese = candidate_by_text(&json, "Buy cheese");
    let cheese_ref = cheese["ref"].as_str().expect("ref");
    let cheese_id = cheese["block_id_suggestions"][0]
        .as_str()
        .expect("suggestion");
    let (output, assigned) = task_id_json(
        &vault,
        &[
            "-n",
            "Shopping List.md",
            "-t",
            cheese_ref,
            "-i",
            cheese_id,
            "-f",
            "json",
            "-d",
        ],
    );
    assert_success(&output);
    assert_eq!(
        assigned["wikilink_replacement"],
        format!("[[Shopping List#^{cheese_id}]]"),
        "note-path spaced basename"
    );

    let inbox_plain = candidate_by_text(&json, "Inbox plain");
    let inbox_ref = inbox_plain["ref"].as_str().expect("ref");
    let inbox_id = inbox_plain["block_id_suggestions"][0]
        .as_str()
        .expect("suggestion");
    let (output, assigned) = task_id_json(
        &vault,
        &[
            "-r",
            "mac_inbox",
            "-t",
            inbox_ref,
            "-i",
            inbox_id,
            "-f",
            "json",
            "-d",
        ],
    );
    assert_success(&output);
    assert_eq!(
        assigned["wikilink_replacement"],
        format!("[[mac_inbox#^{inbox_id}]]"),
        "route mode"
    );

    let old_done = candidate_by_text(&json, "Old done");
    let old_ref = old_done["ref"].as_str().expect("ref");
    let (output, assigned) = task_id_json(
        &vault,
        &[
            "-n",
            "done/old.md",
            "-t",
            old_ref,
            "-i",
            "old-done",
            "-f",
            "json",
            "--allow-closed",
            "-d",
        ],
    );
    assert_success(&output);
    assert_eq!(
        assigned["wikilink_replacement"], "[[done/old#^old-done]]",
        "archive always keeps the path"
    );
}

#[test]
fn shell_completion_returns_nothing() {
    let temp = TempDir::new("bob-cli-complete-wikilink-task-shell");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    let output = bob_command()
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg("1")
        .arg("--")
        .arg("bob")
        .arg("capture")
        .arg("((s")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run bob __complete");
    assert_success(&output);
    assert!(
        stdout(&output).trim().is_empty(),
        "shell completion stays out of wikilink_task: {}",
        format_output(&output)
    );
}

#[test]
fn wikilink_task_coexists_with_trailing_template_markers() {
    let temp = TempDir::new("bob-cli-wikilink-task-template-coexist");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    write_fixture(&vault);

    // Trailing `@@@` on the same line: the `((` token must keep its parse
    // spans and completion contract, and the rewrite must leave byte ranges
    // intact.
    let draft = "Follow up ((s @@@";
    let parsed = parse_json(draft);
    let spans = parsed["spans"].as_array().expect("spans array");
    assert!(
        spans
            .iter()
            .any(|span| span["kind"] == "wikilink_task_delimiter"),
        "trailing @@@ hid (( spans: {parsed}"
    );
    let json = complete_json(&vault, 13, draft);
    assert_eq!(json["context"], "wikilink_task", "{draft:?}: {json}");
    assert_eq!(json["replacement"]["start"], 10);
    assert_eq!(json["replacement"]["end"], 13);
    let identified = first_identified(&json);
    let replacement = identified["replacement"].as_str().expect("link");
    let spliced = splice(draft, &json, replacement);
    assert!(
        spliced.contains("@@@"),
        "splice lost trailing template marker: {spliced:?}"
    );
    let rewrite_output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg("13")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run capture-rewrite");
    assert_success(&rewrite_output);
    let rewrite: serde_json::Value =
        serde_json::from_str(stdout(&rewrite_output).trim())
            .expect("rewrite JSON");
    assert_eq!(rewrite["input"], draft);

    // `@@template:` rows mask `((` tokens: inside a template row there are no
    // `wikilink_task_*` spans, so the completion gate stays empty. This is the
    // coexistence contract: template rows never leak `((` fields.
    let template_draft = "@@template:\nTask {0}\n\nFollow up ((s";
    let template_parsed = parse_json(template_draft);
    let template_spans = template_parsed["spans"]
        .as_array()
        .expect("template spans array");
    assert!(
        !template_spans
            .iter()
            .any(|span| span["kind"] == "wikilink_task_delimiter"),
        "template row leaked (( spans: {template_parsed}"
    );
}
