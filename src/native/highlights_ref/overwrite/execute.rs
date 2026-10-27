//! Staged, task-preserving overwrite transaction.
//!
//! Route renderers hand this module private staged files. The executor owns
//! the locked recheck, publication, sync verification, scoped commit, and
//! rollback boundary; it never queues a half-published overwrite.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(test)]
use std::cell::Cell;

use sha2::{Digest, Sha256};

use super::super::reading_plan::ReadingTaskAction;
use super::super::{
    frontmatter, guard, marker, note, ob, reference_mutation, source_store,
    sync, CommandError, Config, MarkerValue, PdfSyncOutcome, Projection,
    SyncOptions, FIELD_ID, FIELD_PARENT, FIELD_SOURCE_MARKDOWN, FIELD_STATUS,
};
use super::{recovery, OverwritePreflight, OverwriteRequest};
use crate::native::file_coordination::{Coordination, MoveContext};
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
    FailedNewFileWrite,
    LateHistorySaveBeforeRollback,
    CrashAfterLateHistorySave,
    AfterIntentPersistence,
    AfterFrozenNote,
    AfterSidecarRemoval,
    AfterArchivePdf,
    AfterAssets,
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
    /// Markdown-rendered replacement: staged `source.md` + `code.json`.
    pub(crate) source_store: Option<source_store::StagedStoreFiles>,
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
    if initial.annotated.is_some()
        && let Some(refusal) =
            super::annotated_execution_refusal(initial.highlights)
                .map_err(|refusal| refusal.command_error())?
    {
        return Err(refusal.command_error());
    }
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
    let session =
        super::super::marker_write::MarkerWriteSession::for_explicit_write()?;
    execute_with_sync_and_session(
        config,
        initial,
        request,
        staged,
        &mut vault_sync,
        session,
    )
}

