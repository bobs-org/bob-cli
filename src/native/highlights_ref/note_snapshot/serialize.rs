//! The sole producer of the engine-neutral projected-Markdown contract.

use serde::Serialize;

use super::view::{
    Block, Chip, ChipKind, HeadingClass, Inline, LinkTarget, ListKind,
    SnapshotHeading, SnapshotSection, SnapshotTask, SnapshotTaskStatus,
    SnapshotView,
};

#[derive(Serialize)]
struct ProjectedMetadata {
    title: String,
    #[serde(rename = "bob-snapshot")]
    bob_snapshot: bool,
    #[serde(rename = "bob-kicker")]
    bob_kicker: String,
    #[serde(rename = "bob-display-title")]
    bob_display_title: String,
    #[serde(rename = "bob-dek", skip_serializing_if = "Option::is_none")]
    bob_dek: Option<String>,
    #[serde(rename = "bob-meta")]
    bob_meta: Vec<String>,
    #[serde(rename = "bob-stats")]
    bob_stats: Vec<ProjectedStatusCount>,
    #[serde(rename = "bob-omitted", skip_serializing_if = "Option::is_none")]
    bob_omitted: Option<String>,
    #[serde(rename = "bob-snapshot-line")]
    bob_snapshot_line: String,
    #[serde(rename = "bob-footer")]
    bob_footer: String,
    #[serde(rename = "bob-glyph-table")]
    bob_glyph_table: String,
    #[serde(rename = "bob-return-links")]
    bob_return_links: bool,
}

#[derive(Serialize)]
struct ProjectedStatusCount {
    status: &'static str,
    count: u64,
}

