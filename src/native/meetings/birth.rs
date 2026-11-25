//! Meeting birth helpers: recap rendering and LINKS children.
//!
//! Pure, I/O-free except for the vault index already built by the caller.
//! The sync glue in `highlights_ref::meeting_birth` owns vault reads,
//! the birth-store lookup, and config validation; this module only shapes
//! strings so both inline ingest and the queued drain birth byte-identical
//! notes and tasks.

use super::{MeetingNotes, RecapMeta};

/// Render the full owned recap for a meetings birth from normalized notes.
pub(crate) fn render_birth_recap(
    notes: &MeetingNotes,
    meta: &RecapMeta,
) -> String {
    super::recap::render_recap(notes, meta)
}

/// Render an empty owned recap when the birth store is missing.
///
/// The region stays `Owned` (valid digest over the meta line alone) so a
/// later `bob ref scan` preserves it byte for byte and never rewrites it.
pub(crate) fn render_empty_recap(meta: &RecapMeta) -> String {
    let empty = MeetingNotes::default();
    super::recap::render_recap(&empty, meta)
}

/// Build the LINKS child for a meetings birth insert.
///
/// Reading-task children use two-space indent; the caller appends meeting
/// warnings after this line. The label is already validated and the link is
/// already canonical.
pub(crate) fn meeting_links_child(
    relation_label: &str,
    canonical_event_link: &str,
) -> String {
    crate::native::task_relations::format::format_links_line(
        "  ",
        &[(
            canonical_event_link.to_string(),
            Some(relation_label.to_string()),
        )],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> RecapMeta {
        RecapMeta {
            date: "2026-11-24".into(),
            span: "14:00–14:30".into(),
            event_link: "[[20261124#^cedar-driftwood]]".into(),
            producer: Some("VoiceQuill".into()),
            event_title: None,
        }
    }

    #[test]
    fn empty_recap_is_owned() {
        let recap = render_empty_recap(&meta());
        assert!(recap.contains("<!-- bob:meeting-recap:begin v1 sha256:"));
        assert!(recap.contains("<!-- bob:meeting-recap:end -->"));
        assert_eq!(
            super::super::recap::recap_state(&recap),
            super::super::recap::RecapState::Owned
        );
    }

    #[test]
    fn links_child_is_canonical() {
        let line = meeting_links_child("notes from", "[[job#^ref-x]]");
        assert_eq!(line, "  - 🪢 **LINKS:** notes from [[job#^ref-x]]");
    }
}
