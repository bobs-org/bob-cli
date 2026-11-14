//! Read the shared row grammar inside a generated Projects catalog.

use crate::native::host_catalog::{owned_regions, RegionDamage, PROJECTS};

pub(crate) const SCHEDULED_GLYPH: &str = "🗓️";
pub(crate) const BLOCKED_GLYPH: &str = "⛓️";
pub(crate) const WARNING_GLYPH: &str = "⚠️";
pub(crate) const DONE_GLYPH: &str = "✅";
pub(crate) const CANCELED_GLYPH: &str = "❌";
pub(crate) const CLOSED_GROUP_LABEL: &str = "**Closed**";
pub(crate) const ROW_SEPARATOR: &str = " — ";

/// Render one catalog row: `> - [glyph ]**[[link]]**[ — criterion]`.
/// Closed-group indentation is applied by the planner; every row parses
/// back through [`parse_catalog`] to the same key and state.
pub(crate) fn render_row(
    link_inner: &str,
    state: CatalogRowState,
    criterion: Option<&str>,
) -> String {
    let mut row = String::from("> - ");
    let glyph = match state {
        CatalogRowState::Open => None,
        CatalogRowState::Scheduled => Some(SCHEDULED_GLYPH),
        CatalogRowState::Blocked => Some(BLOCKED_GLYPH),
        CatalogRowState::Warning => Some(WARNING_GLYPH),
        CatalogRowState::Done => Some(DONE_GLYPH),
        CatalogRowState::Canceled => Some(CANCELED_GLYPH),
    };
    if let Some(glyph) = glyph {
        row.push_str(glyph);
        row.push(' ');
    }
    row.push_str("**[[");
    row.push_str(link_inner);
    row.push_str("]]**");
    if let Some(criterion) = criterion
        && !criterion.is_empty()
    {
        row.push_str(ROW_SEPARATOR);
        row.push_str(criterion);
    }
    row
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogRowState {
    Open,
    Scheduled,
    Blocked,
    Warning,
    Done,
    Canceled,
}

impl CatalogRowState {
    pub(crate) fn is_open(self) -> bool {
        !matches!(self, Self::Done | Self::Canceled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogRow {
    /// Lowercased last path segment of the note target, without heading.
    pub(crate) key: String,
    /// The first wikilink's path target, before its alias (including a heading).
    pub(crate) link_target: String,
    pub(crate) state: CatalogRowState,
    /// 1-based physical line in the source note.
    pub(crate) line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CatalogRead {
    Absent,
    Damaged(RegionDamage),
    Rows(Vec<CatalogRow>),
}

/// Parse all real Projects callouts in a note. The host-catalog scanner owns
/// callout recognition, so examples in frontmatter and fences stay invisible.
pub(crate) fn parse_catalog(text: &str) -> CatalogRead {
    let regions = match owned_regions(&PROJECTS, text) {
        Ok(regions) => regions,
        Err(damage) => return CatalogRead::Damaged(damage),
    };
    if regions.is_empty() {
        return CatalogRead::Absent;
    }

    let rows = regions
        .into_iter()
        .flat_map(|region| {
            let start_line = region.start_line;
            region.lines.into_iter().enumerate().filter_map(
                move |(offset, line)| {
                    parse_row(&line).map(|(link_target, key, state)| {
                        CatalogRow {
                            key,
                            link_target,
                            state,
                            line: start_line + offset,
                        }
                    })
                },
            )
        })
        .collect();
    CatalogRead::Rows(rows)
}

pub(crate) fn parse_row(
    line: &str,
) -> Option<(String, String, CatalogRowState)> {
    let quote_indent = line.chars().take_while(|ch| *ch == ' ').count();
    if quote_indent > 3 {
        return None;
    }
    let after_quote = line[quote_indent..].strip_prefix('>')?;
    let list_content = after_quote.trim_start_matches([' ', '\t']);
    let item = list_item_content(list_content)?;
    let link_start = item.find("[[")?;
    let close_start = link_start + 2 + item[link_start + 2..].find("]]")?;
    let raw_inner = &item[link_start + 2..close_start];
    if raw_inner.contains("[[") {
        return None;
    }
    let raw_target = raw_inner
        .split_once('|')
        .map_or(raw_inner, |(target, _)| target)
        .trim();
    let key_target = raw_target
        .split_once('#')
        .map_or(raw_target, |(target, _)| target)
        .trim();
    let key = key_target.rsplit('/').next()?.trim().to_ascii_lowercase();
    if key.is_empty() {
        return None;
    }
    let state = row_state(item[..link_start].trim_start());
    Some((raw_target.to_string(), key, state))
}

fn list_item_content(content: &str) -> Option<&str> {
    let content = content.trim_start_matches([' ', '\t']);
    let bytes = content.as_bytes();
    let first = *bytes.first()?;
    if matches!(first, b'-' | b'+' | b'*')
        && bytes.get(1).is_some_and(u8::is_ascii_whitespace)
    {
        return Some(content[2..].trim_start_matches([' ', '\t']));
    }
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits > 0
        && bytes
            .get(digits)
            .is_some_and(|byte| matches!(byte, b'.' | b')'))
        && bytes.get(digits + 1).is_some_and(u8::is_ascii_whitespace)
    {
        return Some(content[digits + 2..].trim_start_matches([' ', '\t']));
    }
    None
}

fn row_state(before_link: &str) -> CatalogRowState {
    for (glyph, state) in [
        (SCHEDULED_GLYPH, CatalogRowState::Scheduled),
        (BLOCKED_GLYPH, CatalogRowState::Blocked),
        (WARNING_GLYPH, CatalogRowState::Warning),
        (DONE_GLYPH, CatalogRowState::Done),
        (CANCELED_GLYPH, CatalogRowState::Canceled),
    ] {
        if glyph_prefix_len(before_link, glyph).is_some() {
            return state;
        }
    }
    CatalogRowState::Open
}

pub(crate) fn glyph_prefix_len(text: &str, glyph: &str) -> Option<usize> {
    if text.starts_with(glyph) {
        return Some(glyph.len());
    }
    let base = glyph.strip_suffix('\u{fe0f}')?;
    text.starts_with(base).then_some(base.len())
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn group_label_is_not_a_row_and_link_targets_keep_path_and_heading() {
        assert_eq!(CLOSED_GROUP_LABEL, "**Closed**");
        assert_eq!(ROW_SEPARATOR, " — ");
        let parsed = parse_catalog(concat!(
            "> [!projects]\n",
            "> - **Closed**\n",
            "> - **[[dir/Child#Now|Alias]]** — Criterion\n",
        ));
        let CatalogRead::Rows(rows) = parsed else {
            panic!("catalog should parse");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, "child");
        assert_eq!(rows[0].link_target, "dir/Child#Now");
        assert_eq!(rows[0].line, 3);
        assert!(rows[0].state.is_open());

        let regions = owned_regions(
            &PROJECTS,
            concat!(
                "> [!projects]\n",
                "> - **Closed**\n",
                "> - **[[dir/Child#Now|Alias]]** — Criterion\n",
            ),
        )
        .expect("region scan");
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start_line, 1);
        assert_eq!(regions[0].end_line, 3);
        assert_eq!(regions[0].lines.len(), 3);
    }
}
