//! Shared plan-budget core: the `plan:` config definition, a pure
//! ledger budget and lint engine, the ledger-derived Today engine,
//! and the NEXT/PENDING lane counters built on the native Tasks
//! engine. `docs/plan.md` is the authoritative definition; this
//! module is its Rust implementation, shared by `bob plan`, the
//! task-status hooks, and capture.

pub(crate) mod cli;
pub(crate) mod today;

pub(crate) use today::{TodayResult, TodayTask};

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chrono::NaiveDate;
use serde::Serialize;

use super::{
    capture_block_ids, capture_pomodoros, config::PlanConfig, dataview,
    markdown, pomodoro,
};

/// Lint codes emitted by [`compute`]. They appear verbatim in human
/// output and JSON so every surface can match on them.
pub(crate) const LINT_THEME_CAP: &str = "plan_theme_cap_exceeded";
pub(crate) const LINT_LINK_CAP: &str = "plan_link_cap_exceeded";
pub(crate) const LINT_DUPLICATE_NAME: &str = "duplicate_open_pomodoro_name";
pub(crate) const LINT_INVENTORY_LABEL: &str = "inventory_label_open";
pub(crate) const LINT_SUBHEADING: &str = "subheading_in_pomodoros";
pub(crate) const LINT_NEXT_CAP: &str = "next_cap_exceeded";
pub(crate) const LINT_PENDING_CAP: &str = "pending_cap_exceeded";

/// Display name used when an unnamed open entry holds counted links.
pub(crate) const UNNAMED_THEME: &str = "(unnamed)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PlanStatus {
    Ok,
    Over,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanMeter {
    pub(crate) count: usize,
    pub(crate) cap: u32,
    pub(crate) over: bool,
}

/// One lane meter: the whole NEXT or PENDING lane, Today
/// included, so counts never swing during the day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LaneBudget {
    pub(crate) count: usize,
    pub(crate) cap: u32,
    pub(crate) over: bool,
}

/// Both lane meters together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Lanes {
    pub(crate) next: LaneBudget,
    pub(crate) pending: LaneBudget,
}

/// Today's dedicated-task count: the number of [`TodayTask`] rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TodayBudget {
    pub(crate) count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanCaps {
    pub(crate) max_themes: u32,
    pub(crate) max_links: u32,
    pub(crate) max_next: u32,
    pub(crate) max_pending: u32,
    pub(crate) max_ready: u32,
    pub(crate) strict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanEntry {
    pub(crate) line: usize,
    pub(crate) name: String,
    pub(crate) components: Vec<String>,
    pub(crate) exempt: bool,
    pub(crate) running: bool,
    pub(crate) highlight: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) time_range: Option<String>,
    pub(crate) links: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanLint {
    pub(crate) code: String,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<usize>,
}

/// The pure ledger half of a plan report: everything derived from the
/// daily note contents without touching the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerBudget {
    pub(crate) has_section: bool,
    pub(crate) themes: PlanMeter,
    pub(crate) links: PlanMeter,
    pub(crate) status: PlanStatus,
    pub(crate) theme_names: Vec<String>,
    pub(crate) entries: Vec<PlanEntry>,
    pub(crate) warnings: Vec<PlanLint>,
}

/// The full report shared by `bob plan` and the hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanReport {
    pub(crate) date: String,
    pub(crate) daily_file: String,
    pub(crate) caps: PlanCaps,
    pub(crate) status: PlanStatus,
    pub(crate) themes: PlanMeter,
    pub(crate) links: PlanMeter,
    pub(crate) today: TodayBudget,
    pub(crate) next: LaneBudget,
    pub(crate) pending: LaneBudget,
    pub(crate) today_tasks: Vec<TodayTask>,
    pub(crate) theme_names: Vec<String>,
    pub(crate) entries: Vec<PlanEntry>,
    pub(crate) warnings: Vec<PlanLint>,
}

impl PlanReport {
    pub(crate) fn caps_of(config: &PlanConfig) -> PlanCaps {
        PlanCaps {
            max_themes: config.max_themes(),
            max_links: config.max_links(),
            max_next: config.max_next(),
            max_pending: config.max_pending(),
            max_ready: config.max_ready(),
            strict: config.strict(),
        }
    }
}

