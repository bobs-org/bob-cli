use std::path::{Path, PathBuf};

use super::{
    definition_index, select_section, select_section_with_options, stub_target,
    HostKind, Selector,
};

fn select<'a>(
    path: &Path,
    text: &'a str,
    selector: Selector<'_>,
) -> Result<super::ProseSection<'a>, super::SectionRefusal> {
    select_section(path, text, selector, |_| None)
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/native/prose/tests/fixtures/sections")
            .join(name),
    )
    .expect("section fixture")
}

fn refusal<'a>(
    path: &Path,
    text: &'a str,
    selector: Selector<'_>,
) -> super::SectionRefusal {
    select(path, text, selector).expect_err("section should be refused")
}

fn area(body: &str) -> String {
    format!("---\ntype: [[area]]\n---\n{body}")
}

#[test]
fn nested_paths_and_line_selection_follow_real_heading_boundaries() {
    let text = fixture("boundaries.md");
    let path = Path::new("area.md");
    let section = select(path, &text, Selector::Section("Alpha#Child#Skipped"))
        .expect("nested section");
    assert_eq!(section.host_kind, HostKind::Area);
    assert_eq!(section.raw_title, "Skipped");
    assert_eq!(section.heading_level, 4);
    assert_eq!(section.identity_path, "alpha#child#skipped");
    assert_eq!(section.moved_headings, []);

    let fake_line = text
        .lines()
        .position(|line| line == "## Fence fake")
        .expect("fake heading in fence")
        + 1;
    let by_line = select(path, &text, Selector::Line(fake_line))
        .expect("fenced fake heading stays inside Skipped");
    assert_eq!(by_line.raw_title, "Skipped");

    let setext = select(path, &text, Selector::Section("Setext child"))
        .expect("single-line Setext heading");
    assert_eq!(setext.raw_title, "Setext child");
    assert_eq!(setext.heading_level, 2);
    assert!(setext.body_range.start > 0);
}

#[test]
fn crlf_bom_unicode_empty_sections_and_seam_ranges_are_preserved() {
    let text = "\u{feff}---\r\ntype: [[area]]\r\n---\r\n## Café 東京\r\n\r\ntext\r\n## Empty\r\n## Next\r\n";
    let section =
        select(Path::new("area.md"), text, Selector::Section("café 東京"))
            .expect("unicode heading identity");
    assert_eq!(section.raw_title, "Café 東京");
    assert_eq!(section.leading_seam, 4..5);
    assert_eq!(section.trailing_seam.end, section.body_range.end);

    let empty = select(Path::new("area.md"), text, Selector::Section("Empty"))
        .expect("empty section remains eligible");
    assert_eq!(empty.body_range.start, empty.body_range.end);
}

#[test]
fn fake_headings_inside_quotes_lists_and_fences_never_become_boundaries() {
    let text = fixture("boundaries.md");
    let doc = super::scan::classify(&text);
    let headings = super::collect_headings(&doc);
    let titles: Vec<&str> = headings
        .iter()
        .map(|heading| heading.raw_title.as_str())
        .collect();
    assert!(!titles.contains(&"Quote fake"));
    assert!(!titles.contains(&"List fake"));
    assert!(!titles.contains(&"Fence fake"));
}

#[test]
fn project_owned_sections_refuse_but_authored_section_is_eligible() {
    let text = fixture("project.md");
    let path = Path::new("project.md");
    for title in ["Projects", "Tasks", "Next & In Progress"] {
        let error = refusal(path, &text, Selector::Section(title));
        assert_eq!(error.code, "structural-section", "{title}");
        assert!(error.line.is_some(), "{title}: {error:?}");
    }
    let authored = select(path, &text, Selector::Section("Authored"))
        .expect("authored project section");
    assert_eq!(authored.host_kind, HostKind::Project);
}

