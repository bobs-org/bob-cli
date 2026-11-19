#[cfg(test)]
use super::LogKind;

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct MachineWriter {
    pub(crate) kind: LogKind,
    pub(crate) helper: &'static str,
    pub(crate) source: &'static str,
    pub(crate) sample: String,
}

/// Machine writers in this repo use their real formatter where available;
/// plugin samples cite the source constant/formatter that owns the text.
#[cfg(test)]
pub(crate) fn machine_writers() -> Vec<MachineWriter> {
    use crate::native::{
        capture_schedule_log, gcal::lifecycle::AbsentVerdict,
        task_merge::section_provenance_reason,
    };
    vec![
        writer(
            LogKind::Schedule,
            "priority_roll_reason",
            "src/native/capture/plan.rs",
            capture_schedule_log::priority_roll_reason("P1", "P2", 3, 2, 7),
        ),
        writer(
            LogKind::Schedule,
            "priority_roll_reason",
            "src/native/capture/project_note.rs",
            capture_schedule_log::priority_roll_reason("P1", "P2", 3, 2, 7),
        ),
        writer(
            LogKind::Schedule,
            "randomize_reason",
            "src/native/randomize_plan.rs",
            capture_schedule_log::randomize_reason("P2", 3, 2, 7, None),
        ),
        writer(
            LogKind::Cancel,
            "cancel_reason",
            "src/native/gcal/plan.rs",
            AbsentVerdict::Deleted
                .cancel_reason()
                .expect("deleted reason"),
        ),
        writer(
            LogKind::Cancel,
            "cancel_reason",
            "src/native/gcal/plan.rs",
            AbsentVerdict::Declined
                .cancel_reason()
                .expect("declined reason"),
        ),
        writer(
            LogKind::Cancel,
            "cancel_reason",
            "src/native/gcal/plan.rs",
            AbsentVerdict::Moved("Fri 2026-11-20".into())
                .cancel_reason()
                .expect("moved reason"),
        ),
        writer(
            LogKind::Cancel,
            "section_provenance_reason",
            "src/native/task_merge/retirement.rs",
            section_provenance_reason("target", "heading"),
        ),
        writer(
            LogKind::Schedule,
            "SCHEDULE_LOG_AUTO_REASON_EMOJI",
            "bob-navigation-hotkeys/src/030-bullet-blocks-and-logs.js",
            "🎲 P2 roll · in 3 (2–7) days".into(),
        ),
        writer(
            LogKind::Work,
            "WORK_LOG_NO_REASON_GIVEN",
            "bob-navigation-hotkeys/src/030-bullet-blocks-and-logs.js",
            "🤷 no reason given".into(),
        ),
        writer(
            LogKind::Rotten,
            "formatRottenLogEntryText",
            "bob-navigation-hotkeys/src/195-rotten-keep.js",
            "🍂 P1 → P2".into(),
        ),
        writer(
            LogKind::Cancel,
            "RELATION_CANCEL_FALLBACK",
            "bob-navigation-hotkeys/src/596-relation-cancel-compose.js",
            "🪢 duplicate of [[target#^id]]".into(),
        ),
        writer(
            LogKind::Work,
            "UNBLOCK_LOG_MARKER",
            "bob-navigation-hotkeys/src/030-bullet-blocks-and-logs.js",
            "🔓 unblocked after prerequisite completion".into(),
        ),
        writer(
            LogKind::Work,
            "GMAIL_REPLY_MARKER",
            "bob-navigation-hotkeys/src/030-bullet-blocks-and-logs.js",
            "📬 reply received".into(),
        ),
    ]
}

#[cfg(test)]
fn writer(
    kind: LogKind,
    helper: &'static str,
    source: &'static str,
    sample: String,
) -> MachineWriter {
    MachineWriter {
        kind,
        helper,
        source,
        sample,
    }
}

pub(crate) fn is_machine_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    ["🎲", "🤷", "🍂", "🪢", "📬", "🔓"]
        .iter()
        .any(|glyph| trimmed.starts_with(glyph))
        || trimmed.starts_with("📥 merged into [[")
        || trimmed.starts_with("Deleted in Google Calendar")
        || trimmed.starts_with("Declined in Google Calendar")
        || (trimmed.starts_with("Moved to ")
            && trimmed.ends_with(" in Google Calendar")
            && trimmed.len() > "Moved to  in Google Calendar".len())
}

#[cfg(test)]
pub(crate) fn writer_call_sites() -> &'static [(&'static str, &'static str)] {
    &[
        ("priority_roll_reason", "src/native/capture/plan.rs"),
        ("priority_roll_reason", "src/native/capture/project_note.rs"),
        ("randomize_reason", "src/native/randomize_plan.rs"),
        ("cancel_reason", "src/native/gcal/plan.rs"),
        (
            "section_provenance_reason",
            "src/native/task_merge/retirement.rs",
        ),
    ]
}
