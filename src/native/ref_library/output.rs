//! Shared output plumbing for the `bob ref` library verbs.
//!
//! One [`REF_SCHEMA_VERSION`], one JSON envelope, one human/machine error
//! shape, the reading-state chips, and Markdown table escaping live here so
//! `find`, `list`, and `show` render alike. Stdout in JSON mode is pure
//! JSON; human errors go to stderr as `bob ref: error: <message>` with an
//! optional `hint:` line.
use serde::Serialize;

use super::find::{FindResult, FindSummary, Verdict};
use super::{
    row_date, Coverage, LibraryCounts, ListSelection, RefRow, LIST_STATE_ORDER,
};
use crate::native::env as bob_env;
use crate::native::highlights_ref::COMMAND_NAME;
use crate::native::style::{display_width, pad_right, truncate, Styler};

/// Versioned envelope marker shared by `find`, `list`, and `show`. Bump
/// only for a breaking change; new optional fields keep the version.
pub(crate) const REF_SCHEMA_VERSION: u32 = 1;

/// Output formats for the library verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Human,
    Json,
    Markdown,
}

impl Format {
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "json" => Self::Json,
            "markdown" => Self::Markdown,
            _ => Self::Human,
        }
    }
}

/// Local-time stamp for `generated_at`, pinned in tests with `BOB_NOW`.
pub(crate) fn generated_at() -> String {
    bob_env::current_datetime()
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

/// The `ref find` success envelope.
#[derive(Debug, Clone, Serialize)]
struct FindEnvelope<'a> {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    coverage: &'a Coverage,
    library: &'a LibraryCounts,
    summary: &'a FindSummary,
    results: &'a [FindResult],
}

/// Print the `ref find` envelope as compact one-line JSON.
pub(crate) fn print_find_json(
    coverage: &Coverage,
    library: &LibraryCounts,
    summary: &FindSummary,
    results: &[FindResult],
) {
    let envelope = FindEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref find",
        generated_at: generated_at(),
        coverage,
        library,
        summary,
        results,
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("find envelope serializes")
    );
}

/// One disambiguation candidate for a `ref show` error: the note a `REF`
/// almost named, so agents can retry without another lookup.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ErrorCandidate {
    pub path: String,
    pub title: String,
}

impl ErrorCandidate {
    pub(crate) fn new(path: &str, title: &str) -> Self {
        Self {
            path: path.to_string(),
            title: title.to_string(),
        }
    }
}

/// The JSON error envelope shared by the library verbs.
#[derive(Debug, Clone, Serialize)]
struct ErrorEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    error: ErrorBody,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates: Option<Vec<ErrorCandidate>>,
}

