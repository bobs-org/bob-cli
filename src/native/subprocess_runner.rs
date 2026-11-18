//! Hardened subprocess runner shared by gkeep and gcal.
//!
//! Spawns a child in its own process group, writes optional stdin on a
//! helper thread, drains stdout and stderr, and enforces a deadline.
//! On timeout the whole process group is killed so an orphaned grandchild
//! cannot hold the pipes open. After the leader exits, remaining
//! stragglers in the group are killed before the readers are given a bounded
//! grace period to finish draining.

use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

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
    /// Optional SIGTERM grace before SIGKILL; None preserves immediate kill.
    pub term_grace: Option<Duration>,
    /// Signal-safe cancellation flag, polled alongside the deadline.
    pub interrupt: Option<&'static AtomicBool>,
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
    pub interrupted: bool,
    /// True when stdout held more than `stdout_cap` bytes.
    pub stdout_truncated: bool,
    /// True when either pipe remained open past the post-exit drain grace.
    pub output_incomplete: bool,
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
        match exited_without_reaping(&mut child)? {
            true => {
                kill_stragglers(child.id());
                break child.wait()?;
            }
            false => {
                if Instant::now() >= deadline {
                    kill_tree(&mut child);
                    let _ = stdin_writer.join();
                    let drain_deadline = Instant::now() + PIPE_DRAIN_GRACE;
                    let stdout = receive_before(stdout_reader, drain_deadline);
                    let stderr = receive_before(stderr_reader, drain_deadline);
                    let output_incomplete =
                        stdout.is_none() || stderr.is_none();
                    let mut stderr = stderr.unwrap_or_default();
                    if output_incomplete {
                        if !stderr.is_empty() {
                            stderr.push('\n');
                        }
                        stderr.push_str("subprocess output truncated: inherited pipe remained open after child exit");
                    }
                    return Ok(RunOutput {
                        status: None,
                        stdout: stdout.unwrap_or_default(),
                        stderr,
                        timed_out: true,
                        stdin_error: None,
                    });
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let stdin_error = stdin_writer.join().unwrap_or(None);
    let drain_deadline = Instant::now() + PIPE_DRAIN_GRACE;
    let stdout = receive_before(stdout_reader, drain_deadline);
    let stderr = receive_before(stderr_reader, drain_deadline);
    let output_incomplete = stdout.is_none() || stderr.is_none();
    let mut stderr = stderr.unwrap_or_default();
    if output_incomplete {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str("subprocess output truncated: inherited pipe remained open after child exit");
    }
    Ok(RunOutput {
        status: status.code(),
        stdout: stdout.unwrap_or_default(),
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
    let mut timed_out = false;
    let mut interrupted;
    let status = loop {
        interrupted = spec
            .interrupt
            .is_some_and(|flag| flag.load(Ordering::Acquire));
        if interrupted || Instant::now() >= deadline {
            timed_out = !interrupted;
            terminate_tree(&mut child, spec.term_grace);
            break None;
        }
        match exited_without_reaping(&mut child) {
            Ok(true) => {
                kill_stragglers(child.id());
                break child.wait()?.code();
            }
            Ok(false) => thread::sleep(Duration::from_millis(10)),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                continue
            }
            Err(error) => {
                terminate_tree(&mut child, spec.term_grace);
                return Err(error);
            }
        }
    };
    let drain_deadline = Instant::now() + PIPE_DRAIN_GRACE;
    let stdout = receive_before(stdout_reader, drain_deadline);
    let stderr = receive_before(stderr_reader, drain_deadline);
    let output_incomplete = stdout.is_none() || stderr.is_none();
    let (stdout, stdout_truncated) = stdout.unwrap_or_default();
    let (stderr, _) = stderr.unwrap_or_default();
    Ok(RawRunOutput {
        status,
        stdout,
        stderr,
        timed_out,
        interrupted,
        stdout_truncated,
        output_incomplete,
    })
}

fn terminate_tree(child: &mut std::process::Child, grace: Option<Duration>) {
    #[cfg(unix)]
    if let Some(grace) = grace {
        // Keep the leader unreaped until the final group kill to prevent PID reuse.
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGTERM);
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if matches!(exited_without_reaping(child), Ok(true)) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(not(unix))]
    let _ = grace;
    kill_tree(child);
}

/// Poll for child exit without reaping it, keeping its process-group ID from
/// being recycled until the straggler kill has been sent.
fn exited_without_reaping(child: &mut std::process::Child) -> io::Result<bool> {
    #[cfg(unix)]
    {
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { info.si_pid() } != 0)
    }
    #[cfg(not(unix))]
    {
        Ok(child.try_wait()?.is_some())
    }
}

fn receive_before<T: Send + 'static>(
    reader: thread::JoinHandle<T>,
    deadline: Instant,
) -> Option<T> {
    let (sender, receiver) = mpsc::sync_channel(1);
    #[cfg(test)]
    let tracker = tests::PIPE_DRAIN_READER_TRACKER
        .with(|tracker| tracker.borrow().clone());
    #[cfg(test)]
    if let Some(tracker) = &tracker {
        tracker.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    thread::spawn(move || {
        #[cfg(test)]
        let _finished = tracker.map(DrainReaderFinished);
        let value = reader.join().ok();
        let _ = sender.send(value);
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    receiver.recv_timeout(remaining).ok().flatten()
}

#[cfg(test)]
struct DrainReaderFinished(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[cfg(test)]
impl Drop for DrainReaderFinished {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Kill stragglers in the child's process group after the leader exits.
fn kill_stragglers(pid: u32) {
    #[cfg(unix)]
    {
        // The leader is still an unreaped zombie here, so this process-group
        // ID cannot yet have been recycled.
        let _ = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
    }
}

/// Kill the whole process tree on timeout.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id();
        let killed =
            unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) } == 0;
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
    use std::{
        cell::RefCell,
        fs,
        panic::{catch_unwind, AssertUnwindSafe},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    thread_local! {
        pub(super) static PIPE_DRAIN_READER_TRACKER: RefCell<Option<Arc<AtomicUsize>>> = const { RefCell::new(None) };
    }

    fn raw_spec(program: &str, args: &[&str], timeout_ms: u64) -> RawRunSpec {
        RawRunSpec {
            program: PathBuf::from(program),
            args: args.iter().map(OsString::from).collect(),
            env: Vec::new(),
            env_remove: Vec::new(),
            timeout: Duration::from_millis(timeout_ms),
            term_grace: None,
            interrupt: None,
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

    #[cfg(unix)]
    #[test]
    fn raw_grace_allows_term_cleanup_and_escalates_if_ignored() {
        let mut spec = raw_spec("sh", &["-c", "trap 'echo cleaned; exit 0' TERM; while :; do sleep 0.02; done"], 300);
        spec.term_grace = Some(Duration::from_secs(1));
        let out = run_raw(&spec).unwrap();
        assert!(out.timed_out);
        assert!(!out.interrupted);
        assert!(String::from_utf8_lossy(&out.stdout).contains("cleaned"));
        let mut spec = raw_spec(
            "sh",
            &["-c", "trap '' TERM; while :; do sleep 0.02; done"],
            100,
        );
        spec.term_grace = Some(Duration::from_millis(100));
        let began = Instant::now();
        assert!(run_raw(&spec).unwrap().timed_out);
        assert!(began.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn raw_interrupt_stops_before_deadline() {
        static STOP: AtomicBool = AtomicBool::new(false);
        let mut spec = raw_spec("sleep", &["60"], 60_000);
        spec.interrupt = Some(&STOP);
        spec.term_grace = Some(Duration::from_millis(100));
        let setter = thread::spawn(|| {
            thread::sleep(Duration::from_millis(100));
            STOP.store(true, Ordering::Relaxed);
        });
        let began = Instant::now();
        let out = run_raw(&spec).unwrap();
        setter.join().unwrap();
        assert!(out.interrupted);
        assert!(!out.timed_out);
        assert!(began.elapsed() < Duration::from_secs(2));
    }

    /// The detached supervisor owns and reaps the pipe holder. The fixture
    /// proves cleanup even when assertions unwind; its control and identity
    /// files stay outside the pipes being tested.
    #[cfg(unix)]
    const PIPE_DRAIN_HOLDER_SCRIPT: &str = r#"
import json, os, subprocess, time

ready = os.environ["BOB_PIPE_DRAIN_READY"]
started = os.environ["BOB_PIPE_DRAIN_STARTED"]
identity = os.environ["BOB_PIPE_DRAIN_IDENTITY"]
exited = os.environ["BOB_PIPE_DRAIN_EXITED"]
open(started, "w").close()

if os.fork() == 0:
    os.setsid()

    def save(path, value):
        temporary = path + ".tmp"
        with open(temporary, "w") as file:
            json.dump(value, file)
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, path)

    supervisor_pid = os.getpid()
    save(identity, {"supervisor_pid": supervisor_pid})
    holder_code = """
import os, time
release = os.environ["BOB_PIPE_DRAIN_RELEASE"]
deadline = time.monotonic() + 30
while not os.path.exists(release) and time.monotonic() < deadline:
    time.sleep(0.01)
"""
    holder = subprocess.Popen(["python3", "-c", holder_code])
    save(identity, {"supervisor_pid": supervisor_pid, "holder_pid": holder.pid})
    open(ready, "w").close()
    for descriptor in (0, 1, 2):
        os.close(descriptor)
    return_code = holder.wait()
    save(exited, {"holder_pid": holder.pid, "return_code": return_code})
    os._exit(0)

deadline = time.monotonic() + 3
while not os.path.exists(ready) and time.monotonic() < deadline:
    time.sleep(0.01)
leader_done = os.environ["BOB_PIPE_DRAIN_LEADER_DONE"]
open(leader_done, "w").close()
os._exit(0 if os.path.exists(ready) else 1)
"#;

    #[cfg(unix)]
    struct PipeDrainFixture {
        root: PathBuf,
        readers: Arc<AtomicUsize>,
        previous_tracker: Option<Arc<AtomicUsize>>,
        cleaned: bool,
    }

    #[cfg(unix)]
    impl PipeDrainFixture {
        fn new(root: PathBuf) -> Self {
            let readers = Arc::new(AtomicUsize::new(0));
            let previous_tracker = PIPE_DRAIN_READER_TRACKER
                .with(|tracker| tracker.replace(Some(readers.clone())));
            Self {
                root,
                readers,
                previous_tracker,
                cleaned: false,
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }

        fn env(&self) -> Vec<(OsString, OsString)> {
            [
                ("BOB_PIPE_DRAIN_READY", "ready"),
                ("BOB_PIPE_DRAIN_STARTED", "started"),
                ("BOB_PIPE_DRAIN_LEADER_DONE", "leader-done"),
                ("BOB_PIPE_DRAIN_IDENTITY", "identity.json"),
                ("BOB_PIPE_DRAIN_EXITED", "exited.json"),
                ("BOB_PIPE_DRAIN_RELEASE", "release"),
            ]
            .into_iter()
            .map(|(key, file)| {
                (OsString::from(key), self.path(file).into_os_string())
            })
            .collect()
        }

        fn run_spec(&self) -> RunSpec {
            RunSpec {
                program: PathBuf::from("python3"),
                args: ["-c", PIPE_DRAIN_HOLDER_SCRIPT]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
                stdin: None,
                env: self.env(),
                env_remove: Vec::new(),
                timeout: Duration::from_secs(5),
            }
        }

        fn raw_spec(&self) -> RawRunSpec {
            RawRunSpec {
                program: PathBuf::from("python3"),
                args: ["-c", PIPE_DRAIN_HOLDER_SCRIPT]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
                env: self.env(),
                env_remove: Vec::new(),
                timeout: Duration::from_secs(5),
                term_grace: None,
                interrupt: None,
                stdout_cap: 1024,
                stderr_cap: 1024,
            }
        }

        fn cleanup(&mut self) -> Result<(), String> {
            if self.cleaned {
                return Ok(());
            }
            fs::write(self.path("release"), "release")
                .map_err(|error| format!("write release sentinel: {error}"))?;

            let startup_deadline = Instant::now() + Duration::from_secs(4);
            while !self.path("identity.json").exists()
                && self.path("started").exists()
                && !self.path("leader-done").exists()
                && Instant::now() < startup_deadline
            {
                thread::sleep(Duration::from_millis(5));
            }

            if self.path("identity.json").exists() {
                self.wait_for_holder_exit()?;
            } else if self.path("started").exists()
                && !self.path("leader-done").exists()
            {
                return Err("fixture leader did not finish startup".to_owned());
            }

            let readers_deadline = Instant::now() + Duration::from_secs(2);
            while self.readers.load(Ordering::SeqCst) != 0
                && Instant::now() < readers_deadline
            {
                thread::sleep(Duration::from_millis(5));
            }
            if self.readers.load(Ordering::SeqCst) != 0 {
                return Err(
                    "pipe reader wrapper thread remained after release"
                        .to_owned(),
                );
            }

            fs::write(self.path("cleanup-complete"), "clean")
                .map_err(|error| format!("write cleanup proof: {error}"))?;
            self.cleaned = true;
            Ok(())
        }

        fn wait_for_holder_exit(&self) -> Result<(), String> {
            let identity_path = self.path("identity.json");
            let identity: serde_json::Value =
                serde_json::from_slice(&fs::read(&identity_path).map_err(
                    |error| format!("read holder identity: {error}"),
                )?)
                .map_err(|error| format!("parse holder identity: {error}"))?;
            let supervisor_pid = identity["supervisor_pid"]
                .as_u64()
                .ok_or_else(|| "identity has no supervisor pid".to_owned())?
                as u32;
            let holder_pid =
                identity["holder_pid"].as_u64().map(|pid| pid as u32);
            let exit_path = self.path("exited.json");
            let exit_deadline = Instant::now() + Duration::from_secs(2);
            while !exit_path.exists() && Instant::now() < exit_deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if !exit_path.exists() {
                if let Some(pid) = holder_pid {
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
                let reap_deadline = Instant::now() + Duration::from_secs(2);
                while !exit_path.exists() && Instant::now() < reap_deadline {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            if !exit_path.exists() {
                return Err("detached supervisor did not acknowledge waitpid"
                    .to_owned());
            }

            let exited: serde_json::Value = serde_json::from_slice(
                &fs::read(&exit_path).map_err(|error| {
                    format!("read holder exit acknowledgement: {error}")
                })?,
            )
            .map_err(|error| {
                format!("parse holder exit acknowledgement: {error}")
            })?;
            if exited["holder_pid"].as_u64()
                != Some(holder_pid.unwrap_or_default() as u64)
            {
                return Err(
                    "exit acknowledgement names a different holder".to_owned()
                );
            }
            if exited["return_code"].as_i64() != Some(0) {
                return Err(
                    "holder did not exit from the release sentinel".to_owned()
                );
            }

            let gone_deadline = Instant::now() + Duration::from_secs(2);
            while (process_exists(supervisor_pid)
                || holder_pid.is_some_and(process_exists))
                && Instant::now() < gone_deadline
            {
                thread::sleep(Duration::from_millis(5));
            }
            if holder_pid.is_some_and(process_exists) {
                return Err(
                    "supervisor waitpid acknowledged but holder still exists"
                        .to_owned(),
                );
            }
            if process_exists(supervisor_pid) {
                return Err("detached supervisor remained after its exit acknowledgement".to_owned());
            }
            Ok(())
        }
    }

    #[cfg(unix)]
    impl Drop for PipeDrainFixture {
        fn drop(&mut self) {
            let _ = self.cleanup();
            PIPE_DRAIN_READER_TRACKER.with(|tracker| {
                tracker.replace(self.previous_tracker.take());
            });
        }
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0
            || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    #[cfg(unix)]
    fn assert_cleanup_proven(root: &std::path::Path) {
        assert_eq!(
            fs::read_to_string(root.join("cleanup-complete"))
                .expect("fixture cleanup proof exists"),
            "clean",
            "fixture Drop did not prove holder and reader cleanup"
        );
        if let Ok(bytes) = fs::read(root.join("identity.json")) {
            let identity: serde_json::Value = serde_json::from_slice(&bytes)
                .expect("cleanup identity is valid JSON");
            for key in ["supervisor_pid", "holder_pid"] {
                if let Some(pid) = identity[key].as_u64() {
                    assert!(
                        !process_exists(pid as u32),
                        "{key} {pid} survived cleanup"
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_bounds_pipe_drain_when_detached_grandchild_holds_output() {
        let temp =
            tempfile::TempDir::new().expect("tempdir for pipe-drain fixture");
        let root = temp.path().to_path_buf();
        let mut fixture = PipeDrainFixture::new(root);
        let started = Instant::now();
        let output = run(&fixture.run_spec()).expect("fixture leader runs");
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(output.stderr.contains("output truncated"));
        fixture.cleanup().expect("release and reap detached holder");
        assert_cleanup_proven(temp.path());
    }

    #[cfg(unix)]
    #[test]
    fn raw_run_bounds_pipe_drain_when_detached_grandchild_holds_output() {
        let temp =
            tempfile::TempDir::new().expect("tempdir for pipe-drain fixture");
        let mut fixture = PipeDrainFixture::new(temp.path().to_path_buf());
        let started = Instant::now();
        let output = run_raw(&fixture.raw_spec()).expect("fixture leader runs");
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(output.output_incomplete);
        fixture.cleanup().expect("release and reap detached holder");
        assert_cleanup_proven(temp.path());
    }

    #[cfg(unix)]
    #[test]
    fn pipe_drain_fixture_cleans_up_during_assertion_unwind() {
        let temp =
            tempfile::TempDir::new().expect("tempdir for pipe-drain unwind");
        let root = temp.path().to_path_buf();
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let fixture = PipeDrainFixture::new(root);
            let output = run(&fixture.run_spec()).expect("fixture leader runs");
            assert!(output.stderr.contains("output truncated"));
            panic!("exercise fixture cleanup during assertion unwind");
        }));
        assert!(unwound.is_err());
        assert_cleanup_proven(temp.path());
    }

    #[cfg(unix)]
    #[test]
    fn pipe_drain_fixture_cleans_up_when_runner_fails_early() {
        let temp =
            tempfile::TempDir::new().expect("tempdir for early runner failure");
        let mut fixture = PipeDrainFixture::new(temp.path().to_path_buf());
        let mut spec = fixture.run_spec();
        spec.program =
            PathBuf::from("bob-definitely-missing-pipe-drain-fixture");
        let error =
            run(&spec).expect_err("missing fixture program must fail to spawn");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        fixture
            .cleanup()
            .expect("early failure has no holder or reader threads");
        assert_cleanup_proven(temp.path());
    }
}
