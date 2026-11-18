use super::{
    adapters,
    config::Cadence,
    gates,
    receipt::{GateInfo, Outcome},
    schedule::Trigger,
    state::StepState,
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
    pub detail: String,
    pub consumes_interval: bool,
}
pub(super) struct StepSpec {
    pub name: &'static str,
    pub intake: bool,
    pub default: Cadence,
    pub ceiling: Duration,
    pub argv: &'static [&'static str],
    pub gate: fn(&StepContext<'_>) -> Option<(GateInfo, Option<GateResult>)>,
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
fn gcal_gate(
    context: &StepContext<'_>,
) -> Option<(GateInfo, Option<GateResult>)> {
    if let Some(block) = gates::readiness_gate(context.vault, context.now) {
        let detail = block.detail.clone();
        let reason = block.reason.clone();
        return Some((
            GateInfo {
                kind: "readiness".into(),
                open: false,
                reason,
                detail: detail.clone(),
                fingerprint_changed: None,
                hook: None,
                full_scan_due_at: None,
            },
            Some(block),
        ));
    }
    Some((
        GateInfo {
            kind: "readiness".into(),
            open: true,
            reason: "ready".into(),
            detail: "today's note is ready".into(),
            fingerprint_changed: None,
            hook: None,
            full_scan_due_at: None,
        },
        None,
    ))
}
fn tasks_gate(
    context: &StepContext<'_>,
) -> Option<(GateInfo, Option<GateResult>)> {
    gcal_gate(context)
}
fn refs_gate(
    context: &StepContext<'_>,
) -> Option<(GateInfo, Option<GateResult>)> {
    Some(gates::refs_gate(context))
}
fn no_gate(_: &StepContext<'_>) -> Option<(GateInfo, Option<GateResult>)> {
    None
}
const fn step_with(
    name: &'static str,
    intake: bool,
    default: Cadence,
    ceiling_mins: u64,
    argv: &'static [&'static str],
    gate: fn(&StepContext<'_>) -> Option<(GateInfo, Option<GateResult>)>,
    adapter: fn(&RawRunOutput, &str) -> Adapted,
) -> StepSpec {
    StepSpec {
        name,
        intake,
        default,
        ceiling: Duration::from_secs(ceiling_mins * 60),
        argv,
        gate,
        adapter,
    }
}
pub(super) const STEPS: &[StepSpec] = &[
    step_with(
        "gcal",
        true,
        Cadence::Interval(60),
        10,
        &["gcal", "sync", "-q", "-f", "json"],
        gcal_gate,
        adapters::gcal,
    ),
    step_with(
        "gmail",
        true,
        Cadence::Interval(60),
        20,
        &["gmail", "sync", "-q", "-f", "json"],
        no_gate,
        adapters::gmail,
    ),
    step_with(
        "gkeep",
        true,
        Cadence::Manual,
        20,
        &["gkeep", "pull", "-f", "json"],
        no_gate,
        adapters::gkeep,
    ),
    step_with(
        "refs",
        true,
        Cadence::Interval(15),
        30,
        &["ref", "scan", "-w", "-f", "json"],
        refs_gate,
        adapters::refs,
    ),
    step_with(
        "projects",
        false,
        Cadence::Interval(15),
        10,
        &["projects", "sync", "-f", "json"],
        no_gate,
        adapters::projects,
    ),
    step_with(
        "tasks",
        false,
        Cadence::Interval(15),
        10,
        &["task", "reconcile", "--retry-timeout", "120", "-f", "json"],
        tasks_gate,
        adapters::tasks,
    ),
];
