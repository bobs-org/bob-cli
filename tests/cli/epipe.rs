//! A closed stdout pipe ends bob quietly instead of panicking.

use crate::support::*;
use std::io::Read;
use std::process::Stdio;

const NOW: &str = "2026-10-11 12:00:00";

#[test]
fn closed_stdout_pipe_exits_zero_with_empty_stderr() {
    // `stats -f json` over the full fixture range renders ~350 KB, well
    // past the 64 KB pipe buffer, so closing the reader mid-stream
    // deterministically delivers EPIPE to the next write.
    let mut child = bob_command()
        .env("BOB_DIR", fixture("stats/vault"))
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1")
        .args(["stats", "-r", "2026-01-01..2026-10-11", "-f", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bob stats");
    // Read like `head -c 10`, then close the reader early.
    let mut head = [0_u8; 10];
    let mut read = 0;
    while read < head.len() {
        match child
            .stdout
            .as_mut()
            .expect("piped stdout")
            .read(&mut head[read..])
        {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(error) => panic!("read bob stdout: {error}"),
        }
    }
    assert!(read > 0, "bob stats wrote no output");
    drop(child.stdout.take());
    let output = child.wait_with_output().expect("wait for bob stats");
    assert_eq!(output.status.code(), Some(0), "status after EPIPE");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "stderr leaked a panic:\n{stderr}"
    );
    assert!(
        !stderr.contains("Broken pipe"),
        "stderr leaked EPIPE:\n{stderr}"
    );
    assert!(
        !stderr.contains("BrokenPipe"),
        "stderr leaked EPIPE:\n{stderr}"
    );
}

#[test]
fn closed_stderr_usage_error_exits_two_quietly() {
    // An invalid subcommand prints a clap error to stderr. Dropping the
    // read ends immediately after spawn closes the pipes before bob writes,
    // so `error.print()` deterministically hits BrokenPipe without sleep.
    let mut child = bob_command()
        .args(["bogus-subcommand-xyz"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bob bogus subcommand");
    drop(child.stdout.take());
    drop(child.stderr.take());
    let output = child.wait_with_output().expect("wait for bob bogus");
    assert_eq!(
        output.status.code(),
        Some(2),
        "usage error into a closed pipe keeps exit 2"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "stderr leaked a panic:\n{stderr}"
    );
    assert!(
        !stderr.contains("Broken pipe"),
        "stderr leaked EPIPE:\n{stderr}"
    );
    assert!(
        !stderr.contains("BrokenPipe"),
        "stderr leaked EPIPE:\n{stderr}"
    );
}

#[test]
fn closed_stdout_help_exits_zero_quietly() {
    // `--help` renders to stdout with clap exit code 0. Dropping the read
    // ends immediately after spawn closes the pipes before bob writes, so
    // the render deterministically hits BrokenPipe without sleep.
    let mut child = bob_command()
        .args(["--help"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bob --help");
    drop(child.stdout.take());
    drop(child.stderr.take());
    let output = child.wait_with_output().expect("wait for bob --help");
    assert_eq!(
        output.status.code(),
        Some(0),
        "help into a closed pipe keeps exit 0"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "stderr leaked a panic:\n{stderr}"
    );
    assert!(
        !stderr.contains("Broken pipe"),
        "stderr leaked EPIPE:\n{stderr}"
    );
    assert!(
        !stderr.contains("BrokenPipe"),
        "stderr leaked EPIPE:\n{stderr}"
    );
}
