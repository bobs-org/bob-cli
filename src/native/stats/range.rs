use chrono::NaiveDate;

/// A configured or requested stats window. [`super::series::resolve_range`]
/// turns this into complete-day bounds, grouping, and the prior window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RangeSpec {
    Days(u32),
    Weeks(u32),
    Months(u32),
    Years(u32),
    All,
    Explicit { start: NaiveDate, end: NaiveDate },
}

impl RangeSpec {
    pub(crate) fn four_weeks() -> Self {
        Self::Weeks(4)
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(invalid_range(text));
        }
        if text.eq_ignore_ascii_case("all") {
            return Ok(Self::All);
        }
        if let Some((start, end)) = text.split_once("..") {
            return parse_explicit(start, end);
        }
        parse_length(text)
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Days(count) => format!("{count}d"),
            Self::Weeks(count) => format!("{count}w"),
            Self::Months(count) => format!("{count}m"),
            Self::Years(count) => format!("{count}y"),
            Self::All => "all".to_string(),
            Self::Explicit { start, end } => format!(
                "{}..{}",
                start.format("%Y-%m-%d"),
                end.format("%Y-%m-%d")
            ),
        }
    }

    pub(crate) fn preset_label(&self) -> Option<String> {
        match self {
            Self::Explicit { .. } => None,
            _ => Some(self.label()),
        }
    }
}

fn parse_length(text: &str) -> Result<RangeSpec, String> {
    let mut chars = text.chars();
    let Some(unit) = chars.next_back() else {
        return Err(invalid_range(text));
    };
    let number = chars.as_str();
    if number.is_empty()
        || !number.bytes().all(|byte| byte.is_ascii_digit())
        || number.starts_with('0')
    {
        return Err(invalid_range(text));
    }
    let count = number.parse::<u32>().map_err(|_| invalid_range(text))?;
    if count == 0 {
        return Err(invalid_range(text));
    }
    match unit {
        'd' | 'D' => Ok(RangeSpec::Days(count)),
        'w' | 'W' => Ok(RangeSpec::Weeks(count)),
        'm' | 'M' => Ok(RangeSpec::Months(count)),
        'y' | 'Y' => Ok(RangeSpec::Years(count)),
        _ => Err(invalid_range(text)),
    }
}

fn parse_explicit(start: &str, end: &str) -> Result<RangeSpec, String> {
    let start_text = start.trim();
    let end_text = end.trim();
    let start = parse_iso_date(start_text).ok_or_else(|| {
        format!(
            "must be inclusive YYYY-MM-DD..YYYY-MM-DD; got {start_text}..{end_text}"
        )
    })?;
    let end = parse_iso_date(end_text).ok_or_else(|| {
        format!(
            "must be inclusive YYYY-MM-DD..YYYY-MM-DD; got {start_text}..{end_text}"
        )
    })?;
    if start > end {
        return Err(format!(
            "must be an inclusive interval; {start_text} is after {end_text}"
        ));
    }
    Ok(RangeSpec::Explicit { start, end })
}

pub(crate) fn parse_iso_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}

fn invalid_range(text: &str) -> String {
    format!(
        "must be a preset (7d, 4w, 3m, 1y, all), a positive Nd/Nw/Nm/Ny \
         length, or inclusive YYYY-MM-DD..YYYY-MM-DD; got {text:?}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_presets_and_lengths() {
        assert_eq!(RangeSpec::parse("4w").unwrap(), RangeSpec::Weeks(4));
        assert_eq!(RangeSpec::parse(" 7d ").unwrap(), RangeSpec::Days(7));
        assert_eq!(RangeSpec::parse("3m").unwrap(), RangeSpec::Months(3));
        assert_eq!(RangeSpec::parse("1y").unwrap(), RangeSpec::Years(1));
        assert_eq!(RangeSpec::parse("all").unwrap(), RangeSpec::All);
        assert_eq!(RangeSpec::parse("13w").unwrap(), RangeSpec::Weeks(13));
        assert_eq!(RangeSpec::parse("52W").unwrap(), RangeSpec::Weeks(52));
    }

    #[test]
    fn parses_explicit_inclusive_dates() {
        let range = RangeSpec::parse("2026-09-13..2026-10-10").unwrap();
        assert_eq!(
            range,
            RangeSpec::Explicit {
                start: NaiveDate::from_ymd_opt(2026, 9, 13).unwrap(),
                end: NaiveDate::from_ymd_opt(2026, 10, 10).unwrap(),
            }
        );
    }

    #[test]
    fn rejects_zero_negative_overflow_and_inverted() {
        for text in [
            "",
            "0d",
            "0w",
            "-4w",
            "04w",
            "4",
            "4x",
            "1.5w",
            "4294967296d",
            "2026-13-01..2026-10-10",
            "2026-02-30..2026-03-01",
            "2026-10-10..2026-09-13",
            "yesterday",
        ] {
            RangeSpec::parse(text).expect_err(&format!("must reject {text:?}"));
        }
    }

    #[test]
    fn labels_round_trip_presets() {
        assert_eq!(RangeSpec::four_weeks().label(), "4w");
        assert_eq!(RangeSpec::All.label(), "all");
        assert_eq!(
            RangeSpec::four_weeks().preset_label().as_deref(),
            Some("4w")
        );
        let explicit = RangeSpec::parse("2026-01-01..2026-01-02").unwrap();
        assert_eq!(explicit.preset_label(), None);
        assert_eq!(explicit.label(), "2026-01-01..2026-01-02");
    }
}
