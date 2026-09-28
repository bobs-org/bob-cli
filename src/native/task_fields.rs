//! Shared Obsidian inline-field scanning for task lines.
//!
//! Task lines carry `[key:: value]` and `(key:: value)` fields anywhere on
//! the line, with any spacing around `::`. This is the same recognition the
//! `<ctrl+shift+enter>` toggle, `bob projects sync`, and the picker use.
//! The scanner generalizes the `scheduled` matching formerly owned by
//! `capture_task_toggle` so every consumer shares one implementation.

#![allow(dead_code)]

use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use regex::Regex;

/// Any `[key:: value]` or `(key:: value)` field, with the key matched
/// case-sensitively and any spacing allowed around `::`.
static INLINE_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\[([A-Za-z][A-Za-z0-9_-]*)\s*::\s*([^\]\n]*)\]|\(([A-Za-z][A-Za-z0-9_-]*)\s*::\s*([^)\n]*)\)",
    )
    .expect("valid inline field regex")
});

/// One recognized inline field on a task line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineField {
    /// Byte range of the whole field, delimiters included.
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// Byte range of the trimmed value inside the field.
    pub(crate) value_start: usize,
    pub(crate) value_end: usize,
    /// The trimmed value.
    pub(crate) value: String,
}

/// Every `key` field on `line`, in line order.
pub(crate) fn inline_fields(line: &str, key: &str) -> Vec<InlineField> {
    INLINE_FIELD_RE
        .captures_iter(line)
        .filter_map(|captures| {
            let whole = captures.get(0).expect("whole match");
            let (matched_key, raw) = captures
                .get(1)
                .and_then(|group| {
                    captures.get(2).map(|value| (group.as_str(), value))
                })
                .or_else(|| {
                    captures.get(3).and_then(|group| {
                        captures.get(4).map(|value| (group.as_str(), value))
                    })
                })?;
            (matched_key == key).then(|| {
                let raw_text = raw.as_str();
                let leading = raw_text.len().saturating_sub(
                    raw_text.trim_start_matches([' ', '\t']).len(),
                );
                let trimmed = raw_text.trim_start_matches([' ', '\t']);
                let trailing = trimmed.len().saturating_sub(
                    trimmed.trim_end_matches([' ', '\t']).len(),
                );
                let value_start = raw.start() + leading;
                let value_end = raw.end() - trailing;
                InlineField {
                    start: whole.start(),
                    end: whole.end(),
                    value_start,
                    value_end,
                    value: line[value_start..value_end].to_string(),
                }
            })
        })
        .collect()
}

/// Whether `line` carries any field named by `keys`.
pub(crate) fn has_any_field(line: &str, keys: &[&str]) -> bool {
    INLINE_FIELD_RE.captures_iter(line).any(|captures| {
        captures
            .get(1)
            .map(|group| group.as_str())
            .or_else(|| captures.get(3).map(|group| group.as_str()))
            .is_some_and(|matched_key| keys.contains(&matched_key))
    })
}

/// A strict `YYYY-MM-DD` calendar date; anything else (including
/// out-of-range months and days) is `None`.
pub(crate) fn parse_strict_calendar_date(value: &str) -> Option<NaiveDate> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let all_digits = |range: std::ops::Range<usize>| {
        bytes[range].iter().all(u8::is_ascii_digit)
    };
    if !all_digits(0..4) || !all_digits(5..7) || !all_digits(8..10) {
        return None;
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

/// Format a date as `YYYY-MM-DD`.
pub(crate) fn format_calendar_date(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bracket_and_paren_forms() {
        let line = "- [ ] A [priority:: high] (scheduled:: 2026-09-10) #task";
        let fields = inline_fields(line, "priority");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, "high");
        let scheduled = inline_fields(line, "scheduled");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].value, "2026-09-10");
    }

    #[test]
    fn matches_with_no_space_after_the_colons() {
        let line = "- [ ] A [scheduled::2026-09-10] #task";
        let fields = inline_fields(line, "scheduled");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, "2026-09-10");
        assert_eq!(
            &line[fields[0].start..fields[0].end],
            "[scheduled::2026-09-10]"
        );
        assert_eq!(
            &line[fields[0].value_start..fields[0].value_end],
            "2026-09-10"
        );
    }

    #[test]
    fn matches_with_spacing_around_the_colons() {
        let line = "- [ ] A [scheduled ::  2026-09-10 ] #task";
        let fields = inline_fields(line, "scheduled");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, "2026-09-10");
        assert_eq!(
            &line[fields[0].value_start..fields[0].value_end],
            "2026-09-10"
        );
    }

    #[test]
    fn reports_exact_byte_ranges() {
        let line = "- [ ] Do it [priority:: high] #task";
        let fields = inline_fields(line, "priority");
        assert_eq!(fields.len(), 1);
        let field = &fields[0];
        assert_eq!(&line[field.start..field.end], "[priority:: high]");
        assert_eq!(&line[field.value_start..field.value_end], "high");
        assert_eq!(field.start, 12);
        assert_eq!(field.end, 29);
        assert_eq!(field.value_start, 24);
        assert_eq!(field.value_end, 28);
    }

    #[test]
    fn reports_duplicate_fields_in_order() {
        let line = "- [ ] A [scheduled:: 2026-09-10] (scheduled:: 2026-09-11)";
        let fields = inline_fields(line, "scheduled");
        assert_eq!(
            fields
                .iter()
                .map(|field| field.value.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-09-10", "2026-09-11"]
        );
        assert!(fields[0].end <= fields[1].start);
    }

    #[test]
    fn filters_by_key() {
        let line = "- [ ] A [priority:: high] [scheduled:: 2026-09-10]";
        assert_eq!(inline_fields(line, "due").len(), 0);
        assert_eq!(inline_fields(line, "priority").len(), 1);
        assert_eq!(inline_fields(line, "scheduled").len(), 1);
    }

    #[test]
    fn key_match_is_case_sensitive() {
        let line = "- [ ] A [Scheduled:: 2026-09-10]";
        assert_eq!(inline_fields(line, "scheduled").len(), 0);
        assert_eq!(inline_fields(line, "Scheduled").len(), 1);
    }

    #[test]
    fn has_any_field_checks_several_keys() {
        let line = "- [ ] A [due:: 2026-09-10] #task";
        assert!(has_any_field(line, &["due", "repeat"]));
        assert!(has_any_field(line, &["due"]));
        assert!(!has_any_field(line, &["repeat", "scheduled"]));
        assert!(!has_any_field(
            "- [ ] Plain #task",
            &["due", "repeat", "priority", "scheduled"]
        ));
    }

    #[test]
    fn strict_date_rejects_non_dates() {
        assert!(parse_strict_calendar_date("2026-09-10").is_some());
        assert!(parse_strict_calendar_date("2026-9-10").is_none());
        assert!(parse_strict_calendar_date("2026-13-10").is_none());
        assert!(parse_strict_calendar_date("2026-09-32").is_none());
        assert!(parse_strict_calendar_date("tomorrow").is_none());
        assert!(parse_strict_calendar_date(" 2026-09-10").is_none());
        assert!(parse_strict_calendar_date("2026-09-10 ").is_none());
    }

    #[test]
    fn strict_date_rejects_impossible_february() {
        assert!(parse_strict_calendar_date("2026-02-29").is_none());
        assert!(parse_strict_calendar_date("2024-02-29").is_some());
    }

    #[test]
    fn format_round_trips_through_strict_parse() {
        let date = parse_strict_calendar_date("2026-03-05").expect("valid");
        assert_eq!(format_calendar_date(date), "2026-03-05");
    }
}
