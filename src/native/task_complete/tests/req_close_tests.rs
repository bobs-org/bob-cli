//! Pre-close requirement groups and snapshot identity (RV vectors).
//!
//! `tests/fixtures/req_close/vectors.json` is the shared contract for
//! this family. Later close-engine phases append to the same file
//! without renumbering RV1–RV11.

use std::path::Path;

use super::super::req_close::{
    collect_owner, copy, groups_from_owners, owner_block_preimage,
    owner_snapshot_key, ReqCloseGroup, ReqCloseOwner,
};
use super::*;
use crate::native::note_tasks::{self, BlockIdLookup};
use crate::native::task_reqs::MAX_REQ_DEPTH;

const VECTORS: &str =
    include_str!("../../../../tests/fixtures/req_close/vectors.json");

fn settings() -> crate::native::note_tasks::NoteTaskSettings {
    note_tasks::read_settings(Path::new("/no-bob-vault"))
}

pub(super) fn vectors() -> serde_json::Value {
    serde_json::from_str(VECTORS).expect("valid requirement-close vectors")
}

fn source_of(lines: &[serde_json::Value]) -> String {
    let mut source = lines
        .iter()
        .map(|line| line.as_str().expect("source line is a string"))
        .collect::<Vec<_>>()
        .join("\n");
    source.push('\n');
    source
}

fn closing_task<'a>(
    scan: &'a note_tasks::NoteTaskScan,
    spec: &serde_json::Value,
    id: &str,
) -> &'a crate::native::note_tasks::NoteTask {
    if let Some(block_id) = spec["block_id"].as_str() {
        match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => task,
            other => panic!("{id} block_id {block_id}: {other:?}"),
        }
    } else {
        let line = spec["line"].as_u64().unwrap_or_else(|| {
            panic!("{id} closing spec needs block_id or line")
        }) as usize;
        scan.task_at(line.saturating_sub(1))
            .unwrap_or_else(|| panic!("{id} no task at line {line}"))
    }
}

pub(super) fn collect_case(
    case: &serde_json::Value,
    settings: &crate::native::note_tasks::NoteTaskSettings,
) -> (Vec<ReqCloseOwner>, Vec<ReqCloseGroup>) {
    let id = case["id"].as_str().unwrap_or("?");
    let mut owners = Vec::new();
    for note in case["notes"].as_array().expect("notes list") {
        let contents = source_of(
            note["source_lines"]
                .as_array()
                .unwrap_or_else(|| panic!("{id} source_lines")),
        );
        let path = Path::new(note["path"].as_str().expect("note path"));
        let scan = note_tasks::scan(&contents, settings);
        for spec in note["closing"].as_array().expect("closing list") {
            let task = closing_task(&scan, spec, id);
            if let Some(owner) = collect_owner(&contents, path, task, settings)
            {
                owners.push(owner);
            }
        }
    }
    let groups = groups_from_owners(&owners);
    (owners, groups)
}

fn folded_count(value: &serde_json::Value, key: &str) -> usize {
    value[key].as_u64().unwrap_or(0) as usize
}

fn assert_owner(id: &str, owner: &ReqCloseOwner, expected: &serde_json::Value) {
    assert_eq!(owner.task_key, expected["task_key"], "{id} task_key");
    assert_eq!(owner.key, expected["key"], "{id} owner key");
    assert_eq!(owner.path, expected["path"], "{id} path");
    assert_eq!(owner.block_id, expected["block_id"], "{id} block_id");
    assert_eq!(owner.line, expected["line"], "{id} line");
    assert_eq!(owner.text, expected["text"], "{id} text");
    assert_eq!(owner.preimage, expected["preimage"], "{id} preimage");
    assert_eq!(
        owner.depth_exceeded,
        expected["depth_exceeded"].as_bool().unwrap_or(false),
        "{id} depth_exceeded"
    );
    assert_eq!(
        owner_snapshot_key(&owner.task_key, &owner.preimage),
        owner.key,
        "{id} snapshot key binds the preimage"
    );
    assert!(
        owner.preimage.contains(&owner.task_line) || owner.task_line.is_empty(),
        "{id} task_line sits inside the preimage"
    );
}

