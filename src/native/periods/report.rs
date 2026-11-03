//! `bob periods show`: one read-only daily stats pass assembled into the
//! period strip and parts-table contract.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{Datelike, Days, NaiveDate, NaiveDateTime, Weekday};
use clap::ArgMatches;
use serde_json::{json, Map, Value};

use super::{
    calendar::{weeks_in_year, PeriodId, PeriodState},
    labels, note,
};
use crate::native::{
    config::{self, load_stats_config},
    env as bob_env,
    stats::{
        clock::StatsClock,
        compute::compute,
        error::StatsError,
        range::RangeSpec,
        registry::MetricUnit,
        report::{SeriesPoint, StatsReport, TypicalRelation},
        request::{Grouping, MetricSelection, StatsRequest},
        series::{format_minutes, format_value, median},
    },
    style::Styler,
};

const COMMAND_NAME: &str = "bob periods show";
const SCHEMA_VERSION: u32 = 1;
const FACT_IDS: [&str; 5] =
    ["worked", "reviewed", "done", "created", "review_closed"];

#[derive(Debug, Clone)]
struct ShowError {
    code: String,
    message: String,
    hint: String,
    exit_code: i32,
}

impl ShowError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_period".to_string(),
            message: message.into(),
            hint: "Pass a period id such as 2026-W45, 2026-Q4, 2026, or all. See docs/periods.md."
                .to_string(),
            exit_code: 2,
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            code: "io_error".to_string(),
            message: message.into(),
            hint: "Check the vault path and file permissions.".to_string(),
            exit_code: 1,
        }
    }

    fn config(message: impl Into<String>) -> Self {
        Self {
            code: "config_error".to_string(),
            message: message.into(),
            hint: "Check the Bob stats configuration.".to_string(),
            exit_code: 2,
        }
    }

    fn from_stats(error: StatsError) -> Self {
        Self {
            code: error.code().to_string(),
            message: error.message().to_string(),
            hint: match error.exit_code() {
                1 => "Check the vault path and file permissions.".to_string(),
                _ => "Check the stats configuration and docs/periods.md."
                    .to_string(),
            },
            exit_code: error.exit_code(),
        }
    }

    fn json(&self) -> Value {
        json!({
            "ok": false,
            "schema_version": SCHEMA_VERSION,
            "error": {
                "code": self.code,
                "message": self.message,
                "hint": self.hint,
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Span {
    start: NaiveDate,
    end: NaiveDate,
}

#[derive(Debug, Clone, Default)]
struct MetricDays {
    points: BTreeMap<NaiveDate, SeriesPoint>,
    unit: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Clone)]
struct DailyMetrics {
    metrics: BTreeMap<String, MetricDays>,
    homes: Value,
    theme_hues: Value,
    warnings: Value,
}

#[derive(Debug, Clone)]
struct Aggregate {
    value: Option<i64>,
    of: Option<i64>,
    display: String,
    quality: &'static str,
    so_far: bool,
    offclock: Option<i64>,
    parts: BTreeMap<String, BTreeMap<String, i64>>,
}

#[derive(Debug, Clone)]
struct Typical {
    value: Option<i64>,
    display: String,
    relation: Option<TypicalRelation>,
    words: String,
    detail: String,
    start: NaiveDate,
    end: NaiveDate,
    periods: usize,
    usable: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        if matches.get_one::<String>("format").map(String::as_str)
            == Some("json")
        {
            Self::Json
        } else {
            Self::Human
        }
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = super::cli::show_command();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = OutputFormat::from_matches(&matches);
    match execute(&matches) {
        Ok(report) => {
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string(&report).unwrap_or_default()
                ),
                OutputFormat::Human => {
                    print!("{}", render_human(&report, &Styler::detect()))
                }
            }
            0
        }
        Err(error) => {
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string(&error.json()).unwrap_or_default()
                ),
                OutputFormat::Human => {
                    eprintln!("{COMMAND_NAME}: {}", error.message)
                }
            }
            error.exit_code
        }
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn execute(matches: &ArgMatches) -> Result<Value, ShowError> {
    let vault = bob_env::bob_dir();
    let now = bob_env::current_datetime();
    let today = now.date();
    let id = matches
        .get_one::<String>("period")
        .map(|text| PeriodId::parse(text).map_err(ShowError::usage))
        .transpose()?
        .unwrap_or_else(|| PeriodId::week_for_date(today));
    let clock = matches
        .get_one::<String>("clock")
        .and_then(|value| StatsClock::parse_cli(value))
        .unwrap_or_default();
    build_report(&vault, config::config_path(), id, now, clock)
}

fn build_report(
    vault: &Path,
    config_path: PathBuf,
    id: PeriodId,
    now: NaiveDateTime,
    clock: StatsClock,
) -> Result<Value, ShowError> {
    let today = now.date();
    let span = period_span(id, vault, today)?;
    let state = id.state(today);
    let visible_fact_ids = visible_fact_ids(&config_path)?;
    let history_start = typical_history_start(id).unwrap_or(span.start);
    let daily = if state == PeriodState::Future || visible_fact_ids.is_empty() {
        DailyMetrics {
            metrics: BTreeMap::new(),
            homes: json!({}),
            theme_hues: json!({}),
            warnings: json!([]),
        }
    } else {
        load_daily_metrics(
            vault,
            config_path,
            history_start.min(span.start),
            span.end.min(today),
            &visible_fact_ids,
            now,
            clock,
        )?
    };
    let mut report = report_base(vault, id, span, today, now, state)?;
    let observed_end = match state {
        PeriodState::Future => None,
        PeriodState::Current => Some(today),
        PeriodState::Closed => Some(span.end),
    };
    if let Some(observed_end) = observed_end {
        let observed = Span {
            start: span.start,
            end: observed_end.min(span.end),
        };
        report["period"]["coverage"] =
            coverage_value(span, observed_end, state, &daily);
        let facts = build_facts(id, observed, today, state, &daily, clock);
        let lanes = build_lanes(
            id,
            observed,
            today,
            &daily,
            &daily.homes,
            &daily.theme_hues,
        );
        let (columns, children, groups) = build_parts(
            vault,
            id,
            span,
            Some(observed_end),
            today,
            &daily,
            &daily.homes,
        )?;
        report["facts"] = Value::Array(facts);
        report["lanes"] = lanes;
        report["columns"] = Value::Array(columns);
        report["children"] = Value::Array(children);
        report["groups"] = Value::Array(groups);
        report["homes"] = daily.homes;
        report["theme_hues"] = daily.theme_hues;
    } else {
        report["facts"] = json!([]);
        report["lanes"] =
            json!({ "root_parent": [], "theme": [], "types_folded": false });
        report["columns"] = Value::Array(empty_future_columns(id, span));
        report["children"] = Value::Array(empty_future_children(id, span));
        report["groups"] = json!([]);
        report["homes"] = daily.homes;
        report["theme_hues"] = daily.theme_hues;
    }
    report["warnings"] = daily.warnings;
    Ok(report)
}

fn visible_fact_ids(config_path: &Path) -> Result<Vec<String>, ShowError> {
    let config = load_stats_config(config_path)
        .map_err(|error| ShowError::config(error.to_string()))?;
    Ok(FACT_IDS
        .iter()
        .filter(|id| !config.hide.iter().any(|hidden| hidden == **id))
        .map(|id| (*id).to_string())
        .collect())
}

fn load_daily_metrics(
    vault: &Path,
    config_path: PathBuf,
    start: NaiveDate,
    end: NaiveDate,
    ids: &[String],
    now: NaiveDateTime,
    clock: StatsClock,
) -> Result<DailyMetrics, ShowError> {
    if start > end {
        return Ok(DailyMetrics {
            metrics: BTreeMap::new(),
            homes: json!({}),
            theme_hues: json!({}),
            warnings: json!([]),
        });
    }
    let mut request = StatsRequest::at(vault.to_path_buf(), config_path, now);
    request.selection = MetricSelection::Explicit(ids.to_vec());
    request.range = Some(RangeSpec::Explicit { start, end }.label());
    request.grouping = Grouping::Day;
    request.clock = clock;
    let report = compute(&request).map_err(ShowError::from_stats)?;
    Ok(daily_metrics_from_report(report))
}

fn daily_metrics_from_report(report: StatsReport) -> DailyMetrics {
    let mut metrics = BTreeMap::new();
    for metric in report.metrics {
        let points = metric
            .points
            .iter()
            .map(|point| (point.date, point.clone()))
            .collect();
        metrics.insert(
            metric.id.clone(),
            MetricDays {
                points,
                unit: Some(metric.unit.clone()),
                label: Some(metric.label.clone()),
            },
        );
    }
    DailyMetrics {
        metrics,
        homes: serde_json::to_value(report.homes).unwrap_or_else(|_| json!({})),
        theme_hues: serde_json::to_value(report.theme_hues)
            .unwrap_or_else(|_| json!({})),
        warnings: serde_json::to_value(report.warnings)
            .unwrap_or_else(|_| json!([])),
    }
}

fn period_span(
    id: PeriodId,
    vault: &Path,
    today: NaiveDate,
) -> Result<Span, ShowError> {
    match id {
        PeriodId::AllTime => {
            let earliest = earliest_canonical_daily(vault)?.unwrap_or(today);
            let start = PeriodId::week_for_date(earliest)
                .start()
                .unwrap_or(earliest);
            Ok(Span { start, end: today })
        }
        _ => {
            let start = id.start().ok_or_else(|| {
                ShowError::usage(format!("invalid period {id}"))
            })?;
            let end = id.end().ok_or_else(|| {
                ShowError::usage(format!("invalid period {id}"))
            })?;
            Ok(Span { start, end })
        }
    }
}

fn earliest_canonical_daily(
    vault: &Path,
) -> Result<Option<NaiveDate>, ShowError> {
    let mut earliest = None;
    let entries = fs::read_dir(vault).map_err(|error| {
        ShowError::io(format!("read {}: {error}", vault.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            ShowError::io(format!("read vault entry: {error}"))
        })?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(folder) = dir.file_name().and_then(|name| name.to_str())
        else {
            continue;
        };
        if folder.len() != 4
            || !folder.bytes().all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        for file in fs::read_dir(&dir).map_err(|error| {
            ShowError::io(format!("read {}: {error}", dir.display()))
        })? {
            let file = file.map_err(|error| {
                ShowError::io(format!("read vault entry: {error}"))
            })?;
            let path = file.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str())
            else {
                continue;
            };
            if name.len() != 11 || !name.ends_with(".md") {
                continue;
            }
            let Some(date) =
                NaiveDate::parse_from_str(&name[..8], "%Y%m%d").ok()
            else {
                continue;
            };
            if date.format("%Y").to_string() == folder {
                earliest = Some(
                    earliest
                        .map_or(date, |current: NaiveDate| current.min(date)),
                );
            }
        }
    }
    Ok(earliest)
}

fn typical_history_start(id: PeriodId) -> Option<NaiveDate> {
    let count = match id {
        PeriodId::Week { .. } | PeriodId::Quarter { .. } => 4,
        PeriodId::Year(_) => 3,
        PeriodId::AllTime => return None,
    };
    let mut cursor = id;
    for _ in 0..count {
        cursor = cursor.prev()?;
    }
    cursor.start()
}

fn report_base(
    vault: &Path,
    id: PeriodId,
    span: Span,
    today: NaiveDate,
    now: NaiveDateTime,
    state: PeriodState,
) -> Result<Value, ShowError> {
    let path = id.path();
    let note_path = vault.join(&path);
    let contents = match fs::read_to_string(&note_path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ShowError::io(format!(
                "read {}: {error}",
                note_path.display()
            )))
        }
    };
    let parsed = note::parse(contents.as_deref().unwrap_or(""));
    let state_name = state_name(state);
    let observed_through = match state {
        PeriodState::Future => Value::Null,
        PeriodState::Current => json!(date_text(today)),
        PeriodState::Closed => json!(date_text(span.end)),
    };
    let days = span_days(span);
    let weeks = period_week_count(id, span);
    let period = json!({
        "id": id.to_string(),
        "kind": kind_name(id),
        "short_label": labels::short_label(id),
        "title": if id == PeriodId::AllTime { "All time".to_string() } else { labels::title(id) },
        "range_label": if id == PeriodId::AllTime { format!("{} – {}", date_text(span.start), date_text(span.end)) } else { labels::range_label(span.start, span.end) },
        "start": date_text(span.start),
        "end": date_text(span.end),
        "days": days,
        "weeks": weeks,
        "state": state_name,
        "observed_through": observed_through,
        "coverage": {
            "tracked_days": 0,
            "elapsed_days": match state { PeriodState::Future => 0, PeriodState::Current => span_days(Span { start: span.start, end: today.min(span.end) }), PeriodState::Closed => days },
            "total_days": days,
            "display": if state == PeriodState::Future { format!("Starts {}", date_weekday(span.start)) } else { String::new() },
        },
    });
    let parent = id.parent().map(|parent| {
        link_value(vault, parent, &labels::up_label(Some(parent)))
    });
    let prev = id.prev().map(|previous| {
        link_value(vault, previous, &labels::prev_label(previous, id_year(id)))
    });
    let next = id.next().map(|next| {
        link_value(vault, next, &labels::next_label(next, id_year(id)))
    });
    let mut links = Map::new();
    links.insert(
        "parent".to_string(),
        parent.unwrap_or_else(
            || json!({ "id": "org", "path": "org.md", "label": "↑ org" }),
        ),
    );
    links.insert("prev".to_string(), prev.unwrap_or(Value::Null));
    links.insert("next".to_string(), next.unwrap_or(Value::Null));
    Ok(json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "today": date_text(today),
        "generated_at": now.format("%Y-%m-%dT%H:%M:%S").to_string(),
        "period": period,
        "note": {
            "path": path.to_string_lossy(),
            "exists": contents.is_some(),
            "instructions_version": parsed.instructions_version,
            "review": { "state": parsed.review_state.as_str(), "headline": parsed.headline },
        },
        "links": Value::Object(links),
        "facts": [],
        "lanes": {},
        "columns": [],
        "groups": [],
        "children": [],
        "homes": {},
        "theme_hues": {},
        "warnings": [],
    }))
}

