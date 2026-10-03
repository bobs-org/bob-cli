//! Comparison, equality, and arithmetic evaluation.

use super::super::*;
use serde_json::Number;
use std::cmp::Ordering;

pub(in crate::native::dataview) fn compare_operator_value(
    vault: &NativeVault,
    op: NativeBinaryOp,
    left: &DataviewValue,
    right: &DataviewValue,
) -> DataviewValue {
    let equal = values_equal(vault, left, right);
    let value = match op {
        NativeBinaryOp::Equal => equal,
        NativeBinaryOp::NotEqual => !equal,
        NativeBinaryOp::Less if values_include_null(left, right) => false,
        NativeBinaryOp::Less => {
            compare_values(vault, left, right) == Ordering::Less
        }
        NativeBinaryOp::LessEqual if values_include_null(left, right) => false,
        NativeBinaryOp::LessEqual => {
            matches!(
                compare_values(vault, left, right),
                Ordering::Less | Ordering::Equal
            )
        }
        NativeBinaryOp::Greater if values_include_null(left, right) => false,
        NativeBinaryOp::Greater => {
            compare_values(vault, left, right) == Ordering::Greater
        }
        NativeBinaryOp::GreaterEqual if values_include_null(left, right) => {
            false
        }
        NativeBinaryOp::GreaterEqual => {
            matches!(
                compare_values(vault, left, right),
                Ordering::Greater | Ordering::Equal
            )
        }
        NativeBinaryOp::Add
        | NativeBinaryOp::And
        | NativeBinaryOp::Divide
        | NativeBinaryOp::Multiply
        | NativeBinaryOp::Or
        | NativeBinaryOp::Subtract => unreachable!("not a comparison operator"),
    };
    DataviewValue::Bool(value)
}

pub(in crate::native::dataview) fn values_include_null(
    left: &DataviewValue,
    right: &DataviewValue,
) -> bool {
    matches!(left, DataviewValue::Null) || matches!(right, DataviewValue::Null)
}

pub(in crate::native::dataview) fn values_equal(
    vault: &NativeVault,
    left: &DataviewValue,
    right: &DataviewValue,
) -> bool {
    match (left, right) {
        (DataviewValue::Number(left), DataviewValue::Number(right)) => {
            left.as_f64() == right.as_f64()
        }
        (DataviewValue::Link(_), _) => {
            vault.field_value_matches_link(left, &value_text(right))
        }
        (_, DataviewValue::Link(_)) => {
            vault.field_value_matches_link(right, &value_text(left))
        }
        _ => left == right,
    }
}

pub(in crate::native::dataview) fn compare_values(
    vault: &NativeVault,
    left: &DataviewValue,
    right: &DataviewValue,
) -> Ordering {
    if values_equal(vault, left, right) {
        return Ordering::Equal;
    }
    match (left, right) {
        (DataviewValue::Null, _) => Ordering::Greater,
        (_, DataviewValue::Null) => Ordering::Less,
        (DataviewValue::Number(left), DataviewValue::Number(right)) => left
            .as_f64()
            .partial_cmp(&right.as_f64())
            .unwrap_or(Ordering::Equal),
        (DataviewValue::Bool(left), DataviewValue::Bool(right)) => {
            left.cmp(right)
        }
        (DataviewValue::Array(left), DataviewValue::Array(right)) => {
            left.len().cmp(&right.len())
        }
        _ => value_text(left).cmp(&value_text(right)),
    }
}

pub(in crate::native::dataview) fn arithmetic_value(
    op: NativeBinaryOp,
    left: DataviewValue,
    right: DataviewValue,
) -> DataviewValue {
    match op {
        NativeBinaryOp::Add => add_value(left, right),
        NativeBinaryOp::Subtract => {
            numeric_binary_value(left, right, |left, right| left - right)
        }
        NativeBinaryOp::Multiply => multiply_value(left, right),
        NativeBinaryOp::Divide => {
            numeric_binary_value(left, right, |left, right| left / right)
        }
        NativeBinaryOp::And
        | NativeBinaryOp::Equal
        | NativeBinaryOp::Greater
        | NativeBinaryOp::GreaterEqual
        | NativeBinaryOp::Less
        | NativeBinaryOp::LessEqual
        | NativeBinaryOp::NotEqual
        | NativeBinaryOp::Or => unreachable!("not an arithmetic operator"),
    }
}

