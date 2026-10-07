//! Exact single-note views for `bob ref show`.
//!
//! A `REF` resolves through the exact match kinds plus a unique
//! `title_exact` hit: several matches collapse to the one non-superseded
//! row, any other multi-match is ambiguous, and a miss suggests up to
//! three title candidates. Shown rows extend the base index row with the
//! parsed annotations (quote and comment kept apart), the user's own
//! notes, and the `## Tasks` lines, in human, Markdown, or versioned JSON.

use clap::ArgMatches;
use serde::Serialize;

use super::cli::list_config_from_matches;
use super::output::{
    dated_suffix, generated_at, print_find_error, print_show_error, state_chip,
    ErrorCandidate, Format, REF_SCHEMA_VERSION,
};
use super::{
    build_index, resolve_query, Coverage, LibraryCounts, MatchKind, RefIndex,
    RefRow,
};
use crate::native::highlights_ref::{
    parse_managed_region, split_frontmatter, split_note_body, RegionBlockKind,
};
use crate::native::style::{
    display_width, pad_right, terminal_width, truncate, Styler,
};

/// One shown annotation: the author's words and the user's words travel
/// in separate fields, exactly as the region parser keeps them.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShowAnnotation {
    pub page_label: Option<String>,
    pub kind: ShowAnnotationKind,
    pub quote: Option<String>,
    pub comment: Option<String>,
    pub asset: Option<String>,
    pub block_id: String,
    pub link: String,
}

/// The annotation kind, serialized as `highlight`, `note`, or `image`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShowAnnotationKind {
    Highlight,
    Note,
    Image,
}

/// What the managed region left out of the shown annotations, as counts:
/// leaked marker mirrors, preamble blocks, and tombstoned highlights.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ShowExcluded {
    pub marker_mirrors: usize,
    pub preamble: usize,
    pub removed: usize,
}

/// One parsed `## Tasks` line: the checkbox state, its mark, the text with
/// the block link and inline fields stripped, and the linked block id.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShowTask {
    pub checked: bool,
    pub mark: char,
    pub text: String,
    pub block_id: String,
}

/// One shown reference: the base index row plus the note's content.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShowRow {
    #[serde(flatten)]
    pub row: RefRow,
    pub annotations_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_region: Option<String>,
    pub annotations: Vec<ShowAnnotation>,
    pub excluded: ShowExcluded,
    pub own_notes: String,
    pub tasks: Vec<ShowTask>,
    pub also: Vec<String>,
}

/// A resolved `REF`: the winning row plus superseded companion paths.
pub(crate) struct ResolvedShow {
    pub row: usize,
    pub also: Vec<String>,
}

/// Why one `REF` failed to resolve: an ambiguous multi-match or a miss
/// with up to three title candidates as hints.
pub(crate) enum ShowFailure {
    Ambiguous {
        query: String,
        candidates: Vec<ErrorCandidate>,
    },
    NotFound {
        query: String,
        candidates: Vec<ErrorCandidate>,
    },
}