fn build_facts(
    id: PeriodId,
    span: Span,
    today: NaiveDate,
    state: PeriodState,
    daily: &DailyMetrics,
    clock: StatsClock,
) -> Vec<Value> {
    FACT_IDS
        .iter()
        .filter_map(|metric_id| {
            let metric = daily.metrics.get(*metric_id)?;
            let aggregate = aggregate(metric_id, metric, span, today);
            let unit = if *metric_id == "review_closed" { "days" } else { metric.unit.as_deref().unwrap_or("count") };
            let typical = if *metric_id == "review_closed" || id == PeriodId::AllTime {
                None
            } else {
                build_typical(id, metric_id, metric, today, state)
            };
            let mut fact = json!({
                "metric": metric_id,
                "label": metric.label.as_deref().unwrap_or(metric_id),
                "unit": unit,
                "value": aggregate.value,
                "display": aggregate.display,
                "so_far": aggregate.so_far,
                "quality": aggregate.quality,
                "typical": typical.as_ref().map(typical_value).unwrap_or(Value::Null),
            });
            if let Some(of) = aggregate.of { fact["of"] = json!(of); }
            if *metric_id == "worked"
                && clock != StatsClock::OffClock
                && let Some(offclock) = aggregate.offclock.filter(|value| *value > 0)
            {
                fact["offclock"] = json!({ "value": offclock, "display": format_minutes(offclock) });
            }
            Some(fact)
        })
        .collect()
}

