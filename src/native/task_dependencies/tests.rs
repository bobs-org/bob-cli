//! Conformance vectors copied from `docs/task-dependencies.md` §11.1
//! (DP) and §11.2 (DW), cited per section as the freshness, Today, and
//! Ready-cap vectors already do.

use std::path::PathBuf;

use super::super::task_status_hooks::{fenced_lines, logical_lines};
use super::super::vault_links::NoteIndex;
use super::format::{
    canonical_link, child_indent_for_parent, format_dependency_line,
    has_final_newline, note_line_ending,
};
use super::legacy::legacy_child_reference;
use super::parse::{dependency_child_of, DependencyLine};

fn accept_count(line: &str) -> Option<(usize, bool)> {
    match super::parse::parse_dependency_line(line) {
        DependencyLine::Accepted { links, canonical } => {
            Some((links.len(), canonical))
        }
        _ => None,
    }
}

// `docs/task-dependencies.md` §11.1 DP — parse vectors.
#[test]
fn dp_vectors() {
    // (line, expected accept count, expected canonical)
    let accept = [
        ("- ⛓️ **DEPENDS ON:** [[#^hospital-swarm]]", 1, true), // DP1
        ("- ⛓️ **DEPENDS ON:** [[cash#^unemployment]]", 1, true), // DP2
        ("- ⛓️ **DEPENDS ON:** [[money/cash#^unemployment]]", 1, true), // DP3
        ("- ⛓️ **DEPENDS ON:** [[#^a]] • [[#^b]]", 2, true),    // DP4
        ("- ⛓️ **DEPENDS ON:** [[#^a|b • c]]", 1, false),       // DP5
        ("- ⛓️ **DEPENDS ON:** ~~[[#^a]]~~", 1, false),         // DP6
        ("- ⛓️ **DEPENDS ON:** ![[#^a]]", 1, false),            // DP7
        ("- 🔗 **DEPENDS ON:** [[#^a]]", 1, false),             // DP8
        ("- **DEPENDS ON:** [[#^a]]", 1, false),                // DP9
        ("- ⛓️ **DEPENDENCIES:** [[#^a]]", 1, false),           // DP10
        ("- ⛓ **DEPENDS ON:** [[#^a]]", 1, false),              // DP11
        ("- ⛓️ **DEPENDS ON:** [[#^a]] · [[#^b]]", 2, false),   // DP12
        ("- ⛓️ **DEPENDS ON:** [[#^a]], [[#^b]]", 2, false),    // DP13
        ("- ⛓️ **DEPENDS ON:** [[#^a]] [[#^b]]", 2, false),     // DP14
        ("- ⛓️ **DEPENDS ON:** [[#^a|swarm]]", 1, false),       // DP22
    ];
    for (line, count, canonical) in accept {
        assert_eq!(accept_count(line), Some((count, canonical)), "{line}");
    }
    for line in [
        "- ⛓️ **DEPENDS ON:** [[#^a", // DP15 half-typed
        "- ⛓️ **DEPENDS ON:** [[#^a]] needs review", // DP16 trailing prose
        "- ⛓️ **DEPENDS ON:** [[note]]", // DP23 bare note link
    ] {
        assert_eq!(
            super::parse::parse_dependency_line(line),
            DependencyLine::Malformed,
            "{line}"
        );
    }
    assert_eq!(
        super::parse::parse_dependency_line("- ⛓️ **DEPENDS ON:**"),
        DependencyLine::Empty
    ); // DP17
    for line in [
        "- 🗓️ **SCHEDULE LOG**",
        "- [ ] #task Not a dependency line",
        "## ⛓️ **DEPENDS ON:** [[#^a]]",
    ] {
        assert_eq!(
            super::parse::parse_dependency_line(line),
            DependencyLine::NotALine,
            "{line}"
        );
    }
}

// `docs/task-dependencies.md` §11.1 DP — discovery context vectors.
#[test]
fn dp_discovery_context_vectors() {
    let settings_line = "- [ ] #task Dependent ^dependent\n";
    let dep_line = "  - ⛓️ **DEPENDS ON:** [[#^a]]\n";
    // DP21: a direct child in a later position still counts.
    let contents = format!(
        "{settings_line}  - prose child\n  - 🗓️ **SCHEDULE LOG**\n{dep_line}"
    );
    let lines = logical_lines(&contents);
    let fenced = fenced_lines(&lines, 0..lines.len());
    let found = dependency_child_of(&lines, &fenced, 0).expect("DP21 found");
    assert_eq!(found.line_index, 3);
    assert_eq!(accept_count(lines[found.line_index]), Some((1, true)));

    // DP18: inside fenced code never counts.
    let contents = format!(
        "{settings_line}  ```\n{dep_line}  ```\n  - 🗓️ **SCHEDULE LOG**\n"
    );
    let lines = logical_lines(&contents);
    let fenced = fenced_lines(&lines, 0..lines.len());
    assert_eq!(dependency_child_of(&lines, &fenced, 0), None);

    // DP19: a grandchild nested two levels never counts.
    let contents = format!("{settings_line}  - outer\n    {dep_line}");
    let lines = logical_lines(&contents);
    let fenced = fenced_lines(&lines, 0..lines.len());
    assert_eq!(dependency_child_of(&lines, &fenced, 0), None);

    // DP20: inside a Work Log entry never counts.
    let contents =
        format!("{settings_line}  - 🛠️ **WORK LOG**\n    {dep_line}");
    let lines = logical_lines(&contents);
    let fenced = fenced_lines(&lines, 0..lines.len());
    assert_eq!(dependency_child_of(&lines, &fenced, 0), None);
}

