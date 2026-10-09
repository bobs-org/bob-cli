//! Focused v2-planning tests: classification, birth/adoption, status
//! conflicts, parent-free snapshots, embed healing, and the plan composer.
//! Vault-free throughout: the resolver is a stub and located tasks are
//! literals.
use super::*;
use crate::native::capture_targets::CaptureTargetKind;
use crate::native::parent_notes::{
    ParentError, ParentMatchKind, ResolvedParent,
};
use crate::native::ref_tasks::LocatedRefTask;

fn located(
    path: &str,
    mark: char,
    block_id: Option<&str>,
    archived: bool,
    residence: Option<&str>,
) -> LocatedRefTask {
    LocatedRefTask {
        path: path.to_string(),
        line_index: 0,
        line: format!("- [{mark}] #task #ref [[ref/papers/stem|Stem]]"),
        mark,
        block_id: block_id.map(str::to_string),
        archived,
        residence: residence.map(str::to_string),
        closed_on: None,
        in_capture_target: true,
    }
}

fn closed_located(
    path: &str,
    mark: char,
    closed_on: Option<&str>,
) -> LocatedRefTask {
    LocatedRefTask {
        path: path.to_string(),
        line_index: 0,
        line: format!("- [{mark}] #task #ref [[ref/papers/stem|Stem]]"),
        mark,
        block_id: Some("ref-stem".to_string()),
        archived: false,
        residence: Some("sase".to_string()),
        closed_on: closed_on.map(str::to_string),
        in_capture_target: true,
    }
}

type TestResolverResult = std::result::Result<ResolvedParent, ParentError>;

fn stub_resolver(hint: &str) -> TestResolverResult {
    if hint == "sase" {
        Ok(ResolvedParent {
            route: "sase".to_string(),
            kind: CaptureTargetKind::Area,
            label: "sase.md".to_string(),
            matched: ParentMatchKind::Stem,
        })
    } else {
        Err(ParentError::NotParent {
            label: hint.to_string(),
        })
    }
}

fn every_parent_error() -> Vec<ParentError> {
    vec![
        ParentError::Unknown {
            input: "nope".to_string(),
            normalized: "nope".to_string(),
            suggestions: Vec::new(),
        },
        ParentError::Ambiguous {
            alias: "shared".to_string(),
            claimants: Vec::new(),
        },
        ParentError::Terminal {
            route: "done".to_string(),
            status: "done".to_string(),
        },
        ParentError::NotParent {
            label: "obsidian_ref.md".to_string(),
        },
    ]
}

#[test]
fn classify_missing_note_is_v2_birth() {
    assert_eq!(
        super::classify_note_branch(false, "ref/papers/stem.md", "", &[]),
        super::NoteBranch::V2
    );
}

#[test]
fn classify_outside_task_makes_existing_note_v2() {
    let live = located("sase.md", '*', Some("ref-stem"), false, Some("sase"));
    assert_eq!(
        super::classify_note_branch(
            true,
            "ref/papers/stem.md",
            "# Stem\n",
            &[live]
        ),
        super::NoteBranch::V2
    );
}

#[test]
fn classify_archived_task_makes_existing_note_v2() {
    let archived = located(
        "done/sase_done.md",
        'x',
        Some("ref-stem"),
        true,
        Some("sase"),
    );
    assert_eq!(
        super::classify_note_branch(
            true,
            "ref/papers/stem.md",
            "# Stem\n",
            &[archived]
        ),
        super::NoteBranch::V2
    );
}

#[test]
fn classify_managed_embed_makes_existing_note_v2() {
    let body = "# Stem\n\n![[sase#^ref-stem]]\n";
    assert_eq!(
        super::classify_note_branch(true, "ref/papers/stem.md", body, &[]),
        super::NoteBranch::V2
    );
}

#[test]
fn classify_in_note_tracker_stays_v1() {
    let body = "# Stem\n\n- [ ] #task [[lib/papers/stem.pdf]] #hide ^ref\n";
    assert_eq!(
        super::classify_note_branch(true, "ref/papers/stem.md", body, &[]),
        super::NoteBranch::V1
    );
}

#[test]
fn classify_trackerless_legacy_stays_v1() {
    let body = "# Stem\n\nSome authored notes.\n";
    assert_eq!(
        super::classify_note_branch(true, "ref/papers/stem.md", body, &[]),
        super::NoteBranch::V1
    );
}

