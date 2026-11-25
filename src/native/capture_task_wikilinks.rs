//! `((query` Task Wikilink completion: field detection and the vault-wide
//! `wikilink_task` snapshot.
//!
//! `((` is a draft and prose-field trigger. Accepting a row inserts a plain
//! Obsidian block link (`[[target#^id]]`); the vault never stores `((`. Bob
//! owns the trigger grammar, the candidates, the link text, and the ID
//! write. Capture drafts use [`completion_field_at`]; plain prose fields
//! (`capture-complete --links-only --task-links`) use
//! [`prose_completion_field_at`], which drops every capture-grammar check.

use std::path::Path;

use chrono::NaiveDate;
use serde::Serialize;

use super::{
    capture_dependency_tasks, capture_language,
    capture_language::{EditorMode, Need, Span, SpanKind},
    capture_links, capture_tasks, env as bob_env, pomodoro, task_dependencies,
    task_status_hooks,
};

/// The `((query` field under the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WikilinkTaskField {
    pub(crate) query: String,
    pub(crate) replacement: (usize, usize),
}

/// One `wikilink_task` candidate. Field names match the relation /
/// dependency rows so the Mac's unified decoder stays unchanged, plus
/// additive `cursor_after` and `link_target`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WikilinkTaskCandidate {
    pub(crate) replacement: String,
    #[serde(rename = "ref")]
    pub(crate) task_ref: String,
    pub(crate) note_path: String,
    pub(crate) locator: String,
    pub(crate) group: String,
    pub(crate) hidden: bool,
    pub(crate) block_id: Option<String>,
    pub(crate) requires_block_id: bool,
    pub(crate) block_id_suggestions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disabled_reason: Option<String>,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
    pub(crate) status_type: &'static str,
    pub(crate) text: String,
    pub(crate) section: Option<String>,
    pub(crate) depth: usize,
    pub(crate) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) task_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cursor_after: Option<usize>,
    pub(crate) link_target: String,
}

const QUERY_STOP: &[u8] = b"()[]`\n\r";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WikilinkTaskToken {
    opener: usize,
    query_end: usize,
    run_end: usize,
    closer: Option<usize>,
}

/// Locate the `((query` field at `cursor`, if any. `current_note_path`
/// is the draft item's destination (vault-relative, with `.md`) and is
/// unused for detection; grouping uses it at candidate time.
pub(crate) fn completion_field_at(
    raw: &str,
    cursor: usize,
    _current_note_path: &str,
) -> Option<WikilinkTaskField> {
    if cursor > raw.len() || !raw.is_char_boundary(cursor) {
        return None;
    }
    wikilink_task_tokens(raw)
        .into_iter()
        .filter_map(|token| field_at_token(raw, token, cursor))
        .next_back()
}

/// Locate the `((query` field at `cursor` in plain prose (`capture-complete
/// --links-only --task-links`). These are the capture-draft token rules
/// minus every capture-grammar check: prose has no items, Work Log text,
/// or whole-item actions, so `x ((foo` and `- 1 ((foo` open here. The
/// shared `prose_trigger_vectors.json` pins these rules for every port.
pub(crate) fn prose_completion_field_at(
    raw: &str,
    cursor: usize,
) -> Option<WikilinkTaskField> {
    if cursor > raw.len() || !raw.is_char_boundary(cursor) {
        return None;
    }
    tokens_where(raw, |_| true)
        .into_iter()
        .filter_map(|token| field_at_token(raw, token, cursor))
        .next_back()
}

fn is_whole_item_action(item: &capture_language::EditorItemParse) -> bool {
    matches!(
        item.mode,
        EditorMode::PomodoroAdjust
            | EditorMode::PomodoroShift
            | EditorMode::PomodoroClose
            | EditorMode::PomodoroPast
            | EditorMode::PomodoroStart
            | EditorMode::TaskComplete
            | EditorMode::TaskGoto
            | EditorMode::Ref
    ) || item.pomodoro_close.is_some()
        || item.pomodoro_past.is_some()
        || item.pomodoro_start.is_some()
        || matches!(
            item.needs.as_slice(),
            [Need::TaskLink]
                | [Need::TaskGoto]
                | [Need::NoteGoto]
                | [Need::TaskComplete]
                | [Need::ActiveTask]
        )
}