/// Resolve one `REF` through the exact match kinds plus a unique
/// `title_exact` hit. Several matches collapse to the one non-superseded
/// row; any other multi-match is ambiguous and never picks the first stem.
pub(crate) fn resolve_show(
    index: &RefIndex,
    query: &str,
) -> Result<ResolvedShow, ShowFailure> {
    let hits = resolve_query(&index.rows, &index.bob_dir, query);
    let mut exact = Vec::new();
    let mut title_exact = Vec::new();
    let mut scored: Vec<(u32, usize)> = Vec::new();
    for hit in &hits {
        match hit.kind {
            MatchKind::Identity
            | MatchKind::Path
            | MatchKind::SourcePdf
            | MatchKind::Id
            | MatchKind::Stem => exact.push(hit.row),
            MatchKind::TitleExact => {
                title_exact.push(hit.row);
                if let Some(score) = hit.score {
                    scored.push((score, hit.row));
                }
            }
            MatchKind::Title | MatchKind::SlugTitle => {
                if let Some(score) = hit.score {
                    scored.push((score, hit.row));
                }
            }
        }
    }
    if title_exact.len() == 1 && !exact.contains(&title_exact[0]) {
        exact.push(title_exact[0]);
    }
    exact.sort_by(|left, right| {
        super::primary_rank(&index.rows[*left])
            .cmp(&super::primary_rank(&index.rows[*right]))
    });
    if exact.len() == 1 {
        return Ok(ResolvedShow {
            row: exact[0],
            also: Vec::new(),
        });
    }
    if exact.len() > 1 {
        let live: Vec<usize> = exact
            .iter()
            .copied()
            .filter(|row| index.rows[*row].superseded_by.is_none())
            .collect();
        if live.len() == 1 {
            let winner = live[0];
            let also = exact
                .iter()
                .filter(|row| **row != winner)
                .map(|row| index.rows[*row].path.clone())
                .collect();
            return Ok(ResolvedShow { row: winner, also });
        }
        return Err(ShowFailure::Ambiguous {
            query: query.to_string(),
            candidates: exact
                .iter()
                .map(|row| {
                    ErrorCandidate::new(
                        &index.rows[*row].path,
                        &index.rows[*row].title,
                    )
                })
                .collect(),
        });
    }
    scored.sort_by_key(|left| std::cmp::Reverse(left.0));
    scored.dedup_by_key(|(_, row)| *row);
    Err(ShowFailure::NotFound {
        query: query.to_string(),
        candidates: scored
            .into_iter()
            .take(3)
            .map(|(_, row)| {
                ErrorCandidate::new(
                    &index.rows[row].path,
                    &index.rows[row].title,
                )
            })
            .collect(),
    })
}

/// Load one resolved row's content: the managed region parsed back into
/// annotations (mirror, preamble, and tombstone blocks excluded and
/// counted), the user's own notes, and the `## Tasks` lines.
pub(crate) fn load_show_row(
    index: &RefIndex,
    resolved: &ResolvedShow,
    comments_only: bool,
    no_annotations: bool,
) -> Result<ShowRow, String> {
    let row = &index.rows[resolved.row];
    let contents = std::fs::read_to_string(index.bob_dir.join(&row.path))
        .map_err(|error| {
            format!("could not read note {}: {error}", row.path)
        })?;
    let body = split_frontmatter(&contents)
        .map(|(_, rest)| rest)
        .unwrap_or(contents);
    let parts = split_note_body(&body);
    let stem = row.path.strip_suffix(".md").unwrap_or(&row.path);
    let mut annotations = Vec::new();
    let mut excluded = ShowExcluded::default();
    let (annotations_status, raw_region) = match parts.region.as_deref() {
        None => ("absent".to_string(), None),
        Some(region) => {
            let parsed = parse_managed_region(region);
            for block in &parsed.blocks {
                if block.mirror {
                    excluded.marker_mirrors += 1;
                    continue;
                }
                if block.in_preamble {
                    excluded.preamble += 1;
                    continue;
                }
                let (kind, quote, comment, asset) = match block.kind {
                    RegionBlockKind::Highlight => (
                        ShowAnnotationKind::Highlight,
                        block.quote.clone(),
                        block.comment.clone(),
                        None,
                    ),
                    RegionBlockKind::Note => (
                        ShowAnnotationKind::Note,
                        None,
                        block.comment.clone(),
                        None,
                    ),
                    RegionBlockKind::Image => (
                        ShowAnnotationKind::Image,
                        None,
                        block.comment.clone(),
                        block.asset.clone(),
                    ),
                };
                annotations.push(ShowAnnotation {
                    page_label: block.page_label.clone(),
                    kind,
                    quote,
                    comment,
                    asset,
                    block_id: block.block_id.clone(),
                    link: format!("[[{stem}#^{}]]", block.block_id),
                });
            }
            excluded.removed = parsed.removed.len();
            if parsed.unparsed.is_empty() {
                ("parsed".to_string(), None)
            } else {
                ("unparsed".to_string(), Some(region.to_string()))
            }
        }
    };
    if no_annotations {
        annotations.clear();
    } else if comments_only {
        annotations.retain(|annotation| {
            annotation.kind == ShowAnnotationKind::Note
                || annotation
                    .comment
                    .as_deref()
                    .is_some_and(|comment| !comment.is_empty())
        });
    }
    Ok(ShowRow {
        row: row.clone(),
        annotations_status,
        raw_region,
        annotations,
        excluded,
        own_notes: parts.own_notes,
        tasks: parts
            .tasks
            .into_iter()
            .map(|task| ShowTask {
                checked: task.checked,
                mark: task.mark,
                text: task.text,
                block_id: task.block_id,
            })
            .collect(),
        also: resolved.also.clone(),
    })
}

