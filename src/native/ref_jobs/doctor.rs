//! The `ref jobs:` doctor row: pending, stuck, oldest pending age.

use super::{
    output::age_short_secs,
    spool::{jobs_dir, oldest_pending_age_secs, pending_and_stuck_counts},
};

/// Warn when a pending job is older than 1 hour (hint
/// `bob ref jobs run`) or any job is stuck.
pub(crate) fn append_ref_jobs_doctor_row(warnings: &mut Vec<String>) {
    let root = jobs_dir();
    let (pending, stuck) = pending_and_stuck_counts(&root);
    let oldest = oldest_pending_age_secs(&root);
    if pending == 0 && stuck == 0 {
        println!("ref jobs: ok (nothing pending)");
        return;
    }
    let mut parts = Vec::new();
    if pending > 0 {
        parts.push(format!("{pending} pending"));
    }
    if stuck > 0 {
        parts.push(format!("{stuck} stuck"));
    }
    if let Some(secs) = oldest {
        parts.push(format!("oldest {}", age_short_secs(secs)));
    }
    let detail = parts.join(" · ");
    let stale = oldest.is_some_and(|secs| secs >= 3600);
    if stuck > 0 || stale {
        println!("ref jobs: warn ({detail} · hint: bob ref jobs run)");
        if stuck > 0 {
            warnings.push(format!(
                "{stuck} ref job(s) are stuck: the fallback write failed; run `bob ref jobs run` to retry"
            ));
        }
        if stale {
            warnings.push(
                "a ref job has been pending for over an hour: run `bob ref jobs run`".to_string(),
            );
        }
    } else {
        println!("ref jobs: ok ({detail})");
    }
}
