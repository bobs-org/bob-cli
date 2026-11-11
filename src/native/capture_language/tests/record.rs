//! Record-language claim tests (phase `record-language`).
//!
//! Route precedence, literal fallback, all marker families, field
//! opacity, source spans, empty title/partial stem, and template/list
//! boundaries. Fail-closed execution is covered by the CLI suite in
//! `tests/cli/capture/record.rs`.

use super::super::model::*;
use super::super::record::{
    RecordRoutePolicy, RECORDS_CONFIG_INVALID_CODE, RECORD_FIELD_NESTED_CODE,
    RECORD_FOLDER_NOT_RECORD_CODE, RECORD_MARKER_UNSUPPORTED_CODE,
    RECORD_SHAPE_CODE, RECORD_STEM_CODE, RECORD_TITLE_CODE,
    RECORD_TYPE_AS_FOLDER_CODE,
};
use super::*;
use crate::native::records::schema::parse_records_config;
use std::path::Path;

const CONFIG: &str = r#"
records:
  restaurant:
    dir: eat
    label: Restaurant
    fields:
      status: {required: true}
      location: {}
"#;

fn configured_policy() -> RecordRoutePolicy {
    let config = parse_records_config(CONFIG, Path::new("/config.yml"))
        .expect("test records config is valid");
    RecordRoutePolicy::from_config(&config, Some(Path::new("/config.yml")))
}

fn strict_options(
    policy: Option<&RecordRoutePolicy>,
) -> CaptureParseOptions<'_> {
    CaptureParseOptions {
        url_routing: None,
        record_policy: policy,
        notebook_policy: None,
        explicit_destination: false,
        has_global_destination: false,
        recognize_lists: true,
        global_route: None,
        forced_task: None,
        forced_drawer: None,
    }
}

fn editor_options(
    policy: Option<&RecordRoutePolicy>,
) -> EditorParseOptions<'_> {
    EditorParseOptions {
        url_routing: None,
        has_global_destination: false,
        record_policy: policy,
        notebook_policy: None,
        global_route: None,
    }
}

fn strict(raw: &str) -> Result<ParsedCaptureText, String> {
    let policy = configured_policy();
    parse_capture_text_with_options(
        raw,
        None,
        None,
        &strict_options(Some(&policy)),
    )
}

#[test]
fn draft_policy_glue_finds_configured_dirs() {
    let dir = std::env::temp_dir().join(format!(
        "bob-record-glue-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.yml");
    std::fs::write(&config, CONFIG).unwrap();
    let policy = crate::native::capture_record::record_policy_for_draft(
        &dir,
        Some(&config),
        "Ramen Nagi @eat/",
    );
    assert!(policy.configured_dir("eat").is_some(), "eat is configured");
    assert!(policy.configured_dir("nope").is_none());
    assert_eq!(
        crate::native::capture_language::candidate_dirs("Ramen Nagi @eat/"),
        vec!["eat".to_string()]
    );
    assert!(
        crate::native::capture_language::candidate_dirs("Buy milk").is_empty()
    );
}

fn strict_code(raw: &str) -> &'static str {
    let policy = configured_policy();
    parse_capture_draft_with_options_coded(
        raw,
        None,
        None,
        &strict_options(Some(&policy)),
    )
    .expect_err("record refusal")
    .code
    .expect("coded record refusal")
}

fn editor_parse(raw: &str) -> EditorItemParse {
    let policy = configured_policy();
    parse_for_editor_with(raw, &editor_options(Some(&policy)))
        .items
        .remove(0)
}

fn assert_record_title(raw: &str, title: &str, dir: &str) {
    let parsed = strict(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("{raw}: not a record note: {:?}", parsed.kind);
    };
    assert_eq!(intent.title, title, "{raw}");
    assert_eq!(intent.dir, dir, "{raw}");
    assert_eq!(parsed.route.as_deref(), Some(dir), "{raw}");
    assert_eq!(parsed.body, title, "{raw}");
}

#[test]
fn route_first_and_title_first_claim_creation() {
    assert_record_title("@eat/ Ramen Nagi", "Ramen Nagi", "eat");
    assert_record_title("Ramen Nagi @eat/", "Ramen Nagi", "eat");
}

