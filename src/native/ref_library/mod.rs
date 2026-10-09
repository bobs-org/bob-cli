//! Read-only reference-library index (phase `index`).
//!
//! [`build_index`] walks `<ref-dir>/**/*.md` once and yields one [`RefRow`]
//! per note: status precedence and derived reading state ([`status`]), stored
//! identity keys ([`identity`]), dates, origin, supersession, diagnostics,
//! and coverage, plus query resolution and title scoring ([`resolve`]). The
//! index never writes; later phases (`find`, `list`, `show`, `doctor`) read
//! these rows.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub(crate) mod cli;
mod coverage;
mod find;
mod frontmatter;
mod identity;
pub(crate) mod list;
pub(crate) mod output;
mod resolve;
mod row;
pub(crate) mod show;
mod status;

#[cfg(test)]
mod tests;

pub(crate) use coverage::{
    count_zorg_records, normalize_block_id, zorg_source_files, ZorgCoverage,
};
pub(crate) mod migrate_tasks;
pub(crate) mod migrate_zorg;
// Reserved for the `planner` phase: the shared record-parser API.
#[allow(unused_imports)]
pub(crate) use coverage::{parse_zorg_records, record_is_mirrored, ZorgRecord};
pub(crate) use frontmatter::ParsedFrontmatter;
pub(crate) use identity::{
    classify_query, doi_keys, normalize_doi_query, parse_arxiv_query,
    stored_identity, url_query_keys, QueryKind,
};
pub(crate) use list::{
    fill_git_dates, filter_list, parse_since_cutoff, row_date, select,
    validate_since, ListSelection, LIST_STATE_ORDER,
};
pub(crate) use resolve::{
    path_matches, primary_rank, resolve_query, MatchKind,
};
pub(crate) use row::{
    bare_parent_name, strip_wikilink_brackets, Coverage, Diagnostic,
    LibraryCounts, RefIdentity, RefRow, RefSnapshot, RefTaskView,
};
pub(crate) use status::{decide_status, TrackerHit};

use frontmatter::FrontValue;

use crate::native::highlights_ref::{
    humanize_stem, parse_managed_region, split_frontmatter, split_note_body,
    RegionBlockKind,
};

/// Library locations, resolved like the Highlights config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LibraryConfig {
    pub bob_dir: PathBuf,
    pub ref_dir: PathBuf,
    pub xlib_dir: PathBuf,
}

impl LibraryConfig {
    #[cfg(test)]
    pub(crate) fn for_ref_dir(bob_dir: PathBuf, ref_dir: PathBuf) -> Self {
        Self {
            xlib_dir: bob_dir.join("xlib"),
            bob_dir,
            ref_dir,
        }
    }
}

/// An index build failure (a missing ref dir or an unreadable directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LibraryError {
    message: String,
}

impl LibraryError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LibraryError {}

/// The built index: one row per note plus coverage and library counts.
#[derive(Debug, Clone)]
pub(crate) struct RefIndex {
    pub bob_dir: PathBuf,
    pub rows: Vec<RefRow>,
    pub coverage: Coverage,
    pub counts: LibraryCounts,
    pub ref_tasks: crate::native::ref_tasks::RefTaskIndex,
}

impl RefIndex {
    #[cfg(test)]
    pub(crate) fn row_by_path(&self, path: &str) -> Option<&RefRow> {
        self.rows.iter().find(|row| row.path == path)
    }
}

