use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde::Serialize;

use super::value::DataQuality;

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Schema-1 success report. Dates serialize as ISO strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct StatsReport {
    pub(crate) ok: bool,
    pub(crate) schema_version: u32,
    #[serde(serialize_with = "iso_date")]
    pub(crate) today: NaiveDate,
    pub(crate) range: RangeReport,
    pub(crate) metrics: Vec<MetricReport>,
    pub(crate) health: HealthReport,
    pub(crate) warnings: Vec<StatsWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RangeReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) preset: Option<String>,
    pub(crate) offset: u32,
    #[serde(serialize_with = "iso_date")]
    pub(crate) start: NaiveDate,
    #[serde(serialize_with = "iso_date")]
    pub(crate) end: NaiveDate,
    pub(crate) days: u32,
    pub(crate) includes_today: bool,
    pub(crate) group: GroupName,
    pub(crate) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prior: Option<PriorRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GroupName {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PriorRange {
    #[serde(serialize_with = "iso_date")]
    pub(crate) start: NaiveDate,
    #[serde(serialize_with = "iso_date")]
    pub(crate) end: NaiveDate,
    pub(crate) label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MetricReport {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) kind: String,
    pub(crate) unit: String,
    pub(crate) direction: String,
    pub(crate) color: String,
    pub(crate) source: String,
    pub(crate) origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(serialize_with = "iso_date_opt")]
    pub(crate) since: Option<NaiveDate>,
    pub(crate) definition: String,
    pub(crate) limits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parameters: Option<MetricParameters>,
    pub(crate) summary: MetricSummary,
    pub(crate) points: Vec<SeriesPoint>,
    pub(crate) prior_points: Vec<SeriesPoint>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) parts_summary: BTreeMap<String, Vec<PartSummary>>,
    pub(crate) warnings: Vec<StatsWarning>,
}