/// The error body inside [`ErrorEnvelope`].
#[derive(Debug, Clone, Serialize)]
struct ErrorBody {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

/// Report a failure in the requested format: human errors go to stderr,
/// JSON errors go to stdout as the shared error envelope.
pub(crate) fn print_find_error(
    format: Format,
    command: &'static str,
    code: &str,
    message: &str,
    hint: Option<&str>,
) {
    if format == Format::Json {
        let envelope = ErrorEnvelope {
            ok: false,
            schema_version: REF_SCHEMA_VERSION,
            command,
            error: ErrorBody {
                code: code.to_string(),
                message: message.to_string(),
                hint: hint.map(str::to_string),
            },
            candidates: None,
        };
        println!(
            "{}",
            serde_json::to_string(&envelope)
                .expect("error envelope serializes")
        );
        return;
    }
    eprintln!("{COMMAND_NAME}: error: {message}");
    if let Some(hint) = hint {
        eprintln!("hint: {hint}");
    }
}

/// Report a `ref show` resolution failure: human errors go to stderr with
/// one `hint:` line per candidate, JSON errors go to stdout with the
/// shared envelope plus its `candidates` array.
pub(crate) fn print_show_error(
    format: Format,
    code: &str,
    message: &str,
    candidates: &[ErrorCandidate],
) {
    if format == Format::Json {
        let envelope = ErrorEnvelope {
            ok: false,
            schema_version: REF_SCHEMA_VERSION,
            command: "ref show",
            error: ErrorBody {
                code: code.to_string(),
                message: message.to_string(),
                hint: None,
            },
            candidates: (!candidates.is_empty()).then(|| candidates.to_vec()),
        };
        println!(
            "{}",
            serde_json::to_string(&envelope)
                .expect("show error envelope serializes")
        );
        return;
    }
    eprintln!("{COMMAND_NAME}: error: {message}");
    for candidate in candidates {
        eprintln!("hint: {} — {}", candidate.path, candidate.title);
    }
}

/// Escape one Markdown table cell: backslashes, pipes, and newlines.
pub(crate) fn escape_cell(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', " ")
        .replace('\r', "")
}

/// A reading-state chip: the glyph plus word always render, and color
/// only reinforces them.
pub(crate) fn state_chip(styler: Styler, reading_state: &str) -> String {
    match reading_state {
        "finished" => styler.green("✓ FINISHED"),
        "started" => styler.yellow("▸ STARTED"),
        "queued" => styler.cyan("○ QUEUED"),
        "dropped" => styler.dim("✗ DROPPED"),
        _ => styler.dim("? UNKNOWN"),
    }
}

/// The verdict chip for a query with no exact library match.
fn verdict_chip(styler: Styler, verdict: Verdict) -> String {
    match verdict {
        Verdict::InLibrary => state_chip(styler, "unknown"),
        Verdict::InIntake => styler.cyan("⇣ IN INTAKE"),
        Verdict::Possible => styler.yellow("≈ POSSIBLE"),
        Verdict::NotFound => styler.dim("· NOT FOUND"),
    }
}

/// Width of the chip column: every chip pads to the widest label.
fn chip_width() -> usize {
    [
        "✓ FINISHED",
        "▸ STARTED",
        "○ QUEUED",
        "✗ DROPPED",
        "? UNKNOWN",
        "≈ POSSIBLE",
        "· NOT FOUND",
        "⇣ IN INTAKE",
    ]
    .iter()
    .map(|chip| display_width(chip))
    .max()
    .unwrap_or(0)
}

/// The dated suffix for a primary match, when a date is known.
pub(crate) fn dated_suffix(row: &super::RefRow) -> Option<String> {
    match row.reading_state.as_str() {
        "finished" => row.finished.clone().map(|date| format!("read {date}")),
        "dropped" => row.finished.clone().map(|date| format!("dropped {date}")),
        "queued" => {
            row.added.clone().map(|date| format!("queued since {date}"))
        }
        "started" => row.added.clone().map(|date| format!("started {date}")),
        _ => None,
    }
}

/// Render one `bob ref find` result block in human form.
pub(crate) fn render_find_human(
    coverage: &Coverage,
    summary: &FindSummary,
    results: &[FindResult],
    intake_word: &str,
    styler: Styler,
    width: usize,
) -> String {
    let noun = if summary.queries == 1 {
        "query"
    } else {
        "queries"
    };
    let mut out = format!(
        "bob ref find · {} {noun} · {}/ ({} notes)\n",
        summary.queries, coverage.ref_dir, coverage.notes,
    );
    let pad = chip_width();
    let indent = " ".repeat(2 + pad + 3);
    let mut pending_sync = false;
    for result in results {
        out.push('\n');
        let chip = match result.verdict {
            Verdict::InLibrary => state_chip(
                styler,
                result.reading_state.as_deref().unwrap_or("unknown"),
            ),
            verdict => verdict_chip(styler, verdict),
        };
        // Pad on the plain label so ANSI styling never shifts columns.
        let plain_len = display_width(&strip_ansi(&chip));
        let padding = " ".repeat(pad.saturating_sub(plain_len));
        out.push_str(&format!(
            "  {chip}{padding}   {}\n",
            truncate(&result.query, width.saturating_sub(2 + pad + 3)),
        ));
        match result.verdict {
            Verdict::InLibrary => {
                let primary = &result.matches[0];
                out.push_str(&indent);
                out.push_str(&truncate(
                    &primary.row.title,
                    width.saturating_sub(indent.len()),
                ));
                out.push('\n');
                out.push_str(&indent);
                out.push_str(&detail_line(
                    &primary.row,
                    &mut pending_sync,
                    styler,
                    width.saturating_sub(indent.len()),
                ));
                out.push('\n');
                for secondary in result.matches.iter().skip(1) {
                    out.push_str(&indent);
                    out.push_str(&styler.dim(&truncate(
                        &also_line(&secondary.row),
                        width.saturating_sub(indent.len()),
                    )));
                    out.push('\n');
                }
            }
            Verdict::InIntake => {
                let title = result.intake.first().map_or_else(
                    || result.query.clone(),
                    |hit| {
                        hit.title
                            .clone()
                            .unwrap_or_else(|| hit.source_url.clone())
                    },
                );
                out.push_str(&indent);
                out.push_str(&truncate(
                    &title,
                    width.saturating_sub(indent.len()),
                ));
                out.push('\n');
                for hit in &result.intake {
                    out.push_str(&indent);
                    let mut line = hit.path.clone();
                    if let Some(status) = hit.status.as_deref() {
                        line.push_str(&format!(" · {}", status.to_uppercase()));
                    }
                    out.push_str(&styler.dim(&truncate(
                        &line,
                        width.saturating_sub(indent.len()),
                    )));
                    out.push('\n');
                }
            }
            Verdict::Possible => {
                let best = &result.candidates[0];
                out.push_str(&indent);
                out.push_str(&truncate(
                    &possible_line(result.candidates.len(), best),
                    width.saturating_sub(indent.len()),
                ));
                out.push('\n');
            }
            Verdict::NotFound => {}
        }
    }
    out.push('\n');
    out.push_str(&truncate(
        &format!("  {}", footer_text(summary, results, intake_word)),
        width,
    ));
    out.push('\n');
    if pending_sync {
        out.push_str(&styler.dim(
            "  * status changed since the last scan; run `bob ref scan` to reconcile",
        ));
        out.push('\n');
    }
    out
}

/// The detail line under a primary match: path, status, identity, date.
/// The path is dimmed; overlong lines truncate, keeping the path head.
fn detail_line(
    row: &super::RefRow,
    pending_sync: &mut bool,
    styler: Styler,
    max: usize,
) -> String {
    let pending = row.status_sync != "ok";
    if pending {
        *pending_sync = true;
    }
    let mut status = row.status.as_deref().unwrap_or("—").to_uppercase();
    if pending {
        status.push('*');
    }
    let mut rest = format!(" · {status}");
    if let Some(arxiv) = row.identity.arxiv.as_deref() {
        rest.push_str(&format!(" · arXiv {arxiv}"));
    } else if let Some(doi) = row.identity.doi.as_deref() {
        rest.push_str(&format!(" · DOI {doi}"));
    }
    if let Some(dated) = dated_suffix(row) {
        rest.push_str(&format!(" · {dated}"));
    }
    let budget = max.saturating_sub(display_width(&rest)).max(8);
    let path = truncate(&row.path, budget.max(8));
    let plain = format!("{path}{rest}");
    if display_width(&plain) > max {
        return truncate(&plain, max);
    }
    format!("{}{}", styler.dim(&path), rest)
}

/// A dim companion line for a non-primary exact match.
fn also_line(row: &super::RefRow) -> String {
    let mut line = format!("also {} ({}, ", row.path, row.era);
    if row.superseded_by.is_some() {
        line.push_str("superseded)");
    } else {
        line.push_str(row.status.as_deref().unwrap_or("—"));
        line.push(')');
    }
    line
}

/// The one-line summary of the title candidates for a query.
fn possible_line(count: usize, best: &super::find::FindCandidate) -> String {
    let noun = if count == 1 {
        "candidate"
    } else {
        "candidates"
    };
    format!(
        "{count} {noun} · best {} “{}” · {}",
        best.score, best.row.title, best.row.path,
    )
}

/// The closing summary line: library counts, verdict counts, intake word.
fn footer_text(
    summary: &FindSummary,
    results: &[FindResult],
    intake_word: &str,
) -> String {
    let mut parts = vec![format!(
        "{} of {} in library{}",
        summary.in_library,
        summary.queries,
        library_breakdown(results),
    )];
    if summary.possible > 0 {
        parts.push(format!("{} possible", summary.possible));
    }
    if summary.not_found > 0 {
        parts.push(format!("{} not found", summary.not_found));
    }
    if summary.in_intake > 0 {
        parts.push(format!("{} in intake", summary.in_intake));
    }
    parts.push(intake_word.to_string());
    parts.join(" · ")
}

/// Parenthesized reading-state split over the in-library queries, in
/// most-advanced order; empty when nothing is in the library.
fn library_breakdown(results: &[FindResult]) -> String {
    let mut counts = [0usize; 5];
    let mut total = 0usize;
    for result in results {
        if result.verdict != Verdict::InLibrary {
            continue;
        }
        total += 1;
        match result.reading_state.as_deref() {
            Some("finished") => counts[0] += 1,
            Some("started") => counts[1] += 1,
            Some("queued") => counts[2] += 1,
            Some("dropped") => counts[3] += 1,
            _ => counts[4] += 1,
        }
    }
    if total == 0 {
        return String::new();
    }
    let names = ["finished", "started", "queued", "dropped", "unknown"];
    let parts = counts
        .iter()
        .zip(names)
        .filter(|(count, _)| **count > 0)
        .map(|(count, name)| format!("{count} {name}"))
        .collect::<Vec<_>>();
    format!(" ({})", parts.join(" · "))
}

/// Render `bob ref find` results as a Markdown table plus coverage line.
pub(crate) fn render_find_markdown(
    summary: &FindSummary,
    results: &[FindResult],
) -> String {
    let mut out =
        String::from("| Query | Verdict | Reading state | Reference |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for result in results {
        let verdict = match result.verdict {
            Verdict::InLibrary => "In library",
            Verdict::InIntake => "In intake",
            Verdict::Possible => "Possible",
            Verdict::NotFound => "Not found",
        };
        let state = result.reading_state.as_deref().unwrap_or("—");
        let reference = match result.verdict {
            Verdict::InLibrary => {
                let row = &result.matches[0].row;
                format!("{} {}", row.link, row.title)
            }
            Verdict::InIntake => result
                .intake
                .first()
                .map(|hit| {
                    hit.title.clone().unwrap_or_else(|| hit.source_url.clone())
                })
                .unwrap_or_default(),
            Verdict::Possible => {
                let best = &result.candidates[0];
                format!(
                    "{} candidates, best {} “{}” {}",
                    result.candidates.len(),
                    best.score,
                    best.row.title,
                    best.row.link,
                )
            }
            Verdict::NotFound => String::new(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            escape_cell(&result.query),
            verdict,
            escape_cell(state),
            escape_cell(&reference),
        ));
    }
    out.push_str(&format!(
        "Library check: {} of {} in library ({} finished) · coverage: ref/ only\n",
        summary.in_library, summary.queries, summary.finished,
    ));
    out
}

/// The `ref list` filters view inside the JSON envelope: the effective
/// reading states plus every active filter. `limit` is `null` with
/// `-A/--all`.
#[derive(Debug, Clone, Serialize)]
struct ListFiltersView<'a> {
    reading_state: &'a [String],
    reading_state_defaulted: bool,
    status: Option<&'a [String]>,
    ref_type: Option<&'a [String]>,
    origin: Option<&'a str>,
    parent: Option<&'a str>,
    since: Option<&'a str>,
    limit: Option<u64>,
    undated_excluded: usize,
}

