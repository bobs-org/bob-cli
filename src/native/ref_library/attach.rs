//! Verified attach of saved Markdown sources to filed references.
//!
//! `bob ref attach-source` backfills the write-once source store for
//! references that were filed before the store existed (notably the
//! agent-report library): it finds same-stem Markdown candidates under
//! each `--search` path, verifies each candidate against the text of the
//! filed library PDF, and records only verified sources.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use clap::ArgMatches;
use serde::Serialize;

use super::{
    build_index,
    cli::list_config_from_matches,
    output::{
        generated_at, print_find_error, print_show_error, Format,
        REF_SCHEMA_VERSION,
    },
    show::{resolve_show, ShowFailure},
    RefRow,
};
use crate::native::highlights_ref::{
    code_catalog::{self, CatalogLink, CodeBlock},
    pdf_links, reference_mutation,
    source_store::{self, StoredVerified},
    split_frontmatter, COMMAND_NAME,
};

/// References committed per scoped git batch.
const ATTACH_BATCH_SIZE: usize = 100;
/// Normalized characters below which a block cannot verify on its own.
const MIN_MATCHABLE_CHARS: usize = 12;
/// Per-unit similarity floor for an in-order match to count.
const UNIT_SIMILARITY: f64 = 0.85;
/// Share of matchable units that must verify for a candidate to attach.
const COVERAGE: f64 = 0.8;
/// Paragraphs compared when a candidate carries no matchable code.
const PARAGRAPH_FALLBACK_COUNT: usize = 5;

/// One verified candidate: its bytes plus the catalog with text-match pages.
#[derive(Clone)]
struct VerifiedCandidate {
    bytes: Vec<u8>,
    blocks: Vec<CodeBlock>,
    links: Vec<CatalogLink>,
    matched: usize,
    total: usize,
}

/// Resolve the pandoc binary exactly like `ref create`: an explicit
/// `BOB_PANDOC_COMMAND` wins, otherwise the first `pandoc` on `PATH`.
fn pandoc_command() -> Option<OsString> {
    const ENV_PANDOC_COMMAND: &str = "BOB_PANDOC_COMMAND";
    if let Some(command) = std::env::var_os(ENV_PANDOC_COMMAND)
        && !command.is_empty()
    {
        return Some(command);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("pandoc"))
        .find(|candidate| candidate.is_file())
        .map(PathBuf::into_os_string)
}

/// The candidate's title: frontmatter `title`, else the first ATX H1
/// outside fenced blocks. `None` when the candidate names no title.
fn candidate_title(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let (lines, _) =
        split_frontmatter(&text).unwrap_or((Vec::new(), text.to_string()));
    let front = super::frontmatter::ParsedFrontmatter::parse(&lines);
    if let Some(title) = front.get_str("title") {
        let title = title.trim();
        if !title.is_empty() {
            return Some(title.to_string());
        }
    }
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(title) = trimmed.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

/// Whitespace-collapsed, case-insensitive title comparison.
fn titles_match(left: &str, right: &str) -> bool {
    fn normalize(title: &str) -> String {
        title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }
    normalize(left) == normalize(right)
}

/// Fold compatibility punctuation to ASCII so a source quote and its
/// typeset PDF glyph compare equal on both sides.
fn fold_compatibility(text: &str) -> String {
    text.chars()
        .flat_map(|ch| match ch {
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => {
                vec!['\'']
            }
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => {
                vec!['"']
            }
            '\u{2013}' | '\u{2014}' => vec!['-'],
            '\u{2026}' => vec!['.', '.', '.'],
            _ => vec![ch],
        })
        .collect()
}

/// Normalize rendered text for matching: strip return-link tag glyphs
/// and pills, fold compatibility punctuation, drop fvextra wrap glyphs,
/// then drop every whitespace run.
fn normalize_text(text: &str) -> String {
    let stripped =
        crate::native::highlights_ref::text::strip_return_link_glyphs(text);
    fold_compatibility(&stripped)
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '↪')
        .collect()
}

/// Strip Markdown prose formatting from one candidate line so it compares
/// like the typeset PDF text: headings, quotes, list markers, table
/// rules, links, images, tags, and bold/code runs keep only their words.
/// Code-block bodies never pass through here.
fn strip_prose_formatting(line: &str) -> String {
    let mut text = line.to_string();
    // Autolinks first: `<https://…>` is a URL, not a tag.
    text = autolink_to_url(&text);
    // HTML tags carry no readable words.
    text = strip_html_tags(&text);
    // Images keep their alt text, links their label.
    text = link_to_label(&text);
    // Bold, strikethrough, and code-span runs keep their content.
    for marker in ["**", "__", "~~", "`"] {
        text = text.replace(marker, "");
    }
    // Single-word emphasis (`*Positions:*`, `_stem_`) keeps its word.
    text = strip_single_word_emphasis(&text, '*');
    text = strip_single_word_emphasis(&text, '_');
    // Block prefixes: headings, quotes, list markers, table pipes.
    let mut stripped = text.trim_start();
    for prefix in ["###### ", "##### ", "#### ", "### ", "## ", "# ", "> "] {
        if let Some(rest) = stripped.strip_prefix(prefix) {
            stripped = rest.trim_start();
            break;
        }
    }
    if let Some(rest) = stripped.strip_prefix(['-', '*', '+']) {
        if rest.starts_with([' ', '\t']) {
            stripped = rest.trim_start();
        }
    }
    if let Some((number, rest)) = stripped.split_once(['.', ')']) {
        if !number.is_empty()
            && number.chars().all(|ch| ch.is_ascii_digit())
            && rest.starts_with([' ', '\t'])
        {
            stripped = rest.trim_start();
        }
    }
    stripped
        .replace('|', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a character edge is whitespace, punctuation, or the line
/// end: the only legal neighbour of an emphasis marker. Identifiers like
/// `wait_for_it` and globs like `*.md` never qualify.
fn is_word_edge(side: Option<char>) -> bool {
    side.is_none_or(|ch| ch.is_whitespace() || ch.is_ascii_punctuation())
}

/// Remove `*word*`/`_word_` emphasis pairs (no spaces inside, the open
/// marker after an edge and the close marker before one), keeping the
/// word. A lone marker, a mid-identifier marker (`wait_for_it`), and
/// globs (`*.md`) stay literal.
fn strip_single_word_emphasis(text: &str, marker: char) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(marker) {
        let opens_cleanly = is_word_edge(rest[..open].chars().next_back());
        let after_open = &rest[open + marker.len_utf8()..];
        let Some(close) = after_open.find(marker) else {
            break;
        };
        let word = &after_open[..close];
        let closes_cleanly = is_word_edge(
            after_open[close + marker.len_utf8()..].chars().next(),
        );
        if !opens_cleanly
            || !closes_cleanly
            || word.is_empty()
            || word.chars().any(|ch| ch.is_whitespace() || ch == marker)
        {
            out.push_str(&rest[..open + marker.len_utf8()]);
            rest = after_open;
            continue;
        }
        out.push_str(&rest[..open]);
        out.push_str(word);
        rest = &after_open[close + marker.len_utf8()..];
    }
    out.push_str(rest);
    out
}

fn autolink_to_url(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let inner = &rest[start + 1..start + end];
        if inner.starts_with("http://")
            || inner.starts_with("https://")
            || inner.starts_with("mailto:")
        {
            out.push_str(&rest[..start]);
            out.push_str(inner);
            rest = &rest[start + end + 1..];
        } else {
            out.push_str(&rest[..start + end + 1]);
            rest = &rest[start + end + 1..];
        }
    }
    out.push_str(rest);
    out
}

