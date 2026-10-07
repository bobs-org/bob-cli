//! Zorg-era reading-record coverage for `bob ref doctor` (phase `doctor`).
//!
//! The library index covers only `<ref-dir>/**/*.md`, so doctor counts the
//! zorg-era `status::` reading records that live outside the ref directory.
//! A record is one list line matching
//! `^\s*[*-] status::\s*(UNREAD|COLLECT_FLEETING_NOTES|REVIEW_FLEETING_NOTES|REVIEW_LIT_NOTES|READ|ABANDONED|BOOK)\b`
//! (case-insensitive). Its owner is the nearest preceding less-indented list
//! line that ends in a `^z-…` block id. A record is already mirrored into the
//! library when some ref note carries the same `source_block` and the same
//! `source_path`; mirrored records are subtracted from the count.
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::row::RefRow;

/// Zorg-era statuses counted as reading records.
const ZORG_STATUSES: &[&str] = &[
    "UNREAD",
    "COLLECT_FLEETING_NOTES",
    "REVIEW_FLEETING_NOTES",
    "REVIEW_LIT_NOTES",
    "READ",
    "ABANDONED",
    "BOOK",
];

/// Unmirrored zorg-era reading records outside the ref directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ZorgCoverage {
    /// Total unmirrored records across all scanned files.
    pub total: usize,
    /// Per-file unmirrored counts, ordered by count descending, then path.
    pub per_file: Vec<(String, usize)>,
}

impl ZorgCoverage {
    /// The top-5 `("<name>", <count>)` entries, for the doctor row.
    pub(crate) fn top_files(&self) -> &[(String, usize)] {
        let end = self.per_file.len().min(5);
        &self.per_file[..end]
    }
}

/// Count unmirrored zorg-era records under `bob_dir`, excluding `ref_dir`,
/// hidden directories, `_generated/`, and `*.assets/` directories.
pub(crate) fn count_zorg_records(
    bob_dir: &Path,
    ref_dir: &Path,
    rows: &[RefRow],
) -> ZorgCoverage {
    let mirrored = mirrored_sources(rows);
    let mut files = Vec::new();
    collect_zorg_files(bob_dir, bob_dir, ref_dir, &mut files);
    files.sort();
    let mut per_file: Vec<(String, usize)> = Vec::new();
    let mut total = 0usize;
    for path in &files {
        let display = path
            .strip_prefix(bob_dir)
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        let count = count_unmirrored_in_file(&contents, &display, &mirrored);
        if count > 0 {
            per_file.push((display, count));
            total += count;
        }
    }
    per_file.sort_by(|left, right| {
        right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
    });
    ZorgCoverage { total, per_file }
}

/// `(source_block, source_path)` pairs the library already mirrors, with the
/// block id normalized to a leading `^` and both sides trimmed.
fn mirrored_sources(rows: &[RefRow]) -> BTreeSet<(String, String)> {
    let mut mirrored = BTreeSet::new();
    for row in rows {
        let (Some(block), Some(path)) =
            (row.source_block.as_deref(), row.source_path.as_deref())
        else {
            continue;
        };
        let block = normalize_block_id(block);
        let path = path.trim();
        if block.is_empty() || path.is_empty() {
            continue;
        }
        mirrored.insert((block, path.to_string()));
    }
    mirrored
}

fn normalize_block_id(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'').trim();
    if trimmed.starts_with('^') {
        trimmed.to_string()
    } else {
        format!("^{trimmed}")
    }
}

/// Collect `*.md` files under `dir`, skipping the ref dir, hidden
/// directories, `_generated/`, and `*.assets/` directories.
fn collect_zorg_files(
    bob_dir: &Path,
    dir: &Path,
    ref_dir: &Path,
    files: &mut Vec<PathBuf>,
) {
    if dir != bob_dir && dir.starts_with(ref_dir) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut ordered: Vec<_> = entries.flatten().collect();
    ordered.sort_by_key(|entry| entry.file_name());
    for entry in ordered {
        let path = entry.path();
        if path.is_dir() {
            let name =
                path.file_name().and_then(OsStr::to_str).unwrap_or_default();
            if name.starts_with('.')
                || name == "_generated"
                || name.ends_with(".assets")
            {
                continue;
            }
            collect_zorg_files(bob_dir, &path, ref_dir, files);
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
        files.push(path);
    }
}

/// Count a file's zorg records minus the ones the library mirrors.
fn count_unmirrored_in_file(
    contents: &str,
    display: &str,
    mirrored: &BTreeSet<(String, String)>,
) -> usize {
    let lines: Vec<&str> = contents.lines().collect();
    // Owner candidates: (indent, block id) for list lines ending in `^z-…`.
    let mut owners: Vec<Option<(usize, String)>> =
        Vec::with_capacity(lines.len());
    for line in &lines {
        owners.push(owner_candidate(line));
    }
    let mut count = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if !is_zorg_status_line(line) {
            continue;
        }
        let indent = leading_width(line);
        let mut owner: Option<&str> = None;
        for candidate in owners[..index].iter().rev() {
            if let Some((owner_indent, block)) = candidate
                && *owner_indent < indent
            {
                owner = Some(block.as_str());
                break;
            }
        }
        if let Some(block) = owner
            && mirrored.contains(&(block.to_string(), display.to_string()))
        {
            continue;
        }
        count += 1;
    }
    count
}