#[test]
fn classify_task_inside_own_note_stays_v1() {
    let inside =
        located("ref/papers/stem.md", '*', Some("ref-stem"), false, None);
    assert_eq!(
        super::classify_note_branch(
            true,
            "ref/papers/stem.md",
            "# Stem\n",
            &[inside]
        ),
        super::NoteBranch::V1
    );
}

#[test]
fn birth_parent_keeps_resolved_route() {
    let parent = super::resolve_birth_parent("sase", &stub_resolver);
    assert_eq!(parent.route, "sase");
    assert_eq!(parent.label, "sase.md");
    assert_eq!(parent.warning_child, None);
    assert_eq!(parent.hint, "sase");
}

#[test]
fn birth_parent_falls_back_on_every_error_kind() {
    for error in every_parent_error() {
        let resolver = |_: &str| -> TestResolverResult { Err(error.clone()) };
        let parent = super::resolve_birth_parent("nope", &resolver);
        assert_eq!(parent.route, "mac_inbox");
        assert_eq!(parent.label, "mac_inbox.md");
        let warning = parent.warning_child.expect("fallback warns");
        assert!(warning.contains("'nope'"), "hint kept: {warning}");
        assert!(warning.contains("Ctrl+Shift+M"), "refile hint: {warning}");
    }
}

#[test]
fn fallback_child_text_is_exact() {
    assert_eq!(
        super::parent_fallback_child("obsidian_ref"),
        "⚠️ parent 'obsidian_ref' is not an open area or project · refile me with Ctrl+Shift+M"
    );
}

#[test]
fn birth_with_no_candidates_inserts() {
    let (task, diagnostics) = super::select_birth_task(&[]);
    assert_eq!(task, super::BirthTask::Insert);
    assert!(diagnostics.is_empty());
}

#[test]
fn birth_adopts_unique_orphan() {
    let orphan = located("sase.md", '*', Some("ref-stem"), false, Some("sase"));
    let (task, _) = super::select_birth_task(std::slice::from_ref(&orphan));
    assert_eq!(task, super::BirthTask::Adopt(orphan));
}

#[test]
fn birth_adopts_newest_closed() {
    let older = LocatedRefTask {
        closed_on: Some("2026-10-01".to_string()),
        ..closed_located("sase.md", 'x', Some("2026-10-01"))
    };
    let newer = LocatedRefTask {
        closed_on: Some("2026-10-08".to_string()),
        ..closed_located("sase.md", '-', Some("2026-10-08"))
    };
    let (task, _) = super::select_birth_task(&[older, newer.clone()]);
    assert_eq!(task, super::BirthTask::Adopt(newer));
}

#[test]
fn birth_refuses_multiple_open() {
    let first = located("sase.md", '*', Some("ref-stem"), false, Some("sase"));
    let second = located("bob.md", '/', Some("ref-stem"), false, Some("bob"));
    let (task, diagnostics) = super::select_birth_task(&[first, second]);
    assert_eq!(task, super::BirthTask::Refused);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "multiple_open_ref_tasks"),
        "ambiguity diagnostic: {diagnostics:?}"
    );
}

#[test]
fn birth_refuses_open_archive_task() {
    let archived = located(
        "done/sase_done.md",
        '*',
        Some("ref-stem"),
        true,
        Some("sase"),
    );
    let (task, diagnostics) = super::select_birth_task(&[archived]);
    assert_eq!(task, super::BirthTask::Refused);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "open_ref_task_in_done"),
        "archive diagnostic: {diagnostics:?}"
    );
}

#[test]
fn missing_task_open_surfaces_guidance() {
    for status in [Some("next"), Some("legacy"), None] {
        let diagnostic = super::missing_task_diagnostic(status)
            .expect("open v2 note without a task warns");
        assert_eq!(diagnostic.code, "open_ref_without_task");
        assert!(
            diagnostic.detail.contains("git"),
            "restore: {}",
            diagnostic.detail
        );
        assert!(
            diagnostic.detail.contains("abandoned"),
            "abandoned: {}",
            diagnostic.detail
        );
    }
}

