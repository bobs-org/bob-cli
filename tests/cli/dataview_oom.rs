//! Native Dataview FLATTEN budget and allocation regression tests.

use crate::support::*;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

const FLATTEN_LIMIT: usize = 100_000;
#[cfg(target_os = "linux")]
const AS_LIMIT_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(target_os = "linux")]
const GROUPED_DELTA_LIMIT: u64 = 128 * 1024 * 1024;
#[cfg(target_os = "linux")]
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

const CENSUS_QUERY: &str = r#"
TABLE WITHOUT ID Status, length(rows) AS Count
FLATTEN file.tasks AS t
GROUP BY t.status AS Status
SORT Status ASC
"#;

const FILTERED_CENSUS_QUERY: &str = r#"
TABLE WITHOUT ID Status, length(rows) AS Count
FLATTEN file.tasks AS t
WHERE t.blockId = "t-1"
GROUP BY t.status AS Status
SORT Status ASC
"#;

#[cfg(target_os = "linux")]
const LIST_BASELINE_QUERY: &str = "LIST LIMIT 0";

#[test]
fn flatten_default_budget_fails_cli_json_and_markdown() {
    let temp = TempDir::new("bob-cli-dataview-flatten-budget");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Grid.md"),
        &format!(
            "---\n{}{}---\n# Grid\n",
            yaml_int_array("left", 500),
            yaml_int_array("right", 201),
        ),
    );

    let query = r#"TABLE WITHOUT ID left, right FROM "Grid.md" FLATTEN left AS x FLATTEN right AS y"#;
    for format in ["json", "markdown"] {
        let output = run_query(&vault, format, query);
        assert_eq!(
            output.status.code(),
            Some(1),
            "flatten budget should fail {format}:\n{}",
            format_output(&output)
        );
        assert!(
            stdout(&output).is_empty(),
            "flatten budget must not emit partial {format} stdout:\n{}",
            format_output(&output)
        );
        let err = stderr(&output);
        assert!(
            err.contains("native query failed"),
            "expected NativeQuery stderr for {format}:\n{}",
            format_output(&output)
        );
        assert!(
            err.contains(&format!(
                "FLATTEN right AS y would exceed the {FLATTEN_LIMIT}-row limit"
            )),
            "expected flatten diagnostic for {format}:\n{}",
            format_output(&output)
        );
        assert!(
            err.contains("at least "),
            "count should be a lower bound for {format}:\n{}",
            format_output(&output)
        );
        assert!(
            err.contains("DQL TASK query"),
            "diagnostic should mention TASK for {format}:\n{}",
            format_output(&output)
        );
    }

    let filtered = run_query(
        &vault,
        "json",
        r#"TABLE WITHOUT ID left FROM "Grid.md" WHERE false FLATTEN left AS x FLATTEN right AS y"#,
    );
    assert_success(&filtered);
    let json: Value =
        serde_json::from_str(stdout(&filtered).trim()).expect("filtered json");
    assert_eq!(json["result"]["values"], serde_json::json!([]));
}

#[test]
fn flatten_later_limit_does_not_hide_default_budget() {
    let temp = TempDir::new("bob-cli-dataview-flatten-later-limit");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Grid.md"),
        &format!(
            "---\n{}{}---\n# Grid\n",
            yaml_int_array("left", 500),
            yaml_int_array("right", 201),
        ),
    );
    let output = run_query(
        &vault,
        "json",
        r#"TABLE WITHOUT ID left, right FROM "Grid.md" FLATTEN left AS x FLATTEN right AS y LIMIT 1"#,
    );
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(stdout(&output).is_empty(), "{}", format_output(&output));
    assert!(
        stderr(&output).contains(&format!("{FLATTEN_LIMIT}-row limit")),
        "{}",
        format_output(&output)
    );
}

#[test]
fn synthetic_task_census_counts_are_exact() {
    for count in [1_000usize, 2_000, 2_413] {
        let temp = TempDir::new(&format!("bob-cli-dataview-census-{count}"));
        let vault = temp.path().join("vault");
        write_file(&vault.join("Tasks.md"), &task_note(count));
        let output = run_query(&vault, "json", CENSUS_QUERY);
        assert_success(&output);
        let json: Value = serde_json::from_str(stdout(&output).trim())
            .unwrap_or_else(|error| {
                panic!(
                    "census json {count}: {error}\n{}",
                    format_output(&output)
                )
            });
        assert_eq!(
            json["result"]["values"],
            census_counts(count),
            "census {count}:\n{}",
            format_output(&output)
        );

        let filtered = run_query(&vault, "json", FILTERED_CENSUS_QUERY);
        assert_success(&filtered);
        let json: Value = serde_json::from_str(stdout(&filtered).trim())
            .expect("filtered census json");
        assert_eq!(
            json["result"]["values"],
            serde_json::json!([["x", 1]]),
            "filtered census {count}:\n{}",
            format_output(&filtered)
        );
    }
}

#[test]
fn synthetic_list_census_counts_nested_children() {
    let temp = TempDir::new("bob-cli-dataview-list-census");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Lists.md"),
        "# Lists\n\n- parent\n  - child a\n  - child b\n- sibling\n",
    );
    let output = run_query(
        &vault,
        "json",
        r#"
TABLE WITHOUT ID Kind, length(rows) AS Count
FLATTEN file.lists AS item
GROUP BY choice(typeof(item.parent) = "number", "child", "top") AS Kind
SORT Kind ASC
"#,
    );
    assert_success(&output);
    let json: Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(
        json["result"]["values"],
        serde_json::json!([["child", 2], ["top", 2]])
    );
}

