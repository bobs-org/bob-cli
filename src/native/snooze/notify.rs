//! Wake nudge arbitration: Mac Capture claim, desktop fallback, `off`.
//!
//! The durable receipt protocol is independent of `snooze.notify`. This
//! module only chooses the post-commit nudge and the `notified` report.

use std::fs;

use serde_json::{json, Value};

use crate::native::{config::NotifyMode, desktop_notify, env as bob_env};

use super::receipt::{self, WakeReceipt};

const TITLE: &str = "bob snooze";

#[cfg(test)]
thread_local! {
    static TEST_PID_ALIVE: std::cell::RefCell<Option<fn(i32) -> bool>> =
        const { std::cell::RefCell::new(None) };
    static TEST_DESKTOP: std::cell::RefCell<Option<bool>> =
        const { std::cell::RefCell::new(None) };
    static TEST_DESKTOP_CALLS: std::cell::RefCell<Vec<(String, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Who received the wake nudge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Notified {
    MacCapture,
    Desktop,
    None,
}

impl Notified {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MacCapture => "mac_capture",
            Self::Desktop => "desktop",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Delivery {
    pub notified: Notified,
    pub wake_id: Value,
}

/// Write the receipt, post Darwin once it is readable, then nudge.
///
/// Receipt, Darwin, and desktop errors warn on stderr and never fail the
/// committed wake. `wake_id` is reported only when the matching receipt is
/// readable.
pub(crate) fn deliver(
    report: &Value,
    daily_file: &std::path::Path,
    woken_at: &str,
    mode: NotifyMode,
) -> Delivery {
    let Some(receipt) = receipt::from_report(report, daily_file, woken_at)
    else {
        return Delivery {
            notified: Notified::None,
            wake_id: Value::Null,
        };
    };
    match write_and_post(&receipt) {
        Ok(()) => Delivery {
            notified: nudge(mode, report),
            wake_id: json!(receipt.wake_id),
        },
        Err(message) => {
            eprintln!("bob snooze: {message}");
            Delivery {
                notified: Notified::None,
                wake_id: Value::Null,
            }
        }
    }
}

fn write_and_post(receipt: &WakeReceipt) -> Result<(), String> {
    receipt::write_atomic(receipt)
        .map_err(|error| format!("could not write wake receipt: {error}"))?;
    if !receipt::is_readable(&receipt.wake_id) {
        return Err("wake receipt is not readable".into());
    }
    if !receipt::post_darwin() {
        eprintln!(
            "bob snooze: could not post wake notification: {}",
            receipt::WOKEN_NOTIFICATION
        );
    }
    Ok(())
}

fn nudge(mode: NotifyMode, report: &Value) -> Notified {
    match mode {
        NotifyMode::Off => Notified::None,
        NotifyMode::Desktop => send_desktop_nudge(report),
        NotifyMode::Auto => {
            if live_mac_claim() {
                Notified::MacCapture
            } else {
                send_desktop_nudge(report)
            }
        }
    }
}

fn send_desktop_nudge(report: &Value) -> Notified {
    let woken = report
        .get("woken")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if woken.is_empty() {
        return Notified::None;
    }
    let (title, body) = desktop_message(woken);
    if send_desktop(&title, &body) {
        Notified::Desktop
    } else {
        Notified::None
    }
}

/// One-task copy: `Ready now: <title> (woke 15:32 · WRITE)`.
/// Batches: `N snoozed tasks are ready: …`.
pub(crate) fn desktop_message(woken: &[Value]) -> (String, String) {
    if woken.len() == 1 {
        let task_title = woken[0]
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("task");
        let woke_at = woken[0]
            .get("woke_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        let destination = woken[0]
            .get("destination")
            .and_then(|value| {
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .filter(|name| !name.is_empty());
        let body = match destination {
            Some(name) => {
                format!("Ready now: {task_title} (woke {woke_at} · {name})")
            }
            None => format!("Ready now: {task_title} (woke {woke_at})"),
        };
        return (TITLE.to_string(), body);
    }
    let titles: Vec<&str> = woken
        .iter()
        .map(|task| task.get("title").and_then(Value::as_str).unwrap_or("task"))
        .take(3)
        .collect();
    let listed = titles.join(", ");
    let body = if woken.len() > 3 {
        format!("{} snoozed tasks are ready: {listed}, …", woken.len())
    } else {
        format!("{} snoozed tasks are ready: {listed}", woken.len())
    };
    (TITLE.to_string(), body)
}

/// A claim is live only when it is well-formed and its pid is alive.
/// Corrupt or unsupported claims do not suppress the desktop fallback.
pub(crate) fn live_mac_claim() -> bool {
    let Ok(text) = fs::read_to_string(receipt::notifier_path()) else {
        return false;
    };
    parse_live_claim(&text)
}

fn parse_live_claim(text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return false;
    }
    let Some(pid) = object.get("pid").and_then(Value::as_i64) else {
        return false;
    };
    if pid <= 0 || pid > i64::from(i32::MAX) {
        return false;
    }
    for key in ["app", "version", "started_at"] {
        match object.get(key).and_then(Value::as_str) {
            Some(value) if !value.is_empty() => {}
            _ => return false,
        }
    }
    pid_is_alive(pid as i32)
}

fn pid_is_alive(pid: i32) -> bool {
    #[cfg(test)]
    {
        if let Some(hook) = TEST_PID_ALIVE.with(|cell| *cell.borrow()) {
            return hook(pid);
        }
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn send_desktop(title: &str, body: &str) -> bool {
    #[cfg(test)]
    {
        TEST_DESKTOP_CALLS.with(|calls| {
            calls
                .borrow_mut()
                .push((title.to_string(), body.to_string()));
        });
        if let Some(forced) = TEST_DESKTOP.with(|cell| *cell.borrow()) {
            return forced;
        }
    }
    if let Ok(sink) = bob_env::var("BOB_SNOOZE_NOTIFY_SINK") {
        if sink == "fail" || sink.is_empty() || sink == "off" {
            return false;
        }
        let payload = json!({"title": title, "body": body});
        return fs::write(&sink, payload.to_string()).is_ok();
    }
    matches!(
        desktop_notify::send_notification_result(title, body),
        desktop_notify::DesktopNotifyResult::Sent
    )
}

#[cfg(test)]
pub(crate) fn set_pid_alive(hook: Option<fn(i32) -> bool>) {
    TEST_PID_ALIVE.with(|cell| *cell.borrow_mut() = hook);
}

#[cfg(test)]
pub(crate) fn set_desktop_result(result: Option<bool>) {
    TEST_DESKTOP.with(|cell| *cell.borrow_mut() = result);
}

#[cfg(test)]
pub(crate) fn take_desktop_calls() -> Vec<(String, String)> {
    TEST_DESKTOP_CALLS.with(|calls| std::mem::take(&mut *calls.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::env::TestEnvGuard;
    use std::path::Path;

    fn isolate() -> (tempfile::TempDir, TestEnvGuard) {
        let temp = tempfile::tempdir().expect("tempdir");
        let guard = TestEnvGuard::set(&[(
            "XDG_STATE_HOME",
            Some(temp.path().as_os_str()),
        )]);
        (temp, guard)
    }

    fn report() -> Value {
        json!({
            "schema_version": 1,
            "date": "2026-10-08",
            "woken": [{
                "route": "jobs.md",
                "block_id": "job",
                "title": "Email Dan",
                "target": "1530",
                "woke_at": "15:32",
                "destination": {"line": 4, "name": "WRITE", "created": false},
            }],
            "plan": {"count": 4, "cap": 10, "over": false},
        })
    }

    fn write_claim(body: &str) {
        fs::create_dir_all(receipt::snooze_state_dir()).expect("state dir");
        fs::write(receipt::notifier_path(), body).expect("write claim");
    }

    fn always_alive(_: i32) -> bool {
        true
    }
    fn never_alive(_: i32) -> bool {
        false
    }

    #[test]
    fn single_and_batch_desktop_copy() {
        let one = [json!({
            "title": "Email Dan",
            "woke_at": "15:32",
            "destination": {"name": "WRITE"},
        })];
        assert_eq!(
            desktop_message(&one),
            (
                "bob snooze".into(),
                "Ready now: Email Dan (woke 15:32 · WRITE)".into()
            )
        );
        let many = [
            json!({"title": "One"}),
            json!({"title": "Two"}),
            json!({"title": "Three"}),
        ];
        assert_eq!(
            desktop_message(&many),
            (
                "bob snooze".into(),
                "3 snoozed tasks are ready: One, Two, Three".into()
            )
        );
    }

    #[test]
    fn auto_yields_to_live_claim_and_skips_desktop() {
        let (_temp, _guard) = isolate();
        let _ = take_desktop_calls();
        let _ = receipt::take_darwin_posts();
        set_pid_alive(Some(always_alive));
        set_desktop_result(Some(true));
        write_claim(
            r#"{"schema_version":1,"pid":4242,"app":"Bob Mac Capture","version":"1.0","started_at":"2026-10-08T12:00:00Z"}"#,
        );
        let delivery = deliver(
            &report(),
            Path::new("2026/20261008.md"),
            "2026-10-08T15:32:00-04:00",
            NotifyMode::Auto,
        );
        set_pid_alive(None);
        set_desktop_result(None);
        assert_eq!(delivery.notified, Notified::MacCapture);
        assert!(delivery.wake_id.as_str().is_some());
        assert!(take_desktop_calls().is_empty());
        assert_eq!(receipt::take_darwin_posts(), [receipt::WOKEN_NOTIFICATION]);
    }

    #[test]
    fn corrupt_and_dead_claims_do_not_suppress_desktop() {
        let (_temp, _guard) = isolate();
        set_desktop_result(Some(true));
        for claim in [
            "not json",
            r#"{"schema_version":2,"pid":1,"app":"x","version":"1","started_at":"t"}"#,
            r#"{"schema_version":1,"app":"x","version":"1","started_at":"t"}"#,
            r#"{"schema_version":1,"pid":"nope","app":"x","version":"1","started_at":"t"}"#,
        ] {
            write_claim(claim);
            set_pid_alive(Some(always_alive));
            let delivery = deliver(
                &report(),
                Path::new("day.md"),
                "now",
                NotifyMode::Auto,
            );
            assert_eq!(delivery.notified, Notified::Desktop, "{claim}");
        }
        write_claim(
            r#"{"schema_version":1,"pid":9,"app":"x","version":"1","started_at":"t"}"#,
        );
        set_pid_alive(Some(never_alive));
        let delivery =
            deliver(&report(), Path::new("day.md"), "now", NotifyMode::Auto);
        set_pid_alive(None);
        set_desktop_result(None);
        assert_eq!(delivery.notified, Notified::Desktop);
    }

    #[test]
    fn desktop_mode_ignores_live_claim_and_off_nudges_nothing() {
        let (_temp, _guard) = isolate();
        let _ = take_desktop_calls();
        set_pid_alive(Some(always_alive));
        set_desktop_result(Some(true));
        write_claim(
            r#"{"schema_version":1,"pid":7,"app":"x","version":"1","started_at":"t"}"#,
        );
        let desktop =
            deliver(&report(), Path::new("day.md"), "now", NotifyMode::Desktop);
        assert_eq!(desktop.notified, Notified::Desktop);
        assert_eq!(take_desktop_calls().len(), 1);
        let off =
            deliver(&report(), Path::new("day.md"), "now", NotifyMode::Off);
        set_pid_alive(None);
        set_desktop_result(None);
        assert_eq!(off.notified, Notified::None);
        assert!(take_desktop_calls().is_empty());
        assert!(off.wake_id.as_str().is_some());
    }

    #[test]
    fn desktop_failure_reports_none_and_keeps_receipt() {
        let (_temp, _guard) = isolate();
        set_desktop_result(Some(false));
        let delivery =
            deliver(&report(), Path::new("day.md"), "now", NotifyMode::Desktop);
        set_desktop_result(None);
        assert_eq!(delivery.notified, Notified::None);
        assert!(delivery.wake_id.as_str().is_some());
        assert!(receipt::last_wake_path().exists());
    }

    #[test]
    fn darwin_failure_still_reports_wake_id_when_receipt_is_readable() {
        let (_temp, _guard) = isolate();
        receipt::set_darwin_fail(true);
        set_desktop_result(Some(true));
        let delivery =
            deliver(&report(), Path::new("day.md"), "now", NotifyMode::Desktop);
        receipt::set_darwin_fail(false);
        set_desktop_result(None);
        assert_eq!(delivery.notified, Notified::Desktop);
        assert!(delivery.wake_id.as_str().is_some());
    }
}
