//! Pure merged-Pomodoro planners: merge names, split, select, narrow,
//! and shed (R3–R7).
//!
//! Landing lives in [`super::routing::land`]. These helpers are plain
//! data in, plain data out so later capture and Obsidian phases share
//! one contract with the vector file.
#![allow(dead_code)]

use std::collections::BTreeSet;

use crate::native::{
    config::themes::ThemeSources,
    plan_budget::{normalize_component, split_components},
};

use super::routing::{
    contains_key, contains_key_prefix, is_exact, is_meet_name, is_prefix_exact,
    parse_type_selector, type_components, TypeComponent,
};

/// Deduping name join for R5: `base`'s components, then each other's
/// components that are not already present. First spelling wins; labels
/// stay; selector order is the `others` order.
pub(crate) fn merge_names(base: &str, others: &[&str]) -> String {
    let mut seen = BTreeSet::new();
    let mut parts = Vec::new();
    for name in std::iter::once(base).chain(others.iter().copied()) {
        for component in split_components(name) {
            let key = normalize_component(&component);
            if seen.insert(key) {
                parts.push(component);
            }
        }
    }
    parts.join(" + ")
}

/// Closed-session name (R6 narrow). Labels stay. When at least one
/// worked link's type is a type component of `name`, the result is
/// those labels plus exactly the matching type components, in `name`
/// order and spelling. Single-type and MEET names are unchanged.
pub(crate) fn narrow_closed(
    name: &str,
    worked_keys: &[&str],
    sources: &ThemeSources,
) -> String {
    if is_meet_name(Some(name)) {
        return name.to_string();
    }
    let classified = type_components(Some(name), sources);
    let type_keys: BTreeSet<&str> = classified
        .iter()
        .filter(|item| item.is_type)
        .map(|item| item.key.as_str())
        .collect();
    if type_keys.len() < 2 {
        return name.to_string();
    }
    let worked: BTreeSet<String> = worked_keys
        .iter()
        .map(|key| normalize_component(key))
        .collect();
    if !worked.iter().any(|key| type_keys.contains(key.as_str())) {
        return name.to_string();
    }
    join_kept(&classified, |item| {
        !item.is_type || worked.contains(&item.key)
    })
}

/// Carried-session / source-after-move name (R6 / R7 shed). Drop each
/// type T that had at least one link before and none after. If that
/// would drop every type, keep the base. Components that never had a
/// link stay. Labels stay. Single-type and MEET names are unchanged.
pub(crate) fn shed(
    name: &str,
    before_keys: &[&str],
    after_keys: &[&str],
    sources: &ThemeSources,
) -> String {
    if is_meet_name(Some(name)) {
        return name.to_string();
    }
    let classified = type_components(Some(name), sources);
    let type_items: Vec<&TypeComponent> =
        classified.iter().filter(|item| item.is_type).collect();
    if type_items.len() < 2 {
        return name.to_string();
    }
    let before: BTreeSet<String> = before_keys
        .iter()
        .map(|key| normalize_component(key))
        .collect();
    let after: BTreeSet<String> = after_keys
        .iter()
        .map(|key| normalize_component(key))
        .collect();
    let mut drop: BTreeSet<&str> = type_items
        .iter()
        .map(|item| item.key.as_str())
        .filter(|key| before.contains(*key) && !after.contains(*key))
        .collect();
    let all_types_dropped = type_items
        .iter()
        .all(|item| drop.contains(item.key.as_str()));
    if all_types_dropped && let Some(base) = type_items.first() {
        drop.remove(base.key.as_str());
    }
    join_kept(&classified, |item| {
        !item.is_type || !drop.contains(item.key.as_str())
    })
}

fn join_kept(
    classified: &[TypeComponent],
    keep: impl Fn(&TypeComponent) -> bool,
) -> String {
    classified
        .iter()
        .filter(|item| keep(item))
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// One new planned entry produced by a split, with the source-link
/// indices that move into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitCreated {
    pub(crate) name: String,
    pub(crate) link_indices: Vec<usize>,
}

/// Remaining name plus ordered new entries (R4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitPlan {
    pub(crate) remaining_name: String,
    pub(crate) created: Vec<SplitCreated>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SplitError {
    Closed,
    Meet,
    NotMerged,
    TypeNotNeeded,
    BaseType { name: String },
    TypeUnknown { selector: String },
    TypeRepeated { name: String },
}

