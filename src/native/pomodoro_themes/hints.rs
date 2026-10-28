use std::collections::{BTreeMap, BTreeSet};

use chrono::{Duration, NaiveDate};
use serde::Serialize;

use crate::native::pomodoro_homes::Hue;

use super::{hue::resolve_hue, journal::Prefs, ThemeRecord};

pub(crate) const HINT_WINDOW_DAYS: i64 = 30;
pub(crate) const MIN_DAYS_TOGETHER: u32 = 2;
pub(crate) const NOT_CONFIGURED_HINT_DAYS: i64 = 7;
pub(crate) const NOT_CONFIGURED_HINT_HUMAN_CAP: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CollisionHint {
    pub(crate) code: &'static str,
    pub(crate) themes: Vec<String>,
    pub(crate) hue: Hue,
    pub(crate) days_together: u32,
    pub(crate) suggest: HintSuggest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HintSuggest {
    pub(crate) name: String,
    pub(crate) hue: Hue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HintTheme {
    pub(crate) key: String,
    pub(crate) name: String,
}

/// Visible unconfigured themes used today or in the last 7 days.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NotConfiguredHint {
    pub(crate) code: &'static str,
    pub(crate) theme: HintTheme,
    pub(crate) last_used: String,
    pub(crate) days_used: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) similar_to: Option<String>,
}

/// Additive list hints retain each existing JSON object shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum ThemeHint {
    Collision(CollisionHint),
    NotConfigured(NotConfiguredHint),
    UnknownDefault(UnknownDefaultHint),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct UnknownDefaultHint {
    pub(crate) code: &'static str,
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) similar_to: Vec<String>,
}

pub(crate) fn human_unknown_default_hint(hint: &UnknownDefaultHint) -> String {
    let suggestion = hint
        .similar_to
        .first()
        .map(|name| format!("; did you mean {name}?"))
        .unwrap_or_default();
    format!("{} suggests unknown theme {}{} → edit theme: in that note or add {} under themes: in {}",
        hint.path, hint.name, suggestion, hint.name, crate::native::config::display_config_path())
}

pub(crate) fn not_configured_hints(
    catalog: &super::Catalog,
) -> Vec<NotConfiguredHint> {
    if !catalog.has_configured() {
        return Vec::new();
    }
    let window_start = catalog
        .today
        .checked_sub_signed(Duration::days(NOT_CONFIGURED_HINT_DAYS - 1))
        .unwrap_or(NaiveDate::MIN);
    let mut hints: Vec<NotConfiguredHint> = catalog
        .records()
        .filter(|record| {
            !catalog.is_hidden(&record.key)
                && catalog.unconfigured(&record.key)
                && record.last_used >= window_start
                && record.last_used <= catalog.today
        })
        .map(|record| {
            let similar = catalog.similar_to(&record.name);
            NotConfiguredHint {
                code: super::THEME_NOT_CONFIGURED,
                theme: HintTheme {
                    key: record.key.clone(),
                    name: record.name.clone(),
                },
                last_used: record.last_used.to_string(),
                days_used: record.days_used,
                similar_to: similar.into_iter().next(),
            }
        })
        .collect();
    hints.sort_by(|left, right| {
        right
            .last_used
            .cmp(&left.last_used)
            .then_with(|| left.theme.name.cmp(&right.theme.name))
            .then_with(|| left.theme.key.cmp(&right.theme.key))
    });
    hints
}

pub(crate) fn human_not_configured_hint(hint: &NotConfiguredHint) -> String {
    super::theme_not_configured_list_hint(
        &hint.theme.name,
        hint.similar_to.as_deref(),
    )
}

