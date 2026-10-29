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
mod coverage;
mod defaults;
mod hints;
mod hue;
mod journal;
pub(crate) mod migrate;
pub(crate) mod preselect;
mod rank;
pub(crate) mod rename;
mod scan;
mod similar;
pub(crate) mod split;
pub(crate) mod suggest;
pub(crate) mod suggest_cli;
#[cfg(test)]
mod tests;
pub(crate) mod write;

pub(crate) use cli::run_list;
pub(crate) use hints::{CollisionHint, NotConfiguredHint};
pub(crate) use hue::resolve_hue;
pub(crate) use journal::Prefs;
pub(crate) use migrate::{build_cli_migrate, run_migrate};
pub(crate) use rank::{rank_history, RankTheme};
pub(crate) use rename::{build_cli_rename, renamed_reappeared, run_rename};
pub(crate) use split::{build_cli_split, run_split};
pub(crate) use write::{run_color, run_hide, run_unhide};

pub(crate) const THEME_NOT_CONFIGURED: &str = "theme_not_configured";
pub(crate) const THEMES_CONFIG_INVALID: &str = "themes_config_invalid";

/// `SNACK isn't one of your types`, plus `; did you mean
/// CHORE?` when PT7 similarity matches a configured theme.
pub(crate) fn theme_not_configured_message(
    name: &str,
    similar: &[String],
) -> String {
    match similar.first() {
        Some(suggestion) => format!(
            "{name} isn't one of your types; did you mean {suggestion}?"
        ),
        None => format!("{name} isn't one of your types"),
    }
}

/// List-hint rendering: add it under `themes:` or fold it into a
/// similar configured name.
pub(crate) fn theme_not_configured_list_hint(
    name: &str,
    similar: Option<&str>,
) -> String {
    let path = config::display_config_path();
    match similar {
        Some(suggestion) => format!(
            "{name} isn't one of your types → add it under themes: in {path}, or fold it in: bob types rename {name} {suggestion}"
        ),
        None => format!(
            "{name} isn't one of your types → add it under themes: in {path}"
        ),
    }
}

/// One Recent-row theme for PT7 menus: used history carries dates,
/// never-used configured themes omit them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuTheme {
    pub(crate) name: String,
    pub(crate) last_used: Option<NaiveDate>,
    pub(crate) days_used: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HueSource {
    Config,
    Pin,
    Exempt,
    Unconfigured,
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
    pub(crate) configured: Option<bool>,
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
                if theme.configured == Some(false) {
                    return format!("{}{}", styler.dim("◌"), theme.name);
                }
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
    inventory: BTreeSet<String>,
    themes: Option<config::ThemesConfig>,
    config_warning: Option<String>,
    warnings: Vec<String>,
}

impl Catalog {
    pub(crate) fn load(
        vault: &Path,
        today: NaiveDate,
        fresh: bool,
    ) -> std::io::Result<Self> {
        Self::load_inner(vault, today, fresh, None, None, None)
    }

    /// Completion only needs area hues for remembered themes and names in
    /// this draft/ledger. Avoid reading every unrelated root note per keypress.
    pub(crate) fn load_for_completion(
        vault: &Path,
        today: NaiveDate,
        extra_keys: BTreeSet<String>,
        live: Option<&crate::native::capture_pomodoros::PomodoroScan>,
    ) -> std::io::Result<Self> {
        Self::load_inner(vault, today, false, Some(extra_keys), live, None)
    }

    /// Explicit list only: its concurrent default scan supplies root-area
    /// facts before the catalog is used, so skip a duplicate area scan here.
    pub(super) fn load_without_areas(
        vault: &Path,
        today: NaiveDate,
        fresh: bool,
    ) -> std::io::Result<Self> {
        Self::load_inner(vault, today, fresh, None, None, Some(BTreeMap::new()))
    }

