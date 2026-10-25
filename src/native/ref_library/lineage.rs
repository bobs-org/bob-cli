//! Reading families from the note-local `earlier:` list.
//!
//! A reference with no rereads is a family of one. A reread (`<root>__vN`)
//! carries an `earlier:` list naming every live family member, newest
//! first, plus any replaced captures under `ref_history/`. The list is
//! written once at birth; the index only reads it, never re-derives it.
//!
//! ```yaml
//! earlier:
//!   - "[[ref/papers/attention_is_all_you_need__v2|v2 · 2026-09-01]]"
//!   - "[[ref/papers/attention_is_all_you_need|v1 · 2026-03-02]]"
//! ```
//!
//! Both block-style (Obsidian's canonical Properties form) and inline
//! (`earlier: ["[[...]]", ...]` or a bare value) parse. Each entry is
//! `[[target|label]]` or bare; the label is static display text (a version
//! and a date, plus a highlight count for frozen notes) and is never
//! parsed for identity.
use std::collections::{BTreeMap, BTreeSet};

use super::frontmatter::ParsedFrontmatter;
use super::row::{Diagnostic, RefRow};

/// What an `earlier:` entry points at: a live reference under `ref/`, or
/// a frozen replaced capture under `ref_history/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EarlierKind {
    Reading,
    Replaced,
}

/// One parsed `earlier:` entry: the normalized link target, its static
/// label (if any), and which tree it points into.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct EarlierEntry {
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub kind: EarlierKind,
}

/// Parse the note-local `earlier:` list: block or inline, `[[target|label]]`
/// or bare. Entries with an empty target are dropped.
pub(crate) fn parse_earlier(front: &ParsedFrontmatter) -> Vec<EarlierEntry> {
    front
        .get_all("earlier")
        .iter()
        .map(|raw| parse_entry(raw))
        .filter(|entry| !entry.target.is_empty())
        .collect()
}

