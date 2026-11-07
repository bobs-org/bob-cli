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

/// Where a resolved home's hue came from (R8). Buckets carry none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HomeHueSource {
    /// The nearest area or project `color:` palette slot.
    Color,
    /// The nearest `color:` slot is grey: `hue` is null on purpose.
    Muted,
    /// No slot on the walk: the top-level area stem hash.
    Hash,
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) untracked: bool,
    /// Off-clock marker beside deprecated `untracked`. Both are set from
    /// the session clock; readers should prefer `off_clock`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) off_clock: bool,
    pub(crate) parts: Vec<HomePart>,
}
