//! Conservative outgoing Markdown renderer for compose bodies.
//!
//! The renderer turns an authored compose body (already stripped of `## Notes`
//! by the source model) into an inline-styled HTML fragment and a plain-text
//! alternative with equivalent content. It supports a deliberately small
//! subset — paragraphs, headings, emphasis, lists, safe links, blockquotes,
//! and ordinary code — and refuses plugin or automation syntax with a kind and
//! a 1-based line number instead of body excerpts.
//!
//! Wikilinks flatten to readable text (`[[target|alias]]` becomes the alias,
//! unaliased links become their visible target) and every flattening is
//! recorded deterministically in document order. `![[embed]]` references and
//! local Markdown images are accepted only when they name a declared
//! attachment; accepted embeds become the transmitted filename, never
//! expanded note content. Raw HTML is escaped, never executed. No Dataview,
//! Tasks, or Templater syntax runs, no URL is fetched, and no wikilink is
//! traversed to gather content.

use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use regex::Regex;

/// One flattened reference: the span as written and its replacement text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlattenedLink {
    pub(crate) original: String,
    pub(crate) text: String,
}

/// Rendered outgoing alternatives with equivalent content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedMessage {
    /// Inline-styled HTML fragment for the MIME HTML alternative.
    pub(crate) html_body: String,
    /// Plain-text alternative carrying the same links and references.
    pub(crate) plain_body: String,
    /// Every wikilink flattening and accepted embed reference, in order.
    pub(crate) flattened_links: Vec<FlattenedLink>,
}

/// A refusal to render: a machine-readable kind plus a 1-based line number.
///
/// The refusal never carries body text, so it is safe for errors, logs, and
/// diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderRefusal {
    kind: &'static str,
    line: usize,
}

impl RenderRefusal {
    pub(crate) fn hint(&self) -> &'static str {
        match self.kind {
            "task_checkbox" => {
                "task checkboxes never send; delete the checkbox or move the task out of the message"
            }
            "block_id" => {
                "block ids are vault-local; delete the ^id before sending"
            }
            "inline_field" => {
                "inline fields are vault-local; rewrite the [key:: value] as plain prose"
            }
            "inline_query" => {
                "inline queries never run for outgoing mail; replace the `= ...` span with its result"
            }
            "dataview_fence" => {
                "dataview fences never run for outgoing mail; replace the fence with its result"
            }
            "tasks_fence" => {
                "tasks fences never run for outgoing mail; replace the fence with plain text"
            }
            "templater" => {
                "templater syntax never expands for outgoing mail; replace the <% ... %> span with plain text"
            }
            "unsafe_link" => {
                "links must use http, https, or mailto destinations; fix the link target"
            }
            "remote_image" => {
                "remote images never load in outgoing mail; attach the file or replace it with a link"
            }
            "undeclared_embed" => {
                "embeds must name a declared attachment; add the file to attachments or remove the embed"
            }
            _ => "edit the message and preview again",
        }
    }
}

impl std::fmt::Display for RenderRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "outgoing message refuses {} at line {}",
            self.kind, self.line
        )
    }
}

impl std::error::Error for RenderRefusal {}

/// Render an authored body to HTML and plain alternatives.
///
/// `declared_attachments` are the vault-relative paths from the compose
/// frontmatter. Embeds and local images must name one of them; every declared
/// path is attached by the later transport phase whether or not the body
/// references it.
pub(crate) fn render(
    body: &str,
    declared_attachments: &[PathBuf],
) -> Result<RenderedMessage, RenderRefusal> {
    let spans = collect_spans(body);
    let textual = textual_scan(body, &spans, declared_attachments);
    let mut refusals = Vec::new();
    refusals.extend(spans.fence_refusals);
    refusals.extend(spans.query_refusals);
    refusals.extend(link_refusals(&spans.links));
    refusals.extend(image_refusals(&spans.images, declared_attachments));
    refusals.extend(textual.refusals);
    if let Some(first) = refusals
        .iter()
        .min_by_key(|refusal| refusal_line(body, refusal.offset))
    {
        return Err(RenderRefusal {
            kind: first.kind,
            line: refusal_line(body, first.offset),
        });
    }
    let transformed = apply_edits(body, &textual.edits);
    let html_body = render_html(&transformed);
    let plain_body = render_plain(&transformed);
    Ok(RenderedMessage {
        html_body,
        plain_body,
        flattened_links: textual.flattened,
    })
}

struct PendingRefusal {
    kind: &'static str,
    offset: usize,
}

fn refusal_line(body: &str, offset: usize) -> usize {
    body[..offset.min(body.len())].matches('\n').count() + 1
}

