//! Shared plan-budget core: the `plan:` config definition, a pure
//! ledger budget and lint engine, and the NOW counter built on the
//! native Tasks engine. `docs/plan.md` is the authoritative definition;
//! this module is its Rust implementation, shared by `bob plan`, the
//! task-status hooks, and capture.

pub(crate) mod cli;

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
pub(crate) const LINT_NOW_CAP: &str = "now_cap_exceeded";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NowBudget {
    pub(crate) count: usize,
    pub(crate) cap: u32,
    pub(crate) over: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanCaps {
    pub(crate) max_themes: u32,
    pub(crate) max_links: u32,
    pub(crate) max_now: u32,
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

/// The full report shared by `bob plan`, the hooks, and capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanReport {
    pub(crate) date: String,
    pub(crate) daily_file: String,
    pub(crate) caps: PlanCaps,
    pub(crate) status: PlanStatus,
    pub(crate) themes: PlanMeter,
    pub(crate) links: PlanMeter,
    pub(crate) now: NowBudget,
    pub(crate) theme_names: Vec<String>,
    pub(crate) entries: Vec<PlanEntry>,
    pub(crate) warnings: Vec<PlanLint>,
}

impl PlanReport {
    pub(crate) fn caps_of(config: &PlanConfig) -> PlanCaps {
        PlanCaps {
            max_themes: config.max_themes(),
            max_links: config.max_links(),
            max_now: config.max_now(),
            strict: config.strict(),
        }
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

/// Whether `text` carries the case-sensitive whole token `#now`:
/// preceded by the line start or whitespace, followed by the end or
/// whitespace. `#nowadays` and `#now/x` never match.
///
/// Shared with the close-drop and now-token phases (per-row `now`
/// flags and the `^` picker); `bob plan` itself counts NOW through
/// the Tasks engine plus this predicate.
pub(crate) fn has_now_tag(text: &str) -> bool {
    let mut search = text;
    let mut offset = 0;
    while let Some(found) = search.find("#now") {
        let absolute = offset + found;
        let before_ok = text[..absolute]
            .chars()
            .next_back()
            .is_none_or(|cell| cell.is_whitespace());
        let after_ok = text[absolute + "#now".len()..]
            .chars()
            .next()
            .is_none_or(|cell| cell.is_whitespace());
        if before_ok && after_ok {
            return true;
        }
        search = &search[found + 1..];
        offset = absolute + 1;
    }
    false
}

/// Count this week's bets through the native Tasks engine, so the
/// result honors the vault's Tasks settings, then keep only whole
/// `#now` tokens: the engine's `tags include` also matches subtags
/// like `#now/x`. A scan failure reads as zero: the vault was
/// already proven readable by the daily-note read, and the query
/// text itself is covered by tests.
pub(crate) fn count_now(
    bob_dir: &Path,
    today: NaiveDate,
    config: &PlanConfig,
) -> NowBudget {
    let now = today
        .and_hms_opt(12, 0, 0)
        .unwrap_or(chrono::NaiveDateTime::MIN);
    let count = dataview::query_matching_descriptions(
        bob_dir,
        dataview::NOW_QUERY,
        now,
    )
    .map(|descriptions| {
        descriptions.iter().filter(|text| has_now_tag(text)).count()
    })
    .unwrap_or(0);
    let cap = config.max_now();
    let over = u64::try_from(count).unwrap_or(u64::MAX) > u64::from(cap);
    NowBudget { count, cap, over }
}

/// Pure ledger budget and lint engine implementing `docs/plan.md`
/// rules 1-9. Only open entries count; completed and cancelled
/// entries are history.
pub(crate) fn compute(contents: &str, config: &PlanConfig) -> LedgerBudget {
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
fn split_components(name: &str) -> Vec<String> {
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
        for link in block_links(line) {
            links.push(link);
        }
    }
    links
}

/// Block links `[[target#^id]]` (also `![[…]]` and `[[…|alias]]`,
/// with or without a trailing `#` move-only marker) outside
/// `~~…~~` struck spans.
fn block_links(line: &str) -> Vec<(String, String)> {
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
                links.push((target, block_id.to_string()));
            }
        }
        base = link_end;
        rest = &line[base..];
    }
    links
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
        compute(contents, &PlanConfig::default())
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
    fn has_now_tag_matches_whole_tokens_only() {
        assert!(has_now_tag("- [ ] #task Foo #now [created:: 2026-09-30]"));
        assert!(has_now_tag("#now leading"));
        assert!(has_now_tag("trailing #now"));
        assert!(!has_now_tag("- [ ] #task #nowadays"));
        assert!(!has_now_tag("- [ ] #task #now/x"));
        assert!(!has_now_tag("- [ ] #task #NOW"));
        assert!(!has_now_tag("- [ ] #task snow"));
    }

    #[test]
    fn now_query_parses_through_the_native_engine() {
        // The fixed NOW query must stay valid Tasks syntax; the count
        // itself is covered by the fixture-vault CLI tests.
        let vault = tempfile::tempdir().expect("temp vault");
        let now = chrono::NaiveDate::from_ymd_opt(2026, 9, 30)
            .expect("date")
            .and_hms_opt(12, 0, 0)
            .expect("noon");
        let descriptions = dataview::query_matching_descriptions(
            vault.path(),
            dataview::NOW_QUERY,
            now,
        )
        .expect("NOW query parses and runs");
        assert!(descriptions.is_empty());
    }
}