/// Kind-specific validated parameters retained for the metrics catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MetricParameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) measure: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) themes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) exclude_themes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) event: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags_any: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags_all: Vec<String>,
    #[serde(rename = "in", skip_serializing_if = "Vec::is_empty")]
    pub(crate) in_notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) task_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recurring: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags: Vec<String>,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub(crate) match_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) field_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) expr: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) dependencies: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) select: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tiers: Option<JournalTiersJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum JournalTiersJson {
    Named(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MetricSummary {
    pub(crate) headline: Headline,
    pub(crate) total: Option<ValueDisplay>,
    pub(crate) mean: Option<i64>,
    pub(crate) p25: Option<i64>,
    pub(crate) p75: Option<i64>,
    pub(crate) days_with_data: u32,
    pub(crate) days_met: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prior: Option<PriorHeadline>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) delta: Option<Delta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Headline {
    pub(crate) value: Option<i64>,
    pub(crate) display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) suffix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stat: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ValueDisplay {
    pub(crate) value: Option<i64>,
    pub(crate) display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PriorHeadline {
    pub(crate) headline: Headline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Delta {
    pub(crate) display: String,
    pub(crate) words: String,
    pub(crate) sign: i8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SeriesPoint {
    #[serde(serialize_with = "iso_date")]
    pub(crate) date: NaiveDate,
    #[serde(serialize_with = "iso_date")]
    pub(crate) end: NaiveDate,
    pub(crate) value: Option<i64>,
    pub(crate) display: String,
    pub(crate) quality: DataQuality,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    pub(crate) provisional: bool,
    pub(crate) rolling_median: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) num: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) den: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) partial: Option<bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) parts: BTreeMap<String, BTreeMap<String, i64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PartSummary {
    pub(crate) key: String,
    pub(crate) value: i64,
    pub(crate) display: String,
    pub(crate) share: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct HealthReport {
    pub(crate) journal: JournalHealthReport,
    pub(crate) tasks: TaskHealthReport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct JournalHealthReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(serialize_with = "iso_date_opt")]
    pub(crate) since: Option<NaiveDate>,
    pub(crate) malformed_lines: u32,
    pub(crate) hosts: Vec<JournalHostReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct JournalHostReport {
    pub(crate) host: String,
    pub(crate) events: u32,
    pub(crate) last_ts: String,
    #[serde(serialize_with = "iso_date")]
    pub(crate) last_day: NaiveDate,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct TaskHealthReport {
    pub(crate) invalid_dates: u32,
    pub(crate) closed_without_date: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) missing_created: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct StatsWarning {
    pub(crate) code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) metric: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(serialize_with = "iso_date_opt")]
    pub(crate) date: Option<NaiveDate>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct StatsErrorEnvelope {
    pub(crate) ok: bool,
    pub(crate) schema_version: u32,
    pub(crate) error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ErrorBody {
    pub(crate) code: String,
    pub(crate) message: String,
}

fn iso_date<S>(date: &NaiveDate, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&date.format("%Y-%m-%d").to_string())
}

fn iso_date_opt<S>(
    date: &Option<NaiveDate>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match date {
        Some(date) => iso_date(date, serializer),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn success_report_matches_schema_one_shape() {
        let mut parts = BTreeMap::new();
        parts.insert(
            "theme".to_string(),
            BTreeMap::from([("GTD".to_string(), 25)]),
        );
        let mut parts_summary = BTreeMap::new();
        parts_summary.insert(
            "theme".to_string(),
            vec![PartSummary {
                key: "SASE".to_string(),
                value: 3330,
                display: "55h 30m".to_string(),
                share: "41%".to_string(),
            }],
        );
        let report = StatsReport {
            ok: true,
            schema_version: SCHEMA_VERSION,
            today: date(2026, 10, 11),
            range: RangeReport {
                preset: Some("4w".to_string()),
                offset: 0,
                start: date(2026, 9, 13),
                end: date(2026, 10, 10),
                days: 28,
                includes_today: true,
                group: GroupName::Day,
                label: "Sep 13 – Oct 10".to_string(),
                prior: Some(PriorRange {
                    start: date(2026, 8, 16),
                    end: date(2026, 9, 12),
                    label: "Aug 16 – Sep 12".to_string(),
                }),
            },
            metrics: vec![MetricReport {
                id: "worked".to_string(),
                label: "Worked".to_string(),
                kind: "pomodoro".to_string(),
                unit: "minutes".to_string(),
                direction: "up".to_string(),
                color: "orange".to_string(),
                source: "derived".to_string(),
                origin: "builtin".to_string(),
                since: Some(date(2026, 6, 1)),
                definition: "Minutes in closed Pomodoros.".to_string(),
                limits: vec![
                    "Open Pomodoros are plans and never count.".to_string()
                ],
                parameters: Some(MetricParameters {
                    measure: Some("minutes".to_string()),
                    themes: Vec::new(),
                    exclude_themes: Vec::new(),
                    event: None,
                    tags_any: Vec::new(),
                    tags_all: Vec::new(),
                    in_notes: Vec::new(),
                    task_kind: None,
                    recurring: None,
                    tags: Vec::new(),
                    match_text: None,
                    field: None,
                    field_source: None,
                    expr: None,
                    dependencies: Vec::new(),
                    select: None,
                    tiers: None,
                }),
                summary: MetricSummary {
                    headline: Headline {
                        value: Some(290),
                        display: "4h 50m".to_string(),
                        suffix: Some("/day".to_string()),
                        stat: Some("median".to_string()),
                    },
                    total: Some(ValueDisplay {
                        value: Some(8120),
                        display: "135h 20m".to_string(),
                    }),
                    mean: Some(290),
                    p25: Some(220),
                    p75: Some(330),
                    days_with_data: 28,
                    days_met: None,
                    prior: Some(PriorHeadline {
                        headline: Headline {
                            value: Some(274),
                            display: "4h 34m".to_string(),
                            suffix: None,
                            stat: None,
                        },
                    }),
                    delta: Some(Delta {
                        display: "▲ 6%".to_string(),
                        words: "up 6% vs the prior 4 weeks".to_string(),
                        sign: 1,
                    }),
                },
                points: vec![
                    SeriesPoint {
                        date: date(2026, 9, 14),
                        end: date(2026, 9, 14),
                        value: None,
                        display: "–".to_string(),
                        quality: DataQuality::Missing,
                        reason: Some("no daily note".to_string()),
                        provisional: false,
                        rolling_median: Some(255),
                        num: None,
                        den: None,
                        partial: None,
                        parts: BTreeMap::new(),
                    },
                    SeriesPoint {
                        date: date(2026, 10, 11),
                        end: date(2026, 10, 11),
                        value: Some(25),
                        display: "25m".to_string(),
                        quality: DataQuality::Ok,
                        reason: None,
                        provisional: true,
                        rolling_median: None,
                        num: None,
                        den: None,
                        partial: None,
                        parts,
                    },
                ],
                prior_points: Vec::new(),
                parts_summary,
                warnings: Vec::new(),
            }],
            health: HealthReport {
                journal: JournalHealthReport {
                    since: Some(date(2026, 10, 12)),
                    malformed_lines: 0,
                    hosts: vec![JournalHostReport {
                        host: "apollo".to_string(),
                        events: 41,
                        last_ts: "2026-10-11T05:41:07-04:00".to_string(),
                        last_day: date(2026, 10, 11),
                    }],
                },
                tasks: TaskHealthReport {
                    invalid_dates: 3,
                    closed_without_date: 220,
                    missing_created: None,
                },
            },
            warnings: vec![StatsWarning {
                code: "journal_gap".to_string(),
                metric: Some("reviewed".to_string()),
                date: Some(date(2026, 10, 9)),
                message: "Review closed but no answers were recorded"
                    .to_string(),
            }],
        };

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["today"], "2026-10-11");
        assert_eq!(json["range"]["preset"], "4w");
        assert_eq!(json["range"]["start"], "2026-09-13");
        assert_eq!(json["range"]["end"], "2026-10-10");
        assert_eq!(json["metrics"][0]["origin"], "builtin");
        assert_eq!(json["metrics"][0]["parameters"]["measure"], "minutes");
        assert_eq!(
            json["metrics"][0]["points"][0]["value"],
            serde_json::Value::Null
        );
        assert!(json["metrics"][0]["points"][0].get("parts").is_none());
        assert_eq!(json["metrics"][0]["points"][0]["reason"], "no daily note");
        assert_eq!(
            json["metrics"][0]["points"][1]["parts"]["theme"]["GTD"],
            25
        );
        assert_eq!(
            json["metrics"][0]["points"][1]["rolling_median"],
            serde_json::Value::Null
        );
        assert!(json["metrics"][0]["points"][0].get("partial").is_none());
        assert_eq!(json["health"]["tasks"]["invalid_dates"], 3);
        assert!(json["health"]["tasks"].get("missing_created").is_none());
        assert_eq!(json["warnings"][0]["date"], "2026-10-09");
    }

    #[test]
    fn ratio_and_grouped_fields_are_optional() {
        let point = SeriesPoint {
            date: date(2026, 10, 1),
            end: date(2026, 10, 7),
            value: Some(1000),
            display: "10%".to_string(),
            quality: DataQuality::Ok,
            reason: None,
            provisional: false,
            rolling_median: Some(1000),
            num: Some(1),
            den: Some(10),
            partial: Some(true),
            parts: BTreeMap::new(),
        };
        let json = serde_json::to_value(&point).unwrap();
        assert_eq!(json["num"], 1);
        assert_eq!(json["den"], 10);
        assert_eq!(json["partial"], true);
        assert_eq!(json["date"], "2026-10-01");
        assert_eq!(json["end"], "2026-10-07");
        let _ = [GroupName::Week, GroupName::Month];
    }
}
