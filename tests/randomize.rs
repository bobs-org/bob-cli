//! Integration coverage for `bob randomize`: CLI registration, dry-run
//! previews, live rewrites, git cooperation, failure modes, the JSON
//! contract, and `task-status-hooks` parity.
//!
//! The few helpers below are copied from `tests/cli.rs` on purpose so
//! this file stays self-contained instead of growing that file.

use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;

const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const TEST_MISSING_CONFIG_FILE: &str =
    "/definitely/missing/bob-cli-test-config.yml";
const BOB_NOW: &str = "2026-09-28 12:00:00";
const SEED: &str = "0x7f3a91c2";

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{}-{}-{}-{}",
            prefix,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before epoch")
                .as_nanos(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("create temp dir {}: {error}", path.display())
        });
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn bob_command() -> Command {
    let mut command = Command::new(BOB_BIN);
    command.env("BOB_CONFIG_FILE", TEST_MISSING_CONFIG_FILE);
    let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let isolation = std::env::temp_dir()
        .join(format!("bob-cli-test-iso-{}-{nonce}", std::process::id()));
    let _ = fs::create_dir_all(&isolation);
    command.env("BOB_VAULT_SYNC_LOCK_FILE", isolation.join("bob_sync.lock"));
    command.env("XDG_STATE_HOME", isolation.join("state"));
    command
}

/// Point a command at a fixture vault with pinned clocks. The state home
/// is pinned under `temp` so tests can assert on recovery records.
fn vault_env(
    command: &mut Command,
    temp: &TempDir,
    vault: &Path,
    config: &Path,
) -> PathBuf {
    let daily = vault.join("2026/20260928.md");
    command
        .env("BOB_DIR", vault)
        .env("BOB_NOW", BOB_NOW)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_CONFIG_FILE", config)
        .env("NO_COLOR", "1")
        .env("XDG_STATE_HOME", temp.path().join("state"));
    daily
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn git<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("color.status=false")
        .args(args)
        .output()
        .expect("run git")
}

fn git_in<I, S>(directory: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("color.status=false")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .expect("run git");
    assert_success(&output);
    output
}

/// `git` output without asserting success, for log inspection.
fn git_capture<I, S>(directory: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .expect("run git")
}

/// Owned output lines of a `git` inspection command.
fn git_lines<I, S>(directory: &Path, args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    stdout(&git_capture(directory, args))
        .lines()
        .map(str::to_string)
        .collect()
}

fn path_str(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("path is not UTF-8: {}", path.display()))
}

fn configure_test_git_identity(repo: &Path) {
    git_in(repo, ["config", "user.name", "Test User"]);
    git_in(repo, ["config", "user.email", "test@example.com"]);
    git_in(repo, ["config", "commit.gpgsign", "false"]);
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command success:\n{}",
        format_output(output)
    );
}

fn assert_exit(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "expected exit {code}:\n{}",
        format_output(output)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn format_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(output),
        stderr(output)
    )
}

fn parse_json(output: &Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("parse JSON stdout: {error}\n{}", format_output(output))
    })
}

fn write_priority_config(path: &Path) {
    write_file(
        path,
        concat!(
            "properties:\n",
            "  - name: priority\n",
            "    values: priority\n",
            "    schedules: scheduled\n",
            "    levels:\n",
            "      - label: P1\n",
            "        value: high\n",
            "        min_days: 2\n",
            "        max_days: 7\n",
            "      - label: P2\n",
            "        value: medium\n",
            "        min_days: 8\n",
            "        max_days: 30\n",
            "      - label: P3\n",
            "        value: low\n",
            "        min_days: 31\n",
            "        max_days: 90\n",
            "      - label: P4\n",
            "        value: lowest\n",
            "        min_days: 91\n",
            "        max_days: 365\n",
        ),
    );
}

fn write_blocked_tasks_settings(vault: &Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        concat!(
            "{\n",
            "  \"globalFilter\": \"#task\",\n",
            "  \"taskFormat\": \"dataview\",\n",
            "  \"statusSettings\": {\n",
            "    \"coreStatuses\": [\n",
            "      {\"symbol\":\" \",\"name\":\"Todo\",\"nextStatusSymbol\":\"x\",\"availableAsCommand\":true,\"type\":\"TODO\"},\n",
            "      {\"symbol\":\"x\",\"name\":\"Done\",\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,\"type\":\"DONE\"}\n",
            "    ],\n",
            "    \"customStatuses\": [\n",
            "      {\"symbol\":\"/\",\"name\":\"In Progress\",\"nextStatusSymbol\":\"x\",\"availableAsCommand\":true,\"type\":\"IN_PROGRESS\"},\n",
            "      {\"symbol\":\"*\",\"name\":\"Next\",\"nextStatusSymbol\":\"x\",\"availableAsCommand\":true,\"type\":\"ON_HOLD\"},\n",
            "      {\"symbol\":\"?\",\"name\":\"Blocked\",\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,\"type\":\"ON_HOLD\"},\n",
            "      {\"symbol\":\"-\",\"name\":\"Canceled\",\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,\"type\":\"CANCELLED\"}\n",
            "    ]\n",
            "  }\n",
            "}\n",
        ),
    );
}

struct Fixture {
    project: PathBuf,
    plain: PathBuf,
    daily: PathBuf,
}

