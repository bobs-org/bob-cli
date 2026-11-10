//! Structured Google Calendar description parse.
//!
//! One linear HTML or plain-text tokenize feeds both directive verdicts
//! and the bullet renderer, so a line cannot count as both a
//! directive and a bullet, or as neither. `EventDescription` stores the
//! outlined lines unescaped; its `Debug` impl prints counts only.

use std::fmt;

use crate::native::{
    clock_tags::{google_clock_token, merge_clock_tags, ClockTag},
    event_task::{parse_buffer_token, BufferPart, EventBuffer},
};

use super::model::parse_route_token;

mod boilerplate;
pub(crate) mod bullets;
mod html;
mod plain;

/// Inputs later filter passes use to drop badge duplicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DescriptionContext<'a> {
    pub join_link: Option<&'a str>,
    pub location: Option<&'a str>,
}

/// Combined route, untracked, and buffer from directive lines. The first
/// route and the first value per buffer side win. Per-side `_set` flags
/// distinguish "set to 0" from unset so a private note can clear a
/// Google buffer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DescriptionDirectives {
    pub route: Option<String>,
    pub clock_tag: ClockTag,
    /// Deprecated compatibility mirror. It is true only when explicit off
    /// wins, so a conflicting `#on` plus alias remains on-clock.
    pub untracked: bool,
    pub buffer: EventBuffer,
    pub before_set: bool,
    pub after_set: bool,
}

/// One labelled buffer reason: the minutes for one side plus the
/// rendered reason inlines (links preserved, tags inert).
#[derive(Clone, PartialEq, Eq, Default)]
pub(crate) struct BufferReason {
    pub minutes: u16,
    pub inlines: Vec<Inline>,
}

impl fmt::Debug for BufferReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferReason")
            .field("minutes", &self.minutes)
            .field("inlines", &self.inlines.len())
            .finish()
    }
}

/// Labelled reasons for each buffer side, at most one per side.
#[derive(Clone, PartialEq, Eq, Default)]
pub(crate) struct BufferReasons {
    pub before: Option<BufferReason>,
    pub after: Option<BufferReason>,
}

impl fmt::Debug for BufferReasons {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferReasons")
            .field("before", &self.before.is_some())
            .field("after", &self.after.is_some())
            .finish()
    }
}

impl BufferReasons {
    pub(crate) fn is_empty(&self) -> bool {
        self.before.is_none() && self.after.is_none()
    }

    pub(crate) fn count(&self) -> usize {
        usize::from(self.before.is_some()) + usize::from(self.after.is_some())
    }

    pub(crate) fn iter(
        &self,
    ) -> impl Iterator<Item = (BufferPart, &BufferReason)> {
        self.before
            .as_ref()
            .map(|reason| (BufferPart::Before, reason))
            .into_iter()
            .chain(
                self.after
                    .as_ref()
                    .map(|reason| (BufferPart::After, reason)),
            )
    }
}

/// Parsed description: outlined lines, directive verdicts, size flag.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct EventDescription {
    pub lines: Vec<DescriptionLine>,
    /// Private-note prose, rendered above Google's copied lines and
    /// exempt from `description_lines`.
    pub note_lines: Vec<DescriptionLine>,
    pub directives: DescriptionDirectives,
    pub buffer_reasons: BufferReasons,
    pub oversized: bool,
}

impl fmt::Debug for EventDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventDescription")
            .field("lines", &self.lines.len())
            .field("note_lines", &self.note_lines.len())
            .field("directives", &self.directives)
            .field("buffer_reasons", &self.buffer_reasons.count())
            .field("oversized", &self.oversized)
            .finish()
    }
}