fn execute_with_sync_and_session(
    config: &Config,
    initial: &OverwritePreflight,
    request: &OverwriteRequest,
    staged: StagedOverwrite,
    mut vault_sync: impl FnMut(bool) -> Result<(), String>,
    session: super::super::marker_write::MarkerWriteSession,
) -> Result<OverwriteResult, CommandError> {
    if initial.annotated.is_some() && !session.coordinates() {
        return Err(CommandError::new(
            "annotated overwrite requires a coordinated macOS file write; no overwrite was written; close it in Highlights and retry",
        )
        .with_code("annotated_reference"));
    }
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
    ensure_staged_store_is_preflighted(request, &locked, &staged)?;

    let base_head = reference_mutation::git_head(&config.bob_dir, &child_env)
        .map_err(|error| {
        CommandError::new(format!("could not read vault HEAD: {error}"))
    })?;
    let subject = format!(
        "{OVERWRITE_SUBJECT_PREFIX} {}/{}",
        locked.ref_type, locked.member.stem
    );
    let manifest = transaction_manifest(&locked);
    let live_pdf = config.bob_dir.join(&locked.pdf_path);
    let transaction = if let Some(annotated) = &locked.annotated {
        let archive_pdf =
            config.bob_dir.join(&annotated.bundle.annotated_pdf_path);
        session.coordinate_explicit_pdf_move_with(
            &live_pdf,
            &archive_pdf,
            |scope| {
                overwrite_inside_accessor(
                    config,
                    &locked,
                    request,
                    &staged,
                    &child_env,
                    &base_head,
                    subject.clone(),
                    &manifest,
                    Some(scope),
                )
            },
        )
    } else {
        session.coordinate_explicit_pdf_write_with(&live_pdf, || {
            overwrite_inside_accessor(
                config,
                &locked,
                request,
                &staged,
                &child_env,
                &base_head,
                subject.clone(),
                &manifest,
                None,
            )
        })
    };

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
                "PDF coordination timed out after {:.2}s; no overwrite was written; close it in Highlights and retry",
                timeout.waited.as_secs_f64()
            ))
            .with_code("coordination_timeout"));
        }
        Ok(Coordination::Failed(detail)) => {
            return Err(CommandError::new(format!(
                "PDF coordination failed: {detail}; no overwrite was written; close it in Highlights and retry"
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

fn overwrite_inside_accessor(
    config: &Config,
    locked: &OverwritePreflight,
    request: &OverwriteRequest,
    staged: &StagedOverwrite,
    child_env: &ob::ChildEnv,
    base_head: &str,
    subject: String,
    manifest: &BTreeSet<PathBuf>,
    mut pdf_move: Option<&mut MoveContext<'_>>,
) -> Result<OverwriteResult, CommandError> {
    // Coordination asks an open Highlights document to save before
    // the first vault mutation. Re-plan inside the accessor so a
    // presenter save cannot be overwritten as if it were our write.
    if locked.annotated.is_some()
        && !matches!(preimages_match(config, locked), Ok(true))
    {
        return Err(coordinated_preimage_error(config, locked));
    }
    let current =
        super::prepare(config, &locked.member, request, &locked.planned_date)
            .map_err(|refusal| refusal.command_error())?;
    if !same_locked_preflight(locked, &current) {
        if locked.annotated.is_some() {
            return Err(coordinated_preimage_error(config, locked));
        }
        return Err(CommandError::new(
            "overwrite target changed during PDF coordination; no files were written",
        )
        .with_code("preimage_changed"));
    }

    let mut intent = recovery::OverwriteIntent::for_transaction(
        config,
        &locked.member.stem,
        &locked.ref_type,
        base_head,
        subject.clone(),
        &locked.bundle_preimages,
        &annotated_history_paths(locked),
    )?;
    if locked.annotated.is_some() {
        intent = intent.with_annotated_moves(
            annotated_moves(locked, staged)?,
            annotated_writes(locked, staged)?,
        );
    }
    if let Err(error) = recovery::save_intent(config, &intent) {
        let rollback = recovery::rollback_transaction(
            config,
            child_env,
            &intent,
            pdf_move.as_deref_mut(),
        );
        return Err(CommandError::new(format!(
            "could not persist overwrite intent: {error}{}",
            rollback_suffix(rollback)
        ))
        .with_human_code("verify_failed"));
    }
    #[cfg(test)]
    if let Err(error) = trip(OverwriteFailpoint::AfterIntentPersistence) {
        let rollback = recovery::rollback_transaction(
            config,
            child_env,
            &intent,
            pdf_move.as_deref_mut(),
        );
        return Err(CommandError::new(format!(
            "{error}{}",
            rollback_suffix(rollback)
        ))
        .with_human_code("verify_failed"));
    }

    let applied = apply_and_verify(
        config,
        locked,
        request,
        staged,
        manifest,
        pdf_move.as_deref_mut(),
    );
    #[cfg(test)]
    let applied = if applied.is_ok()
        && failpoint_is(OverwriteFailpoint::LateHistorySaveBeforeRollback)
    {
        mutate_older_archive_for_test(config, locked).and_then(|()| {
            Err(CommandError::new(
                "injected failpoint LateHistorySaveBeforeRollback",
            ))
        })
    } else {
        applied
    };
    #[cfg(test)]
    if applied.is_ok()
        && failpoint_is(OverwriteFailpoint::CrashAfterLateHistorySave)
    {
        mutate_older_archive_for_test(config, locked)?;
        return Err(CommandError::new(
            "injected crash after a late save to an older archive",
        )
        .with_code("injected_crash"));
    }
    #[cfg(test)]
    if failpoint_is(OverwriteFailpoint::CrashWithoutRollback) && applied.is_ok()
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
                child_env,
                &intent,
                pdf_move.as_deref_mut(),
            );
            return Err(CommandError::new(format!(
                "{error}{}",
                rollback_suffix(rollback)
            ))
            .with_human_code("verify_failed"));
        }
    };

    let paths =
        match manifest_commit_paths(&config.bob_dir, child_env, manifest) {
            Ok(paths) => paths,
            Err(error) => {
                let rollback = recovery::rollback_transaction(
                    config,
                    child_env,
                    &intent,
                    pdf_move.as_deref_mut(),
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
            child_env,
            &intent,
            pdf_move.as_deref_mut(),
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
            child_env,
            &intent,
            pdf_move.as_deref_mut(),
        );
        return Err(CommandError::new(format!(
            "{error}{}",
            rollback_suffix(rollback)
        ))
        .with_human_code("verify_failed"));
    }
    let commit_result = {
        let result =
            ob::commit_paths(&config.bob_dir, child_env, &subject, &paths);
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
                child_env,
                &intent,
                pdf_move.as_deref_mut(),
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
                    child_env,
                    Some(base_head),
                    &subject,
                )
                && committed_subject == subject
            {
                sha
            } else {
                let rollback = recovery::rollback_transaction(
                    config, child_env, &intent, pdf_move,
                );
                return Err(CommandError::new(format!(
                    "Git commit failed: {error}{}",
                    rollback_suffix(rollback)
                ))
                .with_human_code("verify_failed"));
            }
        }
    };

    if !prove_commit(&config.bob_dir, child_env, base_head, &sha, &subject) {
        return Err(CommandError::new(format!(
            "overwrite commit could not be proved; left its intent in place for recovery (HEAD is {sha})"
        ))
        .with_code("commit_unverified"));
    }
    // A crash before this update is still recoverable from the exact
    // HEAD commit subject; storing the SHA makes proof unambiguous.
    let mut push_pending = intent.record_expected_head(config, &sha).is_err();
    if recovery::clear_intent(config).is_err() {
        push_pending = true;
    }

    Ok(OverwriteResult {
        note: locked.note_path.clone(),
        pdf: locked.pdf_path.clone(),
        archive: archive_path(locked).map(str::to_string),
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
        && expected.annotated == current.annotated
        && expected.status == current.status
        && expected.audio_path == current.audio_path
}

fn transaction_manifest(plan: &OverwritePreflight) -> BTreeSet<PathBuf> {
    let mut paths = plan
        .bundle_preimages
        .iter()
        .map(|preimage| PathBuf::from(&preimage.path))
        .collect::<BTreeSet<_>>();
    if let Some(annotated) = &plan.annotated {
        paths.insert(PathBuf::from(&annotated.bundle.note_path));
        paths.insert(PathBuf::from(&annotated.bundle.annotated_pdf_path));
        paths.insert(PathBuf::from(&annotated.bundle.assets_path));
        paths.extend(annotated.asset_map.values().cloned());
    }
    paths
}

fn preimages_match(
    config: &Config,
    plan: &OverwritePreflight,
) -> Result<bool, CommandError> {
    let paths = plan
        .bundle_preimages
        .iter()
        .map(|preimage| PathBuf::from(&preimage.path))
        .collect::<BTreeSet<_>>();
    let current =
        super::collect_preimages(config, &paths).map_err(|error| {
            CommandError::new(format!(
                "could not recheck overwrite preimages: {error}"
            ))
        })?;
    Ok(current == plan.bundle_preimages)
}

fn coordinated_preimage_error(
    config: &Config,
    plan: &OverwritePreflight,
) -> CommandError {
    let marker_changed = plan.annotated.as_ref().is_some_and(|annotated| {
        super::super::marker::read_pdf_marker(
            &config.bob_dir.join(&plan.pdf_path),
        )
        .is_ok_and(|marker| {
            super::sha256(marker.contents.as_bytes())
                != annotated.live_marker_sha256
        })
    });
    if marker_changed {
        CommandError::new(
            "marker changed in Highlights during overwrite; no files were written; close it in Highlights and retry",
        )
        .with_code("marker_changed")
    } else {
        CommandError::new(
            "Highlights or another writer changed a planned PDF, sidecar, note, or link while the overwrite waited; no files were written. Run bob ref scan -w to commit the save, then retry the overwrite",
        )
        .with_code("preimage_changed")
    }
}

fn annotated_moves(
    plan: &OverwritePreflight,
    staged: &StagedOverwrite,
) -> Result<Vec<recovery::PendingMove>, CommandError> {
    let annotated = plan
        .annotated
        .as_ref()
        .expect("annotated move requested only for annotated plans");
    let pdf_hash = plan
        .bundle_preimages
        .iter()
        .find(|preimage| preimage.path == plan.pdf_path)
        .and_then(|preimage| preimage.sha256.as_deref())
        .ok_or_else(|| {
            CommandError::new("annotated PDF preimage is missing")
        })?;
    let replacement_hash = hash_file(&staged.private_pdf)?;
    let mut moves = vec![recovery::pdf_move_for_archive(
        &annotated.bundle,
        &plan.pdf_path,
        pdf_hash,
        Some(&replacement_hash),
    )];
    if let Some(live_assets) = &annotated.live_assets_path {
        moves.push(recovery::assets_move_for_archive(
            &annotated.bundle,
            live_assets,
            annotated.asset_manifest.clone(),
        ));
    }
    Ok(moves)
}

fn annotated_history_paths(plan: &OverwritePreflight) -> BTreeSet<String> {
    let Some(annotated) = &plan.annotated else {
        return BTreeSet::new();
    };
    let mut paths = BTreeSet::new();
    for record in &annotated.history {
        paths.insert(record.note_path.clone());
        if let Some(pdf) = &record.annotated_pdf {
            paths.insert(pdf.path.clone());
        }
        if let Some(assets) = &record.assets_path {
            paths.insert(assets.clone());
        }
    }
    paths
}

#[cfg(test)]
fn mutate_older_archive_for_test(
    config: &Config,
    plan: &OverwritePreflight,
) -> Result<(), CommandError> {
    let path = plan
        .annotated
        .as_ref()
        .and_then(|annotated| annotated.history.first())
        .and_then(|record| record.annotated_pdf.as_ref())
        .map(|pdf| pdf.path.as_str())
        .ok_or_else(|| {
            CommandError::new("test fixture has no older annotated archive")
        })?;
    let absolute = config.bob_dir.join(path);
    let mut bytes = fs::read(&absolute).map_err(|error| {
        CommandError::new(format!("read older archive for test: {error}"))
    })?;
    bytes.extend_from_slice(b"\nlate older archive save\n");
    fs::write(&absolute, bytes).map_err(|error| {
        CommandError::new(format!("write older archive for test: {error}"))
    })
}

fn annotated_writes(
    plan: &OverwritePreflight,
    staged: &StagedOverwrite,
) -> Result<Vec<recovery::PendingWrite>, CommandError> {
    let mut writes = Vec::new();
    if let Some(annotated) = &plan.annotated {
        writes.push(recovery::PendingWrite {
            path: annotated.frozen.path.clone(),
            sha256: super::sha256(annotated.frozen.contents.as_bytes()),
        });
    }
    if let Some(audio) = &staged.audio
        && plan
            .bundle_preimages
            .iter()
            .find(|preimage| preimage.path == audio.destination)
            .is_some_and(|preimage| preimage.sha256.is_none())
    {
        let hash = hash_file(&audio.private_path)?;
        writes.push(recovery::PendingWrite {
            path: audio.destination.clone(),
            sha256: hash,
        });
    }
    if let Some(store) = &staged.source_store {
        for (relative, scratch) in [
            (
                store.planned.source_rel.as_str(),
                store.scratch_source.as_path(),
            ),
            (
                store.planned.code_rel.as_str(),
                store.scratch_code.as_path(),
            ),
        ] {
            let created = plan.bundle_preimages.iter().any(|preimage| {
                preimage.path == relative && preimage.sha256.is_none()
            });
            if created {
                writes.push(recovery::PendingWrite {
                    path: relative.to_string(),
                    sha256: hash_file(scratch)?,
                });
            }
        }
    }
    Ok(writes)
}

fn hash_file(path: &Path) -> Result<String, CommandError> {
    let bytes = fs::read(path).map_err(|error| {
        CommandError::new(format!(
            "could not read {} for SHA-256: {error}",
            path.display()
        ))
    })?;
    Ok(super::sha256(&bytes))
}

fn archive_path(plan: &OverwritePreflight) -> Option<&str> {
    plan.annotated
        .as_ref()
        .map(|annotated| annotated.frozen.path.as_str())
        .or_else(|| {
            plan.frozen_note
                .as_ref()
                .map(|archive| archive.path.as_str())
        })
}

fn archive_note(plan: &OverwritePreflight) -> Option<&super::FrozenNote> {
    plan.annotated
        .as_ref()
        .map(|annotated| &annotated.frozen)
        .or(plan.frozen_note.as_ref())
}

/// The link plan has already been rendered from the exact preflight bytes.
/// Exclude the live note, which is replaced by the new capture skeleton and
/// whose self-links are rewritten in the frozen archive body.
fn planned_inbound_writes(
    plan: &OverwritePreflight,
) -> BTreeMap<String, String> {
    let Some(annotated) = &plan.annotated else {
        return BTreeMap::new();
    };
    annotated
        .inbound
        .files
        .iter()
        .filter(|file| {
            reference_mutation::vault_path(&file.path) != plan.note_path
        })
        .map(|file| {
            (
                reference_mutation::vault_path(&file.path),
                file.contents.clone(),
            )
        })
        .collect()
}

/// Apply the reading-task edit on top of any planned inbound link rewrites
/// to the same Markdown file. The file preimage was checked before intent
/// persistence, and link rewrites preserve line count, so its row remains
/// addressable by its original block ID or line index.
fn replace_task_subtree_after_links(
    contents: &str,
    plan: &OverwritePreflight,
    replacement: &str,
) -> Result<String, CommandError> {
    let mut lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    let line_index = task_line_index(&lines, plan)?;
    let end = task_subtree_end(&lines, line_index);
    lines.splice(
        line_index..end,
        replacement.split_inclusive('\n').collect::<Vec<_>>(),
    );
    Ok(lines.concat())
}

fn task_subtree_from_contents(
    contents: &str,
    plan: &OverwritePreflight,
) -> Result<String, CommandError> {
    let lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    let start = task_line_index(&lines, plan)?;
    let end = task_subtree_end(&lines, start);
    Ok(lines[start..end].concat())
}

fn task_line_index(
    lines: &[&str],
    plan: &OverwritePreflight,
) -> Result<usize, CommandError> {
    if let Some(block_id) = plan.task.block_id.as_deref() {
        if lines
            .get(plan.task.line_index)
            .is_some_and(|line| line_has_exact_block_id(line, block_id))
        {
            return Ok(plan.task.line_index);
        }
        return lines
            .iter()
            .position(|line| line_has_exact_block_id(line, block_id))
            .ok_or_else(|| {
                CommandError::new(
                    "could not locate the preserved reading task after inbound link planning",
                )
            });
    }
    (plan.task.line_index < lines.len())
        .then_some(plan.task.line_index)
        .ok_or_else(|| {
            CommandError::new(
                "could not locate the preserved reading task after inbound link planning",
            )
        })
}

fn line_has_exact_block_id(line: &str, expected: &str) -> bool {
    crate::native::collect_done::trailing_block_id_in_line(line).as_deref()
        == Some(expected)
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
            Ok(metadata) if metadata.is_file() => {
                if source_store::is_store_code_json(relative)
                    && source_store::code_catalog_ignored(
                        vault,
                        relative.parent().unwrap_or(relative),
                    )
                {
                    // Keep `code.json` on disk and omit it from the commit
                    // so `git add` cannot fail on an ignored catalog.
                    continue;
                }
                paths.push(relative.clone());
            }
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
    let store_files = staged.source_store.as_ref().map(|store| {
        [
            ("source.md", store.scratch_source.as_path()),
            ("code.json", store.scratch_code.as_path()),
        ]
    });
    for (label, path) in std::iter::once(("PDF", staged.private_pdf.as_path()))
        .chain(
            staged
                .audio
                .iter()
                .map(|audio| ("audio", audio.private_path.as_path())),
        )
        .chain(store_files.into_iter().flatten())
    {
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

fn ensure_staged_store_is_preflighted(
    request: &OverwriteRequest,
    plan: &OverwritePreflight,
    staged: &StagedOverwrite,
) -> Result<(), CommandError> {
    let Some(store) = &staged.source_store else {
        return Ok(());
    };
    let Some(planned) = &request.planned_source_store else {
        return Err(CommandError::new(
            "staged source store was not included in overwrite preflight",
        ));
    };
    if planned.source_rel != store.planned.source_rel
        || planned.code_rel != store.planned.code_rel
    {
        return Err(CommandError::new(
            "staged source store does not match the overwrite preflight",
        ));
    }
    let source_ok = plan
        .bundle_preimages
        .iter()
        .any(|preimage| preimage.path == store.planned.source_rel);
    let code_ok = plan
        .bundle_preimages
        .iter()
        .any(|preimage| preimage.path == store.planned.code_rel);
    if source_ok && code_ok {
        Ok(())
    } else {
        Err(CommandError::new(
            "staged source store destination was not included in overwrite preflight",
        ))
    }
}

fn apply_and_verify(
    config: &Config,
    plan: &OverwritePreflight,
    request: &OverwriteRequest,
    staged: &StagedOverwrite,
    manifest: &BTreeSet<PathBuf>,
    pdf_move: Option<&mut MoveContext<'_>>,
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
    let mut rewritten_files = planned_inbound_writes(plan);
    let task_subtree_before_gesture = if plan.task.path != plan.note_path {
        let task_contents = rewritten_files
            .get(&plan.task.path)
            .cloned()
            .unwrap_or_else(|| {
                String::from_utf8(original_task_file.clone())
                    .expect("preflighted task Markdown is UTF-8")
            });
        task_subtree_from_contents(&task_contents, plan)?
    } else {
        plan.task.subtree.clone()
    };
    let expected_subtree = rewrite_task_subtree(
        &task_subtree_before_gesture,
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
    if let Some(store) = &staged.source_store {
        all_paths.insert(PathBuf::from(&store.planned.source_rel));
        all_paths.insert(PathBuf::from(&store.planned.code_rel));
    }
    if let Some(annotated) = &plan.annotated {
        all_paths.insert(PathBuf::from(&annotated.bundle.note_path));
        all_paths.insert(PathBuf::from(&annotated.bundle.annotated_pdf_path));
        all_paths.insert(PathBuf::from(&annotated.bundle.assets_path));
        all_paths.extend(annotated.asset_map.values().cloned());
        all_paths.extend(
            annotated.inbound.files.iter().map(|file| file.path.clone()),
        );
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

    if let Some(archive) = archive_note(plan) {
        ensure_vault_parent_is_safe(config, Path::new(&archive.path))?;
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
    if plan.annotated.is_some() {
        let Some(scope) = pdf_move else {
            return Err(CommandError::new(
                "annotated overwrite is missing its coordinated PDF move scope",
            ));
        };
        move_annotated_bundle(config, plan, scope)?;
    }
    publish_audio(config, plan, staged)?;
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterAudio)?;
    if plan.annotated.is_some() {
        publish_pdf_after_archive(config, plan, &staged.private_pdf)?;
    } else {
        write_expected_file(config, plan, &plan.pdf_path, &staged.private_pdf)?;
    }
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterPdf)?;

    if let Some(store) = &staged.source_store {
        install_staged_source_store(config, plan, store)?;
    }

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
        archive_path(plan),
        &plan.planned_date,
        plan.highlights,
        staged
            .source_store
            .as_ref()
            .map(|store| store.planned.pointer.clone()),
        request.clear_source_markdown || staged.source_store.is_some(),
    )?;
    write_expected_bytes(
        config,
        plan,
        &plan.note_path,
        note_contents.as_bytes(),
    )?;
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterNote)?;

    let mut expected_task_file = None;
    if plan.task.path != plan.note_path {
        let task_contents =
            rewritten_files.remove(&plan.task.path).unwrap_or_else(|| {
                String::from_utf8(original_task_file.clone())
                    .expect("preflighted task Markdown is UTF-8")
            });
        let updated = replace_task_subtree_after_links(
            &task_contents,
            plan,
            &expected_subtree,
        )?;
        expected_task_file = Some(updated.as_bytes().to_vec());
        rewritten_files.insert(plan.task.path.clone(), updated);
    }
    for (relative, contents) in &rewritten_files {
        write_expected_bytes(config, plan, relative, contents.as_bytes())?;
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

    verify_identity_and_links(config, plan, &staged.marker, &rewritten_files)?;
    verify_task_subtree(
        config,
        plan,
        &expected_subtree,
        expected_mark,
        &original_task_file,
        expected_task_file.as_deref(),
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

fn move_annotated_bundle(
    config: &Config,
    plan: &OverwritePreflight,
    pdf_move: &mut MoveContext<'_>,
) -> Result<(), CommandError> {
    let annotated = plan
        .annotated
        .as_ref()
        .expect("annotated bundle move requires an annotated plan");
    let pdf_preimage = plan
        .bundle_preimages
        .iter()
        .find(|preimage| preimage.path == plan.pdf_path)
        .ok_or_else(|| {
            CommandError::new("annotated PDF was not preflighted")
        })?;
    let pdf_hash = pdf_preimage
        .sha256
        .as_deref()
        .ok_or_else(|| CommandError::new("annotated PDF preimage is absent"))?;
    verify_expected_preimage(config, plan, &plan.pdf_path)?;
    let archive_pdf = &annotated.bundle.annotated_pdf_path;
    require_preflighted_absent(config, plan, archive_pdf)?;
    let source = config.bob_dir.join(&plan.pdf_path);
    let destination = config.bob_dir.join(archive_pdf);
    ensure_vault_parent_is_safe(config, Path::new(archive_pdf))?;
    let parent = destination.parent().unwrap_or(&config.bob_dir);
    ensure_same_filesystem(&source, parent)?;
    let before = fs::symlink_metadata(&source).map_err(|error| {
        CommandError::new(format!("could not inspect annotated PDF: {error}"))
    })?;
    pdf_move.move_forward().map_err(|error| {
        CommandError::new(format!(
            "could not rename annotated PDF into its archive without replacement: {error}"
        ))
    })?;
    if hash_file(&destination)? != pdf_hash {
        return Err(CommandError::new(
            "annotated PDF changed while it was being archived",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let after = fs::symlink_metadata(&destination).map_err(|error| {
            CommandError::new(format!("could not verify archived PDF: {error}"))
        })?;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(CommandError::new(
                "annotated PDF archive did not preserve the original file object",
            ));
        }
    }

    #[cfg(test)]
    trip(OverwriteFailpoint::AfterArchivePdf)?;

    if let Some(live_assets) = &annotated.live_assets_path {
        let live_manifest =
            recovery::asset_manifest(&config.bob_dir, live_assets)
                .map_err(CommandError::new)?;
        if live_manifest != annotated.asset_manifest {
            return Err(CommandError::new(
                "annotated assets changed before their archive move",
            ));
        }
        let archive_assets = &annotated.bundle.assets_path;
        require_preflighted_absent(config, plan, archive_assets)?;
        let source = config.bob_dir.join(live_assets);
        let destination = config.bob_dir.join(archive_assets);
        ensure_vault_parent_is_safe(config, Path::new(archive_assets))?;
        let parent = destination.parent().unwrap_or(&config.bob_dir);
        ensure_same_filesystem(&source, parent)?;
        super::super::io::rename_no_replace(&source, &destination).map_err(
            |error| {
                CommandError::new(format!(
                    "could not rename annotated assets into their archive without replacement: {error}"
                ))
            },
        )?;
        let archived_manifest =
            recovery::asset_manifest(&config.bob_dir, archive_assets)
                .map_err(CommandError::new)?;
        if archived_manifest != annotated.asset_manifest {
            return Err(CommandError::new(
                "archived assets do not match the preflighted bundle",
            ));
        }
    }
    #[cfg(test)]
    trip(OverwriteFailpoint::AfterAssets)?;
    Ok(())
}

fn require_preflighted_absent(
    config: &Config,
    plan: &OverwritePreflight,
    relative: &str,
) -> Result<(), CommandError> {
    let expected_absent = plan
        .bundle_preimages
        .iter()
        .any(|preimage| preimage.path == relative && preimage.sha256.is_none());
    if !expected_absent {
        return Err(CommandError::new(format!(
            "archive destination was not preflighted as absent: {relative}"
        )));
    }
    match fs::symlink_metadata(config.bob_dir.join(relative)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CommandError::new(format!(
            "could not inspect archive destination {relative}: {error}"
        ))),
        Ok(_) => Err(CommandError::new(format!(
            "archive destination appeared after preflight: {relative}"
        ))),
    }
}

fn ensure_vault_parent_is_safe(
    config: &Config,
    relative: &Path,
) -> Result<(), CommandError> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            !matches!(component, std::path::Component::Normal(_))
        })
    {
        return Err(CommandError::new("archive path is not vault-relative"));
    }
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = config.bob_dir.clone();
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(CommandError::new("archive parent path is unsafe"));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CommandError::new(format!(
                    "archive parent contains a symlink: {}",
                    current.display()
                )));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(CommandError::new(format!(
                    "archive parent is not a directory: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(CommandError::new(format!(
                    "could not inspect archive parent {}: {error}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

fn ensure_same_filesystem(
    source: &Path,
    destination_parent: &Path,
) -> Result<(), CommandError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let source_metadata =
            fs::symlink_metadata(source).map_err(|error| {
                CommandError::new(format!(
                    "could not inspect move source {}: {error}",
                    source.display()
                ))
            })?;
        let parent_metadata =
            fs::metadata(destination_parent).map_err(|error| {
                CommandError::new(format!(
                    "could not inspect archive filesystem {}: {error}",
                    destination_parent.display()
                ))
            })?;
        if source_metadata.dev() != parent_metadata.dev() {
            return Err(CommandError::new(format!(
                "cannot archive {} without copying because the archive is on another filesystem",
                source.display()
            )));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination_parent);
        Err(CommandError::new(
            "cannot guarantee a same-filesystem annotated archive move on this platform",
        ))
    }
}

fn publish_pdf_after_archive(
    config: &Config,
    plan: &OverwritePreflight,
    staged: &Path,
) -> Result<(), CommandError> {
    let annotated = plan
        .annotated
        .as_ref()
        .expect("replacement publish requires an annotated plan");
    let destination = config.bob_dir.join(&plan.pdf_path);
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CommandError::new(format!(
                "could not inspect live PDF destination: {error}"
            )));
        }
        Ok(_) => {
            return Err(CommandError::new(
                "live PDF destination was recreated before replacement publication",
            ));
        }
    }
    let pdf_archive = config.bob_dir.join(&annotated.bundle.annotated_pdf_path);
    let expected_old = plan
        .bundle_preimages
        .iter()
        .find(|preimage| preimage.path == plan.pdf_path)
        .and_then(|preimage| preimage.sha256.as_deref())
        .ok_or_else(|| {
            CommandError::new("annotated PDF preimage is missing")
        })?;
    if hash_file(&pdf_archive)? != expected_old {
        return Err(CommandError::new(
            "annotated PDF archive changed before replacement publication",
        ));
    }
    let bytes = fs::read(staged).map_err(|error| {
        CommandError::new(format!("could not read staged PDF: {error}"))
    })?;
    atomic_publish_new_file(&destination, &bytes, "replacement PDF")
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

fn install_staged_source_store(
    config: &Config,
    plan: &OverwritePreflight,
    store: &source_store::StagedStoreFiles,
) -> Result<(), CommandError> {
    let source_bytes = fs::read(&store.scratch_source).map_err(|error| {
        CommandError::new(format!("could not read staged source.md: {error}"))
    })?;
    let dest_source = config.bob_dir.join(&store.planned.source_rel);
    let existing_source = dest_source
        .is_file()
        .then(|| fs::read(&dest_source).ok())
        .flatten();
    let reuse = existing_source.as_deref() == Some(source_bytes.as_slice());
    if existing_source.is_some() && !reuse {
        eprintln!(
            "warning: source store {} already holds different bytes; refusing to overwrite",
            store.planned.dir_rel.display()
        );
        return Ok(());
    }
    if !reuse {
        write_expected_bytes(
            config,
            plan,
            &store.planned.source_rel,
            &source_bytes,
        )?;
    }
    let code_bytes = fs::read(&store.scratch_code).map_err(|error| {
        CommandError::new(format!("could not read staged code.json: {error}"))
    })?;
    let dest_code = config.bob_dir.join(&store.planned.code_rel);
    let code_unchanged = dest_code.is_file()
        && fs::read(&dest_code).ok().as_deref() == Some(code_bytes.as_slice());
    if !code_unchanged {
        write_expected_bytes(
            config,
            plan,
            &store.planned.code_rel,
            &code_bytes,
        )?;
    }
    if source_store::code_catalog_ignored(
        config.bob_dir.as_path(),
        &store.planned.dir_rel,
    ) {
        eprintln!("{}", source_store::ignored_catalog_warning());
    }
    Ok(())
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
    atomic_publish_new_file(path, bytes, "frozen note")
}

static NEXT_OVERWRITE_TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn atomic_publish_new_file(
    destination: &Path,
    bytes: &[u8],
    description: &str,
) -> Result<(), CommandError> {
    let parent = destination.parent().ok_or_else(|| {
        CommandError::new(format!(
            "{description} destination has no parent: {}",
            destination.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CommandError::new(format!(
            "could not create {description} directory {}: {error}",
            parent.display()
        ))
    })?;
    let filename = destination.file_name().ok_or_else(|| {
        CommandError::new(format!(
            "{description} destination has no filename: {}",
            destination.display()
        ))
    })?;
    let (temp_path, mut file) = loop {
        let id = NEXT_OVERWRITE_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let mut name = std::ffi::OsString::from(".bob-overwrite-tmp-");
        name.push(filename);
        name.push(format!("-{}-{id}.tmp", std::process::id()));
        let temp_path = parent.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => break (temp_path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => {
                return Err(CommandError::new(format!(
                    "could not create temporary {description} without replacement: {error}"
                )));
            }
        }
    };

    let write_result = (|| {
        #[cfg(test)]
        if failpoint_is(OverwriteFailpoint::FailedNewFileWrite) {
            let partial = bytes.len().max(1) / 2;
            file.write_all(&bytes[..partial.min(bytes.len())])?;
            return Err(std::io::Error::other(
                "injected partial write failure",
            ));
        }
        file.write_all(bytes)?;
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(CommandError::new(format!(
            "could not write complete {description} {}: {error}",
            destination.display()
        )));
    }

    if let Err(error) =
        super::super::io::rename_no_replace(&temp_path, destination)
    {
        let _ = fs::remove_file(&temp_path);
        return Err(CommandError::new(format!(
            "could not publish {description} without replacing a file at {}: {error}",
            destination.display()
        )));
    }
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CommandError::new(format!(
                "could not sync {description} directory {}: {error}",
                parent.display()
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
    highlights: u64,
    source_markdown: Option<String>,
    replace_source_markdown: bool,
) -> Result<String, CommandError> {
    let lined = match archive {
        Some(path) => {
            super::prepend_archive_lineage(existing, path, date, highlights)
                .map_err(CommandError::new)?
        }
        None => existing.to_string(),
    };
    let parsed = frontmatter::parse_note(&lined);
    let retained = retain_note_local_frontmatter(
        parsed.frontmatter,
        replace_source_markdown,
    );
    let mut entries = with_recaptured(retained, date);
    if let Some(pointer) = source_markdown {
        entries.push(source_markdown_entry(&pointer));
    }
    let note = super::super::ParsedNote {
        frontmatter: entries,
        body: body.to_string(),
        original: None,
    };
    Ok(note.render_original())
}

fn source_markdown_entry(pointer: &str) -> super::super::FrontmatterEntry {
    super::super::FrontmatterEntry {
        key: Some(FIELD_SOURCE_MARKDOWN.to_string()),
        value: Some(MarkerValue::String(pointer.to_string())),
        raw: format!(
            "{FIELD_SOURCE_MARKDOWN}: {}",
            MarkerValue::String(pointer.to_string()).as_frontmatter_value()
        ),
    }
}

fn retain_note_local_frontmatter(
    entries: Vec<super::super::FrontmatterEntry>,
    drop_source_markdown: bool,
) -> Vec<super::super::FrontmatterEntry> {
    let mut retained = Vec::new();
    let mut skipping_continuations = false;
    for entry in entries {
        let keep = match entry.key.as_deref() {
            Some("created" | "earlier" | "recaptured") => true,
            Some(FIELD_SOURCE_MARKDOWN) if drop_source_markdown => false,
            Some(key) => {
                !super::super::is_managed_frontmatter_field(key)
                    && !super::super::is_standard_user_field(key)
            }
            None => !skipping_continuations,
        };
        match entry.key.as_deref() {
            Some(key) => skipping_continuations = !keep && !key.is_empty(),
            None if keep => skipping_continuations = false,
            None => {}
        }
        if keep
            && entry.key.as_deref() != Some("recaptured")
            && !(drop_source_markdown
                && entry.key.as_deref() == Some(FIELD_SOURCE_MARKDOWN))
        {
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

fn verify_task_subtree(
    config: &Config,
    plan: &OverwritePreflight,
    expected: &str,
    expected_mark: char,
    original_file: &[u8],
    expected_task_file: Option<&[u8]>,
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
        let expected_file = match expected_task_file {
            Some(bytes) => bytes.to_vec(),
            None => replace_subtree_in_bytes(
                original_file,
                plan.task.line_index,
                &plan.task.subtree,
                expected,
            )?,
        };
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
    if let Some(block_id) = task.block_id.as_deref() {
        if lines
            .get(task.line_index)
            .is_some_and(|line| line_has_exact_block_id(line, block_id))
        {
            return Ok(task.line_index);
        }
        return lines
            .iter()
            .position(|line| line_has_exact_block_id(line, block_id))
            .ok_or_else(|| {
                CommandError::new("could not locate the preserved reading task")
            });
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
    rewritten_files: &BTreeMap<String, String>,
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
    for (relative, expected) in rewritten_files {
        let actual = fs::read_to_string(config.bob_dir.join(relative))
            .map_err(|error| {
                CommandError::new(format!(
                    "could not verify inbound highlight links in {relative}: {error}"
                ))
            })?;
        if actual != *expected {
            return Err(CommandError::new(format!(
                "inbound highlight-link rewrite did not persist exactly in {relative}"
            )));
        }
    }
    if let Some(annotated) = &plan.annotated {
        let archive_contents = fs::read_to_string(
            config.bob_dir.join(&annotated.bundle.note_path),
        )
        .map_err(|error| {
            CommandError::new(format!(
                "could not read frozen annotated note: {error}"
            ))
        })?;
        if archive_contents != annotated.frozen.contents {
            return Err(CommandError::new(
                "frozen annotated note differs from its prepared archive contents",
            ));
        }
        let archive_pdf_path =
            config.bob_dir.join(&annotated.bundle.annotated_pdf_path);
        let archive_hash = hash_file(&archive_pdf_path)?;
        let expected_hash = plan
            .bundle_preimages
            .iter()
            .find(|preimage| preimage.path == plan.pdf_path)
            .and_then(|preimage| preimage.sha256.as_deref());
        if expected_hash != Some(archive_hash.as_str()) {
            return Err(CommandError::new(
                "archived annotated PDF hash does not match its preimage",
            ));
        }
        let records = super::archive::records_for_live_note(
            &config.bob_dir,
            &plan.note_path,
        )
        .map_err(|error| CommandError::new(error.message))?;
        let valid_record = records.iter().any(|record| {
            record.note_path == annotated.bundle.note_path
                && record.parent
                    == format!("[[{}]]", plan.note_path.trim_end_matches(".md"))
                && record.annotated_pdf.as_ref().is_some_and(|pdf| {
                    pdf.path == annotated.bundle.annotated_pdf_path
                        && pdf.sha256 == archive_hash
                })
        });
        if !valid_record {
            return Err(CommandError::new(
                "annotated archive metadata or lineage does not match the retained PDF",
            ));
        }
        for finding in &annotated.inbound.highlight_fragments {
            let Some((_, fragment)) = finding.target.rsplit_once("#^") else {
                continue;
            };
            if fragment.starts_with("h-")
                && !archive_contents.contains(&format!("^{fragment}"))
            {
                return Err(CommandError::new(format!(
                    "rewritten highlight anchor does not resolve in {}: ^{fragment}",
                    annotated.bundle.note_path
                )));
            }
        }
        for (source, dest) in &annotated.asset_map {
            let from = config.bob_dir.join(source);
            let to = config.bob_dir.join(dest);
            if fs::symlink_metadata(&from).is_ok() || !to.is_file() {
                return Err(CommandError::new(format!(
                    "archived asset move is incomplete: {} -> {}",
                    source.display(),
                    dest.display()
                )));
            }
        }
        if let Some(live_assets) = &annotated.live_assets_path {
            let archived_assets = recovery::asset_manifest(
                &config.bob_dir,
                &annotated.bundle.assets_path,
            )
            .map_err(CommandError::new)?;
            if archived_assets != annotated.asset_manifest
                || fs::symlink_metadata(config.bob_dir.join(live_assets))
                    .is_ok()
            {
                return Err(CommandError::new(
                    "archived assets do not match the preflighted bundle",
                ));
            }
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
            retain_note_local_frontmatter(parsed.frontmatter, false),
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

    #[test]
    fn note_local_frontmatter_drops_source_markdown_when_replacing() {
        let parsed = frontmatter::parse_note(concat!(
            "---\n",
            "created: 2026-01-01\n",
            "source_markdown: .bob/ref-sources/papers/old--abcabcabcabc/source.md\n",
            "custom: keep\n",
            "---\n",
            "body\n",
        ));
        let kept =
            retain_note_local_frontmatter(parsed.frontmatter.clone(), false);
        assert!(kept.iter().any(|entry| {
            entry.key.as_deref() == Some(FIELD_SOURCE_MARKDOWN)
        }));
        let dropped = retain_note_local_frontmatter(parsed.frontmatter, true);
        assert!(dropped.iter().all(|entry| {
            entry.key.as_deref() != Some(FIELD_SOURCE_MARKDOWN)
        }));
        assert!(dropped
            .iter()
            .any(|entry| entry.key.as_deref() == Some("custom")));
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
        "- status: next\n",
        "- parent: sase\n",
        "- title: A Better Report\n",
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
        old_sidecar: Vec<u8>,
        old_audio: Vec<u8>,
        old_task_file: Vec<u8>,
        new_audio_bytes: Vec<u8>,
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
        // The old companion exists before filing so the filed note embeds
        // it and the bundle stays in sync.
        fs::write(vault.join("lib/papers/report.mp3"), "old audio")
            .expect("old audio");

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

        // Extend the filed reference with real mutations: authored prose for
        // the frozen note, an unannotated tracked `.textbundle` sidecar, and
        // a preserved child under the reading task. All stable for preflight
        // without re-sync.
        let note_path = vault.join("ref/papers/report.md");
        let mut note_body = fs::read_to_string(&note_path).expect("filed note");
        note_body.push_str("\nAuthored paragraph that must freeze.\n");
        fs::write(&note_path, &note_body).expect("author prose");
        let bundle_dir = vault.join("lib/papers/report.textbundle");
        fs::create_dir_all(&bundle_dir).expect("textbundle");
        fs::write(bundle_dir.join("text.md"), "Report\n======\n\n## Page 1\n")
            .expect("sidecar text");
        fs::write(bundle_dir.join("info.json"), "{}").expect("info");
        let parent_path = vault.join("sase.md");
        let parent_body = fs::read_to_string(&parent_path).expect("parent");
        let mut parent_lines: Vec<String> =
            parent_body.lines().map(str::to_string).collect();
        let task_index = parent_lines
            .iter()
            .position(|line| line.contains("^ref-report"))
            .expect("reading task");
        parent_lines
            .insert(task_index + 1, "  - [ ] preserved child".to_string());
        fs::write(&parent_path, parent_lines.join("\n") + "\n")
            .expect("task child");
        git(&vault, &["add", "-A"]);
        git(&vault, &["commit", "-q", "-m", "extend fixture bundle"]);

        let index = crate::native::ref_library::build_index(
            &crate::native::ref_library::LibraryConfig {
                bob_dir: config.bob_dir.clone(),
                ref_dir: config.ref_dir.clone(),
                xlib_dir: config.xlib_dir.clone(),
            },
        )
        .expect("index");
        let outcome = super::super::super::recapture::preflight_for_stem(
            &config,
            &index.rows,
            "report",
            None,
            None,
            Some(super::super::super::force::ForceMode::Overwrite),
            "Report",
            super::super::super::recapture::StemIdentity::Title("Report"),
        )
        .expect("select stem");
        let super::super::super::recapture::CaptureOutcome::Overwrite(
            selection,
        ) = outcome
        else {
            panic!("expected overwrite selection, got {outcome:?}");
        };
        let member = selection.family.newest().clone();
        let mut request = super::super::OverwriteRequest::default();
        request.explicit_title = Some("A Better Report".to_string());
        request.explicit_status = Some("next".to_string());
        request.replacement_audio_path =
            Some("lib/papers/report.m4a".to_string());
        let plan =
            super::super::prepare(&config, &member, &request, "2026-10-16")
                .unwrap_or_else(|refusal| {
                    panic!(
                        "prepare refused {}: {}",
                        refusal.code, refusal.message
                    )
                });
        assert!(
            plan.frozen_note.is_some(),
            "fixture prose must plan a frozen note"
        );
        assert!(
            plan.audio_path.as_deref() == Some("lib/papers/report.mp3"),
            "fixture must discover the old companion: {:?}",
            plan.audio_path
        );

        let staged_pdf = temp.path().join("staged.pdf");
        write_marker_pdf(&staged_pdf, STAGED_MARKER);
        let private_audio = temp.path().join("new.m4a");
        fs::write(&private_audio, "new audio").expect("staged audio");
        let staged = StagedOverwrite {
            private_pdf: staged_pdf,
            audio: Some(super::super::super::overwrite::execute::StagedAudio {
                private_path: private_audio,
                destination: "lib/papers/report.m4a".to_string(),
            }),
            marker: FinalMarkerMetadata {
                id: "report".into(),
                title: "A Better Report".into(),
                status: "next".into(),
                parent: Some("sase".into()),
                captured: Some("2026-10-16".into()),
            },
            source_store: None,
        };
        let note = vault.join("ref/papers/report.md");
        let sidecar_text = vault.join("lib/papers/report.textbundle/text.md");
        let old_audio = vault.join("lib/papers/report.mp3");
        let task_file = vault.join(plan.task.path.as_str());
        ExecuteFixture {
            old_note: fs::read(&note).expect("note"),
            old_pdf: fs::read(&pdf).expect("pdf"),
            old_head: git(&vault, &["rev-parse", "HEAD"]),
            old_status: git(
                &vault,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            ),
            old_sidecar: fs::read(&sidecar_text).expect("sidecar"),
            old_audio: fs::read(&old_audio).expect("audio"),
            old_task_file: fs::read(&task_file).expect("task file"),
            new_audio_bytes: b"new audio".to_vec(),
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
        // Abort restores the prose carrier, sidecar text, companion, and
        // task file with its preserved child.
        assert_eq!(
            fs::read(
                fixture.vault.join("lib/papers/report.textbundle/text.md")
            )
            .expect("sidecar"),
            fixture.old_sidecar,
            "the stale sidecar must survive an abort"
        );
        assert_eq!(
            fs::read(fixture.vault.join("lib/papers/report.mp3"))
                .expect("audio"),
            fixture.old_audio,
            "the old companion must survive an abort"
        );
        assert!(
            !fixture.vault.join("lib/papers/report.m4a").exists(),
            "the replacement audio must not exist after an abort"
        );
        let task_path = fixture.vault.join(&fixture.plan.task.path);
        assert_eq!(
            fs::read(&task_path).expect("task file"),
            fixture.old_task_file,
            "the reading task file must match the pre-overwrite bytes"
        );
        assert!(
            fs::read_to_string(&task_path)
                .expect("task file")
                .contains("preserved child"),
            "the preserved child must survive an abort"
        );
        if let Some(archive) = &fixture.plan.frozen_note {
            assert!(
                !fixture.vault.join(&archive.path).exists(),
                "no frozen note may exist after an abort"
            );
        }
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
        // The stronger fixture proves each mutation: prose archives, the
        // stale sidecar is gone, the audio extension swaps, and the task
        // title/status edit lands.
        let archive_path = fixture
            .plan
            .frozen_note
            .as_ref()
            .expect("fixture plans a frozen note")
            .path
            .clone();
        let archive_body =
            fs::read_to_string(fixture.vault.join(&archive_path))
                .expect("frozen note");
        assert!(
            archive_body.contains("Authored paragraph that must freeze."),
            "the archive keeps the authored prose"
        );
        assert!(
            !fixture.vault.join("lib/papers/report.textbundle").exists(),
            "the stale sidecar is removed"
        );
        assert!(
            !fixture.vault.join("lib/papers/report.mp3").exists(),
            "the old companion is removed on extension swap"
        );
        assert_eq!(
            fs::read(fixture.vault.join("lib/papers/report.m4a"))
                .expect("new audio"),
            fixture.new_audio_bytes,
            "the replacement audio bytes land at the new extension"
        );
        let task_path = fixture.vault.join(&fixture.plan.task.path);
        let task_body = fs::read_to_string(&task_path).expect("task file");
        assert!(
            task_body.contains("preserved child"),
            "the task edit preserves children:\n{task_body}"
        );
        assert!(
            task_body.contains("A Better Report"),
            "the title change lands in the task:\n{task_body}"
        );
        assert_ne!(
            task_body.as_bytes(),
            &fixture.old_task_file,
            "the task file must change"
        );
        assert!(
            fs::read_to_string(&fixture.note)
                .expect("note")
                .contains("A Better Report"),
            "the live note carries the new title"
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

    fn refresh_fixture_plan(fixture: &mut ExecuteFixture) {
        fixture.request.dry_run = true;
        fixture.plan = super::super::prepare(
            &fixture.config,
            &fixture.plan.member,
            &fixture.request,
            &fixture.plan.planned_date,
        )
        .unwrap_or_else(|refusal| {
            panic!("prepare fixture overwrite: {}", refusal.message)
        });
        fixture.request.dry_run = false;
        fixture.old_note = fs::read(&fixture.note).expect("refresh old note");
        fixture.old_pdf = fs::read(&fixture.pdf).expect("refresh old PDF");
        fixture.old_task_file =
            fs::read(fixture.vault.join(&fixture.plan.task.path))
                .expect("refresh old task file");
        fixture.old_head = git(&fixture.vault, &["rev-parse", "HEAD"]);
        fixture.old_status = git(
            &fixture.vault,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        );
    }

    fn seed_live_store(fixture: &mut ExecuteFixture, bytes: &[u8]) -> String {
        let planned = source_store::PlannedSourceStore::for_bytes(
            &fixture.plan.ref_type,
            &fixture.plan.member.stem,
            bytes,
        );
        let dir = fixture.vault.join(&planned.dir_rel);
        fs::create_dir_all(&dir).expect("store dir");
        fs::write(dir.join("source.md"), bytes).expect("source.md");
        fs::write(dir.join("code.json"), "{}\n").expect("code.json");
        let mut note = fs::read_to_string(&fixture.note).expect("note");
        if !note.contains("source_markdown:") {
            // Insert with the other preserved note-local fields, just
            // before the pipeline block, so a dry-run sync still sees
            // the note as in-sync.
            let line = format!("source_markdown: {}\n", planned.pointer);
            note = if let Some(idx) = note.find("source_pdf:") {
                format!("{}{}{}", &note[..idx], line, &note[idx..])
            } else {
                note.replacen("---\n", &format!("---\n{line}"), 1)
            };
            fs::write(&fixture.note, note).expect("write pointer");
        }
        git(&fixture.vault, &["add", "-A"]);
        git(&fixture.vault, &["commit", "-q", "-m", "seed source store"]);
        refresh_fixture_plan(fixture);
        planned.pointer
    }

    fn attach_staged_store(
        fixture: &mut ExecuteFixture,
        source_bytes: &[u8],
    ) -> source_store::StagedStoreFiles {
        let scratch = fixture._temp.path().join("store-scratch");
        let ref_type = fixture.plan.ref_type.clone();
        let stem = fixture.plan.member.stem.clone();
        let pdf_rel = fixture.plan.pdf_path.clone();
        let catalog =
            super::super::super::code_catalog::CatalogOutcome::Missing;
        let params = source_store::StoreWriteParams {
            ref_type: &ref_type,
            stem: &stem,
            source_bytes,
            route: "markdown",
            origin: "/tmp/report.md".to_string(),
            source_note: None,
            source_note_sha256: None,
            pdf_rel,
            pdf_sha256: hex::encode(Sha256::digest(b"staged-pdf")),
            catalog: &catalog,
            verified: None,
        };
        let staged = source_store::stage_store_files(&scratch, params)
            .expect("stage store");
        fixture.request.planned_source_store = Some(staged.planned.clone());
        refresh_fixture_plan(fixture);
        fixture.staged.source_store = Some(staged.clone());
        staged
    }

    fn annotated_execute_fixture(tag: &str) -> ExecuteFixture {
        let mut fixture = filed_execute_fixture(tag);
        let assets = fixture.vault.join("ref/papers/report.assets/figure.png");
        fs::create_dir_all(assets.parent().expect("assets parent"))
            .expect("assets dir");
        fs::write(&assets, b"archived figure bytes").expect("asset");

        let mut note = fs::read_to_string(&fixture.note).expect("live note");
        note.push_str(
            "\n![[ref/papers/report.assets/figure.png|420]]\n\nQuote stays ^h-execute\n",
        );
        fs::write(&fixture.note, note).expect("annotated note");

        let task_file = fixture.vault.join(&fixture.plan.task.path);
        let task_contents = fs::read_to_string(&task_file).expect("task file");
        let block = fixture
            .plan
            .task
            .block_id
            .as_deref()
            .expect("fixture task block id");
        let task_contents = task_contents.replacen(
            &format!("^{block}"),
            &format!("^{block}\n  - See [[ref/papers/report#^h-execute]]"),
            1,
        );
        fs::write(&task_file, task_contents).expect("inbound task link");
        git(&fixture.vault, &["add", "-A"]);
        git(
            &fixture.vault,
            &["commit", "-q", "-m", "add annotated overwrite fixtures"],
        );
        refresh_fixture_plan(&mut fixture);
        assert!(fixture.plan.annotated.is_some());
        fixture
    }

    fn coordinated_fake_session(
        coordinator: super::super::super::marker_write::FakeCoordinator,
    ) -> super::super::super::marker_write::MarkerWriteSession {
        let config = crate::native::config::HighlightsConfig::default();
        let mut policy =
            super::super::super::pdf_write_policy::PdfWritePolicy::from_config(
                &config,
            );
        policy.coordinate_pdf_writes = true;
        super::super::super::marker_write::MarkerWriteSession::new(
            policy,
            super::super::super::marker_write::MarkerWriteCoordinator::Fake(
                coordinator,
            ),
        )
    }

    fn run_annotated_execute(
        fixture: &ExecuteFixture,
        coordinator: super::super::super::marker_write::FakeCoordinator,
    ) -> (
        Result<OverwriteResult, CommandError>,
        super::super::super::marker_write::FakeCoordinator,
    ) {
        let observed = coordinator.clone();
        let mut request = fixture.request.clone();
        // The fake coordinator is the test-only platform seam; keep Linux
        // preflight on its preview path while exercising the real executor.
        request.dry_run = true;
        let result = execute_with_sync_and_session(
            &fixture.config,
            &fixture.plan,
            &request,
            fixture.staged.clone(),
            |_| Ok(()),
            coordinated_fake_session(coordinator),
        );
        (result, observed)
    }

    fn annotated_pdf_endpoints(fixture: &ExecuteFixture) -> (PathBuf, PathBuf) {
        let archive = fixture
            .plan
            .annotated
            .as_ref()
            .expect("annotated")
            .bundle
            .annotated_pdf_path
            .clone();
        (fixture.pdf.clone(), fixture.vault.join(archive))
    }

    fn forward_move_events(
        from: &Path,
        to: &Path,
    ) -> Vec<super::super::super::marker_write::FakeMoveEvent> {
        use super::super::super::marker_write::FakeMoveEvent;
        vec![
            FakeMoveEvent::Will(from.to_path_buf(), to.to_path_buf()),
            FakeMoveEvent::Did(from.to_path_buf(), to.to_path_buf()),
        ]
    }

    fn forward_and_inverse_move_events(
        from: &Path,
        to: &Path,
    ) -> Vec<super::super::super::marker_write::FakeMoveEvent> {
        use super::super::super::marker_write::FakeMoveEvent;
        vec![
            FakeMoveEvent::Will(from.to_path_buf(), to.to_path_buf()),
            FakeMoveEvent::Did(from.to_path_buf(), to.to_path_buf()),
            FakeMoveEvent::Will(to.to_path_buf(), from.to_path_buf()),
            FakeMoveEvent::Did(to.to_path_buf(), from.to_path_buf()),
        ]
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
    fn annotated_overwrite_moves_bundle_and_merges_task_link_edit_once() {
        use std::os::unix::fs::MetadataExt;

        let fixture = annotated_execute_fixture("bob-cli-overwrite-annotated");
        let annotated = fixture.plan.annotated.as_ref().expect("annotated");
        let archive_note = annotated.bundle.note_path.clone();
        let archive_pdf = annotated.bundle.annotated_pdf_path.clone();
        let archive_assets = annotated.bundle.assets_path.clone();
        let old_pdf_identity =
            fs::metadata(&fixture.pdf).expect("old PDF metadata");
        let old_pdf = fixture.old_pdf.clone();
        let old_assets =
            fs::read(fixture.vault.join("ref/papers/report.assets/figure.png"))
                .expect("old asset");
        let fake = super::super::super::marker_write::FakeCoordinator::grant(
            std::time::Duration::from_millis(17),
        );
        let (result, observed) = run_annotated_execute(&fixture, fake);
        let result = result.unwrap_or_else(|error| panic!("execute: {error}"));

        assert_eq!(observed.call_count(), 1, "one outer PDF accessor");
        let (live_pdf, archive_pdf_abs) = annotated_pdf_endpoints(&fixture);
        assert_eq!(
            observed.move_events(),
            forward_move_events(&live_pdf, &archive_pdf_abs),
            "one forward notification pair"
        );
        assert_eq!(result.archive.as_deref(), Some(archive_note.as_str()));
        let retained_pdf = fixture.vault.join(&archive_pdf);
        assert_eq!(fs::read(&retained_pdf).expect("archived PDF"), old_pdf);
        assert_eq!(
            fs::read(fixture.vault.join(&archive_assets).join("figure.png"))
                .expect("archived asset"),
            old_assets
        );
        assert!(
            !fixture.vault.join("ref/papers/report.assets").exists(),
            "the live assets directory moved"
        );
        assert!(fixture.pdf.is_file(), "the replacement PDF is live");
        #[cfg(unix)]
        {
            let archived_pdf_identity =
                fs::metadata(&retained_pdf).expect("archived PDF metadata");
            assert_eq!(old_pdf_identity.dev(), archived_pdf_identity.dev());
            assert_eq!(old_pdf_identity.ino(), archived_pdf_identity.ino());
        }

        let frozen = fs::read_to_string(fixture.vault.join(&archive_note))
            .expect("frozen annotated note");
        assert!(frozen.contains("Quote stays ^h-execute"));
        assert!(
            frozen.contains(&format!("![[{archive_assets}/figure.png|420]]"))
        );
        assert!(frozen.contains("annotated_pdf:"));
        assert!(frozen.contains(&archive_pdf));
        assert!(frozen.contains("annotated_pdf_sha256:"));

        let task =
            fs::read_to_string(fixture.vault.join(&fixture.plan.task.path))
                .expect("updated reading task");
        let archived_target = archive_note.trim_end_matches(".md");
        assert!(task.contains(&format!("[[{archived_target}#^h-execute]]")));
        assert!(task.contains("A Better Report"));
        assert!(task.contains("preserved child"));

        let live_note = fs::read_to_string(&fixture.note).expect("live note");
        assert!(live_note.contains(&format!(
            "[[{}|replaced 2026-10-16",
            archive_note.trim_end_matches(".md")
        )));
        assert!(live_note.contains("replaced 2026-10-16 · 0 highlights"));
        assert_eq!(
            git(
                &fixture.vault,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            ),
            "",
            "the scoped commit includes the archive bundle and rewritten links"
        );
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
    }

    #[test]
    fn task_edit_and_verification_match_the_exact_reading_block_id() {
        let mut fixture =
            annotated_execute_fixture("bob-cli-overwrite-exact-task-id");
        let task_file = fixture.vault.join(&fixture.plan.task.path);
        let block = fixture
            .plan
            .task
            .block_id
            .clone()
            .expect("reading-task block id");
        assert_eq!(block, "ref-report");
        let contents = fs::read_to_string(&task_file).expect("task file");
        let lines = contents.lines().collect::<Vec<_>>();
        let task_index = lines
            .iter()
            .position(|line| line_has_exact_block_id(line, &block))
            .expect("exact reading task line");
        let mut lines =
            lines.into_iter().map(str::to_string).collect::<Vec<_>>();
        lines.splice(
            task_index..task_index,
            [
                "- [ ] #task Similar name v2 ^ref-report-v2".to_string(),
                "- [ ] #task Similar name two ^ref-report-two".to_string(),
            ],
        );
        fs::write(&task_file, lines.join("\n") + "\n")
            .expect("insert decoy tasks");
        git(&fixture.vault, &["add", "-A"]);
        git(
            &fixture.vault,
            &["commit", "-q", "-m", "add similar task ids"],
        );
        refresh_fixture_plan(&mut fixture);

        grant_and_overwrite(&fixture);
        let after = fs::read_to_string(&task_file).expect("task after");
        assert!(after.contains("- [ ] #task Similar name v2 ^ref-report-v2"));
        assert!(after.contains("- [ ] #task Similar name two ^ref-report-two"));
        assert!(after.contains(&format!("^{block}")));
        assert!(after.contains("A Better Report"));
        assert_eq!(
            after
                .lines()
                .filter(|line| line_has_exact_block_id(line, &block))
                .count(),
            1,
            "only the exact reading task remains matched"
        );
    }

    #[test]
    fn annotated_coordination_hold_writes_nothing_and_has_retry_hint() {
        let fixture =
            annotated_execute_fixture("bob-cli-overwrite-annotated-hold");
        let annotated = fixture.plan.annotated.as_ref().expect("annotated");
        let fake = super::super::super::marker_write::FakeCoordinator::hold(
            std::time::Duration::from_secs(2),
        );
        let (result, observed) = run_annotated_execute(&fixture, fake);
        let error = result.expect_err("held coordinated write");
        assert_eq!(error.code, Some("coordination_timeout"));
        assert!(error.message.contains("close it in Highlights and retry"));
        assert_eq!(observed.call_count(), 1);
        assert_eq!(
            fs::read(&fixture.note).expect("live note"),
            fixture.old_note
        );
        assert_eq!(fs::read(&fixture.pdf).expect("live PDF"), fixture.old_pdf);
        assert!(
            !fixture.vault.join(&annotated.bundle.note_path).exists()
                && !fixture
                    .vault
                    .join(&annotated.bundle.annotated_pdf_path)
                    .exists()
                && !fixture.vault.join(&annotated.bundle.assets_path).exists()
        );
        assert_eq!(
            git(
                &fixture.vault,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            ),
            fixture.old_status
        );
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
    }

    #[test]
    fn coordinated_save_aborts_safely_then_retry_archives_saved_pdf() {
        let mut fixture =
            annotated_execute_fixture("bob-cli-overwrite-annotated-save");
        let mut saved_pdf = fixture.old_pdf.clone();
        saved_pdf.extend_from_slice(b"\n% presenter saved new annotations\n");
        let fake = super::super::super::marker_write::FakeCoordinator::grant_after_save(
            std::time::Duration::from_millis(11),
            saved_pdf.clone(),
        );
        let (result, observed) = run_annotated_execute(&fixture, fake);
        let error = result.expect_err("changed preimage abort");
        assert_eq!(error.code, Some("preimage_changed"));
        assert!(error
            .message
            .contains("Run bob ref scan -w to commit the save"));
        assert_eq!(observed.call_count(), 1);
        assert!(
            observed.move_events().is_empty(),
            "a presenter save after preflight must refuse before any move: {:?}",
            observed.move_events()
        );
        assert_eq!(fs::read(&fixture.pdf).expect("presenter save"), saved_pdf);
        assert_eq!(
            fs::read(&fixture.note).expect("live note"),
            fixture.old_note
        );
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());

        super::super::super::sync::sync_pdf(
            &fixture.config,
            &fixture.pdf,
            SyncOptions {
                dry_run: false,
                write_pdf: false,
                prefer: None,
                ignore_unannotated_sidecar: false,
            },
        )
        .expect("sync presenter save before retry");
        git(&fixture.vault, &["add", "-A"]);
        git(
            &fixture.vault,
            &["commit", "-q", "-m", "sync presenter save"],
        );
        fixture.old_pdf = saved_pdf.clone();
        refresh_fixture_plan(&mut fixture);
        let fake = super::super::super::marker_write::FakeCoordinator::grant(
            std::time::Duration::ZERO,
        );
        let (result, observed) = run_annotated_execute(&fixture, fake);
        let result = result.unwrap_or_else(|error| panic!("retry: {error}"));
        assert_eq!(observed.call_count(), 1);
        let archive_pdf = fixture
            .plan
            .annotated
            .as_ref()
            .expect("annotated retry plan")
            .bundle
            .annotated_pdf_path
            .clone();
        assert_eq!(
            fs::read(fixture.vault.join(archive_pdf)).expect("saved archive"),
            saved_pdf
        );
        assert_eq!(
            result.commit_sha,
            git(&fixture.vault, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn changed_marker_during_coordination_is_a_specific_hold() {
        let fixture =
            annotated_execute_fixture("bob-cli-overwrite-annotated-marker");
        let changed = fixture._temp.path().join("changed-marker.pdf");
        write_marker_pdf(&changed, STAGED_MARKER);
        let fake = super::super::super::marker_write::FakeCoordinator::grant_after_save(
            std::time::Duration::ZERO,
            fs::read(&changed).expect("changed marker PDF"),
        );
        let (result, observed) = run_annotated_execute(&fixture, fake);
        let error = result.expect_err("changed marker abort");
        assert_eq!(error.code, Some("marker_changed"));
        assert!(error.message.contains("close it in Highlights and retry"));
        assert_eq!(observed.call_count(), 1);
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
    }

    #[test]
    fn failed_annotated_transaction_reverses_the_pdf_move_inside_accessor() {
        let fixture =
            annotated_execute_fixture("bob-cli-overwrite-annotated-rollback");
        let archive = fixture
            .plan
            .annotated
            .as_ref()
            .expect("annotated")
            .bundle
            .clone();
        let _armed = arm(OverwriteFailpoint::AfterArchivePdf);
        let fake = super::super::super::marker_write::FakeCoordinator::grant(
            std::time::Duration::ZERO,
        );
        let (result, observed) = run_annotated_execute(&fixture, fake);
        let error = result.expect_err("injected post-move failure");
        assert!(error.message.contains("rolled back"), "{error}");
        assert_eq!(observed.call_count(), 1);
        let (live_pdf, archive_pdf) = annotated_pdf_endpoints(&fixture);
        assert_eq!(
            observed.move_events(),
            forward_and_inverse_move_events(&live_pdf, &archive_pdf),
            "rollback inverts the notified PDF move inside the same accessor"
        );
        assert_eq!(
            fs::read(&fixture.pdf).expect("restored PDF"),
            fixture.old_pdf
        );
        assert!(
            !fixture.vault.join(&archive.note_path).exists()
                && !fixture.vault.join(&archive.annotated_pdf_path).exists()
                && !fixture.vault.join(&archive.assets_path).exists()
        );
        assert_eq!(
            git(
                &fixture.vault,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            ),
            fixture.old_status
        );
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
    }

    #[test]
    fn annotated_failpoint_sweep_restores_bundle_links_and_clears_intent() {
        const POINTS: &[OverwriteFailpoint] = &[
            OverwriteFailpoint::AfterStagingValidation,
            OverwriteFailpoint::AfterIntentPersistence,
            OverwriteFailpoint::FailedNewFileWrite,
            OverwriteFailpoint::AfterFrozenNote,
            OverwriteFailpoint::AfterSidecarRemoval,
            OverwriteFailpoint::AfterArchivePdf,
            OverwriteFailpoint::AfterAssets,
            OverwriteFailpoint::AfterAudio,
            OverwriteFailpoint::AfterPdf,
            OverwriteFailpoint::AfterNote,
            OverwriteFailpoint::AfterTaskEdit,
            OverwriteFailpoint::AfterSyncWrite,
            OverwriteFailpoint::ForcedVerificationFailure,
            OverwriteFailpoint::BeforeCommit,
        ];
        for point in POINTS {
            let fixture = annotated_execute_fixture(&format!(
                "bob-cli-overwrite-annotated-fp-{point:?}"
            ));
            let _armed = arm(*point);
            let fake =
                super::super::super::marker_write::FakeCoordinator::grant(
                    std::time::Duration::ZERO,
                );
            let (result, observed) = run_annotated_execute(&fixture, fake);
            let error = result.expect_err(&format!("{point:?} must abort"));
            assert!(
                error.to_string().contains("injected failpoint")
                    || error
                        .to_string()
                        .contains("injected partial write failure")
                    || error.code == Some("injected_failpoint"),
                "{point:?} unexpected error: {error}"
            );
            assert_intact_annotated_state(&fixture);
            let (live_pdf, archive_pdf) = annotated_pdf_endpoints(&fixture);
            match point {
                OverwriteFailpoint::AfterStagingValidation => {
                    assert_eq!(observed.call_count(), 0, "{point:?}");
                    assert!(observed.move_events().is_empty(), "{point:?}");
                }
                OverwriteFailpoint::AfterIntentPersistence
                | OverwriteFailpoint::FailedNewFileWrite
                | OverwriteFailpoint::AfterFrozenNote
                | OverwriteFailpoint::AfterSidecarRemoval => {
                    assert_eq!(observed.call_count(), 1, "{point:?}");
                    assert!(
                        observed.move_events().is_empty(),
                        "{point:?} must not notify a move: {:?}",
                        observed.move_events()
                    );
                }
                _ => {
                    assert_eq!(observed.call_count(), 1, "{point:?}");
                    assert_eq!(
                        observed.move_events(),
                        forward_and_inverse_move_events(
                            &live_pdf,
                            &archive_pdf
                        ),
                        "{point:?}"
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn annotated_overwrite_rollback_keeps_vault_relative_paths_through_symlinked_parent(
    ) {
        use std::os::unix::fs::symlink;

        let mut fixture =
            annotated_execute_fixture("bob-cli-overwrite-symlink-parent");
        let actual_parent = fixture._temp.path().join("actual-parent");
        fs::create_dir_all(&actual_parent).expect("actual parent");
        let actual_vault = actual_parent.join("vault");
        fs::rename(&fixture.vault, &actual_vault).expect("move vault");
        let alias_parent = fixture._temp.path().join("vault-parent-link");
        symlink(&actual_parent, &alias_parent).expect("symlink parent");
        let alias_vault = alias_parent.join("vault");
        // Overwrite only runs against the `BOB_DIR` vault, which moved.
        let _bob_dir = crate::native::env::TestEnvGuard::set(&[(
            "BOB_DIR",
            Some(alias_vault.as_os_str()),
        )]);
        fixture.vault = alias_vault.clone();
        fixture.note = alias_vault.join("ref/papers/report.md");
        fixture.pdf = alias_vault.join("lib/papers/report.pdf");
        fixture.config = Config::for_vault(&alias_vault);
        refresh_fixture_plan(&mut fixture);

        let _armed = arm(OverwriteFailpoint::CrashWithoutRollback);
        let fake = super::super::super::marker_write::FakeCoordinator::grant(
            std::time::Duration::ZERO,
        );
        let (result, _) = run_annotated_execute(&fixture, fake);
        let error = result.expect_err("injected crash");
        assert_eq!(error.code, Some("injected_crash"));
        drop(_armed);
        let intent = super::super::recovery::load_intent(&fixture.config)
            .expect("load symlink-vault intent")
            .expect("crashed transaction intent");
        for path in intent.touched.iter().chain(&intent.created) {
            assert!(
                !Path::new(path).is_absolute(),
                "intent path stays vault-relative: {path}"
            );
        }
        for step in &intent.moves {
            for path in [&step.source, &step.dest] {
                assert!(!Path::new(path).is_absolute(), "move path: {path}");
            }
            for file in &step.files {
                assert!(
                    !Path::new(&file.rel).is_absolute(),
                    "asset path: {}",
                    file.rel
                );
            }
        }
        for write in &intent.writes {
            assert!(
                !Path::new(&write.path).is_absolute(),
                "write path: {}",
                write.path
            );
        }
        let annotated =
            fixture.plan.annotated.as_ref().expect("annotated plan");
        for preimage in &fixture.plan.bundle_preimages {
            assert!(
                !Path::new(&preimage.path).is_absolute(),
                "preimage path: {}",
                preimage.path
            );
        }
        for preimage in &annotated.preimages {
            assert!(
                !Path::new(&preimage.path).is_absolute(),
                "annotated preimage path: {}",
                preimage.path
            );
        }
        assert!(
            annotated
                .live_assets_path
                .as_deref()
                .is_some_and(|path| !Path::new(path).is_absolute()),
            "assets_relative stays vault-relative"
        );
        super::super::recovery::recover_with_locks_held(
            &fixture.config,
            &ob::child_env(),
            false,
        )
        .expect("rollback symlink-vault transaction");
        assert_intact_annotated_state(&fixture);
    }

    #[test]
    fn older_archive_save_survives_failed_second_overwrite_rollback() {
        let (fixture, first_archive_pdf) = second_annotated_overwrite_fixture(
            "bob-cli-overwrite-older-save-rollback",
        );
        let original_archive =
            fs::read(fixture.vault.join(&first_archive_pdf)).expect("archive");
        let _armed = arm(OverwriteFailpoint::LateHistorySaveBeforeRollback);
        let fake = super::super::super::marker_write::FakeCoordinator::grant(
            std::time::Duration::ZERO,
        );
        let (result, _) = run_annotated_execute(&fixture, fake);
        let error = result.expect_err("late history save failpoint");
        assert!(
            error.to_string().contains("LateHistorySaveBeforeRollback"),
            "unexpected failure before failpoint: {error}"
        );
        assert_eq!(
            fs::read(fixture.vault.join(&first_archive_pdf))
                .expect("older archive after rollback"),
            [original_archive, b"\nlate older archive save\n".to_vec()]
                .concat(),
            "rollback never restores an older archive from HEAD"
        );
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
        assert_eq!(
            fs::read(&fixture.note).expect("live note"),
            fixture.old_note
        );
        assert_eq!(fs::read(&fixture.pdf).expect("live PDF"), fixture.old_pdf);
    }

    #[test]
    fn older_archive_save_survives_second_overwrite_crash_recovery() {
        let (fixture, first_archive_pdf) = second_annotated_overwrite_fixture(
            "bob-cli-overwrite-older-save-crash",
        );
        let original_archive =
            fs::read(fixture.vault.join(&first_archive_pdf)).expect("archive");
        let _armed = arm(OverwriteFailpoint::CrashAfterLateHistorySave);
        let fake = super::super::super::marker_write::FakeCoordinator::grant(
            std::time::Duration::ZERO,
        );
        let (result, _) = run_annotated_execute(&fixture, fake);
        let error = result.expect_err("injected crash");
        assert_eq!(
            error.code,
            Some("injected_crash"),
            "unexpected failure before failpoint: {error}"
        );
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_some());
        drop(_armed);

        super::super::recovery::recover_with_locks_held(
            &fixture.config,
            &ob::child_env(),
            false,
        )
        .expect("recover second overwrite");
        assert_eq!(
            fs::read(fixture.vault.join(&first_archive_pdf))
                .expect("older archive after recovery"),
            [original_archive, b"\nlate older archive save\n".to_vec()]
                .concat(),
            "crash recovery leaves the older archive byte-for-byte as saved"
        );
        assert_eq!(
            fs::read(&fixture.note).expect("live note"),
            fixture.old_note
        );
        assert_eq!(fs::read(&fixture.pdf).expect("live PDF"), fixture.old_pdf);
        assert!(super::super::recovery::load_intent(&fixture.config)
            .expect("load intent")
            .is_none());
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
    fn overwrite_repoints_source_markdown_and_keeps_frozen_pointer() {
        let mut fixture =
            filed_execute_fixture("bob-cli-overwrite-store-repoint");
        let old_bytes = b"# Old saved markdown\n";
        let old_pointer = seed_live_store(&mut fixture, old_bytes);
        let new_bytes = b"# Replacement saved markdown\n";
        let staged = attach_staged_store(&mut fixture, new_bytes);
        let result = run_execute(&fixture, |_| Ok(()))
            .unwrap_or_else(|error| panic!("execute: {error}"));
        assert_committed_new_state(&fixture, &result);
        let live = fs::read_to_string(&fixture.note).expect("live note");
        assert!(
            live.contains(&format!(
                "source_markdown: {}",
                staged.planned.pointer
            )),
            "live note must repoint:\n{live}"
        );
        assert!(
            !live.contains(&format!("source_markdown: {old_pointer}")),
            "live note must drop the old pointer:\n{live}"
        );
        let archive = fixture
            .plan
            .frozen_note
            .as_ref()
            .expect("fixture plans a frozen note");
        let frozen = fs::read_to_string(fixture.vault.join(&archive.path))
            .expect("frozen note");
        assert!(
            frozen.contains(&format!("source_markdown: {old_pointer}")),
            "frozen note keeps the old pointer:\n{frozen}"
        );
        assert!(
            fixture.vault.join(&old_pointer).is_file(),
            "old store stays as history"
        );
        assert!(
            fixture.vault.join(&staged.planned.source_rel).is_file(),
            "new store is installed"
        );
    }

    #[test]
    fn after_note_failpoint_removes_new_store_and_restores_pointer() {
        let mut fixture =
            filed_execute_fixture("bob-cli-overwrite-store-rollback");
        let old_bytes = b"# Old saved markdown\n";
        let old_pointer = seed_live_store(&mut fixture, old_bytes);
        let new_bytes = b"# Replacement saved markdown\n";
        let staged = attach_staged_store(&mut fixture, new_bytes);
        let _armed = arm(OverwriteFailpoint::AfterNote);
        let error = run_execute(&fixture, |_| Ok(()))
            .expect_err("injected failpoint must abort");
        assert!(
            error.to_string().contains("injected failpoint")
                || error.code == Some("injected_failpoint"),
            "unexpected error: {error}"
        );
        assert_intact_old_state(&fixture);
        let live = fs::read_to_string(&fixture.note).expect("live note");
        assert!(
            live.contains(&format!("source_markdown: {old_pointer}")),
            "rollback restores the old pointer:\n{live}"
        );
        assert!(
            fixture.vault.join(&old_pointer).is_file(),
            "old store survives rollback"
        );
        assert!(
            !fixture.vault.join(&staged.planned.source_rel).exists(),
            "new store must be removed on rollback"
        );
    }

    #[test]
    fn unrelated_staged_and_dirty_files_survive_abort() {
        let fixture = filed_execute_fixture("bob-cli-overwrite-fp-unrelated");
        // An unrelated staged file and an unrelated dirty change must
        // survive a rollback that restores only the transaction paths.
        let staged_path = fixture.vault.join("unrelated-staged.md");
        fs::write(&staged_path, "staged\n").expect("staged");
        git(&fixture.vault, &["add", "unrelated-staged.md"]);
        let dirty_path = fixture.vault.join("unrelated-dirty.md");
        fs::write(&dirty_path, "dirty\n").expect("dirty");
        let status_before = git(
            &fixture.vault,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        );
        assert!(
            status_before.contains("unrelated-staged.md")
                && status_before.contains("unrelated-dirty.md"),
            "unrelated files are staged/dirty:\n{status_before}"
        );
        let _armed = arm(OverwriteFailpoint::AfterPdf);
        let error = run_execute(&fixture, |_| Ok(()))
            .expect_err("injected failpoint must abort");
        assert!(
            error.to_string().contains("injected failpoint")
                || error.code == Some("injected_failpoint"),
            "unexpected error: {error}"
        );
        assert_eq!(
            fs::read(&staged_path).expect("staged"),
            b"staged\n",
            "staged unrelated file survives"
        );
        assert_eq!(
            fs::read(&dirty_path).expect("dirty"),
            b"dirty\n",
            "dirty unrelated file survives"
        );
        assert_eq!(
            git(
                &fixture.vault,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            ),
            status_before,
            "rollback leaves unrelated staged/dirty state untouched"
        );
        // The transaction paths themselves still roll back.
        assert_eq!(fs::read(&fixture.note).expect("note"), fixture.old_note,);
        assert_eq!(fs::read(&fixture.pdf).expect("pdf"), fixture.old_pdf);
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

    const IDENTITY_SIDECAR: &str = "\
## Page 1

> Stable quote for identity.

- #task Follow up on the quote.
";

    const SECOND_STAGED_MARKER: &str = concat!(
        "- status: next\n",
        "- parent: sase\n",
        "- title: A Better Report\n",
        "- id: report\n",
        "- captured: 2026-10-17\n",
    );

    fn plant_identity_sidecar(fixture: &ExecuteFixture) {
        fs::write(fixture.vault.join("lib/papers/report.md"), IDENTITY_SIDECAR)
            .expect("identity sidecar");
    }

    fn sync_fixture_pdf(fixture: &ExecuteFixture) {
        super::super::super::sync::sync_pdf(
            &fixture.config,
            &fixture.pdf,
            SyncOptions {
                dry_run: false,
                write_pdf: false,
                prefer: None,
                ignore_unannotated_sidecar: false,
            },
        )
        .unwrap_or_else(|error| panic!("sync identity sidecar: {error}"));
    }

    fn highlights_count_from_note(note: &Path) -> Option<u64> {
        fs::read_to_string(note).ok().and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("highlights_count:")
                    .and_then(|value| value.trim().parse().ok())
            })
        })
    }

    fn first_highlight_id(contents: &str) -> String {
        contents
            .split_whitespace()
            .find_map(|token| {
                token.strip_prefix('^').and_then(|id| {
                    (id.starts_with("h-") && id.len() == 14)
                        .then(|| id.to_string())
                })
            })
            .expect("highlight block id")
    }

    fn follow_up_lines(parent: &str) -> Vec<String> {
        parent
            .lines()
            .filter(|line| line.contains("Follow up on the quote"))
            .map(str::to_string)
            .collect()
    }

    fn processed_id(line: &str) -> String {
        line.split("[h::")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .expect("processed id")
            .to_string()
    }

    fn library_row_paths(fixture: &ExecuteFixture) -> Vec<String> {
        crate::native::ref_library::build_index(
            &crate::native::ref_library::LibraryConfig {
                bob_dir: fixture.config.bob_dir.clone(),
                ref_dir: fixture.config.ref_dir.clone(),
                xlib_dir: fixture.config.xlib_dir.clone(),
            },
        )
        .expect("index")
        .rows
        .into_iter()
        .map(|row| row.path)
        .collect()
    }

    fn processed_annotation_execute_fixture(tag: &str) -> ExecuteFixture {
        let mut fixture = filed_execute_fixture(tag);
        fixture.request.replacement_audio_path = None;
        fixture.staged.audio = None;
        plant_identity_sidecar(&fixture);
        sync_fixture_pdf(&fixture);
        git(&fixture.vault, &["add", "-A"]);
        git(
            &fixture.vault,
            &["commit", "-q", "-m", "plant processed annotation"],
        );
        fixture.plan.member.highlights =
            highlights_count_from_note(&fixture.note);
        refresh_fixture_plan(&mut fixture);
        assert!(fixture.plan.annotated.is_some());
        assert!(
            fixture.plan.highlights >= 1,
            "processed annotation must report a real highlight count, got {}",
            fixture.plan.highlights
        );
        fixture
    }

    fn grant_and_overwrite(fixture: &ExecuteFixture) -> OverwriteResult {
        let fake = super::super::super::marker_write::FakeCoordinator::grant(
            std::time::Duration::ZERO,
        );
        let (result, observed) = run_annotated_execute(fixture, fake);
        let result = result.unwrap_or_else(|error| panic!("execute: {error}"));
        assert_eq!(observed.call_count(), 1, "one outer PDF accessor");
        result
    }

    fn assert_intact_annotated_state(fixture: &ExecuteFixture) {
        assert_intact_old_state(fixture);
        let annotated = fixture.plan.annotated.as_ref().expect("annotated");
        let live_assets = annotated
            .live_assets_path
            .as_deref()
            .expect("fixture assets path");
        for file in &annotated.asset_manifest {
            let path = fixture.vault.join(live_assets).join(&file.rel);
            let bytes = fs::read(&path).expect("restored asset");
            assert_eq!(
                super::super::sha256(&bytes),
                file.sha256,
                "asset bytes restore exactly at {}",
                path.display()
            );
        }
        for path in [
            &annotated.bundle.note_path,
            &annotated.bundle.annotated_pdf_path,
            &annotated.bundle.assets_path,
        ] {
            assert!(
                !fixture.vault.join(path).exists(),
                "current archive bundle path is absent after rollback: {path}"
            );
        }
    }

    fn second_annotated_overwrite_fixture(
        tag: &str,
    ) -> (ExecuteFixture, String) {
        let mut fixture = annotated_execute_fixture(tag);
        let first_archive_pdf = fixture
            .plan
            .annotated
            .as_ref()
            .expect("first annotated plan")
            .bundle
            .annotated_pdf_path
            .clone();
        grant_and_overwrite(&fixture);
        write_marker_pdf(&fixture.staged.private_pdf, SECOND_STAGED_MARKER);
        fixture.staged.marker.captured = Some("2026-10-17".into());
        fs::create_dir_all(fixture.vault.join("ref/papers/report.assets"))
            .expect("second assets directory");
        fs::write(
            fixture.vault.join("ref/papers/report.assets/revision.png"),
            b"second annotation asset",
        )
        .expect("second asset");
        git(&fixture.vault, &["add", "-A"]);
        git(
            &fixture.vault,
            &["commit", "-q", "-m", "add second annotation"],
        );
        refresh_fixture_plan(&mut fixture);
        assert_eq!(
            fixture
                .plan
                .annotated
                .as_ref()
                .expect("second annotated plan")
                .history
                .len(),
            1,
            "second overwrite fingerprints the older archive"
        );
        (fixture, first_archive_pdf)
    }

    #[test]
    fn processed_highlight_identity_survives_overwrite_and_rescan() {
        let fixture = processed_annotation_execute_fixture(
            "bob-cli-overwrite-h-identity",
        );
        let live_before = fs::read_to_string(&fixture.note).expect("live note");
        let highlight_id = first_highlight_id(&live_before);
        let parent_before =
            fs::read_to_string(fixture.vault.join("sase.md")).expect("parent");
        let follows = follow_up_lines(&parent_before);
        assert_eq!(follows.len(), 1, "one follow-up before overwrite");
        let processed = processed_id(&follows[0]);
        let task_before =
            fs::read_to_string(fixture.vault.join(&fixture.plan.task.path))
                .expect("reading task");
        let block = fixture
            .plan
            .task
            .block_id
            .clone()
            .expect("reading-task block id");

        let result = grant_and_overwrite(&fixture);
        let archive = fixture
            .plan
            .annotated
            .as_ref()
            .expect("annotated")
            .bundle
            .note_path
            .clone();
        let frozen =
            fs::read_to_string(fixture.vault.join(&archive)).expect("archive");
        assert!(
            frozen.contains(&format!("^{highlight_id}")),
            "frozen note keeps the processed highlight id:\n{frozen}"
        );
        assert!(
            !frozen.contains("#task #ref") && !frozen.contains("^ref"),
            "archive must not grow a reading tracker:\n{frozen}"
        );
        let open_tasks = frozen
            .lines()
            .filter(|line| {
                ref_tasks::task_mark(line).is_some_and(ref_tasks::is_open_mark)
            })
            .collect::<Vec<_>>();
        assert!(
            open_tasks.is_empty(),
            "frozen note must have no open task lines outside the neutralized banner; found {open_tasks:?}:\n{frozen}"
        );
        let count = fixture.plan.highlights;
        let count_label = if count == 1 {
            "1 highlight".to_string()
        } else {
            format!("{count} highlights")
        };
        let live = fs::read_to_string(&fixture.note).expect("live after");
        assert!(
            live.contains(&format!("replaced 2026-10-16 · {count_label}")),
            "live lineage must use the real highlight count:\n{live}"
        );
        assert_eq!(result.highlights, count);
        let paths = library_row_paths(&fixture);
        assert!(
            paths.iter().all(|path| !path.contains("ref_history/")),
            "archives must not appear as library rows:\n{paths:?}"
        );

        plant_identity_sidecar(&fixture);
        sync_fixture_pdf(&fixture);
        let live_again = fs::read_to_string(&fixture.note).expect("rescan");
        assert_eq!(
            first_highlight_id(&live_again),
            highlight_id,
            "re-highlighting the same passage must keep ^{highlight_id}:\n{live_again}"
        );
        let parent_after =
            fs::read_to_string(fixture.vault.join("sase.md")).expect("parent");
        let follows_after = follow_up_lines(&parent_after);
        assert_eq!(
            follows_after.len(),
            1,
            "rescan must not mint a duplicate follow-up:\n{parent_after}"
        );
        assert_eq!(
            processed_id(&follows_after[0]),
            processed,
            "processed [h::] id must survive:\n{}",
            follows_after[0]
        );
        let task_after =
            fs::read_to_string(fixture.vault.join(&fixture.plan.task.path))
                .expect("reading task after");
        assert!(
            task_after.contains(&format!("^{block}")),
            "reading-task identity must stay:\n{task_after}"
        );
        assert_eq!(
            task_after
                .lines()
                .filter(|line| line.contains(&format!("^{block}")))
                .count(),
            task_before
                .lines()
                .filter(|line| line.contains(&format!("^{block}")))
                .count()
        );
    }

    #[test]
    fn two_annotated_overwrites_keep_newest_first_archives_and_task() {
        let mut fixture =
            processed_annotation_execute_fixture("bob-cli-overwrite-ann-twice");
        let block = fixture
            .plan
            .task
            .block_id
            .clone()
            .expect("reading-task block id");
        let first = grant_and_overwrite(&fixture);
        let first_archive = first.archive.clone().expect("first archive");

        plant_identity_sidecar(&fixture);
        sync_fixture_pdf(&fixture);
        git(&fixture.vault, &["add", "-A"]);
        git(
            &fixture.vault,
            &["commit", "-q", "-m", "re-annotate for second overwrite"],
        );
        write_marker_pdf(&fixture.staged.private_pdf, SECOND_STAGED_MARKER);
        fixture.staged.marker.captured = Some("2026-10-17".into());
        fixture.plan.member.highlights =
            highlights_count_from_note(&fixture.note);
        refresh_fixture_plan(&mut fixture);
        assert!(fixture.plan.annotated.is_some());
        let second = grant_and_overwrite(&fixture);
        let second_archive = second.archive.clone().expect("second archive");
        assert_ne!(first_archive, second_archive);
        assert!(
            fixture.vault.join(&first_archive).is_file()
                && fixture.vault.join(&second_archive).is_file(),
            "both annotated archives remain"
        );
        let live = fs::read_to_string(&fixture.note).expect("live");
        let newest = format!("{}|", second_archive.trim_end_matches(".md"));
        let older = format!("{}|", first_archive.trim_end_matches(".md"));
        let first_pos = live.find(&newest).expect("newest archive");
        let second_pos = live.find(&older).expect("older archive");
        assert!(first_pos < second_pos, "newest archive is first:\n{live}");
        let parent =
            fs::read_to_string(fixture.vault.join("sase.md")).expect("parent");
        assert_eq!(
            parent
                .lines()
                .filter(|line| line.contains(&format!("^{block}")))
                .count(),
            1,
            "exactly one reading task remains:\n{parent}"
        );
        let paths = library_row_paths(&fixture);
        assert!(
            paths.iter().all(|path| !path.contains("ref_history/")),
            "neither archive is a library row:\n{paths:?}"
        );
    }
}
