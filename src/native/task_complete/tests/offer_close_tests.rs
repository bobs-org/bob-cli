use super::super::offer_close::{
    describe_offer_close, plan_offer_close, OfferCloseAction, OfferCloseInput,
    OfferCloseRefusal,
};
use super::*;
use crate::native::note_tasks;
use chrono::NaiveDate;

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 5).expect("valid test date")
}

fn index(
    snapshot: &[(PathBuf, String)],
) -> crate::native::vault_links::NoteIndex {
    crate::native::vault_links::NoteIndex::from_paths(
        snapshot.iter().map(|(path, _)| path.clone()),
    )
}

fn input<'a>(
    key: &'a str,
    path: &'a Path,
    line_index: usize,
    expected_line: &'a str,
    source_text: &'a str,
    group: &'a str,
    snapshot: &'a [(PathBuf, String)],
    settings: &'a TasksSettings,
    index: &'a crate::native::vault_links::NoteIndex,
) -> OfferCloseInput<'a> {
    OfferCloseInput {
        key,
        path,
        line_index,
        expected_line,
        source_text,
        group,
        is_new_task: false,
        muted: false,
        today: date(),
        tasks_settings: settings,
        note_settings: settings,
        snapshot,
        snapshot_complete: true,
        day_relative: Path::new("today.md"),
        index,
    }
}

#[test]
fn done_is_root_only_and_descriptor_reports_open_child_scope() {
    let source = concat!(
        "- [ ] #task Parent ^parent\n",
        "  - [ ] #task Child ^child\n",
    );
    let path = PathBuf::from("Tasks.md");
    let snapshot = vec![(path.clone(), source.to_string())];
    let note_index = index(&snapshot);
    let settings = test_settings();
    let input = input(
        "Tasks.md#^parent",
        &path,
        0,
        "- [ ] #task Parent ^parent",
        source,
        "hand_off",
        &snapshot,
        &settings,
        &note_index,
    );

    let descriptor = describe_offer_close(&input);
    assert!(descriptor.complete && descriptor.cancel);
    assert!(descriptor.refusal.is_none());
    assert_eq!(
        descriptor.complete_consequence.as_deref(),
        Some("✓ Completes “Parent” · Ready → Done")
    );
    assert_eq!(
        descriptor.scope_note.as_deref(),
        Some("· its 1 open subtask stays open")
    );

    let plan = plan_offer_close(&input, OfferCloseAction::Done)
        .expect("eligible root closes");
    assert_eq!(
        plan.source_postimage,
        concat!(
            "- [x] #task Parent  [completion:: 2026-11-05] ^parent\n",
            "  - [ ] #task Child ^child\n",
        )
    );
}

#[test]
fn cancel_reason_and_blank_reason_follow_task_card_log_policy() {
    let path = PathBuf::from("Tasks.md");
    let source = concat!(
        "- [ ] #task Retire hook ^retire\n",
        "  - ❌ **CANCEL LOG**\n",
        "    - *2026-10-01* — earlier reason\n",
    );
    let snapshot = vec![(path.clone(), source.to_string())];
    let note_index = index(&snapshot);
    let settings = test_settings();
    let card_input = input(
        "Tasks.md#^retire",
        &path,
        0,
        "- [ ] #task Retire hook ^retire",
        source,
        "hand_off",
        &snapshot,
        &settings,
        &note_index,
    );
    let plan = plan_offer_close(
        &card_input,
        OfferCloseAction::Cancel {
            reason: "  duplicate\n implementation  ".to_string(),
        },
    )
    .expect("eligible task cancels");
    assert_eq!(
        plan.source_postimage,
        concat!(
            "- [-] #task Retire hook [cancelled:: 2026-11-05] ^retire\n",
            "  - ❌ **CANCEL LOG**\n",
            "    - *2026-11-05* — duplicate implementation\n",
            "    - *2026-10-01* — earlier reason\n",
        )
    );
    assert_eq!(plan.reason.as_deref(), Some("duplicate implementation"));

    let blank = plan_offer_close(
        &card_input,
        OfferCloseAction::Cancel {
            reason: " \n ".to_string(),
        },
    )
    .expect("blank reason remains a valid cancel");
    assert!(blank
        .source_postimage
        .contains("*2026-11-05* — 🤷 no reason given"));

    let without_log = "- [ ] #task Retire hook ^retire\n";
    let no_log_snapshot = vec![(path.clone(), without_log.to_string())];
    let no_log_index = index(&no_log_snapshot);
    let no_log_input = input(
        "Tasks.md#^retire",
        &path,
        0,
        "- [ ] #task Retire hook ^retire",
        without_log,
        "hand_off",
        &no_log_snapshot,
        &settings,
        &no_log_index,
    );
    let authored = plan_offer_close(
        &no_log_input,
        OfferCloseAction::Cancel {
            reason: "retire duplicate".to_string(),
        },
    )
    .expect("authored reason creates a log");
    assert!(authored.source_postimage.contains("CANCEL LOG"));
    assert!(authored
        .source_postimage
        .contains("*2026-11-05* — retire duplicate"));
}

