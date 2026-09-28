//! Link, embed, numeric, and aggregate builtins.

use super::super::*;
use std::cmp::Ordering;

pub(in crate::native::dataview) fn evaluate_link_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    embed: bool,
) -> DataviewValue {
    let values = evaluated_args(args, context);
    let ([path] | [path, _]) = values.as_slice() else {
        return DataviewValue::Null;
    };
    let display = values
        .get(1)
        .map(value_text)
        .filter(|value| !value.is_empty());
    let path_text = value_text(path);
    DataviewValue::Link(DataviewLink::new(
        normalized_link_literal_path(&path_text),
        display,
        embed,
        path_text,
    ))
}

pub(in crate::native::dataview) fn evaluate_external_link_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let values = evaluated_args(args, context);
    let ([url] | [url, _]) = values.as_slice() else {
        return DataviewValue::Null;
    };
    let url_text = value_text(url);
    DataviewValue::Link(DataviewLink::new(
        url_text.clone(),
        values
            .get(1)
            .map(value_text)
            .filter(|value| !value.is_empty()),
        false,
        url_text,
    ))
}

pub(in crate::native::dataview) fn evaluate_embed_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    match evaluated_args(args, context).as_slice() {
        [DataviewValue::Link(link)] => {
            let mut link = link.clone();
            link.embed = true;
            DataviewValue::Link(link)
        }
        [DataviewValue::Link(link), embed] => {
            let mut link = link.clone();
            link.embed = embed.is_truthy();
            DataviewValue::Link(link)
        }
        [_, ..] => evaluate_link_call(args, context, true),
        _ => DataviewValue::Null,
    }
}

pub(in crate::native::dataview) fn evaluate_round_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let ([value] | [value, _]) = args else {
        return DataviewValue::Null;
    };
    let digits = args
        .get(1)
        .map(|arg| integer_value(&arg.evaluate(context)))
        .unwrap_or(Some(0))
        .unwrap_or(0);
    vectorize_unary(value.evaluate(context), |value| {
        let Some(number) = numeric_f64(&value) else {
            return DataviewValue::Null;
        };
        let factor = 10_f64.powi(digits as i32);
        number_from_f64_smart((number * factor).round() / factor)
    })
}

pub(in crate::native::dataview) fn evaluate_numeric_round_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    function: impl Fn(f64) -> f64 + Copy,
) -> DataviewValue {
    evaluate_unary_vector_call(args, context, |value| {
        numeric_f64(&value)
            .map(|value| number_from_f64_smart(function(value)))
            .unwrap_or(DataviewValue::Null)
    })
}

pub(in crate::native::dataview) fn evaluate_extreme_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    target_ordering: Ordering,
) -> DataviewValue {
    let values = aggregate_args(args, context);
    let mut best: Option<DataviewValue> = None;
    for value in values {
        match &best {
            None => best = Some(value),
            Some(best_value)
                if compare_values(context.vault, &value, best_value)
                    == target_ordering =>
            {
                best = Some(value);
            }
            _ => {}
        }
    }
    best.unwrap_or(DataviewValue::Null)
}

#[derive(Clone, Copy)]
pub(in crate::native::dataview) enum NumericAggregate {
    Average,
    Product,
    Sum,
}

pub(in crate::native::dataview) fn evaluate_numeric_aggregate_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
    aggregate: NumericAggregate,
) -> DataviewValue {
    let [arg] = args else {
        return DataviewValue::Null;
    };
    let values = collection_value(arg.evaluate(context));
    let numbers = values.iter().filter_map(numeric_f64).collect::<Vec<_>>();
    if numbers.is_empty() {
        return DataviewValue::Null;
    }
    let value = match aggregate {
        NumericAggregate::Average => {
            numbers.iter().sum::<f64>() / numbers.len() as f64
        }
        NumericAggregate::Product => numbers.iter().product(),
        NumericAggregate::Sum => numbers.iter().sum(),
    };
    number_from_f64_smart(value)
}

pub(in crate::native::dataview) fn evaluate_reduce_call(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    let [collection, operator] = args else {
        return DataviewValue::Null;
    };
    let mut values = collection_value(collection.evaluate(context)).into_iter();
    let Some(mut result) = values.next() else {
        return DataviewValue::Null;
    };
    let operator = value_text(&operator.evaluate(context));
    for value in values {
        result = match operator.as_str() {
            "+" => add_value(result, value),
            "-" => {
                numeric_binary_value(result, value, |left, right| left - right)
            }
            "*" => multiply_value(result, value),
            "/" => {
                numeric_binary_value(result, value, |left, right| left / right)
            }
            "&" => DataviewValue::Bool(result.is_truthy() && value.is_truthy()),
            "|" => DataviewValue::Bool(result.is_truthy() || value.is_truthy()),
            _ => return DataviewValue::Null,
        };
    }
    result
}
