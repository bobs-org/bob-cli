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
use std::time::Duration;
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
pub(crate) const BOB_COMPLETION_TEST_DEADLINE_MS: &str = "10000";

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
    // A test that supplies only -b must never fall back to the real vault.
    // Online writers must explicitly select the same fixture in BOB_DIR.
    command.env("BOB_DIR", isolation.join("unused-vault"));
    command.env("BOB_VAULT_SYNC_LOCK_FILE", isolation.join("bob_sync.lock"));
    command.env("BOB_VAULT_SYNC_NOTIFY", "0");
    command.env("XDG_STATE_HOME", isolation.join("state"));
    command.env("XDG_CACHE_HOME", isolation.join("cache"));
    command
}

/// Return missing packages from `bob highlights doctor`, or an error when
/// the package check itself cannot be trusted.
pub(crate) fn note_profile_package_skip(
    vault: &Path,
) -> Result<Option<String>, String> {
    let output = bob_command()
        .arg("highlights")
        .arg("doctor")
        .arg("--no-hooks")
        .env("BOB_DIR", vault)
        .output()
        .map_err(|error| format!("run highlights doctor: {error}"))?;
    let report = stdout(&output);
    note_profile_package_skip_report(&report)
}

fn note_profile_package_skip_report(
    report: &str,
) -> Result<Option<String>, String> {
    let Some(row) = report
        .lines()
        .find(|line| line.starts_with("latex_packages: "))
    else {
        return Err(format!("doctor omitted latex_packages row:\n{report}"));
    };
    if let Some(missing) = row
        .strip_prefix("latex_packages: warn (missing ")
        .and_then(|row| row.strip_suffix(')'))
    {
        return Ok(Some(missing.to_string()));
    }
    if row.contains("kpsewhich not found") || row.contains("kpsewhich failed") {
        return Err(row.to_string());
    }
    Ok(None)
}

pub(crate) fn skip_if_missing_note_profile_packages(
    vault: &Path,
    test_name: &str,
) -> bool {
    match note_profile_package_skip(vault) {
        Ok(Some(missing)) => {
            eprintln!("skipping {test_name}: missing TeX packages: {missing}");
            true
        }
        Ok(None) => false,
        Err(reason) => {
            eprintln!("skipping {test_name}: {reason}");
            true
        }
    }
}

#[cfg(test)]
mod note_profile_package_skip_tests {
    use super::note_profile_package_skip_report;

    #[test]
    fn note_profile_package_skip_reports_missing_packages() {
        assert_eq!(
            note_profile_package_skip_report(
                "latex_packages: warn (missing titlesec.sty, needspace.sty)\n"
            ),
            Ok(Some("titlesec.sty, needspace.sty".to_string()))
        );
    }
}

/// Build a `bob` command for shell completion tests with a deadline that
/// tolerates machine load without changing the production default.
pub(crate) fn bob_completion_command() -> Command {
    let mut command = bob_command();
    command.env("BOB_COMPLETE_DEADLINE_MS", BOB_COMPLETION_TEST_DEADLINE_MS);
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

pub(crate) fn write_unblocked_project_with_future_resident(
    vault: &Path,
    day_file: &Path,
) {
    write_toggle_task_settings(vault);
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "status: wip\n",
            "---\n\n",
            "- [ ] #task Ship project #prj #hide ^prj\n",
            "- [?] #task Scheduled resident [scheduled:: 2026-10-20] ^resident\n",
        ),
    );
    write_file(day_file, "## Pomodoros\n- [ ] () — CURRENT\n  - context\n");
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

