//! Shared D8 relocation applier for task-type agreement.
//!
//! Both `bob task move` and capture `t:` retypes call this helper so the
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
use crate::native::plan_budget::{normalize_component, split_components};
use crate::native::pomodoro_themes::routing;

/// One ledger relocation applied to the daily note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeRelocation {
    pub(crate) outcome: &'static str,
    pub(crate) from: Option<String>,
    pub(crate) to: String,
}

/// Strict-cap refusal from the shared applier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StrictCapRefusal {
    pub(crate) name: String,
    pub(crate) count: usize,
    pub(crate) cap: u32,
}

/// Apply D8 relocation to `day_contents` for `block_id` becoming
/// `type_key`/`type_name`. Returns `None` when the ledger needs no change
/// (no live link, already agreeing, or no Pomodoros section).
pub(crate) fn apply_to_day_contents(
    day_contents: &str,
    block_id: &str,
    type_key: &str,
    type_name: &str,
    sources: &ThemeSources,
    cap: u32,
    strict: bool,
) -> Result<Option<(String, TypeRelocation)>, StrictCapRefusal> {
    if !capture_pomodoros::scan(day_contents).has_section {
        return Ok(None);
    }
    let Some(found) = find_live_link(day_contents, block_id) else {
        return Ok(None);
    };
    let scan = capture_pomodoros::scan(day_contents);
    let mut entries = Vec::new();
    let mut ledger_keys = Vec::new();
    for entry in &scan.entries {
        let open = entry.state == PomodoroState::Open;
        let running = open && entry.time_range.is_some();
        let links = if open {
            entry_child_links(day_contents, entry.line).len()
        } else {
            0
        };
        if open && let Some(name) = entry.name.as_deref() {
            for component in split_components(name) {
                let key = normalize_component(&component);
                if !key.is_empty() {
                    ledger_keys.push(key);
                }
            }
        }
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
    let Some(live_index) = live_index else {
        return Ok(None);
    };
    let exempt: Vec<String> = sources.exempt.iter().cloned().collect();
    let exempt_set: BTreeSet<String> = exempt
        .iter()
        .map(|value| normalize_component(value))
        .collect();
    let themes_before = ledger_keys
        .iter()
        .filter(|key| !exempt_set.contains(key.as_str()))
        .collect::<BTreeSet<_>>()
        .len();
    let outcome = routing::relocate(&routing::RelocateInput {
        entries: &entries,
        live_link: Some(live_index),
        type_key,
        type_name,
        sources,
        ledger_keys: &ledger_keys,
        themes_before,
        cap,
        strict,
    });
    match outcome {
        routing::RelocateOutcome::Unchanged => Ok(None),
        routing::RelocateOutcome::Refused { .. } => Err(StrictCapRefusal {
            name: type_name.to_string(),
            count: themes_before.saturating_add(1),
            cap,
        }),
        routing::RelocateOutcome::RenameEntry { index, new_name } => {
            let Some(entry) = scan.entries.get(index) else {
                return Ok(None);
            };
            let from = entry.name.clone();
            match rename_entry(day_contents, entry.line, &new_name) {
                Some(updated) => Ok(Some((
                    updated,
                    TypeRelocation {
                        outcome: "rename_entry",
                        from,
                        to: new_name,
                    },
                ))),
                None => Ok(None),
            }
        }
        routing::RelocateOutcome::RenamePlaceholder { index, new_name } => {
            let Some(entry) = scan.entries.get(index) else {
                return Ok(None);
            };
            let from = entry.name.clone();
            match rename_entry(day_contents, entry.line, &new_name) {
                Some(updated) => Ok(Some((
                    updated,
                    TypeRelocation {
                        outcome: "rename_placeholder",
                        from,
                        to: new_name,
                    },
                ))),
                None => Ok(None),
            }
        }
        routing::RelocateOutcome::AppendComponent { index, new_name } => {
            let Some(entry) = scan.entries.get(index) else {
                return Ok(None);
            };
            let from = entry.name.clone();
            match rename_entry(day_contents, entry.line, &new_name) {
                Some(updated) => Ok(Some((
                    updated,
                    TypeRelocation {
                        outcome: "append_component",
                        from,
                        to: new_name,
                    },
                ))),
                None => Ok(None),
            }
        }
        routing::RelocateOutcome::MoveTo { from, to } => {
            let Some(source_entry) = scan.entries.get(from) else {
                return Ok(None);
            };
            let Some(dest_entry) = scan.entries.get(to) else {
                return Ok(None);
            };
            let from_name = source_entry.name.clone();
            let to_name = dest_entry
                .name
                .clone()
                .unwrap_or_else(|| type_name.to_string());
            let Some(link) = find_live_link(day_contents, block_id) else {
                return Ok(None);
            };
            match move_subtree_to_entry(
                day_contents,
                link.line_index,
                link.subtree_end,
                dest_entry.line - 1,
            ) {
                Ok((updated, _)) => Ok(Some((
                    updated,
                    TypeRelocation {
                        outcome: "move_to",
                        from: from_name,
                        to: to_name,
                    },
                ))),
                Err(_) => Ok(None),
            }
        }
        routing::RelocateOutcome::CreateBelow { from, name } => {
            let Some(source_entry) = scan.entries.get(from) else {
                return Ok(None);
            };
            let from_name = source_entry.name.clone();
            let Some(link) = find_live_link(day_contents, block_id) else {
                return Ok(None);
            };
            match create_below_and_move(
                day_contents,
                source_entry.line,
                link.line_index,
                link.subtree_end,
                &name,
            ) {
                Some(updated) => Ok(Some((
                    updated,
                    TypeRelocation {
                        outcome: "create_below",
                        from: from_name,
                        to: name,
                    },
                ))),
                None => Ok(None),
            }
        }
    }
}

pub(crate) struct LiveLink {
    pub(crate) entry_line: usize,
    pub(crate) line_index: usize,
    pub(crate) subtree_end: usize,
}

pub(crate) fn find_live_link(
    day_contents: &str,
    block_id: &str,
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

    const CAP: u32 = 30;

    #[test]
    fn rename_running_entry() {
        let day = "## Pomodoros\n- [ ] (**0900-1000**) — BUILD\n  - [[sase#^fix-x]]\n";
        let (updated, info) = apply_to_day_contents(
            day,
            "fix-x",
            "fix",
            "FIX",
            &sources(),
            CAP,
            false,
        )
        .expect("relocate")
        .expect("changed");
        assert_eq!(info.outcome, "rename_entry");
        assert_eq!(info.from.as_deref(), Some("BUILD"));
        assert_eq!(info.to, "FIX");
        assert!(updated.contains("— FIX"));
        assert!(updated.contains("[[sase#^fix-x]]"));
    }

    #[test]
    fn append_to_running_entry() {
        let day = "## Pomodoros\n- [ ] (**0900-1000**) — BUILD\n  - [[sase#^fix-x]]\n  - [[sase#^other-x]]\n";
        let (updated, info) = apply_to_day_contents(
            day,
            "fix-x",
            "fix",
            "FIX",
            &sources(),
            CAP,
            false,
        )
        .expect("relocate")
        .expect("changed");
        assert_eq!(info.outcome, "append_component");
        assert_eq!(info.from.as_deref(), Some("BUILD"));
        assert_eq!(info.to, "BUILD + FIX");
        assert!(updated.contains("— BUILD + FIX"));
    }

    #[test]
    fn move_to_existing_entry() {
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-x]]\n- [ ] () — FIX\n";
        let (updated, info) = apply_to_day_contents(
            day,
            "fix-x",
            "fix",
            "FIX",
            &sources(),
            CAP,
            false,
        )
        .expect("relocate")
        .expect("changed");
        assert_eq!(info.outcome, "move_to");
        assert_eq!(info.to, "FIX");
        assert!(updated.contains("[[sase#^fix-x]]"));
    }

    #[test]
    fn rename_placeholder_entry() {
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-x]]\n";
        let (updated, info) = apply_to_day_contents(
            day,
            "fix-x",
            "fix",
            "FIX",
            &sources(),
            CAP,
            false,
        )
        .expect("relocate")
        .expect("changed");
        assert_eq!(info.outcome, "rename_placeholder");
        assert_eq!(info.from.as_deref(), Some("BUILD"));
        assert_eq!(info.to, "FIX");
        assert!(updated.contains("— FIX"));
    }

    #[test]
    fn create_below_entry() {
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-x]]\n  - [[sase#^other-x]]\n";
        let (updated, info) = apply_to_day_contents(
            day,
            "fix-x",
            "fix",
            "FIX",
            &sources(),
            CAP,
            false,
        )
        .expect("relocate")
        .expect("changed");
        assert_eq!(info.outcome, "create_below");
        assert_eq!(info.to, "FIX");
        assert!(updated.contains("[[sase#^fix-x]]"));
    }

    #[test]
    fn strict_cap_refuses() {
        let day = "## Pomodoros\n- [ ] () — BUILD\n  - [[sase#^fix-x]]\n  - [[sase#^other-x]]\n";
        let error = apply_to_day_contents(
            day,
            "fix-x",
            "fix",
            "FIX",
            &sources(),
            1,
            true,
        )
        .expect_err("strict-cap refuses");
        assert_eq!(error.name, "FIX");
        assert_eq!(error.cap, 1);
    }

    #[test]
    fn already_agreeing_needs_no_change() {
        let day = "## Pomodoros\n- [ ] () — FIX\n  - [[sase#^fix-x]]\n";
        let result = apply_to_day_contents(
            day,
            "fix-x",
            "fix",
            "FIX",
            &sources(),
            CAP,
            false,
        )
        .expect("relocate");
        assert!(result.is_none());
    }
}
