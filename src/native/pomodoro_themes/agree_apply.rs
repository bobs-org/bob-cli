#![allow(dead_code)]
//! Reusable ledger applier for Pomodoro name agreement.
//!
//! Both `bob task reconcile` and every bob writer run this over a day's text:
//! classify each chosen open entry's direct children, run the pure
//! [`super::agree::agree_entry`] planner, and rewrite only name tails. It
//! preserves CRLF line endings and a missing final newline, and it never
//! writes task fields or moves links.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use crate::native::{
    capture::{line_spans, list_item_body, nearest_shallower_list_item_parent},
    capture_pomodoros::{self, PomodoroState},
    config::{self, themes::ThemeSources},
    markdown, note_tasks, pomodoro,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    pomodoro_themes::{
        agree::{self, AgreeChild},
        relocation_apply,
    },
    task_dependencies,
    vault_links::LinkResolution,
};

/// How a link target resolves to a task type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskResolution {
    Typed {
        key: String,
        name: String,
        is_open: bool,
        has_repair: bool,
    },
    Untyped {
        is_open: bool,
    },
    Unresolved,
    Ambiguous,
}

/// Caller-provided task-type lookup over a snapshot.
///
/// Reconcile implements this over its staged post-images so a task this run
/// rewrote is typed from the new line; the capture planner implements it
/// over its staged files. The default [`DefaultTaskTypeLookup`] reads from
/// disk through one [`HomeResolver`] per run.
pub(crate) trait TaskTypeLookup {
    fn resolve(&mut self, target: &str, block_id: &str) -> TaskResolution;
}

/// Which entries the applier agrees.
#[derive(Debug, Clone)]
pub(crate) enum EntrySelector {
    AllOpen,
    Lines(BTreeSet<usize>),
}

/// One entry rename applied to the daily note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgreeRename {
    pub(crate) line: usize,
    pub(crate) before: String,
    pub(crate) after: String,
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
}

/// One item agreement reports but never writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgreeUnrepairedEntry {
    pub(crate) line: usize,
    pub(crate) entry: String,
    pub(crate) code: String,
    pub(crate) key: Option<String>,
    pub(crate) message: String,
}

/// Output of [`agree_ledger_entries`].
#[derive(Debug, Clone)]
pub(crate) struct AgreeApplyOutcome {
    pub(crate) contents: String,
    pub(crate) changed: bool,
    pub(crate) entries: Vec<AgreeRename>,
    pub(crate) unrepaired: Vec<AgreeUnrepairedEntry>,
}

/// Agree chosen entries in `day_contents`.
///
/// `daily_path` is the day file's vault-relative path (for empty-target link
/// resolution). `selector` chooses all open entries or a set of 1-based
/// entry lines. `lookup` classifies each link target. `sources` is the type
/// vocabulary. Only name tails change; every other byte stays identical.
pub(crate) fn agree_ledger_entries<L: TaskTypeLookup>(
    day_contents: &str,
    daily_path: &Path,
    selector: &EntrySelector,
    lookup: &mut L,
    sources: &ThemeSources,
) -> AgreeApplyOutcome {
    let scan = capture_pomodoros::scan(day_contents);
    if !scan.has_section {
        return AgreeApplyOutcome {
            contents: day_contents.to_string(),
            changed: false,
            entries: Vec::new(),
            unrepaired: Vec::new(),
        };
    }
    let spans = line_spans(day_contents);
    let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
    let section = pomodoro::pomodoros_section_range(&line_text);
    let fenced: BTreeSet<usize> = section
        .map(|range| markdown::fenced_lines(&line_text, range))
        .unwrap_or_default();

    let wanted: Option<BTreeSet<usize>> = match selector {
        EntrySelector::AllOpen => None,
        EntrySelector::Lines(lines) => Some(lines.clone()),
    };

    let mut contents = day_contents.to_string();
    let mut renames = Vec::new();
    let mut unrepaired_all = Vec::new();

    for entry in &scan.entries {
        if let Some(wanted) = &wanted
            && !wanted.contains(&entry.line)
        {
            continue;
        }
        let open = entry.state == PomodoroState::Open;
        if matches!(selector, EntrySelector::AllOpen) && !open {
            continue;
        }
        let children = direct_agree_children(
            &spans, entry.line, &fenced, &line_text, lookup, daily_path,
        );
        let outcome =
            agree::agree_entry(entry.name.as_deref(), open, &children, sources);
        for item in &outcome.unrepaired {
            unrepaired_all.push(AgreeUnrepairedEntry {
                line: entry.line,
                entry: entry.name.clone().unwrap_or_default(),
                code: item.code.clone(),
                key: item.key.clone(),
                message: unrepaired_message(item, entry),
            });
        }
        let before = entry.name.clone().unwrap_or_default();
        if outcome.name == before
            && outcome.added.is_empty()
            && outcome.removed.is_empty()
        {
            continue;
        }
        if entry.name.is_none() {
            continue;
        }
        let Some(updated) = relocation_apply::rename_entry(
            &contents,
            entry.line,
            &outcome.name,
        ) else {
            continue;
        };
        contents = updated;
        renames.push(AgreeRename {
            line: entry.line,
            before,
            after: outcome.name,
            added: outcome.added,
            removed: outcome.removed,
        });
    }

    let changed = !renames.is_empty();
    AgreeApplyOutcome {
        contents,
        changed,
        entries: renames,
        unrepaired: unrepaired_all,
    }
}

