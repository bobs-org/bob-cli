//! Section stubs, diagnostics, and route hints (bob-cli-bd.6).
use super::*;
use crate::native::highlights_ref::sections::*;

fn highlight(
    order: usize,
    page_label: &str,
    text: &str,
    comment: Option<&str>,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::Highlight,
        page_label: Some(page_label.to_string()),
        page_index: None,
        linked_page_style: false,
        text: text.to_string(),
        section_heading: None,
        comment_section_heading: None,
        comment: comment.map(str::to_string),
        task_source: comment.map(str::to_string),
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn sticky(order: usize, page_label: &str, text: &str) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::StandaloneNote,
        page_label: Some(page_label.to_string()),
        page_index: None,
        linked_page_style: false,
        text: text.to_string(),
        section_heading: None,
        comment_section_heading: None,
        comment: None,
        task_source: None,
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn declaration(
    order: usize,
    page_label: &str,
    heading: &str,
    description: &str,
) -> SidecarAnnotation {
    SidecarAnnotation {
        kind: SidecarAnnotationKind::StandaloneNote,
        page_label: Some(page_label.to_string()),
        page_index: None,
        linked_page_style: false,
        text: description.to_string(),
        section_heading: Some(heading.to_string()),
        comment_section_heading: None,
        comment: None,
        task_source: None,
        image: None,
        order,
        ordinal_on_page: order,
    }
}

fn region_note(body: &str) -> ParsedNote {
    ParsedNote {
        frontmatter: Vec::new(),
        body: format!(
            "<!-- highlights:begin -->\n\n{body}\n<!-- highlights:end -->"
        ),
        original: None,
    }
}

fn render_existing(
    bob_dir: &Path,
    annotations: Vec<SidecarAnnotation>,
    existing_body: &str,
) -> RenderedHighlights {
    let config = test_config_for_bob_dir(bob_dir.to_path_buf());
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let ref_note = bob_dir.join("ref/papers/example.md");
    let input = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations,
    };
    let note = region_note(existing_body);
    render_sidecar_highlights(&config, &pdf, &ref_note, &note, &input, false)
        .expect("render sectioned highlights")
}

fn task_candidates(
    bob_dir: &Path,
    annotations: &[SidecarAnnotation],
    rendered: &RenderedHighlights,
) -> Result<Vec<AnnotationTaskCandidate>> {
    let config = test_config_for_bob_dir(bob_dir.to_path_buf());
    let ref_note = bob_dir.join("ref/papers/example.md");
    let pdf = bob_dir.join("lib/papers/example.pdf");
    let sidecar = SidecarInput {
        path: bob_dir.join("lib/papers/example.md"),
        annotations: annotations.to_vec(),
    };
    annotation_task_candidates(
        &config,
        &ref_note,
        &pdf,
        Some(&sidecar),
        Some(rendered),
        false,
    )
}

const OLD_ID: &str = "h-oldabc123";

fn published_sticky_region() -> String {
    format!("### Page 1\n\n> [!note] A keeper\n\n^{OLD_ID}\n")
}

fn memory_declaration() -> Vec<SidecarAnnotation> {
    vec![declaration(1, "Page 1", "Memory", "A keeper")]
}

#[test]
fn declaring_sticky_absorbs_published_note_without_a_stub() {
    let bob_dir = temp_bob_dir("edges-decl-absorb");
    let rendered = render_existing(
        &bob_dir,
        memory_declaration(),
        &published_sticky_region(),
    );
    assert!(rendered.content.contains("### Memory"));
    assert!(rendered.content.contains("^s-memory"));
    assert!(!rendered.content.contains(OLD_ID));
    assert!(!rendered.content.contains("Section declaration"));
    assert!(!rendered.content.contains("Removed highlight"));
}

#[test]
fn declaring_sticky_redirect_stub_appears_when_linked() {
    let bob_dir = temp_bob_dir("edges-decl-linked");
    write_test_file(
        &bob_dir.join("log.md"),
        &format!("Link back [[ref/papers/example#^{OLD_ID}|old]]\n"),
    );
    let rendered = render_existing(
        &bob_dir,
        memory_declaration(),
        &published_sticky_region(),
    );
    assert!(rendered.content.contains("[!info] Section declaration"));
    assert!(rendered
        .content
        .contains("Now the card of [[#^s-memory|Memory]]"));
    assert!(rendered.content.contains(&format!("^{OLD_ID}")));
    assert!(!rendered.content.contains("[!warning] Removed highlight"));
}

