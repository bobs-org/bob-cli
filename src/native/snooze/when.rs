//! Injected zoned snooze time grammar.
//!
//! Contract: `docs/snooze.md` § Time grammar. Supports `1530`, `15:30`,
//! `3:30p`, `3p`, `19`, `+45m`, `+2h`, `+1h30m`, and the approved
//! next-occurrence-today reading of bare `330`. Relative times round upward
//! to a five-minute boundary. Resolution is in Bob's zone with an injected
//! clock: a strictly future time on the same day, DST gaps refused,
//! midnight overflow refused, and the first occurrence of a folded wall
//! time. Past or next-day times are teaching refusals directing multi-day
//! deferral to Schedule (`s:`).

use chrono::{NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Tz;

/// Injected clock: today's date, the current wall time, and Bob's zone.
/// Callers build this from `env::current_datetime`,
/// `env::configured_timezone`, or test fixtures; this module never reads
/// process state itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnoozeClock {
    pub(crate) date: NaiveDate,
    pub(crate) now_minutes: u16,
    pub(crate) zone: Tz,
}

/// Resolved target time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTime {
    /// Minutes since midnight.
    pub(crate) minutes: u16,
    /// Four-digit writer form (`1530`).
    pub(crate) hhmm: String,
    /// RFC 3339 instant in Bob's zone.
    pub(crate) rfc3339: String,
}

/// Teaching refusal for an unusable time expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WhenRefusal {
    /// Valid shape, but not strictly in the future today.
    Past,
    /// Resolves past midnight: same-day only, use `s:` for another day.
    NextDay,
    /// Wall time does not exist (DST gap).
    DstGap,
    /// Not a snooze time at all.
    Invalid,
}

impl WhenRefusal {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Past => "past",
            Self::NextDay => "next_day",
            Self::DstGap => "dst_gap",
            Self::Invalid => "invalid",
        }
    }

    /// Human teaching copy shared by every snooze entry surface.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Past | Self::NextDay => {
                "same-day only — Schedule (s:) defers to another day"
            }
            Self::DstGap => {
                "that wall time does not exist today — pick a nearby time"
            }
            Self::Invalid => {
                "use 1530, 15:30, 3:30p, 3p, 19, +45m, +2h, or +1h30m"
            }
        }
    }
}

/// Parse `input` against `clock` into a same-day future target.
pub(crate) fn parse_snooze_when(
    input: &str,
    clock: SnoozeClock,
) -> Result<ResolvedTime, WhenRefusal> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(WhenRefusal::Invalid);
    }
    if let Some(rest) = trimmed.strip_prefix('+') {
        let delta = parse_relative_minutes(rest)?;
        return resolve_relative(delta, clock);
    }
    resolve_absolute(trimmed, clock)
}

/// Writer form for minutes since midnight.
pub(crate) fn format_hhmm(minutes: u16) -> String {
    format!("{:02}{:02}", minutes / 60, minutes % 60)
}

fn parse_relative_minutes(rest: &str) -> Result<u16, WhenRefusal> {
    let text = rest.trim().to_lowercase();
    if text.is_empty() {
        return Err(WhenRefusal::Invalid);
    }
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut total: u16 = 0;
    let mut saw_component = false;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if start == index {
            return Err(WhenRefusal::Invalid);
        }
        let amount: u16 = text[start..index]
            .parse()
            .map_err(|_| WhenRefusal::Invalid)?;
        if index >= bytes.len() {
            return Err(WhenRefusal::Invalid);
        }
        let unit = bytes[index] as char;
        index += 1;
        match unit {
            'h' => {
                total = total
                    .checked_add(
                        amount.checked_mul(60).ok_or(WhenRefusal::Invalid)?,
                    )
                    .ok_or(WhenRefusal::Invalid)?;
                saw_component = true;
            }
            'm' => {
                total =
                    total.checked_add(amount).ok_or(WhenRefusal::Invalid)?;
                saw_component = true;
            }
            _ => return Err(WhenRefusal::Invalid),
        }
    }
    if !saw_component || total == 0 {
        return Err(WhenRefusal::Invalid);
    }
    // Round upward to a five-minute boundary.
    let rounded = total.div_ceil(5) * 5;
    Ok(rounded)
}

fn resolve_relative(
    delta: u16,
    clock: SnoozeClock,
) -> Result<ResolvedTime, WhenRefusal> {
    let target = u32::from(clock.now_minutes) + u32::from(delta);
    if target >= 24 * 60 {
        return Err(WhenRefusal::NextDay);
    }
    let minutes = target as u16;
    zoned_time(clock, minutes).ok_or(WhenRefusal::DstGap)
}

