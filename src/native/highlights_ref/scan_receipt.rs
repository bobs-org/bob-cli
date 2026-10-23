//! Scan receipt (`last-scan.json`) and the Darwin scan-finished notification.
//!
//! Every writing `bob ref scan` that acquired the scan writer lock writes the
//! exact envelope line JSON mode would print to
//! `<state>/bob-cli/ref/last-scan.json`, then (on macOS) posts
//! `org.bobs.bob.ref.scan-finished` so Bob Mac Capture can re-read it.
//! Dry runs, `ref sync`, and scans that never got the lock write nothing.
//! A receipt or notification failure warns on stderr and never changes
//! stdout or the exit code.
use super::*;

/// Darwin notification posted after every scan receipt write, on macOS.
pub(super) const SCAN_FINISHED_NOTIFICATION: &str =
    "org.bobs.bob.ref.scan-finished";

/// Tracks one writing full scan's receipt inputs: the planned intake moves
/// and how many already completed. Hard failures after the lock write the
/// same error envelope JSON mode would print.
pub(super) struct WriteScanReceipt<'a> {
    config: &'a Config,
    options: SyncOptions,
    intake: Vec<IntakeMove>,
    completed_intake: usize,
}

impl<'a> WriteScanReceipt<'a> {
    pub(super) fn new(config: &'a Config, options: SyncOptions) -> Self {
        Self {
            config,
            options,
            intake: Vec::new(),
            completed_intake: 0,
        }
    }

    pub(super) fn set_intake(
        &mut self,
        intake: Vec<IntakeMove>,
        completed_intake: usize,
    ) {
        self.intake = intake;
        self.completed_intake = completed_intake;
    }

    fn completed_moves(&self) -> Vec<ScanJsonIntakeMove> {
        intake_moves(
            self.config,
            &self.intake[..self.completed_intake.min(self.intake.len())],
        )
    }

    /// Write the hard-failure envelope for `error` as the scan receipt.
    pub(super) fn write_error(&self, error: &CommandError) {
        emit_scan_receipt(&render_scan_error_envelope(
            self.config,
            self.options,
            "write",
            self.completed_moves(),
            error,
            false,
        ));
    }
}

/// Receipt path: `<state>/bob-cli/ref/last-scan.json` (`$XDG_STATE_HOME`,
/// else `~/.local/state`), the same root as `scan.lock`.
pub(super) fn scan_receipt_path() -> PathBuf {
    bob_env::bob_cli_state_dir()
        .join("ref")
        .join("last-scan.json")
}

/// Atomically write one envelope line as the scan receipt (temp file plus
/// rename, directory mode 0700, file mode 0600), then post the Darwin
/// notification. Failures warn on stderr; stdout and the exit code never
/// change.
pub(super) fn emit_scan_receipt(envelope_line: &str) {
    if let Err(error) = write_receipt_file(envelope_line) {
        eprintln!("bob ref: could not write scan receipt: {error}");
    }
    post_scan_finished_notification();
}

fn write_receipt_file(envelope_line: &str) -> std::io::Result<()> {
    let path = scan_receipt_path();
    let dir = path.parent().expect("receipt path has a parent");
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        let _ = builder.create(dir);
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let temp = dir.join(format!(
        ".last-scan.{}.{}.tmp",
        process::id(),
        receipt_nonce()
    ));
    let write_result = (|| -> std::io::Result<()> {
        fs::write(&temp, format!("{envelope_line}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
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

fn receipt_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    NONCE.fetch_add(1, Ordering::Relaxed).wrapping_add(nanos)
}

fn post_scan_finished_notification() {
    #[cfg(target_os = "macos")]
    {
        if !darwin::post(SCAN_FINISHED_NOTIFICATION) {
            eprintln!(
                "bob ref: could not post scan-finished notification: {SCAN_FINISHED_NOTIFICATION}"
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // No listeners off macOS; the receipt alone is the channel.
        let _ = SCAN_FINISHED_NOTIFICATION;
    }
}

#[cfg(target_os = "macos")]
mod darwin {
    use std::ffi::CString;
    use std::os::raw::c_char;

    #[link(name = "System")]
    unsafe extern "C" {
        fn notify_post(name: *const c_char) -> i32;
    }

    pub(super) fn post(name: &str) -> bool {
        let Ok(name) = CString::new(name) else {
            return false;
        };
        // `notify_post` returns 0 on success.
        unsafe { notify_post(name.as_ptr()) == 0 }
    }
}
