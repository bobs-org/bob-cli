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
    /// Accepted comment (`# Name` first-line) declaration orders, in sidecar
    /// order. Sticky semantics (`declaration_orders`, stubs, task mapping)
    /// never see these.
    pub(super) anchor_orders: Vec<usize>,
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
    /// Accepted `# Name` first-line comment declarations (highlights,
    /// absorbed/standalone underlines, images). Followers never declare.
    pub(super) comment_declarations: BTreeSet<usize>,
    /// Rejected comment declarations (empty or reserved titles).
    pub(super) comment_rejected: BTreeMap<usize, Rejection>,
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
    /// Byte span of the `@token` or `#token`, including tolerated punctuation.
    pub(super) span: Range<usize>,
    pub(super) key: String,
    pub(super) raw: String,
    pub(super) sigil: char,
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
        let (alias, is_hash) = if let Some(alias) = token.strip_prefix('@') {
            (alias, false)
        } else if let Some(alias) = token.strip_prefix('#') {
            (alias, true)
        } else {
            break;
        };
        if !valid_alias(alias) {
            break;
        }
        if is_hash && !valid_hash_alias(alias) {
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

/// Extra `#alias` rules shared with the scanner: more than digits (Obsidian's
/// own tag rule) and never keyed `task`.
fn valid_hash_alias(alias: &str) -> bool {
    if alias.chars().all(|character| character.is_numeric()) {
        return false;
    }
    section_key(alias) != "task"
}

/// Shared `#name` validity beyond the sigil shape: more than digits and never
/// keyed `task`. The end-boundary check lives with the caller, which sees the
/// character after the token.
fn hash_tag_key(tag_name: &str) -> Option<String> {
    if tag_name.chars().all(|character| character.is_numeric()) {
        return None;
    }
    let key = section_key(tag_name);
    if key == "task" {
        return None;
    }
    Some(key)
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
/// tags. Both `@name` and `#name` spellings are scanned; `#name` tokens must
/// additionally end at whitespace or the end of the line, contain more than
/// digits, and never key `task`. Spans are UTF-8 byte ranges into the
/// original text.
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
            let rest = &line[cursor..];
            let next_at = rest.find('@');
            let next_hash = rest.find('#');
            let (relative, sigil) = match (next_at, next_hash) {
                (None, None) => break,
                (Some(at), None) => (at, '@'),
                (None, Some(hash)) => (hash, '#'),
                (Some(at), Some(hash)) => {
                    if at < hash {
                        (at, '@')
                    } else {
                        (hash, '#')
                    }
                }
            };
            let at = cursor + relative;
            // `@` and `#` are one byte, so stepping one byte forward is safe.
            cursor = at + 1;
            if code_ranges.iter().any(|range| range.contains(&at)) {
                continue;
            }
            let Some(hit) = token_at(line, at, sigil) else {
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
            let key = if hit.sigil == '#' {
                match hash_tag_key(tag_name) {
                    Some(key) => key,
                    None => continue,
                }
            } else {
                section_key(tag_name)
            };
            hits.push(SectionTagHit {
                line: line_index + 1,
                span: hit.span,
                key,
                raw: hit.raw,
                sigil: hit.sigil,
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
    sigil: char,
    escaped: bool,
}

fn token_at(line: &str, at: usize, sigil: char) -> Option<RawTagHit> {
    debug_assert!(line[at..].starts_with(sigil));
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
    if sigil == '#' {
        // `#name` must end at whitespace or the end of the line, so nested
        // Obsidian tags (`#area/memory`), `#git:home`, and `#memory)` never
        // group. `@name` keeps today's looser end rule (out of scope).
        if line[end..]
            .chars()
            .next()
            .is_some_and(|character| !character.is_whitespace())
        {
            return None;
        }
        let tag_name =
            line[at + 1..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if hash_tag_key(tag_name).is_none() {
            return None;
        }
    }
    Some(RawTagHit {
        span: at..end,
        raw: line[at..end].to_string(),
        sigil,
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

/// Rewrite consumed positional tags for a selected section. Escaped `@`
/// positional tags are displayed without their escape slash and never match;
/// escaped `#` tags render exactly as typed (backslash included), which
/// Obsidian shows as plain `#name` text without creating a tag.
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
                SectionTagHitKind::Escaped if hit.sigil == '@' => {
                    if hit.span.start > 0
                        && line.as_bytes()[hit.span.start - 1] == b'\\'
                    {
                        edits.push((hit.span.start - 1..hit.span.start, ""));
                    }
                }
                SectionTagHitKind::Escaped => {
                    // `\#name` stays verbatim: stripping the slash would turn
                    // it into a real Obsidian tag and change notes with no
                    // sections.
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
/// occurrences) and every scanner-shaped `#name` token (escaped or not),
/// together with the whitespace immediately before it.
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
        let rest = &text[search..];
        let next_at = rest.find('@');
        let next_hash = rest.find('#');
        let (relative, sigil) = match (next_at, next_hash) {
            (None, None) => break,
            (Some(at), None) => (at, '@'),
            (None, Some(hash)) => (hash, '#'),
            (Some(at), Some(hash)) => {
                if at < hash {
                    (at, '@')
                } else {
                    (hash, '#')
                }
            }
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
        if sigil == '#' && !hash_identity_token(text, cursor, at, end) {
            // `C#`, URL fragments, wikilink headings, nested tags, `#task`,
            // and digit-only tags stay part of every key.
            search = at + 1;
            continue;
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

/// Shared `#name` identity predicate mirroring the scanner: the sigil (or its
/// escape slash) must start the text or follow whitespace, the token must end
/// at whitespace or the end of the text, and the name must be more than digits
/// and never key `task`.
fn hash_identity_token(
    text: &str,
    cursor: usize,
    at: usize,
    end: usize,
) -> bool {
    let escaped = at > cursor && text.as_bytes()[at - 1] == b'\\';
    let anchor = if escaped { at - 1 } else { at };
    if anchor > cursor
        && !text[..anchor]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    {
        return false;
    }
    if text[end..]
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return false;
    }
    let tag_name =
        text[at + 1..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
    hash_tag_key(tag_name).is_some()
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

    enum DeclarationKind {
        Sticky,
        Comment,
    }

    struct Candidate<'a> {
        annotation: &'a SidecarAnnotation,
        heading: &'a str,
        kind: DeclarationKind,
    }

    let mut candidates = Vec::<Candidate<'_>>::new();
    for annotation in annotations {
        if let Some(heading) = annotation.section_heading.as_deref() {
            candidates.push(Candidate {
                annotation,
                heading,
                kind: DeclarationKind::Sticky,
            });
        } else if let Some(heading) =
            annotation.comment_section_heading.as_deref()
        {
            // Continuation followers (`<`) are never declaration sites; at
            // most the head of a continuation group declares. This covers
            // merged followers and dangling `<` markers alike.
            if folds.continuations.head_of.contains_key(&annotation.order) {
                continue;
            }
            if let Some(source) = annotation.task_source.as_deref()
                && matches!(
                    scan_continuation_marker(source),
                    MarkerScan::Marker { .. }
                )
            {
                continue;
            }
            candidates.push(Candidate {
                annotation,
                heading,
                kind: DeclarationKind::Comment,
            });
        }
    }

    for candidate in candidates {
        let annotation = candidate.annotation;
        let heading = candidate.heading;
        let is_sticky = matches!(candidate.kind, DeclarationKind::Sticky);
        if is_sticky {
            plan.declarations.insert(annotation.order);
        }
        match scan_section_declaration(heading) {
            Ok(declaration) => {
                if let Some(index) = plan.sections.iter().position(|section| {
                    !section.keys.is_disjoint(&declaration.keys)
                }) {
                    let section = &mut plan.sections[index];
                    if is_sticky {
                        section.declaration_orders.push(annotation.order);
                    } else {
                        section.anchor_orders.push(annotation.order);
                    }
                    if is_sticky && !annotation.text.trim().is_empty() {
                        if !section.description.trim().is_empty() {
                            section.description.push_str("\n\n");
                        }
                        section.description.push_str(annotation.text.trim());
                    }
                    section.keys.extend(declaration.keys);
                    plan.diagnostics.push(SectionDiagnostic {
                        kind: SectionDiagnosticKind::Duplicate,
                        page_label: None,
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
                    if !is_sticky {
                        plan.comment_declarations.insert(annotation.order);
                    }
                } else {
                    let base = section_slug(
                        &declaration.title,
                        &declaration.aliases,
                        &declaration.primary_key,
                        source_pdf_value,
                    );
                    let slug = unique_slug(&base, &mut slugs);
                    let description = if is_sticky {
                        annotation.text.trim().to_string()
                    } else {
                        String::new()
                    };
                    let (declaration_orders, anchor_orders) = if is_sticky {
                        (vec![annotation.order], Vec::new())
                    } else {
                        (Vec::new(), vec![annotation.order])
                    };
                    plan.sections.push(PlannedSection {
                        title: declaration.title,
                        tag: declaration.tag,
                        block_id: format!("s-{slug}"),
                        slug,
                        keys: declaration.keys,
                        declaration_orders,
                        anchor_orders,
                        description,
                        page_label: annotation.page_label.clone(),
                    });
                    if !is_sticky {
                        plan.comment_declarations.insert(annotation.order);
                    }
                }
            }
            Err(rejection) => {
                let (kind, page_label, detail) = match rejection {
                    Rejection::TitleEmpty => (
                        SectionDiagnosticKind::TitleEmpty,
                        annotation.page_label.clone(),
                        format!("\"# {heading}\" has no letters or digits"),
                    ),
                    Rejection::ReservedTitle => (
                        SectionDiagnosticKind::ReservedTitle,
                        None,
                        format!(
                            "\"{}\" ({}) ignored",
                            sanitize_section_title(heading),
                            annotation
                                .page_label
                                .as_deref()
                                .unwrap_or("unknown page"),
                        ),
                    ),
                };
                if is_sticky {
                    plan.rejected.insert(annotation.order, rejection);
                } else {
                    plan.comment_rejected.insert(annotation.order, rejection);
                }
                plan.diagnostics.push(SectionDiagnostic {
                    kind,
                    page_label,
                    detail,
                });
            }
        }
    }

    if plan.sections.is_empty() {
        emit_declaration_in_comment_without_sections(
            &mut plan,
            annotations,
            folds,
        );
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

    let order_to_section = plan
        .sections
        .iter()
        .enumerate()
        .flat_map(|(index, section)| {
            section
                .anchor_orders
                .iter()
                .map(move |order| (*order, index))
        })
        .collect::<BTreeMap<_, _>>();

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
        for (order, annotation, source) in &member_sources {
            if annotation.kind == SidecarAnnotationKind::StandaloneNote {
                continue;
            }
            // Followers never declare, so even a first-line `#` there is
            // literal. Declaration sites skip the first line (the declaration
            // slot or an intentional `\#` escape); only later lines report.
            let skip_first = !folds.continuations.head_of.contains_key(order);
            if let Some(line) =
                first_escaped_heading_after_first_line(source, skip_first)
            {
                plan.diagnostics.push(SectionDiagnostic {
                    kind: SectionDiagnosticKind::DeclarationInComment,
                    page_label: annotation.page_label.clone(),
                    detail: declaration_in_comment_detail(&line),
                });
            }
        }
        // Accepted comment declarations in member order (head, then absorbed
        // underlines; followers never declare). The first is the home; later
        // ones still declare (merged above) and are consumed with a conflict.
        let mut declaration_sections_in_order = Vec::<usize>::new();
        for (order, _, _) in &member_sources {
            if let Some(index) = order_to_section.get(order).copied()
                && !declaration_sections_in_order.contains(&index)
            {
                declaration_sections_in_order.push(index);
            }
        }
        // Tag scanning sees the source after declaration removal, so a
        // declaration's own `@alias`/`#alias` tokens never count as tags.
        let stripped_sources = member_sources
            .iter()
            .map(|(order, annotation, source)| {
                if plan.comment_declarations.contains(order) {
                    (*order, *annotation, remove_first_declaration_line(source))
                } else {
                    (*order, *annotation, source.clone())
                }
            })
            .collect::<Vec<_>>();
        let tag_hits = stripped_sources
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
        let mut tag_home = None;
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
                        if tag_home.is_none() {
                            tag_home = Some(index);
                        }
                    } else {
                        let hint =
                            closest_section_tag(&hit.key, &plan.sections);
                        // `near_miss`: an unknown `#name` reports only as a
                        // likely typo with a did-you-mean hint; far-off
                        // hashtags stay quiet (and keep the scan's
                        // `section_tags_unknown` count quiet). `@name`
                        // behavior is unchanged.
                        if hit.sigil == '#' {
                            let Some(tag) = hint else {
                                continue;
                            };
                            let hint_text = match tag.strip_prefix('@') {
                                Some(rest) => format!("#{rest}"),
                                None => tag,
                            };
                            plan.diagnostics.push(SectionDiagnostic {
                                kind: SectionDiagnosticKind::TagUnknown,
                                page_label: annotation.page_label.clone(),
                                detail: format!(
                                    "{} (did you mean {hint_text}?)",
                                    hit.raw
                                ),
                            });
                        } else {
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
                }
                SectionTagHitKind::Escaped => {}
            }
            let _ = order;
        }
        let declaration_home = declaration_sections_in_order.first().copied();
        // A declaration beats tags.
        let home = declaration_home.or(tag_home);
        if let Some(home_index) = home.filter(|_| !sticky_only_tags) {
            plan.home_of.insert(head_order, home_index);
            let home_section = &plan.sections[home_index];
            // Distinct sections in first-appearance order: declarations in
            // member order first, then tag sections.
            let mut distinct_order = Vec::<usize>::new();
            for index in &declaration_sections_in_order {
                if !distinct_order.contains(index) {
                    distinct_order.push(*index);
                }
            }
            for (_, index, _) in &resolved_tags {
                if !distinct_order.contains(index) {
                    distinct_order.push(*index);
                }
            }
            let others = distinct_order
                .iter()
                .filter(|index| **index != home_index)
                .collect::<Vec<_>>();
            if !others.is_empty() {
                let mut names = vec![home_section.tag.clone()];
                for index in &distinct_order {
                    if *index == home_index {
                        continue;
                    }
                    let label = plan.sections[*index].tag.clone();
                    if !names.contains(&label) {
                        names.push(label);
                    }
                }
                let page_label = resolved_tags
                    .first()
                    .and_then(|(_, _, annotation)| {
                        annotation.page_label.clone()
                    })
                    .or_else(|| {
                        member_sources.first().and_then(|(_, annotation, _)| {
                            annotation.page_label.clone()
                        })
                    });
                plan.diagnostics.push(SectionDiagnostic {
                    kind: SectionDiagnosticKind::Conflict,
                    page_label,
                    detail: format!(
                        "{} (kept {})",
                        names.join(", "),
                        home_section.tag
                    ),
                });
            }
            for (order, annotation, source) in &member_sources {
                let without_decl = if plan.comment_declarations.contains(order)
                {
                    remove_first_declaration_line(source)
                } else {
                    source.clone()
                };
                let rewritten =
                    display_source(&without_decl, &home_section.keys);
                if rewritten != *source {
                    plan.rewritten.insert(*order, rewritten);
                } else {
                    let _ = annotation;
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

fn is_unescaped_section_heading_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix('#') else {
        return false;
    };
    if rest.starts_with('#') {
        return false;
    }
    let Some(first) = rest.chars().next() else {
        return false;
    };
    first.is_whitespace() && !rest.trim().is_empty()
}

fn is_any_section_heading_line(line: &str) -> bool {
    is_unescaped_section_heading_line(line) || is_escaped_section_heading(line)
}

/// Remove the first non-empty `# Name` / `\# Name` line and one blank line
/// after it. Used for accepted comment declarations.
fn remove_first_declaration_line(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut first: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        if !line.trim().is_empty() {
            first = Some(index);
            break;
        }
    }
    let Some(index) = first else {
        return source.to_string();
    };
    let line = lines[index];
    if !is_any_section_heading_line(line) {
        return source.to_string();
    }
    let mut next = index + 1;
    if next < lines.len() && lines[next].trim().is_empty() {
        next += 1;
    }
    lines[next..].join("\n")
}

fn first_escaped_heading_after_first_line(
    source: &str,
    skip_first: bool,
) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut start = 0usize;
    if skip_first {
        let mut found = false;
        for (index, line) in lines.iter().enumerate() {
            if !line.trim().is_empty() {
                start = index + 1;
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    for line in &lines[start..] {
        if is_escaped_section_heading(line) {
            return Some(line.to_string());
        }
    }
    None
}

fn declaration_in_comment_detail(line: &str) -> String {
    format!(
        "\"{}\" is literal; put \"# Name\" on the first line of a comment or sticky note",
        line.trim_start_matches('\\')
    )
}

fn emit_declaration_in_comment_without_sections(
    plan: &mut SectionPlan,
    annotations: &[SidecarAnnotation],
    folds: &AnnotationFolds,
) {
    for annotation in annotations {
        if annotation.kind == SidecarAnnotationKind::StandaloneNote {
            continue;
        }
        if is_sidecar_marker_mirror(annotation) {
            continue;
        }
        let source = if folds.underlines.host_of.contains_key(&annotation.order)
        {
            effective_underline_display_source(annotation)
        } else {
            effective_display_source(annotation, folds)
        };
        // Followers are never declaration sites, so even a first-line `#`
        // there is literal. Everywhere else the first line is the declaration
        // slot (or an intentional `\#` escape), so only later lines report.
        let skip_first =
            !folds.continuations.head_of.contains_key(&annotation.order);
        if let Some(line) =
            first_escaped_heading_after_first_line(&source, skip_first)
        {
            plan.diagnostics.push(SectionDiagnostic {
                kind: SectionDiagnosticKind::DeclarationInComment,
                page_label: annotation.page_label.clone(),
                detail: declaration_in_comment_detail(&line),
            });
        }
    }
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