fn strip_html_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let inner = &rest[start + 1..start + end];
        // A tag carries attributes (`=`, `/`) or no spaces at all;
        // `a < b` comparisons keep their literal brackets.
        let is_tag = !inner.contains(' ')
            || inner.contains('=')
            || inner.starts_with('/');
        if !is_tag {
            out.push_str(&rest[..start + end + 1]);
            rest = &rest[start + end + 1..];
            continue;
        }
        out.push_str(&rest[..start]);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// `[label](url)`, `![alt](url)`, and `[label][ref]` keep their label;
/// footnotes `[^n]` keep nothing.
fn link_to_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let Some(close) = rest[start..].find(']') else {
            break;
        };
        let label = rest[start + 1..start + close].to_string();
        let after = &rest[start + close + 1..];
        if label.starts_with('^') {
            out.push_str(&rest[..start]);
            rest = after;
        } else if after.starts_with('(') {
            let Some(paren) = after.find(')') else {
                out.push_str(&rest[..start + close + 1]);
                rest = after;
                continue;
            };
            out.push_str(&rest[..start]);
            out.push_str(&label);
            rest = &after[paren + 1..];
        } else if after.starts_with('[') {
            let Some(bracket) = after.find(']') else {
                out.push_str(&rest[..start + close + 1]);
                rest = after;
                continue;
            };
            out.push_str(&rest[..start]);
            out.push_str(&label);
            rest = &after[bracket + 1..];
        } else {
            out.push_str(&rest[..start + close + 1]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Non-empty normalized lines of one matchable prose unit: Markdown
/// formatting is stripped before normalization so typeset words compare.
fn prose_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| normalize_text(&strip_prose_formatting(line)))
        .filter(|line| !line.is_empty())
        .collect()
}

/// Non-empty normalized lines of one matchable unit.
fn unit_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(normalize_text)
        .filter(|line| !line.is_empty())
        .collect()
}

/// Greedily match normalized `lines` in order against `haystack` from
/// `cursor`, returning matched/total characters plus the match span.
fn match_lines(
    lines: &[String],
    haystack: &str,
    cursor: &mut usize,
) -> (usize, usize, Option<(usize, usize)>) {
    let total: usize = lines.iter().map(String::len).sum();
    let mut matched = 0usize;
    let mut first: Option<usize> = None;
    let mut last: Option<usize> = None;
    for line in lines {
        let from = (*cursor).min(haystack.len());
        let Some(offset) = haystack[from..].find(line.as_str()) else {
            continue;
        };
        let start = from + offset;
        matched += line.len();
        first.get_or_insert(start);
        last = Some(start + line.len());
        *cursor = start + line.len();
    }
    (matched, total, first.zip(last).map(|(s, e)| (s, e)))
}

/// A block verifies on its own when it carries enough normalized text
/// and is not Mermaid (diagrams never survive the PDF text layer
/// byte-identically).
fn is_matchable(block: &CodeBlock) -> bool {
    block.kind != "mermaid"
        && normalize_text(&block.text).chars().count() >= MIN_MATCHABLE_CHARS
}

/// Run the render's record and report passes without XeLaTeX. Preserve
/// the source route's reader and section numbering; pages are assigned later.
pub(crate) fn record_source_catalog(
    bytes: &[u8],
    route: &str,
) -> Result<code_catalog::CodeCatalog, String> {
    let snapshot = route == "note_snapshot";
    let reader = match route {
        "markdown" | "plain_note" | "attached" => {
            code_catalog::MARKDOWN_PANDOC_READER
        }
        "note_snapshot" => {
            crate::native::highlights_ref::note_snapshot::profile::SNAPSHOT_PANDOC_READER
        }
        _ => return Err(format!("unsupported saved-source route: {route}")),
    };
    let pandoc = pandoc_command().ok_or_else(|| {
        "pandoc command not found; install pandoc or set BOB_PANDOC_COMMAND"
            .to_string()
    })?;
    let scratch = std::env::temp_dir().join(format!(
        "bob-attach-source-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    fs::create_dir_all(&scratch).map_err(|error| {
        format!("create attach scratch {}: {error}", scratch.display())
    })?;
    let result = (|| {
        let candidate = scratch.join("source.md");
        fs::write(&candidate, bytes)
            .map_err(|error| format!("write record source: {error}"))?;
        let record = scratch.join("code-catalog.lua");
        fs::write(
            &record,
            format!(
                "{}\n{}",
                code_catalog::TAB_EXPANSION_PRELUDE,
                code_catalog::FILTER
            ),
        )
        .map_err(|error| {
            format!("write attach record filter {}: {error}", record.display())
        })?;
        let report_filter = scratch.join("code-catalog-report.lua");
        fs::write(&report_filter, code_catalog::REPORT_FILTER).map_err(
            |error| {
                format!(
                    "write attach report filter {}: {error}",
                    report_filter.display()
                )
            },
        )?;
        let report_path = scratch.join("code-catalog.json");
        let rendered = scratch.join("native.txt");
        let section_filter = scratch.join("code-break.lua");
        fs::write(
            &section_filter,
            crate::native::highlights_ref::create::PANDOC_CODE_BREAK_FILTER,
        )
        .map_err(|error| format!("write section filter: {error}"))?;
        let mut command = Command::new(&pandoc);
        if !snapshot {
            command.arg("--number-sections");
        }
        let status = command
            .arg(&candidate)
            .arg("-f")
            .arg(reader)
            .arg("--preserve-tabs")
            .arg("--lua-filter")
            .arg(&record)
            .arg("--lua-filter")
            .arg(&section_filter)
            .arg("--lua-filter")
            .arg(&report_filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                code_catalog::REPORT_METADATA_KEY,
                report_path.display()
            ))
            .arg("-t")
            .arg("native")
            .arg("-o")
            .arg(&rendered)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| {
                format!("run pandoc for {}: {error}", candidate.display())
            })?;
        if !status.status.success() {
            let detail =
                String::from_utf8_lossy(&status.stderr).trim().to_string();
            return Err(format!(
                "pandoc failed on {}: {}",
                candidate.display(),
                if detail.is_empty() {
                    "pandoc produced no diagnostic output".to_string()
                } else {
                    detail
                }
            ));
        }
        code_catalog::read_report_catalog(&report_path)
    })();
    let _ = fs::remove_dir_all(&scratch);
    result
}

