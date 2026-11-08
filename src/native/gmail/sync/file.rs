//! Routed mail-note and task filing through one vault transaction.
//!
//! Remote work is complete before [`apply`] is called. The live path then
//! holds the maintenance lock while it refreshes enrollment, the mail index,
//! destinations, and open-task membership; plans all notes and task edits;
//! writes the coalesced batch; commits only owned paths; and appends the
//! enrollment journal last. Dry-run uses the same planner without a guard,
//! lock, directory creation, or journal write.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};

use super::{
    enroll::{
        self, AdoptionIntent, CandidateDecision, EnrollmentEvent,
        EnrollmentFold, EnrollmentOrigin, FiledVia, Membership,
    },
    retry::{SharedBudget, Sleeper},
    scan::{self, WatchScan},
    ThreadKey,
};
use crate::native::{
    attachments::journal as attachment_journal,
    capture::{self, StagedTextFile},
    env as bob_env,
    gmail::{
        note::index::MailIndex,
        resolve,
        route::{RouteContext, RouteProblem, RouteVerdict},
        save::{self, PreparedSave, SaveEventFacts, StagedSaveNote},
        GmailError,
    },
    ob, vault_sync,
};

/// A fully prepared thread and the route resolved from its fetched labels.
/// No Gmail request is performed by this module.
pub(crate) struct FilingCandidate {
    pub(crate) save: PreparedSave,
    pub(crate) route: RouteVerdict,
}

/// Inputs from discovery and the read-only preflight scan. The live apply
/// rereads these snapshots under the vault lock; dry-run consumes them as-is.
pub(crate) struct FilingInput<'a> {
    pub(crate) vault: &'a Path,
    pub(crate) mail_folder: &'a Path,
    pub(crate) link_template: &'a str,
    pub(crate) inbox: &'a str,
    pub(crate) candidates: Vec<FilingCandidate>,
    pub(crate) adoptions: &'a [AdoptionIntent],
    pub(crate) enrollment: &'a EnrollmentFold,
    pub(crate) watchers: &'a WatchScan,
    pub(crate) dry_run: bool,
    pub(crate) saved_at: DateTime<Utc>,
    pub(crate) timezone: chrono_tz::Tz,
    pub(crate) budget: &'a SharedBudget,
}

/// One thread's result for the command report. Paths are vault-relative and
/// the `link` contains the inert mail-note wikilink returned by save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilingResult {
    pub(crate) key: ThreadKey,
    pub(crate) note: Option<String>,
    pub(crate) link: Option<String>,
    pub(crate) task_file: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) route: Option<String>,
    pub(crate) via: Option<FiledVia>,
    pub(crate) problem: Option<RouteProblem>,
    pub(crate) first_seen: bool,
}

/// One candidate that could not be planned independently. Gmail errors are
/// already body-free and redacted by the shared transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilingFailure {
    pub(crate) key: ThreadKey,
    pub(crate) kind: String,
    pub(crate) message: String,
}

/// Results of the combined planning and apply boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FilingOutcome {
    pub(crate) results: Vec<FilingResult>,
    pub(crate) failures: Vec<FilingFailure>,
    pub(crate) warnings: Vec<String>,
    pub(crate) deferred: Vec<ThreadKey>,
    /// A busy vault lock is a quiet success/skip for sync.
    pub(crate) busy: bool,
    /// Enrollment journal failures are recoverable by the next adoption scan
    /// but make this run fail, as the durable row is not yet enrolled.
    pub(crate) failed: bool,
    pub(crate) changed: bool,
}

#[derive(Debug, Clone)]
struct Destination {
    relative: PathBuf,
    via: FiledVia,
    route: Option<String>,
    problem: Option<RouteProblem>,
    verdict: RouteVerdict,
}

struct DestinationSnapshot {
    target: PathBuf,
    original: Option<String>,
    updated: String,
}

struct AttemptPlan {
    outcome: FilingOutcome,
    works: BTreeMap<(String, String), PreparedSave>,
    destinations: BTreeMap<ThreadKey, Destination>,
    plans: Vec<StagedSaveNote>,
    staged: Vec<StagedTextFile>,
    task_commit_paths: Vec<PathBuf>,
    auxiliary_commit_paths: Vec<PathBuf>,
    enrollment_events: Vec<EnrollmentEvent>,
}

impl AttemptPlan {
    fn into_retry_candidates(mut self) -> Vec<FilingCandidate> {
        self.works
            .into_iter()
            .filter_map(|((account, thread_id), save)| {
                let key = ThreadKey::new(&account, &thread_id)?;
                let destination = self.destinations.remove(&key)?;
                Some(FilingCandidate {
                    save,
                    route: destination.verdict,
                })
            })
            .collect()
    }
}

