//! Dry-run planner for `bob ref migrate-zorg`.
//!
//! Planning is a pure function of the parsed records, the built index
//! rows, the vault's Markdown stem set, and each source file's lines. It
//! returns planned notes with their exact rendered contents plus the
//! metadata the report needs. Every parsed record lands in exactly one
//! bucket — `note`, `chapter`, `skipped`, or `already_migrated` — so
//! `note + chapter + skipped` always equals doctor's unmirrored count.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::Path;

use super::super::{
    build_row, parse_zorg_records, record_is_mirrored, stored_identity,
    zorg_source_files, RefRow, ZorgRecord,
};
use super::render::{
    chapter_title, field_values, owner_tags, record_title, related_lines,
    render_note, titleize_id,
};
use crate::native::highlights_ref::validate_and_clean;

/// One planned legacy note with its exact rendered contents.
#[derive(Debug, Clone)]
pub(crate) struct PlannedNote {
    /// Vault-relative target path, e.g. `ref/zorg/work_ref/critique_nvim.md`.
    pub path: String,
    /// Exact file contents to write.
    pub contents: String,
    pub title: String,
    pub ref_type: String,
    pub source_path: String,
    pub source_block: String,
    pub source_id: String,
    pub legacy_status: String,
    pub reading_state: String,
    pub urls: Vec<String>,
    /// Folded chapters; empty for plain notes.
    pub chapter_count: usize,
    pub is_book: bool,
    /// The stem the ID wanted, when a collision renamed it.
    pub renamed: Option<RenamedStem>,
}

/// A renamed stem plus the vault file it avoided.
#[derive(Debug, Clone)]
pub(crate) struct RenamedStem {
    pub from: String,
    pub avoids: String,
}

/// A record the planner will not migrate, with its reason.
#[derive(Debug, Clone)]
pub(crate) struct SkippedRecord {
    pub path: String,
    pub line: usize,
    pub reason: &'static str,
}

/// A planned URL identity key that already names an existing note.
#[derive(Debug, Clone)]
pub(crate) struct IdentityHit {
    pub key: String,
    pub planned: String,
    pub note: String,
}

/// The full dry-run migration plan.
#[derive(Debug, Clone)]
pub(crate) struct MigrationPlan {
    /// Unmirrored records, the same number doctor reports.
    pub records: usize,
    /// Unmirrored records per file, count descending then path.
    pub by_file: Vec<(String, usize)>,
    /// Unmirrored records per canonical status.
    pub by_status: Vec<(String, usize)>,
    pub notes: Vec<PlannedNote>,
    pub chapters: usize,
    pub already_migrated: usize,
    pub skipped: Vec<SkippedRecord>,
    pub identity_hits: Vec<IdentityHit>,
}

impl MigrationPlan {
    pub(crate) fn renamed(&self) -> usize {
        self.notes
            .iter()
            .filter(|note| note.renamed.is_some())
            .count()
    }

    pub(crate) fn no_url(&self) -> usize {
        self.notes
            .iter()
            .filter(|note| note.urls.is_empty())
            .count()
    }
}

/// Plan the full migration without writing anything.
pub(crate) fn plan_migration(
    bob_dir: &Path,
    ref_dir: &Path,
    rows: &[RefRow],
) -> MigrationPlan {
    let ref_prefix = ref_dir
        .strip_prefix(bob_dir)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| "ref".to_string());
    let mut taken = vault_stems(bob_dir);
    let mut plan = MigrationPlan {
        records: 0,
        by_file: Vec::new(),
        by_status: Vec::new(),
        notes: Vec::new(),
        chapters: 0,
        already_migrated: 0,
        skipped: Vec::new(),
        identity_hits: Vec::new(),
    };
    let mut status_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut file_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (path, display) in zorg_source_files(bob_dir, ref_dir) {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        plan_file(
            &contents,
            &display,
            &ref_prefix,
            rows,
            &mut taken,
            &mut plan,
            &mut status_counts,
            &mut file_counts,
        );
    }
    let mut by_file: Vec<(String, usize)> = file_counts.into_iter().collect();
    by_file.sort_by(|left, right| {
        right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
    });
    plan.by_file = by_file;
    plan.by_status = status_counts.into_iter().collect();
    plan.identity_hits = find_identity_hits(&plan.notes, rows);
    plan
}

