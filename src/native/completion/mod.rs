//! Shell completion answered live by the installed `bob` binary.
//!
//! Every TAB runs a hidden `bob __complete` request against one
//! composed clap tree plus bob's own read-only value providers, so
//! completion can never drift from the CLI. See `docs/completion.md`
//! for the protocol and the runtime model.

mod engine;
mod kinds;
mod present;
mod protocol;
mod tree;

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;

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
                None => (present::complete_request(&request), 0),
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
