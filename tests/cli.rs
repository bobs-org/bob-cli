use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const BOB_POMODORO_RUNTIMES_BIN: &str =
    env!("CARGO_BIN_EXE_bob_pomodoro_runtimes");
const STOPWATCH: &str = "\u{23f1}\u{fe0f}";

#[test]
fn cache_extraction_writes_expected_files_and_modes() {
    let temp = TempDir::new("bob-cli-cache");
    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg("--help")
        .env("BOB_CLI_USE_SCRIPT", "1")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run bob pomodoro-runtimes --help");

    assert_success(&output);

    let script_dir = single_script_cache_dir(temp.path());
    let executable_assets = [
        "bob_pomodoro",
        "bob_pomodoro_runtimes",
        "bob_notify",
        "bob_sync",
        "tmux_bob_pomodoro",
    ];

    for asset in executable_assets {
        let path = script_dir.join(asset);
        assert!(
            path.is_file(),
            "missing extracted asset: {}",
            path.display()
        );
        assert_unix_mode(&path, 0o755);
    }

    let helper = script_dir.join("lib/bob_shell.sh");
    assert!(
        helper.is_file(),
        "missing extracted helper: {}",
        helper.display()
    );
    assert_unix_mode(&helper, 0o644);
}

#[test]
fn pomodoro_runtimes_help_runs_without_script_cache() {
    let temp = TempDir::new("bob-cli-native-help");
    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg("--help")
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("run native bob pomodoro-runtimes --help");

    assert_success(&output);
    assert!(
        stdout(&output).contains("Annotate completed Bob Pomodoro"),
        "expected native help text:\n{}",
        format_output(&output)
    );
    assert!(
        !temp.path().join("bob-cli/scripts").exists(),
        "native help should not extract script assets"
    );
}

#[test]
fn pass_through_arguments_and_exit_statuses_reach_runtimes_command() {
    let temp = TempDir::new("bob-cli-runtimes-check");
    let stub_bin = temp.path().join("bin");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );

    let note = temp.path().join("needs_runtime_suffixes.md");
    fs::copy(
        fixture("pomodoro_runtimes/needs_runtime_suffixes.md"),
        &note,
    )
    .expect("copy runtime fixture");

    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg("--check")
        .arg(&note)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro-runtimes --check");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected child status 1, got:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("would update"),
        "expected runtimes command to receive --check:\n{}",
        format_output(&output)
    );
}

#[test]
fn pomodoro_runtimes_updates_notes_and_is_idempotent() {
    let temp = TempDir::new("bob-cli-runtimes-update");
    let stub_bin = temp.path().join("bin");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );

    let note = temp.path().join("legacy_runtime_suffixes.md");
    fs::copy(
        fixture("pomodoro_runtimes/legacy_runtime_suffixes.md"),
        &note,
    )
    .expect("copy legacy runtime fixture");

    let first = bob_command()
        .arg("pomodoro-runtimes")
        .arg(&note)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro-runtimes");

    assert_success(&first);
    assert!(
        stdout(&first).contains("updated:"),
        "expected first run to update note:\n{}",
        format_output(&first)
    );

    let contents = fs::read_to_string(&note).expect("read updated note");
    assert!(contents.contains(&format!("## Pomodoros {STOPWATCH} 50m")));
    assert!(contents.contains(&format!(
        "Replace legacy runtime suffix (09:00-09:25) {STOPWATCH} 25m"
    )));
    assert!(!contents.contains("[runtime::"));

    let second = bob_command()
        .arg("pomodoro-runtimes")
        .arg(&note)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("rerun bob pomodoro-runtimes");

    assert_success(&second);
    assert!(
        stdout(&second).is_empty(),
        "second run should be idempotent:\n{}",
        format_output(&second)
    );
}

#[test]
fn legacy_pomodoro_runtimes_shim_uses_native_implementation() {
    let temp = TempDir::new("bob-cli-runtimes-shim");
    let stub_bin = temp.path().join("bin");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );

    let note = temp.path().join("needs_runtime_suffixes.md");
    fs::copy(
        fixture("pomodoro_runtimes/needs_runtime_suffixes.md"),
        &note,
    )
    .expect("copy runtime fixture");

    let output = Command::new(BOB_POMODORO_RUNTIMES_BIN)
        .arg("--check")
        .arg(&note)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run legacy bob_pomodoro_runtimes shim");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected child status 1, got:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("would update"),
        "expected shim to run native check path:\n{}",
        format_output(&output)
    );
}

#[test]
fn pomodoro_runtimes_reports_missing_note_without_touching_real_vault() {
    let temp = TempDir::new("bob-cli-runtimes-missing");
    let stub_bin = temp.path().join("bin");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
  exit 0
fi
exit 64
"#,
    );

    let missing = temp.path().join("missing-day.md");
    let output = bob_command()
        .arg("pomodoro-runtimes")
        .arg(&missing)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("BOB_DIR", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro-runtimes with missing note");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected missing note status 2:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("note not found"),
        "expected missing note error:\n{}",
        format_output(&output)
    );
}

