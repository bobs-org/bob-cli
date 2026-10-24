use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use crate::native::{
    capture_pomodoros::{self, PomodoroState},
    is_always_excluded_note_directory_name,
    plan_budget::{normalize_component, split_components},
    pomodoro_homes::{fallback_hue, Hue},
    projects::{
        frontmatter_is_area, frontmatter_value, parse_frontmatter,
        trim_yaml_scalar, wikilink_path_target,
    },
    stats::index::canonical_daily_date,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FileTheme {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) entries: u32,
    pub(crate) open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FileSummary {
    pub(crate) date: String,
    pub(crate) themes: Vec<FileTheme>,
}

pub(crate) fn canonical_relative(date: NaiveDate) -> String {
    format!("{}/{}", date.format("%Y"), date.format("%Y%m%d.md"))
}

pub(crate) fn scan_contents(contents: &str) -> Vec<FileTheme> {
    themes_from_scan(&capture_pomodoros::scan(contents))
}

pub(crate) fn themes_from_scan(
    scan: &capture_pomodoros::PomodoroScan,
) -> Vec<FileTheme> {
    let mut by_key: BTreeMap<String, FileTheme> = BTreeMap::new();
    for entry in &scan.entries {
        let Some(name) = entry.name.as_deref() else {
            continue;
        };
        for component in split_components(name) {
            let key = normalize_component(&component);
            if key.is_empty() {
                continue;
            }
            let open = entry.state == PomodoroState::Open;
            let slot = by_key.entry(key.clone()).or_insert(FileTheme {
                key,
                name: component.clone(),
                entries: 0,
                open: false,
            });
            slot.name = component;
            slot.entries = slot.entries.saturating_add(1);
            slot.open |= open;
        }
    }
    by_key.into_values().collect()
}

pub(crate) fn list_daily_notes(
    vault: &Path,
) -> Vec<(NaiveDate, String, PathBuf)> {
    let entries = match fs::read_dir(vault) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut notes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_always_excluded_note_directory_name(std::ffi::OsStr::new(name)) {
            continue;
        }
        if name.len() != 4 || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let year_entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for file in year_entries.flatten() {
            let file_path = file.path();
            let Ok(file_type) = file.file_type() else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }
            let Ok(relative) = file_path.strip_prefix(vault) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            let Some(date) = canonical_daily_date(&relative) else {
                continue;
            };
            notes.push((date, relative, file_path));
        }
    }
    notes
        .sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    notes
}

pub(crate) fn year_dir_fingerprints(
    vault: &Path,
) -> BTreeMap<String, (u64, u64)> {
    let mut years = BTreeMap::new();
    let entries = match fs::read_dir(vault) {
        Ok(entries) => entries,
        Err(_) => return years,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.len() != 4 || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        if let Some(fingerprint) = super::cache::fingerprint(&path) {
            years.insert(name.to_string(), fingerprint);
        }
    }
    years
}

pub(crate) fn scan_root_areas(vault: &Path) -> BTreeMap<String, Hue> {
    scan_root_areas_matching(vault, None)
}

pub(crate) fn scan_root_areas_for_keys(
    vault: &Path,
    keys: &std::collections::BTreeSet<String>,
) -> BTreeMap<String, Hue> {
    scan_root_areas_matching(vault, Some(keys))
}

fn scan_root_areas_matching(
    vault: &Path,
    keys: Option<&std::collections::BTreeSet<String>>,
) -> BTreeMap<String, Hue> {
    let mut areas = BTreeMap::new();
    if keys.is_some_and(|keys| keys.is_empty()) {
        return areas;
    }
    let entries = match fs::read_dir(vault) {
        Ok(entries) => entries,
        Err(_) => return areas,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        let stem = name.trim_end_matches(".md");
        let key = normalize_component(stem);
        if keys.is_some_and(|keys| !keys.contains(&key)) {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(frontmatter) = parse_frontmatter(&contents) else {
            continue;
        };
        if !frontmatter_is_area(&frontmatter) {
            continue;
        }
        let Some(parent) = frontmatter_value(&frontmatter, "parent")
            .and_then(wikilink_path_target)
        else {
            continue;
        };
        let parent_stem = parent.strip_suffix(".md").unwrap_or(&parent);
        if !parent_stem.eq_ignore_ascii_case("area") {
            continue;
        }
        let hue = frontmatter_value(&frontmatter, "color")
            .and_then(|value| Hue::parse(trim_yaml_scalar(value)))
            .unwrap_or_else(|| fallback_hue(stem));
        areas.insert(key, hue);
    }
    areas
}

pub(crate) fn parse_summary_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
}
