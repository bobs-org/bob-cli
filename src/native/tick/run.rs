use super::{
    catalog::{StepContext, STEPS},
    config::{Cadence, Config},
    lock::RunLock,
    now,
    receipt::{Outcome, Receipt, StepReceipt},
    render,
    schedule::{self, Trigger},
    signals,
    state::{State, StepState},
    Options,
};
use crate::native::{
    env as bob_env, pref_journal,
    spinner::Spinner,
    subprocess_runner::{self, RawRunSpec},
};
use std::{
    fs,
    io::IsTerminal,
    time::{Duration, Instant},
};

type Error = (i32, String);
fn io_error(e: std::io::Error) -> Error {
    (
        if signals::number() == 0 {
            1
        } else {
            128 + signals::number()
        },
        e.to_string(),
    )
}
pub(super) fn execute(mut options: Options) -> Result<i32, Error> {
    let config = Config::load().map_err(|e| (2, e))?;
    let started_at = now().map_err(|e| (2, e))?;
    // One identity for both explicit and environment vault paths; read-only modes
    // still accept a missing vault without creating it.
    options.vault = fs::canonicalize(&options.vault).unwrap_or_else(|_| {
        if options.vault.is_absolute() {
            options.vault.clone()
        } else {
            std::env::current_dir()
                .unwrap_or_default()
                .join(&options.vault)
        }
    });
    let host = pref_journal::host_label();
    let banner = config
        .writer
        .as_ref()
        .filter(|w| *w != &host)
        .map(|w| format!("this host is {host}; tick.writer is {w}"));
    if !options.dry_run
        && !options.status
        && let Some(banner) = &banner
    {
        return Err((2, format!("{banner}\nhint: run on the writer host, or correct tick.writer (defaults to gcal.writer)")));
    }
    if !options.status
        && let Some(name) = options
            .names
            .iter()
            .find(|n| config.steps[*n] == Cadence::Off)
    {
        return Err((
            2,
            format!("tick.steps.{name}: off; enable it before naming it"),
        ));
    }
    // Validate every execution setting before any state write or child spawn.
    let timeout_override = bob_env::var("BOB_TICK_STEP_TIMEOUT_SECS").ok().map(|s| {
        s.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0 && *n <= 86400.0).map(Duration::from_secs_f64).ok_or_else(|| (2, "BOB_TICK_STEP_TIMEOUT_SECS: expected positive seconds <= 86400".into()))
    }).transpose()?;
    let dir = bob_env::bob_cli_state_dir().join("tick");
    let read_only = options.dry_run || options.status;
    let _signals = if read_only {
        None
    } else {
        Some(signals::Signals::install().map_err(io_error)?)
    };
    let mut lock = if read_only {
        None
    } else {
        match RunLock::acquire(
            &dir,
            started_at,
            options.trigger,
            options.quiet || options.json,
        )
        .map_err(io_error)?
        {
            Ok(lock) => Some(lock),
            Err(busy) => {
                if options.json {
                    println!(
                        "{}",
                        serde_json::json!({"schema_version":1, "command":"tick", "host":host, "vault":options.vault, "trigger": options.trigger, "exit_code":0, "reason":"already_running", "detail":busy, "steps":[]})
                    );
                } else if !options.quiet {
                    println!("bob tick · {busy}");
                }
                return Ok(0);
            }
        }
    };
    // Read under the lock, not before it: an interactive wait may span a whole run.
    let mut state = State::load(&dir, &host, &options.vault);
    let previous_receipt = if options.status {
        fs::read(dir.join("last.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Receipt>(&b).ok())
            .filter(|r| r.host == host && r.vault == options.vault)
    } else {
        None
    };
    let mut receipt = Receipt {
        schema_version: 1,
        command: "tick".into(),
        host,
        vault: options.vault.clone(),
        trigger: options.trigger,
        started_at,
        finished_at: started_at,
        exit_code: 0,
        steps: Vec::new(),
        mode: if options.dry_run {
            Some("plan".into())
        } else if options.status {
            Some("status".into())
        } else {
            None
        },
        banner,
    };
    let began = Instant::now();
    let mut intake_ran = false;
    let mut printed_header = false;
    for step in STEPS {
        if !read_only && signals::number() != 0 {
            break;
        }
        if options.status
            && !options.names.is_empty()
            && !options.names.iter().any(|n| n == step.name)
        {
            continue;
        }
        let at = now().map_err(|e| (2, e))?;
        let prior = state.steps.get(step.name).cloned().unwrap_or_default();
        let cadence = config.steps[step.name];
        let selection = schedule::decide(
            cadence,
            &prior,
            at,
            options.trigger,
            options.names.iter().any(|n| n == step.name),
            !step.intake,
            intake_ran,
        );
        if selection.clock_reversed {
            eprintln!("bob tick: warning: {} last start is in the future; ignoring it", step.name);
        }
        let mut row = StepReceipt {
            name: step.name.into(),
            cadence: cadence.to_string(),
            outcome: Outcome::Skipped,
            reason: selection.reason.into(),
            detail: String::new(),
            started_at: None,
            duration_ms: None,
            exit_code: None,
            summary: if options.trigger == Trigger::Named
                && !options.names.iter().any(|n| n == step.name)
            {
                "not named".into()
            } else {
                selection.reason.replace('_', " ")
            },
            next_due_at: selection.next_due_at,
            last_success_at: prior.last_success_at,
            last_finished_at: prior.last_finished_at,
            last_outcome: prior.last_outcome,
            last_summary: prior.last_summary.clone(),
            would_run: options.dry_run.then_some(selection.run),
            stderr_tail: Vec::new(),
        };
        let gate = if selection.run {
            (step.gate)(&StepContext {
                vault: &options.vault,
                now: at,
                trigger: options.trigger,
                state: &prior,
            })
        } else {
            None
        };
        if let Some(gate) = gate {
            row.outcome = gate.outcome;
            row.reason = gate.reason;
            row.detail = gate.detail;
            row.summary = row.detail.clone();
            row.would_run = options.dry_run.then_some(false);
            if !read_only {
                record_result(&mut state, &row, at, false);
            }
        } else if selection.run && options.dry_run {
            if step.intake {
                intake_ran = true;
            }
        } else if selection.run && !options.status {
            lock.as_mut()
                .expect("writing run holds lock")
                .step(step.name)
                .map_err(io_error)?;
            if !options.quiet && !options.json && !printed_header {
                render::header(&receipt);
                printed_header = true;
            }
            // Persist follow-up intent before spawning: even abrupt death during an
            // intake must not lose the derive catch-up. Roll it back on spawn failure.
            let pending_before: Vec<_> = STEPS
                .iter()
                .filter(|s| !s.intake)
                .map(|s| {
                    (
                        s.name,
                        state
                            .steps
                            .get(s.name)
                            .is_some_and(|s| s.follow_up_pending),
                    )
                })
                .collect();
            if step.intake {
                for (name, _) in &pending_before {
                    state
                        .steps
                        .entry((*name).into())
                        .or_default()
                        .follow_up_pending = true;
                }
                state.save(&dir).map_err(io_error)?;
            }
            let spinner = if !options.quiet
                && !options.json
                && std::io::stderr().is_terminal()
            {
                Some(Spinner::start_elapsed(&format!(
                    "bob tick · {}",
                    step.name
                )))
            } else {
                None
            };
            let child_began = Instant::now();
            let program = bob_env::var_os("BOB_TICK_STEP_EXE")
                .map(Into::into)
                .map(Ok)
                .unwrap_or_else(std::env::current_exe);
            let output = program.and_then(|program| {
                subprocess_runner::run_raw(&RawRunSpec {
                    program,
                    args: step.args(&options.vault),
                    env: vec![(
                        "BOB_DIR".into(),
                        options.vault.as_os_str().to_owned(),
                    )],
                    env_remove: Vec::new(),
                    timeout: timeout_override.unwrap_or(step.ceiling),
                    term_grace: Some(Duration::from_secs(30)),
                    interrupt: Some(&signals::INTERRUPTED),
                    stdout_cap: 4 * 1024 * 1024,
                    stderr_cap: 64 * 1024,
                })
            });
            drop(spinner);
            row.duration_ms =
                Some(child_began.elapsed().as_millis().min(u64::MAX as u128)
                    as u64);
            let consumes_interval = match output {
                Ok(output) => {
                    row.started_at = Some(at);
                    row.exit_code = output.status;
                    let adapted = (step.adapter)(&output, selection.reason);
                    row.outcome = adapted.outcome;
                    row.reason = adapted.reason;
                    row.summary = adapted.summary;
                    if step.intake {
                        intake_ran = true;
                    }
                    if output.stdout_truncated || output.output_incomplete {
                        eprintln!(
                            "bob tick: warning: {} output was truncated",
                            step.name
                        );
                    }
                    row.stderr_tail = String::from_utf8_lossy(&output.stderr)
                        .lines()
                        .rev()
                        .take(8)
                        .map(render::safe_text)
                        .collect();
                    row.stderr_tail.reverse();
                    adapted.consumes_interval
                }
                Err(e) => {
                    if step.intake {
                        for (name, pending) in pending_before {
                            state
                                .steps
                                .entry(name.into())
                                .or_default()
                                .follow_up_pending = pending;
                        }
                    }
                    row.outcome = Outcome::Failed;
                    row.reason = "spawn_failed".into();
                    row.summary = "could not start child".into();
                    row.stderr_tail = vec![render::safe_text(&e.to_string())];
                    false
                }
            };
            let finished = now().map_err(|e| (2, e))?;
            record_result(&mut state, &row, finished, consumes_interval);
            if row.started_at.is_some() && !step.intake {
                state
                    .steps
                    .entry(step.name.into())
                    .or_default()
                    .follow_up_pending = false;
            }
            let updated = &state.steps[step.name];
            row.next_due_at = schedule::decide(
                cadence,
                updated,
                finished,
                Trigger::Scheduled,
                false,
                !step.intake,
                false,
            )
            .next_due_at;
            row.last_success_at = updated.last_success_at;
            row.last_finished_at = updated.last_finished_at;
            row.last_outcome = updated.last_outcome;
            row.last_summary = updated.last_summary.clone();
        }
        if options.status {
            row.started_at = prior.last_started_at;
            row.outcome = prior.last_outcome.unwrap_or(Outcome::Skipped);
            row.reason = prior
                .last_reason
                .clone()
                .unwrap_or_else(|| "never_ran".into());
            row.summary = prior
                .last_summary
                .clone()
                .unwrap_or_else(|| "never ran".into());
            row.exit_code = prior.last_exit_code;
            // Due now includes persisted follow-up, even if the cadence is later.
            if selection.run {
                row.next_due_at = Some(at);
            }
        }
        if !read_only {
            state.save(&dir).map_err(io_error)?;
            if row.outcome == Outcome::Failed {
                receipt.exit_code = 1;
            }
            if !options.json
                && (row.outcome == Outcome::Failed
                    || (!options.quiet
                        && (row.started_at.is_some()
                            || row.outcome == Outcome::Waiting)))
            {
                if !printed_header && !options.quiet {
                    render::header(&receipt);
                    printed_header = true;
                }
                render::run_row(&row);
            }
        }
        receipt.steps.push(row);
    }
    receipt.finished_at = now().map_err(|e| (2, e))?;
    if !read_only {
        for row in &mut receipt.steps {
            if matches!(config.steps[&row.name], Cadence::Interval(_))
                && state
                    .steps
                    .get(&row.name)
                    .is_some_and(|s| s.follow_up_pending)
            {
                row.next_due_at = Some(receipt.finished_at);
            }
        }
    }
    if !read_only {
        if signals::number() != 0 {
            receipt.exit_code = 128 + signals::number();
        }
        receipt.save(&dir).map_err(io_error)?;
    }
    if options.json {
        println!(
            "{}",
            serde_json::to_string(&receipt).expect("receipt serializes")
        );
    } else if options.dry_run || options.status {
        render::view(&receipt, previous_receipt.as_ref(), options.quiet);
    } else if !options.quiet || receipt.exit_code != 0 {
        render::summary(&receipt, began.elapsed(), printed_header);
    }
    Ok(receipt.exit_code)
}
fn record_result(
    state: &mut State,
    row: &StepReceipt,
    finished: chrono::DateTime<chrono::Utc>,
    consumes_interval: bool,
) {
    let entry: &mut StepState =
        state.steps.entry(row.name.clone()).or_default();
    if consumes_interval {
        entry.last_started_at = row.started_at;
    }
    entry.last_finished_at = Some(finished);
    entry.last_outcome = Some(row.outcome);
    entry.last_reason = Some(row.reason.clone());
    entry.last_summary = Some(row.summary.clone());
    entry.last_exit_code = row.exit_code;
    if row.outcome == Outcome::Ok {
        entry.last_success_at = Some(finished);
    }
}
