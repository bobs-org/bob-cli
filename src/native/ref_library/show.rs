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
use std::collections::BTreeSet;

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
    UnresolvedImageNoteState,
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
    pub images: Vec<String>,
    pub unresolved_images: Vec<String>,
    #[serde(skip)]
    pub unresolved_image_states: Vec<UnresolvedImageNoteState>,
    /// Tucked `<` group pieces, head first. Present only for groups; empty
    /// for every single highlight, note, and image.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pieces: Vec<ShowPiece>,
    pub block_id: String,
    pub link: String,
}

/// One tucked `<` group piece: the highlight's own words, page, comment,
/// and image notes, in sidecar order. The top-level annotation fields keep
/// the flattened group view so current consumers still see every word.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShowPiece {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_label: Option<String>,
    pub quote: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub images: Vec<String>,
    pub unresolved_images: Vec<String>,
    #[serde(skip)]
    pub unresolved_image_states: Vec<UnresolvedImageNoteState>,
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
/// `path` is set only on elsewhere follow-ups, so in-note rows stay
/// byte-identical.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShowTask {
    pub checked: bool,
    pub mark: char,
    pub text: String,
    pub block_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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
                    images: block.images.clone(),
                    unresolved_images: block.unresolved_images.clone(),
                    unresolved_image_states: block
                        .unresolved_image_states
                        .clone(),
                    pieces: block
                        .pieces
                        .iter()
                        .map(|piece| ShowPiece {
                            page_label: piece.page_label.clone(),
                            quote: piece.quote.clone(),
                            comment: piece.comment.clone(),
                            images: piece.images.clone(),
                            unresolved_images: piece.unresolved_images.clone(),
                            unresolved_image_states: piece
                                .unresolved_image_states
                                .clone(),
                        })
                        .collect(),
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
                || !annotation.images.is_empty()
                || !annotation.unresolved_images.is_empty()
        });
    }
    let mut tasks: Vec<ShowTask> = parts
        .tasks
        .into_iter()
        .map(|task| ShowTask {
            checked: task.checked,
            mark: task.mark,
            text: task.text,
            block_id: task.block_id,
            path: None,
        })
        .collect();
    // Follow-ups found elsewhere, after the in-note ones.
    for follow in index.ref_tasks.follow_ups(&row.path) {
        tasks.push(ShowTask {
            checked: matches!(follow.mark, 'x' | 'X'),
            mark: follow.mark,
            text: follow.text.clone(),
            block_id: follow.block_id.clone(),
            path: Some(follow.path.clone()),
        });
    }
    Ok(ShowRow {
        row: row.clone(),
        annotations_status,
        raw_region,
        annotations,
        excluded,
        own_notes: parts.own_notes,
        tasks,
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
                    "ref show",
                    "ambiguous_reference",
                    &format!("ambiguous reference: {query}"),
                    &candidates,
                );
                return 1;
            }
            Err(ShowFailure::NotFound { query, candidates }) => {
                print_show_error(
                    format,
                    "ref show",
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
            || !annotation.images.is_empty()
            || !annotation.unresolved_images.is_empty()
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
    wrap_text_with_source(text, width)
        .into_iter()
        .map(|(line, _)| line)
        .collect()
}

/// Wrap like [`wrap_text`], tracking each output character's source index
/// in `text`. Inserted spaces between words have no source.
fn wrap_text_with_source(
    text: &str,
    width: usize,
) -> Vec<(String, Vec<Option<usize>>)> {
    let width = width.max(1);
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    for (index, character) in chars.iter().copied().enumerate() {
        if character == '\n' {
            push_wrapped_paragraph(
                &chars[start..index],
                start,
                width,
                &mut out,
            );
            start = index + 1;
        }
    }
    push_wrapped_paragraph(&chars[start..], start, width, &mut out);
    out
}

fn push_wrapped_paragraph(
    paragraph: &[char],
    paragraph_start: usize,
    width: usize,
    out: &mut Vec<(String, Vec<Option<usize>>)>,
) {
    let mut line = String::new();
    let mut source = Vec::new();
    let mut len = 0usize;
    for (word, word_source) in paragraph_words(paragraph, paragraph_start) {
        for (chunk, chunk_source) in
            split_word_with_source(&word, &word_source, width)
        {
            let chunk_len = display_width(&chunk);
            if len == 0 {
                line.push_str(&chunk);
                source.extend(chunk_source.into_iter().map(Some));
                len = chunk_len;
            } else if len + 1 + chunk_len <= width {
                line.push(' ');
                source.push(None);
                line.push_str(&chunk);
                source.extend(chunk_source.into_iter().map(Some));
                len += 1 + chunk_len;
            } else {
                out.push((
                    std::mem::take(&mut line),
                    std::mem::take(&mut source),
                ));
                line.push_str(&chunk);
                source.extend(chunk_source.into_iter().map(Some));
                len = chunk_len;
            }
        }
    }
    out.push((line, source));
}

fn paragraph_words(
    paragraph: &[char],
    paragraph_start: usize,
) -> Vec<(String, Vec<usize>)> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut current_source = Vec::new();
    for (offset, character) in paragraph.iter().copied().enumerate() {
        if character.is_whitespace() {
            if !current.is_empty() {
                words.push((
                    std::mem::take(&mut current),
                    std::mem::take(&mut current_source),
                ));
            }
        } else {
            current.push(character);
            current_source.push(paragraph_start + offset);
        }
    }
    if !current.is_empty() {
        words.push((current, current_source));
    }
    words
}