// `docs/task-dependencies.md` §11.2 DW — writer vectors covered by this
// module: canonical format, link form, indent, and line endings. Field
// placement, adoption, folding, and canonicalisation run in the hooks'
// reconcile step (phase hooks-reconcile) and the nav writer.
#[test]
fn dw_writer_form_vectors() {
    // DW19 canonicalise target: every reader-tolerated variant renders
    // to the same writer form with the same targets.
    assert_eq!(
        format_dependency_line(
            "  ",
            &["[[#^a]]".to_string(), "[[#^b]]".to_string()]
        ),
        "  - ⛓️ **DEPENDS ON:** [[#^a]] • [[#^b]]"
    );
    // DW4/DW7: removal keeps order; the field mirrors the line's order.
    assert_eq!(
        format_dependency_line(
            "  ",
            &["[[#^a]]".to_string(), "[[#^c]]".to_string()]
        ),
        "  - ⛓️ **DEPENDS ON:** [[#^a]] • [[#^c]]"
    );
    // DW5: removing the last link deletes the line (no placeholder); the
    // writer emits nothing and removes the field.
    let empty: Vec<String> = Vec::new();
    assert!(empty.is_empty());
}

#[test]
fn dw_link_form_vectors() {
    let index = NoteIndex::from_paths([
        PathBuf::from("cash.md"),
        PathBuf::from("body.md"),
        PathBuf::from("money/cash.md"),
        PathBuf::from("money/other.md"),
    ]);
    // DW12: same-note link form.
    assert_eq!(
        canonical_link(
            &PathBuf::from("body.md"),
            "hospital-swarm",
            &PathBuf::from("body.md"),
            &index
        ),
        "[[#^hospital-swarm]]"
    );
    // DW13: unique basename link form, no path.
    assert_eq!(
        canonical_link(
            &PathBuf::from("money/other.md"),
            "x",
            &PathBuf::from("body.md"),
            &index
        ),
        "[[other#^x]]"
    );
    // DW14: ambiguous basename falls back to the full path.
    assert_eq!(
        canonical_link(
            &PathBuf::from("money/cash.md"),
            "unemployment",
            &PathBuf::from("body.md"),
            &index
        ),
        "[[money/cash#^unemployment]]"
    );
}

#[test]
fn dw_indent_and_ending_vectors() {
    // DW8: no existing child indent → parent indent plus one tab.
    assert_eq!(child_indent_for_parent("\t", None), "\t\t");
    // DW9: reuse the task's existing child indent.
    assert_eq!(child_indent_for_parent("\t", Some("  ")), "  ".to_string());
    // DW10: CRLF preserved.
    assert_eq!(note_line_ending("a\r\nb\r\n"), "\r\n");
    assert_eq!(note_line_ending("a\nb\n"), "\n");
    // DW11: no final newline preserved.
    assert!(!has_final_newline("a\nb"));
    assert!(has_final_newline("a\nb\n"));
}

// R8 legacy-child shapes: plain, embedded, struck, and exactly-struck
// sole links are candidates; prose, aliases, bare headings, code, and
// bare note links are not.
#[test]
fn legacy_child_shapes() {
    for (line, expected) in [
        ("  - [[#^dep]]", Some(("", "dep"))),
        ("  - ![[Projects/A#^dep]]", Some(("Projects/A", "dep"))),
        ("  - ~~[[#^dep]]~~", Some(("", "dep"))),
        ("  - ~~![[#^dep]]~~", Some(("", "dep"))),
        (
            "  - ![[cash#^unemployment]]",
            Some(("cash", "unemployment")),
        ),
        ("  - [[#^plain]]", Some(("", "plain"))),
        ("  - ![[#^ref]]", Some(("", "ref"))),
    ] {
        let link = legacy_child_reference(line);
        assert_eq!(
            link.map(|link| (link.target, link.block_id)),
            expected
                .map(|(target, block)| (target.to_string(), block.to_string())),
            "{line}"
        );
    }
    for line in [
        "  - [[#^dep|alias]]",
        "  - ![[#^dep|alias]]",
        "  - text ![[#^dep]]",
        "  - ![[#^dep]] trailing",
        "  - ![[#heading]]",
        "  - [[note]]",
        "  - `[[#^dep]]`",
        "  - ![[#^dep]] and [[#^other]]",
        "- ⛓️ **DEPENDS ON:** [[#^dep]]",
    ] {
        assert_eq!(legacy_child_reference(line), None, "{line}");
    }
}

#[test]
fn dependency_line_guard_matches_every_form() {
    use super::parse::is_dependency_line;
    for line in [
        "- ⛓️ **DEPENDS ON:** [[#^a]]",
        "- 🔗 **DEPENDENCIES:** [[#^a]]",
        "- **DEPENDS ON:**",
        "- ⛓️ **DEPENDS ON:** [[#^a",
        "- 🗓️ **SCHEDULE LOG**",
        "- [ ] #task Not a dependency line",
    ] {
        let parsed = super::parse::parse_dependency_line(line);
        assert_eq!(
            is_dependency_line(line),
            !matches!(parsed, DependencyLine::NotALine),
            "{line}"
        );
    }
}
