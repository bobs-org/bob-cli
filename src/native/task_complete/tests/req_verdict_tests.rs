//! Requirement choice and common card-number vectors (RV12–RV19).

use super::super::req_verdicts::{
    resolve_verdicts, ReqCardNumbering, ReqCardRowKind, ReqChoiceState,
    ReqVerdict, ReqVerdictError, ReqVerdictRequest,
};
use super::req_close_tests::{collect_case, vectors};
use super::*;
use crate::native::note_tasks;

fn case<'a>(vectors: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    vectors["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing {id}"))
}

fn row_key(
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

fn verdict(value: &str) -> ReqVerdict {
    match value {
        "met" => ReqVerdict::Met,
        "drop" => ReqVerdict::Drop,
        "carry" => ReqVerdict::Carry,
        "carry_pulled" => ReqVerdict::CarryPulled,
        other => panic!("unknown verdict {other}"),
    }
}

fn requests(value: &serde_json::Value) -> Vec<ReqVerdictRequest> {
    value
        .as_array()
        .expect("requests")
        .iter()
        .map(|request| ReqVerdictRequest {
            selector: request["selector"]
                .as_str()
                .expect("selector")
                .to_string(),
            verdict: verdict(request["verdict"].as_str().expect("verdict")),
            reason: request["reason"].as_str().map(str::to_string),
        })
        .collect()
}

fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .expect("string list")
        .iter()
        .map(|value| value.as_str().expect("string").to_string())
        .collect()
}

fn collect_choice_case(
    choice_case: &serde_json::Value,
) -> (
    Vec<crate::native::task_complete::req_close::ReqCloseOwner>,
    Vec<crate::native::task_complete::req_close::ReqCloseGroup>,
) {
    let all = vectors();
    let base_id = choice_case["input"]["base_case"]
        .as_str()
        .expect("base_case");
    let mut base = case(&all, base_id).clone();
    if let Some(lines) = choice_case["input"]["source_lines_after"].as_array() {
        base["notes"][0]["source_lines"] =
            serde_json::Value::Array(lines.clone());
    }
    collect_case(
        &base,
        &note_tasks::read_settings(Path::new("/no-bob-vault")),
    )
}

#[test]
fn choices_vectors_have_stable_ids_and_cover_every_verdict_pair() {
    let all = vectors();
    let cases: Vec<_> = all["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|case| case["family"] == "choices")
        .collect();
    assert_eq!(cases.len(), 8);
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["id"], format!("RV{}", index + 12));
    }

    let vector = case(&all, "RV12");
    let (owners, _) = collect_choice_case(vector);
    let key = vector["input"]["key"].as_str().expect("key");
    let offers = Vec::new();
    let inputs = vector["input"]["pairs"].as_array().expect("input pairs");
    let expected = vector["expect"]["pair_results"]
        .as_array()
        .expect("expected pairs");
    assert_eq!(inputs.len(), expected.len());
    for (pair, expected) in inputs.iter().zip(expected) {
        assert_eq!(pair["result"], expected["result"]);
        let choices = ["left", "right"].map(|side| ReqVerdictRequest {
            selector: key.to_string(),
            verdict: verdict(pair[side].as_str().expect("pair verdict")),
            reason: None,
        });
        let result = resolve_verdicts(&owners, &offers, &choices, None);
        match expected["result"].as_str().expect("pair result") {
            "idempotent" => {
                let selected = result.expect("same verdict is idempotent");
                assert_eq!(selected.verdicts.len(), 1);
            }
            "offer_verdict_conflict" => {
                assert_eq!(
                    result
                        .expect_err("different final choices conflict")
                        .code(),
                    "offer_verdict_conflict"
                );
            }
            other => panic!("unknown pair result {other}"),
        }
    }
}

