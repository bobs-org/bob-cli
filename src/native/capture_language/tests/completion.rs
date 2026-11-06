//! Cursor completion tests.

use super::super::completion::*;
use super::*;

#[test]
fn bare_at_completes_an_empty_route() {
    let completion = field("@", 1).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.route, None);
    assert_eq!(completion.query, "");
    assert_eq!(completion.replacement, (1, 1));
}

#[test]
fn leading_route_fragment_completes_with_no_body_yet() {
    // The lone `@ca` token never routes for `bob capture` (no body text
    // exists), but it is still the fragment a live editor completes.
    let completion = field("@ca", 3).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "ca");
    assert_eq!(completion.replacement, (1, 3));
}

#[test]
fn cursor_mid_route_fragment_uses_the_prefix_before_the_cursor() {
    let completion = field("@cash buy milk", 2).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "c");
    // The whole fragment is replaced regardless of where the cursor sits.
    assert_eq!(completion.replacement, (1, 5));
}

#[test]
fn missing_route_portion_of_bullet_marker_completes_a_route() {
    let completion = field("Idea @#Ideas", 6).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "");
    assert_eq!(completion.replacement, (6, 6));
}

#[test]
fn missing_route_portion_of_pomodoro_marker_completes_a_route() {
    let completion = field("@:focus-123", 1).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.replacement, (1, 1));
}

#[test]
fn missing_route_portion_of_task_block_id_marker_completes_a_route() {
    let completion = field("@^focus-123", 1).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.replacement, (1, 1));
}

#[test]
fn missing_route_portion_of_sub_bullet_marker_completes_a_route() {
    let completion = field("@+focus-123", 1).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.replacement, (1, 1));
}

#[test]
fn section_completes_after_a_resolved_route() {
    let completion = field("Idea @notes#Id", 14).expect("section field");
    assert_eq!(completion.context, CompletionContext::Section);
    assert_eq!(completion.route.as_deref(), Some("notes"));
    assert_eq!(completion.query, "Id");
    assert_eq!(completion.replacement, (12, 14));
}

#[test]
fn quoted_section_literal_decodes_its_query_and_replaces_the_quotes() {
    let hash = "Idea @sase".len();

    let partial = "Idea @sase#\"Fu";
    let field_partial = field(partial, partial.len()).expect("partial");
    assert_eq!(field_partial.context, CompletionContext::Section);
    assert_eq!(field_partial.route.as_deref(), Some("sase"));
    assert_eq!(field_partial.query, "Fu");
    assert_eq!(field_partial.replacement, (hash + 1, partial.len()));

    let closed = "Idea @sase#\"Future Work\"";
    let field_closed = field(closed, closed.len()).expect("closed");
    assert_eq!(field_closed.query, "Future Work");
    assert_eq!(field_closed.replacement, (hash + 1, closed.len()));

    let inside = field(closed, hash + 3).expect("inside");
    assert_eq!(inside.query, "F");
    assert_eq!(inside.replacement, (hash + 1, closed.len()));
}

#[test]
fn list_header_completes_section_after_bullets() {
    let raw = "@sase#Fu\n- boom";
    let cursor = "@sase#Fu".len();
    let completion = field(raw, cursor).expect("header section");
    assert_eq!(completion.context, CompletionContext::Section);
    assert_eq!(completion.route.as_deref(), Some("sase"));
    assert_eq!(completion.query, "Fu");
    assert_eq!(completion.replacement, (6, cursor));
}

#[test]
fn list_header_completes_route_after_bullets() {
    let raw = "@sa\n- boom";
    let completion = field(raw, 3).expect("header route");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "sa");
    assert_eq!(completion.replacement, (1, 3));
}

#[test]
fn list_header_completes_with_crlf_and_utf8_before_it() {
    let raw = "🙂\n@sase#Fu\r\n- boom";
    let header = raw.find("@sase#Fu").expect("header");
    let cursor = header + "@sase#Fu".len();
    let completion = field(raw, cursor).expect("utf8 header");
    assert_eq!(completion.context, CompletionContext::Section);
    assert_eq!(completion.route.as_deref(), Some("sase"));
    assert_eq!(completion.query, "Fu");
    assert_eq!(completion.replacement, (header + 6, cursor));
}

#[test]
fn list_header_completes_partial_quoted_section_after_bullets() {
    let raw = "@sase#\"Fu\n- boom";
    let cursor = "@sase#\"Fu".len();
    let completion = field(raw, cursor).expect("quoted header");
    assert_eq!(completion.context, CompletionContext::Section);
    assert_eq!(completion.query, "Fu");
    assert_eq!(completion.replacement, (6, cursor));
}

#[test]
fn pomodoro_block_id_completes_after_a_resolved_route() {
    let completion = field("Do work @Dev:foc", 16).expect("pomodoro id");
    assert_eq!(completion.context, CompletionContext::PomodoroBlockId);
    assert_eq!(completion.route.as_deref(), Some("dev"));
    assert_eq!(completion.query, "foc");
    assert_eq!(completion.replacement, (13, 16));
}

#[test]
fn pomodoro_name_completes_after_hash_even_without_a_block_id() {
    let raw = "Do work @Dev:id#bu";
    let hash = raw.find('#').expect("hash");
    let completion = field(raw, raw.len()).expect("pomodoro name");
    assert_eq!(completion.context, CompletionContext::PomodoroName);
    assert_eq!(completion.route.as_deref(), Some("dev"));
    assert_eq!(completion.block_id.as_deref(), Some("id"));
    assert_eq!(completion.query, "bu");
    assert_eq!(completion.replacement, (hash + 1, raw.len()));

    let empty = "Do work @Dev:id#";
    let empty_hash = empty.find('#').expect("hash");
    let empty_field = field(empty, empty.len()).expect("empty name");
    assert_eq!(empty_field.context, CompletionContext::PomodoroName);
    assert_eq!(empty_field.query, "");
    assert_eq!(empty_field.replacement, (empty_hash + 1, empty_hash + 1));

    let no_id = "a @dev:#bu";
    let no_id_hash = no_id.find('#').expect("hash");
    let no_id_field = field(no_id, no_id.len()).expect("name without id");
    assert_eq!(no_id_field.context, CompletionContext::PomodoroName);
    assert_eq!(no_id_field.route.as_deref(), Some("dev"));
    assert_eq!(no_id_field.block_id, None);
    assert_eq!(no_id_field.query, "bu");
    assert_eq!(no_id_field.replacement, (no_id_hash + 1, no_id.len()));
}

#[test]
fn pomodoro_name_completion_keeps_route_and_id_contexts() {
    let raw = "Do work @Dev:id#bu";
    let at = raw.find('@').expect("at");
    let colon = raw.find(':').expect("colon");
    let hash = raw.find('#').expect("hash");

    let route = field(raw, at + 3).expect("route");
    assert_eq!(route.context, CompletionContext::Route);
    assert_eq!(route.replacement, (at + 1, colon));

    let id = field(raw, colon + 2).expect("block id");
    assert_eq!(id.context, CompletionContext::PomodoroBlockId);
    assert_eq!(id.replacement, (colon + 1, hash));
    assert_eq!(&raw[colon + 1..hash], "id");

    let on_hash = field(raw, hash).expect("cursor on hash stays id");
    assert_eq!(on_hash.context, CompletionContext::PomodoroBlockId);
    assert_eq!(on_hash.replacement, (colon + 1, hash));
}