impl SplitError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Closed => "split_closed",
            Self::Meet => "split_meet",
            Self::NotMerged => "split_not_merged",
            Self::TypeNotNeeded => "split_type_not_needed",
            Self::BaseType { .. } => "split_base_type",
            Self::TypeUnknown { .. } => "split_type_unknown",
            Self::TypeRepeated { .. } => "split_type_repeated",
        }
    }
}

pub(crate) struct SplitPlanInput<'a> {
    pub(crate) name: &'a str,
    pub(crate) open: bool,
    pub(crate) link_keys: &'a [Option<&'a str>],
    pub(crate) split_selectors: Option<&'a [&'a str]>,
    pub(crate) sources: &'a ThemeSources,
}

/// Partition an open merged Pomodoro by type (R4). `link_keys` is each
/// top-level Task Link's effective type, in child order. `None`
/// selectors split every merged-in type (the right-hand type when there
/// are exactly two).
pub(crate) fn split_plan(
    input: &SplitPlanInput<'_>,
) -> Result<SplitPlan, SplitError> {
    if !input.open {
        return Err(SplitError::Closed);
    }
    if is_meet_name(Some(input.name)) {
        return Err(SplitError::Meet);
    }
    let classified = type_components(Some(input.name), input.sources);
    let type_items: Vec<&TypeComponent> =
        classified.iter().filter(|item| item.is_type).collect();
    if type_items.len() < 2 {
        return Err(SplitError::NotMerged);
    }
    let base = type_items[0];
    let merged_in = &type_items[1..];
    let selected = match input.split_selectors {
        None | Some([]) => merged_in.to_vec(),
        Some(_) if type_items.len() == 2 => {
            return Err(SplitError::TypeNotNeeded);
        }
        Some(selectors) => resolve_split_types(selectors, base, merged_in)?,
    };
    let selected_keys: BTreeSet<&str> =
        selected.iter().map(|item| item.key.as_str()).collect();
    let remaining_name = join_kept(&classified, |item| {
        !item.is_type || !selected_keys.contains(item.key.as_str())
    });
    let created = selected
        .iter()
        .map(|item| SplitCreated {
            name: item.name.clone(),
            link_indices: input
                .link_keys
                .iter()
                .enumerate()
                .filter_map(|(index, key)| {
                    let key = key.as_ref()?;
                    (normalize_component(key) == item.key).then_some(index)
                })
                .collect(),
        })
        .collect();
    Ok(SplitPlan {
        remaining_name,
        created,
    })
}

fn resolve_split_types<'a>(
    selectors: &[&str],
    base: &'a TypeComponent,
    merged_in: &[&'a TypeComponent],
) -> Result<Vec<&'a TypeComponent>, SplitError> {
    let mut chosen: Vec<&TypeComponent> = Vec::new();
    let mut seen = BTreeSet::new();
    for selector in selectors {
        let parsed = parse_type_selector(selector);
        if parsed.key.is_empty() {
            return Err(SplitError::TypeUnknown {
                selector: (*selector).to_string(),
            });
        }
        if parsed.key == base.key
            || (!parsed.literal && base.key.starts_with(&parsed.key))
        {
            return Err(SplitError::BaseType {
                name: base.name.clone(),
            });
        }
        let resolved =
            match_split_type(&parsed, merged_in).ok_or_else(|| {
                SplitError::TypeUnknown {
                    selector: (*selector).to_string(),
                }
            })?;
        if !seen.insert(resolved.key.as_str()) {
            return Err(SplitError::TypeRepeated {
                name: resolved.name.clone(),
            });
        }
        chosen.push(resolved);
    }
    chosen.sort_by_key(|item| {
        merged_in
            .iter()
            .position(|candidate| candidate.key == item.key)
    });
    Ok(chosen)
}

fn match_split_type<'a>(
    parsed: &super::routing::ParsedSelector,
    merged_in: &[&'a TypeComponent],
) -> Option<&'a TypeComponent> {
    if let Some(exact) = merged_in
        .iter()
        .find(|item| item.key == parsed.key)
        .copied()
    {
        return Some(exact);
    }
    if parsed.literal {
        return None;
    }
    let matches: Vec<&TypeComponent> = merged_in
        .iter()
        .copied()
        .filter(|item| item.key.starts_with(&parsed.key))
        .collect();
    match matches.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// One ledger entry as seen by type-selector resolution (R3).
#[derive(Debug, Clone)]
pub(crate) struct SelectEntry<'a> {
    pub(crate) name: Option<&'a str>,
    pub(crate) open: bool,
    pub(crate) running: bool,
    pub(crate) timed: bool,
}

