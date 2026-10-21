use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::super::capture::parse_managed_task_log_marker;
use super::super::task_dependencies::{
    blocks::{self, BlocksLine, BlocksLinePlan, BlocksUpdateKind},
    format::canonical_link,
    legacy::legacy_child_reference,
    parse::{self as dep_parse, DependencyLine},
};
use super::super::vault_links::NoteIndex;
use super::format::{
    format_linked_from_line, format_links_line, relation_insert_slot,
    RelationSlotKind,
};
use super::label::LabelError;
use super::pair::{
    mirror_entry, pair_state, plan_pair_write, LinksOp, MirrorDelta,
    PairAction, PairKind, PairRecords, PairRequest, PairState, RecordKind,
    Side, Writability,
};
use super::parse::{
    linked_from_children_of, links_children_of, parse_linked_from_line,
    parse_links_line,
};
use super::plan::{
    linked_from_desired_after_delta, plan_linked_from_line, plan_links_edit,
    LinksEdit, LinksLinePlan, RelationEdit,
};
use super::{
    is_linked_from_line, is_links_line, is_relation_line, LinkedFromLine,
    LinksLine,
};

fn accepted_links(line: &str) -> (usize, bool, Vec<Option<String>>) {
    match parse_links_line(line) {
        LinksLine::Accepted { entries, canonical } => (
            entries.len(),
            canonical,
            entries.into_iter().map(|entry| entry.label).collect(),
        ),
        other => panic!("expected accepted LINKS line, got {other:?}: {line}"),
    }
}

fn accepted_from(line: &str) -> (usize, bool, Vec<Option<String>>) {
    match parse_linked_from_line(line) {
        LinkedFromLine::Accepted { entries, canonical } => (
            entries.len(),
            canonical,
            entries.into_iter().map(|entry| entry.label).collect(),
        ),
        other => {
            panic!("expected accepted LINKED FROM line, got {other:?}: {line}")
        }
    }
}

// LK1–LK20: `docs/task-relations.md` parse vectors.
#[test]
fn lk_parse_vectors() {
    let (count, canonical, labels) = accepted_links("- 🪢 **LINKS:** [[#^a]]"); // LK1
    assert_eq!((count, canonical, labels), (1, true, vec![None]));

    let (count, canonical, labels) = accepted_links(
        "- 🪢 **LINKS:** follow-up of [[done/bob_done#^cap]] • [[sase#^retry]]",
    ); // LK2
    assert_eq!(
        (count, canonical, labels),
        (2, true, vec![Some("follow-up of".into()), None])
    );

    for line in [
        "- 🪢 **LINKS:** see [[#^a]] · merged into [[b#^c]]",
        "- 🪢 **LINKS:** see [[#^a]], merged into [[b#^c]]",
        "- 🪢 **LINKS:** see [[#^a]] merged into [[b#^c]]",
    ] {
        let (count, canonical, labels) = accepted_links(line); // LK3
        assert_eq!(count, 2, "{line}");
        assert!(!canonical, "{line}");
        assert_eq!(
            labels,
            [Some("see".into()), Some("merged into".into())],
            "{line}"
        );
    }

    for line in [
        "- **LINKS:** [[#^a]]",
        "- 🔗 **LINKS:** [[#^a]]",
        "- 🪢 **LINKS TO:** [[#^a]]",
    ] {
        let (count, canonical, _) = accepted_links(line); // LK4
        assert_eq!((count, canonical), (1, false), "{line}");
    }

    let (count, canonical, labels) =
        accepted_links("- 🪢 **LINKS:** [[#^a|merged into]]"); // LK5
    assert_eq!(count, 1);
    assert!(!canonical);
    assert_eq!(labels, [Some("merged into".into())]);

    let (_, _, labels) = accepted_links("- 🪢 **LINKS:** see [[#^a|x]]"); // LK6
    assert_eq!(labels, [Some("see".into())]);

    for line in ["- 🪢 **LINKS:** ~~[[#^a]]~~", "- 🪢 **LINKS:** ![[#^a]]"]
    {
        let (count, canonical, _) = accepted_links(line); // LK7
        assert_eq!((count, canonical), (1, false), "{line}");
    }

    let (count, canonical, labels) =
        accepted_links("- 🪢 **LINKS:** see [[#^a]] • see also [[#^a]]"); // LK8
    assert_eq!(count, 2);
    assert!(!canonical);
    assert_eq!(labels, [Some("see".into()), Some("see also".into())]);

    let (_, _, labels) = accepted_links("- 🪢 **LINKS:** see:   also [[#^a]]"); // LK9
    assert_eq!(labels, [Some("see: also".into())]);

    assert_eq!(parse_links_line("- 🪢 **LINKS:**"), LinksLine::Empty); // LK10

    for line in [
        "- 🪢 **LINKS:** [[#^a]] until Friday", // LK11
        "- 🪢 **LINKS:** [[#^a",                // LK12
        "- 🪢 **LINKS:** [[note]]",
        "- 🪢 **LINKS:** [[note#Heading]]",
        "- 🪢 **LINKS:** • ,",
        "- 🪢 **LINKS:** see (old) [[#^a]]", // LK13
        "- 🪢 **LINKS:** \"quoted\" [[#^a]]",
    ] {
        assert_eq!(parse_links_line(line), LinksLine::Malformed, "{line}");
    }

    for line in [
        "- LINKS: [[ref#^a]]", // LK14
        "- **links:** [[#^a]]",
        "- **RELATED:** [[#^a]]",
        "- **SEE ALSO:** [[#^a]]",
        "🪢 **LINKS:** [[#^a]]", // LK15 no marker
        "> - 🪢 **LINKS:** [[#^a]]",
    ] {
        assert_eq!(parse_links_line(line), LinksLine::NotALine, "{line}");
        assert!(!is_links_line(line), "{line}");
    }

    let (count, canonical, labels) = accepted_from(
        "- 🪢 **LINKED FROM:** [[bob#^fix]] (follow-up of) • [[#^b]]",
    ); // LK16
    assert_eq!(count, 2);
    assert!(!canonical); // unsorted vs LK20
    assert_eq!(labels, [Some("follow-up of".into()), None]);
    let (count, canonical, labels) = accepted_from(
        "- 🪢 **LINKED FROM:** [[#^b]] • [[bob#^fix]] (follow-up of)",
    );
    assert_eq!(count, 2);
    assert!(canonical);
    assert_eq!(labels, [None, Some("follow-up of".into())]);

    let (count, canonical, _) = accepted_from("- **BACKLINKS:** [[#^b]]"); // LK17
    assert_eq!((count, canonical), (1, false));

    for line in [
        "- 🪢 **LINKED FROM:** [[#^b]] extra",
        "- 🪢 **LINKED FROM:** [[#^b]] (a) (b)",
    ] {
        assert_eq!(
            parse_linked_from_line(line),
            LinkedFromLine::Malformed,
            "{line}"
        ); // LK18
    }
}