#[test]
fn missing_task_terminal_stays_silent() {
    assert_eq!(super::missing_task_diagnostic(Some("read")), None);
    assert_eq!(super::missing_task_diagnostic(Some("abandoned")), None);
}

#[test]
fn status_mark_matches_projection_marks() {
    for (status, mark) in [
        ("ready", ' '),
        ("next", '*'),
        ("wip", '/'),
        ("read", 'x'),
        ("abandoned", '-'),
    ] {
        assert_eq!(super::status_mark(Some(status)), Some(mark));
        let mut projection = test_projection(Vec::new());
        projection.insert("status".to_string(), string_value(status));
        assert_eq!(
            super::projection_pdf_task_mark(&projection),
            mark,
            "seam disagrees for {status}"
        );
    }
    assert_eq!(super::status_mark(Some("legacy")), None);
    assert_eq!(super::status_mark(None), None);
}

fn signal_inputs<'a>(
    mark: char,
    current: Option<&'a str>,
    base: Option<&'a str>,
    marker: Option<&'a str>,
    frontmatter: Option<&'a str>,
    archived: bool,
) -> super::V2TaskSignalInputs<'a> {
    super::V2TaskSignalInputs {
        mark,
        current_status: current,
        base_status: base,
        marker_status: marker,
        frontmatter_status: frontmatter,
        task_archived: archived,
    }
}

#[test]
fn signal_unchanged_checkbox_is_quiet() {
    let signal = super::v2_task_status_signal(signal_inputs(
        '*',
        Some("next"),
        Some("next"),
        Some("next"),
        Some("next"),
        false,
    ))
    .expect("unchanged sync is quiet");
    assert_eq!(signal.target, Some("next"));
    assert!(!signal.task_changed);
    assert_eq!(signal.drive_status, None);
    assert_eq!(signal.checkbox_edit, None);
    assert!(!signal.reopen);
}

#[test]
fn signal_task_gesture_drives_status() {
    let signal = super::v2_task_status_signal(signal_inputs(
        'x',
        Some("next"),
        Some("next"),
        Some("next"),
        Some("next"),
        false,
    ))
    .expect("task close drives");
    assert!(signal.task_changed);
    assert_eq!(signal.drive_status, Some("read"));
    assert_eq!(signal.checkbox_edit, None);
    assert!(!signal.reopen);
}

#[test]
fn signal_marker_only_change_edits_the_line() {
    let signal = super::v2_task_status_signal(signal_inputs(
        '*',
        Some("ready"),
        Some("next"),
        Some("ready"),
        Some("next"),
        false,
    ))
    .expect("marker-only change edits");
    assert!(!signal.task_changed);
    assert_eq!(signal.drive_status, None);
    assert_eq!(signal.checkbox_edit, Some(' '));
    assert!(!signal.reopen);
}

#[test]
fn signal_frontmatter_only_change_edits_the_line() {
    let signal = super::v2_task_status_signal(signal_inputs(
        ' ',
        Some("next"),
        Some("ready"),
        Some("ready"),
        Some("next"),
        false,
    ))
    .expect("frontmatter-only change edits");
    assert_eq!(signal.checkbox_edit, Some('*'));
}

#[test]
fn signal_conflicting_inputs_refuse() {
    let error = super::v2_task_status_signal(signal_inputs(
        'x',
        Some("abandoned"),
        Some("next"),
        Some("abandoned"),
        Some("next"),
        false,
    ))
    .expect_err("task close plus marker cancel conflicts");
    assert!(error.to_string().contains("conflict"), "{error}");
}

#[test]
fn signal_changed_sides_agreeing_with_task_drive() {
    // The marker already moved to the task's status, so the merge reflects
    // it: the gesture is recorded, but nothing is left to drive.
    let signal = super::v2_task_status_signal(signal_inputs(
        'x',
        Some("read"),
        Some("next"),
        Some("read"),
        Some("next"),
        false,
    ))
    .expect("compatible changes agree");
    assert!(signal.task_changed);
    assert_eq!(signal.target, Some("read"));
    assert_eq!(signal.drive_status, None);
    assert_eq!(signal.checkbox_edit, None);
}

