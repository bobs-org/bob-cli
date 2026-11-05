//! Shared D8 relocation applier for task-type agreement.
//!
//! Both `bob task move` and capture `#<type>` retypes call this helper so the
//! live Task Link keeps agreeing with the task's new type. It is pure over
//! the day-note contents plus the new type: it finds the dedicated live
//! link, runs `routing::relocate`, applies the winning edit, and reports
//! the ledger relocation for JSON and human output.

use std::collections::BTreeSet;

use crate::native::capture::{
    line_spans, list_item_body, nearest_shallower_list_item_parent,
};
use crate::native::capture_pomodoros::{self, PomodoroState};
use crate::native::capture_task_toggle::{
    child_block_end_line, move_subtree_to_entry,
};
use crate::native::config::themes::ThemeSources;
use crate::native::pomodoro_themes::{
    agree_apply::{self, TaskTypeLookup},
    routing,
};

/// One ledger relocation applied to the daily note.
///
/// Line fields are 0-based headline indices. `source_before` / `dest_before`
/// index the input contents; `dest_after` indexes the updated contents.
/// Capture uses them to compose explicit Pomodoro block refs. `bob task move`
/// ignores them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeRelocation {
    pub(crate) outcome: &'static str,
    pub(crate) from: Option<String>,
    pub(crate) to: String,
    pub(crate) source_before: usize,
    pub(crate) dest_before: Option<usize>,
    pub(crate) dest_after: usize,
    pub(crate) dest_created: bool,
}

fn dest_headline_after_removing_source(
    dest_headline: usize,
    source_line: usize,
    source_end: usize,
) -> usize {
    if source_line < dest_headline {
        dest_headline.saturating_sub(
            source_end.saturating_sub(source_line).saturating_add(1),
        )
    } else {
        dest_headline
    }
}

/// Apply D8 relocation to `day_contents` for `block_id` becoming
/// `type_key`/`type_name`. Returns `None` when the ledger needs no change
/// (no live link, already agreeing, or no Pomodoros section). The ledger
/// never refuses: types are listed, never budgeted.
pub(crate) fn apply_to_day_contents(
    day_contents: &str,
    block_id: &str,
    type_key: &str,
    type_name: &str,
    sources: &ThemeSources,
) -> Option<(String, TypeRelocation)> {
    apply_inner(day_contents, block_id, type_key, type_name, sources, None)
}

/// Same as [`apply_to_day_contents`] but the live link must resolve to
/// `(expected_path, block_id)`. A same-ID link into another note never
/// matches; several open matches for the same task are ambiguous and
/// leave the ledger unchanged.
pub(crate) fn apply_to_day_contents_for_task(
    day_contents: &str,
    daily_path: &std::path::Path,
    expected_path: &std::path::Path,
    block_id: &str,
    type_key: &str,
    type_name: &str,
    sources: &ThemeSources,
) -> Option<(String, TypeRelocation)> {
    apply_inner(
        day_contents,
        block_id,
        type_key,
        type_name,
        sources,
        Some((daily_path, expected_path)),
    )
}

