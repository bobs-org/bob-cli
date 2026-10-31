//! The Bob-owned Events stamp: one italic line under `## Events`.
//!
//! Grammar: `*↻ synced <when> · [<account>](<day-url>)*`
//!
//! The stamp records the last sync run. Every run at a new minute
//! rewrites it. The painter never marks it stale.

use std::ops::Range;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use crate::native::{event_task, markdown};

use super::{render, GcalError};

/// Minutes after which today's stamp would paint as stale.
/// `None` because sync runs on demand, so the painter never marks a
/// stamp stale.
#[allow(dead_code)] // plugin-paint reads this contract; CLI never paints.
pub(crate) const STAMP_STALE_AFTER_MINUTES: Option<i64> = None;

/// One parsed Events stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub account: String,
    pub synced_at: NaiveDateTime,
}

impl Stamp {
    /// The `<when>` fragment the note and the human echo share.
    pub(crate) fn when_text(&self, note_day: NaiveDate) -> String {
        if self.synced_at.date() == note_day {
            self.synced_at.format("%H:%M").to_string()
        } else {
            self.synced_at.format("%a %Y-%m-%d %H:%M").to_string()
        }
    }

    /// Dim human echo: `↻ synced 14:05 · you@example.com` (no URL).
    pub(crate) fn echo(&self, note_day: NaiveDate) -> String {
        format!("↻ synced {} · {}", self.when_text(note_day), self.account)
    }

    pub(crate) fn json_synced_at(&self) -> String {
        self.synced_at.format("%Y-%m-%dT%H:%M").to_string()
    }
}

/// Truncate `now` to the minute the stamp writes.
pub(crate) fn truncate_to_minute(now: NaiveDateTime) -> NaiveDateTime {
    now.with_second(0)
        .and_then(|dt| dt.with_nanosecond(0))
        .unwrap_or(now)
}

/// Canonical stamp line for this account, time, and note day.
pub(crate) fn render(
    account: &str,
    synced_at: NaiveDateTime,
    note_day: NaiveDate,
) -> String {
    let stamp = Stamp {
        account: account.to_string(),
        synced_at: truncate_to_minute(synced_at),
    };
    let escaped = escape_account(account);
    let url = day_url(note_day, account);
    format!(
        "*↻ synced {} · [{escaped}]({url})*",
        stamp.when_text(note_day)
    )
}

/// Google Calendar deep link for the note's day in this account.
pub(crate) fn day_url(note_day: NaiveDate, account: &str) -> String {
    format!(
        "https://calendar.google.com/calendar/r/day/{}/{}/{}?authuser={}",
        note_day.year(),
        note_day.month(),
        note_day.day(),
        render::percent_encode_query(account),
    )
}