fn assert_group(id: &str, group: &ReqCloseGroup, expected: &serde_json::Value) {
    assert_eq!(group.group, "requirements", "{id} group");
    assert_eq!(group.kind, "requirements", "{id} kind");
    assert_eq!(group.group, expected["group"], "{id} group field");
    assert_eq!(group.kind, expected["kind"], "{id} kind field");
    assert_eq!(
        group.owner.task_key, expected["owner"]["task_key"],
        "{id} owner.task_key"
    );
    assert_eq!(group.owner.key, expected["owner"]["key"], "{id} owner.key");
    assert_eq!(
        group.owner.path, expected["owner"]["path"],
        "{id} owner.path"
    );
    assert_eq!(
        group.owner.block_id, expected["owner"]["block_id"],
        "{id} owner.block_id"
    );
    assert_eq!(
        group.owner.line as u64,
        expected["owner"]["line"].as_u64().unwrap_or(0),
        "{id} owner.line"
    );
    assert_eq!(
        group.owner.text, expected["owner"]["text"],
        "{id} owner.text"
    );
    assert_eq!(group.copy.title, expected["copy"]["title"], "{id} title");
    assert_eq!(group.copy.scope, expected["copy"]["scope"], "{id} scope");
    assert_eq!(group.copy.why, expected["copy"]["why"], "{id} why");
    assert_eq!(
        group.copy.history.as_deref(),
        expected["copy"]["history"].as_str(),
        "{id} history"
    );
    assert_eq!(group.copy.glyph, expected["copy"]["glyph"], "{id} glyph");
    assert_eq!(group.copy.footer, expected["copy"]["footer"], "{id} footer");
    assert_eq!(group.copy.hint, expected["copy"]["hint"], "{id} hint");
    let folded = &expected["folded"];
    assert_eq!(
        group.folded.dropped,
        folded_count(folded, "dropped"),
        "{id} folded.dropped"
    );
    assert_eq!(
        group.folded.carried,
        folded_count(folded, "carried"),
        "{id} folded.carried"
    );
    assert_eq!(
        group.folded.met,
        folded_count(folded, "met"),
        "{id} folded.met"
    );
    let rows = expected["rows"].as_array().expect("rows");
    assert_eq!(group.rows.len(), rows.len(), "{id} row count");
    for (row, expected_row) in group.rows.iter().zip(rows) {
        assert_eq!(
            row.offer.key, expected_row["offer"]["key"],
            "{id} offer.key"
        );
        assert_eq!(
            row.offer.n as u64,
            expected_row["offer"]["n"].as_u64().unwrap_or(0),
            "{id} offer.n"
        );
        assert_eq!(
            row.offer.group, expected_row["offer"]["group"],
            "{id} offer.group"
        );
        assert_eq!(
            row.offer.selected,
            expected_row["offer"]["selected"].as_bool().unwrap_or(false),
            "{id} offer.selected"
        );
        assert_eq!(
            row.owner_key, expected_row["req"]["owner"],
            "{id} req.owner"
        );
        assert_eq!(row.path, expected_row["req"]["path"], "{id} req.path");
        assert_eq!(
            row.depth as u64,
            expected_row["req"]["depth"].as_u64().unwrap_or(0),
            "{id} req.depth"
        );
        assert_eq!(
            row.parent_key.as_deref(),
            expected_row["req"]["parent_key"].as_str(),
            "{id} req.parent_key"
        );
        assert_eq!(row.text, expected_row["req"]["text"], "{id} req.text");
        assert_eq!(
            row.line as u64,
            expected_row["req"]["line"].as_u64().unwrap_or(0),
            "{id} req.line"
        );
        assert_eq!(row.state, expected_row["req"]["state"], "{id} req.state");
        assert_eq!(
            row.pending_descendants as u64,
            expected_row["req"]["pending_descendants"]
                .as_u64()
                .unwrap_or(0),
            "{id} req.pending_descendants"
        );
        assert!(!row.depth_exceeded, "{id} listed rows stay in bound");
        assert_eq!(row.run, expected_row["run"], "{id} run");
        assert_eq!(
            row.accessibility, expected_row["accessibility"],
            "{id} accessibility"
        );
        let cons = &expected_row["consequences"];
        assert_eq!(row.consequences.met, cons["met"], "{id} consequences.met");
        assert_eq!(
            row.consequences.drop, cons["drop"],
            "{id} consequences.drop"
        );
        assert_eq!(
            row.consequences.carry, cons["carry"],
            "{id} consequences.carry"
        );
        assert_eq!(
            row.consequences.carry_pulled, cons["carry_pulled"],
            "{id} consequences.carry_pulled"
        );
    }
}