/// The `ref show` success envelope: coverage, library counts, and the
/// extended content rows, in query order.
#[derive(Debug, Clone, Serialize)]
struct ShowEnvelope<'a> {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    coverage: &'a Coverage,
    library: &'a LibraryCounts,
    refs: &'a [ShowRow],
}

/// Print the `ref show` envelope as compact one-line JSON.
fn print_show_json(
    coverage: &Coverage,
    library: &LibraryCounts,
    rows: &[ShowRow],
) {
    let envelope = ShowEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref show",
        generated_at: generated_at(),
        coverage,
        library,
        refs: rows,
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("show envelope serializes")
    );
}

/// Run `bob ref show`: resolve every `REF` and load its content before
/// printing anything, so one failure exits 1 and prints nothing else.
pub(crate) fn run_show(matches: &ArgMatches) -> i32 {
    let format = Format::from_name(
        matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human"),
    );
    let config = list_config_from_matches(matches);
    let index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            print_find_error(
                format,
                "ref show",
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
            return 1;
        }
    };
    let queries = matches
        .get_many::<String>("ref")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let comments_only = matches.get_flag("comments-only");
    let no_annotations = matches.get_flag("no-annotations");
    let mut rows = Vec::with_capacity(queries.len());
    for query in &queries {
        match resolve_show(&index, query) {
            Ok(resolved) => {
                match load_show_row(
                    &index,
                    &resolved,
                    comments_only,
                    no_annotations,
                ) {
                    Ok(row) => rows.push(row),
                    Err(message) => {
                        print_find_error(
                            format,
                            "ref show",
                            "read_failed",
                            &message,
                            None,
                        );
                        return 1;
                    }
                }
            }
            Err(ShowFailure::Ambiguous { query, candidates }) => {
                print_show_error(
                    format,
                    "ambiguous_reference",
                    &format!("ambiguous reference: {query}"),
                    &candidates,
                );
                return 1;
            }
            Err(ShowFailure::NotFound { query, candidates }) => {
                print_show_error(
                    format,
                    "unknown_reference",
                    &format!("no reference note matches {query}"),
                    &candidates,
                );
                return 1;
            }
        }
    }
    match format {
        Format::Json => {
            print_show_json(&index.coverage, &index.counts, &rows);
        }
        Format::Markdown => {
            print!("{}", render_show_markdown(&rows, no_annotations));
        }
        Format::Human => {
            print!(
                "{}",
                render_show_human(
                    &rows,
                    comments_only,
                    no_annotations,
                    Styler::detect(),
                    terminal_width(),
                )
            );
        }
    }
    0
}

/// `1 comment` / `3 comments`: the sketch pluralizes, so the header does.
fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// The annotation summary for the human header and the Markdown bullet:
/// the shown highlights, notes, and images plus the shown comment count.
fn annotation_summary(shown: &[ShowAnnotation]) -> String {
    let mut highlights = 0usize;
    let mut notes = 0usize;
    let mut images = 0usize;
    let mut comments = 0usize;
    for annotation in shown {
        match annotation.kind {
            ShowAnnotationKind::Highlight => highlights += 1,
            ShowAnnotationKind::Note => notes += 1,
            ShowAnnotationKind::Image => images += 1,
        }
        if annotation
            .comment
            .as_deref()
            .is_some_and(|comment| !comment.is_empty())
        {
            comments += 1;
        }
    }
    // Standalone notes always carry their text as the comment.
    let mut parts = Vec::new();
    if highlights > 0 {
        parts.push(plural(highlights, "highlight", "highlights"));
    }
    if notes > 0 {
        parts.push(plural(notes, "note", "notes"));
    }
    if images > 0 {
        parts.push(plural(images, "image", "images"));
    }
    if parts.is_empty() {
        parts.push("0 highlights".to_string());
    }
    parts.push(plural(comments, "comment", "comments"));
    parts.join(" · ")
}

