//! Step gates for `bob tick`: readiness, gcal cooldown helpers, refs change gate.
use super::{
    catalog::{GateResult, StepContext},
    receipt::GateInfo,
    schedule::Trigger,
    state::StepState,
};
use crate::native::{config as bob_config, env as bob_env, pomodoro};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, time::SystemTime};

/// Shared readiness over gcal's `WaitingReason` rules.
///
/// Uses the same path `bob task reconcile` reads (`pomodoro::day_file_for`)
/// and verifies it matches the path gcal resolves for today. A closed gate
/// is `waiting` with the stable code; an open gate is `None`.
pub(super) fn readiness_gate(
    vault: &Path,
    now: DateTime<Utc>,
) -> Option<GateResult> {
    let day_file = pomodoro::day_file_for(vault);
    let today = match bob_env::configured_timezone() {
        Some(tz) => now.with_timezone(&tz).date_naive(),
        None => now.with_timezone(&chrono::Local).date_naive(),
    };
    let gcal_rel = crate::native::gcal::snapshot::daily_relative_path(today);
    let gcal_abs = vault.join(&gcal_rel);
    if gcal_abs != day_file {
        eprintln!(
            "bob tick: warning: daily note path mismatch (pomodoro {} vs gcal {})",
            day_file.display(),
            gcal_abs.display()
        );
    }
    let contents = match fs::read_to_string(&day_file) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Some(GateResult {
                outcome: super::receipt::Outcome::Waiting,
                reason: "note_missing".into(),
                detail: "today's note doesn't exist yet".into(),
            });
        }
        Err(e) => {
            return Some(GateResult {
                outcome: super::receipt::Outcome::Waiting,
                reason: "note_missing".into(),
                detail: format!("today's note unreadable: {e}"),
            });
        }
    };
    if let Some(reason) =
        crate::native::gcal::sync::WaitingReason::of_contents(&contents)
    {
        return Some(GateResult {
            outcome: super::receipt::Outcome::Waiting,
            reason: reason.code().into(),
            detail: reason.detail(),
        });
    }
    None
}

/// Today's local day string (`YYYY-MM-DD`) in Bob's configured timezone.
pub(super) fn today_string_from(now: DateTime<Utc>) -> String {
    match bob_env::configured_timezone() {
        Some(tz) => now.with_timezone(&tz).date_naive().to_string(),
        None => now.with_timezone(&chrono::Local).date_naive().to_string(),
    }
}

/// Whether gcal should run with `-n 2` (yesterday + today).
pub(super) fn gcal_needs_yesterday(state: &StepState, today: &str) -> bool {
    state.yesterday_synced_on.as_deref() != Some(today)
}

/// Effective last start for gcal's due-gate: the later of tick's own last
/// start and gcal's host status `ts` when that status outcome is not
/// `waiting`.
pub(super) fn gcal_effective_last_started(
    prior: &StepState,
) -> Option<DateTime<Utc>> {
    let mut last = prior.last_started_at;
    if let Some(status) = crate::native::gcal::journal::read_status()
        && status.outcome != "waiting"
    {
        let ts_utc = status.ts.to_utc();
        last = match last {
            Some(existing) if existing >= ts_utc => Some(existing),
            _ => Some(ts_utc),
        };
    }
    last
}

/// Whether a pre-scan hook is configured (env or file config).
pub(super) fn refs_hook_configured() -> bool {
    if let Some(os) = bob_env::var_os("BOB_HIGHLIGHTS_PRE_SCAN_HOOK")
        && !os.to_string_lossy().trim().is_empty()
    {
        return true;
    }
    if bob_env::var_os("BOB_HIGHLIGHTS_PRE_SCAN_COMMAND").is_some() {
        return true;
    }
    let path = bob_config::config_path();
    bob_config::load_highlights_config(&path)
        .map(|c| c.pre_scan_hook().is_some())
        .unwrap_or(false)
}

