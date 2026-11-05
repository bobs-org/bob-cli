//! Hardened subprocess runner shared by gkeep and gcal.
//!
//! Spawns a child in its own process group, writes optional stdin on a
//! helper thread, drains stdout and stderr, and enforces a deadline.
//! On timeout the whole process group is killed so an orphaned grandchild
//! cannot hold the pipes open. After the leader exits, remaining
//! stragglers in the group are killed before the reader joins.

use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// One subprocess invocation. Stdio is always piped; gog and the Keep
/// adapter never own the terminal.
#[derive(Debug, Clone)]
pub(crate) struct RunSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub stdin: Option<Vec<u8>>,
    pub env: Vec<(OsString, OsString)>,
    /// Variables removed from the child's inherited environment.
    pub env_remove: Vec<OsString>,
    pub timeout: Duration,
}

/// Captured result of [`run`].
#[derive(Debug)]
pub(crate) struct RunOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub stdin_error: Option<String>,
}

/// One raw-bytes invocation. Stdio is always piped; unlike [`RunSpec`]
/// there is no stdin payload and both pipes are drained with byte caps so
/// a runaway child cannot grow memory without bound. Retained bytes past
/// a cap are discarded but still drained, so the child is never blocked
/// on a full pipe.
#[derive(Debug, Clone)]
pub(crate) struct RawRunSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    /// Variables removed from the child's inherited environment.
    pub env_remove: Vec<OsString>,
    pub timeout: Duration,
    /// Retained stdout bytes; one byte past the cap is kept to detect the
    /// overage. The rest is drained and discarded.
    pub stdout_cap: usize,
    /// Retained stderr bytes; truncation is silent past the cap.
    pub stderr_cap: usize,
}

/// Captured result of [`run_raw`].
#[derive(Debug)]
pub(crate) struct RawRunOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    /// True when stdout held more than `stdout_cap` bytes.
    pub stdout_truncated: bool,
}

/// Spawn `spec.program`, enforce `spec.timeout`, and collect output.
///
/// A spawn failure (including after the ETXTBSY retry) is returned as
/// `Err`. A timeout is a successful `RunOutput` with `timed_out: true`.
pub(crate) fn run(spec: &RunSpec) -> io::Result<RunOutput> {
    let mut child = spawn(spec)?;
    let deadline = Instant::now() + spec.timeout;
    let payload = spec.stdin.clone();
    let stdin_pipe = child.stdin.take();
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdin_writer = thread::spawn(move || {
        let mut stdin = stdin_pipe?;
        if let Some(payload) = payload
            && let Err(error) = stdin.write_all(&payload)
        {
            return Some(error.to_string());
        }
        None
    });
    let stdout_reader = thread::spawn(move || drain_pipe(stdout_pipe));
    let stderr_reader = thread::spawn(move || drain_pipe(stderr_pipe));
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    kill_tree(&mut child);
                    let _ = stdin_writer.join();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Ok(RunOutput {
                        status: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        timed_out: true,
                        stdin_error: None,
                    });
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    kill_stragglers(child.id());
    let stdin_error = stdin_writer.join().unwrap_or(None);
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(RunOutput {
        status: status.code(),
        stdout,
        stderr,
        timed_out: false,
        stdin_error,
    })
}

/// Spawn `spec.program`, enforce `spec.timeout`, and collect raw bytes.
///
/// A spawn failure (including after the ETXTBSY retry) is returned as
/// `Err`. A timeout is a successful `RawRunOutput` with `timed_out: true`;
/// the whole process group is killed and reaped before returning, so no
/// child survives the call.
pub(crate) fn run_raw(spec: &RawRunSpec) -> io::Result<RawRunOutput> {
    let mut child = spawn_raw(spec)?;
    let deadline = Instant::now() + spec.timeout;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_cap = spec.stdout_cap;
    let stderr_cap = spec.stderr_cap;
    let stdout_reader =
        thread::spawn(move || drain_pipe_capped(stdout_pipe, stdout_cap));
    let stderr_reader =
        thread::spawn(move || drain_pipe_capped(stderr_pipe, stderr_cap));
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    kill_tree(&mut child);
                    let (stdout, stdout_truncated) =
                        stdout_reader.join().unwrap_or_default();
                    let (stderr, _) = stderr_reader.join().unwrap_or_default();
                    return Ok(RawRunOutput {
                        status: None,
                        stdout,
                        stderr,
                        timed_out: true,
                        stdout_truncated,
                    });
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    kill_stragglers(child.id());
    let (stdout, stdout_truncated) = stdout_reader.join().unwrap_or_default();
    let (stderr, _) = stderr_reader.join().unwrap_or_default();
    Ok(RawRunOutput {
        status: status.code(),
        stdout,
        stderr,
        timed_out: false,
        stdout_truncated,
    })
}

