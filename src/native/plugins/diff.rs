use similar::{ChangeTag, TextDiff};

use super::model::{DiffKind, DiffLine, FileDiff};

const DIFF_CONTEXT_LINES: usize = 3;
const DIFF_BODY_LINE_LIMIT: usize = 60;
pub(super) const MINIFIED_BYTE_THRESHOLD: usize = 16 * 1024;
const MINIFIED_LINE_THRESHOLD: usize = 2;

pub(super) fn diff_existing_file(old: &[u8], new: &[u8]) -> FileDiff {
    if is_binary_or_minified(old, new) {
        return FileDiff::Binary {
            old_len: old.len(),
            new_len: new.len(),
        };
    }

    let Ok(old_text) = std::str::from_utf8(old) else {
        return FileDiff::Binary {
            old_len: old.len(),
            new_len: new.len(),
        };
    };
    let Ok(new_text) = std::str::from_utf8(new) else {
        return FileDiff::Binary {
            old_len: old.len(),
            new_len: new.len(),
        };
    };

    diff_text(old_text, new_text)
}

fn is_binary_or_minified(old: &[u8], new: &[u8]) -> bool {
    if std::str::from_utf8(old).is_err() || std::str::from_utf8(new).is_err() {
        return true;
    }

    old.len().max(new.len()) >= MINIFIED_BYTE_THRESHOLD
        && (byte_line_count(old) <= MINIFIED_LINE_THRESHOLD
            || byte_line_count(new) <= MINIFIED_LINE_THRESHOLD)
}

pub(super) fn diff_text(old: &str, new: &str) -> FileDiff {
    let diff = TextDiff::from_lines(old, new);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }

    let mut lines = Vec::new();
    let mut unified = diff.unified_diff();
    unified.context_radius(DIFF_CONTEXT_LINES);
    for hunk in unified.iter_hunks() {
        lines.push(DiffLine {
            kind: DiffKind::Hunk,
            text: hunk.header().to_string(),
        });
        for change in hunk.iter_changes() {
            let kind = match change.tag() {
                ChangeTag::Insert => DiffKind::Add,
                ChangeTag::Delete => DiffKind::Del,
                ChangeTag::Equal => DiffKind::Context,
            };
            let value = change.value().trim_end_matches(['\r', '\n']);
            lines.push(DiffLine {
                kind,
                text: format!("{}{value}", change.tag()),
            });
        }
    }

    let hidden = lines.len().saturating_sub(DIFF_BODY_LINE_LIMIT);
    lines.truncate(DIFF_BODY_LINE_LIMIT);
    FileDiff::Text {
        lines,
        added,
        removed,
        hidden,
    }
}

pub(super) fn byte_line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let lines = bytes.iter().filter(|byte| **byte == b'\n').count();
    if bytes.ends_with(b"\n") {
        lines
    } else {
        lines + 1
    }
}