#[test]
fn signal_without_base_evidence_refuses_disagreement() {
    let error = super::v2_task_status_signal(signal_inputs(
        '*',
        Some("ready"),
        None,
        Some("ready"),
        Some("ready"),
        false,
    ))
    .expect_err("no base means no blessing");
    assert!(error.to_string().contains("no stored base"), "{error}");
}

#[test]
fn signal_without_base_agreement_is_quiet() {
    let signal = super::v2_task_status_signal(signal_inputs(
        '*',
        Some("next"),
        None,
        Some("next"),
        Some("next"),
        false,
    ))
    .expect("agreement needs no base");
    assert_eq!(signal.drive_status, None);
    assert_eq!(signal.checkbox_edit, None);
}

#[test]
fn signal_question_overlay_agrees_with_open() {
    let signal = super::v2_task_status_signal(signal_inputs(
        '?',
        Some("next"),
        Some("next"),
        Some("next"),
        Some("next"),
        false,
    ))
    .expect("[?] overlay is quiet on agreement");
    assert_eq!(signal.task_status, super::PdfTaskStatus::Blocked);
    assert_eq!(signal.target, Some("next"));
    assert_eq!(signal.checkbox_edit, None);
}

#[test]
fn signal_question_on_terminal_drives_reopen_ready() {
    let signal = super::v2_task_status_signal(signal_inputs(
        '?',
        Some("read"),
        Some("read"),
        Some("read"),
        Some("read"),
        true,
    ))
    .expect("[?] on terminal reopens");
    assert_eq!(signal.drive_status, Some("ready"));
}

#[test]
fn signal_archived_reopen_never_edits_done() {
    let signal = super::v2_task_status_signal(signal_inputs(
        'x',
        Some("next"),
        Some("read"),
        Some("next"),
        Some("next"),
        true,
    ))
    .expect("archived reopen plans fresh");
    assert!(signal.reopen);
    assert_eq!(signal.checkbox_edit, None);
    assert_eq!(signal.drive_status, None);
}

#[test]
fn signal_live_terminal_reopen_edits_the_line() {
    let signal = super::v2_task_status_signal(signal_inputs(
        'x',
        Some("next"),
        Some("read"),
        Some("next"),
        Some("next"),
        false,
    ))
    .expect("live reopen edits");
    assert!(!signal.reopen);
    assert_eq!(signal.checkbox_edit, Some('*'));
}

#[test]
fn signal_marker_close_edits_the_line() {
    let signal = super::v2_task_status_signal(signal_inputs(
        '*',
        Some("read"),
        Some("next"),
        Some("read"),
        Some("next"),
        false,
    ))
    .expect("marker close edits");
    assert_eq!(signal.checkbox_edit, Some('x'));
}

#[test]
fn signal_unknown_mark_is_quiet() {
    let signal = super::v2_task_status_signal(signal_inputs(
        'o',
        Some("next"),
        Some("next"),
        Some("next"),
        Some("next"),
        false,
    ))
    .expect("unknown marks never error");
    assert_eq!(signal.target, None);
    assert_eq!(signal.checkbox_edit, None);
}

#[test]
fn signal_uncheckboxable_current_refuses() {
    let error = super::v2_task_status_signal(signal_inputs(
        '*',
        Some("legacy"),
        Some("next"),
        Some("legacy"),
        Some("next"),
        false,
    ))
    .expect_err("legacy has no checkbox");
    assert!(error.to_string().contains("legacy"), "{error}");
}

#[test]
fn without_parent_drops_only_parent() {
    let projection = test_projection(vec![
        ("status", string_value("next")),
        ("parent", string_value("[[sase]]")),
        ("title", string_value("Stem")),
    ]);
    let parent_free = super::without_parent(&projection);
    assert_eq!(parent_free.get("parent"), None);
    assert_eq!(
        parent_free.get("status").and_then(MarkerValue::as_string),
        Some("next")
    );
    assert!(parent_free.contains_key("title"));
    assert!(projection.contains_key("parent"), "input untouched");
}

#[test]
fn first_v2_transition_has_no_false_conflict() {
    let old_base = test_projection(vec![
        ("status", string_value("next")),
        ("parent", string_value("[[obsidian_ref]]")),
        ("title", string_value("Stem")),
    ]);
    let new_base = test_projection(vec![
        ("status", string_value("next")),
        ("title", string_value("Stem")),
    ]);
    let normalized = super::normalize_v2_base(&old_base);
    assert_eq!(normalized, new_base);
    let merge =
        super::merge_projection_changes(&normalized, &new_base, &new_base);
    assert!(merge.conflicts.is_empty(), "move is not a conflict");
    assert_eq!(
        super::projection_hash(&normalized).expect("hash"),
        super::projection_hash(&new_base).expect("hash"),
        "hashes match after normalization"
    );
}

