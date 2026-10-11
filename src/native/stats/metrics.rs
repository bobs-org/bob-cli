//! Read-only evaluators for metrics derived from the shared vault index.
//!
//! Each evaluator is pure: all vault parsing and I/O happen in `index`.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, NaiveDate};
use regex::Regex;

use super::{
    index::{
        DailyNoteInputs, DateDiagnostic, DatedValue, IndexedTask, StatsIndex,
        TaskKindPart,
    },
    registry::{
        ChecklistMeasure, FieldSource, MetricDef, MetricSpec, MetricUnit,
        PomodoroMeasure, RecurringFilter, TaskEvent, TaskKindFilter,
    },
    report::StatsWarning,
    value::{DailyValue, DataQuality},
};

/// Evaluated daily facts for a single non-journal, non-formula metric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetricEvaluation {
    /// Effective tracking start after explicit and global `since` rules.
    pub(crate) since: Option<NaiveDate>,
    pub(crate) values: Vec<DailyValue>,
    /// Metric-specific limitations that do not belong to one daily point.
    pub(crate) warnings: Vec<StatsWarning>,
}

/// Evaluate a Pomodoro, task, checklist, or field metric over the requested
/// dates. Journal and formula metrics are evaluated by their own phases.
pub(crate) fn evaluate_metric(
    metric: &MetricDef,
    index: &StatsIndex,
    dates: &[NaiveDate],
    today: NaiveDate,
    global_since: Option<NaiveDate>,
) -> Option<MetricEvaluation> {
    let (inferred_since, evaluate, warnings) = match &metric.spec {
        MetricSpec::Pomodoro { .. } => {
            let since = pomodoro_since(index);
            let values = evaluate_pomodoros(metric, index, dates, today);
            (since, values, Vec::new())
        }
        MetricSpec::Tasks { .. } => {
            let since = task_since(metric, index);
            let effective_since = effective_since(metric, since, global_since);
            let values =
                evaluate_tasks(metric, index, dates, today, effective_since);
            (since, values, Vec::new())
        }
        MetricSpec::Checklist { .. } => {
            let result = evaluate_checklist(metric, index, dates, today);
            (result.since, result.values, result.warnings)
        }
        MetricSpec::Field { .. } => {
            let since = field_since(metric, index);
            let values = evaluate_fields(metric, index, dates, today, since);
            (since, values, Vec::new())
        }
        MetricSpec::Formula { .. } | MetricSpec::Journal { .. } => {
            return None;
        }
    };

    // An explicit metric start replaces inferred history. The global start
    // is a floor applied after that choice.
    let since = effective_since(metric, inferred_since, global_since);
    let values = evaluate
        .into_iter()
        .map(|mut value| {
            if since.is_some_and(|start| value.date < start) {
                value.value = None;
                value.num = None;
                value.den = None;
                value.quality = DataQuality::NotTracked;
                value.reason = Some("before tracking began".to_string());
                value.parts.clear();
            }
            value
        })
        .collect();

    Some(MetricEvaluation {
        since,
        values,
        warnings,
    })
}

pub(crate) fn effective_since(
    metric: &MetricDef,
    inferred_since: Option<NaiveDate>,
    global_since: Option<NaiveDate>,
) -> Option<NaiveDate> {
    let metric_since = metric.since.or(inferred_since);
    global_since
        .map(|floor| metric_since.map_or(floor, |value| value.max(floor)))
        .or(metric_since)
}

fn pomodoro_since(index: &StatsIndex) -> Option<NaiveDate> {
    index
        .daily_notes
        .iter()
        .filter(|(_, note)| note.has_pomodoros_heading)
        .map(|(date, _)| *date)
        .min()
}

#[derive(Default)]
struct PomodoroDay {
    total: i64,
    sessions: i64,
    parts: BTreeMap<String, i64>,
    used_fallback: bool,
    missing_duration: bool,
}

