use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(test)]
use std::{cell::RefCell, collections::HashMap};

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime};

// Thread-local test overrides for environment variables.
//
// Lib tests share one process, and process environment is process-global:
// one test's `std::env::set_var` is visible to every other test thread, so
// parallel `cargo test --lib` flakes whenever a writer and a reader overlap
// on `BOB_DAY_FILE` or `BOB_NOW` (bob-cli-2e, bob-cli-40, bob-cli-5c).
//
// Tests must never touch process environment. Instead they set overrides
// here through `TestEnvGuard`, which are visible only to the current
// thread and restored on drop (even on panic). Production code reads
// environment through `var` and `var_os`, which consult these overrides
// first in test builds and read the real process environment otherwise.
//
// A `None` entry unsets the variable for the current thread. Code that
// passes environment to a child process (or another thread) must forward
// the effective values explicitly with `inherit_overrides`, because a
// child only inherits process environment.
#[cfg(test)]
thread_local! {
    static TEST_OVERRIDES: RefCell<HashMap<String, Option<OsString>>> =
        RefCell::new(HashMap::new());
}

/// Read an environment variable, consulting the current thread's test
/// overrides first in test builds. Falls through to process environment
/// for keys no test overrides, so ambient reads are stable: no test can
/// mutate what they see anymore.
pub(crate) fn var(key: &str) -> Result<String, env::VarError> {
    #[cfg(test)]
    {
        if let Some(hit) = TEST_OVERRIDES
            .with(|overrides| overrides.borrow().get(key).cloned())
        {
            return match hit {
                Some(value) => {
                    value.into_string().map_err(env::VarError::NotUnicode)
                }
                None => Err(env::VarError::NotPresent),
            };
        }
    }
    env::var(key)
}

/// `OsString` variant of [`var`].
pub(crate) fn var_os(key: &str) -> Option<OsString> {
    #[cfg(test)]
    {
        if let Some(hit) = TEST_OVERRIDES
            .with(|overrides| overrides.borrow().get(key).cloned())
        {
            return hit;
        }
    }
    env::var_os(key)
}

/// Panic-safe scoped test environment override. Setting an override never
/// touches process environment: it is recorded in the current thread's
/// [`TEST_OVERRIDES`] map and the previous effective value is restored when
/// the guard drops. Hold the guard for the whole body of any test that
/// needs an override.
///
/// This guard is deliberately thread-local rather than a process-wide lock:
/// tests that override different keys (or the same key with the same
/// intent) still run in parallel, and ambient readers need no
/// synchronization because nothing mutates what they read.
#[cfg(test)]
pub(crate) struct TestEnvGuard {
    saved: Vec<(String, Option<OsString>)>,
}

#[cfg(test)]
impl TestEnvGuard {
    /// Override `vars` for the current thread until the guard drops. A
    /// `None` value unsets the variable. Values are `Option<&OsStr>` so
    /// both string literals (`Some(OsStr::new("..."))`) and paths
    /// (`Some(path.as_os_str())`) fit the same call.
    pub(crate) fn set(vars: &[(&str, Option<&OsStr>)]) -> Self {
        let saved = TEST_OVERRIDES.with(|overrides| {
            let mut overrides = overrides.borrow_mut();
            vars.iter()
                .map(|(key, value)| {
                    let old = overrides
                        .get(*key)
                        .cloned()
                        .unwrap_or_else(|| env::var_os(key));
                    overrides.insert(
                        (*key).to_string(),
                        value.map(|value| value.to_os_string()),
                    );
                    ((*key).to_string(), old)
                })
                .collect()
        });
        Self { saved }
    }
}

#[cfg(test)]
impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        TEST_OVERRIDES.with(|overrides| {
            let mut overrides = overrides.borrow_mut();
            for (key, old) in self.saved.drain(..) {
                match old {
                    Some(old) => {
                        overrides.insert(key, Some(old));
                    }
                    None => {
                        overrides.remove(&key);
                    }
                }
            }
        });
    }
}