#[test]
fn fixture_lists_every_row_case_without_renumbering() {
    let vectors = vectors();
    assert_eq!(vectors["schema_version"], 1);
    assert_eq!(vectors["family"], "rows");
    assert_eq!(vectors["contract"], "docs/task-requirements.md §7");
    let cases = vectors["cases"].as_array().expect("case list");
    let row_cases: Vec<_> = cases
        .iter()
        .filter(|case| case["family"] == "rows")
        .collect();
    assert_eq!(row_cases.len(), 11, "family rows freezes RV1–RV11");
    for (index, case) in row_cases.iter().enumerate() {
        assert_eq!(case["id"], format!("RV{}", index + 1));
    }
}

#[test]
fn copy_catalog_matches_shared_fixture() {
    let catalog = &vectors()["copy"];
    assert_eq!(copy::WHY, catalog["why"]);
    assert_eq!(copy::DROP_PROMPT, catalog["drop_prompt"]);
    assert_eq!(copy::FOOTER, catalog["footer"]);
    assert_eq!(copy::JUMP_HINT, catalog["jump_hint"]);
    assert_eq!(copy::HINT, catalog["hint"]);
    assert_eq!(copy::TEACHING, catalog["teaching"]);
    assert_eq!(
        copy::overlap("ancestor"),
        catalog["overlap"]
            .as_str()
            .expect("overlap")
            .replace("{ancestor}", "ancestor")
    );
    assert_eq!(
        copy::nested_go_with_it(3),
        catalog["nested_go_with_it"]
            .as_str()
            .expect("nested")
            .replace("{k}", "3")
    );
    assert_eq!(
        copy::met_receipt(4, "S"),
        catalog["met_receipt"]
            .as_str()
            .expect("met_receipt")
            .replace("{n}", "4")
            .replace("{S}", "S")
    );
    assert_eq!(
        copy::human_met_line(4, "S"),
        catalog["human_met"]
            .as_str()
            .expect("human_met")
            .replace("{n}", "4")
            .replace("{S}", "S")
    );
    assert_eq!(
        copy::drop_receipt("T", None),
        catalog["drop_receipt"]
            .as_str()
            .expect("drop_receipt")
            .replace("{T}", "T")
    );
    assert_eq!(
        copy::drop_receipt("T", Some("because")),
        "⊘ Dropped “T” · “because”"
    );
    assert_eq!(
        copy::carry_receipt("T", false),
        catalog["carry_receipt"]
            .as_str()
            .expect("carry_receipt")
            .replace("{T}", "T")
    );
    assert_eq!(
        copy::carry_receipt("T", true),
        catalog["carry_pulled_receipt"]
            .as_str()
            .expect("carry_pulled")
            .replace("{T}", "T")
    );
    assert_eq!(copy::summary(4, 1, 1), catalog["summary"]);
    assert_eq!(
        copy::accessibility(2, "will be dropped"),
        catalog["accessibility"]
    );
    assert_eq!(
        copy::would_met(4, "S"),
        "would meet 4 requirements with “S”"
    );
    assert_eq!(copy::quoted("Ship"), "“Ship”");
    assert_eq!(
        copy::group_title("Ship the exporter"),
        "Requirements · “Ship the exporter”"
    );
    assert_eq!(copy::scope(4, 1), "4 reqs");
    assert_eq!(copy::scope(2, 2), "2 reqs in 2 branches");
    assert_eq!(copy::history(0, 0, 0), None);
    assert_eq!(
        copy::history(1, 0, 0).as_deref(),
        Some("· 1 dropped earlier")
    );
    assert_eq!(
        copy::history(1, 1, 1).as_deref(),
        Some("· 1 dropped earlier · 1 carried earlier · 1 met earlier")
    );
}

#[test]
fn fixture_vectors_match_collector_and_groups() {
    let vectors = vectors();
    let settings = settings();
    for case in vectors["cases"]
        .as_array()
        .expect("case list")
        .iter()
        .filter(|case| case["family"] == "rows")
    {
        let id = case["id"].as_str().unwrap_or("?");
        let (owners, groups) = collect_case(case, &settings);
        let expected_owners =
            case["expect"]["owners"].as_array().expect("owners list");
        let expected_groups =
            case["expect"]["groups"].as_array().expect("groups list");
        assert_eq!(owners.len(), expected_owners.len(), "{id} owner count");
        assert_eq!(groups.len(), expected_groups.len(), "{id} group count");
        for (owner, expected) in owners.iter().zip(expected_owners) {
            assert_owner(id, owner, expected);
            let pending = expected_groups
                .iter()
                .find(|group| group["owner"]["key"] == owner.key)
                .map(|group| {
                    group["rows"].as_array().map(Vec::len).unwrap_or(0)
                })
                .unwrap_or(0);
            assert_eq!(owner.pending_count(), pending, "{id} pending_count");
        }
        for (group, expected) in groups.iter().zip(expected_groups) {
            assert_group(id, group, expected);
        }
    }
}