/// Project note (grouping-eligible), ordinary note, and a daily note
/// with an open Pomodoro linking one due P2 task.
fn write_main_fixture(vault: &Path) -> Fixture {
    let project = vault.join("projects/alpha.md");
    let plain = vault.join("notes/plain.md");
    let daily = vault.join("2026/20260928.md");
    write_blocked_tasks_settings(vault);
    write_file(
        &project,
        concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "# Alpha\n\n",
            "## Tasks\n\n",
            "- [ ] Alpha P1 [priority:: high] [scheduled:: 2026-09-10] #task ^alpha-p1\n",
            "- [ ] Alpha P2 [priority:: medium] [scheduled:: 2026-09-10] #task ^alpha-p2\n",
            "- [ ] Alpha P3 [priority:: low] [scheduled:: 2026-09-10] #task ^alpha-p3\n",
            "- [ ] Linked P2 [priority:: medium] [scheduled:: 2026-09-10] #task ^linked\n",
            "- [ ] Due P0 [scheduled:: 2026-09-10] #task\n",
            "- [ ] Future P2 [priority:: medium] [scheduled:: 2026-12-01] #task\n",
            "- [*] Next P2 [priority:: medium] [scheduled:: 2026-09-10] #task\n",
            "- [/] Working P2 [priority:: medium] [scheduled:: 2026-09-10] #task\n",
            "- [x] Done P2 [priority:: medium] [scheduled:: 2026-09-10] #task\n",
        ),
    );
    write_file(
        &plain,
        concat!(
            "# Plain\n\n",
            "## Tasks\n\n",
            "- [ ] Plain P2 [priority:: medium] [scheduled:: 2026-09-11] #task\n",
            "- [ ] Plain dup [priority:: medium] [priority:: high] [scheduled:: 2026-09-10] #task\n",
            "- [ ] Plain bad date [priority:: medium] [scheduled:: someday] #task\n",
            "- [ ] Plain unknown [priority:: highest] [scheduled:: 2026-09-10] #task\n",
            "- [ ] Plain due field [priority:: medium] [scheduled:: 2026-09-10] [due:: 2026-09-20] #task\n",
            "- [ ] Plain repeat [priority:: medium] [scheduled:: 2026-09-10] [repeat:: every week] #task\n",
        ),
    );
    write_file(
        &daily,
        concat!(
            "# 2026-09-28\n\n",
            "## Pomodoros\n\n",
            "- [ ] Focus (0900-0930)\n",
            "  - [[alpha#^linked]]\n",
            "\n",
            "## Tasks\n\n",
            "- [ ] Daily P2 [priority:: medium] [scheduled:: 2026-09-10] #task\n",
        ),
    );
    Fixture {
        project,
        plain,
        daily,
    }
}

fn init_pair(temp: &TempDir) -> (PathBuf, PathBuf) {
    let vault = temp.path().join("vault");
    let remote = temp.path().join("remote.git");
    fs::create_dir_all(&vault).expect("create vault");
    git(["init", "-q", "--bare", path_str(&remote)]);
    git([
        "--git-dir",
        path_str(&remote),
        "symbolic-ref",
        "HEAD",
        "refs/heads/master",
    ]);
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["symbolic-ref", "HEAD", "refs/heads/master"]);
    configure_test_git_identity(&vault);
    git_in(&vault, ["remote", "add", "origin", path_str(&remote)]);
    (vault, remote)
}

/// Clone the peer only after the vault's initial push, mirroring
/// `tests/cli.rs::init_vault_sync_pair`.
fn clone_peer(temp: &TempDir, remote: &Path) -> PathBuf {
    let peer = temp.path().join("peer");
    git(["clone", "-q", path_str(remote), path_str(&peer)]);
    configure_test_git_identity(&peer);
    peer
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
}

#[test]
fn randomize_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("randomize")
        .arg("--help")
        .output()
        .expect("run bob randomize --help");
    assert_success(&output);
    let help = stdout(&output);
    for marker in [
        "Preview what would move and where",
        "BOB_DIR",
        "BOB_PRIORITY_ROLL_SEED",
    ] {
        assert!(
            help.contains(marker),
            "expected randomize help to contain {marker:?}:\n{help}"
        );
    }
    let positions = [
        "--dry-run",
        "--format",
        "--help",
        "--level",
        "--offline",
        "--retry-timeout",
        "--seed",
        "--until",
    ]
    .map(|option| {
        help.find(option).unwrap_or_else(|| {
            panic!("expected randomize help to list {option}:\n{help}")
        })
    });
    let mut sorted = positions.to_vec();
    sorted.sort_unstable();
    assert_eq!(
        positions.to_vec(),
        sorted,
        "randomize options are not alphabetical:\n{help}"
    );
    for (long, short) in [
        ("--dry-run", "-d"),
        ("--format", "-f"),
        ("--help", "-h"),
        ("--level", "-l"),
        ("--offline", "-o"),
        ("--retry-timeout", "-r"),
        ("--seed", "-s"),
        ("--until", "-u"),
    ] {
        assert!(
            help.contains(short),
            "expected short alias {short} for {long}:\n{help}"
        );
    }
    let top = bob_command()
        .arg("--help")
        .output()
        .expect("run bob --help");
    assert_success(&top);
    assert!(
        stdout(&top).contains("randomize"),
        "expected top-level help to list randomize:\n{}",
        format_output(&top)
    );
}

