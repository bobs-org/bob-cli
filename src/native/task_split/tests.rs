use std::{collections::BTreeMap, path::PathBuf};

use serde_json::{json, Value};

use super::plan::{
    plan_task_split, SplitRequest, SplitSelection, SplitSettings,
};
use crate::native::{note_tasks, task_move::VaultSnapshot};

#[test]
fn split_conformance_vectors_match_byte_for_byte() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/task_split/vectors.json"
    ))
    .expect("valid split vectors");
    let cases = vectors["cases"].as_array().expect("vector cases");
    let task_settings =
        note_tasks::read_settings(&PathBuf::from("/missing-vault"));
    for case in cases {
        let id = case["id"].as_str().expect("case id");
        let input = &case["input"];
        let mut files = BTreeMap::new();
        for (path, contents) in input["files"].as_object().expect("files") {
            files.insert(
                PathBuf::from(path),
                contents.as_str().unwrap().to_string(),
            );
        }
        let snapshot = VaultSnapshot { notes: files };
        let owner_path =
            PathBuf::from(input["owner"]["path"].as_str().unwrap());
        let owner_line = input["owner"]["line"].as_u64().unwrap() as usize;
        let selection = parse_selection(&input["selection"]);
        let request = SplitRequest {
            owner_path,
            owner_line,
            selection,
        };
        let settings = SplitSettings {
            linked_from_lines: input["settings"]["linked_from_lines"]
                .as_bool()
                .unwrap_or(true),
            linked_from_mentions: input["settings"]["linked_from_mentions"]
                .as_bool()
                .unwrap_or(true),
            inbox_route: input["settings"]["inbox_route"]
                .as_str()
                .unwrap_or("inbox.md")
                .to_string(),
        };
        let result = plan_task_split(
            &snapshot,
            &request,
            input["today"].as_str().unwrap(),
            &settings,
            &task_settings,
        );
        match (&case["expect"]["ok"], result) {
            (Value::Bool(true), Ok(plan)) => {
                let expected_files =
                    case["expect"]["files"].as_object().unwrap();
                for (path, expected) in expected_files {
                    assert_eq!(
                        plan.postimages
                            .get(&PathBuf::from(path))
                            .map(String::as_str),
                        expected.as_str(),
                        "{id} file {path}"
                    );
                }
                let tasks = plan
                    .tasks
                    .iter()
                    .map(|task| {
                        json!({
                            "path": task.path,
                            "block_id": task.block_id,
                            "title": task.title,
                            "anchor_reused": task.anchor_reused,
                            "child_lines": task.child_lines,
                        })
                    })
                    .collect::<Vec<_>>();
                assert_eq!(json!(tasks), case["expect"]["tasks"], "{id} tasks");
                assert_eq!(
                    plan.notice,
                    case["expect"]["notice"].as_str().unwrap(),
                    "{id} notice"
                );
                assert_eq!(
                    plan.cursor_path,
                    case["expect"]["cursor"]["path"]
                        .as_str()
                        .unwrap_or(&plan.cursor_path),
                    "{id} cursor path"
                );
                assert_eq!(
                    plan.cursor_line,
                    case["expect"]["cursor"]["line"]
                        .as_u64()
                        .unwrap_or(plan.cursor_line as u64)
                        as usize,
                    "{id} cursor line"
                );
                if let Some(placement) = case["expect"]["placement"].as_object()
                {
                    assert_eq!(
                        plan.placement.kind,
                        placement["kind"].as_str().unwrap(),
                        "{id} placement kind"
                    );
                    assert_eq!(
                        plan.placement.path,
                        placement["path"].as_str().unwrap(),
                        "{id} placement path"
                    );
                }
                if let Some(owner_id) =
                    case["expect"]["owner_minted_id"].as_str()
                {
                    assert_eq!(
                        plan.owner_minted_id.as_deref(),
                        Some(owner_id),
                        "{id} owner minted id"
                    );
                }
            }
            (Value::Bool(false), Err(error)) => {
                assert_eq!(
                    error.code,
                    case["expect"]["code"].as_str().unwrap(),
                    "{id} error code"
                );
                assert_eq!(
                    error.message,
                    case["expect"]["message"].as_str().unwrap(),
                    "{id} error message"
                );
            }
            (expected, actual) => {
                panic!("{id}: expected {expected}, got {actual:?}")
            }
        }
    }
}

fn settings() -> SplitSettings {
    SplitSettings {
        linked_from_lines: true,
        linked_from_mentions: true,
        inbox_route: "inbox.md".to_string(),
    }
}

fn task_settings() -> crate::native::note_tasks::NoteTaskSettings {
    crate::native::note_tasks::read_settings(&PathBuf::from("/missing-vault"))
}