fn build_typical(
    id: PeriodId,
    metric_id: &str,
    metric: &MetricDays,
    today: NaiveDate,
    state: PeriodState,
) -> Option<Typical> {
    let window_size = match id {
        PeriodId::Week { .. } | PeriodId::Quarter { .. } => 4,
        PeriodId::Year(_) => 3,
        PeriodId::AllTime => return None,
    };
    let mut prior = Vec::new();
    let mut cursor = id;
    for _ in 0..window_size {
        let Some(previous) = cursor.prev() else {
            return Some(no_typical(kind_name(id), id, window_size));
        };
        cursor = previous;
        prior.push(cursor);
    }
    let window_start = prior.last()?.start()?;
    let window_end = prior.first()?.end()?;
    let mut usable_values = Vec::new();
    for prior_id in &prior {
        let prior_span = Span {
            start: prior_id.start()?,
            end: prior_id.end()?,
        };
        let aggregate = aggregate(metric_id, metric, prior_span, today);
        if !period_is_usable(metric, prior_span, today) {
            continue;
        }
        let Some(value) = aggregate.value else {
            continue;
        };
        let scaled = match id {
            PeriodId::Week { .. } => value,
            _ => scale_to_weeks(
                value,
                period_week_count(
                    id,
                    Span {
                        start: id.start()?,
                        end: id.end()?,
                    },
                ),
                period_week_count(*prior_id, prior_span),
            ),
        };
        usable_values.push(scaled);
    }
    let usable = usable_values.len();
    let kind = kind_name(id);
    let periods_label = prior_window_label(&prior);
    if usable < 2 {
        let words = super::super::stats::typical::period_typical_words(
            kind, None, None, None,
        );
        let detail = super::super::stats::typical::period_typical_detail(
            kind,
            None,
            &periods_label,
            usable,
            window_size,
            None,
            None,
            None,
            false,
        );
        return Some(Typical {
            value: None,
            display: "–".to_string(),
            relation: None,
            words,
            detail,
            start: window_start,
            end: window_end,
            periods: window_size,
            usable,
        });
    }
    let value = median(&usable_values)?;
    let current = aggregate(
        metric_id,
        metric,
        Span {
            start: id.start()?,
            end: id.end()?,
        },
        today,
    )
    .value;
    let relation = if state == PeriodState::Closed {
        current.map(|current| compare_typical(current, value))
    } else {
        None
    };
    let difference = current.map(|current| {
        format_metric_value(
            current.saturating_sub(value).unsigned_abs() as i64,
            metric.unit.as_deref(),
        )
    });
    let display = format_metric_value(value, metric.unit.as_deref());
    let difference_for_words = difference
        .as_deref()
        .filter(|_| relation.is_some_and(|rel| rel != TypicalRelation::Equal));
    let words = super::super::stats::typical::period_typical_words(
        kind,
        Some(&display),
        relation,
        difference_for_words,
    );
    let window_end_label = periods_label;
    let target_weeks = period_week_count(
        id,
        Span {
            start: id.start()?,
            end: id.end()?,
        },
    );
    let scale_weeks =
        matches!(id, PeriodId::Quarter { .. } | PeriodId::Year(_))
            .then_some(target_weeks)
            .filter(|target| {
                prior.iter().any(|prior_id| {
                    let Some(prior_span) = period_bounds(*prior_id) else {
                        return false;
                    };
                    period_week_count(*prior_id, prior_span) != *target
                })
            });
    let words = if let Some(weeks) = scale_weeks {
        format!("{words}, scaled to {weeks} weeks")
    } else {
        words
    };
    let detail = super::super::stats::typical::period_typical_detail(
        kind,
        Some(&display),
        &window_end_label,
        usable,
        window_size,
        scale_weeks,
        relation,
        difference.as_deref(),
        state == PeriodState::Current,
    );
    Some(Typical {
        value: Some(value),
        display: display.clone(),
        relation,
        words,
        detail,
        start: window_start,
        end: window_end,
        periods: window_size,
        usable,
    })
}

fn period_is_usable(metric: &MetricDays, span: Span, today: NaiveDate) -> bool {
    if span.end >= today {
        return false;
    }
    let mut has_data = false;
    let mut date = span.start;
    loop {
        if let Some(point) = metric.points.get(&date) {
            if point.quality
                == crate::native::stats::value::DataQuality::NotTracked
            {
                return false;
            }
            has_data |= point.value.is_some();
        }
        if date == span.end {
            break;
        }
        let Some(next) = date.succ_opt() else {
            break;
        };
        date = next;
    }
    has_data
}

fn no_typical(kind: &str, id: PeriodId, periods: usize) -> Typical {
    let start = id.start().unwrap_or_else(|| {
        NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid lower bound")
    });
    let end = id.end().unwrap_or(start);
    let words = super::super::stats::typical::period_typical_words(
        kind, None, None, None,
    );
    let detail = super::super::stats::typical::period_typical_detail(
        kind, None, "", 0, periods, None, None, None, false,
    );
    Typical {
        value: None,
        display: "–".to_string(),
        relation: None,
        words,
        detail,
        start,
        end,
        periods,
        usable: 0,
    }
}

fn scale_to_weeks(value: i64, target_weeks: u32, source_weeks: u32) -> i64 {
    if source_weeks == 0 {
        return value;
    }
    let numerator = i128::from(value).saturating_mul(i128::from(target_weeks));
    let denominator = i128::from(source_weeks);
    let rounded = if numerator >= 0 {
        numerator + denominator / 2
    } else {
        numerator - denominator / 2
    };
    (rounded / denominator).clamp(i128::from(i64::MIN), i128::from(i64::MAX))
        as i64
}

fn compare_typical(current: i64, typical: i64) -> TypicalRelation {
    if current == typical
        || (typical != 0
            && (i128::from(current) - i128::from(typical))
                .abs()
                .saturating_mul(100)
                <= i128::from(typical.abs()))
    {
        TypicalRelation::Equal
    } else if current > typical {
        TypicalRelation::Above
    } else {
        TypicalRelation::Below
    }
}

fn typical_value(typical: &Typical) -> Value {
    json!({
        "value": typical.value,
        "display": typical.display,
        "relation": typical.relation,
        "words": typical.words,
        "detail": typical.detail,
        "window": {
            "start": date_text(typical.start),
            "end": date_text(typical.end),
            "periods": typical.periods,
            "usable": typical.usable,
        },
    })
}

fn aggregate(
    metric_id: &str,
    metric: &MetricDays,
    span: Span,
    today: NaiveDate,
) -> Aggregate {
    let points: Vec<&SeriesPoint> = metric
        .points
        .iter()
        .filter(|(date, _)| **date >= span.start && **date <= span.end)
        .map(|(_, point)| point)
        .collect();
    let values: Vec<i64> =
        points.iter().filter_map(|point| point.value).collect();
    let of = if metric_id == "review_closed" {
        Some(values.len() as i64)
    } else {
        None
    };
    let value = if metric_id == "review_closed" {
        (!values.is_empty())
            .then(|| values.iter().filter(|value| **value > 0).count() as i64)
    } else if values.is_empty() {
        None
    } else {
        Some(
            values
                .iter()
                .fold(0_i64, |sum, value| sum.saturating_add(*value)),
        )
    };
    let quality = combined_quality(&points);
    let unit = if metric_id == "review_closed" {
        Some("count")
    } else {
        metric.unit.as_deref()
    };
    let display = if metric_id == "review_closed" {
        match (value, of) {
            (Some(value), Some(of)) => format!("{value} of {of}"),
            _ => "–".to_string(),
        }
    } else {
        value.map_or_else(
            || "–".to_string(),
            |value| format_metric_value(value, unit),
        )
    };
    let mut parts = BTreeMap::<String, BTreeMap<String, i64>>::new();
    let mut offclock = 0_i64;
    for point in &points {
        if let Some(value) = point.off_clock {
            offclock = offclock.saturating_add(value);
        }
        for (dimension, items) in &point.parts {
            let into = parts.entry(dimension.clone()).or_default();
            for (key, value) in items {
                *into.entry(key.clone()).or_default() =
                    into.get(key).copied().unwrap_or(0).saturating_add(*value);
            }
        }
    }
    Aggregate {
        value,
        of,
        display,
        quality,
        so_far: span.start <= today && span.end >= today,
        offclock: (offclock > 0).then_some(offclock),
        parts,
    }
}

