//! Orchestration for `bob gmail sync` (and its hidden `pull` alias).
//!
//! Remote fetching, routing, policy checks, and body preparation finish
//! before the vault lock. The scan and [`super::file::FilingOutcome`] are
//! the extension point for later watch work: that phase consumes them
//! directly and never parses the rendered report.

use std::{collections::BTreeSet, path::Path, time::Duration};

use super::{
    discover::{self, select_candidates, validate_limit},
    enroll::{EnrollmentFold, EnrollmentOrigin, Membership},
    file::{self, FilingCandidate, FilingInput, FilingOutcome},
    guard::{self, GuardAttempt},
    report::{self, SyncReport, SyncResult},
    retry::{
        is_auth_abort, CoordinatedRead, SharedBudget, SystemClock,
        SystemSleeper,
    },
    scan, ThreadKey,
};
use crate::native::{
    env as bob_env,
    gmail::{
        cli::SyncArgs,
        config::GmailConfig,
        gog::Gog,
        note::index::MailIndex,
        policy::NeverSavePolicy,
        resolve::{self, ResolvedIdentity},
        route::{self, RouteContext},
        save, ui, GmailError,
    },
    vault_sync,
};

pub(crate) fn run(args: &SyncArgs) -> i32 {
    match run_inner(args) {
        Ok(report) => {
            report::print(args, &report);
            report.exit_code()
        }
        Err(error) => ui::report_error("sync", &error, args.format.as_str()),
    }
}

