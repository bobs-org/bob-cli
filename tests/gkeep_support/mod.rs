//! Shared `bob gkeep` integration-test harness: an isolated vault plus
//! config builder and the fake adapter.
//!
//! The fake adapter is an executable shell script speaking adapter
//! protocol v1; pointing `BOB_GKEEP_ADAPTER` at it replaces
//! `uv run --script …`, the same idea as `BOB_CLIPBOARD_CMD`. The
//! `adapter` phase extends this module; other phases put extra helpers
//! in their own test file.

#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(crate) static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");

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
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn current_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos()
}

/// An isolated vault with a `gkeep:` config section.
pub(crate) struct GkeepEnv {
    dir: TempDir,
    vault: PathBuf,
    config: PathBuf,
}

impl GkeepEnv {
    /// Create the vault and a config file with `gkeep.email` set.
    pub(crate) fn new(prefix: &str) -> Self {
        let dir = TempDir::new(prefix);
        let vault = dir.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault dir");
        fs::create_dir_all(dir.path().join("home")).expect("create home dir");
        let config = dir.path().join("config.yml");
        fs::write(&config, "gkeep:\n  email: bryanbugyi34@gmail.com\n")
            .expect("write gkeep config");
        Self { dir, vault, config }
    }

    /// The vault root.
    pub(crate) fn vault(&self) -> &Path {
        &self.vault
    }

    /// The config file path.
    pub(crate) fn config(&self) -> &Path {
        &self.config
    }

    /// Overwrite the config file.
    pub(crate) fn write_config(&self, text: &str) {
        fs::write(&self.config, text).expect("write gkeep config");
    }

    /// A `bob` command isolated to this vault and config, with the
    /// adapter override removed unless a test sets it.
    ///
    /// Hermetic clip defaults (mirroring `tests/cli/support.rs`):
    /// fake DNS for the fetch resolved-address check, a missing
    /// web-clip adapter, and a failing curl stand-in. Tests that clip
    /// override `BOB_HIGHLIGHTS_CURL`/`BOB_WEB_CLIP_ADAPTER` per case;
    /// offline tests (dry runs, `list`) override them with sentinels
    /// that record any invocation.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(BOB_BIN);
        command
            .env("BOB_DIR", self.vault())
            .env("BOB_CONFIG_FILE", self.config())
            .env_remove("BOB_GKEEP_ADAPTER")
            .env(
                "BOB_VAULT_SYNC_LOCK_FILE",
                self.dir.path().join("bob_sync.lock"),
            )
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .env("HOME", self.dir.path().join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("BOB_HIGHLIGHTS_RESOLVE", "*=203.0.113.1")
            .env(
                "BOB_WEB_CLIP_ADAPTER",
                "/definitely/missing/bob-cli-test-web-clip-adapter",
            )
            .env("BOB_HIGHLIGHTS_CURL", "/bin/false");
        command
    }

    /// Write the fake adapter and return its path; the caller opts in
    /// with `.env("BOB_GKEEP_ADAPTER", path)`.
    pub(crate) fn write_fake_adapter(&self) -> PathBuf {
        let path = self.dir.path().join("fake-gkeep-adapter.sh");
        fs::write(
            &path,
            r#"#!/bin/sh
# Fake gkeep adapter: one JSON request on stdin, one JSON response on
# stdout. Answers ping; every other op is a protocol error.
request=$(cat)
case "$request" in
  *'"op":"ping"'*|*'"op": "ping"'*)
    printf '{"ok":true,"protocol":1,"python":"3.12.3","gkeepapi":"0.17.1","gpsoauth":"2.0.0"}'
    ;;
  *)
    printf '{"ok":false,"error":{"kind":"protocol","message":"fake adapter: unsupported op"}}'
    ;;
esac
"#,
        )
        .expect("write fake adapter");
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&path)
                .expect("stat fake adapter")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions)
                .expect("chmod fake adapter");
        }
        path
    }
}

pub(crate) fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Isolate a `git` command from the developer's global/system ignore rules.
pub(crate) fn isolate_git(command: &mut Command) -> &mut Command {
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
}

/// Production-style vault allowlist: ignore everything, then un-ignore
/// notes, directory traversal, and Obsidian JSON.
pub(crate) fn production_allowlist() -> &'static str {
    "# Ignore everything by default.\n\
     *\n\
     \n\
     !.gitignore\n\
     !*/\n\
     !*.md\n\
     !.obsidian/\n\
     !.obsidian/**/*.json\n"
}

