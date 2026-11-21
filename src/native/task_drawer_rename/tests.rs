use std::path::Path;

use super::*;

fn settings() -> NoteTaskSettings {
    note_tasks::read_settings(Path::new("/no-bob-vault"))
}

fn spec() -> RenameSpec {
    RenameSpec::parse("requirements", "notes").expect("valid spec")
}

fn note(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

const PROJECT: &str = "---\ntype: \"[[project]]\"\n---\n";

fn project(lines: &[&str]) -> String {
    format!("{PROJECT}{}", note(lines))
}

/// Run the pass, check the safety invariants, and check that a second pass
/// reports nothing.
fn renamed(contents: &str) -> NoteRename {
    renamed_with(contents, &spec())
}

fn renamed_with(contents: &str, spec: &RenameSpec) -> NoteRename {
    let pass = rename_note(contents, spec, &settings());
    if let Some(after) = &pass.updated {
        content_preserved(contents, after, spec, &pass)
            .expect("content preserved");
        settled(after, spec, &settings()).expect("settled after one pass");
        assert!(
            !after.contains("[ ] REQ")
                && after.matches("- [").count()
                    == contents.matches("- [").count(),
            "no bullet becomes a req"
        );
    }
    pass
}

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

#[test]
fn spec_keys_old_and_cases_new_for_both_containers() {
    let spec =
        RenameSpec::parse(" requirements ", "future  work").expect("spec");
    assert_eq!(spec.old_key, "REQUIREMENTS");
    assert_eq!(spec.new_key, "FUTURE WORK");
    assert_eq!(spec.drawer_title, "FUTURE WORK");
    assert_eq!(spec.section_title, "Future Work");
}

#[test]
fn spec_refuses_reserved_invalid_and_identical_names() {
    let code =
        |old: &str, new: &str| RenameSpec::parse(old, new).unwrap_err().code;
    assert_eq!(code("requirements", "work log"), "reserved_drawer");
    assert_eq!(code("requirements", "tasks"), "reserved_drawer");
    assert_eq!(code("tasks", "notes"), "reserved_drawer");
    assert_eq!(code("requirements", "café"), "invalid_drawer_title");
    assert_eq!(code("requirements", "  "), "invalid_drawer_title");
    assert_eq!(code("", "notes"), "invalid_drawer_title");
    assert_eq!(code("Notes", "NOTES "), "same_drawer");
}

// ---------------------------------------------------------------------------
// Task drawers
// ---------------------------------------------------------------------------

#[test]
fn a_lone_drawer_is_retitled_in_place() {
    let contents = note(&[
        "- [ ] #task Ship the exporter ^export",
        "\t- REQUIREMENTS",
        "\t\t- [ ] not a req: still a drawer entry",
        "\t\t- Handles Windows files ^win",
        "- [x] #task Shipped ✅ 2026-11-20",
        "\t- requirements",
        "\t- REQUIREMENTS",
        "\t\t- done already",
    ]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Ship the exporter ^export",
                "\t- NOTES",
                "\t\t- [ ] not a req: still a drawer entry",
                "\t\t- Handles Windows files ^win",
                "- [x] #task Shipped ✅ 2026-11-20",
                "\t- requirements",
                "\t- NOTES",
                "\t\t- done already",
            ])
            .as_str()
        )
    );
    let lines: Vec<(usize, bool)> = pass
        .drawers
        .iter()
        .map(|change| (change.line, change.open))
        .collect();
    assert_eq!(lines, [(1, true), (5, false)]);
    assert_eq!(pass.drawers[0].titles, ["REQUIREMENTS"]);
    assert_eq!(pass.drawers[0].block_id.as_deref(), Some("export"));
    assert_eq!(pass.renamed(), 2);
    assert_eq!(pass.merged(), 0);
}

#[test]
fn an_existing_new_drawer_merges_at_the_earliest_position() {
    let contents = note(&[
        "- [ ] #task Ship",
        "\t- REQUIREMENTS",
        "\t\t- first",
        "\t\t\t- nested ^keep",
        "\t- a loose note",
        "\t- NOTES",
        "\t\t- second",
        "\t\t- first",
        "\t- 🛠️ **WORK LOG**",
        "\t\t- *2026-11-20* — started",
    ]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Ship",
                "\t- NOTES",
                "\t\t- first",
                "\t\t\t- nested ^keep",
                "\t\t- second",
                "\t\t- first",
                "\t- a loose note",
                "\t- 🛠️ **WORK LOG**",
                "\t\t- *2026-11-20* — started",
            ])
            .as_str()
        )
    );
    assert_eq!(pass.drawers[0].titles, ["REQUIREMENTS", "NOTES"]);
    assert_eq!(pass.merged(), 1);
}

#[test]
fn old_after_new_appends_to_new_and_twice_old_merges_too() {
    let contents = note(&[
        "- [ ] #task Ship",
        "\t- NOTES",
        "\t\t- kept first",
        "\t- REQUIREMENTS",
        "\t\t- appended",
        "\t- REQUIREMENTS",
        "\t\t- appended again",
        "",
        "Trailing paragraph.",
    ]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Ship",
                "\t- NOTES",
                "\t\t- kept first",
                "\t\t- appended",
                "\t\t- appended again",
                "",
                "Trailing paragraph.",
            ])
            .as_str()
        )
    );
    assert_eq!(pass.renamed(), 2);
    assert_eq!(pass.merged(), 2);
}

