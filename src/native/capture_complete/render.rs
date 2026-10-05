use serde_json::json;

use super::model::{
    Candidates, CaptureCompleteResult, CompleteError, DependencyCandidate,
    OutputFormat, PomodoroNameCandidate, TaskCompleteCandidate,
    TaskLinkCandidate, TaskParentCandidate,
};
use super::COMMAND_NAME;
use crate::native::{
    capture_block_ids,
    capture_completable_tasks::{group_label, role_label},
    capture_language::CompletionContext,
    capture_link_tasks,
    capture_pomodoros::PomodoroState,
    style::Styler,
};

pub(super) fn print_success(
    result: &CaptureCompleteResult,
    output_format: OutputFormat,
) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

pub(super) fn print_human_success(result: &CaptureCompleteResult) {
    let styler = Styler::detect();
    print_human_success_with_styler(result, &styler);
}

pub(super) fn print_human_success_with_styler(
    result: &CaptureCompleteResult,
    styler: &Styler,
) {
    let context_label = result.context.map(context_label).unwrap_or("none");
    println!(
        "Capture complete {} {}",
        styler.separator(),
        styler.cyan(context_label)
    );
    println!();
    println!(
        "  {}  {}-{}",
        styler.dim("replacement"),
        result.replacement.start,
        result.replacement.end
    );
    if let Some(block_id) = &result.block_id {
        println!();
        println!("  {}", styler.dim(&block_id_summary(block_id)));
    }
    if let Some(query) = &result.query {
        println!("  {}  {:?}", styler.dim("query"), query);
    }

    if result.candidates.len() == 0 {
        println!();
        println!("  No candidates found.");
        print_warnings(result, styler);
        println!();
        println!("0 candidates");
        return;
    }

    println!();
    println!("  Candidates");
    for line in candidate_lines(&result.candidates, result.context) {
        println!("    {} {}", styler.cyan(&line.0), styler.dim(&line.1));
    }
    print_warnings(result, styler);
    println!();
    println!("{} {}", result.candidates.len(), plural_candidates(result));
}

pub(super) fn block_id_summary(
    block_id: &capture_block_ids::BlockIdField,
) -> String {
    let intent = match block_id.intent {
        capture_block_ids::BlockIdIntent::Link => "link",
        capture_block_ids::BlockIdIntent::New => "new",
        capture_block_ids::BlockIdIntent::ProjectNote => "project_note",
    };
    let count = block_id.used.len();
    let ids = if count == 1 { "ID" } else { "IDs" };
    let mut summary =
        format!("{} {intent} {count} {ids} in use", block_id.relative_target);
    if !block_id.suggestions.is_empty() {
        summary.push_str("  suggestions: ");
        summary.push_str(&block_id.suggestions.join(", "));
    }
    summary
}

pub(super) fn print_warnings(result: &CaptureCompleteResult, styler: &Styler) {
    if result.warnings.is_empty() {
        return;
    }
    println!();
    println!("  Warnings");
    for warning in &result.warnings {
        println!("    {}", styler.yellow(warning));
    }
}

pub(super) fn plural_candidates(
    result: &CaptureCompleteResult,
) -> &'static str {
    if result.candidates.len() == 1 {
        "candidate"
    } else {
        "candidates"
    }
}

/// Human row for one `task_link` candidate: the `@route:id` (or
/// `@route:…` for ID-less tasks) plus `[status] text  · tail`, where the
/// tail is the queued Pomodoro name (`Planned` when unnamed), `In
/// Progress`, `Next`, or the note label, with `· needs ID
/// (^suggestion)` on ID-less rows and `· scheduled DATE` whenever the task
/// carries a scheduled date.
pub(super) fn task_link_line(item: &TaskLinkCandidate) -> (String, String) {
    let tail = match item.pomodoro.as_ref() {
        Some(pomodoro) => pomodoro
            .name
            .clone()
            .unwrap_or_else(|| "Planned".to_string()),
        None => match item.group {
            capture_link_tasks::LinkTaskGroup::Queued => "Planned".to_string(),
            capture_link_tasks::LinkTaskGroup::InProgress => {
                "In Progress".to_string()
            }
            capture_link_tasks::LinkTaskGroup::Next => "Next".to_string(),
            capture_link_tasks::LinkTaskGroup::Note => {
                format!("{}.md", item.route)
            }
        },
    };
    let mut detail =
        format!("[{}] {}  · {tail}", item.status_symbol, item.text);
    if item.requires_block_id {
        match item.block_id_suggestions.first() {
            Some(suggestion) => {
                detail.push_str(&format!(" · needs ID (^{suggestion})"))
            }
            None => detail.push_str(" · needs ID"),
        }
    }
    if let Some(scheduled) = &item.scheduled {
        detail.push_str(&format!("  · scheduled {scheduled}"));
    }
    let label = if item.requires_block_id {
        format!("@{}:…", item.route)
    } else {
        item.replacement.clone()
    };
    (label, detail)
}

