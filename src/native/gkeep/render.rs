//! Keep note → Markdown rendering (pure).
//!
//! One Keep note becomes one top-level Obsidian task. Keep text is data:
//! it is never markup and never capture grammar, so everything from the
//! note is normalized and escaped before it reaches the vault. See the
//! epic plan's Rendering section for the full contract.

use std::sync::LazyLock;

use regex::Regex;

use super::ledger::format_marker;
use super::model::{AttachmentKind, KeepNote};
use crate::native::capture;

/// One rendered Keep note as a vault task block.
///
/// `markdown` holds the task line and its children joined with `\n` and
/// has no trailing newline; the caller (`pull`) joins blocks.
pub(super) struct RenderedBlock {
    pub(super) markdown: String,
}

/// Render `note` as a vault task block.
///
/// `indent` is the target note's indent unit (one tab or two spaces) and
/// prefixes every child; grandchildren use it twice. When `revision` is
/// set the task description carries a `· revised` annotation.
pub(super) fn render_note(
    note: &KeepNote,
    indent: &str,
    revision: bool,
) -> RenderedBlock {
    render_note_in(note, indent, revision, None, &chrono::Local)
}

/// Render `note` with one extra visible child (already escaped): the
/// permanent clip-failure fallback.
pub(super) fn render_note_with_fallback(
    note: &KeepNote,
    indent: &str,
    revision: bool,
    fallback: &str,
) -> RenderedBlock {
    render_note_in(note, indent, revision, Some(fallback), &chrono::Local)
}

fn render_note_in<Tz: chrono::TimeZone>(
    note: &KeepNote,
    indent: &str,
    revision: bool,
    extra_last_child: Option<&str>,
    tz: &Tz,
) -> RenderedBlock
where
    Tz::Offset: std::fmt::Display,
{
    let (title_raw, consumed_first_line) = raw_title(note);
    let mut task_text = escape_task_text(&title_raw);
    if revision {
        task_text.push_str(" · revised");
    }
    if let Some(url) = note.url.as_deref().filter(|url| !url.trim().is_empty())
    {
        task_text.push_str(&format!(
            " [💡]({} \"Open in Google Keep\")",
            encode_source_url(url),
        ));
    }
    let created = created_date_in(note, tz);
    let task_line = capture::format_task_line(&task_text, &created, None, None);

    let child_prefix = indent.to_string();
    let grandchild_prefix = format!("{indent}{indent}");
    let mut lines = vec![task_line];

    for child in text_children(note, consumed_first_line) {
        lines.push(format!("{child_prefix}- {child}"));
    }
    for (marker, body) in list_children(note) {
        let level = if marker {
            &grandchild_prefix
        } else {
            &child_prefix
        };
        lines.push(format!("{level}- {body}"));
    }
    if !note.attachments.is_empty() {
        lines.push(format!("{child_prefix}- {}", attachment_summary(note)));
        for ocr in ocr_children(note) {
            lines.push(format!("{grandchild_prefix}- {ocr}"));
        }
    }
    if !note.labels.is_empty() {
        let labels = note
            .labels
            .iter()
            .map(|label| escape_task_text(&normalize_body(label)))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("{child_prefix}- 🏷 {labels}"));
    }
    if let Some(extra) = extra_last_child {
        lines.push(format!("{child_prefix}- {extra}"));
    }
    lines.push(format!(
        "{child_prefix}{}",
        format_marker(&note.id, &note.content.fingerprint())
    ));

    RenderedBlock {
        markdown: lines.join("\n"),
    }
}

/// The same title derivation as [`render_note`], unescaped, for tables.
pub(super) fn display_title(note: &KeepNote) -> String {
    raw_title(note).0
}

/// Escape free text for the task line: it sits mid-line, so only the
/// token, comment, field, and block-id hazards apply.
pub(super) fn escape_task_text(text: &str) -> String {
    let staged = text.replace("%%", "%&#37;");
    let staged = escape_colon_runs(&staged);
    let staged = escape_hash_task_token(&staged);
    escape_trailing_block_id(&staged)
}