/// Superseded notes excluded from a `ref list` view.
#[derive(Debug, Clone, Serialize)]
struct ListHidden {
    superseded: usize,
}

/// The `ref list` success envelope.
#[derive(Debug, Clone, Serialize)]
struct ListEnvelope<'a> {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    coverage: &'a Coverage,
    filters: ListFiltersView<'a>,
    matched: usize,
    returned: usize,
    truncated: bool,
    hidden: ListHidden,
    library: &'a LibraryCounts,
    refs: &'a [RefRow],
}

/// Print the `ref list` envelope as compact one-line JSON.
#[allow(clippy::too_many_arguments)]
pub(crate) fn print_list_json(
    coverage: &Coverage,
    selection: &ListSelection,
    matched: usize,
    returned: usize,
    truncated: bool,
    hidden_superseded: usize,
    undated_excluded: usize,
    library: &LibraryCounts,
    refs: &[RefRow],
) {
    let envelope = ListEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref list",
        generated_at: generated_at(),
        coverage,
        filters: ListFiltersView {
            reading_state: &selection.states,
            reading_state_defaulted: selection.defaulted,
            status: selection.statuses.as_deref(),
            ref_type: selection.ref_types.as_deref(),
            origin: selection.origin.as_deref(),
            parent: selection.parent.as_deref(),
            since: selection.since.as_deref(),
            limit: selection.limit,
            undated_excluded,
        },
        matched,
        returned,
        truncated,
        hidden: ListHidden {
            superseded: hidden_superseded,
        },
        library,
        refs,
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("list envelope serializes")
    );
}

