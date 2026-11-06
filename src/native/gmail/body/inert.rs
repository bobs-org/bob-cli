//! Inert serialization boundary for Gmail sender text.
//!
//! Every sender-derived string (subjects, bodies, attachment names, snippets,
//! link labels, URLs) passes through this module before it can reach a note.
//! The structured [`Block`]/[`Inline`] representation lets renderer-owned
//! structure (paragraphs, headings, lists, emphasis, tables, links) survive
//! while sender content can never introduce structure of its own: sender text
//! enters only as `String` payloads inside [`Inline::Text`], [`Block::Heading`]
//! text, [`Block::Code`] content, or table cells, and the final serializer
//! ([`render_blocks`]) escapes each payload at render time. There is no path
//! that concatenates an already-sanitized string with unchecked sender bytes.
//!
//! Later phases (`html`, `plain`, body integration) construct [`Block`] values
//! from decoded mail and serialize them here. Contexts without structure
//! (frontmatter values, H1 lines, aliases, YAML documents) use the documented
//! single-application helpers instead:
//!
//! * [`inert_single_line`] for headers, snippets, and filename display text.
//!   Filesystem slug/path policy stays with the save phase; this helper only
//!   neutralizes syntax.
//! * [`inert_multiline`] for prose embedded without block structure.
//! * [`inert_label`] for Markdown link labels and wikilink aliases.
//! * [`inert_yaml_value`] for values destined for a YAML document. YAML quoting
//!   itself stays with `serde_yaml` in the note layer; the helper neutralizes
//!   sender syntax so the serialized document and the decoded value are inert.
//!
//! Every helper is single-application: applying it twice escapes the backslashes
//! the first pass inserted. Callers pass raw sender text exactly once.
//!
//! Escaping favors readable visible text over stripping: delimiters are broken
//! with backslash escapes (which render as the original character) or, inside
//! code, with a zero-width non-joiner (which renders invisibly). The
//! adversarial tests in this file prove the protection against Bob's own
//! scanners rather than against a copy of this implementation.

use url::Url;

/// Zero-width non-joiner: the invisible breaker used inside code content,
/// where backslash escapes would show literally.
const ZW_NON_JOINER: char = '\u{200c}';

/// Renderer-owned block structure. Sender text appears only in the `String`
/// payloads, which [`render_blocks`] escapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    /// One or more sender lines separated by soft breaks.
    Paragraph(Vec<Inline>),
    /// Renderer-chosen level (clamped to 1-6); `text` is raw sender text.
    Heading { level: u8, text: String },
    /// Renderer-owned markers; sender text lives in the item blocks.
    /// Checkbox syntax (`- [ ]`) is never emitted.
    List {
        ordered: bool,
        start: u32,
        items: Vec<Vec<Block>>,
    },
    /// Fenced code with exactly the `text` info string. Delimiter runs size
    /// past any backtick run in the content.
    Code(String),
    /// Sender cells as structured inlines; pipes and newlines are
    /// neutralized at render while safe links, emphasis, and code survive as
    /// renderer-owned structure with final escaping owned here. When
    /// `headers` is empty the rows fall back to `"; "`-joined paragraphs so
    /// no table syntax is emitted from headerless data.
    Table {
        headers: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
}

/// Renderer-owned inline structure. Sender text appears only in the `String`
/// payloads. There is deliberately no image variant: sender images become
/// ordinary safe links via [`remote_image_link`], never `![]()` syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Inline {
    /// Raw sender text, escaped at render time.
    Text(String),
    /// Renderer-owned `*...*` around already-structured children.
    Emphasis(Vec<Inline>),
    /// Renderer-owned `**...**` around already-structured children.
    Strong(Vec<Inline>),
    /// Raw sender text rendered as an inline code span with sized
    /// delimiters. Leading `=`/`$=` query openers are broken.
    Code(String),
    /// Renderer-owned link. `url` is `None` for rejected destinations; the
    /// label text still renders, so invalid links retain readable text.
    Link {
        label: Vec<Inline>,
        url: Option<SafeUrl>,
    },
}

/// A validated absolute link destination. Only `http`, `https`, and `mailto`
/// survive [`SafeUrl::parse`]; everything else (including control-obfuscated
/// schemes, protocol-relative, and relative targets) is rejected. The inner
/// string is the `url`-crate serialization, which percent-encodes destination
/// delimiters while leaving ordinary fragments and query parameters intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SafeUrl {
    inner: String,
}

impl SafeUrl {
    /// Validate `raw` as an absolute `http(s)`/`mailto` URL.
    ///
    /// Returns `None` when the target is relative, uses another scheme, or
    /// carries bytes that URL serialization cannot make destination-safe
    /// (controls, `<`, `>`, `"`). Plain spaces are accepted and percent-encoded
    /// by serialization; comparison is on the parsed scheme, so `HTTP://`
    /// normalizes and passes while `java\tscript:` fails.
    ///
    /// Raw wikilink delimiters (`[[`, `]]`) are refused even when the `url`
    /// crate leaves them unencoded: Bob's raw scanner still sees them inside
    /// the rendered `[label](<dest>)` destination. Ordinary query parameters,
    /// fragments, `mailto` addresses, and valid IPv6 hosts (for example
    /// `http://[::1]/`) never contain those pairs and stay usable. Refusal
    /// keeps readable label text while never Markdown-escaping opaque IDs or
    /// mutating transport inputs.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        if raw.chars().any(|c| {
            c.is_control() || c == '<' || c == '>' || c == '"' || c == '\u{7f}'
        }) {
            return None;
        }
        let url = Url::parse(raw.trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https" | "mailto") {
            return None;
        }
        if url.scheme() == "mailto" && url.path().is_empty() {
            return None;
        }
        let inner = url.to_string();
        if inner.chars().any(|c| {
            c.is_control() || c == ' ' || c == '<' || c == '>' || c == '"'
        }) {
            return None;
        }
        if inner.contains("[[") || inner.contains("]]") {
            return None;
        }
        Some(Self { inner })
    }

    /// The normalized destination, safe to embed as `[label](<dest>)`.
    pub(crate) fn as_str(&self) -> &str {
        &self.inner
    }
}

/// Render structured blocks to inert Markdown.
///
/// This is the final serialization boundary: every sender payload is escaped
/// here, so callers must never pre-escape text placed into [`Block`] values.
pub(crate) fn render_blocks(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        render_block(block, 0, &mut out);
    }
    out
}

/// Split raw sender prose into paragraph blocks on blank lines.
///
/// Each physical line is trimmed of surrounding indentation (sender
/// indentation never becomes a list level or an indented code block) and
/// escaped for both inline and line-start hazards at render time.
#[cfg(test)]
pub(crate) fn prose_blocks(text: &str) -> Vec<Block> {
    let normalized = normalize_sender(text);
    let mut blocks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in normalized.split('\n') {
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(Block::Paragraph(vec![Inline::Text(
                    current.join("\n"),
                )]));
                current.clear();
            }
            continue;
        }
        current.push(line.to_string());
    }
    if !current.is_empty() {
        blocks.push(Block::Paragraph(vec![Inline::Text(current.join("\n"))]));
    }
    blocks
}

/// Neutralize raw sender text for single-line contexts: headers, snippets,
/// H1 lines, table cells before pipe handling, link labels, and filename
/// display text.
///
/// Collapses every whitespace run (including newlines) to one space, strips
/// surrounding space, then breaks every inline hazard. The result is safe to
/// embed on one renderer-owned line. Single-application: pass raw text once.
pub(crate) fn inert_single_line(text: &str) -> String {
    escape_inline_text(&collapse_whitespace(text))
}

