use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::theme_menu;

use super::{
    model::{
        Candidates, CompleteError, OverrideRunningSession,
        PomodoroNameCandidate,
    },
    support::{bounded_warning, rank},
};
use crate::native::{
    capture, capture_language,
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    config::{self},
    plan_budget::{self},
    pomodoro,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    pomodoro_themes::{
        self,
        preselect::{self, MenuChoice, MenuEntry, MenuSuggestion},
        suggest::ThemeSuggestion,
    },
};

pub(super) fn pomodoro_candidates(
    bob_dir: &Path,
    query: &str,
    start: bool,
    literal: bool,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let (candidates, warnings, _) =
        pomodoro_candidates_inner(bob_dir, query, start, literal, None)?;
    Ok((candidates, warnings))
}

pub(super) fn pomodoro_name_candidates_for_task(
    bob_dir: &Path,
    query: &str,
    literal: bool,
    route: &str,
    block_id: Option<&str>,
    new_task: bool,
) -> Result<(Candidates, Vec<String>, Option<MenuSuggestion>), CompleteError> {
    pomodoro_candidates_inner(
        bob_dir,
        query,
        false,
        literal,
        Some((route, if new_task { None } else { block_id })),
    )
}

fn pomodoro_candidates_inner(
    bob_dir: &Path,
    query: &str,
    start: bool,
    literal: bool,
    task_target: Option<(&str, Option<&str>)>,
) -> Result<(Candidates, Vec<String>, Option<MenuSuggestion>), CompleteError> {
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
    let hint = plan_creation_hint(&scan);
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
    let suggestion = if !start {
        task_target.and_then(|(route, block_id)| {
            resolve_task_suggestion(
                bob_dir,
                route,
                block_id,
                catalog.as_ref()?,
                &mut warnings,
            )
        })
    } else {
        None
    };
    let next_up_line = start
        .then(|| {
            unnamed_start_next_up_line(
                &scan,
                &contents,
                &day_relative_for(bob_dir, &day_file),
            )
        })
        .flatten();
    let (candidates, menu_suggestion) = if let Some(suggestion) = suggestion {
        let choice = menu_choice(&suggestion, &scan, hint.as_ref());
        let candidates = theme_menu::compose_task_aware(
            &scan,
            query,
            catalog.as_ref(),
            start,
            literal,
            &suggestion,
            choice,
            next_up_line,
        );
        let output_choice = if matches!(choice, MenuChoice::Create)
            && !query.is_empty()
            && !candidates.iter().any(|row| row.suggested)
        {
            MenuChoice::Fallback(preselect::MenuFallback::CreationBlocked)
        } else {
            choice
        };
        (
            candidates,
            Some(MenuSuggestion::new(suggestion, output_choice, false)),
        )
    } else {
        (
            theme_menu::compose(
                &scan,
                query,
                catalog.as_ref(),
                start,
                literal,
                next_up_line,
            ),
            None,
        )
    };
    Ok((
        Candidates::PomodoroName(candidates),
        warnings,
        menu_suggestion,
    ))
}

pub(super) fn resolve_task_suggestion(
    bob_dir: &Path,
    route: &str,
    block_id: Option<&str>,
    catalog: &pomodoro_themes::Catalog,
    warnings: &mut Vec<String>,
) -> Option<ThemeSuggestion> {
    let cfg = match config::load_plan_config(&config::config_path()) {
        Ok(config) => config,
        Err(error) => {
            warnings
                .push(bounded_warning(format!("read plan config: {error}")));
            return None;
        }
    };
    let relative = capture::route_label(route);
    let result = if let Some(block_id) = block_id {
        let path = bob_dir.join(&relative);
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                String::new()
            }
            Err(error) => {
                warnings.push(bounded_warning(format!(
                    "read task note {}: {error}",
                    path.display()
                )));
                String::new()
            }
        };
        let settings = crate::native::note_tasks::read_settings(bob_dir);
        match crate::native::note_tasks::unique_capture_task_line(
            &contents, &settings, block_id,
        ) {
            Some(line) => {
                let mut source =
                    HomeResolver::new(DiskHomeSource::new(bob_dir));
                pomodoro_themes::suggest::suggest_for_task(
                    &mut source,
                    catalog,
                    &cfg,
                    Path::new(&relative),
                    line,
                )
            }
            None => {
                let mut source =
                    HomeResolver::new(DiskHomeSource::new(bob_dir));
                pomodoro_themes::suggest::suggest_for_new_task(
                    &mut source,
                    catalog,
                    &cfg,
                    Path::new(&relative),
                )
            }
        }
    } else {
        let mut source = HomeResolver::new(DiskHomeSource::new(bob_dir));
        pomodoro_themes::suggest::suggest_for_new_task(
            &mut source,
            catalog,
            &cfg,
            Path::new(&relative),
        )
    };
    warnings.extend(result.warnings.iter().map(|warning| {
        let suffix = warning.path.as_deref().map(|path| format!(" · {path}"));
        bounded_warning(format!(
            "{}{}",
            warning.code,
            suffix.unwrap_or_default()
        ))
    }));
    result.suggestion
}