#[test]
fn structural_overlap_refuses_but_navigation_pin_stays_independent() {
    let all = vectors();
    let overlap = case(&all, "RV13");
    let (owners, _) = collect_choice_case(overlap);
    let error = resolve_verdicts(
        &owners,
        &[],
        &requests(&overlap["input"]["requests"]),
        None,
    )
    .expect_err("a descendant cannot override its selected ancestor");
    assert_eq!(error.code(), overlap["expect"]["error_code"]);

    let pin = case(&all, "RV14");
    let (owners, _) = collect_choice_case(pin);
    let selected = resolve_verdicts(
        &owners,
        &[],
        &requests(&pin["input"]["requests"]),
        pin["input"]["jump"].as_str(),
    )
    .expect("a navigation pin does not conflict with branch ownership");
    assert_eq!(selected.verdicts.len(), 1);
    let expected_verdict = &pin["expect"]["verdicts"][0];
    assert_eq!(selected.verdicts[0].row.key, expected_verdict["key"]);
    assert_eq!(
        selected.verdicts[0].verdict.as_str(),
        expected_verdict["verdict"]
    );
    let jump = selected.jump.expect("pin");
    assert_eq!(jump.card_row.kind, ReqCardRowKind::Requirement);
    assert_eq!(jump.card_row.key, pin["expect"]["jump"]["key"]);
    assert_eq!(
        jump.requirement.expect("req provenance").owner_task_key,
        "Tasks.md#^export"
    );
}