/// Which open entries a type selector may match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectFilter {
    /// Today's open entries (R3 default).
    Open,
    /// Open untimed planned entries other than `except` (R5 dest).
    UntimedPlanned { except: Option<usize> },
    /// Open merged Pomodoros, matching their base type (`=/` base).
    MergedBase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectOutcome {
    Entry(usize),
    Missing,
}

pub(crate) struct SelectInput<'a> {
    pub(crate) entries: &'a [SelectEntry<'a>],
    pub(crate) selector: &'a str,
    pub(crate) filter: SelectFilter,
    pub(crate) sources: &'a ThemeSources,
}

/// Resolve a type selector over today's open entries without creating
/// one (R3). Whole-key match first (running exact, first open exact,
/// first containing); then the same three steps with key-prefix
/// matching unless the selector is a quoted literal.
pub(crate) fn select_by_type(input: &SelectInput<'_>) -> SelectOutcome {
    let parsed = parse_type_selector(input.selector);
    if parsed.key.is_empty() {
        return SelectOutcome::Missing;
    }
    match input.filter {
        SelectFilter::MergedBase => select_merged_base(input, &parsed),
        SelectFilter::Open | SelectFilter::UntimedPlanned { .. } => {
            select_open(input, &parsed)
        }
    }
}

fn eligible(input: &SelectInput<'_>, index: usize) -> bool {
    let Some(entry) = input.entries.get(index) else {
        return false;
    };
    if !entry.open {
        return false;
    }
    match input.filter {
        SelectFilter::Open => true,
        SelectFilter::UntimedPlanned { except } => {
            !entry.running && !entry.timed && except != Some(index)
        }
        SelectFilter::MergedBase => {
            type_components(entry.name, input.sources)
                .iter()
                .filter(|item| item.is_type)
                .count()
                >= 2
        }
    }
}

fn select_open(
    input: &SelectInput<'_>,
    parsed: &super::routing::ParsedSelector,
) -> SelectOutcome {
    let exempt: Vec<String> = input.sources.exempt.iter().cloned().collect();
    let whole = find_place_match(input, |entry| {
        is_exact(entry.name, &parsed.key, &exempt)
    })
    .or_else(|| {
        find_first(input, |entry| contains_key(entry.name, &parsed.key))
    });
    if let Some(index) = whole {
        return SelectOutcome::Entry(index);
    }
    if parsed.literal {
        return SelectOutcome::Missing;
    }
    find_place_match(input, |entry| {
        is_prefix_exact(entry.name, &parsed.key, &exempt)
    })
    .or_else(|| {
        find_first(input, |entry| contains_key_prefix(entry.name, &parsed.key))
    })
    .map_or(SelectOutcome::Missing, SelectOutcome::Entry)
}

fn select_merged_base(
    input: &SelectInput<'_>,
    parsed: &super::routing::ParsedSelector,
) -> SelectOutcome {
    let whole = find_place_match(input, |entry| {
        base_key(entry.name, input.sources).as_deref()
            == Some(parsed.key.as_str())
    });
    if let Some(index) = whole {
        return SelectOutcome::Entry(index);
    }
    if parsed.literal {
        return SelectOutcome::Missing;
    }
    find_place_match(input, |entry| {
        base_key(entry.name, input.sources)
            .is_some_and(|key| key.starts_with(&parsed.key))
    })
    .map_or(SelectOutcome::Missing, SelectOutcome::Entry)
}

fn base_key(name: Option<&str>, sources: &ThemeSources) -> Option<String> {
    type_components(name, sources)
        .into_iter()
        .find(|item| item.is_type)
        .map(|item| item.key)
}

fn find_place_match(
    input: &SelectInput<'_>,
    pred: impl Fn(&SelectEntry<'_>) -> bool,
) -> Option<usize> {
    let running =
        input.entries.iter().enumerate().find_map(|(index, entry)| {
            (eligible(input, index) && entry.running && pred(entry))
                .then_some(index)
        });
    if running.is_some() {
        return running;
    }
    find_first(input, pred)
}

fn find_first(
    input: &SelectInput<'_>,
    pred: impl Fn(&SelectEntry<'_>) -> bool,
) -> Option<usize> {
    input.entries.iter().enumerate().find_map(|(index, entry)| {
        (eligible(input, index) && pred(entry)).then_some(index)
    })
}