#[test]
fn nested_owners_report_their_original_lines() {
    let contents = note(&[
        "- [ ] #task Parent",
        "\t- [ ] #task Child",
        "\t\t- REQUIREMENTS",
        "\t\t\t- child entry",
        "\t\t- NOTES",
        "\t\t\t- child note",
        "\t- REQUIREMENTS",
        "\t\t- parent entry",
    ]);
    let pass = renamed(&contents);
    let lines: Vec<usize> =
        pass.drawers.iter().map(|change| change.line).collect();
    assert_eq!(lines, [1, 2]);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Parent",
                "\t- [ ] #task Child",
                "\t\t- NOTES",
                "\t\t\t- child entry",
                "\t\t\t- child note",
                "\t- NOTES",
                "\t\t- parent entry",
            ])
            .as_str()
        )
    );
}

#[test]
fn merged_entries_take_the_kept_drawers_indentation() {
    let contents = note(&[
        "- [ ] #task Ship",
        "    - NOTES",
        "        - kept",
        "\t- REQUIREMENTS",
        "\t\t- moved",
        "\t\t\t- deeper",
    ]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Ship",
                "    - NOTES",
                "        - kept",
                "    \t- moved",
                "    \t\t- deeper",
            ])
            .as_str()
        )
    );
}

#[test]
fn crlf_and_a_missing_final_newline_are_kept() {
    let contents = "- [ ] #task Ship\r\n\t- NOTES\r\n\t\t- a\r\n\t- REQUIREMENTS\r\n\t\t- b";
    let pass = renamed(contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some("- [ ] #task Ship\r\n\t- NOTES\r\n\t\t- a\r\n\t\t- b")
    );
}

#[test]
fn a_last_drawer_keeps_the_blank_line_that_parts_the_block() {
    let contents = note(&[
        "- [ ] #task Ship",
        "\t- NOTES",
        "\t\t- a",
        "\t- REQUIREMENTS",
        "\t\t- b",
        "",
        "## Next",
    ]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            note(&[
                "- [ ] #task Ship",
                "\t- NOTES",
                "\t\t- a",
                "\t\t- b",
                "",
                "## Next"
            ])
            .as_str()
        )
    );
}

#[test]
fn a_note_without_old_plans_nothing() {
    let contents = note(&["- [ ] #task Ship", "\t- NOTES", "\t\t- a"]);
    assert_eq!(renamed(&contents), NoteRename::default());
}

// ---------------------------------------------------------------------------
// Project sections
// ---------------------------------------------------------------------------

#[test]
fn a_project_section_is_retitled_in_title_case() {
    let contents = project(&[
        "# Exporter",
        "- [ ] #task Ship the exporter ^prj",
        "",
        "## Requirements",
        "",
        "- Inspired by [[bob]]!",
        "",
        "## Tasks",
    ]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            project(&[
                "# Exporter",
                "- [ ] #task Ship the exporter ^prj",
                "",
                "## Notes",
                "",
                "- Inspired by [[bob]]!",
                "",
                "## Tasks",
            ])
            .as_str()
        )
    );
    assert_eq!(pass.sections.len(), 1);
    assert_eq!(pass.sections[0].line, 7);
    assert_eq!(pass.sections[0].titles, ["Requirements"]);
}

#[test]
fn an_empty_last_section_is_retitled() {
    let contents = project(&["- [ ] #task Ship ^prj", "", "## Requirements"]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(project(&["- [ ] #task Ship ^prj", "", "## Notes"]).as_str())
    );
}

#[test]
fn sections_merge_into_the_earliest_and_the_tail_does_not_grow() {
    let contents = project(&[
        "## Notes",
        "",
        "- a",
        "",
        "## Goals",
        "",
        "Ship it.",
        "",
        "## Requirements",
        "",
        "- r",
        "\t- r child",
        "### Detail",
        "Prose.",
    ]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            project(&[
                "## Notes",
                "",
                "- a",
                "- r",
                "\t- r child",
                "### Detail",
                "Prose.",
                "",
                "## Goals",
                "",
                "Ship it.",
            ])
            .as_str()
        )
    );
    assert_eq!(pass.sections[0].titles, ["Notes", "Requirements"]);
    assert_eq!(pass.merged(), 1);
}

#[test]
fn a_paragraph_body_is_merged_after_a_blank_line() {
    let contents = project(&[
        "## Requirements",
        "Prose one.",
        "## Notes",
        "Prose two.",
        "## Goals",
    ]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(
            project(&["## Notes", "Prose one.", "", "Prose two.", "## Goals"])
                .as_str()
        )
    );
}

#[test]
fn an_empty_kept_section_takes_the_moved_body() {
    let contents =
        project(&["## Notes", "## Requirements", "- r", "", "## Goals"]);
    let pass = renamed(&contents);
    assert_eq!(
        pass.updated.as_deref(),
        Some(project(&["## Notes", "", "- r", "", "## Goals"]).as_str())
    );
}