    fn load_inner(
        vault: &Path,
        today: NaiveDate,
        fresh: bool,
        area_keys: Option<BTreeSet<String>>,
        live: Option<&crate::native::capture_pomodoros::PomodoroScan>,
        cached_areas: Option<BTreeMap<String, Hue>>,
    ) -> std::io::Result<Self> {
        let sources = config::load_theme_sources();
        let (prefs, mut warnings) = journal::load_prefs(vault)?;
        if let Some(warning) = sources.warning.clone() {
            warnings.push(warning);
        }
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
            None => {
                cached_areas.unwrap_or_else(|| scan::scan_root_areas(vault))
            }
        };
        Ok(Self {
            today,
            records,
            days,
            prefs,
            areas,
            exempt: sources.exempt,
            inventory: sources.inventory,
            themes: sources.themes,
            config_warning: sources.warning,
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

    /// Warning from a present-but-invalid `themes:` block, if any.
    pub(crate) fn config_warning(&self) -> Option<&str> {
        self.config_warning.as_deref()
    }

    /// Hidden theme keys, including configured names that have never
    /// been used and so have no history record.
    pub(crate) fn hidden_keys(&self) -> impl Iterator<Item = &str> {
        self.prefs
            .by_key
            .iter()
            .filter(|(_, pref)| pref.hidden)
            .map(|(key, _)| key.as_str())
    }

    /// `None` when no `themes:` vocabulary is configured, else whether the
    /// key belongs to it. Inventory labels always report `Some(true)`.
    pub(crate) fn configured(&self, key: &str) -> Option<bool> {
        if self.inventory.contains(key) {
            return self.themes.as_ref().map(|_| true);
        }
        self.themes.as_ref().map(|themes| themes.contains(key))
    }

    pub(crate) fn config_color(&self, key: &str) -> Option<Hue> {
        self.themes.as_ref().and_then(|themes| themes.color(key))
    }

    pub(crate) fn configured_themes(
        &self,
    ) -> Option<&[config::ConfiguredTheme]> {
        self.themes.as_ref().map(|themes| themes.entries.as_slice())
    }

    pub(crate) fn has_configured(&self) -> bool {
        self.themes.is_some()
    }

    pub(crate) fn unconfigured(&self, key: &str) -> bool {
        match &self.themes {
            None => false,
            Some(themes) => {
                !themes.contains(key) && !self.inventory.contains(key)
            }
        }
    }

    pub(crate) fn display_name(&self, key: &str, canonical: &str) -> String {
        if let Some(record) = self.records.get(key) {
            return record.name.clone();
        }
        if let Some(themes) = &self.themes
            && let Some(entry) =
                themes.entries.iter().find(|entry| entry.key == key)
        {
            return entry.name.clone();
        }
        canonical.to_string()
    }

    #[cfg(test)]
    pub(crate) fn for_suggest(
        known: &[(String, String)],
        configured: &[(String, String)],
        live: &[String],
        areas: BTreeMap<String, Hue>,
        hidden: &[String],
        exempt: &[String],
    ) -> Self {
        let today = NaiveDate::from_ymd_opt(2026, 10, 28).expect("valid date");
        let mut records = BTreeMap::new();
        for (key, name) in known {
            records.insert(
                key.clone(),
                ThemeRecord {
                    key: key.clone(),
                    name: name.clone(),
                    first_used: today,
                    last_used: today,
                    days_used: 1,
                    entries: 1,
                    today: None,
                },
            );
        }
        for key in live {
            records.entry(key.clone()).or_insert_with(|| ThemeRecord {
                key: key.clone(),
                name: key.to_ascii_uppercase(),
                first_used: today,
                last_used: today,
                days_used: 1,
                entries: 1,
                today: Some(TodayPresence::Open),
            });
        }
        let mut prefs = Prefs::default();
        for key in hidden {
            prefs.by_key.insert(
                key.clone(),
                journal::KeyPref {
                    hidden: true,
                    pin: None,
                },
            );
        }
        let themes = if configured.is_empty() {
            None
        } else {
            Some(config::ThemesConfig {
                entries: configured
                    .iter()
                    .map(|(key, name)| config::ConfiguredTheme {
                        key: key.clone(),
                        name: name.clone(),
                        color: None,
                    })
                    .collect(),
            })
        };
        Self {
            today,
            records,
            days: BTreeMap::new(),
            prefs,
            areas,
            exempt: exempt.iter().cloned().collect(),
            inventory: BTreeSet::new(),
            themes,
            config_warning: None,
            warnings: Vec::new(),
        }
    }

    pub(crate) fn theme_ref(&self, key: &str, name: &str) -> ThemeRef {
        let configured = self.configured(key);
        let (hue, hue_source) = resolve_hue(
            key,
            self.prefs.pin(key),
            &self.exempt,
            &self.areas,
            self.config_color(key),
            self.unconfigured(key),
        );
        ThemeRef {
            key: key.to_string(),
            name: name.to_string(),
            hue,
            hue_source,
            known: self.records.contains_key(key) || configured == Some(true),
            configured,
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
            self.themes.as_ref(),
        )
    }

    pub(crate) fn similar_to(&self, query: &str) -> Vec<String> {
        if let Some(themes) = &self.themes {
            let never_used = NaiveDate::MIN;
            let candidates: Vec<similar::SimilarTheme<'_>> = themes
                .entries
                .iter()
                .filter(|entry| !self.is_hidden(&entry.key))
                .map(|entry| match self.records.get(&entry.key) {
                    Some(record) => similar::SimilarTheme {
                        key: record.key.as_str(),
                        name: record.name.as_str(),
                        last_used: record.last_used,
                        hidden: false,
                    },
                    None => similar::SimilarTheme {
                        key: entry.key.as_str(),
                        name: entry.name.as_str(),
                        last_used: never_used,
                        hidden: false,
                    },
                })
                .collect();
            return similar::similar_to(query, &candidates);
        }
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

    /// Recent-row ranking for theme-name menus. With a `themes:` block,
    /// only configured keys participate: used ones in PT7 recency order,
    /// then never-used configured names in config order.
    pub(crate) fn rank_for_menu(
        &self,
        query: &str,
        exclusions: &BTreeSet<String>,
    ) -> Vec<MenuTheme> {
        let Some(themes) = &self.themes else {
            return self
                .rank_for_query(query, exclusions)
                .into_iter()
                .map(|record| MenuTheme {
                    name: record.name.clone(),
                    last_used: Some(record.last_used),
                    days_used: Some(record.days_used),
                })
                .collect();
        };
        let configured_keys: BTreeSet<&str> = themes
            .entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect();
        let ranked: Vec<RankTheme<'_>> = self
            .records
            .values()
            .filter(|record| configured_keys.contains(record.key.as_str()))
            .map(|record| RankTheme {
                key: &record.key,
                name: &record.name,
                last_used: record.last_used,
                days_used: record.days_used,
            })
            .collect();
        let mut rows: Vec<MenuTheme> = rank_history(&ranked, query, exclusions)
            .into_iter()
            .map(|index| {
                let theme = &ranked[index];
                MenuTheme {
                    name: theme.name.to_string(),
                    last_used: Some(theme.last_used),
                    days_used: Some(theme.days_used),
                }
            })
            .collect();
        let used: BTreeSet<&str> =
            ranked.iter().map(|theme| theme.key).collect();
        for entry in &themes.entries {
            if exclusions.contains(&entry.key)
                || self.is_hidden(&entry.key)
                || used.contains(entry.key.as_str())
            {
                continue;
            }
            rows.push(MenuTheme {
                name: entry.name.clone(),
                last_used: None,
                days_used: None,
            });
        }
        rows
    }

    /// Visible unconfigured themes used today or in the last 7 days.
    pub(crate) fn not_configured_hints(&self) -> Vec<NotConfiguredHint> {
        hints::not_configured_hints(self)
    }
}

/// Small read-only view for capture-parse, which only needs hue and known
/// membership for the components present in one draft.
pub(crate) struct ThemeLookup {
    known_keys: BTreeSet<String>,
    prefs: Prefs,
    areas: BTreeMap<String, Hue>,
    exempt: BTreeSet<String>,
    inventory: BTreeSet<String>,
    themes: Option<config::ThemesConfig>,
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
            let sources = config::load_theme_sources();
            return Ok(Self {
                known_keys,
                prefs,
                areas: scan::scan_root_areas_for_keys(vault, keys),
                exempt: sources.exempt,
                inventory: sources.inventory,
                themes: sources.themes,
            });
        }

        let catalog = Catalog::load(vault, today, false)?;
        Ok(Self {
            known_keys: catalog.records.into_keys().collect(),
            prefs: catalog.prefs,
            areas: catalog.areas,
            exempt: catalog.exempt,
            inventory: catalog.inventory,
            themes: catalog.themes,
        })
    }

    fn unconfigured(&self, key: &str) -> bool {
        match &self.themes {
            None => false,
            Some(themes) => {
                !themes.contains(key) && !self.inventory.contains(key)
            }
        }
    }

    fn config_color(&self, key: &str) -> Option<Hue> {
        self.themes.as_ref().and_then(|themes| themes.color(key))
    }

    fn configured(&self, key: &str) -> Option<bool> {
        if self.inventory.contains(key) {
            return self.themes.as_ref().map(|_| true);
        }
        self.themes.as_ref().map(|themes| themes.contains(key))
    }

    pub(crate) fn theme_ref(&self, key: &str, name: &str) -> ThemeRef {
        let configured = self.configured(key);
        let (hue, hue_source) = resolve_hue(
            key,
            self.prefs.pin(key),
            &self.exempt,
            &self.areas,
            self.config_color(key),
            self.unconfigured(key),
        );
        ThemeRef {
            key: key.to_string(),
            name: name.to_string(),
            hue,
            hue_source,
            known: self.known_keys.contains(key) || configured == Some(true),
            configured,
            start: None,
            end: None,
        }
    }
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