#[test]
fn explicit_stem_claims_both_positions() {
    let parsed =
        strict("@eat/^nagi-shop Ramen Nagi").expect("route-first stem");
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("not a record note");
    };
    assert_eq!(intent.explicit_stem.as_deref(), Some("nagi-shop"));
    assert_eq!(intent.title, "Ramen Nagi");

    let parsed =
        strict("Ramen Nagi @eat/^nagi-shop").expect("title-first stem");
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("not a record note");
    };
    assert_eq!(intent.explicit_stem.as_deref(), Some("nagi-shop"));
    assert_eq!(intent.title, "Ramen Nagi");
}

#[test]
fn bare_route_is_a_title_need() {
    assert_eq!(strict_code("@eat/"), RECORD_TITLE_CODE);
    assert_eq!(strict_code("@eat/   "), RECORD_TITLE_CODE);
    let item = editor_parse("@eat/");
    assert_eq!(item.mode, EditorMode::RecordNote);
    assert_eq!(item.needs, vec![Need::RecordTitle]);
    assert!(item.diagnostics.is_empty(), "bare route stays completable");
}

#[test]
fn middle_route_tokens_stay_literal() {
    // A record route in the middle of the line never claims; the line
    // stays an ordinary task.
    let parsed =
        strict("see @eat/ for details").expect("middle route stays a task");
    assert!(matches!(parsed.kind, CaptureKind::Task));
    assert_eq!(parsed.body, "see @eat/ for details");
}

#[test]
fn unknown_folders_stay_literal_task_text() {
    let parsed =
        strict("Ramen @notafolder/").expect("unknown folder stays a task");
    assert!(matches!(parsed.kind, CaptureKind::Task));
    // Absent policy (no records config at all) also stays literal.
    let parsed = parse_capture_text_with_options(
        "@eat/ Ramen Nagi",
        None,
        None,
        &strict_options(None),
    )
    .expect("absent policy stays a task");
    assert!(matches!(parsed.kind, CaptureKind::Task));
}

#[test]
fn forced_destination_keeps_record_text_literal() {
    let policy = configured_policy();
    let parsed = parse_capture_text_with_options(
        "@eat/ Ramen Nagi",
        Some("work"),
        None,
        &strict_options(Some(&policy)),
    )
    .expect("forced route keeps the text literal");
    assert!(matches!(parsed.kind, CaptureKind::Task));
}

