//! Read-only note catalog for Go To Notes (`//`).
//!
//! Phase `cli-note-catalog`: walks the vault once with frontmatter-only
//! reads (first 8 KiB per file), prunes the hidden tiers, classifies kinds,
//! and produces ranked rows plus the empty-state map, with golden tests on
//! a synthetic tier vault. No grammar and no CLI surface; phase
//! `cli-note-goto` consumes [`scan`] and [`rank`] verbatim.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

use chrono::{Datelike, NaiveDate};
use serde::Serialize;

use crate::native::{
    albums::registry::AlbumRegistry,
    areas::graph::build_area_graph,
    bank::month::filename_month_for,
    capture_link_tasks::match_score,
    config, done_archive, env as bob_env,
    highlights_ref::Config as HighlightsConfig,
    notebooks::snapshot::{display_slash, VaultSnapshot},
    periods::{calendar::PeriodId, labels::range_label},
    pomodoro::{day_file_for, parse_day_file_date},
    pomodoro_homes::{HomeResolver, HomeSource, Hue, NoteFacts},
    projects::{
        frontmatter_is_album, frontmatter_is_area, frontmatter_is_notebook,
        frontmatter_is_project, frontmatter_is_record,
        frontmatter_top_level_value, frontmatter_type_names, parse_frontmatter,
        project_is_finished, trim_yaml_scalar, Frontmatter, ProjectStatus,
    },
    vault_links::{LinkResolution, NoteIndex},
};

mod tiers;
pub(crate) use tiers::{path_tier, NoteTier, TierContext};

#[cfg(test)]
mod tests;

/// First bytes read per file. Frontmatter parsing stops at the closing
/// `---`, so one capped read covers every row field without ever reading
/// a body, headings, blocks, or tasks.
pub(crate) const PREFIX_CAP: usize = 8192;

/// Upper bound on catalog warnings. Unreadable and non-UTF-8 files share
/// the usual warnings array with completion, so the list stays bounded
/// and deduplicated.
pub(crate) const MAX_WARNINGS: usize = 50;

/// Vault-relative inbox stems. `mac_inbox` is the pinned default;
/// `gmail_inbox` and `gkeep_inbox` are the sync intake defaults and
/// `inbox` the bare inbox route.
const INBOX_STEMS: &[&str] =
    &["mac_inbox", "inbox", "gkeep_inbox", "gmail_inbox"];

/// A scanned vault: rows in canonical order plus bounded warnings.
/// `warnings` names unreadable or non-UTF-8 files and never fails
/// the scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NoteCatalog {
    pub vault_root: PathBuf,
    pub rows: Vec<NoteRow>,
    pub warnings: Vec<String>,
}

/// One Go To Notes row. Field names and shapes match the
/// `note_goto` contract; `cli-note-goto` adds only `replacement`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NoteRow {
    pub note_path: String,
    pub kind: NoteKindKey,
    pub title: String,
    pub match_keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hue: Option<Hue>,
    pub rank: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<NoteGroup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u8>,
}

/// Row kind, serialized snake_case per the `note_goto` contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NoteKindKey {
    Inbox,
    Area,
    Project,
    Notebook,
    Album,
    Record,
    Ref,
    Period,
    Daily,
    Archive,
    Satellite,
    Bank,
    Mail,
    Note,
    View,
}

/// Empty-state map group, serialized snake_case per contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NoteGroup {
    Today,
    Inbox,
    Areas,
    Projects,
    Notebooks,
    Albums,
    Done,
}

impl NoteKindKey {
    /// Snake-case kind key used in JSON and the text renderer.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Area => "area",
            Self::Project => "project",
            Self::Notebook => "notebook",
            Self::Album => "album",
            Self::Record => "record",
            Self::Ref => "ref",
            Self::Period => "period",
            Self::Daily => "daily",
            Self::Archive => "archive",
            Self::Satellite => "satellite",
            Self::Bank => "bank",
            Self::Mail => "mail",
            Self::Note => "note",
            Self::View => "view",
        }
    }

    /// Tier rank: lower sorts first at equal match score.
    fn rank(self, closed_project: bool) -> u8 {
        match self {
            Self::Inbox => 0,
            Self::Area => 1,
            Self::Project if !closed_project => 2,
            Self::Project => 5,
            Self::Notebook | Self::Album => 3,
            Self::Record => 5,
            Self::Ref => 7,
            Self::Period => 6,
            Self::Daily => 8,
            Self::Archive => 4,
            Self::Satellite => 8,
            Self::Bank | Self::Mail | Self::Note | Self::View => 4,
        }
    }
}

/// Walk the vault once and classify every living note.
///
/// Frontmatter-only reads (first [`PREFIX_CAP`] bytes per file), never
/// writes, never creates a file, and never touches `.obsidian`.
/// Unreadable or non-UTF-8 files become bounded warnings.
pub(crate) fn scan(bob_dir: &Path) -> NoteCatalog {
    let vault_root =
        fs::canonicalize(bob_dir).unwrap_or_else(|_| bob_dir.to_path_buf());
    // Path collection reads no contents: hidden tiers that the path
    // alone decides never cost a file read.
    let mut collect = Collect::new(bob_dir);
    collect.run();
    let highlights = HighlightsConfig::for_vault(bob_dir);
    let tiers = TierContext::for_vault(bob_dir);
    let context =
        ClassifyContext::new(&vault_root, &highlights, &gmail_folder());

    // Read, parse, and classify every candidate in parallel; chunks
    // hold contiguous path ranges so the merged result stays sorted.
    // One parse per file: owned facts feed classification and every
    // row field, so no prefix is ever parsed twice.
    let (mut entries, mut warnings) =
        read_classify_parallel(bob_dir, &collect.paths, &context, &tiers);
    warnings.extend(collect.warnings);
    // Legacy record nouns come from album declarations, so resolve
    // them after the first pass and re-check the leftovers.
    let record_ids = album_record_ids(&entries);
    for entry in entries.iter_mut() {
        if entry.kind == ClassifiedKind::Note
            && is_legacy_record_types(&entry.facts, &record_ids)
        {
            entry.kind = ClassifiedKind::Record;
        }
    }

    // The shared snapshot only ever sees graph-relevant buffers
    // (areas, projects, notebooks, albums), never the whole vault.
    let mut graph_buffers = BTreeMap::new();
    let mut discovered = BTreeSet::new();
    for entry in &entries {
        discovered.insert(entry.relative.clone());
        if matches!(
            entry.kind,
            ClassifiedKind::Area
                | ClassifiedKind::Project { .. }
                | ClassifiedKind::Notebook
                | ClassifiedKind::Album
        ) {
            graph_buffers.insert(entry.relative.clone(), entry.prefix.clone());
        }
    }
    let snapshot = VaultSnapshot::from_prefixes(discovered, graph_buffers);

    // One indexed hue/breadcrumb pass over the prefixes already read;
    // no note is read twice. Row workers each hold a resolver over
    // shared maps, so chains resolve in parallel without locking.
    let source = PrefixHomes::new(&entries);
    let mut rows = build_rows_parallel(&entries, &source);

    apply_map_groups(&mut rows, bob_dir, &snapshot);

    warnings.sort();
    warnings.dedup();
    warnings.truncate(MAX_WARNINGS);
    NoteCatalog {
        vault_root,
        rows,
        warnings,
    }
}