/// One collapsed legacy-era count for the human summary line.
#[derive(Debug, Clone)]
pub(crate) struct CollapsedLegacy {
    pub state: &'static str,
    pub count: usize,
}

/// Render `bob ref list` in human form: a header, one group per reading
/// state with its count, aligned rows, then the legacy-collapse,
/// truncation, and library lines. Without `-s`, legacy-era rows collapse
/// to one dim summary line instead of listing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_list_human(
    selection: &ListSelection,
    coverage: &Coverage,
    library: &LibraryCounts,
    listed_total: usize,
    returned: &[RefRow],
    group_totals: &[(&str, usize)],
    truncated_more: usize,
    collapsed: &[CollapsedLegacy],
    styler: Styler,
    width: usize,
) -> String {
    let mut out = String::new();
    out.push_str(&truncate(
        &list_header(selection, library, listed_total),
        width,
    ));
    out.push('\n');
    if returned.is_empty() && collapsed.is_empty() {
        out.push('\n');
        if selection.defaulted {
            out.push_str("  Nothing queued ✓\n");
        } else {
            out.push_str("  No matching notes.\n");
        }
        out.push_str(&truncate(&library_line(library, coverage), width));
        out.push('\n');
        return out;
    }
    let chip_width = returned
        .iter()
        .map(|row| display_width(&list_status_chip(row)))
        .max()
        .unwrap_or(0);
    let title_width = returned
        .iter()
        .map(|row| display_width(&row.title))
        .max()
        .unwrap_or(0);
    let mut pending_sync = false;
    for state in LIST_STATE_ORDER {
        let group: Vec<&RefRow> = returned
            .iter()
            .filter(|row| row.reading_state == state)
            .collect();
        if group.is_empty() {
            continue;
        }
        // The heading counts every listed row in the state, not just the
        // capped rows shown below it.
        let total = group_totals
            .iter()
            .find(|(name, _)| *name == state)
            .map(|(_, count)| *count)
            .unwrap_or(group.len());
        out.push('\n');
        out.push_str(&format!(
            "  {}  {}\n",
            group_heading(styler, state),
            total,
        ));
        for row in group {
            if row.status_sync != "ok" {
                pending_sync = true;
            }
            out.push_str(&list_row_line(
                row,
                chip_width,
                title_width,
                styler,
                width,
            ));
            out.push('\n');
        }
    }
    if truncated_more > 0 {
        out.push_str(&styler.dim(&truncate(
            &format!("  … {truncated_more} more · -n N or -A to show more"),
            width,
        )));
        out.push('\n');
    }
    if !collapsed.is_empty() {
        let total: usize = collapsed.iter().map(|item| item.count).sum();
        let note = if total == 1 { "note" } else { "notes" };
        let breakdown = collapsed
            .iter()
            .map(|item| format!("{} {}", item.count, item.state))
            .collect::<Vec<_>>()
            .join(" · ");
        out.push_str(&styler.dim(&truncate(
            &format!(
                "  + {total} zorg-era legacy {note} hidden ({breakdown}) · show them with -s legacy"
            ),
            width,
        )));
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&truncate(&library_line(library, coverage), width));
    out.push('\n');
    if pending_sync {
        out.push_str(&styler.dim(
            "  * status changed since the last scan; run `bob ref scan` to reconcile",
        ));
        out.push('\n');
    }
    out
}