/// Plan one source file's records, in source order.
#[allow(clippy::too_many_arguments)]
fn plan_file(
    contents: &str,
    display: &str,
    ref_prefix: &str,
    rows: &[RefRow],
    taken: &mut BTreeMap<String, String>,
    plan: &mut MigrationPlan,
    status_counts: &mut BTreeMap<String, usize>,
    file_counts: &mut BTreeMap<String, usize>,
) {
    let lines: Vec<&str> = contents.lines().collect();
    let records = parse_zorg_records(contents, display);
    if records.is_empty() {
        return;
    }
    let mirrored: Vec<bool> = records
        .iter()
        .map(|record| record_is_mirrored(record, rows))
        .collect();
    // A book file holds exactly one BOOK record; chapters fold into its
    // note only when that BOOK record itself is still unmirrored.
    let book_index = single_book_index(&records);
    let book_block = book_index.and_then(|index| {
        (!mirrored[index])
            .then(|| records[index].owner_block.clone())
            .flatten()
    });
    let hub = hub_stem(display);
    for (index, record) in records.iter().enumerate() {
        if mirrored[index] {
            plan.already_migrated += 1;
            continue;
        }
        plan.records += 1;
        *status_counts.entry(record.status.clone()).or_insert(0) += 1;
        *file_counts.entry(display.to_string()).or_insert(0) += 1;
        let block = block_lines(&lines, record);
        if record.owner_block.is_none() {
            plan.skipped.push(skip(record, display, "no_owner"));
            continue;
        }
        if record.id.is_none() && record.lid.is_none() {
            plan.skipped.push(skip(record, display, "no_id"));
            continue;
        }
        let is_book = record.status == "BOOK";
        if !is_book
            && let Some(book) = book_block.as_deref()
            && (record.lid.is_some()
                || record.book_target.as_deref() == Some(book))
        {
            plan.chapters += 1;
            continue;
        }
        if !is_book && record.lid.is_some() {
            plan.skipped
                .push(skip(record, display, "unassigned_chapter"));
            continue;
        }
        plan.notes.push(plan_note(
            record, &block, &lines, &records, &mirrored, book_index, display,
            hub, ref_prefix, taken,
        ));
    }
}

