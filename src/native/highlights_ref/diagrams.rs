//! Collect, render, and swap Mermaid code blocks for `bob ref create` PDFs.
//!
//! [`prepare`] never fails the import: a missing renderer, adapter crash,
//! timeout, or collect error writes fallback notes and warnings. The env
//! kill switch [`ENV_DIAGRAMS`] skips the pass entirely.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};

use super::{
    clip_adapter::{
        AdapterFailure, ClipAdapterClient, DiagramItem, DiagramsRequest,
    },
    CommandError, Result,
};
use crate::native::style::Styler;

/// Env kill switch (A6). Values `off`, `0`, `false`, or `no`
/// (case-insensitive) disable the diagram pass.
pub(crate) const ENV_DIAGRAMS: &str = "BOB_HIGHLIGHTS_DIAGRAMS";

/// Unique diagrams sent to the adapter; extras degrade with [`LIMIT_ERROR`].
pub(crate) const MAX_DIAGRAM_ITEMS: usize = 200;
const LIMIT_ERROR: &str =
    "too many Mermaid diagrams in one document (limit 200)";
const SCALE_WARN_BELOW: f64 = 0.5;

/// Pandoc metadata key carrying the collect-pass manifest path.
pub(crate) const MANIFEST_METADATA_KEY: &str = "bob-diagrams-manifest";

/// Pandoc metadata key carrying the swap-filter results path.
pub(crate) const RESULTS_METADATA_KEY: &str = "bob-diagrams-results";

/// Throwaway collect filter. Embedded like [`super::return_links::FILTER`].
pub(crate) const COLLECT_FILTER: &str = include_str!("diagrams_collect.lua");

/// First-position swap filter for both Markdown PDF routes.
pub(crate) const SWAP_FILTER: &str = include_str!("diagrams.lua");

/// TeX height-budget macros for fitted Mermaid diagrams.
pub(crate) const HEADER_INCLUDES: &str = include_str!("diagrams.tex");

/// Fraction of the text height one rendered diagram may fill.
///
/// Must equal `\BobFitHeightBudget` in `diagrams.tex`.
pub(crate) const FIT_HEIGHT_BUDGET: f64 = 0.8;

/// Paths the swap filter needs when [`super::create::RenderFilters::diagrams`]
/// is `Some`. Both Markdown PDF routes pass this after [`prepare`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagramFilter {
    pub(crate) filter: PathBuf,
    pub(crate) results: PathBuf,
}

/// `--lua-filter` then `--metadata bob-diagrams-results=…`. `filter` and
/// `results` are the paths the renderer should pass (already resolved
/// for cwd=scratch on the snapshot route).
pub(crate) fn apply_swap_filter_args(
    cmd: &mut Command,
    filter: &Path,
    results: &Path,
) {
    cmd.arg("--lua-filter")
        .arg(filter)
        .arg("--metadata")
        .arg(format!("{}={}", RESULTS_METADATA_KEY, results.display()));
}

/// Reader flags for the collect pass: the same flags the route's main
/// pandoc invocation uses, so keys always agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagramReader {
    /// Generic Markdown route: no `-f`, pandoc infers from the path.
    Inferred,
    /// Snapshot route: [`super::note_snapshot::profile::SNAPSHOT_PANDOC_READER`].
    Snapshot,
}

/// One unique Mermaid source, in first-seen document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollectedDiagram {
    pub(crate) id: String,
    pub(crate) text: String,
    /// One-based position of this source's first block in the document.
    pub(crate) first_occurrence: usize,
    pub(crate) occurrences: usize,
}

/// Per-id swap entry: a rendered PDF or a visible error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum DiagramOutcome {
    Pdf { pdf: String },
    Error { error: String },
}

/// `results.json` the swap filter reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DiagramResults {
    pub(crate) diagrams: BTreeMap<String, DiagramOutcome>,
    pub(crate) fallback_error: Option<String>,
}

/// Markdown the collect pass reads: the generic route's source path, or
/// the snapshot route's projected text (written to `diagrams/input.md`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum DiagramSource<'a> {
    Path(&'a Path),
    Text(&'a str),
}

/// Reader flags plus the route's text box for the budgeted scale check.
///
/// The height limit is `text_height_pt * FIT_HEIGHT_BUDGET`, matching the
/// TeX `\BobFitHeightBudget` budget in `diagrams.tex`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DiagramRoute {
    pub(crate) reader: DiagramReader,
    pub(crate) text_width_pt: f64,
    pub(crate) text_height_pt: f64,
}

/// Swap-filter paths plus the summary/warning lines for the success block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagramPass {
    pub(crate) filter: Option<DiagramFilter>,
    total: usize,
    rendered: usize,
    warnings: Vec<String>,
}

impl DiagramPass {
    fn skipped() -> Self {
        Self {
            filter: None,
            total: 0,
            rendered: 0,
            warnings: Vec::new(),
        }
    }

    /// `diagrams: rendered N of M` on stdout when blocks exist, then
    /// stderr warnings with [`Styler::warning_prefix`].
    pub(crate) fn emit(&self, styler: &Styler) {
        if self.total > 0 {
            println!(
                "diagrams: rendered {} of {} Mermaid diagrams",
                self.rendered, self.total
            );
        }
        for warning in &self.warnings {
            eprintln!("{}: {warning}", styler.warning_prefix());
        }
    }
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    id: String,
    text: String,
}

/// `false` when `BOB_HIGHLIGHTS_DIAGRAMS` is `off`/`0`/`false`/`no`
/// (case-insensitive). Missing or any other value leaves the pass on.
pub(crate) fn enabled() -> bool {
    match crate::native::env::var(ENV_DIAGRAMS) {
        Ok(value) => {
            let folded = value.trim().to_ascii_lowercase();
            !matches!(folded.as_str(), "off" | "0" | "false" | "no")
        }
        Err(_) => true,
    }
}