#[test]
fn update_spelling_claims_both_positions_and_keeps_existing_stems() {
    for (raw, stem, note) in [
        ("@eat/ninja_sushi Went back", "ninja_sushi", "Went back"),
        ("Went back @eat/東京店", "東京店", "Went back"),
    ] {
        let parsed =
            strict(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
        let CaptureKind::RecordUpdate { intent } = parsed.kind else {
            panic!("{raw}: expected RecordUpdate");
        };
        assert_eq!(intent.update_stem.as_deref(), Some(stem), "{raw}");
        assert_eq!(intent.explicit_stem, None, "{raw}");
        assert_eq!(intent.title, note, "{raw}");
        assert_eq!(parsed.body, note, "{raw}");
        assert_eq!(intent.action, super::super::record::RecordAction::Update);
        let (start, end) = intent.stem_range.expect("target stem range");
        assert_eq!(&raw[start..end], stem, "{raw}");
        let editor = editor_parse(raw);
        assert_eq!(editor.mode, EditorMode::RecordUpdate, "{raw}");
        assert!(editor.needs.is_empty(), "{raw}");
        assert!(editor.diagnostics.is_empty(), "{raw}");
    }
}

#[test]
fn route_only_update_has_no_title_need_and_parses_authored_notes() {
    let only_route = editor_parse("@eat/ninja_sushi");
    assert_eq!(only_route.mode, EditorMode::RecordUpdate);
    assert!(only_route.needs.is_empty());
    assert!(only_route.diagnostics.is_empty());

    let parsed = strict(
        "@eat/ninja_sushi\n- status:: liked\n- Still great\n  - Kelly agrees",
    )
    .expect("field-only and nested-note update");
    let CaptureKind::RecordUpdate { intent } = parsed.kind else {
        panic!("not a record update");
    };
    assert_eq!(intent.title, "");
    assert_eq!(intent.fields[0].key, "status");
    assert_eq!(intent.fields[0].value, "liked");
    assert_eq!(intent.notes[0].lines[0].text, "Still great");
    assert_eq!(intent.notes[0].lines[0].children[0].text, "Kelly agrees");
}

#[test]
fn malformed_stems_stay_in_the_record_path() {
    assert_eq!(strict_code("@eat/^"), RECORD_STEM_CODE);
    // A valid stem with more title text stays a creation.
    assert_record_title("@eat/^bad stem", "stem", "eat");
}

#[test]
fn unrelated_nested_task_routes_keep_task_meaning() {
    // These are nested task route forms, not record stem spellings.
    for raw in ["Ramen @eat/a+b", "Ramen @eat/#sec"] {
        match strict(raw) {
            Ok(parsed) => assert!(
                matches!(parsed.kind, CaptureKind::Task),
                "{raw}: {:?}",
                parsed.kind
            ),
            Err(error) => assert!(
                !error.contains("record"),
                "{raw}: record error leaks: {error}"
            ),
        }
    }
    for raw in ["@eat/^bad_stem Ramen", "Ramen @eat/^bad_stem"] {
        assert_eq!(strict_code(raw), RECORD_STEM_CODE, "{raw}");
    }
}

#[test]
fn task_markers_at_both_route_edges_refuse_as_record_modifiers() {
    let markers = [
        "#dinner",
        "s:3",
        "$2",
        "%c",
        "%n",
        "%l",
        "&work:abc",
        "*work:abc",
        "~work:abc",
        "!work:abc",
        "+query",
        ":abc",
        "^abc",
        "=3",
        "x2",
    ];
    for marker in markers {
        for raw in [
            format!("@eat/ Ramen {marker}"),
            format!("Ramen {marker} @eat/"),
            format!("{marker} @eat/ Ramen"),
            format!("Ramen @eat/ {marker}"),
        ] {
            let error = parse_capture_draft_with_options_coded(
                &raw,
                None,
                None,
                &strict_options(Some(&configured_policy())),
            )
            .expect_err("task modifier must not hide the record route");
            assert_eq!(
                error.code,
                Some(RECORD_MARKER_UNSUPPORTED_CODE),
                "{raw}: {error:?}"
            );
            let editor = parse_for_editor_with(
                &raw,
                &editor_options(Some(&configured_policy())),
            );
            assert_eq!(editor.items[0].mode, EditorMode::RecordNote, "{raw}");
            assert_eq!(
                editor.items[0].diagnostics[0].code,
                RECORD_MARKER_UNSUPPORTED_CODE,
                "{raw}"
            );
        }
    }
}

#[test]
fn task_modifier_utf8_ranges_point_to_original_bytes() {
    for raw in ["🍜 @eat/ s:3", "s:3 @eat/ 🍜"] {
        let error = parse_capture_draft_with_options_coded(
            raw,
            None,
            None,
            &strict_options(Some(&configured_policy())),
        )
        .expect_err("schedule marker refuses");
        let (start, end) = error.span.expect("marker span");
        assert_eq!(&raw[start..end], "s:3", "{raw}");
        assert_eq!(error.code, Some(RECORD_MARKER_UNSUPPORTED_CODE));
    }
}

#[test]
fn escaped_type_text_decodes_in_title_but_keeps_original_range() {
    let raw = "Ramen \\#READ @eat/";
    let parsed = strict(raw).expect("escaped type text is literal");
    let CaptureKind::RecordNote { intent } = parsed.kind else {
        panic!("not a record");
    };
    assert_eq!(intent.title, "Ramen #READ");
    let range = intent.title_range.expect("original title range");
    assert_eq!(&raw[range.0..range.1], "Ramen \\#READ");
}

#[test]
fn field_values_do_not_claim_template_directives_or_hide_placeholders() {
    let policy = configured_policy();
    for raw in [
        "Ramen @eat/\\n- location:: %t",
        "@eat/ Ramen\\n- location:: %t",
    ] {
        let raw = raw.replace("\\n", "\n");
        let masked = super::super::record::mask_field_values_for_template(
            &raw,
            Some(&policy),
        );
        assert!(super::super::template::analyze(&masked).directive.is_none());
        strict(&raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
    }

    let raw = "{0} @eat/ %t\n- location:: {1}\n\nRamen %t";
    let masked = super::super::record::mask_field_values_for_template(
        raw,
        Some(&policy),
    );
    let analysis = super::super::template::analyze(&masked);
    assert!(analysis.directive.is_some(), "parent %t remains active");
    assert_eq!(analysis.placeholders.len(), 2);
    assert_eq!(
        &raw[analysis.placeholders[1].range.clone()],
        "{1}",
        "field placeholders remain available to the existing expander"
    );
}

#[test]
fn existing_non_record_folder_refuses() {
    let policy = RecordRoutePolicy::absent()
        .with_existing(["work".to_string()].into_iter().collect());
    let error = parse_capture_text_with_options(
        "Status @work/",
        None,
        None,
        &strict_options(Some(&policy)),
    )
    .expect_err("existing non-record folder refuses");
    assert!(error.contains("not a record folder"), "{error}");
    let code = parse_capture_draft_with_options_coded(
        "Status @work/",
        None,
        None,
        &strict_options(Some(&policy)),
    )
    .expect_err("coded refusal")
    .code
    .expect("code");
    assert_eq!(code, RECORD_FOLDER_NOT_RECORD_CODE);
}

#[test]
fn configured_type_id_as_folder_names_its_directory() {
    assert_eq!(
        strict_code("Dinner @restaurant/"),
        RECORD_TYPE_AS_FOLDER_CODE
    );
}

#[test]
fn malformed_config_refuses_candidates() {
    let policy = RecordRoutePolicy::invalid(
        Path::new("/config.yml"),
        "configuration root must be a mapping".to_string(),
    );
    for raw in ["@eat/ Ramen", "Ramen @eat/", "Ramen @notafolder/"] {
        let error = parse_capture_draft_with_options_coded(
            raw,
            None,
            None,
            &strict_options(Some(&policy)),
        )
        .expect_err("malformed config refuses")
        .code
        .expect("code");
        assert_eq!(error, RECORDS_CONFIG_INVALID_CODE, "{raw}");
    }
    // Items without a record candidate keep working under malformed config.
    let parsed = parse_capture_text_with_options(
        "Buy milk @work",
        None,
        None,
        &strict_options(Some(&policy)),
    )
    .expect("unrelated capture keeps working");
    assert!(matches!(parsed.kind, CaptureKind::Task));
}

#[test]
fn fields_parse_notes_stay_notes() {
    let parsed = strict(
        "@eat/ Ramen Nagi\n- status:: liked\n- location:: Clark\n- Spicy tuna was great.",
    )
    .expect("fields and notes");
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("not a record note");
    };
    assert_eq!(intent.fields.len(), 2);
    assert_eq!(intent.fields[0].key, "status");
    assert_eq!(intent.fields[0].value, "liked");
    assert_eq!(intent.fields[1].key, "location");
    assert_eq!(intent.fields[1].value, "Clark");
    assert_eq!(intent.notes.len(), 1);
    assert_eq!(intent.notes[0].lines.len(), 1);
    assert_eq!(intent.notes[0].lines[0].text, "Spicy tuna was great.");
    // A single colon stays a note, never a field.
    let parsed =
        strict("@eat/ Ramen\n- location: Clark").expect("single colon");
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("not a record note");
    };
    assert!(intent.fields.is_empty(), "single colon is not a field");
    assert_eq!(intent.notes[0].lines[0].text, "location: Clark");
    // Unknown keys stay field inputs for engine teaching.
    let parsed =
        strict("@eat/ Ramen\n- bogus:: yes").expect("unknown key stays input");
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("not a record note");
    };
    assert_eq!(intent.fields.len(), 1);
    assert_eq!(intent.fields[0].key, "bogus");
}