#[test]
fn shared_numbering_and_mark_all_keep_requirement_rows_out_of_task_picks() {
    let vector = case(&vectors(), "RV15").clone();
    let (owners, _) = collect_choice_case(&vector);
    let offers: Vec<String> = vector["input"]["task_offer_keys"]
        .as_array()
        .expect("task offer keys")
        .iter()
        .map(|key| key.as_str().expect("key").to_string())
        .collect();
    let selected = resolve_verdicts(&owners, &offers, &[], None).expect("card");
    let expected = vector["expect"]["numbered"].as_array().expect("numbered");
    assert_eq!(selected.numbering.rows.len(), expected.len());
    for (actual, expected) in selected.numbering.rows.iter().zip(expected) {
        assert_eq!(actual.key, expected["key"]);
        assert_eq!(actual.n as u64, expected["n"]);
        assert_eq!(
            actual.kind,
            if expected["kind"] == "requirement" {
                ReqCardRowKind::Requirement
            } else {
                ReqCardRowKind::TaskOffer
            }
        );
    }
    assert_eq!(selected.numbering.mark_all_keys(), offers);
    assert_eq!(
        selected.groups[0]
            .rows
            .iter()
            .map(|row| row.offer.n)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    let legacy = ReqCardNumbering::new(&[], &offers).expect("legacy card");
    assert_eq!(
        legacy.rows.iter().map(|row| row.n).collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn staged_fold_hides_without_discarding_and_restaging_met_unfolds() {
    let all = vectors();
    let vector = case(&all, "RV16");
    let (owners, groups) = collect_choice_case(vector);
    let group = &groups[0];
    let child = row_key(&groups, "1.1.1");
    let ancestor = row_key(&groups, "1.1");
    let mut state = ReqChoiceState::default();
    state.stage(child.clone(), ReqVerdict::Carry);
    state.stage(ancestor.clone(), ReqVerdict::Drop);
    let folded = state.fold(group);
    let after_drop = &vector["expect"]["after_drop"];
    let visible: Vec<String> = folded
        .iter()
        .filter(|row| row.visible)
        .map(|row| row.key.clone())
        .collect();
    assert_eq!(visible, strings(&after_drop["visible"]));
    let hidden = folded.iter().find(|row| row.key == child).expect("child");
    assert!(!hidden.visible);
    assert_eq!(hidden.hidden_by.as_deref(), Some(ancestor.as_str()));
    let root = folded.iter().find(|row| row.key == ancestor).expect("root");
    assert_eq!(
        root.nested_go_with_it.map(|count| count as u64),
        after_drop["nested_go_with_it"]["count"].as_u64()
    );
    assert_eq!(
        resolve_verdicts(&owners, &[], &state.requests(), None)
            .expect_err("folded choices are still validated")
            .code(),
        after_drop["overlap_code"]
    );
    let mut explicit_met = requests(&case(&all, "RV13")["input"]["requests"]);
    explicit_met[1].verdict = ReqVerdict::Met;
    assert_eq!(
        resolve_verdicts(&owners, &[], &explicit_met, None)
            .expect_err("an explicit child Met conflicts with branch ownership")
            .code(),
        "req_verdict_overlap"
    );

    state.stage(ancestor, ReqVerdict::Met);
    let unfolded = state.fold(group);
    let after_met = &vector["expect"]["after_met"];
    let visible: Vec<String> = unfolded
        .iter()
        .filter(|row| row.visible)
        .map(|row| row.key.clone())
        .collect();
    assert_eq!(visible, strings(&after_met["visible"]));
    assert_eq!(state.choice(&child), ReqVerdict::Carry);
    let resolved = resolve_verdicts(&owners, &[], &state.requests(), None)
        .expect("the child's retained choice now stands alone");
    assert_eq!(resolved.verdicts.len(), 1);
    assert_eq!(resolved.verdicts[0].row.key, child);
    assert_eq!(
        resolved.verdicts[0].verdict.as_str(),
        after_met["choices"][0]["verdict"]
    );
    assert_eq!(after_met["error_code"], serde_json::Value::Null);

    let mut toggle = ReqChoiceState::default();
    for action in vector["input"]["toggle"].as_array().expect("toggle") {
        toggle.stage(
            action["key"].as_str().expect("toggle key"),
            verdict(action["verdict"].as_str().expect("toggle verdict")),
        );
    }
    assert_eq!(
        toggle.choice(vector["input"]["toggle_key"].as_str().expect("key")),
        verdict(vector["expect"]["toggle_final"].as_str().expect("final"))
    );
}

#[test]
fn disjoint_multi_owner_choices_keep_owner_provenance() {
    let vector = case(&vectors(), "RV17").clone();
    let (owners, _) = collect_choice_case(&vector);
    let selected = resolve_verdicts(
        &owners,
        &[],
        &requests(&vector["input"]["requests"]),
        None,
    )
    .expect("different closing owners have independent branches");
    let expected = vector["expect"]["verdicts"].as_array().expect("verdicts");
    assert_eq!(selected.verdicts.len(), expected.len());
    for (actual, expected) in selected.verdicts.iter().zip(expected) {
        assert_eq!(actual.row.key, expected["key"]);
        assert_eq!(actual.row.owner_task_key, expected["owner_task_key"]);
        assert_eq!(actual.verdict.as_str(), expected["verdict"]);
    }
}

#[test]
fn old_keys_and_probe_preimages_refuse_reordered_duplicate_labels() {
    let vector = case(&vectors(), "RV18").clone();
    let all = vectors();
    let probe_case = case(&all, "RV10");
    let (probe_owners, _) = collect_case(
        probe_case,
        &note_tasks::read_settings(Path::new("/no-bob-vault")),
    );
    let fresh_case = {
        let base = case(&all, "RV10");
        let mut base = base.clone();
        base["notes"][0]["source_lines"] =
            vector["input"]["source_lines_after"].clone();
        base
    };
    let (fresh_owners, _) = collect_case(
        &fresh_case,
        &note_tasks::read_settings(Path::new("/no-bob-vault")),
    );
    let requests = requests(&vector["input"]["requests"]);
    let stale = resolve_verdicts(&fresh_owners, &[], &requests, None)
        .expect_err("old exact key does not bind to the reordered branch");
    assert_eq!(stale.code(), vector["expect"]["error_code"]);
    match stale {
        ReqVerdictError::Stale {
            selectors,
            current: Some(current),
        } => {
            assert_eq!(
                selectors,
                [vector["expect"]["old_key"]
                    .as_str()
                    .expect("old key")
                    .to_string()]
            );
            assert_eq!(current.groups.len(), 1);
            assert_eq!(current.rows.len(), 4);
            assert_ne!(current.rows[0].key, vector["expect"]["old_key"]);
        }
        other => {
            panic!("stale refusal should carry the current card: {other:?}")
        }
    }

    let selected = resolve_verdicts(&probe_owners, &[], &requests, None)
        .expect("probe selection");
    assert_eq!(
        super::super::req_verdicts::revalidate_preimages(
            &selected,
            &fresh_owners,
            &[]
        )
        .expect_err("the complete owner preimage changed")
        .code(),
        "req_close_stale"
    );
}

#[test]
fn duplicate_owner_identity_refuses_and_marks_stale() {
    let vector = case(&vectors(), "RV19").clone();
    let (mut owners, _) = collect_choice_case(&vector);
    let index = vector["input"]["duplicate_owner_index"]
        .as_u64()
        .expect("duplicate owner index") as usize;
    owners.push(owners[index].clone());
    assert_eq!(
        resolve_verdicts(&owners, &[], &[], None)
            .expect_err("one closing owner cannot occur twice")
            .code(),
        vector["expect"]["error_code"]
    );
}
