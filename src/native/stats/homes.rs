//! Indexed homes engine for stats: session splits, catalog, and health.

use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap},
    path::Path,
};

use chrono::{Datelike, NaiveDate};

use super::{
    clock::StatsClock,
    index::{PomodoroSession, StatsIndex},
    report::{
        HomeCatalogEntry, HomeLintReport, HomesHealthReport,
        HueCollisionReport, OverlapReport, RootFieldInvalidReport,
        RootFieldNotRootReport, SessionFact, StatsWarning,
    },
    series::format_minutes,
    session_facts::session_fact,
};
use crate::native::{
    capture::ClockStamp,
    event_task::{clock_verdict, event_task_by_id, ClockVerdict, LinkFact},
    note_tasks::{self, BlockIdLookup, NoteTaskScan, NoteTaskSettings},
    pomodoro_homes::{
        attribute_session, effective_clock, path_string, unfiled_home,
        unlinked_home, Home, HomeKind, HomeResolver, Hue, IndexedHomeSource,
        NoteKind, ResolverHealth, SessionHomes, SessionKind, SessionLink,
    },
    vault_links::LinkResolution,
};

pub(crate) struct HomesEngine<'a> {
    resolver: RefCell<HomeResolver<IndexedHomeSource<'a>>>,
    seen: RefCell<BTreeMap<String, Home>>,
}

pub(crate) struct SessionSplit {
    pub(crate) kind: SessionKind,
    pub(crate) project: Vec<(String, i64)>,
    pub(crate) area: Vec<(String, i64)>,
    pub(crate) root_parent: Vec<(String, i64)>,
    pub(crate) project_apportioned: Vec<(String, i64)>,
    pub(crate) area_apportioned: Vec<(String, i64)>,
    pub(crate) root_parent_apportioned: Vec<(String, i64)>,
    pub(crate) lead_project: Option<String>,
    pub(crate) lead_area: Option<String>,
    pub(crate) lead_root_parent: Option<String>,
}

impl<'a> HomesEngine<'a> {
    pub(crate) fn new(index: &'a StatsIndex) -> Self {
        let source =
            IndexedHomeSource::from_buffers(&index.note_index, &index.buffers);
        Self {
            resolver: RefCell::new(HomeResolver::new(source)),
            seen: RefCell::new(BTreeMap::new()),
        }
    }

    pub(crate) fn home_for_note(&self, path: &str) -> Home {
        let home = if path.is_empty() {
            unfiled_home()
        } else {
            self.resolver.borrow_mut().resolve_note(Path::new(path))
        };
        self.remember(&home);
        home
    }

    pub(crate) fn home_for_task(
        &self,
        path: &str,
        line_number: usize,
        line: &str,
    ) -> Home {
        if path.is_empty() {
            return self.home_for_note(path);
        }
        let home = self.resolver.borrow_mut().home_for_task(
            Path::new(path),
            line_number,
            line,
        );
        self.remember(&home);
        home
    }

    pub(crate) fn attribute(
        &self,
        session: &PomodoroSession,
        daily_path: &Path,
        counted_minutes: Option<u32>,
    ) -> SessionHomes {
        let attributed = attribute_session(
            session.name.as_deref(),
            &session.links,
            true,
            counted_minutes,
            session.untracked,
            daily_path,
            &mut self.resolver.borrow_mut(),
        );
        self.record_session(&attributed);
        attributed
    }

    pub(crate) fn finish(
        &self,
        index: &StatsIndex,
        start: NaiveDate,
        end: NaiveDate,
        collect_sessions: bool,
        clock: StatsClock,
        bob_dir: &Path,
    ) -> (
        BTreeMap<String, HomeCatalogEntry>,
        HomesHealthReport,
        Vec<StatsWarning>,
        Vec<SessionFact>,
    ) {
        let mut top_areas: BTreeMap<Hue, Vec<(String, String)>> =
            BTreeMap::new();
        // Resolve under one borrow, then remember after it drops:
        // remember -> ensure_root re-borrows the resolver for unseen roots.
        let mut inspected = Vec::new();
        {
            let mut resolver = self.resolver.borrow_mut();
            for (path, contents) in &index.buffers {
                if !needs_global_inspect(contents) {
                    continue;
                }
                let path = Path::new(path);
                let Some(facts) = resolver.facts(path) else {
                    continue;
                };
                match facts.kind {
                    NoteKind::Project => {
                        inspected.push(resolver.resolve_note(path));
                    }
                    NoteKind::Area => {
                        let home = resolver.resolve_note(path);
                        let path_key = path_string(path);
                        if home.area == path_key
                            && let Some(hue) = home.hue
                        {
                            top_areas
                                .entry(hue)
                                .or_default()
                                .push((home.label.clone(), path_key));
                        }
                        inspected.push(home);
                    }
                    NoteKind::Other => {}
                }
            }
        }
        for home in inspected {
            self.remember(&home);
        }

        let mut health = HomesHealthReport::default();

        let mut collisions = Vec::new();
        for (hue, mut areas) in top_areas {
            if areas.len() < 2 {
                continue;
            }
            areas.sort_by(|left, right| left.0.cmp(&right.0));
            collisions.push(HueCollisionReport {
                hue,
                areas: areas.iter().map(|(label, _)| label.clone()).collect(),
            });
        }
        health.hue_collisions = collisions;

        // Writer-matching lookup facts from the already indexed buffers:
        // `note_tasks` scans (ordinary vs embedded) plus same-note events.
        let task_lookup =
            ClockTaskLookup::from_buffers(&index.buffers, bob_dir);

        let mut sessions = Vec::new();
        let mut intervals: Vec<OverlapSpan> = Vec::new();
        for (date, note) in &index.daily_notes {
            if *date < start || *date > end {
                continue;
            }
            let daily_path = Path::new(&note.path);
            for session in &note.closed_pomodoros {
                // Clock diagnostics cover closed timed sessions and ignore
                // the request lens, so every view reports the same window.
                if let Some(session_clock) = session.stats_clock() {
                    if session_clock == StatsClock::OffClock {
                        health.off_clock_sessions =
                            health.off_clock_sessions.saturating_add(1);
                        health.untracked_sessions =
                            health.untracked_sessions.saturating_add(1);
                    }
                    match session.stamp {
                        ClockStamp::Bare => {
                            health.unstamped_sessions =
                                health.unstamped_sessions.saturating_add(1);
                        }
                        ClockStamp::Conflict => {
                            health.clock_conflicts =
                                health.clock_conflicts.saturating_add(1);
                        }
                        ClockStamp::Keyed => {}
                    }
                    if session.stamp == ClockStamp::Keyed {
                        let mut resolver = self.resolver.borrow_mut();
                        if session_drifted(
                            session,
                            daily_path,
                            &task_lookup,
                            &mut resolver,
                        ) {
                            health.clock_drift =
                                health.clock_drift.saturating_add(1);
                        }
                    }
                    if let Some((overlap_start, overlap_end)) =
                        parse_session_interval(session.time_range.as_deref())
                    {
                        intervals.push(OverlapSpan {
                            date: *date,
                            path: note.path.clone(),
                            line: session.line,
                            start: overlap_start,
                            end: overlap_end,
                        });
                    }
                }
                let minutes = session.counted_minutes();
                let attributed = self.attribute(session, daily_path, minutes);
                let show = collect_sessions
                    && match session.stats_clock() {
                        Some(session_clock) => match clock {
                            StatsClock::Worked => {
                                session_clock == StatsClock::Worked
                            }
                            StatsClock::OffClock => {
                                session_clock == StatsClock::OffClock
                            }
                            StatsClock::Total => true,
                        },
                        None => false,
                    };
                if show {
                    sessions.push(session_fact(
                        *date,
                        &note.path,
                        session,
                        &attributed,
                    ));
                }
                match attributed.homes.kind {
                    SessionKind::Unlinked => {
                        health.unlinked_sessions =
                            health.unlinked_sessions.saturating_add(1);
                        if let Some(minutes) = minutes {
                            health.unlinked_minutes =
                                health.unlinked_minutes.saturating_add(minutes);
                        }
                    }
                    SessionKind::Work => {
                        // Session names never choose a home: every closed
                        // session attributes through its Task Links.
                        let (unfiled, ambiguous, unresolved) = {
                            let mut resolver = self.resolver.borrow_mut();
                            link_health(session, daily_path, &mut resolver)
                        };
                        health.unfiled_links =
                            health.unfiled_links.saturating_add(unfiled);
                        health.ambiguous_links =
                            health.ambiguous_links.saturating_add(ambiguous);
                        health.unresolved_links =
                            health.unresolved_links.saturating_add(unresolved);
                    }
                }
            }
        }
        let (overlap_minutes, overlaps) = detect_overlaps(intervals);
        health.overlap_minutes = overlap_minutes;
        health.overlaps = overlaps;

        let needed_areas: Vec<String> = self
            .seen
            .borrow()
            .values()
            .map(|home| home.area.clone())
            .filter(|area| !area.starts_with('@'))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut extra_areas = Vec::new();
        {
            let mut resolver = self.resolver.borrow_mut();
            for area in needed_areas {
                if self.seen.borrow().contains_key(&area) {
                    continue;
                }
                extra_areas.push(resolver.resolve_note(Path::new(&area)));
            }
        }
        for home in extra_areas {
            self.remember(&home);
        }

        // Clock and root-field lookups can add resolver lints while sessions
        // are evaluated, so snapshot health after the final resolver use.
        {
            let resolver = self.resolver.borrow();
            let resolver_health: ResolverHealth = resolver.health().clone();
            health.area_color_invalid = resolver_health
                .area_color_invalid
                .iter()
                .map(|lint| HomeLintReport {
                    path: lint.path.clone(),
                    raw: lint.raw.clone(),
                    key: lint.key.clone(),
                })
                .collect();
            health.note_offclock_invalid = resolver_health
                .note_offclock_invalid
                .iter()
                .map(|lint| HomeLintReport {
                    path: lint.path.clone(),
                    raw: lint.raw.clone(),
                    key: lint.key.clone(),
                })
                .collect();
            health.note_offclock_ignored = resolver_health
                .note_offclock_ignored
                .iter()
                .map(|lint| HomeLintReport {
                    path: lint.path.clone(),
                    raw: lint.raw.clone(),
                    key: lint.key.clone(),
                })
                .collect();
            health.note_offclock_shadowed = resolver_health
                .note_offclock_shadowed
                .iter()
                .map(|lint| HomeLintReport {
                    path: lint.path.clone(),
                    raw: lint.raw.clone(),
                    key: lint.key.clone(),
                })
                .collect();
            health.root_field_invalid = resolver_health
                .root_field_invalid
                .iter()
                .map(|lint| RootFieldInvalidReport {
                    path: lint.path.clone(),
                    line: lint.line,
                    raw: lint.raw.clone(),
                    reason: lint.reason.clone(),
                })
                .collect();
            health.root_field_not_root = resolver_health
                .root_field_not_root
                .iter()
                .map(|lint| RootFieldNotRootReport {
                    path: lint.path.clone(),
                    line: lint.line,
                    raw: lint.raw.clone(),
                    root: lint.root.clone(),
                })
                .collect();
        }

        let mut catalog = BTreeMap::new();
        for (key, home) in self.seen.borrow().iter() {
            catalog.insert(key.clone(), catalog_entry(home));
        }

        let warnings = homes_warnings(&health);
        (catalog, health, warnings, sessions)
    }

