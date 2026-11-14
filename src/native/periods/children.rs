//! The periods calendar's child partition.
//!
//! A stats-strip period splits into contiguous child spans: the days of an
//! ISO week, the weeks of a quarter or ISO week-year, and the quarters of
//! all time. Long periods also group their columns: an ISO week-year by
//! quarter and all time by ISO week-year. `bob periods show` lays out its
//! columns, children, and groups from these specs, and
//! `bob bank report -c` buckets money over exactly the same spans, so a
//! bank bucket and a strip column with the same id cover the same days.

use chrono::{Datelike, Days, NaiveDate, Weekday};

use super::{
    calendar::{quarter_of_week, PeriodId},
    labels,
};

/// An inclusive span of calendar days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
}

/// One child of a period: a day (`day`) or a sub-period (`id`), its span
/// clipped to the parent, and its group caption (`Q1`–`Q4` for the weeks of
/// an ISO week-year, the ISO year for the quarters of all time).
#[derive(Debug, Clone)]
pub(crate) struct ChildSpec {
    pub(crate) id: Option<PeriodId>,
    pub(crate) day: Option<NaiveDate>,
    pub(crate) span: Span,
    pub(crate) group: Option<String>,
}

impl ChildSpec {
    /// The child's stable id: `YYYY-MM-DD` for a day, otherwise the
    /// canonical period id (`2026-W46`, `2026-Q4`, `2026`).
    pub(crate) fn key(&self) -> String {
        match (self.day, self.id) {
            (Some(day), _) => day.format("%Y-%m-%d").to_string(),
            (None, Some(id)) => id.to_string(),
            (None, None) => String::new(),
        }
    }

    /// The child's axis caption: `Wed 11` for a day, otherwise the short
    /// period label (`W46`, `Q4`, `2026`).
    pub(crate) fn caption(&self) -> String {
        match (self.day, self.id) {
            (Some(day), _) => day_caption(day),
            (None, Some(id)) => labels::short_label(id),
            (None, None) => String::new(),
        }
    }
}

/// One group of a long period's columns: a quarter of an ISO week-year or
/// an ISO week-year of all time, clipped to the parent.
#[derive(Debug, Clone)]
pub(crate) struct GroupSpec {
    pub(crate) id: PeriodId,
    pub(crate) caption: String,
    pub(crate) span: Span,
}

