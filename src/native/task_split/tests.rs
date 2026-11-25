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
                let tasks =
                    plan.tasks.iter().map(vector_task).collect::<Vec<_>>();
                assert_eq!(json!(tasks), case["expect"]["tasks"], "{id} tasks");
                if let Some(stubs) = case["expect"].get("stubs") {
                    let actual = plan
                        .stubs
                        .iter()
                        .map(|stub| {
                            json!({
                                "path": stub.path,
                                "line": stub.line,
                                "text": stub.text,
                            })
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(&json!(actual), stubs, "{id} stubs");
                }
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

/// A planned task as the vectors pin it. Carried requirements add their
/// depth and move map; plain splits keep the original five fields.
fn vector_task(task: &super::plan::SplitTask) -> Value {
    let mut value = json!({
        "path": task.path,
        "block_id": task.block_id,
        "title": task.title,
        "anchor_reused": task.anchor_reused,
        "child_lines": task.child_lines,
    });
    if let Some(depth) = task.req_depth {
        value["req_depth"] = json!(depth);
        value["moves"] = json!(task
            .moves
            .iter()
            .map(
                |line_move| json!({"from": line_move.from, "to": line_move.to})
            )
            .collect::<Vec<_>>());
    }
    value
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

fn carry(
    contents: &str,
    target: super::carry::ReqRef<'_>,
) -> Result<super::carry::CarryOutcome, super::plan::SplitError> {
    let snapshot = VaultSnapshot::from_notes([(
        PathBuf::from("db.md"),
        contents.to_string(),
    )]);
    super::carry::carry_req(
        &snapshot,
        &PathBuf::from("db.md"),
        1,
        target,
        "2026-11-21",
        &settings(),
        &task_settings(),
    )
}

const CARRY_OWNER: &str =
    "- [ ] #task Ship the exporter [created:: 2026-10-20] ^export
\t- [ ] Handles supported inputs
\t\t- [ ] Handles Windows files
\t\t\t- [ ] Preserves CRLF
\t\t\t- Fixture: windows.csv
\t\t- [ ] Handles Windows files
\t- [ ] Benchmarks within 5% of v1
- [ ] #task Write the parser [created:: 2026-10-19] ^parser
";

#[test]
fn carry_req_by_row_key_moves_the_branch_and_maps_descendants() {
    // Two reqs share the label "Handles Windows files"; the row key's path
    // picks the first branch, never the duplicate leaf.
    let key = crate::native::task_reqs::req_row_key(
        "db.md#^export",
        &[1, 1],
        "Handles Windows files",
    );
    let outcome = carry(CARRY_OWNER, super::carry::ReqRef::RowKey(&key))
        .unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(outcome.path, "db.md");
    assert_eq!(outcome.block_id, "handles-windows");
    assert_eq!(
        outcome.plan.postimages[&PathBuf::from("db.md")],
        "- [ ] #task Ship the exporter [created:: 2026-10-20] ^export
\t- [ ] Handles supported inputs
\t\t- [>] [[#^handles-windows|Handles Windows files]]
\t\t- [ ] Handles Windows files
\t- [ ] Benchmarks within 5% of v1
- [ ] #task Handles Windows files [created:: 2026-11-21] ^handles-windows
\t- 🪢 **LINKED FROM:** [[#^export]] (💬 mentions)
\t- [ ] Preserves CRLF
\t- **Notes**
\t\t- Fixture: windows.csv
- [ ] #task Write the parser [created:: 2026-10-19] ^parser
"
    );
    assert_eq!(outcome.line, 6);
    let moves: Vec<(usize, usize)> = outcome
        .moves
        .iter()
        .map(|line_move| (line_move.from, line_move.to))
        .collect();
    assert_eq!(moves, vec![(4, 8), (5, 10)]);
}

#[test]
fn carry_req_by_line_digest_and_its_refusals() {
    let digest = |line: usize| {
        note_tasks::line_digest(CARRY_OWNER.lines().nth(line - 1).unwrap())
    };
    let leaf = digest(6);
    let outcome = carry(
        CARRY_OWNER,
        super::carry::ReqRef::LineDigest {
            line: 6,
            digest: &leaf,
        },
    )
    .unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    // The leaf sits after the first branch; its task lands below the
    // owner's whole block.
    assert_eq!(outcome.line, 8);
    assert!(outcome.moves.is_empty());
    assert_eq!(
        outcome.plan.stubs[0].text,
        "- [>] [[#^handles-windows|Handles Windows files]]"
    );

    let stale = carry(
        CARRY_OWNER,
        super::carry::ReqRef::LineDigest {
            line: 6,
            digest: "00000000",
        },
    )
    .unwrap_err();
    assert_eq!(stale.code, "split_stale_ref");

    let stale_key = carry(
        CARRY_OWNER,
        super::carry::ReqRef::RowKey("db.md#^export/req/1.3~00000000"),
    )
    .unwrap_err();
    assert_eq!(stale_key.code, "split_stale_ref");

    let note = "- [ ] #task Ship the exporter [created:: 2026-10-20] ^export
  - Morgan reported the original issue.
";
    let note_digest = note_tasks::line_digest(note.lines().nth(1).unwrap());
    let refused = carry(
        note,
        super::carry::ReqRef::LineDigest {
            line: 2,
            digest: &note_digest,
        },
    )
    .unwrap_err();
    assert_eq!(refused.code, "not_a_req");
}

#[test]
fn req_candidates_list_every_depth_with_their_state() {
    let snapshot = VaultSnapshot::from_notes([(
        PathBuf::from("db.md"),
        "- [ ] #task Ship the exporter [created:: 2026-10-20] ^export
  - [ ] Handles supported inputs
    - [-] Handles Windows files
      - [ ] Preserves CRLF
  - Morgan reported the original issue.
"
        .to_string(),
    )]);
    let candidates = super::plan::list_task_split_candidates(
        &snapshot,
        &SplitRequest {
            owner_path: PathBuf::from("db.md"),
            // A req line names its root owner.
            owner_line: 3,
            selection: SplitSelection::Cursor { line: 0 },
        },
        &task_settings(),
    )
    .unwrap();
    let rows: Vec<Value> = candidates
        .iter()
        .map(|candidate| {
            json!([
                candidate.ordinal,
                candidate.line,
                candidate.title,
                candidate.req.as_ref().map(|req| json!([
                    req.depth,
                    req.mark,
                    req.state,
                    req.carriable
                ])),
            ])
        })
        .collect();
    assert_eq!(
        json!(rows),
        json!([
            [
                1,
                2,
                "Handles supported inputs",
                [1, "open", "pending", true]
            ],
            [
                2,
                3,
                "Handles Windows files",
                [2, "dropped", "dropped", false]
            ],
            [3, 4, "Preserves CRLF", [3, "open", "excluded", false]],
            [4, 5, "Morgan reported the original issue.", null],
        ])
    );
}

#[test]
fn command_settle_leaves_split_with_nothing_note_local() {
    // Fixture proof for phase command-settle: the settled split postimages
    // (source owner plus the new inbox task) are a settle fixed point, so a
    // follow-up reconcile plans nothing note-local.
    let source = "---\ntype: [[area]]\n---\n## Tasks\n\n- [ ] #task Big one ^owner\n  - [ ] First req\n  - [ ] Second req\n- [ ] #task Future [scheduled:: 2099-01-01] ^future\n";
    let notes = [("work.md", source), ("inbox.md", "## Tasks\n")];
    let planned = split(&notes, "work.md", 6, SplitSelection::Nth(vec![1]));
    let mut postimages = planned.postimages.clone();
    let fired = crate::native::task_settle::tests::assert_command_settle_holds(
        &mut postimages,
    );
    assert!(fired, "settle must regroup the split source");
    let settled_src = postimages
        .get(&PathBuf::from("work.md"))
        .expect("source postimage");
    assert!(settled_src.contains("- [?] #task Future"), "{settled_src}");
    // The split child stays in the owner note with its managed back-link.
    assert!(
        settled_src.contains("- [ ] #task First req"),
        "{settled_src}"
    );
}
