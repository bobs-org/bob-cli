//! Record field/value completion, route labels, and section create rows.

use super::super::{
    engine::{build_result, build_result_for, CompleteRequest},
    model::{
        CandidateContinuation, Candidates, CaptureCompleteResult,
        RecordEditAction, RecordValueCandidate, Replacement,
        RouteCandidateKind, SectionMenuRow,
    },
    records::last_catalog_files_opened,
    render::candidate_lines,
    shell::shell_completion,
};
use super::{write_file, TempDir};
use crate::native::capture_language::{
    CompletionContext, RECORDS_CONFIG_INVALID_CODE, RECORD_ASSIST_VERSION,
};
use crate::native::env::TestEnvGuard;
use std::ffi::OsStr;
use std::path::Path;

const NOW: &str = "2026-10-07 13:40:00";

const RECORDS_CONFIG: &str = r#"records:
  restaurant:
    dir: eat
    label: Restaurant
    emoji: "🍽️"
    dated_notes: true
    fields:
      status:
        required: true
        values:
          to_try: "To try"
          liked: "Liked"
      category:
        required: true
        suggest: true
      location:
        required: true
        suggest: true
      visited:
        values: date
      tags:
        list: true
        suggest: true
      website:
        values: url
      rating:
        values: number
      related:
        values: link
      companions:
        values: link
        list: true
        default: ["[[kelly]]"]
"#;

struct RecordVault {
    temp: TempDir,
    config: std::path::PathBuf,
}

impl RecordVault {
    fn new(prefix: &str) -> Self {
        let temp = TempDir::new(prefix);
        let config = temp.path().join("config.yml");
        write_file(&config, RECORDS_CONFIG);
        Self { temp, config }
    }

    fn path(&self) -> &Path {
        self.temp.path()
    }

    fn write_note(&self, stem: &str, category: &str, location: &str) {
        write_file(
            &self.path().join("eat").join(format!("{stem}.md")),
            &format!(
                "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
                 title: {stem:?}\nstatus: liked\ncategory: {category:?}\n\
                 location: {location}\n---\n# {stem}\n\n## Notes\n"
            ),
        );
    }

    fn complete(&self, raw: &str, cursor: usize) -> CaptureCompleteResult {
        self.request(raw, cursor, false, None)
    }

    fn next_missing(&self, raw: &str, cursor: usize) -> CaptureCompleteResult {
        self.request(raw, cursor, true, None)
    }

    fn named_field(
        &self,
        raw: &str,
        cursor: usize,
        key: &str,
    ) -> CaptureCompleteResult {
        self.request(raw, cursor, false, Some(key))
    }

    fn request(
        &self,
        raw: &str,
        cursor: usize,
        next_missing: bool,
        record_field: Option<&str>,
    ) -> CaptureCompleteResult {
        let _guard = TestEnvGuard::set(&[
            ("BOB_CONFIG_FILE", Some(self.config.as_os_str())),
            ("BOB_NOW", Some(OsStr::new(NOW))),
        ]);
        if next_missing || record_field.is_some() {
            build_result_for(&CompleteRequest {
                bob_dir: self.path(),
                raw_text: raw,
                cursor,
                all_tasks: false,
                closed_tasks: false,
                next_missing_field: next_missing,
                record_field,
            })
            .expect("targeted complete")
        } else {
            build_result(self.path(), raw, cursor, false, false)
                .expect("complete")
        }
    }
}

fn apply(
    raw: &str,
    result: &CaptureCompleteResult,
    replacement: &str,
) -> String {
    format!(
        "{}{}{}",
        &raw[..result.replacement.start],
        replacement,
        &raw[result.replacement.end..]
    )
}

fn apply_edit(raw: &str, edit: &RecordEditAction) -> String {
    format!(
        "{}{}{}",
        &raw[..edit.replacement_range.start],
        edit.replacement,
        &raw[edit.replacement_range.end..]
    )
}

fn record_edit(result: &CaptureCompleteResult) -> &RecordEditAction {
    result
        .record
        .as_ref()
        .and_then(|meta| meta.edit.as_ref())
        .expect("record.edit")
}

fn value_rows(result: &CaptureCompleteResult) -> &[RecordValueCandidate] {
    let Candidates::RecordValue(rows) = &result.candidates else {
        panic!("expected record value candidates: {result:?}");
    };
    rows
}