/// The strip's columns: days of a week, weeks of a quarter or ISO
/// week-year (grouped `Q1`–`Q4` on a year), and quarters of all time
/// (grouped by ISO year).
pub(crate) fn column_specs(id: PeriodId, span: Span) -> Vec<ChildSpec> {
    match id {
        PeriodId::Week { .. } => dates_between(span)
            .into_iter()
            .map(|day| ChildSpec {
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
                                    quarter_of_week(week),
                                _ => 1,
                            }
                        ))
                    } else {
                        None
                    };
                    ChildSpec {
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
            .map(|quarter| ChildSpec {
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

/// The parts table's children: the columns for a week or quarter, the
/// four quarters of an ISO week-year, and the ISO years of all time.
pub(crate) fn child_specs(id: PeriodId, span: Span) -> Vec<ChildSpec> {
    match id {
        PeriodId::Week { .. } => column_specs(id, span),
        PeriodId::Quarter { .. } => column_specs(id, span),
        PeriodId::Year(year) => (1..=4)
            .filter_map(|quarter| {
                let id = PeriodId::Quarter { year, quarter };
                let qspan = period_bounds(id)?;
                Some(ChildSpec {
                    id: Some(id),
                    day: None,
                    span: clip_span(qspan, span),
                    group: None,
                })
            })
            .collect(),
        PeriodId::AllTime => years_between(span)
            .into_iter()
            .map(|id| ChildSpec {
                id: Some(id),
                day: None,
                span: clip_span(period_bounds(id).unwrap_or(span), span),
                group: None,
            })
            .collect(),
    }
}

/// The column groups of a long period: `Q1`–`Q4` for an ISO week-year and
/// one group per ISO year for all time. Weeks and quarters have none.
pub(crate) fn group_specs(id: PeriodId, span: Span) -> Vec<GroupSpec> {
    match id {
        PeriodId::Year(year) => (1..=4)
            .map(|quarter| {
                let child = PeriodId::Quarter { year, quarter };
                GroupSpec {
                    id: child,
                    caption: format!("Q{quarter}"),
                    span: clip_span(period_bounds(child).unwrap_or(span), span),
                }
            })
            .collect(),
        PeriodId::AllTime => years_between(span)
            .into_iter()
            .map(|child| GroupSpec {
                id: child,
                caption: match child {
                    PeriodId::Year(year) => year.to_string(),
                    _ => labels::short_label(child),
                },
                span: clip_span(period_bounds(child).unwrap_or(span), span),
            })
            .collect(),
        PeriodId::Week { .. } | PeriodId::Quarter { .. } => Vec::new(),
    }
}

/// Every calendar day of `span`, or none when the span is reversed.
pub(crate) fn dates_between(span: Span) -> Vec<NaiveDate> {
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

/// The ISO weeks touching `span`, in order.
pub(crate) fn weeks_between(span: Span) -> Vec<PeriodId> {
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

/// The quarters touching `span`, in order.
pub(crate) fn quarters_between(span: Span) -> Vec<PeriodId> {
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

/// The ISO week-years from the one holding `span.start` through the one
/// holding `span.end`.
pub(crate) fn years_between(span: Span) -> Vec<PeriodId> {
    let (PeriodId::Year(first), PeriodId::Year(last)) = (
        PeriodId::year_for_date(span.start),
        PeriodId::year_for_date(span.end),
    ) else {
        return Vec::new();
    };
    (first..=last).map(PeriodId::Year).collect()
}

/// A period's natural span, or `None` for all time.
pub(crate) fn period_bounds(id: PeriodId) -> Option<Span> {
    Some(Span {
        start: id.start()?,
        end: id.end()?,
    })
}

/// `inner` clipped to `outer`.
pub(crate) fn clip_span(inner: Span, outer: Span) -> Span {
    Span {
        start: inner.start.max(outer.start),
        end: inner.end.min(outer.end),
    }
}

/// A day's axis caption: `{weekday} {day}`, for example `Wed 11`.
pub(crate) fn day_caption(day: NaiveDate) -> String {
    format!("{} {}", weekday(day), day.day())
}

/// The three-letter English weekday of `date`.
pub(crate) fn weekday(date: NaiveDate) -> &'static str {
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

fn quarter_year(id: PeriodId) -> i32 {
    match id {
        PeriodId::Quarter { year, .. } => year,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn natural(id: PeriodId) -> Span {
        period_bounds(id).unwrap()
    }

    /// Each spec's span is inside the parent, and the specs tile it with
    /// no gap or overlap.
    fn assert_partition(specs: &[Span], parent: Span) {
        assert!(!specs.is_empty());
        assert_eq!(specs.first().unwrap().start, parent.start);
        assert_eq!(specs.last().unwrap().end, parent.end);
        for pair in specs.windows(2) {
            assert!(pair[0].start <= pair[0].end);
            assert_eq!(pair[0].end.succ_opt().unwrap(), pair[1].start);
        }
    }

    fn keys(specs: &[ChildSpec]) -> Vec<String> {
        specs.iter().map(ChildSpec::key).collect()
    }

    fn captions(specs: &[ChildSpec]) -> Vec<String> {
        specs.iter().map(ChildSpec::caption).collect()
    }

    #[test]
    fn week_children_are_its_seven_days() {
        let week = PeriodId::Week {
            year: 2026,
            week: 46,
        };
        let span = natural(week);
        let columns = column_specs(week, span);
        assert_eq!(
            keys(&columns),
            [
                "2026-11-09",
                "2026-11-10",
                "2026-11-11",
                "2026-11-12",
                "2026-11-13",
                "2026-11-14",
                "2026-11-15"
            ]
        );
        assert_eq!(
            captions(&columns),
            [
                "Mon 9", "Tue 10", "Wed 11", "Thu 12", "Fri 13", "Sat 14",
                "Sun 15"
            ]
        );
        assert!(columns.iter().all(|spec| spec.group.is_none()
            && spec.id.is_none()
            && spec.day == Some(spec.span.start)));
        assert_partition(
            &columns.iter().map(|spec| spec.span).collect::<Vec<_>>(),
            span,
        );
        assert_eq!(keys(&child_specs(week, span)), keys(&columns));
        assert!(group_specs(week, span).is_empty());
    }

    #[test]
    fn year_boundary_weeks_keep_their_iso_days() {
        // 2026-W01 starts in December 2025; 2026-W53 ends in January 2027.
        let first = PeriodId::Week {
            year: 2026,
            week: 1,
        };
        let first_columns = column_specs(first, natural(first));
        assert_eq!(first_columns[0].key(), "2025-12-29");
        assert_eq!(first_columns[0].caption(), "Mon 29");
        assert_eq!(first_columns[6].key(), "2026-01-04");
        assert_eq!(first_columns[6].caption(), "Sun 4");

        let last = PeriodId::Week {
            year: 2026,
            week: 53,
        };
        let last_columns = column_specs(last, natural(last));
        assert_eq!(last_columns[0].key(), "2026-12-28");
        assert_eq!(last_columns[6].key(), "2027-01-03");
        assert_eq!(last_columns[6].caption(), "Sun 3");
    }

    #[test]
    fn q4_children_are_its_fourteen_weeks() {
        let quarter = PeriodId::Quarter {
            year: 2026,
            quarter: 4,
        };
        let span = natural(quarter);
        let columns = column_specs(quarter, span);
        assert_eq!(columns.len(), 14);
        assert_eq!(columns[0].key(), "2026-W40");
        assert_eq!(columns[0].caption(), "W40");
        assert_eq!(columns[13].key(), "2026-W53");
        assert_eq!(columns[13].caption(), "W53");
        assert!(columns.iter().all(|spec| spec.group.is_none()));
        assert_partition(
            &columns.iter().map(|spec| spec.span).collect::<Vec<_>>(),
            span,
        );
        assert_eq!(keys(&child_specs(quarter, span)), keys(&columns));
        assert!(group_specs(quarter, span).is_empty());
    }

    #[test]
    fn iso_year_children_are_53_weeks_grouped_by_quarter() {
        let year = PeriodId::Year(2026);
        let span = natural(year);
        let columns = column_specs(year, span);
        assert_eq!(columns.len(), 53);
        assert_partition(
            &columns.iter().map(|spec| spec.span).collect::<Vec<_>>(),
            span,
        );
        let groups = group_specs(year, span);
        assert_eq!(
            groups
                .iter()
                .map(|group| group.caption.as_str())
                .collect::<Vec<_>>(),
            ["Q1", "Q2", "Q3", "Q4"]
        );
        assert_partition(
            &groups.iter().map(|group| group.span).collect::<Vec<_>>(),
            span,
        );
        // Every week sits inside the group its caption names.
        for column in &columns {
            let caption = column.group.as_deref().expect("week group");
            let group = groups
                .iter()
                .find(|group| group.caption == caption)
                .expect("named group");
            assert!(group.span.start <= column.span.start);
            assert!(column.span.end <= group.span.end);
            assert_eq!(
                column.caption(),
                labels::short_label(column.id.unwrap())
            );
        }
        assert_eq!(
            columns
                .iter()
                .filter(|spec| spec.group.as_deref() == Some("Q4"))
                .count(),
            14
        );
        // The parts table's children are the same four quarters.
        let children = child_specs(year, span);
        assert_eq!(
            keys(&children),
            ["2026-Q1", "2026-Q2", "2026-Q3", "2026-Q4"]
        );
        assert_eq!(captions(&children), ["Q1", "Q2", "Q3", "Q4"]);
        for (child, group) in children.iter().zip(&groups) {
            assert_eq!(child.span, group.span);
            assert_eq!(Some(group.id), child.id);
        }
    }

    #[test]
    fn all_time_children_cross_years_with_clipped_ends() {
        // Mid-quarter start and end: the first and last quarters clip.
        let span = Span {
            start: date(2024, 11, 20),
            end: date(2026, 11, 12),
        };
        let columns = column_specs(PeriodId::AllTime, span);
        assert_eq!(
            keys(&columns),
            [
                "2024-Q4", "2025-Q1", "2025-Q2", "2025-Q3", "2025-Q4",
                "2026-Q1", "2026-Q2", "2026-Q3", "2026-Q4"
            ]
        );
        assert_eq!(columns[0].caption(), "Q4");
        assert_eq!(columns[0].span.start, date(2024, 11, 20));
        assert_eq!(columns[8].span.end, date(2026, 11, 12));
        assert_eq!(
            columns
                .iter()
                .map(|spec| spec.group.clone().unwrap())
                .collect::<Vec<_>>(),
            [
                "2024", "2025", "2025", "2025", "2025", "2026", "2026", "2026",
                "2026"
            ]
        );
        assert_partition(
            &columns.iter().map(|spec| spec.span).collect::<Vec<_>>(),
            span,
        );
        let groups = group_specs(PeriodId::AllTime, span);
        assert_eq!(
            groups
                .iter()
                .map(|group| group.caption.as_str())
                .collect::<Vec<_>>(),
            ["2024", "2025", "2026"]
        );
        assert_partition(
            &groups.iter().map(|group| group.span).collect::<Vec<_>>(),
            span,
        );
        let children = child_specs(PeriodId::AllTime, span);
        assert_eq!(keys(&children), ["2024", "2025", "2026"]);
        assert_eq!(captions(&children), ["2024", "2025", "2026"]);
        for (child, group) in children.iter().zip(&groups) {
            assert_eq!(child.span, group.span);
        }
    }

    #[test]
    fn day_captions_match_the_strip_and_periods_match_short_labels() {
        for (day, caption) in [
            (date(2026, 11, 9), "Mon 9"),
            (date(2026, 11, 11), "Wed 11"),
            (date(2026, 11, 15), "Sun 15"),
        ] {
            assert_eq!(day_caption(day), caption);
            assert_eq!(
                day_caption(day),
                format!("{} {}", weekday(day), day.day())
            );
        }
        for id in [
            PeriodId::Week {
                year: 2026,
                week: 46,
            },
            PeriodId::Quarter {
                year: 2026,
                quarter: 4,
            },
            PeriodId::Year(2026),
        ] {
            let spec = ChildSpec {
                id: Some(id),
                day: None,
                span: natural(id),
                group: None,
            };
            assert_eq!(spec.caption(), labels::short_label(id));
            assert_eq!(spec.key(), id.to_string());
        }
    }

    #[test]
    fn reversed_spans_have_no_days() {
        let span = Span {
            start: date(2026, 11, 13),
            end: date(2026, 11, 12),
        };
        assert!(dates_between(span).is_empty());
    }
}