#[test]
fn tmux_pomodoro_formats_native_pomodoro_status() {
    let temp = TempDir::new("bob-cli-tmux-path");
    let output = bob_command()
        .arg("tmux-pomodoro")
        .env(
            "BOB_DAY_FILE",
            fixture("pomodoro/day_with_open_pomodoro.md"),
        )
        .env("BOB_NOW", "2026-06-01 09:10:01")
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob tmux-pomodoro");

    assert_success(&output);
    assert_eq!(stdout(&output), "[<65m] 0945-1015 Review crate skeleton | ");
}

#[test]
fn pomodoro_missing_day_file_is_a_successful_noop() {
    let temp = TempDir::new("bob-cli-pomodoro-missing");
    let output = bob_command()
        .arg("pomodoro")
        .env("BOB_DAY_FILE", temp.path().join("missing-day.md"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob pomodoro with missing day file");

    assert_success(&output);
    assert!(stdout(&output).is_empty(), "expected empty stdout");
    assert!(stderr(&output).is_empty(), "expected empty stderr");
}

#[test]
fn bob_sync_uses_stubbed_ob_and_git_commands() {
    let temp = TempDir::new("bob-cli-sync");
    let stub_bin = temp.path().join("bin");
    let vault = temp.path().join("vault");
    let home = temp.path().join("home");
    let log = temp.path().join("commands.log");
    fs::create_dir_all(&stub_bin).expect("create stub bin");
    fs::create_dir_all(&vault).expect("create vault");
    fs::create_dir_all(&home).expect("create home");

    write_executable(
        &stub_bin.join("ob"),
        r#"#!/bin/sh
printf 'ob %s\n' "$*" >> "$STUB_LOG"
case "$1" in
  sync|sync-status) exit 0 ;;
esac
exit 64
"#,
    );
    write_executable(
        &stub_bin.join("git"),
        r#"#!/bin/sh
printf 'git %s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "-C" ]; then
  shift 2
fi
case "$1" in
  rev-parse)
    printf 'true\n'
    exit 0
    ;;
  add)
    exit 0
    ;;
  diff)
    exit 0
    ;;
  push)
    exit 0
    ;;
esac
exit 64
"#,
    );

    let output = bob_command()
        .arg("sync")
        .env("BOB_DIR", &vault)
        .env("BOB_SYNC_LOCK_FILE", temp.path().join("bob_sync.lock"))
        .env("HOME", &home)
        .env("PATH", path_with_prefix(&stub_bin))
        .env("STUB_LOG", &log)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .output()
        .expect("run bob sync");

    assert_success(&output);
    let log_contents = fs::read_to_string(&log).expect("read stub command log");
    assert!(
        log_contents.contains(&format!("ob sync --path {}", vault.display()))
    );
    assert!(log_contents
        .contains(&format!("ob sync-status --path {}", vault.display())));
    assert!(log_contents.contains(&format!(
        "git -C {} rev-parse --is-inside-work-tree",
        vault.display()
    )));
    assert!(
        log_contents.contains(&format!("git -C {} add -A .", vault.display()))
    );
    assert!(log_contents.contains(&format!(
        "git -C {} diff --cached --quiet --exit-code",
        vault.display()
    )));
    assert!(log_contents.contains(&format!("git -C {} push", vault.display())));
    assert!(
        !log_contents.contains(" commit "),
        "no-change sync should not commit"
    );
}

fn bob_command() -> Command {
    Command::new(BOB_BIN)
}

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(relative)
}

fn single_script_cache_dir(cache_home: &Path) -> PathBuf {
    let scripts_root = cache_home.join("bob-cli/scripts");
    let mut entries: Vec<_> = fs::read_dir(&scripts_root)
        .unwrap_or_else(|error| {
            panic!("read script cache root {}: {error}", scripts_root.display())
        })
        .map(|entry| entry.expect("read cache entry").path())
        .collect();

    entries.sort();
    assert_eq!(entries.len(), 1, "expected one script cache directory");
    entries.pop().expect("script cache directory")
}

fn path_with_prefix(prefix: &Path) -> String {
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![prefix.to_path_buf()];
    paths.extend(std::env::split_paths(&current_path));
    std::env::join_paths(paths)
        .expect("join PATH")
        .into_string()
        .expect("PATH is UTF-8")
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|error| {
        panic!("write executable stub {}: {error}", path.display())
    });
    set_mode(path, 0o755);
}

#[cfg(unix)]
fn assert_unix_mode(path: &Path, expected: u32) {
    let mode = fs::metadata(path)
        .unwrap_or_else(|error| panic!("stat {}: {error}", path.display()))
        .mode()
        & 0o777;
    assert_eq!(mode, expected, "unexpected mode for {}", path.display());
}

#[cfg(not(unix))]
fn assert_unix_mode(_path: &Path, _expected: u32) {}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions)
        .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command success:\n{}",
        format_output(output)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn format_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(output),
        stderr(output)
    )
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
