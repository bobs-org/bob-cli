//! Fully detached worker kick.
//!
//! After a successful capture commit, `kick()` spawns
//! `bob ref jobs run -q` in a new session with stdin from
//! `/dev/null` and stdout/stderr appended to `worker.log`. It never
//! waits and never inherits the caller's pipes, so neither a Mac
//! app's 20 s lane timeout nor a terminal capture holds the worker
//! open. `BOB_REF_JOBS_KICK=off|0|false` disables it (the CLI test
//! harness sets it to `off`).

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Env var that disables the kick when `off`, `0`, or `false`.
pub(crate) const ENV_KICK_DISABLE: &str = "BOB_REF_JOBS_KICK";

/// Whether `value` disables the kick.
pub(crate) fn kick_disabled_value(value: &str) -> bool {
    matches!(value, "off" | "0" | "false")
}

/// Whether the kick is disabled in this process.
pub(crate) fn kick_disabled() -> bool {
    std::env::var(ENV_KICK_DISABLE)
        .map(|value| kick_disabled_value(&value))
        .unwrap_or(false)
}

/// Kick the worker with this executable. A disabled kick is a silent
/// no-op; a spawn failure returns an error the caller prints as a
/// warning without changing the exit code.
pub(crate) fn kick() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("find bob executable: {error}"))?;
    kick_with(&exe)
}

/// Same as [`kick`], but with an injectable executable for tests.
/// Honors [`ENV_KICK_DISABLE`] like [`kick`].
pub(crate) fn kick_with(exe: &Path) -> Result<(), String> {
    if kick_disabled() {
        return Ok(());
    }
    spawn_detached(exe, &worker_log_path()?)
}

fn worker_log_path() -> Result<PathBuf, String> {
    let root = super::spool::jobs_dir();
    fs::create_dir_all(&root).map_err(|error| {
        format!("create spool directory {}: {error}", root.display())
    })?;
    Ok(root.join("worker.log"))
}

