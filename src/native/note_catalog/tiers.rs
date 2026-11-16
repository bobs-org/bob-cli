//! Shared path tiers for note pickers and vault-wide link completion.

use std::path::{Path, PathBuf};

use crate::native::{done_archive, highlights_ref::Config as HighlightsConfig};
use chrono::NaiveDate;

const HISTORY_DIRS: &[&str] = &["old_lib", "ref_history", "lit_review"];
const SATELLITE_SUFFIXES: &[&str] = &["habit", "done", "poms", "events"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteTier {
    Living,
    Archive,
    Hidden,
}

/// Path-only facts shared by all note-tier consumers in one vault.
pub(crate) struct TierContext {
    library_roots: Vec<PathBuf>,
}

impl TierContext {
    pub(crate) fn for_vault(bob_dir: &Path) -> Self {
        let highlights = HighlightsConfig::for_vault(bob_dir);
        Self::with_highlights(bob_dir, &highlights)
    }

    pub(super) fn with_highlights(
        bob_dir: &Path,
        highlights: &HighlightsConfig,
    ) -> Self {
        let vault_root = std::fs::canonicalize(bob_dir)
            .unwrap_or_else(|_| bob_dir.to_path_buf());
        let library_roots = [&highlights.lib_dir, &highlights.xlib_dir]
            .into_iter()
            .map(|path| relativize(&vault_root, path))
            .filter(|path| !path.as_os_str().is_empty())
            .collect();
        Self { library_roots }
    }

    pub(super) fn hidden_directory(&self, relative: &Path) -> bool {
        has_hidden_component(relative, true)
            || self
                .library_roots
                .iter()
                .any(|root| relative.starts_with(root))
    }
}

/// Classify a vault-relative Markdown or Bases path. The first matching
/// rule wins so hidden library and metadata paths stay hidden even when
/// they sit below an archive directory.
pub(crate) fn path_tier(
    bob_dir: &Path,
    relative: &Path,
    context: &TierContext,
) -> NoteTier {
    if has_hidden_component(relative, false)
        || context
            .library_roots
            .iter()
            .any(|root| relative.starts_with(root))
        || is_sibling_of_pdf(bob_dir, relative)
    {
        return NoteTier::Hidden;
    }
    if done_archive::is_archive_path(relative)
        || under_fixed_history(relative)
        || is_satellite(relative)
    {
        return NoteTier::Archive;
    }
    NoteTier::Living
}

fn has_hidden_component(
    relative: &Path,
    include_final_underscore: bool,
) -> bool {
    let components: Vec<_> = relative.components().collect();
    components.iter().enumerate().any(|(index, component)| {
        let name = component.as_os_str().to_string_lossy();
        if name.starts_with('.') {
            return true;
        }
        name.starts_with('_')
            && (include_final_underscore || index + 1 < components.len())
    })
}

fn under_fixed_history(relative: &Path) -> bool {
    relative.components().next().is_some_and(|component| {
        HISTORY_DIRS.iter().any(|directory| {
            component.as_os_str().to_string_lossy() == *directory
        })
    })
}

fn is_sibling_of_pdf(bob_dir: &Path, relative: &Path) -> bool {
    if !relative
        .extension()
        .is_some_and(|extension| extension == "md")
    {
        return false;
    }
    let Some(stem) = relative.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let pdf = relative
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{stem}.pdf"));
    bob_dir.join(pdf).is_file()
}

fn is_satellite(relative: &Path) -> bool {
    satellite_date_suffix(relative).is_some()
}

pub(super) fn satellite_date_suffix(
    relative: &Path,
) -> Option<(NaiveDate, String)> {
    let mut components = relative.components();
    let (Some(year), Some(file), None) =
        (components.next(), components.next(), components.next())
    else {
        return None;
    };
    let year_component = year.as_os_str().to_string_lossy();
    let file = file.as_os_str().to_string_lossy();
    if year_component.len() != 4
        || !year_component.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let stem = file.strip_suffix(".md")?;
    let (date, suffix) = stem.split_once('_')?;
    if date.len() != 8
        || !date.bytes().all(|byte| byte.is_ascii_digit())
        || !SATELLITE_SUFFIXES.contains(&suffix)
    {
        return None;
    }
    let parsed = date.parse::<u32>().ok()?;
    let date_year = (parsed / 10_000) as i32;
    let month = parsed / 100 % 100;
    let day = parsed % 100;
    if date_year.to_string() != year_component {
        return None;
    }
    Some((
        NaiveDate::from_ymd_opt(date_year, month, day)?,
        suffix.to_string(),
    ))
}

fn relativize(vault_root: &Path, absolute: &Path) -> PathBuf {
    absolute
        .strip_prefix(vault_root)
        .map_or_else(|_| absolute.to_path_buf(), Path::to_path_buf)
}
