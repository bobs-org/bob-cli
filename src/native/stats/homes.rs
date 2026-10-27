//! Indexed homes engine for stats: session splits, catalog, and health.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

use chrono::NaiveDate;

use super::{
    index::{PomodoroSession, StatsIndex},
    report::{
        HomeCatalogEntry, HomeLintReport, HomesHealthReport,
        HueCollisionReport, SessionFact, StatsWarning,
    },
    session_facts::session_fact,
};
use crate::native::pomodoro_homes::{
    attribute_session, daily_home, path_string, unfiled_home, unlinked_home,
    Home, HomeKind, HomeResolver, Hue, IndexedHomeSource, NoteKind,
    ResolverHealth, SessionHomes, SessionKind, DAILY_KEY,
};

pub(crate) fn default_exempt() -> Vec<String> {
    crate::native::config::PlanConfig::default()
        .exempt()
        .to_vec()
}

pub(crate) struct HomesEngine<'a> {
    resolver: RefCell<HomeResolver<IndexedHomeSource<'a>>>,
    exempt: Vec<String>,
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
    pub(crate) fn new(index: &'a StatsIndex, exempt: &[String]) -> Self {
        let source =
            IndexedHomeSource::from_buffers(&index.note_index, &index.buffers);
        Self {
            resolver: RefCell::new(HomeResolver::new(source)),
            exempt: exempt.to_vec(),
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
            &self.exempt,
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
    ) -> (
        BTreeMap<String, HomeCatalogEntry>,
        HomesHealthReport,
        Vec<StatsWarning>,
        Vec<SessionFact>,
    ) {
        let mut top_areas: BTreeMap<Hue, Vec<(String, String)>> =
            BTreeMap::new();
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
                        let home = resolver.resolve_note(path);
                        self.remember(&home);
                    }
                    NoteKind::Area => {
                        let home = resolver.resolve_note(path);
                        self.remember(&home);
                        let path_key = path_string(path);
                        if home.area == path_key
                            && let Some(hue) = home.hue
                        {
                            top_areas
                                .entry(hue)
                                .or_default()
                                .push((home.label.clone(), path_key));
                        }
                    }
                    NoteKind::Other => {}
                }
            }
        }

        let mut health = HomesHealthReport::default();
        {
            let resolver = self.resolver.borrow();
            let resolver_health: ResolverHealth = resolver.health().clone();
            health.area_color_invalid = resolver_health
                .area_color_invalid
                .iter()
                .map(|lint| HomeLintReport {
                    path: lint.path.clone(),
                    raw: lint.raw.clone(),
                })
                .collect();
        }

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

        let mut sessions = Vec::new();
        for (date, note) in &index.daily_notes {
            if *date < start || *date > end {
                continue;
            }
            let daily_path = Path::new(&note.path);
            for session in &note.closed_pomodoros {
                let minutes =
                    session.recorded_minutes.or(session.range_minutes);
                let attributed = self.attribute(session, daily_path, minutes);
                if collect_sessions {
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
                    SessionKind::Review => {}
                }
            }
        }

        let needed_areas: Vec<String> = self
            .seen
            .borrow()
            .values()
            .map(|home| home.area.clone())
            .filter(|area| !area.starts_with('@'))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        {
            let mut resolver = self.resolver.borrow_mut();
            for area in needed_areas {
                if self.seen.borrow().contains_key(&area) {
                    continue;
                }
                let home = resolver.resolve_note(Path::new(&area));
                self.remember(&home);
            }
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
        if home.root_parent == DAILY_KEY {
            self.seen
                .borrow_mut()
                .entry(DAILY_KEY.to_string())
                .or_insert_with(daily_home);
            return;
        }
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
        "@daily" => daily_home(),
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
            residence_chain: Vec::new(),
            root_parent: key.to_string(),
        },
    }
}

