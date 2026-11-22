//! Atomic carry/drop write pass (RV `writes` family).
//!
//! These tests pin the exact bytes `plan_req_writes` produces by reusing
//! `task_split::carry_req` and `task_block_layout::writers::drop_req`,
//! with no alternate writers and no separate commit path.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::{
    req_verdicts::{resolve_verdicts, ReqVerdict, ReqVerdictRequest},
    req_writes::plan_req_writes,
};
use super::req_close_tests::{collect_case, vectors};
use crate::native::{
    note_tasks, task_move::VaultSnapshot, task_split::plan::SplitSettings,
};

fn task_settings() -> note_tasks::NoteTaskSettings {
    note_tasks::read_settings(Path::new("/no-bob-vault"))
}

fn split_settings() -> SplitSettings {
    SplitSettings {
        linked_from_lines: true,
        linked_from_mentions: true,
        inbox_route: "inbox.md".to_string(),
    }
}

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 22).expect("valid date")
}

fn snapshot_for(
    case: &serde_json::Value,
) -> (
    Vec<crate::native::task_complete::req_close::ReqCloseOwner>,
    VaultSnapshot,
) {
    let (owners, _) = collect_case(case, &task_settings());
    let mut notes = std::collections::BTreeMap::new();
    for note in case["notes"].as_array().expect("notes") {
        let path = note["path"].as_str().expect("path");
        let lines = note["source_lines"]
            .as_array()
            .expect("source_lines")
            .iter()
            .map(|line| line.as_str().expect("line"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        notes.insert(PathBuf::from(path), lines);
    }
    (owners, VaultSnapshot { notes })
}

fn row_key_for(
    groups: &[crate::native::task_complete::req_close::ReqCloseGroup],
    path: &str,
) -> String {
    groups
        .iter()
        .flat_map(|group| &group.rows)
        .find(|row| row.path == path)
        .unwrap_or_else(|| panic!("missing requirement path {path}"))
        .offer
        .key
        .clone()
}

fn resolve(
    owners: &[crate::native::task_complete::req_close::ReqCloseOwner],
    requests: Vec<ReqVerdictRequest>,
) -> crate::native::task_complete::req_verdicts::ReqVerdictSelection {
    resolve_verdicts(owners, &[], &requests, None).expect("verdicts resolve")
}

fn case<'a>(id: &str) -> serde_json::Value {
    let all = vectors();
    all["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing {id}"))
        .clone()
}

