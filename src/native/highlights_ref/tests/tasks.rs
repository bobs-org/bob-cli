//! PDF task lines, annotation tasks, and insertion tests.
use super::*;

#[test]
fn highlights_ref_task_line_parser_recognizes_generated_pdf_task() {
    assert_eq!(
        super::parse_pdf_task_line("Body without generated task.\n")
            .expect("parse missing task"),
        super::PdfTaskLineState::Missing
    );

    for (mark, expected_status) in [
        (' ', super::PdfTaskStatus::Ready),
        ('*', super::PdfTaskStatus::Next),
        ('/', super::PdfTaskStatus::Wip),
        ('x', super::PdfTaskStatus::Read),
        ('X', super::PdfTaskStatus::Read),
        ('-', super::PdfTaskStatus::Abandoned),
    ] {
        let parsed = super::parse_pdf_task_line(&format!(
            "# Example\n\n- [{mark}] #task [[lib/books/example.pdf]] #hide ^ref\n"
        ))
        .unwrap_or_else(|error| panic!("parse [{mark}] task: {error}"));
        match parsed {
            super::PdfTaskLineState::Present(task) => {
                assert_eq!(task.line_index, 2);
                assert_eq!(task.mark, mark);
                assert_eq!(task.checked, matches!(mark, 'x' | 'X'));
                assert_eq!(task.status(), expected_status);
            }
            super::PdfTaskLineState::Missing => {
                panic!("expected [{mark}] task line")
            }
        }
    }

    let checked = super::parse_pdf_task_line(
        "- [X] #task [[lib/books/example.PDF|Example]] #hide ^ref\n",
    )
    .expect("parse checked task");
    match checked {
        super::PdfTaskLineState::Present(task) => {
            assert!(task.checked);
            assert_eq!(task.status(), super::PdfTaskStatus::Read);
        }
        super::PdfTaskLineState::Missing => panic!("expected task line"),
    }

    let cancelled = super::parse_pdf_task_line(
        "- [-] #task [[lib/chat/bulk_obsidian_task_properties.pdf]] #hide [cancelled:: 2026-06-04] ^ref\n",
    )
    .expect("parse cancelled task");
    match cancelled {
        super::PdfTaskLineState::Present(task) => {
            assert_eq!(task.mark, '-');
            assert!(!task.checked);
            assert_eq!(task.status(), super::PdfTaskStatus::Abandoned);
        }
        super::PdfTaskLineState::Missing => panic!("expected task line"),
    }

    let legacy_with_priority = super::parse_pdf_task_line(
        "- [ ] #task [[lib/books/example.pdf]] [p::2] ^ref\n",
    )
    .expect("parse legacy task with [p::2]");
    match legacy_with_priority {
        super::PdfTaskLineState::Present(task) => assert!(!task.checked),
        super::PdfTaskLineState::Missing => panic!("expected task line"),
    }

    let legacy_without_marker = super::parse_pdf_task_line(
        "- [ ] #task [[lib/books/example.pdf]] ^ref\n",
    )
    .expect("parse legacy task without hide tag");
    match legacy_without_marker {
        super::PdfTaskLineState::Present(task) => assert!(!task.checked),
        super::PdfTaskLineState::Missing => panic!("expected task line"),
    }

    let typed = super::parse_pdf_task_line(
        "- [ ] #task #ref [[lib/books/example.pdf]] #hide ^ref\n",
    )
    .expect("parse typed task with #ref");
    match typed {
        super::PdfTaskLineState::Present(task) => {
            assert!(!task.checked);
            assert_eq!(task.status(), super::PdfTaskStatus::Ready);
        }
        super::PdfTaskLineState::Missing => panic!("expected task line"),
    }
}

#[test]
fn highlights_ref_task_line_parser_rejects_malformed_and_duplicate_tasks() {
    let missing_tag =
        super::parse_pdf_task_line("- [ ] [[lib/books/example.pdf]] ^ref\n")
            .expect_err("task without tag should fail");
    assert!(
        missing_tag.to_string().contains("malformed"),
        "{missing_tag}"
    );
    for mark in ["[ ]", "[*]", "[/]", "[x]", "[X]", "[-]"] {
        assert!(missing_tag.to_string().contains(mark), "{missing_tag}");
    }

    let non_pdf =
        super::parse_pdf_task_line("- [ ] #task [[ref/example.md]] ^ref\n")
            .expect_err("task without PDF link should fail");
    assert!(non_pdf.to_string().contains("malformed"), "{non_pdf}");

    let custom_marker = super::parse_pdf_task_line(
        "- [>] #task [[lib/books/example.pdf]] #hide ^ref\n",
    )
    .expect_err("custom task marker should fail");
    assert!(
        custom_marker.to_string().contains("malformed"),
        "{custom_marker}"
    );

    let duplicate = super::parse_pdf_task_line(
        "- [ ] #task [[lib/one.pdf]] ^ref\n- [x] #task [[lib/two.pdf]] ^ref\n",
    )
    .expect_err("duplicate task block id should fail");
    assert!(
        duplicate
            .to_string()
            .contains("multiple generated PDF task"),
        "{duplicate}"
    );
}

