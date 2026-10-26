//! Staged, task-preserving overwrite transaction.
//!
//! Route renderers hand this module private staged files. The executor owns
//! the locked recheck, publication, sync verification, scoped commit, and
//! rollback boundary; it never queues a half-published overwrite.

use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::cell::Cell;

use sha2::{Digest, Sha256};

use super::super::reading_plan::ReadingTaskAction;
use super::super::{
    frontmatter, guard, marker, note, ob, reference_mutation, sync,
    CommandError, Config, MarkerValue, PdfSyncOutcome, Projection, SyncOptions,
    FIELD_ID, FIELD_PARENT, FIELD_STATUS,
};
use super::{recovery, OverwritePreflight, OverwriteRequest};
use crate::native::file_coordination::Coordination;
use crate::native::ref_library;
use crate::native::ref_tasks;

const OVERWRITE_SUBJECT_PREFIX: &str = "bob ref create: overwrite";

/// Test-only injection points for the overwrite executor. Never compiled
/// into release builds and never read from a CLI flag or environment
/// variable.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverwriteFailpoint {
    AfterStagingValidation,
    AfterIntentPersistence,
    AfterFrozenNote,
    AfterSidecarRemoval,
    AfterAudio,
    AfterPdf,
    AfterNote,
    AfterTaskEdit,
    AfterSyncWrite,
    ForcedVerificationFailure,
    BeforeCommit,
    CommitReportedAsError,
    PostSyncFailure,
    CrashWithoutRollback,
}

#[cfg(test)]
thread_local! {
    static FAILPOINT: Cell<Option<OverwriteFailpoint>> = Cell::new(None);
}

#[cfg(test)]
fn failpoint_is(point: OverwriteFailpoint) -> bool {
    FAILPOINT.with(|cell| cell.get() == Some(point))
}

#[cfg(test)]
fn trip(point: OverwriteFailpoint) -> Result<(), CommandError> {
    if failpoint_is(point) {
        Err(CommandError::new(format!("injected failpoint {point:?}"))
            .with_code("injected_failpoint"))
    } else {
        Ok(())
    }
}

/// Final marker identity expected from the private PDF renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::native::highlights_ref) struct FinalMarkerMetadata {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) parent: Option<String>,
    pub(crate) captured: Option<String>,
}

/// A requested audio replacement. The destination is vault-relative and
/// must have been included in the read-only overwrite preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::native::highlights_ref) struct StagedAudio {
    pub(crate) private_path: PathBuf,
    pub(crate) destination: String,
}

/// Files produced in private scratch by a capture route. Absence of audio
/// means preserve the existing companion, including under `--no-audio`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::native::highlights_ref) struct StagedOverwrite {
    pub(crate) private_pdf: PathBuf,
    pub(crate) audio: Option<StagedAudio>,
    pub(crate) marker: FinalMarkerMetadata,
}

/// Structured facts for a route-specific success renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::native::highlights_ref) struct OverwriteResult {
    pub(crate) note: String,
    pub(crate) pdf: String,
    pub(crate) archive: Option<String>,
    pub(crate) task_path: String,
    pub(crate) task_block_id: Option<String>,
    pub(crate) task_mark: char,
    /// Final task status after applying an explicit `-s`, else the
    /// preserved pre-overwrite status.
    pub(crate) task_status: String,
    /// Highlight count of the replaced reading, for the success line.
    pub(crate) highlights: u64,
    pub(crate) commit_sha: String,
    pub(crate) push_pending: bool,
}

/// Apply one prepared overwrite. Network-backed vault sync runs while the
/// two mutation locks are held; tests and future local-only callers use the
/// private service seam below.
pub(in crate::native::highlights_ref) fn execute(
    config: &Config,
    initial: &OverwritePreflight,
    request: &OverwriteRequest,
    staged: StagedOverwrite,
) -> Result<OverwriteResult, CommandError> {
    execute_with_sync(config, initial, request, staged, |pre| {
        let report =
            crate::native::vault_sync::run_cycle_with_existing_lock_report(
                &ob::child_env(),
                true,
            );
        if report.ok && report.conflicts.is_empty() {
            Ok(())
        } else if report.conflicts.is_empty() {
            Err(report.error.unwrap_or_else(|| {
                if pre {
                    "pre-sync reported failure".to_string()
                } else {
                    "post-sync reported failure".to_string()
                }
            }))
        } else {
            Err(format!(
                "vault sync conflict in {}",
                report.conflicts.join(", ")
            ))
        }
    })
}