#[test]
fn sections_group_and_nested_notes_attach() {
    let parsed = strict(
        "@eat/ Ramen\n- VISIT NOTES\n- Great broth\n  - extra noodles\n- location:: Clark",
    )
    .expect("sections");
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("not a record note");
    };
    assert_eq!(intent.notes.len(), 1, "one section group");
    assert_eq!(
        intent.notes[0].section.as_deref(),
        Some("VISIT NOTES"),
        "section header groups"
    );
    assert_eq!(intent.notes[0].lines[0].text, "Great broth");
    assert_eq!(intent.notes[0].lines[0].children.len(), 1);
    assert_eq!(intent.notes[0].lines[0].children[0].text, "extra noodles");
    assert_eq!(intent.fields.len(), 1);
    assert_eq!(intent.fields[0].key, "location");
}

#[test]
fn nested_bullets_under_fields_refuse() {
    assert_eq!(
        strict_code("@eat/ Ramen\n- location:: Clark\n  - nested detail"),
        RECORD_FIELD_NESTED_CODE
    );
}

#[test]
fn marker_families_refuse_outside_field_values() {
    for (raw, marker) in [
        ("Ramen #dinner @eat/", "#dinner"),
        ("Ramen s:3 @eat/", "s:3"),
        ("Ramen $2 @eat/", "$2"),
        ("Ramen p:2 @eat/", "p:2"),
        ("Ramen %c @eat/", "%c"),
        ("Ramen %n @eat/", "%n"),
        ("Ramen %l @eat/", "%l"),
        ("Ramen &work:abc @eat/", "&work:abc"),
        ("Ramen *work:abc @eat/", "*work:abc"),
        ("Ramen ~work:abc @eat/", "~work:abc"),
        ("Ramen !work:abc @eat/", "!work:abc"),
        ("Ramen +query @eat/", "+query"),
        ("Ramen :abc @eat/", ":abc"),
        ("Ramen ^abc @eat/", "^abc"),
        ("Ramen =3 @eat/", "=3"),
        ("Ramen x2 @eat/", "x2"),
    ] {
        let error = parse_capture_text_with_options(
            raw,
            None,
            None,
            &strict_options(Some(&configured_policy())),
        )
        .expect_err("marker refuses");
        assert!(error.contains(marker), "{raw}: {error}");
        assert!(error.contains("task marker"), "{raw}: {error}");
        let code = parse_capture_draft_with_options_coded(
            raw,
            None,
            None,
            &strict_options(Some(&configured_policy())),
        )
        .expect_err("coded refusal")
        .code
        .expect("code");
        assert_eq!(code, RECORD_MARKER_UNSUPPORTED_CODE, "{raw}");
    }
}