fn combined_quality(points: &[&SeriesPoint]) -> &'static str {
    use crate::native::stats::value::DataQuality;
    if points.is_empty() {
        return "not_tracked";
    }
    let has_value = points.iter().any(|point| point.value.is_some());
    if !has_value {
        if points
            .iter()
            .all(|point| point.quality == DataQuality::NotTracked)
        {
            return "not_tracked";
        }
        if points
            .iter()
            .any(|point| point.quality == DataQuality::Missing)
        {
            return "missing";
        }
        return "partial";
    }
    if points.iter().any(|point| {
        matches!(
            point.quality,
            DataQuality::Missing
                | DataQuality::NotTracked
                | DataQuality::Partial
        )
    }) {
        "partial"
    } else {
        "ok"
    }
}

fn format_metric_value(value: i64, unit: Option<&str>) -> String {
    let unit = match unit.unwrap_or("count") {
        "minutes" => MetricUnit::Minutes,
        "signed" => MetricUnit::Signed,
        "bool" => MetricUnit::Bool,
        "ratio" => MetricUnit::Ratio,
        _ => MetricUnit::Count,
    };
    format_value(Some(value), unit)
}

fn build_lanes(
    id: PeriodId,
    span: Span,
    today: NaiveDate,
    daily: &DailyMetrics,
    homes: &Value,
    theme_hues: &Value,
) -> Value {
    let Some(worked) = daily.metrics.get("worked") else {
        return json!({ "root_parent": [], "theme": [], "types_folded": id != PeriodId::AllTime && !matches!(id, PeriodId::Week { .. }) });
    };
    let aggregate = aggregate("worked", worked, span, today);
    let total = aggregate.value.unwrap_or(0).max(0);
    let mut roots = aggregate
        .parts
        .get("root_parent")
        .cloned()
        .unwrap_or_default();
    if roots.is_empty() {
        roots = aggregate.parts.get("area").cloned().unwrap_or_default();
    }
    let root_rows = lane_rows(roots, total, homes, None);
    let mut themes = aggregate.parts.get("theme").cloned().unwrap_or_default();
    let types_folded = !matches!(id, PeriodId::Week { .. });
    if types_folded && total > 0 {
        let mut kept = BTreeMap::new();
        let mut folded = 0_i64;
        for (key, minutes) in themes {
            let is_unnamed =
                key.trim().is_empty() || key.eq_ignore_ascii_case("(unnamed)");
            let under_two_percent =
                i128::from(minutes.max(0)) * 100 < i128::from(total) * 2;
            if is_unnamed || under_two_percent {
                folded = folded.saturating_add(minutes);
            } else {
                kept.insert(key, minutes);
            }
        }
        if folded > 0 {
            *kept.entry("other".to_string()).or_default() += folded;
        }
        themes = kept;
    }
    let theme_rows = lane_rows(themes, total, homes, Some(theme_hues));
    json!({ "root_parent": root_rows, "theme": theme_rows, "types_folded": types_folded })
}

fn lane_rows(
    values: BTreeMap<String, i64>,
    total: i64,
    catalog: &Value,
    themes: Option<&Value>,
) -> Vec<Value> {
    let mut rows: Vec<_> =
        values.into_iter().filter(|(_, value)| *value > 0).collect();
    rows.sort_by(|(key_a, value_a), (key_b, value_b)| {
        value_b.cmp(value_a).then_with(|| key_a.cmp(key_b))
    });
    rows.into_iter()
        .map(|(key, minutes)| {
            let label = themes
                .and_then(|catalog| catalog.get(normalize_key(&key)))
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str)
                .or_else(|| {
                    catalog
                        .get(&key)
                        .and_then(|entry| entry.get("label"))
                        .and_then(Value::as_str)
                })
                .unwrap_or(&key);
            json!({
                "key": key,
                "label": label,
                "minutes": minutes,
                "display": format_minutes(minutes),
                "share": percent(minutes, total),
            })
        })
        .collect()
}

fn normalize_key(text: &str) -> String {
    text.trim().to_lowercase().replace(' ', "_")
}

fn percent(value: i64, total: i64) -> String {
    if total <= 0 {
        return "0%".to_string();
    }
    let rounded = (i128::from(value).saturating_mul(100)
        + i128::from(total) / 2)
        / i128::from(total);
    format!("{}%", rounded)
}

#[derive(Debug, Clone)]
struct PartSpec {
    id: Option<PeriodId>,
    day: Option<NaiveDate>,
    span: Span,
    group: Option<String>,
}

fn build_parts(
    vault: &Path,
    id: PeriodId,
    span: Span,
    observed_end: Option<NaiveDate>,
    today: NaiveDate,
    daily: &DailyMetrics,
    homes: &Value,
) -> Result<(Vec<Value>, Vec<Value>, Vec<Value>), ShowError> {
    let column_specs = column_specs(id, span);
    let child_specs = child_specs(id, span);
    let mut columns = Vec::with_capacity(column_specs.len());
    for spec in &column_specs {
        columns.push(build_column(
            vault,
            spec.clone(),
            observed_end,
            today,
            daily,
            homes,
        )?);
    }
    let mut children = Vec::with_capacity(child_specs.len());
    for spec in &child_specs {
        children.push(build_child(
            vault,
            spec.clone(),
            observed_end,
            today,
            daily,
            homes,
        )?);
    }
    let groups = build_groups(id, span, observed_end, today, daily);
    Ok((columns, children, groups))
}

fn column_specs(id: PeriodId, span: Span) -> Vec<PartSpec> {
    match id {
        PeriodId::Week { .. } => dates_between(span)
            .into_iter()
            .map(|day| PartSpec {
                id: None,
                day: Some(day),
                span: Span {
                    start: day,
                    end: day,
                },
                group: None,
            })
            .collect(),
        PeriodId::Quarter { .. } | PeriodId::Year(_) => {
            let weeks = weeks_between(span);
            weeks
                .into_iter()
                .map(|week| {
                    let group = if matches!(id, PeriodId::Year(_)) {
                        Some(format!(
                            "Q{}",
                            match week {
                                PeriodId::Week { week, .. } =>
                                    super::calendar::quarter_of_week(week),
                                _ => 1,
                            }
                        ))
                    } else {
                        None
                    };
                    PartSpec {
                        id: Some(week),
                        day: None,
                        span: clip_span(
                            Span {
                                start: week.start().unwrap(),
                                end: week.end().unwrap(),
                            },
                            span,
                        ),
                        group,
                    }
                })
                .collect()
        }
        PeriodId::AllTime => quarters_between(span)
            .into_iter()
            .map(|quarter| PartSpec {
                id: Some(quarter),
                day: None,
                span: clip_span(
                    Span {
                        start: quarter.start().unwrap(),
                        end: quarter.end().unwrap(),
                    },
                    span,
                ),
                group: Some(format!("{}", quarter_year(quarter))),
            })
            .collect(),
    }
}

