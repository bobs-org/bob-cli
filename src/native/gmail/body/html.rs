//! HTML5 DOM walker for Gmail bodies.
//!
//! The parser never loads resources. We copy visible text and selected
//! formatting into Bob's structured inert representation, then serialize it
//! only through `inert::render_blocks`. Sender HTML, attributes, and CSS never
//! become Markdown structure directly.

use std::collections::HashSet;

use scraper::node::Node;
use scraper::{ElementRef, Html};
use url::Url;

use super::inert::{remote_image_link, render_blocks, Block, Inline, SafeUrl};
use super::InlineImage;

/// Sanitized output of one HTML candidate. The three prose segments have
/// already passed through the inert serializer. The inline image list only
/// contains MIME resources referenced by visible HTML images.
pub(crate) struct HtmlRender {
    pub(crate) markdown: String,
    pub(crate) quoted: Option<String>,
    pub(crate) signature: Option<String>,
    pub(crate) inline_images: Vec<InlineImage>,
    /// Explicit structure flags so selection never infers tables from a
    /// literal `|` in serialized text. True when any segment holds a
    /// semantic `Block::Table`; true when any segment holds a renderer-owned
    /// safe link (`Inline::Link` with a validated destination).
    pub(crate) has_table: bool,
    pub(crate) has_link: bool,
}

/// Parse an HTML candidate with an HTML5 parser, fold history and signatures
/// into separate inert prose, and return referenced inline image resources.
/// No resource is fetched and no sender-provided image embed is emitted.
pub(crate) fn render_html(
    html: &str,
    inline_images: &[InlineImage],
    bulletin: bool,
) -> HtmlRender {
    let document = Html::parse_document(html);
    let hidden_classes = hidden_css_classes(document.root_element());
    let mut context = WalkContext {
        hidden_classes,
        inline_images,
        referenced_images: Vec::new(),
        bulletin,
    };
    let nodes = parse_container(document.root_element(), &mut context);
    let mut segments = Segments::default();
    split_segments(&nodes, Segment::Main, &mut segments);
    let has_table = [&segments.main, &segments.quoted, &segments.signature]
        .iter()
        .any(|blocks| blocks_have_table(blocks));
    let has_link = [&segments.main, &segments.quoted, &segments.signature]
        .iter()
        .any(|blocks| blocks_have_link(blocks));
    HtmlRender {
        markdown: render_optional(&segments.main),
        quoted: render_nonempty(&segments.quoted),
        signature: render_nonempty(&segments.signature),
        inline_images: context.referenced_images,
        has_table,
        has_link,
    }
}

#[derive(Default)]
struct Segments {
    main: Vec<Block>,
    quoted: Vec<Block>,
    signature: Vec<Block>,
}

#[derive(Clone, Copy)]
enum Segment {
    Main,
    Quoted,
    Signature,
}

enum RenderNode {
    Block(Block),
    Quote(Vec<RenderNode>),
    Signature(Vec<RenderNode>),
}

struct WalkContext<'a> {
    hidden_classes: HashSet<String>,
    inline_images: &'a [InlineImage],
    referenced_images: Vec<InlineImage>,
    bulletin: bool,
}

fn render_optional(blocks: &[Block]) -> String {
    render_blocks(blocks).trim().to_string()
}

fn render_nonempty(blocks: &[Block]) -> Option<String> {
    let rendered = render_optional(blocks);
    (!rendered.is_empty()).then_some(rendered)
}

fn split_segments(nodes: &[RenderNode], target: Segment, out: &mut Segments) {
    for node in nodes {
        match node {
            RenderNode::Block(block) => match target {
                Segment::Main => out.main.push(block.clone()),
                Segment::Quoted => out.quoted.push(block.clone()),
                Segment::Signature => out.signature.push(block.clone()),
            },
            RenderNode::Quote(children) => {
                split_segments(children, Segment::Quoted, out);
            }
            RenderNode::Signature(children) => {
                split_segments(children, Segment::Signature, out);
            }
        }
    }
}

fn blocks_have_table(blocks: &[Block]) -> bool {
    blocks.iter().any(|block| match block {
        Block::Table { .. } => true,
        Block::List { items, .. } => {
            items.iter().any(|item| blocks_have_table(item))
        }
        _ => false,
    })
}

fn inlines_have_link(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Link { url: Some(_), .. } => true,
        Inline::Emphasis(children) | Inline::Strong(children) => {
            inlines_have_link(children)
        }
        Inline::Link { label, url: None } => inlines_have_link(label),
        _ => false,
    })
}

fn blocks_have_link(blocks: &[Block]) -> bool {
    blocks.iter().any(|block| match block {
        Block::Paragraph(inlines) => inlines_have_link(inlines),
        Block::List { items, .. } => {
            items.iter().any(|item| blocks_have_link(item))
        }
        Block::Heading { .. } | Block::Code(_) => false,
        Block::Table { headers, rows } => {
            headers.iter().any(|cell| inlines_have_link(cell))
                || rows
                    .iter()
                    .any(|row| row.iter().any(|cell| inlines_have_link(cell)))
        }
    })
}

