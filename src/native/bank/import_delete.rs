//! Post-batch cleanup for `bob bank import -D/--delete`.
//!
//! Deletion always runs after the guarded import transaction, outside the
//! vault lock. It is never part of the planned transaction or its rollback
//! set, and deleted bytes are never journaled.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use super::{identity::sha256_hex, import_run::PreparedSource};
use crate::native::{host_paths, vault_rewrite::TxnFailure};

/// Per-file cleanup outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeleteStatus {
    WouldDelete,
    Deleted,
    Kept,
}

impl DeleteStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::WouldDelete => "would_delete",
            Self::Deleted => "deleted",
            Self::Kept => "kept",
        }
    }
}

/// Why a file was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeleteReason {
    ImportFailed,
    ChangedSinceRead,
    Io,
}

impl DeleteReason {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::ImportFailed => "import_failed",
            Self::ChangedSinceRead => "changed_since_read",
            Self::Io => "io",
        }
    }
}

/// One file in the delete report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeleteFileOutcome {
    pub(crate) path: PathBuf,
    pub(crate) status: DeleteStatus,
    pub(crate) reason: Option<DeleteReason>,
    pub(crate) message: Option<String>,
}

/// The additive `delete` object in the import JSON report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeleteReport {
    pub(crate) files: Vec<DeleteFileOutcome>,
}

impl DeleteReport {
    pub(crate) fn empty() -> Self {
        Self { files: Vec::new() }
    }

    pub(crate) fn would_delete(paths: &[PathBuf]) -> Self {
        Self {
            files: paths
                .iter()
                .cloned()
                .map(|path| DeleteFileOutcome {
                    path,
                    status: DeleteStatus::WouldDelete,
                    reason: None,
                    message: None,
                })
                .collect(),
        }
    }

    pub(crate) fn kept_import_failed(paths: &[PathBuf]) -> Self {
        Self {
            files: paths
                .iter()
                .cloned()
                .map(|path| DeleteFileOutcome {
                    path,
                    status: DeleteStatus::Kept,
                    reason: Some(DeleteReason::ImportFailed),
                    message: Some("import failed; nothing deleted".to_owned()),
                })
                .collect(),
        }
    }

    pub(crate) fn has_kept(&self) -> bool {
        self.files
            .iter()
            .any(|file| file.status == DeleteStatus::Kept)
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "files": self.files.iter().map(|file| {
                json!({
                    "path": absolute_label(&file.path),
                    "status": file.status.as_str(),
                    "reason": file.reason.as_ref().map(DeleteReason::as_str),
                    "message": file.message.as_deref(),
                })
            }).collect::<Vec<_>>(),
        })
    }
}

fn absolute_label(path: &Path) -> String {
    if path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }
    std::env::current_dir().map_or_else(
        |_| path.to_string_lossy().into_owned(),
        |cwd| cwd.join(path).to_string_lossy().into_owned(),
    )
}

fn delete_failure(
    code: &'static str,
    message: impl Into<String>,
    hint: &str,
) -> TxnFailure {
    TxnFailure::new(code, message, Some(hint.to_owned()))
}

/// Refusal for `-D` with no FILE operand.
pub(crate) fn missing_files_failure() -> TxnFailure {
    delete_failure(
        "bank_import_missing_files",
        "-D needs at least one FILE to delete",
        "name the exports to import and delete, e.g. `bob bank import -D ~/Downloads/*.qfx`",
    )
}

/// Human phrase naming every file in a destructive delete line.
pub(crate) fn describe_paths(paths: &[PathBuf]) -> String {
    let displays: Vec<String> = paths
        .iter()
        .map(|path| host_paths::display_path(path))
        .collect();
    if displays.len() == 1 {
        return displays.into_iter().next().unwrap_or_default();
    }
    let mut parents = Vec::with_capacity(paths.len());
    let mut names = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| host_paths::display_path(path));
        let parent = path.parent().map_or_else(
            || ".".to_owned(),
            |parent| {
                if parent.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    host_paths::display_path(parent)
                }
            },
        );
        parents.push(parent);
        names.push(name);
    }
    let same_parent =
        !parents.is_empty() && parents.iter().all(|p| *p == parents[0]);
    if same_parent {
        format!(
            "{} files from {}: {}",
            paths.len(),
            parents[0],
            names.join(", ")
        )
    } else {
        format!("{} files: {}", paths.len(), displays.join(", "))
    }
}