/// Escape every colon in any run of two or more colons
/// (`:::` → `\:\:\:`). A non-overlapping replace leaves `::` in
/// odd-length runs, so runs are scanned manually.
fn escape_colon_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ':' {
            let mut j = i;
            while j < chars.len() && chars[j] == ':' {
                j += 1;
            }
            if j - i >= 2 {
                for _ in i..j {
                    out.push_str("\\:");
                }
            } else {
                out.push(':');
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Escape free text for a child bullet: the task-line escaping plus the
/// leading-marker rule, since a child starts a fresh list item.
pub(crate) fn escape_child_text(text: &str) -> String {
    escape_leading_marker(&escape_task_text(text))
}

static LEADING_MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(#{1,6}(\s|$)|>|\d+[.)](\s|$)|\||[+*-](\s|$))")
        .expect("valid leading-marker regex")
});

/// `\#task` for every whitespace-delimited `#task` token.
///
/// The Tasks global filter matches per whitespace token, so only exact
/// tokens are phantom tasks; `#tasks` and `(#task)` stay literal. A
/// manual scan (not a regex) keeps adjacent `#task #task` tokens working,
/// since the crate has no look-ahead.
fn escape_hash_task_token(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("#task") {
        let (before, after) = rest.split_at(index);
        let after_tag = &after["#task".len()..];
        let boundary =
            |side: Option<char>| side.is_none_or(|c| c.is_whitespace());
        escaped.push_str(before);
        if boundary(before.chars().next_back())
            && boundary(after_tag.chars().next())
        {
            escaped.push_str("\\#task");
        } else {
            escaped.push_str("#task");
        }
        rest = after_tag;
    }
    escaped.push_str(rest);
    escaped
}

/// `\^id` when the text ends with a trailing ` ^id` block id.
///
/// Mirrors `collect_done::trailing_block_id_in_line`: the caret must
/// start the text or follow any Unicode whitespace (NBSP included) and
/// be followed only by block-id bytes.
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
    let trailing_ws = &text[trimmed.len()..];
    format!("{}\\^{id}{trailing_ws}", &trimmed[..caret])
}

/// Backslash-escape a child-leading heading, quote, list, table
/// marker, thematic break, or code fence.
fn escape_leading_marker(text: &str) -> String {
    if LEADING_MARKER_RE.is_match(text)
        || is_thematic_break(text)
        || is_code_fence(text)
    {
        format!("\\{text}")
    } else {
        text.to_string()
    }
}

/// A thematic break: a line that is only `---`, `***`, or `___`,
/// with spaces allowed between and around the markers.
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

/// A code fence: a leading ` ``` ` or `~~~`.
fn is_code_fence(text: &str) -> bool {
    text.starts_with("```") || text.starts_with("~~~")
}

