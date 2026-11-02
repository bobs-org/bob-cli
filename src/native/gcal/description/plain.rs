//! Linear plain-text tokenizer for Google Calendar descriptions.

use super::{
    normalize_plain_chars, Inline, LineMarker, SourceKind, SourceLine,
};

pub(super) fn tokenize(raw: &str) -> Vec<SourceLine> {
    let normalized = normalize_plain_chars(raw);
    let mut lines: Vec<SourceLine> =
        normalized.split('\n').map(tokenize_line).collect();
    assign_list_depths(&mut lines);
    lines
}

fn tokenize_line(line: &str) -> SourceLine {
    if line.trim().is_empty() {
        return SourceLine::blank();
    }
    let (indent, rest) = split_indent(line);
    if let Some((marker, body)) = parse_list_marker(rest) {
        SourceLine::list_item(marker, indent, parse_inlines(body))
    } else {
        SourceLine::paragraph(parse_inlines(line.trim_start()))
    }
}

/// Store indent columns in `depth` until [`assign_list_depths`].
fn split_indent(line: &str) -> (u8, &str) {
    let mut cols: u32 = 0;
    let mut rest_at = 0;
    for (index, ch) in line.char_indices() {
        match ch {
            ' ' => cols += 1,
            '\t' => cols = (cols / 4 + 1) * 4,
            _ => return (cols.min(u32::from(u8::MAX)) as u8, &line[index..]),
        }
        rest_at = index + ch.len_utf8();
    }
    (cols.min(u32::from(u8::MAX)) as u8, &line[rest_at..])
}

fn parse_list_marker(rest: &str) -> Option<(LineMarker, &str)> {
    const BULLETS: &[char] = &['-', '*', '•', '◦', '‣', '▪'];
    let mut chars = rest.chars();
    let first = chars.next()?;
    if BULLETS.contains(&first) {
        return chars
            .next()
            .filter(|ch| *ch == ' ')
            .map(|_| (LineMarker::Bullet, chars.as_str()));
    }
    if !first.is_ascii_digit() {
        return None;
    }
    let digits = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    let number: u32 = rest[..digits].parse().ok()?;
    let after = &rest[digits..];
    let punct = after.chars().next()?;
    if punct != '.' && punct != ')' {
        return None;
    }
    let rest = after[punct.len_utf8()..].strip_prefix(' ')?;
    Some((LineMarker::Ordered(number), rest))
}

fn assign_list_depths(lines: &mut [SourceLine]) {
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].is_list_item() {
            index += 1;
            continue;
        }
        let start = index;
        while index < lines.len() && lines[index].is_list_item() {
            index += 1;
        }
        let mut levels: Vec<u8> =
            lines[start..index].iter().filter_map(list_indent).collect();
        levels.sort_unstable();
        levels.dedup();
        for line in &mut lines[start..index] {
            if let SourceKind::ListItem { depth, .. } = &mut line.kind {
                let indent = *depth;
                *depth = u8::try_from(
                    levels
                        .iter()
                        .position(|level| *level == indent)
                        .unwrap_or(0)
                        + 1,
                )
                .unwrap_or(1);
            }
        }
    }
}

fn list_indent(line: &SourceLine) -> Option<u8> {
    match line.kind {
        SourceKind::ListItem { depth, .. } => Some(depth),
        _ => None,
    }
}

fn parse_inlines(input: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        if let Some((inline, tail)) = take_special(rest) {
            let inline = apply_outlook(&mut out, inline);
            push_inline(&mut out, inline);
            rest = tail;
            continue;
        }
        let take = match find_next_potential(rest) {
            Some(0) => rest.chars().next().map_or(1, char::len_utf8),
            Some(index) => index,
            None => rest.len(),
        };
        push_text(&mut out, &rest[..take]);
        rest = &rest[take..];
    }
    out
}

fn find_next_potential(input: &str) -> Option<usize> {
    input.char_indices().find_map(|(index, _)| {
        let slice = &input[index..];
        if slice.starts_with("![")
            || slice.starts_with('[')
            || slice.starts_with('<')
            || slice.starts_with("http://")
            || slice.starts_with("https://")
        {
            Some(index)
        } else {
            None
        }
    })
}

fn take_special(input: &str) -> Option<(Inline, &str)> {
    take_image(input)
        .or_else(|| take_wikilink(input))
        .or_else(|| take_markdown_link(input))
        .or_else(|| take_bracket_url(input))
        .or_else(|| take_bare_url(input))
}