fn split(
    notes: &[(&str, &str)],
    owner: &str,
    line: usize,
    selection: SplitSelection,
) -> super::plan::SplitPlan {
    let snapshot =
        VaultSnapshot::from_notes(notes.iter().map(|(path, contents)| {
            (PathBuf::from(path), (*contents).to_string())
        }));
    plan_task_split(
        &snapshot,
        &SplitRequest {
            owner_path: PathBuf::from(owner),
            owner_line: line,
            selection,
        },
        "2026-11-19",
        &settings(),
        &task_settings(),
    )
    .unwrap_or_else(|error| {
        panic!("split plan failed ({}): {}", error.code, error.message)
    })
}

fn post<'a>(plan: &'a super::plan::SplitPlan, path: &str) -> &'a str {
    plan.postimages
        .get(&PathBuf::from(path))
        .map(String::as_str)
        .unwrap_or_else(|| panic!("missing postimage {path}"))
}

fn drafted_summary_note(summary: &str, body: &str) -> String {
    format!(
        "---\nsummary: {summary:?}\nsummary_digest: {}\nsummary_drafted: 2026-11-19\n---\n\n{body}",
        crate::native::intentions::digest(summary),
    )
}

#[test]
fn split_repairs_cross_note_anchors_in_owned_fields_and_payload() {
    let summary = "Plan [[#^a1]] next.";
    let source = format!(
        "---\nsummary: {summary:?}\nsummary_digest: {}\nsummary_drafted: 2026-11-19\n---\n\n- [ ] #task Big one ^owner\n  - Note alpha ^a1\n    - detail [[2026/20261118#^stay]] plus [[#^stay]]\n  - Note beta\n  - Stay put ^stay\n",
        crate::native::intentions::digest(summary),
    );
    let weekly_summary = "Saw [[2026/20261118#^a1]] move.";
    let old_dep = crate::native::task_dependencies::dependency_id(
        std::path::Path::new("2026/20261118.md"),
        "a1",
    )
    .expect("source dependency id");
    let new_dep = crate::native::task_dependencies::dependency_id(
        std::path::Path::new("inbox.md"),
        "a1",
    )
    .expect("destination dependency id");
    let bystander_body =
        format!("- [ ] #task Watcher [dependsOn:: {old_dep}] ^w\n");
    let weekly = drafted_summary_note(weekly_summary, "Weekly body.\n");
    let notes = [
        ("2026/20261118.md", source.as_str()),
        ("inbox.md", "## Tasks\n"),
        ("2026/2026-W47.md", weekly.as_str()),
        ("watcher.md", bystander_body.as_str()),
    ];
    let plan =
        split(&notes, "2026/20261118.md", 7, SplitSelection::Nth(vec![1]));

    // The extracted anchor leaves the source: stubs point at the new note
    // while owned fields and bystander links follow it there.
    let moved = post(&plan, "2026/20261118.md");
    assert!(moved.contains("- [[inbox#^a1|"), "{moved}");
    assert!(!moved.contains("- Note alpha ^a1"), "{moved}");
    assert!(moved.contains("- Note beta"), "{moved}");
    assert!(
        moved.contains("summary: \"Plan [[inbox#^a1]] next.\""),
        "{moved}"
    );
    let fields = crate::native::intentions::read(
        moved,
        crate::native::intentions::NoteSubject::Day(
            chrono::NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(),
        ),
    );
    assert_eq!(
        fields.summary_digest.value.as_deref(),
        Some(
            crate::native::intentions::digest("Plan [[inbox#^a1]] next.")
                .as_str()
        )
    );

    let dest = post(&plan, "inbox.md");
    assert!(dest.contains("- [ ] #task Note alpha"), "{dest}");
    assert!(dest.contains("^a1"), "{dest}");
    // The payload keeps working: the explicit moved link follows the
    // anchor, and the pathless stayed link is qualified back to the source.
    assert!(
        dest.contains("[[2026/20261118#^stay]] plus [[2026/20261118#^stay]]"),
        "{dest}"
    );
    // Task and cursor lines are located on the final postimage bytes.
    let expected = dest
        .lines()
        .position(|line| line.contains("^a1") && line.contains("#task"))
        .expect("extracted task lands in destination")
        + 1;
    assert_eq!(plan.tasks[0].line, expected);
    assert_eq!(plan.cursor_line, expected);
    assert_eq!(plan.cursor_path, "inbox.md");

    let weekly = post(&plan, "2026/2026-W47.md");
    assert!(
        weekly.contains("summary: \"Saw [[inbox#^a1]] move.\""),
        "{weekly}"
    );

    let watcher = post(&plan, "watcher.md");
    assert!(
        watcher.contains(&format!("[dependsOn:: {new_dep}]")),
        "{watcher}"
    );
    assert!(!watcher.contains(&old_dep), "{watcher}");

    let repaired: std::collections::BTreeMap<_, _> = plan
        .repaired_files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                (file.link_count, file.dependency_token_count),
            )
        })
        .collect();
    assert!(
        repaired.contains_key(&PathBuf::from("inbox.md")),
        "{repaired:?}"
    );
    assert_eq!(
        repaired.get(&PathBuf::from("2026/20261118.md")),
        Some(&(1, 0)),
        "{repaired:?}"
    );
}

