//! Shared helpers for CLI integration tests.

use sha2::Digest;
use sha2::Sha256;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub(crate) static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
pub(crate) const TEST_MISSING_CONFIG_FILE: &str =
    "/definitely/missing/bob-cli-test-config.yml";
/// Keeps `highlights doctor` from spawning the real `uv` web-clip adapter;
/// web-article tests override it with their fake adapter.
pub(crate) const TEST_MISSING_WEB_CLIP_ADAPTER: &str =
    "/definitely/missing/bob-cli-test-web-clip-adapter";

pub(crate) fn bob_command() -> Command {
    let mut command = Command::new(BOB_BIN);
    command.env("BOB_CONFIG_FILE", TEST_MISSING_CONFIG_FILE);
    command.env("BOB_REF_JOBS_KICK", "off");
    command.env("BOB_WEB_CLIP_ADAPTER", TEST_MISSING_WEB_CLIP_ADAPTER);
    // Hermetic clipboard display: a stale SSH-forwarded DISPLAY (or
    // WAYLAND_DISPLAY) or a tmux session would otherwise send spawned
    // `bob` down the xclip/wl-paste/tmux path, so the gate must not
    // depend on the session. Tests covering clipboard display set these
    // explicitly.
    command.env_remove("DISPLAY");
    command.env_remove("WAYLAND_DISPLAY");
    command.env_remove("TMUX");
    // Hermetic DNS for the fetch resolved-address check: a wildcard public
    // address replaces real DNS, so fake-curl tests never touch the
    // network to resolve. Tests covering the check itself override this.
    command.env("BOB_HIGHLIGHTS_RESOLVE", "*=203.0.113.1");
    let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let isolation = std::env::temp_dir()
        .join(format!("bob-cli-test-iso-{}-{nonce}", std::process::id()));
    let _ = fs::create_dir_all(&isolation);
    command.env("BOB_VAULT_SYNC_LOCK_FILE", isolation.join("bob_sync.lock"));
    command.env("XDG_STATE_HOME", isolation.join("state"));
    command
}

pub(crate) fn write_capture_task_settings(vault: &Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
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
}

pub(crate) fn write_toggle_task_settings(vault: &Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        r##"{
          "globalFilter": "#task",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","type":"TODO"},
              {"symbol":"x","name":"Done","type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"*","name":"Next","type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","type":"TODO"},
              {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
              {"symbol":"-","name":"Canceled","type":"CANCELLED"}
            ]
          }
        }"##,
    );
}

pub(crate) fn capture_pomodoro_ref(line: &str, line_number: usize) -> String {
    let digest = hex::encode(Sha256::digest(line.trim_end().as_bytes()));
    format!("{line_number}:{}", &digest[..8])
}

pub(crate) fn run_with_stdin(command: &mut Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn command with piped stdin");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(input.as_bytes())
        .expect("write stdin");
    drop(child.stdin.take());
    child.wait_with_output().expect("wait for command")
}

pub(crate) fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(relative)
}

pub(crate) fn init_vault_sync_pair(
    temp: &TempDir,
) -> (PathBuf, PathBuf, PathBuf) {
    let vault = temp.path().join("vault");
    let remote = temp.path().join("remote.git");
    let peer = temp.path().join("peer");
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
    write_file(&vault.join("initial.md"), "- [ ] initial #task\n");
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    git_in(&vault, ["push", "-q", "-u", "origin", "master"]);
    git(["clone", "-q", path_str(&remote), path_str(&peer)]);
    configure_test_git_identity(&peer);
    (vault, remote, peer)
}

pub(crate) fn vault_sync_command(vault: &Path, temp: &TempDir) -> Command {
    let home = temp.path().join("home");
    let stub_bin = temp.path().join("vault-sync-bin");
    fs::create_dir_all(&home).expect("create vault-sync home");
    fs::create_dir_all(&stub_bin).expect("create vault-sync stub bin");
    write_executable(&stub_bin.join("bob"), "#!/bin/sh\nexit 0\n");

    let mut command = bob_command();
    command
        .arg("vault-sync")
        .env("BOB_DIR", vault)
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .env("PATH", path_with_prefix(&stub_bin))
        .env("XDG_CACHE_HOME", temp.path().join("cache"));
    command
}

