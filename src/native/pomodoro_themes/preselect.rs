//! Shared menu preselection rule for task-aware Pomodoro choices.

use std::collections::BTreeSet;

use serde::Serialize;

use super::{suggest::ThemeSuggestion, ThemeRef};
use crate::native::plan_budget::{normalize_component, split_components};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MenuFallback {
    UnknownTheme,
    CreationBlocked,
    StrictCap,
    AlreadyQueued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuChoice {
    Entry(usize),
    Create,
    Fallback(MenuFallback),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MenuEntry<'a> {
    pub(crate) name: Option<&'a str>,
    pub(crate) open: bool,
    pub(crate) running: bool,
}

/// One menu-facing suggestion. The rule result remains at the top level;
/// menu state is additive and omitted when it does not apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MenuSuggestion {
    #[serde(flatten)]
    pub(crate) suggestion: ThemeSuggestion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fallback: Option<MenuFallback>,
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) create: bool,
}

impl MenuSuggestion {
    pub(crate) fn new(
        suggestion: ThemeSuggestion,
        choice: MenuChoice,
        include_create: bool,
    ) -> Self {
        let (fallback, create) = match choice {
            MenuChoice::Entry(_) => (None, false),
            MenuChoice::Create => (None, include_create),
            MenuChoice::Fallback(code) => (Some(code), false),
        };
        Self {
            suggestion,
            fallback,
            create,
        }
    }
}

/// Pick an existing open row before considering creation. Entries are
/// passed in ledger order; an exact running row wins, then the first exact,
/// then the first containing row.
pub(crate) fn choose(
    suggestion: &ThemeRef,
    entries: &[MenuEntry<'_>],
    exempt: &[String],
    ledger_keys: &[String],
    creation_allowed: bool,
    strict: bool,
    themes_before: usize,
    cap: u32,
    already_queued: bool,
) -> MenuChoice {
    if already_queued {
        return MenuChoice::Fallback(MenuFallback::AlreadyQueued);
    }

    let key = suggestion.key.as_str();
    let exempt: BTreeSet<String> = exempt
        .iter()
        .map(|value| normalize_component(value))
        .collect();
    let exact = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.open)
        .filter(|(_, entry)| {
            let Some(name) = entry.name else { return false };
            let components: BTreeSet<String> = split_components(name)
                .into_iter()
                .map(|component| normalize_component(&component))
                .filter(|component| {
                    component == key || !exempt.contains(component)
                })
                .collect();
            components.len() == 1 && components.contains(key)
        })
        .map(|(index, entry)| (index, entry.running))
        .collect::<Vec<_>>();

    if suggestion.known {
        if let Some((index, _)) = exact.iter().find(|(_, running)| *running) {
            return MenuChoice::Entry(*index);
        }
        if let Some((index, _)) = exact.first() {
            return MenuChoice::Entry(*index);
        }
        if let Some((index, _)) =
            entries.iter().enumerate().find(|(_, entry)| {
                entry.open
                    && entry.name.is_some_and(|name| {
                        split_components(name).iter().any(|component| {
                            normalize_component(component) == key
                        })
                    })
            })
        {
            return MenuChoice::Entry(index);
        }
    }

    if !suggestion.known {
        return MenuChoice::Fallback(MenuFallback::UnknownTheme);
    }
    if !creation_allowed {
        return MenuChoice::Fallback(MenuFallback::CreationBlocked);
    }
    let exempt_suggestion = exempt.contains(key);
    let already_in_ledger = ledger_keys
        .iter()
        .any(|name| normalize_component(name) == key);
    let adds_theme = !exempt_suggestion && !already_in_ledger;
    if strict && adds_theme && themes_before.saturating_add(1) > cap as usize {
        return MenuChoice::Fallback(MenuFallback::StrictCap);
    }
    MenuChoice::Create
}