/// Neutralize raw sender prose that is embedded without block structure.
///
/// Keeps one output line per input line and applies both inline and
/// line-start escaping to each. Prefer [`prose_blocks`] plus [`render_blocks`]
/// when block structure is available.
#[cfg(test)]
pub(crate) fn inert_multiline(text: &str) -> String {
    render_text_multiline(&normalize_sender(text))
}

/// Neutralize raw sender text for use as a Markdown link label or wikilink
/// alias. Same neutralization as [`inert_single_line`]; the separate name
/// records the caller's contract (label text, never a destination).
#[cfg(test)]
pub(crate) fn inert_label(text: &str) -> String {
    inert_single_line(text)
}

/// Neutralize a raw sender value destined for a YAML document.
///
/// Applies the same syntax neutralization as [`inert_single_line`]; YAML
/// quoting itself stays with `serde_yaml` in the note layer. The serialized
/// document and the decoded value both stay free of executable structure.
#[cfg(test)]
pub(crate) fn inert_yaml_value(text: &str) -> String {
    inert_single_line(text)
}

/// Build the inert inline for a sender-provided remote image.
///
/// A remote image never becomes image syntax. Allowed destinations yield an
/// ordinary safe link labelled with a frame emoji plus the alt text or host;
/// anything else yields the same label as plain text. Dropping tracking pixels
/// is the HTML phase's job; this boundary only guarantees the image can never
/// load.
pub(crate) fn remote_image_link(alt_or_host: &str, url: &str) -> Inline {
    let visible = alt_or_host.trim();
    let label = if visible.is_empty() {
        "🖼 image".to_string()
    } else {
        format!("🖼 {visible}")
    };
    match SafeUrl::parse(url) {
        Some(safe) => Inline::Link {
            label: vec![Inline::Text(label)],
            url: Some(safe),
        },
        None => Inline::Text(label),
    }
}

/// Emit a writer-owned local image embed.
///
/// Only the save phase calls this, with a path it has already copied into the
/// vault. Sender URLs and absolute or parent-climbing paths are refused so a
/// forged `cid:` or `file:` reference can never become a local embed through
/// this function. Returns `None` for anything outside the narrow accepted
/// shape (`name`, `dir/name`, vault-safe characters only).
#[cfg(test)]
pub(crate) fn verified_local_image_embed(
    relative_path: &str,
    alt: &str,
) -> Option<String> {
    if !is_writer_image_path(relative_path) {
        return None;
    }
    Some(format!("![{}]({relative_path})", inert_single_line(alt)))
}

/// Display width for trusted local image wikilink embeds minted by the save
/// phase. The width suffix keeps saved mail images at a readable size; it is
/// a display hint, not a pixel rewrite of the imported bytes.
pub(crate) const LOCAL_IMAGE_DISPLAY_WIDTH: u32 = 400;

/// Emit a writer-owned local image wikilink embed (`![[path|400]]`).
///
/// Only the save phase calls this, with a path it has already copied into the
/// vault under its image directory. The same writer-path gate as
/// [`verified_local_image_embed`] applies, so a forged sender reference can
/// never mint this syntax: only a validated writer-owned imported path
/// produces an embed.
pub(crate) fn verified_local_image_wikilink(
    relative_path: &str,
) -> Option<String> {
    if !is_writer_image_path(relative_path) {
        return None;
    }
    Some(format!("![[{relative_path}|{LOCAL_IMAGE_DISPLAY_WIDTH}]]"))
}

/// Vault-relative writer-image shape: `name` or `dir/name` with slug-safe
/// characters only. Empty, absolute, parent-climbing, repeated-separator, and
/// scheme/query/anchor/quoting shapes are all refused as sender shapes.
fn is_writer_image_path(relative_path: &str) -> bool {
    if relative_path.is_empty()
        || relative_path.starts_with('/')
        || relative_path.contains("..")
        || relative_path.contains("//")
    {
        return false;
    }
    // Writer paths are vault-relative slugs: letters, digits, and a small
    // set of safe separators. Anything else (schemes, queries, anchors,
    // quoting characters) is refused as a sender shape.
    relative_path.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/')
    })
}

/// Normalize sender text without changing its readable content: CRLF/CR and
/// Unicode line/paragraph separators become `\n`, and stray control
/// characters (except `\n` and `\t`) are dropped so they cannot hide or
/// rejoin delimiter pairs. All other characters, including Unicode spacing
/// and emoji, are preserved.
fn normalize_sender(text: &str) -> String {
    normalize_crlf(text)
}

/// Normalize line endings and separators, dropping stray controls.
///
/// CRLF pairs fold to one newline, lone CR and Unicode separators become
/// newlines, and remaining C0/C1 controls (except tab) are dropped so they
/// can neither hide nor rejoin delimiter pairs.
fn normalize_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\u{85}' | '\u{2028}' | '\u{2029}' => out.push('\n'),
            c if c == '\t' || c == '\n' => out.push(c),
            c if c.is_control() || matches!(c, '\u{7f}'..='\u{9f}') => {}
            c => out.push(c),
        }
    }
    out
}

/// Break every inline hazard in one sender line.
///
/// Layering matters: sender backslashes are doubled first so they cannot form
/// escapes that undo the passes that follow. Backslash escapes render as the
/// original character, so readable text is preserved.
fn escape_inline_text(text: &str) -> String {
    let staged = double_backslashes_before_punct(text);
    // Templater openers start with "<%": break the pair before "<" is
    // encoded, so no "<%" substring can survive.
    let staged = staged.replace("<%", "<\\%");
    // "<" becomes an entity, not a backslash escape: "\<" would still leave
    // the "<!--" substring intact, while "&lt;" removes every "<" opener
    // (tags, comments, Templater, bob:gmail markers) and renders as "<".
    let staged = staged.replace('<', "&lt;");
    let staged = staged.replace('`', "\\`");
    let staged = staged.replace('[', "\\[").replace(']', "\\]");
    let staged = escape_colon_runs(&staged);
    let staged = escape_tag_hashes(&staged);
    let staged = staged.replace("%%", "%&#37;");
    let staged = escape_repeated_dollars(&staged);
    let staged = escape_trailing_block_id(&staged);
    staged.replace('🍅', "&#127813;")
}

/// A backslash before ASCII punctuation would form a CommonMark escape, so
/// double it: organizer-typed `\[` stays literal instead of unescaping the
/// `[` this module is about to neutralize.
fn double_backslashes_before_punct(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &current) in chars.iter().enumerate() {
        out.push(current);
        if current == '\\'
            && chars
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_punctuation())
        {
            out.push('\\');
        }
    }
    out
}

/// Escape every `#` that starts tag-shaped text (`#tag`, `#a/b`), unless it
/// is part of `##`, follows an alphanumeric (as in `C#`), or is followed by
/// whitespace or another `#`. Broader than the vault's mid-line escaper on
/// purpose: sender brackets become `\[...\]`, so a tag inside them must still
/// break. Renders as the original `#`.
fn escape_tag_hashes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &current) in chars.iter().enumerate() {
        if current == '#'
            && chars.get(index + 1).is_some_and(|next| {
                next.is_ascii_alphanumeric() || matches!(next, '_' | '-' | '/')
            })
            && (index == 0
                || (!chars[index - 1].is_alphanumeric()
                    && chars[index - 1] != '#'))
        {
            out.push('\\');
        }
        out.push(current);
    }
    out
}

/// `\$` for every `$` on a line that holds two or more of them (inline math
/// such as `$5-$10`). A lone `$` (`Q4: $5`) is literal and stays.
fn escape_repeated_dollars(text: &str) -> String {
    if text.chars().filter(|&c| c == '$').count() < 2 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '$' {
            out.push_str("\\$");
        } else {
            out.push(c);
        }
    }
    out
}

