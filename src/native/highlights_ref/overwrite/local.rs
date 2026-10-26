//! Local-route overwrite reports and staged-PDF marker extraction.
//!
//! Create's Markdown, original local PDF, and same-day vault-note routes
//! own rendering into private scratch. This module turns the prepared
//! plan and executor result into the user-facing dry-run and success
//! lines those routes share.

use std::path::Path;

use super::super::{
    marker, CommandError, MarkerValue, FIELD_CAPTURED, FIELD_ID, FIELD_PARENT,
    FIELD_STATUS,
};
use super::execute::{FinalMarkerMetadata, OverwriteResult};
use super::OverwritePreflight;

/// Dry-run plan for a local overwrite. No adapters, pandoc, or listen
/// run before this is printed, and it always ends with `writes: none`.
pub(in crate::native::highlights_ref) fn print_dry_run(
    plan: &OverwritePreflight,
    title: &str,
    replacing_audio: bool,
    explicit_status: Option<&str>,
) {
    println!("ok would replace Highlights-ready PDF");
    print_overwrite_line(
        &plan.ref_type,
        &plan.member.stem,
        &final_status(plan, explicit_status),
        plan.highlights,
    );
    println!("title: {title}");
    print_archive_line(plan);
    print_task_line(
        plan,
        predicted_mark(plan, explicit_status),
        &final_status(plan, explicit_status),
        kept_closed_task(plan, explicit_status),
    );
    if replacing_audio {
        println!(
            "audio: replace {}",
            plan.audio_path.as_deref().unwrap_or("companion")
        );
    } else if let Some(audio) = &plan.audio_path {
        println!(
            "audio: keep {audio} (narrates the replaced capture; add -L to re-narrate)"
        );
    } else {
        println!("audio: none");
    }
    println!("writes: none");
}

/// Success report after a verified local overwrite commit.
pub(in crate::native::highlights_ref) fn print_success(
    plan: &OverwritePreflight,
    result: &OverwriteResult,
    title: &str,
    replaced_audio: Option<&str>,
    kept_closed: bool,
) {
    print_success_report(plan, result, title, replaced_audio, kept_closed);
    print_commit_result(result);
}

/// Shared success details without the final commit line, so collection
/// output can include its part summary before printing the transaction result.
pub(in crate::native::highlights_ref) fn print_success_report(
    plan: &OverwritePreflight,
    result: &OverwriteResult,
    title: &str,
    replaced_audio: Option<&str>,
    kept_closed: bool,
) {
    println!("ok replaced Highlights-ready PDF");
    print_overwrite_line(
        &plan.ref_type,
        &plan.member.stem,
        &result.task_status,
        result.highlights,
    );
    println!("title: {title}");
    print_archive_line(plan);
    print_task_line(plan, result.task_mark, &result.task_status, kept_closed);
    if let Some(audio) = replaced_audio {
        println!("audio: replaced {audio}");
    } else if let Some(audio) = &plan.audio_path {
        println!(
            "audio: kept {audio} (narrates the replaced capture; add -L to re-narrate)"
        );
    }
}

/// Shared final transaction status for all in-place overwrite routes.
pub(in crate::native::highlights_ref) fn print_commit_result(
    result: &OverwriteResult,
) {
    if result.push_pending {
        println!(
            "overwritten; push pending (run bob vault-sync) {}",
            result.commit_sha
        );
    } else {
        println!(
            "sync: committed {}",
            &result.commit_sha[..result.commit_sha.len().min(7)]
        );
    }
}

/// The selected reading plus its final status and highlight count.
fn print_overwrite_line(
    ref_type: &str,
    stem: &str,
    status: &str,
    highlights: u64,
) {
    let count = if highlights == 1 {
        "1 highlight".to_string()
    } else {
        format!("{highlights} highlights")
    };
    println!("overwrite: {ref_type}/{stem} ({status} · {count})");
}

/// The status the overwrite leaves behind: an explicit `-s` wins,
/// otherwise the preserved pre-overwrite status.
fn final_status(
    plan: &OverwritePreflight,
    explicit_status: Option<&str>,
) -> String {
    explicit_status
        .map(str::to_string)
        .unwrap_or_else(|| plan.status.clone())
}

