use std::{
    ffi::OsString,
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

/// Serializes the `BOB_DAY_FILE` override: the override is
/// process-global, so parallel tests must never set and read it at the
/// same time. Hold the guard for the whole body of any test that touches
/// the day file.
static DAY_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn day_file_guard() -> std::sync::MutexGuard<'static, ()> {
    DAY_FILE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

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

fn with_env<T>(
    key: &str,
    value: impl Into<OsString>,
    f: impl FnOnce() -> T,
) -> T {
    let old = std::env::var_os(key);
    unsafe {
        std::env::set_var(key, value.into());
    }
    let result = f();
    unsafe {
        match old {
            Some(old) => std::env::set_var(key, old),
            None => std::env::remove_var(key),
        }
    }
    result
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