pub(crate) fn vault_sync_lock_file(temp: &TempDir) -> PathBuf {
    temp.path().join("vault-sync.lock")
}

pub(crate) fn vault_sync_state_file(temp: &TempDir) -> PathBuf {
    temp.path().join("vault-sync.json")
}

pub(crate) fn git<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("color.status=false")
        .args(args)
        .output()
        .expect("run git");
    assert_success(&output);
    output
}

pub(crate) fn git_in<I, S>(directory: &Path, args: I) -> Output
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

pub(crate) fn path_str(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("path is not UTF-8: {}", path.display()))
}

pub(crate) fn path_with_prefix(prefix: &Path) -> String {
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![prefix.to_path_buf()];
    paths.extend(std::env::split_paths(&current_path));
    std::env::join_paths(paths)
        .expect("join PATH")
        .into_string()
        .expect("PATH is UTF-8")
}

pub(crate) fn highlight_block_ids(contents: &str) -> Vec<String> {
    contents
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix('^')
                .filter(|id| id.starts_with("h-"))
                .map(str::to_string)
        })
        .collect()
}

pub(crate) fn assert_annotation_tasks_in_tasks_section(
    contents: &str,
    task_lines: &[&str],
) {
    let heading = contents
        .find("## Tasks\n")
        .unwrap_or_else(|| panic!("missing ## Tasks heading:\n{contents}"));
    let highlights = contents.find("## Highlights\n").unwrap_or_else(|| {
        panic!("missing ## Highlights heading:\n{contents}")
    });
    assert!(
        heading < highlights,
        "## Tasks should precede ## Highlights:\n{contents}"
    );
    assert_eq!(
        contents.matches("## Tasks").count(),
        1,
        "should not create a duplicate Tasks heading:\n{contents}"
    );
    for line in task_lines {
        let pos = contents
            .find(*line)
            .unwrap_or_else(|| panic!("missing task line {line}:\n{contents}"));
        assert!(
            pos > heading && pos < highlights,
            "task should sit under ## Tasks and before ## Highlights: {line}\n{contents}"
        );
    }
}

pub(crate) fn write_highlights_pdf(path: &Path, marker_contents: &str) {
    write_highlights_pdf_pages(path, &[&[marker_contents]]);
}

pub(crate) fn write_highlights_pdf_pages(
    path: &Path,
    page_text_annotations: &[&[&str]],
) {
    use lopdf::{dictionary, Document, Object, Stream};

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }

    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let mut page_ids = Vec::new();
    for annotations in page_text_annotations {
        let content_id =
            doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let annot_refs = annotations
            .iter()
            .map(|contents| {
                let annot_id = doc.add_object(dictionary! {
                    "Type" => "Annot",
                    "Subtype" => "Text",
                    "Rect" => vec![
                        Object::Integer(0),
                        Object::Integer(0),
                        Object::Integer(24),
                        Object::Integer(24),
                    ],
                    "Contents" => pdf_text_string(contents),
                });
                Object::Reference(annot_id)
            })
            .collect::<Vec<_>>();
        let mut page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ],
            "Contents" => content_id,
        };
        if !annot_refs.is_empty() {
            page.set("Annots", Object::Array(annot_refs));
        }
        let page_id = doc.add_object(page);
        page_ids.push(Object::Reference(page_id));
    }
    doc.set_object(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids,
            "Count" => page_text_annotations.len() as i64,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap_or_else(|error| {
        panic!("write PDF {}: {error}", path.display())
    });
}