fn execute_with_sync(
    config: &Config,
    initial: &OverwritePreflight,
    request: &OverwriteRequest,
    staged: StagedOverwrite,
    mut vault_sync: impl FnMut(bool) -> Result<(), String>,
) -> Result<OverwriteResult, CommandError> {
    validate_staged_files(config, &staged)?;
    validate_staged_metadata(initial, request, &staged)?;
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterStagingValidation)?;

    let child_env = ob::child_env();
    let _locks = reference_mutation::acquire_mutation_session(
        &config.bob_dir,
        &child_env,
        || eprintln!("waiting for another vault maintenance run…"),
    )
    .map_err(|error| match error {
        reference_mutation::MutationSessionError::Maintenance(error) => {
            CommandError::new(format!(
                "could not acquire the vault maintenance lock: {error}"
            ))
            .with_code("lock_busy")
        }
        reference_mutation::MutationSessionError::NotAWorktree => {
            CommandError::new("in-place overwrite requires the Bob vault to be a Git worktree")
                .with_code("not_a_worktree")
        }
        reference_mutation::MutationSessionError::Scan(error) => error,
    })?;

    recovery::recover_with_locks_held(config, &child_env, false)?;
    vault_sync(true).map_err(|detail| {
        CommandError::new(format!("pre-sync failed: {detail}"))
            .with_code("sync_failed")
    })?;

    // Re-derive selection after the locked pre-sync, then re-plan. A
    // default selection must still be the newest live member; an explicit
    // `-N` member must still exist in the same family. The exact prepared
    // manifest, selected task, archive destination, and all preimages must
    // still match the renderer's original plan; the command never silently
    // retargets a changed source.
    let selected = super::reselect_after_presync(config, initial, request)?;
    let locked =
        super::prepare(config, &selected, request, &initial.planned_date)
            .map_err(|refusal| refusal.command_error())?;
    if locked.bundle_sha256 != initial.bundle_sha256
        || locked.task != initial.task
        || locked.note_path != initial.note_path
        || locked.pdf_path != initial.pdf_path
    {
        return Err(CommandError::new(
            "overwrite target changed while the replacement was rendering; no files were written",
        )
        .with_code("preimage_changed"));
    }
    ensure_staged_audio_is_preflighted(request, &locked, &staged)?;

    let base_head = reference_mutation::git_head(&config.bob_dir, &child_env)
        .map_err(|error| {
        CommandError::new(format!("could not read vault HEAD: {error}"))
    })?;
    let subject = format!(
        "{OVERWRITE_SUBJECT_PREFIX} {}/{}",
        locked.ref_type, locked.member.stem
    );
    let manifest = locked
        .bundle_preimages
        .iter()
        .map(|preimage| PathBuf::from(&preimage.path))
        .collect::<BTreeSet<_>>();
    let transaction = super::super::marker_write::MarkerWriteSession::coordinate_explicit_pdf_write(
        &config.bob_dir.join(&locked.pdf_path),
        || {
            // Coordination asks an open Highlights document to save before
            // the first vault mutation. Re-plan inside the accessor so a
            // presenter save cannot be overwritten as if it were our write.
            let current = super::prepare(
                config,
                &locked.member,
                request,
                &locked.planned_date,
            )
            .map_err(|refusal| refusal.command_error())?;
            if !same_locked_preflight(&locked, &current) {
                return Err(CommandError::new(
                    "overwrite target changed during PDF coordination; no files were written",
                )
                .with_code("preimage_changed"));
            }

            let mut intent = recovery::OverwriteIntent::for_transaction(
                config,
                &locked.member.stem,
                &locked.ref_type,
                &base_head,
                subject.clone(),
                &locked.bundle_preimages,
            )?;
            if let Err(error) = recovery::save_intent(config, &intent) {
                let rollback = recovery::rollback_transaction(
                    config,
                    &child_env,
                    &intent,
                );
                return Err(CommandError::new(format!(
                    "could not persist overwrite intent: {error}{}",
                    rollback_suffix(rollback)
                ))
                .with_human_code("verify_failed"));
            }
            #[cfg(test)]
            if let Err(error) = trip(OverwriteFailpoint::AfterIntentPersistence)
            {
                let rollback = recovery::rollback_transaction(
                    config,
                    &child_env,
                    &intent,
                );
                return Err(CommandError::new(format!(
                    "{error}{}",
                    rollback_suffix(rollback)
                ))
                .with_human_code("verify_failed"));
            }

            let applied =
                apply_and_verify(config, &locked, request, &staged, &manifest);
            #[cfg(test)]
            if failpoint_is(OverwriteFailpoint::CrashWithoutRollback)
                && applied.is_ok()
            {
                return Err(CommandError::new(
                    "injected crash after applying the overwrite",
                )
                .with_code("injected_crash"));
            }
            let task_mark = match applied {
                Ok((_, task_mark)) => task_mark,
                Err(error) => {
                    let rollback = recovery::rollback_transaction(
                        config,
                        &child_env,
                        &intent,
                    );
                    return Err(CommandError::new(format!(
                        "{error}{}",
                        rollback_suffix(rollback)
                    ))
                    .with_human_code("verify_failed"));
                }
            };

            let paths = match manifest_commit_paths(
                &config.bob_dir,
                &child_env,
                &manifest,
            ) {
                Ok(paths) => paths,
                Err(error) => {
                    let rollback = recovery::rollback_transaction(
                        config,
                        &child_env,
                        &intent,
                    );
                    return Err(CommandError::new(format!(
                        "could not build scoped overwrite commit paths: {error}{}",
                        rollback_suffix(rollback)
                    ))
                    .with_human_code("verify_failed"));
                }
            };
            if paths.is_empty() {
                let rollback = recovery::rollback_transaction(
                    config,
                    &child_env,
                    &intent,
                );
                return Err(CommandError::new(format!(
                    "verified overwrite produced no scoped Git paths{}",
                    rollback_suffix(rollback)
                ))
                .with_human_code("verify_failed"));
            }
            #[cfg(test)]
            if let Err(error) = trip(OverwriteFailpoint::BeforeCommit) {
                let rollback = recovery::rollback_transaction(
                    config,
                    &child_env,
                    &intent,
                );
                return Err(CommandError::new(format!(
                    "{error}{}",
                    rollback_suffix(rollback)
                ))
                .with_human_code("verify_failed"));
            }
            let commit_result = {
                let result = ob::commit_paths(
                    &config.bob_dir,
                    &child_env,
                    &subject,
                    &paths,
                );
                #[cfg(test)]
                if failpoint_is(OverwriteFailpoint::CommitReportedAsError)
                    && matches!(result, Ok(Some(_)))
                {
                    Err("injected commit reporting error".to_string())
                } else {
                    result
                }
                #[cfg(not(test))]
                {
                    result
                }
            };
            let sha = match commit_result {
                Ok(Some(sha)) => sha,
                Ok(None) => {
                    let rollback = recovery::rollback_transaction(
                        config,
                        &child_env,
                        &intent,
                    );
                    return Err(CommandError::new(format!(
                        "verified overwrite produced no scoped Git commit{}",
                        rollback_suffix(rollback)
                    ))
                    .with_human_code("verify_failed"));
                }
                Err(error) => {
                    if let Some((sha, committed_subject)) =
                        reference_mutation::committed_despite_error(
                            &config.bob_dir,
                            &child_env,
                            Some(&base_head),
                            &subject,
                        )
                        && committed_subject == subject
                    {
                        sha
                    } else {
                        let rollback = recovery::rollback_transaction(
                            config,
                            &child_env,
                            &intent,
                        );
                        return Err(CommandError::new(format!(
                            "Git commit failed: {error}{}",
                            rollback_suffix(rollback)
                        ))
                        .with_human_code("verify_failed"));
                    }
                }
            };

            if !prove_commit(
                &config.bob_dir,
                &child_env,
                &base_head,
                &sha,
                &subject,
            ) {
                return Err(CommandError::new(format!(
                    "overwrite commit could not be proved; left its intent in place for recovery (HEAD is {sha})"
                ))
                .with_code("commit_unverified"));
            }
            // A crash before this update is still recoverable from the exact
            // HEAD commit subject; storing the SHA makes proof unambiguous.
            let mut push_pending =
                intent.record_expected_head(config, &sha).is_err();
            if recovery::clear_intent(config).is_err() {
                push_pending = true;
            }

            Ok(OverwriteResult {
                note: locked.note_path.clone(),
                pdf: locked.pdf_path.clone(),
                archive: locked
                    .frozen_note
                    .as_ref()
                    .map(|archive| archive.path.clone()),
                task_path: locked.task.path.clone(),
                task_block_id: locked.task.block_id.clone(),
                task_mark,
                task_status: request
                    .explicit_status
                    .clone()
                    .unwrap_or_else(|| locked.status.clone()),
                highlights: locked.member.highlights.unwrap_or(0),
                commit_sha: sha,
                push_pending,
            })
        },
    );

    let mut result = match transaction {
        Ok(Coordination::Granted {
            value: Ok(value), ..
        }) => value,
        Ok(Coordination::Granted {
            value: Err(error), ..
        }) => {
            return Err(error);
        }
        Ok(Coordination::Held(timeout)) => {
            return Err(CommandError::new(format!(
                "PDF coordination timed out after {:.2}s; no overwrite was written",
                timeout.waited.as_secs_f64()
            ))
            .with_code("coordination_timeout"));
        }
        Ok(Coordination::Failed(detail)) => {
            return Err(CommandError::new(format!(
                "PDF coordination failed: {detail}; no overwrite was written"
            ))
            .with_code("coordination_failed"));
        }
        Err(error) => return Err(error),
    };
    let post_sync = vault_sync(false);
    #[cfg(test)]
    let post_sync = if failpoint_is(OverwriteFailpoint::PostSyncFailure) {
        Err("injected post-sync failure".to_string())
    } else {
        post_sync
    };
    if post_sync.is_err() {
        result.push_pending = true;
    }
    Ok(result)
}

fn same_locked_preflight(
    expected: &OverwritePreflight,
    current: &OverwritePreflight,
) -> bool {
    expected.bundle_sha256 == current.bundle_sha256
        && expected.task == current.task
        && expected.member.path == current.member.path
        && expected.member.stem == current.member.stem
        && expected.member.reading == current.member.reading
        && expected.note_path == current.note_path
        && expected.pdf_path == current.pdf_path
        && expected.ref_type == current.ref_type
        && expected.frozen_note == current.frozen_note
        && expected.status == current.status
        && expected.audio_path == current.audio_path
}

fn rollback_suffix(result: Result<(), CommandError>) -> String {
    match result {
        Ok(()) => "; rolled back the staged overwrite".to_string(),
        Err(error) => format!(
            "; rollback failed and the overwrite intent remains: {error}"
        ),
    }
}

fn prove_commit(
    vault: &Path,
    child_env: &ob::ChildEnv,
    base_head: &str,
    commit: &str,
    expected_subject: &str,
) -> bool {
    let subject = ob::git_command(vault, child_env)
        .args(["log", "-1", "--format=%s", commit])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        });
    if subject.as_deref() != Some(expected_subject) {
        return false;
    }
    let Some(head) = reference_mutation::git_head(vault, child_env).ok() else {
        return false;
    };
    head != base_head
        && ob::git_command(vault, child_env)
            .args(["merge-base", "--is-ancestor", commit, &head])
            .output()
            .is_ok_and(|output| output.status.success())
}