#[test]
fn resolved_folds_keep_historical_nodes_off_the_group() {
    let vectors = vectors();
    let case = vectors["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == "RV6")
        .expect("RV6");
    let (owners, groups) = collect_case(case, &settings());
    let owner = owners.first().expect("RV6 owner");
    let states: Vec<&str> = owner.nodes.iter().map(|node| node.state).collect();
    assert_eq!(states, ["pending", "dropped", "excluded", "carried", "met"]);
    let marks: Vec<&str> = owner.nodes.iter().map(|node| node.mark).collect();
    assert_eq!(marks, ["open", "dropped", "open", "carried", "legacy-met"]);
    assert_eq!(owner.nodes[2].text, "Excluded child");
    assert_eq!(owner.nodes[3].text, "[[#^carried|Carried]]");
    assert_eq!(owner.pending_count(), 1);
    assert_eq!(owner.pending_branches(), 1);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].rows.len(), 1);
    assert_eq!(groups[0].rows[0].text, "Live");
    assert_eq!(groups[0].folded.dropped, 1);
    assert_eq!(groups[0].folded.carried, 1);
    assert_eq!(groups[0].folded.met, 1);
}

#[test]
fn note_interrupt_omits_the_nested_checkbox() {
    let vectors = vectors();
    let case = vectors["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == "RV3")
        .expect("RV3");
    let (owners, _) = collect_case(case, &settings());
    let texts: Vec<&str> = owners[0]
        .nodes
        .iter()
        .map(|node| node.text.as_str())
        .collect();
    assert_eq!(texts, ["Real req", "Another req"]);
}

#[test]
fn identical_block_hashes_stay_distinct_through_task_key() {
    let vectors = vectors();
    let case = vectors["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == "RV11")
        .expect("RV11");
    let (owners, _) = collect_case(case, &settings());
    assert_eq!(owners.len(), 2);
    let left_hash = owners[0].key.rsplit('@').next().expect("hash");
    let right_hash = owners[1].key.rsplit('@').next().expect("hash");
    assert_eq!(left_hash, right_hash);
    assert_ne!(owners[0].task_key, owners[1].task_key);
    assert_ne!(owners[0].key, owners[1].key);
}

#[test]
fn depth_above_max_is_kept_not_truncated() {
    let settings = settings();
    let mut lines = vec!["- [ ] #task Deep ^deep".to_string()];
    for depth in 1..=MAX_REQ_DEPTH + 1 {
        lines.push(format!("{}- [ ] d{depth}", "\t".repeat(depth)));
    }
    let mut contents = lines.join("\n");
    contents.push('\n');
    let scan = note_tasks::scan(&contents, &settings);
    let task = match scan.by_block_id("deep") {
        BlockIdLookup::Found(task) => task,
        other => panic!("deep owner: {other:?}"),
    };
    let owner =
        collect_owner(&contents, Path::new("Tasks.md"), task, &settings)
            .expect("non-recurring owner");
    assert!(owner.depth_exceeded);
    assert_eq!(owner.nodes.len(), MAX_REQ_DEPTH + 1);
    let last = owner.nodes.last().expect("deepest node");
    assert_eq!(last.depth, MAX_REQ_DEPTH + 1);
    assert!(last.depth_exceeded);
    assert_eq!(last.state, "pending");
    assert_eq!(
        owner.pending_count(),
        MAX_REQ_DEPTH + 1,
        "depth refusal keeps every pending row"
    );
    let groups = groups_from_owners(std::slice::from_ref(&owner));
    assert_eq!(groups[0].rows.len(), MAX_REQ_DEPTH + 1);
    assert!(groups[0].rows.last().expect("row").depth_exceeded);
}

#[test]
fn tree_close_binds_the_open_preimage_before_mutation() {
    let mut vault = MemoryVault::new();
    let source = concat!(
        "- [ ] #task Ship the exporter ^export\n",
        "\t- [ ] Handles supported inputs\n",
        "\t\t- [ ] Handles Windows files\n",
    );
    vault.insert("Tasks.md", source);
    let root = vault.absolute("Tasks.md");
    let outcome = complete_task_tree(
        &vault,
        &root,
        "export",
        "2026-10-05",
        RootPolicy::Explicit,
    )
    .expect("tree closes");
    assert_eq!(outcome.req_owners.len(), 1);
    let owner = &outcome.req_owners[0];
    assert_eq!(owner.task_key, "Tasks.md#^export");
    assert_eq!(owner.text, "Ship the exporter");
    assert_eq!(owner.preimage, source);
    assert!(owner.preimage.contains("- [ ] #task Ship the exporter"));
    assert!(!owner.preimage.contains("- [x]"));
    let updated = outcome.changed_files.get(&root).expect("Tasks.md changed");
    assert!(updated.contains("- [x] #task Ship the exporter"));
    let groups = groups_from_owners(&outcome.req_owners);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].rows.len(), 2);
    assert_eq!(groups[0].copy.scope, "2 reqs");
}

