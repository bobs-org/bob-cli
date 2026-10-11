//! Assembled read-only Daily Stats engine entry point.
//!
//! The integration phase wires request validation, isolated stats
//! configuration and registry loading, one shared vault/journal index read,
//! inferred tracking starts, required dependency evaluation (formula inputs
//! plus the journal `review_closed` gap gate), and the series presentation
//! layer into [`compute`]. Evaluators stay pure: this module performs the
//! only I/O (the config file plus the single index pass) and never writes to
//! the vault or the config.

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
    series::{build_metric_series, resolve_range, ResolvedRange},
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
        Some(text) => RangeSpec::parse(text).map_err(StatsError::usage)?,
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
            evaluate_one(def, &index, &[], today, config.since, &probe)
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

    let needed: BTreeSet<&str> =
        ordered.iter().map(|def| def.id.as_str()).collect();
    let mut evaluations: BTreeMap<String, MetricEvaluation> = BTreeMap::new();
    for def in &ordered {
        if matches!(def.spec, MetricSpec::Journal { .. })
            && needed.contains("review_closed")
            && !evaluations.contains_key("review_closed")
            && let Some(review_closed) = registry.get("review_closed")
            && let Some(evaluation) = evaluate_one(
                review_closed,
                &index,
                &dates,
                today,
                config.since,
                &evaluations,
            )
        {
            evaluations.insert(review_closed.id.clone(), evaluation);
        }
        if let Some(evaluation) =
            evaluate_one(def, &index, &dates, today, config.since, &evaluations)
        {
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
        metrics,
        health: index_health(&index),
        warnings: dedupe_warnings(registry.warnings.clone()),
    })
}

/// Selected metrics plus their dependencies in evaluation order, with the
/// journal `review_closed` gate added whenever any journal metric needs it —
/// even when it is hidden or unselected.
fn evaluation_set<'a>(
    registry: &'a MetricRegistry,
    selected: &[&'a MetricDef],
) -> Vec<&'a MetricDef> {
    let mut needed = registry.with_dependencies(selected);
    let wants_journal = needed
        .iter()
        .any(|def| matches!(def.spec, MetricSpec::Journal { .. }));
    if wants_journal
        && let Some(review_closed) = registry.get("review_closed")
        && !needed.iter().any(|def| def.id == "review_closed")
    {
        needed.push(review_closed);
        needed.sort_by_key(|def| {
            registry
                .evaluation_order
                .iter()
                .position(|id| id == &def.id)
                .unwrap_or(usize::MAX)
        });
    }
    needed
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
        MetricSpec::Formula { .. } => {
            evaluate_formula(def, evaluations, dates, today, global_since)
        }
    }
}

