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
        execute_with_retry, is_auth_abort, pace_before_request, should_retry,
        SharedBudget, SystemClock, SystemSleeper,
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

    let gog = Gog::resolve(crate::native::google::GMAIL, config.timeout())?;
    gog.require_version()?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    let budget =
        SharedBudget::new(Duration::from_secs(u64::from(args.retry_timeout)));
    let sleeper = SystemSleeper;
    let clock = SystemClock;
    // One pacing slot shared across every Gmail stage: paged search, label
    // enumeration, candidate metadata, hydration, policy metadata, and
    // attachment reads.
    let mut last_request: Option<std::time::Instant> = None;

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
    let mut has_deferred = false;
    if args.limit > 0 {
        let outcome = discover::discover_ids(
            &gog,
            &account,
            label,
            &budget,
            &sleeper,
            &clock,
            &mut last_request,
        )?;
        if outcome.deferred {
            has_deferred = true;
        }
        warnings.extend(outcome.warnings);
        discovered_ids = outcome.ids;
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
        let labels = match discover::fetch_labels_once(
            &gog,
            &account,
            &budget,
            &sleeper,
            &clock,
            &mut last_request,
        ) {
            Ok(labels) => labels,
            Err(error) if is_auth_abort(&error) => return Err(error),
            Err(error)
                if error.kind() == "deferred" || should_retry(&error) =>
            {
                warnings.push(
                    "deferred remaining Gmail fetches when the retry budget ran out"
                        .to_string(),
                );
                has_deferred = true;
                fetch_deferred_keys.extend(candidates.iter().cloned());
                Vec::new()
            }
            Err(error) => return Err(error),
        };
        let labels_by_id = route::labels_by_id(labels);
        let route_ctx = RouteContext::load(&vault);
        // When label enumeration deferred, skip metadata entirely; the
        // candidates above are already marked deferred.
        let fetched =
            if has_deferred && fetch_deferred_keys.len() == candidates.len() {
                discover::FetchOutcome {
                    successes: Vec::new(),
                    failures: Vec::new(),
                    warnings: Vec::new(),
                    deferred: candidates.clone(),
                    aborted: None,
                }
            } else {
                discover::fetch_candidate_metadata(
                    &gog,
                    &candidates,
                    &budget,
                    &sleeper,
                    &clock,
                    &mut last_request,
                )
            };
        warnings.extend(fetched.warnings);
        if let Some(error) = fetched.aborted {
            return Err(error);
        }
        if !fetched.deferred.is_empty() {
            has_deferred = true;
            fetch_deferred_keys.extend(fetched.deferred.iter().cloned());
        }
        for (key, error) in fetched.failures {
            warnings.push(format!(
                "skipped Gmail thread {}: {}",
                key.thread_id(),
                error.message()
            ));
            extra_results.push(SyncResult {
                thread_id: key.thread_id().to_string(),
                note: None,
                link: None,
                status: "problem",
                task_file: None,
                route: None,
            });
        }

        // Hydration, policy checks, and attachment reads share the run's
        // pacing slot through a coordinated reader, so no whole-batch retry
        // runs without pacing its underlying reads. A revoked grant aborts
        // before vault writes; budget exhaustion defers the rest.
        let gate = std::cell::RefCell::new(last_request);
        let paced =
            CoordinatedRead::new(&gog, &sleeper, &budget, &clock, &gate);
        for (key, thread) in fetched.successes {
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
                &paced,
                &account,
                identity,
                config.body(),
                &never_save,
            ) {
                Ok(prepared) => filing_candidates.push(FilingCandidate {
                    save: prepared,
                    route: verdict,
                }),
                Err(error) if is_auth_abort(&error) => return Err(error),
                Err(error)
                    if error.kind() == "deferred" || should_retry(&error) =>
                {
                    warnings.push(
                        "deferred remaining Gmail fetches when the retry budget ran out"
                            .to_string(),
                    );
                    has_deferred = true;
                    fetch_deferred_keys.push(key);
                    // Defer this and every later hydration: the shared
                    // budget cannot afford another underlying read.
                    break;
                }
                Err(error) => {
                    warnings.push(format!(
                        "skipped Gmail thread {}: {}",
                        key.thread_id(),
                        error.message()
                    ));
                    extra_results.push(SyncResult {
                        thread_id: key.thread_id().to_string(),
                        note: None,
                        link: None,
                        status: "problem",
                        task_file: None,
                        route: None,
                    });
                }
            }
        }
        last_request = gate.into_inner();
        let _ = last_request;
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
        )?
    };
    // Metadata exhaustion or hydration deferral is a body-free failure
    // state: successes still file, but the run must not report success
    // after a real transient failure.
    if has_deferred {
        outcome.failed = true;
        outcome.deferred.extend(fetch_deferred_keys.iter().cloned());
        outcome.deferred.sort();
        outcome.deferred.dedup();
    }
    if outcome.busy {
        return Ok(SyncReport::busy(args.dry_run));
    }

    // The report needs the pre-fetch enrollment fold plus this run's new
    // events (in `outcome`) to count durable watch membership. The
    // revalidated snapshot inside `outcome` decides live membership.
    Ok(report::from_outcome(
        args.dry_run,
        outcome,
        extra_results,
        warnings,
        &watchers,
        &account,
        &enrollment,
    ))
}

