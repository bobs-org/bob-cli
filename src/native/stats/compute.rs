//! Assembled read-only Daily Stats engine entry point.
//!
//! [`compute`] is the assembled read-only engine: request validation, stats
//! config and registry, one vault/journal index pass, dependency evaluation
//! (formula inputs and the journal `review_closed` gap gate), rolling-median
//! lead-in, and series presentation. Evaluators stay pure; this module is
//! the only I/O (config plus the index) and never writes the vault.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;

use super::{
    clock::StatsClock,
    error::StatsError,
    formula::evaluate_formula,
    homes::{label_parts, HomesEngine},
    index::StatsIndex,
    journal::evaluate_journal,
    metrics::{effective_since, evaluate_with_homes, MetricEvaluation},
    range::RangeSpec,
    registry::{
        build_registry, Direction, MetricDef, MetricRegistry, MetricSpec,
    },
    report::{
        ClockOption, ClockReport, HealthReport, JournalHealthReport,
        JournalHostReport, MetricReport, StatsReport, StatsWarning,
        TaskHealthReport, SCHEMA_VERSION,
    },
    request::StatsRequest,
    series::{
        build_metric_series, format_minutes, lead_in_start, resolve_range,
        ResolvedRange,
    },
};
use crate::native::config::{load_stats_config, load_theme_sources_from};
use crate::native::{
    config::themes::ThemeSources,
    plan_budget::normalize_component,
    pomodoro_themes::{Catalog as ThemeCatalog, ThemeRef},
};

/// Compute a schema-1 stats report for one request.
///
/// The report covers the selected metrics in requested (or default overview)
/// order plus the hidden dependencies they need. The prior summary is always
/// included so the headline delta has something to compare against;
/// `prior_points` are filled only when the request sets `compare`. Unknown
/// explicit metric IDs are [`StatsError::unknown_metric`]; invalid configured
/// entries stay local registry warnings and never disable unrelated metrics.
pub(crate) fn compute(
    request: &StatsRequest,
) -> Result<StatsReport, StatsError> {
    let today = request.now.date();
    let config = load_stats_config(&request.config)?;
    let spec = match &request.range {
        Some(text) => RangeSpec::parse(text).map_err(|error| {
            StatsError::usage(format!("stats.range {text:?}: {error}"))
        })?,
        None => config.range.clone(),
    };
    let registry = build_registry(&config);
    let selected = registry.select(&request.selection)?;
    let ordered = evaluation_set(&registry, &selected);
    let index = StatsIndex::read(&request.vault, request.now)?;
    let homes = HomesEngine::new(&index);
    let theme_sources = load_theme_sources_from(&request.config);

    // Probe pass over an empty date span: tracking starts are index-derived,
    // so this yields every metric's inferred since without any daily values.
    // The result feeds `all`-range resolution below.
    let mut probe: BTreeMap<String, MetricEvaluation> = BTreeMap::new();
    for def in &ordered {
        if let Some(evaluation) = evaluate_one(
            def,
            &index,
            &[],
            today,
            config.since,
            None,
            &probe,
            &homes,
            def.canonical_clock(),
            &theme_sources,
        ) {
            probe.insert(def.id.clone(), evaluation);
        }
    }
    let mut display_probe = probe.clone();
    for def in &selected {
        if let Some(effective) = def.effective_clock(request.clock)
            && effective != def.canonical_clock()
            && let Some(evaluation) = evaluate_one(
                def,
                &index,
                &[],
                today,
                config.since,
                None,
                &probe,
                &homes,
                effective,
                &theme_sources,
            )
        {
            display_probe.insert(def.id.clone(), evaluation);
        }
    }
    let all_since = selected
        .iter()
        .filter_map(|def| {
            let inferred = display_probe
                .get(def.id.as_str())
                .and_then(|probe| probe.since);
            effective_since(def, inferred, config.since)
        })
        .min();

    // The prior window always resolves so the summary delta is comparable;
    // `compare` only controls whether prior points are reported.
    let resolved = resolve_range(
        &spec,
        today,
        request.offset,
        request.grouping,
        true,
        all_since,
    )
    .map_err(StatsError::usage)?;
    let dates = evaluation_dates(&resolved, today);
    let range_start = Some(resolved.report.start);

    let mut evaluations: BTreeMap<String, MetricEvaluation> = BTreeMap::new();
    for def in &ordered {
        if let Some(evaluation) = evaluate_one(
            def,
            &index,
            &dates,
            today,
            config.since,
            range_start,
            &evaluations,
            &homes,
            def.canonical_clock(),
            &theme_sources,
        ) {
            evaluations.insert(def.id.clone(), evaluation);
        }
    }
    let mut display = evaluations.clone();
    for def in &selected {
        if let Some(effective) = def.effective_clock(request.clock)
            && effective != def.canonical_clock()
            && let Some(evaluation) = evaluate_one(
                def,
                &index,
                &dates,
                today,
                config.since,
                range_start,
                &evaluations,
                &homes,
                effective,
                &theme_sources,
            )
        {
            display.insert(def.id.clone(), evaluation);
        }
    }

    let health_end = if resolved.includes_today {
        today
    } else {
        resolved.report.end
    };
    let collect_sessions = selected
        .iter()
        .any(|def| matches!(def.spec, MetricSpec::Pomodoro { .. }));
    let (mut catalog, homes_health, homes_warnings, sessions) = homes.finish(
        &index,
        resolved.report.start,
        health_end,
        collect_sessions,
        request.clock,
        &request.vault,
    );

    let mut metrics = Vec::with_capacity(selected.len());
    for def in &selected {
        let evaluation = display
            .get(def.id.as_str())
            .expect("the dispatcher evaluates every selected metric kind");
        let series = build_metric_series(
            &evaluation.values,
            &def.id,
            def.unit,
            &resolved,
            def.kind_name() == "checklist",
        );
        let mut parts_summary = series.parts_summary;
        label_parts(&mut parts_summary, &catalog);
        let effective = def.effective_clock(request.clock);
        let label = match effective {
            Some(clock) => clock.metric_label(&def.id, &def.label),
            None => def.label.clone(),
        };
        let direction = if effective.is_some_and(StatsClock::is_neutral) {
            Direction::Neutral.as_str()
        } else {
            def.direction.as_str()
        };
        let mut parameters = def.parameters();
        if let Some(clock) = effective {
            parameters.clock = Some(clock.as_str().to_string());
        }
        metrics.push(MetricReport {
            id: def.id.clone(),
            label,
            kind: def.kind_name().to_string(),
            unit: def.unit.as_str().to_string(),
            direction: direction.to_string(),
            color: def.color.as_str().to_string(),
            source: def.source.as_str().to_string(),
            origin: def.origin.as_str().to_string(),
            since: evaluation.since,
            definition: def.definition.clone(),
            limits: def.limits.clone(),
            parameters: Some(parameters),
            summary: series.summary,
            chart: series.chart,
            points: series.points,
            prior_points: if request.compare {
                series.prior_points
            } else {
                Vec::new()
            },
            daily_points: series.daily_points,
            parts_summary,
            chores: series.chores,
            warnings: evaluation.warnings.clone(),
        });
    }

    prune_home_catalog(&mut catalog, &metrics, &sessions);
    let theme_hues = report_theme_hues(&request.vault, today, &metrics);

    let mut health = index_health(&index);
    health.homes = homes_health;
    let clock_end = if resolved.includes_today {
        today
    } else {
        resolved.report.end
    };
    Ok(StatsReport {
        ok: true,
        schema_version: SCHEMA_VERSION,
        today,
        range: resolved.report.clone(),
        clock: clock_report(
            &index,
            request.clock,
            resolved.report.start,
            clock_end,
        ),
        health,
        warnings: dedupe_warnings(
            registry
                .warnings
                .iter()
                .cloned()
                .chain(
                    metrics.iter().flat_map(|metric| metric.warnings.clone()),
                )
                .chain(homes_warnings)
                .collect(),
        ),
        metrics,
        homes: catalog,
        theme_hues,
        sessions,
    })
}

fn report_theme_hues(
    vault: &std::path::Path,
    today: NaiveDate,
    metrics: &[MetricReport],
) -> BTreeMap<String, ThemeRef> {
    let mut keys = BTreeSet::new();
    for metric in metrics {
        for dimension in ["theme", "task_type"] {
            if let Some(parts) = metric.parts_summary.get(dimension) {
                keys.extend(
                    parts.iter().map(|part| normalize_component(&part.key)),
                );
            }
        }
        for point in metric
            .points
            .iter()
            .chain(&metric.prior_points)
            .chain(&metric.daily_points)
        {
            for dimension in ["theme", "task_type"] {
                if let Some(parts) = point.parts.get(dimension) {
                    keys.extend(
                        parts.keys().map(|key| normalize_component(key)),
                    );
                }
            }
            if let Some(parts) = point.apportioned.get("theme") {
                keys.extend(parts.keys().map(|key| normalize_component(key)));
            }
        }
    }
    keys.remove("(unnamed)");
    keys.retain(|key| !key.starts_with('@'));
    if keys.is_empty() {
        return BTreeMap::new();
    }
    let Ok(catalog) = ThemeCatalog::load(vault, today, false) else {
        return BTreeMap::new();
    };
    keys.into_iter()
        .map(|key| {
            let display = catalog.display_name(&key, &key.to_ascii_uppercase());
            (key.clone(), catalog.theme_ref(&key, &display))
        })
        .collect()
}

fn prune_home_catalog(
    catalog: &mut BTreeMap<String, super::report::HomeCatalogEntry>,
    metrics: &[MetricReport],
    sessions: &[super::report::SessionFact],
) {
    let mut used = BTreeSet::new();
    for metric in metrics {
        for (dimension, parts) in &metric.parts_summary {
            if dimension == "area"
                || dimension == "project"
                || dimension == "root_parent"
            {
                used.extend(parts.iter().map(|part| part.key.clone()));
            }
        }
        for point in metric
            .points
            .iter()
            .chain(&metric.prior_points)
            .chain(&metric.daily_points)
        {
            for dimension in ["area", "project", "root_parent"] {
                if let Some(parts) = point.parts.get(dimension) {
                    used.extend(parts.keys().cloned());
                }
                if let Some(parts) = point.apportioned.get(dimension) {
                    used.extend(parts.keys().cloned());
                }
            }
        }
    }
    for session in sessions {
        for part in &session.homes.parts {
            used.insert(part.key.clone());
            if let Some(area) = &part.area {
                used.insert(area.clone());
            }
        }
    }
    let referenced = used
        .iter()
        .filter_map(|key| catalog.get(key))
        .flat_map(|entry| {
            entry
                .area
                .iter()
                .chain(&entry.subarea)
                .chain(std::iter::once(&entry.root_parent))
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    used.extend(referenced);
    catalog.retain(|key, _| used.contains(key));
}

/// Selected metrics plus their dependencies in evaluation order. Journal
/// metrics pull `review_closed` through [`MetricRegistry::with_dependencies`]
/// so the gap gate evaluates first even when it is hidden or unselected.
fn evaluation_set<'a>(
    registry: &'a MetricRegistry,
    selected: &[&'a MetricDef],
) -> Vec<&'a MetricDef> {
    registry.with_dependencies(selected)
}

/// Dispatch one metric to its kind evaluator. The match covers every spec
/// variant, so this always returns an evaluation for registry definitions.
#[allow(clippy::too_many_arguments)]
fn evaluate_one(
    def: &MetricDef,
    index: &StatsIndex,
    dates: &[NaiveDate],
    today: NaiveDate,
    global_since: Option<NaiveDate>,
    range_start: Option<NaiveDate>,
    evaluations: &BTreeMap<String, MetricEvaluation>,
    homes: &HomesEngine<'_>,
    clock: StatsClock,
    theme_sources: &ThemeSources,
) -> Option<MetricEvaluation> {
    match &def.spec {
        MetricSpec::Pomodoro { .. }
        | MetricSpec::Tasks { .. }
        | MetricSpec::Checklist { .. }
        | MetricSpec::Field { .. } => evaluate_with_homes(
            def,
            index,
            dates,
            today,
            global_since,
            homes,
            clock,
            theme_sources,
        ),
        MetricSpec::Journal { .. } => evaluate_journal(
            def,
            index,
            dates,
            today,
            global_since,
            evaluations,
        ),
        MetricSpec::Formula { .. } => evaluate_formula(
            def,
            evaluations,
            dates,
            today,
            global_since,
            range_start,
        ),
    }
}

fn clock_report(
    index: &StatsIndex,
    clock: StatsClock,
    start: NaiveDate,
    end: NaiveDate,
) -> ClockReport {
    ClockReport {
        value: clock.as_str().to_string(),
        label: clock.label().to_string(),
        since: index.off_clock_adoption(),
        options: StatsClock::ALL
            .into_iter()
            .map(|option| {
                let minutes = index.window_clock_minutes(option, start, end);
                ClockOption {
                    value: option.as_str().to_string(),
                    label: option.label().to_string(),
                    description: option.description().to_string(),
                    minutes,
                    display: format_minutes(minutes),
                }
            })
            .collect(),
    }
}

/// Every date the evaluators must cover: rolling-median lead-in, the prior
/// window (for the always included prior summary and, under `compare`, prior
/// points), the complete window, and the provisional today point.
fn evaluation_dates(
    resolved: &ResolvedRange,
    today: NaiveDate,
) -> Vec<NaiveDate> {
    let displayed_start = resolved
        .prior
        .map(|span| span.start)
        .into_iter()
        .chain(resolved.complete.map(|span| span.start))
        .chain([today])
        .min()
        .unwrap_or(today);
    let start = lead_in_start(displayed_start, resolved.group)
        .unwrap_or(displayed_start)
        .min(displayed_start);
    let mut dates = Vec::new();
    let mut date = start;
    while date <= today {
        dates.push(date);
        match date.succ_opt() {
            Some(next) => date = next,
            None => break,
        }
    }
    dates
}

fn index_health(index: &StatsIndex) -> HealthReport {
    HealthReport {
        journal: JournalHealthReport {
            since: index.health.journal.since,
            malformed_lines: index.health.journal.malformed_lines,
            hosts: index
                .health
                .journal
                .hosts
                .iter()
                .map(|host| JournalHostReport {
                    host: host.host.clone(),
                    events: host.events,
                    last_ts: host.last_ts.clone(),
                    last_day: host.last_day,
                })
                .collect(),
        },
        tasks: TaskHealthReport {
            invalid_dates: index.health.invalid_dates,
            closed_without_date: index.health.closed_without_date,
            missing_created: (index.health.missing_created > 0)
                .then_some(index.health.missing_created),
        },
        homes: Default::default(),
    }
}

fn dedupe_warnings(warnings: Vec<StatsWarning>) -> Vec<StatsWarning> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::with_capacity(warnings.len());
    for warning in warnings {
        let key = (
            warning.code.clone(),
            warning.metric.clone(),
            warning.date,
            warning.message.clone(),
        );
        if seen.insert(key) {
            deduped.push(warning);
        }
    }
    deduped
}