fn parse_container(
    element: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> Vec<RenderNode> {
    let mut output = Vec::new();
    let mut inline = Vec::new();
    let mut quote_tail = false;

    for child in element.children() {
        let value = child.value();
        let Some(child_element) = ElementRef::wrap(child) else {
            if let Node::Text(text) = value {
                if quote_tail {
                    flush_inline(&mut inline, &mut output);
                    output.push(RenderNode::Quote(vec![RenderNode::Block(
                        Block::Paragraph(vec![Inline::Text(text.to_string())]),
                    )]));
                } else {
                    inline.push(Inline::Text(text.to_string()));
                }
            }
            continue;
        };

        if is_hidden(child_element, context)
            || drops_content(child_element.value().name())
        {
            continue;
        }

        if is_signature(child_element) {
            flush_inline(&mut inline, &mut output);
            output.push(RenderNode::Signature(parse_element_contents(
                child_element,
                context,
            )));
            continue;
        }

        if is_quote_container(child_element) {
            flush_inline(&mut inline, &mut output);
            output.push(RenderNode::Quote(parse_element_contents(
                child_element,
                context,
            )));
            continue;
        }

        if quote_tail {
            flush_inline(&mut inline, &mut output);
            output.push(RenderNode::Quote(parse_element_nodes(
                child_element,
                context,
            )));
            continue;
        }

        if is_quote_divider(child_element, context) {
            flush_inline(&mut inline, &mut output);
            quote_tail = true;
            output.push(RenderNode::Quote(parse_element_nodes(
                child_element,
                context,
            )));
            continue;
        }

        if is_block_element(child_element.value().name()) {
            flush_inline(&mut inline, &mut output);
            output.extend(parse_element_nodes(child_element, context));
        } else {
            inline.extend(parse_inline_element(child_element, context));
        }
    }

    flush_inline(&mut inline, &mut output);
    output
}

fn flush_inline(inline: &mut Vec<Inline>, output: &mut Vec<RenderNode>) {
    if !inline.is_empty() {
        output
            .push(RenderNode::Block(Block::Paragraph(std::mem::take(inline))));
    }
}

fn parse_element_nodes(
    element: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> Vec<RenderNode> {
    if is_hidden(element, context) || drops_content(element.value().name()) {
        return Vec::new();
    }
    match element.value().name() {
        "table" => parse_table(element, context),
        "ul" | "ol" => parse_list(element, context),
        "pre" => vec![RenderNode::Block(Block::Code(visible_text(
            element, context,
        )))],
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = element.value().name()[1..].parse::<u8>().unwrap_or(6);
            vec![RenderNode::Block(Block::Heading {
                level,
                text: visible_text(element, context),
            })]
        }
        "hr" => vec![RenderNode::Block(Block::Paragraph(vec![Inline::Text(
            "—".to_string(),
        )]))],
        "p" | "dt" | "dd" | "address" => vec![RenderNode::Block(
            Block::Paragraph(parse_inline_children(element, context)),
        )],
        _ => parse_container(element, context),
    }
}

fn parse_element_contents(
    element: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> Vec<RenderNode> {
    parse_element_nodes(element, context)
}

fn parse_list(
    element: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> Vec<RenderNode> {
    let mut items = Vec::new();
    let mut extra = Vec::new();
    for child in element.child_elements() {
        if child.value().name() != "li" || is_hidden(child, context) {
            continue;
        }
        let nodes = parse_container(child, context);
        let mut blocks = Vec::new();
        for node in nodes {
            match node {
                RenderNode::Block(block) => blocks.push(block),
                // Preserve quote/signature segmentation through lists:
                // quoted history inside an `<li>` belongs in the folded
                // history segment, not in the list item's main text.
                RenderNode::Quote(children) => {
                    extra.push(RenderNode::Quote(children));
                }
                RenderNode::Signature(children) => {
                    extra.push(RenderNode::Signature(children));
                }
            }
        }
        if !blocks.is_empty() {
            items.push(blocks);
        }
    }
    if items.is_empty() {
        if extra.is_empty() {
            return parse_container(element, context);
        }
        return extra;
    }
    let start = element
        .attr("start")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1);
    let mut out = vec![RenderNode::Block(Block::List {
        ordered: element.value().name() == "ol",
        start,
        items,
    })];
    out.extend(extra);
    out
}

fn parse_inline_children(
    element: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> Vec<Inline> {
    let mut output = Vec::new();
    for child in element.children() {
        let value = child.value();
        if let Some(child_element) = ElementRef::wrap(child) {
            output.extend(parse_inline_element(child_element, context));
        } else if let Node::Text(text) = value {
            output.push(Inline::Text(text.to_string()));
        }
    }
    output
}

fn parse_inline_element(
    element: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> Vec<Inline> {
    let name = element.value().name();
    if is_hidden(element, context) || drops_content(name) {
        return Vec::new();
    }
    match name {
        "br" => vec![Inline::Text("\n".to_string())],
        "img" => image_inline(element, context),
        "source" => Vec::new(),
        "svg" => {
            let title = element
                .child_elements()
                .find(|child| child.value().name() == "title")
                .map(|title| visible_text(title, context))
                .or_else(|| element.attr("aria-label").map(str::to_string));
            title
                .filter(|text| !text.trim().is_empty())
                .map(Inline::Text)
                .into_iter()
                .collect()
        }
        "object" | "embed" => {
            let fallback = element
                .attr("alt")
                .or_else(|| element.attr("title"))
                .map(str::to_string)
                .unwrap_or_else(|| visible_text(element, context));
            (!fallback.trim().is_empty())
                .then_some(Inline::Text(fallback))
                .into_iter()
                .collect()
        }
        "a" => {
            let label = parse_inline_children(element, context);
            let url = element.attr("href").and_then(SafeUrl::parse);
            vec![Inline::Link { label, url }]
        }
        "strong" | "b" => {
            vec![Inline::Strong(parse_inline_children(element, context))]
        }
        "em" | "i" => {
            vec![Inline::Emphasis(parse_inline_children(element, context))]
        }
        "code" | "kbd" | "samp" => {
            vec![Inline::Code(visible_text(element, context))]
        }
        "input" | "button" | "select" | "textarea" => Vec::new(),
        _ => parse_inline_children(element, context),
    }
}

fn image_inline(
    element: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> Vec<Inline> {
    if image_is_tracking(element) {
        return Vec::new();
    }
    let alt = element.attr("alt").unwrap_or("").trim();
    if context.bulletin {
        return (!alt.is_empty())
            .then(|| Inline::Text(alt.to_string()))
            .into_iter()
            .collect();
    }

    let src = element
        .attr("src")
        .filter(|src| !src.trim().is_empty())
        .or_else(|| element.attr("srcset").and_then(first_srcset_url));
    let Some(src) = src else {
        return image_label(alt, None).into_iter().collect();
    };

    let is_cid = src
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cid:"));
    if is_cid && let Some(cid) = normalize_cid(src) {
        let matches: Vec<&InlineImage> = context
            .inline_images
            .iter()
            .filter(|image| {
                image.content_id.as_deref().is_some_and(|candidate| {
                    normalize_cid(candidate).is_some_and(|candidate| {
                        candidate.eq_ignore_ascii_case(&cid)
                    })
                })
            })
            .collect();
        if matches.len() == 1 {
            context.referenced_images.push(matches[0].clone());
            return image_label(alt, None).into_iter().collect();
        }
        return image_label(alt, None).into_iter().collect();
    }

    let parsed = Url::parse(src).ok();
    let remote = parsed.as_ref().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && SafeUrl::parse(src).is_some()
    });
    if remote {
        let host = parsed.as_ref().and_then(Url::host_str).unwrap_or("");
        let label = if alt.is_empty() { host } else { alt };
        return vec![remote_image_link(label, src)];
    }
    image_label(alt, parsed.as_ref().and_then(Url::host_str))
        .into_iter()
        .collect()
}

fn image_label(alt: &str, host: Option<&str>) -> Option<Inline> {
    let label = if !alt.trim().is_empty() {
        format!("🖼 {}", alt.trim())
    } else if let Some(host) = host.filter(|host| !host.is_empty()) {
        format!("🖼 {host}")
    } else {
        "🖼 image".to_string()
    };
    Some(Inline::Text(label))
}

fn first_srcset_url(srcset: &str) -> Option<&str> {
    srcset.split(',').find_map(|candidate| {
        let url = candidate.split_whitespace().next()?;
        (!url.is_empty()).then_some(url)
    })
}

fn normalize_cid(value: &str) -> Option<String> {
    let mut value = value.trim();
    if value
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cid:"))
    {
        value = &value[4..];
    }
    value = value.trim().trim_start_matches('<').trim_end_matches('>');
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

fn parse_table(
    table: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> Vec<RenderNode> {
    let scan = scan_table(table, context);
    if context.bulletin || scan.unsupported || !scan.semantic {
        let mut out = linear_table(&scan);
        out.extend(scan.extra);
        return out;
    }
    let header_index = scan
        .rows
        .iter()
        .position(|row| row.has_header_cells)
        .or_else(|| (scan.has_thead || scan.caption.is_some()).then_some(0));
    let Some(header_index) =
        header_index.filter(|index| *index < scan.rows.len())
    else {
        let mut out = linear_table(&scan);
        out.extend(scan.extra);
        return out;
    };
    let headers = scan.rows[header_index].cells.clone();
    if headers.is_empty() || headers.iter().all(|cell| cell_is_empty(cell)) {
        let mut out = linear_table(&scan);
        out.extend(scan.extra);
        return out;
    }
    let rows = scan
        .rows
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != header_index)
        .map(|(_, row)| row.cells.clone())
        .collect();
    let mut output = Vec::new();
    if let Some(caption) =
        scan.caption.filter(|caption| !cell_is_empty(caption))
    {
        output.push(RenderNode::Block(Block::Paragraph(caption)));
    }
    output.push(RenderNode::Block(Block::Table { headers, rows }));
    output.extend(scan.extra);
    output
}

struct TableRow {
    cells: Vec<Vec<Inline>>,
    has_header_cells: bool,
}

struct TableScan {
    rows: Vec<TableRow>,
    has_thead: bool,
    caption: Option<Vec<Inline>>,
    semantic: bool,
    unsupported: bool,
    /// Quote/signature segments found inside layout/semantic table cells.
    /// Returned alongside the table so folded history never stays in main
    /// text when a Gmail quote or signature is wrapped in a table.
    extra: Vec<RenderNode>,
}

fn scan_table(
    table: ElementRef<'_>,
    context: &mut WalkContext<'_>,
) -> TableScan {
    let mut scan = TableScan {
        rows: Vec::new(),
        has_thead: false,
        caption: None,
        semantic: false,
        unsupported: false,
        extra: Vec::new(),
    };
    scan_table_children(table, &mut scan, context);
    scan.semantic = scan.has_thead
        || scan.caption.is_some()
        || scan.rows.iter().any(|row| row.has_header_cells);
    scan
}

fn scan_table_children(
    table: ElementRef<'_>,
    scan: &mut TableScan,
    context: &mut WalkContext<'_>,
) {
    for child in table.child_elements() {
        if is_hidden(child, context) || drops_content(child.value().name()) {
            continue;
        }
        match child.value().name() {
            "table" => {
                scan.unsupported = true;
            }
            "thead" => {
                scan.has_thead = true;
                scan_table_children(child, scan, context);
            }
            "caption" => {
                // Keep structured inlines so safe links survive layout
                // flattening and semantic rendering with final escaping owned
                // by the inert serializer.
                let caption = parse_inline_children(child, context);
                if !cell_is_empty(&caption) {
                    scan.caption = Some(caption);
                }
            }
            "tr" => {
                let mut cells = Vec::new();
                let mut has_header_cells = false;
                for cell in child.child_elements() {
                    let name = cell.value().name();
                    if !matches!(name, "th" | "td") || is_hidden(cell, context)
                    {
                        continue;
                    }
                    if has_expanding_span(cell) || contains_nested_table(cell) {
                        scan.unsupported = true;
                    }
                    has_header_cells |= name == "th";
                    // Block-aware cell parsing so a Gmail quote/signature
                    // wrapped in a layout table stays folded: main Blocks
                    // become the cell's inlines, Quote/Signature nodes go to
                    // `scan.extra` and are returned outside the table.
                    let nodes = parse_container(cell, context);
                    let mut main_blocks = Vec::new();
                    for node in nodes {
                        match node {
                            RenderNode::Block(block) => main_blocks.push(block),
                            RenderNode::Quote(children) => {
                                scan.extra.push(RenderNode::Quote(children));
                            }
                            RenderNode::Signature(children) => {
                                scan.extra
                                    .push(RenderNode::Signature(children));
                            }
                        }
                    }
                    cells.push(blocks_to_cell_inlines(&main_blocks));
                }
                if !cells.is_empty() {
                    scan.rows.push(TableRow {
                        cells,
                        has_header_cells,
                    });
                }
            }
            _ => scan_table_children(child, scan, context),
        }
    }
}

fn linear_table(scan: &TableScan) -> Vec<RenderNode> {
    let mut output = Vec::new();
    if let Some(caption) = scan.caption.as_ref()
        && !cell_is_empty(caption)
    {
        output.push(RenderNode::Block(Block::Paragraph(caption.clone())));
    }
    for row in &scan.rows {
        let mut combined: Vec<Inline> = Vec::new();
        for cell in &row.cells {
            if cell_is_empty(cell) {
                continue;
            }
            if !combined.is_empty() {
                combined.push(Inline::Text(" · ".to_string()));
            }
            combined.extend(cell.clone());
        }
        if combined.is_empty() {
            continue;
        }
        output.push(RenderNode::Block(Block::Paragraph(combined)));
    }
    output
}

/// True when a table cell's inlines hold no readable content: empty, or only
/// whitespace text with no structure (links, code, emphasis).
fn cell_is_empty(inlines: &[Inline]) -> bool {
    inlines.iter().all(|inline| match inline {
        Inline::Text(text) => text.trim().is_empty(),
        Inline::Emphasis(children) | Inline::Strong(children) => {
            cell_is_empty(children)
        }
        Inline::Code(text) => text.trim().is_empty(),
        Inline::Link { label, url } => url.is_none() && cell_is_empty(label),
    })
}

/// Flatten a table cell's main Blocks into inline content. Paragraphs
/// contribute their inlines; headings, lists, and nested tables are linearized
/// with readable separators so no cell text is lost when a layout table wraps
/// ordinary prose. Quote/Signature nodes never reach here: the caller splits
/// them into `scan.extra` before calling this.
fn blocks_to_cell_inlines(blocks: &[Block]) -> Vec<Inline> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                if !out.is_empty() && !inlines.is_empty() {
                    out.push(Inline::Text(" ".to_string()));
                }
                out.extend(inlines.clone());
            }
            Block::Heading { text, .. } => {
                if text.trim().is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(Inline::Text(" ".to_string()));
                }
                out.push(Inline::Text(text.clone()));
            }
            Block::List { items, .. } => {
                for item in items {
                    let item_inlines = blocks_to_cell_inlines(item);
                    if cell_is_empty(&item_inlines) {
                        continue;
                    }
                    if !out.is_empty() {
                        out.push(Inline::Text("; ".to_string()));
                    }
                    out.extend(item_inlines);
                }
            }
            Block::Code(content) => {
                if content.trim().is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(Inline::Text(" ".to_string()));
                }
                out.push(Inline::Code(content.clone()));
            }
            Block::Table { headers, rows } => {
                for header in headers {
                    if cell_is_empty(header) {
                        continue;
                    }
                    if !out.is_empty() {
                        out.push(Inline::Text(" ".to_string()));
                    }
                    out.extend(header.clone());
                }
                for row in rows {
                    for cell in row {
                        if cell_is_empty(cell) {
                            continue;
                        }
                        if !out.is_empty() {
                            out.push(Inline::Text(" ".to_string()));
                        }
                        out.extend(cell.clone());
                    }
                }
            }
        }
    }
    out
}