/// Walk the archive scope and build its read-only note catalog. Only
/// top-level `done/`, fixed history folders, and year directories can
/// contain rows in this scope; hidden directories and symlinks are
/// pruned before file reads.
pub(crate) fn scan_archive(bob_dir: &Path) -> NoteCatalog {
    let vault_root =
        fs::canonicalize(bob_dir).unwrap_or_else(|_| bob_dir.to_path_buf());
    let tiers = TierContext::for_vault(bob_dir);
    let mut collect = ArchiveCollect::new(bob_dir, &tiers);
    collect.run();
    let (rows, mut warnings) =
        read_archive_rows_parallel(bob_dir, &collect.paths);
    warnings.extend(collect.warnings);
    warnings.sort();
    warnings.dedup();
    warnings.truncate(MAX_WARNINGS);
    NoteCatalog {
        vault_root,
        rows: canonical_archive_order(rows),
        warnings,
    }
}

struct ArchiveCollect<'a> {
    bob_dir: &'a Path,
    tiers: &'a TierContext,
    paths: BTreeSet<PathBuf>,
    warnings: Vec<String>,
}

impl<'a> ArchiveCollect<'a> {
    fn new(bob_dir: &'a Path, tiers: &'a TierContext) -> Self {
        Self {
            bob_dir,
            tiers,
            paths: BTreeSet::new(),
            warnings: Vec::new(),
        }
    }

    fn run(&mut self) {
        let entries = match fs::read_dir(self.bob_dir) {
            Ok(entries) => entries,
            Err(error) => {
                self.warn(format!(".: failed to read directory: {error}"));
                return;
            }
        };
        let mut entries = match entries.collect::<Result<Vec<_>, _>>() {
            Ok(entries) => entries,
            Err(error) => {
                self.warn(format!(
                    ".: failed to read directory entries: {error}"
                ));
                return;
            }
        };
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let relative = PathBuf::from(entry.file_name());
            let file_type = match entry.path().symlink_metadata() {
                Ok(metadata) => metadata.file_type(),
                Err(error) => {
                    self.warn(format!(
                        "{}: failed to inspect path: {error}",
                        display_slash(&relative)
                    ));
                    continue;
                }
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let name = relative.file_name().and_then(|name| name.to_str());
            let archive_root = done_archive::skip_archive_directory(
                self.bob_dir,
                &self.bob_dir.join(&relative),
            );
            let history_root = name.is_some_and(|name| {
                matches!(name, "old_lib" | "ref_history" | "lit_review")
            });
            let satellite_year = name.is_some_and(is_year_directory);
            if !(archive_root || history_root || satellite_year)
                || self.tiers.hidden_directory(&relative)
            {
                continue;
            }
            self.visit_dir(&relative);
        }
    }

    fn visit_dir(&mut self, relative: &Path) {
        let entries = match fs::read_dir(self.bob_dir.join(relative)) {
            Ok(entries) => entries,
            Err(error) => {
                self.warn(format!(
                    "{}: failed to read directory: {error}",
                    display_dir(relative)
                ));
                return;
            }
        };
        let mut entries = match entries.collect::<Result<Vec<_>, _>>() {
            Ok(entries) => entries,
            Err(error) => {
                self.warn(format!(
                    "{}: failed to read directory entries: {error}",
                    display_dir(relative)
                ));
                return;
            }
        };
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let child = relative.join(entry.file_name());
            let file_type = match entry.path().symlink_metadata() {
                Ok(metadata) => metadata.file_type(),
                Err(error) => {
                    self.warn(format!(
                        "{}: failed to inspect path: {error}",
                        display_slash(&child)
                    ));
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if !self.tiers.hidden_directory(&child) {
                    self.visit_dir(&child);
                }
                continue;
            }
            if !file_type.is_file()
                || child.extension().is_none_or(|extension| extension != "md")
                || path_tier(self.bob_dir, &child, self.tiers)
                    != NoteTier::Archive
            {
                continue;
            }
            self.paths.insert(child);
        }
    }

    fn warn(&mut self, warning: String) {
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }
}

fn is_year_directory(name: &str) -> bool {
    name.len() == 4 && name.bytes().all(|byte| byte.is_ascii_digit())
}

fn read_archive_rows_parallel(
    bob_dir: &Path,
    paths: &BTreeSet<PathBuf>,
) -> (Vec<NoteRow>, Vec<String>) {
    if paths.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let paths: Vec<_> = paths.iter().cloned().collect();
    let threads = read_thread_count(paths.len());
    let chunk = paths.len().div_ceil(threads);
    let mut rows = Vec::with_capacity(paths.len());
    let mut warnings = Vec::new();
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for owned in paths.chunks(chunk) {
            let bob_dir = bob_dir.to_path_buf();
            let owned = owned.to_vec();
            handles.push(scope.spawn(move || {
                let mut rows = Vec::with_capacity(owned.len());
                let mut warnings = Vec::new();
                for relative in owned {
                    match read_prefix_full(&bob_dir.join(&relative)) {
                        Ok(prefix) => {
                            rows.push(build_archive_row(&relative, &prefix))
                        }
                        Err(error) => warnings.push(format!(
                            "{}: {error}",
                            display_slash(&relative)
                        )),
                    }
                }
                (rows, warnings)
            }));
        }
        for handle in handles {
            let (mut chunk_rows, mut chunk_warnings) =
                handle.join().expect("archive catalog reader");
            rows.append(&mut chunk_rows);
            warnings.append(&mut chunk_warnings);
        }
    });
    (rows, warnings)
}

fn build_archive_row(relative: &Path, prefix: &str) -> NoteRow {
    if let Some((date, suffix)) = tiers::satellite_date_suffix(relative) {
        let stem = file_stem(relative);
        let title = format!("{} {suffix}", date.format("%Y-%m-%d"));
        return NoteRow {
            note_path: display_slash(relative),
            kind: NoteKindKey::Satellite,
            title: title.clone(),
            match_keys: vec![title, stem],
            detail: Some(date.format("%A").to_string()),
            status: None,
            hue: None,
            rank: 8,
            group: None,
            depth: None,
        };
    }

    let facts = Facts::parse(prefix);
    let stem = file_stem(relative);
    let title = facts
        .title
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| stem.clone());
    let mut match_keys = vec![title.clone()];
    if stem != title
        && !match_keys.iter().any(|key| key.eq_ignore_ascii_case(&stem))
    {
        match_keys.push(stem);
    }
    for alias in facts.aliases {
        if !match_keys
            .iter()
            .any(|key| key.eq_ignore_ascii_case(&alias))
        {
            match_keys.push(alias);
        }
    }
    let frontmatter = parse_frontmatter(prefix);
    let parent = frontmatter
        .as_ref()
        .and_then(|frontmatter| {
            frontmatter_top_level_value(frontmatter, "parent")
        })
        .and_then(parent_link_label);
    let detail = parent.or_else(|| {
        relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(display_slash)
    });
    NoteRow {
        note_path: display_slash(relative),
        kind: NoteKindKey::Archive,
        title,
        match_keys,
        detail,
        status: None,
        hue: None,
        rank: 4,
        group: done_archive::is_archive_path(relative)
            .then_some(NoteGroup::Done),
        depth: None,
    }
}