/// Recheck local snapshots, plan every successful candidate together, and
/// persist the result as one scoped transaction. All callers must complete
/// Gmail fetching and body preparation before entering this function.
pub(crate) fn apply<S: Sleeper>(
    input: FilingInput<'_>,
    sleeper: &S,
) -> Result<FilingOutcome, GmailError> {
    if input.dry_run {
        let index = resolve::scan_index(
            input.vault,
            input.mail_folder,
            input.link_template,
        )?;
        let plan = build_attempt(
            input.vault,
            input.mail_folder,
            input.link_template,
            input.inbox,
            input.candidates,
            input.adoptions,
            input.enrollment,
            input.watchers,
            &index,
            true,
            false,
            input.saved_at,
            input.timezone,
        )?;
        return Ok(plan.outcome);
    }

    let Some(_lock) = acquire_vault_lock(input.budget, sleeper)? else {
        return Ok(FilingOutcome {
            busy: true,
            ..FilingOutcome::default()
        });
    };

    let mut candidates = input.candidates;
    for attempt in 0..2 {
        let enrollment = EnrollmentFold::load(input.vault).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not reread the Gmail enrollment journal: {error}"),
            )
        })?;
        let index = resolve::scan_index(
            input.vault,
            input.mail_folder,
            input.link_template,
        )?;
        let watchers =
            scan::scan_watching_tasks(input.vault, input.mail_folder, &index)?;
        let child_env = ob::child_env();
        let is_worktree = ob::detect_git_worktree(input.vault, &child_env)
            .map_err(|error| {
                GmailError::runtime(
                    "git",
                    format!("could not determine whether the vault is a Git worktree: {error}"),
                )
            })?;

        let plan = build_attempt(
            input.vault,
            input.mail_folder,
            input.link_template,
            input.inbox,
            std::mem::take(&mut candidates),
            input.adoptions,
            &enrollment,
            &watchers,
            &index,
            false,
            is_worktree,
            input.saved_at,
            input.timezone,
        )?;
        match persist_attempt(input.vault, &child_env, is_worktree, plan) {
            Ok(outcome) => return Ok(outcome),
            Err((error, plan))
                if error.kind() == "note_changed" && attempt == 0 =>
            {
                candidates = plan.into_retry_candidates();
            }
            Err((error, plan)) if error.kind() == "note_changed" => {
                let mut outcome = plan.outcome;
                for result in &mut outcome.results {
                    if matches!(result.status, "filed" | "adopted") {
                        outcome.deferred.push(result.key.clone());
                    }
                }
                outcome
                    .results
                    .retain(|result| !outcome.deferred.contains(&result.key));
                outcome.failed = true;
                outcome.changed = false;
                outcome.warnings.push(
                    "vault files changed again during filing; remaining threads were deferred".to_string(),
                );
                let _ = error;
                return Ok(outcome);
            }
            Err((error, _)) => return Err(error),
        }
    }

    unreachable!("the filing retry loop returns after its bounded attempts")
}

