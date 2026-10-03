use std::path::{Path, PathBuf};

use super::{
    super::{
        model::{Candidates, CaptureCompleteResult, Replacement},
        render::candidate_lines,
    },
    day_file_guard, result, with_env, write_file, TempDir,
};
use crate::native::capture_language::CompletionContext;

/// The `:` picker worked example: vault-wide linkable tasks in
/// canonical order behind a `BOB_DAY_FILE` ledger, with the capture
/// clock pinned so pull-forward flags are deterministic.
pub(super) fn task_link_fixture(root: &Path) -> PathBuf {
    write_file(
        &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
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
        &root.join("mac_inbox.md"),
        "---\ntype: [[area]]\n---\n- [ ] #task Call the bank [created::2026-09-29]\n",
    );
    write_file(
        &root.join("health.md"),
        "---\ntype: [[area]]\n---\n## Errands\n- [?] #task Book dentist [scheduled::2026-10-03]\n",
    );
    write_file(
        &root.join("bob.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n- [ ] #task Polish capture picker ^polish\n\t- [ ] #task Tune fuzzy weights\n",
    );
    write_file(
        &root.join("sase.md"),
        "---\ntype: [[project]]\nstatus: wip\n---\n## Bugs\n- [*] #task Fix deep bug ^deep-fix\n- [ ] #task Fix flaky gkeep test\n- [x] #task Old fix ^old-fix\n## Writing\n- [/] #task Draft outline ^outline\n- [ ] #task Ship blog post #now ^blog\n- [-] #task Dropped idea\n",
    );
    write_file(
        &root.join("archive.md"),
        "---\ntype: [[project]]\nstatus: done\n---\n- [ ] #task Leftover ^leftover\n",
    );
    write_file(&root.join("scratch.md"), "- [ ] #task Loose end ^loose\n");
    let day_file = root.join("2026/20260930.md");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n\t- [[sase#^deep-fix]]\n",
    );
    day_file
}

pub(super) fn task_link_result(
    root: &Path,
    day_file: &Path,
    raw: &str,
    cursor: usize,
) -> CaptureCompleteResult {
    with_env("BOB_DAY_FILE", day_file, || {
        with_env("BOB_NOW", "2026-09-30 09:02:00", || {
            result(root, raw, cursor)
        })
    })
}

#[test]
fn task_link_completion_lists_worked_example_in_canonical_order() {
    let _guard = day_file_guard();
    let temp = TempDir::new("bob-cli-capture-complete-task-link");
    let day_file = task_link_fixture(temp.path());

    let value = task_link_result(temp.path(), &day_file, ":", 1);
    assert_eq!(value.context, Some(CompletionContext::TaskLink));
    assert_eq!(value.replacement, Replacement { start: 0, end: 1 });
    let Candidates::TaskLink(candidates) = &value.candidates else {
        panic!("expected task-link candidates");
    };
    let replacements: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.replacement.as_str())
        .collect();
    assert_eq!(
        replacements,
        vec![
            "@sase:deep-fix",
            "@sase:outline",
            "",
            "",
            "@bob:polish",
            "",
            "",
            "@sase:blog",
        ]
    );
    assert!(value.warnings.is_empty());
}

#[test]
fn task_link_completion_pins_json_shape_and_omissions() {
    let _guard = day_file_guard();
    let temp = TempDir::new("bob-cli-capture-complete-task-link-json");
    let day_file = task_link_fixture(temp.path());

    let value = task_link_result(temp.path(), &day_file, ":", 1);
    let json = serde_json::to_value(&value).expect("serialize result");
    assert_eq!(json["context"], "task_link");
    assert_eq!(json["schema_version"], 1);
    assert!(json.get("block_id").is_none());

    let identified = &json["candidates"][0];
    assert_eq!(
        identified,
        &serde_json::json!({
            "replacement": "@sase:deep-fix",
            "ref": identified["ref"],
            "route": "sase",
            "note_kind": "project",
            "block_id": "deep-fix",
            "requires_block_id": false,
            "block_id_suggestions": [],
            "status_symbol": "*",
            "status_name": "Next",
            "status_type": "ON_HOLD",
            "text": "Fix deep bug",
            "section": "Bugs",
            "depth": 0,
            "line": 6,
            "group": "queued",
            "scheduled": null,
            "pomodoro": {
                "line": 2,
                "name": "BUGS",
                "time_range": null,
                "is_current": false,
            },
        })
    );
    assert!(identified.get("now").is_none());
    assert!(identified.get("pulls_forward").is_none());

    let missing = &json["candidates"][6];
    assert_eq!(missing["replacement"], "");
    assert_eq!(missing["route"], "sase");
    assert_eq!(missing["note_kind"], "project");
    assert!(missing["block_id"].is_null());
    assert_eq!(missing["requires_block_id"], true);
    assert_eq!(
        missing["block_id_suggestions"],
        serde_json::json!(["fix-flaky-gkeep", "flaky-gkeep-test"])
    );
    assert_eq!(missing["status_symbol"], " ");
    assert_eq!(missing["text"], "Fix flaky gkeep test");
    assert_eq!(missing["section"], "Bugs");
    assert_eq!(missing["group"], "note");
    assert!(missing["scheduled"].is_null());
    assert!(missing["pomodoro"].is_null());
    assert!(missing.get("now").is_none());
    assert!(missing.get("pulls_forward").is_none());

    // The retired `#now` text stays on the row without a `now` key,
    // and the pull-forward flag serializes when set.
    let bet = &json["candidates"][7];
    assert_eq!(bet["replacement"], "@sase:blog");
    assert_eq!(bet["text"], "Ship blog post #now");
    assert_eq!(bet["group"], "note");
    assert!(bet.get("now").is_none());
    assert_eq!(json["candidates"][3]["pulls_forward"], true);
    assert_eq!(
        json["candidates"][3]["scheduled"],
        serde_json::json!("2026-10-03")
    );
}

#[test]
fn task_link_completion_queries_cover_the_sigil_and_rank() {
    let _guard = day_file_guard();
    let temp = TempDir::new("bob-cli-capture-complete-task-link-query");
    let day_file = task_link_fixture(temp.path());

    // `:dee` narrows to the queued row with a sigil-inclusive range.
    let raw = ":dee";
    let ranked = task_link_result(temp.path(), &day_file, raw, raw.len());
    assert_eq!(ranked.context, Some(CompletionContext::TaskLink));
    assert_eq!(ranked.replacement, Replacement { start: 0, end: 4 });
    let Candidates::TaskLink(candidates) = &ranked.candidates else {
        panic!("expected task-link candidates");
    };
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].replacement, "@sase:deep-fix");

    // A cursor just before the sigil refetches the full list.
    let full = task_link_result(temp.path(), &day_file, raw, 0);
    assert_eq!(full.context, Some(CompletionContext::TaskLink));
    assert_eq!(full.replacement, Replacement { start: 0, end: 4 });
    assert_eq!(full.candidates.len(), 8);
}