/// A wrong-vault write must refuse before locks, sync state, or Git changes.
pub(crate) fn assert_vault_override_refused(
    command: &mut Command,
    selected: &Path,
) {
    let temp = TempDir::new("bob-cli-wrong-vault");
    let (default, remote, _) = init_vault_sync_pair(&temp);
    write_file(&default.join("pending.md"), "must remain uncommitted\n");
    let heads = [&default, selected, &remote]
        .map(|path| stdout(&git_in(path, ["rev-parse", "HEAD"])));
    let status = [&default, selected]
        .map(|path| stdout(&git_in(path, ["status", "--porcelain"])));
    fn snapshot(path: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut files = std::collections::BTreeMap::new();
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                files.extend(snapshot(&path));
            } else {
                files.insert(path.clone(), fs::read(path).unwrap());
            }
        }
        files
    }
    let before = [snapshot(&default), snapshot(selected)];
    let lock = temp.path().join("sync.lock");
    let state = temp.path().join("sync.json");
    let output = command
        .args(["-b", path_str(selected), "-f", "json"])
        .env("BOB_DIR", &default)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("BOB_VAULT_SYNC_STATE_FILE", &state)
        .output()
        .expect("run wrong-vault write");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).unwrap();
    // Reference commands nest failures; vocabulary renames use top-level fields.
    let error = json.get("error").unwrap_or(&json);
    assert_eq!(error["code"], "vault_mismatch", "{json}");
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("differs from BOB_DIR"));
    assert!(!lock.exists(), "refusal must precede lock acquisition");
    assert!(!state.exists(), "refusal must precede sync state writes");
    assert_eq!(before, [snapshot(&default), snapshot(selected)]);
    assert_eq!(
        heads,
        [&default, selected, &remote]
            .map(|path| { stdout(&git_in(path, ["rev-parse", "HEAD"])) })
    );
    assert_eq!(
        status,
        [&default, selected]
            .map(|path| { stdout(&git_in(path, ["status", "--porcelain"])) })
    );
}

pub(crate) fn vault_sync_command(vault: &Path, temp: &TempDir) -> Command {
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("create vault-sync home");

    let mut command = bob_command();
    command
        .arg("vault-sync")
        .env("BOB_DIR", vault)
        .env("BOB_VAULT_SYNC_LOCK_FILE", vault_sync_lock_file(temp))
        .env("BOB_VAULT_SYNC_STATE_FILE", vault_sync_state_file(temp))
        .env("BOB_VAULT_SYNC_NOTIFY", "0")
        .env("HOME", home)
        .env("NO_COLOR", "1")
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

/// Set `path`'s mtime to now plus `offset_secs` (negative = past).
///
/// Uses `File::set_modified` so tests do not depend on GNU `touch -d`.
pub(crate) fn touch_mtime_offset(path: &Path, offset_secs: i64) {
    let now = SystemTime::now();
    let target = if offset_secs >= 0 {
        now + Duration::from_secs(offset_secs as u64)
    } else {
        now - Duration::from_secs(offset_secs.unsigned_abs())
    };
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(target))
        .unwrap_or_else(|error| {
            panic!("set mtime of {}: {error}", path.display())
        });
}

/// Run `command` and return `Err` if it exceeds `timeout_secs`.
///
/// GNU `timeout(1)` is not on default macOS PATH; zsh zpty drivers hang
/// the job without a deadline. Callers skip rather than fail when the
/// host cannot drive zpty.
pub(crate) fn try_output_with_timeout(
    mut command: Command,
    timeout_secs: u64,
) -> Result<Output, String> {
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command
        .spawn()
        .map_err(|error| format!("spawn timed command: {error}"))?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(result) => {
            result.map_err(|error| format!("wait timed command: {error}"))
        }
        Err(_) => {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = rx.recv_timeout(Duration::from_secs(5));
            Err(format!(
                "command timed out after {timeout_secs}s (pid {pid})"
            ))
        }
    }
}

/// True when `left` and `right` are the same path, including `/var` vs
/// `/private/var` on macOS.
pub(crate) fn paths_match(
    left: impl AsRef<Path>,
    right: impl AsRef<Path>,
) -> bool {
    let left = left.as_ref();
    let right = right.as_ref();
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(first), Ok(second)) => first == second,
        _ => false,
    }
}

/// Seed a minimal legacy (v1) reference note so the first sync updates it
/// instead of taking the v2 birth path: new births file a reading task in
/// the parent note and render a managed embed, while an in-note `^ref`
/// tracker keeps the legacy behavior these fixtures assert. `task_line` is
/// the exact generated line a v1 birth would have rendered (mark matching
/// the marker status, e.g. `- [/] #task #ref [[lib/example.pdf]] #hide
/// ^ref` for `wip`).
pub(crate) fn seed_v1_ref_note(note: &Path, task_line: &str) {
    write_file(
        note,
        &format!(
            "{task_line}\n\n## Highlights\n\n<!-- highlights:begin -->\n\n<!-- highlights:end -->\n"
        ),
    );
}