fn build_attempt(
    vault: &Path,
    mail_folder: &Path,
    link_template: &str,
    inbox: &str,
    candidates: Vec<FilingCandidate>,
    initial_adoptions: &[AdoptionIntent],
    enrollment: &EnrollmentFold,
    watchers: &WatchScan,
    index: &MailIndex,
    dry_run: bool,
    is_worktree: bool,
    saved_at: DateTime<Utc>,
    timezone: chrono_tz::Tz,
) -> Result<AttemptPlan, GmailError> {
    let membership = Membership::new(enrollment, watchers);
    let route_context = RouteContext::load(vault);
    let initial_adoption_keys: BTreeSet<_> = initial_adoptions
        .iter()
        .map(|intent| intent.key.clone())
        .collect();
    let candidate_keys: BTreeSet<_> = candidates
        .iter()
        .filter_map(|candidate| candidate_key(candidate).ok())
        .collect();
    let mut outcome = FilingOutcome::default();
    outcome
        .warnings
        .extend(enrollment.warnings().iter().cloned());
    outcome.warnings.extend(watchers.warnings().iter().cloned());
    let mut works = BTreeMap::new();
    let mut destinations = BTreeMap::new();
    let mut enrollment_events = Vec::new();
    let mut enrolled_events = BTreeSet::new();

    for candidate in candidates {
        let key = match candidate_key(&candidate) {
            Ok(key) => key,
            Err(error) => {
                outcome.warnings.push(format!(
                    "skipped a prepared Gmail candidate with an invalid identity: {}",
                    error.message()
                ));
                continue;
            }
        };
        match membership.decide(&key) {
            CandidateDecision::Enrolled(origin) => {
                outcome
                    .results
                    .push(already_enrolled(&key, origin, enrollment, index));
            }
            CandidateDecision::Adopt(intent) => {
                record_adoption(
                    vault,
                    &intent,
                    index,
                    &mut outcome,
                    &mut enrollment_events,
                    &mut enrolled_events,
                );
            }
            CandidateDecision::File => {
                if initial_adoption_keys.contains(&key) {
                    outcome.deferred.push(key);
                    continue;
                }
                match destination_for_route(
                    vault,
                    inbox,
                    &candidate.route,
                    &route_context,
                ) {
                    Ok(destination) => {
                        destinations.insert(key.clone(), destination);
                        works.insert(key.index_key(), candidate.save);
                    }
                    Err(error) => outcome.failures.push(FilingFailure {
                        key,
                        kind: error.kind().to_string(),
                        message: error.message().to_string(),
                    }),
                }
            }
        }
    }

    // A task that was open when Gmail fetching began may have been closed or
    // deleted during that work. Never turn that stale adoption into a new
    // filing. Recheck each initial adoption from the fresh scan above.
    for intent in initial_adoptions {
        if candidate_keys.contains(&intent.key) {
            continue;
        }
        match membership.decide(&intent.key) {
            CandidateDecision::Enrolled(origin) => outcome
                .results
                .push(already_enrolled(&intent.key, origin, enrollment, index)),
            CandidateDecision::Adopt(current) => record_adoption(
                vault,
                &current,
                index,
                &mut outcome,
                &mut enrollment_events,
                &mut enrolled_events,
            ),
            CandidateDecision::File => {
                outcome.deferred.push(intent.key.clone());
            }
        }
    }

    let mut route_parent_links = BTreeMap::new();
    for (key, destination) in &destinations {
        if let Some(route) = &destination.route {
            route_parent_links.insert(key.index_key(), format!("[[{route}]]"));
        }
    }

    // Probe each candidate independently so one corrupt or ambiguous saved
    // note does not discard other valid candidates. The final batch pass below
    // restores shared filename and attachment collision accounting.
    let keys: Vec<_> = works.keys().cloned().collect();
    for key in keys {
        let Some(work) = works.remove(&key) else {
            continue;
        };
        let thread_key = ThreadKey::new(&key.0, &key.1)
            .expect("prepared save identity was checked above");
        let mut one = BTreeMap::from([(key.clone(), work)]);
        let parent = route_parent_links
            .get(&key)
            .map(|parent| BTreeMap::from([(key.clone(), parent.clone())]))
            .unwrap_or_default();
        match save::plan_prepared_saves_with_parent_links(
            vault,
            mail_folder,
            link_template,
            &mut one,
            index,
            None,
            &parent,
            false,
            saved_at,
            timezone,
        ) {
            Ok(_) => works.extend(one),
            Err(error) => {
                destinations.remove(&thread_key);
                outcome.failures.push(FilingFailure {
                    key: thread_key,
                    kind: error.kind().to_string(),
                    message: error.message().to_string(),
                });
            }
        }
    }

    let plans = match save::plan_prepared_saves_with_parent_links(
        vault,
        mail_folder,
        link_template,
        &mut works,
        index,
        None,
        &route_parent_links,
        false,
        saved_at,
        timezone,
    ) {
        Ok(plans) => plans,
        Err(error) => {
            for (account, thread_id) in works.keys() {
                if let Some(key) = ThreadKey::new(account, thread_id) {
                    outcome.failures.push(FilingFailure {
                        key,
                        kind: error.kind().to_string(),
                        message: error.message().to_string(),
                    });
                }
            }
            works.clear();
            destinations.clear();
            Vec::new()
        }
    };

    let mut staged = plans
        .iter()
        .filter(|plan| plan.changed)
        .map(save::staged_note)
        .collect::<Result<Vec<_>, _>>()?;
    let mut snapshots = load_destinations(vault, inbox, &destinations)?;
    let today = bob_env::current_datetime()
        .date()
        .format("%Y-%m-%d")
        .to_string();
    for plan in &plans {
        let Some(key) = ThreadKey::new(&plan.account, &plan.thread_id) else {
            continue;
        };
        let Some(destination) = destinations.get(&key) else {
            continue;
        };
        let Some(snapshot) = snapshots.get_mut(&destination.relative) else {
            continue;
        };
        let block =
            task_block(&plan.link, &today, destination.problem.as_ref());
        snapshot.updated =
            capture::insert_task_line(&snapshot.updated, &block).0;
    }
    let mut task_commit_paths = Vec::new();
    for snapshot in snapshots.values() {
        if snapshot.original.as_deref() != Some(snapshot.updated.as_str()) {
            task_commit_paths.push(
                snapshot
                    .target
                    .strip_prefix(vault)
                    .map_err(|_| {
                        GmailError::runtime(
                            "refused",
                            "a task destination escaped the vault".to_string(),
                        )
                    })?
                    .to_path_buf(),
            );
            staged.push(StagedTextFile {
                target: snapshot.target.clone(),
                target_existed: snapshot.original.is_some(),
                original_target: snapshot.original.clone().unwrap_or_default(),
                updated_target: snapshot.updated.clone(),
            });
        }
    }

    for plan in &plans {
        let Some(key) = ThreadKey::new(&plan.account, &plan.thread_id) else {
            continue;
        };
        let Some(destination) = destinations.get(&key) else {
            continue;
        };
        let note_path = path_relative(vault, &plan.path)?;
        outcome.results.push(FilingResult {
            key: key.clone(),
            note: Some(note_path.clone()),
            link: Some(plan.link.clone()),
            task_file: Some(save::path_to_slashes(&destination.relative)),
            status: if dry_run { "would_file" } else { "filed" },
            route: destination.route.clone(),
            via: Some(destination.via),
            problem: destination.problem.clone(),
            first_seen: destination.problem.is_some(),
        });
        if !dry_run {
            enrollment_events.push(EnrollmentEvent::Filed {
                key,
                note: note_path,
                task_file: save::path_to_slashes(&destination.relative),
                via: destination.via,
                route: destination.route.clone(),
            });
        }
    }

    let auxiliary = if plans.iter().any(|plan| plan.changed) {
        let child_env = ob::child_env();
        save::plan_auxiliary_files(
            vault,
            mail_folder,
            is_worktree,
            &child_env,
            true,
        )?
    } else {
        save::AuxiliaryFiles {
            staged: Vec::new(),
            commit_paths: Vec::new(),
        }
    };
    staged.extend(auxiliary.staged.iter().map(save::clone_staged));
    save::validate_write_targets(vault, &plans, &works, &staged)?;

    outcome
        .results
        .sort_by(|left, right| left.key.cmp(&right.key));
    outcome
        .failures
        .sort_by(|left, right| left.key.cmp(&right.key));
    outcome.deferred.sort();
    outcome.deferred.dedup();
    outcome.changed = !staged.is_empty() || !enrollment_events.is_empty();

    Ok(AttemptPlan {
        outcome,
        works,
        destinations,
        plans,
        staged,
        task_commit_paths,
        auxiliary_commit_paths: auxiliary.commit_paths,
        enrollment_events,
    })
}