/// Up-front gate for `-D`, run after expansion and before parse, lock, or
/// sync. Missing files fall through to the ordinary read error so the batch
/// still refuses as `import_failed`; everything else non-regular or inside
/// the vault refuses here.
pub(crate) fn check_delete_supported(
    paths: &[PathBuf],
    vault: &Path,
) -> Result<(), TxnFailure> {
    let vault_canonical = fs::canonicalize(vault).ok();
    for path in paths {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => continue,
        };
        let kind = metadata.file_type();
        if kind.is_symlink() {
            return Err(delete_failure(
                "bank_import_delete_unsupported",
                format!(
                    "-D deletes only regular files; {} is a symlink",
                    host_paths::display_path(path)
                ),
                "pass the real file, or drop -D",
            ));
        }
        if kind.is_dir() {
            return Err(delete_failure(
                "bank_import_delete_unsupported",
                format!(
                    "-D deletes only regular files; {} is a directory",
                    host_paths::display_path(path)
                ),
                "pass the real file, or drop -D",
            ));
        }
        if !kind.is_file() {
            return Err(delete_failure(
                "bank_import_delete_unsupported",
                format!(
                    "-D deletes only regular files; {} is not a regular file",
                    host_paths::display_path(path)
                ),
                "pass the real file, or drop -D",
            ));
        }
        if let (Some(vault_root), Ok(canonical)) =
            (vault_canonical.as_ref(), fs::canonicalize(path))
            && canonical.starts_with(vault_root)
        {
            return Err(delete_failure(
                "bank_import_delete_unsupported",
                format!(
                    "-D never deletes vault files; {} is inside the vault",
                    host_paths::display_path(path)
                ),
                "move the export out of the vault, or drop -D",
            ));
        }
    }
    Ok(())
}

fn io_message(error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::PermissionDenied {
        format!("{error}; {}", host_paths::permission_hint())
    } else {
        error.to_string()
    }
}

/// Cleanup after a successful import. Keeps going after a failure so one
/// locked file never strands the rest of the batch.
pub(crate) fn remove_sources_with(
    sources: &[PreparedSource],
    remove: impl Fn(&Path) -> io::Result<()>,
) -> DeleteReport {
    let mut files = Vec::with_capacity(sources.len());
    for source in sources {
        let path = &source.path;
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                files.push(DeleteFileOutcome {
                    path: path.clone(),
                    status: DeleteStatus::Deleted,
                    reason: None,
                    message: None,
                });
                continue;
            }
            Ok(metadata) if !metadata.file_type().is_file() => {
                files.push(kept_changed(path.clone()));
                continue;
            }
            Err(_) | Ok(_) => {}
        }
        let digest = match fs::read(path) {
            Ok(bytes) => sha256_hex(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                files.push(DeleteFileOutcome {
                    path: path.clone(),
                    status: DeleteStatus::Deleted,
                    reason: None,
                    message: None,
                });
                continue;
            }
            Err(error) => {
                files.push(DeleteFileOutcome {
                    path: path.clone(),
                    status: DeleteStatus::Kept,
                    reason: Some(DeleteReason::Io),
                    message: Some(io_message(&error)),
                });
                continue;
            }
        };
        if digest != source.statement.file_digest {
            files.push(kept_changed(path.clone()));
            continue;
        }
        match remove(path) {
            Ok(()) => files.push(DeleteFileOutcome {
                path: path.clone(),
                status: DeleteStatus::Deleted,
                reason: None,
                message: None,
            }),
            Err(error) => files.push(DeleteFileOutcome {
                path: path.clone(),
                status: DeleteStatus::Kept,
                reason: Some(DeleteReason::Io),
                message: Some(io_message(&error)),
            }),
        }
    }
    DeleteReport { files }
}

fn kept_changed(path: PathBuf) -> DeleteFileOutcome {
    DeleteFileOutcome {
        path,
        status: DeleteStatus::Kept,
        reason: Some(DeleteReason::ChangedSinceRead),
        message: Some("changed since bob read it".to_owned()),
    }
}

/// Public entry point wiring the real unlink.
pub(crate) fn remove_sources(sources: &[PreparedSource]) -> DeleteReport {
    remove_sources_with(sources, |path| fs::remove_file(path))
}

