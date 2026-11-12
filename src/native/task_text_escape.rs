//! Shared free-text escaping for vault task lines.
//!
//! Moved from `gkeep::render` so every importer (gkeep, gcal) escapes
//! titles the same way. Keep text is data: it sits mid-line, so only
//! the token, comment, field, and block-id hazards apply.
//!
//! Imported prose (gcal descriptions) goes further through
//! [`escape_note_inline`] (mid-line hazards) and [`escape_note_text`]
//! (plus the line-start hazards for text that begins a list item body).
//! Both leave URLs alone — link targets are the bullet renderer's job —
//! and neither applies the drawer or managed-line guards; those need
//! the row context and live in the bullet renderer.

use std::sync::LazyLock;

use regex::Regex;

/// Escape free text for the task line: it sits mid-line, so only the
/// token, comment, field, and block-id hazards apply.
pub(crate) fn escape_task_text(text: &str) -> String {
    let staged = text.replace("%%", "%&#37;");
    let staged = escape_colon_runs(&staged);
    let staged = escape_hash_task_token(&staged);
    escape_trailing_block_id(&staged)
}

/// Escape free text for a child bullet: the task-line escaping plus the
/// leading-marker rule, since a child starts a fresh list item.
pub(crate) fn escape_child_text(text: &str) -> String {
    escape_leading_marker(&escape_task_text(text))
}

/// Escape mid-line prose so it stays inert data.
///
/// Layering: the caller (the description parser) splits structure first —
/// links, bold/italic spans, list markers — and passes only text nodes
/// here. So this escapes the text-node hazards (`code`, `[refs]`,
/// `<tags>`, `#tags`, `$math$`, 🍅) and leaves link URLs, which may
/// legitimately contain `#fragments`, untouched.
pub(crate) fn escape_note_inline(text: &str) -> String {
    // Organizer-written escapes first, so they cannot unescape what follows.
    let staged = escape_backslash_before_punct(text);
    let staged = escape_task_text(&staged);
    let staged = staged.replace('`', "\\`");
    let staged = staged.replace('[', "\\[");
    let staged = staged.replace('<', "\\<");
    let staged = escape_tag_hashes(&staged);
    let staged = escape_repeated_dollars(&staged);
    staged.replace('🍅', "&#127813;")
}

/// Escape prose that begins a list item body: the inline escaper plus
/// the line-start rules (headings, quotes, tables, lists, breaks, fences,
/// ordered markers). Input is one body line.
pub(crate) fn escape_note_text(text: &str) -> String {
    escape_note_leading(&escape_note_inline(text))
}

/// A backslash before ASCII punctuation would form a CommonMark escape,
/// so double it: organizer-typed `\[` stays literal instead of
/// unescaping the `[` we are about to neutralize.
fn escape_backslash_before_punct(text: &str) -> String {
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

/// `\#` for a `#` that starts the node or follows whitespace, when a
/// non-space, non-`#` character follows. That is the tag shape
/// (`#offclock`, `#task`); `##`, `# `, and `/#frag` stay literal.
///
/// `gcal::render::escape_title` reuses this after clock tokens are
/// stripped, so organizer-authored mid-line tags stay data.
pub(crate) fn escape_tag_hashes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &current) in chars.iter().enumerate() {
        if current == '#'
            && (index == 0 || chars[index - 1].is_whitespace())
            && chars
                .get(index + 1)
                .is_some_and(|next| !next.is_whitespace() && *next != '#')
        {
            out.push('\\');
        }
        out.push(current);
    }
    out
}