#[test]
fn pomodoro_completion_ranges_end_before_the_start_suffix() {
    let raw = "Do work @sase:outline#dee=-2";
    let hash = raw.find('#').expect("hash");
    let suffix_start = raw.find('=').expect("suffix");
    // A cursor inside the name completes just the name: accepting the
    // candidate replaces `dee` and leaves `=-2` in place.
    let name = field(raw, hash + 2).expect("pomodoro name");
    assert_eq!(name.context, CompletionContext::PomodoroName);
    assert_eq!(name.route.as_deref(), Some("sase"));
    assert_eq!(name.block_id.as_deref(), Some("outline"));
    assert_eq!(name.query, "d");
    assert_eq!(name.replacement, (hash + 1, suffix_start));
    assert_eq!(&raw[name.replacement.0..name.replacement.1], "dee");

    // A cursor on the `=` boundary still completes the name.
    let boundary = field(raw, suffix_start).expect("boundary name");
    assert_eq!(boundary.context, CompletionContext::PomodoroName);
    assert_eq!(boundary.replacement, (hash + 1, suffix_start));

    // A cursor inside the suffix offers no completion at all.
    assert_eq!(field(raw, suffix_start + 1), None);
    assert_eq!(field(raw, raw.len()), None);

    let block_raw = "Do work @sase:out=3";
    let block_suffix = block_raw.find('=').expect("suffix");
    let block = field(block_raw, block_suffix).expect("block id");
    assert_eq!(block.context, CompletionContext::PomodoroBlockId);
    assert_eq!(block.query, "out");
    assert_eq!(block.replacement, (14, block_suffix));
    assert_eq!(field(block_raw, block_raw.len()), None);
}

#[test]
fn pomodoro_link_suffix_names_complete_without_replacing_the_link_start() {
    for raw in ["@work:outline=bu", "^work:outline=bu"] {
        let completion = field(raw, raw.len()).expect("suffix Pomodoro name");
        assert_eq!(
            completion.context,
            CompletionContext::PomodoroName,
            "{raw}"
        );
        assert_eq!(completion.route.as_deref(), Some("work"), "{raw}");
        assert_eq!(completion.block_id.as_deref(), Some("outline"), "{raw}");
        assert_eq!(completion.query, "bu", "{raw}");
        assert_eq!(completion.replacement, (raw.len() - 2, raw.len()), "{raw}");
        assert!(completion.bare_name, "{raw}");
    }

    let timed = "@work:outline=bu3";
    let name_end = timed.find('3').expect("timing suffix");
    let completion = field(timed, name_end).expect("name before timing");
    assert_eq!(completion.context, CompletionContext::PomodoroName);
    assert_eq!(completion.query, "bu");
    assert_eq!(
        completion.replacement,
        (timed.find("bu").expect("name query"), name_end)
    );

    let hash = "^work:outline=3#bu";
    let completion = field(hash, hash.len()).expect("hash suffix name");
    assert_eq!(completion.context, CompletionContext::PomodoroName);
    assert_eq!(completion.query, "bu");
    assert_eq!(
        completion.replacement,
        (hash.find('#').unwrap(), hash.len())
    );
    assert!(completion.bare_name);

    let empty = "^work:outline=3#";
    let completion = field(empty, empty.len()).expect("empty suffix name");
    assert_eq!(completion.context, CompletionContext::PomodoroName);
    assert_eq!(completion.query, "");
    assert_eq!(completion.replacement, (empty.len() - 1, empty.len()));
    assert!(completion.bare_name);
}

#[test]
fn caret_suffix_names_preserve_the_link_part_for_completion() {
    // Route/block cursors keep `active_task` with a replacement ending
    // before `=`, even with a suffix name present.
    for raw in ["^work:outline=bugs", "^work:outline=3#bugs"] {
        for cursor in [4, 10] {
            let completion = field(raw, cursor).expect(raw);
            assert_eq!(
                completion.context,
                CompletionContext::ActiveTask,
                "{raw}@{cursor}"
            );
            assert_eq!(completion.replacement, (1, 13), "{raw}@{cursor}");
        }
    }
    // Suffix names still replace only the name.
    let raw = "^work:outline=bugs";
    let completion = field(raw, 16).expect("suffix name");
    assert_eq!(completion.context, CompletionContext::PomodoroName);
    assert_eq!(completion.replacement, (14, 18));
    // Timing bytes stay non-completable.
    for (raw, cursor) in [
        ("^work:outline=3#bugs", 14),
        ("^work:outline=3#bugs", 15),
        ("^work:outline=bugs3", 19),
    ] {
        assert_eq!(field(raw, cursor), None, "{raw}@{cursor}");
    }
}

#[test]
fn task_block_id_route_and_authored_id_both_complete() {
    let route = field("Do work @Dev^new-id", 12).expect("route field");
    assert_eq!(route.context, CompletionContext::Route);
    assert_eq!(route.query, "Dev");
    assert_eq!(route.replacement, (9, 12));

    let id = field("Do work @Dev^new-id", 13).expect("task block id");
    assert_eq!(id.context, CompletionContext::TaskBlockId);
    assert_eq!(id.route.as_deref(), Some("dev"));
    assert_eq!(id.query, "");
    assert_eq!(id.replacement, (13, 19));

    let end = field("Do work @Dev^new-id", 19).expect("task block id end");
    assert_eq!(end.context, CompletionContext::TaskBlockId);
    assert_eq!(end.query, "new-id");
    assert_eq!(end.replacement, (13, 19));
}

#[test]
fn block_id_project_note_sigil_is_excluded_from_replacement() {
    // The retired `:` project-note form offers no completion.
    assert_eq!(field("@sase:x+", 7), None);
    assert_eq!(field("@sase:x+", 8), None);
    assert_eq!(field("@cash:goog-exit+#bugs", 10), None);

    let caret = field("@sase^x@", 7).expect("caret block id");
    assert_eq!(caret.context, CompletionContext::TaskBlockId);
    assert_eq!(caret.query, "x");
    assert_eq!(caret.replacement, (6, 7));
    assert_eq!(field("@sase^x@", 8), None);

    // Either sealed separator reports the project-note intent with the
    // seal excluded from the replacement; a completed seal offers no
    // picker, while a trailing `#` opens name completion.
    let offer = field("@sase:x@", 7).expect("offer block id");
    assert_eq!(offer.context, CompletionContext::PomodoroBlockId);
    assert_eq!(offer.route.as_deref(), Some("sase"));
    assert_eq!(offer.query, "x");
    assert_eq!(offer.replacement, (6, 7));
    assert_eq!(field("@sase:x@", 8), None);

    let named = field("@cash:goog-exit@#bugs", 10).expect("offer block id");
    assert_eq!(named.context, CompletionContext::PomodoroBlockId);
    assert_eq!(named.route.as_deref(), Some("cash"));
    assert_eq!(named.query, "goog");
    assert_eq!(named.replacement, (6, 15));

    let name = field("@cash:goog-exit@#bu", 19).expect("offer name");
    assert_eq!(name.context, CompletionContext::PomodoroName);
    assert_eq!(name.route.as_deref(), Some("cash"));
    assert_eq!(name.block_id.as_deref(), Some("goog-exit"));
    assert_eq!(name.query, "bu");
    assert_eq!(name.replacement, (17, 19));

    let named = field("@cash^goog-exit@#bugs", 10).expect("caret block id");
    assert_eq!(named.context, CompletionContext::TaskBlockId);
    assert_eq!(named.query, "goog");
    // Plus is excluded: replacement ends before the sigil.
    assert_eq!(named.replacement, (6, 15));

    // Inside the `#name` the context is the Pomodoro name and the
    // replacement is just the name.
    let name = field("@cash^goog-exit@#bu", 19).expect("pomodoro name");
    assert_eq!(name.context, CompletionContext::PomodoroName);
    assert_eq!(name.route.as_deref(), Some("cash"));
    assert_eq!(name.block_id.as_deref(), Some("goog-exit"));
    assert_eq!(name.query, "bu");
    assert_eq!(name.replacement, (17, 19));
}

#[test]
fn legacy_pomodoro_alias_completes_the_same_as_the_canonical_form() {
    let completion = field("Do work @!Dev:foc", 17).expect("pomodoro id");
    assert_eq!(completion.context, CompletionContext::PomodoroBlockId);
    assert_eq!(completion.route.as_deref(), Some("dev"));
    assert_eq!(completion.query, "foc");
    assert_eq!(completion.replacement, (14, 17));
}