#[test]
fn an_ordinary_note_heading_is_excluded_not_renamed() {
    let contents = note(&["# Area", "## Requirements", "- a"]);
    let pass = renamed(&contents);
    assert_eq!(pass.updated, None);
    assert_eq!(
        pass.excluded,
        [Excluded {
            line: 2,
            text: "## Requirements".into(),
            reason: "an ordinary heading outside a project note",
        }]
    );
}

#[test]
fn a_heading_left_ambiguous_refuses_the_note() {
    let refusal = |lines: &[&str]| {
        let pass = renamed(&project(lines));
        assert_eq!(pass.updated, None);
        pass.refusal.expect("refused")
    };
    assert!(refusal(&["## Requirements", "- a", "### Requirements"])
        .contains("it also has `### Requirements` on line 6"),);
    assert!(
        refusal(&["## Requirements", "- a", "## Goals", "### Notes"])
            .contains("it already has `### Notes` on line 7, so renaming ## Requirements would leave two Notes headings"),
    );
    assert!(refusal(&[
        "## Requirements",
        "### Ideas",
        "## Notes",
        "### Ideas"
    ])
    .contains("merging would put two `### Ideas` headings under ## Notes"),);
    // A refused note keeps its task drawers too: no partial note writes.
    let pass = renamed(&project(&[
        "- [ ] #task Ship ^prj",
        "\t- REQUIREMENTS",
        "\t\t- a",
        "## Requirements",
        "# Requirements",
    ]));
    assert!(pass.refusal.is_some());
    assert!(pass.drawers.is_empty());
}

#[test]
fn a_structural_new_section_refuses_project_notes_only() {
    let spec = RenameSpec::parse("later", "events").expect("spec");
    let pass = renamed_with(&project(&["## Later", "- a"]), &spec);
    assert_eq!(
        pass.refusal.as_deref(),
        Some("## Events is a structural section, so ## Later cannot become it")
    );
    let task = note(&["- [ ] #task Ship", "\t- LATER", "\t\t- a"]);
    let pass = renamed_with(&task, &spec);
    assert_eq!(
        pass.updated.as_deref(),
        Some(note(&["- [ ] #task Ship", "\t- EVENTS", "\t\t- a"]).as_str())
    );
}

// ---------------------------------------------------------------------------
// Excluded lookalikes
// ---------------------------------------------------------------------------

#[test]
fn fences_quotes_and_stray_bullets_are_excluded() {
    let contents = note(&[
        "- [ ] #task Ship",
        "\t- [ ] a req",
        "\t\t- REQUIREMENTS",
        "- REQUIREMENTS",
        "> - REQUIREMENTS",
        "```",
        "- REQUIREMENTS",
        "```",
    ]);
    let pass = renamed(&contents);
    assert_eq!(pass.updated, None);
    let reasons: Vec<(usize, &str)> = pass
        .excluded
        .iter()
        .map(|item| (item.line, item.reason))
        .collect();
    assert_eq!(
        reasons,
        [
            (3, "not a direct child of a #task"),
            (4, "not a direct child of a #task"),
            (5, "inside a quote"),
            (7, "inside a code fence"),
        ]
    );
}

#[test]
fn other_heading_levels_in_a_project_note_are_excluded() {
    let pass = renamed(&project(&["## Goals", "### Requirements", "- a"]));
    assert_eq!(pass.updated, None);
    assert_eq!(pass.excluded[0].reason, "not a ## section");
}

#[test]
fn templates_report_headings_and_drawers_outside_fences() {
    let contents = note(&[
        "- [ ] <% tp.file.title %> #task",
        "\t- REQUIREMENTS",
        "## Requirements",
        "```",
        "## Requirements",
        "```",
    ]);
    assert_eq!(
        template_lookalikes(&contents, &spec()),
        [
            (2, "drawer", "- REQUIREMENTS".to_string()),
            (3, "heading", "## Requirements".to_string()),
        ]
    );
}

// ---------------------------------------------------------------------------
// Safety checks
// ---------------------------------------------------------------------------

#[test]
fn content_check_refuses_a_dropped_or_invented_line() {
    let before = note(&["- [ ] #task Ship", "\t- REQUIREMENTS", "\t\t- a"]);
    let pass = rename_note(&before, &spec(), &settings());
    let dropped = note(&["- [ ] #task Ship", "\t- NOTES"]);
    assert!(content_preserved(&before, &dropped, &spec(), &pass)
        .unwrap_err()
        .contains("drop the line \"- a\""));
    let invented = note(&["- [ ] #task Ship", "\t- NOTES", "\t\t- [ ] a"]);
    assert!(content_preserved(&before, &invented, &spec(), &pass).is_err());
    let doubled =
        note(&["- [ ] #task Ship", "\t- NOTES", "\t\t- a", "\t- NOTES"]);
    assert!(content_preserved(&before, &doubled, &spec(), &pass)
        .unwrap_err()
        .contains("drawer headers"));
}