fn manifest_commit_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    manifest: &BTreeSet<PathBuf>,
) -> Result<Vec<PathBuf>, CommandError> {
    let mut paths = Vec::new();
    for relative in manifest {
        let absolute = vault.join(relative);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.is_file() => paths.push(relative.clone()),
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(CommandError::new(format!(
                    "unsupported overwrite path type: {}",
                    relative.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let tracked = ob::git_command(vault, child_env)
                    .args(["ls-files", "--error-unmatch", "--"])
                    .arg(relative)
                    .output()
                    .map_err(|error| {
                        CommandError::new(format!(
                            "could not inspect scoped Git path: {error}"
                        ))
                    })?;
                if tracked.status.success() {
                    paths.push(relative.clone());
                }
            }
            Err(error) => {
                return Err(CommandError::new(format!(
                    "could not inspect overwrite path {}: {error}",
                    relative.display()
                )));
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn validate_staged_files(
    config: &Config,
    staged: &StagedOverwrite,
) -> Result<(), CommandError> {
    let vault = fs::canonicalize(&config.bob_dir).map_err(|error| {
        CommandError::new(format!("could not resolve Bob vault: {error}"))
    })?;
    for (label, path) in std::iter::once(("PDF", &staged.private_pdf)).chain(
        staged
            .audio
            .iter()
            .map(|audio| ("audio", &audio.private_path)),
    ) {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            CommandError::new(format!(
                "could not read staged {label} {}: {error}",
                path.display()
            ))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CommandError::new(format!(
                "staged {label} is not a regular file: {}",
                path.display()
            )));
        }
        let canonical = fs::canonicalize(path).map_err(|error| {
            CommandError::new(format!(
                "could not resolve staged {label}: {error}"
            ))
        })?;
        if canonical.starts_with(&vault) {
            return Err(CommandError::new(format!(
                "staged {label} must stay outside the Bob vault until publication"
            )));
        }
    }
    Ok(())
}

fn validate_staged_metadata(
    initial: &OverwritePreflight,
    request: &OverwriteRequest,
    staged: &StagedOverwrite,
) -> Result<(), CommandError> {
    if staged.marker.id != initial.member.stem {
        return Err(CommandError::new(format!(
            "staged PDF identity {} does not match selected reference {}",
            staged.marker.id, initial.member.stem
        )));
    }
    if staged.marker.title.trim().is_empty() {
        return Err(CommandError::new("staged PDF has an empty title"));
    }
    if let Some(title) = &request.explicit_title
        && staged.marker.title != *title
    {
        return Err(CommandError::new(
            "staged PDF title does not match the explicit overwrite title",
        ));
    }
    let expected_parent = request
        .resolved_parent
        .as_deref()
        .or(initial.parent.as_deref());
    if expected_parent.is_some_and(|parent| {
        staged.marker.parent.as_deref().is_none_or(|staged_parent| {
            normalize_route(parent) != normalize_route(staged_parent)
        })
    }) {
        return Err(CommandError::new(
            "staged PDF parent does not match the selected reference parent",
        ));
    }
    if let Some(status) = &request.explicit_status
        && status != "blocked"
        && staged.marker.status != marker_status_for_request(status)?
    {
        return Err(CommandError::new(
            "staged PDF status does not match the explicit overwrite status",
        ));
    }
    if request.explicit_status.is_none() {
        let status_matches = match initial.status.as_str() {
            "blocked" => matches!(
                staged.marker.status.as_str(),
                "ready" | "next" | "wip"
            ),
            status => status == staged.marker.status,
        };
        if !status_matches {
            return Err(CommandError::new(
                "staged PDF status would change the selected reading task",
            ));
        }
    }
    if staged.marker.captured.as_deref().is_none_or(str::is_empty) {
        return Err(CommandError::new(
            "staged PDF is missing the refreshed captured date",
        ));
    }
    Ok(())
}

fn ensure_staged_audio_is_preflighted(
    request: &OverwriteRequest,
    plan: &OverwritePreflight,
    staged: &StagedOverwrite,
) -> Result<(), CommandError> {
    match (&staged.audio, &request.replacement_audio_path) {
        (None, _) => Ok(()),
        (Some(audio), Some(expected))
            if &audio.destination == expected
                && plan
                    .bundle_preimages
                    .iter()
                    .any(|preimage| preimage.path == audio.destination) =>
        {
            Ok(())
        }
        (Some(_), _) => Err(CommandError::new(
            "replacement audio destination was not included in overwrite preflight",
        )),
    }
}

fn apply_and_verify(
    config: &Config,
    plan: &OverwritePreflight,
    request: &OverwriteRequest,
    staged: &StagedOverwrite,
    manifest: &BTreeSet<PathBuf>,
) -> Result<(String, char), CommandError> {
    let pdf = config.bob_dir.join(&plan.pdf_path);
    let note_path = config.bob_dir.join(&plan.note_path);
    let old_note = fs::read_to_string(&note_path).map_err(|error| {
        CommandError::new(format!(
            "could not read selected reference note: {error}"
        ))
    })?;
    let original_task_file = fs::read(config.bob_dir.join(&plan.task.path))
        .map_err(|error| {
            CommandError::new(format!("could not read selected task: {error}"))
        })?;
    if hex::encode(Sha256::digest(&original_task_file)) != plan.task.file_sha256
    {
        return Err(CommandError::new(
            "selected reading task changed after locked preflight",
        ));
    }

    let marker = marker::read_pdf_marker(&staged.private_pdf)?;
    let mut projection =
        marker::parse_marker_with_normalization(&marker.contents)?.projection;
    ensure_marker_matches(&projection, &staged.marker)?;
    apply_final_projection(plan, request, staged, &mut projection)?;

    let expected_mark = request
        .explicit_status
        .as_deref()
        .map(status_to_mark)
        .transpose()?
        .unwrap_or(plan.task.mark);
    let expected_subtree = rewrite_task_subtree(
        &plan.task.subtree,
        &plan.note_path,
        &plan.pdf_path,
        &staged.marker.title,
        expected_mark,
        request.explicit_status.is_some(),
        staged.marker.title != plan.title,
    )?;

    let mut all_paths = BTreeSet::new();
    for path in [&plan.note_path, &plan.pdf_path, &plan.task.path] {
        all_paths.insert(PathBuf::from(path));
    }
    if let Some(audio) = &staged.audio {
        all_paths.insert(PathBuf::from(&audio.destination));
    }
    if !all_paths.is_subset(manifest) {
        return Err(CommandError::new(format!(
            "overwrite transaction predicted an unplanned write: {}",
            all_paths
                .difference(manifest)
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    if let Some(archive) = &plan.frozen_note {
        write_new_file(
            &config.bob_dir.join(&archive.path),
            archive.contents.as_bytes(),
        )?;
    }
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterFrozenNote)?;

    remove_old_sidecar(config, plan)?;
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterSidecarRemoval)?;
    publish_audio(config, plan, staged)?;
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterAudio)?;
    write_expected_file(config, plan, &plan.pdf_path, &staged.private_pdf)?;
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterPdf)?;

    let body = super::render_fresh_skeleton(
        config,
        &plan.note_path,
        &pdf,
        Path::new(&plan.pdf_path),
        &projection,
        &plan.task_view,
        &staged.marker.title,
        current_audio_absolute(config, plan, staged).as_deref(),
    );
    let body = if plan.task.path == plan.note_path {
        replace_inline_tracker(&body, &expected_subtree)?
    } else {
        body
    };
    let note_contents = fresh_note_contents(
        &old_note,
        &body,
        plan.frozen_note
            .as_ref()
            .map(|archive| archive.path.as_str()),
        &plan.planned_date,
    )?;
    write_expected_bytes(
        config,
        plan,
        &plan.note_path,
        note_contents.as_bytes(),
    )?;
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterNote)?;

    if plan.task.path != plan.note_path {
        replace_task_file_subtree(
            &config.bob_dir.join(&plan.task.path),
            &original_task_file,
            plan.task.line_index,
            &plan.task.subtree,
            &expected_subtree,
        )?;
    }
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterTaskEdit)?;

    let sync_plan = sync::plan_single_pdf_sync(
        config,
        &pdf,
        SyncOptions {
            dry_run: false,
            write_pdf: false,
            prefer: None,
            ignore_unannotated_sidecar: false,
        },
    )?;
    ensure_sync_writes_are_scoped(config, &sync_plan, manifest)?;
    if !matches!(
        sync_plan
            .reading_task_plan
            .as_ref()
            .map(|reading| &reading.action),
        Some(ReadingTaskAction::NoWrite)
    ) {
        return Err(CommandError::new(
            "replacement sync planned a reading-task change or birth",
        ));
    }
    match sync::write_single_pdf_sync_with_external_coordination(
        config, &sync_plan,
    )? {
        PdfSyncOutcome::Written(_) => {}
        PdfSyncOutcome::Held(hold) => {
            return Err(CommandError::new(hold.message))
        }
    }
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterSyncWrite)?;

    let verification = sync::plan_single_pdf_sync(
        config,
        &pdf,
        SyncOptions {
            dry_run: true,
            write_pdf: false,
            prefer: None,
            ignore_unannotated_sidecar: false,
        },
    )?;
    if verification.stable_note_action != "none"
        || verification.marker_write_needed
        || !matches!(
            verification
                .reading_task_plan
                .as_ref()
                .map(|reading| &reading.action),
            Some(ReadingTaskAction::NoWrite)
        )
    {
        return Err(CommandError::new(
            "second dry-run single-PDF sync was not a no-op",
        ));
    }

    verify_identity_and_links(config, plan, &staged.marker)?;
    verify_task_subtree(
        config,
        plan,
        &expected_subtree,
        expected_mark,
        &original_task_file,
    )?;
    #[cfg(test)]
    trip(OverwriteFailpoint::ForcedVerificationFailure)?;
    Ok((expected_subtree, expected_mark))
}

