use super::{
    parse::{LayoutOptions, QueryAst},
    result::TaskResult,
    settings::TaskFormat,
    task::{Priority, Task, TaskDate},
};

pub(super) fn markdown(
    result: &TaskResult,
    query: &QueryAst,
    format: TaskFormat,
) -> String {
    // Static CLI output has no toolbar or edit/postpone controls to render.
    // Their parsed toggles remain visible in the JSON query metadata.
    let mut lines = Vec::new();
    if let Some(explanation) = &result.explanation {
        lines.extend(explanation.lines().map(str::to_string));
        lines.push(String::new());
    }

    for group in &result.groups {
        for heading in &group.headings {
            lines.push(format!(
                "{} {}",
                "#".repeat((4 + heading.level).min(6)),
                heading.name
            ));
            lines.push(String::new());
        }
        render_group_tasks(
            &mut lines,
            &group.tasks,
            &result.tree_tasks,
            &query.layout,
            format,
        );
        if !group.tasks.is_empty() {
            lines.push(String::new());
        }
    }

    if query.layout.show_task_count {
        lines.push(result.count_text.clone());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

fn render_group_tasks(
    lines: &mut Vec<String>,
    tasks: &[Task],
    all_tasks: &[Task],
    layout: &LayoutOptions,
    format: TaskFormat,
) {
    if !layout.show_tree {
        lines.extend(
            tasks
                .iter()
                .map(|task| render_task(task, 0, layout, format)),
        );
        return;
    }

    let keys = tasks
        .iter()
        .map(|task| (task.path.as_str(), task.line_number))
        .collect::<std::collections::HashSet<_>>();
    for task in tasks {
        if task
            .parent_task_line_number
            .is_some_and(|parent| keys.contains(&(task.path.as_str(), parent)))
        {
            continue;
        }
        render_tree(lines, task, all_tasks, 0, layout, format);
    }
}

fn render_tree(
    lines: &mut Vec<String>,
    task: &Task,
    tasks: &[Task],
    depth: usize,
    layout: &LayoutOptions,
    format: TaskFormat,
) {
    lines.push(render_task(task, depth, layout, format));
    for child_line in &task.child_task_line_numbers {
        if let Some(child) = tasks.iter().find(|candidate| {
            candidate.path == task.path && candidate.line_number == *child_line
        }) {
            render_tree(lines, child, tasks, depth + 1, layout, format);
        }
    }
}

fn render_task(
    task: &Task,
    depth: usize,
    layout: &LayoutOptions,
    format: TaskFormat,
) -> String {
    let mut description = task.display_description.clone();
    if !layout.show_tags {
        description = description
            .split_whitespace()
            .filter(|word| !word.starts_with('#'))
            .collect::<Vec<_>>()
            .join(" ");
    }
    let mut components = vec![description];
    push_field(
        &mut components,
        layout.show_id,
        layout.short_mode,
        format,
        "id",
        "🆔",
        &task.id,
    );
    push_field(
        &mut components,
        layout.show_depends_on,
        layout.short_mode,
        format,
        "dependsOn",
        "⛔",
        &task.depends_on.join(", "),
    );
    if layout.show_priority && task.priority != Priority::Normal {
        components.push(match format {
            TaskFormat::Dataview => {
                format!("[priority:: {}]", priority_name(task.priority))
            }
            TaskFormat::Emoji => priority_emoji(task.priority).to_string(),
        });
    }
    push_field(
        &mut components,
        layout.show_recurrence_rule,
        layout.short_mode,
        format,
        "repeat",
        "🔁",
        &task.recurrence_rule,
    );
    push_field(
        &mut components,
        layout.show_on_completion,
        layout.short_mode,
        format,
        "onCompletion",
        "🏁",
        &task.on_completion,
    );
    push_date(
        &mut components,
        layout.show_created_date,
        layout.short_mode,
        format,
        "created",
        "➕",
        &task.created,
    );
    push_date(
        &mut components,
        layout.show_start_date,
        layout.short_mode,
        format,
        "start",
        "🛫",
        &task.start,
    );
    push_date(
        &mut components,
        layout.show_scheduled_date,
        layout.short_mode,
        format,
        "scheduled",
        "⏳",
        &task.scheduled,
    );
    push_date(
        &mut components,
        layout.show_due_date,
        layout.short_mode,
        format,
        "due",
        "📅",
        &task.due,
    );
    push_date(
        &mut components,
        layout.show_cancelled_date,
        layout.short_mode,
        format,
        "cancelled",
        "❌",
        &task.cancelled,
    );
    push_date(
        &mut components,
        layout.show_done_date,
        layout.short_mode,
        format,
        "completion",
        "✅",
        &task.done,
    );
    if let Some(block) = &task.block_id {
        components.push(format!("^{block}"));
    }
    if layout.show_urgency {
        components.push(format!("(urgency: {})", task.urgency));
    }
    if layout.show_backlink {
        components.push(if layout.short_mode {
            format!("[[{}|🔗]]", task.path)
        } else if let Some(heading) = &task.heading {
            format!(
                "([[{}#{}|{} > {}]])",
                task.path,
                heading,
                task.file.filename_without_extension,
                heading
            )
        } else {
            format!(
                "([[{}|{}]])",
                task.path, task.file.filename_without_extension
            )
        });
    }
    format!(
        "{}- [{}] {}",
        "    ".repeat(depth),
        task.status.symbol,
        components
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn push_field(
    components: &mut Vec<String>,
    shown: bool,
    short: bool,
    format: TaskFormat,
    name: &str,
    emoji: &str,
    value: &str,
) {
    if !shown || value.is_empty() {
        return;
    }
    components.push(match format {
        TaskFormat::Dataview if short => format!("[{name}::]"),
        TaskFormat::Dataview => format!("[{name}:: {value}]"),
        TaskFormat::Emoji if short => emoji.to_string(),
        TaskFormat::Emoji => format!("{emoji} {value}"),
    });
}

fn push_date(
    components: &mut Vec<String>,
    shown: bool,
    short: bool,
    format: TaskFormat,
    name: &str,
    emoji: &str,
    date: &Option<TaskDate>,
) {
    if let Some(date) = date {
        push_field(components, shown, short, format, name, emoji, &date.raw);
    }
}

fn priority_name(priority: Priority) -> &'static str {
    match priority {
        Priority::Highest => "highest",
        Priority::High => "high",
        Priority::Medium => "medium",
        Priority::Normal => "normal",
        Priority::Low => "low",
        Priority::Lowest => "lowest",
    }
}

fn priority_emoji(priority: Priority) -> &'static str {
    match priority {
        Priority::Highest => "🔺",
        Priority::High => "⏫",
        Priority::Medium => "🔼",
        Priority::Normal => "",
        Priority::Low => "🔽",
        Priority::Lowest => "⏬",
    }
}
