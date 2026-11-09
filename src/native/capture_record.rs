//! Native record-capture adapter scaffolding (phase `record-language`).
//!
//! Both parse and execution build their immutable record route policy
//! through [`record_policy_for_draft`]: one memoized policy per
//! invocation, with record-specific I/O limited to a few `is_dir`
//! stats for lexically-detected candidate dirs. The per-folder record
//! index stays untouched here; the `record-parse` phase extends this
//! adapter with the shared resolver and catalog wiring.

use std::collections::HashSet;
use std::path::Path;

use super::capture_language::{candidate_dirs, RecordRoutePolicy};
use super::config;

/// Build the record route policy for one draft: lexical candidates
/// first, then config load, then `is_dir` stats for candidates only.
/// No candidates means no config or filesystem reads at all, so
/// ordinary task drafts pay nothing for record support.
pub(crate) fn record_policy_for_draft(
    vault_root: &Path,
    config_path: Option<&Path>,
    raw_text: &str,
) -> RecordRoutePolicy {
    let candidates = candidate_dirs(raw_text);
    if candidates.is_empty() {
        return RecordRoutePolicy::absent();
    }
    let path = config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(config::config_path);
    let policy = match config::records::load_records_config_from(&path) {
        Ok(records) => RecordRoutePolicy::from_config(&records, Some(&path)),
        Err(diagnostic) => {
            RecordRoutePolicy::invalid(&path, diagnostic.message)
        }
    };
    let mut existing = HashSet::new();
    for dir in &candidates {
        if policy.configured_dir(dir).is_some() {
            continue;
        }
        if vault_root.join(dir).is_dir() {
            existing.insert(dir.clone());
        }
    }
    policy.with_existing(existing)
}