fn apply_final_projection(
    plan: &OverwritePreflight,
    request: &OverwriteRequest,
    staged: &StagedOverwrite,
    projection: &mut Projection,
) -> Result<(), CommandError> {
    projection.insert(
        "title".to_string(),
        MarkerValue::String(staged.marker.title.clone()),
    );
    if let Some(parent) = request
        .resolved_parent
        .as_deref()
        .or(staged.marker.parent.as_deref())
        .or(plan.parent.as_deref())
    {
        projection.insert(
            FIELD_PARENT.to_string(),
            MarkerValue::String(parent.to_string()),
        );
    }
    if let Some(status) = request.explicit_status.as_deref()
        && status != "blocked"
    {
        projection.insert(
            FIELD_STATUS.to_string(),
            MarkerValue::String(marker_status_for_request(status)?.to_string()),
        );
    }
    Ok(())
}

fn ensure_marker_matches(
    projection: &Projection,
    expected: &FinalMarkerMetadata,
) -> Result<(), CommandError> {
    let id = projection
        .get(FIELD_ID)
        .and_then(MarkerValue::as_string)
        .unwrap_or_default();
    let title = projection
        .get("title")
        .and_then(MarkerValue::as_string)
        .unwrap_or_default();
    let status = projection
        .get(FIELD_STATUS)
        .and_then(MarkerValue::as_string)
        .unwrap_or_default();
    let parent = projection
        .get(FIELD_PARENT)
        .and_then(MarkerValue::as_string);
    let captured = projection.get("captured").and_then(MarkerValue::as_string);
    if id != expected.id || title != expected.title || status != expected.status
    {
        return Err(CommandError::new(
            "staged PDF marker changed after the route validated its final metadata",
        ));
    }
    if expected.parent.as_deref().is_some_and(|expected_parent| {
        parent.is_none_or(|parent| {
            normalize_route(parent) != normalize_route(expected_parent)
        })
    }) {
        return Err(CommandError::new(
            "staged PDF parent does not match the selected reference parent",
        ));
    }
    if expected
        .captured
        .as_deref()
        .is_some_and(|expected_captured| captured != Some(expected_captured))
    {
        return Err(CommandError::new(
            "staged PDF capture date does not match its final marker metadata",
        ));
    }
    Ok(())
}

fn remove_old_sidecar(
    config: &Config,
    plan: &OverwritePreflight,
) -> Result<(), CommandError> {
    let pdf = config.bob_dir.join(&plan.pdf_path);
    let Some(sidecar) = super::super::sidecar::discover_sidecar_path(&pdf)?
    else {
        return Ok(());
    };
    let bundle_root = sidecar.parent().filter(|parent| {
        parent
            .extension()
            .is_some_and(|extension| extension == "textbundle")
    });
    if let Some(root) = bundle_root {
        let root_rel = root
            .strip_prefix(&config.bob_dir)
            .map_err(|_| CommandError::new("sidecar escaped the Bob vault"))?;
        let root_name = reference_mutation::vault_path(root_rel);
        if !plan
            .bundle_preimages
            .iter()
            .any(|preimage| preimage.path == root_name)
        {
            return Err(CommandError::new(
                "sidecar directory appeared after overwrite preflight",
            ));
        }
        let mut files = plan
            .bundle_preimages
            .iter()
            .filter(|preimage| {
                Path::new(&preimage.path).starts_with(root_rel)
                    && preimage.sha256.is_some()
                    && config.bob_dir.join(&preimage.path).is_file()
            })
            .map(|preimage| PathBuf::from(&preimage.path))
            .collect::<Vec<_>>();
        files.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for relative in files {
            verify_expected_preimage(
                config,
                plan,
                &relative.to_string_lossy(),
            )?;
            fs::remove_file(config.bob_dir.join(relative)).map_err(
                |error| {
                    CommandError::new(format!(
                        "could not remove stale sidecar file: {error}"
                    ))
                },
            )?;
        }
        let mut directories = plan
            .bundle_preimages
            .iter()
            .filter(|preimage| Path::new(&preimage.path).starts_with(root_rel))
            .map(|preimage| config.bob_dir.join(&preimage.path))
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        directories
            .sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for directory in directories {
            let empty = fs::read_dir(&directory)
                .map_err(|error| {
                    CommandError::new(format!(
                        "could not inspect stale sidecar directory: {error}"
                    ))
                })?
                .next()
                .is_none();
            if empty {
                fs::remove_dir(directory).map_err(|error| {
                    CommandError::new(format!(
                        "could not remove stale sidecar directory: {error}"
                    ))
                })?;
            } else if directory == root {
                return Err(CommandError::new(
                    "sidecar directory changed after overwrite preflight",
                ));
            }
        }
    } else {
        let relative = sidecar
            .strip_prefix(&config.bob_dir)
            .map_err(|_| CommandError::new("sidecar escaped the Bob vault"))?;
        let relative = reference_mutation::vault_path(relative);
        verify_expected_preimage(config, plan, &relative)?;
        fs::remove_file(sidecar).map_err(|error| {
            CommandError::new(format!(
                "could not remove stale sidecar: {error}"
            ))
        })?;
    }
    Ok(())
}

fn publish_audio(
    config: &Config,
    plan: &OverwritePreflight,
    staged: &StagedOverwrite,
) -> Result<(), CommandError> {
    let Some(audio) = &staged.audio else {
        return Ok(());
    };
    if plan
        .audio_path
        .as_deref()
        .is_some_and(|old| old != audio.destination)
    {
        let old_relative = plan.audio_path.as_deref().unwrap();
        verify_expected_preimage(config, plan, old_relative)?;
        let old = config.bob_dir.join(old_relative);
        fs::remove_file(old).map_err(|error| {
            CommandError::new(format!(
                "could not remove replaced companion audio: {error}"
            ))
        })?;
    }
    let bytes = fs::read(&audio.private_path).map_err(|error| {
        CommandError::new(format!("could not read staged audio: {error}"))
    })?;
    write_expected_bytes(config, plan, &audio.destination, &bytes)
}

fn write_expected_file(
    config: &Config,
    plan: &OverwritePreflight,
    relative: &str,
    source: &Path,
) -> Result<(), CommandError> {
    let bytes = fs::read(source).map_err(|error| {
        CommandError::new(format!("could not read staged PDF: {error}"))
    })?;
    write_expected_bytes(config, plan, relative, &bytes)
}

fn write_expected_bytes(
    config: &Config,
    plan: &OverwritePreflight,
    relative: &str,
    bytes: &[u8],
) -> Result<(), CommandError> {
    verify_expected_preimage(config, plan, relative)?;
    let destination = config.bob_dir.join(relative);
    let parent = destination.parent().unwrap_or(&config.bob_dir);
    fs::create_dir_all(parent).map_err(|error| {
        CommandError::new(format!(
            "could not create overwrite destination directory: {error}"
        ))
    })?;
    reference_mutation::atomic_write_bytes(&destination, bytes).map_err(
        |error| {
            CommandError::new(format!(
                "could not install overwrite file: {error}"
            ))
        },
    )
}

fn verify_expected_preimage(
    config: &Config,
    plan: &OverwritePreflight,
    relative: &str,
) -> Result<(), CommandError> {
    let preimage = plan
        .bundle_preimages
        .iter()
        .find(|preimage| preimage.path == relative)
        .ok_or_else(|| {
            CommandError::new(format!(
                "overwrite path was not preflighted: {relative}"
            ))
        })?;
    let path = config.bob_dir.join(relative);
    let actual = match fs::read(&path) {
        Ok(bytes) => Some(hex::encode(Sha256::digest(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(CommandError::new(format!(
                "could not recheck overwrite path {relative}: {error}"
            )));
        }
    };
    if actual != preimage.sha256 {
        return Err(CommandError::new(format!(
            "overwrite path changed after locked preflight: {relative}"
        )));
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "could not create frozen-note directory: {error}"
            ))
        })?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
        CommandError::new(format!(
            "could not create frozen note {} without replacement: {error}",
            path.display()
        ))
    })?;
    file.write_all(bytes).map_err(|error| {
        CommandError::new(format!(
            "could not write frozen note {}: {error}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        CommandError::new(format!(
            "could not sync frozen note {}: {error}",
            path.display()
        ))
    })
}