/// Every date the evaluators must cover: the prior window (for the always
/// included prior summary and, under `compare`, prior points), the complete
/// window, and the provisional today point.
fn evaluation_dates(
    resolved: &ResolvedRange,
    today: NaiveDate,
) -> Vec<NaiveDate> {
    let start = resolved
        .prior
        .map(|span| span.start)
        .into_iter()
        .chain(resolved.complete.map(|span| span.start))
        .chain([today])
        .min()
        .unwrap_or(today);
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
    use std::path::PathBuf;

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
        assert!(report.warnings.is_empty());

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
        assert_eq!(worked.summary.delta.as_ref().unwrap().display, "new");
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
        expect_point(&done.points[2], day(10), Some(3), DataQuality::Ok, false);
        expect_point(&done.points[3], day(11), Some(1), DataQuality::Ok, true);
        assert_eq!(done.points[1].parts["kind"]["task"], 4);
        assert_eq!(done.points[1].parts["kind"]["ref"], 1);
        assert_eq!(done.points[1].parts["note"]["tasks.md"], 4);
        assert_eq!(done.points[1].parts["note"]["2026/20261009.md"], 1);
        assert_eq!(done.summary.headline.value, Some(4));
        assert_eq!(done.summary.total.as_ref().unwrap().value, Some(8));
        assert_eq!(done.summary.mean, Some(4));
        assert_eq!(done.summary.p25, Some(4));
        assert_eq!(done.summary.p75, Some(5));
        assert_eq!(done.summary.days_with_data, 2);
        let kinds = &done.parts_summary["kind"];
        assert_eq!(kinds[0].key, "task");
        assert_eq!(kinds[0].value, 7);
        assert_eq!(kinds[0].share, "88%");
        assert_eq!(kinds[1].key, "ref");
        assert_eq!(kinds[1].value, 1);
        assert_eq!(kinds[1].share, "13%");

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
            Some(3),
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
        assert_eq!(created.summary.headline.value, Some(3));
        assert_eq!(created.summary.total.as_ref().unwrap().value, Some(10));
        assert_eq!(created.summary.mean, Some(3));
        assert_eq!(created.summary.p25, Some(3));
        assert_eq!(created.summary.p75, Some(4));
        assert_eq!(created.summary.days_with_data, 3);
        let prior = created.summary.prior.as_ref().expect("prior summary");
        assert_eq!(prior.headline.value, Some(1));
        let delta = created.summary.delta.as_ref().expect("delta");
        assert_eq!(delta.display, "▲ 3×");
        assert_eq!(delta.words, "up 3 times vs the prior selected period");
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
        // since is Oct 10); Oct 10 flows 3 - 3 - 1 and shrinks.
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

        // M6 review_closed: met Oct 9 and 10; nothing due before tracking
        // (Oct 8) or today, which reads as missing rather than missed.
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
            None,
            DataQuality::Missing,
            true,
        );
        assert_eq!(closed.points[1].display, "met");
        assert_eq!(closed.summary.headline.value, Some(2));
        assert_eq!(closed.summary.headline.display, "2 of 2 days");
        assert_eq!(closed.summary.days_with_data, 2);
        assert_eq!(closed.summary.days_met, Some(2));
        let delta = closed.summary.delta.as_ref().expect("bool delta");
        assert_eq!(delta.display, "▲ 2 days");
        assert_eq!(delta.sign, 1);

        // M7 dropped: only the [!] row with a cancelled date counts.
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
            Some(1),
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
        assert_eq!(dropped.summary.headline.value, Some(1));
        assert_eq!(dropped.summary.days_with_data, 1);

        // M8 habits: the Meditate chore misses Oct 9 and completes late on
        // Oct 10; ratios pool rather than average.
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
            Some(10000),
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
        assert_eq!(habits.points[1].den, Some(1));
        assert_eq!(habits.points[1].parts["missed"]["meditate"], 1);
        assert_eq!(habits.points[2].parts["met"]["meditate"], 1);
        assert_eq!(habits.points[2].display, "100%");
        assert_eq!(habits.summary.headline.value, Some(5000));
        assert_eq!(habits.summary.headline.display, "50%");
        assert_eq!(habits.summary.mean, Some(5000));
        assert_eq!(habits.summary.p25, Some(2500));
        assert_eq!(habits.summary.p75, Some(7500));
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
        expect_point(&flow.points[2], day(10), Some(4), DataQuality::Ok, false);
        expect_point(&flow.points[3], day(11), Some(1), DataQuality::Ok, true);
        assert_eq!(flow.points[2].display, "+4");
        assert_eq!(flow.summary.headline.value, Some(4));

        // Health: each affected task counts once (the malformed created
        // date, the future scheduled date); the undated open task counts
        // once as missing-created; the journal has five events from one
        // host starting Oct 8.
        assert_eq!(report.health.tasks.invalid_dates, 2);
        assert_eq!(report.health.tasks.closed_without_date, 0);
        assert_eq!(report.health.tasks.missing_created, Some(1));
        assert_eq!(report.health.journal.since, Some(day(8)));
        assert_eq!(report.health.journal.malformed_lines, 0);
        assert_eq!(report.health.journal.hosts.len(), 1);
        let host = &report.health.journal.hosts[0];
        assert_eq!(host.host, "apollo");
        assert_eq!(host.events, 5);
        assert_eq!(host.last_ts, "2026-10-11T09:00:00-04:00");
        assert_eq!(host.last_day, day(11));
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
        assert_eq!(habits["points"][2]["den"], 1);
        assert_eq!(habits["points"][2]["value"], 10000);
        let closed = &json["metrics"][5];
        assert_eq!(closed["points"][1]["display"], "met");

        // Health keeps the schema counters; the gap warning names its day.
        assert_eq!(json["health"]["tasks"]["invalid_dates"], 2);
        assert_eq!(json["health"]["tasks"]["missing_created"], 1);
        assert_eq!(json["health"]["journal"]["hosts"][0]["events"], 5);
        assert_eq!(json["metrics"][1]["warnings"][0]["code"], "journal_gap");
        assert_eq!(json["metrics"][1]["warnings"][0]["date"], "2026-10-09");
        assert!(json["warnings"].as_array().unwrap().is_empty());
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

    fn snapshot_tree(root: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
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

    #[test]
    fn compute_is_deterministic_and_leaves_vault_and_config_untouched() {
        let dir = TempDir::new().unwrap();
        let config = write_config(&dir, &golden_config());
        let vault_before = snapshot_tree(&fixture_vault());
        let config_before = std::fs::read(&config).unwrap();
        let request = golden_request(fixture_vault(), config.clone());

        let first = serde_json::to_string(&compute(&request).unwrap()).unwrap();
        let second =
            serde_json::to_string(&compute(&request).unwrap()).unwrap();
        assert_eq!(first, second, "repeated calls agree byte for byte");

        assert_eq!(snapshot_tree(&fixture_vault()), vault_before);
        assert_eq!(std::fs::read(&config).unwrap(), config_before);
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

        // One clipped October bucket pools the two tracked days (0/1 and
        // 1/1); the untracked Oct 8 makes it partial and the provisional
        // today never enters the pool.
        assert_eq!(habits.points.len(), 1);
        let bucket = &habits.points[0];
        assert_eq!(bucket.date, day(8));
        assert_eq!(bucket.end, day(10));
        assert_eq!(bucket.value, Some(5000));
        assert_eq!(bucket.display, "50%");
        assert_eq!(bucket.num, Some(1));
        assert_eq!(bucket.den, Some(2));
        assert_eq!(bucket.quality, DataQuality::Partial);
        assert_eq!(bucket.partial, Some(true));
        assert!(!bucket.provisional);

        let worked = metric(&report, "worked");
        assert_eq!(worked.points.len(), 1);
        assert_eq!(worked.points[0].value, Some(32));
        assert_eq!(worked.points[0].display, "32m");
        assert_eq!(worked.points[0].parts["theme"]["GTD"], 29);
    }

    /// Opt-in live-vault parity: with BOB_STATS_REAL_VAULT_PARITY=1 (and
    /// BOB_NOW for a deterministic window) this runs compute against the
    /// real vault and checks two independent oracles over the complete days
    /// of a 7-day window: the sessions total equals a plain-text count of
    /// closed Pomodoro rows in canonical daily notes, and the worked total
    /// covers the recorded `[t:: Nm]` minutes. It also prints the compute
    /// wall time for rollout measurement (no CI time assertion). Without
    /// the opt-in variable this passes without touching any vault.
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
                    recorded_minutes += marker
                        .captures_iter(line)
                        .map(|captures| captures[1].parse::<i64>().unwrap_or(0))
                        .sum::<i64>();
                }
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
}
