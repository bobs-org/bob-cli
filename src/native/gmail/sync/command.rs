//! Orchestration for `bob gmail sync` (and its hidden `pull` alias).
//!
//! Remote fetching, routing, policy checks, and body preparation finish
//! before the vault lock. The scan and [`super::file::FilingOutcome`] are
//! the extension point for watch work: that phase consumes them directly
//! and never parses the rendered report.

use std::{collections::BTreeSet, path::Path, time::Duration};

use super::{
    archive_preview::{self, PreviewInput},
    archive_scan::scan_archive_evidence,
    archive_state::{ArchiveJournal, ArchiveLabelIdentity},
    discover::{self, select_candidates, validate_limit},
    enroll::{EnrollmentFold, EnrollmentOrigin, Membership},
    file::{self, FilingCandidate, FilingInput, FilingOutcome},
    guard::{self, GuardAttempt},
    health,
    report::{self, SyncReport, SyncResult},
    retry::{
        is_auth_abort, CoordinatedRead, SharedBudget, SystemClock,
        SystemSleeper,
    },
    scan,
    watch_apply::{ACTION_BOUNCED, ACTION_LIFTED},
    watch_state::WatchJournal,
    ThreadKey,
};
use crate::native::{
    config as bob_config, env as bob_env,
    gmail::{
        cli::SyncArgs,
        config::GmailConfig,
        gog::{GmailLabel, Gog},
        note::index::MailIndex,
        policy::NeverSavePolicy,
        resolve::{self, GmailRead, ResolvedIdentity},
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
            GuardAttempt::Acquired(guard) => {
                health::record_attempt()?;
                Some(guard)
            }
            GuardAttempt::Busy => {
                if let Some(message) =
                    health::note_busy_skip("the Gmail sync guard")?
                {
                    return Err(GmailError::runtime("stale", message));
                }
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
    let mut known_metadata = std::collections::BTreeMap::new();
    let mut fetch_deferred_keys: Vec<ThreadKey> = Vec::new();
    let labels = match discover::fetch_labels_once(&coordinator, &account) {
        Ok(labels) => Some(labels),
        Err(error)
            if is_auth_abort(&error) || coordinator.auth_abort().is_some() =>
        {
            return Err(coordinator.auth_abort().unwrap_or(error));
        }
        Err(error) if coordinator.gave_up() || error.kind() == "deferred" => {
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
    let health = labels.as_ref().map(|labels| health::HealthConfig {
        labels_by_id: route::labels_by_id(labels.clone()),
        enrollment_label: Some(label.to_string()),
        route_prefixes: config.route_prefixes().to_vec(),
    });
    if let Some(health_config) = health.as_ref()
        && !candidates.is_empty()
    {
        let labels_by_id = &health_config.labels_by_id;
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
        known_metadata.extend(
            successes
                .iter()
                .map(|(key, thread)| (key.clone(), thread.clone())),
        );
        let mut prepared_at = 0usize;
        while prepared_at < successes.len() {
            let (key, thread) = &successes[prepared_at];
            let label_ids: BTreeSet<String> = thread
                .messages
                .iter()
                .flat_map(|message| message.label_ids.iter().cloned())
                .collect();
            let verdict = route::route(
                labels_by_id,
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

    let timezone = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    let saved_at = save::current_utc();
    let additional: Vec<ThreadKey> = filing_candidates
        .iter()
        .filter_map(|candidate| {
            ThreadKey::new(
                &candidate.save.confirmed.identity.account,
                &candidate.save.confirmed.identity.thread_id,
            )
        })
        .chain(adoptions.iter().map(|intent| intent.key.clone()))
        .collect();
    let mut watches = Vec::new();
    let mut watch_failures = Vec::new();
    let mut watch_deferred = Vec::new();
    match WatchJournal::load(&vault) {
        Ok(watch_journal) => {
            let batch = super::watch::prepare_watches(
                &coordinator,
                &vault,
                &account,
                &enrollment,
                &watchers,
                &additional,
                &known_metadata,
                &watch_journal,
                &index,
                config.body(),
                &never_save,
                timezone,
            );
            if let Some(error) = batch.aborted {
                return Err(coordinator.auth_abort().unwrap_or(error));
            }
            warnings.extend(batch.warnings);
            has_run_failure |=
                !batch.failures.is_empty() || !batch.deferred.is_empty();
            watches = batch.prepared;
            watch_failures = batch.failures;
            watch_deferred = batch.deferred;
        }
        Err(error) => {
            warnings.push(format!(
                "could not read the Gmail watch journal: {error}"
            ));
            has_run_failure = true;
        }
    }
    // Recovery runs even in watch-only mode, after the enrollment label
    // disappears, and with `-l 0`: a pending evidence file means a prior
    // scoped commit or its journal appends still need finishing. Dry-run
    // only projects the recovery rows; it never writes or deletes evidence.
    let needs_recovery = !matches!(
        file::recovery_presence(&vault, &account),
        file::RecoveryPresence::Absent
    );
    let mut outcome = if filing_candidates.is_empty()
        && adoptions.is_empty()
        && !needs_recovery
        && watches.is_empty()
        && watch_failures.is_empty()
        && watch_deferred.is_empty()
        && health.is_none()
    {
        FilingOutcome::default()
    } else {
        file::apply(
            FilingInput {
                vault: &vault,
                account: &account,
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
                watches,
                watch_failures,
                watch_deferred,
                reply_wait_days: config.reply_wait_days(),
                health,
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
    if !args.dry_run
        && outcome.busy
        && let Some(message) = health::note_busy_skip("the vault lock")?
    {
        return Err(GmailError::runtime("stale", message));
    }
    if !args.dry_run && !outcome.busy {
        health::record_success()?;
    }
    // The filing projection's watch outcomes feed archive planning: threads
    // with a newly applied incoming move hold for this run (C18). Clone
    // before `finish_report` consumes the outcome.
    let watch_threads = outcome.watch_threads.clone();
    let mut report = finish_report(
        args.dry_run,
        outcome,
        extra_results,
        warnings,
        &watchers,
        &account,
        &enrollment,
    );
    if args.dry_run {
        attach_archive_dry_run(
            &coordinator,
            &config,
            &vault,
            &account,
            &enrollment,
            &index,
            labels.as_ref(),
            args,
            &watch_threads,
            &mut report,
        )?;
    }
    Ok(report)
}

/// Read-only dry-run archive projection after the filing projection.
/// No locks, no directories, no journal entries, no label creation: the
/// planner only reads. Live application is the following phase's
/// responsibility, so live runs skip this entirely. Needs no modify grant
/// and ignores the writer pin.
#[allow(clippy::too_many_arguments)]
fn attach_archive_dry_run<R: GmailRead>(
    reader: &R,
    config: &GmailConfig,
    vault: &Path,
    account: &str,
    enrollment: &EnrollmentFold,
    index: &MailIndex,
    labels: Option<&Vec<GmailLabel>>,
    args: &SyncArgs,
    watch_threads: &[super::watch_apply::WatchThreadOutcome],
    report: &mut SyncReport,
) -> Result<(), GmailError> {
    if !config.archive_enabled() {
        return Ok(());
    }
    if args.no_archive {
        report.archive_skipped = true;
        return Ok(());
    }
    let relations = match bob_config::load_relations_config(
        &bob_config::config_path(),
    ) {
        Ok(relations) => relations,
        Err(error) => {
            report.warnings.push(format!(
                    "skipped archive preview: could not read relations config: {error}"
                ));
            report.ok = false;
            return Ok(());
        }
    };
    let folder = Path::new(config.folder());
    let scan = match scan_archive_evidence(vault, folder, index, &relations) {
        Ok(scan) => scan,
        Err(error) => {
            report.warnings.push(format!(
                "skipped archive preview: {}: {}",
                error.kind(),
                error.message(),
            ));
            report.ok = false;
            return Ok(());
        }
    };
    if !scan.scan_complete() {
        report.warnings.push(
            "skipped archive preview: the vault scan did not read every note"
                .to_string(),
        );
    }
    let journal = match ArchiveJournal::load(vault) {
        Ok(journal) => journal,
        Err(error) => {
            report.warnings.push(format!(
                "skipped archive preview: could not read the Gmail archive journal: {error}"
            ));
            report.ok = false;
            return Ok(());
        }
    };
    let Some(labels) = labels else {
        report.warnings.push(
            "skipped archive preview: the Gmail label list was unavailable"
                .to_string(),
        );
        report.ok = false;
        return Ok(());
    };
    let receipt = match ArchiveLabelIdentity::new(
        config.archive_label(),
        labels
            .iter()
            .find(|label| label.name == config.archive_label())
            .map(|label| label.id.clone()),
    ) {
        Some(receipt) => receipt,
        None => {
            report.warnings.push(
                "skipped archive preview: the archive receipt label is invalid"
                    .to_string(),
            );
            report.ok = false;
            return Ok(());
        }
    };
    let mut suppressed = BTreeSet::new();
    for outcome in watch_threads {
        if outcome.action == ACTION_LIFTED || outcome.action == ACTION_BOUNCED {
            suppressed.insert(outcome.key.clone());
        }
    }
    let never_save = NeverSavePolicy::from_config(config);
    let input = PreviewInput {
        account,
        enrollment,
        scan: &scan,
        journal: &journal,
        suppressed: &suppressed,
        policy: &never_save,
        labels,
        receipt_label: receipt,
    };
    match archive_preview::preview(
        reader,
        &input,
        config,
        args.confirm.as_deref(),
    ) {
        Ok(preview) => {
            report.warnings.extend(preview.warnings);
            if preview.failed || !preview.deferred.is_empty() {
                report.ok = false;
            }
            if preview.plan.gate.is_paused()
                && let archive_preview::ArchiveGate::Paused { reason } =
                    &preview.plan.gate
                && *reason == archive_preview::GATE_MISMATCH
            {
                report.warnings.push(
                    "the confirmation digest does not match the current Gmail preview".to_string(),
                );
            }
            report.attach_archive_preview(&preview.plan);
        }
        Err(error) => return Err(error),
    }
    Ok(())
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