#[test]
fn randomize_dry_run_writes_nothing_and_prints_replay() {
    let temp = TempDir::new("bob-cli-randomize-dry");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    let fixture = write_main_fixture(&vault);
    let before = [
        fs::read_to_string(&fixture.project).expect("read project"),
        fs::read_to_string(&fixture.plain).expect("read plain"),
        fs::read_to_string(&fixture.daily).expect("read daily"),
    ];

    // A held lock must not block a dry run: no lock is taken.
    let lock_path = temp.path().join("held.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold lock");

    let mut command = bob_command();
    command
        .arg("randomize")
        .arg("--dry-run")
        .arg("--seed")
        .arg(SEED);
    vault_env(&mut command, &temp, &vault, &config);
    command.env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path);
    let output = command.output().expect("run bob randomize --dry-run");
    drop(lock);
    assert_success(&output);

    let human = stdout(&output);
    for marker in [
        "dry run",
        "Would re-roll 5 tasks in 3 notes",
        "projects/alpha.md (3)",
        "notes/plain.md (1)",
        "2026/20260928.md (1)",
        "P1  high",
        "Next 5 weeks",
        "Left alone",
        "Still due",
        "1 P0 task",
        "tasks need a look",
        "Nothing was written. Apply these dates with: bob randomize --seed 0x7f3a91c2",
    ] {
        assert!(
            human.contains(marker),
            "expected dry-run output to contain {marker:?}:\n{}",
            format_output(&output)
        );
    }
    assert!(
        !human.contains("Committed"),
        "dry run must not commit:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "expected clean dry-run stderr:\n{}",
        format_output(&output)
    );

    assert_eq!(
        fs::read_to_string(&fixture.project).expect("read project"),
        before[0],
        "dry run must not write notes"
    );
    assert_eq!(
        fs::read_to_string(&fixture.plain).expect("read plain"),
        before[1],
        "dry run must not write notes"
    );
    assert_eq!(
        fs::read_to_string(&fixture.daily).expect("read daily"),
        before[2],
        "dry run must not write notes"
    );
    assert!(
        !temp.path().join("state/bob-cli/randomize").exists(),
        "dry run must not create a recovery directory"
    );
}

#[test]
fn randomize_live_offline_rewrites_notes_with_status_log_and_grouping() {
    let temp = TempDir::new("bob-cli-randomize-live");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    let fixture = write_main_fixture(&vault);
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    commit_all(&vault, "initial vault");

    let mut command = bob_command();
    command
        .arg("randomize")
        .arg("--offline")
        .arg("--format")
        .arg("json")
        .arg("--seed")
        .arg(SEED);
    vault_env(&mut command, &temp, &vault, &config);
    let output = command.output().expect("run bob randomize live");
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["git"]["mode"], "offline");
    assert_eq!(json["summary"]["rerolled"], 5);
    assert_eq!(json["summary"]["notes"], 3);
    assert_eq!(json["summary"]["still_due_p0"], 1);

    // The same seed must replay the same dates: collect first.
    let dates: Vec<(String, u64, String, String)> = json["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .map(|task| {
            (
                task["path"].as_str().expect("path").to_string(),
                task["line"].as_u64().expect("line"),
                task["from"].as_str().expect("from").to_string(),
                task["to"].as_str().expect("to").to_string(),
            )
        })
        .collect();
    assert_eq!(dates.len(), 5);

    let project = fs::read_to_string(&fixture.project).expect("read project");
    let plain = fs::read_to_string(&fixture.plain).expect("read plain");
    let daily = fs::read_to_string(&fixture.daily).expect("read daily");

    // Every rerolled task lands its new date, Blocked status, and log.
    for (path, _line, _from, to) in &dates {
        let contents = match path.as_str() {
            "projects/alpha.md" => &project,
            "notes/plain.md" => &plain,
            "2026/20260928.md" => &daily,
            other => panic!("unexpected task path {other}"),
        };
        assert!(
            contents.contains(&format!("[scheduled:: {to}]")),
            "expected new date {to} in {path}:\n{contents}"
        );
    }
    assert!(
        project.contains("🎲 P1 randomize")
            && project.contains("🎲 P2 randomize")
            && project.contains("🎲 P3 randomize"),
        "expected randomize log reasons in project:\n{project}"
    );
    assert!(
        project.contains("### Blocked"),
        "expected regrouped Blocked section in project:\n{project}"
    );
    assert!(
        plain.contains("🎲 P2 randomize"),
        "expected randomize log reason in plain note:\n{plain}"
    );
    // Excluded tasks are byte-untouched.
    for line in [
        "- [ ] Due P0 [scheduled:: 2026-09-10] #task\n",
        "- [ ] Future P2 [priority:: medium] [scheduled:: 2026-12-01] #task\n",
        "- [*] Next P2 [priority:: medium] [scheduled:: 2026-09-10] #task\n",
        "- [/] Working P2 [priority:: medium] [scheduled:: 2026-09-10] #task\n",
        "- [x] Done P2 [priority:: medium] [scheduled:: 2026-09-10] #task\n",
        "- [ ] Linked P2 [priority:: medium] [scheduled:: 2026-09-10] #task ^linked\n",
    ] {
        assert!(
            project.contains(line),
            "expected untouched line {line:?}:\n{project}"
        );
    }
    for line in [
        "- [ ] Plain dup [priority:: medium] [priority:: high] [scheduled:: 2026-09-10] #task\n",
        "- [ ] Plain bad date [priority:: medium] [scheduled:: someday] #task\n",
        "- [ ] Plain unknown [priority:: highest] [scheduled:: 2026-09-10] #task\n",
        "- [ ] Plain due field [priority:: medium] [scheduled:: 2026-09-10] [due:: 2026-09-20] #task\n",
        "- [ ] Plain repeat [priority:: medium] [scheduled:: 2026-09-10] [repeat:: every week] #task\n",
    ] {
        assert!(
            plain.contains(line),
            "expected untouched line {line:?}:\n{plain}"
        );
    }

    // Exactly one scoped local commit with the contract message.
    let subjects = git_lines(&vault, ["log", "--format=%H %s"]);
    assert_eq!(
        subjects.len(),
        2,
        "expected initial + randomize commits:\n{subjects:?}"
    );
    assert!(
        subjects[0].contains("bob randomize 2026-09-28: 5 tasks in 3 notes"),
        "unexpected commit subject:\n{subjects:?}"
    );
    let sha = subjects[0]
        .split_whitespace()
        .next()
        .expect("sha")
        .to_string();
    let body = stdout(&git_capture(&vault, ["log", "--format=%B", "-n", "1"]));
    for marker in [
        "P1 1 · P2 3 · P3 1",
        "until 2026-09-28 · seed 0x7f3a91c2",
        "3 projects/alpha.md",
        "1 2026/20260928.md",
        "1 notes/plain.md",
    ] {
        assert!(
            body.contains(marker),
            "expected commit body to contain {marker:?}:\n{body}"
        );
    }
    let mut changed =
        git_lines(&vault, ["show", "--name-only", "--format=", &sha]);
    changed.sort_unstable();
    assert_eq!(
        changed,
        vec!["2026/20260928.md", "notes/plain.md", "projects/alpha.md"],
        "scoped commit must contain only rewritten notes"
    );

    // The JSON notes section mirrors the commit.
    let note_paths: Vec<&str> = json["notes"]
        .as_array()
        .expect("notes array")
        .iter()
        .map(|note| note["path"].as_str().expect("note path"))
        .collect();
    assert_eq!(
        note_paths,
        vec!["projects/alpha.md", "2026/20260928.md", "notes/plain.md"],
        "notes ordered by count desc then path:\n{json}"
    );
    let recovery = json["recovery_directory"]
        .as_str()
        .expect("recovery directory");
    assert!(
        Path::new(recovery).join("manifest.json").is_file(),
        "missing recovery manifest in {recovery}"
    );
}