fn split_word_with_source(
    word: &str,
    source: &[usize],
    width: usize,
) -> Vec<(String, Vec<usize>)> {
    let chars: Vec<char> = word.chars().collect();
    if display_width(word) <= width {
        return vec![(word.to_string(), source.to_vec())];
    }
    let width = width.max(1);
    chars
        .chunks(width)
        .enumerate()
        .map(|(index, chunk)| {
            let start = index * width;
            (
                chunk.iter().collect(),
                source[start..start + chunk.len()].to_vec(),
            )
        })
        .collect()
}

/// Render one quote: an opening curly quote on the first line, a hanging
/// indent after, and the closing quote on the last line.
fn quote_lines(quote: &str, styler: Styler, width: usize) -> Vec<String> {
    let wrapped = wrap_text_with_source(quote, width.saturating_sub(5).max(1));
    let mut out = Vec::with_capacity(wrapped.len().max(1));
    let colored_lines = styler_quote_lines(quote, &wrapped, styler);
    for (index, line) in colored_lines.iter().enumerate() {
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

fn styler_quote_lines(
    quote: &str,
    lines: &[(String, Vec<Option<usize>>)],
    styler: Styler,
) -> Vec<String> {
    if !styler.is_color() {
        return lines.iter().map(|(line, _)| line.clone()).collect();
    }
    let quote_chars = quote.chars().collect::<Vec<_>>();
    let mut markers = Vec::new();
    let mut index = 0;
    while index + 1 < quote_chars.len() {
        if quote_chars[index] == '*' && quote_chars[index + 1] == '*' {
            markers.push(index);
            index += 2;
        } else {
            index += 1;
        }
    }
    let mut paired_firsts = BTreeSet::new();
    let mut paired_stars = BTreeSet::new();
    for pair in markers.chunks_exact(2) {
        if pair[1] > pair[0] + 2 {
            for first in [pair[0], pair[1]] {
                paired_firsts.insert(first);
                paired_stars.insert(first);
                paired_stars.insert(first + 1);
            }
        }
    }
    if paired_firsts.is_empty() {
        return lines.iter().map(|(line, _)| line.clone()).collect();
    }
    let mut consume_second = false;
    let mut active = false;
    lines
        .iter()
        .map(|(line, sources)| {
            let chars: Vec<char> = line.chars().collect();
            debug_assert_eq!(chars.len(), sources.len());
            let mut rendered = String::new();
            let mut segment = String::new();
            let mut cursor = 0usize;
            while cursor < chars.len() {
                let character = chars[cursor];
                let source = sources.get(cursor).copied().flatten();
                if consume_second {
                    consume_second = false;
                    if character == '*'
                        && source
                            .is_some_and(|index| paired_stars.contains(&index))
                    {
                        cursor += 1;
                        continue;
                    }
                }
                if character == '*' {
                    if let Some(first) = source {
                        if paired_firsts.contains(&first) {
                            flush_quote_segment(
                                &mut rendered,
                                &mut segment,
                                active,
                                styler,
                            );
                            active = !active;
                            cursor += 1;
                            if cursor < chars.len()
                                && chars[cursor] == '*'
                                && sources.get(cursor).copied().flatten()
                                    == Some(first + 1)
                            {
                                cursor += 1;
                            } else {
                                consume_second = true;
                            }
                            continue;
                        }
                    }
                }
                segment.push(character);
                cursor += 1;
            }
            flush_quote_segment(&mut rendered, &mut segment, active, styler);
            rendered
        })
        .collect()
}

fn flush_quote_segment(
    rendered: &mut String,
    segment: &mut String,
    active: bool,
    styler: Styler,
) {
    if segment.is_empty() {
        return;
    }
    let taken = std::mem::take(segment);
    if !active {
        rendered.push_str(&taken);
        return;
    }
    let inner = taken.trim();
    if inner.is_empty() {
        rendered.push_str(&taken);
        return;
    }
    let prefix_len = taken.len() - taken.trim_start().len();
    let suffix_len = taken.len() - taken.trim_end().len();
    rendered.push_str(&taken[..prefix_len]);
    rendered.push_str(
        &styler.underline_ink(&taken[prefix_len..taken.len() - suffix_len]),
    );
    rendered.push_str(&taken[taken.len() - suffix_len..]);
}

fn image_note_lines(
    annotation: &ShowAnnotation,
    styler: Styler,
    width: usize,
) -> Vec<String> {
    piece_image_note_lines(
        &annotation.images,
        &annotation.unresolved_images,
        &annotation.unresolved_image_states,
        styler,
        width,
    )
}

fn piece_image_note_lines(
    images: &[String],
    unresolved_images: &[String],
    unresolved_image_states: &[UnresolvedImageNoteState],
    styler: Styler,
    width: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    for path in images {
        out.push(styler.dim(&truncate(&format!("    [image: {path}]"), width)));
    }
    for (index, source) in unresolved_images.iter().enumerate() {
        let label = match unresolved_image_states.get(index) {
            Some(UnresolvedImageNoteState::Unreadable) => "image unreadable",
            Some(UnresolvedImageNoteState::Unsupported) => "image unsupported",
            Some(UnresolvedImageNoteState::NotFound) | None => {
                "image not found"
            }
        };
        out.push(
            styler
                .yellow(&truncate(&format!("    [{label}: {source}]"), width)),
        );
    }
    out
}

/// The dim seam before each follower piece in human output, at the quote
/// indent: `┄ <page label>`, or a bare `┄` without a label.
fn piece_seam_human(
    page_label: Option<&str>,
    styler: Styler,
    width: usize,
) -> String {
    let plain = match page_label.filter(|label| !label.is_empty()) {
        Some(label) => format!("    ┄ {label}"),
        None => "    ┄".to_string(),
    };
    styler.dim(&truncate(&plain, width))
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
/// comment for highlights and images, or the bare note text. A tucked `<`
/// group prints each piece in order with a dim seam before every follower.
fn annotation_lines(
    annotation: &ShowAnnotation,
    styler: Styler,
    width: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    match annotation.kind {
        ShowAnnotationKind::Highlight => {
            if !annotation.pieces.is_empty() {
                for (index, piece) in annotation.pieces.iter().enumerate() {
                    if index > 0 {
                        out.push(piece_seam_human(
                            piece.page_label.as_deref(),
                            styler,
                            width,
                        ));
                    }
                    if !piece.quote.is_empty() {
                        out.extend(quote_lines(&piece.quote, styler, width));
                    }
                    out.extend(piece_image_note_lines(
                        &piece.images,
                        &piece.unresolved_images,
                        &piece.unresolved_image_states,
                        styler,
                        width,
                    ));
                    if let Some(comment) = piece.comment.as_deref()
                        && !comment.is_empty()
                    {
                        out.extend(comment_lines(comment, styler, width));
                    }
                }
                return out;
            }
            if let Some(quote) = annotation.quote.as_deref()
                && !quote.is_empty()
            {
                out.extend(quote_lines(quote, styler, width));
            }
            out.extend(image_note_lines(annotation, styler, width));
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
            out.extend(image_note_lines(annotation, styler, width));
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
            out.extend(image_note_lines(annotation, styler, width));
            if let Some(comment) = annotation.comment.as_deref()
                && !comment.is_empty()
            {
                out.extend(comment_lines(comment, styler, width));
            }
        }
    }
    out
}

/// Lane label by mark for the reading-task line.
fn lane_label(mark: char) -> String {
    match mark {
        ' ' => "Ready".to_string(),
        '*' => "⭐ Next".to_string(),
        '/' => "▸ In Progress".to_string(),
        '?' => "Blocked".to_string(),
        other => format!("[{other}]"),
    }
}

/// The reading-task line for human and Markdown, or `None` when there is
/// no task and no ambiguity to report.
fn reading_task_plain(row: &ShowRow) -> Option<String> {
    // Ambiguity first: task is null but the diagnostic names the count.
    if let Some(diag) = row
        .row
        .diagnostics
        .iter()
        .find(|d| d.code == "multiple_open_ref_tasks")
    {
        let count = diag
            .detail
            .split_whitespace()
            .next()
            .unwrap_or("2")
            .to_string();
        return Some(format!(
            "📖 Reading task  ⚠ {count} open tasks claim this ref · run bob ref doctor",
        ));
    }
    let task = row.row.task.as_ref()?;
    let is_v1 =
        task.block_id.as_deref() == Some("ref") && task.path == row.row.path;
    if is_v1 {
        return Some(format!(
            "📖 Reading task  in this note (v1) · {}",
            lane_label(task.mark),
        ));
    }
    let closed = matches!(task.mark, 'x' | 'X' | '-');
    if closed {
        let verb = if task.mark == '-' { "Dropped" } else { "Read" };
        let mut line = format!("📖 {verb}");
        if let Some(date) = row.row.finished.as_deref() {
            line.push_str(&format!(" · {date}"));
        }
        if let Some(parent) = row.row.parent.as_deref() {
            line.push_str(&format!(" · {parent}"));
        }
        if task.archived {
            line.push_str(" (archived)");
        }
        return Some(line);
    }
    // Open v2.
    let residence = row.row.parent.as_deref().unwrap_or(&task.path);
    Some(format!(
        "📖 Reading task  {residence} · {} → {}",
        lane_label(task.mark),
        task.link,
    ))
}

/// The styled reading-task line: the link dims so piped output stays plain.
fn reading_task_styled(
    row: &ShowRow,
    styler: Styler,
    width: usize,
) -> Option<String> {
    let plain = reading_task_plain(row)?;
    // Dim only the `[[...]]` link on open-v2 lines.
    if let Some(task) = row.row.task.as_ref()
        && task.block_id.as_deref() != Some("ref")
        && !matches!(task.mark, 'x' | 'X' | '-')
        && row
            .row
            .diagnostics
            .iter()
            .all(|d| d.code != "multiple_open_ref_tasks")
    {
        let styled_link = styler.dim(&task.link);
        let with_style = plain.replacen(&task.link, &styled_link, 1);
        return Some(truncate(&with_style, width));
    }
    Some(truncate(&plain, width))
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
    if let Some(line) = reading_task_styled(row, styler, width) {
        out.push_str(&line);
        out.push('\n');
    }
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
            // Elsewhere follow-ups carry a dim ` · <path>` suffix.
            let page = row
                .annotations
                .iter()
                .find(|annotation| annotation.block_id == task.block_id)
                .and_then(|annotation| annotation.page_label.as_deref());
            let mut suffix =
                page.map(|page| format!("   {page}")).unwrap_or_default();
            if let Some(path) = task.path.as_deref() {
                suffix.push_str(&format!(" · {path}"));
            }
            // Only dim the path part; keep page suffix behavior.
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
    if let Some(task_line) = reading_task_plain(row) {
        out.push_str(&format!("- **Reading task:** {task_line}\n"));
    }
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
                    if !annotation.pieces.is_empty() {
                        for (index, piece) in
                            annotation.pieces.iter().enumerate()
                        {
                            if index > 0 {
                                out.push('\n');
                                out.push_str(&format!(
                                    "{}\n",
                                    piece_seam_markdown(
                                        piece.page_label.as_deref()
                                    )
                                ));
                            }
                            push_markdown_piece_quote(&mut out, &piece.quote);
                            push_markdown_piece_image_notes(&mut out, piece);
                            push_markdown_piece_comment(
                                &mut out,
                                piece.comment.as_deref(),
                            );
                        }
                    } else {
                        if let Some(quote) = annotation.quote.as_deref()
                            && !quote.is_empty()
                        {
                            out.push('\n');
                            for line in quote.lines() {
                                out.push_str(&format!("> {line}\n"));
                            }
                        }
                        push_markdown_image_notes(&mut out, annotation);
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
                    push_markdown_image_notes(&mut out, annotation);
                }
                ShowAnnotationKind::Image => {
                    if let Some(asset) = annotation.asset.as_deref() {
                        out.push_str(&format!("\nImage: {asset}\n"));
                    }
                    push_markdown_image_notes(&mut out, annotation);
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

fn push_markdown_image_notes(out: &mut String, annotation: &ShowAnnotation) {
    for path in &annotation.images {
        out.push_str(&format!("\nImage: {path}\n"));
    }
    for (index, source) in annotation.unresolved_images.iter().enumerate() {
        let label = match annotation.unresolved_image_states.get(index) {
            Some(UnresolvedImageNoteState::Unreadable) => "Image unreadable",
            Some(UnresolvedImageNoteState::Unsupported) => "Image unsupported",
            Some(UnresolvedImageNoteState::NotFound) | None => {
                "Image not found"
            }
        };
        out.push_str(&format!("\n{label}: {source}\n"));
    }
}

/// The italic seam before each follower piece in Markdown output.
fn piece_seam_markdown(page_label: Option<&str>) -> String {
    match page_label.filter(|label| !label.is_empty()) {
        Some(label) => format!("*┄ {label}*"),
        None => "*┄*".to_string(),
    }
}

fn push_markdown_piece_quote(out: &mut String, quote: &str) {
    if quote.is_empty() {
        return;
    }
    out.push('\n');
    for line in quote.lines() {
        out.push_str(&format!("> {line}\n"));
    }
}

fn push_markdown_piece_image_notes(out: &mut String, piece: &ShowPiece) {
    for path in &piece.images {
        out.push_str(&format!("\nImage: {path}\n"));
    }
    for (index, source) in piece.unresolved_images.iter().enumerate() {
        let label = match piece.unresolved_image_states.get(index) {
            Some(UnresolvedImageNoteState::Unreadable) => "Image unreadable",
            Some(UnresolvedImageNoteState::Unsupported) => "Image unsupported",
            Some(UnresolvedImageNoteState::NotFound) | None => {
                "Image not found"
            }
        };
        out.push_str(&format!("\n{label}: {source}\n"));
    }
}

fn push_markdown_piece_comment(out: &mut String, comment: Option<&str>) {
    if let Some(comment) = comment
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
                marker_id: None,
                origin: "external".to_string(),
                ref_type: Some("papers".to_string()),
                era: "modern".to_string(),
                status: Some("read".to_string()),
                status_sync: "ok".to_string(),
                frontmatter_status: None,
                legacy_status: None,
                blocked: false,
                blocked_by: Vec::new(),
                reading_state: "finished".to_string(),
                reading_state_source: "ref_task:[x]".to_string(),
                parent: Some("sase_ref".to_string()),
                task: None,
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
                added_at: None,
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
                source_id: None,
                source_blocks: Vec::new(),
                v2: false,
                task_line: None,
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
                    images: Vec::new(),
                    unresolved_images: Vec::new(),
                    unresolved_image_states: Vec::new(),
                    pieces: Vec::new(),
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
                    images: Vec::new(),
                    unresolved_images: Vec::new(),
                    unresolved_image_states: Vec::new(),
                    pieces: Vec::new(),
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
                path: None,
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
    fn quote_emphasis_is_underlined_bold_in_color_output() {
        let quote = "before **bold words** after **unclosed";
        let wrapped = wrap_text_with_source(quote, 200);
        let plain = styler_quote_lines(quote, &wrapped, Styler::plain());
        assert_eq!(plain, vec![quote]);
        let colored = styler_quote_lines(quote, &wrapped, Styler::colored());
        assert_eq!(
            colored,
            vec!["before \u{1b}[1;4mbold words\u{1b}[0m after **unclosed"]
        );
    }

    #[test]
    fn quote_emphasis_reopens_after_wrapping() {
        let quote = "**one two three four**";
        let wrapped = wrap_text_with_source(quote, 7);
        let colored = styler_quote_lines(quote, &wrapped, Styler::colored());
        assert!(colored.len() > 1, "{colored:?}");
        assert!(colored[0].contains("\u{1b}[1;4m"), "{colored:?}");
        assert!(colored[0].ends_with("\u{1b}[0m"), "{colored:?}");
        assert!(colored[1].starts_with("\u{1b}[1;4m"), "{colored:?}");
        assert!(colored
            .last()
            .is_some_and(|line| line.ends_with("\u{1b}[0m")));
    }

    #[test]
    fn quote_emphasis_keeps_edge_whitespace_unpainted() {
        let quote = "**  one two three four  **";
        let wrapped = wrap_text_with_source(quote, 9);
        let colored = styler_quote_lines(quote, &wrapped, Styler::colored());
        assert!(colored.len() > 1, "{colored:?}");
        for line in &colored {
            assert_painted_inner_has_no_edge_whitespace(line);
        }
        let one_line = styler_quote_lines(
            quote,
            &wrap_text_with_source(quote, 200),
            Styler::colored(),
        );
        assert_eq!(one_line.len(), 1, "{one_line:?}");
        assert_painted_inner_has_no_edge_whitespace(&one_line[0]);
        assert!(
            one_line[0].contains(" \u{1b}[1;4m")
                || one_line[0].starts_with("\u{1b}[1;4m"),
            "leading edge sits outside or at paint start: {one_line:?}"
        );
        assert!(
            one_line[0].contains("\u{1b}[0m ")
                || one_line[0].ends_with("\u{1b}[0m"),
            "trailing edge sits outside or at paint end: {one_line:?}"
        );
    }

    fn assert_painted_inner_has_no_edge_whitespace(line: &str) {
        let mut rest = line;
        while let Some(start) = rest.find("\u{1b}[1;4m") {
            rest = &rest[start + "\u{1b}[1;4m".len()..];
            let end = rest
                .find("\u{1b}[0m")
                .unwrap_or_else(|| panic!("unclosed paint in {line}"));
            let inner = &rest[..end];
            assert_eq!(
                inner,
                inner.trim(),
                "edge whitespace inside paint: {line}"
            );
            rest = &rest[end + "\u{1b}[0m".len()..];
        }
    }

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(character) = chars.next() {
            if character == '\u{1b}' && chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(character);
            }
        }
        out
    }

    fn quote_payload(lines: &[String]) -> Vec<String> {
        let last = lines.len().saturating_sub(1);
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let mut payload = line.as_str();
                payload = if index == 0 {
                    payload.strip_prefix("    “").expect("opening quote prefix")
                } else {
                    payload.strip_prefix("     ").expect("hanging indent")
                };
                if index == last {
                    payload = payload
                        .strip_suffix('”')
                        .expect("closing quote suffix");
                }
                payload.to_string()
            })
            .collect()
    }

    #[test]
    fn quote_lines_preserve_split_emphasis_delimiters() {
        let quote = format!("**{}** plain after", "a".repeat(72));
        let plain = quote_lines(&quote, Styler::plain(), 80);
        let colored = quote_lines(&quote, Styler::colored(), 80);
        let wrapped = wrap_text(&quote, 75);
        assert_eq!(quote_payload(&plain), wrapped);
        assert!(
            wrapped
                .iter()
                .any(|line| line.contains('*') && !line.contains("**")),
            "closing delimiter should split: {wrapped:?}"
        );

        let colored_payload = quote_payload(&colored);
        let stripped: String = colored_payload
            .iter()
            .map(|line| strip_ansi(line))
            .collect();
        assert_eq!(stripped, "a".repeat(72) + " plain after");
        assert!(
            colored.iter().all(|line| !strip_ansi(line).contains("**")),
            "{colored:?}"
        );
        assert_eq!(strip_ansi(&colored_payload[0]), "a".repeat(72));
        assert!(
            colored_payload[0].starts_with("\u{1b}[1;4m")
                && colored_payload[0].ends_with("\u{1b}[0m"),
            "{colored_payload:?}"
        );
        assert!(
            !colored_payload[1].contains("\u{1b}[1;4m"),
            "plain text after split close must not stay bold: {colored_payload:?}"
        );
        assert_eq!(strip_ansi(&colored_payload[1]), " plain after");
    }

    #[test]
    fn quote_lines_consume_split_opening_and_later_runs() {
        let opening = format!("{}**{}** after", "a".repeat(74), "b".repeat(5));
        let colored = quote_lines(&opening, Styler::colored(), 80);
        let plain = quote_lines(&opening, Styler::plain(), 80);
        assert_eq!(quote_payload(&plain), wrap_text(&opening, 75));
        let payloads = quote_payload(&colored);
        let stripped: String =
            payloads.iter().map(|line| strip_ansi(line)).collect();
        assert_eq!(stripped, "a".repeat(74) + &"b".repeat(5) + " after");
        assert!(
            payloads.iter().any(|line| {
                line.contains("\u{1b}[1;4m") && strip_ansi(line).contains('b')
            }),
            "{colored:?}"
        );
        assert!(
            payloads.iter().any(|line| line.contains("\u{1b}[0m after")),
            "bold must reset before trailing plain text: {colored:?}"
        );

        let two_runs = format!("**{}** plain **z**", "a".repeat(72));
        let colored = quote_lines(&two_runs, Styler::colored(), 80);
        let plain = quote_lines(&two_runs, Styler::plain(), 80);
        assert_eq!(quote_payload(&plain), wrap_text(&two_runs, 75));
        let payloads = quote_payload(&colored);
        let stripped: String =
            payloads.iter().map(|line| strip_ansi(line)).collect();
        assert_eq!(stripped, "a".repeat(72) + " plain z");
        assert!(
            payloads[0].contains("\u{1b}[1;4m")
                && payloads[0].ends_with("\u{1b}[0m"),
            "{payloads:?}"
        );
        assert_eq!(payloads[1], " plain \u{1b}[1;4mz\u{1b}[0m", "{payloads:?}");
    }

    #[test]
    fn quote_lines_keep_emphasis_at_one_content_column() {
        let quote = "**ab** x **c**";
        let plain = quote_lines(quote, Styler::plain(), 1);
        let colored = quote_lines(quote, Styler::colored(), 1);
        assert_eq!(quote_payload(&plain), wrap_text(quote, 1));
        let payloads = quote_payload(&colored);
        let stripped: String =
            payloads.iter().map(|line| strip_ansi(line)).collect();
        assert_eq!(stripped, "abxc");
        let joined = colored.join("\n");
        assert!(joined.contains("\u{1b}[1;4ma\u{1b}[0m"), "{colored:?}");
        assert!(joined.contains("\u{1b}[1;4mb\u{1b}[0m"), "{colored:?}");
        assert!(joined.contains("\u{1b}[1;4mc\u{1b}[0m"), "{colored:?}");
        assert!(
            payloads
                .iter()
                .any(|line| strip_ansi(line) == "x"
                    && !line.contains("\u{1b}[1;4m")),
            "{colored:?}"
        );
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
