use std::path::Path;

use super::*;
use crate::native::task_children::{
    classify_task_child, classify_task_child_with_filter, TaskChildKind,
};

fn settings() -> NoteTaskSettings {
    note_tasks::read_settings(Path::new("/no-bob-vault"))
}

fn source_of(lines: &[String]) -> String {
    let mut source = lines.join("\n");
    source.push('\n');
    source
}

#[test]
fn parses_every_checkbox_mark() {
    let settings = settings();
    for (body, expected, ch) in [
        ("- [ ] text", ReqMark::Open, ' '),
        ("- [-] text", ReqMark::Dropped, '-'),
        ("- [>] [[#^id|Alias]]", ReqMark::Carried, '>'),
        ("- [x] text", ReqMark::LegacyMet, 'x'),
        ("- [X] text", ReqMark::LegacyMet, 'x'),
        ("- [?] text", ReqMark::Unknown('?'), '?'),
        ("- [/] text", ReqMark::Unknown('/'), '?'),
        ("- [ ]", ReqMark::Open, ' '),
    ] {
        let mark = checkbox_mark_without_filter(body, &settings.global_filter);
        assert_eq!(mark, Some(expected), "{body}");
        assert_eq!(mark.unwrap().mark_char(), ch, "{body} char");
    }
}

#[test]
fn rejects_non_requirement_shapes() {
    let settings = settings();
    for line in [
        "- [ ] #task Real task",
        "- [x] #task Done task",
        "- [ ]Fix missing space",
        "- plain note",
        "> - [ ] quoted",
        "- NOTES",
        "\t- [ ] #taskbar item",
    ] {
        // The last line carries no whole-token `#task`, so it is a
        // requirement candidate by shape; the rest are never reqs.
        let expected = line.contains("#taskbar");
        assert_eq!(
            checkbox_mark_without_filter(line, &settings.global_filter)
                .is_some(),
            expected,
            "{line}"
        );
    }
}

#[test]
fn child_kind_detects_requirements_before_authored() {
    for (line, open, expected) in [
        (
            "\t- [ ] a requirement",
            None,
            TaskChildKind::Requirement {
                mark: ReqMark::Open,
            },
        ),
        (
            "\t- [-] dropped",
            None,
            TaskChildKind::Requirement {
                mark: ReqMark::Dropped,
            },
        ),
        (
            "\t- [?] flagged",
            None,
            TaskChildKind::Requirement {
                mark: ReqMark::Unknown('?'),
            },
        ),
        ("\t- [ ] #task nested", None, TaskChildKind::Authored),
        (
            "\t- [ ] #task nested",
            Some(true),
            TaskChildKind::NestedTask { open: true },
        ),
        (
            "\t- [x] #task closed",
            Some(false),
            TaskChildKind::NestedTask { open: false },
        ),
        (
            "\t- NOTES",
            None,
            TaskChildKind::Drawer {
                title: "NOTES".to_string(),
            },
        ),
        ("\t- a plain note", None, TaskChildKind::Authored),
        ("> - [ ] quoted", None, TaskChildKind::Authored),
    ] {
        assert_eq!(classify_task_child(line, open), expected, "{line}");
    }
}

#[test]
fn child_kind_with_custom_filter_matches_whole_tokens() {
    // A vault whose global filter differs still separates its own tasks
    // from requirement checkboxes.
    assert_eq!(
        classify_task_child_with_filter("\t- [ ] #todo real", None, "#todo"),
        TaskChildKind::Authored
    );
    assert_eq!(
        classify_task_child_with_filter("\t- [ ] plain", None, "#todo"),
        TaskChildKind::Requirement {
            mark: ReqMark::Open
        }
    );
    assert_eq!(
        classify_task_child_with_filter(
            "\t- [ ] #todo real",
            Some(true),
            "#todo"
        ),
        TaskChildKind::NestedTask { open: true }
    );
}

#[test]
fn fixture_lists_every_shared_case_without_renumbering() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/task_reqs/vectors.json"
    ))
    .expect("valid requirement vectors");
    assert_eq!(vectors["schema_version"], 1);
    assert_eq!(vectors["max_depth"], MAX_REQ_DEPTH);
    let cases = vectors["cases"].as_array().expect("case list");
    assert!(!cases.is_empty(), "the shared vector set is nonempty");
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["id"], format!("RQ{}", index + 1));
    }
}

#[test]
fn fixture_vectors_match_recognizer() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/task_reqs/vectors.json"
    ))
    .expect("valid requirement vectors");
    let settings = settings();
    let cases = vectors["cases"].as_array().expect("case list");
    for case in cases {
        check_vector_case(case, &settings);
    }
}