#[test]
fn randomize_needs_a_look_skips_are_reported() {
    let temp = TempDir::new("bob-cli-randomize-skips");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_main_fixture(&vault);

    let mut command = bob_command();
    command
        .arg("randomize")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--seed")
        .arg(SEED);
    vault_env(&mut command, &temp, &vault, &config);
    let output = command.output().expect("run bob randomize skips");
    assert_success(&output);
    let json = parse_json(&output);
    let skipped: Vec<(String, u64, String, Option<String>)> = json["skipped"]
        .as_array()
        .expect("skipped array")
        .iter()
        .map(|skip| {
            (
                skip["path"].as_str().expect("path").to_string(),
                skip["line"].as_u64().expect("line"),
                skip["reason"].as_str().expect("reason").to_string(),
                skip["detail"].as_str().map(str::to_string),
            )
        })
        .collect();
    for expected in [
        ("notes/plain.md", 6u64, "duplicate_field", None),
        ("notes/plain.md", 7u64, "invalid_scheduled", None),
        (
            "notes/plain.md",
            8u64,
            "unknown_priority",
            Some("highest".to_string()),
        ),
        ("notes/plain.md", 9u64, "hard_date", None),
        ("notes/plain.md", 10u64, "hard_date", None),
        ("projects/alpha.md", 14u64, "next", None),
        ("projects/alpha.md", 15u64, "in_progress", None),
        ("projects/alpha.md", 11u64, "pomodoro", None),
    ] {
        assert!(
            skipped.contains(&(
                expected.0.to_string(),
                expected.1,
                expected.2.to_string(),
                expected.3.clone()
            )),
            "expected skip {expected:?} in {skipped:?}"
        );
    }
}

fn task_dates(json: &serde_json::Value) -> Vec<(String, u64, String, String)> {
    json["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .map(|task| {
            (
                task["path"].as_str().expect("path").to_string(),
                task["line"].as_u64().expect("line"),
                task["from"].as_str().expect("from").to_string(),
                task["to"].as_str().expect("to").to_string(),
            )
        })
        .collect()
}

fn run_randomize(
    temp: &TempDir,
    vault: &Path,
    config: &Path,
    args: &[&str],
) -> Output {
    let mut command = bob_command();
    command.arg("randomize");
    for arg in args {
        command.arg(arg);
    }
    vault_env(&mut command, temp, vault, config);
    command.output().expect("run bob randomize")
}