#[test]
fn residence_parent_renders_from_route() {
    assert_eq!(
        super::residence_parent_value("sase"),
        string_value("[[sase]]")
    );
}

#[test]
fn marker_hint_round_trips_parent() {
    let parent_free = test_projection(vec![("status", string_value("next"))]);
    let marker =
        super::marker_projection_with_parent_hint(&parent_free, "sase");
    assert_eq!(
        marker.get("parent").and_then(MarkerValue::as_string),
        Some("[[sase]]")
    );
    assert_eq!(super::without_parent(&marker), parent_free);
}

#[test]
fn embed_target_forms_follow_the_design() {
    let root = located("sase.md", '*', Some("ref-stem"), false, Some("sase"));
    assert_eq!(super::managed_embed_target(&root), "sase");
    let archived = located(
        "done/sase_done.md",
        'x',
        Some("ref-stem"),
        true,
        Some("sase"),
    );
    assert_eq!(super::managed_embed_target(&archived), "done/sase_done");
    let nested = located("projects/foo.md", '*', Some("ref-x"), false, None);
    assert_eq!(super::managed_embed_target(&nested), "projects/foo");
}

#[test]
fn heal_inserts_missing_embed_below_h1() {
    let healed = super::heal_managed_embed(
        "# Stem\n\nSome notes.\n",
        "sase",
        "ref-stem",
    );
    assert_eq!(healed, "# Stem\n\n![[sase#^ref-stem]]\n\nSome notes.\n");
}

#[test]
fn heal_repoints_stale_embed() {
    let body = "# Stem\n\n![[bob#^ref-old]]\n\nSome notes.\n";
    let healed = super::heal_managed_embed(body, "sase", "ref-stem");
    assert_eq!(healed, "# Stem\n\n![[sase#^ref-stem]]\n\nSome notes.\n");
}

#[test]
fn heal_collapses_duplicate_embeds() {
    let body = "# Stem\n\n![[sase#^ref-stem]]\n\n![[bob#^ref-old]]\n";
    assert_eq!(
        super::heal_managed_embed(body, "sase", "ref-stem"),
        "# Stem\n\n![[sase#^ref-stem]]\n\n"
    );
}

#[test]
fn heal_correct_body_is_byte_identical() {
    let body = "# Stem\n\n![[sase#^ref-stem]]\n\nSome notes.\n";
    assert_eq!(super::heal_managed_embed(body, "sase", "ref-stem"), body);
}

#[test]
fn heal_preserves_authored_material() {
    let body = "# Stem\n\n![[other|x]]\n\n![[lib/papers/stem.mp3]]\n\nProse.\n";
    let healed = super::heal_managed_embed(body, "sase", "ref-stem");
    assert_eq!(
        healed,
        "# Stem\n\n![[sase#^ref-stem]]\n\n![[other|x]]\n\n![[lib/papers/stem.mp3]]\n\nProse.\n"
    );
}

#[test]
fn heal_without_h1_leaves_body_alone() {
    let body = "No heading here.\n\n![[bob#^ref-old]]\n";
    assert_eq!(super::heal_managed_embed(body, "sase", "ref-stem"), body);
}

#[test]
fn heal_preserves_crlf() {
    let body = "# Stem\r\n\r\nSome notes.\r\n";
    assert_eq!(
        super::heal_managed_embed(body, "sase", "ref-stem"),
        "# Stem\r\n\r\n![[sase#^ref-stem]]\r\n\r\nSome notes.\r\n"
    );
}

#[test]
fn birth_body_shows_embed_not_tracker() {
    let body = super::v2_birth_body("Stem Title", "sase", "ref-stem", "", None);
    assert!(body.starts_with("\n# Stem Title\n\n![[sase#^ref-stem]]\n\n"));
    assert!(body.contains("## Highlights\n"));
    assert!(body.contains(super::MANAGED_BODY_BEGIN));
    assert!(body.contains(super::MANAGED_BODY_END));
    assert!(!body.contains("#hide"), "no hide tag");
    assert!(
        !body.split_whitespace().any(|token| token == "^ref"),
        "no in-note tracker"
    );
}