fn parent_link_label(raw: &str) -> Option<String> {
    let value = trim_yaml_scalar(raw);
    let inner = value.strip_prefix("[[")?.strip_suffix("]]")?;
    let (target, alias) = inner
        .split_once('|')
        .map_or((inner, None), |(target, alias)| (target, Some(alias)));
    if let Some(alias) = alias.map(str::trim).filter(|alias| !alias.is_empty())
    {
        return Some(alias.to_string());
    }
    let target = target.split_once('#').map_or(target, |(target, _)| target);
    target
        .trim()
        .rsplit('/')
        .next()
        .filter(|label| !label.is_empty())
        .map(str::to_string)
}

fn canonical_archive_order(mut rows: Vec<NoteRow>) -> Vec<NoteRow> {
    rows.sort_by(|left, right| {
        let category = |row: &NoteRow| match row.kind {
            NoteKindKey::Satellite => 2,
            NoteKindKey::Archive if row.group == Some(NoteGroup::Done) => 0,
            _ => 1,
        };
        let left_category = category(left);
        let right_category = category(right);
        left_category.cmp(&right_category).then_with(|| {
            if left_category == 2 {
                right.note_path.cmp(&left.note_path)
            } else {
                left.title
                    .to_lowercase()
                    .cmp(&right.title.to_lowercase())
                    .then_with(|| left.note_path.cmp(&right.note_path))
            }
        })
    });
    rows
}

/// Rank rows for a non-empty query: primary `match_keys` matches first
/// by score descending, then `note_path` (minus extension) matches by
/// score descending. Ties keep canonical position. An empty or
/// whitespace-only query returns the canonical snapshot. Scoring uses
/// the shared [`match_score`], which ANDs across whitespace terms.
pub(crate) fn rank<'a>(
    catalog: &'a NoteCatalog,
    query: &str,
) -> Vec<&'a NoteRow> {
    if query.split_whitespace().next().is_none() {
        return catalog.rows.iter().collect();
    }
    let mut primary: Vec<(u32, &NoteRow)> = Vec::new();
    let mut secondary: Vec<(u32, &NoteRow)> = Vec::new();
    for row in &catalog.rows {
        if let Some(score) = match_score(&row.match_keys, query) {
            primary.push((score, row));
            continue;
        }
        let stem_path = strip_extension(&row.note_path);
        if let Some(score) = match_score(&[stem_path.to_string()], query) {
            secondary.push((score, row));
        }
    }
    // Stable sorts: ties keep the canonical position from `scan`.
    primary.sort_by_key(|row| std::cmp::Reverse(row.0));
    secondary.sort_by_key(|row| std::cmp::Reverse(row.0));
    primary
        .into_iter()
        .chain(secondary)
        .map(|(_, row)| row)
        .collect()
}

fn strip_extension(path: &str) -> &str {
    match path.rfind('.') {
        Some(index) => &path[..index],
        None => path,
    }
}

/// One recursive vault walk collecting candidate paths. The walk
/// reads no contents: every entry is metadata only, so pruned
/// directories and skipped extensions never cost a file read.
struct Collect<'a> {
    bob_dir: &'a Path,
    /// Every discovered `.md`/`.base` path, readable or not, sorted.
    paths: Vec<PathBuf>,
    warnings: Vec<String>,
}

impl<'a> Collect<'a> {
    fn new(bob_dir: &'a Path) -> Self {
        Self {
            bob_dir,
            paths: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn run(&mut self) {
        // The vault root fans out across worker threads, one per
        // top-level directory; each thread merges an ordered set, so
        // the merged walk stays sorted and deterministic.
        let mut paths = BTreeSet::new();
        let mut warnings = Vec::new();
        let mut dirs = Vec::new();
        let mut root_files = Vec::new();
        self.visit_top(&mut dirs, &mut root_files, &mut warnings);
        for path in root_files {
            paths.insert(path);
        }
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(dirs.len());
            for dir in dirs {
                let bob_dir = self.bob_dir.to_path_buf();
                handles.push(scope.spawn(move || {
                    let mut collector = Collect::new(&bob_dir);
                    let mut sub = BTreeSet::new();
                    collector.visit_dir(&dir, &mut sub);
                    (sub, collector.warnings)
                }));
            }
            for handle in handles {
                let (sub, mut sub_warnings) =
                    handle.join().expect("catalog walk");
                paths.extend(sub);
                warnings.append(&mut sub_warnings);
            }
        });
        self.warnings = warnings;
        self.paths = paths.into_iter().collect();
    }