#[test]
fn field_values_stay_opaque() {
    let parsed = strict(
        "@eat/ Ramen\n- note:: had #dinner with s:3 and $2 %c &x:y ~z http://x [[Wikilink]] :link ^id +q =3 x2",
    )
    .expect("opaque field values");
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("not a record note");
    };
    assert_eq!(intent.fields.len(), 1);
    assert!(intent.fields[0].value.contains("#dinner"));
    assert!(intent.fields[0].value.contains("http://x"));
    assert!(intent.fields[0].value.contains("[[Wikilink]]"));
}

#[test]
fn prose_punctuation_stays_literal() {
    for raw in [
        "Ramen (great!) @eat/",
        "Ramen :) @eat/",
        "Ramen :smile: @eat/",
        "Ramen \"quoted #tag\" @eat/",
        "a = b @eat/",
        "x marks the spot @eat/",
        "C++ notes @eat/",
    ] {
        strict(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
    }
}

#[test]
fn editor_spans_cover_route_stem_and_fields() {
    let item = editor_parse("@eat/^nagi-shop Ramen\n- location:: Clark");
    assert_eq!(item.mode, EditorMode::RecordNote);
    assert_eq!(item.route.as_deref(), Some("eat"));
    assert_eq!(item.body, "Ramen");
    assert!(item.needs.is_empty());
    assert!(item.diagnostics.is_empty());
    let kinds: Vec<SpanKind> =
        item.spans.iter().map(|span| span.kind).collect();
    assert!(kinds.contains(&SpanKind::RecordRoute), "{kinds:?}");
    assert!(kinds.contains(&SpanKind::RecordFieldKey), "{kinds:?}");
    assert!(kinds.contains(&SpanKind::RecordFieldValue), "{kinds:?}");
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == SpanKind::RecordRoute)
            .count(),
        1,
        "one record_route over the whole token: {kinds:?}"
    );
    // Ranges re-slice the typed text.
    let raw = "@eat/^nagi-shop Ramen\n- location:: Clark";
    for span in &item.spans {
        let text = &raw[span.start..span.end];
        match span.kind {
            SpanKind::RecordRoute => assert_eq!(text, "@eat/^nagi-shop"),
            SpanKind::RecordFieldKey => assert_eq!(text, "location"),
            SpanKind::RecordFieldValue => assert_eq!(text, "Clark"),
            _ => {}
        }
    }
}