fn catalog_entry(home: &Home) -> HomeCatalogEntry {
    let path = (!home.key.starts_with('@')).then(|| home.key.clone());
    let project_label = (home.kind == HomeKind::Area)
        .then(|| format!("{} · area tasks", home.label));
    let short_label = home
        .key
        .strip_prefix("@review:")
        .map(|name| name.to_string());
    HomeCatalogEntry {
        label: home.label.clone(),
        kind: home.kind,
        area: Some(home.area.clone()),
        subarea: home.subarea.clone(),
        path,
        hue: home.hue,
        project_label,
        short_label,
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
                    None,
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
                "{}: invalid color {:?}; using fallback. Set color: to a palette slot",
                lint.path, lint.raw
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
    warnings
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
                "    review_only:",
                "      label: Review only",
                "      kind: pomodoro",
                "      measure: minutes",
                "      review: only",
                "    review_count:",
                "      label: Review session count",
                "      kind: pomodoro",
                "      measure: count",
                "      review: only",
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
                "review_only",
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

        assert_eq!(part_value(worked, "area", "gtd.md"), 45);
        assert_eq!(part_value(worked, "area", "dev.md"), 90);
        assert_eq!(part_value(worked, "area", "job.md"), 7);
        assert_eq!(part_value(worked, "area", "@unlinked"), 10);
        assert_eq!(part_value(worked, "area", "@unfiled"), 15);

        assert_eq!(part_value(worked, "project", "sase.md"), 30);
        assert_eq!(part_value(worked, "project", "sase_blog.md"), 50);
        assert_eq!(part_value(worked, "project", "@review:PLAN"), 35);
        assert_eq!(part_value(worked, "project", "bob.md"), 10);
        assert_eq!(part_value(worked, "project", "dev.md"), 10);
        assert_eq!(part_value(worked, "project", "job.md"), 4);
        assert_eq!(part_value(worked, "project", "job_apply.md"), 3);
        assert_eq!(part_value(worked, "project", "@unlinked"), 10);
        assert_eq!(part_value(worked, "project", "@unfiled"), 15);

        // H16 worked root_parent sums to 167 with leaf projects folded
        // into their top-level root.
        assert_eq!(dimension_total(worked, "root_parent"), 167);
        assert_eq!(part_value(worked, "root_parent", "sase.md"), 80);
        assert_eq!(part_value(worked, "root_parent", "@daily"), 35);
        assert_eq!(part_value(worked, "root_parent", "bob.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "dev.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "job.md"), 4);
        assert_eq!(part_value(worked, "root_parent", "job_apply.md"), 3);
        assert_eq!(part_value(worked, "root_parent", "@unlinked"), 10);
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
        let review_root = worked.parts_summary["root_parent"]
            .iter()
            .find(|part| part.key == "@daily")
            .unwrap();
        assert_eq!(review_root.apportioned, None);
        // Root labels are bare catalog labels.
        assert_eq!(sase_root.label.as_deref(), Some("sase"));
        assert_eq!(review_root.label.as_deref(), Some("<daily>"));

        let sessions = metric(&report, "sessions");
        assert_eq!(sessions.summary.total.as_ref().unwrap().value, Some(8));
        assert_eq!(dimension_total(sessions, "area"), 8);
        assert_eq!(dimension_total(sessions, "project"), 8);
        assert_eq!(dimension_total(sessions, "root_parent"), 8);
        assert_eq!(part_value(sessions, "project", "sase.md"), 1);
        assert_eq!(part_value(sessions, "project", "sase_blog.md"), 1);
        assert_eq!(part_value(sessions, "project", "@review:PLAN"), 2);
        // Count-mode lead roots group distinct tasks by root.
        assert_eq!(part_value(sessions, "root_parent", "sase.md"), 2);
        assert_eq!(part_value(sessions, "root_parent", "@daily"), 2);

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
        // and the sase share of HACK; review/unlinked never match.
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
            report.homes["@review:PLAN"].root_parent,
            "@daily".to_string()
        );

        // H11 exclude_in:[sase] never drops review/unlinked; work tasks
        // whose chain matches sase are removed (HACK keeps bob+dev = 20).
        let exclude_sase = metric(&report, "exclude_sase");
        assert_eq!(
            exclude_sase.summary.total.as_ref().unwrap().value,
            Some(87)
        );
        assert_eq!(dimension_total(exclude_sase, "project"), 87);
        assert_eq!(part_value(exclude_sase, "project", "@review:PLAN"), 35);
        assert_eq!(part_value(exclude_sase, "project", "@unlinked"), 10);

        // H12 review: only is the PLAN review row (named + unnamed bridge).
        let review_only = metric(&report, "review_only");
        assert_eq!(review_only.summary.total.as_ref().unwrap().value, Some(35));
        assert_eq!(part_value(review_only, "project", "@review:PLAN"), 35);

        // H13 sessions list: facts for every closed row, today included.
        assert_eq!(report.sessions.len(), 9);
        assert_eq!(report.sessions[0].name.as_deref(), Some("PLAN"));
        assert_eq!(report.sessions[0].minutes, Some(25));
        assert_eq!(report.sessions[0].homes.kind, SessionKind::Review);
        assert_eq!(report.sessions[1].name.as_deref(), Some("HACK"));
        assert_eq!(report.sessions[1].homes.kind, SessionKind::Unlinked);
        assert_eq!(report.sessions[2].name.as_deref(), Some("BLOG"));
        assert_eq!(report.sessions[7].name, None);
        assert_eq!(report.sessions[7].homes.kind, SessionKind::Review);
        assert_eq!(report.sessions[8].name.as_deref(), Some("BLOG"));
        assert_eq!(report.sessions[8].minutes, Some(15));
        assert_eq!(
            report.sessions[8].date,
            NaiveDate::from_ymd_opt(2026, 10, 18).unwrap()
        );

        assert!(!report.homes.contains_key("body.md"));
        assert!(!report.homes.contains_key("invalid.md"));
        assert_eq!(report.health.homes.unlinked_sessions, 1);
        assert_eq!(report.health.homes.unlinked_minutes, 10);
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
            ["worked", "review_time", "review_count"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
        request.range = Some("2026-10-14..2026-10-14".to_string());
        request.grouping = Grouping::Day;
        let report = compute(&request).expect("review share vector computes");
        let worked = metric(&report, "worked");
        let review_time = metric(&report, "review_time");
        let review_count = metric(&report, "review_count");
        assert_eq!(worked.summary.total.as_ref().unwrap().value, Some(50));
        assert_eq!(part_value(worked, "project", "@review:PLAN"), 25);
        assert_eq!(review_time.summary.total.as_ref().unwrap().value, Some(25));
        assert_eq!(part_value(review_time, "project", "@review:PLAN"), 25);
        assert_eq!(review_count.summary.total.as_ref().unwrap().value, Some(1));
        assert_eq!(part_value(review_count, "project", "@review:PLAN"), 1);
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
        assert_eq!(part_value(worked, "root_parent", "@daily"), 35);
        assert_eq!(part_value(worked, "root_parent", "bob.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "dev.md"), 10);
        assert_eq!(part_value(worked, "root_parent", "job.md"), 4);
        assert_eq!(part_value(worked, "root_parent", "job_apply.md"), 3);
        assert_eq!(part_value(worked, "root_parent", "@unlinked"), 10);
        assert_eq!(part_value(worked, "root_parent", "@unfiled"), 15);
        // Area and project dimensions match H2 and H3.
        assert_eq!(part_value(worked, "area", "gtd.md"), 45);
        assert_eq!(part_value(worked, "area", "dev.md"), 90);
        assert_eq!(part_value(worked, "area", "job.md"), 7);
        assert_eq!(part_value(worked, "area", "@unlinked"), 10);
        assert_eq!(part_value(worked, "area", "@unfiled"), 15);
        assert_eq!(part_value(worked, "project", "sase.md"), 30);
        assert_eq!(part_value(worked, "project", "sase_blog.md"), 50);
        assert_eq!(part_value(worked, "project", "@review:PLAN"), 35);
        assert_eq!(part_value(worked, "project", "bob.md"), 10);
        assert_eq!(part_value(worked, "project", "dev.md"), 10);
        assert_eq!(part_value(worked, "project", "job.md"), 4);
        assert_eq!(part_value(worked, "project", "job_apply.md"), 3);
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
        assert_eq!(part_value(sessions, "root_parent", "@daily"), 2);
    }

    #[test]
    fn review_bucket_carries_short_label_for_compact_views() {
        let dir = TempDir::new().unwrap();
        let report = compute(&homes_request(homes_vault(), write_config(&dir)))
            .expect("homes fixture computes");
        let review = report
            .homes
            .get("@review:PLAN")
            .expect("review bucket catalog entry");
        assert_eq!(review.label, "PLAN review");
        assert_eq!(review.short_label.as_deref(), Some("PLAN"));
        let daily = report
            .homes
            .get("@daily")
            .expect("daily root catalog entry");
        assert_eq!(daily.label, "<daily>");
        assert_eq!(daily.kind, HomeKind::Bucket);
        assert_eq!(daily.hue, None);
        assert_eq!(daily.short_label, None);
        for (key, entry) in &report.homes {
            if key.starts_with("@review:") {
                continue;
            }
            assert_eq!(
                entry.short_label, None,
                "only review buckets carry short_label ({key})"
            );
        }
        let value = serde_json::to_value(&report).expect("report serializes");
        let catalog = value["homes"].as_object().unwrap();
        assert_eq!(catalog["@review:PLAN"]["short_label"], "PLAN");
        assert_eq!(catalog["@daily"]["label"], "<daily>");
        assert_eq!(catalog["@daily"]["kind"], "bucket");
        assert!(catalog["@daily"]["hue"].is_null());
        assert!(catalog["@daily"].get("short_label").is_none());
        assert!(
            catalog["sase.md"].get("short_label").is_none(),
            "non-review entries omit short_label"
        );
    }
}