fn evaluate_pomodoros(
    metric: &MetricDef,
    index: &StatsIndex,
    dates: &[NaiveDate],
    today: NaiveDate,
) -> Vec<DailyValue> {
    let MetricSpec::Pomodoro {
        measure,
        themes,
        exclude_themes,
    } = &metric.spec
    else {
        return Vec::new();
    };
    let mut per_day = BTreeMap::<NaiveDate, PomodoroDay>::new();
    for (date, note) in &index.daily_notes {
        if !note.has_pomodoros_heading {
            continue;
        }
        for session in &note.closed_pomodoros {
            let components = pomodoro_components(session.name.as_deref());
            let selected = components
                .iter()
                .filter(|component| {
                    theme_matches(component, themes, exclude_themes)
                })
                .cloned()
                .collect::<Vec<_>>();
            if selected.is_empty() {
                continue;
            }

            let day = per_day.entry(*date).or_default();
            match measure {
                PomodoroMeasure::Count => {
                    day.sessions = day.sessions.saturating_add(1);
                    // A session belongs to one part in count mode, so part
                    // totals continue to add up to the metric total.
                    add_part(&mut day.parts, selected[0].clone(), 1);
                }
                PomodoroMeasure::Minutes => {
                    let (minutes, fallback) = match session.recorded_minutes {
                        Some(minutes) => (Some(i64::from(minutes)), false),
                        None => (
                            session.range_minutes.map(i64::from),
                            session.range_minutes.is_some(),
                        ),
                    };
                    let Some(minutes) = minutes else {
                        day.missing_duration = true;
                        continue;
                    };
                    day.sessions = day.sessions.saturating_add(1);
                    day.used_fallback |= fallback;
                    let count =
                        i64::try_from(components.len()).unwrap_or(1).max(1);
                    let each = minutes / count;
                    let remainder = minutes % count;
                    for (position, component) in components.iter().enumerate() {
                        if !theme_matches(component, themes, exclude_themes) {
                            continue;
                        }
                        let share = each + i64::from(position == 0) * remainder;
                        day.total = day.total.saturating_add(share);
                        add_part(&mut day.parts, component.clone(), share);
                    }
                }
            }
        }
    }

    dates
        .iter()
        .map(|date| {
            let provisional = *date == today;
            let Some(note) = index.daily_notes.get(date) else {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::Missing,
                    Some("no daily note".to_string()),
                    provisional,
                    BTreeMap::new(),
                );
            };
            if !note.has_pomodoros_heading {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::Missing,
                    Some("Pomodoros section missing".to_string()),
                    provisional,
                    BTreeMap::new(),
                );
            }

            let day = per_day.get(date);
            let total = day.map_or(0, |day| match measure {
                PomodoroMeasure::Minutes => day.total,
                PomodoroMeasure::Count => day.sessions,
            });
            let quality = if day.is_some_and(|day| {
                day.used_fallback || day.missing_duration
            }) {
                DataQuality::Partial
            } else {
                DataQuality::Ok
            };
            let reason = match day {
                Some(day) if day.missing_duration && day.used_fallback => {
                    Some("some Pomodoro durations are unavailable; others use range estimates".to_string())
                }
                Some(day) if day.missing_duration => {
                    Some("one or more Pomodoro durations are unavailable".to_string())
                }
                Some(day) if day.used_fallback => {
                    Some("minutes estimated from Pomodoro time ranges".to_string())
                }
                _ => None,
            };
            let value = if matches!(measure, PomodoroMeasure::Minutes)
                && day.is_some_and(|day| day.missing_duration && day.sessions == 0)
            {
                None
            } else {
                Some(total)
            };
            let parts = day.map_or_else(BTreeMap::new, |day| {
                BTreeMap::from([("theme".to_string(), day.parts.clone())])
            });
            daily(
                *date,
                value,
                None,
                None,
                quality,
                reason,
                provisional,
                parts,
            )
        })
        .collect()
}

fn pomodoro_components(name: Option<&str>) -> Vec<String> {
    let components = name
        .map(crate::native::plan_budget::split_components)
        .unwrap_or_default()
        .into_iter()
        .map(|component| component.to_ascii_uppercase())
        .collect::<Vec<_>>();
    if components.is_empty() {
        vec!["(unnamed)".to_string()]
    } else {
        components
    }
}

fn theme_matches(
    component: &str,
    themes: &[String],
    exclude: &[String],
) -> bool {
    let has_included = themes.is_empty()
        || themes.iter().any(|theme| {
            component.eq_ignore_ascii_case(theme.trim().trim_start_matches('#'))
        });
    let excluded = exclude.iter().any(|theme| {
        component.eq_ignore_ascii_case(theme.trim().trim_start_matches('#'))
    });
    has_included && !excluded
}

fn task_since(metric: &MetricDef, index: &StatsIndex) -> Option<NaiveDate> {
    let MetricSpec::Tasks { event, .. } = &metric.spec else {
        return None;
    };
    index
        .tasks
        .iter()
        .filter(|task| task_matches(metric, index, task))
        .filter_map(|task| task_event_date(task, *event))
        .filter(|date| date.diagnostic == DateDiagnostic::Ok)
        .filter_map(|date| date.date)
        .min()
}