/// One outlined description line, still unescaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DescriptionLine {
    pub depth: u8,
    pub marker: LineMarker,
    pub inlines: Vec<Inline>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineMarker {
    Bullet,
    Ordered(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Inline {
    Text {
        text: String,
        bold: bool,
        italic: bool,
    },
    Link {
        text: String,
        url: String,
    },
}

/// Tokenized line before filter and outline. Blank and separator lines
/// stay until [`filter_source_lines`], where the boilerplate passes run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceLine {
    kind: SourceKind,
    inlines: Vec<Inline>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceKind {
    Blank,
    Separator,
    Paragraph,
    ListItem { marker: LineMarker, depth: u8 },
}

/// Parse `raw` into outlined lines and the same directive verdicts the
/// previous HTML-normalizing scanner produced.
pub(crate) fn parse_description(
    raw: &str,
    ctx: &DescriptionContext<'_>,
) -> EventDescription {
    let oversized = raw.len() > 65_536;
    let source = if looks_like_html(raw) {
        html::tokenize(raw)
    } else {
        plain::tokenize(raw)
    };
    let (directives, buffer_reasons, consumed) =
        directives_from_source(&source);
    let filtered = filter_source_lines(source, &consumed, ctx);
    EventDescription {
        lines: outline(filtered),
        note_lines: Vec::new(),
        directives,
        buffer_reasons,
        oversized,
    }
}

/// Parse private-note text on the plain-text tokenizer, never the HTML
/// sniffing path. Prose lands in `lines` so [`GcalEvent::apply_notes`]
/// can copy it onto `note_lines`.
pub(crate) fn parse_note(
    raw: &str,
    ctx: &DescriptionContext<'_>,
) -> EventDescription {
    let oversized = raw.len() > 65_536;
    let source = plain::tokenize(raw);
    let (directives, buffer_reasons, consumed) =
        directives_from_source(&source);
    let filtered = filter_source_lines(source, &consumed, ctx);
    EventDescription {
        lines: outline(filtered),
        note_lines: Vec::new(),
        directives,
        buffer_reasons,
        oversized,
    }
}

/// Reimplement today's scanner on top of [`parse_description`].
#[cfg(test)]
pub(crate) fn description_directives(
    description: &str,
) -> DescriptionDirectives {
    parse_description(
        description,
        &DescriptionContext {
            join_link: None,
            location: None,
        },
    )
    .directives
}

/// Drop directive lines and consumed reason lines, then separator,
/// badge-duplicate, and vendor-block lines while blank and separator
/// lines are still present.
fn filter_source_lines(
    lines: Vec<SourceLine>,
    consumed: &[bool],
    ctx: &DescriptionContext<'_>,
) -> Vec<SourceLine> {
    let lines: Vec<SourceLine> = lines
        .into_iter()
        .enumerate()
        .filter(|(index, line)| {
            !is_directive_line(line)
                && !consumed.get(*index).copied().unwrap_or(false)
        })
        .map(|(_, line)| line)
        .collect();
    boilerplate::filter(lines, ctx)
}

fn outline(lines: Vec<SourceLine>) -> Vec<DescriptionLine> {
    let content: Vec<SourceLine> =
        lines.into_iter().filter(|line| line.is_content()).collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < content.len() {
        if content[index].is_list_item() {
            let bump = out.last().is_some_and(|(lead_in, _)| *lead_in);
            let start = index;
            while index < content.len() && content[index].is_list_item() {
                index += 1;
            }
            for item in &content[start..index] {
                let SourceKind::ListItem { marker, depth } = item.kind else {
                    continue;
                };
                let depth = if bump { depth.saturating_add(1) } else { depth };
                if let Some(line) =
                    finish_line(depth, marker, item.inlines.clone(), true)
                {
                    out.push((false, line));
                }
            }
        } else {
            let lead_in = content[index].is_lead_in();
            if let Some(line) = finish_line(
                1,
                LineMarker::Bullet,
                content[index].inlines.clone(),
                false,
            ) {
                out.push((lead_in, line));
            }
            index += 1;
        }
    }
    out.into_iter().map(|(_, line)| line).collect()
}

fn finish_line(
    depth: u8,
    marker: LineMarker,
    mut inlines: Vec<Inline>,
    strip_checkbox: bool,
) -> Option<DescriptionLine> {
    if strip_checkbox {
        strip_leading_checkbox(&mut inlines);
    }
    let inlines = merge_and_trim(inlines);
    if inlines.is_empty() {
        return None;
    }
    Some(DescriptionLine {
        depth: depth.clamp(1, 2),
        marker,
        inlines,
    })
}

const CHECKBOX_GLYPHS: &[&str] = &["[ ]", "[x]", "[X]", "☐", "☑", "☒"];

fn strip_leading_checkbox(inlines: &mut [Inline]) {
    let Some(Inline::Text { text, .. }) = inlines.first_mut() else {
        return;
    };
    let trimmed = text.trim_start();
    for glyph in CHECKBOX_GLYPHS {
        if let Some(rest) = trimmed.strip_prefix(glyph) {
            *text = rest.trim_start().to_string();
            return;
        }
    }
}

fn merge_and_trim(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut merged: Vec<Inline> = Vec::new();
    for inline in inlines {
        match (merged.last_mut(), inline) {
            (
                Some(Inline::Text { text, bold, italic }),
                Inline::Text {
                    text: more,
                    bold: bold2,
                    italic: italic2,
                },
            ) if *bold == bold2 && *italic == italic2 => {
                text.push_str(&more);
            }
            (_, inline) => merged.push(inline),
        }
    }
    trim_edge_text(&mut merged);
    merged.retain(|inline| match inline {
        Inline::Text { text, .. } => !text.is_empty(),
        Inline::Link { text, url } => !text.is_empty() || !url.is_empty(),
    });
    trim_edge_text(&mut merged);
    merged.retain(|inline| match inline {
        Inline::Text { text, .. } => !text.is_empty(),
        Inline::Link { text, url } => !text.is_empty() || !url.is_empty(),
    });
    merged
}

fn trim_edge_text(inlines: &mut [Inline]) {
    if let Some(Inline::Text { text, .. }) = inlines.first_mut() {
        *text = text.trim_start().to_string();
    }
    if let Some(Inline::Text { text, .. }) = inlines.last_mut() {
        *text = text.trim_end().to_string();
    }
}

fn directives_from_source(
    lines: &[SourceLine],
) -> (DescriptionDirectives, BufferReasons, Vec<bool>) {
    let mut out = DescriptionDirectives::default();
    let mut before_set = false;
    let mut after_set = false;
    let mut reasons = BufferReasons::default();
    let mut consumed = vec![false; lines.len()];
    for (index, line) in lines.iter().enumerate() {
        if is_directive_line(line) {
            for token in project_text(line)
                .split(|ch: char| ch.is_whitespace() || ch == ',')
                .filter(|token| !token.is_empty())
            {
                if let Some(route) = parse_route_token(token) {
                    if out.route.is_none() {
                        out.route = Some(route);
                    }
                    continue;
                }
                if let Some(clock_tag) = google_clock_token(token) {
                    out.clock_tag =
                        merge_clock_tags(&out.clock_tag, &clock_tag);
                    continue;
                }
                let Some((part, minutes)) = parse_buffer_token(token) else {
                    continue;
                };
                match part {
                    BufferPart::Before if !before_set => {
                        out.buffer.before_min = minutes;
                        before_set = true;
                    }
                    BufferPart::After if !after_set => {
                        out.buffer.after_min = minutes;
                        after_set = true;
                    }
                    _ => {}
                }
            }
            continue;
        }
        if !line.is_content() {
            continue;
        }
        let Some((part, minutes, inlines)) = reason_line(line) else {
            continue;
        };
        match part {
            BufferPart::Before if !before_set => {
                out.buffer.before_min = minutes;
                before_set = true;
                reasons.before = Some(BufferReason { minutes, inlines });
                consumed[index] = true;
            }
            BufferPart::After if !after_set => {
                out.buffer.after_min = minutes;
                after_set = true;
                reasons.after = Some(BufferReason { minutes, inlines });
                consumed[index] = true;
            }
            BufferPart::Before
                if out.buffer.before_min == minutes
                    && reasons.before.is_none() =>
            {
                reasons.before = Some(BufferReason { minutes, inlines });
                consumed[index] = true;
            }
            BufferPart::After
                if out.buffer.after_min == minutes
                    && reasons.after.is_none() =>
            {
                reasons.after = Some(BufferReason { minutes, inlines });
                consumed[index] = true;
            }
            _ => {}
        }
    }
    out.untracked = out.clock_tag.explicit_off();
    out.before_set = before_set;
    out.after_set = after_set;
    (out, reasons, consumed)
}

fn reason_line(line: &SourceLine) -> Option<(BufferPart, u16, Vec<Inline>)> {
    if !line.is_content() {
        return None;
    }
    let projected = project_text(line);
    let chars: Vec<char> = projected.chars().collect();
    let mut pos = 0;
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }
    let token_start = pos;
    if pos >= chars.len() {
        return None;
    }
    match chars[pos] {
        '-' | '\u{2212}' | '\u{2013}' | '+' => {}
        _ => return None,
    }
    pos += 1;
    let digits_start = pos;
    while pos < chars.len() && chars[pos].is_ascii_digit() {
        pos += 1;
    }
    let digits_len = pos - digits_start;
    if digits_len == 0 || digits_len > 3 {
        return None;
    }
    if pos < chars.len() && matches!(chars[pos], 'm' | 'M') {
        pos += 1;
    }
    let token_text: String = chars[token_start..pos].iter().collect();
    let (part, minutes) = parse_buffer_token(&token_text)?;
    if pos >= chars.len() {
        return None;
    }
    let first_sep = chars[pos];
    if !(first_sep == ':' || first_sep == ',' || first_sep.is_whitespace()) {
        return None;
    }
    pos += 1;
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }
    if pos < chars.len()
        && matches!(chars[pos], '\u{2014}' | '\u{2013}' | '-' | ':')
    {
        let dash_pos = pos;
        pos += 1;
        let ws_start = pos;
        while pos < chars.len() && chars[pos].is_whitespace() {
            pos += 1;
        }
        if pos == ws_start {
            pos = dash_pos;
        }
    }
    let prefix_len = pos;
    let reason_text: String = chars[pos..].iter().collect();
    let trimmed = reason_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.chars().next()?;
    let first_ok = if first.is_alphabetic() {
        true
    } else if !first.is_ascii()
        && !matches!(first, '\u{2010}'..='\u{2015}' | '\u{2212}')
    {
        true
    } else {
        false
    };
    if !first_ok {
        return None;
    }
    let mut leading_text_len = 0usize;
    for inline in &line.inlines {
        match inline {
            Inline::Text { text, .. } => {
                leading_text_len += text.chars().count();
            }
            Inline::Link { .. } => break,
        }
    }
    if prefix_len > leading_text_len {
        return None;
    }
    let mut remaining = prefix_len;
    let mut rest: Vec<Inline> = Vec::new();
    for inline in &line.inlines {
        if remaining == 0 {
            rest.push(inline.clone());
            continue;
        }
        match inline {
            Inline::Text { text, bold, italic } => {
                let len = text.chars().count();
                if len <= remaining {
                    remaining -= len;
                } else {
                    let skipped: String =
                        text.chars().skip(remaining).collect();
                    remaining = 0;
                    rest.push(Inline::Text {
                        text: skipped,
                        bold: *bold,
                        italic: *italic,
                    });
                }
            }
            Inline::Link { .. } => {
                return None;
            }
        }
    }
    if remaining != 0 {
        return None;
    }
    let merged = merge_and_trim(rest);
    if merged.is_empty() {
        return None;
    }
    Some((part, minutes, merged))
}

