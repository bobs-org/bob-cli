use std::fs::{self, File};

use super::{
    model::{
        remaining_outputs, AppliedWrite, ApplyError, ApplyOutcome,
        ApplySession, ReasonCode, WritePlan,
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
        if let Err(error) = preflight_for_replacement(plan, &next, &applied) {
            cleanup_temps(&remaining_temps, session.tool);
            let _ = update_manifest(
                &recovery_dir,
                plan,
                session,
                "partial",
                &applied,
            );
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
                    recovery_directory: Some(recovery_dir),
            });
        }
        if let Err(error) = fs::rename(&next.temp, &next.dest) {
            cleanup_temps(&remaining_temps, session.tool);
            let _ = fs::remove_file(&next.temp);
            let _ = update_manifest(
                &recovery_dir,
                plan,
                session,
                "partial",
                &applied,
            );
            let applied_paths = applied
                .iter()
                .map(|item| item.path.clone())
                .collect::<Vec<_>>();
            let remaining = remaining_outputs(plan, &applied_paths);
            return Err(ApplyError::io(
                format!("failed to replace {}: {error}", next.dest.display()),
                applied_paths,
                remaining,
                Some(recovery_dir),
            ));
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
