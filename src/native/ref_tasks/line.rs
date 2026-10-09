//! Candidate and follow-up line parsing.

/// One `#ref` task line that resolves to no ref note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrphanRefTask {
    pub path: String,
    pub line_index: usize,
    pub reason: String,
    pub target: String,
}

/// One `🔖` follow-up link attributed to a ref note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefFollowUp {
    pub path: String,
    pub line_index: usize,
    pub mark: char,
    pub text: String,
    pub block_id: String,
}

/// Drop leading `>` quote markers: up to three spaces, `>`, one optional
/// space, repeated. Mirrors the private `freshness/placement.rs`
/// `strip_blockquote_prefix`.
pub(crate) fn strip_blockquote_prefix(mut line: &str) -> &str {
    loop {
        let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
        if spaces > 3 || line.as_bytes().get(spaces) != Some(&b'>') {
            return line;
        }
        line = &line[spaces + 1..];
        if let Some(rest) = line.strip_prefix(' ') {
            line = rest;
        }
    }
}

/// The checkbox mark on a task line, if `line` (already blockquote-stripped)
/// is a list task line: indentation, then `-`/`*`/`+`, a space, `[m]`, then
/// end-of-line or whitespace.
pub(crate) fn task_mark(line: &str) -> Option<char> {
    let trimmed_start = line.trim_start_matches(|c| c == ' ' || c == '\t');
    // Indentation then marker.
    let after_marker = trimmed_start
        .strip_prefix("- ")
        .or_else(|| trimmed_start.strip_prefix("* "))
        .or_else(|| trimmed_start.strip_prefix("+ "))?;
    // task_mark callers pass the blockquote-stripped line, but the `- `
    // above already consumed the marker+space; now expect `[m]`.
    let bracket = after_marker.strip_prefix('[')?;
    let mark = bracket.chars().next()?;
    let rest = bracket[mark.len_utf8()..].strip_prefix(']')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(mark)
}

/// True when the line is a task line per the locator rules.
pub(crate) fn is_task_line(line: &str) -> bool {
    task_mark(line).is_some()
}

/// True when whitespace tokens include `#ref`, ASCII case-insensitive.
/// `#references` and `#ref/x` do not count.
pub(crate) fn has_ref_token(line: &str) -> bool {
    line.split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("#ref"))
}

/// True when whitespace tokens include the exact token `^ref`.
pub(crate) fn has_caret_ref(line: &str) -> bool {
    line.split_whitespace().any(|token| token == "^ref")
}

/// True when the line is an embed line (`- ![[...]]` or managed embed):
/// after the checkbox, the body starts with `!`. Such lines are ignored.
pub(crate) fn is_embed_task_line(stripped: &str) -> bool {
    let trimmed_start = stripped.trim_start_matches(|c| c == ' ' || c == '\t');
    let after = trimmed_start
        .strip_prefix("- ")
        .or_else(|| trimmed_start.strip_prefix("* "))
        .or_else(|| trimmed_start.strip_prefix("+ "));
    let Some(after) = after else {
        return false;
    };
    // Skip `[m] ` prefix.
    let Some(bracket_end) = after.find(']') else {
        return false;
    };
    let rest = after[bracket_end + 1..].trim_start();
    rest.starts_with('!')
}

/// Parse a follow-up `## Tasks`-style line via the shared
/// `highlights_ref` parser so both paths strip the `🔖` link and inline
/// fields identically.
pub(crate) fn parse_follow_up_task(
    line: &str,
) -> Option<(char, String, String)> {
    let task = crate::native::highlights_ref::parse_follow_up_task(line)?;
    Some((task.mark, task.text, task.block_id))
}

/// Full block ID length bound (`ref-<slug>` at most 44 characters).
pub(crate) const REF_BLOCK_ID_MAX_LEN: usize = 44;
/// Sanitized title alias bound (at most 100 characters plus `…`).
pub(crate) const REF_TITLE_MAX_LEN: usize = 100;

/// Sanitize a ref-note title for use as a wikilink alias: remove `[`, `]`,
/// `|`, `#`, `^`, and backticks, collapse whitespace runs to one space,
/// trim, then truncate at a word boundary to at most 100 characters plus
/// `…` when longer.
pub(crate) fn sanitize_title_alias(raw: &str) -> String {
    let stripped: String = raw
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '|' | '#' | '^' | '`'))
        .collect();
    let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= REF_TITLE_MAX_LEN {
        return collapsed;
    }
    let mut end = 0usize;
    let mut last_space: Option<usize> = None;
    let mut count = 0usize;
    for (byte, ch) in collapsed.char_indices() {
        if count >= REF_TITLE_MAX_LEN {
            break;
        }
        if ch.is_whitespace() {
            last_space = Some(byte);
        }
        end = byte + ch.len_utf8();
        count += 1;
    }
    let cut = last_space.unwrap_or(end);
    let mut out = collapsed[..cut].trim_end().to_string();
    out.push('…');
    out
}