    fn record_session(&self, attributed: &SessionHomes) {
        for home in &attributed.task_homes {
            self.remember(home);
        }
        if let Some(home) = &attributed.lead_home {
            self.remember(home);
        }
        for part in &attributed.homes.parts {
            if let Some(area) = &part.area {
                self.needed_area_stub(area, part.area_label.as_deref());
            }
        }
    }

    fn remember(&self, home: &Home) {
        self.seen
            .borrow_mut()
            .entry(home.key.clone())
            .or_insert_with(|| home.clone());
        if home.area.starts_with('@') {
            self.needed_area_stub(&home.area, Some(&home.area_label));
        }
        self.ensure_root(home);
    }

    fn ensure_root(&self, home: &Home) {
        // Only `unlinked` and `unfiled` remain as reserved buckets; every
        // other root is a real note resolved through the note tree.
        if home.root_parent == home.key || home.root_parent.starts_with('@') {
            return;
        }
        if self.seen.borrow().contains_key(&home.root_parent) {
            return;
        }
        let root_home = self
            .resolver
            .borrow_mut()
            .resolve_note(Path::new(&home.root_parent));
        self.remember(&root_home);
    }

    fn needed_area_stub(&self, area: &str, label: Option<&str>) {
        if self.seen.borrow().contains_key(area) {
            return;
        }
        if area.starts_with('@') {
            self.seen.borrow_mut().insert(
                area.to_string(),
                bucket_home(
                    area,
                    label.unwrap_or(area.trim_start_matches('@')),
                ),
            );
        }
    }
}

/// Parse facts only for notes that can affect hue or area color.
/// Refs and dailies without those keys stay unread in the global pass.
fn needs_global_inspect(contents: &str) -> bool {
    let Some(head) = yaml_frontmatter_head(contents) else {
        return false;
    };
    head.contains("[[area]]") || head.contains("[[project]]")
}

fn yaml_frontmatter_head(contents: &str) -> Option<&str> {
    let rest = contents.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---").or_else(|| rest.find("\r\n---"))?;
    Some(&rest[..end])
}

fn bucket_home(key: &str, label: &str) -> Home {
    match key {
        "@unlinked" => unlinked_home(),
        "@unfiled" => unfiled_home(),
        _ => Home {
            key: key.to_string(),
            label: label.to_string(),
            kind: HomeKind::Bucket,
            area: key.to_string(),
            area_label: label.to_string(),
            subarea: None,
            hue: None,
            hue_source: None,
            root_hue: None,
            residence_chain: Vec::new(),
            root_parent: key.to_string(),
        },
    }
}

fn catalog_entry(home: &Home) -> HomeCatalogEntry {
    let path = (!home.key.starts_with('@')).then(|| home.key.clone());
    let project_label = (home.kind == HomeKind::Area)
        .then(|| format!("{} · area tasks", home.label));
    HomeCatalogEntry {
        label: home.label.clone(),
        kind: home.kind,
        area: Some(home.area.clone()),
        subarea: home.subarea.clone(),
        path,
        hue: home.hue,
        hue_source: home.hue_source,
        project_label,
        root_parent: home.root_parent.clone(),
    }
}

pub(crate) fn split_from_kept(
    kept: &[(Home, u32)],
    minutes_mode: bool,
) -> SessionSplit {
    let mut grouped = Vec::<KeptGroup>::new();
    let mut indices = HashMap::<String, usize>::new();
    for (index, (home, minutes)) in kept.iter().enumerate() {
        let group_index = if let Some(group_index) = indices.get(&home.key) {
            *group_index
        } else {
            let group_index = grouped.len();
            indices.insert(home.key.clone(), group_index);
            grouped.push(KeptGroup {
                home: home.clone(),
                tasks: 0,
                minutes: 0,
                first: index,
            });
            group_index
        };
        let group = &mut grouped[group_index];
        group.tasks += 1;
        group.minutes = group.minutes.saturating_add(*minutes);
    }
    let mut project: Vec<(String, i64)> = Vec::new();
    let mut area: Vec<(String, i64)> = Vec::new();
    let mut root_parent: Vec<(String, i64)> = Vec::new();
    if minutes_mode {
        for group in &grouped {
            project.push((group.home.key.clone(), i64::from(group.minutes)));
            add_named(&mut area, &group.home.area, i64::from(group.minutes));
            add_named(
                &mut root_parent,
                &group.home.root_parent,
                i64::from(group.minutes),
            );
        }
    }
    // Root task counts come from distinct kept tasks, not grouped homes.
    let mut root_counts: HashMap<String, (u32, usize)> = HashMap::new();
    for (index, (home, _)) in kept.iter().enumerate() {
        let entry = root_counts
            .entry(home.root_parent.clone())
            .or_insert((0, index));
        entry.0 += 1;
        entry.1 = entry.1.min(index);
    }
    if !minutes_mode {
        root_parent = Vec::new();
    }
    let project_keys = project
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
    let area_keys = area
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
    let root_keys = if minutes_mode {
        root_parent
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<BTreeSet<_>>()
    } else {
        root_counts
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    };
    let project_apportioned = if project_keys.len() >= 2 {
        project.clone()
    } else {
        Vec::new()
    };
    let area_apportioned = if area_keys.len() >= 2 {
        area.clone()
    } else {
        Vec::new()
    };
    let root_parent_apportioned = if root_keys.len() >= 2 && minutes_mode {
        root_parent.clone()
    } else {
        Vec::new()
    };
    let lead = grouped
        .iter()
        .max_by(|left, right| {
            left.tasks
                .cmp(&right.tasks)
                .then_with(|| right.first.cmp(&left.first))
        })
        .map(|group| group.home.clone());
    let lead_root_parent = root_counts
        .into_iter()
        .max_by(|left, right| {
            left.1
                 .0
                .cmp(&right.1 .0)
                .then_with(|| right.1 .1.cmp(&left.1 .1))
        })
        .map(|(key, _)| key);
    SessionSplit {
        kind: SessionKind::Work,
        project,
        area,
        root_parent,
        project_apportioned,
        area_apportioned,
        root_parent_apportioned,
        lead_project: lead.as_ref().map(|home| home.key.clone()),
        lead_area: lead.as_ref().map(|home| home.area.clone()),
        lead_root_parent,
    }
}

struct KeptGroup {
    home: Home,
    tasks: u32,
    minutes: u32,
    first: usize,
}

