//! Scalar coercion, object, and vectorized-unary builtins.

use super::super::*;
use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use regex::Regex;
use std::collections::BTreeMap;

pub(in crate::native::dataview) fn evaluate_unary_scalar_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    function: impl FnOnce(DataviewValue) -> DataviewValue,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    function(arg.evaluate(context))
}

pub(in crate::native::dataview) fn evaluate_unary_vector_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    function: impl Fn(DataviewValue) -> DataviewValue,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    vectorize_unary(arg.evaluate(context), function)
}

pub(in crate::native::dataview) fn evaluate_object_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let values = evaluated_args(args, context);
    let mut object = BTreeMap::new();
    for pair in values.chunks_exact(2) {
        object.insert(value_text(&pair[0]), pair[1].clone());
    }
    DataviewValue::Object(object)
}

pub(in crate::native::dataview) fn date_value(
    value: DataviewValue,
) -> DataviewValue {
    match value {
        DataviewValue::Date(_) | DataviewValue::DateTime(_) => value,
        DataviewValue::Link(link) => date_from_text(&link.path)
            .or_else(|| {
                note_stem(&link.path).and_then(|stem| date_from_text(&stem))
            })
            .unwrap_or(DataviewValue::Null),
        value => {
            date_from_text(&value_text(&value)).unwrap_or(DataviewValue::Null)
        }
    }
}

pub(in crate::native::dataview) fn date_from_text(
    text: &str,
) -> Option<DataviewValue> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("today") {
        return Some(DataviewValue::Date(Utc::now().date_naive().to_string()));
    }
    if trimmed.eq_ignore_ascii_case("now") {
        return Some(DataviewValue::DateTime(
            Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        ));
    }
    if DateTime::parse_from_rfc3339(trimmed).is_ok()
        || NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S").is_ok()
    {
        return Some(DataviewValue::DateTime(trimmed.to_string()));
    }
    if NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").is_ok() {
        return Some(DataviewValue::Date(trimmed.to_string()));
    }

    let date = Regex::new(r"\d{4}-\d{2}-\d{2}").expect("valid date regex");
    date.find(trimmed)
        .and_then(|match_| date_from_text(match_.as_str()))
}

pub(in crate::native::dataview) fn duration_value(
    value: DataviewValue,
) -> DataviewValue {
    match value {
        DataviewValue::Duration(_) => value,
        value => duration_text_to_iso(&value_text(&value))
            .map(DataviewValue::Duration)
            .unwrap_or(DataviewValue::Null),
    }
}

pub(in crate::native::dataview) fn duration_text_to_iso(
    text: &str,
) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('P') && duration_to_millis(trimmed).is_some() {
        return Some(trimmed.to_string());
    }

    let unit = Regex::new(
        r"(?i)([-+]?\d+)\s*(milliseconds?|msecs?|ms|seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d|weeks?|w|months?|mos?|mo|years?|yrs?|y)",
    )
    .expect("valid duration regex");
    let mut years = 0i64;
    let mut months = 0i64;
    let mut days = 0i64;
    let mut hours = 0i64;
    let mut minutes = 0i64;
    let mut seconds = 0i64;
    let mut milliseconds = 0i64;
    let mut matched = false;

    for captures in unit.captures_iter(trimmed) {
        matched = true;
        let amount = captures[1].parse::<i64>().ok()?;
        match captures[2].to_ascii_lowercase().as_str() {
            "millisecond" | "milliseconds" | "msec" | "msecs" | "ms" => {
                milliseconds += amount;
            }
            "second" | "seconds" | "sec" | "secs" | "s" => seconds += amount,
            "minute" | "minutes" | "min" | "mins" | "m" => minutes += amount,
            "hour" | "hours" | "hr" | "hrs" | "h" => hours += amount,
            "day" | "days" | "d" => days += amount,
            "week" | "weeks" | "w" => days += amount * 7,
            "month" | "months" | "mo" | "mos" => months += amount,
            "year" | "years" | "yr" | "yrs" | "y" => years += amount,
            _ => return None,
        }
    }
    if !matched {
        return None;
    }

    let mut output = String::from("P");
    if years != 0 {
        output.push_str(&format!("{years}Y"));
    }
    if months != 0 {
        output.push_str(&format!("{months}M"));
    }
    if days != 0 {
        output.push_str(&format!("{days}D"));
    }
    if hours != 0 || minutes != 0 || seconds != 0 || milliseconds != 0 {
        output.push('T');
        if hours != 0 {
            output.push_str(&format!("{hours}H"));
        }
        if minutes != 0 {
            output.push_str(&format!("{minutes}M"));
        }
        if milliseconds != 0 {
            let whole_seconds = seconds + milliseconds / 1000;
            let millis = milliseconds.rem_euclid(1000);
            if millis == 0 {
                if whole_seconds != 0 {
                    output.push_str(&format!("{whole_seconds}S"));
                }
            } else {
                output.push_str(&format!("{whole_seconds}.{millis:03}S"));
            }
        } else if seconds != 0 {
            output.push_str(&format!("{seconds}S"));
        }
    }
    if output == "P" {
        output.push_str("T0S");
    }
    Some(output)
}

pub(in crate::native::dataview) fn number_value(
    value: DataviewValue,
) -> DataviewValue {
    match value {
        DataviewValue::Number(_) => value,
        value => {
            let number = Regex::new(r"[-+]?\d+(?:\.\d+)?")
                .expect("valid number extraction regex");
            number
                .find(&value_text(&value))
                .and_then(|match_| {
                    parse_expression_number(match_.as_str()).ok()
                })
                .map(DataviewValue::Number)
                .unwrap_or(DataviewValue::Null)
        }
    }
}

pub(in crate::native::dataview) fn string_value(
    value: DataviewValue,
) -> DataviewValue {
    DataviewValue::String(match value {
        DataviewValue::Duration(value) => duration_to_human(&value),
        value => value_text(&value),
    })
}

pub(in crate::native::dataview) fn duration_to_human(value: &str) -> String {
    let Some(milliseconds) = duration_to_millis(value) else {
        return value.to_string();
    };
    let mut seconds = milliseconds / 1000;
    let days = seconds / 86_400;
    seconds %= 86_400;
    let hours = seconds / 3_600;
    seconds %= 3_600;
    let minutes = seconds / 60;
    seconds %= 60;
    for (amount, singular) in [
        (days, "day"),
        (hours, "hour"),
        (minutes, "minute"),
        (seconds, "second"),
    ] {
        if amount != 0 {
            let suffix = if amount == 1 { "" } else { "s" };
            return format!("{amount} {singular}{suffix}");
        }
    }
    "0 seconds".to_string()
}

pub(in crate::native::dataview) fn typeof_value(
    value: DataviewValue,
) -> DataviewValue {
    let name = match value {
        DataviewValue::Null => "null",
        DataviewValue::Bool(_) => "boolean",
        DataviewValue::Number(_) => "number",
        DataviewValue::String(_) => "string",
        DataviewValue::Date(_) => "date",
        DataviewValue::DateTime(_) => "date",
        DataviewValue::Duration(_) => "duration",
        DataviewValue::Link(_) => "link",
        DataviewValue::Array(_) => "array",
        DataviewValue::Object(_) => "object",
    };
    DataviewValue::String(name.to_string())
}
