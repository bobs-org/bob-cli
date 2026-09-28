//! Collection builtins: filter, map, sort, and set operations.

use super::super::*;
use serde_json::Number;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(in crate::native::dataview) enum ContainsMode {
    Contains,
    Exact,
    Insensitive,
}

pub(in crate::native::dataview) fn evaluate_contains_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    mode: ContainsMode,
) -> DataviewValue {
    let [container, needle] = args else {
        return DataviewValue::Null;
    };
    let container = container.evaluate(context);
    let needle = needle.evaluate(context);
    DataviewValue::Bool(contains_value(
        context.vault,
        &container,
        &needle,
        mode,
    ))
}

pub(in crate::native::dataview) fn contains_value(
    vault: &NativeVault,
    container: &DataviewValue,
    needle: &DataviewValue,
    mode: ContainsMode,
) -> bool {
    match container {
        DataviewValue::Object(object) => {
            let needle = value_text(needle);
            match mode {
                ContainsMode::Insensitive => {
                    object.keys().any(|key| key.eq_ignore_ascii_case(&needle))
                }
                ContainsMode::Contains | ContainsMode::Exact => {
                    object.contains_key(&needle)
                }
            }
        }
        DataviewValue::Array(values) => values.iter().any(|value| match mode {
            ContainsMode::Insensitive => {
                value_text(value).eq_ignore_ascii_case(&value_text(needle))
            }
            ContainsMode::Contains | ContainsMode::Exact => {
                values_equal(vault, value, needle)
            }
        }),
        value => {
            let haystack = value_text(value);
            let needle = value_text(needle);
            match mode {
                ContainsMode::Insensitive => haystack
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase()),
                ContainsMode::Contains | ContainsMode::Exact => {
                    haystack.contains(&needle)
                }
            }
        }
    }
}

pub(in crate::native::dataview) fn evaluate_containsword_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [container, needle] = args else {
        return DataviewValue::Null;
    };
    let container = container.evaluate(context);
    let needle = value_text(&needle.evaluate(context)).to_ascii_lowercase();
    match container {
        DataviewValue::Array(values) => DataviewValue::Array(
            values
                .iter()
                .map(|value| {
                    DataviewValue::Bool(text_has_word(
                        &value_text(value),
                        &needle,
                    ))
                })
                .collect(),
        ),
        value => {
            DataviewValue::Bool(text_has_word(&value_text(&value), &needle))
        }
    }
}

pub(in crate::native::dataview) fn text_has_word(
    text: &str,
    needle: &str,
) -> bool {
    text.split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .any(|word| word.eq_ignore_ascii_case(needle))
}

pub(in crate::native::dataview) fn evaluate_extract_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [object, keys @ ..] = args else {
        return DataviewValue::Null;
    };
    let DataviewValue::Object(object) = object.evaluate(context) else {
        return DataviewValue::Null;
    };
    let mut extracted = BTreeMap::new();
    for key in keys {
        let key = value_text(&key.evaluate(context));
        if let Some(value) = object.get(&key) {
            extracted.insert(key, value.clone());
        }
    }
    DataviewValue::Object(extracted)
}

pub(in crate::native::dataview) fn evaluate_sort_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    let mut values = collection_value(arg.evaluate(context));
    values.sort_by(|left, right| compare_values(context.vault, left, right));
    DataviewValue::Array(values)
}

pub(in crate::native::dataview) fn evaluate_reverse_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    let mut values = collection_value(arg.evaluate(context));
    values.reverse();
    DataviewValue::Array(values)
}

pub(in crate::native::dataview) fn length_value(
    value: DataviewValue,
) -> DataviewValue {
    let length = match value {
        DataviewValue::Array(values) => values.len(),
        DataviewValue::Object(values) => values.len(),
        value => value_text(&value).chars().count(),
    };
    DataviewValue::Number(Number::from(length as u64))
}

pub(in crate::native::dataview) fn evaluate_nonnull_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    DataviewValue::Array(
        collection_value(arg.evaluate(context))
            .into_iter()
            .filter(|value| !matches!(value, DataviewValue::Null))
            .collect(),
    )
}

pub(in crate::native::dataview) fn evaluate_firstvalue_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    collection_value(arg.evaluate(context))
        .into_iter()
        .find(|value| !matches!(value, DataviewValue::Null))
        .unwrap_or(DataviewValue::Null)
}

#[derive(Debug, Clone, Copy)]
pub(in crate::native::dataview) enum Quantifier {
    All,
    Any,
    None,
}

pub(in crate::native::dataview) fn evaluate_filter_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let Some((values, parameter, body)) = collection_lambda_args(args, context)
    else {
        return DataviewValue::Null;
    };
    DataviewValue::Array(
        values
            .into_iter()
            .filter(|value| {
                body.evaluate(&context.with_variable(parameter, value.clone()))
                    .is_truthy()
            })
            .collect(),
    )
}

