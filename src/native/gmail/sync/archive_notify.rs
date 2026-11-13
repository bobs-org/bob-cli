//! One desktop notification for newly durable Gmail archive outcomes.
//!
//! The summary fires after a live `bob gmail sync` journals fresh `archived`
//! rows, even under `-q`, and stays silent for everything else: dry-run and
//! preview rows, historical `recent_archive` snapshots, repeat syncs with no
//! new rows, skipped or failed archive phases, and paused batches. The body
//! carries a usable `bob gmail unarchive` command with the exact archived
//! thread ids, never note prose or sender text. Delivery is best-effort and
//! never touches the report, its JSON, or the exit code.

use super::{
    archive_apply::ACTION_ARCHIVED, archive_preview::ACTION_PAUSED,
    report::SyncReport,
};
use crate::native::{desktop_notify, gmail::cli::SyncArgs};

/// Thread ids to announce: distinct live `archived` rows in first-seen order.
pub(crate) fn archived_thread_ids(report: &SyncReport) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut ids = Vec::new();
    for row in &report.archive {
        if row.action != ACTION_ARCHIVED {
            continue;
        }
        if seen.insert(row.thread_id.clone()) {
            ids.push(row.thread_id.clone());
        }
    }
    ids.sort();
    ids
}

/// Whether this completed run earned a notification. Only newly durable
/// `archived` rows qualify: previews, history reads, skips, pauses, and
/// archive-phase failures never do.
pub(crate) fn should_notify(args: &SyncArgs, report: &SyncReport) -> bool {
    if args.dry_run
        || report.dry_run
        || report.busy
        || report.archive_skipped
        || report.archive_error.is_some()
    {
        return false;
    }
    if report.archive.iter().any(|row| row.action == ACTION_PAUSED) {
        return false;
    }
    !archived_thread_ids(report).is_empty()
}

/// One summary line plus the exact undo command users can copy. Thread ids
/// come from Gmail's own identifier alphabet; anything outside it is
/// single-quoted so the copied command stays inert.
pub(crate) fn notification_body(thread_ids: &[String]) -> String {
    let command = unarchive_display(thread_ids);
    if thread_ids.len() == 1 {
        format!("Archived 1 Gmail thread — undo: {command}")
    } else {
        format!(
            "Archived {} Gmail threads — undo: {command}",
            thread_ids.len()
        )
    }
}

/// The argv a user runs to restore exactly these threads.
pub(crate) fn unarchive_argv(thread_ids: &[String]) -> Vec<String> {
    let mut argv = vec![
        "bob".to_string(),
        "gmail".to_string(),
        "unarchive".to_string(),
        "--".to_string(),
    ];
    argv.extend(thread_ids.iter().cloned());
    argv
}

/// The same argv rendered for the notification body, with hostile tokens
/// quoted.
pub(crate) fn unarchive_display(thread_ids: &[String]) -> String {
    let quoted: Vec<String> = thread_ids
        .iter()
        .map(|id| desktop_notify::shell_display_token(id))
        .collect();
    unarchive_argv(&quoted).join(" ")
}

/// Fire the notification when this run earned one. Best-effort: delivery
/// failures stay silent and never affect the report or exit code.
pub(crate) fn maybe_notify(args: &SyncArgs, report: &SyncReport) {
    if !should_notify(args, report) {
        return;
    }
    let ids = archived_thread_ids(report);
    desktop_notify::send_notification(
        desktop_notify::GMAIL_TITLE,
        &notification_body(&ids),
    );
}

#[cfg(test)]
mod tests {
    use super::super::report::{
        ArchiveConfigSummary, ArchiveHint, ArchiveRow, SyncReport,
    };
    use super::*;