#[test]
fn lk15_discovery_skips_fenced_grandchild_and_non_task() {
    let lines = [
        "- [ ] #task Parent ^parent",
        "  - first user child",
        "    - 🪢 **LINKS:** [[#^nested]]",
        "  - 🪢 **LINKS:** [[#^a]]",
        "  ```md",
        "  - 🪢 **LINKS:** [[#^fenced]]",
        "  ```",
        "- prose, not a task",
        "  - 🪢 **LINKS:** [[#^under-prose]]",
    ];
    let fenced = BTreeSet::from([4, 5, 6]);
    let children = links_children_of(&lines, &fenced, 0);
    assert_eq!(
        children
            .iter()
            .map(|child| child.line_index)
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert!(links_children_of(&lines, &fenced, 7).is_empty());
}

#[test]
fn lk19_cross_parse_vectors() {
    let links = "- 🪢 **LINKS:** [[#^a]]";
    let linked_from = "- 🪢 **LINKED FROM:** [[#^b]]";
    let depends = "- ⛓️ **DEPENDS ON:** [[#^a]]"; // DP1
    let blocks = "- 🧱 **BLOCKS:** [[#^a]]"; // BK1
    assert_eq!(
        dep_parse::parse_dependency_line(links),
        DependencyLine::NotALine
    );
    assert_eq!(
        dep_parse::parse_dependency_line(linked_from),
        DependencyLine::NotALine
    );
    assert_eq!(blocks::parse_blocks_line(links), BlocksLine::NotALine);
    assert_eq!(blocks::parse_blocks_line(linked_from), BlocksLine::NotALine);
    assert_eq!(parse_links_line(linked_from), LinksLine::NotALine);
    assert_eq!(parse_linked_from_line(links), LinkedFromLine::NotALine);
    assert_eq!(parse_links_line(depends), LinksLine::NotALine);
    assert_eq!(parse_links_line(blocks), LinksLine::NotALine);
    assert_eq!(parse_linked_from_line(depends), LinkedFromLine::NotALine);
    assert_eq!(parse_linked_from_line(blocks), LinkedFromLine::NotALine);
}

#[test]
fn lk20_codepoint_sort_matches_bk21() {
    let mut entries = vec![
        ("[[zeta#^e]]".to_string(), None),
        ("[[élan#^d]]".to_string(), None),
        ("[[cash#^c]]".to_string(), None),
        ("[[Cash#^a]]".to_string(), None),
        ("[[#^b]]".to_string(), None),
    ];
    super::format::sort_linked_from_entries(&mut entries);
    assert_eq!(
        entries
            .iter()
            .map(|(link, _)| link.as_str())
            .collect::<Vec<_>>(),
        [
            "[[#^b]]",
            "[[Cash#^a]]",
            "[[cash#^c]]",
            "[[zeta#^e]]",
            "[[élan#^d]]",
        ]
    );
    assert_eq!(
        format_linked_from_line(
            "",
            &[
                ("[[zeta#^e]]".into(), None),
                ("[[élan#^d]]".into(), None),
                ("[[cash#^c]]".into(), None),
                ("[[Cash#^a]]".into(), None),
                ("[[#^b]]".into(), None),
            ]
        ),
        "- 🪢 **LINKED FROM:** [[#^b]] • [[Cash#^a]] • [[cash#^c]] • [[zeta#^e]] • [[élan#^d]]"
    );
}

#[test]
fn mc15_recognisers_never_claim_relation_lines() {
    let links = "- 🪢 **LINKS:** [[#^a]]";
    let linked_from = "- 🪢 **LINKED FROM:** [[#^b]] (see)";
    for line in [links, linked_from] {
        assert_eq!(legacy_child_reference(line), None, "{line}");
        assert!(parse_managed_task_log_marker(line).is_none(), "{line}");
        assert!(!dep_parse::is_dependency_line(line), "{line}");
        assert!(!blocks::is_blocks_line(line), "{line}");
        assert!(!blocks::is_cancel_log(line), "{line}");
        assert!(!super::parse::is_subprojects_line(line), "{line}");
        assert!(is_relation_line(line), "{line}");
    }
    for line in [
        "- ⛓️ **DEPENDS ON:** [[#^a]]",
        "- 🧱 **BLOCKS:** [[#^a]]",
        "- 🗓️ **SCHEDULE LOG**",
        "- 🛠️ **WORK LOG**",
        "- ❌ **CANCEL LOG**",
        "- 🧩 **Sub-projects:** [[Child]]",
    ] {
        assert!(!is_links_line(line), "{line}");
        assert!(!is_linked_from_line(line), "{line}");
        assert!(!is_relation_line(line), "{line}");
    }
}

#[test]
fn lw_planner_write_vectors() {
    let empty = ["- [ ] #task T ^t"];
    let add_t = LinksEdit {
        add: vec![("[[#^t]]".into(), None)],
        ..LinksEdit::default()
    };
    match plan_links_edit(&empty, &BTreeSet::new(), 0, &add_t) {
        LinksLinePlan::Edits { edits, report } => {
            assert_eq!(report.added, ["[[#^t]]"]);
            match &edits[0] {
                RelationEdit::Insert { text, .. } => {
                    assert_eq!(text, "\t- 🪢 **LINKS:** [[#^t]]"); // LW1
                }
                other => panic!("expected insert, got {other:?}"),
            }
        }
        other => panic!("expected edits, got {other:?}"),
    }

    let existing = ["- [ ] #task T ^t", "  - 🪢 **LINKS:** [[#^a]]"];
    let add_see = LinksEdit {
        add: vec![("[[#^t]]".into(), Some("see".into()))],
        ..LinksEdit::default()
    };
    match plan_links_edit(&existing, &BTreeSet::new(), 0, &add_see) {
        LinksLinePlan::Edits { edits, report } => {
            assert_eq!(report.added, ["[[#^t]]"]);
            match &edits[0] {
                RelationEdit::Replace { text, .. } => {
                    assert_eq!(text, "  - 🪢 **LINKS:** [[#^a]] • see [[#^t]]");
                    // LW2
                }
                other => panic!("expected replace, got {other:?}"),
            }
        }
        other => panic!("expected edits, got {other:?}"),
    }

    let same = LinksEdit {
        add: vec![("[[#^a]]".into(), None)],
        ..LinksEdit::default()
    };
    assert_eq!(
        plan_links_edit(&existing, &BTreeSet::new(), 0, &same),
        LinksLinePlan::Noop
    ); // LW3

    let relabel = LinksEdit {
        add: vec![("[[#^a]]".into(), Some("see".into()))],
        ..LinksEdit::default()
    };
    match plan_links_edit(&existing, &BTreeSet::new(), 0, &relabel) {
        LinksLinePlan::Edits { edits, report } => {
            assert_eq!(report.relabeled, ["[[#^a]]"]);
            match &edits[0] {
                RelationEdit::Replace { text, .. } => {
                    assert_eq!(text, "  - 🪢 **LINKS:** see [[#^a]]"); // LW4
                }
                other => panic!("expected replace, got {other:?}"),
            }
        }
        other => panic!("expected edits, got {other:?}"),
    }

    let remove_only = LinksEdit {
        remove: BTreeSet::from(["[[#^a]]".into()]),
        ..LinksEdit::default()
    };
    match plan_links_edit(&existing, &BTreeSet::new(), 0, &remove_only) {
        LinksLinePlan::Edits { edits, report } => {
            assert_eq!(report.removed, ["[[#^a]]"]);
            assert_eq!(edits, [RelationEdit::Remove { line_index: 1 }]); // LW5
        }
        other => panic!("expected edits, got {other:?}"),
    }

    let three = [
        "- [ ] #task T ^t",
        "  - 🪢 **LINKS:** [[#^a]] • [[#^b]] • [[#^c]]",
    ];
    let remove_middle = LinksEdit {
        remove: BTreeSet::from(["[[#^b]]".into()]),
        ..LinksEdit::default()
    };
    match plan_links_edit(&three, &BTreeSet::new(), 0, &remove_middle) {
        LinksLinePlan::Edits { edits, .. } => match &edits[0] {
            RelationEdit::Replace { text, .. } => {
                assert_eq!(text, "  - 🪢 **LINKS:** [[#^a]] • [[#^c]]"); // LW6
            }
            other => panic!("expected replace, got {other:?}"),
        },
        other => panic!("expected edits, got {other:?}"),
    }
}

#[test]
fn lw7_lw8_insert_slots() {
    let managed = [
        "- [ ] #task T ^t",
        "  - ❌ **CANCEL LOG**",
        "    - archived note",
        "  - ⛓️ **DEPENDS ON:** [[#^up]]",
        "    - nested dependency note",
        "  - 🧱 **BLOCKS:** [[#^down]]",
        "  - 🧩 **Sub-projects:** [[Child]]",
        "  - user child",
    ];
    let links_slot = relation_insert_slot(
        &managed,
        &BTreeSet::new(),
        0,
        RelationSlotKind::Links,
    );
    assert_eq!(links_slot.line_index, 7); // LW7 after Sub-projects
    let with_links = [
        "- [ ] #task T ^t",
        "  - 🧩 **Sub-projects:** [[Child]]",
        "  - 🪢 **LINKS:** [[#^a]]",
        "  - user child",
    ];
    let from_slot = relation_insert_slot(
        &with_links,
        &BTreeSet::new(),
        0,
        RelationSlotKind::LinkedFrom,
    );
    assert_eq!(from_slot.line_index, 3); // LW7 after LINKS

    let user_first = [
        "- [ ] #task T ^t",
        "  - user child",
        "  - 🗓️ **SCHEDULE LOG**",
    ];
    let slot = relation_insert_slot(
        &user_first,
        &BTreeSet::new(),
        0,
        RelationSlotKind::Links,
    );
    assert_eq!(slot.line_index, 1); // LW8 first child
}

#[test]
fn lw9_merges_duplicate_links_lines() {
    let duplicate = [
        "- [ ] #task T ^t",
        "  - 🪢 **LINKS:** see [[#^a]]",
        "  - 🪢 **LINKS:** other [[#^a]] • [[#^b]]",
    ];
    let add = LinksEdit {
        add: vec![("[[#^c]]".into(), None)],
        ..LinksEdit::default()
    };
    match plan_links_edit(&duplicate, &BTreeSet::new(), 0, &add) {
        LinksLinePlan::Edits { edits, .. } => {
            assert_eq!(edits.len(), 2);
            match &edits[0] {
                RelationEdit::Replace { text, .. } => {
                    assert_eq!(
                        text,
                        "  - 🪢 **LINKS:** see [[#^a]] • [[#^b]] • [[#^c]]"
                    );
                }
                other => panic!("expected replace, got {other:?}"),
            }
            assert_eq!(edits[1], RelationEdit::Remove { line_index: 2 });
        }
        other => panic!("expected edits, got {other:?}"),
    }
}

#[test]
fn lw10_refuses_malformed_links_line() {
    let malformed =
        ["- [ ] #task T ^t", "  - 🪢 **LINKS:** [[#^a]] until Friday"];
    let add = LinksEdit {
        add: vec![("[[#^b]]".into(), None)],
        ..LinksEdit::default()
    };
    assert_eq!(
        plan_links_edit(&malformed, &BTreeSet::new(), 0, &add),
        LinksLinePlan::Malformed
    );
}

#[test]
fn lw11_label_rules_on_plan() {
    let empty = ["- [ ] #task T ^t"];
    let collapse = LinksEdit {
        add: vec![("[[#^t]]".into(), Some(" Follow-up  of:".into()))],
        ..LinksEdit::default()
    };
    match plan_links_edit(&empty, &BTreeSet::new(), 0, &collapse) {
        LinksLinePlan::Edits { edits, .. } => match &edits[0] {
            RelationEdit::Insert { text, .. } => {
                assert!(text.contains("Follow-up of [[#^t]]"), "{text}");
            }
            other => panic!("expected insert, got {other:?}"),
        },
        other => panic!("expected edits, got {other:?}"),
    }
    let too_long = LinksEdit {
        add: vec![("[[#^t]]".into(), Some("a".repeat(41)))],
        ..LinksEdit::default()
    };
    assert_eq!(
        plan_links_edit(&empty, &BTreeSet::new(), 0, &too_long),
        LinksLinePlan::InvalidLabel(LabelError::TooLong)
    );
    let bad_char = LinksEdit {
        add: vec![("[[#^t]]".into(), Some("see (old)".into()))],
        ..LinksEdit::default()
    };
    assert_eq!(
        plan_links_edit(&empty, &BTreeSet::new(), 0, &bad_char),
        LinksLinePlan::InvalidLabel(LabelError::ForbiddenCharacter)
    );
}

#[test]
fn lw12_reuses_four_space_indent() {
    let lines = ["- [ ] #task T ^t", "    - user child"];
    let slot = relation_insert_slot(
        &lines,
        &BTreeSet::new(),
        0,
        RelationSlotKind::Links,
    );
    assert_eq!(slot.indent, "    ");
    assert_eq!(slot.line_index, 1);
}

#[test]
fn lw13_canonical_link_forms() {
    let index = NoteIndex::from_paths([
        PathBuf::from("note.md"),
        PathBuf::from("dir/note.md"),
        PathBuf::from("other.md"),
        PathBuf::from("done/x_done.md"),
    ]);
    assert_eq!(
        canonical_link(
            &PathBuf::from("other.md"),
            "t",
            &PathBuf::from("body.md"),
            &index
        ),
        "[[other#^t]]"
    );
    assert_eq!(
        canonical_link(
            &PathBuf::from("dir/note.md"),
            "t",
            &PathBuf::from("body.md"),
            &index
        ),
        "[[dir/note#^t]]"
    );
    assert_eq!(
        canonical_link(
            &PathBuf::from("done/x_done.md"),
            "t",
            &PathBuf::from("body.md"),
            &index
        ),
        "[[done/x_done#^t]]"
    );
}

#[test]
fn rule6_linked_from_planner_table() {
    let desired = BTreeSet::from([("[[#^s]]".to_string(), None)]);
    let missing = ["- [ ] #task T ^t", "  - child"];
    assert!(matches!(
        plan_linked_from_line(&missing, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Edits { report, .. }
            if report.kind == BlocksUpdateKind::Inserted
    ));

    let rewritten = ["- [ ] #task T ^t", "  - 🪢 **LINKED FROM:** [[#^old]]"];
    assert!(matches!(
        plan_linked_from_line(&rewritten, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Edits { report, .. }
            if report.kind == BlocksUpdateKind::Rewritten
    ));

    let noncanonical = ["- [ ] #task T ^t", "  - **BACKLINKS:** [[#^s]]"];
    assert!(matches!(
        plan_linked_from_line(&noncanonical, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Edits { report, .. }
            if report.kind == BlocksUpdateKind::Canonicalized
    ));

    let remove = ["- [ ] #task T ^t", "  - 🪢 **LINKED FROM:** [[#^s]]"];
    assert!(matches!(
        plan_linked_from_line(
            &remove,
            &BTreeSet::new(),
            0,
            &BTreeSet::new()
        ),
        BlocksLinePlan::Edits { report, .. }
            if report.kind == BlocksUpdateKind::Removed
    ));

    let duplicate = [
        "- [ ] #task T ^t",
        "  - 🪢 **LINKED FROM:** [[#^s]]",
        "  - 🪢 **LINKED FROM:** [[#^s]]",
    ];
    assert!(matches!(
        plan_linked_from_line(&duplicate, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Edits { report, .. }
            if report.kind == BlocksUpdateKind::Deduplicated
    ));

    let malformed =
        ["- [ ] #task T ^t", "  - 🪢 **LINKED FROM:** [[#^s]] extra"];
    assert_eq!(
        plan_linked_from_line(&malformed, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Malformed
    );
}

#[test]
fn linked_from_delta_keeps_labels_and_sorts() {
    let existing = match parse_linked_from_line(
        "- 🪢 **LINKED FROM:** [[z#^last]] (see) • [[#^keep]]",
    ) {
        LinkedFromLine::Accepted { entries, .. } => entries,
        other => panic!("expected accepted line, got {other:?}"),
    };
    let adds = BTreeMap::from([(
        "new".to_string(),
        ("[[a#^new]]".to_string(), Some("next".into())),
    )]);
    let removes = BTreeSet::from(["last".to_string()]);
    let desired = linked_from_desired_after_delta(
        &existing,
        |entry| Some((entry.block_id.clone(), entry.canonical_text())),
        &adds,
        &removes,
    );
    assert_eq!(
        desired,
        [
            ("[[#^keep]]".into(), None),
            ("[[a#^new]]".into(), Some("next".into())),
        ]
    );
}

#[test]
fn formats_canonical_lines() {
    assert_eq!(
        format_links_line(
            "  ",
            &[
                ("[[#^a]]".into(), Some("see".into())),
                ("[[cash#^b]]".into(), None),
            ]
        ),
        "  - 🪢 **LINKS:** see [[#^a]] • [[cash#^b]]"
    );
}

#[test]
fn linked_from_children_skip_nested() {
    let lines = [
        "- [ ] #task T ^t",
        "  - user",
        "    - 🪢 **LINKED FROM:** [[#^nested]]",
        "  - 🪢 **LINKED FROM:** [[#^a]]",
    ];
    let children = linked_from_children_of(&lines, &BTreeSet::new(), 0);
    assert_eq!(
        children
            .iter()
            .map(|child| child.line_index)
            .collect::<Vec<_>>(),
        vec![3]
    );
}

fn pair_writable(here: RecordKind, there: RecordKind) -> PairRecords {
    PairRecords {
        here,
        here_writability: Writability::Writable,
        there,
        there_writability: Writability::Writable,
    }
}

fn pair_archived_there(here: RecordKind, there: RecordKind) -> PairRecords {
    PairRecords {
        here,
        here_writability: Writability::Writable,
        there,
        there_writability: Writability::Archived,
    }
}

// LW4 (narrowed): re-adding with a new non-empty label relabels in
// place; the same label is a no-op.
#[test]
fn lw4_narrowed_relabel_keeps_order() {
    let labelled = ["- [ ] #task T ^t", "  - 🪢 **LINKS:** see [[#^a]]"];
    let relabel = LinksEdit {
        add: vec![("[[#^a]]".into(), Some("other".into()))],
        ..LinksEdit::default()
    };
    match plan_links_edit(&labelled, &BTreeSet::new(), 0, &relabel) {
        LinksLinePlan::Edits { report, edits } => {
            assert_eq!(report.relabeled, ["[[#^a]]"]);
            match &edits[0] {
                RelationEdit::Replace { text, .. } => {
                    assert_eq!(text, "  - 🪢 **LINKS:** other [[#^a]]");
                }
                other => panic!("expected replace, got {other:?}"),
            }
        }
        other => panic!("expected edits, got {other:?}"),
    }

    let same = LinksEdit {
        add: vec![("[[#^a]]".into(), Some("see".into()))],
        ..LinksEdit::default()
    };
    assert_eq!(
        plan_links_edit(&labelled, &BTreeSet::new(), 0, &same),
        LinksLinePlan::Noop
    );
}

// LW16 (line level): a bare add never strips a label.
#[test]
fn lw16_bare_add_keeps_label() {
    let labelled = ["- [ ] #task T ^t", "  - 🪢 **LINKS:** see [[#^a]]"];
    let bare = LinksEdit {
        add: vec![("[[#^a]]".into(), None)],
        ..LinksEdit::default()
    };
    assert_eq!(
        plan_links_edit(&labelled, &BTreeSet::new(), 0, &bare),
        LinksLinePlan::Noop
    );
}

// LW15: a plain add on a pair that already holds a plain record is
// `unchanged`, whichever side stores it.
#[test]
fn lw15_plain_add_with_plain_pair_is_unchanged() {
    for (here, there) in [
        (RecordKind::Plain, RecordKind::Absent),
        (RecordKind::Absent, RecordKind::Plain),
        (RecordKind::Plain, RecordKind::Plain),
    ] {
        let plan =
            plan_pair_write(&pair_writable(here, there), &PairRequest::Link);
        assert_eq!(plan.action, PairAction::Unchanged, "{plan:?}");
        assert_eq!(plan.refusal, None);
        assert_eq!(plan.here_op, None);
        assert_eq!(plan.there_op, None);
        assert_eq!(plan.kind, Some(PairKind::Undirected));
    }
}

// LW16 (pair level): a plain add on a pair with a statement either way
// is `unchanged` and keeps the label.
#[test]
fn lw16_plain_add_with_statement_is_unchanged() {
    for (here, there) in [
        (RecordKind::Labelled("see".into()), RecordKind::Absent),
        (RecordKind::Absent, RecordKind::Labelled("see".into())),
        (
            RecordKind::Labelled("x".into()),
            RecordKind::Labelled("y".into()),
        ),
    ] {
        let plan =
            plan_pair_write(&pair_writable(here, there), &PairRequest::Link);
        assert_eq!(plan.action, PairAction::Unchanged, "{plan:?}");
        assert_eq!(plan.refusal, None);
        assert_eq!(plan.here_op, None);
        assert_eq!(plan.there_op, None);
        assert_eq!(plan.kind, Some(PairKind::Directed));
    }
}

// LW17: N —x→ T with writable T holding plain [[N]] moves the record.
#[test]
fn lw17_state_moves_reciprocal_plain() {
    let records = pair_writable(RecordKind::Absent, RecordKind::Plain);
    let plan = plan_pair_write(
        &records,
        &PairRequest::State {
            source: Side::Here,
            label: "x".into(),
        },
    );
    assert_eq!(plan.action, PairAction::Relabeled);
    assert_eq!(plan.refusal, None);
    assert_eq!(
        plan.here_op,
        Some(LinksOp::Add {
            label: Some("x".into())
        })
    );
    assert_eq!(plan.there_op, Some(LinksOp::Remove));
    assert_eq!(plan.moved_from, Some(Side::There));
    assert_eq!(plan.kind, Some(PairKind::Directed));
    assert_eq!(plan.here_mirror, MirrorDelta::Remove);
    assert_eq!(
        plan.there_mirror,
        MirrorDelta::Upsert {
            label: Some("x".into())
        }
    );
}

// LW18: as LW17, but T's plain record lives in `done/`: it stays,
// subsumed and unwarned.
#[test]
fn lw18_state_keeps_archived_plain() {
    let records = pair_archived_there(RecordKind::Absent, RecordKind::Plain);
    let plan = plan_pair_write(
        &records,
        &PairRequest::State {
            source: Side::Here,
            label: "x".into(),
        },
    );
    assert_eq!(plan.action, PairAction::Relabeled);
    assert_eq!(plan.refusal, None);
    assert_eq!(
        plan.here_op,
        Some(LinksOp::Add {
            label: Some("x".into())
        })
    );
    assert_eq!(plan.there_op, None);
    assert_eq!(plan.moved_from, None);
    assert_eq!(plan.kind, Some(PairKind::Directed));
    assert_eq!(plan.here_mirror, MirrorDelta::Remove);
}

// LW19: clearing the only label keeps the record without its label.
#[test]
fn lw19_clear_only_label_keeps_plain() {
    let records =
        pair_writable(RecordKind::Labelled("x".into()), RecordKind::Absent);
    let plan = plan_pair_write(
        &records,
        &PairRequest::ClearLabel { source: Side::Here },
    );
    assert_eq!(plan.action, PairAction::Relabeled);
    assert_eq!(plan.refusal, None);
    assert_eq!(plan.here_op, Some(LinksOp::Relabel { label: None }));
    assert_eq!(plan.there_op, None);
    assert_eq!(plan.kind, Some(PairKind::Undirected));
}

// LW20: clearing N —x→ T while T —y→ N exists removes N's record.
#[test]
fn lw20_clear_with_reverse_statement_removes() {
    let records = pair_writable(
        RecordKind::Labelled("x".into()),
        RecordKind::Labelled("y".into()),
    );
    let plan = plan_pair_write(
        &records,
        &PairRequest::ClearLabel { source: Side::Here },
    );
    assert_eq!(plan.action, PairAction::Removed);
    assert_eq!(plan.refusal, None);
    assert_eq!(plan.here_op, Some(LinksOp::Remove));
    assert_eq!(plan.there_op, None);
    assert_eq!(plan.kind, Some(PairKind::Directed));
    assert_eq!(plan.there_mirror, MirrorDelta::Remove);
}

// LW21: swapping N —x→ T moves the statement; a reverse statement
// refuses it.
#[test]
fn lw21_swap_moves_statement() {
    let records =
        pair_writable(RecordKind::Labelled("x".into()), RecordKind::Absent);
    let plan = plan_pair_write(
        &records,
        &PairRequest::Swap {
            from: Side::Here,
            label: "x".into(),
        },
    );
    assert_eq!(plan.action, PairAction::Relabeled);
    assert_eq!(plan.refusal, None);
    assert_eq!(plan.here_op, Some(LinksOp::Remove));
    assert_eq!(
        plan.there_op,
        Some(LinksOp::Add {
            label: Some("x".into())
        })
    );
    assert_eq!(plan.moved_from, Some(Side::Here));
    assert_eq!(plan.kind, Some(PairKind::Directed));

    let blocked = pair_writable(
        RecordKind::Labelled("x".into()),
        RecordKind::Labelled("y".into()),
    );
    let refused = plan_pair_write(
        &blocked,
        &PairRequest::Swap {
            from: Side::Here,
            label: "x".into(),
        },
    );
    assert!(refused.refusal.is_some(), "{refused:?}");
    assert_eq!(refused.here_op, None);
    assert_eq!(refused.there_op, None);
}

// LW22: unlinking a doubly plain pair removes every writable record;
// removing the only statement while a redundant plain remains does too.
#[test]
fn lw22_unlink_and_remove_statement_clean_up() {
    let both = pair_writable(RecordKind::Plain, RecordKind::Plain);
    let plan = plan_pair_write(&both, &PairRequest::Unlink);
    assert_eq!(plan.action, PairAction::Removed);
    assert_eq!(plan.refusal, None);
    assert_eq!(plan.here_op, Some(LinksOp::Remove));
    assert_eq!(plan.there_op, Some(LinksOp::Remove));
    assert_eq!(plan.kind, None);

    let statement_plus_plain =
        pair_writable(RecordKind::Labelled("x".into()), RecordKind::Plain);
    let plan = plan_pair_write(
        &statement_plus_plain,
        &PairRequest::RemoveStatement { source: Side::Here },
    );
    assert_eq!(plan.action, PairAction::Removed);
    assert_eq!(plan.refusal, None);
    assert_eq!(plan.here_op, Some(LinksOp::Remove));
    assert_eq!(plan.there_op, Some(LinksOp::Remove));
    assert_eq!(plan.kind, None);
}

// LW23: N —x→ T with plain records on both sides relabels N in place
// and removes T's reciprocal.
#[test]
fn lw23_state_with_two_plains_relabels_and_moves() {
    let records = pair_writable(RecordKind::Plain, RecordKind::Plain);
    let plan = plan_pair_write(
        &records,
        &PairRequest::State {
            source: Side::Here,
            label: "x".into(),
        },
    );
    assert_eq!(plan.action, PairAction::Relabeled);
    assert_eq!(plan.refusal, None);
    assert_eq!(
        plan.here_op,
        Some(LinksOp::Relabel {
            label: Some("x".into())
        })
    );
    assert_eq!(plan.there_op, Some(LinksOp::Remove));
    assert_eq!(plan.moved_from, Some(Side::There));
    assert_eq!(plan.kind, Some(PairKind::Directed));
}

// Endpoint-swap property: plain pairs read the same from either end.
#[test]
fn plain_pairs_are_endpoint_symmetric() {
    let slots = [RecordKind::Absent, RecordKind::Plain];
    for here in slots.iter().cloned() {
        for there in slots.iter().cloned() {
            let forward = pair_writable(here.clone(), there.clone());
            let backward = pair_writable(there.clone(), here.clone());
            match (pair_state(&forward), pair_state(&backward)) {
                (
                    PairState::Undirected {
                        here_plain: fh,
                        there_plain: ft,
                    },
                    PairState::Undirected {
                        here_plain: bh,
                        there_plain: bt,
                    },
                ) => {
                    assert_eq!(fh, bt, "here must equal swapped there");
                    assert_eq!(ft, bh, "there must equal swapped here");
                }
                (PairState::Unrelated, PairState::Unrelated) => {}
                (forward_state, backward_state) => {
                    panic!(
                        "plain pair states must swap, got {forward_state:?} and {backward_state:?}"
                    );
                }
            }
            assert_eq!(
                mirror_entry(&forward, Side::Here),
                mirror_entry(&backward, Side::There),
                "here-mirror must equal swapped there-mirror"
            );
            assert_eq!(
                mirror_entry(&forward, Side::There),
                mirror_entry(&backward, Side::Here),
                "there-mirror must equal swapped here-mirror"
            );
            let forward_plan = plan_pair_write(&forward, &PairRequest::Link);
            let backward_plan = plan_pair_write(&backward, &PairRequest::Link);
            assert_eq!(forward_plan.action, backward_plan.action);
            assert_eq!(forward_plan.kind, backward_plan.kind);
            if matches!(here, RecordKind::Absent)
                && matches!(there, RecordKind::Absent)
            {
                // Unrelated: each perspective adds to its own Here.
                assert_eq!(
                    forward_plan.here_op,
                    Some(LinksOp::Add { label: None })
                );
                assert_eq!(forward_plan.there_op, None);
                assert_eq!(
                    backward_plan.here_op,
                    Some(LinksOp::Add { label: None })
                );
                assert_eq!(backward_plan.there_op, None);
            } else {
                assert_eq!(forward_plan.here_op, backward_plan.there_op);
                assert_eq!(forward_plan.there_op, backward_plan.here_op);
            }
        }
    }
}