#[cfg(target_os = "linux")]
#[test]
fn synthetic_task_census_stays_under_address_space_cap() {
    let mut measurements = Vec::new();
    for count in [1_000usize, 2_000, 2_413] {
        let temp = TempDir::new(&format!("bob-cli-dataview-rss-{count}"));
        let vault = temp.path().join("vault");
        write_file(&vault.join("Tasks.md"), &task_note(count));

        let baseline = run_limited_query(&vault, LIST_BASELINE_QUERY);
        assert_eq!(
            baseline.code,
            Some(0),
            "LIST LIMIT 0 failed for {count}: {}",
            baseline.summary()
        );
        let census = run_limited_query(&vault, CENSUS_QUERY);
        assert_eq!(
            census.code,
            Some(0),
            "census failed under 512 MiB for {count}: {}",
            census.summary()
        );
        let json: Value = serde_json::from_str(census.stdout.trim())
            .unwrap_or_else(|error| {
                panic!(
                    "capped census json {count}: {error}\n{}",
                    census.summary()
                )
            });
        assert_eq!(json["result"]["values"], census_counts(count));
        eprintln!(
            "dataview flatten RSS: tasks={count} baseline={} census={} delta={}",
            format_bytes(baseline.max_rss),
            format_bytes(census.max_rss),
            format_bytes(census.max_rss.saturating_sub(baseline.max_rss))
        );
        measurements.push((count, baseline.max_rss, census.max_rss));
    }

    let two_thousand = measurements
        .iter()
        .find(|(count, _, _)| *count == 2_000)
        .expect("2000-task measurement");
    let delta = two_thousand.2.saturating_sub(two_thousand.1);
    assert!(
        delta < GROUPED_DELTA_LIMIT,
        "2000-task grouped census added {} above index baseline (limit {})",
        format_bytes(delta),
        format_bytes(GROUPED_DELTA_LIMIT)
    );
}

fn run_query(vault: &Path, format: &str, query: &str) -> Output {
    query_command(vault, format, query)
        .output()
        .unwrap_or_else(|error| panic!("run bob query: {error}"))
}

fn query_command(vault: &Path, format: &str, query: &str) -> Command {
    let mut command = bob_command();
    command
        .arg("query")
        .arg("--bob-dir")
        .arg(vault)
        .arg("--engine")
        .arg("native")
        .arg("--format")
        .arg(format)
        .arg("--query")
        .arg(query);
    command
}

fn yaml_int_array(name: &str, count: usize) -> String {
    let items = (0..count)
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}: [{items}]\n")
}

fn task_note(count: usize) -> String {
    let mut body = String::from("# Tasks\n\n");
    for index in 0..count {
        let marker = if index % 2 == 0 { ' ' } else { 'x' };
        body.push_str(&format!("- [{marker}] Task {index} ^t-{index}\n"));
    }
    body
}

fn census_counts(count: usize) -> Value {
    let open = count.div_ceil(2);
    let done = count / 2;
    serde_json::json!([[" ", open], ["x", done]])
}

#[cfg(target_os = "linux")]
fn format_bytes(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

#[cfg(target_os = "linux")]
struct LimitedOutput {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    max_rss: u64,
}

#[cfg(target_os = "linux")]
impl LimitedOutput {
    fn summary(&self) -> String {
        format!(
            "status: {:?}\nmax_rss: {}\nstdout:\n{}\nstderr:\n{}",
            self.code,
            format_bytes(self.max_rss),
            self.stdout,
            self.stderr
        )
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::zombie_processes)]
fn run_limited_query(vault: &Path, query: &str) -> LimitedOutput {
    use std::io::{self, Read};
    use std::mem;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::thread;
    use std::time::{Duration, Instant};

    let mut command = query_command(vault, "json", query);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: AS_LIMIT_BYTES,
                rlim_max: AS_LIMIT_BYTES,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }

    let mut child = command.spawn().expect("spawn capped bob query");
    let mut stdout_pipe = child.stdout.take().expect("stdout pipe");
    let mut stderr_pipe = child.stderr.take().expect("stderr pipe");
    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let pid = child.id() as libc::pid_t;
    let started = Instant::now();
    loop {
        let mut status = 0;
        let mut usage = unsafe { mem::zeroed::<libc::rusage>() };
        let waited =
            unsafe { libc::wait4(pid, &mut status, libc::WNOHANG, &mut usage) };
        if waited < 0 {
            panic!("wait4 failed: {}", io::Error::last_os_error());
        }
        if waited == pid {
            mem::forget(child);
            let stdout = String::from_utf8_lossy(
                &stdout_handle.join().expect("join stdout"),
            )
            .into_owned();
            let stderr = String::from_utf8_lossy(
                &stderr_handle.join().expect("join stderr"),
            )
            .into_owned();
            return LimitedOutput {
                code: wait_status_code(status),
                stdout,
                stderr,
                max_rss: (usage.ru_maxrss as u64).saturating_mul(1024),
            };
        }
        if started.elapsed() >= QUERY_TIMEOUT {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
            let mut status = 0;
            let mut usage = unsafe { mem::zeroed::<libc::rusage>() };
            unsafe {
                libc::wait4(pid, &mut status, 0, &mut usage);
            }
            mem::forget(child);
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            panic!(
                "bob query exceeded {}s under a 512 MiB address-space cap",
                QUERY_TIMEOUT.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "linux")]
fn wait_status_code(status: i32) -> Option<i32> {
    if libc::WIFEXITED(status) {
        Some(libc::WEXITSTATUS(status))
    } else {
        None
    }
}