/// The header line under the title: state chip, date, type, origin, path.
fn header_line(row: &ShowRow, styler: Styler, width: usize) -> String {
    let mut segments = vec![state_chip(styler, &row.row.reading_state)];
    if let Some(dated) = dated_suffix(&row.row) {
        segments.push(dated);
    }
    if let Some(ref_type) = row.row.ref_type.as_deref() {
        segments.push(ref_type.to_string());
    }
    segments.push(row.row.origin.clone());
    segments.push(row.row.path.clone());
    truncate(&segments.join(" · "), width)
}

/// Metadata rows with values only: source (with arXiv/DOI), PDF, audio,
/// parent, and the last-scan snapshot.
fn metadata_rows(row: &ShowRow) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if !row.row.urls.is_empty() {
        let mut source = row.row.urls.join(", ");
        if let Some(arxiv) = row.row.identity.arxiv.as_deref() {
            source.push_str(&format!(" (arXiv {arxiv})"));
        } else if let Some(doi) = row.row.identity.doi.as_deref() {
            source.push_str(&format!(" (DOI {doi})"));
        }
        rows.push(("source".to_string(), source));
    }
    if let Some(pdf) = row.row.source_pdf.as_deref() {
        rows.push(("pdf".to_string(), pdf.to_string()));
    }
    if let Some(audio) = row.row.audio.as_deref() {
        rows.push(("audio".to_string(), audio.to_string()));
    }
    if let Some(parent) = row.row.parent.as_deref() {
        rows.push(("parent".to_string(), parent.to_string()));
    }
    if let Some(snapshot) = row.row.snapshot.as_ref() {
        let value = match snapshot.synced_at.as_deref() {
            Some(synced_at) => {
                format!("annotations as of the last scan, {synced_at}")
            }
            None => "annotations as of the last scan".to_string(),
        };
        rows.push(("snapshot".to_string(), value));
    }
    rows
}

/// The dim parenthetical for excluded region blocks, or `None` when
/// nothing was excluded.
fn excluded_line(excluded: &ShowExcluded) -> Option<String> {
    let mut parts = Vec::new();
    if excluded.marker_mirrors > 0 {
        parts.push(plural(
            excluded.marker_mirrors,
            "leaked marker block",
            "leaked marker blocks",
        ));
    }
    if excluded.preamble > 0 {
        parts.push(plural(
            excluded.preamble,
            "preamble block",
            "preamble blocks",
        ));
    }
    if excluded.removed > 0 {
        parts.push(plural(
            excluded.removed,
            "removed highlight",
            "removed highlights",
        ));
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!("({} not shown)", parts.join(" and ")))
}

/// Word-wrap paragraphs to `width` display columns. Hard-splits words
/// longer than the width; blank lines stay blank.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut len = 0usize;
        for word in paragraph.split_whitespace() {
            for chunk in split_word(word, width) {
                let chunk_len = display_width(&chunk);
                if len == 0 {
                    line.push_str(&chunk);
                    len = chunk_len;
                } else if len + 1 + chunk_len <= width {
                    line.push(' ');
                    line.push_str(&chunk);
                    len += 1 + chunk_len;
                } else {
                    out.push(std::mem::take(&mut line));
                    line.push_str(&chunk);
                    len = chunk_len;
                }
            }
        }
        out.push(line);
    }
    out
}