#[test]
fn birth_body_anchors_audio_after_embed() {
    let body = super::v2_birth_body(
        "Stem",
        "sase",
        "ref-stem",
        "",
        Some("lib/papers/stem.mp3"),
    );
    let lines: Vec<&str> = body.lines().collect();
    let embed = lines
        .iter()
        .position(|line| *line == "![[sase#^ref-stem]]")
        .expect("embed");
    let audio = lines
        .iter()
        .position(|line| *line == "![[lib/papers/stem.mp3]]")
        .expect("audio");
    let highlights = lines
        .iter()
        .position(|line| *line == "## Highlights")
        .expect("highlights");
    assert!(embed < audio && audio < highlights);
}

#[test]
fn audio_anchors_after_managed_embed() {
    let body = "# Stem\n\n![[sase#^ref-stem]]\n\n## Highlights\n";
    let anchored = super::maybe_insert_audio_embed_after_managed(
        body,
        "lib/papers/stem.mp3",
    );
    assert_eq!(
        anchored,
        "# Stem\n\n![[sase#^ref-stem]]\n\n![[lib/papers/stem.mp3]]\n\n## Highlights\n"
    );
}

#[test]
fn audio_anchor_never_duplicates_or_invents() {
    let present = "# Stem\n\n![[sase#^ref-stem]]\n\n![[lib/papers/stem.mp3]]\n";
    assert_eq!(
        super::maybe_insert_audio_embed_after_managed(
            present,
            "lib/papers/stem.mp3"
        ),
        present
    );
    let no_embed = "# Stem\n\nSome notes.\n";
    assert_eq!(
        super::maybe_insert_audio_embed_after_managed(
            no_embed,
            "lib/papers/stem.mp3"
        ),
        no_embed
    );
}

#[test]
fn region_own_notes_excludes_managed_embed() {
    let body = "# Stem\n\n![[sase#^ref-stem]]\n\nMy notes.\n\n## Highlights\n\n<!-- highlights:begin -->\n\nhl\n\n<!-- highlights:end -->\n";
    let parts = super::split_note_body(body);
    assert_eq!(parts.h1.as_deref(), Some("Stem"));
    assert!(!parts.own_notes.contains("![[sase#^ref-stem]]"));
    assert!(parts.own_notes.contains("My notes."));
}

#[test]
fn region_own_notes_keeps_legacy_body_verbatim() {
    let body = "# Stem\n\n![[sase#^ref-stem]]\n\nMy notes.\n";
    let parts = super::split_note_body(body);
    assert!(parts.own_notes.contains("![[sase#^ref-stem]]"));
}

#[test]
fn v1_birth_body_stays_frozen() {
    let projection = test_projection(vec![
        ("status", string_value("next")),
        ("parent", string_value("[[obsidian_ref]]")),
        ("title", string_value("Stem")),
    ]);
    let body = super::default_note_body(
        std::path::Path::new("lib/papers/stem.pdf"),
        &projection,
        "lib/papers/stem.pdf",
        None,
        None,
    );
    assert!(body.contains("^ref"), "v1 tracker kept");
    assert!(body.contains("#hide"), "v1 hide tag kept");
}

#[test]
fn pdf_task_question_overlay_stays_put() {
    let body = "- [?] #task [[lib/papers/stem.pdf]] #hide ^ref\n";
    let projection = test_projection(vec![("status", string_value("ready"))]);
    let rewritten =
        super::rewrite_pdf_task_checkbox_for_projection(body, &projection)
            .expect("rewrite");
    assert_eq!(rewritten, body, "[?] overlay preserved");
}

