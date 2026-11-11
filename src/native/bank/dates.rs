//! Day, month, week, and year argument parsing for `bob bank`.
//!
//! `-D` takes `today`, `yesterday`, `tomorrow`, or `YYYY-MM-DD` (as in
//! `bob gcal list`). `-m` takes `YYYY-MM`, `-w` takes `YYYY-Www` (ISO,
//! Monday first), and `-y` takes `YYYY`. `BOB_NOW` overrides today
//! everywhere.

use chrono::NaiveDate;

use super::grammar::YearMonth;

pub(crate) fn resolve_day(
    value: &str,
    today: NaiveDate,
) -> Result<NaiveDate, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "today" => Ok(today),
        "tomorrow" => today.succ_opt().ok_or_else(|| {
            "tomorrow is out of range".to_owned()
        }),
        "yesterday" => today.pred_opt().ok_or_else(|| {
            "yesterday is out of range".to_owned()
        }),
        value => NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
            format!(
                "invalid day {value:?}; use today, tomorrow, yesterday, or YYYY-MM-DD"
            )
        }),
    }
}

pub(crate) fn parse_month_arg(value: &str) -> Result<YearMonth, String> {
    YearMonth::parse(value.trim())
        .ok_or_else(|| format!("invalid month {value:?}; use YYYY-MM"))
}

pub(crate) fn parse_year_arg(value: &str) -> Result<i32, String> {
    let trimmed = value.trim();
    if trimmed.len() == 4
        && let Ok(year) = trimmed.parse::<i32>()
        && (1900..=2100).contains(&year)
    {
        return Ok(year);
    }
    Err(format!("invalid year {value:?}; use YYYY"))
}

pub(crate) fn parse_week_arg(
    value: &str,
) -> Result<(NaiveDate, NaiveDate), String> {
    let trimmed = value.trim();
    let (year_part, week_part) = trimmed
        .split_once("-W")
        .ok_or_else(|| format!("invalid week {value:?}; use YYYY-Www"))?;
    let year: i32 = year_part
        .parse()
        .map_err(|_| format!("invalid week {value:?}; use YYYY-Www"))?;
    let week: u32 = week_part
        .parse()
        .map_err(|_| format!("invalid week {value:?}; use YYYY-Www"))?;
    if !(1..=53).contains(&week) {
        return Err(format!("invalid week {value:?}; use YYYY-Www"));
    }
    let monday =
        NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon)
            .ok_or_else(|| format!("invalid week {value:?}; use YYYY-Www"))?;
    let sunday = monday
        .succ_opt()
        .and_then(|date| date.succ_opt())
        .and_then(|date| date.succ_opt())
        .and_then(|date| date.succ_opt())
        .and_then(|date| date.succ_opt())
        .and_then(|date| date.succ_opt())
        .ok_or_else(|| format!("invalid week {value:?}; use YYYY-Www"))?;
    Ok((monday, sunday))
}

pub(crate) fn month_range(month: YearMonth) -> (NaiveDate, NaiveDate) {
    let start =
        NaiveDate::from_ymd_opt(month.year, month.month, 1).expect("month");
    let (next_year, next_month) = if month.month == 12 {
        (month.year + 1, 1)
    } else {
        (month.year, month.month + 1)
    };
    let next_start =
        NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("month");
    let end = next_start.pred_opt().expect("month end");
    (start, end)
}

pub(crate) fn year_range(year: i32) -> (NaiveDate, NaiveDate) {
    (
        NaiveDate::from_ymd_opt(year, 1, 1).expect("year"),
        NaiveDate::from_ymd_opt(year, 12, 31).expect("year"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 14).unwrap()
    }

    #[test]
    fn day_words_and_iso() {
        assert_eq!(resolve_day("today", today()).unwrap(), today());
        assert_eq!(
            resolve_day("tomorrow", today()).unwrap(),
            NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()
        );
        assert_eq!(
            resolve_day("yesterday", today()).unwrap(),
            NaiveDate::from_ymd_opt(2026, 11, 13).unwrap()
        );
        assert_eq!(
            resolve_day("2026-11-10", today()).unwrap(),
            NaiveDate::from_ymd_opt(2026, 11, 10).unwrap()
        );
        assert!(resolve_day("nov 10", today()).is_err());
    }

    #[test]
    fn month_week_year_parse() {
        let month = parse_month_arg("2026-11").unwrap();
        assert_eq!(month.as_str(), "2026-11");
        assert!(parse_month_arg("2026-13").is_err());
        let (monday, sunday) = parse_week_arg("2026-W46").unwrap();
        assert_eq!(monday.weekday(), chrono::Weekday::Mon);
        assert_eq!(monday, NaiveDate::from_ymd_opt(2026, 11, 9).unwrap());
        assert_eq!(sunday, NaiveDate::from_ymd_opt(2026, 11, 15).unwrap());
        assert_eq!(parse_year_arg("2026").unwrap(), 2026);
        assert!(parse_year_arg("26").is_err());
    }
}
