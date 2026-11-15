//! Calendar-step helpers for subscription ledgers.
//!
//! Cadence intervals are always shifted from one original anchor date, never
//! by walking month-to-month from a clamped intermediate. Monthly, quarterly,
//! and yearly steps clamp to the last day of the target month (so 31 January
//! plus one month is 28/29 February, and 29 February plus one year is 28
//! February). Four-weekly steps add exactly 28 days.
//!
//! Timeline buckets align to `renews` when set, otherwise to the latest
//! charge. The current interval is retained even when its due date is later
//! than today. A 1.5-step still-charging window compares integer day counts
//! with an inclusive threshold (`elapsed * 2 <= step_days * 3`).

use chrono::{Datelike, Days, Months, NaiveDate};

use super::registry::SubscriptionCadence;

/// Shift `date` by `steps` cadence periods from that original date.
pub(crate) fn add_cadence_steps(
    date: NaiveDate,
    steps: i32,
    cadence: SubscriptionCadence,
) -> Option<NaiveDate> {
    if steps == 0 {
        return Some(date);
    }
    match cadence {
        SubscriptionCadence::Monthly => shift_months(date, i64::from(steps)),
        SubscriptionCadence::Quarterly => {
            shift_months(date, i64::from(steps).checked_mul(3)?)
        }
        SubscriptionCadence::Yearly => {
            shift_months(date, i64::from(steps).checked_mul(12)?)
        }
        SubscriptionCadence::FourWeekly => {
            shift_days(date, i64::from(steps).checked_mul(28)?)
        }
    }
}

/// Inclusive 1.5-cadence-step window from `from` through `today`.
pub(crate) fn within_one_and_half_steps(
    from: NaiveDate,
    today: NaiveDate,
    cadence: SubscriptionCadence,
) -> bool {
    let elapsed = (today - from).num_days();
    if elapsed < 0 {
        return true;
    }
    let Some(next) = add_cadence_steps(from, 1, cadence) else {
        return false;
    };
    let step_days = (next - from).num_days();
    elapsed
        .checked_mul(2)
        .zip(step_days.checked_mul(3))
        .is_some_and(|(elapsed_days, threshold)| elapsed_days <= threshold)
}

/// Half-up division for nonnegative integer cents.
pub(crate) fn div_round_half_up(numer: i64, denom: i64) -> Option<i64> {
    div_round_half_up_i128(i128::from(numer), i128::from(denom))
}

/// Half-up `amount * mul / denom` using a widened intermediate.
pub(crate) fn mul_div_round_half_up(
    amount: i64,
    mul: i64,
    denom: i64,
) -> Option<i64> {
    if amount < 0 || mul < 0 || denom <= 0 {
        return None;
    }
    let numer = i128::from(amount).checked_mul(i128::from(mul))?;
    div_round_half_up_i128(numer, i128::from(denom))
}

/// Locate the cadence step whose due date is the first on or after `today`.
///
/// Steps are signed offsets from the original `anchor`, so a future `renews`
/// date walks backward and an old charge walks forward with no 240-step cap.
pub(crate) fn current_step(
    anchor: NaiveDate,
    today: NaiveDate,
    cadence: SubscriptionCadence,
) -> Option<i32> {
    let mut n = estimate_steps(anchor, today, cadence);
    loop {
        let due = add_cadence_steps(anchor, n, cadence)?;
        if due >= today {
            break;
        }
        n = n.checked_add(1)?;
    }
    loop {
        let Some(prev) = n.checked_sub(1) else {
            break;
        };
        let Some(prev_due) = add_cadence_steps(anchor, prev, cadence) else {
            break;
        };
        if prev_due < today {
            break;
        }
        n = prev;
    }
    Some(n)
}

fn div_round_half_up_i128(numer: i128, denom: i128) -> Option<i64> {
    if numer < 0 || denom <= 0 {
        return None;
    }
    let half = denom / 2;
    let rounded = numer.checked_add(half)?.checked_div(denom)?;
    i64::try_from(rounded).ok()
}

fn estimate_steps(
    anchor: NaiveDate,
    today: NaiveDate,
    cadence: SubscriptionCadence,
) -> i32 {
    match cadence {
        SubscriptionCadence::Monthly => months_between(anchor, today),
        SubscriptionCadence::Quarterly => months_between(anchor, today) / 3,
        SubscriptionCadence::Yearly => today.year() - anchor.year(),
        SubscriptionCadence::FourWeekly => {
            let days = (today - anchor).num_days();
            i32::try_from(days / 28).unwrap_or(if days < 0 {
                i32::MIN
            } else {
                i32::MAX
            })
        }
    }
}

fn months_between(anchor: NaiveDate, today: NaiveDate) -> i32 {
    (today.year() - anchor.year()).saturating_mul(12)
        + (i32::try_from(today.month()).unwrap_or(0)
            - i32::try_from(anchor.month()).unwrap_or(0))
}

fn shift_months(date: NaiveDate, months: i64) -> Option<NaiveDate> {
    let amount = Months::new(months.unsigned_abs().try_into().ok()?);
    if months < 0 {
        date.checked_sub_months(amount)
    } else {
        date.checked_add_months(amount)
    }
}