#[test]
fn annotation_task_candidates_extract_from_comments_and_notes() {
    let annotations = parse_sidecar_markdown(
        "\
## Page 2

- status: wip
- parent: obsidian

---

> Quote with a task comment.

- #task Review the contradiction.
- Ordinary comment bullet.

---

Note:
- #task Follow up on the standalone note.
- [x] #task Preserve accepted source checkboxes.
- Untagged standalone bullet.
",
    );
    let sidecar = super::SidecarInput {
        path: PathBuf::from("example.md"),
        annotations,
    };
    let config = test_config();
    let pdf = Path::new("/tmp/bob/lib/example.pdf");
    let ref_note = Path::new("/tmp/bob/ref/example.md");

    let candidates = super::annotation_task_candidates(
        &config,
        ref_note,
        pdf,
        Some(&sidecar),
        None,
    )
    .expect("extract annotation task candidates");

    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.task_text.as_str())
            .collect::<Vec<_>>(),
        vec![
            "#task Review the contradiction.",
            "#task Follow up on the standalone note.",
            "#task Preserve accepted source checkboxes.",
        ]
    );
    assert_eq!(
        sidecar.annotations[1].comment.as_deref(),
        Some("#task Review the contradiction.\nOrdinary comment bullet.")
    );

    // The comment task points at the highlight's block; both standalone
    // bullets share the standalone note's block; the two ids differ.
    let highlight_block_id =
        super::annotation_block_id(&config, pdf, &sidecar.annotations[1]);
    let note_block_id =
        super::annotation_block_id(&config, pdf, &sidecar.annotations[2]);
    assert_eq!(candidates[0].source_block_id, highlight_block_id);
    assert_eq!(candidates[1].source_block_id, note_block_id);
    assert_eq!(candidates[2].source_block_id, note_block_id);
    assert_ne!(highlight_block_id, note_block_id);
}

#[test]
fn annotation_task_route_suffix_is_strict_and_stripped_from_identity() {
    assert_eq!(
        super::split_annotation_task_route_suffix("#task Follow up @alice"),
        ("#task Follow up".to_string(), Some("alice".to_string()))
    );
    assert_eq!(
        super::split_annotation_task_route_suffix("#task Follow @a_b-2"),
        ("#task Follow".to_string(), Some("a_b-2".to_string()))
    );

    for text in [
        "#task Keep @alice.",
        "#task Keep @alice/bob",
        "#task Keep @alice.md",
        "#task Keep @",
        "#task Keep @-alice",
        "#task Keep @..",
        "#task@alice",
    ] {
        assert_eq!(
            super::split_annotation_task_route_suffix(text),
            (text.to_string(), None),
            "{text}"
        );
    }

    assert_eq!(
        super::annotation_task_identity(
            "#task Follow up @alice [created::2026-06-07]"
        )
        .as_deref(),
        Some("#task Follow up")
    );
}

#[test]
fn annotation_task_candidate_records_route_and_processed_id() {
    let bob_dir = temp_bob_dir("candidate-route");
    write_test_file(
        &bob_dir.join("alice.md"),
        "---\nparent: \"[[people]]\"\n---\n",
    );
    let config = test_config_for_bob_dir(bob_dir.clone());
    let ref_note = bob_dir.join("ref/books/task-notes.md");
    let candidates = super::annotation_task_candidates_from_text(
        &config,
        &ref_note,
        "\
- #task Follow up with Alice @alice
- #task Keep unsafe token @alice.md
",
        "h-abc123",
    )
    .expect("extract routed task candidates");

    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].task_text, "#task Follow up with Alice");
    assert_eq!(candidates[0].identity, "#task Follow up with Alice");
    assert_eq!(
        candidates[0].target,
        super::AnnotationTaskTarget::RoutedNote(bob_dir.join("alice.md"))
    );
    assert_eq!(
        candidates[0].processed_id,
        super::annotation_task_processed_id(
            &config,
            &ref_note,
            "h-abc123",
            "#task Follow up with Alice",
        )
    );
    let rendered = super::render_annotation_task_line(
        &config,
        &candidates[0],
        "2026-06-07",
    );
    assert!(
        rendered.contains("[[ref/books/task-notes#^h-abc123|"),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("[h:: {}]", candidates[0].processed_id)),
        "new tasks must render short processed marker: {rendered}"
    );
    assert!(
        !rendered.contains("[highlight_task:: "),
        "new tasks must not render legacy processed marker: {rendered}"
    );
    assert!(
        rendered.contains("[created::2026-06-07]"),
        "created date missing: {rendered}"
    );

    assert_eq!(
        candidates[1].target,
        super::AnnotationTaskTarget::ReferenceNote
    );
    assert_eq!(candidates[1].task_text, "#task Keep unsafe token @alice.md");
    let same_note_rendered = super::render_annotation_task_line(
        &config,
        &candidates[1],
        "2026-06-07",
    );
    assert!(
        same_note_rendered.contains("[[#^h-abc123|"),
        "{same_note_rendered}"
    );
}