/// Keep a wikilink as text so a later Markdown image/link cannot absorb
/// it as part of a larger `[label](url)` token. The renderer escapes its
/// brackets before writing it into the vault.
fn take_wikilink(input: &str) -> Option<(Inline, &str)> {
    let rest = input.strip_prefix("[[")?;
    let end = rest.find("]]")? + 2;
    Some((
        Inline::Text {
            text: format!("[[{}", &rest[..end]),
            bold: false,
            italic: false,
        },
        &rest[end..],
    ))
}

fn take_image(input: &str) -> Option<(Inline, &str)> {
    let rest = input.strip_prefix("![")?;
    let (alt, url, tail) = split_label_and_url(rest)?;
    let _ = url;
    Some((
        Inline::Text {
            text: alt,
            bold: false,
            italic: false,
        },
        tail,
    ))
}

fn take_markdown_link(input: &str) -> Option<(Inline, &str)> {
    let rest = input.strip_prefix('[')?;
    let (text, url, tail) = split_label_and_url(rest)?;
    Some((Inline::Link { text, url }, tail))
}

fn split_label_and_url(input: &str) -> Option<(String, String, &str)> {
    let bracket = input.find("](")?;
    let text = input[..bracket].to_string();
    let rest = &input[bracket + 2..];
    let end = rest.find(')')?;
    Some((text, rest[..end].to_string(), &rest[end + 1..]))
}

fn take_bracket_url(input: &str) -> Option<(Inline, &str)> {
    let rest = input.strip_prefix('<')?;
    let end = rest.find('>')?;
    let inner = &rest[..end];
    if !looks_like_url(inner) {
        return None;
    }
    let url = inner.to_string();
    Some((
        Inline::Link {
            text: url.clone(),
            url,
        },
        &rest[end + 1..],
    ))
}

fn take_bare_url(input: &str) -> Option<(Inline, &str)> {
    if !looks_like_url(input) {
        return None;
    }
    let end = input
        .find(|ch: char| ch.is_whitespace() || ch == '<' || ch == '[')
        .unwrap_or(input.len());
    let (raw, tail) = input.split_at(end);
    let url = trim_url_trail(raw);
    if url.is_empty() {
        return None;
    }
    Some((
        Inline::Link {
            text: url.clone(),
            url,
        },
        tail,
    ))
}

fn apply_outlook(out: &mut Vec<Inline>, inline: Inline) -> Inline {
    let Inline::Link { text, url } = &inline else {
        return inline;
    };
    if text != url {
        return inline;
    }
    let Some(Inline::Text { text: previous, .. }) = out.last_mut() else {
        return inline;
    };
    if previous.ends_with(char::is_whitespace) || previous.is_empty() {
        return inline;
    }
    let word = if let Some(space_at) = previous.rfind(char::is_whitespace) {
        let space_len = previous[space_at..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);
        let word = previous[space_at + space_len..].to_string();
        previous.truncate(space_at + space_len);
        word
    } else {
        std::mem::take(previous)
    };
    if word.is_empty() {
        return inline;
    }
    if previous.is_empty() {
        out.pop();
    }
    Inline::Link {
        text: word,
        url: url.clone(),
    }
}

fn looks_like_url(text: &str) -> bool {
    text.starts_with("http://") || text.starts_with("https://")
}

fn trim_url_trail(raw: &str) -> String {
    let mut url = raw.to_string();
    while url
        .chars()
        .last()
        .is_some_and(|ch| matches!(ch, '.' | ',' | ';' | ':' | '!' | '?'))
    {
        url.pop();
    }
    while url.ends_with(')') {
        let opens = url.chars().filter(|ch| *ch == '(').count();
        let closes = url.chars().filter(|ch| *ch == ')').count();
        if closes <= opens {
            break;
        }
        url.pop();
    }
    url
}

fn push_text(out: &mut Vec<Inline>, text: &str) {
    if text.is_empty() {
        return;
    }
    push_inline(
        out,
        Inline::Text {
            text: text.to_string(),
            bold: false,
            italic: false,
        },
    );
}

fn push_inline(out: &mut Vec<Inline>, inline: Inline) {
    match (out.last_mut(), inline) {
        (
            Some(Inline::Text { text, bold, italic }),
            Inline::Text {
                text: more,
                bold: bold2,
                italic: italic2,
            },
        ) if *bold == bold2 && *italic == italic2 => text.push_str(&more),
        (_, inline) => out.push(inline),
    }
}
