//! The host-catalog transaction: guarded install plus one scoped commit.
//!
//! [`apply_and_commit`] is the lock-free core. It never takes the
//! maintenance lock and never runs a vault-sync cycle, so nightly (which
//! already holds the lock) and a later capture transaction can compose it.
//! [`run_locked`] wraps it for standalone commands with the same lifecycle
//! as `vault_rewrite::run`: vault identity, bounded lock wait, worktree
//! check, pre-sync, plan, guarded write, scoped commit, post-sync. It does
//! not copy that module's rename/journal assumptions: installs go through the
//! shared capture staged-file writer (disk-preimage checks, no-replace
//! creation, rollback), and recovery never restores over a later edit.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use super::FileEdit;
use crate::native::{
    capture::{
        changed_after_planning, read_failed, write_staged_files_guarded,
        CaptureError, CaptureFailureKind, StagedTextFile,
    },
    collect_done,
    ob::{self, ChildEnv, LockWaitError},
    style::Styler,
    vault_rewrite::{
        committed_despite_error, git_head, unstage_paths, TxnFailure,
    },
    vault_sync,
};

/// Bounded wait for `bob_sync.lock` before a standalone write.
const LOCK_WAIT: Duration = Duration::from_secs(3);

/// Files a plan read but does not rewrite, with the exact bytes it saw
/// (`None` means the path must still be absent). Validated at both guard
/// points of the shared writer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReadDependencies {
    files: BTreeMap<PathBuf, Option<Vec<u8>>>,
    frontmatter: BTreeMap<PathBuf, Option<String>>,
    directories: BTreeMap<PathBuf, Option<Vec<PathBuf>>>,
    directory_entries: BTreeMap<PathBuf, (Option<String>, Vec<String>)>,
}

impl ReadDependencies {
    pub(crate) fn insert(&mut self, path: PathBuf, expected: Option<String>) {
        self.insert_bytes(path, expected.map(String::into_bytes));
    }

    pub(crate) fn insert_bytes(
        &mut self,
        path: PathBuf,
        expected: Option<Vec<u8>>,
    ) {
        self.files.insert(path, expected);
    }

    /// Guard only a Markdown note's frontmatter projection. This keeps
    /// maintenance indexes independent of unrelated record bodies.
    pub(crate) fn insert_frontmatter(
        &mut self,
        path: PathBuf,
        expected: Option<String>,
    ) {
        self.frontmatter.insert(path, expected);
    }

    /// Guard the presence and direct Markdown membership of one folder.
    /// `None` means that the folder itself was absent when planned.
    pub(crate) fn insert_directory_membership(
        &mut self,
        path: PathBuf,
        expected: Option<Vec<PathBuf>>,
    ) {
        self.directories.insert(path, expected);
    }

    /// Guard the names in a directory when a plan depends on the set of
    /// files it contains (for example bank months or host journals).
    pub(crate) fn insert_directory(
        &mut self,
        path: PathBuf,
        extension: Option<&str>,
        mut expected: Vec<String>,
    ) {
        expected.sort();
        expected.dedup();
        self.directory_entries
            .insert(path, (extension.map(str::to_owned), expected));
    }

    /// Fold another plan's read inputs into this one. Later entries for the
    /// same path replace earlier ones.
    pub(crate) fn extend(&mut self, other: Self) {
        self.files.extend(other.files);
        self.frontmatter.extend(other.frontmatter);
        self.directories.extend(other.directories);
        self.directory_entries.extend(other.directory_entries);
    }