#[test]
fn processed_task_index_scans_states_indents_and_done_notes() {
    let bob_dir = temp_bob_dir("processed-index");
    write_test_file(
        &bob_dir.join("root.md"),
        "\
- [ ] #task Active [[ref/books/task#^ht-active|x]] [highlight_task:: active-id]
  - [x] #task Nested child [[ref/books/task#^ht-nested|x]] [h:: nested-id]
- [-] #task Canceled [[ref/books/task#^ht-cancel|x]] @alice [cancelled::2026-06-08]
",
    );
    write_test_file(
        &bob_dir.join("done/alice_done.md"),
        "- [x] #task Archived [[ref/books/task#^h-done|x]] [h:: done-id] [created::2026-06-07]\n",
    );
    write_test_file(
        &bob_dir.join(".git/ignored.md"),
        "- [x] #task Ignored [[ref/books/task#^ht-ignored|x]] [highlight_task:: ignored-id]\n",
    );
    let config = test_config_for_bob_dir(bob_dir);

    let index =
        super::processed_task_index(&config).expect("build processed index");

    assert!(index.legacy_source_task_anchors.contains("ht-active"));
    assert!(index.legacy_source_task_anchors.contains("ht-nested"));
    assert!(index.legacy_source_task_anchors.contains("ht-cancel"));
    assert!(!index.legacy_source_task_anchors.contains("h-done"));
    assert!(!index.legacy_source_task_anchors.contains("ht-ignored"));
    assert!(index.processed_ids.contains("active-id"));
    assert!(index.processed_ids.contains("nested-id"));
    assert!(index.processed_ids.contains("done-id"));
    assert!(!index.processed_ids.contains("ignored-id"));
    assert!(index.legacy_identities.contains("#task Active"));
    assert!(index.legacy_identities.contains("#task Nested child"));
    assert!(index.legacy_identities.contains("#task Archived"));
    assert!(index.legacy_identities.contains("#task Canceled"));
}

#[test]
fn processed_task_index_legacy_identity_blocks_recreation() {
    let bob_dir = temp_bob_dir("legacy-index");
    write_test_file(
        &bob_dir.join("done/old.md"),
        "- [x] #task Existing follow-up [[ref/books/task#^h-old|x]] [created::2026-06-01]\n",
    );
    let config = test_config_for_bob_dir(bob_dir.clone());
    let mut index =
        super::processed_task_index(&config).expect("build processed index");
    let ref_note = bob_dir.join("ref/books/task.md");
    let candidate = super::annotation_task_candidates_from_text(
        &config,
        &ref_note,
        "- #task Existing follow-up\n",
        "h-new",
    )
    .expect("extract candidate")
    .pop()
    .expect("candidate");

    assert!(!index.accept(&candidate));
}

#[test]
fn processed_task_index_legacy_ht_backlink_blocks_edited_recreation() {
    let bob_dir = temp_bob_dir("legacy-ht-index");
    let config = test_config_for_bob_dir(bob_dir.clone());
    let ref_note = bob_dir.join("ref/books/task.md");
    let candidate = super::annotation_task_candidates_from_text(
        &config,
        &ref_note,
        "- #task Original follow-up\n",
        "h-source123456",
    )
    .expect("extract candidate")
    .pop()
    .expect("candidate");
    let legacy_anchor =
        super::annotation_task_legacy_source_task_block_id(&candidate);
    write_test_file(
        &bob_dir.join("done/old.md"),
        &format!(
            "- [x] #task Edited archived follow-up [[ref/books/task#^{legacy_anchor}|x]] [created::2026-06-01]\n"
        ),
    );

    let mut index =
        super::processed_task_index(&config).expect("build processed index");

    assert!(index.legacy_source_task_anchors.contains(&legacy_anchor));
    assert!(!index.processed_ids.contains(&candidate.processed_id));
    assert!(!index.legacy_identities.contains(&candidate.identity));
    assert!(!index.accept(&candidate));
}