/// Per-page PDF text via `pdftotext -layout`, one entry per physical page.
fn pdf_page_texts(
    pdf: &Path,
    checked: &mut bool,
) -> Result<Vec<String>, String> {
    if !*checked {
        let missing = match Command::new("pdftotext")
            .arg("-v")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Ok(status) => !status.success(),
            Err(_) => true,
        };
        if missing {
            return Err(
                "pdftotext not found; install poppler-utils to verify sources"
                    .to_string(),
            );
        }
        *checked = true;
    }
    let output = Command::new("pdftotext")
        .arg("-layout")
        .arg(pdf)
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            format!("run pdftotext on {}: {error}", pdf.display())
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "pdftotext failed on {}: {}",
            pdf.display(),
            if detail.is_empty() {
                "pdftotext produced no diagnostic output".to_string()
            } else {
                detail
            }
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(text.split('\u{c}').map(str::to_string).collect())
}

/// Page number (1-based) for a haystack offset, from page boundaries.
fn page_for_offset(boundaries: &[usize], offset: usize) -> u32 {
    let mut page = 1u32;
    for (index, start) in boundaries.iter().enumerate() {
        if *start <= offset {
            page = (index as u32) + 1;
        } else {
            break;
        }
    }
    page
}

/// Render chrome that never reaches the PDF as words: listen-card divs,
/// standalone images, and audio embeds. Comparing these would fail a true
/// source, so the paragraph fallback skips them when selecting content.
fn is_render_chrome(paragraph: &str) -> bool {
    paragraph.contains("class=\"listen\"")
        || paragraph.lines().all(|line| {
            let line = line.trim_start();
            line.starts_with("![")
                || line.starts_with("<img")
                || line.starts_with("<audio")
                || line.starts_with("<div")
                || line.starts_with("</div")
        })
}

/// Longest paragraphs of the candidate, for sources without matchable
/// code: the `count` longest by normalized length, returned in document
/// order so the in-order match walks the PDF forward. Formatting is
/// stripped at match time, not here.
fn longest_paragraphs(bytes: &[u8], count: usize) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let (_, body) =
        split_frontmatter(&text).unwrap_or((Vec::new(), text.to_string()));
    let mut indexed: Vec<(usize, String)> = body
        .split("\n\n")
        .enumerate()
        .map(|(index, paragraph)| (index, paragraph.trim().to_string()))
        .filter(|(_, paragraph)| {
            !paragraph.is_empty() && !is_render_chrome(paragraph)
        })
        .collect();
    indexed.sort_by_cached_key(|(_, paragraph)| {
        std::cmp::Reverse(
            prose_lines(paragraph)
                .iter()
                .map(String::len)
                .sum::<usize>(),
        )
    });
    indexed.truncate(count);
    indexed.sort_by_key(|(index, _)| *index);
    indexed
        .into_iter()
        .map(|(_, paragraph)| paragraph)
        .collect()
}

/// Verify one candidate against the library PDF text: at least 80% of
/// matchable blocks (or, with no matchable blocks, of the five longest
/// paragraphs) must match in order at 85% or better similarity. Pages
/// come from the match position.
fn verify_candidate(
    _candidate: &Path,
    bytes: &[u8],
    catalog: code_catalog::CodeCatalog,
    pages: &[String],
) -> Result<VerifiedCandidate, String> {
    let mut haystack = String::new();
    let mut boundaries = Vec::with_capacity(pages.len());
    for page in pages {
        boundaries.push(haystack.len());
        haystack.push_str(&normalize_text(page));
    }
    let code_catalog::CodeCatalog {
        version: _,
        mut blocks,
        links,
    } = catalog;
    let links = links.unwrap_or_default();
    let matchable: Vec<usize> = blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| is_matchable(block))
        .map(|(index, _)| index)
        .collect();
    if matchable.is_empty() {
        let paragraphs = longest_paragraphs(bytes, PARAGRAPH_FALLBACK_COUNT);
        let mut matched_units = 0usize;
        let mut cursor = 0usize;
        for paragraph in &paragraphs {
            let lines = prose_lines(paragraph);
            if lines.is_empty() {
                continue;
            }
            let (matched, total, _) =
                match_lines(&lines, &haystack, &mut cursor);
            if total > 0 && (matched as f64) / (total as f64) >= UNIT_SIMILARITY
            {
                matched_units += 1;
            }
        }
        let total = paragraphs.len();
        if total == 0 || (matched_units as f64) / (total as f64) < COVERAGE {
            return Err(format!(
                "low match {matched_units}/{total} paragraphs"
            ));
        }
        return Ok(VerifiedCandidate {
            bytes: bytes.to_vec(),
            blocks,
            links,
            matched: matched_units,
            total,
        });
    }
    let mut matched_units = 0usize;
    let mut cursor = 0usize;
    for index in &matchable {
        let lines = unit_lines(&blocks[*index].text);
        let (matched, total, span) =
            match_lines(&lines, &haystack, &mut cursor);
        if total > 0 && (matched as f64) / (total as f64) >= UNIT_SIMILARITY {
            matched_units += 1;
            if let Some((start, end)) = span {
                let page = page_for_offset(&boundaries, start);
                let page_end =
                    page_for_offset(&boundaries, end.saturating_sub(1));
                let stored = &mut blocks[*index];
                stored.page = Some(page);
                stored.page_end = Some(page_end);
                stored.page_source = Some("text_match".to_string());
            }
        }
    }
    let total = matchable.len();
    if (matched_units as f64) / (total as f64) < COVERAGE {
        return Err(format!("low match {matched_units}/{total} blocks"));
    }
    Ok(VerifiedCandidate {
        bytes: bytes.to_vec(),
        blocks,
        links,
        matched: matched_units,
        total,
    })
}