/// Kill stragglers in the child's process group after the leader exits.
fn kill_stragglers(pid: u32) {
    #[cfg(unix)]
    {
        let target = format!("-{pid}");
        let _ = Command::new("kill").args(["-KILL", "--", &target]).output();
    }
}

/// Kill the whole process tree on timeout.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id();
        let target = format!("-{pid}");
        let killed = Command::new("kill")
            .args(["-KILL", "--", &target])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !killed {
            let _ = child.kill();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// Spawn with piped stdio, retrying a transient ETXTBSY.
fn spawn(spec: &RunSpec) -> io::Result<std::process::Child> {
    let mut attempts = 0;
    loop {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        for key in &spec.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error)
                if error.raw_os_error() == Some(26) && attempts < 100 =>
            {
                attempts += 1;
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}

fn drain_pipe<R: Read>(pipe: Option<R>) -> String {
    let mut text = String::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_string(&mut text);
    }
    text
}

/// Drain a pipe keeping at most `cap + 1` bytes. The extra byte is the
/// overage signal; everything past it is still drained (so the child
/// never blocks on a full pipe) but discarded.
fn drain_pipe_capped<R: Read>(pipe: Option<R>, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let Some(mut pipe) = pipe else {
        return (kept, false);
    };
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                let room = cap.saturating_add(1).saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..read.min(room)]);
            }
            Err(_) => break,
        }
    }
    let truncated = kept.len() > cap;
    (kept, truncated)
}

/// Spawn with piped stdout/stderr, no stdin, and no payload writer.
fn spawn_raw(spec: &RawRunSpec) -> io::Result<std::process::Child> {
    let mut attempts = 0;
    loop {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        for key in &spec.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error)
                if error.raw_os_error() == Some(26) && attempts < 100 =>
            {
                attempts += 1;
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_spec(program: &str, args: &[&str], timeout_ms: u64) -> RawRunSpec {
        RawRunSpec {
            program: PathBuf::from(program),
            args: args.iter().map(OsString::from).collect(),
            env: Vec::new(),
            env_remove: Vec::new(),
            timeout: Duration::from_millis(timeout_ms),
            stdout_cap: 1024,
            stderr_cap: 128,
        }
    }

    #[test]
    fn raw_run_collects_bytes() {
        let output =
            run_raw(&raw_spec("printf", &["hi"], 5_000)).expect("printf runs");
        assert!(!output.timed_out);
        assert_eq!(output.stdout, b"hi");
        assert_eq!(output.status, Some(0));
        assert!(!output.stdout_truncated);
    }

    #[test]
    fn raw_run_times_out_and_reaps() {
        let output =
            run_raw(&raw_spec("sleep", &["47"], 100)).expect("sleep runs");
        assert!(output.timed_out);
        assert_eq!(output.status, None);
        // The process group kill must leave no `sleep 47` behind.
        let survivors = Command::new("pgrep")
            .args(["-f", "sleep 47"])
            .output()
            .expect("pgrep runs");
        assert!(
            !survivors.status.success(),
            "timed-out sleep survived the process-group kill"
        );
    }

    #[test]
    fn raw_run_flags_stdout_over_cap() {
        let spec = RawRunSpec {
            stdout_cap: 8,
            ..raw_spec("printf", &["0123456789abcdef"], 5_000)
        };
        let output = run_raw(&spec).expect("printf runs");
        assert!(!output.timed_out);
        assert!(output.stdout_truncated);
        assert_eq!(output.stdout.len(), 9);
    }

    #[test]
    fn raw_run_missing_program_is_a_spawn_error() {
        let error =
            run_raw(&raw_spec("bob-definitely-missing-binary", &[], 5_000))
                .expect_err("missing binary fails to spawn");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