/// Build the read-only index over `<ref-dir>/**/*.md`.
pub(crate) fn build_index(
    config: &LibraryConfig,
) -> Result<RefIndex, LibraryError> {
    if !config.ref_dir.is_dir() {
        return Err(LibraryError::new(format!(
            "reference directory not found: {}",
            config.ref_dir.display()
        )));
    }
    let ref_tasks = crate::native::ref_tasks::RefTaskIndex::build(
        &config.bob_dir,
        &config.ref_dir,
    );
    let mut files = Vec::new();
    let mut skipped = 0usize;
    collect_member_files(&config.ref_dir, &mut files, &mut skipped)?;
    files.sort();
    let mut rows = Vec::with_capacity(files.len());
    for path in &files {
        // Display paths are vault-relative (`ref/papers/x.md`); the
        // under-ref path drives `ref_type` and `origin`.
        let display = strip_forward(path, &config.bob_dir)
            .or_else(|| strip_forward(path, &config.ref_dir))
            .expect("member under ref dir");
        let under_ref =
            strip_forward(path, &config.ref_dir).expect("member under ref dir");
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let body = split_frontmatter(&contents)
                    .map(|(_, rest)| rest)
                    .unwrap_or(contents.clone());
                let v1_hits = crate::native::ref_tasks::find_trackers(&body);
                let mut selection = ref_tasks.select(&display, &v1_hits);
                // The note is v2 when any candidate exists or the body
                // holds the managed embed.
                if crate::native::ref_tasks::find_managed_embed(&body).is_some()
                {
                    selection.v2 = true;
                }
                rows.push(build_row(
                    &display, &under_ref, &contents, selection,
                ));
            }
            Err(error) => {
                let selection =
                    crate::native::ref_tasks::RefTaskIndex::build_empty_selection();
                let mut row = build_row(&display, &under_ref, "", selection);
                row.diagnostics.push(Diagnostic::new(
                    "invalid_yaml",
                    format!("could not read note: {error}"),
                ));
                rows.push(row);
            }
        }
    }
    apply_supersession(&mut rows);
    for row in &mut rows {
        row.diagnostics
            .sort_by(|a, b| (&a.code, &a.detail).cmp(&(&b.code, &b.detail)));
    }
    let counts = library_counts(&rows);
    let coverage = Coverage {
        ref_dir: config
            .ref_dir
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("ref")
            .to_string(),
        notes: rows.len(),
        skipped,
        intake: "not_checked".to_string(),
        scope: Coverage::scope_text(),
        annotations: Coverage::annotations_text(),
        git_dates: None,
    };
    Ok(RefIndex {
        bob_dir: config.bob_dir.clone(),
        rows,
        coverage,
        counts,
        ref_tasks,
    })
}

/// Collect member files: `<ref-dir>/**/*.md` minus hidden directories,
/// `*.assets/` directories, and conflict copies.
fn collect_member_files(
    dir: &Path,
    files: &mut Vec<PathBuf>,
    skipped: &mut usize,
) -> Result<(), LibraryError> {
    let entries = std::fs::read_dir(dir).map_err(|error| {
        LibraryError::new(format!("scan {}: {error}", dir.display()))
    })?;
    let mut ordered =
        entries.collect::<Result<Vec<_>, _>>().map_err(|error| {
            LibraryError::new(format!("scan {}: {error}", dir.display()))
        })?;
    ordered.sort_by_key(|entry| entry.file_name());
    for entry in ordered {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            LibraryError::new(format!("stat {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            let name =
                path.file_name().and_then(OsStr::to_str).unwrap_or_default();
            if name.starts_with('.') || name.ends_with(".assets") {
                *skipped += count_markdown_under(&path);
                continue;
            }
            collect_member_files(&path, files, skipped)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        let is_markdown = path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if crate::native::ref_tasks::is_conflict_copy_name(name) {
            // Only Markdown conflict copies count as skipped notes;
            // non-Markdown conflict files are not library members.
            if is_markdown {
                *skipped += 1;
            }
            continue;
        }
        if !is_markdown {
            continue;
        }
        files.push(path);
    }
    Ok(())
}

fn count_markdown_under(dir: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                count += 1;
            }
        }
    }
    count
}

/// Forward-slash `path` relative to `base`, or `None` when outside.
fn strip_forward(path: &Path, base: &Path) -> Option<String> {
    path.strip_prefix(base)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
}