/// Indexes of recognized stamp lines inside the Events section.
///
/// A stamp is unfenced, not a list item or heading, and after trimming
/// leading whitespace and leading `*` / `_` starts with `↻`.
pub(crate) fn recognize(
    lines: &[&str],
    events_range: Range<usize>,
) -> Vec<usize> {
    let mut found = Vec::new();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(lines);
    let mut fence = None;
    for (index, line) in lines.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if let Some(open) = fence {
            if markdown::closes_fence(line, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = markdown::fence_marker(line) {
            fence = Some(open);
            continue;
        }
        if !events_range.contains(&index) {
            continue;
        }
        if is_list_item(line) || markdown::atx_heading(line).is_some() {
            continue;
        }
        let stripped = line.trim_start().trim_start_matches(['*', '_']);
        if stripped.starts_with('↻') {
            found.push(index);
        }
    }
    found
}

/// Parse a recognized line into its account and sync time.
///
/// A recognized line that fails to parse is malformed.
pub(crate) fn parse(line: &str, note_day: NaiveDate) -> Option<Stamp> {
    let trimmed = line.trim();
    let stripped = trimmed.trim_start_matches(['*', '_']).trim_start();
    let rest = stripped.strip_prefix('↻')?.trim_start();
    let rest = rest.strip_prefix("synced")?.trim_start();
    let (synced_at, rest) = parse_when(rest, note_day)?;
    let rest = rest.trim_start().strip_prefix('·')?.trim_start();
    let (account_raw, _url, rest) = split_link(rest)?;
    let rest = rest.trim_end_matches(['*', '_']).trim();
    if !rest.is_empty() {
        return None;
    }
    Some(Stamp {
        account: unescape_account(account_raw),
        synced_at,
    })
}

/// A stamp is due when it is missing, malformed, not canonical, not in
/// the slot, duplicated, or from a different minute than `now`. A stamp
/// from any earlier minute is due. `now` is the minute the caller
/// passes in.
pub(crate) fn is_due(
    lines: &[&str],
    events_range: Range<usize>,
    now: NaiveDateTime,
    note_day: NaiveDate,
) -> bool {
    let recognized = recognize(lines, events_range.clone());
    if recognized.len() != 1 {
        return true;
    }
    let index = recognized[0];
    let Some(parsed) = parse(lines[index], note_day) else {
        return true;
    };
    if parsed.synced_at != now {
        return true;
    }
    if render(&parsed.account, parsed.synced_at, note_day) != lines[index] {
        return true;
    }
    if index != canonical_slot(lines, events_range.clone()) {
        return true;
    }
    if lines.get(index + 1).is_some_and(|line| {
        !line.trim().is_empty()
            && !is_list_item(line)
            && markdown::atx_heading(line).is_none()
    }) {
        return true;
    }
    false
}

/// Index at which the single canonical stamp sits: after the blank
/// line(s) under the heading, before any other section line.
pub(crate) fn canonical_slot(
    lines: &[&str],
    events_range: Range<usize>,
) -> usize {
    let mut slot = events_range.start;
    while slot < events_range.end && lines[slot].trim().is_empty() {
        slot += 1;
    }
    slot
}

/// Refuse a day whose first parsed stamp names a different account.
pub(crate) fn account_guard(
    contents: &str,
    run_account: &str,
    note_day: NaiveDate,
    display_path: &str,
) -> Result<(), GcalError> {
    let lines: Vec<&str> = contents.lines().collect();
    let Some(range) = event_task::events_section_range(&lines) else {
        return Ok(());
    };
    let Some(&index) = recognize(&lines, range).first() else {
        return Ok(());
    };
    let Some(parsed) = parse(lines[index], note_day) else {
        return Ok(());
    };
    if parsed.account.eq_ignore_ascii_case(run_account) {
        return Ok(());
    }
    Err(GcalError::setup(
        "account_changed",
        format!(
            "{display_path} was synced with {}; this run uses {run_account}",
            parsed.account
        ),
    )
    .with_hint(&format!(
        "delete the ↻ line under ## Events to hand this day to \
         {run_account}, or set gcal.account: {}",
        parsed.account
    )))
}

/// CommonMark list item: `- ` / `* ` / `+ ` or an ordered marker.
pub(crate) fn is_list_item(line: &str) -> bool {
    let rest = line.trim_start_matches([' ', '\t']);
    let mut chars = rest.chars();
    match chars.next() {
        Some('-') | Some('*') | Some('+') => {
            chars.next().is_none_or(char::is_whitespace)
        }
        Some(c) if c.is_ascii_digit() => {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            rest.get(digits..).is_some_and(|after| {
                after.starts_with(['.', ')'])
                    && after.get(1..).is_some_and(|tail| {
                        tail.is_empty() || tail.starts_with([' ', '\t'])
                    })
            })
        }
        _ => false,
    }
}

fn parse_when(
    rest: &str,
    note_day: NaiveDate,
) -> Option<(NaiveDateTime, &str)> {
    if rest.starts_with(|ch: char| ch.is_ascii_digit()) {
        parse_clock_when(rest, note_day)
    } else {
        parse_dated_when(rest)
    }
}

fn parse_clock_when(
    rest: &str,
    note_day: NaiveDate,
) -> Option<(NaiveDateTime, &str)> {
    let clock = rest.get(..5)?;
    if clock.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let hour: u32 = clock.get(..2)?.parse().ok()?;
    let minute: u32 = clock.get(3..)?.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    Some((note_day.and_time(time), rest.get(5..)?))
}

fn parse_dated_when(rest: &str) -> Option<(NaiveDateTime, &str)> {
    // `Thu 2026-10-29 21:40` is 20 bytes of ASCII.
    let candidate = rest.get(..20)?;
    let dt =
        NaiveDateTime::parse_from_str(candidate, "%a %Y-%m-%d %H:%M").ok()?;
    Some((dt, rest.get(20..)?))
}

fn split_link(rest: &str) -> Option<(&str, &str, &str)> {
    let rest = rest.strip_prefix('[')?;
    let mut end = None;
    let mut escaped = false;
    for (index, ch) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == ']' {
            end = Some(index);
            break;
        }
    }
    let end = end?;
    let account_raw = rest.get(..end)?;
    let rest = rest.get(end + 1..)?.strip_prefix('(')?;
    let url_end = rest.find(')')?;
    let url = rest.get(..url_end)?;
    let rest = rest.get(url_end + 1..)?;
    Some((account_raw, url, rest))
}

