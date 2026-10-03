//! Date, duration, currency, and by-key aggregation builtins.

use super::super::*;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use regex::Regex;
use std::{cmp::Ordering, collections::BTreeMap};

pub(in crate::native::dataview) fn striptime_value(
    value: DataviewValue,
) -> DataviewValue {
    match value {
        DataviewValue::Date(_) => value,
        DataviewValue::DateTime(value) | DataviewValue::String(value) => {
            date_from_text(&value)
                .and_then(|value| match value {
                    DataviewValue::Date(date) => {
                        Some(DataviewValue::Date(date))
                    }
                    DataviewValue::DateTime(datetime) => datetime
                        .split_once('T')
                        .map(|(date, _)| DataviewValue::Date(date.to_string())),
                    _ => None,
                })
                .unwrap_or(DataviewValue::Null)
        }
        _ => DataviewValue::Null,
    }
}

pub(in crate::native::dataview) fn evaluate_dateformat_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [date, format] = args else {
        return DataviewValue::Null;
    };
    let format = value_text(&format.evaluate(context));
    vectorize_unary(date.evaluate(context), |value| {
        date_components(&value)
            .map(|date| DataviewValue::String(format_date(&date, &format)))
            .unwrap_or(DataviewValue::Null)
    })
}

pub(in crate::native::dataview) struct DateComponents {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    timestamp_millis: Option<i64>,
}

pub(in crate::native::dataview) fn date_components(
    value: &DataviewValue,
) -> Option<DateComponents> {
    let value = match value {
        DataviewValue::Date(value)
        | DataviewValue::DateTime(value)
        | DataviewValue::String(value) => value.as_str(),
        _ => return None,
    };
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        let datetime = datetime.with_timezone(&Utc);
        return Some(DateComponents {
            year: datetime
                .naive_utc()
                .date()
                .format("%Y")
                .to_string()
                .parse()
                .ok()?,
            month: datetime
                .naive_utc()
                .date()
                .format("%m")
                .to_string()
                .parse()
                .ok()?,
            day: datetime
                .naive_utc()
                .date()
                .format("%d")
                .to_string()
                .parse()
                .ok()?,
            hour: datetime
                .naive_utc()
                .time()
                .format("%H")
                .to_string()
                .parse()
                .ok()?,
            minute: datetime
                .naive_utc()
                .time()
                .format("%M")
                .to_string()
                .parse()
                .ok()?,
            second: datetime
                .naive_utc()
                .time()
                .format("%S")
                .to_string()
                .parse()
                .ok()?,
            timestamp_millis: Some(datetime.timestamp_millis()),
        });
    }
    if let Ok(datetime) =
        NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
    {
        return Some(DateComponents {
            year: datetime.date().format("%Y").to_string().parse().ok()?,
            month: datetime.date().format("%m").to_string().parse().ok()?,
            day: datetime.date().format("%d").to_string().parse().ok()?,
            hour: datetime.time().format("%H").to_string().parse().ok()?,
            minute: datetime.time().format("%M").to_string().parse().ok()?,
            second: datetime.time().format("%S").to_string().parse().ok()?,
            timestamp_millis: Some(datetime.and_utc().timestamp_millis()),
        });
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(DateComponents {
            year: date.format("%Y").to_string().parse().ok()?,
            month: date.format("%m").to_string().parse().ok()?,
            day: date.format("%d").to_string().parse().ok()?,
            hour: 0,
            minute: 0,
            second: 0,
            timestamp_millis: date
                .and_hms_opt(0, 0, 0)
                .map(|datetime| datetime.and_utc().timestamp_millis()),
        });
    }
    None
}

pub(in crate::native::dataview) fn format_date(
    date: &DateComponents,
    format: &str,
) -> String {
    let mut output = String::new();
    let mut index = 0usize;
    while index < format.len() {
        let rest = &format[index..];
        let (replacement, width) =
            if rest.starts_with("yyyy") || rest.starts_with("YYYY") {
                (format!("{:04}", date.year), 4)
            } else if rest.starts_with("yy") || rest.starts_with("YY") {
                (format!("{:02}", date.year.rem_euclid(100)), 2)
            } else if rest.starts_with("MM") {
                (format!("{:02}", date.month), 2)
            } else if rest.starts_with('M') {
                (date.month.to_string(), 1)
            } else if rest.starts_with("dd") || rest.starts_with("DD") {
                (format!("{:02}", date.day), 2)
            } else if rest.starts_with('d') || rest.starts_with('D') {
                (date.day.to_string(), 1)
            } else if rest.starts_with("HH") {
                (format!("{:02}", date.hour), 2)
            } else if rest.starts_with('H') {
                (date.hour.to_string(), 1)
            } else if rest.starts_with("mm") {
                (format!("{:02}", date.minute), 2)
            } else if rest.starts_with('m') {
                (date.minute.to_string(), 1)
            } else if rest.starts_with("ss") {
                (format!("{:02}", date.second), 2)
            } else if rest.starts_with('s') {
                (date.second.to_string(), 1)
            } else if rest.starts_with('x') {
                (date.timestamp_millis.unwrap_or_default().to_string(), 1)
            } else {
                let ch = rest.chars().next().expect("non-empty format rest");
                output.push(ch);
                index += ch.len_utf8();
                continue;
            };
        output.push_str(&replacement);
        index += width;
    }
    output
}