pub(crate) fn set_pdf_marker_contents(path: &Path, marker_contents: &str) {
    let mut doc = lopdf::Document::load(path)
        .unwrap_or_else(|error| panic!("load PDF {}: {error}", path.display()));
    let marker_id = first_text_annotation_id(&doc);
    doc.get_object_mut(marker_id)
        .expect("get marker object")
        .as_dict_mut()
        .expect("marker is dictionary")
        .set("Contents", pdf_text_string(marker_contents));
    doc.save(path).unwrap_or_else(|error| {
        panic!("write PDF {}: {error}", path.display())
    });
}

pub(crate) fn pdf_marker_contents(path: &Path) -> String {
    let doc = lopdf::Document::load(path)
        .unwrap_or_else(|error| panic!("load PDF {}: {error}", path.display()));
    let marker_id = first_text_annotation_id(&doc);
    let marker = doc
        .get_dictionary(marker_id)
        .expect("get marker annotation dictionary");
    lopdf::decode_text_string(marker.get(b"Contents").expect("marker contents"))
        .expect("decode marker contents")
}

pub(crate) fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn first_text_annotation_id(
    doc: &lopdf::Document,
) -> lopdf::ObjectId {
    for (_, page_id) in doc.get_pages() {
        let page = doc.get_dictionary(page_id).expect("get page dictionary");
        let annots = page
            .get(b"Annots")
            .expect("page annotations")
            .as_array()
            .expect("annotation array");
        for annot in annots {
            let annot_id = annot.as_reference().expect("annotation reference");
            let annot_dict =
                doc.get_dictionary(annot_id).expect("annotation dictionary");
            if annot_dict
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .is_ok_and(|name| name == b"Text")
            {
                return annot_id;
            }
        }
    }
    panic!("missing /Text annotation");
}

pub(crate) fn pdf_text_string(contents: &str) -> lopdf::Object {
    lopdf::Object::String(
        lopdf::encode_utf16_be(contents),
        lopdf::StringFormat::Hexadecimal,
    )
}