/// Collect `<stem>.md` files under one search path, skipping
/// dot-directories. A file path counts when its name matches.
fn collect_under(path: &Path, stem: &str, found: &mut Vec<PathBuf>) {
    let wanted = format!("{stem}.md");
    if path.is_file() {
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy() == wanted)
        {
            found.push(path.to_path_buf());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let entry_path = entry.path();
        if entry_path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if entry_path.is_dir() {
            collect_under(&entry_path, stem, found);
        } else if entry_path
            .file_name()
            .is_some_and(|name| name.to_string_lossy() == wanted)
        {
            found.push(entry_path);
        }
    }
}

/// Every verification candidate for one note stem: each `<stem>.md`
/// under the search paths, or the single named file for one named REF.
fn find_all_candidates(
    stem: &str,
    search: &[PathBuf],
    single_file_override: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(file) = single_file_override {
        return vec![file.to_path_buf()];
    }
    let mut found = Vec::new();
    for path in search {
        collect_under(path, stem, &mut found);
    }
    found.sort();
    found.dedup();
    found
}

/// One candidate's verification verdict: the verified payload when the
/// candidate passes, else the skip reason the human and JSON reports name.
#[derive(Clone)]
struct CandidateAttempt {
    path: PathBuf,
    verified: Option<VerifiedCandidate>,
    /// `verified` for a pass, else the skip reason (`title mismatch`,
    /// `low match 3/8 blocks`, `unreadable candidate: …`, …).
    verdict: String,
}

/// Verify one candidate file against the library PDF text: the title must
/// match (unless a human reviewed the explicit `REF -s FILE` pairing),
/// then the existing block-match verification runs unchanged.
fn attempt_candidate(
    candidate: &Path,
    row_title: &str,
    explicit_pair: bool,
    pdf_path: &Path,
    pages: &[String],
) -> CandidateAttempt {
    let failure = |verdict: String| CandidateAttempt {
        path: candidate.to_path_buf(),
        verified: None,
        verdict,
    };
    let bytes = match fs::read(candidate) {
        Ok(bytes) => bytes,
        Err(error) => {
            return failure(format!("unreadable candidate: {error}"));
        }
    };
    if !explicit_pair
        && candidate_title(&bytes)
            .is_none_or(|title| !titles_match(&title, row_title))
    {
        return failure("title mismatch".to_string());
    }
    let mut catalog = match record_source_catalog(&bytes, "attached") {
        Ok(catalog) => catalog,
        Err(error) => return failure(error),
    };
    if let Some(links) = catalog.links.as_mut()
        && let Err(error) = pdf_links::annotate_catalog_links(links, pdf_path)
    {
        return failure(format!("unreadable PDF annotations: {error}"));
    }
    match verify_candidate(candidate, &bytes, catalog, pages) {
        Ok(verified) => CandidateAttempt {
            path: candidate.to_path_buf(),
            verified: Some(verified),
            verdict: "verified".to_string(),
        },
        Err(reason) => failure(reason),
    }
}

/// Orphan stores whose path matches the reference type and stem. The
/// inspection result is the same live-pointer set used by `ref doctor`, so
/// stores named by either a current or frozen reference note are excluded.
fn matching_orphan_stores(
    row: &RefRow,
    orphan_stores: &BTreeSet<String>,
) -> Vec<String> {
    let (Some(stem), Some(ref_type)) = (note_stem(row), store_type(row)) else {
        return Vec::new();
    };
    let parent = Path::new(source_store::STORE_ROOT).join(ref_type);
    let prefix = format!("{stem}--");
    orphan_stores
        .iter()
        .filter(|store| {
            let path = Path::new(store);
            path.parent() == Some(parent.as_path())
                && path.file_name().is_some_and(|name| {
                    name.to_string_lossy().starts_with(&prefix)
                })
        })
        .cloned()
        .collect()
}

/// Orphan stores that have a verified saved source and a catalog for this
/// exact filed PDF. `read_store` checks both the directory hash and the
/// catalog's source digest before returning either payload.
fn qualifying_orphan_stores(
    bob_dir: &Path,
    row: &RefRow,
    matching: &[String],
) -> Vec<String> {
    let Some(pdf) = row.source_pdf.as_deref().map(str::trim) else {
        return Vec::new();
    };
    matching
        .iter()
        .filter(|store| {
            let pointer = format!("{store}/source.md");
            let read = source_store::read_store(bob_dir, &pointer);
            read.bytes.is_some()
                && read
                    .catalog
                    .as_ref()
                    .is_some_and(|catalog| catalog.pdf.path == pdf)
        })
        .cloned()
        .collect()
}

/// Adopt an already-written render-time store. The store files are included
/// in the caller's commit set as harmless no-op paths; this function only
/// changes the reference note.
fn adopt_verified(
    bob_dir: &Path,
    row: &RefRow,
    store: &str,
) -> Result<Vec<PathBuf>, String> {
    let pointer = format!("{store}/source.md");
    let note_path = bob_dir.join(&row.path);
    set_note_pointer(&note_path, &pointer)?;
    let store_dir = bob_dir.join(store);
    Ok(vec![
        note_path,
        store_dir.join("source.md"),
        store_dir.join("code.json"),
    ])
}

/// One row of the `attach-source` JSON envelope.
#[derive(Debug, Clone, Serialize)]
struct AttachCandidateJson {
    path: String,
    verdict: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<usize>,
}

/// One reference row of the `attach-source` JSON envelope.
#[derive(Debug, Clone, Serialize)]
struct AttachResultJson {
    path: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    candidates: Vec<AttachCandidateJson>,
}

/// The `ref attach-source` success envelope.
#[derive(Debug, Clone, Serialize)]
struct AttachEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    dry_run: bool,
    attached: usize,
    skipped: usize,
    results: Vec<AttachResultJson>,
}

