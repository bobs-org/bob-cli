//! Real-shell verification for `bob completion`.
//!
//! Install and `status -v` classify registration with one bounded shell
//! probe each (`zsh -ic` or `bash -ic`). The probe runs in its own session
//! with no controlling terminal (so an interactive shell can never stop on
//! `SIGTTIN`/`SIGTTOU` under a terminal), with stdin closed and
//! `BOB_COMPLETION_PROBE=1`, and its whole process group is killed at
//! the deadline (8 s, or the hidden `BOB_COMPLETION_PROBE_TIMEOUT_MS`
//! override for tests). Stdout is drained on a reader thread while
//! waiting, so more than 64 KiB of rc output cannot block the probe and a
//! background job inheriting the pipe cannot hang bob. Probe scripts carry
//! `# bob-completion-*` markers so tests can fake the shell by branching on
//! the script text; real rc noise is ignored by parsing only the lines
//! between `bob-*-start` and `bob-*-end`.

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

/// Kill the whole probe process group at the deadline, not just the child.
#[cfg(unix)]
fn kill_process_group(child: &mut std::process::Child) {
    // The child called setsid, so its pid is its pgid: a negative pid
    // addresses the group.
    let pid = child.id() as i32;
    // Best effort; fall back to killing just the child.
    unsafe {
        if libc::kill(-pid, libc::SIGKILL) != 0 {
            let _ = child.kill();
        }
    }
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut std::process::Child) {
    let _ = child.kill();
}

pub(crate) fn probe_timeout_ms() -> u64 {
    std::env::var("BOB_COMPLETION_PROBE_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_PROBE_TIMEOUT_MS)
}

fn run_bounded_shell(
    shell_bin: &str,
    script: &str,
    timeout_ms: u64,
) -> ProbeOutcome {
    let mut command = Command::new(shell_bin);
    command
        .args(["-i", "-c", script])
        .env("BOB_COMPLETION_PROBE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New session, no controlling terminal: an interactive shell
        // started under a terminal can otherwise stop on SIGTTIN/SIGTTOU
        // when it opens /dev/tty from a background process group.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ProbeOutcome::NotFound(format!(
                "{shell_bin} not found ({error})"
            ));
        }
    };
    // Drain stdout concurrently so a chatty rc (> 64 KiB) cannot block the
    // shell, and a background job inheriting the pipe cannot hang bob after
    // the shell exits.
    let stdout_rx = child.stdout.take().map(|pipe| {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut stdout = String::new();
            let mut reader = std::io::BufReader::new(pipe);
            let _ = Read::read_to_string(&mut reader, &mut stdout);
            let _ = tx.send(stdout);
        });
        rx
    });
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_process_group(&mut child);
                    let _ = child.wait();
                    return ProbeOutcome::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
    let _ = child.wait();
    let stdout = stdout_rx
        .and_then(|rx| rx.recv_timeout(Duration::from_secs(2)).ok())
        .unwrap_or_default();
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
    let output = match run_bounded_shell("zsh", script, probe_timeout_ms()) {
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
}

fn display_path(path: &str) -> String {
    super::report::tilde(std::path::Path::new(path))
}

/// Probe a real shell for the registration of the adapter at `target`.
pub(crate) fn probe_registration(
    shell: Shell,
    target: &std::path::Path,
) -> Registration {
    match shell {
        Shell::Zsh => probe_zsh(target),
        Shell::Bash => probe_bash(target),
    }
}

fn probe_zsh(target: &std::path::Path) -> Registration {
    let script = "# bob-completion-verify-probe\n\
        (( ${+_comps} )) || { autoload -Uz compinit 2>/dev/null; compinit -D 2>/dev/null; }\n\
        autoload +X _bob 2>/dev/null\n\
        print -r -- bob-verify-start\n\
        for entry in $fpath; do print -r -- \"bob-fpath-entry=$entry\"; done\n\
        print -r -- \"bob-comp=${_comps[bob]:-}\"\n\
        print -r -- \"bob-source=${functions_source[_bob]:-}\"\n\
        print -r -- bob-verify-end";
    let output = match run_bounded_shell("zsh", script, probe_timeout_ms()) {
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
    if comp == Shell::Zsh.function_name() {
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

fn probe_bash(_target: &std::path::Path) -> Registration {
    // Trigger bash-completion's lazy loader when it exists
    // (_comp_load in bash-completion >= 2.12, else __load_completion),
    // then report what `bob` is bound to. Without bash-completion the
    // loader is absent and an unregistered adapter stays unregistered,
    // with a `source <path>` remedy from the caller.
    let script = "# bob-completion-verify-probe-bash\n\
        if declare -F _comp_load >/dev/null 2>&1; then _comp_load bob 2>/dev/null;\n\
        elif declare -F __load_completion >/dev/null 2>&1; then __load_completion bob 2>/dev/null; fi\n\
        echo bob-verify-start\n\
        complete -p bob 2>/dev/null\n\
        echo bob-verify-end";
    let output = match run_bounded_shell("bash", script, probe_timeout_ms()) {
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
    for line in &lines[start + 1..end] {
        let line = line.trim();
        if !line.starts_with("complete ") || !line.ends_with(" bob") {
            continue;
        }
        let function = complete_function(line);
        match function {
            Some(name) if name == Shell::Bash.function_name() => {
                return Registration::Registered;
            }
            Some(name) => return Registration::BoundTo(name.to_string()),
            None => continue,
        }
    }
    Registration::NotRegistered { on_fpath: false }
}

/// The `-F <function>` word in a `complete -p bob` line, if any.
fn complete_function(line: &str) -> Option<&str> {
    let mut words = line.split_whitespace();
    while let Some(word) = words.next() {
        if word == "-F"
            && let Some(function) = words.next()
        {
            return Some(function);
        }
    }
    None
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
