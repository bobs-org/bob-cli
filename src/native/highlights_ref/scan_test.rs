//! Debug-only handshake and plan-log seams for adversarial scan tests.
//!
//! CLI tests set `BOB_REF_SCAN_TEST_PAUSE` to a stage name (`planning`,
//! `after_snapshot`, `before_evidence`, `after_prepare`, `before_lock`,
//! `before_intake`, `after_intake_move`) plus ready/go files. The child
//! writes `<seq>:<stage>` to the ready path and waits until the go path
//! contains that seq, so waits are handshake-bounded rather than
//! sleep-and-hope. Release builds compile these into no-ops.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static MUTATION_LOCK_DEPTH: AtomicUsize = AtomicUsize::new(0);

pub(super) fn enter_mutation_window() {
    MUTATION_LOCK_DEPTH.fetch_add(1, Ordering::SeqCst);
}

pub(super) fn leave_mutation_window() {
    MUTATION_LOCK_DEPTH.fetch_sub(1, Ordering::SeqCst);
}

#[cfg(debug_assertions)]
fn mutation_window_held() -> bool {
    MUTATION_LOCK_DEPTH.load(Ordering::SeqCst) > 0
}

/// Pause when this process was launched with a matching test handshake.
pub(super) fn pause(stage: &str) {
    #[cfg(debug_assertions)]
    pause_debug(stage);
    #[cfg(not(debug_assertions))]
    let _ = stage;
}

/// Append one plan_pdfs invocation to the test plan log.
pub(super) fn log_plan_pdfs(pdfs: &[PathBuf]) {
    #[cfg(debug_assertions)]
    log_plan_pdfs_debug(pdfs);
    #[cfg(not(debug_assertions))]
    let _ = pdfs;
}

#[cfg(debug_assertions)]
const ENV_PAUSE: &str = "BOB_REF_SCAN_TEST_PAUSE";
#[cfg(debug_assertions)]
const ENV_READY: &str = "BOB_REF_SCAN_TEST_READY";
#[cfg(debug_assertions)]
const ENV_GO: &str = "BOB_REF_SCAN_TEST_GO";
#[cfg(debug_assertions)]
const ENV_PAUSE_EVERY: &str = "BOB_REF_SCAN_TEST_PAUSE_EVERY";
#[cfg(debug_assertions)]
const ENV_PLAN_LOG: &str = "BOB_REF_SCAN_TEST_PLAN_LOG";
#[cfg(debug_assertions)]
static PAUSE_SEQ: AtomicUsize = AtomicUsize::new(0);
#[cfg(debug_assertions)]
static PAUSED_STAGES: std::sync::Mutex<Vec<String>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(debug_assertions)]
fn pause_debug(stage: &str) {
    use std::time::{Duration, Instant};
    if !wants_stage(stage) {
        return;
    }
    let Some(ready) = std::env::var_os(ENV_READY) else {
        return;
    };
    let Some(go) = std::env::var_os(ENV_GO) else {
        return;
    };
    if !should_pause_stage(stage) {
        return;
    }
    let seq = PAUSE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(parent) = Path::new(&ready).parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(&ready, format!("{seq}:{stage}")).is_err() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if fs::read_to_string(&go)
            .is_ok_and(|body| body.trim() == seq.to_string())
        {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    eprintln!(
        "bob ref scan: test handshake timed out waiting for {ENV_GO} seq {seq} at {stage}"
    );
}

#[cfg(debug_assertions)]
fn wants_stage(stage: &str) -> bool {
    let Ok(wanted) = std::env::var(ENV_PAUSE) else {
        return false;
    };
    wanted.split(',').any(|part| part.trim() == stage)
}

#[cfg(debug_assertions)]
fn should_pause_stage(stage: &str) -> bool {
    if std::env::var_os(ENV_PAUSE_EVERY).is_some() {
        return true;
    }
    let Ok(mut seen) = PAUSED_STAGES.lock() else {
        return true;
    };
    if seen.iter().any(|name| name == stage) {
        return false;
    }
    seen.push(stage.to_string());
    true
}

#[cfg(debug_assertions)]
fn log_plan_pdfs_debug(pdfs: &[PathBuf]) {
    let Some(path) = std::env::var_os(ENV_PLAN_LOG) else {
        return;
    };
    if let Some(parent) = Path::new(&path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let locked = u8::from(mutation_window_held());
    let mut line = format!("locked={locked} count={}", pdfs.len());
    for pdf in pdfs {
        line.push(' ');
        line.push_str(&pdf.to_string_lossy());
    }
    line.push('\n');
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(line.as_bytes())
        });
}