/// The human header: `reading queue` for the default view, otherwise
/// `N matching notes`.
fn list_header(
    selection: &ListSelection,
    library: &LibraryCounts,
    listed_total: usize,
) -> String {
    if selection.defaulted {
        format!(
            "bob ref · reading queue · {listed_total} open · {} in library",
            library.notes,
        )
    } else {
        let noun = if listed_total == 1 {
            "matching note"
        } else {
            "matching notes"
        };
        format!(
            "bob ref · {listed_total} {noun} · {} in library",
            library.notes,
        )
    }
}

/// The closing library-counts line in most-advanced state order.
fn library_line(library: &LibraryCounts, coverage: &Coverage) -> String {
    format!(
        "  library  {} finished · {} started · {} queued · {} dropped · {} unknown · coverage {}/ only",
        library.finished,
        library.started,
        library.queued,
        library.dropped,
        library.unknown,
        coverage.ref_dir,
    )
}

/// A reading-state group heading in its chip color.
fn group_heading(styler: Styler, state: &str) -> String {
    match state {
        "finished" => styler.green("FINISHED"),
        "started" => styler.yellow("STARTED"),
        "queued" => styler.cyan("QUEUED"),
        "dropped" => styler.dim("DROPPED"),
        _ => styler.dim("UNKNOWN"),
    }
}