fn parse_entry(raw: &str) -> EarlierEntry {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .unwrap_or(trimmed);
    let (head, label) = match inner.split_once('|') {
        Some((target, label)) => {
            (target.trim(), nonempty(label.trim().to_string()))
        }
        None => (inner.trim(), None),
    };
    let target = normalize_target(head);
    let kind = if is_ref_target(&target) {
        EarlierKind::Reading
    } else {
        EarlierKind::Replaced
    };
    EarlierEntry {
        target,
        label,
        kind,
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

/// Normalize an `earlier:` link target to a vault-relative path: strip any
/// `#fragment`, and accept the `.md` suffix with or without it.
fn normalize_target(head: &str) -> String {
    let head = head.split('#').next().unwrap_or("").trim();
    head.to_string()
}

/// Whether a normalized target points under `ref/`.
fn is_ref_target(target: &str) -> bool {
    target == "ref"
        || target.starts_with("ref/")
        || target.starts_with("./ref/")
        || target.starts_with("/ref/")
}

/// The reading number: 1 plus the number of `earlier:` entries whose link
/// target is under `ref/`. It is computed from frontmatter, never parsed
/// from a filename, so a stem that merely looks versioned (such as
/// `protocol__v2`) is reading 1 — and a deleted middle reading leaves the
/// surviving numbers unchanged.
pub(crate) fn reading_number(entries: &[EarlierEntry]) -> u32 {
    1 + entries
        .iter()
        .filter(|entry| entry.kind == EarlierKind::Reading)
        .count() as u32
}

/// Resolve a normalized `Reading` target to a live row index by
/// vault-relative path, with or without the `.md` suffix.
fn resolve_reading(
    target: &str,
    by_path: &BTreeMap<&str, usize>,
) -> Option<usize> {
    if let Some(index) = by_path.get(target) {
        return Some(*index);
    }
    let with_md = format!("{target}.md");
    by_path.get(with_md.as_str()).copied()
}

/// Build families as connected components over `Reading` entries that
/// resolve to live rows. Sets each row's family key (the root path), its
/// reading number, and whether it has later readings; warns on entries
/// that resolve nowhere.
pub(crate) fn apply_families(rows: &mut [RefRow], bob_dir: &std::path::Path) {
    let by_path: BTreeMap<&str, usize> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.path.as_str(), index))
        .collect();
    let mut union = UnionFind::new(rows.len());
    // Resolution per row, kept for the dangling pass below. `Reading`
    // entries resolve to live row indices; `Replaced` entries resolve to
    // the row-count sentinel when the frozen note exists on disk.
    let mut resolved: Vec<Vec<Option<usize>>> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let mut row_resolved = Vec::with_capacity(row.earlier.len());
        for entry in &row.earlier {
            let target = if entry.kind == EarlierKind::Reading {
                resolve_reading(&entry.target, &by_path)
            } else {
                resolve_history(&entry.target, bob_dir)
            };
            if entry.kind == EarlierKind::Reading
                && let Some(other) = target
            {
                union.union(index, other);
            }
            row_resolved.push(target);
        }
        resolved.push(row_resolved);
    }
    // Components keyed by root member: the member with no `Reading`
    // entries, else the smallest path (hand-written oddities only).
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for index in 0..rows.len() {
        members.entry(union.find(index)).or_default().push(index);
    }
    for component in members.values() {
        let root = component
            .iter()
            .copied()
            .filter(|index| {
                rows[*index]
                    .earlier
                    .iter()
                    .all(|entry| entry.kind != EarlierKind::Reading)
            })
            .min_by_key(|index| &rows[*index].path)
            .or_else(|| {
                component
                    .iter()
                    .copied()
                    .min_by_key(|index| &rows[*index].path)
            })
            .expect("non-empty component");
        let root_path = rows[root].path.clone();
        let family_size = component.len();
        // Newest-first reading order within the family for `later:`.
        let mut by_reading: Vec<usize> = component.clone();
        by_reading.sort_by(|left, right| {
            reading_number(&rows[*left].earlier)
                .cmp(&reading_number(&rows[*right].earlier))
                .then_with(|| rows[*left].path.cmp(&rows[*right].path))
        });
        for index in component {
            rows[*index].family = root_path.clone();
            let number = reading_number(&rows[*index].earlier);
            let carries_reading = rows[*index]
                .earlier
                .iter()
                .any(|entry| entry.kind == EarlierKind::Reading);
            rows[*index].reading =
                (family_size >= 2 || carries_reading).then_some(number);
            rows[*index].has_later = by_reading.iter().any(|other| {
                *other != *index
                    && reading_number(&rows[*other].earlier) > number
            });
        }
    }
    // Dangling entries warn on their own row, of either kind.
    for (index, targets) in resolved.iter().enumerate() {
        for (entry, hit) in rows[index].earlier.iter().zip(targets.iter()) {
            if hit.is_none() {
                let location = match entry.kind {
                    EarlierKind::Reading => "ref/",
                    EarlierKind::Replaced => "ref_history/",
                };
                rows[index].diagnostics.push(Diagnostic::new(
                    "earlier_dangling",
                    format!(
                        "earlier: entry [[{}]] does not resolve under {location}",
                        entry.target
                    ),
                ));
            }
        }
    }
}

/// A `Replaced` entry resolves when the frozen note exists on disk, with
/// or without the `.md` suffix. Returns the row-count sentinel `usize::MAX`
/// (never a real row) so callers can tell resolution from family edges.
fn resolve_history(target: &str, bob_dir: &std::path::Path) -> Option<usize> {
    let trimmed = target
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_start_matches("./");
    let direct = bob_dir.join(trimmed);
    if direct.is_file() {
        return Some(usize::MAX);
    }
    if direct.extension().is_none()
        && bob_dir.join(format!("{trimmed}.md")).is_file()
    {
        return Some(usize::MAX);
    }
    None
}