#[test]
fn task_completes_after_a_resolved_sub_bullet_route() {
    let completion = field("note @Cash+goog", 15).expect("task field");
    assert_eq!(completion.context, CompletionContext::Task);
    assert_eq!(completion.route.as_deref(), Some("cash"));
    assert_eq!(completion.query, "goog");
    assert_eq!(completion.replacement, (11, 15));
}

#[test]
fn right_component_without_a_resolved_route_has_no_completion() {
    assert_eq!(field("@:foc", 5), None);
    assert_eq!(field("@^foc", 5), None);
    assert_eq!(field("@+foc", 5), None);
}

#[test]
fn cursor_in_body_text_has_no_completion() {
    assert_eq!(field("buy milk @groceries", 4), None);
}

#[test]
fn cursor_on_a_middle_token_has_no_completion() {
    // `@home` here stays literal body text because the leading `@work`
    // marker wins, exactly like `parse_for_editor` reports it.
    assert_eq!(field("@work buy milk @home", 17), None);
    assert!(field("@work buy milk @home", 0).is_some());
}

#[test]
fn cursor_past_a_trailing_space_has_no_completion() {
    assert_eq!(field("Body @dev ", 10), None);
}

#[test]
fn free_standing_hash_fragments_request_task_type_completion() {
    for (raw, cursor, query) in [
        ("Fix it #n", 9, "n"),
        ("Fix it @sase #no", 15, "n"),
        ("Fix it #now", 11, "now"),
        ("Fix #", 5, ""),
    ] {
        let completion = field(raw, cursor).expect(raw);
        assert_eq!(completion.context, CompletionContext::TaskType, "{raw}");
        assert_eq!(completion.query, query, "{raw}");
    }

    let bare = field("Fix #", 5).expect("bare edge hash");
    assert_eq!(bare.replacement, (4, 5));
}

#[test]
fn hash_completion_is_lexical_even_when_the_subject_will_refuse_it() {
    for raw in ["@r:id #n", "#n", "#now"] {
        let completion = field(raw, raw.len()).expect(raw);
        assert_eq!(completion.context, CompletionContext::TaskType, "{raw}");
    }
}

#[test]
fn work_log_bullet_lines_request_no_marker_completion() {
    // Bullet text is literal: `@`, `@@`, and task pickers stay suppressed
    // on bullet lines, on a plain close and on a chain close alike.
    for raw in ["x\n- 1 @", "x\n- 1 @@", "x =\n- 1 @", "x\n- @"] {
        assert_eq!(field(raw, raw.len()), None, "{raw}");
    }
    // The parent close line itself also requests nothing.
    assert_eq!(field("x\n- 1 wired", 1), None);
}

#[test]
fn retired_double_colon_marker_has_no_completion_field() {
    assert_eq!(field("Do work @Dev::new-id", 12), None);
    assert_eq!(field("Do work @Dev::new-id", 14), None);
    assert_eq!(field("Do work @Dev::new-id", 20), None);
    assert_eq!(field("@::focus-123", 1), None);
}

#[test]
fn invalid_block_id_characters_still_produce_a_field() {
    // The field extractor never validates block-ID syntax; a discovery
    // scan naturally returns no candidates for a query no real block ID
    // could match, without a separate invalid/error path here.
    let completion = field("note @dev+bad.id", 16).expect("task field");
    assert_eq!(completion.context, CompletionContext::Task);
    assert_eq!(completion.route.as_deref(), Some("dev"));
    assert_eq!(completion.query, "bad.id");
}

#[test]
fn terminal_markers_do_not_interfere_with_route_completion() {
    let completion = field("body $2 s:1 %c @ca", 18).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "ca");
    assert_eq!(completion.replacement, (16, 18));
}