fn persist_attempt(
    vault: &Path,
    child_env: &ob::ChildEnv,
    is_worktree: bool,
    mut plan: AttemptPlan,
) -> Result<FilingOutcome, (GmailError, AttemptPlan)> {
    let facts = match save::save_event_facts(
        vault,
        child_env,
        is_worktree,
        &plan.plans,
        &plan.works,
    ) {
        Ok(facts) => facts,
        Err(error) => return Err((error, plan)),
    };

    let mut commit_paths =
        match commit_paths(vault, child_env, is_worktree, &plan, &facts) {
            Ok(paths) => paths,
            Err(error) => return Err((error, plan)),
        };
    let created_images = match save::write_prepared_batch(
        vault,
        &plan.works,
        &plan.plans,
        &plan.staged,
    ) {
        Ok(paths) => paths,
        Err(error) => return Err((error, plan)),
    };
    commit_paths
        .extend(created_images.into_iter().map(|(relative, _)| relative));
    commit_paths.sort();
    commit_paths.dedup();

    if is_worktree && !commit_paths.is_empty() {
        let count = plan.enrollment_events.len();
        if let Err(error) = ob::commit_paths(
            vault,
            child_env,
            &format!("gmail: sync {count} threads"),
            &commit_paths,
        ) {
            return Err((
                GmailError::runtime(
                    "uncommitted",
                    format!("Gmail files were written but could not be committed: {error}"),
                )
                .with_hint("run `bob gmail sync` again to finish the scoped commit"),
                plan,
            ));
        }
    }

    let host = vault_sync::hostname_slug();
    let journaled_notes: Vec<_> = plan
        .plans
        .iter()
        .filter(|note| {
            note.changed
                || note.path.strip_prefix(vault).ok().is_some_and(|relative| {
                    commit_paths.iter().any(|path| path == relative)
                })
        })
        .collect();
    let mut origins = Vec::new();
    for note in &journaled_notes {
        let key = (note.account.clone(), note.thread_id.clone());
        let Some(event_facts) = facts.get(&key) else {
            continue;
        };
        if let Err(error) =
            save::append_save_event(vault, &host, note, event_facts)
        {
            plan.outcome.failed = true;
            plan.outcome
                .warnings
                .push(format!("could not append Gmail save journal: {error}"));
        }
        origins.extend(event_facts.origins.iter().cloned());
    }
    for failure in
        attachment_journal::append_origin_events(vault, &origins, "gmail")
    {
        plan.outcome.warnings.push(format!(
            "could not record imported image {}: {}",
            failure.saved, failure.error
        ));
    }

    for event in &plan.enrollment_events {
        if let Err(error) = enroll::append(vault, &host, event) {
            plan.outcome.failed = true;
            plan.outcome.warnings.push(format!(
                "could not append Gmail enrollment journal: {error}"
            ));
        }
    }
    Ok(plan.outcome)
}

fn destination_for_route(
    vault: &Path,
    inbox: &str,
    verdict: &RouteVerdict,
    context: &RouteContext,
) -> Result<Destination, GmailError> {
    match verdict {
        RouteVerdict::Route(parent) => {
            let Some(current) = context.resolve_route(&parent.route) else {
                return Err(GmailError::runtime(
                    "route_changed",
                    "the routed area or project changed during Gmail preparation".to_string(),
                ));
            };
            if current.route != parent.route {
                return Err(GmailError::runtime(
                    "route_changed",
                    "the routed destination no longer matches the resolved label".to_string(),
                ));
            }
            let relative =
                save::safe_relative_path(&format!("{}.md", parent.route))?;
            save::validate_target_path(vault, &relative)?;
            let metadata = fs::symlink_metadata(vault.join(&relative))
                .map_err(|error| {
                    GmailError::runtime(
                        "route_changed",
                        format!(
                            "the routed destination is unavailable: {error}"
                        ),
                    )
                })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(GmailError::runtime(
                    "refused",
                    "the routed destination is not a regular in-vault note"
                        .to_string(),
                ));
            }
            Ok(Destination {
                relative,
                via: FiledVia::Route,
                route: Some(parent.route.clone()),
                problem: None,
                verdict: RouteVerdict::Route(parent.clone()),
            })
        }
        RouteVerdict::Unrouted => inbox_destination(
            vault,
            inbox,
            FiledVia::Inbox,
            None,
            RouteVerdict::Unrouted,
        ),
        RouteVerdict::Problem(problem) => inbox_destination(
            vault,
            inbox,
            FiledVia::Problem,
            Some(problem.clone()),
            RouteVerdict::Problem(problem.clone()),
        ),
    }
}

fn inbox_destination(
    vault: &Path,
    inbox: &str,
    via: FiledVia,
    problem: Option<RouteProblem>,
    verdict: RouteVerdict,
) -> Result<Destination, GmailError> {
    let relative = save::safe_relative_path(inbox)?;
    save::validate_target_path(vault, &relative)?;
    Ok(Destination {
        relative,
        via,
        route: None,
        problem,
        verdict,
    })
}

fn load_destinations(
    vault: &Path,
    inbox: &str,
    destinations: &BTreeMap<ThreadKey, Destination>,
) -> Result<BTreeMap<PathBuf, DestinationSnapshot>, GmailError> {
    let mut snapshots = BTreeMap::new();
    for destination in destinations.values() {
        if snapshots.contains_key(&destination.relative) {
            continue;
        }
        let target = vault.join(&destination.relative);
        let (original, updated) = match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(GmailError::runtime(
                        "refused",
                        "a task destination is not a regular in-vault note"
                            .to_string(),
                    ));
                }
                let text = fs::read_to_string(&target).map_err(|error| {
                    GmailError::runtime(
                        "write",
                        format!("could not read task destination: {error}"),
                    )
                })?;
                (Some(text.clone()), text)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if destination.relative == PathBuf::from(inbox) {
                    (None, inbox_template())
                } else {
                    return Err(GmailError::runtime(
                        "route_changed",
                        "the routed task destination disappeared during filing"
                            .to_string(),
                    ));
                }
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "path",
                    format!("could not inspect a task destination: {error}"),
                ));
            }
        };
        snapshots.insert(
            destination.relative.clone(),
            DestinationSnapshot {
                target,
                original,
                updated,
            },
        );
    }
    Ok(snapshots)
}