pub(crate) fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create parent {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

pub(crate) fn blocked_tasks_settings_json(extra_custom_status: &str) -> String {
    format!(
        concat!(
            "{{\n",
            "  \"globalFilter\": \"#task\",\n",
            "  \"taskFormat\": \"dataview\",\n",
            "  \"statusSettings\": {{\n",
            "    \"coreStatuses\": [\n",
            "      {{\"symbol\":\" \",\"name\":\"Todo\",\"nextStatusSymbol\":\"x\",\"availableAsCommand\":true,\"type\":\"TODO\"}},\n",
            "      {{\"symbol\":\"x\",\"name\":\"Done\",\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,\"type\":\"DONE\"}}\n",
            "    ],\n",
            "    \"customStatuses\": [\n",
            "      {{\"symbol\":\"/\",\"name\":\"In Progress\",\"nextStatusSymbol\":\"x\",\"availableAsCommand\":true,\"type\":\"IN_PROGRESS\"}},\n",
            "      {{\"symbol\":\"*\",\"name\":\"Next\",\"nextStatusSymbol\":\"x\",\"availableAsCommand\":true,\"type\":\"ON_HOLD\"}},\n",
            "      {{\"symbol\":\"?\",\"name\":\"Blocked\",\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,\"type\":\"ON_HOLD\"}},\n",
            "      {{\"symbol\":\"-\",\"name\":\"Canceled\",\"nextStatusSymbol\":\" \",\"availableAsCommand\":true,\"type\":\"CANCELLED\"}},\n",
            "      {{\"symbol\":\"~\",\"name\":\"Reference\",\"nextStatusSymbol\":\" \",\"availableAsCommand\":false,\"type\":\"NON_TASK\"}}{}\n",
            "    ]\n",
            "  }}\n",
            "}}\n",
        ),
        extra_custom_status
    )
}

pub(crate) fn write_blocked_tasks_settings(vault: &Path) {
    write_file(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        &blocked_tasks_settings_json(""),
    );
}

pub(crate) fn configure_test_git_identity(repo: &Path) {
    git_in(repo, ["config", "user.name", "Test User"]);
    git_in(repo, ["config", "user.email", "test@example.com"]);
    git_in(repo, ["config", "commit.gpgsign", "false"]);
}

/// Materialize an executable stub without ever holding a writable descriptor
/// on the file the tests execute.
///
/// `fs::write` keeps a writable descriptor open while it fills the file, and
/// `cargo test` runs every test in this one process. A child that another test
/// thread forks inside that window inherits the descriptor — `O_CLOEXEC` only
/// drops it once the child reaches `execve` — and until then every attempt to
/// execute the stub fails with `ETXTBSY` ("Text file busy", os error 26), even
/// from an unrelated process such as `bob` spawning its `ob` shim. Writing the
/// payload to a scratch file that is never executed and letting a short-lived
/// `cp` child create the stub keeps the writable descriptor out of this
/// process, so no fork can capture it.
pub(crate) fn write_executable(path: &Path, contents: &str) {
    let payload = scratch_payload_path(path);
    fs::write(&payload, contents).unwrap_or_else(|error| {
        panic!(
            "write executable stub payload {}: {error}",
            payload.display()
        )
    });
    // Copy onto a fresh inode so rewriting a stub cannot disturb a copy that
    // is still executing.
    let _ = fs::remove_file(path);
    let output = Command::new("cp")
        .arg("--")
        .arg(&payload)
        .arg(path)
        .output()
        .unwrap_or_else(|error| {
            panic!("copy executable stub {}: {error}", path.display())
        });
    assert!(
        output.status.success(),
        "copy executable stub {}:\n{}",
        path.display(),
        format_output(&output)
    );
    fs::remove_file(&payload).unwrap_or_else(|error| {
        panic!("remove stub payload {}: {error}", payload.display())
    });
    set_mode(path, 0o755);
}

/// Scratch path for a stub payload: written in this process, never executed,
/// and removed once `cp` has copied it onto the stub path.
pub(crate) fn scratch_payload_path(path: &Path) -> PathBuf {
    let file_name = path.file_name().unwrap_or_else(|| {
        panic!("stub path has no file name: {}", path.display())
    });
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".{unique}.payload"));
    path.with_file_name(name)
}

pub(crate) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
pub(crate) fn assert_unix_mode(path: &Path, expected: u32) {
    let mode = fs::metadata(path)
        .unwrap_or_else(|error| panic!("stat {}: {error}", path.display()))
        .mode()
        & 0o777;
    assert_eq!(mode, expected, "unexpected mode for {}", path.display());
}

#[cfg(not(unix))]
pub(crate) fn assert_unix_mode(_path: &Path, _expected: u32) {}

#[cfg(unix)]
pub(crate) fn set_mode(path: &Path, mode: u32) {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions)
        .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
}

#[cfg(not(unix))]
pub(crate) fn set_mode(_path: &Path, _mode: u32) {}

pub(crate) fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command success:\n{}",
        format_output(output)
    );
}

pub(crate) fn assert_stdout_has_no_ansi(output: &Output) {
    assert!(
        !output.stdout.contains(&0x1b),
        "stdout must not contain ANSI escape codes:\n{}",
        stdout(output)
    );
}

pub(crate) fn assert_text_order(text: &str, needles: &[&str]) {
    let mut last = 0;
    for needle in needles {
        let position = text
            .find(needle)
            .unwrap_or_else(|| panic!("expected `{needle}` in text:\n{text}"));
        assert!(position >= last, "`{needle}` is out of order:\n{text}");
        last = position;
    }
}

