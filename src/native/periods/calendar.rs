//! ISO period calendar: weeks, quarters, years, and all time.
//!
//! The contract lives in `docs/periods.md` §1. Weeks are ISO weeks
//! (Monday–Sunday); quarters are 13 whole ISO weeks (Q4 takes W53 when it
//! exists); years are ISO week-years; all time is a singleton.

use std::fmt;
use std::path::PathBuf;

use chrono::{Datelike, NaiveDate, Weekday};

/// A period above the day: an ISO week, a 13-week quarter, an ISO
/// week-year, or the all-time singleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PeriodId {
    Week { year: i32, week: u32 },
    Quarter { year: i32, quarter: u32 },
    Year(i32),
    AllTime,
}

/// Whether a period lies ahead of, around, or behind `today`.
///
/// Later phases (report) reuse the state machine; core keeps it here so
/// the calendar owns the definition.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeriodState {
    Future,
    Current,
    Closed,
}

/// The accepted id forms, named in usage errors.
pub(crate) const ACCEPTED_FORMS: &str =
    "a week 2026-W45, a quarter 2026-Q4, a year 2026, all, \
     or a vault-relative note path such as 2026/2026-W45.md";

/// Number of ISO weeks in `year` (52 or 53).
pub(crate) fn weeks_in_year(year: i32) -> u32 {
    let dec28 =
        NaiveDate::from_ymd_opt(year, 12, 28).expect("Dec 28 always exists");
    dec28.iso_week().week()
}

/// Quarter (1–4) containing ISO week `week`.
pub(crate) fn quarter_of_week(week: u32) -> u32 {
    ((week - 1) / 13 + 1).min(4)
}

