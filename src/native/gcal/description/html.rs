//! Linear HTML tokenizer for Google Calendar descriptions.

use super::{
    decode_html_entities, is_zero_width, Inline, LineMarker, SourceLine,
};

struct ListCtx {
    ordered: bool,
    next: u32,
}

struct Tokenizer<'a> {
    rest: &'a str,
    bold: u32,
    italic: u32,
    heading: u32,
    lists: Vec<ListCtx>,
    in_item: bool,
    in_row: bool,
    first_cell: bool,
    in_link: Option<(String, String)>,
    pending_space: bool,
    current: Vec<Inline>,
    out: Vec<SourceLine>,
}

pub(super) fn tokenize(raw: &str) -> Vec<SourceLine> {
    let mut tok = Tokenizer {
        rest: raw,
        bold: 0,
        italic: 0,
        heading: 0,
        lists: Vec::new(),
        in_item: false,
        in_row: false,
        first_cell: true,
        in_link: None,
        pending_space: false,
        current: Vec::new(),
        out: Vec::new(),
    };
    tok.run();
    tok.out
}

impl<'a> Tokenizer<'a> {
    fn run(&mut self) {
        while !self.rest.is_empty() {
            let Some(start) = self.rest.find('<') else {
                let text = self.rest;
                self.rest = "";
                self.emit_decoded_text(text);
                break;
            };
            if start > 0 {
                let (text, rest) = self.rest.split_at(start);
                self.emit_decoded_text(text);
                self.rest = rest;
            }
            if !self.consume_markup() {
                self.emit_decoded_text("<");
                self.rest = &self.rest[1..];
            }
        }
        self.flush_content();
    }

    fn consume_markup(&mut self) -> bool {
        if let Some(rest) = self.rest.strip_prefix("<!--") {
            if let Some(end) = rest.find("-->") {
                self.rest = &rest[end + 3..];
            } else {
                self.rest = "";
            }
            return true;
        }
        let Some((tag, rest)) = parse_tag(self.rest) else {
            return false;
        };
        self.rest = rest;
        self.apply_tag(&tag);
        true
    }

    fn apply_tag(&mut self, tag: &ParsedTag<'_>) {
        if tag_changes_style(tag.name) {
            self.flush_pending();
        }
        if tag.closing {
            self.apply_close(tag.name);
        } else {
            self.apply_open(tag);
        }
    }

    fn apply_open(&mut self, tag: &ParsedTag<'_>) {
        if is_drop_tag(tag.name) {
            self.skip_until_close(tag.name);
            return;
        }
        if is_break_name(tag.name) {
            self.end_line();
        }
        if heading_name(tag.name) {
            self.heading += 1;
        }
        if tag.name.eq_ignore_ascii_case("b")
            || tag.name.eq_ignore_ascii_case("strong")
        {
            self.bold += 1;
        }
        if tag.name.eq_ignore_ascii_case("i")
            || tag.name.eq_ignore_ascii_case("em")
        {
            self.italic += 1;
        }
        if tag.name.eq_ignore_ascii_case("ul") {
            self.lists.push(ListCtx {
                ordered: false,
                next: 1,
            });
            self.in_item = false;
        } else if tag.name.eq_ignore_ascii_case("ol") {
            self.lists.push(ListCtx {
                ordered: true,
                next: 1,
            });
            self.in_item = false;
        } else if tag.name.eq_ignore_ascii_case("li") {
            self.start_list_item();
        } else if tag.name.eq_ignore_ascii_case("hr") {
            self.out.push(SourceLine::separator());
        } else if tag.name.eq_ignore_ascii_case("tr") {
            self.in_row = true;
            self.first_cell = true;
        } else if tag.name.eq_ignore_ascii_case("td")
            || tag.name.eq_ignore_ascii_case("th")
        {
            self.start_cell();
        } else if tag.name.eq_ignore_ascii_case("a") {
            self.open_link(tag);
        } else if tag.name.eq_ignore_ascii_case("img") {
            self.emit_image(tag);
        }
    }

    fn apply_close(&mut self, name: &str) {
        if is_break_name(name) {
            self.end_line();
        }
        if heading_name(name) {
            self.heading = self.heading.saturating_sub(1);
        }
        if name.eq_ignore_ascii_case("b") || name.eq_ignore_ascii_case("strong")
        {
            self.bold = self.bold.saturating_sub(1);
        }
        if name.eq_ignore_ascii_case("i") || name.eq_ignore_ascii_case("em") {
            self.italic = self.italic.saturating_sub(1);
        }
        if name.eq_ignore_ascii_case("ul") || name.eq_ignore_ascii_case("ol") {
            self.lists.pop();
            self.in_item = false;
        } else if name.eq_ignore_ascii_case("li") {
            self.in_item = false;
        } else if name.eq_ignore_ascii_case("tr")
            || name.eq_ignore_ascii_case("table")
        {
            self.in_row = false;
        } else if name.eq_ignore_ascii_case("a") {
            self.close_link();
        }
    }

