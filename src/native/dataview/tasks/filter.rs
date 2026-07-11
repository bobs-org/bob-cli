use std::collections::HashMap;

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, Weekday};
use regex::RegexBuilder;

use super::{
    js::JsSandbox,
    parse::{
        DateField, DateRelation, FilterExpr, Presence, PresenceField,
        PriorityRelation, TextField, TextOperator,
    },
    task::{Task, TaskDate},
};
use crate::native::dataview::DataviewError;

pub(super) fn apply(
    filters: &[FilterExpr],
    tasks: Vec<Task>,
    now: NaiveDateTime,
    global_filter: &str,
    javascript: &mut JsSandbox,
) -> Result<Vec<Task>, DataviewError> {
    let regexes = compile_filter_regexes(filters)
        .map_err(|message| DataviewError::TasksQuery { message })?;
    for filter in filters {
        validate_filter(filter, now.date())
            .map_err(|message| DataviewError::TasksQuery { message })?;
        validate_function_filter(filter, javascript)?;
    }
    tasks
        .into_iter()
        .filter_map(|task| {
            let result = filters.iter().try_fold(true, |matched, filter| {
                if !matched {
                    Ok(false)
                } else {
                    matches_filter(
                        filter,
                        &task,
                        now.date(),
                        global_filter,
                        javascript,
                        &regexes,
                    )
                }
            });
            match result {
                Ok(true) => Some(Ok(task)),
                Ok(false) => None,
                Err(message) => {
                    Some(Err(DataviewError::TasksQuery { message }))
                }
            }
        })
        .collect()
}

