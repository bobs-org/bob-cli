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

fn compile_presenter(dir: &Path) -> Option<PathBuf> {
    let binary = dir.join("presenter");
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
            return None;
        }
        Err(error) => panic!("run swiftc: {error}"),
    };
    assert!(
        compilation.status.success(),
        "swiftc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    Some(binary)
}

fn wait_for_document(
    presenter: &Presenter,
    expected_path: &Path,
    expected_text: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = presenter.command("state");
        if state["url"] == expected_path.to_string_lossy().as_ref()
            && state["text"] == expected_text
            && state["dirty"] == false
        {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "presenter did not reach expected state: {state}"
        );
        thread::sleep(Duration::from_millis(25));
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
fn no_presenter_move_scope_moves_forward_and_back_without_replacing() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("source.pdf");
    let to = dir.path().join("destination.pdf");
    fs::write(&from, "saved before access").unwrap();

    assert_written(coordinate_moving_write(
        &from,
        &to,
        Duration::from_secs(2),
        |scope| {
            scope.move_forward()?;
            assert!(!from.exists());
            assert_eq!(fs::read_to_string(&to)?, "saved before access");
            scope.move_back()?;
            assert!(!to.exists());
            Ok(())
        },
    ));
    assert_eq!(fs::read_to_string(&from).unwrap(), "saved before access");
}

#[test]
fn moving_scope_keeps_presenter_at_new_url_for_later_saves() {
    let dir = tempfile::tempdir().unwrap();
    let Some(binary) = compile_presenter(dir.path()) else {
        return;
    };
    let from = dir.path().join("presented.txt");
    let to = dir.path().join("moved.txt");
    fs::write(&from, "original").unwrap();
    let Some(mut presenter) = Presenter::start(&binary, &from) else {
        return;
    };
    let edited = presenter.command("edit +user");
    assert_eq!(edited["dirty"], true);

    assert_written(coordinate_moving_write(
        &from,
        &to,
        Duration::from_secs(2),
        |scope| {
            assert_eq!(fs::read_to_string(&from)?, "original+user");
            scope.move_forward()?;
            append(&to, "+bob")
        },
    ));
    assert!(!from.exists());
    assert_eq!(fs::read_to_string(&to).unwrap(), "original+user+bob");
    let moved = wait_for_document(&presenter, &to, "original+user+bob");
    assert!(
        moved["reads"].as_u64().unwrap() > edited["reads"].as_u64().unwrap()
    );

    assert_eq!(presenter.command("edit +later")["dirty"], true);
    assert_eq!(presenter.command("save")["event"], "saved");
    assert_eq!(fs::read_to_string(&to).unwrap(), "original+user+bob+later");
    assert!(
        !from.exists(),
        "a later save must not recreate the old path"
    );
}

#[test]
fn inverse_move_notification_restores_presenter_url_for_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let Some(binary) = compile_presenter(dir.path()) else {
        return;
    };
    let from = dir.path().join("presented.txt");
    let to = dir.path().join("temporary.txt");
    fs::write(&from, "original").unwrap();
    let Some(mut presenter) = Presenter::start(&binary, &from) else {
        return;
    };
    presenter.command("edit +user");

    assert_written(coordinate_moving_write(
        &from,
        &to,
        Duration::from_secs(2),
        |scope| {
            scope.move_forward()?;
            append(&to, "+bob")?;
            scope.move_back()
        },
    ));
    assert!(!to.exists());
    assert_eq!(fs::read_to_string(&from).unwrap(), "original+user+bob");
    wait_for_document(&presenter, &from, "original+user+bob");

    assert_eq!(presenter.command("edit +rollback-save")["dirty"], true);
    assert_eq!(presenter.command("save")["event"], "saved");
    assert_eq!(
        fs::read_to_string(&from).unwrap(),
        "original+user+bob+rollback-save"
    );
    assert!(!to.exists());
}

#[test]
fn moving_scope_rejects_destination_collision_without_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("source.pdf");
    let to = dir.path().join("occupied.pdf");
    fs::write(&from, "source").unwrap();
    fs::write(&to, "occupant").unwrap();

    let outcome =
        coordinate_moving_write(&from, &to, Duration::from_secs(2), |scope| {
            scope.move_forward()
        });
    let Coordination::Granted { value, .. } = outcome else {
        panic!("coordinated accessor should run")
    };
    assert_eq!(value.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&from).unwrap(), "source");
    assert_eq!(fs::read_to_string(&to).unwrap(), "occupant");
}

#[test]
fn moving_scope_keeps_accessor_errors_inside_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("source.pdf");
    let to = dir.path().join("destination.pdf");
    fs::write(&from, "source").unwrap();

    let outcome =
        coordinate_moving_write(&from, &to, Duration::from_secs(2), |scope| {
            scope.move_forward()?;
            Err(std::io::Error::other("verification failed"))
        });
    let Coordination::Granted { value, .. } = outcome else {
        panic!("accessor error must remain a granted value")
    };
    assert_eq!(value.unwrap_err().to_string(), "verification failed");
    assert!(!from.exists());
    assert_eq!(fs::read_to_string(&to).unwrap(), "source");
}

#[test]
fn changed_accessor_url_does_not_run_the_move_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let planned = dir.path().join("planned.pdf");
    let actual = dir.path().join("relocated.pdf");
    let mut called = false;
    let started =
        super::macos::start_accessor_for_test(&actual, &planned, || {
            called = true
        });
    assert!(!started);
    assert!(!called);
}

#[test]
fn nsdocument_saves_reloads_and_holds_a_blocked_presenter() {
    let dir = tempfile::tempdir().unwrap();
    let Some(binary) = compile_presenter(dir.path()) else {
        return;
    };

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
    let forbidden = path.with_file_name("forbidden.txt");
    let outcome = coordinate_moving_write(
        &path,
        &forbidden,
        Duration::from_secs(2),
        |scope| {
            scope.move_forward()?;
            append(&forbidden, "+FORBIDDEN")
        },
    );
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
    assert!(!forbidden.exists());
    assert_eq!(presenter.receive()["event"], "unblocked");
    assert_eq!(presenter.command("save")["event"], "saved");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "original+user+bob+unsaved"
    );
    assert_eq!(presenter.command("state")["dirty"], false);
}
