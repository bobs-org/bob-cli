//! Capture-marker slot for shell completion: TEXT on `capture`,
//! `capture-parse`, and `capture-rewrite`.
//!
//! The slot builds `raw_text` from the TEXT words before the cursor (joined
//! with single spaces, mirroring `bob capture`'s convention) plus the cursor
//! prefix, with `cursor = raw_text.len()`, then calls the in-process
//! [`capture_complete`](crate::native::capture_complete) extraction. Release
//! gates: boundary (marker in the cursor word), safe rows only (handled
//! inside the extraction), end of word only (non-empty `--suffix` yields
//! nothing, replacement must end at the cursor), and no wikilinks (deferred
//! before `NoteIndex::read`). Presentation uses the `!prefix` directive with
//! a Unicode-scalar count.

use std::ffi::{OsStr, OsString};

use super::{context::Context, protocol};
use crate::native::capture_complete;

/// Complete capture markers for one TEXT cursor.
///
/// `before` holds every word before the cursor (command word excluded, like
/// the presenter's walk) for `--bob-dir` resolution; `text_start` is the
/// index in `before` where TEXT began (`None` when no TEXT word precedes the
/// cursor). Returns `None` when no marker applies so the caller can fall
/// back (empty cursor) or show nothing (TEXT already started).
pub(crate) fn capture_text_lines(
    before: &[OsString],
    text_start: Option<usize>,
    cursor: &OsStr,
    suffix: Option<&OsString>,
) -> Option<Vec<String>> {
    // End of the active word only: a non-empty suffix means the cursor sits
    // in the middle of the word.
    if suffix.is_some_and(|suffix| !suffix.is_empty()) {
        return Some(Vec::new());
    }
    let cursor_text = cursor.to_string_lossy();
    if cursor_text.contains(['\n', '\r']) {
        return Some(Vec::new());
    }
    let text_words: &[OsString] = match text_start {
        Some(start) if start <= before.len() => &before[start..],
        _ => &[],
    };
    let mut raw_text = text_words
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let cursor_prefix = cursor_text.into_owned();
    if raw_text.is_empty() {
        raw_text = cursor_prefix;
    } else if cursor_prefix.is_empty() {
        raw_text.push(' ');
    } else {
        raw_text.push(' ');
        raw_text.push_str(&cursor_prefix);
    }
    // The cursor word may itself contain spaces (a single quoted shell word
    // like `fix it @dev:`): it is still the tail of `raw_text`.
    let cursor_bytes = cursor.to_string_lossy().len();
    let cursor_word_start = raw_text.len().saturating_sub(cursor_bytes);
    if !raw_text.is_char_boundary(cursor_word_start) {
        return Some(Vec::new());
    }
    let slot = Context::parse(before);
    let cursor_offset = raw_text.len();
    let completion = match capture_complete::shell_completion(
        &slot.bob_dir,
        &raw_text,
        cursor_offset,
    ) {
        Ok(completion) => completion?,
        Err(_) => return Some(Vec::new()),
    };
    // Boundary gate: the marker must start inside the cursor word.
    if completion.marker_start < cursor_word_start
        || completion.marker_start > raw_text.len()
        || !raw_text.is_char_boundary(completion.marker_start)
    {
        return Some(Vec::new());
    }
    if completion.rows.is_empty() {
        return Some(Vec::new());
    }
    let Some(keep_slice) =
        raw_text.get(cursor_word_start..completion.marker_start)
    else {
        return Some(Vec::new());
    };
    let keep = keep_slice.chars().count();
    let mut lines = Vec::new();
    if keep > 0 {
        lines.push(protocol::prefix_line(keep));
    }
    for row in &completion.rows {
        if let Some(line) = protocol::candidate_line(
            OsStr::new(&row.full),
            &row.description,
            &row.group,
            row.nospace,
        ) {
            lines.push(line);
        }
    }
    Some(lines)
}