/// `\$` for every `$` on a line that holds two or more of them (inline
/// math such as `$5-$10`). A lone `$` (`Q4: $5`) is literal and stays.
fn escape_repeated_dollars(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (line_number, line) in text.split('\n').enumerate() {
        if line_number > 0 {
            out.push('\n');
        }
        if line.chars().filter(|&c| c == '$').count() >= 2 {
            for c in line.chars() {
                if c == '$' {
                    out.push_str("\\$");
                } else {
                    out.push(c);
                }
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The line-start half of [`escape_note_text`]: headings, quotes, tables,
/// bare list markers, thematic breaks, and `~~~` fences get a leading
/// backslash, while `N.` / `N)` markers escape their punctuation (`N\.`
/// / `N\)`) because a backslash before a digit is not a CommonMark
/// escape and would show as a stray backslash in Source mode.
fn escape_note_leading(text: &str) -> String {
    if text.starts_with('#') {
        return format!("\\{text}");
    }
    if let Some((digits, marker, rest)) = split_ordered_marker(text) {
        return format!("{digits}\\{marker}{rest}");
    }
    let hazardous = text.starts_with('>')
        || text.starts_with('|')
        || starts_with_bare_list_marker(text)
        || is_thematic_break(text)
        || text.starts_with("~~~");
    if hazardous {
        format!("\\{text}")
    } else {
        text.to_string()
    }
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

/// A leading `+`, `-`, or `*` followed by whitespace or end starts a
/// list item (or is a bare marker).
fn starts_with_bare_list_marker(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some('+' | '*' | '-') => chars.next().is_none_or(|c| c.is_whitespace()),
        _ => false,
    }
}

static LEADING_MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(#{1,6}(\s|$)|>|\d+[.)](\s|$)|\||[+*-](\s|$))")
        .expect("valid leading-marker regex")
});

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_task_text_leaves_intended_markup_alone() {
        assert_eq!(
            escape_task_text("see [[link]] and https://x.test and #tag"),
            "see [[link]] and https://x.test and #tag"
        );
        assert_eq!(escape_task_text("#tasks"), "#tasks");
        assert_eq!(escape_task_text("(#task)"), "(#task)");
        assert_eq!(escape_task_text("a^b"), "a^b");
        assert_eq!(escape_task_text("trailing ^"), "trailing ^");
    }

    #[test]
    fn note_inline_neutralizes_code_spans_queries_and_math() {
        assert_eq!(
            escape_note_inline("Run `$= dv.el(\"b\", 1)` now"),
            "Run \\`$= dv.el(\"b\", 1)\\` now"
        );
        assert_eq!(
            escape_note_inline("costs $5-$10 total"),
            "costs \\$5-\\$10 total"
        );
        assert_eq!(
            escape_note_inline("Use <% tp.date.now() %> here"),
            "Use \\<% tp.date.now() %> here"
        );
    }

    #[test]
    fn note_inline_neutralizes_refs_tags_and_fields() {
        assert_eq!(
            escape_note_inline("See [[Secret Note]]"),
            "See \\[\\[Secret Note]]"
        );
        assert_eq!(
            escape_note_inline("Bring shoes #offclock"),
            "Bring shoes \\#offclock"
        );
        assert_eq!(
            escape_note_inline("RSVP #task by Friday"),
            "RSVP \\#task by Friday"
        );
        assert_eq!(escape_note_inline("Room:: 4B"), "Room\\:\\: 4B");
        assert_eq!(
            escape_note_inline("See [due:: tomorrow] plus 100%% sure"),
            "See \\[due\\:\\: tomorrow] plus 100%&#37; sure"
        );
        assert_eq!(
            escape_note_inline("Bring 🍅 salad"),
            "Bring &#127813; salad"
        );
    }

    #[test]
    fn note_inline_disarms_organizer_escapes_first() {
        assert_eq!(escape_note_inline("\\#notatag"), "\\\\#notatag");
        assert_eq!(escape_note_inline("trailing \\"), "trailing \\");
    }

    #[test]
    fn note_text_adds_line_start_guards() {
        // Mid-line `>` and `1.` stay; at line start they escape.
        assert_eq!(escape_note_inline("> quote"), "> quote");
        assert_eq!(escape_note_inline("use 1. sugar"), "use 1. sugar");
        assert_eq!(escape_note_text("> quote"), "\\> quote");
        assert_eq!(escape_note_text("| cell |"), "\\| cell |");
        assert_eq!(escape_note_text("+ plus"), "\\+ plus");
        assert_eq!(escape_note_text("- dash"), "\\- dash");
        assert_eq!(escape_note_text("* star"), "\\* star");
        assert_eq!(escape_note_text("# heading"), "\\# heading");
        assert_eq!(escape_note_text("## heading"), "\\## heading");
        assert_eq!(escape_note_text("#tag"), "\\#tag");
        assert_eq!(escape_note_text("---"), "\\---");
        assert_eq!(escape_note_text("***"), "\\***");
        assert_eq!(escape_note_text("```rust"), "\\`\\`\\`rust");
        assert_eq!(escape_note_text("~~~"), "\\~~~");
        assert_eq!(escape_note_text("1. ordered"), "1\\. ordered");
        assert_eq!(escape_note_text("2) second"), "2\\) second");
    }

    #[test]
    fn note_escapers_leave_ordinary_prose_alone() {
        for prose in [
            "AT&T",
            "5 > 3",
            "a:b",
            "see https://x.test/#frag",
            "C# and F#",
            "Q4: $5",
            "it's 50% done",
            "*emphasis*",
            "~~strike~~",
            "TBD",
        ] {
            assert_eq!(escape_note_inline(prose), prose, "{prose}");
            assert_eq!(escape_note_text(prose), prose, "{prose}");
        }
    }
}
