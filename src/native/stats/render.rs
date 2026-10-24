//! Human, JSON, and CSV renderers for `bob stats`.

use chrono::{Datelike, NaiveDate, Timelike, Weekday};

use super::report::{
    HealthReport, HomeCatalogEntry, JournalHostReport, MetricReport,
    PartSummary, SeriesPoint, StatsReport, StatsWarning,
};
use super::value::DataQuality;
use crate::native::plan_budget::normalize_component;
use crate::native::pomodoro_homes::{HomeKind, Hue};
use crate::native::style::{pad_right, terminal_width, Styler, SPARK_BLOCKS};

const NULL_MARK: char = '·';
const BOOL_MET: char = '●';
const BOOL_MISSED: char = '○';
const TODAY_MARK: char = '▯';
const CHART_ROWS: usize = 3;

pub(crate) fn json_line(report: &StatsReport) -> String {
    serde_json::to_string(report).unwrap_or_else(|_| {
        "{\"ok\":false,\"schema_version\":1,\"error\":{\"code\":\"encode\",\
         \"message\":\"failed to encode stats report\"}}"
            .to_string()
    })
}

pub(crate) fn csv(report: &StatsReport) -> String {
    let mut out = String::from(
        "date,end,metric,value,display,quality,provisional,num,den\n",
    );
    for metric in &report.metrics {
        for point in &metric.points {
            out.push_str(&csv_row(&metric.id, point));
        }
    }
    out
}

pub(crate) fn overview(report: &StatsReport, styler: &Styler) -> String {
    let mut out = String::new();
    out.push_str(&header(report, styler));
    out.push('\n');
    out.push_str(&overview_table(report, styler));
    if let Some(today) = today_line(report, styler) {
        out.push('\n');
        out.push_str(&today);
        out.push('\n');
    }
    if let Some(journal) = journal_line(&report.health, report.today, styler) {
        out.push_str(&journal);
        out.push('\n');
    }
    out.push_str(&warning_lines(report, styler));
    out
}

pub(crate) fn detail(report: &StatsReport, styler: &Styler) -> String {
    let mut out = String::new();
    for (index, metric) in report.metrics.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&detail_metric(report, metric, styler));
    }
    out.push_str(&warning_lines(report, styler));
    out
}

pub(crate) fn catalog(report: &StatsReport, styler: &Styler) -> String {
    let mut out = String::new();
    let id_w = col_width(report, |m| m.id.len(), 2);
    let label_w = col_width(report, |m| m.label.len(), 5);
    let kind_w = col_width(report, |m| m.kind.len(), 4);
    let unit_w = col_width(report, |m| m.unit.len(), 4);
    let source_w = col_width(report, |m| m.source.len(), 6);
    let origin_w = col_width(report, |m| m.origin.len(), 6);
    out.push_str(&format!(
        "{}  {}  {}  {}  {}  {}  since\n",
        pad_right("id", id_w),
        pad_right("label", label_w),
        pad_right("kind", kind_w),
        pad_right("unit", unit_w),
        pad_right("source", source_w),
        pad_right("origin", origin_w),
    ));
    for metric in &report.metrics {
        let since = metric
            .since
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "–".to_string());
        out.push_str(&format!(
            "{}  {}  {}  {}  {}  {}  {since}\n",
            pad_right(&metric.id, id_w),
            pad_right(&metric.label, label_w),
            pad_right(&metric.kind, kind_w),
            pad_right(&metric.unit, unit_w),
            pad_right(&metric.source, source_w),
            pad_right(&metric.origin, origin_w),
        ));
        if !metric.definition.is_empty() {
            out.push_str("  ");
            out.push_str(&styler.dim(&metric.definition));
            out.push('\n');
        }
    }
    out.push_str(&warning_lines(report, styler));
    out
}

pub(crate) fn catalog_json(report: &StatsReport) -> String {
    let metrics: Vec<serde_json::Value> = report
        .metrics
        .iter()
        .map(|metric| {
            serde_json::json!({
                "id": metric.id,
                "label": metric.label,
                "kind": metric.kind,
                "unit": metric.unit,
                "direction": metric.direction,
                "color": metric.color,
                "source": metric.source,
                "origin": metric.origin,
                "since": metric.since.map(|d| d.format("%Y-%m-%d").to_string()),
                "definition": metric.definition,
                "limits": metric.limits,
                "parameters": metric.parameters,
            })
        })
        .collect();
    serde_json::json!({
        "ok": true,
        "schema_version": report.schema_version,
        "metrics": metrics,
        "warnings": report.warnings,
    })
    .to_string()
}

fn col_width(
    report: &StatsReport,
    len: impl Fn(&MetricReport) -> usize,
    header: usize,
) -> usize {
    report
        .metrics
        .iter()
        .map(len)
        .max()
        .unwrap_or(0)
        .max(header)
}

fn csv_row(metric: &str, point: &SeriesPoint) -> String {
    let value = point
        .value
        .map(|value| value.to_string())
        .unwrap_or_default();
    let quality = quality_name(point.quality);
    let num = point.num.map(|n| n.to_string()).unwrap_or_default();
    let den = point.den.map(|n| n.to_string()).unwrap_or_default();
    format!(
        "{},{},{},{},{},{quality},{},{num},{den}\n",
        point.date,
        point.end,
        csv_field(metric),
        value,
        csv_field(&point.display),
        point.provisional,
    )
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn quality_name(quality: DataQuality) -> &'static str {
    match quality {
        DataQuality::Ok => "ok",
        DataQuality::Partial => "partial",
        DataQuality::Missing => "missing",
        DataQuality::NotTracked => "not_tracked",
    }
}