/// Byte ranges collected from one parser pass, plus parser-level refusals.
struct Spans {
    /// Fenced code blocks, inline code spans, and raw HTML spans.
    code: Vec<Range<usize>>,
    /// Full spans of links with their destinations.
    links: Vec<(Range<usize>, String)>,
    /// Full spans of Markdown images with their destinations.
    images: Vec<(Range<usize>, String)>,
    fence_refusals: Vec<PendingRefusal>,
    query_refusals: Vec<PendingRefusal>,
}

fn collect_spans(body: &str) -> Spans {
    let mut spans = Spans {
        code: Vec::new(),
        links: Vec::new(),
        images: Vec::new(),
        fence_refusals: Vec::new(),
        query_refusals: Vec::new(),
    };
    let parser = Parser::new_ext(body, Options::empty()).into_offset_iter();
    for (event, range) in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                if let CodeBlockKind::Fenced(info) = &kind {
                    let name =
                        info.split_whitespace().next().unwrap_or_default();
                    let lowered = name.to_ascii_lowercase();
                    if lowered == "dataview" || lowered == "dataviewjs" {
                        spans.fence_refusals.push(PendingRefusal {
                            kind: "dataview_fence",
                            offset: range.start,
                        });
                    } else if lowered == "tasks" {
                        spans.fence_refusals.push(PendingRefusal {
                            kind: "tasks_fence",
                            offset: range.start,
                        });
                    }
                }
                push_range(&mut spans.code, range);
            }
            Event::End(TagEnd::CodeBlock) => {
                push_range(&mut spans.code, range);
            }
            Event::Code(literal) => {
                if literal.starts_with('=') || literal.starts_with("$=") {
                    spans.query_refusals.push(PendingRefusal {
                        kind: "inline_query",
                        offset: range.start,
                    });
                }
                push_range(&mut spans.code, range);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                spans.links.push((range, dest_url.into_string()));
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                spans.images.push((range, dest_url.into_string()));
            }
            Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock) => {
                push_range(&mut spans.code, range);
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                push_range(&mut spans.code, range);
            }
            _ => {}
        }
    }
    spans
}

fn push_range(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    if let Some(last) = ranges.last()
        && last.start == range.start
        && last.end == range.end
    {
        return;
    }
    ranges.push(range);
}

fn overlaps(ranges: &[Range<usize>], start: usize, end: usize) -> bool {
    ranges
        .iter()
        .any(|range| start < range.end && range.start < end)
}

fn link_refusals(links: &[(Range<usize>, String)]) -> Vec<PendingRefusal> {
    links
        .iter()
        .filter(|(_, dest)| !is_safe_destination(dest))
        .map(|(range, _)| PendingRefusal {
            kind: "unsafe_link",
            offset: range.start,
        })
        .collect()
}

/// Only http, https, and mailto destinations can be verified by a reader.
/// Relative links, anchors, and every other scheme are refused.
fn is_safe_destination(dest: &str) -> bool {
    let dest = dest.trim();
    if let Some((scheme, rest)) = dest.split_once(':') {
        let scheme = scheme.to_ascii_lowercase();
        if rest.is_empty() {
            return false;
        }
        matches!(scheme.as_str(), "http" | "https" | "mailto")
    } else {
        false
    }
}

fn image_refusals(
    images: &[(Range<usize>, String)],
    declared: &[PathBuf],
) -> Vec<PendingRefusal> {
    let mut refusals = Vec::new();
    for (range, dest) in images {
        if is_remote_image(dest) {
            refusals.push(PendingRefusal {
                kind: "remote_image",
                offset: range.start,
            });
        } else if match_declared(dest, declared).is_none() {
            refusals.push(PendingRefusal {
                kind: "undeclared_embed",
                offset: range.start,
            });
        }
    }
    refusals
}

/// A Markdown image destination is remote when it carries a URI scheme or is
/// protocol-relative. Vault-relative attachment paths never do.
fn is_remote_image(dest: &str) -> bool {
    let dest = dest.trim();
    if dest.starts_with("//") {
        return true;
    }
    match dest.find(':') {
        None => false,
        Some(colon) => {
            let scheme = &dest[..colon];
            !scheme.contains('/')
                && scheme
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
        }
    }
}

struct TextualScan {
    refusals: Vec<PendingRefusal>,
    edits: Vec<(Range<usize>, String)>,
    flattened: Vec<FlattenedLink>,
}