fn child_specs(id: PeriodId, span: Span) -> Vec<PartSpec> {
    match id {
        PeriodId::Week { .. } => column_specs(id, span),
        PeriodId::Quarter { .. } => column_specs(id, span),
        PeriodId::Year(year) => (1..=4)
            .filter_map(|quarter| {
                let id = PeriodId::Quarter { year, quarter };
                let qspan = period_bounds(id)?;
                Some(PartSpec {
                    id: Some(id),
                    day: None,
                    span: clip_span(qspan, span),
                    group: None,
                })
            })
            .collect(),
        PeriodId::AllTime => {
            let first_year = PeriodId::year_for_date(span.start);
            let last_year = PeriodId::year_for_date(span.end);
            let (PeriodId::Year(first), PeriodId::Year(last)) =
                (first_year, last_year)
            else {
                return Vec::new();
            };
            (first..=last)
                .map(|year| {
                    let id = PeriodId::Year(year);
                    PartSpec {
                        id: Some(id),
                        day: None,
                        span: clip_span(
                            period_bounds(id).unwrap_or(span),
                            span,
                        ),
                        group: None,
                    }
                })
                .collect()
        }
    }
}

fn build_column(
    vault: &Path,
    spec: PartSpec,
    observed_end: Option<NaiveDate>,
    today: NaiveDate,
    daily: &DailyMetrics,
    homes: &Value,
) -> Result<Value, ShowError> {
    if let Some(day) = spec.day {
        let state = day_state(day, today);
        let path = daily_path(day);
        let path_exists = vault.join(&path).is_file();
        if state == PeriodState::Future {
            return Ok(json!({
                "id": date_text(day), "kind": "day", "label": labels::day_label(day, day.weekday() == Weekday::Mon),
                "caption": format!("{} {}", weekday(day), day.day()), "path": path, "exists": path_exists,
                "start": date_text(day), "end": date_text(day), "state": "future", "partial": null,
                "worked": null, "stack": [], "mark": null, "tick": null, "group": null,
            }));
        }
        let worked = daily
            .metrics
            .get("worked")
            .map(|metric| aggregate("worked", metric, spec.span, today));
        let closeout = closeout_for_day(day, daily, today);
        return Ok(json!({
            "id": date_text(day), "kind": "day", "label": labels::day_label(day, day.weekday() == Weekday::Mon),
            "caption": format!("{} {}", weekday(day), day.day()), "path": path, "exists": path_exists,
            "start": date_text(day), "end": date_text(day), "state": state_name(state), "partial": null,
            "worked": worked.as_ref().map(worked_column), "stack": worked.as_ref().map(|value| stack_value(value, homes)).unwrap_or_default(),
            "mark": closeout, "tick": null, "group": spec.group,
        }));
    }
    let child_id = spec.id.expect("period column has a period id");
    let state = child_id.state(today);
    let worked = if state == PeriodState::Future || observed_end.is_none() {
        None
    } else {
        daily.metrics.get("worked").map(|metric| {
            aggregate(
                "worked",
                metric,
                observed_span(spec.span, observed_end),
                today,
            )
        })
    };
    let note_exists = vault.join(child_id.path()).is_file();
    let partial = period_bounds(child_id).is_some_and(|natural| {
        natural.start != spec.span.start || natural.end != spec.span.end
    });
    let mark = if matches!(child_id, PeriodId::Week { .. }) {
        Some(review_state_for(vault, child_id)?.0)
    } else {
        None
    };
    let range_label = labels::range_label(spec.span.start, spec.span.end);
    let tick = if matches!(child_id, PeriodId::Week { .. })
        && matches!(state, PeriodState::Current | PeriodState::Closed)
    {
        month_tick(spec.span)
    } else {
        None
    };
    Ok(json!({
        "id": child_id.to_string(), "kind": kind_name(child_id), "label": labels::short_label(child_id),
        "caption": labels::short_label(child_id), "path": child_id.path().to_string_lossy(), "exists": note_exists,
        "start": date_text(spec.span.start), "end": date_text(spec.span.end), "state": state_name(state),
        "partial": partial, "worked": worked.as_ref().map(worked_column),
        "stack": worked.as_ref().map(|value| stack_value(value, homes)).unwrap_or_default(),
        "mark": mark, "tick": tick, "group": spec.group,
        "range_label": range_label,
    }))
}

fn build_child(
    vault: &Path,
    spec: PartSpec,
    observed_end: Option<NaiveDate>,
    today: NaiveDate,
    daily: &DailyMetrics,
    homes: &Value,
) -> Result<Value, ShowError> {
    if let Some(day) = spec.day {
        let state = day_state(day, today);
        let path = daily_path(day);
        let exists = vault.join(&path).is_file();
        if state == PeriodState::Future {
            return Ok(json!({
                "id": date_text(day),
                "kind": "day",
                "label": labels::day_label(day, day.weekday() == Weekday::Mon),
                "range_label": date_text(day),
                "path": path,
                "exists": exists,
                "state": "future",
            }));
        }
        let mut child =
            build_column(vault, spec, observed_end, today, daily, homes)?;
        child["review"] = Value::Null;
        child["closeout"] = child["mark"].clone();
        child["done"] = daily
            .metrics
            .get("done")
            .map(|metric| {
                aggregate(
                    "done",
                    metric,
                    Span {
                        start: day,
                        end: day,
                    },
                    today,
                )
            })
            .as_ref()
            .map(simple_metric_child)
            .unwrap_or(Value::Null);
        if let Some(worked) = daily.metrics.get("worked").map(|metric| {
            aggregate(
                "worked",
                metric,
                Span {
                    start: day,
                    end: day,
                },
                today,
            )
        }) {
            child["worked"] = worked_child(&worked);
            child["mostly"] = json!(mostly_rows(&worked, homes));
            child["mostly_display"] = json!(mostly_display(&worked, homes));
            child["stack"] = json!(stack_value(&worked, homes));
        } else {
            child["mostly"] = json!([]);
            child["mostly_display"] = json!("");
        }
        return Ok(child);
    }
    let id = spec.id.expect("period child has id");
    let state = id.state(today);
    let label = labels::short_label(id);
    let path = id.path();
    let exists = vault.join(&path).is_file();
    let mut child = json!({
        "id": id.to_string(), "kind": kind_name(id), "label": label,
        "range_label": labels::range_label(spec.span.start, spec.span.end),
        "path": path.to_string_lossy(), "exists": exists, "state": state_name(state),
    });
    if state == PeriodState::Future || observed_end.is_none() {
        return Ok(child);
    }
    let span = observed_span(spec.span, observed_end);
    let worked = daily
        .metrics
        .get("worked")
        .map(|metric| aggregate("worked", metric, span, today));
    let done = daily
        .metrics
        .get("done")
        .map(|metric| aggregate("done", metric, span, today));
    child["worked"] = worked.as_ref().map(worked_child).unwrap_or(Value::Null);
    child["done"] = done
        .as_ref()
        .map(simple_metric_child)
        .unwrap_or(Value::Null);
    let review = review_state_for(vault, id)?;
    child["review"] = json!({ "state": review.0, "headline": review.1, "reflection_lead": review.2 });
    child["closeout"] = Value::Null;
    if let Some(aggregate) = worked {
        child["mostly"] = json!(mostly_rows(&aggregate, homes));
        child["mostly_display"] = json!(mostly_display(&aggregate, homes));
        child["stack"] = json!(stack_value(&aggregate, homes));
    } else {
        child["mostly"] = json!([]);
        child["mostly_display"] = json!("");
        child["stack"] = json!([]);
    }
    Ok(child)
}

fn worked_column(aggregate: &Aggregate) -> Value {
    json!({ "value": aggregate.value, "display": aggregate.display, "quality": aggregate.quality })
}

fn worked_child(aggregate: &Aggregate) -> Value {
    json!({
        "value": aggregate.value,
        "display": aggregate.display,
        "so_far": aggregate.so_far,
        "quality": aggregate.quality,
    })
}