fn collapse_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn contains_nested_table(element: ElementRef<'_>) -> bool {
    element
        .descendent_elements()
        .any(|child| child.value().name() == "table")
}

fn has_expanding_span(element: ElementRef<'_>) -> bool {
    ["colspan", "rowspan"].iter().any(|attribute| {
        element
            .attr(attribute)
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|value| value > 1)
    })
}

fn is_quote_container(element: ElementRef<'_>) -> bool {
    match element.value().name() {
        "blockquote" => true,
        _ => has_class(element, "gmail_quote"),
    }
}

fn is_signature(element: ElementRef<'_>) -> bool {
    has_class(element, "gmail_signature")
}

fn is_quote_divider(
    element: ElementRef<'_>,
    context: &WalkContext<'_>,
) -> bool {
    // Outlook's reply separator uses `id`, not `class`: `<div
    // id='divRplyFwdMsg'>`. The existing test passed only via the bold-header
    // fallback, so check the id explicitly. Keep the class check for
    // forward-compatibility and require strong header evidence otherwise, so
    // ordinary prose mentioning From never folds.
    if element
        .attr("id")
        .is_some_and(|id| id.eq_ignore_ascii_case("divRplyFwdMsg"))
        || has_class(element, "divRplyFwdMsg")
    {
        return true;
    }
    if !matches!(element.value().name(), "div" | "p" | "section") {
        return false;
    }
    let text =
        collapse_space(&visible_text(element, context)).to_ascii_lowercase();
    if text.starts_with("on ") && text.contains(" wrote:") {
        return true;
    }
    if text.starts_with("-----original message-----") {
        return true;
    }
    let mut fields = HashSet::new();
    for child in element.descendent_elements() {
        if !matches!(child.value().name(), "b" | "strong")
            || is_hidden(child, context)
        {
            continue;
        }
        let label =
            collapse_space(&visible_text(child, context)).to_ascii_lowercase();
        for field in ["from:", "sent:", "to:", "subject:"] {
            if label.starts_with(field) {
                fields.insert(field);
            }
        }
    }
    fields.len() >= 2
}

