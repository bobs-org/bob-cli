//! Tag spans, inline fields, descriptions, and whitespace helpers.
use super::*;

pub(super) fn contains_task_tag(text: &str) -> bool {
    contains_tag(text, "#task")
}

pub(super) fn contains_hide_tag(text: &str) -> bool {
    contains_tag(text, HIDE_TAG)
}

pub(super) fn contains_tag(text: &str, tag: &str) -> bool {
    tag_span(text, tag).is_some()
}

pub(super) fn tag_spans(text: &str, tag: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let Some((start, end)) = tag_span(&text[offset..], tag) else {
            break;
        };
        spans.push((offset + start, offset + end));
        offset += end;
    }
    spans
}

/// Locates `tag` as a whole token, honoring the same boundaries as the Tasks
/// plugin so substrings like `#taskish` or `#hidden` are not matched.
pub(super) fn tag_span(text: &str, tag: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    while let Some(relative_index) = text[offset..].find(tag) {
        let index = offset + relative_index;
        let end = index + tag.len();
        let before = text[..index].chars().next_back();
        let after = text[end..].chars().next();
        if before.is_none_or(is_task_tag_left_boundary)
            && after.is_none_or(is_task_tag_right_boundary)
        {
            return Some((index, end));
        }
        offset = index + 1;
    }
    None
}

pub(super) fn hide_tag_span(line_text: &str) -> Option<(usize, usize)> {
    tag_span(line_text, HIDE_TAG)
}

pub(super) fn is_task_tag_left_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, '(' | '[' | '{')
}

pub(super) fn is_task_tag_right_boundary(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ']' | ')' | '}' | ':' | '.' | ',' | ';' | '!' | '?'
        )
}

pub(super) fn has_trailing_prj_anchor(line: &str) -> bool {
    let trimmed = line.trim_end();
    let Some(before_anchor) = trimmed.strip_suffix("^prj") else {
        return false;
    };
    before_anchor.is_empty()
        || before_anchor
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
}

pub(super) fn markdown_fence_line(
    line: &str,
    fence: &mut Option<(char, usize)>,
) -> bool {
    let trimmed = line.trim_start_matches(' ');
    let indentation = line.len() - trimmed.len();
    if indentation > 3 {
        return fence.is_some();
    }

    if let Some((marker, opening_length)) = *fence {
        let marker_count =
            trimmed.chars().take_while(|ch| ch == &marker).count();
        if marker_count >= opening_length
            && trimmed[marker_count..].trim().is_empty()
        {
            *fence = None;
        }
        return true;
    }

    let Some(marker) = trimmed.chars().next() else {
        return false;
    };
    if !matches!(marker, '`' | '~') {
        return false;
    }
    let marker_count = trimmed.chars().take_while(|ch| ch == &marker).count();
    if marker_count < 3 {
        return false;
    }
    *fence = Some((marker, marker_count));
    true
}

pub(super) fn inline_field_value(text: &str, key: &str) -> Option<String> {
    inline_field_span(text, key).map(|field| {
        text[field.value_start..field.value_end].trim().to_string()
    })
}

pub(super) fn inline_field_span(
    text: &str,
    key: &str,
) -> Option<InlineFieldSpan> {
    inline_field_spans(text, key).into_iter().next()
}

pub(super) fn inline_field_spans(
    text: &str,
    key: &str,
) -> Vec<InlineFieldSpan> {
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let square = text[offset..].find('[');
        let parenthesis = text[offset..].find('(');
        let Some(open_relative) = (match (square, parenthesis) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        }) else {
            break;
        };
        let open = offset + open_relative;
        let opening = text.as_bytes()[open];
        let closing = if opening == b'[' { ']' } else { ')' };
        let Some(close_relative) = text[open + 1..].find(closing) else {
            offset = open + 1;
            continue;
        };
        let close = open + 1 + close_relative;
        let inner = &text[open + 1..close];
        if let Some((field_key, value)) = inner.split_once("::")
            && field_key.trim() == key
        {
            let value_start = open + 1 + field_key.len() + "::".len();
            fields.push(InlineFieldSpan {
                start: open,
                end: close + 1,
                value_start,
                value_end: value_start + value.len(),
            });
        }
        offset = close + 1;
    }
    fields
}

pub(super) fn parse_inline_schedule_date(value: &str) -> Option<NaiveDate> {
    is_exact_date_shape(value)
        .then(|| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .flatten()
}

pub(super) fn task_description(text: &str) -> String {
    let without_anchor = strip_trailing_block_id(text);
    let without_fields = remove_inline_fields(without_anchor);
    without_fields
        .split_whitespace()
        .filter(|token| {
            *token != "#task"
                && *token != PROJECT_TASK_TAG
                && *token != HIDE_TAG
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn strip_trailing_block_id(text: &str) -> &str {
    let trimmed = text.trim_end();
    let Some(anchor_start) = trimmed.rfind('^') else {
        return trimmed;
    };
    if anchor_start > 0
        && !trimmed[..anchor_start]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    {
        return trimmed;
    }

    let anchor = &trimmed[anchor_start + 1..];
    if anchor.is_empty() || anchor.chars().any(char::is_whitespace) {
        return trimmed;
    }

    trimmed[..anchor_start].trim_end()
}

pub(super) fn remove_inline_fields(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut offset = 0;
    while offset < text.len() {
        let square = text[offset..].find('[');
        let parenthesis = text[offset..].find('(');
        let Some(open_relative) = (match (square, parenthesis) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        }) else {
            break;
        };
        let open = offset + open_relative;
        let opening = text.as_bytes()[open];
        let closing = if opening == b'[' { ']' } else { ')' };
        let Some(close_relative) = text[open + 1..].find(closing) else {
            output.push_str(&text[offset..=open]);
            offset = open + 1;
            continue;
        };
        let close = open + 1 + close_relative;
        let inner = &text[open + 1..close];
        if inner.contains("::") {
            output.push_str(&text[offset..open]);
            offset = close + 1;
            continue;
        }
        output.push_str(&text[offset..=close]);
        offset = close + 1;
    }
    output.push_str(&text[offset..]);
    output
}