    pub(crate) fn iter(
        &self,
    ) -> impl Iterator<Item = (&Path, Option<&[u8]>)> + '_ {
        self.files
            .iter()
            .map(|(path, contents)| (path.as_path(), contents.as_deref()))
    }

    pub(crate) fn validate(&self, vault: &Path) -> Result<(), CaptureError> {
        for (relative, expected) in &self.files {
            let absolute = vault.join(relative);
            match (fs::read(&absolute), expected) {
                (Ok(bytes), Some(expected))
                    if bytes.as_slice() == expected.as_slice() => {}
                (Ok(_), Some(_)) => {
                    return Err(changed_after_planning(format!(
                        "{} changed on disk after planning",
                        relative.display()
                    )));
                }
                (Ok(_), None) => {
                    return Err(changed_after_planning(format!(
                        "{} appeared on disk after planning",
                        relative.display()
                    )));
                }
                (Err(error), Some(_))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Err(changed_after_planning(format!(
                        "{} was deleted after planning",
                        relative.display()
                    )));
                }
                (Err(error), None)
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                (Err(error), _) => {
                    return Err(read_failed(format!(
                        "read {}: {error}",
                        absolute.display()
                    )));
                }
            }
        }
        for (relative, expected) in &self.frontmatter {
            let absolute = vault.join(relative);
            let current =
                match crate::native::records::catalog::read_frontmatter_head(
                    &absolute,
                ) {
                    Ok(current) => current,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        return Err(changed_after_planning(format!(
                            "{} was deleted after planning",
                            relative.display()
                        )));
                    }
                    Err(error) => {
                        return Err(read_failed(format!(
                            "read frontmatter {}: {error}",
                            absolute.display()
                        )));
                    }
                };
            if &current != expected {
                return Err(changed_after_planning(format!(
                    "{} frontmatter changed on disk after planning",
                    relative.display()
                )));
            }
        }
        for (relative, expected) in &self.directories {
            let current = markdown_membership(vault, relative)?;
            if &current != expected {
                return Err(changed_after_planning(format!(
                    "Markdown membership in {} changed on disk after planning",
                    relative.display()
                )));
            }
        }
        for (relative, (extension, expected)) in &self.directory_entries {
            let absolute = vault.join(relative);
            let mut actual = match fs::read_dir(&absolute) {
                Ok(entries) => entries
                    .filter_map(|entry| match entry {
                        Ok(entry) => {
                            let name = entry
                                .file_name()
                                .to_string_lossy()
                                .into_owned();
                            let matches =
                                extension.as_ref().is_none_or(|extension| {
                                    Path::new(&name).extension().is_some_and(
                                        |actual| actual == extension.as_str(),
                                    )
                                });
                            matches.then_some(Ok(name))
                        }
                        Err(error) => Some(Err(error)),
                    })
                    .map(|entry| {
                        entry.map_err(|error| {
                            read_failed(format!(
                                "read directory {}: {error}",
                                absolute.display()
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Vec::new()
                }
                Err(error) => {
                    return Err(read_failed(format!(
                        "read directory {}: {error}",
                        absolute.display()
                    )));
                }
            };
            actual.sort();
            if &actual != expected {
                return Err(changed_after_planning(format!(
                    "directory {} changed on disk after planning",
                    relative.display()
                )));
            }
        }
        Ok(())
    }
}

fn markdown_membership(
    vault: &Path,
    relative: &Path,
) -> Result<Option<Vec<PathBuf>>, CaptureError> {
    let directory = vault.join(relative);
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(read_failed(format!(
                "inspect record folder {}: {error}",
                directory.display()
            )));
        }
    };
    if !metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Err(changed_after_planning(format!(
            "{} is no longer a directory",
            relative.display()
        )));
    }
    let vault_canon = fs::canonicalize(vault).map_err(|error| {
        read_failed(format!("resolve vault {}: {error}", vault.display()))
    })?;
    let directory_canon = fs::canonicalize(&directory).map_err(|error| {
        read_failed(format!(
            "resolve record folder {}: {error}",
            directory.display()
        ))
    })?;
    if !directory_canon.starts_with(&vault_canon) {
        return Err(changed_after_planning(format!(
            "record folder {} resolves outside the vault",
            relative.display()
        )));
    }
    let entries = fs::read_dir(&directory).map_err(|error| {
        read_failed(format!(
            "list record folder {}: {error}",
            directory.display()
        ))
    })?;
    let mut members = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            read_failed(format!(
                "read an entry in record folder {}: {error}",
                directory.display()
            ))
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        if !stem.is_empty() && extension.eq_ignore_ascii_case("md") {
            members.push(relative.join(name));
        }
    }
    members.sort();
    Ok(Some(members))
}

/// One ordered append to a journal file. `original_target` is the exact byte
/// preimage (`None` means the path must still be absent); `suffix` is one or
/// more already-serialized newline-terminated events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedAppend {
    pub(crate) target: PathBuf,
    pub(crate) original_target: Option<Vec<u8>>,
    pub(crate) suffix: Vec<u8>,
}