fn apply_inner(
    day_contents: &str,
    block_id: &str,
    type_key: &str,
    type_name: &str,
    sources: &ThemeSources,
    expected: Option<(&std::path::Path, &std::path::Path)>,
) -> Option<(String, TypeRelocation)> {
    if !capture_pomodoros::scan(day_contents).has_section {
        return None;
    }
    let found = find_live_link_filtered(day_contents, block_id, expected)?;
    let scan = capture_pomodoros::scan(day_contents);
    let mut entries = Vec::new();
    for entry in &scan.entries {
        let open = entry.state == PomodoroState::Open;
        let running = open && entry.time_range.is_some();
        let links = if open {
            entry_child_links(day_contents, entry.line).len()
        } else {
            0
        };
        entries.push(routing::RelocateEntry {
            name: entry.name.clone(),
            open,
            running,
            links,
        });
    }
    let live_index = scan
        .entries
        .iter()
        .position(|entry| entry.line == found.entry_line);
    let live_index = live_index?;
    let outcome = routing::relocate(&routing::RelocateInput {
        entries: &entries,
        live_link: Some(live_index),
        type_key,
        type_name,
        sources,
    });
    match outcome {
        routing::RelocateOutcome::Unchanged => None,
        routing::RelocateOutcome::RenameEntry { index, new_name } => {
            let entry = scan.entries.get(index)?;
            let from = entry.name.clone();
            let line = entry.line.saturating_sub(1);
            match rename_entry(day_contents, entry.line, &new_name) {
                Some(updated) => Some((
                    updated,
                    TypeRelocation {
                        outcome: "rename_entry",
                        from,
                        to: new_name,
                        source_before: line,
                        dest_before: Some(line),
                        dest_after: line,
                        dest_created: false,
                    },
                )),
                None => None,
            }
        }
        routing::RelocateOutcome::RenamePlaceholder { index, new_name } => {
            let entry = scan.entries.get(index)?;
            let from = entry.name.clone();
            let line = entry.line.saturating_sub(1);
            match rename_entry(day_contents, entry.line, &new_name) {
                Some(updated) => Some((
                    updated,
                    TypeRelocation {
                        outcome: "rename_placeholder",
                        from,
                        to: new_name,
                        source_before: line,
                        dest_before: Some(line),
                        dest_after: line,
                        dest_created: false,
                    },
                )),
                None => None,
            }
        }
        routing::RelocateOutcome::AppendComponent { index, new_name } => {
            let entry = scan.entries.get(index)?;
            let from = entry.name.clone();
            let line = entry.line.saturating_sub(1);
            match rename_entry(day_contents, entry.line, &new_name) {
                Some(updated) => Some((
                    updated,
                    TypeRelocation {
                        outcome: "append_component",
                        from,
                        to: new_name,
                        source_before: line,
                        dest_before: Some(line),
                        dest_after: line,
                        dest_created: false,
                    },
                )),
                None => None,
            }
        }
        routing::RelocateOutcome::MoveTo { from, to } => {
            let source_entry = scan.entries.get(from)?;
            let dest_entry = scan.entries.get(to)?;
            let from_name = source_entry.name.clone();
            let to_name = dest_entry
                .name
                .clone()
                .unwrap_or_else(|| type_name.to_string());
            let link =
                find_live_link_filtered(day_contents, block_id, expected)?;
            let dest_before = dest_entry.line.saturating_sub(1);
            let dest_after = dest_headline_after_removing_source(
                dest_before,
                link.line_index,
                link.subtree_end,
            );
            match move_subtree_to_entry(
                day_contents,
                link.line_index,
                link.subtree_end,
                dest_before,
            ) {
                Ok((updated, _)) => Some((
                    updated,
                    TypeRelocation {
                        outcome: "move_to",
                        from: from_name,
                        to: to_name,
                        source_before: source_entry.line.saturating_sub(1),
                        dest_before: Some(dest_before),
                        dest_after,
                        dest_created: false,
                    },
                )),
                Err(_) => None,
            }
        }
        routing::RelocateOutcome::CreateBelow { from, name } => {
            let source_entry = scan.entries.get(from)?;
            let from_name = source_entry.name.clone();
            let link =
                find_live_link_filtered(day_contents, block_id, expected)?;
            let spans = line_spans(day_contents);
            let source_index = source_entry.line.saturating_sub(1);
            let source_end = child_block_end_line(&spans, source_index);
            let dest_after_insert = source_end + 1;
            let dest_after = dest_headline_after_removing_source(
                dest_after_insert,
                link.line_index,
                link.subtree_end,
            );
            match create_below_and_move(
                day_contents,
                source_entry.line,
                link.line_index,
                link.subtree_end,
                &name,
            ) {
                Some(updated) => Some((
                    updated,
                    TypeRelocation {
                        outcome: "create_below",
                        from: from_name,
                        to: name,
                        source_before: source_index,
                        dest_before: None,
                        dest_after,
                        dest_created: true,
                    },
                )),
                None => None,
            }
        }
    }
}

