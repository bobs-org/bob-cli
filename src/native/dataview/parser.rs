//! Native query and source-expression parser.

use super::*;
use serde_json::Number;

impl NativeQuery {
    pub(super) fn parse(query: &str) -> Result<Self, DataviewError> {
        let tokens = NativeLexer::new(query)
            .tokenize()
            .map_err(native_query_error)?;
        NativeParser::new(tokens)
            .parse_query()
            .map_err(native_query_error)
    }
}

impl NativeSourceExpr {
    pub(super) fn parse(source: &str) -> Result<Self, DataviewError> {
        if source.trim().is_empty() {
            return Ok(Self::All);
        }
        let tokens = NativeLexer::new(source)
            .tokenize()
            .map_err(native_query_error)?;
        NativeParser::new(tokens)
            .parse_source_query()
            .map_err(native_query_error)
    }
}

pub(super) struct NativeParser {
    tokens: Vec<NativeToken>,
    position: usize,
}

impl NativeParser {
    pub(super) fn new(tokens: Vec<NativeToken>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    pub(super) fn parse_query(&mut self) -> Result<NativeQuery, String> {
        let kind = self.parse_query_kind()?;
        let mut commands = Vec::new();
        while !self.at_eof() {
            commands.push(self.parse_data_command()?);
        }
        self.expect_eof()?;
        Ok(NativeQuery { kind, commands })
    }

    pub(super) fn parse_source_query(
        &mut self,
    ) -> Result<NativeSourceExpr, String> {
        let source = self.parse_source_expr()?;
        self.expect_eof()?;
        Ok(source)
    }

    pub(super) fn parse_data_command(
        &mut self,
    ) -> Result<NativeDataCommand, String> {
        match self.peek() {
            NativeToken::From => {
                self.position += 1;
                Ok(NativeDataCommand::From(self.parse_source_expr()?))
            }
            NativeToken::Where => {
                self.position += 1;
                let tokens = self.collect_expression(ExpressionStop::Data)?;
                Ok(NativeDataCommand::Where(NativeExpression::where_clause(
                    tokens,
                )?))
            }
            NativeToken::Sort => {
                self.position += 1;
                let mut tokens =
                    self.collect_expression(ExpressionStop::Data)?;
                let direction = match tokens.last() {
                    Some(NativeToken::Asc) => {
                        tokens.pop();
                        Some(SortDirection::Ascending)
                    }
                    Some(NativeToken::Desc) => {
                        tokens.pop();
                        Some(SortDirection::Descending)
                    }
                    _ => None,
                };
                Ok(NativeDataCommand::Sort {
                    expression: NativeExpression::new(tokens)?,
                    direction,
                })
            }
            NativeToken::Group => {
                self.position += 1;
                self.expect_by()?;
                let expression = self.parse_aliased_command_expression()?;
                Ok(NativeDataCommand::GroupBy {
                    expression: expression.0,
                    alias: expression.1,
                })
            }
            NativeToken::Flatten => {
                self.position += 1;
                let expression = self.parse_aliased_command_expression()?;
                Ok(NativeDataCommand::Flatten {
                    expression: expression.0,
                    alias: expression.1,
                })
            }
            NativeToken::Limit => {
                self.position += 1;
                Ok(NativeDataCommand::Limit(self.expect_limit()?))
            }
            token => Err(format!(
                "expected DQL data command, found {}; native parser supports \
                 FROM, WHERE, SORT, GROUP BY, FLATTEN, and LIMIT",
                native_token_name(token)
            )),
        }
    }

    pub(super) fn parse_aliased_command_expression(
        &mut self,
    ) -> Result<(NativeExpression, Option<String>), String> {
        let tokens = self.collect_expression(ExpressionStop::DataOrAs)?;
        let expression = NativeExpression::new(tokens)?;
        let alias = if self.take_as() {
            Some(self.expect_alias()?)
        } else {
            None
        };
        Ok((expression, alias))
    }

    pub(super) fn parse_source_expr(
        &mut self,
    ) -> Result<NativeSourceExpr, String> {
        self.parse_source_or()
    }

    pub(super) fn parse_source_or(
        &mut self,
    ) -> Result<NativeSourceExpr, String> {
        let mut source = self.parse_source_and()?;
        while self.take_or() {
            let right = self.parse_source_and()?;
            source = NativeSourceExpr::Or(Box::new(source), Box::new(right));
        }
        Ok(source)
    }

    pub(super) fn parse_source_and(
        &mut self,
    ) -> Result<NativeSourceExpr, String> {
        let mut source = self.parse_source_unary()?;
        while self.take_and() {
            let right = self.parse_source_unary()?;
            source = NativeSourceExpr::And(Box::new(source), Box::new(right));
        }
        Ok(source)
    }

    pub(super) fn parse_source_unary(
        &mut self,
    ) -> Result<NativeSourceExpr, String> {
        if self.take_minus() {
            return Ok(NativeSourceExpr::Not(Box::new(
                self.parse_source_unary()?,
            )));
        }
        self.parse_source_primary()
    }

    pub(super) fn parse_source_primary(
        &mut self,
    ) -> Result<NativeSourceExpr, String> {
        match self.peek() {
            NativeToken::Tag(tag) => {
                let tag = tag.clone();
                self.position += 1;
                Ok(NativeSourceExpr::Tag(tag))
            }
            NativeToken::String(path) => {
                let path = path.clone();
                self.position += 1;
                Ok(NativeSourceExpr::Path(path))
            }
            NativeToken::Link(link) => {
                let link = link.clone();
                self.position += 1;
                Ok(NativeSourceExpr::IncomingLink(link))
            }
            NativeToken::Identifier(identifier)
                if identifier.eq_ignore_ascii_case("outgoing") =>
            {
                self.position += 1;
                self.expect_lparen()?;
                let link = self.expect_link()?;
                self.expect_rparen()?;
                Ok(NativeSourceExpr::OutgoingLink(link))
            }
            NativeToken::LParen => {
                self.position += 1;
                let source = self.parse_source_expr()?;
                self.expect_rparen()?;
                Ok(source)
            }
            token => Err(format!(
                "expected Dataview source expression, found {}; native source \
                 expressions support tags, quoted folders/files, wikilinks, \
                 outgoing([[note]]), AND, OR, unary -, and parentheses",
                native_token_name(token)
            )),
        }
    }

    pub(super) fn parse_expr(&mut self) -> Result<NativeExpr, String> {
        self.parse_lambda()
    }

    pub(super) fn parse_lambda(&mut self) -> Result<NativeExpr, String> {
        let start = self.position;
        if let NativeToken::Identifier(parameter) = self.peek().clone() {
            self.position += 1;
            if self.take_arrow() {
                let body = self.parse_lambda()?;
                return Ok(NativeExpr::Lambda {
                    parameter,
                    body: Box::new(body),
                });
            }
        }
        self.position = start;

        if matches!(self.peek(), NativeToken::LParen) {
            self.position += 1;
            if let NativeToken::Identifier(parameter) = self.peek().clone() {
                self.position += 1;
                if matches!(self.peek(), NativeToken::RParen) {
                    self.position += 1;
                    if self.take_arrow() {
                        let body = self.parse_lambda()?;
                        return Ok(NativeExpr::Lambda {
                            parameter,
                            body: Box::new(body),
                        });
                    }
                }
            }
        }
        self.position = start;

        self.parse_or()
    }

    pub(super) fn parse_or(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_and()?;
        while self.take_or() {
            let right = self.parse_and()?;
            expr = NativeExpr::Binary {
                op: NativeBinaryOp::Or,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    pub(super) fn parse_and(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_comparison()?;
        while self.take_and() {
            let right = self.parse_comparison()?;
            expr = NativeExpr::Binary {
                op: NativeBinaryOp::And,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    pub(super) fn parse_comparison(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_term()?;
        while let Some(op) = self.take_comparison_op() {
            let right = self.parse_term()?;
            expr = NativeExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    pub(super) fn parse_term(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_factor()?;
        while let Some(op) = self.take_additive_op() {
            let right = self.parse_factor()?;
            expr = NativeExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    pub(super) fn parse_factor(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_unary()?;
        while let Some(op) = self.take_multiplicative_op() {
            let right = self.parse_unary()?;
            expr = NativeExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    pub(super) fn parse_unary(&mut self) -> Result<NativeExpr, String> {
        if self.take_not() {
            return Ok(NativeExpr::Unary {
                op: NativeUnaryOp::Not,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.take_minus() {
            return Ok(NativeExpr::Unary {
                op: NativeUnaryOp::Negate,
                expr: Box::new(self.parse_unary()?),
            });
        }
        self.parse_postfix()
    }

    pub(super) fn parse_postfix(&mut self) -> Result<NativeExpr, String> {
        let mut expr = self.parse_primary()?;
        while self.take_dot() {
            let field = self.expect_identifier()?;
            expr = NativeExpr::GetAttr {
                target: Box::new(expr),
                field,
            };
        }
        Ok(expr)
    }

    pub(super) fn parse_primary(&mut self) -> Result<NativeExpr, String> {
        match self.peek() {
            NativeToken::Bool(value) => {
                let value = *value;
                self.position += 1;
                Ok(NativeExpr::Literal(DataviewValue::Bool(value)))
            }
            NativeToken::Null => {
                self.position += 1;
                Ok(NativeExpr::Literal(DataviewValue::Null))
            }
            NativeToken::Number(value) => {
                let value = parse_expression_number(value)?;
                self.position += 1;
                Ok(NativeExpr::Literal(DataviewValue::Number(value)))
            }
            NativeToken::String(value) => {
                let value = value.clone();
                self.position += 1;
                Ok(NativeExpr::Literal(DataviewValue::String(value)))
            }
            NativeToken::Link(value) => {
                let value = value.clone();
                self.position += 1;
                Ok(NativeExpr::LinkLiteral(value))
            }
            NativeToken::Identifier(identifier) => {
                let identifier = identifier.clone();
                self.position += 1;
                if self.take_lparen() {
                    Ok(NativeExpr::Call {
                        function: identifier,
                        args: self.parse_call_args()?,
                    })
                } else {
                    Ok(NativeExpr::Identifier(identifier))
                }
            }
            NativeToken::List => {
                self.position += 1;
                if self.take_lparen() {
                    Ok(NativeExpr::Call {
                        function: "list".to_string(),
                        args: self.parse_call_args()?,
                    })
                } else {
                    Err("LIST is only valid as a query type or list(...) constructor".to_string())
                }
            }
            NativeToken::Sort => {
                self.position += 1;
                if self.take_lparen() {
                    Ok(NativeExpr::Call {
                        function: "sort".to_string(),
                        args: self.parse_call_args()?,
                    })
                } else {
                    Err("SORT is only valid as a data command or sort(...) function".to_string())
                }
            }
            NativeToken::LParen => {
                self.position += 1;
                let expr = self.parse_expr()?;
                self.expect_rparen()?;
                Ok(expr)
            }
            NativeToken::LBracket => self.parse_array(),
            NativeToken::LBrace => self.parse_object(),
            token => Err(format!(
                "expected expression, found {}; native expression parser \
                 supports literals, field access, calls, arrays, objects, \
                 lambdas, operators, and parentheses",
                native_token_name(token)
            )),
        }
    }

    pub(super) fn parse_call_args(
        &mut self,
    ) -> Result<Vec<NativeExpr>, String> {
        let mut args = Vec::new();
        if self.take_rparen() {
            return Ok(args);
        }
        loop {
            args.push(self.parse_expr()?);
            if self.take_comma() {
                continue;
            }
            self.expect_rparen()?;
            return Ok(args);
        }
    }

    pub(super) fn parse_array(&mut self) -> Result<NativeExpr, String> {
        self.position += 1;
        let mut values = Vec::new();
        if self.take_rbracket() {
            return Ok(NativeExpr::Array(values));
        }
        loop {
            values.push(self.parse_expr()?);
            if self.take_comma() {
                continue;
            }
            self.expect_rbracket()?;
            return Ok(NativeExpr::Array(values));
        }
    }

    pub(super) fn parse_object(&mut self) -> Result<NativeExpr, String> {
        self.position += 1;
        let mut values = Vec::new();
        if self.take_rbrace() {
            return Ok(NativeExpr::Object(values));
        }
        loop {
            let key = self.expect_object_key()?;
            self.expect_colon()?;
            let value = self.parse_expr()?;
            values.push((key, value));
            if self.take_comma() {
                continue;
            }
            self.expect_rbrace()?;
            return Ok(NativeExpr::Object(values));
        }
    }

    pub(super) fn parse_query_kind(
        &mut self,
    ) -> Result<NativeQueryKind, String> {
        match self.peek() {
            NativeToken::List => {
                self.position += 1;
                let without_id = self.take_without_id()?;
                let expression = if self.at_data_command() || self.at_eof() {
                    None
                } else {
                    Some(NativeExpression::new(
                        self.collect_expression(ExpressionStop::Data)?,
                    )?)
                };
                Ok(NativeQueryKind::List {
                    expression,
                    without_id,
                })
            }
            NativeToken::Table => {
                self.position += 1;
                let without_id = self.take_without_id()?;
                let mut columns = vec![self.parse_table_select()?];
                while self.take_comma() {
                    columns.push(self.parse_table_select()?);
                }
                Ok(NativeQueryKind::Table {
                    columns,
                    without_id,
                })
            }
            NativeToken::Task => {
                self.position += 1;
                let without_id = self.take_without_id()?;
                Ok(NativeQueryKind::Task {
                    _without_id: without_id,
                })
            }
            NativeToken::Calendar => {
                self.position += 1;
                let without_id = self.take_without_id()?;
                let expression = NativeExpression::new(
                    self.collect_expression(ExpressionStop::Data)?,
                )?;
                Ok(NativeQueryKind::Calendar {
                    expression,
                    _without_id: without_id,
                })
            }
            token => Err(format!(
                "native parser supports LIST, TABLE, TASK, and CALENDAR \
                 queries; found {}",
                native_token_name(token)
            )),
        }
    }

    pub(super) fn parse_table_select(
        &mut self,
    ) -> Result<NativeSelect, String> {
        let expression = NativeExpression::new(
            self.collect_expression(ExpressionStop::TableSelect)?,
        )?;
        let alias = if self.take_as() {
            Some(self.expect_alias()?)
        } else {
            None
        };
        Ok(NativeSelect { expression, alias })
    }

    pub(super) fn collect_expression(
        &mut self,
        stop: ExpressionStop,
    ) -> Result<Vec<NativeToken>, String> {
        let mut tokens = Vec::new();
        let mut depth = 0usize;
        loop {
            let token = self.peek();
            if matches!(token, NativeToken::Eof) {
                break;
            }
            if depth == 0
                && stop.stops_at(token)
                && !self.current_token_starts_function_call()
            {
                break;
            }

            match token {
                NativeToken::LParen
                | NativeToken::LBracket
                | NativeToken::LBrace => {
                    depth += 1;
                }
                NativeToken::RParen
                | NativeToken::RBracket
                | NativeToken::RBrace => {
                    if depth == 0 {
                        return Err(format!(
                            "unexpected {} in DQL expression",
                            native_token_name(token)
                        ));
                    }
                    depth -= 1;
                }
                _ => {}
            }
            tokens.push(token.clone());
            self.position += 1;
        }

        if depth != 0 {
            return Err("unterminated grouping in DQL expression".to_string());
        }
        if tokens.is_empty() {
            return Err("expected DQL expression".to_string());
        }
        Ok(tokens)
    }

    pub(super) fn take_and(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::And))
    }

    pub(super) fn take_or(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Or))
    }

    pub(super) fn take_dot(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Dot))
    }

    pub(super) fn take_comma(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Comma))
    }

    pub(super) fn take_minus(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Minus))
    }

    pub(super) fn take_not(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Not))
    }

    pub(super) fn take_arrow(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::Arrow))
    }

    pub(super) fn take_lparen(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::LParen))
    }