/// One planned vault write: staged files, the read inputs guarded beside
/// them, the single scoped commit subject, and a post-install verifier.
pub(crate) struct PlannedTxn<T> {
    pub(crate) files: Vec<StagedTextFile>,
    pub(crate) appends: Vec<PlannedAppend>,
    pub(crate) deps: ReadDependencies,
    pub(crate) subject: String,
    pub(crate) verify: Box<dyn FnOnce() -> Result<(), String>>,
    pub(crate) context: T,
    /// Noun used in install/verify error messages, e.g. `"notebook"`.
    pub(crate) kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitInfo {
    pub(crate) sha: String,
    pub(crate) subject: String,
}

/// What a finished transaction did. `commit` is `None` for a no-op plan.
#[derive(Debug)]
pub(crate) struct TxnResult<T> {
    pub(crate) commit: Option<CommitInfo>,
    pub(crate) sync_pre: &'static str,
    pub(crate) sync_post: &'static str,
    pub(crate) post_sync_warning: Option<String>,
    pub(crate) context: T,
}

fn failure(
    code: &'static str,
    message: impl Into<String>,
    hint: &str,
) -> TxnFailure {
    TxnFailure::new(code, message, Some(hint.to_string()))
}

fn relative_of(vault: &Path, target: &Path) -> PathBuf {
    target.strip_prefix(vault).unwrap_or(target).to_path_buf()
}

/// Stage each host rewrite as an in-place update of an existing file.
pub(crate) fn staged_rewrites<'a>(
    vault: &Path,
    edits: impl IntoIterator<Item = &'a FileEdit>,
) -> Vec<StagedTextFile> {
    edits
        .into_iter()
        .map(|edit| StagedTextFile {
            target: vault.join(&edit.path),
            target_existed: true,
            original_target: edit.before.clone(),
            updated_target: edit.after.clone(),
        })
        .collect()
}

/// Restore each staged file's before-image only while it still holds our
/// after-image; a later edit is never overwritten. Returns the paths left
/// in place because they diverged or could not be restored.
fn rollback_guarded(files: &[StagedTextFile]) -> Vec<String> {
    let mut left = Vec::new();
    for staged in files {
        let current = fs::read(&staged.target).ok();
        if current.as_deref() != Some(staged.updated_target.as_bytes()) {
            if current.is_some() || staged.target_existed {
                left.push(format!(
                    "{} changed since the write and was left in place",
                    staged.target.display()
                ));
            }
            continue;
        }
        let result = if staged.target_existed {
            collect_done::atomic_write(&staged.target, &staged.original_target)
        } else {
            fs::remove_file(&staged.target)
        };
        if let Err(error) = result {
            left.push(format!(
                "could not restore {}: {error}",
                staged.target.display()
            ));
        }
    }
    left
}

fn rollback_note(left: &[String]) -> String {
    if left.is_empty() {
        "; the write was rolled back".to_string()
    } else {
        format!("; rollback issues: {}", left.join("; "))
    }
}

#[derive(Debug)]
struct AppliedAppend {
    target: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

static APPEND_TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn append_postimage(before: Option<&[u8]>, suffix: &[u8]) -> Vec<u8> {
    let mut after = Vec::with_capacity(
        before.map_or(0, <[u8]>::len).saturating_add(suffix.len()),
    );
    if let Some(before) = before {
        after.extend_from_slice(before);
    }
    after.extend_from_slice(suffix);
    after
}

/// Open an append target without replacing a planted file, then verify the
/// bytes through the opened handle before adding anything.
fn open_append_target(
    path: &Path,
    expected: Option<&[u8]>,
) -> Result<File, String> {
    match expected {
        Some(_) => {
            let mut file = OpenOptions::new()
                .read(true)
                .append(true)
                .open(path)
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        format!(
                            "{} was deleted on disk after planning",
                            path.display()
                        )
                    } else {
                        format!("open {}: {error}", path.display())
                    }
                })?;
            file.seek(SeekFrom::Start(0))
                .map_err(|error| format!("seek {}: {error}", path.display()))?;
            let mut actual = Vec::new();
            file.read_to_end(&mut actual)
                .map_err(|error| format!("read {}: {error}", path.display()))?;
            if Some(actual.as_slice()) != expected {
                return Err(format!(
                    "{} changed on disk after planning",
                    path.display()
                ));
            }
            Ok(file)
        }
        None => {
            let mut options = OpenOptions::new();
            options.read(true).append(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o644);
            }
            options.open(path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    format!(
                        "{} appeared on disk after planning",
                        path.display()
                    )
                } else {
                    format!("create {}: {error}", path.display())
                }
            })
        }
    }
}

fn read_append_target(file: &mut File, path: &Path) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("seek {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(bytes)
}