#[test]
fn declaring_sticky_redirect_stub_persists_after_unlink() {
    let bob_dir = temp_bob_dir("edges-decl-persist");
    let link = bob_dir.join("log.md");
    write_test_file(
        &link,
        &format!("Link back [[ref/papers/example#^{OLD_ID}|old]]\n"),
    );
    let first = render_existing(
        &bob_dir,
        memory_declaration(),
        &published_sticky_region(),
    );
    assert!(first.content.contains("[!info] Section declaration"));
    std::fs::remove_file(&link).expect("remove vault link");
    let second =
        render_existing(&bob_dir, memory_declaration(), &first.content);
    assert!(
        second.content.contains("[!info] Section declaration"),
        "stub persists once tombstoned: {}",
        second.content
    );
    assert!(second.content.contains(&format!("^{OLD_ID}")));
    assert!(!second.content.contains("[!warning] Removed highlight"));
}

#[test]
fn declaring_sticky_redirect_stub_disappears_without_declaration() {
    let bob_dir = temp_bob_dir("edges-decl-gone");
    write_test_file(
        &bob_dir.join("log.md"),
        &format!("Link back [[ref/papers/example#^{OLD_ID}|old]]\n"),
    );
    let first = render_existing(
        &bob_dir,
        memory_declaration(),
        &published_sticky_region(),
    );
    assert!(first.content.contains("[!info] Section declaration"));
    let plain = vec![sticky(1, "Page 1", "A keeper")];
    let second = render_existing(&bob_dir, plain, &first.content);
    assert!(!second.content.contains("[!info] Section declaration"));
}

#[test]
fn removed_section_needs_a_link_or_prior_stub() {
    let bob_dir = temp_bob_dir("edges-removed-quiet");
    let declared = vec![
        declaration(1, "Page 1", "Memory", "About memory."),
        highlight(2, "Page 4", "The quoted passage", Some("@memory")),
    ];
    let first = render_existing(&bob_dir, declared, "");
    assert!(first.content.contains("^s-memory"));
    let undeclared = vec![highlight(2, "Page 4", "The quoted passage", None)];
    let second = render_existing(&bob_dir, undeclared, &first.content);
    assert!(!second.content.contains("[!info] Removed section"));
    assert!(!second.content.contains("^s-memory"));
    assert!(second.content.contains("### Page 4"));
    assert!(!second.content.contains("@memory"));
}

#[test]
fn removed_section_stub_appears_when_linked_and_persists() {
    let bob_dir = temp_bob_dir("edges-removed-linked");
    let declared = vec![
        declaration(1, "Page 1", "Memory", "About memory."),
        highlight(2, "Page 4", "The quoted passage", Some("@memory")),
    ];
    let first = render_existing(&bob_dir, declared, "");
    assert!(first.content.contains("^s-memory"));
    write_test_file(
        &bob_dir.join("log.md"),
        "See [[ref/papers/example#^s-memory|Memory]]\n",
    );
    let undeclared = vec![highlight(2, "Page 4", "The quoted passage", None)];
    let second = render_existing(&bob_dir, undeclared.clone(), &first.content);
    assert!(second.content.contains("[!info] Removed section"));
    assert!(second
        .content
        .contains("\"Memory\" is no longer declared in the Highlights notes."));
    assert!(second.content.contains("^s-memory"));
    std::fs::remove_file(bob_dir.join("log.md")).expect("remove vault link");
    let third = render_existing(&bob_dir, undeclared, &second.content);
    assert!(
        third.content.contains("[!info] Removed section"),
        "already-stubbed section keeps its stub: {}",
        third.content
    );
}

#[test]
fn removed_section_stub_disappears_when_redeclared() {
    let bob_dir = temp_bob_dir("edges-removed-redeclare");
    let declared = vec![
        declaration(1, "Page 1", "Memory", "About memory."),
        highlight(2, "Page 4", "The quoted passage", Some("@memory")),
    ];
    let first = render_existing(&bob_dir, declared.clone(), "");
    write_test_file(
        &bob_dir.join("log.md"),
        "See [[ref/papers/example#^s-memory|Memory]]\n",
    );
    let undeclared = vec![highlight(2, "Page 4", "The quoted passage", None)];
    let second = render_existing(&bob_dir, undeclared, &first.content);
    assert!(second.content.contains("[!info] Removed section"));
    let third = render_existing(&bob_dir, declared, &second.content);
    assert!(!third.content.contains("[!info] Removed section"));
    assert!(third.content.contains("### Memory"));
    assert!(third.content.contains("^s-memory"));
}