    pub(super) fn take_rparen(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::RParen))
    }

    pub(super) fn take_rbracket(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::RBracket))
    }

    pub(super) fn take_rbrace(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::RBrace))
    }

    pub(super) fn take_comparison_op(&mut self) -> Option<NativeBinaryOp> {
        let op = match self.peek() {
            NativeToken::Equal => NativeBinaryOp::Equal,
            NativeToken::NotEqual => NativeBinaryOp::NotEqual,
            NativeToken::Less => NativeBinaryOp::Less,
            NativeToken::LessEqual => NativeBinaryOp::LessEqual,
            NativeToken::Greater => NativeBinaryOp::Greater,
            NativeToken::GreaterEqual => NativeBinaryOp::GreaterEqual,
            _ => return None,
        };
        self.position += 1;
        Some(op)
    }

    pub(super) fn take_additive_op(&mut self) -> Option<NativeBinaryOp> {
        let op = match self.peek() {
            NativeToken::Plus => NativeBinaryOp::Add,
            NativeToken::Minus => NativeBinaryOp::Subtract,
            _ => return None,
        };
        self.position += 1;
        Some(op)
    }

    pub(super) fn take_multiplicative_op(&mut self) -> Option<NativeBinaryOp> {
        let op = match self.peek() {
            NativeToken::Star => NativeBinaryOp::Multiply,
            NativeToken::Slash => NativeBinaryOp::Divide,
            _ => return None,
        };
        self.position += 1;
        Some(op)
    }

    pub(super) fn take_as(&mut self) -> bool {
        self.take(|token| matches!(token, NativeToken::As))
    }

    pub(super) fn take_without_id(&mut self) -> Result<bool, String> {
        if !self.take(|token| matches!(token, NativeToken::Without)) {
            return Ok(false);
        }
        if self.take(|token| matches!(token, NativeToken::Id)) {
            Ok(true)
        } else {
            Err(format!(
                "expected ID after WITHOUT, found {}",
                native_token_name(self.peek())
            ))
        }
    }

    pub(super) fn take(
        &mut self,
        predicate: impl FnOnce(&NativeToken) -> bool,
    ) -> bool {
        if predicate(self.peek()) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    pub(super) fn expect_alias(&mut self) -> Result<String, String> {
        match self.peek() {
            NativeToken::Identifier(alias) => {
                let alias = alias.clone();
                self.position += 1;
                Ok(alias)
            }
            NativeToken::String(alias) => {
                let alias = alias.clone();
                self.position += 1;
                Ok(alias)
            }
            token => Err(format!(
                "expected alias after AS, found {}",
                native_token_name(token)
            )),
        }
    }

    pub(super) fn expect_identifier(&mut self) -> Result<String, String> {
        let NativeToken::Identifier(identifier) = self.peek() else {
            return Err(format!(
                "expected field name, found {}",
                native_token_name(self.peek())
            ));
        };
        let identifier = identifier.clone();
        self.position += 1;
        Ok(identifier)
    }

    pub(super) fn expect_link(&mut self) -> Result<String, String> {
        let NativeToken::Link(link) = self.peek() else {
            return Err(format!(
                "expected wikilink, found {}",
                native_token_name(self.peek())
            ));
        };
        let link = link.clone();
        self.position += 1;
        Ok(link)
    }

    pub(super) fn expect_limit(&mut self) -> Result<usize, String> {
        let NativeToken::Number(limit) = self.peek() else {
            return Err(format!(
                "expected LIMIT count, found {}",
                native_token_name(self.peek())
            ));
        };
        let limit = limit.parse::<usize>().map_err(|_| {
            format!("LIMIT count must be a non-negative integer: {limit}")
        })?;
        self.position += 1;
        Ok(limit)
    }

    pub(super) fn expect_by(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::By) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected BY after GROUP, found {}",
            native_token_name(self.peek())
        ))
    }

    pub(super) fn expect_lparen(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::LParen) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected '(', found {}",
            native_token_name(self.peek())
        ))
    }

    pub(super) fn expect_rparen(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::RParen) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected ')', found {}",
            native_token_name(self.peek())
        ))
    }

    pub(super) fn expect_rbracket(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::RBracket) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected ']', found {}",
            native_token_name(self.peek())
        ))
    }

    pub(super) fn expect_rbrace(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::RBrace) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected '}}', found {}",
            native_token_name(self.peek())
        ))
    }

    pub(super) fn expect_colon(&mut self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::Colon) {
            self.position += 1;
            return Ok(());
        }
        Err(format!(
            "expected ':', found {}",
            native_token_name(self.peek())
        ))
    }

    pub(super) fn expect_object_key(&mut self) -> Result<String, String> {
        match self.peek() {
            NativeToken::Identifier(key) | NativeToken::String(key) => {
                let key = key.clone();
                self.position += 1;
                Ok(key)
            }
            token => Err(format!(
                "expected object key, found {}",
                native_token_name(token)
            )),
        }
    }

    pub(super) fn expect_eof(&self) -> Result<(), String> {
        if matches!(self.peek(), NativeToken::Eof) {
            return Ok(());
        }
        Err(format!(
            "unexpected {} after native query; native engine supports LIST \
             or TABLE <fields> FROM \"folder\" WHERE <expression>",
            native_token_name(self.peek())
        ))
    }

    pub(super) fn peek(&self) -> &NativeToken {
        self.tokens
            .get(self.position)
            .unwrap_or_else(|| self.tokens.last().expect("lexer adds EOF"))
    }

    pub(super) fn current_token_starts_function_call(&self) -> bool {
        matches!(self.peek(), NativeToken::Sort)
            && matches!(
                self.tokens.get(self.position + 1),
                Some(NativeToken::LParen)
            )
    }

    pub(super) fn at_eof(&self) -> bool {
        matches!(self.peek(), NativeToken::Eof)
    }

    pub(super) fn at_data_command(&self) -> bool {
        is_data_command(self.peek())
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ExpressionStop {
    Data,
    DataOrAs,
    TableSelect,
}

impl ExpressionStop {
    pub(super) fn stops_at(self, token: &NativeToken) -> bool {
        match self {
            Self::Data => is_data_command(token),
            Self::DataOrAs => {
                is_data_command(token) || matches!(token, NativeToken::As)
            }
            Self::TableSelect => {
                is_data_command(token)
                    || matches!(token, NativeToken::Comma | NativeToken::As)
            }
        }
    }
}

pub(super) fn native_query_error(message: String) -> DataviewError {
    DataviewError::NativeQuery { message }
}

pub(super) fn parse_native_expression(
    tokens: Vec<NativeToken>,
) -> Result<NativeExpr, String> {
    let mut tokens = tokens;
    tokens.push(NativeToken::Eof);
    let mut parser = NativeParser::new(tokens);
    let expr = parser.parse_expr()?;
    parser.expect_eof()?;
    Ok(expr)
}

pub(super) fn parse_expression_number(value: &str) -> Result<Number, String> {
    if let Ok(value) = value.parse::<i64>() {
        return Ok(Number::from(value));
    }
    if let Ok(value) = value.parse::<u64>() {
        return Ok(Number::from(value));
    }
    value
        .parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .ok_or_else(|| format!("invalid number literal: {value}"))
}

pub(super) fn field_chain_from_tokens(
    tokens: &[NativeToken],
) -> Option<Vec<String>> {
    let mut tokens = tokens.iter();
    let NativeToken::Identifier(first) = tokens.next()? else {
        return None;
    };
    let mut chain = vec![first.clone()];
    loop {
        match tokens.next() {
            None => return Some(chain),
            Some(NativeToken::Dot) => {
                let Some(NativeToken::Identifier(field)) = tokens.next() else {
                    return None;
                };
                chain.push(field.clone());
            }
            Some(_) => return None,
        }
    }
}

pub(super) fn expression_tokens_to_string(tokens: &[NativeToken]) -> String {
    if let Some(chain) = field_chain_from_tokens(tokens) {
        return chain.join(".");
    }

    let mut output = String::new();
    let mut previous_word = false;
    for token in tokens {
        let piece = token_expression_piece(token);
        let current_word = token_is_wordlike(token);
        if !output.is_empty()
            && should_space_expression_piece(
                &output,
                previous_word,
                current_word,
                token,
            )
        {
            output.push(' ');
        }
        output.push_str(&piece);
        previous_word = current_word;
    }
    output
}

pub(super) fn token_expression_piece(token: &NativeToken) -> String {
    match token {
        NativeToken::And => "AND".to_string(),
        NativeToken::As => "AS".to_string(),
        NativeToken::Asc => "ASC".to_string(),
        NativeToken::Bool(value) => value.to_string(),
        NativeToken::By => "BY".to_string(),
        NativeToken::Calendar => "CALENDAR".to_string(),
        NativeToken::Colon => ":".to_string(),
        NativeToken::Comma => ",".to_string(),
        NativeToken::Desc => "DESC".to_string(),
        NativeToken::Dot => ".".to_string(),
        NativeToken::Equal => "=".to_string(),
        NativeToken::Arrow => "=>".to_string(),
        NativeToken::Eof => String::new(),
        NativeToken::Flatten => "FLATTEN".to_string(),
        NativeToken::From => "FROM".to_string(),
        NativeToken::Greater => ">".to_string(),
        NativeToken::GreaterEqual => ">=".to_string(),
        NativeToken::Group => "GROUP".to_string(),
        NativeToken::Identifier(value) => value.clone(),
        NativeToken::LBrace => "{".to_string(),
        NativeToken::LBracket => "[".to_string(),
        NativeToken::Less => "<".to_string(),
        NativeToken::LessEqual => "<=".to_string(),
        NativeToken::Link(value) => format!("[[{value}]]"),
        NativeToken::List => "LIST".to_string(),
        NativeToken::LParen => "(".to_string(),
        NativeToken::Minus => "-".to_string(),
        NativeToken::Not => "!".to_string(),
        NativeToken::NotEqual => "!=".to_string(),
        NativeToken::Null => "null".to_string(),
        NativeToken::Number(value) => value.clone(),
        NativeToken::Or => "OR".to_string(),
        NativeToken::Plus => "+".to_string(),
        NativeToken::RBrace => "}".to_string(),
        NativeToken::RBracket => "]".to_string(),
        NativeToken::RParen => ")".to_string(),
        NativeToken::Slash => "/".to_string(),
        NativeToken::String(value) => format!("{value:?}"),
        NativeToken::Sort => "SORT".to_string(),
        NativeToken::Star => "*".to_string(),
        NativeToken::Tag(value) => value.clone(),
        NativeToken::Table => "TABLE".to_string(),
        NativeToken::Task => "TASK".to_string(),
        NativeToken::Limit => "LIMIT".to_string(),
        NativeToken::Without => "WITHOUT".to_string(),
        NativeToken::Where => "WHERE".to_string(),
        NativeToken::Id => "ID".to_string(),
    }
}

pub(super) fn token_is_wordlike(token: &NativeToken) -> bool {
    matches!(
        token,
        NativeToken::And
            | NativeToken::As
            | NativeToken::Asc
            | NativeToken::Bool(_)
            | NativeToken::By
            | NativeToken::Calendar
            | NativeToken::Desc
            | NativeToken::Flatten
            | NativeToken::From
            | NativeToken::Group
            | NativeToken::Identifier(_)
            | NativeToken::Link(_)
            | NativeToken::List
            | NativeToken::Null
            | NativeToken::Number(_)
            | NativeToken::Or
            | NativeToken::Sort
            | NativeToken::String(_)
            | NativeToken::Tag(_)
            | NativeToken::Table
            | NativeToken::Task
            | NativeToken::Limit
            | NativeToken::Without
            | NativeToken::Where
            | NativeToken::Id
    )
}

pub(super) fn should_space_expression_piece(
    output: &str,
    previous_word: bool,
    current_word: bool,
    token: &NativeToken,
) -> bool {
    if matches!(
        token,
        NativeToken::Comma
            | NativeToken::Dot
            | NativeToken::RParen
            | NativeToken::RBracket
            | NativeToken::RBrace
    ) {
        return false;
    }
    if output.ends_with(['(', '[', '{', '.', '-', '!', '/']) {
        return false;
    }
    previous_word || current_word
}

pub(super) fn is_data_command(token: &NativeToken) -> bool {
    matches!(
        token,
        NativeToken::From
            | NativeToken::Where
            | NativeToken::Sort
            | NativeToken::Group
            | NativeToken::Flatten
            | NativeToken::Limit
    )
}