/// Split one word into display-width chunks that fit `width`.
fn split_word(word: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    if display_width(word) <= width {
        return vec![word.to_string()];
    }
    chars
        .chunks(width.max(1))
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// Render one quote: an opening curly quote on the first line, a hanging
/// indent after, and the closing quote on the last line.
fn quote_lines(quote: &str, width: usize) -> Vec<String> {
    let wrapped = wrap_text(quote, width.saturating_sub(5).max(1));
    let mut out = Vec::with_capacity(wrapped.len().max(1));
    for (index, line) in wrapped.iter().enumerate() {
        if index == 0 {
            out.push(format!("    “{line}"));
        } else {
            out.push(format!("     {line}"));
        }
    }
    if let Some(last) = out.last_mut() {
        last.push('”');
    }
    out
}

/// Render one comment: a cyan `↳` leader with a hanging indent.
fn comment_lines(comment: &str, styler: Styler, width: usize) -> Vec<String> {
    wrap_text(comment, width.saturating_sub(8).max(1))
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let plain = if index == 0 {
                format!("      ↳ {line}")
            } else {
                format!("        {line}")
            };
            styler.cyan(&plain)
        })
        .collect()
}

/// Render one annotation block: the quote (unless empty), then the
/// comment for highlights and images, or the bare note text.
fn annotation_lines(
    annotation: &ShowAnnotation,
    styler: Styler,
    width: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    match annotation.kind {
        ShowAnnotationKind::Highlight => {
            if let Some(quote) = annotation.quote.as_deref()
                && !quote.is_empty()
            {
                out.extend(quote_lines(quote, width));
            }
            if let Some(comment) = annotation.comment.as_deref()
                && !comment.is_empty()
            {
                out.extend(comment_lines(comment, styler, width));
            }
        }
        ShowAnnotationKind::Note => {
            if let Some(note) = annotation.comment.as_deref()
                && !note.is_empty()
            {
                for line in wrap_text(note, width.saturating_sub(4).max(1)) {
                    out.push(format!("    {line}"));
                }
            }
        }
        ShowAnnotationKind::Image => {
            if let Some(asset) = annotation.asset.as_deref() {
                out.push(
                    styler.dim(&truncate(
                        &format!("    [image: {asset}]"),
                        width,
                    )),
                );
            }
            if let Some(comment) = annotation.comment.as_deref()
                && !comment.is_empty()
            {
                out.extend(comment_lines(comment, styler, width));
            }
        }
    }
    out
}

/// Render one shown row in human form.
fn render_one_human(
    row: &ShowRow,
    comments_only: bool,
    no_annotations: bool,
    styler: Styler,
    width: usize,
) -> String {
    let mut out = String::new();
    out.push_str(&truncate(&row.row.title, width));
    out.push('\n');
    out.push_str(&header_line(row, styler, width));
    out.push('\n');
    for (label, value) in metadata_rows(row) {
        out.push_str(&truncate(
            &format!("  {}  {value}", pad_right(&label, 8)),
            width,
        ));
        out.push('\n');
    }
    for path in &row.also {
        out.push_str(&styler.dim(&truncate(
            &format!("  also: {path} (superseded legacy note)"),
            width,
        )));
        out.push('\n');
    }
    if !no_annotations {
        let has_excluded = excluded_line(&row.excluded);
        if row.annotations.is_empty() {
            if comments_only {
                out.push('\n');
                out.push_str("No comments or standalone notes.\n");
            } else if let Some(excluded) = has_excluded {
                out.push('\n');
                out.push_str(
                    &styler.dim(&truncate(&format!("  {excluded}"), width)),
                );
                out.push('\n');
            }
        } else {
            out.push('\n');
            out.push_str(&truncate(
                &format!(
                    "ANNOTATIONS · {}",
                    annotation_summary(&row.annotations)
                ),
                width,
            ));
            out.push('\n');
            let mut current_page: Option<String> = None;
            let mut page_seen = false;
            for annotation in &row.annotations {
                match annotation.page_label.as_deref() {
                    Some(page) if current_page.as_deref() != Some(page) => {
                        out.push_str(&truncate(&format!("  {page}"), width));
                        out.push('\n');
                        current_page = Some(page.to_string());
                        page_seen = true;
                    }
                    None if page_seen => {
                        current_page = None;
                    }
                    _ => {}
                }
                for line in annotation_lines(annotation, styler, width) {
                    out.push_str(line.trim_end());
                    out.push('\n');
                }
            }
            if let Some(excluded) = has_excluded {
                out.push_str(
                    &styler.dim(&truncate(&format!("  {excluded}"), width)),
                );
                out.push('\n');
            }
        }
    }
    if !row.own_notes.is_empty() {
        out.push('\n');
        out.push_str("NOTES\n");
        for line in wrap_text(&row.own_notes, width.saturating_sub(2).max(1)) {
            if line.is_empty() {
                out.push('\n');
            } else {
                out.push_str(&truncate(&format!("  {line}"), width));
                out.push('\n');
            }
        }
    }
    if !row.tasks.is_empty() {
        out.push('\n');
        out.push_str("TASKS\n");
        for task in &row.tasks {
            let box_glyph = if task.checked { "☑" } else { "☐" };
            // The linked annotation's page label travels as a dim
            // suffix; tasks without a matching annotation keep no suffix.
            let page = row
                .annotations
                .iter()
                .find(|annotation| annotation.block_id == task.block_id)
                .and_then(|annotation| annotation.page_label.as_deref());
            let suffix =
                page.map(|page| format!("   {page}")).unwrap_or_default();
            let base = truncate(
                &format!("  {box_glyph} {}", task.text),
                width.saturating_sub(display_width(&suffix)).max(1),
            );
            out.push_str(&base);
            if !suffix.is_empty() {
                out.push_str(&styler.dim(&suffix));
            }
            out.push('\n');
        }
    }
    out
}