fn menu_choice(
    suggestion: &ThemeSuggestion,
    scan: &capture_pomodoros::PomodoroScan,
    hint: Option<&PlanCreationHint>,
) -> MenuChoice {
    let exempt = hint.map_or_else(Vec::new, |hint| hint.exempt.clone());
    let entries = scan
        .entries
        .iter()
        .map(|entry| MenuEntry {
            name: entry.name.as_deref(),
            open: entry.state == PomodoroState::Open,
            running: entry.is_current,
        })
        .collect::<Vec<_>>();
    let creation_allowed = scan.has_section
        && scan
            .entries
            .iter()
            .filter(|entry| {
                entry.state == PomodoroState::Open && entry.time_range.is_some()
            })
            .count()
            <= 1;
    preselect::choose(
        &suggestion.theme,
        &entries,
        &exempt,
        creation_allowed,
        false,
    )
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
    let relative = day_file
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|year| {
            Path::new(year).join(day_file.file_name().unwrap_or_default())
        })
        .unwrap_or_else(|| day_file.to_path_buf());
    let next_up_line = start
        .then(|| unnamed_start_next_up_line(&scan, &contents, &relative))
        .flatten();
    Ok((
        Candidates::PomodoroName(theme_menu::compose(
            &scan,
            query,
            None,
            start,
            false,
            next_up_line,
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
    theme_menu::compose(scan, query, None, true, false, None)
}

fn entry_key(entry: &PomodoroEntry) -> String {
    plan_budget::normalize_component(entry.name.as_deref().unwrap_or_default())
}

fn day_relative_for(bob_dir: &Path, day_file: &Path) -> PathBuf {
    PathBuf::from(capture_pomodoros::relative_day_file(day_file, bob_dir))
}

fn unnamed_start_next_up_line(
    scan: &capture_pomodoros::PomodoroScan,
    contents: &str,
    day_relative: &Path,
) -> Option<usize> {
    let has_running = scan.entries.iter().any(|entry| {
        entry.state == PomodoroState::Open && entry.time_range.is_some()
    });
    if has_running {
        return capture_pomodoros::next_future_pomodoro(scan)
            .map(|entry| entry.line);
    }
    let spec = capture_language::PomodoroStartSpec {
        raw: String::new(),
        duration_units: 5,
        offset_units: 0,
        drop: Vec::new(),
    };
    capture::unnamed_start_target(
        day_relative,
        contents,
        scan,
        crate::native::env::current_datetime(),
        &spec,
    )
    .map(|entry| entry.line)
}

pub(super) fn today_start_rows(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
    next_up_line: Option<usize>,
) -> Vec<PomodoroNameCandidate> {
    let next_up_line = next_up_line.or_else(|| {
        capture_pomodoros::next_future_pomodoro(scan).map(|entry| entry.line)
    });

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
    // Deduplicate completed keys keeping the latest occurrence last in
    // document order, then list the most recent first so an empty query
    // resurfaces the last session first.
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
            again_entries.remove(position);
        }
        again_entries.push(entry);
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

/// Exempt-type context for a `creates_pomodoro` row: the shared PT12
/// preselection rule skips exempt components when matching entries.
/// `None` when the daily note has no Pomodoros section or the plan
/// config is unavailable.
pub(super) struct PlanCreationHint {
    pub(super) exempt: Vec<String>,
}

pub(super) fn plan_creation_hint(
    scan: &capture_pomodoros::PomodoroScan,
) -> Option<PlanCreationHint> {
    if !scan.has_section {
        return None;
    }
    let config = config::load_plan_config(&config::config_path()).ok()?;
    Some(PlanCreationHint {
        exempt: config
            .exempt()
            .iter()
            .map(|name| plan_budget::normalize_component(name))
            .collect(),
    })
}

#[cfg(test)]
pub(super) fn pomodoro_name_candidates_from_scan(
    scan: &capture_pomodoros::PomodoroScan,
    query: &str,
) -> Vec<PomodoroNameCandidate> {
    theme_menu::compose(scan, query, None, false, false, None)
}

pub(super) fn creation_candidate(
    scan: &capture_pomodoros::PomodoroScan,
    name: String,
) -> PomodoroNameCandidate {
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
        suggested: false,
        suggestion_label: None,
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
        suggested: false,
        suggestion_label: None,
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

fn day_file_for_completion(bob_dir: &Path) -> std::path::PathBuf {
    let today = crate::native::env::current_datetime().date();
    let canonical = bob_dir.join(format!(
        "{}/{}.md",
        today.format("%Y"),
        today.format("%Y%m%d")
    ));
    crate::native::env::var_os("BOB_DAY_FILE")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or(canonical)
}

fn open_merged_entries(
    scan: &capture_pomodoros::PomodoroScan,
    sources: &crate::native::config::themes::ThemeSources,
) -> Vec<(usize, String, String)> {
    scan.entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            if entry.state != capture_pomodoros::PomodoroState::Open {
                return None;
            }
            let name = entry.name.clone()?;
            let components =
                crate::native::pomodoro_themes::routing::type_components(
                    Some(&name),
                    sources,
                );
            let types: Vec<_> =
                components.into_iter().filter(|item| item.is_type).collect();
            if types.len() < 2 {
                return None;
            }
            let base = types.first()?.name.clone();
            Some((index, name, base))
        })
        .collect()
}

pub(super) fn pomodoro_split_base_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let day_file = day_file_for_completion(bob_dir);
    let (scan, _contents, mut warnings) = read_daily(&day_file)?;
    let today = crate::native::env::current_datetime().date();
    let catalog = crate::native::pomodoro_themes::Catalog::load_for_completion(
        bob_dir,
        today,
        Default::default(),
        (day_file
            == bob_dir.join(format!(
                "{}/{}.md",
                today.format("%Y"),
                today.format("%Y%m%d")
            )))
        .then_some(&scan),
    );
    let sources = match catalog {
        Ok(catalog) => {
            warnings.extend(
                catalog.warnings().iter().cloned().map(bounded_warning),
            );
            catalog.theme_sources()
        }
        Err(error) => {
            warnings
                .push(bounded_warning(format!("read theme catalog: {error}")));
            crate::native::config::themes::load_theme_sources()
        }
    };
    let merged = open_merged_entries(&scan, &sources);
    let query_lower = query.to_lowercase();
    let mut items: Vec<PomodoroNameCandidate> = Vec::new();
    items.push(PomodoroNameCandidate {
        replacement: String::new(),
        pomodoro_ref: None,
        name: Some("current".to_string()),
        themes: Vec::new(),
        origin: None,
        last_used: None,
        days_used: None,
        hidden: false,
        similar_to: Vec::new(),
        requires_name: false,
        creates_pomodoro: false,
        next_up: false,
        suggested: false,
        suggestion_label: None,
        line: None,
        state: capture_pomodoros::PomodoroState::Open,
        status_symbol: ' ',
        time_range: None,
        placeholder: true,
        is_current: true,
        child_count: 0,
        match_count: 0,
    });
    for (index, full_name, base) in merged {
        if !query.is_empty()
            && !base.to_lowercase().starts_with(&query_lower)
            && !full_name.to_lowercase().starts_with(&query_lower)
            && !full_name.to_lowercase().contains(&query_lower)
        {
            continue;
        }
        let entry = &scan.entries[index];
        let key = crate::native::plan_budget::normalize_component(&base);
        let theme_catalog = crate::native::pomodoro_themes::Catalog::load(
            bob_dir, today, false,
        )
        .ok();
        let themes = theme_catalog
            .as_ref()
            .map(|catalog| vec![catalog.theme_ref(&key, &base)])
            .unwrap_or_default();
        items.push(PomodoroNameCandidate {
            replacement: base,
            pomodoro_ref: Some(entry.pomodoro_ref.clone()),
            name: Some(full_name),
            themes,
            origin: None,
            last_used: None,
            days_used: None,
            hidden: false,
            similar_to: Vec::new(),
            requires_name: false,
            creates_pomodoro: false,
            next_up: false,
            suggested: false,
            suggestion_label: None,
            line: Some(entry.line),
            state: entry.state,
            status_symbol: entry.status_symbol,
            time_range: entry.time_range.clone(),
            placeholder: entry.placeholder,
            is_current: entry.is_current,
            child_count: entry.child_count,
            match_count: 0,
        });
    }
    Ok((Candidates::PomodoroName(items), warnings))
}

pub(super) fn pomodoro_split_type_candidates(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let day_file = day_file_for_completion(bob_dir);
    let (scan, contents, mut warnings) = read_daily(&day_file)?;
    let today = crate::native::env::current_datetime().date();
    let catalog = crate::native::pomodoro_themes::Catalog::load_for_completion(
        bob_dir,
        today,
        Default::default(),
        (day_file
            == bob_dir.join(format!(
                "{}/{}.md",
                today.format("%Y"),
                today.format("%Y%m%d")
            )))
        .then_some(&scan),
    );
    let sources = match catalog {
        Ok(catalog) => {
            warnings.extend(
                catalog.warnings().iter().cloned().map(bounded_warning),
            );
            catalog.theme_sources()
        }
        Err(error) => {
            warnings
                .push(bounded_warning(format!("read theme catalog: {error}")));
            crate::native::config::themes::load_theme_sources()
        }
    };
    let item = capture_language::editor_item_at(raw_text, cursor);
    let (base, selected): (Option<String>, Vec<String>) = item
        .and_then(|item| {
            item.pomodoro_split.map(|spec| {
                (
                    spec.base,
                    spec.selectors
                        .into_iter()
                        .map(|selector| selector.text)
                        .collect(),
                )
            })
        })
        .unwrap_or((None, Vec::new()));
    let entries: Vec<crate::native::pomodoro_themes::merged::SelectEntry<'_>> =
        scan.entries
            .iter()
            .map(
                |entry| crate::native::pomodoro_themes::merged::SelectEntry {
                    name: entry.name.as_deref(),
                    open: entry.state == capture_pomodoros::PomodoroState::Open,
                    running: entry.is_current,
                    timed: entry.time_range.is_some(),
                },
            )
            .collect();
    let target_index = if let Some(base_name) = base.as_deref() {
        let input = crate::native::pomodoro_themes::merged::SelectInput {
            entries: &entries,
            selector: base_name,
            filter:
                crate::native::pomodoro_themes::merged::SelectFilter::MergedBase,
            sources: &sources,
        };
        match crate::native::pomodoro_themes::merged::select_by_type(&input) {
            crate::native::pomodoro_themes::merged::SelectOutcome::Entry(
                index,
            ) => Some(index),
            _ => None,
        }
    } else {
        let timed: Vec<usize> = scan
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.state == capture_pomodoros::PomodoroState::Open
                    && entry.time_range.is_some()
            })
            .map(|(index, _)| index)
            .collect();
        if timed.len() == 1 {
            Some(timed[0])
        } else if timed.is_empty() {
            capture_pomodoros::next_future_pomodoro(&scan).and_then(|next| {
                scan.entries.iter().position(|entry| {
                    entry.line == next.line && entry.name == next.name
                })
            })
        } else {
            None
        }
    };
    let Some(target_index) = target_index else {
        return Ok((Candidates::PomodoroName(Vec::new()), warnings));
    };
    let target = &scan.entries[target_index];
    let target_name = target.name.clone().unwrap_or_default();
    let components = crate::native::pomodoro_themes::routing::type_components(
        Some(&target_name),
        &sources,
    );
    let merged_in: Vec<(String, String)> = {
        let types: Vec<_> =
            components.into_iter().filter(|item| item.is_type).collect();
        if types.len() < 2 {
            return Ok((Candidates::PomodoroName(Vec::new()), warnings));
        }
        if types.len() == 2 {
            return Ok((Candidates::PomodoroName(Vec::new()), warnings));
        }
        types
            .into_iter()
            .skip(1)
            .map(|item| (item.key, item.name))
            .collect()
    };
    let selected_keys: std::collections::BTreeSet<String> = selected
        .iter()
        .map(|name| crate::native::plan_budget::normalize_component(name))
        .collect();
    let line_texts: Vec<&str> = contents.lines().collect();
    let entry_index = target.line.checked_sub(1).unwrap_or(usize::MAX);
    let counts = if entry_index < line_texts.len() {
        let spans = capture::line_spans(&contents);
        let end = crate::native::capture_task_toggle::child_block_end_line(
            &spans,
            entry_index,
        );
        let parent_indent = line_texts
            .get(entry_index)
            .map(|text| text.len() - text.trim_start().len())
            .unwrap_or(0);
        let mut first_indent: Option<usize> = None;
        for index in
            (entry_index + 1)..=end.min(line_texts.len().saturating_sub(1))
        {
            let text = line_texts.get(index).copied().unwrap_or("");
            if text.trim().is_empty() {
                continue;
            }
            let indent = text.len() - text.trim_start().len();
            if indent <= parent_indent {
                continue;
            }
            if first_indent.is_none() {
                first_indent = Some(indent);
            }
        }
        let top_indent = first_indent.unwrap_or(usize::MAX);
        let mut top_lines = Vec::new();
        for index in
            (entry_index + 1)..=end.min(line_texts.len().saturating_sub(1))
        {
            let text = line_texts.get(index).copied().unwrap_or("");
            if text.trim().is_empty() {
                continue;
            }
            if text.len() - text.trim_start().len() == top_indent {
                top_lines.push(index);
            }
        }
        let fenced = crate::native::markdown::fenced_lines(
            &line_texts,
            0..line_texts.len(),
        );
        let start = top_lines.first().copied().unwrap_or(entry_index + 1);
        let numbered =
            crate::native::capture_pomodoro_close::number_task_links_in_range(
                &line_texts,
                &fenced,
                start
                    ..end
                        .min(line_texts.len())
                        .saturating_add(1)
                        .min(line_texts.len()),
            );
        let top_set: std::collections::BTreeSet<usize> =
            top_lines.into_iter().collect();
        let mut planner =
            crate::native::capture::CaptureBatchPlanner::default();
        let mut theme_ctx =
            crate::native::capture::CaptureTaskThemeContext::new(
                bob_dir, today, None,
            );
        let mut counts = std::collections::BTreeMap::new();
        for link in numbered {
            let zero = link.line.checked_sub(1).unwrap_or(usize::MAX);
            if !top_set.contains(&zero) {
                continue;
            }
            let target_path = if link.path_part.is_empty() {
                day_file.clone()
            } else {
                bob_dir.join(format!("{}.md", link.path_part))
            };
            let residence = if link.path_part.is_empty() {
                day_file
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("today")
                    .to_string()
            } else {
                link.path_part.clone()
            };
            let task_contents = std::fs::read_to_string(&target_path).ok();
            let Some(task_contents) = task_contents else {
                continue;
            };
            let settings = crate::native::note_tasks::read_settings(bob_dir);
            let scan =
                crate::native::note_tasks::scan(&task_contents, &settings);
            let task_line = match scan.by_block_id(&link.block_id) {
                crate::native::note_tasks::BlockIdLookup::Found(task) => {
                    task_contents
                        .lines()
                        .nth(task.line_index)
                        .unwrap_or("")
                        .to_string()
                }
                _ => continue,
            };
            let mut placement_warnings = Vec::new();
            let key = theme_ctx
                .placement_for_task(
                    &mut planner,
                    &residence,
                    &task_line,
                    &mut placement_warnings,
                )
                .map(|placed| placed.suggestion.theme.key.clone());
            for warning in placement_warnings {
                let message = warning.message().into_owned();
                if !warnings.contains(&message) {
                    warnings.push(message);
                }
            }
            if let Some(key) = key
                && merged_in.iter().any(|(merged_key, _)| merged_key == &key)
            {
                *counts.entry(key).or_insert(0) += 1;
            }
        }
        counts
    } else {
        Default::default()
    };
    let query_lower = query.to_lowercase();
    let is_literal =
        raw_text
            .get(..cursor.min(raw_text.len()))
            .is_some_and(|prefix| {
                let hash = prefix.rfind('#');
                hash.is_some_and(|hash| {
                    prefix[hash + 1..].starts_with(['"', '“'])
                })
            });
    let catalog_for_refs =
        crate::native::pomodoro_themes::Catalog::load(bob_dir, today, false)
            .ok();
    let mut items = Vec::new();
    for (key, name) in merged_in {
        if selected_keys.contains(&key) {
            continue;
        }
        if !query.is_empty() {
            if is_literal {
                if key.to_lowercase() != query_lower {
                    continue;
                }
            } else if !key.to_lowercase().starts_with(&query_lower)
                && !name.to_lowercase().starts_with(&query_lower)
            {
                continue;
            }
        }
        let themes = catalog_for_refs
            .as_ref()
            .map(|catalog| vec![catalog.theme_ref(&key, &name)])
            .unwrap_or_default();
        let count = counts.get(&key).copied().unwrap_or(0);
        items.push(PomodoroNameCandidate {
            replacement: name.clone(),
            pomodoro_ref: None,
            name: Some(format!(
                "{name} · {}",
                if count == 0 {
                    "no tasks".to_string()
                } else if count == 1 {
                    "1 task".to_string()
                } else {
                    format!("{count} tasks")
                }
            )),
            themes,
            origin: None,
            last_used: None,
            days_used: None,
            hidden: false,
            similar_to: Vec::new(),
            requires_name: false,
            creates_pomodoro: false,
            next_up: false,
            suggested: false,
            suggestion_label: None,
            line: Some(target.line),
            state: target.state,
            status_symbol: target.status_symbol,
            time_range: target.time_range.clone(),
            placeholder: target.placeholder,
            is_current: target.is_current,
            child_count: count,
            match_count: count,
        });
    }
    Ok((Candidates::PomodoroName(items), warnings))
}