fn record_adoption(
    vault: &Path,
    intent: &AdoptionIntent,
    index: &MailIndex,
    outcome: &mut FilingOutcome,
    events: &mut Vec<EnrollmentEvent>,
    recorded: &mut BTreeSet<ThreadKey>,
) {
    let note_path = match save::safe_relative_path(&intent.note) {
        Ok(path) => path,
        Err(error) => {
            outcome.failures.push(FilingFailure {
                key: intent.key.clone(),
                kind: error.kind().to_string(),
                message: error.message().to_string(),
            });
            return;
        }
    };
    let task_path = match save::safe_relative_path(&intent.task_file) {
        Ok(path) => path,
        Err(error) => {
            outcome.failures.push(FilingFailure {
                key: intent.key.clone(),
                kind: error.kind().to_string(),
                message: error.message().to_string(),
            });
            return;
        }
    };
    if let Err(error) = save::validate_target_path(vault, &note_path)
        .and_then(|()| save::validate_target_path(vault, &task_path))
    {
        outcome.failures.push(FilingFailure {
            key: intent.key.clone(),
            kind: error.kind().to_string(),
            message: error.message().to_string(),
        });
        return;
    }
    for relative in [&note_path, &task_path] {
        let exists_as_file = fs::symlink_metadata(vault.join(relative))
            .is_ok_and(|metadata| {
                metadata.is_file() && !metadata.file_type().is_symlink()
            });
        if !exists_as_file {
            outcome.failures.push(FilingFailure {
                key: intent.key.clone(),
                kind: "note_changed".to_string(),
                message:
                    "the adopted mail note or task disappeared during filing"
                        .to_string(),
            });
            return;
        }
    }
    let entry = index.by_identity.get(&intent.key.index_key());
    outcome.results.push(FilingResult {
        key: intent.key.clone(),
        note: Some(intent.note.clone()),
        link: entry.map(|entry| entry.label.clone()),
        task_file: Some(intent.task_file.clone()),
        status: "adopted",
        route: None,
        via: None,
        problem: None,
        first_seen: false,
    });
    if recorded.insert(intent.key.clone()) {
        events.push(intent.event());
    }
}

fn already_enrolled(
    key: &ThreadKey,
    origin: EnrollmentOrigin,
    enrollment: &EnrollmentFold,
    index: &MailIndex,
) -> FilingResult {
    let latest = enrollment.latest(key);
    let entry = index.by_identity.get(&key.index_key());
    FilingResult {
        key: key.clone(),
        note: entry
            .map(|entry| entry.path.clone())
            .or_else(|| latest.and_then(|record| record.note.clone())),
        link: entry.map(|entry| entry.label.clone()),
        task_file: latest.and_then(|record| record.task_file.clone()),
        status: match origin {
            EnrollmentOrigin::Legacy => "already_pulled",
            EnrollmentOrigin::Sync => "already_filed",
        },
        route: latest.and_then(|record| record.route.clone()),
        via: latest.and_then(|record| record.via),
        problem: None,
        first_seen: false,
    }
}

fn path_relative(vault: &Path, absolute: &Path) -> Result<String, GmailError> {
    let relative = absolute.strip_prefix(vault).map_err(|_| {
        GmailError::runtime(
            "refused",
            "a planned mail note escaped the vault".to_string(),
        )
    })?;
    Ok(save::path_to_slashes(relative))
}

pub(crate) fn inbox_template() -> String {
    "---\nparent: \"[[inbox]]\"\ntype: \"[[area]]\"\nready_cap: off\n---\n\n# Gmail inbox\n\n## Tasks\n".to_string()
}

fn task_block(
    link: &str,
    created: &str,
    problem: Option<&RouteProblem>,
) -> String {
    let mut block = capture::format_task_line(link, created, None, None);
    if let Some(problem) = problem {
        block.push_str("\n\t- ⚠️ **GMAIL ROUTE:** `");
        block.push_str(&problem.label.replace('`', "\\`"));
        block.push_str("` ");
        block.push_str(&problem.message);
        if !problem.hint.is_empty() {
            block.push_str(" — ");
            block.push_str(&problem.hint);
        }
    }
    block
}

