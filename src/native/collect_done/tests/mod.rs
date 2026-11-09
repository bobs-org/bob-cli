//! Shared fixtures for collect_done tests.
use super::*;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

mod execution;
mod plan;
mod snapshot;
mod unit;

use super::super::task_status_hooks_write::{
    apply_plan, new_run_id, ApplySession,
};
use super::snapshot::scan_membership;
use std::time::Duration;

/// A guarded-apply session rooted in a temporary state directory, with the
/// same membership scope production uses and no real sleeping.
fn test_session(state_dir: &Path, vault: &Path) -> ApplySession {
    let rescan_vault = vault.to_path_buf();
    ApplySession {
        tool: "task-archive",
        state_home: state_dir.to_path_buf(),
        quiet_period: Duration::from_secs(2),
        retention: Duration::from_secs(30 * 24 * 60 * 60),
        run_id: new_run_id(),
        now_system: Box::new(SystemTime::now),
        sleep: Box::new(|_| {}),
        rescan: Box::new(move || scan_membership(&rescan_vault)),
        before_preflight: None,
        after_staging: None,
        before_replace: None,
        fail_staging: None,
    }
}

/// Plan the whole archive collection and apply it through the guarded engine.
fn apply_archive(vault: &Path, threshold: usize) {
    let state = TempDir::new("bob-cli-archive-test-state");
    let planned = plan_archive(vault, threshold).expect("plan archive");
    apply_plan(&planned.write_plan, &test_session(state.path(), vault))
        .unwrap_or_else(|error| panic!("apply archive: {}", error.message));
}

fn note_index<const N: usize>(paths: [&str; N]) -> NoteIndex {
    NoteIndex::from_paths(paths.into_iter().map(PathBuf::from))
}

fn moved_targets<const N: usize>(
    entries: [(&str, &str, &str); N],
) -> MovedBlockTargets {
    entries
        .into_iter()
        .map(|(source_path, block_id, target)| {
            (
                (PathBuf::from(source_path), block_id.to_string()),
                MovedBlockTarget {
                    archive_target: target.to_string(),
                    block_id: block_id.to_string(),
                    old_dependency_id: dependency_id(
                        Path::new(source_path),
                        block_id,
                    )
                    .expect("valid source dependency id"),
                    new_dependency_id: dependency_id(
                        &PathBuf::from(format!("{target}.md")),
                        block_id,
                    )
                    .expect("valid archive dependency id"),
                },
            )
        })
        .collect::<BTreeMap<_, _>>()
}

fn string_set<const N: usize>(values: [&str; N]) -> BTreeSet<String> {
    values.into_iter().map(str::to_string).collect()
}

fn os_args<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.into_iter().map(OsString::from).collect()
}

fn area_note(body: &str) -> String {
    format!("---\ntype: [[area]]\n---\n\n{body}")
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{}-{}-{}-{}",
            prefix,
            std::process::id(),
            current_time_nanos(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("create temp dir {}: {error}", path.display())
        });
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(error) = remove_dir_all_if_exists(&self.path) {
            eprintln!(
                "failed to remove temp dir {}: {error}",
                self.path.display()
            );
        }
    }
}

fn current_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}

fn remove_dir_all_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
