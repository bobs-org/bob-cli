//! `depends_on` frontmatter reader (`docs/task-dependencies.md`
//! §13.2, §13.11).
//!
//! Parses the project note's `depends_on` list through the shared
//! key-parameterised frontmatter reader, canonicalising every entry
//! to a plain `[[target#^block]]` link: aliases and `!` embeds are
//! canonicalised away, and a bare project link gains `#^prj`.

use super::super::projects::{
    read_frontmatter_list, Frontmatter, FrontmatterList,
};
use super::super::task_dependencies::parse_block_link_inside;

/// Frontmatter key holding a project's dependency links.
pub(crate) const DEPENDS_ON_KEY: &str = "depends_on";

/// One canonicalised `depends_on` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependsOnItem {
    /// Canonical link text (`[[target#^block]]`, never same-note form).
    pub(crate) link: String,
    /// Raw target part before `#^` (empty for same-note `[[#^x]]`).
    pub(crate) target: String,
    /// Block id (`prj` for bare project links).
    pub(crate) block_id: String,
    /// 1-based file line of the entry (the key line for flow lists).
    pub(crate) line_number: usize,
    /// Syntactic same-note target; resolution may flag more entries.
    pub(crate) internal: bool,
}

/// Parsed shape of a note's `depends_on` property (§13.11 PDP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DependsOnProperty {
    /// Key absent (D3: a missing property means empty).
    Absent,
    /// `null`, `""`, `[]`, or a key with no items.
    Empty,
    Items(Vec<DependsOnItem>),
    Malformed(String),
}

/// Parse a note's `depends_on` frontmatter property.
pub(crate) fn parse_depends_on(
    frontmatter: &Frontmatter<'_>,
) -> DependsOnProperty {
    match read_frontmatter_list(frontmatter, DEPENDS_ON_KEY) {
        FrontmatterList::Absent => DependsOnProperty::Absent,
        FrontmatterList::Empty => DependsOnProperty::Empty,
        FrontmatterList::Items(items) => {
            if items.is_empty() {
                return DependsOnProperty::Empty;
            }
            let mut parsed = Vec::with_capacity(items.len());
            for item in &items {
                match parse_depends_on_item(&item.text) {
                    Ok((target, block_id)) => parsed.push(DependsOnItem {
                        link: format!("[[{target}#^{block_id}]]"),
                        internal: target.is_empty(),
                        target,
                        block_id,
                        line_number: item.line_number,
                    }),
                    Err(reason) => {
                        return DependsOnProperty::Malformed(format!(
                            "line {}: {reason}",
                            item.line_number
                        ));
                    }
                }
            }
            DependsOnProperty::Items(parsed)
        }
        FrontmatterList::Scalar { text, line_number } => {
            parse_depends_on_scalar(&text, line_number)
        }
        FrontmatterList::Malformed {
            reason,
            line_number,
        } => DependsOnProperty::Malformed(match line_number {
            Some(line) => format!("line {line}: {reason}"),
            None => reason,
        }),
    }
}

/// One inline `depends_on` value: a quoted link, an empty shape, or
/// malformed.
fn parse_depends_on_scalar(
    text: &str,
    line_number: usize,
) -> DependsOnProperty {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("null")
        || trimmed == "~"
        || trimmed.is_empty()
    {
        return DependsOnProperty::Empty;
    }
    if let Some(content) = unquote_item_scalar(trimmed)
        && content.trim().is_empty()
    {
        return DependsOnProperty::Empty;
    }
    match parse_depends_on_item(trimmed) {
        Ok((target, block_id)) => {
            DependsOnProperty::Items(vec![DependsOnItem {
                link: format!("[[{target}#^{block_id}]]"),
                internal: target.is_empty(),
                target,
                block_id,
                line_number,
            }])
        }
        Err(reason) => DependsOnProperty::Malformed(format!(
            "line {line_number}: {reason}"
        )),
    }
}

/// Canonicalise one raw list entry to `(target, block_id)`.
///
/// The entry must be a quoted `[[note#^block]]` scalar. A bare
/// `[[...]]` item is malformed: YAML reads it as a nested array
/// (§13.2 reader tolerance).
fn parse_depends_on_item(raw: &str) -> Result<(String, String), String> {
    let raw = raw.trim();
    let Some(content) = unquote_item_scalar(raw) else {
        return Err(format!(
            "{raw} is not a quoted task link (YAML reads a bare [[...]] as a nested array)"
        ));
    };
    let content = content.trim();
    // Struck links are not tolerated: they never block and the
    // writer never writes them.
    if content.starts_with("~~") {
        return Err(format!("{raw} is struck; remove it or un-strike it"));
    }
    let content = content.strip_prefix('!').unwrap_or(content);
    if content.starts_with('!')
        || !content.starts_with("[[")
        || !content.ends_with("]]")
    {
        return Err(format!(
            "{raw} is not a task link; expected [[note#^block]]"
        ));
    }
    let body = &content[2..content.len() - 2];
    if body.contains("[[") || body.contains("]]") {
        return Err(format!(
            "{raw} is not one task link; expected [[note#^block]]"
        ));
    }
    if let Some((target, block_id)) = parse_block_link_inside(body) {
        return Ok((target, block_id));
    }
    if !body.contains('#') && !body.contains('|') {
        let target = body.trim();
        if !target.is_empty() {
            // A bare project link canonicalises to `#^prj` (§13.2).
            return Ok((target.to_string(), "prj".to_string()));
        }
    }
    Err(format!(
        "{raw} is not a task link; expected [[note#^block]]"
    ))
}

/// Strip one YAML quote layer, requiring trailing content to be blank
/// or a `#` comment.
fn unquote_item_scalar(text: &str) -> Option<&str> {
    let (quote, rest) = match text.as_bytes().first() {
        Some(b'"') => ('"', &text[1..]),
        Some(b'\'') => ('\'', &text[1..]),
        _ => return None,
    };
    let end = rest.find(quote)?;
    let after = rest[end + quote.len_utf8()..].trim();
    if after.is_empty() || after.starts_with('#') {
        Some(rest[..end].trim())
    } else {
        None
    }
}