pub(in crate::native::dataview) fn add_value(
    left: DataviewValue,
    right: DataviewValue,
) -> DataviewValue {
    match (left, right) {
        (DataviewValue::Number(left), DataviewValue::Number(right)) => {
            add_numbers(&left, &right)
        }
        (DataviewValue::Array(left), DataviewValue::Array(right)) => {
            let mut left = unwrap_shared(left);
            left.extend(unwrap_shared(right));
            DataviewValue::array(left)
        }
        (DataviewValue::Array(left), right) => {
            let mut left = unwrap_shared(left);
            left.push(right);
            DataviewValue::array(left)
        }
        (left, DataviewValue::Array(right)) => {
            let mut right = unwrap_shared(right);
            right.insert(0, left);
            DataviewValue::array(right)
        }
        (DataviewValue::Null, _) | (_, DataviewValue::Null) => {
            DataviewValue::Null
        }
        (left, right) => DataviewValue::String(format!(
            "{}{}",
            value_text(&left),
            value_text(&right)
        )),
    }
}

pub(in crate::native::dataview) fn add_numbers(
    left: &Number,
    right: &Number,
) -> DataviewValue {
    if let (Some(left), Some(right)) = (left.as_i64(), right.as_i64())
        && let Some(value) = left.checked_add(right)
    {
        return DataviewValue::Number(Number::from(value));
    }
    number_from_f64(
        left.as_f64().unwrap_or(0.0) + right.as_f64().unwrap_or(0.0),
    )
}

pub(in crate::native::dataview) fn numeric_binary_value(
    left: DataviewValue,
    right: DataviewValue,
    op: impl FnOnce(f64, f64) -> f64,
) -> DataviewValue {
    let (DataviewValue::Number(left), DataviewValue::Number(right)) =
        (left, right)
    else {
        return DataviewValue::Null;
    };
    number_from_f64_smart(op(
        left.as_f64().unwrap_or(0.0),
        right.as_f64().unwrap_or(0.0),
    ))
}

pub(in crate::native::dataview) fn multiply_value(
    left: DataviewValue,
    right: DataviewValue,
) -> DataviewValue {
    match (&left, &right) {
        (DataviewValue::String(text), DataviewValue::Number(number))
        | (DataviewValue::Number(number), DataviewValue::String(text)) => {
            let count = number
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(0);
            DataviewValue::String(text.repeat(count))
        }
        _ => numeric_binary_value(left, right, |left, right| left * right),
    }
}

pub(in crate::native::dataview) fn negate_value(
    value: DataviewValue,
) -> DataviewValue {
    let DataviewValue::Number(number) = value else {
        return DataviewValue::Null;
    };
    if let Some(value) = number.as_i64()
        && let Some(value) = value.checked_neg()
    {
        return DataviewValue::Number(Number::from(value));
    }
    number_from_f64(-number.as_f64().unwrap_or(0.0))
}

pub(in crate::native::dataview) fn number_from_f64(
    value: f64,
) -> DataviewValue {
    Number::from_f64(value)
        .map(DataviewValue::Number)
        .unwrap_or(DataviewValue::Null)
}

pub(in crate::native::dataview) fn number_from_f64_smart(
    value: f64,
) -> DataviewValue {
    if !value.is_finite() {
        return DataviewValue::Null;
    }
    if value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        return DataviewValue::Number(Number::from(value as i64));
    }
    number_from_f64(value)
}

pub(in crate::native::dataview) fn value_text(value: &DataviewValue) -> String {
    match value {
        DataviewValue::Null => String::new(),
        DataviewValue::Bool(value) => value.to_string(),
        DataviewValue::Number(value) => value.to_string(),
        DataviewValue::String(value)
        | DataviewValue::Date(value)
        | DataviewValue::DateTime(value)
        | DataviewValue::Duration(value) => value.clone(),
        DataviewValue::Link(link) => {
            link.display.clone().unwrap_or_else(|| link.path.clone())
        }
        DataviewValue::Array(values) => {
            values.iter().map(value_text).collect::<Vec<_>>().join(", ")
        }
        DataviewValue::Object(_) => {
            serde_json::to_string(&value.to_plain_json()).unwrap_or_default()
        }
    }
}
