//! Pure scheduling: gates and execution are deliberately outside this module.
use super::{config::Cadence, state::StepState};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Trigger {
    Scheduled,
    Named,
    All,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub run: bool,
    pub reason: &'static str,
    pub next_due_at: Option<DateTime<Utc>>,
    pub clock_reversed: bool,
}
pub(super) fn decide(
    cadence: Cadence,
    state: &StepState,
    now: DateTime<Utc>,
    trigger: Trigger,
    selected: bool,
    derive: bool,
    intake_ran: bool,
) -> Selection {
    let clock_reversed = state.last_started_at.is_some_and(|last| last > now);
    let last = state.last_started_at.filter(|_| !clock_reversed);
    let next_due_at = match (cadence, last) {
        (Cadence::Interval(minutes), Some(last)) => {
            DateTime::from_timestamp(last.timestamp().div_euclid(60) * 60, 0)
                .and_then(|t| t.checked_add_signed(TimeDelta::minutes(minutes)))
        }
        _ => None,
    };
    let (run, reason) = if cadence == Cadence::Off {
        (false, "off")
    } else if trigger == Trigger::Named {
        (selected, if selected { "named" } else { "not_due" })
    } else if trigger == Trigger::All {
        (true, "all")
    } else if cadence == Cadence::Manual {
        (false, "manual")
    } else if derive && intake_ran {
        (true, "follows_intake")
    } else if derive && state.follow_up_pending {
        (true, "follow_up")
    } else if last.is_none() {
        (true, "never_ran")
    } else if next_due_at.is_some_and(|next| now >= next) {
        (true, "due")
    } else {
        (false, "not_due")
    };
    Selection {
        run,
        reason,
        next_due_at,
        clock_reversed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn time(t: &str) -> DateTime<Utc> {
        t.parse().unwrap()
    }
    #[test]
    fn minute_floor_clock_reversal_and_boundaries() {
        let s = StepState {
            last_started_at: Some(time("2026-11-18T10:00:58Z")),
            ..Default::default()
        };
        for (at, due) in [
            ("2026-11-18T10:59:59Z", false),
            ("2026-11-18T11:00:00Z", true),
            ("2026-11-18T09:59:00Z", true),
        ] {
            let d = decide(
                Cadence::Interval(60),
                &s,
                time(at),
                Trigger::Scheduled,
                false,
                false,
                false,
            );
            assert_eq!(d.run, due);
            assert_eq!(d.clock_reversed, at.contains("09:59"));
        }
    }
    #[test]
    fn trigger_and_follow_up_matrix() {
        let now = time("2026-11-18T10:00:00Z");
        for cadence in [Cadence::Interval(15), Cadence::Manual, Cadence::Off] {
            for trigger in [Trigger::Scheduled, Trigger::Named, Trigger::All] {
                for selected in [false, true] {
                    for pending in [false, true] {
                        for intake in [false, true] {
                            for derive in [false, true] {
                                let s = StepState {
                                    last_started_at: Some(now),
                                    follow_up_pending: pending,
                                    ..Default::default()
                                };
                                let expected = cadence != Cadence::Off
                                    && match trigger {
                                        Trigger::All => true,
                                        Trigger::Named => selected,
                                        Trigger::Scheduled => {
                                            cadence != Cadence::Manual
                                                && derive
                                                && (pending || intake)
                                        }
                                    };
                                assert_eq!(
                                    decide(
                                        cadence, &s, now, trigger, selected,
                                        derive, intake
                                    )
                                    .run,
                                    expected
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
