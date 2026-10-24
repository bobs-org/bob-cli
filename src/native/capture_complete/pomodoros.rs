use std::{fs, io, path::Path};

use super::theme_menu;

use super::{
    model::{
        Candidates, CompleteError, OverrideRunningSession,
        PomodoroNameCandidate,
    },
    support::{bounded_warning, rank},
};
use crate::native::{
    capture_language,
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    config::{self},
    plan_budget::{self},
    pomodoro,
};

pub(super) fn pomodoro_candidates(
    bob_dir: &Path,
    query: &str,
    start: bool,
    literal: bool,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    // Resolve Bob's date once: DATE may be a subprocess and timezone
    // configuration is read on each call.
    let today = crate::native::env::current_datetime().date();
    let canonical = bob_dir.join(format!(
        "{}/{}.md",
        today.format("%Y"),
        today.format("%Y%m%d")
    ));
    let day_file = crate::native::env::var_os("BOB_DAY_FILE")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| canonical.clone());
    let (scan, contents, mut warnings) = read_daily(&day_file)?;
    let daily_key = plan_budget::daily_key_from_path(&day_file);
    let hint = plan_creation_hint(&contents, &scan, daily_key.as_deref());
    let keys = scan
        .entries
        .iter()
        .filter_map(|entry| entry.name.as_deref())
        .chain(std::iter::once(query))
        .flat_map(plan_budget::split_components)
        .map(|name| plan_budget::normalize_component(&name))
        .collect();
    let catalog = crate::native::pomodoro_themes::Catalog::load_for_completion(
        bob_dir,
        today,
        keys,
        (day_file == canonical).then_some(&scan),
    );
    let catalog = match catalog {
        Ok(catalog) => {
            warnings.extend(
                catalog.warnings().iter().cloned().map(bounded_warning),
            );
            Some(catalog)
        }
        Err(error) => {
            warnings
                .push(bounded_warning(format!("read theme catalog: {error}")));
            None
        }
    };
    let candidates = theme_menu::compose(
        &scan,
        query,
        hint.as_ref(),
        catalog.as_ref(),
        start,
        literal,
    );
    Ok((Candidates::PomodoroName(candidates), warnings))
}

fn read_daily(
    day_file: &Path,
) -> Result<(capture_pomodoros::PomodoroScan, String, Vec<String>), CompleteError>
{
    let contents = match fs::read_to_string(day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((
                capture_pomodoros::scan(""),
                String::new(),
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
            )))
        }
    };
    let scan = capture_pomodoros::scan(&contents);
    let mut warnings = scan.warnings.clone();
    if !scan.has_section {
        warnings.push(bounded_warning(format!(
            "Bob daily note has no Pomodoros section: {}",
            day_file.display()
        )));
    }
    Ok((scan, contents, warnings))
}

#[cfg(test)]
pub(super) fn pomodoro_name_candidates_at(
    day_file: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    candidates_at(day_file, query, false)
}

#[cfg(test)]
fn candidates_at(
    day_file: &Path,
    query: &str,
    start: bool,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let (scan, contents, warnings) = read_daily(day_file)?;
    let daily_key = plan_budget::daily_key_from_path(day_file);
    let hint = plan_creation_hint(&contents, &scan, daily_key.as_deref());
    Ok((
        Candidates::PomodoroName(theme_menu::compose(
            &scan,
            query,
            hint.as_ref(),
            None,
            start,
            false,
        )),
        warnings,
    ))
}

/// The single running Pomodoro session, for the `running` half of the
/// named-start completion `override` object. `None` when the daily note
/// is missing or holds zero or several timed open entries. An unnamed
/// running session also carries its derived forecast `homes`; a named
/// one does no resolution.
pub(super) fn running_pomodoro_session(
    bob_dir: &Path,
) -> Option<OverrideRunningSession> {
    let day_file = pomodoro::day_file_for(bob_dir);
    let contents = fs::read_to_string(&day_file).ok()?;
    let scan = capture_pomodoros::scan(&contents);
    let mut running = scan.entries.iter().filter(|entry| {
        entry.state == PomodoroState::Open && entry.time_range.is_some()
    });
    let entry = running.next()?;
    if running.next().is_some() {
        return None;
    }
    let homes = if entry.name.as_deref().is_some_and(|name| !name.is_empty()) {
        None
    } else {
        let exempt = config::load_plan_config(&config::config_path())
            .map(|loaded| loaded.exempt().to_vec())
            .unwrap_or_else(|_| {
                config::PlanConfig::default().exempt().to_vec()
            });
        let daily_path = day_file
            .strip_prefix(bob_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| day_file.clone());
        let mut resolver = crate::native::pomodoro_homes::HomeResolver::new(
            crate::native::pomodoro_homes::DiskHomeSource::new(bob_dir),
        );
        Some(
            crate::native::pomodoro_homes::attribute_ledger_entry(
                &contents,
                entry.line,
                None,
                false,
                None,
                &exempt,
                &daily_path,
                &mut resolver,
            )
            .homes,
        )
    };
    Some(OverrideRunningSession {
        pomodoro_name: entry.name.clone(),
        line: entry.line,
        time_range: entry.time_range.clone().unwrap_or_default(),
        homes,
    })
}