/// Escape every colon in any run of two or more colons (`::` -> `\:\:`).
/// A non-overlapping replace leaves `::` in odd-length runs, so runs are
/// scanned manually. Breaks bare `key:: value` Dataview fields as well as
/// the bracketed forms (whose brackets are escaped separately).
fn escape_colon_runs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == ':' {
            let mut end = index;
            while end < chars.len() && chars[end] == ':' {
                end += 1;
            }
            if end - index >= 2 {
                for _ in index..end {
                    out.push_str("\\:");
                }
            } else {
                out.push(':');
            }
            index = end;
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

/// `\^id` when the text ends with a trailing ` ^id` block id.
///
/// Mirrors `collect_done::trailing_block_id_in_line`: the caret must start
/// the text or follow Unicode whitespace and be followed only by block-id
/// bytes. The backslash breaks the match because the caret no longer follows
/// whitespace.
fn escape_trailing_block_id(text: &str) -> String {
    let trimmed = text.trim_end();
    let Some(caret) = trimmed.rfind('^') else {
        return text.to_string();
    };
    let id = &trimmed[caret + 1..];
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || (caret > 0
            && !trimmed[..caret]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace))
    {
        return text.to_string();
    }
    let trailing = &text[trimmed.len()..];
    format!("{}\\^{id}{trailing}", &trimmed[..caret])
}

/// Escape the line-start hazards of one sender line: headings, quotes,
/// tables, list markers (bare and ordered, after any indentation), thematic
/// breaks, and fences. Leading indentation is stripped first so sender
/// indentation can never become a list level or an indented code block.
fn escape_line_start(line: &str) -> String {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with('#') {
        return format!("\\{trimmed}");
    }
    if let Some((digits, marker, rest)) = split_ordered_marker(trimmed) {
        return format!("{digits}\\{marker}{rest}");
    }
    if trimmed.starts_with('>')
        || trimmed.starts_with('|')
        || starts_with_bare_list_marker(trimmed)
        || is_thematic_break(trimmed)
        || is_code_fence(trimmed)
    {
        return format!("\\{trimmed}");
    }
    trimmed.to_string()
}

/// Split a leading `N.` / `N)` ordered marker (digits, punctuation, then
/// whitespace or end) into its parts.
fn split_ordered_marker(text: &str) -> Option<(String, char, String)> {
    let digits: String =
        text.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &text[digits.len()..];
    let mut chars = rest.chars();
    let marker = chars.next()?;
    if !matches!(marker, '.' | ')') {
        return None;
    }
    let after = chars.as_str();
    if after.chars().next().is_none_or(|c| c.is_whitespace()) {
        Some((digits, marker, after.to_string()))
    } else {
        None
    }
}

/// A leading `+`, `-`, or `*` followed by whitespace or end.
fn starts_with_bare_list_marker(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some('+' | '*' | '-') => chars.next().is_none_or(|c| c.is_whitespace()),
        _ => false,
    }
}

/// A thematic break: a line of only `---`, `***`, or `___`, with spaces or
/// tabs allowed between and around the markers.
fn is_thematic_break(text: &str) -> bool {
    let stripped: String =
        text.chars().filter(|c| *c != ' ' && *c != '\t').collect();
    if stripped.len() < 3 {
        return false;
    }
    let mut chars = stripped.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    stripped.chars().all(|c| c == first)
        && text.chars().all(|c| c == first || c == ' ' || c == '\t')
}

/// A code fence: a leading run of `` ` `` or `~`.
fn is_code_fence(text: &str) -> bool {
    text.starts_with("```") || text.starts_with("~~~")
}

/// Collapse every whitespace run to one space and trim the ends.
fn collapse_whitespace(text: &str) -> String {
    let normalized = normalize_crlf(text);
    let mut out = String::with_capacity(normalized.len());
    let mut pending_space = false;
    for c in normalized.chars() {
        if c == '\n' || c.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
    }
    out
}

/// Render one raw sender string that may span lines: escape inline hazards
/// on every line, then line-start hazards, and rejoin with newlines.
#[cfg(test)]
fn render_text_multiline(raw: &str) -> String {
    normalize_sender(raw)
        .split('\n')
        .map(|line| escape_line_start(&escape_inline_text(line.trim_end())))
        .collect::<Vec<String>>()
        .join("\n")
}

