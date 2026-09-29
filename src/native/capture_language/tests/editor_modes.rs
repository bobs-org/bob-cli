//! Editor mode, need, and agreement tests.

use super::super::draft::*;
use super::super::editor_model::*;
use super::super::editor_parse::*;
use super::super::model::*;
use super::super::project_tasks::*;
use super::*;

#[test]
fn editor_modes_and_needs_cover_every_marker_shape() {
    // (input, mode, route, section, block_id, needs)
    let cases: &[MarkerCase] = &[
        (
            "Body @dev+focus-123",
            EditorMode::SubBullet,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "Body @dev+",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::Task],
        ),
        (
            "Body @+focus-123",
            EditorMode::Incomplete,
            None,
            None,
            Some("focus-123"),
            &[Need::Route],
        ),
        (
            "Body @+",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::Task],
        ),
        (
            "Body @dev+focus-123#req",
            EditorMode::SubBullet,
            Some("dev"),
            Some("req"),
            Some("focus-123"),
            &[],
        ),
        (
            "Body @dev+focus-123#",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            Some("focus-123"),
            &[Need::TaskSection],
        ),
        (
            "Body @dev+#req",
            EditorMode::Incomplete,
            Some("dev"),
            Some("req"),
            None,
            &[Need::Task],
        ),
        (
            "Body @dev+#",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::Task, Need::TaskSection],
        ),
        (
            "Body @+focus-123#req",
            EditorMode::Incomplete,
            None,
            Some("req"),
            Some("focus-123"),
            &[Need::Route],
        ),
        (
            "Body @+#req",
            EditorMode::Incomplete,
            None,
            Some("req"),
            None,
            &[Need::Route, Need::Task],
        ),
        (
            "Body @+#",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::Task, Need::TaskSection],
        ),
        (
            "Body @dev^focus-123",
            EditorMode::Task,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "Body @dev^",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::BlockId],
        ),
        (
            "Body @^focus-123",
            EditorMode::Incomplete,
            None,
            None,
            Some("focus-123"),
            &[Need::Route],
        ),
        (
            "Body @^",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::BlockId],
        ),
        (
            "Body @dev^focus-123+",
            EditorMode::ProjectNote,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "Body @^focus-123+",
            EditorMode::ProjectNote,
            None,
            None,
            Some("focus-123"),
            &[Need::Route],
        ),
        (
            "Body @dev^+",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::BlockId],
        ),
        (
            "Body @dev:focus-123",
            EditorMode::PomodoroTask,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "Body @dev:focus-123#bugs",
            EditorMode::PomodoroTask,
            Some("dev"),
            Some("bugs"),
            Some("focus-123"),
            &[],
        ),
        (
            "Body @dev:focus-123#",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            Some("focus-123"),
            &[Need::PomodoroName],
        ),
        (
            "Body @dev:#bugs",
            EditorMode::Incomplete,
            Some("dev"),
            Some("bugs"),
            None,
            &[Need::PomodoroId],
        ),
        (
            "Body @dev:#",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::PomodoroId, Need::PomodoroName],
        ),
        (
            "Body @!dev:focus-123",
            EditorMode::PomodoroTask,
            Some("dev"),
            None,
            Some("focus-123"),
            &[],
        ),
        (
            "Body @!dev:focus-123#bugs",
            EditorMode::PomodoroTask,
            Some("dev"),
            Some("bugs"),
            Some("focus-123"),
            &[],
        ),
        (
            "Body @dev:",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::PomodoroId],
        ),
        (
            "Body @!dev",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::PomodoroId],
        ),
        (
            "Body @:focus-123",
            EditorMode::Incomplete,
            None,
            None,
            Some("focus-123"),
            &[Need::Route],
        ),
        (
            "Body @:",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::PomodoroId],
        ),
        (
            "Body @dev^focus-123+#",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            Some("focus-123"),
            &[Need::PomodoroName],
        ),
        (
            "Body @dev:+",
            EditorMode::Incomplete,
            Some("dev"),
            None,
            None,
            &[Need::PomodoroId],
        ),
        (
            "Body @!",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route, Need::PomodoroId],
        ),
        (
            "Body @",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route],
        ),
        (
            "Body @#",
            EditorMode::Incomplete,
            None,
            None,
            None,
            &[Need::Route],
        ),
        (
            "Body @#Ideas",
            EditorMode::Incomplete,
            None,
            Some("Ideas"),
            None,
            &[Need::Route],
        ),
        (
            "Body @notes#",
            EditorMode::Bullet,
            Some("notes"),
            None,
            None,
            &[Need::Section],
        ),
        (
            "Body @notes#Ideas",
            EditorMode::Bullet,
            Some("notes"),
            Some("Ideas"),
            None,
            &[],
        ),
        (
            "Body @work",
            EditorMode::Task,
            Some("work"),
            None,
            None,
            &[],
        ),
        ("buy milk", EditorMode::Task, None, None, None, &[]),
        ("@route", EditorMode::Task, None, None, None, &[]),
    ];

    for (raw, mode, route, section, block_id, needs) in cases {
        let parse = editor(raw);
        assert_eq!(parse.mode, *mode, "{raw}");
        assert_eq!(parse.route.as_deref(), *route, "{raw}");
        assert_eq!(parse.section.as_deref(), *section, "{raw}");
        assert_eq!(parse.block_id.as_deref(), *block_id, "{raw}");
        assert_eq!(parse.needs, needs.to_vec(), "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
}

#[test]
fn editor_reports_unused_project_note_pomodoro_over_the_name() {
    // A `#pomodoro` name with no ` :` task is unused until task bullets
    // can name IDs. The token-level mode stays `project_note`.
    let raw = "Finish it @cash^goog-exit+#bugs";
    let parse = editor(raw);
    assert_eq!(parse.mode, EditorMode::ProjectNote);
    assert_eq!(parse.route.as_deref(), Some("cash"));
    assert_eq!(parse.section.as_deref(), Some("bugs"));
    assert_eq!(parse.block_id.as_deref(), Some("goog-exit"));
    assert!(parse.needs.is_empty());
    assert_eq!(codes(&parse), vec!["unused_project_note_pomodoro"]);
    let diagnostic = &parse.diagnostics[0];
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(
        diagnostic.message,
        unused_project_note_pomodoro_error("bugs")
    );
    let hash = raw.find('#').expect("hash");
    assert_eq!(diagnostic.range, Some((hash, raw.len())));

    // An empty name is still an incomplete state with a placeholder over
    // `#`, not an error.
    let partial = editor("Finish it @cash^goog-exit+#");
    assert_eq!(partial.mode, EditorMode::Incomplete);
    assert_eq!(partial.needs, vec![Need::PomodoroName]);
    assert!(partial.diagnostics.is_empty());
}

#[test]
fn editor_reports_project_task_ids_with_modes_spans_and_diagnostics() {
    let raw = "Finish the Google exit packet! @cash^goog-exit+#admin\n\
               - Draft the resignation memo :draft-memo\n\
               \x20 - keep it short\n\
               - Collect the equity paperwork ^equity-docs";
    let parse = editor(raw);
    assert_eq!(parse.mode, EditorMode::PomodoroProjectNote);
    assert_eq!(parse.route.as_deref(), Some("cash"));
    assert_eq!(parse.section.as_deref(), Some("admin"));
    assert_eq!(parse.block_id.as_deref(), Some("goog-exit"));
    assert!(parse.needs.is_empty());
    assert!(parse.diagnostics.is_empty());
    assert_eq!(
        parse
            .sub_bullets
            .iter()
            .map(|sub| (sub.body.as_str(), sub.task_id.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                "Draft the resignation memo",
                Some(ProjectTaskId {
                    block_id: "draft-memo".to_string(),
                    link: true,
                })
            ),
            ("keep it short", None),
            (
                "Collect the equity paperwork",
                Some(ProjectTaskId {
                    block_id: "equity-docs".to_string(),
                    link: false,
                })
            ),
        ]
    );
    // `:` spans the sigil as a link marker plus the ID; `^` spans only
    // the ID, exactly like separators elsewhere.
    let memo = raw.find(":draft-memo").expect("memo id");
    let docs = raw.find("^equity-docs").expect("docs id");
    assert!(
        ranges(&parse).contains(&(
            memo,
            memo + 1,
            SpanKind::ProjectTaskLinkMarker
        )),
        "{raw}"
    );
    assert!(
        ranges(&parse).contains(&(
            memo + 1,
            memo + ":draft-memo".len(),
            SpanKind::ProjectTaskBlockId
        )),
        "{raw}"
    );
    assert!(
        ranges(&parse).contains(&(
            docs + 1,
            docs + "^equity-docs".len(),
            SpanKind::ProjectTaskBlockId
        )),
        "{raw}"
    );
    assert!(parse.spans.iter().all(|span| span.start != docs), "{raw}");

    // A `^`-only project note keeps the `project_note` mode.
    let caret_only = editor("Finish it @cash^x+\n- Doc ^docs");
    assert_eq!(caret_only.mode, EditorMode::ProjectNote);
    assert!(caret_only.diagnostics.is_empty());

    // Every rule violation is a diagnostic with the execution message,
    // ranged over the offending token (the `#name` component for the
    // unused-`#pomodoro` rule, the second ID for duplicates).
    let cases: &[(&str, &str, String, &str, bool)] = &[
        (
            "Finish it :foo @cash^x+",
            "misplaced_project_task_id",
            misplaced_parent_task_id_error(":foo"),
            ":foo",
            false,
        ),
        (
            "Finish it @cash^x+\n- Draft\n  - nested :foo",
            "misplaced_project_task_id",
            misplaced_nested_task_id_error(":foo"),
            ":foo",
            false,
        ),
        (
            "Finish it @cash^x+\n- Draft :a_b",
            "invalid_project_task_id",
            invalid_project_task_id_charset_error("a_b"),
            ":a_b",
            false,
        ),
        (
            "Finish it @cash^x+\n- Draft :PRJ",
            "invalid_project_task_id",
            reserved_project_task_id_error("PRJ"),
            ":PRJ",
            false,
        ),
        (
            "Finish it @cash^x+\n- :foo",
            "invalid_project_task_id",
            empty_project_task_body_error(2),
            ":foo",
            false,
        ),
        (
            "Finish it @cash^x+\n- [x] Foo :foo",
            "invalid_project_task_id",
            checkbox_project_task_id_error("foo", 'x'),
            ":foo",
            false,
        ),
        (
            "Finish it @cash^x+\n- One :same\n- Two :same",
            "duplicate_project_task_id",
            duplicate_project_task_id_error("same", 2, 3),
            ":same",
            true,
        ),
        (
            "Finish it @cash^x+#bugs\n- Doc ^docs",
            "unused_project_note_pomodoro",
            unused_project_note_pomodoro_error("bugs"),
            "#bugs",
            false,
        ),
    ];
    for (raw, code, message, needle, last) in cases {
        let parse = editor(raw);
        assert_eq!(codes(&parse), vec![*code], "{raw}");
        assert_eq!(parse.diagnostics[0].message, *message, "{raw}");
        assert_eq!(parse.diagnostics[0].severity, Severity::Error, "{raw}");
        let start = if *last {
            raw.rfind(needle).expect("needle")
        } else {
            raw.find(needle).expect("needle")
        };
        assert_eq!(
            parse.diagnostics[0].range,
            Some((start, start + needle.len())),
            "{raw}"
        );
    }

    // A duplicate keeps the first ID and still upgrades the mode.
    let duplicate = editor("Finish it @cash^x+\n- One :same\n- Two :same");
    assert_eq!(duplicate.mode, EditorMode::PomodoroProjectNote);

    // A lone sigil ending a first-level bullet is unfinished: mode
    // `incomplete` needing `block_id`, a placeholder span over the sigil,
    // and no diagnostic.
    for (raw, sigil) in [
        ("Finish it @cash^x+\n- Draft :", ':'),
        ("Finish it @cash^x+\n- Draft ^", '^'),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
        assert_eq!(parse.needs, vec![Need::BlockId], "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
        let start = raw.rfind(sigil).expect("sigil");
        assert!(
            ranges(&parse).contains(&(
                start,
                start + 1,
                SpanKind::InteractivePlaceholder
            )),
            "{raw}"
        );
    }

    // Outside a project-note item the lookalike stays literal text.
    let literal = editor("Fix @sase\n- ratio 3 :1");
    assert_eq!(literal.mode, EditorMode::Task);
    assert_eq!(sub_bullet_bodies(&literal.sub_bullets), vec!["ratio 3 :1"]);
    assert!(literal.sub_bullets[0].task_id.is_none());
    assert!(literal.diagnostics.is_empty());
}

#[test]
fn editor_spans_cover_every_marker_shape() {
    let cases: &[(&str, &[SpanKind])] = &[
        (
            "Body @dev+focus-123",
            &[SpanKind::SubBulletRoute, SpanKind::SubBulletBlockId],
        ),
        (
            "Body @dev+",
            &[SpanKind::SubBulletRoute, SpanKind::InteractivePlaceholder],
        ),
        (
            "Body @+focus-123",
            &[SpanKind::InteractivePlaceholder, SpanKind::SubBulletBlockId],
        ),
        ("Body @+", &[SpanKind::InteractivePlaceholder]),
        (
            "Body @dev+focus-123#req",
            &[
                SpanKind::SubBulletRoute,
                SpanKind::SubBulletBlockId,
                SpanKind::SubBulletSection,
            ],
        ),
        (
            "Body @dev+focus-123#",
            &[
                SpanKind::SubBulletRoute,
                SpanKind::SubBulletBlockId,
                SpanKind::InteractivePlaceholder,
            ],
        ),
        (
            "Body @dev+#req",
            &[
                SpanKind::SubBulletRoute,
                SpanKind::InteractivePlaceholder,
                SpanKind::SubBulletSection,
            ],
        ),
        (
            "Body @dev+#",
            &[
                SpanKind::SubBulletRoute,
                SpanKind::InteractivePlaceholder,
                SpanKind::InteractivePlaceholder,
            ],
        ),
        (
            "Body @+focus-123#req",
            &[
                SpanKind::InteractivePlaceholder,
                SpanKind::SubBulletBlockId,
                SpanKind::SubBulletSection,
            ],
        ),
        (
            "Body @+#req",
            &[SpanKind::InteractivePlaceholder, SpanKind::SubBulletSection],
        ),
        (
            "Body @+#",
            &[
                SpanKind::InteractivePlaceholder,
                SpanKind::InteractivePlaceholder,
            ],
        ),
        (
            "Body @dev^focus-123",
            &[SpanKind::TaskBlockIdRoute, SpanKind::TaskBlockId],
        ),
        (
            "Body @dev^",
            &[SpanKind::TaskBlockIdRoute, SpanKind::InteractivePlaceholder],
        ),
        (
            "Body @^focus-123",
            &[SpanKind::InteractivePlaceholder, SpanKind::TaskBlockId],
        ),
        ("Body @^", &[SpanKind::InteractivePlaceholder]),
        (
            "Body @dev^focus-123+",
            &[
                SpanKind::TaskBlockIdRoute,
                SpanKind::TaskBlockId,
                SpanKind::ProjectNoteMarker,
            ],
        ),
        (
            "Body @^focus-123+",
            &[
                SpanKind::InteractivePlaceholder,
                SpanKind::TaskBlockId,
                SpanKind::ProjectNoteMarker,
            ],
        ),
        (
            "Body @dev^+",
            &[
                SpanKind::TaskBlockIdRoute,
                SpanKind::InteractivePlaceholder,
                SpanKind::ProjectNoteMarker,
            ],
        ),
        (
            "Body @dev:focus-123",
            &[SpanKind::PomodoroRoute, SpanKind::PomodoroBlockId],
        ),
        (
            "Body @dev:focus-123#bugs",
            &[
                SpanKind::PomodoroRoute,
                SpanKind::PomodoroBlockId,
                SpanKind::PomodoroName,
            ],
        ),
        (
            "Body @dev:focus-123#",
            &[
                SpanKind::PomodoroRoute,
                SpanKind::PomodoroBlockId,
                SpanKind::InteractivePlaceholder,
            ],
        ),
        (
            "Body @dev^focus-123+#bugs",
            &[
                SpanKind::TaskBlockIdRoute,
                SpanKind::TaskBlockId,
                SpanKind::ProjectNoteMarker,
                SpanKind::PomodoroName,
            ],
        ),
        (
            "Body @dev^focus-123+#",
            &[
                SpanKind::TaskBlockIdRoute,
                SpanKind::TaskBlockId,
                SpanKind::ProjectNoteMarker,
                SpanKind::InteractivePlaceholder,
            ],
        ),
        (
            "Body @dev:#bugs",
            &[
                SpanKind::PomodoroRoute,
                SpanKind::InteractivePlaceholder,
                SpanKind::PomodoroName,
            ],
        ),
        (
            "Body @dev:#",
            &[
                SpanKind::PomodoroRoute,
                SpanKind::InteractivePlaceholder,
                SpanKind::InteractivePlaceholder,
            ],
        ),
        (
            "Body @!dev:focus-123",
            &[SpanKind::PomodoroRoute, SpanKind::PomodoroBlockId],
        ),
        (
            "Body @dev:",
            &[SpanKind::PomodoroRoute, SpanKind::InteractivePlaceholder],
        ),
        ("Body @!dev", &[SpanKind::PomodoroRoute]),
        (
            "Body @:focus-123",
            &[SpanKind::InteractivePlaceholder, SpanKind::PomodoroBlockId],
        ),
        ("Body @:", &[SpanKind::InteractivePlaceholder]),
        ("Body @!", &[SpanKind::InteractivePlaceholder]),
        ("Body @", &[SpanKind::InteractivePlaceholder]),
        ("Body @#", &[SpanKind::InteractivePlaceholder]),
        (
            "Body @#Ideas",
            &[SpanKind::InteractivePlaceholder, SpanKind::Section],
        ),
        (
            "Body @notes#",
            &[SpanKind::Route, SpanKind::InteractivePlaceholder],
        ),
        ("Body @notes#Ideas", &[SpanKind::Route, SpanKind::Section]),
        ("Body @work", &[SpanKind::Route]),
        ("buy milk", &[]),
    ];

    for (raw, kinds) in cases {
        let parse = editor(raw);
        assert_eq!(span_kinds(&parse), kinds.to_vec(), "{raw}");
        for span in &parse.spans {
            assert!(raw.is_char_boundary(span.start), "{raw}");
            assert!(raw.is_char_boundary(span.end), "{raw}");
            assert!(span.start < span.end, "{raw}");
        }
        for pair in parse.spans.windows(2) {
            assert!(pair[0].end <= pair[1].start, "{raw}");
        }
    }
}

#[test]
fn editor_agrees_with_execution_for_resolved_captures() {
    let inputs = [
        "buy milk",
        "buy milk @groceries",
        "@Groceries Buy Milk",
        "a @b @C",
        "@Work buy milk @home",
        "Email @home soon",
        "@route",
        "@bad! body @Good",
        "Do thing @Dev^Foo-Bar",
        "@Dev^Foo-Bar Do thing s:2",
        "Do thing @Dev^Foo-Bar p:2 s:1",
        "Do thing @Dev:Foo-Bar",
        "@Dev:Foo-Bar Do thing s:2",
        "Do thing @!Dev:Foo-Bar s:2",
        "Do thing @Dev:Foo-Bar#bugs",
        "@Dev:Foo-Bar#bugs Do thing s:2",
        "Do thing @!Dev:Foo-Bar#bugs s:2",
        "Do work @sase:deep-fix#c++",
        "@sase:deep-fix#c++ Do work",
        "Do work @sase:deep-fix#bob+sase",
        "Called today @Cash+Goog-Exit",
        "Called today %log @Cash+Goog-Exit",
        "Called today @Cash+Goog-Exit s:1",
        "Postgres 17 minimum @foo+bar#requirements",
        "@foo+bar#requirements Postgres 17 minimum",
        "Postgres 17 minimum @foo+bar#requirements s:1",
        "Postgres 17 minimum %log @foo+bar#requirements",
        "Postgres 17 minimum @foo+bar#q-and-a",
        "Postgres 17 minimum @foo+bar#Q&A",
        "Finish the Google exit packet! @cash^goog-exit+#admin\n- Draft the resignation memo :draft-memo\n  - keep it short\n- Collect the equity paperwork ^equity-docs",
        "Finish it\n- Draft :d @cash^x+",
        "Fix @sase\n- ratio 3 :1",
        "@Cash+Goog-Exit",
        "@Cash+Goog-Exit!",
        "@Cash+Goog-Exit#bugs",
        "@Cash+Goog-Exit#deep+work",
        "Some note @foo#bar",
        "@foo#bar Some note",
        "Some note @foo#",
        "@foo# Some note",
        "body p:2 s:1 % @groceries",
        "body @groceries %log p:3 s:4",
        "Jot @notes#time:box",
        "take s:1 pill",
        "save % now",
        "body %bad!",
        "remembered to bump the timeout #",
        "paste the failing output % #",
        "paste the failing output # %",
        "Finish it @cash^goog-exit+",
        "=x",
        "=X",
        "=x1",
        "=x1,3",
        "=x3,1",
        "=x!2",
        "=x1,3!2",
        "=X1!2",
        "=x0",
        "=x0!2",
        "@r:id=x",
        "@r:id=x1!2",
        "^r:id=x",
        "^r:id=x1",
        "Text @r:id=x",
        "Text @r:id=X",
        "Text @r:id=x!1",
        "+",
        "-",
        "++",
        "--",
        "+5",
        "-2",
        "++3",
        "--2",
        "  ++3  ",
        "++03",
        "=",
        "=3",
        "=-",
        "=-2",
        "=3-",
        "=2-1",
        "=0",
        "=03",
        "  =3  ",
    ];

    for raw in inputs {
        let executed =
            parse_capture_text_with_clip_control(raw, None, None, true)
                .unwrap_or_else(|error| panic!("{raw}: {error}"));
        let parse = editor(raw);
        assert_eq!(parse.body, executed.body, "{raw}");
        assert_eq!(parse.route, executed.route, "{raw}");
        let expected_mode = match &executed.kind {
            CaptureKind::Task | CaptureKind::TaskWithBlockId { .. } => {
                EditorMode::Task
            }
            CaptureKind::Bullet { .. } => EditorMode::Bullet,
            CaptureKind::Pomodoro { .. } => EditorMode::PomodoroTask,
            CaptureKind::SubBullet { .. } => EditorMode::SubBullet,
            CaptureKind::PomodoroNote => EditorMode::PomodoroNote,
            CaptureKind::ProjectNote { .. } => {
                if executed.sub_bullets.iter().any(|sub| {
                    sub.task_id.as_ref().is_some_and(|task| task.link)
                }) {
                    EditorMode::PomodoroProjectNote
                } else {
                    EditorMode::ProjectNote
                }
            }
            CaptureKind::TaskToggle { .. } => EditorMode::TaskToggle,
            CaptureKind::PomodoroAdjust { .. } => EditorMode::PomodoroAdjust,
            CaptureKind::PomodoroShift { .. } => EditorMode::PomodoroShift,
            CaptureKind::PomodoroLink { .. } => EditorMode::PomodoroLink,
            CaptureKind::PomodoroClose { .. } => EditorMode::PomodoroClose,
            CaptureKind::PomodoroStart { .. } => EditorMode::PomodoroStart,
        };
        assert_eq!(parse.mode, expected_mode, "{raw}");
        assert_eq!(
            parse
                .sub_bullets
                .iter()
                .map(|sub| (sub.body.as_str(), sub.task_id.clone()))
                .collect::<Vec<_>>(),
            executed
                .sub_bullets
                .iter()
                .map(|sub| (sub.body.as_str(), sub.task_id.clone()))
                .collect::<Vec<_>>(),
            "{raw}"
        );
        if let CaptureKind::TaskWithBlockId { block_id } = &executed.kind {
            assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
        }
        if let CaptureKind::Pomodoro {
            block_id,
            pomodoro_name,
            ..
        } = &executed.kind
        {
            assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
            assert_eq!(
                parse.section.as_deref(),
                pomodoro_name.as_deref(),
                "{raw}"
            );
        }
        if let CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId(block_id),
            section,
        } = &executed.kind
        {
            assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
            assert_eq!(
                parse.section.as_deref(),
                section.as_ref().map(|selector| selector.text.as_str()),
                "{raw}"
            );
        }
        if let CaptureKind::Bullet { section_prefix, .. } = &executed.kind {
            assert_eq!(parse.section, *section_prefix, "{raw}");
        }
        if let CaptureKind::TaskToggle {
            block_id,
            pomodoro_name,
            ..
        } = &executed.kind
        {
            assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
            assert_eq!(
                parse.section.as_deref(),
                pomodoro_name.as_deref(),
                "{raw}"
            );
        }
        if let CaptureKind::ProjectNote {
            block_id,
            pomodoro_name,
        } = &executed.kind
        {
            assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
            assert_eq!(
                parse.section.as_deref(),
                pomodoro_name.as_deref(),
                "{raw}"
            );
        }
        // The close `raw` keeps the typed `=`, so parse and execution
        // agree on link forms as well as whole-item closes. Compare the
        // full selection, not only `raw`.
        if let CaptureKind::Pomodoro { close, .. } = &executed.kind {
            match close {
                Some(expected) => {
                    let actual =
                        parse.pomodoro_close.as_ref().expect("close spec");
                    assert_eq!(actual.raw, expected.raw, "{raw}");
                    assert_eq!(
                        actual.in_progress, expected.in_progress,
                        "{raw}"
                    );
                    assert_eq!(actual.complete, expected.complete, "{raw}");
                }
                None => assert!(parse.pomodoro_close.is_none(), "{raw}"),
            }
        }
        if let CaptureKind::PomodoroClose { spec } = &executed.kind {
            let actual = parse.pomodoro_close.as_ref().expect("close spec");
            assert_eq!(actual.raw, spec.raw, "{raw}");
            assert_eq!(actual.in_progress, spec.in_progress, "{raw}");
            assert_eq!(actual.complete, spec.complete, "{raw}");
        }
        if let CaptureKind::PomodoroStart { spec } = &executed.kind {
            let actual = parse.pomodoro_start.as_ref().expect("start spec");
            assert_eq!(actual.raw, spec.raw, "{raw}");
            assert_eq!(actual.duration_units, spec.duration_units, "{raw}");
            assert_eq!(actual.offset_units, spec.offset_units, "{raw}");
        }
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
}

#[test]
fn editor_reports_pomodoro_close_modes_spans_specs_and_diagnostics() {
    // Exact whole-item closes: mode, span, and additive spec.
    for raw in ["=x", "=X", "  =x  "] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroClose, "{raw}");
        assert_eq!(parse.body, raw.trim(), "{raw}");
        assert!(parse.needs.is_empty(), "{raw}");
        let close = parse.pomodoro_close.as_ref().expect("close spec");
        assert_eq!(close.raw, raw.trim(), "{raw}");
        assert!(
            ranges(&parse).contains(&(
                raw.find('=').expect("="),
                raw.find('=').expect("=") + 2,
                SpanKind::PomodoroClose
            )),
            "{raw}"
        );
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
    // Close suffixes keep their link/task mode with a `pomodoro_close`
    // span and spec.
    for (raw, mode, body) in [
        ("@r:id=x", EditorMode::PomodoroLink, ""),
        ("^r:id=x", EditorMode::PomodoroLink, ""),
        ("Text @r:id=x", EditorMode::PomodoroTask, "Text"),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, mode, "{raw}");
        assert_eq!(parse.body, body, "{raw}");
        assert!(parse.pomodoro_close.is_some(), "{raw}");
        assert!(parse.pomodoro_start.is_none(), "{raw}");
        assert!(
            span_kinds(&parse).contains(&SpanKind::PomodoroClose),
            "{raw}"
        );
        assert!(
            !span_kinds(&parse).contains(&SpanKind::PomodoroStart),
            "{raw}"
        );
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
    // Selection-bearing closes report the additive spec and the list
    // spans.
    for (raw, in_progress, complete) in [
        ("=x1", Some(vec![1u32]), Vec::new()),
        ("=x1,3!2", Some(vec![1u32, 3]), vec![2]),
        ("=x!2", None, vec![2]),
        ("=x0", Some(Vec::new()), Vec::new()),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroClose, "{raw}");
        let close = parse.pomodoro_close.as_ref().expect("close spec");
        assert_eq!(close.raw, raw, "{raw}");
        assert_eq!(close.in_progress, in_progress, "{raw}");
        assert_eq!(close.complete, complete, "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
    let selected = editor("=x1,3!2");
    assert_eq!(
        ranges(&selected),
        vec![
            (0, 2, SpanKind::PomodoroClose),
            (2, 5, SpanKind::PomodoroCloseInProgress),
            (5, 7, SpanKind::PomodoroCloseComplete),
        ],
        "=x1,3!2"
    );
    // A dangling separator is an editing state with the partial spec.
    let pending = editor("=x1,");
    assert_eq!(pending.mode, EditorMode::Incomplete, "=x1,");
    assert_eq!(pending.needs, vec![Need::PomodoroCloseTask], "=x1,");
    let partial = pending.pomodoro_close.as_ref().expect("partial spec");
    assert_eq!(partial.in_progress, Some(vec![1]), "=x1,");
    assert_eq!(
        ranges(&pending),
        vec![
            (0, 2, SpanKind::PomodoroClose),
            (2, 3, SpanKind::PomodoroCloseInProgress),
            (3, 4, SpanKind::InteractivePlaceholder),
        ],
        "=x1,"
    );
    assert!(pending.diagnostics.is_empty(), "=x1,");
    // A malformed selection reports `invalid_pomodoro_close` with no spec.
    let duplicate = editor("=x1,1");
    assert_eq!(duplicate.mode, EditorMode::PomodoroClose, "=x1,1");
    assert_eq!(codes(&duplicate), vec!["invalid_pomodoro_close"], "=x1,1");
    assert_eq!(duplicate.diagnostics[0].range, Some((4, 5)), "=x1,1");
    assert!(duplicate.pomodoro_close.is_none(), "=x1,1");
    // Near misses and conflicts report `invalid_pomodoro_close`.
    let shape = editor("=x more");
    assert_eq!(shape.mode, EditorMode::PomodoroClose, "=x more");
    assert_eq!(codes(&shape), vec!["invalid_pomodoro_close"], "=x more");
    assert_eq!(shape.diagnostics[0].range, Some((3, 7)), "=x more");
    let child = editor("=x\n- child");
    assert_eq!(child.mode, EditorMode::PomodoroClose, "=x child");
    assert_eq!(codes(&child), vec!["invalid_pomodoro_close"], "=x child");
    let named = editor("@r:id#n=x");
    assert_eq!(codes(&named), vec!["invalid_pomodoro_close"], "@r:id#n=x");
    assert_eq!(named.diagnostics[0].range, Some((5, 7)), "@r:id#n=x");
    let caret_named = editor("^r:id#n=x");
    assert_eq!(caret_named.mode, EditorMode::PomodoroLink, "^r:id#n=x");
    assert_eq!(
        codes(&caret_named),
        vec!["invalid_pomodoro_close"],
        "^r:id#n=x"
    );
    assert_eq!(caret_named.diagnostics[0].range, Some((5, 7)), "^r:id#n=x");
    let scheduled = editor("Text @r:id=x s:2");
    assert_eq!(
        codes(&scheduled),
        vec!["invalid_pomodoro_close"],
        "Text @r:id=x s:2"
    );
    assert_eq!(
        scheduled.diagnostics[0].range,
        Some((13, 16)),
        "Text @r:id=x s:2"
    );
    // Prose lookalikes stay ordinary tasks with no diagnostics.
    // `=3` is now a whole-item start, not prose, and `=x!` is a dangling
    // separator (an incomplete close, not prose).
    for raw in ["=xx", "=xa", "==", "Plan =x", "= foo", "=- foo"] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Task, "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
        assert!(parse.pomodoro_close.is_none(), "{raw}");
        assert!(parse.pomodoro_start.is_none(), "{raw}");
    }
    let dangling = editor("=x!");
    assert_eq!(dangling.mode, EditorMode::Incomplete, "=x!");
    assert_eq!(dangling.needs, vec![Need::PomodoroCloseTask], "=x!");
    assert!(dangling.diagnostics.is_empty(), "=x!");
    // A multi-item draft mixes an adjustment, a close, and a task.
    let mixed = parse_for_editor("+5\n\n=x\n\nCall bank @Cash+");
    assert_eq!(mixed.items.len(), 3, "mixed");
    assert_eq!(mixed.items[0].mode, EditorMode::PomodoroAdjust, "mixed");
    assert_eq!(mixed.items[1].mode, EditorMode::PomodoroClose, "mixed");
    assert!(mixed.items[1].pomodoro_close.is_some(), "mixed close spec");
}

#[test]
fn editor_reports_pomodoro_start_modes_spans_specs_and_diagnostics() {
    // Exact whole-item starts: mode, span, and additive spec.
    for (raw, suffix, duration, offset) in [
        ("=", "", 5, 0),
        ("=3", "3", 3, 0),
        ("=-", "-", 5, 1),
        ("=-2", "-2", 5, 2),
        ("=3-", "3-", 3, 1),
        ("=2-1", "2-1", 2, 1),
        ("=0", "0", 0, 0),
        ("=03", "03", 3, 0),
        ("  =3  ", "3", 3, 0),
    ] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroStart, "{raw}");
        assert_eq!(parse.body, raw.trim(), "{raw}");
        assert!(parse.needs.is_empty(), "{raw}");
        let start = parse.pomodoro_start.as_ref().expect("start spec");
        assert_eq!(start.raw, suffix, "{raw}");
        assert_eq!(start.duration_units, duration, "{raw}");
        assert_eq!(start.offset_units, offset, "{raw}");
        let token_start = raw.find('=').expect("=");
        assert!(
            ranges(&parse).contains(&(
                token_start,
                token_start + raw.trim_start().trim_end().len(),
                SpanKind::PomodoroStart
            )),
            "{raw}"
        );
        assert!(parse.diagnostics.is_empty(), "{raw}");
        assert!(parse.pomodoro_close.is_none(), "{raw}");
    }
    // Counted tokens with extra text report `invalid_pomodoro_start`
    // on the extra text, never a task.
    let shape = editor("=3 more");
    assert_eq!(shape.mode, EditorMode::PomodoroStart, "=3 more");
    assert_eq!(codes(&shape), vec!["invalid_pomodoro_start"], "=3 more");
    assert_eq!(shape.diagnostics[0].range, Some((3, 7)), "=3 more");
    assert!(shape.pomodoro_start.is_none(), "=3 more");
    let glued = editor("=3x");
    assert_eq!(codes(&glued), vec!["invalid_pomodoro_start"], "=3x");
    assert_eq!(glued.diagnostics[0].range, Some((2, 3)), "=3x");
    // Exact tokens with child lines report on the child line.
    let child = editor("=\n- child");
    assert_eq!(child.mode, EditorMode::PomodoroStart, "= child");
    assert_eq!(codes(&child), vec!["invalid_pomodoro_start"], "= child");
    assert!(child.pomodoro_start.is_none(), "= child");
    // Overflow reports on the token.
    let overflow = editor("=99999999999999999999999");
    assert_eq!(overflow.mode, EditorMode::PomodoroStart, "overflow");
    assert_eq!(codes(&overflow), vec!["invalid_pomodoro_start"], "overflow");
    assert_eq!(overflow.diagnostics[0].range, Some((0, 24)), "overflow");
    // Bare tokens with prose, close shapes, and mid-body tokens stay
    // ordinary tasks.
    for raw in ["= foo", "=- foo", "==", "=xx", "=xa", "Plan =3", "a=3"] {
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Task, "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
        assert!(parse.pomodoro_start.is_none(), "{raw}");
    }
    // A `@@` declaration never applies to start items.
    let declared = parse_for_editor("@@work\nFirst task\n\n=3\n");
    assert_eq!(declared.items[1].mode, EditorMode::PomodoroStart);
    assert!(declared.items[1].route.is_none());
    assert!(declared.items[1].pomodoro_start.is_some());
}

