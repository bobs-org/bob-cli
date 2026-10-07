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

/// One parsed zorg-era reading record: a `status::` list line plus the
/// owner block that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ZorgRecord {
    /// Vault-relative source path (`/`-separated), e.g. `work_ref.md`.
    pub source_path: String,
    /// 1-based line number of the `status::` line.
    pub status_line: usize,
    /// Canonical uppercase status (`READ`, `BOOK`, …).
    pub status: String,
    /// Owner block id with its `^` (e.g. `^z-250419-09`), if any.
    pub owner_block: Option<String>,
    /// 1-based line number of the owner line, if any.
    pub owner_line: Option<usize>,
    /// Indent width of the owner line, if any.
    pub owner_indent: Option<usize>,
    /// The `ID::` token on the owner line, if any.
    pub id: Option<String>,
    /// The `LID::` token on the owner line, if any.
    pub lid: Option<String>,
    /// 1-based first line of the record block (the owner line, or the
    /// status line when there is no owner).
    pub block_start: usize,
    /// 1-based last line of the record block (inclusive).
    pub block_end: usize,
    /// The `| BOOK:` target block (with its `^`) when the record block
    /// ties this record to a book record, if any.
    pub book_target: Option<String>,
}

impl ZorgCoverage {
    /// The top-5 `("<name>", <count>)` entries, for the doctor row.
    pub(crate) fn top_files(&self) -> &[(String, usize)] {
        let end = self.per_file.len().min(5);
        &self.per_file[..end]
    }
}

/// Vault Markdown files scanned for zorg records as
/// (absolute path, vault-relative `/`-separated display), sorted.
pub(crate) fn zorg_source_files(
    bob_dir: &Path,
    ref_dir: &Path,
) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    collect_zorg_files(bob_dir, bob_dir, ref_dir, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let display = path
                .strip_prefix(bob_dir)
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            (path, display)
        })
        .collect()
}

/// Count unmirrored zorg-era records under `bob_dir`, excluding `ref_dir`,
/// hidden directories, `_generated/`, and `*.assets/` directories.
pub(crate) fn count_zorg_records(
    bob_dir: &Path,
    ref_dir: &Path,
    rows: &[RefRow],
) -> ZorgCoverage {
    let mut per_file: Vec<(String, usize)> = Vec::new();
    let mut total = 0usize;
    for (path, display) in zorg_source_files(bob_dir, ref_dir) {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let count = parse_zorg_records(&contents, &display)
            .iter()
            .filter(|record| !record_is_mirrored(record, rows))
            .count();
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

/// Parse every zorg-era reading record in one file's contents. `display`
/// is the vault-relative source path (`/`-separated).
pub(crate) fn parse_zorg_records(
    contents: &str,
    display: &str,
) -> Vec<ZorgRecord> {
    let lines: Vec<&str> = contents.lines().collect();
    // Owner candidates: (indent, block id) for list lines ending in `^z-…`.
    let mut owners: Vec<Option<(usize, String)>> =
        Vec::with_capacity(lines.len());
    for line in &lines {
        owners.push(owner_candidate(line));
    }
    let mut records = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(status) = zorg_status_value(line) else {
            continue;
        };
        let indent = leading_width(line);
        let mut owner: Option<(usize, usize, String)> = None;
        for (candidate_index, candidate) in
            owners[..index].iter().enumerate().rev()
        {
            if let Some((owner_indent, block)) = candidate
                && *owner_indent < indent
            {
                owner = Some((candidate_index, *owner_indent, block.clone()));
                break;
            }
        }
        let status_line = index + 1;
        let Some((owner_index, owner_indent, owner_block)) = owner else {
            records.push(ZorgRecord {
                source_path: display.to_string(),
                status_line,
                status,
                owner_block: None,
                owner_line: None,
                owner_indent: None,
                id: None,
                lid: None,
                block_start: status_line,
                block_end: status_line,
                book_target: None,
            });
            continue;
        };
        let owner_text = lines[owner_index];
        let (block_start, block_end) =
            record_block_range(&lines, owner_index, owner_indent);
        records.push(ZorgRecord {
            source_path: display.to_string(),
            status_line,
            status,
            owner_block: Some(owner_block),
            owner_line: Some(owner_index + 1),
            owner_indent: Some(owner_indent),
            id: id_token(owner_text, "ID::"),
            lid: id_token(owner_text, "LID::"),
            block_start,
            block_end,
            book_target: book_target_in_block(
                &lines[block_start - 1..block_end],
            ),
        });
    }
    records
}

/// True when the library already mirrors `record` (the provenance rule):
/// some indexed ref note carries the same `source_path` and either the
/// same `source_block` with a matching `source_id` (or no `source_id`,
/// as tolerated for older notes), or the record's block in its
/// `source_blocks`. Records without an owner are never mirrored.
pub(crate) fn record_is_mirrored(record: &ZorgRecord, rows: &[RefRow]) -> bool {
    let Some(owner) = record.owner_block.as_deref().map(normalize_block_id)
    else {
        return false;
    };
    if owner.is_empty() {
        return false;
    }
    rows.iter().any(|row| {
        let Some(path) = row
            .source_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            return false;
        };
        if path != record.source_path {
            return false;
        }
        if row
            .source_blocks
            .iter()
            .map(|block| normalize_block_id(block))
            .any(|block| block == owner)
        {
            return true;
        }
        let Some(block) = row
            .source_block
            .as_deref()
            .map(normalize_block_id)
            .filter(|block| !block.is_empty())
        else {
            return false;
        };
        if block != owner {
            return false;
        }
        match row
            .source_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            // Older notes carry no `source_id`: block plus path is enough.
            None => true,
            Some(source_id) => {
                record.id.as_deref() == Some(source_id)
                    || record.lid.as_deref() == Some(source_id)
            }
        }
    })
}

