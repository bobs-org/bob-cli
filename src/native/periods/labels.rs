//! Period labels: range labels, titles, short and nav labels.
//!
//! The contract lives in `docs/periods.md` §1. Labels never make anyone
//! decode ISO: each title prints its real dates.

use chrono::{Datelike, NaiveDate};

use super::calendar::{weeks_in_year, PeriodId};

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
    "Nov", "Dec",
];

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn month_abbrev(month: u32) -> &'static str {
    MONTHS[(month - 1) as usize]
}

fn weekday_abbrev(date: NaiveDate) -> &'static str {
    WEEKDAYS[date.weekday().num_days_from_monday() as usize]
}

/// Range label with an en dash surrounded by spaces:
/// `Nov 2 – 8`, `Oct 26 – Nov 1`, or `Dec 28, 2026 – Jan 3, 2027`.
pub(crate) fn range_label(start: NaiveDate, end: NaiveDate) -> String {
    if start.year() != end.year() {
        format!(
            "{} {}, {} – {} {}, {}",
            month_abbrev(start.month()),
            start.day(),
            start.year(),
            month_abbrev(end.month()),
            end.day(),
            end.year()
        )
    } else if start.month() != end.month() {
        format!(
            "{} {} – {} {}",
            month_abbrev(start.month()),
            start.day(),
            month_abbrev(end.month()),
            end.day()
        )
    } else {
        format!(
            "{} {} – {}",
            month_abbrev(start.month()),
            start.day(),
            end.day()
        )
    }
}

/// Full note title: `2026-W45 · Nov 2 – 8`,
/// `2026-Q4 · W40–W53 · Sep 28, 2026 – Jan 3, 2027`,
/// `2026 · Dec 29, 2025 – Jan 3, 2027 · 53 weeks`, or `All time`.
pub(crate) fn title(id: PeriodId) -> String {
    match id {
        PeriodId::Week { .. } => {
            let (Some(start), Some(end)) = (id.start(), id.end()) else {
                return id.to_string();
            };
            format!("{id} · {}", range_label(start, end))
        }
        PeriodId::Quarter { year, quarter } => {
            let (Some(start), Some(end)) = (id.start(), id.end()) else {
                return id.to_string();
            };
            let first = (quarter - 1) * 13 + 1;
            let last = if quarter < 4 {
                quarter * 13
            } else {
                weeks_in_year(year)
            };
            format!("{id} · W{first}–W{last} · {}", range_label(start, end))
        }
        PeriodId::Year(year) => {
            let (Some(start), Some(end)) = (id.start(), id.end()) else {
                return id.to_string();
            };
            let weeks = weeks_in_year(year);
            format!("{id} · {} · {weeks} weeks", range_label(start, end))
        }
        PeriodId::AllTime => "All time".to_string(),
    }
}

/// Short label: `W45`, `Q4`, `2026`, or `All time`.
pub(crate) fn short_label(id: PeriodId) -> String {
    match id {
        PeriodId::Week { week, .. } => format!("W{week}"),
        PeriodId::Quarter { quarter, .. } => format!("Q{quarter}"),
        PeriodId::Year(year) => format!("{year:04}"),
        PeriodId::AllTime => "All time".to_string(),
    }
}

/// Previous-neighbour label: `‹ <short>`, using the long id when the
/// neighbour is in another ISO year.
pub(crate) fn prev_label(neighbour: PeriodId, current_year: i32) -> String {
    match neighbour {
        PeriodId::Week { year, .. } | PeriodId::Quarter { year, .. } => {
            if year == current_year {
                format!("‹ {}", short_label(neighbour))
            } else {
                format!("‹ {neighbour}")
            }
        }
        PeriodId::Year(year) => {
            if year == current_year {
                format!("‹ {}", short_label(neighbour))
            } else {
                format!("‹ {neighbour}")
            }
        }
        PeriodId::AllTime => format!("‹ {}", short_label(neighbour)),
    }
}