fn header(report: &StatsReport, styler: &Styler) -> String {
    let sep = styler.separator();
    let today =
        format!("{} {}", report.today.format("%a"), short_date(report.today));
    let mut parts = vec!["Stats".to_string(), today];
    let window = match report.range.preset.as_deref() {
        Some(preset) => {
            format!("{}, {}", preset_words(preset), report.range.label)
        }
        None => report.range.label.clone(),
    };
    parts.push(window);
    if let Some(prior) = &report.range.prior {
        parts.push(format!("vs {}", prior.label));
    }
    format!("{}\n", parts.join(&format!(" {sep} ")))
}

fn preset_words(preset: &str) -> String {
    if preset == "all" {
        return "all time".to_string();
    }
    let Some((count, unit)) =
        preset.split_at_checked(preset.len().saturating_sub(1))
    else {
        return format!("last {preset}");
    };
    match unit {
        "d" => format!("last {count} days"),
        "w" => format!("last {count} weeks"),
        "m" => format!("last {count} months"),
        "y" if count == "1" => "last year".to_string(),
        "y" => format!("last {count} years"),
        _ => format!("last {preset}"),
    }
}

fn short_date(date: NaiveDate) -> String {
    format!("{} {}", date.format("%b"), date.day())
}

fn overview_table(report: &StatsReport, styler: &Styler) -> String {
    let label_w = report
        .metrics
        .iter()
        .map(|metric| metric.label.len())
        .max()
        .unwrap_or(8)
        .max(8);
    let spark_budget = terminal_width().saturating_sub(label_w + 36).max(8);
    let mut out = String::new();
    out.push_str(&overview_axis(report, label_w, spark_budget, styler));
    for metric in &report.metrics {
        out.push_str(&overview_row(metric, label_w, spark_budget, styler));
    }
    out
}

fn overview_axis(
    report: &StatsReport,
    label_w: usize,
    spark_budget: usize,
    styler: &Styler,
) -> String {
    let start = short_date(report.range.start);
    let end = short_date(report.range.end);
    let spark_w = spark_len(report, spark_budget);
    let inner = spark_w.saturating_sub(start.len() + end.len() + 1);
    let bar = "─".repeat(inner.max(1));
    let span = format!("{start} {bar} {end}");
    format!(
        "{}  {}  yesterday   summary   vs prior\n",
        pad_right("", label_w),
        pad_right(&styler.dim(&span), spark_w.max(span.len())),
    )
}

fn spark_len(report: &StatsReport, budget: usize) -> usize {
    report
        .metrics
        .first()
        .map(|metric| spark_cells(metric, budget).chars().count())
        .unwrap_or(budget)
}

fn overview_row(
    metric: &MetricReport,
    label_w: usize,
    spark_budget: usize,
    styler: &Styler,
) -> String {
    let spark =
        color_spark(&spark_cells(metric, spark_budget), &metric.color, styler);
    let yesterday = yesterday_display(metric);
    let headline = headline_cell(metric);
    let delta = metric
        .summary
        .delta
        .as_ref()
        .map(|delta| delta.display.as_str())
        .unwrap_or("–");
    format!(
        "{}  {spark}  {:>10}  {:>11}  {delta}\n",
        pad_right(&metric.label, label_w),
        yesterday,
        headline,
    )
}

fn headline_cell(metric: &MetricReport) -> String {
    match metric.summary.headline.suffix.as_deref() {
        Some(suffix) if suffix.starts_with('/') => {
            format!("{}{suffix}", metric.summary.headline.display)
        }
        Some(suffix) => {
            format!("{} {suffix}", metric.summary.headline.display)
        }
        None => metric.summary.headline.display.clone(),
    }
}

fn yesterday_display(metric: &MetricReport) -> String {
    metric
        .points
        .iter()
        .rev()
        .find(|point| !point.provisional)
        .map(|point| point.display.clone())
        .unwrap_or_else(|| "–".to_string())
}

fn spark_cells(metric: &MetricReport, budget: usize) -> String {
    let points = downsample(&metric.points, budget);
    if metric.unit == "bool" {
        return points.iter().map(bool_mark).collect();
    }
    let values: Vec<Option<i64>> = points.iter().map(spark_value).collect();
    signed_sparkline(&values)
}

fn spark_value(point: &SeriesPoint) -> Option<i64> {
    match point.quality {
        DataQuality::Missing | DataQuality::NotTracked => None,
        _ => point.value,
    }
}

fn bool_mark(point: &SeriesPoint) -> char {
    match spark_value(point) {
        None => NULL_MARK,
        Some(value) if value > 0 => BOOL_MET,
        Some(_) => BOOL_MISSED,
    }
}