/// Human row for a `task_parent` candidate, using the plus marker so the
/// rendered replacement matches the draft an accept will insert.
pub(super) fn task_parent_line(item: &TaskParentCandidate) -> (String, String) {
    let tail = match item.pomodoro.as_ref() {
        Some(pomodoro) => pomodoro
            .name
            .clone()
            .unwrap_or_else(|| "Planned".to_string()),
        None => match item.group {
            capture_link_tasks::LinkTaskGroup::Queued => "Planned".to_string(),
            capture_link_tasks::LinkTaskGroup::InProgress => {
                "In Progress".to_string()
            }
            capture_link_tasks::LinkTaskGroup::Next => "Next".to_string(),
            capture_link_tasks::LinkTaskGroup::Note => {
                format!("{}.md", item.route)
            }
        },
    };
    let mut detail =
        format!("[{}] {}  · {tail}", item.status_symbol, item.text);
    if item.requires_block_id {
        match item.block_id_suggestions.first() {
            Some(suggestion) => {
                detail.push_str(&format!(" · needs ID (^{suggestion})"))
            }
            None => detail.push_str(" · needs ID"),
        }
    }
    let label = if item.requires_block_id {
        format!("@{}+…", item.route)
    } else {
        item.replacement.clone()
    };
    (label, detail)
}

/// One human line for a `task_complete` candidate: the insertable
/// `!note:block-id` (or a needs-ID marker for rows the Add block ID
/// flow must resolve first) plus task text, locator, picker group or
/// today placement, and any guard reason.
pub(super) fn task_complete_line(
    item: &TaskCompleteCandidate,
) -> (String, String) {
    let mut detail =
        format!("[{}] {}  · {}", item.status_symbol, item.text, item.locator);
    match item.today.as_ref() {
        Some(today) => {
            detail.push_str(&format!("  · today {}", role_label(today.role)));
            if let Some(name) = today
                .pomodoro
                .as_ref()
                .and_then(|pomodoro| pomodoro.name.as_ref())
            {
                detail.push_str(&format!(" ({name})"));
            }
            if today.sessions >= 2 {
                detail.push_str(&format!(" · {} sessions", today.sessions));
            }
        }
        None => {
            detail.push_str(&format!("  · {}", group_label(item.group)));
        }
    }
    if let Some(reason) = item.disabled_reason.as_deref() {
        detail.push_str(&format!("  · {reason}"));
    } else if item.requires_block_id {
        match item.block_id_suggestions.first() {
            Some(suggestion) => {
                detail.push_str(&format!(" · needs ID (^{suggestion})"))
            }
            None => detail.push_str(" · needs ID"),
        }
    } else if item.already_selected {
        detail.push_str(" · already in this draft");
    }
    if let Some(scheduled) = &item.scheduled {
        detail.push_str(&format!("  · scheduled {scheduled}"));
    }
    let label = if item.replacement.is_empty() {
        "…".to_string()
    } else {
        item.replacement.clone()
    };
    (label, detail)
}

/// One human line for a `task_dependency` candidate: the insertable
/// replacement (or a needs-ID marker for rows the Add block ID flow must
/// resolve first) plus task text, locator, and any guard reason.
pub(super) fn dependency_line(item: &DependencyCandidate) -> (String, String) {
    let mut detail =
        format!("[{}] {}  · {}", item.status_symbol, item.text, item.locator);
    if let Some(reason) = item.disabled_reason.as_deref() {
        detail.push_str(&format!("  · {reason}"));
    } else if item.requires_block_id {
        match item.block_id_suggestions.first() {
            Some(suggestion) => {
                detail.push_str(&format!(" · needs ID (^{suggestion})"))
            }
            None => detail.push_str(" · needs ID"),
        }
    } else if item.already_dependency {
        detail.push_str(" · already added");
    }
    let label = if item.replacement.is_empty() {
        "…".to_string()
    } else {
        item.replacement.clone()
    };
    (label, detail)
}

