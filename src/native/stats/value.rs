use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde::Serialize;

/// Evaluator output for one calendar day. Formatting, grouping, and
/// rolling medians are added later; evaluators never format UI text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DailyValue {
    pub(crate) date: NaiveDate,
    pub(crate) value: Option<i64>,
    pub(crate) num: Option<i64>,
    pub(crate) den: Option<i64>,
    pub(crate) quality: DataQuality,
    pub(crate) reason: Option<String>,
    pub(crate) provisional: bool,
    pub(crate) parts: BTreeMap<String, BTreeMap<String, i64>>,
    /// Minutes in each part that came from sessions with two or more keys
    /// in that dimension. Empty for count metrics.
    pub(crate) apportioned: BTreeMap<String, BTreeMap<String, i64>>,
    /// Checklist calendar state, set only by checklist evaluators.
    pub(crate) checklist_state: Option<ChecklistState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DataQuality {
    Ok,
    Partial,
    Missing,
    NotTracked,
}

/// Daily checklist calendar mark. Aggregated buckets never carry this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChecklistState {
    Met,
    Missed,
    NotDue,
    Missing,
    NotTracked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_serializes_snake_case() {
        let json = serde_json::to_value(DataQuality::NotTracked).unwrap();
        assert_eq!(json, "not_tracked");
        assert_eq!(
            serde_json::to_value(DataQuality::Partial).unwrap(),
            "partial"
        );
        let mut parts = BTreeMap::new();
        parts.insert(
            "theme".to_string(),
            BTreeMap::from([("GTD".to_string(), 25)]),
        );
        let value = DailyValue {
            date: NaiveDate::from_ymd_opt(2026, 10, 11).unwrap(),
            value: Some(25),
            num: None,
            den: None,
            quality: DataQuality::Ok,
            reason: None,
            provisional: true,
            parts,
            apportioned: BTreeMap::new(),
            checklist_state: None,
        };
        assert!(value.provisional);
        assert_eq!(value.parts["theme"]["GTD"], 25);
    }
}