fn downsample(points: &[SeriesPoint], budget: usize) -> Vec<SeriesPoint> {
    if points.len() <= budget {
        return points.to_vec();
    }
    let mut weeks: Vec<(NaiveDate, Vec<&SeriesPoint>)> = Vec::new();
    for point in points {
        let week = point.date.week(Weekday::Mon).first_day();
        match weeks.last_mut() {
            Some((start, bucket)) if *start == week => bucket.push(point),
            _ => weeks.push((week, vec![point])),
        }
    }
    if weeks.len() > budget {
        let chunk = weeks.len().div_ceil(budget).max(1);
        return weeks
            .chunks(chunk)
            .map(|chunk| {
                collapse_points(
                    chunk.iter().flat_map(|(_, p)| p.iter().copied()),
                )
            })
            .collect();
    }
    weeks
        .into_iter()
        .map(|(_, bucket)| collapse_points(bucket))
        .collect()
}

fn collapse_points<'a>(
    points: impl IntoIterator<Item = &'a SeriesPoint>,
) -> SeriesPoint {
    let points: Vec<&SeriesPoint> = points.into_iter().collect();
    let first = points[0];
    let last = points[points.len() - 1];
    let values: Vec<i64> =
        points.iter().filter_map(|p| spark_value(p)).collect();
    let value = if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<i64>() / values.len() as i64)
    };
    SeriesPoint {
        date: first.date,
        end: last.end,
        value,
        display: first.display.clone(),
        quality: if value.is_some() {
            DataQuality::Ok
        } else {
            DataQuality::Missing
        },
        reason: None,
        provisional: points.iter().all(|p| p.provisional),
        rolling_median: None,
        num: None,
        den: None,
        partial: None,
        parts: Default::default(),
        parts_display: Default::default(),
        apportioned: Default::default(),
        checklist_state: None,
    }
}

fn signed_sparkline(values: &[Option<i64>]) -> String {
    let numbers: Vec<i64> = values.iter().copied().flatten().collect();
    if numbers.is_empty() {
        return values.iter().map(|_| NULL_MARK).collect();
    }
    let min = *numbers.iter().min().unwrap_or(&0);
    let max = *numbers.iter().max().unwrap_or(&0);
    let (low, high) = if min < 0 { (min, max.max(0)) } else { (0, max) };
    let span = (high - low).max(1);
    values
        .iter()
        .map(|value| match value {
            None => NULL_MARK,
            Some(value) => {
                let ratio = (*value - low) as f64 / span as f64;
                let index = (ratio * 7.0).round() as usize;
                SPARK_BLOCKS[index.min(7)]
            }
        })
        .collect()
}

fn color_spark(spark: &str, color: &str, styler: &Styler) -> String {
    spark
        .chars()
        .map(|ch| {
            if ch == NULL_MARK {
                styler.dim(&ch.to_string())
            } else {
                styler.metric_hue(color, &ch.to_string())
            }
        })
        .collect()
}

fn today_line(report: &StatsReport, styler: &Styler) -> Option<String> {
    if !report.range.includes_today {
        return None;
    }
    let sep = styler.separator();
    let mut parts = Vec::new();
    for metric in &report.metrics {
        let Some(point) = metric.points.iter().find(|p| p.provisional) else {
            continue;
        };
        parts.push(format!("{} {}", metric.label, point.display));
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "today so far {sep} {}",
        parts.join(&format!(" {sep} "))
    ))
}

fn journal_line(
    health: &HealthReport,
    today: NaiveDate,
    styler: &Styler,
) -> Option<String> {
    if health.journal.hosts.is_empty() {
        return None;
    }
    let sep = styler.separator();
    let hosts: Vec<String> = health
        .journal
        .hosts
        .iter()
        .map(|host| host_cell(host, today, styler))
        .collect();
    Some(format!("journal {sep} {}", hosts.join(&format!(" {sep} "))))
}

fn host_cell(
    host: &JournalHostReport,
    today: NaiveDate,
    styler: &Styler,
) -> String {
    let age = (today - host.last_day).num_days();
    if age <= 0 {
        let time =
            parse_hm(&host.last_ts).unwrap_or_else(|| "today".to_string());
        format!("{} {time} {}", host.host, styler.green("✓"))
    } else {
        format!("{} {age}d ago", host.host)
    }
}

fn parse_hm(ts: &str) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    Some(format!("{:02}:{:02}", parsed.hour(), parsed.minute()))
}

fn warning_lines(report: &StatsReport, styler: &Styler) -> String {
    let mut warnings: Vec<&StatsWarning> = report.warnings.iter().collect();
    for metric in &report.metrics {
        warnings.extend(metric.warnings.iter());
    }
    let mut out = String::new();
    for warning in warnings {
        out.push_str(&format!(
            "{} {}  {}\n",
            styler.warning_prefix(),
            warning.message,
            styler.dim(&warning.code),
        ));
    }
    out
}

fn detail_metric(
    report: &StatsReport,
    metric: &MetricReport,
    styler: &Styler,
) -> String {
    let mut out = String::new();
    out.push_str(&detail_header(report, metric, styler));
    out.push('\n');
    if metric.unit == "bool" {
        out.push_str(&bool_grid(&metric.points, styler));
    } else {
        out.push_str(&bar_chart(metric, styler));
    }
    out.push('\n');
    out.push_str(&detail_summary(metric, report, styler));
    out.push_str(&parts_lines(report, metric, styler));
    out
}