fn find_value<'a>(
    rows: &'a [RecordValueCandidate],
    value: &str,
) -> &'a RecordValueCandidate {
    rows.iter()
        .find(|row| row.value == value)
        .unwrap_or_else(|| panic!("missing value {value:?} in {rows:?}"))
}

#[test]
fn route_menu_offers_eat_with_new_label_and_continuation() {
    let vault = RecordVault::new("bob-cli-complete-record-route");
    vault.write_note("nagi", "Ramen", "\"Clark\"");
    vault.write_note("west_1", "Pizza", "Westfield");
    let value = vault.complete("@", 1);
    assert_eq!(value.context, Some(CompletionContext::Route));
    let Candidates::Route(routes) = &value.candidates else {
        panic!("expected route candidates");
    };
    let eat = routes
        .iter()
        .find(|route| route.route == "eat/")
        .unwrap_or_else(|| panic!("missing eat route: {routes:?}"));
    assert_eq!(eat.kind, RouteCandidateKind::Record);
    assert_eq!(eat.replacement, "eat/ ");
    assert_eq!(eat.emoji.as_deref(), Some("🍽️"));
    assert_eq!(eat.record_type.as_deref(), Some("restaurant"));
    assert!(eat.count.is_some_and(|count| count >= 2), "{eat:?}");
    assert_eq!(
        eat.label,
        format!("🍽️ New restaurant · {}", eat.count.unwrap())
    );
    let continuation = eat.continuation.as_ref().expect("continuation");
    assert!(continuation.next_missing_field);
    assert_eq!(
        continuation.cursor,
        value.replacement.start + eat.replacement.len()
    );
}

#[test]
fn record_target_menu_ranks_matches_and_shows_stored_values() {
    let vault = RecordVault::new("bob-cli-complete-record-targets");
    vault.write_note("west_1", "Pizza", "Westfield");
    vault.write_note("west_2", "Pizza", "Westfield");
    write_file(
        &vault.path().join("eat/wast.md"),
        "---\ntitle: Wast\nstatus: liked\ncategory: Italian\nlocation: Clark\n---\n# Wast\n",
    );

    let raw = "@eat/wes";
    let result = vault.complete(raw, raw.len());
    assert_eq!(result.context, Some(CompletionContext::RecordTarget));
    assert_eq!(
        result.replacement,
        Replacement {
            start: 0,
            end: raw.len()
        }
    );
    let Candidates::RecordTarget(rows) = &result.candidates else {
        panic!("expected record targets: {result:?}");
    };
    let fuzzy = rows
        .iter()
        .position(|row| row.stem == "wast")
        .expect("fuzzy title match");
    assert!(
        rows[..fuzzy].iter().all(|row| row.stem.starts_with("west")),
        "prefix rows must precede fuzzy title matches: {rows:?}"
    );
    let west = rows
        .iter()
        .find(|row| row.stem == "west_1")
        .expect("west_1 target");
    assert_eq!(west.replacement, "@eat/west_1");
    assert_eq!(west.route, "@eat/west_1");
    assert_eq!(west.title, "west_1");
    assert_eq!(west.status.as_deref(), Some("Liked"));
    assert_eq!(
        west.fields
            .iter()
            .map(|field| (field.key.as_str(), field.value.as_str()))
            .collect::<Vec<_>>(),
        [("category", "Pizza"), ("location", "Westfield")]
    );
    assert!(west.continuation.next_missing_field);
    assert_eq!(west.continuation.cursor, west.cursor_after);

    let explicit = "@eat/^west-1";
    let explicit_result = vault.complete(explicit, explicit.len());
    assert_ne!(
        explicit_result.context,
        Some(CompletionContext::RecordTarget),
        "explicit-new stems keep the create path"
    );
}

#[test]
fn record_target_menu_skips_spaced_stems() {
    let vault = RecordVault::new("bob-cli-complete-record-spaced");
    vault.write_note("nagi", "Ramen", "Clark");
    write_file(
        &vault.path().join("eat/Ramen Nagi.md"),
        "---\ntitle: Ramen Nagi\nstatus: liked\ncategory: Ramen\nlocation: Clark\n---\n# Ramen Nagi\n",
    );
    let raw = "@eat/";
    let result = vault.complete(raw, raw.len());
    assert_eq!(result.context, Some(CompletionContext::RecordTarget));
    let Candidates::RecordTarget(rows) = &result.candidates else {
        panic!("expected record targets: {result:?}");
    };
    assert!(
        !rows.iter().any(|row| row.stem == "Ramen Nagi"),
        "spaced stem is not addressable: {rows:?}"
    );
    assert!(rows.iter().any(|row| row.stem == "nagi"), "{rows:?}");
}