/// Seed a minimal area note so a marker `parent` hint resolves to a real
/// residence instead of the `mac_inbox` fallback.
pub(crate) fn write_area_note(note: &Path, title: &str) {
    write_file(
        note,
        &format!("---\ntype: \"[[area]]\"\n---\n\n# {title}\n"),
    );
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
    git_in(repo, ["config", "core.precomposeunicode", "true"]);
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

/// Change the mode of `path` without following a final symlink.
///
/// `fs::set_permissions` follows the last component, so a tree that
/// contains a symlink would otherwise chmod the target. Directories
/// are opened with `O_NOFOLLOW` and `fchmod`'d through that fd.
#[cfg(unix)]
pub(crate) fn set_mode_nofollow(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let c_path =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains interior NUL",
            )
        })?;
    // SAFETY: `c_path` is a valid C string. `O_NOFOLLOW` refuses a
    // final symlink, so the fd refers to `path` itself. The fd is
    // closed before this function returns.
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::fchmod(fd, mode as libc::mode_t) };
    let chmod_error = io::Error::last_os_error();
    unsafe { libc::close(fd) };
    if rc == 0 {
        Ok(())
    } else {
        Err(chmod_error)
    }
}

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
    capture_json_dry_run_matches_real_flags(vault, day_file, now, &[], args)
}

/// Like [`capture_json_dry_run_matches_real`], with extra clap flags
/// inserted before `--` so `-p/--pull-in` is not swallowed as TEXT.
pub(crate) fn capture_json_dry_run_matches_real_flags(
    vault: &Path,
    day_file: &Path,
    now: &str,
    flags: &[&str],
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
        command.args(flags).arg("--").args(args);
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
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            repair_owner_write_in_tree(path);
            match fs::remove_dir_all(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

/// Add owner write and execute on real directories inside `root`.
///
/// Walks with `symlink_metadata` and never follows a symlink, so a
/// fixture that points at a foreign tree cannot chmod that target.
/// Directories owned by another uid are left alone.
#[cfg(unix)]
fn repair_owner_write_in_tree(root: &Path) {
    // SAFETY: `geteuid` is always safe to call.
    let euid = unsafe { libc::geteuid() };
    repair_owner_write_walk(root, euid);
}

#[cfg(unix)]
fn repair_owner_write_walk(path: &Path, euid: u32) {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(_) => return,
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return;
    }
    if meta.uid() == euid {
        let mode = meta.mode() & 0o777;
        if mode & 0o300 != 0o300 {
            let _ = set_mode_nofollow(path, mode | 0o300);
        }
    }
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            repair_owner_write_walk(&entry.path(), euid);
        }
    }
}

#[cfg(not(unix))]
fn repair_owner_write_in_tree(_root: &Path) {}

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

#[cfg(unix)]
#[test]
fn tempdir_drop_removes_write_locked_tree() {
    let temp = TempDir::new("bob-cli-tempdir-locked");
    let path = temp.path().to_path_buf();
    let nested = path.join("vault").join("nested");
    fs::create_dir_all(&nested).expect("nested vault dir");
    write_file(&nested.join("file.md"), "x\n");
    set_mode(&nested.join("file.md"), 0o444);
    set_mode(&nested, 0o555);
    set_mode(&path.join("vault"), 0o555);
    set_mode(&path, 0o555);
    drop(temp);
    assert!(!path.exists(), "locked temp dir leaked: {}", path.display());
}

#[cfg(unix)]
#[test]
fn tempdir_drop_does_not_chmod_symlink_target() {
    let temp = TempDir::new("bob-cli-tempdir-symlink");
    let outside = TempDir::new("bob-cli-tempdir-outside");
    let path = temp.path().to_path_buf();
    let outside_dir = outside.path().join("foreign");
    fs::create_dir_all(&outside_dir).expect("foreign dir");
    write_file(&outside_dir.join("secret.md"), "keep\n");
    std::os::unix::fs::symlink(&outside_dir, path.join("link"))
        .expect("symlink into foreign dir");
    set_mode(&outside_dir, 0o555);
    set_mode(&path, 0o555);
    drop(temp);
    assert!(!path.exists(), "temp dir leaked: {}", path.display());
    assert_unix_mode(&outside_dir, 0o555);
    set_mode(&outside_dir, 0o755);
}