/// Scan non-code text for plugin syntax and flatten wikilinks and embeds.
///
/// Every match is checked against the parser's code, link, image, and raw
/// HTML ranges, so literal examples in fenced code, inline code, or escaped
/// text never trigger. Escaped `\\[[` sequences are left for the Markdown
/// renderer, which shows them literally.
fn textual_scan(
    body: &str,
    spans: &Spans,
    declared: &[PathBuf],
) -> TextualScan {
    let mut scan = TextualScan {
        refusals: Vec::new(),
        edits: Vec::new(),
        flattened: Vec::new(),
    };
    // Link and image spans join the skip ranges for textual scans: their
    // destinations are already checked from the parsed events.
    let mut skip = spans.code.clone();
    skip.extend(spans.links.iter().map(|(range, _)| range.clone()));
    skip.extend(spans.images.iter().map(|(range, _)| range.clone()));
    scan_templater(body, &skip, &mut scan);
    scan_inline_fields(body, &skip, &mut scan);
    scan_line_syntax(body, &spans.code, &mut scan);
    scan_wikilinks(body, &skip, declared, &mut scan);
    // Accepted local Markdown images become filename references as well.
    for (range, dest) in &spans.images {
        if is_remote_image(dest) {
            continue;
        }
        let Some(matched) = match_declared(dest, declared) else {
            continue;
        };
        let filename = declared_filename(&declared[matched]);
        scan.edits.push((range.clone(), escape_literal(&filename)));
        scan.flattened.push(FlattenedLink {
            original: body[range.clone()].to_string(),
            text: filename,
        });
    }
    scan
}

fn scan_templater(body: &str, skip: &[Range<usize>], scan: &mut TextualScan) {
    let mut cursor = 0;
    while let Some(relative) = body[cursor..].find("<%") {
        let start = cursor + relative;
        if !overlaps(skip, start, start + 2) {
            scan.refusals.push(PendingRefusal {
                kind: "templater",
                offset: start,
            });
        }
        cursor = start + 2;
    }
}

fn inline_field_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"[\(\[][^()\[\]\n]*::").expect("inline field pattern")
    })
}

fn scan_inline_fields(
    body: &str,
    skip: &[Range<usize>],
    scan: &mut TextualScan,
) {
    for found in inline_field_pattern().find_iter(body) {
        if !overlaps(skip, found.start(), found.end()) {
            scan.refusals.push(PendingRefusal {
                kind: "inline_field",
                offset: found.start(),
            });
        }
    }
}

fn checkbox_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^[ ]{0,3}(?:>[ ]?)*([-*+]|\d{1,9}[.)])[ ]+\[[ xX]\]")
            .expect("checkbox pattern")
    })
}

fn block_id_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?:^|[ ])\^[A-Za-z0-9][A-Za-z0-9-]*[ ]*$")
            .expect("block id pattern")
    })
}

/// Line-local refusals: actual task checkboxes and trailing block ids.
///
/// Matches overlapping code ranges are skipped, so fenced and inline literal
/// examples stay inert.
fn scan_line_syntax(body: &str, code: &[Range<usize>], scan: &mut TextualScan) {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let line_end = offset + line.len();
        let content_end =
            line_end.saturating_sub(usize::from(line.ends_with('\n')));
        // Match against the line without its terminator so `$` anchors work.
        let text = line.trim_end_matches(['\r', '\n']);
        if !overlaps(code, offset, content_end.max(offset)) {
            if let Some(found) = checkbox_pattern().find(text) {
                scan.refusals.push(PendingRefusal {
                    kind: "task_checkbox",
                    offset: offset + found.start(),
                });
            } else if let Some(found) = block_id_pattern().find(text) {
                scan.refusals.push(PendingRefusal {
                    kind: "block_id",
                    offset: offset + found.start(),
                });
            }
        } else {
            // A line straddling code still reports matches fully outside it.
            for found in checkbox_pattern().find_iter(text) {
                let start = offset + found.start();
                if !overlaps(code, start, start + found.len().max(1)) {
                    scan.refusals.push(PendingRefusal {
                        kind: "task_checkbox",
                        offset: start,
                    });
                }
            }
            if let Some(found) = block_id_pattern().find(text) {
                let start = offset + found.start();
                if !overlaps(code, start, start + found.len().max(1)) {
                    scan.refusals.push(PendingRefusal {
                        kind: "block_id",
                        offset: start,
                    });
                }
            }
        }
        offset = line_end;
    }
}