/// Agree one touched source entry after a D8 `MoveTo`.
///
/// `source_entry_line` is the 1-based entry line in `day_contents` (the
/// `source_before + 1` that [`TypeRelocation`] reports). Only that entry
/// is agreed; when type inputs are incomplete the lookup resolves
/// everything as unresolved, which blocks drops but never writes.
pub(crate) fn agree_source_entry<L: TaskTypeLookup>(
    day_contents: &str,
    daily_path: &std::path::Path,
    source_entry_line: usize,
    lookup: &mut L,
    sources: &ThemeSources,
) -> Option<String> {
    let mut lines = BTreeSet::new();
    lines.insert(source_entry_line);
    let outcome = agree_apply::agree_ledger_entries(
        day_contents,
        daily_path,
        &agree_apply::EntrySelector::Lines(lines),
        lookup,
        sources,
    );
    outcome.changed.then_some(outcome.contents)
}

pub(crate) struct LiveLink {
    pub(crate) entry_line: usize,
    pub(crate) line_index: usize,
    pub(crate) subtree_end: usize,
}

#[cfg(test)]
pub(crate) fn find_live_link(
    day_contents: &str,
    block_id: &str,
) -> Option<LiveLink> {
    find_live_link_filtered(day_contents, block_id, None)
}

/// Find a dedicated live link to `(expected_path, block_id)`.
///
/// Links resolve to a vault-relative path: an empty target means the
/// daily note itself, otherwise `target_to_markdown_path`. A same-ID
/// link into another note never matches and never poisons ambiguity.
/// Two or more open matches for the same task are ambiguous and leave
/// the ledger unchanged.
#[cfg(test)]
pub(crate) fn find_live_link_for_task(
    day_contents: &str,
    daily_path: &std::path::Path,
    expected_path: &std::path::Path,
    block_id: &str,
) -> Option<LiveLink> {
    find_live_link_filtered(
        day_contents,
        block_id,
        Some((daily_path, expected_path)),
    )
}

fn find_live_link_filtered(
    day_contents: &str,
    block_id: &str,
    expected: Option<(&std::path::Path, &std::path::Path)>,
) -> Option<LiveLink> {
    let spans = line_spans(day_contents);
    let scan = capture_pomodoros::scan(day_contents);
    let mut found: Option<LiveLink> = None;
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        let entry_index = entry.line.saturating_sub(1);
        if entry_index >= spans.len() {
            continue;
        }
        let end = child_block_end_line(&spans, entry_index);
        for line_index in
            entry_index + 1..=end.min(spans.len().saturating_sub(1))
        {
            if nearest_shallower_list_item_parent(&spans, line_index)
                != Some(entry_index)
            {
                continue;
            }
            let text = spans[line_index].text;
            let Some(body) = list_item_body(text).map(str::trim) else {
                continue;
            };
            for span in crate::native::task_dependencies::block_link_spans(text)
            {
                if span.struck || span.embedded {
                    continue;
                }
                if span.block_id == block_id {
                    if let Some((daily_path, expected_path)) = expected
                        && {
                            let resolved = if span.target.is_empty() {
                                daily_path.to_path_buf()
                            } else {
                                match crate::native::vault_links::target_to_markdown_path(
                                    &span.target,
                                ) {
                                    Some(path) => path,
                                    None => continue,
                                }
                            };
                            resolved != expected_path
                        }
                    {
                        continue;
                    }
                    let token = &text[span.token_start..span.end];
                    let trimmed_body = body.trim();
                    if trimmed_body == token.trim()
                        || trimmed_body
                            == format!(
                                "[[{}#^{}]]",
                                span.target, span.block_id
                            )
                            .as_str()
                    {
                        if found.is_some() {
                            return None;
                        }
                        let subtree_end =
                            child_block_end_line(&spans, line_index);
                        found = Some(LiveLink {
                            entry_line: entry.line,
                            line_index,
                            subtree_end,
                        });
                    } else if trimmed_body.contains(&span.block_id)
                        && let Some(inside) = trimmed_body
                            .strip_prefix("[[")
                            .and_then(|rest| rest.strip_suffix("]]"))
                        && let Some((_, id)) =
                            crate::native::task_dependencies::parse_block_link_inside(
                                inside,
                            )
                        && id == block_id
                    {
                        if found.is_some() {
                            return None;
                        }
                        let subtree_end =
                            child_block_end_line(&spans, line_index);
                        found = Some(LiveLink {
                            entry_line: entry.line,
                            line_index,
                            subtree_end,
                        });
                    }
                }
            }
        }
    }
    found
}