/// The status chip for one list row: the uppercased status (or `—`)
/// with a pending-sync `*` footnote marker.
fn list_status_chip(row: &RefRow) -> String {
    let mut chip = row
        .status
        .as_deref()
        .map(str::to_uppercase)
        .unwrap_or("—".to_string());
    if row.status_sync != "ok" {
        chip.push('*');
    }
    chip
}

/// One aligned list row: status chip, row date (or `—`), title, type
/// with `♫` when audio is bound, and a dim path. The path drops first
/// on narrow terminals, then titles truncate.
fn list_row_line(
    row: &RefRow,
    chip_width: usize,
    title_width: usize,
    styler: Styler,
    width: usize,
) -> String {
    let chip = pad_right(&list_status_chip(row), chip_width);
    let date = row_date(row).unwrap_or("—");
    let mut kind = row.ref_type.clone().unwrap_or("—".to_string());
    if row.audio.is_some() {
        kind.push_str(" ♫");
    }
    let title = pad_right(&row.title, title_width);
    let full = format!("    {chip}  {date}  {title}  {kind}  {}", row.path);
    if display_width(&full) <= width {
        let head = format!("    {chip}  {date}  {title}  {kind}  ");
        return format!("{head}{}", styler.dim(&row.path));
    }
    let without_path = format!("    {chip}  {date}  {title}  {kind}");
    if display_width(&without_path) <= width {
        return without_path;
    }
    let fixed = format!("    {chip}  {date}  ");
    let tail = format!("  {kind}");
    let budget =
        width.saturating_sub(display_width(&fixed) + display_width(&tail));
    let short_title = truncate(row.title.trim_end(), budget.max(1));
    format!("{fixed}{short_title}{tail}")
}

/// Render `bob ref list` rows as a Markdown table plus the
/// matched/returned and coverage lines.
pub(crate) fn render_list_markdown(
    coverage: &Coverage,
    matched: usize,
    returned: &[RefRow],
) -> String {
    let mut out =
        String::from("| State | Status | Date | Title | Type | Note |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for row in returned {
        let state = match row.reading_state.as_str() {
            "finished" => "Finished",
            "started" => "Started",
            "queued" => "Queued",
            "dropped" => "Dropped",
            _ => "Unknown",
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            state,
            escape_cell(&list_status_chip(row)),
            escape_cell(row_date(row).unwrap_or("—")),
            escape_cell(&row.title),
            escape_cell(row.ref_type.as_deref().unwrap_or("—")),
            escape_cell(&row.link),
        ));
    }
    out.push_str(&format!(
        "{} of {matched} matching notes shown · coverage: {}/ only\n",
        returned.len(),
        coverage.ref_dir,
    ));
    out
}