/// Stem of a reference note path (`ref/chat/report.md` → `report`).
fn note_stem(row: &RefRow) -> Option<String> {
    Path::new(&row.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

/// Why a named or filed reference is out of scope for attach, or `None`
/// when it is eligible: PDF-backed, with no saved source and no live
/// `source_note` (snapshots are never attached from a live note, because
/// the note has changed since).
fn ineligibility(bob_dir: &Path, row: &RefRow) -> Option<String> {
    if row
        .source_pdf
        .as_deref()
        .is_none_or(|pdf| pdf.trim().is_empty())
    {
        return Some("snapshot".to_string());
    }
    if row
        .source_markdown
        .as_deref()
        .is_some_and(|pointer| !pointer.trim().is_empty())
    {
        return Some("already attached".to_string());
    }
    let contents = fs::read_to_string(bob_dir.join(&row.path)).ok()?;
    let (lines, _) =
        split_frontmatter(&contents).unwrap_or((Vec::new(), contents));
    let front = super::frontmatter::ParsedFrontmatter::parse(&lines);
    front
        .get_str("source_note")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|_| "snapshot".to_string())
}

/// Store type directory for one eligible row: the note's `ref_type`,
/// else the `lib/<type>/` parent of its filed PDF.
fn store_type(row: &RefRow) -> Option<String> {
    if let Some(ref_type) = row.ref_type.as_deref().map(str::trim)
        && !ref_type.is_empty()
    {
        return Some(ref_type.to_string());
    }
    let pdf = row.source_pdf.as_deref()?.trim();
    let under_lib = pdf.strip_prefix("lib/")?;
    let first = under_lib.split('/').next()?;
    (!first.is_empty() && under_lib.contains('/')).then(|| first.to_string())
}

/// Set the note's `source_markdown` frontmatter pointer, appending the
/// line before the closing fence (or replacing the existing line, so a
/// re-run stays idempotent).
fn set_note_pointer(note_path: &Path, pointer: &str) -> Result<(), String> {
    let contents = fs::read_to_string(note_path)
        .map_err(|error| format!("read {}: {error}", note_path.display()))?;
    let line = format!("source_markdown: {pointer}");
    let updated = match split_frontmatter(&contents) {
        Some((lines, body)) => {
            let mut replaced = false;
            let mut kept: Vec<String> = Vec::with_capacity(lines.len() + 1);
            for raw in lines {
                let key = raw.split(':').next().unwrap_or("").trim();
                if key == "source_markdown" {
                    if !replaced {
                        kept.push(line.clone());
                        replaced = true;
                    }
                } else {
                    kept.push(raw);
                }
            }
            if !replaced {
                kept.push(line);
            }
            format!("---\n{}\n---\n{body}", kept.join("\n"))
        }
        None => format!("---\n{line}\n---\n{contents}"),
    };
    fs::write(note_path, updated)
        .map_err(|error| format!("write {}: {error}", note_path.display()))
}

/// Write one verified store and point the note at it. Returns the
/// absolute store paths for the batch commit.
fn attach_verified(
    bob_dir: &Path,
    row: &RefRow,
    stem: &str,
    ref_type: &str,
    candidate: &Path,
    verified: &VerifiedCandidate,
    pdf_sha256: &str,
) -> Result<Vec<PathBuf>, String> {
    let pdf_rel = row
        .source_pdf
        .as_deref()
        .map(str::trim)
        .filter(|pdf| !pdf.is_empty())
        .ok_or_else(|| "reference has no filed PDF".to_string())?;
    let catalog =
        code_catalog::CatalogOutcome::Catalog(code_catalog::CodeCatalog {
            version: 1,
            blocks: verified.blocks.clone(),
            links: Some(verified.links.clone()),
        });
    // The caller holds the reference-mutation session (bob_sync.lock then
    // scan.lock) across every store write, so no concurrent sync can
    // commit half a store.
    let outcome = source_store::write_store(
        bob_dir,
        source_store::StoreWriteParams {
            ref_type,
            stem,
            source_bytes: &verified.bytes,
            route: "attached",
            origin: candidate.display().to_string(),
            source_note: None,
            source_note_sha256: None,
            pdf_rel: pdf_rel.to_string(),
            pdf_sha256: pdf_sha256.to_string(),
            catalog: &catalog,
            verified: Some(StoredVerified {
                matched: verified.matched,
                total: verified.total,
            }),
        },
    )
    .map_err(|error| format!("write source store: {error}"))?;
    if let Some(warning) = &outcome.warning
        && !outcome.reused
        && !outcome.code_written
    {
        return Err(warning.clone());
    }
    if let Some(warning) = &outcome.warning {
        eprintln!("{warning}");
    }
    let note_path = bob_dir.join(&row.path);
    set_note_pointer(&note_path, &outcome.pointer)?;
    let dir_abs = bob_dir.join(&outcome.dir_rel);
    Ok(vec![
        note_path,
        dir_abs.join("source.md"),
        dir_abs.join("code.json"),
    ])
}

/// Commit one batch of attached references as a single revertable commit.
fn commit_batch(
    bob_dir: &Path,
    paths: &BTreeSet<PathBuf>,
    count: usize,
    commits: &mut Vec<String>,
) -> Result<(), String> {
    let (included, ignored) =
        reference_mutation::partition_ignored(bob_dir, paths);
    if !ignored.is_empty() {
        eprintln!(
            "warning: {} attach path(s) ignored by the vault .gitignore; code copy won't reach other hosts — add \"{}\" (docs/vault-git-sync.md#reference-sources)",
            ignored.len(),
            source_store::CODE_JSON_ALLOWLIST,
        );
    }
    let subject = format!("bob ref attach-source: {count} references");
    match reference_mutation::commit_paths(bob_dir, &subject, &included)? {
        Some(sha) => {
            println!("committed {sha} ({count} references)");
            commits.push(sha);
        }
        None => eprintln!("warning: nothing to commit for {count} attach(s)"),
    }
    Ok(())
}

/// Run `bob ref attach-source`: verify same-stem candidates against filed
/// library PDFs and record only verified sources.
pub(crate) fn run_attach_source(matches: &ArgMatches) -> i32 {
    let queries: Vec<String> = matches
        .get_many::<String>("ref")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    let search: Vec<PathBuf> = matches
        .get_many::<String>("search")
        .map(|values| values.map(PathBuf::from).collect())
        .unwrap_or_default();
    let dry_run = matches.get_flag("dry-run");
    let format = Format::from_name(
        matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human"),
    );
    let json_mode = format == Format::Json;
    for path in &search {
        if !path.exists() {
            eprintln!(
                "{COMMAND_NAME}: error: search path not found: {}",
                path.display()
            );
            return 2;
        }
    }
    // One named file plus one named REF attaches that file regardless
    // of its stem.
    let single_file_override: Option<&Path> =
        (queries.len() == 1 && search.len() == 1 && search[0].is_file())
            .then(|| search[0].as_path());
    let config = list_config_from_matches(matches);
    let index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            print_find_error(
                format,
                "ref attach-source",
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
            return 1;
        }
    };
    // Resolve every named REF before touching the vault.
    let mut selected = Vec::new();
    if queries.is_empty() {
        selected.extend(0..index.rows.len());
    } else {
        for query in &queries {
            match resolve_show(&index, query) {
                Ok(resolved) => selected.push(resolved.row),
                Err(ShowFailure::Ambiguous { query, candidates }) => {
                    print_show_error(
                        format,
                        "ref attach-source",
                        "ambiguous_reference",
                        &format!("ambiguous reference: {query}"),
                        &candidates,
                    );
                    return 1;
                }
                Err(ShowFailure::NotFound { query, candidates }) => {
                    print_show_error(
                        format,
                        "ref attach-source",
                        "unknown_reference",
                        &format!("no reference note matches {query}"),
                        &candidates,
                    );
                    return 1;
                }
            }
        }
    }
    selected.sort_unstable();
    selected.dedup();
    // Hold the reference-mutation session (bob_sync.lock then scan.lock)
    // across every write-mode store and note update.
    let _session = if dry_run {
        None
    } else {
        match reference_mutation::acquire_session(&config.bob_dir, || {
            eprintln!("waiting for vault maintenance lock to attach sources…")
        }) {
            Ok(session) => Some(session),
            Err(error) => {
                eprintln!(
                    "{COMMAND_NAME}: error: could not take the reference-mutation session: {}",
                    mutation_session_error(&error),
                );
                return 1;
            }
        }
    };
    let orphan_stores: BTreeSet<String> =
        source_store::inspect_source_stores(&config.bob_dir, &index.rows)
            .orphans
            .into_iter()
            .filter_map(|issue| match issue {
                source_store::SourceStoreIssue::Orphan { store } => Some(store),
                _ => None,
            })
            .collect();
    let mut attached = 0usize;
    let mut skipped: Vec<(&str, String)> = Vec::new();
    let mut batch_paths = BTreeSet::new();
    let mut batch_count = 0usize;
    let mut commits = Vec::new();
    let mut failed = false;
    let mut pdftotext_checked = false;
    let mut json_results: Vec<AttachResultJson> = Vec::new();
    let named_only = !queries.is_empty();
    let automatic_adoption_only = search.is_empty() && queries.is_empty();
    // Report one row in human mode: the `attached`/`skipped` line, plus
    // one indented `candidate: verdict` line per candidate when the stem
    // resolved to several files. JSON mode collects the same rows and
    // prints a single envelope at the end, keeping stdout pure JSON.
    let report_row =
        |path: &str,
         status: &str,
         reason: Option<&str>,
         attempts: &[CandidateAttempt],
         multi: bool,
         json_results: &mut Vec<AttachResultJson>| {
            if json_mode {
                json_results.push(AttachResultJson {
                    path: path.to_string(),
                    status: status.to_string(),
                    reason: reason.map(str::to_string),
                    candidates: attempts
                        .iter()
                        .map(|attempt| {
                            let (matched, total) = attempt
                                .verified
                                .as_ref()
                                .map(|verified| {
                                    (
                                        Some(verified.matched),
                                        Some(verified.total),
                                    )
                                })
                                .unwrap_or((None, None));
                            AttachCandidateJson {
                                path: attempt.path.display().to_string(),
                                verdict: attempt.verdict.clone(),
                                matched,
                                total,
                            }
                        })
                        .collect(),
                });
                return;
            }
            match (status, reason) {
                ("adopted", _) => {
                    println!("{path}: adopted (saved at create)");
                }
                ("attached", Some(detail)) => {
                    println!("{path}: attached ({detail})");
                }
                ("attached", None) => println!("{path}: attached"),
                (_, Some(skip)) => println!("{path}: skipped ({skip})"),
                (_, None) => println!("{path}: skipped"),
            }
            if multi {
                for attempt in attempts {
                    if attempt
                        .verdict
                        .starts_with("saved source does not match:")
                    {
                        continue;
                    }
                    println!(
                        "  {}: {}",
                        attempt.path.display(),
                        attempt.verdict
                    );
                }
            }
        };
    for row_index in selected {
        let row = &index.rows[row_index];
        let matching_stores = matching_orphan_stores(row, &orphan_stores);
        if automatic_adoption_only && matching_stores.is_empty() {
            continue;
        }
        if let Some(reason) = ineligibility(&config.bob_dir, row) {
            if named_only {
                report_row(
                    &row.path,
                    "skipped",
                    Some(&reason),
                    &[],
                    false,
                    &mut json_results,
                );
                skipped.push((&row.path, reason));
            }
            continue;
        }
        let (Some(stem), Some(ref_type)) = (note_stem(row), store_type(row))
        else {
            let reason = "no candidate".to_string();
            report_row(
                &row.path,
                "skipped",
                Some(&reason),
                &[],
                false,
                &mut json_results,
            );
            skipped.push((&row.path, reason));
            continue;
        };
        let qualifying_stores =
            qualifying_orphan_stores(&config.bob_dir, row, &matching_stores);
        if qualifying_stores.len() > 1 {
            let attempts: Vec<CandidateAttempt> = qualifying_stores
                .iter()
                .map(|store| CandidateAttempt {
                    path: config.bob_dir.join(store).join("source.md"),
                    verified: None,
                    verdict: "qualifying saved source".to_string(),
                })
                .collect();
            let reason = "ambiguous saved sources".to_string();
            report_row(
                &row.path,
                "skipped",
                Some(&reason),
                &attempts,
                false,
                &mut json_results,
            );
            skipped.push((&row.path, reason));
            continue;
        }

        let mut adoption_failure: Option<CandidateAttempt> = None;
        if let Some(store) = qualifying_stores.first() {
            let pdf_path = config
                .bob_dir
                .join(row.source_pdf.as_deref().unwrap_or_default());
            let pages = match pdf_page_texts(&pdf_path, &mut pdftotext_checked)
            {
                Ok(pages) => pages,
                Err(error) => {
                    eprintln!("{COMMAND_NAME}: error: {error}");
                    return 1;
                }
            };
            let source_path = config.bob_dir.join(store).join("source.md");
            let attempt = attempt_candidate(
                &source_path,
                &row.title,
                false,
                &pdf_path,
                &pages,
            );
            if attempt.verified.is_some() {
                if dry_run {
                    report_row(
                        &row.path,
                        "adopted",
                        None,
                        &[],
                        false,
                        &mut json_results,
                    );
                    attached += 1;
                    continue;
                }
                match adopt_verified(&config.bob_dir, row, store) {
                    Ok(paths) => {
                        report_row(
                            &row.path,
                            "adopted",
                            None,
                            &[],
                            false,
                            &mut json_results,
                        );
                        attached += 1;
                        batch_paths.extend(paths);
                        batch_count += 1;
                        if batch_count >= ATTACH_BATCH_SIZE {
                            if let Err(error) = commit_batch(
                                &config.bob_dir,
                                &batch_paths,
                                batch_count,
                                &mut commits,
                            ) {
                                eprintln!("{COMMAND_NAME}: error: {error}");
                                return 1;
                            }
                            batch_paths.clear();
                            batch_count = 0;
                        }
                        continue;
                    }
                    Err(error) => {
                        report_row(
                            &row.path,
                            "skipped",
                            Some(&error),
                            &[],
                            false,
                            &mut json_results,
                        );
                        skipped.push((&row.path, error));
                        failed = true;
                        continue;
                    }
                }
            }
            let reason = attempt.verdict.clone();
            if reason.starts_with("unreadable candidate")
                || reason.starts_with("unreadable PDF annotations")
                || reason.contains("pandoc")
            {
                failed = true;
            }
            if search.is_empty() {
                let reason = format!("saved source does not match: {reason}");
                report_row(
                    &row.path,
                    "skipped",
                    Some(&reason),
                    &[attempt],
                    false,
                    &mut json_results,
                );
                skipped.push((&row.path, reason));
                continue;
            }
            if !json_mode {
                println!(
                    "{}: skipped (saved source does not match: {})",
                    row.path, attempt.verdict
                );
            }
            adoption_failure = Some(CandidateAttempt {
                path: source_path,
                verified: None,
                verdict: format!("saved source does not match: {reason}"),
            });
        }

        if search.is_empty() {
            let reason = "no saved source; pass -s".to_string();
            report_row(
                &row.path,
                "skipped",
                Some(&reason),
                &[],
                false,
                &mut json_results,
            );
            skipped.push((&row.path, reason));
            continue;
        }
        let candidates =
            find_all_candidates(&stem, &search, single_file_override);
        // An explicit `REF -s FILE` pair carries a human review of the
        // pairing, so it skips the title check but still verifies.
        let explicit_pair = single_file_override.is_some();
        let multi = candidates.len() > 1 && !explicit_pair;
        if candidates.is_empty() {
            let reason = "no candidate".to_string();
            let failed_source: Vec<CandidateAttempt> =
                adoption_failure.into_iter().collect();
            report_row(
                &row.path,
                "skipped",
                Some(&reason),
                &failed_source,
                false,
                &mut json_results,
            );
            skipped.push((&row.path, reason));
            continue;
        }
        let pdf_path = config
            .bob_dir
            .join(row.source_pdf.as_deref().unwrap_or_default());
        let pages = match pdf_page_texts(&pdf_path, &mut pdftotext_checked) {
            Ok(pages) => pages,
            Err(error) => {
                eprintln!("{COMMAND_NAME}: error: {error}");
                return 1;
            }
        };
        let attempts: Vec<CandidateAttempt> = candidates
            .iter()
            .map(|candidate| {
                attempt_candidate(
                    candidate,
                    &row.title,
                    explicit_pair,
                    &pdf_path,
                    &pages,
                )
            })
            .collect();
        let mut reported_attempts: Vec<CandidateAttempt> =
            adoption_failure.into_iter().collect();
        reported_attempts.extend(attempts.iter().cloned());
        for attempt in &attempts {
            if attempt.verdict.starts_with("unreadable candidate")
                || attempt.verdict.starts_with("unreadable PDF annotations")
                || attempt.verdict.contains("pandoc")
            {
                failed = true;
                break;
            }
        }
        let passing: Vec<usize> = attempts
            .iter()
            .enumerate()
            .filter(|(_, attempt)| attempt.verified.is_some())
            .map(|(index, _)| index)
            .collect();
        // Several candidates attach only through disambiguation: exactly
        // one verified candidate wins; zero or several keep the skip.
        let chosen = match (multi, passing.len()) {
            (false, 1) => Some(passing[0]),
            (false, 0) => None,
            (true, 1) => Some(passing[0]),
            _ => None,
        };
        let Some(chosen_index) = chosen else {
            let reason = if multi {
                format!(
                    "ambiguous: {} of {} verified",
                    passing.len(),
                    attempts.len()
                )
            } else {
                attempts
                    .first()
                    .map(|attempt| attempt.verdict.clone())
                    .unwrap_or_else(|| "no candidate".to_string())
            };
            report_row(
                &row.path,
                "skipped",
                Some(&reason),
                &reported_attempts,
                multi,
                &mut json_results,
            );
            skipped.push((&row.path, reason));
            continue;
        };
        let attempt = &attempts[chosen_index];
        let candidate = attempt.path.clone();
        let verified = attempt
            .verified
            .as_ref()
            .expect("chosen candidate verified");
        if dry_run {
            let detail = multi.then(|| {
                format!(
                    "1 of {} verified: {}",
                    attempts.len(),
                    candidate.display()
                )
            });
            report_row(
                &row.path,
                "attached",
                detail.as_deref(),
                &reported_attempts,
                multi,
                &mut json_results,
            );
            attached += 1;
            continue;
        }
        let pdf_sha256 = match sha256_file(&pdf_path) {
            Ok(sha) => sha,
            Err(error) => {
                let reason = format!("unreadable pdf: {error}");
                report_row(
                    &row.path,
                    "skipped",
                    Some(&reason),
                    &reported_attempts,
                    multi,
                    &mut json_results,
                );
                skipped.push((&row.path, reason));
                failed = true;
                continue;
            }
        };
        match attach_verified(
            &config.bob_dir,
            row,
            &stem,
            &ref_type,
            &candidate,
            verified,
            &pdf_sha256,
        ) {
            Ok(paths) => {
                let detail = multi.then(|| {
                    format!(
                        "1 of {} verified: {}",
                        attempts.len(),
                        candidate.display()
                    )
                });
                report_row(
                    &row.path,
                    "attached",
                    detail.as_deref(),
                    &reported_attempts,
                    multi,
                    &mut json_results,
                );
                attached += 1;
                batch_paths.extend(paths);
                batch_count += 1;
                if batch_count >= ATTACH_BATCH_SIZE {
                    if let Err(error) = commit_batch(
                        &config.bob_dir,
                        &batch_paths,
                        batch_count,
                        &mut commits,
                    ) {
                        eprintln!("{COMMAND_NAME}: error: {error}");
                        return 1;
                    }
                    batch_paths.clear();
                    batch_count = 0;
                }
            }
            Err(error) => {
                report_row(
                    &row.path,
                    "skipped",
                    Some(error.as_str()),
                    &reported_attempts,
                    multi,
                    &mut json_results,
                );
                skipped.push((&row.path, error));
                failed = true;
            }
        }
    }
    if !dry_run && batch_count > 0 {
        if let Err(error) = commit_batch(
            &config.bob_dir,
            &batch_paths,
            batch_count,
            &mut commits,
        ) {
            eprintln!("{COMMAND_NAME}: error: {error}");
            return 1;
        }
    }
    if json_mode {
        let envelope = AttachEnvelope {
            ok: !failed,
            schema_version: REF_SCHEMA_VERSION,
            command: "ref attach-source",
            generated_at: generated_at(),
            dry_run,
            attached,
            skipped: skipped.len(),
            results: json_results,
        };
        println!(
            "{}",
            serde_json::to_string(&envelope)
                .expect("attach envelope serializes")
        );
        return if failed { 1 } else { 0 };
    }
    let mut reasons: BTreeSet<&str> = BTreeSet::new();
    for (_, reason) in &skipped {
        reasons.insert(reason.as_str());
    }
    let breakdown = reasons
        .iter()
        .map(|reason| {
            let count = skipped.iter().filter(|(_, r)| r == reason).count();
            format!("{count} {reason}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    if breakdown.is_empty() {
        println!("summary: {attached} attached, 0 skipped");
    } else {
        println!(
            "summary: {attached} attached, {} skipped ({breakdown})",
            skipped.len()
        );
    }
    if dry_run {
        println!("writes: none");
    }
    if failed {
        1
    } else {
        0
    }
}

fn mutation_session_error(
    error: &reference_mutation::MutationSessionError,
) -> String {
    match error {
        reference_mutation::MutationSessionError::Maintenance(error) => {
            error.to_string()
        }
        reference_mutation::MutationSessionError::NotAWorktree => {
            "vault is not a Git worktree".to_string()
        }
        reference_mutation::MutationSessionError::Scan(error) => {
            error.to_string()
        }
    }
}

fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path)?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_record_pass_preserves_route_section_numbering() {
        if pandoc_command().is_none() {
            eprintln!("skipping record pass: pandoc is required");
            return;
        }
        let source = b"# Report\n\n## Details\n\nSee [link](https://example.test).\n\n```rust\nlet saved = 1;\n```\n";
        for route in ["markdown", "plain_note", "attached", "note_snapshot"] {
            let catalog =
                record_source_catalog(source, route).expect("record pass");
            let expected_number = (route != "note_snapshot").then_some("1");
            let link = &catalog.links.as_ref().unwrap()[0];
            let section = link.section.as_ref().unwrap();
            assert_eq!(section.title, "Details");
            assert_eq!(section.number.as_deref(), expected_number, "{route}");
            assert_eq!(catalog.blocks[0].section.as_ref().unwrap(), section);
            assert_eq!(catalog.blocks[0].text, "let saved = 1;");
            assert_eq!(link.page, None);
        }
        assert!(record_source_catalog(source, "unsupported")
            .unwrap_err()
            .contains("unsupported saved-source route"));
    }

    #[test]
    fn attach_json_envelope_names_every_candidate() {
        let envelope = AttachEnvelope {
            ok: true,
            schema_version: REF_SCHEMA_VERSION,
            command: "ref attach-source",
            generated_at: "2026-10-27T00:00:00".to_string(),
            dry_run: true,
            attached: 0,
            skipped: 1,
            results: vec![AttachResultJson {
                path: "ref/chat/report.md".to_string(),
                status: "skipped".to_string(),
                reason: Some("ambiguous: 0 of 2 verified".to_string()),
                candidates: vec![
                    AttachCandidateJson {
                        path: "/sidecar-a/report.md".to_string(),
                        verdict: "title mismatch".to_string(),
                        matched: None,
                        total: None,
                    },
                    AttachCandidateJson {
                        path: "/sidecar-b/report.md".to_string(),
                        verdict: "verified".to_string(),
                        matched: Some(2),
                        total: Some(2),
                    },
                ],
            }],
        };
        let json =
            serde_json::to_string(&envelope).expect("envelope serializes");
        assert!(json.contains("ambiguous: 0 of 2 verified"));
        assert!(json.contains("/sidecar-a/report.md"));
        assert!(json.contains("\"matched\":2"));
    }

    #[test]
    fn titles_compare_case_and_whitespace_insensitively() {
        assert!(titles_match("Harbor Cranes", "harbor  cranes"));
        assert!(!titles_match("Harbor Cranes", "Desert Owls"));
    }

    #[test]
    fn prose_stripping_keeps_words_not_markup() {
        assert_eq!(
            strip_prose_formatting("- *Positions:* `cdx` vs. **Sources**"),
            "Positions: cdx vs. Sources"
        );
        assert_eq!(strip_prose_formatting("# A MEMORY Deck"), "A MEMORY Deck");
        assert_eq!(
            strip_prose_formatting("| 09:26 | launched |"),
            "09:26 launched"
        );
        assert_eq!(
            strip_prose_formatting("[Measurements](#m) data"),
            "Measurements data"
        );
        // Snake-case identifiers and globs survive verbatim.
        assert_eq!(
            strip_prose_formatting("reads `wait_dependencies_satisfied_at`"),
            "reads wait_dependencies_satisfied_at"
        );
        assert_eq!(
            strip_prose_formatting("every `*.md` file"),
            "every *.md file"
        );
    }

    #[test]
    fn compatibility_folding_unifies_quotes_and_dashes() {
        assert_eq!(
            normalize_text("11q’s “plan” — done…"),
            "11q's\"plan\"-done..."
        );
    }

    #[test]
    fn longest_paragraphs_select_but_keep_document_order() {
        let bytes =
            b"# T\n\nshort\n\nthe longest paragraph here yes\n\nmid one here\n";
        let paragraphs = longest_paragraphs(bytes, 2);
        assert_eq!(paragraphs.len(), 2);
        // Longest two, in document order.
        assert!(paragraphs[0].contains("longest paragraph"));
        assert!(paragraphs[1].contains("mid one"));
    }
}
