use std::collections::{BTreeMap, BTreeSet};

use super::{
    blocks_children_of, blocks_desired_after_delta, blocks_insert_slot,
    format_blocks_line, is_blocks_line, parse_blocks_line, plan_blocks_line,
    sort_blocks_links, BlocksEdit, BlocksLine, BlocksLinePlan,
    BlocksUpdateKind,
};

fn accepted(line: &str) -> (usize, bool) {
    match parse_blocks_line(line) {
        BlocksLine::Accepted { links, canonical } => (links.len(), canonical),
        other => panic!("expected accepted BLOCKS line, got {other:?}: {line}"),
    }
}

// BK1–BK21: `docs/task-dependencies.md` §14 parse and ordering vectors.
#[test]
fn bk_parse_vectors() {
    let cases = [
        ("- 🧱 **BLOCKS:** [[#^a]]", 1, true), // BK1
        (
            "- 🧱 **BLOCKS:** [[#^a]] • [[cash#^b]] • [[money/cash#^c]]",
            3,
            true,
        ), // BK2
        ("- 🧱 **BLOCKS:** [[#^a|b • c]]", 1, false), // BK3
        ("- **BLOCKS:** [[#^a]]", 1, false),   // BK4
        ("- 🚧 **BLOCKS:** [[#^a]]", 1, false), // BK5
        ("- ⛓️ **BLOCKS:** [[#^a]]", 1, false), // BK6
        ("- ⛓ **BLOCKS:** [[#^a]]", 1, false), // BK6
        ("- 🧱 **BLOCKING:** [[#^a]]", 1, false), // BK7
        ("- 🧱 **BLOCKS:** [[#^a]] · [[#^b]]", 2, false), // BK8
        ("- 🧱 **BLOCKS:** [[#^a]], [[#^b]]", 2, false), // BK8
        ("- 🧱 **BLOCKS:** [[#^a]] [[#^b]]", 2, false), // BK8
        ("- 🧱 **BLOCKS:** ~~[[#^a]]~~", 1, false), // BK9
        ("- 🧱 **BLOCKS:** ![[#^a]]", 1, false), // BK9
        ("- 🧱 **BLOCKS:** [[cash#^b]] • [[#^a]]", 2, false), // BK10
        ("- 🧱 **BLOCKS:** [[#^a]] • [[#^a]]", 2, false), // BK11
    ];
    for (line, count, canonical) in cases {
        assert_eq!(accepted(line), (count, canonical), "{line}");
    }

    assert_eq!(parse_blocks_line("- 🧱 **BLOCKS:**"), BlocksLine::Empty); // BK12
    for line in [
        "- 🧱 **BLOCKS:** [[#^a",                // BK13
        "- 🧱 **BLOCKS:** [[#^a]] until Friday", // BK14
        "- 🧱 **BLOCKS:** [[note]]",             // BK15
        "- 🧱 **BLOCKS:** [[note#Heading]]",     // BK15
        "- 🧱 **BLOCKS:** • ,",                  // BK15
    ] {
        assert_eq!(parse_blocks_line(line), BlocksLine::Malformed, "{line}");
    }
    for line in [
        "- 🧱 **blocks:** [[#^a]]",     // BK16
        "- 🧱 **BLOCKED BY:** [[#^a]]", // BK17
        "🧱 **BLOCKS:** [[#^a]]",       // BK18
        "> - 🧱 **BLOCKS:** [[#^a]]",   // BK18
        "## - 🧱 **BLOCKS:** [[#^a]]",  // BK18
    ] {
        assert_eq!(parse_blocks_line(line), BlocksLine::NotALine, "{line}");
    }
    assert!(!is_blocks_line("- 🧱 **blocks:** [[#^a]]"));
}