/// Strip ANSI escape sequences for width math on styled chips.
fn strip_ansi(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if char == '\u{1b}' && chars.peek() == Some(&'[') {
            for char in chars.by_ref() {
                if char == 'm' {
                    break;
                }
            }
        } else {
            plain.push(char);
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_carry_glyph_word_and_color() {
        let styler = Styler::colored();
        let cases = [
            ("finished", "✓ FINISHED", "32;1"),
            ("started", "▸ STARTED", "33;1"),
            ("queued", "○ QUEUED", "36;1"),
            ("dropped", "✗ DROPPED", "2"),
            ("unknown", "? UNKNOWN", "2"),
            ("bogus", "? UNKNOWN", "2"),
        ];
        for (state, label, code) in cases {
            let chip = state_chip(styler, state);
            assert!(
                chip.contains(label),
                "chip for {state} misses its label: {chip}",
            );
            assert!(
                chip.contains(&format!("\u{1b}[{code}m")),
                "chip for {state} misses color {code}: {chip}",
            );
        }
        assert!(verdict_chip(styler, Verdict::Possible).contains("≈ POSSIBLE"));
        assert!(verdict_chip(styler, Verdict::NotFound).contains("· NOT FOUND"));
        assert!(verdict_chip(styler, Verdict::InIntake).contains("⇣ IN INTAKE"));
        let plain = Styler::plain();
        assert_eq!(state_chip(plain, "finished"), "✓ FINISHED");
    }

    #[test]
    fn markdown_cells_escape_pipes_and_newlines() {
        assert_eq!(escape_cell("a|b\nc\\d"), "a\\|b c\\\\d");
    }

    fn list_test_row(
        path: &str,
        title: &str,
        status: Option<&str>,
        reading_state: &str,
    ) -> RefRow {
        RefRow {
            path: path.to_string(),
            link: format!("[[{}]]", path.strip_suffix(".md").unwrap_or(path)),
            title: title.to_string(),
            origin: "external".to_string(),
            ref_type: Some("papers".to_string()),
            era: "modern".to_string(),
            status: status.map(str::to_string),
            status_sync: "ok".to_string(),
            frontmatter_status: None,
            legacy_status: None,
            reading_state: reading_state.to_string(),
            reading_state_source: "ref_task:[ ]".to_string(),
            parent: None,
            urls: Vec::new(),
            identity: super::super::RefIdentity {
                keys: Vec::new(),
                arxiv: None,
                doi: None,
            },
            author: None,
            published: None,
            captured: None,
            added: Some("2026-10-01".to_string()),
            added_source: Some("created".to_string()),
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
            source_block: None,
            source_path: None,
        }
    }

    #[test]
    fn list_human_groups_carry_color_and_counts() {
        use super::super::select;
        let selection =
            select(None, None, None, None, None, None, None, Some(50));
        assert!(selection.defaulted);
        let coverage = Coverage {
            ref_dir: "ref".to_string(),
            notes: 2,
            skipped: 0,
            intake: "not_checked".to_string(),
            scope: Coverage::scope_text(),
            annotations: Coverage::annotations_text(),
            git_dates: None,
        };
        let library = LibraryCounts {
            notes: 2,
            queued: 1,
            started: 1,
            ..LibraryCounts::default()
        };
        let rows = vec![
            list_test_row(
                "ref/papers/queued_note.md",
                "Queued Note",
                Some("ready"),
                "queued",
            ),
            list_test_row(
                "ref/papers/started_note.md",
                "Started Note",
                Some("wip"),
                "started",
            ),
        ];
        let colored = render_list_human(
            &selection,
            &coverage,
            &library,
            2,
            &rows,
            &[("started", 1), ("queued", 1)],
            0,
            &[],
            Styler::colored(),
            120,
        );
        assert!(
            colored.contains("reading queue"),
            "header names the default view:\n{colored}"
        );
        assert!(
            colored.contains("\u{1b}[36;1mQUEUED\u{1b}[0m"),
            "queued heading is cyan:\n{colored}"
        );
        assert!(
            colored.contains("\u{1b}[33;1mSTARTED\u{1b}[0m"),
            "started heading is yellow:\n{colored}"
        );
        // Started orders before queued.
        let started = colored.find("STARTED").expect("started heading");
        let queued = colored.find("QUEUED").expect("queued heading");
        assert!(started < queued, "state order:\n{colored}");
        let plain = render_list_human(
            &selection,
            &coverage,
            &library,
            2,
            &rows,
            &[("started", 1), ("queued", 1)],
            0,
            &[],
            Styler::plain(),
            120,
        );
        assert!(!plain.contains('\u{1b}'), "plain has no ANSI:\n{plain}");
        assert!(
            plain.contains("ref/papers/queued_note.md"),
            "wide terminals keep the dim path:\n{plain}"
        );
    }
}