pub(crate) fn split_from_homes(
    attributed: &SessionHomes,
    minutes_mode: bool,
) -> SessionSplit {
    let mut project = Vec::new();
    let mut area = Vec::new();
    for part in &attributed.homes.parts {
        let minutes = if minutes_mode {
            i64::from(part.minutes.unwrap_or(0))
        } else {
            0
        };
        if minutes_mode {
            project.push((part.key.clone(), minutes));
            if let Some(area_key) = &part.area {
                add_named(&mut area, area_key, minutes);
            }
        }
    }
    // Root parts group per-task shares by root, never by splitting the
    // session across roots directly.
    let mut root_parent: Vec<(String, i64)> = Vec::new();
    let mut root_counts: HashMap<String, (u32, usize)> = HashMap::new();
    if !attributed.task_homes.is_empty() {
        for (index, home) in attributed.task_homes.iter().enumerate() {
            let entry = root_counts
                .entry(home.root_parent.clone())
                .or_insert((0, index));
            entry.0 += 1;
            entry.1 = entry.1.min(index);
        }
        if minutes_mode {
            let total: u32 = attributed
                .homes
                .parts
                .iter()
                .filter_map(|part| part.minutes)
                .sum();
            let shares = crate::native::pomodoro_homes::split_minutes(
                total,
                attributed.task_homes.len(),
            );
            for (index, home) in attributed.task_homes.iter().enumerate() {
                let minutes =
                    i64::from(shares.get(index).copied().unwrap_or(0));
                if let Some(existing) = root_parent
                    .iter_mut()
                    .find(|(key, _)| *key == home.root_parent)
                {
                    existing.1 = existing.1.saturating_add(minutes);
                } else {
                    root_parent.push((home.root_parent.clone(), minutes));
                }
            }
        }
    } else if minutes_mode {
        for part in &attributed.homes.parts {
            let minutes = i64::from(part.minutes.unwrap_or(0));
            let root_key = attributed
                .lead_home
                .as_ref()
                .map_or(part.key.as_str(), |home| home.root_parent.as_str());
            add_named(&mut root_parent, root_key, minutes);
            let first = root_counts.len();
            let entry = root_counts
                .entry(root_key.to_string())
                .or_insert((0, first));
            entry.0 += 1;
        }
    } else if let Some(lead) = attributed.lead_home.as_ref() {
        root_counts
            .entry(lead.root_parent.clone())
            .or_insert((1, 0));
    }
    if !minutes_mode && attributed.task_homes.is_empty() {
        // Count-mode buckets keep the lead root from the bucket home.
    }
    let project_keys = project
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
    let area_keys = area
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
    let root_keys = if minutes_mode {
        root_parent
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<BTreeSet<_>>()
    } else {
        root_counts
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    };
    let project_apportioned = if project_keys.len() >= 2 {
        project.clone()
    } else {
        Vec::new()
    };
    let area_apportioned = if area_keys.len() >= 2 {
        area.clone()
    } else {
        Vec::new()
    };
    let root_parent_apportioned = if root_keys.len() >= 2 && minutes_mode {
        root_parent.clone()
    } else {
        Vec::new()
    };
    let lead = attributed.lead_home.as_ref();
    let lead_root_parent = if attributed.task_homes.is_empty() {
        lead.map(|home| home.root_parent.clone())
    } else {
        root_counts
            .into_iter()
            .max_by(|left, right| {
                left.1
                     .0
                    .cmp(&right.1 .0)
                    .then_with(|| right.1 .1.cmp(&left.1 .1))
            })
            .map(|(key, _)| key)
    };
    SessionSplit {
        kind: attributed.homes.kind,
        project,
        area,
        root_parent,
        project_apportioned,
        area_apportioned,
        root_parent_apportioned,
        lead_project: lead.map(|home| home.key.clone()),
        lead_area: lead.map(|home| home.area.clone()),
        lead_root_parent,
    }
}

fn add_named(parts: &mut Vec<(String, i64)>, key: &str, value: i64) {
    if let Some(existing) = parts.iter_mut().find(|(name, _)| name == key) {
        existing.1 = existing.1.saturating_add(value);
    } else {
        parts.push((key.to_string(), value));
    }
}

/// In-memory task lookup matching the counted-link writer's
/// `resolve_clock_fact`: ordinary vs embedded `note_tasks` scans over the
/// already indexed buffers, plus same-note Event Tasks. Frontmatter and
/// fenced examples are not tasks. Duplicate block ids resolve to no line.
struct ClockTaskLookup<'a> {
    contents: HashMap<String, &'a str>,
    ordinary: RefCell<HashMap<String, NoteTaskScan>>,
    embedded: RefCell<HashMap<String, NoteTaskScan>>,
    settings: NoteTaskSettings,
    all_settings: NoteTaskSettings,
}

impl<'a> ClockTaskLookup<'a> {
    fn from_buffers(buffers: &'a [(String, String)], bob_dir: &Path) -> Self {
        let settings = note_tasks::read_settings(bob_dir);
        let mut all_settings = settings.clone();
        all_settings.global_filter.clear();
        Self {
            contents: buffers
                .iter()
                .map(|(path, contents)| (path.clone(), contents.as_str()))
                .collect(),
            ordinary: RefCell::new(HashMap::new()),
            embedded: RefCell::new(HashMap::new()),
            settings,
            all_settings,
        }
    }

    fn contents(&self, path: &str) -> Option<&'a str> {
        self.contents.get(path).copied()
    }

    fn task_off_clock(
        &self,
        path: &str,
        block_id: &str,
        embedded: bool,
        resolver: &mut HomeResolver<IndexedHomeSource<'_>>,
    ) -> Option<bool> {
        let contents = self.contents(path)?;
        let settings = if embedded {
            &self.all_settings
        } else {
            &self.settings
        };
        let cache = if embedded {
            &self.embedded
        } else {
            &self.ordinary
        };
        let mut cache = cache.borrow_mut();
        let scan = cache
            .entry(path.to_string())
            .or_insert_with(|| note_tasks::scan(contents, settings));
        match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => {
                let line = contents.lines().nth(task.line_index)?;
                Some(effective_clock(resolver, Path::new(path), line).off_clock)
            }
            BlockIdLookup::NotATask { .. }
            | BlockIdLookup::Duplicate(_)
            | BlockIdLookup::Missing => None,
        }
    }
}

/// Whether a keyed row's key disagrees with the verdict recomputed from its
/// counted Task Links. Closed rows are history: later tag edits are
/// reported here and never applied. Fully resolved lineups, including the
/// empty Unlinked lineup, can drift; unresolved or ambiguous links keep
/// their existing resolver diagnostics. Bare rows carry no key, and
/// conflicts already surface through `clock_conflicts`.
fn session_drifted(
    session: &PomodoroSession,
    daily_path: &Path,
    lookup: &ClockTaskLookup<'_>,
    resolver: &mut HomeResolver<IndexedHomeSource<'_>>,
) -> bool {
    let Some(row_clock) = session.stats_clock() else {
        return false;
    };
    let mut seen = BTreeSet::new();
    let mut facts = Vec::new();
    for link in &session.links {
        if !seen.insert((link.target.clone(), link.block_id.clone())) {
            continue;
        }
        let Some(off_clock) =
            resolve_link_clock(link, daily_path, lookup, resolver)
        else {
            return false;
        };
        facts.push(LinkFact {
            off_clock: Some(off_clock),
        });
    }
    match clock_verdict(&facts) {
        ClockVerdict::OffClock => row_clock == StatsClock::Worked,
        ClockVerdict::Worked { .. } => row_clock == StatsClock::OffClock,
    }
}

/// Resolve one counted Task Link to its current task line and resolve its
/// effective clock there (`#on`, then `#off` or an alias, then the nearest
/// area/project `offclock:` default). `None` means the link does not pin
/// to exactly one task: no block id, an ambiguous or missing note, or a
/// missing or duplicated block id. Same-note empty targets try Event Tasks first,
/// then the writer `note_tasks` lookup (embedded links ignore the global
/// filter). Archived tasks resolve wherever the note index finds them.
fn resolve_link_clock(
    link: &SessionLink,
    daily_path: &Path,
    lookup: &ClockTaskLookup<'_>,
    resolver: &mut HomeResolver<IndexedHomeSource<'_>>,
) -> Option<bool> {
    if link.block_id.is_empty() {
        return None;
    }
    let LinkResolution::Found(path) =
        resolver.resolve_target(daily_path, &link.target)
    else {
        return None;
    };
    let path_key = path_string(&path);
    if link.target.is_empty()
        && let Some(contents) = lookup.contents(&path_key)
        && let Some(event) = event_task_by_id(&path, contents, &link.block_id)
    {
        let line = contents.lines().nth(event.line_index)?;
        return Some(effective_clock(resolver, daily_path, line).off_clock);
    }
    lookup.task_off_clock(&path_key, &link.block_id, link.embedded, resolver)
}

/// Parse a ledger time range into half-open minutes for overlap checks.
/// Overnight spans normalize past midnight; touching endpoints stay
/// empty, so adjacent sessions never overlap. Returns `None` for
/// unparseable ranges and zero-length spans.
fn parse_session_interval(time_range: Option<&str>) -> Option<(u32, u32)> {
    let range = time_range?;
    let (start_text, end_text) = range.split_once('-')?;
    let (start_hour, start_min) = parse_hhmm(start_text)?;
    let (end_hour, end_min) = parse_hhmm(end_text)?;
    let start = start_hour * 60 + start_min;
    let mut end = end_hour * 60 + end_min;
    if end < start {
        end += 24 * 60;
    }
    (end > start).then_some((start, end))
}

fn parse_hhmm(text: &str) -> Option<(u32, u32)> {
    let digits: String = text
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    if digits.len() != 4 {
        return None;
    }
    let hour = digits[..2].parse::<u32>().ok()?;
    let minute = digits[2..].parse::<u32>().ok()?;
    (hour <= 23 && minute <= 59).then_some((hour, minute))
}

/// Cap for the reported overlapping pairs. The aggregate stays exact past
/// the cap; only the detail sample is bounded.
const MAX_OVERLAP_DETAILS: usize = 50;

/// One closed timed session as a half-open interval on its ledger date.
/// Overnight ends are already normalized past midnight (`end` may exceed
/// 1440); absolute coordinates then let the next day's early session
/// overlap this one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OverlapSpan {
    date: NaiveDate,
    path: String,
    line: u32,
    start: u32,
    end: u32,
}

impl OverlapSpan {
    fn abs_start(&self) -> i64 {
        abs_minutes(self.date, self.start)
    }

    fn abs_end(&self) -> i64 {
        abs_minutes(self.date, self.end)
    }
}

fn abs_minutes(date: NaiveDate, minutes: u32) -> i64 {
    i64::from(date.num_days_from_ce()) * 24 * 60 + i64::from(minutes)
}

/// Ranked overlap detail for a max-heap of the 50 smallest pairs.
#[derive(Debug, Clone, Eq, PartialEq)]
struct RankedOverlap(OverlapReport);

impl Ord for RankedOverlap {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .date
            .cmp(&other.0.date)
            .then_with(|| self.0.lines.cmp(&other.0.lines))
            .then_with(|| self.0.minutes.cmp(&other.0.minutes))
            .then_with(|| self.0.paths.cmp(&other.0.paths))
    }
}