fn shift_days(date: NaiveDate, days: i64) -> Option<NaiveDate> {
    let amount = Days::new(days.unsigned_abs());
    if days < 0 {
        date.checked_sub_days(amount)
    } else {
        date.checked_add_days(amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("date")
    }

    #[test]
    fn monthly_and_quarterly_clamp_month_end_from_the_anchor() {
        let jan31 = date(2026, 1, 31);
        assert_eq!(
            add_cadence_steps(jan31, 1, SubscriptionCadence::Monthly),
            Some(date(2026, 2, 28))
        );
        assert_eq!(
            add_cadence_steps(jan31, 2, SubscriptionCadence::Monthly),
            Some(date(2026, 3, 31))
        );
        assert_eq!(
            add_cadence_steps(jan31, -1, SubscriptionCadence::Monthly),
            Some(date(2025, 12, 31))
        );
        assert_eq!(
            add_cadence_steps(jan31, 1, SubscriptionCadence::Quarterly),
            Some(date(2026, 4, 30))
        );
        assert_eq!(
            add_cadence_steps(jan31, 2, SubscriptionCadence::Quarterly),
            Some(date(2026, 7, 31))
        );
    }

    #[test]
    fn yearly_clamps_leap_day_from_the_anchor() {
        let leap = date(2024, 2, 29);
        assert_eq!(
            add_cadence_steps(leap, 1, SubscriptionCadence::Yearly),
            Some(date(2025, 2, 28))
        );
        assert_eq!(
            add_cadence_steps(leap, 2, SubscriptionCadence::Yearly),
            Some(date(2026, 2, 28))
        );
        assert_eq!(
            add_cadence_steps(leap, 4, SubscriptionCadence::Yearly),
            Some(date(2028, 2, 29))
        );
    }

    #[test]
    fn four_weekly_adds_exact_days() {
        let start = date(2026, 1, 6);
        assert_eq!(
            add_cadence_steps(start, 1, SubscriptionCadence::FourWeekly),
            Some(date(2026, 2, 3))
        );
        assert_eq!(
            add_cadence_steps(start, 2, SubscriptionCadence::FourWeekly),
            Some(date(2026, 3, 3))
        );
        assert_eq!(
            add_cadence_steps(start, -1, SubscriptionCadence::FourWeekly),
            Some(date(2025, 12, 9))
        );
    }

    #[test]
    fn still_charging_threshold_is_inclusive_one_and_half_steps() {
        let latest = date(2026, 1, 1);
        let cadence = SubscriptionCadence::Monthly;
        // January has 31 days; 1.5 * 31 = 46.5, so 46 days is in, 47 is out.
        assert!(within_one_and_half_steps(
            latest,
            date(2026, 2, 16),
            cadence
        ));
        assert!(!within_one_and_half_steps(
            latest,
            date(2026, 2, 17),
            cadence
        ));
        let four = date(2026, 1, 1);
        assert!(within_one_and_half_steps(
            four,
            date(2026, 2, 12),
            SubscriptionCadence::FourWeekly
        ));
        assert!(!within_one_and_half_steps(
            four,
            date(2026, 2, 13),
            SubscriptionCadence::FourWeekly
        ));
    }

    #[test]
    fn half_up_rounding_covers_exact_half_and_fractions() {
        assert_eq!(div_round_half_up(6, 12), Some(1));
        assert_eq!(div_round_half_up(5, 12), Some(0));
        assert_eq!(div_round_half_up(13 * 6, 12), Some(7));
        assert_eq!(div_round_half_up(13 * 999, 12), Some(1082));
        assert_eq!(div_round_half_up(101, 3), Some(34));
        assert_eq!(div_round_half_up(100, 3), Some(33));
        assert_eq!(div_round_half_up(2, 3), Some(1));
        assert_eq!(div_round_half_up(1, 3), Some(0));
        assert_eq!(
            mul_div_round_half_up(1_000_000_000_000_000_000, 13, 12),
            Some(1_083_333_333_333_333_333)
        );
        assert_eq!(mul_div_round_half_up(i64::MAX, 13, 12), None);
        assert_eq!(div_round_half_up(i64::MAX, 1), Some(i64::MAX));
    }

    #[test]
    fn current_step_walks_backward_for_future_renews_and_past_anchors() {
        let today = date(2026, 11, 15);
        assert_eq!(
            current_step(
                date(2028, 1, 31),
                today,
                SubscriptionCadence::Monthly
            ),
            Some(-14)
        );
        assert_eq!(
            add_cadence_steps(
                date(2028, 1, 31),
                -14,
                SubscriptionCadence::Monthly
            ),
            Some(date(2026, 11, 30))
        );
        assert_eq!(
            current_step(
                date(2000, 1, 15),
                today,
                SubscriptionCadence::Monthly
            ),
            Some(322)
        );
        assert_eq!(
            current_step(
                date(2026, 11, 15),
                today,
                SubscriptionCadence::Monthly
            ),
            Some(0)
        );
        assert_eq!(
            current_step(
                date(2026, 8, 15),
                today,
                SubscriptionCadence::Monthly
            ),
            Some(3)
        );
    }
}