fn check_vector_case(case: &serde_json::Value, settings: &NoteTaskSettings) {
    let id = case["id"].as_str().unwrap_or("?");
    let lines: Vec<String> = case["source_lines"]
        .as_array()
        .unwrap_or_else(|| panic!("{id} has source_lines"))
        .iter()
        .map(|line| {
            line.as_str()
                .unwrap_or_else(|| panic!("{id} line is a string"))
                .to_string()
        })
        .collect();
    let contents = source_of(&lines);
    // Exercise the per-owner entry point every recursive consumer uses.
    let scan = note_tasks::scan(&contents, settings);
    let mut forest = Vec::new();
    for task in scan.tasks() {
        forest.extend(req_forest(&contents, task, settings));
    }
    let expected = &case["expect"];
    let nodes = expected["nodes"].as_array().expect("nodes list");
    assert_eq!(
        forest.len(),
        nodes.len(),
        "{id} recognizes every expected node"
    );
    for (node, expected) in forest.iter().zip(nodes.iter()) {
        assert_eq!(node.line_index + 1, expected["line"], "{id} line");
        assert_eq!(node.depth, expected["depth"], "{id} depth");
        assert_eq!(
            node.parent.map(|line| line + 1),
            expected["parent"].as_u64().map(|line| line as usize),
            "{id} parent"
        );
        assert_eq!(node.owner_line + 1, expected["owner"], "{id} owner");
        assert_eq!(
            node.mark.name(),
            expected["mark"].as_str().unwrap_or("?"),
            "{id} mark"
        );
        assert_eq!(
            node.state.name(),
            expected["state"].as_str().unwrap_or("?"),
            "{id} state"
        );
        if node.state.name() == "excluded" {
            let (by, because) = match node.state {
                EffectiveReqState::Excluded { by, because } => (by, because),
                _ => unreachable!(),
            };
            assert_eq!(by + 1, expected["excluded_by"], "{id} excluded_by");
            let cause = match because {
                ReqExcludeCause::Dropped => "dropped",
                ReqExcludeCause::Carried => "carried",
            };
            assert_eq!(cause, expected["excluded_because"], "{id} cause");
        }
        assert_eq!(node.text, expected["text"], "{id} text");
        assert_eq!(
            node.anchor.as_deref(),
            expected["anchor"].as_str(),
            "{id} anchor"
        );
        assert_eq!(req_path(node), expected["path"], "{id} path");
        assert_eq!(
            node.subtree_end + 1,
            expected["subtree_end"],
            "{id} subtree_end"
        );
        let notes: Vec<(u64, u64)> = node
            .note_ranges
            .iter()
            .map(|(start, end)| (*start as u64 + 1, *end as u64 + 1))
            .collect();
        let expected_notes: Vec<(u64, u64)> = expected["notes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|range| {
                (
                    range[0].as_u64().unwrap_or(0),
                    range[1].as_u64().unwrap_or(0),
                )
            })
            .collect();
        assert_eq!(notes, expected_notes, "{id} notes");
        assert_eq!(
            node.depth_exceeded,
            expected
                .get("depth_exceeded")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            "{id} depth_exceeded"
        );
    }
    if let Some(probes) = expected.get("probes") {
        for probe in probes.as_array().cloned().unwrap_or_default() {
            let line = probe["line"].as_u64().expect("probe line") as usize;
            let index = line - 1;
            let owner = probe["owner"].as_u64().map(|owner| owner as usize - 1);
            assert_eq!(
                owner_of_line(&contents, index, settings),
                owner,
                "{id} probe owner at line {line}"
            );
            assert_eq!(
                is_req_line(&contents, index, settings),
                probe["is_req"].as_bool().unwrap_or(false),
                "{id} probe is_req at line {line}"
            );
        }
    }
    if expected
        .get("assert_distinct_keys")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let owner_key = expected["owner_key"].as_str().expect("owner key");
        let mut keys = Vec::new();
        for node in &forest {
            let key = req_row_key(owner_key, &node.path, &node.text);
            let dotted = req_path(node);
            assert!(
                key.starts_with(&format!("{owner_key}/req/{dotted}~")),
                "{id} row key shape: {key}"
            );
            let suffix = key.rsplit('~').next().unwrap_or("");
            assert_eq!(suffix.len(), 8, "{id} row key digest: {key}");
            assert!(
                suffix.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{id} row key hex: {key}"
            );
            keys.push(key);
        }
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), forest.len(), "{id} distinct row keys");
    }
}

fn strip_req_heads(contents: &str, settings: &NoteTaskSettings) -> String {
    contents
        .split_inclusive('\n')
        .enumerate()
        .filter(|(index, _)| !is_req_line(contents, *index, settings))
        .map(|(_, line)| line)
        .collect()
}

fn note_task_idents(
    contents: &str,
    settings: &NoteTaskSettings,
) -> Vec<(Option<String>, String, char)> {
    note_tasks::scan(contents, settings)
        .tasks()
        .iter()
        .map(|task| {
            (
                task.block_id.clone(),
                task.description.clone(),
                task.status_symbol,
            )
        })
        .collect()
}

fn hook_task_idents(
    contents: &str,
    settings: &NoteTaskSettings,
) -> Vec<(Option<String>, String, char)> {
    crate::native::task_status_hooks::parse_tasks(contents, settings)
        .into_iter()
        .map(|task| (task.block_id, task.description, task.status))
        .collect()
}

