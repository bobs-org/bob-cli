use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{engine::build_result, model::CaptureCompleteResult};

mod active_tasks;
mod output;
mod parent_tasks;
mod pomodoros;
mod routes_tasks;
mod task_links;

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

// Overrides below are thread-local (see `crate::native::env`), so no
// serializing lock is needed: a test's `BOB_DAY_FILE` override is
// invisible to every other test thread.

fn result(bob_dir: &Path, raw: &str, cursor: usize) -> CaptureCompleteResult {
    build_result(bob_dir, raw, cursor, false).expect("build result")
}

fn result_all(
    bob_dir: &Path,
    raw: &str,
    cursor: usize,
) -> CaptureCompleteResult {
    build_result(bob_dir, raw, cursor, true).expect("build result")
}

fn write_settings(root: &Path) {
    write_file(
        &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"*","name":"Next","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
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
        if let Err(error) = fs::remove_dir_all(&self.path) {
            eprintln!("failed to remove {}: {error}", self.path.display());
        }
    }
}

fn current_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}