fn has_class(element: ElementRef<'_>, wanted: &str) -> bool {
    element.attr("class").is_some_and(|classes| {
        classes
            .split_ascii_whitespace()
            .any(|class| class.eq_ignore_ascii_case(wanted))
    })
}

fn is_block_element(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "body"
            | "blockquote"
            | "div"
            | "dl"
            | "dt"
            | "dd"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "html"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
    )
}

fn drops_content(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "button"
            | "frame"
            | "frameset"
            | "form"
            | "head"
            | "iframe"
            | "input"
            | "link"
            | "meta"
            | "script"
            | "select"
            | "style"
            | "textarea"
            | "title"
    )
}

fn is_hidden(element: ElementRef<'_>, context: &WalkContext<'_>) -> bool {
    if element.attr("hidden").is_some()
        || element
            .attr("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        || class_is_hidden(element, &context.hidden_classes)
    {
        return true;
    }
    let style = element.attr("style").unwrap_or("");
    if style_is_hidden(style) {
        return true;
    }
    ["width", "height"].iter().any(|dimension| {
        element
            .attr(dimension)
            .and_then(parse_dimension)
            .is_some_and(|value| value == 0.0)
    })
}

fn image_is_tracking(element: ElementRef<'_>) -> bool {
    if ["width", "height"].iter().any(|dimension| {
        element
            .attr(dimension)
            .and_then(parse_dimension)
            .is_some_and(|value| value == 0.0)
    }) {
        return true;
    }
    let width = element.attr("width").and_then(parse_dimension).or_else(|| {
        style_dimension(element.attr("style").unwrap_or(""), "width")
    });
    let height =
        element
            .attr("height")
            .and_then(parse_dimension)
            .or_else(|| {
                style_dimension(element.attr("style").unwrap_or(""), "height")
            });
    width
        .zip(height)
        .is_some_and(|(width, height)| width <= 2.0 && height <= 2.0)
}

fn parse_dimension(value: &str) -> Option<f64> {
    let value = value
        .trim()
        .to_ascii_lowercase()
        .split("!important")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let numeric = value.strip_suffix("px").unwrap_or(&value).trim();
    numeric
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn style_dimension(style: &str, wanted: &str) -> Option<f64> {
    style_properties(style)
        .into_iter()
        .find_map(|(key, value)| {
            (key == wanted).then(|| parse_dimension(&value)).flatten()
        })
}

fn style_is_hidden(style: &str) -> bool {
    let properties = style_properties(style);
    let get = |key: &str| {
        properties
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value.as_str()))
    };
    if get("display").is_some_and(|value| value == "none")
        || get("visibility")
            .is_some_and(|value| matches!(value, "hidden" | "collapse"))
        || get("content-visibility").is_some_and(|value| value == "hidden")
        || get("mso-hide").is_some_and(|value| value == "all")
        || get("opacity")
            .and_then(|value| value.parse::<f64>().ok())
            .is_some_and(|value| value == 0.0)
        || ["width", "height"].iter().any(|dimension| {
            get(dimension)
                .and_then(parse_dimension)
                .is_some_and(|value| value == 0.0)
        })
    {
        return true;
    }
    let max_height_zero = get("max-height")
        .and_then(parse_dimension)
        .is_some_and(|value| value == 0.0);
    let overflow_hidden = ["overflow", "overflow-y"]
        .iter()
        .any(|key| get(key).is_some_and(|value| value == "hidden"));
    max_height_zero && overflow_hidden
}

