//! Managed-embed detection, reused by `ref-sync-v2` for healing.

use crate::native::markdown;
use crate::native::task_dependencies::parse_block_link_inside;

/// One managed reading-task embed: `![[<target>#^<id>]]` on its own line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedEmbed {
    pub line_index: usize,
    pub target: String,
    pub block_id: String,
}

/// Find the managed embed between the H1 and the first `##` heading (or the
/// managed region begin), skipping fenced lines.
///
/// Returns the first line whose trimmed text is exactly one block embed
/// `![[<target>#^<id>]]`: no alias, nothing else on the line.
pub(crate) fn find_managed_embed(body: &str) -> Option<ManagedEmbed> {
    let lines: Vec<&str> = body.lines().collect();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    // End at the first `##` heading or the managed region begin.
    let mut end = lines.len();
    let mut h1_seen = false;
    let mut start = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if fenced.contains(&index) {
            continue;
        }
        if line.contains("<!-- highlights:begin -->") {
            end = index;
            break;
        }
        if let Some((level, _)) = markdown::atx_heading(line) {
            if level == 1 && !h1_seen {
                h1_seen = true;
                start = index + 1;
                continue;
            }
            if h1_seen && level == 2 {
                end = index;
                break;
            }
        }
    }
    if !h1_seen {
        // No H1: search from the top to the first `##` or region begin.
        start = 0;
        end = lines.len();
        for (index, line) in lines.iter().enumerate() {
            if fenced.contains(&index) {
                continue;
            }
            if line.contains("<!-- highlights:begin -->") {
                end = index;
                break;
            }
            if let Some((level, _)) = markdown::atx_heading(line)
                && level == 2
            {
                end = index;
                break;
            }
        }
    }
    for index in start..end.min(lines.len()) {
        if fenced.contains(&index) {
            continue;
        }
        let trimmed = lines[index].trim();
        let Some(inner) = trimmed
            .strip_prefix("![[")
            .and_then(|rest| rest.strip_suffix("]]"))
        else {
            continue;
        };
        if inner.contains('|') {
            continue;
        }
        let Some((target, block_id)) = parse_block_link_inside(inner) else {
            continue;
        };
        return Some(ManagedEmbed {
            line_index: index,
            target,
            block_id,
        });
    }
    None
}
