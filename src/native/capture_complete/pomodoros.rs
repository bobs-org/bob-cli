use std::{fs, io, path::Path};

use super::{
    model::{Candidates, CompleteError, PomodoroNameCandidate},
    support::{bounded_warning, rank},
};
use crate::native::{
    capture_language,
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    config::{self},
    plan_budget::{self},
    pomodoro,
};

pub(super) fn pomodoro_name_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let day_file = pomodoro::day_file_for(bob_dir);
    pomodoro_name_candidates_at(&day_file, query)
}

pub(super) fn pomodoro_name_candidates_at(
    day_file: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let contents = match fs::read_to_string(day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((
                Candidates::PomodoroName(Vec::new()),
                vec![bounded_warning(format!(
                    "Bob daily note does not exist: {}",
                    day_file.display()
                ))],
            ));
        }
        Err(error) => {
            return Err(CompleteError::io(format!(
                "read daily note {}: {error}",
                day_file.display()
            )));
        }
    };

    let scan = capture_pomodoros::scan(&contents);
    let mut warnings = Vec::new();
    if !scan.has_section {
        warnings.push(bounded_warning(format!(
            "Bob daily note has no Pomodoros section: {}",
            day_file.display()
        )));
    }
    warnings.extend(scan.warnings.iter().cloned());
    let daily_key = plan_budget::daily_key_from_path(day_file);
    let plan_hint = plan_creation_hint(&contents, &scan, daily_key.as_deref());
    let candidates =
        pomodoro_name_candidates_from_scan_with_hint(&scan, query, plan_hint);

    Ok((Candidates::PomodoroName(candidates), warnings))
}

pub(super) fn pomodoro_start_name_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let day_file = pomodoro::day_file_for(bob_dir);
    pomodoro_start_name_candidates_at(&day_file, query)
}

pub(super) fn pomodoro_start_name_candidates_at(
    day_file: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let contents = match fs::read_to_string(day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((
                Candidates::PomodoroName(Vec::new()),
                vec![bounded_warning(format!(
                    "Bob daily note does not exist: {}",
                    day_file.display()
                ))],
            ));
        }
        Err(error) => {
            return Err(CompleteError::io(format!(
                "read daily note {}: {error}",
                day_file.display()
            )));
        }
    };

    let scan = capture_pomodoros::scan(&contents);
    let mut warnings = Vec::new();
    if !scan.has_section {
        warnings.push(bounded_warning(format!(
            "Bob daily note has no Pomodoros section: {}",
            day_file.display()
        )));
    }
    warnings.extend(scan.warnings.iter().cloned());
    let daily_key = plan_budget::daily_key_from_path(day_file);
    let plan_hint = plan_creation_hint(&contents, &scan, daily_key.as_deref());
    let candidates = pomodoro_start_name_candidates_from_scan_with_hint(
        &scan, query, plan_hint,
    );

    Ok((Candidates::PomodoroName(candidates), warnings))
}

#[cfg(test)]
pub(super) fn pomodoro_start_name_candidates_from_scan(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    pomodoro_start_name_candidates_from_scan_with_hint(scan, query, None)
}

