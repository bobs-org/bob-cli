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
    attribute_session, path_string, unfiled_home, unlinked_home, Home,
    HomeKind, HomeResolver, Hue, IndexedHomeSource, NoteKind, ResolverHealth,
    SessionHomes, SessionKind,
};

pub(crate) fn default_exempt() -> Vec<String> {
    vec!["GTD".to_string()]
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
    pub(crate) project_apportioned: Vec<(String, i64)>,
    pub(crate) area_apportioned: Vec<(String, i64)>,
    pub(crate) lead_project: Option<String>,
    pub(crate) lead_area: Option<String>,
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
        let mut rolled_up = BTreeMap::<String, u32>::new();
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
                        let path_key = path_string(path);
                        if home.key != path_key
                            && home.kind == HomeKind::Project
                        {
                            *rolled_up.entry(home.key.clone()).or_default() +=
                                1;
                        }
                    }
                    NoteKind::Area => {
                        let home = resolver.resolve_note(path);
                        self.remember(&home);
                        let path_key = path_string(path);
                        if home.area == path_key {
                            if let Some(hue) = home.hue {
                                top_areas
                                    .entry(hue)
                                    .or_default()
                                    .push((home.label.clone(), path_key));
                            }
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
            health.rollup_invalid = resolver_health
                .rollup_invalid
                .iter()
                .map(|lint| HomeLintReport {
                    path: lint.path.clone(),
                    raw: lint.raw.clone(),
                })
                .collect();
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
            catalog.insert(
                key.clone(),
                catalog_entry(home, rolled_up.get(key).copied()),
            );
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

/// Parse facts only for notes that can affect rollup, hue, or area color.
/// Refs and dailies without those keys stay unread in the global pass.
fn needs_global_inspect(contents: &str) -> bool {
    let Some(head) = yaml_frontmatter_head(contents) else {
        return false;
    };
    head.contains("[[area]]")
        || head.contains("[[project]]")
        || head.contains("rollup:")
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
            pre_rollup_chain: Vec::new(),
        },
    }
}