#[cfg(test)]
mod clock_breakdown_tests;

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};

    use chrono::{Datelike, NaiveDate, NaiveDateTime};
    use tempfile::TempDir;

    use super::compute;
    use crate::native::env::TestEnvGuard;
    use crate::native::stats::clock::StatsClock;
    use crate::native::stats::report::SeriesPoint;
    use crate::native::stats::request::{
        Grouping, MetricSelection, StatsRequest,
    };
    use crate::native::stats::value::{ChecklistState, DataQuality};

    /// Fixed clock for the golden vault: Sunday 2026-10-11 at noon.
    fn fixed_now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 11)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn day(value: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, value).unwrap()
    }

    fn fixture_vault() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/stats/vault"
        ))
    }

    /// Stats config exercising every metric kind through the same compute
    /// path: the eleven built-ins plus one custom metric per kind.
    fn golden_config() -> String {
        [
            "stats:",
            "  metrics:",
            "    sase_time:",
            "      label: SASE time",
            "      kind: pomodoro",
            "      measure: minutes",
            "      themes: [SASE]",
            "    ref_done:",
            "      label: Ref done",
            "      kind: tasks",
            "      event: done",
            "      task_kind: ref",
            "    habit_count:",
            "      label: Habit count",
            "      kind: checklist",
            "      tags: [gtd, pre]",
            "      measure: count",
            "    energy:",
            "      label: Energy",
            "      kind: field",
            "      field: energy",
            "      source: either",
            "      unit: count",
            "    all_reviewed:",
            "      label: All reviewed",
            "      kind: journal",
            "      select: reviewed",
            "      tiers: all",
            "    done_and_dropped:",
            "      label: Done and dropped",
            "      kind: formula",
            "      expr: done + dropped",
            "      unit: signed",
            "",
        ]
        .join("\n")
    }

    fn write_config(dir: &TempDir, text: &str) -> PathBuf {
        let path = dir.path().join("config.yml");
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The golden request: all eleven built-ins plus the six customs over
    /// the explicit 2026-10-08..2026-10-11 window (complete 10-08..10-10,
    /// prior 10-05..10-07, provisional today 10-11) with day grouping.
    fn golden_request(vault: PathBuf, config: PathBuf) -> StatsRequest {
        let mut request = StatsRequest::at(vault, config, fixed_now());
        request.selection = MetricSelection::Explicit(
            [
                "worked",
                "reviewed",
                "done",
                "created",
                "net",
                "review_closed",
                "dropped",
                "habits",
                "keep_rate",
                "review_time",
                "sessions",
                "sase_time",
                "ref_done",
                "habit_count",
                "energy",
                "all_reviewed",
                "done_and_dropped",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        );
        request.range = Some("2026-10-08..2026-10-11".to_string());
        request.grouping = Grouping::Day;
        request.compare = true;
        request
    }

    fn golden_report() -> crate::native::stats::report::StatsReport {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let _config_guard =
            TestEnvGuard::set(&[("BOB_CONFIG_FILE", Some(config.as_os_str()))]);
        let request = golden_request(fixture_vault(), config);
        compute(&request).expect("golden fixture computes")
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

    fn expect_point(
        point: &SeriesPoint,
        date: NaiveDate,
        value: Option<i64>,
        quality: DataQuality,
        provisional: bool,
    ) {
        assert_eq!(point.date, date, "point date");
        assert_eq!(point.end, date, "day point end");
        assert_eq!(point.value, value, "value on {}", date);
        assert_eq!(point.quality, quality, "quality on {}", date);
        assert_eq!(point.provisional, provisional, "provisional on {}", date);
        assert_eq!(point.partial, None, "day points never carry partial");
        if provisional {
            assert_eq!(
                point.rolling_median, None,
                "today has no rolling median"
            );
        }
    }

    fn untracked(point: &SeriesPoint) {
        assert_eq!(point.quality, DataQuality::NotTracked);
        assert_eq!(point.value, None);
        assert_eq!(point.reason.as_deref(), Some("before tracking began"));
    }

    #[test]
    fn golden_vault_reports_all_builtins_and_every_custom_kind() {
        let report = golden_report();
        assert!(report.ok);
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.today, day(11));
        assert_eq!(report.theme_hues["plan"].hue, None);
        assert_eq!(
            report.theme_hues["plan"].hue_source,
            crate::native::pomodoro_themes::HueSource::Exempt
        );
        assert!(report.theme_hues["sase"].known);
        assert_eq!(report.warnings.len(), 4);
        assert!(report
            .warnings
            .iter()
            .filter(|warning| warning.code != "clock_unstamped")
            .all(|warning| warning.code == "journal_gap"));
        assert_eq!(
            report
                .warnings
                .iter()
                .filter(|warning| warning.code == "clock_unstamped")
                .count(),
            1
        );

        // Explicit ranges carry no preset; the end stays yesterday while
        // today rides along as a provisional point.
        assert_eq!(report.range.preset, None);
        assert_eq!(report.range.offset, 0);
        assert_eq!(report.range.start, day(8));
        assert_eq!(report.range.end, day(10));
        assert_eq!(report.range.days, 3);
        assert!(report.range.includes_today);
        let prior = report.range.prior.as_ref().expect("prior resolves");
        assert_eq!(prior.start, day(5));
        assert_eq!(prior.end, day(7));

        // Selection order is preserved through the report.
        let ids: Vec<&str> = report
            .metrics
            .iter()
            .map(|metric| metric.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "worked",
                "reviewed",
                "done",
                "created",
                "net",
                "review_closed",
                "dropped",
                "habits",
                "keep_rate",
                "review_time",
                "sessions",
                "sase_time",
                "ref_done",
                "habit_count",
                "energy",
                "all_reviewed",
                "done_and_dropped",
            ]
        );

        // M1 worked: 25m PLAN plus the odd 7m PLAN+SASE composite (4m PLAN,
        // 3m SASE); Oct 8 predates Pomodoro tracking; Oct 10's open-only
        // section is a real zero; today is provisional.
        let worked = metric(&report, "worked");
        assert_eq!(worked.kind, "pomodoro");
        assert_eq!(worked.unit, "minutes");
        assert_eq!(worked.origin, "builtin");
        assert_eq!(worked.source, "derived");
        assert_eq!(worked.since, Some(day(9)));
        expect_point(
            &worked.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        untracked(&worked.points[0]);
        expect_point(
            &worked.points[1],
            day(9),
            Some(32),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &worked.points[2],
            day(10),
            Some(0),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &worked.points[3],
            day(11),
            Some(10),
            DataQuality::Ok,
            true,
        );
        assert_eq!(worked.points[1].display, "32m");
        assert_eq!(worked.points[3].display, "10m");
        assert_eq!(worked.points[1].parts["theme"]["PLAN"], 29);
        assert_eq!(worked.points[1].parts["theme"]["SASE"], 3);
        assert_eq!(worked.summary.headline.value, Some(16));
        assert_eq!(worked.summary.headline.display, "16m");
        assert_eq!(worked.summary.total.as_ref().unwrap().value, Some(32));
        assert_eq!(worked.summary.mean, Some(16));
        assert_eq!(worked.summary.p25, Some(8));
        assert_eq!(worked.summary.p75, Some(24));
        assert_eq!(worked.summary.days_with_data, 2);
        assert_eq!(worked.summary.delta.as_ref().unwrap().display, "–");
        assert_eq!(
            worked.chart.group,
            crate::native::stats::report::GroupName::Day
        );
        assert_eq!(worked.chart.p25, worked.summary.p25);
        assert_eq!(worked.chart.p75, worked.summary.p75);
        assert!(worked.daily_points.is_empty());
        assert_eq!(worked.points[1].parts_display["theme"]["PLAN"], "29m");
        assert_eq!(worked.points[1].parts_display["theme"]["SASE"], "3m");
        let theme = &worked.parts_summary["theme"];
        assert_eq!(theme[0].key, "PLAN");
        assert_eq!(theme[0].value, 29);
        assert_eq!(theme[0].share, "91%");
        assert_eq!(theme[1].key, "SASE");
        assert_eq!(theme[1].value, 3);
        assert_eq!(theme[1].share, "9%");

        // M2 reviewed: Oct 8 has only a pre-tier event (real zero, not a
        // gap); Oct 9 closed review with no events (journal gap); Oct 10
        // answers one row twice (last answer wins) plus a second row.
        let reviewed = metric(&report, "reviewed");
        assert_eq!(reviewed.source, "recorded");
        assert_eq!(reviewed.since, Some(day(8)));
        expect_point(
            &reviewed.points[0],
            day(8),
            Some(0),
            DataQuality::Ok,
            false,
        );
        assert!(reviewed.points[0].parts.is_empty());
        expect_point(
            &reviewed.points[1],
            day(9),
            None,
            DataQuality::Missing,
            false,
        );
        assert_eq!(
            reviewed.points[1].reason.as_deref(),
            Some("review closed but no answers were recorded")
        );
        expect_point(
            &reviewed.points[2],
            day(10),
            Some(2),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &reviewed.points[3],
            day(11),
            Some(1),
            DataQuality::Ok,
            true,
        );
        assert_eq!(reviewed.points[2].parts["tier"]["next"], 1);
        assert_eq!(reviewed.points[2].parts["tier"]["pending"], 1);
        assert_eq!(reviewed.points[2].parts["outcome"]["done"], 1);
        assert_eq!(reviewed.points[2].parts["outcome"]["keep"], 1);
        assert_eq!(reviewed.summary.headline.value, Some(1));
        assert_eq!(reviewed.summary.days_with_data, 2);
        assert_eq!(reviewed.warnings.len(), 1);
        assert_eq!(reviewed.warnings[0].code, "journal_gap");
        assert_eq!(reviewed.warnings[0].date, Some(day(9)));

        // M3 done: five completions on Oct 9 (the ^gtd row, the recurring
        // row, and the reopened open-status row never count); the malformed
        // created date still leaves a valid completion; the archived row
        // counts under its source residence.
        let done = metric(&report, "done");
        assert_eq!(done.since, Some(day(9)));
        expect_point(
            &done.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(&done.points[1], day(9), Some(5), DataQuality::Ok, false);
        expect_point(&done.points[2], day(10), Some(4), DataQuality::Ok, false);
        expect_point(&done.points[3], day(11), Some(1), DataQuality::Ok, true);
        assert_eq!(done.points[1].parts["kind"]["task"], 4);
        assert_eq!(done.points[1].parts["kind"]["ref"], 1);
        assert_eq!(done.points[2].parts["kind"]["task"], 3);
        assert_eq!(done.points[2].parts["kind"]["prj"], 1);
        assert_eq!(done.points[1].parts["note"]["tasks.md"], 4);
        assert_eq!(done.points[1].parts["note"]["2026/20261009.md"], 1);
        assert_eq!(done.summary.headline.value, Some(5));
        assert_eq!(done.summary.total.as_ref().unwrap().value, Some(9));
        assert_eq!(done.summary.mean, Some(5));
        assert_eq!(done.summary.p25, Some(4));
        assert_eq!(done.summary.p75, Some(5));
        assert_eq!(done.summary.days_with_data, 2);
        let kinds = &done.parts_summary["kind"];
        assert_eq!(kinds[0].key, "task");
        assert_eq!(kinds[0].value, 7);
        assert_eq!(kinds[0].share, "78%");
        assert_eq!(kinds[1].key, "prj");
        assert_eq!(kinds[1].value, 1);
        assert_eq!(kinds[1].share, "11%");
        assert_eq!(kinds[2].key, "ref");
        assert_eq!(kinds[2].value, 1);
        assert_eq!(kinds[2].share, "11%");

        // M4 created: the archived win surfaces on Oct 7 (prior window);
        // Oct 9 counts five (Morning review and Meditate are tasks too).
        let created = metric(&report, "created");
        assert_eq!(created.since, Some(day(7)));
        expect_point(
            &created.points[0],
            day(8),
            Some(2),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &created.points[1],
            day(9),
            Some(5),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &created.points[2],
            day(10),
            Some(5),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &created.points[3],
            day(11),
            Some(1),
            DataQuality::Ok,
            true,
        );
        assert_eq!(created.points[1].parts["note"]["tasks.md"], 1);
        assert_eq!(created.summary.headline.value, Some(5));
        assert_eq!(created.summary.total.as_ref().unwrap().value, Some(12));
        assert_eq!(created.summary.mean, Some(4));
        assert_eq!(created.summary.p25, Some(4));
        assert_eq!(created.summary.p75, Some(5));
        assert_eq!(created.summary.days_with_data, 3);
        let prior = created.summary.prior.as_ref().expect("prior summary");
        assert_eq!(prior.headline.value, Some(1));
        let delta = created.summary.delta.as_ref().expect("delta");
        assert_eq!(delta.display, "▲ 5×");
        assert_eq!(delta.words, "up 5 times vs the prior selected period");
        assert_eq!(delta.sign, 1);
        // Prior points cover Oct 5..7: untracked, untracked, then the
        // archived win's creation day.
        assert_eq!(created.prior_points.len(), 3);
        expect_point(
            &created.prior_points[0],
            day(5),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &created.prior_points[1],
            day(6),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &created.prior_points[2],
            day(7),
            Some(1),
            DataQuality::Ok,
            false,
        );

        // M5 net: nulls propagate before tracking starts (latest dependency
        // since is Oct 10); Oct 10 flows 5 - 4 - 2 and shrinks.
        let net = metric(&report, "net");
        assert_eq!(net.unit, "signed");
        assert_eq!(net.since, Some(day(10)));
        expect_point(
            &net.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &net.points[1],
            day(9),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(&net.points[2], day(10), Some(-1), DataQuality::Ok, false);
        expect_point(&net.points[3], day(11), Some(0), DataQuality::Ok, true);
        assert_eq!(net.points[2].display, "−1");
        assert_eq!(net.summary.headline.value, Some(-1));
        assert_eq!(net.summary.headline.display, "−1");
        assert_eq!(net.summary.headline.suffix.as_deref(), Some("total"));
        assert_eq!(net.summary.headline.stat.as_deref(), Some("total"));
        assert_eq!(net.summary.days_with_data, 1);
        let delta = net.summary.delta.as_ref().expect("net delta");
        assert_eq!(delta.display, "shrinking");
        assert_eq!(delta.sign, -1);

        // M6 review_closed: met Oct 9 and 10 from daily notes plus the
        // archived gtd_daily history; today's POST row in gtd_daily.md is
        // still a TODO, so today is missed (0) rather than nothing-due.
        let closed = metric(&report, "review_closed");
        assert_eq!(closed.unit, "bool");
        assert_eq!(closed.since, Some(day(9)));
        expect_point(
            &closed.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &closed.points[1],
            day(9),
            Some(1),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &closed.points[2],
            day(10),
            Some(1),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &closed.points[3],
            day(11),
            Some(0),
            DataQuality::Ok,
            true,
        );
        assert_eq!(closed.points[1].display, "met");
        assert_eq!(closed.points[3].display, "not yet");
        assert_eq!(closed.summary.headline.value, Some(2));
        assert_eq!(closed.summary.headline.display, "2 of 2 days");
        assert_eq!(closed.summary.days_with_data, 2);
        assert_eq!(closed.summary.days_met, Some(2));
        assert_eq!(
            closed.points[0].checklist_state,
            Some(ChecklistState::NotTracked)
        );
        assert_eq!(closed.points[1].checklist_state, Some(ChecklistState::Met));
        assert_eq!(closed.points[2].checklist_state, Some(ChecklistState::Met));
        assert_eq!(
            closed.points[3].checklist_state,
            Some(ChecklistState::Missed)
        );
        assert_eq!(closed.chores.len(), 1);
        assert_eq!(closed.chores[0].key, "morning review");
        assert_eq!(closed.chores[0].met, 2);
        assert_eq!(closed.chores[0].due, 2);
        assert_eq!(closed.chores[0].display, "2 of 2 days");
        let delta = closed.summary.delta.as_ref().expect("bool delta");
        assert_eq!(delta.display, "–");
        assert_eq!(delta.sign, 0);

        // M7 dropped: [!] and [-] CANCELLED rows with cancelled dates.
        let dropped = metric(&report, "dropped");
        assert_eq!(dropped.since, Some(day(10)));
        expect_point(
            &dropped.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &dropped.points[1],
            day(9),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &dropped.points[2],
            day(10),
            Some(2),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &dropped.points[3],
            day(11),
            Some(0),
            DataQuality::Ok,
            true,
        );
        assert_eq!(dropped.summary.headline.value, Some(2));
        assert_eq!(dropped.summary.days_with_data, 1);

        // M8 habits: Journal (gtd_daily.md) is skipped every day; Meditate
        // misses Oct 9, completes late on Oct 10 (archive row), and is
        // still outstanding today via the daily-note TODO. Stretch is
        // deferred to Oct 20 and never due. Ratios pool 0/2 + 1/2 = 25%.
        let habits = metric(&report, "habits");
        assert_eq!(habits.unit, "ratio");
        assert_eq!(habits.since, Some(day(9)));
        expect_point(
            &habits.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &habits.points[1],
            day(9),
            Some(0),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &habits.points[2],
            day(10),
            Some(5000),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &habits.points[3],
            day(11),
            Some(0),
            DataQuality::Ok,
            true,
        );
        assert_eq!(habits.points[1].num, Some(0));
        assert_eq!(habits.points[1].den, Some(2));
        assert_eq!(habits.points[1].parts["missed"]["meditate"], 1);
        assert_eq!(habits.points[1].parts["missed"]["journal"], 1);
        assert_eq!(habits.points[2].num, Some(1));
        assert_eq!(habits.points[2].den, Some(2));
        assert_eq!(habits.points[2].parts["met"]["meditate"], 1);
        assert_eq!(habits.points[2].parts["missed"]["journal"], 1);
        assert_eq!(habits.points[2].display, "50%");
        assert_eq!(habits.summary.headline.value, Some(2500));
        assert_eq!(habits.summary.headline.display, "25%");
        assert_eq!(habits.summary.mean, Some(2500));
        assert_eq!(habits.summary.p25, Some(1250));
        assert_eq!(habits.summary.p75, Some(3750));
        assert_eq!(habits.summary.days_with_data, 2);
        assert_eq!(habits.chart.p25, Some(1250));
        assert_eq!(habits.chart.p75, Some(3750));
        assert_eq!(
            habits.points[0].checklist_state,
            Some(ChecklistState::NotTracked)
        );
        assert_eq!(
            habits.points[1].checklist_state,
            Some(ChecklistState::Missed)
        );
        assert_eq!(
            habits.points[2].checklist_state,
            Some(ChecklistState::Missed)
        );
        assert_eq!(
            habits.points[3].checklist_state,
            Some(ChecklistState::Missed)
        );
        assert_eq!(habits.chores.len(), 2);
        assert_eq!(habits.chores[0].key, "journal");
        assert_eq!(habits.chores[0].met, 0);
        assert_eq!(habits.chores[0].due, 2);
        assert_eq!(habits.chores[0].display, "0 of 2 days");
        assert_eq!(habits.chores[1].key, "meditate");
        assert_eq!(habits.chores[1].met, 1);
        assert_eq!(habits.chores[1].due, 2);
        assert_eq!(habits.chores[1].display, "1 of 2 days");
        assert_eq!(habits.points[1].parts_display["missed"]["journal"], "1");

        // M9 keep_rate: one keep of two answers on Oct 10; the gap day and
        // the pre-only day carry no denominator.
        let keep = metric(&report, "keep_rate");
        assert_eq!(keep.unit, "ratio");
        assert_eq!(keep.since, Some(day(8)));
        expect_point(
            &keep.points[0],
            day(8),
            None,
            DataQuality::Missing,
            false,
        );
        assert_eq!(
            keep.points[0].reason.as_deref(),
            Some("no answers in tiers [pending, next] were recorded")
        );
        expect_point(
            &keep.points[1],
            day(9),
            None,
            DataQuality::Missing,
            false,
        );
        expect_point(
            &keep.points[2],
            day(10),
            Some(5000),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &keep.points[3],
            day(11),
            Some(10000),
            DataQuality::Ok,
            true,
        );
        assert_eq!(keep.points[2].num, Some(1));
        assert_eq!(keep.points[2].den, Some(2));
        assert_eq!(keep.summary.headline.value, Some(5000));
        assert_eq!(keep.summary.days_with_data, 1);
        assert_eq!(keep.warnings.len(), 1);
        assert_eq!(keep.warnings[0].code, "journal_gap");

        // M10 review_time: minutes attributed to tasks in, or rooted at,
        // gtd. The Oct 9 PLAN rows carry no Task Links, so they are
        // unlinked and never match `in:`; the whole window reads zero.
        let review_time = metric(&report, "review_time");
        assert_eq!(review_time.since, Some(day(9)));
        assert_eq!(
            review_time.parameters.as_ref().map(|p| p.in_notes.clone()),
            Some(vec!["gtd".to_string()])
        );
        assert!(review_time
            .parameters
            .as_ref()
            .is_some_and(|p| p.themes.is_empty()));
        expect_point(
            &review_time.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &review_time.points[1],
            day(9),
            Some(0),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &review_time.points[2],
            day(10),
            Some(0),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &review_time.points[3],
            day(11),
            Some(0),
            DataQuality::Ok,
            true,
        );
        assert_eq!(review_time.summary.headline.value, Some(0));
        assert_eq!(review_time.summary.total.as_ref().unwrap().value, Some(0));
        assert_eq!(review_time.summary.mean, Some(0));
        assert_eq!(review_time.summary.p25, Some(0));
        assert_eq!(review_time.summary.p75, Some(0));

        // The Worked lens filters the bare off-clock MEET row out of the
        // drill-down; Total brings it back with real span minutes.
        assert_eq!(report.sessions.len(), 3);
        assert_eq!(report.sessions[0].name.as_deref(), Some("PLAN"));
        assert_eq!(report.sessions[0].minutes, Some(25));
        assert_eq!(report.sessions[0].quality, DataQuality::Ok);
        assert!(!report.sessions[0].off_clock);
        assert_eq!(report.sessions[1].name.as_deref(), Some("PLAN + SASE"));
        assert_eq!(report.sessions[1].minutes, Some(7));
        assert!(!report.sessions[1].off_clock);
        assert_eq!(report.sessions[2].name.as_deref(), Some("SASE"));
        assert_eq!(report.sessions[2].minutes, Some(10));
        assert_eq!(report.sessions[2].date, day(11));
        assert!(!report.sessions[2].off_clock);
        assert!(!report.sessions[2].homes.untracked);
        assert!(!report.sessions[2].homes.off_clock);
        assert_eq!(report.health.homes.untracked_sessions, 1);
        assert_eq!(report.health.homes.off_clock_sessions, 1);
        assert_eq!(report.health.homes.unstamped_sessions, 1);
        assert_eq!(report.health.homes.clock_conflicts, 0);
        assert_eq!(report.health.homes.clock_drift, 0);
        assert_eq!(report.health.homes.overlap_minutes, 0);
        assert!(report.health.homes.overlaps.is_empty());

        // M11 sessions: count mode counts each closed row once even when it
        // carries two themes; parts still attribute the row to PLAN.
        let sessions = metric(&report, "sessions");
        assert_eq!(sessions.since, Some(day(9)));
        expect_point(
            &sessions.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &sessions.points[1],
            day(9),
            Some(2),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &sessions.points[2],
            day(10),
            Some(0),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &sessions.points[3],
            day(11),
            Some(1),
            DataQuality::Ok,
            true,
        );
        assert_eq!(sessions.points[1].parts["theme"]["PLAN"], 2);
        assert_eq!(sessions.summary.headline.value, Some(1));
        assert_eq!(sessions.summary.total.as_ref().unwrap().value, Some(2));

        // Custom pomodoro / tasks / checklist / field metrics.
        let sase = metric(&report, "sase_time");
        assert_eq!(sase.origin, "custom");
        expect_point(&sase.points[1], day(9), Some(3), DataQuality::Ok, false);
        expect_point(&sase.points[2], day(10), Some(0), DataQuality::Ok, false);
        expect_point(&sase.points[3], day(11), Some(10), DataQuality::Ok, true);
        assert_eq!(sase.summary.headline.value, Some(2));
        assert_eq!(sase.summary.total.as_ref().unwrap().value, Some(3));
        let summary = &sase.parts_summary["theme"][0];
        assert_eq!(summary.key, "SASE");
        assert_eq!(summary.value, 3);

        let ref_done = metric(&report, "ref_done");
        assert_eq!(ref_done.origin, "custom");
        assert_eq!(
            ref_done.parameters.as_ref().unwrap().task_kind.as_deref(),
            Some("ref")
        );
        expect_point(
            &ref_done.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &ref_done.points[1],
            day(9),
            Some(1),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &ref_done.points[2],
            day(10),
            Some(0),
            DataQuality::Ok,
            false,
        );
        assert_eq!(ref_done.points[1].parts["kind"]["ref"], 1);
        assert_eq!(ref_done.summary.headline.value, Some(1));

        let habit_count = metric(&report, "habit_count");
        expect_point(
            &habit_count.points[1],
            day(9),
            Some(0),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &habit_count.points[2],
            day(10),
            Some(1),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &habit_count.points[3],
            day(11),
            Some(0),
            DataQuality::Ok,
            true,
        );
        assert_eq!(habit_count.summary.headline.value, Some(1));
        assert_eq!(habit_count.summary.total.as_ref().unwrap().value, Some(1));

        // Custom field: frontmatter on Oct 8, inline fallback on Oct 9,
        // absent on Oct 10 and today.
        let energy = metric(&report, "energy");
        assert_eq!(energy.since, Some(day(8)));
        expect_point(
            &energy.points[0],
            day(8),
            Some(3),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &energy.points[1],
            day(9),
            Some(5),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &energy.points[2],
            day(10),
            None,
            DataQuality::Missing,
            false,
        );
        assert_eq!(
            energy.points[2].reason.as_deref(),
            Some("field \"energy\" is absent")
        );
        expect_point(
            &energy.points[3],
            day(11),
            None,
            DataQuality::Missing,
            true,
        );
        assert_eq!(energy.summary.headline.value, Some(4));
        assert_eq!(energy.summary.total.as_ref().unwrap().value, Some(8));

        // Custom journal across all tiers: the pre-only Oct 8 counts here.
        let all = metric(&report, "all_reviewed");
        assert_eq!(all.source, "recorded");
        expect_point(&all.points[0], day(8), Some(1), DataQuality::Ok, false);
        assert_eq!(all.points[0].parts["tier"]["pre"], 1);
        expect_point(&all.points[1], day(9), None, DataQuality::Missing, false);
        expect_point(&all.points[2], day(10), Some(2), DataQuality::Ok, false);
        expect_point(&all.points[3], day(11), Some(1), DataQuality::Ok, true);
        assert_eq!(all.summary.headline.value, Some(2));
        assert_eq!(all.warnings.len(), 1);
        assert_eq!(all.warnings[0].code, "journal_gap");

        // Custom formula over two task metrics with signed display.
        let flow = metric(&report, "done_and_dropped");
        assert_eq!(flow.unit, "signed");
        assert_eq!(flow.since, Some(day(10)));
        let parameters = flow.parameters.as_ref().unwrap();
        assert_eq!(parameters.expr.as_deref(), Some("done + dropped"));
        assert_eq!(parameters.dependencies, ["done", "dropped"]);
        expect_point(&flow.points[2], day(10), Some(6), DataQuality::Ok, false);
        expect_point(&flow.points[3], day(11), Some(1), DataQuality::Ok, true);
        assert_eq!(flow.points[2].display, "+6");
        assert_eq!(flow.summary.headline.value, Some(6));

        // Health: the malformed created date counts once; a future
        // scheduled date is valid planning data. The undated open task
        // counts once as missing-created; two journal hosts (apollo plus
        // hermes with an earlier duplicate and one malformed line).
        assert_eq!(report.health.tasks.invalid_dates, 1);
        assert_eq!(report.health.tasks.closed_without_date, 0);
        assert_eq!(report.health.tasks.missing_created, Some(1));
        assert_eq!(report.health.journal.since, Some(day(8)));
        assert_eq!(report.health.journal.malformed_lines, 1);
        assert_eq!(report.health.journal.hosts.len(), 2);
        let host = &report.health.journal.hosts[0];
        assert_eq!(host.host, "apollo");
        assert_eq!(host.events, 5);
        assert_eq!(host.last_ts, "2026-10-11T09:00:00-04:00");
        assert_eq!(host.last_day, day(11));
        let hermes = &report.health.journal.hosts[1];
        assert_eq!(hermes.host, "hermes");
        assert_eq!(hermes.events, 1);
        assert_eq!(hermes.last_ts, "2026-10-10T08:00:00-04:00");
        assert_eq!(hermes.last_day, day(10));
    }

    #[test]
    fn golden_typical_objects_are_hand_derived() {
        let report = golden_report();
        let worked = metric(&report, "worked");
        // Oct 8 predates Pomodoro tracking: its Oct 1–7 window is empty.
        let tip = worked.points[0].typical.as_ref().unwrap();
        assert_eq!(tip.start, day(1));
        assert_eq!(tip.end, day(7));
        assert_eq!(tip.days, 0);
        assert_eq!(tip.value, None);
        assert_eq!(tip.display, "–");
        assert_eq!(tip.relation, None);
        assert_eq!(tip.words, "no typical day yet");
        assert_eq!(
            tip.detail,
            "No typical day yet: 0 of the previous 7 days have data (needs 4)."
        );
        // Oct 9 (32m): its Oct 2–8 window holds only untracked days.
        let tip = worked.points[1].typical.as_ref().unwrap();
        assert_eq!(tip.start, day(2));
        assert_eq!(tip.end, day(8));
        assert_eq!(tip.days, 0);
        assert_eq!(tip.value, None);
        // Oct 10 (0m): its Oct 3–9 window holds one usable day (Oct 9).
        let tip = worked.points[2].typical.as_ref().unwrap();
        assert_eq!(tip.start, day(3));
        assert_eq!(tip.end, day(9));
        assert_eq!(tip.days, 1);
        assert_eq!(tip.value, None);
        assert_eq!(tip.relation, None);
        assert_eq!(
            tip.detail,
            "No typical day yet: 1 of the previous 7 days has data (needs 4)."
        );
        // Today, Oct 11 (10m, provisional): two usable days, still short.
        let tip = worked.points[3].typical.as_ref().unwrap();
        assert_eq!(tip.start, day(4));
        assert_eq!(tip.end, day(10));
        assert_eq!(tip.days, 2);
        assert_eq!(tip.value, None);
        assert_eq!(tip.relation, None);

        let done = metric(&report, "done");
        let tip = done.points[0].typical.as_ref().unwrap();
        assert_eq!(tip.days, 0);
        assert_eq!(tip.value, None);
        let tip = done.points[1].typical.as_ref().unwrap();
        assert_eq!(tip.start, day(2));
        assert_eq!(tip.end, day(8));
        assert_eq!(tip.days, 0);
        assert_eq!(tip.value, None);
        // Oct 10 (4): its Oct 3–9 window holds Oct 9 (5) only.
        let tip = done.points[2].typical.as_ref().unwrap();
        assert_eq!(tip.start, day(3));
        assert_eq!(tip.end, day(9));
        assert_eq!(tip.days, 1);
        assert_eq!(tip.value, None);
        assert_eq!(
            tip.detail,
            "No typical day yet: 1 of the previous 7 days has data (needs 4)."
        );
        // Today, Oct 11 (1, provisional): Oct 9 (5) and Oct 10 (4).
        let tip = done.points[3].typical.as_ref().unwrap();
        assert_eq!(tip.start, day(4));
        assert_eq!(tip.end, day(10));
        assert_eq!(tip.days, 2);
        assert_eq!(tip.value, None);
        assert_eq!(tip.relation, None);

        // Bool, ratio, and signed metrics never carry a typical day, even
        // with comparison points present.
        for id in ["review_closed", "habits", "keep_rate", "net"] {
            let points = &metric(&report, id).points;
            assert!(
                points.iter().all(|point| point.typical.is_none()),
                "{id} points carry no typical day"
            );
        }
        let created = metric(&report, "created");
        assert!(
            created
                .prior_points
                .iter()
                .all(|point| point.typical.is_none()),
            "prior points never carry a typical day"
        );
    }

    #[test]
    fn report_serializes_as_schema_one_with_optional_fields() {
        let report = golden_report();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["today"], "2026-10-11");
        // Explicit ranges omit the preset key; the prior window is present.
        assert!(json["range"].get("preset").is_none());
        assert_eq!(json["range"]["start"], "2026-10-08");
        assert_eq!(json["range"]["end"], "2026-10-10");
        assert_eq!(json["range"]["days"], 3);
        assert_eq!(json["range"]["includes_today"], true);
        assert_eq!(json["range"]["group"], "day");
        assert_eq!(json["range"]["prior"]["start"], "2026-10-05");

        let worked = &json["metrics"][0];
        assert_eq!(worked["id"], "worked");
        assert_eq!(worked["label"], "Worked");
        assert_eq!(worked["kind"], "pomodoro");
        assert_eq!(worked["unit"], "minutes");
        assert_eq!(worked["direction"], "up");
        assert_eq!(worked["color"], "orange");
        assert_eq!(worked["source"], "derived");
        assert_eq!(worked["origin"], "builtin");
        assert_eq!(worked["since"], "2026-10-09");
        assert_eq!(worked["parameters"]["measure"], "minutes");
        assert_eq!(worked["parameters"]["clock"], "worked");
        assert_eq!(json["clock"]["value"], "worked");
        assert_eq!(json["clock"]["label"], "Worked");
        assert_eq!(json["clock"]["since"], "2026-10-09");
        assert_eq!(json["clock"]["options"][0]["value"], "worked");
        assert_eq!(json["clock"]["options"][0]["minutes"], 42);
        assert_eq!(json["clock"]["options"][0]["display"], "42m");
        assert_eq!(json["clock"]["options"][1]["value"], "off-clock");
        assert_eq!(json["clock"]["options"][1]["minutes"], 60);
        assert_eq!(json["clock"]["options"][1]["display"], "1h 00m");
        assert_eq!(json["clock"]["options"][2]["value"], "total");
        assert_eq!(json["clock"]["options"][2]["minutes"], 102);
        assert_eq!(json["clock"]["options"][2]["display"], "1h 42m");
        assert_eq!(worked["points"][1]["off_clock"], 60);
        assert!(worked["points"][2].get("off_clock").is_none());
        assert!(json["metrics"][1]["parameters"].get("clock").is_none());
        // Empty parts maps are omitted; nulls serialize as null.
        assert!(worked["points"][0].get("parts").is_none());
        assert_eq!(worked["points"][0]["value"], serde_json::Value::Null);
        assert_eq!(worked["points"][0]["quality"], "not_tracked");
        assert_eq!(worked["points"][1]["parts"]["theme"]["PLAN"], 29);
        assert_eq!(
            worked["points"][3]["rolling_median"],
            serde_json::Value::Null
        );
        assert_eq!(worked["points"][3]["provisional"], true);
        assert_eq!(worked["chart"]["group"], "day");
        assert_eq!(worked["chart"]["p25"], 8);
        assert_eq!(worked["chart"]["p75"], 24);
        assert!(worked.get("daily_points").is_none());
        assert_eq!(
            worked["points"][1]["parts_display"]["theme"]["PLAN"],
            "29m"
        );

        // Ratio points carry pooled num/den; bool points read as met days.
        let habits = &json["metrics"][7];
        assert_eq!(habits["id"], "habits");
        assert_eq!(habits["points"][2]["num"], 1);
        assert_eq!(habits["points"][2]["den"], 2);
        assert_eq!(habits["points"][2]["value"], 5000);
        assert_eq!(habits["points"][1]["checklist_state"], "missed");
        assert_eq!(habits["chores"][0]["key"], "journal");
        assert_eq!(habits["chores"][0]["display"], "0 of 2 days");
        assert_eq!(habits["chores"][1]["key"], "meditate");
        assert_eq!(habits["chores"][1]["display"], "1 of 2 days");
        let closed = &json["metrics"][5];
        assert_eq!(closed["points"][1]["display"], "met");
        assert_eq!(closed["points"][1]["checklist_state"], "met");
        // Bool, ratio, and signed points omit the key; the minutes point
        // carries the short-history object with JSON nulls, not omissions.
        for index in 0..4 {
            assert!(closed["points"][index].get("typical").is_none());
            assert!(habits["points"][index].get("typical").is_none());
        }
        let net = &json["metrics"][4];
        assert_eq!(net["unit"], "signed");
        assert!(net["points"][2].get("typical").is_none());
        assert_eq!(worked["points"][2]["typical"]["days"], 1);
        assert_eq!(
            worked["points"][2]["typical"]["value"],
            serde_json::Value::Null
        );
        assert_eq!(worked["points"][2]["typical"]["display"], "–");
        assert_eq!(
            worked["points"][2]["typical"]["relation"],
            serde_json::Value::Null
        );
        assert_eq!(worked["points"][2]["typical"]["start"], "2026-10-03");
        assert_eq!(worked["points"][2]["typical"]["end"], "2026-10-09");

        // Health keeps the schema counters; the gap warning names its day.
        assert_eq!(json["health"]["tasks"]["invalid_dates"], 1);
        assert_eq!(json["health"]["tasks"]["missing_created"], 1);
        assert_eq!(json["health"]["journal"]["malformed_lines"], 1);
        assert_eq!(json["health"]["journal"]["hosts"][0]["events"], 5);
        assert_eq!(json["health"]["journal"]["hosts"][1]["host"], "hermes");
        assert_eq!(json["metrics"][1]["warnings"][0]["code"], "journal_gap");
        assert_eq!(json["metrics"][1]["warnings"][0]["date"], "2026-10-09");
        assert_eq!(json["warnings"][0]["code"], "journal_gap");
    }

    #[test]
    fn clock_lenses_pin_formulas_adoption_and_grouped_off_clock() {
        let dir = TempDir::new().unwrap();
        let config = write_config(
            &dir,
            "
stats:
  hide: [hidden_worked]
  metrics:
    family_time:
      label: Family time
      kind: pomodoro
      clock: off-clock
    hidden_worked:
      label: Hidden worked
      kind: pomodoro
    inner:
      label: Inner
      kind: formula
      expr: worked
      unit: minutes
    outer:
      label: Outer
      kind: formula
      expr: inner + hidden_worked
      unit: minutes
",
        );
        let mut request = golden_request(fixture_vault(), config.clone());
        request.selection = MetricSelection::Explicit(
            [
                "worked",
                "sessions",
                "family_time",
                "inner",
                "outer",
                "done",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        );
        request.clock = StatsClock::OffClock;
        let off = compute(&request).expect("off-clock compute");
        assert_eq!(off.clock.value, "off-clock");
        assert_eq!(off.clock.label, "Off-clock");
        assert_eq!(off.clock.since, Some(day(9)));
        assert_eq!(off.clock.options[0].minutes, 42);
        assert_eq!(off.clock.options[1].minutes, 60);
        assert_eq!(off.clock.options[2].minutes, 102);

        let worked = metric(&off, "worked");
        assert_eq!(worked.label, "Off-clock");
        assert_eq!(worked.direction, "neutral");
        assert_eq!(
            worked.parameters.as_ref().unwrap().clock.as_deref(),
            Some("off-clock")
        );
        expect_point(
            &worked.points[0],
            day(8),
            None,
            DataQuality::NotTracked,
            false,
        );
        expect_point(
            &worked.points[1],
            day(9),
            Some(60),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &worked.points[2],
            day(10),
            Some(0),
            DataQuality::Ok,
            false,
        );
        expect_point(
            &worked.points[3],
            day(11),
            Some(0),
            DataQuality::Ok,
            true,
        );
        assert_eq!(worked.points[1].off_clock, None);
        assert!(worked.parts_summary["theme"]
            .iter()
            .all(|part| part.off_clock.is_none()));

        let sessions = metric(&off, "sessions");
        assert_eq!(sessions.label, "Off-clock sessions");
        expect_point(
            &sessions.points[1],
            day(9),
            Some(1),
            DataQuality::Ok,
            false,
        );
        // The Off-clock lens shows only the bare MEET row, with real span
        // minutes and both off-clock markers set.
        assert_eq!(off.sessions.len(), 1);
        assert_eq!(off.sessions[0].name.as_deref(), Some("MEET"));
        assert_eq!(off.sessions[0].minutes, Some(60));
        assert_eq!(off.sessions[0].quality, DataQuality::Ok);
        assert!(off.sessions[0].off_clock);
        assert!(off.sessions[0].homes.untracked);
        assert!(off.sessions[0].homes.off_clock);

        let family = metric(&off, "family_time");
        assert_eq!(family.label, "Family time · off-clock");
        expect_point(
            &family.points[1],
            day(9),
            Some(60),
            DataQuality::Ok,
            false,
        );

        let inner = metric(&off, "inner");
        assert_eq!(inner.label, "Inner");
        assert_eq!(inner.direction, "up");
        expect_point(
            &inner.points[1],
            day(9),
            Some(32),
            DataQuality::Ok,
            false,
        );
        assert_eq!(inner.points[1].off_clock, None);
        assert!(inner.parameters.as_ref().unwrap().clock.is_none());

        let outer = metric(&off, "outer");
        expect_point(
            &outer.points[1],
            day(9),
            Some(64),
            DataQuality::Ok,
            false,
        );

        let done = metric(&off, "done");
        assert_eq!(done.label, "Done");
        expect_point(&done.points[1], day(9), Some(5), DataQuality::Ok, false);

        request.clock = StatsClock::Total;
        let total = compute(&request).expect("total compute");
        let worked = metric(&total, "worked");
        assert_eq!(worked.label, "Total");
        assert_eq!(worked.direction, "neutral");
        expect_point(
            &worked.points[1],
            day(9),
            Some(92),
            DataQuality::Ok,
            false,
        );
        assert_eq!(worked.points[1].off_clock, Some(60));
        let meet = worked.parts_summary["theme"]
            .iter()
            .find(|part| part.key == "MEET")
            .expect("MEET part");
        assert_eq!(meet.value, 60);
        assert_eq!(meet.off_clock, Some(60));
        let plan = worked.parts_summary["theme"]
            .iter()
            .find(|part| part.key == "PLAN")
            .expect("PLAN part");
        assert_eq!(plan.value, 29);
        assert_eq!(plan.off_clock, None);
        expect_point(
            &metric(&total, "sessions").points[1],
            day(9),
            Some(3),
            DataQuality::Ok,
            false,
        );
        // Total shows every row: the bare MEET row carries real span
        // minutes and the off-clock flag, worked rows omit the flag.
        assert_eq!(total.sessions.len(), 4);
        let meet = total
            .sessions
            .iter()
            .find(|session| session.name.as_deref() == Some("MEET"))
            .expect("MEET session");
        assert_eq!(meet.minutes, Some(60));
        assert!(meet.off_clock);
        assert!(meet.homes.untracked);
        assert!(meet.homes.off_clock);
        assert!(total
            .sessions
            .iter()
            .filter(|session| session.name.as_deref() != Some("MEET"))
            .all(|session| !session.off_clock));
        let meet_json = serde_json::to_value(meet).expect("session JSON");
        assert_eq!(meet_json["off_clock"], true);
        assert_eq!(meet_json["homes"]["off_clock"], true);
        assert_eq!(meet_json["homes"]["untracked"], true);
        let plan_json =
            serde_json::to_value(&total.sessions[0]).expect("session JSON");
        assert!(plan_json.get("off_clock").is_none());
        assert!(plan_json["homes"].get("off_clock").is_none());
        assert!(plan_json["homes"].get("untracked").is_none());
        expect_point(
            &metric(&total, "inner").points[1],
            day(9),
            Some(32),
            DataQuality::Ok,
            false,
        );

        request.clock = StatsClock::Worked;
        request.grouping = Grouping::Month;
        let pinned = compute(&request).expect("worked pin compute");
        let worked = metric(&pinned, "worked");
        assert_eq!(worked.label, "Worked");
        assert_eq!(worked.direction, "up");
        assert_eq!(worked.points[0].value, Some(32));
        assert_eq!(worked.points[0].off_clock, Some(60));
        assert_eq!(worked.daily_points[1].off_clock, Some(60));
        let family = metric(&pinned, "family_time");
        assert_eq!(
            family.parameters.as_ref().unwrap().clock.as_deref(),
            Some("off-clock")
        );
        assert_eq!(family.direction, "neutral");
        assert_eq!(family.points[0].value, Some(60));
        assert_eq!(family.points[0].off_clock, None);
        // The Worked lens keeps the three worked rows; diagnostics still
        // describe the whole window.
        assert_eq!(pinned.sessions.len(), 3);
        assert_eq!(pinned.sessions[2].name.as_deref(), Some("SASE"));
        assert_eq!(pinned.sessions[2].minutes, Some(10));
        assert!(!pinned.sessions[2].off_clock);
        assert!(!pinned.sessions[2].homes.untracked);
        assert_eq!(pinned.health.homes.off_clock_sessions, 1);
        assert_eq!(pinned.health.homes.untracked_sessions, 1);
        assert_eq!(pinned.health.homes.unstamped_sessions, 1);
        assert_eq!(pinned.health.homes.clock_conflicts, 0);
        assert_eq!(pinned.health.homes.clock_drift, 0);
        assert!(pinned.health.homes.overlaps.is_empty());
        let warnings: Vec<&str> = pinned
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect();
        assert!(warnings.contains(&"clock_unstamped"));
        assert!(!warnings.contains(&"clock_conflict"));
        assert!(!warnings.contains(&"clock_drift"));
        assert!(!warnings.contains(&"clock_overlap"));
    }

    #[test]
    fn session_diagnostics_filter_sessions_and_report_window_counts() {
        let workspace = TempDir::new().unwrap();
        let vault = workspace.path().join("vault");
        copy_tree(&fixture_vault(), &vault);
        for dir in ["a", "b"] {
            let note = vault.join(dir).join("dup.md");
            std::fs::create_dir_all(note.parent().unwrap()).unwrap();
            std::fs::write(
                &note,
                "- [x] #task Dup task [created:: 2026-10-09] [completion:: 2026-10-09] ^t\n",
            )
            .unwrap();
        }
        let tasks = vault.join("tasks.md");
        let mut task_text = std::fs::read_to_string(&tasks).unwrap();
        task_text.push_str(
            "- [ ] #task Off-clock errand #offclock [created:: 2026-10-09] ^off1\n",
        );
        task_text.push_str(
            "- [x] #task On-clock chore [created:: 2026-10-09] [completion:: 2026-10-09] ^on1\n",
        );
        std::fs::write(&tasks, task_text).unwrap();
        let daily = vault.join("2026/20261009.md");
        let mut daily_text = std::fs::read_to_string(&daily).unwrap();
        daily_text.push_str(
            "- [x] (**10:00 - 10:30** [t:: 20m] [off:: 45m]) — CONFLICT\n",
        );
        daily_text.push_str("- [x] (**08:10 - 08:40** [t:: 20m]) — OVER\n");
        daily_text.push_str(
            "- [x] (**14:00 - 14:25** [t:: 25m]) — LATE\n  - [[TASKS#^off1]]\n",
        );
        daily_text.push_str(
            "- [x] (**15:00 - 15:20** [off:: 20m]) — EARLY\n  - [[tasks#^on1]]\n",
        );
        daily_text.push_str(
            "- [x] (**16:00 - 16:10** [t:: 10m]) — LOST\n  - [[missing#^x1]]\n",
        );
        daily_text.push_str(
            "- [x] (**16:20 - 16:30** [t:: 10m]) — TWICE\n  - [[dup#^t]]\n",
        );
        daily_text.push_str(
            "- [x] (**16:40 - 16:50** [t:: 10m]) — NOBLOCK\n  - [[tasks#^nope]]\n",
        );
        daily_text.push_str("- [x] (**18:00 - 18:30**) — SOLO\n");
        daily_text.push_str("- [x] () — GHOST\n");
        std::fs::write(&daily, daily_text).unwrap();
        let config = write_config(&workspace, "stats:\n");

        let mut request =
            StatsRequest::at(vault.clone(), config.clone(), fixed_now());
        request.selection =
            MetricSelection::Explicit(vec!["worked".to_string()]);
        request.range = Some("2026-10-08..2026-10-11".to_string());
        request.grouping = Grouping::Day;
        let report = compute(&request).expect("worked diagnostics");

        // Keyed rows count their keys; the conflict keeps `[t::]`, and the
        // bare rows count their spans off the clock.
        let worked = metric(&report, "worked");
        assert_eq!(worked.points[1].value, Some(127));
        assert_eq!(worked.points[1].off_clock, Some(110));

        // The Worked lens hides the three off-clock rows; the completed
        // placeholder never appears in any lens.
        assert_eq!(report.sessions.len(), 9);
        assert!(report.sessions.iter().all(|session| !session.off_clock));
        assert!(report
            .sessions
            .iter()
            .all(|session| session.name.as_deref() != Some("GHOST")));

        // Diagnostics describe the whole window, not the lens.
        let health = &report.health.homes;
        assert_eq!(health.off_clock_sessions, 3);
        assert_eq!(health.untracked_sessions, 3);
        assert_eq!(health.unstamped_sessions, 2);
        assert_eq!(health.clock_conflicts, 1);
        assert_eq!(health.clock_drift, 2);
        assert_eq!(health.overlap_minutes, 15);
        assert_eq!(health.overlaps.len(), 1);
        assert_eq!(health.overlaps[0].minutes, 15);
        assert_eq!(health.overlaps[0].lines, vec![9, 14]);
        // Unlinked rows carry real minutes now: the bare SOLO span counts.
        // The completed GHOST placeholder stays unlinked but out of every
        // diagnostic and lens.
        assert_eq!(health.unlinked_minutes, 112);
        assert_eq!(health.unlinked_sessions, 7);
        let codes: Vec<&str> = report
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect();
        for code in [
            "clock_conflict",
            "clock_drift",
            "clock_unstamped",
            "clock_overlap",
        ] {
            assert!(codes.contains(&code), "missing {code}: {codes:?}");
        }

        // The Off-clock lens shows only MEET, EARLY, and SOLO.
        let mut off_request = request.clone();
        off_request.clock = StatsClock::OffClock;
        let off = compute(&off_request).expect("off-clock diagnostics");
        assert_eq!(off.sessions.len(), 3);
        assert!(off.sessions.iter().all(|session| session.off_clock));
        assert_eq!(off.health.homes.clock_drift, 2);
        assert_eq!(off.health.homes.overlap_minutes, 15);

        // Total shows every timed row with real minutes, still no GHOST.
        let mut total_request = request.clone();
        total_request.clock = StatsClock::Total;
        let total = compute(&total_request).expect("total diagnostics");
        assert_eq!(total.sessions.len(), 12);
        let solo = total
            .sessions
            .iter()
            .find(|session| session.name.as_deref() == Some("SOLO"))
            .expect("SOLO session");
        assert_eq!(solo.minutes, Some(30));
        assert!(solo.off_clock);
        let conflict = total
            .sessions
            .iter()
            .find(|session| session.name.as_deref() == Some("CONFLICT"))
            .expect("CONFLICT session");
        assert_eq!(conflict.minutes, Some(20));
        assert!(!conflict.off_clock);
        let meet = total
            .sessions
            .iter()
            .find(|session| session.name.as_deref() == Some("MEET"))
            .expect("MEET session");
        let meet_json = serde_json::to_value(meet).expect("session JSON");
        assert_eq!(meet_json["minutes"], 60);
        assert_eq!(meet_json["off_clock"], true);
        assert_eq!(meet_json["homes"]["off_clock"], true);
        assert_eq!(meet_json["homes"]["untracked"], true);
        let part_minutes: u32 = meet_json["homes"]["parts"]
            .as_array()
            .expect("home parts")
            .iter()
            .map(|part| part["minutes"].as_u64().unwrap_or(0) as u32)
            .sum();
        assert_eq!(part_minutes, 60);

        // Empty diagnostics stay omitted from the JSON contract.
        let mut golden_request =
            golden_request(fixture_vault(), config.clone());
        golden_request.selection =
            MetricSelection::Explicit(vec!["worked".to_string()]);
        let golden =
            compute(&golden_request).expect("pristine fixture computes");
        let health_json =
            serde_json::to_value(&golden.health.homes).expect("health JSON");
        assert!(health_json.get("clock_conflicts").is_none());
        assert!(health_json.get("clock_drift").is_none());
        assert!(health_json.get("overlap_minutes").is_none());
        assert!(health_json.get("overlaps").is_none());
        assert_eq!(health_json["off_clock_sessions"], 1);
        assert_eq!(health_json["untracked_sessions"], 1);
        assert_eq!(health_json["unstamped_sessions"], 1);
    }

    #[test]
    fn clock_diagnostics_repair_empty_lineups_parsed_tasks_and_cross_note_overlap(
    ) {
        let workspace = TempDir::new().unwrap();
        let vault = workspace.path().join("vault");
        copy_tree(&fixture_vault(), &vault);
        for dir in ["a", "b"] {
            let note = vault.join(dir).join("dup.md");
            std::fs::create_dir_all(note.parent().unwrap()).unwrap();
            std::fs::write(
                &note,
                "- [x] #task Dup task [created:: 2026-10-09] [completion:: 2026-10-09] ^t\n",
            )
            .unwrap();
        }
        std::fs::write(
            vault.join("embed.md"),
            "- [ ] checkbox only #offclock ^plain\n",
        )
        .unwrap();
        std::fs::create_dir_all(vault.join("done")).unwrap();
        std::fs::write(
            vault.join("done/old.md"),
            "- [x] #task Archived #offclock [created:: 2026-10-01] [completion:: 2026-10-09] ^arch1\n",
        )
        .unwrap();
        let tasks = vault.join("tasks.md");
        let mut task_text = std::fs::read_to_string(&tasks).unwrap();
        task_text = format!(
            concat!(
                "---\n",
                "- [ ] #task Ghost #offclock ^ghost\n",
                "---\n",
                "{task_text}",
                "- [ ] #task Off-clock errand #offclock [created:: 2026-10-09] ^off1\n",
                "- [x] #task On-clock chore [created:: 2026-10-09] [completion:: 2026-10-09] ^on1\n",
                "- [ ] #task Walk #off-clock [created:: 2026-10-09] ^walk\n",
                "- [ ] #task Cook #untracked [created:: 2026-10-09] ^cook\n",
                "- [ ] #task One ^dup\n",
                "- [ ] #task Two ^dup\n",
                "```\n",
                "- [ ] #task Fenced ghost #offclock ^fence1\n",
                "```\n",
                "- [ ] #task Real fence target [created:: 2026-10-09] ^fence1\n",
            ),
            task_text = task_text
        );
        std::fs::write(&tasks, task_text).unwrap();

        let oct8 = vault.join("2026/20261008.md");
        let mut oct8_text = std::fs::read_to_string(&oct8).unwrap();
        oct8_text.push_str(
            "\n## Pomodoros\n- [x] (**23:45 - 00:30** [t:: 45m]) — PREV\n",
        );
        std::fs::write(&oct8, oct8_text).unwrap();

        let oct9 = vault.join("2026/20261009.md");
        let mut oct9_text = std::fs::read_to_string(&oct9).unwrap();
        oct9_text.push_str(concat!(
            "- [x] (**14:00 - 14:25** [t:: 25m]) — LATE\n  - [[TASKS#^off1]]\n",
            "- [x] (**15:00 - 15:20** [off:: 20m]) — EARLY\n  - [[tasks#^on1]]\n",
            "- [x] (**16:00 - 16:10** [off:: 10m]) — LOST\n  - [[missing#^x1]]\n",
            "- [x] (**16:20 - 16:30** [off:: 10m]) — TWICE\n  - [[dup#^t]]\n",
            "- [x] (**16:40 - 16:50** [off:: 10m]) — NOBLOCK\n  - [[tasks#^nope]]\n",
            "- [x] (**17:00 - 17:10** [off:: 10m]) — DUPID\n  - [[tasks#^dup]]\n",
            "- [x] (**17:20 - 17:30** [off:: 10m]) — GHOST\n  - [[tasks#^ghost]]\n",
            "- [x] (**17:40 - 17:50** [off:: 10m]) — FENCE\n  - [[tasks#^fence1]]\n",
            "- [x] (**18:00 - 18:20** [off:: 20m]) — ALIAS\n  - [[tasks#^walk]]\n  - [[tasks#^cook]]\n",
            "- [x] (**18:30 - 18:40** [off:: 10m]) — ARCH\n  - [[old#^arch1]]\n",
            "- [x] (**18:50 - 19:00** [off:: 10m]) — EMBED\n  - ![[embed#^plain]]\n",
            "- [x] (**19:10 - 19:35** [off:: 25m]) — EMPTY\n",
            "- [x] (**19:40 - 20:00** [t:: 20m]) — EMPTYWORK\n",
            "- [x] (**20:10 - 20:30** [off:: 20m]) — DEFER\n  - LINKS [[tasks#^off1]]\n",
            "- [x] (**21:00 - 21:30** [off:: 30m]) — NIGHT\n  - [[#^night]]\n",
            "- [x] (**23:30 - 00:30** [t:: 60m]) — NIGHTSHIFT\n",
            "\n## Events\n- [ ] #task **2100-2130** Night #untracked ^night\n",
        ));
        std::fs::write(&oct9, oct9_text).unwrap();

        let oct10 = vault.join("2026/20261010.md");
        let mut oct10_text = std::fs::read_to_string(&oct10).unwrap();
        oct10_text.push_str("- [x] (**00:15 - 00:45** [t:: 30m]) — DAWN\n");
        std::fs::write(&oct10, oct10_text).unwrap();

        let config = write_config(&workspace, "stats:\n");
        let vault_hash = tree_hash(&vault);
        let config_hash = {
            let mut hasher = DefaultHasher::new();
            std::fs::read(&config).unwrap().hash(&mut hasher);
            hasher.finish()
        };

        let mut request =
            StatsRequest::at(vault.clone(), config.clone(), fixed_now());
        request.selection =
            MetricSelection::Explicit(vec!["worked".to_string()]);
        request.range = Some("2026-10-09..2026-10-10".to_string());
        request.grouping = Grouping::Day;
        let worked = compute(&request).expect("worked diagnostics");
        let mut off_request = request.clone();
        off_request.clock = StatsClock::OffClock;
        let off = compute(&off_request).expect("off-clock diagnostics");
        let mut total_request = request.clone();
        total_request.clock = StatsClock::Total;
        let total = compute(&total_request).expect("total diagnostics");

        for report in [&worked, &off, &total] {
            let health = &report.health.homes;
            // LATE, EARLY, FENCE (resolved on-clock), EMPTY, DEFER.
            assert_eq!(health.clock_drift, 5, "{:?}", report.clock.value);
            // PREV on Oct 8 is outside the ledger-date window.
            assert_eq!(health.overlap_minutes, 15, "{:?}", report.clock.value);
            assert_eq!(health.overlaps.len(), 1);
            assert_eq!(health.overlaps[0].date, day(9));
            assert_eq!(health.overlaps[0].minutes, 15);
            assert_eq!(
                health.overlaps[0].paths,
                Some(vec![
                    "2026/20261009.md".to_string(),
                    "2026/20261010.md".to_string(),
                ])
            );
            assert_eq!(health.overlaps[0].lines.len(), 2);
        }

        let overlap_json =
            serde_json::to_value(&worked.health.homes.overlaps[0])
                .expect("overlap JSON");
        assert_eq!(overlap_json["date"], "2026-10-09");
        assert_eq!(
            overlap_json["paths"],
            serde_json::json!(["2026/20261009.md", "2026/20261010.md"])
        );

        // Overlap is diagnostic-only: clock totals still sum recorded keys.
        let options = &worked.clock.options;
        assert_eq!(options[0].value, "worked");
        assert_eq!(options[1].value, "off-clock");
        assert_eq!(options[2].minutes, options[0].minutes + options[1].minutes);
        assert!(options[0].minutes > 0);
        assert!(options[1].minutes > 0);
        assert_eq!(options[0].minutes, off.clock.options[0].minutes);
        assert_eq!(options[1].minutes, total.clock.options[1].minutes);

        assert_eq!(tree_hash(&vault), vault_hash);
        let config_after = {
            let mut hasher = DefaultHasher::new();
            std::fs::read(&config).unwrap().hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(config_after, config_hash);
    }

    #[test]
    fn unknown_metric_is_a_request_error_with_schema_one_envelope() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let mut request = golden_request(fixture_vault(), config);
        request.selection = MetricSelection::Explicit(vec!["nope".to_string()]);
        let error = compute(&request).expect_err("unknown metric fails");
        assert_eq!(error.code(), "unknown_metric");
        assert_eq!(error.exit_code(), 2);
        let json = serde_json::to_value(error.envelope()).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["error"]["code"], "unknown_metric");
    }

    #[test]
    fn malformed_range_config_and_request_fail_as_usage_or_config_errors() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, "stats:\n  range: 0d\n");
        let request = StatsRequest::at(fixture_vault(), config, fixed_now());
        let error = compute(&request).expect_err("bad range fails");
        assert_eq!(error.code(), "config");
        assert_eq!(error.exit_code(), 2);

        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let mut request = golden_request(fixture_vault(), config);
        request.range = Some("bogus".to_string());
        let error = compute(&request).expect_err("bad request range fails");
        assert_eq!(error.code(), "usage");
        assert!(
            error.message().contains("bogus"),
            "usage error names the range: {}",
            error.message()
        );
    }

    #[test]
    fn missing_vault_is_io_and_emoji_vault_is_format() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let request = StatsRequest::at(
            dir.path().join("no-such-vault"),
            config,
            fixed_now(),
        );
        let error = compute(&request).expect_err("missing vault fails");
        assert_eq!(error.code(), "io");
        assert_eq!(error.exit_code(), 1);

        // Without the Tasks data.json the vault defaults to Emoji format,
        // which the engine refuses to summarize.
        let vault = TempDir::new().unwrap();
        std::fs::write(vault.path().join("note.md"), "- [ ] #task x\n")
            .unwrap();
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let request =
            StatsRequest::at(vault.path().to_path_buf(), config, fixed_now());
        let error = compute(&request).expect_err("emoji vault fails");
        assert_eq!(error.code(), "format");
        assert_eq!(error.exit_code(), 2);
    }

    fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push((
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        std::fs::read(&path).unwrap(),
                    ));
                }
            }
        }
        files.sort();
        files
    }

    fn copy_tree(src: &Path, dst: &Path) {
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

    fn tree_hash(root: &Path) -> u64 {
        let mut hasher = DefaultHasher::new();
        for (path, bytes) in snapshot_tree(root) {
            path.hash(&mut hasher);
            bytes.hash(&mut hasher);
        }
        hasher.finish()
    }

    #[test]
    fn compute_is_deterministic_and_leaves_vault_and_config_untouched() {
        let workspace = TempDir::new().unwrap();
        let vault = workspace.path().join("vault");
        copy_tree(&fixture_vault(), &vault);
        let config = write_config(&workspace, &golden_config());
        let vault_hash = tree_hash(&vault);
        let config_hash = {
            let mut hasher = DefaultHasher::new();
            std::fs::read(&config).unwrap().hash(&mut hasher);
            hasher.finish()
        };
        assert!(
            snapshot_tree(&vault)
                .iter()
                .any(|(path, _)| path.starts_with(".bob")
                    || path.starts_with(".obsidian")),
            "copy includes dot directories"
        );
        let request = golden_request(vault.clone(), config.clone());

        let first = serde_json::to_string(&compute(&request).unwrap()).unwrap();
        let second =
            serde_json::to_string(&compute(&request).unwrap()).unwrap();
        assert_eq!(first, second, "repeated calls agree byte for byte");

        assert_eq!(tree_hash(&vault), vault_hash);
        let mut hasher = DefaultHasher::new();
        std::fs::read(&config).unwrap().hash(&mut hasher);
        assert_eq!(hasher.finish(), config_hash);
    }

    #[test]
    fn recorded_durations_add_up_to_the_reported_total() {
        // Independent oracle: a plain text scan of `[t:: Nm]` markers on Oct
        // 9 must equal the worked total, since every fixture session on that
        // day records its duration and none falls back to a range estimate.
        let note =
            std::fs::read_to_string(fixture_vault().join("2026/20261009.md"))
                .unwrap();
        let marker = regex::Regex::new(r"\[t::\s*(\d+)m\]").unwrap();
        let recorded: i64 = marker
            .captures_iter(&note)
            .map(|captures| captures[1].parse::<i64>().unwrap())
            .sum();
        assert_eq!(recorded, 32);

        let report = golden_report();
        let worked = metric(&report, "worked");
        assert_eq!(worked.points[1].value, Some(recorded));
        assert_eq!(worked.points[1].quality, DataQuality::Ok);
    }

    #[test]
    fn month_grouping_pools_complete_days_and_marks_the_clipped_bucket() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let mut request = golden_request(fixture_vault(), config);
        request.grouping = Grouping::Month;
        request.compare = false;
        let report = compute(&request).expect("grouped compute");
        assert_eq!(
            report.range.group,
            crate::native::stats::report::GroupName::Month
        );

        // Without compare the prior summary stays (it explains the delta)
        // but no prior points are reported.
        let habits = metric(&report, "habits");
        assert_eq!(habits.prior_points.len(), 0);
        assert!(habits.summary.prior.is_some());

        // One clipped October bucket pools the two tracked days (0/2 and
        // 1/2); the untracked Oct 8 makes it partial. Today is a separate
        // provisional point and never enters the pool.
        assert_eq!(habits.points.len(), 2);
        let bucket = &habits.points[0];
        assert_eq!(bucket.date, day(8));
        assert_eq!(bucket.end, day(10));
        assert_eq!(bucket.value, Some(2500));
        assert_eq!(bucket.display, "25%");
        assert_eq!(bucket.num, Some(1));
        assert_eq!(bucket.den, Some(4));
        assert_eq!(bucket.quality, DataQuality::Partial);
        assert_eq!(bucket.partial, Some(true));
        assert!(!bucket.provisional);
        assert!(habits.points[1].provisional);
        assert_eq!(habits.points[1].date, day(11));
        assert_eq!(habits.points[1].rolling_median, None);

        let worked = metric(&report, "worked");
        assert_eq!(worked.points.len(), 2);
        assert_eq!(worked.points[0].value, Some(32));
        assert_eq!(worked.points[0].display, "32m");
        assert_eq!(worked.points[0].parts["theme"]["PLAN"], 29);
        assert!(worked.points[0].checklist_state.is_none());
        assert_eq!(worked.points[0].parts_display["theme"]["PLAN"], "29m");
        assert_eq!(
            worked.chart.group,
            crate::native::stats::report::GroupName::Month
        );
        // One displayed bucket (32) plus provisional today; quartiles of [32].
        assert_eq!(worked.chart.p25, Some(32));
        assert_eq!(worked.chart.p75, Some(32));
        assert_ne!(worked.chart.p25, worked.summary.p25);
        let daily_dates: Vec<_> =
            worked.daily_points.iter().map(|p| p.date).collect();
        assert_eq!(daily_dates, vec![day(8), day(9), day(10), day(11)]);
        assert!(worked.daily_points[3].provisional);
        assert_eq!(
            habits.daily_points[1].checklist_state,
            Some(ChecklistState::Missed)
        );
        assert_eq!(habits.chart.p25, Some(2500));
        assert_eq!(habits.chart.p75, Some(2500));
        assert_eq!(habits.summary.p25, Some(1250));
        assert_eq!(habits.chores[1].display, "1 of 2 days");
    }

    #[test]
    fn week_grouping_adds_daily_points_and_a_week_chart_band() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let mut request = golden_request(fixture_vault(), config);
        request.grouping = Grouping::Week;
        request.compare = false;
        let report = compute(&request).expect("week grouped compute");
        assert_eq!(
            report.range.group,
            crate::native::stats::report::GroupName::Week
        );
        let habits = metric(&report, "habits");
        assert_eq!(
            habits.chart.group,
            crate::native::stats::report::GroupName::Week
        );
        // One clipped week bucket pools 0/2 + 1/2; today stays separate.
        assert_eq!(habits.points.len(), 2);
        assert_eq!(habits.points[0].value, Some(2500));
        assert!(habits.points[0].checklist_state.is_none());
        assert_eq!(habits.chart.p25, Some(2500));
        assert_eq!(habits.chart.p75, Some(2500));
        assert_eq!(habits.summary.p25, Some(1250));
        let daily_dates: Vec<_> =
            habits.daily_points.iter().map(|p| p.date).collect();
        assert_eq!(daily_dates, vec![day(8), day(9), day(10), day(11)]);
        assert_eq!(
            habits.daily_points[0].checklist_state,
            Some(ChecklistState::NotTracked)
        );
        let net = metric(&report, "net");
        assert_eq!(net.chart.p25, Some(-1));
        assert_eq!(net.chart.p75, Some(-1));
        assert!(metric(&report, "worked").chores.is_empty());
    }

    /// Opt-in live-vault parity: with BOB_STATS_REAL_VAULT_PARITY=1 (and
    /// BOB_NOW for a deterministic window) this runs compute against the
    /// real vault and checks independent oracles over the complete days of
    /// a 7-day window. Sessions equal a closed-row count under `## Pomodoros`.
    /// Done/created/dropped equal a line-regex count over every non-excluded
    /// `.md` (including `done/`), using Tasks `data.json` status types,
    /// excluding recurring rows and the daily-note `^gtd` row, and skipping
    /// fenced code. Worked equals recorded `[t::]` minutes on days where
    /// every closed ledger row carries that marker, and is `>=` otherwise.
    /// Prints compute wall time for rollout measurement. Without the opt-in
    /// variable this passes without touching any vault.
    #[test]
    fn real_vault_parity_is_opt_in() {
        if std::env::var_os("BOB_STATS_REAL_VAULT_PARITY").is_none() {
            return;
        }
        let now_text = std::env::var("BOB_NOW").unwrap_or_else(|_| {
            panic!("BOB_STATS_REAL_VAULT_PARITY=1 requires BOB_NOW")
        });
        let now = NaiveDateTime::parse_from_str(&now_text, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| {
                NaiveDate::parse_from_str(
                    &now_text[..10.min(now_text.len())],
                    "%Y-%m-%d",
                )
                .map(|date| date.and_hms_opt(12, 0, 0).unwrap())
            })
            .expect("BOB_NOW must be YYYY-MM-DD[ HH:MM:SS]");
        let vault = std::env::var_os("BOB_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join("bob"))
            })
            .expect("BOB_STATS_REAL_VAULT_PARITY=1 requires BOB_DIR or HOME");
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, "stats:\n");
        let mut request = StatsRequest::at(vault.clone(), config, now);
        request.selection = MetricSelection::Explicit(
            ["worked", "sessions", "done", "created", "dropped"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
        request.range = Some("7d".to_string());
        request.grouping = Grouping::Day;
        request.compare = false;

        let started = std::time::Instant::now();
        let report = compute(&request).expect("real vault computes");
        let elapsed = started.elapsed();
        eprintln!(
            "stats compute over {} took {}ms",
            vault.display(),
            elapsed.as_millis()
        );

        let today = now.date();
        let (mut closed_rows, mut recorded_minutes) = (0i64, 0i64);
        let row = regex::Regex::new(r"^-\s+\[[xX]\]").unwrap();
        let marker = regex::Regex::new(r"\[t::\s*(\d+)m\]").unwrap();
        let heading = regex::Regex::new(r"^#{1,6}\s+Pomodoros\b").unwrap();
        let any_heading = regex::Regex::new(r"^#{1,6}\s").unwrap();
        let mut all_closed_have_t =
            std::collections::BTreeMap::<NaiveDate, bool>::new();
        let mut recorded_by_day =
            std::collections::BTreeMap::<NaiveDate, i64>::new();
        for back in 1..=7 {
            let date = today - chrono::Duration::days(back);
            let note = vault.join(format!(
                "{}/{:04}{:02}{:02}.md",
                date.format("%Y"),
                date.year(),
                date.month(),
                date.day()
            ));
            let Ok(contents) = std::fs::read_to_string(&note) else {
                continue;
            };
            let mut in_section = false;
            let mut in_fence = false;
            let mut day_all_t = true;
            let mut day_recorded = 0i64;
            let mut day_closed = 0i64;
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("```") {
                    in_fence = !in_fence;
                    continue;
                }
                if in_fence {
                    continue;
                }
                if any_heading.is_match(trimmed) {
                    in_section = heading.is_match(trimmed);
                    continue;
                }
                if in_section && row.is_match(line) {
                    closed_rows += 1;
                    day_closed += 1;
                    let minutes = marker
                        .captures_iter(line)
                        .map(|captures| captures[1].parse::<i64>().unwrap_or(0))
                        .sum::<i64>();
                    if minutes == 0 && !marker.is_match(line) {
                        day_all_t = false;
                    }
                    recorded_minutes += minutes;
                    day_recorded += minutes;
                }
            }
            if day_closed > 0 {
                all_closed_have_t.insert(date, day_all_t);
                recorded_by_day.insert(date, day_recorded);
            }
        }

        let sessions = metric(&report, "sessions");
        let worked = metric(&report, "worked");
        let session_total = sessions
            .summary
            .total
            .as_ref()
            .and_then(|total| total.value);
        let worked_total =
            worked.summary.total.as_ref().and_then(|total| total.value);
        assert_eq!(
            session_total,
            Some(closed_rows),
            "sessions total matches the independent closed-row count"
        );
        assert!(
            worked_total.unwrap_or(0) >= recorded_minutes,
            "worked total {worked_total:?} covers recorded {recorded_minutes}m"
        );
        for point in worked.points.iter().filter(|point| !point.provisional) {
            let Some(recorded) = recorded_by_day.get(&point.date) else {
                continue;
            };
            if all_closed_have_t.get(&point.date).copied().unwrap_or(false) {
                assert_eq!(
                    point.value,
                    Some(*recorded),
                    "worked on {} equals recorded [t::] minutes",
                    point.date
                );
            } else {
                assert!(
                    point.value.unwrap_or(0) >= *recorded,
                    "worked on {} covers recorded {recorded}m",
                    point.date
                );
            }
        }

        // Independent task oracle: raw-line counts over complete days.
        // Status types come from the vault's Tasks data.json via serde_json,
        // independently of the engine parser. Recurring rows (`[repeat::` /
        // 🔁) and the daily-note `^gtd` row are excluded; fenced code is
        // skipped. The engine's Dataview parser is the source of truth for
        // exotic inline-field spellings this regex does not see.
        let (status_of, global_filter) = live_task_status_map(&vault);
        let checkbox = regex::Regex::new(r"^-\s+\[(.)\]").unwrap();
        let created_re =
            regex::Regex::new(r"\[created::\s*(\d{4}-\d{2}-\d{2})\]").unwrap();
        let completion_re =
            regex::Regex::new(r"\[completion::\s*(\d{4}-\d{2}-\d{2})\]")
                .unwrap();
        let cancelled_re =
            regex::Regex::new(r"\[cancelled::\s*(\d{4}-\d{2}-\d{2})\]")
                .unwrap();
        let gtd_re = regex::Regex::new(r"\^gtd\s*$").unwrap();
        let mut oracle_done =
            std::collections::BTreeMap::<NaiveDate, i64>::new();
        let mut oracle_created =
            std::collections::BTreeMap::<NaiveDate, i64>::new();
        let mut oracle_dropped =
            std::collections::BTreeMap::<NaiveDate, i64>::new();
        for path in walk_markdown(&vault) {
            let Ok(relative) = path.strip_prefix(&vault) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut in_fence = false;
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("```") {
                    in_fence = !in_fence;
                    continue;
                }
                if in_fence {
                    continue;
                }
                let Some(captures) = checkbox.captures(trimmed) else {
                    continue;
                };
                if !global_filter.is_empty()
                    && !trimmed.contains(&global_filter)
                {
                    continue;
                }
                let symbol = captures[1].chars().next().unwrap_or(' ');
                let status = status_of
                    .get(&symbol)
                    .cloned()
                    .unwrap_or_else(|| "TODO".to_string());
                let recurring =
                    trimmed.contains("[repeat::") || trimmed.contains('🔁');
                let daily_gtd =
                    crate::native::stats::index::canonical_daily_date(
                        &relative,
                    )
                    .is_some()
                        && gtd_re.is_match(trimmed);
                if recurring || daily_gtd {
                    continue;
                }
                if let Some(captures) = created_re.captures(trimmed)
                    && let Ok(date) =
                        NaiveDate::parse_from_str(&captures[1], "%Y-%m-%d")
                {
                    *oracle_created.entry(date).or_default() += 1;
                }
                if status == "DONE"
                    && let Some(captures) = completion_re.captures(trimmed)
                    && let Ok(date) =
                        NaiveDate::parse_from_str(&captures[1], "%Y-%m-%d")
                {
                    *oracle_done.entry(date).or_default() += 1;
                }
                if status == "CANCELLED"
                    && let Some(captures) = cancelled_re.captures(trimmed)
                    && let Ok(date) =
                        NaiveDate::parse_from_str(&captures[1], "%Y-%m-%d")
                {
                    *oracle_dropped.entry(date).or_default() += 1;
                }
            }
        }

        for (id, oracle) in [
            ("done", &oracle_done),
            ("created", &oracle_created),
            ("dropped", &oracle_dropped),
        ] {
            let report_metric = metric(&report, id);
            for point in report_metric
                .points
                .iter()
                .filter(|point| !point.provisional)
            {
                let expected = oracle.get(&point.date).copied().unwrap_or(0);
                if point.quality == DataQuality::NotTracked {
                    continue;
                }
                assert_eq!(
                    point.value.unwrap_or(0),
                    expected,
                    "{id} on {} matches the independent line-regex oracle",
                    point.date
                );
            }
        }
    }

    fn live_task_status_map(
        vault: &Path,
    ) -> (std::collections::HashMap<char, String>, String) {
        let path =
            vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        let mut map = std::collections::HashMap::from([
            (' ', "TODO".to_string()),
            ('x', "DONE".to_string()),
            ('X', "DONE".to_string()),
            ('-', "CANCELLED".to_string()),
        ]);
        let mut global_filter = "#task".to_string();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (map, global_filter);
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return (map, global_filter);
        };
        if let Some(filter) = json.get("globalFilter").and_then(|v| v.as_str())
        {
            global_filter = filter.to_string();
        }
        for key in ["coreStatuses", "customStatuses"] {
            let Some(list) = json
                .get("statusSettings")
                .and_then(|settings| settings.get(key))
                .and_then(|value| value.as_array())
            else {
                continue;
            };
            for entry in list {
                let Some(symbol) = entry
                    .get("symbol")
                    .and_then(|value| value.as_str())
                    .and_then(|text| text.chars().next())
                else {
                    continue;
                };
                let status = entry
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("TODO")
                    .to_string();
                map.insert(symbol, status);
            }
        }
        (map, global_filter)
    }

    fn walk_markdown(vault: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut stack = vec![vault.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if file_type.is_dir() {
                    if name.starts_with('.')
                        || matches!(
                            name.as_ref(),
                            "_conflicts" | "_generated" | "_templates"
                        )
                    {
                        continue;
                    }
                    stack.push(entry.path());
                } else if file_type.is_file()
                    && name
                        .rsplit('.')
                        .next()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                {
                    files.push(entry.path());
                }
            }
        }
        files
    }

    #[test]
    fn default_range_and_overview_report_four_weeks() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, "stats:\n");
        let request = StatsRequest::at(fixture_vault(), config, fixed_now());
        let report = compute(&request).expect("default compute");
        assert_eq!(report.range.preset.as_deref(), Some("4w"));
        assert_eq!(report.range.days, 28);
        assert_eq!(
            report.range.start,
            NaiveDate::from_ymd_opt(2026, 9, 13).unwrap()
        );
        assert_eq!(report.range.end, day(10));
        assert!(report.range.includes_today);
        let ids: Vec<&str> = report
            .metrics
            .iter()
            .map(|metric| metric.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "worked",
                "reviewed",
                "done",
                "created",
                "net",
                "review_closed"
            ]
        );
        for metric in &report.metrics {
            // Four weeks of complete days plus the provisional today point.
            assert_eq!(metric.points.len(), 29, "day points for {}", metric.id);
            assert!(metric.points.last().unwrap().provisional);
        }
    }

    #[test]
    fn named_preset_range_is_reported() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let mut request = golden_request(fixture_vault(), config);
        request.range = Some("7d".to_string());
        let report = compute(&request).expect("7d compute");
        assert_eq!(report.range.preset.as_deref(), Some("7d"));
        assert_eq!(report.range.days, 7);
        assert_eq!(report.range.start, day(4));
        assert_eq!(report.range.end, day(10));
        assert!(report.range.includes_today);
    }

    #[test]
    fn selecting_an_invalidated_metric_includes_the_registry_warning() {
        let dir = TempDir::new().unwrap();
        let config = write_config(
            &dir,
            "
stats:
  metrics:
    dropped:
      label: Broken
      kind: nope
",
        );
        let mut request = golden_request(fixture_vault(), config);
        request.selection =
            MetricSelection::Explicit(vec!["dropped".to_string()]);
        let error = compute(&request).expect_err("invalidated metric");
        assert_eq!(error.code(), "unknown_metric");
        assert!(
            error.message().contains("stats.metrics.dropped.kind"),
            "error names the registry warning: {}",
            error.message()
        );
    }

    #[test]
    fn invalid_custom_and_duplicate_overview_still_compute() {
        let dir = TempDir::new().unwrap();
        let config = write_config(
            &dir,
            "
stats:
  overview: [worked, worked]
  metrics:
    bad: not-a-mapping
",
        );
        let request = StatsRequest::at(fixture_vault(), config, fixed_now());
        let report = compute(&request).expect("overview still computes");
        let ids: Vec<&str> = report
            .metrics
            .iter()
            .map(|metric| metric.id.as_str())
            .collect();
        assert_eq!(ids, ["worked"]);
        let bad_warnings: Vec<_> = report
            .warnings
            .iter()
            .filter(|warning| warning.message.contains("stats.metrics.bad"))
            .collect();
        assert_eq!(bad_warnings.len(), 1);
    }

    #[test]
    fn review_closed_redefined_as_formula_still_gates_journal_gaps() {
        let dir = TempDir::new().unwrap();
        let config = write_config(
            &dir,
            "
stats:
  metrics:
    morning_post:
      label: Morning post
      kind: checklist
      tags: [gtd, post]
      match: Morning review
      measure: bool
    review_closed:
      label: Review closed
      kind: formula
      expr: morning_post
      unit: bool
",
        );
        let mut request = golden_request(fixture_vault(), config);
        request.selection = MetricSelection::Explicit(vec![
            "reviewed".to_string(),
            "review_closed".to_string(),
        ]);
        let report = compute(&request).expect("formula gate computes");
        let closed = metric(&report, "review_closed");
        assert_eq!(closed.kind, "formula");
        expect_point(
            &closed.points[1],
            day(9),
            Some(1),
            DataQuality::Ok,
            false,
        );
        let reviewed = metric(&report, "reviewed");
        assert_eq!(reviewed.warnings[0].code, "journal_gap");
        assert_eq!(reviewed.warnings[0].date, Some(day(9)));
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "journal_gap"
                && warning.date == Some(day(9))));
    }

    #[test]
    fn builtin_limits_describe_task_checklist_and_journal_edges() {
        let report = golden_report();
        let done = metric(&report, "done");
        assert!(done
            .limits
            .iter()
            .any(|line| line.contains("Reopened tasks leave Done")));
        assert!(done
            .limits
            .iter()
            .any(|line| line.contains("daily-note ^gtd")));
        let habits = metric(&report, "habits");
        assert!(habits
            .limits
            .iter()
            .any(|line| line.contains("Renaming a chore")));
        let reviewed = metric(&report, "reviewed");
        assert!(reviewed
            .limits
            .iter()
            .any(|line| line.contains("first recorded answer")));
    }

    #[test]
    fn archive_move_does_not_change_task_metric_values() {
        let workspace = TempDir::new().unwrap();
        let vault = workspace.path().join("vault");
        copy_tree(&fixture_vault(), &vault);
        let config = write_config(&workspace, &golden_config());
        let before = compute(&golden_request(vault.clone(), config.clone()))
            .expect("before move");

        let tasks_path = vault.join("tasks.md");
        let tasks = std::fs::read_to_string(&tasks_path).unwrap();
        let mut moved = None;
        let mut kept = Vec::new();
        for line in tasks.lines() {
            if line.contains("Write draft") {
                moved = Some(line.to_string());
            } else {
                kept.push(line);
            }
        }
        let moved = moved.expect("Write draft row");
        std::fs::write(&tasks_path, kept.join("\n") + "\n").unwrap();
        let archive = vault.join("done/tasks_done.md");
        let mut archived = std::fs::read_to_string(&archive).unwrap();
        if !archived.ends_with('\n') {
            archived.push('\n');
        }
        archived.push_str(&moved);
        archived.push('\n');
        std::fs::write(&archive, archived).unwrap();

        let after =
            compute(&golden_request(vault, config)).expect("after move");
        for id in ["done", "created", "dropped", "net"] {
            let before_metric = metric(&before, id);
            let after_metric = metric(&after, id);
            let before_values: Vec<_> = before_metric
                .points
                .iter()
                .map(|point| (point.date, point.value, point.quality))
                .collect();
            let after_values: Vec<_> = after_metric
                .points
                .iter()
                .map(|point| (point.date, point.value, point.quality))
                .collect();
            assert_eq!(
                before_values, after_values,
                "{id} unchanged by archive move"
            );
        }
    }
}