fn is_directive_line(line: &SourceLine) -> bool {
    if !line.is_content() {
        return false;
    }
    let text = project_text(line);
    let tokens: Vec<&str> = text
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|token| !token.is_empty())
        .collect();
    !tokens.is_empty() && tokens.iter().copied().all(is_gcal_directive_token)
}

fn is_gcal_directive_token(token: &str) -> bool {
    parse_route_token(token).is_some()
        || google_clock_token(token).is_some()
        || parse_buffer_token(token).is_some()
}

fn project_text(line: &SourceLine) -> String {
    let mut out = String::new();
    for inline in &line.inlines {
        match inline {
            Inline::Text { text, .. } | Inline::Link { text, .. } => {
                out.push_str(text);
            }
        }
    }
    out
}

impl SourceLine {
    fn blank() -> Self {
        Self {
            kind: SourceKind::Blank,
            inlines: Vec::new(),
        }
    }

    fn separator() -> Self {
        Self {
            kind: SourceKind::Separator,
            inlines: Vec::new(),
        }
    }

    fn paragraph(inlines: Vec<Inline>) -> Self {
        Self {
            kind: SourceKind::Paragraph,
            inlines,
        }
    }

    fn list_item(marker: LineMarker, depth: u8, inlines: Vec<Inline>) -> Self {
        Self {
            kind: SourceKind::ListItem { marker, depth },
            inlines,
        }
    }

