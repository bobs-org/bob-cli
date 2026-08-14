use std::{
    cmp::Ordering,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;
use serde::Serialize;

use super::{
    capture_language::{CompletionContext, Span, SpanKind},
    is_always_excluded_note_directory_name, markdown,
};

const RESULT_LIMIT: usize = 20;
const WARNING_LIMIT: usize = 8;

static BLOCK_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)\^([A-Za-z0-9_-]+)\s*$").unwrap());

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkCompletionField {
    pub(crate) context: CompletionContext,
    pub(crate) query: String,
    pub(crate) replacement: (usize, usize),
    target: LinkCompletionTarget,
    suffix_close: bool,
    existing_close_end: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkCompletionTarget {
    Note,
    Heading(LinkSearchScope),
    Block(LinkSearchScope),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkSearchScope {
    Target(String),
    Current(String),
    Vault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteIndex {
    notes: Vec<NoteEntry>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoteEntry {
    path: String,
    insert_target: String,
    stem: String,
    aliases: Vec<String>,
    headings: Vec<HeadingEntry>,
    blocks: Vec<BlockEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeadingEntry {
    title: String,
    level: usize,
    ordinal: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockEntry {
    block_id: String,
    preview: Option<String>,
    ordinal: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WikilinkNoteCandidate {
    pub(crate) replacement: String,
    pub(crate) cursor_after: usize,
    pub(crate) path: String,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) alias: Option<String>,
    pub(crate) match_kind: MatchKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WikilinkHeadingCandidate {
    pub(crate) replacement: String,
    pub(crate) cursor_after: usize,
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) heading: String,
    pub(crate) level: usize,
    pub(crate) match_kind: MatchKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WikilinkBlockCandidate {
    pub(crate) replacement: String,
    pub(crate) cursor_after: usize,
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) block_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) preview: Option<String>,
    pub(crate) match_kind: MatchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatchKind {
    Empty,
    ExactAlias,
    ExactStem,
    ExactPath,
    PrefixAlias,
    PrefixStem,
    PrefixPath,
    WordBoundaryAlias,
    WordBoundaryStem,
    WordBoundaryPath,
    AcronymAlias,
    AcronymStem,
    AcronymPath,
    SubstringAlias,
    SubstringStem,
    SubstringPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchScore {
    tier: u8,
    component: u8,
    value: String,
    kind: MatchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Component {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkSyntax {
    start: usize,
    end: usize,
    inner_start: usize,
    inner_end: usize,
    close: Option<(usize, usize)>,
    target: Option<Component>,
    heading: Option<Component>,
    block: Option<Component>,
    alias: Option<Component>,
    delimiters: Vec<Component>,
    scope: Option<SyntaxScope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntaxScope {
    TargetHeading,
    TargetBlock,
    CurrentHeading,
    CurrentBlock,
    VaultHeading,
    VaultBlock,
}

pub(crate) fn wikilink_spans(raw: &str) -> Vec<Span> {
    let code_ranges = code_ranges(raw);
    scan_links(raw, &code_ranges)
        .into_iter()
        .flat_map(|link| link.spans())
        .collect()
}

pub(crate) fn completion_field_at(
    raw: &str,
    cursor: usize,
    current_note_path: String,
) -> Option<LinkCompletionField> {
    let code_ranges = code_ranges(raw);
    scan_links(raw, &code_ranges)
        .into_iter()
        .find_map(|link| link.completion_field(raw, cursor, &current_note_path))
}

impl NoteIndex {
    pub(crate) fn read(bob_dir: &Path) -> Result<Self, String> {
        let mut paths = Vec::new();
        let mut warnings = WarningCollector::default();
        collect_markdown_paths(bob_dir, bob_dir, &mut paths, &mut warnings)
            .map_err(|error| {
                format!("scan vault {}: {error}", bob_dir.display())
            })?;
        paths.sort();

        let mut notes = Vec::new();
        for path in paths {
            notes.push(NoteEntry::read(bob_dir, &path, &mut warnings));
        }
        notes
            .sort_by(|left, right| compare_normalized(&left.path, &right.path));

        Ok(Self {
            notes,
            warnings: warnings.finish(),
        })
    }

    pub(crate) fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
}

pub(crate) fn note_candidates(
    field: &LinkCompletionField,
    index: &NoteIndex,
) -> Vec<WikilinkNoteCandidate> {
    let mut ranked = index
        .notes
        .iter()
        .filter_map(|note| {
            note.best_note_match(&field.query)
                .map(|score| (score, note))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_score, left), (right_score, right)| {
        compare_score_rank(left_score, right_score)
            .then_with(|| compare_normalized(&left.path, &right.path))
    });
    ranked.truncate(RESULT_LIMIT);

    ranked
        .into_iter()
        .map(|(score, note)| {
            let alias = score.kind.alias_match().then(|| score.value.clone());
            let mut replacement = note.insert_target.clone();
            if let Some(alias) = &alias {
                replacement.push('|');
                replacement.push_str(alias);
            }
            let replacement = field.replacement_text(replacement);
            WikilinkNoteCandidate {
                cursor_after: field.cursor_after(replacement.len()),
                replacement,
                path: note.path.clone(),
                name: note.stem.clone(),
                alias,
                match_kind: score.kind,
            }
        })
        .collect()
}

pub(crate) fn heading_candidates(
    field: &LinkCompletionField,
    index: &NoteIndex,
) -> Vec<WikilinkHeadingCandidate> {
    let LinkCompletionTarget::Heading(scope) = &field.target else {
        return Vec::new();
    };
    let mut ranked = notes_for_scope(index, scope)
        .into_iter()
        .flat_map(|note| {
            note.headings.iter().filter_map(move |heading| {
                best_match(&field.query, heading.title_match_items())
                    .map(|score| (score, note, heading))
            })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(
        |(left_score, left_note, left_heading),
         (right_score, right_note, right_heading)| {
            compare_score_rank(left_score, right_score)
                .then_with(|| {
                    compare_normalized(&left_note.path, &right_note.path)
                })
                .then_with(|| left_heading.ordinal.cmp(&right_heading.ordinal))
        },
    );
    ranked.truncate(RESULT_LIMIT);

    ranked
        .into_iter()
        .map(|(score, note, heading)| {
            let replacement = match scope {
                LinkSearchScope::Vault => {
                    format!("{}#{}", note.insert_target, heading.title)
                }
                LinkSearchScope::Target(_) | LinkSearchScope::Current(_) => {
                    heading.title.clone()
                }
            };
            let replacement = field.replacement_text(replacement);
            WikilinkHeadingCandidate {
                cursor_after: field.cursor_after(replacement.len()),
                replacement,
                path: note.path.clone(),
                name: note.stem.clone(),
                heading: heading.title.clone(),
                level: heading.level,
                match_kind: score.kind,
            }
        })
        .collect()
}

pub(crate) fn block_candidates(
    field: &LinkCompletionField,
    index: &NoteIndex,
) -> Vec<WikilinkBlockCandidate> {
    let LinkCompletionTarget::Block(scope) = &field.target else {
        return Vec::new();
    };
    let mut ranked = notes_for_scope(index, scope)
        .into_iter()
        .flat_map(|note| {
            note.blocks.iter().filter_map(move |block| {
                best_match(&field.query, block.match_items())
                    .map(|score| (score, note, block))
            })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(
        |(left_score, left_note, left_block),
         (right_score, right_note, right_block)| {
            compare_score_rank(left_score, right_score)
                .then_with(|| {
                    compare_normalized(&left_note.path, &right_note.path)
                })
                .then_with(|| left_block.ordinal.cmp(&right_block.ordinal))
        },
    );
    ranked.truncate(RESULT_LIMIT);

    ranked
        .into_iter()
        .map(|(score, note, block)| {
            let replacement = match scope {
                LinkSearchScope::Vault => {
                    format!("{}#^{}", note.insert_target, block.block_id)
                }
                LinkSearchScope::Target(_) | LinkSearchScope::Current(_) => {
                    block.block_id.clone()
                }
            };
            let replacement = field.replacement_text(replacement);
            WikilinkBlockCandidate {
                cursor_after: field.cursor_after(replacement.len()),
                replacement,
                path: note.path.clone(),
                name: note.stem.clone(),
                block_id: block.block_id.clone(),
                preview: block.preview.clone(),
                match_kind: score.kind,
            }
        })
        .collect()
}

impl LinkCompletionField {
    fn replacement_text(&self, mut replacement: String) -> String {
        if self.suffix_close {
            replacement.push_str("]]");
        }
        replacement
    }

    fn cursor_after(&self, replacement_len: usize) -> usize {
        let old_len = self.replacement.1 - self.replacement.0;
        match self.existing_close_end {
            Some(close_end) => {
                (close_end as isize + replacement_len as isize
                    - old_len as isize) as usize
            }
            None => self.replacement.0 + replacement_len,
        }
    }
}

impl LinkSyntax {
    fn spans(&self) -> Vec<Span> {
        let mut spans = Vec::new();
        spans.extend(self.delimiters.iter().map(|component| Span {
            start: component.start,
            end: component.end,
            kind: SpanKind::WikilinkDelimiter,
        }));
        if let Some(target) = self.target.filter(Component::is_non_empty) {
            spans.push(target.span(SpanKind::WikilinkTarget));
        }
        if let Some(heading) = self.heading.filter(Component::is_non_empty) {
            spans.push(heading.span(SpanKind::WikilinkHeading));
        }
        if let Some(block) = self.block.filter(Component::is_non_empty) {
            spans.push(block.span(SpanKind::WikilinkBlockId));
        }
        if let Some(alias) = self.alias.filter(Component::is_non_empty) {
            spans.push(alias.span(SpanKind::WikilinkAlias));
        }
        spans.sort_by_key(|span| (span.start, span.end));
        spans
    }

    fn completion_field(
        &self,
        raw: &str,
        cursor: usize,
        current_note_path: &str,
    ) -> Option<LinkCompletionField> {
        if cursor < self.start || cursor > self.end {
            return None;
        }
        if cursor < self.inner_start {
            return None;
        }
        if self.close.is_some_and(|(start, _)| cursor > start) {
            return None;
        }
        if self
            .alias
            .is_some_and(|alias| cursor > alias.start && cursor <= alias.end)
        {
            return None;
        }

        match self.scope {
            Some(SyntaxScope::VaultHeading) => self.subpath_field(
                raw,
                CompletionContext::WikilinkHeading,
                LinkCompletionTarget::Heading(LinkSearchScope::Vault),
                self.heading?,
                cursor,
                self.inner_start,
            ),
            Some(SyntaxScope::VaultBlock) => self.subpath_field(
                raw,
                CompletionContext::WikilinkBlock,
                LinkCompletionTarget::Block(LinkSearchScope::Vault),
                self.block?,
                cursor,
                self.inner_start,
            ),
            Some(SyntaxScope::CurrentHeading) => self.subpath_field(
                raw,
                CompletionContext::WikilinkHeading,
                LinkCompletionTarget::Heading(LinkSearchScope::Current(
                    current_note_path.to_string(),
                )),
                self.heading?,
                cursor,
                self.heading?.start,
            ),
            Some(SyntaxScope::CurrentBlock) => self.subpath_field(
                raw,
                CompletionContext::WikilinkBlock,
                LinkCompletionTarget::Block(LinkSearchScope::Current(
                    current_note_path.to_string(),
                )),
                self.block?,
                cursor,
                self.block?.start,
            ),
            Some(SyntaxScope::TargetHeading) => {
                if let Some(target) = self.target
                    && cursor <= target.end
                {
                    return self.note_field(raw, cursor);
                }
                let raw_target = self.component_text(raw, self.target?);
                self.subpath_field(
                    raw,
                    CompletionContext::WikilinkHeading,
                    LinkCompletionTarget::Heading(LinkSearchScope::Target(
                        raw_target.to_string(),
                    )),
                    self.heading?,
                    cursor,
                    self.heading?.start,
                )
            }
            Some(SyntaxScope::TargetBlock) => {
                if let Some(target) = self.target
                    && cursor <= target.end
                {
                    return self.note_field(raw, cursor);
                }
                let raw_target = self.component_text(raw, self.target?);
                self.subpath_field(
                    raw,
                    CompletionContext::WikilinkBlock,
                    LinkCompletionTarget::Block(LinkSearchScope::Target(
                        raw_target.to_string(),
                    )),
                    self.block?,
                    cursor,
                    self.block?.start,
                )
            }
            None => self.note_field(raw, cursor),
        }
    }

    fn note_field(
        &self,
        raw: &str,
        cursor: usize,
    ) -> Option<LinkCompletionField> {
        let target = self.target.unwrap_or(Component {
            start: self.inner_start,
            end: self.inner_start,
        });
        if cursor < target.start || cursor > target.end {
            return None;
        }
        let replacement_end = self.inner_end;
        Some(LinkCompletionField {
            context: CompletionContext::WikilinkNote,
            query: query_from_component(raw, target, cursor),
            replacement: (self.inner_start, replacement_end),
            target: LinkCompletionTarget::Note,
            suffix_close: self.close.is_none(),
            existing_close_end: self.close.map(|(_, end)| end),
        })
    }

    fn subpath_field(
        &self,
        raw: &str,
        context: CompletionContext,
        target: LinkCompletionTarget,
        component: Component,
        cursor: usize,
        replacement_start: usize,
    ) -> Option<LinkCompletionField> {
        if cursor < component.start || cursor > component.end {
            return None;
        }
        Some(LinkCompletionField {
            context,
            query: query_from_component(raw, component, cursor),
            replacement: (replacement_start, component.end),
            target,
            suffix_close: self.close.is_none()
                && component.end == self.inner_end,
            existing_close_end: self.close.map(|(_, end)| end),
        })
    }

    fn component_text<'a>(
        &self,
        raw: &'a str,
        component: Component,
    ) -> &'a str {
        &raw[component.start..component.end]
    }
}

impl Component {
    fn is_non_empty(&self) -> bool {
        self.start < self.end
    }

    fn span(self, kind: SpanKind) -> Span {
        Span {
            start: self.start,
            end: self.end,
            kind,
        }
    }
}

fn query_from_component(
    raw: &str,
    component: Component,
    cursor: usize,
) -> String {
    let end = cursor.clamp(component.start, component.end);
    raw[component.start..end].to_string()
}

fn scan_links(raw: &str, code_ranges: &[(usize, usize)]) -> Vec<LinkSyntax> {
    let mut links = Vec::new();
    let mut line_start = 0;
    for line in raw.split_inclusive('\n') {
        let line_end = line_start + line.trim_end_matches(['\r', '\n']).len();
        scan_line(raw, line_start, line_end, code_ranges, &mut links);
        line_start += line.len();
    }
    if !raw.ends_with('\n') && line_start == 0 {
        scan_line(raw, 0, raw.len(), code_ranges, &mut links);
    }
    links
}

fn scan_line(
    raw: &str,
    line_start: usize,
    line_end: usize,
    code_ranges: &[(usize, usize)],
    links: &mut Vec<LinkSyntax>,
) {
    let mut cursor = line_start;
    while cursor + 1 < line_end {
        let Some((open_start, open_bracket, open_end)) =
            next_open(raw, cursor, line_end)
        else {
            break;
        };
        if is_escaped(raw, open_bracket)
            || covered_by_code(code_ranges, open_start)
        {
            cursor = open_bracket + 1;
            continue;
        }

        match close_or_nested(raw, open_end, line_end, code_ranges) {
            LinkScanEnd::Close(close_start) => {
                let close_end = close_start + 2;
                links.push(parse_link(
                    raw,
                    open_start,
                    open_end,
                    close_start,
                    Some((close_start, close_end)),
                ));
                cursor = close_end;
            }
            LinkScanEnd::Nested(nested_start) => {
                cursor = nested_start;
            }
            LinkScanEnd::LineEnd => {
                links.push(parse_link(
                    raw, open_start, open_end, line_end, None,
                ));
                break;
            }
        }
    }
}

fn next_open(
    raw: &str,
    start: usize,
    end: usize,
) -> Option<(usize, usize, usize)> {
    let bytes = raw.as_bytes();
    let mut index = start;
    while index + 1 < end {
        if bytes[index] == b'[' && bytes[index + 1] == b'[' {
            let open_start = if index > start && bytes[index - 1] == b'!' {
                index - 1
            } else {
                index
            };
            return Some((open_start, index, index + 2));
        }
        index += 1;
    }
    None
}

enum LinkScanEnd {
    Close(usize),
    Nested(usize),
    LineEnd,
}

fn close_or_nested(
    raw: &str,
    start: usize,
    end: usize,
    code_ranges: &[(usize, usize)],
) -> LinkScanEnd {
    let bytes = raw.as_bytes();
    let mut index = start;
    while index + 1 < end {
        if covered_by_code(code_ranges, index) {
            index += 1;
            continue;
        }
        if bytes[index] == b'['
            && bytes[index + 1] == b'['
            && !is_escaped(raw, index)
        {
            return LinkScanEnd::Nested(index);
        }
        if bytes[index] == b']'
            && bytes[index + 1] == b']'
            && !is_escaped(raw, index)
        {
            return LinkScanEnd::Close(index);
        }
        index += 1;
    }
    LinkScanEnd::LineEnd
}

fn parse_link(
    raw: &str,
    start: usize,
    open_end: usize,
    inner_end: usize,
    close: Option<(usize, usize)>,
) -> LinkSyntax {
    let inner_start = open_end;
    let mut delimiters = vec![Component {
        start,
        end: open_end,
    }];
    if let Some((start, end)) = close {
        delimiters.push(Component { start, end });
    }

    let (main_end, alias) = match raw[inner_start..inner_end].find('|') {
        Some(relative) => {
            let separator = inner_start + relative;
            delimiters.push(Component {
                start: separator,
                end: separator + 1,
            });
            (
                separator,
                Some(Component {
                    start: separator + 1,
                    end: inner_end,
                }),
            )
        }
        None => (inner_end, None),
    };

    let parsed = parse_main(raw, inner_start, main_end, &mut delimiters);
    LinkSyntax {
        start,
        end: close.map_or(inner_end, |(_, end)| end),
        inner_start,
        inner_end,
        close,
        target: parsed.target,
        heading: parsed.heading,
        block: parsed.block,
        alias,
        delimiters,
        scope: parsed.scope,
    }
}

struct ParsedMain {
    target: Option<Component>,
    heading: Option<Component>,
    block: Option<Component>,
    scope: Option<SyntaxScope>,
}

fn parse_main(
    raw: &str,
    start: usize,
    end: usize,
    delimiters: &mut Vec<Component>,
) -> ParsedMain {
    let main = &raw[start..end];
    if main.starts_with("^^") {
        delimiters.push(Component {
            start,
            end: start + 2,
        });
        return ParsedMain {
            target: None,
            heading: None,
            block: Some(Component {
                start: start + 2,
                end,
            }),
            scope: Some(SyntaxScope::VaultBlock),
        };
    }
    if main.starts_with("##") {
        delimiters.push(Component {
            start,
            end: start + 2,
        });
        return ParsedMain {
            target: None,
            heading: Some(Component {
                start: start + 2,
                end,
            }),
            block: None,
            scope: Some(SyntaxScope::VaultHeading),
        };
    }
    if main.starts_with("#^") {
        delimiters.push(Component {
            start,
            end: start + 2,
        });
        return ParsedMain {
            target: None,
            heading: None,
            block: Some(Component {
                start: start + 2,
                end,
            }),
            scope: Some(SyntaxScope::CurrentBlock),
        };
    }
    if main.starts_with('#') {
        delimiters.push(Component {
            start,
            end: start + 1,
        });
        return ParsedMain {
            target: None,
            heading: Some(Component {
                start: start + 1,
                end,
            }),
            block: None,
            scope: Some(SyntaxScope::CurrentHeading),
        };
    }
    if let Some(relative) = main.find("#^") {
        let separator = start + relative;
        delimiters.push(Component {
            start: separator,
            end: separator + 2,
        });
        return ParsedMain {
            target: Some(Component {
                start,
                end: separator,
            }),
            heading: None,
            block: Some(Component {
                start: separator + 2,
                end,
            }),
            scope: Some(SyntaxScope::TargetBlock),
        };
    }
    if let Some(relative) = main.find('#') {
        let separator = start + relative;
        delimiters.push(Component {
            start: separator,
            end: separator + 1,
        });
        return ParsedMain {
            target: Some(Component {
                start,
                end: separator,
            }),
            heading: Some(Component {
                start: separator + 1,
                end,
            }),
            block: None,
            scope: Some(SyntaxScope::TargetHeading),
        };
    }

    ParsedMain {
        target: Some(Component { start, end }),
        heading: None,
        block: None,
        scope: None,
    }
}

fn code_ranges(raw: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut line_start = 0;
    let mut fence = None;
    for line in raw.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_content = line.trim_end_matches(['\r', '\n']);
        if let Some(open) = fence {
            ranges.push((line_start, line_end));
            if markdown::closes_fence(line_content, open) {
                fence = None;
            }
        } else if let Some(open) = markdown::fence_marker(line_content) {
            ranges.push((line_start, line_end));
            fence = Some(open);
        } else {
            ranges.extend(inline_code_ranges(line_content, line_start));
        }
        line_start = line_end;
    }
    if raw.is_empty() {
        return ranges;
    }
    if !raw.ends_with('\n') && line_start == 0 {
        ranges.extend(inline_code_ranges(raw, 0));
    }
    ranges
}

fn inline_code_ranges(line: &str, offset: usize) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'`' || is_escaped(line, index) {
            index += 1;
            continue;
        }
        let tick_count = bytes[index..]
            .iter()
            .take_while(|byte| **byte == b'`')
            .count();
        let content_start = index + tick_count;
        let mut close = content_start;
        while close < bytes.len() {
            if bytes[close] == b'`'
                && bytes[close..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count()
                    == tick_count
            {
                ranges.push((offset + index, offset + close + tick_count));
                index = close + tick_count;
                break;
            }
            close += 1;
        }
        if close >= bytes.len() {
            ranges.push((offset + index, offset + bytes.len()));
            break;
        }
    }
    ranges
}

fn covered_by_code(ranges: &[(usize, usize)], index: usize) -> bool {
    ranges
        .iter()
        .any(|(start, end)| *start <= index && index < *end)
}

fn is_escaped(raw: &str, index: usize) -> bool {
    let bytes = raw.as_bytes();
    let mut cursor = index;
    let mut count = 0;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        count += 1;
        cursor -= 1;
    }
    count % 2 == 1
}

fn collect_markdown_paths(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
    warnings: &mut WarningCollector,
) -> io::Result<()> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                warnings.push(format!(
                    "skipping {}: {error}",
                    relative_display(root, &path)
                ));
                continue;
            }
        };
        if file_type.is_dir() {
            if is_excluded_directory(&entry.file_name()) {
                continue;
            }
            if let Err(error) =
                collect_markdown_paths(root, &path, paths, warnings)
            {
                warnings.push(format!(
                    "skipping {}: {error}",
                    relative_display(root, &path)
                ));
            }
        } else if file_type.is_file() && has_markdown_extension(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn is_excluded_directory(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
        || is_always_excluded_note_directory_name(name)
}

fn has_markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

impl NoteEntry {
    fn read(
        bob_dir: &Path,
        path: &Path,
        warnings: &mut WarningCollector,
    ) -> Self {
        let relative_path = path
            .strip_prefix(bob_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let insert_target = relative_path
            .strip_suffix(".md")
            .unwrap_or(&relative_path)
            .to_string();
        let stem = Path::new(&relative_path)
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or(&relative_path)
            .to_string();

        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) => {
                warnings.push(format!("read {relative_path}: {error}"));
                return Self {
                    path: relative_path,
                    insert_target,
                    stem,
                    aliases: Vec::new(),
                    headings: Vec::new(),
                    blocks: Vec::new(),
                };
            }
        };
        let aliases =
            aliases_from_frontmatter(&contents, &relative_path, warnings);
        let (headings, blocks) = scan_note_body(&contents);

        Self {
            path: relative_path,
            insert_target,
            stem,
            aliases,
            headings,
            blocks,
        }
    }

    fn best_note_match(&self, query: &str) -> Option<MatchScore> {
        let mut items = vec![
            MatchItem::new(&self.stem, MatchComponent::Stem),
            MatchItem::new(&self.insert_target, MatchComponent::Path),
            MatchItem::new(&self.path, MatchComponent::Path),
        ];
        items.extend(
            self.aliases
                .iter()
                .map(|alias| MatchItem::new(alias, MatchComponent::Alias)),
        );
        best_match(query, items)
    }
}

fn aliases_from_frontmatter(
    contents: &str,
    relative_path: &str,
    warnings: &mut WarningCollector,
) -> Vec<String> {
    let Some(frontmatter) = closed_frontmatter(contents) else {
        return Vec::new();
    };
    let parsed: serde_yaml::Value = match serde_yaml::from_str(frontmatter) {
        Ok(parsed) => parsed,
        Err(error) => {
            warnings.push(format!("parse aliases in {relative_path}: {error}"));
            return Vec::new();
        }
    };
    let serde_yaml::Value::Mapping(mapping) = parsed else {
        return Vec::new();
    };
    let Some(value) = mapping.get(serde_yaml::Value::String("aliases".into()))
    else {
        return Vec::new();
    };
    let aliases = match value {
        serde_yaml::Value::String(value) => vec![value.clone()],
        serde_yaml::Value::Sequence(values) => values
            .iter()
            .filter_map(|value| match value {
                serde_yaml::Value::String(value) => Some(value.clone()),
                _ => None,
            })
            .collect(),
        serde_yaml::Value::Null => Vec::new(),
        _ => {
            warnings
                .push(format!("ignore non-string aliases in {relative_path}"));
            Vec::new()
        }
    };
    dedup_strings(
        aliases
            .into_iter()
            .map(|alias| alias.trim().to_string())
            .filter(|alias| !alias.is_empty())
            .collect(),
    )
}

fn closed_frontmatter(contents: &str) -> Option<&str> {
    let marker_len = if contents.starts_with("---\r\n") {
        5
    } else if contents.starts_with("---\n") {
        4
    } else {
        return None;
    };
    let rest = &contents[marker_len..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let line_content = line.trim_end_matches(['\r', '\n']);
        if line_content == "---" {
            return Some(&rest[..offset]);
        }
        offset += line.len();
    }
    None
}

fn scan_note_body(contents: &str) -> (Vec<HeadingEntry>, Vec<BlockEntry>) {
    let mut headings = Vec::new();
    let mut blocks = Vec::new();
    let mut fence = None;
    let frontmatter_end = frontmatter_line_end(contents);

    for (line_index, line) in contents.lines().enumerate() {
        if frontmatter_end.is_some_and(|end| line_index <= end) {
            continue;
        }
        if let Some(open) = fence {
            if markdown::closes_fence(line, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = markdown::fence_marker(line) {
            fence = Some(open);
            continue;
        }
        if let Some((level, title)) = markdown::atx_heading(line)
            && !title.is_empty()
        {
            headings.push(HeadingEntry {
                title: title.to_string(),
                level,
                ordinal: line_index,
            });
        }
        if let Some(captures) = BLOCK_ID_RE.captures(line)
            && let Some(block_id) = captures.get(1)
        {
            blocks.push(BlockEntry {
                block_id: block_id.as_str().to_string(),
                preview: block_preview(line, block_id.start()),
                ordinal: line_index,
            });
        }
    }
    (headings, blocks)
}

fn frontmatter_line_end(contents: &str) -> Option<usize> {
    let lines = contents.lines().collect::<Vec<_>>();
    markdown::strictly_closed_frontmatter_end(&lines)
}

fn block_preview(line: &str, block_start: usize) -> Option<String> {
    let preview = line[..block_start].trim_end().trim_end_matches('^').trim();
    (!preview.is_empty()).then(|| preview.chars().take(80).collect())
}

fn notes_for_scope<'a>(
    index: &'a NoteIndex,
    scope: &LinkSearchScope,
) -> Vec<&'a NoteEntry> {
    match scope {
        LinkSearchScope::Vault => index.notes.iter().collect(),
        LinkSearchScope::Target(target) => index.resolve_target(target),
        LinkSearchScope::Current(path) => index.resolve_target(path),
    }
}

impl NoteIndex {
    fn resolve_target<'a>(&'a self, target: &str) -> Vec<&'a NoteEntry> {
        let normalized = normalize_target(target);
        if normalized.is_empty() {
            return Vec::new();
        }
        let exact_path = self
            .notes
            .iter()
            .filter(|note| {
                equals_normalized(&note.insert_target, &normalized)
                    || equals_normalized(&note.path, &normalized)
                    || equals_normalized(
                        &note.path,
                        &format!("{normalized}.md"),
                    )
            })
            .collect::<Vec<_>>();
        if !exact_path.is_empty() {
            return exact_path;
        }
        if normalized.contains('/') {
            return Vec::new();
        }
        let stem = self
            .notes
            .iter()
            .filter(|note| equals_normalized(&note.stem, &normalized))
            .collect::<Vec<_>>();
        if stem.len() == 1 {
            return stem;
        }
        let alias = self
            .notes
            .iter()
            .filter(|note| {
                note.aliases
                    .iter()
                    .any(|alias| equals_normalized(alias, &normalized))
            })
            .collect::<Vec<_>>();
        if alias.len() == 1 {
            return alias;
        }
        Vec::new()
    }
}

fn normalize_target(target: &str) -> String {
    let target = target.trim().replace('\\', "/");
    target.strip_suffix(".md").unwrap_or(&target).to_string()
}

impl HeadingEntry {
    fn title_match_items(&self) -> Vec<MatchItem<'_>> {
        vec![MatchItem::new(&self.title, MatchComponent::Stem)]
    }
}

impl BlockEntry {
    fn match_items(&self) -> Vec<MatchItem<'_>> {
        vec![MatchItem::new(&self.block_id, MatchComponent::Stem)]
    }
}

#[derive(Debug, Clone, Copy)]
struct MatchItem<'a> {
    value: &'a str,
    component: MatchComponent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchComponent {
    Alias,
    Stem,
    Path,
}

impl<'a> MatchItem<'a> {
    fn new(value: &'a str, component: MatchComponent) -> Self {
        Self { value, component }
    }
}

fn best_match(query: &str, items: Vec<MatchItem<'_>>) -> Option<MatchScore> {
    let mut scores = items
        .into_iter()
        .filter_map(|item| match_score(query, item))
        .collect::<Vec<_>>();
    scores.sort_by(compare_scores);
    scores.into_iter().next()
}

fn match_score(query: &str, item: MatchItem<'_>) -> Option<MatchScore> {
    if query.is_empty() {
        return Some(MatchScore {
            tier: 9,
            component: item.component.empty_rank(),
            value: item.value.to_string(),
            kind: MatchKind::Empty,
        });
    }
    let query = query.to_ascii_lowercase();
    let value = item.value.to_ascii_lowercase();
    let tier = if value == query {
        MatchTier::Exact
    } else if value.starts_with(&query) {
        MatchTier::Prefix
    } else if word_boundary_match(&value, &query) {
        MatchTier::WordBoundary
    } else if acronym(&value).starts_with(&query) {
        MatchTier::Acronym
    } else if value.contains(&query) {
        MatchTier::Substring
    } else {
        return None;
    };
    Some(MatchScore {
        tier: tier.rank(),
        component: item.component.rank_for_tier(tier),
        value: item.value.to_string(),
        kind: item.component.match_kind(tier),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchTier {
    Exact,
    Prefix,
    WordBoundary,
    Acronym,
    Substring,
}

impl MatchTier {
    fn rank(self) -> u8 {
        match self {
            Self::Exact => 0,
            Self::Prefix => 1,
            Self::WordBoundary => 2,
            Self::Acronym => 3,
            Self::Substring => 4,
        }
    }
}

impl MatchComponent {
    fn rank(self) -> u8 {
        match self {
            Self::Alias => 0,
            Self::Stem => 1,
            Self::Path => 2,
        }
    }

    fn rank_for_tier(self, tier: MatchTier) -> u8 {
        if tier == MatchTier::Prefix {
            match self {
                Self::Alias | Self::Stem => 0,
                Self::Path => 1,
            }
        } else {
            self.rank()
        }
    }

    fn empty_rank(self) -> u8 {
        match self {
            Self::Stem => 0,
            Self::Path => 1,
            Self::Alias => 2,
        }
    }

    fn match_kind(self, tier: MatchTier) -> MatchKind {
        match (tier, self) {
            (MatchTier::Exact, Self::Alias) => MatchKind::ExactAlias,
            (MatchTier::Exact, Self::Stem) => MatchKind::ExactStem,
            (MatchTier::Exact, Self::Path) => MatchKind::ExactPath,
            (MatchTier::Prefix, Self::Alias) => MatchKind::PrefixAlias,
            (MatchTier::Prefix, Self::Stem) => MatchKind::PrefixStem,
            (MatchTier::Prefix, Self::Path) => MatchKind::PrefixPath,
            (MatchTier::WordBoundary, Self::Alias) => {
                MatchKind::WordBoundaryAlias
            }
            (MatchTier::WordBoundary, Self::Stem) => {
                MatchKind::WordBoundaryStem
            }
            (MatchTier::WordBoundary, Self::Path) => {
                MatchKind::WordBoundaryPath
            }
            (MatchTier::Acronym, Self::Alias) => MatchKind::AcronymAlias,
            (MatchTier::Acronym, Self::Stem) => MatchKind::AcronymStem,
            (MatchTier::Acronym, Self::Path) => MatchKind::AcronymPath,
            (MatchTier::Substring, Self::Alias) => MatchKind::SubstringAlias,
            (MatchTier::Substring, Self::Stem) => MatchKind::SubstringStem,
            (MatchTier::Substring, Self::Path) => MatchKind::SubstringPath,
        }
    }
}

impl MatchKind {
    fn alias_match(self) -> bool {
        matches!(
            self,
            Self::ExactAlias
                | Self::PrefixAlias
                | Self::WordBoundaryAlias
                | Self::AcronymAlias
                | Self::SubstringAlias
        )
    }
}

fn compare_scores(left: &MatchScore, right: &MatchScore) -> Ordering {
    compare_score_rank(left, right)
        .then_with(|| compare_normalized(&left.value, &right.value))
}

fn compare_score_rank(left: &MatchScore, right: &MatchScore) -> Ordering {
    left.tier
        .cmp(&right.tier)
        .then_with(|| left.component.cmp(&right.component))
}

fn word_boundary_match(value: &str, query: &str) -> bool {
    words(value).any(|word| word.starts_with(query))
}

fn acronym(value: &str) -> String {
    words(value)
        .filter_map(|word| word.chars().next())
        .collect::<String>()
}

fn words(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_'
        })
        .filter(|word| !word.is_empty())
}

fn compare_normalized(left: &str, right: &str) -> Ordering {
    left.to_ascii_lowercase()
        .cmp(&right.to_ascii_lowercase())
        .then_with(|| left.cmp(right))
}

fn equals_normalized(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn dedup_strings(values: Vec<String>) -> Vec<String> {
    let mut output = Vec::new();
    for value in values {
        if !output.contains(&value) {
            output.push(value);
        }
    }
    output
}

#[derive(Debug, Default)]
struct WarningCollector {
    warnings: Vec<String>,
    extra: usize,
}

impl WarningCollector {
    fn push(&mut self, warning: String) {
        if self.warnings.len() < WARNING_LIMIT {
            self.warnings.push(warning);
        } else {
            self.extra += 1;
        }
    }

    fn finish(mut self) -> Vec<String> {
        if self.extra > 0 {
            self.warnings.push(format!(
                "{} additional link-index warnings omitted",
                self.extra
            ));
        }
        self.warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn scans_complete_incomplete_embed_and_subpath_spans() {
        let raw = "see ![[Projects/Alpha#^block-1|Alias]] and [[sas";
        let spans = wikilink_spans(raw);
        let observed = spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![
                (4, 7, SpanKind::WikilinkDelimiter),
                (7, 21, SpanKind::WikilinkTarget),
                (21, 23, SpanKind::WikilinkDelimiter),
                (23, 30, SpanKind::WikilinkBlockId),
                (30, 31, SpanKind::WikilinkDelimiter),
                (31, 36, SpanKind::WikilinkAlias),
                (36, 38, SpanKind::WikilinkDelimiter),
                (43, 45, SpanKind::WikilinkDelimiter),
                (45, 48, SpanKind::WikilinkTarget),
            ]
        );
    }

    #[test]
    fn scanner_ignores_escaped_and_code_literal_links() {
        let raw =
            "\\[[escaped]] `[[inline]]`\n```md\n[[fenced]]\n```\n[[real]]";
        let spans = wikilink_spans(raw);
        assert_eq!(
            spans
                .iter()
                .filter(|span| span.kind == SpanKind::WikilinkTarget)
                .map(|span| &raw[span.start..span.end])
                .collect::<Vec<_>>(),
            vec!["real"]
        );
    }

    #[test]
    fn scanner_recovers_from_nested_openers() {
        let raw = "[[bad [[good]]";
        let spans = wikilink_spans(raw);
        assert_eq!(
            spans
                .iter()
                .filter(|span| span.kind == SpanKind::WikilinkTarget)
                .map(|span| &raw[span.start..span.end])
                .collect::<Vec<_>>(),
            vec!["good"]
        );
    }

    #[test]
    fn note_completion_deduplicates_existing_close_and_synthesizes_missing_close(
    ) {
        let temp = TempDir::new("bob-cli-links-note-close");
        write_file(&temp.path().join("sase.md"), "# Design\n");
        let index = NoteIndex::read(temp.path()).expect("index");

        let closed = completion_field_at("[[sas]]", 5, "mac_inbox.md".into())
            .expect("closed field");
        let closed_candidate = &note_candidates(&closed, &index)[0];
        assert_eq!(closed.replacement, (2, 5));
        assert_eq!(closed_candidate.replacement, "sase");
        assert_eq!(closed_candidate.cursor_after, 8);

        let open = completion_field_at("[[sas", 5, "mac_inbox.md".into())
            .expect("open");
        let open_candidate = &note_candidates(&open, &index)[0];
        assert_eq!(open_candidate.replacement, "sase]]");
        assert_eq!(open_candidate.cursor_after, 8);
    }

    #[test]
    fn note_completion_ranks_aliases_stems_paths_and_limits_empty_queries() {
        let temp = TempDir::new("bob-cli-links-note-ranking");
        for index in 0..25 {
            write_file(&temp.path().join(format!("Notes/{index:02}.md")), "");
        }
        write_file(
            &temp.path().join("Artificial Intelligence.md"),
            "---\naliases: [AI]\n---\n",
        );
        write_file(&temp.path().join("Projects/Alpha.md"), "");
        let index = NoteIndex::read(temp.path()).expect("index");

        let field = completion_field_at("[[AI", 4, "mac_inbox.md".into())
            .expect("field");
        let candidate = &note_candidates(&field, &index)[0];
        assert_eq!(candidate.replacement, "Artificial Intelligence|AI]]");
        assert_eq!(candidate.alias.as_deref(), Some("AI"));
        assert_eq!(candidate.match_kind, MatchKind::ExactAlias);

        let empty =
            completion_field_at("[[", 2, "mac_inbox.md".into()).expect("field");
        let empty_candidates = note_candidates(&empty, &index);
        assert_eq!(empty_candidates.len(), RESULT_LIMIT);
        assert_eq!(
            empty_candidates[0].replacement,
            "Artificial Intelligence]]"
        );
        assert_eq!(empty_candidates[0].alias, None);
        assert_eq!(empty_candidates[0].match_kind, MatchKind::Empty);
    }

    #[test]
    fn heading_and_block_completion_resolve_target_same_note_and_vault_scope() {
        let temp = TempDir::new("bob-cli-links-subpaths");
        write_file(
            &temp.path().join("sase.md"),
            "# Design\nParagraph ^block-1\n",
        );
        write_file(&temp.path().join("Other.md"), "# Discovery\nText ^other\n");
        let index = NoteIndex::read(temp.path()).expect("index");

        let target = completion_field_at("[[sase#De", 9, "mac_inbox.md".into())
            .expect("target heading");
        assert_eq!(target.context, CompletionContext::WikilinkHeading);
        let heading = &heading_candidates(&target, &index)[0];
        assert_eq!(heading.replacement, "Design]]");
        assert_eq!(heading.path, "sase.md");

        let same = completion_field_at("[[#^blo", 7, "sase.md".into())
            .expect("same block");
        let block = &block_candidates(&same, &index)[0];
        assert_eq!(block.replacement, "block-1]]");
        assert_eq!(block.preview.as_deref(), Some("Paragraph"));

        let vault = completion_field_at("[[##Disc", 8, "sase.md".into())
            .expect("vault heading");
        let heading = &heading_candidates(&vault, &index)[0];
        assert_eq!(heading.replacement, "Other#Discovery]]");
    }

    #[test]
    fn index_skips_hidden_generated_template_and_symlink_directories() {
        let temp = TempDir::new("bob-cli-links-exclusions");
        write_file(&temp.path().join("Visible.md"), "");
        write_file(&temp.path().join(".trash/Hidden.md"), "");
        write_file(&temp.path().join("_generated/Hidden.md"), "");
        write_file(&temp.path().join("_templates/Hidden.md"), "");
        let index = NoteIndex::read(temp.path()).expect("index");
        assert_eq!(
            index
                .notes
                .iter()
                .map(|note| note.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Visible.md"]
        );
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "{}-{}-{}-{}",
                prefix,
                std::process::id(),
                current_time_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn current_time_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    }
}