/// Append one planned suffix. A failed short write is rolled back only when
/// the file still consists of its exact preimage plus the bytes this call
/// wrote; a concurrent edit is preserved and reported.
fn append_guarded(append: &PlannedAppend) -> Result<AppliedAppend, String> {
    let path = &append.target;
    if append.suffix.is_empty() {
        return Ok(AppliedAppend {
            target: path.clone(),
            before: append.original_target.clone(),
            after: append.original_target.clone().unwrap_or_default(),
        });
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let mut file = open_append_target(path, append.original_target.as_deref())?;
    let mut written = 0usize;
    while written < append.suffix.len() {
        match file.write(&append.suffix[written..]) {
            Ok(0) => break,
            Ok(count) => written += count,
            Err(error) => {
                let detail = rollback_partial_append(
                    file,
                    path,
                    append.original_target.as_deref(),
                    &append.suffix[..written],
                );
                return Err(format!(
                    "append {}: {error}{detail}",
                    path.display()
                ));
            }
        }
    }
    if written != append.suffix.len() {
        let detail = rollback_partial_append(
            file,
            path,
            append.original_target.as_deref(),
            &append.suffix[..written],
        );
        return Err(format!("append {}: short write{detail}", path.display()));
    }
    if let Err(error) = file.flush() {
        let detail = rollback_partial_append(
            file,
            path,
            append.original_target.as_deref(),
            &append.suffix,
        );
        return Err(format!("flush {}: {error}{detail}", path.display()));
    }
    let actual = match read_append_target(&mut file, path) {
        Ok(actual) => actual,
        Err(error) => {
            let detail = rollback_partial_append(
                file,
                path,
                append.original_target.as_deref(),
                &append.suffix,
            );
            return Err(format!("{error}{detail}"));
        }
    };
    let expected_after =
        append_postimage(append.original_target.as_deref(), &append.suffix);
    if actual != expected_after {
        let detail = rollback_partial_append(
            file,
            path,
            append.original_target.as_deref(),
            &append.suffix,
        );
        return Err(format!(
            "{} changed while appending{detail}",
            path.display()
        ));
    }
    Ok(AppliedAppend {
        target: path.clone(),
        before: append.original_target.clone(),
        after: expected_after,
    })
}

fn rollback_partial_append(
    mut file: File,
    path: &Path,
    before: Option<&[u8]>,
    written_suffix: &[u8],
) -> String {
    let expected = append_postimage(before, written_suffix);
    match read_append_target(&mut file, path) {
        Ok(actual) if actual == expected => {
            if let Some(before) = before {
                if let Err(error) = file.set_len(before.len() as u64) {
                    return format!(
                        "; partial append rollback failed: {error}"
                    );
                }
                if let Err(error) = file.sync_all() {
                    return format!(
                        "; partial append rollback failed: {error}"
                    );
                }
            } else {
                drop(file);
                if let Err(error) = fs::remove_file(path) {
                    return format!(
                        "; partial append rollback failed: {error}"
                    );
                }
            }
            "; partial append was rolled back".to_owned()
        }
        Ok(_) => {
            "; partial append diverged; later bytes were preserved".to_owned()
        }
        Err(error) => format!("; could not inspect partial append: {error}"),
    }
}

fn atomic_write_bytes(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "append target has no parent",
        )
    })?;
    let stem = path.file_name().unwrap_or_default().to_string_lossy();
    let (temporary, mut file) = loop {
        let id = APPEND_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temporary = parent
            .join(format!(".{stem}.bob-rollback-{}-{id}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o644);
        }
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => return Err(error),
        }
    };
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn rollback_appends(appends: &[AppliedAppend]) -> Vec<String> {
    let mut left = Vec::new();
    for append in appends.iter().rev() {
        match fs::read(&append.target) {
            Ok(current) if current == append.after => match &append.before {
                Some(before) => {
                    if let Err(error) =
                        atomic_write_bytes(&append.target, before)
                    {
                        left.push(format!(
                            "could not restore {}: {error}",
                            append.target.display()
                        ));
                    }
                }
                None => {
                    if let Err(error) = fs::remove_file(&append.target) {
                        left.push(format!(
                            "could not remove {}: {error}",
                            append.target.display()
                        ));
                    }
                }
            },
            Ok(_) => left.push(format!(
                "{} changed since the append and was left in place",
                append.target.display()
            )),
            Err(error) => left.push(format!(
                "could not read {} during rollback: {error}",
                append.target.display()
            )),
        }
    }
    left
}