pub(crate) fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Run a `bob capture -f json` batch as a dry run and for real against the
/// same vault, then assert both JSON results are identical except for
/// `dry_run`. Returns the real-run JSON. Dry runs stage the same planner
/// state, so batch-level keys (`pomodoro_blocks`, `plan_budget`) must agree.
pub(crate) fn capture_json_dry_run_matches_real(
    vault: &Path,
    day_file: &Path,
    now: &str,
    args: &[&str],
) -> serde_json::Value {
    let run = |dry_run: bool| {
        let mut command = bob_command();
        command
            .arg("capture")
            .arg("-b")
            .arg(vault)
            .arg("-f")
            .arg("json");
        if dry_run {
            command.arg("--dry-run");
        }
        command.arg("--").args(args);
        let output = command
            .env("BOB_DAY_FILE", day_file)
            .env("BOB_NOW", now)
            .output()
            .expect("run capture");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
    };
    let dry: serde_json::Value = run(true);
    assert_eq!(dry["dry_run"], true);
    let real: serde_json::Value = run(false);
    assert_eq!(real["dry_run"], false);
    let mut masked = dry.clone();
    masked["dry_run"] = serde_json::Value::Bool(false);
    if let Some(serde_json::Value::Array(items)) = masked.get_mut("captures") {
        for item in items {
            item["dry_run"] = serde_json::Value::Bool(false);
        }
    }
    assert_eq!(
        masked, real,
        "dry-run JSON must equal real-run JSON except for dry_run"
    );
    real
}

pub(crate) fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub(crate) fn format_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(output),
        stderr(output)
    )
}

pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(crate) fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{}-{}-{}-{}",
            prefix,
            std::process::id(),
            current_time_nanos(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("create temp dir {}: {error}", path.display())
        });
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(error) = remove_dir_all_if_exists(&self.path) {
            eprintln!(
                "failed to remove temp dir {}: {error}",
                self.path.display()
            );
        }
    }
}

pub(crate) fn current_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}