#[test]
fn editor_empty_values_need_record_value() {
    let item = editor_parse("@eat/ Ramen\n- location::");
    assert_eq!(item.mode, EditorMode::RecordNote);
    assert_eq!(item.needs, vec![Need::RecordValue]);
}

#[test]
fn record_claims_before_transfer() {
    // A `>` in the title stays title text; the record claims before the
    // transfer parser can split it.
    let parsed = strict("A > B @eat/").expect("record wins over transfer");
    assert!(matches!(parsed.kind, CaptureKind::RecordNote { .. }));
}

#[test]
fn list_bullets_can_claim_records() {
    // A record route on a list bullet claims before list parsing, while
    // sibling bullets under the same header stay tasks.
    let policy = configured_policy();
    let draft = parse_capture_draft_with_options(
        "@work\n- Ramen @eat/\n- Buy milk",
        None,
        None,
        &strict_options(Some(&policy)),
    )
    .expect("mixed list draft");
    assert_eq!(draft.items.len(), 2);
    assert!(
        matches!(draft.items[0].parsed.kind, CaptureKind::RecordNote { .. }),
        "record bullet claims"
    );
    assert!(
        matches!(draft.items[1].parsed.kind, CaptureKind::Task),
        "sibling stays a task"
    );
}

#[test]
fn local_record_route_overrides_global_declaration() {
    // `@@work` lifts the sibling task but leaves the record on `eat/`.
    let policy = configured_policy();
    let draft = parse_capture_draft_with_options(
        "@@work\nRamen @eat/\n\nBuy milk",
        None,
        None,
        &strict_options(Some(&policy)),
    )
    .expect("global declaration draft");
    assert_eq!(draft.items.len(), 2);
    let CaptureKind::RecordNote { .. } = draft.items[0].parsed.kind else {
        panic!("record keeps its local route");
    };
    assert_eq!(draft.items[0].parsed.route.as_deref(), Some("eat"));
    assert_eq!(draft.items[1].parsed.route.as_deref(), Some("work"));
}

#[test]
fn invalid_child_lines_stay_in_the_record_path() {
    assert_eq!(
        strict_code("@eat/ Ramen\n    - four-space indent"),
        RECORD_SHAPE_CODE
    );
    assert_eq!(
        strict_code("@eat/ Ramen\n  - nested without owner"),
        RECORD_SHAPE_CODE
    );
}

#[test]
fn unicode_ranges_survive() {
    let raw = "@eat/ 🍜 Ramen — émoticônes\n- location:: Zürich";
    let parsed = strict(raw).expect("unicode record");
    let CaptureKind::RecordNote { intent } = &parsed.kind else {
        panic!("not a record note");
    };
    let (start, end) = intent.title_range.expect("title range");
    assert_eq!(&raw[start..end], "🍜 Ramen — émoticônes");
    let (start, end) = intent.route_range;
    assert_eq!(&raw[start..end], "@eat/");
    assert_eq!(
        &raw[intent.fields[0].value_range.0..intent.fields[0].value_range.1],
        "Zürich"
    );
}

#[test]
fn record_claim_is_shared_between_paths() {
    // The strict and editor paths share one claim: the same draft
    // classifies identically on both surfaces.
    let raw = "@eat/ Ramen Nagi\n- location:: Clark\n- Great spot";
    let strict_parsed = strict(raw).expect("strict");
    let editor = editor_parse(raw);
    assert_eq!(editor.mode, EditorMode::RecordNote);
    let CaptureKind::RecordNote { intent } = &strict_parsed.kind else {
        panic!("strict did not claim");
    };
    assert_eq!(intent.title, editor.body);
    assert_eq!(Some(intent.dir.as_str()), editor.route.as_deref());
    assert_eq!(intent.title, strict_parsed.body);
}
