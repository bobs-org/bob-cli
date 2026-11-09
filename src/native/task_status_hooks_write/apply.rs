use std::{
    fs::{self, File},
    io,
};

use super::{
    model::{
        remaining_outputs, AppliedWrite, ApplyError, ApplyOutcome,
        ApplySession, ReasonCode, StagedWrite, WritePlan,
    },
    preflight::{preflight, preflight_for_replacement, wait_quiet_period},
    recovery::{create_recovery, prune_completed, update_manifest},
    staging::{cleanup_temps, stage_outputs},
};
use crate::native::ob;

pub(crate) fn acquire_maintenance_lock() -> Result<File, ApplyError> {
    match ob::try_acquire_lock() {
        Ok(file) => Ok(file),
        Err(ob::LockAcquireError::Contended) => {
            Err(ApplyError::lock_contention())
        }
        Err(ob::LockAcquireError::Open { path, error })
        | Err(ob::LockAcquireError::Acquire { path, error }) => {
            Err(ApplyError::lock_io(&path, error))
        }
    }
}

pub(crate) fn apply_plan(
    plan: &WritePlan,
    session: &ApplySession,
) -> Result<ApplyOutcome, ApplyError> {
    if plan.outputs.is_empty() {
        return Ok(ApplyOutcome::NoOp);
    }

    wait_quiet_period(plan, session)?;
    if let Some(callback) = &session.before_preflight {
        callback();
    }
    preflight(plan, session, &[])?;

    let recovery_dir = create_recovery(plan, session)?;
    let staged = stage_outputs(plan, session, &recovery_dir)?;
    if let Some(callback) = &session.after_staging {
        callback();
    }
    if let Err(mut error) = preflight(plan, session, &[]) {
        cleanup_temps(&staged, session.tool);
        if error.recovery_directory.is_none() {
            error.recovery_directory = Some(recovery_dir);
        }
        return Err(error);
    }

    let mut applied = Vec::new();
    let mut remaining_temps = staged;
    while !remaining_temps.is_empty() {
        let next = remaining_temps.remove(0);
        if let Some(callback) = &session.before_replace {
            callback(&next.dest);
        }
        if let Err(mut error) =
            preflight_for_replacement(plan, session, &next, &applied)
        {
            cleanup_current_and_remaining(
                &next,
                &remaining_temps,
                session.tool,
            );
            let _ = update_manifest(
                &recovery_dir,
                plan,
                session,
                if applied.is_empty() {
                    "planned"
                } else {
                    "partial"
                },
                &applied,
            );
            if error.recovery_directory.is_none() {
                error.recovery_directory = Some(recovery_dir);
            }
            if applied.is_empty() {
                return Err(error);
            }
            let remaining = remaining_outputs(
                plan,
                &applied
                    .iter()
                    .map(|item| item.path.clone())
                    .collect::<Vec<_>>(),
            );
            return Err(ApplyError {
                reason: ReasonCode::PartialApply,
                message: format!(
                    "applied {} note(s) then stopped ({}); remaining notes were not written",
                    applied.len(),
                    error.message
                ),
                applied_files: applied.iter().map(|item| item.path.clone()).collect(),
                deferred_files: remaining,
                recovery_directory: error.recovery_directory,
            });
        }
        match install_staged(&next) {
            Ok(InstallResult::Applied { leftover_temp }) => {
                if leftover_temp {
                    eprintln!(
                        "bob {}: warning: created {} but failed to remove staging file {}; recovery evidence remains",
                        session.tool,
                        next.dest.display(),
                        next.temp.display()
                    );
                }
            }
            Err(error) => {
                cleanup_current_and_remaining(
                    &next,
                    &remaining_temps,
                    session.tool,
                );
                let _ = update_manifest(
                    &recovery_dir,
                    plan,
                    session,
                    if applied.is_empty() {
                        "planned"
                    } else {
                        "partial"
                    },
                    &applied,
                );
                let applied_paths = applied
                    .iter()
                    .map(|item| item.path.clone())
                    .collect::<Vec<_>>();
                let remaining = remaining_outputs(plan, &applied_paths);
                if next.is_create
                    && error.kind() == io::ErrorKind::AlreadyExists
                {
                    return Err(ApplyError::vault_changed(
                        plan,
                        &applied_paths,
                        Some(recovery_dir),
                        format!(
                            "destination {} already exists",
                            next.dest.display()
                        ),
                    ));
                }
                return Err(ApplyError::io(
                    format!(
                        "failed to {} {}: {error}",
                        if next.is_create { "create" } else { "replace" },
                        next.dest.display()
                    ),
                    applied_paths,
                    remaining,
                    Some(recovery_dir),
                ));
            }
        }
        applied.push(AppliedWrite {
            path: next.dest.clone(),
            proposed_bytes: next.proposed_bytes.clone(),
        });
        if let Err(error) =
            update_manifest(&recovery_dir, plan, session, "partial", &applied)
        {
            eprintln!(
                "bob {}: warning: failed to update recovery manifest {}: {error}",
                session.tool,
                recovery_dir.display()
            );
        }
    }

    if let Err(error) =
        update_manifest(&recovery_dir, plan, session, "applied", &applied)
    {
        eprintln!(
            "bob {}: warning: failed to finalize recovery manifest {}: {error}",
            session.tool,
            recovery_dir.display()
        );
    }
    if let Err(error) = prune_completed(
        &session.state_home.join("bob-cli").join(session.tool),
        session.tool,
        session.now(),
        session.retention,
    ) {
        eprintln!(
            "bob {}: warning: failed to prune old recovery records: {error}",
            session.tool
        );
    }

    Ok(ApplyOutcome::Applied {
        applied_files: applied.into_iter().map(|item| item.path).collect(),
        recovery_directory: recovery_dir,
    })
}

enum InstallResult {
    Applied { leftover_temp: bool },
}

fn install_staged(next: &StagedWrite) -> io::Result<InstallResult> {
    if next.is_create {
        fs::hard_link(&next.temp, &next.dest)?;
        match fs::remove_file(&next.temp) {
            Ok(()) => Ok(InstallResult::Applied {
                leftover_temp: false,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(InstallResult::Applied {
                    leftover_temp: false,
                })
            }
            Err(_) => Ok(InstallResult::Applied {
                leftover_temp: true,
            }),
        }
    } else {
        fs::rename(&next.temp, &next.dest)?;
        Ok(InstallResult::Applied {
            leftover_temp: false,
        })
    }
}

fn cleanup_current_and_remaining(
    current: &StagedWrite,
    remaining: &[StagedWrite],
    tool: &str,
) {
    cleanup_temps(std::slice::from_ref(current), tool);
    cleanup_temps(remaining, tool);
}