#[test]
fn update_field_assistance_uses_current_values_without_legacy_prompts() {
    let vault = RecordVault::new("bob-cli-complete-record-update-assist");
    vault.write_note("nagi", "Ramen", "\"Clark\"");

    let field_draft = "@eat/nagi\n- sta";
    let fields = vault.complete(field_draft, field_draft.len());
    assert_eq!(fields.context, Some(CompletionContext::RecordField));
    let Candidates::RecordField(rows) = &fields.candidates else {
        panic!("expected field candidates: {fields:?}");
    };
    let status = rows
        .iter()
        .find(|row| row.key == "status")
        .expect("status field");
    assert_eq!(status.current.as_deref(), Some("Liked"));
    assert!(!status.defaulted, "updates do not apply create defaults");

    let value_draft = "@eat/nagi\n- status:: ";
    let values = vault.complete(value_draft, value_draft.len());
    assert_eq!(values.context, Some(CompletionContext::RecordValue));
    assert_eq!(
        values
            .record
            .as_ref()
            .and_then(|record| record.current.as_deref()),
        Some("Liked")
    );

    let explicit_empty = vault.next_missing(value_draft, value_draft.len());
    assert_eq!(
        explicit_empty.context,
        Some(CompletionContext::RecordValue),
        "an explicitly empty required edit remains correctable"
    );

    write_file(
        &vault.path().join("eat/legacy.md"),
        "---\ntitle: Legacy\nstatus: liked\n---\n# Legacy\n",
    );
    let legacy = "@eat/legacy";
    let next = vault.next_missing(legacy, legacy.len());
    assert!(
        next.context.is_none(),
        "untouched old gaps are warnings only"
    );
    assert!(next.candidates.is_empty());
}

#[test]
fn field_menu_lists_required_keys_first_on_a_fresh_dash() {
    let vault = RecordVault::new("bob-cli-complete-record-fields");
    let raw = "Ramen Nagi @eat/\n- ";
    let value = vault.complete(raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RecordField));
    let Candidates::RecordField(rows) = &value.candidates else {
        panic!("expected field candidates: {value:?}");
    };
    let keys: Vec<&str> = rows.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(&keys[..3], ["status", "category", "location"], "{keys:?}");
    assert!(rows[0].required && rows[1].required && rows[2].required);
    assert!(keys.contains(&"visited"));
    assert_eq!(rows[0].replacement, "status:: ");
    assert!(!rows[0].continuation.next_missing_field);
    let applied = apply(raw, &value, &rows[0].replacement);
    assert_eq!(applied, "Ramen Nagi @eat/\n- status:: ");
}