fn style_properties(style: &str) -> Vec<(String, String)> {
    let without_comments = strip_css_comments(style);
    without_comments
        .split(';')
        .filter_map(|declaration| {
            let (key, value) = declaration.split_once(':')?;
            let key = key.trim().to_ascii_lowercase();
            let value = value
                .trim()
                .to_ascii_lowercase()
                .split("!important")
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            (!key.is_empty() && !value.is_empty()).then_some((key, value))
        })
        .collect()
}

fn strip_css_comments(css: &str) -> String {
    let mut output = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    while let Some(current) = chars.next() {
        if current == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(comment_char) = chars.next() {
                if comment_char == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
        } else {
            output.push(current);
        }
    }
    output
}

fn hidden_css_classes(root: ElementRef<'_>) -> HashSet<String> {
    let mut hidden = HashSet::new();
    for element in root.descendent_elements() {
        if element.value().name() != "style" {
            continue;
        }
        let css = element.text().collect::<String>();
        let css = strip_css_comments(&css);
        for rule in css.split('}') {
            let Some((selectors, declarations)) = rule.split_once('{') else {
                continue;
            };
            if !style_is_hidden(declarations) {
                continue;
            }
            for selector in selectors.split(',').map(str::trim) {
                let Some(class) = selector.strip_prefix('.') else {
                    continue;
                };
                if !class.is_empty()
                    && class.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '_' | '-')
                    })
                {
                    hidden.insert(class.to_ascii_lowercase());
                }
            }
        }
    }
    hidden
}