pub(super) fn pomodoro_start_name_candidates_from_scan_with_hint(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<PlanCreationHint>,
) -> Vec<PomodoroNameCandidate> {
    let next_up_line =
        capture_pomodoros::next_future_pomodoro(scan).map(|entry| entry.line);

    let mut seen_slugs = Vec::<&str>::new();
    let mut start = Vec::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open
            || entry.time_range.is_some()
            || !entry.placeholder
            || !entry.selectable
            || seen_slugs.contains(&entry.slug.as_str())
        {
            continue;
        }
        let match_count = scan
            .entries
            .iter()
            .filter(|candidate| {
                candidate.state == PomodoroState::Open
                    && candidate.selectable
                    && candidate.slug == entry.slug
            })
            .count();
        seen_slugs.push(&entry.slug);
        start.push(pomodoro_name_candidate_with_next_up(
            entry,
            false,
            match_count,
            next_up_line == Some(entry.line),
        ));
    }
    let start = rank(start, query, |candidate| candidate.replacement.as_str());

    let open_slugs = scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open)
        .map(|entry| entry.slug.as_str())
        .collect::<Vec<_>>();
    // Deduplicate completed slugs keeping the latest in document order,
    // then list the most recent first so an empty query resurfaces the
    // last session first.
    let mut again_entries = Vec::<&PomodoroEntry>::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Completed
            || !entry.selectable
            || open_slugs.contains(&entry.slug.as_str())
        {
            continue;
        }
        if let Some(position) = again_entries
            .iter()
            .position(|existing| existing.slug == entry.slug)
        {
            again_entries[position] = entry;
        } else {
            again_entries.push(entry);
        }
    }
    again_entries.reverse();
    let mut again = Vec::new();
    for entry in again_entries {
        let match_count = scan
            .entries
            .iter()
            .filter(|candidate| {
                candidate.state == PomodoroState::Completed
                    && candidate.selectable
                    && candidate.slug == entry.slug
            })
            .count();
        let mut candidate = pomodoro_name_candidate_with_next_up(
            entry,
            false,
            match_count,
            false,
        );
        // An "again" row starts a new session named like the completed
        // one, so it creates and reports that entry's history.
        candidate.replacement = entry.slug.clone();
        candidate.name = capture_pomodoros::canonicalize_pomodoro_name(
            entry.name.as_deref().unwrap_or(&entry.slug),
        )
        .or_else(|| entry.name.clone());
        candidate.creates_pomodoro = true;
        let display =
            candidate.name.clone().unwrap_or_else(|| entry.slug.clone());
        (candidate.plan_themes_after, candidate.plan_themes_cap) =
            plan_themes_after_for(plan_hint.as_ref(), &display);
        again.push(candidate);
    }
    let again = rank(again, query, |candidate| candidate.replacement.as_str());

    let mut combined = start;
    if let Some(creation) =
        pomodoro_start_creation_candidate(scan, query, plan_hint.as_ref())
    {
        insert_pomodoro_creation_candidate(&mut combined, creation, query);
    }
    combined.extend(again);

    combined.extend(scan.entries.iter().filter_map(|entry| {
        if entry.state != PomodoroState::Open
            || entry.time_range.is_some()
            || !entry.placeholder
            || entry.selectable
        {
            return None;
        }
        Some(pomodoro_name_candidate_with_next_up(
            entry,
            true,
            1,
            next_up_line == Some(entry.line),
        ))
    }));

    combined.extend(scan.entries.iter().filter_map(|entry| {
        if entry.state != PomodoroState::Open
            || entry.time_range.is_none()
            || !entry.selectable
        {
            return None;
        }
        let match_count = scan
            .entries
            .iter()
            .filter(|candidate| {
                candidate.state == PomodoroState::Open
                    && candidate.selectable
                    && candidate.slug == entry.slug
            })
            .count();
        Some(pomodoro_name_candidate_with_next_up(
            entry,
            false,
            match_count,
            false,
        ))
    }));

    combined
}

/// The start-specific create row: only a `Missing` query may create, so a
/// completed-only match ("again") never also offers a duplicate create
/// row. Placement, naming, and plan-budget preview reuse the shared link
/// helper; `named_creation_name` stays untouched for link-form callers.
pub(super) fn pomodoro_start_creation_candidate(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<&PlanCreationHint>,
) -> Option<PomodoroNameCandidate> {
    if query.is_empty() {
        return None;
    }
    if !matches!(
        capture_pomodoros::select_named(scan, query),
        capture_pomodoros::NamedSelection::Missing { .. }
    ) {
        return None;
    }
    pomodoro_creation_candidate(scan, query, plan_hint)
}

/// Theme count preview for a `creates_pomodoro` row: today's theme
/// count plus one for the new name, with the configured cap. `None`
/// when the ledger or the plan config is unavailable.
pub(super) struct PlanCreationHint {
    pub(super) before: usize,
    pub(super) keys: Vec<String>,
    pub(super) exempt: Vec<String>,
    pub(super) cap: u32,
}

pub(super) fn plan_creation_hint(
    contents: &str,
    scan: &capture_pomodoros::PomodoroScan,
    daily_file: Option<&str>,
) -> Option<PlanCreationHint> {
    if !scan.has_section {
        return None;
    }
    let config = config::load_plan_config(&config::config_path()).ok()?;
    let ledger = plan_budget::compute_for_daily(contents, &config, daily_file);
    Some(PlanCreationHint {
        before: ledger.themes.count,
        keys: ledger
            .theme_names
            .iter()
            .map(|name| plan_budget::normalize_component(name))
            .collect(),
        exempt: config
            .exempt()
            .iter()
            .map(|name| plan_budget::normalize_component(name))
            .collect(),
        cap: config.max_themes(),
    })
}