/// Flatten `[[target|alias]]` to the alias and unaliased links to their
/// visible target; resolve `![[embed]]` against declared attachments.
///
/// Replacement text is Markdown-escaped so flattened values can never become
/// structure. Accepted embeds record the transmitted filename.
fn scan_wikilinks(
    body: &str,
    skip: &[Range<usize>],
    declared: &[PathBuf],
    scan: &mut TextualScan,
) {
    let bytes = body.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(open) = find_wikilink_open(bytes, cursor) else {
            break;
        };
        // An escaped `\\!` is a literal bang, so `\\![[x]]` is a plain
        // wikilink, never an embed.
        let embed =
            open > 0 && bytes[open - 1] == b'!' && !is_escaped(bytes, open - 1);
        // `open` points at the first `[` in both cases; an embed only adds
        // the leading `!` to the span, never to the content.
        let content_start = open + 2;
        let Some(close) = find_wikilink_close(bytes, content_start) else {
            break;
        };
        let span_start = if embed { open - 1 } else { open };
        let content = &body[content_start..close];
        if overlaps(skip, span_start, close + 2) || content.contains('\n') {
            cursor = close + 2;
            continue;
        }
        if embed {
            let target = split_target(content);
            match match_declared(target, declared) {
                Some(matched) => {
                    let filename = declared_filename(&declared[matched]);
                    scan.edits.push((
                        span_start..close + 2,
                        escape_literal(&filename),
                    ));
                    scan.flattened.push(FlattenedLink {
                        original: body[span_start..close + 2].to_string(),
                        text: filename,
                    });
                }
                None => scan.refusals.push(PendingRefusal {
                    kind: "undeclared_embed",
                    offset: span_start,
                }),
            }
        } else {
            let text = match split_alias_text(content) {
                Some(alias) if !alias.is_empty() => alias.to_string(),
                // An empty alias would hide the reference; keep the target.
                _ => split_target(content).to_string(),
            };
            scan.edits.push((open..close + 2, escape_literal(&text)));
            scan.flattened.push(FlattenedLink {
                original: body[open..close + 2].to_string(),
                text,
            });
        }
        cursor = close + 2;
    }
}