fn merge_display_label(
    selector: &str,
    full: &str,
    state: &str,
    links: usize,
) -> String {
    let links = if links == 0 {
        "no links".to_string()
    } else if links == 1 {
        "1 link".to_string()
    } else {
        format!("{links} links")
    };
    format!("{selector} → {full} ({state} · {links})")
}

fn first_type_name(
    name: &str,
    sources: &crate::native::config::themes::ThemeSources,
) -> String {
    crate::native::pomodoro_themes::routing::type_components(
        Some(name),
        sources,
    )
    .into_iter()
    .find(|item| item.is_type)
    .map(|item| item.name)
    .unwrap_or_else(|| name.to_string())
}

pub(super) fn pomodoro_merge_src_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let day_file = day_file_for_completion(bob_dir);
    let (scan, _contents, mut warnings) = read_daily(&day_file)?;
    let today = crate::native::env::current_datetime().date();
    let catalog = crate::native::pomodoro_themes::Catalog::load_for_completion(
        bob_dir,
        today,
        Default::default(),
        (day_file
            == bob_dir.join(format!(
                "{}/{}.md",
                today.format("%Y"),
                today.format("%Y%m%d")
            )))
        .then_some(&scan),
    );
    let sources = match catalog {
        Ok(catalog) => {
            warnings.extend(
                catalog.warnings().iter().cloned().map(bounded_warning),
            );
            catalog.theme_sources()
        }
        Err(error) => {
            warnings
                .push(bounded_warning(format!("read theme catalog: {error}")));
            crate::native::config::themes::load_theme_sources()
        }
    };
    let query_lower = query.to_lowercase();
    let mut items: Vec<PomodoroNameCandidate> = Vec::new();
    items.push(PomodoroNameCandidate {
        replacement: String::new(),
        pomodoro_ref: None,
        name: Some(match capture_pomodoros::current_pomodoro(&scan) {
            capture_pomodoros::CurrentPomodoro::Running(index)
            | capture_pomodoros::CurrentPomodoro::NextUp(index) => scan
                .entries
                .get(index)
                .and_then(|entry| {
                    entry.name.as_deref().map(|full| (entry, full))
                })
                .map(|(entry, full)| {
                    merge_display_label(
                        "current",
                        full,
                        if entry.time_range.is_some() {
                            "running"
                        } else {
                            "planned"
                        },
                        entry.child_count,
                    )
                })
                .unwrap_or_else(|| "current".to_string()),
            capture_pomodoros::CurrentPomodoro::Ambiguous
            | capture_pomodoros::CurrentPomodoro::Missing => {
                "current".to_string()
            }
        }),
        themes: Vec::new(),
        origin: None,
        last_used: None,
        days_used: None,
        hidden: false,
        similar_to: Vec::new(),
        requires_name: false,
        creates_pomodoro: false,
        next_up: false,
        suggested: false,
        suggestion_label: None,
        line: None,
        state: capture_pomodoros::PomodoroState::Open,
        status_symbol: ' ',
        time_range: None,
        placeholder: true,
        is_current: true,
        child_count: 0,
        match_count: 0,
    });
    for entry in scan.entries.iter() {
        if entry.state != capture_pomodoros::PomodoroState::Open {
            continue;
        }
        let Some(full_name) = entry.name.clone() else {
            continue;
        };
        let selector = first_type_name(&full_name, &sources);
        if !query.is_empty()
            && !selector.to_lowercase().starts_with(&query_lower)
            && !full_name.to_lowercase().starts_with(&query_lower)
            && !full_name.to_lowercase().contains(&query_lower)
        {
            continue;
        }
        let running = entry.is_current || entry.time_range.is_some();
        items.push(PomodoroNameCandidate {
            replacement: selector.clone(),
            pomodoro_ref: Some(entry.pomodoro_ref.clone()),
            name: Some(merge_display_label(
                &selector,
                &full_name,
                if running { "running" } else { "planned" },
                entry.child_count,
            )),
            themes: Vec::new(),
            origin: None,
            last_used: None,
            days_used: None,
            hidden: false,
            similar_to: Vec::new(),
            requires_name: false,
            creates_pomodoro: false,
            next_up: false,
            suggested: false,
            suggestion_label: None,
            line: Some(entry.line),
            state: entry.state,
            status_symbol: entry.status_symbol,
            time_range: entry.time_range.clone(),
            placeholder: entry.placeholder,
            is_current: entry.is_current,
            child_count: entry.child_count,
            match_count: 0,
        });
    }
    Ok((Candidates::PomodoroName(items), warnings))
}