pub(crate) fn remove_dir_all_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Coverage invariant for batch-level `pomodoro_blocks`.
///
/// `original_day` is the day file before the capture, `final_day` after
/// it, and `json` the capture JSON. Runs a Myers line diff of the two
/// snapshots and checks both directions inside the `## Pomodoros`
/// section:
/// - every inserted or changed final line lies within a block listed in
///   `pomodoro_blocks`, located by its final headline `line`;
/// - every deleted pre-state line resurfaces in a listed block, either
///   as a `removed` line or as the `before` text of a `changed` line.
///
/// Two readings mirror the block tracker's own move handling, so entry
/// moves never fail the invariant. A line whose bytes survive unmapped
/// on the other side of the diff moved rather than changed: whole-item
/// starts relocate entries past untouched ones, and created entries
/// shift queued headlines down, and Myers pairs those shifts as
/// delete/insert pairs. Changes outside the section — Work Log inserts
/// and day-file task status flips from closing — are out of scope; other
/// assertions pin them.
///
/// Headline `line` values are 1-based. Removed lines occupy no final
/// line, so a block spans its headline plus one final line per
/// non-`removed` entry, in order. Whitespace-only lines are skipped on
/// both sides: they carry no Pomodoro content and Myers aligns them
/// arbitrarily around moves.
pub(crate) fn assert_pomodoro_blocks_cover_changes(
    original_day: &str,
    final_day: &str,
    json: &serde_json::Value,
) {
    let blocks = json["pomodoro_blocks"].as_array().unwrap_or_else(|| {
        panic!(
            "expected a pomodoro_blocks array:\n{}",
            serde_json::to_string_pretty(json).unwrap_or_default()
        )
    });
    let mut covered: std::collections::BTreeSet<usize> =
        std::collections::BTreeSet::new();
    let mut removed_texts: std::collections::BTreeSet<&str> =
        std::collections::BTreeSet::new();
    let mut before_texts: std::collections::BTreeSet<&str> =
        std::collections::BTreeSet::new();
    for block in blocks {
        let headline = block["line"].as_u64().unwrap_or_else(|| {
            panic!("block headline line must be a number: {block}")
        }) as usize;
        assert!(headline >= 1, "block headline lines are 1-based: {block}");
        // `cursor` is the 1-based final line of the next non-removed row.
        let mut cursor = headline;
        let rows = block["lines"]
            .as_array()
            .unwrap_or_else(|| panic!("block lines must be an array: {block}"));
        for row in rows {
            let text = row["text"].as_str().unwrap_or_else(|| {
                panic!("block line text must be a string: {row}")
            });
            match row["change"].as_str() {
                Some("removed") => {
                    removed_texts.insert(text);
                }
                Some("changed") => {
                    let before = row["before"].as_str().unwrap_or_else(|| {
                        panic!("changed lines carry before: {row}")
                    });
                    before_texts.insert(before);
                    covered.insert(cursor);
                    cursor += 1;
                }
                _ => {
                    covered.insert(cursor);
                    cursor += 1;
                }
            }
        }
    }

    let original_lines: Vec<&str> = original_day.lines().collect();
    let final_lines: Vec<&str> = final_day.lines().collect();
    let original_section = pomodoro_section_range(&original_lines);
    let final_section = pomodoro_section_range(&final_lines);
    let ops = similar::capture_diff_slices(
        similar::Algorithm::Myers,
        &original_lines,
        &final_lines,
    );
    let mut old_mapped = std::collections::BTreeSet::new();
    let mut new_mapped = std::collections::BTreeSet::new();
    for op in &ops {
        if let similar::DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for offset in 0..*len {
                old_mapped.insert(old_index + offset);
                new_mapped.insert(new_index + offset);
            }
        }
    }
    // Bytes that survive unmapped on the other side moved rather than
    // changed, so neither side counts them as new or lost content.
    let pre_unmapped: std::collections::BTreeSet<&str> = original_lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !old_mapped.contains(index))
        .map(|(_, text)| *text)
        .collect();
    let post_unmapped: std::collections::BTreeSet<&str> = final_lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !new_mapped.contains(index))
        .map(|(_, text)| *text)
        .collect();
    let check_old = |index: usize, text: &str, label: &str| {
        if text.trim().is_empty() || !original_section.contains(&index) {
            return;
        }
        if post_unmapped.contains(text) {
            return;
        }
        assert!(
            removed_texts.contains(text) || before_texts.contains(text),
            "{label} line {index} is in no pomodoro block: {text:?}\n\
             removed: {removed_texts:?}\nbefore: {before_texts:?}"
        );
    };
    let check_new = |index: usize, text: &str, label: &str| {
        if text.trim().is_empty() || !final_section.contains(&index) {
            return;
        }
        if pre_unmapped.contains(text) {
            return;
        }
        assert!(
            covered.contains(&(index + 1)),
            "{label} line {} is in no pomodoro block: {text:?}\nblocks: {}",
            index + 1,
            serde_json::to_string_pretty(&json["pomodoro_blocks"])
                .unwrap_or_default(),
        );
    };
    for op in &ops {
        match op {
            similar::DiffOp::Equal { .. } => {}
            similar::DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for offset in 0..*old_len {
                    let index = old_index + offset;
                    check_old(index, original_lines[index], "deleted");
                }
            }
            similar::DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for offset in 0..*new_len {
                    let index = new_index + offset;
                    check_new(index, final_lines[index], "final");
                }
            }
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                for offset in 0..*old_len {
                    let index = old_index + offset;
                    check_old(index, original_lines[index], "replaced-away");
                }
                for offset in 0..*new_len {
                    let index = new_index + offset;
                    check_new(index, final_lines[index], "replacement");
                }
            }
        }
    }
}

/// 0-based range of the `## Pomodoros` section: the heading line through
/// the line before the next heading or the end of the note. Fenced lines
/// never end the section.
fn pomodoro_section_range(lines: &[&str]) -> std::ops::Range<usize> {
    let mut start = None;
    let mut fenced = false;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
        }
        if fenced {
            continue;
        }
        if trimmed == "## Pomodoros" {
            start = Some(index);
        } else if start.is_some() && trimmed.starts_with('#') {
            return start.unwrap_or(index)..index;
        }
    }
    match start {
        Some(heading) => heading..lines.len(),
        None => 0..0,
    }
}