    fn is_list_item(&self) -> bool {
        matches!(self.kind, SourceKind::ListItem { .. })
    }

    pub(super) fn kind_is_blank(&self) -> bool {
        matches!(self.kind, SourceKind::Blank)
    }

    pub(super) fn kind_is_separator(&self) -> bool {
        matches!(self.kind, SourceKind::Separator)
    }

    pub(super) fn kind_is_list_item(&self) -> bool {
        self.is_list_item()
    }

    pub(super) fn kind_is_bullet_list_item(&self) -> bool {
        matches!(
            self.kind,
            SourceKind::ListItem {
                marker: LineMarker::Bullet,
                ..
            }
        )
    }

    pub(super) fn project_text(&self) -> String {
        project_text(self)
    }

    pub(super) fn inline_urls(&self) -> impl Iterator<Item = &str> {
        self.inlines.iter().filter_map(|inline| match inline {
            Inline::Link { url, .. } => Some(url.as_str()),
            Inline::Text { .. } => None,
        })
    }

    /// The URL when the line is a lone link with no other text.
    pub(super) fn only_link_url(&self) -> Option<&str> {
        let mut found = None;
        for inline in &self.inlines {
            match inline {
                Inline::Link { url, .. } => {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(url.as_str());
                }
                Inline::Text { text, .. } => {
                    if !text.trim().is_empty() {
                        return None;
                    }
                }
            }
        }
        found
    }