impl PeriodId {
    /// Parse every form in §1: canonical ids (case-insensitive),
    /// `all_time`/`all-time`, and vault-relative note paths with or
    /// without `.md`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(invalid_period(text));
        }
        let mut stem = trimmed;
        // Strip a trailing .md (case-insensitive).
        if stem.len() > 3 && stem[stem.len() - 3..].eq_ignore_ascii_case(".md")
        {
            stem = &stem[..stem.len() - 3];
        }
        // A vault-relative path parses as its file stem.
        if let Some(slash) = stem.rfind('/') {
            stem = &stem[slash + 1..];
        }
        // Also accept a lone backslash separator on any host.
        if let Some(slash) = stem.rfind('\\') {
            stem = &stem[slash + 1..];
        }
        let stem = stem.trim();
        if stem.is_empty() {
            return Err(invalid_period(text));
        }
        let lower = stem.to_ascii_lowercase();
        if lower == "all" || lower == "all_time" || lower == "all-time" {
            return Ok(Self::AllTime);
        }
        if let Some(parsed) = parse_year(&lower) {
            return Ok(parsed);
        }
        if let Some(parsed) = parse_week(&lower) {
            return Ok(parsed);
        }
        if let Some(parsed) = parse_quarter(&lower) {
            return Ok(parsed);
        }
        Err(invalid_period(text))
    }

    /// The week containing `date`.
    pub(crate) fn week_for_date(date: NaiveDate) -> Self {
        let iso = date.iso_week();
        Self::Week {
            year: iso.year(),
            week: iso.week(),
        }
    }

    /// The quarter containing `date`.
    pub(crate) fn quarter_for_date(date: NaiveDate) -> Self {
        let iso = date.iso_week();
        Self::Quarter {
            year: iso.year(),
            quarter: quarter_of_week(iso.week()),
        }
    }

    /// The ISO week-year containing `date`.
    pub(crate) fn year_for_date(date: NaiveDate) -> Self {
        Self::Year(date.iso_week().year())
    }

    /// The full chain for `date`: week, quarter, year, all time.
    pub(crate) fn chain_for_date(date: NaiveDate) -> Vec<Self> {
        vec![
            Self::week_for_date(date),
            Self::quarter_for_date(date),
            Self::year_for_date(date),
            Self::AllTime,
        ]
    }

    /// First calendar day of the period, or `None` for all time.
    pub(crate) fn start(self) -> Option<NaiveDate> {
        match self {
            Self::Week { year, week } => {
                NaiveDate::from_isoywd_opt(year, week, Weekday::Mon)
            }
            Self::Quarter { year, quarter } => {
                let first = (quarter - 1) * 13 + 1;
                NaiveDate::from_isoywd_opt(year, first, Weekday::Mon)
            }
            Self::Year(year) => {
                NaiveDate::from_isoywd_opt(year, 1, Weekday::Mon)
            }
            Self::AllTime => None,
        }
    }

    /// Last calendar day of the period, or `None` for all time.
    pub(crate) fn end(self) -> Option<NaiveDate> {
        match self {
            Self::Week { year, week } => {
                NaiveDate::from_isoywd_opt(year, week, Weekday::Mon).and_then(
                    |monday| monday.checked_add_days(chrono::Days::new(6)),
                )
            }
            Self::Quarter { year, quarter } => {
                let last = quarterly_last_week(year, quarter);
                NaiveDate::from_isoywd_opt(year, last, Weekday::Mon).and_then(
                    |monday| monday.checked_add_days(chrono::Days::new(6)),
                )
            }
            Self::Year(year) => {
                let last = weeks_in_year(year);
                NaiveDate::from_isoywd_opt(year, last, Weekday::Mon).and_then(
                    |monday| monday.checked_add_days(chrono::Days::new(6)),
                )
            }
            Self::AllTime => None,
        }
    }

    /// The next period up: a week climbs to its quarter, a quarter to
    /// its year, a year to all time.
    pub(crate) fn parent(self) -> Option<Self> {
        match self {
            Self::Week { year, week } => Some(Self::Quarter {
                year,
                quarter: quarter_of_week(week),
            }),
            Self::Quarter { year, .. } => Some(Self::Year(year)),
            Self::Year(_) => Some(Self::AllTime),
            Self::AllTime => None,
        }
    }

    /// The previous same-kind period, or `None` past 1970 / for all time.
    pub(crate) fn prev(self) -> Option<Self> {
        match self {
            Self::Week { year, week } => {
                if week > 1 {
                    Some(Self::Week {
                        year,
                        week: week - 1,
                    })
                } else if year > 1970 {
                    Some(Self::Week {
                        year: year - 1,
                        week: weeks_in_year(year - 1),
                    })
                } else {
                    None
                }
            }
            Self::Quarter { year, quarter } => {
                if quarter > 1 {
                    Some(Self::Quarter {
                        year,
                        quarter: quarter - 1,
                    })
                } else if year > 1970 {
                    Some(Self::Quarter {
                        year: year - 1,
                        quarter: 4,
                    })
                } else {
                    None
                }
            }
            Self::Year(year) => {
                if year > 1970 {
                    Some(Self::Year(year - 1))
                } else {
                    None
                }
            }
            Self::AllTime => None,
        }
    }

    /// The next same-kind period, or `None` past 2999 / for all time.
    pub(crate) fn next(self) -> Option<Self> {
        match self {
            Self::Week { year, week } => {
                if week < weeks_in_year(year) {
                    Some(Self::Week {
                        year,
                        week: week + 1,
                    })
                } else if year < 2999 {
                    Some(Self::Week {
                        year: year + 1,
                        week: 1,
                    })
                } else {
                    None
                }
            }
            Self::Quarter { year, quarter } => {
                if quarter < 4 {
                    Some(Self::Quarter {
                        year,
                        quarter: quarter + 1,
                    })
                } else if year < 2999 {
                    Some(Self::Quarter {
                        year: year + 1,
                        quarter: 1,
                    })
                } else {
                    None
                }
            }
            Self::Year(year) => {
                if year < 2999 {
                    Some(Self::Year(year + 1))
                } else {
                    None
                }
            }
            Self::AllTime => None,
        }
    }

    /// Vault-relative note path (§1).
    pub(crate) fn path(self) -> PathBuf {
        match self {
            Self::Week { year, week } => {
                PathBuf::from(format!("{year:04}/{year:04}-W{week:02}.md"))
            }
            Self::Quarter { year, quarter } => {
                PathBuf::from(format!("{year:04}/{year:04}-Q{quarter}.md"))
            }
            Self::Year(year) => PathBuf::from(format!("{year:04}.md")),
            Self::AllTime => PathBuf::from("all_time.md"),
        }
    }

    /// Link target for `path` (the same path without `.md`).
    pub(crate) fn link_target(self) -> String {
        match self {
            Self::Week { year, week } => {
                format!("{year:04}/{year:04}-W{week:02}")
            }
            Self::Quarter { year, quarter } => {
                format!("{year:04}/{year:04}-Q{quarter}")
            }
            Self::Year(year) => format!("{year:04}"),
            Self::AllTime => "all_time".to_string(),
        }
    }

    /// Whether the period is ahead of, around, or behind `today`.
    /// All time is always current.
    #[allow(dead_code)]
    pub(crate) fn state(self, today: NaiveDate) -> PeriodState {
        let (Some(start), Some(end)) = (self.start(), self.end()) else {
            return PeriodState::Current;
        };
        if today < start {
            PeriodState::Future
        } else if today > end {
            PeriodState::Closed
        } else {
            PeriodState::Current
        }
    }

    /// The seven calendar days of a week (Monday first).
    pub(crate) fn week_dates(self) -> Vec<NaiveDate> {
        match self {
            Self::Week { .. } => {
                let Some(start) = self.start() else {
                    return Vec::new();
                };
                (0..7)
                    .filter_map(|offset| {
                        start.checked_add_days(chrono::Days::new(offset))
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// The weeks of a quarter, in order.
    pub(crate) fn quarter_weeks(self) -> Vec<Self> {
        match self {
            Self::Quarter { year, quarter } => {
                let first = (quarter - 1) * 13 + 1;
                let last = quarterly_last_week(year, quarter);
                (first..=last)
                    .map(|week| Self::Week { year, week })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// The four quarters of a year, in order.
    pub(crate) fn year_quarters(self) -> Vec<Self> {
        match self {
            Self::Year(year) => (1..=4)
                .map(|quarter| Self::Quarter { year, quarter })
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn quarterly_last_week(year: i32, quarter: u32) -> u32 {
    if quarter < 4 {
        quarter * 13
    } else {
        weeks_in_year(year)
    }
}

fn parse_year(lower: &str) -> Option<PeriodId> {
    if lower.len() != 4 || !lower.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year: i32 = lower.parse().ok()?;
    if (1970..=2999).contains(&year) {
        Some(PeriodId::Year(year))
    } else {
        None
    }
}

fn parse_week(lower: &str) -> Option<PeriodId> {
    let (year_text, week_text) = lower.split_once("-w")?;
    if year_text.len() != 4
        || !year_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    if week_text.is_empty()
        || week_text.len() > 2
        || !week_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let year: i32 = year_text.parse().ok()?;
    let week: u32 = week_text.parse().ok()?;
    if !(1970..=2999).contains(&year) {
        return None;
    }
    if week == 0 || week > weeks_in_year(year) {
        return None;
    }
    Some(PeriodId::Week { year, week })
}

fn parse_quarter(lower: &str) -> Option<PeriodId> {
    let (year_text, quarter_text) = lower.split_once("-q")?;
    if year_text.len() != 4
        || !year_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    if quarter_text.len() != 1
        || !quarter_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let year: i32 = year_text.parse().ok()?;
    let quarter: u32 = quarter_text.parse().ok()?;
    if !(1970..=2999).contains(&year) {
        return None;
    }
    if !(1..=4).contains(&quarter) {
        return None;
    }
    Some(PeriodId::Quarter { year, quarter })
}

fn invalid_period(text: &str) -> String {
    format!("invalid period {text:?}: expected {ACCEPTED_FORMS}")
}

impl fmt::Display for PeriodId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Week { year, week } => {
                write!(f, "{year:04}-W{week:02}")
            }
            Self::Quarter { year, quarter } => {
                write!(f, "{year:04}-Q{quarter}")
            }
            Self::Year(year) => write!(f, "{year:04}"),
            Self::AllTime => write!(f, "all"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    #[test]
    fn golden_calendar_facts() {
        // 2025-12-29 → 2026/2026-W01 → 2026-Q1 → 2026 → all_time.
        let probe = date(2025, 12, 29);
        assert_eq!(
            PeriodId::week_for_date(probe),
            PeriodId::Week {
                year: 2026,
                week: 1
            }
        );
        assert_eq!(
            PeriodId::week_for_date(probe).path(),
            PathBuf::from("2026/2026-W01.md")
        );
        assert_eq!(
            PeriodId::quarter_for_date(probe),
            PeriodId::Quarter {
                year: 2026,
                quarter: 1
            }
        );
        assert_eq!(PeriodId::year_for_date(probe), PeriodId::Year(2026));

        // 2026-W53 = Dec 28, 2026 – Jan 3, 2027, in 2026-Q4.
        let w53 = PeriodId::Week {
            year: 2026,
            week: 53,
        };
        assert_eq!(w53.start(), Some(date(2026, 12, 28)));
        assert_eq!(w53.end(), Some(date(2027, 1, 3)));
        assert_eq!(
            w53.parent(),
            Some(PeriodId::Quarter {
                year: 2026,
                quarter: 4
            })
        );

        // 2027-W01 starts 2027-01-04.
        let w01_2027 = PeriodId::Week {
            year: 2027,
            week: 1,
        };
        assert_eq!(w01_2027.start(), Some(date(2027, 1, 4)));

        // 2026-Q4 = W40–W53 = 14 weeks.
        let q4 = PeriodId::Quarter {
            year: 2026,
            quarter: 4,
        };
        assert_eq!(q4.quarter_weeks().len(), 14);
        assert_eq!(q4.start(), Some(date(2026, 9, 28)));
        assert_eq!(q4.end(), Some(date(2027, 1, 3)));

        // ISO 2026 quarters run Dec 29 – Mar 29, Mar 30 – Jun 28,
        // Jun 29 – Sep 27, and Sep 28 – Jan 3.
        let bounds = [
            (1, date(2025, 12, 29), date(2026, 3, 29)),
            (2, date(2026, 3, 30), date(2026, 6, 28)),
            (3, date(2026, 6, 29), date(2026, 9, 27)),
            (4, date(2026, 9, 28), date(2027, 1, 3)),
        ];
        for (quarter, start, end) in bounds {
            let id = PeriodId::Quarter {
                year: 2026,
                quarter,
            };
            assert_eq!(id.start(), Some(start), "Q{quarter} start");
            assert_eq!(id.end(), Some(end), "Q{quarter} end");
        }
    }

    #[test]
    fn fifty_three_week_year_count_is_eighteen() {
        let long_years: Vec<i32> = (2000..=2100)
            .filter(|year| weeks_in_year(*year) == 53)
            .collect();
        assert_eq!(long_years.len(), 18);
        for expected in [2026, 2032, 2037, 2043, 2048] {
            assert!(
                long_years.contains(&expected),
                "expected {expected} in {long_years:?}"
            );
        }
    }

    #[test]
    fn parses_every_accepted_form() {
        assert_eq!(
            PeriodId::parse("2026-W45").expect("week"),
            PeriodId::Week {
                year: 2026,
                week: 45
            }
        );
        assert_eq!(
            PeriodId::parse("2026-w45").expect("lower week"),
            PeriodId::Week {
                year: 2026,
                week: 45
            }
        );
        assert_eq!(
            PeriodId::parse("2026-Q4").expect("quarter"),
            PeriodId::Quarter {
                year: 2026,
                quarter: 4
            }
        );
        assert_eq!(
            PeriodId::parse("2026").expect("year"),
            PeriodId::Year(2026)
        );
        assert_eq!(PeriodId::parse("all").expect("all"), PeriodId::AllTime);
        assert_eq!(
            PeriodId::parse("all_time").expect("all_time"),
            PeriodId::AllTime
        );
        assert_eq!(
            PeriodId::parse("ALL-TIME").expect("all-time"),
            PeriodId::AllTime
        );
        assert_eq!(
            PeriodId::parse("2026/2026-W45.md").expect("week path"),
            PeriodId::Week {
                year: 2026,
                week: 45
            }
        );
        assert_eq!(
            PeriodId::parse("2026/2026-W45").expect("week path bare"),
            PeriodId::Week {
                year: 2026,
                week: 45
            }
        );
        assert_eq!(
            PeriodId::parse("2026.md").expect("year path"),
            PeriodId::Year(2026)
        );
        assert_eq!(
            PeriodId::parse("all_time.md").expect("all-time path"),
            PeriodId::AllTime
        );
        assert!(PeriodId::parse("2026-W54").is_err());
        assert!(PeriodId::parse("2026-Q5").is_err());
        assert!(PeriodId::parse("1969").is_err());
        assert!(PeriodId::parse("tomorrow").is_err());
    }

    #[test]
    fn week_path_uses_iso_week_year() {
        // Dec 29, 2025 is in ISO 2026-W01, filed under 2026/.
        let week = PeriodId::week_for_date(date(2025, 12, 29));
        assert_eq!(week.path(), PathBuf::from("2026/2026-W01.md"));
        assert_eq!(week.link_target(), "2026/2026-W01");
    }

    #[test]
    fn moment_fixture_agrees_with_chrono() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture =
            manifest.join("tests/fixtures/periods/moment_iso_weeks.tsv");
        let Ok(contents) = std::fs::read_to_string(&fixture) else {
            // The fixture ships with the tree; a missing file fails loudly.
            panic!("missing moment fixture: {}", fixture.display());
        };
        let mut rows = 0;
        for line in contents.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let (day_text, week_path) =
                line.split_once('\t').expect("two TSV columns");
            let day = NaiveDate::parse_from_str(day_text.trim(), "%Y%m%d")
                .expect("YYYYMMDD date");
            let week = PeriodId::week_for_date(day);
            let expected = format!("{}/{}", week_path.trim(), "");
            let actual = format!("{}/", week.link_target());
            assert_eq!(actual, expected, "moment row {day_text}");
            rows += 1;
        }
        assert!(rows > 1000, "fixture should cover 2000–2100");
    }
}
