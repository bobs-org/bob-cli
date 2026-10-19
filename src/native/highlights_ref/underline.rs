//! Underline classification, matching, and render-time emphasis.

use super::*;
use std::ops::Range;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct UnderlinePlan {
    /// Absorbed underline order -> the host highlight member's order.
    pub(super) host_of: BTreeMap<usize, usize>,
    /// Underline rows that stay as their own blocks, in sidecar order.
    pub(super) standalone: BTreeSet<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct AnnotationFolds {
    pub(super) underlines: UnderlinePlan,
    pub(super) continuations: ContinuationPlan,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MatchKey {
    chars: Vec<char>,
    offsets: Vec<usize>,
}

pub(super) fn underline_inner(text: &str) -> Option<String> {
    let delimiters = text
        .match_indices("**")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if delimiters.is_empty() || delimiters.len() % 2 != 0 {
        return None;
    }

    let first = delimiters[0];
    if !text[..first].chars().all(char::is_whitespace) {
        return None;
    }

    let mut inner = String::new();
    let mut previous_end = first;
    for pair in delimiters.chunks_exact(2) {
        let open = pair[0];
        let close = pair[1];
        if open != previous_end {
            let whitespace = &text[previous_end..open];
            if !whitespace.chars().all(char::is_whitespace) {
                return None;
            }
            inner.push_str(whitespace);
        }
        let start = open + 2;
        let value = &text[start..close];
        inner.push_str(value);
        previous_end = close + 2;
    }

    if !text[previous_end..].chars().all(char::is_whitespace) {
        return None;
    }
    (!inner.trim().is_empty()).then_some(inner)
}

pub(super) fn plan_underlines(
    annotations: &[SidecarAnnotation],
    strip_return_links: bool,
) -> UnderlinePlan {
    let mut plan = UnderlinePlan::default();
    let mut underlines = BTreeMap::<usize, (String, String, bool)>::new();

    for annotation in annotations {
        if !is_text_highlight(annotation) {
            continue;
        }
        let Some(inner) = underline_inner(&annotation.text) else {
            continue;
        };
        let cleaned = clean_for_matching(&inner, strip_return_links);
        let has_continuation_marker =
            annotation.task_source.as_deref().is_some_and(|source| {
                matches!(
                    scan_continuation_marker(source),
                    MarkerScan::Marker { .. }
                )
            });
        underlines.insert(
            annotation.order,
            (inner, cleaned, has_continuation_marker),
        );
    }

    let candidates = annotations
        .iter()
        .filter(|annotation| {
            is_text_highlight(annotation)
                && !underlines.contains_key(&annotation.order)
        })
        .map(|annotation| {
            (
                annotation,
                clean_for_matching(&annotation.text, strip_return_links),
            )
        })
        .collect::<Vec<_>>();

    for annotation in annotations {
        let Some((inner, cleaned, has_continuation_marker)) =
            underlines.get(&annotation.order)
        else {
            continue;
        };
        if *has_continuation_marker {
            continue;
        }

        let needle = match_key(cleaned);
        if needle.chars.is_empty() {
            plan.standalone.insert(annotation.order);
            continue;
        }
        let single_word = !inner.trim().chars().any(char::is_whitespace);
        let mut hosts = Vec::new();
        for (candidate, candidate_text) in &candidates {
            if !same_page(annotation, candidate) {
                continue;
            }
            let candidate_key = match_key(candidate_text);
            let occurrences = matching_occurrences(
                &candidate_key,
                &needle,
                candidate_text,
                single_word,
            );
            if !occurrences.is_empty() {
                hosts.push((candidate.order, occurrences.len()));
            }
        }

        match hosts.as_slice() {
            [(host_order, 1)] => {
                plan.host_of.insert(annotation.order, *host_order);
            }
            _ => {
                plan.standalone.insert(annotation.order);
            }
        }
    }

    plan
}

pub(super) fn plan_annotation_folds(
    annotations: &[SidecarAnnotation],
    strip_return_links: bool,
) -> AnnotationFolds {
    let underlines = plan_underlines(annotations, strip_return_links);
    let continuation_input = annotations
        .iter()
        .filter(|annotation| {
            !underlines.host_of.contains_key(&annotation.order)
        })
        .cloned()
        .collect::<Vec<_>>();
    let continuations = plan_continuations(&continuation_input);
    AnnotationFolds {
        underlines,
        continuations,
    }
}

/// Char ranges in `quote` to embolden for one absorbed underline.
pub(super) fn emphasis_ranges(
    quote: &str,
    member_texts: &[String],
    host_member: usize,
    underline_text: &str,
) -> Vec<Range<usize>> {
    let quote_key = match_key(quote);
    if quote_key.chars.is_empty() || host_member >= member_texts.len() {
        return Vec::new();
    }

    let mut member_windows = Vec::with_capacity(member_texts.len());
    let mut cursor = 0usize;
    for member in member_texts {
        let member_key = match_key(member);
        if member_key.chars.is_empty() {
            return Vec::new();
        }
        let Some(start) = find_key(&quote_key.chars, &member_key.chars, cursor)
        else {
            return Vec::new();
        };
        let end = start + member_key.chars.len();
        member_windows.push(start..end);
        cursor = end;
    }

    let host_window = &member_windows[host_member];
    let underline_key = match_key(underline_text);
    let Some(match_start) = find_unique_key(
        &quote_key.chars[host_window.clone()],
        &underline_key.chars,
    ) else {
        return Vec::new();
    };
    let key_start = host_window.start + match_start;
    let key_end = key_start + underline_key.chars.len();
    let chars = quote.chars().collect::<Vec<_>>();
    let mut start = quote_key.offsets[key_start];
    let mut end = quote_key.offsets[key_end - 1] + 1;

    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }

    let mut ranges = Vec::new();
    let mut line_start = start;
    while line_start < end {
        let line_end = chars[line_start..end]
            .iter()
            .position(|character| *character == '\n')
            .map_or(end, |offset| line_start + offset);
        let mut run_start = line_start;
        let mut run_end = line_end;
        while run_start < run_end && chars[run_start].is_whitespace() {
            run_start += 1;
        }
        while run_end > run_start && chars[run_end - 1].is_whitespace() {
            run_end -= 1;
        }
        if run_start < run_end {
            (run_start, run_end) =
                widen_for_flanking(&chars, run_start, run_end);
            ranges.push(run_start..run_end);
        }
        line_start = line_end.saturating_add(1);
    }
    merge_emphasis_ranges(&chars, ranges)
}

/// Merge emphasis ranges and insert their Markdown delimiters.
pub(super) fn apply_emphasis(quote: &str, ranges: &[Range<usize>]) -> String {
    let (quote, ranges) = fold_existing_emphasis(quote, ranges);
    let chars = quote.chars().collect::<Vec<_>>();
    let ranges = merge_emphasis_ranges(&chars, ranges);
    let byte_offsets = char_byte_offsets(&quote);
    let mut rendered = quote;
    for range in ranges.into_iter().rev() {
        rendered.insert_str(byte_offsets[range.end], "**");
        rendered.insert_str(byte_offsets[range.start], "**");
    }
    rendered
}

fn fold_existing_emphasis(
    quote: &str,
    ranges: &[Range<usize>],
) -> (String, Vec<Range<usize>>) {
    let chars = quote.chars().collect::<Vec<_>>();
    let mut requested = ranges.to_vec();
    let mut removed = BTreeSet::new();
    for (open, close) in paired_emphasis_delimiters(&chars) {
        for range in &mut requested {
            if range.start < close + 2 && range.end > open {
                range.start = range.start.min(open);
                range.end = range.end.max(close + 2);
                removed.insert(open);
                removed.insert(open + 1);
                removed.insert(close);
                removed.insert(close + 1);
            }
        }
    }
    if removed.is_empty() {
        return (quote.to_string(), requested);
    }

    let mut transformed = String::new();
    let mut old_to_new = vec![0usize; chars.len() + 1];
    let mut new_char_count = 0usize;
    for (index, character) in chars.iter().copied().enumerate() {
        old_to_new[index] = new_char_count;
        if !removed.contains(&index) {
            transformed.push(character);
            new_char_count += 1;
        }
    }
    old_to_new[chars.len()] = new_char_count;
    let transformed_chars = transformed.chars().collect::<Vec<_>>();
    let adjusted = requested
        .into_iter()
        .map(|range| {
            let start = old_to_new[range.start.min(chars.len())];
            let end = old_to_new[range.end.min(chars.len())];
            let (start, end) =
                widen_for_flanking(&transformed_chars, start, end);
            start..end
        })
        .collect::<Vec<_>>();
    (transformed, adjusted)
}

fn paired_emphasis_delimiters(chars: &[char]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut index = 0usize;
    while index + 1 < chars.len() {
        if chars[index] == '*' && chars[index + 1] == '*' {
            starts.push(index);
            index += 2;
        } else {
            index += 1;
        }
    }
    starts
        .chunks_exact(2)
        .filter_map(|pair| {
            (pair[1] > pair[0] + 2).then_some((pair[0], pair[1]))
        })
        .collect()
}

/// One healed `**…**` run per non-empty line for a standalone underline.
pub(super) fn standalone_underline_quote(cleaned_inner: &str) -> String {
    cleaned_inner
        .split('\n')
        .map(str::trim)
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("**{line}**")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn clean_for_matching(
    text: &str,
    strip_return_links: bool,
) -> String {
    let text = if strip_return_links {
        strip_return_link_glyphs(text)
    } else {
        text.to_string()
    };
    beautify_annotation_text(&text)
}

fn match_key(text: &str) -> MatchKey {
    let mut key = MatchKey::default();
    for (offset, character) in text.chars().enumerate() {
        if character.is_whitespace() || is_ignored_hyphen(character) {
            continue;
        }
        key.chars.push(character);
        key.offsets.push(offset);
    }
    key.offsets.push(text.chars().count());
    key
}

fn is_ignored_hyphen(character: char) -> bool {
    matches!(character, '-' | '\u{00ad}' | '\u{2010}' | '\u{2011}')
}

fn same_page(left: &SidecarAnnotation, right: &SidecarAnnotation) -> bool {
    match (left.page_index, right.page_index) {
        (Some(left), Some(right)) => left == right,
        _ => left.page_label == right.page_label,
    }
}

fn matching_occurrences(
    haystack: &MatchKey,
    needle: &MatchKey,
    cleaned_haystack: &str,
    single_word: bool,
) -> Vec<usize> {
    if needle.chars.is_empty() || needle.chars.len() > haystack.chars.len() {
        return Vec::new();
    }
    let cleaned_chars = cleaned_haystack.chars().collect::<Vec<_>>();
    haystack
        .chars
        .windows(needle.chars.len())
        .enumerate()
        .filter_map(|(start, window)| {
            if window != needle.chars {
                return None;
            }
            if single_word {
                let char_start = haystack.offsets[start];
                let char_end =
                    haystack.offsets[start + needle.chars.len() - 1] + 1;
                if (char_start > 0
                    && is_word_char(cleaned_chars[char_start - 1]))
                    || (char_end < cleaned_chars.len()
                        && is_word_char(cleaned_chars[char_end]))
                {
                    return None;
                }
            }
            Some(start)
        })
        .collect()
}

fn find_key(haystack: &[char], needle: &[char], start: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .get(start..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn find_unique_key(haystack: &[char], needle: &[char]) -> Option<usize> {
    let first = find_key(haystack, needle, 0)?;
    find_key(haystack, needle, first + 1)
        .is_none()
        .then_some(first)
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '’' | '\'')
}

fn is_punctuation_or_symbol(character: char) -> bool {
    !character.is_alphanumeric() && !character.is_whitespace()
}

fn left_flanking(chars: &[char], start: usize) -> bool {
    let Some(after) = chars.get(start).copied() else {
        return false;
    };
    if after.is_whitespace() {
        return false;
    }
    let before = start.checked_sub(1).and_then(|index| chars.get(index));
    !is_punctuation_or_symbol(after)
        || before.is_none_or(|character| {
            character.is_whitespace() || is_punctuation_or_symbol(*character)
        })
}

fn right_flanking(chars: &[char], end: usize) -> bool {
    let Some(before) = end.checked_sub(1).and_then(|index| chars.get(index))
    else {
        return false;
    };
    if before.is_whitespace() {
        return false;
    }
    let after = chars.get(end);
    !is_punctuation_or_symbol(*before)
        || after.is_none_or(|character| {
            character.is_whitespace() || is_punctuation_or_symbol(*character)
        })
}

fn widen_for_flanking(
    chars: &[char],
    mut start: usize,
    mut end: usize,
) -> (usize, usize) {
    if !left_flanking(chars, start) {
        while start > 0
            && !chars[start - 1].is_whitespace()
            && chars[start - 1] != '\n'
        {
            start -= 1;
        }
    }
    if !right_flanking(chars, end) {
        while end < chars.len()
            && !chars[end].is_whitespace()
            && chars[end] != '\n'
        {
            end += 1;
        }
    }
    (start, end)
}

pub(super) fn merge_emphasis_ranges(
    chars: &[char],
    mut ranges: Vec<Range<usize>>,
) -> Vec<Range<usize>> {
    ranges.retain(|range| range.start < range.end && range.end <= chars.len());
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        let Some(previous) = merged.last_mut() else {
            merged.push(range);
            continue;
        };
        let whitespace_gap = range.start >= previous.end
            && chars[previous.end..range.start].iter().all(|character| {
                character.is_whitespace() && *character != '\n'
            });
        if range.start <= previous.end || whitespace_gap {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn char_byte_offsets(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect()
}