/// Initialize a Git vault isolated from global/system Git config.
pub(crate) fn init_git(vault: &Path) {
    git(vault, &["init"]);
    git(vault, &["config", "user.email", "test@example.com"]);
    git(vault, &["config", "user.name", "Test"]);
    git(vault, &["config", "commit.gpgsign", "false"]);
    git(vault, &["config", "core.excludesFile", "/dev/null"]);
}

/// Run `git -C vault <args>` with isolated config.
pub(crate) fn git(vault: &Path, args: &[&str]) -> std::process::Output {
    let output = isolate_git(&mut Command::new("git"))
        .arg("-C")
        .arg(vault)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub(crate) fn git_rev_list_count(vault: &Path) -> usize {
    let out = isolate_git(&mut Command::new("git"))
        .arg("-C")
        .arg(vault)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .expect("git rev-list");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("rev-list count parses")
}

pub(crate) fn git_log_names(vault: &Path) -> Vec<String> {
    let out = isolate_git(&mut Command::new("git"))
        .arg("-C")
        .arg(vault)
        .args(["show", "--name-only", "--format="])
        .output()
        .expect("git show");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect()
}

pub(crate) fn git_check_ignore_quiet(vault: &Path, rel: &str) -> i32 {
    isolate_git(&mut Command::new("git"))
        .arg("-C")
        .arg(vault)
        .args(["check-ignore", "-q", "--", rel])
        .status()
        .expect("git check-ignore")
        .code()
        .unwrap_or(255)
}

/// A fake `BOB_GKEEP_ADAPTER` executable speaking adapter protocol v1.
///
/// For call number N the script saves stdin to `calls/N-<op>.json` and
/// `"$@"` to `calls/N-<op>.argv` (extracting `op` with `sed` from the
/// compact JSON), then prints `responses/<op>.N.json` when it exists and
/// `responses/<op>.json` otherwise. A `responses/<op>.exit` file makes
/// the call exit with that code and no output (an adapter crash); a
/// `responses/<op>.sleep` file sleeps that many seconds first.
pub(crate) struct FakeAdapter {
    root: PathBuf,
    path: PathBuf,
}

impl FakeAdapter {
    /// Write the fake adapter under the test's temp dir.
    pub(crate) fn new(env: &GkeepEnv, name: &str) -> Self {
        let root = env.dir.path().join(name);
        fs::create_dir_all(root.join("calls")).expect("create calls dir");
        fs::create_dir_all(root.join("responses"))
            .expect("create responses dir");
        let path = root.join("fake-adapter.sh");
        fs::write(&path, fake_adapter_script(&root))
            .expect("write fake adapter");
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&path)
                .expect("stat fake adapter")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions)
                .expect("chmod fake adapter");
        }
        Self { root, path }
    }

    /// The executable path to put in `BOB_GKEEP_ADAPTER`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Point a `bob` command at this fake adapter.
    pub(crate) fn install<'a>(
        &self,
        command: &'a mut Command,
    ) -> &'a mut Command {
        command.env("BOB_GKEEP_ADAPTER", self.path())
    }

    /// Serve `body` for every call of `op`.
    pub(crate) fn respond(&self, op: &str, body: &str) {
        fs::write(self.root.join("responses").join(format!("{op}.json")), body)
            .expect("write fake response");
    }

    /// Serve `body` for the Nth call of `op` (1-based).
    pub(crate) fn respond_nth(&self, op: &str, n: usize, body: &str) {
        fs::write(
            self.root.join("responses").join(format!("{op}.{n}.json")),
            body,
        )
        .expect("write fake nth response");
    }

    /// Make `op` exit with `code` and no output (an adapter crash).
    pub(crate) fn set_exit(&self, op: &str, code: i32) {
        fs::write(
            self.root.join("responses").join(format!("{op}.exit")),
            code.to_string(),
        )
        .expect("write fake exit");
    }

    /// Remove a crash planted with [`FakeAdapter::set_exit`].
    pub(crate) fn clear_exit(&self, op: &str) {
        let _ = fs::remove_file(
            self.root.join("responses").join(format!("{op}.exit")),
        );
    }

    /// Make `op` sleep `secs` (fractional ok) before responding.
    pub(crate) fn set_sleep(&self, op: &str, secs: &str) {
        fs::write(
            self.root.join("responses").join(format!("{op}.sleep")),
            secs,
        )
        .expect("write fake sleep");
    }

    /// The stdin the Nth call of `op` received (1-based).
    pub(crate) fn request(&self, op: &str, n: usize) -> String {
        fs::read_to_string(
            self.root.join("calls").join(format!("{n}-{op}.json")),
        )
        .unwrap_or_else(|error| panic!("read {op} call {n} stdin: {error}"))
    }

    /// The argv lines the Nth call of `op` received (1-based).
    pub(crate) fn argv(&self, op: &str, n: usize) -> String {
        fs::read_to_string(
            self.root.join("calls").join(format!("{n}-{op}.argv")),
        )
        .unwrap_or_else(|error| panic!("read {op} call {n} argv: {error}"))
    }

    /// How many adapter calls have been recorded.
    pub(crate) fn call_count(&self) -> usize {
        fs::read_dir(self.root.join("calls"))
            .expect("list calls dir")
            .filter_map(|entry| {
                entry.ok().and_then(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|ext| ext == "json")
                        .then_some(())
                })
            })
            .count()
    }
}