pub(in crate::native::dataview) fn evaluate_durationformat_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [duration, format] = args else {
        return DataviewValue::Null;
    };
    let format = value_text(&format.evaluate(context));
    vectorize_unary(duration.evaluate(context), |value| {
        let duration = match value {
            DataviewValue::Duration(value) | DataviewValue::String(value) => {
                value
            }
            _ => return DataviewValue::Null,
        };
        duration_to_millis(&duration)
            .map(|millis| {
                DataviewValue::String(format_duration(millis, &format))
            })
            .unwrap_or(DataviewValue::Null)
    })
}

pub(in crate::native::dataview) fn duration_to_millis(
    value: &str,
) -> Option<i64> {
    let value = value.trim();
    if !value.starts_with('P') {
        return duration_text_to_iso(value)
            .and_then(|iso| duration_to_millis(&iso));
    }
    let token = Regex::new(r"(?i)([-+]?\d+(?:\.\d+)?)(Y|M|W|D|H|S)")
        .expect("valid ISO duration regex");
    let mut in_time = false;
    let mut millis = 0f64;
    for ch in value.chars() {
        if ch == 'T' {
            in_time = true;
        }
    }
    let mut time_position = false;
    for captures in token.captures_iter(value) {
        let amount = captures[1].parse::<f64>().ok()?;
        let unit = &captures[2];
        let before = &value[..captures.get(0)?.start()];
        if before.contains('T') {
            time_position = true;
        }
        millis += match unit {
            "Y" | "y" => amount * 365.0 * 86_400_000.0,
            "M" | "m" if !time_position && !in_time => {
                amount * 30.0 * 86_400_000.0
            }
            "M" | "m" if !time_position => amount * 30.0 * 86_400_000.0,
            "M" | "m" => amount * 60_000.0,
            "W" | "w" => amount * 7.0 * 86_400_000.0,
            "D" | "d" => amount * 86_400_000.0,
            "H" | "h" => amount * 3_600_000.0,
            "S" | "s" => amount * 1_000.0,
            _ => return None,
        };
    }
    Some(millis.round() as i64)
}

