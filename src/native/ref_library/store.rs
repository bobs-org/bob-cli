//! Shared saved-source reads for `bob ref code` and `bob ref links`.

use std::path::{Component, Path};

use super::{show::resolve_show, RefIndex, RefRow};
use crate::native::highlights_ref::source_store::{
    self, StoreRead, StoreStatus,
};

/// The pointer is frontmatter data. Restrict reads to the store's documented
/// layout so a malformed note cannot make `bob ref code` read outside the vault.
pub(crate) fn valid_store_pointer(pointer: &str) -> bool {
    let path = Path::new(pointer);
    let parts = path.components().collect::<Vec<_>>();
    parts.len() == 5
        && matches!(parts[0], Component::Normal(part) if part == ".bob")
        && matches!(parts[1], Component::Normal(part) if part == "ref-sources")
        && parts[2..]
            .iter()
            .all(|part| matches!(part, Component::Normal(_)))
        && matches!(parts[4], Component::Normal(part) if part == "source.md")
}

pub(crate) fn source_read(index: &RefIndex, row: &RefRow) -> StoreRead {
    match row.source_markdown.as_deref() {
        None => source_store::read_store(&index.bob_dir, ""),
        Some(pointer) if pointer.trim().is_empty() => {
            source_store::read_store(&index.bob_dir, "")
        }
        Some(pointer) if valid_store_pointer(pointer) => {
            source_store::read_store(&index.bob_dir, pointer)
        }
        Some(_) => StoreRead {
            status: StoreStatus::SourceInvalid,
            detail: Some(
                "saved Markdown pointer is outside .bob/ref-sources".into(),
            ),
            bytes: None,
            catalog: None,
        },
    }
}

pub(crate) fn page_label(page: Option<u32>, page_end: Option<u32>) -> String {
    match (page, page_end) {
        (Some(start), Some(end)) if start != end => {
            format!("pp. {start}–{end}")
        }
        (Some(page), _) => format!("p. {page}"),
        _ => "p. ?".to_string(),
    }
}

pub(crate) fn shortest_ref(index: &RefIndex, row_index: usize) -> String {
    let row = &index.rows[row_index];
    let stem = row
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&row.path)
        .strip_suffix(".md")
        .unwrap_or(&row.path);
    let mut candidates =
        vec![row.path.clone(), stem.to_string(), row.title.clone()];
    if let Some(id) = row.marker_id.as_deref() {
        candidates.push(id.to_string());
    }
    candidates.extend(row.urls.iter().cloned());
    candidates.extend(row.identity.arxiv.iter().cloned());
    candidates.extend(row.identity.doi.iter().cloned());
    candidates.extend(row.source_pdf.iter().cloned());
    candidates.sort_by_key(|candidate| candidate.chars().count());
    candidates.dedup();
    candidates
        .into_iter()
        .find(|candidate| {
            resolve_show(index, candidate)
                .is_ok_and(|resolved| resolved.row == row_index)
        })
        .unwrap_or_else(|| row.path.clone())
}

pub(crate) fn shell_quote(text: &str) -> String {
    if text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "_./:-".contains(ch))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

pub(crate) fn usage_error(message: &str) -> i32 {
    eprintln!("bob ref: error: {message}");
    2
}

pub(crate) fn print_unavailable(detail: &str, query: &str, verb: &str) {
    eprintln!("bob ref: error: {detail}");
    eprintln!("hint: bob ref {verb} {} -f json", shell_quote(query));
}
