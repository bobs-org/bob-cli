#![allow(dead_code)]
//! Pure Pomodoro name agreement and close settle (task_types agree-rule).
//!
//! `agree_entry` is the shared rule that keeps an open entry's name in
//! agreement with its Task Links. `settle_closed` names a closed session
//! from its worked types. Both are pure over plain data so the Ledger Tools
//! JS mirror consumes the same vectors. Future phases (reconcile-pass,
//! bob-writers) call these planners; allow dead code until they land.

use std::collections::BTreeSet;

use crate::native::{
    config::themes::ThemeSources,
    plan_budget::{normalize_component, split_components},
};

use super::routing::{is_meet_name, type_components};

/// One direct child of a ledger entry as seen by agreement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgreeChild {
    Live { key: String, name: String },
    Worked { key: String, name: String },
    Unknown,
    Prose,
}

/// One unrepaired item that agreement reports but never writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgreeUnrepaired {
    pub(crate) code: String,
    pub(crate) key: Option<String>,
}

/// Outcome of [`agree_entry`]: the new name plus what changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgreeOutcome {
    pub(crate) name: String,
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
    pub(crate) unrepaired: Vec<AgreeUnrepaired>,
}

/// One worked type for [`settle_closed`], in link order with catalog spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkedType {
    pub(crate) key: String,
    pub(crate) name: String,
}

fn is_type_key(key: &str, sources: &ThemeSources) -> bool {
    match &sources.themes {
        Some(vocabulary) => vocabulary.contains(key),
        None => !sources.inventory.contains(key),
    }
}

fn is_container(name: Option<&str>, sources: &ThemeSources) -> bool {
    type_components(name, sources)
        .iter()
        .all(|item| !item.is_type)
}

/// Pure agreement rule: return the agreed name for an open entry.
///
/// `name` is the entry's ` — NAME` tail (`None` for unnamed). `open` is
/// false for closed entries. `children` are the entry's direct children in
/// ledger order. `sources` is the type vocabulary plus exempt/inventory sets.
///
/// The rule never writes task fields and never moves links; it only renames
/// the tail. It is idempotent: `agree_entry` over its own output is a no-op.
pub(crate) fn agree_entry(
    name: Option<&str>,
    open: bool,
    children: &[AgreeChild],
    sources: &ThemeSources,
) -> AgreeOutcome {
    let original = name.unwrap_or("").to_string();
    let unchanged = || AgreeOutcome {
        name: original.clone(),
        added: Vec::new(),
        removed: Vec::new(),
        unrepaired: Vec::new(),
    };
    if !open {
        return unchanged();
    }
    if is_container(name, sources) {
        return unchanged();
    }
    if is_meet_name(name) {
        let mut unrepaired = Vec::new();
        for child in children {
            if let AgreeChild::Live { key, .. } = child
                && normalize_component(key) != "meet"
            {
                unrepaired.push(AgreeUnrepaired {
                    code: "meet_mismatch".to_string(),
                    key: Some(normalize_component(key)),
                });
            }
        }
        return AgreeOutcome {
            name: original,
            added: Vec::new(),
            removed: Vec::new(),
            unrepaired,
        };
    }

    let classified = type_components(name, sources);
    let component_keys: BTreeSet<String> = split_components(&original)
        .into_iter()
        .map(|component| normalize_component(&component))
        .collect();

    let mut added: Vec<(String, String)> = Vec::new();
    let mut seen_added: BTreeSet<String> = BTreeSet::new();
    let mut unrepaired: Vec<AgreeUnrepaired> = Vec::new();

    for child in children {
        match child {
            AgreeChild::Live { key, name } => {
                let key = normalize_component(key);
                if component_keys.contains(&key) {
                    continue;
                }
                if seen_added.contains(&key) {
                    continue;
                }
                if !is_type_key(&key, sources) {
                    unrepaired.push(AgreeUnrepaired {
                        code: "unconfigured_type".to_string(),
                        key: Some(key),
                    });
                    continue;
                }
                seen_added.insert(key.clone());
                added.push((key, name.clone()));
            }
            AgreeChild::Unknown => {
                unrepaired.push(AgreeUnrepaired {
                    code: "unknown_link".to_string(),
                    key: None,
                });
            }
            AgreeChild::Worked { .. } | AgreeChild::Prose => {}
        }
    }

    let can_drop = !children.is_empty()
        && children.iter().all(|child| {
            matches!(child, AgreeChild::Live { .. } | AgreeChild::Worked { .. })
        });

    let mut removed: Vec<String> = Vec::new();
    if can_drop {
        let supported: BTreeSet<String> = children
            .iter()
            .filter_map(|child| match child {
                AgreeChild::Live { key, .. }
                | AgreeChild::Worked { key, .. } => {
                    Some(normalize_component(key))
                }
                AgreeChild::Unknown | AgreeChild::Prose => None,
            })
            .collect();
        for item in &classified {
            if item.is_type && !supported.contains(&item.key) {
                removed.push(item.key.clone());
            }
        }
    }

    if removed.is_empty() && added.is_empty() {
        return AgreeOutcome {
            name: original,
            added: Vec::new(),
            removed: Vec::new(),
            unrepaired,
        };
    }

    let type_count = classified.iter().filter(|item| item.is_type).count();
    if removed.len() == type_count && added.is_empty() {
        return AgreeOutcome {
            name: original,
            added: Vec::new(),
            removed: Vec::new(),
            unrepaired,
        };
    }

    let added_keys: Vec<String> =
        added.iter().map(|(key, _)| key.clone()).collect();
    let new_name = if removed.len() == 1 && added.len() == 1 {
        substitute_one(&original, &removed[0], &added[0].1)
    } else {
        drop_then_append(&classified, &removed, &added)
    };

    if new_name == original {
        return AgreeOutcome {
            name: original,
            added: Vec::new(),
            removed: Vec::new(),
            unrepaired,
        };
    }

    AgreeOutcome {
        name: new_name,
        added: added_keys,
        removed,
        unrepaired,
    }
}