#[test]
fn randomize_seed_replays_dry_run_dates_filters_levels_and_until() {
    let temp = TempDir::new("bob-cli-randomize-seed");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_main_fixture(&vault);
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    commit_all(&vault, "initial vault");

    let dry = run_randomize(
        &temp,
        &vault,
        &config,
        &["--dry-run", "--format", "json", "--seed", SEED],
    );
    assert_success(&dry);
    let dry_json = parse_json(&dry);
    assert_eq!(dry_json["seed"], SEED);

    // --level filters to P2; everything else counts as not selected.
    let filtered = run_randomize(
        &temp,
        &vault,
        &config,
        &[
            "--dry-run",
            "--format",
            "json",
            "--seed",
            SEED,
            "--level",
            "p2",
            "--level",
            "P2",
        ],
    );
    assert_success(&filtered);
    let filtered_json = parse_json(&filtered);
    assert_eq!(filtered_json["levels"], serde_json::json!(["P2"]));
    assert_eq!(filtered_json["summary"]["rerolled"], 3);
    for task in filtered_json["tasks"].as_array().expect("tasks array") {
        assert_eq!(task["level"], "P2");
    }
    let not_selected = filtered_json["skipped"]
        .as_array()
        .expect("skipped array")
        .iter()
        .filter(|skip| skip["reason"] == "not_selected")
        .count();
    assert_eq!(not_selected, 2, "P1 and P3 tasks:\n{filtered_json}");

    // A live run with the same seed replays every dry-run date. This
    // runs last on this vault: it rewrites the fixture, so nothing is
    // due afterwards.
    let live = run_randomize(
        &temp,
        &vault,
        &config,
        &["--offline", "--format", "json", "--seed", SEED],
    );
    assert_success(&live);
    assert_eq!(task_dates(&parse_json(&live)), task_dates(&dry_json));

    // An unknown label is a usage error that lists the valid labels.
    let bad_level =
        run_randomize(&temp, &vault, &config, &["--dry-run", "--level", "P9"]);
    assert_exit(&bad_level, 2);
    assert!(
        stderr(&bad_level).contains("P1, P2, P3, P4"),
        "expected valid labels:\n{}",
        format_output(&bad_level)
    );

    // --until shifts both the cutoff and the roll base on a fresh vault.
    let temp2 = TempDir::new("bob-cli-randomize-until");
    let vault2 = temp2.path().join("vault");
    let config2 = temp2.path().join("config.yml");
    write_priority_config(&config2);
    write_blocked_tasks_settings(&vault2);
    write_file(
        &vault2.join("2026/20260928.md"),
        "# 2026-09-28\n\n## Pomodoros\n\n- [ ] Focus (0900-0930)\n",
    );
    write_file(
        &vault2.join("note.md"),
        "- [ ] Later P2 [priority:: medium] [scheduled:: 2026-10-03] #task\n",
    );
    let default = run_randomize(
        &temp2,
        &vault2,
        &config2,
        &["--dry-run", "--format", "json", "--seed", SEED],
    );
    assert_success(&default);
    assert_eq!(parse_json(&default)["summary"]["rerolled"], 0);

    let shifted = run_randomize(
        &temp2,
        &vault2,
        &config2,
        &[
            "--dry-run",
            "--format",
            "json",
            "--seed",
            SEED,
            "--until",
            "+7",
        ],
    );
    assert_success(&shifted);
    let shifted_json = parse_json(&shifted);
    assert_eq!(shifted_json["until"], "2026-10-05");
    assert_eq!(shifted_json["summary"]["rerolled"], 1);
    let to = shifted_json["tasks"][0]["to"].as_str().expect("to date");
    assert!(
        ("2026-10-13"..="2026-11-04").contains(&to),
        "P2 window rolls 8-30 days from 2026-10-05, got {to}"
    );

    // A past --until is a usage error.
    let past = run_randomize(
        &temp2,
        &vault2,
        &config2,
        &["--dry-run", "--until", "2026-09-27"],
    );
    assert_exit(&past, 2);
    assert!(
        stderr(&past).contains("today or later"),
        "expected past-date hint:\n{}",
        format_output(&past)
    );

    let bad_seed = run_randomize(
        &temp2,
        &vault2,
        &config2,
        &["--dry-run", "--seed", "not-a-seed"],
    );
    assert_exit(&bad_seed, 2);
    assert!(
        stderr(&bad_seed).contains("--seed"),
        "expected seed hint:\n{}",
        format_output(&bad_seed)
    );
}

