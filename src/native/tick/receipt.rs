use super::{schedule::Trigger, state::atomic_json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Ok,
    Failed,
    Waiting,
    Skipped,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct GateInfo {
    pub kind: String,
    pub open: bool,
    pub reason: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_scan_due_at: Option<DateTime<Utc>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StepReceipt {
    pub name: String,
    pub cadence: String,
    pub outcome: Outcome,
    pub reason: String,
    pub detail: String,
    pub started_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub summary: String,
    pub next_due_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_finished_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<Outcome>,
    pub last_summary: Option<String>,
    /// Only plan mode uses this: it never claims work actually ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub would_run: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateInfo>,
    /// Child stderr is a transient human diagnostic, never persisted in receipts.
    #[serde(skip)]
    pub stderr_tail: Vec<String>,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Receipt {
    pub schema_version: u32,
    pub command: String,
    pub host: String,
    pub vault: PathBuf,
    pub trigger: Trigger,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub exit_code: i32,
    pub steps: Vec<StepReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
}
impl Receipt {
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        atomic_json(dir, "last.json", self)?;
        #[cfg(target_os = "macos")]
        if !crate::native::darwin_notify::post("org.bobs.bob.tick-finished") {
            eprintln!(
                "bob tick: warning: could not post tick-finished notification"
            );
        }
        Ok(())
    }
}
