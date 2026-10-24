//! Section declaration/tag grammar and the pure membership planner.
//!
//! This module intentionally stops before rendering. It consumes sidecar
//! annotations and the existing continuation/underline fold plans, and
//! returns the homes and source rewrites a later renderer can apply.
use super::*;
use std::ops::Range;

const SECTION_SLUG_LIMIT: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Declaration {
    pub(super) title: String,
    pub(super) aliases: Vec<String>,
    pub(super) primary_key: String,
    pub(super) keys: BTreeSet<String>,
    pub(super) tag: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Rejection {
    TitleEmpty,
    ReservedTitle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlannedSection {
    pub(super) title: String,
    pub(super) tag: String,
    /// Public slug, without the `s-` block-id prefix.
    pub(super) slug: String,
    pub(super) block_id: String,
    pub(super) keys: BTreeSet<String>,
    pub(super) declaration_orders: Vec<usize>,
    pub(super) description: String,
    pub(super) page_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SectionDiagnosticKind {
    Sections,
    TagUnknown,
    Conflict,
    Duplicate,
    ReservedTitle,
    TitleEmpty,
    DeclarationInComment,
    TagOnlyNote,
    TagOnTask,
}

impl SectionDiagnosticKind {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Sections => "sections",
            Self::TagUnknown => "section_tag_unknown",
            Self::Conflict => "section_conflict",
            Self::Duplicate => "section_duplicate",
            Self::ReservedTitle => "section_reserved_title",
            Self::TitleEmpty => "section_title_empty",
            Self::DeclarationInComment => "section_declaration_in_comment",
            Self::TagOnlyNote => "section_tag_only_note",
            Self::TagOnTask => "section_tag_on_task",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SectionDiagnostic {
    pub(super) kind: SectionDiagnosticKind,
    pub(super) page_label: Option<String>,
    pub(super) detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SectionPlan {
    pub(super) sections: Vec<PlannedSection>,
    /// Every syntactic declaration sticky, including rejected declarations.
    pub(super) declarations: BTreeSet<usize>,
    pub(super) rejected: BTreeMap<usize, Rejection>,
    /// Display-head order -> index in `sections`.
    pub(super) home_of: BTreeMap<usize, usize>,
    /// Annotation order -> consumed effective display source.
    pub(super) rewritten: BTreeMap<usize, String>,
    pub(super) diagnostics: Vec<SectionDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SectionTagHitKind {
    Positional,
    Escaped,
    OnTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SectionTagHit {
    /// One-based source line number.
    pub(super) line: usize,
    /// Byte span of the `@token`, including tolerated punctuation.
    pub(super) span: Range<usize>,
    pub(super) key: String,
    pub(super) raw: String,
    pub(super) kind: SectionTagHitKind,
}

/// The matching identity for a section title or alias: lowercase Unicode
/// letters/digits only.
pub(super) fn section_key(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Parse the raw text after the declaration's `# ` prefix.
pub(super) fn scan_section_declaration(
    heading: &str,
) -> std::result::Result<Declaration, Rejection> {
    let (title_source, aliases) = trailing_aliases(heading);
    let title = sanitize_section_title(title_source);
    if !title.chars().any(char::is_alphanumeric) {
        return Err(Rejection::TitleEmpty);
    }
    let title_key = section_key(&title);
    if matches!(title_key.as_str(), "otherhighlights" | "removedhighlights") {
        return Err(Rejection::ReservedTitle);
    }

    let mut keys = BTreeSet::new();
    keys.insert(title_key.clone());
    keys.extend(aliases.iter().map(|alias| section_key(alias)));
    let tag = canonical_section_tag(&title, &aliases);
    Ok(Declaration {
        title,
        aliases,
        primary_key: title_key,
        keys,
        tag,
    })
}

fn trailing_aliases(heading: &str) -> (&str, Vec<String>) {
    let mut end = heading.trim_end().len();
    let mut aliases_reversed = Vec::new();
    loop {
        let start = heading[..end]
            .char_indices()
            .rev()
            .find_map(|(index, character)| {
                character
                    .is_whitespace()
                    .then_some(index + character.len_utf8())
            })
            .unwrap_or(0);
        let token = &heading[start..end];
        let Some(alias) = token.strip_prefix('@') else {
            break;
        };
        if !valid_alias(alias) {
            break;
        }
        aliases_reversed.push(alias.to_string());
        end = if start == 0 {
            0
        } else {
            heading[..start]
                .char_indices()
                .rev()
                .find_map(|(index, character)| {
                    (!character.is_whitespace())
                        .then_some(index + character.len_utf8())
                })
                .unwrap_or(0)
        };
        if end == 0 {
            break;
        }
    }
    aliases_reversed.reverse();
    (heading[..end].trim_end(), aliases_reversed)
}

fn valid_alias(alias: &str) -> bool {
    let mut characters = alias.chars();
    characters.next().is_some_and(char::is_alphanumeric)
        && characters.all(|character| {
            character.is_alphanumeric() || matches!(character, '_' | '-')
        })
}

fn sanitize_section_title(title: &str) -> String {
    let without_markers = title
        .replace("[[", "")
        .replace("]]", "")
        .chars()
        .filter(|character| !matches!(character, '#' | '|' | '^'))
        .collect::<String>();
    without_markers
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn canonical_section_tag(title: &str, aliases: &[String]) -> String {
    if let Some(alias) = aliases.first() {
        return format!(
            "@{}",
            alias
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<String>()
        );
    }
    let mut runs = Vec::<String>::new();
    let mut current = String::new();
    for character in title.chars() {
        if character.is_alphanumeric() {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    format!("@{}", runs.join("-"))
}

fn section_slug(
    title: &str,
    aliases: &[String],
    first_key: &str,
    source_pdf_value: &str,
) -> String {
    let source = aliases.first().map(String::as_str).unwrap_or(title);
    let mut runs = Vec::<String>::new();
    let mut current = String::new();
    for character in source.chars() {
        if character.is_ascii_alphanumeric() {
            current.push(character.to_ascii_lowercase());
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    if runs.is_empty() {
        return section_hash_slug(source_pdf_value, first_key);
    }

    let mut slug = String::new();
    for run in runs {
        let separator = usize::from(!slug.is_empty());
        if slug.len() + separator + run.len() <= SECTION_SLUG_LIMIT {
            if separator == 1 {
                slug.push('-');
            }
            slug.push_str(&run);
        } else if slug.is_empty() {
            // A single overlong run has no earlier boundary; cap it so the
            // resulting block id still respects the public length limit.
            slug.push_str(&run[..SECTION_SLUG_LIMIT]);
            break;
        } else {
            break;
        }
    }
    slug
}

fn section_hash_slug(source_pdf_value: &str, first_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_pdf_value.as_bytes());
    hasher.update([0]);
    hasher.update(first_key.as_bytes());
    hex::encode(hasher.finalize())[..8].to_string()
}

fn unique_slug(base: &str, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}-{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// Find positional tags, escaped positional tags, and trailing `#task` route
/// tags. Spans are UTF-8 byte ranges into the original text.
pub(super) fn scan_section_tags(text: &str) -> Vec<SectionTagHit> {
    let mut hits = Vec::new();
    let mut fenced: Option<(char, usize)> = None;
    for (line_index, line) in text.lines().enumerate() {
        if let Some((character, width)) = fenced {
            if closes_fence(line, character, width) {
                fenced = None;
            }
            continue;
        }
        if let Some((character, width)) = opens_fence(line) {
            fenced = Some((character, width));
            continue;
        }
        if !parse_image_note_refs(line).is_empty() {
            continue;
        }

        let code_ranges = inline_code_ranges(line);
        let mut line_hits = Vec::new();
        let mut cursor = 0;
        while cursor < line.len() {
            let Some(relative) = line[cursor..].find('@') else {
                break;
            };
            let at = cursor + relative;
            cursor = at + 1;
            if code_ranges.iter().any(|range| range.contains(&at)) {
                continue;
            }
            let Some(hit) = token_at(line, at) else {
                continue;
            };
            line_hits.push(hit);
        }
        if line_hits.is_empty() {
            continue;
        }

        let (region_start, region_end) =
            positional_region(line, line_index == 0);
        let positional =
            positional_hit_indices(line, &line_hits, region_start, region_end);
        let visible = mask_ranges(line, &code_ranges);
        let task_line = contains_markdown_token(&visible, PDF_TASK_TAG);
        let trimmed_end = line.trim_end().len();
        for (index, hit) in line_hits.into_iter().enumerate() {
            let kind = if hit.escaped {
                if !positional.contains(&index) {
                    continue;
                }
                SectionTagHitKind::Escaped
            } else if task_line
                && hit.span.end == trimmed_end
                && strict_annotation_task_route_name(&hit.raw).is_some()
            {
                SectionTagHitKind::OnTask
            } else if positional.contains(&index) && !task_line {
                SectionTagHitKind::Positional
            } else {
                continue;
            };
            let tag_name =
                hit.raw[1..].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            hits.push(SectionTagHit {
                line: line_index + 1,
                span: hit.span,
                key: section_key(tag_name),
                raw: hit.raw,
                kind,
            });
        }
    }
    hits
}

#[derive(Debug)]
struct RawTagHit {
    span: Range<usize>,
    raw: String,
    escaped: bool,
}

fn token_at(line: &str, at: usize) -> Option<RawTagHit> {
    let prefix = &line[..at];
    let escaped = prefix.ends_with('\\');
    if escaped {
        let before_slash = &prefix[..prefix.len() - 1];
        if before_slash
            .chars()
            .next_back()
            .is_some_and(|ch| !ch.is_whitespace())
        {
            return None;
        }
    } else if prefix
        .chars()
        .next_back()
        .is_some_and(|ch| !ch.is_whitespace())
    {
        return None;
    }

    let rest = &line[at + 1..];
    let first = rest.chars().next()?;
    if !first.is_alphanumeric() {
        return None;
    }
    let mut end = at + 1 + first.len_utf8();
    for (offset, character) in line[end..].char_indices() {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            end += character.len_utf8();
        } else {
            let _ = offset;
            break;
        }
    }
    if line[end..].chars().next().is_some_and(|character| {
        matches!(character, '.' | ',' | ';' | ':' | '!' | '?')
    }) {
        end += line[end..]
            .chars()
            .next()
            .expect("checked punctuation")
            .len_utf8();
    }
    Some(RawTagHit {
        span: at..end,
        raw: line[at..end].to_string(),
        escaped,
    })
}

fn positional_region(line: &str, first_line: bool) -> (usize, usize) {
    let mut start = line.len() - line.trim_start().len();
    let end = line.trim_end().len();
    if first_line {
        for label in ["Comment:", "Note:"] {
            if line[start..end]
                .get(..label.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(label))
            {
                start += label.len();
                while line[start..end]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
                {
                    start +=
                        line[start..end].chars().next().unwrap().len_utf8();
                }
                break;
            }
        }
    }
    while line[start..end]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        start += line[start..end].chars().next().unwrap().len_utf8();
    }
    if start + 1 < end
        && matches!(line.as_bytes()[start], b'-' | b'*' | b'+')
        && line[start + 1..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        start += 1;
        while line[start..end]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
        {
            start += line[start..end].chars().next().unwrap().len_utf8();
        }
    }
    (start, end)
}

fn positional_hit_indices(
    line: &str,
    hits: &[RawTagHit],
    start: usize,
    end: usize,
) -> BTreeSet<usize> {
    let mut positional = BTreeSet::new();
    let mut cursor = start;
    for (index, hit) in hits.iter().enumerate() {
        if hit.span.start < start || hit.span.end > end {
            continue;
        }
        let token_start = hit.span.start - usize::from(hit.escaped);
        if line[cursor..token_start].chars().all(char::is_whitespace) {
            positional.insert(index);
            cursor = hit.span.end;
        } else {
            break;
        }
    }
    cursor = end;
    for (index, hit) in hits.iter().enumerate().rev() {
        if hit.span.start < start || hit.span.end > end {
            continue;
        }
        if line[hit.span.end..cursor].chars().all(char::is_whitespace) {
            positional.insert(index);
            cursor = hit.span.start - usize::from(hit.escaped);
        } else {
            break;
        }
    }
    positional
}

fn inline_code_ranges(line: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while cursor < line.len() {
        let Some(relative) = line[cursor..].find('`') else {
            break;
        };
        let start = cursor + relative;
        let width = line[start..].chars().take_while(|ch| *ch == '`').count();
        let opening_end = start + width;
        let mut search = opening_end;
        let mut closing = None;
        while search < line.len() {
            let Some(relative) = line[search..].find('`') else {
                break;
            };
            let tick = search + relative;
            let run = line[tick..].chars().take_while(|ch| *ch == '`').count();
            if run == width {
                closing = Some(tick + run);
                break;
            }
            search = tick + run;
        }
        if let Some(end) = closing {
            ranges.push(start..end);
            cursor = end;
        } else {
            cursor = opening_end;
        }
    }
    ranges
}

fn mask_ranges(line: &str, ranges: &[Range<usize>]) -> String {
    let mut masked = String::with_capacity(line.len());
    let mut cursor = 0;
    for range in ranges {
        masked.push_str(&line[cursor..range.start]);
        masked.extend(line[range.clone()].chars().map(|_| ' '));
        cursor = range.end;
    }
    masked.push_str(&line[cursor..]);
    masked
}

fn opens_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let character = trimmed.chars().next()?;
    if !matches!(character, '`' | '~') {
        return None;
    }
    let width = trimmed.chars().take_while(|ch| *ch == character).count();
    (width >= 3).then_some((character, width))
}

fn closes_fence(line: &str, character: char, width: usize) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let run = trimmed.chars().take_while(|ch| *ch == character).count();
    run >= width && trimmed[run..].trim().is_empty()
}

/// Rewrite consumed positional tags for a selected section. Escaped
/// positional tags are displayed without their escape slash and never match.
pub(super) fn display_source(
    text: &str,
    home_keys: &BTreeSet<String>,
) -> String {
    let hits = scan_section_tags(text);
    let mut by_line = BTreeMap::<usize, Vec<&SectionTagHit>>::new();
    for hit in &hits {
        by_line.entry(hit.line).or_default().push(hit);
    }
    let mut output = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let Some(line_hits) = by_line.get(&line_number) else {
            output.push(line.to_string());
            continue;
        };
        let mut edits = Vec::<(Range<usize>, &'static str)>::new();
        for hit in line_hits {
            match hit.kind {
                SectionTagHitKind::Positional
                    if home_keys.contains(&hit.key) =>
                {
                    edits.push((hit.span.clone(), ""));
                }
                SectionTagHitKind::Escaped => {
                    if hit.span.start > 0
                        && line.as_bytes()[hit.span.start - 1] == b'\\'
                    {
                        edits.push((hit.span.start - 1..hit.span.start, ""));
                    }
                }
                _ => {}
            }
        }
        if edits.is_empty() {
            output.push(line.to_string());
            continue;
        }
        edits.sort_by_key(|(range, _)| range.start);
        let mut rewritten = String::with_capacity(line.len());
        let mut cursor = 0;
        for (range, replacement) in edits {
            if range.start < cursor {
                continue;
            }
            rewritten.push_str(&line[cursor..range.start]);
            rewritten.push_str(replacement);
            cursor = range.end;
        }
        rewritten.push_str(&line[cursor..]);
        let rewritten = collapse_display_whitespace(&rewritten);
        if rewritten.is_empty()
            || is_empty_list_item(&rewritten)
            || is_empty_comment_label(&rewritten, line_index == 0)
        {
            continue;
        }
        output.push(rewritten);
    }
    output.join("\n")
}

fn collapse_display_whitespace(line: &str) -> String {
    let code_ranges = inline_code_ranges(line);
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    let mut pending_space = false;
    let mut range_index = 0;
    while cursor < line.len() {
        if let Some(range) = code_ranges.get(range_index)
            && range.start == cursor
        {
            if pending_space && !output.is_empty() {
                output.push(' ');
            }
            output.push_str(&line[range.clone()]);
            cursor = range.end;
            range_index += 1;
            pending_space = false;
            continue;
        }
        let character = line[cursor..].chars().next().unwrap();
        cursor += character.len_utf8();
        if character.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !output.is_empty() {
                output.push(' ');
            }
            output.push(character);
            pending_space = false;
        }
    }
    output
}

/// Stable identity normalizer shared by current and existing sidecar blocks.
/// It removes every `@name`-shaped token (escaped or not, including prose
/// occurrences), together with the whitespace immediately before it.
pub(super) fn section_match_key(text: &str) -> String {
    let normalized = normalized_identity_text(&strip_identity_tags(text));
    if normalized.is_empty() {
        normalized_identity_text(text)
    } else {
        normalized
    }
}

fn strip_identity_tags(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut search = 0;
    while search < text.len() {
        let Some(relative) = text[search..].find('@') else {
            break;
        };
        let at = search + relative;
        let Some(first) = text[at + 1..].chars().next() else {
            break;
        };
        if !first.is_alphanumeric() {
            search = at + 1;
            continue;
        }
        let mut end = at + 1 + first.len_utf8();
        while let Some(character) = text[end..].chars().next() {
            if character.is_alphanumeric() || matches!(character, '_' | '-') {
                end += character.len_utf8();
            } else {
                break;
            }
        }
        if text[end..].chars().next().is_some_and(|character| {
            matches!(character, '.' | ',' | ';' | ':' | '!' | '?')
        }) {
            end += text[end..].chars().next().unwrap().len_utf8();
        }
        let mut start = if at > cursor && text.as_bytes()[at - 1] == b'\\' {
            at - 1
        } else {
            at
        };
        while start > cursor {
            let previous = text[..start].chars().next_back().unwrap();
            if !previous.is_whitespace() {
                break;
            }
            start -= previous.len_utf8();
        }
        output.push_str(&text[cursor..start]);
        cursor = end;
        search = end;
    }
    output.push_str(&text[cursor..]);
    output
}

fn is_empty_list_item(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() == 1 && matches!(trimmed, "-" | "*" | "+")
}

fn is_empty_comment_label(line: &str, first_line: bool) -> bool {
    if !first_line {
        return false;
    }
    matches!(line.trim(), "Comment:" | "comment:" | "Note:" | "note:")
}

fn effective_display_source(
    annotation: &SidecarAnnotation,
    folds: &AnnotationFolds,
) -> String {
    match annotation.kind {
        SidecarAnnotationKind::StandaloneNote => annotation.text.clone(),
        SidecarAnnotationKind::Highlight => folds
            .continuations
            .effective_annotation(annotation)
            .task_source
            .as_deref()
            .or(annotation.comment.as_deref())
            .unwrap_or("")
            .to_string(),
        SidecarAnnotationKind::Image => annotation
            .task_source
            .as_deref()
            .or(annotation.comment.as_deref())
            .unwrap_or("")
            .to_string(),
    }
}

fn effective_underline_display_source(
    annotation: &SidecarAnnotation,
) -> String {
    let source = annotation
        .task_source
        .as_deref()
        .or(annotation.comment.as_deref())
        .unwrap_or("");
    match scan_continuation_marker(source) {
        MarkerScan::Escaped { task_source } => task_source,
        _ => source.to_string(),
    }
}

/// Plan section declarations, one home per displayed block, and the exact
/// per-annotation display source after consuming tags for that home.
pub(super) fn plan_sections(
    annotations: &[SidecarAnnotation],
    folds: &AnnotationFolds,
    source_pdf_value: &str,
) -> SectionPlan {
    let mut plan = SectionPlan::default();
    let mut slugs = BTreeSet::new();

    for annotation in annotations {
        let Some(heading) = annotation.section_heading.as_deref() else {
            continue;
        };
        plan.declarations.insert(annotation.order);
        match scan_section_declaration(heading) {
            Ok(declaration) => {
                if let Some(index) = plan.sections.iter().position(|section| {
                    !section.keys.is_disjoint(&declaration.keys)
                }) {
                    let section = &mut plan.sections[index];
                    section.declaration_orders.push(annotation.order);
                    if !annotation.text.trim().is_empty() {
                        if !section.description.trim().is_empty() {
                            section.description.push_str("\n\n");
                        }
                        section.description.push_str(annotation.text.trim());
                    }
                    section.keys.extend(declaration.keys);
                    plan.diagnostics.push(SectionDiagnostic {
                        kind: SectionDiagnosticKind::Duplicate,
                        page_label: annotation.page_label.clone(),
                        detail: format!(
                            "\"{}\" ({}) merged into \"{}\" ({})",
                            declaration.title,
                            annotation
                                .page_label
                                .as_deref()
                                .unwrap_or("unknown page"),
                            section.title,
                            section
                                .page_label
                                .as_deref()
                                .unwrap_or("unknown page")
                        ),
                    });
                } else {
                    let base = section_slug(
                        &declaration.title,
                        &declaration.aliases,
                        &declaration.primary_key,
                        source_pdf_value,
                    );
                    let slug = unique_slug(&base, &mut slugs);
                    let description = annotation.text.trim().to_string();
                    plan.sections.push(PlannedSection {
                        title: declaration.title,
                        tag: declaration.tag,
                        block_id: format!("s-{slug}"),
                        slug,
                        keys: declaration.keys,
                        declaration_orders: vec![annotation.order],
                        description,
                        page_label: annotation.page_label.clone(),
                    });
                }
            }
            Err(rejection) => {
                let (kind, detail) = match rejection {
                    Rejection::TitleEmpty => (
                        SectionDiagnosticKind::TitleEmpty,
                        format!("\"# {heading}\" has no letters or digits"),
                    ),
                    Rejection::ReservedTitle => (
                        SectionDiagnosticKind::ReservedTitle,
                        format!(
                            "\"{}\" ignored",
                            sanitize_section_title(heading)
                        ),
                    ),
                };
                plan.rejected.insert(annotation.order, rejection);
                plan.diagnostics.push(SectionDiagnostic {
                    kind,
                    page_label: annotation.page_label.clone(),
                    detail,
                });
            }
        }
    }

    if plan.sections.is_empty() {
        return plan;
    }

    let key_to_section = plan
        .sections
        .iter()
        .enumerate()
        .flat_map(|(index, section)| {
            section.keys.iter().cloned().map(move |key| (key, index))
        })
        .collect::<BTreeMap<_, _>>();
    let annotations_by_order = annotations
        .iter()
        .map(|annotation| (annotation.order, annotation))
        .collect::<BTreeMap<_, _>>();
    let mut blocks = Vec::<(usize, Vec<usize>)>::new();

    for annotation in annotations {
        if is_sidecar_marker_mirror(annotation)
            || plan.declarations.contains(&annotation.order)
            || folds.continuations.head_of.contains_key(&annotation.order)
            || folds.underlines.host_of.contains_key(&annotation.order)
        {
            continue;
        }
        match annotation.kind {
            SidecarAnnotationKind::Image
            | SidecarAnnotationKind::StandaloneNote => {
                blocks.push((annotation.order, vec![annotation.order]));
            }
            SidecarAnnotationKind::Highlight => {
                let mut members = vec![annotation.order];
                if let Some(followers) =
                    folds.continuations.followers.get(&annotation.order)
                {
                    members.extend(followers.iter().map(|(order, _)| *order));
                }
                let member_orders =
                    members.iter().copied().collect::<BTreeSet<_>>();
                let mut underlines = folds
                    .underlines
                    .host_of
                    .iter()
                    .filter_map(|(order, host)| {
                        member_orders.contains(host).then_some(*order)
                    })
                    .collect::<Vec<_>>();
                underlines.sort_unstable();
                members.extend(underlines);
                blocks.push((annotation.order, members));
            }
        }
    }

    for (head_order, member_orders) in blocks {
        let member_sources = member_orders
            .iter()
            .filter_map(|order| {
                annotations_by_order
                    .get(order)
                    .copied()
                    .map(|annotation| (*order, annotation))
            })
            .map(|(order, annotation)| {
                let source = if folds.underlines.host_of.contains_key(&order) {
                    effective_underline_display_source(annotation)
                } else {
                    effective_display_source(annotation, folds)
                };
                (order, annotation, source)
            })
            .collect::<Vec<_>>();
        for (_, annotation, source) in &member_sources {
            if annotation.kind != SidecarAnnotationKind::StandaloneNote
                && let Some(line) =
                    source.lines().find(|line| is_escaped_section_heading(line))
            {
                plan.diagnostics.push(SectionDiagnostic {
                    kind: SectionDiagnosticKind::DeclarationInComment,
                    page_label: annotation.page_label.clone(),
                    detail: format!(
                        "{} is literal; declare sections on a sticky note",
                        line.trim_start_matches('\\')
                    ),
                });
            }
        }
        let tag_hits = member_sources
            .iter()
            .flat_map(|(order, annotation, source)| {
                scan_section_tags(source)
                    .into_iter()
                    .map(move |hit| (*order, *annotation, source.clone(), hit))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let sticky_only_tags = member_orders.len() == 1
            && annotations_by_order.get(&head_order).is_some_and(
                |annotation| {
                    annotation.kind == SidecarAnnotationKind::StandaloneNote
                        && tag_hits.iter().any(|(_, _, _, hit)| {
                            hit.kind == SectionTagHitKind::Positional
                        })
                        && source_is_only_tags(&annotation.text)
                },
            );
        let mut home = None;
        let mut resolved_tags =
            Vec::<(String, usize, &SidecarAnnotation)>::new();
        for (order, annotation, _, hit) in &tag_hits {
            match hit.kind {
                SectionTagHitKind::OnTask => {
                    if let Some(index) = key_to_section.get(&hit.key).copied() {
                        let route = strict_annotation_task_route_name(&hit.raw)
                            .unwrap_or_default();
                        plan.diagnostics.push(SectionDiagnostic {
                            kind: SectionDiagnosticKind::TagOnTask,
                            page_label: annotation.page_label.clone(),
                            detail: format!(
                                "{} routes this #task to {route}.md; put {} on its own line to join {}",
                                hit.raw, plan.sections[index].tag, plan.sections[index].title
                            ),
                        });
                    }
                }
                SectionTagHitKind::Positional => {
                    if let Some(index) = key_to_section.get(&hit.key).copied() {
                        resolved_tags.push((
                            hit.raw.clone(),
                            index,
                            annotation,
                        ));
                        if home.is_none() {
                            home = Some(index);
                        }
                    } else {
                        let hint =
                            closest_section_tag(&hit.key, &plan.sections);
                        let detail = match hint {
                            Some(tag) => {
                                format!("{} (did you mean {tag}?)", hit.raw)
                            }
                            None => hit.raw.clone(),
                        };
                        plan.diagnostics.push(SectionDiagnostic {
                            kind: SectionDiagnosticKind::TagUnknown,
                            page_label: annotation.page_label.clone(),
                            detail,
                        });
                    }
                }
                SectionTagHitKind::Escaped => {}
            }
            let _ = order;
        }
        if let Some(home_index) = home.filter(|_| !sticky_only_tags) {
            plan.home_of.insert(head_order, home_index);
            let home_section = &plan.sections[home_index];
            let distinct = resolved_tags
                .iter()
                .filter(|(_, index, _)| *index != home_index)
                .collect::<Vec<_>>();
            if !distinct.is_empty() {
                let mut names = vec![home_section.tag.clone()];
                for (raw, index, _) in &distinct {
                    let label = plan.sections[*index].tag.clone();
                    if !names.contains(&label) {
                        names.push(label);
                    }
                    let _ = raw;
                }
                plan.diagnostics.push(SectionDiagnostic {
                    kind: SectionDiagnosticKind::Conflict,
                    page_label: resolved_tags.first().and_then(
                        |(_, _, annotation)| annotation.page_label.clone(),
                    ),
                    detail: format!(
                        "{} (kept {})",
                        names.join(", "),
                        home_section.tag
                    ),
                });
            }
            for order in member_orders {
                let Some(annotation) =
                    annotations_by_order.get(&order).copied()
                else {
                    continue;
                };
                let source = if folds.underlines.host_of.contains_key(&order) {
                    effective_underline_display_source(annotation)
                } else {
                    effective_display_source(annotation, folds)
                };
                let rewritten = display_source(&source, &home_section.keys);
                if rewritten != source {
                    plan.rewritten.insert(order, rewritten);
                }
            }
        } else if sticky_only_tags {
            if let Some((_, annotation, _, hit)) =
                tag_hits.iter().find(|(_, _, _, hit)| {
                    hit.kind == SectionTagHitKind::Positional
                        && key_to_section.contains_key(&hit.key)
                })
            {
                let display_tag = key_to_section
                    .get(&hit.key)
                    .map(|index| plan.sections[*index].tag.as_str())
                    .unwrap_or(&hit.raw);
                plan.diagnostics.push(SectionDiagnostic {
                    kind: SectionDiagnosticKind::TagOnlyNote,
                    page_label: annotation.page_label.clone(),
                    detail: format!(
                        "{} (a note needs text to join a section)",
                        display_tag
                    ),
                });
            }
        }
    }

    let summaries = plan
        .sections
        .iter()
        .enumerate()
        .map(|(index, section)| {
            let count =
                plan.home_of.values().filter(|home| **home == index).count();
            format!("{} {count}", section.title)
        })
        .collect::<Vec<_>>();
    plan.diagnostics.push(SectionDiagnostic {
        kind: SectionDiagnosticKind::Sections,
        page_label: None,
        detail: format!("{} ({})", plan.sections.len(), summaries.join(", ")),
    });
    plan
}

fn source_is_only_tags(text: &str) -> bool {
    let hits = scan_section_tags(text)
        .into_iter()
        .filter(|hit| hit.kind == SectionTagHitKind::Positional)
        .collect::<Vec<_>>();
    if hits.is_empty() {
        return false;
    }
    text.lines().enumerate().all(|(line_index, line)| {
        let mut remainder = line.to_string();
        let mut spans = hits
            .iter()
            .filter(|hit| hit.line == line_index + 1)
            .map(|hit| hit.span.clone())
            .collect::<Vec<_>>();
        spans.sort_by_key(|span| std::cmp::Reverse(span.start));
        for span in spans {
            remainder.replace_range(span, "");
        }
        let mut remainder = remainder.trim().to_string();
        if line_index == 0 {
            for label in ["Comment:", "comment:", "Note:", "note:"] {
                if let Some(rest) = remainder.strip_prefix(label) {
                    remainder = rest.trim().to_string();
                    break;
                }
            }
        }
        if let Some(rest) = strip_unordered_list_marker(&remainder) {
            remainder = rest.trim().to_string();
        }
        remainder.is_empty()
    })
}

fn is_escaped_section_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("\\#") else {
        return false;
    };
    let Some(first) = rest.chars().next() else {
        return false;
    };
    first.is_whitespace() && !rest.trim().is_empty()
}

fn closest_section_tag(
    key: &str,
    sections: &[PlannedSection],
) -> Option<String> {
    let mut best: Option<(usize, usize)> = None;
    for (index, section) in sections.iter().enumerate() {
        for candidate in &section.keys {
            let distance = edit_distance(key, candidate);
            if distance <= 2
                && best
                    .is_none_or(|(best_distance, _)| distance < best_distance)
            {
                best = Some((distance, index));
            }
        }
    }
    best.map(|(_, index)| sections[index].tag.clone())
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1; right.len() + 1];
        for (right_index, right_char) in right.iter().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(
                    previous[right_index]
                        + usize::from(left_char != *right_char),
                );
        }
        previous = current;
    }
    *previous.last().unwrap_or(&0)
}