#[test]
fn blank_cancel_without_a_log_does_not_create_one() {
    let path = PathBuf::from("Tasks.md");
    let source = "- [ ] #task Retire hook ^retire\n";
    let snapshot = vec![(path.clone(), source.to_string())];
    let note_index = index(&snapshot);
    let settings = test_settings();
    let input = input(
        "Tasks.md#^retire",
        &path,
        0,
        "- [ ] #task Retire hook ^retire",
        source,
        "hand_off",
        &snapshot,
        &settings,
        &note_index,
    );
    let plan = plan_offer_close(
        &input,
        OfferCloseAction::Cancel {
            reason: String::new(),
        },
    )
    .expect("eligible task cancels");
    assert_eq!(
        plan.source_postimage,
        "- [-] #task Retire hook [cancelled:: 2026-11-05] ^retire\n"
    );
}

#[test]
fn done_recovers_dependencies_without_building_a_second_offer_round() {
    let path = PathBuf::from("Tasks.md");
    let source = concat!(
        "- [ ] #task Root [id:: root] ^root\n",
        "- [?] #task Waiting [fresh:: 2026-11-01] [dependsOn:: root] [id:: waiting] ^waiting\n",
    );
    let snapshot = vec![(path.clone(), source.to_string())];
    let note_index = index(&snapshot);
    let settings = test_settings();
    let input = input(
        "Tasks.md#^root",
        &path,
        0,
        "- [ ] #task Root [id:: root] ^root",
        source,
        "hand_off",
        &snapshot,
        &settings,
        &note_index,
    );
    let descriptor = describe_offer_close(&input);
    assert_eq!(descriptor.unblocks, 1);
    assert_eq!(
        descriptor.complete_consequence.as_deref(),
        Some(
            "✓ Completes “Root” · Ready → Done · unblocks “Waiting” · back tomorrow as a tickler"
        )
    );

    let plan = plan_offer_close(&input, OfferCloseAction::Done)
        .expect("eligible root closes");
    assert_eq!(plan.recovery.unblocked.len(), 1);
    assert_eq!(plan.recovery.unblocked[0].block_id, "waiting");
    let recovered = plan
        .recovery
        .changed_files
        .get(&path)
        .expect("the dependent note changes");
    assert!(recovered.contains(
        "- [ ] #task Waiting [fresh:: 2026-11-01] [unblocked:: 2026-11-05] [dependsOn:: root] [id:: waiting] ^waiting"
    ), "{recovered}");
    assert!(plan.recovery.unblocked[0].offer.is_none());
}

#[test]
fn idless_offer_key_resolves_without_minting_a_block_id() {
    let path = PathBuf::from("Tasks.md");
    let source = "- [ ] #task Idless task\n";
    let snapshot = vec![(path.clone(), source.to_string())];
    let note_index = index(&snapshot);
    let settings = test_settings();
    let key =
        note_tasks::stable_task_key(&path, None, "Idless task", "#task", 1);
    let input = input(
        &key,
        &path,
        0,
        "- [ ] #task Idless task",
        source,
        "hand_off",
        &snapshot,
        &settings,
        &note_index,
    );
    let plan = plan_offer_close(&input, OfferCloseAction::Done)
        .expect("digest offer closes in place");
    assert!(plan.block_id.is_empty());
    assert_eq!(
        plan.source_postimage,
        "- [x] #task Idless task  [completion:: 2026-11-05]\n"
    );
}

