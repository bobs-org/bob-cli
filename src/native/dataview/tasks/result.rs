use std::{cmp::Ordering, collections::HashSet};

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use serde::Serialize;

use super::{
    js::JsSandbox,
    parse::{
        GroupInstruction, GroupKey, Instruction, QueryAst, SortInstruction,
        SortKey, StatementSource,
    },
    task::{StatusType, Task, TaskDate},
};
use crate::native::dataview::DataviewError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskResult {
    pub(super) count: usize,
    pub(super) count_before_limit: usize,
    pub(super) count_text: String,
    pub(super) tasks: Vec<Task>,
    pub(super) groups: Vec<TaskGroup>,
    pub(super) explanation: Option<String>,
    #[serde(skip)]
    pub(super) tree_tasks: Vec<Task>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskGroup {
    pub(super) names: Vec<String>,
    pub(super) headings: Vec<GroupHeading>,
    pub(super) count: usize,
    pub(super) count_before_limit: usize,
    pub(super) count_text: String,
    pub(super) tasks: Vec<Task>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GroupHeading {
    pub(super) level: usize,
    pub(super) name: String,
}

impl TaskResult {
    pub(super) fn paths(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.groups
            .iter()
            .flat_map(|group| &group.tasks)
            .filter_map(|task| {
                seen.insert(task.path.clone()).then(|| task.path.clone())
            })
            .collect()
    }
}

pub(super) fn build(
    query: &QueryAst,
    mut tasks: Vec<Task>,
    tree_tasks: Vec<Task>,
    now: NaiveDateTime,
    global_filter: &str,
    javascript: &mut JsSandbox,
) -> Result<TaskResult, DataviewError> {
    sort_tasks(&mut tasks, &query.sorting, now, javascript)?;
    let count_before_limit = tasks.len();
    if let Some(limit) = query.limit {
        tasks.truncate(limit);
    }

    let mut groups = group_tasks(tasks, &query.grouping, javascript);
    if let Some(limit) = query.limit_groups
        && !query.grouping.is_empty()
    {
        for group in &mut groups {
            group.count_before_limit = group.tasks.len();
            group.tasks.truncate(limit);
            group.count = group.tasks.len();
            group.count_text =
                task_count_text(group.count, group.count_before_limit);
        }
    }
    set_minimal_headings(&mut groups);

    let mut seen = HashSet::new();
    let tasks = groups
        .iter()
        .flat_map(|group| &group.tasks)
        .filter_map(|task| {
            let key = (task.path.clone(), task.line_number);
            seen.insert(key).then(|| task.clone())
        })
        .collect::<Vec<_>>();
    let count = tasks.len();
    let count_text = task_count_text(count, count_before_limit);
    let explanation = query.explain.then(|| explain(query, global_filter));

    Ok(TaskResult {
        count,
        count_before_limit,
        count_text,
        tasks,
        groups,
        explanation,
        tree_tasks,
    })
}

fn sort_tasks(
    tasks: &mut [Task],
    instructions: &[SortInstruction],
    now: NaiveDateTime,
    javascript: &mut JsSandbox,
) -> Result<(), DataviewError> {
    javascript.validate_function_sorts(instructions)?;
    let defaults = [
        SortKey::StatusType,
        SortKey::Urgency,
        SortKey::Due,
        SortKey::Priority,
        SortKey::Path,
    ];
    let mut error = None;
    tasks.sort_by(|left, right| {
        if error.is_some() {
            return Ordering::Equal;
        }
        for instruction in instructions {
            let ordering = if instruction.key == SortKey::Function {
                javascript.compare_function_sort(
                    instruction.function.as_deref().unwrap_or_default(),
                    left,
                    right,
                )
            } else {
                Ok(compare_key(
                    instruction.key,
                    instruction.tag_index,
                    left,
                    right,
                    now,
                ))
            };
            match ordering {
                Ok(Ordering::Equal) => {}
                Ok(ordering) => {
                    return if instruction.reverse {
                        ordering.reverse()
                    } else {
                        ordering
                    };
                }
                Err(sort_error) => {
                    error = Some(sort_error);
                    return Ordering::Equal;
                }
            }
        }
        for key in defaults {
            let ordering = compare_key(key, None, left, right, now);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    });
    error.map_or(Ok(()), Err)
}

fn compare_key(
    key: SortKey,
    tag_index: Option<usize>,
    left: &Task,
    right: &Task,
    now: NaiveDateTime,
) -> Ordering {
    match key {
        SortKey::Cancelled => compare_dates(&left.cancelled, &right.cancelled),
        SortKey::Created => compare_dates(&left.created, &right.created),
        SortKey::Description => natural_compare(
            &clean_description(&left.display_description),
            &clean_description(&right.display_description),
        ),
        SortKey::Done => compare_dates(&left.done, &right.done),
        SortKey::Due => compare_dates(&left.due, &right.due),
        SortKey::Filename => {
            natural_compare(&left.file.filename, &right.file.filename)
        }
        SortKey::Function => Ordering::Equal,
        SortKey::Happens => {
            compare_optional_dates(happens(left), happens(right))
        }
        SortKey::Heading => natural_compare(
            left.heading.as_deref().unwrap_or_default(),
            right.heading.as_deref().unwrap_or_default(),
        ),
        SortKey::Id => natural_compare(&left.id, &right.id),
        SortKey::Path => natural_compare(&left.path, &right.path),
        SortKey::Priority => left.priority_number.cmp(&right.priority_number),
        SortKey::Random => random_key(left, now).cmp(&random_key(right, now)),
        SortKey::Recurring => right.is_recurring.cmp(&left.is_recurring),
        SortKey::Scheduled => compare_dates(&left.scheduled, &right.scheduled),
        SortKey::Start => compare_dates(&left.start, &right.start),
        SortKey::Status => left.is_done.cmp(&right.is_done),
        SortKey::StatusName => {
            natural_compare(&left.status.name, &right.status.name)
        }
        SortKey::StatusType => status_type_order(left.status.status_type)
            .cmp(&status_type_order(right.status.status_type)),
        SortKey::Tag => compare_tags(tag_index.unwrap_or(1), left, right),
        SortKey::Urgency => right.urgency.total_cmp(&left.urgency),
    }
}

fn compare_dates(
    left: &Option<TaskDate>,
    right: &Option<TaskDate>,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            match (left.valid_date(), right.valid_date()) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => Ordering::Equal,
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_optional_dates(
    left: Option<NaiveDate>,
    right: Option<NaiveDate>,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_tags(index: usize, left: &Task, right: &Task) -> Ordering {
    match (left.tags.get(index - 1), right.tags.get(index - 1)) {
        (Some(left), Some(right)) => natural_compare(left, right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn group_tasks(
    tasks: Vec<Task>,
    instructions: &[GroupInstruction],
    javascript: &mut JsSandbox,
) -> Vec<TaskGroup> {
    if instructions.is_empty() {
        let count = tasks.len();
        return vec![TaskGroup {
            names: Vec::new(),
            headings: Vec::new(),
            count,
            count_before_limit: count,
            count_text: task_count_text(count, count),
            tasks,
        }];
    }

    let mut groups = Vec::<TaskGroup>::new();
    for task in tasks {
        let mut names = vec![Vec::<String>::new()];
        for instruction in instructions {
            let keys = group_keys(instruction, &task, javascript);
            let mut expanded = Vec::new();
            for prefix in names {
                for key in &keys {
                    let mut path = prefix.clone();
                    path.push(key.clone());
                    expanded.push(path);
                }
            }
            names = expanded;
        }
        for name_path in names {
            if let Some(group) =
                groups.iter_mut().find(|group| group.names == name_path)
            {
                group.tasks.push(task.clone());
            } else {
                groups.push(TaskGroup {
                    names: name_path,
                    headings: Vec::new(),
                    count: 1,
                    count_before_limit: 1,
                    count_text: "1 task".to_string(),
                    tasks: vec![task.clone()],
                });
            }
        }
    }
    groups.sort_by(|left, right| {
        for (index, instruction) in instructions.iter().enumerate() {
            let ordering =
                natural_compare(&left.names[index], &right.names[index]);
            if ordering != Ordering::Equal {
                let reverse = if instruction.key == GroupKey::Urgency {
                    !instruction.reverse
                } else {
                    instruction.reverse
                };
                return if reverse {
                    ordering.reverse()
                } else {
                    ordering
                };
            }
        }
        Ordering::Equal
    });
    for group in &mut groups {
        group.count = group.tasks.len();
        group.count_before_limit = group.count;
        group.count_text = task_count_text(group.count, group.count);
    }
    groups
}

fn group_keys(
    instruction: &GroupInstruction,
    task: &Task,
    javascript: &mut JsSandbox,
) -> Vec<String> {
    let one = |value: String| vec![value];
    match instruction.key {
        GroupKey::Backlink => one(backlink(task)),
        GroupKey::Cancelled => one(date_group("cancelled", &task.cancelled)),
        GroupKey::Created => one(date_group("created", &task.created)),
        GroupKey::Done => one(date_group("done", &task.done)),
        GroupKey::Due => one(date_group("due", &task.due)),
        GroupKey::Filename => {
            one(format!("[[{}]]", task.file.filename_without_extension))
        }
        GroupKey::Folder => one(escape_markdown(&task.file.folder)),
        GroupKey::Function => javascript.function_group_keys(
            instruction.function.as_deref().unwrap_or_default(),
            task,
        ),
        GroupKey::Happens => one(happens(task).map_or_else(
            || "No happens date".to_string(),
            format_date_heading,
        )),
        GroupKey::Heading => one(task
            .heading
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "(No heading)".to_string())),
        GroupKey::Id => one(task.id.clone()),
        GroupKey::Path => {
            one(escape_markdown(&task.file.path_without_extension))
        }
        GroupKey::Priority => one(priority_group(task.priority_number)),
        GroupKey::Recurrence => one(if task.recurrence_rule.is_empty() {
            "None".to_string()
        } else {
            task.recurrence_rule.clone()
        }),
        GroupKey::Recurring => one(if task.is_recurring {
            "Recurring"
        } else {
            "Not Recurring"
        }
        .to_string()),
        GroupKey::Root => one(escape_markdown(&task.file.root)),
        GroupKey::Scheduled => one(date_group("scheduled", &task.scheduled)),
        GroupKey::Start => one(date_group("start", &task.start)),
        GroupKey::Status => {
            one(if task.is_done { "Done" } else { "Todo" }.to_string())
        }
        GroupKey::StatusName => one(task.status.name.clone()),
        GroupKey::StatusType => one(status_type_group(task.status.status_type)),
        GroupKey::Tags => {
            if task.tags.is_empty() {
                one("(No tags)".to_string())
            } else {
                task.tags.clone()
            }
        }
        GroupKey::Urgency => one(format!("{:.2}", task.urgency)),
    }
}

fn set_minimal_headings(groups: &mut [TaskGroup]) {
    let mut previous: Option<Vec<String>> = None;
    for group in groups {
        let first_changed = previous
            .as_ref()
            .and_then(|previous| {
                group.names.iter().zip(previous).position(|(a, b)| a != b)
            })
            .unwrap_or(0);
        group.headings = group
            .names
            .iter()
            .enumerate()
            .skip(first_changed)
            .map(|(level, name)| GroupHeading {
                level,
                name: name.clone(),
            })
            .collect();
        previous = Some(group.names.clone());
    }
}

fn explain(query: &QueryAst, global_filter: &str) -> String {
    let mut sections = Vec::new();
    if !global_filter.is_empty() {
        sections.push(format!(
            "Only tasks containing the global filter '{global_filter}'."
        ));
    }

    for (source, label) in [
        (StatementSource::GlobalQuery, "Explanation of the global query"),
        (
            StatementSource::QueryFileDefaults,
            "Explanation of the Query File Defaults (from properties/frontmatter in the query's file)",
        ),
        (
            StatementSource::Query,
            "Explanation of this Tasks code block query",
        ),
    ] {
        let statements = query
            .statements
            .iter()
            .filter(|statement| statement.source == source)
            .filter_map(|statement| match &statement.parsed {
                Instruction::Comment | Instruction::Explain => None,
                Instruction::Limit { count } => Some(format!(
                    "At most {count} task{}.",
                    if *count == 1 { "" } else { "s" }
                )),
                Instruction::LimitGroups { count } => Some(format!(
                    "At most {count} task{} per group (if any \"group by\" options are supplied).",
                    if *count == 1 { "" } else { "s" }
                )),
                _ => Some(statement.instruction.clone()),
            })
            .collect::<Vec<_>>();
        if !statements.is_empty() {
            sections.push(format!(
                "{label}:\n\n{}",
                statements
                    .into_iter()
                    .map(|statement| format!("  {statement}"))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            ));
        }
    }
    sections.join("\n\n")
}

fn task_count_text(count: usize, before: usize) -> String {
    let noun = if before == 1 { "task" } else { "tasks" };
    if count == before {
        format!("{count} {noun}")
    } else {
        format!("{count} of {before} {noun}")
    }
}

fn date_group(name: &str, date: &Option<TaskDate>) -> String {
    match date {
        None => format!("No {name} date"),
        Some(date) => date.valid_date().map_or_else(
            || format!("%%0%% Invalid {name} date"),
            format_date_heading,
        ),
    }
}

fn format_date_heading(date: NaiveDate) -> String {
    format!("{} {}", date.format("%Y-%m-%d"), date.format("%A"))
}

fn happens(task: &Task) -> Option<NaiveDate> {
    [&task.start, &task.scheduled, &task.due]
        .into_iter()
        .flatten()
        .filter_map(TaskDate::valid_date)
        .min()
}

fn status_type_order(status: StatusType) -> u8 {
    match status {
        StatusType::InProgress => 1,
        StatusType::Todo => 2,
        StatusType::OnHold => 3,
        StatusType::Done => 4,
        StatusType::Cancelled => 5,
        StatusType::NonTask => 6,
        StatusType::Empty => 7,
    }
}

fn status_type_group(status: StatusType) -> String {
    format!("%%{}%%{}", status_type_order(status), status.as_str())
}

fn priority_group(priority: u8) -> String {
    match priority {
        0 => "%%0%%Highest priority",
        1 => "%%1%%High priority",
        2 => "%%2%%Medium priority",
        3 => "%%3%%Normal priority",
        4 => "%%4%%Low priority",
        _ => "%%5%%Lowest priority",
    }
    .to_string()
}

fn backlink(task: &Task) -> String {
    let filename = &task.file.filename_without_extension;
    task.heading.as_ref().map_or_else(
        || format!("[[{filename}]]"),
        |heading| format!("[[{filename}#{heading}|{filename} > {heading}]]"),
    )
}

fn escape_markdown(value: &str) -> String {
    value.replace('\\', "\\\\").replace('_', "\\_")
}

fn clean_description(value: &str) -> String {
    let mut value = value.to_string();
    if let Some(rest) = value.strip_prefix("[[")
        && let Some(end) = rest.find("]]")
    {
        let link = &rest[..end];
        let visible = link.split_once('|').map_or(link, |(_, visible)| visible);
        value = format!("{visible}{}", &rest[end + 2..]);
    }
    for (open, close) in [
        ("**", "**"),
        ("*", "*"),
        ("==", "=="),
        ("__", "__"),
        ("_", "_"),
    ] {
        if let Some(rest) = value.strip_prefix(open)
            && let Some(end) = rest.find(close)
        {
            value = format!("{}{}", &rest[..end], &rest[end + close.len()..]);
        }
    }
    value
}

fn random_key(task: &Task, now: NaiveDateTime) -> i32 {
    let input = format!(
        "{}-{:02}-{:02} {}",
        now.year(),
        now.month(),
        now.day(),
        task.description
    );
    let mut hash = 9_i32;
    for unit in input.encode_utf16() {
        hash = (hash ^ i32::from(unit)).wrapping_mul(9_i32.pow(9));
    }
    hash ^ ((hash as u32 >> 9) as i32)
}

fn natural_compare(left: &str, right: &str) -> Ordering {
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    loop {
        match (left.peek(), right.peek()) {
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let a = take_digits(&mut left);
                let b = take_digits(&mut right);
                let ordering = a
                    .trim_start_matches('0')
                    .len()
                    .cmp(&b.trim_start_matches('0').len())
                    .then_with(|| {
                        a.trim_start_matches('0').cmp(b.trim_start_matches('0'))
                    })
                    .then_with(|| a.len().cmp(&b.len()));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(a), Some(b)) => {
                let ordering = a.cmp(b);
                left.next();
                right.next();
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

fn take_digits(iter: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut result = String::new();
    while iter.peek().is_some_and(char::is_ascii_digit) {
        result.push(iter.next().expect("peeked digit"));
    }
    result
}
