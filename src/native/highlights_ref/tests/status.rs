//! PDF task status signals and allowance tests.
use super::*;

#[test]
fn highlights_ref_pdf_task_status_signal_contributes_abandoned() {
    let base = test_projection(vec![
        ("status", string_value("wip")),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let mut resolution = super::SyncResolution {
        decision: super::SyncDecision {
            source: super::SyncSource::AutoMerge,
            reason: "marker/frontmatter unchanged".to_string(),
            marker_contributed: false,
            frontmatter_contributed: false,
        },
        projection: base.clone(),
    };
    let task = super::parse_pdf_task_line(
        "- [-] #task [[lib/books/example.pdf]] #hide ^ref\n",
    )
    .expect("parse cancelled task");

    let signal = super::apply_pdf_task_status_signal(
        &mut resolution,
        &task,
        Some(&base),
        &base,
        &base,
    )
    .expect("apply cancelled task signal");

    assert_eq!(signal.status, super::PdfTaskStatus::Abandoned);
    assert_eq!(signal.status_contributed, Some(super::STATUS_ABANDONED));
    assert_eq!(
        resolution
            .projection
            .get(super::FIELD_STATUS)
            .and_then(MarkerValue::as_string),
        Some(super::STATUS_ABANDONED)
    );
    assert!(resolution.decision.frontmatter_contributed);
    assert!(resolution
        .decision
        .reason
        .contains("cancelled PDF task set status abandoned"));
}

fn signal_resolution(projection: &Projection) -> super::SyncResolution {
    super::SyncResolution {
        decision: super::SyncDecision {
            source: super::SyncSource::AutoMerge,
            reason: "marker/frontmatter unchanged".to_string(),
            marker_contributed: false,
            frontmatter_contributed: false,
        },
        projection: projection.clone(),
    }
}

#[test]
fn highlights_ref_pdf_task_status_signal_ready_reopens_terminal_to_ready() {
    for terminal in [super::STATUS_READ, super::STATUS_ABANDONED] {
        let base = test_projection(vec![
            ("status", string_value(terminal)),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let mut resolution = signal_resolution(&base);
        let task = super::parse_pdf_task_line(
            "- [ ] #task [[lib/books/example.pdf]] #hide ^ref\n",
        )
        .expect("parse unchecked task");

        let signal = super::apply_pdf_task_status_signal(
            &mut resolution,
            &task,
            Some(&base),
            &base,
            &base,
        )
        .expect("apply ready task signal");

        assert_eq!(signal.status, super::PdfTaskStatus::Ready);
        assert_eq!(signal.status_contributed, Some(super::STATUS_READY));
        assert_eq!(
            resolution
                .projection
                .get(super::FIELD_STATUS)
                .and_then(MarkerValue::as_string),
            Some(super::STATUS_READY),
            "{terminal} should reopen to ready"
        );
        assert!(resolution.decision.frontmatter_contributed);
        assert!(resolution
            .decision
            .reason
            .contains("ready PDF task set status ready"));
    }
}

#[test]
fn highlights_ref_pdf_task_status_signal_promotes_ready_and_back() {
    for (base_status, mark, target_status) in [
        (super::STATUS_READY, '*', super::STATUS_NEXT),
        (super::STATUS_NEXT, ' ', super::STATUS_READY),
    ] {
        let base = test_projection(vec![
            ("status", string_value(base_status)),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let mut resolution = signal_resolution(&base);
        let task = super::parse_pdf_task_line(&format!(
            "- [{mark}] #task #ref [[lib/books/example.pdf]] #hide ^ref\n"
        ))
        .unwrap_or_else(|error| panic!("parse [{mark}] task: {error}"));

        let signal = super::apply_pdf_task_status_signal(
            &mut resolution,
            &task,
            Some(&base),
            &base,
            &base,
        )
        .unwrap_or_else(|error| {
            panic!("apply {base_status}->{target_status}: {error}")
        });

        assert_eq!(signal.status_contributed, Some(target_status));
        assert!(resolution.decision.frontmatter_contributed);
        assert_eq!(
            resolution
                .projection
                .get(super::FIELD_STATUS)
                .and_then(MarkerValue::as_string),
            Some(target_status)
        );
    }
}

#[test]
fn highlights_ref_pdf_task_status_signal_maps_all_lifecycle_states() {
    for (mark, expected_status, target) in [
        (' ', super::PdfTaskStatus::Ready, super::STATUS_READY),
        ('*', super::PdfTaskStatus::Next, super::STATUS_NEXT),
        ('/', super::PdfTaskStatus::Wip, super::STATUS_WIP),
        ('x', super::PdfTaskStatus::Read, super::STATUS_READ),
        (
            '-',
            super::PdfTaskStatus::Abandoned,
            super::STATUS_ABANDONED,
        ),
    ] {
        let base = test_projection(vec![
            ("status", string_value(super::STATUS_LEGACY)),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let mut resolution = signal_resolution(&base);
        let task = super::parse_pdf_task_line(&format!(
            "- [{mark}] #task [[lib/books/example.pdf]] #hide ^ref\n"
        ))
        .unwrap_or_else(|error| panic!("parse [{mark}] task: {error}"));

        let signal = super::apply_pdf_task_status_signal(
            &mut resolution,
            &task,
            Some(&base),
            &base,
            &base,
        )
        .unwrap_or_else(|error| panic!("apply [{mark}] task signal: {error}"));

        assert_eq!(signal.status, expected_status);
        assert_eq!(signal.status_contributed, Some(target));
        assert_eq!(
            resolution
                .projection
                .get(super::FIELD_STATUS)
                .and_then(MarkerValue::as_string),
            Some(target),
            "[{mark}] should target {target}"
        );
        assert!(resolution.decision.frontmatter_contributed);

        let projection = test_projection(vec![
            ("status", string_value(target)),
            ("parent", string_value("[[obsidian]]")),
        ]);
        assert_eq!(
            super::projection_pdf_task_mark(&projection),
            mark,
            "{target} should render [{mark}]"
        );

        let mut matching_resolution = signal_resolution(&projection);
        let matching_signal = super::apply_pdf_task_status_signal(
            &mut matching_resolution,
            &task,
            Some(&projection),
            &projection,
            &projection,
        )
        .unwrap_or_else(|error| {
            panic!("apply matching [{mark}] task signal: {error}")
        });
        assert_eq!(matching_signal.status_contributed, None);
        assert!(!matching_resolution.decision.frontmatter_contributed);
    }

    // A missing ^ref task contributes nothing.
    let base = test_projection(vec![
        ("status", string_value(super::STATUS_READ)),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let mut resolution = signal_resolution(&base);
    let signal = super::apply_pdf_task_status_signal(
        &mut resolution,
        &super::PdfTaskLineState::Missing,
        Some(&base),
        &base,
        &base,
    )
    .expect("apply missing task signal");

    assert_eq!(signal.status, super::PdfTaskStatus::Missing);
    assert_eq!(signal.status_contributed, None);
    assert_eq!(
        resolution
            .projection
            .get(super::FIELD_STATUS)
            .and_then(MarkerValue::as_string),
        Some(super::STATUS_READ)
    );
    assert!(!resolution.decision.frontmatter_contributed);
}

#[test]
fn highlights_ref_pdf_task_status_signal_conflicts_with_competing_edit() {
    let base = test_projection(vec![
        ("status", string_value(super::STATUS_READ)),
        ("parent", string_value("[[obsidian]]")),
    ]);
    // The marker is unchanged from the stored base, but the frontmatter was
    // edited to a different terminal status. Moving the ^ref task to Ready
    // selects a third value, so the command must report the divergence.
    let marker = base.clone();
    let frontmatter = test_projection(vec![
        ("status", string_value(super::STATUS_ABANDONED)),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let mut resolution = super::SyncResolution {
        decision: super::SyncDecision {
            source: super::SyncSource::Frontmatter,
            reason: "frontmatter changed status".to_string(),
            marker_contributed: false,
            frontmatter_contributed: true,
        },
        projection: frontmatter.clone(),
    };
    let task = super::parse_pdf_task_line(
        "- [ ] #task [[lib/books/example.pdf]] #hide ^ref\n",
    )
    .expect("parse unchecked task");

    let error = super::apply_pdf_task_status_signal(
        &mut resolution,
        &task,
        Some(&base),
        &marker,
        &frontmatter,
    )
    .expect_err("competing status edit should conflict");

    let message = error.to_string();
    assert!(
        message.contains("conflicts with marker/frontmatter status edit"),
        "unexpected conflict message: {message}"
    );
    assert!(
        message.contains("change the PDF task")
            && message.contains("status to ready"),
        "unexpected conflict resolution hint: {message}"
    );
}

#[test]
fn highlights_ref_task_checkbox_rewrite_and_dirty_allowance_are_narrow() {
    let read_projection = test_projection(vec![
        ("status", string_value("read")),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let abandoned_projection = test_projection(vec![
        ("status", string_value("abandoned")),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let wip_projection = test_projection(vec![
        ("status", string_value("wip")),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let next_projection = test_projection(vec![
        ("status", string_value("next")),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let base_body = "\
# Example

- [ ] #task [[lib/example.pdf]] ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
    let checked_body = base_body.replace("- [ ]", "- [X]");
    let next_body = base_body.replace("- [ ]", "- [*]");
    let in_progress_body = base_body.replace("- [ ]", "- [/]");
    assert!(super::bodies_differ_only_by_pdf_task_checkbox(
        base_body,
        &checked_body
    ));
    assert!(super::bodies_differ_only_by_pdf_task_checkbox(
        base_body, &next_body
    ));
    assert!(super::bodies_differ_only_by_pdf_task_checkbox(
        base_body,
        &in_progress_body
    ));
    let cancelled_body_from_base = base_body.replace("- [ ]", "- [-]");
    assert!(super::bodies_differ_only_by_pdf_task_checkbox(
        base_body,
        &cancelled_body_from_base
    ));
    assert!(super::bodies_differ_only_by_pdf_task_checkbox(
        &checked_body,
        &cancelled_body_from_base
    ));
    assert!(super::bodies_differ_only_by_pdf_task_checkbox(
        &cancelled_body_from_base,
        base_body
    ));

    let rewritten = super::rewrite_pdf_task_checkbox_for_projection(
        base_body,
        &read_projection,
    )
    .expect("rewrite task checkbox");
    // Sync itself closes the mark, so it stamps a completion date before
    // the ^ref token.
    let rewritten_task = rewritten
        .lines()
        .find(|line| line.contains(PDF_TASK_BLOCK_ID))
        .expect("rewritten task line");
    assert!(
        rewritten_task
            .starts_with("- [x] #task [[lib/example.pdf]] [completion:: "),
        "{rewritten_task}"
    );
    assert!(rewritten_task.ends_with("] ^ref"), "{rewritten_task}");

    let hidden_body = "\
# Example

- [ ] #task [[lib/example.pdf]] #hide ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
    let hidden_rewritten = super::rewrite_pdf_task_checkbox_for_projection(
        hidden_body,
        &read_projection,
    )
    .expect("rewrite hidden task checkbox");
    let hidden_task = hidden_rewritten
        .lines()
        .find(|line| line.contains(PDF_TASK_BLOCK_ID))
        .expect("hidden task line");
    assert!(
        hidden_task.starts_with(
            "- [x] #task [[lib/example.pdf]] #hide [completion:: "
        ),
        "{hidden_task}"
    );
    assert!(hidden_task.ends_with("] ^ref"), "{hidden_task}");

    let cancelled_body = "\
# Example

- [-] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] [completion:: 2026-06-05] ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
    let cancelled_checked = super::rewrite_pdf_task_checkbox_for_projection(
        cancelled_body,
        &read_projection,
    )
    .expect("rewrite cancelled task checkbox to checked");
    assert!(cancelled_checked.contains(
        "- [x] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] [completion:: 2026-06-05] ^ref\n"
    ));
    let cancelled_unchecked = super::rewrite_pdf_task_checkbox_for_projection(
        cancelled_body,
        &wip_projection,
    )
    .expect("rewrite cancelled task checkbox to in-progress");
    assert!(cancelled_unchecked.contains(
        "- [/] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] [completion:: 2026-06-05] ^ref\n"
    ));
    let cancelled_next = super::rewrite_pdf_task_checkbox_for_projection(
        cancelled_body,
        &next_projection,
    )
    .expect("rewrite cancelled task checkbox to next");
    assert!(cancelled_next.contains(
        "- [*] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] [completion:: 2026-06-05] ^ref\n"
    ));
    let unchecked_cancelled = super::rewrite_pdf_task_checkbox_for_projection(
        hidden_body,
        &abandoned_projection,
    )
    .expect("rewrite unchecked task checkbox to cancelled");
    let unchecked_cancelled_task = unchecked_cancelled
        .lines()
        .find(|line| line.contains(PDF_TASK_BLOCK_ID))
        .expect("cancelled task line");
    assert!(
        unchecked_cancelled_task
            .starts_with("- [-] #task [[lib/example.pdf]] #hide [cancelled:: "),
        "{unchecked_cancelled_task}"
    );
    assert!(
        unchecked_cancelled_task.ends_with("] ^ref"),
        "{unchecked_cancelled_task}"
    );

    let unrelated_body = checked_body.replace("## Highlights", "Manual");
    assert!(!super::bodies_differ_only_by_pdf_task_checkbox(
        base_body,
        &unrelated_body
    ));

    let base_note = format!("---\nstatus: wip\n---\n{base_body}");
    let current_note = format!("---\nstatus: read\n---\n{checked_body}");
    assert!(super::changes_confined_to_frontmatter_or_pdf_task_checkbox(
        &base_note,
        &current_note
    ));
}

#[test]
fn close_date_stamp_inserts_once_before_ref_and_preserves_existing_dates() {
    let open = "- [/] #task #ref [[lib/example.pdf]] #hide ^ref";
    // `stamp_close_date` runs on the already-rewritten line, so hand it the
    // closed mark exactly as `replace_pdf_task_checkbox_mark` does.
    let stamped = super::stamp_close_date(&open.replace("- [/]", "- [x]"), 'x');
    assert!(
        stamped.starts_with(
            "- [x] #task #ref [[lib/example.pdf]] #hide [completion:: "
        ),
        "{stamped}"
    );
    assert!(stamped.ends_with("] ^ref"), "{stamped}");

    // Stamping is idempotent: an existing bracket or emoji date is kept.
    assert_eq!(super::stamp_close_date(&stamped, 'x'), stamped);
    let emoji = "- [x] #task #ref [[lib/example.pdf]] #hide ✅ 2026-01-02 ^ref";
    assert_eq!(super::stamp_close_date(emoji, 'x'), emoji);

    // `-` stamps a cancellation date instead.
    let cancelled =
        super::stamp_close_date(&open.replace("- [/]", "- [-]"), '-');
    assert!(
        cancelled.starts_with(
            "- [-] #task #ref [[lib/example.pdf]] #hide [cancelled:: "
        ),
        "{cancelled}"
    );
    assert!(cancelled.ends_with("] ^ref"), "{cancelled}");

    // Non-closing marks never stamp, and a reopen never removes a date.
    assert_eq!(super::stamp_close_date(open, ' '), open);
    assert_eq!(super::stamp_close_date(open, '/'), open);
    assert_eq!(super::stamp_close_date(&stamped, ' '), stamped);
}

#[test]
fn blocked_task_agrees_with_every_open_projection() {
    for open in [
        super::STATUS_READY,
        super::STATUS_NEXT,
        super::STATUS_WIP,
        super::STATUS_LEGACY,
    ] {
        let base = test_projection(vec![
            ("status", string_value(open)),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let mut resolution = signal_resolution(&base);
        let task = super::parse_pdf_task_line(
            "- [?] #task #ref [[lib/books/example.pdf]] [dependsOn:: a] #hide ^ref\n",
        )
        .expect("parse blocked task");
        assert_eq!(task.status(), super::PdfTaskStatus::Blocked);

        let signal = super::apply_pdf_task_status_signal(
            &mut resolution,
            &task,
            Some(&base),
            &base,
            &base,
        )
        .expect("blocked agrees with open status");
        assert_eq!(signal.status, super::PdfTaskStatus::Blocked);
        assert_eq!(signal.status_contributed, None);
        assert_eq!(
            resolution
                .projection
                .get(super::FIELD_STATUS)
                .and_then(MarkerValue::as_string),
            Some(open)
        );
        assert!(!resolution.decision.frontmatter_contributed);
    }
}

#[test]
fn blocked_task_reopens_terminal_projections_to_ready() {
    for terminal in [super::STATUS_READ, super::STATUS_ABANDONED] {
        let base = test_projection(vec![
            ("status", string_value(terminal)),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let mut resolution = signal_resolution(&base);
        let task = super::parse_pdf_task_line(
            "- [?] #task #ref [[lib/books/example.pdf]] #hide ^ref\n",
        )
        .expect("parse blocked task");

        let signal = super::apply_pdf_task_status_signal(
            &mut resolution,
            &task,
            Some(&base),
            &base,
            &base,
        )
        .expect("blocked reopens terminal status");
        assert_eq!(signal.status_contributed, Some(super::STATUS_READY));
        assert_eq!(
            resolution
                .projection
                .get(super::FIELD_STATUS)
                .and_then(MarkerValue::as_string),
            Some(super::STATUS_READY)
        );
        assert!(resolution.decision.frontmatter_contributed);
        assert!(resolution
            .decision
            .reason
            .contains("blocked PDF task reopened status ready"));
    }
}

#[test]
fn blocked_task_conflicts_when_marker_moved_status_away() {
    let base = test_projection(vec![
        ("status", string_value(super::STATUS_NEXT)),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let marker = test_projection(vec![
        ("status", string_value(super::STATUS_READ)),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let mut resolution = signal_resolution(&marker);
    let task = super::parse_pdf_task_line(
        "- [?] #task #ref [[lib/books/example.pdf]] #hide ^ref\n",
    )
    .expect("parse blocked task");

    let error = super::apply_pdf_task_status_signal(
        &mut resolution,
        &task,
        Some(&base),
        &marker,
        &base,
    )
    .expect_err("marker moving next->read must conflict with blocked");
    let message = error.to_string();
    assert!(
        message.starts_with("blocked PDF task conflicts"),
        "unexpected conflict message: {message}"
    );
}

#[test]
fn blocked_task_write_back_keeps_mark_for_open_and_closes_for_read() {
    let blocked_body = "\
# Example

- [?] #task [[lib/example.pdf]] #hide ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
    for open in ["ready", "next", "wip"] {
        let projection = test_projection(vec![
            ("status", string_value(open)),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let rewritten = super::rewrite_pdf_task_checkbox_for_projection(
            blocked_body,
            &projection,
        )
        .expect("blocked write-back keeps mark");
        assert!(rewritten.contains("- [?] #task"), "{open}: {rewritten}");
    }
    let read_projection = test_projection(vec![
        ("status", string_value("read")),
        ("parent", string_value("[[obsidian]]")),
    ]);
    let rewritten = super::rewrite_pdf_task_checkbox_for_projection(
        blocked_body,
        &read_projection,
    )
    .expect("blocked write-back closes for read");
    assert!(rewritten.contains("- [x] #task"), "{rewritten}");
    assert!(rewritten.contains("[completion:: "), "{rewritten}");

    let ready_body = blocked_body.replace("- [?]", "- [*]");
    assert!(super::bodies_differ_only_by_pdf_task_checkbox(
        &ready_body,
        blocked_body
    ));
    assert!(super::bodies_differ_only_by_pdf_task_checkbox(
        blocked_body,
        &blocked_body.replace("- [?]", "- [ ]")
    ));
}