#[test]
fn idless_key_survives_same_batch_id_mint_and_note_move() {
    let old_path = PathBuf::from("Old.md");
    let path = PathBuf::from("Moved.md");
    let source = "- [ ] #task Idless task [id:: minted] ^minted\n";
    let snapshot = vec![(path.clone(), source.to_string())];
    let note_index = index(&snapshot);
    let settings = test_settings();
    let key =
        note_tasks::stable_task_key(&old_path, None, "Idless task", "#task", 1);
    let input = input(
        &key,
        &path,
        0,
        "- [ ] #task Idless task [id:: minted] ^minted",
        source,
        "hand_off",
        &snapshot,
        &settings,
        &note_index,
    );
    let plan = plan_offer_close(&input, OfferCloseAction::Done)
        .expect("frozen idless key resolves after mint and move");
    assert!(plan
        .source_postimage
        .starts_with("- [x] #task Idless task [id:: minted]"));
}

#[test]
fn stale_line_and_ineligible_rows_are_refused_with_reasons() {
    let path = PathBuf::from("Tasks.md");
    let expected = "- [ ] #task Root ^root";
    let old = format!("{expected}\n");
    let changed = "- [ ] #task Changed root ^root\n".to_string();
    let snapshot = vec![(path.clone(), changed)];
    let note_index = index(&snapshot);
    let settings = test_settings();
    let stale = input(
        "Tasks.md#^root",
        &path,
        0,
        expected,
        &old,
        "hand_off",
        &snapshot,
        &settings,
        &note_index,
    );
    assert_eq!(
        plan_offer_close(&stale, OfferCloseAction::Done),
        Err(OfferCloseRefusal::ChangedIdentity)
    );

    let recurring = "- [ ] #task Weekly [repeat:: weekly] ^weekly\n";
    let snapshot = vec![(path.clone(), recurring.to_string())];
    let note_index = index(&snapshot);
    let recurring_input = input(
        "Tasks.md#^weekly",
        &path,
        0,
        "- [ ] #task Weekly [repeat:: weekly] ^weekly",
        recurring,
        "hand_off",
        &snapshot,
        &settings,
        &note_index,
    );
    let recurring_descriptor = describe_offer_close(&recurring_input);
    assert_eq!(
        recurring_descriptor.refusal,
        Some(OfferCloseRefusal::Recurring)
    );
    assert_eq!(
        recurring_descriptor.why.as_deref(),
        Some("“Weekly” repeats — close it in Obsidian so Tasks writes the next one")
    );

    let new_task_input = OfferCloseInput {
        is_new_task: true,
        ..recurring_input
    };
    let new_descriptor = describe_offer_close(&new_task_input);
    assert_eq!(new_descriptor.refusal, Some(OfferCloseRefusal::NewTask));
    assert_eq!(
        new_descriptor.why.as_deref(),
        Some(
            "“Weekly” is new in this capture — drop it from the draft instead"
        )
    );
}

#[test]
fn fixture_lists_all_shared_scenarios_without_renumbering() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/offer_close/vectors.json"
    ))
    .expect("valid offer-close vectors");
    let cases = vectors["cases"].as_array().expect("case list");
    assert!(
        cases.len() >= 22,
        "the shared vector set retains its original scenarios"
    );
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["id"], format!("SV{}", index + 1));
    }
}

fn fixture_case<'a>(
    vectors: &'a serde_json::Value,
    id: &str,
) -> &'a serde_json::Value {
    vectors["cases"]
        .as_array()
        .expect("case list")
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("fixture case {id} exists"))
}

fn fixture_input<'a>(
    key: &'a str,
    path: &'a Path,
    line_index: usize,
    expected_line: &'a str,
    source_text: &'a str,
    group: &'a str,
    snapshot: &'a [(PathBuf, String)],
    settings: &'a TasksSettings,
    index: &'a crate::native::vault_links::NoteIndex,
) -> OfferCloseInput<'a> {
    input(
        key,
        path,
        line_index,
        expected_line,
        source_text,
        group,
        snapshot,
        settings,
        index,
    )
}