pub(super) fn candidate_lines(
    candidates: &Candidates,
    context: Option<CompletionContext>,
) -> Vec<(String, String)> {
    match candidates {
        Candidates::Route(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    format!("{}  {:?}", item.label, item.kind),
                )
            })
            .collect(),
        Candidates::Section(items) => items
            .iter()
            .map(|item| (item.replacement.clone(), format!("H{}", item.level)))
            .collect(),
        Candidates::Task(items) => items
            .iter()
            .map(|item| {
                let label = if item.requires_block_id {
                    "needs id".to_string()
                } else {
                    item.replacement.clone()
                };
                let detail = match item.pomodoro.as_ref() {
                    Some(pomodoro) => pomodoro.name.clone().map_or_else(
                        || format!("{}  · Planned", item.text),
                        |name| format!("{}  · {name}", item.text),
                    ),
                    None => item.text.clone(),
                };
                (label, detail)
            })
            .collect(),
        Candidates::TaskSection(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    format!("{}  {} items", item.title, item.child_count),
                )
            })
            .collect(),
        Candidates::ActiveTask(items) => items
            .iter()
            .map(|item| {
                let queue = match item.pomodoro.as_ref() {
                    Some(pomodoro) => pomodoro
                        .name
                        .clone()
                        .unwrap_or_else(|| "Planned".to_string()),
                    None => "Not queued".to_string(),
                };
                (
                    item.replacement.clone(),
                    format!(
                        "[{}] {}  · {}",
                        item.status_symbol, item.text, queue
                    ),
                )
            })
            .collect(),
        Candidates::TaskLink(items) => {
            items.iter().map(task_link_line).collect()
        }
        Candidates::TaskParent(items) => {
            items.iter().map(task_parent_line).collect()
        }
        Candidates::Dependency(items) => {
            items.iter().map(dependency_line).collect()
        }
        Candidates::TaskComplete(items) => {
            items.iter().map(task_complete_line).collect()
        }
        Candidates::PomodoroName(items) => items
            .iter()
            .map(|item| {
                if context == Some(CompletionContext::PomodoroStartName) {
                    return pomodoro_start_name_line(item);
                }
                let name =
                    item.name.clone().unwrap_or_else(|| "unnamed".to_string());
                let slug = if item.replacement.is_empty() {
                    "-"
                } else {
                    &item.replacement
                };
                let time = item.time_range.as_deref().unwrap_or("planned");
                let badges = pomodoro_name_badges(item).join(" ");
                let detail = if badges.is_empty() {
                    format!("{slug}  {time}")
                } else {
                    format!("{slug}  {time}  {badges}")
                };
                (name, detail)
            })
            .collect(),
        Candidates::WikilinkNote(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    item.alias.as_ref().map_or_else(
                        || item.path.clone(),
                        |alias| format!("{}  alias {alias}", item.path),
                    ),
                )
            })
            .collect(),
        Candidates::WikilinkHeading(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    format!("{}  H{}", item.path, item.level),
                )
            })
            .collect(),
        Candidates::WikilinkBlock(items) => items
            .iter()
            .map(|item| {
                (
                    item.replacement.clone(),
                    item.preview.as_ref().map_or_else(
                        || item.path.clone(),
                        |preview| format!("{}  {}", item.path, preview),
                    ),
                )
            })
            .collect(),
    }
}

/// Human row for one `pomodoro_start_name` candidate: the visible name
/// plus the row-kind label (start/next-up, new, again, name-it, running)
/// with its link count or time range.
pub(super) fn pomodoro_start_name_line(
    item: &PomodoroNameCandidate,
) -> (String, String) {
    let name = item.name.clone().unwrap_or_else(|| "unnamed".to_string());
    let links = match item.child_count {
        0 => "Empty".to_string(),
        1 => "1 link".to_string(),
        count => format!("{count} links"),
    };
    let detail = if item.requires_name {
        format!("name it · {links}")
    } else if item.creates_pomodoro && item.state == PomodoroState::Completed {
        match item.time_range.as_deref() {
            Some(range) => format!("again · last {range}"),
            None => "again".to_string(),
        }
    } else if item.creates_pomodoro {
        "new session".to_string()
    } else if item.time_range.is_some() {
        format!("running {}", item.time_range.as_deref().unwrap_or(""))
    } else if item.next_up {
        format!("next up · {links}")
    } else {
        format!("planned · {links}")
    };
    (name, detail)
}

pub(super) fn pomodoro_name_badges(
    item: &PomodoroNameCandidate,
) -> Vec<String> {
    let mut badges = Vec::new();
    if item.is_current {
        badges.push("current".to_string());
    }
    if item.match_count > 1 {
        badges.push(format!("{} matches", item.match_count));
    }
    if item.requires_name {
        badges.push("name it".to_string());
    }
    if item.creates_pomodoro {
        badges.push("create".to_string());
    }
    badges
}

pub(super) fn context_label(context: CompletionContext) -> &'static str {
    match context {
        CompletionContext::Route => "route",
        CompletionContext::Section => "section",
        CompletionContext::PomodoroBlockId => "pomodoro_block_id",
        CompletionContext::TaskBlockId => "task_block_id",
        CompletionContext::ProjectTaskBlockId => "project_task_block_id",
        CompletionContext::PomodoroName => "pomodoro_name",
        CompletionContext::PomodoroStartName => "pomodoro_start_name",
        CompletionContext::Task => "task",
        CompletionContext::TaskSection => "task_section",
        CompletionContext::ActiveTask => "active_task",
        CompletionContext::TaskLink => "task_link",
        CompletionContext::TaskParent => "task_parent",
        CompletionContext::TaskDependency => "task_dependency",
        CompletionContext::TaskComplete => "task_complete",
        CompletionContext::WikilinkNote => "wikilink_note",
        CompletionContext::WikilinkHeading => "wikilink_heading",
        CompletionContext::WikilinkBlock => "wikilink_block",
    }
}

pub(super) fn success_json(result: &CaptureCompleteResult) -> String {
    serde_json::to_string(result).expect("serialize capture complete result")
}

pub(super) fn print_error(
    error: &CompleteError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}