/// Build one row from vault-relative path, raw note contents, and the
/// ref-task selection for this note.
fn build_row(
    rel: &str,
    under_ref: &str,
    contents: &str,
    selection: crate::native::ref_tasks::RefTaskSelection,
) -> RefRow {
    let (raw_lines, body) = split_frontmatter(contents)
        .unwrap_or_else(|| (Vec::new(), contents.to_string()));
    let front = ParsedFrontmatter::parse(&raw_lines);
    let mut diagnostics = Vec::new();
    if let Some(detail) = front.invalid_yaml.clone() {
        diagnostics.push(Diagnostic::new("invalid_yaml", detail));
    }

    let outcome = decide_status(&selection.status_hits, &front);
    for item in &selection.diagnostics {
        diagnostics.push(Diagnostic::new(item.code, item.detail.clone()));
    }
    diagnostics.extend(outcome.diagnostics.clone());
    let era = if !outcome.has_usable_tracker
        && outcome.status.as_deref() == Some("legacy")
    {
        "legacy"
    } else {
        "modern"
    };

    let parts = split_note_body(&body);
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let stem = file_name
        .strip_suffix(".md")
        .or_else(|| file_name.strip_suffix(".MD"))
        .unwrap_or(file_name);
    let title = front
        .get_str("title")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| parts.h1.clone())
        .unwrap_or_else(|| humanize_stem(stem));

    let ref_type = front
        .get_str("ref_type")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| first_dir_under_ref(under_ref));
    if ref_type.is_none() {
        diagnostics.push(Diagnostic::new(
            "missing_type",
            "no ref_type and no parent directory under ref/".to_string(),
        ));
    }
    let origin = if under_ref.starts_with("chat/")
        || ref_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("chat"))
    {
        "agent-report"
    } else {
        "external"
    };

    let front_parent = front
        .get_all("parent")
        .iter()
        .filter_map(|value| bare_parent_name(value))
        .next();
    // Parent: for a v2 selection with residence, the parent is the
    // residence; otherwise the frontmatter parent.
    let parent = if let Some(residence) = selection.residence.as_deref() {
        Some(residence.to_string())
    } else {
        front_parent.clone()
    };
    // parent_mismatch: a selected v2 task with residence r, and a
    // frontmatter parent missing or not ASCII-case-equal to r.
    if let Some(residence) = selection.residence.as_deref() {
        let matches = front_parent
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case(residence));
        if !matches {
            let shown = front_parent.as_deref().unwrap_or("missing");
            diagnostics.push(Diagnostic::new(
                "parent_mismatch",
                format!(
                    "frontmatter parent {shown} ≠ residence {residence}; the next scan sets it",
                ),
            ));
        }
    }
    let mut urls = front.get_all("source_url");
    urls.extend(front.get_all("url"));
    let stored = stored_identity(&urls);
    for opaque in &stored.opaque {
        diagnostics.push(Diagnostic::new(
            "opaque_url",
            format!("non-public or non-URL value: {opaque}"),
        ));
    }

    let (annotation_count, comment_count, region_diagnostics) =
        annotation_counts(parts.region.as_deref());
    diagnostics.extend(region_diagnostics);

    let (added, added_source) = added_date(&front);
    let finished_state =
        matches!(outcome.reading_state, "finished" | "dropped");
    let (finished, finished_source) = if finished_state {
        finished_date(outcome.tracker.as_ref())
    } else {
        (None, None)
    };
    // open_ref_without_task: the note is v2 (a candidate exists or the
    // body holds the managed embed), no candidates and no v1 hits, and
    // the decided status is ready/next/wip. Rule 2 ambiguity also has
    // empty hits but does hold candidates, so it is excluded.
    if selection.v2
        && selection.status_hits.is_empty()
        && !selection
            .diagnostics
            .iter()
            .any(|d| d.code == "multiple_open_ref_tasks")
        && matches!(outcome.status.as_deref(), Some("ready" | "next" | "wip"))
    {
        // No candidates: check the selection really has none. `v2` may be
        // true via the managed embed alone.
        diagnostics.push(Diagnostic::new(
            "open_ref_without_task",
            format!(
                "status {} but no reading task found; restore it from git or set status: abandoned",
                outcome.status.as_deref().unwrap_or("unknown"),
            ),
        ));
    }

    // Task view: v2 selections carry the residence file and block link;
    // v1 carries the ref note with block `ref`; ambiguity and no-task
    // both give null.
    let task = match &selection.task {
        Some(crate::native::ref_tasks::Selected::V2(located)) => {
            let stem = located
                .path
                .strip_suffix(".md")
                .or_else(|| located.path.strip_suffix(".MD"))
                .unwrap_or(&located.path);
            let link = match &located.block_id {
                Some(id) => format!("[[{stem}#^{id}]]"),
                None => format!("[[{stem}]]"),
            };
            Some(RefTaskView {
                path: located.path.clone(),
                block_id: located.block_id.clone(),
                link,
                mark: located.mark,
                archived: located.archived,
            })
        }
        Some(crate::native::ref_tasks::Selected::V1(_)) | None => {
            if outcome.has_usable_tracker
                && let Some(hit) = outcome.tracker.as_ref()
            {
                Some(RefTaskView {
                    path: rel.to_string(),
                    block_id: Some("ref".to_string()),
                    link: format!(
                        "[[{}#^ref]]",
                        rel.strip_suffix(".md").unwrap_or(rel)
                    ),
                    mark: hit.mark,
                    archived: false,
                })
            } else {
                None
            }
        }
    };
    // Rule 2 ambiguity (multiple open) must give null even when a usable
    // v1 tracker exists: detect via the diagnostic.
    let task = if selection
        .diagnostics
        .iter()
        .any(|d| d.code == "multiple_open_ref_tasks")
    {
        None
    } else {
        task
    };

    let link = format!("[[{}]]", rel.strip_suffix(".md").unwrap_or(rel));
    RefRow {
        path: rel.to_string(),
        link,
        title,
        origin: origin.to_string(),
        ref_type,
        era: era.to_string(),
        status: outcome.status.clone(),
        status_sync: outcome.status_sync.to_string(),
        frontmatter_status: outcome.frontmatter_status.clone(),
        legacy_status: front
            .get_str("legacy_status")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        blocked: outcome.blocked,
        reading_state: outcome.reading_state.to_string(),
        reading_state_source: outcome.reading_state_source.clone(),
        parent,
        task,
        urls,
        identity: RefIdentity {
            keys: stored.keys,
            arxiv: stored.arxiv,
            doi: stored.doi,
        },
        author: front_string(&front, "author"),
        published: front_string(&front, "published"),
        captured: front_string(&front, "captured"),
        added,
        added_source,
        finished,
        finished_source,
        source_pdf: front
            .get_str("source_pdf")
            .map(strip_wikilink_brackets)
            .filter(|s| !s.is_empty()),
        audio: front
            .get_str("audio")
            .map(strip_wikilink_brackets)
            .filter(|s| !s.is_empty()),
        annotation_count,
        comment_count,
        snapshot: snapshot(&front),
        research_ref: front
            .get_str("research")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|value| format!("research:{value}")),
        superseded_by: None,
        diagnostics,
        id: front_string(&front, "id"),
        source_block: front_string(&front, "source_block"),
        source_path: front_string(&front, "source_path"),
        source_id: front_string(&front, "source_id"),
        source_blocks: front
            .get_all("source_blocks")
            .iter()
            .map(|block| normalize_block_id(block))
            .filter(|block| !block.is_empty() && block != "^")
            .collect(),
        v2: selection.v2,
    }
}