/// Render shown rows in human form, separated by a dim rule.
pub(crate) fn render_show_human(
    rows: &[ShowRow],
    comments_only: bool,
    no_annotations: bool,
    styler: Styler,
    width: usize,
) -> String {
    let rule = styler.dim(&"─".repeat(width.max(1)));
    rows.iter()
        .map(|row| {
            render_one_human(row, comments_only, no_annotations, styler, width)
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(&format!("\n{rule}\n"))
        + "\n"
}

/// The reading-state bullet: state with its evidence and date, when known.
fn state_bullet(row: &ShowRow) -> String {
    match dated_suffix(&row.row) {
        Some(dated) => format!(
            "{} ({}; {dated})",
            row.row.reading_state, row.row.reading_state_source
        ),
        None => format!(
            "{} ({})",
            row.row.reading_state, row.row.reading_state_source
        ),
    }
}

/// The source bullet: every stored URL plus the arXiv/DOI identity.
fn source_bullet(row: &ShowRow) -> Option<String> {
    if row.row.urls.is_empty() {
        return None;
    }
    let mut source = row.row.urls.join(", ");
    if let Some(arxiv) = row.row.identity.arxiv.as_deref() {
        source.push_str(&format!(" (arXiv {arxiv})"));
    } else if let Some(doi) = row.row.identity.doi.as_deref() {
        source.push_str(&format!(" (DOI {doi})"));
    }
    Some(source)
}

/// Render one shown row as a Markdown digest for an agent's context.
/// Content flags are already applied to `row.annotations` at load time;
/// empty sections stay omitted.
fn render_one_markdown(row: &ShowRow, no_annotations: bool) -> String {
    let mut out = format!("## {}\n", row.row.title);
    out.push_str(&format!("\n- Note: {}\n", row.row.link));
    out.push_str(&format!("- Reading state: {}\n", state_bullet(row)));
    if let Some(source) = source_bullet(row) {
        out.push_str(&format!("- Source: {source}\n"));
    }
    out.push_str(&format!(
        "- Origin: {} · Type: {}\n",
        row.row.origin,
        row.row.ref_type.as_deref().unwrap_or("—"),
    ));
    let mut counts =
        format!("- Annotations: {}", annotation_summary(&row.annotations));
    if let Some(synced_at) = row
        .row
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.synced_at.as_deref())
    {
        counts.push_str(&format!(" (snapshot {synced_at})"));
    }
    out.push_str(&format!("{counts}\n"));
    if let Some(research) = row.row.research_ref.as_deref() {
        out.push_str(&format!("- Research report: {research}\n"));
    }
    if !row.also.is_empty() {
        out.push_str(&format!("- Also: {}\n", row.also.join(", ")));
    }
    if !no_annotations && !row.annotations.is_empty() {
        out.push_str("\n### Annotations\n");
        let mut current_page: Option<String> = None;
        for annotation in &row.annotations {
            if annotation.page_label != current_page {
                current_page = annotation.page_label.clone();
                if let Some(page) = current_page.as_deref() {
                    out.push_str(&format!("\n**{page}**\n"));
                }
            }
            match annotation.kind {
                ShowAnnotationKind::Highlight => {
                    if let Some(quote) = annotation.quote.as_deref()
                        && !quote.is_empty()
                    {
                        out.push('\n');
                        for line in quote.lines() {
                            out.push_str(&format!("> {line}\n"));
                        }
                    }
                    if let Some(comment) = annotation.comment.as_deref()
                        && !comment.is_empty()
                    {
                        out.push('\n');
                        let mut lines = comment.lines();
                        if let Some(first) = lines.next() {
                            out.push_str(&format!("Comment: {first}\n"));
                        }
                        for line in lines {
                            out.push_str(&format!("{line}\n"));
                        }
                    }
                }
                ShowAnnotationKind::Note => {
                    if let Some(note) = annotation.comment.as_deref()
                        && !note.is_empty()
                    {
                        out.push('\n');
                        let mut lines = note.lines();
                        if let Some(first) = lines.next() {
                            out.push_str(&format!("Note: {first}\n"));
                        }
                        for line in lines {
                            out.push_str(&format!("{line}\n"));
                        }
                    }
                }
                ShowAnnotationKind::Image => {
                    if let Some(asset) = annotation.asset.as_deref() {
                        out.push_str(&format!("\nImage: {asset}\n"));
                    }
                    if let Some(comment) = annotation.comment.as_deref()
                        && !comment.is_empty()
                    {
                        out.push_str(&format!("\nComment: {comment}\n"));
                    }
                }
            }
        }
    }
    if !row.own_notes.is_empty() {
        out.push_str(&format!("\n### Notes\n\n{}\n", row.own_notes));
    }
    if !row.tasks.is_empty() {
        out.push_str("\n### Tasks\n\n");
        for task in &row.tasks {
            out.push_str(&format!("- [{}] {}\n", task.mark, task.text));
        }
    }
    out
}

/// Render shown rows as Markdown digests, separated by a rule.
pub(crate) fn render_show_markdown(
    rows: &[ShowRow],
    no_annotations: bool,
) -> String {
    rows.iter()
        .map(|row| {
            render_one_markdown(row, no_annotations)
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::super::{RefIdentity, RefSnapshot};
    use super::*;

    fn show_test_row() -> ShowRow {
        ShowRow {
            row: RefRow {
                path: "ref/papers/ea_graph.md".to_string(),
                link: "[[ref/papers/ea_graph]]".to_string(),
                title: "EA-Graph: Verification Memory".to_string(),
                origin: "external".to_string(),
                ref_type: Some("papers".to_string()),
                era: "modern".to_string(),
                status: Some("read".to_string()),
                status_sync: "ok".to_string(),
                frontmatter_status: None,
                legacy_status: None,
                reading_state: "finished".to_string(),
                reading_state_source: "ref_task:[x]".to_string(),
                parent: Some("sase_ref".to_string()),
                urls: vec!["https://arxiv.org/pdf/2608.04278".to_string()],
                identity: RefIdentity {
                    keys: vec![
                        "https://arxiv.org/abs/2608.04278".to_string(),
                        "arxiv:2608.04278".to_string(),
                    ],
                    arxiv: Some("2608.04278".to_string()),
                    doi: None,
                },
                author: None,
                published: None,
                captured: None,
                added: Some("2026-10-01".to_string()),
                added_source: Some("created".to_string()),
                finished: Some("2026-10-03".to_string()),
                finished_source: Some("ref_task".to_string()),
                source_pdf: Some("lib/papers/ea_graph.pdf".to_string()),
                audio: None,
                annotation_count: 2,
                comment_count: 2,
                snapshot: Some(RefSnapshot {
                    synced_at: Some("2026-10-03T18:00:00".to_string()),
                    highlights_count: None,
                }),
                research_ref: None,
                superseded_by: None,
                diagnostics: Vec::new(),
                id: None,
                source_block: None,
                source_path: None,
            },
            annotations_status: "parsed".to_string(),
            raw_region: None,
            annotations: vec![
                ShowAnnotation {
                    page_label: Some("Page 2".to_string()),
                    kind: ShowAnnotationKind::Highlight,
                    quote: Some("A determinism contract.".to_string()),
                    comment: Some("Support replay?".to_string()),
                    asset: None,
                    block_id: "h-aaaaaaaaaaaaaaaa".to_string(),
                    link: "[[ref/papers/ea_graph#^h-aaaaaaaaaaaaaaaa]]"
                        .to_string(),
                },
                ShowAnnotation {
                    page_label: Some("Page 2".to_string()),
                    kind: ShowAnnotationKind::Note,
                    quote: None,
                    comment: Some("A standalone note.".to_string()),
                    asset: None,
                    block_id: "h-bbbbbbbbbbbbbbbb".to_string(),
                    link: "[[ref/papers/ea_graph#^h-bbbbbbbbbbbbbbbb]]"
                        .to_string(),
                },
            ],
            excluded: ShowExcluded {
                marker_mirrors: 1,
                preamble: 0,
                removed: 2,
            },
            own_notes: "My own synthesis.".to_string(),
            tasks: vec![ShowTask {
                checked: false,
                mark: ' ',
                text: "Compare with the appendix.".to_string(),
                block_id: "h-aaaaaaaaaaaaaaaa".to_string(),
            }],
            also: Vec::new(),
        }
    }

    #[test]
    fn human_render_carries_chips_color_and_structure() {
        let rows = vec![show_test_row()];
        let colored =
            render_show_human(&rows, false, false, Styler::colored(), 120);
        assert!(
            colored.contains("EA-Graph: Verification Memory"),
            "title first:\n{colored}"
        );
        assert!(
            colored.contains("\u{1b}[32;1m✓ FINISHED\u{1b}[0m"),
            "finished chip is green:\n{colored}"
        );
        assert!(
            colored.contains("read 2026-10-03"),
            "dated suffix:\n{colored}"
        );
        assert!(
            colored.contains("(arXiv 2608.04278)"),
            "source identity suffix:\n{colored}"
        );
        assert!(
            colored.contains("\u{1b}[36;1m      ↳ Support replay?\u{1b}[0m"),
            "comment leader is cyan:\n{colored}"
        );
        assert!(
            colored.contains(
                "1 leaked marker block and 2 removed highlights not shown"
            ),
            "excluded counts:\n{colored}"
        );
        assert!(
            colored.contains("ANNOTATIONS · 1 highlight · 1 note · 2 comments"),
            "shown summary:\n{colored}"
        );
        assert!(
            colored.contains("NOTES") && colored.contains("TASKS"),
            "own sections:\n{colored}"
        );
        let plain =
            render_show_human(&rows, false, false, Styler::plain(), 120);
        assert!(!plain.contains('\u{1b}'), "plain has no ANSI:\n{plain}");
        assert!(
            plain.contains("“A determinism contract.”"),
            "curly quotes:\n{plain}"
        );
    }

    #[test]
    fn wrap_hard_splits_long_words_and_keeps_blanks() {
        let lines = wrap_text("ab\n\nsupercalifragilistic", 5);
        assert_eq!(lines, vec!["ab", "", "super", "calif", "ragil", "istic"]);
    }

    #[test]
    fn markdown_digest_keeps_quote_comment_and_tasks() {
        let markdown = render_show_markdown(&[show_test_row()], false);
        assert!(
            markdown.contains("## EA-Graph: Verification Memory"),
            "title heading:\n{markdown}"
        );
        assert!(
            markdown.contains(
                "- Reading state: finished (ref_task:[x]; read 2026-10-03)"
            ),
            "state with evidence and date:\n{markdown}"
        );
        assert!(
            markdown.contains("> A determinism contract."),
            "blockquote:\n{markdown}"
        );
        assert!(
            markdown.contains("Comment: Support replay?"),
            "comment line:\n{markdown}"
        );
        assert!(
            markdown.contains("Note: A standalone note."),
            "standalone note:\n{markdown}"
        );
        assert!(
            markdown.contains("- [ ] Compare with the appendix."),
            "tasks:\n{markdown}"
        );
    }
}
