//! Combined plus-picker integration: a disposable fixture vault covering
//! the catalog boundary, both entry points, operator escape hatches, and
//! accept-then-capture round trips that the Mac client inserts verbatim.

use crate::support::*;
use std::fs;

const BOB_NOW: &str = "2026-09-30 09:02:00";

fn write_plus_picker_vault(vault: &std::path::Path) -> std::path::PathBuf {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
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
    write_file(
        &vault.join("mac_inbox.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Call the bank [created::2026-09-29]\n",
    );
    write_file(
        &vault.join("health.md"),
        "---\ntype: [[area]]\n---\n## Errands\n- [?] #task Book dentist [scheduled::2026-10-03]\n",
    );
    write_file(
        &vault.join("cash.md"),
        "---\ntype: [[area]]\n---\n## Tasks\n\
         - [*] #task Finish Google Exit Packet! ^goog-exit\n\
         - [ ] #task Review notes ^cash-review\n\
         - [ ] #task Café 日本語 ^cafe\n\
         - [ ] #task A very long task that should stay searchable in the plus picker even when the title wraps past the panel width\n\
         \t- [ ] #task Plan the handoff\n",
    );
    write_file(
        &vault.join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Polish capture picker ^polish\n",
    );
    write_file(
        &vault.join("sase.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n## Bugs\n\
         - [*] #task Fix deep bug ^deep-fix\n\
         - [ ] #task Review notes ^sase-review\n\
         - [/] #task Draft outline ^outline\n\
         - [x] #task Old fix ^old-fix\n\
         - [-] #task Dropped idea\n",
    );
    write_file(&vault.join("empty.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("archive.md"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Leftover ^leftover\n",
    );
    write_file(&vault.join("scratch.md"), "- [ ] #task Loose end ^loose\n");
    let day_file = vault.join("2026/20260930.md");
    write_file(
        &day_file,
        "## Pomodoros\n\
         - [ ] () — BUGS\n\t- [[sase#^deep-fix]]\n\
         - [ ] () — ADMIN\n\t- [[cash#^goog-exit]]\n",
    );
    day_file
}

fn complete_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    cursor: usize,
    text: &str,
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-a")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run plus-picker completion");
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
        .expect("run capture-parse");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|error| {
        panic!(
            "parse stdout should be JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn dry_run_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    text: &str,
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", BOB_NOW)
        .output()
        .expect("run capture dry-run");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON")
}

fn replacements(json: &serde_json::Value) -> Vec<&str> {
    json["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .map(|row| row["replacement"].as_str().expect("replacement"))
        .collect()
}

fn texts(json: &serde_json::Value) -> Vec<&str> {
    json["candidates"]
        .as_array()
        .expect("candidate array")
        .iter()
        .map(|row| row["text"].as_str().expect("text"))
        .collect()
}

fn accept(json: &serde_json::Value, draft: &str, replacement: &str) -> String {
    let start = json["replacement"]["start"].as_u64().unwrap() as usize;
    let end = json["replacement"]["end"].as_u64().unwrap() as usize;
    let mut accepted = draft.to_string();
    accepted.replace_range(start..end, replacement);
    accepted
}

#[test]
fn plus_picker_fixture_vault_catalog_and_entry_points() {
    let temp = TempDir::new("bob-cli-plus-picker-integration");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_plus_picker_vault(&vault);

    let vault_wide = complete_json(&vault, &day_file, 1, "+");
    assert_eq!(vault_wide["context"], "task_parent");
    assert_eq!(vault_wide["picker"]["scope"], "vault");
    assert_eq!(vault_wide["picker"]["scope_token"], "+");
    let catalog = texts(&vault_wide);
    assert!(
        !catalog.iter().any(|text| text.contains("Leftover") || text.contains("Loose end")),
        "vault-wide plus must not advertise non-capture notes: {catalog:?}"
    );
    assert_eq!(
        catalog
            .iter()
            .filter(|text| **text == "Review notes")
            .count(),
        2,
        "{catalog:?}"
    );
    assert!(catalog.iter().any(|text| text.contains("Café 日本語")));
    assert!(catalog
        .iter()
        .any(|text| text.starts_with("A very long task")));
    let repls = replacements(&vault_wide);
    assert!(repls.contains(&"@sase+deep-fix"), "{repls:?}");
    assert!(repls.contains(&"@cash+goog-exit"), "{repls:?}");
    assert!(repls.contains(&"@cash+cafe"), "{repls:?}");

    let scoped = complete_json(&vault, &day_file, 6, "@cash+");
    assert_eq!(scoped["context"], "task");
    assert_eq!(scoped["picker"]["kind"], "parent_task");
    assert_eq!(scoped["picker"]["scope"], "note");
    assert_eq!(scoped["picker"]["note_target"], "cash.md");
    let scoped_texts = texts(&scoped);
    assert!(scoped_texts.contains(&"Finish Google Exit Packet!"));
    assert!(scoped_texts
        .iter()
        .any(|text| text.contains("Plan the handoff")));
    assert!(!scoped_texts
        .iter()
        .any(|text| text.contains("Fix deep bug")));

    let missing = complete_json(&vault, &day_file, 7, "@ghost+");
    assert_eq!(missing["context"], "task");
    assert_eq!(missing["picker"]["note_target"], "ghost.md");
    assert!(missing["candidates"].as_array().unwrap().is_empty());

    let empty = complete_json(&vault, &day_file, 7, "@empty+");
    assert_eq!(empty["picker"]["note_target"], "empty.md");
    assert!(empty["candidates"].as_array().unwrap().is_empty());

    let filtered = complete_json(&vault, &day_file, 5, "+bank");
    assert_eq!(filtered["query"], "bank");
    assert_eq!(
        filtered["replacement"],
        serde_json::json!({"start": 0, "end": 5})
    );
    let full = complete_json(&vault, &day_file, 0, "+bank");
    assert_eq!(full["query"], "");
    assert_eq!(full["replacement"], filtered["replacement"]);
    assert!(
        full["candidates"].as_array().unwrap().len()
            > filtered["candidates"].as_array().unwrap().len()
    );
}

#[test]
fn plus_picker_accepts_insert_and_preserves_operators() {
    let temp = TempDir::new("bob-cli-plus-picker-walks");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let day_file = write_plus_picker_vault(&vault);

    let scoped = complete_json(&vault, &day_file, 6, "@cash+");
    let goog = scoped["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["block_id"] == "goog-exit")
        .expect("goog-exit");
    let scoped_draft =
        accept(&scoped, "@cash+", goog["replacement"].as_str().unwrap());
    assert_eq!(scoped_draft, "@cash+goog-exit");
    let scoped_capture = dry_run_json(&vault, &day_file, &scoped_draft);
    assert_eq!(scoped_capture["kind"], "task_toggle");
    assert_eq!(scoped_capture["toggle_behavior"], "ensure_next");

    let suffix = complete_json(&vault, &day_file, 7, "@cash+go#requirements");
    assert_eq!(suffix["context"], "task");
    let suffix_draft = accept(
        &suffix,
        "@cash+go#requirements",
        goog["replacement"].as_str().unwrap(),
    );
    assert_eq!(suffix_draft, "@cash+goog-exit#requirements");

    let prose = "Called the bank +";
    let prose_json = complete_json(&vault, &day_file, prose.len(), prose);
    assert_eq!(prose_json["context"], "task_parent");
    assert!(prose_json["picker"]["action_continuation_keys"].is_null());
    let prose_draft = accept(&prose_json, prose, "@cash+goog-exit");
    assert_eq!(prose_draft, "Called the bank @cash+goog-exit");
    let append = dry_run_json(&vault, &day_file, &prose_draft);
    assert_eq!(append["kind"], "sub_bullet");

    let leading = complete_json(&vault, &day_file, 1, "+");
    let ensure = accept(&leading, "+", "@sase+deep-fix");
    assert_eq!(ensure, "@sase+deep-fix");
    let ensure_capture = dry_run_json(&vault, &day_file, &ensure);
    assert_eq!(ensure_capture["kind"], "task_toggle");
    assert_eq!(ensure_capture["toggle_behavior"], "ensure_next");

    let lone_plus = parse_json("+");
    assert_eq!(lone_plus["mode"], "pomodoro_adjust");
    assert!(lone_plus["needs"].as_array().unwrap().is_empty());
    let unresolved = parse_json("Called the bank +");
    assert_eq!(unresolved["mode"], "incomplete");
    assert_eq!(unresolved["needs"][0], "task_parent");

    for operator in ["+2", "++", "++3", "+2 =x"] {
        let json = complete_json(&vault, &day_file, operator.len(), operator);
        assert!(
            json["context"].is_null(),
            "{operator} opened {:?}: {json}",
            json["context"]
        );
        assert!(json["picker"].is_null(), "{operator}");
    }
    let plus_two = parse_json("+2");
    assert_eq!(plus_two["mode"], "pomodoro_adjust");
    let shift = parse_json("++3");
    assert_eq!(shift["mode"], "pomodoro_shift");

    let later = "First item\n\nCalled the bank +";
    let later_json = complete_json(&vault, &day_file, later.len(), later);
    assert_eq!(later_json["context"], "task_parent");
    assert_eq!(
        later_json["replacement"],
        serde_json::json!({"start": 28, "end": 29})
    );
    let later_draft = accept(&later_json, later, "@cash+cafe");
    assert_eq!(later_draft, "First item\n\nCalled the bank @cash+cafe");

    let child = "Parent\n- note +";
    let child_json = complete_json(&vault, &day_file, child.len(), child);
    assert_eq!(child_json["context"], "task_parent");
    let child_draft = accept(&child_json, child, "@sase+outline");
    assert_eq!(child_draft, "Parent\n- note @sase+outline");
}