/// Render text that sits between renderer-owned inline nodes. Unlike a
/// standalone prose line, whitespace at the edges is meaningful here:
/// `hello <strong>there</strong>` must keep its space. Emit edge whitespace as
/// a Markdown-safe HTML entity so it cannot become indentation or line-start
/// syntax, while escaping the sender's visible text as usual.
fn render_inline_text(raw: &str) -> String {
    normalize_sender(raw)
        .split('\n')
        .map(|line| {
            if line.trim().is_empty() {
                return if line.is_empty() {
                    String::new()
                } else {
                    "&#32;".to_string()
                };
            }
            let leading = line.chars().next().is_some_and(char::is_whitespace);
            let trailing =
                line.chars().next_back().is_some_and(char::is_whitespace);
            let escaped = escape_line_start(&escape_inline_text(line.trim()));
            format!(
                "{}{}{}",
                if leading { "&#32;" } else { "" },
                escaped,
                if trailing { "&#32;" } else { "" }
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Render one raw sender string confined to a single renderer-owned line:
/// collapse whitespace, then break inline hazards.
fn render_text_single(raw: &str) -> String {
    escape_inline_text(&collapse_whitespace(raw))
}

/// Merge adjacent literal [`Inline::Text`] payloads so split DOM text nodes
/// (for example `<span>key:</span><span>: value</span>` or text broken by a
/// dropped hidden element) are escaped as one string. Without this,
/// per-fragment escaping leaves single `:`/`%`/`$` runs that re-form `::`,
/// `%%`, or `$$` across the boundary, and split backslashes undo the
/// doubling pass. Structure (`Emphasis`, `Strong`, `Code`, `Link`) still
/// breaks the run; safety across those edges is enforced by
/// [`needs_edge_break`].
fn coalesce_inlines(inlines: &[Inline]) -> Vec<Inline> {
    let mut out = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Text(raw) => {
                if let Some(Inline::Text(prev)) = out.last_mut() {
                    prev.push_str(raw);
                } else {
                    out.push(Inline::Text(raw.clone()));
                }
            }
            Inline::Emphasis(children) => {
                out.push(Inline::Emphasis(coalesce_inlines(children)));
            }
            Inline::Strong(children) => {
                out.push(Inline::Strong(coalesce_inlines(children)));
            }
            Inline::Code(raw) => out.push(Inline::Code(raw.clone())),
            Inline::Link { label, url } => out.push(Inline::Link {
                label: coalesce_inlines(label),
                url: url.clone(),
            }),
        }
    }
    out
}

/// Whether joining `left` (already rendered) with `right` (next piece) would
/// create active syntax across the composition edge.
///
/// `!` immediately before a renderer-owned link forms a remote Markdown
/// image (`![...](...)`), and `[` immediately before a link forms a raw
/// `[[` wikilink span even when the left bracket is backslash-escaped. A
/// trailing backslash would escape the next structure opener and silently
/// break a trusted link, emphasis, or code span. Adjacent backticks would
/// merge inline-code delimiters. Each case inserts one invisible breaker
/// ([`ZW_NON_JOINER`]) so visible text and trusted structure survive while
/// the hazard can never form. `::`/`%%`/`$$` across two literal runs are
/// handled by [`coalesce_inlines`], not here, because structure always sits
/// between a literal run and any other literal here.
fn needs_edge_break(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let left_bang = left.ends_with('!');
    let left_bracket = left.ends_with('[');
    let left_tick = left.ends_with('`');
    let right_bracket = right.starts_with('[');
    let right_tick = right.starts_with('`');
    if (left_bang || left_bracket) && right_bracket {
        return true;
    }
    if left_tick && right_tick {
        return true;
    }
    // A single trailing backslash would escape the next opener (`\[`, `` \` ``,
    // `\*`) and either undo trusted structure or create a new escape that
    // hides the boundary. Break when the tail has an odd run of backslashes.
    let trailing = left.chars().rev().take_while(|c| *c == '\\').count();
    if trailing % 2 == 1
        && (right_bracket || right_tick || right.starts_with('*'))
    {
        return true;
    }
    false
}

fn push_piece(out: &mut String, piece: &str) {
    if needs_edge_break(out, piece) {
        out.push(ZW_NON_JOINER);
    }
    out.push_str(piece);
}

fn render_single(inline: &Inline, out: &mut String) {
    match inline {
        Inline::Text(raw) => push_piece(out, &render_inline_text(raw)),
        Inline::Emphasis(children) => {
            let mut inner = String::new();
            render_inlines(children, &mut inner);
            let mut piece = String::with_capacity(inner.len() + 2);
            piece.push('*');
            piece.push_str(&inner);
            piece.push('*');
            push_piece(out, &piece);
        }
        Inline::Strong(children) => {
            let mut inner = String::new();
            render_inlines(children, &mut inner);
            let mut piece = String::with_capacity(inner.len() + 4);
            piece.push_str("**");
            piece.push_str(&inner);
            piece.push_str("**");
            push_piece(out, &piece);
        }
        Inline::Code(raw) => {
            let mut piece = String::new();
            render_inline_code(raw, &mut piece);
            push_piece(out, &piece);
        }
        Inline::Link { label, url } => match url {
            Some(safe) => {
                let mut inner = String::new();
                render_inlines(label, &mut inner);
                let mut piece = String::with_capacity(
                    inner.len() + safe.as_str().len() + 5,
                );
                piece.push('[');
                piece.push_str(&inner);
                piece.push_str("](<");
                piece.push_str(safe.as_str());
                piece.push_str(">)");
                push_piece(out, &piece);
            }
            None => {
                let mut inner = String::new();
                render_inlines(label, &mut inner);
                push_piece(out, &inner);
            }
        },
    }
}

fn render_single_label(inline: &Inline, out: &mut String) {
    match inline {
        // Labels and other nested single-line contexts collapse internal
        // newlines instead of emitting raw line breaks.
        Inline::Text(raw) => {
            push_piece(out, &render_inline_text(&raw.replace('\n', " ")));
        }
        Inline::Emphasis(children) => {
            let mut inner = String::new();
            render_inlines(children, &mut inner);
            let mut piece = String::with_capacity(inner.len() + 2);
            piece.push('*');
            piece.push_str(&inner);
            piece.push('*');
            push_piece(out, &piece);
        }
        Inline::Strong(children) => {
            let mut inner = String::new();
            render_inlines(children, &mut inner);
            let mut piece = String::with_capacity(inner.len() + 4);
            piece.push_str("**");
            piece.push_str(&inner);
            piece.push_str("**");
            push_piece(out, &piece);
        }
        Inline::Code(raw) => {
            let mut piece = String::new();
            render_inline_code(raw, &mut piece);
            push_piece(out, &piece);
        }
        Inline::Link { label, url } => match url {
            Some(safe) => {
                let mut inner = String::new();
                render_inlines(label, &mut inner);
                let mut piece = String::with_capacity(
                    inner.len() + safe.as_str().len() + 5,
                );
                piece.push('[');
                piece.push_str(&inner);
                piece.push_str("](<");
                piece.push_str(safe.as_str());
                piece.push_str(">)");
                push_piece(out, &piece);
            }
            None => {
                let mut inner = String::new();
                render_inlines(label, &mut inner);
                push_piece(out, &inner);
            }
        },
    }
}

#[cfg(test)]
fn render_inline(inline: &Inline, out: &mut String) {
    render_single(inline, out);
}

fn render_inlines(inlines: &[Inline], out: &mut String) {
    for inline in coalesce_inlines(inlines).iter() {
        render_single_label(inline, out);
    }
}

/// Render inline code with a delimiter run longer than any backtick run in
/// the content. A trimmed leading `=`/`$=` (Dataview/Tasks inline query) is
/// broken with a zero-width non-joiner after the opening delimiter.
fn render_inline_code(raw: &str, out: &mut String) {
    let flat = normalize_crlf(raw).replace(['\n', '\t'], " ");
    let content = break_code_text(&flat);
    let run = backtick_run(&content).max(1);
    let fence = "`".repeat(run);
    out.push_str(&fence);
    if content.trim_start().starts_with('=')
        || content.trim_start().starts_with("$=")
    {
        out.push(ZW_NON_JOINER);
    }
    out.push_str(&content);
    out.push_str(&fence);
}

/// Break code hazards invisibly: `[[`, `]]`, `<%`, `<!--`, and `%%` pairs
/// are split with a zero-width non-joiner, and the `::` adjacency that
/// bracketed and bare inline fields need is broken the same way. Single
/// characters stay untouched so HTML samples and prose remain readable inside
/// code. Pair-breaking is done character by character so overlapping runs
/// (`[[[`, `%%%`) cannot reform a pair across a replacement boundary.
fn break_code_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut previous = '\0';
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        match current {
            '[' => {
                if previous == '[' {
                    out.push(ZW_NON_JOINER);
                }
                out.push('[');
            }
            ']' => {
                if previous == ']' {
                    out.push(ZW_NON_JOINER);
                }
                out.push(']');
            }
            '%' => {
                if previous == '%' {
                    out.push(ZW_NON_JOINER);
                }
                out.push('%');
            }
            '<' => {
                out.push('<');
                let is_template = chars.get(index + 1) == Some(&'%');
                let is_comment = chars.get(index + 1) == Some(&'!')
                    && chars.get(index + 2) == Some(&'-')
                    && chars.get(index + 3) == Some(&'-');
                if is_template || is_comment {
                    out.push(ZW_NON_JOINER);
                }
            }
            _ => out.push(current),
        }
        previous = current;
        index += 1;
    }
    let staged = break_colon_adjacency(&out);
    break_task_markers_in_code(&staged)
}

