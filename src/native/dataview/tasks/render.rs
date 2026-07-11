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
    global_filter: &str,
) -> String {
    // Static CLI output has no toolbar or edit/postpone controls to render.
    // Their parsed toggles remain visible in the JSON query metadata.
    let mut lines = Vec::new();
    if let Some(explanation) = &result.explanation
        && !explanation.is_empty()
    {
        lines.extend(explanation.lines().map(str::to_string));
        lines.push(String::new());
    }

    for group in &result.groups {
        for heading in &group.headings {
            if heading.name.is_empty() {
                continue;
            }
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
            global_filter,
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
    global_filter: &str,
) {
    if !layout.show_tree {
        lines.extend(
            tasks.iter().map(|task| {
                render_task(task, 0, layout, format, global_filter)
            }),
        );
        return;
    }

    let keys = tasks
        .iter()
        .map(|task| (task.path.as_str(), task.line_number))
        .collect::<std::collections::HashSet<_>>();
    for task in tasks {
        if has_matched_ancestor(task, all_tasks, &keys) {
            continue;
        }
        render_tree(lines, task, all_tasks, 0, layout, format, global_filter);
    }
}

fn has_matched_ancestor(
    task: &Task,
    all_tasks: &[Task],
    matched: &std::collections::HashSet<(&str, usize)>,
) -> bool {
    let mut parent = task.parent_task_line_number;
    while let Some(line_number) = parent {
        if matched.contains(&(task.path.as_str(), line_number)) {
            return true;
        }
        parent = all_tasks
            .iter()
            .find(|candidate| {
                candidate.path == task.path
                    && candidate.line_number == line_number
            })
            .and_then(|candidate| candidate.parent_task_line_number);
    }
    false
}

fn render_tree(
    lines: &mut Vec<String>,
    task: &Task,
    tasks: &[Task],
    depth: usize,
    layout: &LayoutOptions,
    format: TaskFormat,
    global_filter: &str,
) {
    lines.push(render_task(task, depth, layout, format, global_filter));
    for child_line in &task.child_task_line_numbers {
        if let Some(child) = tasks.iter().find(|candidate| {
            candidate.path == task.path && candidate.line_number == *child_line
        }) {
            render_tree(
                lines,
                child,
                tasks,
                depth + 1,
                layout,
                format,
                global_filter,
            );
        }
    }
}

fn render_task(
    task: &Task,
    depth: usize,
    layout: &LayoutOptions,
    format: TaskFormat,
    global_filter: &str,
) -> String {
    let mut description = task.display_description.clone();
    if !layout.show_tags {
        let tags = task.tags.iter().map(String::as_str).chain(
            (global_filter.starts_with('#') && !global_filter.is_empty())
                .then_some(global_filter),
        );
        let mut ranges = tags
            .flat_map(|tag| description.match_indices(tag))
            .filter_map(|(start, tag)| {
                let end = start + tag.len();
                let before = description[..start].chars().next_back();
                let after = description[end..].chars().next();
                (before.is_none_or(char::is_whitespace)
                    && after.is_none_or(char::is_whitespace))
                .then(|| {
                    if start == 0 {
                        start..end + after.map_or(0, char::len_utf8)
                    } else {
                        start
                            - before
                                .expect("non-start has a character")
                                .len_utf8()..end
                    }
                })
            })
            .collect::<Vec<_>>();
        ranges.sort_by_key(|range| range.start);
        ranges.dedup();
        for range in ranges.into_iter().rev() {
            description.replace_range(range, "");
        }
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
