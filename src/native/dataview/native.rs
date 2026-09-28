//! Native query model: DQL types, vault handle, and markdown settings.

use super::*;
use serde_json::Value;
use std::{fs, path::Path};

#[derive(Debug)]
pub(super) struct NativeOutput {
    pub(super) warnings: Vec<String>,
    pub(super) result: Value,
}

#[derive(Debug, Clone)]
pub(super) struct NativeMarkdownSettings {
    pub(super) render_null_as: String,
    pub(super) table_id_column_name: String,
    pub(super) table_group_column_name: String,
}

#[derive(Debug)]
pub(super) struct NativeQuery {
    pub(super) kind: NativeQueryKind,
    pub(super) commands: Vec<NativeDataCommand>,
}

#[derive(Debug)]
pub(super) enum NativeQueryKind {
    List {
        expression: Option<NativeExpression>,
        without_id: bool,
    },
    Table {
        columns: Vec<NativeSelect>,
        without_id: bool,
    },
    Task {
        _without_id: bool,
    },
    Calendar {
        expression: NativeExpression,
        _without_id: bool,
    },
}

#[derive(Debug)]
pub(super) struct NativeSelect {
    pub(super) expression: NativeExpression,
    pub(super) alias: Option<String>,
}

#[derive(Debug)]
pub(super) struct NativeExpression {
    pub(super) raw: String,
    pub(super) expr: NativeExpr,
}

#[derive(Debug)]
pub(super) enum NativeDataCommand {
    From(NativeSourceExpr),
    Where(NativeExpression),
    Sort {
        expression: NativeExpression,
        direction: Option<SortDirection>,
    },
    GroupBy {
        expression: NativeExpression,
        alias: Option<String>,
    },
    Flatten {
        expression: NativeExpression,
        alias: Option<String>,
    },
    Limit(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug)]
pub(super) enum NativeSourceExpr {
    All,
    And(Box<NativeSourceExpr>, Box<NativeSourceExpr>),
    IncomingLink(String),
    Not(Box<NativeSourceExpr>),
    Or(Box<NativeSourceExpr>, Box<NativeSourceExpr>),
    OutgoingLink(String),
    Path(String),
    Tag(String),
}

#[derive(Debug)]
pub(super) enum NativeExpr {
    Array(Vec<NativeExpr>),
    Binary {
        op: NativeBinaryOp,
        left: Box<NativeExpr>,
        right: Box<NativeExpr>,
    },
    Call {
        function: String,
        args: Vec<NativeExpr>,
    },
    GetAttr {
        target: Box<NativeExpr>,
        field: String,
    },
    Identifier(String),
    Lambda {
        parameter: String,
        body: Box<NativeExpr>,
    },
    LinkLiteral(String),
    Literal(DataviewValue),
    Object(Vec<(String, NativeExpr)>),
    Unary {
        op: NativeUnaryOp,
        expr: Box<NativeExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeBinaryOp {
    Add,
    And,
    Divide,
    Equal,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Multiply,
    NotEqual,
    Or,
    Subtract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeUnaryOp {
    Not,
    Negate,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum NativeToken {
    And,
    As,
    Asc,
    Bool(bool),
    By,
    Calendar,
    Colon,
    Comma,
    Desc,
    Dot,
    Equal,
    Arrow,
    Eof,
    Flatten,
    From,
    Greater,
    GreaterEqual,
    Group,
    Identifier(String),
    LBrace,
    LBracket,
    Less,
    LessEqual,
    Link(String),
    List,
    LParen,
    Minus,
    Not,
    NotEqual,
    Null,
    Number(String),
    Or,
    Plus,
    RBrace,
    RBracket,
    RParen,
    Slash,
    String(String),
    Sort,
    Star,
    Tag(String),
    Table,
    Task,
    Limit,
    Without,
    Where,
    Id,
}

impl NativeSelect {
    pub(super) fn header(&self) -> String {
        self.alias
            .clone()
            .unwrap_or_else(|| self.expression.raw.clone())
    }
}

impl Default for NativeMarkdownSettings {
    fn default() -> Self {
        Self {
            render_null_as: "\\-".to_string(),
            table_id_column_name: "File".to_string(),
            table_group_column_name: "Group".to_string(),
        }
    }
}

impl NativeMarkdownSettings {
    pub(super) fn read(bob_dir: &Path) -> Self {
        let mut settings = Self::default();
        let path = bob_dir.join(".obsidian/plugins/dataview/data.json");
        let Ok(contents) = fs::read_to_string(path) else {
            return settings;
        };
        let Ok(value) = serde_json::from_str::<Value>(&contents) else {
            return settings;
        };

        settings.apply(&value);
        settings
    }

    pub(super) fn apply(&mut self, value: &Value) {
        if let Some(render_null_as) =
            value.get("renderNullAs").and_then(Value::as_str)
        {
            self.render_null_as = render_null_as.to_string();
        }
        if let Some(column_name) =
            value.get("tableIdColumnName").and_then(Value::as_str)
        {
            self.table_id_column_name = column_name.to_string();
        }
        if let Some(column_name) =
            value.get("tableGroupColumnName").and_then(Value::as_str)
        {
            self.table_group_column_name = column_name.to_string();
        }
    }
}