/// Serialize a complete view. Internal links are emitted only when their anchor is in
/// `view.anchors`, the final visible set; omitted targets are left as readable labels.
pub(crate) fn serialize_projected_markdown(
    view: &SnapshotView,
) -> Result<String, serde_yaml::Error> {
    let masthead = &view.masthead;
    let metadata = ProjectedMetadata {
        title: masthead.title.clone(),
        bob_snapshot: true,
        bob_kicker: render_inlines(view, &masthead.kicker),
        bob_display_title: render_inlines(view, &masthead.display_title),
        bob_dek: masthead
            .dek
            .as_ref()
            .map(|items| render_inlines(view, items)),
        bob_meta: masthead
            .meta
            .iter()
            .map(|items| render_inlines(view, items))
            .collect(),
        bob_stats: masthead
            .stats
            .iter()
            .map(|item| ProjectedStatusCount {
                status: item.status.as_str(),
                count: item.count,
            })
            .collect(),
        bob_omitted: masthead
            .omitted
            .as_ref()
            .map(|items| render_inlines(view, items)),
        bob_snapshot_line: render_inlines(view, &masthead.snapshot_line),
        bob_footer: render_inlines(view, &masthead.footer),
        bob_glyph_table: masthead.glyph_table.clone(),
        bob_return_links: masthead.return_links,
    };
    let yaml = serde_yaml::to_string(&metadata)?;

    let mut output = String::with_capacity(yaml.len() + 512);
    output.push_str("---\n");
    output.push_str(&yaml);
    output.push_str("---\n\n");

    let mut sections = view.sections.iter().collect::<Vec<_>>();
    if let Some(closed) = &view.closed {
        sections.push(closed);
    }
    for (index, section) in sections.iter().enumerate() {
        if index > 0 {
            output.push_str("\n\n");
        }
        output.push_str(&render_section(view, section));
    }
    if !sections.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

fn render_section(view: &SnapshotView, section: &SnapshotSection) -> String {
    let mut output = render_heading(view, &section.heading);
    let body = render_blocks(view, &section.blocks, 0);
    if !body.is_empty() {
        output.push_str("\n\n");
        output.push_str(&body);
    }
    output
}

fn render_blocks(
    view: &SnapshotView,
    blocks: &[Block],
    indent: usize,
) -> String {
    blocks
        .iter()
        .map(|block| render_block(view, block, indent))
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_block(view: &SnapshotView, block: &Block, indent: usize) -> String {
    let rendered = match block {
        Block::Paragraph(content) => render_inlines(view, content),
        Block::Heading(heading) => render_heading(view, heading),
        Block::List { kind, items } => render_list(view, *kind, items),
        Block::Code { language, text } => {
            render_code(language.as_deref(), text)
        }
        Block::Task(task) => render_task(view, task),
        Block::LabeledRow { label, items } => {
            let label = render_inlines(view, label);
            let items = items
                .iter()
                .map(|item| render_inlines(view, item))
                .collect::<Vec<_>>()
                .join(" · ");
            format!("- {label} {items}")
        }
        Block::LogRow {
            label,
            date,
            content,
            older_count,
        } => {
            let mut line = format!("- {}", render_chip_label(view, label));
            if let Some(date) = date {
                line.push_str(" *");
                line.push_str(&render_inlines(view, date));
                line.push('*');
            }
            if !content.is_empty() {
                line.push_str(" — ");
                line.push_str(&render_inlines(view, content));
            }
            if let Some(count) = older_count {
                line.push_str(&format!(" [+{count} older]{{.muted}}"));
            }
            line
        }
        Block::Image {
            asset_id,
            alt,
            width_percent,
        } => render_image(view, asset_id, alt, *width_percent),
        Block::MissingImage { label } => {
            format!("[{}]{{.missing}}", render_inlines(view, label))
        }
        Block::Embed { source, blocks } => {
            render_div(view, "embed", &[("source", source.as_str())], blocks)
        }
        Block::LiveQuery { blocks } => {
            render_div(view, "livequery", &[], blocks)
        }
        Block::Callout {
            kind,
            title,
            blocks,
        } => {
            let title = plain_inlines(title);
            render_div(
                view,
                "callout",
                &[("kind", kind.as_str()), ("title", title.as_str())],
                blocks,
            )
        }
        Block::Compact { blocks } => render_div(view, "compact", &[], blocks),
        Block::EmptyState(content) => {
            format!("[{}]{{.muted}}", render_inlines(view, content))
        }
    };
    indent_lines(&rendered, indent)
}

fn render_heading(view: &SnapshotView, heading: &SnapshotHeading) -> String {
    let anchor = safe_anchor_id(&heading.anchor.id);
    let mut attributes = format!("#{}", anchor);
    match heading.class {
        HeadingClass::Normal => {}
        HeadingClass::Lane => attributes.push_str(" .lane"),
        HeadingClass::Appendix => attributes.push_str(" .appendix"),
    }
    format!(
        "{} {} {{{attributes}}}",
        heading.level.marker(),
        render_inlines(view, &heading.content)
    )
}

fn render_task(view: &SnapshotView, task: &SnapshotTask) -> String {
    let mut line =
        format!("- []{{.st .st-{}}}", task_status_class(task.status));
    if let Some(anchor) = &task.anchor {
        line.push_str(&format!(" []{{#{}}}", safe_anchor_id(&anchor.id)));
    }
    if !task.content.is_empty() {
        line.push(' ');
        line.push_str(&render_inlines(view, &task.content));
    }
    for chip in &task.chips {
        line.push(' ');
        line.push_str(&render_chip(view, chip));
    }
    if !task.children.is_empty() {
        line.push('\n');
        line.push_str(&render_blocks(view, &task.children, 2));
    }
    line
}

fn task_status_class(status: SnapshotTaskStatus) -> &'static str {
    status.as_str()
}

fn render_list(
    view: &SnapshotView,
    kind: ListKind,
    items: &[super::view::ListItem],
) -> String {
    let mut output = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let marker = match kind {
            ListKind::Unordered => "- ".to_string(),
            ListKind::Ordered { start } => {
                format!("{}. ", start + index as u64)
            }
        };
        let mut line =
            format!("{marker}{}", render_inlines(view, &item.content));
        if !item.children.is_empty() {
            line.push('\n');
            line.push_str(&render_blocks(view, &item.children, marker.len()));
        }
        output.push(line);
    }
    output.join("\n")
}

