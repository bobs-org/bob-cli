//! `bob gkeep doctor`: diagnose the Keep setup as a checklist.
//!
//! Sequential checks — config, account, token, adapter, Keep, target
//! note, and git — each give `ok`, `warn`, `fail`, or `skip`, plus a
//! JSON report with `-f json`. Later checks skip with a reason when a
//! prerequisite failed. Tokens never appear in any output.

use std::{fs, path::Path, process::Command, time::UNIX_EPOCH};

use serde_json::json;

use super::super::{config as bob_config, env as bob_env, ob, style::Styler};
use super::{
    adapter::AdapterClient,
    config::{derive_device_id, GkeepConfig, TokenShape},
    ui, DoctorArgs, GkeepError,
};

/// One checklist row.
struct Check {
    name: &'static str,
    status: CheckStatus,
    summary: String,
    hint: Option<String>,
}

/// The per-check outcome: `ok`, `warn`, `fail`, or `skip`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }

    fn glyph(self, styler: &Styler) -> String {
        match self {
            Self::Ok => styler.green("✓"),
            Self::Warn => styler.yellow("!"),
            Self::Fail => styler.red("✗"),
            Self::Skip => styler.dim("·"),
        }
    }
}

pub(crate) fn run(args: &DoctorArgs) -> i32 {
    let bob_dir = args.bob_dir();
    let config = match GkeepConfig::resolve(None) {
        Ok(config) => config,
        Err(error) => {
            let mut checks = vec![config_fail_check(&error)];
            for name in ["account", "uv", "token", "adapter", "keep", "target"]
            {
                checks.push(skipped(name, "no gkeep config"));
            }
            // The git check needs no config, so it still runs.
            checks.push(git_check(&bob_dir));
            return finish(args, checks);
        }
    };

    let mut checks =
        vec![config_ok_check(), account_check(&config), uv_check()];
    let (token_check, token) = token_check(&config);
    checks.push(token_check);
    let (adapter_check, client) = adapter_check(&config);
    checks.push(adapter_check);
    match (token, client) {
        (Some(master_token), Some(client)) => {
            checks.push(keep_check(&config, &client, &master_token));
        }
        (None, _) => {
            checks.push(skipped("keep", "no working token"));
        }
        (_, None) => {
            checks.push(skipped("keep", "no working adapter"));
        }
    }
    checks.push(target_check(&config, &bob_dir));
    checks.push(git_check(&bob_dir));
    finish(args, checks)
}

/// The `config` row when the config file reads and validates.
fn config_ok_check() -> Check {
    Check {
        name: "config",
        status: CheckStatus::Ok,
        summary: format!(
            "{} · gkeep section",
            display_path(&bob_config::config_path())
        ),
        hint: None,
    }
}

/// The `config` row when resolution fails; everything config-gated
/// skips after it.
fn config_fail_check(error: &GkeepError) -> Check {
    Check {
        name: "config",
        status: CheckStatus::Fail,
        summary: format!(
            "{} · {}",
            display_path(&bob_config::config_path()),
            error.message()
        ),
        hint: error.hint().map(str::to_string),
    }
}

/// A skipped row with its prerequisite reason.
fn skipped(name: &'static str, reason: &str) -> Check {
    Check {
        name,
        status: CheckStatus::Skip,
        summary: format!("skipped: {reason}"),
        hint: None,
    }
}

/// The `account` row: email plus the device id, marked derived or
/// configured. Resolution already validated both, so this never fails.
fn account_check(config: &GkeepConfig) -> Check {
    let derived = config.device_id() == derive_device_id(config.email());
    Check {
        name: "account",
        status: CheckStatus::Ok,
        summary: format!(
            "{} · device {} ({})",
            config.email(),
            short_device_id(config.device_id()),
            if derived { "derived" } else { "configured" }
        ),
        hint: None,
    }
}