fn catalog_entry(home: &Home, rolled_up: Option<u32>) -> HomeCatalogEntry {
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
        project_label,
        rolled_up: rolled_up.filter(|count| *count > 0),
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
    let mut project = Vec::new();
    let mut area = Vec::new();
    if minutes_mode {
        for group in &grouped {
            project.push((group.home.key.clone(), i64::from(group.minutes)));
            add_named(&mut area, &group.home.area, i64::from(group.minutes));
        }
    }
    let project_keys = project
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
    let area_keys = area
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
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
    let lead = grouped
        .iter()
        .max_by(|left, right| {
            left.tasks
                .cmp(&right.tasks)
                .then_with(|| right.first.cmp(&left.first))
        })
        .map(|group| group.home.clone());
    SessionSplit {
        kind: SessionKind::Work,
        project,
        area,
        project_apportioned,
        area_apportioned,
        lead_project: lead.as_ref().map(|home| home.key.clone()),
        lead_area: lead.as_ref().map(|home| home.area.clone()),
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
    let project_keys = project
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
    let area_keys = area
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
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
    let lead = attributed.lead_home.as_ref();
    SessionSplit {
        kind: attributed.homes.kind,
        project,
        area,
        project_apportioned,
        area_apportioned,
        lead_project: lead.map(|home| home.key.clone()),
        lead_area: lead.map(|home| home.area.clone()),
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
    for lint in &health.rollup_invalid {
        let message = if lint.raw == "rollup on non-project note" {
            format!(
                "{}: rollup is only valid on project notes and is treated as off",
                lint.path
            )
        } else {
            format!(
                "{}: invalid rollup {:?}; treated as off",
                lint.path, lint.raw
            )
        };
        warnings.push(StatsWarning {
            code: "rollup_invalid".to_string(),
            metric: None,
            date: None,
            message,
        });
    }
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
        if dimension != "area" && dimension != "project" {
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

        assert_eq!(part_value(worked, "project", "sase.md"), 80);
        assert_eq!(part_value(worked, "project", "@review:GTD"), 35);
        assert_eq!(part_value(worked, "project", "bob.md"), 10);
        assert_eq!(part_value(worked, "project", "dev.md"), 10);
        assert_eq!(part_value(worked, "project", "job.md"), 4);
        assert_eq!(part_value(worked, "project", "job_apply.md"), 3);
        assert_eq!(part_value(worked, "project", "@unlinked"), 10);
        assert_eq!(part_value(worked, "project", "@unfiled"), 15);

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

        let sessions = metric(&report, "sessions");
        assert_eq!(sessions.summary.total.as_ref().unwrap().value, Some(8));
        assert_eq!(dimension_total(sessions, "area"), 8);
        assert_eq!(dimension_total(sessions, "project"), 8);
        assert_eq!(part_value(sessions, "project", "sase.md"), 2);
        assert_eq!(part_value(sessions, "project", "@review:GTD"), 2);

        let done = metric(&report, "done");
        assert_eq!(done.summary.total.as_ref().unwrap().value, Some(7));
        assert_eq!(dimension_total(done, "kind"), 7);
        assert_eq!(dimension_total(done, "area"), 7);
        assert_eq!(dimension_total(done, "project"), 7);
        assert_eq!(dimension_total(done, "note"), 7);
        assert_eq!(part_value(done, "project", "sase.md"), 2);
        assert_eq!(part_value(done, "project", "@unfiled"), 1);
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
        assert_eq!(dimension_total(sase_time, "project"), 10);
        assert_eq!(part_value(sase_time, "project", "sase.md"), 10);
        assert_eq!(part_value(sase_time, "theme", "SASE"), 10);

        // H9 in:[sase] keeps pre-rollup matches, including rolled-up blog
        // and the sase share of HACK; review/unlinked never match.
        let sase_in = metric(&report, "sase_in");
        assert_eq!(sase_in.summary.total.as_ref().unwrap().value, Some(80));
        assert_eq!(dimension_total(sase_in, "area"), 80);
        assert_eq!(dimension_total(sase_in, "project"), 80);
        assert_eq!(part_value(sase_in, "project", "sase.md"), 80);

        // H14 count mode keeps a session once any linked task matches in:.
        let sase_count_in = metric(&report, "sase_count_in");
        assert_eq!(
            sase_count_in.summary.total.as_ref().unwrap().value,
            Some(3)
        );
        assert_eq!(dimension_total(sase_count_in, "area"), 3);
        assert_eq!(dimension_total(sase_count_in, "project"), 3);

        // H10 in:[sase_blog] still measures blog time under rollup.
        let sase_blog_in = metric(&report, "sase_blog_in");
        assert_eq!(
            sase_blog_in.summary.total.as_ref().unwrap().value,
            Some(50)
        );
        assert_eq!(part_value(sase_blog_in, "project", "sase.md"), 50);

        // H11 exclude_in:[sase] never drops review/unlinked; work tasks
        // whose chain matches sase are removed (HACK keeps bob+dev = 20).
        let exclude_sase = metric(&report, "exclude_sase");
        assert_eq!(
            exclude_sase.summary.total.as_ref().unwrap().value,
            Some(87)
        );
        assert_eq!(dimension_total(exclude_sase, "project"), 87);
        assert_eq!(part_value(exclude_sase, "project", "@review:GTD"), 35);
        assert_eq!(part_value(exclude_sase, "project", "@unlinked"), 10);

        // H12 review: only is the GTD review row (named + unnamed bridge).
        let review_only = metric(&report, "review_only");
        assert_eq!(review_only.summary.total.as_ref().unwrap().value, Some(35));
        assert_eq!(part_value(review_only, "project", "@review:GTD"), 35);

        // H13 sessions list: facts for every closed row, today included.
        assert_eq!(report.sessions.len(), 9);
        assert_eq!(report.sessions[0].name.as_deref(), Some("GTD"));
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

        assert_eq!(report.homes["sase.md"].rolled_up, Some(1));
        assert!(!report.homes.contains_key("body.md"));
        assert!(!report.homes.contains_key("invalid.md"));
        assert_eq!(report.health.homes.unlinked_sessions, 1);
        assert_eq!(report.health.homes.unlinked_minutes, 10);
        assert_eq!(report.health.homes.unfiled_links, 1);
        assert!(!report.health.homes.rollup_invalid.is_empty());
        assert!(!report.health.homes.hue_collisions.is_empty());
        assert!(report
            .warnings
            .iter()
            .any(|w| w.code == "area_hue_collision"));
        assert!(report.warnings.iter().any(|w| w.code == "rollup_invalid"));

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
        assert_eq!(part_value(worked, "project", "@review:GTD"), 25);
        assert_eq!(review_time.summary.total.as_ref().unwrap().value, Some(25));
        assert_eq!(part_value(review_time, "project", "@review:GTD"), 25);
        assert_eq!(review_count.summary.total.as_ref().unwrap().value, Some(1));
        assert_eq!(part_value(review_count, "project", "@review:GTD"), 1);
    }
}
