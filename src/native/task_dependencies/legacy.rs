//! R8 legacy dependency children (`docs/task-dependencies.md` §4.2).
//!
//! A legacy child is a direct child bullet whose only content is one
//! block link — plain, `![[…]]`, `~~[[…]]~~`, or `~~![[…]]~~` — and
//! whose resolved target's id (its `[id::]`, canonical id, or same-note
//! bare block id) is in the dependent's field. Plain sole links count
//! too, so un-embedding a legacy child with `!` doesn't silently drop
//! the dependency. Any other sole embed (for example a `#^ref` reading
//! embed without field coverage) is content, not an edge.

use super::super::collect_done::is_block_id_byte;
use super::super::task_status_hooks::after_list_marker;

/// One block link found as the sole content of a legacy child bullet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyChildLink {
    pub(crate) target: String,
    pub(crate) block_id: String,
}

/// Extract the sole block link of a legacy child bullet, if the line has
/// exactly that shape. Field membership (the second half of R8) is
/// checked by the caller, which knows the dependent's field.
pub(crate) fn legacy_child_reference(line: &str) -> Option<LegacyChildLink> {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let marker_end = after_list_marker(line, indent)?;
    let mut body = line[marker_end..].trim();
    if body.contains('`') {
        return None;
    }
    if body.starts_with("~~") {
        body = body.strip_prefix("~~")?.strip_suffix("~~")?.trim();
    } else if body.starts_with('~') || body.ends_with('~') {
        return None;
    }
    body = body.strip_prefix('!').unwrap_or(body);
    let inner = body.strip_prefix("[[")?.strip_suffix("]]")?;
    if inner.contains("[[") || inner.contains("]]") || inner.contains('|') {
        return None;
    }
    let link_target = inner;
    let fragment = link_target.find("#^")?;
    let target = link_target[..fragment].trim();
    let block_id = link_target[fragment + 2..].trim();
    if block_id.is_empty() || !block_id.bytes().all(is_block_id_byte) {
        return None;
    }
    Some(LegacyChildLink {
        target: target.to_string(),
        block_id: block_id.to_string(),
    })
}