fn render_chip_label(view: &SnapshotView, label: &[Inline]) -> String {
    format!("[{}]{{.label}}", render_inlines(view, label))
}

fn render_chip(view: &SnapshotView, chip: &Chip) -> String {
    let content = render_inlines(view, &chip.content);
    let classes = match chip.kind {
        ChipKind::Normal => ".chip",
        ChipKind::Accent => ".chip .chip-accent",
    };
    if let Some(target) = &chip.target {
        if let Some(destination) = render_target(view, target) {
            return format!("[{content}]({destination}){{{classes}}}");
        }
    }
    format!("[{content}]{{{classes}}}")
}

fn render_image(
    view: &SnapshotView,
    asset_id: &str,
    alt: &str,
    width_percent: Option<u8>,
) -> String {
    let Some(asset) = view.assets.iter().find(|asset| asset.id == asset_id)
    else {
        return format!(
            "[Missing image · {}]{{.missing}}",
            escape_text(asset_id)
        );
    };
    let path = asset.relative_path.to_string_lossy().replace('\\', "/");
    let destination = escape_destination(&path);
    let width = width_percent
        .or(asset.width_percent)
        .filter(|width| *width > 0);
    let width = width.map(|width| format!("{{width={}%}}", width.min(100)));
    format!(
        "![{}](<{destination}>){}",
        escape_text(alt),
        width.unwrap_or_default()
    )
}

fn render_div(
    view: &SnapshotView,
    class: &str,
    attributes: &[(&str, &str)],
    blocks: &[Block],
) -> String {
    let mut classes = format!(".{}", safe_class(class));
    for (name, value) in attributes {
        classes.push_str(&format!(" {name}=\"{}\"", escape_attribute(value)));
    }
    let body = render_blocks(view, blocks, 0);
    let fence = ":".repeat(dynamic_fence_len(&body, ':'));
    if body.is_empty() {
        format!("{fence} {{{classes}}}\n{fence}")
    } else {
        format!("{fence} {{{classes}}}\n{body}\n{fence}")
    }
}

fn render_code(language: Option<&str>, code: &str) -> String {
    let fence = "`".repeat(dynamic_fence_len(code, '`'));
    let language = language
        .filter(|language| !language.is_empty())
        .map(|language| format!(" {}", safe_info_string(language)))
        .unwrap_or_default();
    let mut output = format!("{fence}{language}\n{code}");
    if !code.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&fence);
    output
}

fn dynamic_fence_len(content: &str, fence_char: char) -> usize {
    let longest = content
        .lines()
        .map(|line| {
            line.chars()
                .take_while(|character| *character == fence_char)
                .count()
        })
        .max()
        .unwrap_or_default();
    longest.saturating_add(1).max(3)
}

fn safe_info_string(language: &str) -> String {
    language
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '+' | '.')
        })
        .collect()
}

fn safe_class(class: &str) -> String {
    let class = class
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
        })
        .collect::<String>();
    if class.is_empty() {
        "missing".to_string()
    } else {
        class
    }
}

fn safe_anchor_id(id: &str) -> String {
    let mut result = String::with_capacity(id.len());
    for character in id.chars() {
        if character.is_ascii_alphanumeric()
            || matches!(character, '-' | '_' | '.' | ':')
        {
            result.push(character);
        } else {
            result.push('-');
        }
    }
    if result
        .as_bytes()
        .first()
        .is_none_or(|first| !first.is_ascii_alphanumeric())
    {
        result.insert_str(0, "id-");
    }
    result
}