#[test]
fn status_value_menu_offers_schema_choices() {
    let vault = RecordVault::new("bob-cli-complete-record-status");
    let raw = "Ramen Nagi @eat/\n- status:: ";
    let value = vault.complete(raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    assert_eq!(
        value.record.as_ref().map(|meta| meta.field.as_deref()),
        Some(Some("status"))
    );
    let rows = value_rows(&value);
    let values: Vec<&str> = rows.iter().map(|row| row.value.as_str()).collect();
    assert_eq!(values, ["to_try", "liked"], "{values:?}");
    assert_eq!(rows[1].display, "Liked");
    assert_eq!(rows[1].kind, "choices");
}

#[test]
fn next_missing_walks_status_then_category_then_location() {
    let vault = RecordVault::new("bob-cli-complete-record-chain");
    vault.write_note("nagi", "Ramen", "\"Clark\"");
    let mut draft = "Ramen Nagi @eat/".to_string();
    let status = vault.next_missing(&draft, draft.len());
    assert_eq!(status.context, Some(CompletionContext::RecordValue));
    let liked = find_value(value_rows(&status), "liked");
    assert!(liked
        .continuation
        .as_ref()
        .is_some_and(|c| c.next_missing_field));
    draft = apply(&draft, &status, &liked.replacement);
    assert_eq!(draft, "Ramen Nagi @eat/\n- status:: liked");

    let category =
        vault.next_missing(&draft, liked.continuation.as_ref().unwrap().cursor);
    let ramen = find_value(value_rows(&category), "Ramen");
    assert!(ramen
        .continuation
        .as_ref()
        .is_some_and(|c| c.next_missing_field));
    draft = apply(&draft, &category, &ramen.replacement);
    assert_eq!(
        draft,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen"
    );

    let location =
        vault.next_missing(&draft, ramen.continuation.as_ref().unwrap().cursor);
    let clark = find_value(value_rows(&location), "Clark");
    assert!(clark.continuation.is_none(), "{clark:?}");
    draft = apply(&draft, &location, &clark.replacement);
    assert_eq!(
        draft,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark"
    );
}

#[test]
fn westfield_and_wikilink_alias_collapse_to_the_majority_spelling() {
    let vault = RecordVault::new("bob-cli-complete-record-westfield");
    vault.write_note("a", "Pizza", "Westfield");
    vault.write_note("b", "Pizza", "Westfield");
    vault.write_note("c", "Pizza", "\"[[nj_towns#^town|westfield]]\"");
    let raw = "Slice @eat/\n- location:: ";
    let value = vault.complete(raw, raw.len());
    let rows = value_rows(&value);
    let westfield: Vec<&str> = rows
        .iter()
        .filter(|row| {
            row.display.eq_ignore_ascii_case("westfield")
                || row.value.to_lowercase().contains("westfield")
        })
        .map(|row| row.value.as_str())
        .collect();
    assert_eq!(westfield, ["Westfield"], "{rows:?}");
    assert_eq!(find_value(rows, "Westfield").count, Some(3));
}

#[test]
fn near_value_hint_warns_without_rewriting_cheeseteaks() {
    let vault = RecordVault::new("bob-cli-complete-record-near");
    vault.write_note("one", "Cheesesteaks", "\"Clark\"");
    vault.write_note("two", "Cheesesteaks", "\"Clark\"");
    vault.write_note("three", "Cheesesteaks", "\"Clark\"");
    let raw = "Pat @eat/\n- category:: Cheeseteaks";
    let value = vault.complete(raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    assert!(
        value.diagnostics.iter().any(|diag| {
            diag.code == "record_value_near"
                && diag.authored.as_deref() == Some("Cheeseteaks")
                && diag.proposed.as_deref() == Some("Cheesesteaks")
        }),
        "{:?}",
        value.diagnostics
    );
    assert!(value.diagnostics.iter().all(|diag| diag.code != "rewrite"));
}

#[test]
fn empty_section_query_prepends_record_create_and_expands_the_token() {
    let vault = RecordVault::new("bob-cli-complete-record-section");
    write_file(&vault.path().join("eat.md"), "# Ideas\n## Tasks\n");
    let raw = "@eat#";
    let value = vault.complete(raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::Section));
    assert_eq!(value.replacement, Replacement { start: 0, end: 5 });
    let Candidates::Section(rows) = &value.candidates else {
        panic!("expected section candidates: {value:?}");
    };
    let SectionMenuRow::Create(create) = &rows[0] else {
        panic!("expected create row first: {rows:?}");
    };
    assert_eq!(create.kind, "record_create");
    assert_eq!(rows[0].replacement(), "@eat/ ");
    assert_eq!(create.replacement, "@eat/ ");
    assert_eq!(create.label, "🍽️ New restaurant · eat/");
    assert_eq!(
        create.continuation,
        Some(CandidateContinuation {
            cursor: 6,
            next_missing_field: true,
        })
    );
    let headings: Vec<&str> = rows
        .iter()
        .filter_map(SectionMenuRow::as_section)
        .map(|row| row.replacement.as_str())
        .collect();
    assert!(headings.contains(&"@eat#Ideas"), "{headings:?}");
}

#[test]
fn typed_section_query_keeps_headings_and_omits_the_create_row() {
    let vault = RecordVault::new("bob-cli-complete-record-section-query");
    write_file(&vault.path().join("eat.md"), "# Ideas\n");
    let raw = "@eat#Id";
    let value = vault.complete(raw, raw.len());
    let Candidates::Section(rows) = &value.candidates else {
        panic!("expected section candidates: {value:?}");
    };
    assert!(
        rows.iter().all(|row| row.as_section().is_some()),
        "{rows:?}"
    );
    assert_eq!(rows[0].as_section().unwrap().replacement, "@eat#Ideas");
}

#[test]
fn global_destination_section_menu_has_no_create_row() {
    let vault = RecordVault::new("bob-cli-complete-record-global-section");
    write_file(&vault.path().join("eat.md"), "# Ideas\n");
    let raw = "@@eat#";
    let value = vault.complete(raw, raw.len());
    if let Candidates::Section(rows) = &value.candidates {
        assert!(
            rows.iter().all(|row| row.as_section().is_some()),
            "{rows:?}"
        );
    }
}

#[test]
fn ordinary_draft_does_not_open_the_record_catalog() {
    let vault = RecordVault::new("bob-cli-complete-record-fast-path");
    vault.write_note("nagi", "Ramen", "\"Clark\"");
    let raw = "Buy milk";
    let value = vault.complete(raw, raw.len());
    assert!(value.context.is_none(), "{value:?}");
    assert_eq!(last_catalog_files_opened(), 0);
}

#[test]
fn opaque_field_values_do_not_open_the_type_picker() {
    let vault = RecordVault::new("bob-cli-complete-record-opaque");
    let raw = "Ramen Nagi @eat/\n- category:: #fi";
    let value = vault.complete(raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
}

#[test]
fn empty_url_value_has_one_candidate_and_no_continuation() {
    let vault = RecordVault::new("bob-cli-complete-record-url");
    let raw = "Ramen Nagi @eat/\n- website:: ";
    let value = vault.complete(raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    let rows = value_rows(&value);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].value, "");
    assert!(rows[0].continuation.is_none());
}

#[test]
fn date_menu_offers_today_then_yesterday() {
    let vault = RecordVault::new("bob-cli-complete-record-dates");
    let raw = "Ramen Nagi @eat/\n- visited:: ";
    let value = vault.complete(raw, raw.len());
    let values: Vec<&str> = value_rows(&value)
        .iter()
        .map(|row| row.value.as_str())
        .collect();
    assert_eq!(values, ["today", "yesterday"], "{values:?}");
}

#[test]
fn crlf_next_missing_inserts_crlf_bullets() {
    let vault = RecordVault::new("bob-cli-complete-record-crlf");
    let draft = "Ramen Nagi @eat/\r\n- keep this note";
    let status = vault.next_missing(draft, draft.len());
    let liked = find_value(value_rows(&status), "liked");
    assert!(liked.replacement.contains("\r\n"), "{}", liked.replacement);
    let applied = apply(draft, &status, &liked.replacement);
    assert_eq!(
        applied,
        "Ramen Nagi @eat/\r\n- status:: liked\r\n- keep this note"
    );
}

#[test]
fn list_member_completion_replaces_only_the_member_at_the_caret() {
    let vault = RecordVault::new("bob-cli-complete-record-list");
    write_file(
        &vault.path().join("eat/nagi.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: Nagi\nstatus: liked\ncategory: Ramen\nlocation: Clark\n\
         tags: soup\n---\n# Nagi\n",
    );
    let raw = "Ramen Nagi @eat/\n- tags:: hot, so";
    let value = vault.complete(raw, raw.len());
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    let soup = find_value(value_rows(&value), "soup");
    assert_eq!(soup.replacement, "soup");
    let applied = apply(raw, &value, &soup.replacement);
    assert_eq!(applied, "Ramen Nagi @eat/\n- tags:: hot, soup");
}

#[test]
fn empty_remaining_keys_leave_note_bullets_alone() {
    let vault = RecordVault::new("bob-cli-complete-record-notes");
    let raw = "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n\
               - location:: Clark\n- leftover";
    let value = vault.complete(raw, raw.len());
    assert_ne!(value.context, Some(CompletionContext::RecordField));
}

#[test]
fn template_argument_rows_are_not_record_items() {
    let vault = RecordVault::new("bob-cli-complete-record-template");
    let raw = "Buy {0} @eat/ %t\n\nrow";
    let cursor = raw.len();
    let value = vault.complete(raw, cursor);
    assert!(value.context.is_none(), "{value:?}");
    let next = vault.next_missing(raw, cursor);
    assert!(next.context.is_none(), "{next:?}");
    let named = vault.named_field(raw, cursor, "status");
    assert!(named.context.is_none(), "{named:?}");
    assert!(named.record.is_none());
}

#[test]
fn human_output_names_record_field_and_value_rows() {
    let vault = RecordVault::new("bob-cli-complete-record-human");
    let raw = "Ramen Nagi @eat/\n- ";
    let fields = vault.complete(raw, raw.len());
    let lines = candidate_lines(&fields.candidates, fields.context);
    assert!(
        lines.iter().any(|(key, detail)| {
            key == "status" && detail.contains("required")
        }),
        "{lines:?}"
    );
    let raw = "Ramen Nagi @eat/\n- status:: ";
    let values = vault.complete(raw, raw.len());
    let lines = candidate_lines(&values.candidates, values.context);
    assert!(
        lines.iter().any(|(display, _)| display == "Liked"),
        "{lines:?}"
    );
}

#[test]
fn shell_record_route_strips_the_trailing_space() {
    let vault = RecordVault::new("bob-cli-complete-record-shell");
    vault.write_note("nagi", "Ramen", "\"Clark\"");
    let _guard = TestEnvGuard::set(&[
        ("BOB_CONFIG_FILE", Some(vault.config.as_os_str())),
        ("BOB_NOW", Some(OsStr::new(NOW))),
    ]);
    let completion = shell_completion(vault.path(), "@", 1)
        .expect("shell")
        .expect("rows");
    let eat = completion
        .rows
        .iter()
        .find(|row| row.full == "@eat/")
        .unwrap_or_else(|| panic!("missing @eat/: {completion:?}"));
    assert!(!eat.nospace, "{eat:?}");
    assert_eq!(eat.group, "records");
}

#[test]
fn named_field_opens_an_unfiltered_value_menu_without_a_bullet() {
    let vault = RecordVault::new("bob-cli-complete-named-status");
    let raw = "Ramen Nagi @eat/";
    let value = vault.named_field(raw, raw.len(), "status");
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    let meta = value.record.as_ref().expect("record");
    assert_eq!(meta.field.as_deref(), Some("status"));
    assert_eq!(meta.assist_version, Some(RECORD_ASSIST_VERSION));
    let rows = value_rows(&value);
    let values: Vec<&str> = rows.iter().map(|row| row.value.as_str()).collect();
    assert_eq!(values, ["to_try", "liked"], "{values:?}");
    let liked = find_value(rows, "liked");
    assert!(liked
        .continuation
        .as_ref()
        .is_some_and(|c| c.next_missing_field));
    let applied = apply(raw, &value, &liked.replacement);
    assert_eq!(applied, "Ramen Nagi @eat/\n- status:: liked");
}

#[test]
fn named_field_edit_authors_an_empty_optional_bullet() {
    let vault = RecordVault::new("bob-cli-complete-named-optional");
    let raw = "Ramen Nagi @eat/\n- leftover note";
    let value = vault.named_field(raw, raw.len(), "visited");
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    let dates: Vec<&str> = value_rows(&value)
        .iter()
        .map(|row| row.value.as_str())
        .collect();
    assert_eq!(dates, ["today", "yesterday"], "{dates:?}");
    let edit = record_edit(&value);
    assert!(!edit.continuation.next_missing_field);
    let applied = apply_edit(raw, edit);
    assert_eq!(applied, "Ramen Nagi @eat/\n- visited:: \n- leftover note");
    assert_eq!(
        &applied[..edit.cursor_after],
        "Ramen Nagi @eat/\n- visited:: "
    );
    let again = vault.complete(&applied, edit.cursor_after);
    assert_eq!(again.context, Some(CompletionContext::RecordValue));
    assert_eq!(
        again.record.as_ref().and_then(|meta| meta.field.as_deref()),
        Some("visited")
    );
}

#[test]
fn named_field_edit_materializes_the_default_spelling() {
    let vault = RecordVault::new("bob-cli-complete-named-default");
    let raw = "Ramen Nagi @eat/";
    let value = vault.named_field(raw, raw.len(), "companions");
    let edit = record_edit(&value);
    let applied = apply_edit(raw, edit);
    assert_eq!(applied, "Ramen Nagi @eat/\n- companions:: [[kelly]]");
    assert_eq!(&applied[edit.cursor_after..], "[[kelly]]");
    assert!(!edit.continuation.next_missing_field);
}

#[test]
fn named_field_empty_suggested_text_folder_still_has_edit() {
    let vault = RecordVault::new("bob-cli-complete-named-empty-folder");
    let raw = "Ramen Nagi @eat/";
    let value = vault.named_field(raw, raw.len(), "category");
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    assert!(value_rows(&value).is_empty(), "{value:?}");
    let edit = record_edit(&value);
    let applied = apply_edit(raw, edit);
    assert_eq!(applied, "Ramen Nagi @eat/\n- category:: ");
}

#[test]
fn named_field_on_an_invalid_value_is_unfiltered() {
    let vault = RecordVault::new("bob-cli-complete-named-invalid");
    let raw = "Ramen Nagi @eat/\n- status:: bogus";
    let caret = vault.complete(raw, raw.len());
    assert!(
        value_rows(&caret).iter().all(|row| row.value != "liked"),
        "caret completion should filter by the authored value: {caret:?}"
    );
    let value = vault.named_field(raw, raw.len(), "status");
    let rows = value_rows(&value);
    assert_eq!(
        rows.iter()
            .map(|row| row.value.as_str())
            .collect::<Vec<_>>(),
        ["to_try", "liked"]
    );
    let edit = record_edit(&value);
    assert_eq!(apply_edit(raw, edit), raw);
    assert_eq!(&raw[edit.cursor_after..], "bogus");
}

#[test]
fn named_field_preserves_an_explicit_empty() {
    let vault = RecordVault::new("bob-cli-complete-named-empty");
    let raw = "Ramen Nagi @eat/\n- visited:: ";
    let value = vault.named_field(raw, raw.len(), "visited");
    let edit = record_edit(&value);
    assert_eq!(apply_edit(raw, edit), raw);
    let today = find_value(value_rows(&value), "today");
    let applied = apply(raw, &value, &today.replacement);
    assert_eq!(applied, "Ramen Nagi @eat/\n- visited:: today");
}

#[test]
fn named_field_list_uses_caret_member_else_the_final_member() {
    let vault = RecordVault::new("bob-cli-complete-named-list");
    write_file(
        &vault.path().join("eat/nagi.md"),
        "---\nparent: \"[[eat]]\"\ntype: \"[[restaurant]]\"\n\
         title: Nagi\nstatus: liked\ncategory: Ramen\nlocation: Clark\n\
         tags: soup\n---\n# Nagi\n",
    );
    let raw = "Ramen Nagi @eat/\n- tags:: hot, so";
    let in_first = raw.find("hot").expect("hot");
    let first = vault.named_field(raw, in_first, "tags");
    let soup = find_value(value_rows(&first), "soup");
    assert_eq!(
        apply(raw, &first, &soup.replacement),
        "Ramen Nagi @eat/\n- tags:: soup, so"
    );
    let last = vault.named_field(raw, raw.find('@').expect("route"), "tags");
    let soup = find_value(value_rows(&last), "soup");
    assert_eq!(
        apply(raw, &last, &soup.replacement),
        "Ramen Nagi @eat/\n- tags:: hot, soup"
    );
}

#[test]
fn named_field_crlf_inserts_crlf_bullets() {
    let vault = RecordVault::new("bob-cli-complete-named-crlf");
    let draft = "Ramen Nagi @eat/\r\n- keep this note";
    let value = vault.named_field(draft, draft.len(), "status");
    let liked = find_value(value_rows(&value), "liked");
    assert!(liked.replacement.contains("\r\n"), "{}", liked.replacement);
    assert_eq!(
        apply(draft, &value, &liked.replacement),
        "Ramen Nagi @eat/\r\n- status:: liked\r\n- keep this note"
    );
    let edit = record_edit(&value);
    assert!(edit.replacement.contains("\r\n"), "{}", edit.replacement);
    assert_eq!(
        apply_edit(draft, edit),
        "Ramen Nagi @eat/\r\n- status:: \r\n- keep this note"
    );
}

#[test]
fn named_field_unicode_prefix_keeps_byte_offsets() {
    let vault = RecordVault::new("bob-cli-complete-named-unicode");
    let raw = "🎉 party\n\nRamen Nagi @eat/";
    let value = vault.named_field(raw, raw.len(), "status");
    let liked = find_value(value_rows(&value), "liked");
    assert_eq!(
        apply(raw, &value, &liked.replacement),
        "🎉 party\n\nRamen Nagi @eat/\n- status:: liked"
    );
}

#[test]
fn named_field_targets_the_record_item_containing_the_cursor() {
    let vault = RecordVault::new("bob-cli-complete-named-multi");
    let raw = "First @eat/\n\nSecond @eat/";
    let first_cursor = raw.find('\n').expect("split");
    let first = vault.named_field(raw, first_cursor, "status");
    let liked = find_value(value_rows(&first), "liked");
    assert_eq!(
        apply(raw, &first, &liked.replacement),
        "First @eat/\n- status:: liked\n\nSecond @eat/"
    );
    let second = vault.named_field(raw, raw.len(), "status");
    let liked = find_value(value_rows(&second), "liked");
    assert_eq!(
        apply(raw, &second, &liked.replacement),
        "First @eat/\n\nSecond @eat/\n- status:: liked"
    );
}

#[test]
fn named_field_unknown_is_read_only() {
    let vault = RecordVault::new("bob-cli-complete-named-unknown");
    let raw = "Ramen Nagi @eat/";
    let value = vault.named_field(raw, raw.len(), "occasion");
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    assert!(value_rows(&value).is_empty());
    assert!(value
        .record
        .as_ref()
        .is_some_and(|meta| meta.edit.is_none()));
    assert!(
        value.diagnostics.iter().any(|diag| {
            diag.code == "record_unknown_field"
                && diag.field.as_deref() == Some("occasion")
        }),
        "{:?}",
        value.diagnostics
    );
    assert_eq!(apply(raw, &value, ""), raw);
}

#[test]
fn named_field_on_a_non_record_is_empty_success() {
    let vault = RecordVault::new("bob-cli-complete-named-non-record");
    let raw = "Buy milk";
    let value = vault.named_field(raw, raw.len(), "status");
    assert!(value.context.is_none(), "{value:?}");
    assert!(value.record.is_none());
    assert_eq!(last_catalog_files_opened(), 0);
}

#[test]
fn named_field_does_not_fall_into_task_completion() {
    let vault = RecordVault::new("bob-cli-complete-named-task-link");
    let raw = "Follow up :";
    let value = vault.named_field(raw, raw.len(), "status");
    assert!(value.context.is_none(), "{value:?}");
    assert!(value.record.is_none());
    assert!(
        matches!(value.candidates, Candidates::Route(ref rows) if rows.is_empty())
    );
}

#[test]
fn named_field_malformed_config_is_read_only() {
    let vault = RecordVault::new("bob-cli-complete-named-malformed");
    write_file(&vault.config, "records: []\n");
    let raw = "Ramen Nagi @eat/";
    let value = vault.named_field(raw, raw.len(), "status");
    assert!(value.context.is_none(), "{value:?}");
    assert!(value.record.is_none());
    assert!(
        value.diagnostics.iter().any(|diag| {
            diag.code == RECORDS_CONFIG_INVALID_CODE
                && diag.field.as_deref() == Some("status")
        }),
        "{:?}",
        value.diagnostics
    );
    assert_eq!(apply(raw, &value, ""), raw);
}

#[test]
fn named_field_missing_title_still_addresses_the_field() {
    let vault = RecordVault::new("bob-cli-complete-named-title");
    let raw = "@eat/";
    let value = vault.named_field(raw, raw.len(), "status");
    assert_eq!(value.context, Some(CompletionContext::RecordValue));
    assert!(find_value(value_rows(&value), "liked")
        .continuation
        .as_ref()
        .is_some_and(|c| c.next_missing_field));
}

#[test]
fn caret_and_next_missing_advertise_assist_version_without_edit() {
    let vault = RecordVault::new("bob-cli-complete-assist-version");
    let raw = "Ramen Nagi @eat/\n- status:: ";
    let caret = vault.complete(raw, raw.len());
    let meta = caret.record.as_ref().expect("record");
    assert_eq!(meta.assist_version, Some(RECORD_ASSIST_VERSION));
    assert!(meta.edit.is_none());
    let next = vault.next_missing("Ramen Nagi @eat/", "Ramen Nagi @eat/".len());
    let meta = next.record.as_ref().expect("record");
    assert_eq!(meta.assist_version, Some(RECORD_ASSIST_VERSION));
    assert!(meta.edit.is_none());
}