fn front_string(front: &ParsedFrontmatter, key: &str) -> Option<String> {
    front
        .get_str(key)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_dir_under_ref(rel: &str) -> Option<String> {
    let mut parts = rel.split('/');
    let first = parts.next()?;
    parts.next()?;
    (!first.is_empty()).then(|| first.to_string())
}

/// Live annotation counts from the managed region, excluding mirror-shaped,
/// preamble, and tombstone blocks.
fn annotation_counts(region: Option<&str>) -> (usize, usize, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let Some(content) = region else {
        return (0, 0, diagnostics);
    };
    let parsed = parse_managed_region(content);
    if !parsed.unparsed.is_empty() {
        let snippet = parsed.unparsed[0].chars().take(80).collect::<String>();
        diagnostics.push(Diagnostic::new(
            "unparsed_region",
            format!(
                "{} unparsed region block(s); first: {snippet}",
                parsed.unparsed.len()
            ),
        ));
    }
    let mut annotations = 0usize;
    let mut comments = 0usize;
    let mut mirrors = Vec::new();
    let mut preambles = 0usize;
    for block in &parsed.blocks {
        if block.mirror {
            mirrors.push(block.block_id.clone());
            continue;
        }
        if block.in_preamble {
            preambles += 1;
            continue;
        }
        annotations += 1;
        match block.kind {
            RegionBlockKind::Highlight => {
                if block.comment.as_deref().is_some_and(|c| !c.is_empty()) {
                    comments += 1;
                }
            }
            RegionBlockKind::Note => comments += 1,
            RegionBlockKind::Image => {
                if block.comment.as_deref().is_some_and(|c| !c.is_empty()) {
                    comments += 1;
                }
            }
        }
    }
    if !mirrors.is_empty() {
        mirrors.sort();
        diagnostics.push(Diagnostic::new(
            "marker_mirror_excluded",
            format!(
                "{} mirror block(s) excluded: {}",
                mirrors.len(),
                mirrors.join(", ")
            ),
        ));
    }
    if preambles > 0 {
        diagnostics.push(Diagnostic::new(
            "preamble_excluded",
            format!("{preambles} preamble block(s) excluded"),
        ));
    }
    (annotations, comments, diagnostics)
}

/// The `added` date: `captured`, else the date part of `created`, else the
/// zorg `source_block` date (`^z-YYMMDD-…` → `20YY-MM-DD`).
fn added_date(front: &ParsedFrontmatter) -> (Option<String>, Option<String>) {
    if let Some(captured) = front_string(front, "captured") {
        return (Some(captured), Some("captured".to_string()));
    }
    if let Some(created) = front_string(front, "created") {
        return (Some(date_part(&created)), Some("created".to_string()));
    }
    if let Some(block) = front_string(front, "source_block")
        && let Some(date) = zorg_block_date(&block)
    {
        return (Some(date), Some("zorg_block".to_string()));
    }
    (None, None)
}

fn date_part(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 10
        && chrono::NaiveDate::parse_from_str(&trimmed[..10], "%Y-%m-%d").is_ok()
    {
        return trimmed[..10].to_string();
    }
    trimmed.to_string()
}

fn zorg_block_date(block: &str) -> Option<String> {
    let rest = block.trim().strip_prefix("^z-")?;
    if rest.len() < 6 || !rest[..6].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let year = rest[..2].parse::<u32>().ok()?;
    let month = rest[2..4].parse::<u32>().ok()?;
    let day = rest[4..6].parse::<u32>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(format!("20{year:02}-{month:02}-{day:02}"))
}

/// The `finished` date for finished/dropped rows: the selected hit's line
/// via `ref_tasks::close_date` (`[completion:: D]`, `[cancelled:: D]`,
/// `✅ D`/`✔ D`, `❌ D`/`✖ D`).
fn finished_date(
    tracker: Option<&status::TrackerHit>,
) -> (Option<String>, Option<String>) {
    let Some(hit) = tracker else {
        return (None, None);
    };
    if let Some(date) = crate::native::ref_tasks::close_date(&hit.line) {
        return (Some(date), Some("ref_task".to_string()));
    }
    (None, None)
}

fn snapshot(front: &ParsedFrontmatter) -> Option<RefSnapshot> {
    let synced_at = front_string(front, "highlights_synced_at");
    let highlights_count = match front.raw("highlights_count") {
        Some(FrontValue::Str(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else if let Ok(number) = trimmed.parse::<i64>() {
                Some(serde_json::Value::Number(number.into()))
            } else if let Ok(number) = trimmed.parse::<f64>()
                && number.is_finite()
            {
                serde_json::Number::from_f64(number)
                    .map(serde_json::Value::Number)
            } else {
                Some(serde_json::Value::String(trimmed.to_string()))
            }
        }
        Some(FrontValue::List(values)) => Some(serde_json::Value::Array(
            values
                .iter()
                .map(|v| serde_json::Value::String(v.clone()))
                .collect(),
        )),
        None => None,
    };
    if synced_at.is_none() && highlights_count.is_none() {
        return None;
    }
    Some(RefSnapshot {
        synced_at,
        highlights_count,
    })
}

/// Supersession: a PDF-less note sharing an identity key with a PDF-backed
/// note is `superseded_by` the first such note by path. Keys shared by
/// several PDF-backed notes earn a `duplicate_identity` diagnostic.
fn apply_supersession(rows: &mut [RefRow]) {
    // Owned keys: the sharing map must not borrow the rows it reorders.
    let mut by_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let mut seen = BTreeSet::new();
        for key in &row.identity.keys {
            if seen.insert(key.clone()) {
                by_key.entry(key.clone()).or_default().push(index);
            }
        }
    }
    let mut first_pdf_backed: BTreeMap<String, usize> = BTreeMap::new();
    for (key, sharing) in &by_key {
        let mut backed = sharing
            .iter()
            .copied()
            .filter(|index| rows[*index].source_pdf.is_some())
            .collect::<Vec<_>>();
        if backed.is_empty() {
            continue;
        }
        backed.sort_by_key(|index| &rows[*index].path);
        first_pdf_backed.insert(key.clone(), backed[0]);
        if backed.len() > 1 {
            let paths = backed
                .iter()
                .map(|index| rows[*index].path.clone())
                .collect::<Vec<_>>();
            let detail = format!(
                "shared identity {key} across PDFs: {}",
                paths.join(", ")
            );
            for index in sharing {
                rows[*index]
                    .diagnostics
                    .push(Diagnostic::new("duplicate_identity", &detail));
            }
        }
    }
    // Resolve winner paths first so the mutation pass holds no row borrows.
    let winners: Vec<Option<String>> = rows
        .iter()
        .map(|row| {
            if row.source_pdf.is_some() {
                return None;
            }
            let mut candidates = row
                .identity
                .keys
                .iter()
                .filter_map(|key| first_pdf_backed.get(key))
                .copied()
                .collect::<Vec<_>>();
            candidates.sort_by_key(|winner| &rows[*winner].path);
            candidates.first().map(|winner| rows[*winner].path.clone())
        })
        .collect();
    for (row, winner) in rows.iter_mut().zip(winners) {
        if winner.as_deref() != Some(row.path.as_str()) {
            row.superseded_by = winner;
        }
    }
}

/// Library counts over non-superseded rows.
fn library_counts(rows: &[RefRow]) -> LibraryCounts {
    let mut counts = LibraryCounts::default();
    for row in rows.iter().filter(|row| row.superseded_by.is_none()) {
        counts.notes += 1;
        match row.reading_state.as_str() {
            "finished" => counts.finished += 1,
            "started" => counts.started += 1,
            "queued" => counts.queued += 1,
            "dropped" => counts.dropped += 1,
            _ => counts.unknown += 1,
        }
    }
    counts
}