/// Unpinned, non-exempt pairs that share a hue and met on at least two of
/// the last 30 days. The theme with fewer `days_used` is the one to move.
/// Config-colored and muted (exempt or unconfigured) themes never collide.
pub(crate) fn collision_hints(
    today: NaiveDate,
    records: &BTreeMap<String, ThemeRecord>,
    days: &BTreeMap<NaiveDate, BTreeSet<String>>,
    prefs: &Prefs,
    exempt: &BTreeSet<String>,
    areas: &BTreeMap<String, Hue>,
    themes: Option<&crate::native::config::ThemesConfig>,
) -> Vec<CollisionHint> {
    let window_start = today
        .checked_sub_signed(Duration::days(HINT_WINDOW_DAYS - 1))
        .unwrap_or(NaiveDate::MIN);
    let window_days: Vec<(&NaiveDate, &BTreeSet<String>)> = days
        .iter()
        .filter(|(date, _)| **date >= window_start && **date <= today)
        .collect();

    let mut window_keys = BTreeSet::new();
    for (_, keys) in &window_days {
        window_keys.extend(keys.iter().cloned());
    }

    let config_color = |key: &str| themes.and_then(|themes| themes.color(key));
    let unconfigured =
        |key: &str| themes.is_some_and(|themes| !themes.contains(key));
    let mut candidates: Vec<(&ThemeRecord, Hue)> = Vec::new();
    for key in &window_keys {
        let Some(record) = records.get(key) else {
            continue;
        };
        if prefs.pin(key).is_some()
            || exempt.contains(key)
            || config_color(key).is_some()
            || unconfigured(key)
        {
            continue;
        }
        let (Some(hue), _) =
            resolve_hue(key, None, exempt, areas, None, unconfigured(key))
        else {
            continue;
        };
        candidates.push((record, hue));
    }

    let mut usage = BTreeMap::<Hue, u32>::new();
    for key in &window_keys {
        let pin = prefs.pin(key);
        let (Some(hue), _) = resolve_hue(
            key,
            pin,
            exempt,
            areas,
            config_color(key),
            unconfigured(key),
        ) else {
            continue;
        };
        *usage.entry(hue).or_default() += 1;
    }

    let mut pairs: Vec<CollisionHint> = Vec::new();
    for left_index in 0..candidates.len() {
        for right_index in (left_index + 1)..candidates.len() {
            let (left, left_hue) = candidates[left_index];
            let (right, right_hue) = candidates[right_index];
            if left_hue != right_hue {
                continue;
            }
            let days_together = window_days
                .iter()
                .filter(|(_, keys)| {
                    keys.contains(&left.key) && keys.contains(&right.key)
                })
                .count() as u32;
            if days_together < MIN_DAYS_TOGETHER {
                continue;
            }
            let (kept, moved) = if right.days_used != left.days_used {
                if right.days_used < left.days_used {
                    (left, right)
                } else {
                    (right, left)
                }
            } else if right.name >= left.name {
                (left, right)
            } else {
                (right, left)
            };
            let Some(suggest_hue) = suggested_hue(
                moved,
                left_hue,
                &window_days,
                prefs,
                exempt,
                areas,
                &usage,
                themes,
            ) else {
                continue;
            };
            pairs.push(CollisionHint {
                code: "theme_hue_collision",
                themes: vec![kept.name.clone(), moved.name.clone()],
                hue: left_hue,
                days_together,
                suggest: HintSuggest {
                    name: moved.name.clone(),
                    hue: suggest_hue,
                },
            });
        }
    }

    pairs.sort_by(|left, right| {
        right
            .days_together
            .cmp(&left.days_together)
            .then_with(|| left.themes.cmp(&right.themes))
            .then_with(|| left.suggest.name.cmp(&right.suggest.name))
    });
    pairs
}

fn suggested_hue(
    moved: &ThemeRecord,
    current: Hue,
    window_days: &[(&NaiveDate, &BTreeSet<String>)],
    prefs: &Prefs,
    exempt: &BTreeSet<String>,
    areas: &BTreeMap<String, Hue>,
    usage: &BTreeMap<Hue, u32>,
    themes: Option<&crate::native::config::ThemesConfig>,
) -> Option<Hue> {
    let mut excluded = BTreeSet::new();
    excluded.insert(current);
    for (_, keys) in window_days {
        if !keys.contains(&moved.key) {
            continue;
        }
        for other in keys.iter() {
            if other == &moved.key {
                continue;
            }
            let pin = prefs.pin(other);
            let (Some(hue), _) = resolve_hue(
                other,
                pin,
                exempt,
                areas,
                themes.and_then(|themes| themes.color(other)),
                themes.is_some_and(|themes| !themes.contains(other)),
            ) else {
                continue;
            };
            excluded.insert(hue);
        }
    }

    let eligible: Vec<Hue> = Hue::ALL
        .into_iter()
        .filter(|hue| !excluded.contains(hue))
        .collect();
    let pool = if eligible.is_empty() {
        Hue::ALL.into_iter().filter(|hue| *hue != current).collect()
    } else {
        eligible
    };
    pool.into_iter().min_by_key(|hue| {
        (usage.get(hue).copied().unwrap_or(0), hue_index(*hue))
    })
}

fn hue_index(hue: Hue) -> usize {
    Hue::ALL
        .iter()
        .position(|slot| *slot == hue)
        .unwrap_or(Hue::ALL.len())
}

pub(crate) fn human_hint(hint: &CollisionHint) -> String {
    let left = hint.themes.first().map(String::as_str).unwrap_or("?");
    let right = hint.themes.get(1).map(String::as_str).unwrap_or("?");
    format!(
        "{left} and {right} share {} and met on {} days → bob themes color {} {}",
        hint.hue.name(),
        hint.days_together,
        hint.suggest.name,
        hint.suggest.hue.name(),
    )
}