/// A block id normalized to a leading `^`, with surrounding quotes and
/// whitespace trimmed.
pub(crate) fn normalize_block_id(raw: &str) -> String {
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

/// The record block for an owner line: from the owner line to the last
/// line before the next non-blank line whose indent is at most the
/// owner's indent. Returns 1-based (start, inclusive end); trailing
/// blank lines are excluded.
fn record_block_range(
    lines: &[&str],
    owner_index: usize,
    owner_indent: usize,
) -> (usize, usize) {
    let mut end_exclusive = lines.len();
    for (index, line) in lines.iter().enumerate().skip(owner_index + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if leading_width(line) <= owner_indent {
            end_exclusive = index;
            break;
        }
    }
    let mut last = end_exclusive;
    while last > owner_index + 1 && lines[last - 1].trim().is_empty() {
        last -= 1;
    }
    (owner_index + 1, last)
}

/// The first whitespace token after `prefix` (`ID::`, `LID::`) on an
/// owner line, if any.
fn id_token(line: &str, prefix: &str) -> Option<String> {
    line.split_whitespace().find_map(|token| {
        token.strip_prefix(prefix).and_then(|value| {
            let value = value.trim_matches([',', '.', ';', '"', '\'', '`']);
            (!value.is_empty()).then(|| value.to_string())
        })
    })
}

/// The `| BOOK:` target block (with its `^`) in a record block slice,
/// if any: the `#^z-…` target of a `BOOK:` wikilink line.
fn book_target_in_block(block: &[&str]) -> Option<String> {
    for line in block {
        if !line.contains("BOOK:") {
            continue;
        }
        let mut rest = *line;
        while let Some(position) = rest.find("#^z-") {
            rest = &rest[position + 1..];
            let length = rest[1..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .map(|offset| offset + 1)
                .unwrap_or(rest.len());
            let candidate = &rest[..length];
            if candidate.len() > "^z-".len() && candidate.starts_with("^z-") {
                return Some(candidate.to_string());
            }
            rest = &rest[1..];
        }
    }
    None
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

/// The canonical uppercase status of a zorg reading-record status
/// line, if the line is one.
fn zorg_status_value(line: &str) -> Option<String> {
    let indent = leading_width(line);
    let rest = line.get(indent..)?;
    if !(rest.starts_with('-') || rest.starts_with('*')) {
        return None;
    }
    let after = rest[1..].trim_start();
    let fields = strip_prefix_case_insensitive(after, "status::")?;
    let value = fields
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches([',', '.', ';', '"', '\'']);
    ZORG_STATUSES
        .iter()
        .find(|status| status.eq_ignore_ascii_case(value))
        .map(|status| status.to_string())
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

    fn row_with_source(
        block: Option<&str>,
        path: &str,
        source_id: Option<&str>,
        blocks: &[&str],
    ) -> RefRow {
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
            source_block: block.map(str::to_string),
            source_path: Some(path.to_string()),
            source_id: source_id.map(str::to_string),
            source_blocks: blocks
                .iter()
                .map(|block| block.to_string())
                .collect(),
        }
    }

    fn unmirrored_count(
        contents: &str,
        display: &str,
        rows: &[RefRow],
    ) -> usize {
        parse_zorg_records(contents, display)
            .iter()
            .filter(|record| !record_is_mirrored(record, rows))
            .count()
    }

    #[test]
    fn status_lines_count_and_mirrored_records_subtract() {
        let contents = "- 250419#09 [[read]] ID::awesome ^z-250419-09\n  * status:: READ\n  * url:: https://example.com\n\n- stray [[read]] ^z-250419-0a\n  * status:: unread\n";
        // The legacy note carries no `source_id`, so block plus path
        // mirrors the first record even though it has an `ID::`.
        let rows = [row_with_source(
            Some("^z-250419-09"),
            "mcp_ref.md",
            None,
            &[],
        )];
        assert_eq!(unmirrored_count(contents, "mcp_ref.md", &rows), 1);
        assert_eq!(unmirrored_count(contents, "other.md", &rows), 2);
    }

    #[test]
    fn non_zorg_status_values_do_not_count() {
        let contents =
            "- owner ^z-250101-aa\n  * status:: maybe\n  * status:: READY\n";
        assert_eq!(parse_zorg_records(contents, "notes.md").len(), 0);
    }

    #[test]
    fn multibyte_lines_never_panic_or_count() {
        let contents = "- 123456 — not a record\n— status:: READ\n- owner ^z-250101-aa\n  * status:: read\n  — status:: READ\n";
        assert_eq!(parse_zorg_records(contents, "notes.md").len(), 1);
    }

    #[test]
    fn owner_requires_less_indent() {
        // The status line sits at the owner's own indent, so it keeps no
        // owner — but an ownerless record still counts.
        let contents = "- owner ^z-250101-aa\n* status:: READ\n";
        let records = parse_zorg_records(contents, "notes.md");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].owner_block, None);
        assert_eq!(unmirrored_count(contents, "notes.md", &[]), 1);
    }

    #[test]
    fn shared_block_pair_stays_distinct_by_source_id() {
        let contents = "- 250425#0F +gbd [[read]] ID::bidder_declarations_prd ^z-250425-0f\n  | LINKS: [[prd]]\n  * status:: READ\n\n- 250425#0f +gbd [[read]] ID::cs_bidder_dec ^z-250425-0f\n  | LINKS: [[words]]\n  * status:: READ\n";
        let records = parse_zorg_records(contents, "prj_gbd.md");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].owner_block.as_deref(), Some("^z-250425-0f"));
        assert_eq!(records[1].owner_block.as_deref(), Some("^z-250425-0f"));
        assert_eq!(records[0].id.as_deref(), Some("bidder_declarations_prd"));
        assert_eq!(records[1].id.as_deref(), Some("cs_bidder_dec"));
        // A note mirroring the first record leaves the second counted.
        let rows = [row_with_source(
            Some("^z-250425-0f"),
            "prj_gbd.md",
            Some("bidder_declarations_prd"),
            &[],
        )];
        assert!(record_is_mirrored(&records[0], &rows));
        assert!(!record_is_mirrored(&records[1], &rows));
        assert_eq!(unmirrored_count(contents, "prj_gbd.md", &rows), 1);
    }

    #[test]
    fn mismatched_source_id_does_not_mirror() {
        let contents =
            "- 250419#09 [[read]] ID::awesome ^z-250419-09\n  * status:: READ\n";
        let records = parse_zorg_records(contents, "mcp_ref.md");
        assert_eq!(records.len(), 1);
        let rows = [row_with_source(
            Some("^z-250419-09"),
            "mcp_ref.md",
            Some("other"),
            &[],
        )];
        assert!(!record_is_mirrored(&records[0], &rows));
        assert_eq!(unmirrored_count(contents, "mcp_ref.md", &rows), 1);
    }

    #[test]
    fn book_note_source_blocks_subtract_chapters() {
        let contents = "- 250419 250321#0o [[read]] ID::system_for_writing ^z-250321-0o\n  * status:: BOOK\n\n- 250710 250323#0a [[read]] LID::chapter_0 Introduction ^z-250323-0a\n  | BOOK: [[system_for_writing#^z-250321-0o|system_for_writing]]\n  * status:: READ\n\n- 250603 250323#0b [[read]] ID::chapter_1 Capturing ^z-250323-0b\n  | BOOK: [[system_for_writing#^z-250321-0o|system_for_writing]]\n  * status:: READ\n";
        let records = parse_zorg_records(contents, "system_for_writing.md");
        assert_eq!(records.len(), 3);
        // The book note folds both chapters: nothing remains unmirrored.
        let rows = [row_with_source(
            Some("^z-250321-0o"),
            "system_for_writing.md",
            Some("system_for_writing"),
            &["^z-250323-0a", "^z-250323-0b"],
        )];
        assert_eq!(
            unmirrored_count(contents, "system_for_writing.md", &rows),
            0
        );
        // Without `source_blocks`, only the BOOK record itself is mirrored.
        let bare = [row_with_source(
            Some("^z-250321-0o"),
            "system_for_writing.md",
            Some("system_for_writing"),
            &[],
        )];
        assert_eq!(
            unmirrored_count(contents, "system_for_writing.md", &bare),
            2
        );
    }

    #[test]
    fn id_lid_and_book_target_extraction() {
        let contents = "- 250710 250323#0a [[read]] LID::chapter_0 Introduction | pg::9 ^z-250323-0a\n  | BOOK: [[system_for_writing#^z-250321-0o|system_for_writing]]\n  * status:: READ\n";
        let records = parse_zorg_records(contents, "system_for_writing.md");
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.id, None);
        assert_eq!(record.lid.as_deref(), Some("chapter_0"));
        assert_eq!(record.status, "READ");
        assert_eq!(record.status_line, 3);
        assert_eq!(record.owner_line, Some(1));
        assert_eq!(record.book_target.as_deref(), Some("^z-250321-0o"));

        let plain = "- 250419#09 [[read]] ID::awesome ^z-250419-09\n  * status:: read\n";
        let records = parse_zorg_records(plain, "mcp_ref.md");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "READ");
        assert_eq!(records[0].book_target, None);
    }

    #[test]
    fn record_block_range_keeps_internal_blank_lines() {
        let contents = concat!(
            "- 250419#09 [[read]] ID::awesome ^z-250419-09\n",
            "  | LINKS: [[prd]]\n",
            "  * status:: READ\n",
            "  * url:: https://example.com\n",
            "\n",
            "  Continued notes on the same record.\n",
            "\n",
            "\n",
            "- 250420#0a [[read]] ID::other ^z-250420-0a\n",
            "  * status:: UNREAD\n",
        );
        let records = parse_zorg_records(contents, "notes.md");
        assert_eq!(records.len(), 2);
        // Lines 1-6: trailing blanks before the next owner are excluded,
        // the internal blank line is kept.
        assert_eq!((records[0].block_start, records[0].block_end), (1, 6));
        assert_eq!((records[1].block_start, records[1].block_end), (9, 10));
    }
}