    fn is_content(&self) -> bool {
        !matches!(self.kind, SourceKind::Blank | SourceKind::Separator)
    }

    fn is_lead_in(&self) -> bool {
        !self.is_list_item() && (self.all_bold() || self.ends_with_colon())
    }

    fn all_bold(&self) -> bool {
        let mut any = false;
        for inline in &self.inlines {
            if let Inline::Text { text, bold, .. } = inline {
                if text.trim().is_empty() {
                    continue;
                }
                any = true;
                if !*bold {
                    return false;
                }
            }
        }
        any
    }

    fn ends_with_colon(&self) -> bool {
        project_text(self).trim_end().ends_with(':')
    }
}

fn looks_like_html(input: &str) -> bool {
    if input.contains("<!--") {
        return true;
    }
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        rest = rest.strip_prefix('/').unwrap_or(rest);
        let name_len = rest
            .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
            .unwrap_or(rest.len());
        if name_len > 0 && is_known_html_tag(&rest[..name_len]) {
            return true;
        }
    }
    false
}

fn is_known_html_tag(name: &str) -> bool {
    [
        "a",
        "b",
        "strong",
        "i",
        "em",
        "u",
        "s",
        "strike",
        "del",
        "br",
        "p",
        "div",
        "span",
        "font",
        "ul",
        "ol",
        "li",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "table",
        "thead",
        "tbody",
        "tr",
        "td",
        "th",
        "img",
        "hr",
        "html",
        "head",
        "body",
        "style",
        "script",
        "title",
        "html-blob",
    ]
    .iter()
    .any(|tag| name.eq_ignore_ascii_case(tag))
}

fn is_zero_width(ch: char) -> bool {
    matches!(ch, '\u{200B}'..='\u{200D}' | '\u{FEFF}')
}

/// Turn common HTML entities into characters. Byte-identical to the
/// previous `model::decode_html_entities` used for directive lines.
fn decode_html_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(';').unwrap_or(rest.len());
        if end == rest.len() {
            out.push_str(rest);
            return out;
        }
        let entity = &rest[1..end];
        let decoded = match entity {
            "nbsp" => Some(' '),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "#39" | "apos" => Some('\''),
            _ => decode_numeric_entity(entity),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_numeric_entity(entity: &str) -> Option<char> {
    let digits = if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        entity.strip_prefix('#')?.parse::<u32>().ok()?
    };
    char::from_u32(digits)
}