#[test]
fn tree_close_skips_recurring_owners() {
    let mut vault = MemoryVault::new();
    vault.insert(
        "Tasks.md",
        concat!(
            "- [ ] #task Recurring [repeat:: every week] ^rec\n",
            "\t- [ ] A req\n",
        ),
    );
    let root = vault.absolute("Tasks.md");
    let outcome = complete_task_tree(
        &vault,
        &root,
        "rec",
        "2026-10-05",
        RootPolicy::Explicit,
    )
    .expect("recurring root stays open");
    assert!(outcome.req_owners.is_empty());
    assert!(groups_from_owners(&outcome.req_owners).is_empty());
}

#[test]
fn tree_close_on_a_req_less_owner_adds_no_group() {
    let mut vault = MemoryVault::new();
    vault.insert("Tasks.md", "- [ ] #task Bare ^bare\n");
    let root = vault.absolute("Tasks.md");
    let outcome = complete_task_tree(
        &vault,
        &root,
        "bare",
        "2026-10-05",
        RootPolicy::Explicit,
    )
    .expect("bare task closes");
    assert_eq!(outcome.req_owners.len(), 1);
    assert!(outcome.req_owners[0].nodes.is_empty());
    assert!(groups_from_owners(&outcome.req_owners).is_empty());
}

#[test]
fn nested_task_owners_keep_separate_forests_in_close_order() {
    let settings = settings();
    let contents = concat!(
        "- [ ] #task Parent ^parent\n",
        "\t- [ ] Parent req\n",
        "\t- [ ] #task Child ^child\n",
        "\t\t- [ ] Child req\n",
    );
    let scan = note_tasks::scan(contents, &settings);
    let child = match scan.by_block_id("child") {
        BlockIdLookup::Found(task) => task,
        other => panic!("child: {other:?}"),
    };
    let parent = match scan.by_block_id("parent") {
        BlockIdLookup::Found(task) => task,
        other => panic!("parent: {other:?}"),
    };
    let child_owner =
        collect_owner(contents, Path::new("Tasks.md"), child, &settings)
            .expect("child");
    let parent_owner =
        collect_owner(contents, Path::new("Tasks.md"), parent, &settings)
            .expect("parent");
    assert!(child_owner.preimage.starts_with('\t'));
    assert_eq!(owner_block_preimage(contents, child), child_owner.preimage);
    let groups = groups_from_owners(&[child_owner, parent_owner]);
    assert_eq!(
        groups
            .iter()
            .map(|group| group.owner.text.as_str())
            .collect::<Vec<_>>(),
        ["Child", "Parent"]
    );
    assert_eq!(groups[0].rows[0].text, "Child req");
    assert_eq!(groups[1].rows[0].text, "Parent req");
}

#[test]
fn successor_pre_gates_do_not_drop_a_req_only_audit() {
    // Tree close attaches forests before mutation. Groups are derived
    // from those owners even when no unblocked successor exists.
    let mut vault = MemoryVault::new();
    vault.insert(
        "Tasks.md",
        concat!("- [ ] #task Solo ^solo\n", "\t- [ ] Only req\n",),
    );
    let root = vault.absolute("Tasks.md");
    let outcome = complete_task_tree(
        &vault,
        &root,
        "solo",
        "2026-10-05",
        RootPolicy::Explicit,
    )
    .expect("solo closes");
    assert!(outcome.closed_subtasks.is_empty());
    assert!(outcome.left_open.is_empty());
    let groups = groups_from_owners(&outcome.req_owners);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].rows[0].text, "Only req");
    assert_eq!(groups[0].copy.title, "Requirements · “Solo”");
}