/// Slug a ref-note stem to lowercase ASCII separated by `-`.
///
/// Every run of characters outside `[a-z0-9]` becomes one `-`; surrounding
/// `-` runs are trimmed. The returned slug keeps the full `ref-<slug>` ID
/// at most 44 characters by cutting at a `-` boundary; an empty slug
/// yields `reading` (so the full ID is `ref-reading`).
pub(crate) fn slug_ref_stem(stem: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true;
    for byte in stem.bytes() {
        let lower = byte.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower as char);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.starts_with('-') {
        slug.remove(0);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        return "reading".to_string();
    }
    fit_slug_to_id_bound(&slug)
}

fn fit_slug_to_id_bound(slug: &str) -> String {
    let max_slug = REF_BLOCK_ID_MAX_LEN - "ref-".len();
    if slug.len() <= max_slug {
        return slug.to_string();
    }
    let truncated = &slug[..max_slug];
    if let Some(pos) = truncated.rfind('-') {
        let cut = truncated[..pos].trim_matches('-');
        if !cut.is_empty() {
            return cut.to_string();
        }
    }
    truncated.trim_matches('-').to_string()
}

/// Allocate a unique `ref-<slug>` block ID for `stem`.
///
/// `is_taken` reports IDs already present in the fresh destination, its
/// `done_tasks` archive, or reserved earlier in this run. Collisions append
/// `-2`, `-3`, … while preserving the 44-character bound.
pub(crate) fn allocate_ref_block_id(
    stem: &str,
    is_taken: &dyn Fn(&str) -> bool,
) -> String {
    allocate_unique_block_id(&format!("ref-{}", slug_ref_stem(stem)), is_taken)
}

/// Allocate a unique block ID from an explicit `base` ID: the base itself
/// when free, else `base-2`, `base-3`, … while preserving the 44-character
/// bound. Shared by fresh allocation and reopen's preferred-ID fallback.
pub(crate) fn allocate_unique_block_id(
    base: &str,
    is_taken: &dyn Fn(&str) -> bool,
) -> String {
    if !is_taken(base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let suffix = format!("-{n}");
        let max_base = REF_BLOCK_ID_MAX_LEN.saturating_sub(suffix.len());
        let mut trimmed = base;
        if trimmed.len() > max_base {
            let cut = &trimmed[..max_base];
            trimmed = cut.rfind('-').map(|p| &cut[..p]).unwrap_or(cut);
            trimmed = trimmed.trim_matches('-');
            if trimmed.is_empty() {
                trimmed = cut.trim_matches('-');
            }
        }
        let candidate = format!("{trimmed}{suffix}");
        if !is_taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Render the managed reading-task embed for `residence` and `block_id`.
///
/// `residence` is the route for a root note (`sase`) or the vault-relative
/// path without `.md` otherwise (`done/sase_done`).
pub(crate) fn managed_embed_line(residence: &str, block_id: &str) -> String {
    format!("![[{residence}#^{block_id}]]")
}

/// Render one physical v2 reading-task line:
///
/// `- [m] #task #ref [[ref/<type>/<stem>|<title>]] [created::YYYY-MM-DD] ^ref-<slug>`
///
/// The link targets the ref note, never the PDF; no `#hide`, verb, or
/// `[fresh::]`. Terminal marks add `[completion:: DATE]` (`x`/`X`) or
/// `[cancelled:: DATE]` (`-`) immediately before the block ID, using the
/// creation date.
pub(crate) fn render_ref_task_line(
    mark: char,
    ref_target: &str,
    title_raw: &str,
    created: &str,
    block_id: &str,
) -> String {
    let alias = sanitize_title_alias(title_raw);
    let mut line = format!(
        "- [{mark}] #task #ref [[{ref_target}|{alias}]] [created::{created}]"
    );
    match mark {
        'x' | 'X' => line.push_str(&format!(" [completion:: {created}]")),
        '-' => line.push_str(&format!(" [cancelled:: {created}]")),
        _ => {}
    }
    line.push_str(&format!(" ^{block_id}"));
    line
}

/// Stamp a close date before any trailing valid `^id` (generalized from the
/// v1 `^ref`-only rule). Terminal marks insert `[completion:: DATE]`
/// (`x`/`X`) or `[cancelled:: DATE]` (`-`) immediately before the trailing
/// ID. Existing stamps are preserved and non-terminal marks are returned
/// unchanged. The date honors `BOB_NOW` via `env::current_datetime`.
pub(crate) fn stamp_close_date_any_id(line: &str, mark: char) -> String {
    let field = match mark {
        'x' | 'X' => "completion",
        '-' => "cancelled",
        _ => return line.to_string(),
    };
    let already = if field == "completion" {
        line.contains("[completion::") || line.contains('✅')
    } else {
        line.contains("[cancelled::") || line.contains('❌')
    };
    if already {
        return line.to_string();
    }
    let Some(token_start) = trailing_caret_id_start(line) else {
        return line.to_string();
    };
    let date = crate::native::env::current_datetime()
        .date()
        .format("%Y-%m-%d");
    format!(
        "{}[{field}:: {date}] {}",
        &line[..token_start],
        &line[token_start..]
    )
}

fn trailing_caret_id_start(line: &str) -> Option<usize> {
    let trimmed_end = line.trim_end().len();
    let trimmed = &line[..trimmed_end];
    let start = trimmed.rfind('^')?;
    if start == 0 || !line.as_bytes()[start - 1].is_ascii_whitespace() {
        return None;
    }
    let id = &trimmed[start + 1..];
    if id.is_empty()
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    Some(start)
}