#[test]
fn interactive_markers_are_the_only_divergence_from_execution() {
    // `bob capture` keeps these literal because no route resolves; the
    // interactive grammar reports what the picker still owes instead.
    for raw in [
        "Body @",
        "Body @#",
        "Body @#Ideas",
        "Body @:",
        "Body @:focus-123",
    ] {
        let executed =
            parse_capture_text_with_clip_control(raw, None, None, true)
                .unwrap_or_else(|error| panic!("{raw}: {error}"));
        assert_eq!(executed.kind, CaptureKind::Task, "{raw}");
        assert_eq!(executed.route, None, "{raw}");
        assert_eq!(executed.body, raw, "{raw}");

        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
        assert_eq!(parse.body, "Body", "{raw}");
        assert_eq!(parse.needs.first(), Some(&Need::Route), "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }

    // These reach `bob capture`'s strict marker validation and fail it,
    // yet they are ordinary mid-typing states for an editor.
    for raw in [
        "Body @^",
        "Body @dev^",
        "Body @^focus-123",
        "Body @+",
        "Body @dev+",
        "Body @+focus-123",
        "Body @dev+id#",
        "Body @dev+#req",
        "Body @+#req",
        "Body @+#",
        "Body @dev:",
        "Body @dev:id#",
        "Body @dev:#bugs",
        "Body @!",
        "Body @!dev",
    ] {
        parse_capture_text_with_clip_control(raw, None, None, true)
            .expect_err(raw);

        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
        assert_eq!(parse.body, "Body", "{raw}");
        assert!(!parse.needs.is_empty(), "{raw}");
        assert!(parse.diagnostics.is_empty(), "{raw}");
    }
}

#[test]
fn pomodoro_start_suffix_reports_spec_and_non_overlapping_spans() {
    let parse = editor("Do work @sase:outline#deep=-2");
    assert_eq!(parse.mode, EditorMode::PomodoroTask);
    assert_eq!(parse.section.as_deref(), Some("deep"));
    assert_eq!(parse.block_id.as_deref(), Some("outline"));
    assert!(parse.diagnostics.is_empty());
    let start = parse.pomodoro_start.as_ref().expect("start spec");
    assert_eq!(start.raw, "-2");
    assert_eq!(start.duration_units, 5);
    assert_eq!(start.offset_units, 2);
    assert_eq!(
        ranges(&parse),
        vec![
            (8, 13, SpanKind::PomodoroRoute),
            (14, 21, SpanKind::PomodoroBlockId),
            (22, 26, SpanKind::PomodoroName),
            (26, 29, SpanKind::PomodoroStart),
        ]
    );

    let block_only = editor("Do work @sase:outline=3");
    let block_start = block_only.pomodoro_start.as_ref().expect("block start");
    assert_eq!(block_start.raw, "3");
    assert_eq!(block_start.duration_units, 3);
    assert_eq!(block_start.offset_units, 0);
    assert_eq!(
        ranges(&block_only),
        vec![
            (8, 13, SpanKind::PomodoroRoute),
            (14, 21, SpanKind::PomodoroBlockId),
            (21, 23, SpanKind::PomodoroStart),
        ]
    );

    let empty = editor("Do work @sase:outline=");
    let empty_start = empty.pomodoro_start.as_ref().expect("empty start");
    assert_eq!(empty_start.raw, "");
    assert_eq!(empty_start.duration_units, 5);
    assert_eq!(empty_start.offset_units, 0);

    // The suffix survives on incomplete markers too: the block ID is
    // still missing, but the typed start is already structured data.
    let incomplete = editor("Do work @sase:=3");
    assert_eq!(incomplete.mode, EditorMode::Incomplete);
    assert_eq!(incomplete.needs, vec![Need::PomodoroId]);
    let incomplete_start = incomplete
        .pomodoro_start
        .as_ref()
        .expect("incomplete start");
    assert_eq!(incomplete_start.raw, "3");
    assert!(incomplete.diagnostics.is_empty());

    let invalid = editor("Do work @sase:outline=abc");
    assert!(invalid.pomodoro_start.is_none());
    assert_eq!(codes(&invalid), vec!["invalid_pomodoro_start"]);

    let plain = editor("Do work @sase:outline");
    assert!(plain.pomodoro_start.is_none());
    assert!(!span_kinds(&plain).contains(&SpanKind::PomodoroStart));
}

#[test]
fn caret_close_conflicts_agree_with_execution() {
    use super::super::parse_capture_text_with_clip_control;
    // Invalid link closes with item conflicts report the conflict in both
    // execution and the editor: a conflict wins over the lexical diagnostic.
    for raw in ["^r:id=x1,1 s:2", "^r:id=x1, s:2"] {
        let execution =
            parse_capture_text_with_clip_control(raw, None, None, true)
                .expect_err(raw);
        let parse = editor(raw);
        assert_eq!(parse.mode, EditorMode::PomodoroLink, "{raw}");
        let diagnostic = parse.diagnostics.first().expect("diagnostic");
        assert_eq!(diagnostic.message, execution, "{raw}");
        assert!(
            execution.contains("cannot be combined with s:<N>"),
            "{raw}: {execution}"
        );
    }
    let child = "^r:id=x1,1\n- detail";
    let execution =
        parse_capture_text_with_clip_control(child, None, None, true)
            .expect_err(child);
    let parse = editor(child);
    let diagnostic = parse.diagnostics.first().expect("diagnostic");
    assert_eq!(diagnostic.message, execution, "{child}");
    assert!(
        execution.contains("authored child bullets"),
        "{child}: {execution}"
    );
}
