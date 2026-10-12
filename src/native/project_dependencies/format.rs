//! `depends_on` splice writer (`docs/task-dependencies.md` §13.2,
//! §13.12).
//!
//! Edits only the key's span as a text splice: a block list of
//! double-quoted links, the key deleted when empty, inserted before
//! the closing `---` when absent. Line endings, the final-newline
//! state, and unrelated YAML stay byte-identical.

use std::path::Path;

use super::super::projects::is_frontmatter_list_item;
use super::super::task_dependencies::format::{
    canonical_link, path_without_extension,
};
use super::super::task_dependencies::is_archive_path;
use super::super::vault_links::NoteIndex;
use super::parse::DEPENDS_ON_KEY;

/// Writer link form for one project dependency (§13.2): the shortest
/// unambiguous form, except a same-note target is written as
/// `[[note#^block]]` (never `[[#^block]]`) and archive targets keep
/// the explicit `done/` path.
pub(crate) fn format_project_link(
    target: &Path,
    block_id: &str,
    project_path: &Path,
    index: &NoteIndex,
) -> String {
    if target == project_path {
        let stem = project_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("note");
        return format!("[[{stem}#^{block_id}]]");
    }
    if is_archive_path(target) {
        return format!("[[{}#^{block_id}]]", path_without_extension(target));
    }
    canonical_link(target, block_id, project_path, index)
}

/// Splice writer-form `[[...]]` link texts into the note's
/// `depends_on` property.
///
/// Callers check [`super::parse::parse_depends_on`] first and warn on
/// `Malformed`: duplicate keys are left unchanged here, and a note
/// without frontmatter is never given one.
pub(crate) fn splice_depends_on(contents: &str, links: &[String]) -> String {
    let eol = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let final_newline = contents.ends_with('\n');
    let mut lines: Vec<String> = contents
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if final_newline {
        lines.pop();
    }
    let Some(closing) = frontmatter_closing(&lines) else {
        return contents.to_string();
    };
    let keys = lines[1..closing]
        .iter()
        .enumerate()
        .filter_map(|(offset, line)| {
            depends_on_key_rest(line).map(|_| offset + 1)
        })
        .collect::<Vec<_>>();
    if keys.len() > 1 || (keys.is_empty() && links.is_empty()) {
        return contents.to_string();
    }
    let (span_start, span_end) = match keys.first() {
        // Insert before the closing `---`.
        None => (closing, closing),
        Some(&key) => {
            let rest = depends_on_key_rest(&lines[key]).unwrap_or("").trim();
            if rest.is_empty() {
                let mut end = key + 1;
                while end < closing && is_frontmatter_list_item(&lines[end]) {
                    end += 1;
                }
                (key, end)
            } else {
                (key, key + 1)
            }
        }
    };
    let mut replacement = Vec::new();
    if !links.is_empty() {
        replacement.push(format!("{DEPENDS_ON_KEY}:"));
        for link in links {
            replacement.push(format!("  - \"{link}\""));
        }
    }
    lines.splice(span_start..span_end, replacement);
    let mut out = lines.join(eol);
    if final_newline {
        out.push_str(eol);
    }
    out
}

/// Index of the closing `---` when the note opens with frontmatter.
fn frontmatter_closing(lines: &[String]) -> Option<usize> {
    if lines.first().map(String::as_str) != Some("---") {
        return None;
    }
    lines
        .iter()
        .skip(1)
        .position(|line| line == "---")
        .map(|offset| offset + 1)
}

/// Value after `depends_on:` on a raw note line.
fn depends_on_key_rest(line: &str) -> Option<&str> {
    line.strip_prefix(DEPENDS_ON_KEY)?.strip_prefix(':')
}