/// Gmail reads for hydration, policy, and attachment stages that share the
/// run's pacing slot and retry budget.
///
/// Every underlying read is paced and retried individually, so a
/// whole-batch hydration is never retried without pacing its reads. A
/// revoked grant surfaces as `invalid_grant` for an abort before vault
/// writes; budget exhaustion surfaces as `deferred` for a catch-up-next-run
/// deferral.
struct CoordinatedRead<'a> {
    inner: &'a Gog,
    sleeper: &'a SystemSleeper,
    budget: &'a SharedBudget,
    clock: &'a SystemClock,
    last: &'a std::cell::RefCell<Option<std::time::Instant>>,
}

impl<'a> CoordinatedRead<'a> {
    fn new(
        inner: &'a Gog,
        sleeper: &'a SystemSleeper,
        budget: &'a SharedBudget,
        clock: &'a SystemClock,
        last: &'a std::cell::RefCell<Option<std::time::Instant>>,
    ) -> Self {
        Self {
            inner,
            sleeper,
            budget,
            clock,
            last,
        }
    }

    fn run<T>(
        &self,
        operation: impl FnMut() -> Result<T, GmailError>,
    ) -> Result<T, GmailError> {
        if !pace_before_request(
            self.sleeper,
            self.budget,
            self.clock,
            &mut self.last.borrow_mut(),
        ) {
            return Err(crate::native::gmail::sync::retry::deferred_error());
        }
        execute_with_retry(self.sleeper, self.budget, self.clock, operation)
    }
}

impl resolve::GmailRead for CoordinatedRead<'_> {
    fn attachment_bytes(
        &self,
        account: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, GmailError> {
        self.run(|| {
            self.inner
                .attachment_bytes(account, message_id, attachment_id)
        })
    }

    fn thread_get(
        &self,
        account: &str,
        thread_id: &str,
    ) -> Result<crate::native::gmail::model::GmailThread, GmailError> {
        self.run(|| self.inner.thread_get(account, thread_id))
    }

    fn message_metadata(
        &self,
        account: &str,
        message_id: &str,
    ) -> Result<serde_json::Value, GmailError> {
        self.run(|| self.inner.message_metadata(account, message_id))
    }

    fn search_page(
        &self,
        account: &str,
        query: &str,
        max: u32,
        page: Option<&str>,
    ) -> Result<serde_json::Value, GmailError> {
        self.run(|| self.inner.search_page(account, query, max, page))
    }

    fn labels_list(
        &self,
        account: &str,
    ) -> Result<Vec<crate::native::gmail::gog::GmailLabel>, GmailError> {
        self.run(|| self.inner.labels_list(account))
    }

    fn thread_metadata(
        &self,
        account: &str,
        thread_id: &str,
    ) -> Result<crate::native::gmail::model::GmailThread, GmailError> {
        self.run(|| self.inner.thread_metadata(account, thread_id))
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
