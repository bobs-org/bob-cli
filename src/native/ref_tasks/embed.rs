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

/// Find the managed embed in its designated slot: one blank line below the
/// H1 (leading blank/embed run only), skipping fenced lines.
///
/// Returns the first line whose trimmed text is exactly one block embed
/// `![[<target>#^<id>]]`: no alias, nothing else on the line. An unrelated
/// authored block embed in a later introductory paragraph (after prose) is
/// never managed, so healing preserves it and the dirty guard refuses its
/// change as non-managed. No-H1 bodies keep the legacy top-to-first-`##`
/// search; fenced lines are always ignored.
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
    if h1_seen {
        // Designated slot only: the leading blank/embed run below H1.
        // Prose (or a heading/region) ends the slot so a later authored
        // block embed is never managed; standalone non-managed embeds
        // (aliases, audio) are skipped like blanks. Duplicates within the
        // run still collapse via repeated healing.
        for index in start..end.min(lines.len()) {
            if fenced.contains(&index) {
                continue;
            }
            let line = lines[index];
            if line.contains("<!-- highlights:begin -->") {
                break;
            }
            if let Some((level, _)) = markdown::atx_heading(line) {
                // H2 (or any later heading) ends the intro; the slot never
                // crosses it. The initial H1 itself is behind us.
                if level >= 2 {
                    break;
                }
                // Any other heading text counts as prose and ends the run.
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(inner) = trimmed
                .strip_prefix("![[")
                .and_then(|rest| rest.strip_suffix("]]"))
            else {
                break;
            };
            if inner.contains('|') {
                continue;
            }
            let Some((target, block_id)) = parse_block_link_inside(inner)
            else {
                continue;
            };
            return Some(ManagedEmbed {
                line_index: index,
                target,
                block_id,
            });
        }
        return None;
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
