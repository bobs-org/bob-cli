//! One best-effort desktop notification transport shared by vault sync and
//! Gmail sync. Delivery can never roll back the work it announces and never
//! affects exit codes or JSON output: every failure is swallowed.
//!
//! macOS notifies through `osascript`; Linux notifies through `notify-send`
//! when it is on `PATH`. The existing `BOB_VAULT_SYNC_NOTIFY` opt-out covers
//! every caller: the exact values `0`, `false`, or `no` suppress, `1` or
//! `true` force, and anything else keeps the default (on outside tests, off
//! inside unit tests so suites stay silent).

use std::process::{Command, Stdio};

use super::env as bob_env;

/// Title used by vault-sync clobber/conflict/task-loss notifications.
pub(crate) const VAULT_SYNC_TITLE: &str = "bob vault-sync";
/// Title used by the Gmail archive receipt notification.
pub(crate) const GMAIL_TITLE: &str = "bob gmail";

/// Whether desktop notifications may be delivered right now.
pub(crate) fn notifications_enabled() -> bool {
    match bob_env::var("BOB_VAULT_SYNC_NOTIFY") {
        Ok(value)
            if matches!(
                value.to_ascii_lowercase().as_str(),
                "0" | "false" | "no"
            ) =>
        {
            false
        }
        Ok(value)
            if matches!(value.to_ascii_lowercase().as_str(), "1" | "true") =>
        {
            true
        }
        _ => {
            #[cfg(test)]
            {
                false
            }
            #[cfg(not(test))]
            {
                true
            }
        }
    }
}

/// Deliver one notification; never fails, never blocks the caller on output.
pub(crate) fn send_notification(title: &str, body: &str) {
    if !notifications_enabled() {
        return;
    }
    let Some((program, args)) = notification_command(title, body) else {
        return;
    };
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Pure program-plus-argv builder behind [`send_notification`]. Returns `None`
/// when this host has no usable transport (Linux without `notify-send` on
/// `PATH`). Tests assert on this value with fake executables instead of
/// delivering real notifications.
pub(crate) fn notification_command(
    title: &str,
    body: &str,
) -> Option<(String, Vec<String>)> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            osascript_escape(body),
            osascript_escape(title),
        );
        Some(("osascript".to_string(), vec!["-e".to_string(), script]))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if !command_on_path("notify-send") {
            return None;
        }
        Some((
            "notify-send".to_string(),
            vec![title.to_string(), body.to_string()],
        ))
    }
}

/// Quote one display token for the undo command users copy from the
/// notification body. Bare tokens from Gmail's own identifier alphabet pass
/// through; anything else is single-quoted so a hostile or malformed id can
/// never inject shell syntax into the copied command.
pub(crate) fn shell_display_token(token: &str) -> String {
    if !token.is_empty()
        && token.chars().all(|char| {
            char.is_ascii_alphanumeric() || char == '-' || char == '_'
        })
    {
        token.to_string()
    } else {
        format!("'{}'", token.replace('\'', "'\\''"))
    }
}

/// Escape one string for embedding in an AppleScript double-quoted literal.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn osascript_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn command_on_path(command: &str) -> bool {
    let Some(path) = bob_env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(command).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osascript_escapes_backslash_then_quote() {
        assert_eq!(
            osascript_escape("say \"hi\" \\ bye"),
            "say \\\"hi\\\" \\\\ bye"
        );
    }

    #[test]
    fn safe_tokens_pass_through_and_hostile_ones_quote() {
        assert_eq!(shell_display_token("18a4b0c2d3e4f501"), "18a4b0c2d3e4f501");
        assert_eq!(shell_display_token("a-b_c9"), "a-b_c9");
        assert_eq!(shell_display_token("id; rm -rf ~"), "'id; rm -rf ~'");
        assert_eq!(shell_display_token("it's"), "'it'\\''s'");
        assert_eq!(shell_display_token(""), "''");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_builds_notify_send_argv() {
        let missing = bob_env::TestEnvGuard::set(&[(
            "PATH",
            Some(std::ffi::OsStr::new("/nonexistent-bob-notify-test/bin")),
        )]);
        assert!(notification_command("t", "b").is_none());
        drop(missing);
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("notify-send");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions =
                std::fs::metadata(&fake).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&fake, permissions).unwrap();
        }
        let present = bob_env::TestEnvGuard::set(&[(
            "PATH",
            Some(dir.path().as_os_str()),
        )]);
        assert_eq!(
            notification_command("bob gmail", "hello"),
            Some((
                "notify-send".to_string(),
                vec!["bob gmail".to_string(), "hello".to_string()]
            ))
        );
        drop(present);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_builds_osascript_argv() {
        let (_, args) = notification_command("bob gmail", "say \"hi\"")
            .expect("osascript is always available");
        assert_eq!(args[0], "-e");
        assert_eq!(
            args[1],
            "display notification \"say \\\"hi\\\"\" with title \"bob gmail\""
        );
    }
}
