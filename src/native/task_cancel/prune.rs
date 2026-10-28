//! Pure immediate-semantics Pomodoro link pruning (`docs/task-relations.md` §17).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::native::{
    pomodoro::pomodoros_section_range,
    task_status_hooks::{after_list_marker, logical_lines, scan_pomodoros},
    vault_links::NoteIndex,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskIdentity {
    pub(crate) path: PathBuf,
    pub(crate) block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PruneResult {
    pub(crate) content: Option<String>,
    pub(crate) changed: bool,
    pub(crate) removed_link_count: usize,
    pub(crate) removed_bullet_count: usize,
    pub(crate) removed_entry_count: usize,
    pub(crate) unresolved_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct Match {
    start: usize,
    end: usize,
    bullet_line: usize,
    subtree_end: usize,
    dedicated: bool,
}

pub(crate) fn prune_daily(
    daily_path: &Path,
    daily_content: Option<&str>,
    targets: &BTreeSet<TaskIdentity>,
    index: &NoteIndex,
) -> PruneResult {
    let Some(content) = daily_content else {
        return PruneResult {
            content: None,
            changed: false,
            removed_link_count: 0,
            removed_bullet_count: 0,
            removed_entry_count: 0,
            unresolved_count: 0,
        };
    };
    let lines = logical_lines(content);
    let Some(section) = pomodoros_section_range(&lines) else {
        return unchanged(content);
    };
    let model = scan_pomodoros(&lines, section);
    let mut matches = Vec::new();
    let mut unresolved = 0usize;

    for bullet in &model.bullets {
        let entry = &model.entries[bullet.entry_index];
        if !entry.open {
            continue;
        }
        for link in &bullet.links {
            if link.struck {
                continue;
            }
            let resolved =
                index.resolve(Some(daily_path), &link.reference.target);
            let identity = resolved.map(|path| TaskIdentity {
                path,
                block_id: link.reference.block_id.clone(),
            });
            let matches_target = identity
                .as_ref()
                .is_some_and(|identity| targets.contains(identity));
            if matches_target {
                matches.push(Match {
                    start: link.edit_start,
                    end: link.edit_end,
                    bullet_line: bullet.line_index,
                    subtree_end: bullet.end_line,
                    dedicated: false,
                });
            } else if identity.is_none()
                && targets
                    .iter()
                    .any(|target| target.block_id == link.reference.block_id)
            {
                unresolved += 1;
            } else {
            }
        }
    }

    for matched in &mut matches {
        let Some(bullet) = model
            .bullets
            .iter()
            .find(|bullet| bullet.line_index == matched.bullet_line)
        else {
            continue;
        };
        let occurrences = &bullet.links;
        matched.dedicated = occurrences.iter().all(|link| {
            !link.struck
                && targets.contains(&TaskIdentity {
                    path: index
                        .resolve(Some(daily_path), &link.reference.target)
                        .unwrap_or_default(),
                    block_id: link.reference.block_id.clone(),
                })
        }) && is_dedicated_line(
            lines[matched.bullet_line],
            occurrences
                .iter()
                .map(|link| (link.edit_start, link.edit_end)),
        );
    }

    if matches.is_empty() {
        return PruneResult {
            unresolved_count: unresolved,
            ..unchanged(content)
        };
    }

    let mut deleted_lines = BTreeSet::new();
    let mut removed_bullet_count = 0usize;
    for matched in &matches {
        if !matched.dedicated || deleted_lines.contains(&matched.bullet_line) {
            continue;
        }
        if (matched.bullet_line..matched.subtree_end)
            .all(|line| deleted_lines.contains(&line))
        {
            continue;
        }
        deleted_lines.extend(matched.bullet_line..matched.subtree_end);
        removed_bullet_count += 1;
    }

    let mut edits_by_line = BTreeMap::<usize, Vec<(usize, usize)>>::new();
    for matched in &matches {
        if deleted_lines.contains(&matched.bullet_line) || matched.dedicated {
            continue;
        }
        edits_by_line
            .entry(matched.bullet_line)
            .or_default()
            .push((matched.start, matched.end));
    }
    let mut segments = content
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    for (line_index, edits) in edits_by_line {
        let Some(segment) = segments.get_mut(line_index) else {
            continue;
        };
        let ending = line_ending(segment).to_string();
        let mut updated = logical_line(segment).to_string();
        let mut edits = edits;
        edits.sort_by(|left, right| right.0.cmp(&left.0));
        for (start, end) in edits {
            if start <= end && end <= updated.len() {
                updated = remove_token_and_normalize_gap(&updated, start, end);
            }
        }
        *segment = format!("{updated}{ending}");
    }
    let output = segments
        .into_iter()
        .enumerate()
        .filter_map(|(line, segment)| {
            (!deleted_lines.contains(&line)).then_some(segment)
        })
        .collect::<String>();
    PruneResult {
        changed: output != content,
        content: Some(output),
        removed_link_count: matches.len(),
        removed_bullet_count,
        removed_entry_count: 0,
        unresolved_count: unresolved,
    }
}

fn unchanged(content: &str) -> PruneResult {
    PruneResult {
        content: Some(content.to_string()),
        changed: false,
        removed_link_count: 0,
        removed_bullet_count: 0,
        removed_entry_count: 0,
        unresolved_count: 0,
    }
}

fn is_dedicated_line<'a>(
    line: &str,
    spans: impl Iterator<Item = (usize, usize)>,
) -> bool {
    let Some(marker_end) = after_list_marker(
        line,
        line.bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count(),
    ) else {
        return false;
    };
    let mut spans = spans.collect::<Vec<_>>();
    spans.sort_by_key(|span| span.0);
    let Some((first_start, _)) = spans.first().copied() else {
        return false;
    };
    let Some((_, last_end)) = spans.last().copied() else {
        return false;
    };
    line.get(marker_end..first_start)
        .map(str::trim)
        .is_some_and(str::is_empty)
        && line
            .get(last_end..)
            .map(str::trim)
            .is_some_and(str::is_empty)
        && spans.windows(2).all(|pair| {
            line.get(pair[0].1..pair[1].0)
                .map(str::trim)
                .is_some_and(str::is_empty)
        })
}

fn remove_token_and_normalize_gap(
    line: &str,
    start: usize,
    end: usize,
) -> String {
    let prefix = &line[..start];
    let suffix = &line[end..];
    let left_space = prefix.chars().last().is_some_and(char::is_whitespace);
    let right_space = suffix.chars().next().is_some_and(char::is_whitespace);
    let prefix = if left_space {
        prefix.trim_end()
    } else {
        prefix
    };
    let suffix = if right_space {
        suffix.trim_start()
    } else {
        suffix
    };
    match (
        left_space,
        right_space,
        prefix.is_empty(),
        suffix.is_empty(),
    ) {
        (true, true, _, _) if !prefix.is_empty() && !suffix.is_empty() => {
            format!("{prefix} {suffix}")
        }
        _ => format!("{prefix}{suffix}"),
    }
}

fn line_ending(segment: &str) -> &str {
    if segment.ends_with("\r\n") {
        "\r\n"
    } else if segment.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}
