//! Block-ID completion contract for `capture-complete`.
//!
//! Additive `block_id` object (intent, marker range, body, allowed-character
//! rule, used IDs, suggestions) backing the Block ID Picker. Bob stays the
//! only authority for grammar, candidates, intent, used IDs, and suggestions.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use serde::Serialize;

use super::{
    capture,
    capture_language::{
        editor_item_at, parse_for_editor, project_task_block_id_detail,
        CompletionContext,
    },
    collect_done, note_tasks,
};

/// Regex matching exactly one allowed block-ID character, for both `:` and
/// `^`. Mirrors [`collect_done::is_block_id_byte`]: ASCII letters, digits,
/// and `-`.
pub(crate) const BLOCK_ID_ALLOWED_CHARACTER: &str = "[A-Za-z0-9-]";
/// Human wording from the block-ID validator errors.
pub(crate) const BLOCK_ID_ALLOWED_DESCRIPTION: &str = "A-Z, a-z, 0-9 or '-'";

/// Which thing the person can mean on the right-hand side of `@route:`/`@route^`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockIdIntent {
    Link,
    New,
    ProjectNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BlockIdMarkerRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct UsedBlockId {
    pub(crate) id: String,
    pub(crate) line: usize,
    pub(crate) task: bool,
    pub(crate) status_symbol: Option<char>,
    pub(crate) status_name: Option<String>,
    pub(crate) text: String,
}

/// Additive top-level `block_id` object, present exactly when the context is
/// `pomodoro_block_id`, `task_block_id`, or `project_task_block_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BlockIdField {
    pub(crate) route: String,
    pub(crate) relative_target: String,
    pub(crate) note_exists: bool,
    pub(crate) marker: String,
    pub(crate) marker_range: BlockIdMarkerRange,
    pub(crate) intent: BlockIdIntent,
    pub(crate) body: String,
    pub(crate) allowed_character: String,
    pub(crate) allowed_description: String,
    pub(crate) suggestions: Vec<String>,
    pub(crate) used: Vec<UsedBlockId>,
}

pub(crate) struct BlockIdRequest<'a> {
    pub(crate) route: &'a str,
    pub(crate) replacement: (usize, usize),
    pub(crate) context: CompletionContext,
}

/// Whether `byte` is allowed in a block ID for either marker. Mirrors
/// [`collect_done::is_block_id_byte`] so the contract can never drift from
/// the validator again.
pub(crate) fn is_block_id_byte(byte: u8) -> bool {
    collect_done::is_block_id_byte(byte)
}

pub(crate) fn allowed_rule_for_marker(
    _marker: char,
) -> (&'static str, &'static str) {
    (BLOCK_ID_ALLOWED_CHARACTER, BLOCK_ID_ALLOWED_DESCRIPTION)
}

pub(crate) fn is_valid_block_id(_marker: char, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    id.bytes().all(is_block_id_byte)
}

/// Build the additive `block_id` object for a `pomodoro_block_id` or
/// `task_block_id` field. Reads the routed note (missing is not an error).
pub(crate) fn build_block_id_field(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
    request: &BlockIdRequest<'_>,
) -> BlockIdField {
    if request.context == CompletionContext::ProjectTaskBlockId {
        return build_project_task_block_id_field(
            bob_dir, raw_text, cursor, request,
        );
    }
    let marker = match request.context {
        CompletionContext::PomodoroBlockId => ':',
        _ => '^',
    };
    let route = request.route.to_ascii_lowercase();
    let relative_target = capture::route_label(&route);
    let (allowed_character, allowed_description) =
        allowed_rule_for_marker(marker);
    let marker_range = marker_token_range(raw_text, request.replacement);
    // Only the `^` marker carries the project-note intent (a `+` after the
    // replacement). A `+` after a `:` block ID is the retired project-note
    // form, which offers no completion.
    let project_note = marker == '^'
        && raw_text
            .get(request.replacement.1..)
            .is_some_and(|rest| rest.starts_with('+'));
    let intent = if project_note {
        BlockIdIntent::ProjectNote
    } else if marker == '^' {
        BlockIdIntent::New
    } else {
        detect_colon_intent(raw_text, cursor, request)
    };
    let body = editor_item_at(raw_text, cursor)
        .map(|item| item.body)
        .unwrap_or_default();

    let target = bob_dir.join(&relative_target);
    let contents = std::fs::read_to_string(&target).ok();
    let note_exists = contents.is_some();
    let mut used = Vec::new();
    if note_exists
        && intent != BlockIdIntent::ProjectNote
        && let Some(contents) = &contents
    {
        let settings = note_tasks::read_settings(bob_dir);
        used = collect_used(contents, &settings);
    }
    let used_ids: Vec<String> =
        used.iter().map(|entry| entry.id.clone()).collect();
    let suggestions = if !note_exists
        || body.is_empty()
        || matches!(intent, BlockIdIntent::Link)
    {
        Vec::new()
    } else {
        suggest_ids(&body, marker, &used_ids)
    };
    // `used` is `[]` for `project_note` intent.
    let used = if intent == BlockIdIntent::ProjectNote {
        Vec::new()
    } else {
        used
    };

    BlockIdField {
        route,
        relative_target,
        note_exists,
        marker: marker.to_string(),
        marker_range,
        intent,
        body,
        allowed_character: allowed_character.to_string(),
        allowed_description: allowed_description.to_string(),
        suggestions,
        used,
    }
}