#[test]
fn split_same_note_fabricates_no_retargets() {
    let weekly_summary = "Saw [[notes#^a1]] stay.";
    let weekly = drafted_summary_note(weekly_summary, "Weekly body.\n");
    let notes = [
        (
            "notes.md",
            "- [ ] #task Big one ^owner\n  - Note alpha ^a1\n  - Stay put ^stay\n",
        ),
        ("2026/2026-W47.md", weekly.as_str()),
    ];
    let plan = split(&notes, "notes.md", 1, SplitSelection::Nth(vec![1]));
    // The anchor never leaves the note: the bystander keeps resolving to
    // it and never enters the postimages.
    assert!(!plan
        .postimages
        .contains_key(&PathBuf::from("2026/2026-W47.md")));
    assert!(plan.repaired_files.is_empty());
    let moved = post(&plan, "notes.md");
    assert!(moved.contains("- [[#^a1|"), "{moved}");
    let expected = moved
        .lines()
        .position(|line| line.contains("^a1") && line.contains("#task"))
        .expect("extracted task lands below owner")
        + 1;
    assert_eq!(plan.tasks[0].line, expected);
    assert_eq!(plan.cursor_line, expected);
    assert_eq!(plan.cursor_path, "notes.md");
}

#[test]
fn split_minted_anchor_still_qualifies_pathless_stayed_anchor_links() {
    let notes = [(
        "2026/20261118.md",
        concat!(
            "- [ ] #task Big one ^owner\n",
            "  - Note alpha\n",
            "    - Detail [[#^stay]]\n",
            "  - Stay put ^stay\n",
        ),
    )];
    let plan =
        split(&notes, "2026/20261118.md", 0, SplitSelection::Nth(vec![1]));
    let destination = post(&plan, "inbox.md");
    assert!(
        destination.contains("[[2026/20261118#^stay]]"),
        "{destination}"
    );
    assert_eq!(
        plan.repaired_files
            .iter()
            .find(|file| file.path.as_path() == std::path::Path::new("inbox.md"))
            .map(|file| file.link_count),
        Some(1),
        "link qualification is counted even without relocated anchors"
    );
    assert_eq!(
        plan.tasks[0].line,
        destination
            .lines()
            .position(|line| line.contains("#task Note alpha"))
            .unwrap()
            + 1
    );
    assert_eq!(plan.cursor_line, plan.tasks[0].line);
}

#[test]
fn split_repairs_whole_payload_with_multiline_fence_and_comment_context() {
    let fence = char::from(96).to_string().repeat(3);
    let source = format!(
        "- [ ] #task Big one ^owner\n  - Note alpha ^alpha\n    {fence}md\n    [[2026/20261118#^alpha]]\n    {fence}\n    <!--\n    [[2026/20261118#^alpha]]\n    -->\n    [[#^stay]]\n  - Stay put ^stay\n"
    );
    let notes = [("2026/20261118.md", source.as_str())];
    let plan =
        split(&notes, "2026/20261118.md", 0, SplitSelection::Nth(vec![1]));
    let destination = post(&plan, "inbox.md");
    assert!(
        destination.contains("[[2026/20261118#^alpha]]"),
        "fenced and commented references preserve their source text: {destination}"
    );
    assert!(
        destination.contains("[[2026/20261118#^stay]]"),
        "live pathless links to stayed source anchors are qualified: {destination}"
    );
    assert_eq!(
        destination.matches("[[2026/20261118#^alpha]]").count(),
        2,
        "neither the fence nor multiline comment is rewritten"
    );
    assert_eq!(
        plan.repaired_files
            .iter()
            .find(|file| file.path.as_path() == std::path::Path::new("inbox.md"))
            .map(|file| file.link_count),
        Some(1)
    );
    let landing = destination
        .lines()
        .position(|line| line.contains("#task Note alpha"))
        .unwrap()
        + 1;
    assert_eq!(plan.tasks[0].line, landing);
    assert_eq!(plan.cursor_line, landing);
}

fn parse_selection(value: &Value) -> SplitSelection {
    match value["kind"].as_str().unwrap() {
        "cursor" => SplitSelection::Cursor {
            line: value["line"].as_u64().unwrap() as usize,
        },
        "count" => SplitSelection::Count {
            line: value["line"].as_u64().unwrap() as usize,
            count: value["count"].as_u64().unwrap() as usize,
        },
        "lines" => SplitSelection::Lines {
            from: value["from"].as_u64().unwrap() as usize,
            to: value["to"].as_u64().unwrap() as usize,
        },
        "nth" => SplitSelection::Nth(
            value["picks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as usize)
                .collect(),
        ),
        "match" => SplitSelection::Match(
            value["texts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect(),
        ),
        "section" => SplitSelection::Section {
            title: value["title"].as_str().unwrap().to_string(),
        },
        other => panic!("unknown vector selection kind: {other}"),
    }
}