fn render_inlines(view: &SnapshotView, inlines: &[Inline]) -> String {
    let mut output = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) => output.push_str(&escape_text(text)),
            Inline::Link { label, target } => {
                let label = render_inlines(view, label);
                if let Some(destination) = render_target(view, target) {
                    output.push_str(&format!("[{label}]({destination})"));
                } else {
                    output.push_str(&label);
                }
            }
            Inline::Emphasis(content) => {
                output.push('*');
                output.push_str(&render_inlines(view, content));
                output.push('*');
            }
            Inline::Strong(content) => {
                output.push_str("**");
                output.push_str(&render_inlines(view, content));
                output.push_str("**");
            }
            Inline::Strike(content) => {
                output.push_str(&render_strike(view, content))
            }
            Inline::Code(code) => output.push_str(&render_code_span(code)),
            Inline::Chip(chip) => output.push_str(&render_chip(view, chip)),
            Inline::Muted(content) => {
                output.push('[');
                output.push_str(&render_inlines(view, content));
                output.push_str("]{.muted}");
            }
            Inline::Break => output.push_str("  \n"),
        }
    }
    output
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StrikePiece {
    Text {
        wrappers: Vec<InlineWrapper>,
        content: Inline,
    },
    Link {
        label: Vec<Inline>,
        target: LinkTarget,
    },
    LinkChip(Chip),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InlineWrapper {
    Emphasis,
    Strong,
    Muted,
}

fn render_strike(view: &SnapshotView, content: &[Inline]) -> String {
    let mut pieces = Vec::new();
    collect_strike_pieces(content, &[], &mut pieces);

    let mut output = String::new();
    let mut current_wrappers: Option<Vec<InlineWrapper>> = None;
    let mut current_content = Vec::new();
    let mut current_strike = String::new();

    for piece in pieces {
        match piece {
            StrikePiece::Text { wrappers, content } => {
                if current_wrappers
                    .as_ref()
                    .is_some_and(|current| *current != wrappers)
                {
                    current_strike.push_str(&render_strike_text_group(
                        view,
                        current_wrappers.take().as_deref().unwrap_or_default(),
                        &current_content,
                    ));
                    current_content.clear();
                }
                if current_wrappers.is_none() {
                    current_wrappers = Some(wrappers);
                }
                current_content.push(content);
            }
            StrikePiece::Link { label, target } => {
                flush_strike_text_group(
                    view,
                    &mut current_strike,
                    &mut current_wrappers,
                    &mut current_content,
                );
                flush_strike_run(&mut output, &mut current_strike);
                let label = render_inlines(view, &label);
                if let Some(destination) = render_target(view, &target) {
                    output.push_str(&format!("[{label}]({destination})"));
                } else {
                    output.push_str(&label);
                }
            }
            StrikePiece::LinkChip(chip) => {
                flush_strike_text_group(
                    view,
                    &mut current_strike,
                    &mut current_wrappers,
                    &mut current_content,
                );
                flush_strike_run(&mut output, &mut current_strike);
                output.push_str(&render_chip(view, &chip));
            }
        }
    }
    flush_strike_text_group(
        view,
        &mut current_strike,
        &mut current_wrappers,
        &mut current_content,
    );
    flush_strike_run(&mut output, &mut current_strike);
    output
}

fn flush_strike_text_group(
    view: &SnapshotView,
    current_strike: &mut String,
    current_wrappers: &mut Option<Vec<InlineWrapper>>,
    current_content: &mut Vec<Inline>,
) {
    if current_content.is_empty() {
        *current_wrappers = None;
        return;
    }
    current_strike.push_str(&render_strike_text_group(
        view,
        current_wrappers.take().as_deref().unwrap_or_default(),
        current_content,
    ));
    current_content.clear();
}

fn render_strike_text_group(
    view: &SnapshotView,
    wrappers: &[InlineWrapper],
    content: &[Inline],
) -> String {
    let mut rendered = render_inlines(view, content);
    for wrapper in wrappers.iter().rev() {
        rendered = match wrapper {
            InlineWrapper::Emphasis => format!("*{rendered}*"),
            InlineWrapper::Strong => format!("**{rendered}**"),
            InlineWrapper::Muted => format!("[{rendered}]{{.muted}}"),
        };
    }
    rendered
}

fn flush_strike_run(output: &mut String, current_strike: &mut String) {
    if !current_strike.is_empty() {
        output.push_str("~~");
        output.push_str(current_strike);
        output.push_str("~~");
        current_strike.clear();
    }
}

fn collect_strike_pieces(
    inlines: &[Inline],
    wrappers: &[InlineWrapper],
    output: &mut Vec<StrikePiece>,
) {
    for inline in inlines {
        match inline {
            Inline::Link { label, target } => output.push(StrikePiece::Link {
                label: label.clone(),
                target: target.clone(),
            }),
            Inline::Chip(chip) if chip.target.is_some() => {
                output.push(StrikePiece::LinkChip(chip.clone()))
            }
            Inline::Emphasis(content) => collect_with_wrapper(
                content,
                wrappers,
                InlineWrapper::Emphasis,
                output,
            ),
            Inline::Strong(content) => collect_with_wrapper(
                content,
                wrappers,
                InlineWrapper::Strong,
                output,
            ),
            Inline::Muted(content) => collect_with_wrapper(
                content,
                wrappers,
                InlineWrapper::Muted,
                output,
            ),
            Inline::Strike(content) => {
                collect_strike_pieces(content, wrappers, output)
            }
            content => output.push(StrikePiece::Text {
                wrappers: wrappers.to_vec(),
                content: content.clone(),
            }),
        }
    }
}

fn collect_with_wrapper(
    content: &[Inline],
    wrappers: &[InlineWrapper],
    wrapper: InlineWrapper,
    output: &mut Vec<StrikePiece>,
) {
    let mut nested = wrappers.to_vec();
    nested.push(wrapper);
    collect_strike_pieces(content, &nested, output);
}

fn render_code_span(code: &str) -> String {
    let longest = code
        .chars()
        .fold((0_usize, 0_usize), |(longest, current), character| {
            if character == '`' {
                let current = current + 1;
                (longest.max(current), current)
            } else {
                (longest, 0)
            }
        })
        .0;
    let fence = "`".repeat(longest + 1);
    if code.starts_with('`')
        || code.starts_with(' ')
        || code.ends_with('`')
        || code.ends_with(' ')
    {
        format!("{fence} {code} {fence}")
    } else {
        format!("{fence}{code}{fence}")
    }
}

fn render_target(view: &SnapshotView, target: &LinkTarget) -> Option<String> {
    let destination = match target {
        LinkTarget::Url(destination) | LinkTarget::ObsidianUri(destination) => {
            destination.clone()
        }
        LinkTarget::Internal(identity) => {
            let anchor = view
                .anchors
                .iter()
                .find(|anchor| &anchor.identity == identity)?;
            format!("#{}", safe_anchor_id(&anchor.id))
        }
    };
    Some(format!("<{}>", escape_destination(&destination)))
}

fn escape_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_punctuation() {
            output.push('\\');
        }
        output.push(character);
    }
    output
}

fn escape_destination(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b' ' => output.push_str("%20"),
            b'\n' => output.push_str("%0A"),
            b'\r' => output.push_str("%0D"),
            b'\t' => output.push_str("%09"),
            b'<' => output.push_str("%3C"),
            b'>' => output.push_str("%3E"),
            b'\\' => output.push_str("%5C"),
            _ if byte.is_ascii() => output.push(byte as char),
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', " ")
        .replace('\r', " ")
}

fn plain_inlines(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text(text) | Inline::Code(text) => text.clone(),
            Inline::Link { label, .. } => plain_inlines(label),
            Inline::Emphasis(content)
            | Inline::Strong(content)
            | Inline::Strike(content)
            | Inline::Muted(content) => plain_inlines(content),
            Inline::Chip(chip) => plain_inlines(&chip.content),
            Inline::Break => " ".to_string(),
        })
        .collect::<String>()
}

fn indent_lines(value: &str, indent: usize) -> String {
    if indent == 0 || value.is_empty() {
        return value.to_string();
    }
    let prefix = " ".repeat(indent);
    value
        .split('\n')
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