fn fake_adapter_script(root: &Path) -> String {
    let quoted = root.to_string_lossy().replace('\'', "'\\''");
    format!(
        "#!/bin/sh\n\
         # Fake gkeep adapter: one JSON request on stdin, one JSON response\n\
         # on stdout. See FakeAdapter for the calls/ and responses/ layout.\n\
         ROOT='{quoted}'\n\
         mkdir -p \"$ROOT/calls\"\n\
         next=\"$ROOT/next\"\n\
         n=1\n\
         [ -f \"$next\" ] && n=$(cat \"$next\")\n\
         echo $((n + 1)) > \"$next\"\n\
         request=$(cat)\n\
         op=$(printf '%s' \"$request\" | sed -n 's/.*\"op\":\"\\([a-z_]*\\)\".*/\\1/p')\n\
         [ -z \"$op\" ] && op=unknown\n\
         printf '%s' \"$request\" > \"$ROOT/calls/$n-$op.json\"\n\
         if [ $# -gt 0 ]; then\n\
         printf '%s\\n' \"$@\" > \"$ROOT/calls/$n-$op.argv\"\n\
         else\n\
         : > \"$ROOT/calls/$n-$op.argv\"\n\
         fi\n\
         [ -f \"$ROOT/responses/$op.sleep\" ] && sleep \"$(cat \"$ROOT/responses/$op.sleep\")\"\n\
         [ -f \"$ROOT/responses/$op.exit\" ] && exit \"$(cat \"$ROOT/responses/$op.exit\")\"\n\
         response=\"$ROOT/responses/$op.$n.json\"\n\
         [ -f \"$response\" ] || response=\"$ROOT/responses/$op.json\"\n\
         if [ -f \"$response\" ]; then cat \"$response\"; exit 0; fi\n\
         printf '{{\"ok\":false,\"error\":{{\"kind\":\"protocol\",\
         \"message\":\"fake adapter: no response for op %s\"}}}}' \"$op\"\n"
    )
}

/// A typed `KeepNote` fixture builder:
///
/// ```text
/// note("Call dentist").text("…").created("2026-09-27T21:14:03Z")
/// ```
pub(crate) struct NoteBuilder {
    id: String,
    kind: &'static str,
    title: String,
    text: String,
    items: Vec<(String, bool, bool)>,
    pinned: bool,
    archived: bool,
    shared: bool,
    labels: Vec<String>,
    attachments: Vec<(String, Option<String>)>,
    links: Vec<(String, String)>,
    created: String,
    edited: Option<String>,
    url: Option<String>,
}

/// Start a note fixture with `title` and id `note-1`.
pub(crate) fn note(title: &str) -> NoteBuilder {
    NoteBuilder {
        id: "note-1".to_string(),
        kind: "note",
        title: title.to_string(),
        text: String::new(),
        items: Vec::new(),
        pinned: false,
        archived: false,
        shared: false,
        labels: Vec::new(),
        attachments: Vec::new(),
        links: Vec::new(),
        created: "2026-09-27T21:14:03Z".to_string(),
        edited: None,
        url: None,
    }
}

impl NoteBuilder {
    /// Set the Keep id.
    pub(crate) fn id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    /// Set the free-text body.
    pub(crate) fn text(mut self, text: &str) -> Self {
        self.text = text.to_string();
        self
    }

