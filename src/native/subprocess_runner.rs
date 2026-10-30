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
