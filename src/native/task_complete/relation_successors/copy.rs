//! Bob-authored relation-successor copy (`docs/task-dependencies.md` §12.18).
//!
//! This is a typed presentation API for the classifier and downstream
//! capture/authoring consumers. Keep formatting here so those consumers do
//! not build relation copy independently.

use crate::native::note_tasks::clean_description;

const MAX_TASK_TEXT_CHARS: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Constructed by the classifier and presentation consumers.
pub(crate) enum EventKind {
    Parked,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Used by the authoring and released-target presentation consumers.
pub(crate) enum ReleaseState {
    Parked,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Produced by the classifier before it is rendered as copy.
pub(crate) enum HeldReason {
    AlreadyPlanned,
    AlreadyStarted,
    WaitsFor(Vec<String>),
    Blocked { waits_on: usize },
    Scheduled(String),
    Project,
    Hidden,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct GroupCopy<'a> {
    pub(crate) label: &'a str,
    pub(crate) target: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct HeldNoticeCopy<'a> {
    pub(crate) source: &'a str,
    pub(crate) label: &'a str,
    pub(crate) target: &'a str,
    pub(crate) reason: &'a HeldReason,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FoldCopy<'a> {
    pub(crate) source: Option<&'a str>,
    pub(crate) waits_for: &'a [String],
    pub(crate) already_started: usize,
    pub(crate) blocked: usize,
    pub(crate) hidden: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickedNoticeCopy<'a> {
    pub(crate) count: usize,
    pub(crate) destination: &'a str,
    pub(crate) source: Option<&'a str>,
    pub(crate) label: &'a str,
    pub(crate) target: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructuredWarning {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

/// Clean task text using the standard task-description convention and cap
/// the result at 48 Unicode scalar values, including the ellipsis.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn task_text(text: &str) -> String {
    let cleaned = clean_description(text, "#task", None);
    let chars = cleaned.chars().collect::<Vec<_>>();
    if chars.len() <= MAX_TASK_TEXT_CHARS {
        return cleaned;
    }
    let visible = chars
        .into_iter()
        .take(MAX_TASK_TEXT_CHARS - 1)
        .collect::<String>();
    format!("{}…", visible.trim_end())
}

fn quoted(text: &str) -> String {
    format!("“{}”", task_text(text))
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
        None => String::new(),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn group_title(copy: GroupCopy<'_>) -> String {
    format!("{} {}", capitalized(copy.label), quoted(copy.target))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn event_why(target: &str, event: EventKind) -> String {
    match event {
        EventKind::Parked => {
            format!("{} is parked · Pending, off today's plan", quoted(target))
        }
        EventKind::Done => format!("{} is done", quoted(target)),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn contrast_tag() -> &'static str {
    "your links · not dependents"
}

/// Build one folded held summary. Empty parts are omitted; no line is
/// returned when every count and wait is zero.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn fold_line(copy: FoldCopy<'_>) -> Option<String> {
    let mut parts = Vec::new();
    if let (Some(source), Some(target)) = (copy.source, copy.waits_for.first())
    {
        let mut waits =
            format!("{} waits for {}", quoted(source), quoted(target));
        if copy.waits_for.len() > 1 {
            waits.push_str(&format!(
                " +{} more waiting",
                copy.waits_for.len() - 1
            ));
        }
        parts.push(waits);
    }
    if copy.already_started > 0 {
        parts.push(format!("{} already started", copy.already_started));
    }
    if copy.blocked > 0 {
        parts.push(format!("{} blocked", copy.blocked));
    }
    if copy.hidden > 0 {
        parts.push(format!("{} hidden", copy.hidden));
    }
    (!parts.is_empty()).then(|| format!("not offered · {}", parts.join(" · ")))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn trigger_suffix(targets: &[String]) -> Option<String> {
    let first = targets.first()?;
    if targets.len() == 1 {
        return None;
    }
    if targets.len() == 2 {
        Some(format!(
            "after {} and {}",
            quoted(first),
            quoted(&targets[1])
        ))
    } else {
        Some(format!(
            "after {} and {} more",
            quoted(first),
            targets.len() - 1
        ))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn secondary_reason(copy: GroupCopy<'_>) -> String {
    format!("· {} {}", copy.label, quoted(copy.target))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn picker_title(
    copy: GroupCopy<'_>,
    relation_only_count: Option<usize>,
    existing_mixed_title: Option<&str>,
) -> String {
    if let Some(title) = existing_mixed_title {
        return title.to_string();
    }
    match relation_only_count {
        Some(count) => format!("{} {} tasks", capitalized(copy.label), count),
        None => group_title(copy),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn capsule(label: &str) -> String {
    capitalized(label)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn picked_notice(copy: PickedNoticeCopy<'_>) -> String {
    let relation = format!("{} {}", copy.label, quoted(copy.target));
    if let (1, Some(source)) = (copy.count, copy.source) {
        return format!(
            "🪢 Next in {}: {} · {}",
            copy.destination,
            task_text(source),
            relation
        );
    }
    format!(
        "🪢 {} linked → {} · {}",
        copy.count, copy.destination, relation
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn held_notice(copy: HeldNoticeCopy<'_>) -> Option<String> {
    let reason = match copy.reason {
        HeldReason::AlreadyPlanned => return None,
        HeldReason::AlreadyStarted => "already started".to_string(),
        HeldReason::WaitsFor(targets) => {
            let first = targets.first()?;
            if targets.len() == 1 {
                format!("waits for {}", quoted(first))
            } else {
                format!(
                    "waits for {} and {} more",
                    quoted(first),
                    targets.len() - 1
                )
            }
        }
        HeldReason::Blocked { waits_on } => {
            format!("blocked (waits on {waits_on})")
        }
        HeldReason::Scheduled(date) => format!("scheduled {date}"),
        HeldReason::Project => "blocked by its project".to_string(),
        HeldReason::Hidden => "hidden".to_string(),
    };
    Some(format!(
        "🪢 {} {} {} · not offered: {}",
        quoted(copy.source),
        copy.label,
        quoted(copy.target),
        reason
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn uncertainty_notice(labels: &[String]) -> Option<String> {
    let subject = match labels.len() {
        0 => return None,
        1 => format!("{} links", labels[0]),
        _ => "relation links".to_string(),
    };
    Some(format!("🪢 Couldn't check {subject} · nothing offered"))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn cli_header(copy: GroupCopy<'_>, event: EventKind) -> String {
    let event_copy = match event {
        EventKind::Parked => "parked",
        EventKind::Done => "done",
    };
    format!(
        "{} · it is {} ({})",
        group_title(copy),
        event_copy,
        "your 🪢 links, not dependents"
    )
}

/// The CLI's compact held-row fold has a distinct two-space separator
/// after “not offered” per the shared copy table.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn cli_held_fold(
    source: &str,
    target: &str,
    already_started: usize,
) -> String {
    let mut line = format!(
        "not offered  {} waits for {}",
        quoted(source),
        quoted(target)
    );
    if already_started > 0 {
        line.push_str(&format!(" · {already_started} already started"));
    }
    line
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn mac_notification(
    count: usize,
    label: &str,
    target: &str,
) -> String {
    format!("🪢 Pulled in {count} · {label} {}", quoted(target))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn mac_badge_help() -> &'static str {
    "Offered by your relation link"
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn suggestion() -> &'static str {
    "🪢 offers"
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn relation_row() -> &'static str {
    "🪢 offers source"
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn list_header(count: usize) -> String {
    format!("· {count} offer")
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn list_row(count: usize) -> String {
    format!("{count} links · offers source")
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn release_caveat(
    state: ReleaseState,
    target: &str,
    source: Option<&str>,
) -> String {
    match state {
        ReleaseState::Parked => format!(
            "{} is already parked — {} will be offered when it is done",
            quoted(target),
            source
                .map(quoted)
                .unwrap_or_else(|| "the source".to_string())
        ),
        ReleaseState::Closed => format!(
            "{} is already closed — nothing left to wait for",
            quoted(target)
        ),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn inverse_hint(
    typed: &str,
    source: &str,
    configured_label: &str,
) -> String {
    format!(
        "{} never offers anything · to offer {} after this task, use ~~ with {}",
        quoted(typed),
        quoted(source),
        quoted(configured_label)
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn already_released_warning(
    _label: &str,
    source: &str,
    target: &str,
) -> StructuredWarning {
    StructuredWarning {
        code: "relation_successor_already_released",
        message: release_caveat(ReleaseState::Parked, target, Some(source)),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn rename_warning(label: &str) -> String {
    format!(
        "{} is a successor label in config.yml — update relations.successor_labels",
        quoted(label)
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn capture_complete_verdict(source: &str, target: &str) -> String {
    format!(
        "🪢 Offers {} when {} is parked or done · never blocks",
        quoted(source),
        quoted(target)
    )
}
