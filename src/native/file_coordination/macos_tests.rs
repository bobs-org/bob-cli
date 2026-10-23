use super::*;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Instant;

struct Presenter {
    child: Child,
    input: ChildStdin,
    output: Receiver<Value>,
    reader: Option<thread::JoinHandle<()>>,
}

impl Presenter {
    fn start(binary: &Path, path: &Path) -> Option<Self> {
        let mut child = Command::new(binary)
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start NSDocument fixture");
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let line = line.expect("read presenter output");
                let value =
                    serde_json::from_str(&line).expect("presenter JSON");
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let presenter = Self {
            child,
            input,
            output: receiver,
            reader: Some(reader),
        };
        let ready = presenter.receive();
        if ready["event"] == "skip" {
            eprintln!("skipped NSDocument fixture: {}", ready["reason"]);
            return None;
        }
        assert_eq!(ready["event"], "ready", "{ready}");
        Some(presenter)
    }

    fn receive(&self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(15))
            .expect("presenter response within 15 seconds")
    }

    fn command(&mut self, command: &str) -> Value {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
        self.receive()
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

fn append(path: &Path, text: &str) -> std::io::Result<()> {
    // Replace the file, like the PDF writer, rather than just an in-place write.
    let contents = fs::read_to_string(path)? + text;
    let staging = path.with_extension("tmp");
    fs::write(&staging, contents)?;
    fs::rename(staging, path)
}

fn assert_written(outcome: Coordination<std::io::Result<()>>) {
    match outcome {
        Coordination::Granted { value, .. } => value.expect("write"),
        other => panic!("expected granted write, got {other:?}"),
    }
}

#[test]
fn no_presenter_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no presenter ü.txt");
    fs::write(&path, "original").unwrap();
    assert_written(coordinate_merging_write(
        &path,
        Duration::from_secs(2),
        || append(&path, "+bob"),
    ));
    assert_eq!(fs::read_to_string(path).unwrap(), "original+bob");
}

#[test]
fn nsdocument_saves_reloads_and_holds_a_blocked_presenter() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("presenter");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/file_coordination/presenter.swift");
    let compilation = match Command::new("swiftc")
        .arg(&fixture)
        .arg("-o")
        .arg(&binary)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped NSDocument fixture: swiftc is missing");
            return;
        }
        Err(error) => panic!("run swiftc: {error}"),
    };
    assert!(
        compilation.status.success(),
        "swiftc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );

    let path = dir.path().join("presented.txt");
    fs::write(&path, "original").unwrap();
    let Some(mut presenter) = Presenter::start(&binary, &path) else {
        return;
    };
    let edited = presenter.command("edit +user");
    assert_eq!(edited["dirty"], true);
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    assert_written(coordinate_merging_write(
        &path,
        Duration::from_secs(2),
        || {
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "original+user",
                "presenter must save BEFORE accessor"
            );
            append(&path, "+bob")
        },
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "original+user+bob");

    // AppKit receives its post-write notification asynchronously.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = presenter.command("state");
        if state["text"] == "original+user+bob" && state["dirty"] == false {
            assert!(
                state["reads"].as_u64().unwrap()
                    > edited["reads"].as_u64().unwrap()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "presenter did not reload clean: {state}"
        );
        thread::sleep(Duration::from_millis(25));
    }

    assert_eq!(presenter.command("edit +unsaved")["dirty"], true);
    assert_eq!(presenter.command("block 6")["event"], "blocked");
    let start = Instant::now();
    let outcome =
        coordinate_merging_write(&path, Duration::from_secs(2), || {
            append(&path, "+FORBIDDEN")
        });
    assert!(
        matches!(outcome, Coordination::Held(TimedOut { waited }) if waited >= Duration::from_secs(2)),
        "{outcome:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "timeout exceeded budget: {:?}",
        start.elapsed()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "original+user+bob");
    assert_eq!(presenter.receive()["event"], "unblocked");
    assert_eq!(presenter.command("save")["event"], "saved");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "original+user+bob+unsaved"
    );
    assert_eq!(presenter.command("state")["dirty"], false);
}
