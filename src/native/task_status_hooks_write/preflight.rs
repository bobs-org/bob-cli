use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    model::{
        remaining_outputs, AppliedWrite, ApplyError, ApplySession,
        CaptureError, InputKind, InputSnapshot, InputState, PlannedWrite,
        ReasonCode, StagedWrite, WritePlan,
    },
    snapshot::{capture_optional, capture_required, recapture},
    staging::inspect_destination_parent,
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
        if output.is_create() {
            continue;
        }
        let metadata = fs::symlink_metadata(&output.path).map_err(|error| {
            ApplyError::quiet_period(
                plan,
                format!("could not read {}: {error}", output.path.display()),
            )
        })?;
        let mtime = metadata
            .modified()
            .ok()
            .or_else(|| output.identity().and_then(|identity| identity.mtime));
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
        if let Some(applied_item) = applied.iter().find(|item| {
            item.path == input.path
                || fs::canonicalize(&item.path).ok().is_some_and(
                    |applied_canonical| {
                        input.identity().is_some_and(|identity| {
                            identity.canonical_path == applied_canonical
                        })
                    },
                )
        }) {
            check_applied_input(plan, applied_item, input, &applied_paths)?;
            continue;
        }
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
        if !input.matches(&current) {
            return Err(ApplyError::vault_changed(
                plan,
                &applied_paths,
                None,
                format!("{} changed", input.path.display()),
            ));
        }
    }

    check_membership(plan, session, applied)?;

    for output in &plan.outputs {
        if applied.iter().any(|item| item.path == output.path) {
            continue;
        }
        inspect_destination_parent(
            &output.path,
            &plan.vault_path,
            &plan.vault_canonical,
            false,
        )
        .map_err(|error| {
            let reason = match error {
                CaptureError::Unsupported { .. } => ReasonCode::UnsupportedFile,
                _ => ReasonCode::Io,
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
        check_unwritten_output(output).map_err(|error| {
            if applied.is_empty() {
                error
            } else {
                ApplyError::vault_changed(
                    plan,
                    &applied_paths,
                    None,
                    error.message.clone(),
                )
            }
        })?;
    }
    Ok(())
}

pub(super) fn preflight_for_replacement(
    plan: &WritePlan,
    session: &ApplySession,
    next: &StagedWrite,
    applied: &[AppliedWrite],
) -> Result<(), ApplyError> {
    let applied_paths = applied
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    for item in applied {
        check_applied_output(plan, item, &applied_paths)?;
    }
    check_membership(plan, session, applied)?;
    let output = plan
        .outputs
        .iter()
        .find(|output| output.path == next.dest)
        .expect("staged path is part of the plan");
    inspect_destination_parent(
        &output.path,
        &plan.vault_path,
        &plan.vault_canonical,
        false,
    )
    .map_err(|error| {
        let reason = match error {
            CaptureError::Unsupported { .. } => ReasonCode::UnsupportedFile,
            _ => ReasonCode::Io,
        };
        ApplyError::capture(reason, error)
    })?;
    check_unwritten_output(output)
}

fn check_membership(
    plan: &WritePlan,
    session: &ApplySession,
    applied: &[AppliedWrite],
) -> Result<(), ApplyError> {
    let applied_paths = applied
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    let scanned = (session.rescan)().map_err(|error| {
        ApplyError::io(
            format!("failed to rescan vault: {error}"),
            applied_paths.clone(),
            remaining_outputs(plan, &applied_paths),
            None,
        )
    })?;
    let original: BTreeSet<PathBuf> = plan.scan_paths.iter().cloned().collect();
    let current: BTreeSet<PathBuf> = scanned.into_iter().collect();
    let applied_creates: BTreeSet<PathBuf> = applied
        .iter()
        .filter(|item| {
            plan.outputs
                .iter()
                .any(|output| output.path == item.path && output.is_create())
        })
        .map(|item| item.path.clone())
        .collect();
    let applied_removes: BTreeSet<PathBuf> = applied
        .iter()
        .filter(|item| item.is_remove())
        .map(|item| item.path.clone())
        .collect();
    let unplanned_addition = current
        .difference(&original)
        .any(|path| !applied_creates.contains(path));
    let unplanned_removal = original
        .difference(&current)
        .any(|path| !applied_removes.contains(path));
    let reappeared = applied_removes.iter().any(|path| current.contains(path));
    if unplanned_addition || unplanned_removal || reappeared {
        let detail = if reappeared {
            "a removed note reappeared"
        } else {
            "the set of scanned notes changed"
        };
        return Err(ApplyError::vault_changed(
            plan,
            &applied_paths,
            None,
            detail,
        ));
    }
    Ok(())
}

fn check_applied_input(
    plan: &WritePlan,
    applied_item: &AppliedWrite,
    input: &InputSnapshot,
    applied_paths: &[PathBuf],
) -> Result<(), ApplyError> {
    check_applied_path(plan, applied_item, &input.path, applied_paths)
}

fn check_applied_output(
    plan: &WritePlan,
    applied_item: &AppliedWrite,
    applied_paths: &[PathBuf],
) -> Result<(), ApplyError> {
    check_applied_path(plan, applied_item, &applied_item.path, applied_paths)
}

fn check_applied_path(
    plan: &WritePlan,
    applied_item: &AppliedWrite,
    path: &Path,
    applied_paths: &[PathBuf],
) -> Result<(), ApplyError> {
    if applied_item.is_remove() {
        let current =
            capture_optional(path, InputKind::Note).map_err(|error| {
                ApplyError::vault_changed(
                    plan,
                    applied_paths,
                    None,
                    error.message(),
                )
            })?;
        if !current.is_missing() {
            return Err(ApplyError::vault_changed(
                plan,
                applied_paths,
                None,
                format!(
                    "already-removed {} reappeared",
                    applied_item.path.display()
                ),
            ));
        }
        return Ok(());
    }
    let current = capture_required(path, InputKind::Note).map_err(|error| {
        ApplyError::vault_changed(plan, applied_paths, None, error.message())
    })?;
    if current.bytes() != applied_item.proposed_bytes.as_deref() {
        return Err(ApplyError::vault_changed(
            plan,
            applied_paths,
            None,
            format!(
                "already-written {} changed after replacement",
                applied_item.path.display()
            ),
        ));
    }
    Ok(())
}

fn check_unwritten_output(output: &PlannedWrite) -> Result<(), ApplyError> {
    if output.is_create() {
        let current = capture_optional(&output.path, InputKind::Note).map_err(
            |error| {
                let reason = match error {
                    CaptureError::Unstable(_) => ReasonCode::UnstableRead,
                    CaptureError::Unsupported { .. } => {
                        ReasonCode::UnsupportedFile
                    }
                    _ => ReasonCode::Io,
                };
                ApplyError::capture(reason, error)
            },
        )?;
        if !current.is_missing() {
            return Err(ApplyError::capture(
                ReasonCode::VaultChanged,
                CaptureError::Unstable(output.path.clone()),
            ));
        }
        return Ok(());
    }

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
    let planned_identity = match &output.original {
        InputState::Present { identity, .. } => identity,
        InputState::Missing => {
            return Err(ApplyError::capture(
                ReasonCode::VaultChanged,
                CaptureError::NotFound(output.path.clone()),
            ));
        }
    };
    if identity.nlink != 1 {
        let verb = if output.is_remove() {
            "remove"
        } else {
            "replace"
        };
        return Err(ApplyError::capture(
            ReasonCode::UnsupportedFile,
            CaptureError::Unsupported {
                path: output.path.clone(),
                message: format!("refusing to {verb} a multiply linked file"),
            },
        ));
    }
    if identity != planned_identity {
        return Err(ApplyError::capture(
            ReasonCode::VaultChanged,
            CaptureError::Unstable(output.path.clone()),
        ));
    }
    if current.bytes() != Some(output.original_bytes()) {
        return Err(ApplyError::capture(
            ReasonCode::VaultChanged,
            CaptureError::Unstable(output.path.clone()),
        ));
    }
    Ok(())
}
