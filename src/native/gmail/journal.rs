//! The per-host `bob gmail` journal: `.bob/gmail/<host>.jsonl`.
//!
//! Shell phase owns the namespace and the malformed-line count for
//! `doctor`; save/compose/sync phases append their own events.

use std::path::Path;

use crate::native::pref_journal::Namespace;

pub(crate) const NAMESPACE: Namespace = Namespace {
    dir: ".bob/gmail",
    noun: "gmail",
};

pub(crate) const PULLED_KIND: &str = "gmail.pulled";

/// Count malformed `gmail.*` journal lines for `doctor`.
pub(crate) fn malformed_count(vault: &Path) -> usize {
    let directory = vault.join(NAMESPACE.dir);
    let mut paths = match std::fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "jsonl")
                    && path.is_file()
            })
            .collect::<Vec<_>>(),
        Err(_) => return 0,
    };
    paths.sort();
    let mut count = 0usize;
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed: Result<serde_json::Value, _> =
                serde_json::from_str(trimmed);
            let Ok(value) = parsed else {
                count += 1;
                continue;
            };
            let kind = value.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            if kind.is_empty() || !kind.starts_with("gmail.") {
                // Unknown or missing kinds are skipped by the fold; only
                // malformed `gmail.*` lines count here so future save
                // events never trip the shell-phase doctor.
                if kind.starts_with("gmail.") {
                    count += 1;
                }
                continue;
            }
            if value.get("ts").is_none()
                || value.get("host").is_none()
                || value.get("value").is_none()
            {
                count += 1;
            }
        }
    }
    count
}
