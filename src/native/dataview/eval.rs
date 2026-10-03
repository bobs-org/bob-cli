//! Native expression evaluation and builtin call dispatch.

use super::*;
use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap};

impl NativeExpression {
    pub(super) fn new(tokens: Vec<NativeToken>) -> Result<Self, String> {
        if tokens.is_empty() {
            return Err("expected expression".to_string());
        }
        let raw = expression_tokens_to_string(&tokens);
        let expr = parse_native_expression(tokens)?;
        Ok(Self { raw, expr })
    }

    pub(super) fn where_clause(
        tokens: Vec<NativeToken>,
    ) -> Result<Self, String> {
        Self::new(tokens)
    }

    pub(super) fn evaluate(&self, context: &EvalContext<'_>) -> DataviewValue {
        self.expr.evaluate(context)
    }
}

impl NativeExpr {
    pub(super) fn evaluate(&self, context: &EvalContext<'_>) -> DataviewValue {
        match self {
            Self::Array(values) => DataviewValue::array(
                values.iter().map(|value| value.evaluate(context)).collect(),
            ),
            Self::Binary { op, left, right } => match op {
                NativeBinaryOp::And => {
                    let left = left.evaluate(context);
                    if !left.is_truthy() {
                        DataviewValue::Bool(false)
                    } else {
                        DataviewValue::Bool(right.evaluate(context).is_truthy())
                    }
                }
                NativeBinaryOp::Or => {
                    let left = left.evaluate(context);
                    if left.is_truthy() {
                        DataviewValue::Bool(true)
                    } else {
                        DataviewValue::Bool(right.evaluate(context).is_truthy())
                    }
                }
                NativeBinaryOp::Equal
                | NativeBinaryOp::NotEqual
                | NativeBinaryOp::Less
                | NativeBinaryOp::LessEqual
                | NativeBinaryOp::Greater
                | NativeBinaryOp::GreaterEqual => {
                    let left = left.evaluate(context);
                    let right = right.evaluate(context);
                    compare_operator_value(context.vault, *op, &left, &right)
                }
                NativeBinaryOp::Add
                | NativeBinaryOp::Subtract
                | NativeBinaryOp::Multiply
                | NativeBinaryOp::Divide => {
                    let left = left.evaluate(context);
                    let right = right.evaluate(context);
                    arithmetic_value(*op, left, right)
                }
            },
            Self::Call { function, args } => {
                evaluate_call(function, args, context)
            }
            Self::GetAttr { target, field } => {
                let target = target.evaluate(context);
                context.vault.attr_value(&target, field)
            }
            Self::Identifier(identifier) if identifier == "this" => {
                context.vault.page_value(
                    context.vault.origin_index.unwrap_or(context.page_index),
                )
            }
            Self::Identifier(identifier) => context
                .variables
                .get(identifier)
                .cloned()
                .or_else(|| context.row_field_value(identifier))
                .unwrap_or_else(|| {
                    context
                        .vault
                        .page_field_value(context.page_index, identifier)
                }),
            Self::Lambda { .. } => DataviewValue::Null,
            Self::LinkLiteral(raw) => DataviewValue::Link(
                native_expression_link(raw)
                    .map(|mut link| {
                        if let Some(path) = context
                            .vault
                            .index
                            .resolve_target_path(&link.raw_target)
                        {
                            link.path = path;
                        }
                        link
                    })
                    .unwrap_or_else(|| DataviewLink::page(raw)),
            ),
            Self::Literal(value) => value.clone(),
            Self::Object(fields) => DataviewValue::object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.evaluate(context)))
                    .collect(),
            ),
            Self::Unary { op, expr } => {
                let value = expr.evaluate(context);
                match op {
                    NativeUnaryOp::Not => {
                        DataviewValue::Bool(!value.is_truthy())
                    }
                    NativeUnaryOp::Negate => negate_value(value),
                }
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct EvalContext<'a> {
    pub(super) vault: &'a NativeVault,
    pub(super) page_index: usize,
    pub(super) row_value: &'a DataviewValue,
    pub(super) variables: Cow<'a, BTreeMap<String, DataviewValue>>,
}

impl<'a> EvalContext<'a> {
    pub(super) fn row_field_value(&self, field: &str) -> Option<DataviewValue> {
        self.row_value.as_object_field(field).cloned()
    }

    pub(super) fn with_variable(
        &self,
        name: &str,
        value: DataviewValue,
    ) -> EvalContext<'a> {
        let mut variables = self.variables.clone().into_owned();
        variables.insert(name.to_string(), value);
        EvalContext {
            vault: self.vault,
            page_index: self.page_index,
            row_value: self.row_value,
            variables: Cow::Owned(variables),
        }
    }
}

