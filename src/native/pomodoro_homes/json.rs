use serde::Serialize;

use super::Hue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionState {
    Attributed,
    Forecast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionKind {
    Work,
    Unlinked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HomeKind {
    Project,
    Area,
    Bucket,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HomePart {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: HomeKind,
    pub(crate) area: Option<String>,
    pub(crate) area_label: Option<String>,
    pub(crate) hue: Option<Hue>,
    pub(crate) tasks: u32,
    pub(crate) share: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) minutes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Homes {
    pub(crate) state: SessionState,
    pub(crate) kind: SessionKind,
    pub(crate) label: String,
    pub(crate) chip: Option<String>,
    pub(crate) hue: Option<Hue>,
    pub(crate) parts: Vec<HomePart>,
}