    fn skip_until_close(&mut self, name: &str) {
        if let Some(end) = find_close_tag(self.rest, name) {
            self.rest = &self.rest[end..];
        } else {
            self.rest = "";
        }
    }

    fn start_list_item(&mut self) {
        if self.lists.is_empty() {
            self.lists.push(ListCtx {
                ordered: false,
                next: 1,
            });
        }
        self.in_item = true;
        if let Some(list) = self.lists.last_mut()
            && list.ordered
        {
            list.next += 1;
        }
    }

    fn start_cell(&mut self) {
        if self.in_row && !self.first_cell {
            self.emit_raw(" · ");
        }
        self.first_cell = false;
    }

    fn open_link(&mut self, tag: &ParsedTag<'_>) {
        if self.in_link.is_some() {
            return;
        }
        if let Some(href) = attr(tag, "href") {
            self.in_link = Some((href.to_string(), String::new()));
        }
    }

    fn flush_pending(&mut self) {
        if !self.pending_space {
            return;
        }
        self.pending_space = false;
        if self.has_content() {
            self.push_text(" ");
        }
    }

    fn close_link(&mut self) {
        if let Some((url, text)) = self.in_link.take()
            && !text.is_empty()
        {
            self.current.push(Inline::Link { text, url });
        }
    }

    fn flush_link_text(&mut self) {
        if let Some((url, text)) = self.in_link.as_mut()
            && !text.is_empty()
        {
            let text = std::mem::take(text);
            let url = url.clone();
            self.current.push(Inline::Link { text, url });
        }
    }

    fn emit_image(&mut self, tag: &ParsedTag<'_>) {
        if let Some(alt) = attr(tag, "alt")
            && !alt.is_empty()
        {
            self.emit_raw(alt);
        }
    }

    fn end_line(&mut self) {
        self.pending_space = false;
        self.flush_link_text();
        if self.current.is_empty() {
            self.out.push(SourceLine::blank());
            return;
        }
        let inlines = std::mem::take(&mut self.current);
        self.out.push(self.make_line(inlines));
    }

    fn flush_content(&mut self) {
        self.pending_space = false;
        self.flush_link_text();
        if self.current.is_empty() {
            return;
        }
        let inlines = std::mem::take(&mut self.current);
        self.out.push(self.make_line(inlines));
    }

    fn make_line(&self, inlines: Vec<Inline>) -> SourceLine {
        if self.in_item {
            self.list_line(inlines)
        } else {
            SourceLine::paragraph(inlines)
        }
    }

    fn list_line(&self, inlines: Vec<Inline>) -> SourceLine {
        let depth = self.lists.len().max(1) as u8;
        let marker = self
            .lists
            .last()
            .and_then(|list| {
                list.ordered.then_some(LineMarker::Ordered(
                    list.next.saturating_sub(1).max(1),
                ))
            })
            .unwrap_or(LineMarker::Bullet);
        SourceLine::list_item(marker, depth, inlines)
    }

    fn emit_decoded_text(&mut self, raw: &str) {
        let decoded = decode_html_entities(raw);
        for ch in decoded.chars() {
            if is_zero_width(ch) {
                continue;
            }
            if ch == '\u{00A0}' || ch.is_whitespace() {
                self.pending_space = true;
                continue;
            }
            self.emit_non_space(ch);
        }
    }

    fn emit_non_space(&mut self, ch: char) {
        if self.pending_space {
            self.pending_space = false;
            if self.has_content() {
                self.push_text(" ");
            }
        }
        let mut buf = [0; 4];
        self.push_text(ch.encode_utf8(&mut buf));
    }

    fn emit_raw(&mut self, text: &str) {
        for ch in text.chars() {
            if ch.is_whitespace() {
                self.pending_space = true;
            } else {
                self.emit_non_space(ch);
            }
        }
    }

    fn has_content(&self) -> bool {
        !self.current.is_empty()
            || self
                .in_link
                .as_ref()
                .is_some_and(|(_, text)| !text.is_empty())
    }

    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if let Some((_, buf)) = &mut self.in_link {
            buf.push_str(s);
            return;
        }
        let (bold, italic) = self.text_flags();
        if let Some(Inline::Text {
            text,
            bold: last_bold,
            italic: last_italic,
        }) = self.current.last_mut()
            && *last_bold == bold
            && *last_italic == italic
        {
            text.push_str(s);
            return;
        }
        self.current.push(Inline::Text {
            text: s.to_string(),
            bold,
            italic,
        });
    }

    fn text_flags(&self) -> (bool, bool) {
        (self.bold > 0 || self.heading > 0, self.italic > 0)
    }
}

struct ParsedTag<'a> {
    name: &'a str,
    closing: bool,
    attrs: Vec<(String, String)>,
}