fn current_audio_absolute(
    config: &Config,
    plan: &OverwritePreflight,
    staged: &StagedOverwrite,
) -> Option<PathBuf> {
    staged
        .audio
        .as_ref()
        .map(|audio| config.bob_dir.join(&audio.destination))
        .or_else(|| {
            plan.audio_path
                .as_ref()
                .map(|path| config.bob_dir.join(path))
        })
}

fn fresh_note_contents(
    existing: &str,
    body: &str,
    archive: Option<&str>,
    date: &str,
) -> Result<String, CommandError> {
    let lined = match archive {
        Some(path) => super::prepend_archive_lineage(existing, path, date, 0)
            .map_err(CommandError::new)?,
        None => existing.to_string(),
    };
    let parsed = frontmatter::parse_note(&lined);
    let retained = retain_note_local_frontmatter(parsed.frontmatter);
    let note = super::super::ParsedNote {
        frontmatter: with_recaptured(retained, date),
        body: body.to_string(),
        original: None,
    };
    Ok(note.render_original())
}

fn retain_note_local_frontmatter(
    entries: Vec<super::super::FrontmatterEntry>,
) -> Vec<super::super::FrontmatterEntry> {
    let mut retained = Vec::new();
    let mut skipping_continuations = false;
    for entry in entries {
        let keep = match entry.key.as_deref() {
            Some("created" | "earlier" | "recaptured") => true,
            Some(key) => {
                !super::super::is_managed_frontmatter_field(key)
                    && !super::super::is_standard_user_field(key)
            }
            None => !skipping_continuations,
        };
        match entry.key.as_deref() {
            Some(key) => skipping_continuations = !keep && key != "",
            None if keep => skipping_continuations = false,
            None => {}
        }
        if keep && entry.key.as_deref() != Some("recaptured") {
            retained.push(entry);
        }
    }
    retained
}

fn with_recaptured(
    mut entries: Vec<super::super::FrontmatterEntry>,
    date: &str,
) -> Vec<super::super::FrontmatterEntry> {
    entries.push(super::super::FrontmatterEntry {
        key: Some("recaptured".to_string()),
        value: Some(MarkerValue::String(date.to_string())),
        raw: format!(
            "recaptured: {}",
            MarkerValue::String(date.to_string()).as_frontmatter_value()
        ),
    });
    entries
}

fn replace_inline_tracker(
    body: &str,
    subtree: &str,
) -> Result<String, CommandError> {
    let trackers = ref_tasks::find_trackers(body);
    if trackers.len() != 1 {
        return Err(CommandError::new(
            "fresh replacement skeleton did not contain exactly one managed reading task",
        ));
    }
    let index = body
        .lines()
        .position(|line| line.trim() == trackers[0].line)
        .ok_or_else(|| {
            CommandError::new(
                "could not locate the generated reading-task line",
            )
        })?;
    let segments = body.split_inclusive('\n').collect::<Vec<_>>();
    let mut rendered = String::new();
    for (current, segment) in segments.iter().enumerate() {
        if current == index {
            rendered.push_str(subtree);
            if !subtree.ends_with('\n') && segment.ends_with('\n') {
                rendered.push('\n');
            }
        } else {
            rendered.push_str(segment);
        }
    }
    Ok(rendered)
}

fn rewrite_task_subtree(
    subtree: &str,
    note_path: &str,
    pdf_path: &str,
    title: &str,
    target_mark: char,
    change_status: bool,
    require_title_link: bool,
) -> Result<String, CommandError> {
    let (line, rest) = subtree.split_once('\n').unwrap_or((subtree, ""));
    let carriage_return = line.ends_with('\r');
    let line = line.strip_suffix('\r').unwrap_or(line);
    // The alias is rewritten only on a real title change; an unchanged
    // title keeps the task line (and any custom alias) byte-identical.
    let (mut updated, title_link_found) = if require_title_link {
        rewrite_task_alias(line, note_path, pdf_path, title)
    } else {
        (line.to_string(), true)
    };
    if require_title_link && !title_link_found {
        return Err(CommandError::new(
            "selected reading task has no link to the reference whose title changed",
        ));
    }
    if change_status {
        let stripped = ref_tasks::strip_blockquote_prefix(&updated);
        let old_mark = ref_tasks::task_mark(stripped).ok_or_else(|| {
            CommandError::new("selected reading task no longer has a checkbox")
        })?;
        let marker = format!("[{old_mark}]");
        let at = updated.find(&marker).ok_or_else(|| {
            CommandError::new(
                "could not locate the selected reading task checkbox",
            )
        })?;
        updated.replace_range(at + 1..at + 2, &target_mark.to_string());
        if matches!(target_mark, 'x' | 'X' | '-') {
            updated = note::stamp_close_date(&updated, target_mark);
        }
    }
    if carriage_return {
        updated.push('\r');
    }
    let mut result = updated;
    if subtree.contains('\n') {
        result.push('\n');
        result.push_str(rest);
    }
    Ok(result)
}

fn rewrite_task_alias(
    line: &str,
    note_path: &str,
    pdf_path: &str,
    title: &str,
) -> (String, bool) {
    let wanted = [note_path, pdf_path]
        .iter()
        .map(|path| normalize_route(path))
        .collect::<HashSet<_>>();
    let mut output = String::with_capacity(line.len() + title.len());
    let mut cursor = 0usize;
    let mut found = false;
    while let Some(start_rel) = line[cursor..].find("[[") {
        let start = cursor + start_rel;
        output.push_str(&line[cursor..start]);
        let content_start = start + 2;
        let Some(end_rel) = line[content_start..].find("]]") else {
            output.push_str(&line[start..]);
            return (output, found);
        };
        let end = content_start + end_rel;
        let inner = &line[content_start..end];
        let (target, old_alias) = inner.split_once('|').unwrap_or((inner, ""));
        if wanted.contains(&normalize_route(target)) {
            found = true;
            output.push_str("[[");
            output.push_str(target);
            output.push('|');
            output.push_str(&ref_tasks::overwrite_task_alias(title, old_alias));
            output.push_str("]]");
        } else {
            output.push_str(&line[start..end + 2]);
        }
        cursor = end + 2;
    }
    output.push_str(&line[cursor..]);
    (output, found)
}

fn replace_task_file_subtree(
    path: &Path,
    original: &[u8],
    line_index: usize,
    expected: &str,
    replacement: &str,
) -> Result<(), CommandError> {
    let current = fs::read(path).map_err(|error| {
        CommandError::new(format!(
            "could not re-read reading task {}: {error}",
            path.display()
        ))
    })?;
    if current != original {
        return Err(CommandError::new(
            "selected reading task changed after the locked recheck",
        ));
    }
    let contents = String::from_utf8(current).map_err(|error| {
        CommandError::new(format!("reading task is not UTF-8: {error}"))
    })?;
    let mut lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    let end = task_subtree_end(&lines, line_index);
    let current_subtree = lines[line_index..end].concat();
    if normalized_subtree(&current_subtree) != normalized_subtree(expected) {
        return Err(CommandError::new("selected reading task subtree changed"));
    }
    let replacement_lines =
        replacement.split_inclusive('\n').collect::<Vec<_>>();
    lines.splice(line_index..end, replacement_lines);
    reference_mutation::atomic_write_bytes(path, lines.concat().as_bytes())
        .map_err(|error| {
            CommandError::new(format!("could not update reading task: {error}"))
        })
}

