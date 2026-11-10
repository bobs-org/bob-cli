//! Archive command execution: lock ownership, bounded guarded retries, and
//! the Git finish.
//!
//! Standalone execution owns the shared maintenance lock from before planning
//! until Git completes; `run_collection` is the lock-already-held entry
//! nightly uses. Both apply one captured plan through the guarded write
//! engine (`ApplySession.tool = "task-archive"`), rebuilding the entire plan
//! while no output has been installed. A partial write stops: it reports the
//! exact applied and deferred paths plus the recovery directory, skips Git,
//! and never replays the request on its own.
use std::time::{Duration, Instant};

use super::super::task_status_hooks_write::{
    apply_plan, ApplyOutcome, ApplySession, ReasonCode,
};
use super::*;

use git::{finish_git, touched_git_paths};
use health::Diagnostics;
use plan::{
    carried_diagnostics, is_plan_refusal, plan_archive, PlannedArchive,
};
use report::{human_header, render_human, render_json, OutputFormat, Report};
use snapshot::scan_membership;

/// Recovery root name under `$XDG_STATE_HOME/bob-cli/`.
pub(super) const TOOL_NAME: &str = "task-archive";
/// How long standalone execution waits for the maintenance lock.
pub(super) const LOCK_WAIT: Duration = Duration::from_secs(30);
/// Separate budget for rebuilding and reapplying a stale plan.
pub(super) const RETRY_BUDGET: Duration = Duration::from_secs(30);
const RETRY_PAUSE: Duration = Duration::from_millis(250);

/// Injectable time so retry budgets are deterministic in tests.
pub(super) struct Clock {
    pub(super) elapsed: Box<dyn Fn() -> Duration>,
    pub(super) sleep: Box<dyn Fn(Duration)>,
}

impl Clock {
    fn production() -> Self {
        let started = Instant::now();
        Self {
            elapsed: Box::new(move || started.elapsed()),
            sleep: Box::new(std::thread::sleep),
        }
    }
}

type PlanFn = Box<dyn FnMut(&Path, usize) -> io::Result<PlannedArchive>>;

/// Everything a test swaps to drive planning, apply, and time.
pub(super) struct Hooks {
    pub(super) plan: PlanFn,
    pub(super) session: Box<dyn FnMut(&Path) -> ApplySession>,
    pub(super) child_env: Box<dyn Fn() -> ChildEnv>,
    pub(super) clock: Clock,
    pub(super) budget: Duration,
    pub(super) lock_wait: Duration,
}

impl Hooks {
    pub(super) fn production() -> Self {
        Self {
            plan: Box::new(plan_archive),
            session: Box::new(production_session),
            child_env: Box::new(ob::child_env),
            clock: Clock::production(),
            budget: RETRY_BUDGET,
            lock_wait: LOCK_WAIT,
        }
    }
}

fn production_session(vault: &Path) -> ApplySession {
    let rescan_vault = vault.to_path_buf();
    let mut session = ApplySession::production(Box::new(move || {
        scan_membership(&rescan_vault)
    }));
    session.tool = TOOL_NAME;
    session
}

/// How planning and apply ended, before Git.
enum Settled {
    /// The report already holds the final outcome (preview or failure).
    Recorded,
    /// The plan was empty: nothing to apply, no Git.
    Noop,
    Applied {
        files: Vec<PathBuf>,
        recovery_directory: PathBuf,
    },
}

/// Plan, apply, and finish Git. The caller owns the lock for non-preview
/// runs. Every outcome lands in `report`.
pub(super) fn execute(
    report: &mut Report,
    child_env: &ChildEnv,
    hooks: &mut Hooks,
) {
    match plan_and_apply(report, hooks) {
        Settled::Recorded => {}
        Settled::Noop => report.noop(),
        Settled::Applied {
            files,
            recovery_directory,
        } => {
            let applied = relative_paths(&report.vault, &files);
            let git = finish_git(
                &report.vault,
                child_env,
                touched_git_paths(&applied),
            );
            report.applied(applied, recovery_directory, git);
        }
    }
}