fn unrepaired_message(
    item: &agree::AgreeUnrepaired,
    entry: &capture_pomodoros::PomodoroEntry,
) -> String {
    let entry_name = entry.name.as_deref().unwrap_or("");
    match item.code.as_str() {
        "meet_mismatch" => {
            let key = item.key.as_deref().unwrap_or("task");
            format!("{key} task under MEET — {entry_name}")
        }
        "unconfigured_type" => {
            let key = item.key.as_deref().unwrap_or("type");
            format!("{key} isn't one of your types")
        }
        _ => "unresolved link".to_string(),
    }
}

fn direct_agree_children<L: TaskTypeLookup>(
    spans: &[crate::native::capture::LineSpan<'_>],
    entry_line: usize,
    fenced: &BTreeSet<usize>,
    line_text: &[&str],
    lookup: &mut L,
    _daily_path: &Path,
) -> Vec<AgreeChild> {
    let entry_index = entry_line.saturating_sub(1);
    if entry_index >= spans.len() {
        return Vec::new();
    }
    let end = crate::native::capture_task_toggle::child_block_end_line(
        spans,
        entry_index,
    );
    let mut children = Vec::new();
    for line_index in entry_index + 1..=end.min(spans.len().saturating_sub(1)) {
        if fenced.contains(&line_index) {
            continue;
        }
        if nearest_shallower_list_item_parent(spans, line_index)
            != Some(entry_index)
        {
            continue;
        }
        let text = spans[line_index].text;
        let _ = line_text;
        children.push(classify_child_line(text, lookup));
    }
    children
}

fn classify_child_line<L: TaskTypeLookup>(
    line: &str,
    lookup: &mut L,
) -> AgreeChild {
    let Some(body) = list_item_body(line).map(str::trim) else {
        return AgreeChild::Prose;
    };
    let Some((target, block_id, struck, embedded, tomato)) =
        parse_dedicated_link(line, body)
    else {
        return AgreeChild::Prose;
    };
    match lookup.resolve(&target, &block_id) {
        TaskResolution::Unresolved | TaskResolution::Ambiguous => {
            AgreeChild::Unknown
        }
        TaskResolution::Untyped { .. } => AgreeChild::Unknown,
        TaskResolution::Typed {
            key,
            name,
            is_open,
            has_repair,
        } => {
            if has_repair {
                return AgreeChild::Unknown;
            }
            if key.is_empty() {
                return AgreeChild::Unknown;
            }
            if struck || embedded || tomato {
                AgreeChild::Worked { key, name }
            } else if !is_open {
                AgreeChild::Unknown
            } else {
                AgreeChild::Live { key, name }
            }
        }
    }
}

fn parse_dedicated_link(
    line: &str,
    body: &str,
) -> Option<(String, String, bool, bool, bool)> {
    let tomato = body.contains('🍅');
    let without_tomato = body.replace('🍅', "").trim().to_string();
    if without_tomato.starts_with('~') {
        return None;
    }
    let without_strike = without_tomato.replace("~~", "").trim().to_string();
    let cleaned = without_strike.trim();
    if cleaned.is_empty() {
        return None;
    }
    let links = task_dependencies::block_link_spans(line);
    if links.len() != 1 {
        return None;
    }
    let span = &links[0];
    let struck = span.struck;
    let (target, block_id, embedded) =
        if let Some(rest) = cleaned.strip_prefix('!') {
            let rest = rest.trim();
            let inner = rest
                .strip_prefix("[[")
                .and_then(|rest| rest.strip_suffix('#'))
                .or_else(|| rest.strip_prefix("[["))?;
            let inner = inner.strip_suffix("]]")?;
            let (target, block_id) =
                task_dependencies::parse_block_link_inside(inner)?;
            (target, block_id, true)
        } else {
            let inner = cleaned.strip_prefix("[[").and_then(|rest| {
                rest.strip_suffix("]]#").or_else(|| rest.strip_suffix("]]"))
            })?;
            let (target, block_id) =
                task_dependencies::parse_block_link_inside(inner)?;
            (target, block_id, false)
        };
    if target.is_empty() && block_id.is_empty() {
        return None;
    }
    if span.block_id != block_id {
        return None;
    }
    Some((target, block_id, struck, embedded, tomato))
}

/// Default disk-backed lookup over one [`HomeResolver`] per run.
///
/// Tasks are typed as if open so worked links keep their pre-close types.
/// When the catalog, config, or vault read is unavailable the lookup
/// resolves everything as unresolved, which blocks drops but never writes.
pub(crate) struct DefaultTaskTypeLookup {
    vault: PathBuf,
    daily_path: PathBuf,
    daily_contents: String,
    catalog: Option<crate::native::pomodoro_themes::Catalog>,
    config: config::PlanConfig,
    resolver: HomeResolver<DiskHomeSource>,
    settings: note_tasks::NoteTaskSettings,
}

impl DefaultTaskTypeLookup {
    pub(crate) fn new(
        vault: &Path,
        daily_path: &Path,
        daily_contents: &str,
    ) -> Self {
        let today = daily_path
            .to_string_lossy()
            .rsplit('/')
            .next()
            .and_then(|file| file.strip_suffix(".md"))
            .and_then(|stem| {
                chrono::NaiveDate::parse_from_str(stem, "%Y%m%d").ok()
            })
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let catalog =
            crate::native::pomodoro_themes::Catalog::load(vault, today, false)
                .ok();
        let config = config::load_plan_config(&config::config_path())
            .unwrap_or_default();
        let settings = note_tasks::read_settings(vault);
        Self {
            vault: vault.to_path_buf(),
            daily_path: daily_path.to_path_buf(),
            daily_contents: daily_contents.to_string(),
            catalog,
            config,
            resolver: HomeResolver::new(DiskHomeSource::new(vault)),
            settings,
        }
    }
}

impl TaskTypeLookup for DefaultTaskTypeLookup {
    fn resolve(&mut self, target: &str, block_id: &str) -> TaskResolution {
        let Some(catalog) = self.catalog.as_ref() else {
            return TaskResolution::Unresolved;
        };
        let residence: PathBuf = if target.is_empty() {
            self.daily_path.clone()
        } else {
            match self.resolver.resolve_target(&self.daily_path, target) {
                LinkResolution::Found(path) => path,
                LinkResolution::Ambiguous => {
                    return TaskResolution::Ambiguous;
                }
                LinkResolution::Missing => {
                    return TaskResolution::Unresolved;
                }
            }
        };
        let contents = if residence == self.daily_path {
            self.daily_contents.clone()
        } else {
            match std::fs::read_to_string(self.vault.join(&residence)) {
                Ok(contents) => contents,
                Err(_) => return TaskResolution::Unresolved,
            }
        };
        let scan = note_tasks::scan(&contents, &self.settings);
        let found = match scan.by_block_id(block_id) {
            note_tasks::BlockIdLookup::Found(task) => task,
            note_tasks::BlockIdLookup::Duplicate(_) => {
                return TaskResolution::Ambiguous;
            }
            note_tasks::BlockIdLookup::NotATask { .. }
            | note_tasks::BlockIdLookup::Missing => {
                return TaskResolution::Unresolved;
            }
        };
        let Some(line) = contents.lines().nth(found.line_index) else {
            return TaskResolution::Unresolved;
        };
        let is_open = !is_closed_line(line);
        let result =
            crate::native::pomodoro_themes::suggest::suggest_for_task_as_if_open(
                &mut self.resolver,
                catalog,
                &self.config,
                &residence,
                line,
            );
        let has_repair = result
            .suggestion
            .as_ref()
            .is_some_and(|suggestion| suggestion.repair.is_some());
        match result.suggestion {
            None => TaskResolution::Untyped { is_open },
            Some(suggestion) => TaskResolution::Typed {
                key: suggestion.theme.key,
                name: suggestion.theme.name,
                is_open,
                has_repair,
            },
        }
    }
}

fn is_closed_line(line: &str) -> bool {
    use crate::native::ref_tasks::{strip_blockquote_prefix, task_mark};
    let stripped = strip_blockquote_prefix(line);
    matches!(task_mark(stripped), Some('x' | 'X' | '-'))
}

/// In-memory lookup for tests: `(target, block_id)` to type.
#[cfg(test)]
pub(crate) struct MapLookup {
    pub(crate) entries:
        std::collections::BTreeMap<(String, String), TaskResolution>,
}

#[cfg(test)]
impl TaskTypeLookup for MapLookup {
    fn resolve(&mut self, target: &str, block_id: &str) -> TaskResolution {
        self.entries
            .get(&(target.to_string(), block_id.to_string()))
            .cloned()
            .unwrap_or(TaskResolution::Unresolved)
    }
}

#[cfg(test)]
pub(crate) fn typed(key: &str, name: &str, is_open: bool) -> TaskResolution {
    TaskResolution::Typed {
        key: key.to_string(),
        name: name.to_string(),
        is_open,
        has_repair: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::config::themes::{ConfiguredTheme, ThemesConfig};

    fn sources() -> ThemeSources {
        ThemeSources {
            exempt: ["plan".to_string()].into_iter().collect(),
            inventory: ["later".to_string()].into_iter().collect(),
            themes: Some(ThemesConfig {
                entries: ["BUILD", "FIX", "WRITE", "MEET"]
                    .iter()
                    .map(|name| ConfiguredTheme {
                        key: name.to_lowercase(),
                        name: (*name).to_string(),
                        color: None,
                    })
                    .collect(),
            }),
            warning: None,
        }
    }

    fn lookup(pairs: &[((&str, &str), TaskResolution)]) -> MapLookup {
        MapLookup {
            entries: pairs
                .iter()
                .map(|((target, block), resolution)| {
                    (
                        ((*target).to_string(), (*block).to_string()),
                        resolution.clone(),
                    )
                })
                .collect(),
        }
    }

    fn daily_path() -> PathBuf {
        PathBuf::from("2026/20261105.md")
    }

    #[test]
    fn agrees_single_entry_add_and_drop() {
        let day = "## Pomodoros\n- [ ] () — FIX\n  - [[sase#^aaa111]]\n";
        let mut look =
            lookup(&[(("sase", "aaa111"), typed("build", "BUILD", true))]);
        let out = agree_ledger_entries(
            day,
            &daily_path(),
            &EntrySelector::AllOpen,
            &mut look,
            &sources(),
        );
        assert!(out.changed);
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].before, "FIX");
        assert_eq!(out.entries[0].after, "BUILD");
        assert!(out.contents.contains("— BUILD"));
        assert!(!out.contents.contains("— FIX"));
        let second = agree_ledger_entries(
            &out.contents,
            &daily_path(),
            &EntrySelector::AllOpen,
            &mut lookup(&[(("sase", "aaa111"), typed("build", "BUILD", true))]),
            &sources(),
        );
        assert!(!second.changed);
        assert_eq!(second.contents, out.contents);
    }

    #[test]
    fn unknown_and_prose_protect_types() {
        let day = "## Pomodoros\n- [ ] () — BUILD + FIX\n  - [[sase#^aaa111]]\n  - draft post\n";
        let mut look =
            lookup(&[(("sase", "aaa111"), typed("build", "BUILD", true))]);
        let out = agree_ledger_entries(
            day,
            &daily_path(),
            &EntrySelector::AllOpen,
            &mut look,
            &sources(),
        );
        assert!(!out.changed);
        assert!(out.contents.contains("— BUILD + FIX"));
    }

    #[test]
    fn worked_supports_and_meet_reports() {
        let day = "## Pomodoros\n- [ ] () — MEET\n  - [[sase#^aaa111]]\n";
        let mut look =
            lookup(&[(("sase", "aaa111"), typed("write", "WRITE", true))]);
        let out = agree_ledger_entries(
            day,
            &daily_path(),
            &EntrySelector::AllOpen,
            &mut look,
            &sources(),
        );
        assert!(!out.changed);
        assert_eq!(out.unrepaired.len(), 1);
        assert_eq!(out.unrepaired[0].code, "meet_mismatch");
    }

    #[test]
    fn preserves_crlf_and_missing_final_newline() {
        let day = "## Pomodoros\r\n- [ ] () — FIX\r\n  - [[sase#^aaa111]]";
        let mut look =
            lookup(&[(("sase", "aaa111"), typed("build", "BUILD", true))]);
        let out = agree_ledger_entries(
            day,
            &daily_path(),
            &EntrySelector::AllOpen,
            &mut look,
            &sources(),
        );
        assert!(out.changed);
        assert!(out.contents.contains("\r\n"));
        assert!(!out.contents.ends_with('\n'));
        assert!(out.contents.contains("— BUILD"));
    }

    #[test]
    fn selector_limits_to_chosen_lines() {
        let day = "## Pomodoros\n- [ ] () — FIX\n  - [[sase#^aaa111]]\n- [ ] () — FIX\n  - [[sase#^bbb222]]\n";
        let mut look = lookup(&[
            (("sase", "aaa111"), typed("build", "BUILD", true)),
            (("sase", "bbb222"), typed("build", "BUILD", true)),
        ]);
        let mut lines = BTreeSet::new();
        lines.insert(2usize);
        let out = agree_ledger_entries(
            day,
            &daily_path(),
            &EntrySelector::Lines(lines),
            &mut look,
            &sources(),
        );
        assert!(out.changed);
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].line, 2);
        assert!(out.contents.contains("— BUILD\n  - [[sase#^aaa111]]"));
        assert!(out.contents.contains("— FIX\n  - [[sase#^bbb222]]"));
    }
}