fn run_inner(args: &SyncArgs) -> Result<SyncReport, GmailError> {
    validate_limit(args.limit)?;
    let config = GmailConfig::resolve()?;
    let vault = args.bob_dir();
    if !vault.is_dir() {
        return Err(GmailError::setup(
            "vault",
            format!("Bob vault root does not exist: {}", vault.display()),
        ));
    }
    if !args.dry_run {
        check_writer(&config)?;
    }
    let label = config
        .sync_label()
        .filter(|label| !label.is_empty())
        .ok_or_else(|| {
            GmailError::setup(
                "sync_disabled",
                "Gmail sync is disabled by an empty gmail.sync.label"
                    .to_string(),
            )
            .with_hint(
                "set gmail.sync.label to a Gmail label name, such as bob",
            )
        })?;

    let gog = Gog::resolve(
        crate::native::google::GMAIL,
        config.gog(),
        config.timeout(),
    )?;
    gog.require_version()?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    let budget =
        SharedBudget::new(Duration::from_secs(u64::from(args.retry_timeout)));
    let sleeper = SystemSleeper;
    let clock = SystemClock;
    // One coordinator wraps every Gmail read during preparation. It latches
    // raw auth aborts and wait-budget give-ups before shared save/policy
    // code re-labels them, and it paces across every stage.
    let last_request = std::cell::RefCell::new(None);
    let coordinator =
        CoordinatedRead::new(&gog, &sleeper, &budget, &clock, &last_request);

    let _guard = if args.dry_run {
        None
    } else {
        match guard::acquire_with_budget(&sleeper, &budget, &clock, &account)
            .map_err(|error| {
                GmailError::runtime(
                    "lock",
                    format!("could not acquire the Gmail sync guard: {error}"),
                )
            })? {
            GuardAttempt::Acquired(guard) => Some(guard),
            GuardAttempt::Busy => {
                return Ok(SyncReport::busy(args.dry_run));
            }
        }
    };

    let enrollment = EnrollmentFold::load(&vault).map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not read the Gmail enrollment journal: {error}"),
        )
    })?;
    let folder = Path::new(config.folder());
    let index = resolve::scan_index(&vault, folder, config.link_label())?;
    let watchers = scan::scan_watching_tasks(&vault, folder, &index)?;
    let membership = Membership::new(&enrollment, &watchers);
    let adoptions = membership.adoptions(&account);

    let mut warnings = Vec::new();
    warnings.extend(enrollment.warnings().iter().cloned());
    warnings.extend(watchers.warnings().iter().cloned());

    let mut discovered_ids = Vec::new();
    let mut has_run_failure = false;
    if args.limit > 0 {
        match discover::discover_ids(&coordinator, &account, label) {
            Err(error) if is_auth_abort(&error) => {
                return Err(coordinator.auth_abort().unwrap_or(error));
            }
            Err(error) => {
                warnings.push(format!(
                    "Gmail search failed: {}: {}",
                    error.kind(),
                    error.message()
                ));
                has_run_failure = true;
            }
            Ok(outcome) => {
                if outcome.deferred || outcome.failed {
                    has_run_failure = true;
                }
                warnings.extend(outcome.warnings);
                discovered_ids = outcome.ids;
            }
        }
        if let Some(error) = coordinator.auth_abort() {
            return Err(error);
        }
    }
    let adopted_keys: std::collections::BTreeSet<ThreadKey> =
        adoptions.iter().map(|intent| intent.key.clone()).collect();
    let candidates = select_candidates(
        &discovered_ids,
        &account,
        &enrollment,
        &adopted_keys,
        args.limit,
    )?;
    let mut extra_results = Vec::new();
    for id in &discovered_ids {
        let Some(key) = ThreadKey::new(&account, id) else {
            continue;
        };
        if !enrollment.is_enrolled(&key) {
            continue;
        }
        extra_results.push(enrolled_row(&key, &enrollment, &index));
    }

    let never_save = NeverSavePolicy::from_config(&config);
    let mut filing_candidates = Vec::new();
    let mut fetch_deferred_keys: Vec<ThreadKey> = Vec::new();
    if !candidates.is_empty() {
        let labels = match discover::fetch_labels_once(&coordinator, &account) {
            Ok(labels) => Some(labels),
            Err(error)
                if is_auth_abort(&error)
                    || coordinator.auth_abort().is_some() =>
            {
                return Err(coordinator.auth_abort().unwrap_or(error));
            }
            Err(error)
                if coordinator.gave_up() || error.kind() == "deferred" =>
            {
                warnings.push(format!(
                    "deferred remaining Gmail fetches when the retry budget ran out: {}: {}",
                    error.kind(),
                    error.message()
                ));
                has_run_failure = true;
                fetch_deferred_keys.extend(candidates.iter().cloned());
                None
            }
            Err(error) => {
                warnings.push(format!(
                    "Gmail label list failed: {}: {}",
                    error.kind(),
                    error.message()
                ));
                has_run_failure = true;
                None
            }
        };
        if let Some(labels) = labels {
            let labels_by_id = route::labels_by_id(labels);
            let route_ctx = RouteContext::load(&vault);
            let fetched =
                discover::fetch_candidate_metadata(&coordinator, &candidates);
            warnings.extend(fetched.warnings);
            if let Some(error) = fetched.aborted {
                return Err(coordinator.auth_abort().unwrap_or(error));
            }
            if let Some(error) = coordinator.auth_abort() {
                return Err(error);
            }
            if !fetched.deferred.is_empty() {
                has_run_failure = true;
                fetch_deferred_keys.extend(fetched.deferred.iter().cloned());
            }
            for (key, error) in fetched.failures {
                warnings.push(format!(
                    "skipped Gmail thread {}: {}: {}",
                    key.thread_id(),
                    error.kind(),
                    error.message()
                ));
                extra_results.push(problem_row(&key));
            }

            let successes = fetched.successes;
            let mut prepared_at = 0usize;
            while prepared_at < successes.len() {
                let (key, thread) = &successes[prepared_at];
                let label_ids: BTreeSet<String> = thread
                    .messages
                    .iter()
                    .flat_map(|message| message.label_ids.iter().cloned())
                    .collect();
                let verdict = route::route(
                    &labels_by_id,
                    &label_ids,
                    Some(label),
                    config.route_prefixes(),
                    &route_ctx,
                );
                let identity = ResolvedIdentity {
                    account: key.account().to_string(),
                    thread_id: key.thread_id().to_string(),
                    known_path: index
                        .by_identity
                        .get(&key.index_key())
                        .map(|entry| entry.path.clone()),
                };
                match save::prepare_sync_candidate(
                    &coordinator,
                    &account,
                    identity,
                    config.body(),
                    &never_save,
                ) {
                    Ok(prepared) => filing_candidates.push(FilingCandidate {
                        save: prepared,
                        route: verdict,
                    }),
                    Err(_) if coordinator.auth_abort().is_some() => {
                        return Err(coordinator
                            .auth_abort()
                            .expect("auth abort latched"));
                    }
                    Err(error) if coordinator.gave_up() => {
                        warnings.push(format!(
                            "deferred remaining Gmail fetches when the retry budget ran out: {}: {}",
                            error.kind(),
                            error.message()
                        ));
                        has_run_failure = true;
                        fetch_deferred_keys.extend(
                            successes[prepared_at..]
                                .iter()
                                .map(|(key, _)| key.clone()),
                        );
                        break;
                    }
                    Err(error) => {
                        warnings.push(format!(
                            "skipped Gmail thread {}: {}: {}",
                            key.thread_id(),
                            error.kind(),
                            error.message()
                        ));
                        extra_results.push(problem_row(key));
                    }
                }
                prepared_at += 1;
            }
        }
    }

    let timezone = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    let saved_at = save::current_utc();
    // Recovery must run even in watch-only mode and after the enrollment
    // label disappears: a pending sync-recovery file means a prior scoped
    // commit or its journal appends still need finishing.
    let needs_recovery = !args.dry_run && file::has_pending_recovery(&vault);
    let mut outcome = if filing_candidates.is_empty()
        && adoptions.is_empty()
        && !needs_recovery
    {
        FilingOutcome::default()
    } else {
        file::apply(
            FilingInput {
                vault: &vault,
                mail_folder: folder,
                link_template: config.link_label(),
                inbox: config.sync_inbox(),
                candidates: filing_candidates,
                adoptions: &adoptions,
                enrollment: &enrollment,
                watchers: &watchers,
                dry_run: args.dry_run,
                saved_at,
                timezone,
                budget: &budget,
            },
            &sleeper,
            &clock,
        )?
    };
    // Search, metadata, or hydration failure is a body-free failure state:
    // successes still file, adoption and recovery still run, but the run
    // must not report success.
    if has_run_failure {
        outcome.failed = true;
        outcome.deferred.extend(fetch_deferred_keys.iter().cloned());
        outcome.deferred.sort();
        outcome.deferred.dedup();
    }
    Ok(finish_report(
        args.dry_run,
        outcome,
        extra_results,
        warnings,
        &watchers,
        &account,
        &enrollment,
    ))
}