fn normalize_plain_chars(text: &str) -> String {
    decode_html_entities(text)
        .chars()
        .filter(|ch| !is_zero_width(*ch))
        .map(|ch| if ch == '\u{00A0}' { ' ' } else { ch })
        .collect::<String>()
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::clock_tags::ClockKind;
    use serde_json::Value;

    fn parse(raw: &str) -> EventDescription {
        parse_description(
            raw,
            &DescriptionContext {
                join_link: None,
                location: None,
            },
        )
    }

    fn parse_with_ctx(raw: &str, case: &Value) -> EventDescription {
        parse_description(
            raw,
            &DescriptionContext {
                join_link: case["join_link"].as_str(),
                location: case["location"].as_str(),
            },
        )
    }

    fn marker_label(marker: LineMarker) -> String {
        match marker {
            LineMarker::Bullet => "-".to_string(),
            LineMarker::Ordered(n) => format!("{n}."),
        }
    }

    fn assert_outlined_lines(
        id: &str,
        parsed: &EventDescription,
        expected_lines: &[Value],
    ) {
        assert_eq!(
            parsed.lines.len(),
            expected_lines.len(),
            "{id} line count: got {:#?}",
            parsed
                .lines
                .iter()
                .map(|line| (
                    line.depth,
                    marker_label(line.marker),
                    line.inlines.iter().map(inline_json).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        );
        for (line, expected) in parsed.lines.iter().zip(expected_lines.iter()) {
            assert_eq!(
                line.depth,
                u8::try_from(expected["depth"].as_u64().expect("depth"))
                    .expect("depth fits"),
                "{id} depth"
            );
            assert_eq!(
                marker_label(line.marker),
                expected["marker"].as_str().expect("marker"),
                "{id} marker"
            );
            let expected_inlines =
                expected["inlines"].as_array().expect("inlines");
            let got: Vec<Value> =
                line.inlines.iter().map(inline_json).collect();
            assert_eq!(got, *expected_inlines, "{id} inlines");
        }
    }

    fn inline_json(inline: &Inline) -> Value {
        match inline {
            Inline::Text { text, bold, italic } => {
                let mut map = serde_json::Map::new();
                map.insert("text".into(), Value::String(text.clone()));
                if *bold {
                    map.insert("bold".into(), Value::Bool(true));
                }
                if *italic {
                    map.insert("italic".into(), Value::Bool(true));
                }
                Value::Object(map)
            }
            Inline::Link { text, url } => {
                let mut map = serde_json::Map::new();
                map.insert("text".into(), Value::String(text.clone()));
                map.insert("url".into(), Value::String(url.clone()));
                Value::Object(map)
            }
        }
    }

    #[test]
    fn description_directive_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/event_buffer/vectors.json"
        ))
        .expect("shared vectors JSON");
        for case in vectors["description"].as_array().expect("description") {
            let parsed =
                description_directives(case["input"].as_str().expect("input"));
            assert_eq!(
                parsed.route.as_deref(),
                case["route"].as_str(),
                "{}",
                case["id"]
            );
            assert_eq!(
                parsed.untracked,
                case["untracked"].as_bool().expect("untracked"),
                "{}",
                case["id"]
            );
            if let Some(expected) = case.get("clock") {
                let clock = match parsed.clock_tag.kind() {
                    ClockKind::None => "none",
                    ClockKind::Off => "off",
                    ClockKind::On => "on",
                };
                assert_eq!(
                    clock,
                    expected.as_str().expect("clock"),
                    "{}",
                    case["id"]
                );
            }
            let before =
                u16::try_from(case["before"].as_u64().expect("before"))
                    .expect("before fits");
            let after = u16::try_from(case["after"].as_u64().expect("after"))
                .expect("after fits");
            assert_eq!(
                parsed.buffer,
                EventBuffer {
                    before_min: before,
                    after_min: after,
                },
                "{}",
                case["id"]
            );
        }
    }

    #[test]
    fn on_directive_is_removed_but_tagged_prose_remains_a_bullet() {
        let parsed = parse_description(
            "Bring shoes\n@love #on -30 +15\nBring snacks #offclock",
            &DescriptionContext {
                join_link: None,
                location: None,
            },
        );
        assert_eq!(parsed.directives.route.as_deref(), Some("love"));
        assert_eq!(parsed.directives.clock_tag.kind(), ClockKind::On);
        assert!(!parsed.directives.untracked);
        assert_eq!(parsed.directives.buffer.before_min, 30);
        assert_eq!(parsed.directives.buffer.after_min, 15);
        let bullets = parsed
            .lines
            .iter()
            .map(|line| {
                line.inlines
                    .iter()
                    .map(|inline| match inline {
                        Inline::Text { text, .. }
                        | Inline::Link { text, .. } => text.as_str(),
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(bullets, ["Bring shoes", "Bring snacks #offclock"]);
    }

    fn assert_buffer_reasons(
        id: &str,
        parsed: &EventDescription,
        case: &Value,
    ) {
        let Some(expected) = case.get("reasons") else {
            assert!(
                parsed.buffer_reasons.is_empty(),
                "{id} expected no reasons, got {:?}",
                parsed.buffer_reasons
            );
            return;
        };
        for (side, slot) in [
            ("before", &parsed.buffer_reasons.before),
            ("after", &parsed.buffer_reasons.after),
        ] {
            let expected_side = expected.get(side);
            match (slot, expected_side) {
                (None, None) => {}
                (None, Some(value)) if value.is_null() => {}
                (Some(reason), Some(value)) if !value.is_null() => {
                    assert_eq!(
                        reason.minutes,
                        u16::try_from(
                            value["minutes"].as_u64().expect("minutes")
                        )
                        .expect("minutes fit"),
                        "{id} {side} minutes"
                    );
                    let expected_inlines =
                        value["inlines"].as_array().expect("inlines");
                    let got: Vec<Value> =
                        reason.inlines.iter().map(inline_json).collect();
                    assert_eq!(got, *expected_inlines, "{id} {side} inlines");
                }
                (None, Some(_)) => {
                    panic!("{id} expected {side} reason, got none");
                }
                (Some(_), None | Some(_)) => {
                    panic!("{id} expected no {side} reason");
                }
            }
        }
    }

    #[test]
    fn outline_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/gcal_description/outline.json"
        ))
        .expect("outline JSON");
        for case in vectors["cases"].as_array().expect("cases") {
            let id = case["id"].as_str().expect("id");
            let parsed = parse(case["input"].as_str().expect("input"));
            if let Some(expected) = case.get("directives") {
                assert_eq!(
                    parsed.directives.route.as_deref(),
                    expected["route"].as_str(),
                    "{id} route"
                );
                assert_eq!(
                    parsed.directives.untracked,
                    expected["untracked"].as_bool().unwrap_or(false),
                    "{id} untracked"
                );
                assert_eq!(
                    parsed.directives.buffer.before_min,
                    u16::try_from(expected["before"].as_u64().unwrap_or(0))
                        .expect("before"),
                    "{id} before"
                );
                assert_eq!(
                    parsed.directives.buffer.after_min,
                    u16::try_from(expected["after"].as_u64().unwrap_or(0))
                        .expect("after"),
                    "{id} after"
                );
            }
            assert_buffer_reasons(id, &parsed, case);
            let expected_lines = case["lines"].as_array().expect("lines");
            assert_outlined_lines(id, &parsed, expected_lines);
            assert_eq!(
                parsed.oversized,
                case["oversized"].as_bool().unwrap_or(false),
                "{id} oversized"
            );
        }
    }

    #[test]
    fn boilerplate_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/gcal_description/boilerplate.json"
        ))
        .expect("boilerplate JSON");
        for case in vectors["cases"].as_array().expect("cases") {
            let id = case["id"].as_str().expect("id");
            let parsed =
                parse_with_ctx(case["input"].as_str().expect("input"), case);
            assert!(
                parsed.buffer_reasons.is_empty(),
                "{id} boilerplate must not create reasons: {:?}",
                parsed.buffer_reasons
            );
            let expected_lines = case["lines"].as_array().expect("lines");
            assert_outlined_lines(id, &parsed, expected_lines);
            assert_eq!(
                parsed.oversized,
                case["oversized"].as_bool().unwrap_or(false),
                "{id} oversized"
            );
        }
    }

    #[test]
    fn keep_bullet_labels_by_vendor() {
        let walls = [
            (
                "Hello\n\nJoin with Google Meet\nhttps://meet.google.com/aaa-bbbb-ccc",
                "Join Google Meet",
                "https://meet.google.com/aaa-bbbb-ccc",
            ),
            (
                "Hello\n\nJoin on your computer\nhttps://teams.live.com/meet/9370000000000?p=FAKE",
                "Join Teams meeting",
                "https://teams.live.com/meet/9370000000000?p=FAKE",
            ),
            (
                "Hello\n\nJoin by video system\nhttps://example.webex.com/meet/FAKE",
                "Join Webex meeting",
                "https://example.webex.com/meet/FAKE",
            ),
        ];
        for (input, label, url) in walls {
            let parsed = parse(input);
            assert_eq!(parsed.lines.len(), 2, "{label}");
            let [_, keep] = parsed.lines.as_slice() else {
                panic!("{label} keeps one join line");
            };
            assert_eq!(keep.depth, 1, "{label}");
            assert_eq!(keep.marker, LineMarker::Bullet, "{label}");
            assert_eq!(
                keep.inlines,
                vec![Inline::Link {
                    text: label.to_string(),
                    url: url.to_string(),
                }],
                "{label}"
            );
        }
    }

    #[test]
    fn no_vendor_content_writes_no_keep_bullet() {
        let parsed = parse("Hello\n\nSee you there");
        assert_eq!(parsed.lines.len(), 2);
    }

    #[test]
    fn oversized_flag_does_not_stop_parse() {
        let raw = format!("{}\n@love", "a".repeat(65_537));
        let parsed = parse(&raw);
        assert!(parsed.oversized);
        assert_eq!(parsed.directives.route.as_deref(), Some("love"));
        assert_eq!(parsed.lines.len(), 1);
        let with_reason =
            parse(&format!("{}\n-30 Drive home", "a".repeat(65_537)));
        assert!(with_reason.oversized);
        assert_eq!(with_reason.directives.buffer.before_min, 30);
        assert!(!with_reason.buffer_reasons.is_empty());
        assert_eq!(
            with_reason
                .buffer_reasons
                .before
                .as_ref()
                .expect("reason")
                .minutes,
            30
        );
    }

    #[test]
    fn debug_prints_counts_only() {
        let parsed = parse(
            "SECRET_PHRASE https://secret.example/x\n-30 SECRET_REASON here\n@love #offclock",
        );
        let debug = format!("{parsed:?}");
        assert!(!debug.contains("SECRET_PHRASE"), "{debug}");
        assert!(!debug.contains("SECRET_REASON"), "{debug}");
        assert!(!debug.contains("secret.example"), "{debug}");
        assert!(debug.contains("lines: 1"), "{debug}");
        assert!(debug.contains("note_lines: 0"), "{debug}");
        assert!(debug.contains("buffer_reasons: 1"), "{debug}");
        assert!(debug.contains("oversized: false"), "{debug}");
    }

    #[test]
    fn parse_note_uses_plain_tokenizer_and_set_flags() {
        let parsed = parse_note(
            "@family\n-0\n+15 Drive home\nBring the stroller",
            &DescriptionContext {
                join_link: None,
                location: None,
            },
        );
        assert_eq!(parsed.directives.route.as_deref(), Some("family"));
        assert!(parsed.directives.before_set);
        assert_eq!(parsed.directives.buffer.before_min, 0);
        assert!(parsed.directives.after_set);
        assert_eq!(parsed.directives.buffer.after_min, 15);
        assert_eq!(parsed.lines.len(), 1);
        let prose: String = parsed.lines[0]
            .inlines
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } | Inline::Link { text, .. } => {
                    text.as_str()
                }
            })
            .collect();
        assert_eq!(prose, "Bring the stroller");
    }

    #[test]
    fn looks_like_html_uses_known_tags_only() {
        assert!(looks_like_html("x<br>y"));
        assert!(looks_like_html("<!-- hi -->"));
        assert!(looks_like_html("<html-blob>x</html-blob>"));
        assert!(!looks_like_html("5 < 10"));
        assert!(!looks_like_html("text<https://x.test>"));
        assert!(!looks_like_html("see <apple> pie"));
    }
}