#[test]
fn randomize_bare_remote_syncs_scoped_commit_and_push() {
    let temp = TempDir::new("bob-cli-randomize-remote");
    let (vault, remote) = init_pair(&temp);
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_main_fixture(&vault);
    commit_all(&vault, "initial vault");
    git_in(&vault, ["push", "-q", "-u", "origin", "master"]);
    let peer = clone_peer(&temp, &remote);

    // A peer's non-overlapping change merges before planning.
    write_file(&peer.join("peer-side.md"), "# peer\n");
    git_in(&peer, ["add", "-A"]);
    git_in(&peer, ["commit", "-q", "-m", "peer side note"]);
    git_in(&peer, ["push", "-q", "origin", "master"]);

    // An unrelated dirty file lands in its own earlier vault() commit.
    write_file(&vault.join("dirty.md"), "# dirty\n");

    let output = run_randomize(&temp, &vault, &config, &["--seed", SEED]);
    assert_success(&output);
    let human = stdout(&output);
    for marker in [
        "Re-rolled 5 tasks in 3 notes",
        "Committed",
        "Pushed to origin/master",
        "seed 0x7f3a91c2",
        "undo:",
    ] {
        assert!(
            human.contains(marker),
            "expected live output to contain {marker:?}:\n{}",
            format_output(&output)
        );
    }

    // The peer change merged before planning.
    assert!(
        vault.join("peer-side.md").is_file(),
        "peer change was not merged"
    );

    let subjects = git_lines(&vault, ["log", "--format=%H %s"]);
    let randomize: Vec<&String> = subjects
        .iter()
        .filter(|line| line.contains("bob randomize "))
        .collect();
    assert_eq!(
        randomize.len(),
        1,
        "expected exactly one bob randomize commit:\n{subjects:?}"
    );
    assert!(
        subjects.iter().any(|line| line.contains("vault(")),
        "expected an earlier vault() pre-sync commit:\n{subjects:?}"
    );
    let sha = randomize[0]
        .split_whitespace()
        .next()
        .expect("sha")
        .to_string();
    let mut changed =
        git_lines(&vault, ["show", "--name-only", "--format=", &sha]);
    changed.sort_unstable();
    assert_eq!(
        changed,
        vec!["2026/20260928.md", "notes/plain.md", "projects/alpha.md"],
        "scoped commit must contain only rewritten notes"
    );

    // It was pushed: the remote matches local HEAD.
    let head = git_capture(&vault, ["rev-parse", "HEAD"]);
    let remote_head = git_capture(&vault, ["rev-parse", "origin/master"]);
    assert_eq!(stdout(&head), stdout(&remote_head));
}

#[test]
fn randomize_offline_commits_without_pushing() {
    let temp = TempDir::new("bob-cli-randomize-offline");
    let (vault, _remote) = init_pair(&temp);
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_main_fixture(&vault);
    commit_all(&vault, "initial vault");
    git_in(&vault, ["push", "-q", "-u", "origin", "master"]);

    let output = run_randomize(
        &temp,
        &vault,
        &config,
        &["--offline", "--format", "json", "--seed", SEED],
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["git"]["mode"], "offline");
    assert!(json["git"]["pre_sync"].is_null());
    assert!(json["git"]["post_sync"].is_null());
    assert_eq!(json["git"]["commit"]["paths"].as_array().unwrap().len(), 3);

    let head = git_capture(&vault, ["rev-parse", "HEAD"]);
    let remote_head = git_capture(&vault, ["rev-parse", "origin/master"]);
    assert_ne!(
        stdout(&head),
        stdout(&remote_head),
        "offline run must not push"
    );
}

#[test]
fn randomize_non_git_vault_warns_and_writes() {
    let temp = TempDir::new("bob-cli-randomize-nongit");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_main_fixture(&vault);

    let output = run_randomize(
        &temp,
        &vault,
        &config,
        &["--format", "json", "--seed", SEED],
    );
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["git"]["mode"], "not_a_worktree");
    assert!(json["git"]["commit"].is_null());
    assert_eq!(json["summary"]["rerolled"], 5);
    assert!(
        stderr(&output).contains("not a git worktree"),
        "expected worktree warning:\n{}",
        format_output(&output)
    );
    let project =
        fs::read_to_string(vault.join("projects/alpha.md")).expect("read");
    assert!(
        project.contains("🎲 P1 randomize") && project.contains("### Blocked"),
        "notes are still written:\n{project}"
    );
}

#[test]
fn randomize_held_lock_fails_fast_with_no_writes() {
    let temp = TempDir::new("bob-cli-randomize-lock");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    let fixture = write_main_fixture(&vault);
    let before = fs::read_to_string(&fixture.project).expect("read project");

    let lock_path = temp.path().join("test.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold lock");

    let mut command = bob_command();
    command
        .arg("randomize")
        .arg("-r")
        .arg("1")
        .arg("--seed")
        .arg(SEED);
    vault_env(&mut command, &temp, &vault, &config);
    command.env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path);
    let output = command.output().expect("run contended randomize");
    drop(lock);
    assert_exit(&output, 1);
    assert!(
        stderr(&output).contains("maintenance run"),
        "expected lock hint:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&fixture.project).expect("read project"),
        before,
        "contended run must not write"
    );
    assert!(
        !temp.path().join("state/bob-cli/randomize").exists(),
        "contended run must not create recovery state"
    );
}

#[test]
fn randomize_unreachable_remote_fails_presync_with_offline_hint() {
    let temp = TempDir::new("bob-cli-randomize-unreachable");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    let fixture = write_main_fixture(&vault);
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    git_in(
        &vault,
        ["remote", "add", "origin", "/nonexistent/remote.git"],
    );
    commit_all(&vault, "initial vault");
    let before = fs::read_to_string(&fixture.project).expect("read project");

    let output = run_randomize(&temp, &vault, &config, &["--seed", SEED]);
    assert_exit(&output, 1);
    assert!(
        stderr(&output).contains("--offline"),
        "expected offline hint:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(&fixture.project).expect("read project"),
        before,
        "failed pre-sync must not write"
    );

    let json_output = run_randomize(
        &temp,
        &vault,
        &config,
        &["--format", "json", "--seed", SEED],
    );
    assert_exit(&json_output, 1);
    let json = parse_json(&json_output);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["stage"], "pre_sync");
    assert_eq!(json["git"]["mode"], "sync");
}

