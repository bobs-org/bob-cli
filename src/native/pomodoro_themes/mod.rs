//! Derived Pomodoro theme catalog: scan, cache, journal fold, hue, and rank.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chrono::NaiveDate;
use serde::Serialize;

use crate::native::{
    config,
    plan_budget::{normalize_component, split_components},
    pomodoro_homes::Hue,
    style::Styler,
};

mod cache;
pub(crate) mod cli;
mod hints;
mod hue;
mod journal;
mod rank;
pub(crate) mod rename;
mod scan;
mod similar;
pub(crate) mod split;
#[cfg(test)]
mod tests;
pub(crate) mod write;

pub(crate) use cli::run_list;
pub(crate) use hints::CollisionHint;
pub(crate) use hue::resolve_hue;
pub(crate) use journal::Prefs;
pub(crate) use rank::{rank_history, RankTheme};
pub(crate) use rename::{build_cli_rename, renamed_reappeared, run_rename};
pub(crate) use split::{build_cli_split, run_split};
pub(crate) use write::{run_color, run_hide, run_unhide};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HueSource {
    Pin,
    Exempt,
    Area,
    Hash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TodayPresence {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ThemeRef {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) hue: Option<Hue>,
    pub(crate) hue_source: HueSource,
    #[serde(skip_serializing_if = "is_true")]
    pub(crate) known: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end: Option<usize>,
}

/// Render a terminal-only swatch before each known component. Callers keep
/// their existing text when color is disabled, so `NO_COLOR` stays byte-safe.
pub(crate) fn swatch_name(
    themes: &[ThemeRef],
    separator: &str,
    styler: &Styler,
) -> Option<String> {
    if !styler.is_color() || themes.is_empty() {
        return None;
    }
    Some(
        themes
            .iter()
            .map(|theme| {
                if !theme.known {
                    return theme.name.clone();
                }
                let swatch = match theme.hue {
                    Some(hue) => {
                        styler.paint(&format!("38;5;{}", hue.xterm()), "■")
                    }
                    None => styler.dim("□"),
                };
                format!("{swatch}{}", theme.name)
            })
            .collect::<Vec<_>>()
            .join(separator),
    )
}

fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub(crate) struct ThemeRecord {
    pub(crate) key: String,
    pub(crate) name: String,
    #[serde(with = "cache::date")]
    pub(crate) first_used: NaiveDate,
    #[serde(with = "cache::date")]
    pub(crate) last_used: NaiveDate,
    pub(crate) days_used: u32,
    pub(crate) entries: u32,
    pub(crate) today: Option<TodayPresence>,
}

#[derive(Debug, Clone)]
pub(crate) struct Catalog {
    pub(crate) today: NaiveDate,
    records: BTreeMap<String, ThemeRecord>,
    days: BTreeMap<NaiveDate, BTreeSet<String>>,
    prefs: Prefs,
    areas: BTreeMap<String, Hue>,
    exempt: BTreeSet<String>,
    warnings: Vec<String>,
}

impl Catalog {
    pub(crate) fn load(
        vault: &Path,
        today: NaiveDate,
        fresh: bool,
    ) -> std::io::Result<Self> {
        Self::load_inner(vault, today, fresh, None, None)
    }

    /// Completion only needs area hues for remembered themes and names in
    /// this draft/ledger. Avoid reading every unrelated root note per keypress.
    pub(crate) fn load_for_completion(
        vault: &Path,
        today: NaiveDate,
        extra_keys: BTreeSet<String>,
        live: Option<&crate::native::capture_pomodoros::PomodoroScan>,
    ) -> std::io::Result<Self> {
        Self::load_inner(vault, today, false, Some(extra_keys), live)
    }

    fn load_inner(
        vault: &Path,
        today: NaiveDate,
        fresh: bool,
        area_keys: Option<BTreeSet<String>>,
        live: Option<&crate::native::capture_pomodoros::PomodoroScan>,
    ) -> std::io::Result<Self> {
        let exempt = load_exempt();
        let (prefs, mut warnings) = journal::load_prefs(vault)?;
        let mut scan_warnings = Vec::new();
        let (records, days) = load_records(
            vault,
            today,
            fresh,
            area_keys.is_some(),
            live,
            &mut scan_warnings,
        );
        warnings.extend(scan_warnings);
        let areas = match area_keys {
            Some(mut keys) => {
                keys.extend(records.keys().cloned());
                scan::scan_root_areas_for_keys(vault, &keys)
            }
            None => scan::scan_root_areas(vault),
        };
        Ok(Self {
            today,
            records,
            days,
            prefs,
            areas,
            exempt,
            warnings,
        })
    }

    pub(crate) fn get(&self, key: &str) -> Option<&ThemeRecord> {
        self.records.get(key)
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = &ThemeRecord> {
        self.records.values()
    }

    pub(crate) fn known_count(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn hidden_count(&self) -> usize {
        self.prefs
            .by_key
            .values()
            .filter(|pref| pref.hidden)
            .count()
    }

    pub(crate) fn today_count(&self) -> usize {
        self.records
            .values()
            .filter(|record| record.today.is_some())
            .count()
    }

    pub(crate) fn is_hidden(&self, key: &str) -> bool {
        self.prefs.hidden(key)
    }

    pub(crate) fn is_exempt(&self, key: &str) -> bool {
        self.exempt.contains(key)
    }

    pub(crate) fn pin(&self, key: &str) -> Option<Hue> {
        self.prefs.pin(key)
    }

    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub(crate) fn theme_ref(&self, key: &str, name: &str) -> ThemeRef {
        let (hue, hue_source) =
            resolve_hue(key, self.prefs.pin(key), &self.exempt, &self.areas);
        ThemeRef {
            key: key.to_string(),
            name: name.to_string(),
            hue,
            hue_source,
            known: self.records.contains_key(key),
            start: None,
            end: None,
        }
    }

    pub(crate) fn theme_refs_for_name(&self, name: &str) -> Vec<ThemeRef> {
        split_components(name)
            .into_iter()
            .map(|component| {
                let key = normalize_component(&component);
                self.theme_ref(&key, &component)
            })
            .collect()
    }

    /// Build refs for an already-split list of theme names. The plan budget
    /// uses `(unnamed)` as a placeholder, which is deliberately not a theme.
    pub(crate) fn theme_refs_for_names(
        &self,
        names: &[String],
    ) -> Vec<ThemeRef> {
        names
            .iter()
            .filter(|name| name.as_str() != "(unnamed)")
            .flat_map(|name| self.theme_refs_for_name(name))
            .collect()
    }

    pub(crate) fn collision_hints(&self) -> Vec<CollisionHint> {
        hints::collision_hints(
            self.today,
            &self.records,
            &self.days,
            &self.prefs,
            &self.exempt,
            &self.areas,
        )
    }

    pub(crate) fn similar_to(&self, query: &str) -> Vec<String> {
        similar::similar_to(
            query,
            &self
                .records()
                .map(|record| similar::SimilarTheme {
                    key: &record.key,
                    name: &record.name,
                    last_used: record.last_used,
                    hidden: self.is_hidden(&record.key),
                })
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn rank_for_query(
        &self,
        query: &str,
        exclusions: &BTreeSet<String>,
    ) -> Vec<&ThemeRecord> {
        let ranked: Vec<RankTheme<'_>> = self
            .records
            .values()
            .map(|record| RankTheme {
                key: &record.key,
                name: &record.name,
                last_used: record.last_used,
                days_used: record.days_used,
            })
            .collect();
        rank_history(&ranked, query, exclusions)
            .into_iter()
            .map(|index| {
                self.records
                    .get(ranked[index].key)
                    .expect("ranked key is in catalog")
            })
            .collect()
    }
}

/// Small read-only view for capture-parse, which only needs hue and known
/// membership for the components present in one draft.
pub(crate) struct ThemeLookup {
    known_keys: BTreeSet<String>,
    prefs: Prefs,
    areas: BTreeMap<String, Hue>,
    exempt: BTreeSet<String>,
}

impl ThemeLookup {
    pub(crate) fn load_for_theme_keys(
        vault: &Path,
        today: NaiveDate,
        keys: &BTreeSet<String>,
    ) -> std::io::Result<Self> {
        let years = scan::year_dir_fingerprints(vault);
        if let Some(known_keys) = cache::load_known_keys(vault, today, &years) {
            let (prefs, _) = journal::load_prefs(vault)?;
            return Ok(Self {
                known_keys,
                prefs,
                areas: scan::scan_root_areas_for_keys(vault, keys),
                exempt: load_exempt(),
            });
        }

        let catalog = Catalog::load(vault, today, false)?;
        Ok(Self {
            known_keys: catalog.records.into_keys().collect(),
            prefs: catalog.prefs,
            areas: catalog.areas,
            exempt: catalog.exempt,
        })
    }

    pub(crate) fn theme_ref(&self, key: &str, name: &str) -> ThemeRef {
        let (hue, hue_source) =
            resolve_hue(key, self.prefs.pin(key), &self.exempt, &self.areas);
        ThemeRef {
            key: key.to_string(),
            name: name.to_string(),
            hue,
            hue_source,
            known: self.known_keys.contains(key),
            start: None,
            end: None,
        }
    }
}

fn load_exempt() -> BTreeSet<String> {
    config::load_plan_config(&config::config_path())
        .unwrap_or_else(|_| config::PlanConfig::default())
        .exempt()
        .iter()
        .map(|value| normalize_component(value))
        .collect()
}

fn load_records(
    vault: &Path,
    today: NaiveDate,
    fresh: bool,
    completion: bool,
    live: Option<&crate::native::capture_pomodoros::PomodoroScan>,
    warnings: &mut Vec<String>,
) -> (
    BTreeMap<String, ThemeRecord>,
    BTreeMap<NaiveDate, BTreeSet<String>>,
) {
    let years = scan::year_dir_fingerprints(vault);
    if completion
        && let Some((mut records, mut days, cached_warnings)) =
            cache::load_completion(vault, today, &years)
    {
        warnings.extend(cached_warnings);
        let live = read_today_summary(vault, today, live, warnings);
        merge_today(&mut records, &mut days, today, live.as_ref());
        return (records, days);
    }
    let key = cache::vault_key(vault);
    let mut cache_file = cache::load();
    let sweep = cache::needs_sweep(cache_file.vaults.get(&key), &years, fresh);
    let vault_cache = cache_file.vaults.entry(key.clone()).or_default();

    if sweep {
        let notes = scan::list_daily_notes(vault);
        let mut kept: BTreeMap<String, cache::CachedFile> = BTreeMap::new();
        for (date, relative, path) in notes {
            if date > today {
                continue;
            }
            if date == today {
                continue;
            }
            let Some((mtime_ns, size)) = cache::fingerprint(&path) else {
                continue;
            };
            if let Some(cached) = vault_cache.files.get(&relative)
                && cached.mtime_ns == mtime_ns
                && cached.size == size
            {
                kept.insert(relative, cached.clone());
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    kept.insert(
                        relative,
                        cache::CachedFile {
                            mtime_ns,
                            size,
                            summary: scan::FileSummary {
                                date: date.to_string(),
                                themes: scan::scan_contents(&contents),
                            },
                        },
                    );
                }
                Err(error) => {
                    warnings
                        .push(format!("could not read {relative}: {error}"));
                }
            }
        }
        vault_cache.files = kept;
        vault_cache.last_sweep_unix = cache::now_unix();
        vault_cache.years = years
            .iter()
            .map(|(year, (mtime_ns, size))| {
                (
                    year.clone(),
                    cache::YearFingerprint {
                        mtime_ns: *mtime_ns,
                        size: *size,
                    },
                )
            })
            .collect();
    }

    let today_relative = scan::canonical_relative(today);
    let today_path = vault.join(&today_relative);
    let history_warnings = warnings.clone();
    let today_summary = read_today_summary(vault, today, live, warnings);
    if let Some(summary) = today_summary.as_ref()
        && let Some((mtime_ns, size)) = cache::fingerprint(&today_path)
    {
        vault_cache.files.insert(
            today_relative,
            cache::CachedFile {
                mtime_ns,
                size,
                summary: summary.clone(),
            },
        );
    }

    let (records, days) =
        aggregate_records(vault_cache, today, today_summary.as_ref());
    cache::store_known_keys(
        vault,
        today,
        vault_cache.last_sweep_unix,
        &vault_cache.years,
        records.keys().cloned().collect(),
    );
    let (past_records, past_days) = aggregate_records(vault_cache, today, None);
    let history = cache::VaultCache {
        files: BTreeMap::new(),
        ..vault_cache.clone()
    };
    cache_file.version = 1;
    cache::store(&cache_file, Some(&key));
    cache::store_completion(
        vault,
        today,
        history,
        past_records,
        past_days,
        &history_warnings,
    );
    (records, days)
}

fn read_today_summary(
    vault: &Path,
    today: NaiveDate,
    live: Option<&crate::native::capture_pomodoros::PomodoroScan>,
    warnings: &mut Vec<String>,
) -> Option<scan::FileSummary> {
    if let Some(live) = live {
        return Some(scan::FileSummary {
            date: today.to_string(),
            themes: scan::themes_from_scan(live),
        });
    }
    let relative = scan::canonical_relative(today);
    match std::fs::read_to_string(vault.join(&relative)) {
        Ok(contents) => Some(scan::FileSummary {
            date: today.to_string(),
            themes: scan::scan_contents(&contents),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            warnings.push(format!("could not read {relative}: {error}"));
            None
        }
    }
}

fn aggregate_records(
    vault_cache: &cache::VaultCache,
    today: NaiveDate,
    today_live: Option<&scan::FileSummary>,
) -> (
    BTreeMap<String, ThemeRecord>,
    BTreeMap<NaiveDate, BTreeSet<String>>,
) {
    let mut records: BTreeMap<String, ThemeRecord> = BTreeMap::new();
    let mut days: BTreeMap<NaiveDate, BTreeSet<String>> = BTreeMap::new();
    for cached in vault_cache.files.values() {
        if cached.summary.themes.is_empty() {
            continue;
        }
        let Some(date) = scan::parse_summary_date(&cached.summary.date) else {
            continue;
        };
        if date >= today {
            continue;
        }
        merge_file(&mut records, &mut days, date, &cached.summary.themes);
    }
    merge_today(&mut records, &mut days, today, today_live);
    (records, days)
}

fn merge_today(
    records: &mut BTreeMap<String, ThemeRecord>,
    days: &mut BTreeMap<NaiveDate, BTreeSet<String>>,
    today: NaiveDate,
    today_live: Option<&scan::FileSummary>,
) {
    if let Some(today_live) = today_live {
        merge_file(records, days, today, &today_live.themes);
        for theme in &today_live.themes {
            if let Some(record) = records.get_mut(&theme.key) {
                record.today = Some(if theme.open {
                    TodayPresence::Open
                } else {
                    TodayPresence::Closed
                });
            }
        }
    }
}

fn merge_file(
    records: &mut BTreeMap<String, ThemeRecord>,
    days: &mut BTreeMap<NaiveDate, BTreeSet<String>>,
    date: NaiveDate,
    themes: &[scan::FileTheme],
) {
    let day_keys = days.entry(date).or_default();
    for theme in themes {
        day_keys.insert(theme.key.clone());
        match records.get_mut(&theme.key) {
            Some(record) => {
                if date < record.first_used {
                    record.first_used = date;
                }
                if date >= record.last_used {
                    record.last_used = date;
                    record.name = theme.name.clone();
                }
                record.days_used = record.days_used.saturating_add(1);
                record.entries = record.entries.saturating_add(theme.entries);
            }
            None => {
                records.insert(
                    theme.key.clone(),
                    ThemeRecord {
                        key: theme.key.clone(),
                        name: theme.name.clone(),
                        first_used: date,
                        last_used: date,
                        days_used: 1,
                        entries: theme.entries,
                        today: None,
                    },
                );
            }
        }
    }
}
