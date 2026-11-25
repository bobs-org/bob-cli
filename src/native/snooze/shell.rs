//! Snapshot and guarded-write shell for the snooze CLI.
//!
//! The write shell deliberately keeps the whole Markdown membership and
//! content snapshot: row resolution, task state, inherited type, and budget
//! forecasts can all depend on notes that are not themselves rewritten.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(test)]
use std::cell::RefCell;

use crate::native::{
    capture::{self, CaptureFailureKind, StagedTextFile},
    ob::{self, LockWaitError},
    vault_links::NoteIndex,
};

use super::plan::FilePostimage;

const TASKS_SETTINGS: &str =
    ".obsidian/plugins/obsidian-tasks-plugin/data.json";

#[cfg(test)]
thread_local! {
    static BEFORE_NEXT_VALIDATION: RefCell<Option<Box<dyn FnOnce()>>> =
        RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn before_next_validation(hook: impl FnOnce() + 'static) {
    BEFORE_NEXT_VALIDATION
        .with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_validation_hook() {
    let hook = BEFORE_NEXT_VALIDATION.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(not(test))]
fn run_validation_hook() {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputSnapshot {
    /// Vault-relative Markdown path to exact text, including line endings.
    pub(crate) notes: BTreeMap<PathBuf, String>,
    config: Option<String>,
    tasks_settings: Option<String>,
}

impl InputSnapshot {
    pub(crate) fn read(root: &Path) -> io::Result<Self> {
        let mut paths = Vec::new();
        collect_markdown_paths(root, root, &mut paths)?;
        let mut notes = BTreeMap::new();
        for relative in paths {
            let contents = fs::read_to_string(root.join(&relative))?;
            notes.insert(relative, contents);
        }
        let config = read_optional(&crate::native::config::config_path())?;
        let tasks_settings = read_optional(&root.join(TASKS_SETTINGS))?;
        Ok(Self {
            notes,
            config,
            tasks_settings,
        })
    }

    pub(crate) fn note_index(&self) -> NoteIndex {
        NoteIndex::from_paths(self.notes.keys().cloned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LockOutcome {
    Deferred,
    Failed(String),
}

pub(crate) fn acquire(timeout: Duration) -> Result<std::fs::File, LockOutcome> {
    match ob::acquire_lock_waiting(timeout, || {}) {
        Ok(file) => Ok(file),
        Err(LockWaitError::Timeout { .. }) => Err(LockOutcome::Deferred),
        Err(error) => Err(LockOutcome::Failed(error.to_string())),
    }
}

#[derive(Debug)]
pub(crate) enum InstallError {
    Changed,
    Failed(String),
}

/// Install changed postimages as one rollback-capable batch. Read-only
/// inputs are checked before staging and again immediately before install.
pub(crate) fn install(
    root: &Path,
    postimages: &[FilePostimage],
    snapshot: &InputSnapshot,
) -> Result<(), InstallError> {
    let mut staged = Vec::new();
    for postimage in postimages {
        let Some(original) = snapshot.notes.get(&postimage.path) else {
            return Err(InstallError::Failed(format!(
                "planned note is absent from the vault snapshot: {}",
                postimage.path.display()
            )));
        };
        if original == &postimage.contents {
            continue;
        }
        staged.push(StagedTextFile {
            target: root.join(&postimage.path),
            target_existed: true,
            original_target: original.clone(),
            updated_target: postimage.contents.clone(),
        });
    }
    if staged.is_empty() {
        return Ok(());
    }

    match capture::write_staged_files_guarded(&staged, || {
        run_validation_hook();
        let current = InputSnapshot::read(root).map_err(|error| {
            capture::read_failed(format!(
                "refresh snooze write inputs: {error}"
            ))
        })?;
        if &current != snapshot {
            return Err(capture::changed_after_planning(
                "vault Markdown, resolver membership, or configuration changed after snooze planning",
            ));
        }
        Ok(())
    }) {
        Ok(()) => Ok(()),
        Err(error) if error.typed == Some(CaptureFailureKind::NoteChanged) => {
            Err(InstallError::Changed)
        }
        Err(error) => Err(InstallError::Failed(error.message)),
    }
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn collect_markdown_paths(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut entries = entries.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            collect_markdown_paths(root, &path, paths)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            paths.push(relative.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lock_timeout_is_reported_as_deferred() {
        let temp = tempfile::tempdir().expect("tempdir");
        let lock_path = temp.path().join("vault.lock");
        let _env = crate::native::env::TestEnvGuard::set(&[(
            "BOB_VAULT_SYNC_LOCK_FILE",
            Some(lock_path.as_os_str()),
        )]);
        let held = ob::acquire_lock_waiting(Duration::ZERO, || {})
            .expect("acquire test lock");
        assert!(matches!(
            acquire(Duration::ZERO),
            Err(LockOutcome::Deferred)
        ));
        drop(held);
    }

    #[test]
    fn changed_read_dependency_refuses_before_install() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let day = root.join("today.md");
        let dependency = root.join("type.md");
        let config = root.join("bob.yml");
        fs::write(&day, "before day\n").expect("write day");
        fs::write(&dependency, "before type\n").expect("write dependency");
        fs::write(&config, "timezone: America/New_York\n").expect("config");
        let _env = crate::native::env::TestEnvGuard::set(&[(
            "BOB_CONFIG_FILE",
            Some(config.as_os_str()),
        )]);
        let snapshot = InputSnapshot::read(root).expect("snapshot");
        before_next_validation({
            let dependency = dependency.clone();
            move || fs::write(dependency, "changed type\n").expect("drift")
        });
        let result = install(
            root,
            &[FilePostimage {
                path: PathBuf::from("today.md"),
                contents: "after day\n".to_string(),
            }],
            &snapshot,
        );
        assert!(matches!(result, Err(InstallError::Changed)), "{result:?}");
        assert_eq!(fs::read_to_string(day).unwrap(), "before day\n");
    }

    #[test]
    fn second_preimage_drift_remains_unwritten_after_one_retry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let day = root.join("today.md");
        let dependency = root.join("type.md");
        let config = root.join("bob.yml");
        fs::write(&day, "before day\n").expect("write day");
        fs::write(&dependency, "before type\n").expect("write dependency");
        fs::write(&config, "timezone: America/New_York\n").expect("config");
        let _env = crate::native::env::TestEnvGuard::set(&[(
            "BOB_CONFIG_FILE",
            Some(config.as_os_str()),
        )]);

        for attempt in 0..2 {
            let snapshot = InputSnapshot::read(root).expect("fresh snapshot");
            before_next_validation({
                let dependency = dependency.clone();
                move || {
                    fs::write(dependency, format!("drift {attempt}\n"))
                        .expect("inject drift");
                }
            });
            let result = install(
                root,
                &[FilePostimage {
                    path: PathBuf::from("today.md"),
                    contents: "after day\n".to_string(),
                }],
                &snapshot,
            );
            assert!(matches!(result, Err(InstallError::Changed)), "{result:?}");
        }
        assert_eq!(fs::read_to_string(day).unwrap(), "before day\n");
    }
}