/// Spawn `exe ref jobs run -q` detached: new session and process
/// group, stdin from `/dev/null`, stdout and stderr appended to the
/// worker log. Returns once the child spawns; never waits.
fn spawn_detached(exe: &Path, log: &Path) -> Result<(), String> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let out = options.open(log).map_err(|error| {
        format!("open worker log {}: {error}", log.display())
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(log, fs::Permissions::from_mode(0o600));
    }
    let err = out.try_clone().map_err(|error| {
        format!("clone worker log {}: {error}", log.display())
    })?;
    let mut command = Command::new(exe);
    command
        .args(["ref", "jobs", "run", "-q"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New session, no controlling terminal (following
        // `completion/verify.rs`): the worker must not hold any pipe
        // the caller (or a Mac lane) waits on. Note this is setsid
        // *without* `process_group(0)`: a group leader cannot call
        // setsid, so the combination would silently stay put.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    command.spawn().map_err(|error| {
        format!("spawn clip worker {}: {error}", exe.display())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Serialize the tests that mutate process env: nothing else in
    /// this suite touches `XDG_STATE_HOME` in-process, and the child
    /// inherits whatever we set here.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Run `f` with `vars` set and `unset` removed, restoring
    /// everything before returning. The lock serializes our own spawn
    /// sections; restoration is immediate — the kicked child already
    /// copied its env at fork — so parallel tests never observe our
    /// values while we poll for results _without_ env held.
    fn with_spawn_env<F, T>(
        vars: Vec<(&'static str, std::ffi::OsString)>,
        unset: &[&'static str],
        f: F,
    ) -> T
    where
        F: FnOnce() -> T,
    {
        let _lock = env_lock().lock().expect("env lock");
        let mut saved = Vec::new();
        for (key, _) in &vars {
            saved.push((*key, std::env::var_os(key)));
        }
        for key in unset {
            saved.push((*key, std::env::var_os(key)));
        }
        for (key, value) in &vars {
            unsafe {
                std::env::set_var(key, value);
            }
        }
        for key in unset {
            unsafe {
                std::env::remove_var(key);
            }
        }
        let out = f();
        for (key, old) in saved {
            unsafe {
                match old {
                    Some(old) => std::env::set_var(key, old),
                    None => std::env::remove_var(key),
                }
            }
        }
        out
    }

    #[test]
    fn kick_disable_values() {
        assert!(kick_disabled_value("off"));
        assert!(kick_disabled_value("0"));
        assert!(kick_disabled_value("false"));
        assert!(!kick_disabled_value(""));
        assert!(!kick_disabled_value("1"));
        assert!(!kick_disabled_value("on"));
        assert!(!kick_disabled_value("OFF"));
    }

    /// Resolve the real `bob` binary by asking cargo: this stays
    /// correct whatever target dir the harness assigns, and is a
    /// fast no-op once the outer `cargo test` has built the bins.
    fn bob_binary() -> PathBuf {
        let cargo =
            std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let output = std::process::Command::new(cargo)
            .args(["build", "--bin", "bob", "--message-format=json"])
            .output()
            .expect("cargo build --bin bob");
        assert!(
            output.status.success(),
            "cargo build --bin bob failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let artifact: serde_json::Value =
                serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
            let is_bob = artifact
                .pointer("/target/name")
                .and_then(|name| name.as_str())
                == Some("bob");
            if is_bob
                && let Some(exe) =
                    artifact.get("executable").and_then(|exe| exe.as_str())
            {
                return PathBuf::from(exe);
            }
        }
        panic!("cargo never reported the bob executable");
    }

    fn write_executable(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("script parent");
        }
        fs::write(path, contents).expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .expect("chmod script");
        }
    }

    #[test]
    fn kicked_child_leaves_session_and_stdio() {
        let root = std::env::temp_dir()
            .join(format!("bob-cli-ref-jobs-kick-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let state = root.join("state");

        // Fake executable: record its session id and stdio situation.
        let probe = root.join("probe");
        let script = probe.join("fake-bob.sh");
        let session_file = probe.join("session");
        write_executable(
            &script,
            &format!(
                "#!/bin/sh\nps -o sess= -p $$ | tr -d ' ' > {}\nif [ -t 0 ]; then echo BAD-TTY-STDIN; else echo GOOD-NO-TTY; fi\necho PROBE-OUT\necho PROBE-ERR >&2\n",
                session_file.display()
            ),
        );
        // Spawn with the kick env, then restore immediately: the child
        // already copied its env at fork, and later polling must not
        // leak our values into parallel tests.
        with_spawn_env(
            vec![("XDG_STATE_HOME", state.clone().into_os_string())],
            &[ENV_KICK_DISABLE],
            || kick_with(&script).expect("kick fake executable"),
        );

        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if session_file.is_file() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "kicked child never ran"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let child_session =
            fs::read_to_string(&session_file).expect("read session");
        let own_session = unsafe { libc::getsid(0) }.to_string();
        assert_ne!(
            child_session.trim(),
            own_session.trim(),
            "kicked child must run in a new session"
        );
        // The session file lands before stdio flushes: poll for
        // every marker so a loaded machine cannot fail the read.
        let log_path = state.join("bob-cli/ref/jobs/worker.log");
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(15);
        let log = loop {
            let log = fs::read_to_string(&log_path).unwrap_or_default();
            if log.contains("PROBE-OUT")
                && log.contains("PROBE-ERR")
                && log.contains("GOOD-NO-TTY")
            {
                break log;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker log never filled: {log}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        assert!(log.contains("PROBE-OUT"), "{log}");
        assert!(log.contains("PROBE-ERR"), "{log}");
        assert!(log.contains("GOOD-NO-TTY"), "{log}");
    }

    #[test]
    fn kicked_worker_drains_a_seeded_job() {
        let root = std::env::temp_dir().join(format!(
            "bob-cli-ref-jobs-kick-drain-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let state = root.join("state");
        let vault = root.join("vault");
        fs::create_dir_all(&vault).expect("create vault");

        // Fake curl: every fetch is a tiny article page.
        let curl = root.join("fake-curl.sh");
        write_executable(
            &curl,
            "#!/bin/sh\ndest=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then dest=\"$arg\"; fi\n  prev=\"$arg\"\n  url=\"$arg\"\ndone\nprintf '<html><body>article</body></html>' > \"$dest\"\nprintf '200\\ntext/html; charset=utf-8\\n\\n'\n",
        );
        let adapter = root.join("fake-adapter.sh");
        write_executable(
            &adapter,
            &format!(
                "#!/bin/sh\nrequest=$(cat)\nout_pdf=$(printf '%s' \"$request\" | sed -n 's/.*\"out_pdf\":\"\\([^\"]*\\)\".*/\\1/p')\ncp {root}/fixture.pdf \"$out_pdf\"\nprintf '%s' '{CAPTURE}'\n",
                root = root.display(),
                CAPTURE = r#"{"protocol":1,"ok":true,"op":"capture","kind":"article","final_url":"https://example.com/post","title":"Seeded","author":"","published":"","site":"Example","description":"","metadata_sources":{"title":"og:title","author":"","published":"","site":""},"word_count":10,"capture":{"browser":"none","browser_version":"","mode":"headless","retried_after_challenge":false},"fidelity":{"status":"ok","page_large_media":0,"kept_large_media":0,"page_code_blocks":0,"kept_code_blocks":0,"page_words":10,"kept_words":10},"images":{"total":0,"kept":0,"skipped_small":0,"failed":0},"pdf_bytes":100,"warnings":[]}"#,
            ),
        );
        // Minimal one-page PDF for the fake adapter to copy.
        write_fixture_pdf(&root.join("fixture.pdf"));

        // Seed one pending job straight into the spool.
        let jobs = state.join("bob-cli/ref/jobs");
        let created_at = chrono::Local::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let job = serde_json::json!({
            "schema_version": 1,
            "id": "20261007T143012-abcdef",
            "created_at": created_at,
            "source": "capture",
            "bob_dir": vault,
            "url": "https://example.com/post",
            "cleaned_url": "https://example.com/post",
            "dedupe_key": "https://example.com/post",
            "display": "example.com/post",
            "route_hint": "article",
            "attempts": 0,
            "fallback": {
                "relative_target": "mac_inbox.md",
                "task_line": "- [ ] #task https://example.com/post [created::2026-10-07]"
            }
        });
        fs::create_dir_all(jobs.join("pending")).expect("pending dir");
        fs::write(
            jobs.join("pending/20261007T143012-abcdef.json"),
            serde_json::to_vec_pretty(&job).expect("encode job"),
        )
        .expect("seed job");

        let bob = bob_binary();
        assert!(bob.is_file(), "bob binary at {}", bob.display());
        // Spawn with the worker env, then restore immediately: the
        // child already copied its env at fork, and the long poll
        // below must not leak our values into parallel tests.
        with_spawn_env(
            vec![
                ("XDG_STATE_HOME", state.into_os_string()),
                ("BOB_HIGHLIGHTS_CURL", curl.into_os_string()),
                ("BOB_WEB_CLIP_ADAPTER", adapter.into_os_string()),
                (
                    "BOB_HIGHLIGHTS_RESOLVE",
                    std::ffi::OsString::from("*=203.0.113.1"),
                ),
                (
                    "BOB_CONFIG_FILE",
                    std::ffi::OsString::from(
                        "/definitely/missing/bob-cli-test-config.yml",
                    ),
                ),
                (
                    "BOB_VAULT_SYNC_LOCK_FILE",
                    root.join("bob_sync.lock").into_os_string(),
                ),
            ],
            &[ENV_KICK_DISABLE],
            || kick_with(&bob).expect("kick real worker"),
        );

        let done = jobs.join("done.jsonl");
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            if let Ok(contents) = fs::read_to_string(&done)
                && contents.contains("20261007T143012-abcdef")
            {
                assert!(
                    contents.contains("\"outcome\":\"created\""),
                    "{contents}"
                );
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "kicked worker never drained the seeded job"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            !jobs.join("pending/20261007T143012-abcdef.json").exists(),
            "pending job file is gone"
        );
        // `post` is a skipped URL segment, so create's stem rule
        // falls back to the adapter title (`seeded`).
        assert!(
            vault.join("xlib/blogs/seeded.pdf").is_file(),
            "intake PDF was clipped"
        );
    }

    /// Minimal one-page PDF, matching the test-suite bare fixture.
    fn write_fixture_pdf(path: &Path) {
        use lopdf::{dictionary, Document, Stream};
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("pdf parent");
        }
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let content_id =
            doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                lopdf::Object::Integer(0),
                lopdf::Object::Integer(0),
                lopdf::Object::Integer(612),
                lopdf::Object::Integer(792),
            ],
            "Contents" => content_id,
        });
        doc.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![lopdf::Object::Reference(page_id)],
                "Count" => 1,
            },
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.save(path).expect("write fixture pdf");
    }
}