    /// The vault root level, always walked inline without
    /// descending: top-level directories fan out to worker threads.
    fn visit_top(
        &mut self,
        dirs: &mut Vec<PathBuf>,
        files: &mut Vec<PathBuf>,
        warnings: &mut Vec<String>,
    ) {
        let entries = match fs::read_dir(self.bob_dir) {
            Ok(entries) => entries,
            Err(error) => {
                warnings.push(format!(".: failed to read directory: {error}"));
                return;
            }
        };
        let mut entries = match entries.collect::<Result<Vec<_>, _>>() {
            Ok(entries) => entries,
            Err(error) => {
                warnings.push(format!(
                    ".: failed to read directory entries: {error}"
                ));
                return;
            }
        };
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let file_type = match entry.path().symlink_metadata() {
                Ok(metadata) => metadata.file_type(),
                Err(error) => {
                    warnings.push(format!(
                        "{}: failed to inspect path: {error}",
                        display_slash(Path::new(&entry.file_name()))
                    ));
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child = Path::new("").join(entry.file_name());
            if file_type.is_dir() {
                if name.starts_with('.') || name.starts_with('_') {
                    continue;
                }
                if done_archive::skip_archive_directory(
                    self.bob_dir,
                    &self.bob_dir.join(&child),
                ) {
                    continue;
                }
                dirs.push(child);
                continue;
            }
            if !file_type.is_file() || name.starts_with('.') {
                continue;
            }
            let is_markdown =
                child.extension().is_some_and(|extension| extension == "md");
            let is_base = child
                .extension()
                .is_some_and(|extension| extension == "base");
            if is_markdown || is_base {
                files.push(child);
            }
        }
    }

    fn visit_dir(&mut self, relative: &Path, paths: &mut BTreeSet<PathBuf>) {
        let absolute = self.bob_dir.join(relative);
        let entries = match fs::read_dir(&absolute) {
            Ok(entries) => entries,
            Err(error) => {
                self.warn(format!(
                    "{}: failed to read directory: {error}",
                    display_dir(relative)
                ));
                return;
            }
        };
        let mut entries = match entries.collect::<Result<Vec<_>, _>>() {
            Ok(entries) => entries,
            Err(error) => {
                self.warn(format!(
                    "{}: failed to read directory entries: {error}",
                    display_dir(relative)
                ));
                return;
            }
        };
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            // Never follow symlinks: symlinked directories stay
            // unentered and symlinked files stay unlisted, matching the
            // existing note walkers.
            let file_type = match entry.path().symlink_metadata() {
                Ok(metadata) => metadata.file_type(),
                Err(error) => {
                    self.warn(format!(
                        "{}: failed to inspect path: {error}",
                        display_slash(&relative.join(entry.file_name()))
                    ));
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child = relative.join(entry.file_name());
            if file_type.is_dir() {
                // H1: dot-prefixed and `_`-prefixed directories are
                // pruned at any depth, as is the top-level `done/`
                // archive (H2).
                if name.starts_with('.') || name.starts_with('_') {
                    continue;
                }
                if done_archive::skip_archive_directory(
                    self.bob_dir,
                    &self.bob_dir.join(&child),
                ) {
                    continue;
                }
                self.visit_dir(&child, paths);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            // H1: dot-prefixed file names are hidden at any depth. A
            // file literally named `.md` is hidden here as well.
            if name.starts_with('.') {
                continue;
            }
            let is_markdown =
                child.extension().is_some_and(|extension| extension == "md");
            let is_base = child
                .extension()
                .is_some_and(|extension| extension == "base");
            if !is_markdown && !is_base {
                continue;
            }
            paths.insert(child);
        }
    }

    fn warn(&mut self, warning: String) {
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }
}

fn display_dir(relative: &Path) -> String {
    if relative.as_os_str().is_empty() {
        ".".to_string()
    } else {
        display_slash(relative)
    }
}

/// Fast-path prefix cap: most notes close their frontmatter inside
/// the first 2 KiB, so one short read serves them. Project state
/// scans the whole [`PREFIX_CAP`] window, so project candidates
/// extend to the full cap.
const FAST_PREFIX_CAP: usize = 2048;

/// One short read: up to [`FAST_PREFIX_CAP`] bytes as UTF-8 plus
/// whether the file ended there. The caller classifies from the
/// frontmatter inside this window and never reads the body. A
/// multibyte character split by the cap truncates to the character
/// boundary instead of warning.
fn read_prefix_fast(absolute: &Path) -> Result<(String, bool), String> {
    use std::io::Read as _;
    let mut file = fs::File::open(absolute)
        .map_err(|error| format!("failed to read file: {error}"))?;
    let mut bytes = vec![0u8; FAST_PREFIX_CAP];
    let read = file
        .read(&mut bytes)
        .map_err(|error| format!("failed to read file: {error}"))?;
    bytes.truncate(read);
    let prefix = truncate_to_valid_prefix(bytes)?;
    Ok((prefix, read < FAST_PREFIX_CAP))
}

/// The full [`PREFIX_CAP`] window for project candidates whose fast
/// path was inconclusive.
fn read_prefix_full(absolute: &Path) -> Result<String, String> {
    use std::io::Read as _;
    let file = fs::File::open(absolute)
        .map_err(|error| format!("failed to read file: {error}"))?;
    let mut bytes = Vec::new();
    file.take(PREFIX_CAP as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read file: {error}"))?;
    truncate_to_valid_prefix(bytes)
}

/// UTF-8 prefix that tolerates a read cap splitting the final
/// character: an incomplete trailing sequence truncates, while a
/// genuinely invalid sequence stays a warning.
fn truncate_to_valid_prefix(mut bytes: Vec<u8>) -> Result<String, String> {
    match String::from_utf8(bytes) {
        Ok(prefix) => Ok(prefix),
        Err(error) => {
            if error.utf8_error().error_len().is_none() {
                let valid_up_to = error.utf8_error().valid_up_to();
                bytes = error.into_bytes();
                bytes.truncate(valid_up_to);
                String::from_utf8(bytes)
                    .map_err(|_| "file is not valid UTF-8".to_string())
            } else {
                Err("file is not valid UTF-8".to_string())
            }
        }
    }
}

/// Whether a project candidate needs the full window: an open
/// status with no task verdict yet and more bytes unread.
fn project_needs_full_window(
    kind: Option<ClassifiedKind>,
    facts: &Facts,
    at_eof: bool,
) -> bool {
    matches!(kind, Some(ClassifiedKind::Project { closed: false }))
        && !facts.status_is_terminal()
        && !at_eof
}

/// Owned row inputs from one frontmatter parse. The scan parses
/// each prefix once; classification and every row field read these
/// facts instead of re-parsing.
#[derive(Debug, Clone, Default)]
struct Facts {
    title: Option<String>,
    aliases: Vec<String>,
    summary: Option<String>,
    status: Option<String>,
    type_names: Vec<String>,
    is_area: bool,
    is_project: bool,
    is_notebook: bool,
    is_album: bool,
    is_record: bool,
    is_ref_type: bool,
    is_day_type: bool,
}

impl Facts {
    fn parse(prefix: &str) -> Self {
        let Some(frontmatter) = parse_frontmatter(prefix) else {
            return Self::default();
        };
        let scalar = |key: &str| {
            frontmatter_top_level_value(&frontmatter, key)
                .map(|raw| trim_yaml_scalar(raw).trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let type_names = frontmatter_type_names(&frontmatter);
        Self {
            title: scalar("title"),
            aliases: frontmatter_aliases(&frontmatter),
            summary: scalar("summary"),
            status: scalar("status"),
            type_names: type_names.clone(),
            is_area: frontmatter_is_area(&frontmatter),
            is_project: frontmatter_is_project(&frontmatter),
            is_notebook: frontmatter_is_notebook(&frontmatter),
            is_album: frontmatter_is_album(&frontmatter),
            is_record: frontmatter_is_record(&frontmatter),
            is_ref_type: type_names.iter().any(|name| name == "ref"),
            is_day_type: type_names.iter().any(|name| name == "day"),
        }
    }

    fn status_is_terminal(&self) -> bool {
        ProjectStatus::parse(self.status.as_deref()).is_terminal()
    }
}

/// Worker count for catalog scans, bounded by the job size.
fn read_thread_count(jobs: usize) -> usize {
    std::thread::available_parallelism()
        .map(|threads| threads.get().max(1))
        .unwrap_or(4)
        .min(jobs)
}

/// Read, parse, and classify every candidate prefix across worker
/// threads, in path order. Path-only hidden tiers, unreadable files,
/// and non-UTF-8 files never become entries; the last two become
/// warnings. Chunks hold contiguous path ranges so the merged result
/// stays sorted without re-sorting.
fn read_classify_parallel(
    bob_dir: &Path,
    candidates: &[PathBuf],
    context: &ClassifyContext,
    tiers: &TierContext,
) -> (Vec<Classified>, Vec<String>) {
    if candidates.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // `ClassifyContext` borrows nothing, so chunks share one reference.
    let threads = read_thread_count(candidates.len());
    let chunk = candidates.len().div_ceil(threads);
    let mut entries: Vec<Classified> = Vec::with_capacity(candidates.len());
    let mut warnings = Vec::new();
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for chunk_paths in candidates.chunks(chunk) {
            let bob_dir = bob_dir.to_path_buf();
            let owned: Vec<PathBuf> = chunk_paths.to_vec();
            handles.push(scope.spawn(move || {
                let mut entries = Vec::with_capacity(owned.len());
                let mut warnings = Vec::new();
                for relative in owned {
                    if path_tier(&bob_dir, &relative, tiers) != NoteTier::Living
                    {
                        continue;
                    }
                    let absolute = bob_dir.join(&relative);
                    let (mut prefix, at_eof) = match read_prefix_fast(&absolute)
                    {
                        Ok(fast) => fast,
                        Err(error) => {
                            warnings.push(format!(
                                "{}: {error}",
                                display_slash(&relative)
                            ));
                            continue;
                        }
                    };
                    let mut facts = Facts::parse(&prefix);
                    let mut kind =
                        classify(&relative, &facts, &prefix, context);
                    if project_needs_full_window(kind, &facts, at_eof) {
                        // The `^prj` verdict may sit past the fast
                        // path; only project candidates pay for the
                        // full window.
                        match read_prefix_full(&absolute) {
                            Ok(full) => {
                                prefix = full;
                                facts = Facts::parse(&prefix);
                                kind = classify(
                                    &relative, &facts, &prefix, context,
                                );
                            }
                            Err(error) => {
                                warnings.push(format!(
                                    "{}: {error}",
                                    display_slash(&relative)
                                ));
                                continue;
                            }
                        }
                    }
                    if let Some(kind) = kind {
                        let home_facts = NoteFacts::parse(&prefix);
                        entries.push(Classified {
                            relative,
                            prefix,
                            facts,
                            kind,
                            home_facts,
                        });
                    }
                }
                (entries, warnings)
            }));
        }
        for handle in handles {
            let (mut chunk_entries, mut chunk_warnings) =
                handle.join().expect("catalog reader");
            entries.append(&mut chunk_entries);
            warnings.append(&mut chunk_warnings);
        }
    });
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    (entries, warnings)
}

/// Paths a candidate is hidden under, resolved once per scan.
struct ClassifyContext {
    ref_dir: PathBuf,
    gmail_folder: PathBuf,
}

impl ClassifyContext {
    fn new(
        vault_root: &Path,
        highlights: &HighlightsConfig,
        gmail_folder: &str,
    ) -> Self {
        Self {
            ref_dir: relativize(vault_root, &highlights.ref_dir),
            gmail_folder: PathBuf::from(gmail_folder),
        }
    }
}

fn relativize(vault_root: &Path, absolute: &Path) -> PathBuf {
    absolute
        .strip_prefix(vault_root)
        .map_or_else(|_| absolute.to_path_buf(), Path::to_path_buf)
}

fn gmail_folder() -> String {
    config::load_gmail_config(&config::config_path())
        .ok()
        .and_then(|settings| settings.folder)
        .filter(|folder| !folder.trim().is_empty())
        .unwrap_or_else(|| "mail".to_string())
}

/// A classified living note awaiting row fields.
struct Classified {
    relative: PathBuf,
    prefix: String,
    facts: Facts,
    kind: ClassifiedKind,
    /// Home facts parsed up front on the worker thread, so the hue
    /// pass only does map lookups.
    home_facts: NoteFacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClassifiedKind {
    Daily,
    Inbox,
    Area,
    Project { closed: bool },
    Notebook,
    Album,
    Record,
    Ref,
    Period,
    Bank,
    Mail,
    Note,
    View,
}

/// First-match-wins kind classification in §5.2 order, from one
/// parse's owned facts. Path tiers are decided before any read in
/// [`path_tier`]; `.base` views list per the `bases`
/// decision. Legacy record nouns resolve in a second pass (see
/// [`album_record_ids`]).
fn classify(
    relative: &Path,
    facts: &Facts,
    prefix: &str,
    context: &ClassifyContext,
) -> Option<ClassifiedKind> {
    let is_base = relative
        .extension()
        .is_some_and(|extension| extension == "base");
    if is_base {
        return Some(ClassifiedKind::View);
    }
    let stem = file_stem(relative);
    // Kinds, first match wins.
    if is_daily(relative) {
        return Some(ClassifiedKind::Daily);
    }
    if is_inbox_file(relative) {
        return Some(ClassifiedKind::Inbox);
    }
    if facts.is_area {
        return Some(ClassifiedKind::Area);
    }
    if facts.is_project {
        let closed = project_closed(relative, facts, prefix);
        return Some(ClassifiedKind::Project { closed });
    }
    if facts.is_notebook {
        return Some(ClassifiedKind::Notebook);
    }
    if facts.is_album {
        return Some(ClassifiedKind::Album);
    }
    if facts.is_record {
        return Some(ClassifiedKind::Record);
    }
    if facts.is_ref_type || under_dir(relative, &context.ref_dir) {
        return Some(ClassifiedKind::Ref);
    }
    if is_period_note(&stem, facts.is_day_type) {
        return Some(ClassifiedKind::Period);
    }
    if is_bank_note(relative) {
        return Some(ClassifiedKind::Bank);
    }
    if under_dir(relative, &context.gmail_folder) {
        return Some(ClassifiedKind::Mail);
    }
    Some(ClassifiedKind::Note)
}

fn is_legacy_record_types(
    facts: &Facts,
    record_ids: &BTreeSet<String>,
) -> bool {
    facts
        .type_names
        .iter()
        .any(|name| record_ids.contains(name))
}

/// Nouns claimed by album declarations, for the legacy record
/// second pass.
fn album_record_ids(entries: &[Classified]) -> BTreeSet<String> {
    let mut record_ids = BTreeSet::new();
    for entry in entries {
        if entry.kind != ClassifiedKind::Album {
            continue;
        }
        if let Some(declaration) =
            crate::native::albums::parse_album_declaration(
                &entry.relative,
                &entry.prefix,
            )
            && let Some(noun) = declaration.noun_claim
        {
            record_ids.insert(noun);
        }
    }
    record_ids
}

fn under_dir(relative: &Path, dir: &Path) -> bool {
    !dir.as_os_str().is_empty() && relative.starts_with(dir)
}

/// `YYYY/YYYYMMDD.md` under a top-level four-digit directory with a
/// valid calendar date.
fn is_daily(relative: &Path) -> bool {
    daily_date(relative).is_some()
}

fn daily_date(relative: &Path) -> Option<NaiveDate> {
    let mut components = relative.components();
    let (Some(first), Some(second), None) =
        (components.next(), components.next(), components.next())
    else {
        return None;
    };
    let (Some(year), Some(file)) =
        (first.as_os_str().to_str(), second.as_os_str().to_str())
    else {
        return None;
    };
    if year.len() != 4 || !year.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let stem = file.strip_suffix(".md")?;
    if stem.len() != 8 || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let month: u32 = stem[4..6].parse().ok()?;
    let day: u32 = stem[6..8].parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn is_inbox_file(relative: &Path) -> bool {
    relative
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| INBOX_STEMS.contains(&stem))
}

/// A bob period note (week, quarter, year, all time), or any other
/// note whose frontmatter type is `[[day]]` (legacy monthly and zorg
/// notes).
fn is_period_note(stem: &str, is_day_type: bool) -> bool {
    PeriodId::parse(stem).is_ok() || is_day_type
}

/// The bank hub (`bank.md` at the vault root) or a bank month note
/// (`bank/YYYY-MM.md`), named by the bank scan's own month parser.
fn is_bank_note(relative: &Path) -> bool {
    if relative == Path::new("bank.md") {
        return true;
    }
    relative.parent() == Some(Path::new("bank"))
        && filename_month_for(relative).is_some()
}

fn file_stem(relative: &Path) -> String {
    relative
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

impl ClassifiedKind {
    fn key(self) -> NoteKindKey {
        match self {
            Self::Daily => NoteKindKey::Daily,
            Self::Inbox => NoteKindKey::Inbox,
            Self::Area => NoteKindKey::Area,
            Self::Project { .. } => NoteKindKey::Project,
            Self::Notebook => NoteKindKey::Notebook,
            Self::Album => NoteKindKey::Album,
            Self::Record => NoteKindKey::Record,
            Self::Ref => NoteKindKey::Ref,
            Self::Period => NoteKindKey::Period,
            Self::Bank => NoteKindKey::Bank,
            Self::Mail => NoteKindKey::Mail,
            Self::Note => NoteKindKey::Note,
            Self::View => NoteKindKey::View,
        }
    }
}

/// In-memory [`HomeSource`] over the facts the workers already
/// parsed, so hue and breadcrumb never re-read a note. Backed by
/// reference-counted maps so row workers each hold a cheap clone.
#[derive(Clone)]
struct PrefixHomes {
    index: std::sync::Arc<NoteIndex>,
    facts: std::sync::Arc<HashMap<PathBuf, NoteFacts>>,
}

impl PrefixHomes {
    fn new(entries: &[Classified]) -> Self {
        let mut facts = HashMap::new();
        let mut paths = Vec::with_capacity(entries.len());
        for entry in entries {
            paths.push(entry.relative.clone());
            facts.insert(entry.relative.clone(), entry.home_facts.clone());
        }
        Self {
            index: std::sync::Arc::new(NoteIndex::from_paths(paths)),
            facts: std::sync::Arc::new(facts),
        }
    }
}

impl HomeSource for PrefixHomes {
    fn resolve(&self, current: &Path, target: &str) -> LinkResolution {
        self.index.resolve_detailed(Some(current), target)
    }

    fn read_facts(&self, path: &Path) -> Option<NoteFacts> {
        self.facts.get(path).cloned()
    }
}

/// Row fields in path order across worker threads, each with its
/// own resolver over the shared home maps.
fn build_rows_parallel(
    entries: &[Classified],
    source: &PrefixHomes,
) -> Vec<NoteRow> {
    if entries.is_empty() {
        return Vec::new();
    }
    let threads = read_thread_count(entries.len());
    let chunk = entries.len().div_ceil(threads);
    let mut rows = Vec::with_capacity(entries.len());
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for chunk_entries in entries.chunks(chunk) {
            let source = source.clone();
            handles.push(scope.spawn(move || {
                let mut resolver = HomeResolver::new(source);
                chunk_entries
                    .iter()
                    .map(|entry| build_row(entry, &mut resolver))
                    .collect::<Vec<_>>()
            }));
        }
        for handle in handles {
            rows.extend(handle.join().expect("catalog rows"));
        }
    });
    rows
}

/// Titles, keys, detail, status, and hue per §5.3.
fn build_row(
    entry: &Classified,
    resolver: &mut HomeResolver<PrefixHomes>,
) -> NoteRow {
    let stem = file_stem(&entry.relative);
    let facts = &entry.facts;

    let title = match entry.kind {
        ClassifiedKind::Daily => daily_date(&entry.relative)
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| stem.clone()),
        ClassifiedKind::View => stem.clone(),
        _ => facts
            .title
            .clone()
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| stem.clone()),
    };

    let match_keys = match entry.kind {
        ClassifiedKind::Daily => {
            let iso = title.clone();
            let mut keys = vec![iso];
            if stem != keys[0] {
                keys.push(stem.clone());
            }
            keys
        }
        _ => {
            let mut keys = vec![title.clone()];
            if stem != title
                && !keys.iter().any(|key| key.eq_ignore_ascii_case(&stem))
            {
                keys.push(stem.clone());
            }
            for alias in &facts.aliases {
                if !keys.iter().any(|key| key.eq_ignore_ascii_case(alias)) {
                    keys.push(alias.clone());
                }
            }
            keys
        }
    };

    let home = resolver.resolve_note(&entry.relative);
    let hue = home.hue;
    let breadcrumb = breadcrumb_for(&home.residence_chain);

    let detail = match entry.kind {
        ClassifiedKind::Daily => daily_date(&entry.relative)
            .map(|date| date.format("%A").to_string()),
        ClassifiedKind::Period => period_span(&stem).or(breadcrumb.clone()),
        ClassifiedKind::Notebook => {
            facts.summary.clone().or(breadcrumb.clone())
        }
        _ => breadcrumb.clone(),
    }
    .filter(|detail| !detail.is_empty());

    let status = match entry.kind {
        ClassifiedKind::Project { closed } => Some(project_status_label(
            facts.status.as_deref(),
            &entry.prefix,
            closed,
        )),
        ClassifiedKind::Ref => facts
            .status
            .clone()
            .filter(|status| !status.is_empty())
            .map(|status| capitalize(&status)),
        _ => None,
    };

    let closed = matches!(entry.kind, ClassifiedKind::Project { closed: true });
    NoteRow {
        note_path: display_slash(&entry.relative),
        rank: entry.kind.key().rank(closed),
        kind: entry.kind.key(),
        title,
        match_keys,
        detail,
        status,
        hue,
        group: None,
        depth: None,
    }
}

/// Frontmatter `aliases:` as a list, a single string, or a flow list,
/// trimmed, non-empty, and in file order.
fn frontmatter_aliases(frontmatter: &Frontmatter<'_>) -> Vec<String> {
    let Some(index) = frontmatter.lines.iter().position(|line| {
        !line.starts_with([' ', '\t'])
            && line
                .strip_prefix("aliases")
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some()
    }) else {
        return Vec::new();
    };
    let raw = frontmatter.lines[index]
        .strip_prefix("aliases")
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or("")
        .trim();
    if raw.starts_with('[') {
        let inner = raw
            .strip_prefix('[')
            .and_then(|rest| rest.rfind(']').map(|end| &rest[..end]))
            .unwrap_or("");
        return inner
            .split(',')
            .map(|item| trim_yaml_scalar(item).trim().to_string())
            .filter(|alias| !alias.is_empty())
            .collect();
    }
    if !raw.is_empty() {
        let alias = trim_yaml_scalar(raw).trim().to_string();
        return (!alias.is_empty()).then_some(alias).into_iter().collect();
    }
    let mut aliases = Vec::new();
    for line in &frontmatter.lines[index + 1..] {
        if line.starts_with([' ', '\t']) {
            let trimmed = line.trim();
            let Some(item) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("-\t"))
                .or_else(|| (trimmed == "-").then_some(""))
            else {
                // An indented non-item line ends the block list.
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                break;
            };
            let alias = trim_yaml_scalar(item).trim().to_string();
            if !alias.is_empty() {
                aliases.push(alias);
            }
            continue;
        }
        break;
    }
    aliases
}

/// The ancestor chain from the home walk, excluding the note itself:
/// root first, at most the nearest three ancestors, never the `area`
/// type root, labelled by stem and joined with `›`.
fn breadcrumb_for(residence_chain: &[String]) -> Option<String> {
    if residence_chain.len() < 2 {
        return None;
    }
    let mut ancestors: Vec<&str> = residence_chain[1..]
        .iter()
        .rev()
        .map(String::as_str)
        .filter(|ancestor| {
            Path::new(ancestor)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_none_or(|stem| !stem.eq_ignore_ascii_case("area"))
        })
        .collect();
    // `residence_chain` is note-first; after reversing, the tail holds
    // the nearest ancestors. Keep at most three, root first.
    if ancestors.len() > 3 {
        ancestors = ancestors[ancestors.len() - 3..].to_vec();
    }
    let labels: Vec<String> = ancestors
        .iter()
        .map(|ancestor| {
            Path::new(ancestor)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(ancestor)
                .to_string()
        })
        .collect();
    (!labels.is_empty()).then(|| labels.join("›"))
}

/// A period's date span from the existing label helpers when bob
/// recognizes the note, with the year appended for same-year spans
/// (for example `Nov 9 – 15, 2026`).
fn period_span(stem: &str) -> Option<String> {
    let id = PeriodId::parse(stem).ok()?;
    let (start, end) = match (id.start(), id.end()) {
        (Some(start), Some(end)) => (start, end),
        _ => return None,
    };
    let label = range_label(start, end);
    if start.year() == end.year() {
        Some(format!("{label}, {}", start.year()))
    } else {
        Some(label)
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => {
            first.to_uppercase().collect::<String>() + chars.as_str()
        }
        None => String::new(),
    }
}

/// The label `bob projects` prints for the project's `^prj` line:
/// WIP for open work, Done/Canceled when finished. Blocked derivation
/// needs the vault-wide dependency projection, so an open project with
/// open prerequisites still reads WIP here.
fn project_status_label(
    status: Option<&str>,
    prefix: &str,
    closed: bool,
) -> String {
    let parsed = ProjectStatus::parse(status);
    if parsed.is_terminal() {
        return capitalize(parsed.label());
    }
    if closed {
        return prj_mark_label(prefix).unwrap_or_else(|| "Done".to_string());
    }
    match parsed {
        ProjectStatus::Wip => "WIP".to_string(),
        ProjectStatus::Waiting => "Waiting".to_string(),
        ProjectStatus::Other(other) => capitalize(&other.to_lowercase()),
        _ => "WIP".to_string(),
    }
}

/// Whether a project is closed: a terminal frontmatter status, or a
/// closed `^prj` task. The common open case fast-paths on one line
/// scan: with no closed-mark `^prj` line anywhere in the window, the
/// full task parse cannot finish the project either (its Done/Canceled
/// verdicts come only from such a line), so the expensive
/// [`project_is_finished`] runs only when a closed mark exists.
fn project_closed(relative: &Path, facts: &Facts, prefix: &str) -> bool {
    if facts.status_is_terminal() {
        return true;
    }
    if prj_mark_label(prefix).is_none() {
        return false;
    }
    project_is_finished(relative, prefix)
}

/// Done/Canceled from the first closed-mark `^prj` task line, if any.
/// Bullets (`-`, `*`, `+`) and marks (`x`/`X` Done, `-` Canceled)
/// mirror the task parser's own mapping.
fn prj_mark_label(prefix: &str) -> Option<String> {
    for line in prefix.lines() {
        if !line.contains("^prj") {
            continue;
        }
        let trimmed = line.trim_start();
        let Some(after_bullet) = trimmed
            .strip_prefix('-')
            .or_else(|| trimmed.strip_prefix('*'))
            .or_else(|| trimmed.strip_prefix('+'))
        else {
            continue;
        };
        // The task parser takes one whitespace run after the bullet.
        let content = after_bullet.trim_start_matches([' ', '\t']);
        if content.len() == after_bullet.len() {
            continue;
        }
        let Some(rest) = content.strip_prefix('[') else {
            continue;
        };
        let label = rest
            .strip_prefix('x')
            .or_else(|| rest.strip_prefix('X'))
            .map(|_| "Done")
            .or_else(|| rest.strip_prefix('-').map(|_| "Canceled"));
        if label.is_some() {
            return label.map(str::to_string);
        }
    }
    None
}

/// Empty-state map groups (§5.4) plus canonical order (§5.5).
///
/// A row appears once: map rows carry `group` (and `depth` for areas)
/// and come first in group order; every other row follows sorted by
/// `(rank, within-rank key)`.
fn apply_map_groups(
    rows: &mut [NoteRow],
    bob_dir: &Path,
    snapshot: &VaultSnapshot,
) {
    let mut placement: HashMap<String, (NoteGroup, usize, Option<u8>)> =
        HashMap::new();
    let mut place = |path: &str, group: NoteGroup, depth: Option<u8>| {
        if !placement.contains_key(path) {
            let sequence = placement.len();
            placement.insert(path.to_string(), (group, sequence, depth));
        }
    };

    // 1. Today: today's daily note, then the current ISO week note.
    for candidate in today_paths(bob_dir) {
        place(&display_slash(&candidate), NoteGroup::Today, None);
    }
    // 2. Inbox: `mac_inbox` first, then the rest in canonical order.
    let mut inbox: Vec<&NoteRow> = rows
        .iter()
        .filter(|row| row.kind == NoteKindKey::Inbox)
        .collect();
    inbox.sort_by_key(|row| canonical_key(row));
    inbox.sort_by_key(|row| row.note_path != "mac_inbox.md");
    for row in inbox {
        place(&row.note_path, NoteGroup::Inbox, None);
    }
    // 3. Areas in area-tree order with depth capped at 2.
    for (path, depth) in area_order(snapshot) {
        place(&display_slash(&path), NoteGroup::Areas, depth);
    }
    // 4. Open projects in capture-targets order.
    let mut projects: Vec<&NoteRow> = rows
        .iter()
        .filter(|row| {
            row.kind == NoteKindKey::Project
                && row.status.as_deref() != Some("Done")
                && row.status.as_deref() != Some("Canceled")
        })
        .collect();
    projects.sort_by(|left, right| {
        project_map_key(left).cmp(&project_map_key(right))
    });
    for row in projects {
        place(&row.note_path, NoteGroup::Projects, None);
    }
    // 5. Notebooks in capture-targets order.
    let mut notebooks: Vec<&NoteRow> = rows
        .iter()
        .filter(|row| row.kind == NoteKindKey::Notebook)
        .collect();
    notebooks.sort_by(|left, right| {
        project_map_key(left).cmp(&project_map_key(right))
    });
    for row in notebooks {
        place(&row.note_path, NoteGroup::Notebooks, None);
    }
    // 6. Albums in album-registry order.
    for path in album_order(bob_dir, snapshot) {
        place(&path, NoteGroup::Albums, None);
    }

    for row in rows.iter_mut() {
        if let Some((group, _, depth)) = placement.get(&row.note_path) {
            row.group = Some(*group);
            row.depth = *depth;
        }
    }
    sort_canonical(rows, &placement);
}

/// Canonical order: map rows first in §5.4 placement order, then
/// `(rank, within-rank key)`. Daily and period rows sort newest-first
/// by path; every other kind sorts by case-folded title, then path.
fn sort_canonical(
    rows: &mut [NoteRow],
    placement: &HashMap<String, (NoteGroup, usize, Option<u8>)>,
) {
    rows.sort_by(|left, right| {
        match (
            placement.get(&left.note_path),
            placement.get(&right.note_path),
        ) {
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (
                Some((left_group, left_sequence, _)),
                Some((right_group, right_sequence, _)),
            ) => group_rank(*left_group)
                .cmp(&group_rank(*right_group))
                .then_with(|| left_sequence.cmp(right_sequence)),
            (None, None) => canonical_cmp(left, right),
        }
    });
}

/// Today's daily note then the current ISO week note, each only when
/// its file exists. Nothing is ever created.
fn today_paths(bob_dir: &Path) -> Vec<PathBuf> {
    let day_file = day_file_for(bob_dir);
    let today = day_file
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(parse_day_file_date)
        .unwrap_or_else(|| bob_env::current_datetime().date());
    let mut candidates = Vec::new();
    if let Ok(relative) = day_file.strip_prefix(bob_dir)
        && bob_dir.join(relative).is_file()
    {
        candidates.push(relative.to_path_buf());
    }
    let week = PeriodId::week_for_date(today).to_string();
    let iso_year = today.iso_week().year();
    for candidate in [
        PathBuf::from(format!("{iso_year}/{week}.md")),
        PathBuf::from(format!("{week}.md")),
    ] {
        if bob_dir.join(&candidate).is_file() {
            candidates.push(candidate);
            break;
        }
    }
    candidates
}

/// Area-tree order: depth-first from the graph roots, then unrooted
/// areas by stem. Depth is one-below the graph depth (top-level areas
/// read 0 and omit it), capped at 2.
fn area_order(snapshot: &VaultSnapshot) -> Vec<(PathBuf, Option<u8>)> {
    let graph = build_area_graph(snapshot);
    let mut order = Vec::new();
    let mut visited = BTreeSet::new();
    for root in &graph.root_children {
        push_area(&graph, root, &mut visited, &mut order);
    }
    let mut unplaced: Vec<_> = graph
        .areas
        .values()
        .filter(|node| !visited.contains(&node.path))
        .collect();
    unplaced.sort_by(|left, right| {
        left.stem
            .to_ascii_lowercase()
            .cmp(&right.stem.to_ascii_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    for node in unplaced {
        push_area(&graph, &node.path.clone(), &mut visited, &mut order);
    }
    order
}

fn push_area(
    graph: &crate::native::areas::AreaGraph,
    path: &Path,
    visited: &mut BTreeSet<PathBuf>,
    order: &mut Vec<(PathBuf, Option<u8>)>,
) {
    if !visited.insert(path.to_path_buf()) {
        return;
    }
    let Some(node) = graph.areas.get(path) else {
        return;
    };
    let depth = node.depth.map(|depth| depth.saturating_sub(1).min(2) as u8);
    let depth = depth.filter(|depth| *depth > 0);
    order.push((path.to_path_buf(), depth));
    for child in &node.children {
        push_area(graph, child, visited, order);
    }
}

/// Capture-targets order for root-routable notes: by route stem, then
/// path. Nested notes (never capture routes) sort after routable ones
/// by path.
fn project_map_key(row: &NoteRow) -> (bool, String, String) {
    let path = Path::new(&row.note_path);
    let routable = path
        .parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
        && is_route_stem(&row.note_path);
    (
        !routable,
        file_stem(path).to_ascii_lowercase(),
        row.note_path.clone(),
    )
}

fn is_route_stem(note_path: &str) -> bool {
    let stem = strip_extension(note_path.rsplit('/').next().unwrap_or(""));
    !stem.is_empty()
        && stem.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || byte == b'_'
                || byte == b'-'
        })
}

/// Album notes in album-registry order.
fn album_order(bob_dir: &Path, snapshot: &VaultSnapshot) -> Vec<String> {
    AlbumRegistry::from_snapshot(bob_dir, snapshot)
        .albums
        .iter()
        .map(|album| album.declaration.path.clone())
        .collect()
}

fn group_rank(group: NoteGroup) -> u8 {
    match group {
        NoteGroup::Today => 0,
        NoteGroup::Inbox => 1,
        NoteGroup::Areas => 2,
        NoteGroup::Projects => 3,
        NoteGroup::Notebooks => 4,
        NoteGroup::Albums => 5,
        NoteGroup::Done => 6,
    }
}

fn canonical_cmp(left: &NoteRow, right: &NoteRow) -> std::cmp::Ordering {
    left.rank.cmp(&right.rank).then_with(|| {
        match (is_dated_kind(left.kind), is_dated_kind(right.kind)) {
            (true, true) => right.note_path.cmp(&left.note_path),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => canonical_key(left).cmp(&canonical_key(right)),
        }
    })
}

fn is_dated_kind(kind: NoteKindKey) -> bool {
    matches!(kind, NoteKindKey::Daily | NoteKindKey::Period)
}

fn canonical_key(row: &NoteRow) -> (String, String) {
    (row.title.to_lowercase(), row.note_path.clone())
}
