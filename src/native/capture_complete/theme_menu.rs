//! PT7 menu composition and PT8 lossless replacements.

use std::collections::BTreeSet;

use super::{
    model::PomodoroNameCandidate,
    pomodoros::{
        creation_candidate, pomodoro_name_candidates_from_entries,
        today_start_rows, PlanCreationHint,
    },
};
use crate::native::{
    capture_language::selector_slug,
    capture_pomodoros::{
        canonicalize_pomodoro_name, select_named, NamedSelection, PomodoroScan,
        PomodoroState,
    },
    plan_budget::{normalize_component, split_components},
    pomodoro_themes::Catalog,
};

pub(super) fn lossless_replacement(scan: &PomodoroScan, name: &str) -> String {
    let slug = selector_slug(name);
    let same_target = match select_named(scan, &slug) {
        NamedSelection::Missing { .. } => true,
        NamedSelection::Found(entry) | NamedSelection::CompletedOnly(entry) => {
            entry.name.as_deref().is_some_and(|selected| {
                normalize_component(selected) == normalize_component(name)
            })
        }
    };
    if same_target && canonicalize_pomodoro_name(&slug).as_deref() == Some(name)
    {
        slug
    } else {
        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Literal fields have already been decoded by the lexer. Their matching
/// uses keys, so a space never accidentally selects the hyphenated theme.
fn match_rank(name: &str, query: &str, literal: bool) -> Option<u8> {
    let (name, query) = if literal {
        (normalize_component(name), normalize_component(query))
    } else {
        (selector_slug(name), selector_slug(query))
    };
    if name == query && !query.is_empty() {
        Some(0)
    } else if name.starts_with(&query) {
        Some(1)
    } else if !literal && name.contains(&query) {
        Some(2)
    } else {
        None
    }
}

pub(super) fn compose(
    scan: &PomodoroScan,
    query: &str,
    hint: Option<&PlanCreationHint>,
    catalog: Option<&Catalog>,
    start: bool,
    literal: bool,
) -> Vec<PomodoroNameCandidate> {
    let today = if start {
        today_start_rows(scan, "", hint)
    } else {
        pomodoro_name_candidates_from_entries(&scan.entries, "")
    };
    // Exclude components of *all* Today rows, before filtering the query.
    let exclusions: BTreeSet<String> = today
        .iter()
        .filter(|row| !row.requires_name)
        .filter_map(|row| row.name.as_deref())
        .flat_map(split_components)
        .map(|name| normalize_component(&name))
        .chain(catalog.into_iter().flat_map(|catalog| {
            catalog
                .records()
                .filter(|record| catalog.is_hidden(&record.key))
                .map(|record| record.key.clone())
        }))
        .collect();
    let exact_today = today.iter().any(|row| {
        !row.requires_name
            && row
                .name
                .as_deref()
                .is_some_and(|name| match_rank(name, query, literal) == Some(0))
    });
    let (utilities, mut matching): (Vec<_>, Vec<_>) =
        today.into_iter().partition(|row| {
            row.requires_name
                || (start
                    && row.time_range.is_some()
                    && row.state == PomodoroState::Open)
        });
    matching.retain(|row| {
        row.name
            .as_deref()
            .is_some_and(|name| match_rank(name, query, literal).is_some())
    });
    matching.sort_by_key(|row| {
        (
            start && row.state == PomodoroState::Completed,
            match_rank(row.name.as_deref().unwrap_or_default(), query, literal)
                .map(|rank| rank.max(1)),
        )
    });

    let mut prefix = Vec::new();
    let mut substring = Vec::new();
    if let Some(catalog) = catalog {
        for record in catalog
            .rank_for_query(query, &exclusions)
            .into_iter()
            .filter(|record| match_rank(&record.name, query, literal).is_some())
            .take(if query.is_empty() { 5 } else { 12 })
        {
            let mut row = creation_candidate(scan, record.name.clone(), hint);
            row.origin = Some("history");
            row.last_used = Some(record.last_used.to_string());
            row.days_used = Some(record.days_used);
            if match_rank(&record.name, query, literal) == Some(2) {
                substring.push(row);
            } else {
                prefix.push(row);
            }
        }
    }
    let exact_recent = prefix.iter().any(|row| {
        row.name
            .as_deref()
            .is_some_and(|name| match_rank(name, query, literal) == Some(0))
    });
    let can_place = scan.has_section
        && scan
            .entries
            .iter()
            .filter(|entry| {
                entry.state == PomodoroState::Open && entry.time_range.is_some()
            })
            .count()
            <= 1;
    let creation = (!exact_today && !exact_recent && can_place)
        .then(|| canonicalize_pomodoro_name(query))
        .flatten()
        .map(|name| {
            let mut row = creation_candidate(scan, name.clone(), hint);
            if let Some(catalog) = catalog {
                row.hidden = catalog.is_hidden(&normalize_component(&name));
                row.similar_to = catalog.similar_to(&name);
            }
            row
        });
    let mut rows: Vec<_> = matching
        .into_iter()
        .chain(prefix)
        .chain(creation)
        .chain(substring)
        .chain(utilities)
        .collect();
    for row in &mut rows {
        if let Some(name) = row.name.as_deref() {
            let selects_other = matches!(select_named(scan, &row.replacement),
                NamedSelection::Found(entry) | NamedSelection::CompletedOnly(entry)
                if entry.name.as_deref().is_some_and(|selected| normalize_component(selected) != normalize_component(name)));
            if (literal || selects_other) && !row.requires_name {
                row.replacement = lossless_replacement(scan, name);
            }
            if let Some(catalog) = catalog {
                row.themes = catalog.theme_refs_for_name(name);
            }
        }
    }
    rows
}
