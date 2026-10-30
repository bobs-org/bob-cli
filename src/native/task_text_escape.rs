//! Shared free-text escaping for vault task lines.
//!
//! Moved from `gkeep::render` so every importer (gkeep, gcal) escapes
//! titles the same way. Keep text is data: it sits mid-line, so only
//! the token, comment, field, and block-id hazards apply.

/// Escape free text for the task line: it sits mid-line, so only the
/// token, comment, field, and block-id hazards apply.
pub(crate) fn escape_task_text(text: &str) -> String {
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
}