fn evaluate_tasks(
    metric: &MetricDef,
    index: &StatsIndex,
    dates: &[NaiveDate],
    today: NaiveDate,
    since: Option<NaiveDate>,
) -> Vec<DailyValue> {
    let MetricSpec::Tasks { event, .. } = &metric.spec else {
        return Vec::new();
    };
    let mut counts = BTreeMap::<NaiveDate, i64>::new();
    let mut parts =
        BTreeMap::<NaiveDate, BTreeMap<String, BTreeMap<String, i64>>>::new();
    for task in &index.tasks {
        if !task_matches(metric, index, task) {
            continue;
        }
        let Some(date) = task_event_date(task, *event) else {
            continue;
        };
        if date.diagnostic != DateDiagnostic::Ok {
            continue;
        }
        let Some(date) = date.date else {
            continue;
        };
        *counts.entry(date).or_default() += 1;
        let parts_for_day = parts.entry(date).or_default();
        let kind = match task.task_kind {
            TaskKindPart::Task => "task",
            TaskKindPart::Prj => "prj",
            TaskKindPart::Ref => "ref",
        };
        add_dimension_part(parts_for_day, "kind", kind, 1);
        add_dimension_part(parts_for_day, "note", &task.source_path, 1);
    }

    dates
        .iter()
        .map(|date| {
            let provisional = *date == today;
            if since.is_some_and(|start| *date < start) {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::NotTracked,
                    Some("before tracking began".to_string()),
                    provisional,
                    BTreeMap::new(),
                );
            }
            if since.is_none() {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::Missing,
                    Some(
                        "no qualifying task event has been recorded"
                            .to_string(),
                    ),
                    provisional,
                    BTreeMap::new(),
                );
            }
            daily(
                *date,
                Some(*counts.get(date).unwrap_or(&0)),
                None,
                None,
                DataQuality::Ok,
                None,
                provisional,
                parts.get(date).cloned().unwrap_or_default(),
            )
        })
        .collect()
}

fn task_event_date(
    task: &IndexedTask,
    event: TaskEvent,
) -> Option<&DatedValue> {
    match event {
        TaskEvent::Created => Some(&task.created),
        TaskEvent::Done if task.status_type == "DONE" => Some(&task.done),
        TaskEvent::Dropped if task.status_type == "CANCELLED" => {
            Some(&task.cancelled)
        }
        TaskEvent::Done | TaskEvent::Dropped => None,
    }
}

fn task_matches(
    metric: &MetricDef,
    index: &StatsIndex,
    task: &IndexedTask,
) -> bool {
    let MetricSpec::Tasks {
        tags_any,
        tags_all,
        in_notes,
        task_kind,
        recurring,
        ..
    } = &metric.spec
    else {
        return false;
    };
    if is_daily_gtd_task(task)
        || match recurring {
            RecurringFilter::Exclude => task.recurring,
            RecurringFilter::Only => !task.recurring,
            RecurringFilter::Include => false,
        }
    {
        return false;
    }
    if task_kind.is_some_and(|filter| !kind_matches(filter, task.task_kind)) {
        return false;
    }
    let tags = task
        .tags
        .iter()
        .map(|tag| tag.trim_start_matches('#').to_lowercase())
        .collect::<BTreeSet<_>>();
    if !tags_any.is_empty()
        && !tags_any.iter().any(|tag| {
            tags.contains(&tag.trim().trim_start_matches('#').to_lowercase())
        })
    {
        return false;
    }
    if !tags_all.iter().all(|tag| {
        tags.contains(&tag.trim().trim_start_matches('#').to_lowercase())
    }) {
        return false;
    }
    if in_notes.is_empty() {
        return true;
    }
    let mut paths = vec![task.source_path.as_str()];
    let ancestors = index.parent_ancestors(&task.source_path);
    paths.extend(ancestors.iter().map(String::as_str));
    in_notes.iter().any(|target| {
        paths.iter().any(|path| note_target_matches(target, path))
    })
}