fn verify_task_subtree(
    config: &Config,
    plan: &OverwritePreflight,
    expected: &str,
    expected_mark: char,
    original_file: &[u8],
) -> Result<(), CommandError> {
    let path = config.bob_dir.join(&plan.task.path);
    let contents = fs::read_to_string(&path).map_err(|error| {
        CommandError::new(format!("could not verify reading task: {error}"))
    })?;
    let lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    let line_index = find_task_line(&lines, &plan.task, expected)?;
    let end = task_subtree_end(&lines, line_index);
    let actual = lines[line_index..end].concat();
    if normalized_subtree(&actual) != normalized_subtree(expected) {
        return Err(CommandError::new(
            "reading-task children or content changed during overwrite",
        ));
    }
    let line = lines[line_index].trim_end_matches(['\r', '\n']);
    if ref_tasks::task_mark(ref_tasks::strip_blockquote_prefix(line))
        != Some(expected_mark)
    {
        return Err(CommandError::new(
            "reading-task checkbox does not match the requested status",
        ));
    }

    if plan.task.path != plan.note_path {
        let expected_file = replace_subtree_in_bytes(
            original_file,
            plan.task.line_index,
            &plan.task.subtree,
            expected,
        )?;
        if fs::read(&path).ok().as_deref() != Some(expected_file.as_slice()) {
            return Err(CommandError::new(
                "unrelated reading-task file content changed during overwrite",
            ));
        }
    }
    let index =
        ref_tasks::RefTaskIndex::build(&config.bob_dir, &config.ref_dir);
    let note_key = plan.note_path.as_str();
    let matching = index
        .candidates(note_key)
        .iter()
        .filter(|candidate| {
            candidate.path == plan.task.path
                && candidate.block_id == plan.task.block_id
        })
        .count();
    if matching != 1 {
        return Err(CommandError::new(format!(
            "expected exactly one reading task after overwrite, found {matching}"
        )));
    }
    Ok(())
}

fn replace_subtree_in_bytes(
    original: &[u8],
    line_index: usize,
    old_subtree: &str,
    new_subtree: &str,
) -> Result<Vec<u8>, CommandError> {
    let contents = String::from_utf8(original.to_vec()).map_err(|error| {
        CommandError::new(format!("reading task is not UTF-8: {error}"))
    })?;
    let mut lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    let end = task_subtree_end(&lines, line_index);
    if normalized_subtree(&lines[line_index..end].concat())
        != normalized_subtree(old_subtree)
    {
        return Err(CommandError::new(
            "selected reading task preimage changed",
        ));
    }
    lines.splice(
        line_index..end,
        new_subtree.split_inclusive('\n').collect::<Vec<_>>(),
    );
    Ok(lines.concat().into_bytes())
}

fn find_task_line(
    lines: &[&str],
    task: &super::TaskFingerprint,
    expected: &str,
) -> Result<usize, CommandError> {
    if let Some(block_id) = task.block_id.as_deref()
        && let Some(index) = lines
            .iter()
            .position(|line| line.contains(&format!("^{block_id}")))
    {
        return Ok(index);
    }
    let first = expected.lines().next().unwrap_or_default();
    lines
        .iter()
        .position(|line| {
            normalize_task_line(line) == normalize_task_line(first)
        })
        .ok_or_else(|| {
            CommandError::new("could not locate the preserved reading task")
        })
}

fn task_subtree_end(lines: &[&str], start: usize) -> usize {
    let indent = lines[start]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .count();
    let mut end = start + 1;
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            end = index + 1;
            continue;
        }
        let child_indent = line
            .chars()
            .take_while(|character| matches!(character, ' ' | '\t'))
            .count();
        if child_indent <= indent {
            break;
        }
        end = index + 1;
    }
    end
}

fn verify_identity_and_links(
    config: &Config,
    plan: &OverwritePreflight,
    marker: &FinalMarkerMetadata,
) -> Result<(), CommandError> {
    let library = ref_library::build_index(&ref_library::LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    })
    .map_err(|error| {
        CommandError::new(format!(
            "could not verify reference identity: {error}"
        ))
    })?;
    let row = library
        .rows
        .iter()
        .find(|row| row.path == plan.note_path)
        .ok_or_else(|| {
            CommandError::new("replacement reference is missing from the index")
        })?;
    if row.id.as_deref() != Some(marker.id.as_str())
        || row.title != marker.title
        || marker
            .captured
            .as_deref()
            .is_some_and(|captured| row.captured.as_deref() != Some(captured))
        || marker.parent.as_deref().is_some_and(|parent| {
            row.parent.as_deref().is_none_or(|actual| {
                normalize_route(actual) != normalize_route(parent)
            })
        })
        || row
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "duplicate_identity")
    {
        return Err(CommandError::new(
            "replacement identity is inconsistent or duplicated",
        ));
    }
    let note_contents = fs::read_to_string(
        config.bob_dir.join(&plan.note_path),
    )
    .map_err(|error| {
        CommandError::new(format!("could not verify replacement note: {error}"))
    })?;
    let front_lines = frontmatter::split_frontmatter(&note_contents)
        .map(|(lines, _)| lines)
        .unwrap_or_default();
    let front = ref_library::ParsedFrontmatter::parse(&front_lines);
    for entry in ref_library::lineage::parse_earlier(&front) {
        let mut target = PathBuf::from(entry.target);
        if target.extension().is_none() {
            target.set_extension("md");
        }
        if !config.bob_dir.join(&target).is_file() {
            return Err(CommandError::new(format!(
                "replacement lineage link does not resolve: {}",
                target.display()
            )));
        }
    }
    let parsed_note = frontmatter::parse_note(&note_contents);
    let managed_embed = ref_tasks::find_managed_embed(&parsed_note.body);
    if plan.task.path != plan.note_path {
        let embed = managed_embed.ok_or_else(|| {
            CommandError::new(
                "replacement note is missing its managed reading-task embed",
            )
        })?;
        if normalize_route(&embed.target) != normalize_route(&plan.task.path)
            || plan.task.block_id.as_deref() != Some(embed.block_id.as_str())
        {
            return Err(CommandError::new(
                "replacement managed embed does not resolve to the preserved reading task",
            ));
        }
    }
    Ok(())
}

fn ensure_sync_writes_are_scoped(
    config: &Config,
    plan: &super::super::PdfSyncPlan,
    manifest: &BTreeSet<PathBuf>,
) -> Result<(), CommandError> {
    let writes = guard::scoped_write_path_set(config, plan).write_paths;
    let mut unexpected = Vec::new();
    for absolute in &writes {
        let Ok(relative) = absolute.strip_prefix(&config.bob_dir) else {
            unexpected.push(absolute.display().to_string());
            continue;
        };
        if !manifest.contains(&PathBuf::from(relative)) {
            unexpected.push(reference_mutation::vault_path(relative));
        }
    }
    if unexpected.is_empty() {
        Ok(())
    } else {
        Err(CommandError::new(format!(
            "single-PDF sync planned writes outside the overwrite manifest: {}",
            unexpected.join(", ")
        )))
    }
}

pub(in crate::native::highlights_ref) fn status_to_mark(
    status: &str,
) -> Result<char, CommandError> {
    match status {
        "ready" => Ok(' '),
        "next" => Ok('*'),
        "wip" | "in_progress" => Ok('/'),
        "blocked" => Ok('?'),
        "read" => Ok('x'),
        "abandoned" => Ok('-'),
        _ => Err(CommandError::new(format!(
            "unsupported overwrite task status: {status}"
        ))),
    }
}

fn marker_status_for_request(
    status: &str,
) -> Result<&'static str, CommandError> {
    match status {
        "ready" => Ok("ready"),
        "next" => Ok("next"),
        "wip" | "in_progress" => Ok("wip"),
        "read" => Ok("read"),
        "abandoned" => Ok("abandoned"),
        "blocked" => Ok("blocked"),
        _ => Err(CommandError::new(format!(
            "unsupported overwrite task status: {status}"
        ))),
    }
}