#[allow(clippy::too_many_arguments)]
fn plan_inputs<'a>(
    note_exists: bool,
    body: &'a str,
    candidates: &'a [LocatedRefTask],
    resolved: Option<&'a str>,
    base: Option<&'a str>,
    marker: Option<&'a str>,
    frontmatter: Option<&'a str>,
    resolver: &'a dyn Fn(&str) -> TestResolverResult,
    archive_open: bool,
) -> super::ReadingTaskPlanInputs<'a> {
    super::ReadingTaskPlanInputs {
        note_exists,
        ref_note_rel: "ref/papers/stem.md",
        note_body: body,
        candidates,
        v1_hits: &[],
        parent_hint: "sase",
        resolved_status: resolved,
        base_status: base,
        marker_status: marker,
        frontmatter_status: frontmatter,
        archive_source_open: archive_open,
        resolver,
    }
}

#[test]
fn plan_v1_branch_hands_off() {
    let body = "# Stem\n\n- [ ] #task [[lib/papers/stem.pdf]] #hide ^ref\n";
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        true,
        body,
        &[],
        None,
        None,
        None,
        None,
        resolver,
        false,
    ))
    .expect("plan");
    assert_eq!(plan.branch, super::NoteBranch::V1);
    assert_eq!(plan.kind, super::ReadingTaskKind::V1);
    assert_eq!(plan.action, super::ReadingTaskAction::NoWrite);
}

#[test]
fn plan_birth_inserts_into_resolved_parent() {
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        false,
        "",
        &[],
        Some("next"),
        None,
        Some("next"),
        None,
        resolver,
        false,
    ))
    .expect("plan");
    assert_eq!(plan.kind, super::ReadingTaskKind::Birth);
    assert_eq!(
        plan.action,
        super::ReadingTaskAction::Insert {
            destination_route: "sase".to_string(),
            destination_label: "sase.md".to_string(),
            warning_child: None,
            mark: '*',
            prefer_block_id: None,
        }
    );
    assert_eq!(plan.embed, None, "ID allocated at execution");
}

#[test]
fn plan_birth_warns_on_unresolvable_hint() {
    let failing = |_: &str| -> TestResolverResult {
        Err(ParentError::Unknown {
            input: "nope".to_string(),
            normalized: "nope".to_string(),
            suggestions: Vec::new(),
        })
    };
    let resolver = &failing as &dyn Fn(&str) -> TestResolverResult;
    let mut inputs = plan_inputs(
        false,
        "",
        &[],
        Some("ready"),
        None,
        Some("ready"),
        None,
        resolver,
        false,
    );
    inputs.parent_hint = "nope";
    let plan = super::plan_reading_task(inputs).expect("plan");
    match plan.action {
        super::ReadingTaskAction::Insert {
            destination_route,
            warning_child,
            ..
        } => {
            assert_eq!(destination_route, "mac_inbox");
            assert!(warning_child.is_some_and(|child| child.contains("'nope'")));
        }
        other => panic!("expected insert, got {other:?}"),
    }
}

#[test]
fn plan_birth_adopts_orphan() {
    let orphan = located("sase.md", '*', Some("ref-stem"), false, Some("sase"));
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        false,
        "",
        &[orphan],
        Some("next"),
        None,
        Some("next"),
        None,
        resolver,
        false,
    ))
    .expect("plan");
    assert_eq!(plan.kind, super::ReadingTaskKind::Birth);
    assert_eq!(plan.action, super::ReadingTaskAction::NoWrite);
    assert_eq!(plan.residence.as_deref(), Some("sase"));
    assert_eq!(
        plan.embed,
        Some(super::ReadingTaskEmbed {
            target: "sase".to_string(),
            block_id: "ref-stem".to_string(),
        })
    );
}

#[test]
fn plan_existing_marker_change_edits_line() {
    let task = located("sase.md", '*', Some("ref-stem"), false, Some("sase"));
    let body = "# Stem\n\n![[sase#^ref-stem]]\n";
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        true,
        body,
        &[task],
        Some("ready"),
        Some("next"),
        Some("ready"),
        Some("next"),
        resolver,
        false,
    ))
    .expect("plan");
    assert_eq!(plan.kind, super::ReadingTaskKind::Existing);
    assert_eq!(
        plan.action,
        super::ReadingTaskAction::LineEdit { target_mark: ' ' }
    );
}