pub(super) fn plan_themes_after_for(
    hint: Option<&PlanCreationHint>,
    name: &str,
) -> (Option<usize>, Option<u32>) {
    let Some(hint) = hint else {
        return (None, None);
    };
    let mut fresh: Vec<String> = Vec::new();
    for component in plan_budget::split_components(name) {
        let key = plan_budget::normalize_component(&component);
        if hint.exempt.contains(&key) || hint.keys.contains(&key) {
            continue;
        }
        if !fresh.contains(&key) {
            fresh.push(key);
        }
    }
    (Some(hint.before + fresh.len()), Some(hint.cap))
}

#[cfg(test)]
pub(super) fn pomodoro_name_candidates_from_scan(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    pomodoro_name_candidates_from_scan_with_hint(scan, query, None)
}

pub(super) fn pomodoro_name_candidates_from_scan_with_hint(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<PlanCreationHint>,
) -> Vec<PomodoroNameCandidate> {
    let mut candidates =
        pomodoro_name_candidates_from_entries(&scan.entries, query);
    if let Some(creation) =
        pomodoro_creation_candidate(scan, query, plan_hint.as_ref())
    {
        insert_pomodoro_creation_candidate(&mut candidates, creation, query);
    }
    candidates
}

pub(super) fn pomodoro_creation_candidate(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<&PlanCreationHint>,
) -> Option<PomodoroNameCandidate> {
    let name = capture_pomodoros::named_creation_name(scan, query)?;
    let (plan_themes_after, plan_themes_cap) =
        plan_themes_after_for(plan_hint, &name);
    Some(PomodoroNameCandidate {
        replacement: capture_language::selector_slug(&name),
        pomodoro_ref: None,
        name: Some(name),
        requires_name: false,
        creates_pomodoro: true,
        next_up: false,
        plan_themes_after,
        plan_themes_cap,
        line: None,
        state: PomodoroState::Open,
        status_symbol: ' ',
        time_range: None,
        placeholder: true,
        is_current: false,
        child_count: 0,
        match_count: 1,
    })
}

pub(super) fn insert_pomodoro_creation_candidate(
    candidates: &mut Vec<PomodoroNameCandidate>,
    creation: PomodoroNameCandidate,
    query: &str,
) {
    let query = query.to_lowercase();
    let index = candidates
        .iter()
        .position(|candidate| {
            candidate.requires_name
                || !candidate.replacement.to_lowercase().starts_with(&query)
        })
        .unwrap_or(candidates.len());
    candidates.insert(index, creation);
}

pub(super) fn pomodoro_name_candidates_from_entries(
    entries: &[PomodoroEntry],
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    let open_entries = entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open)
        .collect::<Vec<_>>();
    let mut seen_slugs = Vec::<&str>::new();
    let mut named = Vec::new();
    for entry in &open_entries {
        if !entry.selectable || seen_slugs.contains(&entry.slug.as_str()) {
            continue;
        }
        let match_count = open_entries
            .iter()
            .filter(|candidate| {
                candidate.selectable && candidate.slug == entry.slug
            })
            .count();
        seen_slugs.push(&entry.slug);
        named.push(pomodoro_name_candidate(entry, false, match_count));
    }

    let mut candidates =
        rank(named, query, |candidate| candidate.replacement.as_str());
    candidates.extend(
        open_entries
            .into_iter()
            .filter(|entry| !entry.selectable)
            .map(|entry| pomodoro_name_candidate(entry, true, 1)),
    );
    candidates
}

pub(super) fn pomodoro_name_candidate(
    entry: &PomodoroEntry,
    requires_name: bool,
    match_count: usize,
) -> PomodoroNameCandidate {
    pomodoro_name_candidate_with_next_up(
        entry,
        requires_name,
        match_count,
        false,
    )
}

/// [`pomodoro_name_candidate`] plus the start-aware `next_up` marker. The
/// shared `pomodoro_name` context always passes false so its JSON stays
/// byte-identical; only `pomodoro_start_name` candidates set it.
pub(super) fn pomodoro_name_candidate_with_next_up(
    entry: &PomodoroEntry,
    requires_name: bool,
    match_count: usize,
    next_up: bool,
) -> PomodoroNameCandidate {
    PomodoroNameCandidate {
        replacement: if requires_name {
            String::new()
        } else {
            entry.slug.clone()
        },
        pomodoro_ref: Some(entry.pomodoro_ref.clone()),
        name: entry.name.clone(),
        requires_name,
        creates_pomodoro: false,
        next_up,
        plan_themes_after: None,
        plan_themes_cap: None,
        line: Some(entry.line),
        state: entry.state,
        status_symbol: entry.status_symbol,
        time_range: entry.time_range.clone(),
        placeholder: entry.placeholder,
        is_current: entry.is_current,
        child_count: entry.child_count,
        match_count,
    }
}