fn normalize_route(value: &str) -> String {
    let value = value
        .trim()
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
        .unwrap_or(value.trim());
    let target = value.split('|').next().unwrap_or(value);
    target
        .strip_suffix(".md")
        .or_else(|| target.strip_suffix(".MD"))
        .unwrap_or(target)
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

fn normalize_task_line(line: &str) -> String {
    line.trim_end_matches(['\r', '\n']).to_string()
}

fn normalized_subtree(value: &str) -> String {
    value.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_title_and_status_edits_preserve_other_task_content() {
        let before = "- [ ] #task #ref [[ref/papers/old|Old title (v2)]] [created::2026-01-01] ^ref-old\n  child with [priority:: high]\n";
        let after = rewrite_task_subtree(
            before,
            "ref/papers/old.md",
            "lib/papers/old.pdf",
            "A Better Title",
            'x',
            true,
            true,
        )
        .expect("rewrite task");
        assert!(after.starts_with(
            "- [x] #task #ref [[ref/papers/old|A Better Title (v2)]] [created::2026-01-01]"
        ));
        assert!(after.contains("[completion::"));
        assert!(after.ends_with("  child with [priority:: high]\n"));
    }

    #[test]
    fn unchanged_title_keeps_task_subtree_byte_identical() {
        let before = "- [ ] #task #ref [[ref/papers/old|My custom alias]] [created::2026-01-01] ^ref-old\n  child with [priority:: high]\n";
        let after = rewrite_task_subtree(
            before,
            "ref/papers/old.md",
            "lib/papers/old.pdf",
            "Old title",
            ' ',
            false,
            false,
        )
        .expect("rewrite task");
        assert_eq!(after, before);
    }

    #[test]
    fn changed_title_without_old_suffix_adds_none() {
        let before = "- [ ] #task #ref [[ref/papers/old|Old title]] [created::2026-01-01] ^ref-old\n";
        let after = rewrite_task_subtree(
            before,
            "ref/papers/old.md",
            "lib/papers/old.pdf",
            "A Better Title",
            ' ',
            false,
            true,
        )
        .expect("rewrite task");
        assert!(after.starts_with(
            "- [ ] #task #ref [[ref/papers/old|A Better Title]] [created::2026-01-01]"
        ));
    }

    #[test]
    fn note_local_frontmatter_keeps_created_lineage_and_custom_fields() {
        let parsed = frontmatter::parse_note(
            "---\ntitle: Old\ncreated: 2026-01-01\nearlier:\n  - [[ref/papers/old]]\nrecaptured: 2026-01-02\ncustom:\n  keep: yes\nhighlights_marker_hash: deadbeef\n---\nbody\n",
        );
        let entries = with_recaptured(
            retain_note_local_frontmatter(parsed.frontmatter),
            "2026-02-03",
        );
        let note = super::super::super::ParsedNote {
            frontmatter: entries,
            body: "new body\n".to_string(),
            original: None,
        }
        .render_original();
        assert!(note.contains("created: 2026-01-01"));
        assert!(note.contains("earlier:\n  - [[ref/papers/old]]"));
        assert!(note.contains("custom:\n  keep: yes"));
        assert!(note.contains("recaptured: 2026-02-03"));
        assert!(!note.contains("highlights_marker_hash"));
        assert!(!note.contains("title: Old"));
    }

    struct FailpointGuard;

    impl Drop for FailpointGuard {
        fn drop(&mut self) {
            FAILPOINT.with(|cell| cell.set(None));
        }
    }

    fn arm(point: OverwriteFailpoint) -> FailpointGuard {
        FAILPOINT.with(|cell| cell.set(Some(point)));
        FailpointGuard
    }

    const READY_MARKER: &str = concat!(
        "- status: ready\n",
        "- parent: sase\n",
        "- title: Report\n",
        "- id: report\n",
        "- captured: 2026-01-01\n",
    );
    const STAGED_MARKER: &str = concat!(
        "- status: ready\n",
        "- parent: sase\n",
        "- title: Report\n",
        "- id: report\n",
        "- captured: 2026-10-16\n",
    );

    fn git(vault: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write_marker_pdf(path: &Path, marker: &str) {
        use lopdf::{dictionary, Document, Object, Stream};
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("pdf parent");
        }
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let content_id =
            doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let annot_id = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(24),
                Object::Integer(24),
            ],
            "Contents" => marker::pdf_text_string(marker),
        });
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ],
            "Contents" => content_id,
            "Annots" => vec![Object::Reference(annot_id)],
        });
        doc.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            },
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.save(path).expect("write marker PDF");
    }

    struct ExecuteFixture {
        _temp: tempfile::TempDir,
        _guard: crate::native::env::TestEnvGuard,
        config: Config,
        plan: super::super::OverwritePreflight,
        request: super::super::OverwriteRequest,
        staged: StagedOverwrite,
        vault: PathBuf,
        note: PathBuf,
        pdf: PathBuf,
        old_note: Vec<u8>,
        old_pdf: Vec<u8>,
        old_head: String,
        old_status: String,
    }

    fn filed_execute_fixture(tag: &str) -> ExecuteFixture {
        use std::ffi::OsStr;
        let temp = tempfile::Builder::new()
            .prefix(tag)
            .tempdir()
            .expect("temp");
        let vault = temp.path().join("vault");
        let state = temp.path().join("state");
        let config_home = temp.path().join("config");
        let home = temp.path().join("home");
        let lock = temp.path().join("bob_sync.lock");
        fs::create_dir_all(vault.join("lib/papers")).expect("lib");
        fs::create_dir_all(vault.join("ref/papers")).expect("ref");
        fs::create_dir_all(vault.join("xlib")).expect("xlib");
        fs::create_dir_all(&state).expect("state");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&config_home).expect("config");
        fs::write(
            vault.join("sase.md"),
            "---\ntype: \"[[area]]\"\n---\n\n# SASE\n",
        )
        .expect("area");
        let pdf = vault.join("lib/papers/report.pdf");
        write_marker_pdf(&pdf, READY_MARKER);

        let guard = crate::native::env::TestEnvGuard::set(&[
            ("BOB_DIR", Some(vault.as_os_str())),
            ("HOME", Some(home.as_os_str())),
            ("XDG_STATE_HOME", Some(state.as_os_str())),
            ("XDG_CONFIG_HOME", Some(config_home.as_os_str())),
            ("BOB_VAULT_SYNC_LOCK_FILE", Some(lock.as_os_str())),
            ("BOB_NOW", Some(OsStr::new("2026-10-16 12:00:00"))),
        ]);

        let config = Config::for_vault(&vault);
        super::super::super::sync::sync_pdf(
            &config,
            &pdf,
            SyncOptions {
                dry_run: false,
                write_pdf: false,
                prefer: None,
                ignore_unannotated_sidecar: false,
            },
        )
        .unwrap_or_else(|error| panic!("file reference via sync: {error}"));

        git(&vault, &["init", "-q"]);
        git(&vault, &["config", "user.email", "test@example.com"]);
        git(&vault, &["config", "user.name", "Test"]);
        git(&vault, &["config", "commit.gpgsign", "false"]);
        git(&vault, &["add", "-A"]);
        git(&vault, &["commit", "-q", "-m", "file reference"]);

        let index = crate::native::ref_library::build_index(
            &crate::native::ref_library::LibraryConfig {
                bob_dir: config.bob_dir.clone(),
                ref_dir: config.ref_dir.clone(),
                xlib_dir: config.xlib_dir.clone(),
            },
        )
        .expect("index");
        let family = super::super::super::recapture::select_overwrite_for_stem(
            &config,
            &index.rows,
            "report",
            None,
            super::super::super::recapture::StemIdentity::Title("Report"),
        )
        .expect("select stem")
        .expect("filed family");
        let member = family.newest().clone();
        let request = super::super::OverwriteRequest::default();
        let plan =
            super::super::prepare(&config, &member, &request, "2026-10-16")
                .unwrap_or_else(|refusal| {
                    panic!(
                        "prepare refused {}: {}",
                        refusal.code, refusal.message
                    )
                });

        let staged_pdf = temp.path().join("staged.pdf");
        write_marker_pdf(&staged_pdf, STAGED_MARKER);
        let staged = StagedOverwrite {
            private_pdf: staged_pdf,
            audio: None,
            marker: FinalMarkerMetadata {
                id: "report".into(),
                title: "Report".into(),
                status: "ready".into(),
                parent: Some("sase".into()),
                captured: Some("2026-10-16".into()),
            },
        };
        let note = vault.join("ref/papers/report.md");
        ExecuteFixture {
            old_note: fs::read(&note).expect("note"),
            old_pdf: fs::read(&pdf).expect("pdf"),
            old_head: git(&vault, &["rev-parse", "HEAD"]),
            old_status: git(
                &vault,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            ),
            _temp: temp,
            _guard: guard,
            config,
            plan,
            request,
            staged,
            vault,
            note,
            pdf,
        }
    }

    fn assert_intact_old_state(fixture: &ExecuteFixture) {
        assert_eq!(
            fs::read(&fixture.note).expect("note"),
            fixture.old_note,
            "the live note must match the pre-overwrite bytes"
        );
        assert_eq!(
            fs::read(&fixture.pdf).expect("pdf"),
            fixture.old_pdf,
            "the library PDF must match the pre-overwrite bytes"
        );
        assert_eq!(
            git(&fixture.vault, &["rev-parse", "HEAD"]),
            fixture.old_head,
            "HEAD must stay at the pre-overwrite commit"
        );
        assert_eq!(
            git(
                &fixture.vault,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            ),
            fixture.old_status,
            "the worktree and index must match the pre-overwrite state"
        );
        assert!(
            super::super::recovery::load_intent(&fixture.config)
                .expect("load intent")
                .is_none(),
            "the overwrite intent must be cleared"
        );
    }

    fn assert_committed_new_state(
        fixture: &ExecuteFixture,
        result: &OverwriteResult,
    ) {
        assert_ne!(
            fs::read(&fixture.note).expect("note"),
            fixture.old_note,
            "the live note must change"
        );
        assert_ne!(
            fs::read(&fixture.pdf).expect("pdf"),
            fixture.old_pdf,
            "the library PDF must change"
        );
        let head = git(&fixture.vault, &["rev-parse", "HEAD"]);
        assert_ne!(head, fixture.old_head, "HEAD must advance");
        assert_eq!(head, result.commit_sha, "the result names the new HEAD");
        assert!(
            git(
                &fixture.vault,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )
            .is_empty(),
            "the committed overwrite leaves a clean worktree"
        );
        assert!(
            super::super::recovery::load_intent(&fixture.config)
                .expect("load intent")
                .is_none(),
            "the overwrite intent must be cleared"
        );
    }

    fn run_execute(
        fixture: &ExecuteFixture,
        mut vault_sync: impl FnMut(bool) -> Result<(), String>,
    ) -> Result<OverwriteResult, CommandError> {
        execute_with_sync(
            &fixture.config,
            &fixture.plan,
            &fixture.request,
            fixture.staged.clone(),
            &mut vault_sync,
        )
    }

    #[test]
    fn overwrite_commits_when_pre_sync_leaves_selection_unchanged() {
        let fixture = filed_execute_fixture("bob-cli-overwrite-fp-happy");
        let result = run_execute(&fixture, |_| Ok(()))
            .unwrap_or_else(|error| panic!("execute: {error}"));
        assert!(!result.push_pending, "a no-op post-sync is not pending");
        assert_committed_new_state(&fixture, &result);
    }

    #[test]
    fn failpoints_that_abort_before_commit_restore_the_old_state() {
        const POINTS: &[OverwriteFailpoint] = &[
            OverwriteFailpoint::AfterStagingValidation,
            OverwriteFailpoint::AfterIntentPersistence,
            OverwriteFailpoint::AfterFrozenNote,
            OverwriteFailpoint::AfterSidecarRemoval,
            OverwriteFailpoint::AfterAudio,
            OverwriteFailpoint::AfterPdf,
            OverwriteFailpoint::AfterNote,
            OverwriteFailpoint::AfterTaskEdit,
            OverwriteFailpoint::AfterSyncWrite,
            OverwriteFailpoint::ForcedVerificationFailure,
            OverwriteFailpoint::BeforeCommit,
        ];
        for point in POINTS {
            let fixture = filed_execute_fixture(&format!(
                "bob-cli-overwrite-fp-{point:?}"
            ));
            let _armed = arm(*point);
            let error = run_execute(&fixture, |_| Ok(()))
                .expect_err(&format!("{point:?} must abort"));
            assert!(
                error.to_string().contains("injected failpoint")
                    || error.code == Some("injected_failpoint"),
                "{point:?} unexpected error: {error}"
            );
            assert_intact_old_state(&fixture);
        }
    }

    #[test]
    fn crash_without_rollback_is_recovered_to_the_old_state() {
        let fixture = filed_execute_fixture("bob-cli-overwrite-fp-crash");
        let _armed = arm(OverwriteFailpoint::CrashWithoutRollback);
        let error =
            run_execute(&fixture, |_| Ok(())).expect_err("injected crash");
        assert_eq!(error.code, Some("injected_crash"));
        assert_ne!(
            fs::read(&fixture.note).expect("note"),
            fixture.old_note,
            "the crash leaves half-applied files"
        );
        assert!(
            super::super::recovery::load_intent(&fixture.config)
                .expect("load intent")
                .is_some(),
            "the crash keeps the intent"
        );
        drop(_armed);
        let notice = super::super::recovery::recover_with_locks_held(
            &fixture.config,
            &ob::child_env(),
            false,
        )
        .expect("recover crashed overwrite");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: rolled back an interrupted overwrite of report")
        );
        assert_intact_old_state(&fixture);
    }

    #[test]
    fn commit_reported_as_error_keeps_the_commit() {
        let fixture = filed_execute_fixture("bob-cli-overwrite-fp-commit-err");
        let _armed = arm(OverwriteFailpoint::CommitReportedAsError);
        let result =
            run_execute(&fixture, |_| Ok(())).unwrap_or_else(|error| {
                panic!("commit-reported-as-error must keep the commit: {error}")
            });
        assert!(!result.push_pending);
        assert_committed_new_state(&fixture, &result);
    }

    #[test]
    fn post_sync_failure_keeps_the_commit_and_sets_push_pending() {
        let fixture = filed_execute_fixture("bob-cli-overwrite-fp-post-sync");
        let _armed = arm(OverwriteFailpoint::PostSyncFailure);
        let result = run_execute(&fixture, |_| Ok(()))
            .unwrap_or_else(|error| panic!("post-sync: {error}"));
        assert!(
            result.push_pending,
            "a post-sync failure reports push-pending"
        );
        assert_committed_new_state(&fixture, &result);
    }

    #[test]
    fn pre_sync_byte_change_aborts_with_preimage_changed() {
        let fixture = filed_execute_fixture("bob-cli-overwrite-fp-preimage");
        let note = fixture.note.clone();
        let vault = fixture.vault.clone();
        let error = run_execute(&fixture, |pre| {
            if pre {
                let mut body = fs::read_to_string(&note).expect("note");
                body.push_str("\nmutated after pre-sync\n");
                fs::write(&note, body).expect("mutate");
                git(&vault, &["add", "-A"]);
                git(
                    &vault,
                    &["commit", "-q", "-m", "pre-sync pulled a newer note"],
                );
            }
            Ok(())
        })
        .expect_err("preimage must change");
        assert_eq!(error.code, Some("preimage_changed"), "{error}");
        assert!(
            fs::read_to_string(&fixture.note)
                .expect("note")
                .contains("mutated after pre-sync"),
            "overwrite must not roll back the pulled pre-sync commit"
        );
        let subject = git(&fixture.vault, &["log", "-1", "--format=%s"]);
        assert_eq!(subject, "pre-sync pulled a newer note");
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
    }

    #[test]
    fn pre_sync_later_reading_aborts_default_selection() {
        let fixture = filed_execute_fixture("bob-cli-overwrite-fp-later");
        let vault = fixture.vault.clone();
        let pdf = fixture.pdf.clone();
        let error = run_execute(&fixture, |pre| {
            if pre {
                fs::copy(&pdf, vault.join("lib/papers/report__v2.pdf"))
                    .expect("later pdf");
                fs::write(
                    vault.join("ref/papers/report__v2.md"),
                    "---\nid: report__v2\ntitle: Report\nearlier:\n  - \"[[ref/papers/report]]\"\nsource_pdf: lib/papers/report__v2.pdf\n---\n",
                )
                .expect("later note");
                git(&vault, &["add", "-A"]);
                git(
                    &vault,
                    &[
                        "commit",
                        "-q",
                        "-m",
                        "pre-sync pulled a later reading",
                    ],
                );
            }
            Ok(())
        })
        .expect_err("later reading must retarget default selection");
        assert_eq!(error.code, Some("preimage_changed"), "{error}");
        assert_eq!(
            fs::read(&fixture.note).expect("note"),
            fixture.old_note,
            "overwrite must not rewrite the now-older reading"
        );
        let subject = git(&fixture.vault, &["log", "-1", "--format=%s"]);
        assert_eq!(subject, "pre-sync pulled a later reading");
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
    }

    #[test]
    fn named_member_still_selected_after_pre_sync() {
        let mut fixture = filed_execute_fixture("bob-cli-overwrite-fp-named");
        fixture.request.name = Some("report".into());
        let result = run_execute(&fixture, |_| Ok(()))
            .unwrap_or_else(|error| panic!("named execute: {error}"));
        assert_committed_new_state(&fixture, &result);
    }

    #[test]
    fn named_member_missing_after_pre_sync_aborts() {
        let mut fixture =
            filed_execute_fixture("bob-cli-overwrite-fp-named-gone");
        fixture.request.name = Some("report".into());
        let note = fixture.note.clone();
        let error = run_execute(&fixture, |pre| {
            if pre {
                fs::remove_file(&note).expect("remove named member");
            }
            Ok(())
        })
        .expect_err("missing named member");
        assert_eq!(error.code, Some("preimage_changed"));
        fs::write(&fixture.note, &fixture.old_note).expect("restore note");
        assert_eq!(
            git(&fixture.vault, &["rev-parse", "HEAD"]),
            fixture.old_head
        );
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
    }
}