#[test]
fn plan_reopen_archived_inserts_into_source_parent() {
    let task = located(
        "done/sase_done.md",
        'x',
        Some("ref-stem"),
        true,
        Some("sase"),
    );
    let body = "# Stem\n\n![[done/sase_done#^ref-stem]]\n";
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        true,
        body,
        &[task],
        Some("next"),
        Some("read"),
        Some("next"),
        Some("next"),
        resolver,
        true,
    ))
    .expect("plan");
    assert_eq!(plan.kind, super::ReadingTaskKind::Reopen);
    assert_eq!(
        plan.action,
        super::ReadingTaskAction::Insert {
            destination_route: "sase".to_string(),
            destination_label: "sase.md".to_string(),
            warning_child: None,
            mark: '*',
            prefer_block_id: Some("ref-stem".to_string()),
        }
    );
    assert!(!plan.refuse_status_parent_writes);
}

#[test]
fn plan_reopen_without_source_parent_uses_inbox() {
    let task = located(
        "done/sase_done.md",
        'x',
        Some("ref-stem"),
        true,
        Some("sase"),
    );
    let body = "# Stem\n\n![[done/sase_done#^ref-stem]]\n";
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        true,
        body,
        &[task],
        Some("next"),
        Some("read"),
        Some("next"),
        Some("next"),
        resolver,
        false,
    ))
    .expect("plan");
    match plan.action {
        super::ReadingTaskAction::Insert {
            destination_route,
            warning_child,
            prefer_block_id,
            ..
        } => {
            assert_eq!(destination_route, "mac_inbox");
            assert!(warning_child.is_some());
            assert_eq!(prefer_block_id.as_deref(), Some("ref-stem"));
        }
        other => panic!("expected reopen insert, got {other:?}"),
    }
}

#[test]
fn plan_missing_open_never_invents_a_task() {
    let body = "# Stem\n\n![[sase#^ref-stem]]\n";
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        true,
        body,
        &[],
        Some("next"),
        None,
        Some("next"),
        Some("next"),
        resolver,
        false,
    ))
    .expect("plan");
    assert_eq!(plan.kind, super::ReadingTaskKind::Missing);
    assert_eq!(plan.action, super::ReadingTaskAction::NoWrite);
    assert_eq!(plan.embed, None);
    assert!(
        plan.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "open_ref_without_task"),
        "{:?}",
        plan.diagnostics
    );
}

#[test]
fn plan_refused_multiple_open_blocks_writes() {
    let first = located("sase.md", '*', Some("ref-stem"), false, Some("sase"));
    let second = located("bob.md", '/', Some("ref-stem"), false, Some("bob"));
    let body = "# Stem\n\n![[sase#^ref-stem]]\n";
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        true,
        body,
        &[first, second],
        Some("next"),
        Some("next"),
        Some("next"),
        Some("next"),
        resolver,
        false,
    ))
    .expect("plan");
    assert_eq!(plan.kind, super::ReadingTaskKind::Refused);
    assert!(plan.refuse_status_parent_writes);
    assert_eq!(plan.action, super::ReadingTaskAction::NoWrite);
}

#[test]
fn plan_closed_terminal_stays_quiet() {
    let task = located("sase.md", 'x', Some("ref-stem"), false, Some("sase"));
    let body = "# Stem\n\n![[sase#^ref-stem]]\n";
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let plan = super::plan_reading_task(plan_inputs(
        true,
        body,
        &[task],
        Some("read"),
        Some("read"),
        Some("read"),
        Some("read"),
        resolver,
        false,
    ))
    .expect("plan");
    assert_eq!(plan.kind, super::ReadingTaskKind::ClosedTerminal);
    assert_eq!(plan.action, super::ReadingTaskAction::NoWrite);
    assert_eq!(
        plan.embed,
        Some(super::ReadingTaskEmbed {
            target: "sase".to_string(),
            block_id: "ref-stem".to_string(),
        })
    );
}

#[test]
fn plan_task_gesture_conflict_errors() {
    let task = located("sase.md", 'x', Some("ref-stem"), false, Some("sase"));
    let body = "# Stem\n\n![[sase#^ref-stem]]\n";
    let resolver = &stub_resolver as &dyn Fn(&str) -> TestResolverResult;
    let error = super::plan_reading_task(plan_inputs(
        true,
        body,
        &[task],
        Some("abandoned"),
        Some("next"),
        Some("abandoned"),
        Some("next"),
        resolver,
        false,
    ))
    .expect_err("incompatible gestures conflict");
    assert!(error.to_string().contains("conflict"), "{error}");
}