fn is_daily_gtd_task(task: &IndexedTask) -> bool {
    if !task
        .block_id
        .as_deref()
        .is_some_and(|id| id.eq_ignore_ascii_case("gtd"))
    {
        return false;
    }
    let mut parts = task.source_path.split('/');
    let (Some(year), Some(filename), None) =
        (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if filename.len() != 11 || !filename.ends_with(".md") {
        return false;
    }
    let Ok(expected_year) = year.parse::<i32>() else {
        return false;
    };
    NaiveDate::parse_from_str(&filename[..8], "%Y%m%d")
        .is_ok_and(|date| date.year() == expected_year)
}

fn kind_matches(filter: TaskKindFilter, kind: TaskKindPart) -> bool {
    matches!(
        (filter, kind),
        (TaskKindFilter::Task, TaskKindPart::Task)
            | (TaskKindFilter::Prj, TaskKindPart::Prj)
            | (TaskKindFilter::Ref, TaskKindPart::Ref)
    )
}

fn note_target_matches(target: &str, path: &str) -> bool {
    let normalize = |value: &str| {
        let value = value.trim();
        let value = value
            .strip_prefix("[[")
            .and_then(|value| value.strip_suffix("]]"))
            .unwrap_or(value);
        let value = value.split_once('|').map_or(value, |(name, _)| name);
        let value = value.split_once('#').map_or(value, |(name, _)| name);
        let value = value.strip_prefix("./").unwrap_or(value);
        value.trim_end_matches(".md").to_lowercase()
    };
    let target = normalize(target);
    let path_normalized = normalize(path);
    target == path_normalized
        || target.rsplit('/').next() == path_normalized.rsplit('/').next()
}

struct ChecklistResult {
    since: Option<NaiveDate>,
    values: Vec<DailyValue>,
    warnings: Vec<StatsWarning>,
}

#[derive(Debug)]
struct ChecklistInstance {
    chore: String,
    start: NaiveDate,
    completion: Option<NaiveDate>,
}

fn evaluate_checklist(
    metric: &MetricDef,
    index: &StatsIndex,
    dates: &[NaiveDate],
    today: NaiveDate,
) -> ChecklistResult {
    let MetricSpec::Checklist {
        tags,
        match_text,
        measure,
    } = &metric.spec
    else {
        return ChecklistResult {
            since: None,
            values: Vec::new(),
            warnings: Vec::new(),
        };
    };
    let match_text = match_text.as_deref().map(normalize_text);
    let mut instances = Vec::new();
    let mut undatable = 0usize;
    let mut cancelled = 0usize;
    for task in index
        .tasks
        .iter()
        .filter(|task| is_canonical_daily(&task.source_path))
    {
        let task_tags = task
            .tags
            .iter()
            .map(|tag| tag.trim_start_matches('#'))
            .collect::<Vec<_>>();
        if !tags.iter().all(|tag| {
            task_tags.iter().any(|task_tag| {
                task_tag
                    .eq_ignore_ascii_case(tag.trim().trim_start_matches('#'))
            })
        }) {
            continue;
        }
        let chore = chore_identity(task);
        if match_text
            .as_deref()
            .is_some_and(|needle| !chore.contains(needle))
        {
            continue;
        }
        if task.status_type == "CANCELLED" {
            cancelled += 1;
            continue;
        }
        let start_field = if task.scheduled.raw.is_some() {
            &task.scheduled
        } else {
            &task.created
        };
        let Some(start) = start_field.date else {
            undatable += 1;
            continue;
        };
        if start_field.diagnostic == DateDiagnostic::Malformed {
            undatable += 1;
            continue;
        }
        let completion = match task.done.date {
            Some(date) if task.done.diagnostic == DateDiagnostic::Ok => {
                Some(date)
            }
            _ if task.status_type == "DONE" => {
                undatable += 1;
                continue;
            }
            _ => None,
        };
        if completion.is_some_and(|completion| completion < start) {
            undatable += 1;
            continue;
        }
        instances.push(ChecklistInstance {
            chore,
            start,
            completion,
        });
    }

    let since = instances
        .iter()
        .map(|instance| instance.start)
        .filter(|date| *date <= today)
        .min();
    let mut due_by_day = BTreeMap::<NaiveDate, BTreeSet<String>>::new();
    let mut met_by_day = BTreeMap::<NaiveDate, BTreeSet<String>>::new();
    for instance in &instances {
        if instance.start > today {
            continue;
        }
        for date in dates.iter().copied().filter(|date| {
            *date >= instance.start
                && *date <= today
                && instance
                    .completion
                    .is_none_or(|completion| *date <= completion)
        }) {
            due_by_day
                .entry(date)
                .or_default()
                .insert(instance.chore.clone());
            if instance.completion == Some(date) {
                met_by_day
                    .entry(date)
                    .or_default()
                    .insert(instance.chore.clone());
            }
        }
    }

    let values = dates
        .iter()
        .map(|date| {
            let provisional = *date == today;
            if since.is_some_and(|start| *date < start) {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::NotTracked,
                    Some("before tracking began".to_string()),
                    provisional,
                    BTreeMap::new(),
                );
            }
            let due = due_by_day.get(date);
            let met = met_by_day.get(date);
            let due_count = due.map_or(0, BTreeSet::len);
            let met_count = met.map_or(0, BTreeSet::len);
            if due_count == 0 {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::Missing,
                    Some(if since.is_none() {
                        "no matching checklist instance has been recorded"
                            .to_string()
                    } else {
                        "nothing due".to_string()
                    }),
                    provisional,
                    BTreeMap::new(),
                );
            }

            let due_i64 = i64::try_from(due_count).unwrap_or(i64::MAX);
            let met_i64 = i64::try_from(met_count).unwrap_or(i64::MAX);
            let (value, num, den) = match measure {
                ChecklistMeasure::Ratio => {
                    let basis_points = (i128::from(met_i64) * 10_000
                        + i128::from(due_i64) / 2)
                        / i128::from(due_i64);
                    (
                        Some(i64::try_from(basis_points).unwrap_or(i64::MAX)),
                        Some(met_i64),
                        Some(due_i64),
                    )
                }
                ChecklistMeasure::Count => (Some(met_i64), None, None),
                ChecklistMeasure::Bool => {
                    (Some(i64::from(met_count == due_count)), None, None)
                }
            };
            let mut parts = BTreeMap::new();
            for chore in due.into_iter().flatten() {
                let dimension = if met.is_some_and(|met| met.contains(chore)) {
                    "met"
                } else {
                    "missed"
                };
                add_dimension_part(&mut parts, dimension, chore, 1);
            }
            daily(
                *date,
                value,
                num,
                den,
                DataQuality::Ok,
                None,
                provisional,
                parts,
            )
        })
        .collect();

    let mut warnings = Vec::new();
    if undatable > 0 || cancelled > 0 {
        let mut issues = Vec::new();
        if undatable > 0 {
            issues.push(format!(
                "{undatable} matching checklist instance(s) have no usable interval"
            ));
        }
        if cancelled > 0 {
            issues.push(format!(
                "{cancelled} matching cancelled checklist instance(s) were excluded"
            ));
        }
        warnings.push(StatsWarning {
            code: "checklist_limitation".to_string(),
            metric: Some(metric.id.clone()),
            date: None,
            message: issues.join("; "),
        });
    }
    ChecklistResult {
        since,
        values,
        warnings,
    }
}

