//! Shared companion-audio planning and install for Highlights routes.
//!
//! Every `create` route plans its companion the same way: `--audio PATH`
//! binds explicit audio, and an existing companion beside the target is
//! reused. Discovery (frontmatter episode id, narration hash) stays
//! Markdown-only. The copy-audio, install-PDF, delete-created-audio-on-
//! failure sequence lives here so every route shares it.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use super::{audio as audio_mod, Config};
use super::{CommandError, OsStr, Result};
use super::{TargetPlan, TargetWorkflow};

/// One planned companion-audio copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AudioCopyPlan {
    pub(super) source: PathBuf,
    pub(super) dest: PathBuf,
    pub(super) library_dest: Option<PathBuf>,
    pub(super) origin: String,
    pub(super) reused: bool,
}

/// Plan a copy from an already-resolved audio `source`.
pub(super) fn plan_audio_copy_for_source(
    config: &Config,
    target_plan: &TargetPlan,
    force: bool,
    source: PathBuf,
    origin: String,
) -> Result<Option<AudioCopyPlan>> {
    let _ = config;
    let extension = source
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_lowercase();
    if extension.is_empty() {
        return Err(CommandError::new(format!(
            "audio file has no extension: {}",
            source.display()
        )));
    }
    let dest = target_plan.target.with_extension(&extension);
    let library_dest = match &target_plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => Some(library_destination.with_extension(&extension)),
        _ => None,
    };
    if let Some(library_dest) = &library_dest
        && library_dest.exists()
    {
        return Err(CommandError::new(format!(
            "refusing to create {} because the library destination already exists: {}; remove or rename the archived copy before recreating it (bob highlights scan would refuse to move the new audio over it)",
            dest.display(),
            library_dest.display()
        )));
    }
    if dest.exists() {
        if files_have_identical_bytes(&source, &dest)? {
            return Ok(Some(AudioCopyPlan {
                source,
                dest,
                library_dest,
                origin,
                reused: true,
            }));
        }
        if !force {
            return Err(CommandError::new(format!(
                "target audio already exists: {}; pass --force to overwrite it",
                dest.display()
            )));
        }
        return Ok(Some(AudioCopyPlan {
            source,
            dest,
            library_dest,
            origin,
            reused: false,
        }));
    }
    Ok(Some(AudioCopyPlan {
        source,
        dest,
        library_dest,
        origin,
        reused: false,
    }))
}

/// Byte-compare two files (`source == dest` counts as identical).
pub(super) fn files_have_identical_bytes(
    source: &Path,
    dest: &Path,
) -> Result<bool> {
    if source == dest {
        return Ok(true);
    }
    let source_bytes = fs::read(source).map_err(|error| {
        CommandError::new(format!(
            "read {} for audio comparison: {error}",
            source.display()
        ))
    })?;
    let dest_bytes = fs::read(dest).map_err(|error| {
        CommandError::new(format!(
            "read {} for audio comparison: {error}",
            dest.display()
        ))
    })?;
    Ok(source_bytes == dest_bytes)
}

/// Resolve `--audio PATH` (tilde, cwd, existence, extension) and plan it.
pub(super) fn plan_explicit_audio(
    config: &Config,
    target_plan: &TargetPlan,
    force: bool,
    explicit: &Path,
) -> Result<Option<AudioCopyPlan>> {
    let resolved = resolve_audio_arg_path(explicit)?;
    let source = audio_mod::validate_explicit_audio(&resolved)
        .map(|path| fs::canonicalize(&path).unwrap_or(path))?;
    plan_audio_copy_for_source(
        config,
        target_plan,
        force,
        source,
        "--audio".to_string(),
    )
}

fn resolve_audio_arg_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(CommandError::new(
            "audio path must include a nonempty filename",
        ));
    }
    let expanded = super::bob_env::expand_tilde(path);
    if expanded.is_absolute() {
        return Ok(expanded);
    }
    let cwd = env::current_dir().map_err(|error| {
        CommandError::new(format!("resolve current directory: {error}"))
    })?;
    Ok(cwd.join(expanded))
}

/// Reuse an existing companion beside the target (and optionally beside
/// one extra path, e.g. the Markdown or PDF source). Returns `None` when
/// no companion exists.
pub(super) fn plan_reused_companion(
    target_plan: &TargetPlan,
    extra_beside: Option<&Path>,
) -> Result<Option<AudioCopyPlan>> {
    let existing = audio_mod::audio_beside(&target_plan.target)
        .or_else(|| extra_beside.and_then(audio_mod::audio_beside));
    let Some(existing) = existing else {
        return Ok(None);
    };
    let extension = existing
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_lowercase();
    let dest = target_plan.target.with_extension(&extension);
    let library_dest = match &target_plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => Some(library_destination.with_extension(&extension)),
        _ => None,
    };
    if let Some(library_dest) = &library_dest
        && library_dest.exists()
        && existing != *library_dest
    {
        return Err(CommandError::new(format!(
            "refusing to create {} because the library destination already exists: {}; remove or rename the archived copy before recreating it (bob highlights scan would refuse to move the new audio over it)",
            dest.display(),
            library_dest.display()
        )));
    }
    Ok(Some(AudioCopyPlan {
        source: existing.clone(),
        dest,
        library_dest,
        origin: "existing companion".to_string(),
        reused: true,
    }))
}

/// Copy companion audio before the PDF install. Returns the created path
/// (for cleanup) when a copy happened.
pub(super) fn copy_audio_for_install(
    audio: Option<&AudioCopyPlan>,
) -> Result<Option<PathBuf>> {
    let Some(plan) = audio else {
        return Ok(None);
    };
    if plan.reused {
        return Ok(None);
    }
    super::atomic_copy(&plan.source, &plan.dest)?;
    Ok(Some(plan.dest.clone()))
}

/// Delete an audio file this run created after a failed PDF install.
pub(super) fn cleanup_audio_on_failure(created: Option<&PathBuf>) {
    if let Some(path) = created {
        let _ = fs::remove_file(path);
    }
}