    /// Set the Keep `created` timestamp.
    pub(crate) fn created(mut self, timestamp: &str) -> Self {
        self.created = timestamp.to_string();
        self
    }

    /// Set the Keep `edited` timestamp (defaults to `created`).
    pub(crate) fn edited(mut self, timestamp: &str) -> Self {
        self.edited = Some(timestamp.to_string());
        self
    }

    /// Mark the note pinned.
    pub(crate) fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    /// Mark the note archived.
    pub(crate) fn archived(mut self) -> Self {
        self.archived = true;
        self
    }

    /// Mark the note shared with collaborators.
    pub(crate) fn shared(mut self) -> Self {
        self.shared = true;
        self
    }

    /// Add a Keep label.
    pub(crate) fn label(mut self, label: &str) -> Self {
        self.labels.push(label.to_string());
        self
    }

    /// Turn the note into a list with `(text, checked, indented)` items.
    pub(crate) fn list(mut self, items: Vec<(&str, bool, bool)>) -> Self {
        self.kind = "list";
        self.items = items
            .into_iter()
            .map(|(text, checked, indented)| {
                (text.to_string(), checked, indented)
            })
            .collect();
        self
    }

    /// Add an attachment of `kind` (`image`, `drawing`, or `audio`) with
    /// optional OCR text.
    pub(crate) fn attachment(mut self, kind: &str, ocr: Option<&str>) -> Self {
        self.attachments
            .push((kind.to_string(), ocr.map(str::to_string)));
        self
    }

    /// Set the Keep url.
    pub(crate) fn url(mut self, url: &str) -> Self {
        self.url = Some(url.to_string());
        self
    }

    /// Add a shared-link preview (`note.annotations.links` entry).
    pub(crate) fn link(mut self, url: &str, title: &str) -> Self {
        self.links.push((url.to_string(), title.to_string()));
        self
    }

    /// The `KeepContent` object for this note.
    pub(crate) fn content_json(&self) -> serde_json::Value {
        serde_json::json!({
            "title": self.title,
            "text": self.text,
            "items": self.items.iter().map(|(text, checked, indented)| {
                serde_json::json!({
                    "text": text,
                    "checked": checked,
                    "indented": indented,
                })
            }).collect::<Vec<_>>(),
        })
    }

    /// Build the full `KeepNote` JSON object.
    pub(crate) fn build(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "kind": self.kind,
            "content": self.content_json(),
            "pinned": self.pinned,
            "archived": self.archived,
            "shared": self.shared,
            "labels": self.labels,
            "attachments": self.attachments.iter().map(|(kind, ocr)| {
                serde_json::json!({"kind": kind, "extracted_text": ocr})
            }).collect::<Vec<_>>(),
            "links": self.links.iter().map(|(url, title)| {
                serde_json::json!({"url": url, "title": title})
            }).collect::<Vec<_>>(),
            "created": self.created,
            "edited": self.edited.clone().unwrap_or_else(|| self.created.clone()),
            "url": self.url,
        })
    }
}

/// A `ping` success response.
pub(crate) fn ping_ok() -> String {
    serde_json::json!({
        "ok": true,
        "protocol": 1,
        "python": "3.12.3",
        "gkeepapi": "0.17.1",
        "gpsoauth": "2.0.0",
    })
    .to_string()
}

/// A `snapshot` success response for `account` with `notes`.
pub(crate) fn snapshot_ok(
    account: &str,
    notes: Vec<serde_json::Value>,
) -> String {
    serde_json::json!({
        "ok": true,
        "account": account,
        "notes": notes,
    })
    .to_string()
}

/// An `archive` success response from `(id, status)` pairs.
pub(crate) fn archive_ok(results: Vec<(&str, &str)>) -> String {
    serde_json::json!({
        "ok": true,
        "results": results.iter().map(|(id, status)| {
            serde_json::json!({"id": id, "status": status})
        }).collect::<Vec<_>>(),
    })
    .to_string()
}

/// An `exchange` success response carrying `token`.
pub(crate) fn exchange_ok(token: &str) -> String {
    serde_json::json!({
        "ok": true,
        "master_token": token,
    })
    .to_string()
}

/// An `ok:false` response with `kind` and `message`.
pub(crate) fn error_response(kind: &str, message: &str) -> String {
    serde_json::json!({
        "ok": false,
        "error": {"kind": kind, "message": message},
    })
    .to_string()
}