fn is_canonical_daily(path: &str) -> bool {
    let mut parts = path.split('/');
    let (Some(year), Some(filename), None) =
        (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    filename.len() == 11
        && filename.ends_with(".md")
        && year.parse::<i32>().is_ok_and(|expected_year| {
            NaiveDate::parse_from_str(&filename[..8], "%Y%m%d")
                .is_ok_and(|date| date.year() == expected_year)
        })
}

fn chore_identity(task: &IndexedTask) -> String {
    static INLINE_FIELD: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| {
            Regex::new(r"\[[A-Za-z][A-Za-z0-9_-]*::[^\]]*\]")
                .expect("valid inline-field regex")
        });
    static TAG: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r#"(?:^|\s)#[^\s!@#$%^&*(),.?":{}|<>]+"#)
            .expect("valid hashtag regex")
    });
    static BLOCK: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"\s+\^[A-Za-z0-9-]+\s*$").expect("valid block-id regex")
    });
    let without_fields = INLINE_FIELD.replace_all(&task.description, " ");
    let without_tags = TAG.replace_all(&without_fields, " ");
    let without_block = BLOCK.replace_all(&without_tags, " ");
    let identity = without_block
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if identity.is_empty() {
        "(unnamed checklist item)".to_string()
    } else {
        identity
    }
}

fn field_since(metric: &MetricDef, index: &StatsIndex) -> Option<NaiveDate> {
    let MetricSpec::Field { field, source } = &metric.spec else {
        return None;
    };
    index
        .daily_notes
        .iter()
        .filter(|(_, note)| selected_field(note, field, *source).is_some())
        .map(|(date, _)| *date)
        .min()
}

fn evaluate_fields(
    metric: &MetricDef,
    index: &StatsIndex,
    dates: &[NaiveDate],
    today: NaiveDate,
    since: Option<NaiveDate>,
) -> Vec<DailyValue> {
    let MetricSpec::Field { field, source } = &metric.spec else {
        return Vec::new();
    };
    dates
        .iter()
        .map(|date| {
            let provisional = *date == today;
            if since.is_some_and(|start| *date < start) {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::NotTracked,
                    Some("before tracking began".to_string()),
                    provisional,
                    BTreeMap::new(),
                );
            }
            let Some(note) = index.daily_notes.get(date) else {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::Missing,
                    Some("no daily note".to_string()),
                    provisional,
                    BTreeMap::new(),
                );
            };
            let Some(raw) = selected_field(note, field, *source) else {
                return daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::Missing,
                    Some(format!("field {field:?} is absent")),
                    provisional,
                    BTreeMap::new(),
                );
            };
            match parse_field_value(raw, metric.unit) {
                Some(value) => daily(
                    *date,
                    Some(value),
                    None,
                    None,
                    DataQuality::Ok,
                    None,
                    provisional,
                    BTreeMap::new(),
                ),
                None => daily(
                    *date,
                    None,
                    None,
                    None,
                    DataQuality::Missing,
                    Some(format!(
                        "field {field:?} has invalid {} value {raw:?}",
                        unit_name(metric.unit)
                    )),
                    provisional,
                    BTreeMap::new(),
                ),
            }
        })
        .collect()
}

fn selected_field<'a>(
    note: &'a DailyNoteInputs,
    field: &str,
    source: FieldSource,
) -> Option<&'a str> {
    match source {
        FieldSource::Frontmatter => note.frontmatter_fields.get(field),
        FieldSource::Inline => note.inline_fields.get(field),
        FieldSource::Either => note
            .frontmatter_fields
            .get(field)
            .or_else(|| note.inline_fields.get(field)),
    }
    .map(String::as_str)
}

fn parse_field_value(raw: &str, unit: MetricUnit) -> Option<i64> {
    let raw = raw.trim();
    match unit {
        MetricUnit::Count => parse_nonnegative_integer(raw),
        MetricUnit::Minutes => parse_minutes(raw),
        MetricUnit::Bool => match raw.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Some(1),
            "0" | "false" | "no" => Some(0),
            _ => None,
        },
        MetricUnit::Signed | MetricUnit::Ratio => None,
    }
}

fn parse_nonnegative_integer(value: &str) -> Option<i64> {
    if value.is_empty() || value.starts_with('-') || value.starts_with('+') {
        return None;
    }
    value.parse::<i64>().ok()
}