/// A list line ending in a `^z-…` block id yields its indent and block id.
fn owner_candidate(line: &str) -> Option<(usize, String)> {
    let indent = leading_width(line);
    let rest = &line[indent..];
    if !is_list_item(rest) {
        return None;
    }
    let token = rest.split_whitespace().next_back()?;
    let block = token.strip_prefix('^').map(|_| token).filter(|token| {
        token.starts_with("^z-")
            && token["^z-".len()..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && token.len() > "^z-".len()
    })?;
    Some((indent, block.to_string()))
}

/// True for a `-`/`*`/`+`/ordered list item start.
fn is_list_item(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        Some('-') | Some('*') | Some('+') => {
            chars.next().is_some_and(|c| c.is_whitespace())
        }
        Some(c) if c.is_ascii_digit() => {
            let digits = rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            rest[digits.len()..].starts_with(['.', ')'])
                && rest[digits.len() + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_whitespace())
        }
        _ => false,
    }
}

/// True for a zorg reading-record status line.
fn is_zorg_status_line(line: &str) -> bool {
    let indent = leading_width(line);
    let rest = &line[indent..];
    if !(rest.starts_with('-') || rest.starts_with('*')) {
        return false;
    }
    let after = rest[1..].trim_start();
    let Some(fields) = strip_prefix_case_insensitive(after, "status::") else {
        return false;
    };
    let value = fields
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches([',', '.', ';', '"', '\'']);
    !value.is_empty()
        && ZORG_STATUSES
            .iter()
            .any(|status| status.eq_ignore_ascii_case(value))
}

/// Case-insensitive ASCII prefix strip returning the remainder.
/// `get` keeps this panic-free on multibyte input: a remainder that does
/// not start on a char boundary simply does not match.
fn strip_prefix_case_insensitive<'a>(
    haystack: &'a str,
    prefix: &str,
) -> Option<&'a str> {
    if haystack
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    {
        Some(&haystack[prefix.len()..])
    } else {
        None
    }
}

/// Leading whitespace width in characters.
fn leading_width(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_with_source(block: &str, path: &str) -> RefRow {
        RefRow {
            path: "ref/ai/mirrored.md".to_string(),
            link: "[[ref/ai/mirrored]]".to_string(),
            title: "Mirrored".to_string(),
            origin: "external".to_string(),
            ref_type: Some("ai".to_string()),
            era: "legacy".to_string(),
            status: Some("legacy".to_string()),
            status_sync: "ok".to_string(),
            frontmatter_status: None,
            legacy_status: Some("read".to_string()),
            reading_state: "finished".to_string(),
            reading_state_source: "legacy_status:read".to_string(),
            parent: None,
            urls: Vec::new(),
            identity: super::super::row::RefIdentity {
                keys: Vec::new(),
                arxiv: None,
                doi: None,
            },
            author: None,
            published: None,
            captured: None,
            added: None,
            added_source: None,
            finished: None,
            finished_source: None,
            source_pdf: None,
            audio: None,
            annotation_count: 0,
            comment_count: 0,
            snapshot: None,
            research_ref: None,
            superseded_by: None,
            diagnostics: Vec::new(),
            id: None,
            source_block: Some(block.to_string()),
            source_path: Some(path.to_string()),
        }
    }

    #[test]
    fn status_lines_count_and_mirrored_records_subtract() {
        let contents = "- 250419#09 [[read]] ID::awesome ^z-250419-09\n  * status:: READ\n  * url:: https://example.com\n\n- stray [[read]] ^z-250419-0a\n  * status:: unread\n";
        let mirrored =
            mirrored_sources(&[row_with_source("^z-250419-09", "mcp_ref.md")]);
        assert_eq!(
            count_unmirrored_in_file(contents, "mcp_ref.md", &mirrored),
            1
        );
        assert_eq!(
            count_unmirrored_in_file(contents, "other.md", &mirrored),
            2
        );
    }

    #[test]
    fn non_zorg_status_values_do_not_count() {
        let contents =
            "- owner ^z-250101-aa\n  * status:: maybe\n  * status:: READY\n";
        assert_eq!(
            count_unmirrored_in_file(contents, "notes.md", &BTreeSet::new()),
            0
        );
    }

    #[test]
    fn multibyte_lines_never_panic_or_count() {
        let contents = "- 123456 — not a record\n— status:: READ\n- owner ^z-250101-aa\n  * status:: read\n  — status:: READ\n";
        assert_eq!(
            count_unmirrored_in_file(contents, "notes.md", &BTreeSet::new()),
            1
        );
    }

    #[test]
    fn owner_requires_less_indent() {
        let contents = "- owner ^z-250101-aa\n* status:: READ\n";
        assert_eq!(
            count_unmirrored_in_file(contents, "notes.md", &BTreeSet::new()),
            1
        );
    }
}
