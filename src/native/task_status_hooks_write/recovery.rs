use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    model::{
        remaining_outputs, AppliedWrite, ApplyError, ApplySession, ReasonCode,
        WritePlan,
    },
    staging::{ensure_private_dir, sync_dir, write_private_file},
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct RecoveryManifest {
    pub(super) tool: String,
    schema_version: u32,
    vault: String,
    vault_hash: String,
    run_id: String,
    started_at: String,
    started_at_unix: u64,
    completed_at: Option<String>,
    completed_at_unix: Option<u64>,
    pub(super) outcome: String,
    pub(super) notes: Vec<RecoveryNote>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct RecoveryNote {
    path: String,
    pub(super) original_hash: String,
    pub(super) proposed_hash: String,
    pub(super) state: String,
    original_file: String,
    proposed_file: String,
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(super) fn vault_hash(canonical_vault: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        sha256_hex(canonical_vault.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        sha256_hex(canonical_vault.to_string_lossy().as_bytes())
    }
}

fn format_system_time(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("{}", duration.as_secs()),
        Err(_) => "0".to_string(),
    }
}

pub(super) fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(super) fn create_recovery(
    plan: &WritePlan,
    session: &ApplySession,
) -> Result<PathBuf, ApplyError> {
    let hash = vault_hash(&plan.vault_canonical);
    let recovery_dir = session
        .state_home
        .join("bob-cli")
        .join(session.tool)
        .join(hash)
        .join(&session.run_id);
    ensure_private_dir(&session.state_home.join("bob-cli").join(session.tool))
        .and_then(|_| {
            ensure_private_dir(recovery_dir.parent().unwrap_or(&recovery_dir))
        })
        .and_then(|_| ensure_private_dir(&recovery_dir))
        .map_err(|error| ApplyError {
            reason: ReasonCode::RecoveryFailed,
            message: format!(
                "failed to create recovery directory {}: {error}",
                recovery_dir.display()
            ),
            applied_files: Vec::new(),
            deferred_files: remaining_outputs(plan, &[]),
            recovery_directory: None,
        })?;

    for (index, output) in plan.outputs.iter().enumerate() {
        let original_name = format!("{index:04}.original");
        let proposed_name = format!("{index:04}.proposed");
        write_private_file(
            &recovery_dir.join(&original_name),
            &output.original_bytes,
        )
        .and_then(|_| {
            write_private_file(
                &recovery_dir.join(&proposed_name),
                &output.proposed_bytes,
            )
        })
        .map_err(|error| ApplyError {
            reason: ReasonCode::RecoveryFailed,
            message: format!(
                "failed to write recovery copies in {}: {error}",
                recovery_dir.display()
            ),
            applied_files: Vec::new(),
            deferred_files: remaining_outputs(plan, &[]),
            recovery_directory: None,
        })?;
    }

    write_manifest(&recovery_dir, plan, session, "planned", &[])
        .and_then(|_| sync_dir(&recovery_dir))
        .map_err(|error| ApplyError {
            reason: ReasonCode::RecoveryFailed,
            message: format!(
                "failed to persist recovery manifest {}: {error}",
                recovery_dir.display()
            ),
            applied_files: Vec::new(),
            deferred_files: remaining_outputs(plan, &[]),
            recovery_directory: None,
        })?;
    Ok(recovery_dir)
}

fn recovery_manifest(
    plan: &WritePlan,
    session: &ApplySession,
    outcome: &str,
    applied: &[AppliedWrite],
) -> RecoveryManifest {
    let now = session.now();
    let completed = matches!(outcome, "applied");
    RecoveryManifest {
        tool: session.tool.to_string(),
        schema_version: SCHEMA_VERSION,
        vault: plan.vault_canonical.display().to_string(),
        vault_hash: vault_hash(&plan.vault_canonical),
        run_id: session.run_id.clone(),
        started_at: format_system_time(now),
        started_at_unix: unix_secs(now),
        completed_at: completed.then(|| format_system_time(now)),
        completed_at_unix: completed.then(|| unix_secs(now)),
        outcome: outcome.to_string(),
        notes: plan
            .outputs
            .iter()
            .enumerate()
            .map(|(index, output)| {
                let state =
                    if applied.iter().any(|item| item.path == output.path)
                        || outcome == "applied"
                    {
                        "applied"
                    } else {
                        "planned"
                    };
                RecoveryNote {
                    path: output.path.display().to_string(),
                    original_hash: sha256_hex(&output.original_bytes),
                    proposed_hash: sha256_hex(&output.proposed_bytes),
                    state: state.to_string(),
                    original_file: format!("{index:04}.original"),
                    proposed_file: format!("{index:04}.proposed"),
                }
            })
            .collect(),
    }
}

fn write_manifest(
    recovery_dir: &Path,
    plan: &WritePlan,
    session: &ApplySession,
    outcome: &str,
    applied: &[AppliedWrite],
) -> io::Result<()> {
    let manifest = recovery_manifest(plan, session, outcome, applied);
    let encoded =
        serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    let temp =
        recovery_dir.join(format!("manifest.{}.json.tmp", session.run_id));
    write_private_file(&temp, &encoded)?;
    fs::rename(&temp, recovery_dir.join("manifest.json"))?;
    Ok(())
}

pub(super) fn update_manifest(
    recovery_dir: &Path,
    plan: &WritePlan,
    session: &ApplySession,
    outcome: &str,
    applied: &[AppliedWrite],
) -> io::Result<()> {
    write_manifest(recovery_dir, plan, session, outcome, applied)?;
    sync_dir(recovery_dir)
}

pub(super) fn prune_completed(
    root: &Path,
    tool: &str,
    now: SystemTime,
    retention: Duration,
) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for vault_entry in entries {
        let vault_entry = vault_entry?;
        if !vault_entry.file_type()?.is_dir() {
            continue;
        }
        let run_entries = match fs::read_dir(vault_entry.path()) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for run_entry in run_entries {
            let run_entry = run_entry?;
            if !run_entry.file_type()?.is_dir() {
                continue;
            }
            let manifest_path = run_entry.path().join("manifest.json");
            let Ok(text) = fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<RecoveryManifest>(&text)
            else {
                continue;
            };
            if manifest.tool != tool
                || manifest.schema_version != SCHEMA_VERSION
            {
                continue;
            }
            if manifest.outcome != "applied" {
                continue;
            }
            let Some(completed) = manifest.completed_at_unix else {
                continue;
            };
            let completed = UNIX_EPOCH + Duration::from_secs(completed);
            let expired = now
                .duration_since(completed)
                .map(|age| age >= retention)
                .unwrap_or(false);
            if expired {
                fs::remove_dir_all(run_entry.path())?;
            }
        }
    }
    Ok(())
}