fn parse_minutes(value: &str) -> Option<i64> {
    if let Some(integer) = parse_nonnegative_integer(value) {
        return Some(integer);
    }
    static DURATION: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| {
            Regex::new(r"^(?:(\d+)h)?(?:\s*(\d+)m)?$")
                .expect("valid duration regex")
        });
    let captures = DURATION.captures(value)?;
    let hours = captures
        .get(1)
        .map(|value| value.as_str().parse::<i64>().ok())
        .flatten()
        .unwrap_or(0);
    let minutes = captures
        .get(2)
        .map(|value| value.as_str().parse::<i64>().ok())
        .flatten()
        .unwrap_or(0);
    if captures.get(1).is_none() && captures.get(2).is_none() {
        return None;
    }
    if minutes >= 60 {
        return None;
    }
    hours.checked_mul(60)?.checked_add(minutes)
}

fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn unit_name(unit: MetricUnit) -> &'static str {
    match unit {
        MetricUnit::Count => "count",
        MetricUnit::Signed => "signed",
        MetricUnit::Minutes => "minutes",
        MetricUnit::Bool => "bool",
        MetricUnit::Ratio => "ratio",
    }
}

fn daily(
    date: NaiveDate,
    value: Option<i64>,
    num: Option<i64>,
    den: Option<i64>,
    quality: DataQuality,
    reason: Option<String>,
    provisional: bool,
    parts: BTreeMap<String, BTreeMap<String, i64>>,
) -> DailyValue {
    DailyValue {
        date,
        value,
        num,
        den,
        quality,
        reason,
        provisional,
        parts,
    }
}

fn add_part(parts: &mut BTreeMap<String, i64>, key: String, value: i64) {
    *parts.entry(key).or_default() += value;
}