fn parse_tag(input: &str) -> Option<(ParsedTag<'_>, &str)> {
    let rest = input.strip_prefix('<')?;
    if rest.starts_with('!') || rest.starts_with('?') {
        let end = rest.find('>')?;
        return Some((
            ParsedTag {
                name: "",
                closing: false,
                attrs: Vec::new(),
            },
            &rest[end + 1..],
        ));
    }
    let closing = rest.starts_with('/');
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let name_len = rest
        .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .unwrap_or(rest.len());
    if name_len == 0 {
        return None;
    }
    let name = &rest[..name_len];
    let mut rest = &rest[name_len..];
    let mut attrs = Vec::new();
    loop {
        rest = skip_ws(rest);
        if rest.is_empty() {
            return None;
        }
        if let Some(rest) = rest.strip_prefix("/>") {
            return Some((
                ParsedTag {
                    name,
                    closing,
                    attrs,
                },
                rest,
            ));
        }
        if let Some(rest) = rest.strip_prefix('>') {
            return Some((
                ParsedTag {
                    name,
                    closing,
                    attrs,
                },
                rest,
            ));
        }
        if rest.starts_with('/') {
            rest = skip_ws(&rest[1..]);
            let rest = rest.strip_prefix('>')?;
            return Some((
                ParsedTag {
                    name,
                    closing,
                    attrs,
                },
                rest,
            ));
        }
        let (attr_name, after_name) = take_attr_name(rest)?;
        rest = skip_ws(after_name);
        let value = if let Some(after_eq) = rest.strip_prefix('=') {
            rest = skip_ws(after_eq);
            let (value, after_value) = take_attr_value(rest)?;
            rest = after_value;
            decode_html_entities(&value)
        } else {
            String::new()
        };
        attrs.push((attr_name.to_ascii_lowercase(), value));
    }
}

fn take_attr_name(input: &str) -> Option<(&str, &str)> {
    let len = input
        .find(|ch: char| {
            ch.is_ascii_whitespace() || ch == '=' || ch == '>' || ch == '/'
        })
        .unwrap_or(input.len());
    if len == 0 {
        return None;
    }
    Some((&input[..len], &input[len..]))
}

fn take_attr_value(input: &str) -> Option<(String, &str)> {
    let quote = match input.as_bytes().first() {
        Some(b'"') => Some('"'),
        Some(b'\'') => Some('\''),
        _ => None,
    };
    if let Some(quote) = quote {
        let rest = &input[1..];
        let end = rest.find(quote)?;
        Some((rest[..end].to_string(), &rest[end + 1..]))
    } else {
        let len = input
            .find(|ch: char| ch.is_ascii_whitespace() || ch == '>')
            .unwrap_or(input.len());
        Some((input[..len].to_string(), &input[len..]))
    }
}

fn skip_ws(input: &str) -> &str {
    input.trim_start_matches(|ch: char| ch.is_ascii_whitespace())
}

fn attr<'a>(tag: &'a ParsedTag<'_>, name: &str) -> Option<&'a str> {
    tag.attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn is_break_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("br")
        || name.eq_ignore_ascii_case("p")
        || name.eq_ignore_ascii_case("div")
        || name.eq_ignore_ascii_case("li")
        || name.eq_ignore_ascii_case("ul")
        || name.eq_ignore_ascii_case("ol")
        || name.eq_ignore_ascii_case("tr")
        || name.eq_ignore_ascii_case("table")
        || name.eq_ignore_ascii_case("hr")
        || heading_name(name)
}

fn heading_name(name: &str) -> bool {
    matches!(name.as_bytes(), [b'h' | b'H', b'1'..=b'6'])
}

fn tag_changes_style(name: &str) -> bool {
    name.eq_ignore_ascii_case("b")
        || name.eq_ignore_ascii_case("strong")
        || name.eq_ignore_ascii_case("i")
        || name.eq_ignore_ascii_case("em")
        || name.eq_ignore_ascii_case("a")
        || heading_name(name)
}

fn is_drop_tag(name: &str) -> bool {
    name.eq_ignore_ascii_case("script")
        || name.eq_ignore_ascii_case("style")
        || name.eq_ignore_ascii_case("head")
        || name.eq_ignore_ascii_case("title")
}

fn find_close_tag(haystack: &str, name: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let name_bytes = name.as_bytes();
    let mut index = 0;
    while index + 2 + name_bytes.len() <= bytes.len() {
        if bytes[index] == b'<'
            && bytes[index + 1] == b'/'
            && bytes[index + 2..].len() >= name_bytes.len()
            && bytes[index + 2..index + 2 + name_bytes.len()]
                .eq_ignore_ascii_case(name_bytes)
        {
            let after = index + 2 + name_bytes.len();
            let next = *bytes.get(after)?;
            if next == b'>' || next.is_ascii_whitespace() || next == b'/' {
                let rel = haystack.get(after..)?.find('>')?;
                return Some(after + rel + 1);
            }
        }
        index += 1;
    }
    None
}