fn find_wikilink_open(bytes: &[u8], from: usize) -> Option<usize> {
    let mut cursor = from;
    while cursor + 1 < bytes.len() {
        if bytes[cursor] == b'['
            && bytes[cursor + 1] == b'['
            && !is_escaped(bytes, cursor)
        {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn find_wikilink_close(bytes: &[u8], from: usize) -> Option<usize> {
    let mut cursor = from;
    while cursor + 1 < bytes.len() {
        if bytes[cursor] == b']' && bytes[cursor + 1] == b']' {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn is_escaped(bytes: &[u8], at: usize) -> bool {
    let mut backslashes = 0;
    let mut cursor = at;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

fn split_target(content: &str) -> &str {
    match content.find('|') {
        Some(pipe) => &content[..pipe],
        None => content,
    }
}

fn split_alias_text(content: &str) -> Option<&str> {
    content.find('|').map(|pipe| content[pipe + 1..].trim())
}

/// Match an embed or image target against declared attachment paths.
///
/// Accepts an exact vault-relative match, or a bare filename matching a
/// declared path's file name. A `#heading` or `^block` suffix selects within
/// the note and is ignored for matching; percent-escapes are decoded.
fn match_declared(target: &str, declared: &[PathBuf]) -> Option<usize> {
    let normalized = normalize_target(target);
    if normalized.is_empty() {
        return None;
    }
    if let Some(matched) = declared.iter().position(|path| {
        path.to_string_lossy().replace('\\', "/") == normalized
    }) {
        return Some(matched);
    }
    if !normalized.contains('/') {
        return declared.iter().position(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy() == normalized)
        });
    }
    None
}

fn normalize_target(target: &str) -> String {
    let target = target.trim();
    let target = target.strip_prefix("./").unwrap_or(target);
    let end = target.find(['#', '^']).unwrap_or(target.len());
    percent_decode(&target[..end])
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%'
            && let Some(high) = bytes.get(cursor + 1)
            && let Some(low) = bytes.get(cursor + 2)
            && let (Some(high), Some(low)) = (hex_digit(*high), hex_digit(*low))
        {
            decoded.push(high << 4 | low);
            cursor += 3;
        } else {
            decoded.push(bytes[cursor]);
            cursor += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn declared_filename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Backslash-escape ASCII punctuation so inserted text renders literally and
/// can never become Markdown structure.
fn escape_literal(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii_punctuation() {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Apply non-overlapping textual edits in document order.
fn apply_edits(body: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut ordered = edits.to_vec();
    ordered.sort_by_key(|(range, _)| (range.start, range.end));
    let mut transformed = String::with_capacity(body.len());
    let mut cursor = 0;
    for (range, replacement) in ordered {
        if range.start < cursor
            || range.end < range.start
            || range.end > body.len()
        {
            continue;
        }
        transformed.push_str(&body[cursor..range.start]);
        transformed.push_str(&replacement);
        cursor = range.end;
    }
    transformed.push_str(&body[cursor..]);
    transformed
}

/// Render the validated Markdown to an inline-styled HTML fragment.
///
/// Raw HTML events become text so tags show literally instead of executing.
/// Pulldown-cmark's default output is then dressed with inline styles so the
/// fragment reads well in mail clients without a stylesheet.
fn render_html(transformed: &str) -> String {
    let parser = Parser::new_ext(transformed, Options::empty())
        .into_offset_iter()
        .map(|(event, _)| match event {
            Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
            other => other,
        });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    apply_inline_styles(&html)
}

fn apply_inline_styles(html: &str) -> String {
    let mut out = html.to_string();
    out = out.replace(
        "<pre><code",
        "<pre style=\"background:#f6f8fa;padding:12px;border-radius:6px;overflow-x:auto;font-size:13px\"><code style=\"font-family:monospace,monospace\"",
    );
    out = out.replace("<p>", "<p style=\"margin:0 0 12px 0\">");
    out = out
        .replace("<h1>", "<h1 style=\"font-size:20px;margin:16px 0 8px 0\">");
    out = out
        .replace("<h2>", "<h2 style=\"font-size:18px;margin:16px 0 8px 0\">");
    out = out
        .replace("<h3>", "<h3 style=\"font-size:16px;margin:14px 0 8px 0\">");
    out = out
        .replace("<h4>", "<h4 style=\"font-size:14px;margin:12px 0 8px 0\">");
    out = out
        .replace("<h5>", "<h5 style=\"font-size:13px;margin:12px 0 8px 0\">");
    out = out
        .replace("<h6>", "<h6 style=\"font-size:12px;margin:12px 0 8px 0\">");
    out =
        out.replace("<ul", "<ul style=\"margin:0 0 12px 0;padding-left:24px\"");
    out =
        out.replace("<ol", "<ol style=\"margin:0 0 12px 0;padding-left:24px\"");
    out = out.replace("<li>", "<li style=\"margin:0 0 4px 0\">");
    out = out.replace(
        "<blockquote>",
        "<blockquote style=\"margin:0 0 12px 0;padding:0 0 0 12px;border-left:2px solid #888888;color:#444444\">",
    );
    out = out.replace(
        "<code>",
        "<code style=\"font-family:monospace,monospace;background:#f6f8fa;padding:1px 4px;border-radius:4px\">",
    );
    out = out.replace("<a href=", "<a style=\"color:#1a73e8\" href=");
    out = out.replace(
        "<hr />",
        "<hr style=\"border:none;border-top:1px solid #cccccc;margin:16px 0\" />",
    );
    out = out.replace(
        "<hr>",
        "<hr style=\"border:none;border-top:1px solid #cccccc;margin:16px 0\">",
    );
    out
}

/// Render the validated Markdown to plain text with equivalent content.
///
/// Links keep their destinations as `label (url)` unless the label already
/// is the destination; flattened wikilink text and accepted filenames flow
/// through unchanged. List and quote structure degrades to `-`, `1.`, and
/// `> ` prefixes.
fn render_plain(transformed: &str) -> String {
    let mut writer = PlainWriter::default();
    let parser =
        Parser::new_ext(transformed, Options::empty()).into_offset_iter();
    for (event, _) in parser {
        writer.event(&event);
    }
    writer.finish()
}

#[derive(Default)]
struct PlainWriter {
    out: String,
    at_line_start: bool,
    quote_depth: usize,
    lists: Vec<ListState>,
    link_stack: Vec<(usize, String)>,
}

#[derive(Clone, Copy)]
enum ListState {
    Bullet,
    Ordered(u64),
}

impl PlainWriter {
    fn event(&mut self, event: &Event<'_>) {
        match event {
            Event::Start(Tag::Paragraph)
            | Event::Start(Tag::Heading { .. }) => {
                self.break_block();
            }
            Event::End(TagEnd::Paragraph)
            | Event::End(TagEnd::Heading { .. })
            | Event::End(TagEnd::CodeBlock) => self.break_block(),
            Event::Start(Tag::BlockQuote(_)) => {
                self.break_block();
                self.quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.break_block();
            }
            Event::Start(Tag::CodeBlock(_)) => self.break_block(),
            Event::Start(Tag::List(first)) => {
                self.break_block();
                self.lists.push(match first {
                    Some(start) => ListState::Ordered(*start),
                    None => ListState::Bullet,
                });
            }
            Event::End(TagEnd::List(_)) => {
                self.lists.pop();
                self.break_block();
            }
            Event::Start(Tag::Item) => {
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(ListState::Ordered(next)) => {
                        let marker = format!("{next}. ");
                        *next += 1;
                        marker
                    }
                    _ => "- ".to_string(),
                };
                self.break_line();
                let indent = "  ".repeat(depth);
                self.push_text(&format!("{indent}{marker}"));
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.link_stack.push((self.out.len(), dest_url.to_string()));
            }
            Event::End(TagEnd::Link) => {
                if let Some((start, dest)) = self.link_stack.pop()
                    && let Some(label) = self.out.get(start..)
                    && label != dest
                {
                    self.push_text(&format!(" ({dest})"));
                }
            }
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            Event::Start(Tag::Emphasis)
            | Event::Start(Tag::Strong)
            | Event::End(TagEnd::Emphasis)
            | Event::End(TagEnd::Strong) => {}
            Event::Text(text) | Event::Code(text) => self.push_text(text),
            Event::Html(text) | Event::InlineHtml(text) => self.push_text(text),
            Event::SoftBreak | Event::HardBreak => self.push_text("\n"),
            Event::Rule => {
                self.break_block();
                self.push_text("---");
                self.break_block();
            }
            Event::TaskListMarker(_) | Event::FootnoteReference(_) => {}
            _ => {}
        }
    }

    fn finish(mut self) -> String {
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out
    }

    fn break_block(&mut self) {
        if self.out.is_empty() {
            return;
        }
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out.push_str("\n\n");
        self.at_line_start = true;
    }

    fn break_line(&mut self) {
        if self.out.is_empty() {
            self.at_line_start = true;
            return;
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.at_line_start = true;
    }

    fn push_text(&mut self, text: &str) {
        let mut rest = text;
        loop {
            if self.at_line_start {
                for _ in 0..self.quote_depth {
                    self.out.push_str("> ");
                }
                self.at_line_start = false;
            }
            match rest.find('\n') {
                Some(index) => {
                    self.out.push_str(&rest[..=index]);
                    self.at_line_start = true;
                    rest = &rest[index + 1..];
                }
                None => {
                    self.out.push_str(rest);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_attachments() -> Vec<PathBuf> {
        Vec::new()
    }

    fn declared(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn refusal_kind(body: &str, attachments: &[PathBuf]) -> (String, usize) {
        match render(body, attachments) {
            Ok(_) => panic!("expected a refusal for {body:?}"),
            Err(refusal) => (refusal.kind.to_string(), refusal.line),
        }
    }

    #[test]
    fn supported_subset_renders_styled_html_and_equivalent_plain() {
        let body = "# Heading\n\nA paragraph with *emphasis* and a [label](https://example.com/x).\n\n- one\n- two\n\n1. first\n2. second\n\n> quoted\n\n`code` and\n\n```rust\nlet x = 1;\n```\n";
        let rendered =
            render(body, &no_attachments()).expect("supported subset");
        assert!(rendered.flattened_links.is_empty());
        for tag in [
            "<h1 style=",
            "<p style=",
            "<em>",
            "<ul style=",
            "<ol style=",
            "<li style=",
            "<blockquote style=",
            "<code style=",
            "<pre style=",
            "<a style=\"color:#1a73e8\" href=\"https://example.com/x\">label</a>",
        ] {
            assert!(rendered.html_body.contains(tag), "missing {tag}");
        }
        assert!(rendered.plain_body.contains("Heading"));
        assert!(rendered.plain_body.contains("emphasis"));
        assert!(rendered
            .plain_body
            .contains("label (https://example.com/x)"));
        assert!(rendered.plain_body.contains("- one"));
        assert!(rendered.plain_body.contains("1. first"));
        assert!(rendered.plain_body.contains("2. second"));
        assert!(rendered.plain_body.contains("> quoted"));
        assert!(rendered.plain_body.contains("let x = 1;"));
    }

    #[test]
    fn raw_html_is_escaped_never_executed() {
        let body = "Hi <b>there</b>\n\n<script>alert(1)</script>\n";
        let rendered = render(body, &no_attachments()).expect("raw html");
        assert!(rendered.html_body.contains("&lt;b&gt;there&lt;/b&gt;"));
        assert!(rendered.html_body.contains("&lt;script&gt;"));
        assert!(!rendered.html_body.contains("<script>"));
        assert!(rendered.plain_body.contains("<b>there</b>"));
    }

    #[test]
    fn empty_multiline_and_unicode_bodies_round_trip() {
        let empty = render("", &no_attachments()).expect("empty body");
        assert_eq!(empty.html_body, "");
        assert_eq!(empty.plain_body, "");
        assert!(empty.flattened_links.is_empty());

        let body = "Grüße 🎉\n\n第二段 with *tildes* ~not strike~\n";
        let rendered = render(body, &no_attachments()).expect("unicode body");
        assert!(rendered.html_body.contains("Grüße 🎉"));
        assert!(rendered.html_body.contains("第二段"));
        assert!(rendered.plain_body.contains("Grüße 🎉"));
        assert!(rendered.plain_body.contains("~not strike~"));
    }

    #[test]
    fn unsafe_link_destinations_are_refused_with_line_numbers() {
        assert_eq!(
            refusal_kind("[x](javascript:alert(1))", &no_attachments()),
            ("unsafe_link".to_string(), 1)
        );
        assert_eq!(
            refusal_kind(
                "First\n\n[x](data:text/plain;base64,xx)",
                &no_attachments()
            ),
            ("unsafe_link".to_string(), 3)
        );
        assert_eq!(
            refusal_kind("[x](/relative/path)", &no_attachments()),
            ("unsafe_link".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("[x](#anchor)", &no_attachments()),
            ("unsafe_link".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("[x]()", &no_attachments()),
            ("unsafe_link".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("<javascript:alert(1)>", &no_attachments()),
            ("unsafe_link".to_string(), 1)
        );
    }

    #[test]
    fn safe_links_include_mailto_and_autolinks() {
        let body = "[docs](https://example.com) and <https://example.com/y> and [me](mailto:me@example.com)";
        let rendered = render(body, &no_attachments()).expect("safe links");
        assert!(rendered.html_body.contains("href=\"https://example.com\""));
        assert!(rendered
            .html_body
            .contains("href=\"mailto:me@example.com\""));
        // An autolink label already is its destination: no duplication.
        assert!(rendered.plain_body.contains("https://example.com/y"));
        assert!(!rendered
            .plain_body
            .contains("https://example.com/y (https://example.com/y)"));
        assert!(rendered.plain_body.contains("me (mailto:me@example.com)"));
    }

    #[test]
    fn each_refusal_kind_reports_kind_and_line_without_body_text() {
        assert_eq!(
            refusal_kind("- [ ] do this", &no_attachments()),
            ("task_checkbox".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("ok\n\n- [x] done", &no_attachments()),
            ("task_checkbox".to_string(), 3)
        );
        assert_eq!(
            refusal_kind("> - [ ] quoted task", &no_attachments()),
            ("task_checkbox".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("A line with an id ^abc-123", &no_attachments()),
            ("block_id".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("# Title ^note-id", &no_attachments()),
            ("block_id".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("The [status:: draft] value", &no_attachments()),
            ("inline_field".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("The (status:: draft) value", &no_attachments()),
            ("inline_field".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("Total is `= sum(rows)` today", &no_attachments()),
            ("inline_query".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("Total is `$= dv.current()` today", &no_attachments()),
            ("inline_query".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("```dataview\nTABLE a\n```\n", &no_attachments()),
            ("dataview_fence".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("```dataviewjs\nx\n```\n", &no_attachments()),
            ("dataview_fence".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("```tasks\ntodo\n```\n", &no_attachments()),
            ("tasks_fence".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("Hello <% tp.file.title %>", &no_attachments()),
            ("templater".to_string(), 1)
        );
        assert_eq!(
            refusal_kind(
                "![photo](https://example.com/p.png)",
                &no_attachments()
            ),
            ("remote_image".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("![[not-declared.pdf]]", &no_attachments()),
            ("undeclared_embed".to_string(), 1)
        );
        assert_eq!(
            refusal_kind("![alt](files/stray.pdf)", &no_attachments()),
            ("undeclared_embed".to_string(), 1)
        );
    }

    #[test]
    fn earliest_refusal_wins_and_messages_carry_no_body() {
        let body = "- [ ] first problem\n\nHello <% later %>\n";
        let (kind, line) = refusal_kind(body, &no_attachments());
        assert_eq!((kind.as_str(), line), ("task_checkbox", 1));
        let error = render(body, &no_attachments()).expect_err("refusal");
        let message = error.to_string();
        assert!(message.contains("task_checkbox"));
        assert!(message.contains("line 1"));
        assert!(!message.contains("first problem"));
        assert!(!message.contains("later"));
        assert!(!error.hint().is_empty());
    }

    #[test]
    fn literal_examples_in_code_and_escapes_stay_inert() {
        let body = "```markdown\n- [ ] example\n[[link]] ^block\n<% code %>\n[status:: x]\n```\n\nInline `- [ ] task`, `[[link]]`, `<% code %>`, and `[a:: b]` stay.\n\n\\[[not a link]] stays.\n";
        let rendered =
            render(body, &no_attachments()).expect("literal examples");
        assert!(rendered.flattened_links.is_empty());
        assert!(rendered.html_body.contains("- [ ] example"));
        assert!(rendered.plain_body.contains("[[link]]"));
        assert!(rendered.plain_body.contains("[[not a link]]"));
    }

    #[test]
    fn escaped_bang_is_a_literal_bang_before_a_wikilink() {
        let body = "See \\![[kept]] here.\n";
        let rendered = render(body, &no_attachments()).expect("escaped bang");
        assert_eq!(rendered.flattened_links.len(), 1);
        assert_eq!(rendered.flattened_links[0].original, "[[kept]]");
        assert!(rendered.plain_body.contains("!kept"));
    }

    #[test]
    fn ordinary_code_spans_are_not_inline_queries() {
        let body = "Run `a = b` and `x == y` for assignment.\n";
        let rendered = render(body, &no_attachments()).expect("ordinary code");
        assert!(rendered.html_body.contains("<code style="));
        assert!(rendered.plain_body.contains("a = b"));
    }

    #[test]
    fn link_labels_hide_field_like_text_without_refusal() {
        let body = "See [a::b](https://example.com) for details.\n";
        let rendered = render(body, &no_attachments()).expect("link label");
        assert!(rendered.plain_body.contains("a::b (https://example.com)"));
    }

    #[test]
    fn wikilinks_flatten_deterministically_in_document_order() {
        let body = "See [[Lease renewal]] and [[notes/app.md|the plan]] plus [[a|*emph*]].\n";
        let rendered = render(body, &no_attachments()).expect("wikilinks");
        assert_eq!(
            rendered.flattened_links,
            vec![
                FlattenedLink {
                    original: "[[Lease renewal]]".to_string(),
                    text: "Lease renewal".to_string(),
                },
                FlattenedLink {
                    original: "[[notes/app.md|the plan]]".to_string(),
                    text: "the plan".to_string(),
                },
                FlattenedLink {
                    original: "[[a|*emph*]]".to_string(),
                    text: "*emph*".to_string(),
                },
            ]
        );
        assert!(rendered.html_body.contains("Lease renewal"));
        assert!(rendered.html_body.contains("the plan"));
        // Flattened alias punctuation renders literally, never as emphasis.
        assert!(rendered.html_body.contains("*emph*"));
        assert!(!rendered.html_body.contains("<em>emph</em>"));
        assert!(rendered.plain_body.contains("the plan"));
    }

    #[test]
    fn declared_embeds_become_filenames_and_undeclared_refuse() {
        let attachments = declared(&["files/invoice.pdf", "photo.png"]);
        let body = "Attached ![[invoice.pdf]] and ![alt](files/invoice.pdf) and [[photo.png|see]] is text.\n";
        let rendered = render(body, &attachments).expect("declared embeds");
        assert!(rendered.html_body.contains("invoice.pdf"));
        assert!(!rendered.html_body.contains("!["));
        assert!(rendered.plain_body.contains("invoice.pdf"));
        // The bare wikilink to an attachment-looking name still flattens as text.
        assert!(rendered.plain_body.contains("see"));
        let texts: Vec<_> = rendered
            .flattened_links
            .iter()
            .map(|link| link.text.clone())
            .collect();
        assert!(texts.contains(&"invoice.pdf".to_string()));
        // A section suffix on a declared file still resolves to the attachment.
        let sectioned =
            render("See ![[files/invoice.pdf#page-2]] ok", &attachments)
                .expect("sectioned declared embed");
        assert!(sectioned.plain_body.contains("invoice.pdf"));
    }

    #[test]
    fn section_suffix_and_percent_escapes_match_declared_files() {
        let attachments = declared(&["files/my report.pdf"]);
        let body = "See ![[files/my%20report.pdf#page-2]] here.\n";
        let rendered =
            render(body, &attachments).expect("encoded section embed");
        assert!(rendered.plain_body.contains("my report.pdf"));
        assert_eq!(rendered.flattened_links.len(), 1);
    }

    #[test]
    fn attachment_paths_match_exact_or_by_basename() {
        let attachments = declared(&["files/invoice.pdf"]);
        assert!(match_declared("files/invoice.pdf", &attachments).is_some());
        assert!(match_declared("invoice.pdf", &attachments).is_some());
        assert!(match_declared("other/invoice.pdf", &attachments).is_none());
        assert!(match_declared("", &attachments).is_none());
        assert!(
            match_declared("https://example.com/x.pdf", &attachments).is_none()
        );
    }
}
