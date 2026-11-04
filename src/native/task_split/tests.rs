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