fn detail_header(
    report: &StatsReport,
    metric: &MetricReport,
    styler: &Styler,
) -> String {
    let sep = styler.separator();
    let unit = match metric.unit.as_str() {
        "minutes" => "minutes per day",
        "ratio" => "ratio per day",
        "bool" => "days met",
        "signed" => "signed count per day",
        _ => "count per day",
    };
    let mut window = report.range.label.clone();
    if report.range.includes_today {
        window.push_str(" + today");
    }
    let headline = headline_cell(metric);
    let delta = metric
        .summary
        .delta
        .as_ref()
        .map(|delta| {
            if metric.id == "net" {
                format!(" {}", delta.display)
            } else {
                format!(" {} vs prior", delta.display)
            }
        })
        .unwrap_or_default();
    format!(
        "{} {sep} {unit} {sep} {window}          {headline}{delta}\n",
        metric.label,
    )
}

fn bar_chart(metric: &MetricReport, styler: &Styler) -> String {
    let points = &metric.points;
    if points.is_empty() {
        return String::new();
    }
    let max = chart_max(points);
    let y_labels = y_ticks(max, &metric.unit);
    let label_w = y_labels
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(2);
    let gap = if points.len() <= 14 { " " } else { "" };
    let mut rows: Vec<String> = Vec::new();
    for (label, threshold) in &y_labels {
        let mut row = format!("{} ┤", pad_right(label, label_w));
        for (index, point) in points.iter().enumerate() {
            if index > 0 {
                row.push_str(gap);
            }
            row.push_str(&bar_cell(
                point,
                *threshold,
                max,
                &metric.color,
                styler,
            ));
        }
        rows.push(row);
    }
    let axis = format!(
        "{} ┼{}",
        pad_right("0", label_w),
        "─".repeat(chart_width(points.len(), gap)),
    );
    rows.push(axis);
    rows.push(x_axis(points, label_w, gap, styler));
    rows.join("\n") + "\n"
}

fn chart_width(n: usize, gap: &str) -> usize {
    if n == 0 {
        return 1;
    }
    n + gap.len() * n.saturating_sub(1)
}

fn chart_max(points: &[SeriesPoint]) -> i64 {
    let max = points
        .iter()
        .filter_map(spark_value)
        .map(|value| value.abs())
        .max()
        .unwrap_or(0);
    max.max(1)
}

fn y_ticks(max: i64, unit: &str) -> Vec<(String, i64)> {
    let top = nice_top(max, unit);
    let step = (top / CHART_ROWS as i64).max(1);
    (1..=CHART_ROWS)
        .rev()
        .map(|row| {
            let value = step * row as i64;
            (format_tick(value, unit), value)
        })
        .collect()
}

fn nice_top(max: i64, unit: &str) -> i64 {
    if unit == "minutes" {
        let hours = (max + 59) / 60;
        return (hours.max(1) * 60).max(60);
    }
    let mut step = 1;
    while step * (CHART_ROWS as i64) < max {
        step = match step {
            1 => 2,
            2 => 5,
            n if n % 10 == 0 => n * 2,
            n if n % 5 == 0 => n * 2,
            n => n * 5 / 2,
        };
        if step > 1_000_000 {
            break;
        }
    }
    step * CHART_ROWS as i64
}

fn format_tick(value: i64, unit: &str) -> String {
    match unit {
        "minutes" => {
            let hours = value / 60;
            if hours > 0 {
                format!("{hours}h")
            } else {
                format!("{value}m")
            }
        }
        "ratio" => format!("{}%", value / 100),
        _ => value.to_string(),
    }
}

fn bar_cell(
    point: &SeriesPoint,
    threshold: i64,
    max: i64,
    color: &str,
    styler: &Styler,
) -> String {
    if spark_value(point).is_none() {
        if threshold <= max / CHART_ROWS as i64 {
            return styler.dim(&NULL_MARK.to_string());
        }
        return " ".to_string();
    }
    if point.provisional {
        let value = point.value.unwrap_or(0).abs();
        if value >= threshold / 2
            && value < threshold + (max / CHART_ROWS as i64) / 2
            || (threshold == y_bottom(max) && value > 0 && value < threshold)
        {
            return styler.metric_hue(color, &TODAY_MARK.to_string());
        }
        if value >= threshold {
            return styler.metric_hue(color, &TODAY_MARK.to_string());
        }
        return " ".to_string();
    }
    let value = point.value.unwrap_or(0).abs();
    let row_span = (max / CHART_ROWS as i64).max(1);
    if value >= threshold {
        return styler.metric_hue(color, "█");
    }
    let prev = threshold.saturating_sub(row_span);
    if value > prev {
        let frac = (value - prev) as f64 / row_span as f64;
        let index = (frac * 7.0).round() as usize;
        let ch = SPARK_BLOCKS[index.min(7)].to_string();
        return styler.metric_hue(color, &ch);
    }
    " ".to_string()
}

fn y_bottom(max: i64) -> i64 {
    (max / CHART_ROWS as i64).max(1)
}

fn x_axis(
    points: &[SeriesPoint],
    label_w: usize,
    gap: &str,
    styler: &Styler,
) -> String {
    if points.is_empty() {
        return String::new();
    }
    let first = short_date(points[0].date);
    let last = short_date(points[points.len() - 1].date);
    let mid = points
        .get(points.len() / 2)
        .map(|point| short_date(point.date))
        .unwrap_or_default();
    let width = chart_width(points.len(), gap);
    let mut line = vec![' '; width.max(first.len() + last.len())];
    place(&mut line, 0, &first);
    if width > first.len() + last.len() + mid.len() + 2 {
        place(&mut line, (width.saturating_sub(mid.len())) / 2, &mid);
    }
    let last_at = width.saturating_sub(last.len());
    place(&mut line, last_at, &last);
    format!(
        "{}  {}",
        " ".repeat(label_w),
        styler.dim(&line.into_iter().collect::<String>()),
    )
}