#[test]
fn target_run_completion_crosses_transparent_markers_with_source_offsets() {
    struct Case {
        draft: &'static str,
        context: CompletionContext,
        route: Option<&'static str>,
        block_id: Option<&'static str>,
        query: &'static str,
        component: &'static str,
    }

    let cases = [
        Case {
            draft: "@work+a s:3 @dev+| note",
            context: CompletionContext::Task,
            route: Some("dev"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "s:3 @work+a @dev+b#re| note",
            context: CompletionContext::Drawer,
            route: Some("dev"),
            block_id: Some("b"),
            query: "re",
            component: "#re",
        },
        Case {
            draft: "@work+a $1 @dev+| note",
            context: CompletionContext::Task,
            route: Some("dev"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "@work+a %c @dev+| note",
            context: CompletionContext::Task,
            route: Some("dev"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "@work+a #fix @dev+| note",
            context: CompletionContext::Task,
            route: Some("dev"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "@work+a &foo:bar @dev+| note",
            context: CompletionContext::Task,
            route: Some("dev"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "@work+a *foo:bar @dev+| note",
            context: CompletionContext::Task,
            route: Some("dev"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "@work+a ~foo:bar @dev+| note",
            context: CompletionContext::Task,
            route: Some("dev"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "@work+a %c @de| note",
            context: CompletionContext::Route,
            route: None,
            block_id: None,
            query: "de",
            component: "de",
        },
        Case {
            draft: "note s:3 @work+| @dev+b",
            context: CompletionContext::Task,
            route: Some("work"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "note @work+| %c @dev+b",
            context: CompletionContext::Task,
            route: Some("work"),
            block_id: None,
            query: "",
            component: "",
        },
        Case {
            draft: "@work+a %c @dev+b @notes+c#req| note",
            context: CompletionContext::Drawer,
            route: Some("notes"),
            block_id: Some("c"),
            query: "req",
            component: "#req",
        },
        Case {
            draft: "Parent\n- First child\n- Child note @work+a s:3 @dev+|",
            context: CompletionContext::Task,
            route: Some("dev"),
            block_id: None,
            query: "",
            component: "",
        },
    ];

    for case in cases {
        let cursor = case.draft.find('|').expect("cursor marker");
        let raw = case.draft.replace('|', "");
        let completion =
            field(&raw, cursor).unwrap_or_else(|| panic!("{raw:?}"));
        let expected_start = if case.component.is_empty() {
            cursor
        } else {
            raw.find(case.component).expect("component start")
        };
        assert_eq!(completion.context, case.context, "{raw:?}");
        assert_eq!(completion.route.as_deref(), case.route, "{raw:?}");
        assert_eq!(completion.block_id.as_deref(), case.block_id, "{raw:?}");
        assert_eq!(completion.query, case.query, "{raw:?}");
        assert_eq!(
            completion.replacement,
            (expected_start, expected_start + case.component.len()),
            "{raw:?}"
        );
        assert_eq!(
            &raw[completion.replacement.0..completion.replacement.1],
            case.component,
            "{raw:?}"
        );
    }

    let raw = "🌱 earlier\n\n@work+a %c @de note";
    let marker = raw.find("@de").expect("later item marker");
    let completion = field(raw, marker + 3).expect("later item route");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "de");
    assert_eq!(completion.replacement, (marker + 1, marker + 3));
    assert_eq!(
        &raw[completion.replacement.0..completion.replacement.1],
        "de"
    );
}

#[test]
fn completion_field_stays_on_unicode_scalar_boundaries() {
    let raw = "caf\u{e9} \u{1f680} @Cash+goog-exit";
    for cursor in (0..=raw.len()).filter(|&index| raw.is_char_boundary(index)) {
        let _ = field(raw, cursor);
    }
}

#[test]
fn completion_field_uses_byte_offsets_after_multibyte_prefix_text() {
    let raw = "caf\u{e9} \u{1f680} @Cash+goog-exit";
    // "@Cash+goog-exit" starts at byte 11, right after the multibyte
    // café and rocket-emoji body text.
    assert_eq!(&raw[11..16], "@Cash");
    assert_eq!(&raw[17..26], "goog-exit");

    let route = field(raw, 15).expect("route field");
    assert_eq!(route.context, CompletionContext::Route);
    assert_eq!(route.query, "Cas");
    assert_eq!(route.replacement, (12, 16));

    let task = field(raw, 21).expect("task field");
    assert_eq!(task.context, CompletionContext::Task);
    assert_eq!(task.route.as_deref(), Some("cash"));
    assert_eq!(task.query, "goog");
    assert_eq!(task.replacement, (17, 26));
    assert_eq!(task.block_id, None);
}

#[test]
fn drawer_completes_after_hash_on_a_sub_bullet_marker() {
    let raw = "note @Cash+goog#req";
    let hash = raw.find('#').expect("hash");
    let completion = field(raw, raw.len()).expect("drawer");
    assert_eq!(completion.context, CompletionContext::Drawer);
    assert_eq!(completion.route.as_deref(), Some("cash"));
    assert_eq!(completion.block_id.as_deref(), Some("goog"));
    assert_eq!(completion.query, "req");
    assert_eq!(completion.replacement, (hash, raw.len()));
}

#[test]
fn empty_selector_after_hash_is_a_one_byte_drawer_field() {
    let raw = "note @Cash+goog#";
    let completion = field(raw, raw.len()).expect("drawer");
    assert_eq!(completion.context, CompletionContext::Drawer);
    assert_eq!(completion.route.as_deref(), Some("cash"));
    assert_eq!(completion.block_id.as_deref(), Some("goog"));
    assert_eq!(completion.query, "");
    assert_eq!(completion.replacement, (raw.len() - 1, raw.len()));
}

#[test]
fn hash_after_a_bare_block_id_marker_completes_a_task_type() {
    // The `@route+` route side and the task-picker side are unchanged --
    // still `Route` and `Task` -- while the glued suffix is always a drawer;
    // the separate `#type` form still completes a task type.
    let raw = "@Cash+goog#bu";
    let at = raw.find('@').expect("at");
    let plus = raw.find('+').expect("plus");

    let route = field(raw, at + 3).expect("route field");
    assert_eq!(route.context, CompletionContext::Route);

    let task = field(raw, plus + 2).expect("task field");
    assert_eq!(task.context, CompletionContext::Task);

    let drawer = field(raw, raw.len()).expect("drawer field");
    assert_eq!(drawer.context, CompletionContext::Drawer);

    let spaced = "@Cash+goog #bu";
    let name = field(spaced, spaced.len()).expect("task type field");
    assert_eq!(name.context, CompletionContext::TaskType);
    assert_eq!(name.query, "bu");

    // Once the item has body text, the same glued `#` keeps its drawer
    // meaning -- unaffected by the toggle rule.
    let with_body = "note @Cash+goog#bu";
    let section = field(with_body, with_body.len()).expect("drawer");
    assert_eq!(section.context, CompletionContext::Drawer);
}

#[test]
fn explicit_toggle_task_completion_replacement_ends_before_the_bang() {
    let raw = "@Cash+goog-exit!";
    let plus = raw.find('+').expect("plus");
    let bang = raw.find('!').expect("bang");

    let task = field(raw, plus + 5).expect("task field inside id");
    assert_eq!(task.context, CompletionContext::Task);
    assert_eq!(task.route.as_deref(), Some("cash"));
    assert_eq!(task.query, "goog");
    assert_eq!(task.replacement, (plus + 1, bang));
    assert_eq!(&raw[plus + 1..bang], "goog-exit");

    let at_end_of_id = field(raw, bang).expect("cursor at end of id");
    assert_eq!(at_end_of_id.context, CompletionContext::Task);
    assert_eq!(at_end_of_id.replacement, (plus + 1, bang));

    assert_eq!(field(raw, raw.len()), None);
    assert_eq!(field(raw, bang + 1), None);

    let café = "café @Cash+id!";
    let café_plus = café.find('+').expect("plus");
    let café_bang = café.find('!').expect("bang");
    let café_task = field(café, café_plus + 2).expect("utf-8 task");
    assert_eq!(café_task.replacement, (café_plus + 1, café_bang));
    assert!(café.is_char_boundary(café_task.replacement.0));
    assert!(café.is_char_boundary(café_task.replacement.1));
}

#[test]
fn cursor_in_route_or_block_id_of_three_component_marker_keeps_existing_contexts(
) {
    let raw = "note @Cash+goog#req";
    let at = raw.find('@').expect("at");
    let plus = raw.find('+').expect("plus");
    let hash = raw.find('#').expect("hash");

    let route = field(raw, at + 3).expect("route");
    assert_eq!(route.context, CompletionContext::Route);
    assert_eq!(route.block_id, None);
    assert_eq!(route.replacement, (at + 1, plus));

    let task = field(raw, plus + 3).expect("task");
    assert_eq!(task.context, CompletionContext::Task);
    assert_eq!(task.block_id, None);
    assert_eq!(task.query, "go");
    assert_eq!(task.replacement, (plus + 1, hash));
    assert_eq!(&raw[plus + 1..hash], "goog");
}

#[test]
fn hash_ends_the_block_id_and_opens_the_drawer_replacement() {
    let raw = "note @Cash+goog#";
    let plus = raw.find('+').expect("plus");
    let hash = raw.find('#').expect("hash");
    let task = field(raw, hash).expect("cursor on hash stays task");
    assert_eq!(task.context, CompletionContext::Task);
    assert_eq!(task.replacement, (plus + 1, hash));

    let section = field(raw, hash + 1).expect("cursor after hash");
    assert_eq!(section.context, CompletionContext::Drawer);
    assert_eq!(section.replacement, (hash, hash + 1));
}

#[test]
fn empty_block_id_with_section_still_yields_a_drawer_field() {
    let raw = "note @Cash+#req";
    let completion = field(raw, raw.len()).expect("drawer");
    assert_eq!(completion.context, CompletionContext::Drawer);
    assert_eq!(completion.route.as_deref(), Some("cash"));
    assert_eq!(completion.block_id, None);
    assert_eq!(completion.query, "req");
}

#[test]
fn quoted_drawer_literal_decodes_its_query_and_replaces_the_quotes() {
    let hash = "note @Cash+goog".len();

    // A partial literal queries its decoded text; the replacement still
    // covers the `#` and the whole partial literal.
    let partial = "note @Cash+goog#\"FUTURE W";
    let field_partial = field(partial, partial.len()).expect("partial");
    assert_eq!(field_partial.context, CompletionContext::Drawer);
    assert_eq!(field_partial.query, "FUTURE W");
    assert_eq!(field_partial.replacement, (hash, partial.len()));

    // A closed literal queries the whole name, never its closing quote.
    let closed = "note @Cash+goog#\"Q&A\"";
    let field_closed = field(closed, closed.len()).expect("closed");
    assert_eq!(field_closed.query, "Q&A");
    assert_eq!(field_closed.replacement, (hash, closed.len()));

    // A cursor inside the closed literal queries the text before it.
    let inside = field(closed, hash + 3).expect("inside");
    assert_eq!(inside.query, "Q");
    assert_eq!(inside.replacement, (hash, closed.len()));
}

#[test]
fn three_component_right_side_without_a_resolved_route_has_no_completion() {
    assert_eq!(field("@+#req", 6), None);
    assert_eq!(field("@+id#req", 8), None);
}

#[test]
fn completion_field_stays_on_boundaries_of_a_three_component_marker() {
    let raw = "caf\u{e9} \u{1f680} @Cash+goog-exit#req";
    for cursor in (0..=raw.len()).filter(|&index| raw.is_char_boundary(index)) {
        let _ = field(raw, cursor);
    }
    let hash = raw.find('#').expect("hash");
    let section = field(raw, raw.len()).expect("drawer");
    assert_eq!(section.context, CompletionContext::Drawer);
    assert_eq!(section.block_id.as_deref(), Some("goog-exit"));
    assert_eq!(section.replacement, (hash, raw.len()));
}

#[test]
fn leading_three_component_marker_completes_each_component() {
    let raw = "@Cash+goog#req body";
    let plus = raw.find('+').expect("plus");
    let hash = raw.find('#').expect("hash");
    let space = raw.find(' ').expect("space");

    let route = field(raw, 3).expect("route");
    assert_eq!(route.context, CompletionContext::Route);
    assert_eq!(route.replacement, (1, plus));

    let task = field(raw, plus + 2).expect("task");
    assert_eq!(task.context, CompletionContext::Task);
    assert_eq!(task.replacement, (plus + 1, hash));

    let section = field(raw, hash + 2).expect("section");
    assert_eq!(section.context, CompletionContext::Drawer);
    assert_eq!(section.replacement, (hash, space));
    assert_eq!(section.query, "r");
}

#[test]
fn completion_on_a_child_line_completes_a_trailing_route() {
    let raw = "parent line\n- context @ca";
    let completion = field(raw, raw.len()).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "ca");
    let at_index = raw.rfind('@').expect("at sign");
    assert_eq!(completion.replacement, (at_index + 1, raw.len()));
}

#[test]
fn completion_on_a_nested_child_line_completes_a_trailing_route() {
    let raw = "parent line\n- first child\n  - context @ca";
    let completion = field(raw, raw.len()).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "ca");
    let at_index = raw.rfind('@').expect("at sign");
    assert_eq!(completion.replacement, (at_index + 1, raw.len()));
}

#[test]
fn completion_on_nested_prefix_or_orphaned_nested_line_is_empty() {
    let raw = "parent line\n- first child\n  - context @ca";
    let nested_line_start = raw.rfind("  -").expect("nested line");
    assert_eq!(field(raw, nested_line_start), None);
    assert_eq!(field(raw, nested_line_start + 1), None);
    assert_eq!(field(raw, nested_line_start + 3), None);

    let orphan = "parent line\n  - context @ca";
    assert_eq!(field(orphan, orphan.len()), None);
}

#[test]
fn completion_works_on_an_earlier_child_line_not_only_the_last() {
    // A marker on the *first* child line, with more lines after it,
    // still completes -- completion is scoped per physical line, not
    // just to the leading/trailing ends of the whole draft.
    let raw = "parent line\n- first @ca\n- second child\n- third child";
    let at_index = raw.find('@').expect("at sign");
    let cursor = at_index + 3;
    let completion = field(raw, cursor).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "ca");
    let line_end = raw.find("\n- second").expect("line end");
    assert_eq!(completion.replacement, (at_index + 1, line_end));
}

#[test]
fn completion_inside_a_child_bullet_marker_has_no_completion() {
    let raw = "parent\n- @work";
    // Cursor sitting inside the "- " marker itself, before the body.
    let dash_index = raw.rfind("- ").expect("marker");
    assert_eq!(field(raw, dash_index), None);
    assert_eq!(field(raw, dash_index + 1), None);
}

#[test]
fn completion_on_the_parent_line_still_supports_leading_markers() {
    let raw = "@ca parent\n- child";
    let completion = field(raw, 3).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "ca");
    assert_eq!(completion.replacement, (1, 3));
}

#[test]
fn completion_on_a_child_line_never_offers_a_leading_route() {
    // On the parent line a lone `@ca` fragment still completes (see
    // `leading_route_fragment_completes_with_no_body_yet`), because the
    // first line keeps the established leading-route form. A child line
    // never gets that treatment, so the identical lone fragment here is
    // not completable at all.
    let raw = "parent\n- @ca";
    assert_eq!(field(raw, raw.len()), None);
}

#[test]
fn completion_on_global_declarations_only_completes_the_route() {
    let route = field("@@fo", 4).expect("route");
    assert_eq!(route.context, CompletionContext::Route);
    assert_eq!(route.query, "fo");
    assert_eq!(route.replacement, (2, 4));

    let raw = "@@Cash+goog\nnote";
    let plus = raw.find('+').expect("plus");
    assert_eq!(field(raw, plus + 2), None);
}

#[test]
fn completion_inside_an_item_stays_item_local_with_a_global_declaration() {
    let raw = "@@foo\nFirst @ba";
    let at = raw.rfind('@').expect("local at");
    let completion = field(raw, raw.len()).expect("local route");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.replacement, (at + 1, raw.len()));
    assert_eq!(field(raw, 3).unwrap().context, CompletionContext::Route);
    assert_eq!(field(raw, 3).unwrap().replacement, (2, 5));
}

#[test]
fn project_task_block_id_completes_a_lone_colon() {
    let raw = "Finish it @cash^goog-exit@\n- Draft the memo :";
    let completion = field(raw, raw.len()).expect("project task field");
    assert_eq!(completion.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(completion.route.as_deref(), Some("cash_goog_exit"));
    assert_eq!(completion.query, "");
    assert_eq!(completion.replacement, (raw.len(), raw.len()));
}

#[test]
fn project_task_block_id_completes_a_partial_id() {
    let raw = "Finish it @cash^goog-exit@\n- Draft the memo :dr";
    let completion = field(raw, raw.len()).expect("project task field");
    assert_eq!(completion.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(completion.route.as_deref(), Some("cash_goog_exit"));
    assert_eq!(completion.query, "dr");
    let id_start = raw.rfind(":dr").expect("id") + 1;
    assert_eq!(completion.replacement, (id_start, raw.len()));
    // A mid-ID cursor keeps the whole-ID replacement with the typed prefix.
    let mid = field(raw, raw.len() - 1).expect("mid-id field");
    assert_eq!(mid.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(mid.query, "d");
    assert_eq!(mid.replacement, (id_start, raw.len()));
}

#[test]
fn project_task_block_id_completes_a_caret_id() {
    let raw = "Finish it @cash^goog-exit@\n- Collect the equity paperwork ^equ";
    let completion = field(raw, raw.len()).expect("project task field");
    assert_eq!(completion.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(completion.route.as_deref(), Some("cash_goog_exit"));
    assert_eq!(completion.query, "equ");
    let id_start = raw.rfind("^equ").expect("id") + 1;
    assert_eq!(completion.replacement, (id_start, raw.len()));

    let lone = "Finish it @cash^goog-exit@\n- Collect the equity paperwork ^";
    let lone_field = field(lone, lone.len()).expect("lone caret field");
    assert_eq!(lone_field.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(lone_field.query, "");
    assert_eq!(lone_field.replacement, (lone.len(), lone.len()));
}

#[test]
fn task_list_block_id_completes_on_bullet_and_nested_lines() {
    let bullet = "@dev\n- Fix flaky gkeep test :";
    let bullet_field = field(bullet, bullet.len()).expect("list task field");
    assert_eq!(bullet_field.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(bullet_field.route.as_deref(), Some("dev"));
    assert_eq!(bullet_field.replacement, (bullet.len(), bullet.len()));

    let nested = "@dev\n- Fix flaky gkeep test\n  - repro on CI first :";
    let nested_field =
        field(nested, nested.len()).expect("nested list task field");
    assert_eq!(nested_field.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(nested_field.route.as_deref(), Some("dev"));
    assert_eq!(nested_field.replacement, (nested.len(), nested.len()));

    let notes = "@dev#Ideas\n- A good idea :";
    assert!(field(notes, notes.len()).is_none());
}

#[test]
fn nested_project_task_block_id_completes_for_its_owner() {
    let raw =
        "Finish it @cash^goog-exit@\n- Draft the memo ^draft-memo\n  - check the facts :";
    let completion = field(raw, raw.len()).expect("nested project task field");
    assert_eq!(completion.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(completion.route.as_deref(), Some("cash_goog_exit"));
    assert_eq!(completion.replacement, (raw.len(), raw.len()));
}

#[test]
fn project_task_id_before_a_child_line_route_marker_still_completes() {
    let raw =
        "Finish it @cash^goog-exit@\n- Draft the memo :dr @cash^goog-exit@";
    let id_end = raw.rfind(" @cash").expect("marker");
    let completion = field(raw, id_end).expect("project task field");
    assert_eq!(completion.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(completion.route.as_deref(), Some("cash_goog_exit"));
    assert_eq!(completion.query, "dr");
    // A cursor inside the child-line marker itself stays a marker field.
    let seal = raw.rfind('@').expect("seal");
    let marker = field(raw, seal).expect("marker field");
    assert_eq!(marker.context, CompletionContext::TaskBlockId);
}

#[test]
fn pomodoro_start_name_completes_after_hash_on_a_named_start() {
    // `=#` offers an empty name field whose replacement includes `#`, so
    // accepting a bare-spellable candidate removes the marker.
    let empty = field("=#", 2).expect("start name field");
    assert_eq!(empty.context, CompletionContext::PomodoroStartName);
    assert_eq!(empty.route, None);
    assert_eq!(empty.block_id, None);
    assert_eq!(empty.query, "");
    assert_eq!(empty.replacement, (1, 2));
    assert!(empty.bare_name);

    let counted = field("=3#", 3).expect("counted start name field");
    assert_eq!(counted.context, CompletionContext::PomodoroStartName);
    assert_eq!(counted.query, "");
    assert_eq!(counted.replacement, (2, 3));
    assert!(counted.bare_name);

    // A mid-name cursor reports the typed prefix but replaces the whole
    // name part.
    let raw = "=#deep";
    let mid = field(raw, 4).expect("mid-name field");
    assert_eq!(mid.context, CompletionContext::PomodoroStartName);
    assert_eq!(mid.query, "de");
    assert_eq!(mid.replacement, (1, 6));
    assert!(mid.bare_name);

    let end = field(raw, raw.len()).expect("end-of-name field");
    assert_eq!(end.query, "deep");
    assert_eq!(end.replacement, (1, 6));
    assert!(end.bare_name);

    // A cursor on `=<X>` or at the `#` byte itself offers nothing.
    assert_eq!(field("=#bugs", 0), None);
    assert_eq!(field("=#bugs", 1), None);
    assert_eq!(field("=3#bugs", 2), None);
    assert_eq!(field("=#", 1), None);

    // Bare starts offer nothing.
    assert_eq!(field("=", 1), None);
    assert_eq!(field("=3", 2), None);

    // `x#bugs` is a close near miss, never a start name.
    assert_eq!(field("x#bugs", 7), None);
}

#[test]
fn pomodoro_start_name_completes_to_bare_or_hashed_spellings() {
    let empty_hash = field("=#", 2).expect("hash empty");
    assert!(empty_hash.bare_name);
    assert_eq!(empty_hash.replacement, (1, 2));

    let mid = field("=bu", 3).expect("bare mid");
    assert_eq!(mid.context, CompletionContext::PomodoroStartName);
    assert_eq!(mid.query, "bu");
    assert_eq!(mid.replacement, (1, 3));
    assert!(mid.bare_name);

    let end = field("=bugs", 5).expect("bare end");
    assert_eq!(end.query, "bugs");
    assert_eq!(end.replacement, (1, 5));
    assert!(end.bare_name);

    // A cursor on `=` itself offers nothing; just after `=` is the empty
    // name query, matching just after `#` on `=#bugs`.
    assert_eq!(field("=bugs", 0), None);
    let after_eq = field("=bugs", 1).expect("just after =");
    assert_eq!(after_eq.query, "");
    assert_eq!(after_eq.replacement, (1, 5));
    assert!(after_eq.bare_name);

    let chain = "x =bu";
    let chained = field(chain, chain.len()).expect("chain bare");
    assert_eq!(chained.query, "bu");
    assert_eq!(chained.replacement, (3, 5));
    assert!(chained.bare_name);
    assert_eq!(field(chain, 1), None);

    let trail = "x wired it =bu";
    let trailed = field(trail, trail.len()).expect("inline trail");
    assert_eq!(trailed.query, "bu");
    assert_eq!(trailed.replacement, (12, 14));
    assert!(trailed.bare_name);
}

#[test]
fn retired_doubled_start_has_no_name_completion_or_override_context() {
    for raw in ["==#", "==#bugs", "==3#bugs", "==#bugs~2"] {
        assert_eq!(field(raw, raw.len()), None, "{raw}");
        assert_eq!(pomodoro_start_override_at(raw, raw.len()), None, "{raw}");
    }
}

#[test]
fn pomodoro_start_name_context_reports_ledger_behavior() {
    // A complete name field reports whether the swap keeps the running ledger.
    assert_eq!(pomodoro_start_override_at("=#bugs", 3), Some(true));
    assert_eq!(pomodoro_start_override_at("=bugs", 3), Some(true));
    assert_eq!(pomodoro_start_override_at("=3#bugs", 4), Some(false));
    assert_eq!(pomodoro_start_override_at("=-2#bugs", 4), Some(false));
    // A drop list does not change the kept ledger.
    assert_eq!(pomodoro_start_override_at("=#bugs~2", 6), Some(true));
    // A cursor before the name field has no context; an empty name field
    // already reports whether the running ledger is kept.
    assert_eq!(pomodoro_start_override_at("=#", 2), Some(true));
    assert_eq!(pomodoro_start_override_at("=3#", 3), Some(false));
    assert_eq!(pomodoro_start_override_at("=#bugs", 1), None);
    // The cursor immediately after `#` is inside the non-empty name field.
    assert_eq!(pomodoro_start_override_at("=#bugs", 2), Some(true));
    assert_eq!(pomodoro_start_override_at("=3#bugs", 2), None);
    assert_eq!(pomodoro_start_override_at("=#", 1), None);
    // Bare tokens and body text offer nothing.
    assert_eq!(pomodoro_start_override_at("=", 1), None);
    assert_eq!(pomodoro_start_override_at("buy milk", 4), None);
    // An earlier session operator omits override context.
    let chain = "x =#de";
    assert_eq!(pomodoro_start_override_at(chain, chain.len()), None);
    assert_eq!(pomodoro_start_override_at(chain, 1), None);
    let eq_chain = "x =#";
    assert_eq!(pomodoro_start_override_at(eq_chain, eq_chain.len()), None);
}

#[test]
fn pomodoro_start_name_completes_per_token_inside_chains() {
    let chain = "x =#de";
    let completion = field(chain, chain.len()).expect("chain name field");
    assert_eq!(completion.context, CompletionContext::PomodoroStartName);
    assert_eq!(completion.query, "de");
    assert_eq!(completion.replacement, (3, 6));
    assert!(completion.bare_name);

    // A cursor inside the close token stays an empty success.
    assert_eq!(field(chain, 1), None);
}

#[test]
fn pomodoro_start_name_leaves_link_form_names_alone() {
    // `@route:id#name` still completes the link-form Pomodoro name.
    let raw = "a @dev:some-id#bu";
    let completion = field(raw, raw.len()).expect("link name field");
    assert_eq!(completion.context, CompletionContext::PomodoroName);
    assert_eq!(completion.route.as_deref(), Some("dev"));
    assert_eq!(completion.block_id.as_deref(), Some("some-id"));
}

#[test]
fn project_task_block_id_completes_on_nested_project_bullets_through_owner() {
    let nested = "Finish it @cash^goog-exit@\n- Draft the memo :draft\n  - keep it short :x";
    let nested_field =
        field(nested, nested.len()).expect("nested owner task field");
    assert_eq!(nested_field.context, CompletionContext::ProjectTaskBlockId);
    assert_eq!(nested_field.route.as_deref(), Some("cash_goog_exit"));
    assert_eq!(nested_field.query, "x");
    // Non-project items keep `:1` literal.
    let plain = "Fix @sase\n- ratio 3 :1";
    assert_eq!(field(plain, plain.len()), None);
    // The parent line never completes a task ID.
    let parent = "Finish it @cash^goog-exit@\n- Draft the memo :dr";
    let parent_end = parent.find('\n').expect("newline");
    assert_eq!(field(parent, parent_end), None);
    // A cursor before the sigil is outside the token.
    let raw = "Finish it @cash^goog-exit@\n- Draft the memo :dr";
    let sigil = raw.rfind(':').expect("sigil");
    assert_eq!(field(raw, sigil), None);
}

#[test]
fn task_link_query_completes_the_sigil_inclusive_token() {
    // The cursor before the sigil, just after it, in the middle, and at
    // the end all report the `task_link` context; the query runs from
    // just after the sigil to the cursor and the replacement covers the
    // whole token including the sigil.
    let before = field(":dee", 0).expect("before sigil");
    assert_eq!(before.context, CompletionContext::TaskLink);
    assert_eq!(before.query, "");
    assert_eq!(before.replacement, (0, 4));

    let after_sigil = field(":dee", 1).expect("after sigil");
    assert_eq!(after_sigil.context, CompletionContext::TaskLink);
    assert_eq!(after_sigil.query, "");
    assert_eq!(after_sigil.replacement, (0, 4));

    let middle = field(":dee", 2).expect("middle");
    assert_eq!(middle.context, CompletionContext::TaskLink);
    assert_eq!(middle.query, "d");
    assert_eq!(middle.replacement, (0, 4));

    let end = field(":dee", 4).expect("end");
    assert_eq!(end.context, CompletionContext::TaskLink);
    assert_eq!(end.query, "dee");
    assert_eq!(end.replacement, (0, 4));

    let bare = field(":", 1).expect("bare sigil");
    assert_eq!(bare.context, CompletionContext::TaskLink);
    assert_eq!(bare.query, "");
    assert_eq!(bare.replacement, (0, 1));

    // The second item of a batch completes with draft-absolute offsets.
    let batch = "Buy milk\n\n:dee";
    let second = field(batch, batch.len()).expect("second item");
    assert_eq!(second.context, CompletionContext::TaskLink);
    assert_eq!(second.query, "dee");
    assert_eq!(second.replacement, (10, 14));

    // Multi-token and multi-line cases stay prose and offer nothing.
    assert_eq!(field(":dee more", 9), None);
    assert_eq!(field("Buy :dee", 8), None);
    let parented = ":dee\n- x";
    assert_eq!(field(parented, parented.len()), None);
}

#[test]
fn parent_task_plus_completes_the_terminal_token_with_utf8_byte_ranges() {
    let bare = field("+", 1).expect("bare parent-task selector");
    assert_eq!(bare.context, CompletionContext::TaskParent);
    assert_eq!(bare.query, "");
    assert_eq!(bare.replacement, (0, 1));

    let typed = field("Call bank +bank", 15).expect("typed selector");
    assert_eq!(typed.context, CompletionContext::TaskParent);
    assert_eq!(typed.query, "bank");
    assert_eq!(typed.replacement, (10, 15));

    let emoji = "🚀 Call bank +bank";
    let typed = field(emoji, emoji.len()).expect("emoji selector");
    assert_eq!(typed.query, "bank");
    assert_eq!(typed.replacement, (15, 20));

    let crlf = "First\r\n\r\n- note +bank";
    let typed = field(crlf, crlf.len()).expect("CRLF child selector");
    assert_eq!(typed.context, CompletionContext::TaskParent);
    assert_eq!(typed.query, "bank");
    assert_eq!(typed.replacement, (crlf.len() - 5, crlf.len()));
}

#[test]
fn parent_task_plus_is_shared_across_parent_and_authored_lines() {
    for raw in ["+bank", "Call bank +bank", "Parent\n- note +bank"] {
        let completion = field(raw, raw.len()).expect(raw);
        assert_eq!(completion.context, CompletionContext::TaskParent, "{raw}");
        assert_eq!(completion.query, "bank", "{raw}");
    }

    let later_item = "Parent\n\n+bank";
    let completion = field(later_item, later_item.len()).expect("later item");
    assert_eq!(
        completion.replacement,
        (later_item.len() - 5, later_item.len())
    );
}

#[test]
fn parent_task_plus_preserves_operator_and_protected_text_boundaries() {
    for raw in [
        "+2",
        "+0",
        "+2oops",
        "+-",
        "+-3",
        "++",
        "++3",
        "+2 x",
        "C++",
        "a+b",
        "\\+bank",
        "https://example.test/+",
        "@route^id@",
        "@@route +",
        "[[note| +]]",
        "```\n\n+\n\n```",
        "x\n- 1 work +",
    ] {
        assert!(
            field(raw, raw.len()).is_none(),
            "unexpected picker for {raw:?}"
        );
    }
}

#[test]
fn route_completion_beside_a_type_marker_keeps_original_ranges() {
    // A trailing marker never changes the neighboring route field.
    assert_eq!(field("Fix it @sa #fix", 10), field("Fix it @sa", 10));
    let completion = field("Fix it @sa #fix", 10).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.replacement, (8, 10));

    // A leading marker shifts nothing behind it: ranges stay absolute.
    let completion = field("#fix Fix it @sa", 15).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.query, "sa");
    assert_eq!(completion.replacement, (13, 15));

    // A marker on a child line leaves the child's route field alone.
    let raw = "Fix it\n- check #fix @sa";
    let base = "Fix it\n- check @sa";
    let shifted = field(base, base.len()).map(|mut found| {
        // The inserted `#fix ` moves every later byte by its length.
        found.replacement.0 += "#fix ".len();
        found.replacement.1 += "#fix ".len();
        found
    });
    assert_eq!(field(raw, raw.len()), shifted);
    let completion = field(raw, raw.len()).expect("route field");
    assert_eq!(completion.context, CompletionContext::Route);
    assert_eq!(completion.replacement, (21, 23));
}

#[test]
fn query_and_caret_completion_survive_a_neighboring_marker() {
    // Every whole-item query still claims through the masked view.
    assert_eq!(field(":query #fix", 3), field(":query", 3));
    assert_eq!(field("/fix #fix", 2), field("/fix", 2));
    assert_eq!(field("!fix #fix", 2), field("!fix", 2));
    assert_eq!(field("+fix #fix", 2), field("+fix", 2));
    // Caret completion on an existing-task link is unchanged.
    assert_eq!(field("^sase:abc #fix", 6), field("^sase:abc", 6));
}

#[test]
fn a_cursor_on_a_type_marker_requests_task_type_completion() {
    let marker = field("Fix it #fix", 8).expect("type marker field");
    assert_eq!(marker.context, CompletionContext::TaskType);
    assert_eq!(marker.query, "");
    assert_eq!(marker.replacement, (7, 11));

    let empty = field("Fix it #\"", 9).expect("incomplete literal type field");
    assert_eq!(empty.context, CompletionContext::TaskType);
    assert_eq!(empty.query, "");
    assert_eq!(empty.replacement, (7, 9));

    let leading = field("#fix Fix it", 1).expect("leading marker field");
    assert_eq!(leading.context, CompletionContext::TaskType);
    assert_eq!(leading.query, "");
    assert_eq!(leading.replacement, (0, 4));
}

#[test]
fn target_run_members_and_inner_edge_fragments_complete() {
    // Every run member completes through its own route and block ID, and an
    // `@`-fragment at the run's inner edge extends the run once completed.
    let routes = field("@work+a @de", "@work+a @de".len()).expect("routes");
    assert_eq!(routes.context, CompletionContext::Route);
    assert_eq!(routes.query, "de");

    let task = field("@work+a @dev+", "@work+a @dev+".len()).expect("task");
    assert_eq!(task.context, CompletionContext::Task);
    assert_eq!(task.route.as_deref(), Some("dev"));

    let drawer =
        field("@work+a @dev+b#re", "@work+a @dev+b#re".len()).expect("drawer");
    assert_eq!(drawer.context, CompletionContext::Drawer);
    assert_eq!(drawer.route.as_deref(), Some("dev"));

    let colon = field("@work+a @dev:", "@work+a @dev:".len()).expect("colon");
    assert_eq!(colon.context, CompletionContext::PomodoroBlockId);
    assert_eq!(colon.route.as_deref(), Some("dev"));

    let trailing_route =
        field("Called Morgan @wo @dev+b", "Called Morgan @wo".len())
            .expect("trailing routes");
    assert_eq!(trailing_route.context, CompletionContext::Route);
    assert_eq!(trailing_route.query, "wo");

    let trailing_drawer =
        field("note @work+a @dev+b#re", "note @work+a @dev+b#re".len())
            .expect("trailing drawer");
    assert_eq!(trailing_drawer.context, CompletionContext::Drawer);

    // Mid-line prose stays literal, even beside a run shape.
    assert_eq!(field("Discuss @de later", "Discuss @de".len()), None);
}

#[test]
fn transfer_sources_complete_routes_and_tasks() {
    // A cursor in a transfer source completes that note's routes and tasks,
    // even though the `>` destination follows on the same line.
    let route = field("@inbox+bench > @db", 4).expect("source route");
    assert_eq!(route.context, CompletionContext::Route);
    assert_eq!(route.query, "inb");
    assert_eq!(route.replacement, (1, 6));

    let task = field("@inbox+bench > @db", 9).expect("source task");
    assert_eq!(task.context, CompletionContext::Task);
    assert_eq!(task.route.as_deref(), Some("inbox"));
    assert_eq!(task.query, "be");
    assert_eq!(task.replacement, (7, 12));

    let second = field(
        "@inbox+bench @work+pgdump > @db",
        "@inbox+bench @work+p".len(),
    )
    .expect("second source task");
    assert_eq!(second.context, CompletionContext::Task);
    assert_eq!(second.route.as_deref(), Some("work"));
    assert_eq!(second.query, "p");
}

#[test]
fn transfer_destination_routes_complete_after_the_arrow() {
    // `> @` completes routes; a typed `> @db` fragment completes the
    // matching routes (the move-destination list).
    let bare = field("@inbox+bench > @", "@inbox+bench > @".len())
        .expect("destination routes");
    assert_eq!(bare.context, CompletionContext::Route);
    assert_eq!(bare.query, "");
    assert_eq!(bare.replacement, (16, 16));

    let typed = field("@inbox+bench > @db", "@inbox+bench > @db".len())
        .expect("typed destination routes");
    assert_eq!(typed.context, CompletionContext::Route);
    assert_eq!(typed.query, "db");
    assert_eq!(typed.replacement, (16, 18));
}

#[test]
fn transfer_merge_targets_drawers_and_sections_complete() {
    // `@c+` completes the destination note's open tasks (merge targets),
    // `@c+id#` completes that task's drawers, and `@c#` completes the
    // note's sections.
    let target = field("@a+x > @c+", "@a+x > @c+".len()).expect("merge target");
    assert_eq!(target.context, CompletionContext::Task);
    assert_eq!(target.route.as_deref(), Some("c"));
    assert_eq!(target.query, "");

    let typed = field("@a+x > @c+ti", "@a+x > @c+ti".len()).expect("typed id");
    assert_eq!(typed.context, CompletionContext::Task);
    assert_eq!(typed.route.as_deref(), Some("c"));
    assert_eq!(typed.query, "ti");

    let drawer =
        field("@a+x > @c+tid#fu", "@a+x > @c+tid#fu".len()).expect("drawer");
    assert_eq!(drawer.context, CompletionContext::Drawer);
    assert_eq!(drawer.route.as_deref(), Some("c"));
    assert_eq!(drawer.block_id.as_deref(), Some("tid"));
    assert_eq!(drawer.query, "fu");

    let section = field("@a+x > @c#fu", "@a+x > @c#fu".len()).expect("section");
    assert_eq!(section.context, CompletionContext::Section);
    assert_eq!(section.route.as_deref(), Some("c"));
    assert_eq!(section.query, "fu");
}

#[test]
fn transfer_plus_picker_uses_vault_merge_target_role() {
    let text = "@inbox+bench > +up";
    let completion =
        field(text, text.len()).expect("transfer parent-task picker");
    assert_eq!(completion.context, CompletionContext::TaskParent);
    assert_eq!(completion.query, "up");
    assert_eq!(completion.replacement, (15, 18));
    assert_eq!(
        transfer_completion_role(text, text.len()),
        Some(TransferCompletionRole::VaultMergeTarget {
            exclude: vec![("inbox".to_string(), "bench".to_string())],
        })
    );
}

#[test]
fn transfer_arrow_and_gaps_offer_no_completion() {
    // The `>` arrow and the whitespace around it own no completion field,
    // and neither does trailing whitespace past a complete destination.
    // (A cursor at the end of a source still completes that source.)
    assert_eq!(field("@inbox+bench > @db", 13), None);
    assert_eq!(field("@inbox+bench > @db", 14), None);
    assert_eq!(field("@inbox+bench > @db ", 19), None);
    let at_end = field("@inbox+bench > @db", 12).expect("source end");
    assert_eq!(at_end.context, CompletionContext::Task);
    assert_eq!(at_end.query, "bench");
}

#[test]
fn merge_in_source_slots_reuse_route_task_and_parent_contexts() {
    let route_text = "New task < @wo";
    let route = field(route_text, route_text.len()).expect("source route");
    assert_eq!(route.context, CompletionContext::Route);
    assert_eq!(route.query, "wo");

    let task_text = "New task < @work+";
    let task = field(task_text, task_text.len()).expect("scoped source task");
    assert_eq!(task.context, CompletionContext::Task);
    assert_eq!(task.route.as_deref(), Some("work"));
    assert_eq!(task.query, "");

    let parent_text = "New task < +fl";
    let parent = field(parent_text, parent_text.len())
        .expect("vault-wide source picker");
    assert_eq!(parent.context, CompletionContext::TaskParent);
    assert_eq!(parent.query, "fl");

    let complete = "New task < @work+old +fl";
    assert_eq!(
        merge_in_completion_role(complete, complete.len()),
        Some(MergeInCompletionRole {
            exclude: vec![("work".to_string(), "old".to_string())],
        })
    );
}

#[test]
fn merge_in_arrow_and_gaps_do_not_fall_through_to_target_runs() {
    let raw = "New task < @work+old";
    let arrow = raw.find('<').expect("arrow");
    assert_eq!(field(raw, arrow + 1), None);
    assert_eq!(field("New task < ", "New task < ".len()), None);
}

#[test]
fn move_bullet_completion_opens_under_both_seals() {
    for raw in ["@cash^exit@\n- ^", "@cash:exit@\n- ^"] {
        let cursor = raw.len();
        let completion = field(raw, cursor).expect("move field");
        assert_eq!(
            completion.context,
            CompletionContext::ProjectTaskMove,
            "{raw:?}"
        );
        assert_eq!(completion.query, "", "{raw:?}");
        // The replacement owns only the ID: an empty range after a lone sigil.
        assert_eq!(completion.replacement, (cursor, cursor), "{raw:?}");
    }
}

#[test]
fn past_segment_suffix_field_ranges_and_queries() {
    // (draft, cursor, expected query, expected replacement)
    for (raw, cursor, query, replacement) in [
        ("x=~", 3, "", (2, 3)),
        ("x=2~", 4, "", (3, 4)),
        ("x=2~1", 5, "1", (3, 5)),
        ("x=fix~2", 7, "2", (5, 7)),
        ("x=2~1+", 6, "1+", (3, 6)),
        ("x=~~", 4, "~", (2, 4)),
        ("x=2~~", 5, "~", (3, 5)),
        ("x=2~1!=!1", 5, "1", (3, 5)),
    ] {
        let completion = field(raw, cursor).expect("segment field");
        assert_eq!(
            completion.context,
            CompletionContext::PomodoroPastSegment,
            "{raw:?} at {cursor}"
        );
        assert_eq!(completion.query, query, "{raw:?} at {cursor}");
        assert_eq!(completion.replacement, replacement, "{raw:?} at {cursor}");
    }
    // A cursor between the two tildes of `~~` has an empty query.
    let between = field("x=~~", 3).expect("segment field");
    assert_eq!(between.context, CompletionContext::PomodoroPastSegment);
    assert_eq!(between.query, "");
    assert_eq!(between.replacement, (2, 4));
    // A cursor in the name still offers the name field, which ends at `~`.
    let name = field("x=fix~2", 4).expect("name field");
    assert_eq!(name.context, CompletionContext::PomodoroPastName);
    assert_eq!(name.query, "fi");
}