pub(crate) fn entry_child_links(
    day_contents: &str,
    entry_line: usize,
) -> Vec<(String, String)> {
    let spans = line_spans(day_contents);
    if entry_line == 0 || entry_line > spans.len() {
        return Vec::new();
    }
    let entry_index = entry_line - 1;
    if entry_index >= spans.len() {
        return Vec::new();
    }
    let end = child_block_end_line(&spans, entry_index);
    let mut links = Vec::new();
    for line_index in entry_index + 1..=end.min(spans.len().saturating_sub(1)) {
        if nearest_shallower_list_item_parent(&spans, line_index)
            != Some(entry_index)
        {
            continue;
        }
        let text = spans[line_index].text;
        let Some(body) = list_item_body(text).map(str::trim) else {
            continue;
        };
        if !(body.starts_with("[[") && body.ends_with("]]")) {
            continue;
        }
        let inside = &body[2..body.len().saturating_sub(2)];
        let Some((target, block_id)) =
            crate::native::task_dependencies::parse_block_link_inside(inside)
        else {
            continue;
        };
        if target.is_empty() {
            continue;
        }
        links.push((target, block_id));
    }
    links
}

pub(crate) fn rename_entry(
    day_contents: &str,
    entry_line: usize,
    new_name: &str,
) -> Option<String> {
    let spans = line_spans(day_contents);
    let entry_index = entry_line.checked_sub(1)?;
    let line = spans.get(entry_index)?.text;
    let base = capture_pomodoros::without_name_tail(line);
    let mut updated =
        String::with_capacity(day_contents.len() + new_name.len() + 4);
    for (index, span) in spans.iter().enumerate() {
        let start = if index == 0 { 0 } else { spans[index - 1].end };
        if index == entry_index {
            let full = &day_contents[start..span.end];
            let ending = if full.ends_with("\r\n") {
                "\r\n"
            } else if full.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            updated.push_str(base.trim_end_matches([' ', '\t']));
            updated.push_str(" — ");
            updated.push_str(new_name);
            updated.push_str(ending);
        } else {
            updated.push_str(&day_contents[start..span.end]);
        }
    }
    Some(updated)
}

pub(crate) fn create_below_and_move(
    day_contents: &str,
    source_entry_line: usize,
    link_line: usize,
    subtree_end: usize,
    new_name: &str,
) -> Option<String> {
    let spans = line_spans(day_contents);
    let source_index = source_entry_line.checked_sub(1)?;
    if source_index >= spans.len() {
        return None;
    }
    let source_end = child_block_end_line(&spans, source_index);
    let placeholder =
        capture_pomodoros::format_named_placeholder_line(new_name);
    let insert_offset = spans
        .get(source_end)
        .map(|span| span.end)
        .unwrap_or_else(|| day_contents.len());
    let ending = if day_contents[..insert_offset].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let with_placeholder = format!(
        "{}{}{}{}",
        &day_contents[..insert_offset],
        placeholder,
        ending,
        &day_contents[insert_offset..]
    );
    // Re-scan to find adjusted indices.
    let new_spans = line_spans(&with_placeholder);
    let new_entry_index = source_end + 1;
    if link_line >= new_spans.len() {
        return None;
    }
    match move_subtree_to_entry(
        &with_placeholder,
        link_line,
        subtree_end,
        new_entry_index,
    ) {
        Ok((updated, _)) => Some(updated),
        Err(_) => None,
    }
}