fn add_dimension_part(
    parts: &mut BTreeMap<String, BTreeMap<String, i64>>,
    dimension: &str,
    key: &str,
    value: i64,
) {
    *parts
        .entry(dimension.to_string())
        .or_default()
        .entry(key.to_string())
        .or_default() += value;
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use chrono::NaiveDate;

    use super::*;
    use crate::native::{
        config::parse_stats_config, stats::registry::build_registry,
    };

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn builtin(id: &str) -> MetricDef {
        let config =
            parse_stats_config("stats:\n", Path::new("/config.yml")).unwrap();
        build_registry(&config).get(id).unwrap().clone()
    }

    fn custom_metric(yaml: &str, id: &str) -> MetricDef {
        let config = parse_stats_config(
            &format!("stats:\n  metrics:\n    {id}:\n{}", indent(yaml)),
            Path::new("/config.yml"),
        )
        .unwrap();
        build_registry(&config).get(id).unwrap().clone()
    }

    fn indent(yaml: &str) -> String {
        yaml.lines().map(|line| format!("      {line}\n")).collect()
    }

    fn task(
        path: &str,
        status: &str,
        created: Option<&str>,
        done: Option<&str>,
        cancelled: Option<&str>,
        scheduled: Option<&str>,
        description: &str,
        tags: &[&str],
        block_id: Option<&str>,
        kind: TaskKindPart,
    ) -> IndexedTask {
        let dated = |raw: Option<&str>| {
            let raw = raw.map(str::to_string);
            let has_raw = raw.is_some();
            let parsed = raw.as_deref().and_then(|value| {
                NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
            });
            DatedValue {
                raw,
                date: parsed,
                diagnostic: if parsed.is_some() {
                    DateDiagnostic::Ok
                } else if has_raw {
                    DateDiagnostic::Malformed
                } else {
                    DateDiagnostic::Missing
                },
            }
        };
        IndexedTask {
            source_path: path.to_string(),
            archive_path: None,
            status_type: status.to_string(),
            created: dated(created),
            done: dated(done),
            cancelled: dated(cancelled),
            scheduled: dated(scheduled),
            recurring: false,
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            block_id: block_id.map(str::to_string),
            description: description.to_string(),
            task_kind: kind,
        }
    }

    #[test]
    fn pomodoro_minutes_prefer_recorded_duration_and_split_odd_minutes() {
        let mut index = StatsIndex::default();
        index.daily_notes.insert(
            date(10),
            DailyNoteInputs {
                path: "2026/20261010.md".to_string(),
                has_pomodoros_heading: true,
                closed_pomodoros: vec![
                    super::super::index::PomodoroSession {
                        name: Some("SASE + GTD".to_string()),
                        recorded_minutes: Some(7),
                        range_minutes: Some(30),
                        line: 2,
                    },
                    super::super::index::PomodoroSession {
                        name: Some("GTD".to_string()),
                        recorded_minutes: None,
                        range_minutes: Some(25),
                        line: 3,
                    },
                ],
                ..DailyNoteInputs::default()
            },
        );
        let metric = custom_metric(
            "label: GTD minutes\nkind: pomodoro\nmeasure: minutes\nthemes: [GTD]\nunit: minutes",
            "gtd_minutes",
        );
        let result =
            evaluate_metric(&metric, &index, &[date(10)], date(11), None)
                .unwrap();
        let value = &result.values[0];
        // The 7m composite splits 4m to SASE and 3m to GTD. The second
        // session uses its 25m range estimate.
        assert_eq!(value.value, Some(28));
        assert_eq!(value.parts["theme"]["GTD"], 28);
        assert_eq!(value.quality, DataQuality::Partial);
        assert_eq!(result.since, Some(date(10)));
    }

    #[test]
    fn pomodoro_missing_section_empty_section_and_unreadable_time_are_distinct()
    {
        let mut index = StatsIndex::default();
        index.daily_notes.insert(
            date(10),
            DailyNoteInputs {
                path: "2026/20261010.md".to_string(),
                has_pomodoros_heading: true,
                closed_pomodoros: Vec::new(),
                ..DailyNoteInputs::default()
            },
        );
        index.daily_notes.insert(
            date(11),
            DailyNoteInputs {
                path: "2026/20261011.md".to_string(),
                has_pomodoros_heading: true,
                closed_pomodoros: vec![super::super::index::PomodoroSession {
                    name: Some("GTD".to_string()),
                    recorded_minutes: None,
                    range_minutes: None,
                    line: 2,
                }],
                ..DailyNoteInputs::default()
            },
        );
        let metric = builtin("worked");
        let result = evaluate_metric(
            &metric,
            &index,
            &[date(9), date(10), date(11), date(12)],
            date(12),
            None,
        )
        .unwrap();
        assert_eq!(result.values[0].quality, DataQuality::NotTracked);
        assert_eq!(result.values[1].value, Some(0));
        assert_eq!(result.values[1].quality, DataQuality::Ok);
        assert_eq!(result.values[2].value, None);
        assert_eq!(result.values[2].quality, DataQuality::Partial);
        assert_eq!(result.values[3].reason.as_deref(), Some("no daily note"));

        let mut no_section = StatsIndex::default();
        no_section.daily_notes.insert(
            date(10),
            DailyNoteInputs {
                path: "2026/20261010.md".to_string(),
                ..DailyNoteInputs::default()
            },
        );
        let result =
            evaluate_metric(&metric, &no_section, &[date(10)], date(11), None)
                .unwrap();
        assert_eq!(
            result.values[0].reason.as_deref(),
            Some("Pomodoros section missing")
        );
    }

    #[test]
    fn task_events_filter_status_recurring_tags_kind_archive_and_gtd() {
        let mut index = StatsIndex::default();
        index.tasks = vec![
            task(
                "done/2026/20261010_done.md",
                "DONE",
                Some("2026-10-09"),
                Some("2026-10-10"),
                None,
                None,
                "Reference reading",
                &["ref", "important"],
                Some("ref"),
                TaskKindPart::Ref,
            ),
            task(
                "2026/20261010.md",
                "DONE",
                Some("2026-10-09"),
                Some("2026-10-10"),
                None,
                None,
                "GTD walk link",
                &["gtd"],
                Some("gtd"),
                TaskKindPart::Task,
            ),
            task(
                "tasks.md",
                "CANCELLED",
                Some("2026-10-09"),
                None,
                Some("2026-10-10"),
                None,
                "Cancelled ref task",
                &["ref"],
                None,
                TaskKindPart::Ref,
            ),
            task(
                "tasks.md",
                "NON_TASK",
                Some("2026-10-10"),
                None,
                None,
                None,
                "Not a done event",
                &["ref"],
                None,
                TaskKindPart::Ref,
            ),
        ];
        index
            .parent_of
            .insert("child.md".to_string(), "Project.md".to_string());
        let metric = custom_metric(
            "label: Ref done\nkind: tasks\nevent: done\ntask_kind: ref\ntags_all: [important]\nin: [Project]\nunit: count",
            "ref_done",
        );
        // Use an archived row whose mapped source is under Project.
        index.tasks[0].source_path = "child/task.md".to_string();
        index
            .parent_of
            .insert("child/task.md".to_string(), "Project.md".to_string());
        let result = evaluate_metric(
            &metric,
            &index,
            &[date(9), date(10), date(11)],
            date(11),
            None,
        )
        .unwrap();
        assert_eq!(result.since, Some(date(10)));
        assert_eq!(result.values[0].quality, DataQuality::NotTracked);
        assert_eq!(result.values[1].value, Some(1));
        assert_eq!(result.values[1].parts["kind"]["ref"], 1);
        assert_eq!(result.values[1].parts["note"]["child/task.md"], 1);
        assert_eq!(result.values[2].value, Some(0));

        let dropped = evaluate_metric(
            &builtin("dropped"),
            &index,
            &[date(10)],
            date(11),
            None,
        )
        .unwrap();
        assert_eq!(dropped.values[0].value, Some(1));

        let mut explicitly_tracked = builtin("done");
        explicitly_tracked.since = Some(date(9));
        let empty = evaluate_metric(
            &explicitly_tracked,
            &StatsIndex::default(),
            &[date(8), date(9)],
            date(11),
            None,
        )
        .unwrap();
        assert_eq!(empty.values[0].quality, DataQuality::NotTracked);
        assert_eq!(empty.values[1].value, Some(0));
        assert_eq!(empty.values[1].quality, DataQuality::Ok);
    }

    #[test]
    fn checklist_intervals_deduplicate_chore_and_separate_met_from_missed() {
        let mut index = StatsIndex::default();
        index.tasks = vec![
            task(
                "2026/20261009.md",
                "TODO",
                Some("2026-10-09"),
                None,
                None,
                None,
                "Meditate [minutes:: 10] #gtd #pre ^one",
                &["gtd", "pre"],
                Some("one"),
                TaskKindPart::Task,
            ),
            task(
                "2026/20261010.md",
                "DONE",
                Some("2026-10-10"),
                Some("2026-10-11"),
                None,
                None,
                "  MEDITATE #GTD #PRE ^two",
                &["GTD", "PRE"],
                Some("two"),
                TaskKindPart::Task,
            ),
            task(
                "2026/20261010.md",
                "CANCELLED",
                Some("2026-10-10"),
                None,
                Some("2026-10-10"),
                None,
                "Meditate #gtd #pre ^three",
                &["gtd", "pre"],
                Some("three"),
                TaskKindPart::Task,
            ),
        ];
        let metric = builtin("habits");
        let result = evaluate_metric(
            &metric,
            &index,
            &[date(8), date(9), date(10), date(11)],
            date(11),
            None,
        )
        .unwrap();
        assert_eq!(result.since, Some(date(9)));
        assert_eq!(result.values[0].quality, DataQuality::NotTracked);
        assert_eq!(result.values[1].num, Some(0));
        assert_eq!(result.values[1].den, Some(1));
        assert_eq!(result.values[1].parts["missed"]["meditate"], 1);
        assert_eq!(result.values[2].num, Some(0));
        assert_eq!(result.values[2].den, Some(1));
        assert_eq!(result.values[3].num, Some(1));
        assert_eq!(result.values[3].den, Some(1));
        assert_eq!(result.values[3].parts["met"]["meditate"], 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].message.contains("cancelled"));

        let review = builtin("review_closed");
        let result = evaluate_metric(
            &review,
            &index,
            &[date(10), date(11)],
            date(11),
            None,
        )
        .unwrap();
        assert_eq!(result.values[0].quality, DataQuality::Missing);
    }

    #[test]
    fn checklist_nothing_due_is_missing_and_future_schedule_is_not_due() {
        let mut index = StatsIndex::default();
        index.tasks.push(task(
            "2026/20261010.md",
            "TODO",
            Some("2026-10-10"),
            None,
            None,
            Some("2026-10-12"),
            "Review #gtd #pre",
            &["gtd", "pre"],
            None,
            TaskKindPart::Task,
        ));
        let metric = builtin("habits");
        let result = evaluate_metric(
            &metric,
            &index,
            &[date(10), date(11)],
            date(11),
            None,
        )
        .unwrap();
        // The scheduled date is after today, so this is not yet tracking.
        assert_eq!(result.since, None);
        assert_eq!(result.values[0].quality, DataQuality::Missing);
        assert_eq!(
            result.values[1].reason.as_deref(),
            Some("no matching checklist instance has been recorded")
        );
    }

    #[test]
    fn field_values_are_typed_and_either_does_not_fallback_after_invalid_frontmatter(
    ) {
        let mut index = StatsIndex::default();
        index.daily_notes.insert(
            date(10),
            DailyNoteInputs {
                path: "2026/20261010.md".to_string(),
                frontmatter_fields: BTreeMap::from([(
                    "focus".to_string(),
                    "n/a".to_string(),
                )]),
                inline_fields: BTreeMap::from([(
                    "focus".to_string(),
                    "5".to_string(),
                )]),
                ..DailyNoteInputs::default()
            },
        );
        index.daily_notes.insert(
            date(11),
            DailyNoteInputs {
                path: "2026/20261011.md".to_string(),
                inline_fields: BTreeMap::from([(
                    "focus".to_string(),
                    "1h 30m".to_string(),
                )]),
                ..DailyNoteInputs::default()
            },
        );
        let metric = custom_metric(
            "label: Focus minutes\nkind: field\nfield: focus\nsource: either\nunit: minutes",
            "focus_minutes",
        );
        let result = evaluate_metric(
            &metric,
            &index,
            &[date(9), date(10), date(11), date(12)],
            date(12),
            None,
        )
        .unwrap();
        assert_eq!(result.since, Some(date(10)));
        assert_eq!(result.values[0].quality, DataQuality::NotTracked);
        assert_eq!(result.values[1].quality, DataQuality::Missing);
        assert!(result.values[1]
            .reason
            .as_deref()
            .unwrap()
            .contains("invalid minutes"));
        assert_eq!(result.values[2].value, Some(90));
        assert_eq!(result.values[3].quality, DataQuality::Missing);

        assert_eq!(parse_field_value("-1", MetricUnit::Count), None);
        assert_eq!(parse_field_value("false", MetricUnit::Bool), Some(0));
        assert_eq!(parse_field_value("2h 05m", MetricUnit::Minutes), Some(125));
        assert_eq!(parse_field_value("2h 90m", MetricUnit::Minutes), None);
    }
}
