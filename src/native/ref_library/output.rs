//! Shared output plumbing for the `bob ref` library verbs.
//!
//! One [`REF_SCHEMA_VERSION`], one JSON envelope, one human/machine error
//! shape, the reading-state chips, and Markdown table escaping live here so
//! `find`, `list`, and `show` render alike. Stdout in JSON mode is pure
//! JSON; human errors go to stderr as `bob ref: error: <message>` with an
//! optional `hint:` line.
use serde::Serialize;

use super::find::{FindResult, FindSummary, Verdict};
use super::{Coverage, LibraryCounts};
use crate::native::env as bob_env;
use crate::native::highlights_ref::COMMAND_NAME;
use crate::native::style::{display_width, truncate, Styler};

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

/// The JSON error envelope shared by the library verbs.
#[derive(Debug, Clone, Serialize)]
struct ErrorEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    error: ErrorBody,
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

/// Escape one Markdown table cell: backslashes, pipes, and newlines.
pub(crate) fn escape_cell(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', " ")
        .replace('\r', "")
}

/// A reading-state chip: the glyph plus word always render, and color
/// only reinforces them.
fn state_chip(styler: Styler, reading_state: &str) -> String {
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
fn dated_suffix(row: &super::RefRow) -> Option<String> {
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
}
