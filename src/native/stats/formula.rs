//! Integer formula expressions over metric ids.
//!
//! Grammar: integer literals, metric ids, unary minus, parentheses,
//! and `+ - * /`. No function calls or arbitrary code.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;

use super::{
    metrics::{effective_since, explicit_or_global_since, MetricEvaluation},
    registry::MetricDef,
    report::StatsWarning,
    value::{DailyValue, DataQuality},
};

/// A parsed formula. Evaluation is integer arithmetic over already-evaluated
/// dependencies plus integer literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FormulaExpr {
    Literal(i64),
    Metric(String),
    UnaryMinus(Box<FormulaExpr>),
    Binary {
        op: BinOp,
        left: Box<FormulaExpr>,
        right: Box<FormulaExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl FormulaExpr {
    /// Metric ids in first-seen order.
    pub(crate) fn dependencies(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut ids = Vec::new();
        self.collect_ids(&mut seen, &mut ids);
        ids
    }

    fn collect_ids(&self, seen: &mut BTreeSet<String>, ids: &mut Vec<String>) {
        match self {
            Self::Literal(_) => {}
            Self::Metric(id) => {
                if seen.insert(id.clone()) {
                    ids.push(id.clone());
                }
            }
            Self::UnaryMinus(inner) => inner.collect_ids(seen, ids),
            Self::Binary { left, right, .. } => {
                left.collect_ids(seen, ids);
                right.collect_ids(seen, ids);
            }
        }
    }
}

const MAX_PARSE_DEPTH: u8 = 64;

pub(crate) fn parse_formula(expr: &str) -> Result<FormulaExpr, String> {
    let mut parser = Parser {
        src: expr,
        pos: 0,
        depth: 0,
    };
    parser.skip_ws();
    if parser.peek().is_none() {
        return Err("must be a non-empty formula".to_string());
    }
    let parsed = parser.parse_expr()?;
    parser.skip_ws();
    if parser.peek().is_some() {
        return Err(format!(
            "must not contain trailing input starting at {:?}",
            parser.rest()
        ));
    }
    Ok(parsed)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    depth: u8,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let mut chars = self.rest().chars();
        let next = chars.next()?;
        self.pos += next.len_utf8();
        Some(next)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn parse_expr(&mut self) -> Result<FormulaExpr, String> {
        let mut left = self.parse_term()?;
        loop {
            self.skip_ws();
            let op = match self.peek() {
                Some('+') => BinOp::Add,
                Some('-' | '\u{2212}') => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let right = self.parse_term()?;
            left = FormulaExpr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<FormulaExpr, String> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            let op = match self.peek() {
                Some('*') => BinOp::Mul,
                Some('/') => BinOp::Div,
                _ => break,
            };
            self.bump();
            let right = self.parse_unary()?;
            left = FormulaExpr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn enter_nest(&mut self) -> Result<(), String> {
        if self.depth >= MAX_PARSE_DEPTH {
            return Err(
                "must not nest formulas more than 64 levels deep".to_string()
            );
        }
        self.depth += 1;
        Ok(())
    }

    fn exit_nest(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn parse_unary(&mut self) -> Result<FormulaExpr, String> {
        self.skip_ws();
        match self.peek() {
            Some('-' | '\u{2212}') => {
                self.enter_nest()?;
                self.bump();
                let inner = self.parse_unary()?;
                self.exit_nest();
                Ok(FormulaExpr::UnaryMinus(Box::new(inner)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<FormulaExpr, String> {
        self.skip_ws();
        match self.peek() {
            Some('(') => {
                self.enter_nest()?;
                self.bump();
                let inner = self.parse_expr()?;
                self.skip_ws();
                if self.peek() != Some(')') {
                    self.exit_nest();
                    return Err("must close every '(' with ')'".to_string());
                }
                self.bump();
                self.exit_nest();
                Ok(inner)
            }
            Some(ch) if ch.is_ascii_digit() => self.parse_number(),
            Some(ch) if is_ident_start(ch) => self.parse_ident(),
            Some(ch) => Err(format!(
                "must be an integer, metric id, or '('; got {ch:?}"
            )),
            None => Err("must be an integer, metric id, or '('".to_string()),
        }
    }

    fn parse_number(&mut self) -> Result<FormulaExpr, String> {
        let start = self.pos;
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            self.bump();
        }
        let digits = &self.src[start..self.pos];
        let value = digits.parse::<i64>().map_err(|_| {
            format!("must be an integer that fits in i64; got {digits}")
        })?;
        Ok(FormulaExpr::Literal(value))
    }

    fn parse_ident(&mut self) -> Result<FormulaExpr, String> {
        let start = self.pos;
        if !self.peek().is_some_and(is_ident_start) {
            return Err("must start a metric id with a letter".to_string());
        }
        self.bump();
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        Ok(FormulaExpr::Metric(self.src[start..self.pos].to_string()))
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

/// A checked evaluation failure: division by zero or an overflowing
/// operation. Both report null with an actionable reason, never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormulaFailure {
    DivisionByZero,
    Overflow,
}

/// Evaluate a formula metric over the requested dates. Returns `None` for
/// any non-formula metric.
///
/// Dependencies resolve through `evaluations`, which must already hold every
/// validated dependency in the registry's evaluation order; this function
/// never evaluates another metric itself, so callers must not hand it a
/// partially populated map. A dependency without an evaluation contributes
/// null inputs plus one `formula_limitation` warning.
///
/// Semantics: a null input propagates null; quality is always the worst
/// input quality; division rounds half away from zero and `/0` is null
/// whose quality is the worse of the input quality and `missing`.
/// Inferred since is the latest dependency since (every input must be
/// tracked); a literal-only formula uses explicit/global since, else the
/// requested range start passed in by compute — never `dates.first()`,
/// which is the prior-window start when compare is on.
pub(crate) fn evaluate_formula(
    metric: &MetricDef,
    evaluations: &BTreeMap<String, MetricEvaluation>,
    dates: &[NaiveDate],
    today: NaiveDate,
    global_since: Option<NaiveDate>,
    range_start: Option<NaiveDate>,
) -> Option<MetricEvaluation> {
    let MetricSpecRef::Formula { ast, dependencies } = metric_spec(metric)
    else {
        return None;
    };
    let inferred_since = dependencies
        .iter()
        .filter_map(|id| evaluations.get(id))
        .filter_map(|evaluation| evaluation.since)
        .max();
    let since = if dependencies.is_empty() {
        explicit_or_global_since(metric, global_since).or(range_start)
    } else {
        effective_since(metric, inferred_since, global_since)
    };

    let mut warnings = Vec::new();
    let missing: Vec<&str> = dependencies
        .iter()
        .filter(|id| !evaluations.contains_key(id.as_str()))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        warnings.push(StatsWarning {
            code: "formula_limitation".to_string(),
            metric: Some(metric.id.clone()),
            date: None,
            message: format!(
                "formula references unevaluated metric(s) {}; their inputs \
                 are null",
                missing.join(", "),
            ),
        });
    }

    let by_day: BTreeMap<&str, BTreeMap<NaiveDate, &DailyValue>> = dependencies
        .iter()
        .map(|id| {
            (
                id.as_str(),
                evaluations
                    .get(id)
                    .map(|evaluation| {
                        evaluation
                            .values
                            .iter()
                            .map(|value| (value.date, value))
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        })
        .collect();

    let values = dates
        .iter()
        .map(|date| {
            let provisional = *date == today;
            if since.is_some_and(|start| *date < start) {
                return null_before_tracking(*date, provisional);
            }
            let inputs = dependencies
                .iter()
                .map(|id| {
                    (
                        id.as_str(),
                        by_day
                            .get(id.as_str())
                            .and_then(|days| days.get(date).copied()),
                    )
                })
                .collect::<Vec<_>>();
            let quality =
                inputs.iter().fold(DataQuality::Ok, |worst, (_, value)| {
                    worse_quality(
                        worst,
                        value.map_or(DataQuality::Missing, |value| {
                            value.quality
                        }),
                    )
                });
            let lookup = |id: &str| {
                inputs
                    .iter()
                    .find(|(key, _)| *key == id)
                    .and_then(|(_, value)| value.and_then(|value| value.value))
            };
            let nulls: Vec<&str> = dependencies
                .iter()
                .filter(|id| lookup(id).is_none())
                .map(String::as_str)
                .collect();
            match eval_checked(ast, &lookup) {
                Ok(Some(value)) if nulls.is_empty() => {
                    some_value(*date, value, quality, provisional)
                }
                Ok(_) => null_value(
                    *date,
                    quality,
                    format!("null input from {}", nulls.join(", ")),
                    provisional,
                ),
                Err(FormulaFailure::DivisionByZero) => null_value(
                    *date,
                    worse_quality(quality, DataQuality::Missing),
                    "division by zero".to_string(),
                    provisional,
                ),
                Err(FormulaFailure::Overflow) => null_value(
                    *date,
                    worse_quality(quality, DataQuality::Missing),
                    "arithmetic overflow".to_string(),
                    provisional,
                ),
            }
        })
        .collect();

    Some(MetricEvaluation {
        since,
        values,
        warnings,
    })
}

enum MetricSpecRef<'a> {
    Formula {
        ast: &'a FormulaExpr,
        dependencies: &'a [String],
    },
    Other,
}

fn metric_spec(metric: &MetricDef) -> MetricSpecRef<'_> {
    use super::registry::MetricSpec;
    match &metric.spec {
        MetricSpec::Formula {
            ast, dependencies, ..
        } => MetricSpecRef::Formula { ast, dependencies },
        _ => MetricSpecRef::Other,
    }
}

fn worse_quality(left: DataQuality, right: DataQuality) -> DataQuality {
    use DataQuality;
    fn rank(quality: DataQuality) -> u8 {
        match quality {
            DataQuality::Ok => 0,
            DataQuality::Partial => 1,
            DataQuality::Missing => 2,
            DataQuality::NotTracked => 3,
        }
    }
    if rank(left) >= rank(right) {
        left
    } else {
        right
    }
}

fn eval_checked(
    expr: &FormulaExpr,
    lookup: &dyn Fn(&str) -> Option<i64>,
) -> Result<Option<i64>, FormulaFailure> {
    match expr {
        FormulaExpr::Literal(value) => Ok(Some(*value)),
        FormulaExpr::Metric(id) => Ok(lookup(id)),
        FormulaExpr::UnaryMinus(inner) => match eval_checked(inner, lookup)? {
            None => Ok(None),
            Some(value) => value
                .checked_neg()
                .map(Some)
                .ok_or(FormulaFailure::Overflow),
        },
        FormulaExpr::Binary { op, left, right } => {
            let (Some(left), Some(right)) =
                (eval_checked(left, lookup)?, eval_checked(right, lookup)?)
            else {
                return Ok(None);
            };
            match op {
                BinOp::Add => left
                    .checked_add(right)
                    .map(Some)
                    .ok_or(FormulaFailure::Overflow),
                BinOp::Sub => left
                    .checked_sub(right)
                    .map(Some)
                    .ok_or(FormulaFailure::Overflow),
                BinOp::Mul => left
                    .checked_mul(right)
                    .map(Some)
                    .ok_or(FormulaFailure::Overflow),
                BinOp::Div => checked_div_half_away(left, right),
            }
        }
    }
}

/// Integer division rounding half away from zero: 7/2 is 4 and -7/2 is -4.
fn checked_div_half_away(
    numerator: i64,
    denominator: i64,
) -> Result<Option<i64>, FormulaFailure> {
    if denominator == 0 {
        return Err(FormulaFailure::DivisionByZero);
    }
    let quotient = numerator
        .checked_div(denominator)
        .ok_or(FormulaFailure::Overflow)?;
    let remainder = numerator
        .checked_rem(denominator)
        .ok_or(FormulaFailure::Overflow)?;
    if remainder == 0 {
        return Ok(Some(quotient));
    }
    let twice_remainder = i128::from(remainder).abs() * 2;
    if twice_remainder < i128::from(denominator).abs() {
        return Ok(Some(quotient));
    }
    let step = i64::from((numerator > 0) == (denominator > 0));
    quotient
        .checked_add(if step == 1 { 1 } else { -1 })
        .map(Some)
        .ok_or(FormulaFailure::Overflow)
}

fn null_before_tracking(date: NaiveDate, provisional: bool) -> DailyValue {
    use DataQuality;
    DailyValue {
        date,
        value: None,
        num: None,
        den: None,
        quality: DataQuality::NotTracked,
        reason: Some("before tracking began".to_string()),
        provisional,
        parts: std::collections::BTreeMap::new(),
        apportioned: std::collections::BTreeMap::new(),
        checklist_state: None,
    }
}

fn null_value(
    date: NaiveDate,
    quality: DataQuality,
    reason: String,
    provisional: bool,
) -> DailyValue {
    DailyValue {
        date,
        value: None,
        num: None,
        den: None,
        quality,
        reason: Some(reason),
        provisional,
        parts: std::collections::BTreeMap::new(),
        apportioned: std::collections::BTreeMap::new(),
        checklist_state: None,
    }
}

fn some_value(
    date: NaiveDate,
    value: i64,
    quality: DataQuality,
    provisional: bool,
) -> DailyValue {
    DailyValue {
        date,
        value: Some(value),
        num: None,
        den: None,
        quality,
        reason: None,
        provisional,
        parts: std::collections::BTreeMap::new(),
        apportioned: std::collections::BTreeMap::new(),
        checklist_state: None,
    }
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_net_builtin_expression() {
        let expr = parse_formula("created - done - dropped").unwrap();
        assert_eq!(expr.dependencies(), ["created", "done", "dropped"]);
        assert_eq!(
            expr,
            FormulaExpr::Binary {
                op: BinOp::Sub,
                left: Box::new(FormulaExpr::Binary {
                    op: BinOp::Sub,
                    left: Box::new(FormulaExpr::Metric("created".into())),
                    right: Box::new(FormulaExpr::Metric("done".into())),
                }),
                right: Box::new(FormulaExpr::Metric("dropped".into())),
            }
        );
    }

    #[test]
    fn parses_literals_unary_parens_and_products() {
        let expr = parse_formula("-(2 + foo) * 3 / bar").unwrap();
        assert_eq!(expr.dependencies(), ["foo", "bar"]);
        match expr {
            FormulaExpr::Binary { op: BinOp::Div, .. } => {}
            other => panic!("expected division at root, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_trailing_and_non_code() {
        parse_formula("").expect_err("empty");
        parse_formula("foo +").expect_err("trailing operator");
        parse_formula("foo(1)").expect_err("function call");
        parse_formula("1.5").expect_err("float");
        parse_formula("foo; bar").expect_err("statement");
    }

    #[test]
    fn dependencies_dedupe_first_seen() {
        let expr = parse_formula("a + b - a * b").unwrap();
        assert_eq!(expr.dependencies(), ["a", "b"]);
    }

    use std::path::Path;

    use chrono::NaiveDate;

    use crate::native::{
        config::parse_stats_config, stats::registry::build_registry,
    };

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn custom_formula(expr: &str, id: &str) -> MetricDef {
        let config = parse_stats_config(
            &format!(
                "stats:\n  metrics:\n    {id}:\n      label: F\n      kind: formula\n      expr: {expr}\n"
            ),
            Path::new("/config.yml"),
        )
        .unwrap();
        build_registry(&config).get(id).unwrap().clone()
    }

    /// One dependency evaluation holding explicit daily values.
    fn dep_eval(
        since: Option<NaiveDate>,
        points: &[(u32, Option<i64>, DataQuality)],
    ) -> MetricEvaluation {
        MetricEvaluation {
            since,
            values: points
                .iter()
                .map(|(day, value, quality)| DailyValue {
                    date: date(*day),
                    value: *value,
                    num: None,
                    den: None,
                    quality: *quality,
                    reason: None,
                    provisional: false,
                    parts: BTreeMap::new(),
                    apportioned: BTreeMap::new(),
                    checklist_state: None,
                })
                .collect(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn net_evaluates_with_worst_input_quality() {
        let config =
            parse_stats_config("stats:\n", Path::new("/config.yml")).unwrap();
        let net = build_registry(&config).get("net").unwrap().clone();
        let evaluations = BTreeMap::from([
            (
                "created".to_string(),
                dep_eval(Some(date(10)), &[(10, Some(5), DataQuality::Ok)]),
            ),
            (
                "done".to_string(),
                dep_eval(
                    Some(date(10)),
                    &[(10, Some(3), DataQuality::Partial)],
                ),
            ),
            (
                "dropped".to_string(),
                dep_eval(Some(date(10)), &[(10, Some(1), DataQuality::Ok)]),
            ),
        ]);
        let result = evaluate_formula(
            &net,
            &evaluations,
            &[date(9), date(10)],
            date(11),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.since, Some(date(10)));
        assert_eq!(result.values[0].value, None);
        assert_eq!(result.values[0].quality, DataQuality::NotTracked);
        assert_eq!(result.values[1].value, Some(1));
        // A Partial input marks even a successful computation Partial.
        assert_eq!(result.values[1].quality, DataQuality::Partial);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn null_inputs_propagate_null_with_worst_quality_and_names() {
        let metric = custom_formula("created - done - dropped", "flow");
        let evaluations = BTreeMap::from([
            (
                "created".to_string(),
                dep_eval(Some(date(9)), &[(10, Some(5), DataQuality::Ok)]),
            ),
            (
                "done".to_string(),
                dep_eval(Some(date(9)), &[(10, None, DataQuality::Missing)]),
            ),
            (
                "dropped".to_string(),
                dep_eval(Some(date(9)), &[(10, None, DataQuality::NotTracked)]),
            ),
        ]);
        let result = evaluate_formula(
            &metric,
            &evaluations,
            &[date(10)],
            date(11),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.values[0].value, None);
        assert_eq!(result.values[0].quality, DataQuality::NotTracked);
        assert_eq!(
            result.values[0].reason.as_deref(),
            Some("null input from done, dropped")
        );
    }

    #[test]
    fn division_by_zero_and_overflow_are_null_not_panics() {
        let div = custom_formula("created / done", "ratio");
        let evaluations = BTreeMap::from([
            (
                "created".to_string(),
                dep_eval(Some(date(9)), &[(10, Some(7), DataQuality::Ok)]),
            ),
            (
                "done".to_string(),
                dep_eval(Some(date(9)), &[(10, Some(0), DataQuality::Ok)]),
            ),
        ]);
        let result = evaluate_formula(
            &div,
            &evaluations,
            &[date(10)],
            date(11),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.values[0].value, None);
        assert_eq!(result.values[0].quality, DataQuality::Missing);
        assert_eq!(
            result.values[0].reason.as_deref(),
            Some("division by zero")
        );

        for (expr, reason) in [
            ("9223372036854775807 + 1", "arithmetic overflow"),
            ("created * done", "arithmetic overflow"),
            ("-created", "arithmetic overflow"),
            ("created / done", "arithmetic overflow"),
        ] {
            let metric = custom_formula(expr, "big");
            let evaluations = BTreeMap::from([
                (
                    "created".to_string(),
                    dep_eval(
                        Some(date(9)),
                        &[(10, Some(i64::MIN), DataQuality::Ok)],
                    ),
                ),
                (
                    "done".to_string(),
                    dep_eval(Some(date(9)), &[(10, Some(-1), DataQuality::Ok)]),
                ),
            ]);
            // Tune the overflow trigger per expression.
            let evaluations = if expr == "created * done" {
                BTreeMap::from([
                    (
                        "created".to_string(),
                        dep_eval(
                            Some(date(9)),
                            &[(10, Some(i64::MAX), DataQuality::Ok)],
                        ),
                    ),
                    (
                        "done".to_string(),
                        dep_eval(
                            Some(date(9)),
                            &[(10, Some(2), DataQuality::Ok)],
                        ),
                    ),
                ])
            } else {
                evaluations
            };
            let result = evaluate_formula(
                &metric,
                &evaluations,
                &[date(10)],
                date(11),
                None,
                None,
            )
            .unwrap();
            assert_eq!(result.values[0].value, None, "{expr}");
            assert_eq!(
                result.values[0].reason.as_deref(),
                Some(reason),
                "{expr}"
            );
        }
    }

    #[test]
    fn division_rounds_half_away_from_zero() {
        let metric = custom_formula("created / done", "half");
        for (numerator, denominator, expected) in [
            (7, 2, 4),
            (-7, 2, -4),
            (7, -2, -4),
            (-7, -2, 4),
            (5, 2, 3),
            (1, 2, 1),
            (3, 2, 2),
            (-3, 2, -2),
            (5, 3, 2),
            (4, 3, 1),
            (6, 3, 2),
        ] {
            let evaluations = BTreeMap::from([
                (
                    "created".to_string(),
                    dep_eval(
                        Some(date(9)),
                        &[(10, Some(numerator), DataQuality::Ok)],
                    ),
                ),
                (
                    "done".to_string(),
                    dep_eval(
                        Some(date(9)),
                        &[(10, Some(denominator), DataQuality::Ok)],
                    ),
                ),
            ]);
            let result = evaluate_formula(
                &metric,
                &evaluations,
                &[date(10)],
                date(11),
                None,
                None,
            )
            .unwrap();
            assert_eq!(
                result.values[0].value,
                Some(expected),
                "{numerator}/{denominator}"
            );
        }
    }

    #[test]
    fn inferred_since_is_the_latest_dependency_since() {
        let metric = custom_formula("created - done", "flow");
        let evaluations = BTreeMap::from([
            (
                "created".to_string(),
                dep_eval(Some(date(8)), &[(9, Some(2), DataQuality::Ok)]),
            ),
            (
                "done".to_string(),
                dep_eval(Some(date(10)), &[(10, Some(1), DataQuality::Ok)]),
            ),
        ]);
        let result = evaluate_formula(
            &metric,
            &evaluations,
            &[date(8), date(9), date(10)],
            date(11),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.since, Some(date(10)));
        assert_eq!(result.values[0].quality, DataQuality::NotTracked);
        assert_eq!(result.values[1].quality, DataQuality::NotTracked);

        // An explicit metric since overrides the inference.
        let mut explicit = metric.clone();
        explicit.since = Some(date(9));
        let result = evaluate_formula(
            &explicit,
            &evaluations,
            &[date(8), date(9), date(10)],
            date(11),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.since, Some(date(9)));

        // A literal-only formula with no configured since starts at the
        // requested range start.
        let literal = custom_formula("40 + 2", "answer");
        let result = evaluate_formula(
            &literal,
            &BTreeMap::new(),
            &[date(8), date(9), date(10)],
            date(11),
            None,
            Some(date(9)),
        )
        .unwrap();
        assert_eq!(result.since, Some(date(9)));
        assert_eq!(result.values[0].quality, DataQuality::NotTracked);
        assert_eq!(result.values[1].value, Some(42));
        assert_eq!(result.values[1].quality, DataQuality::Ok);
    }

    #[test]
    fn unevaluated_dependencies_warn_once_and_read_null() {
        // `ghost` is not a real metric: the registry would invalidate this
        // formula, so the stranded-dependency path is pinned with a valid
        // formula and a partially populated evaluation map instead.
        let metric = custom_formula("created + done", "haunted");
        let evaluations = BTreeMap::from([(
            "created".to_string(),
            dep_eval(Some(date(10)), &[(10, Some(2), DataQuality::Ok)]),
        )]);
        let result = evaluate_formula(
            &metric,
            &evaluations,
            &[date(10)],
            date(11),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.values[0].value, None);
        assert_eq!(
            result.values[0].reason.as_deref(),
            Some("null input from done")
        );
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].code, "formula_limitation");
        assert!(result.warnings[0].message.contains("done"));
    }

    #[test]
    fn non_formula_metrics_evaluate_elsewhere() {
        let config =
            parse_stats_config("stats:\n", Path::new("/config.yml")).unwrap();
        let registry = build_registry(&config);
        assert!(evaluate_formula(
            registry.get("worked").unwrap(),
            &BTreeMap::new(),
            &[date(10)],
            date(11),
            None,
            None,
        )
        .is_none());
    }

    #[test]
    fn parser_rejects_nesting_deeper_than_sixty_four() {
        let nested_ok = format!("{}1{}", "(".repeat(64), ")".repeat(64));
        parse_formula(&nested_ok).expect("64 parens is allowed");
        let nested_too_deep = format!("{}1{}", "(".repeat(65), ")".repeat(65));
        let error = parse_formula(&nested_too_deep).expect_err("65 parens");
        assert!(error.contains("64"), "{error}");

        let unary_ok = format!("{}1", "-".repeat(64));
        parse_formula(&unary_ok).expect("64 unary minuses is allowed");
        let unary_too_deep = format!("{}1", "-".repeat(65));
        parse_formula(&unary_too_deep).expect_err("65 unary minuses");
    }
}
