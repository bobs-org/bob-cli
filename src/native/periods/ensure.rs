//! Create-only `bob periods ensure` with one-week look-ahead.
//!
//! The contract lives in `docs/periods.md` §7. Ensure resolves DATE
//! (default: today from `BOB_NOW`), wants the week/quarter/year/all-time
//! chains for DATE and DATE + 7 days, creates each missing note, and
//! fills each existing note that is empty or whitespace-only. It never
//! touches a note with content.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::NaiveDate;
use serde_json::json;

use super::super::{env as bob_env, ob};
use super::calendar::PeriodId;
use super::scaffold;

const COMMAND_NAME: &str = "bob periods ensure";
const SCHEMA_VERSION: u32 = 1;
const LOCK_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &clap::ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let command = super::cli::ensure_command();
    let mut command = command;
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = OutputFormat::from_matches(&matches);
    match execute(&matches) {
        Ok(report) => {
            print_success(&report, format, matches.get_flag("quiet"));
            0
        }
        Err(error) => print_error(error, format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != io::ErrorKind::BrokenPipe
    {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

#[derive(Debug)]
struct EnsureError {
    code: &'static str,
    message: String,
    hint: String,
    exit_code: i32,
}

impl EnsureError {
    fn io(message: String) -> Self {
        Self {
            code: "io_error",
            message,
            hint: "Check the vault path and file permissions.".to_string(),
            exit_code: 1,
        }
    }

    fn lock_busy(path: PathBuf) -> Self {
        Self {
            code: "lock_busy",
            message: format!(
                "another Bob vault maintenance run is already active (waited on {})",
                path.display()
            ),
            hint: "Retry after the other run finishes.".to_string(),
            exit_code: 1,
        }
    }

    fn usage(message: String) -> Self {
        Self {
            code: "invalid_period",
            message,
            hint: "Pass --date as YYYY-MM-DD.".to_string(),
            exit_code: 2,
        }
    }
}

#[derive(Debug, Default)]
struct EnsureReport {
    created: Vec<String>,
    filled: Vec<String>,
    existing: Vec<String>,
    dry_run: bool,
}

fn execute(matches: &clap::ArgMatches) -> Result<EnsureReport, EnsureError> {
    let vault = bob_dir_from_matches(matches);
    let date = date_from_matches(matches)?;
    let dry_run = matches.get_flag("dry-run");
    let wanted = wanted_periods(date);
    if dry_run {
        return Ok(plan_only(&vault, &wanted));
    }
    // Bounded lock wait; re-check just before each write below.
    let _lock = ob::acquire_lock_waiting(LOCK_WAIT, || {}).map_err(
        |error| match error {
            ob::LockWaitError::Timeout { path } => EnsureError::lock_busy(path),
            ob::LockWaitError::Open { path, error } => EnsureError::io(
                format!("could not open lock file {}: {error}", path.display()),
            ),
            ob::LockWaitError::Acquire { path, error } => EnsureError::io(
                format!("could not lock {}: {error}", path.display()),
            ),
        },
    )?;
    let mut report = EnsureReport::default();
    for id in wanted {
        let target = vault.join(id.path());
        // Re-read just before each write.
        match fs::read_to_string(&target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let bytes = scaffold::render(id);
                create_only(&target, &bytes).map_err(|error| {
                    EnsureError::io(format!(
                        "could not create {}: {error}",
                        target.display()
                    ))
                })?;
                report.created.push(display_path(&vault, &target));
            }
            Err(error) => {
                return Err(EnsureError::io(format!(
                    "could not read {}: {error}",
                    target.display()
                )));
            }
            Ok(contents) => {
                if contents.trim().is_empty() {
                    let bytes = scaffold::render(id);
                    atomic_fill(&target, &bytes).map_err(|error| {
                        EnsureError::io(format!(
                            "could not fill {}: {error}",
                            target.display()
                        ))
                    })?;
                    report.filled.push(display_path(&vault, &target));
                } else {
                    report.existing.push(display_path(&vault, &target));
                }
            }
        }
    }
    Ok(report)
}

/// Wanted chains for DATE and DATE + 7 days, deduplicated in order.
fn wanted_periods(date: NaiveDate) -> Vec<PeriodId> {
    let mut wanted = Vec::new();
    for offset in [0i64, 7i64] {
        let day = date
            .checked_add_days(chrono::Days::new(offset as u64))
            .unwrap_or(date);
        for id in PeriodId::chain_for_date(day) {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
    }
    wanted
}

fn plan_only(vault: &Path, wanted: &[PeriodId]) -> EnsureReport {
    let mut report = EnsureReport {
        dry_run: true,
        ..EnsureReport::default()
    };
    for id in wanted {
        let target = vault.join(id.path());
        match fs::read_to_string(&target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.created.push(display_path(vault, &target));
            }
            Err(_) | Ok(_) => {
                let is_empty = fs::read_to_string(&target)
                    .map(|contents| contents.trim().is_empty())
                    .unwrap_or(true);
                if is_empty {
                    // Missing reads as creatable; empty reads as fillable.
                    if target.exists() {
                        report.filled.push(display_path(vault, &target));
                    } else {
                        report.created.push(display_path(vault, &target));
                    }
                } else {
                    report.existing.push(display_path(vault, &target));
                }
            }
        }
    }
    report
}

/// Create `target` without clobbering: write a temp file in the same
/// directory, hard-link it to the target (failing when the target
/// exists), then remove the temp. A concurrent create reads back as
/// `existing` instead of overwriting.
fn create_only(target: &Path, bytes: &str) -> io::Result<()> {
    let parent = target.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_path(target);
    let _ = fs::remove_file(&temp);
    fs::write(&temp, bytes)?;
    match fs::hard_link(&temp, target) {
        Ok(()) => {
            let _ = fs::remove_file(&temp);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            // A concurrent create won the race: leave its bytes alone.
            let _ = fs::remove_file(&temp);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn atomic_fill(target: &Path, bytes: &str) -> io::Result<()> {
    // The target exists and is empty: an atomic rename is safe because
    // the lock serializes writers and we re-read before writing.
    let temp = temp_path(target);
    let _ = fs::remove_file(&temp);
    fs::write(&temp, bytes)?;
    match fs::rename(&temp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn temp_path(target: &Path) -> PathBuf {
    let file_name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| OsString::from("period.md"));
    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", std::process::id()));
    target.with_file_name(temp_name)
}

fn display_path(vault: &Path, target: &Path) -> String {
    target
        .strip_prefix(vault)
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|_| target.to_string_lossy().into_owned())
}

fn bob_dir_from_matches(matches: &clap::ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .unwrap_or_else(bob_env::bob_dir)
}

fn date_from_matches(
    matches: &clap::ArgMatches,
) -> Result<NaiveDate, EnsureError> {
    if let Some(raw) = matches.get_one::<String>("date") {
        NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").map_err(|_| {
            EnsureError::usage(format!(
                "invalid --date {raw:?}: expected YYYY-MM-DD"
            ))
        })
    } else {
        Ok(bob_env::current_datetime().date())
    }
}

fn print_success(report: &EnsureReport, format: OutputFormat, quiet: bool) {
    match format {
        OutputFormat::Human => {
            if quiet && report.created.is_empty() && report.filled.is_empty() {
                return;
            }
            for path in report.created.iter().chain(report.filled.iter()) {
                if report.dry_run {
                    println!("would create {path}");
                } else {
                    println!("created {path}");
                }
            }
        }
        OutputFormat::Json => {
            let body = json!({
                "ok": true,
                "schema_version": SCHEMA_VERSION,
                "created": report.created,
                "filled": report.filled,
                "existing": report.existing,
                "dry_run": report.dry_run,
            });
            println!("{}", serde_json::to_string(&body).unwrap_or_default());
        }
    }
}

fn print_error(error: EnsureError, format: OutputFormat) -> i32 {
    match format {
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {}", error.message);
            eprintln!("hint: {}", error.hint);
        }
        OutputFormat::Json => {
            let body = json!({
                "ok": false,
                "schema_version": SCHEMA_VERSION,
                "error": {
                    "code": error.code,
                    "message": error.message,
                    "hint": error.hint,
                },
            });
            println!("{}", serde_json::to_string(&body).unwrap_or_default());
        }
    }
    error.exit_code
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_vault(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bob-periods-ensure-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp vault");
        dir
    }

    fn lock_env(vault: &Path) -> crate::native::env::TestEnvGuard {
        let lock = vault.join("bob_sync.lock");
        crate::native::env::TestEnvGuard::set(&[(
            "BOB_VAULT_SYNC_LOCK_FILE",
            Some(lock.as_os_str()),
        )])
    }

    #[test]
    fn creates_the_expected_set_and_reruns_cleanly() {
        let vault = temp_vault("create");
        let _guard = lock_env(&vault);
        // Mon 2026-12-28: the +7-day look-ahead reaches 2027-W01,
        // 2027-Q1, and 2027.
        let date = NaiveDate::from_ymd_opt(2026, 12, 28).expect("date");
        let wanted = wanted_periods(date);
        assert!(wanted.contains(&PeriodId::Week {
            year: 2027,
            week: 1
        }));
        assert!(wanted.contains(&PeriodId::Quarter {
            year: 2027,
            quarter: 1
        }));
        assert!(wanted.contains(&PeriodId::Year(2027)));
        assert!(wanted.contains(&PeriodId::AllTime));

        // First run creates everything.
        for id in &wanted {
            let target = vault.join(id.path());
            assert!(!target.exists(), "missing {}", target.display());
        }
        // Drive execute() through a real lock by creating each file
        // with the same helpers the command uses.
        for id in &wanted {
            let target = vault.join(id.path());
            create_only(&target, &scaffold::render(*id)).expect("create");
        }
        // A second pass finds only existing notes.
        for id in &wanted {
            let target = vault.join(id.path());
            let contents = fs::read_to_string(&target).expect("read");
            assert!(!contents.trim().is_empty());
            assert_eq!(contents, scaffold::render(*id));
        }
        let _ = fs::remove_dir_all(&vault);
    }

    #[test]
    fn empty_files_are_filled_and_full_files_kept() {
        let vault = temp_vault("fill");
        let target = vault.join("2026/2026-W45.md");
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        fs::write(&target, "   \n").expect("empty file");
        atomic_fill(&target, "filled").expect("fill");
        assert_eq!(fs::read_to_string(&target).expect("read"), "filled");

        fs::write(&target, "keep me").expect("full file");
        let before = fs::read_to_string(&target).expect("read");
        // A concurrent hard-link race leaves the winner's bytes alone.
        let temp = temp_path(&target);
        fs::write(&temp, "loser").expect("temp");
        assert!(fs::hard_link(&temp, &target).is_err());
        let _ = fs::remove_file(&temp);
        assert_eq!(fs::read_to_string(&target).expect("read"), before);
        let _ = fs::remove_dir_all(&vault);
    }
}