fn compile_filter_regexes(
    filters: &[FilterExpr],
) -> Result<HashMap<(String, String), regex::Regex>, String> {
    fn visit(
        filter: &FilterExpr,
        regexes: &mut HashMap<(String, String), regex::Regex>,
    ) -> Result<(), String> {
        match filter {
            FilterExpr::And { left, right }
            | FilterExpr::Or { left, right }
            | FilterExpr::Xor { left, right } => {
                visit(left, regexes)?;
                visit(right, regexes)
            }
            FilterExpr::Not { expression } => visit(expression, regexes),
            FilterExpr::Text {
                operator:
                    TextOperator::RegexMatches | TextOperator::RegexDoesNotMatch,
                value,
                regex_flags,
                ..
            } => {
                let key =
                    (value.clone(), regex_flags.clone().unwrap_or_default());
                if !regexes.contains_key(&key) {
                    regexes.insert(key.clone(), build_regex(&key.0, &key.1)?);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    let mut regexes = HashMap::new();
    for filter in filters {
        visit(filter, &mut regexes)?;
    }
    Ok(regexes)
}

fn validate_function_filter(
    filter: &FilterExpr,
    javascript: &mut JsSandbox,
) -> Result<(), DataviewError> {
    match filter {
        FilterExpr::And { left, right }
        | FilterExpr::Or { left, right }
        | FilterExpr::Xor { left, right } => {
            validate_function_filter(left, javascript)?;
            validate_function_filter(right, javascript)
        }
        FilterExpr::Not { expression } => {
            validate_function_filter(expression, javascript)
        }
        FilterExpr::Function { source } => {
            javascript.validate_expression(source)
        }
        _ => Ok(()),
    }
}

fn validate_filter(
    filter: &FilterExpr,
    today: NaiveDate,
) -> Result<(), String> {
    match filter {
        FilterExpr::And { left, right }
        | FilterExpr::Or { left, right }
        | FilterExpr::Xor { left, right } => {
            validate_filter(left, today)?;
            validate_filter(right, today)
        }
        FilterExpr::Not { expression } => validate_filter(expression, today),
        FilterExpr::Date { field, value, .. } => {
            parse_date_range(value, today).map(|_| ()).ok_or_else(|| {
                format!(
                    "do not understand {} date '{value}'",
                    date_field(*field)
                )
            })
        }
        _ => Ok(()),
    }
}

fn matches_filter(
    filter: &FilterExpr,
    task: &Task,
    today: NaiveDate,
    global_filter: &str,
    javascript: &mut JsSandbox,
    regexes: &HashMap<(String, String), regex::Regex>,
) -> Result<bool, String> {
    match filter {
        FilterExpr::And { left, right } => Ok(matches_filter(
            left,
            task,
            today,
            global_filter,
            javascript,
            regexes,
        )? && matches_filter(
            right,
            task,
            today,
            global_filter,
            javascript,
            regexes,
        )?),
        FilterExpr::Or { left, right } => Ok(matches_filter(
            left,
            task,
            today,
            global_filter,
            javascript,
            regexes,
        )? || matches_filter(
            right,
            task,
            today,
            global_filter,
            javascript,
            regexes,
        )?),
        FilterExpr::Xor { left, right } => Ok(matches_filter(
            left,
            task,
            today,
            global_filter,
            javascript,
            regexes,
        )? ^ matches_filter(
            right,
            task,
            today,
            global_filter,
            javascript,
            regexes,
        )?),
        FilterExpr::Not { expression } => Ok(!matches_filter(
            expression,
            task,
            today,
            global_filter,
            javascript,
            regexes,
        )?),
        FilterExpr::Done { done } => Ok(task.is_done == *done),
        FilterExpr::StatusType { negated, value } => {
            let matched = task.status.status_type.as_str() == value;
            Ok(if *negated { !matched } else { matched })
        }
        FilterExpr::Text {
            field,
            operator,
            value,
            regex_flags,
        } => matches_text(
            *field,
            *operator,
            value,
            regex_flags.as_deref(),
            task,
            global_filter,
            regexes,
        ),
        FilterExpr::Date {
            field,
            relation,
            value,
        } => matches_date(*field, *relation, value, task, today),
        FilterExpr::DatePresence { field, presence } => {
            Ok(matches_date_presence(*field, *presence, task))
        }
        FilterExpr::Priority { relation, value } => {
            let expected = priority_number(value).ok_or_else(|| {
                format!("do not understand priority '{value}'")
            })?;
            Ok(match relation {
                PriorityRelation::Is => task.priority_number == expected,
                PriorityRelation::IsNot => task.priority_number != expected,
                PriorityRelation::Above => task.priority_number < expected,
                PriorityRelation::Below => task.priority_number > expected,
            })
        }
        FilterExpr::Recurring { recurring } => {
            Ok(task.is_recurring == *recurring)
        }
        FilterExpr::Presence { field, present } => {
            let has_value = match field {
                PresenceField::Id => !task.id.is_empty(),
                PresenceField::DependsOn => !task.depends_on.is_empty(),
                PresenceField::Tag => !task.tags.is_empty(),
            };
            Ok(has_value == *present)
        }
        FilterExpr::Blocked { blocked } => Ok(task.is_blocked == *blocked),
        FilterExpr::Blocking { blocking } => Ok(task.is_blocking == *blocking),
        FilterExpr::Function { source } => javascript
            .matches_filter(source, task)
            .map_err(|error| match error {
                DataviewError::TasksQuery { message } => message,
                other => format!("{other:?}"),
            }),
        FilterExpr::ExcludeSubItems => Ok(is_top_level(task)),
    }
}

fn matches_text(
    field: TextField,
    operator: TextOperator,
    needle: &str,
    flags: Option<&str>,
    task: &Task,
    global_filter: &str,
    regexes: &HashMap<(String, String), regex::Regex>,
) -> Result<bool, String> {
    let values = text_values(field, task, global_filter);
    let matched = match operator {
        TextOperator::Is => values.iter().any(|value| value == needle),
        TextOperator::IsNot => values.iter().all(|value| value != needle),
        TextOperator::Includes => values
            .iter()
            .any(|value| includes_case_insensitive(value, needle)),
        TextOperator::DoesNotInclude => values
            .iter()
            .all(|value| !includes_case_insensitive(value, needle)),
        TextOperator::RegexMatches | TextOperator::RegexDoesNotMatch => {
            let key =
                (needle.to_string(), flags.unwrap_or_default().to_string());
            let regex = regexes
                .get(&key)
                .expect("regex filters are compiled before matching");
            let any = values.iter().any(|value| regex.is_match(value));
            if operator == TextOperator::RegexDoesNotMatch {
                !any
            } else {
                any
            }
        }
    };
    Ok(matched)
}

fn text_values(
    field: TextField,
    task: &Task,
    global_filter: &str,
) -> Vec<String> {
    let one = |value: String| vec![value];
    match field {
        TextField::Description => {
            one(remove_global_filter(&task.description, global_filter))
        }
        TextField::Heading => one(task.heading.clone().unwrap_or_default()),
        TextField::Path => one(task.path.clone()),
        TextField::Folder => one(task.file.folder.clone()),
        TextField::Filename => one(task.file.filename.clone()),
        TextField::Root => one(task.file.root.clone()),
        TextField::Backlink => one(match &task.heading {
            Some(heading) => format!("[[{}#{}]]", task.path, heading),
            None => format!("[[{}]]", task.path),
        }),
        TextField::Tag => task.tags.clone(),
        TextField::Recurrence => one(task.recurrence_rule.clone()),
        TextField::Id => one(task.id.clone()),
        TextField::StatusName => one(task.status.name.clone()),
        TextField::StatusSymbol => one(task.status.symbol.clone()),
    }
}

fn remove_global_filter(description: &str, global_filter: &str) -> String {
    if global_filter.is_empty() {
        return description.to_string();
    }
    description
        .replacen(global_filter, "", 1)
        .trim()
        .to_string()
}

fn includes_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn build_regex(source: &str, flags: &str) -> Result<regex::Regex, String> {
    let source = if flags.contains('y') {
        format!(r"\A(?:{source})")
    } else {
        source.to_string()
    };
    RegexBuilder::new(&source)
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'))
        .unicode(true)
        .build()
        .map_err(|error| format!("Parsing regular expression: {error}"))
}

fn priority_number(value: &str) -> Option<u8> {
    match value {
        "highest" => Some(0),
        "high" => Some(1),
        "medium" => Some(2),
        "none" => Some(3),
        "low" => Some(4),
        "lowest" => Some(5),
        _ => None,
    }
}

fn matches_date_presence(
    field: DateField,
    presence: Presence,
    task: &Task,
) -> bool {
    let dates = task_dates(field, task);
    match presence {
        Presence::Has => dates.iter().any(|date| date.is_some()),
        Presence::Missing => dates.iter().all(|date| date.is_none()),
        Presence::Invalid => dates
            .into_iter()
            .filter_map(Option::as_ref)
            .any(|date| !date.valid),
    }
}

fn matches_date(
    field: DateField,
    relation: DateRelation,
    expression: &str,
    task: &Task,
    today: NaiveDate,
) -> Result<bool, String> {
    let range = parse_date_range(expression, today).ok_or_else(|| {
        format!(
            "do not understand {} date '{expression}'",
            date_field(field)
        )
    })?;
    let missing_matches = field == DateField::Start;
    let dates = task_dates(field, task);
    if dates.iter().all(|date| date.is_none()) {
        return Ok(missing_matches);
    }
    Ok(dates.iter().any(|date| {
        date.as_ref()
            .and_then(TaskDate::valid_date)
            .is_some_and(|date| date_matches(date, relation, range))
    }))
}

fn date_matches(
    date: NaiveDate,
    relation: DateRelation,
    (start, end): (NaiveDate, NaiveDate),
) -> bool {
    match relation {
        DateRelation::In | DateRelation::On => date >= start && date <= end,
        DateRelation::Before => date < start,
        DateRelation::After => date > end,
        DateRelation::OnOrBefore => date <= end,
        DateRelation::OnOrAfter => date >= start,
    }
}

fn task_dates(field: DateField, task: &Task) -> Vec<&Option<TaskDate>> {
    match field {
        DateField::Due => vec![&task.due],
        DateField::Scheduled => vec![&task.scheduled],
        DateField::Start => vec![&task.start],
        DateField::Created => vec![&task.created],
        DateField::Done => vec![&task.done],
        DateField::Cancelled => vec![&task.cancelled],
        DateField::Happens => vec![&task.start, &task.scheduled, &task.due],
    }
}

fn date_field(field: DateField) -> &'static str {
    match field {
        DateField::Due => "due",
        DateField::Scheduled => "scheduled",
        DateField::Start => "start",
        DateField::Created => "created",
        DateField::Done => "done",
        DateField::Cancelled => "cancelled",
        DateField::Happens => "happens",
    }
}

fn parse_date_range(
    expression: &str,
    today: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let value = expression.trim().to_ascii_lowercase();
    match value.as_str() {
        "today" => return Some((today, today)),
        "yesterday" => {
            let date = today.pred_opt()?;
            return Some((date, date));
        }
        "tomorrow" => {
            let date = today.succ_opt()?;
            return Some((date, date));
        }
        _ => {}
    }

    let words = value.split_whitespace().collect::<Vec<_>>();
    if let [direction, weekday] = words.as_slice()
        && matches!(*direction, "next" | "last")
        && let Some(weekday) = parse_weekday(weekday)
    {
        let forward = (weekday.num_days_from_monday() as i64
            - today.weekday().num_days_from_monday() as i64)
            .rem_euclid(7);
        let days = if *direction == "next" {
            if forward == 0 {
                7
            } else {
                forward
            }
        } else if forward == 0 {
            -7
        } else {
            forward - 7
        };
        let date = today.checked_add_signed(Duration::days(days))?;
        return Some((date, date));
    }

    if let Some(weekday) = parse_weekday(&value) {
        let forward = (weekday.num_days_from_monday() as i64
            - today.weekday().num_days_from_monday() as i64)
            .rem_euclid(7);
        let closest = if forward > 3 { forward - 7 } else { forward };
        let date = today.checked_add_signed(Duration::days(closest))?;
        return Some((date, date));
    }

    if let Some(range) = parse_numbered_range(&value) {
        return Some(range);
    }

    if let Some(range) = parse_named_range(&value, today) {
        return Some(range);
    }

    if let [first, second] = words.as_slice()
        && let (Ok(first), Ok(second)) = (
            NaiveDate::parse_from_str(first, "%Y-%m-%d"),
            NaiveDate::parse_from_str(second, "%Y-%m-%d"),
        )
    {
        return Some(if first <= second {
            (first, second)
        } else {
            (second, first)
        });
    }
    if let [single] = words.as_slice()
        && let Ok(date) = NaiveDate::parse_from_str(single, "%Y-%m-%d")
    {
        return Some((date, date));
    }
    parse_offset(&words, today).map(|date| (date, date))
}

fn parse_numbered_range(value: &str) -> Option<(NaiveDate, NaiveDate)> {
    if value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let year = value.parse().ok()?;
        return Some((
            NaiveDate::from_ymd_opt(year, 1, 1)?,
            NaiveDate::from_ymd_opt(year, 12, 31)?,
        ));
    }
    if let Some((year, month)) = value.split_once('-')
        && year.len() == 4
        && month.len() == 2
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && month.bytes().all(|byte| byte.is_ascii_digit())
    {
        let start = NaiveDate::from_ymd_opt(
            year.parse().ok()?,
            month.parse().ok()?,
            1,
        )?;
        return Some((start, end_of_month(start)?));
    }
    if value.len() == 7 && &value[4..6] == "-q" {
        let year = value[..4].parse().ok()?;
        let quarter = value[6..].parse::<u32>().ok()?;
        if !(1..=4).contains(&quarter) {
            return None;
        }
        let start = NaiveDate::from_ymd_opt(year, (quarter - 1) * 3 + 1, 1)?;
        let end = start.checked_add_months(Months::new(3))?.pred_opt()?;
        return Some((start, end));
    }
    if value.len() == 8 && &value[4..6] == "-w" {
        let year = value[..4].parse().ok()?;
        let week = value[6..].parse().ok()?;
        let start = NaiveDate::from_isoywd_opt(year, week, Weekday::Mon)?;
        return Some((start, start.checked_add_signed(Duration::days(6))?));
    }
    None
}

fn parse_named_range(
    value: &str,
    today: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let (offset, unit) =
        match value.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["last", unit] => (-1, *unit),
            ["this", unit] => (0, *unit),
            ["next", unit] => (1, *unit),
            _ => return None,
        };
    match unit {
        "week" => {
            let monday = today.checked_sub_signed(Duration::days(
                today.weekday().num_days_from_monday() as i64,
            ))?;
            let start = monday.checked_add_signed(Duration::weeks(offset))?;
            Some((start, start.checked_add_signed(Duration::days(6))?))
        }
        "month" => {
            let current =
                NaiveDate::from_ymd_opt(today.year(), today.month(), 1)?;
            let start = shift_months(current, offset)?;
            Some((start, end_of_month(start)?))
        }
        "quarter" => {
            let month = ((today.month() - 1) / 3) * 3 + 1;
            let current = NaiveDate::from_ymd_opt(today.year(), month, 1)?;
            let start = shift_months(current, offset * 3)?;
            let next = start.checked_add_months(Months::new(3))?;
            Some((start, next.pred_opt()?))
        }
        "year" => {
            let year = today.year().checked_add(offset as i32)?;
            Some((
                NaiveDate::from_ymd_opt(year, 1, 1)?,
                NaiveDate::from_ymd_opt(year, 12, 31)?,
            ))
        }
        _ => None,
    }
}

fn parse_offset(words: &[&str], today: NaiveDate) -> Option<NaiveDate> {
    let (amount, unit, past) = match words {
        [amount, unit, "ago"] => (amount.parse::<i64>().ok()?, *unit, true),
        [amount, unit] => (amount.parse::<i64>().ok()?, *unit, false),
        _ => return None,
    };
    let signed = if past { -amount } else { amount };
    match unit.trim_end_matches('s') {
        "day" => today.checked_add_signed(Duration::days(signed)),
        "week" => today.checked_add_signed(Duration::weeks(signed)),
        "month" => shift_months(today, signed),
        "year" => shift_months(today, signed.checked_mul(12)?),
        _ => None,
    }
}

fn shift_months(date: NaiveDate, months: i64) -> Option<NaiveDate> {
    let amount = Months::new(months.unsigned_abs().try_into().ok()?);
    if months < 0 {
        date.checked_sub_months(amount)
    } else {
        date.checked_add_months(amount)
    }
}

fn end_of_month(date: NaiveDate) -> Option<NaiveDate> {
    let next = date.checked_add_months(Months::new(1))?;
    next.pred_opt()
}

fn parse_weekday(value: &str) -> Option<Weekday> {
    match value {
        "monday" => Some(Weekday::Mon),
        "tuesday" => Some(Weekday::Tue),
        "wednesday" => Some(Weekday::Wed),
        "thursday" => Some(Weekday::Thu),
        "friday" => Some(Weekday::Fri),
        "saturday" => Some(Weekday::Sat),
        "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

fn is_top_level(task: &Task) -> bool {
    if task.indentation.is_empty() {
        return true;
    }
    task.indentation
        .rsplit_once('>')
        .is_some_and(|(_, suffix)| {
            suffix.len() <= 1 && suffix.trim().is_empty()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn relative_ranges_use_iso_weeks_and_calendar_boundaries() {
        let today = date("2026-07-10");
        assert_eq!(
            parse_date_range("this week", today),
            Some((date("2026-07-06"), date("2026-07-12")))
        );
        assert_eq!(
            parse_date_range("next month", today),
            Some((date("2026-08-01"), date("2026-08-31")))
        );
        assert_eq!(
            parse_date_range("last quarter", today),
            Some((date("2026-04-01"), date("2026-06-30")))
        );
        assert_eq!(
            parse_date_range("this year", today),
            Some((date("2026-01-01"), date("2026-12-31")))
        );

        let sunday = date("2027-01-03");
        assert_eq!(
            parse_date_range("this week", sunday),
            Some((date("2026-12-28"), date("2027-01-03")))
        );
        assert_eq!(
            parse_date_range("next quarter", date("2026-12-31")),
            Some((date("2027-01-01"), date("2027-03-31")))
        );
        assert_eq!(
            parse_date_range("next month", date("2027-01-31")),
            Some((date("2027-02-01"), date("2027-02-28")))
        );
    }

    #[test]
    fn absolute_ranges_are_inclusive_and_order_independent() {
        let today = date("2026-07-10");
        assert_eq!(
            parse_date_range("2026-07-12 2026-07-08", today),
            Some((date("2026-07-08"), date("2026-07-12")))
        );
        assert!(date_matches(
            date("2026-07-10"),
            DateRelation::In,
            (date("2026-07-08"), date("2026-07-12"))
        ));
    }

    #[test]
    fn weekday_and_offset_dates_are_pinned_to_now() {
        let today = date("2026-07-10");
        assert_eq!(
            parse_date_range("monday", today),
            Some((date("2026-07-13"), date("2026-07-13")))
        );
        assert_eq!(
            parse_date_range("tuesday", date("2026-07-08")),
            Some((date("2026-07-07"), date("2026-07-07")))
        );
        assert_eq!(
            parse_date_range("next monday", today),
            Some((date("2026-07-13"), date("2026-07-13")))
        );
        assert_eq!(
            parse_date_range("last monday", today),
            Some((date("2026-07-06"), date("2026-07-06")))
        );
        assert_eq!(
            parse_date_range("2 weeks", today),
            Some((date("2026-07-24"), date("2026-07-24")))
        );
        assert_eq!(
            parse_date_range("3 days ago", today),
            Some((date("2026-07-07"), date("2026-07-07")))
        );
    }

    #[test]
    fn numbered_ranges_cover_year_month_quarter_and_iso_week() {
        let today = date("2026-07-10");
        assert_eq!(
            parse_date_range("2026", today),
            Some((date("2026-01-01"), date("2026-12-31")))
        );
        assert_eq!(
            parse_date_range("2026-02", today),
            Some((date("2026-02-01"), date("2026-02-28")))
        );
        assert_eq!(
            parse_date_range("2026-Q3", today),
            Some((date("2026-07-01"), date("2026-09-30")))
        );
        assert_eq!(
            parse_date_range("2026-W01", today),
            Some((date("2025-12-29"), date("2026-01-04")))
        );
    }

    #[test]
    fn regex_flags_match_javascript_filtering_behavior() {
        assert!(build_regex("hello", "i").unwrap().is_match("HELLO"));
        assert!(build_regex("^second", "m")
            .unwrap()
            .is_match("first\nsecond"));
        assert!(build_regex("first.second", "s")
            .unwrap()
            .is_match("first\nsecond"));
        assert!(build_regex("first", "y").unwrap().is_match("first second"));
        assert!(!build_regex("second", "y").unwrap().is_match("first second"));

        // d and g only affect match metadata/state in JavaScript, and u/v
        // select Unicode behavior. None changes a Boolean Tasks match.
        assert!(build_regex("café", "dgv").unwrap().is_match("un café"));
    }

    #[test]
    fn global_filter_removal_only_removes_the_first_occurrence() {
        assert_eq!(
            remove_global_filter("#task keep #task", "#task"),
            "keep #task"
        );
    }
}