/// The first 8 hex digits plus `…` when the id is longer.
fn short_device_id(device_id: &str) -> String {
    if device_id.len() > 8 {
        format!("{}…", &device_id[..8])
    } else {
        device_id.to_string()
    }
}

/// The `token` row plus the working token, if any. A sign-in cookie is
/// a fail; an unrecognized shape warns but is still tried.
fn token_check(config: &GkeepConfig) -> (Check, Option<String>) {
    let command = config.token_command().to_string();
    match config.read_token() {
        Ok((token, TokenShape::MasterToken)) => (
            Check {
                name: "token",
                status: CheckStatus::Ok,
                summary: format!("{command} · master token (aas_et/…)"),
                hint: None,
            },
            Some(token),
        ),
        Ok((token, TokenShape::Unknown)) => (
            Check {
                name: "token",
                status: CheckStatus::Warn,
                summary: format!(
                    "{command} · unrecognized token shape; trying it anyway"
                ),
                hint: None,
            },
            Some(token),
        ),
        // `read_token` rejects cookies with an error before this arm;
        // it stays for exhaustiveness and never yields a token.
        Ok((_, TokenShape::SignInCookie)) => (
            Check {
                name: "token",
                status: CheckStatus::Fail,
                summary: format!(
                    "{command} · the stored value is a sign-in cookie, not \
                     a master token"
                ),
                hint: Some("run `bob gkeep login`".to_string()),
            },
            None,
        ),
        Err(error) => (
            Check {
                name: "token",
                status: CheckStatus::Fail,
                summary: format!("{command} · {}", error.message()),
                hint: error.hint().map(str::to_string),
            },
            None,
        ),
    }
}

/// The `adapter` row plus the resolved client: `ping`, with the `uv`
/// version when the bundled adapter runs.
fn adapter_check(config: &GkeepConfig) -> (Check, Option<AdapterClient>) {
    let client = match AdapterClient::resolve(config) {
        Ok(client) => client,
        Err(error) => {
            return (
                Check {
                    name: "adapter",
                    status: CheckStatus::Fail,
                    summary: error.message().to_string(),
                    hint: error.hint().map(str::to_string),
                },
                None,
            );
        }
    };
    match client.ping(None) {
        Ok(ping) => {
            let mut parts = Vec::new();
            if let Some(uv) = uv_version() {
                parts.push(uv);
            }
            parts.push(format!("Python {}", ping.python));
            parts.push(format!("gkeepapi {}", ping.gkeepapi));
            parts.push(format!("gpsoauth {}", ping.gpsoauth));
            (
                Check {
                    name: "adapter",
                    status: CheckStatus::Ok,
                    summary: parts.join(" · "),
                    hint: None,
                },
                Some(client),
            )
        }
        Err(error) => (
            Check {
                name: "adapter",
                status: CheckStatus::Fail,
                summary: error.message().to_string(),
                hint: error.hint().map(str::to_string),
            },
            None,
        ),
    }
}

/// The `uv` row: the resolved binary (`PATH` first, then the well-known
/// install locations), with an `(outside PATH)` marker when applicable.
/// It warns only when uv is missing everywhere.
fn uv_check() -> Check {
    match bob_env::resolve_uv() {
        Some((path, outside_path)) => {
            let mut summary = format!("available ({})", path.display());
            if outside_path {
                summary.push_str(" (outside PATH)");
            }
            Check {
                name: "uv",
                status: CheckStatus::Ok,
                summary,
                hint: None,
            }
        }
        None => Check {
            name: "uv",
            status: CheckStatus::Warn,
            summary: "uv not found on PATH".to_string(),
            hint: Some(
                "install uv (https://docs.astral.sh/uv/) — bob gkeep runs \
                 its pinned Google Keep adapter with it"
                    .to_string(),
            ),
        },
    }
}