fn resolve_absolute(
    input: &str,
    clock: SnoozeClock,
) -> Result<ResolvedTime, WhenRefusal> {
    let lowered = input.trim().to_lowercase();
    // Split an optional a/p/am/pm suffix.
    let (core, meridiem) = split_meridiem(&lowered);
    if let Some(meridiem) = meridiem {
        return resolve_meridiem(core, meridiem, clock);
    }
    if core.contains(':') {
        let (left, right) = core.split_once(':').ok_or(WhenRefusal::Invalid)?;
        let hour: u16 =
            left.trim().parse().map_err(|_| WhenRefusal::Invalid)?;
        let minute: u16 =
            right.trim().parse().map_err(|_| WhenRefusal::Invalid)?;
        if left.trim().is_empty()
            || right.trim().len() != 2
            || hour > 23
            || minute > 59
        {
            return Err(WhenRefusal::Invalid);
        }
        return future_today(clock, hour * 60 + minute);
    }
    if !core.bytes().all(|byte| byte.is_ascii_digit()) || core.is_empty() {
        return Err(WhenRefusal::Invalid);
    }
    match core.len() {
        1 | 2 => {
            // Hour-only: `19` means 19:00, strictly.
            let hour: u16 = core.parse().map_err(|_| WhenRefusal::Invalid)?;
            if hour > 23 {
                return Err(WhenRefusal::Invalid);
            }
            future_today(clock, hour * 60)
        }
        3 => {
            // Approved next-occurrence-today reading of bare `330`:
            // 3:30 when future, else 15:30 when still today.
            let hour: u16 =
                core[..1].parse().map_err(|_| WhenRefusal::Invalid)?;
            let minute: u16 =
                core[1..].parse().map_err(|_| WhenRefusal::Invalid)?;
            if hour > 12 || minute > 59 {
                return Err(WhenRefusal::Invalid);
            }
            let first = hour * 60 + minute;
            if first > clock.now_minutes {
                return zoned_time(clock, first).ok_or(WhenRefusal::DstGap);
            }
            let second = first + 12 * 60;
            if second < 24 * 60 && second > clock.now_minutes {
                return zoned_time(clock, second).ok_or(WhenRefusal::DstGap);
            }
            Err(WhenRefusal::Past)
        }
        4 => {
            let hour: u16 =
                core[..2].parse().map_err(|_| WhenRefusal::Invalid)?;
            let minute: u16 =
                core[2..].parse().map_err(|_| WhenRefusal::Invalid)?;
            if hour > 23 || minute > 59 {
                return Err(WhenRefusal::Invalid);
            }
            future_today(clock, hour * 60 + minute)
        }
        _ => Err(WhenRefusal::Invalid),
    }
}

fn split_meridiem(lowered: &str) -> (&str, Option<&str>) {
    for suffix in ["am", "pm"] {
        if let Some(core) = lowered.strip_suffix(suffix) {
            return (core.trim_end(), Some(suffix));
        }
    }
    if let Some(core) = lowered
        .strip_suffix('a')
        .or_else(|| lowered.strip_suffix('p'))
    {
        let suffix = &lowered[lowered.len() - 1..];
        // A bare digit core only: `3p`, `3:30p`. A `19p` hour is invalid.
        return (core.trim_end(), Some(suffix));
    }
    (lowered, None)
}

fn resolve_meridiem(
    core: &str,
    meridiem: &str,
    clock: SnoozeClock,
) -> Result<ResolvedTime, WhenRefusal> {
    let pm = meridiem.starts_with('p');
    if let Some((left, right)) = core.split_once(':') {
        let hour: u16 =
            left.trim().parse().map_err(|_| WhenRefusal::Invalid)?;
        let minute: u16 =
            right.trim().parse().map_err(|_| WhenRefusal::Invalid)?;
        if !(1..=12).contains(&hour) || minute > 59 || right.trim().len() != 2 {
            return Err(WhenRefusal::Invalid);
        }
        let hour24 = meridiem_hour(hour, pm);
        return future_today(clock, hour24 * 60 + minute);
    }
    let hour: u16 = core.trim().parse().map_err(|_| WhenRefusal::Invalid)?;
    if !(1..=12).contains(&hour) {
        return Err(WhenRefusal::Invalid);
    }
    future_today(clock, meridiem_hour(hour, pm) * 60)
}

fn meridiem_hour(hour: u16, pm: bool) -> u16 {
    match (hour, pm) {
        (12, false) => 0,
        (12, true) => 12,
        (hour, true) => hour + 12,
        (hour, false) => hour,
    }
}

fn future_today(
    clock: SnoozeClock,
    minutes: u16,
) -> Result<ResolvedTime, WhenRefusal> {
    if minutes <= clock.now_minutes {
        return Err(WhenRefusal::Past);
    }
    zoned_time(clock, minutes).ok_or(WhenRefusal::DstGap)
}