/// The full report shared by `bob plan` and the hooks: the pure
/// ledger half plus the Today rows and lane meters. Lane over-cap
/// appends `next_cap_exceeded` / `pending_cap_exceeded` but never
/// changes `status`: status depends only on themes and links (rule
/// 8). Being exactly at a cap is fine. Nothing is ever refused.
pub(crate) fn assemble_report(
    today: NaiveDate,
    daily_file: &str,
    config: &PlanConfig,
    ledger: &LedgerBudget,
    lanes: &Lanes,
    today_result: &TodayResult,
) -> PlanReport {
    let mut warnings = ledger.warnings.clone();
    warnings.extend(today_result.warnings.iter().cloned());
    if lanes.next.over {
        warnings.push(PlanLint {
            code: LINT_NEXT_CAP.to_string(),
            message: format!(
                "NEXT has {}/{cap} tasks; release some with Alt+N",
                lanes.next.count,
                cap = lanes.next.cap
            ),
            line: None,
        });
    }
    if lanes.pending.over {
        warnings.push(PlanLint {
            code: LINT_PENDING_CAP.to_string(),
            message: format!(
                "PENDING has {}/{cap} tasks; release some with Alt+N",
                lanes.pending.count,
                cap = lanes.pending.cap
            ),
            line: None,
        });
    }
    let status = ledger.status;
    PlanReport {
        date: today.format("%Y-%m-%d").to_string(),
        daily_file: daily_file.to_string(),
        caps: PlanReport::caps_of(config),
        status,
        themes: ledger.themes.clone(),
        links: ledger.links.clone(),
        today: TodayBudget {
            count: today_result.tasks.len(),
        },
        next: lanes.next.clone(),
        pending: lanes.pending.clone(),
        today_tasks: today_result.tasks.clone(),
        theme_names: ledger.theme_names.clone(),
        entries: ledger.entries.clone(),
        warnings,
    }
}

