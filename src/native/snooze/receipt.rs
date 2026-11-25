//! Post-commit wake receipt and Darwin `org.bobs.bob.snooze.woken` post.
//!
//! Written only after a successful vault commit with a nonempty `woken[]`.
//! The Darwin notification is posted only once the matching receipt is
//! readable. Failures warn on stderr and never roll back the wake.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::native::{env as bob_env, pref_journal};

/// Darwin notification posted after a readable wake receipt, on macOS.
pub(crate) const WOKEN_NOTIFICATION: &str = "org.bobs.bob.snooze.woken";

const SCHEMA_VERSION: u32 = 1;

#[cfg(test)]
thread_local! {
    static TEST_DARWIN: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static TEST_DARWIN_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WakeReceipt {
    pub schema_version: u32,
    pub wake_id: String,
    pub date: String,
    pub woken_at: String,
    pub host: String,
    pub daily_file: String,
    pub tasks: Vec<Value>,
    pub plan: Value,
}

/// `$XDG_STATE_HOME/bob-cli/snooze`, else `~/.local/state/bob-cli/snooze`.
pub(crate) fn snooze_state_dir() -> PathBuf {
    bob_env::bob_cli_state_dir().join("snooze")
}

pub(crate) fn last_wake_path() -> PathBuf {
    snooze_state_dir().join("last-wake.json")
}

pub(crate) fn notifier_path() -> PathBuf {
    snooze_state_dir().join("notifier.json")
}

pub(crate) fn new_wake_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let nonce = RECEIPT_NONCE.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{nonce:x}-{}", process::id())
}

static RECEIPT_NONCE: AtomicU64 = AtomicU64::new(0);

/// Build the durable receipt from a committed wake report.
pub(crate) fn from_report(
    report: &Value,
    daily_file: &Path,
    woken_at: &str,
) -> Option<WakeReceipt> {
    let woken = report.get("woken")?.as_array()?;
    if woken.is_empty() {
        return None;
    }
    let tasks = woken.iter().map(receipt_task).collect::<Vec<_>>();
    Some(WakeReceipt {
        schema_version: SCHEMA_VERSION,
        wake_id: new_wake_id(),
        date: report
            .get("date")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        woken_at: woken_at.to_string(),
        host: pref_journal::host_label(),
        daily_file: daily_file.to_string_lossy().into_owned(),
        tasks,
        plan: report.get("plan").cloned().unwrap_or(json!({})),
    })
}

fn receipt_task(entry: &Value) -> Value {
    json!({
        "title": entry.get("title").cloned().unwrap_or(Value::Null),
        "route": entry.get("route").cloned().unwrap_or(Value::Null),
        "block_id": entry.get("block_id").cloned().unwrap_or(Value::Null),
        "target": entry.get("target").cloned().unwrap_or(Value::Null),
        "woke_at": entry.get("woke_at").cloned().unwrap_or(Value::Null),
        "destination": entry
            .get("destination")
            .and_then(|destination| destination.get("name"))
            .cloned()
            .unwrap_or(Value::Null),
        "line": entry
            .get("destination")
            .and_then(|destination| destination.get("line"))
            .cloned()
            .unwrap_or(Value::Null),
    })
}

/// Atomically write the receipt (0600 from creation, then rename).
pub(crate) fn write_atomic(receipt: &WakeReceipt) -> io::Result<()> {
    let path = last_wake_path();
    let dir = path.parent().expect("receipt path has a parent");
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        let _ = builder.create(dir);
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let bytes = serde_json::to_vec_pretty(receipt)?;
    let temp = dir.join(format!(
        ".last-wake.{}.{}.tmp",
        process::id(),
        RECEIPT_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let write_result = (|| -> io::Result<()> {
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                fs::set_permissions(&temp, fs::Permissions::from_mode(0o600));
        }
        fs::rename(&temp, &path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

/// True when `last-wake.json` parses and carries `wake_id`.
pub(crate) fn is_readable(wake_id: &str) -> bool {
    let Ok(text) = fs::read_to_string(last_wake_path()) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
        return false;
    };
    value.get("wake_id").and_then(Value::as_str) == Some(wake_id)
}

pub(crate) fn post_darwin() -> bool {
    #[cfg(test)]
    {
        TEST_DARWIN.with(|calls| {
            calls.borrow_mut().push(WOKEN_NOTIFICATION.to_string());
        });
        !TEST_DARWIN_FAIL.get()
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        crate::native::darwin_notify::post(WOKEN_NOTIFICATION)
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        let _ = WOKEN_NOTIFICATION;
        true
    }
}

#[cfg(test)]
pub(crate) fn take_darwin_posts() -> Vec<String> {
    TEST_DARWIN.with(|calls| std::mem::take(&mut *calls.borrow_mut()))
}

#[cfg(test)]
pub(crate) fn set_darwin_fail(fail: bool) {
    TEST_DARWIN_FAIL.set(fail);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolate_state() -> (tempfile::TempDir, crate::native::env::TestEnvGuard)
    {
        let temp = tempfile::tempdir().expect("tempdir");
        let guard = crate::native::env::TestEnvGuard::set(&[(
            "XDG_STATE_HOME",
            Some(temp.path().as_os_str()),
        )]);
        (temp, guard)
    }

    fn sample_report() -> Value {
        json!({
            "schema_version": 1,
            "date": "2026-10-08",
            "woken": [{
                "route": "jobs.md",
                "block_id": "job",
                "title": "Review the launch plan",
                "target": "1100",
                "woke_at": "12:00",
                "destination": {"line": 3, "name": "WRITE", "created": false},
            }],
            "plan": {"count": 4, "cap": 10, "over": false},
        })
    }

    #[test]
    fn write_is_readable_before_darwin_and_uses_0600() {
        let (_temp, _guard) = isolate_state();
        let _ = take_darwin_posts();
        let receipt = from_report(
            &sample_report(),
            Path::new("2026/20261008.md"),
            "2026-10-08T12:00:00-04:00",
        )
        .expect("nonempty woken");
        write_atomic(&receipt).expect("write receipt");
        assert!(is_readable(&receipt.wake_id));
        let posted = post_darwin();
        assert!(posted);
        assert_eq!(take_darwin_posts(), [WOKEN_NOTIFICATION]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(last_wake_path())
                .expect("receipt metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let on_disk: WakeReceipt = serde_json::from_str(
            &fs::read_to_string(last_wake_path()).expect("read receipt"),
        )
        .expect("parse receipt");
        assert_eq!(on_disk.wake_id, receipt.wake_id);
        assert_eq!(on_disk.tasks[0]["destination"], "WRITE");
        assert_eq!(on_disk.tasks[0]["line"], 3);
        assert_eq!(on_disk.daily_file, "2026/20261008.md");
    }

    #[test]
    fn empty_woken_builds_no_receipt() {
        let report = json!({"woken": [], "plan": {}});
        assert!(from_report(&report, Path::new("day.md"), "now").is_none());
    }
}