#[test]
fn task_link_completion_scopes_to_the_batch_second_item() {
    let _guard = day_file_guard();
    let temp = TempDir::new("bob-cli-capture-complete-task-link-batch");
    let day_file = task_link_fixture(temp.path());

    let raw = "Buy milk\n\n:dee";
    let value = task_link_result(temp.path(), &day_file, raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::TaskLink));
    assert_eq!(value.replacement, Replacement { start: 10, end: 14 });
    let Candidates::TaskLink(candidates) = &value.candidates else {
        panic!("expected task-link candidates");
    };
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].replacement, "@sase:deep-fix");
}

#[test]
fn task_link_human_rows_name_queues_and_missing_ids() {
    let _guard = day_file_guard();
    let temp = TempDir::new("bob-cli-capture-complete-task-link-human");
    let day_file = task_link_fixture(temp.path());

    let value = task_link_result(temp.path(), &day_file, ":", 1);
    let rows = candidate_lines(&value.candidates, value.context);
    assert_eq!(
        rows,
        vec![
            (
                "@sase:deep-fix".to_string(),
                "[*] Fix deep bug  · BUGS".to_string(),
            ),
            (
                "@sase:outline".to_string(),
                "[/] Draft outline  · In Progress".to_string(),
            ),
            (
                "@mac_inbox:…".to_string(),
                "[ ] Call the bank  · mac_inbox.md · needs ID (^call-bank)"
                    .to_string(),
            ),
            (
                "@health:…".to_string(),
                "[?] Book dentist  · health.md · needs ID (^book-dentist)  · scheduled 2026-10-03"
                    .to_string(),
            ),
            (
                "@bob:polish".to_string(),
                "[ ] Polish capture picker  · bob.md".to_string(),
            ),
            (
                "@bob:…".to_string(),
                "[ ] Tune fuzzy weights  · bob.md · needs ID (^tune-fuzzy-weights)"
                    .to_string(),
            ),
            (
                "@sase:…".to_string(),
                "[ ] Fix flaky gkeep test  · sase.md · needs ID (^fix-flaky-gkeep)"
                    .to_string(),
            ),
            (
                "@sase:blog".to_string(),
                "[ ] Ship blog post #now  · sase.md".to_string(),
            ),
        ]
    );
}

#[test]
fn task_link_completion_keeps_candidates_when_the_day_file_is_missing() {
    let _guard = day_file_guard();
    let temp = TempDir::new("bob-cli-capture-complete-task-link-warn");
    let day_file = task_link_fixture(temp.path());
    let missing = day_file.parent().expect("day parent").join("20990101.md");

    let value = task_link_result(temp.path(), &missing, ":", 1);
    assert_eq!(value.context, Some(CompletionContext::TaskLink));
    assert_eq!(value.candidates.len(), 8);
    assert!(
        value
            .warnings
            .iter()
            .any(|warning| warning.contains("Bob daily note does not exist")),
        "a missing ledger warns without dropping candidates: {:?}",
        value.warnings
    );
}