/// The task mark the overwrite leaves behind, for dry-run plans.
fn predicted_mark(
    plan: &OverwritePreflight,
    explicit_status: Option<&str>,
) -> char {
    explicit_status
        .and_then(|status| super::execute::status_to_mark(status).ok())
        .unwrap_or(plan.task.mark)
}

fn print_archive_line(plan: &OverwritePreflight) {
    if let Some(annotated) = &plan.annotated {
        let link_count = annotated.inbound.rewritten;
        let links = if link_count == 1 {
            "1 highlight link".to_string()
        } else {
            format!("{link_count} highlight links")
        };
        println!(
            "earlier: {} ({} highlights · {links} · annotated PDF retained: {})",
            annotated.frozen.path,
            annotated.highlights,
            annotated.bundle.annotated_pdf_path,
        );
    } else if let Some(archive) = &plan.frozen_note {
        println!("earlier: {}", archive.path);
    } else {
        println!(
            "earlier: nothing to archive (0 highlights, no notes); git keeps the old capture"
        );
    }
}

fn print_task_line(
    plan: &OverwritePreflight,
    mark: char,
    status: &str,
    kept_closed: bool,
) {
    let block = plan.task.block_id.as_deref().unwrap_or("ref");
    if kept_closed {
        println!(
            "task: kept {} ^{block} [{mark}] {status} · pass -s ready to reopen",
            plan.task.path,
        );
    } else {
        println!("task: kept {} ^{block} [{mark}] {status}", plan.task.path,);
    }
}

/// Read final marker identity from a privately stamped overwrite PDF.
pub(in crate::native::highlights_ref) fn marker_from_pdf(
    path: &Path,
) -> Result<FinalMarkerMetadata, CommandError> {
    let marker = marker::read_pdf_marker(path)?;
    let projection = marker::parse_marker(&marker.contents)?;
    let id = projection
        .get(FIELD_ID)
        .and_then(MarkerValue::as_string)
        .unwrap_or_default()
        .to_string();
    let title = projection
        .get("title")
        .and_then(MarkerValue::as_string)
        .unwrap_or_default()
        .to_string();
    let status = projection
        .get(FIELD_STATUS)
        .and_then(MarkerValue::as_string)
        .unwrap_or_default()
        .to_string();
    let parent = projection
        .get(FIELD_PARENT)
        .and_then(MarkerValue::as_string)
        .map(str::to_string);
    let captured = projection
        .get(FIELD_CAPTURED)
        .and_then(MarkerValue::as_string)
        .map(str::to_string);
    Ok(FinalMarkerMetadata {
        id,
        title,
        status,
        parent,
        captured,
    })
}

/// Vault-relative companion destination for a requested replacement.
/// Reuses the existing episode path when one is already bound; otherwise
/// the MP3 beside the library PDF.
pub(in crate::native::highlights_ref) fn replacement_audio_destination(
    plan: &OverwritePreflight,
) -> String {
    plan.audio_path.clone().unwrap_or_else(|| {
        Path::new(&plan.pdf_path)
            .with_extension("mp3")
            .to_string_lossy()
            .replace('\\', "/")
    })
}

/// Status to stamp on the replacement PDF. An explicit `-s` wins;
/// otherwise the located task's status is preserved. A blocked overlay
/// keeps the existing live-status class (`ready`/`next`/`wip`) because
/// the marker cannot itself be `blocked`.
pub(in crate::native::highlights_ref) fn stamp_status(
    plan: &OverwritePreflight,
    explicit_status: Option<&str>,
    existing_marker_status: Option<&str>,
) -> String {
    if let Some(status) = explicit_status {
        return status.to_string();
    }
    if plan.status == "blocked" {
        return existing_marker_status
            .filter(|status| matches!(*status, "ready" | "next" | "wip"))
            .unwrap_or("ready")
            .to_string();
    }
    plan.status.clone()
}

/// True when the kept task is closed or archived and the user did not
/// pass `-s` to reopen it.
pub(in crate::native::highlights_ref) fn kept_closed_task(
    plan: &OverwritePreflight,
    explicit_status: Option<&str>,
) -> bool {
    explicit_status.is_none()
        && !plan.task_view.archived
        && matches!(plan.task.mark, 'x' | 'X' | '-')
}
