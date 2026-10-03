//! String, regex, display, and default-value builtins.

use super::super::*;
use regex::Regex;
use serde_json::Number;
use sha2::{Digest, Sha256};

pub(in crate::native::dataview) fn evaluate_regex_test_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    full_match: bool,
) -> DataviewValue {
    let [pattern, input] = args else {
        return DataviewValue::Null;
    };
    let pattern = value_text(&pattern.evaluate(context));
    let input = input.evaluate(context);
    vectorize_unary(input, |input| {
        let text = value_text(&input);
        let pattern = if full_match {
            format!("^(?:{pattern})$")
        } else {
            pattern.clone()
        };
        Regex::new(&pattern)
            .map(|regex| DataviewValue::Bool(regex.is_match(&text)))
            .unwrap_or(DataviewValue::Bool(false))
    })
}

pub(in crate::native::dataview) fn evaluate_regex_replace_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [input, pattern, replacement] = args else {
        return DataviewValue::Null;
    };
    let pattern = value_text(&pattern.evaluate(context));
    let replacement = value_text(&replacement.evaluate(context));
    let Ok(regex) = Regex::new(&pattern) else {
        return DataviewValue::Null;
    };
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::String(
            regex
                .replace_all(&value_text(&input), replacement.as_str())
                .into_owned(),
        )
    })
}

pub(in crate::native::dataview) fn evaluate_replace_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [input, pattern, replacement] = args else {
        return DataviewValue::Null;
    };
    let pattern = value_text(&pattern.evaluate(context));
    let replacement = value_text(&replacement.evaluate(context));
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::String(
            value_text(&input).replace(&pattern, &replacement),
        )
    })
}

pub(in crate::native::dataview) fn string_map(
    value: DataviewValue,
    function: impl FnOnce(&str) -> String,
) -> DataviewValue {
    DataviewValue::String(function(&value_text(&value)))
}

pub(in crate::native::dataview) fn evaluate_split_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([input, delimiter] | [input, delimiter, _]) = args else {
        return DataviewValue::Null;
    };
    let delimiter = value_text(&delimiter.evaluate(context));
    let limit = args
        .get(2)
        .and_then(|arg| integer_value(&arg.evaluate(context)))
        .map(|value| value.max(0) as usize);
    let Ok(regex) = Regex::new(&delimiter) else {
        return DataviewValue::Null;
    };
    vectorize_unary(input.evaluate(context), |input| {
        let pieces = regex
            .split(&value_text(&input))
            .map(|value| DataviewValue::String(value.to_string()))
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        DataviewValue::array(pieces)
    })
}

pub(in crate::native::dataview) fn evaluate_string_predicate_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    predicate: impl Fn(&str, &str) -> bool + Copy,
) -> DataviewValue {
    let [input, needle] = args else {
        return DataviewValue::Null;
    };
    let needle = value_text(&needle.evaluate(context));
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::Bool(predicate(&value_text(&input), &needle))
    })
}

#[derive(Clone, Copy)]
pub(in crate::native::dataview) enum PadSide {
    Left,
    Right,
}

pub(in crate::native::dataview) fn evaluate_pad_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    side: PadSide,
) -> DataviewValue {
    let ([input, length] | [input, length, _]) = args else {
        return DataviewValue::Null;
    };
    let length =
        integer_value(&length.evaluate(context)).unwrap_or(0).max(0) as usize;
    let padding = args
        .get(2)
        .map(|arg| value_text(&arg.evaluate(context)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| " ".to_string());
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::String(pad_text(
            &value_text(&input),
            length,
            &padding,
            side,
        ))
    })
}

pub(in crate::native::dataview) fn pad_text(
    input: &str,
    length: usize,
    padding: &str,
    side: PadSide,
) -> String {
    let current = input.chars().count();
    if current >= length {
        return input.to_string();
    }
    let mut pad = String::new();
    while pad.chars().count() < length - current {
        pad.push_str(padding);
    }
    let pad = pad.chars().take(length - current).collect::<String>();
    match side {
        PadSide::Left => format!("{pad}{input}"),
        PadSide::Right => format!("{input}{pad}"),
    }
}

