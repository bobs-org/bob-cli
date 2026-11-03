//! `%c` origin recording: one `attachment.import` journal event per
//! committed attachment (`docs/attachments.md`).
//!
//! After the notes commit succeeds, every attachment `%c` copied (history
//! entries included) appends an event with `via: "capture"`. Reused
//! destinations still get an event, because the new source is still this
//! embed's origin. Snippet files are not attachments, so they never
//! appear here. Failures never fail the capture: each one becomes an
//! `attachment_origin_failed` warning. Other `commit_capture_batch`
//! callers need no recording: the existing-task-link path always carries
//! `clip_plan: None`, and the inbox-route caller is test-only.

use std::collections::BTreeSet;

use super::*;
use crate::native::attachments::journal;

/// Append one `attachment.import` event per distinct committed
/// attachment (`via: "capture"`), after the notes commit succeeded. In a
/// multi-item draft, repeated copies of one source collapse to one event.
pub(super) fn append_committed_attachment_origins(
    batch: &PlannedCaptureBatch,
    vault: &std::path::Path,
) -> Vec<CaptureWarning> {
    let mut seen = BTreeSet::new();
    let mut attachments = Vec::new();
    for item in &batch.items {
        let Some(plan) = &item.clip_plan else {
            continue;
        };
        for output in
            std::iter::once(&plan.output).chain(plan.output.entries.iter())
        {
            for attachment in &output.attachments {
                if seen.insert((
                    attachment.saved.clone(),
                    attachment.source.clone(),
                )) {
                    attachments.push((
                        attachment.saved.clone(),
                        attachment.source.clone(),
                        attachment.sha256.clone(),
                    ));
                }
            }
        }
    }
    journal::append_origin_events(vault, &attachments, "capture")
        .into_iter()
        .map(|failure| {
            CaptureWarning::Structured(StructuredCaptureWarning {
                code: "attachment_origin_failed",
                label: None,
                count: None,
                message: format!(
                    "could not record attachment origin: {}",
                    failure.error,
                ),
            })
        })
        .collect()
}