/// Build the zoned instant for `minutes` on `clock.date`, choosing the
/// first occurrence of a folded wall time. Returns `None` for a DST gap.
fn zoned_time(clock: SnoozeClock, minutes: u16) -> Option<ResolvedTime> {
    use chrono::LocalResult;
    let time = NaiveTime::from_num_seconds_from_midnight_opt(
        u32::from(minutes) * 60,
        0,
    )?;
    let naive = clock.date.and_time(time);
    let zoned = match clock.zone.from_local_datetime(&naive) {
        LocalResult::Single(zoned) => zoned,
        LocalResult::Ambiguous(first, _second) => first,
        LocalResult::None => return None,
    };
    Some(ResolvedTime {
        minutes,
        hhmm: format_hhmm(minutes),
        rfc3339: zoned.to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::Tz;

    fn utc_clock(hour: u16, minute: u16) -> SnoozeClock {
        SnoozeClock {
            date: NaiveDate::from_ymd_opt(2026, 11, 25).expect("date"),
            now_minutes: hour * 60 + minute,
            zone: Tz::UTC,
        }
    }

    #[test]
    fn absolute_forms_resolve() {
        let clock = utc_clock(13, 0);
        assert_eq!(
            parse_snooze_when("1530", clock).expect("1530").minutes,
            15 * 60 + 30
        );
        assert_eq!(
            parse_snooze_when("15:30", clock).expect("colon").minutes,
            15 * 60 + 30
        );
        assert_eq!(
            parse_snooze_when("3:30p", clock).expect("pm").minutes,
            15 * 60 + 30
        );
        assert_eq!(
            parse_snooze_when("3p", clock).expect("hour pm").hhmm,
            "1500"
        );
        assert_eq!(
            parse_snooze_when("19", clock).expect("hour").minutes,
            19 * 60
        );
    }

    #[test]
    fn bare_digits_use_next_occurrence_today() {
        // 02:00: 330 means 03:30.
        let early = utc_clock(2, 0);
        assert_eq!(parse_snooze_when("330", early).expect("330").hhmm, "0330");
        // 13:00: 03:30 passed, so 330 means 15:30.
        let late = utc_clock(13, 0);
        assert_eq!(parse_snooze_when("330", late).expect("330").hhmm, "1530");
        // 16:00: both passed.
        let past = utc_clock(16, 0);
        assert_eq!(parse_snooze_when("330", past), Err(WhenRefusal::Past));
    }

    #[test]
    fn relative_times_round_up_to_five_minutes() {
        let clock = utc_clock(13, 2);
        // +45m from 13:02 lands on 13:47, no rounding needed.
        assert_eq!(parse_snooze_when("+45m", clock).expect("rel").hhmm, "1347");
        // +2m rounds up to +5m: 13:07.
        assert_eq!(
            parse_snooze_when("+2m", clock).expect("round").hhmm,
            "1307"
        );
        assert_eq!(
            parse_snooze_when("+1h30m", utc_clock(13, 0))
                .expect("combo")
                .hhmm,
            "1430"
        );
    }

    #[test]
    fn past_and_midnight_overflow_refuse_with_teaching_copy() {
        let clock = utc_clock(13, 0);
        assert_eq!(parse_snooze_when("1300", clock), Err(WhenRefusal::Past));
        assert_eq!(parse_snooze_when("+12h", clock), Err(WhenRefusal::NextDay));
        assert!(WhenRefusal::Past.message().contains("Schedule"));
    }

    #[test]
    fn dst_gap_is_refused_and_fold_picks_first() {
        use chrono_tz::America::New_York;
        // 2026-03-08: 02:30 does not exist in New York.
        let gap = SnoozeClock {
            date: NaiveDate::from_ymd_opt(2026, 3, 8).expect("date"),
            now_minutes: 0,
            zone: New_York,
        };
        assert_eq!(parse_snooze_when("0230", gap), Err(WhenRefusal::DstGap));
        // 2026-11-01: 01:30 occurs twice; the first occurrence wins even
        // when it has passed: 01:45 sees 01:30 as past, not 01:30 EDT-again.
        let fold = SnoozeClock {
            date: NaiveDate::from_ymd_opt(2026, 11, 1).expect("date"),
            now_minutes: 105,
            zone: New_York,
        };
        assert_eq!(parse_snooze_when("0130", fold), Err(WhenRefusal::Past));
        let before = SnoozeClock {
            now_minutes: 60,
            ..fold
        };
        let resolved = parse_snooze_when("0130", before).expect("fold");
        assert_eq!(resolved.hhmm, "0130");
        assert!(resolved.rfc3339.contains("-04:00"));
    }
}
