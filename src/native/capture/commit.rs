//! Staging, rollback, temporary files, and clipboard saves.
use super::*;

#[cfg(test)]
thread_local! {
    pub(super) static RENAME_FAIL_TARGET: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
    pub(super) static PLANT_BEFORE_INSTALL: std::cell::RefCell<Option<(PathBuf, String)>> =
        const { std::cell::RefCell::new(None) };
    pub(super) static FORCE_HARD_LINK_UNSUPPORTED: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn test_rename_failure(target: &Path) -> Option<CaptureError> {
    if RENAME_FAIL_TARGET.with(|fail_target| {
        fail_target
            .borrow()
            .as_ref()
            .is_some_and(|fail| fail == target)
    }) {
        return Some(CaptureError::io(format!(
            "replace target {}: injected test failure",
            target.display()
        )));
    }
    None
}

#[cfg(test)]
fn test_plant_before_install(target: &Path) {
    PLANT_BEFORE_INSTALL.with(|plant| {
        if let Some((path, bytes)) = plant.borrow().as_ref()
            && path == target
        {
            fs::write(target, bytes)
                .expect("plant foreign file before install");
        }
    });
}

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
    bob_dir: &Path,
) -> Result<(), CaptureError> {
    batch.read_guards.validate(bob_dir)?;
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
    if let Err(mut error) =
        write_staged_files_guarded(&batch.text_files, || {
            batch.read_guards.validate(bob_dir)
        })
    {
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
    crate::native::relation_labels::invalidate_scan(bob_dir);
    crate::native::capture_drawers::invalidate_drawer_catalog(bob_dir);
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
                        ))
                        .with_typed(CaptureFailureKind::NoteChanged));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(CaptureError::io(format!(
                        "refusing to overwrite {}: note was deleted on disk after planning",
                        staged.target.display()
                    ))
                    .with_typed(CaptureFailureKind::NoteChanged));
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
                    ))
                    .with_typed(CaptureFailureKind::NoteChanged));
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

/// A non-race I/O failure while validating a read dependency.
pub(crate) fn read_failed(message: impl Into<String>) -> CaptureError {
    CaptureError::io(message)
}

/// A read dependency or preimage changed on disk after planning.
pub(crate) fn changed_after_planning(
    message: impl Into<String>,
) -> CaptureError {
    CaptureError::io(message).with_typed(CaptureFailureKind::NoteChanged)
}

pub(crate) fn write_staged_files(
    files: &[StagedTextFile],
) -> Result<(), CaptureError> {
    write_staged_files_guarded(files, || Ok(()))
}