/// Install, verify, and commit one plan. No lock, no vault-sync cycles.
pub(crate) fn apply_and_commit<T>(
    vault: &Path,
    child_env: &ChildEnv,
    planned: PlannedTxn<T>,
) -> Result<(Option<CommitInfo>, T), TxnFailure> {
    let PlannedTxn {
        files,
        appends,
        deps,
        subject,
        verify,
        context,
        kind,
    } = planned;
    if files.is_empty() && appends.iter().all(|append| append.suffix.is_empty())
    {
        return Ok((None, context));
    }
    let mut relatives: Vec<PathBuf> = files
        .iter()
        .map(|staged| relative_of(vault, &staged.target))
        .collect();
    relatives.extend(
        appends
            .iter()
            .filter(|append| !append.suffix.is_empty())
            .map(|append| relative_of(vault, &append.target)),
    );
    relatives.sort();
    relatives.dedup();
    let old_head = git_head(vault, child_env).ok();

    if !files.is_empty() {
        if let Err(error) =
            write_staged_files_guarded(&files, || deps.validate(vault))
        {
            let stale = error.typed == Some(CaptureFailureKind::NoteChanged);
            return Err(if stale {
                failure(
                    "stale_note",
                    error.message,
                    "re-run the command to replan from the latest files",
                )
            } else {
                failure(
                    "verify_failed",
                    format!(
                        "could not install the {kind} files: {}",
                        error.message
                    ),
                    "the write was rolled back; inspect the listed paths and retry",
                )
            });
        }
    } else if let Err(error) = deps.validate(vault) {
        let stale = error.typed == Some(CaptureFailureKind::NoteChanged);
        return Err(if stale {
            failure(
                "stale_note",
                error.message,
                "re-run the command to replan from the latest files",
            )
        } else {
            failure(
                "verify_failed",
                error.message,
                "inspect the vault and retry",
            )
        });
    }

    let mut applied_appends = Vec::new();
    for append in &appends {
        if append.suffix.is_empty() {
            continue;
        }
        match append_guarded(append) {
            Ok(applied) => applied_appends.push(applied),
            Err(error) => {
                let append_left = rollback_appends(&applied_appends);
                let file_left = rollback_guarded(&files);
                let mut left = append_left;
                left.extend(file_left);
                let stale = error.contains("changed on disk after planning")
                    || error.contains("appeared on disk after planning")
                    || error.contains("was deleted on disk after planning");
                return Err(failure(
                    if stale { "stale_note" } else { "verify_failed" },
                    format!(
                        "could not append the {kind} journal: {error}{}",
                        rollback_note(&left)
                    ),
                    if stale {
                        "re-run the command to replan from the latest files"
                    } else {
                        "the write was rolled back; inspect the listed paths and retry"
                    },
                ));
            }
        }
    }

    if let Err(message) = verify() {
        let mut left = rollback_appends(&applied_appends);
        left.extend(rollback_guarded(&files));
        return Err(failure(
            "verify_failed",
            format!(
                "could not verify the written {kind} files: {message}{}",
                rollback_note(&left)
            ),
            "the write was rolled back; inspect the listed paths and retry",
        ));
    }

    let rolled_back = |unstage: String, left: Vec<String>| {
        format!("{unstage}{}", rollback_note(&left))
    };
    let sha = match ob::commit_paths(vault, child_env, &subject, &relatives) {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            let unstage = unstage_paths(vault, child_env, &relatives);
            let mut left = rollback_appends(&applied_appends);
            left.extend(rollback_guarded(&files));
            return Err(failure(
                "verify_failed",
                format!(
                    "the write produced no scoped Git commit{}",
                    rolled_back(unstage, left)
                ),
                "check the vault Git state and retry",
            ));
        }
        Err(error) => {
            match committed_despite_error(
                vault,
                child_env,
                old_head.as_deref(),
                &subject,
            ) {
                Some((sha, committed)) if committed == subject => sha,
                _ => {
                    let unstage = unstage_paths(vault, child_env, &relatives);
                    let mut left = rollback_appends(&applied_appends);
                    left.extend(rollback_guarded(&files));
                    return Err(failure(
                        "verify_failed",
                        format!(
                            "Git commit failed: {error}{}",
                            rolled_back(unstage, left)
                        ),
                        "check the vault Git state and retry",
                    ));
                }
            }
        }
    };
    Ok((Some(CommitInfo { sha, subject }), context))
}

fn sync_cycle(child_env: &ChildEnv) -> Result<(), String> {
    let cycle =
        vault_sync::run_cycle_with_existing_lock_report(child_env, true);
    if cycle.ok && cycle.conflicts.is_empty() {
        return Ok(());
    }
    Err(if cycle.conflicts.is_empty() {
        cycle
            .error
            .unwrap_or_else(|| "vault sync reported failure".to_string())
    } else {
        format!("vault sync conflict in {}", cycle.conflicts.join(", "))
    })
}

