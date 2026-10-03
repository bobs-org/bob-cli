use std::{collections::BTreeMap, sync::Arc};

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
    Array(Arc<Vec<DataviewValue>>),
    Object(Arc<BTreeMap<String, DataviewValue>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DataviewLink {
    pub(super) path: String,
    pub(super) display: Option<String>,
    pub(super) embed: bool,
    pub(super) raw_target: String,
}

impl DataviewValue {
    pub(super) fn array(values: Vec<Self>) -> Self {
        Self::Array(Arc::new(values))
    }

    pub(super) fn object(values: BTreeMap<String, Self>) -> Self {
        Self::Object(Arc::new(values))
    }

    pub(super) fn into_vec(self) -> Result<Vec<Self>, Self> {
        match self {
            Self::Array(values) => Ok(unwrap_shared(values)),
            value => Err(value),
        }
    }

    pub(super) fn into_array_items(self) -> Vec<Self> {
        match self {
            Self::Array(values) => unwrap_shared(values),
            Self::Null => Vec::new(),
            value => vec![value],
        }
    }

    pub(super) fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(values) => Some(values.as_slice()),
            _ => None,
        }
    }

    pub(super) fn as_object(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Object(values) => Some(values),
            _ => None,
        }
    }

    pub(super) fn object_mut(&mut self) -> Option<&mut BTreeMap<String, Self>> {
        match self {
            Self::Object(values) => Some(Arc::make_mut(values)),
            _ => None,
        }
    }

    pub(super) fn array_mut(&mut self) -> Option<&mut Vec<Self>> {
        match self {
            Self::Array(values) => Some(Arc::make_mut(values)),
            _ => None,
        }
    }

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

pub(super) fn unwrap_shared<T: Clone>(arc: Arc<T>) -> T {
    Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone())
}