/// SHA-256 over sorted (relative path, size, mtime_ns) of every regular
/// file under resolved `lib/` and `xlib/`, exactly as `bob ref scan`
/// resolves them.
pub(super) fn refs_fingerprint(vault: &Path) -> Option<String> {
    let config = crate::native::highlights_ref::Config::for_vault(vault);
    let mut entries: Vec<String> = Vec::new();
    for dir in [&config.lib_dir, &config.xlib_dir] {
        let Ok(read) = fs::read_dir(dir) else {
            continue;
        };
        let mut stack: Vec<std::path::PathBuf> =
            read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        while let Some(path) = stack.pop() {
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if let Ok(rd) = fs::read_dir(&path) {
                    stack.extend(rd.filter_map(|e| e.ok().map(|e| e.path())));
                }
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(vault)
                .unwrap_or(&path)
                .display()
                .to_string()
                .replace('\\', "/");
            let size = meta.len();
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            entries.push(format!("{rel}\0{size}\0{mtime_ns}"));
        }
    }
    entries.sort();
    let mut hasher = Sha256::new();
    for entry in &entries {
        hasher.update(entry.as_bytes());
        hasher.update(b"\n");
    }
    Some(hex::encode(hasher.finalize()))
}

/// Refs change gate (scheduled trigger only).
///
/// Returns `(GateInfo, Option<GateResult>)`: `GateInfo` always describes
/// the evaluation for the receipt; `Some` blocks the run.
pub(super) fn refs_gate(
    context: &StepContext<'_>,
) -> (GateInfo, Option<GateResult>) {
    let state = context.state;
    let now = context.now;
    let hook = refs_hook_configured();
    let full_scan_due_at = state
        .last_full_scan_at
        .map(|last| last + chrono::TimeDelta::hours(6));
    let full_due = state.last_full_scan_at.is_none_or(|last| {
        now.signed_duration_since(last) >= chrono::TimeDelta::hours(6)
    });
    if context.trigger != Trigger::Scheduled {
        let reason = match context.trigger {
            Trigger::Named => "named",
            Trigger::All => "all",
            Trigger::Scheduled => "due",
        };
        return (
            GateInfo {
                kind: "refs_change".into(),
                open: true,
                reason: reason.into(),
                detail: if hook {
                    "pre-scan hook configured".into()
                } else {
                    format!(
                        "named run · last full {}",
                        state
                            .last_full_scan_at
                            .map(|t| t.format("%H:%M").to_string())
                            .unwrap_or_else(|| "never".into())
                    )
                },
                fingerprint_changed: None,
                hook: Some(hook),
                full_scan_due_at,
            },
            None,
        );
    }
    let current = refs_fingerprint(context.vault);
    let changed = match (&current, &state.fingerprint) {
        (Some(cur), Some(prev)) => cur != prev,
        (Some(_), None) => true,
        (None, _) => false,
    };
    let open = changed || hook || full_due;
    let clock_label = state
        .last_started_at
        .map(format_clock)
        .unwrap_or_else(|| "never".into());
    let detail = if hook && open {
        "pre-scan hook configured".into()
    } else if changed && state.fingerprint.is_none() {
        "first scan on this host".into()
    } else if changed {
        format!("lib/ changed since {clock_label}")
    } else if !open {
        let due = full_scan_due_at
            .map(format_clock)
            .unwrap_or_else(|| "never".into());
        format!("unchanged since {clock_label} · full scan by {due}")
    } else {
        format!("full scan due · last full {clock_label}")
    };
    let info = GateInfo {
        kind: "refs_change".into(),
        open,
        reason: if open {
            "changed".into()
        } else {
            "unchanged".into()
        },
        detail: detail.clone(),
        fingerprint_changed: Some(changed),
        hook: Some(hook),
        full_scan_due_at,
    };
    if open {
        // Stash current fingerprint for the caller to persist on success.
        // The fingerprint itself is recomputed there; this gate only reports.
        let _ = current;
        (info, None)
    } else {
        (
            info,
            Some(GateResult {
                outcome: super::receipt::Outcome::Skipped,
                reason: "unchanged".into(),
                detail,
            }),
        )
    }
}

fn format_clock(at: DateTime<Utc>) -> String {
    match bob_env::configured_timezone() {
        Some(tz) => at.with_timezone(&tz).format("%H:%M").to_string(),
        None => at.with_timezone(&chrono::Local).format("%H:%M").to_string(),
    }
}