/// Plan one note's target path, rendered contents, and report metadata.
#[allow(clippy::too_many_arguments)]
fn plan_note(
    record: &ZorgRecord,
    block: &[&str],
    lines: &[&str],
    records: &[ZorgRecord],
    mirrored: &[bool],
    book_index: Option<usize>,
    display: &str,
    hub: &str,
    ref_prefix: &str,
    taken: &mut BTreeMap<String, String>,
) -> PlannedNote {
    let source_id = record
        .id
        .clone()
        .or_else(|| record.lid.clone())
        .unwrap_or_default();
    let is_book = record.status == "BOOK"
        && book_index.is_some_and(|index| {
            records[index].owner_block == record.owner_block
                && records[index].owner_line == record.owner_line
        });
    // Folded chapters, in source order: LID records plus the `| BOOK:`
    // tie-in, each still unmirrored.
    let mut folded: Vec<&ZorgRecord> = Vec::new();
    if is_book && let Some(book) = record.owner_block.clone() {
        for (index, other) in records.iter().enumerate() {
            if mirrored[index] || other.status == "BOOK" {
                continue;
            }
            if other.lid.is_some()
                || other.book_target.as_deref() == Some(&book)
            {
                folded.push(other);
            }
        }
    }
    let title = record_title(block, &source_id);
    let tags = record
        .owner_line
        .and_then(|line| lines.get(line - 1))
        .map(|owner| {
            let mut tags = vec!["zorg/reference".to_string()];
            tags.extend(owner_tags(owner));
            tags
        })
        .unwrap_or_else(|| vec!["zorg/reference".to_string()]);
    let file_values = field_values(block, "file");
    let wikilinks: Vec<String> = file_values
        .iter()
        .filter(|value| value.starts_with("[[") && value.contains("]]"))
        .cloned()
        .collect();
    let ref_type = wikilinks
        .first()
        .and_then(|link| ref_kind(link))
        .unwrap_or_else(|| "zorg".to_string());
    let urls: Vec<String> = field_values(block, "url")
        .into_iter()
        .filter_map(|candidate| {
            validate_and_clean(&candidate)
                .ok()
                .map(|clean| clean.cleaned)
        })
        .collect();
    let related = related_lines(block);
    let chapters: Vec<(String, String, String, String)> = folded
        .iter()
        .map(|chapter| {
            let key = chapter
                .lid
                .clone()
                .or_else(|| chapter.id.clone())
                .unwrap_or_default();
            let owner = chapter
                .owner_line
                .and_then(|line| lines.get(line - 1))
                .copied()
                .unwrap_or("");
            let name = chapter_title(
                owner,
                if chapter.lid.is_some() { "LID" } else { "ID" },
            );
            let name = if name.is_empty() {
                titleize_id(&key)
            } else {
                name
            };
            (
                chapter.owner_block.clone().unwrap_or_default(),
                key,
                name,
                chapter.status.to_lowercase(),
            )
        })
        .collect();
    let mut blocks = vec![block.join("\n")];
    for chapter in &folded {
        blocks.push(block_lines(lines, chapter).join("\n"));
    }
    let (stem, avoids) = pick_stem(&source_id, taken);
    let under_ref = format!("zorg/{hub}/{stem}.md");
    let path = format!("{ref_prefix}/{under_ref}");
    taken.insert(stem.to_lowercase(), path.clone());
    let renamed = (stem != source_id).then(|| RenamedStem {
        from: source_id.clone(),
        avoids: avoids.unwrap_or_default(),
    });
    let contents = render_note(
        hub,
        &title,
        &tags,
        &ref_type,
        &record.status.to_lowercase(),
        &urls,
        record.owner_block.as_deref().unwrap_or(""),
        &source_id,
        display,
        record.owner_line.unwrap_or(record.status_line),
        record.block_end,
        &wikilinks,
        &related,
        &chapters,
        &blocks,
    );
    // Prove the rendered frontmatter parses the way the index reads it.
    let reading_state = build_row(&path, &under_ref, &contents)
        .reading_state
        .clone();
    PlannedNote {
        path,
        contents,
        title,
        ref_type,
        source_path: display.to_string(),
        source_block: record.owner_block.clone().unwrap_or_default(),
        source_id,
        legacy_status: record.status.to_lowercase(),
        reading_state,
        urls,
        chapter_count: chapters.len(),
        is_book,
        renamed,
    }
}

/// A collision-free stem: the ID, else `<ID>_ref`, `<ID>_ref_2`, …,
/// compared case-insensitively against every vault Markdown stem and
/// every note already planned in this run.
fn pick_stem(
    id: &str,
    taken: &BTreeMap<String, String>,
) -> (String, Option<String>) {
    let mut candidate = id.to_string();
    let mut avoids = None;
    let mut suffix = 0;
    loop {
        match taken.get(&candidate.to_lowercase()) {
            None => return (candidate, avoids),
            Some(path) => {
                if avoids.is_none() {
                    avoids = Some(path.clone());
                }
            }
        }
        suffix += 1;
        candidate = if suffix == 1 {
            format!("{id}_ref")
        } else {
            format!("{id}_ref_{suffix}")
        };
    }
}

/// The `lib/<kind>/` file kind of the first `file::` wikilink, if any.
fn ref_kind(link: &str) -> Option<String> {
    let inner = link.trim().strip_prefix("[[")?.split("]]").next()?;
    let target = inner.split('|').next()?.split('#').next()?.trim();
    let rest = target.strip_prefix("lib/")?;
    let (kind, tail) = rest.split_once('/')?;
    (!kind.is_empty() && !tail.is_empty()).then(|| kind.to_string())
}