#[test]
fn section_dry_run_lines_cover_the_vocabulary() {
    let bob_dir = temp_bob_dir("edges-diagnostics");
    let annotations = vec![
        declaration(1, "Page 1", "Memory", ""),
        declaration(2, "Page 2", "Visuals", ""),
        highlight(3, "Page 4", "Head", Some("@memry")),
        highlight(4, "Page 7", "Both", Some("@memory @visuals")),
        declaration(5, "Page 9", "Memory", "More about memory."),
        declaration(6, "Page 1", "Other highlights", ""),
        declaration(7, "Page 1", "!!!", ""),
        highlight(8, "Page 3", "Quoted", Some("A note\n\\# Memory")),
        sticky(9, "Page 6", "@memory"),
        declaration(10, "Page 5", "Sase", ""),
        highlight(11, "Page 5", "Task quote", Some("- #task Check @sase")),
    ];
    let rendered = render_existing(&bob_dir, annotations, "");
    let lines = section_report_lines(&rendered.section_diagnostics);
    assert_eq!(
        lines,
        vec![
            "sections: 3 (Memory 1, Visuals 0, Sase 0)".to_string(),
            "section_duplicate: \"Memory\" (Page 9) merged into \"Memory\" (Page 1)".to_string(),
            "section_reserved_title: \"Other highlights\" (Page 1) ignored"
                .to_string(),
            "section_title_empty: Page 1 \"# !!!\" has no letters or digits"
                .to_string(),
            "section_tag_unknown: Page 4 @memry (did you mean @memory?)"
                .to_string(),
            "section_conflict: Page 7 @memory, @visuals (kept @memory)"
                .to_string(),
            "section_declaration_in_comment: Page 3 \"# Memory\" is literal; put \"# Name\" on the first line of a comment or sticky note".to_string(),
            "section_tag_only_note: Page 6 @memory (a note needs text to join a section)".to_string(),
            "section_tag_on_task: Page 5 @sase routes this #task to sase.md; put @sase on its own line to join Sase".to_string(),
        ]
    );
    assert!(section_report_lines(&[]).is_empty());
    let concise = section_concise_details(
        rendered.sections.len(),
        &rendered.section_diagnostics,
    );
    assert_eq!(concise, vec!["3 sections", "1 unknown section tag"]);
    assert!(section_concise_details(0, &[]).is_empty());
    assert_eq!(
        section_concise_details(2, &[]),
        vec!["2 sections".to_string()]
    );
}

#[test]
fn missing_route_target_naming_a_section_gains_a_hint() {
    let bob_dir = temp_bob_dir("edges-route-hint");
    let annotations = vec![highlight(
        1,
        "Page 5",
        "Task quote",
        Some("- #task Draft @memory"),
    )]
    .into_iter()
    .chain([declaration(2, "Page 1", "Memory", "")])
    .collect::<Vec<_>>();
    let rendered = render_existing(&bob_dir, annotations.clone(), "");
    let error = task_candidates(&bob_dir, &annotations, &rendered)
        .expect_err("missing route target fails");
    let message = error.to_string();
    assert!(
        message.contains("routed annotation task target does not exist"),
        "{message}"
    );
    assert!(
        message.contains(
            "; to join section \"Memory\", put @memory on its own line"
        ),
        "{message}"
    );
}

#[test]
fn missing_route_target_without_a_section_keeps_todays_error() {
    let bob_dir = temp_bob_dir("edges-route-plain");
    let annotations = vec![highlight(
        1,
        "Page 5",
        "Task quote",
        Some("- #task Draft @sase"),
    )];
    let rendered = render_existing(&bob_dir, annotations.clone(), "");
    let error = task_candidates(&bob_dir, &annotations, &rendered)
        .expect_err("missing route target fails");
    let message = error.to_string();
    assert!(
        message.contains("routed annotation task target does not exist"),
        "{message}"
    );
    assert!(!message.contains("to join section"), "{message}");
}

#[test]
fn section_tag_and_task_route_share_a_note() {
    let bob_dir = temp_bob_dir("edges-route-precedence");
    write_test_file(&bob_dir.join("sase.md"), "---\nparent: \"[[x]]\"\n---\n");
    let annotations = vec![
        declaration(1, "Page 1", "Memory", ""),
        declaration(2, "Page 1", "Sase", ""),
        highlight(3, "Page 4", "Grouped quote", Some("@memory")),
        highlight(4, "Page 5", "Task quote", Some("- #task Draft @sase")),
    ];
    let rendered = render_existing(&bob_dir, annotations.clone(), "");
    let candidates = task_candidates(&bob_dir, &annotations, &rendered)
        .expect("route plus section both work");
    assert_eq!(candidates.len(), 1);
    assert!(
        matches!(
            &candidates[0].target,
            AnnotationTaskTarget::RoutedNote(path)
                if path.ends_with("sase.md")
        ),
        "{:?}",
        candidates[0].target
    );
    assert!(
        rendered.section_diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == SectionDiagnosticKind::TagOnTask
        }),
        "task-line section tag is reported"
    );
}