fn finish_report(
    dry_run: bool,
    outcome: FilingOutcome,
    extra_results: Vec<SyncResult>,
    warnings: Vec<String>,
    watchers: &scan::WatchScan,
    account: &str,
    enrollment: &EnrollmentFold,
) -> SyncReport {
    if outcome.busy {
        let had_failure = outcome.failed
            || extra_results.iter().any(|row| row.status == "problem")
            || !outcome.failures.is_empty()
            || !outcome.deferred.is_empty();
        if !had_failure {
            return SyncReport::busy(dry_run);
        }
        let mut outcome = outcome;
        outcome.failed = true;
        return report::from_outcome(
            dry_run,
            outcome,
            extra_results,
            warnings,
            watchers,
            account,
            enrollment,
        );
    }
    report::from_outcome(
        dry_run,
        outcome,
        extra_results,
        warnings,
        watchers,
        account,
        enrollment,
    )
}

fn problem_row(key: &ThreadKey) -> SyncResult {
    SyncResult {
        thread_id: key.thread_id().to_string(),
        note: None,
        link: None,
        status: "problem",
        task_file: None,
        route: None,
    }
}

fn enrolled_row(
    key: &ThreadKey,
    fold: &EnrollmentFold,
    index: &MailIndex,
) -> SyncResult {
    let latest = fold.latest(key);
    let entry = index.by_identity.get(&key.index_key());
    SyncResult {
        thread_id: key.thread_id().to_string(),
        note: entry
            .map(|entry| entry.path.clone())
            .or_else(|| latest.and_then(|record| record.note.clone())),
        link: entry.map(|entry| entry.label.clone()),
        status: match fold.origin(key) {
            Some(EnrollmentOrigin::Legacy) => "already_pulled",
            _ => "already_filed",
        },
        task_file: latest.and_then(|record| record.task_file.clone()),
        route: latest.and_then(|record| record.route.clone()),
    }
}

fn check_writer(config: &GmailConfig) -> Result<(), GmailError> {
    let host = vault_sync::hostname_slug();
    if let Some(writer) = config.writer()
        && writer != host
    {
        return Err(GmailError::setup(
            "writer",
            format!("this host ({host}) is not the configured Gmail writer"),
        )
        .with_hint(&format!(
            "run this command on the configured writer {writer}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::sync::file::FilingFailure;

    fn empty_fold() -> EnrollmentFold {
        let temp = tempfile::tempdir().unwrap();
        EnrollmentFold::load(temp.path()).unwrap()
    }

    #[test]
    fn busy_skip_without_failures_stays_quiet() {
        let fold = empty_fold();
        let report = finish_report(
            false,
            FilingOutcome {
                busy: true,
                ..FilingOutcome::default()
            },
            Vec::new(),
            Vec::new(),
            &scan::WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert!(report.busy);
        assert!(report.ok);
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn busy_vault_lock_after_deferral_keeps_failure() {
        let fold = empty_fold();
        let key = ThreadKey::new("me@example.com", "bb02").unwrap();
        let report = finish_report(
            false,
            FilingOutcome {
                busy: true,
                failed: true,
                deferred: vec![key.clone()],
                ..FilingOutcome::default()
            },
            vec![problem_row(&key)],
            vec!["deferred remaining Gmail fetches when the retry budget ran out: rate_limited: quota".into()],
            &scan::WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert!(report.busy);
        assert!(!report.ok);
        assert_eq!(report.exit_code(), 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("rate_limited")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn fetch_failure_warning_includes_kind() {
        let fold = empty_fold();
        let key = ThreadKey::new("me@example.com", "aa01").unwrap();
        let report = finish_report(
            false,
            FilingOutcome {
                failed: true,
                failures: vec![FilingFailure {
                    key: key.clone(),
                    kind: "not_found".into(),
                    message: "gone".into(),
                }],
                ..FilingOutcome::default()
            },
            vec![problem_row(&key)],
            vec!["skipped Gmail thread aa01: not_found: gone".into()],
            &scan::WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert!(!report.ok);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("not_found")
                    && warning.contains("gone")),
            "{:?}",
            report.warnings
        );
    }
}