/// Standalone lifecycle around [`apply_and_commit`]. `plan` runs after the
/// pre-sync, so it sees the synced snapshot. A no-op plan writes nothing,
/// commits nothing, and skips the post-sync. `kind` is the noun in the
/// worktree error (`"{kind} writes require a Git worktree"`).
pub(crate) fn run_locked<T>(
    vault: &Path,
    child_env: &ChildEnv,
    json_mode: bool,
    kind: &'static str,
    plan: impl FnOnce() -> Result<PlannedTxn<T>, TxnFailure>,
) -> Result<TxnResult<T>, TxnFailure> {
    if let Some(detail) = vault_sync::vault_identity_problem(vault) {
        return Err(failure(
            "vault_mismatch",
            detail,
            "point BOB_DIR at the vault passed with -b",
        ));
    }
    let _lock = {
        let on_first_wait = move || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                eprintln!(
                    "  {}",
                    Styler::detect()
                        .dim("waiting for another vault maintenance run…")
                );
            }
        };
        match ob::acquire_lock_waiting(LOCK_WAIT, on_first_wait) {
            Ok(lock) => lock,
            Err(error) => {
                let message = match error {
                    LockWaitError::Timeout { path } => format!(
                        "another Bob vault maintenance run is already active (waited on {})",
                        path.display()
                    ),
                    other => format!(
                        "could not acquire the vault maintenance lock: {other}"
                    ),
                };
                return Err(failure(
                    "lock_busy",
                    message,
                    "wait for the other maintenance run to finish, then retry",
                ));
            }
        }
    };
    match ob::detect_git_worktree(vault, child_env) {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            return Err(failure(
                "not_a_worktree",
                format!(
                    "{kind} writes require a Git worktree: {}",
                    vault.display()
                ),
                "initialize or open the Bob vault as a Git worktree",
            ));
        }
    }
    if let Err(detail) = sync_cycle(child_env) {
        return Err(failure(
            "sync_failed",
            format!("pre-sync failed: {detail}"),
            "run bob vault-sync, resolve the problem, then retry",
        ));
    }

    let planned = plan()?;
    let (commit, context) = apply_and_commit(vault, child_env, planned)?;
    let (sync_post, post_sync_warning) = if commit.is_none() {
        ("skipped", None)
    } else {
        match sync_cycle(child_env) {
            Ok(()) => ("ok", None),
            Err(detail) => (
                "failed",
                Some(format!(
                    "the change is committed locally; post-sync failed ({detail}); run bob vault-sync"
                )),
            ),
        }
    };
    Ok(TxnResult {
        commit,
        sync_pre: "ok",
        sync_post,
        post_sync_warning,
        context,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(vault: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("scratch dir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        git(&vault, &["init", "-q"]);
        git(&vault, &["config", "user.email", "t@t"]);
        git(&vault, &["config", "user.name", "t"]);
        fs::write(vault.join("host.md"), "host\n").unwrap();
        fs::write(vault.join("ancestor.md"), "ancestor\n").unwrap();
        git(&vault, &["add", "-A"]);
        git(&vault, &["commit", "-qm", "init"]);
        (temp, vault)
    }

    fn staged(vault: &Path, before: &str, after: &str) -> Vec<StagedTextFile> {
        vec![
            StagedTextFile {
                target: vault.join("host.md"),
                target_existed: true,
                original_target: before.to_string(),
                updated_target: after.to_string(),
            },
            StagedTextFile {
                target: vault.join("new_note.md"),
                target_existed: false,
                original_target: String::new(),
                updated_target: "fresh\n".to_string(),
            },
        ]
    }

    fn plan(
        vault: &Path,
        files: Vec<StagedTextFile>,
        deps: ReadDependencies,
    ) -> PlannedTxn<()> {
        let _ = vault;
        PlannedTxn {
            files,
            appends: Vec::new(),
            deps,
            subject: "notebooks: test".to_string(),
            verify: Box::new(|| Ok(())),
            context: (),
            kind: "notebook",
        }
    }

    fn deps(contents: &str) -> ReadDependencies {
        let mut deps = ReadDependencies::default();
        deps.insert(PathBuf::from("ancestor.md"), Some(contents.to_string()));
        deps
    }

    fn head(vault: &Path) -> String {
        git(vault, &["rev-parse", "HEAD"])
    }

    #[test]
    fn commits_only_the_staged_paths() {
        let (_temp, vault) = scratch();
        fs::write(vault.join("unrelated.md"), "dirty\n").unwrap();
        let (commit, ()) = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect("applies");
        let commit = commit.expect("committed");
        assert_eq!(commit.subject, "notebooks: test");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host+\n"
        );
        assert_eq!(
            fs::read_to_string(vault.join("new_note.md")).unwrap(),
            "fresh\n"
        );
        let changed =
            git(&vault, &["show", "--name-only", "--format=", "HEAD"]);
        assert_eq!(
            changed.lines().collect::<Vec<_>>(),
            ["host.md", "new_note.md"]
        );
        assert!(
            git(&vault, &["status", "--porcelain"]).contains("unrelated.md")
        );
    }

    #[test]
    fn empty_plan_is_a_no_op() {
        let (_temp, vault) = scratch();
        let before = head(&vault);
        let (commit, ()) = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(&vault, Vec::new(), ReadDependencies::default()),
        )
        .expect("no-op");
        assert!(commit.is_none());
        assert_eq!(head(&vault), before);
    }

    #[test]
    fn stale_read_dependency_refuses_before_any_write() {
        let (_temp, vault) = scratch();
        fs::write(vault.join("ancestor.md"), "edited meanwhile\n").unwrap();
        let before = head(&vault);
        let failure = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect_err("stale");
        assert_eq!(failure.code, "stale_note");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert_eq!(head(&vault), before);
    }

    #[test]
    fn stale_preimage_and_planted_destination_refuse_without_overwriting() {
        let (_temp, vault) = scratch();
        fs::write(vault.join("host.md"), "foreign edit\n").unwrap();
        let failure = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect_err("stale preimage");
        assert_eq!(failure.code, "stale_note");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "foreign edit\n"
        );

        fs::write(vault.join("host.md"), "host\n").unwrap();
        fs::write(vault.join("new_note.md"), "someone else's file\n").unwrap();
        let failure = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect_err("destination appeared");
        assert_eq!(failure.code, "stale_note");
        assert_eq!(
            fs::read_to_string(vault.join("new_note.md")).unwrap(),
            "someone else's file\n"
        );
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn commit_failure_rolls_back_the_write_and_unstages() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, vault) = scratch();
        let hook = vault.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let before = head(&vault);
        let failure = apply_and_commit(
            &vault,
            &Vec::new(),
            plan(
                &vault,
                staged(&vault, "host\n", "host+\n"),
                deps("ancestor\n"),
            ),
        )
        .expect_err("commit fails");
        assert_eq!(failure.code, "verify_failed");
        assert!(
            failure.message.contains("Git commit failed"),
            "{}",
            failure.message
        );
        assert!(
            failure.message.contains("rolled back"),
            "{}",
            failure.message
        );
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert_eq!(head(&vault), before);
        assert_eq!(git(&vault, &["diff", "--cached", "--name-only"]), "");
    }

    #[test]
    fn verify_failure_rolls_back_without_committing() {
        let (_temp, vault) = scratch();
        let before = head(&vault);
        let mut planned = plan(
            &vault,
            staged(&vault, "host\n", "host+\n"),
            deps("ancestor\n"),
        );
        planned.verify = Box::new(|| Err("still stale".to_string()));
        let failure =
            apply_and_commit(&vault, &Vec::new(), planned).expect_err("verify");
        assert_eq!(failure.code, "verify_failed");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert_eq!(head(&vault), before);
    }

    #[test]
    fn rollback_never_overwrites_a_later_edit() {
        let (_temp, vault) = scratch();
        let files = staged(&vault, "host\n", "host+\n");
        fs::write(vault.join("host.md"), "host+\n").unwrap();
        fs::write(vault.join("new_note.md"), "fresh\n").unwrap();
        // The host changed again after our write; the created file is intact.
        fs::write(vault.join("host.md"), "later foreign edit\n").unwrap();
        let left = rollback_guarded(&files);
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].contains("host.md"));
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "later foreign edit\n"
        );
        assert!(
            !vault.join("new_note.md").exists(),
            "our own file is removed"
        );
    }

    #[test]
    fn ordered_append_is_committed_with_staged_rows() {
        let (_temp, vault) = scratch();
        let target = vault.join(".bob/bank/athena.jsonl");
        let before = b"legacy malformed \xff".to_vec();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, &before).unwrap();
        let suffix = b"\n{\"kind\":\"bank.txn\"}\n".to_vec();
        let mut planned = plan(
            &vault,
            staged(&vault, "host\n", "host+\n"),
            deps("ancestor\n"),
        );
        planned.appends.push(PlannedAppend {
            target: target.clone(),
            original_target: Some(before.clone()),
            suffix: suffix.clone(),
        });

        let (commit, ()) = apply_and_commit(&vault, &Vec::new(), planned)
            .expect("rows and journal commit together");
        assert!(commit.is_some());
        let mut expected = before;
        expected.extend_from_slice(&suffix);
        assert_eq!(fs::read(&target).unwrap(), expected);
        let changed =
            git(&vault, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(changed.lines().any(|line| line == "host.md"));
        assert!(changed.lines().any(|line| line == "new_note.md"));
        assert!(changed.lines().any(|line| line == ".bob/bank/athena.jsonl"));
    }

    #[test]
    fn verify_failure_rolls_back_rows_and_journal_append() {
        let (_temp, vault) = scratch();
        let target = vault.join(".bob/bank/athena.jsonl");
        let mut planned = plan(
            &vault,
            staged(&vault, "host\n", "host+\n"),
            deps("ancestor\n"),
        );
        planned.appends.push(PlannedAppend {
            target: target.clone(),
            original_target: None,
            suffix: b"{\"kind\":\"bank.txn\"}\n".to_vec(),
        });
        planned.verify = Box::new(|| Err("forced verifier failure".to_owned()));

        let error = apply_and_commit(&vault, &Vec::new(), planned)
            .expect_err("failed verification rolls back");
        assert_eq!(error.code, "verify_failed");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert!(!target.exists());
        assert_eq!(git(&vault, &["diff", "--cached", "--name-only"]), "");
    }

    #[test]
    fn stale_journal_preimage_rolls_back_installed_rows() {
        let (_temp, vault) = scratch();
        let target = vault.join(".bob/bank/athena.jsonl");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"changed after planning\n").unwrap();
        let mut planned = plan(
            &vault,
            staged(&vault, "host\n", "host+\n"),
            deps("ancestor\n"),
        );
        planned.appends.push(PlannedAppend {
            target: target.clone(),
            original_target: Some(b"planned bytes\n".to_vec()),
            suffix: b"{\"kind\":\"bank.txn\"}\n".to_vec(),
        });

        let error = apply_and_commit(&vault, &Vec::new(), planned)
            .expect_err("stale journal preimage refuses");
        assert_eq!(error.code, "stale_note");
        assert_eq!(fs::read(&target).unwrap(), b"changed after planning\n");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert_eq!(git(&vault, &["diff", "--cached", "--name-only"]), "");
    }

    #[cfg(unix)]
    #[test]
    fn commit_failure_rolls_back_rows_and_journal_append() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, vault) = scratch();
        let target = vault.join(".bob/bank/athena.jsonl");
        let mut planned = plan(
            &vault,
            staged(&vault, "host\n", "host+\n"),
            deps("ancestor\n"),
        );
        planned.appends.push(PlannedAppend {
            target: target.clone(),
            original_target: None,
            suffix: b"{\"kind\":\"bank.txn\"}\n".to_vec(),
        });
        let hook = vault.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        let error = apply_and_commit(&vault, &Vec::new(), planned)
            .expect_err("commit failure rolls back both effects");
        assert_eq!(error.code, "verify_failed");
        assert!(error.message.contains("rolled back"), "{}", error.message);
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
        assert!(!vault.join("new_note.md").exists());
        assert!(!target.exists());
        assert_eq!(git(&vault, &["diff", "--cached", "--name-only"]), "");
    }

    #[test]
    fn partial_append_recovery_restores_exact_binary_preimage() {
        let (_temp, vault) = scratch();
        let target = vault.join(".bob/bank/athena.jsonl");
        let before = b"broken \xff event";
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, before).unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&target)
            .unwrap();
        file.write_all(b"\n{\"kind\":").unwrap();
        let result = rollback_partial_append(
            file,
            &target,
            Some(before),
            b"\n{\"kind\":",
        );
        assert!(result.contains("rolled back"), "{result}");
        assert_eq!(fs::read(&target).unwrap(), before);
    }

    #[test]
    fn changed_directory_membership_refuses_before_staged_writes() {
        let (_temp, vault) = scratch();
        let mut directory_deps = ReadDependencies::default();
        directory_deps.insert_directory(
            PathBuf::from("bank"),
            Some("md"),
            Vec::new(),
        );
        let mut planned =
            plan(&vault, staged(&vault, "host\n", "host+\n"), directory_deps);
        planned.subject = "bank: import 1 rows (checking)".to_owned();
        fs::create_dir_all(vault.join("bank")).unwrap();
        fs::write(vault.join("bank/2026-11.md"), "month\n").unwrap();

        let error = apply_and_commit(&vault, &Vec::new(), planned)
            .expect_err("new month invalidates the snapshot");
        assert_eq!(error.code, "stale_note");
        assert_eq!(
            fs::read_to_string(vault.join("host.md")).unwrap(),
            "host\n"
        );
    }
}