pub(super) fn evaluate_call(
    function: &str,
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> DataviewValue {
    match function.to_ascii_lowercase().as_str() {
        "object" => evaluate_object_call(args, context),
        "list" | "array" => DataviewValue::array(evaluated_args(args, context)),
        "date" => evaluate_unary_vector_call(args, context, date_value),
        "dur" => evaluate_unary_vector_call(args, context, duration_value),
        "number" => evaluate_unary_vector_call(args, context, number_value),
        "string" => evaluate_unary_vector_call(args, context, string_value),
        "link" => evaluate_link_call(args, context, false),
        "elink" => evaluate_external_link_call(args, context),
        "embed" => evaluate_embed_call(args, context),
        "typeof" => evaluate_unary_scalar_call(args, context, typeof_value),
        "round" => evaluate_round_call(args, context),
        "trunc" => evaluate_numeric_round_call(args, context, f64::trunc),
        "floor" => evaluate_numeric_round_call(args, context, f64::floor),
        "ceil" => evaluate_numeric_round_call(args, context, f64::ceil),
        "min" => evaluate_extreme_call(args, context, Ordering::Less),
        "max" => evaluate_extreme_call(args, context, Ordering::Greater),
        "sum" => evaluate_numeric_aggregate_call(
            args,
            context,
            NumericAggregate::Sum,
        ),
        "product" => evaluate_numeric_aggregate_call(
            args,
            context,
            NumericAggregate::Product,
        ),
        "reduce" => evaluate_reduce_call(args, context),
        "average" => evaluate_numeric_aggregate_call(
            args,
            context,
            NumericAggregate::Average,
        ),
        "contains" => {
            evaluate_contains_call(args, context, ContainsMode::Contains)
        }
        "icontains" => {
            evaluate_contains_call(args, context, ContainsMode::Insensitive)
        }
        "econtains" => {
            evaluate_contains_call(args, context, ContainsMode::Exact)
        }
        "containsword" => evaluate_containsword_call(args, context),
        "extract" => evaluate_extract_call(args, context),
        "sort" => evaluate_sort_call(args, context),
        "reverse" => evaluate_reverse_call(args, context),
        "length" => evaluate_unary_scalar_call(args, context, length_value),
        "nonnull" => evaluate_nonnull_call(args, context),
        "firstvalue" => evaluate_firstvalue_call(args, context),
        "filter" => evaluate_filter_call(args, context),
        "map" => evaluate_map_call(args, context),
        "any" => evaluate_quantifier_call(args, context, Quantifier::Any),
        "all" => evaluate_quantifier_call(args, context, Quantifier::All),
        "none" => evaluate_quantifier_call(args, context, Quantifier::None),
        "join" => evaluate_join_call(args, context),
        "unique" => evaluate_unique_call(args, context),
        "flat" => evaluate_flat_call(args, context),
        "slice" => evaluate_slice_call(args, context),
        "regextest" => evaluate_regex_test_call(args, context, false),
        "regexmatch" => evaluate_regex_test_call(args, context, true),
        "regexreplace" => evaluate_regex_replace_call(args, context),
        "replace" => evaluate_replace_call(args, context),
        "lower" => evaluate_unary_vector_call(args, context, |value| {
            string_map(value, str::to_lowercase)
        }),
        "upper" => evaluate_unary_vector_call(args, context, |value| {
            string_map(value, str::to_uppercase)
        }),
        "split" => evaluate_split_call(args, context),
        "startswith" => {
            evaluate_string_predicate_call(args, context, |text, prefix| {
                text.starts_with(prefix)
            })
        }
        "endswith" => {
            evaluate_string_predicate_call(args, context, |text, suffix| {
                text.ends_with(suffix)
            })
        }
        "padleft" => evaluate_pad_call(args, context, PadSide::Left),
        "padright" => evaluate_pad_call(args, context, PadSide::Right),
        "substring" => evaluate_substring_call(args, context),
        "truncate" => evaluate_truncate_call(args, context),
        "default" => evaluate_default_call(args, context, true),
        "ldefault" => evaluate_default_call(args, context, false),
        "display" => evaluate_unary_vector_call(args, context, display_value),
        "choice" => evaluate_choice_call(args, context),
        "hash" => evaluate_hash_call(args, context),
        "striptime" => {
            evaluate_unary_vector_call(args, context, striptime_value)
        }
        "dateformat" => evaluate_dateformat_call(args, context),
        "durationformat" => evaluate_durationformat_call(args, context),
        "currencyformat" => evaluate_currencyformat_call(args, context),
        "localtime" => {
            evaluate_unary_vector_call(args, context, localtime_value)
        }
        "meta" => evaluate_unary_vector_call(args, context, meta_value),
        "minby" => evaluate_extreme_by_call(args, context, Ordering::Less),
        "maxby" => evaluate_extreme_by_call(args, context, Ordering::Greater),
        _ => DataviewValue::Null,
    }
}

pub(super) fn evaluated_args(
    args: &[NativeExpr],
    context: &EvalContext<'_>,
) -> Vec<DataviewValue> {
    args.iter().map(|arg| arg.evaluate(context)).collect()
}

pub(super) fn vectorize_unary(
    value: DataviewValue,
    function: impl Fn(DataviewValue) -> DataviewValue,
) -> DataviewValue {
    match value {
        DataviewValue::Array(values) => DataviewValue::array(
            unwrap_shared(values).into_iter().map(function).collect(),
        ),
        value => function(value),
    }
}

pub(super) fn vectorize_binary(
    left: DataviewValue,
    right: DataviewValue,
    function: impl Fn(DataviewValue, DataviewValue) -> DataviewValue + Copy,
) -> DataviewValue {
    match (left, right) {
        (DataviewValue::Array(left), DataviewValue::Array(right)) => {
            let fallback = DataviewValue::Null;
            DataviewValue::array(
                unwrap_shared(left)
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| {
                        function(
                            value,
                            right
                                .get(index)
                                .cloned()
                                .unwrap_or_else(|| fallback.clone()),
                        )
                    })
                    .collect(),
            )
        }
        (DataviewValue::Array(values), right) => DataviewValue::array(
            unwrap_shared(values)
                .into_iter()
                .map(|value| function(value, right.clone()))
                .collect(),
        ),
        (left, DataviewValue::Array(values)) => DataviewValue::array(
            unwrap_shared(values)
                .into_iter()
                .map(|value| function(left.clone(), value))
                .collect(),
        ),
        (left, right) => function(left, right),
    }
}
