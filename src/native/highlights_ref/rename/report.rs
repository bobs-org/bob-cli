//! Human rendering for the `bob ref rename` planner.

use super::plan::{MoveKind, RenameEnvelope};
use crate::native::style::Styler;

pub(super) fn render_human(report: &RenameEnvelope) -> String {
    let style = Styler::detect();
    let mut out = String::new();
    out.push_str(&format!(
        "{} {} → {}{}\n",
        style.success_prefix(report.mode == "dry_run"),
        style.cyan(&report.from.path),
        style.cyan(&report.to.path),
        if report.mode == "dry_run" {
            " (dry run)"
        } else {
            ""
        },
    ));
    if report.moves.is_empty() {
        out.push_str("  no file moves\n");
    }
    for movement in &report.moves {
        let kind = match movement.kind {
            MoveKind::Note => "note",
            MoveKind::Pdf => "pdf",
            MoveKind::Sidecar => "sidecar",
            MoveKind::Audio => "audio",
        };
        out.push_str(&format!(
            "  {kind}: {} → {}\n",
            movement.from, movement.to
        ));
    }
    out.push_str(&format!(
        "  links: {} in {} file{}\n",
        report.links.rewritten,
        report.links.files.len(),
        if report.links.files.len() == 1 {
            ""
        } else {
            "s"
        },
    ));
    let task_path = report
        .kept
        .task
        .as_ref()
        .map(|task| task.path.as_str())
        .unwrap_or("(none)");
    let task_block = report
        .kept
        .task
        .as_ref()
        .and_then(|task| task.block_id.as_deref())
        .map(|id| format!("#^{id}"))
        .unwrap_or_default();
    if report.id.changed {
        out.push_str(&format!(
            "  id: {} → {} (note and PDF marker)\n",
            report.id.from.as_deref().unwrap_or("(none)"),
            report.id.to.as_deref().unwrap_or("(none)"),
        ));
        out.push_str(&format!("  keeps: task {task_path}{task_block}\n"));
    } else {
        out.push_str(&format!(
            "  keeps: id {} · task {task_path}{task_block}\n",
            report.id.from.as_deref().unwrap_or("(none)"),
        ));
    }
    if !report.frontmatter.is_empty() {
        out.push_str(&format!(
            "  frontmatter: {}\n",
            report.frontmatter.join(", ")
        ));
    }
    for warning in &report.warnings {
        out.push_str(&format!("  {}: {warning}\n", style.warning_prefix()));
    }
    if report.outcome == "unchanged" {
        out.push_str("  outcome: unchanged\n");
    } else if report.outcome == "already_renamed" {
        out.push_str("  outcome: already_renamed\n");
    } else if report.outcome == "id_updated" {
        out.push_str("  outcome: id_updated\n");
    }
    if report.mode == "write" {
        if let Some(commit) = &report.commit {
            let sha = commit
                .get("sha")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(unknown)");
            let subject = commit
                .get("subject")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("bob ref rename");
            out.push_str(&format!("  committed {sha} {subject}\n"));
        }
        out.push_str(&format!(
            "  sync: pre {}, post {}\n",
            report.sync.pre, report.sync.post
        ));
    }
    out
}