fn place(line: &mut [char], at: usize, text: &str) {
    for (offset, ch) in text.chars().enumerate() {
        if let Some(cell) = line.get_mut(at + offset) {
            *cell = ch;
        }
    }
}

fn bool_grid(points: &[SeriesPoint], styler: &Styler) -> String {
    let mut weeks: Vec<(NaiveDate, [char; 7])> = Vec::new();
    for point in points {
        let start = point.date.week(Weekday::Mon).first_day();
        if weeks.last().map(|(week, _)| *week) != Some(start) {
            weeks.push((start, [' '; 7]));
        }
        let index = point.date.weekday().num_days_from_monday() as usize;
        if let Some((_, row)) = weeks.last_mut() {
            row[index] = bool_mark(point);
        }
    }
    let mut out =
        format!("     {}\n", styler.dim("Mon Tue Wed Thu Fri Sat Sun"),);
    for (start, row) in weeks {
        let week = start.iso_week().week();
        let cells: String = row
            .iter()
            .map(|ch| {
                let glyph = if *ch == ' ' { ' ' } else { *ch };
                format!(" {glyph}  ")
            })
            .collect();
        out.push_str(&format!("W{week:<3}{cells}\n"));
    }
    out
}

fn detail_summary(
    metric: &MetricReport,
    report: &StatsReport,
    styler: &Styler,
) -> String {
    let sep = styler.separator();
    let mut bits = Vec::new();
    if let (Some(p25), Some(p75)) = (metric.summary.p25, metric.summary.p75) {
        bits.push(format!(
            "normal  {} – {} (p25–p75)",
            format_unit(p25, &metric.unit),
            format_unit(p75, &metric.unit),
        ));
    }
    if let Some(total) = &metric.summary.total {
        bits.push(format!(
            "total {} over {} days",
            total.display, report.range.days,
        ));
    }
    bits.push(format!(
        "{} of {} days with data",
        metric.summary.days_with_data, report.range.days,
    ));
    format!(" {}\n", bits.join(&format!(" {sep} ")))
}

fn format_unit(value: i64, unit: &str) -> String {
    match unit {
        "minutes" => {
            let hours = value / 60;
            let minutes = value % 60;
            if hours > 0 {
                format!("{hours}h {minutes:02}m")
            } else {
                format!("{minutes}m")
            }
        }
        "ratio" => format!("{}%", value / 100),
        _ => value.to_string(),
    }
}

fn parts_lines(
    report: &StatsReport,
    metric: &MetricReport,
    styler: &Styler,
) -> String {
    let order: &[&str] = match metric.kind.as_str() {
        "pomodoro" => &["area", "project", "theme"],
        "tasks" => &["kind", "area", "project", "note"],
        _ => return legacy_parts_lines(metric, styler),
    };
    let mut out = String::new();
    let sep = styler.separator();
    for name in order {
        let Some(parts) = metric.parts_summary.get(*name) else {
            continue;
        };
        let displayed = if *name == "project" {
            collapse_project_cli(parts)
        } else {
            parts.clone()
        };
        if displayed.is_empty() {
            continue;
        }
        let cells: Vec<String> = displayed
            .iter()
            .map(|part| {
                format_part_cell(
                    part,
                    name,
                    &report.homes,
                    &report.theme_hues,
                    styler,
                )
            })
            .collect();
        out.push_str(&format!(
            " {:<8} {}\n",
            name,
            cells.join(&format!(" {sep} ")),
        ));
    }
    if metric
        .parts_summary
        .get("area")
        .is_some_and(|parts| !parts.is_empty())
        || metric
            .parts_summary
            .get("project")
            .is_some_and(|parts| !parts.is_empty())
    {
        out.push_str(&format!(
            " {:<8} {}\n",
            "",
            styler.dim("grouped by today's note tree · from Task Links"),
        ));
    }
    if metric.unit == "minutes" {
        if let Some(project) = metric.parts_summary.get("project") {
            let total: i64 = project.iter().map(|part| part.value).sum();
            let split: i64 =
                project.iter().filter_map(|part| part.apportioned).sum();
            if split > 0 && total > 0 {
                let percent = ((i128::from(split) * 100)
                    + i128::from(total) / 2)
                    / i128::from(total);
                let line = format!(
                    "{percent}% of project minutes came from sessions with several homes"
                );
                out.push_str(&format!(
                    " {:<8} {}\n",
                    "split",
                    styler.dim(&line),
                ));
            }
        }
    }
    out
}

fn legacy_parts_lines(metric: &MetricReport, styler: &Styler) -> String {
    let mut out = String::new();
    let sep = styler.separator();
    for (name, parts) in &metric.parts_summary {
        if parts.is_empty() {
            continue;
        }
        let cells: Vec<String> = parts
            .iter()
            .map(|part| format!("{} {} {}", part.key, part.display, part.share))
            .collect();
        out.push_str(&format!(
            " {:<8} {}\n",
            name,
            cells.join(&format!(" {sep} ")),
        ));
    }
    out
}