#[test]
fn fixture_rust_vectors_match_engine() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/offer_close/vectors.json"
    ))
    .expect("valid offer-close vectors");
    let settings = test_settings();
    let today = date();

    for id in ["SV1", "SV5", "SV6", "SV7", "SV18"] {
        let case = fixture_case(&vectors, id);
        assert_eq!(case["kind"], "done");
        let path = PathBuf::from(case["path"].as_str().expect("path"));
        let key = case["key"].as_str().expect("key");
        let line = case["line"].as_u64().expect("line") as usize;
        let source = case["source"].as_str().expect("source");
        let group = case["group"].as_str().unwrap_or("hand_off");
        let snapshot = vec![(path.clone(), source.to_string())];
        let note_index = index(&snapshot);
        let expected_line = source
            .split_inclusive('\n')
            .nth(line)
            .expect("line exists")
            .trim_end_matches(['\n', '\r'])
            .to_string();
        let descriptor = describe_offer_close(&fixture_input(
            key,
            &path,
            line,
            &expected_line,
            source,
            group,
            &snapshot,
            &settings,
            &note_index,
        ));
        assert!(descriptor.complete, "{id} eligible");
        assert!(descriptor.refusal.is_none(), "{id} no refusal");
        let expected = &case["expected"];
        if let Some(consequence) = expected["consequence"].as_str() {
            assert_eq!(
                descriptor.complete_consequence.as_deref(),
                Some(consequence),
                "{id} consequence"
            );
        }
        match expected["scope_note"].as_str() {
            Some(note) => assert_eq!(
                descriptor.scope_note.as_deref(),
                Some(note),
                "{id} scope"
            ),
            None => assert!(descriptor.scope_note.is_none(), "{id} no scope"),
        }
        let plan = plan_offer_close(
            &fixture_input(
                key,
                &path,
                line,
                &expected_line,
                source,
                group,
                &snapshot,
                &settings,
                &note_index,
            ),
            OfferCloseAction::Done,
        )
        .unwrap_or_else(|_| panic!("{id} closes"));
        assert_eq!(plan.source_postimage, expected["source"], "{id} bytes");
    }

    for id in ["SV2", "SV3", "SV4", "SV17"] {
        let case = fixture_case(&vectors, id);
        assert_eq!(case["kind"], "cancel");
        let path = PathBuf::from(case["path"].as_str().expect("path"));
        let key = case["key"].as_str().expect("key");
        let line = case["line"].as_u64().expect("line") as usize;
        let source = case["source"].as_str().expect("source");
        let reason = case["reason"].as_str().unwrap_or_default();
        let group = case["group"].as_str().unwrap_or("hand_off");
        let snapshot = vec![(path.clone(), source.to_string())];
        let note_index = index(&snapshot);
        let expected_line = source
            .split_inclusive('\n')
            .nth(line)
            .expect("line exists")
            .trim_end_matches(['\n', '\r'])
            .to_string();
        let plan = plan_offer_close(
            &fixture_input(
                key,
                &path,
                line,
                &expected_line,
                source,
                group,
                &snapshot,
                &settings,
                &note_index,
            ),
            OfferCloseAction::Cancel {
                reason: reason.to_string(),
            },
        )
        .unwrap_or_else(|_| panic!("{id} cancels"));
        assert_eq!(plan.source_postimage, case["expected"]["source"], "{id}");
    }

    let eligibility = fixture_case(&vectors, "SV8");
    for sub in eligibility["cases"].as_array().expect("subcases") {
        let name = sub["name"].as_str().expect("name");
        let key = sub["key"].as_str().expect("key");
        let line = sub["line"].as_str().expect("line");
        let path = PathBuf::from("Tasks.md");
        let source = if sub.get("changed_identity").is_some() {
            "- [ ] #task Changed root ^root\n".to_string()
        } else {
            format!("{line}\n")
        };
        let snapshot = vec![(path.clone(), source.clone())];
        let note_index = index(&snapshot);
        let settings = test_settings();
        let mut base = fixture_input(
            key,
            &path,
            0,
            line,
            &source,
            "hand_off",
            &snapshot,
            &settings,
            &note_index,
        );
        if sub.get("is_new_task").is_some() {
            base.is_new_task = true;
        }
        if sub.get("muted").is_some() {
            base.muted = true;
        }
        let descriptor = describe_offer_close(&base);
        let refusal = sub["expected_refusal"].as_str().expect("refusal");
        assert_eq!(
            descriptor.refusal.map(|r| r.as_str()),
            Some(refusal),
            "{name} machine refusal"
        );
        assert_eq!(
            descriptor.why.as_deref(),
            sub["expected_why"].as_str(),
            "{name} human why"
        );
        assert!(!descriptor.complete && !descriptor.cancel, "{name} refused");
    }

    for id in ["SV9", "SV10", "SV19", "SV23"] {
        let case = fixture_case(&vectors, id);
        assert_eq!(case["kind"], "recover");
        let path = PathBuf::from(case["path"].as_str().expect("path"));
        let key = case["key"].as_str().expect("key");
        let line = case["line"].as_u64().expect("line") as usize;
        let source = case["source"].as_str().expect("source");
        let is_done = case["action"].as_str().expect("action") == "done";
        let action = if is_done {
            OfferCloseAction::Done
        } else {
            OfferCloseAction::Cancel {
                reason: case["reason"].as_str().unwrap_or_default().to_string(),
            }
        };
        let snapshot = vec![(path.clone(), source.to_string())];
        let note_index = index(&snapshot);
        let expected_line = source
            .split_inclusive('\n')
            .nth(line)
            .expect("line")
            .trim_end_matches(['\n', '\r'])
            .to_string();
        let input = fixture_input(
            key,
            &path,
            line,
            &expected_line,
            source,
            "hand_off",
            &snapshot,
            &settings,
            &note_index,
        );
        let descriptor = describe_offer_close(&input);
        let expected = &case["expected"];
        assert_eq!(
            descriptor.unblocks as u64,
            expected["unblocks"].as_u64().expect("unblocks"),
            "{id} unblocks"
        );
        let consequence = if is_done {
            descriptor.complete_consequence.as_deref()
        } else {
            descriptor.cancel_consequence.as_deref()
        };
        assert_eq!(
            consequence,
            expected["consequence"].as_str(),
            "{id} consequence"
        );
        let plan =
            plan_offer_close(&input, action).unwrap_or_else(|_| panic!("{id}"));
        assert!(plan
            .source_postimage
            .starts_with(expected["root"].as_str().expect("root")));
        assert_eq!(plan.recovery.unblocked.len(), descriptor.unblocks);
        let stamped = expected["dependent_unblocked"].as_str();
        let changed = plan
            .recovery
            .changed_files
            .get(&path)
            .expect("dependent note changes");
        let recovered_lines: Vec<&str> = plan
            .recovery
            .unblocked
            .iter()
            .map(|row| {
                changed
                    .lines()
                    .nth(row.source_line_index)
                    .expect("recovered line exists")
            })
            .collect();
        assert_eq!(recovered_lines.len(), descriptor.unblocks);
        match stamped {
            Some(date) => assert!(recovered_lines
                .iter()
                .all(|line| line.contains(&format!("[unblocked:: {date}]"))),
                "{id} recovered rows stamped"),
            None => assert!(
                recovered_lines.iter().all(|line| !line.contains(&format!(
                    "[unblocked:: {}]",
                    today.format("%Y-%m-%d")
                ))),
                "{id} recovered rows unstamped"
            ),
        }
    }

    for id in ["SV20", "SV21", "SV24"] {
        let case = fixture_case(&vectors, id);
        assert_eq!(case["kind"], "recover-unstamped");
        let path = PathBuf::from(case["path"].as_str().expect("path"));
        let key = case["key"].as_str().expect("key");
        let line = case["line"].as_u64().expect("line") as usize;
        let source = case["source"].as_str().expect("source");
        let snapshot = vec![(path.clone(), source.to_string())];
        let note_index = index(&snapshot);
        let expected_line = source
            .split_inclusive('\n')
            .nth(line)
            .expect("line")
            .trim_end_matches(['\n', '\r'])
            .to_string();
        let input = fixture_input(
            key,
            &path,
            line,
            &expected_line,
            source,
            "hand_off",
            &snapshot,
            &settings,
            &note_index,
        );
        let descriptor = describe_offer_close(&input);
        let expected = &case["expected"];
        assert_eq!(
            descriptor.unblocks as u64,
            expected["unblocks"].as_u64().expect("unblocks"),
            "{id} unblocks"
        );
        assert_eq!(
            descriptor.complete_consequence.as_deref(),
            expected["consequence"].as_str(),
            "{id} consequence omits tickler when unstamped"
        );
        assert!(
            !descriptor
                .complete_consequence
                .as_deref()
                .unwrap_or_default()
                .contains("back tomorrow"),
            "{id} no tickler suffix"
        );
        let plan = plan_offer_close(&input, OfferCloseAction::Done)
            .unwrap_or_else(|_| panic!("{id}"));
        let changed = plan
            .recovery
            .changed_files
            .get(&path)
            .expect("dependent note changes");
        let recovered_lines: Vec<&str> = plan
            .recovery
            .unblocked
            .iter()
            .map(|row| {
                changed
                    .lines()
                    .nth(row.source_line_index)
                    .expect("recovered line exists")
            })
            .collect();
        assert_eq!(recovered_lines.len(), descriptor.unblocks);
        assert!(
            recovered_lines.iter().all(|line| !line.contains(&format!(
                "[unblocked:: {}]",
                today.format("%Y-%m-%d")
            ))),
            "{id} recovered row WRITE skip leaves no stamp"
        );
        if id == "SV24" {
            assert!(
                changed.contains(&format!(
                    "[unblocked:: {}]",
                    today.format("%Y-%m-%d")
                )),
                "unrelated row keeps its existing stamp"
            );
        }
        assert!(expected["dependent_unblocked"].is_null(), "{id} null stamp");
    }
}