#[test]
fn catalogs_badge_rows_and_bob_marker_blocks_are_structural_in_any_host() {
    for callout in ["areas", "records"] {
        let text = area(&format!(
            "## Notes\n\n> [!{callout}]\n> owned\n\n## After\n\nbody\n"
        ));
        let error =
            refusal(Path::new("area.md"), &text, Selector::Section("Notes"));
        assert_eq!(error.code, "structural-section");
    }

    let badge = area(
        "## Topic\n\n[`⚪ 0 open`](#Tasks) · [`🔵 0 next/wip`](#Tasks#Next%20&%20In%20Progress) · [`🔴 0 blocked`](#Tasks#Blocked) · [`🟢 0 done/canceled`](#Tasks#Done%20&%20Canceled)\n",
    );
    assert_eq!(
        refusal(Path::new("area.md"), &badge, Selector::Section("Topic")).code,
        "structural-section"
    );

    let marker =
        area("## Topic\n\n<!-- bob:managed begin -->\nowned\n<!-- end -->\n");
    assert_eq!(
        refusal(Path::new("area.md"), &marker, Selector::Section("Topic")).code,
        "structural-section"
    );
}

#[test]
fn host_kind_rejects_other_types_mixed_types_and_excluded_locations() {
    let cases = [
        ("plain.md", "## Topic\nbody\n"),
        ("notebook.md", "---\ntype: [[notebook]]\n---\n## Topic\n"),
        (
            "mixed.md",
            "---\ntype: [\"[[area]]\", \"[[prose]]\"]\n---\n## Topic\n",
        ),
        ("done/old.md", "---\ntype: [[area]]\n---\n## Topic\n"),
        ("ref/paper.md", "---\ntype: [[prose]]\n---\n## Topic\n"),
        ("_templates/area.md", "---\ntype: [[area]]\n---\n## Topic\n"),
        ("2026/20261127.md", "---\ntype: [[area]]\n---\n## Topic\n"),
    ];
    for (path, text) in cases {
        let error = refusal(Path::new(path), text, Selector::Section("Topic"));
        assert_eq!(error.code, "not-host-note", "{path}");
        assert_eq!(error.line, None);
    }

    let legacy = "---\ntype: [[article]]\n---\n## Topic\nbody\n";
    assert_eq!(
        select(Path::new("essay.md"), legacy, Selector::Section("Topic"))
            .expect("legacy prose note")
            .host_kind,
        HostKind::Prose
    );
}

#[test]
fn selectors_report_required_missing_close_and_ambiguous_cases() {
    let text =
        area("Intro\n\n## Child\nbody\n## Repeat\none\n## Repeat\ntwo\n");
    let required = select_section_with_options(
        Path::new("area.md"),
        &text,
        None,
        None,
        |_| None,
    )
    .expect_err("selector required");
    assert_eq!(required.code, "selector-required");
    let both = select_section_with_options(
        Path::new("area.md"),
        &text,
        Some("Child"),
        Some(4),
        |_| None,
    )
    .expect_err("two selectors conflict");
    assert_eq!(both.code, "selector-conflict");

    let preamble_line =
        text.lines().position(|line| line == "Intro").unwrap() + 1;
    let preamble =
        refusal(Path::new("area.md"), &text, Selector::Line(preamble_line));
    assert_eq!(preamble.code, "no-section");
    assert!(preamble.hint.contains("Type a heading"));

    let missing =
        refusal(Path::new("area.md"), &text, Selector::Section("Chlid"));
    assert_eq!(missing.code, "no-section");
    assert!(missing.message.contains("Child"));

    let ambiguous =
        refusal(Path::new("area.md"), &text, Selector::Section("Repeat"));
    assert_eq!(ambiguous.code, "ambiguous-section");
    assert!(ambiguous.message.contains("8, 10"), "{ambiguous:?}");
}

#[test]
fn section_body_refusals_have_section_wording_and_lines() {
    let cases = [
        ("## Topic\n\n- [ ] #task do work\n", "holds-tasks"),
        ("## Topic\n\ntext ^ref\n", "holds-anchor"),
        ("## Topic\n\n<<<<<<< HEAD\n", "conflict-markers"),
    ];
    for (body, code) in cases {
        let text = area(body);
        let error =
            refusal(Path::new("area.md"), &text, Selector::Section("Topic"));
        assert_eq!(error.code, code);
        assert!(error.line.is_some());
        assert!(error.message.contains("section"));
        assert!(!error.hint.is_empty());
    }
}

