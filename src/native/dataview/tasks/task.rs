use std::sync::LazyLock;

use chrono::{NaiveDate, NaiveDateTime};
use regex::Regex;
use serde::Serialize;

use super::settings::{StatusSettings, TaskFormat, TasksSettings};

static BLOCK_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r" \^([a-zA-Z0-9-]+)$").expect("valid block-link regex")
});
static DATE_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("valid date regex")
});
static ID_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9_-]+$").expect("valid task-id regex")
});
static DEPENDS_ON_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9_-]+(?: *, *[a-zA-Z0-9_-]+ *)*$")
        .expect("valid depends-on regex")
});
static RECURRENCE_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9, !]+$").expect("valid recurrence regex")
});
static LETTERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z]+$").expect("valid letters regex"));
static HASH_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|\s)(#[^ !@#$%^&*(),.?\":{}|<>]+)"#)
        .expect("valid hashtag regex")
});
static HASH_TAG_AT_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|\s)(#[^ !@#$%^&*(),.?\":{}|<>]+)$"#)
        .expect("valid trailing hashtag regex")
});

static EMOJI_PRIORITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(🔺|⏫|🔼|🔽|⏬)\u{FE0F}?$")
        .expect("valid emoji priority regex")
});
static EMOJI_START: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("🛫", r"(\d{4}-\d{2}-\d{2})"));
static EMOJI_CREATED: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("➕", r"(\d{4}-\d{2}-\d{2})"));
static EMOJI_SCHEDULED: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("(?:⏳|⌛)", r"(\d{4}-\d{2}-\d{2})"));
static EMOJI_DUE: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("(?:📅|📆|🗓)", r"(\d{4}-\d{2}-\d{2})"));
static EMOJI_DONE: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("✅", r"(\d{4}-\d{2}-\d{2})"));
static EMOJI_CANCELLED: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("❌", r"(\d{4}-\d{2}-\d{2})"));
static EMOJI_RECURRENCE: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("🔁", r"([a-zA-Z0-9, !]+)"));
static EMOJI_ON_COMPLETION: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("🏁", r"([a-zA-Z]+)"));
static EMOJI_ID: LazyLock<Regex> =
    LazyLock::new(|| emoji_value_regex("🆔", r"([a-zA-Z0-9_-]+)"));
static EMOJI_DEPENDS_ON: LazyLock<Regex> = LazyLock::new(|| {
    emoji_value_regex("⛔", r"([a-zA-Z0-9_-]+(?: *, *[a-zA-Z0-9_-]+ *)*)")
});