const INVARIANCE_NOTE: &str = "\
## Tasks
- [ ] #task Owner one ^one
	- [ ] depth 1 req
		- [ ] depth 2 req
			- [ ] depth 3 req
		- a note of the depth-2 req
	- [ ] #task Nested subtask ^nested
		- [ ] nested's own req
	- plain owner note
- [*] #task Next lane ^next
	- [ ] next req
- [/] #task Pending lane ^pending
	- [ ] pending req
- [ ] #task Ready lane ^ready
	- [ ] ready req
		- [ ] ready nested req
";

const TASKS_PLUGIN_SETTINGS: &str = r##"{
  "globalFilter": "#task",
  "taskFormat": "dataview",
  "statusSettings": {
    "coreStatuses": [
      {"symbol":" ","name":"Todo","type":"TODO"},
      {"symbol":"x","name":"Done","type":"DONE"}
    ],
    "customStatuses": [
      {"symbol":"*","name":"Next","type":"TODO"},
      {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
      {"symbol":"-","name":"Cancelled","type":"CANCELLED"}
    ]
  }
}"##;

#[test]
fn reqs_are_invisible_to_task_readers() {
    let settings = settings();
    let stripped = strip_req_heads(INVARIANCE_NOTE, &settings);
    assert!(
        stripped.contains("#task Owner one"),
        "owners stay: {stripped}"
    );
    assert!(
        stripped.contains("#task Nested subtask"),
        "nested tasks stay: {stripped}"
    );
    assert!(
        !stripped.contains("depth 1 req")
            && !stripped.contains("nested's own req")
            && !stripped.contains("ready nested req"),
        "req heads must be gone: {stripped}"
    );
    assert_eq!(
        note_task_idents(INVARIANCE_NOTE, &settings),
        note_task_idents(&stripped, &settings),
        "note_tasks scan identities"
    );
    assert_eq!(
        hook_task_idents(INVARIANCE_NOTE, &settings),
        hook_task_idents(&stripped, &settings),
        "task_status_hooks parse identities"
    );

    let with_reqs = tempfile::tempdir().expect("vault with reqs");
    let without_reqs = tempfile::tempdir().expect("vault without reqs");
    for (dir, body) in [
        (with_reqs.path(), INVARIANCE_NOTE),
        (without_reqs.path(), stripped.as_str()),
    ] {
        let settings_path =
            dir.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        std::fs::create_dir_all(settings_path.parent().expect("parent"))
            .expect("create tasks plugin dir");
        std::fs::write(&settings_path, TASKS_PLUGIN_SETTINGS)
            .expect("write tasks settings");
        std::fs::write(dir.join("tasks.md"), body).expect("write note");
    }
    let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 21)
        .expect("date")
        .and_hms_opt(12, 0, 0)
        .expect("noon");
    for query in [
        crate::native::dataview::OPEN_QUERY,
        crate::native::dataview::READY_QUERY,
        crate::native::dataview::NEXT_QUERY,
        crate::native::dataview::PENDING_QUERY,
    ] {
        let with = crate::native::dataview::query_matching_descriptions(
            with_reqs.path(),
            query,
            now,
        )
        .expect("query vault with reqs");
        let without = crate::native::dataview::query_matching_descriptions(
            without_reqs.path(),
            query,
            now,
        )
        .expect("query vault without reqs");
        assert_eq!(with, without, "dataview query identities for {query}");
        assert!(
            !with.iter().any(|description| {
                description.contains("depth 1 req")
                    || description.contains("next req")
                    || description.contains("pending req")
                    || description.contains("ready req")
            }),
            "a req description leaked into {query}: {with:?}"
        );
    }
}

#[test]
fn req_glyph_depends_on_state_never_depth() {
    // One text glyph per state; depth takes no part, so the same state
    // reads the same at depths 1 through 16.
    for (state, expected) in [
        (EffectiveReqState::Pending, "◇"),
        (EffectiveReqState::Met, "◆"),
        (EffectiveReqState::Moot, "◇"),
        (EffectiveReqState::Dropped, "⊘"),
        (EffectiveReqState::Carried, "→"),
        (
            EffectiveReqState::Excluded {
                by: 0,
                because: ReqExcludeCause::Dropped,
            },
            "◇",
        ),
        (
            EffectiveReqState::Excluded {
                by: 3,
                because: ReqExcludeCause::Carried,
            },
            "◇",
        ),
    ] {
        for _depth in 1..=16 {
            assert_eq!(req_glyph(state), expected, "{state:?}");
        }
        assert_eq!(state.as_str(), state.name(), "{state:?}");
    }
    assert_eq!(EffectiveReqState::Pending.as_str(), "pending");
    assert_eq!(EffectiveReqState::Met.as_str(), "met");
    assert_eq!(EffectiveReqState::Moot.as_str(), "moot");
    assert_eq!(EffectiveReqState::Dropped.as_str(), "dropped");
    assert_eq!(EffectiveReqState::Carried.as_str(), "carried");
    assert_eq!(
        EffectiveReqState::Excluded {
            by: 0,
            because: ReqExcludeCause::Dropped,
        }
        .as_str(),
        "excluded"
    );
}