#[test]
fn writes_depth_one_carry_moves_branch_and_reports_receipt() {
    let base = case("RV1");
    let (owners, snapshot) = snapshot_for(&base);
    let (_, groups) = collect_case(&base, &task_settings());
    let key = row_key_for(&groups, "1.2");
    let selection = resolve(
        &owners,
        vec![ReqVerdictRequest {
            selector: key.clone(),
            verdict: ReqVerdict::Carry,
            reason: None,
        }],
    );
    let plan = plan_req_writes(
        &snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect("depth-1 carry plans");
    assert_eq!(plan.outcomes.len(), 1);
    let outcome = &plan.outcomes[0];
    assert_eq!(outcome.row_key, key);
    assert_eq!(outcome.verdict, ReqVerdict::Carry);
    assert!(!outcome.pulled);
    assert_eq!(
        outcome.receipt,
        "→ “Handles empty input” is its own task now"
    );
    let carried = outcome.carried.as_ref().expect("carried task");
    assert!(!carried.block_id.is_empty());
    assert_eq!(carried.path, "Tasks.md");
    // The stub stays behind and the new task lands below the owner.
    let post = plan
        .postimages
        .get(&PathBuf::from("Tasks.md"))
        .expect("Tasks.md postimage");
    assert!(post.contains("[>]"), "carry leaves a [>] stub");
    assert!(post.contains(&format!("^{}", carried.block_id)));
    // One branch carried: the other three pending rows stay Met.
    assert_eq!(plan.met.len(), 1);
    assert_eq!(plan.met[0].count, 3);
    assert_eq!(
        plan.met[0].receipt,
        "✓ 3 requirements met with “Ship the exporter”"
    );
}

#[test]
fn writes_nested_leaf_carry_maps_no_descendant_moves() {
    let base = case("RV1");
    let (owners, snapshot) = snapshot_for(&base);
    let (_, groups) = collect_case(&base, &task_settings());
    let key = row_key_for(&groups, "1.1.1");
    let selection = resolve(
        &owners,
        vec![ReqVerdictRequest {
            selector: key.clone(),
            verdict: ReqVerdict::Carry,
            reason: None,
        }],
    );
    let plan = plan_req_writes(
        &snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect("leaf carry plans");
    assert_eq!(plan.outcomes.len(), 1);
    assert!(plan.outcomes[0].moves.is_empty());
    assert_eq!(
        plan.outcomes[0].receipt,
        "→ “Preserves CRLF” is its own task now"
    );
    assert_eq!(plan.met[0].count, 3);
}

#[test]
fn writes_branch_carry_covers_descendants_without_met_double_count() {
    let base = case("RV1");
    let (owners, snapshot) = snapshot_for(&base);
    let (_, groups) = collect_case(&base, &task_settings());
    // Depth-1 branch with three pending descendants.
    let key = row_key_for(&groups, "1");
    let selection = resolve(
        &owners,
        vec![ReqVerdictRequest {
            selector: key.clone(),
            verdict: ReqVerdict::Carry,
            reason: None,
        }],
    );
    let plan = plan_req_writes(
        &snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect("branch carry plans");
    assert_eq!(plan.outcomes.len(), 1);
    // The depth-1 branch owns itself plus its three pending descendants, so
    // nothing remains Met.
    assert!(plan.met.is_empty());
    assert!(!plan.outcomes[0].moves.is_empty());
}

#[test]
fn writes_disjoint_mixed_carry_and_drop_are_atomic() {
    let base = case("RV1");
    let (owners, snapshot) = snapshot_for(&base);
    let (_, groups) = collect_case(&base, &task_settings());
    let carry_key = row_key_for(&groups, "1.1.1");
    let drop_key = row_key_for(&groups, "1.2");
    let selection = resolve(
        &owners,
        vec![
            ReqVerdictRequest {
                selector: carry_key.clone(),
                verdict: ReqVerdict::Carry,
                reason: None,
            },
            ReqVerdictRequest {
                selector: drop_key.clone(),
                verdict: ReqVerdict::Drop,
                reason: Some("out of scope".to_string()),
            },
        ],
    );
    let plan = plan_req_writes(
        &snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect("mixed carry/drop plans");
    assert_eq!(plan.outcomes.len(), 2);
    let post = plan
        .postimages
        .get(&PathBuf::from("Tasks.md"))
        .expect("postimage");
    assert!(post.contains("[-] Handles empty input"));
    assert!(post.contains("out of scope"));
    assert!(post.contains("[>]"));
    // Two disjoint branches covered; the remaining two pending rows are Met.
    assert_eq!(plan.met[0].count, 2);
    let receipts = plan
        .outcomes
        .iter()
        .map(|outcome| outcome.receipt.clone())
        .collect::<Vec<_>>();
    assert!(receipts
        .iter()
        .any(|receipt| receipt.contains("out of scope")));
    assert!(receipts
        .iter()
        .any(|receipt| receipt.contains("is its own task")));
}

#[test]
fn writes_drop_with_optional_cancel_log() {
    let base = case("RV1");
    let (owners, snapshot) = snapshot_for(&base);
    let (_, groups) = collect_case(&base, &task_settings());
    let key = row_key_for(&groups, "1.2");
    // Empty reason writes no log entry.
    let selection = resolve(
        &owners,
        vec![ReqVerdictRequest {
            selector: key.clone(),
            verdict: ReqVerdict::Drop,
            reason: None,
        }],
    );
    let plan = plan_req_writes(
        &snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect("reason-less drop plans");
    let post = plan
        .postimages
        .get(&PathBuf::from("Tasks.md"))
        .expect("postimage");
    assert!(post.contains("[-] Handles empty input"));
    assert!(!post.contains("CANCEL LOG"));
    assert_eq!(plan.outcomes[0].receipt, "⊘ Dropped “Handles empty input”");
    // A reason lands in a Cancel Log as the req's first child.
    let selection = resolve(
        &owners,
        vec![ReqVerdictRequest {
            selector: key.clone(),
            verdict: ReqVerdict::Drop,
            reason: Some("  duplicate   effort ".to_string()),
        }],
    );
    let plan = plan_req_writes(
        &snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect("reasoned drop plans");
    let post = plan
        .postimages
        .get(&PathBuf::from("Tasks.md"))
        .expect("postimage");
    assert!(post.contains("❌ **CANCEL LOG**"));
    assert!(post.contains("*2026-11-22* — duplicate effort"));
    assert_eq!(
        plan.outcomes[0].receipt,
        "⊘ Dropped “Handles empty input” · “duplicate effort”"
    );
}

#[test]
fn writes_met_only_changes_no_bytes() {
    let base = case("RV1");
    let (owners, snapshot) = snapshot_for(&base);
    let selection = resolve(&owners, Vec::new());
    let plan = plan_req_writes(
        &snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect("met-only plans");
    assert!(plan.postimages.is_empty());
    assert!(plan.outcomes.is_empty());
    assert_eq!(plan.met.len(), 1);
    assert_eq!(plan.met[0].count, 4);
    assert_eq!(
        plan.met[0].receipt,
        "✓ 4 requirements met with “Ship the exporter”"
    );
}

#[test]
fn writes_notes_with_child_reqs_file_notes_into_new_task() {
    let contents = "- [ ] #task Research the exporter ^research\n\t- [ ] Draft the plan\n\t\t- shared note\n\t- [ ] Benchmarks\n";
    let settings = task_settings();
    let scan = note_tasks::scan(contents, &settings);
    let task = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some("research"))
        .expect("owner")
        .clone();
    let owner = crate::native::task_complete::req_close::collect_owner(
        contents,
        Path::new("Tasks.md"),
        &task,
        &settings,
    )
    .expect("owner");
    let snapshot = VaultSnapshot::from_notes([(
        PathBuf::from("Tasks.md"),
        contents.to_string(),
    )]);
    let groups = crate::native::task_complete::req_close::groups_from_owners(
        std::slice::from_ref(&owner),
    );
    let key = groups[0]
        .rows
        .iter()
        .find(|row| row.text == "Draft the plan")
        .expect("draft row")
        .offer
        .key
        .clone();
    let selection = resolve(
        std::slice::from_ref(&owner),
        vec![ReqVerdictRequest {
            selector: key,
            verdict: ReqVerdict::Carry,
            reason: None,
        }],
    );
    let plan = plan_req_writes(
        &snapshot,
        &[owner],
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &settings,
    )
    .expect("note-carry plans");
    let post = plan
        .postimages
        .get(&PathBuf::from("Tasks.md"))
        .expect("postimage");
    // The carried branch keeps its note; NOTES appears only because a note
    // lands in it.
    assert!(post.contains("NOTES") || post.contains("shared note"));
}

#[test]
fn writes_anchor_reuse_keeps_existing_block_id() {
    let contents = "- [ ] #task Ship the exporter ^export\n\t- [ ] Handles input ^handles\n";
    let settings = task_settings();
    let scan = note_tasks::scan(contents, &settings);
    let task = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some("export"))
        .expect("owner")
        .clone();
    let owner = crate::native::task_complete::req_close::collect_owner(
        contents,
        Path::new("Tasks.md"),
        &task,
        &settings,
    )
    .expect("owner");
    let snapshot = VaultSnapshot::from_notes([(
        PathBuf::from("Tasks.md"),
        contents.to_string(),
    )]);
    let groups = crate::native::task_complete::req_close::groups_from_owners(
        std::slice::from_ref(&owner),
    );
    let key = groups[0].rows[0].offer.key.clone();
    let selection = resolve(
        std::slice::from_ref(&owner),
        vec![ReqVerdictRequest {
            selector: key,
            verdict: ReqVerdict::Carry,
            reason: None,
        }],
    );
    let plan = plan_req_writes(
        &snapshot,
        &[owner],
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &settings,
    )
    .expect("anchored carry plans");
    let carried = plan.outcomes[0].carried.as_ref().expect("carried");
    assert_eq!(carried.block_id, "handles");
}

#[test]
fn writes_stale_replay_after_edit_refuses_without_partial_bytes() {
    let base = case("RV1");
    let (owners, snapshot) = snapshot_for(&base);
    let (_, groups) = collect_case(&base, &task_settings());
    let carry_key = row_key_for(&groups, "1.1.1");
    let drop_key = row_key_for(&groups, "1.2");
    let selection = resolve(
        &owners,
        vec![
            ReqVerdictRequest {
                selector: carry_key,
                verdict: ReqVerdict::Carry,
                reason: None,
            },
            ReqVerdictRequest {
                selector: drop_key,
                verdict: ReqVerdict::Drop,
                reason: None,
            },
        ],
    );
    // Edit the drop row's text between preview and submit.
    let mut edited = snapshot.notes.clone();
    let edited_text = edited
        .get(&PathBuf::from("Tasks.md"))
        .expect("note")
        .replace("Handles empty input", "Handles renamed input");
    edited.insert(PathBuf::from("Tasks.md"), edited_text);
    let edited_snapshot = VaultSnapshot { notes: edited };
    let before = edited_snapshot.notes.clone();
    let error = plan_req_writes(
        &edited_snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect_err("stale edit must refuse");
    assert_eq!(error.code(), "req_close_stale");
    // No separate commit path: a refusal returns no postimages at all, so a
    // caller staging only the returned map writes nothing.
    assert_eq!(edited_snapshot.notes, before);
}

#[test]
fn writes_missing_destination_file_refuses_as_stale() {
    let base = case("RV1");
    let (owners, _) = snapshot_for(&base);
    let (_, groups) = collect_case(&base, &task_settings());
    let key = row_key_for(&groups, "1.2");
    let selection = resolve(
        &owners,
        vec![ReqVerdictRequest {
            selector: key,
            verdict: ReqVerdict::Drop,
            reason: None,
        }],
    );
    let empty = VaultSnapshot {
        notes: std::collections::BTreeMap::new(),
    };
    let error = plan_req_writes(
        &empty,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect_err("missing file must refuse");
    assert_eq!(error.code(), "req_close_stale");
}

#[test]
fn writes_carry_pulled_marks_pulled_receipt() {
    let base = case("RV1");
    let (owners, snapshot) = snapshot_for(&base);
    let (_, groups) = collect_case(&base, &task_settings());
    let key = row_key_for(&groups, "1.2");
    let selection = resolve(
        &owners,
        vec![ReqVerdictRequest {
            selector: key,
            verdict: ReqVerdict::CarryPulled,
            reason: None,
        }],
    );
    let plan = plan_req_writes(
        &snapshot,
        &owners,
        &selection,
        "2026-11-22",
        date(),
        &split_settings(),
        &task_settings(),
    )
    .expect("carry-pulled plans");
    assert!(plan.outcomes[0].pulled);
    assert_eq!(
        plan.outcomes[0].receipt,
        "→ “Handles empty input” is its own task now · pulled in"
    );
}

#[test]
fn writes_vectors_match_pinned_bytes() {
    let all = vectors();
    let cases: Vec<_> = all["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|c| c["family"] == "writes")
        .collect();
    assert_eq!(cases.len(), 9);
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(
            case["id"],
            format!("RV{}", index + 20),
            "writes ids stay dense"
        );
    }
    for pinned in cases {
        let id = pinned["id"].as_str().unwrap();
        // Build owners + snapshot from base_case or inline notes.
        let (owners, snapshot) = if let Some(base_id) =
            pinned["input"]["base_case"].as_str()
        {
            let base = case(base_id);
            // Writes inputs never rewrite the base notes.
            assert!(
                pinned["input"].get("notes").is_none(),
                "{id} uses base_case"
            );
            snapshot_for(&base)
        } else {
            let notes = pinned["input"]["notes"].as_array().unwrap();
            let mut owners = Vec::new();
            let mut files = std::collections::BTreeMap::new();
            for note in notes {
                let path = note["path"].as_str().unwrap();
                let lines = note["source_lines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| l.as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
                files.insert(PathBuf::from(path), lines.clone());
                let scan = note_tasks::scan(&lines, &task_settings());
                for spec in note["closing"].as_array().unwrap() {
                    let task = if let Some(block_id) = spec["block_id"].as_str()
                    {
                        match scan.by_block_id(block_id) {
                            note_tasks::BlockIdLookup::Found(t) => t.clone(),
                            other => panic!("{id} block {block_id}: {other:?}"),
                        }
                    } else {
                        let line = spec["line"].as_u64().unwrap() as usize;
                        scan.task_at(line - 1)
                            .unwrap_or_else(|| panic!("{id} no task at {line}"))
                            .clone()
                    };
                    if let Some(owner) =
                        crate::native::task_complete::req_close::collect_owner(
                            &lines,
                            Path::new(path),
                            &task,
                            &task_settings(),
                        )
                    {
                        owners.push(owner);
                    }
                }
            }
            (owners, VaultSnapshot { notes: files })
        };
        let requests = pinned["input"]["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let verdict = match r["verdict"].as_str().unwrap() {
                    "met" => ReqVerdict::Met,
                    "drop" => ReqVerdict::Drop,
                    "carry" => ReqVerdict::Carry,
                    "carry_pulled" => ReqVerdict::CarryPulled,
                    other => panic!("{id} unknown verdict {other}"),
                };
                ReqVerdictRequest {
                    selector: r["selector"].as_str().unwrap().to_string(),
                    verdict,
                    reason: r["reason"].as_str().map(str::to_string),
                }
            })
            .collect::<Vec<_>>();
        let today = pinned["input"]["today"].as_str().unwrap_or("2026-11-22");
        let selection = resolve(&owners, requests);
        let plan = plan_req_writes(
            &snapshot,
            &owners,
            &selection,
            today,
            date(),
            &split_settings(),
            &task_settings(),
        )
        .unwrap_or_else(|e| panic!("{id} plans: {} {}", e.code(), e));
        // Postimages exact bytes.
        let expected_images =
            pinned["expect"]["postimages"].as_object().unwrap();
        assert_eq!(
            plan.postimages.len(),
            expected_images.len(),
            "{id} postimage count"
        );
        for (path, expected) in expected_images {
            let actual = plan
                .postimages
                .get(&PathBuf::from(path))
                .unwrap_or_else(|| panic!("{id} missing {path}"));
            assert_eq!(
                actual,
                expected.as_str().unwrap(),
                "{id} bytes for {path}"
            );
        }
        // Outcomes receipts and identities.
        let expected_outcomes =
            pinned["expect"]["outcomes"].as_array().unwrap();
        assert_eq!(
            plan.outcomes.len(),
            expected_outcomes.len(),
            "{id} outcome count"
        );
        for (actual, expected) in plan.outcomes.iter().zip(expected_outcomes) {
            assert_eq!(
                actual.row_key,
                expected["row_key"].as_str().unwrap(),
                "{id} row key"
            );
            assert_eq!(
                actual.verdict.as_str(),
                expected["verdict"].as_str().unwrap(),
                "{id} verdict"
            );
            assert_eq!(
                actual.receipt,
                expected["receipt"].as_str().unwrap(),
                "{id} receipt"
            );
            match (actual.carried.as_ref(), expected["carried"].clone()) {
                (None, serde_json::Value::Null) => {}
                (Some(actual_c), expected_c) => {
                    assert_eq!(
                        actual_c.path,
                        expected_c["path"].as_str().unwrap(),
                        "{id} carried path"
                    );
                    assert_eq!(
                        actual_c.line,
                        expected_c["line"].as_u64().unwrap() as usize,
                        "{id} carried line"
                    );
                    assert_eq!(
                        actual_c.block_id,
                        expected_c["block_id"].as_str().unwrap(),
                        "{id} carried id"
                    );
                }
                (None, other) => panic!("{id} unexpected carried {other:?}"),
            }
        }
        let expected_met = pinned["expect"]["met"].as_array().unwrap();
        assert_eq!(plan.met.len(), expected_met.len(), "{id} met count");
        for (actual, expected) in plan.met.iter().zip(expected_met) {
            assert_eq!(
                actual.owner_key,
                expected["owner_key"].as_str().unwrap(),
                "{id} met owner"
            );
            assert_eq!(
                actual.count,
                expected["count"].as_u64().unwrap() as usize,
                "{id} met count"
            );
            assert_eq!(
                actual.receipt,
                expected["receipt"].as_str().unwrap(),
                "{id} met receipt"
            );
        }
    }
}
