use std::collections::{BTreeMap, BTreeSet};

use chrono::{Duration, NaiveDate};
use serde::Serialize;

use crate::native::pomodoro_homes::Hue;

use super::{hue::resolve_hue, journal::Prefs, ThemeRecord};

pub(crate) const HINT_WINDOW_DAYS: i64 = 30;
pub(crate) const MIN_DAYS_TOGETHER: u32 = 2;

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

/// Unpinned, non-exempt pairs that share a hue and met on at least two of
/// the last 30 days. The theme with fewer `days_used` is the one to move.
pub(crate) fn collision_hints(
    today: NaiveDate,
    records: &BTreeMap<String, ThemeRecord>,
    days: &BTreeMap<NaiveDate, BTreeSet<String>>,
    prefs: &Prefs,
    exempt: &BTreeSet<String>,
    areas: &BTreeMap<String, Hue>,
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

    let mut candidates: Vec<(&ThemeRecord, Hue)> = Vec::new();
    for key in &window_keys {
        let Some(record) = records.get(key) else {
            continue;
        };
        if prefs.pin(key).is_some() || exempt.contains(key) {
            continue;
        }
        let (Some(hue), _) = resolve_hue(key, None, exempt, areas) else {
            continue;
        };
        candidates.push((record, hue));
    }

    let mut usage = BTreeMap::<Hue, u32>::new();
    for key in &window_keys {
        let pin = prefs.pin(key);
        let (Some(hue), _) = resolve_hue(key, pin, exempt, areas) else {
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
            let (Some(hue), _) = resolve_hue(other, pin, exempt, areas) else {
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