/// Path-disjoint union-find over row indices.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
        }
    }

    fn find(&mut self, index: usize) -> usize {
        if self.parent[index] != index {
            let root = self.find(self.parent[index]);
            self.parent[index] = root;
            root
        } else {
            index
        }
    }

    fn union(&mut self, left: usize, right: usize) {
        if right == usize::MAX || left == usize::MAX {
            return;
        }
        let (root_left, root_right) = (self.find(left), self.find(right));
        if root_left != root_right {
            self.parent[root_left.max(root_right)] = root_left.min(root_right);
        }
    }
}

/// Later readings of one row: family members with a higher reading
/// number, oldest first.
pub(crate) fn later_readings(index: usize, rows: &[RefRow]) -> Vec<String> {
    let row = &rows[index];
    let Some(reading) = row.reading else {
        return Vec::new();
    };
    let mut later: Vec<(u32, &str)> = rows
        .iter()
        .filter(|other| {
            other.family == row.family
                && other.path != row.path
                && other.reading.is_some_and(|other| other > reading)
        })
        .map(|other| (other.reading.unwrap_or(reading), other.path.as_str()))
        .collect();
    later.sort();
    later
        .into_iter()
        .map(|(_, path)| path.to_string())
        .collect()
}

/// Whether every index in a hit set belongs to one family.
pub(crate) fn is_single_family(hits: &[usize], rows: &[RefRow]) -> bool {
    let mut families = BTreeSet::new();
    for hit in hits {
        if rows[*hit].reading.is_none() {
            return false;
        }
        families.insert(rows[*hit].family.clone());
    }
    families.len() == 1
}

/// The human parenthetical for one `also:`/`also` companion row, relative
/// to the primary row's reading: `earlier reading v1 · read` or `later
/// reading v3 · ready` for family companions, `superseded legacy note`
/// for superseded ones, else the bare state.
pub(crate) fn companion_label(
    row: &RefRow,
    primary_reading: Option<u32>,
) -> String {
    if row.superseded_by.is_some() {
        return "superseded legacy note".to_string();
    }
    let state = row.status.as_deref().unwrap_or(row.reading_state.as_str());
    match (row.reading, primary_reading) {
        (Some(number), Some(primary)) if number < primary => {
            format!("earlier reading v{number} · {state}")
        }
        (Some(number), Some(primary)) if number > primary => {
            format!("later reading v{number} · {state}")
        }
        (Some(number), _) => format!("reading v{number} · {state}"),
        (None, _) => state.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::frontmatter::ParsedFrontmatter;
    use super::*;

    fn front(lines: &[&str]) -> ParsedFrontmatter {
        ParsedFrontmatter::parse(
            &lines
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn parses_block_entries_with_labels() {
        let entries = parse_earlier(&front(&[
            "earlier:",
            "  - \"[[ref/papers/attention__v2|v2 · 2026-09-01]]\"",
            "  - \"[[ref/papers/attention|v1 · 2026-03-02]]\"",
        ]));
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            EarlierEntry {
                target: "ref/papers/attention__v2".to_string(),
                label: Some("v2 · 2026-09-01".to_string()),
                kind: EarlierKind::Reading,
            }
        );
        assert_eq!(entries[1].kind, EarlierKind::Reading);
        assert_eq!(reading_number(&entries), 3);
    }

    #[test]
    fn parses_inline_and_bare_forms() {
        let inline = parse_earlier(&front(&[
            "earlier: [\"[[ref/papers/a|v2 · 2026-01-01]]\"]",
        ]));
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].target, "ref/papers/a");
        let bare = parse_earlier(&front(&["earlier: ref/papers/a"]));
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].label, None);
        let history = parse_earlier(&front(&[
            "earlier: \"[[ref_history/papers/a__2026-10-25|replaced 2026-10-25]]\"",
        ]));
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].kind, EarlierKind::Replaced);
        assert_eq!(reading_number(&history), 1);
    }
}