/// Drop `\r`, turn tabs into a space, remove zero-width characters,
/// collapse inner whitespace runs, and trim.
fn normalize_body(text: &str) -> String {
    let staged = text.replace('\r', "");
    let staged = staged.replace('\t', " ");
    let staged: String = staged
        .chars()
        .filter(|character| {
            !matches!(character, '\u{200B}'..='\u{200D}' | '\u{FEFF}')
        })
        .collect();
    staged.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split free text into normalized non-blank lines.
fn split_text_lines(text: &str) -> Vec<String> {
    text.replace('\r', "")
        .split('\n')
        .map(|line| {
            line.replace('\t', " ")
                .chars()
                .filter(|character| {
                    !matches!(character, '\u{200B}'..='\u{200D}' | '\u{FEFF}')
                })
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

/// Strip one leading `- `, `* `, or `• ` bullet marker, if present.
fn strip_bullet_marker(line: &str) -> &str {
    let Some(first) = line.chars().next() else {
        return line;
    };
    if first == '-' || first == '*' || first == '•' {
        let rest = &line[first.len_utf8()..];
        if rest.starts_with([' ', '\t']) {
            return rest.trim_start_matches([' ', '\t']);
        }
    }
    line
}

/// Items with non-blank normalized text; empty items carry no content.
fn nonempty_item_text(text: &str) -> Option<String> {
    let normalized = normalize_body(text);
    (!normalized.is_empty()).then_some(normalized)
}

/// Derive the task title: the note title, else the first non-blank text
/// line (which the caller then drops from the children), else the first
/// list item, else a fallback. Returns the title and whether the first
/// text line was consumed.
fn raw_title(note: &KeepNote) -> (String, bool) {
    let title = normalize_body(&note.content.title);
    if !title.is_empty() {
        return (title, false);
    }
    let lines = split_text_lines(&note.content.text);
    if let Some(first) = lines.first() {
        return (strip_bullet_marker(first).to_string(), true);
    }
    for item in &note.content.items {
        if let Some(text) = nonempty_item_text(&item.text) {
            return (text, false);
        }
    }
    if !note.attachments.is_empty() {
        return ("Google Keep image note".to_string(), false);
    }
    let count = note.content.items.len();
    (
        format!(
            "Untitled Google Keep list ({count} {})",
            if count == 1 { "item" } else { "items" },
        ),
        false,
    )
}

/// Remaining text lines as escaped child bodies, in note order.
fn text_children(note: &KeepNote, consumed_first_line: bool) -> Vec<String> {
    let lines = split_text_lines(&note.content.text);
    lines
        .iter()
        .skip(usize::from(consumed_first_line))
        .map(|line| escape_child_text(strip_bullet_marker(line)))
        .collect()
}

/// Table counts for `list`: text lines beyond the title plus open and
/// checked list items, using the same normalization as the renderer.
pub(super) struct NoteCounts {
    pub(super) extra_lines: usize,
    pub(super) open_items: usize,
    pub(super) checked_items: usize,
}

/// Count a note's table hints: extra text lines, open/checked items.
pub(super) fn note_counts(note: &KeepNote) -> NoteCounts {
    let (_, consumed_first_line) = raw_title(note);
    let extra_lines = text_children(note, consumed_first_line).len();
    let mut open_items = 0;
    let mut checked_items = 0;
    for item in &note.content.items {
        if nonempty_item_text(&item.text).is_none() {
            continue;
        }
        if item.checked {
            checked_items += 1;
        } else {
            open_items += 1;
        }
    }
    NoteCounts {
        extra_lines,
        open_items,
        checked_items,
    }
}

/// List items as `(indented, "- [ ] body")` in Keep display order.
/// Empty items are skipped; `indented` items render one level deeper.
fn list_children(note: &KeepNote) -> Vec<(bool, String)> {
    note.content
        .items
        .iter()
        .filter_map(|item| {
            let normalized = normalize_body(&item.text);
            if normalized.is_empty() {
                return None;
            }
            let checkbox = if item.checked { "[x]" } else { "[ ]" };
            let body = escape_child_text(&normalized);
            Some((item.indented, format!("{checkbox} {body}")))
        })
        .collect()
}

/// `📎 N image(s)/drawing(s)/audio clip(s)/file(s) stay(s) in Google Keep`.
fn attachment_summary(note: &KeepNote) -> String {
    fn count(
        attachments: &[super::model::Attachment],
        kind: AttachmentKind,
    ) -> usize {
        attachments
            .iter()
            .filter(|attachment| attachment.kind == kind)
            .count()
    }
    fn plural(count: usize, singular: &str, plural: &str) -> String {
        format!("{count} {}", if count == 1 { singular } else { plural })
    }
    let images = count(&note.attachments, AttachmentKind::Image);
    let drawings = count(&note.attachments, AttachmentKind::Drawing);
    let audios = count(&note.attachments, AttachmentKind::Audio);
    let others = count(&note.attachments, AttachmentKind::Other);
    let mut parts = Vec::new();
    if images > 0 {
        parts.push(plural(images, "image", "images"));
    }
    if drawings > 0 {
        parts.push(plural(drawings, "drawing", "drawings"));
    }
    if audios > 0 {
        parts.push(plural(audios, "audio clip", "audio clips"));
    }
    if others > 0 {
        parts.push(plural(others, "file", "files"));
    }
    let verb = if note.attachments.len() == 1 {
        "stays"
    } else {
        "stay"
    };
    format!("📎 {} {verb} in Google Keep", parts.join(", "))
}

/// Non-empty OCR lines as escaped grandchild bodies.
fn ocr_children(note: &KeepNote) -> Vec<String> {
    note.attachments
        .iter()
        .filter_map(|attachment| attachment.extracted_text.as_deref())
        .flat_map(split_text_lines)
        .map(|line| escape_child_text(&line))
        .collect()
}

/// Percent-encode Markdown delimiters and whitespace in a source URL,
/// so a crafted id inside the URL cannot plant a
/// parseable `%%gkeep:v1:…%%` marker before the real one.
fn encode_source_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        if c == '%'
            || c == '('
            || c == ')'
            || c == '<'
            || c == '>'
            || c == '['
            || c == ']'
            || c == '"'
            || c == '\\'
            || c.is_whitespace()
        {
            for byte in c.encode_utf8(&mut [0; 4]).as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether normalized text is blank: empty after the renderer's
/// normalization (zero-width removal, trim). Used by the planner so a
/// note whose text is only zero-width characters counts as empty.
pub(super) fn is_normalized_blank(text: &str) -> bool {
    normalize_body(text).is_empty()
}

/// The Keep `created` date in local time as `YYYY-MM-DD`.
fn created_date_in<Tz: chrono::TimeZone>(note: &KeepNote, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    if let Ok(utc) = note.created.parse::<chrono::DateTime<chrono::Utc>>() {
        utc.with_timezone(tz).format("%Y-%m-%d").to_string()
    } else {
        chrono::Utc::now()
            .with_timezone(tz)
            .format("%Y-%m-%d")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::ledger::parse_markers;
    use super::super::model::{
        Attachment, AttachmentKind, KeepContent, KeepItem, KeepNote,
        KeepNoteKind,
    };
    use super::*;

    fn test_note() -> KeepNote {
        KeepNote {
            id: "note-abc-1".to_string(),
            server_id: None,
            kind: KeepNoteKind::Note,
            content: KeepContent {
                title: String::new(),
                text: String::new(),
                items: Vec::new(),
            },
            pinned: false,
            archived: false,
            shared: false,
            labels: Vec::new(),
            attachments: Vec::new(),
            links: Vec::new(),
            created: "2026-09-27T21:14:03Z".to_string(),
            edited: "2026-09-27T21:14:03Z".to_string(),
            url: Some(
                "https://keep.google.com/u/0/#NOTE/note-abc-1".to_string(),
            ),
        }
    }

    fn item(text: &str, checked: bool, indented: bool) -> KeepItem {
        KeepItem {
            text: text.to_string(),
            checked,
            indented,
        }
    }

    fn image(ocr: Option<&str>) -> Attachment {
        Attachment {
            kind: AttachmentKind::Image,
            extracted_text: ocr.map(str::to_string),
        }
    }

    fn marker_fp(note: &KeepNote) -> String {
        format_marker(&note.id, &note.content.fingerprint())
    }

    #[test]
    fn titled_note_renders_task_line_and_children() {
        let mut note = test_note();
        note.content.title = "Call dentist about crown".to_string();
        note.content.text = "They close at 5 on Fridays".to_string();

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block
            .markdown
            .starts_with("- [ ] #task Call dentist about crown "));
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Call dentist about crown [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- They close at 5 on Fridays\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn fallback_child_precedes_hidden_marker() {
        let mut note = test_note();
        note.content.text = "https://example.com/post".to_string();
        let fallback = "⚠️ Clip failed (blocked): wall · retry: bob ref create https://example.com/post";

        let block = render_note_with_fallback(&note, "\t", false, fallback);
        let lines: Vec<&str> = block.markdown.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("- [ ] #task https://example.com/post "));
        assert_eq!(lines[1], format!("\t- {fallback}"));
        assert_eq!(lines[2], format!("\t{}", marker_fp(&note)));
        // Without a fallback the block has no middle child.
        let plain = render_note(&note, "\t", false);
        assert_eq!(plain.markdown.lines().count(), 2);
    }

    #[test]
    fn untitled_multiline_note_takes_first_line_as_title() {
        let mut note = test_note();
        note.url = None;
        note.content.text =
            "Buy oat milk\n- end caps are on sale\n* limit two".to_string();

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block.markdown.starts_with("- [ ] #task Buy oat milk "));
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Buy oat milk [created::2026-09-27]\n\
                 \t- end caps are on sale\n\
                 \t- limit two\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn unicode_text_passes_through() {
        let mut note = test_note();
        note.content.title = "Hardware store #8 × 1¼″ 🧰".to_string();
        note.content.text = "木材とネジ — café naïve".to_string();

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block
            .markdown
            .starts_with("- [ ] #task Hardware store #8 × 1¼″ 🧰 "));
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Hardware store #8 × 1¼″ 🧰 [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- 木材とネジ — café naïve\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn markdown_looking_text_is_neutralized_in_children() {
        let mut note = test_note();
        note.content.title = "Notes".to_string();
        note.content.text =
            "# heading\n> quote\n1. ordered\n| table |\n+ plus\n- dash\n* star"
                .to_string();

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Notes [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- \\# heading\n\
                 \t- \\> quote\n\
                 \t- \\1. ordered\n\
                 \t- \\| table |\n\
                 \t- \\+ plus\n\
                 \t- dash\n\
                 \t- star\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn capture_grammar_lookalikes_stay_literal() {
        let mut note = test_note();
        note.content.title = "Track =x and +5 and 50% and @x".to_string();

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block
            .markdown
            .starts_with("- [ ] #task Track =x and +5 and 50% and @x "));
        assert!(block.markdown.contains("Track =x and +5 and 50% and @x"));
    }

    #[test]
    fn task_and_block_id_and_comment_and_field_hazards_escape() {
        let mut note = test_note();
        note.content.title = "Fix #task before Friday ^abc123".to_string();
        note.content.text =
            "See [due:: tomorrow] and (x:: y) plus 100%% sure".to_string();

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block
            .markdown
            .starts_with("- [ ] #task Fix \\#task before Friday \\^abc123 "));
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Fix \\#task before Friday \\^abc123 [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- See [due\\:\\: tomorrow] and (x\\:\\: y) plus 100%&#37; sure\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn list_with_checked_nested_and_empty_items() {
        let mut note = test_note();
        note.kind = KeepNoteKind::List;
        note.content.title = "Hardware store".to_string();
        note.content.items = vec![
            item("wood screws", false, false),
            item("#8 × 1¼″", false, true),
            item("sandpaper", true, false),
            item("   ", false, false),
        ];

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Hardware store [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- [ ] wood screws\n\
                 \t\t- [ ] #8 × 1¼″\n\
                 \t- [x] sandpaper\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn untitled_list_takes_first_item_as_title_but_keeps_it() {
        let mut note = test_note();
        note.kind = KeepNoteKind::List;
        note.content.items = vec![
            item("peanut butter", false, false),
            item("jam", true, false),
        ];

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block.markdown.starts_with("- [ ] #task peanut butter "));
        // The title item stays in the children: it is still unchecked.
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task peanut butter [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- [ ] peanut butter\n\
                 \t- [x] jam\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
        assert_eq!(display_title(&note), "peanut butter");
    }

    #[test]
    fn empty_list_falls_back_to_untitled_title() {
        let mut note = test_note();
        note.kind = KeepNoteKind::List;
        note.content.items = vec![item("  ", false, false)];

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block
            .markdown
            .starts_with("- [ ] #task Untitled Google Keep list (1 item) "));
    }

    #[test]
    fn unknown_attachment_kind_renders_as_files() {
        let mut note = test_note();
        note.content.title = "Receipt".to_string();
        note.attachments = vec![Attachment {
            kind: AttachmentKind::Other,
            extracted_text: None,
        }];

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block.markdown.contains("- 📎 1 file stays in Google Keep"));
    }

    #[test]
    fn source_url_spoof_cannot_plant_a_marker() {
        let mut note = test_note();
        note.content.title = "Watch out".to_string();
        note.url = Some(
            "https://keep.google.com/u/0/#NOTE/%%gkeep:v1:spoof:000000000000%% (x)".to_string(),
        );

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        let source = block.markdown.lines().next().expect("task line exists");
        assert!(!source.contains("%%gkeep:v1:spoof:"));
        assert!(source.contains("%25%25gkeep:v1:spoof"));
        let markers: Vec<(String, String)> =
            block.markdown.lines().flat_map(parse_markers).collect();
        assert_eq!(
            markers,
            vec![(note.id.clone(), note.content.fingerprint(),)]
        );
    }

    #[test]
    fn source_link_escapes_title_delimiters_and_omits_blank_urls() {
        let mut note = test_note();
        note.url = Some("https://example.test/a(\"b\\c)?q=x#frag".into());
        let block = render_note_in(&note, "  ", false, None, &chrono::Utc);
        assert!(block.markdown.contains(
            "[💡](https://example.test/a%28%22b%5Cc%29?q=x#frag \"Open in Google Keep\")"
        ), "{}", block.markdown);
        let marker = format!("  {}", marker_fp(&note));
        assert_eq!(block.markdown.lines().last(), Some(marker.as_str()));

        for url in [None, Some(""), Some(" \t\n ")] {
            note.url = url.map(str::to_string);
            let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
            assert!(!block.markdown.contains("[💡]"), "{}", block.markdown);
            assert!(block.markdown.contains("[created::2026-09-27]"));
            assert!(block
                .markdown
                .ends_with(&format!("\t{}", marker_fp(&note))));
        }
    }

    #[test]
    fn decorated_description_keeps_created_field_parseable() {
        let mut note = test_note();
        note.content.title = "Hardware store".into();
        let block = render_note_in(&note, "\t", true, None, &chrono::Utc);
        let task_body = block
            .markdown
            .lines()
            .next()
            .and_then(|line| line.split_once("#task ").map(|(_, body)| body))
            .expect("task body");
        let details = crate::native::dataview::parse_details(
            task_body,
            crate::native::dataview::TaskFormat::Dataview,
        );
        assert!(details.created.is_some());
        assert!(details.description.contains("[💡](https://keep.google.com"));
        assert!(details.description.contains("· revised"));
    }

    #[test]
    fn leading_markers_cover_headings_breaks_and_fences() {
        assert_eq!(escape_child_text("## heading"), "\\## heading");
        assert_eq!(escape_child_text("###### deep"), "\\###### deep");
        assert_eq!(escape_child_text("---"), "\\---");
        assert_eq!(escape_child_text("***"), "\\***");
        assert_eq!(escape_child_text("___"), "\\___");
        assert_eq!(escape_child_text("- - -"), "\\- - -");
        assert_eq!(escape_child_text("```rust"), "\\```rust");
        assert_eq!(escape_child_text("~~~"), "\\~~~");
        assert_eq!(escape_child_text("^abc"), "\\^abc");
        assert_eq!(escape_child_text("\u{a0}^abc"), "\u{a0}\\^abc");
        assert_eq!(escape_colon_runs("a ::: b"), "a \\:\\:\\: b");
        assert_eq!(escape_colon_runs("a :::: b"), "a \\:\\:\\:\\: b");
    }

    #[test]
    fn bare_list_markers_escape() {
        assert_eq!(escape_child_text("-"), "\\-");
        assert_eq!(escape_child_text("*"), "\\*");
        assert_eq!(escape_child_text("+"), "\\+");
    }

    #[test]
    fn labels_join_the_source_line() {
        let mut note = test_note();
        note.content.title = "Pick up parcel".to_string();
        note.labels = vec!["errands".to_string(), "weekend".to_string()];

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Pick up parcel [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- 🏷 errands, weekend\n\t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn image_with_ocr_renders_attachment_line_and_grandchildren() {
        let mut note = test_note();
        note.content.title = "Hardware store".to_string();
        note.content.items = vec![item("wood screws", false, false)];
        note.attachments = vec![image(Some("RECEIPT\nTOTAL 12.99"))];

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Hardware store [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- [ ] wood screws\n\
                 \t- 📎 1 image stays in Google Keep\n\
                 \t\t- RECEIPT\n\
                 \t\t- TOTAL 12.99\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn mixed_attachments_pluralize() {
        let mut note = test_note();
        note.attachments = vec![
            image(None),
            Attachment {
                kind: AttachmentKind::Drawing,
                extracted_text: None,
            },
            Attachment {
                kind: AttachmentKind::Audio,
                extracted_text: None,
            },
        ];

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block
            .markdown
            .starts_with("- [ ] #task Google Keep image note "));
        assert!(block.markdown.contains(
            "- 📎 1 image, 1 drawing, 1 audio clip stay in Google Keep"
        ));
    }

    #[test]
    fn revision_annotates_task_description() {
        let mut note = test_note();
        note.content.title = "Call dentist".to_string();

        let block = render_note_in(&note, "\t", true, None, &chrono::Utc);
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Call dentist · revised [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn crlf_and_tabs_normalize() {
        let mut note = test_note();
        note.content.title = "Call\u{200b} dentist\u{feff}".to_string();
        note.content.text = "They close\tat 5\r\n\r\nFridays".to_string();

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(block.markdown.starts_with("- [ ] #task Call dentist "));
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Call dentist [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 \t- They close at 5\n\
                 \t- Fridays\n\
                 \t{}",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn space_indent_uses_two_spaces() {
        let mut note = test_note();
        note.content.title = "Call dentist".to_string();
        note.content.text = "They close at 5".to_string();

        let block = render_note_in(&note, "  ", false, None, &chrono::Utc);
        assert_eq!(
            block.markdown,
            format!(
                "- [ ] #task Call dentist [💡](https://keep.google.com/u/0/#NOTE/note-abc-1 \"Open in Google Keep\") [created::2026-09-27]\n\
                 {}- They close at 5\n\
                 {}{}",
                "  ",
                "  ",
                marker_fp(&note),
            ),
        );
    }

    #[test]
    fn spoofed_marker_in_keep_text_cannot_survive() {
        let mut note = test_note();
        note.content.title = "Watch out".to_string();
        note.content.text =
            "Someone wrote %%gkeep:v1:x:000000000000%% in here".to_string();

        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(!block.markdown.contains("%%gkeep:v1:x:"));
        assert!(block
            .markdown
            .contains("%&#37;gkeep:v1:x:000000000000%&#37;"));
        // Only the real marker parses back out of the block.
        let markers: Vec<(String, String)> =
            block.markdown.lines().flat_map(parse_markers).collect();
        assert_eq!(
            markers,
            vec![(note.id.clone(), note.content.fingerprint(),)],
        );
    }

    #[test]
    fn markdown_has_no_trailing_newline() {
        let note = test_note();
        let block = render_note_in(&note, "\t", false, None, &chrono::Utc);
        assert!(!block.markdown.ends_with('\n'));
    }

    #[test]
    fn display_title_is_unescaped() {
        let mut note = test_note();
        note.content.title = "Fix #task now".to_string();

        assert_eq!(display_title(&note), "Fix #task now");
        assert!(render_note_in(&note, "\t", false, None, &chrono::Utc)
            .markdown
            .starts_with("- [ ] #task Fix \\#task now "));
    }

    #[test]
    fn escape_task_text_leaves_intended_markup_alone() {
        assert_eq!(
            escape_task_text("see [[link]] and https://x.test and #tag"),
            "see [[link]] and https://x.test and #tag",
        );
        assert_eq!(escape_task_text("#tasks"), "#tasks");
        assert_eq!(escape_task_text("(#task)"), "(#task)");
        assert_eq!(escape_task_text("a^b"), "a^b");
        assert_eq!(escape_task_text("trailing ^"), "trailing ^");
    }

    #[test]
    fn escape_child_text_covers_both_tables() {
        assert_eq!(escape_child_text("#task list"), "\\#task list");
        assert_eq!(escape_child_text("> quoted"), "\\> quoted");
        assert_eq!(escape_child_text("2) second"), "\\2) second");
        assert_eq!(escape_child_text("| cell |"), "\\| cell |");
        assert_eq!(escape_child_text("+ plus"), "\\+ plus");
        // Mid-line hazards are untouched by the leading rule.
        assert_eq!(escape_child_text("a > b"), "a > b");
        assert_eq!(escape_child_text("use 1. sugar"), "use 1. sugar");
    }
}