pub(super) fn pomodoro_merge_dest_candidates(
    bob_dir: &Path,
    raw_text: &str,
    cursor: usize,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let day_file = day_file_for_completion(bob_dir);
    let (scan, _contents, mut warnings) = read_daily(&day_file)?;
    let today = crate::native::env::current_datetime().date();
    let catalog = crate::native::pomodoro_themes::Catalog::load_for_completion(
        bob_dir,
        today,
        Default::default(),
        (day_file
            == bob_dir.join(format!(
                "{}/{}.md",
                today.format("%Y"),
                today.format("%Y%m%d")
            )))
        .then_some(&scan),
    );
    let sources = match catalog {
        Ok(catalog) => {
            warnings.extend(
                catalog.warnings().iter().cloned().map(bounded_warning),
            );
            catalog.theme_sources()
        }
        Err(error) => {
            warnings
                .push(bounded_warning(format!("read theme catalog: {error}")));
            crate::native::config::themes::load_theme_sources()
        }
    };
    let item = capture_language::editor_item_at(raw_text, cursor);
    let (src, selected): (Option<String>, Vec<String>) = item
        .and_then(|item| {
            item.pomodoro_merge.map(|spec| {
                (
                    spec.src,
                    spec.dests.into_iter().map(|dest| dest.text).collect(),
                )
            })
        })
        .unwrap_or((None, Vec::new()));
    let entries: Vec<crate::native::pomodoro_themes::merged::SelectEntry<'_>> =
        scan.entries
            .iter()
            .map(
                |entry| crate::native::pomodoro_themes::merged::SelectEntry {
                    name: entry.name.as_deref(),
                    open: entry.state == capture_pomodoros::PomodoroState::Open,
                    running: entry.is_current,
                    timed: entry.time_range.is_some(),
                },
            )
            .collect();
    let src_index = if let Some(src_name) = src.as_deref() {
        let input = crate::native::pomodoro_themes::merged::SelectInput {
            entries: &entries,
            selector: src_name,
            filter: crate::native::pomodoro_themes::merged::SelectFilter::Open,
            sources: &sources,
        };
        match crate::native::pomodoro_themes::merged::select_by_type(&input) {
            crate::native::pomodoro_themes::merged::SelectOutcome::Entry(
                index,
            ) => Some(index),
            _ => None,
        }
    } else {
        match capture_pomodoros::current_pomodoro(&scan) {
            capture_pomodoros::CurrentPomodoro::Running(index)
            | capture_pomodoros::CurrentPomodoro::NextUp(index) => Some(index),
            capture_pomodoros::CurrentPomodoro::Ambiguous
            | capture_pomodoros::CurrentPomodoro::Missing => None,
        }
    };
    let selected_keys: std::collections::BTreeSet<String> = selected
        .iter()
        .map(|name| crate::native::plan_budget::normalize_component(name))
        .collect();
    let mut selected_indices = Vec::new();
    for selector in &selected {
        let untimed = crate::native::pomodoro_themes::merged::SelectInput {
            entries: &entries,
            selector,
            filter: crate::native::pomodoro_themes::merged::SelectFilter::UntimedPlanned {
                except: src_index,
            },
            sources: &sources,
        };
        if let crate::native::pomodoro_themes::merged::SelectOutcome::Entry(
            index,
        ) = crate::native::pomodoro_themes::merged::select_by_type(&untimed)
        {
            selected_indices.push(index);
        }
    }
    let next_index = src_index.and_then(|src_index| {
        match crate::native::pomodoro_themes::merged::next_planned_below(
            &entries,
            src_index,
            &selected_indices,
        ) {
            crate::native::pomodoro_themes::merged::NextPlanned::Entry(
                index,
            ) => Some(index),
            crate::native::pomodoro_themes::merged::NextPlanned::Missing {
                ..
            } => None,
        }
    });
    let query_lower = query.to_lowercase();
    let mut items = Vec::new();
    let mut order: Vec<usize> = Vec::with_capacity(scan.entries.len());
    if let Some(index) = next_index {
        order.push(index);
    }
    order.extend(
        scan.entries.iter().enumerate().filter_map(|(index, _)| {
            (Some(index) != next_index).then_some(index)
        }),
    );
    for index in order {
        let Some(entry) = scan.entries.get(index) else {
            continue;
        };
        if entry.state != capture_pomodoros::PomodoroState::Open {
            continue;
        }
        if entry.time_range.is_some() || entry.is_current {
            continue;
        }
        if src_index == Some(index) {
            continue;
        }
        let Some(full_name) = entry.name.clone() else {
            continue;
        };
        let selector = first_type_name(&full_name, &sources);
        let key = crate::native::plan_budget::normalize_component(&selector);
        if selected_keys.contains(&key) {
            continue;
        }
        if !query.is_empty()
            && !selector.to_lowercase().starts_with(&query_lower)
            && !full_name.to_lowercase().starts_with(&query_lower)
            && !full_name.to_lowercase().contains(&query_lower)
        {
            continue;
        }
        items.push(PomodoroNameCandidate {
            replacement: selector.clone(),
            pomodoro_ref: Some(entry.pomodoro_ref.clone()),
            name: Some(merge_display_label(
                &selector,
                &full_name,
                if next_index == Some(index) {
                    "next planned"
                } else {
                    "planned"
                },
                entry.child_count,
            )),
            themes: Vec::new(),
            origin: None,
            last_used: None,
            days_used: None,
            hidden: false,
            similar_to: Vec::new(),
            requires_name: false,
            creates_pomodoro: false,
            next_up: false,
            suggested: false,
            suggestion_label: None,
            line: Some(entry.line),
            state: entry.state,
            status_symbol: entry.status_symbol,
            time_range: entry.time_range.clone(),
            placeholder: entry.placeholder,
            is_current: entry.is_current,
            child_count: entry.child_count,
            match_count: 0,
        });
    }
    Ok((Candidates::PomodoroName(items), warnings))
}