/// Mermaid-specific dry-run summary. The plan remains browser-free: this only
/// counts candidate fence openers in text already available to the planner.
pub(crate) fn dry_run_line(markdown: &str) -> Option<String> {
    let count = prescan(markdown);
    if count == 0 {
        return None;
    }
    Some(if enabled() {
        format!("diagrams: {count} Mermaid fence(s) (rendered at create time)")
    } else {
        format!(
            "diagrams: {count} Mermaid fence(s) (rendering disabled by {ENV_DIAGRAMS}=off)"
        )
    })
}

/// Count candidate Mermaid fence openers. May over-match (a nested
/// example inside a `markdown` fence) and must never under-match.
///
/// Strips leading whitespace, `>` quote markers, and list markers, then
/// looks for Mermaid fence openers. It also recognizes `:` and `~`
/// definition-list markers before the fence; Pandoc collection decides
/// whether each candidate is an actual Mermaid code block.
pub(crate) fn prescan(markdown: &str) -> usize {
    markdown
        .lines()
        .filter(|line| is_mermaid_fence_opener(line))
        .count()
}

fn is_mermaid_fence_opener(line: &str) -> bool {
    let rest = strip_quote_and_list_prefix(line);
    is_mermaid_fence(rest) || is_definition_list_mermaid_fence(rest)
}

fn is_mermaid_fence(rest: &str) -> bool {
    fence_info(rest)
        .is_some_and(|info| info.to_ascii_lowercase().contains("mermaid"))
}

/// Recognize definition-list lines whose definition begins with a Mermaid
/// fence. This is intentionally a cheap candidate scan; Pandoc collection
/// remains authoritative and filters out prose that only resembles a list.
fn is_definition_list_mermaid_fence(rest: &str) -> bool {
    if let Some(after_marker) = rest.strip_prefix('~')
        && after_marker.chars().next().is_some_and(char::is_whitespace)
        && is_mermaid_fence(after_marker.trim_start())
    {
        return true;
    }

    rest.char_indices().any(|(index, character)| {
        if character != ':' {
            return false;
        }
        let after_marker = &rest[index + character.len_utf8()..];
        after_marker.chars().next().is_some_and(char::is_whitespace)
            && is_mermaid_fence(after_marker.trim_start())
    })
}

fn strip_quote_and_list_prefix(line: &str) -> &str {
    let mut rest = line;
    loop {
        rest = rest.trim_start();
        if let Some(stripped) = rest.strip_prefix('>') {
            rest = stripped;
            continue;
        }
        if let Some(stripped) = strip_list_marker(rest) {
            rest = stripped;
            continue;
        }
        break;
    }
    rest
}

fn strip_list_marker(rest: &str) -> Option<&str> {
    let bytes = rest.as_bytes();
    if bytes.len() >= 2
        && matches!(bytes[0], b'-' | b'*' | b'+')
        && bytes[1].is_ascii_whitespace()
    {
        return Some(&rest[1..]);
    }
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let after = &rest[digits..];
    let mut chars = after.chars();
    match (chars.next(), chars.next()) {
        (Some(mark), Some(space))
            if matches!(mark, '.' | ')') && space.is_whitespace() =>
        {
            Some(&after[1..])
        }
        _ => None,
    }
}

fn fence_info(rest: &str) -> Option<&str> {
    let bytes = rest.as_bytes();
    let opener = *bytes.first()?;
    if opener != b'`' && opener != b'~' {
        return None;
    }
    let n = bytes.iter().take_while(|byte| **byte == opener).count();
    (n >= 3).then_some(&rest[n..])
}