    fn report_with_actions(actions: &[&'static str]) -> SyncReport {
        SyncReport {
            ok: true,
            dry_run: false,
            pulled: 0,
            results: Vec::new(),
            watch: Vec::new(),
            gone: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 0,
            awaiting: 0,
            first_seen: false,
            busy: false,
            archive: actions
                .iter()
                .enumerate()
                .map(|(index, action)| ArchiveRow {
                    thread_id: format!("thread{index}"),
                    note: String::new(),
                    action,
                    reason: "eligible",
                    decision: "would_archive".to_string(),
                    messages: vec!["m".to_string()],
                    tasks: Vec::new(),
                })
                .collect(),
            archive_digest: None,
            archive_config: Some(ArchiveConfigSummary {
                enabled: true,
                label: "bob/closed".to_string(),
                max: 10,
            }),
            archive_hints: Vec::<ArchiveHint>::new(),
            archive_skipped: false,
            archive_error: None,
            recent_archive: None,
        }
    }

    fn live_args() -> SyncArgs {
        SyncArgs {
            bob_dir: None,
            confirm: None,
            dry_run: false,
            format: crate::native::gmail::cli::HumanFormat::Human,
            limit: 0,
            no_archive: false,
            quiet: true,
            retry_timeout: 0,
        }
    }

    #[test]
    fn archived_rows_notify_even_when_quiet() {
        let report = report_with_actions(&["archived"]);
        assert!(should_notify(&live_args(), &report));
        assert_eq!(archived_thread_ids(&report), vec!["thread0".to_string()]);
    }

    #[test]
    fn archived_ids_sort_and_deduplicate() {
        let mut report = report_with_actions(&["archived", "archived"]);
        report.archive[0].thread_id = "b".to_string();
        report.archive[1].thread_id = "a".to_string();
        report.archive.push(ArchiveRow {
            thread_id: "a".to_string(),
            note: String::new(),
            action: "archived",
            reason: "eligible",
            decision: "would_archive".to_string(),
            messages: vec!["m".to_string()],
            tasks: Vec::new(),
        });
        assert_eq!(
            archived_thread_ids(&report),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn previews_history_and_non_archive_rows_stay_silent() {
        for actions in [
            vec![],
            vec!["would_archive"],
            vec!["kept"],
            vec!["let_go"],
            vec!["restored"],
            vec!["paused"],
        ] {
            let report = report_with_actions(&actions);
            assert!(
                !should_notify(&live_args(), &report),
                "actions {actions:?} must stay silent"
            );
        }
    }

    #[test]
    fn dry_run_busy_skipped_and_failed_runs_stay_silent() {
        let archived = report_with_actions(&["archived"]);
        let mut dry = live_args();
        dry.dry_run = true;
        assert!(!should_notify(&dry, &archived));
        let mut busy = report_with_actions(&["archived"]);
        busy.busy = true;
        assert!(!should_notify(&live_args(), &busy));
        let mut skipped = report_with_actions(&["archived"]);
        skipped.archive_skipped = true;
        assert!(!should_notify(&live_args(), &skipped));
        let mut failed = report_with_actions(&["archived"]);
        failed.archive_error = Some(crate::native::gmail::GmailError::runtime(
            "archive",
            "boom".to_string(),
        ));
        assert!(!should_notify(&live_args(), &failed));
        let paused = report_with_actions(&["archived", "paused"]);
        assert!(!should_notify(&live_args(), &paused));
    }

    #[test]
    fn body_names_the_count_and_the_exact_undo_command() {
        assert_eq!(
            notification_body(&["abc123".to_string()]),
            "Archived 1 Gmail thread — undo: bob gmail unarchive -- abc123"
        );
        assert_eq!(
            notification_body(&["b".to_string(), "a".to_string()]),
            "Archived 2 Gmail threads — undo: bob gmail unarchive -- b a"
        );
        assert_eq!(
            unarchive_argv(&["abc123".to_string()]),
            vec!["bob", "gmail", "unarchive", "--", "abc123"]
        );
    }

    #[test]
    fn hostile_thread_ids_quote_in_display_but_not_argv() {
        let hostile = "x; rm -rf ~".to_string();
        assert_eq!(
            unarchive_display(std::slice::from_ref(&hostile)),
            "bob gmail unarchive -- 'x; rm -rf ~'"
        );
        assert_eq!(unarchive_argv(std::slice::from_ref(&hostile))[4], hostile);
    }
}
