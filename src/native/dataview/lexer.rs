//! Native query lexer: query text into tokens.

use super::*;

pub(super) struct NativeLexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> NativeLexer<'a> {
    pub(super) fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().peekable(),
        }
    }

    pub(super) fn tokenize(mut self) -> Result<Vec<NativeToken>, String> {
        let mut tokens = Vec::new();
        while let Some(ch) = self.chars.next() {
            match ch {
                ch if ch.is_whitespace() => {}
                '(' => tokens.push(NativeToken::LParen),
                ')' => tokens.push(NativeToken::RParen),
                '[' if self.chars.peek() == Some(&'[') => {
                    self.chars.next();
                    tokens.push(NativeToken::Link(self.read_wikilink()?));
                }
                '[' => tokens.push(NativeToken::LBracket),
                ']' => tokens.push(NativeToken::RBracket),
                '{' => tokens.push(NativeToken::LBrace),
                '}' => tokens.push(NativeToken::RBrace),
                ',' => tokens.push(NativeToken::Comma),
                ':' => tokens.push(NativeToken::Colon),
                '.' => tokens.push(NativeToken::Dot),
                '+' => tokens.push(NativeToken::Plus),
                '-' => tokens.push(NativeToken::Minus),
                '*' => tokens.push(NativeToken::Star),
                '/' => tokens.push(NativeToken::Slash),
                '!' if self.chars.peek() == Some(&'=') => {
                    self.chars.next();
                    tokens.push(NativeToken::NotEqual);
                }
                '!' => tokens.push(NativeToken::Not),
                '=' if self.chars.peek() == Some(&'>') => {
                    self.chars.next();
                    tokens.push(NativeToken::Arrow);
                }
                '=' => tokens.push(NativeToken::Equal),
                '<' if self.chars.peek() == Some(&'=') => {
                    self.chars.next();
                    tokens.push(NativeToken::LessEqual);
                }
                '<' => tokens.push(NativeToken::Less),
                '>' if self.chars.peek() == Some(&'=') => {
                    self.chars.next();
                    tokens.push(NativeToken::GreaterEqual);
                }
                '>' => tokens.push(NativeToken::Greater),
                '#' => tokens.push(NativeToken::Tag(self.read_tag())),
                '"' => tokens
                    .push(NativeToken::String(self.read_quoted_string('"')?)),
                '\'' => tokens
                    .push(NativeToken::String(self.read_quoted_string('\'')?)),
                ch if ch.is_ascii_digit() => {
                    tokens.push(NativeToken::Number(self.read_number(ch)));
                }
                ch if is_native_identifier_start(ch) => {
                    let identifier = self.read_identifier(ch);
                    tokens.push(native_identifier_token(identifier));
                }
                other => {
                    return Err(format!(
                        "unsupported token {other:?}; native engine supports \
                         LIST, TABLE, FROM, WHERE, AND, OR, parentheses, \
                         comma-separated table fields, field names, strings, \
                         booleans, and wikilinks"
                    ));
                }
            }
        }
        tokens.push(NativeToken::Eof);
        Ok(tokens)
    }

    pub(super) fn read_quoted_string(
        &mut self,
        quote: char,
    ) -> Result<String, String> {
        let mut output = String::new();
        while let Some(ch) = self.chars.next() {
            if ch == quote {
                return Ok(output);
            }
            if ch == '\\' && quote == '"' {
                let Some(escaped) = self.chars.next() else {
                    return Err("unterminated escape in string literal".into());
                };
                output.push(match escaped {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            } else {
                output.push(ch);
            }
        }

        Err("unterminated string literal".into())
    }

    pub(super) fn read_wikilink(&mut self) -> Result<String, String> {
        let mut output = String::new();
        while let Some(ch) = self.chars.next() {
            if ch == ']' && self.chars.peek() == Some(&']') {
                self.chars.next();
                return Ok(output);
            }
            output.push(ch);
        }

        Err("unterminated wikilink literal".into())
    }

    pub(super) fn read_tag(&mut self) -> String {
        let mut output = String::from("#");
        while self
            .chars
            .peek()
            .is_some_and(|ch| is_native_tag_continue(*ch))
        {
            output
                .push(self.chars.next().expect("peek confirmed tag character"));
        }
        output
    }

    pub(super) fn read_number(&mut self, first: char) -> String {
        let mut output = String::from(first);
        while self
            .chars
            .peek()
            .is_some_and(|ch| ch.is_ascii_digit() || *ch == '.')
        {
            output.push(
                self.chars.next().expect("peek confirmed number character"),
            );
        }
        output
    }

    pub(super) fn read_identifier(&mut self, first: char) -> String {
        let mut output = String::from(first);
        while self
            .chars
            .peek()
            .is_some_and(|ch| is_native_identifier_continue(*ch))
        {
            output.push(
                self.chars
                    .next()
                    .expect("peek confirmed identifier character"),
            );
        }
        output
    }
}
pub(super) fn is_native_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

pub(super) fn is_native_identifier_continue(ch: char) -> bool {
    ch == '_' || ch == '-' || ch.is_ascii_alphanumeric()
}

pub(super) fn is_native_tag_continue(ch: char) -> bool {
    !ch.is_whitespace()
        && !matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | ',' | '"' | '\'')
}