pub(in crate::native::dataview) fn format_duration(
    milliseconds: i64,
    format: &str,
) -> String {
    let total_seconds = milliseconds / 1000;
    let total_minutes = total_seconds / 60;
    let total_hours = total_minutes / 60;
    let total_days = total_hours / 24;
    let total_weeks = total_days / 7;
    let total_months = total_days / 30;
    let total_years = total_days / 365;
    let components = [
        ("yyyy", format!("{total_years:04}")),
        ("yyy", format!("{total_years:03}")),
        ("yy", format!("{total_years:02}")),
        ("y", total_years.to_string()),
        ("MMMM", format!("{total_months:04}")),
        ("MMM", format!("{total_months:03}")),
        ("MM", format!("{total_months:02}")),
        ("M", total_months.to_string()),
        ("www", format!("{total_weeks:03}")),
        ("ww", format!("{total_weeks:02}")),
        ("w", total_weeks.to_string()),
        ("ddd", format!("{total_days:03}")),
        ("dd", format!("{total_days:02}")),
        ("d", total_days.to_string()),
        ("hh", format!("{:02}", total_hours % 24)),
        ("h", (total_hours % 24).to_string()),
        ("mm", format!("{:02}", total_minutes % 60)),
        ("m", (total_minutes % 60).to_string()),
        ("ss", format!("{:02}", total_seconds % 60)),
        ("s", (total_seconds % 60).to_string()),
        ("SSS", format!("{:03}", milliseconds % 1000)),
        ("S", (milliseconds % 1000).to_string()),
    ];

    let mut output = String::new();
    let mut index = 0usize;
    let mut literal = false;
    while index < format.len() {
        let rest = &format[index..];
        if rest.starts_with('\'') {
            literal = !literal;
            index += 1;
            continue;
        }
        if !literal
            && let Some((token, replacement)) =
                components.iter().find(|(token, _)| rest.starts_with(token))
        {
            output.push_str(replacement);
            index += token.len();
            continue;
        }
        let ch = rest.chars().next().expect("non-empty duration format rest");
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

pub(in crate::native::dataview) fn evaluate_currencyformat_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([number] | [number, _]) = args else {
        return DataviewValue::Null;
    };
    let currency = args
        .get(1)
        .map(|arg| value_text(&arg.evaluate(context)))
        .unwrap_or_else(|| "USD".to_string());
    vectorize_unary(number.evaluate(context), |value| {
        numeric_f64(&value)
            .map(|number| {
                DataviewValue::String(format_currency(number, &currency))
            })
            .unwrap_or(DataviewValue::Null)
    })
}

pub(in crate::native::dataview) fn format_currency(
    number: f64,
    currency: &str,
) -> String {
    let symbol = match currency.to_ascii_uppercase().as_str() {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        _ => currency,
    };
    format!("{symbol}{}", format_number_with_commas(number))
}

pub(in crate::native::dataview) fn format_number_with_commas(
    number: f64,
) -> String {
    let sign = if number < 0.0 { "-" } else { "" };
    let abs = number.abs();
    let formatted = format!("{abs:.2}");
    let (whole, fraction) = formatted
        .split_once('.')
        .expect("fixed precision includes decimal");
    let mut grouped = String::new();
    for (index, ch) in whole.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let whole = grouped.chars().rev().collect::<String>();
    format!("{sign}{whole}.{fraction}")
}

pub(in crate::native::dataview) fn localtime_value(
    value: DataviewValue,
) -> DataviewValue {
    match value {
        DataviewValue::Date(_) | DataviewValue::DateTime(_) => value,
        value => date_value(value),
    }
}

pub(in crate::native::dataview) fn meta_value(
    value: DataviewValue,
) -> DataviewValue {
    let DataviewValue::Link(link) = value else {
        return DataviewValue::Null;
    };
    let (path, subpath) = link
        .path
        .split_once('#')
        .map_or((link.path.as_str(), None), |(path, subpath)| {
            (path, Some(subpath))
        });
    let subpath =
        subpath.map(|subpath| subpath.trim_start_matches('^').to_string());
    let link_type = match subpath.as_deref() {
        None => "file",
        Some(raw) if link.path.contains("#^") || raw.starts_with('^') => {
            "block"
        }
        Some(_) => "header",
    };
    let mut object = BTreeMap::new();
    object.insert(
        "display".to_string(),
        link.display
            .clone()
            .map(DataviewValue::String)
            .unwrap_or(DataviewValue::Null),
    );
    object.insert("embed".to_string(), DataviewValue::Bool(link.embed));
    object.insert("path".to_string(), DataviewValue::String(path.to_string()));
    object.insert(
        "subpath".to_string(),
        subpath
            .map(DataviewValue::String)
            .unwrap_or(DataviewValue::Null),
    );
    object.insert(
        "type".to_string(),
        DataviewValue::String(link_type.to_string()),
    );
    DataviewValue::object(object)
}

pub(in crate::native::dataview) fn evaluate_extreme_by_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    target_ordering: Ordering,
) -> DataviewValue {
    let Some((values, parameter, body)) = collection_lambda_args(args, context)
    else {
        return DataviewValue::Null;
    };
    let mut best: Option<(DataviewValue, DataviewValue)> = None;
    for value in values {
        let key =
            body.evaluate(&context.with_variable(parameter, value.clone()));
        match &best {
            None => best = Some((value, key)),
            Some((_, best_key))
                if compare_values(context.vault, &key, best_key)
                    == target_ordering =>
            {
                best = Some((value, key));
            }
            _ => {}
        }
    }
    best.map(|(value, _)| value).unwrap_or(DataviewValue::Null)
}

pub(in crate::native::dataview) fn collection_lambda_args<'a>(
    args: &'a [NativeExpr],
    context: &EvalContext<'_>,
) -> Option<(Vec<DataviewValue>, &'a str, &'a NativeExpr)> {
    let [collection, NativeExpr::Lambda { parameter, body }] = args else {
        return None;
    };
    Some((
        collection_value(collection.evaluate(context)),
        parameter.as_str(),
        body,
    ))
}

pub(in crate::native::dataview) fn collection_value(
    value: DataviewValue,
) -> Vec<DataviewValue> {
    value.into_array_items()
}

pub(in crate::native::dataview) fn aggregate_args(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> Vec<DataviewValue> {
    match args {
        [arg] => collection_value(arg.evaluate(context)),
        args => evaluated_args(args, context),
    }
}

pub(in crate::native::dataview) fn numeric_f64(
    value: &DataviewValue,
) -> Option<f64> {
    match value {
        DataviewValue::Number(number) => number.as_f64(),
        _ => None,
    }
}

pub(in crate::native::dataview) fn integer_value(
    value: &DataviewValue,
) -> Option<i64> {
    match value {
        DataviewValue::Number(number) => number
            .as_i64()
            .or_else(|| {
                number.as_u64().and_then(|value| i64::try_from(value).ok())
            })
            .or_else(|| number.as_f64().map(|value| value as i64)),
        _ => None,
    }
}
