//! Period-strip charts: top-N roots, configured types, stacks, and
//! `display_short`. bob ranks and folds; the plugin only paints.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::native::{
    config::themes::ThemeSources, stats::series::format_minutes,
};

const CHARTS_VERSION: u32 = 1;
const COMPONENTS_KEEP: usize = 7;

const UNLINKED: &str = "@unlinked";
const UNFILED: &str = "@unfiled";
const UNNAMED: &str = "@unnamed";
const OTHER_NAMES: &str = "@other_names";
const REST_KEY: &str = "@rest";
const UNATTRIBUTED_KEY: &str = "@unattributed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypesPolicy {
    Configured,
    Components,
}

impl TypesPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Components => "components",
        }
    }
}

pub(crate) fn types_policy(sources: &ThemeSources) -> TypesPolicy {
    if sources.themes.is_some() {
        TypesPolicy::Configured
    } else {
        TypesPolicy::Components
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeriesKind {
    Root,
    Type,
    Rest,
    Unattributed,
}

impl SeriesKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Type => "type",
            Self::Rest => "rest",
            Self::Unattributed => "unattributed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    key: String,
    label: String,
    minutes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Series {
    key: String,
    kind: SeriesKind,
    label: String,
    minutes: i64,
    members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Charts {
    roots_limit: u8,
    types_policy: TypesPolicy,
    roots: Vec<Series>,
    types: Vec<Series>,
    root_map: BTreeMap<String, String>,
    type_map: BTreeMap<String, String>,
}

pub(crate) fn empty(limit: u8, policy: TypesPolicy) -> Charts {
    Charts {
        roots_limit: limit,
        types_policy: policy,
        roots: Vec::new(),
        types: Vec::new(),
        root_map: BTreeMap::new(),
        type_map: BTreeMap::new(),
    }
}

/// Whole-hours form (`35h`, rounded half up) when minutes ≥ 600;
/// otherwise the ordinary `display`.
pub(crate) fn display_short(minutes: i64) -> String {
    let abs = minutes.unsigned_abs();
    if abs >= 600 {
        let hours = (abs + 30) / 60;
        if minutes < 0 {
            format!("−{hours}h")
        } else {
            format!("{hours}h")
        }
    } else {
        format_minutes(minutes)
    }
}

pub(crate) fn display_short_for(value: Option<i64>, display: &str) -> String {
    match value {
        Some(minutes) => display_short(minutes),
        None => display.to_string(),
    }
}

pub(crate) fn empty_stacks() -> Value {
    json!({ "roots": [], "types": [] })
}

pub(crate) fn build(
    parts: &BTreeMap<String, BTreeMap<String, i64>>,
    worked: i64,
    homes: &Value,
    theme_hues: &Value,
    top_roots: u8,
    policy: TypesPolicy,
) -> Charts {
    if worked <= 0 {
        return empty(top_roots, policy);
    }
    let roots_parts = root_parts(parts);
    let types_parts = parts.get("task_type").cloned().unwrap_or_default();
    let (roots, root_map) = root_series(&roots_parts, top_roots, homes, worked);
    let (types, type_map) =
        type_series(&types_parts, policy, theme_hues, worked);
    Charts {
        roots_limit: top_roots,
        types_policy: policy,
        roots,
        types,
        root_map,
        type_map,
    }
}

pub(crate) fn charts_json(charts: &Charts, worked: i64) -> Value {
    json!({
        "version": CHARTS_VERSION,
        "roots": {
            "limit": charts.roots_limit,
            "dimension": "root_parent",
            "series": series_list_json(&charts.roots, worked),
        },
        "types": {
            "policy": charts.types_policy.as_str(),
            "series": series_list_json(&charts.types, worked),
        },
    })
}

pub(crate) fn stacks_json(
    charts: &Charts,
    parts: &BTreeMap<String, BTreeMap<String, i64>>,
) -> Value {
    json!({
        "roots": stack_entries(&charts.roots, &charts.root_map, &root_parts(parts)),
        "types": stack_entries(
            &charts.types,
            &charts.type_map,
            parts.get("task_type").unwrap_or(&BTreeMap::new()),
        ),
    })
}

fn root_parts(
    parts: &BTreeMap<String, BTreeMap<String, i64>>,
) -> BTreeMap<String, i64> {
    parts
        .get("root_parent")
        .filter(|values| !values.is_empty())
        .cloned()
        .or_else(|| parts.get("area").cloned())
        .unwrap_or_default()
}

fn root_series(
    parts: &BTreeMap<String, i64>,
    top_roots: u8,
    homes: &Value,
    worked: i64,
) -> (Vec<Series>, BTreeMap<String, String>) {
    let mut named = Vec::new();
    let mut unlinked = 0_i64;
    let mut unfiled = 0_i64;
    let mut map = BTreeMap::new();
    for (key, minutes) in parts {
        if *minutes <= 0 {
            continue;
        }
        if key == UNLINKED {
            unlinked = unlinked.saturating_add(*minutes);
            map.insert(key.clone(), UNATTRIBUTED_KEY.to_string());
        } else if key == UNFILED {
            unfiled = unfiled.saturating_add(*minutes);
            map.insert(key.clone(), UNATTRIBUTED_KEY.to_string());
        } else if key.starts_with('@') {
            continue;
        } else {
            named.push((key.clone(), *minutes));
        }
    }
    named.sort_by(|(key_a, minutes_a), (key_b, minutes_b)| {
        minutes_b.cmp(minutes_a).then_with(|| key_a.cmp(key_b))
    });
    let keep = usize::from(top_roots).min(named.len());
    let (kept, folded) = named.split_at(keep);
    let mut series = Vec::new();
    for (key, minutes) in kept {
        map.insert(key.clone(), key.clone());
        series.push(Series {
            key: key.clone(),
            kind: SeriesKind::Root,
            label: home_label(homes, key),
            minutes: *minutes,
            members: Vec::new(),
        });
    }
    if !folded.is_empty() {
        let members: Vec<Member> = folded
            .iter()
            .map(|(key, minutes)| {
                map.insert(key.clone(), REST_KEY.to_string());
                Member {
                    key: key.clone(),
                    label: home_label(homes, key),
                    minutes: *minutes,
                }
            })
            .collect();
        let minutes = members.iter().map(|member| member.minutes).sum();
        series.push(Series {
            key: REST_KEY.to_string(),
            kind: SeriesKind::Rest,
            label: format!("{} more", members.len()),
            minutes,
            members,
        });
    }
    let unattributed = unlinked.saturating_add(unfiled);
    if unattributed > 0 || (series.is_empty() && worked > 0) {
        let minutes = if unattributed > 0 {
            unattributed
        } else {
            worked
        };
        let mut members = Vec::new();
        if unlinked > 0 {
            members.push(Member {
                key: UNLINKED.to_string(),
                label: "unlinked".to_string(),
                minutes: unlinked,
            });
        }
        if unfiled > 0 {
            members.push(Member {
                key: UNFILED.to_string(),
                label: "unfiled".to_string(),
                minutes: unfiled,
            });
        }
        let label = if unlinked > 0 && unfiled > 0 {
            "no root"
        } else if unfiled > 0 {
            "unfiled"
        } else {
            "unlinked"
        };
        series.push(Series {
            key: UNATTRIBUTED_KEY.to_string(),
            kind: SeriesKind::Unattributed,
            label: label.to_string(),
            minutes,
            members,
        });
    }
    (series, map)
}

fn type_series(
    parts: &BTreeMap<String, i64>,
    policy: TypesPolicy,
    theme_hues: &Value,
    worked: i64,
) -> (Vec<Series>, BTreeMap<String, String>) {
    let mut named = Vec::new();
    let mut unnamed = 0_i64;
    let mut other_names = 0_i64;
    let mut map = BTreeMap::new();
    for (key, minutes) in parts {
        if *minutes <= 0 {
            continue;
        }
        if key == UNNAMED {
            unnamed = unnamed.saturating_add(*minutes);
            map.insert(key.clone(), UNATTRIBUTED_KEY.to_string());
        } else if key == OTHER_NAMES {
            other_names = other_names.saturating_add(*minutes);
            map.insert(key.clone(), UNATTRIBUTED_KEY.to_string());
        } else if key.starts_with('@') {
            continue;
        } else {
            named.push((key.clone(), *minutes));
        }
    }
    named.sort_by(|(key_a, minutes_a), (key_b, minutes_b)| {
        minutes_b.cmp(minutes_a).then_with(|| key_a.cmp(key_b))
    });
    let keep = match policy {
        TypesPolicy::Configured => named.len(),
        TypesPolicy::Components => COMPONENTS_KEEP.min(named.len()),
    };
    let (kept, folded) = named.split_at(keep);
    let mut series = Vec::new();
    for (key, minutes) in kept {
        map.insert(key.clone(), key.clone());
        series.push(Series {
            key: key.clone(),
            kind: SeriesKind::Type,
            label: type_label(theme_hues, key),
            minutes: *minutes,
            members: Vec::new(),
        });
    }
    if !folded.is_empty() {
        let members: Vec<Member> = folded
            .iter()
            .map(|(key, minutes)| {
                map.insert(key.clone(), REST_KEY.to_string());
                Member {
                    key: key.clone(),
                    label: type_label(theme_hues, key),
                    minutes: *minutes,
                }
            })
            .collect();
        let minutes = members.iter().map(|member| member.minutes).sum();
        series.push(Series {
            key: REST_KEY.to_string(),
            kind: SeriesKind::Rest,
            label: format!("{} more types", members.len()),
            minutes,
            members,
        });
    }
    let unattributed = unnamed.saturating_add(other_names);
    if unattributed > 0 || (series.is_empty() && worked > 0) {
        let minutes = if unattributed > 0 {
            unattributed
        } else {
            worked
        };
        let mut members = Vec::new();
        if unnamed > 0 {
            members.push(Member {
                key: UNNAMED.to_string(),
                label: "unnamed".to_string(),
                minutes: unnamed,
            });
        }
        if other_names > 0 {
            members.push(Member {
                key: OTHER_NAMES.to_string(),
                label: "other names".to_string(),
                minutes: other_names,
            });
        }
        series.push(Series {
            key: UNATTRIBUTED_KEY.to_string(),
            kind: SeriesKind::Unattributed,
            label: "untyped".to_string(),
            minutes,
            members,
        });
    }
    (series, map)
}

fn stack_entries(
    series: &[Series],
    map: &BTreeMap<String, String>,
    parts: &BTreeMap<String, i64>,
) -> Vec<Value> {
    let mut totals: BTreeMap<String, i64> = BTreeMap::new();
    for (key, minutes) in parts {
        if *minutes <= 0 {
            continue;
        }
        if let Some(series_key) = map.get(key) {
            *totals.entry(series_key.clone()).or_default() += *minutes;
        }
    }
    series
        .iter()
        .filter_map(|entry| {
            let minutes = totals.get(&entry.key).copied().unwrap_or(0);
            (minutes > 0)
                .then(|| json!({ "key": entry.key, "minutes": minutes }))
        })
        .collect()
}

fn series_list_json(series: &[Series], total: i64) -> Vec<Value> {
    series
        .iter()
        .map(|entry| series_json(entry, total))
        .collect()
}

fn series_json(series: &Series, total: i64) -> Value {
    let mut value = json!({
        "key": series.key,
        "kind": series.kind.as_str(),
        "label": series.label,
        "minutes": series.minutes,
        "display": format_minutes(series.minutes),
        "display_short": display_short(series.minutes),
        "share": percent(series.minutes, total),
    });
    if !series.members.is_empty() {
        value["members"] = Value::Array(
            series
                .members
                .iter()
                .map(|member| {
                    json!({
                        "key": member.key,
                        "label": member.label,
                        "minutes": member.minutes,
                        "display": format_minutes(member.minutes),
                    })
                })
                .collect(),
        );
    }
    value
}

fn home_label(homes: &Value, key: &str) -> String {
    homes
        .get(key)
        .and_then(|home| home.get("label"))
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_string()
}

fn type_label(theme_hues: &Value, key: &str) -> String {
    theme_hues
        .get(key)
        .and_then(|entry| entry.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_string()
}

fn percent(value: i64, total: i64) -> String {
    if total <= 0 {
        return "0%".to_string();
    }
    let rounded = (i128::from(value).saturating_mul(100)
        + i128::from(total) / 2)
        / i128::from(total);
    format!("{rounded}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn homes(pairs: &[(&str, &str)]) -> Value {
        let mut map = serde_json::Map::new();
        for (key, label) in pairs {
            map.insert((*key).to_string(), json!({ "label": label }));
        }
        Value::Object(map)
    }

    fn hues(pairs: &[(&str, &str)]) -> Value {
        let mut map = serde_json::Map::new();
        for (key, name) in pairs {
            map.insert((*key).to_string(), json!({ "name": name }));
        }
        Value::Object(map)
    }

    fn parts(
        roots: &[(&str, i64)],
        types: &[(&str, i64)],
    ) -> BTreeMap<String, BTreeMap<String, i64>> {
        let mut out = BTreeMap::new();
        out.insert(
            "root_parent".to_string(),
            roots
                .iter()
                .map(|(key, minutes)| ((*key).to_string(), *minutes))
                .collect(),
        );
        out.insert(
            "task_type".to_string(),
            types
                .iter()
                .map(|(key, minutes)| ((*key).to_string(), *minutes))
                .collect(),
        );
        out
    }

    fn keys(series: &[Series]) -> Vec<&str> {
        series.iter().map(|entry| entry.key.as_str()).collect()
    }

    #[test]
    fn display_short_uses_whole_hours_from_ten_hours() {
        assert_eq!(display_short(599), "9h 59m");
        assert_eq!(display_short(600), "10h");
        assert_eq!(display_short(629), "10h");
        assert_eq!(display_short(630), "11h");
        assert_eq!(display_short(35), "35m");
        assert_eq!(display_short_for(None, "–"), "–");
    }

    #[test]
    fn empty_worked_has_empty_series() {
        let charts = build(
            &parts(&[("bob.md", 10)], &[("build", 10)]),
            0,
            &homes(&[("bob.md", "bob")]),
            &hues(&[("build", "BUILD")]),
            5,
            TypesPolicy::Configured,
        );
        assert!(charts.roots.is_empty());
        assert!(charts.types.is_empty());
        let json = charts_json(&charts, 0);
        assert_eq!(json["roots"]["limit"], 5);
        assert_eq!(json["roots"]["series"].as_array().unwrap().len(), 0);
        assert_eq!(json["types"]["policy"], "configured");
    }

    #[test]
    fn n_one_folds_the_rest_and_breaks_ties_by_key() {
        let charts = build(
            &parts(
                &[("zebra.md", 10), ("alpha.md", 10), ("mid.md", 5)],
                &[("build", 25)],
            ),
            25,
            &homes(&[
                ("zebra.md", "zebra"),
                ("alpha.md", "alpha"),
                ("mid.md", "mid"),
            ]),
            &hues(&[("build", "BUILD")]),
            1,
            TypesPolicy::Configured,
        );
        assert_eq!(keys(&charts.roots), ["alpha.md", "@rest"]);
        assert_eq!(charts.roots[1].label, "2 more");
        assert_eq!(
            charts.roots[1]
                .members
                .iter()
                .map(|member| member.key.as_str())
                .collect::<Vec<_>>(),
            ["zebra.md", "mid.md"]
        );
    }

    #[test]
    fn n_above_the_root_count_has_no_rest() {
        let charts = build(
            &parts(&[("bob.md", 10), ("sase.md", 5)], &[("build", 15)]),
            15,
            &homes(&[("bob.md", "bob"), ("sase.md", "sase")]),
            &hues(&[("build", "BUILD")]),
            5,
            TypesPolicy::Configured,
        );
        assert_eq!(keys(&charts.roots), ["bob.md", "sase.md"]);
        assert!(!keys(&charts.roots).contains(&"@rest"));
    }

    #[test]
    fn real_rest_and_other_notes_never_use_synthetic_keys() {
        let charts = build(
            &parts(
                &[
                    ("bob.md", 40),
                    ("rest.md", 20),
                    ("other.md", 10),
                    ("job.md", 5),
                ],
                &[("build", 75)],
            ),
            75,
            &homes(&[
                ("bob.md", "bob"),
                ("rest.md", "rest"),
                ("other.md", "other"),
                ("job.md", "job"),
            ]),
            &hues(&[("build", "BUILD")]),
            2,
            TypesPolicy::Configured,
        );
        assert_eq!(keys(&charts.roots), ["bob.md", "rest.md", "@rest"]);
        assert_eq!(charts.roots[1].kind, SeriesKind::Root);
        assert_eq!(charts.roots[1].label, "rest");
        assert_eq!(
            charts.roots[2]
                .members
                .iter()
                .map(|member| member.key.as_str())
                .collect::<Vec<_>>(),
            ["other.md", "job.md"]
        );
        let json = charts_json(&charts, 75);
        assert_eq!(json["roots"]["series"][1]["key"], "rest.md");
        assert_eq!(json["roots"]["series"][2]["key"], "@rest");
    }

    #[test]
    fn only_unattributed_time_is_one_hatched_series() {
        let charts = build(
            &parts(
                &[(UNLINKED, 20), (UNFILED, 10)],
                &[(UNNAMED, 20), (OTHER_NAMES, 10)],
            ),
            30,
            &json!({}),
            &json!({}),
            5,
            TypesPolicy::Configured,
        );
        assert_eq!(keys(&charts.roots), ["@unattributed"]);
        assert_eq!(charts.roots[0].label, "no root");
        assert_eq!(charts.roots[0].minutes, 30);
        assert_eq!(keys(&charts.types), ["@unattributed"]);
        assert_eq!(charts.types[0].label, "untyped");
        assert_eq!(charts.types[0].members[0].label, "unnamed");
        assert_eq!(charts.types[0].members[1].label, "other names");
    }

    #[test]
    fn unlinked_only_and_unfiled_only_pick_the_matching_label() {
        let unlinked = build(
            &parts(&[(UNLINKED, 12)], &[(UNNAMED, 12)]),
            12,
            &json!({}),
            &json!({}),
            5,
            TypesPolicy::Configured,
        );
        assert_eq!(unlinked.roots[0].label, "unlinked");
        let unfiled = build(
            &parts(&[(UNFILED, 8)], &[(UNNAMED, 8)]),
            8,
            &json!({}),
            &json!({}),
            5,
            TypesPolicy::Configured,
        );
        assert_eq!(unfiled.roots[0].label, "unfiled");
    }

    #[test]
    fn configured_types_keep_every_named_type_and_fold_buckets() {
        let charts = build(
            &parts(
                &[("bob.md", 55)],
                &[("build", 30), ("fix", 13), ("plan", 12), (UNNAMED, 0)],
            ),
            55,
            &homes(&[("bob.md", "bob")]),
            &hues(&[("build", "BUILD"), ("fix", "FIX"), ("plan", "PLAN")]),
            5,
            TypesPolicy::Configured,
        );
        assert_eq!(keys(&charts.types), ["build", "fix", "plan"]);
        assert_eq!(charts.types[0].label, "BUILD");
        assert_eq!(charts.types_policy, TypesPolicy::Configured);
    }

    #[test]
    fn mixed_type_plus_label_and_odd_split_and_unnamed_show_as_parts() {
        // BUILD + SASE (SASE is a label): all 30 minutes on BUILD.
        let labelled = build(
            &parts(&[("bob.md", 30)], &[("build", 30)]),
            30,
            &homes(&[("bob.md", "bob")]),
            &hues(&[("build", "BUILD")]),
            5,
            TypesPolicy::Configured,
        );
        assert_eq!(keys(&labelled.types), ["build"]);
        assert_eq!(labelled.types[0].minutes, 30);

        // BUILD + FIX over 25 minutes: 13 and 12.
        let split = build(
            &parts(&[("bob.md", 25)], &[("build", 13), ("fix", 12)]),
            25,
            &homes(&[("bob.md", "bob")]),
            &hues(&[("build", "BUILD"), ("fix", "FIX")]),
            5,
            TypesPolicy::Configured,
        );
        assert_eq!(split.types[0].minutes, 13);
        assert_eq!(split.types[1].minutes, 12);

        let unnamed = build(
            &parts(&[("bob.md", 20)], &[(UNNAMED, 20)]),
            20,
            &homes(&[("bob.md", "bob")]),
            &json!({}),
            5,
            TypesPolicy::Configured,
        );
        assert_eq!(keys(&unnamed.types), ["@unattributed"]);
        assert_eq!(unnamed.types[0].label, "untyped");
    }

    #[test]
    fn components_policy_keeps_seven_then_rest() {
        let mut types = BTreeMap::new();
        for index in 0..9 {
            types.insert(format!("t{index}"), i64::from(9 - index));
        }
        let total: i64 = types.values().sum();
        let mut all = BTreeMap::new();
        all.insert(
            "root_parent".to_string(),
            BTreeMap::from([("bob.md".to_string(), total)]),
        );
        all.insert("task_type".to_string(), types);
        let charts = build(
            &all,
            total,
            &homes(&[("bob.md", "bob")]),
            &json!({}),
            5,
            TypesPolicy::Components,
        );
        assert_eq!(charts.types.len(), 8);
        assert_eq!(charts.types[7].key, REST_KEY);
        assert_eq!(charts.types[7].label, "2 more types");
        assert_eq!(charts.types_policy, TypesPolicy::Components);
    }

    #[test]
    fn stacks_fold_members_and_sum_to_worked() {
        let parts = parts(
            &[
                ("bob.md", 40),
                ("job.md", 10),
                ("sase.md", 5),
                (UNLINKED, 5),
            ],
            &[("build", 40), ("fix", 15), (UNNAMED, 5)],
        );
        let charts = build(
            &parts,
            60,
            &homes(&[
                ("bob.md", "bob"),
                ("job.md", "job"),
                ("sase.md", "sase"),
            ]),
            &hues(&[("build", "BUILD"), ("fix", "FIX")]),
            1,
            TypesPolicy::Configured,
        );
        let stacks = stacks_json(&charts, &parts);
        assert_eq!(
            stacks["roots"],
            json!([
                { "key": "bob.md", "minutes": 40 },
                { "key": "@rest", "minutes": 15 },
                { "key": "@unattributed", "minutes": 5 },
            ])
        );
        let root_sum: i64 = stacks["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["minutes"].as_i64().unwrap())
            .sum();
        let type_sum: i64 = stacks["types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["minutes"].as_i64().unwrap())
            .sum();
        assert_eq!(root_sum, 60);
        assert_eq!(type_sum, 60);
    }

    #[test]
    fn share_rounds_like_lanes() {
        let charts = build(
            &parts(&[("bob.md", 1)], &[("build", 1)]),
            3,
            &homes(&[("bob.md", "bob")]),
            &hues(&[("build", "BUILD")]),
            5,
            TypesPolicy::Configured,
        );
        let json = charts_json(&charts, 3);
        assert_eq!(json["roots"]["series"][0]["share"], "33%");
    }

    #[test]
    fn unattributed_roots_never_take_a_top_n_slot() {
        let charts = build(
            &parts(
                &[(UNLINKED, 100), ("bob.md", 10), ("sase.md", 5)],
                &[("build", 115)],
            ),
            115,
            &homes(&[("bob.md", "bob"), ("sase.md", "sase")]),
            &hues(&[("build", "BUILD")]),
            1,
            TypesPolicy::Configured,
        );
        assert_eq!(keys(&charts.roots), ["bob.md", "@rest", "@unattributed"]);
        assert_eq!(charts.roots[0].minutes, 10);
        assert_eq!(charts.roots[2].minutes, 100);
    }
}