#[test]
fn randomize_json_contract_keys_and_failure_shape() {
    let temp = TempDir::new("bob-cli-randomize-json");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_main_fixture(&vault);
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    commit_all(&vault, "initial vault");

    let output = run_randomize(
        &temp,
        &vault,
        &config,
        &["--offline", "--format", "json", "--seed", SEED],
    );
    assert_success(&output);
    // Exactly one JSON document on stdout, even with stderr warnings.
    let text = stdout(&output).trim().to_string();
    assert!(
        text.lines().count() == 1
            || serde_json::from_str::<serde_json::Value>(&text).is_ok(),
        "expected one JSON document:\n{}",
        format_output(&output)
    );
    let json = parse_json(&output);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["today"], "2026-09-28");
    assert_eq!(json["until"], "2026-09-28");
    assert_eq!(json["seed"], SEED);
    assert!(json["levels"].is_null());

    for key in ["rerolled", "notes", "unchanged", "still_due_p0", "by_level"] {
        assert!(
            json["summary"].get(key).is_some(),
            "missing summary.{key}:\n{json}"
        );
    }
    let level = &json["summary"]["by_level"][0];
    for key in [
        "label", "value", "count", "min_days", "max_days", "first", "last",
    ] {
        assert!(level.get(key).is_some(), "missing by_level.{key}:\n{json}");
    }
    let task = &json["tasks"][0];
    for key in [
        "path",
        "line",
        "ref",
        "block_id",
        "description",
        "level",
        "value",
        "min_days",
        "max_days",
        "offset_days",
        "from",
        "to",
        "status_from",
        "status_to",
        "schedule_log",
    ] {
        assert!(task.get(key).is_some(), "missing task.{key}:\n{json}");
    }
    assert!(
        ["prepended", "created"]
            .contains(&task["schedule_log"].as_str().expect("log kind")),
        "bad schedule_log value:\n{json}"
    );
    for key in ["path", "line", "reason", "detail"] {
        assert!(
            json["skipped"][0].get(key).is_some(),
            "missing skipped.{key}:\n{json}"
        );
    }
    for key in ["path", "tasks", "regrouped"] {
        assert!(
            json["notes"][0].get(key).is_some(),
            "missing notes.{key}:\n{json}"
        );
    }
    assert_eq!(
        json["load"].as_array().expect("load array").len(),
        35,
        "load covers today+1..=today+35"
    );
    assert_eq!(json["load"][0]["date"], "2026-09-29");
    assert!(json["warnings"].as_array().is_some());
    assert_eq!(json["git"]["mode"], "offline");
    let sha = json["git"]["commit"]["sha"].as_str().expect("sha");
    assert_eq!(sha.len(), 40, "full sha in JSON, got {sha}");
    assert!(json["error"].is_null());
    // Progress goes to stderr in JSON mode; stdout holds only the document.
    assert!(
        !stderr(&output).contains('{'),
        "stderr must not carry JSON:\n{}",
        format_output(&output)
    );

    // Failure shape: no Blocked status, before any write.
    let temp2 = TempDir::new("bob-cli-randomize-noblocked");
    let vault2 = temp2.path().join("vault");
    let config2 = temp2.path().join("config.yml");
    write_priority_config(&config2);
    write_file(
        &vault2.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Todo","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"*","name":"Next","type":"ON_HOLD"}
            ]
          }
        }"##,
    );
    write_file(
        &vault2.join("2026/20260928.md"),
        "# 2026-09-28\n\n## Pomodoros\n\n- [ ] Focus (0900-0930)\n",
    );
    write_file(
        &vault2.join("note.md"),
        "- [ ] Due P1 [priority:: high] [scheduled:: 2026-09-10] #task\n",
    );
    let before = fs::read_to_string(vault2.join("note.md")).expect("read note");
    let failed = run_randomize(
        &temp2,
        &vault2,
        &config2,
        &["--format", "json", "--seed", SEED],
    );
    assert_exit(&failed, 1);
    let failed_json = parse_json(&failed);
    assert_eq!(failed_json["ok"], false);
    assert_eq!(failed_json["error"]["stage"], "plan");
    assert!(
        failed_json["error"]["message"]
            .as_str()
            .expect("message")
            .contains("Blocked"),
        "expected Blocked message:\n{failed_json}"
    );
    assert_eq!(
        fs::read_to_string(vault2.join("note.md")).expect("read note"),
        before,
        "blocked-status failure must not write"
    );
}