/// Build the additive `block_id` object for a `project_task_block_id`
/// field: a trailing ` :id` / ` ^id` on a project-note task bullet. Intent
/// is always `new`. The routed note is never read beyond setting
/// `note_exists` for the project note itself; `used` is `prj` plus every
/// other accepted task ID already typed in the item.
fn build_project_task_block_id_field(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
    request: &BlockIdRequest<'_>,
) -> BlockIdField {
    let Some(detail) =
        project_task_block_id_detail(raw_text, cursor, request.replacement)
    else {
        return BlockIdField {
            route: request.route.to_string(),
            relative_target: format!("{}.md", request.route),
            note_exists: false,
            marker: "^".to_string(),
            marker_range: BlockIdMarkerRange {
                start: request.replacement.0,
                end: request.replacement.1,
            },
            intent: BlockIdIntent::New,
            body: String::new(),
            allowed_character: BLOCK_ID_ALLOWED_CHARACTER.to_string(),
            allowed_description: BLOCK_ID_ALLOWED_DESCRIPTION.to_string(),
            suggestions: Vec::new(),
            used: Vec::new(),
        };
    };
    let (allowed_character, allowed_description) =
        allowed_rule_for_marker(detail.marker);
    let relative_target = format!("{}.md", detail.stem);
    let note_exists = bob_dir.join(&relative_target).is_file();
    let used_ids: Vec<String> =
        detail.used.iter().map(|entry| entry.id.clone()).collect();
    let suggestions = if detail.body.is_empty() {
        Vec::new()
    } else {
        suggest_ids(&detail.body, detail.marker, &used_ids)
    };
    BlockIdField {
        route: detail.stem,
        relative_target,
        note_exists,
        marker: detail.marker.to_string(),
        marker_range: BlockIdMarkerRange {
            start: detail.marker_range.0,
            end: detail.marker_range.1,
        },
        intent: BlockIdIntent::New,
        body: detail.body,
        allowed_character: allowed_character.to_string(),
        allowed_description: allowed_description.to_string(),
        suggestions,
        used: detail
            .used
            .into_iter()
            .map(|entry| UsedBlockId {
                id: entry.id,
                line: entry.line,
                task: true,
                status_symbol: None,
                status_name: None,
                text: entry.text,
            })
            .collect(),
    }
}

/// Whole marker token range from `@` through any suffix: the whitespace-free
/// token holding the replacement. Falls back to the replacement itself.
fn marker_token_range(
    raw_text: &str,
    replacement: (usize, usize),
) -> BlockIdMarkerRange {
    let bytes = raw_text.as_bytes();
    let mut start = replacement.0;
    while start > 0 {
        let prev = start - 1;
        if !raw_text.is_char_boundary(prev) {
            start = prev;
            continue;
        }
        let byte = bytes[prev];
        if byte.is_ascii_whitespace() {
            break;
        }
        start = prev;
    }
    let mut end = replacement.1;
    // Include a directly attached project-note sigil.
    if raw_text
        .get(end..)
        .is_some_and(|rest| rest.starts_with('+'))
    {
        end += 1;
    }
    while end < raw_text.len() {
        if !raw_text.is_char_boundary(end) {
            end += 1;
            continue;
        }
        let byte = bytes[end];
        if byte.is_ascii_whitespace() {
            break;
        }
        // Include the `#name` and `=<X>`/`=x` tail so highlighting covers
        // the whole marker token.
        end += 1;
    }
    BlockIdMarkerRange { start, end }
}

