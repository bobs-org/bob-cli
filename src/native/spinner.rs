//! A stderr progress spinner shared by gkeep and gcal login.
//!
//! On a TTY, [`Spinner::start`] animates with `\r` redraws. Without a
//! TTY, `BOB_PROGRESS=1` prints one line-oriented `{label}…` line and
//! never uses `\r`. Otherwise construction is a no-op. Callers pass no
//! label at all in JSON mode or with `--quiet`, so construction itself
//! stays unconditional here.

use std::{
    io::{IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Braille progress frames drawn by [`Spinner`].
pub(crate) const SPINNER_FRAMES: &[&str] =
    &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A stderr progress spinner, stopped by clearing the line on drop.
pub(crate) struct Spinner {
    running: Option<RunningSpinner>,
}

struct RunningSpinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Start spinning `label` on stderr, or print a single progress line
    /// when `BOB_PROGRESS=1` and stderr is not a TTY.
    pub(crate) fn start(label: &str) -> Self {
        if !std::io::stderr().is_terminal() {
            crate::native::style::stderr_progress_line(&format!("{label}…"));
            return Self { running: None };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let label = label.to_string();
        let handle = thread::spawn(move || {
            let mut frame = 0;
            while !thread_stop.load(Ordering::Relaxed) {
                let glyph = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
                eprint!("\r{glyph} {label}…");
                let _ = std::io::stderr().flush();
                frame += 1;
                thread::sleep(Duration::from_millis(80));
            }
        });
        Self {
            running: Some(RunningSpinner {
                stop,
                handle: Some(handle),
            }),
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        if let Some(running) = self.running.as_mut() {
            running.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = running.handle.take() {
                let _ = handle.join();
            }
            eprint!("\r\x1b[K");
            let _ = std::io::stderr().flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_are_braille() {
        assert_eq!(SPINNER_FRAMES.len(), 10);
        assert_eq!(SPINNER_FRAMES[0], "⠋");
    }

    #[test]
    fn spinner_start_and_drop_never_panics() {
        let _spinner = Spinner::start("Syncing Google Keep");
    }
}