/// Case-insensitive component key: collapsed whitespace, lowercased.
pub(crate) fn normalize_component(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Count the NEXT and PENDING lanes through the native Tasks
/// engine, so the result honors the vault's Tasks settings. Each
/// counts the whole lane, Today included. A scan failure reads as
/// zero: the vault was already proven readable by the daily-note
/// read, and the query text itself is covered by tests.
pub(crate) fn count_lanes(
    bob_dir: &Path,
    today: NaiveDate,
    config: &PlanConfig,
) -> Lanes {
    let now = today
        .and_hms_opt(12, 0, 0)
        .unwrap_or(chrono::NaiveDateTime::MIN);
    Lanes {
        next: count_lane(bob_dir, dataview::NEXT_QUERY, now, config.max_next()),
        pending: count_lane(
            bob_dir,
            dataview::PENDING_QUERY,
            now,
            config.max_pending(),
        ),
    }
}

fn count_lane(
    bob_dir: &Path,
    query: &str,
    now: chrono::NaiveDateTime,
    cap: u32,
) -> LaneBudget {
    let count = dataview::query_matching_descriptions(bob_dir, query, now)
        .map(|descriptions| descriptions.len())
        .unwrap_or(0);
    let over = u64::try_from(count).unwrap_or(u64::MAX) > u64::from(cap);
    LaneBudget { count, cap, over }
}

/// Pure ledger budget and lint engine implementing `docs/plan.md`
/// rules 1-9. Only open entries count; completed and cancelled
/// entries are history.
///
/// `daily_file` is the daily note's vault-relative path without `.md`
/// (for example `2026/20260930`); an empty link target, that path,
/// and its basename all mean the daily note itself (rule 6). Callers
/// that know the day file must pass it; `compute` keeps the old
/// behaviour for callers that do not.
pub(crate) fn compute_for_daily(
    contents: &str,
    config: &PlanConfig,
    daily_file: Option<&str>,
) -> LedgerBudget {
    let lines: Vec<&str> = contents.lines().collect();
    let Some(section) = pomodoro::pomodoros_section_range(&lines) else {
        return empty_budget(config);
    };
    let fenced = markdown::fenced_lines(&lines, section.clone());
    let scan = capture_pomodoros::scan(contents);

    let mut warnings = Vec::new();
    for index in section.clone() {
        if fenced.contains(&index) {
            continue;
        }
        if let Some((level, _)) = markdown::atx_heading(lines[index])
            && level >= 3
        {
            warnings.push(PlanLint {
                code: LINT_SUBHEADING.to_string(),
                message: "subheading inside the Pomodoros section splits \
                    time totals"
                    .to_string(),
                line: Some(index + 1),
            });
        }
    }

    let exempt: BTreeSet<String> = config
        .exempt()
        .iter()
        .map(|value| normalize_component(value))
        .collect();
    let inventory: BTreeSet<String> = config
        .inventory_labels()
        .iter()
        .map(|value| normalize_component(value))
        .collect();

    // Entry spans: an entry owns the lines after it up to the next
    // column-0 entry or the section end. Column-0 non-entry lines
    // (subheadings, prose) close the span, so links under them are
    // never attributed to an entry.
    let entry_lines: BTreeSet<usize> =
        scan.entries.iter().map(|entry| entry.line).collect();
    let mut entries = Vec::new();
    let mut theme_names = Vec::new();
    let mut theme_keys: BTreeSet<String> = BTreeSet::new();
    let mut seen_themes: BTreeMap<String, usize> = BTreeMap::new();
    let mut inventory_warned: BTreeSet<String> = BTreeSet::new();
    let mut seen_links: BTreeSet<(String, String)> = BTreeSet::new();
    let mut unnamed_counted = false;

    for entry in &scan.entries {
        if entry.state != capture_pomodoros::PomodoroState::Open {
            continue;
        }
        let components = entry
            .name
            .as_deref()
            .map(split_components)
            .unwrap_or_default();
        let keys = components
            .iter()
            .map(|component| normalize_component(component))
            .collect::<Vec<_>>();
        let exempt_entry =
            !keys.is_empty() && keys.iter().all(|key| exempt.contains(key));

        let entry_links = collect_entry_links(
            &lines,
            section.clone(),
            &fenced,
            &entry_lines,
            entry.line,
            exempt_entry,
            daily_file,
        );
        if components.is_empty() && entry_links.is_empty() {
            // An empty `()` placeholder never counts.
            continue;
        }

        let mut distinct_links = BTreeSet::new();
        if !exempt_entry {
            for link in &entry_links {
                distinct_links.insert(link.clone());
                seen_links.insert(link.clone());
            }
        }

        // Duplicate-name lint: the same non-exempt component on two
        // open entries, reported at the later entry.
        for (component, key) in components.iter().zip(keys.iter()) {
            if exempt.contains(key) {
                continue;
            }
            if let Some(first) = seen_themes.get(key) {
                warnings.push(PlanLint {
                    code: LINT_DUPLICATE_NAME.to_string(),
                    message: format!(
                        "{component} is open in more than one Pomodoro \
                        (lines {first} and {})",
                        entry.line
                    ),
                    line: Some(entry.line),
                });
            } else {
                seen_themes.insert(key.clone(), entry.line);
            }
            if inventory.contains(key) && !inventory_warned.contains(key) {
                inventory_warned.insert(key.clone());
                warnings.push(PlanLint {
                    code: LINT_INVENTORY_LABEL.to_string(),
                    message: format!(
                        "{component} is an inventory label, not a theme"
                    ),
                    line: Some(entry.line),
                });
            }
        }

        if components.is_empty() && !unnamed_counted {
            unnamed_counted = true;
            theme_keys.insert(UNNAMED_THEME.to_string());
            theme_names.push(UNNAMED_THEME.to_string());
        }
        for (component, key) in components.iter().zip(keys.iter()) {
            if exempt.contains(key) || !theme_keys.insert(key.clone()) {
                continue;
            }
            theme_names.push((*component).clone());
        }

        entries.push(PlanEntry {
            line: entry.line,
            name: entry
                .name
                .clone()
                .unwrap_or_else(|| UNNAMED_THEME.to_string()),
            components,
            exempt: exempt_entry,
            running: entry.is_current,
            highlight: false,
            time_range: entry.time_range.clone(),
            links: distinct_links.len(),
        });
    }

    if let Some(first) = entries.iter().position(|entry| !entry.exempt) {
        entries[first].highlight = true;
    }

    let themes = meter(theme_names.len(), config.max_themes());
    let links = meter(seen_links.len(), config.max_links());
    let status = if themes.over || links.over {
        PlanStatus::Over
    } else {
        PlanStatus::Ok
    };
    if themes.over {
        warnings.push(PlanLint {
            code: LINT_THEME_CAP.to_string(),
            message: format!(
                "today's plan has {}/{cap} themes",
                themes.count,
                cap = themes.cap
            ),
            line: None,
        });
    }
    if links.over {
        warnings.push(PlanLint {
            code: LINT_LINK_CAP.to_string(),
            message: format!(
                "today's plan has {}/{cap} links",
                links.count,
                cap = links.cap
            ),
            line: None,
        });
    }

    LedgerBudget {
        has_section: true,
        themes,
        links,
        status,
        theme_names,
        entries,
        warnings,
    }
}

fn empty_budget(config: &PlanConfig) -> LedgerBudget {
    LedgerBudget {
        has_section: false,
        themes: meter(0, config.max_themes()),
        links: meter(0, config.max_links()),
        status: PlanStatus::Ok,
        theme_names: Vec::new(),
        entries: Vec::new(),
        warnings: Vec::new(),
    }
}

fn meter(count: usize, cap: u32) -> PlanMeter {
    let over = u64::try_from(count).unwrap_or(u64::MAX) > u64::from(cap);
    PlanMeter { count, cap, over }
}

/// Split a merged entry name (`BOB + DECKS`) into components.
pub(crate) fn split_components(name: &str) -> Vec<String> {
    name.split('+')
        .map(str::trim)
        .filter(|component| !component.is_empty())
        .map(str::to_string)
        .collect()
}

/// Distinct `(target, block_id)` pairs among the descendant bullets
/// of one open entry. Exempt entries contribute nothing. Struck
/// links and fenced code never count.
fn collect_entry_links(
    lines: &[&str],
    section: std::ops::Range<usize>,
    fenced: &BTreeSet<usize>,
    entry_lines: &BTreeSet<usize>,
    entry_line: usize,
    exempt_entry: bool,
    daily_file: Option<&str>,
) -> Vec<(String, String)> {
    if exempt_entry {
        return Vec::new();
    }
    // `entry_line` is 1-based, so the first descendant sits at slice
    // offset 0. Only indented descendant bullets belong to the entry:
    // any column-0 line (the next entry, a subheading, prose) closes
    // the span.
    let mut links = Vec::new();
    for (offset, line) in lines[entry_line..section.end].iter().enumerate() {
        let index = entry_line + offset;
        if entry_lines.contains(&(index + 1)) {
            break;
        }
        if fenced.contains(&index) {
            continue;
        }
        if !line.is_empty() && !line.starts_with([' ', '\t']) {
            break;
        }
        for link in block_links(line, daily_file) {
            links.push(link);
        }
    }
    links
}

/// Block links `[[target#^id]]` (also `![[…]]` and `[[…|alias]]`,
/// with or without a trailing `#` move-only marker) outside
/// `~~…~~` struck spans.
///
/// An empty target, the daily note's vault-relative path without
/// `.md`, and its basename all canonicalize to the daily path
/// itself (rule 6) when `daily_file` is known.
fn block_links(line: &str, daily_file: Option<&str>) -> Vec<(String, String)> {
    let struck = struck_inner_spans(line);
    let mut links = Vec::new();
    let mut rest = line;
    let mut base = 0;
    while let Some(open) = rest.find("[[") {
        let absolute_open = base + open;
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("]]") else {
            break;
        };
        let mut inside = &after_open[..close];
        let mut link_end = absolute_open + 2 + close + 2;
        // A trailing `#` move-only marker, inside or outside.
        let outside_hash = rest[open + 2 + close + 2..].starts_with('#');
        if outside_hash {
            link_end += 1;
        }
        inside = inside.strip_suffix('#').unwrap_or(inside);
        let target = inside.split('|').next().unwrap_or("");
        if let Some((target, block_id)) = target.split_once("#^") {
            let block_id = block_id.trim();
            if !block_id.is_empty()
                && block_id.bytes().all(capture_block_ids::is_block_id_byte)
                && !struck.iter().any(|(start, end)| {
                    absolute_open >= *start && link_end <= *end
                })
            {
                let mut target = target.trim().to_string();
                if let Some(stripped) = target.strip_suffix(".md") {
                    target = stripped.to_string();
                }
                let target = canonical_link_target(&target, daily_file);
                links.push((target, block_id.to_string()));
            }
        }
        base = link_end;
        rest = &line[base..];
    }
    links
}

/// Vault-relative daily key (`YYYY/YYYYMMDD`) for a day-file path.
/// Returns the parent directory plus the file stem, so an absolute
/// path still yields the vault-relative key the ledger links use.
pub(crate) fn daily_key_from_path(day_file: &Path) -> Option<String> {
    let stem = day_file.file_stem()?.to_str()?;
    let parent = day_file.parent()?.file_name()?.to_str()?;
    Some(format!("{parent}/{stem}"))
}

/// Canonical daily-note link target (rule 6): an empty target, the
/// daily note's vault-relative path without `.md`, and its basename
/// are the same key. A `.md` suffix is stripped before comparing,
/// and the canonical key is the daily path itself.
fn canonical_link_target(target: &str, daily_file: Option<&str>) -> String {
    let Some(daily) = daily_file else {
        return target.to_string();
    };
    let daily = daily.strip_suffix(".md").unwrap_or(daily);
    if daily.is_empty() {
        return target.to_string();
    }
    let basename = daily.rsplit('/').next().unwrap_or(daily);
    if target.is_empty() || target == daily || target == basename {
        daily.to_string()
    } else {
        target.to_string()
    }
}

/// Inner spans of `~~…~~` struck pairs on one line: from just
/// after the opening marks to just before the closing marks.
fn struck_inner_spans(line: &str) -> Vec<(usize, usize)> {
    let mut marks = Vec::new();
    let mut base = 0;
    let mut rest = line;
    while let Some(found) = rest.find("~~") {
        marks.push(base + found);
        base += found + 2;
        rest = &line[base..];
    }
    marks
        .chunks(2)
        .filter(|pair| pair.len() == 2)
        .map(|pair| (pair[0] + 2, pair[1]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(contents: &str) -> LedgerBudget {
        compute_for_daily(contents, &PlanConfig::default(), None)
    }

    fn codes(budget: &LedgerBudget) -> Vec<&str> {
        budget
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect()
    }

    #[test]
    fn merged_name_counts_decks_once_with_duplicate_lint() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — BOB + DECKS\n\
            \x20   - [[a#^one]]\n\
            - [ ] () — DECKS\n\
            \x20   - [[b#^two]]\n",
        );
        assert!(ledger.has_section);
        assert_eq!(ledger.theme_names, vec!["BOB", "DECKS"]);
        assert_eq!(ledger.themes.count, 2);
        assert!(!ledger.themes.over);
        assert_eq!(ledger.links.count, 2);
        assert_eq!(ledger.status, PlanStatus::Ok);
        assert_eq!(codes(&ledger), vec![LINT_DUPLICATE_NAME]);
        assert_eq!(ledger.warnings[0].line, Some(5));
        assert!(ledger.entries[0].highlight);
        assert!(!ledger.entries[1].highlight);
    }

    #[test]
    fn link_forms_count_once_and_exclusions_hold() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - [[task#^aaa]] and ~~[[task#^struck]]~~\n\
            \x20   - ![[emb#^eee]]\n\
            \x20   - [[mark#^mmm]]#\n\
            - [ ] () — DECKS\n\
            \x20   - [[task#^aaa]]\n\
            - [ ] () — GTD\n\
            \x20   - [[#^gtd]]\n",
        );
        assert_eq!(ledger.theme_names, vec!["GOALS", "DECKS"]);
        assert_eq!(ledger.links.count, 3);
        assert!(ledger.warnings.is_empty());
        let gtd = ledger
            .entries
            .iter()
            .find(|entry| entry.name == "GTD")
            .expect("GTD row");
        assert!(gtd.exempt);
        assert_eq!(gtd.links, 0);
        assert_eq!(ledger.entries[0].links, 3);
    }

    #[test]
    fn unnamed_placeholder_counts_only_with_links() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] ()\n\
            \x20   - [[solo#^one]]\n\
            - [ ] ()\n\
            - [ ] () — GOALS\n",
        );
        assert_eq!(
            ledger.theme_names,
            vec![UNNAMED_THEME.to_string(), "GOALS".to_string()]
        );
        assert_eq!(ledger.themes.count, 2);
        assert_eq!(ledger.links.count, 1);
        assert_eq!(ledger.entries.len(), 2);
        assert!(ledger.entries[0].highlight);
    }

    #[test]
    fn subheading_splits_time_totals_and_lints() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - [[task#^aaa]]\n\
            \n\
            ### Notes\n\
            \n\
            - [[task#^bbb]] is just prose\n",
        );
        assert_eq!(ledger.links.count, 1);
        assert_eq!(codes(&ledger), vec![LINT_SUBHEADING]);
        assert_eq!(ledger.warnings[0].line, Some(6));
    }

    #[test]
    fn open_inventory_label_counts_and_lints() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — LATER\n\
            \x20   - [[task#^aaa]]\n",
        );
        assert_eq!(ledger.theme_names, vec!["LATER"]);
        assert_eq!(ledger.themes.count, 1);
        assert_eq!(codes(&ledger), vec![LINT_INVENTORY_LABEL]);
        assert_eq!(ledger.warnings[0].line, Some(3));
    }

    #[test]
    fn four_themes_are_over_but_three_are_fine() {
        let over = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — ONE\n\
            - [ ] () — TWO\n\
            - [ ] () — THREE\n\
            - [ ] () — FOUR\n",
        );
        assert_eq!(over.themes.count, 4);
        assert!(over.themes.over);
        assert_eq!(over.status, PlanStatus::Over);
        assert_eq!(codes(&over), vec![LINT_THEME_CAP]);
        assert_eq!(over.warnings[0].line, None);

        let fine = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — ONE\n\
            - [ ] () — TWO\n\
            - [ ] () — THREE\n",
        );
        assert_eq!(fine.status, PlanStatus::Ok);
        assert!(fine.warnings.is_empty());
    }

    #[test]
    fn cancelled_and_completed_entries_never_count() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [-] () — GONE\n\
            \x20   - [[task#^aaa]]\n\
            - [x] () — DONE\n\
            \x20   - [[task#^bbb]]\n\
            - [ ] () — GOALS\n\
            \x20   - [[task#^ccc]]\n",
        );
        assert_eq!(ledger.theme_names, vec!["GOALS"]);
        assert_eq!(ledger.links.count, 1);
        assert_eq!(ledger.entries.len(), 1);
    }

    #[test]
    fn missing_section_reports_empty() {
        let ledger = budget("# Just a note\n");
        assert!(!ledger.has_section);
        assert_eq!(ledger.status, PlanStatus::Ok);
        assert!(ledger.entries.is_empty());
    }

    #[test]
    fn fenced_links_and_fenced_entries_never_count() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            ```\n\
            - [ ] () — FAKE\n\
            \x20   - [[task#^aaa]]\n\
            ```\n\
            - [ ] () — GOALS\n\
            \x20   - [[task#^bbb]]\n\
            ```\n\
            [[task#^ccc]]\n\
            ```\n",
        );
        assert_eq!(ledger.theme_names, vec!["GOALS"]);
        assert_eq!(ledger.links.count, 1);
    }

    #[test]
    fn running_and_highlight_flags_follow_ledger_order() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] (0945-1015) — GOALS\n\
            \x20   - [[task#^aaa]]\n\
            - [ ] () — DECKS\n",
        );
        assert!(ledger.entries[0].running);
        assert!(ledger.entries[0].highlight);
        assert_eq!(ledger.entries[0].time_range.as_deref(), Some("0945-1015"));
        assert!(!ledger.entries[1].running);
    }

    #[test]
    fn components_compare_case_insensitively() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — goals\n\
            - [ ] () — GOALS\n",
        );
        assert_eq!(ledger.theme_names, vec!["goals"]);
        assert_eq!(ledger.themes.count, 1);
        assert_eq!(codes(&ledger), vec![LINT_DUPLICATE_NAME]);
    }

    #[test]
    fn alias_and_md_suffix_links_count() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - [[task.md#^aaa|alias]]\n\
            \x20   - [[task#^aaa]]\n",
        );
        assert_eq!(ledger.links.count, 1);
    }

    #[test]
    fn markers_and_inner_hash_links_count() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - 🍅 [[task#^aaa]]\n\
            \x20   - [[other#^bbb#]]\n",
        );
        assert_eq!(ledger.links.count, 2);
        assert_eq!(ledger.entries[0].links, 2);
    }

    #[test]
    fn lane_queries_parse_through_the_native_engine() {
        // The fixed lane queries must stay valid Tasks syntax; the
        // counts themselves are covered by the fixture-vault CLI
        // tests.
        let vault = tempfile::tempdir().expect("temp vault");
        let now = chrono::NaiveDate::from_ymd_opt(2026, 9, 30)
            .expect("date")
            .and_hms_opt(12, 0, 0)
            .expect("noon");
        for query in [dataview::NEXT_QUERY, dataview::PENDING_QUERY] {
            let descriptions =
                dataview::query_matching_descriptions(vault.path(), query, now)
                    .expect("lane query parses and runs");
            assert!(descriptions.is_empty());
        }
    }

    #[test]
    fn lane_over_never_changes_status() {
        let ledger = budget(
            "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n",
        );
        let today = chrono::NaiveDate::from_ymd_opt(2026, 9, 30).expect("date");
        let config = PlanConfig::default();
        let lanes = Lanes {
            next: LaneBudget {
                count: config.max_next() as usize + 1,
                cap: config.max_next(),
                over: true,
            },
            pending: LaneBudget {
                count: config.max_pending() as usize + 1,
                cap: config.max_pending(),
                over: true,
            },
        };
        let today_result = TodayResult {
            tasks: Vec::new(),
            warnings: Vec::new(),
        };
        let report = assemble_report(
            today,
            "2026/20260930.md",
            &config,
            &ledger,
            &lanes,
            &today_result,
        );
        assert_eq!(report.status, PlanStatus::Ok);
        assert_eq!(codes(&ledger), Vec::<&str>::new());
        assert_eq!(
            report
                .warnings
                .iter()
                .map(|w| w.code.as_str())
                .collect::<Vec<_>>(),
            vec![LINT_NEXT_CAP, LINT_PENDING_CAP]
        );
    }

    #[test]
    fn eleven_links_are_over_the_cap() {
        let mut contents = String::from("## Pomodoros\n\n- [ ] () — GOALS\n");
        for index in 0..11 {
            contents.push_str(&format!("    - [[task#^{index:04}]]\n"));
        }
        let ledger = budget(&contents);
        assert_eq!(ledger.links.count, 11);
        assert!(ledger.links.over);
        assert_eq!(ledger.status, PlanStatus::Over);
        assert_eq!(codes(&ledger), vec![LINT_LINK_CAP]);
    }

    #[test]
    fn empty_target_means_the_daily_note() {
        let contents = "## Pomodoros\n\
            \n\
            - [ ] () — GOALS\n\
            \x20   - [[#^aaa]]\n\
            \x20   - [[2026/20260930#^aaa]]\n\
            \x20   - [[20260930#^aaa]]\n";
        let daily = "2026/20260930";
        let ledger =
            compute_for_daily(contents, &PlanConfig::default(), Some(daily));
        assert_eq!(ledger.links.count, 1);
        assert_eq!(ledger.entries[0].links, 1);
        let without_daily = budget(contents);
        assert_eq!(without_daily.links.count, 3);
    }
}