#[cfg(test)]
pub(super) fn pomodoro_start_name_candidates_at(
    day_file: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    candidates_at(day_file, query, true)
}

#[cfg(test)]
pub(super) fn pomodoro_start_name_candidates_from_scan(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    pomodoro_start_name_candidates_from_scan_with_hint(scan, query, None)
}

#[cfg(test)]
pub(super) fn pomodoro_start_name_candidates_from_scan_with_hint(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<PlanCreationHint>,
) -> Vec<PomodoroNameCandidate> {
    theme_menu::compose(scan, query, plan_hint.as_ref(), None, true, false)
}

fn entry_key(entry: &PomodoroEntry) -> String {
    plan_budget::normalize_component(entry.name.as_deref().unwrap_or_default())
}

pub(super) fn today_start_rows(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<&PlanCreationHint>,
) -> Vec<PomodoroNameCandidate> {
    let next_up_line =
        capture_pomodoros::next_future_pomodoro(scan).map(|entry| entry.line);

    let mut seen_keys = Vec::<String>::new();
    let mut start = Vec::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open
            || entry.time_range.is_some()
            || !entry.placeholder
            || !entry.selectable
            || seen_keys.contains(&entry_key(entry))
        {
            continue;
        }
        let match_count = scan
            .entries
            .iter()
            .filter(|candidate| {
                candidate.state == PomodoroState::Open
                    && candidate.selectable
                    && entry_key(candidate) == entry_key(entry)
            })
            .count();
        seen_keys.push(entry_key(entry));
        start.push(pomodoro_name_candidate_with_next_up(
            entry,
            false,
            match_count,
            next_up_line == Some(entry.line),
        ));
    }
    let start = rank(start, query, |candidate| candidate.replacement.as_str());

    let open_keys = scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open)
        .map(entry_key)
        .collect::<Vec<_>>();
    // Deduplicate completed keys keeping the latest in document order,
    // then list the most recent first so an empty query resurfaces the
    // last session first.
    let mut again_entries = Vec::<&PomodoroEntry>::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Completed
            || !entry.selectable
            || open_keys.contains(&entry_key(entry))
        {
            continue;
        }
        if let Some(position) = again_entries
            .iter()
            .position(|existing| entry_key(existing) == entry_key(entry))
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
                    && entry_key(candidate) == entry_key(entry)
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
        candidate.replacement = theme_menu::lossless_replacement(
            scan,
            entry.name.as_deref().unwrap_or(&entry.slug),
        );
        candidate.name = capture_pomodoros::canonicalize_pomodoro_name(
            entry.name.as_deref().unwrap_or(&entry.slug),
        )
        .or_else(|| entry.name.clone());
        candidate.creates_pomodoro = true;
        let display =
            candidate.name.clone().unwrap_or_else(|| entry.slug.clone());
        (candidate.plan_themes_after, candidate.plan_themes_cap) =
            plan_themes_after_for(plan_hint, &display);
        again.push(candidate);
    }
    let again = rank(again, query, |candidate| {
        candidate.name.as_deref().unwrap_or_default()
    });

    let mut combined = start;
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
                    && entry_key(candidate) == entry_key(entry)
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

#[cfg(test)]
pub(super) fn pomodoro_name_candidates_from_scan_with_hint(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    plan_hint: Option<PlanCreationHint>,
) -> Vec<PomodoroNameCandidate> {
    theme_menu::compose(scan, query, plan_hint.as_ref(), None, false, false)
}

pub(super) fn creation_candidate(
    scan: &capture_pomodoros::PomodoroScan,
    name: String,
    hint: Option<&PlanCreationHint>,
) -> PomodoroNameCandidate {
    let (plan_themes_after, plan_themes_cap) =
        plan_themes_after_for(hint, &name);
    PomodoroNameCandidate {
        replacement: theme_menu::lossless_replacement(scan, &name),
        pomodoro_ref: None,
        name: Some(name),
        themes: Vec::new(),
        origin: None,
        last_used: None,
        days_used: None,
        hidden: false,
        similar_to: Vec::new(),
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
    }
}

pub(super) fn pomodoro_name_candidates_from_entries(
    entries: &[PomodoroEntry],
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    let open_entries = entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Open)
        .collect::<Vec<_>>();
    let mut seen_keys = Vec::<String>::new();
    let mut named = Vec::new();
    for entry in &open_entries {
        if !entry.selectable || seen_keys.contains(&entry_key(entry)) {
            continue;
        }
        let match_count = open_entries
            .iter()
            .filter(|candidate| {
                candidate.selectable && entry_key(candidate) == entry_key(entry)
            })
            .count();
        seen_keys.push(entry_key(entry));
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

/// Prefix `#` onto start-name replacements that cannot be spelled bare, so
/// accepting a substring match like `2fa` on `=fa|` yields `=#2fa`.
pub(super) fn apply_bare_start_name_replacements(candidates: &mut Candidates) {
    let Candidates::PomodoroName(items) = candidates else {
        return;
    };
    for item in items {
        if item.replacement.is_empty() {
            continue;
        }
        if !capture_language::can_spell_bare(&item.replacement) {
            item.replacement.insert(0, '#');
        }
    }
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
        themes: Vec::new(),
        origin: None,
        last_used: None,
        days_used: None,
        hidden: false,
        similar_to: Vec::new(),
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