/// `uv --version` (for example `uv 0.11.8`), only when the bundled
/// adapter runs instead of `BOB_GKEEP_ADAPTER`.
fn uv_version() -> Option<String> {
    if GkeepConfig::adapter_override().is_some() {
        return None;
    }
    let (uv, _) = bob_env::resolve_uv()?;
    let output = Command::new(uv).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// The `keep` row: a `snapshot` proving reachability, with inbox and
/// pinned counts plus the state-cache age.
fn keep_check(
    config: &GkeepConfig,
    client: &AdapterClient,
    master_token: &str,
) -> Check {
    let credentials =
        super::adapter::Credentials::from_config(config, master_token);
    match client.snapshot(&credentials, false, None) {
        Ok(notes) => {
            let pinned = notes.iter().filter(|note| note.pinned).count();
            Check {
                name: "keep",
                status: CheckStatus::Ok,
                summary: format!(
                    "reachable · {} · {pinned} pinned · {}",
                    inbox_count(notes.len()),
                    state_cache_summary(),
                ),
                hint: None,
            }
        }
        Err(error) => Check {
            name: "keep",
            status: CheckStatus::Fail,
            summary: error.message().to_string(),
            hint: error.hint().map(str::to_string),
        },
    }
}

/// `1 in inbox` or `N in inbox`.
fn inbox_count(count: usize) -> String {
    if count == 1 {
        "1 in inbox".to_string()
    } else {
        format!("{count} in inbox")
    }
}

/// The state-cache age from the cache dir, or that there is no cache.
fn state_cache_summary() -> String {
    let path = bob_env::bob_cli_cache_dir()
        .join("gkeep")
        .join("state.json");
    let modified = fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_secs() as i64);
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|age| age.as_secs() as i64)
        .unwrap_or(0);
    match modified {
        None => "no state cache yet".to_string(),
        Some(modified) => {
            let age = ui::format_age(now, modified);
            if age == "now" {
                "state cache just synced".to_string()
            } else {
                format!("state cache {age} old")
            }
        }
    }
}

/// The `target` row: the target note must exist, and a missing Tasks
/// heading warns that `pull` appends after the last task.
fn target_check(config: &GkeepConfig, bob_dir: &Path) -> Check {
    let path = config.target_path(bob_dir);
    let display = display_path(&path);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Check {
                name: "target",
                status: CheckStatus::Fail,
                summary: format!("{display} · missing"),
                hint: Some("create it or set gkeep.target".to_string()),
            };
        }
        Err(error) => {
            return Check {
                name: "target",
                status: CheckStatus::Fail,
                summary: format!("{display} · read the target note: {error}"),
                hint: None,
            };
        }
    };
    if contents.lines().any(is_tasks_heading) {
        Check {
            name: "target",
            status: CheckStatus::Ok,
            summary: format!("{display} · Tasks section found"),
            hint: None,
        }
    } else {
        Check {
            name: "target",
            status: CheckStatus::Warn,
            summary: format!(
                "{display} · no Tasks heading; pull appends after the last \
                 task"
            ),
            hint: None,
        }
    }
}

