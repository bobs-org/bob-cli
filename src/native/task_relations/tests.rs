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
use super::parse::{
    linked_from_children_of, linked_from_label_spans, links_children_of,
    links_label_spans, parse_linked_from_line, parse_links_line,
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

#[test]
fn relation_label_spans_cover_links_gaps_mirrors_and_aliases() {
    let links = "- 🪢 **LINKS:** old [[#^a]] • [[#^b|old]]";
    let spans = links_label_spans(links).expect("accepted LINKS spans");
    assert_eq!(
        spans
            .iter()
            .map(|span| (&links[span.range.clone()], span.label.as_str()))
            .collect::<Vec<_>>(),
        [("old", "old"), ("old", "old")]
    );

    let mirrors = "- 🪢 **LINKED FROM:** [[#^a]] (old) • [[#^b|old]]";
    let spans =
        linked_from_label_spans(mirrors).expect("accepted mirror spans");
    assert_eq!(
        spans
            .iter()
            .map(|span| (&mirrors[span.range.clone()], span.label.as_str()))
            .collect::<Vec<_>>(),
        [("old", "old"), ("old", "old")]
    );
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
