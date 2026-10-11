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
    error::StatsError,
    formula::evaluate_formula,
    index::StatsIndex,
    journal::evaluate_journal,
    metrics::{effective_since, evaluate_metric, MetricEvaluation},
    range::RangeSpec,
    registry::{build_registry, MetricDef, MetricRegistry, MetricSpec},
    report::{
        HealthReport, JournalHealthReport, JournalHostReport, MetricReport,
        StatsReport, StatsWarning, TaskHealthReport, SCHEMA_VERSION,
    },
    request::StatsRequest,
    series::{
        build_metric_series, lead_in_start, resolve_range, ResolvedRange,
    },
};
use crate::native::config::load_stats_config;

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

    // Probe pass over an empty date span: tracking starts are index-derived,
    // so this yields every metric's inferred since without any daily values.
    // The result feeds `all`-range resolution below.
    let mut probe: BTreeMap<String, MetricEvaluation> = BTreeMap::new();
    for def in &ordered {
        if let Some(evaluation) =
            evaluate_one(def, &index, &[], today, config.since, None, &probe)
        {
            probe.insert(def.id.clone(), evaluation);
        }
    }
    let all_since = selected
        .iter()
        .filter_map(|def| {
            let inferred =
                probe.get(def.id.as_str()).and_then(|probe| probe.since);
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
        ) {
            evaluations.insert(def.id.clone(), evaluation);
        }
    }

    let mut metrics = Vec::with_capacity(selected.len());
    for def in &selected {
        let evaluation = evaluations
            .get(def.id.as_str())
            .expect("the dispatcher evaluates every selected metric kind");
        let series = build_metric_series(
            &evaluation.values,
            &def.id,
            def.unit,
            &resolved,
        );
        metrics.push(MetricReport {
            id: def.id.clone(),
            label: def.label.clone(),
            kind: def.kind_name().to_string(),
            unit: def.unit.as_str().to_string(),
            direction: def.direction.as_str().to_string(),
            color: def.color.as_str().to_string(),
            source: def.source.as_str().to_string(),
            origin: def.origin.as_str().to_string(),
            since: evaluation.since,
            definition: def.definition.clone(),
            limits: def.limits.clone(),
            parameters: Some(def.parameters()),
            summary: series.summary,
            points: series.points,
            prior_points: if request.compare {
                series.prior_points
            } else {
                Vec::new()
            },
            parts_summary: series.parts_summary,
            warnings: evaluation.warnings.clone(),
        });
    }

    Ok(StatsReport {
        ok: true,
        schema_version: SCHEMA_VERSION,
        today,
        range: resolved.report,
        health: index_health(&index),
        warnings: dedupe_warnings(
            registry
                .warnings
                .iter()
                .cloned()
                .chain(
                    metrics.iter().flat_map(|metric| metric.warnings.clone()),
                )
                .collect(),
        ),
        metrics,
    })
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
) -> Option<MetricEvaluation> {
    match &def.spec {
        MetricSpec::Pomodoro { .. }
        | MetricSpec::Tasks { .. }
        | MetricSpec::Checklist { .. }
        | MetricSpec::Field { .. } => {
            evaluate_metric(def, index, dates, today, global_since)
        }
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
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};

    use chrono::{Datelike, NaiveDate, NaiveDateTime};
    use tempfile::TempDir;

    use super::compute;
    use crate::native::stats::report::SeriesPoint;
    use crate::native::stats::request::{
        Grouping, MetricSelection, StatsRequest,
    };
    use crate::native::stats::value::DataQuality;

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
        assert_eq!(report.warnings.len(), 3);
        assert!(report
            .warnings
            .iter()
            .all(|warning| warning.code == "journal_gap"));

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

        // M1 worked: 25m GTD plus the odd 7m GTD+SASE composite (4m GTD,
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
        assert_eq!(worked.points[1].parts["theme"]["GTD"], 29);
        assert_eq!(worked.points[1].parts["theme"]["SASE"], 3);
        assert_eq!(worked.summary.headline.value, Some(16));
        assert_eq!(worked.summary.headline.display, "16m");
        assert_eq!(worked.summary.total.as_ref().unwrap().value, Some(32));
        assert_eq!(worked.summary.mean, Some(16));
        assert_eq!(worked.summary.p25, Some(8));
        assert_eq!(worked.summary.p75, Some(24));
        assert_eq!(worked.summary.days_with_data, 2);
        assert_eq!(worked.summary.delta.as_ref().unwrap().display, "–");
        let theme = &worked.parts_summary["theme"];
        assert_eq!(theme[0].key, "GTD");
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
        assert_eq!(closed.points[3].display, "missed");
        assert_eq!(closed.summary.headline.value, Some(2));
        assert_eq!(closed.summary.headline.display, "2 of 2 days");
        assert_eq!(closed.summary.days_with_data, 2);
        assert_eq!(closed.summary.days_met, Some(2));
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

        // M10 review_time: GTD components only; the SASE-only today scores
        // a provisional zero rather than missing.
        let review_time = metric(&report, "review_time");
        assert_eq!(review_time.since, Some(day(9)));
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
            Some(29),
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
        assert_eq!(review_time.summary.headline.value, Some(15));
        assert_eq!(review_time.summary.total.as_ref().unwrap().value, Some(29));
        assert_eq!(review_time.summary.mean, Some(15));
        assert_eq!(review_time.summary.p25, Some(7));
        assert_eq!(review_time.summary.p75, Some(22));

        // M11 sessions: count mode counts each closed row once even when it
        // carries two themes; parts still attribute the row to GTD.
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
        assert_eq!(sessions.points[1].parts["theme"]["GTD"], 2);
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
        // Empty parts maps are omitted; nulls serialize as null.
        assert!(worked["points"][0].get("parts").is_none());
        assert_eq!(worked["points"][0]["value"], serde_json::Value::Null);
        assert_eq!(worked["points"][0]["quality"], "not_tracked");
        assert_eq!(worked["points"][1]["parts"]["theme"]["GTD"], 29);
        assert_eq!(
            worked["points"][3]["rolling_median"],
            serde_json::Value::Null
        );
        assert_eq!(worked["points"][3]["provisional"], true);

        // Ratio points carry pooled num/den; bool points read as met days.
        let habits = &json["metrics"][7];
        assert_eq!(habits["id"], "habits");
        assert_eq!(habits["points"][2]["num"], 1);
        assert_eq!(habits["points"][2]["den"], 2);
        assert_eq!(habits["points"][2]["value"], 5000);
        let closed = &json["metrics"][5];
        assert_eq!(closed["points"][1]["display"], "met");

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
        assert_eq!(worked.points[0].parts["theme"]["GTD"], 29);
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