#[test]
fn child_prose_stub_returns_inline_next_action_and_supports_both_stub_forms() {
    for body in ["- [[child_note|Display]]", "- ![[child_note]]"] {
        let text = area(&format!("## Topic\n\n{body}\n"));
        let error = select_section(
            Path::new("area.md"),
            &text,
            Selector::Section("Topic"),
            |stub| {
                (stub.target == "child_note")
                    .then(|| PathBuf::from("child_note.md"))
            },
        )
        .expect_err("recognized child note stub");
        assert_eq!(error.code, "stub-section");
        let next = error.next.expect("inline next action");
        assert_eq!(next.command, "inline");
        assert_eq!(next.note, PathBuf::from("child_note.md"));

        let section =
            select(Path::new("area.md"), &text, Selector::Section("Topic"))
                .expect("stub not recognized as a child by an empty resolver");
        let parsed = stub_target(&section).expect("syntactic stub");
        assert_eq!(parsed.target, "child_note");
        assert_eq!(parsed.embedded, body.contains("![["));
    }
}

#[test]
fn unclosed_fences_multiline_setext_owned_numbers_and_depth_have_named_refusals(
) {
    let unclosed = area("## Topic\n\ntext\n```md\nopen\n");
    assert_eq!(
        refusal(Path::new("area.md"), &unclosed, Selector::Section("Topic"))
            .code,
        "unclosed-fence"
    );

    let multiline = area("## Topic\n\nSetext title\ncontinued\n------\n");
    assert_eq!(
        refusal(Path::new("area.md"), &multiline, Selector::Section("Topic"))
            .code,
        "setext-multiline"
    );

    let numbered = area("## Topic\n\n### 1.5 Literal number\nbody\n");
    assert_eq!(
        refusal(Path::new("area.md"), &numbered, Selector::Section("Topic"))
            .code,
        "owned-number-heading"
    );
    assert_eq!(super::promoted_level(7, 1), Some(7));
    assert_eq!(super::promoted_level(3, 3), None);
}

#[test]
fn definition_scanner_casefolds_labels_and_ignores_protected_regions() {
    let text = area(
        "## Topic\n\n[t][Guide] [guide][] [GUIDE] [^Foot]\n`[code]`\n%% [comment] %%\n<!-- [html] -->\n```md\n[fence]\n```\n\n[guide]: https://example.test\n[^foot]: note\n",
    );
    let index = definition_index(&text);
    assert_eq!(
        index.definitions.get("guide").map(Vec::as_slice),
        Some(&[14][..])
    );
    assert_eq!(
        index.definitions.get("^foot").map(Vec::as_slice),
        Some(&[15][..])
    );
    assert_eq!(
        index.uses.get("guide").map(Vec::as_slice),
        Some(&[6, 6, 6][..])
    );
    assert_eq!(index.uses.get("^foot").map(Vec::as_slice), Some(&[6][..]));

    let references = fixture("references.md");
    let error =
        refusal(Path::new("area.md"), &references, Selector::Section("Move"));
    assert_eq!(error.code, "definition-boundary");
    assert_eq!(error.line, Some(6));
}

#[test]
fn section_exports_moved_headings_and_block_ids() {
    let text = area("## Topic\n\n### Child\nbody ^block-id\n#### Deep\ntext\n");
    let section =
        select(Path::new("area.md"), &text, Selector::Section("Topic"))
            .expect("eligible section");
    assert_eq!(section.moved_headings.len(), 2);
    assert_eq!(section.moved_headings[0].title, "Child");
    assert_eq!(section.moved_headings[0].original_level, 3);
    assert_eq!(section.moved_block_ids[0].id, "block-id");
}