fn commit_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    is_worktree: bool,
    plan: &AttemptPlan,
    facts: &BTreeMap<(String, String), SaveEventFacts>,
) -> Result<Vec<PathBuf>, GmailError> {
    let mut paths = plan.auxiliary_commit_paths.clone();
    paths.extend(plan.task_commit_paths.iter().cloned());
    for note in &plan.plans {
        let relative = note
            .path
            .strip_prefix(vault)
            .map_err(|_| {
                GmailError::runtime(
                    "refused",
                    "a planned mail note escaped the vault".to_string(),
                )
            })?
            .to_path_buf();
        let dirty = if is_worktree {
            save::git_path_is_dirty(vault, child_env, &relative)
                .map_err(|error| GmailError::runtime("git", error))?
        } else {
            false
        };
        if note.changed || dirty {
            paths.push(relative);
        }
        if let Some(event_facts) =
            facts.get(&(note.account.clone(), note.thread_id.clone()))
        {
            for (saved, _, _) in &event_facts.origins {
                let relative = save::safe_relative_path(saved)?;
                if is_worktree {
                    if save::git_path_is_dirty(vault, child_env, &relative)
                        .map_err(|error| GmailError::runtime("git", error))?
                    {
                        paths.push(relative);
                    }
                }
            }
        }
    }

    // A previous commit may have failed after durable writes. Include only
    // this transaction's adoption targets when they remain dirty.
    if is_worktree {
        for result in &plan.outcome.results {
            if result.status != "adopted" {
                continue;
            }
            for relative in
                [result.note.as_deref(), result.task_file.as_deref()]
                    .into_iter()
                    .flatten()
            {
                let relative = save::safe_relative_path(relative)?;
                if save::git_path_is_dirty(vault, child_env, &relative)
                    .map_err(|error| GmailError::runtime("git", error))?
                {
                    paths.push(relative);
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn acquire_vault_lock<S: Sleeper>(
    budget: &SharedBudget,
    sleeper: &S,
) -> Result<Option<std::fs::File>, GmailError> {
    loop {
        match ob::try_acquire_lock() {
            Ok(file) => return Ok(Some(file)),
            Err(ob::LockAcquireError::Contended) => {
                let remaining = budget.remaining();
                if remaining.is_zero() {
                    return Ok(None);
                }
                sleeper.sleep(remaining.min(Duration::from_millis(100)));
            }
            Err(ob::LockAcquireError::Open { path, error }) => {
                return Err(GmailError::runtime(
                    "lock",
                    format!(
                        "could not open the vault lock {}: {error}",
                        path.display()
                    ),
                ));
            }
            Err(ob::LockAcquireError::Acquire { path, error }) => {
                return Err(GmailError::runtime(
                    "lock",
                    format!(
                        "could not acquire the vault lock {}: {error}",
                        path.display()
                    ),
                ));
            }
        }
    }
}

fn candidate_key(candidate: &FilingCandidate) -> Result<ThreadKey, GmailError> {
    ThreadKey::new(
        &candidate.save.confirmed.identity.account,
        &candidate.save.confirmed.identity.thread_id,
    )
    .ok_or_else(|| {
        GmailError::runtime(
            "unresolvable",
            "the prepared Gmail candidate has an invalid identity".to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, process::Command, time::Duration};

    use crate::native::{
        config::GmailBodyMode,
        env::TestEnvGuard,
        gmail::{
            model::GmailThread,
            prepare,
            resolve::{ConfirmedThread, GmailRead, ResolvedIdentity},
        },
        parent_notes,
    };

    struct NoRemote;

    impl GmailRead for NoRemote {
        fn attachment_bytes(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, GmailError> {
            Err(unexpected_remote())
        }

        fn thread_get(
            &self,
            _: &str,
            _: &str,
        ) -> Result<GmailThread, GmailError> {
            Err(unexpected_remote())
        }

        fn message_metadata(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, GmailError> {
            Err(unexpected_remote())
        }

        fn search_page(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            Err(unexpected_remote())
        }

        fn labels_list(
            &self,
            _: &str,
        ) -> Result<Vec<super::super::super::gog::GmailLabel>, GmailError>
        {
            Err(unexpected_remote())
        }
    }

    fn unexpected_remote() -> GmailError {
        GmailError::runtime(
            "test",
            "filing attempted an unexpected Gmail request".to_string(),
        )
    }

    struct NoSleep;

    impl Sleeper for NoSleep {
        fn sleep(&self, _: Duration) {}
    }

    fn test_env(root: &Path) -> TestEnvGuard {
        let lock = root.join("maintenance.lock");
        TestEnvGuard::set(&[
            ("BOB_NOW", Some(OsStr::new("2026-11-07 12:00:00"))),
            ("BOB_VAULT_SYNC_LOCK_FILE", Some(lock.as_os_str())),
        ])
    }

    fn candidate(thread_id: &str, route: RouteVerdict) -> FilingCandidate {
        let thread = GmailThread::from_gog_json(
            format!(
                r#"{{"thread":{{"id":"{thread_id}","messages":[{{"id":"e{thread_id}","threadId":"{thread_id}","internalDate":"1794070800000","snippet":"A short preview","payload":{{"mimeType":"text/plain","headers":[{{"name":"From","value":"Ada Example <ada@example.com>"}},{{"name":"Subject","value":"Lease renewal"}}],"body":{{"size":5,"data":"SGVsbG8="}}}}}}]}}}}"#
            )
            .as_bytes(),
        )
        .expect("valid test thread");
        let confirmed = ConfirmedThread {
            identity: ResolvedIdentity {
                account: "me@example.com".to_string(),
                thread_id: thread_id.to_string(),
                known_path: None,
            },
            thread,
        };
        let prepared = prepare::prepare_thread(
            &NoRemote,
            "me@example.com",
            &confirmed.thread,
            GmailBodyMode::Headers,
        )
        .expect("headers mode does not fetch bodies");
        FilingCandidate {
            save: PreparedSave {
                confirmed,
                prepared,
                body_mode: GmailBodyMode::Headers,
            },
            route,
        }
    }

    fn area(vault: &Path, route: &str, headings: &str) {
        fs::write(
            vault.join(format!("{route}.md")),
            format!("---\ntype: \"[[area]]\"\n---\n\n# {route}\n\n{headings}"),
        )
        .expect("write area note");
    }

    fn git(vault: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn route(vault: &Path, name: &str) -> RouteVerdict {
        RouteVerdict::Route(
            RouteContext::load(vault)
                .resolve_route(name)
                .expect("area route resolves"),
        )
    }

    fn apply_one(
        vault: &Path,
        candidate: FilingCandidate,
        fold: &EnrollmentFold,
        watchers: &WatchScan,
        dry_run: bool,
    ) -> FilingOutcome {
        let budget = SharedBudget::new(Duration::ZERO);
        apply(
            FilingInput {
                vault,
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates: vec![candidate],
                adoptions: &[],
                enrollment: fold,
                watchers,
                dry_run,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
            },
            &NoSleep,
        )
        .expect("filing succeeds")
    }

    #[test]
    fn routed_task_lands_in_ready_block_and_mail_note_uses_route_parent() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(
            vault,
            "job",
            "## Tasks\n\n### Next & In Progress\n\n### Done & Canceled\n",
        );
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let outcome = apply_one(
            vault,
            candidate("aabb", route(vault, "job")),
            &fold,
            &watchers,
            false,
        );

        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].status, "filed");
        assert_eq!(outcome.results[0].task_file.as_deref(), Some("job.md"));
        let task_note = fs::read_to_string(vault.join("job.md")).unwrap();
        let task_position = task_note.find("- [ ] #task").unwrap();
        let next_position = task_note.find("### Next & In Progress").unwrap();
        assert!(task_position < next_position, "{task_note}");
        assert!(task_note.contains("[created::2026-11-07]"), "{task_note}");
        assert!(!task_note.contains("[fresh::"));

        let mail_path = outcome.results[0].note.as_ref().unwrap();
        let mail_note = fs::read_to_string(vault.join(mail_path)).unwrap();
        assert!(
            mail_note.starts_with("---\nparent: '[[job]]'\n"),
            "{mail_note}"
        );
        let journal = fs::read_to_string(vault.join(format!(
            ".bob/gmail/{}.jsonl",
            crate::native::vault_sync::hostname_slug()
        )))
        .unwrap();
        assert!(journal.contains("gmail.filed"));
    }

    #[test]
    fn problem_filing_creates_inbox_and_managed_first_child() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let problem = RouteProblem {
            kind: crate::native::gmail::route::RouteProblemKind::UnknownNote,
            label: "#/missing".to_string(),
            message: "unknown note".to_string(),
            hint: "create the note or change the label".to_string(),
        };
        let outcome = apply_one(
            vault,
            candidate("ccdd", RouteVerdict::Problem(problem)),
            &fold,
            &watchers,
            false,
        );

        let result = &outcome.results[0];
        assert_eq!(result.via, Some(FiledVia::Problem));
        assert!(result.first_seen);
        let inbox = fs::read_to_string(vault.join("gmail_inbox.md")).unwrap();
        assert!(inbox.contains("parent: \"[[inbox]]\""));
        assert!(inbox.contains("type: \"[[area]]\""));
        assert!(inbox.contains("ready_cap: off"));
        assert!(inbox.contains(
            "\n\t- ⚠️ **GMAIL ROUTE:** `#/missing` unknown note — create the note or change the label"
        ));
    }

    #[test]
    fn one_batch_routes_threads_to_different_parents_and_allocates_collisions()
    {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(vault, "job", "## Tasks\n");
        area(vault, "body", "## Tasks\n");
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let budget = SharedBudget::new(Duration::ZERO);
        let candidates = vec![
            candidate("aabb", route(vault, "job")),
            candidate("ccdd", route(vault, "body")),
        ];
        let outcome = apply(
            FilingInput {
                vault,
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates,
                adoptions: &[],
                enrollment: &fold,
                watchers: &watchers,
                dry_run: false,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
            },
            &NoSleep,
        )
        .expect("batch files");

        assert_eq!(outcome.results.len(), 2);
        let note_paths: BTreeSet<_> = outcome
            .results
            .iter()
            .map(|result| result.note.as_ref().unwrap().clone())
            .collect();
        assert_eq!(note_paths.len(), 2, "same-title notes need distinct paths");
        let parents: BTreeSet<_> = note_paths
            .iter()
            .map(|relative| {
                let note = fs::read_to_string(vault.join(relative)).unwrap();
                assert!(note.contains("parent:"), "{note}");
                if note.starts_with("---\nparent: '[[job]]'") {
                    "job"
                } else if note.starts_with("---\nparent: '[[body]]'") {
                    "body"
                } else {
                    panic!("unexpected note parent:\n{note}")
                }
            })
            .collect();
        assert_eq!(
            parents,
            BTreeSet::from(["body", "job"]),
            "notes: {note_paths:?}"
        );
    }

    #[test]
    fn refreshing_an_existing_note_preserves_its_parent_notes_and_reply() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(vault, "job", "## Tasks\n");
        area(vault, "body", "## Tasks\n");
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let outcome = apply_one(
            vault,
            candidate("aabb", route(vault, "job")),
            &fold,
            &watchers,
            false,
        );
        let note_path = outcome.results[0].note.as_ref().unwrap();
        let absolute = vault.join(note_path);
        let mut contents = fs::read_to_string(&absolute).unwrap();
        contents = contents
            .replace("parent: '[[job]]'", "parent: '[[custom_parent]]'");
        contents = contents.replace(
            "## Notes\n\n## Conversation",
            "## Notes\n\nKeep this note.\n\n## Conversation",
        );
        contents.push_str("\n## Reply\n\nDraft reply stays here.\n");
        fs::write(&absolute, contents).unwrap();

        let index = resolve::scan_index(
            vault,
            Path::new("mail"),
            "✉️ {subject} · {who}",
        )
        .unwrap();
        let mut candidate = candidate("aabb", route(vault, "body"));
        candidate.save.confirmed.identity.known_path = Some(note_path.clone());
        let key = ThreadKey::new("me@example.com", "aabb").unwrap();
        let parent_links =
            BTreeMap::from([(key.index_key(), "[[body]]".to_string())]);
        let mut works = BTreeMap::from([(key.index_key(), candidate.save)]);
        let plans = save::plan_prepared_saves_with_parent_links(
            vault,
            Path::new("mail"),
            "✉️ {subject} · {who}",
            &mut works,
            &index,
            None,
            &parent_links,
            false,
            Utc::now(),
            chrono_tz::UTC,
        )
        .unwrap();
        let updated = &plans[0].updated;
        assert!(updated.starts_with("---\nparent: '[[custom_parent]]'\n"));
        assert!(updated.contains("Keep this note."));
        assert!(updated.contains("## Reply\n\nDraft reply stays here."));
    }

    #[test]
    fn git_apply_uses_one_sync_commit_and_leaves_unrelated_dirt_alone() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(vault, "job", "## Tasks\n");
        git(vault, &["init", "-q"]);
        git(vault, &["config", "user.name", "Sync Test"]);
        git(vault, &["config", "user.email", "sync@example.test"]);
        git(vault, &["add", "job.md"]);
        git(vault, &["commit", "-q", "-m", "base"]);
        fs::write(
            vault.join("unrelated.md"),
            "leave this outside the sync commit\n",
        )
        .unwrap();

        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let outcome = apply_one(
            vault,
            candidate("aabb", route(vault, "job")),
            &fold,
            &watchers,
            false,
        );
        assert_eq!(outcome.results[0].status, "filed");
        assert_eq!(
            git(vault, &["log", "-1", "--pretty=%s"]),
            "gmail: sync 1 threads"
        );
        assert_eq!(
            git(vault, &["status", "--porcelain", "--", "unrelated.md"]),
            "?? unrelated.md"
        );
    }

    #[test]
    fn dry_run_plans_filing_without_creating_files_or_directories() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        area(vault, "job", "## Tasks\n");
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let outcome = apply_one(
            vault,
            candidate("aabb", route(vault, "job")),
            &fold,
            &watchers,
            true,
        );
        assert_eq!(outcome.results[0].status, "would_file");
        assert!(!vault.join("mail").exists());
        assert!(!vault.join("gmail_inbox.md").exists());
        assert!(!vault.join(".bob").exists());
    }

    #[test]
    fn an_adoption_closed_during_fetch_is_deferred_without_filing() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        let _env = test_env(vault);
        let key = ThreadKey::new("me@example.com", "aabb").unwrap();
        let stale = AdoptionIntent {
            key: key.clone(),
            note: "mail/2026/lease.md".to_string(),
            task_file: "job.md".to_string(),
            task_line: 1,
            watchers: 1,
        };
        let fold = EnrollmentFold::load(vault).unwrap();
        let watchers = WatchScan::default();
        let budget = SharedBudget::new(Duration::ZERO);
        let outcome = apply(
            FilingInput {
                vault,
                mail_folder: Path::new("mail"),
                link_template: "✉️ {subject} · {who}",
                inbox: "gmail_inbox.md",
                candidates: Vec::new(),
                adoptions: std::slice::from_ref(&stale),
                enrollment: &fold,
                watchers: &watchers,
                dry_run: false,
                saved_at: Utc::now(),
                timezone: chrono_tz::UTC,
                budget: &budget,
            },
            &NoSleep,
        )
        .expect("stale adoption is deferred");

        assert_eq!(outcome.deferred, vec![key]);
        assert!(outcome.results.is_empty());
        assert!(!vault.join(".bob").exists());
    }

    #[test]
    fn route_problem_child_and_ready_placement_are_stable() {
        let problem = RouteProblem {
            kind: crate::native::gmail::route::RouteProblemKind::UnknownNote,
            label: "#/unknown".to_string(),
            message: "unknown route".to_string(),
            hint: "use one label per note".to_string(),
        };
        let block = task_block(
            "[[mail/lease|✉️ Lease · Ada]]",
            "2026-11-07",
            Some(&problem),
        );
        let original = "## Tasks\n\n### Next & In Progress\n- [ ] #task Active\n\n### Done & Canceled\n- [x] #task Finished\n";
        let updated = capture::insert_task_line(original, &block).0;
        let row = updated.find("- [ ] #task [[mail/lease").unwrap();
        let child = updated.find("\n\t- ⚠️ **GMAIL ROUTE:**").unwrap();
        let next = updated.find("### Next & In Progress").unwrap();
        let done = updated.find("### Done & Canceled").unwrap();
        assert!(row < child && child < next && next < done, "{updated}");
        assert!(updated.contains("[created::2026-11-07]"));
        assert!(!updated.contains("[fresh::"));
    }

    #[test]
    fn missing_inbox_template_keeps_the_capture_parent_and_ready_cap() {
        let template = inbox_template();
        assert!(template.contains("parent: \"[[inbox]]\""));
        assert!(template.contains("type: \"[[area]]\""));
        assert!(template.contains("ready_cap: off"));
        assert!(template.ends_with("## Tasks\n"));
    }

    #[test]
    fn route_context_still_uses_parent_note_eligibility() {
        let temp = tempfile::tempdir().expect("temporary vault");
        area(temp.path(), "job", "## Tasks\n");
        let resolved = parent_notes::resolve_parent(temp.path(), "job")
            .expect("area resolves as a capture parent");
        assert_eq!(resolved.route, "job");
    }
}
