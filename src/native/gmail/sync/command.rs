//! Orchestration for `bob gmail sync` (and its hidden `pull` alias).
//!
//! Remote fetching, routing, policy checks, and body preparation finish
//! before the vault lock. The scan and [`super::file::FilingOutcome`] are
//! the extension point for later watch work: that phase consumes them
//! directly and never parses the rendered report.

use std::{
    collections::BTreeSet,
    path::Path,
    time::{Duration, Instant},
};

use super::{
    discover::{self, select_candidates, validate_limit},
    enroll::{EnrollmentFold, EnrollmentOrigin, Membership},
    file::{self, FilingCandidate, FilingInput, FilingOutcome},
    guard::{self, GuardAttempt},
    report::{self, SyncReport, SyncResult},
    retry::{
        execute_with_retry, is_auth_abort, pace_before_request, SharedBudget,
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

    let gog = Gog::resolve(crate::native::google::GMAIL, config.timeout())?;
    gog.require_version()?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    let budget =
        SharedBudget::new(Duration::from_secs(u64::from(args.retry_timeout)));
    let sleeper = SystemSleeper;

    let _guard = if args.dry_run {
        None
    } else {
        match guard::acquire_with_budget(&sleeper, &budget, &account).map_err(
            |error| {
                GmailError::runtime(
                    "lock",
                    format!("could not acquire the Gmail sync guard: {error}"),
                )
            },
        )? {
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
    if args.limit > 0 {
        let outcome = discover::discover_ids(&gog, &account, label)?;
        warnings.extend(outcome.warnings);
        discovered_ids = outcome.ids;
    }
    let candidates =
        select_candidates(&discovered_ids, &account, &enrollment, args.limit)?;
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
    if !candidates.is_empty() {
        let labels =
            discover::fetch_labels_once(&gog, &account, &budget, &sleeper)?;
        let labels_by_id = route::labels_by_id(labels);
        let route_ctx = RouteContext::load(&vault);
        let fetched = discover::fetch_candidate_metadata(
            &gog,
            &candidates,
            &budget,
            &sleeper,
        );
        warnings.extend(fetched.warnings);
        if let Some(error) = fetched.aborted {
            return Err(error);
        }
        for (key, _error) in fetched.failures {
            extra_results.push(SyncResult {
                thread_id: key.thread_id().to_string(),
                note: None,
                link: None,
                status: "problem",
                task_file: None,
                route: None,
            });
        }

        let mut last_request: Option<Instant> = None;
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
            if !pace_before_request(&sleeper, &budget, &mut last_request) {
                warnings.push(
                    "deferred remaining Gmail fetches when the retry budget ran out"
                        .to_string(),
                );
                break;
            }
            let identity = ResolvedIdentity {
                account: key.account().to_string(),
                thread_id: key.thread_id().to_string(),
                known_path: index
                    .by_identity
                    .get(&key.index_key())
                    .map(|entry| entry.path.clone()),
            };
            match execute_with_retry(&sleeper, &budget, || {
                save::prepare_sync_candidate(
                    &gog,
                    &account,
                    identity.clone(),
                    config.body(),
                    &never_save,
                )
            }) {
                Ok(prepared) => filing_candidates.push(FilingCandidate {
                    save: prepared,
                    route: verdict,
                }),
                Err(error) if is_auth_abort(&error) => return Err(error),
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
    }

    let timezone = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    let saved_at = save::current_utc();
    let outcome = if filing_candidates.is_empty() && adoptions.is_empty() {
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
    if outcome.busy {
        return Ok(SyncReport::busy(args.dry_run));
    }

    Ok(report::from_outcome(
        args.dry_run,
        outcome,
        extra_results,
        warnings,
        &watchers,
        &account,
    ))
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