#[test]
fn randomize_hooks_parity_after_live_run() {
    let temp = TempDir::new("bob-cli-randomize-parity");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_blocked_tasks_settings(&vault);
    // No Pomodoro links here: hooks must have nothing left to change in
    // the touched notes, including grouping.
    write_file(
        &vault.join("projects/alpha.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "# Alpha\n\n",
            "## Tasks\n\n",
            "- [ ] Alpha P1 [priority:: high] [scheduled:: 2026-09-10] #task ^alpha-p1\n",
            "- [ ] Alpha P2 [priority:: medium] [scheduled:: 2026-09-10] #task ^alpha-p2\n",
        ),
    );
    write_file(
        &vault.join("notes/plain.md"),
        concat!(
            "# Plain\n\n",
            "## Tasks\n\n",
            "- [ ] Plain P2 [priority:: medium] [scheduled:: 2026-09-11] #task\n",
        ),
    );
    write_file(
        &vault.join("2026/20260928.md"),
        concat!(
            "# 2026-09-28\n\n",
            "## Pomodoros\n\n",
            "- [ ] Focus (0900-0930)\n",
            "  - jotting, no task link\n",
        ),
    );

    let live =
        run_randomize(&temp, &vault, &config, &["--offline", "--seed", SEED]);
    assert_success(&live);

    let mut hooks = bob_command();
    hooks
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", vault.join("2026/20260928.md"))
        .env("BOB_NOW", BOB_NOW)
        .env("BOB_CONFIG_FILE", &config)
        .env("NO_COLOR", "1");
    let output = hooks.output().expect("run task-status-hooks");
    assert_success(&output);
    let text = stdout(&output);
    for touched in ["projects/alpha.md", "notes/plain.md"] {
        assert!(
            !text.contains(touched),
            "hooks still wants to change {touched}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn randomize_nothing_due_missing_config() {
    let temp = TempDir::new("bob-cli-randomize-empty");
    let vault = temp.path().join("vault");
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_blocked_tasks_settings(&vault);
    write_file(
        &vault.join("2026/20260928.md"),
        "# 2026-09-28\n\n## Pomodoros\n\n- [ ] Focus (0900-0930)\n",
    );
    write_file(
        &vault.join("note.md"),
        concat!(
            "- [ ] Due P0 [scheduled:: 2026-09-10] #task\n",
            "- [ ] Future P2 [priority:: medium] [scheduled:: 2026-12-01] #task\n",
        ),
    );
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    commit_all(&vault, "initial vault");

    let output =
        run_randomize(&temp, &vault, &config, &["--offline", "--seed", SEED]);
    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "Nothing to re-roll — no prioritized tasks are due by 2026-09-28."
        ),
        "expected nothing-due line:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).contains("1 P0 task"),
        "expected P0 count:\n{}",
        format_output(&output)
    );
    let log = git_capture(&vault, ["log", "--format=%s"]);
    assert_eq!(
        stdout(&log).lines().count(),
        1,
        "nothing due must not commit:\n{}",
        stdout(&log)
    );

    // A missing config is fatal before any write.
    let mut missing = bob_command();
    missing.arg("randomize").arg("--dry-run");
    vault_env(
        &mut missing,
        &temp,
        &vault,
        Path::new(TEST_MISSING_CONFIG_FILE),
    );
    let missing = missing.output().expect("run without config");
    assert_exit(&missing, 1);
    assert!(
        stderr(&missing).contains("priority levels need"),
        "expected config error:\n{}",
        format_output(&missing)
    );

    let json_missing = {
        let mut command = bob_command();
        command
            .arg("randomize")
            .arg("--dry-run")
            .arg("--format")
            .arg("json");
        vault_env(
            &mut command,
            &temp,
            &vault,
            Path::new(TEST_MISSING_CONFIG_FILE),
        );
        command.output().expect("run without config json")
    };
    assert_exit(&json_missing, 1);
    let json = parse_json(&json_missing);
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["stage"], "config");
}

#[test]
fn randomize_post_sync_conflict_keeps_local_commit_and_warns() {
    let temp = TempDir::new("bob-cli-randomize-conflict");
    let (vault, remote) = init_pair(&temp);
    let config = temp.path().join("config.yml");
    write_priority_config(&config);
    write_main_fixture(&vault);
    commit_all(&vault, "initial vault");
    git_in(&vault, ["push", "-q", "-u", "origin", "master"]);
    let peer = clone_peer(&temp, &remote);

    // One-shot post-commit hook: after randomize commits, the peer
    // pushes a same-line edit so the post-sync merge conflicts.
    let hook = vault.join(".git/hooks/post-commit");
    write_file(
        &hook,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX\n",
                "rm -f \"{hook}\"\n",
                "sed -i 's/^- \\[.\\] Alpha P2.*$/- [?] Peer conflict [priority:: medium] [scheduled:: 2026-11-11] #task ^alpha-p2/' \"{peer}/projects/alpha.md\"\n",
                "git -C \"{peer}\" add -A\n",
                "git -C \"{peer}\" commit -q -m \"peer conflicting edit\"\n",
                "git -C \"{peer}\" push -q origin master\n",
                "exit 0\n",
            ),
            hook = path_str(&hook),
            peer = path_str(&peer),
        ),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions =
            fs::metadata(&hook).expect("hook metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).expect("chmod hook");
    }

    let output = run_randomize(&temp, &vault, &config, &["--seed", SEED]);
    assert_exit(&output, 1);
    assert!(
        stderr(&output).contains("conflict"),
        "expected conflict warning:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("re-running re-rolls"),
        "expected re-run hint:\n{}",
        format_output(&output)
    );
    assert!(
        vault.join("_conflicts").is_dir(),
        "expected _conflicts/ copy"
    );
    // The local commit stands: exactly one bob randomize commit exists.
    let log = git_capture(&vault, ["log", "--format=%s"]);
    assert_eq!(
        stdout(&log)
            .lines()
            .filter(|line| line.contains("bob randomize "))
            .count(),
        1,
        "local commit must stand:\n{}",
        stdout(&log)
    );
}