#[test]
fn fixture_joint_roots_unblock_shared_dependent_only_together() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/offer_close/vectors.json"
    ))
    .expect("valid offer-close vectors");
    let case = fixture_case(&vectors, "SV22");
    assert_eq!(case["kind"], "joint-recover");
    let path = PathBuf::from(case["path"].as_str().expect("path"));
    let source = case["source"].as_str().expect("source");
    let keys = case["keys"].as_array().expect("keys");
    let lines = case["lines"].as_array().expect("lines");
    let settings = test_settings();
    let snapshot = vec![(path.clone(), source.to_string())];
    let note_index = index(&snapshot);
    assert_eq!(keys.len(), 2);
    assert_eq!(lines.len(), 2);

    for (position, key) in keys.iter().enumerate() {
        let key = key.as_str().expect("key");
        let line = lines[position].as_u64().expect("line") as usize;
        let expected_line = source
            .split_inclusive('\n')
            .nth(line)
            .expect("line")
            .trim_end_matches(['\n', '\r'])
            .to_string();
        let single = fixture_input(
            key,
            &path,
            line,
            &expected_line,
            source,
            "hand_off",
            &snapshot,
            &settings,
            &note_index,
        );
        let descriptor = describe_offer_close(&single);
        assert_eq!(descriptor.unblocks, 0, "single root recovers nothing");
        assert!(
            descriptor
                .complete_consequence
                .as_deref()
                .is_some_and(|copy| !copy.contains("unblocks")),
            "single copy has no unblock suffix"
        );
    }

    let today = date();
    let closed_one =
        "- [x] #task Root one [id:: one]  [completion:: 2026-11-05] ^one";
    let closed_two =
        "- [x] #task Root two [id:: two]  [completion:: 2026-11-05] ^two";
    let joint_source = format!(
        "{closed_one}\n{closed_two}\n- [?] #task Joint waiting [fresh:: 2026-11-01] [dependsOn:: one,two] [id:: waiting] ^waiting\n"
    );
    let joint_snapshot = vec![(path.clone(), joint_source.clone())];
    let completed = vec![
        super::super::successors::CompletedTask {
            relative_path: path.clone(),
            block_id: "one".to_string(),
            task_id: "one".to_string(),
            text: "Root one".to_string(),
            is_root: true,
        },
        super::super::successors::CompletedTask {
            relative_path: path.clone(),
            block_id: "two".to_string(),
            task_id: "two".to_string(),
            text: "Root two".to_string(),
            is_root: true,
        },
    ];
    let joint = super::super::successors::plan_recovery_only(
        super::super::successors::SuccessorInput {
            bob_dir: None,
            completed,
            snapshot: joint_snapshot,
            note_texts: Vec::new(),
            pre_day: None,
            post_day: None,
            closing: None,
            day_relative: PathBuf::from("today.md"),
            today,
            tasks_settings: &settings,
            note_settings: &settings,
            link_unblocked: false,
            keep_moving: false,
            snapshot_complete: true,
            selection: super::super::successors::SuccessorSelection::None,
            index: &note_index,
            relation: None,
        },
        &std::collections::BTreeSet::new(),
    );
    let expected = &case["expected"];
    assert_eq!(
        joint.unblocked.len() as u64,
        expected["joint_unblocks"].as_u64().expect("joint"),
        "joint unblocks the shared dependent"
    );
    assert_eq!(joint.unblocked[0].block_id, "waiting");
    let changed = joint
        .changed_files
        .get(&path)
        .expect("joint recovery writes");
    assert!(
        changed
            .contains(&format!("[unblocked:: {}]", today.format("%Y-%m-%d"))),
        "joint recovery stamps"
    );
}