/// Parse-visible trigger spans for the same tokens that completion uses.
pub(crate) fn wikilink_task_spans(raw: &str) -> Vec<Span> {
    wikilink_task_tokens(raw)
        .into_iter()
        .flat_map(|token| {
            let mut spans = vec![Span {
                start: token.opener,
                end: token.opener + 2,
                kind: SpanKind::WikilinkTaskDelimiter,
                project_note: None,
                title: None,
                key: None,
                section_action: None,
            }];
            if token.query_end > token.opener + 2 {
                spans.push(Span {
                    start: token.opener + 2,
                    end: token.query_end,
                    kind: SpanKind::WikilinkTaskQuery,
                    project_note: None,
                    title: None,
                    key: None,
                    section_action: None,
                });
            }
            if let Some(closer) = token.closer {
                spans.push(Span {
                    start: closer,
                    end: closer + 2,
                    kind: SpanKind::WikilinkTaskDelimiter,
                    project_note: None,
                    title: None,
                    key: None,
                    section_action: None,
                });
            }
            spans
        })
        .collect()
}

/// Capture-draft tokens: the prose tokens minus those in Work Log text or
/// on a whole-item action.
fn wikilink_task_tokens(raw: &str) -> Vec<WikilinkTaskToken> {
    tokens_where(raw, |opener| {
        !capture_language::cursor_in_close_log_text(raw, opener + 2)
            && !capture_language::editor_item_at(raw, opener + 2)
                .is_some_and(|item| is_whole_item_action(&item))
    })
}

/// Every valid `((` token per physical line whose opener also passes
/// `keep`.
fn tokens_where(
    raw: &str,
    keep: impl Fn(usize) -> bool,
) -> Vec<WikilinkTaskToken> {
    let bytes = raw.as_bytes();
    let mut tokens = Vec::new();
    for line in capture_language::split_physical_lines(raw) {
        let mut index = line.start;
        while index + 1 < line.end {
            if bytes[index] == b'('
                && bytes[index + 1] == b'('
                && is_valid_opener(raw, line.start, index, line.end)
                && let Some(token) = token_from_opener(raw, index, line.end)
                && keep(index)
            {
                tokens.push(token);
            }
            index += 1;
        }
    }
    tokens
}

fn is_valid_opener(
    raw: &str,
    line_start: usize,
    opener: usize,
    line_end: usize,
) -> bool {
    let bytes = raw.as_bytes();
    let at_line_start = opener == line_start;
    let after_space =
        opener > line_start && matches!(bytes[opener - 1], b' ' | b'\t');
    if !at_line_start && !after_space {
        return false;
    }
    if opener + 2 < line_end && bytes[opener + 2] == b'(' {
        return false;
    }
    !capture_links::is_protected_markup_range(raw, opener, opener + 2)
}

fn token_from_opener(
    raw: &str,
    opener: usize,
    line_end: usize,
) -> Option<WikilinkTaskToken> {
    let inner_start = opener + 2;
    let bytes = raw.as_bytes();
    let mut index = inner_start;
    while index < line_end {
        let byte = bytes[index];
        if byte == b')' {
            if index + 1 < line_end && bytes[index + 1] == b')' {
                return Some(WikilinkTaskToken {
                    opener,
                    query_end: index,
                    run_end: index,
                    closer: Some(index),
                });
            }
            break;
        }
        if QUERY_STOP.contains(&byte) {
            break;
        }
        index += 1;
    }
    Some(WikilinkTaskToken {
        opener,
        query_end: index,
        run_end: index,
        closer: None,
    })
}

fn field_at_token(
    raw: &str,
    token: WikilinkTaskToken,
    cursor: usize,
) -> Option<WikilinkTaskField> {
    if cursor < token.opener {
        return None;
    }
    let inner_start = token.opener + 2;
    if let Some(closer) = token.closer {
        let closer_end = closer + 2;
        if cursor > closer_end {
            return None;
        }
        let query_end = cursor.clamp(inner_start, closer);
        return Some(WikilinkTaskField {
            query: raw.get(inner_start..query_end)?.to_string(),
            replacement: (token.opener, closer_end),
        });
    }
    if cursor > token.run_end {
        return None;
    }
    let query_end = cursor.max(inner_start);
    Some(WikilinkTaskField {
        query: raw.get(inner_start..query_end)?.to_string(),
        replacement: (
            token.opener,
            non_whitespace_run_end(raw, cursor, token.run_end),
        ),
    })
}