impl PartialOrd for RankedOverlap {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn overlap_pair(
    left: &OverlapSpan,
    right: &OverlapSpan,
) -> Option<OverlapReport> {
    let shared = left
        .abs_end()
        .min(right.abs_end())
        .saturating_sub(left.abs_start().max(right.abs_start()));
    if shared <= 0 {
        return None;
    }
    let (first, second) = if (left.date, left.line, left.path.as_str())
        <= (right.date, right.line, right.path.as_str())
    {
        (left, right)
    } else {
        (right, left)
    };
    let paths = (first.path != second.path)
        .then(|| vec![first.path.clone(), second.path.clone()]);
    Some(OverlapReport {
        date: first.date,
        lines: vec![first.line, second.line],
        minutes: u32::try_from(shared).unwrap_or(u32::MAX),
        paths,
    })
}

/// Sweep closed timed sessions for overlap using actual timed intervals,
/// independent of keyed minute overrides. Intervals use date-aware minute
/// coordinates so an overnight session can overlap the next day's early
/// session. Nonoverlapping rows skip in start order. The aggregate is the
/// union covered by two or more sessions, so a triple overlap contributes
/// its shared minutes once even though the pairwise detail lists every
/// pair. At most 50 details are kept during traversal (the 50 that would
/// sort first by date, lines, minutes, then paths). Clock totals never
/// subtract this value.
fn detect_overlaps(
    mut intervals: Vec<OverlapSpan>,
) -> (u32, Vec<OverlapReport>) {
    if intervals.is_empty() {
        return (0, Vec::new());
    }
    let mut events: Vec<(i64, i32)> = Vec::with_capacity(intervals.len() * 2);
    for span in &intervals {
        events.push((span.abs_start(), 1));
        events.push((span.abs_end(), -1));
    }
    events.sort_by(|left, right| {
        left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1))
    });
    let mut total = 0u32;
    let mut depth = 0i32;
    let mut covered_from = 0i64;
    for (at, delta) in events {
        let covered = depth >= 2;
        depth += delta;
        if covered && depth < 2 {
            total = total.saturating_add(
                u32::try_from(at.saturating_sub(covered_from)).unwrap_or(0),
            );
        } else if !covered && depth >= 2 {
            covered_from = at;
        }
    }

    intervals.sort_by(|left, right| {
        left.abs_start()
            .cmp(&right.abs_start())
            .then_with(|| left.date.cmp(&right.date))
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut kept: BinaryHeap<RankedOverlap> = BinaryHeap::new();
    for (index, left) in intervals.iter().enumerate() {
        for right in &intervals[index + 1..] {
            if right.abs_start() >= left.abs_end() {
                break;
            }
            let Some(detail) = overlap_pair(left, right) else {
                continue;
            };
            if kept.len() < MAX_OVERLAP_DETAILS {
                kept.push(RankedOverlap(detail));
            } else if kept
                .peek()
                .is_some_and(|worst| RankedOverlap(detail.clone()) < *worst)
            {
                kept.pop();
                kept.push(RankedOverlap(detail));
            }
        }
    }
    let mut details: Vec<OverlapReport> =
        kept.into_iter().map(|ranked| ranked.0).collect();
    details.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| left.lines.cmp(&right.lines))
            .then_with(|| left.minutes.cmp(&right.minutes))
            .then_with(|| left.paths.cmp(&right.paths))
    });
    (total, details)
}

fn link_health(
    session: &PomodoroSession,
    daily_path: &Path,
    resolver: &mut HomeResolver<IndexedHomeSource<'_>>,
) -> (u32, u32, u32) {
    let mut unfiled = 0u32;
    let mut ambiguous = 0u32;
    let mut unresolved = 0u32;
    let mut seen = BTreeSet::new();
    for link in &session.links {
        let resolution = resolver.resolve_target(daily_path, &link.target);
        let identity = match &resolution {
            crate::native::vault_links::LinkResolution::Found(path) => {
                format!("found:{}#{}", path_string(path), link.block_id)
            }
            crate::native::vault_links::LinkResolution::Ambiguous => {
                format!("ambiguous:{}#{}", link.target, link.block_id)
            }
            crate::native::vault_links::LinkResolution::Missing => {
                format!("missing:{}#{}", link.target, link.block_id)
            }
        };
        if !seen.insert(identity) {
            continue;
        }
        match resolution {
            crate::native::vault_links::LinkResolution::Found(_) => {
                let home = resolver.home_for_link(
                    daily_path,
                    resolution,
                    &link.block_id,
                );
                if home.key == "@unfiled" {
                    unfiled += 1;
                }
            }
            crate::native::vault_links::LinkResolution::Ambiguous => {
                ambiguous += 1;
                unfiled += 1;
            }
            crate::native::vault_links::LinkResolution::Missing => {
                unresolved += 1;
                unfiled += 1;
            }
        }
    }
    (unfiled, ambiguous, unresolved)
}

fn homes_warnings(health: &HomesHealthReport) -> Vec<StatsWarning> {
    let mut warnings = Vec::new();
    for lint in &health.area_color_invalid {
        warnings.push(StatsWarning {
            code: "area_color_invalid".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{}: invalid color {:?}; the note inherits its color. Set color: to a palette slot or grey",
                lint.path, lint.raw
            ),
        });
    }
    for lint in &health.note_offclock_invalid {
        let key = lint.key.as_deref().unwrap_or("offclock");
        warnings.push(StatsWarning {
            code: "note_offclock_invalid".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{}: invalid {key} value {:?}; using on-clock. Use true or false",
                lint.path, lint.raw
            ),
        });
    }
    for lint in &health.note_offclock_ignored {
        let key = lint.key.as_deref().unwrap_or("offclock");
        warnings.push(StatsWarning {
            code: "note_offclock_ignored".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{}: {key} value {:?} ignored; only area and project notes set clock defaults",
                lint.path, lint.raw
            ),
        });
    }
    for lint in &health.note_offclock_shadowed {
        warnings.push(StatsWarning {
            code: "note_offclock_shadowed".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{}: offclock ignored; offclock_types decides this note's clock",
                lint.path
            ),
        });
    }
    for collision in &health.hue_collisions {
        let names = join_names(&collision.areas);
        warnings.push(StatsWarning {
            code: "area_hue_collision".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{names} share hue {}; set color: on one of them",
                collision.hue.name()
            ),
        });
    }
    for lint in &health.root_field_invalid {
        let hint = match lint.reason.as_str() {
            "invalid" => "write the bare note name",
            "duplicate" => "keep a single root field",
            "missing" => "name an existing note",
            "ambiguous" => "use a unique path",
            "no_home" => "file that note in an area or project",
            _ => "repair the field",
        };
        warnings.push(StatsWarning {
            code: "root_field_invalid".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{}:{}: root field {:?} ignored ({}); {hint}",
                lint.path, lint.line, lint.raw, lint.reason
            ),
        });
    }
    for lint in &health.root_field_not_root {
        let root_stem = Path::new(&lint.root)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(lint.root.as_str());
        warnings.push(StatsWarning {
            code: "root_field_not_root".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{}:{}: {} groups under {root_stem}; write [root:: {root_stem}] or set root: true on {}",
                lint.path, lint.line, lint.raw, lint.raw
            ),
        });
    }
    // Clock diagnostics ride the existing warning channel so renderers can
    // surface them without a second vault pass. Messages stay lowercase:
    // only the off-clock view headers use the capitalized clock names.
    if health.clock_conflicts > 0 {
        warnings.push(StatsWarning {
            code: "clock_conflict".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{} closed {} more than one duration key; counted as worked",
                health.clock_conflicts,
                plural_session(health.clock_conflicts),
            ),
        });
    }
    if health.clock_drift > 0 {
        warnings.push(StatsWarning {
            code: "clock_drift".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{} closed {} with the linked tasks' current clocks; closed rows keep their keys",
                health.clock_drift,
                if health.clock_drift == 1 {
                    "session disagrees"
                } else {
                    "sessions disagree"
                },
            ),
        });
    }
    if health.unstamped_sessions > 0 {
        warnings.push(StatsWarning {
            code: "clock_unstamped".to_string(),
            metric: None,
            date: None,
            message: format!(
                "{} closed {} no duration key; counted from the time span",
                health.unstamped_sessions,
                plural_session(health.unstamped_sessions),
            ),
        });
    }
    if health.overlap_minutes > 0 {
        warnings.push(StatsWarning {
            code: "clock_overlap".to_string(),
            metric: None,
            date: None,
            message: format!(
                "closed sessions overlap by {}; clock totals count every session",
                format_minutes(i64::from(health.overlap_minutes)),
            ),
        });
    }
    warnings
}

fn plural_session(count: u32) -> &'static str {
    if count == 1 {
        "session has"
    } else {
        "sessions have"
    }
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        rest => {
            let (last, head) = rest.split_last().expect("non-empty");
            format!("{} and {last}", head.join(", "))
        }
    }
}