#[test]
fn bk_direct_child_discovery_vectors() {
    let lines = [
        "- [ ] #task Parent ^parent",
        "  - first user child",
        "    - 🧱 **BLOCKS:** [[#^nested]]",
        "  - 🧱 **BLOCKS:** [[#^a]]", // BK19: a later direct child counts
        "  - 🧱 **BLOCKS:** [[#^b]]", // duplicate managed line
        "  ```md",
        "  - 🧱 **BLOCKS:** [[#^fenced]]",
        "  ```",
        "- [ ] #task Other ^other",
        "  - 🧱 **BLOCKS:** [[#^other]]",
    ];
    let fenced = BTreeSet::from([5, 6, 7]);
    let children = blocks_children_of(&lines, &fenced, 0);
    assert_eq!(
        children
            .iter()
            .map(|child| child.line_index)
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert!(blocks_children_of(&lines, &fenced, 1).is_empty());
}

#[test]
fn bk_cross_parse_and_codepoint_sort_vectors() {
    let blocks = "- 🧱 **BLOCKS:** [[#^a]]"; // BK20
    assert_eq!(
        super::super::parse::parse_dependency_line(blocks),
        super::super::parse::DependencyLine::NotALine
    );
    let dependency = "- ⛓️ **DEPENDS ON:** [[#^a]]"; // BK20
    assert_eq!(parse_blocks_line(dependency), BlocksLine::NotALine);

    let mut links = vec![
        "[[zeta#^e]]".to_string(),
        "[[élan#^d]]".to_string(),
        "[[cash#^c]]".to_string(),
        "[[Cash#^a]]".to_string(),
        "[[#^b]]".to_string(),
    ];
    sort_blocks_links(&mut links);
    assert_eq!(
        links,
        [
            "[[#^b]]",
            "[[Cash#^a]]",
            "[[cash#^c]]",
            "[[zeta#^e]]",
            "[[élan#^d]]",
        ]
    ); // BK21
}

#[test]
fn formats_canonical_brick_line() {
    assert_eq!(
        format_blocks_line(
            "  ",
            &["[[#^a]]".to_string(), "[[cash#^b]]".to_string()]
        ),
        "  - 🧱 **BLOCKS:** [[#^a]] • [[cash#^b]]"
    );
}

#[test]
fn insert_slot_follows_depends_subtree_and_cancel_log() {
    let with_dependency = [
        "- [ ] #task T ^t",
        "  - ⛓️ **DEPENDS ON:** [[#^upstream]]",
        "    - nested dependency note",
        "  - user child",
    ];
    assert_eq!(
        blocks_insert_slot(&with_dependency, &BTreeSet::new(), 0).line_index,
        3
    );
    let with_cancel_log = [
        "- [ ] #task T ^t",
        "  - ❌ **CANCEL LOG**",
        "    - archived note",
        "  - user child",
    ];
    assert_eq!(
        blocks_insert_slot(&with_cancel_log, &BTreeSet::new(), 0).line_index,
        3
    );
}

#[test]
fn planner_covers_missing_rewritten_canonical_removed_duplicate_and_malformed()
{
    let desired =
        BTreeSet::from(["[[#^a]]".to_string(), "[[cash#^b]]".to_string()]);
    let missing = [
        "- [ ] #task T ^t",
        "  - ⛓️ **DEPENDS ON:** [[#^up]]",
        "  - child",
    ];
    assert_eq!(
        plan_blocks_line(&missing, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Edits {
            edits: vec![BlocksEdit::Insert {
                line_index: 2,
                text: "  - 🧱 **BLOCKS:** [[#^a]] • [[cash#^b]]".to_string(),
            }],
            report: super::BlocksLineReport {
                kind: BlocksUpdateKind::Inserted,
                added: vec!["[[#^a]]".to_string(), "[[cash#^b]]".to_string()],
                removed: vec![],
            },
        }
    );

    let rewritten = ["- [ ] #task T ^t", "  - 🧱 **BLOCKS:** [[#^old]]"];
    assert!(matches!(
        plan_blocks_line(&rewritten, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Edits { report, .. }
            if report.kind == BlocksUpdateKind::Rewritten
    ));

    let noncanonical =
        ["- [ ] #task T ^t", "  - **BLOCKS:** [[cash#^b]] · [[#^a]]"];
    assert!(matches!(
        plan_blocks_line(&noncanonical, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Edits { report, .. }
            if report.kind == BlocksUpdateKind::Canonicalized
    ));

    let empty = ["- [ ] #task T ^t", "  - 🧱 **BLOCKS:** [[#^old]]"];
    assert!(matches!(
        plan_blocks_line(&empty, &BTreeSet::new(), 0, &BTreeSet::new()),
        BlocksLinePlan::Edits { edits, report }
            if edits == vec![BlocksEdit::Remove { line_index: 1 }]
                && report.kind == BlocksUpdateKind::Removed
    ));

    let duplicate = [
        "- [ ] #task T ^t",
        "  - 🧱 **BLOCKS:** [[#^a]] • [[#^a]]",
        "  - 🧱 **BLOCKING:** [[#^a]]",
    ];
    assert!(matches!(
        plan_blocks_line(
            &duplicate,
            &BTreeSet::new(),
            0,
            &BTreeSet::from(["[[#^a]]".to_string()])
        ),
        BlocksLinePlan::Edits { edits, report }
            if edits.len() == 2 && report.kind == BlocksUpdateKind::Deduplicated
    ));

    let malformed = [
        "- [ ] #task T ^t",
        "  - 🧱 **BLOCKS:** [[#^a]] after",
        "  - 🧱 **BLOCKS:** [[#^b]]",
    ];
    assert_eq!(
        plan_blocks_line(&malformed, &BTreeSet::new(), 0, &desired),
        BlocksLinePlan::Malformed
    );
}

#[test]
fn delta_resolves_identities_then_applies_add_remove_and_sorts() {
    let existing =
        match parse_blocks_line("- 🧱 **BLOCKS:** [[z#^last]] • [[#^keep]]")
        {
            BlocksLine::Accepted { links, .. } => links,
            other => panic!("expected accepted line, got {other:?}"),
        };
    let adds = BTreeMap::from([("new".to_string(), "[[a#^new]]".to_string())]);
    let removes = BTreeSet::from(["last".to_string()]);
    let desired = blocks_desired_after_delta(
        &existing,
        |link| {
            let id = link.block_id.clone();
            let text = if link.target.is_empty() {
                format!("[[#^{}]]", link.block_id)
            } else {
                format!("[[{}#^{}]]", link.target, link.block_id)
            };
            Some((id, text))
        },
        &adds,
        &removes,
    );
    assert_eq!(desired, ["[[#^keep]]", "[[a#^new]]"]);
}
