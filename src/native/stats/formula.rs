//! Integer formula expressions over metric ids.
//!
//! Grammar: integer literals, metric ids, unary minus, parentheses,
//! and `+ - * /`. No function calls or arbitrary code.

use std::collections::BTreeSet;

/// A parsed formula. Evaluation happens in a later phase.
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

pub(crate) fn parse_formula(expr: &str) -> Result<FormulaExpr, String> {
    let mut parser = Parser { src: expr, pos: 0 };
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

    fn parse_unary(&mut self) -> Result<FormulaExpr, String> {
        self.skip_ws();
        match self.peek() {
            Some('-' | '\u{2212}') => {
                self.bump();
                Ok(FormulaExpr::UnaryMinus(Box::new(self.parse_unary()?)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<FormulaExpr, String> {
        self.skip_ws();
        match self.peek() {
            Some('(') => {
                self.bump();
                let inner = self.parse_expr()?;
                self.skip_ws();
                if self.peek() != Some(')') {
                    return Err("must close every '(' with ')'".to_string());
                }
                self.bump();
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
}