#[test]
fn annotation_task_insertion_is_idempotent_and_preserves_existing_states() {
    let body = "\
# Example

- [ ] #task [[lib/example.pdf]] #hide ^ref
- [x] #task Existing done [created::2026-06-01] [completion::2026-06-02]
- [-] #task Existing cancelled [created::2026-06-01] [cancelled::2026-06-02] [due::2026-06-03]

## Manual Notes

Keep me here.

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
    let block_id = "h-abc123def456";
    let alias = super::SOURCE_LINK_ALIAS;
    let config = test_config();
    let ref_note = Path::new("/tmp/bob/ref/example.md");
    let candidates = super::annotation_task_candidates_from_text(
        &config,
        ref_note,
        "\
- #task Existing done
- #task Existing cancelled
- #task New follow-up [due::2026-06-08]
",
        block_id,
    )
    .expect("extract annotation task candidates");

    let updated = super::insert_missing_annotation_tasks(
        &config,
        body,
        &candidates,
        "2026-06-07",
    )
    .expect("insert annotation tasks");

    // The created task carries a same-file source backlink followed by the
    // durable processed marker and created date. Legacy tasks that already
    // sat under ^ref become the Tasks section's existing content; the new
    // task is appended below them.
    let new_line = format!(
        "- [ ] #task New follow-up [due::2026-06-08] [[#^{block_id}|{alias}]] [h:: {}] [created::2026-06-07]",
        candidates[2].processed_id
    );
    assert!(
        updated.contains(&format!(
            "- [ ] #task [[lib/example.pdf]] #hide ^ref\n\n## Tasks\n- [x] #task Existing done [created::2026-06-01] [completion::2026-06-02]\n- [-] #task Existing cancelled [created::2026-06-01] [cancelled::2026-06-02] [due::2026-06-03]\n{new_line}\n"
        )),
        "{updated}"
    );
    assert_eq!(updated.matches("## Tasks").count(), 1, "{updated}");
    assert!(
        !updated.contains("[highlight_task:: "),
        "new tasks must not render legacy processed markers:\n{updated}"
    );
    assert!(
        updated.contains(&format!("[h:: {}]", candidates[2].processed_id)),
        "new tasks must render short processed markers:\n{updated}"
    );
    // The pre-existing link-less tasks are preserved, not recreated.
    assert_eq!(updated.matches("#task Existing done").count(), 1);
    assert_eq!(updated.matches("#task Existing cancelled").count(), 1);
    assert!(updated.contains("## Manual Notes\n\nKeep me here."));
    assert!(updated.contains("## Highlights\n\n<!-- highlights:begin -->"));

    let rerun = super::insert_missing_annotation_tasks(
        &config,
        &updated,
        &candidates,
        "2026-06-07",
    )
    .expect("rerun annotation task insertion");
    assert_eq!(rerun, updated);

    // A completed linked task keeps its link and is not recreated: identity
    // strips the injected block link on the existing-line side.
    let completed_linked = updated.replace(
        &new_line,
        &format!(
            "{} [completion::2026-06-09]",
            new_line.replacen("- [ ]", "- [x]", 1)
        ),
    );
    let rerun_completed = super::insert_missing_annotation_tasks(
        &config,
        &completed_linked,
        &candidates,
        "2026-06-07",
    )
    .expect("rerun completed linked annotation task insertion");
    assert_eq!(rerun_completed, completed_linked);
}

fn annotation_task_ref_body(between_ref_and_highlights: &str) -> String {
    format!(
        "\
# Example

- [ ] #task [[lib/example.pdf]] #hide ^ref
{between_ref_and_highlights}## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
"
    )
}

fn insert_annotation_tasks(body: &str, lines: &[&str]) -> String {
    super::insert_annotation_task_lines_into_tasks_section(
        body,
        &lines
            .iter()
            .map(|line| (*line).to_string())
            .collect::<Vec<_>>(),
    )
    .expect("insert annotation tasks")
}

#[test]
fn annotation_tasks_append_to_existing_tasks_section() {
    let body = annotation_task_ref_body(
        "
## Tasks

- [ ] #task First
- [ ] #task Second

",
    );
    let updated = insert_annotation_tasks(&body, &["- [ ] #task Third"]);
    assert!(
        updated.contains(
            "## Tasks\n\n- [ ] #task First\n- [ ] #task Second\n- [ ] #task Third\n\n## Highlights"
        ),
        "{updated}"
    );
    assert_eq!(updated.matches("## Tasks").count(), 1, "{updated}");
}