fn substitute_one(
    original: &str,
    removed_key: &str,
    added_name: &str,
) -> String {
    let parts = split_components(original);
    let mut out = Vec::with_capacity(parts.len());
    let mut replaced = false;
    for part in parts {
        if normalize_component(&part) == removed_key {
            if !replaced {
                out.push(added_name.to_string());
                replaced = true;
            }
            continue;
        }
        out.push(part);
    }
    if !replaced {
        out.push(added_name.to_string());
    }
    out.join(" + ")
}

fn drop_then_append(
    classified: &[super::routing::TypeComponent],
    removed: &[String],
    added: &[(String, String)],
) -> String {
    let remove_set: BTreeSet<&str> =
        removed.iter().map(String::as_str).collect();
    let mut parts = Vec::new();
    for item in classified {
        if item.is_type && remove_set.contains(item.key.as_str()) {
            continue;
        }
        parts.push(item.name.clone());
    }
    for (_, name) in added {
        parts.push(name.clone());
    }
    parts.join(" + ")
}

/// Pure close settle: name a closed session from its worked types.
///
/// `worked` carries the worked links' types in link order with catalog
/// spelling. Labels stay; type components whose key is worked stay in the
/// name's order and spelling; missing worked types append in link order when
/// they classify as types; one-drop-one-add substitutes in place. MEET
/// entries, containers, and closes with no typed worked link keep the name.
/// Prose never protects at close.
pub(crate) fn settle_closed(
    name: &str,
    worked: &[WorkedType],
    sources: &ThemeSources,
) -> String {
    if is_meet_name(Some(name)) {
        return name.to_string();
    }
    if is_container(Some(name), sources) {
        return name.to_string();
    }
    let mut typed_worked: Vec<&WorkedType> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for item in worked {
        let key = normalize_component(&item.key);
        if key.is_empty() || !seen.insert(key.clone()) {
            continue;
        }
        if is_type_key(&key, sources) {
            typed_worked.push(item);
        }
    }
    if typed_worked.is_empty() {
        return name.to_string();
    }
    let worked_keys: BTreeSet<String> = typed_worked
        .iter()
        .map(|item| normalize_component(&item.key))
        .collect();

    let classified = type_components(Some(name), sources);
    let mut kept: Vec<&super::routing::TypeComponent> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for item in &classified {
        if !item.is_type {
            continue;
        }
        if worked_keys.contains(&item.key) {
            kept.push(item);
        } else {
            dropped.push(item.key.clone());
        }
    }

    let mut to_add: Vec<&WorkedType> = Vec::new();
    let existing_type_keys: BTreeSet<String> = classified
        .iter()
        .filter(|item| item.is_type)
        .map(|item| item.key.clone())
        .collect();
    let mut seen_add: BTreeSet<String> = BTreeSet::new();
    for item in &typed_worked {
        let key = normalize_component(&item.key);
        if existing_type_keys.contains(&key) {
            continue;
        }
        if seen_add.insert(key) {
            to_add.push(item);
        }
    }

    if dropped.is_empty() && to_add.is_empty() {
        return name.to_string();
    }

    if dropped.len() == 1 && to_add.len() == 1 {
        return substitute_one(name, &dropped[0], &to_add[0].name);
    }

    let mut parts = Vec::new();
    for item in &classified {
        if item.is_type && !worked_keys.contains(&item.key) {
            continue;
        }
        parts.push(item.name.clone());
    }
    for item in to_add {
        parts.push(item.name.clone());
    }
    parts.join(" + ")
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::native::config::themes::{ConfiguredTheme, ThemesConfig};

    fn sources(
        vocab: &[String],
        exempt: &[String],
        inventory: &[String],
    ) -> ThemeSources {
        ThemeSources {
            exempt: exempt
                .iter()
                .map(|value| normalize_component(value))
                .collect(),
            inventory: inventory
                .iter()
                .map(|value| normalize_component(value))
                .collect(),
            themes: if vocab.is_empty() {
                None
            } else {
                Some(ThemesConfig {
                    entries: vocab
                        .iter()
                        .map(|name| ConfiguredTheme {
                            key: normalize_component(name),
                            name: name.clone(),
                            color: None,
                        })
                        .collect(),
                })
            },
            warning: None,
        }
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn opt_str(value: &Value) -> Option<String> {
        value.as_str().map(str::to_string)
    }

    fn parse_children(value: &Value) -> Vec<AgreeChild> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let kind = item["kind"].as_str().unwrap_or("prose");
                        match kind {
                            "live" => AgreeChild::Live {
                                key: normalize_component(
                                    item["key"].as_str().unwrap_or(""),
                                ),
                                name: item["name"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string(),
                            },
                            "worked" => AgreeChild::Worked {
                                key: normalize_component(
                                    item["key"].as_str().unwrap_or(""),
                                ),
                                name: item["name"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string(),
                            },
                            "unknown" => AgreeChild::Unknown,
                            _ => AgreeChild::Prose,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn shared_agree_entry_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/pomodoro_themes/vectors.json"
        ))
        .expect("shared vectors JSON");
        let cases = vectors["agree_entry"]
            .as_array()
            .expect("agree_entry vectors");
        assert!(!cases.is_empty());
        for case in cases {
            let id = case["name"].as_str().unwrap_or("?");
            let entry = opt_str(&case["entry"]);
            let open = case["open"].as_bool().unwrap_or(true);
            let children = parse_children(&case["children"]);
            let made = sources(
                &strings(&case["vocab"]),
                &strings(&case["exempt"]),
                &strings(&case["inventory"]),
            );
            let outcome = agree_entry(entry.as_deref(), open, &children, &made);
            let expected = &case["expected"];
            assert_eq!(
                outcome.name,
                expected["name"].as_str().unwrap_or(""),
                "{id} name"
            );
            let added: Vec<String> = expected["added"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(outcome.added, added, "{id} added");
            let removed: Vec<String> = expected["removed"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(outcome.removed, removed, "{id} removed");
            let want_unrepaired = expected["unrepaired"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                outcome.unrepaired.len(),
                want_unrepaired.len(),
                "{id} unrepaired len"
            );
            for (got, want) in
                outcome.unrepaired.iter().zip(want_unrepaired.iter())
            {
                assert_eq!(
                    got.code,
                    want["code"].as_str().unwrap_or(""),
                    "{id} unrepaired code"
                );
                assert_eq!(
                    got.key,
                    want["key"].as_str().map(str::to_string),
                    "{id} unrepaired key"
                );
            }
            let second = agree_entry(
                if expected["name"].as_str().unwrap_or("").is_empty()
                    && case["entry"].is_null()
                {
                    None
                } else {
                    Some(expected["name"].as_str().unwrap_or(""))
                },
                open,
                &children,
                &made,
            );
            assert_eq!(second.name, outcome.name, "{id} idempotent name");
            assert!(
                second.added.is_empty() && second.removed.is_empty(),
                "{id} idempotent no rewrite"
            );
            assert_eq!(
                second.unrepaired, outcome.unrepaired,
                "{id} idempotent unrepaired"
            );
        }
    }

    #[test]
    fn shared_settle_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/pomodoro_themes/vectors.json"
        ))
        .expect("shared vectors JSON");
        let cases = vectors["settle"].as_array().expect("settle vectors");
        assert!(!cases.is_empty());
        for case in cases {
            let id = case["name"].as_str().unwrap_or("?");
            let entry = case["entry"].as_str().unwrap_or("");
            let worked: Vec<WorkedType> = case["worked"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| WorkedType {
                            key: normalize_component(
                                item["key"].as_str().unwrap_or(""),
                            ),
                            name: item["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let made = sources(
                &strings(&case["vocab"]),
                &strings(&case["exempt"]),
                &strings(&case["inventory"]),
            );
            assert_eq!(
                settle_closed(entry, &worked, &made),
                case["expected"].as_str().unwrap_or(""),
                "{id}"
            );
        }
    }
}