fn non_whitespace_run_end(raw: &str, cursor: usize, run_end: usize) -> usize {
    let bytes = raw.as_bytes();
    let mut end = cursor.min(run_end);
    while end < run_end && !bytes[end].is_ascii_whitespace() {
        end += 1;
    }
    end
}

/// Vault-wide `wikilink_task` snapshot for `field`, grouped against
/// `owner_note` (vault-relative path with `.md`).
pub(crate) fn candidates(
    bob_dir: &Path,
    field: &WikilinkTaskField,
    owner_note: &str,
) -> (Vec<WikilinkTaskCandidate>, Vec<String>) {
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let previous_daily = previous_daily_note(bob_dir, &discovered);
    let ordered = capture_dependency_tasks::order_for_picker(
        &discovered.tasks,
        &field.query,
        Some(owner_note),
    );
    let rows = ordered
        .into_iter()
        .map(|task| {
            let link_target = task_dependencies::format::cross_note_link_target(
                Path::new(&task.note_path),
                &discovered.index,
            );
            let requires_block_id = task.block_id.is_none();
            let disabled_reason =
                disabled_reason(bob_dir, task, previous_daily.as_deref());
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                task_dependencies::format::cross_note_link(
                    Path::new(&task.note_path),
                    task.block_id.as_deref().expect("identified wikilink task"),
                    &discovered.index,
                )
            };
            let cursor_after = (!replacement.is_empty())
                .then_some(field.replacement.0 + replacement.len());
            WikilinkTaskCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: capture_dependency_tasks::group_for(
                    task,
                    Some(owner_note),
                )
                .to_string(),
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                task_kind: task.task_kind,
                cursor_after,
                link_target,
            }
        })
        .collect();
    (rows, discovered.warnings)
}

fn disabled_reason(
    bob_dir: &Path,
    task: &capture_dependency_tasks::DependencyTask,
    previous_daily: Option<&str>,
) -> Option<String> {
    if task.duplicate_id {
        return Some(format!(
            "duplicate block ID ^{} in {}",
            task.block_id.as_deref().unwrap_or_default(),
            task.note_path
        ));
    }
    if task.block_id.is_some() {
        return None;
    }
    if task_dependencies::is_archive_path(Path::new(&task.note_path)) {
        return Some("archived tasks can't get an id".to_string());
    }
    if previous_daily == Some(task.note_path.as_str()) {
        return Some("the previous daily note is read-only".to_string());
    }
    let absolute = bob_dir.join(&task.note_path);
    if absolute
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.permissions().readonly())
    {
        return Some("read-only note".to_string());
    }
    None
}

fn previous_daily_note(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
) -> Option<String> {
    let anchor = task_status_hooks::daily_anchor_date(
        &pomodoro::day_file_for(bob_dir),
        bob_env::current_datetime().date(),
    );
    let mut previous: Option<(NaiveDate, String)> = None;
    let mut seen = std::collections::HashSet::new();
    for paths in discovered.basenames.values() {
        for path in paths {
            if !seen.insert(path) {
                continue;
            }
            if let Some(date) =
                task_status_hooks::canonical_daily_date(Path::new(path))
                && date < anchor
                && previous.as_ref().is_none_or(|(day, _)| date > *day)
            {
                previous = Some((date, path.clone()));
            }
        }
    }
    previous.map(|(_, path)| path)
}

