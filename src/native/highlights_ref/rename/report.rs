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
    out.push_str(&format!(
        "  keeps: id {} · task {}{}\n",
        report.kept.marker_id.as_deref().unwrap_or("(none)"),
        report
            .kept
            .task
            .as_ref()
            .map(|task| task.path.as_str())
            .unwrap_or("(none)"),
        report
            .kept
            .task
            .as_ref()
            .and_then(|task| task.block_id.as_deref())
            .map(|id| format!("#^{id}"))
            .unwrap_or_default(),
    ));
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
    }
    out
}
