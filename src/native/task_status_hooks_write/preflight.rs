use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    model::{
        remaining_outputs, AppliedWrite, ApplyError, ApplySession,
        CaptureError, InputKind, PlannedWrite, ReasonCode, StagedWrite,
        WritePlan,
    },
    snapshot::{capture_required, recapture},
};

pub(super) fn wait_quiet_period(
    plan: &WritePlan,
    session: &ApplySession,
) -> Result<(), ApplyError> {
    let structural = plan
        .outputs
        .iter()
        .filter(|output| output.structural_regrouping)
        .collect::<Vec<_>>();
    if structural.is_empty() {
        return Ok(());
    }

    let now = session.now();
    let mut wait = Duration::ZERO;
    for output in &structural {
        let metadata = fs::symlink_metadata(&output.path).map_err(|error| {
            ApplyError::quiet_period(
                plan,
                format!("could not read {}: {error}", output.path.display()),
            )
        })?;
        let mtime = metadata.modified().ok().or(output.identity.mtime);
        let Some(mtime) = mtime else {
            return Err(ApplyError::quiet_period(
                plan,
                format!(
                    "modification time of {} is uncertain",
                    output.path.display()
                ),
            ));
        };
        match now.duration_since(mtime) {
            Ok(age) => {
                wait = wait.max(session.quiet_period.saturating_sub(age));
            }
            Err(_) => {
                return Err(ApplyError::quiet_period(
                    plan,
                    format!(
                        "modification time of {} is in the future",
                        output.path.display()
                    ),
                ));
            }
        }
    }
    if wait > session.quiet_period {
        wait = session.quiet_period;
    }
    if wait > Duration::ZERO {
        (session.sleep)(wait);
    }
    Ok(())
}

pub(super) fn preflight(
    plan: &WritePlan,
    session: &ApplySession,
    applied: &[AppliedWrite],
) -> Result<(), ApplyError> {
    let applied_paths = applied
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    for input in &plan.inputs {
        let current = recapture(input).map_err(|error| {
            let reason = match error {
                CaptureError::Unstable(_) => ReasonCode::UnstableRead,
                CaptureError::Unsupported { .. } => ReasonCode::UnsupportedFile,
                _ => ReasonCode::VaultChanged,
            };
            if applied.is_empty() {
                ApplyError::capture(reason, error)
            } else {
                ApplyError::vault_changed(
                    plan,
                    &applied_paths,
                    None,
                    error.message(),
                )
            }
        })?;
        if let Some(applied_item) = applied.iter().find(|item| {
            item.path == input.path
                || current.identity().is_some_and(|identity| {
                    fs::canonicalize(&item.path).ok().as_ref()
                        == Some(&identity.canonical_path)
                })
        }) {
            if current.bytes() != Some(applied_item.proposed_bytes.as_slice()) {
                return Err(ApplyError::vault_changed(
                    plan,
                    &applied_paths,
                    None,
                    format!(
                        "already-written {} changed after replacement",
                        applied_item.path.display()
                    ),
                ));
            }
            continue;
        }
        if !input.matches(&current) {
            return Err(ApplyError::vault_changed(
                plan,
                &applied_paths,
                None,
                format!("{} changed", input.path.display()),
            ));
        }
    }

    let scanned = (session.rescan)().map_err(|error| {
        ApplyError::io(
            format!("failed to rescan vault: {error}"),
            applied_paths.clone(),
            remaining_outputs(plan, &applied_paths),
            None,
        )
    })?;
    let original: BTreeSet<&Path> =
        plan.scan_paths.iter().map(PathBuf::as_path).collect();
    let current: BTreeSet<&Path> =
        scanned.iter().map(PathBuf::as_path).collect();
    if original != current {
        return Err(ApplyError::vault_changed(
            plan,
            &applied_paths,
            None,
            "the set of scanned notes changed",
        ));
    }

    for output in &plan.outputs {
        if applied.iter().any(|item| item.path == output.path) {
            continue;
        }
        check_unwritten_output(output).map_err(|error| {
            ApplyError::vault_changed(
                plan,
                &applied_paths,
                None,
                error.message.clone(),
            )
        })?;
    }
    Ok(())
}

pub(super) fn preflight_for_replacement(
    plan: &WritePlan,
    next: &StagedWrite,
    applied: &[AppliedWrite],
) -> Result<(), ApplyError> {
    for item in applied {
        let current =
            capture_required(&item.path, InputKind::Note).map_err(|error| {
                ApplyError::capture(ReasonCode::VaultChanged, error)
            })?;
        if current.bytes() != Some(item.proposed_bytes.as_slice()) {
            return Err(ApplyError::vault_changed(
                plan,
                &applied
                    .iter()
                    .map(|item| item.path.clone())
                    .collect::<Vec<_>>(),
                None,
                format!(
                    "already-written {} changed after replacement",
                    item.path.display()
                ),
            ));
        }
    }
    let output = plan
        .outputs
        .iter()
        .find(|output| output.path == next.dest)
        .expect("staged path is part of the plan");
    check_unwritten_output(output)
}

fn check_unwritten_output(output: &PlannedWrite) -> Result<(), ApplyError> {
    let current =
        capture_required(&output.path, InputKind::Note).map_err(|error| {
            let reason = match error {
                CaptureError::Unstable(_) => ReasonCode::UnstableRead,
                CaptureError::Unsupported { .. } => ReasonCode::UnsupportedFile,
                CaptureError::NotFound(_) => ReasonCode::VaultChanged,
                _ => ReasonCode::Io,
            };
            ApplyError::capture(reason, error)
        })?;
    let identity = current.identity().ok_or_else(|| {
        ApplyError::capture(
            ReasonCode::VaultChanged,
            CaptureError::NotFound(output.path.clone()),
        )
    })?;
    if identity.nlink != 1 {
        return Err(ApplyError::capture(
            ReasonCode::UnsupportedFile,
            CaptureError::Unsupported {
                path: output.path.clone(),
                message: "refusing to replace a multiply linked file"
                    .to_string(),
            },
        ));
    }
    if identity != &output.identity {
        return Err(ApplyError::capture(
            ReasonCode::VaultChanged,
            CaptureError::Unstable(output.path.clone()),
        ));
    }
    if current.bytes() != Some(output.original_bytes.as_slice()) {
        return Err(ApplyError::capture(
            ReasonCode::VaultChanged,
            CaptureError::Unstable(output.path.clone()),
        ));
    }
    Ok(())
}