/// Install staged files like [`write_staged_files`], also running
/// `validate_reads` before staging and again before any target is replaced,
/// so read-only inputs (ancestors, notes that are not rewritten) are guarded
/// at both validation points.
pub(crate) fn write_staged_files_guarded(
    files: &[StagedTextFile],
    validate_reads: impl Fn() -> Result<(), CaptureError>,
) -> Result<(), CaptureError> {
    validate_reads()?;
    // Refuse a stale batch before creating any temporary files.
    validate_disk_preimages(files)?;
    let created_dirs = ensure_new_file_parents(files)?;
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
                rollback_created_dirs(&created_dirs);
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
                    rollback_created_dirs(&created_dirs);
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
    if let Err(error) =
        validate_reads().and_then(|()| validate_disk_preimages(files))
    {
        cleanup_pending_text_files(&pending);
        rollback_created_dirs(&created_dirs);
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
            return Err(fail_after_partial_apply(
                error,
                pending_file,
                &pending,
                &applied,
                &created_dirs,
            ));
        }
        #[cfg(test)]
        if let Some(error) = test_rename_failure(&pending_file.staged.target) {
            return Err(fail_after_partial_apply(
                error,
                pending_file,
                &pending,
                &applied,
                &created_dirs,
            ));
        }
        #[cfg(test)]
        test_plant_before_install(&pending_file.staged.target);
        if let Err(error) =
            install_staged_target(&pending_file.temporary, pending_file.staged)
        {
            return Err(fail_after_partial_apply(
                error,
                pending_file,
                &pending,
                &applied,
                &created_dirs,
            ));
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

fn ensure_new_file_parents(
    files: &[StagedTextFile],
) -> Result<Vec<PathBuf>, CaptureError> {
    let mut created = Vec::new();
    for staged in files {
        if staged.target_existed {
            continue;
        }
        let Some(parent) = staged.target.parent() else {
            continue;
        };
        if parent.as_os_str().is_empty() || parent.exists() {
            continue;
        }
        let mut missing = Vec::new();
        let mut cursor = parent;
        loop {
            if cursor.exists() {
                break;
            }
            missing.push(cursor.to_path_buf());
            match cursor.parent() {
                Some(next) if next != cursor => cursor = next,
                _ => break,
            }
        }
        missing.reverse();
        for dir in &missing {
            match fs::create_dir(dir) {
                Ok(()) => created.push(dir.clone()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    rollback_created_dirs(&created);
                    return Err(fs_error(
                        "create parent directory for",
                        &staged.target,
                        error,
                    ));
                }
            }
        }
    }
    Ok(created)
}

fn rollback_created_dirs(created: &[PathBuf]) {
    for dir in created.iter().rev() {
        let _ = fs::remove_dir(dir);
    }
}

fn fail_after_partial_apply(
    error: CaptureError,
    pending_file: PendingTextFile<'_>,
    pending: &[PendingTextFile<'_>],
    applied: &[AppliedTextFile],
    created_dirs: &[PathBuf],
) -> CaptureError {
    remove_temporary_file(&pending_file.temporary);
    if let Some(backup) = &pending_file.backup {
        remove_temporary_file(backup);
    }
    cleanup_pending_text_files(pending);
    let mut message = error.message.clone();
    append_rollback_message(&mut message, rollback_applied_files(applied));
    rollback_created_dirs(created_dirs);
    let mut wrapped = CaptureError::io(message);
    if let Some(typed) = error.typed {
        wrapped = wrapped.with_typed(typed);
    }
    wrapped
}

fn created_after_planning(target: &Path) -> CaptureError {
    CaptureError::io(format!(
        "refusing to overwrite {}: note was created on disk after planning",
        target.display()
    ))
    .with_typed(CaptureFailureKind::NoteChanged)
}

pub(super) fn hard_link_unsupported(error: &io::Error) -> bool {
    match error.kind() {
        io::ErrorKind::Unsupported
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::CrossesDevices => true,
        _ => matches!(
            error.raw_os_error(),
            Some(libc::EPERM | libc::EXDEV | libc::ENOTSUP)
        ),
    }
}

fn install_staged_target(
    temporary: &Path,
    staged: &StagedTextFile,
) -> Result<(), CaptureError> {
    if staged.target_existed {
        fs::rename(temporary, &staged.target).map_err(|error| {
            CaptureError::io(format!(
                "replace target {}: {error}",
                staged.target.display()
            ))
        })
    } else {
        install_new_file(temporary, &staged.target, &staged.updated_target)
    }
}

/// Install a newly created note without replacing a path that appeared
/// after planning. Prefer `hard_link(temp, target)` then unlink the temp;
/// when hard links are unsupported, fall back to `create_new`.
fn install_new_file(
    temporary: &Path,
    target: &Path,
    contents: &str,
) -> Result<(), CaptureError> {
    #[cfg(test)]
    if FORCE_HARD_LINK_UNSUPPORTED.with(std::cell::Cell::get) {
        let result = install_new_file_create_new(target, contents);
        remove_temporary_file(temporary);
        return result;
    }
    match fs::hard_link(temporary, target) {
        Ok(()) => {
            remove_temporary_file(temporary);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(created_after_planning(target))
        }
        Err(error) if hard_link_unsupported(&error) => {
            let result = install_new_file_create_new(target, contents);
            remove_temporary_file(temporary);
            result
        }
        Err(error) => Err(fs_error("install new file", target, error)),
    }
}

fn install_new_file_create_new(
    target: &Path,
    contents: &str,
) -> Result<(), CaptureError> {
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(created_after_planning(target));
        }
        Err(error) => {
            return Err(fs_error("install new file", target, error));
        }
    };
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(target);
        return Err(fs_error("install new file", target, error));
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