#[cfg(test)]
fn fallback_code(fallback: MenuFallback) -> &'static str {
    match fallback {
        MenuFallback::UnknownTheme => "unknown_theme",
        MenuFallback::CreationBlocked => "creation_blocked",
        MenuFallback::StrictCap => "strict_cap",
        MenuFallback::AlreadyQueued => "already_queued",
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::pomodoro_themes::{HueSource, ThemeRef};
    use serde_json::Value;

    fn suggestion(known: bool) -> ThemeRef {
        ThemeRef {
            key: "build".to_string(),
            name: "BUILD".to_string(),
            hue: None,
            hue_source: HueSource::Hash,
            known,
            configured: None,
            start: None,
            end: None,
        }
    }

    #[test]
    fn exact_running_precedes_first_exact_then_containing() {
        let entries = [
            MenuEntry {
                name: Some("PLAN + BUILD"),
                open: true,
                running: false,
            },
            MenuEntry {
                name: Some("BUILD"),
                open: true,
                running: true,
            },
            MenuEntry {
                name: Some("BUILD + FIX"),
                open: true,
                running: false,
            },
        ];
        assert_eq!(
            choose(
                &suggestion(true),
                &entries,
                &["PLAN".into()],
                &[],
                true,
                false,
                0,
                5,
                false
            ),
            MenuChoice::Entry(1)
        );
    }

    #[test]
    fn unknown_and_cap_fallbacks_are_explicit() {
        assert_eq!(
            choose(&suggestion(false), &[], &[], &[], true, false, 0, 5, false),
            MenuChoice::Fallback(MenuFallback::UnknownTheme)
        );
        assert_eq!(
            choose(&suggestion(true), &[], &[], &[], true, true, 5, 5, false),
            MenuChoice::Fallback(MenuFallback::StrictCap)
        );
    }

    #[test]
    fn shared_preselect_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/pomodoro_themes/vectors.json"
        ))
        .expect("shared vectors JSON");
        for case in vectors["preselect"].as_array().expect("preselect vectors")
        {
            let raw = &case["suggestion"];
            let suggestion = ThemeRef {
                key: raw["key"].as_str().unwrap().to_string(),
                name: raw["name"].as_str().unwrap().to_string(),
                hue: None,
                hue_source: HueSource::Hash,
                known: raw["known"].as_bool().unwrap(),
                configured: None,
                start: None,
                end: None,
            };
            let entries = case["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| MenuEntry {
                    name: entry["name"].as_str(),
                    open: entry["open"].as_bool().unwrap(),
                    running: entry["running"].as_bool().unwrap(),
                })
                .collect::<Vec<_>>();
            let exempt = case["exempt"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            let ledger_keys = case["ledger_keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            let choice = choose(
                &suggestion,
                &entries,
                &exempt,
                &ledger_keys,
                case["creation_allowed"].as_bool().unwrap(),
                case["strict"].as_bool().unwrap(),
                case["themes_before"].as_u64().unwrap() as usize,
                case["cap"].as_u64().unwrap() as u32,
                case["already_queued"].as_bool().unwrap(),
            );
            let expected = &case["expected"];
            match expected["kind"].as_str().unwrap() {
                "entry" => assert_eq!(
                    choice,
                    MenuChoice::Entry(
                        expected["index"].as_u64().unwrap() as usize
                    ),
                    "{}",
                    case["id"]
                ),
                "create" => {
                    assert_eq!(choice, MenuChoice::Create, "{}", case["id"])
                }
                "fallback" => {
                    let MenuChoice::Fallback(fallback) = choice else {
                        panic!(
                            "{}: expected fallback, got {choice:?}",
                            case["id"]
                        );
                    };
                    assert_eq!(
                        fallback_code(fallback),
                        expected["code"].as_str().unwrap(),
                        "{}",
                        case["id"]
                    );
                }
                other => panic!("unexpected expected kind {other}"),
            }
        }
    }
}
