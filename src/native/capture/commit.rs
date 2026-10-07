//! Staging, rollback, temporary files, and clipboard saves.
use super::*;

pub(crate) fn validate_target_parent(
    target: &Path,
) -> Result<(), CaptureError> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    if parent.is_dir() {
        Ok(())
    } else {
        Err(CaptureError::io(format!(
            "create target {}: Bob vault root does not exist: {}",
            target.display(),
            parent.display(),
        )))
    }
}

pub(super) fn commit_capture_batch(
    batch: &mut PlannedCaptureBatch,
) -> Result<(), CaptureError> {
    let created_clip_files = save_clip_plans(&batch.items)?;
    let created_jobs = match save_ref_job_plans(&batch.staged_jobs) {
        Ok(created) => created,
        Err(mut message) => {
            if !created_clip_files.is_empty() {
                let cleanup =
                    capture_clip::cleanup_created(&created_clip_files);
                capture_clip::append_cleanup_message(&mut message, &cleanup);
            }
            return Err(CaptureError::io(message));
        }
    };
    if let Err(mut error) = write_staged_files(&batch.text_files) {
        if !created_clip_files.is_empty() {
            let cleanup = capture_clip::cleanup_created(&created_clip_files);
            capture_clip::append_cleanup_message(&mut error.message, &cleanup);
        }
        if !created_jobs.is_empty() {
            let paths: Vec<PathBuf> =
                created_jobs.iter().map(|(_, path)| path.clone()).collect();
            let failures = ref_jobs::remove_created(&paths);
            append_ref_job_cleanup_message(&mut error.message, &failures);
        }
        return Err(error);
    }
    for (item, path) in created_jobs {
        let id = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        if let Some(planned) = batch.items.get_mut(item)
            && let Some(reference) = planned.result.r#ref.as_mut()
        {
            reference.job = Some(RefJobJson {
                id,
                state: "pending",
            });
        }
    }
    Ok(())
}

/// Enqueue every staged ref job, returning each batch position with
/// its created spool path. A failure removes the jobs this capture
/// already created, exactly as clipboard cleanup does.
pub(super) fn save_ref_job_plans(
    staged: &[StagedRefJob],
) -> Result<Vec<(usize, PathBuf)>, String> {
    let root = ref_jobs::jobs_dir();
    let mut created = Vec::new();
    for staged_job in staged {
        match ref_jobs::enqueue(&root, &staged_job.job) {
            Ok(path) => created.push((staged_job.item, path)),
            Err(message) => {
                if !created.is_empty() {
                    let paths: Vec<PathBuf> =
                        created.iter().map(|(_, path)| path.clone()).collect();
                    let failures = ref_jobs::remove_created(&paths);
                    let mut full = message;
                    append_ref_job_cleanup_message(&mut full, &failures);
                    return Err(full);
                }
                return Err(message);
            }
        }
    }
    Ok(created)
}

fn append_ref_job_cleanup_message(message: &mut String, failures: &[String]) {
    if failures.is_empty() {
        message.push_str("; removed ref jobs created by this capture");
    } else {
        message.push_str("; ref-job cleanup also failed: ");
        message.push_str(&failures.join("; "));
    }
}

pub(super) fn save_clip_plans(
    items: &[PlannedCaptureItem],
) -> Result<Vec<PathBuf>, CaptureError> {
    let mut created = Vec::new();
    for item in items {
        let Some(plan) = &item.clip_plan else {
            continue;
        };
        match plan.save() {
            Ok(paths) => created.extend(paths),
            Err(mut message) => {
                if !created.is_empty() {
                    let cleanup = capture_clip::cleanup_created(&created);
                    capture_clip::append_cleanup_message(
                        &mut message,
                        &cleanup,
                    );
                }
                return Err(CaptureError::io(message));
            }
        }
    }
    Ok(created)
}

pub(super) struct PendingTextFile<'a> {
    pub(super) staged: &'a StagedTextFile,
    pub(super) temporary: PathBuf,
    pub(super) backup: Option<PathBuf>,
}

pub(super) struct AppliedTextFile {
    pub(super) target: PathBuf,
    pub(super) backup: Option<PathBuf>,
    pub(super) target_existed: bool,
}