pub(super) fn native_identifier_token(identifier: String) -> NativeToken {
    match identifier.to_ascii_lowercase().as_str() {
        "and" => NativeToken::And,
        "as" => NativeToken::As,
        "asc" | "ascending" => NativeToken::Asc,
        "by" => NativeToken::By,
        "calendar" => NativeToken::Calendar,
        "desc" | "descending" => NativeToken::Desc,
        "flatten" => NativeToken::Flatten,
        "false" => NativeToken::Bool(false),
        "from" => NativeToken::From,
        "group" => NativeToken::Group,
        "id" => NativeToken::Id,
        "limit" => NativeToken::Limit,
        "list" => NativeToken::List,
        "null" => NativeToken::Null,
        "or" => NativeToken::Or,
        "sort" => NativeToken::Sort,
        "table" => NativeToken::Table,
        "task" => NativeToken::Task,
        "true" => NativeToken::Bool(true),
        "where" => NativeToken::Where,
        "without" => NativeToken::Without,
        _ => NativeToken::Identifier(identifier),
    }
}

pub(super) fn native_token_name(token: &NativeToken) -> &'static str {
    match token {
        NativeToken::And => "AND",
        NativeToken::As => "AS",
        NativeToken::Asc => "ASC",
        NativeToken::Bool(_) => "boolean",
        NativeToken::By => "BY",
        NativeToken::Calendar => "CALENDAR",
        NativeToken::Colon => "':'",
        NativeToken::Comma => "','",
        NativeToken::Desc => "DESC",
        NativeToken::Dot => "'.'",
        NativeToken::Equal => "'='",
        NativeToken::Arrow => "'=>'",
        NativeToken::Eof => "end of query",
        NativeToken::Flatten => "FLATTEN",
        NativeToken::From => "FROM",
        NativeToken::Greater => "'>'",
        NativeToken::GreaterEqual => "'>='",
        NativeToken::Group => "GROUP",
        NativeToken::Identifier(_) => "field name",
        NativeToken::LBrace => "'{'",
        NativeToken::LBracket => "'['",
        NativeToken::Less => "'<'",
        NativeToken::LessEqual => "'<='",
        NativeToken::Link(_) => "wikilink",
        NativeToken::List => "LIST",
        NativeToken::LParen => "'('",
        NativeToken::Minus => "'-'",
        NativeToken::Not => "'!'",
        NativeToken::NotEqual => "'!='",
        NativeToken::Null => "null",
        NativeToken::Number(_) => "number",
        NativeToken::Or => "OR",
        NativeToken::Plus => "'+'",
        NativeToken::RBrace => "'}'",
        NativeToken::RBracket => "']'",
        NativeToken::RParen => "')'",
        NativeToken::Slash => "'/'",
        NativeToken::String(_) => "string",
        NativeToken::Sort => "SORT",
        NativeToken::Star => "'*'",
        NativeToken::Tag(_) => "tag",
        NativeToken::Table => "TABLE",
        NativeToken::Task => "TASK",
        NativeToken::Limit => "LIMIT",
        NativeToken::Without => "WITHOUT",
        NativeToken::Where => "WHERE",
        NativeToken::Id => "ID",
    }
}