/// Break task-shaped lines inside code content: the vault's task scanner
/// reads every line including fenced code, so a `- [ ]` (or `*`/`+`/ordered)
/// marker followed by `[` gets a zero-width non-joiner before the bracket.
/// Invisible in render, and inline code is unaffected in practice because a
/// code span never starts a task line.
fn break_task_markers_in_code(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let indent_len =
                line.len() - line.trim_start_matches([' ', '\t']).len();
            let (indent, rest) = line.split_at(indent_len);
            let marker_len = if rest.starts_with(['-', '*', '+']) {
                rest[1..].chars().next().map_or(0, |c| {
                    if c == ' ' || c == '\t' {
                        1
                    } else {
                        0
                    }
                })
            } else {
                let digits: String =
                    rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.is_empty() {
                    0
                } else {
                    let after = &rest[digits.len()..];
                    let mut chars = after.chars();
                    match chars.next() {
                        Some('.') | Some(')') => chars.next().map_or(0, |c| {
                            if c == ' ' || c == '\t' {
                                digits.len() + 2
                            } else {
                                0
                            }
                        }),
                        _ => 0,
                    }
                }
            };
            if marker_len == 0 {
                return line.to_string();
            }
            let after_marker = &rest[marker_len..];
            let gap = after_marker.len()
                - after_marker.trim_start_matches([' ', '\t']).len();
            if after_marker[gap..].starts_with('[') {
                format!(
                    "{indent}{}{ZW_NON_JOINER}{}",
                    &rest[..marker_len + gap],
                    &after_marker[gap..]
                )
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Break a trailing ` ^id` block id on any code line with a zero-width
/// non-joiner after the caret. The vault's block-id scanner reads every line
/// including fenced code, so the caret must no longer follow whitespace.
fn break_trailing_carets(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let trimmed = line.trim_end();
            let Some(caret) = trimmed.rfind('^') else {
                return line.to_string();
            };
            let id = &trimmed[caret + 1..];
            let preceded = caret == 0
                || trimmed[..caret]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace);
            if !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && preceded
            {
                format!(
                    "{}{ZW_NON_JOINER}{}{}",
                    &trimmed[..caret + 1],
                    id,
                    &line[trimmed.len()..]
                )
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Insert a zero-width non-joiner between adjacent colons so `::` field
/// separators cannot form inside code content.
fn break_colon_adjacency(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_colon = false;
    for c in text.chars() {
        if c == ':' {
            if previous_colon {
                out.push(ZW_NON_JOINER);
            }
            out.push(':');
            previous_colon = true;
        } else {
            out.push(c);
            previous_colon = false;
        }
    }
    out
}

/// Length of the longest backtick run in the content, plus one for safety.
/// The caller takes the max with one for empty content.
fn backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest + 1
}

fn render_block(block: &Block, depth: usize, out: &mut String) {
    match block {
        Block::Paragraph(inlines) => {
            let mut body = String::new();
            for inline in coalesce_inlines(inlines).iter() {
                render_single(inline, &mut body);
            }
            out.push_str(&body);
            out.push_str("\n\n");
        }
        Block::Heading { level, text } => {
            let level = (*level).clamp(1, 6);
            out.push_str(&"#".repeat(level as usize));
            out.push(' ');
            out.push_str(&render_text_single(text));
            out.push_str("\n\n");
        }
        Block::List {
            ordered,
            start,
            items,
        } => {
            render_list(*ordered, *start, items, depth, out);
            out.push('\n');
        }
        Block::Code(content) => {
            render_code_block(content, out);
        }
        Block::Table { headers, rows } => {
            render_table(headers, rows, out);
        }
    }
}

fn render_list(
    ordered: bool,
    start: u32,
    items: &[Vec<Block>],
    depth: usize,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    for (index, item) in items.iter().enumerate() {
        let marker = if ordered {
            format!("{}.", start.saturating_add(index as u32))
        } else {
            "-".to_string()
        };
        let mut first = true;
        for block in item {
            let mut rendered = String::new();
            render_block(block, depth + 1, &mut rendered);
            for line in rendered.trim_end_matches('\n').split('\n') {
                if first {
                    first = false;
                    out.push_str(&indent);
                    out.push_str(&marker);
                    out.push(' ');
                    out.push_str(line.trim_start());
                } else {
                    out.push_str(&indent);
                    out.push_str("  ");
                    out.push_str(line.trim_start());
                }
                out.push('\n');
            }
        }
        if first {
            out.push_str(&indent);
            out.push_str(&marker);
            out.push('\n');
        }
    }
}

/// Fenced code with exactly the `text` info string and a delimiter sized past
/// every backtick run in the content, so sender fences can never close the
/// block early and the info string can never execute.
fn render_code_block(content: &str, out: &mut String) {
    let broken = break_code_text(&normalize_sender(content));
    let broken = break_trailing_carets(&broken);
    let width = backtick_run(&broken).max(3);
    let fence = "`".repeat(width);
    out.push_str(&fence);
    out.push_str("text\n");
    out.push_str(&broken);
    if !broken.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&fence);
    out.push_str("\n\n");
}

/// Render one table cell's structured inlines to a single-line Markdown
/// fragment. Links, emphasis, and code survive as renderer-owned structure;
/// pipes and newlines are neutralized so sender pipes can never split a
/// renderer-owned row. Final escaping stays here via [`render_inlines`].
fn render_table_cell_inlines(inlines: &[Inline]) -> String {
    let mut buf = String::new();
    render_inlines(inlines, &mut buf);
    buf.replace('|', "\\|").replace('\n', " ")
}

fn render_table(
    headers: &[Vec<Inline>],
    rows: &[Vec<Vec<Inline>>],
    out: &mut String,
) {
    if headers.is_empty() {
        for row in rows {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| render_table_cell_inlines(cell))
                .collect();
            // Preserve all readable cells; skip fully empty rows so no
            // empty paragraph is emitted from headerless data.
            if cells.iter().all(|cell| cell.trim().is_empty()) {
                continue;
            }
            out.push_str(&cells.join("; "));
            out.push_str("\n\n");
        }
        return;
    }
    let rendered_headers: Vec<String> = headers
        .iter()
        .map(|cell| render_table_cell_inlines(cell))
        .collect();
    // Never truncate extra cells to the first header row's width: widen the
    // table to the widest row so every readable cell survives with its
    // boundary intact.
    let width = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let mut padded_headers = rendered_headers;
    padded_headers.resize_with(width, String::new);
    out.push_str("| ");
    out.push_str(&padded_headers.join(" | "));
    out.push_str(" |\n| ");
    out.push_str(&vec!["---"; width].join(" | "));
    out.push_str(" |\n");
    for row in rows {
        let mut cells: Vec<String> = row
            .iter()
            .map(|cell| render_table_cell_inlines(cell))
            .collect();
        cells.resize_with(width, String::new);
        out.push_str("| ");
        out.push_str(&cells.join(" | "));
        out.push_str(" |\n");
    }
    out.push('\n');
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::collect_done::block_ids_in_markdown;
    use crate::native::task_dependencies::raw_wikilink_spans;
    use crate::native::task_fields::all_inline_fields;
    use crate::native::task_status_hooks::{
        parse_tasks, TaskStatusType, TasksSettings,
    };
    use std::collections::{BTreeMap, BTreeSet};

    /// Minimal task settings for scanner assertions: default vault checkbox
    /// semantics without any note I/O.
    fn scanner_settings() -> TasksSettings {
        TasksSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
                ('X', TaskStatusType::Done),
                ('/', TaskStatusType::InProgress),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        }
    }

    /// Sender fragments that must never survive as vault syntax.
    fn hostile_fragments() -> Vec<&'static str> {
        vec![
            "[[link]]",
            "[[target|alias]]",
            "![[embed]]",
            "![alt](https://example.test/i.png)",
            "[key:: value]",
            "(key:: value)",
            "status:: done",
            "#tag",
            "#task",
            "#area/sub",
            "[#tag]",
            "(#task)",
            "- [ ] open task",
            "- [x] closed task",
            "* [ ] star task",
            "1. [ ] ordered task",
            "> - [ ] quoted task",
            "  - [ ] indented task",
            "trailing ^block-id",
            "trailing ^abc",
            "^lonely",
            "%%comment%%",
            "$x^2$",
            "$$display$$",
            "$5-$10",
            "```dataview\nTABLE file FROM #x```",
            "```",
            "````",
            "~~~",
            "<b>bold</b>",
            "<script>alert(1)</script>",
            "<!-- comment -->",
            "<!-- bob:gmail:begin v1 -->",
            "<%@ taglib %>",
            "<% tp.file.title %>",
            "<%= view %>",
            "<%* code %>",
            "<%# comment %>",
            "[label](javascript:alert(1))",
            "[label](data:text/html,hi)",
            "[label](file:///etc/passwd)",
            "[label](obsidian://open?vault=x)",
            "= 1+1",
            "$= 2+2",
            "# Heading attempt",
            "## h2 attempt",
            "> quote attempt",
            "| table | attempt |",
            "---",
            "***",
            "___",
            "- list item",
            "+ plus item",
            "* star item",
            "1. ordered",
            "2) paren ordered",
            "    indented code",
            "🍅 25m",
            "`inline code [key:: v]`",
            "C# code",
            "Q4: $5",
            "escaped \\[bracket\\]",
            "&lt;already entity&gt;",
            "java\tscript:alert(1)",
            "https://example.test/a)b",
            "mailto:someone@example.test?subject=hi",
        ]
    }

    /// Independent observation: Bob's own scanners must find nothing
    /// actionable in `doc`. `allowed_wikilinks` exempts trusted wrapper
    /// syntax (for example a mail-note alias span) by exact span text.
    fn assert_scanners_clean(doc: &str, allowed_wikilinks: &[&str]) {
        let settings = scanner_settings();
        let tasks = parse_tasks(doc, &settings);
        assert!(
            tasks.is_empty(),
            "parse_tasks found tasks in inert output:\n{doc}\n{tasks:?}"
        );
        for line in doc.split('\n') {
            let fields = all_inline_fields(line);
            assert!(
                fields.is_empty(),
                "inline field in inert output line {line:?} of:\n{doc}\n{fields:?}"
            );
            let spans = raw_wikilink_spans(line);
            let unexpected: Vec<_> = spans
                .iter()
                .filter_map(|span| line.get(span.open..span.end))
                .filter(|text| !allowed_wikilinks.contains(text))
                .collect();
            assert!(
                unexpected.is_empty(),
                "wikilink {unexpected:?} in inert output line {line:?} of:\n{doc}"
            );
        }
        let block_ids = block_ids_in_markdown(doc);
        assert!(
            block_ids.is_empty(),
            "block ids {block_ids:?} in inert output:\n{doc}"
        );
    }

    fn assert_no_active_syntax(doc: &str) {
        // Lone `]]` closers are asserted only through the scanners: no listed
        // Bob scanner acts on `]]` without its literal `[[` opener, which never
        // survives here.
        let mut fence_width: Option<usize> = None;
        for line in doc.split('\n') {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                let width = trimmed.chars().take_while(|c| *c == '`').count();
                let rest = trimmed[width..].trim();
                if let Some(open) = fence_width {
                    // A backtick-only run at least as long closes the fence.
                    if rest.is_empty() && width >= open {
                        fence_width = None;
                    }
                    continue;
                }
                assert!(
                    rest.is_empty() || rest == "text",
                    "non-text fence {line:?} in inert output:\n{doc}"
                );
                fence_width = Some(width);
                continue;
            }
            if fence_width.is_some() {
                // Fenced content is literal text: inner ```dataview lines and
                // sender punctuation cannot open structure from in here.
                continue;
            }
            for forbidden in ["<%", "<!--", "![", "[["] {
                assert!(
                    !line.contains(forbidden),
                    "forbidden {forbidden:?} in inert output line {line:?} of:\n{doc}"
                );
            }
        }
    }

    /// Every ASCII letter run of length 3+ in `fragment` must survive in
    /// `output`: escaping breaks delimiters but preserves readable text.
    fn assert_readable(fragment: &str, output: &str) {
        let mut run = String::new();
        let mut runs = Vec::new();
        for c in fragment.chars() {
            if c.is_ascii_alphabetic() {
                run.push(c);
            } else if run.len() >= 3 {
                runs.push(std::mem::take(&mut run));
            } else {
                run.clear();
            }
        }
        if run.len() >= 3 {
            runs.push(run);
        }
        for run in runs {
            assert!(
                output.contains(&run),
                "readable text {run:?} from {fragment:?} lost in:\n{output}"
            );
        }
    }

    #[test]
    fn helpers_neutralize_hostile_fragments() {
        for fragment in hostile_fragments() {
            let single = inert_single_line(fragment);
            assert_scanners_clean(&single, &[]);
            assert_no_active_syntax(&single);
            assert_readable(fragment, &single);

            let multi = inert_multiline(fragment);
            assert_scanners_clean(&multi, &[]);
            assert_no_active_syntax(&multi);
            assert_readable(fragment, &multi);

            let label = inert_label(fragment);
            assert_scanners_clean(&label, &[]);
            assert_no_active_syntax(&label);
            assert_readable(fragment, &label);

            let yaml = inert_yaml_value(fragment);
            assert_scanners_clean(&yaml, &[]);
            assert_no_active_syntax(&yaml);
            assert_readable(fragment, &yaml);

            let blocks = render_blocks(&prose_blocks(fragment));
            assert_scanners_clean(&blocks, &[]);
            assert_no_active_syntax(&blocks);
            assert_readable(fragment, &blocks);
        }
    }

    #[test]
    fn link_policy_allows_only_http_mailto() {
        let accepted = [
            "https://example.test/a?b=c&d=e#f",
            "http://example.test/path with space",
            "HTTP://EXAMPLE.TEST/upper",
            "https://example.test/a(b)",
            "mailto:someone@example.test",
            "mailto:someone@example.test?subject=hi",
        ];
        for url in accepted {
            let safe = SafeUrl::parse(url);
            assert!(safe.is_some(), "rejected usable URL {url:?}");
            let safe = safe.unwrap();
            assert!(
                !safe.as_str().chars().any(|c| c.is_control()
                    || c == ' '
                    || c == '<'
                    || c == '>'
                    || c == '"'),
                "unsafe destination {safe:?} from {url:?}"
            );
            let rendered =
                render_blocks(&[Block::Paragraph(vec![Inline::Link {
                    label: vec![Inline::Text("Read [[more]]".to_string())],
                    url: Some(safe),
                }])]);
            assert_scanners_clean(&rendered, &[]);
            assert_no_active_syntax(&rendered);
            assert!(rendered.contains("Read"), "label lost:\n{rendered}");
        }
        let rejected = [
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "obsidian://open?vault=x",
            "ftp://example.test/x",
            "//example.test/protocol-relative",
            "/relative/path",
            "relative/path",
            "#fragment-only",
            "java\tscript:alert(1)",
            "java\nscript:alert(1)",
            "",
            "mailto:",
            "https://exa mple.test/space",
            "https://example.test/a<b",
        ];
        for url in rejected {
            assert!(
                SafeUrl::parse(url).is_none(),
                "accepted dangerous URL {url:?}"
            );
            let rendered =
                render_blocks(&[Block::Paragraph(vec![Inline::Link {
                    label: vec![Inline::Text("keep this label".to_string())],
                    url: SafeUrl::parse(url),
                }])]);
            assert_scanners_clean(&rendered, &[]);
            assert!(
                rendered.contains("keep this label"),
                "invalid link lost readable text for {url:?}:\n{rendered}"
            );
            assert!(
                !rendered.contains('(') || !rendered.contains("]("),
                "invalid link kept link syntax for {url:?}:\n{rendered}"
            );
        }
    }

    #[test]
    fn fenced_code_keeps_text_info_and_sizes_delimiters() {
        let content = "sender fence\n```dataview\nTABLE file\n```\n\
             trailing [[link]] and ^block plus <%tpl%> and %%c%%";
        let rendered = render_blocks(&[Block::Code(content.to_string())]);
        let mut lines = rendered.trim_end().lines();
        let opening = lines.next().unwrap();
        assert!(
            opening == "```text"
                || (opening.starts_with("````") && opening.ends_with("text")),
            "unexpected fence {opening:?} in:\n{rendered}"
        );
        let closing = lines.next_back().unwrap();
        assert!(
            closing.len() >= 3 && closing.chars().all(|c| c == '`'),
            "unexpected closing fence {closing:?} in:\n{rendered}"
        );
        assert_no_active_syntax(&rendered);
        assert_scanners_clean(&rendered, &[]);
        assert_readable(
            "sender fence dataview TABLE file trailing link block tpl",
            &rendered,
        );
    }

    #[test]
    fn inline_queries_are_broken() {
        for query in ["= 1+1", "=link", "$= 2+2", "  = spaced"] {
            let mut out = String::new();
            render_inline_code(query, &mut out);
            assert!(
                !out.contains("`=") && !out.contains("`$= "),
                "query opener survived in {out:?}"
            );
            assert!(out.contains(ZW_NON_JOINER), "no breaker in {out:?}");
            let doc = format!("before {out} after");
            assert_scanners_clean(&doc, &[]);
        }
    }

    #[test]
    fn remote_images_never_become_image_syntax() {
        let linked = remote_image_link("logo", "https://example.test/i.png");
        let mut out = String::new();
        render_inline(&linked, &mut out);
        assert!(!out.contains('!'), "image syntax in {out:?}");
        assert!(out.contains("logo"), "alt text lost in {out:?}");
        assert_scanners_clean(&out, &[]);

        let denied = remote_image_link("pic", "javascript:alert(1)");
        let mut out = String::new();
        render_inline(&denied, &mut out);
        assert!(!out.contains('!'), "image syntax in {out:?}");
        assert!(!out.contains("]("), "link syntax in {out:?}");
        assert!(out.contains("pic"), "alt text lost in {out:?}");
    }

    #[test]
    fn structured_inline_text_keeps_spaces_around_renderer_formatting() {
        let rendered = render_blocks(&[Block::Paragraph(vec![
            Inline::Text("Hello ".to_string()),
            Inline::Strong(vec![Inline::Text("friend".to_string())]),
            Inline::Text(" world".to_string()),
            Inline::Strong(vec![
                Inline::Text("very ".to_string()),
                Inline::Emphasis(vec![Inline::Text("nice".to_string())]),
            ]),
        ])]);
        assert_eq!(
            rendered,
            "Hello&#32;**friend**&#32;world**very&#32;*nice***\n\n"
        );
        assert_scanners_clean(&rendered, &[]);
    }

    #[test]
    fn verified_embed_accepts_only_writer_paths() {
        assert_eq!(
            verified_local_image_embed("mail/a1b2.png", "logo <b>").as_deref(),
            Some("![logo &lt;b>](mail/a1b2.png)")
        );
        for bad in [
            "https://example.test/i.png",
            "cid:logo",
            "/absolute.png",
            "../escape.png",
            "a..b/../../x.png",
            "with space.png",
            "semi;colon.png",
            "back\\slash.png",
            "par(en.png",
            "br[ack.png",
            "quo\"te.png",
            "lt<gt.png",
            "pipe|.png",
            "star*.png",
            "tick`.png",
            "",
            "a:b.png",
            "dir//double.png",
        ] {
            assert!(
                verified_local_image_embed(bad, "alt").is_none(),
                "accepted sender path {bad:?}"
            );
        }
    }

    #[test]
    fn verified_wikilink_mints_exact_resized_embed_for_writer_paths_only() {
        assert_eq!(
            verified_local_image_wikilink("img/mail/aabbcc/logo.png")
                .as_deref(),
            Some("![[img/mail/aabbcc/logo.png|400]]")
        );
        for bad in [
            "https://example.test/i.png",
            "cid:logo",
            "/absolute.png",
            "../escape.png",
            "a..b/../../x.png",
            "with space.png",
            "semi;colon.png",
            "back\\slash.png",
            "par(en.png",
            "br[ack.png",
            "quo\"te.png",
            "lt<gt.png",
            "pipe|.png",
            "star*.png",
            "tick`.png",
            "",
            "a:b.png",
            "dir//double.png",
            "img/mail/aabbcc/evil|.png",
        ] {
            assert!(
                verified_local_image_wikilink(bad).is_none(),
                "accepted sender path {bad:?}"
            );
        }
    }

    #[test]
    fn yaml_values_survive_serialization_without_active_syntax() {
        for fragment in hostile_fragments() {
            let value = inert_yaml_value(fragment);
            let mut mapping = serde_yaml::Mapping::new();
            mapping.insert(
                serde_yaml::Value::from("subject"),
                serde_yaml::Value::from(value.clone()),
            );
            let doc = serde_yaml::to_string(&mapping).unwrap();
            assert_scanners_clean(&doc, &[]);
            for forbidden in ["[[", "<%", "<!--", "![", "]]"] {
                assert!(
                    !doc.contains(forbidden),
                    "forbidden {forbidden:?} from {fragment:?} in YAML:\n{doc}"
                );
            }
            let decoded: serde_yaml::Mapping =
                serde_yaml::from_str(&doc).unwrap();
            let round_tripped =
                decoded.get("subject").unwrap().as_str().unwrap();
            assert_eq!(
                round_tripped, value,
                "YAML round trip changed the value for {fragment:?}"
            );
            assert_readable(fragment, round_tripped);
        }
    }

    #[test]
    fn simulated_card_holds_no_sender_structure() {
        let subject = "Re: [[plans]] <%todo%> <!--x--> #update $5-$10";
        let from = "Evil <evil@example.test> [key:: v]";
        let mut mapping = serde_yaml::Mapping::new();
        mapping.insert(
            serde_yaml::Value::from("subject"),
            serde_yaml::Value::from(inert_yaml_value(subject)),
        );
        mapping.insert(
            serde_yaml::Value::from("from"),
            serde_yaml::Value::from(inert_yaml_value(from)),
        );
        let frontmatter = serde_yaml::to_string(&mapping).unwrap();

        // Trusted wrapper syntax, owned by mail-note, never by the sender.
        // Bracketed sender labels are covered at the helper level; here the
        // label is plain so the wrapper span matches exactly.
        let alias_line = "[[2026-11-05|Quarterly review]]";
        let doc = format!(
            "---\n{frontmatter}---\n# {heading}\n\n{table}\n\n{paras}\n\n> {quoted}\n\n{alias}\n",
            heading = inert_single_line(subject),
            table = render_blocks(&[Block::Table {
                headers: vec![
                    vec![Inline::Text("From".to_string())],
                    vec![Inline::Text("Subject".to_string())],
                ],
                rows: vec![vec![
                    vec![Inline::Text(from.to_string())],
                    vec![Inline::Text(subject.to_string())],
                ]],
            }])
            .trim(),
            paras = render_blocks(&prose_blocks(
                "- [ ] get this done\n\nSee [[notes]] and #task now ^abc"
            ))
            .trim(),
            quoted = inert_multiline("- [ ] quoted task\n#tag inside quote"),
            alias = alias_line,
        );
        assert_scanners_clean(&doc, &[alias_line]);
        // The trusted alias wrapper is excluded from the sender assertion.
        assert_no_active_syntax(&doc.replace(alias_line, ""));
        assert!(doc.contains("Quarterly"), "trusted alias text lost");
        assert!(doc.contains("review"), "sender label text lost");
        assert!(doc.contains("get this done"), "sender prose lost");
    }

    /// Deterministic pseudo-random generator (xorshift64* with a fixed seed):
    /// no new dev-dependencies, reproducible adversarial coverage.
    struct XorShift64 {
        state: u64,
    }

    impl XorShift64 {
        fn next(&mut self) -> u64 {
            let mut x = self.state;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.state = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    #[test]
    fn adversarial_corpus_stays_inert_and_readable() {
        let alphabet = [
            "[[",
            "]]",
            "![[",
            "[",
            "]",
            "(",
            ")",
            "#",
            "^",
            "%",
            "$",
            "`",
            "<",
            ">",
            "!",
            "=",
            ":",
            ";",
            "-",
            "*",
            "+",
            "|",
            "~",
            "\\",
            "&",
            "key:: value",
            "- [ ]",
            "<%",
            "<!--",
            "bob:gmail",
            "needle42",
            "hello",
            "wörld",
            "日本語",
            "🎉",
            "\u{a0}",
            "\u{200b}",
            "\u{200c}",
            "\u{2028}",
            "\u{0}",
            "\u{7}",
            "\r\n",
            "\t",
            " ",
            "https://",
            "javascript:",
            "mailto:",
            "\n\n",
            "```",
            "---",
        ];
        let mut rng = XorShift64 {
            state: 0x1234_5678_9abc_def1,
        };
        for _ in 0..2000 {
            let pieces = rng.below(8) + 1;
            let mut input = String::new();
            for _ in 0..pieces {
                input.push_str(alphabet[rng.below(alphabet.len())]);
            }
            // Every generated case carries a readable marker.
            input.push_str(" needle42 ");

            let single = inert_single_line(&input);
            assert_scanners_clean(&single, &[]);
            assert_no_active_syntax(&single);
            assert!(single.contains("needle42"), "marker lost in {single:?}");

            let multi = inert_multiline(&input);
            assert_scanners_clean(&multi, &[]);
            assert_no_active_syntax(&multi);
            assert!(multi.contains("needle42"), "marker lost in {multi:?}");

            let blocks = render_blocks(&prose_blocks(&input));
            assert_scanners_clean(&blocks, &[]);
            assert_no_active_syntax(&blocks);
            assert!(blocks.contains("needle42"), "marker lost in {blocks:?}");

            let code = render_blocks(&[Block::Code(input.clone())]);
            assert_scanners_clean(&code, &[]);
            for forbidden in ["[[", "]]", "<%", "<!--", "%%"] {
                assert!(
                    !code.contains(forbidden),
                    "forbidden {forbidden:?} in code for {input:?}:\n{code}"
                );
            }
            assert!(
                code.contains("needle42"),
                "marker lost in code for {input:?}"
            );

            // Determinism: rendering twice gives the same bytes.
            assert_eq!(blocks, render_blocks(&prose_blocks(&input)));
        }
    }

    #[test]
    fn readable_text_survives_untouched() {
        let cases = [
            ("Quarterly report $5 of $10", "Quarterly report $5 of $10"),
            ("C# guide", "C# guide"),
            ("meet at 3:30", "meet at 3:30"),
            ("100% done", "100% done"),
            ("wörld 日本語 🎉", "wörld 日本語 🎉"),
            ("a\rb\r\nc", "a\nb\nc"),
        ];
        for (input, _) in cases {
            let out = inert_multiline(input);
            assert_scanners_clean(&out, &[]);
        }
        assert_eq!(inert_multiline("a\rb\r\nc"), "a\nb\nc");
        assert_eq!(inert_single_line("  spaced   out  "), "spaced out");
        assert_eq!(inert_single_line("C# guide"), "C# guide");
        assert_eq!(inert_single_line("Q4: $5"), "Q4: $5");
    }

    #[test]
    fn bang_before_trusted_link_never_forms_remote_image() {
        // `!` immediately before a renderer-owned link would form `![...](...)`.
        let rendered = render_blocks(&[Block::Paragraph(vec![
            Inline::Text("!".to_string()),
            Inline::Link {
                label: vec![Inline::Text("load".to_string())],
                url: SafeUrl::parse("https://example.test/tracker.png"),
            },
        ])]);
        assert!(
            !rendered.contains("!["),
            "remote image formed in:\n{rendered}"
        );
        assert!(rendered.contains('!'), "visible bang lost in:\n{rendered}");
        assert!(rendered.contains("load"), "label lost in:\n{rendered}");
        assert_scanners_clean(&rendered, &[]);
        assert_no_active_syntax(&rendered);
    }

    #[test]
    fn split_colon_runs_cannot_reform_bare_or_bracketed_fields() {
        // `<span>key:</span><span>: value</span>` arrives as two adjacent
        // literal runs; coalescing must escape the reformed `::` for both the
        // bracketed scanner and a bare `key:: value` observer.
        let rendered = render_blocks(&[Block::Paragraph(vec![
            Inline::Text("key:".to_string()),
            Inline::Text(": value".to_string()),
        ])]);
        assert!(
            !rendered.contains("::"),
            "split field reformed in:\n{rendered}"
        );
        assert!(rendered.contains("key"), "readable text lost:\n{rendered}");
        assert!(
            rendered.contains("value"),
            "readable text lost:\n{rendered}"
        );
        assert_scanners_clean(&rendered, &[]);
        // Bare Dataview observer: `key:: value` with a space must not survive.
        assert!(
            !rendered.contains("key:: value") && !rendered.contains("key ::"),
            "bare field survived in:\n{rendered}"
        );
    }

    #[test]
    fn split_percent_math_and_backslash_edges_stay_inert() {
        for (left, right, forbidden) in [
            ("%", "%", "%%"),
            ("$", "$", "$$"),
            ("\\", "[", "\\["),
            ("`", "`", "``"),
        ] {
            let rendered = render_blocks(&[Block::Paragraph(vec![
                Inline::Text(left.to_string()),
                Inline::Text(right.to_string()),
            ])]);
            // `\\[` still contains a backslash escape, not a bare link opener;
            // the assertion is that no raw active pair survives unbroken.
            if forbidden == "\\[" {
                assert_scanners_clean(&rendered, &[]);
                continue;
            }
            assert!(
                !rendered.contains(forbidden),
                "split {forbidden:?} reformed from {left:?}+{right:?} in:\n{rendered}"
            );
            assert_scanners_clean(&rendered, &[]);
        }
        // Backslash before a trusted link opener must not escape the link.
        let rendered = render_blocks(&[Block::Paragraph(vec![
            Inline::Text("\\".to_string()),
            Inline::Link {
                label: vec![Inline::Text("go".to_string())],
                url: SafeUrl::parse("https://example.test/a"),
            },
        ])]);
        assert!(
            rendered.contains("](<https://example.test/a>)"),
            "trusted link broken in:\n{rendered}"
        );
        assert_scanners_clean(&rendered, &[]);
    }

    #[test]
    fn hidden_node_boundaries_cannot_rejoin_hazards() {
        // Text broken by a dropped hidden element (`key:` + hidden + `: v`)
        // coalesces after skipping, so the reformed `::` is still escaped.
        let rendered = render_blocks(&[Block::Paragraph(vec![
            Inline::Text("key:".to_string()),
            Inline::Text(": v".to_string()),
        ])]);
        assert!(
            !rendered.contains("::"),
            "hidden split reformed:\n{rendered}"
        );
        assert_scanners_clean(&rendered, &[]);
    }

    #[test]
    fn safe_url_refuses_wikilinks_but_keeps_usable_urls() {
        assert!(
            SafeUrl::parse("https://example.test/[[forged]]").is_none(),
            "forged wikilink destination accepted"
        );
        assert!(
            SafeUrl::parse("https://example.test/a]]b").is_none()
                || SafeUrl::parse("https://example.test/a]]b")
                    .is_none_or(|safe| !safe.as_str().contains("]]")),
            "raw wikilink survived in destination"
        );
        for usable in [
            "https://example.test/a?b=c&d=e#f",
            "http://example.test/path with space",
            "mailto:someone@example.test?subject=hi",
            "http://[::1]/",
            "https://example.test/a(b)",
        ] {
            assert!(
                SafeUrl::parse(usable).is_some(),
                "usable URL refused: {usable:?}"
            );
        }
        // Ordinary query/fragment/mailto/IPv6 destinations never contain the
        // refused pair, so they stay usable and scanner-clean as links.
        let rendered = render_blocks(&[Block::Paragraph(vec![Inline::Link {
            label: vec![Inline::Text("open".to_string())],
            url: SafeUrl::parse("https://example.test/a?b=c&d=e#f"),
        }])]);
        assert_scanners_clean(&rendered, &[]);
    }
}