/// Whether `line` is a Markdown heading for the Tasks section.
fn is_tasks_heading(line: &str) -> bool {
    let trimmed = line.trim();
    let hashes = trimmed.chars().take_while(|&cell| cell == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    let rest = &trimmed[hashes..];
    rest.starts_with([' ', '\t']) && rest.trim() == "Tasks"
}

/// The `git` row: a worktree commits before archiving; anything else
/// warns that `pull` will not commit.
fn git_check(bob_dir: &Path) -> Check {
    match ob::detect_git_worktree(bob_dir, &ob::child_env()) {
        Ok(true) => Check {
            name: "git",
            status: CheckStatus::Ok,
            summary: "vault is a Git worktree · pull commits before \
                      archiving"
                .to_string(),
            hint: None,
        },
        Ok(false) => Check {
            name: "git",
            status: CheckStatus::Warn,
            summary: "not a Git worktree · pull will not commit".to_string(),
            hint: None,
        },
        Err(error) => Check {
            name: "git",
            status: CheckStatus::Warn,
            summary: format!("could not check git: {error}"),
            hint: None,
        },
    }
}

/// Collapse a leading home dir to `~` for short summaries.
fn display_path(path: &Path) -> String {
    display_path_with_home(path, &bob_env::home_dir())
}

fn display_path_with_home(path: &Path, home: &Path) -> String {
    if let Ok(rest) = path.strip_prefix(home) {
        if rest.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

/// Print the human checklist or the JSON report; exit 0 unless a check
/// failed.
fn finish(args: &DoctorArgs, checks: Vec<Check>) -> i32 {
    let failed = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();
    let warned = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Warn)
        .count();
    if args.format.is_json() {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": failed == 0,
                "checks": checks
                    .iter()
                    .map(|check| json!({
                        "name": check.name,
                        "status": check.status.as_str(),
                        "summary": check.summary,
                        "hint": check.hint,
                    }))
                    .collect::<Vec<_>>(),
            })
        );
        return if failed == 0 { 0 } else { 1 };
    }

    let styler = Styler::detect();
    println!("{}", styler.cyan("Google Keep doctor"));
    println!();
    let width = checks
        .iter()
        .map(|check| check.name.len())
        .max()
        .unwrap_or(0);
    for check in &checks {
        println!(
            "  {} {:width$} {}",
            check.status.glyph(&styler),
            check.name,
            check.summary,
            width = width,
        );
        if let Some(hint) = &check.hint
            && !matches!(check.status, CheckStatus::Ok | CheckStatus::Skip)
        {
            println!("      {}", styler.dim(&format!("hint: {hint}")));
        }
    }
    println!();
    if failed > 0 {
        println!(
            "{}",
            styler.red(&format!(
                "error {}",
                plural(failed, "1 check failed", "checks failed")
            ))
        );
        1
    } else if warned > 0 {
        println!(
            "{} {}",
            styler.warning_prefix(),
            plural(warned, "1 warning", "warnings")
        );
        0
    } else {
        println!("{} all checks passed", styler.success_prefix(false));
        0
    }
}

/// `plural(1, "1 warning", "warnings")` → `1 warning`, else `N warnings`.
fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        one.to_string()
    } else {
        format!("{count} {many}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_heading_matches_headings_only() {
        assert!(is_tasks_heading("## Tasks"));
        assert!(is_tasks_heading("   ### Tasks  "));
        assert!(!is_tasks_heading("## Tasks today"));
        assert!(!is_tasks_heading("##Tasks"));
        assert!(!is_tasks_heading("- [ ] ## Tasks"));
        assert!(!is_tasks_heading("####### Tasks"));
        assert!(!is_tasks_heading("Tasks"));
    }

    #[test]
    fn home_prefix_collapses_to_tilde() {
        let home = Path::new("/home/tester");
        assert_eq!(
            display_path_with_home(
                Path::new("/home/tester/bob/gkeep_inbox.md"),
                home
            ),
            "~/bob/gkeep_inbox.md"
        );
        assert_eq!(
            display_path_with_home(Path::new("/tmp/x.md"), home),
            "/tmp/x.md"
        );
        assert_eq!(display_path_with_home(home, home), "~");
    }

    #[test]
    fn device_id_shortens_to_eight_plus_ellipsis() {
        assert_eq!(short_device_id("3f9c0a1b2c3d4e5f"), "3f9c0a1b…");
        assert_eq!(short_device_id("abc"), "abc");
    }

    #[test]
    fn check_status_names_match_the_json_contract() {
        assert_eq!(CheckStatus::Ok.as_str(), "ok");
        assert_eq!(CheckStatus::Warn.as_str(), "warn");
        assert_eq!(CheckStatus::Fail.as_str(), "fail");
        assert_eq!(CheckStatus::Skip.as_str(), "skip");
    }
}
