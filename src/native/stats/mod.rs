//! Read-only Daily Stats engine types, config registry, index, and metric
//! evaluators. The integration phase wires these into `compute`.
#![allow(dead_code)]

pub(crate) mod compute;
pub(crate) mod error;
pub(crate) mod formula;
pub(crate) mod index;
pub(crate) mod journal;
pub(crate) mod metrics;
pub(crate) mod range;
pub(crate) mod registry;
pub(crate) mod report;
pub(crate) mod request;
pub(crate) mod series;
pub(crate) mod value;

#[allow(unused_imports)]
pub(crate) use compute::compute;
#[allow(unused_imports)]
pub(crate) use error::StatsError;
#[allow(unused_imports)]
pub(crate) use formula::evaluate_formula;
#[allow(unused_imports)]
pub(crate) use journal::evaluate_journal;
#[allow(unused_imports)]
pub(crate) use metrics::{evaluate_metric, MetricEvaluation};
#[allow(unused_imports)]
pub(crate) use range::RangeSpec;
#[allow(unused_imports)]
pub(crate) use registry::{build_registry, MetricDef, MetricRegistry};
#[allow(unused_imports)]
pub(crate) use report::{StatsErrorEnvelope, StatsReport, SCHEMA_VERSION};
#[allow(unused_imports)]
pub(crate) use request::StatsRequest;
#[allow(unused_imports)]
pub(crate) use value::DailyValue;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::NaiveDate;

    use super::*;
    use crate::native::config::{load_stats_config, parse_stats_config};

    #[test]
    fn public_surface_wires_config_registry_and_request() {
        let config = parse_stats_config(
            "stats:\n",
            PathBuf::from("/config.yml").as_path(),
        )
        .unwrap();
        let registry: MetricRegistry = build_registry(&config);
        let worked: &MetricDef = registry.get("worked").unwrap();
        assert_eq!(worked.kind_name(), "pomodoro");
        let today = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let request = StatsRequest::at(
            PathBuf::from("/vault"),
            PathBuf::from("/config.yml"),
            today.and_hms_opt(0, 0, 0).unwrap(),
        );
        assert_eq!(request.now.date(), today);
        assert_eq!(RangeSpec::four_weeks().label(), "4w");
        let error = StatsError::unknown_metric("nope");
        let envelope: StatsErrorEnvelope = error.envelope();
        assert_eq!(envelope.schema_version, SCHEMA_VERSION);
        let _report = StatsReport {
            ok: true,
            schema_version: SCHEMA_VERSION,
            today,
            range: report::RangeReport {
                preset: Some("4w".to_string()),
                offset: 0,
                start: today,
                end: today,
                days: 0,
                includes_today: true,
                group: report::GroupName::Day,
                label: "today".to_string(),
                prior: None,
            },
            metrics: Vec::new(),
            health: report::HealthReport::default(),
            warnings: Vec::new(),
        };
        let _value = DailyValue {
            date: today,
            value: None,
            num: None,
            den: None,
            quality: value::DataQuality::Missing,
            reason: Some("untracked".to_string()),
            provisional: false,
            parts: Default::default(),
        };
        let missing = load_stats_config(
            PathBuf::from("/definitely/missing.yml").as_path(),
        )
        .unwrap();
        assert_eq!(missing.range, RangeSpec::four_weeks());
    }
}
