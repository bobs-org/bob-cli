//! Shell completion answered live by the installed `bob` binary.
//!
//! Every TAB runs a hidden `bob __complete` request against one
//! composed clap tree plus bob's own read-only value providers, so
//! completion can never drift from the CLI. See `docs/completion.md`
//! for the protocol and the runtime model.

mod adapters;
mod context;
mod engine;
mod kinds;
mod present;
mod protocol;
mod providers;
mod tree;

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Vault scans run behind this deadline so a slow vault can never
/// delay the prompt. The hidden `BOB_COMPLETE_DEADLINE_MS` override
/// exists for tests.
const COMPLETE_DEADLINE_MS: u64 = 150;

#[allow(unused_imports)]
pub(crate) use tree::tree;

/// Answer a hidden `__complete` request.
///
/// Exit 0 with protocol 1 response lines on stdout, including on
/// internal failure (which yields empty output). A malformed request
/// exits 2 with a message on stderr. Panics are silenced into empty
/// output, and completion never writes except to `BOB_COMPLETE_DEBUG`.
pub(crate) fn run_complete(argv: &[OsString]) -> i32 {
    // A silent panic hook plus `catch_unwind` turns panics into empty
    // output; adapters discard stderr and treat that as no candidates.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_complete_inner(argv)
        }));
    std::panic::set_hook(previous_hook);
    outcome.unwrap_or(0)
}

fn run_complete_inner(argv: &[OsString]) -> i32 {
    let started = Instant::now();
    let mut debug_error: Option<String> = None;
    // The shell changes only presentation in protocol 1, and the suffix
    // is recorded for the debug log and later phases; both stay in the
    // request shape so adapters can rely on it.
    let mut debug_shell = String::from("malformed");
    let mut debug_suffix: Option<OsString> = None;
    let (lines, exit_code) = match protocol::parse_request(argv) {
        Err(message) => {
            eprintln!("{message}");
            debug_error = Some(message.clone());
            (Vec::new(), 2)
        }
        Ok(request) => {
            debug_shell = format!("{:?}", request.shell);
            debug_suffix = request.suffix.clone();
            match protocol::skew_message(request.protocol) {
                Some(skew) => (vec![skew], 0),
                None => {
                    let deadline = deadline_ms();
                    let (sender, receiver) = mpsc::channel();
                    // The worker owns the request; the main thread
                    // only waits. On timeout the main thread prints
                    // nothing and exits, so a straggling worker can
                    // never delay the prompt.
                    std::thread::spawn(move || {
                        let lines = std::panic::catch_unwind(
                            std::panic::AssertUnwindSafe(|| {
                                present::complete_request(&request)
                            }),
                        )
                        .unwrap_or_default();
                        let _ = sender.send(lines);
                    });
                    match receiver.recv_timeout(Duration::from_millis(deadline))
                    {
                        Ok(lines) => (lines, 0),
                        Err(_) => {
                            let message = format!("timeout after {deadline}ms");
                            debug_error = Some(message);
                            (Vec::new(), 0)
                        }
                    }
                }
            }
        }
    };
    write_debug_log(
        argv,
        &debug_shell,
        debug_suffix.as_ref(),
        &lines,
        started,
        debug_error.as_deref(),
    );
    if exit_code == 0 && !lines.is_empty() {
        println!("{}", lines.join("\n"));
    }
    exit_code
}

/// Read the completion deadline: 150 ms, unless the hidden
/// `BOB_COMPLETE_DEADLINE_MS` test override sets a positive value.
fn deadline_ms() -> u64 {
    std::env::var("BOB_COMPLETE_DEADLINE_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(COMPLETE_DEADLINE_MS)
}

/// Append the request, the response, the elapsed milliseconds, and any
/// error to `BOB_COMPLETE_DEBUG`. Failures stay silent: debugging must
/// never break completion.
fn write_debug_log(
    argv: &[OsString],
    shell: &str,
    suffix: Option<&OsString>,
    lines: &[String],
    started: Instant,
    error: Option<&str>,
) {
    let Ok(path) = std::env::var("BOB_COMPLETE_DEBUG") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let elapsed_ms = started.elapsed().as_millis();
    let mut entry = format!(
        "request={argv:?} shell={shell} suffix={suffix:?}\nresponse={lines:?}\nelapsed_ms={elapsed_ms}\n"
    );
    if let Some(error) = error {
        entry.push_str(&format!("error={error:?}\n"));
    }
    let path = std::path::Path::new(&path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) =
        OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = file.write_all(entry.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Restores the deadline override when the guard drops, so the
    /// mutation cannot leak into other tests.
    struct DeadlineGuard {
        previous: Option<String>,
    }

    impl DeadlineGuard {
        fn set(value: Option<&str>) -> Self {
            let previous = std::env::var("BOB_COMPLETE_DEADLINE_MS").ok();
            // `unsafe` because another thread could read the var
            // concurrently; no other test reads this var, and the
            // guard restores it on drop.
            unsafe {
                match value {
                    Some(value) => {
                        std::env::set_var("BOB_COMPLETE_DEADLINE_MS", value);
                    }
                    None => {
                        std::env::remove_var("BOB_COMPLETE_DEADLINE_MS");
                    }
                }
            }
            Self { previous }
        }
    }

    impl Drop for DeadlineGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(previous) => {
                        std::env::set_var("BOB_COMPLETE_DEADLINE_MS", previous);
                    }
                    None => {
                        std::env::remove_var("BOB_COMPLETE_DEADLINE_MS");
                    }
                }
            }
        }
    }

    #[test]
    fn deadline_defaults_and_falls_back() {
        let _unset = DeadlineGuard::set(None);
        assert_eq!(deadline_ms(), COMPLETE_DEADLINE_MS);

        let _override = DeadlineGuard::set(Some("5000"));
        assert_eq!(deadline_ms(), 5000);

        // Empty, zero, and non-numeric overrides fall back instead
        // of disabling the deadline.
        for raw in ["", "0", "abc"] {
            let _guard = DeadlineGuard::set(Some(raw));
            assert_eq!(deadline_ms(), COMPLETE_DEADLINE_MS, "raw: {raw}");
        }
    }
}