/// Optimistic disk-preimage guard for the shared capture batch writer.
///
/// Compares every planned target against its planning-time preimage
/// (`target_existed` / `original_target` from `batch.rs`). A missing
/// formerly existing target, different bytes, or a newly appeared target
/// that was absent at planning refuses the batch; other read errors
/// propagate. These checks narrow the external-edit race but are not
/// cross-process locking or a fully serialized transaction.
pub(super) fn validate_disk_preimages(
    files: &[StagedTextFile],
) -> Result<(), CaptureError> {
    for staged in files {
        if staged.target_existed {
            match fs::read(&staged.target) {
                Ok(bytes) => {
                    if bytes != staged.original_target.as_bytes() {
                        return Err(CaptureError::io(format!(
                            "refusing to overwrite {}: note changed on disk after planning",
                            staged.target.display()
                        )));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(CaptureError::io(format!(
                        "refusing to overwrite {}: note was deleted on disk after planning",
                        staged.target.display()
                    )));
                }
                Err(error) => {
                    return Err(fs_error("read target", &staged.target, error));
                }
            }
        } else {
            match fs::symlink_metadata(&staged.target) {
                Ok(_) => {
                    return Err(CaptureError::io(format!(
                        "refusing to overwrite {}: note was created on disk after planning",
                        staged.target.display()
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(fs_error("read target", &staged.target, error));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn write_staged_files(
    files: &[StagedTextFile],
) -> Result<(), CaptureError> {
    // Refuse a stale batch before creating any temporary files.
    validate_disk_preimages(files)?;
    let mut pending = Vec::new();
    for (index, staged) in files.iter().enumerate() {
        let role = format!("batch-{index}");
        let temporary = match write_temporary_file(
            &staged.target,
            &staged.updated_target,
            &role,
        ) {
            Ok(path) => path,
            Err(error) => {
                cleanup_pending_text_files(&pending);
                return Err(error);
            }
        };
        let backup = if staged.target_existed {
            match write_temporary_file(
                &staged.target,
                &staged.original_target,
                "backup",
            ) {
                Ok(path) => Some(path),
                Err(error) => {
                    remove_temporary_file(&temporary);
                    cleanup_pending_text_files(&pending);
                    return Err(error);
                }
            }
        } else {
            None
        };
        pending.push(PendingTextFile {
            staged,
            temporary,
            backup,
        });
    }

    // Revalidate after staging so an edit that landed while temporary
    // files were written still refuses before any target is replaced.
    if let Err(error) = validate_disk_preimages(files) {
        cleanup_pending_text_files(&pending);
        return Err(error);
    }

    let mut applied = Vec::new();
    while !pending.is_empty() {
        let pending_file = pending.remove(0);
        // Narrow the remaining race: recheck this target immediately
        // before replacing it.
        if let Err(error) =
            validate_disk_preimages(std::slice::from_ref(pending_file.staged))
        {
            remove_temporary_file(&pending_file.temporary);
            if let Some(backup) = &pending_file.backup {
                remove_temporary_file(backup);
            }
            cleanup_pending_text_files(&pending);
            let mut message = error.message.clone();
            append_rollback_message(
                &mut message,
                rollback_applied_files(&applied),
            );
            return Err(CaptureError::io(message));
        }
        if let Err(error) =
            fs::rename(&pending_file.temporary, &pending_file.staged.target)
        {
            remove_temporary_file(&pending_file.temporary);
            if let Some(backup) = &pending_file.backup {
                remove_temporary_file(backup);
            }
            cleanup_pending_text_files(&pending);
            let mut message = format!(
                "replace target {}: {error}",
                pending_file.staged.target.display()
            );
            append_rollback_message(
                &mut message,
                rollback_applied_files(&applied),
            );
            return Err(CaptureError::io(message));
        }
        applied.push(AppliedTextFile {
            target: pending_file.staged.target.clone(),
            backup: pending_file.backup,
            target_existed: pending_file.staged.target_existed,
        });
    }

    for file in &applied {
        if let Some(backup) = &file.backup {
            remove_temporary_file(backup);
        }
    }
    Ok(())
}

pub(super) fn cleanup_pending_text_files(files: &[PendingTextFile<'_>]) {
    for file in files {
        remove_temporary_file(&file.temporary);
        if let Some(backup) = &file.backup {
            remove_temporary_file(backup);
        }
    }
}

pub(super) fn rollback_applied_files(files: &[AppliedTextFile]) -> Vec<String> {
    let mut failures = Vec::new();
    for file in files.iter().rev() {
        let rollback = match &file.backup {
            Some(backup) => fs::rename(backup, &file.target),
            None if file.target_existed => Ok(()),
            None => fs::remove_file(&file.target),
        };
        if let Err(error) = rollback {
            let suffix = file
                .backup
                .as_ref()
                .map(|backup| {
                    format!("; original remains at {}", backup.display())
                })
                .unwrap_or_default();
            failures.push(format!(
                "rollback of {} failed: {error}{suffix}",
                file.target.display()
            ));
        }
    }
    failures
}

pub(super) fn append_rollback_message(
    message: &mut String,
    failures: Vec<String>,
) {
    if failures.is_empty() {
        message.push_str("; rolled back earlier note writes");
    } else {
        message.push_str("; ");
        message.push_str(&failures.join("; "));
    }
}

pub(super) fn paths_refer_to_same_file(first: &Path, second: &Path) -> bool {
    if first == second {
        return true;
    }
    match (fs::canonicalize(first), fs::canonicalize(second)) {
        (Ok(first), Ok(second)) => first == second,
        _ => false,
    }
}

pub(super) static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn write_temporary_file(
    destination: &Path,
    contents: &str,
    role: &str,
) -> Result<PathBuf, CaptureError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("note");

    for _ in 0..100 {
        let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{file_name}.bob-capture-{}-{sequence}-{role}.tmp",
            std::process::id()
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => {
                return Err(fs_error(
                    "create temporary file for",
                    destination,
                    error,
                ));
            }
        };
        if let Ok(metadata) = fs::metadata(destination)
            && let Err(error) = file.set_permissions(metadata.permissions())
        {
            remove_temporary_file(&path);
            return Err(fs_error(
                "set temporary file permissions for",
                destination,
                error,
            ));
        }
        if let Err(error) = file.write_all(contents.as_bytes()) {
            remove_temporary_file(&path);
            return Err(fs_error(
                "write temporary file for",
                destination,
                error,
            ));
        }
        if let Err(error) = file.sync_all() {
            remove_temporary_file(&path);
            return Err(fs_error(
                "sync temporary file for",
                destination,
                error,
            ));
        }
        return Ok(path);
    }

    Err(CaptureError::io(format!(
        "could not allocate temporary file for {}",
        destination.display()
    )))
}

pub(super) fn remove_temporary_file(path: &Path) {
    let _ = fs::remove_file(path);
}

pub(super) fn read_target(target: &Path) -> Result<String, CaptureError> {
    fs::read_to_string(target)
        .map_err(|error| fs_error("read target", target, error))
}

pub(super) fn fs_error(
    action: &str,
    path: &Path,
    error: io::Error,
) -> CaptureError {
    CaptureError::io(format!("{action} {}: {error}", path.display()))
}
