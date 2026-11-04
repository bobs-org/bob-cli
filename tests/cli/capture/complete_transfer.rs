//! `bob capture-complete` on `>` transfer lines: sources narrow to
//! movable-or-mergeable tasks, destinations list routes with the `+` hint,
//! `@c+` lists merge targets minus the sources with a merge consequence,
//! `@c+id#` lists drawers, and `@c#` lists sections with creation and the
//! §3.4 near-duplicate refusal.

use crate::support::*;
use serde_json::Value;
use std::path::Path;

fn transfer_vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("inbox.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "## Tasks\n",
            "- [ ] #task Benchmark logical replication ^bench\n",
            "  - Try pgbench at 10k TPS\n",
            "- [ ] #task Cut over prod ^cutover\n",
            "- [-] #task Done deal ^done\n",
            "- [ ] #task Project shell ^prj\n",
            "- [ ] #task Parent work ^parent\n",
            "  - [ ] #task Child work ^child\n",
            "  - [ ] #task Nested work ^nested\n",
            "    - [ ] #task Sub work ^sub\n",
        ),
    );
    write_file(
        &vault.join("db.md"),
        concat!(
            "---\ntype: \"[[area]]\"\n---\n",
            "## Tasks\n",
            "- [ ] #task Upgrade Postgres ^upgrade\n",
            "\t- FUTURE WORK\n",
            "\t\t- existing note\n",
            "- [ ] #task Legacy cleanup ^legacy\n",
            "## Future Work\n",
            "\n",
            "## Someday\n",
        ),
    );
    vault
}

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

/// Warnings are omitted from the JSON when empty, so a missing key reads
/// as no warnings.
fn warnings_of(json: &Value) -> Vec<&str> {
    json.get("warnings")
        .and_then(|warnings| warnings.as_array())
        .map(|warnings| {
            warnings
                .iter()
                .filter_map(|warning| warning.as_str())
                .collect()
        })
        .unwrap_or_default()
}

fn complete_end(temp: &TempDir, vault: &Path, draft: &str) -> Value {
    complete_at(temp, vault, draft, draft.len())
}