pub(crate) fn label_parts(
    parts_summary: &mut BTreeMap<String, Vec<super::report::PartSummary>>,
    catalog: &BTreeMap<String, HomeCatalogEntry>,
) {
    for (dimension, parts) in parts_summary {
        if dimension != "area"
            && dimension != "project"
            && dimension != "root_parent"
        {
            continue;
        }
        for part in parts {
            if part.key == "other" {
                continue;
            }
            let Some(entry) = catalog.get(&part.key) else {
                continue;
            };
            part.label = Some(if dimension == "project" {
                entry
                    .project_label
                    .clone()
                    .unwrap_or_else(|| entry.label.clone())
            } else {
                entry.label.clone()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::NaiveDate;
    use tempfile::TempDir;

    use super::HomeKind;
    use crate::native::pomodoro_homes::SessionKind;
    use crate::native::stats::compute::compute;
    use crate::native::stats::request::{
        Grouping, MetricSelection, StatsRequest,
    };

    fn homes_vault() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/stats/homes_vault"
        ))
    }

    fn write_config(dir: &TempDir) -> PathBuf {
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            [
                "stats:",
                "  metrics:",
                "    sase_time:",
                "      label: SASE time",
                "      kind: pomodoro",
                "      measure: minutes",
                "      themes: [SASE]",
                "    sase_in:",
                "      label: SASE in",
                "      kind: pomodoro",
                "      measure: minutes",
                "      in: [sase]",
                "    sase_count_in:",
                "      label: SASE session count",
                "      kind: pomodoro",
                "      measure: count",
                "      in: [sase]",
                "    sase_blog_in:",
                "      label: SASE blog in",
                "      kind: pomodoro",
                "      measure: minutes",
                "      in: [sase_blog]",
                "    exclude_sase:",
                "      label: Exclude sase",
                "      kind: pomodoro",
                "      measure: minutes",
                "      exclude_in: [sase]",
                "    gtd_daily_in:",
                "      label: gtd_daily in",
                "      kind: pomodoro",
                "      measure: minutes",
                "      in: [gtd_daily]",
                "    gtd_daily_count:",
                "      label: gtd_daily session count",
                "      kind: pomodoro",
                "      measure: count",
                "      in: [gtd_daily]",
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        path
    }

    fn homes_request(vault: PathBuf, config: PathBuf) -> StatsRequest {
        let today = NaiveDate::from_ymd_opt(2026, 10, 18)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let mut request = StatsRequest::at(vault, config, today);
        request.selection = MetricSelection::Explicit(
            [
                "worked",
                "sessions",
                "done",
                "sase_time",
                "sase_in",
                "sase_count_in",
                "sase_blog_in",
                "exclude_sase",
                "gtd_daily_in",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        );
        request.range = Some("2026-10-15..2026-10-18".to_string());
        request.grouping = Grouping::Day;
        request
    }

    fn metric<'a>(
        report: &'a crate::native::stats::report::StatsReport,
        id: &str,
    ) -> &'a crate::native::stats::report::MetricReport {
        report
            .metrics
            .iter()
            .find(|metric| metric.id == id)
            .unwrap_or_else(|| panic!("missing metric {id}"))
    }

    fn part_value(
        metric: &crate::native::stats::report::MetricReport,
        dimension: &str,
        key: &str,
    ) -> i64 {
        metric.parts_summary[dimension]
            .iter()
            .find(|part| part.key == key)
            .map(|part| part.value)
            .unwrap_or(0)
    }

    fn dimension_total(
        metric: &crate::native::stats::report::MetricReport,
        dimension: &str,
    ) -> i64 {
        metric.parts_summary[dimension]
            .iter()
            .map(|part| part.value)
            .sum()
    }

    #[test]
    fn h_vectors_cover_parts_buckets_apportioned_and_theme_filter() {
        let dir = TempDir::new().unwrap();
        let report = compute(&homes_request(homes_vault(), write_config(&dir)))
            .expect("homes fixture computes");

        let worked = metric(&report, "worked");
        assert_eq!(worked.summary.total.as_ref().unwrap().value, Some(167));
        assert_eq!(dimension_total(worked, "area"), 167);
        assert_eq!(dimension_total(worked, "project"), 167);
        assert_eq!(dimension_total(worked, "theme"), 167);

        assert_eq!(part_value(worked, "area", "gtd.md"), 20);
        assert_eq!(part_value(worked, "area", "dev.md"), 90);
        assert_eq!(part_value(worked, "area", "job.md"), 7);
        assert_eq!(part_value(worked, "area", "@unlinked"), 35);
        assert_eq!(part_value(worked, "area", "@unfiled"), 15);

        assert_eq!(part_value(worked, "project", "sase.md"), 30);
        assert_eq!(part_value(worked, "project", "sase_blog.md"), 50);
        assert_eq!(part_value(worked, "project", "gtd_daily.md"), 10);
        assert_eq!(part_value(worked, "project", "bob.md"), 10);
        assert_eq!(part_value(worked, "project", "dev.md"), 10);
        assert_eq!(part_value(worked, "project", "job.md"), 4);
        assert_eq!(part_value(worked, "project", "job_apply.md"), 3);
        assert_eq!(part_value(worked, "project", "@unlinked"), 35);
        assert_eq!(part_value(worked, "project", "@unfiled"), 15);

        // H16 worked root_parent sums to 167 with leaf projects folded
        // into their top-level root.
        assert_eq!(dimension_total(worked, "root_parent"), 167);
        assert_eq!(part_value(worked, "root_parent", "sase.md"), 80);
        assert_eq!(part_value(worked, "root_parent", "gtd_daily.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "bob.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "dev.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "job.md"), 4);
        assert_eq!(part_value(worked, "root_parent", "job_apply.md"), 3);
        assert_eq!(part_value(worked, "root_parent", "@unlinked"), 35);
        assert_eq!(part_value(worked, "root_parent", "@unfiled"), 15);

        let sase = worked.parts_summary["project"]
            .iter()
            .find(|part| part.key == "sase.md")
            .unwrap();
        assert_eq!(sase.apportioned, Some(10));
        let job = worked.parts_summary["project"]
            .iter()
            .find(|part| part.key == "job.md")
            .unwrap();
        assert_eq!(job.apportioned, Some(4));
        let gtd_area = worked.parts_summary["area"]
            .iter()
            .find(|part| part.key == "gtd.md")
            .unwrap();
        assert_eq!(gtd_area.apportioned, Some(10));
        // Root apportioned comes from sessions with two or more roots:
        // HACK spreads bob/sase/dev and JOB spreads job/job_apply.
        let sase_root = worked.parts_summary["root_parent"]
            .iter()
            .find(|part| part.key == "sase.md")
            .unwrap();
        assert_eq!(sase_root.apportioned, Some(10));
        assert_eq!(sase_root.apportioned_display.as_deref(), Some("10m"));
        let bob_root = worked.parts_summary["root_parent"]
            .iter()
            .find(|part| part.key == "bob.md")
            .unwrap();
        assert_eq!(bob_root.apportioned, Some(10));
        let dev_root = worked.parts_summary["root_parent"]
            .iter()
            .find(|part| part.key == "dev.md")
            .unwrap();
        assert_eq!(dev_root.apportioned, Some(10));
        let job_root = worked.parts_summary["root_parent"]
            .iter()
            .find(|part| part.key == "job.md")
            .unwrap();
        assert_eq!(job_root.apportioned, Some(4));
        let job_apply_root = worked.parts_summary["root_parent"]
            .iter()
            .find(|part| part.key == "job_apply.md")
            .unwrap();
        assert_eq!(job_apply_root.apportioned, Some(3));
        let gtd_daily_root = worked.parts_summary["root_parent"]
            .iter()
            .find(|part| part.key == "gtd_daily.md")
            .unwrap();
        assert_eq!(gtd_daily_root.apportioned, None);
        // Root labels are bare catalog labels.
        assert_eq!(sase_root.label.as_deref(), Some("sase"));
        assert_eq!(gtd_daily_root.label.as_deref(), Some("gtd_daily"));

        let sessions = metric(&report, "sessions");
        assert_eq!(sessions.summary.total.as_ref().unwrap().value, Some(8));
        assert_eq!(dimension_total(sessions, "area"), 8);
        assert_eq!(dimension_total(sessions, "project"), 8);
        assert_eq!(dimension_total(sessions, "root_parent"), 8);
        assert_eq!(part_value(sessions, "project", "sase.md"), 1);
        assert_eq!(part_value(sessions, "project", "sase_blog.md"), 1);
        assert_eq!(part_value(sessions, "project", "@unlinked"), 2);
        // Count-mode lead roots group distinct tasks by root.
        assert_eq!(part_value(sessions, "root_parent", "sase.md"), 2);
        assert_eq!(part_value(sessions, "root_parent", "@unlinked"), 2);

        let done = metric(&report, "done");
        assert_eq!(done.summary.total.as_ref().unwrap().value, Some(7));
        assert_eq!(dimension_total(done, "kind"), 7);
        assert_eq!(dimension_total(done, "area"), 7);
        assert_eq!(dimension_total(done, "root_parent"), 7);
        assert_eq!(dimension_total(done, "project"), 7);
        assert_eq!(dimension_total(done, "note"), 7);
        assert_eq!(part_value(done, "project", "sase.md"), 2);
        assert_eq!(part_value(done, "project", "@unfiled"), 1);
        assert_eq!(part_value(done, "root_parent", "sase.md"), 2);
        assert_eq!(part_value(done, "root_parent", "bob.md"), 1);
        assert_eq!(part_value(done, "root_parent", "job.md"), 1);
        assert_eq!(part_value(done, "root_parent", "job_apply.md"), 1);
        assert_eq!(part_value(done, "root_parent", "gtd.md"), 1);
        assert_eq!(part_value(done, "root_parent", "@unfiled"), 1);
        assert_eq!(
            done.parts_summary["project"]
                .iter()
                .find(|part| part.key == "job.md")
                .unwrap()
                .label
                .as_deref(),
            Some("job · area tasks")
        );

        let sase_time = metric(&report, "sase_time");
        assert_eq!(sase_time.summary.total.as_ref().unwrap().value, Some(10));
        assert_eq!(dimension_total(sase_time, "area"), 10);
        assert_eq!(dimension_total(sase_time, "root_parent"), 10);
        assert_eq!(dimension_total(sase_time, "project"), 10);
        assert_eq!(part_value(sase_time, "project", "sase.md"), 10);
        assert_eq!(part_value(sase_time, "root_parent", "sase.md"), 10);
        assert_eq!(part_value(sase_time, "theme", "SASE"), 10);

        // H9 in:[sase] keeps residence-chain matches, including sase_blog
        // and the sase share of HACK; unlinked sessions never match.
        let sase_in = metric(&report, "sase_in");
        assert_eq!(sase_in.summary.total.as_ref().unwrap().value, Some(80));
        assert_eq!(dimension_total(sase_in, "area"), 80);
        assert_eq!(dimension_total(sase_in, "root_parent"), 80);
        assert_eq!(dimension_total(sase_in, "project"), 80);
        assert_eq!(part_value(sase_in, "project", "sase.md"), 30);
        assert_eq!(part_value(sase_in, "project", "sase_blog.md"), 50);
        assert_eq!(part_value(sase_in, "root_parent", "sase.md"), 80);

        // H14 count mode keeps a session once any linked task matches in:.
        let sase_count_in = metric(&report, "sase_count_in");
        assert_eq!(
            sase_count_in.summary.total.as_ref().unwrap().value,
            Some(3)
        );
        assert_eq!(dimension_total(sase_count_in, "area"), 3);
        assert_eq!(dimension_total(sase_count_in, "project"), 3);

        // H10 in:[sase_blog] measures on its own.
        let sase_blog_in = metric(&report, "sase_blog_in");
        assert_eq!(
            sase_blog_in.summary.total.as_ref().unwrap().value,
            Some(50)
        );
        assert_eq!(part_value(sase_blog_in, "project", "sase_blog.md"), 50);
        assert_eq!(dimension_total(sase_blog_in, "root_parent"), 50);
        assert_eq!(part_value(sase_blog_in, "root_parent", "sase.md"), 50);
        // The catalog keeps the referenced root even when only the leaf
        // carries minutes in the span.
        assert!(report.homes.contains_key("sase.md"));
        assert_eq!(
            report.homes["sase_blog.md"].root_parent,
            "sase.md".to_string()
        );
        assert_eq!(report.homes["sase.md"].root_parent, "sase.md".to_string());
        assert_eq!(
            report.homes["gtd_daily.md"].root_parent,
            "gtd_daily.md".to_string()
        );
        assert_eq!(
            report.homes["gtd_daily.md"].area.as_deref(),
            Some("gtd.md")
        );

        // H11 exclude_in:[sase] never drops unlinked sessions; work tasks
        // whose chain matches sase are removed (HACK keeps bob+dev = 20).
        let exclude_sase = metric(&report, "exclude_sase");
        assert_eq!(
            exclude_sase.summary.total.as_ref().unwrap().value,
            Some(87)
        );
        assert_eq!(dimension_total(exclude_sase, "project"), 87);
        assert_eq!(part_value(exclude_sase, "project", "@unlinked"), 35);
        assert_eq!(part_value(exclude_sase, "project", "gtd_daily.md"), 10);

        // H12 in:[gtd_daily] is the Oct 16 session linking the rooted ^gtd.
        let gtd_daily_in = metric(&report, "gtd_daily_in");
        assert_eq!(
            gtd_daily_in.summary.total.as_ref().unwrap().value,
            Some(10)
        );
        assert_eq!(part_value(gtd_daily_in, "project", "gtd_daily.md"), 10);
        assert_eq!(part_value(gtd_daily_in, "area", "gtd.md"), 10);
        assert_eq!(part_value(gtd_daily_in, "root_parent", "gtd_daily.md"), 10);

        // H13 sessions list: facts for every closed row, today included.
        assert_eq!(report.sessions.len(), 9);
        assert_eq!(report.sessions[0].name.as_deref(), Some("PLAN"));
        assert_eq!(report.sessions[0].minutes, Some(25));
        assert_eq!(report.sessions[0].homes.kind, SessionKind::Unlinked);
        assert_eq!(report.sessions[1].name.as_deref(), Some("HACK"));
        assert_eq!(report.sessions[1].homes.kind, SessionKind::Unlinked);
        assert_eq!(report.sessions[2].name.as_deref(), Some("BLOG"));
        assert_eq!(report.sessions[7].name, None);
        assert_eq!(report.sessions[7].homes.kind, SessionKind::Work);
        assert_eq!(report.sessions[7].homes.label, "gtd_daily");
        assert_eq!(report.sessions[8].name.as_deref(), Some("BLOG"));
        assert_eq!(report.sessions[8].minutes, Some(15));
        assert_eq!(
            report.sessions[8].date,
            NaiveDate::from_ymd_opt(2026, 10, 18).unwrap()
        );

        assert!(!report.homes.contains_key("body.md"));
        assert!(!report.homes.contains_key("invalid.md"));
        assert_eq!(report.health.homes.unlinked_sessions, 2);
        assert_eq!(report.health.homes.unlinked_minutes, 35);
        assert_eq!(report.health.homes.untracked_sessions, 0);
        assert_eq!(report.health.homes.unfiled_links, 1);
        assert!(!report.health.homes.hue_collisions.is_empty());
        assert!(report
            .warnings
            .iter()
            .any(|w| w.code == "area_hue_collision"));
        assert!(!report.warnings.iter().any(|w| w.code == "rollup_invalid"));

        let first = serde_json::to_vec(&report).unwrap();
        let second = compute(&homes_request(homes_vault(), write_config(&dir)))
            .expect("repeat compute");
        assert_eq!(first, serde_json::to_vec(&second).unwrap());
    }

    #[test]
    fn h_vector_review_time_includes_a_gtd_share_inside_work() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir);
        let today = NaiveDate::from_ymd_opt(2026, 10, 18)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let mut request = StatsRequest::at(homes_vault(), config, today);
        request.selection = MetricSelection::Explicit(
            ["worked", "review_time", "gtd_daily_count"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
        request.range = Some("2026-10-14..2026-10-14".to_string());
        request.grouping = Grouping::Day;
        let report = compute(&request).expect("review share vector computes");
        let worked = metric(&report, "worked");
        let review_time = metric(&report, "review_time");
        let review_count = metric(&report, "gtd_daily_count");
        // H15: the Oct 14 work session links another day's rooted ^gtd, so
        // it splits 50m into 25m gtd_daily and 25m bob; the root-parent
        // split matches. review_time is `in: [gtd]`, and both gtd_daily
        // and bob (a gtd project) sit under gtd, so it keeps both shares.
        assert_eq!(worked.summary.total.as_ref().unwrap().value, Some(50));
        assert_eq!(part_value(worked, "project", "gtd_daily.md"), 25);
        assert_eq!(part_value(worked, "project", "bob.md"), 25);
        assert_eq!(part_value(worked, "root_parent", "gtd_daily.md"), 25);
        assert_eq!(part_value(worked, "root_parent", "bob.md"), 25);
        assert_eq!(review_time.summary.total.as_ref().unwrap().value, Some(50));
        assert_eq!(part_value(review_time, "project", "gtd_daily.md"), 25);
        assert_eq!(part_value(review_time, "project", "bob.md"), 25);
        assert_eq!(review_count.summary.total.as_ref().unwrap().value, Some(1));
        assert_eq!(part_value(review_count, "project", "gtd_daily.md"), 1);
    }

    #[test]
    fn worked_root_parent_conserves_totals_in_week_grouping() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir);
        let today = NaiveDate::from_ymd_opt(2026, 10, 18)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let mut request = StatsRequest::at(homes_vault(), config, today);
        request.selection = MetricSelection::Explicit(
            ["worked"].into_iter().map(str::to_string).collect(),
        );
        request.range = Some("2026-10-15..2026-10-18".to_string());
        request.grouping = Grouping::Week;
        let report = compute(&request).expect("week grouping computes");
        let worked = metric(&report, "worked");
        assert_eq!(dimension_total(worked, "area"), 167);
        assert_eq!(dimension_total(worked, "project"), 167);
        assert_eq!(dimension_total(worked, "root_parent"), 167);
        assert_eq!(part_value(worked, "root_parent", "sase.md"), 80);
        for point in worked
            .points
            .iter()
            .chain(&worked.prior_points)
            .chain(&worked.daily_points)
        {
            let area: i64 = point
                .parts
                .get("area")
                .map_or(0, |entries| entries.values().sum());
            let root: i64 = point
                .parts
                .get("root_parent")
                .map_or(0, |entries| entries.values().sum());
            let project: i64 = point
                .parts
                .get("project")
                .map_or(0, |entries| entries.values().sum());
            if point.value.is_some() {
                assert_eq!(area, project, "area/project mismatch");
                assert_eq!(root, project, "root/project mismatch");
            }
            let area_app: i64 = point
                .apportioned
                .get("area")
                .map_or(0, |entries| entries.values().sum());
            let root_app: i64 = point
                .apportioned
                .get("root_parent")
                .map_or(0, |entries| entries.values().sum());
            let project_app: i64 = point
                .apportioned
                .get("project")
                .map_or(0, |entries| entries.values().sum());
            let _ = (area_app, root_app, project_app);
        }
    }

    #[test]
    fn single_metric_worked_today_resolves_unseen_inspect_roots() {
        let workspace = TempDir::new().unwrap();
        let vault = workspace.path().join("vault");
        copy_tree(&homes_vault(), &vault);
        // Child sorts before its parent so global inspect reaches it first,
        // and neither note is linked from today's sessions.
        std::fs::write(
            vault.join("aaa_leaf.md"),
            [
                "---",
                "type: \"[[project]]\"",
                "parent: \"[[zzz_root]]\"",
                "---",
                "# Nested leaf",
                "",
                "## Tasks",
                "",
                "- [ ] #task Nested leaf task ^t",
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            vault.join("zzz_root.md"),
            [
                "---",
                "type: \"[[project]]\"",
                "parent: \"[[dev]]\"",
                "---",
                "# Nested root",
                "",
                "## Tasks",
                "",
                "- [ ] #task Nested root task ^t",
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        let config = write_config(&workspace);
        let today = NaiveDate::from_ymd_opt(2026, 10, 18)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let mut request = StatsRequest::at(vault, config, today);
        request.selection = MetricSelection::Explicit(
            ["worked"].into_iter().map(str::to_string).collect(),
        );
        request.range = Some("2026-10-18..2026-10-18".to_string());
        request.grouping = Grouping::Day;
        let report = compute(&request)
            .expect("single-metric worked covering today computes");
        // Today's span is incomplete, so the headline total stays empty;
        // session facts still include the day's BLOG row.
        assert!(metric(&report, "worked").summary.total.is_none());
        assert_eq!(report.sessions.len(), 1);
        assert_eq!(report.sessions[0].name.as_deref(), Some("BLOG"));
        assert_eq!(report.sessions[0].minutes, Some(15));
        assert_eq!(
            report.sessions[0].date,
            NaiveDate::from_ymd_opt(2026, 10, 18).unwrap()
        );
        assert_eq!(
            report.homes["sase_blog.md"].root_parent,
            "sase.md".to_string()
        );
    }

    #[test]
    fn lead_root_groups_tasks_not_projects() {
        use crate::native::pomodoro_homes::{Home, HomeKind};
        fn home(key: &str, root: &str, area: &str) -> Home {
            Home {
                key: key.to_string(),
                label: key.to_string(),
                kind: HomeKind::Project,
                area: area.to_string(),
                area_label: area.to_string(),
                subarea: None,
                hue: None,
                hue_source: None,
                root_hue: None,
                residence_chain: vec![key.to_string()],
                root_parent: root.to_string(),
            }
        }
        // A=2 and B=2 tasks under root R, C=3 under root S. The lead
        // project is C but the lead root is R (4 tasks vs 3).
        let kept = vec![
            (home("a.md", "r.md", "dev.md"), 10),
            (home("a.md", "r.md", "dev.md"), 10),
            (home("b.md", "r.md", "dev.md"), 10),
            (home("b.md", "r.md", "dev.md"), 10),
            (home("c.md", "s.md", "gtd.md"), 10),
            (home("c.md", "s.md", "gtd.md"), 10),
            (home("c.md", "s.md", "gtd.md"), 10),
        ];
        let split = super::split_from_kept(&kept, true);
        assert_eq!(split.lead_project.as_deref(), Some("c.md"));
        assert_eq!(split.lead_root_parent.as_deref(), Some("r.md"));
        assert_eq!(
            split
                .root_parent
                .iter()
                .find(|(key, _)| key == "r.md")
                .map(|(_, value)| *value),
            Some(40)
        );
        assert_eq!(
            split
                .root_parent
                .iter()
                .find(|(key, _)| key == "s.md")
                .map(|(_, value)| *value),
            Some(30)
        );
        let count = super::split_from_kept(&kept, false);
        assert_eq!(count.lead_root_parent.as_deref(), Some("r.md"));
    }

    #[test]
    fn golden_json_carries_root_parent_beside_area_and_project() {
        let dir = TempDir::new().unwrap();
        let report = compute(&homes_request(homes_vault(), write_config(&dir)))
            .expect("golden report computes");
        let value = serde_json::to_value(&report).expect("report serializes");
        let worked = value["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|metric| metric["id"] == "worked")
            .expect("worked metric");
        let summary = &worked["parts_summary"];
        assert!(summary.get("root_parent").is_some());
        assert!(summary.get("area").is_some());
        assert!(summary.get("project").is_some());
        let roots = summary["root_parent"].as_array().unwrap();
        let sase = roots
            .iter()
            .find(|part| part["key"] == "sase.md")
            .expect("sase root");
        assert_eq!(sase["value"], 80);
        assert_eq!(sase["label"], "sase");
        assert!(sase.get("apportioned").is_some());
        let point = &worked["points"][0];
        assert!(point["parts"].get("root_parent").is_some());
        assert!(point["parts"].get("area").is_some());
        assert!(point["parts"].get("project").is_some());
        let catalog = value["homes"].as_object().unwrap();
        assert_eq!(
            catalog["sase_blog.md"]["root_parent"],
            serde_json::Value::String("sase.md".to_string())
        );
        assert_eq!(
            catalog["sase.md"]["root_parent"],
            serde_json::Value::String("sase.md".to_string())
        );
    }

    fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let dest = dst.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), dest).unwrap();
            }
        }
    }

    #[test]
    fn root_flag_regroups_blog_minutes_under_its_own_root() {
        let workspace = TempDir::new().unwrap();
        let vault = workspace.path().join("vault");
        copy_tree(&homes_vault(), &vault);
        let blog = vault.join("sase_blog.md");
        let contents = std::fs::read_to_string(&blog).unwrap();
        let flagged = contents.replacen(
            "type: \"[[project]]\"",
            "type: \"[[project]]\"\nroot: true",
            1,
        );
        assert_ne!(flagged, contents, "sase_blog.md gains root: true");
        std::fs::write(&blog, flagged).unwrap();
        let config = write_config(&workspace);
        let report = compute(&homes_request(vault, config))
            .expect("flagged homes fixture computes");

        let worked = metric(&report, "worked");
        // H17: blog minutes regroup under their own root; total unchanged.
        assert_eq!(dimension_total(worked, "root_parent"), 167);
        assert_eq!(part_value(worked, "root_parent", "sase.md"), 30);
        assert_eq!(part_value(worked, "root_parent", "sase_blog.md"), 50);
        assert_eq!(part_value(worked, "root_parent", "gtd_daily.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "bob.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "dev.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "job.md"), 4);
        assert_eq!(part_value(worked, "root_parent", "job_apply.md"), 3);
        assert_eq!(part_value(worked, "root_parent", "@unlinked"), 35);
        assert_eq!(part_value(worked, "root_parent", "@unfiled"), 15);
        // Area and project dimensions match H2 and H3.
        assert_eq!(part_value(worked, "area", "gtd.md"), 20);
        assert_eq!(part_value(worked, "area", "dev.md"), 90);
        assert_eq!(part_value(worked, "area", "job.md"), 7);
        assert_eq!(part_value(worked, "area", "@unlinked"), 35);
        assert_eq!(part_value(worked, "area", "@unfiled"), 15);
        assert_eq!(part_value(worked, "project", "sase.md"), 30);
        assert_eq!(part_value(worked, "project", "sase_blog.md"), 50);
        assert_eq!(part_value(worked, "project", "gtd_daily.md"), 10);
        assert_eq!(part_value(worked, "project", "bob.md"), 10);
        assert_eq!(part_value(worked, "project", "dev.md"), 10);
        assert_eq!(part_value(worked, "project", "job.md"), 4);
        assert_eq!(part_value(worked, "project", "job_apply.md"), 3);
        assert_eq!(part_value(worked, "project", "@unlinked"), 35);
        // in: filters still match the full residence chain.
        let sase_in = metric(&report, "sase_in");
        assert_eq!(sase_in.summary.total.as_ref().unwrap().value, Some(80));
        let sase_blog_in = metric(&report, "sase_blog_in");
        assert_eq!(
            sase_blog_in.summary.total.as_ref().unwrap().value,
            Some(50)
        );
        // The catalog names the new root.
        assert_eq!(
            report.homes["sase_blog.md"].root_parent,
            "sase_blog.md".to_string()
        );
        assert_eq!(report.homes["sase.md"].root_parent, "sase.md".to_string());
        // Count mode: the BLOG session leads sase_blog, PLAN+SASE leads sase.
        let sessions = metric(&report, "sessions");
        assert_eq!(sessions.summary.total.as_ref().unwrap().value, Some(8));
        assert_eq!(dimension_total(sessions, "root_parent"), 8);
        assert_eq!(part_value(sessions, "root_parent", "sase.md"), 1);
        assert_eq!(part_value(sessions, "root_parent", "sase_blog.md"), 1);
        assert_eq!(part_value(sessions, "root_parent", "@unlinked"), 2);
    }

    #[test]
    fn nearest_color_mutes_grey_areas_and_keeps_root_hues() {
        use crate::native::pomodoro_homes::{HomeHueSource, Hue};
        let workspace = TempDir::new().unwrap();
        let vault = workspace.path().join("vault");
        copy_tree(&homes_vault(), &vault);
        let edit = |name: &str, from: &str, to: &str| {
            let path = vault.join(name);
            let contents = std::fs::read_to_string(&path).unwrap();
            let edited = contents.replacen(from, to, 1);
            assert_ne!(edited, contents, "{name} edited");
            std::fs::write(&path, edited).unwrap();
        };
        edit("gtd.md", "color: orange", "color: grey");
        edit(
            "sase_blog.md",
            "parent: \"[[sase]]\"",
            "parent: \"[[sase]]\"\ncolor: red",
        );
        let config = write_config(&workspace);
        let report = compute(&homes_request(vault, config))
            .expect("recolored homes fixture computes");

        // R8: grey mutes gtd and everything under it on purpose.
        for key in ["gtd.md", "gtd_daily.md", "bob.md"] {
            let entry = &report.homes[key];
            assert_eq!(entry.hue, None, "{key} hue");
            assert_eq!(entry.hue_source, Some(HomeHueSource::Muted), "{key}");
        }
        // A sub-project's own slot colors it, never its root.
        let blog = &report.homes["sase_blog.md"];
        assert_eq!(blog.hue, Some(Hue::Red));
        assert_eq!(blog.hue_source, Some(HomeHueSource::Color));
        assert_eq!(blog.root_parent, "sase.md");
        let sase = &report.homes["sase.md"];
        assert_eq!(sase.hue, Some(Hue::Blue));
        assert_eq!(sase.hue_source, Some(HomeHueSource::Color));
        // A muted area never collides; body keeps orange alone.
        assert!(report.health.homes.hue_collisions.is_empty());
        assert!(!report
            .warnings
            .iter()
            .any(|w| w.code == "area_hue_collision"
                || w.code == "area_color_invalid"));

        let value = serde_json::to_value(&report).expect("report serializes");
        let catalog = value["homes"].as_object().unwrap();
        assert!(catalog["gtd.md"]["hue"].is_null());
        assert_eq!(catalog["gtd.md"]["hue_source"], "muted");
        assert_eq!(catalog["dev.md"]["hue_source"], "color");
        // Buckets carry no hue source.
        assert!(catalog["@unlinked"].get("hue_source").is_none());
    }

    #[test]
    fn only_unlinked_and_unfiled_buckets_remain_in_the_catalog() {
        let dir = TempDir::new().unwrap();
        let report = compute(&homes_request(homes_vault(), write_config(&dir)))
            .expect("homes fixture computes");
        for (key, entry) in &report.homes {
            if key.starts_with('@') {
                assert!(
                    key == "@unlinked" || key == "@unfiled",
                    "unexpected bucket {key}"
                );
                assert_eq!(entry.kind, HomeKind::Bucket);
            }
        }
        let unlinked = report
            .homes
            .get("@unlinked")
            .expect("unlinked catalog entry");
        assert_eq!(unlinked.label, "unlinked");
        assert_eq!(unlinked.root_parent, "@unlinked");
        let gtd_daily = report
            .homes
            .get("gtd_daily.md")
            .expect("gtd_daily catalog entry");
        assert_eq!(gtd_daily.kind, HomeKind::Area);
        assert_eq!(gtd_daily.root_parent, "gtd_daily.md");
        let value = serde_json::to_value(&report).expect("report serializes");
        let catalog = value["homes"].as_object().unwrap();
        assert!(catalog.get("@daily").is_none());
        assert!(catalog.get("@review:PLAN").is_none());
        assert_eq!(catalog["gtd_daily.md"]["root_parent"], "gtd_daily.md");
        assert_eq!(catalog["@unlinked"]["label"], "unlinked");
    }

    #[test]
    fn session_intervals_parse_spans_and_reject_empty_ranges() {
        assert_eq!(
            super::parse_session_interval(Some("08:00 - 08:25")),
            Some((480, 505))
        );
        assert_eq!(
            super::parse_session_interval(Some("0800-0825")),
            Some((480, 505))
        );
        assert_eq!(
            super::parse_session_interval(Some("2200-0100")),
            Some((1320, 1500))
        );
        assert_eq!(super::parse_session_interval(Some("0900-0900")), None);
        assert_eq!(super::parse_session_interval(None), None);
        assert_eq!(super::parse_session_interval(Some("nope")), None);
        assert_eq!(super::parse_session_interval(Some("25:00-2600")), None);
    }

    fn span(
        date: NaiveDate,
        path: &str,
        line: u32,
        start: u32,
        end: u32,
    ) -> super::OverlapSpan {
        super::OverlapSpan {
            date,
            path: path.to_string(),
            line,
            start,
            end,
        }
    }

    #[test]
    fn overlap_sweep_counts_shared_minutes_once_for_triple_overlaps() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 9).expect("fixture date");
        let note = "2026/20261009.md";
        // A 08:00-09:00, B 08:30-09:30, C 08:45-09:15: the union covered
        // twice is 08:30-09:15 (45m) even though three pairs overlap.
        let (total, details) = super::detect_overlaps(vec![
            span(day, note, 9, 480, 540),
            span(day, note, 10, 510, 570),
            span(day, note, 11, 525, 555),
        ]);
        assert_eq!(total, 45);
        assert_eq!(details.len(), 3);
        assert_eq!(details[0].lines, vec![9, 10]);
        assert_eq!(details[0].minutes, 30);
        assert_eq!(details[1].lines, vec![9, 11]);
        assert_eq!(details[1].minutes, 15);
        assert_eq!(details[2].lines, vec![10, 11]);
        assert_eq!(details[2].minutes, 30);
        assert!(details.iter().all(|detail| detail.date == day));
        assert!(details.iter().all(|detail| detail.paths.is_none()));
    }

    #[test]
    fn overlap_ignores_touching_endpoints_and_normalizes_overnight() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 9).expect("fixture date");
        let note = "2026/20261009.md";
        // Adjacent sessions share an endpoint but never overlap.
        let (total, details) = super::detect_overlaps(vec![
            span(day, note, 9, 480, 510),
            span(day, note, 10, 510, 540),
        ]);
        assert_eq!(total, 0);
        assert!(details.is_empty());
        // Two overnight spans overlap past midnight after normalization.
        let (total, details) = super::detect_overlaps(vec![
            span(day, note, 9, 1380, 1500),
            span(day, note, 10, 1320, 1410),
        ]);
        assert_eq!(total, 30);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].lines, vec![9, 10]);
        assert_eq!(details[0].minutes, 30);
        assert!(details[0].paths.is_none());
    }

    #[test]
    fn overlap_cross_note_overnight_pair_uses_earlier_date_and_paths() {
        let ninth = NaiveDate::from_ymd_opt(2026, 10, 9).expect("oct 9");
        let tenth = NaiveDate::from_ymd_opt(2026, 10, 10).expect("oct 10");
        // 23:30-00:30 on Oct 9 and 00:15-00:45 on Oct 10 share 15 minutes.
        let (total, details) = super::detect_overlaps(vec![
            span(ninth, "2026/20261009.md", 12, 1410, 1470),
            span(tenth, "2026/20261010.md", 4, 15, 45),
        ]);
        assert_eq!(total, 15);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].date, ninth);
        assert_eq!(details[0].lines, vec![12, 4]);
        assert_eq!(details[0].minutes, 15);
        assert_eq!(
            details[0].paths,
            Some(vec![
                "2026/20261009.md".to_string(),
                "2026/20261010.md".to_string(),
            ])
        );
    }

    #[test]
    fn overlap_details_cap_at_fifty_while_the_aggregate_stays_exact() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 9).expect("fixture date");
        let note = "2026/20261009.md";
        let intervals = (1..=61u32)
            .map(|line| span(day, note, line, 480, 540))
            .collect();
        let (total, details) = super::detect_overlaps(intervals);
        assert_eq!(total, 60);
        assert_eq!(details.len(), 50);
        assert_eq!(details[0].lines, vec![1, 2]);
        assert_eq!(details[49].lines, vec![1, 51]);
        assert!(details.iter().all(|detail| detail.paths.is_none()));
    }

    #[test]
    fn overlap_skips_many_nonoverlapping_rows_on_one_day() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 9).expect("fixture date");
        let note = "2026/20261009.md";
        // 80 adjacent 15-minute sessions: touching endpoints, no overlap.
        let intervals = (0..80u32)
            .map(|index| {
                let start = 8 * 60 + index * 15;
                span(day, note, index + 1, start, start + 15)
            })
            .collect();
        let (total, details) = super::detect_overlaps(intervals);
        assert_eq!(total, 0);
        assert!(details.is_empty());
    }

    #[test]
    fn parsed_task_lookup_skips_fences_frontmatter_and_duplicates() {
        let dir = TempDir::new().unwrap();
        let buffers = vec![
            (
                "tasks.md".to_string(),
                concat!(
                    "---\n",
                    "- [ ] #task Ghost #offclock ^ghost\n",
                    "---\n",
                    "- [ ] #task One ^t\n",
                    "- [ ] #task Two ^t\n",
                    "- [ ] #task Three ^u\n",
                    "```\n",
                    "- [ ] #task Fenced #offclock ^off1\n",
                    "```\n",
                    "- [ ] #task Real ^off1\n",
                    "- [ ] checkbox only ^plain\n",
                )
                .to_string(),
            ),
            (
                "done/old.md".to_string(),
                "- [x] #task Archived #offclock [created:: 2026-10-01] [completion:: 2026-10-09] ^arch1\n"
                    .to_string(),
            ),
        ];
        let lookup = super::ClockTaskLookup::from_buffers(&buffers, dir.path());
        let note_index = crate::native::vault_links::NoteIndex::from_paths(
            buffers
                .iter()
                .map(|(path, _)| std::path::PathBuf::from(path)),
        );
        let mut resolver = crate::native::pomodoro_homes::HomeResolver::new(
            crate::native::pomodoro_homes::IndexedHomeSource::from_buffers(
                &note_index,
                &buffers,
            ),
        );
        assert!(lookup
            .task_off_clock("tasks.md", "t", false, &mut resolver)
            .is_none());
        assert_eq!(
            lookup.task_off_clock("tasks.md", "u", false, &mut resolver),
            Some(false)
        );
        assert!(lookup
            .task_off_clock("tasks.md", "ghost", false, &mut resolver)
            .is_none());
        assert_eq!(
            lookup.task_off_clock("tasks.md", "off1", false, &mut resolver),
            Some(false)
        );
        assert!(lookup
            .task_off_clock("tasks.md", "plain", false, &mut resolver)
            .is_none());
        assert_eq!(
            lookup.task_off_clock("tasks.md", "plain", true, &mut resolver),
            Some(false)
        );
        assert_eq!(
            lookup.task_off_clock("done/old.md", "arch1", false, &mut resolver),
            Some(true)
        );
    }
}