fn class_is_hidden(
    element: ElementRef<'_>,
    hidden_classes: &HashSet<String>,
) -> bool {
    element.attr("class").is_some_and(|classes| {
        classes
            .split_ascii_whitespace()
            .any(|class| hidden_classes.contains(&class.to_ascii_lowercase()))
    })
}

fn visible_text(element: ElementRef<'_>, context: &WalkContext<'_>) -> String {
    let mut output = String::new();
    append_visible_text(element, context, &mut output);
    output
}

fn append_visible_text(
    element: ElementRef<'_>,
    context: &WalkContext<'_>,
    output: &mut String,
) {
    if is_hidden(element, context) || drops_content(element.value().name()) {
        return;
    }
    for child in element.children() {
        let value = child.value();
        if let Some(child_element) = ElementRef::wrap(child) {
            if child_element.value().name() == "img" {
                if !image_is_tracking(child_element)
                    && let Some(alt) = child_element
                        .attr("alt")
                        .filter(|alt| !alt.trim().is_empty())
                {
                    output.push_str(alt.trim());
                    output.push(' ');
                }
            } else if child_element.value().name() == "svg" {
                if let Some(title) = child_element
                    .child_elements()
                    .find(|child| child.value().name() == "title")
                {
                    output.push_str(&title.text().collect::<String>());
                    output.push(' ');
                }
            } else if matches!(child_element.value().name(), "object" | "embed")
            {
                if let Some(alt) = child_element
                    .attr("alt")
                    .or_else(|| child_element.attr("title"))
                {
                    output.push_str(alt);
                    output.push(' ');
                } else {
                    append_visible_text(child_element, context, output);
                }
            } else {
                append_visible_text(child_element, context, output);
                if is_block_element(child_element.value().name())
                    || child_element.value().name() == "br"
                {
                    output.push(' ');
                }
            }
        } else if let Node::Text(text) = value {
            output.push_str(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::body::InlineImageSource;

    fn render(html: &str) -> HtmlRender {
        render_html(html, &[], false)
    }

    #[test]
    fn renders_html_structure_and_unwraps_layout_tables() {
        let result = render(
            "<h2>Receipt</h2><p>Hello <strong>friend</strong>.</p><table><tr><td>Order</td><td>42</td></tr><tr><td>Total</td><td>$8</td></tr></table>",
        );
        assert!(result.markdown.contains("## Receipt"));
        assert!(
            result.markdown.contains("Hello&#32;**friend**."),
            "{:?}",
            result.markdown
        );
        assert!(result.markdown.contains("Order · 42"));
        assert!(!result.markdown.contains("| Order |"));
    }

    #[test]
    fn renders_semantic_receipt_table_as_markdown() {
        let result = render(
            "<table><caption>Order summary</caption><thead><tr><th>Item</th><th>Qty</th></tr></thead><tbody><tr><td>Tea</td><td>2</td></tr></tbody></table>",
        );
        assert!(result.markdown.contains("Order summary"));
        assert!(result.markdown.contains("| Item | Qty |"));
        assert!(result.markdown.contains("| Tea | 2 |"));
    }

    #[test]
    fn folds_gmail_quotes_and_signatures_without_swallowing_new_reply() {
        let result = render(
            "<p>New reply</p><div class='gmail_quote'><blockquote>Old message</blockquote></div><p>One more thought</p><div class='gmail_signature'>Thanks, Alice</div>",
        );
        assert!(result.markdown.contains("New reply"));
        assert!(result.markdown.contains("One more thought"));
        assert!(!result.markdown.contains("Old message"));
        assert!(result.quoted.as_deref().unwrap().contains("Old message"));
        assert!(result
            .signature
            .as_deref()
            .unwrap()
            .contains("Thanks, Alice"));
    }

    #[test]
    fn recognizes_outlook_and_on_wrote_dividers_as_quote_tails() {
        let outlook = render(
            "<p>My reply</p><div id='divRplyFwdMsg'><b>From:</b> Pat<br><b>Sent:</b> Tuesday</div><div>Old conversation</div>",
        );
        assert!(outlook.markdown.contains("My reply"));
        assert!(outlook
            .quoted
            .as_deref()
            .unwrap()
            .contains("Old conversation"));

        let gmail = render(
            "<p>Current</p><div>On Tue, Pat wrote:</div><div>Earlier text</div>",
        );
        assert!(gmail.markdown.contains("Current"));
        assert!(gmail.quoted.as_deref().unwrap().contains("Earlier text"));
    }

    #[test]
    fn drops_hidden_preheaders_executable_content_and_forms() {
        let result = render(
            "<style>.preheader { display : none !important; }</style><div class='preheader'>hidden preview</div><script>evil()</script><form><p>form text</p></form><p>visible</p>",
        );
        assert!(result.markdown.contains("visible"));
        assert!(!result.markdown.contains("hidden preview"));
        assert!(!result.markdown.contains("evil"));
        assert!(!result.markdown.contains("form text"));
    }

    #[test]
    fn drops_zero_hidden_and_tracking_images_and_links_only_safe_remote_images()
    {
        let result = render(
            "<p><img src='https://mail.example/pixel' width='1' height='1'><img src='https://mail.example/hidden' style='display:none' alt='hidden'><img src='https://example.test/i.png' alt='map'></p><p><img src='javascript:alert(1)' alt='unsafe'></p>",
        );
        assert!(!result.markdown.contains("pixel"));
        assert!(!result.markdown.contains("hidden"));
        assert!(
            result
                .markdown
                .contains("[🖼 map](<https://example.test/i.png>)"),
            "{:?}",
            result.markdown
        );
        assert!(result.markdown.contains("🖼 unsafe"));
        assert!(!result.markdown.contains("![]("));
    }

    #[test]
    fn cid_images_resolve_only_unique_references_and_bulletins_drop_candidates()
    {
        let images = vec![
            InlineImage {
                content_id: Some("<logo>".to_string()),
                mime_type: "image/png".to_string(),
                size: Some(12),
                filename: Some("logo.png".to_string()),
                source: InlineImageSource::Bytes(vec![1, 2, 3]),
            },
            InlineImage {
                content_id: Some("<attached>".to_string()),
                mime_type: "image/jpeg".to_string(),
                size: Some(20),
                filename: None,
                source: InlineImageSource::AttachmentId(
                    "opaque-id".to_string(),
                ),
            },
        ];
        let html = "<p><img src='cid:logo' alt='Brand'><img src='cid:attached' alt='Photo'><img src='cid:missing' alt='Missing'></p>";
        let result = render_html(html, &images, false);
        assert_eq!(result.inline_images.len(), 2);
        assert_eq!(
            result.inline_images[0].filename.as_deref(),
            Some("logo.png")
        );
        assert!(matches!(
            result.inline_images[1].source,
            InlineImageSource::AttachmentId(ref id) if id == "opaque-id"
        ));
        assert!(result.markdown.contains("🖼 Brand"));
        assert!(result.markdown.contains("🖼 Photo"));
        assert!(result.markdown.contains("🖼 Missing"));

        let ambiguous = [images[0].clone(), images[0].clone()];
        let result = render_html(
            "<img src='cid:logo' alt='Ambiguous'>",
            &ambiguous,
            false,
        );
        assert!(result.inline_images.is_empty());
        assert!(result.markdown.contains("🖼 Ambiguous"));

        let bulletin = render_html(html, &images, true);
        assert!(bulletin.inline_images.is_empty());
        assert!(!bulletin.markdown.contains("!["));
    }

    #[test]
    fn picture_and_srcset_use_only_the_img_fallback_without_loading_sources() {
        let result = render(
            "<picture><source srcset='javascript:alert(1) 1x'><img srcset='https://example.test/chart.png 2x' alt='Chart'></picture><div style='background-image:url(https://example.test/bg.png)'>Newsletter text</div>",
        );
        assert!(result
            .markdown
            .contains("[🖼 Chart](<https://example.test/chart.png>)"));
        assert!(result.markdown.contains("Newsletter text"));
        assert!(!result.markdown.contains("bg.png"));
        assert!(!result.markdown.contains("javascript:"));
        assert!(result.inline_images.is_empty());
    }

    #[test]
    fn spanning_data_tables_fall_back_to_linear_text() {
        let result = render(
            "<table><caption>Items</caption><tr><th>Name</th><th>Value</th></tr><tr><td rowspan='2'>First</td><td>A</td></tr><tr><td>B</td></tr></table>",
        );
        assert!(result.markdown.contains("Items"));
        assert!(result.markdown.contains("First · A"));
        assert!(result.markdown.contains("B"));
        assert!(!result.markdown.contains("| Name |"));
    }

    #[test]
    fn unsafe_links_remain_text_and_html_is_deterministic_after_repair() {
        let html = "<p><a href='javascript:alert(1)' onclick='alert(2)'>click</a><a href='https://safe.example/?x=1&amp;y=2'>safe</a><broken><b>repaired";
        let first = render(html);
        let second = render(html);
        assert_eq!(first.markdown, second.markdown);
        assert!(first.markdown.contains("click"));
        assert!(!first.markdown.contains("javascript:"));
        assert!(first
            .markdown
            .contains("[safe](<https://safe.example/?x=1&y=2>)"));
        assert!(first.markdown.contains("repaired"));
        assert!(!first.markdown.contains("onclick"));
    }

    #[test]
    fn bulletin_flattens_tables_and_emits_no_image_links_or_imports() {
        let images = vec![InlineImage {
            content_id: Some("<logo>".to_string()),
            mime_type: "image/png".to_string(),
            size: Some(12),
            filename: None,
            source: InlineImageSource::AttachmentId("opaque".to_string()),
        }];
        let result = render_html(
            "<table><thead><tr><th>Plan</th><th>Price</th></tr></thead><tr><td>Basic</td><td>$4</td></tr></table><img src='https://image.example/a.png' alt='chart'><img src='cid:logo' alt='logo'>",
            &images,
            true,
        );
        assert!(result.markdown.contains("Plan · Price"));
        assert!(result.markdown.contains("Basic · $4"));
        assert!(result.markdown.contains("chart"));
        assert!(!result.markdown.contains("[🖼"));
        assert!(result.inline_images.is_empty());
    }

    #[test]
    fn layout_table_keeps_article_link_and_boundaries() {
        let result = render_html(
            "<table><tr><td><a href='https://example.test/article'>Read article</a></td></tr></table>",
            &[],
            true,
        );
        assert!(
            result.markdown.contains("https://example.test/article"),
            "article URL lost in:\n{}",
            result.markdown
        );
        assert!(result.has_link, "semantic link flag missing");
    }

    #[test]
    fn table_wrapped_quote_and_signature_stay_folded_with_new_reply_main() {
        let result = render(
            "<p>My reply</p><table><tr><td><div class='gmail_quote'>Old history</div></td></tr></table><p>After table</p>",
        );
        assert!(result.markdown.contains("My reply"));
        // Quote inside the layout table belongs in folded history, not main.
        assert!(!result.markdown.contains("Old history"));
        assert!(
            result.quoted.as_deref().is_some_and(|q| q.contains("Old")),
            "table-wrapped quote lost: main={:?} quoted={:?}",
            result.markdown,
            result.quoted
        );
        let sig = render(
            "<p>New reply</p><table><tr><td><div class='gmail_signature'>Best, Pat</div></td></tr></table>",
        );
        assert!(sig.markdown.contains("New reply"));
        assert!(
            sig.signature.as_deref().is_some_and(|s| s.contains("Pat")),
            "table-wrapped signature lost: main={:?} sig={:?}",
            sig.markdown,
            sig.signature
        );
    }

    #[test]
    fn outlook_id_divider_folds_without_bold_headers() {
        let result = render(
            "<p>My reply</p><div id='divRplyFwdMsg'>Earlier email</div><p>Old history</p>",
        );
        assert!(result.markdown.contains("My reply"));
        assert!(
            result.quoted.as_deref().is_some_and(|q| q.contains("Old")),
            "id-only Outlook divider did not fold: main={:?} quoted={:?}",
            result.markdown,
            result.quoted
        );
    }

    #[test]
    fn semantic_table_keeps_all_cells_without_truncation() {
        let result = render(
            "<table><thead><tr><th>A</th><th>B</th></tr></thead><tbody><tr><td>1</td><td>2</td><td>3</td></tr></tbody></table>",
        );
        assert!(result.markdown.contains('1'));
        assert!(result.markdown.contains('2'));
        assert!(result.markdown.contains('3'), "extra cell truncated");
        assert!(result.has_table);
    }
}
