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

use chrono::{Days, Months, NaiveDate};

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
    if numer < 0 || denom <= 0 {
        return None;
    }
    let half = denom / 2;
    numer.checked_add(half)?.checked_div(denom)
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
    }
}
