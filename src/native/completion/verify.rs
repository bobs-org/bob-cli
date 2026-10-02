//! Real-shell verification for `bob completion`.
//!
//! Install and `status -v` classify registration with one bounded `zsh -ic`
//! probe each. The probe runs in its own process group with stdin closed
//! and `BOB_COMPLETION_PROBE=1`, and is killed at the deadline (8 s, or the
//! hidden `BOB_COMPLETION_PROBE_TIMEOUT_MS` override for tests). Probe
//! scripts carry `# bob-completion-*` markers so tests can fake `zsh` by
//! branching on the script text; real rc noise is ignored by parsing only
//! the lines between `bob-*-start` and `bob-*-end`.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::cli::Shell;

const DEFAULT_PROBE_TIMEOUT_MS: u64 = 8000;

/// Outcome of running a bounded probe.
enum ProbeOutcome {
    Output(String),
    TimedOut,
    NotFound(String),
}

pub(crate) fn probe_timeout_ms() -> u64 {
    std::env::var("BOB_COMPLETION_PROBE_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_PROBE_TIMEOUT_MS)
}

fn run_bounded_zsh(script: &str, timeout_ms: u64) -> ProbeOutcome {
    let mut command = Command::new("zsh");
    command
        .args(["-i", "-c", script])
        .env("BOB_COMPLETION_PROBE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ProbeOutcome::NotFound(format!("zsh not found ({error})"));
        }
    };
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return ProbeOutcome::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let _ = child.wait();
    ProbeOutcome::Output(stdout)
}

/// Read `$fpath` through a bounded `zsh -ic` probe.
///
/// Returns `None` when zsh is missing, the probe times out, or the output
/// has no marker block; callers fall back to probe-free defaults.
pub(crate) fn fpath_entries() -> Option<Vec<PathBuf>> {
    let script = "# bob-completion-fpath-probe\n\
        print -r -- bob-fpath-start\n\
        print -l -- $fpath\n\
        print -r -- bob-fpath-end";
    let output = match run_bounded_zsh(script, probe_timeout_ms()) {
        ProbeOutcome::Output(output) => output,
        ProbeOutcome::TimedOut | ProbeOutcome::NotFound(_) => return None,
    };
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.iter().position(|line| *line == "bob-fpath-start")?;
    let end = lines.iter().position(|line| *line == "bob-fpath-end")?;
    if end <= start {
        return None;
    }
    Some(
        lines[start + 1..end]
            .iter()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect(),
    )
}

/// Live registration of `target` for `shell`, from one bounded probe.
#[derive(Debug, Clone)]
pub(crate) enum Registration {
    Registered,
    ShadowedBy(String),
    BoundTo(String),
    NotRegistered { on_fpath: bool },
    Unverified(String),
}

impl Registration {
    pub(crate) fn text(&self, shell: Shell) -> String {
        match self {
            Registration::Registered => {
                format!("registered as {}", shell.function_name())
            }
            Registration::ShadowedBy(file) => {
                format!("shadowed by {}", display_path(file))
            }
            Registration::BoundTo(function) => {
                format!("bob is bound to {function}")
            }
            Registration::NotRegistered { .. } => "not registered".to_string(),
            Registration::Unverified(reason) => {
                format!("unverified ({reason})")
            }
        }
    }

    pub(crate) fn healthy(&self) -> bool {
        matches!(self, Registration::Registered)
    }
}

fn display_path(path: &str) -> String {
    super::report::tilde(std::path::Path::new(path))
}

/// Probe a real shell for the registration of the adapter at `target`.
pub(crate) fn probe_registration(
    shell: Shell,
    target: &std::path::Path,
) -> Registration {
    let script = "# bob-completion-verify-probe\n\
        autoload -Uz compinit 2>/dev/null\n\
        compinit -D 2>/dev/null\n\
        autoload +X _bob 2>/dev/null\n\
        print -r -- bob-verify-start\n\
        for entry in $fpath; do print -r -- \"bob-fpath-entry=$entry\"; done\n\
        print -r -- \"bob-comp=${_comps[bob]:-}\"\n\
        print -r -- \"bob-source=${functions_source[_bob]:-}\"\n\
        print -r -- bob-verify-end";
    let output = match run_bounded_zsh(script, probe_timeout_ms()) {
        ProbeOutcome::Output(output) => output,
        ProbeOutcome::TimedOut => {
            return Registration::Unverified("timed out".to_string());
        }
        ProbeOutcome::NotFound(reason) => {
            return Registration::Unverified(reason);
        }
    };
    let lines: Vec<&str> = output.lines().collect();
    let (Some(start), Some(end)) = (
        lines.iter().position(|line| *line == "bob-verify-start"),
        lines.iter().position(|line| *line == "bob-verify-end"),
    ) else {
        return Registration::Unverified("probe failed".to_string());
    };
    if end <= start {
        return Registration::Unverified("probe failed".to_string());
    }
    let mut fpath: Vec<PathBuf> = Vec::new();
    let mut comp = String::new();
    let mut source = String::new();
    for line in &lines[start + 1..end] {
        if let Some(entry) = line.strip_prefix("bob-fpath-entry=") {
            fpath.push(PathBuf::from(entry));
        } else if let Some(value) = line.strip_prefix("bob-comp=") {
            comp = value.to_string();
        } else if let Some(value) = line.strip_prefix("bob-source=") {
            source = value.to_string();
        }
    }
    if comp == shell.function_name() {
        if !source.is_empty() && std::path::Path::new(&source) == target {
            return Registration::Registered;
        }
        if !source.is_empty() {
            return Registration::ShadowedBy(source);
        }
        return Registration::ShadowedBy("(unknown file)".to_string());
    }
    if !comp.is_empty() {
        return Registration::BoundTo(comp);
    }
    let dir = target.parent();
    let on_fpath =
        dir.is_some_and(|dir| fpath.iter().any(|entry| entry == dir));
    Registration::NotRegistered { on_fpath }
}

/// The resolved `bob` on `PATH`, if any.
pub(crate) fn bob_on_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("bob");
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Warn when `command -v bob` resolves somewhere other than this binary.
pub(crate) fn path_shadow_warning() -> Option<String> {
    let on_path = bob_on_path()?;
    let running = std::env::current_exe().ok()?;
    let same = canonical(&on_path) == canonical(&running);
    if same {
        return None;
    }
    Some(format!(
        "`bob` on PATH is {}, not this binary ({}).\n     \
         <TAB> asks the bob on PATH. Fix PATH or reinstall.",
        super::report::tilde(&on_path),
        super::report::tilde(&running),
    ))
}

fn canonical(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_override_falls_back() {
        unsafe {
            std::env::set_var("BOB_COMPLETION_PROBE_TIMEOUT_MS", "250");
        }
        assert_eq!(probe_timeout_ms(), 250);
        unsafe {
            std::env::set_var("BOB_COMPLETION_PROBE_TIMEOUT_MS", "bogus");
        }
        assert_eq!(probe_timeout_ms(), DEFAULT_PROBE_TIMEOUT_MS);
        unsafe {
            std::env::remove_var("BOB_COMPLETION_PROBE_TIMEOUT_MS");
        }
        assert_eq!(probe_timeout_ms(), DEFAULT_PROBE_TIMEOUT_MS);
    }
}