fn plan_and_apply(report: &mut Report, hooks: &mut Hooks) -> Settled {
    let vault = report.vault.clone();
    let begin = (hooks.clock.elapsed)();
    let within_budget = |hooks: &Hooks| {
        (hooks.clock.elapsed)().saturating_sub(begin) < hooks.budget
    };

    loop {
        let planned = match (hooks.plan)(&vault, report.threshold) {
            Ok(planned) => planned,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                if within_budget(hooks) {
                    progress(
                        "the vault changed while it was being read; rebuilding the plan",
                    );
                    (hooks.clock.sleep)(RETRY_PAUSE);
                    continue;
                }
                report.fail(
                    "retry_exhausted",
                    format!(
                        "the vault kept changing for {}s while planning; no vault changes were made ({error})",
                        hooks.budget.as_secs()
                    ),
                );
                return Settled::Recorded;
            }
            Err(error) => {
                let reason = if is_plan_refusal(&error) {
                    "plan_refused"
                } else {
                    "plan_failed"
                };
                if let Some(diagnostics) = carried_diagnostics(&error) {
                    report.diagnostics = diagnostics.clone();
                } else {
                    report.diagnostics = Diagnostics::incomplete();
                }
                report.fail(
                    reason,
                    format!(
                        "failed to plan archive for {}: {error}",
                        vault.display()
                    ),
                );
                return Settled::Recorded;
            }
        };
        let PlannedArchive {
            collection,
            write_plan,
            diagnostics,
        } = planned;
        report.diagnostics = diagnostics;
        let has_outputs = !write_plan.outputs.is_empty();
        report.plan = Some(collection);

        if report.dry_run {
            report.preview();
            return Settled::Recorded;
        }
        if !has_outputs {
            return Settled::Noop;
        }

        let session = (hooks.session)(&vault);
        match apply_plan(&write_plan, &session) {
            Ok(ApplyOutcome::NoOp) => return Settled::Noop,
            Ok(ApplyOutcome::Applied {
                applied_files,
                recovery_directory,
            }) => {
                return Settled::Applied {
                    files: applied_files,
                    recovery_directory,
                };
            }
            Err(error) => {
                let retryable = matches!(
                    error.reason,
                    ReasonCode::VaultChanged
                        | ReasonCode::QuietPeriod
                        | ReasonCode::UnstableRead
                ) && error.applied_files.is_empty();
                if retryable && within_budget(hooks) {
                    progress(
                        "the vault changed during apply; rebuilding the plan",
                    );
                    (hooks.clock.sleep)(RETRY_PAUSE);
                    continue;
                }
                let applied = relative_paths(&vault, &error.applied_files);
                let deferred = relative_paths(&vault, &error.deferred_files);
                let (reason, message) = if !error.applied_files.is_empty() {
                    (
                        "partial_apply",
                        format!(
                            "{}; applied {} path(s), deferred {}; inspect the recovery directory, then rerun explicitly (exact replays are detected, so an interrupted move completes without duplicating blocks)",
                            error.message,
                            applied.len(),
                            deferred.len()
                        ),
                    )
                } else if retryable {
                    (
                        "retry_exhausted",
                        format!(
                            "the vault kept changing for {}s; no vault changes were made ({})",
                            hooks.budget.as_secs(),
                            error.message
                        ),
                    )
                } else {
                    (
                        "write_failed",
                        format!(
                            "no vault changes were made: {}",
                            error.message
                        ),
                    )
                };
                report.fail_with_paths(
                    reason,
                    message,
                    applied,
                    deferred,
                    error.recovery_directory,
                );
                return Settled::Recorded;
            }
        }
    }
}

fn relative_paths(vault: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| {
            path.strip_prefix(vault)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.clone())
        })
        .collect()
}

fn progress(message: &str) {
    eprintln!("{COMMAND_NAME}: {message}");
}

/// Standalone `bob task archive`: owns the maintenance lock for non-preview
/// runs, from before planning through Git completion.
pub(super) fn run_standalone(args: Args) -> i32 {
    let mut report =
        Report::new(bob_env::bob_dir(), args.threshold, args.dry_run);
    if args.format == OutputFormat::Human {
        print!("{}", human_header(&report));
    }
    execute_standalone(&mut report, &mut Hooks::production());
    emit(&report, args.format)
}

/// Lock ownership around [`execute`] for the standalone command.
///
/// A preview never takes the lock, probes Git, or builds recovery state — it
/// does not even source the ssh-agent environment. Every other run waits up
/// to `hooks.lock_wait` for the lock and holds it until Git completes.
pub(super) fn execute_standalone(report: &mut Report, hooks: &mut Hooks) {
    if report.dry_run {
        execute(report, &Vec::new(), hooks);
        return;
    }
    let guard = match ob::acquire_lock_waiting(hooks.lock_wait, || {
        progress(&format!(
            "another Bob vault maintenance run holds the lock; waiting up to {}s",
            hooks.lock_wait.as_secs()
        ));
    }) {
        Ok(guard) => guard,
        Err(error) => {
            let reason = match error {
                ob::LockWaitError::Timeout { .. } => "lock_timeout",
                _ => "lock_error",
            };
            report.fail(reason, error.to_string());
            return;
        }
    };
    let child_env = (hooks.child_env)();
    execute(report, &child_env, hooks);
    drop(guard);
}

/// Run the archive collection against the vault and commit/push the result.
///
/// The caller already owns the maintenance lock — nightly holds it across its
/// steps — so this never acquires it and never syncs. Human output only.
pub(crate) fn run_collection(threshold: usize, child_env: &ChildEnv) -> i32 {
    run_collection_with(threshold, child_env, &mut Hooks::production())
}

pub(super) fn run_collection_with(
    threshold: usize,
    child_env: &ChildEnv,
    hooks: &mut Hooks,
) -> i32 {
    let mut report = Report::new(bob_env::bob_dir(), threshold, false);
    print!("{}", human_header(&report));
    execute(&mut report, child_env, hooks);
    emit(&report, OutputFormat::Human)
}

/// Render the finished report and return its exit code.
fn emit(report: &Report, format: OutputFormat) -> i32 {
    match format {
        OutputFormat::Human => print!("{}", render_human(report)),
        OutputFormat::Json => println!("{}", render_json(report)),
    }
    if let Some(message) = report.exit.message.as_deref()
        && report.exit.outcome == report::Outcome::Failed
    {
        eprintln!("{COMMAND_NAME}: {message}");
    }
    report.exit_code()
}