/// `link` exactly when Bob's own whole-item parse would classify the item as
/// `pomodoro_link` once a valid ID fills the part. Substitutes a placeholder
/// ID into the replacement range and re-runs the editor parser so intent can
/// never drift from capture semantics.
fn detect_colon_intent(
    raw_text: &str,
    cursor: usize,
    request: &BlockIdRequest<'_>,
) -> BlockIdIntent {
    let original_index = match editor_item_at(raw_text, cursor) {
        Some(item) => item.index,
        None => return BlockIdIntent::New,
    };
    let (start, end) = request.replacement;
    if start > raw_text.len() || end > raw_text.len() || start > end {
        return BlockIdIntent::New;
    }
    let mut modified = String::with_capacity(raw_text.len() + 1);
    modified.push_str(&raw_text[..start]);
    modified.push('x');
    modified.push_str(&raw_text[end..]);
    let parsed = parse_for_editor(&modified);
    let matches = parsed
        .items
        .iter()
        .find(|item| item.index == original_index)
        .is_some_and(|item| {
            (item.mode == super::capture_language::EditorMode::PomodoroLink
                && item.diagnostics.is_empty())
                || (item.mode
                    == super::capture_language::EditorMode::Incomplete
                    && item.needs
                        == vec![
                            super::capture_language::Need::PomodoroCloseTask,
                        ])
        });
    if matches {
        BlockIdIntent::Link
    } else {
        BlockIdIntent::New
    }
}

/// Every ID `block_ids_in_markdown` finds, deduplicated by first occurrence,
/// in document order, with 1-based lines.
pub(crate) fn collect_used(
    contents: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> Vec<UsedBlockId> {
    let scan = note_tasks::scan(contents, settings);
    let mut seen: HashMap<String, UsedBlockId> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (line_index, line) in contents.split('\n').enumerate() {
        let line_text = line.strip_suffix('\r').unwrap_or(line);
        let Some(id) = collect_done::trailing_block_id_in_line(line_text)
        else {
            continue;
        };
        if seen.contains_key(&id) {
            continue;
        }
        let line_no = line_index + 1;
        let entry = match scan.task_at(line_index) {
            Some(task) if task.block_id.as_deref() == Some(id.as_str()) => {
                UsedBlockId {
                    id: id.clone(),
                    line: line_no,
                    task: true,
                    status_symbol: Some(task.status_symbol),
                    status_name: Some(task.status_name.clone()),
                    text: task.description.clone(),
                }
            }
            _ => UsedBlockId {
                id: id.clone(),
                line: line_no,
                task: false,
                status_symbol: None,
                status_name: None,
                text: nontask_text(line_text, &id),
            },
        };
        order.push(id.clone());
        seen.insert(id, entry);
    }
    order
        .into_iter()
        .filter_map(|id| seen.remove(&id))
        .collect()
}

fn nontask_text(line: &str, id: &str) -> String {
    let trimmed = line.trim_end();
    let without_id = trimmed
        .strip_suffix(&format!("^{id}"))
        .unwrap_or(trimmed)
        .trim_end();
    let without_marker = strip_list_marker(without_id);
    const LIMIT: usize = 160;
    let trimmed = without_marker.trim();
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_string();
    }
    trimmed.chars().take(LIMIT).collect()
}

fn strip_list_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest;
        }
        if trimmed == prefix.trim_end() {
            return "";
        }
    }
    let digit_len = trimmed
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_len > 0 {
        let after_digits = &trimmed[digit_len..];
        let mut chars = after_digits.chars();
        if matches!(chars.next(), Some('.') | Some(')'))
            && chars.next().is_some_and(|next| next.is_whitespace())
        {
            return after_digits[1..].trim_start();
        }
    }
    trimmed
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "be", "by", "for", "from", "in", "into",
    "is", "it", "its", "my", "of", "on", "or", "our", "so", "that", "the",
    "their", "this", "to", "via", "with", "your",
];

