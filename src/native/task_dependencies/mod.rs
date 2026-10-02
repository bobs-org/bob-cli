//! Task dependency lines: the `docs/task-dependencies.md` contract.
//!
//! A task's prerequisites live as plain task dependency links on one
//! managed first-child line (`- ⛓️ **DEPENDS ON:** [[#^a]] • [[#^b]]`).
//! This module owns the line parser ([`parse`]), the canonical formatter
//! and link form ([`format`]), and the R8 legacy-child recogniser
//! ([`legacy`]), plus the shared wikilink scanner every implementation
//! builds on.

use std::{
    ffi::OsStr,
    io,
    ops::Range,
    path::{Component, Path},
};

pub(crate) mod format;
pub(crate) mod legacy;
pub(crate) mod parse;

#[cfg(test)]
mod tests;

pub(crate) use legacy::legacy_child_reference;
pub(crate) use parse::{is_dependency_line, DependencyLine};

/// One `[[...]]` occurrence in a line of Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WikilinkSpan {
    /// Byte offset of the opening `[[`.
    pub(crate) open: usize,
    /// Byte offset just past the closing `]]`.
    pub(crate) end: usize,
}

/// A validated `[[target#^block-id]]` occurrence with its decorations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockLinkSpan {
    pub(crate) target: String,
    pub(crate) block_id: String,
    /// Byte offset of the opening `[[`.
    pub(crate) open: usize,
    /// Byte offset just past the closing `]]`.
    pub(crate) end: usize,
    /// Start of the full token, including a leading `!` when embedded.
    pub(crate) token_start: usize,
    pub(crate) embedded: bool,
    pub(crate) struck: bool,
    pub(crate) aliased: bool,
    /// Removal span: the token, widened to an exactly-wrapping `~~` pair.
    pub(crate) full_start: usize,
    /// Removal span end (see [`BlockLinkSpan::full_start`]).
    pub(crate) full_end: usize,
}

/// Raw `[[...]]` spans in a line, skipping inline code spans.
///
/// This is the shared link scanner: it replicates the historical
/// `task_status_hooks::block_link_occurrences` iteration (a `[[` search
/// followed by the next `]]`, advancing past each match) so every caller
/// agrees on what a wikilink is. Spans that overlap an inline code span
/// are ignored.
pub(crate) fn raw_wikilink_spans(line: &str) -> Vec<WikilinkSpan> {
    let code_spans = inline_code_spans(line);
    let mut spans = Vec::new();
    let mut rest = line;
    let mut base = 0;
    while let Some(open) = rest.find("[[") {
        let absolute_open = base + open;
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("]]") else {
            break;
        };
        let end = absolute_open + 2 + close + 2;
        if !code_spans
            .iter()
            .any(|span| absolute_open < span.end && end > span.start)
        {
            spans.push(WikilinkSpan {
                open: absolute_open,
                end,
            });
        }
        base = end;
        rest = &after_open[close + 2..];
    }
    spans
}

/// Validated block links in a line: aliases, `!`, and `~~` are parsed,
/// inline code spans are ignored, and only `#^block-id` targets with
/// valid block-id bytes are returned.
pub(crate) fn block_link_spans(line: &str) -> Vec<BlockLinkSpan> {
    let struck_spans = strikethrough_spans(line);
    let mut links = Vec::new();
    for span in raw_wikilink_spans(line) {
        let inside = &line[span.open + 2..span.end - 2];
        let link_target = inside.split('|').next().unwrap_or("");
        let Some(fragment) = link_target.find("#^") else {
            continue;
        };
        let target = link_target[..fragment].trim();
        let block_id = link_target[fragment + 2..].trim();
        if block_id.is_empty()
            || !block_id.bytes().all(super::collect_done::is_block_id_byte)
        {
            continue;
        }
        let embedded = line[..span.open].ends_with('!');
        let token_start = span.open - usize::from(embedded);
        let struck_span = struck_spans.iter().find(|struck| {
            token_start >= struck.start + 2 && span.end <= struck.end - 2
        });
        let struck = struck_span.is_some();
        let (full_start, full_end) = struck_span
            .filter(|struck| {
                token_start == struck.start + 2 && span.end == struck.end - 2
            })
            .map_or((token_start, span.end), |struck| {
                (struck.start, struck.end)
            });
        links.push(BlockLinkSpan {
            target: target.to_string(),
            block_id: block_id.to_string(),
            open: span.open,
            end: span.end,
            token_start,
            embedded,
            struck,
            aliased: inside.contains('|'),
            full_start,
            full_end,
        });
    }
    links
}

/// Byte ranges of inline code spans (`` `code` ``) in a line.
pub(crate) fn inline_code_spans(line: &str) -> Vec<Range<usize>> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'`' {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] == b'`' {
            index += 1;
        }
        let width = index - start;
        let mut search = index;
        while search < bytes.len() {
            if bytes[search] != b'`' {
                search += 1;
                continue;
            }
            let close = search;
            while search < bytes.len() && bytes[search] == b'`' {
                search += 1;
            }
            if search - close == width {
                spans.push(start..search);
                index = search;
                break;
            }
        }
        if index == start + width {
            break;
        }
    }
    spans
}

/// Byte ranges of `~~`-delimited spans in a line.
pub(crate) fn strikethrough_spans(line: &str) -> Vec<Range<usize>> {
    let mut delimiters = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = line[cursor..].find("~~") {
        let position = cursor + offset;
        delimiters.push(position);
        cursor = position + 2;
    }
    delimiters
        .chunks_exact(2)
        .map(|pair| pair[0]..pair[1] + 2)
        .collect()
}

/// Canonical vault-wide id for a `(note, block-id)` task identity.
///
/// Prefers the target's existing valid `[id::]` (callers pass it through
/// untouched); otherwise this is the id writers stamp onto targets. A
/// target whose path cannot be encoded (spaces, dots) is refused only
/// when it has no `[id::]` yet.
pub(crate) fn dependency_id(
    relative_path: &Path,
    block_id: &str,
) -> io::Result<String> {
    if block_id.is_empty()
        || !block_id.bytes().all(super::collect_done::is_block_id_byte)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid task dependency block id: {block_id}"),
        ));
    }
    let note = vault_relative_link_target(relative_path)?;
    let value = format!("{}__{block_id}", note.replace('/', "__"));
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "dependency id contains unsupported path characters: {value}"
            ),
        ));
    }
    Ok(value)
}

fn vault_relative_link_target(relative_path: &Path) -> io::Result<String> {
    let mut path_without_extension = relative_path.to_path_buf();
    path_without_extension.set_extension("");

    let mut components = Vec::new();
    for component in path_without_extension.components() {
        let Component::Normal(part) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "dependency note path is not vault-relative: {}",
                    relative_path.display()
                ),
            ));
        };
        components.push(part.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "dependency note path is not valid UTF-8: {}",
                    relative_path.display()
                ),
            )
        })?);
    }

    Ok(components.join("/"))
}

/// Whether a resolved path lives under the `done/` archive.
pub(crate) fn is_archive_path(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Normal(first)) if first == OsStr::new("done")
    )
}