pub(in crate::native::dataview) fn evaluate_substring_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([input, start] | [input, start, _]) = args else {
        return DataviewValue::Null;
    };
    let start = integer_value(&start.evaluate(context)).unwrap_or(0);
    let end = args
        .get(2)
        .and_then(|arg| integer_value(&arg.evaluate(context)));
    vectorize_unary(input.evaluate(context), |input| {
        let text = value_text(&input);
        let chars = text.chars().collect::<Vec<_>>();
        let (start, end) = slice_bounds(chars.len(), start, end);
        DataviewValue::String(chars[start..end].iter().collect())
    })
}

pub(in crate::native::dataview) fn evaluate_truncate_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([input, length] | [input, length, _]) = args else {
        return DataviewValue::Null;
    };
    let length =
        integer_value(&length.evaluate(context)).unwrap_or(0).max(0) as usize;
    let suffix = args
        .get(2)
        .map(|arg| value_text(&arg.evaluate(context)))
        .unwrap_or_else(|| "...".to_string());
    vectorize_unary(input.evaluate(context), |input| {
        DataviewValue::String(truncate_text(
            &value_text(&input),
            length,
            &suffix,
        ))
    })
}

pub(in crate::native::dataview) fn truncate_text(
    input: &str,
    length: usize,
    suffix: &str,
) -> String {
    let input_len = input.chars().count();
    if input_len <= length {
        return input.to_string();
    }
    let suffix_len = suffix.chars().count();
    if suffix_len >= length {
        return suffix.chars().take(length).collect();
    }
    let prefix = input.chars().take(length - suffix_len).collect::<String>();
    format!("{prefix}{suffix}")
}

pub(in crate::native::dataview) fn evaluate_default_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    vectorized: bool,
) -> DataviewValue {
    let [value, fallback] = args else {
        return DataviewValue::Null;
    };
    let value = value.evaluate(context);
    let fallback = fallback.evaluate(context);
    if vectorized {
        vectorize_binary(value, fallback, default_pair)
    } else {
        default_pair(value, fallback)
    }
}

pub(in crate::native::dataview) fn default_pair(
    value: DataviewValue,
    fallback: DataviewValue,
) -> DataviewValue {
    if matches!(value, DataviewValue::Null) {
        fallback
    } else {
        value
    }
}

pub(in crate::native::dataview) fn display_value(
    value: DataviewValue,
) -> DataviewValue {
    DataviewValue::String(display_text(&value))
}

pub(in crate::native::dataview) fn display_text(
    value: &DataviewValue,
) -> String {
    match value {
        DataviewValue::Link(link) => {
            link.display.clone().unwrap_or_else(|| {
                note_stem(&link.path).unwrap_or_else(|| link.path.clone())
            })
        }
        DataviewValue::Array(values) => values
            .iter()
            .map(display_text)
            .collect::<Vec<_>>()
            .join(", "),
        DataviewValue::String(value) => markdown_display_text(value),
        value => value_text(value),
    }
}

pub(in crate::native::dataview) fn markdown_display_text(
    value: &str,
) -> String {
    let wikilink =
        Regex::new(r"!?\[\[([^\]|#]+)(?:#[^\]|]+)?(?:\|([^\]]+))?\]\]")
            .expect("valid wikilink display regex");
    let markdown_link = Regex::new(r"\[([^\]]+)\]\([^)]+\)")
        .expect("valid markdown link regex");
    let emphasis = Regex::new(r"[*_`]").expect("valid emphasis cleanup regex");
    let value =
        wikilink.replace_all(value, |captures: &regex::Captures<'_>| {
            captures
                .get(2)
                .or_else(|| captures.get(1))
                .map(|match_| match_.as_str())
                .unwrap_or_default()
                .to_string()
        });
    let value = markdown_link.replace_all(&value, "$1");
    emphasis.replace_all(&value, "").into_owned()
}

pub(in crate::native::dataview) fn evaluate_choice_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [condition, left, right] = args else {
        return DataviewValue::Null;
    };
    if condition.evaluate(context).is_truthy() {
        left.evaluate(context)
    } else {
        right.evaluate(context)
    }
}

pub(in crate::native::dataview) fn evaluate_hash_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let values = evaluated_args(args, context);
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update(value_text(&value));
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let value = u64::from_be_bytes(bytes) & 0x7fff_ffff_ffff_ffff;
    DataviewValue::Number(Number::from(value))
}