/// Backend-formatted `[[target#^id]]` for a just-assigned block ID.
pub(crate) fn wikilink_replacement(
    index: &super::vault_links::NoteIndex,
    target: &Path,
    block_id: &str,
) -> String {
    task_dependencies::format::cross_note_link(target, block_id, index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field_at(raw: &str, cursor: usize) -> Option<WikilinkTaskField> {
        completion_field_at(raw, cursor, "mac_inbox.md")
    }

    #[test]
    fn opener_at_line_start() {
        let raw = "((s";
        let found = field_at(raw, raw.len()).expect("field");
        assert_eq!(found.query, "s");
        assert_eq!(found.replacement, (0, 3));
    }

    #[test]
    fn opener_after_space() {
        let raw = "Follow up ((s";
        let found = field_at(raw, raw.len()).expect("field");
        assert_eq!(found.query, "s");
        assert_eq!(found.replacement, (10, 13));
    }

    #[test]
    fn mid_word_never_triggers() {
        assert!(field_at("f((x", 4).is_none());
    }

    #[test]
    fn triple_paren_never_triggers() {
        assert!(field_at("(((foo", 6).is_none());
        assert!(field_at(" (((foo", 7).is_none());
    }

    #[test]
    fn closed_token_covers_closer() {
        let raw = "Follow up ((foo)) more";
        let opener = raw.find("((").expect("opener");
        let found = field_at(raw, opener + 4).expect("inside foo");
        assert_eq!(found.query, "fo");
        assert_eq!(found.replacement, (opener, opener + 7));
        let at_start = field_at(raw, opener).expect("at start");
        assert_eq!(at_start.query, "");
        assert_eq!(at_start.replacement, (opener, opener + 7));
    }

    #[test]
    fn mid_word_caret_does_not_swallow_following_text() {
        let raw = "Follow up ((foobar baz";
        let opener = raw.find("((").expect("opener");
        let cursor = opener + 2 + 3; // inside "foobar"
        let found = field_at(raw, cursor).expect("field");
        assert_eq!(found.query, "foo");
        assert_eq!(found.replacement, (opener, opener + 2 + 6));
    }

    #[test]
    fn forbidden_query_chars_reject_the_field() {
        assert!(field_at("Follow up ((fo(o", 16).is_none());
        assert!(field_at("Follow up ((fo[o", 16).is_none());
        assert!(field_at("Follow up ((fo`o", 16).is_none());
    }

    #[test]
    fn unclosed_token_stops_before_punctuation_and_preserves_suffix() {
        let raw = "Follow up ((foo and [existing](url)";
        let found = field_at(raw, 15).expect("field before link suffix");
        assert_eq!(found.query, "foo");
        assert_eq!(found.replacement, (10, 15));
        let spliced = format!("{}[[target#^id]]{}", &raw[..10], &raw[15..]);
        assert!(spliced.ends_with(" and [existing](url)"));

        for raw in ["Follow up ((foo) bar", "Follow up ((foo`code`)"] {
            let found = field_at(raw, 15).expect("field before punctuation");
            assert_eq!(found.query, "foo");
            assert_eq!(found.replacement, (10, 15));
        }
    }

    #[test]
    fn closed_token_covers_caret_through_end_of_closer() {
        let raw = "((foo))";
        for cursor in [0, 2, 4, 5, 6, 7] {
            let found =
                field_at(raw, cursor).expect("field through closer end");
            assert_eq!(found.replacement, (0, 7));
        }
        assert!(field_at(raw, 8).is_none());
    }

    #[test]
    fn inside_wikilink_is_protected() {
        assert!(field_at("See [[note ((x", 14).is_none());
    }

    #[test]
    fn whole_item_actions_request_no_field() {
        for raw in ["=", "x", "+5", "--", "!", ":", "/", "^sase:id"] {
            assert!(
                field_at(raw, raw.len()).is_none(),
                "action {raw:?} should not be a field"
            );
        }
    }

    #[test]
    fn crlf_draft_keeps_byte_offsets() {
        let raw = "Follow up ((s\r\nnext";
        let found = field_at(raw, 13).expect("field");
        assert_eq!(found.query, "s");
        assert_eq!(found.replacement, (10, 13));
    }

    #[test]
    fn multibyte_prefix_uses_byte_offsets() {
        let raw = "Café ((s";
        let opener = raw.find("((").expect("opener");
        let found = field_at(raw, raw.len()).expect("field");
        assert_eq!(found.query, "s");
        assert_eq!(found.replacement, (opener, raw.len()));
    }

    #[test]
    fn at_sigil_in_query_stays_wikilink_task() {
        let raw = "((@foo";
        let found = field_at(raw, raw.len()).expect("field");
        assert_eq!(found.query, "@foo");
        assert_eq!(found.replacement, (0, raw.len()));
    }

    #[test]
    fn work_log_child_is_not_a_field() {
        let raw = "x\n- Follow up ((s";
        assert!(field_at(raw, raw.len()).is_none());
    }

    #[test]
    fn prose_opens_where_capture_grammar_suppresses() {
        for raw in
            ["x ((foo", "- 1 ((foo", "x\n- Follow up ((s", "@inbox ((foo"]
        {
            let prose = prose_completion_field_at(raw, raw.len())
                .unwrap_or_else(|| panic!("prose field for {raw:?}"));
            let opener = raw.rfind("((").expect("opener");
            assert_eq!(prose.replacement, (opener, raw.len()), "{raw:?}");
        }
        let work_log = "x\n- Follow up ((s";
        assert!(field_at(work_log, work_log.len()).is_none());
    }

    #[test]
    fn prose_keeps_the_token_rules() {
        for raw in ["f((x", "(((x", "See [[note ((x", "see `((x`"] {
            assert!(
                prose_completion_field_at(raw, raw.len()).is_none(),
                "{raw:?}"
            );
        }
        let raw = "Superseded by ((api do";
        let found =
            prose_completion_field_at(raw, raw.len() - 3).expect("prose field");
        assert_eq!(found.query, "api");
        assert_eq!(found.replacement, (14, 19));
    }

    #[test]
    fn prose_rejects_cursor_off_a_char_boundary() {
        let raw = "Café ((s";
        assert!(prose_completion_field_at(raw, 4).is_none());
        assert!(prose_completion_field_at(raw, raw.len() + 1).is_none());
    }

    /// The shared prose trigger contract: bob-plugins replays the same
    /// bytes through its own `fieldAt` port, so every case is written as
    /// strings, never offsets. If bob's behavior differs from a
    /// hand-written expectation, read the code and decide which is wrong.
    #[test]
    fn prose_trigger_vectors_match() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/task_wikilinks/prose_trigger_vectors.json"
        ))
        .expect("prose trigger vectors parse");
        assert_eq!(vectors["version"], 1);
        assert_eq!(
            vectors["source"],
            "tests/fixtures/task_wikilinks/prose_trigger_vectors.json"
        );
        let cases = vectors["cases"].as_array().expect("cases array");
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().expect("name");
            let before = case["before"].as_str().expect("before");
            let after = case["after"].as_str().expect("after");
            let raw = format!("{before}{after}");
            let actual = prose_completion_field_at(&raw, before.len());
            let expected = &case["expect"];
            if expected.is_null() {
                assert!(actual.is_none(), "{name}: expected no field");
                continue;
            }
            let field =
                actual.unwrap_or_else(|| panic!("{name}: expected a field"));
            assert_eq!(
                field.query,
                expected["query"].as_str().expect("query"),
                "{name}: query"
            );
            let replaced = &raw[field.replacement.0..field.replacement.1];
            assert_eq!(
                replaced,
                expected["replaced"].as_str().expect("replaced"),
                "{name}: replaced"
            );
            // A replacement ends in `))` exactly when the token is
            // closed: an unclosed query stops before any `)`.
            assert_eq!(
                replaced.ends_with("))"),
                expected["closed"].as_bool().expect("closed"),
                "{name}: closed"
            );
        }
    }

    #[test]
    fn task_spans_cover_exactly_the_completion_fields() {
        // Mirrors the trigger/non-trigger corpus in
        // tests/cli/capture/complete_wikilink_task.rs, plus punctuation
        // boundaries. The @ selector case is omitted because it contains a
        // capture marker inside the query and capture spans intentionally win.
        let drafts = [
            "((",
            "Follow up ((s",
            "Follow up ((foo)) more",
            "Follow up ((foobar baz",
            "Follow up ((s\r\nnext",
            "Café ((s",
            "First item\n\nFollow up ((s",
            "Do work @sase:id ((s",
            "Follow up ((foo and [existing](url)",
            "Follow up ((foo) bar",
            "Follow up ((foo`code`)",
            "f((x",
            "(((foo",
            "See [[note ((x",
            "Follow up ((fo(o",
            "=",
            "x",
            "+5",
            "--",
            "!",
            ":",
            "/",
            "^sase:id",
            "x\n- Follow up ((s",
        ];
        for raw in drafts {
            let spans = wikilink_task_spans(raw);
            for cursor in raw
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(raw.len()))
            {
                let in_spans = spans
                    .iter()
                    .any(|span| cursor >= span.start && cursor <= span.end);
                assert_eq!(
                    field_at(raw, cursor).is_some(),
                    in_spans,
                    "span/field mismatch for {raw:?} at byte {cursor}: {spans:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod vectors_tests {
    use super::super::{
        capture_dependency_tasks, task_dependencies::format as link_format,
        vault_links::NoteIndex,
    };
    use super::*;
    use std::ffi::OsStr;

    static TASK_WIKILINK_VECTORS: std::sync::LazyLock<serde_json::Value> =
        std::sync::LazyLock::new(|| {
            serde_json::from_str(include_str!(
                "../../tests/fixtures/task_wikilinks/vectors.json"
            ))
            .expect("wikilink vectors parse")
        });

    fn expect_text(value: &serde_json::Value) -> &str {
        value.as_str().expect("vector string")
    }

    fn expect_optional_text(value: &serde_json::Value) -> Option<&str> {
        if value.is_null() {
            None
        } else {
            Some(value.as_str().expect("vector string or null"))
        }
    }

    fn write_vector_vault(
        bob_dir: &Path,
        vault: &serde_json::Map<String, serde_json::Value>,
    ) {
        for (path, content) in vault {
            let full = bob_dir.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).expect("vector parent");
            }
            std::fs::write(&full, expect_text(content)).expect("vector file");
        }
    }

    /// The shared parity contract: every vector pins `candidates()` row
    /// order and fields plus `cross_note_link_target` for the row's
    /// note. If bob's behavior differs from a hand-written expectation,
    /// read the code and fix the vector, never bob.
    #[test]
    fn vectors_match_candidates_and_link_targets() {
        for vector in TASK_WIKILINK_VECTORS["vectors"]
            .as_array()
            .expect("vectors array")
        {
            let name = expect_text(&vector["name"]);
            let vault_dir = tempfile::tempdir().expect("vector vault");
            write_vector_vault(
                vault_dir.path(),
                vector["vault"].as_object().expect("vault map"),
            );
            let _now = crate::native::env::TestEnvGuard::set(&[(
                "BOB_NOW",
                Some(OsStr::new(expect_text(&vector["now"]))),
            )]);
            let field = WikilinkTaskField {
                query: expect_text(&vector["query"]).to_string(),
                replacement: (0, 0),
            };
            let owner = expect_text(&vector["owner"]);
            let (rows, _) = candidates(vault_dir.path(), &field, owner);
            let (note_paths, _) =
                capture_dependency_tasks::vault_note_paths(vault_dir.path());
            let index = NoteIndex::from_paths(note_paths);
            let expected = vector["rows"].as_array().expect("rows array");
            assert_eq!(rows.len(), expected.len(), "{name}: row count");
            for (row, want) in rows.iter().zip(expected) {
                assert_eq!(
                    row.note_path,
                    expect_text(&want["note_path"]),
                    "{name}"
                );
                assert_eq!(
                    row.line,
                    want["line"].as_u64().expect("line") as usize,
                    "{name}"
                );
                assert_eq!(row.group, expect_text(&want["group"]), "{name}");
                assert_eq!(
                    row.hidden,
                    want["hidden"].as_bool().expect("hidden"),
                    "{name}"
                );
                assert_eq!(
                    row.block_id.as_deref(),
                    expect_optional_text(&want["block_id"]),
                    "{name}"
                );
                assert_eq!(
                    row.locator,
                    expect_text(&want["locator"]),
                    "{name}"
                );
                assert_eq!(
                    row.link_target,
                    expect_text(&want["link_target"]),
                    "{name}"
                );
                assert_eq!(
                    link_format::cross_note_link_target(
                        Path::new(&row.note_path),
                        &index
                    ),
                    row.link_target,
                    "{name}: link target"
                );
                assert_eq!(
                    row.replacement,
                    expect_text(&want["replacement"]),
                    "{name}"
                );
                assert_eq!(
                    row.disabled_reason.as_deref(),
                    expect_optional_text(&want["disabled_reason"]),
                    "{name}"
                );
                let symbol = expect_text(&want["status_symbol"]);
                assert_eq!(
                    Some(row.status_symbol),
                    symbol.chars().next(),
                    "{name}"
                );
                assert_eq!(symbol.chars().count(), 1, "{name}");
                assert_eq!(row.text, expect_text(&want["text"]), "{name}");
            }
        }
    }
}
