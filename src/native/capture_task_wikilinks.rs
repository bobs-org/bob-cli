//! `((query` Task Wikilink completion: field detection and the vault-wide
//! `wikilink_task` snapshot.
//!
//! `((` is a draft-only trigger. Accepting a row inserts a plain Obsidian
//! block link (`[[target#^id]]`); the vault never stores `((`. Bob owns
//! the trigger grammar, the candidates, the link text, and the ID write.

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
        .last()
}

fn is_whole_item_action(item: &capture_language::EditorItemParse) -> bool {
    matches!(
        item.mode,
        EditorMode::PomodoroAdjust
            | EditorMode::PomodoroShift
            | EditorMode::PomodoroClose
            | EditorMode::PomodoroStart
            | EditorMode::TaskComplete
            | EditorMode::Ref
    ) || item.pomodoro_close.is_some()
        || item.pomodoro_start.is_some()
        || matches!(
            item.needs.as_slice(),
            [Need::TaskLink]
                | [Need::TaskGoto]
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
            }];
            if token.query_end > token.opener + 2 {
                spans.push(Span {
                    start: token.opener + 2,
                    end: token.query_end,
                    kind: SpanKind::WikilinkTaskQuery,
                });
            }
            if let Some(closer) = token.closer {
                spans.push(Span {
                    start: closer,
                    end: closer + 2,
                    kind: SpanKind::WikilinkTaskDelimiter,
                });
            }
            spans
        })
        .collect()
}

fn wikilink_task_tokens(raw: &str) -> Vec<WikilinkTaskToken> {
    let bytes = raw.as_bytes();
    let mut tokens = Vec::new();
    for line in capture_language::split_physical_lines(raw) {
        let mut index = line.start;
        while index + 1 < line.end {
            if bytes[index] == b'(' && bytes[index + 1] == b'(' {
                if is_valid_opener(raw, line.start, index, line.end)
                    && let Some(token) = token_from_opener(raw, index, line.end)
                    && !capture_language::cursor_in_close_log_text(
                        raw,
                        index + 2,
                    )
                    && !capture_language::editor_item_at(raw, index + 2)
                        .is_some_and(|item| is_whole_item_action(&item))
                {
                    tokens.push(token);
                }
                index += 1;
            } else {
                index += 1;
            }
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
