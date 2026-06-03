use std::collections::BTreeMap;

use serde_json::{Number, Value};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum DataviewValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Date(String),
    DateTime(String),
    Duration(String),
    Link(DataviewLink),
    Array(Vec<DataviewValue>),
    Object(BTreeMap<String, DataviewValue>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DataviewLink {
    pub(super) path: String,
    pub(super) display: Option<String>,
    pub(super) embed: bool,
    pub(super) raw_target: String,
}

impl DataviewValue {
    pub(super) fn is_truthy(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Null => false,
            Self::String(value)
            | Self::Date(value)
            | Self::DateTime(value)
            | Self::Duration(value) => !value.is_empty(),
            Self::Number(number) => number.as_f64() != Some(0.0),
            Self::Link(_) => true,
            Self::Array(values) => !values.is_empty(),
            Self::Object(values) => !values.is_empty(),
        }
    }

    pub(super) fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub(super) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub(super) fn to_plain_json(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(*value),
            Self::Number(value) => Value::Number(value.clone()),
            Self::String(value)
            | Self::Date(value)
            | Self::DateTime(value)
            | Self::Duration(value) => Value::String(value.clone()),
            Self::Link(link) => link.to_plain_json(),
            Self::Array(values) => {
                Value::Array(values.iter().map(Self::to_plain_json).collect())
            }
            Self::Object(values) => Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_plain_json()))
                    .collect(),
            ),
        }
    }
}

impl DataviewLink {
    pub(super) fn new(
        path: String,
        display: Option<String>,
        embed: bool,
        raw_target: String,
    ) -> Self {
        Self {
            path,
            display,
            embed,
            raw_target,
        }
    }

    pub(super) fn page(path: &str) -> Self {
        Self {
            path: path.to_string(),
            display: None,
            embed: false,
            raw_target: path.to_string(),
        }
    }

    pub(super) fn to_plain_json(&self) -> Value {
        serde_json::json!({
            "type": "link",
            "path": self.path,
            "display": self.display,
            "embed": self.embed,
        })
    }
}