fn collapse_project_cli(parts: &[PartSummary]) -> Vec<PartSummary> {
    let mut homes = Vec::new();
    let mut buckets = Vec::new();
    for part in parts {
        if is_bucket_key(&part.key) {
            buckets.push(part.clone());
        } else {
            homes.push(part.clone());
        }
    }
    let overflow = if homes.len() > 5 {
        homes.split_off(5)
    } else {
        Vec::new()
    };
    if !overflow.is_empty() {
        let value = overflow
            .iter()
            .fold(0_i64, |sum, part| sum.saturating_add(part.value));
        let apportioned = overflow
            .iter()
            .filter_map(|part| part.apportioned)
            .fold(0_i64, i64::saturating_add);
        let all_total = parts.iter().fold(0_i64, |sum, part| {
            sum.saturating_add(part.value.saturating_abs())
        });
        let share = if all_total == 0 {
            0
        } else {
            ((i128::from(value.saturating_abs()) * 100)
                + i128::from(all_total) / 2)
                / i128::from(all_total)
        };
        homes.push(PartSummary {
            key: "other".to_string(),
            value,
            display: overflow
                .first()
                .map(|part| {
                    if part.display.ends_with('m') {
                        format_unit(value, "minutes")
                    } else {
                        value.to_string()
                    }
                })
                .unwrap_or_else(|| value.to_string()),
            share: format!("{share}%"),
            label: None,
            apportioned: (apportioned > 0).then_some(apportioned),
            apportioned_display: None,
        });
    }
    homes.extend(buckets);
    homes
}

fn is_bucket_key(key: &str) -> bool {
    key.starts_with('@')
}

fn format_part_cell(
    part: &PartSummary,
    dimension: &str,
    catalog: &std::collections::BTreeMap<String, HomeCatalogEntry>,
    theme_hues: &std::collections::BTreeMap<
        String,
        crate::native::pomodoro_themes::ThemeRef,
    >,
    styler: &Styler,
) -> String {
    let label = part.label.as_deref().unwrap_or(&part.key);
    let body = format!("{label} {} {}", part.display, part.share);
    if dimension == "theme" {
        if !styler.is_color() {
            return body;
        }
        let key = normalize_component(&part.key);
        let Some(theme) = theme_hues.get(&key).filter(|theme| theme.known)
        else {
            return body;
        };
        return match theme.hue {
            Some(hue) => format!(
                "{} {body}",
                styler.paint(&format!("38;5;{}", hue.xterm()), "■")
            ),
            None => format!("{} {body}", styler.dim("□")),
        };
    }
    if dimension != "area" && dimension != "project" {
        return body;
    }
    let entry = catalog.get(&part.key);
    let bucket = part.key == "other"
        || entry.is_some_and(|entry| entry.kind == HomeKind::Bucket)
        || is_bucket_key(&part.key);
    if !styler.is_color() {
        return body;
    }
    if bucket {
        if part.key.starts_with("@review:")
            && let Some(hue) = entry.and_then(|entry| entry.hue)
        {
            return format!("{} {body}", hue_dot(styler, hue));
        }
        return styler.dim(&body);
    }
    let hue = entry.and_then(|entry| entry.hue);
    match hue {
        Some(hue) => format!("{} {body}", hue_dot(styler, hue)),
        None => format!("● {body}"),
    }
}

