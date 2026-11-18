use super::{
    config::Cadence, receipt::Outcome, schedule::Trigger, state::StepState,
};
use crate::native::subprocess_runner::RawRunOutput;
use chrono::{DateTime, Utc};
use std::{ffi::OsString, path::Path, time::Duration};

/// Inputs available to step safety/change gates. Gates never mutate scheduler state.
pub(super) struct StepContext<'a> {
    pub vault: &'a Path,
    pub now: DateTime<Utc>,
    pub trigger: Trigger,
    pub state: &'a StepState,
}
pub(super) struct GateResult {
    pub outcome: Outcome,
    pub reason: String,
    pub detail: String,
}
pub(super) struct Adapted {
    pub outcome: Outcome,
    pub reason: String,
    pub summary: String,
    pub consumes_interval: bool,
}
pub(super) struct StepSpec {
    pub name: &'static str,
    pub intake: bool,
    pub default: Cadence,
    pub ceiling: Duration,
    pub argv: &'static [&'static str],
    pub gate: fn(&StepContext<'_>) -> Option<GateResult>,
    pub adapter: fn(&RawRunOutput, &str) -> Adapted,
}
impl StepSpec {
    pub fn args(&self, vault: &Path) -> Vec<OsString> {
        let mut args: Vec<_> = self.argv.iter().map(OsString::from).collect();
        args.extend([
            OsString::from("--bob-dir"),
            vault.as_os_str().to_owned(),
        ]);
        args
    }
}
fn open(context: &StepContext<'_>) -> Option<GateResult> {
    // The next phase replaces these no-op gates with per-step preconditions.
    let StepContext {
        vault,
        now,
        trigger,
        state,
    } = context;
    let _ = (vault, now, trigger, state);
    None
}
fn generic(output: &RawRunOutput, reason: &str) -> Adapted {
    let (outcome, reason, summary) = if output.interrupted {
        (Outcome::Failed, "interrupted", "interrupted".into())
    } else if output.timed_out {
        (Outcome::Failed, "timeout", "time limit exceeded".into())
    } else if output.status == Some(0) {
        (Outcome::Ok, reason, "done".into())
    } else {
        (
            Outcome::Failed,
            "exit_nonzero",
            format!(
                "exit {}",
                output
                    .status
                    .map_or_else(|| "signal".into(), |n| n.to_string())
            ),
        )
    };
    Adapted {
        outcome,
        reason: reason.into(),
        summary,
        consumes_interval: true,
    }
}
const fn step(
    name: &'static str,
    intake: bool,
    default: Cadence,
    ceiling_mins: u64,
    argv: &'static [&'static str],
) -> StepSpec {
    StepSpec {
        name,
        intake,
        default,
        ceiling: Duration::from_secs(ceiling_mins * 60),
        argv,
        gate: open,
        adapter: generic,
    }
}
pub(super) const STEPS: &[StepSpec] = &[
    step(
        "gcal",
        true,
        Cadence::Interval(60),
        10,
        &["gcal", "sync", "-q", "-f", "json"],
    ),
    step(
        "gmail",
        true,
        Cadence::Interval(60),
        20,
        &["gmail", "sync", "-q", "-f", "json"],
    ),
    step(
        "gkeep",
        true,
        Cadence::Manual,
        20,
        &["gkeep", "pull", "-f", "json"],
    ),
    step(
        "refs",
        true,
        Cadence::Interval(15),
        30,
        &["ref", "scan", "-w", "-f", "json"],
    ),
    step(
        "projects",
        false,
        Cadence::Interval(15),
        10,
        &["projects", "sync", "-f", "json"],
    ),
    step(
        "tasks",
        false,
        Cadence::Interval(15),
        10,
        &["task", "reconcile", "--retry-timeout", "120", "-f", "json"],
    ),
];