/// Every vault Markdown stem (lowercased) to its vault-relative path:
/// every directory except hidden ones, so `_generated/` and `ref/` count.
fn vault_stems(bob_dir: &Path) -> BTreeMap<String, String> {
    let mut stems = BTreeMap::new();
    let mut stack = vec![bob_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut ordered: Vec<_> = entries.flatten().collect();
        ordered.sort_by_key(|entry| entry.file_name());
        for entry in ordered {
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or_default();
                if name.starts_with('.') {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if !path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                continue;
            }
            let display = path
                .strip_prefix(bob_dir)
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let stem = path
                .file_stem()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .to_lowercase();
            stems.entry(stem).or_insert(display);
        }
    }
    stems
}

/// The index of the file's single BOOK record, if it holds exactly one.
fn single_book_index(records: &[ZorgRecord]) -> Option<usize> {
    let mut found = None;
    for (index, record) in records.iter().enumerate() {
        if record.status != "BOOK" {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(index);
    }
    found
}

/// The source file's stem (`work_ref.md` → `work_ref`).
fn hub_stem(display: &str) -> &str {
    display
        .rsplit('/')
        .next()
        .unwrap_or(display)
        .strip_suffix(".md")
        .or_else(|| {
            display
                .rsplit('/')
                .next()
                .unwrap_or(display)
                .strip_suffix(".MD")
        })
        .unwrap_or(display)
}

/// One record block's lines, 1-based `block_start..=block_end`.
fn block_lines<'a>(lines: &[&'a str], record: &ZorgRecord) -> Vec<&'a str> {
    let start = record.block_start.saturating_sub(1);
    let end = record.block_end.min(lines.len());
    lines.get(start..end).unwrap_or(&[]).to_vec()
}

fn skip(
    record: &ZorgRecord,
    display: &str,
    reason: &'static str,
) -> SkippedRecord {
    SkippedRecord {
        path: display.to_string(),
        line: record.owner_line.unwrap_or(record.status_line),
        reason,
    }
}

/// URL identity hits against existing notes: reported, never deduped.
fn find_identity_hits(
    notes: &[PlannedNote],
    rows: &[RefRow],
) -> Vec<IdentityHit> {
    let mut known: BTreeMap<String, String> = BTreeMap::new();
    for row in rows {
        for key in &row.identity.keys {
            known.entry(key.clone()).or_insert_with(|| row.path.clone());
        }
        if let Some(arxiv) = row.identity.arxiv.as_deref() {
            known
                .entry(format!("arxiv:{arxiv}"))
                .or_insert_with(|| row.path.clone());
        }
        if let Some(doi) = row.identity.doi.as_deref() {
            known
                .entry(format!("doi:{doi}"))
                .or_insert_with(|| row.path.clone());
        }
    }
    let mut seen = BTreeSet::new();
    let mut hits = Vec::new();
    for note in notes {
        let stored = stored_identity(&note.urls);
        let mut keys = stored.keys;
        if let Some(arxiv) = stored.arxiv {
            keys.push(format!("arxiv:{arxiv}"));
        }
        if let Some(doi) = stored.doi {
            keys.push(format!("doi:{doi}"));
        }
        for key in keys {
            if let Some(path) = known.get(&key)
                && seen.insert((key.clone(), note.path.clone()))
            {
                hits.push(IdentityHit {
                    key: key.clone(),
                    planned: note.path.clone(),
                    note: path.clone(),
                });
            }
        }
    }
    hits.sort_by(|left, right| {
        (&left.planned, &left.key).cmp(&(&right.planned, &right.key))
    });
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_kind_reads_only_lib_targets() {
        assert_eq!(ref_kind("[[lib/docs/x.pdf]]"), Some("docs".to_string()));
        assert_eq!(ref_kind("[[lib/chat/y]]"), Some("chat".to_string()));
        assert_eq!(ref_kind("[[books/x]]"), None);
        assert_eq!(ref_kind("NONE"), None);
    }

    #[test]
    fn hub_stem_strips_extension() {
        assert_eq!(hub_stem("work_ref.md"), "work_ref");
        assert_eq!(hub_stem("notes/nvim_ref.md"), "nvim_ref");
    }
}