pub(in crate::native::dataview) fn evaluate_map_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let Some((values, parameter, body)) = collection_lambda_args(args, context)
    else {
        return DataviewValue::Null;
    };
    DataviewValue::Array(
        values
            .into_iter()
            .map(|value| {
                body.evaluate(&context.with_variable(parameter, value))
            })
            .collect(),
    )
}

pub(in crate::native::dataview) fn evaluate_quantifier_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    quantifier: Quantifier,
) -> DataviewValue {
    let values = match args {
        [collection] => collection_value(collection.evaluate(context)),
        [collection, NativeExpr::Lambda { parameter, body }] => {
            let values = collection_value(collection.evaluate(context));
            let projected = values
                .into_iter()
                .map(|value| {
                    body.evaluate(&context.with_variable(parameter, value))
                })
                .collect::<Vec<_>>();
            return DataviewValue::Bool(match quantifier {
                Quantifier::All => {
                    projected.iter().all(DataviewValue::is_truthy)
                }
                Quantifier::Any => {
                    projected.iter().any(DataviewValue::is_truthy)
                }
                Quantifier::None => {
                    !projected.iter().any(DataviewValue::is_truthy)
                }
            });
        }
        _ => {
            let values = evaluated_args(args, context);
            return DataviewValue::Bool(match quantifier {
                Quantifier::All => values.iter().all(DataviewValue::is_truthy),
                Quantifier::Any => values.iter().any(DataviewValue::is_truthy),
                Quantifier::None => {
                    !values.iter().any(DataviewValue::is_truthy)
                }
            });
        }
    };

    DataviewValue::Bool(match quantifier {
        Quantifier::All => values.iter().all(DataviewValue::is_truthy),
        Quantifier::Any => values.iter().any(DataviewValue::is_truthy),
        Quantifier::None => !values.iter().any(DataviewValue::is_truthy),
    })
}

pub(in crate::native::dataview) fn evaluate_join_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([collection] | [collection, _]) = args else {
        return DataviewValue::Null;
    };
    let separator = args
        .get(1)
        .map(|arg| value_text(&arg.evaluate(context)))
        .unwrap_or_else(|| ", ".to_string());
    let values = collection_value(collection.evaluate(context));
    DataviewValue::String(
        values
            .iter()
            .map(value_text)
            .collect::<Vec<_>>()
            .join(&separator),
    )
}

pub(in crate::native::dataview) fn evaluate_unique_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    let mut unique = Vec::new();
    for value in collection_value(arg.evaluate(context)) {
        if !unique
            .iter()
            .any(|existing| values_equal(context.vault, existing, &value))
        {
            unique.push(value);
        }
    }
    DataviewValue::Array(unique)
}

pub(in crate::native::dataview) fn evaluate_flat_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([collection] | [collection, _]) = args else {
        return DataviewValue::Null;
    };
    let depth = args
        .get(1)
        .and_then(|arg| integer_value(&arg.evaluate(context)))
        .unwrap_or(1)
        .max(0) as usize;
    let mut output = Vec::new();
    flatten_values(
        collection_value(collection.evaluate(context)),
        depth,
        &mut output,
    );
    DataviewValue::Array(output)
}

pub(in crate::native::dataview) fn flatten_values(
    values: Vec<DataviewValue>,
    depth: usize,
    output: &mut Vec<DataviewValue>,
) {
    for value in values {
        match value {
            DataviewValue::Array(values) if depth > 0 => {
                flatten_values(values, depth - 1, output);
            }
            value => output.push(value),
        }
    }
}

pub(in crate::native::dataview) fn evaluate_slice_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([collection] | [collection, _] | [collection, _, _]) = args else {
        return DataviewValue::Null;
    };
    let start = args
        .get(1)
        .and_then(|arg| integer_value(&arg.evaluate(context)))
        .unwrap_or(0);
    let end = args
        .get(2)
        .and_then(|arg| integer_value(&arg.evaluate(context)));
    let values = collection_value(collection.evaluate(context));
    let (start, end) = slice_bounds(values.len(), start, end);
    DataviewValue::Array(values[start..end].to_vec())
}

pub(in crate::native::dataview) fn slice_bounds(
    length: usize,
    start: i64,
    end: Option<i64>,
) -> (usize, usize) {
    let length_i64 = length as i64;
    let start = if start < 0 { length_i64 + start } else { start }
        .clamp(0, length_i64) as usize;
    let end = end.unwrap_or(length_i64);
    let end = if end < 0 { length_i64 + end } else { end }
        .clamp(start as i64, length_i64) as usize;
    (start, end)
}