const LEADING_VERBS: &[&str] = &[
    "add",
    "build",
    "check",
    "clean",
    "create",
    "delete",
    "document",
    "enable",
    "ensure",
    "finish",
    "fix",
    "implement",
    "improve",
    "investigate",
    "make",
    "migrate",
    "move",
    "plan",
    "read",
    "refactor",
    "remove",
    "rename",
    "research",
    "review",
    "run",
    "start",
    "stop",
    "support",
    "test",
    "try",
    "update",
    "use",
    "write",
];

fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word)
}

/// ASCII alphanumeric runs, lowercased. Non-ASCII characters are separators.
fn words_in(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() {
            current.push(byte.to_ascii_lowercase() as char);
        } else if !current.is_empty() {
            if !is_stopword(&current) {
                words.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if !current.is_empty() && !is_stopword(&current) {
        words.push(current);
    }
    words
}

fn phrase_spans(body: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    let bytes = body.as_bytes();
    let mut index = 0;
    while index < body.len() {
        if !body.is_char_boundary(index) {
            index += 1;
            continue;
        }
        let byte = bytes[index];
        if byte == b'`' {
            if let Some(close) = body[index + 1..].find('`') {
                phrases.push(body[index + 1..index + 1 + close].to_string());
                index = index + 1 + close + 1;
                continue;
            }
            index += 1;
        } else if byte == b'"' || body[index..].starts_with('“') {
            let open_len = if body[index..].starts_with('“') {
                3
            } else {
                1
            };
            let rest = &body[index + open_len..];
            let mut close_at = None;
            let mut offset = 0;
            while offset < rest.len() {
                if !rest.is_char_boundary(offset) {
                    offset += 1;
                    continue;
                }
                if rest[offset..].starts_with('"')
                    || rest[offset..].starts_with('”')
                {
                    close_at = Some(offset);
                    break;
                }
                offset += 1;
            }
            if let Some(close) = close_at {
                phrases.push(rest[..close].to_string());
                let close_len = if rest[close..].starts_with('”') {
                    3
                } else {
                    1
                };
                index = index + open_len + close + close_len;
                continue;
            }
            index += open_len;
        } else if byte == b'(' {
            if let Some(close) = body[index + 1..].find(')') {
                phrases.push(body[index + 1..index + 1 + close].to_string());
                index = index + 1 + close + 1;
                continue;
            }
            index += 1;
        } else if body[index..].starts_with("[[") {
            if let Some(close) = body[index + 2..].find("]]") {
                let inner = &body[index + 2..index + 2 + close];
                phrases.push(wikilink_phrase(inner));
                index = index + 2 + close + 2;
                continue;
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    phrases
}

fn wikilink_phrase(inner: &str) -> String {
    if let Some((_, alias)) = inner.split_once('|') {
        return alias.to_string();
    }
    let target = inner.split_once('#').map_or(inner, |(target, _)| target);
    target.rsplit('/').next().unwrap_or(target).to_string()
}

fn join_truncated(words: &[String], marker: char) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let joined = words.join("-");
    let truncated = if joined.len() <= 32 {
        joined
    } else {
        let prefix = &joined[..32];
        match prefix.rfind('-') {
            Some(cut) if cut > 0 => joined[..cut].to_string(),
            _ => return None,
        }
    };
    if truncated.is_empty() || !is_valid_block_id(marker, &truncated) {
        return None;
    }
    Some(truncated)
}

/// Suggestions for `new` and `project_note` intents with a non-empty body:
/// at most 3, deterministic. Pure function.
pub(crate) fn suggest_ids(
    body: &str,
    marker: char,
    used_ids: &[String],
) -> Vec<String> {
    let used: HashSet<&str> = used_ids.iter().map(String::as_str).collect();
    let mut candidates: Vec<String> = Vec::new();

    let prose = words_in(body);
    // Candidate 1: the first phrase with at least one word, as its first 4.
    for phrase in phrase_spans(body) {
        let words = words_in(&phrase);
        if words.is_empty() {
            continue;
        }
        let take = words.len().min(4);
        if let Some(joined) = join_truncated(&words[..take], marker) {
            candidates.push(joined);
        }
        break;
    }
    // Candidate 2: the first 3 prose words.
    if !prose.is_empty() {
        let take = prose.len().min(3);
        if let Some(joined) = join_truncated(&prose[..take], marker) {
            candidates.push(joined);
        }
        // Candidate 3: when the first prose word is a leading verb, the
        // next 3 words.
        if LEADING_VERBS.contains(&prose[0].as_str()) && prose.len() > 1 {
            let end = (1 + 3).min(prose.len());
            if let Some(joined) = join_truncated(&prose[1..end], marker) {
                candidates.push(joined);
            }
        }
    }

    // Deduplicate, preserving order.
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for candidate in candidates {
        if seen.insert(candidate.clone()) {
            deduped.push(candidate);
        }
    }
    // A candidate already in `used` takes the first free `-2`…`-9` suffix,
    // or is dropped when none is free. Membership is exact.
    let mut out = Vec::new();
    for candidate in deduped {
        if !used.contains(candidate.as_str()) {
            out.push(candidate);
        } else {
            let mut placed = None;
            for suffix in 2..=9 {
                let suffixed = format!("{candidate}-{suffix}");
                if suffixed.len() > 64 {
                    continue;
                }
                if !is_valid_block_id(marker, &suffixed) {
                    continue;
                }
                if !used.contains(suffixed.as_str())
                    && !seen.contains(&suffixed)
                {
                    placed = Some(suffixed);
                    break;
                }
            }
            if let Some(next) = placed {
                seen.insert(next.clone());
                out.push(next);
            }
        }
        if out.len() >= 3 {
            break;
        }
    }
    out.truncate(3);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> note_tasks::NoteTaskSettings {
        note_tasks::read_settings(Path::new("/tmp/bob-cli-no-vault"))
    }

    #[test]
    fn allowed_regex_agrees_with_validator_for_every_ascii_char() {
        let regex =
            regex::Regex::new(&format!("^{}$", BLOCK_ID_ALLOWED_CHARACTER))
                .expect("valid block-ID regex");
        for byte in 0..128u8 {
            let text = (byte as char).to_string();
            assert_eq!(
                regex.is_match(&text),
                collect_done::is_block_id_byte(byte),
                "block-ID rule drift for {byte:#04X}"
            );
        }
        for marker in [':', '^'] {
            assert_eq!(
                allowed_rule_for_marker(marker),
                (BLOCK_ID_ALLOWED_CHARACTER, BLOCK_ID_ALLOWED_DESCRIPTION),
                "marker {marker:?} must share one rule"
            );
        }
        assert!(!is_valid_block_id(':', "foo_bar"));
        assert!(is_valid_block_id(':', "foo-bar"));
        assert!(!is_valid_block_id('^', "foo_bar"));
    }

    #[test]
    fn suggestions_follow_the_pinned_examples() {
        let cases = [
            (
                "Fix flaky gkeep test",
                '^',
                vec!["fix-flaky-gkeep", "flaky-gkeep-test"],
            ),
            (
                "Add concept of \"agent data panels”!",
                '^',
                vec![
                    "agent-data-panels",
                    "add-concept-agent",
                    "concept-agent-data",
                ],
            ),
            (
                "Add support for new `%hold` directive!",
                '^',
                vec!["hold", "add-support-new", "support-new-hold"],
            ),
            ("Release v0.18.0!", '^', vec!["release-v0-18"]),
            ("Tool", '^', vec!["tool-2"]),
        ];
        for (body, marker, expected) in cases {
            let used = if body == "Tool" {
                vec!["tool".to_string()]
            } else {
                vec![]
            };
            assert_eq!(
                suggest_ids(body, marker, &used),
                expected.into_iter().map(str::to_string).collect::<Vec<_>>(),
                "body {body:?}"
            );
        }
        assert!(suggest_ids("é🚀", '^', &[]).is_empty());
    }

    #[test]
    fn used_covers_done_nontask_duplicates_and_document_order() {
        let contents = "# S\n- [ ] #task First ^bbb\n- [x] #task Done ^aaa\npara ^ccc\n- [ ] #task Again ^bbb\n";
        let used = collect_used(contents, &settings());
        let ids: Vec<&str> =
            used.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, vec!["bbb", "aaa", "ccc"]);
        assert_eq!(used[0].line, 2);
        assert!(used[0].task);
        assert_eq!(used[1].line, 3);
        assert!(used[1].task);
        assert_eq!(used[2].line, 4);
        assert!(!used[2].task);
        assert_eq!(used[2].text, "para");
    }
}