fn block_ids(json: &Value) -> Vec<String> {
    json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|row| row["block_id"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn transfer_sources_narrow_to_movable_or_mergeable_tasks() {
    let temp = TempDir::new("bob-cli-transfer-complete-sources");
    let vault = transfer_vault(&temp);
    // A partial source id completes the source note's tasks.
    let json = complete_at(&temp, &vault, "@inbox+ > @db", 7);
    assert_eq!(
        json["context"].as_str().expect("context"),
        "task",
        "{json:?}"
    );
    assert_eq!(
        json["replacement"],
        serde_json::json!({"start": 7, "end": 7})
    );
    let ids = block_ids(&json);
    for present in ["bench", "cutover", "parent", "child", "sub"] {
        assert!(
            ids.iter().any(|id| id == present),
            "{present} should list, got {ids:?}"
        );
    }
    // Closed, project, and nested-with-open-subtasks rows never list:
    // `done` is closed, `prj` cannot move or merge, and `nested` keeps an
    // open nested task.
    for absent in ["done", "prj", "nested"] {
        assert!(
            !ids.iter().any(|id| id == absent),
            "{absent} should not list, got {ids:?}"
        );
    }
    // The same scoping holds mid-line on a complete transfer.
    let draft = "@inbox+bench > @db";
    let mid = complete_at(&temp, &vault, draft, 9);
    assert_eq!(mid["context"].as_str().expect("context"), "task");
    assert_eq!(mid["query"].as_str().expect("query"), "be");
}

#[test]
fn transfer_destination_routes_complete_after_the_arrow() {
    let temp = TempDir::new("bob-cli-transfer-complete-routes");
    let vault = transfer_vault(&temp);
    let bare = complete_end(&temp, &vault, "@inbox+bench > @");
    assert_eq!(
        bare["context"].as_str().expect("context"),
        "route",
        "{bare:?}"
    );
    let typed = complete_end(&temp, &vault, "@inbox+bench > @d");
    assert_eq!(
        typed["context"].as_str().expect("context"),
        "route",
        "{typed:?}"
    );
    let routes: Vec<&str> = typed["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|row| row["route"].as_str())
        .collect();
    assert!(routes.contains(&"db"), "db should list, got {routes:?}");
}

#[test]
fn transfer_complete_note_destination_carries_the_merge_hint() {
    let temp = TempDir::new("bob-cli-transfer-complete-hint");
    let vault = transfer_vault(&temp);
    let json = complete_end(&temp, &vault, "@inbox+bench > @db");
    assert_eq!(
        json["context"].as_str().expect("context"),
        "route",
        "{json:?}"
    );
    let warnings = warnings_of(&json);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("type + to merge")),
        "merge hint should warn, got {warnings:?}"
    );
}

#[test]
fn transfer_merge_targets_drop_sources_and_carry_consequences() {
    let temp = TempDir::new("bob-cli-transfer-complete-targets");
    let vault = transfer_vault(&temp);
    let json = complete_end(&temp, &vault, "@db+legacy > @db+up");
    assert_eq!(
        json["context"].as_str().expect("context"),
        "task",
        "{json:?}"
    );
    let ids = block_ids(&json);
    assert!(
        ids.iter().any(|id| id == "upgrade"),
        "upgrade should list, got {ids:?}"
    );
    assert!(
        !ids.iter().any(|id| id == "legacy"),
        "the typed source should not list, got {ids:?}"
    );
    for row in json["candidates"].as_array().expect("candidates") {
        let consequence =
            row["consequence"].as_str().expect("merge consequence");
        assert!(
            consequence.contains("⤵ merge into"),
            "row should carry the merge label, got {row:?}"
        );
    }
}

#[test]
fn transfer_drawer_destination_lists_task_drawers() {
    let temp = TempDir::new("bob-cli-transfer-complete-drawers");
    let vault = transfer_vault(&temp);
    let json = complete_end(&temp, &vault, "@db+legacy > @db+upgrade#fu");
    assert_eq!(
        json["context"].as_str().expect("context"),
        "drawer",
        "{json:?}"
    );
    let titles: Vec<&str> = json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|row| row["title"].as_str())
        .collect();
    assert!(
        titles.contains(&"FUTURE WORK"),
        "FUTURE WORK should list, got {titles:?}"
    );
}

#[test]
fn transfer_section_destination_lists_sections_then_creation() {
    let temp = TempDir::new("bob-cli-transfer-complete-sections");
    let vault = transfer_vault(&temp);
    let json = complete_end(&temp, &vault, "@db+legacy > @db#fu");
    assert_eq!(
        json["context"].as_str().expect("context"),
        "section",
        "{json:?}"
    );
    let titles: Vec<&str> = json["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|row| row["title"].as_str())
        .collect();
    assert!(
        titles.contains(&"Future Work"),
        "Future Work should list, got {titles:?}"
    );
    // An unmatched name gains a creation row.
    let create = complete_end(&temp, &vault, "@db+legacy > @db#brand-new");
    let created: Vec<&str> = create["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|row| row["replacement"].as_str())
        .collect();
    assert!(
        created.contains(&"brand-new"),
        "creation row should list, got {created:?}"
    );
    // A strict prefix of an existing heading is refused with the §3.4 copy.
    let near = complete_end(&temp, &vault, "@db+legacy > @db#futur");
    let warnings = warnings_of(&near);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("did you mean `## Future Work`")),
        "near-duplicate should warn, got {warnings:?}"
    );
    // An exact slug match selects the heading with no creation row.
    let exact = complete_end(&temp, &vault, "@db+legacy > @db#future-work");
    let exact_rows = exact["candidates"].as_array().expect("candidates");
    assert_eq!(exact_rows.len(), 1, "{exact_rows:?}");
    assert!(warnings_of(&exact).is_empty(), "{exact:?}");
}