fn simple_metric_child(aggregate: &Aggregate) -> Value {
    json!({ "value": aggregate.value, "display": aggregate.display, "quality": aggregate.quality })
}

fn closeout_for_day(
    day: NaiveDate,
    daily: &DailyMetrics,
    today: NaiveDate,
) -> Option<&'static str> {
    if day == today {
        return Some("open");
    }
    let point = daily.metrics.get("review_closed")?.points.get(&day)?;
    if point.value.is_none()
        || point.quality == crate::native::stats::value::DataQuality::NotTracked
    {
        None
    } else if point.value.unwrap_or(0) > 0 {
        Some("met")
    } else {
        Some("missed")
    }
}

fn review_state_for(
    vault: &Path,
    id: PeriodId,
) -> Result<(String, Option<String>, Option<String>), ShowError> {
    let path = vault.join(id.path());
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ShowError::io(format!(
                "read {}: {error}",
                path.display()
            )))
        }
    };
    let parsed = note::parse(contents.as_deref().unwrap_or(""));
    Ok((
        parsed.review_state.as_str().to_string(),
        parsed.headline,
        parsed.reflection_lead,
    ))
}

fn mostly_rows(aggregate: &Aggregate, homes: &Value) -> Vec<Value> {
    let root = aggregate
        .parts
        .get("root_parent")
        .or_else(|| aggregate.parts.get("area"));
    let Some(root) = root else {
        return Vec::new();
    };
    let total = aggregate.value.unwrap_or(0);
    if total <= 0 {
        return Vec::new();
    }
    let mut rows: Vec<_> =
        root.iter().filter(|(_, value)| **value > 0).collect();
    rows.sort_by(|(key_a, value_a), (key_b, value_b)| {
        value_b.cmp(value_a).then_with(|| key_a.cmp(key_b))
    });
    rows.into_iter()
        .take(3)
        .map(|(key, value)| {
            json!({
                "key": key,
                "label": home_label(homes, key),
                "share": percent(*value, total),
            })
        })
        .collect()
}