fn escape_account(account: &str) -> String {
    let mut out = String::with_capacity(account.len());
    for ch in account.chars() {
        if matches!(ch, '\\' | '*' | '_' | '[' | ']' | '`' | '<' | '>') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn unescape_account(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            } else {
                out.push('\\');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::event_task::events_section_range;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 30).expect("date")
    }

    fn at(hour: u32, minute: u32) -> NaiveDateTime {
        day().and_hms_opt(hour, minute, 0).expect("time")
    }

    fn lines_of(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    fn events(text: &str) -> (Vec<&str>, Range<usize>) {
        let lines = lines_of(text);
        let range = events_section_range(&lines).expect("Events");
        (lines, range)
    }

    #[test]
    fn same_day_round_trip() {
        let rendered = render("you@example.com", at(14, 5), day());
        assert_eq!(
            rendered,
            "*↻ synced 14:05 · [you@example.com](\
             https://calendar.google.com/calendar/r/day/2026/10/30\
             ?authuser=you%40example.com)*"
        );
        let parsed = parse(&rendered, day()).expect("parse");
        assert_eq!(parsed.account, "you@example.com");
        assert_eq!(parsed.synced_at, at(14, 5));
        assert_eq!(parsed.echo(day()), "↻ synced 14:05 · you@example.com");
        assert_eq!(parsed.json_synced_at(), "2026-10-30T14:05");
    }

    #[test]
    fn other_day_round_trip() {
        let friday = day();
        let thursday = NaiveDate::from_ymd_opt(2026, 10, 29).expect("date");
        let synced = thursday.and_hms_opt(21, 40, 0).expect("time");
        let rendered = render("you@example.com", synced, friday);
        assert!(
            rendered.starts_with("*↻ synced Thu 2026-10-29 21:40 · "),
            "{rendered}"
        );
        let parsed = parse(&rendered, friday).expect("parse");
        assert_eq!(parsed.synced_at, synced);
        assert_eq!(
            parsed.echo(friday),
            "↻ synced Thu 2026-10-29 21:40 · you@example.com"
        );
    }

    #[test]
    fn escaped_account_round_trips() {
        let account = "first_last+cal@x.com";
        let rendered = render(account, at(12, 0), day());
        assert!(rendered.contains("first\\_last+cal@x.com"), "{rendered}");
        assert!(
            rendered.contains("authuser=first_last%2Bcal%40x.com"),
            "{rendered}"
        );
        let parsed = parse(&rendered, day()).expect("parse");
        assert_eq!(parsed.account, account);
    }

    #[test]
    fn malformed_recognized_line_does_not_parse() {
        assert!(parse("*↻ not a stamp*", day()).is_none());
        assert!(parse("*↻ synced 14:05*", day()).is_none());
        assert!(parse("*↻ synced 14:05 · you@example.com*", day()).is_none());
    }

    #[test]
    fn recognize_skips_list_rows_fences_and_outside_events() {
        let text = concat!(
            "↻ outside\n",
            "## Events\n",
            "\n",
            "*↻ synced 12:00 · [you@example.com](https://example.com)*\n",
            "- ↻ list row\n",
            "```\n",
            "↻ fenced\n",
            "```\n",
            "## Pomodoros\n",
            "*↻ synced 12:00 · [you@example.com](https://example.com)*\n",
        );
        let (lines, range) = events(text);
        assert_eq!(recognize(&lines, range), vec![3]);
    }

    #[test]
    fn underscore_emphasis_is_recognized() {
        let text = concat!(
            "## Events\n",
            "\n",
            "_↻ synced 12:00 · [you@example.com](https://example.com)_\n",
            "## Pomodoros\n",
        );
        let (lines, range) = events(text);
        assert_eq!(recognize(&lines, range), vec![2]);
        assert!(parse(lines[2], day()).is_some());
    }

    #[test]
    fn missing_duplicate_future_and_edit_are_due() {
        let note_day = day();
        let now = at(12, 0);
        let canonical = render("you@example.com", now, note_day);
        let with_stamp = format!("## Events\n\n{canonical}\n## Pomodoros\n");
        let (lines, range) = events(&with_stamp);
        assert!(!is_due(&lines, range.clone(), now, note_day));

        let missing = "## Events\n\n- row\n## Pomodoros\n";
        let (lines, range) = events(missing);
        assert!(is_due(&lines, range, now, note_day));

        let edited = with_stamp.replace("12:00", "noon");
        let (lines, range) = events(&edited);
        assert!(is_due(&lines, range, now, note_day));

        let future = render("you@example.com", at(12, 1), note_day);
        let text = format!("## Events\n\n{future}\n## Pomodoros\n");
        let (lines, range) = events(&text);
        assert!(is_due(&lines, range, now, note_day));

        let duplicated =
            format!("## Events\n\n{canonical}\n{canonical}\n## Pomodoros\n");
        let (lines, range) = events(&duplicated);
        assert!(is_due(&lines, range, now, note_day));
    }

    #[test]
    fn older_stamp_is_due() {
        let note_day = day();
        let written = at(12, 0);
        let canonical = render("you@example.com", written, note_day);
        let text = format!("## Events\n\n{canonical}\n## Pomodoros\n");
        let (lines, range) = events(&text);
        assert!(!is_due(&lines, range.clone(), written, note_day));
        assert!(is_due(&lines, range.clone(), at(12, 1), note_day));
        assert!(is_due(&lines, range, at(18, 0), note_day));
        assert_eq!(STAMP_STALE_AFTER_MINUTES, None);
    }

    #[test]
    fn misplaced_stamp_is_due() {
        let canonical = render("you@example.com", at(12, 0), day());
        let text = format!("## Events\n\n- row\n{canonical}\n## Pomodoros\n");
        let (lines, range) = events(&text);
        assert!(is_due(&lines, range, at(12, 0), day()));
    }

    #[test]
    fn account_guard_refuses_a_different_account() {
        let canonical = render("a@example.com", at(12, 0), day());
        let text = format!("## Events\n\n{canonical}\n## Pomodoros\n");
        let error =
            account_guard(&text, "b@example.com", day(), "2026/20261030.md")
                .expect_err("guard");
        assert_eq!(error.kind(), "account_changed");
        assert_eq!(error.exit_code(), 2);
        assert!(
            error
                .message()
                .contains("2026/20261030.md was synced with a@example.com"),
            "{}",
            error.message()
        );
        assert!(
            error.hint().is_some_and(|hint| {
                hint.contains("hand this day to b@example.com")
                    && hint.contains("gcal.account: a@example.com")
            }),
            "{:?}",
            error.hint()
        );
    }

    #[test]
    fn account_guard_ignores_malformed_and_matching_stamps() {
        let same = render("you@example.com", at(12, 0), day());
        let text = format!("## Events\n\n{same}\n## Pomodoros\n");
        assert!(account_guard(&text, "YOU@example.com", day(), "p").is_ok());

        let malformed = "## Events\n\n*↻ broken*\n## Pomodoros\n";
        assert!(account_guard(malformed, "b@example.com", day(), "p").is_ok());
    }

    #[test]
    fn truncate_drops_seconds() {
        let now = day().and_hms_opt(14, 5, 59).expect("time");
        assert_eq!(truncate_to_minute(now), at(14, 5));
    }
}