fn hue_dot(styler: &Styler, hue: Hue) -> String {
    styler.paint(&format!("38;5;{}", hue.xterm()), "●")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::stats::report::{
        GroupName, Headline, JournalHealthReport, MetricSummary, RangeReport,
        TaskHealthReport,
    };
    use crate::native::style::{sparkline, Styler};
    use std::collections::BTreeMap;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn point(day: u32, value: Option<i64>, display: &str) -> SeriesPoint {
        SeriesPoint {
            date: date(day),
            end: date(day),
            value,
            display: display.to_string(),
            quality: if value.is_some() {
                DataQuality::Ok
            } else {
                DataQuality::Missing
            },
            reason: None,
            provisional: false,
            rolling_median: None,
            num: None,
            den: None,
            partial: None,
            parts: BTreeMap::new(),
            parts_display: BTreeMap::new(),
            apportioned: BTreeMap::new(),
            checklist_state: None,
        }
    }

    fn report(points: Vec<SeriesPoint>) -> StatsReport {
        StatsReport {
            ok: true,
            schema_version: 1,
            today: date(11),
            range: RangeReport {
                preset: Some("7d".to_string()),
                offset: 0,
                start: date(8),
                end: date(10),
                days: 3,
                includes_today: true,
                group: GroupName::Day,
                label: "Oct 8 – Oct 10".to_string(),
                prior: None,
            },
            metrics: vec![MetricReport {
                id: "worked".to_string(),
                label: "Worked".to_string(),
                kind: "pomodoro".to_string(),
                unit: "minutes".to_string(),
                direction: "up".to_string(),
                color: "orange".to_string(),
                source: "derived".to_string(),
                origin: "builtin".to_string(),
                since: Some(date(9)),
                definition: "Minutes in closed Pomodoros.".to_string(),
                limits: Vec::new(),
                parameters: None,
                summary: MetricSummary {
                    headline: Headline {
                        value: Some(32),
                        display: "32m".to_string(),
                        suffix: Some("/day".to_string()),
                        stat: Some("median".to_string()),
                    },
                    total: None,
                    mean: Some(32),
                    p25: Some(0),
                    p75: Some(32),
                    days_with_data: 2,
                    days_met: None,
                    prior: None,
                    delta: None,
                },
                chart: crate::native::stats::report::ChartBand {
                    group: GroupName::Day,
                    p25: Some(0),
                    p75: Some(32),
                },
                points,
                prior_points: Vec::new(),
                daily_points: Vec::new(),
                parts_summary: BTreeMap::new(),
                chores: Vec::new(),
                warnings: Vec::new(),
            }],
            health: HealthReport {
                journal: JournalHealthReport::default(),
                tasks: TaskHealthReport::default(),
                homes: Default::default(),
            },
            warnings: Vec::new(),
            homes: BTreeMap::new(),
            theme_hues: BTreeMap::new(),
            sessions: Vec::new(),
        }
    }

    #[test]
    fn theme_dimension_uses_theme_squares_and_respects_no_color() {
        let part = PartSummary {
            key: "GTD".to_string(),
            value: 25,
            display: "25m".to_string(),
            share: "100%".to_string(),
            label: None,
            apportioned: None,
            apportioned_display: None,
        };
        let refs = BTreeMap::from([(
            "gtd".to_string(),
            crate::native::pomodoro_themes::ThemeRef {
                key: "gtd".to_string(),
                name: "GTD".to_string(),
                hue: None,
                hue_source: crate::native::pomodoro_themes::HueSource::Exempt,
                known: true,
                start: None,
                end: None,
            },
        )]);
        let homes = BTreeMap::new();
        assert_eq!(
            format_part_cell(&part, "theme", &homes, &refs, &Styler::plain(),),
            "GTD 25m 100%"
        );
        assert!(format_part_cell(
            &part,
            "theme",
            &homes,
            &refs,
            &Styler::colored(),
        )
        .contains("□"));
    }

    #[test]
    fn sparkline_uses_null_mark_and_blocks() {
        let cells = signed_sparkline(&[None, Some(0), Some(10)]);
        assert!(cells.starts_with(NULL_MARK));
        assert_eq!(cells.chars().count(), 3);
        assert_eq!(cells.chars().last(), Some('█'));
        let _ = sparkline(&[0, 1, 4]);
    }

    #[test]
    fn csv_has_tidy_header_and_empty_null_value() {
        let report =
            report(vec![point(9, Some(32), "32m"), point(10, None, "–")]);
        let text = csv(&report);
        assert!(text.starts_with(
            "date,end,metric,value,display,quality,provisional,num,den\n"
        ));
        assert!(text.contains("2026-10-09,2026-10-09,worked,32,32m,ok,false,,"));
        assert!(
            text.contains("2026-10-10,2026-10-10,worked,,–,missing,false,,")
        );
    }

    #[test]
    fn overview_plain_contains_labels_and_today() {
        let mut today = point(11, Some(10), "10m");
        today.provisional = true;
        let report = report(vec![point(9, Some(32), "32m"), today]);
        let text = overview(&report, &Styler::plain());
        assert!(text.contains("Stats"));
        assert!(text.contains("Worked"));
        assert!(text.contains("today so far"));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn overview_colored_paints_sparkline() {
        let report = report(vec![point(9, Some(32), "32m")]);
        let text = overview(&report, &Styler::colored());
        assert!(text.contains("\u{1b}[38;5;208m"));
    }

    #[test]
    fn json_is_one_line() {
        let report = report(vec![point(9, Some(32), "32m")]);
        let text = json_line(&report);
        assert!(!text.contains('\n'));
        assert!(text.starts_with('{'));
        assert!(text.contains("\"ok\":true"));
    }

    #[test]
    fn detail_prints_every_pomodoro_parts_dimension() {
        let mut report = report(vec![point(9, Some(32), "32m")]);
        report.metrics[0].parts_summary.insert(
            "area".to_string(),
            vec![PartSummary {
                key: "gtd.md".to_string(),
                value: 25,
                display: "25m".to_string(),
                share: "78%".to_string(),
                label: Some("gtd".to_string()),
                apportioned: None,
                apportioned_display: None,
            }],
        );
        report.metrics[0].parts_summary.insert(
            "project".to_string(),
            vec![PartSummary {
                key: "@review:GTD".to_string(),
                value: 25,
                display: "25m".to_string(),
                share: "78%".to_string(),
                label: Some("GTD review".to_string()),
                apportioned: None,
                apportioned_display: None,
            }],
        );
        report.homes.insert(
            "@review:GTD".to_string(),
            HomeCatalogEntry {
                label: "GTD review".to_string(),
                kind: HomeKind::Bucket,
                area: Some("gtd.md".to_string()),
                subarea: None,
                path: None,
                hue: Some(Hue::Orange),
                project_label: None,
                rolled_up: None,
            },
        );
        report.metrics[0].parts_summary.insert(
            "theme".to_string(),
            vec![PartSummary {
                key: "GTD".to_string(),
                value: 25,
                display: "25m".to_string(),
                share: "78%".to_string(),
                label: None,
                apportioned: None,
                apportioned_display: None,
            }],
        );
        let text = detail(&report, &Styler::plain());
        assert!(
            text.contains("area") && text.contains("gtd 25m 78%"),
            "{text:?}"
        );
        assert!(
            text.contains("project") && text.contains("GTD review 25m 78%"),
            "{text:?}"
        );
        assert!(
            text.contains("theme") && text.contains("GTD 25m 78%"),
            "{text:?}"
        );
        assert!(text.contains("grouped by today's note tree · from Task Links"));
        let area_idx = text.find("area").expect("area dimension");
        let project_idx = text.find("project").expect("project dimension");
        let theme_idx = text.find("theme").expect("theme dimension");
        assert!(area_idx < project_idx && project_idx < theme_idx);
        assert!(!text.contains("●"));
        let colored = detail(&report, &Styler::colored());
        assert!(colored.contains("\u{1b}[38;5;208m●"), "{colored:?}");
    }

    #[test]
    fn overview_header_says_summary_and_total_suffix_has_space() {
        let base = report(vec![point(9, Some(32), "32m")]);
        let text = overview(&base, &Styler::plain());
        assert!(text.contains("summary"), "{text:?}");
        assert!(!text.contains("median/day"), "{text:?}");

        let mut total_report = report(vec![point(9, Some(-89), "−89")]);
        total_report.metrics[0].summary.headline.display = "−89".to_string();
        total_report.metrics[0].summary.headline.suffix =
            Some("total".to_string());
        let total_text = overview(&total_report, &Styler::plain());
        assert!(total_text.contains("−89 total"), "{total_text:?}");
        assert!(!total_text.contains("−89total"), "{total_text:?}");

        let slash_text = overview(&base, &Styler::plain());
        assert!(slash_text.contains("32m/day"), "{slash_text:?}");
    }

    #[test]
    fn net_detail_header_drops_vs_prior() {
        use crate::native::stats::report::Delta;
        let mut net_report = report(vec![point(9, Some(-1), "−1")]);
        net_report.metrics[0].id = "net".to_string();
        net_report.metrics[0].label = "Net flow".to_string();
        net_report.metrics[0].summary.headline.display = "−89".to_string();
        net_report.metrics[0].summary.headline.suffix =
            Some("total".to_string());
        net_report.metrics[0].summary.delta = Some(Delta {
            display: "shrinking".to_string(),
            words: "net flow is shrinking".to_string(),
            sign: -1,
        });
        let net_text = detail(&net_report, &Styler::plain());
        assert!(net_text.contains("−89 total shrinking"), "{net_text:?}");
        assert!(!net_text.contains("vs prior"), "{net_text:?}");

        let mut other_report = report(vec![point(9, Some(32), "32m")]);
        other_report.metrics[0].summary.delta = Some(Delta {
            display: "▲ 6%".to_string(),
            words: "up 6% vs the prior 7 days".to_string(),
            sign: 1,
        });
        let other_text = detail(&other_report, &Styler::plain());
        assert!(other_text.contains("vs prior"), "{other_text:?}");
    }

    #[test]
    fn detail_prints_every_task_parts_dimension() {
        let mut report = report(vec![point(9, Some(7), "7")]);
        report.metrics[0].id = "done".to_string();
        report.metrics[0].label = "Done".to_string();
        report.metrics[0].kind = "tasks".to_string();
        report.metrics[0].unit = "count".to_string();
        report.metrics[0].parts_summary.insert(
            "kind".to_string(),
            vec![PartSummary {
                key: "task".to_string(),
                value: 5,
                display: "5".to_string(),
                share: "71%".to_string(),
                label: None,
                apportioned: None,
                apportioned_display: None,
            }],
        );
        report.metrics[0].parts_summary.insert(
            "area".to_string(),
            vec![PartSummary {
                key: "dev.md".to_string(),
                value: 4,
                display: "4".to_string(),
                share: "57%".to_string(),
                label: Some("dev".to_string()),
                apportioned: None,
                apportioned_display: None,
            }],
        );
        report.metrics[0].parts_summary.insert(
            "project".to_string(),
            vec![PartSummary {
                key: "sase.md".to_string(),
                value: 3,
                display: "3".to_string(),
                share: "43%".to_string(),
                label: Some("sase".to_string()),
                apportioned: None,
                apportioned_display: None,
            }],
        );
        report.metrics[0].parts_summary.insert(
            "note".to_string(),
            vec![PartSummary {
                key: "sase.md".to_string(),
                value: 3,
                display: "3".to_string(),
                share: "43%".to_string(),
                label: None,
                apportioned: None,
                apportioned_display: None,
            }],
        );
        let text = detail(&report, &Styler::plain());
        assert!(
            text.contains("kind") && text.contains("task 5 71%"),
            "{text:?}"
        );
        assert!(
            text.contains("area") && text.contains("dev 4 57%"),
            "{text:?}"
        );
        assert!(
            text.contains("project") && text.contains("sase 3 43%"),
            "{text:?}"
        );
        assert!(
            text.contains("note") && text.contains("sase.md 3 43%"),
            "{text:?}"
        );
        let kind_idx = text.find("kind").expect("kind dimension");
        let area_idx = text.find("area").expect("area dimension");
        let project_idx = text.find("project").expect("project dimension");
        let note_idx = text.find("note").expect("note dimension");
        assert!(
            kind_idx < area_idx
                && area_idx < project_idx
                && project_idx < note_idx
        );
    }
}