/// Failure for a committed import whose cleanup kept at least one file.
pub(crate) fn delete_failed_failure(kept: &[DeleteFileOutcome]) -> TxnFailure {
    let count = kept.len();
    let noun = if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    };
    let mut lines = kept
        .iter()
        .map(|file| {
            let detail = file.message.as_deref().unwrap_or("could not delete");
            format!("{} — {detail}", host_paths::display_path(&file.path))
        })
        .collect::<Vec<_>>();
    lines.sort();
    let message = format!(
        "import succeeded, but could not delete {noun}\n{}",
        lines.join("\n")
    );
    delete_failure(
        "bank_delete_failed",
        message,
        "Re-run the same command to retry, or delete it by hand — it holds your account number.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::bank::source;

    fn prepared_with(path: PathBuf, bytes: &[u8]) -> PreparedSource {
        let label = path.to_string_lossy().into_owned();
        PreparedSource {
            label,
            path,
            statement: source::parse_export("label", bytes, 0)
                .expect("test CSV parses"),
        }
    }

    const CSV: &str = "11/12/2026,-13.71,*,,\"SMASHBURGER CARD 1234\"\n";

    #[test]
    fn phrase_names_one_file() {
        let home = tempfile::tempdir().expect("home");
        let _guard = crate::native::env::TestEnvGuard::set(&[(
            "HOME",
            Some(home.path().as_os_str()),
        )]);
        let path = home.path().join("Downloads/Checking1.qfx");
        assert_eq!(
            describe_paths(std::slice::from_ref(&path)),
            format!("~/Downloads/Checking1.qfx")
        );
    }

    #[test]
    fn phrase_groups_one_parent() {
        let home = tempfile::tempdir().expect("home");
        let _guard = crate::native::env::TestEnvGuard::set(&[(
            "HOME",
            Some(home.path().as_os_str()),
        )]);
        let paths = vec![
            PathBuf::from("/home/bryan/Downloads/Checking1.qfx"),
            PathBuf::from("/home/bryan/Downloads/Checking1 (1).qfx"),
        ];
        assert_eq!(
            describe_paths(&paths),
            "2 files from /home/bryan/Downloads: Checking1.qfx, Checking1 (1).qfx"
        );
    }

    #[test]
    fn phrase_lists_several_parents() {
        let home = tempfile::tempdir().expect("home");
        let _guard = crate::native::env::TestEnvGuard::set(&[(
            "HOME",
            Some(home.path().as_os_str()),
        )]);
        let paths = vec![
            PathBuf::from("/home/bryan/Downloads/a.qfx"),
            PathBuf::from("/tmp/b.csv"),
        ];
        assert_eq!(
            describe_paths(&paths),
            "2 files: /home/bryan/Downloads/a.qfx, /tmp/b.csv"
        );
    }

    #[test]
    fn up_front_check_refuses_symlink_directory_and_vault_files() {
        let temp = tempfile::tempdir().expect("temp");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("vault");
        let regular = temp.path().join("export.qfx");
        fs::write(&regular, b"qfx").expect("regular");
        assert!(
            check_delete_supported(std::slice::from_ref(&regular), &vault)
                .is_ok()
        );

        #[cfg(unix)]
        {
            let link = temp.path().join("link.qfx");
            std::os::unix::fs::symlink(&regular, &link).expect("symlink");
            let error =
                check_delete_supported(std::slice::from_ref(&link), &vault)
                    .expect_err("symlink is refused");
            assert_eq!(error.code, "bank_import_delete_unsupported");
            assert!(
                error.message.contains("is a symlink"),
                "{}",
                error.message
            );
        }

        let directory = temp.path().join("dir.qfx");
        fs::create_dir(&directory).expect("directory");
        let error =
            check_delete_supported(std::slice::from_ref(&directory), &vault)
                .expect_err("directory is refused");
        assert_eq!(error.code, "bank_import_delete_unsupported");

        let inside = vault.join("x.qfx");
        fs::write(&inside, b"qfx").expect("vault file");
        let error =
            check_delete_supported(std::slice::from_ref(&inside), &vault)
                .expect_err("vault file is refused");
        assert_eq!(error.code, "bank_import_delete_unsupported");
        assert!(
            error.message.contains("inside the vault"),
            "{}",
            error.message
        );
    }

    #[test]
    fn cleanup_counts_deleted_changed_and_injected_failures() {
        let temp = tempfile::tempdir().expect("temp");
        let gone = temp.path().join("gone.qfx");
        let same = temp.path().join("same.qfx");
        fs::write(&same, CSV).expect("same");
        let changed = temp.path().join("changed.qfx");
        fs::write(&changed, CSV).expect("changed");
        let locked = temp.path().join("locked.qfx");
        fs::write(&locked, CSV).expect("locked");

        let sources = vec![
            prepared_with(gone.clone(), CSV.as_bytes()),
            prepared_with(same.clone(), CSV.as_bytes()),
            prepared_with(changed.clone(), CSV.as_bytes()),
            prepared_with(locked.clone(), CSV.as_bytes()),
        ];
        fs::write(&changed, "11/12/2026,-99.99,*,,\"OTHER\"\n")
            .expect("mutate");
        let report = remove_sources_with(&sources, |path| {
            if path == locked {
                Err(io::Error::other("locked"))
            } else {
                fs::remove_file(path)
            }
        });
        assert_eq!(report.files[0].status, DeleteStatus::Deleted);
        assert_eq!(report.files[1].status, DeleteStatus::Deleted);
        assert_eq!(report.files[2].status, DeleteStatus::Kept);
        assert_eq!(
            report.files[2].reason,
            Some(DeleteReason::ChangedSinceRead)
        );
        assert_eq!(report.files[3].status, DeleteStatus::Kept);
        assert_eq!(report.files[3].reason, Some(DeleteReason::Io));
        assert!(report.has_kept());
        assert!(!same.exists());
        assert!(locked.exists());
    }
}
