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
    assert_eq!(
        descriptor.complete_consequence.as_deref(),
        Some("✓ Completes “Parent” · Ready → Done")
    );
    assert_eq!(
        descriptor.scope_note.as_deref(),
        Some("· its 1 embedded task stays open")
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
    assert_eq!(
        describe_offer_close(&recurring_input).why.as_deref(),
        Some("recurring")
    );

    let new_task_input = OfferCloseInput {
        is_new_task: true,
        ..recurring_input
    };
    assert_eq!(
        describe_offer_close(&new_task_input).why.as_deref(),
        Some("new_task")
    );
}

#[test]
fn fixture_publishes_all_sixteen_shared_scenarios() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/offer_close/vectors.json"
    ))
    .expect("valid offer-close vectors");
    let cases = vectors["cases"].as_array().expect("case list");
    assert_eq!(cases.len(), 16);
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["id"], format!("SV{}", index + 1));
    }
}