fn emoji_value_regex(symbol: &str, value: &str) -> Regex {
    Regex::new(&format!(r"{symbol}\u{{FE0F}}? *{value}$"))
        .expect("valid emoji task-field regex")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum StatusType {
    Todo,
    Done,
    InProgress,
    OnHold,
    Cancelled,
    NonTask,
    Empty,
}

impl StatusType {
    fn from_settings(value: &str) -> Self {
        match value {
            "DONE" => Self::Done,
            "IN_PROGRESS" => Self::InProgress,
            "ON_HOLD" => Self::OnHold,
            "CANCELLED" => Self::Cancelled,
            "NON_TASK" => Self::NonTask,
            "EMPTY" => Self::Empty,
            _ => Self::Todo,
        }
    }

    pub(super) fn is_done(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::NonTask)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Status {
    pub(super) symbol: String,
    pub(super) name: String,
    pub(super) next_symbol: String,
    pub(super) available_as_command: bool,
    #[serde(rename = "type")]
    pub(super) status_type: StatusType,
}

#[derive(Debug)]
pub(super) struct StatusRegistry {
    statuses: Vec<Status>,
}

impl StatusRegistry {
    pub(super) fn new(settings: &StatusSettings) -> Self {
        let mut statuses = Vec::new();
        for configured in settings
            .core_statuses
            .iter()
            .chain(&settings.custom_statuses)
        {
            if statuses
                .iter()
                .any(|status: &Status| status.symbol == configured.symbol)
            {
                continue;
            }
            statuses.push(Status {
                symbol: configured.symbol.clone(),
                name: configured.name.clone(),
                next_symbol: configured.next_status_symbol.clone(),
                available_as_command: configured.available_as_command,
                status_type: StatusType::from_settings(&configured.status_type),
            });
        }
        Self { statuses }
    }

    pub(super) fn by_symbol_or_create(&self, symbol: char) -> Status {
        let symbol = symbol.to_string();
        self.statuses
            .iter()
            .find(|status| status.symbol == symbol)
            .cloned()
            .unwrap_or_else(|| Status {
                symbol,
                name: "Unknown".to_string(),
                next_symbol: "x".to_string(),
                available_as_command: false,
                status_type: StatusType::Todo,
            })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(super) enum Priority {
    Highest,
    High,
    Medium,
    #[default]
    Normal,
    Low,
    Lowest,
}

impl Priority {
    pub(super) fn number(self) -> u8 {
        match self {
            Self::Highest => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Normal => 3,
            Self::Low => 4,
            Self::Lowest => 5,
        }
    }

    fn urgency(self) -> f64 {
        match self {
            Self::Highest => 9.0,
            Self::High => 6.0,
            Self::Medium => 3.9,
            Self::Normal => 1.95,
            Self::Low => 0.0,
            Self::Lowest => -1.8,
        }
    }

    fn from_name(value: &str) -> Option<Self> {
        match value {
            "highest" => Some(Self::Highest),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            "lowest" => Some(Self::Lowest),
            _ => None,
        }
    }

    fn from_emoji(value: &str) -> Option<Self> {
        match value {
            "🔺" => Some(Self::Highest),
            "⏫" => Some(Self::High),
            "🔼" => Some(Self::Medium),
            "🔽" => Some(Self::Low),
            "⏬" => Some(Self::Lowest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskDate {
    pub(super) raw: String,
    pub(super) value: Option<String>,
    pub(super) valid: bool,
    #[serde(skip)]
    date: Option<NaiveDate>,
}

impl TaskDate {
    fn parse(raw: &str) -> Self {
        let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok();
        Self {
            raw: raw.to_string(),
            value: date.map(|date| date.format("%Y-%m-%d").to_string()),
            valid: date.is_some(),
            date,
        }
    }

    fn valid_date(&self) -> Option<NaiveDate> {
        self.date
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskFile {
    pub(super) path: String,
    pub(super) path_without_extension: String,
    pub(super) root: String,
    pub(super) folder: String,
    pub(super) filename: String,
    pub(super) filename_without_extension: String,
}

impl TaskFile {
    pub(super) fn new(path: String) -> Self {
        let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
        let filename_without_extension = filename
            .strip_suffix(".md")
            .unwrap_or(&filename)
            .to_string();
        let path_without_extension =
            path.strip_suffix(".md").unwrap_or(&path).to_string();
        let folder = path
            .strip_suffix(&filename)
            .filter(|value| !value.is_empty())
            .unwrap_or("/")
            .to_string();
        let root = path
            .split_once('/')
            .map_or_else(|| "/".to_string(), |(root, _)| format!("{root}/"));
        Self {
            path,
            path_without_extension,
            root,
            folder,
            filename,
            filename_without_extension,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Task {
    pub(super) file: TaskFile,
    pub(super) path: String,
    pub(super) line_number: usize,
    pub(super) heading: Option<String>,
    pub(super) has_heading: bool,
    pub(super) indentation: String,
    pub(super) list_marker: String,
    pub(super) original_markdown: String,
    pub(super) text: String,
    pub(super) description: String,
    pub(super) display_description: String,
    pub(super) description_without_tags: String,
    pub(super) status: Status,
    pub(super) is_done: bool,
    pub(super) priority: Priority,
    pub(super) priority_number: u8,
    pub(super) created: Option<TaskDate>,
    pub(super) start: Option<TaskDate>,
    pub(super) scheduled: Option<TaskDate>,
    pub(super) due: Option<TaskDate>,
    pub(super) done: Option<TaskDate>,
    pub(super) cancelled: Option<TaskDate>,
    pub(super) recurrence_rule: String,
    pub(super) is_recurring: bool,
    pub(super) on_completion: String,
    pub(super) id: String,
    pub(super) depends_on: Vec<String>,
    pub(super) tags: Vec<String>,
    pub(super) block_id: Option<String>,
    pub(super) parent_line_number: Option<usize>,
    pub(super) parent_task_line_number: Option<usize>,
    pub(super) child_line_numbers: Vec<usize>,
    pub(super) child_task_line_numbers: Vec<usize>,
    pub(super) root_line_number: usize,
    pub(super) is_root: bool,
    pub(super) is_blocked: bool,
    pub(super) is_blocking: bool,
    pub(super) urgency: f64,
}

impl Task {
    pub(super) fn from_line(
        line: &str,
        file: TaskFile,
        line_number: usize,
        heading: Option<String>,
        settings: &TasksSettings,
        statuses: &StatusRegistry,
        now: NaiveDateTime,
    ) -> Option<Self> {
        let components = parse_task_line(line)?;
        let mut body = components.body.trim().to_string();
        let text = body.clone();
        let block_match = BLOCK_LINK.captures(&body).and_then(|captures| {
            Some((
                captures.get(0)?.start(),
                captures.get(1)?.as_str().to_string(),
            ))
        });
        let block_id = block_match.map(|(start, id)| {
            body = body[..start].trim().to_string();
            id
        });

        if !body.contains(&settings.global_filter) {
            return None;
        }

        let mut details = parse_details(&body, settings.task_format);
        details.tags.retain(|tag| tag != &settings.global_filter);
        let display_description = if settings.remove_global_filter {
            remove_global_filter_as_word(
                &details.description,
                &settings.global_filter,
            )
        } else {
            details.description.clone()
        };
        let description_without_tags = HASH_TAGS
            .replace_all(&details.description, "")
            .trim()
            .to_string();
        let recurrence_rule = details
            .recurrence_source
            .as_deref()
            .and_then(|rule| {
                let reference =
                    [&details.due, &details.scheduled, &details.start]
                        .into_iter()
                        .flatten()
                        .next();
                if reference.is_some_and(|date| !date.valid) {
                    return None;
                }
                normalize_recurrence(rule)
            })
            .unwrap_or_default();
        let status = statuses.by_symbol_or_create(components.status);
        let is_done = status.status_type.is_done();
        let priority_number = details.priority.number();
        let urgency = calculate_urgency(&details, now);
        let has_heading = heading.is_some();
        let path = file.path.clone();

        Some(Self {
            file,
            path,
            line_number,
            heading,
            has_heading,
            indentation: components.indentation,
            list_marker: components.list_marker,
            original_markdown: line.to_string(),
            text,
            description: details.description,
            display_description,
            description_without_tags,
            status,
            is_done,
            priority: details.priority,
            priority_number,
            created: details.created,
            start: details.start,
            scheduled: details.scheduled,
            due: details.due,
            done: details.done,
            cancelled: details.cancelled,
            recurrence_rule: recurrence_rule.clone(),
            is_recurring: !recurrence_rule.is_empty(),
            on_completion: details.on_completion,
            id: details.id,
            depends_on: details.depends_on,
            tags: details.tags,
            block_id,
            parent_line_number: None,
            parent_task_line_number: None,
            child_line_numbers: Vec::new(),
            child_task_line_numbers: Vec::new(),
            root_line_number: line_number,
            is_root: true,
            is_blocked: false,
            is_blocking: false,
            urgency,
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct ListLine {
    pub(super) indentation: String,
    pub(super) list_marker: String,
    pub(super) body: String,
    pub(super) depth: usize,
}

pub(super) fn parse_list_line(line: &str) -> Option<ListLine> {
    let prefix_end = line
        .char_indices()
        .take_while(|(_, value)| value.is_whitespace() || *value == '>')
        .map(|(index, value)| index + value.len_utf8())
        .last()
        .unwrap_or(0);
    let indentation = &line[..prefix_end];
    let rest = &line[prefix_end..];
    let (list_marker, after_marker) = if let Some(marker) = ['-', '*', '+']
        .into_iter()
        .find(|marker| rest.starts_with(*marker))
    {
        (marker.to_string(), &rest[marker.len_utf8()..])
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let suffix = rest.as_bytes().get(digits).copied()?;
        if !matches!(suffix, b'.' | b')') {
            return None;
        }
        (rest[..=digits].to_string(), &rest[digits + 1..])
    };
    let spaces = after_marker
        .bytes()
        .take_while(|byte| *byte == b' ')
        .count();
    if spaces == 0 {
        return None;
    }
    let body = after_marker[spaces..].to_string();
    Some(ListLine {
        indentation: indentation.to_string(),
        list_marker,
        body,
        depth: indentation_depth(indentation),
    })
}

fn indentation_depth(indentation: &str) -> usize {
    indentation.chars().fold(0, |depth, value| match value {
        '\t' | '>' => depth + 4,
        _ => depth + 1,
    })
}

struct TaskLineComponents {
    indentation: String,
    list_marker: String,
    status: char,
    body: String,
}

fn parse_task_line(line: &str) -> Option<TaskLineComponents> {
    let list = parse_list_line(line)?;
    let after_open = list.body.strip_prefix('[')?;
    let mut chars = after_open.chars();
    let status = chars.next()?;
    let after_status = chars.as_str().strip_prefix(']')?;
    let body = after_status.trim_start_matches(' ').trim().to_string();
    Some(TaskLineComponents {
        indentation: list.indentation,
        list_marker: list.list_marker,
        status,
        body,
    })
}

#[derive(Debug, Default)]
struct TaskDetails {
    description: String,
    priority: Priority,
    created: Option<TaskDate>,
    start: Option<TaskDate>,
    scheduled: Option<TaskDate>,
    due: Option<TaskDate>,
    done: Option<TaskDate>,
    cancelled: Option<TaskDate>,
    recurrence_source: Option<String>,
    on_completion: String,
    id: String,
    depends_on: Vec<String>,
    tags: Vec<String>,
}

fn parse_details(line: &str, format: TaskFormat) -> TaskDetails {
    let mut details = TaskDetails::default();
    let mut state = line.trim().to_string();
    let mut trailing_tags = Vec::new();

    for _ in 0..=20 {
        let parsed_field = match format {
            TaskFormat::Dataview => {
                try_take_dataview_field(&mut state, &mut details)
            }
            TaskFormat::Emoji => try_take_emoji_field(&mut state, &mut details),
        };
        if parsed_field {
            continue;
        }

        if let Some(tag) = take_regex_value(&mut state, &HASH_TAG_AT_END) {
            trailing_tags.push(tag);
            continue;
        }
        break;
    }

    trailing_tags.reverse();
    if !trailing_tags.is_empty() {
        if !state.is_empty() {
            state.push(' ');
        }
        state.push_str(&trailing_tags.join(" "));
    }
    details.description = state;
    details.tags = HASH_TAGS
        .captures_iter(&details.description)
        .filter_map(|captures| captures.get(1))
        .map(|value| value.as_str().trim().to_string())
        .collect();
    details
}

fn try_take_dataview_field(
    state: &mut String,
    details: &mut TaskDetails,
) -> bool {
    let Some((start, field)) = trailing_inline_field(state) else {
        return false;
    };
    let Some((key, value)) = field.split_once("::") else {
        return false;
    };
    if key != key.trim() {
        return false;
    }
    let value = value.trim();
    let recognized = match key {
        "priority" => Priority::from_name(value)
            .map(|priority| details.priority = priority)
            .is_some(),
        "start" => set_date(&mut details.start, value),
        "created" => set_date(&mut details.created, value),
        "scheduled" => set_date(&mut details.scheduled, value),
        "due" => set_date(&mut details.due, value),
        "completion" => set_date(&mut details.done, value),
        "cancelled" => set_date(&mut details.cancelled, value),
        "repeat" if RECURRENCE_VALUE.is_match(value) => {
            details.recurrence_source = Some(value.to_string());
            true
        }
        "onCompletion" if LETTERS.is_match(value) => {
            details.on_completion = parse_on_completion(value);
            true
        }
        "id" if ID_VALUE.is_match(value) => {
            details.id = value.to_string();
            true
        }
        "dependsOn" if DEPENDS_ON_VALUE.is_match(value) => {
            details.depends_on = parse_depends_on(value);
            true
        }
        _ => false,
    };
    if recognized {
        *state = state[..start].trim().to_string();
    }
    recognized
}

fn trailing_inline_field(state: &str) -> Option<(usize, String)> {
    let mut end = state.trim_end().len();
    if state[..end].ends_with(',') {
        end -= 1;
        end = state[..end].trim_end().len();
    }
    let close = state[..end].chars().next_back()?;
    let open = match close {
        ']' => '[',
        ')' => '(',
        _ => return None,
    };
    let without_close = &state[..end - close.len_utf8()];
    let start = without_close.rfind(open)?;
    let inner = without_close[start + open.len_utf8()..].trim().to_string();
    Some((start, inner))
}

fn try_take_emoji_field(state: &mut String, details: &mut TaskDetails) -> bool {
    if let Some(value) = take_regex_value(state, &EMOJI_PRIORITY) {
        details.priority = Priority::from_emoji(&value).unwrap_or_default();
        return true;
    }
    for (regex, target) in [
        (&*EMOJI_DONE, &mut details.done),
        (&*EMOJI_CANCELLED, &mut details.cancelled),
        (&*EMOJI_DUE, &mut details.due),
        (&*EMOJI_SCHEDULED, &mut details.scheduled),
        (&*EMOJI_START, &mut details.start),
        (&*EMOJI_CREATED, &mut details.created),
    ] {
        if let Some(value) = take_regex_value(state, regex) {
            *target = Some(TaskDate::parse(&value));
            return true;
        }
    }
    if let Some(value) = take_regex_value(state, &EMOJI_RECURRENCE) {
        details.recurrence_source = Some(value.trim().to_string());
        return true;
    }
    if let Some(value) = take_regex_value(state, &EMOJI_ON_COMPLETION) {
        details.on_completion = parse_on_completion(&value);
        return true;
    }
    if let Some(value) = take_regex_value(state, &EMOJI_ID) {
        details.id = value.trim().to_string();
        return true;
    }
    if let Some(value) = take_regex_value(state, &EMOJI_DEPENDS_ON) {
        details.depends_on = parse_depends_on(&value);
        return true;
    }
    false
}

fn take_regex_value(state: &mut String, regex: &Regex) -> Option<String> {
    let captures = regex.captures(state)?;
    let whole = captures.get(0)?;
    let value = captures.get(1)?.as_str().trim().to_string();
    *state = state[..whole.start()].trim().to_string();
    Some(value)
}

fn set_date(target: &mut Option<TaskDate>, value: &str) -> bool {
    if !DATE_VALUE.is_match(value) {
        return false;
    }
    *target = Some(TaskDate::parse(value));
    true
}

fn parse_depends_on(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_on_completion(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "delete" => "delete".to_string(),
        "keep" => "keep".to_string(),
        _ => String::new(),
    }
}

fn remove_global_filter_as_word(description: &str, filter: &str) -> String {
    if filter.is_empty() {
        return description.to_string();
    }
    description
        .split_whitespace()
        .filter(|word| *word != filter)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_recurrence(value: &str) -> Option<String> {
    let words = value.split_whitespace().collect::<Vec<_>>();
    if words.len() < 2 || !words[0].eq_ignore_ascii_case("every") {
        return None;
    }
    let when_done = words.len() >= 2
        && words[words.len() - 2].eq_ignore_ascii_case("when")
        && words[words.len() - 1].eq_ignore_ascii_case("done");
    let rule_words = if when_done {
        &words[..words.len() - 2]
    } else {
        &words[..]
    };
    if rule_words.len() < 2 {
        return None;
    }

    let weekdays = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let months = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let second = rule_words[1].to_ascii_lowercase();
    let mut normalized =
        if weekdays.contains(&second.as_str()) && rule_words.len() == 2 {
            format!("every week on {}", title_case(&second))
        } else {
            let valid_base = matches!(
                second.as_str(),
                "day"
                    | "days"
                    | "weekday"
                    | "weekdays"
                    | "week"
                    | "weeks"
                    | "month"
                    | "months"
                    | "year"
                    | "years"
            ) || months.contains(&second.as_str())
                || (second.parse::<u32>().is_ok_and(|interval| interval > 0)
                    && rule_words.get(2).is_some_and(|unit| {
                        matches!(
                            unit.to_ascii_lowercase().as_str(),
                            "day"
                                | "days"
                                | "week"
                                | "weeks"
                                | "month"
                                | "months"
                                | "year"
                                | "years"
                        )
                    }));
            if !valid_base {
                return None;
            }
            normalize_recurrence_words(rule_words, &weekdays, &months)?
        };
    if when_done {
        normalized.push_str(" when done");
    }
    Some(normalized)
}

fn normalize_recurrence_words(
    words: &[&str],
    weekdays: &[&str],
    months: &[&str],
) -> Option<String> {
    let mut output = Vec::with_capacity(words.len());
    for (index, word) in words.iter().enumerate() {
        let punctuation =
            word.chars().rev().take_while(|value| *value == ',').count();
        let bare = &word[..word.len() - punctuation];
        let lower = bare.to_ascii_lowercase();
        let normalized = if weekdays.contains(&lower.as_str())
            || months.contains(&lower.as_str())
        {
            title_case(&lower)
        } else if lower.parse::<u32>().is_ok()
            || is_ordinal(&lower)
            || matches!(
                lower.as_str(),
                "every"
                    | "day"
                    | "days"
                    | "weekday"
                    | "weekdays"
                    | "week"
                    | "weeks"
                    | "month"
                    | "months"
                    | "year"
                    | "years"
                    | "on"
                    | "the"
                    | "last"
                    | "and"
            )
        {
            lower
        } else {
            return None;
        };
        if index == 0 && normalized != "every" {
            return None;
        }
        output.push(format!("{normalized}{}", ",".repeat(punctuation)));
    }
    Some(output.join(" "))
}

fn is_ordinal(value: &str) -> bool {
    ["st", "nd", "rd", "th"].iter().any(|suffix| {
        value.strip_suffix(suffix).is_some_and(|number| {
            !number.is_empty() && number.parse::<u32>().is_ok()
        })
    })
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    format!("{}{}", first.to_ascii_uppercase(), characters.as_str())
}

fn calculate_urgency(details: &TaskDetails, now: NaiveDateTime) -> f64 {
    let today = now.date();
    let mut urgency = details.priority.urgency();
    if let Some(due) = details.due.as_ref().and_then(TaskDate::valid_date) {
        let days_overdue = today.signed_duration_since(due).num_days() as f64;
        let multiplier = if days_overdue >= 7.0 {
            1.0
        } else if days_overdue >= -14.0 {
            ((days_overdue + 14.0) * 0.8) / 21.0 + 0.2
        } else {
            0.2
        };
        urgency += multiplier * 12.0;
    }
    if details
        .scheduled
        .as_ref()
        .and_then(TaskDate::valid_date)
        .is_some_and(|scheduled| today >= scheduled)
    {
        urgency += 5.0;
    }
    if details
        .start
        .as_ref()
        .and_then(TaskDate::valid_date)
        .is_some_and(|start| today < start)
    {
        urgency -= 3.0;
    }
    urgency
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::dataview::tasks::settings::TasksSettings;

    fn settings(format: TaskFormat) -> TasksSettings {
        TasksSettings {
            task_format: format,
            global_filter: "#task".to_string(),
            ..TasksSettings::default()
        }
    }

    fn parsed_task(line: &str, format: TaskFormat) -> Task {
        let settings = settings(format);
        let statuses = StatusRegistry::new(&settings.status_settings);
        Task::from_line(
            line,
            TaskFile::new("Folder/Note.md".to_string()),
            7,
            Some("Heading".to_string()),
            &settings,
            &statuses,
            NaiveDate::from_ymd_opt(2026, 7, 10)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        )
        .expect("task")
    }

    #[test]
    fn task_line_parser_matches_tasks_markers_and_spacing() {
        for line in [
            "- [ ]task",
            "  * [/] task",
            "\t+ [*] task",
            "1. [x] task",
            "22) [-] task",
            "> - [?] quoted",
        ] {
            assert!(parse_task_line(line).is_some(), "{line}");
        }
        for line in ["- plain", "text - [ ] task", "- [] task", "-[ ] task"] {
            assert!(parse_task_line(line).is_none(), "{line}");
        }
    }

    #[test]
    fn dataview_parser_extracts_all_fields_and_cleans_description() {
        let task = parsed_task(
            "- [ ] #task Ship #hide [due:: 2026-07-12] [scheduled:: 2026-07-10] [start:: 2026-07-08] [created:: 2026-07-01] [completion:: 2026-07-09] [cancelled:: 2026-07-11] [priority:: high] [repeat:: every week] [id:: dv-all] [dependsOn:: done-root, other] [onCompletion:: keep] ^block-id",
            TaskFormat::Dataview,
        );
        assert_eq!(task.description, "#task Ship #hide");
        assert_eq!(task.display_description, "#task Ship #hide");
        assert_eq!(task.description_without_tags, "Ship");
        assert_eq!(task.tags, ["#hide"]);
        assert_eq!(task.priority, Priority::High);
        assert_eq!(
            task.due.as_ref().unwrap().value.as_deref(),
            Some("2026-07-12")
        );
        assert_eq!(task.recurrence_rule, "every week");
        assert_eq!(task.id, "dv-all");
        assert_eq!(task.depends_on, ["done-root", "other"]);
        assert_eq!(task.on_completion, "keep");
        assert_eq!(task.block_id.as_deref(), Some("block-id"));
    }

    #[test]
    fn emoji_parser_extracts_all_fields_and_variant_selectors() {
        let task = parsed_task(
            "- [ ] #task Ship #hide 🔺 🔁 every Sunday 🛫 2026-07-08 ⏳️ 2026-07-10 📆 2026-07-12 ➕ 2026-07-01 ✅ 2026-07-09 ❌ 2026-07-11 🆔 emoji-all ⛔ done-root 🏁 delete",
            TaskFormat::Emoji,
        );
        assert_eq!(task.description, "#task Ship #hide");
        assert_eq!(task.priority, Priority::Highest);
        assert_eq!(task.recurrence_rule, "every week on Sunday");
        assert_eq!(task.on_completion, "delete");
        assert_eq!(task.id, "emoji-all");
        assert_eq!(task.depends_on, ["done-root"]);
        assert!(task.scheduled.as_ref().unwrap().valid);
    }

    #[test]
    fn invalid_dates_and_recurrences_match_tasks_semantics() {
        let task = parsed_task(
            "- [ ] #task Invalid [scheduled:: 2026-99-99] [repeat:: every week]",
            TaskFormat::Dataview,
        );
        assert!(!task.scheduled.as_ref().unwrap().valid);
        assert!(
            !task.is_recurring,
            "invalid reference date drops recurrence"
        );

        let task = parsed_task(
            "- [ ] #task Invalid recurrence [repeat:: every seven weeks]",
            TaskFormat::Dataview,
        );
        assert_eq!(task.description, "#task Invalid recurrence");
        assert!(!task.is_recurring);
    }

    #[test]
    fn metadata_must_be_trailing_but_tags_can_be_interleaved() {
        let details = parse_details(
            "Wobble [priority::high] #tag1 [due::2025-10-05] #tag2 [repeat::every day] #tag3",
            TaskFormat::Dataview,
        );
        assert_eq!(details.description, "Wobble #tag1 #tag2 #tag3");
        assert_eq!(details.priority, Priority::High);
        assert_eq!(details.tags, ["#tag1", "#tag2", "#tag3"]);

        let details = parse_details(
            "Due in the middle [due:: 2026-07-10] then text",
            TaskFormat::Dataview,
        );
        assert!(details.due.is_none());
    }

    #[test]
    fn dataview_fields_honor_delimiters_whitespace_commas_and_case() {
        let details = parse_details(
            "Task [ due::2026-07-12], (priority:: high)       ,",
            TaskFormat::Dataview,
        );
        assert_eq!(details.description, "Task");
        assert_eq!(details.priority, Priority::High);
        assert!(details.due.as_ref().unwrap().valid);

        for unrecognized in [
            "Task [due::2026-07-12)",
            "Task [due :: 2026-07-12]",
            "Task [Due:: 2026-07-12]",
            "Task due:: 2026-07-12",
            "Task [id:: invalid*id]",
            "Task [dependson:: abc]",
        ] {
            let details = parse_details(unrecognized, TaskFormat::Dataview);
            assert_eq!(details.description, unrecognized, "{unrecognized}");
        }
    }

    #[test]
    fn all_priorities_have_tasks_v8_names_numbers_and_scores() {
        for (dataview, emoji, expected, number, urgency) in [
            ("highest", "🔺", Priority::Highest, 0, 9.0),
            ("high", "⏫", Priority::High, 1, 6.0),
            ("medium", "🔼", Priority::Medium, 2, 3.9),
            ("low", "🔽", Priority::Low, 4, 0.0),
            ("lowest", "⏬", Priority::Lowest, 5, -1.8),
        ] {
            let details = parse_details(
                &format!("[priority:: {dataview}]"),
                TaskFormat::Dataview,
            );
            assert_eq!(details.priority, expected);
            let details = parse_details(emoji, TaskFormat::Emoji);
            assert_eq!(details.priority, expected);
            assert_eq!(expected.number(), number);
            assert_eq!(expected.urgency(), urgency);
        }
        assert_eq!(Priority::Normal.number(), 3);
        assert_eq!(Priority::Normal.urgency(), 1.95);
    }

    #[test]
    fn recurrence_rules_are_validated_and_standardized() {
        for (input, expected) in [
            ("every day", "every day"),
            ("every 3 weeks when done", "every 3 weeks when done"),
            ("every Sunday", "every week on Sunday"),
            (
                "every month on the 2nd Wednesday",
                "every month on the 2nd Wednesday",
            ),
            (
                "every April and December on the 1st and 24th",
                "every April and December on the 1st and 24th",
            ),
        ] {
            assert_eq!(normalize_recurrence(input).as_deref(), Some(expected));
        }
        for invalid in [
            "weekly",
            "every",
            "every seven weeks",
            "every 0 days",
            "every week on Funday",
        ] {
            assert_eq!(normalize_recurrence(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn urgency_due_scheduled_and_start_boundaries_match_tasks_v8() {
        let now = NaiveDate::from_ymd_opt(2026, 7, 10)
            .unwrap()
            .and_hms_opt(23, 59, 0)
            .unwrap();
        let low = |due: Option<&str>,
                   scheduled: Option<&str>,
                   start: Option<&str>| {
            TaskDetails {
                priority: Priority::Low,
                due: due.map(TaskDate::parse),
                scheduled: scheduled.map(TaskDate::parse),
                start: start.map(TaskDate::parse),
                ..TaskDetails::default()
            }
        };
        assert!(
            (calculate_urgency(&low(Some("2026-07-10"), None, None), now)
                - 8.8)
                .abs()
                < 0.00001
        );
        assert!(
            (calculate_urgency(&low(Some("2026-07-25"), None, None), now,)
                - 2.4)
                .abs()
                < 0.00001
        );
        assert_eq!(
            calculate_urgency(&low(None, Some("2026-07-10"), None), now),
            5.0
        );
        assert_eq!(
            calculate_urgency(&low(None, Some("2026-07-11"), None), now),
            0.0
        );
        assert_eq!(
            calculate_urgency(&low(None, None, Some("2026-07-11")), now),
            -3.0
        );
        assert_eq!(
            calculate_urgency(&low(None, None, Some("2026-07-10")), now),
            0.0
        );
        assert_eq!(
            calculate_urgency(&low(Some("2026-99-99"), None, None), now),
            0.0
        );
    }

    #[test]
    fn unknown_status_is_todo_and_remove_global_filter_is_display_only() {
        let mut settings = settings(TaskFormat::Dataview);
        settings.remove_global_filter = true;
        let statuses = StatusRegistry::new(&settings.status_settings);
        let task = Task::from_line(
            "- [?] #task Unknown #task/subtag",
            TaskFile::new("Note.md".to_string()),
            0,
            None,
            &settings,
            &statuses,
            NaiveDate::from_ymd_opt(2026, 7, 10)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(task.status.name, "Unknown");
        assert_eq!(task.status.status_type, StatusType::Todo);
        assert_eq!(task.status.next_symbol, "x");
        assert_eq!(task.description, "#task Unknown #task/subtag");
        assert_eq!(task.display_description, "Unknown #task/subtag");
        assert_eq!(task.tags, ["#task/subtag"]);
    }

    #[test]
    fn urgency_matches_tasks_v8_coefficients() {
        let low = parsed_task(
            "- [ ] #task Due today [due:: 2026-07-10] [priority:: low]",
            TaskFormat::Dataview,
        );
        assert!((low.urgency - 8.8).abs() < 0.00001);

        let complete = parsed_task(
            "- [ ] #task Complete [due:: 2026-07-12] [scheduled:: 2026-07-10] [start:: 2026-07-11] [priority:: high]",
            TaskFormat::Dataview,
        );
        let expected = 6.0 + 7.885714285714286 + 5.0 - 3.0;
        assert!((complete.urgency - expected).abs() < 0.00001);
    }

    #[test]
    fn file_context_matches_tasks_expose_properties() {
        let file = TaskFile::new("some/folder/fileName.md".to_string());
        assert_eq!(file.path_without_extension, "some/folder/fileName");
        assert_eq!(file.root, "some/");
        assert_eq!(file.folder, "some/folder/");
        assert_eq!(file.filename, "fileName.md");
        assert_eq!(file.filename_without_extension, "fileName");
        assert_eq!(TaskFile::new("root.md".to_string()).folder, "/");
    }
}