/// Run pandoc with the collect filter and the route's reader flags.
/// Returns unique blocks in first-seen order, with occurrence counts.
pub(crate) fn collect(
    pandoc: &OsStr,
    source: &Path,
    reader: DiagramReader,
    workdir: &Path,
) -> Result<Vec<CollectedDiagram>> {
    fs::create_dir_all(workdir).map_err(|error| {
        CommandError::new(format!(
            "create diagrams workdir {}: {error}",
            workdir.display()
        ))
    })?;
    let filter_path = workdir.join("diagrams-collect.lua");
    fs::write(
        &filter_path,
        format!(
            "{}\n{}",
            super::code_catalog::TAB_EXPANSION_PRELUDE,
            COLLECT_FILTER
        ),
    )
    .map_err(|error| {
        CommandError::new(format!(
            "write collect filter {}: {error}",
            filter_path.display()
        ))
    })?;
    let manifest_path = workdir.join("manifest.json");
    let throwaway = workdir.join("collect.out");
    let mut cmd = Command::new(pandoc);
    cmd.arg(source).arg("--preserve-tabs");
    if let DiagramReader::Snapshot = reader {
        cmd.arg("-f")
            .arg(super::note_snapshot::profile::SNAPSHOT_PANDOC_READER);
    }
    let output = cmd
        .arg("-o")
        .arg(&throwaway)
        .arg("--lua-filter")
        .arg(&filter_path)
        .arg("--metadata")
        .arg(format!(
            "{}={}",
            MANIFEST_METADATA_KEY,
            manifest_path.display()
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            CommandError::new(format!(
                "run pandoc collect {}: {error}",
                Path::new(pandoc).display()
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = match (stderr.is_empty(), stdout.is_empty()) {
            (false, false) => format!("{stderr}\n{stdout}"),
            (false, true) => stderr,
            (true, false) => stdout,
            (true, true) => "pandoc produced no diagnostic output".to_string(),
        };
        return Err(CommandError::new(format!(
            "pandoc collect failed while reading {} (exit {}):\n{}",
            source.display(),
            output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            detail
        )));
    }
    let raw = fs::read_to_string(&manifest_path).map_err(|error| {
        CommandError::new(format!(
            "read diagrams manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    parse_manifest(&raw)
}

fn parse_manifest(raw: &str) -> Result<Vec<CollectedDiagram>> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| {
            CommandError::new(format!("parse diagrams manifest: {error}"))
        })?;
    let entries: Vec<ManifestEntry> = match value {
        serde_json::Value::Array(_) => {
            serde_json::from_value(value).map_err(|error| {
                CommandError::new(format!(
                    "parse diagrams manifest entries: {error}"
                ))
            })?
        }
        serde_json::Value::Object(map) if map.is_empty() => Vec::new(),
        other => {
            return Err(CommandError::new(format!(
                "diagrams manifest must be a JSON array, got {other}"
            )));
        }
    };
    let mut ordered: Vec<CollectedDiagram> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (position, entry) in entries.into_iter().enumerate() {
        if let Some(&i) = index.get(&entry.id) {
            ordered[i].occurrences += 1;
            continue;
        }
        index.insert(entry.id.clone(), ordered.len());
        ordered.push(CollectedDiagram {
            id: entry.id,
            text: entry.text,
            first_occurrence: position + 1,
            occurrences: 1,
        });
    }
    Ok(ordered)
}

/// Write `results.json` with absolute PDF paths per the shared contract.
pub(crate) fn write_results(
    path: &Path,
    results: &DiagramResults,
) -> Result<()> {
    let json = serde_json::to_string(results).map_err(|error| {
        CommandError::new(format!(
            "encode diagrams results {}: {error}",
            path.display()
        ))
    })?;
    fs::write(path, json).map_err(|error| {
        CommandError::new(format!(
            "write diagrams results {}: {error}",
            path.display()
        ))
    })
}

/// Orchestrate collect → adapter → `results.json`. Never returns an error.
pub(crate) fn prepare(
    pandoc: Option<&OsStr>,
    source: DiagramSource<'_>,
    route: DiagramRoute,
    scratch: &Path,
) -> DiagramPass {
    if !enabled() {
        return DiagramPass::skipped();
    }
    let markdown = match source_markdown(&source) {
        Ok(text) => text,
        Err(_) => return DiagramPass::skipped(),
    };
    if prescan(&markdown) == 0 {
        return DiagramPass::skipped();
    }
    let workdir = scratch.join("diagrams");
    if let Err(error) = fs::create_dir_all(&workdir) {
        return DiagramPass {
            filter: None,
            total: 0,
            rendered: 0,
            warnings: vec![unavailable_warning(
                &format!(
                    "create diagrams workdir {}: {error}",
                    workdir.display()
                ),
                None,
            )],
        };
    }
    let filter_path = workdir.join("diagrams.lua");
    if let Err(error) = fs::write(&filter_path, SWAP_FILTER) {
        return DiagramPass {
            filter: None,
            total: 0,
            rendered: 0,
            warnings: vec![unavailable_warning(
                &format!(
                    "write swap filter {}: {error}",
                    filter_path.display()
                ),
                None,
            )],
        };
    }
    let results_path = workdir.join("results.json");
    let collect_path = match collect_source_path(&source, &markdown, &workdir) {
        Ok(path) => path,
        Err(reason) => {
            return fallback_pass(&filter_path, &results_path, reason, None, 0);
        }
    };
    let Some(pandoc) = pandoc else {
        return fallback_pass(
            &filter_path,
            &results_path,
            "pandoc command not found; install pandoc or set BOB_PANDOC_COMMAND"
                .to_string(),
            None,
            0,
        );
    };
    let blocks = match collect(pandoc, &collect_path, route.reader, &workdir) {
        Ok(blocks) if blocks.is_empty() => return DiagramPass::skipped(),
        Ok(blocks) => blocks,
        Err(error) => {
            return fallback_pass(
                &filter_path,
                &results_path,
                error.message,
                None,
                0,
            );
        }
    };
    render_collected(blocks, route, &workdir, &filter_path, &results_path)
}

fn source_markdown(
    source: &DiagramSource<'_>,
) -> std::result::Result<String, String> {
    match source {
        DiagramSource::Path(path) => fs::read_to_string(path)
            .map_err(|error| format!("read {}: {error}", path.display())),
        DiagramSource::Text(text) => Ok((*text).to_string()),
    }
}

fn collect_source_path(
    source: &DiagramSource<'_>,
    markdown: &str,
    workdir: &Path,
) -> std::result::Result<PathBuf, String> {
    match source {
        DiagramSource::Path(path) => Ok((*path).to_path_buf()),
        DiagramSource::Text(_) => {
            let path = workdir.join("input.md");
            fs::write(&path, markdown).map_err(|error| {
                format!("write {}: {error}", path.display())
            })?;
            Ok(path)
        }
    }
}

/// Keep PDF fallback notes and doctor rows short without losing UTF-8 boundaries.
pub(super) fn compact_error(reason: &str) -> String {
    reason
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("render failed")
        .chars()
        .take(200)
        .collect()
}

fn fallback_pass(
    filter: &Path,
    results: &Path,
    reason: String,
    hint: Option<&str>,
    total: usize,
) -> DiagramPass {
    let _ = write_results(
        results,
        &DiagramResults {
            diagrams: BTreeMap::new(),
            fallback_error: Some(compact_error(&reason)),
        },
    );
    DiagramPass {
        filter: Some(DiagramFilter {
            filter: filter.to_path_buf(),
            results: results.to_path_buf(),
        }),
        total,
        rendered: 0,
        warnings: vec![unavailable_warning(&reason, hint)],
    }
}

fn fallback_from_adapter(
    filter: &Path,
    results: &Path,
    fail: AdapterFailure,
    total: usize,
) -> DiagramPass {
    fallback_pass(filter, results, fail.message, fail.hint.as_deref(), total)
}

fn render_collected(
    blocks: Vec<CollectedDiagram>,
    route: DiagramRoute,
    workdir: &Path,
    filter_path: &Path,
    results_path: &Path,
) -> DiagramPass {
    let total = total_occurrences(&blocks);
    let send = blocks.len().min(MAX_DIAGRAM_ITEMS);
    let workdir_abs = match workdir.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            return fallback_pass(
                filter_path,
                results_path,
                format!(
                    "resolve diagrams workdir {}: {error}",
                    workdir.display()
                ),
                None,
                total,
            );
        }
    };
    let client = match ClipAdapterClient::resolve_for_diagrams(send) {
        Ok(client) => client,
        Err(fail) => {
            return fallback_from_adapter(
                filter_path,
                results_path,
                fail,
                total,
            );
        }
    };
    let items = blocks
        .iter()
        .take(send)
        .map(|block| DiagramItem {
            id: block.id.clone(),
            source: block.text.clone(),
        })
        .collect();
    let request =
        DiagramsRequest::new(workdir_abs.to_string_lossy().into_owned(), items);
    let success = match client.diagrams(&request) {
        Ok(success) => success,
        Err(fail) => {
            return fallback_from_adapter(
                filter_path,
                results_path,
                fail,
                total,
            );
        }
    };
    map_adapter_results(
        blocks,
        success,
        route,
        filter_path,
        results_path,
        total,
    )
}

fn map_adapter_results(
    blocks: Vec<CollectedDiagram>,
    success: super::clip_adapter::DiagramsSuccess,
    route: DiagramRoute,
    filter_path: &Path,
    results_path: &Path,
    total: usize,
) -> DiagramPass {
    let mut by_id = BTreeMap::new();
    for item in success.results {
        by_id.insert(item.id.clone(), item);
    }
    let mut diagrams = BTreeMap::new();
    let mut rendered = 0;
    let mut fail_warnings = Vec::new();
    let mut scale_warnings = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if index >= MAX_DIAGRAM_ITEMS {
            diagrams.insert(
                block.id.clone(),
                DiagramOutcome::Error {
                    error: LIMIT_ERROR.to_string(),
                },
            );
            fail_warnings.push(not_rendered_warning(
                block.first_occurrence,
                total,
                LIMIT_ERROR,
            ));
            continue;
        }
        match by_id.get(&block.id) {
            Some(result) if result.ok => {
                let pdf = result
                    .pdf
                    .as_deref()
                    .filter(|path| Path::new(path).is_file());
                let Some(pdf) = pdf else {
                    let error = "rendered PDF missing".to_string();
                    diagrams.insert(
                        block.id.clone(),
                        DiagramOutcome::Error {
                            error: error.clone(),
                        },
                    );
                    fail_warnings.push(not_rendered_warning(
                        block.first_occurrence,
                        total,
                        &error,
                    ));
                    continue;
                };
                diagrams.insert(
                    block.id.clone(),
                    DiagramOutcome::Pdf {
                        pdf: pdf.to_string(),
                    },
                );
                rendered += block.occurrences;
                if let (Some(width_pt), Some(height_pt)) =
                    (result.width_pt, result.height_pt)
                    && let Some(scale) = effective_scale(
                        width_pt,
                        height_pt,
                        route.text_width_pt,
                        route.text_height_pt,
                    )
                    && scale < SCALE_WARN_BELOW
                {
                    scale_warnings.push(scale_warning(
                        block.first_occurrence,
                        total,
                        diagram_label(&block.text),
                        scale,
                    ));
                }
            }
            Some(result) => {
                let error = result
                    .error
                    .as_deref()
                    .filter(|text| !text.is_empty())
                    .unwrap_or("render failed")
                    .to_string();
                diagrams.insert(
                    block.id.clone(),
                    DiagramOutcome::Error {
                        error: error.clone(),
                    },
                );
                fail_warnings.push(not_rendered_warning(
                    block.first_occurrence,
                    total,
                    &error,
                ));
            }
            None => {
                let error = "no result from renderer";
                diagrams.insert(
                    block.id.clone(),
                    DiagramOutcome::Error {
                        error: error.to_string(),
                    },
                );
                fail_warnings.push(not_rendered_warning(
                    block.first_occurrence,
                    total,
                    error,
                ));
            }
        }
    }
    fail_warnings.extend(scale_warnings);
    let _ = write_results(
        results_path,
        &DiagramResults {
            diagrams,
            fallback_error: None,
        },
    );
    DiagramPass {
        filter: Some(DiagramFilter {
            filter: filter_path.to_path_buf(),
            results: results_path.to_path_buf(),
        }),
        total,
        rendered,
        warnings: fail_warnings,
    }
}

fn unavailable_warning(reason: &str, hint: Option<&str>) -> String {
    let mut line = format!("Mermaid diagrams printed as source: {reason}");
    if let Some(hint) = hint.filter(|hint| !hint.is_empty()) {
        line.push_str("; ");
        line.push_str(hint);
    }
    line
}

fn not_rendered_warning(first: usize, total: usize, error: &str) -> String {
    format!("Mermaid diagram {first} of {total} not rendered: {error}")
}

fn scale_warning(
    first: usize,
    total: usize,
    label: &str,
    scale: f64,
) -> String {
    let percent = (scale * 100.0).round() as i32;
    format!(
        "Mermaid diagram {first} of {total} ({label}) prints at {percent}% scale; labels may be small"
    )
}

fn diagram_label(source: &str) -> &str {
    source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("diagram")
}

fn total_occurrences(blocks: &[CollectedDiagram]) -> usize {
    blocks.iter().map(|block| block.occurrences).sum()
}

fn effective_scale(
    width_pt: f64,
    height_pt: f64,
    text_width_pt: f64,
    text_height_pt: f64,
) -> Option<f64> {
    if width_pt <= 0.0
        || height_pt <= 0.0
        || text_width_pt <= 0.0
        || text_height_pt <= 0.0
    {
        return None;
    }
    Some(
        1.0_f64
            .min(text_width_pt / width_pt)
            .min((text_height_pt * FIT_HEIGHT_BUDGET) / height_pt),
    )
}

/// One-page PDF xdvipdfmx can include. Hand-written 7-bit ASCII so tests
/// do not depend on lopdf's empty-stream output, which XeLaTeX rejects.
#[cfg(test)]
pub(crate) fn write_one_page_pdf(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create diagram PDF parent");
    }
    fs::write(path, include_bytes!("diagrams_blank.pdf"))
        .expect("write one-page diagram PDF");
}

/// Sized one-page PDF with the same object layout as `diagrams_blank.pdf`.
///
/// Hand-built 7-bit ASCII with a computed `xref` table and
/// `/MediaBox [0 0 w h]`. The content stream draws two Helvetica 10pt words:
/// `diagramtop` near the top edge and `diagrambottom` about 6pt above the
/// bottom edge, so `pdftotext` can locate both edges and measure the scale
/// on the rendered page.
#[cfg(test)]
pub(crate) fn write_sized_pdf(path: &Path, width_pt: f64, height_pt: f64) {
    fn fmt_pt(value: f64) -> String {
        if value.fract() == 0.0 {
            format!("{}", value as i64)
        } else {
            let mut text = format!("{value}");
            while text.ends_with('0') {
                text.pop();
            }
            if text.ends_with('.') {
                text.pop();
            }
            text
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create sized diagram PDF parent");
    }
    let width = fmt_pt(width_pt);
    let height = fmt_pt(height_pt);
    let top_y = fmt_pt(height_pt - 15.0);
    let stream = format!(
        "BT /F1 10 Tf 10 {top_y} Td (diagramtop) Tj ET\nBT /F1 10 Tf 10 10 Td (diagrambottom) Tj ET\n"
    );
    let objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
        ),
        format!("<< /Length {} >>\nstream\n{stream}endstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets: Vec<usize> = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(
            format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes(),
        );
    }
    let xref_at = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n", objects.len() + 1).as_bytes(),
    );
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    fs::write(path, out).expect("write sized diagram PDF");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, ffi::OsStr, process::Command};

    use crate::native::env::TestEnvGuard;
    use crate::native::highlights_ref::create::pandoc_command;

    const FLOW: &str = "flowchart LR\n  A --> B\n";
    const SEQUENCE: &str = "sequenceDiagram\n  A->>B: hi\n";

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "bob-cli-diagrams-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn skip_pandoc(test_name: &str) -> Option<std::ffi::OsString> {
        let Some(pandoc) = pandoc_command() else {
            eprintln!("skipping {test_name}: pandoc/xelatex is required");
            return None;
        };
        Some(pandoc)
    }

    fn write_executable(path: &Path, body: &str) {
        // Shared fork-safe scratch-then-copy writer (bead `bob-cli-5l`).
        crate::native::test_stub::write_executable_stub(path, body);
    }

    fn generic_route() -> DiagramRoute {
        DiagramRoute {
            reader: DiagramReader::Inferred,
            text_width_pt: 489.6,
            text_height_pt: 669.6,
        }
    }

    fn collect_source() -> String {
        format!(
            "# Diagrams\n\n\
             ```text\n\
             flowchart LR\n\
               skip --> me\n\
             ```\n\n\
             ```\n\
             flowchart LR\n\
               also --> skip\n\
             ```\n\n\
             ~~~ {{.mermaid}}\n{FLOW}~~~\n\n\
             - nested\n\n\
               ```mermaid\n{FLOW}```\n\n\
             ```mermaid\n{SEQUENCE}```\n"
        )
    }

    #[test]
    fn kill_switch_reads_off_values_through_native_env() {
        for value in ["off", "OFF", "0", "false", "No", " nO "] {
            let _guard =
                TestEnvGuard::set(&[(ENV_DIAGRAMS, Some(OsStr::new(value)))]);
            assert!(!enabled(), "{value} must disable");
        }
        let _guard =
            TestEnvGuard::set(&[(ENV_DIAGRAMS, Some(OsStr::new("on")))]);
        assert!(enabled());
        let _guard = TestEnvGuard::set(&[(ENV_DIAGRAMS, None)]);
        assert!(enabled(), "unset leaves the pass on");
    }

    #[test]
    fn prescan_counts_backtick_tilde_attribute_quoted_and_list_fences() {
        let markdown = "\
```mermaid\nflowchart LR\n```\n\
~~~mermaid\nflowchart TD\n~~~\n\
``` {.mermaid}\nflowchart TB\n```\n\
> ```mermaid\n> flowchart LR\n> ```\n\
- ```mermaid\n  flowchart LR\n  ```\n\
  1. ```mermaid\n     flowchart LR\n     ```\n\
    ```mermaid\n    flowchart LR\n    ```\n\
````markdown\n```mermaid\nflowchart LR\n```\n````\n\
```text\nflowchart LR\n```\n\
```\nflowchart LR\n```\n\
plain flowchart LR in a paragraph\n\
~~~\nuntagged tilde fence\n~~~\n";
        // 3 explicit + 1 quote + 1 list + 1 ordered + 1 indented + 1
        // nested-in-markdown + 3 definition-list openers = 11.
        // text/untagged/prose and ordinary tilde fences = 0.
        let markdown = format!(
            "{markdown}\
             Term\n\
             : ```mermaid\n\
               flowchart LR\n\
               ```\n\
             Term two\n\
             ~   ~~~mermaid\n\
             flowchart TD\n\
             ~~~\n\
             > - Nested term\n\
             >   : ~~~mermaid\n\
             >     flowchart TB\n\
             >     ~~~\n\
             ~~~text\n\
             not a definition fence\n"
        );
        assert_eq!(prescan(&markdown), 11);
        assert_eq!(prescan("# No diagrams\n\nHello.\n"), 0);
    }

    #[test]
    fn collect_finds_mermaid_fences_in_definition_lists() {
        let Some(pandoc) = skip_pandoc("definition-list collect") else {
            return;
        };
        let temp = TempDir::new("definition-list-collect");
        let source = temp.path.join("definitions.md");
        fs::write(
            &source,
            "# Definition diagrams\n\n\
             Term\n\
             : ```mermaid\n\
               flowchart LR\n\
                 A --> B\n\
               ```\n",
        )
        .expect("write definition-list source");

        let blocks = collect(
            &pandoc,
            &source,
            DiagramReader::Inferred,
            &temp.path.join("collect"),
        )
        .expect("collect definition-list Mermaid fence");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert_eq!(blocks[0].first_occurrence, 1);
        assert!(blocks[0].text.contains("flowchart LR"), "{blocks:?}");
    }

    #[test]
    fn collect_finds_attribute_and_nested_fences_on_both_readers() {
        let Some(pandoc) = skip_pandoc("collect") else {
            return;
        };
        let temp = TempDir::new("collect");
        let source = temp.path.join("doc.md");
        fs::write(&source, collect_source()).expect("write source");
        for reader in [DiagramReader::Inferred, DiagramReader::Snapshot] {
            let workdir = temp.path.join(format!("{reader:?}"));
            let blocks =
                collect(&pandoc, &source, reader, &workdir).expect("collect");
            assert_eq!(
                blocks.len(),
                2,
                "{reader:?}: unique sources, untagged ignored: {blocks:?}"
            );
            let flow = blocks
                .iter()
                .find(|block| block.text.contains("flowchart LR"))
                .expect("flowchart");
            assert_eq!(flow.occurrences, 2, "{reader:?} de-duplicates");
            assert_eq!(flow.id.len(), 40, "sha1 hex: {}", flow.id);
            assert!(
                flow.id.chars().all(|c| c.is_ascii_hexdigit()),
                "{}",
                flow.id
            );
            let seq = blocks
                .iter()
                .find(|block| block.text.contains("sequenceDiagram"))
                .expect("sequence");
            assert_eq!(seq.occurrences, 1);
            assert_ne!(flow.id, seq.id);
        }
    }

    #[test]
    fn prepare_skips_adapter_when_kill_switch_or_no_candidates() {
        let temp = TempDir::new("prepare-skip");
        let adapter = temp.path.join("adapter.sh");
        write_executable(
            &adapter,
            "#!/bin/sh\necho called > \"$(dirname \"$0\")/called\"\nexit 99\n",
        );
        let called = temp.path.join("called");
        let source = temp.path.join("plain.md");
        fs::write(&source, "# Hi\n\nNo mermaid.\n").expect("write plain");
        let _guard = TestEnvGuard::set(&[(
            super::super::clip_adapter::ENV_ADAPTER_OVERRIDE,
            Some(adapter.as_os_str()),
        )]);
        let pass = prepare(
            None,
            DiagramSource::Path(&source),
            generic_route(),
            &temp.path,
        );
        assert!(pass.filter.is_none());
        assert!(!called.exists(), "no mermaid must not spawn");

        let mermaid = temp.path.join("m.md");
        fs::write(&mermaid, "```mermaid\nflowchart LR\n  A --> B\n```\n")
            .expect("write mermaid");
        let _guard = TestEnvGuard::set(&[
            (ENV_DIAGRAMS, Some(OsStr::new("off"))),
            (
                super::super::clip_adapter::ENV_ADAPTER_OVERRIDE,
                Some(adapter.as_os_str()),
            ),
        ]);
        let pass = prepare(
            None,
            DiagramSource::Path(&mermaid),
            generic_route(),
            &temp.path,
        );
        assert!(pass.filter.is_none());
        assert!(!called.exists(), "kill switch must not spawn");
    }

    #[test]
    fn prepare_adapter_crash_writes_fallback_and_keeps_filter() {
        let Some(pandoc) = skip_pandoc("prepare crash") else {
            return;
        };
        let temp = TempDir::new("prepare-crash");
        let adapter = temp.path.join("adapter.sh");
        write_executable(&adapter, "#!/bin/sh\necho boom >&2\nexit 3\n");
        let source = temp.path.join("doc.md");
        fs::write(&source, "```mermaid\nflowchart LR\n  A --> B\n```\n")
            .expect("write mermaid");
        let _guard = TestEnvGuard::set(&[(
            super::super::clip_adapter::ENV_ADAPTER_OVERRIDE,
            Some(adapter.as_os_str()),
        )]);
        let pass = prepare(
            Some(pandoc.as_os_str()),
            DiagramSource::Path(&source),
            generic_route(),
            &temp.path,
        );
        assert!(pass.filter.is_some());
        assert!(
            pass.warnings
                .iter()
                .any(|warning| warning.contains("crashed")),
            "{:?}",
            pass.warnings
        );
        let results =
            fs::read_to_string(temp.path.join("diagrams/results.json"))
                .expect("results");
        assert!(results.contains("fallback_error"), "{results}");
        assert!(results.contains("web clip adapter crashed"), "{results}");
    }

    #[test]
    fn fallback_note_is_compact_but_warning_keeps_full_adapter_error() {
        let temp = TempDir::new("compact-fallback");
        let results = temp.path.join("results.json");
        let reason = format!(
            "\n  Browser launch failed: {}  \n╔══ Install browsers ══╗\n",
            "é".repeat(220)
        );
        let pass = fallback_from_adapter(
            &temp.path.join("filter.lua"),
            &results,
            AdapterFailure {
                message: reason.clone(),
                hint: Some("playwright install".into()),
            },
            1,
        );
        let parsed: DiagramResults = serde_json::from_str(
            &fs::read_to_string(results).expect("read fallback results"),
        )
        .expect("parse fallback results");
        let note = parsed.fallback_error.expect("fallback note");
        assert_eq!(note.chars().count(), 200);
        assert_eq!(note.lines().count(), 1);
        assert!(note.starts_with("Browser launch failed:"));
        assert!(pass.warnings[0].contains(&reason));
        assert!(pass.warnings[0].contains("playwright install"));
        assert_eq!(compact_error("\n  short reason  \nmore"), "short reason");
    }

    #[test]
    fn header_and_rust_height_budgets_agree() {
        assert!(
            HEADER_INCLUDES.contains(&format!(
                "\\newcommand{{\\BobFitHeightBudget}}{{{FIT_HEIGHT_BUDGET}}}"
            )),
            "diagrams.tex budget must match FIT_HEIGHT_BUDGET: {HEADER_INCLUDES}"
        );
    }

    #[test]
    fn scale_helpers_match_the_a8_contract() {
        assert_eq!(effective_scale(200.0, 100.0, 489.6, 669.6), Some(1.0));
        let scale =
            effective_scale(1200.0, 100.0, 489.6, 669.6).expect("wide diagram");
        assert!((scale - 489.6 / 1200.0).abs() < 1e-9);
        assert!(scale < 0.5);
        let tall =
            effective_scale(514.5, 714.75, 489.6, 669.6).expect("tall diagram");
        assert!(
            (tall - FIT_HEIGHT_BUDGET * 669.6 / 714.75).abs() < 1e-9,
            "tall diagram uses the budgeted height: {tall}"
        );
        assert_eq!(
            scale_warning(1, 12, "flowchart LR", 0.46),
            "Mermaid diagram 1 of 12 (flowchart LR) prints at 46% scale; labels may be small"
        );
        assert_eq!(
            diagram_label("\nflowchart LR\n  A --> B\n"),
            "flowchart LR"
        );
        assert_eq!(
            not_rendered_warning(4, 12, "Parse error on line 4: …"),
            "Mermaid diagram 4 of 12 not rendered: Parse error on line 4: …"
        );
    }

    #[test]
    fn parse_manifest_keeps_true_first_positions_across_interleaved_duplicates()
    {
        let manifest = |ids: &[&str]| {
            serde_json::to_string(
                &ids.iter()
                    .map(|id| serde_json::json!({ "id": id, "text": id }))
                    .collect::<Vec<_>>(),
            )
            .expect("serialize manifest")
        };

        let short = parse_manifest(&manifest(&["A", "B", "A"]))
            .expect("parse A/B/A manifest");
        assert_eq!(short.len(), 2);
        assert_eq!(short[0].first_occurrence, 1);
        assert_eq!(short[0].occurrences, 2);
        assert_eq!(short[1].first_occurrence, 2);
        assert_eq!(short[1].occurrences, 1);
        assert_eq!(total_occurrences(&short), 3);
        assert_eq!(
            not_rendered_warning(
                short[1].first_occurrence,
                total_occurrences(&short),
                "Parse error"
            ),
            "Mermaid diagram 2 of 3 not rendered: Parse error"
        );

        let longer = parse_manifest(&manifest(&["A", "B", "A", "C", "B", "A"]))
            .expect("parse interleaved manifest");
        assert_eq!(
            longer
                .iter()
                .map(|block| (
                    block.id.as_str(),
                    block.first_occurrence,
                    block.occurrences
                ))
                .collect::<Vec<_>>(),
            [("A", 1, 3), ("B", 2, 2), ("C", 4, 1)]
        );
        assert_eq!(total_occurrences(&longer), 6);
    }

    #[test]
    fn write_results_emits_absolute_pdf_or_error_and_null_fallback() {
        let temp = TempDir::new("results");
        let pdf = temp.path.join("abcd.pdf");
        write_one_page_pdf(&pdf);
        let path = temp.path.join("results.json");
        let mut diagrams = BTreeMap::new();
        diagrams.insert(
            "a".repeat(40),
            DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        diagrams.insert(
            "b".repeat(40),
            DiagramOutcome::Error {
                error: "Parse error on line 4".to_string(),
            },
        );
        write_results(
            &path,
            &DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write");
        let raw = fs::read_to_string(&path).expect("read");
        let parsed: DiagramResults = serde_json::from_str(&raw).expect("parse");
        assert!(parsed.fallback_error.is_none());
        match parsed.diagrams.get(&"a".repeat(40)) {
            Some(DiagramOutcome::Pdf { pdf: written }) => {
                assert!(Path::new(written).is_absolute(), "{written}");
                assert_eq!(written, &pdf.display().to_string());
            }
            other => panic!("pdf entry: {other:?}"),
        }
        match parsed.diagrams.get(&"b".repeat(40)) {
            Some(DiagramOutcome::Error { error }) => {
                assert!(error.contains("Parse error"));
            }
            other => panic!("error entry: {other:?}"),
        }
        assert!(raw.contains("\"fallback_error\":null"), "{raw}");
    }

    fn swap_native(source: &str, results: &DiagramResults) -> Option<String> {
        let pandoc = skip_pandoc("swap native")?;
        let temp = TempDir::new("swap-native");
        let md = temp.path.join("doc.md");
        fs::write(&md, source).expect("write source");
        let filter = temp.path.join("diagrams.lua");
        fs::write(&filter, SWAP_FILTER).expect("write swap filter");
        let results_path = temp.path.join("results.json");
        write_results(&results_path, results).expect("write results");
        let output = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("native")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                RESULTS_METADATA_KEY,
                results_path.display()
            ))
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    #[test]
    fn swap_preserves_identifier_and_drops_source_in_native() {
        let temp = TempDir::new("id-pdf");
        let pdf = temp.path.join("fig.pdf");
        write_one_page_pdf(&pdf);
        let Some(pandoc) = skip_pandoc("swap native collect") else {
            return;
        };
        let md = temp.path.join("doc.md");
        fs::write(&md, format!("``` {{#fig-flow .mermaid}}\n{FLOW}```\n"))
            .expect("write source");
        let workdir = temp.path.join("collect");
        let blocks = collect(&pandoc, &md, DiagramReader::Inferred, &workdir)
            .expect("collect");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let mut diagrams = BTreeMap::new();
        diagrams.insert(
            blocks[0].id.clone(),
            DiagramOutcome::Pdf {
                pdf: pdf.display().to_string(),
            },
        );
        let Some(native) = swap_native(
            &fs::read_to_string(&md).expect("read"),
            &DiagramResults {
                diagrams,
                fallback_error: None,
            },
        ) else {
            return;
        };
        assert!(
            native.contains("fig-flow"),
            "identifier preserved: {native}"
        );
        assert!(native.contains("Image"), "swapped to Image: {native}");
        assert!(!native.contains("flowchart LR"), "source gone: {native}");
    }

    #[test]
    fn swap_error_emits_italic_note_then_source() {
        let Some(pandoc) = skip_pandoc("swap error") else {
            return;
        };
        let temp = TempDir::new("swap-error");
        let md = temp.path.join("doc.md");
        fs::write(&md, format!("```mermaid\n{FLOW}```\n"))
            .expect("write source");
        let workdir = temp.path.join("collect");
        let blocks = collect(&pandoc, &md, DiagramReader::Inferred, &workdir)
            .expect("collect");
        let mut diagrams = BTreeMap::new();
        diagrams.insert(
            blocks[0].id.clone(),
            DiagramOutcome::Error {
                error: "Parse error on line 4: bad".to_string(),
            },
        );
        let filter = temp.path.join("diagrams.lua");
        fs::write(&filter, SWAP_FILTER).expect("write filter");
        let results_path = temp.path.join("results.json");
        write_results(
            &results_path,
            &DiagramResults {
                diagrams,
                fallback_error: None,
            },
        )
        .expect("write results");
        let output = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("latex")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                RESULTS_METADATA_KEY,
                results_path.display()
            ))
            .output()
            .expect("run pandoc");
        assert!(output.status.success(), "{output:?}");
        let latex = String::from_utf8_lossy(&output.stdout);
        let note_at = latex
            .find("Mermaid diagram not rendered: Parse error on line 4")
            .unwrap_or_else(|| panic!("note: {latex}"));
        let source_at = latex
            .find("flowchart LR")
            .unwrap_or_else(|| panic!("source: {latex}"));
        assert!(note_at < source_at, "note before source: {latex}");
        assert!(
            latex.contains("\\emph{") || latex.contains("\\textit{"),
            "italic note: {latex}"
        );
    }

    #[test]
    fn absent_or_corrupt_results_leave_latex_unchanged() {
        let Some(pandoc) = skip_pandoc("swap corrupt") else {
            return;
        };
        let temp = TempDir::new("swap-corrupt");
        let md = temp.path.join("doc.md");
        let source = format!("```mermaid\n{FLOW}```\n");
        fs::write(&md, &source).expect("write source");
        let filter = temp.path.join("diagrams.lua");
        fs::write(&filter, SWAP_FILTER).expect("write filter");

        let baseline = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("latex")
            .output()
            .expect("baseline");
        assert!(baseline.status.success(), "{baseline:?}");
        let baseline_tex = String::from_utf8_lossy(&baseline.stdout);

        let missing = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("latex")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                RESULTS_METADATA_KEY,
                temp.path.join("missing.json").display()
            ))
            .output()
            .expect("missing results");
        assert!(missing.status.success(), "{missing:?}");
        assert_eq!(String::from_utf8_lossy(&missing.stdout), baseline_tex);

        let corrupt_path = temp.path.join("results.json");
        fs::write(&corrupt_path, "{not json").expect("corrupt");
        let corrupt = Command::new(&pandoc)
            .arg(&md)
            .arg("-t")
            .arg("latex")
            .arg("--lua-filter")
            .arg(&filter)
            .arg("--metadata")
            .arg(format!(
                "{}={}",
                RESULTS_METADATA_KEY,
                corrupt_path.display()
            ))
            .output()
            .expect("corrupt results");
        assert!(corrupt.status.success(), "{corrupt:?}");
        assert_eq!(String::from_utf8_lossy(&corrupt.stdout), baseline_tex);
    }
}