#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::native::config::themes::{ConfiguredTheme, ThemesConfig};

    fn sources() -> ThemeSources {
        ThemeSources {
            exempt: BTreeSet::new(),
            inventory: BTreeSet::new(),
            themes: Some(ThemesConfig {
                entries: vec![
                    ConfiguredTheme {
                        key: "build".to_string(),
                        name: "BUILD".to_string(),
                        color: None,
                    },
                    ConfiguredTheme {
                        key: "fix".to_string(),
                        name: "FIX".to_string(),
                        color: None,
                    },
                ],
            }),
            warning: None,
        }
    }

    #[test]
    fn rename_running_entry() {
        let day = "## Pomodoros\n- [ ] (**0900-1000**) — BUILD\n  - [[sase#^fix-x]]\n";
        let (updated, info) =
            apply_to_day_contents(day, "fix-x", "fix", "FIX", &sources())
                .expect("changed");
        assert_eq!(info.outcome, "rename_entry");
        assert_eq!(info.from.as_deref(), Some("BUILD"));
        assert_eq!(info.to, "FIX");
        assert_eq!(info.source_before, 1);
        assert_eq!(info.dest_before, Some(1));
        assert_eq!(info.dest_after, 1);
        assert!(!info.dest_created);
        assert!(updated.contains("— FIX"));
        assert!(updated.contains("[[sase#^fix-x]]"));
    }

    #[test]
    fn append_to_running_entry() {
        let day = "## Pomodoros\n- [ ] (**0900-1000**) — BUILD\n  - [[sase#^fix-x]]\n  - [[sase#^other-x]]\n";
        let (updated, info) =
            apply_to_day_contents(day, "fix-x", "fix", "FIX", &sources())
                .expect("changed");
        assert_eq!(info.outcome, "append_component");
        assert_eq!(info.from.as_deref(), Some("BUILD"));
        assert_eq!(info.to, "BUILD + FIX");
        assert_eq!(info.source_before, 1);
        assert_eq!(info.dest_after, 1);
        assert!(!info.dest_created);
        assert!(updated.contains("— BUILD + FIX"));
    }

    #[test]
    fn move_to_existing_entry() {
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-x]]\n- [ ] () — FIX\n";
        let (updated, info) =
            apply_to_day_contents(day, "fix-x", "fix", "FIX", &sources())
                .expect("changed");
        assert_eq!(info.outcome, "move_to");
        assert_eq!(info.to, "FIX");
        assert_eq!(info.source_before, 1);
        assert_eq!(info.dest_before, Some(3));
        assert_eq!(info.dest_after, 2);
        assert!(!info.dest_created);
        assert!(updated.contains("[[sase#^fix-x]]"));
    }

    #[test]
    fn rename_placeholder_entry() {
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-x]]\n";
        let (updated, info) =
            apply_to_day_contents(day, "fix-x", "fix", "FIX", &sources())
                .expect("changed");
        assert_eq!(info.outcome, "rename_placeholder");
        assert_eq!(info.from.as_deref(), Some("BUILD"));
        assert_eq!(info.to, "FIX");
        assert_eq!(info.source_before, 1);
        assert_eq!(info.dest_after, 1);
        assert!(!info.dest_created);
        assert!(updated.contains("— FIX"));
    }

    #[test]
    fn create_below_entry() {
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-x]]\n  - [[sase#^other-x]]\n";
        let (updated, info) =
            apply_to_day_contents(day, "fix-x", "fix", "FIX", &sources())
                .expect("changed");
        assert_eq!(info.outcome, "create_below");
        assert_eq!(info.to, "FIX");
        assert_eq!(info.source_before, 1);
        assert_eq!(info.dest_before, None);
        assert_eq!(info.dest_after, 3);
        assert!(info.dest_created);
        assert!(updated.contains("[[sase#^fix-x]]"));
    }

    #[test]
    fn new_type_past_old_cap_still_creates() {
        // The type cap is gone: a new type always creates below, even
        // when the ledger already holds many types.
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-x]]\n  - [[sase#^other-x]]\n";
        let (updated, info) =
            apply_to_day_contents(day, "fix-x", "fix", "FIX", &sources())
                .expect("changed");
        assert_eq!(info.outcome, "create_below");
        assert_eq!(info.to, "FIX");
        assert!(info.dest_created);
        assert!(updated.contains("[[sase#^fix-x]]"));
    }

    #[test]
    fn already_agreeing_needs_no_change() {
        let day = "## Pomodoros\n- [ ] () — FIX\n  - [[sase#^fix-x]]\n";
        let result =
            apply_to_day_contents(day, "fix-x", "fix", "FIX", &sources());
        assert!(result.is_none());
    }

    #[test]
    fn live_link_matches_resolved_path_not_block_id_alone() {
        use std::path::Path;
        // Same block ID in another note never matches the expected task.
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[other#^dup]]\n";
        let daily = Path::new("2026/20261105.md");
        let expected = Path::new("sase.md");
        assert!(find_live_link_for_task(day, daily, expected, "dup").is_none());
        // Same block ID under the expected home matches.
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^dup]]\n";
        assert!(find_live_link_for_task(day, daily, expected, "dup").is_some());
        // An empty target resolves to the daily note itself.
        let daily_task = Path::new("2026/20261105.md");
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[#^dup]]\n";
        assert!(
            find_live_link_for_task(day, daily, daily_task, "dup").is_some()
        );
        assert!(find_live_link_for_task(day, daily, expected, "dup").is_none());
    }

    #[test]
    fn duplicate_open_matches_are_ambiguous() {
        use std::path::Path;
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^dup]]\n- [ ] () — FIX\n  - [[sase#^dup]]\n";
        let daily = Path::new("2026/20261105.md");
        let expected = Path::new("sase.md");
        assert!(find_live_link_for_task(day, daily, expected, "dup").is_none());
        // The legacy block-ID-only lookup is also ambiguous here.
        assert!(find_live_link(day, "dup").is_none());
    }

    #[test]
    fn move_to_source_agrees_in_same_write() {
        use std::path::PathBuf;

        use crate::native::pomodoro_themes::agree_apply::{
            self, EntrySelector, MapLookup,
        };
        // Post-`MoveTo` state: the FIX link moved to `— FIX`, leaving the
        // source `— BUILD + FIX` with only its BUILD link. Agreeing the
        // source entry drops the extra type so the write is a fixed point.
        let moved = "## Pomodoros\n- [ ] () — BUILD + FIX\n  - [[sase#^build1]]\n- [ ] () — FIX\n  - [[sase#^fix1]]\n";
        let daily = PathBuf::from("2026/20261105.md");
        let source_line = 2usize;
        let mut lookup = MapLookup {
            entries: [(
                ("sase".to_string(), "build1".to_string()),
                agree_apply::TaskResolution::Typed {
                    key: "build".to_string(),
                    name: "BUILD".to_string(),
                    is_open: true,
                    has_repair: false,
                },
            )]
            .into_iter()
            .collect(),
        };
        let agreed = agree_source_entry(
            moved,
            &daily,
            source_line,
            &mut lookup,
            &sources(),
        )
        .expect("agrees source");
        assert!(agreed.contains("— BUILD\n"));
        assert!(!agreed.contains("— BUILD + FIX"));
        // The agreed output is a fixed point over the touched entry.
        let mut second_lookup = MapLookup {
            entries: [(
                ("sase".to_string(), "build1".to_string()),
                agree_apply::TaskResolution::Typed {
                    key: "build".to_string(),
                    name: "BUILD".to_string(),
                    is_open: true,
                    has_repair: false,
                },
            )]
            .into_iter()
            .collect(),
        };
        let second = agree_apply::agree_ledger_entries(
            &agreed,
            &daily,
            &EntrySelector::Lines([source_line].into_iter().collect()),
            &mut second_lookup,
            &sources(),
        );
        assert!(!second.changed);
    }
}
