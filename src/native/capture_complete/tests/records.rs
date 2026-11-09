//! Record field/value completion, route labels, and section create rows.

use super::super::{
    engine::{build_result, build_result_for, CompleteRequest},
    model::{
        CandidateContinuation, Candidates, CaptureCompleteResult,
        RecordValueCandidate, Replacement, RouteCandidateKind, SectionMenuRow,
    },
    records::last_catalog_files_opened,
    render::candidate_lines,
    shell::shell_completion,
};
use super::{write_file, TempDir};
use crate::native::{capture_language::CompletionContext, env::TestEnvGuard};
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
        self.complete_for(raw, cursor, false)
    }

    fn next_missing(&self, raw: &str, cursor: usize) -> CaptureCompleteResult {
        self.complete_for(raw, cursor, true)
    }

    fn complete_for(
        &self,
        raw: &str,
        cursor: usize,
        next_missing: bool,
    ) -> CaptureCompleteResult {
        let _guard = TestEnvGuard::set(&[
            ("BOB_CONFIG_FILE", Some(self.config.as_os_str())),
            ("BOB_NOW", Some(OsStr::new(NOW))),
        ]);
        if next_missing {
            build_result_for(&CompleteRequest {
                bob_dir: self.path(),
                raw_text: raw,
                cursor,
                all_tasks: false,
                closed_tasks: false,
                next_missing_field: true,
            })
            .expect("next-missing complete")
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
