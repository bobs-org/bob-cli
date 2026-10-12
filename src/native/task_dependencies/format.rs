//! Canonical Depends-On formatter and link form
//! (`docs/task-dependencies.md` §§2.1, 3).
//!
//! Writer form: `⛓️` (U+26D3 U+FE0F), a space, `**DEPENDS ON:**`, a
//! space, then links joined by ` • ` (U+2022, one space each side).
//! No aliases, embeds, or strikes. Existing links keep their order and
//! new links append; the field mirrors the line's order.

use std::ffi::OsStr;
use std::path::Path;

use super::super::vault_links::NoteIndex;

pub(crate) const DEPENDS_ON_EMOJI: &str = "⛓️";
pub(crate) const DEPENDS_ON_LABEL: &str = "DEPENDS ON";
pub(crate) const DEPENDS_ON_SEPARATOR: &str = " • ";

/// Render one canonical Depends-On line: `{indent}- ⛓️ **DEPENDS ON:** …`.
pub(crate) fn format_dependency_line(
    indent: &str,
    link_texts: &[String],
) -> String {
    format!(
        "{indent}- {DEPENDS_ON_EMOJI} **{DEPENDS_ON_LABEL}:** {}",
        link_texts.join(DEPENDS_ON_SEPARATOR)
    )
}

/// Shortest unambiguous link form for a prerequisite target
/// (`docs/task-dependencies.md` §3):
/// 1. `[[#^id]]` for a target in the same note;
/// 2. `[[basename#^id]]` when the basename is unique in the vault
///    (case-insensitive);
/// 3. otherwise `[[dir/note#^id]]`, the full vault-relative path
///    without `.md`.
pub(crate) fn canonical_link(
    target: &Path,
    block_id: &str,
    source: &Path,
    index: &NoteIndex,
) -> String {
    if target == source {
        return format!("[[#^{block_id}]]");
    }
    if let Some(basename) = target.file_stem().and_then(OsStr::to_str)
        && index.resolve(None, basename).as_deref() == Some(target)
    {
        return format!("[[{basename}#^{block_id}]]");
    }
    format!("[[{}#^{block_id}]]", path_without_extension(target))
}

fn path_without_extension(path: &Path) -> String {
    let mut without_extension = path.to_path_buf();
    without_extension.set_extension("");
    without_extension
        .components()
        .filter_map(|component| {
            component.as_os_str().to_str().map(str::to_string)
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Child indent for a new Depends-On line (`docs/task-dependencies.md`
/// §2.1): reuse the task's existing child indent, otherwise use the
/// parent's indent plus one tab.
pub(crate) fn child_indent_for_parent(
    parent_indent: &str,
    existing_child_indent: Option<&str>,
) -> String {
    existing_child_indent
        .map_or_else(|| format!("{parent_indent}\t"), str::to_string)
}

/// Line ending used by a note: CRLF when the note uses CRLF.
pub(crate) fn note_line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Whether a note's contents end with a final newline.
pub(crate) fn has_final_newline(contents: &str) -> bool {
    contents.ends_with('\n')
}