/// Next-neighbour label: `<short> ›`, using the long id when the
/// neighbour is in another ISO year.
pub(crate) fn next_label(neighbour: PeriodId, current_year: i32) -> String {
    match neighbour {
        PeriodId::Week { year, .. } | PeriodId::Quarter { year, .. } => {
            if year == current_year {
                format!("{} ›", short_label(neighbour))
            } else {
                format!("{neighbour} ›")
            }
        }
        PeriodId::Year(year) => {
            if year == current_year {
                format!("{} ›", short_label(neighbour))
            } else {
                format!("{neighbour} ›")
            }
        }
        PeriodId::AllTime => format!("{} ›", short_label(neighbour)),
    }
}

/// Up label: `↑ 2026-Q4`, `↑ 2026`, `↑ All time`, or `↑ org`.
pub(crate) fn up_label(parent: Option<PeriodId>) -> String {
    match parent {
        Some(PeriodId::AllTime) => "↑ All time".to_string(),
        Some(id) => format!("↑ {id}"),
        None => "↑ org".to_string(),
    }
}

/// Day label: `Mon 2`, with the month on the week's first day and on
/// the 1st of any month (`Mon Nov 2`, `Sun Nov 1`).
pub(crate) fn day_label(date: NaiveDate, is_week_first_day: bool) -> String {
    let weekday = weekday_abbrev(date);
    if is_week_first_day || date.day() == 1 {
        format!("{} {} {}", weekday, month_abbrev(date.month()), date.day())
    } else {
        format!("{weekday} {}", date.day())
    }
}

#[cfg(test)]
mod tests {
    use super::super::calendar::PeriodId;
    use super::*;
    use chrono::NaiveDate;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    #[test]
    fn range_labels_cover_three_shapes() {
        assert_eq!(
            range_label(date(2026, 11, 2), date(2026, 11, 8)),
            "Nov 2 – 8"
        );
        assert_eq!(
            range_label(date(2026, 10, 26), date(2026, 11, 1)),
            "Oct 26 – Nov 1"
        );
        assert_eq!(
            range_label(date(2026, 12, 28), date(2027, 1, 3)),
            "Dec 28, 2026 – Jan 3, 2027"
        );
    }

    #[test]
    fn titles_match_the_contract() {
        let week = PeriodId::Week {
            year: 2026,
            week: 45,
        };
        assert_eq!(title(week), "2026-W45 · Nov 2 – 8");
        let quarter = PeriodId::Quarter {
            year: 2026,
            quarter: 4,
        };
        assert_eq!(
            title(quarter),
            "2026-Q4 · W40–W53 · Sep 28, 2026 – Jan 3, 2027"
        );
        let year = PeriodId::Year(2026);
        assert_eq!(title(year), "2026 · Dec 29, 2025 – Jan 3, 2027 · 53 weeks");
        assert_eq!(title(PeriodId::AllTime), "All time");
    }

    #[test]
    fn nav_labels_add_the_year_across_iso_years() {
        let next_quarter = PeriodId::Quarter {
            year: 2027,
            quarter: 1,
        };
        assert_eq!(next_label(next_quarter, 2026), "2027-Q1 ›");
        let prev_week = PeriodId::Week {
            year: 2026,
            week: 44,
        };
        assert_eq!(prev_label(prev_week, 2026), "‹ W44");
        assert_eq!(
            up_label(Some(PeriodId::Quarter {
                year: 2026,
                quarter: 4
            })),
            "↑ 2026-Q4"
        );
        assert_eq!(up_label(Some(PeriodId::AllTime)), "↑ All time");
        assert_eq!(up_label(None), "↑ org");
    }

    #[test]
    fn day_labels_name_the_month_on_first_days() {
        assert_eq!(day_label(date(2026, 11, 2), true), "Mon Nov 2");
        assert_eq!(day_label(date(2026, 11, 3), false), "Tue 3");
        assert_eq!(day_label(date(2026, 11, 1), false), "Sun Nov 1");
    }
}