#[test]
fn annotation_tasks_reuse_h1_or_closed_atx_tasks_heading() {
    let h1_body = annotation_task_ref_body(
        "
# Tasks

- [ ] #task First

",
    );
    let h1_updated = insert_annotation_tasks(&h1_body, &["- [ ] #task Second"]);
    assert!(
        h1_updated.contains(
            "# Tasks\n\n- [ ] #task First\n- [ ] #task Second\n\n## Highlights"
        ),
        "{h1_updated}"
    );
    assert!(!h1_updated.contains("## Tasks"), "{h1_updated}");

    let closed_body = annotation_task_ref_body(
        "
## Tasks ##

- [ ] #task First

",
    );
    let closed_updated =
        insert_annotation_tasks(&closed_body, &["- [ ] #task Second"]);
    assert!(
        closed_updated.contains(
            "## Tasks ##\n\n- [ ] #task First\n- [ ] #task Second\n\n## Highlights"
        ),
        "{closed_updated}"
    );
    assert_eq!(
        closed_updated.matches("## Tasks").count(),
        1,
        "{closed_updated}"
    );
}

#[test]
fn annotation_tasks_ignore_fenced_and_managed_tasks_headings() {
    let body = annotation_task_ref_body(
        "
```
## Tasks
```

",
    )
    .replace(
        "<!-- highlights:begin -->\n\n<!-- highlights:end -->",
        "<!-- highlights:begin -->\n\n### Tasks\n\n<!-- highlights:end -->",
    );
    let updated = insert_annotation_tasks(&body, &["- [ ] #task New"]);
    assert!(
        updated.contains(
            "- [ ] #task [[lib/example.pdf]] #hide ^ref\n\n## Tasks\n\n- [ ] #task New\n"
        ),
        "{updated}"
    );
    assert!(updated.contains("```\n## Tasks\n```"), "{updated}");
    assert!(
        updated.contains("<!-- highlights:begin -->\n\n### Tasks\n"),
        "{updated}"
    );
    assert_eq!(
        updated.lines().filter(|line| *line == "## Tasks").count(),
        2,
        "{updated}"
    );
}

#[test]
fn annotation_tasks_fill_empty_tasks_section_with_blank_lines() {
    let body = annotation_task_ref_body(
        "
## Tasks

",
    );
    let updated = insert_annotation_tasks(&body, &["- [ ] #task New"]);
    assert!(
        updated.contains("## Tasks\n\n- [ ] #task New\n\n## Highlights"),
        "{updated}"
    );
    assert_eq!(updated.matches("## Tasks").count(), 1, "{updated}");
}

#[test]
fn annotation_task_insertion_preserves_crlf_line_endings() {
    let body = annotation_task_ref_body("\n").replace('\n', "\r\n");
    let updated = insert_annotation_tasks(&body, &["- [ ] #task New"]);
    assert!(
        !updated.replace("\r\n", "").contains('\n'),
        "inserted a bare LF:\n{updated:?}"
    );
    assert!(
        updated.contains(
            "- [ ] #task [[lib/example.pdf]] #hide ^ref\r\n\r\n## Tasks\r\n\r\n- [ ] #task New\r\n"
        ),
        "{updated:?}"
    );
    assert!(
        updated.contains("- [ ] #task New\r\n\r\n## Highlights\r\n"),
        "{updated:?}"
    );
}

#[test]
fn annotation_tasks_create_section_after_unterminated_ref_line() {
    let body = "- [ ] #task [[lib/example.pdf]] #hide ^ref";
    let updated = insert_annotation_tasks(body, &["- [ ] #task New"]);
    assert_eq!(
        updated,
        "- [ ] #task [[lib/example.pdf]] #hide ^ref\n\n## Tasks\n\n- [ ] #task New\n"
    );
}

#[test]
fn annotation_task_batches_append_in_insertion_order() {
    let body = annotation_task_ref_body("\n");
    let first = insert_annotation_tasks(&body, &["- [ ] #task First"]);
    let second = insert_annotation_tasks(&first, &["- [ ] #task Second"]);
    assert!(
        second.contains(
            "## Tasks\n\n- [ ] #task First\n- [ ] #task Second\n\n## Highlights"
        ),
        "{second}"
    );
    assert_eq!(second.matches("## Tasks").count(), 1, "{second}");
    let first_pos = second.find("- [ ] #task First").expect("first task");
    let second_pos = second.find("- [ ] #task Second").expect("second task");
    assert!(first_pos < second_pos, "{second}");
}