/// Clone the current thread's test overrides.
///
/// Thread-local overrides do not cross thread boundaries: a thread spawned
/// while overrides are active starts with an empty map. When the test itself
/// spawns the thread that must observe the overrides (for example the lock
/// waiter in `ob::tests::lock_wait_behavior`), capture them here and
/// re-apply them on the new thread with [`TestEnvGuard::set`]:
///
/// ```ignore
/// let snapshot = snapshot_overrides();
/// std::thread::spawn(move || {
///     let scoped: Vec<(&str, Option<&OsStr>)> = snapshot
///         .iter()
///         .map(|(key, value)| (key.as_str(), value.as_deref()))
///         .collect();
///     let _guard = TestEnvGuard::set(&scoped);
///     // ... work that reads overridden variables ...
/// });
/// ```
#[cfg(test)]
pub(crate) fn snapshot_overrides() -> Vec<(String, Option<OsString>)> {
    TEST_OVERRIDES.with(|overrides| {
        overrides
            .borrow()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
}

/// Run `f` with one test override set, restoring afterwards. Thin wrapper
/// over [`TestEnvGuard`] for the common single-key case.
#[cfg(test)]
pub(crate) fn with_var<T>(
    key: &str,
    value: impl Into<OsString>,
    f: impl FnOnce() -> T,
) -> T {
    let owned = value.into();
    let _guard = TestEnvGuard::set(&[(key, Some(owned.as_os_str()))]);
    f()
}

/// Forward the current thread's test overrides to a child process.
///
/// A spawned child inherits process environment, which tests never mutate,
/// so without this the child would miss every override. Call it on the
/// [`Command`] before spawning whenever the child (or the code it runs)
/// reads a variable a test may override. In non-test builds this is a
/// no-op.
pub(crate) fn inherit_overrides(command: &mut Command) {
    #[cfg(test)]
    {
        TEST_OVERRIDES.with(|overrides| {
            for (key, value) in overrides.borrow().iter() {
                match value {
                    Some(value) => {
                        command.env(key, value);
                    }
                    None => {
                        command.env_remove(key);
                    }
                }
            }
        });
    }
    #[cfg(not(test))]
    {
        let _ = command;
    }
}

/// Pin `TZ=UTC0` for timestamp-determinism tests.
///
/// `TZ` is read by libc, not by Rust code, so a thread-local override
/// cannot pin it; and unlike every other test variable it is always set to
/// the same value and never restored. Setting it exactly once per process
/// keeps that behavior while removing the repeated racy mutation.
#[cfg(test)]
pub(crate) fn pin_tz_utc0_for_test() {
    use std::sync::Once;
    static PIN: Once = Once::new();
    PIN.call_once(|| {
        #[allow(clippy::disallowed_methods)]
        unsafe {
            env::set_var("TZ", "UTC0");
        }
    });
}

pub fn bob_dir() -> PathBuf {
    var_os("BOB_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| expand_tilde(&path))
        .unwrap_or_else(|| home_dir().join("bob"))
}

pub fn plugins_dir() -> PathBuf {
    var_os("BOB_PLUGINS_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| expand_tilde(&path))
        .unwrap_or_else(|| {
            home_dir().join("projects/github/bobs-org/bob-plugins")
        })
}

pub fn plugin_backups_dir() -> PathBuf {
    var_os("BOB_PLUGIN_BACKUPS_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| expand_tilde(&path))
        .unwrap_or_else(|| {
            home_dir().join(".local/state/bob-cli/plugin-backups")
        })
}

pub fn home_dir() -> PathBuf {
    var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn state_home() -> PathBuf {
    var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })
        .unwrap_or_else(|| env::temp_dir().join("bob-cli-state"))
}

pub fn bob_cli_state_dir() -> PathBuf {
    state_home().join("bob-cli")
}

/// Create (but do not lock) a machine-wide state lock file: the directory
/// is mode 0700 and the file mode 0600. Shared by the ingest lock and the
/// `ref scan` writer lock so both serialize on the same permissions.
pub(crate) fn create_state_lock_file(
    dir: &Path,
    file_name: &str,
) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        let _ = builder.create(dir);
        let _ = std::fs::set_permissions(
            dir,
            std::fs::Permissions::from_mode(0o700),
        );
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(file_name))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &dir.join(file_name),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    Ok(file)
}

pub fn bob_cli_cache_dir() -> PathBuf {
    var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(|| env::temp_dir().join("bob-cli-cache"))
        .join("bob-cli")
}

pub fn expand_tilde(path: &Path) -> PathBuf {
    let Some(path_text) = path.to_str() else {
        return path.to_path_buf();
    };

    if path_text == "~" {
        return home_dir();
    }

    if let Some(suffix) = path_text.strip_prefix("~/") {
        return home_dir().join(suffix);
    }

    path.to_path_buf()
}

pub fn current_datetime() -> NaiveDateTime {
    if let Some(override_value) =
        var("BOB_NOW").ok().filter(|value| !value.is_empty())
        && let Some(parsed) = parse_datetime_override(&override_value)
    {
        return parsed;
    }

    if let Some(date_value) = var("DATE").ok().filter(|value| !value.is_empty())
    {
        if let Some(parsed) = parse_datetime_override(&date_value) {
            return parsed;
        }

        if let Some(parsed) = date_command_datetime(&date_value) {
            return parsed;
        }
    }

    Local::now().naive_local()
}

pub fn default_day_file(bob_dir: &Path) -> PathBuf {
    let today = current_datetime();
    bob_dir.join(format!("{:04}", today.year())).join(format!(
        "{:04}{:02}{:02}.md",
        today.year(),
        today.month(),
        today.day()
    ))
}

pub fn parse_datetime_override(value: &str) -> Option<NaiveDateTime> {
    let normalized = value.replace('T', " ");
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(&normalized, format) {
            return Some(parsed);
        }
    }

    NaiveDate::parse_from_str(&normalized, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
}

pub fn parse_hhmm(value: &str) -> Option<NaiveTime> {
    let normalized = value.replace(':', "");
    if normalized.len() != 4
        || !normalized.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let hour = normalized[..2].parse().ok()?;
    let minute = normalized[2..].parse().ok()?;
    NaiveTime::from_hms_opt(hour, minute, 0)
}

pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    1
}

pub fn os_to_string(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

/// Resolve the `uv` binary shared by the web-clip and Keep adapters.
///
/// Returns the path plus whether it was found outside `PATH`. `PATH` is
/// checked first; then `$HOME/.local/bin/uv`, `$HOME/.cargo/bin/uv`,
/// `/opt/homebrew/bin/uv`, and `/usr/local/bin/uv`, in that order. The
/// Mac app environment sees only `~/.local/bin`, so the fallbacks keep
/// `bob ref create` and `bob gkeep` working there.
pub fn resolve_uv() -> Option<(PathBuf, bool)> {
    if let Some(path) = find_on_path("uv") {
        return Some((path, false));
    }
    let home = home_dir();
    let mut candidates = vec![
        home.join(".local/bin/uv"),
        home.join(".cargo/bin/uv"),
        PathBuf::from("/opt/homebrew/bin/uv"),
        PathBuf::from("/usr/local/bin/uv"),
    ];
    candidates.retain(|path| is_executable_file(path));
    candidates.into_iter().next().map(|path| (path, true))
}

/// Look `name` up on `PATH`, returning an executable file if found.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| is_executable_file(path))
}

#[cfg(unix)]
pub fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn date_command_datetime(date_command: &str) -> Option<NaiveDateTime> {
    let output = run_date_command(date_command, ["+%Y-%m-%d %H:%M:%S"])?;
    parse_datetime_override(output.trim())
}

fn run_date_command<const N: usize>(
    date_command: &str,
    args: [&str; N],
) -> Option<String> {
    let parts = split_command(date_command);
    let (program, command_args) = parts.split_first()?;
    let output = Command::new(program)
        .args(command_args)
        .args(args)
        .output()
        .ok()?;

    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn split_command(command: &str) -> Vec<OsString> {
    command.split_whitespace().map(OsString::from).collect()
}