fn mostly_display(aggregate: &Aggregate, homes: &Value) -> String {
    mostly_rows(aggregate, homes)
        .iter()
        .filter_map(|row| {
            Some(format!(
                "{} {}",
                row.get("label")?.as_str()?,
                row.get("share")?.as_str()?
            ))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn stack_value(aggregate: &Aggregate, homes: &Value) -> Vec<Value> {
    let root = aggregate
        .parts
        .get("root_parent")
        .or_else(|| aggregate.parts.get("area"));
    let Some(root) = root else {
        return Vec::new();
    };
    let mut rows: Vec<_> =
        root.iter().filter(|(_, value)| **value > 0).collect();
    rows.sort_by(|(key_a, value_a), (key_b, value_b)| {
        value_b.cmp(value_a).then_with(|| key_a.cmp(key_b))
    });
    rows.into_iter()
        .map(|(key, value)| {
            let _label = home_label(homes, key);
            json!({ "key": key, "minutes": value })
        })
        .collect()
}

fn home_label(homes: &Value, key: &str) -> String {
    homes
        .get(key)
        .and_then(|home| home.get("label"))
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_string()
}

fn build_groups(
    id: PeriodId,
    parent_span: Span,
    observed_end: Option<NaiveDate>,
    today: NaiveDate,
    daily: &DailyMetrics,
) -> Vec<Value> {
    match id {
        PeriodId::Year(year) => (1..=4)
            .map(|quarter| {
                let child_id = PeriodId::Quarter { year, quarter };
                let span = clip_span(
                    period_bounds(child_id).unwrap_or(parent_span),
                    parent_span,
                );
                group_row(
                    format!("Q{quarter}"),
                    format!("Q{quarter}"),
                    span,
                    observed_end,
                    today,
                    daily,
                )
            })
            .collect(),
        PeriodId::AllTime => {
            let (PeriodId::Year(first), PeriodId::Year(last)) = (
                PeriodId::year_for_date(parent_span.start),
                PeriodId::year_for_date(parent_span.end),
            ) else {
                return Vec::new();
            };
            (first..=last)
                .map(|year| {
                    let child_id = PeriodId::Year(year);
                    let span = clip_span(
                        period_bounds(child_id).unwrap_or(parent_span),
                        parent_span,
                    );
                    group_row(
                        year.to_string(),
                        year.to_string(),
                        span,
                        observed_end,
                        today,
                        daily,
                    )
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn group_row(
    key: String,
    label: String,
    span: Span,
    observed_end: Option<NaiveDate>,
    today: NaiveDate,
    daily: &DailyMetrics,
) -> Value {
    let worked = observed_end.and_then(|end| {
        daily.metrics.get("worked").map(|metric| {
            aggregate("worked", metric, observed_span(span, Some(end)), today)
        })
    });
    json!({
        "key": key,
        "label": label,
        "worked_display": worked.map(|aggregate| aggregate.display).unwrap_or_else(|| "–".to_string()),
    })
}

fn empty_future_columns(id: PeriodId, span: Span) -> Vec<Value> {
    column_specs(id, span).into_iter().map(|spec| {
        if let Some(day) = spec.day {
            json!({ "id": date_text(day), "kind": "day", "label": labels::day_label(day, day.weekday() == Weekday::Mon), "caption": format!("{} {}", weekday(day), day.day()), "start": date_text(day), "end": date_text(day), "state": "future", "partial": null, "worked": null, "stack": [], "mark": null, "tick": null, "group": spec.group })
        } else {
            let child = spec.id.expect("period spec id");
            json!({ "id": child.to_string(), "kind": kind_name(child), "label": labels::short_label(child), "caption": labels::short_label(child), "path": child.path().to_string_lossy(), "exists": false, "start": date_text(spec.span.start), "end": date_text(spec.span.end), "state": "future", "partial": false, "worked": null, "stack": [], "mark": null, "tick": null, "group": spec.group })
        }
    }).collect()
}

fn empty_future_children(id: PeriodId, span: Span) -> Vec<Value> {
    child_specs(id, span).into_iter().map(|spec| {
        if let Some(day) = spec.day {
            json!({ "id": date_text(day), "kind": "day", "label": labels::day_label(day, day.weekday() == Weekday::Mon), "range_label": date_text(day), "path": daily_path(day), "exists": false, "state": "future" })
        } else {
            let child = spec.id.expect("period child id");
            json!({ "id": child.to_string(), "kind": kind_name(child), "label": labels::short_label(child), "range_label": labels::range_label(spec.span.start, spec.span.end), "path": child.path().to_string_lossy(), "exists": false, "state": "future" })
        }
    }).collect()
}

fn coverage_value(
    span: Span,
    observed_end: NaiveDate,
    state: PeriodState,
    daily: &DailyMetrics,
) -> Value {
    let elapsed_end = observed_end.min(span.end);
    let elapsed = span_days(Span {
        start: span.start,
        end: elapsed_end,
    });
    let tracked = daily
        .metrics
        .get("worked")
        .map(|metric| {
            dates_between(Span {
                start: span.start,
                end: elapsed_end,
            })
            .into_iter()
            .filter(|date| {
                metric.points.get(date).is_some_and(|point| {
                    !matches!(
                        point.quality,
                        crate::native::stats::value::DataQuality::NotTracked
                            | crate::native::stats::value::DataQuality::Missing
                    )
                })
            })
            .count() as u32
        })
        .unwrap_or(0);
    let display = match state {
        PeriodState::Future => format!("Starts {}", date_weekday(span.start)),
        PeriodState::Current => {
            format!("so far · {}–{}", weekday(span.start), weekday(elapsed_end))
        }
        PeriodState::Closed if tracked < span_days(span) => {
            format!("{tracked} of {} days logged", span_days(span))
        }
        PeriodState::Closed => String::new(),
    };
    json!({ "tracked_days": tracked, "elapsed_days": elapsed, "total_days": span_days(span), "display": display })
}

fn dates_between(span: Span) -> Vec<NaiveDate> {
    if span.start > span.end {
        return Vec::new();
    }
    let mut dates = Vec::new();
    let mut date = span.start;
    loop {
        dates.push(date);
        if date == span.end {
            break;
        }
        let Some(next) = date.succ_opt() else {
            break;
        };
        date = next;
    }
    dates
}

fn weeks_between(span: Span) -> Vec<PeriodId> {
    let Some(mut date) = PeriodId::week_for_date(span.start).start() else {
        return Vec::new();
    };
    let mut weeks = Vec::new();
    while date <= span.end {
        weeks.push(PeriodId::week_for_date(date));
        let Some(next) = date.checked_add_days(Days::new(7)) else {
            break;
        };
        date = next;
    }
    weeks
}

fn quarters_between(span: Span) -> Vec<PeriodId> {
    let mut cursor = PeriodId::quarter_for_date(span.start);
    let mut quarters = Vec::new();
    loop {
        if cursor.start().is_some_and(|start| start > span.end) {
            break;
        }
        quarters.push(cursor);
        let Some(next) = cursor.next() else {
            break;
        };
        cursor = next;
    }
    quarters
}

fn period_bounds(id: PeriodId) -> Option<Span> {
    Some(Span {
        start: id.start()?,
        end: id.end()?,
    })
}

fn clip_span(inner: Span, outer: Span) -> Span {
    Span {
        start: inner.start.max(outer.start),
        end: inner.end.min(outer.end),
    }
}

fn observed_span(span: Span, observed_end: Option<NaiveDate>) -> Span {
    Span {
        start: span.start,
        end: observed_end.map_or(span.end, |end| end.min(span.end)),
    }
}

fn span_days(span: Span) -> u32 {
    if span.start > span.end {
        return 0;
    }
    (span.end - span.start)
        .num_days()
        .saturating_add(1)
        .try_into()
        .unwrap_or(0)
}

fn period_week_count(id: PeriodId, span: Span) -> u32 {
    match id {
        PeriodId::Week { .. } => 1,
        PeriodId::Quarter { .. } => span_days(span).div_ceil(7),
        PeriodId::Year(year) => weeks_in_year(year),
        PeriodId::AllTime => weeks_between(span).len() as u32,
    }
}

fn month_tick(span: Span) -> Option<String> {
    (1..=12)
        .find_map(|month| {
            let first = NaiveDate::from_ymd_opt(span.start.year(), month, 1)?;
            (first >= span.start && first <= span.end)
                .then(|| first.format("%b").to_string())
        })
        .or_else(|| {
            for year in [span.start.year(), span.end.year()] {
                for month in 1..=12 {
                    if let Some(first) = NaiveDate::from_ymd_opt(year, month, 1)
                        && first >= span.start
                        && first <= span.end
                    {
                        return Some(first.format("%b").to_string());
                    }
                }
            }
            None
        })
}

fn day_state(day: NaiveDate, today: NaiveDate) -> PeriodState {
    if day < today {
        PeriodState::Closed
    } else if day > today {
        PeriodState::Future
    } else {
        PeriodState::Current
    }
}

fn state_name(state: PeriodState) -> &'static str {
    match state {
        PeriodState::Future => "future",
        PeriodState::Current => "current",
        PeriodState::Closed => "closed",
    }
}

fn kind_name(id: PeriodId) -> &'static str {
    match id {
        PeriodId::Week { .. } => "week",
        PeriodId::Quarter { .. } => "quarter",
        PeriodId::Year(_) => "year",
        PeriodId::AllTime => "all",
    }
}

fn id_year(id: PeriodId) -> i32 {
    match id {
        PeriodId::Week { year, .. }
        | PeriodId::Quarter { year, .. }
        | PeriodId::Year(year) => year,
        PeriodId::AllTime => 0,
    }
}

fn quarter_year(id: PeriodId) -> i32 {
    match id {
        PeriodId::Quarter { year, .. } => year,
        _ => 0,
    }
}

fn link_value(vault: &Path, id: PeriodId, label: &str) -> Value {
    let path = id.path();
    json!({ "id": id.to_string(), "path": path.to_string_lossy(), "label": label, "exists": vault.join(path).is_file() })
}

fn daily_path(day: NaiveDate) -> String {
    format!(
        "{}/{:04}{:02}{:02}.md",
        day.format("%Y"),
        day.year(),
        day.month(),
        day.day()
    )
}

fn date_text(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn weekday(date: NaiveDate) -> &'static str {
    match date.weekday() {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

fn date_weekday(date: NaiveDate) -> String {
    labels::day_label(date, true)
}

fn prior_window_label(periods: &[PeriodId]) -> String {
    let Some(first) = periods.last() else {
        return String::new();
    };
    let Some(last) = periods.first() else {
        return String::new();
    };
    match (first, last) {
        (
            PeriodId::Week {
                year: first_year,
                week: first_week,
            },
            PeriodId::Week {
                year: last_year,
                week: last_week,
            },
        ) if first_year == last_year => format!("W{first_week}–W{last_week}"),
        (
            PeriodId::Quarter {
                year: first_year,
                quarter: first_quarter,
            },
            PeriodId::Quarter {
                year: last_year,
                quarter: last_quarter,
            },
        ) if first_year == last_year => {
            format!("Q{first_quarter}–Q{last_quarter}")
        }
        (PeriodId::Year(first_year), PeriodId::Year(last_year)) => {
            format!("{first_year}–{last_year}")
        }
        _ => format!("{first}–{last}"),
    }
}

fn render_human(report: &Value, styler: &Styler) -> String {
    let mut lines = Vec::new();
    let period = &report["period"];
    let title = period["title"].as_str().unwrap_or("Period");
    let coverage = period["coverage"]["display"].as_str().unwrap_or("");
    let state = period["state"].as_str().unwrap_or("current");
    let right = if coverage.is_empty() {
        state.to_string()
    } else {
        coverage.to_string()
    };
    lines.push(format!("{}  {}", styler.bold(title), styler.dim(&right)));

    if report["facts"]
        .as_array()
        .is_some_and(|facts| !facts.is_empty())
    {
        for fact in report["facts"].as_array().into_iter().flatten() {
            let label = fact["label"].as_str().unwrap_or("Metric");
            let display = fact["display"].as_str().unwrap_or("–");
            let mut line = format!("{}  {}", styler.cyan(label), display);
            if let Some(words) = fact["typical"]["words"].as_str() {
                line.push_str(&format!("  {}", styler.dim(words)));
            }
            if let Some(offclock) = fact["offclock"]["display"].as_str() {
                line.push_str(&format!(
                    "  {}",
                    styler.dim(&format!("◌ +{offclock} off-clock"))
                ));
            }
            lines.push(line);
        }
    } else if state == "future" {
        lines.push(styler.dim(coverage));
    }

    let roots = report["lanes"]["root_parent"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !roots.is_empty() {
        let total: i64 =
            roots.iter().filter_map(|row| row["minutes"].as_i64()).sum();
        let bar: String = roots
            .iter()
            .map(|row| {
                let minutes = row["minutes"].as_i64().unwrap_or(0);
                let width = if total > 0 {
                    ((minutes as f64 / total as f64) * 24.0).round() as usize
                } else {
                    0
                };
                "█".repeat(width.max(1))
            })
            .collect::<Vec<_>>()
            .join("");
        lines.push(styler.blue(&bar));
        lines.push(
            roots
                .iter()
                .map(|row| {
                    format!(
                        "{} {} {}",
                        row["label"].as_str().unwrap_or(""),
                        row["display"].as_str().unwrap_or(""),
                        row["share"].as_str().unwrap_or("0%")
                    )
                })
                .collect::<Vec<_>>()
                .join("  ·  "),
        );
    }

    let columns = report["columns"].as_array().cloned().unwrap_or_default();
    if !columns.is_empty() {
        let values: Vec<Option<i64>> = columns
            .iter()
            .map(|column| column["worked"]["value"].as_i64())
            .collect();
        let max = values.iter().flatten().copied().max().unwrap_or(0);
        let glyphs = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let spark: String = values
            .iter()
            .map(|value| match (value, max) {
                (Some(value), max) if max > 0 => {
                    glyphs[(((*value as f64 / max as f64) * 7.0).round()
                        as usize)
                        .min(7)]
                }
                _ => '·',
            })
            .collect();
        let labels = columns
            .iter()
            .map(|column| column["caption"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(format!("{}  {}", styler.bold(&spark), styler.dim(&labels)));
    }

    let children = report["children"].as_array().cloned().unwrap_or_default();
    if !children.is_empty() {
        lines.push(String::new());
        for child in children {
            if child["state"].as_str() == Some("future") {
                lines.push(format!(
                    "{}  {}",
                    child["label"].as_str().unwrap_or(""),
                    child["range_label"].as_str().unwrap_or("")
                ));
                continue;
            }
            let label = child["label"].as_str().unwrap_or("");
            let worked = child["worked"]["display"].as_str().unwrap_or("–");
            let review = child["review"]["state"].as_str().unwrap_or("");
            let summary = child["review"]["headline"]
                .as_str()
                .or_else(|| child["mostly_display"].as_str())
                .unwrap_or("");
            let mark = match review {
                "reflected" => styler.green("✓"),
                "drafted" | "edited" => styler.yellow("·"),
                "empty" => styler.dim("–"),
                _ => child["closeout"].as_str().unwrap_or("").to_string(),
            };
            lines.push(format!("{mark} {label:<8} {worked:<10} {}", summary));
        }
    }
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::native::stats::value::DataQuality;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn point(
        date: NaiveDate,
        value: i64,
        parts: BTreeMap<String, BTreeMap<String, i64>>,
    ) -> SeriesPoint {
        SeriesPoint {
            date,
            end: date,
            value: Some(value),
            display: value.to_string(),
            quality: DataQuality::Ok,
            reason: None,
            provisional: false,
            rolling_median: None,
            typical: None,
            num: None,
            den: None,
            partial: None,
            parts,
            parts_display: BTreeMap::new(),
            apportioned: BTreeMap::new(),
            checklist_state: None,
            off_clock: None,
            clock_breakdown: None,
            parts_clock_breakdown: BTreeMap::new(),
        }
    }

    #[test]
    fn quarter_columns_cover_the_fourteenth_week_and_year_groups() {
        let quarter = PeriodId::Quarter {
            year: 2026,
            quarter: 4,
        };
        let quarter_span = period_bounds(quarter).unwrap();
        assert_eq!(weeks_between(quarter_span).len(), 14);
        assert_eq!(column_specs(quarter, quarter_span).len(), 14);

        let year = PeriodId::Year(2026);
        let year_span = period_bounds(year).unwrap();
        let columns = column_specs(year, year_span);
        assert_eq!(columns.len(), 53);
        assert_eq!(columns.first().unwrap().group.as_deref(), Some("Q1"));
        assert_eq!(columns.last().unwrap().group.as_deref(), Some("Q4"));
        assert_eq!(child_specs(year, year_span).len(), 4);
    }

    #[test]
    fn all_time_starts_on_the_earliest_canonical_daily_week() {
        let vault = tempfile::tempdir().unwrap();
        for path in [
            "2026/20260528.md",
            "2024/20240101.md",
            "2024/not-a-daily.md",
            "2024/20240101.txt",
        ] {
            let full = vault.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, "").unwrap();
        }
        assert_eq!(
            earliest_canonical_daily(vault.path()).unwrap(),
            Some(date(2024, 1, 1))
        );
        assert_eq!(
            period_span(PeriodId::AllTime, vault.path(), date(2026, 11, 3))
                .unwrap()
                .start,
            date(2024, 1, 1)
        );
    }

    #[test]
    fn future_report_has_rows_but_does_not_compute_or_create() {
        let vault = tempfile::tempdir().unwrap();
        let now = date(2026, 11, 3).and_hms_opt(10, 42, 0).unwrap();
        let id = PeriodId::Week {
            year: 2026,
            week: 48,
        };
        let report = build_report(
            vault.path(),
            vault.path().join("config.yml"),
            id,
            now,
            StatsClock::Worked,
        )
        .unwrap();
        assert_eq!(report["period"]["state"], "future");
        assert_eq!(report["facts"].as_array().unwrap().len(), 0);
        assert_eq!(report["columns"].as_array().unwrap().len(), 7);
        assert!(report["columns"]
            .as_array()
            .unwrap()
            .iter()
            .all(|column| column["state"] == "future"));
        assert!(!vault.path().join(id.path()).exists());
    }

    #[test]
    fn aggregation_sums_today_and_folds_small_period_types() {
        let today = date(2026, 11, 3);
        let parts = BTreeMap::from([
            (
                "root_parent".to_string(),
                BTreeMap::from([("bob".to_string(), 100)]),
            ),
            (
                "theme".to_string(),
                BTreeMap::from([
                    ("A".to_string(), 60),
                    ("B".to_string(), 1),
                    ("(unnamed)".to_string(), 39),
                ]),
            ),
        ]);
        let metric = MetricDays {
            points: BTreeMap::from([(today, point(today, 100, parts))]),
            unit: Some("minutes".to_string()),
            label: Some("Worked".to_string()),
        };
        let span = Span {
            start: today,
            end: today,
        };
        let aggregate = aggregate("worked", &metric, span, today);
        assert_eq!(aggregate.value, Some(100));
        assert!(aggregate.so_far);

        // Build lanes through a full daily metric so both lane totals derive
        // from the same worked aggregate.
        let folded = build_lanes(
            PeriodId::Quarter {
                year: 2026,
                quarter: 4,
            },
            span,
            today,
            &DailyMetrics {
                metrics: BTreeMap::from([(
                    "worked".to_string(),
                    metric.clone(),
                )]),
                homes: json!({}),
                theme_hues: json!({}),
                warnings: json!([]),
            },
            &json!({}),
            &json!({}),
        );
        assert_eq!(folded["theme"].as_array().unwrap().len(), 2);
        assert_eq!(folded["theme"][1]["key"], "other");
        assert_eq!(folded["theme"][1]["minutes"], 40);
        let week = build_lanes(
            PeriodId::Week {
                year: 2026,
                week: 45,
            },
            span,
            today,
            &DailyMetrics {
                metrics: BTreeMap::from([("worked".to_string(), metric)]),
                homes: json!({}),
                theme_hues: json!({}),
                warnings: json!([]),
            },
            &json!({}),
            &json!({}),
        );
        assert_eq!(week["theme"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn four_usable_prior_weeks_build_the_current_typical_sentence() {
        let today = date(2026, 11, 3);
        let mut points = BTreeMap::new();
        for offset in 0..28 {
            let day = date(2026, 10, 5) + chrono::Days::new(offset);
            points.insert(day, point(day, 1, BTreeMap::new()));
        }
        let metric = MetricDays {
            points,
            unit: Some("minutes".to_string()),
            label: Some("Worked".to_string()),
        };
        let typical = build_typical(
            PeriodId::Week {
                year: 2026,
                week: 45,
            },
            "worked",
            &metric,
            today,
            PeriodState::Current,
        )
        .unwrap();
        assert_eq!(typical.value, Some(7));
        assert_eq!(typical.words, "typical week 7m");
        assert!(typical.detail.contains("W41–W44"));
    }
}
