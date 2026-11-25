//! Drawer scanner and read-only `bob capture-drawers` CLI.
//!
//! This module owns the title predicates (drawer titles and the
//! Pomodoro-name sibling that additionally allows `+`), checkbox detection,
//! direct-child enumeration, selector matching, and insertion geometry. The
//! shared selector slug grammar lives in `capture_language`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture::{
        self, document_line_ending, dominant_indent_unit,
        first_child_indentation, first_direct_managed_log_start, insert_at,
        insertion_text_preserving_line_endings, leading_spaces_or_tabs_len,
        leading_whitespace, line_spans, list_item_body,
        parse_managed_task_log_marker, LineSpan,
    },
    capture_language::{
        self,
        place::{
            self, format_notebook_section_create_title, PlaceCandidate,
            PlaceCatalog, PlaceOutcome, ProjectSection, TaskDrawer,
        },
    },
    capture_project_note::format_project_section_title,
    env as bob_env,
    markdown::fenced_lines,
    note_tasks::{self, BlockIdLookup, NoteTask, RefLookup, TaskRef},
    style::Styler,
    task_block_layout,
};

/// The authored form that made a list item a drawer.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DrawerMarker {
    Bold,
    LegacyCaps,
    Section,
}

impl DrawerMarker {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Bold => "bold",
            Self::LegacyCaps => "legacy_caps",
            Self::Section => "section",
        }
    }
}

/// A direct-child drawer bullet or eligible project H2 section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Drawer {
    pub(crate) title: String,
    pub(crate) key: String,
    pub(crate) slug: String,
    pub(crate) marker: DrawerMarker,
    /// Half-open UTF-8 byte offsets for the complete header line.
    pub(crate) header_range: std::ops::Range<usize>,
    /// Half-open UTF-8 byte offsets for the title alone.
    pub(crate) title_range: std::ops::Range<usize>,
    /// One-based line number of the drawer bullet.
    pub(crate) line: usize,
    pub(crate) indentation: String,
    pub(crate) block_end: usize,
    pub(crate) entry_count: usize,
}

impl Drawer {
    pub(crate) fn line_index(&self) -> usize {
        self.line - 1
    }
}

/// Body of a created task-drawer header: the resolved title in bold.
/// Titles never carry markers: bold titles reject `*` and legacy titles
/// cannot contain it, so wrapping is never doubled.
pub(crate) fn drawer_header_body(title: &str) -> String {
    format!("**{title}**")
}

/// A created task-drawer header line: `{indent}- **{title}**`.
pub(crate) fn drawer_header_line(indent: &str, title: &str) -> String {
    format!("{indent}- {}", drawer_header_body(title))
}

/// Where a capture block lands under a selected drawer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrawerInsertion {
    pub(crate) offset: usize,
    pub(crate) indentation: String,
}

/// Canonical title for a quoted drawer literal: whitespace collapsed,
/// capitalization preserved, and valid as a bold or legacy drawer title.
/// Legacy-only titles such as `NOTES:` remain selectable; creation still
/// requires a valid bold header.
pub(crate) fn normalize_drawer_literal(decoded: &str) -> Option<String> {
    let canonical = capture_language::normalize_task_text(decoded);
    if canonical.is_empty() {
        return None;
    }
    (normalize_bold_drawer_title(&canonical).is_some()
        || is_legacy_drawer_title(&canonical))
    .then_some(canonical)
}

/// Validate a title for direct whole-bold container syntax while preserving
/// its authored case.
pub(crate) fn normalize_bold_drawer_title(decoded: &str) -> Option<String> {
    let wrapped = format!("**{decoded}**");
    drawer_body_title(&wrapped)
        .filter(|(_, marker)| *marker == DrawerMarker::Bold)
        .map(|(title, _)| title.to_string())
}

/// Whitespace-collapsed Unicode uppercase key for a drawer title.
pub(crate) fn drawer_key(title: &str) -> String {
    place::place_key(title)
}

/// Whether `value` is a valid drawer slug: ASCII letter start, then ASCII
/// letters, digits, or hyphens.
pub(crate) fn is_drawer_slug(value: &str) -> bool {
    place::is_place_slug(value)
}

/// Reserved drawer keys derived from existing managed-line owners, never a
/// hand-written list.
pub(crate) fn reserved_drawer_keys() -> HashSet<String> {
    use crate::native::{
        task_dependencies::{blocks as dep_blocks, format as dep_format},
        task_relations,
    };
    let mut keys = HashSet::new();
    // Managed task log labels (Schedule, Work, Rotten) plus Cancel Log.
    for (label, _) in crate::native::capture::MANAGED_TASK_LOG_LABELS.iter() {
        keys.insert(drawer_key(label));
    }
    keys.insert(drawer_key(dep_blocks::CANCEL_LOG_LABEL));
    // Dependency labels.
    keys.insert(drawer_key(dep_format::DEPENDS_ON_LABEL));
    keys.insert(drawer_key(dep_blocks::BLOCKS_LABEL));
    // Relation labels.
    keys.insert(drawer_key(task_relations::LINKS_LABEL));
    keys.insert(drawer_key(task_relations::LINKS_TO_LABEL));
    keys.insert(drawer_key(task_relations::LINKED_FROM_LABEL));
    keys.insert(drawer_key(task_relations::BACKLINKS_LABEL));
    // Plain TASKS.
    keys.insert(drawer_key("TASKS"));
    keys
}

pub(crate) fn is_reserved_drawer_key(key: &str) -> bool {
    reserved_drawer_keys().contains(key)
}

/// Whether a task drawer selector may never name `key`: a managed-line
/// label, or a structural H2 (`## Snoozed`, `## Pomodoros`, a status group)
/// that would never read back as a drawer. The commit-time task resolver
/// and the lexical editor parse share this one predicate.
pub(crate) fn is_task_drawer_unwritable_key(key: &str) -> bool {
    is_reserved_drawer_key(key) || is_structural_h2_key(key)
}

/// The managed key a drawer selector would be refused for, if any. An exact
/// literal selector has one key; a slug selector is checked under both its
/// plain and hyphen-as-space spellings, in that order. The commit-time
/// resolver and the lexical editor parse share this one rule so a reserved
/// name never reads differently in each place.
pub(crate) fn reserved_selector_key(
    selector_text: &str,
    exact: bool,
) -> Option<String> {
    if exact {
        let key = drawer_key(selector_text);
        return is_task_drawer_unwritable_key(&key).then_some(key);
    }
    let key_plain = drawer_key(selector_text);
    let key_spaced = drawer_key(&selector_text.replace('-', " "));
    [key_plain, key_spaced]
        .into_iter()
        .find(|key| is_task_drawer_unwritable_key(key))
}

/// The task-drawer `reserved_drawer` refusal for a lexically reserved
/// selector, or `None` when the name is not managed. Whether the target is a
/// `^prj` section or has a near-duplicate is vault-dependent and left to the
/// commit-time resolver.
pub(crate) fn reserved_selector_error(
    selector_text: &str,
    exact: bool,
) -> Option<DrawerResolveError> {
    reserved_selector_key(selector_text, exact)
        .map(|key| reserved_drawer_error(&key, DrawerScope::Task))
}

// ---------------------------------------------------------------------------
// Cached drawer catalog (relation_labels/scan.rs precedent).
// ---------------------------------------------------------------------------

pub(crate) const DRAWER_CATALOG_VERSION: u32 = 3;
const DRAWER_SNAPSHOT_TTL_SECS: u64 = 60;
const DRAWER_STALE_TMP_SECS: u64 = 10 * 60;
const DRAWER_RECENT_DAYS: u64 = 90;
const DRAWER_MAX_RECENT: usize = 8;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct DrawerScanCache {
    version: u32,
    vault: String,
    #[serde(default)]
    last_scan_unix: u64,
    #[serde(default)]
    files: BTreeMap<String, CachedDrawerNote>,
    #[serde(default)]
    records: BTreeMap<String, DrawerCatalogRecord>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedDrawerNote {
    mtime_ns: u64,
    size: u64,
    drawers: Vec<DrawerCatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DrawerCatalogEntry {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) slug: String,
    pub(crate) marker: DrawerMarker,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct DrawerCatalogRecord {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) slug: String,
    pub(crate) note_count: u64,
    pub(crate) recent_count: u64,
    pub(crate) last_seen: u64,
    #[serde(default)]
    pub(crate) spellings: HashMap<String, u64>,
    /// Counts by authored title and the source marker that supplied it.
    #[serde(default)]
    pub(crate) spelling_markers: HashMap<String, HashMap<String, u64>>,
    #[serde(default)]
    pub(crate) notes: Vec<String>,
}

impl PlaceCatalog for BTreeMap<String, DrawerCatalogRecord> {
    fn spelling_for_slug(&self, slug: &str) -> String {
        drawer_spelling_for_slug(self, slug)
    }

    fn near_title(&self, new_key: &str) -> Option<String> {
        near_catalog_title(new_key, self)
    }

    fn is_legacy_title(&self, title: &str) -> bool {
        is_legacy_drawer_title(title)
    }
}

fn place_candidates(drawers: &[Drawer]) -> Vec<PlaceCandidate> {
    drawers
        .iter()
        .map(|drawer| {
            PlaceCandidate::new(
                drawer.title.clone(),
                drawer.key.clone(),
                drawer.slug.clone(),
                drawer.line,
            )
        })
        .collect()
}

fn map_drawer_outcome(
    outcome: PlaceOutcome,
    drawers: &[Drawer],
    scope: DrawerScope,
) -> Result<DrawerResolved, DrawerResolveError> {
    match outcome {
        PlaceOutcome::Existing(item) => {
            let drawer = drawers
                .iter()
                .find(|drawer| drawer.line == item.line)
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "matched place line {} missing from drawers",
                        item.line
                    )
                });
            Ok(DrawerResolved {
                outcome: DrawerResolveOutcome::Existing { drawer },
                created: false,
                warning: None,
            })
        }
        PlaceOutcome::Create {
            title,
            key,
            slug,
            warning,
        } => Ok(DrawerResolved {
            outcome: DrawerResolveOutcome::Create {
                title,
                key,
                slug,
                marker: scope.create_marker(),
            },
            created: true,
            warning,
        }),
        PlaceOutcome::Refuse { code, message } => {
            Err(DrawerResolveError { code, message })
        }
        PlaceOutcome::NearMiss { message, code, .. } => {
            Err(DrawerResolveError {
                code: code.unwrap_or("drawer_near_duplicate"),
                message: message.unwrap_or_else(|| {
                    "did you mean a nearby drawer?".to_string()
                }),
            })
        }
        PlaceOutcome::Ambiguous { matches } => {
            unreachable!("drawer adapters refuse ambiguity: {matches:?}")
        }
    }
}

pub(crate) fn drawer_now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn drawer_fingerprint(path: &Path) -> Option<(u64, u64)> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((u64::try_from(modified.as_nanos()).ok()?, metadata.len()))
}

pub(crate) fn drawer_catalog_dir() -> PathBuf {
    bob_env::bob_cli_cache_dir().join("drawers")
}

pub(crate) fn drawer_cache_path(vault: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = drawer_vault_key(vault);
    let digest = hex::encode(Sha256::digest(key.as_bytes()));
    drawer_catalog_dir()
        .join(format!("catalog-v{DRAWER_CATALOG_VERSION}-{digest}.json"))
}

pub(crate) fn drawer_vault_key(vault: &Path) -> String {
    fs::canonicalize(vault)
        .unwrap_or_else(|_| vault.to_path_buf())
        .display()
        .to_string()
}

pub(crate) fn invalidate_drawer_catalog(vault: &Path) {
    let _ = fs::remove_file(drawer_cache_path(vault));
}

fn drawer_display_relative(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

pub(crate) fn load_drawer_catalog(
    vault: &Path,
) -> io::Result<BTreeMap<String, DrawerCatalogRecord>> {
    let cache_path = drawer_cache_path(vault);
    let mut cache = fs::read_to_string(&cache_path)
        .ok()
        .and_then(|text| serde_json::from_str::<DrawerScanCache>(&text).ok())
        .filter(|cache| {
            cache.version == DRAWER_CATALOG_VERSION
                && cache.vault == drawer_vault_key(vault)
        })
        .unwrap_or_else(|| DrawerScanCache {
            version: DRAWER_CATALOG_VERSION,
            vault: drawer_vault_key(vault),
            last_scan_unix: 0,
            files: BTreeMap::new(),
            records: BTreeMap::new(),
        });
    if cache.last_scan_unix > 0
        && drawer_now_unix().saturating_sub(cache.last_scan_unix)
            < DRAWER_SNAPSHOT_TTL_SECS
    {
        return Ok(cache.records);
    }
    let (mut relative_paths, _) =
        crate::native::capture_dependency_tasks::vault_note_paths(vault);
    relative_paths.sort();
    let previous = std::mem::take(&mut cache.files);
    let mut next = BTreeMap::new();
    let mut cache_dirty = true;
    // Per-key aggregation.
    let mut per_key_notes: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    let mut per_key_recent: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    let mut per_key_spellings: BTreeMap<String, HashMap<String, u64>> =
        BTreeMap::new();
    let mut per_key_spelling_markers: BTreeMap<
        String,
        HashMap<String, HashMap<String, u64>>,
    > = BTreeMap::new();
    let mut per_key_last: BTreeMap<String, u64> = BTreeMap::new();
    let mut per_key_slug: BTreeMap<String, String> = BTreeMap::new();
    let now = drawer_now_unix();
    let recent_cutoff = now.saturating_sub(DRAWER_RECENT_DAYS * 24 * 3600);
    for relative in &relative_paths {
        let absolute = vault.join(relative);
        let Some((mtime_ns, size)) = drawer_fingerprint(&absolute) else {
            cache_dirty = true;
            continue;
        };
        let key = drawer_display_relative(relative);
        let mtime_unix = mtime_ns / 1_000_000_000;
        let is_done = crate::native::done_archive::is_archive_path(relative);
        let note = match previous.get(&key) {
            Some(cached)
                if cached.mtime_ns == mtime_ns && cached.size == size =>
            {
                cached.clone()
            }
            _ => {
                cache_dirty = true;
                scan_drawer_note(&absolute, relative, mtime_ns, size)
            }
        };
        for entry in &note.drawers {
            per_key_notes
                .entry(entry.key.clone())
                .or_default()
                .insert(key.clone());
            if !is_done && mtime_unix >= recent_cutoff {
                per_key_recent
                    .entry(entry.key.clone())
                    .or_default()
                    .insert(key.clone());
            }
            *per_key_spellings
                .entry(entry.key.clone())
                .or_default()
                .entry(entry.title.clone())
                .or_insert(0) += 1;
            *per_key_spelling_markers
                .entry(entry.key.clone())
                .or_default()
                .entry(entry.title.clone())
                .or_default()
                .entry(entry.marker.as_str().to_string())
                .or_insert(0) += 1;
            per_key_slug
                .entry(entry.key.clone())
                .or_insert_with(|| entry.slug.clone());
            let last = per_key_last.entry(entry.key.clone()).or_insert(0);
            *last = (*last).max(mtime_unix);
        }
        next.insert(key, note);
    }
    let mut records: BTreeMap<String, DrawerCatalogRecord> = BTreeMap::new();
    for (key, notes) in per_key_notes {
        let recent = per_key_recent.get(&key).map_or(0, |s| s.len() as u64);
        let spellings = per_key_spellings.remove(&key).unwrap_or_default();
        let spelling_markers =
            per_key_spelling_markers.remove(&key).unwrap_or_default();
        let title =
            most_used_spelling(&spellings).unwrap_or_else(|| key.clone());
        let slug = per_key_slug
            .get(&key)
            .cloned()
            .unwrap_or_else(|| capture_language::selector_slug(&title));
        records.insert(
            key.clone(),
            DrawerCatalogRecord {
                key: key.clone(),
                title,
                slug,
                note_count: notes.len() as u64,
                recent_count: recent,
                last_seen: per_key_last.get(&key).copied().unwrap_or(0),
                spellings,
                spelling_markers,
                notes: {
                    let mut v: Vec<String> = notes.into_iter().collect();
                    v.sort();
                    v
                },
            },
        );
    }
    cache.files = next;
    cache.last_scan_unix = drawer_now_unix();
    cache.records = records.clone();
    if cache_dirty {
        if let Ok(text) = serde_json::to_string(&cache) {
            let _ =
                crate::native::collect_done::atomic_write(&cache_path, &text);
        }
        sweep_drawer_catalog_cache(&cache.vault);
    }
    Ok(records)
}

fn most_used_spelling(spellings: &HashMap<String, u64>) -> Option<String> {
    let mut best: Option<(&String, u64)> = None;
    // Stable tie break: smallest title wins when counts tie.
    let mut titles: Vec<&String> = spellings.keys().collect();
    titles.sort();
    for title in titles {
        let count = spellings.get(title).copied().unwrap_or(0);
        match best {
            None => best = Some((title, count)),
            Some((_, best_count)) if count > best_count => {
                best = Some((title, count));
            }
            _ => {}
        }
    }
    best.map(|(title, _)| title.clone())
}

fn scan_drawer_note(
    absolute: &Path,
    _relative: &Path,
    mtime_ns: u64,
    size: u64,
) -> CachedDrawerNote {
    let contents = match fs::read(absolute) {
        Ok(bytes) => bytes,
        Err(_) => {
            return CachedDrawerNote {
                mtime_ns,
                size,
                drawers: Vec::new(),
            };
        }
    };
    // Byte-prefilter: most notes carry no drawers or H2s.
    let has_drawer_hint = contents.windows(5).any(|w| {
        w == b"#task"
            || w == b"## "
            || w == b"##\t"
            || (w.len() >= 2 && w[0] == b'#' && w[1] == b'#')
    });
    if !has_drawer_hint {
        return CachedDrawerNote {
            mtime_ns,
            size,
            drawers: Vec::new(),
        };
    }
    let Ok(text) = std::str::from_utf8(&contents) else {
        return CachedDrawerNote {
            mtime_ns,
            size,
            drawers: Vec::new(),
        };
    };
    let mut entries: Vec<DrawerCatalogEntry> = Vec::new();
    let mut seen_in_note: HashSet<String> = HashSet::new();
    let push_entry = |key: String,
                      title: String,
                      slug: String,
                      marker: DrawerMarker,
                      entries: &mut Vec<DrawerCatalogEntry>,
                      seen: &mut HashSet<String>| {
        if is_reserved_drawer_key(&key) {
            return;
        }
        if !seen.insert(key.clone()) {
            // Count distinct notes once, but track spelling usage
            // separately via the catalog aggregation (which counts
            // every occurrence). For the per-note cache we keep one
            // row per key to keep the cache small; spelling counts
            // are rebuilt from occurrences across notes at aggregation
            // time by re-scanning? Instead store one entry per
            // occurrence key here and let aggregation dedupe notes
            // while counting spellings per occurrence below.
            // To keep spelling frequency, we push duplicates and let
            // the aggregator count them, but dedupe distinct notes
            // via the notes set. So allow duplicates here.
        }
        entries.push(DrawerCatalogEntry {
            key,
            title,
            slug,
            marker,
        });
    };
    // Drawer children of #task lines.
    let settings =
        note_tasks::read_settings(absolute.parent().unwrap_or(Path::new("/")));
    // Use vault-agnostic settings fallback: read_settings needs vault
    // root, not note parent. Re-read via note_tasks with empty settings
    // when parent is not a vault; the scan still finds #task lines.
    let _ = &settings;
    // Scan drawers by reusing the task scan on this note's text.
    // We read settings from the vault root when possible; otherwise
    // fall back to a minimal scan that still finds #task markers.
    let scan_settings = note_tasks::read_settings(Path::new("/nonexistent"));
    let scan = note_tasks::scan(text, &scan_settings);
    // Also try with real settings when the note is inside a vault with
    // custom filters: prefer tasks found either way.
    for task in scan.tasks() {
        for drawer in drawers(text, task) {
            push_entry(
                drawer.key.clone(),
                drawer.title.clone(),
                drawer.slug.clone(),
                drawer.marker,
                &mut entries,
                &mut seen_in_note,
            );
        }
    }
    // H2 headings in project notes.
    if is_project_note_text(text) {
        for h2 in project_h2_entries(text) {
            push_entry(
                h2.key,
                h2.title,
                h2.slug,
                h2.marker,
                &mut entries,
                &mut seen_in_note,
            );
        }
    }
    CachedDrawerNote {
        mtime_ns,
        size,
        drawers: entries,
    }
}

fn is_project_note_text(contents: &str) -> bool {
    use crate::native::projects::{frontmatter_is_project, parse_frontmatter};
    let Some(frontmatter) = parse_frontmatter(contents) else {
        return false;
    };
    frontmatter_is_project(&frontmatter)
}

fn project_h2_entries(contents: &str) -> Vec<DrawerCatalogEntry> {
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = fenced_lines(&lines, 0..lines.len());
    let mut entries = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if fenced.contains(&idx) {
            continue;
        }
        let Some((level, title)) = crate::native::markdown::atx_heading(line)
        else {
            continue;
        };
        if level != 2 {
            continue;
        }
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        let key = drawer_key(title);
        if is_reserved_drawer_key(&key) || is_structural_h2_key(&key) {
            continue;
        }
        // Skip drawer-looking prose? H2s that are already ALL-CAPS
        // drawers are still catalogued (they share the key space).
        entries.push(DrawerCatalogEntry {
            key: key.clone(),
            title: title.to_string(),
            slug: capture_language::selector_slug(title),
            marker: DrawerMarker::Section,
        });
    }
    entries
}

pub(crate) fn is_structural_h2_key(key: &str) -> bool {
    use crate::native::task_status_groups::{
        TITLE_ACTIVE, TITLE_BLOCKED, TITLE_CLOSED,
    };
    if key == drawer_key("Tasks")
        || key == drawer_key(TITLE_ACTIVE)
        || key == drawer_key(TITLE_BLOCKED)
        || key == drawer_key(TITLE_CLOSED)
        || key == drawer_key("Pomodoros")
        || key == drawer_key("Events")
        || key == drawer_key("Highlights")
        || key == drawer_key("Snoozed")
    {
        return true;
    }
    crate::native::host_catalog::GENERATED_CATALOGS
        .iter()
        .any(|spec| key == drawer_key(spec.heading))
}

// ---------------------------------------------------------------------------
// Project-note `##` sections as drawers (phase `cli-project-drawers`).
//
// On a project note's own `^prj` task, `#drawer` resolves against the note's
// eligible `##` headings by the same exact key rules task drawers use.
// Structural headings (Tasks, the status-group titles, Pomodoros, Events,
// Highlights) are never eligible; selects against them are refused before
// the shared resolver runs so no second `## Pomodoros` is ever created.
// ---------------------------------------------------------------------------

/// Whether `parent` is the project note's own `^prj` task: the parent line
/// carries a trailing `^prj` anchor and names a `#task`.
pub(crate) fn is_prj_parent(contents: &str, parent: &NoteTask) -> bool {
    let lines = capture::line_spans(contents);
    let Some(line) = lines.get(parent.line_index) else {
        return false;
    };
    crate::native::projects::has_trailing_prj_anchor(line.text)
        && line.text.contains("#task")
}

/// Eligible `##` sections of a project note as drawer values, in document
/// order: raw heading title, exact drawer key rules, the heading's 1-based
/// line, the section's end byte offset, and the section's top-level list
/// items as the entry count. Structural and reserved headings are excluded
/// (they refuse instead of matching).
pub(crate) fn project_h2_drawers(contents: &str) -> Vec<Drawer> {
    let lines = capture::line_spans(contents);
    let text_lines: Vec<&str> = lines.iter().map(|line| line.text).collect();
    let fenced = fenced_lines(&text_lines, 0..text_lines.len());
    let mut drawers = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if fenced.contains(&idx) {
            continue;
        }
        let Some((level, title)) =
            crate::native::markdown::atx_heading(line.text)
        else {
            continue;
        };
        if level != 2 {
            continue;
        }
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        let title_start = title.as_ptr() as usize - line.text.as_ptr() as usize;
        let key = drawer_key(title);
        if is_reserved_drawer_key(&key) || is_structural_h2_key(&key) {
            continue;
        }
        let end_line = h2_section_end_line(&lines, &fenced, idx);
        let entry_count =
            h2_section_entry_count(&lines, &fenced, idx + 1, end_line);
        let header_start = line_start(&lines, idx);
        drawers.push(Drawer {
            key: key.clone(),
            slug: capture_language::selector_slug(title),
            title: title.to_string(),
            marker: DrawerMarker::Section,
            header_range: header_start..header_start + line.text.len(),
            title_range: header_start + title_start
                ..header_start + title_start + title.len(),
            line: idx + 1,
            indentation: String::new(),
            block_end: line_offset(&lines, end_line),
            entry_count,
        });
    }
    drawers
}

/// Exclusive end line index of the `##` section opened at `heading_idx`:
/// the next heading of level 2 or higher, else the end of the note. Fenced
/// headings never end a section.
fn h2_section_end_line(
    lines: &[capture::LineSpan<'_>],
    fenced: &std::collections::BTreeSet<usize>,
    heading_idx: usize,
) -> usize {
    for (offset, line) in lines[heading_idx + 1..].iter().enumerate() {
        let idx = heading_idx + 1 + offset;
        if fenced.contains(&idx) {
            continue;
        }
        if let Some((level, _)) =
            crate::native::markdown::atx_heading(line.text)
            && level <= 2
        {
            return idx;
        }
    }
    lines.len()
}

/// Byte offset where line `index` starts (`contents.len()` for one past
/// the last line).
fn line_offset(lines: &[capture::LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else if index <= lines.len() {
        lines[index - 1].end
    } else {
        lines.last().map(|line| line.end).unwrap_or(0)
    }
}

/// Count of shallowest-indent list items in a section body: the section's
/// top-level entries. Fenced and blank lines never count.
fn h2_section_entry_count(
    lines: &[capture::LineSpan<'_>],
    fenced: &std::collections::BTreeSet<usize>,
    start_idx: usize,
    end_idx: usize,
) -> usize {
    let mut indents = Vec::new();
    for idx in start_idx..end_idx.min(lines.len()) {
        if fenced.contains(&idx) {
            continue;
        }
        let text = lines[idx].text;
        if text.trim().is_empty() {
            continue;
        }
        if capture::list_item_body(text).is_some() {
            indents.push(capture::leading_spaces_or_tabs_len(text));
        }
    }
    let Some(min) = indents.iter().min().copied() else {
        return 0;
    };
    indents.into_iter().filter(|indent| *indent == min).count()
}

/// Last nonblank content line index of an H2 section body, if any.
/// Fenced code lines count as content: insertion stays positional, after
/// the section's last nonblank line.
fn h2_section_last_content_line(
    lines: &[capture::LineSpan<'_>],
    heading_idx: usize,
    end_idx: usize,
) -> Option<usize> {
    (heading_idx + 1..end_idx.min(lines.len()))
        .rev()
        .find(|idx| !lines[*idx].text.trim().is_empty())
}

/// Where a capture block lands in an H2 section: the byte offset plus
/// whether the section is empty. A nonempty section takes the start of the
/// line after its last nonblank content line; an empty section takes the
/// heading line's end, and the caller then separates with a blank line the
/// way empty sections do elsewhere.
pub(crate) fn h2_insert_position(
    contents: &str,
    drawer: &Drawer,
) -> (usize, bool) {
    let lines = capture::line_spans(contents);
    let heading_idx = drawer.line.saturating_sub(1).min(lines.len());
    let end_idx = h2_section_end_line_idx(&lines, drawer.block_end);
    match h2_section_last_content_line(&lines, heading_idx, end_idx) {
        Some(idx) => (lines[idx].end, false),
        None => (
            lines.get(heading_idx).map(|line| line.end).unwrap_or(0),
            true,
        ),
    }
}

/// Exclusive end line index of the section a drawer value's `block_end`
/// marks: the first line starting at or past that offset, else the end of
/// the note.
fn h2_section_end_line_idx(
    lines: &[capture::LineSpan<'_>],
    block_end: usize,
) -> usize {
    for idx in 0..=lines.len() {
        if line_offset(lines, idx) >= block_end {
            return idx;
        }
    }
    lines.len()
}

/// Which drawer key space a resolution runs in: task-child drawers or a
/// project note's `##` sections. The key rules are identical; only the
/// refusal nouns and the created title's casing differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawerScope {
    Task,
    ProjectSection,
}

impl DrawerScope {
    fn noun(self) -> &'static str {
        match self {
            Self::Task => "drawer",
            Self::ProjectSection => "section",
        }
    }

    /// The header marker a created drawer carries in this scope.
    pub(crate) fn create_marker(self) -> DrawerMarker {
        match self {
            Self::Task => DrawerMarker::Bold,
            Self::ProjectSection => DrawerMarker::Section,
        }
    }
}

/// Commit-time H2 resolution for a `^prj` target: structural and reserved
/// keys refuse before matching (a project `##` section is captured with
/// `@<route>#<Heading>`, never created here), then the shared exact rules
/// run with section nouns and the shared create-title rule.
pub(crate) fn resolve_h2_for_write(
    drawers: &[Drawer],
    selector_text: &str,
    exact: bool,
    catalog: &BTreeMap<String, DrawerCatalogRecord>,
    route: &str,
    parent_block_id: Option<&str>,
) -> Result<DrawerResolved, DrawerResolveError> {
    let keys: Vec<String> = if exact {
        vec![drawer_key(selector_text)]
    } else {
        let slug = selector_text.to_string();
        vec![drawer_key(&slug), drawer_key(&slug.replace('-', " "))]
    };
    for key in &keys {
        if is_structural_h2_key(key) {
            return Err(DrawerResolveError::new(
                "reserved_drawer",
                format!(
                    "use `@{route}#<Heading>` to capture into a project `##` section"
                ),
            ));
        }
        if is_reserved_drawer_key(key) {
            return Err(DrawerResolveError::new(
                "reserved_drawer",
                format!(
                    "`{key}` is managed by Bob; name the section something else"
                ),
            ));
        }
    }
    resolve_drawer_for_write(
        drawers,
        selector_text,
        exact,
        catalog,
        route,
        parent_block_id,
        DrawerScope::ProjectSection,
    )
}

/// Human preview consequence for an H2 menu row.
pub(crate) fn h2_consequence(title: &str) -> String {
    format!("Adds to ## {title}")
}

fn parse_drawer_catalog_version_and_hash(
    version_and_hash: &str,
) -> Option<(u32, &str)> {
    let (version_text, hash) = version_and_hash.split_once('-')?;
    let version: u32 = version_text.parse().ok()?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some((version, hash))
}

fn drawer_owned_temp_version(name: &str) -> Option<u32> {
    let inner = name.strip_prefix('.')?.strip_suffix(".tmp")?;
    let (catalog_part, pid_text) = inner.rsplit_once('.')?;
    if pid_text.is_empty() || !pid_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let _: u32 = pid_text.parse().ok()?;
    let rest = catalog_part
        .strip_suffix(".json")?
        .strip_prefix("catalog-v")?;
    let (version, _) = parse_drawer_catalog_version_and_hash(rest)?;
    Some(version)
}

fn sweep_drawer_catalog_cache(keep_vault: &str) {
    let Ok(entries) = fs::read_dir(drawer_catalog_dir()) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(rest) = name
            .strip_prefix("catalog-v")
            .and_then(|n| n.strip_suffix(".json"))
        {
            sweep_drawer_catalog_file(&path, rest, keep_vault);
            continue;
        }
        if let Some(version) = drawer_owned_temp_version(name) {
            if version > DRAWER_CATALOG_VERSION {
                continue;
            }
            let stale = fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age.as_secs() >= DRAWER_STALE_TMP_SECS);
            if stale {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn sweep_drawer_catalog_file(
    path: &Path,
    version_and_hash: &str,
    keep_vault: &str,
) {
    let Some((version, _)) =
        parse_drawer_catalog_version_and_hash(version_and_hash)
    else {
        return;
    };
    if version < DRAWER_CATALOG_VERSION {
        let _ = fs::remove_file(path);
        return;
    }
    if version > DRAWER_CATALOG_VERSION {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let Ok(stored) = serde_json::from_str::<DrawerScanCache>(&text) else {
        return;
    };
    if stored.version != DRAWER_CATALOG_VERSION
        || stored.vault == keep_vault
        || Path::new(&stored.vault).is_dir()
    {
        return;
    }
    let _ = fs::remove_file(path);
}

/// Ranked catalog rows: distinct non-done notes in 90d, then total
/// distinct notes, then latest mtime, then key.
pub(crate) fn rank_drawer_catalog<'a>(
    records: &'a BTreeMap<String, DrawerCatalogRecord>,
) -> Vec<&'a DrawerCatalogRecord> {
    let mut rows: Vec<&DrawerCatalogRecord> = records.values().collect();
    rows.sort_by(|a, b| {
        b.recent_count
            .cmp(&a.recent_count)
            .then_with(|| b.note_count.cmp(&a.note_count))
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.key.cmp(&b.key))
    });
    rows
}

/// Empty-query Recent: at most eight keys, each used in at least two notes.
pub(crate) fn drawer_catalog_recent<'a>(
    records: &'a BTreeMap<String, DrawerCatalogRecord>,
    exclude_keys: &HashSet<String>,
) -> Vec<&'a DrawerCatalogRecord> {
    rank_drawer_catalog(records)
        .into_iter()
        .filter(|r| r.note_count >= 2 && !exclude_keys.contains(&r.key))
        .take(DRAWER_MAX_RECENT)
        .collect()
}

fn catalog_spelling_marker(
    record: &DrawerCatalogRecord,
    title: &str,
) -> DrawerMarker {
    let markers = record.spelling_markers.get(title);
    let Some(markers) = markers else {
        return DrawerMarker::LegacyCaps;
    };
    let selected = ["bold", "legacy_caps", "section"]
        .into_iter()
        .filter_map(|name| markers.get(name).map(|count| (name, *count)))
        .max_by(|left, right| {
            left.1.cmp(&right.1).then_with(|| right.0.cmp(left.0))
        })
        .map(|(name, _)| name);
    match selected {
        Some("bold") => DrawerMarker::Bold,
        Some("section") => DrawerMarker::Section,
        _ => DrawerMarker::LegacyCaps,
    }
}

/// How a typed drawer query matches a drawer's key or slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawerMatch {
    Prefix,
    Substring,
}

/// Match a typed query against a drawer's key or slug: the key by its
/// uppercased text, the slug by its hyphenated lowercase form, so both
/// `fut` and `FUTURE W` find `FUTURE WORK`. An empty query matches nothing;
/// callers list their unfiltered rows themselves.
pub(crate) fn drawer_query_match(
    key: &str,
    slug: &str,
    query: &str,
) -> Option<DrawerMatch> {
    let q = drawer_key(query);
    let q_slug = capture_language::selector_slug(query);
    if q.is_empty() && q_slug.is_empty() {
        return None;
    }
    if (!q.is_empty() && key.starts_with(&q))
        || (!q_slug.is_empty() && slug.starts_with(&q_slug))
    {
        return Some(DrawerMatch::Prefix);
    }
    if (!q.is_empty() && key.contains(&q))
        || (!q_slug.is_empty() && slug.contains(&q_slug))
    {
        return Some(DrawerMatch::Substring);
    }
    None
}

/// Typed query: prefix before substring, on key or slug, in catalog rank
/// order within each tier.
pub(crate) fn drawer_catalog_search<'a>(
    records: &'a BTreeMap<String, DrawerCatalogRecord>,
    query: &str,
    exclude_keys: &HashSet<String>,
) -> Vec<&'a DrawerCatalogRecord> {
    let mut prefix = Vec::new();
    let mut substr = Vec::new();
    for record in rank_drawer_catalog(records) {
        if exclude_keys.contains(&record.key) {
            continue;
        }
        match drawer_query_match(&record.key, &record.slug, query) {
            Some(DrawerMatch::Prefix) => prefix.push(record),
            Some(DrawerMatch::Substring) => substr.push(record),
            None => {}
        }
    }
    prefix.into_iter().chain(substr).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DrawerResolveOutcome {
    Existing {
        drawer: Drawer,
    },
    Create {
        title: String,
        key: String,
        slug: String,
        /// The marker the writer emits for this destination: `bold` for a
        /// task `- **Title**` header, `section` for a project `## Title`
        /// heading. Existing drawers keep their own `drawer.marker`.
        marker: DrawerMarker,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrawerResolved {
    pub(crate) outcome: DrawerResolveOutcome,
    pub(crate) created: bool,
    pub(crate) warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrawerResolveError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl DrawerResolveError {
    fn new(code: &'static str, message: String) -> Self {
        Self { code, message }
    }
}

/// Commit-time drawer resolution: exact match, ambiguous, reserved,
/// near-duplicate guard (slug only), otherwise creation with catalog
/// spelling and near-catalog warning. `scope` selects the refusal nouns
/// and the created title's casing; the key rules are identical.
pub(crate) fn resolve_drawer_for_write(
    drawers: &[Drawer],
    selector_text: &str,
    exact: bool,
    catalog: &BTreeMap<String, DrawerCatalogRecord>,
    route: &str,
    parent_block_id: Option<&str>,
    scope: DrawerScope,
) -> Result<DrawerResolved, DrawerResolveError> {
    // ^prj structural and reserved keys are checked by
    // `resolve_h2_for_write` before this shared matcher runs.
    let _ = (route, parent_block_id);
    let items = place_candidates(drawers);
    let is_reserved = |key: &str| is_reserved_drawer_key(key);
    // Task drawers share the structural H2 space (`## Snoozed` belongs to
    // the snooze writer): a selector naming one is refused here so capture
    // never creates it as a `- **…**` user drawer. The created bullet
    // would never be recognized as a drawer anyway.
    let outcome = match scope {
        DrawerScope::Task => TaskDrawer::resolve(
            selector_text,
            exact,
            &items,
            catalog,
            is_task_drawer_unwritable_key,
        ),
        DrawerScope::ProjectSection => ProjectSection::resolve(
            selector_text,
            exact,
            &items,
            catalog,
            is_reserved,
        ),
    };
    map_drawer_outcome(outcome, drawers, scope)
}

/// The `reserved_drawer` refusal for a managed key, naming the key the
/// writer would have used.
fn reserved_drawer_error(key: &str, scope: DrawerScope) -> DrawerResolveError {
    DrawerResolveError::new(
        "reserved_drawer",
        format!(
            "`{key}` is managed by Bob; name the {} something else",
            scope.noun()
        ),
    )
}

pub(crate) fn find_near_duplicate_drawer<'a>(
    drawers: &'a [Drawer],
    slug: &str,
) -> Option<&'a Drawer> {
    let items = place_candidates(drawers);
    let found = place::find_key_near_duplicate(&items, slug)?;
    drawers.iter().find(|drawer| drawer.line == found.line)
}

/// Title of a catalog key within edit distance 2 of `new_key` that is used
/// in at least two notes: the typo hint behind `drawer_near_catalog`.
pub(crate) fn near_catalog_title(
    new_key: &str,
    catalog: &BTreeMap<String, DrawerCatalogRecord>,
) -> Option<String> {
    catalog
        .iter()
        .filter(|(key, record)| {
            key.as_str() != new_key && record.note_count >= 2
        })
        .find(|(key, _)| {
            crate::native::note_tasks::bounded_levenshtein(new_key, key, 2)
                .is_some()
        })
        .map(|(_, record)| preferred_create_title(record))
}

fn is_explicit_marker(marker: &str) -> bool {
    matches!(marker, "bold" | "section")
}

fn explicit_spelling_count(record: &DrawerCatalogRecord, title: &str) -> u64 {
    record
        .spelling_markers
        .get(title)
        .map(|markers| {
            markers
                .iter()
                .filter(|(marker, _)| is_explicit_marker(marker))
                .map(|(_, count)| *count)
                .sum()
        })
        .unwrap_or(0)
}

/// Preferred explicit spelling on one catalog key: bold or section
/// markers only, highest explicit count, then smallest title. A frequent
/// legacy spelling cannot defeat an available explicit one; marker
/// metadata, not the spelling's appearance, decides eligibility.
pub(crate) fn preferred_explicit_spelling(
    record: &DrawerCatalogRecord,
) -> Option<String> {
    let mut titles: Vec<&String> = record.spelling_markers.keys().collect();
    titles.sort();
    let mut best: Option<(u64, String)> = None;
    for title in titles {
        let count = explicit_spelling_count(record, title);
        if count == 0 {
            continue;
        }
        match &best {
            None => best = Some((count, title.clone())),
            Some((best_count, best_title)) => {
                if count > *best_count
                    || (count == *best_count && title < best_title)
                {
                    best = Some((count, title.clone()));
                }
            }
        }
    }
    best.map(|(_, title)| title)
}

/// Create-title for a catalog record: explicit spelling when any bold or
/// section marker exists, else the notebook rule on the record's slug so
/// a legacy-only key never reseeds ALL-CAPS.
pub(crate) fn preferred_create_title(record: &DrawerCatalogRecord) -> String {
    preferred_explicit_spelling(record).unwrap_or_else(|| {
        let slug = if record.slug.is_empty() {
            capture_language::selector_slug(&record.key)
        } else {
            record.slug.clone()
        };
        format_notebook_section_create_title(&slug)
    })
}

/// Spelling for creation from a slug: the preferred explicit catalog
/// spelling among keys whose key or hyphen-space variant matches, else
/// the notebook create-title rule. Stable tie break by smallest title.
pub(crate) fn drawer_spelling_for_slug(
    records: &BTreeMap<String, DrawerCatalogRecord>,
    slug: &str,
) -> String {
    let key_plain = drawer_key(slug);
    let key_spaced = drawer_key(&slug.replace('-', " "));
    let mut best: Option<(u64, String)> = None;
    for (key, record) in records {
        if *key != key_plain && *key != key_spaced {
            continue;
        }
        let Some(title) = preferred_explicit_spelling(record) else {
            continue;
        };
        let count = explicit_spelling_count(record, &title);
        match &best {
            None => best = Some((count, title)),
            Some((best_count, best_title)) => {
                if count > *best_count
                    || (count == *best_count && title < *best_title)
                {
                    best = Some((count, title));
                }
            }
        }
    }
    best.map(|(_, title)| title)
        .unwrap_or_else(|| format_notebook_section_create_title(slug))
}

/// Enumerate qualifying drawer bullets that are direct children of `task`.
pub(crate) fn drawers(contents: &str, task: &NoteTask) -> Vec<Drawer> {
    let lines = line_spans(contents);
    let children =
        direct_child_list_indexes(&lines, task.line_index, task.block_end);
    let mut drawers = Vec::new();
    for (index, &line_index) in children.iter().enumerate() {
        let Some(header) = recognize_drawer_header(contents, task, line_index)
        else {
            continue;
        };
        let line = lines[line_index];
        let block_end = children
            .get(index + 1)
            .map(|&next| line_start(&lines, next))
            .unwrap_or(task.block_end);
        let entry_count =
            direct_child_list_indexes(&lines, line_index, block_end).len();
        drawers.push(Drawer {
            key: drawer_key(&header.title),
            slug: capture_language::selector_slug(&header.title),
            title: header.title,
            marker: header.marker,
            header_range: header.header_range,
            title_range: header.title_range,
            line: line_index + 1,
            indentation: leading_whitespace(line.text).to_string(),
            block_end,
            entry_count,
        });
    }
    drawers
}

/// Find or create the resolved task drawer / project H2 section. The
/// returned note contains the header when it was created, ready for the
/// caller to insert its payload at the usual drawer insertion point.
pub(crate) fn ensure_drawer(
    contents: &str,
    task: &NoteTask,
    resolved: &DrawerResolved,
) -> Result<(String, Drawer, bool), String> {
    match &resolved.outcome {
        DrawerResolveOutcome::Existing { drawer } => {
            Ok((contents.to_string(), drawer.clone(), resolved.created))
        }
        DrawerResolveOutcome::Create { title, key, .. } => {
            let updated = if is_prj_parent(contents, task) {
                let ending = document_line_ending(contents);
                let double = format!("{ending}{ending}");
                let separator =
                    if contents.is_empty() || contents.ends_with(&double) {
                        String::new()
                    } else if contents.ends_with(ending) {
                        ending.to_string()
                    } else {
                        double
                    };
                format!("{contents}{separator}## {title}{ending}")
            } else {
                let lines = line_spans(contents);
                let drawer_indent = first_child_indentation(
                    &lines,
                    task.line_index,
                    task.block_end,
                    &task.indentation,
                )
                .or_else(|| {
                    dominant_indent_unit(&lines)
                        .map(|unit| format!("{}{}", task.indentation, unit))
                })
                .unwrap_or_else(|| format!("{}\t", task.indentation));
                // The block-layout module owns the slot: a created `NOTES`
                // lands first among the drawers, every other drawer keeps
                // its place before the managed logs.
                let header_offset = task_block_layout::drawer_header_offset(
                    contents,
                    task,
                    *key == task_block_layout::NOTES_KEY,
                );
                let header = drawer_header_line(&drawer_indent, title);
                let addition = insertion_text_preserving_line_endings(
                    contents,
                    header_offset,
                    &header,
                );
                let mut updated_task = task.clone();
                if header_offset <= updated_task.block_end {
                    updated_task.block_end += addition.len();
                }
                let updated = insert_at(contents, header_offset, &addition);
                let drawer = drawers(&updated, &updated_task)
                    .into_iter()
                    .find(|drawer| drawer.key == *key)
                    .ok_or_else(|| {
                        format!("failed to create drawer {title}")
                    })?;
                return Ok((updated, drawer, true));
            };

            let drawer = project_h2_drawers(&updated)
                .into_iter()
                .find(|drawer| drawer.key == *key)
                .ok_or_else(|| format!("failed to create drawer {title}"))?;
            Ok((updated, drawer, true))
        }
    }
}

/// Insertion offset and indent for a capture nested under `drawer`.
pub(crate) fn drawer_insertion(
    contents: &str,
    drawer: &Drawer,
) -> DrawerInsertion {
    let lines = line_spans(contents);
    let indentation = first_child_indentation(
        &lines,
        drawer.line_index(),
        drawer.block_end,
        &drawer.indentation,
    )
    .or_else(|| {
        dominant_indent_unit(&lines)
            .map(|unit| format!("{}{}", drawer.indentation, unit))
    })
    .unwrap_or_else(|| format!("{}\t", drawer.indentation));
    let offset = first_direct_managed_log_start(
        &lines,
        drawer.line_index(),
        drawer.block_end,
    )
    .unwrap_or(drawer.block_end);
    DrawerInsertion {
        offset,
        indentation,
    }
}

/// Text of the last direct entry a capture lands after in `drawer`, or
/// `None` for an empty drawer. Managed logs never count: a capture lands
/// before them.
pub(crate) fn drawer_last_entry_text(
    contents: &str,
    drawer: &Drawer,
) -> Option<String> {
    let lines = line_spans(contents);
    let insertion = drawer_insertion(contents, drawer);
    direct_child_list_indexes(&lines, drawer.line_index(), drawer.block_end)
        .into_iter()
        .filter(|&index| line_start(&lines, index) < insertion.offset)
        .filter(|&index| {
            parse_managed_task_log_marker(lines[index].text).is_none()
        })
        .next_back()
        .and_then(|index| list_item_body(lines[index].text))
        .map(capture_language::normalize_task_text)
        .filter(|text| !text.is_empty())
}

/// Body-level drawer grammar. This is deliberately independent of note
/// context; vault readers must call [`recognize_drawer_header`].
pub(crate) fn drawer_body_title(body: &str) -> Option<(&str, DrawerMarker)> {
    if has_task_checkbox(body) {
        return None;
    }
    let trimmed = body.trim();
    if is_legacy_drawer_title(trimmed) {
        return Some((trimmed, DrawerMarker::LegacyCaps));
    }
    let inner = trimmed.strip_prefix("**")?.strip_suffix("**")?;
    if inner.is_empty()
        || inner
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '*'))
        || inner.trim() != inner
        || inner.ends_with(':')
        || !inner.chars().any(char::is_alphanumeric)
    {
        return None;
    }
    Some((inner, DrawerMarker::Bold))
}

/// Named refusal returned by pure drawer-title validation helpers.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawerTitleRefusal {
    NotADrawerTitle,
    Reserved,
    EmptyDrawer,
}

#[cfg_attr(not(test), allow(dead_code))]
impl DrawerTitleRefusal {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::NotADrawerTitle => "not_a_drawer_title",
            Self::Reserved => "reserved",
            Self::EmptyDrawer => "empty_drawer",
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn valid_bold_drawer_title(title: &str) -> bool {
    !title.is_empty()
        && title.trim() == title
        && !title
            .chars()
            .any(|character| matches!(character, '\n' | '\r'))
        && !title.contains('*')
        && !title.ends_with(':')
        && title.chars().any(char::is_alphanumeric)
}

/// Validate a new drawer name and its required content independently of
/// recognition. Capture creation remains nonempty during the dual-read
/// window.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn can_create_drawer(
    title: &str,
    has_content: bool,
) -> Result<(), DrawerTitleRefusal> {
    if !valid_bold_drawer_title(title) {
        return Err(DrawerTitleRefusal::NotADrawerTitle);
    }
    let key = drawer_key(title);
    if is_reserved_drawer_key(&key) || is_structural_h2_key(&key) {
        return Err(DrawerTitleRefusal::Reserved);
    }
    if !has_content {
        return Err(DrawerTitleRefusal::EmptyDrawer);
    }
    Ok(())
}

/// Map a drawer header to its project-note H2 spelling. Bold titles are
/// lossless; only the legacy compatibility branch keeps the old Title Case
/// conversion during dual-read.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn heading_for_drawer(title: &str, marker: DrawerMarker) -> String {
    match marker {
        DrawerMarker::LegacyCaps => format_project_section_title(title),
        DrawerMarker::Bold | DrawerMarker::Section => title.to_string(),
    }
}

/// Map an H2 heading to a drawer title, preserving its authored bytes. Empty
/// sections map to valid empty drawers; creation policy is checked separately.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn drawer_for_heading(
    title: &str,
) -> Result<String, DrawerTitleRefusal> {
    if title.is_empty() {
        return Ok(String::new());
    }
    if !valid_bold_drawer_title(title) {
        return Err(DrawerTitleRefusal::NotADrawerTitle);
    }
    let key = drawer_key(title);
    if is_reserved_drawer_key(&key) || is_structural_h2_key(&key) {
        return Err(DrawerTitleRefusal::Reserved);
    }
    Ok(title.to_string())
}

/// A contextually recognized task drawer header, with source byte ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrawerHeader {
    pub(crate) title: String,
    pub(crate) marker: DrawerMarker,
    pub(crate) header_range: std::ops::Range<usize>,
    pub(crate) title_range: std::ops::Range<usize>,
}

/// Recognize a drawer only as a direct, live list-item child of a task.
/// Fences, quotes, managed rows, reserved keys, and the checkbox rule are
/// shared here by all vault readers.
pub(crate) fn recognize_drawer_header(
    contents: &str,
    task: &NoteTask,
    line_index: usize,
) -> Option<DrawerHeader> {
    let lines = line_spans(contents);
    let line = *lines.get(line_index)?;
    let direct_children =
        direct_child_list_indexes(&lines, task.line_index, task.block_end);
    if !direct_children.contains(&line_index) {
        return None;
    }
    let logical: Vec<&str> = lines
        .iter()
        .map(|line| list_item_body(line.text).unwrap_or(line.text))
        .collect();
    if fenced_lines(&logical, 0..logical.len()).contains(&line_index) {
        return None;
    }
    let body = list_item_body(line.text)?;
    if crate::native::markdown::is_blockquote_line(body)
        || is_managed_drawer_line(line.text)
    {
        return None;
    }
    let (title, marker) = drawer_body_title(body)?;
    let key = drawer_key(title);
    // Keep the old plain ALL-CAPS spellings of task log labels readable as
    // drawers. The rendered, managed log rows are excluded above; only a
    // bold header would collide with the managed label's reserved key.
    let legacy_log_label = marker == DrawerMarker::LegacyCaps
        && crate::native::capture::MANAGED_TASK_LOG_LABELS
            .iter()
            .any(|(label, _)| drawer_key(label) == key);
    if (is_reserved_drawer_key(&key) && !legacy_log_label)
        || is_structural_h2_key(&key)
    {
        return None;
    }
    let line_offset = line_start(&lines, line_index);
    let body_offset = body.as_ptr() as usize - line.text.as_ptr() as usize;
    let title_offset = title.as_ptr() as usize - body.as_ptr() as usize;
    let title_offset = body_offset + title_offset;
    Some(DrawerHeader {
        title: title.to_string(),
        marker,
        header_range: line_offset..line_offset + line.text.len(),
        title_range: line_offset + title_offset
            ..line_offset + title_offset + title.len(),
    })
}

fn is_managed_drawer_line(line: &str) -> bool {
    crate::native::task_relations::parse::is_subprojects_line(line)
        || crate::native::task_dependencies::is_managed_link_line(line)
        || crate::native::task_dependencies::blocks::is_cancel_log(line)
        || parse_managed_task_log_marker(line).is_some()
}

/// Title of a list-item body when its body shape is drawer-like, else `None`.
/// Contextual vault readers use [`recognize_drawer_header`].
#[cfg(test)]
fn parse_drawer_title(body: &str) -> Option<&str> {
    drawer_body_title(body).map(|(title, _)| title)
}

/// Whether `title` matches the original ALL-CAPS drawer grammar.
///
/// Shared base: lead `A-Z` or `0-9`, rest from `A-Z`, `0-9`, space, tab,
/// and `& ' ( ) , . / -`, with at least one `A-Z`. Drawer titles add `?`,
/// `!`, and `;` anywhere after the lead, plus `:` only as the title's
/// final character (so `OPTIONS:` qualifies but `NOTE: X`, `KEY:: VALUE`,
/// and `OPTIONS::` do not). `+` stays out because
/// [`parse_drawer_title`] uses this predicate to decide whether an
/// ordinary child bullet is a drawer.
pub(crate) fn is_legacy_drawer_title(title: &str) -> bool {
    let mut chars = title.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_legacy_drawer_title_first(first) {
        return false;
    }
    let mut has_letter = first.is_ascii_uppercase();
    let rest: Vec<char> = chars.collect();
    for (index, character) in rest.iter().enumerate() {
        if !is_legacy_drawer_rest(*character, index + 1 == rest.len()) {
            return false;
        }
        has_letter |= character.is_ascii_uppercase();
    }
    has_letter
}

/// Whether `name` is a canonical Pomodoro name.
///
/// Shared base: lead `A-Z` or `0-9`, rest from `A-Z`, `0-9`, space, tab,
/// `& ' ( ) , . / -`, and `+` in the non-leading position, with at least
/// one `A-Z`. Pomodoro names never take the drawer punctuation (`?`, `!`,
/// `;`, `:`), so `WHY?` is not a Pomodoro name.
pub(crate) fn is_pomodoro_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_legacy_drawer_title_first(first) {
        return false;
    }
    let mut has_letter = first.is_ascii_uppercase();
    for character in chars {
        if !is_pomodoro_rest(character) {
            return false;
        }
        has_letter |= character.is_ascii_uppercase();
    }
    has_letter
}

fn is_legacy_drawer_title_first(character: char) -> bool {
    character.is_ascii_uppercase() || character.is_ascii_digit()
}

fn is_legacy_drawer_rest(character: char, is_last: bool) -> bool {
    if is_legacy_drawer_title_first(character) {
        return true;
    }
    if matches!(
        character,
        ' ' | '\t' | '&' | '\'' | '(' | ')' | ',' | '.' | '/' | '-'
    ) {
        return true;
    }
    if matches!(character, '?' | '!' | ';') {
        return true;
    }
    if character == ':' {
        return is_last;
    }
    false
}

fn is_pomodoro_rest(character: char) -> bool {
    is_legacy_drawer_title_first(character)
        || matches!(
            character,
            ' ' | '\t' | '&' | '\'' | '(' | ')' | ',' | '.' | '/' | '-'
        )
        || character == '+'
}

/// Plugin `PROJECT_CHILD_LIST_ITEM_RE` checkbox: `[x]` plus following
/// whitespace, where `x` is a single character other than `]` or newline.
fn has_task_checkbox(body: &str) -> bool {
    let mut chars = body.chars();
    if chars.next() != Some('[') {
        return false;
    }
    let Some(status) = chars.next() else {
        return false;
    };
    if status == ']' || status == '\n' {
        return false;
    }
    if chars.next() != Some(']') {
        return false;
    }
    leading_spaces_or_tabs_len(chars.as_str()) > 0
}

fn direct_child_list_indexes(
    lines: &[LineSpan<'_>],
    parent_index: usize,
    block_end: usize,
) -> Vec<usize> {
    let parent_indent = drawer_indent_columns(lines[parent_index].text);
    let mut indexes = Vec::new();
    for (offset, line) in lines[parent_index + 1..].iter().enumerate() {
        if line.end > block_end {
            break;
        }
        let line_index = parent_index + 1 + offset;
        if drawer_indent_columns(line.text) <= parent_indent
            || list_item_body(line.text).is_none()
            || drawer_nearest_shallower_list_parent(lines, line_index)
                != Some(parent_index)
        {
            continue;
        }
        indexes.push(line_index);
    }
    indexes
}

/// Bob task lists use one tab or two spaces for one nesting unit. Normalize
/// tabs to two columns for drawer ancestry so mixed-indentation siblings keep
/// the same direct-child relationship.
fn drawer_indent_columns(line: &str) -> usize {
    let mut columns = 0;
    for character in line.chars() {
        match character {
            ' ' => columns += 1,
            '\t' => columns += 2,
            _ => break,
        }
    }
    columns
}

fn drawer_nearest_shallower_list_parent(
    lines: &[LineSpan<'_>],
    child_index: usize,
) -> Option<usize> {
    if child_index == 0 {
        return None;
    }
    let child_indent = drawer_indent_columns(lines[child_index].text);
    (0..child_index).rev().find(|&index| {
        let text = lines[index].text;
        !text.trim().is_empty()
            && drawer_indent_columns(text) < child_indent
            && list_item_body(text).is_some()
    })
}

fn line_start(lines: &[LineSpan<'_>], index: usize) -> usize {
    if index == 0 {
        0
    } else {
        lines[index - 1].end
    }
}

const COMMAND_NAME: &str = "bob capture-drawers";

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match CaptureDrawersRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_error(error, output_format),
    };

    match list_capture_drawers(&request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_error(error, output_format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("List a task's drawers and project-note sections")
        .long_about(
            "List the direct-child drawer bullets of one task in a \
routed Bob note.\n\n\
Task drawers include legacy ALL-CAPS headers and whole-bold titles.\n\n\
The command is read-only and reports every qualifying drawer in document \
order with its original title, key, whitespace-free slug, marker, line, and \
entry count, plus recent catalog drawers absent from the task. When the parent is \
the project note's own `^prj` task, the note's eligible `##` sections are \
reported instead of task-child drawers. It uses the same \
parent-task lookup and error messages as `bob capture`: a missing, \
duplicated, or non-task block ID is an error, as is a stale or ambiguous \
--task-ref. A resolved task with no drawers returns a successful empty list \
so picker callers can skip the chooser.\n\n\
Exactly one of -i/--block-id or -t/--task-ref is required. Use --block-id \
when the parent already has an ID, and --task-ref for the stale-safe picker \
ref from `bob capture-complete` or `bob capture-tasks`.",
        )
        .after_help(
            "Examples:\n  bob capture-drawers --route foo --block-id bar\n  bob capture-drawers -r foo -i bar -f json\n  bob capture-drawers -r cash -t 12:abcd1234\n  bob capture-drawers -b ~/bob -r project-alpha -i goog-exit\n\nEnvironment:\n  BOB_DIR                    Bob vault root when --bob-dir is omitted",
        )
        .disable_help_flag(true)
        .arg(block_id_arg())
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(route_arg())
        .arg(task_ref_arg())
}

fn block_id_arg() -> Arg {
    Arg::new("block-id")
        .long("block-id")
        .short('i')
        .value_name("ID")
        .help("Block ID of the parent task")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn route_arg() -> Arg {
    Arg::new("route")
        .long("route")
        .short('r')
        .value_name("NAME")
        .help("Route/name of the note that contains the task")
}

fn task_ref_arg() -> Arg {
    Arg::new("task-ref")
        .long("task-ref")
        .short('t')
        .value_name("REF")
        .help("Stale-safe task ref from capture-complete or capture-tasks")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParentSelector {
    BlockId(String),
    TaskRef(TaskRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureDrawersRequest {
    bob_dir: PathBuf,
    route: String,
    parent: ParentSelector,
}

impl CaptureDrawersRequest {
    fn from_matches(matches: &ArgMatches) -> Result<Self, CaptureDrawersError> {
        let block_id = matches.get_one::<String>("block-id");
        let task_ref = matches.get_one::<String>("task-ref");
        let parent = match (block_id, task_ref) {
            (Some(block_id), None) => {
                if !capture_language::is_block_id(block_id) {
                    return Err(CaptureDrawersError::usage(
                        "--block-id must be non-empty and contain only A-Z, a-z, 0-9 or '-'",
                    ));
                }
                ParentSelector::BlockId(block_id.clone())
            }
            (None, Some(task_ref)) => {
                let parsed = TaskRef::parse(task_ref).ok_or_else(|| {
                    CaptureDrawersError::usage(
                        "--task-ref must use <line>:<digest>",
                    )
                })?;
                ParentSelector::TaskRef(parsed)
            }
            (Some(_), Some(_)) | (None, None) => {
                return Err(CaptureDrawersError::usage(
                    "exactly one of --block-id or --task-ref is required",
                ));
            }
        };

        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            route: route_from_matches(matches)?,
            parent,
        })
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn route_from_matches(
    matches: &ArgMatches,
) -> Result<String, CaptureDrawersError> {
    let Some(route) = matches.get_one::<String>("route") else {
        return Err(CaptureDrawersError::usage("--route is required"));
    };
    if capture::is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }
    Err(CaptureDrawersError::usage(
        "--route must contain only A-Z, a-z, 0-9, '_' or '-'",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureDrawersResult {
    ok: bool,
    schema_version: u32,
    route: String,
    block_id: Option<String>,
    #[serde(rename = "ref")]
    task_ref: String,
    count: usize,
    drawers: Vec<ListedDrawer>,
    recent: Vec<RecentDrawer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ListedDrawer {
    title: String,
    key: String,
    slug: String,
    marker: DrawerMarker,
    line: usize,
    entry_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RecentDrawer {
    title: String,
    key: String,
    marker: DrawerMarker,
    note_count: u64,
    last_seen: u64,
}

fn list_capture_drawers(
    request: &CaptureDrawersRequest,
) -> Result<CaptureDrawersResult, CaptureDrawersError> {
    let relative_target = capture::route_label(&request.route);
    let target = request.bob_dir.join(&relative_target);
    let contents = match fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CaptureDrawersError::io(format!(
                "note does not exist: {}",
                target.display()
            )));
        }
        Err(error) => {
            return Err(CaptureDrawersError::io(format!(
                "read target {}: {error}",
                target.display()
            )));
        }
    };

    let settings = note_tasks::read_settings(&request.bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let parent = resolve_parent(&scan, &request.route, &request.parent)?;
    // On the project note's own `^prj` task, the note's eligible `##`
    // sections are the drawers.
    let found = if is_prj_parent(&contents, parent) {
        project_h2_drawers(&contents)
    } else {
        drawers(&contents, parent)
    };
    let drawers = found
        .iter()
        .map(|drawer| ListedDrawer {
            title: drawer.title.clone(),
            key: drawer.key.clone(),
            slug: drawer.slug.clone(),
            marker: drawer.marker,
            line: drawer.line,
            entry_count: drawer.entry_count,
        })
        .collect::<Vec<_>>();
    // Recent from catalog, excluding keys already on the task.
    let exclude: HashSet<String> =
        found.iter().map(|d| d.key.clone()).collect();
    let catalog = load_drawer_catalog(&request.bob_dir).ok();
    let recent = catalog
        .as_ref()
        .map(|records| drawer_catalog_recent(records, &exclude))
        .unwrap_or_default()
        .into_iter()
        .map(|record| {
            let title = preferred_create_title(record);
            let marker = catalog_spelling_marker(record, &title);
            RecentDrawer {
                title,
                key: record.key.clone(),
                marker,
                note_count: record.note_count,
                last_seen: record.last_seen,
            }
        })
        .collect::<Vec<_>>();

    Ok(CaptureDrawersResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        route: request.route.clone(),
        block_id: parent.block_id.clone(),
        task_ref: parent.task_ref(),
        count: drawers.len(),
        drawers,
        recent,
    })
}

fn resolve_parent<'a>(
    scan: &'a note_tasks::NoteTaskScan,
    route: &str,
    parent: &ParentSelector,
) -> Result<&'a NoteTask, CaptureDrawersError> {
    match parent {
        ParentSelector::BlockId(block_id) => match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => Ok(task),
            BlockIdLookup::NotATask {
                line_index,
                excerpt,
            } => Err(CaptureDrawersError::io(format!(
                "^{block_id} in {route}.md is not a task (line {}: {excerpt})",
                line_index + 1
            ))),
            BlockIdLookup::Duplicate(count) => {
                Err(CaptureDrawersError::io(format!(
                    "block ID ^{block_id} appears {count} times in {route}.md; make it unique before capturing"
                )))
            }
            BlockIdLookup::Missing => {
                let choices = format!(
                    "run 'bob capture-tasks -r {route}' to list task block IDs"
                );
                let message = match scan.suggest_block_id(block_id) {
                    Some(suggestion) => format!(
                        "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}? ({choices})"
                    ),
                    None => format!(
                        "no task with block ID ^{block_id} in {route}.md ({choices})"
                    ),
                };
                Err(CaptureDrawersError::io(message))
            }
        },
        ParentSelector::TaskRef(task_ref) => {
            match scan.by_ref(task_ref.line, &task_ref.digest) {
                RefLookup::Found(task) => Ok(task),
                RefLookup::Stale => Err(CaptureDrawersError::io(format!(
                    "the selected task is no longer in {route}.md; rerun the task picker"
                ))),
                RefLookup::Ambiguous => {
                    Err(CaptureDrawersError::io(format!(
                        "the selected task matches more than one line in {route}.md; rerun the task picker"
                    )))
                }
            }
        }
    }
}

fn print_success(result: &CaptureDrawersResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureDrawersResult) {
    let styler = Styler::detect();
    print!("{}", human_success(result, &styler));
}

fn human_success(result: &CaptureDrawersResult, styler: &Styler) -> String {
    let route_label = capture::route_label(&result.route);
    let mut output = format!(
        "Capture drawers {} {}",
        styler.separator(),
        styler.cyan(&route_label)
    );
    if let Some(block_id) = &result.block_id {
        output.push(' ');
        output.push_str(&styler.cyan(&format!("^{block_id}")));
    }
    output.push_str("\n\n");

    if result.drawers.is_empty() {
        output.push_str("  No drawers found.\n");
    } else {
        for drawer in &result.drawers {
            let items = format!(
                "{} {}",
                drawer.entry_count,
                if drawer.entry_count == 1 {
                    "item"
                } else {
                    "items"
                }
            );
            output.push_str(&format!(
                "  {}  {}  {}\n",
                styler.cyan(&drawer.title),
                styler.dim(&drawer.slug),
                styler.dim(&items)
            ));
        }
    }

    output.push('\n');
    output.push_str(&format!(
        "{} {}\n",
        result.count,
        if result.count == 1 {
            "drawer"
        } else {
            "drawers"
        }
    ));
    output
}

fn success_json(result: &CaptureDrawersResult) -> String {
    serde_json::to_string(result).expect("serialize capture drawers result")
}

fn print_error(error: CaptureDrawersError, output_format: OutputFormat) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureDrawersError {
    kind: CaptureDrawersErrorKind,
    message: String,
}

impl CaptureDrawersError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureDrawersErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureDrawersErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureDrawersErrorKind {
    Usage,
    Io,
}

impl CaptureDrawersErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use crate::native::note_tasks;
    use serde_json::Value;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn parent_task(contents: &str) -> NoteTask {
        let settings = note_tasks::read_settings(Path::new("/nonexistent"));
        let scan = note_tasks::scan(contents, &settings);
        scan.task_named("Parent")
            .cloned()
            .unwrap_or_else(|| panic!("missing Parent task in {contents:?}"))
    }

    fn drawers_of(contents: &str) -> Vec<Drawer> {
        drawers(contents, &parent_task(contents))
    }

    fn titles_of(contents: &str) -> Vec<String> {
        drawers_of(contents)
            .into_iter()
            .map(|drawer| drawer.title)
            .collect()
    }

    fn drawer_named<'a>(drawers: &'a [Drawer], title: &str) -> &'a Drawer {
        drawers
            .iter()
            .find(|drawer| drawer.title == title)
            .unwrap_or_else(|| panic!("missing drawer {title:?}"))
    }

    fn drawer_vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/task_drawers/vectors.json"
        ))
        .expect("valid drawer vectors")
    }

    fn vector_source(case: &Value) -> String {
        let lines: Vec<&str> = case["source_lines"]
            .as_array()
            .expect("source lines")
            .iter()
            .map(|line| line.as_str().expect("line string"))
            .collect();
        let ending = if case["eol"].as_str() == Some("crlf") {
            "\r\n"
        } else {
            "\n"
        };
        format!("{}{ending}", lines.join(ending))
    }

    fn vector_task(contents: &str, owner_id: &str) -> NoteTask {
        let settings = note_tasks::read_settings(Path::new("/nonexistent"));
        let scan = note_tasks::scan(contents, &settings);
        match scan.by_block_id(owner_id) {
            BlockIdLookup::Found(task) => task.clone(),
            _ => panic!("missing task block id {owner_id}"),
        }
    }

    fn check_drawer_expectations(
        contents: &str,
        task: &NoteTask,
        expected: &Value,
        id: &str,
    ) {
        let found = drawers(contents, task);
        let expected = expected.as_array().expect("expected drawer list");
        assert_eq!(found.len(), expected.len(), "{id}");
        for (drawer, expected) in found.iter().zip(expected) {
            assert_eq!(drawer.title, expected["title"], "{id}");
            assert_eq!(drawer.key, expected["key"], "{id}");
            assert_eq!(drawer.marker.as_str(), expected["marker"], "{id}");
            assert_eq!(
                drawer.line,
                expected["line"].as_u64().unwrap() as usize,
                "{id}"
            );
            if let Some(range) = expected["header_range"].as_array() {
                let start = range[0].as_u64().unwrap() as usize;
                let end = range[1].as_u64().unwrap() as usize;
                assert_eq!(drawer.header_range, start..end, "{id}");
                assert_eq!(
                    &contents[drawer.header_range.clone()],
                    capture::line_spans(contents)[drawer.line_index()].text,
                    "{id} header bytes"
                );
            }
            if let Some(range) = expected["title_range"].as_array() {
                let start = range[0].as_u64().unwrap() as usize;
                let end = range[1].as_u64().unwrap() as usize;
                assert_eq!(drawer.title_range, start..end, "{id}");
                assert_eq!(
                    &contents[drawer.title_range.clone()],
                    drawer.title.as_str(),
                    "{id} title bytes"
                );
            }
        }
    }

    #[test]
    fn drawer_conformance_vectors_match_reader_and_mapping() {
        let vectors = drawer_vectors();
        assert_eq!(vectors["schema_version"], 1);
        for (index, case) in
            vectors["cases"].as_array().unwrap().iter().enumerate()
        {
            let id = case["id"].as_str().unwrap();
            assert_eq!(id, format!("DV{}", index + 1));
            let contents = vector_source(case);
            let task = vector_task(&contents, case["owner"].as_str().unwrap());
            check_drawer_expectations(&contents, &task, &case["expect"], id);
            if let Some(owner) = case["other_owner"].as_str() {
                let task = vector_task(&contents, owner);
                check_drawer_expectations(
                    &contents,
                    &task,
                    &case["other_expect"],
                    id,
                );
            }
        }
        for (index, case) in
            vectors["body_cases"].as_array().unwrap().iter().enumerate()
        {
            let id = case["id"].as_str().unwrap();
            assert_eq!(id, format!("DV{}", index + 5));
            let result = drawer_body_title(case["body"].as_str().unwrap());
            if case["excluded"].as_bool().unwrap_or(false) {
                assert_eq!(result, None, "{id}");
            } else {
                let (title, marker) = result.unwrap_or_else(|| panic!("{id}"));
                assert_eq!(title, case["title"], "{id}");
                assert_eq!(marker.as_str(), case["marker"], "{id}");
            }
        }
        for (index, case) in vectors["heading_mappings"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let id = case["id"].as_str().unwrap();
            assert_eq!(id, format!("DH{}", index + 1));
            let title = case["title"].as_str().unwrap();
            let heading = heading_for_drawer(title, DrawerMarker::Bold);
            assert_eq!(heading, case["heading"], "{id}");
            assert_eq!(
                drawer_for_heading(&heading),
                Ok(title.to_string()),
                "{id}"
            );
        }
    }

    #[test]
    fn snoozed_is_structural_never_a_user_drawer() {
        // Row-model reservation (bob-cli-q5.1.1): capture never offers or
        // creates `## Snoozed` as a task drawer; the snooze writer owns it.
        assert!(is_structural_h2_key(&drawer_key("Snoozed")));
        assert!(is_structural_h2_key(&drawer_key("snoozed")));
        let contents = "- [ ] #task Parent ^parent\n\n## Snoozed\n\n- **1530** [[job#^x]]\n";
        assert!(
            !titles_of(contents).iter().any(|title| title == "Snoozed"),
            "Snoozed must not appear as a drawer"
        );
        assert!(
            resolve_drawer_for_write(
                &[],
                "Snoozed",
                false,
                &BTreeMap::new(),
                "note",
                Some("parent"),
                DrawerScope::Task,
            )
            .is_err(),
            "Snoozed must refuse drawer creation"
        );
    }

    #[test]
    fn ensure_drawer_creates_task_drawer_then_rescans_it() {
        let contents = "- [ ] #task Parent ^parent\n";
        let task = parent_task(contents);
        let resolved = resolve_drawer_for_write(
            &[],
            "requirements",
            false,
            &BTreeMap::new(),
            "note",
            Some("parent"),
            DrawerScope::Task,
        )
        .expect("valid task drawer");

        let (updated, drawer, created) =
            ensure_drawer(contents, &task, &resolved).expect("create drawer");

        assert!(created);
        assert_eq!(drawer.title, "Requirements");
        assert!(updated.contains("\t- **Requirements**\n"));
        assert_eq!(drawers(&updated, &parent_task(&updated)).len(), 1);
    }

    #[test]
    fn ensure_drawer_creates_project_h2_sibling_and_rescans_it() {
        let contents = "- [ ] #task Project ^prj\n## Tasks\n";
        let settings = note_tasks::read_settings(Path::new("/nonexistent"));
        let task = note_tasks::scan(contents, &settings)
            .task_named("Project")
            .cloned()
            .expect("project task");
        let resolved = resolve_drawer_for_write(
            &[],
            "research",
            false,
            &BTreeMap::new(),
            "project",
            Some("prj"),
            DrawerScope::ProjectSection,
        )
        .expect("valid project section");

        let (updated, drawer, created) =
            ensure_drawer(contents, &task, &resolved).expect("create section");

        assert!(created);
        assert_eq!(drawer.title, "Research");
        assert!(updated.contains("\n\n## Research\n"));
        assert!(project_h2_drawers(&updated)
            .iter()
            .any(|candidate| candidate.title == "Research"));
    }

    #[test]
    fn query_matches_a_key_or_slug_by_prefix_before_substring() {
        let m = |query: &str| {
            drawer_query_match("FUTURE WORK", "future-work", query)
        };
        assert_eq!(m("fut"), Some(DrawerMatch::Prefix));
        assert_eq!(m("FUTURE W"), Some(DrawerMatch::Prefix));
        assert_eq!(m("future-wo"), Some(DrawerMatch::Prefix));
        assert_eq!(m("work"), Some(DrawerMatch::Substring));
        assert_eq!(m("e w"), Some(DrawerMatch::Substring));
        assert_eq!(m("perf"), None);
        // An empty query matches nothing; callers list their own rows.
        assert_eq!(m(""), None);
        assert_eq!(m("  "), None);
    }

    #[test]
    fn catalog_search_ranks_prefix_tier_first_and_skips_excluded_keys() {
        let record = |key: &str, notes: u64| DrawerCatalogRecord {
            key: key.to_string(),
            title: key.to_string(),
            slug: capture_language::selector_slug(key),
            note_count: notes,
            recent_count: notes,
            ..DrawerCatalogRecord::default()
        };
        let records: BTreeMap<String, DrawerCatalogRecord> = [
            record("SUB FUTURE", 9),
            record("FUTURE ONE", 1),
            record("FUTURE TWO", 5),
            record("OTHER", 9),
        ]
        .into_iter()
        .map(|record| (record.key.clone(), record))
        .collect();
        let exclude: HashSet<String> =
            ["FUTURE TWO".to_string()].into_iter().collect();
        let keys: Vec<&str> =
            drawer_catalog_search(&records, "future", &exclude)
                .into_iter()
                .map(|record| record.key.as_str())
                .collect();
        assert_eq!(keys, ["FUTURE ONE", "SUB FUTURE"]);
        assert!(drawer_catalog_search(&records, "", &exclude).is_empty());
    }

    #[test]
    fn catalog_v3_rebuilds_v2_and_keeps_marker_provenance_per_spelling() {
        assert_eq!(DRAWER_CATALOG_VERSION, 3);
        let temp = TempDir::new("bob-cli-drawer-catalog-v3");
        fs::write(
            temp.path().join("one.md"),
            "- [ ] #task Parent\n  - **Future Work**\n    - note\n  - FUTURE WORK\n",
        )
        .expect("write note");
        let v3_path = drawer_cache_path(temp.path());
        let v2_name = v3_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("cache filename")
            .replacen("catalog-v3-", "catalog-v2-", 1);
        let v2_path = v3_path.with_file_name(v2_name);
        fs::create_dir_all(v2_path.parent().unwrap()).expect("cache dir");
        fs::write(
            &v2_path,
            serde_json::to_string(&DrawerScanCache {
                version: 2,
                vault: drawer_vault_key(temp.path()),
                last_scan_unix: drawer_now_unix(),
                files: BTreeMap::new(),
                records: BTreeMap::new(),
            })
            .unwrap(),
        )
        .expect("write stale v2 cache");

        let records = load_drawer_catalog(temp.path()).expect("rebuild cache");
        let record = &records["FUTURE WORK"];
        assert_eq!(record.spellings["Future Work"], 1);
        assert_eq!(record.spellings["FUTURE WORK"], 1);
        assert_eq!(record.spelling_markers["Future Work"]["bold"], 1);
        assert_eq!(record.spelling_markers["FUTURE WORK"]["legacy_caps"], 1);
        let cached: DrawerScanCache = serde_json::from_str(
            &fs::read_to_string(v3_path).expect("v3 cache exists"),
        )
        .expect("v3 cache JSON");
        assert_eq!(cached.version, 3);

        let mut explicit_only = BTreeMap::new();
        explicit_only.insert(
            "FUTURE WORK".to_string(),
            DrawerCatalogRecord {
                key: "FUTURE WORK".to_string(),
                title: "Future Work".to_string(),
                slug: "future-work".to_string(),
                spellings: HashMap::from([("Future Work".to_string(), 5)]),
                spelling_markers: HashMap::from([(
                    "Future Work".to_string(),
                    HashMap::from([("bold".to_string(), 5)]),
                )]),
                ..DrawerCatalogRecord::default()
            },
        );
        let created = resolve_drawer_for_write(
            &[],
            "future-work",
            false,
            &explicit_only,
            "one",
            Some("parent"),
            DrawerScope::Task,
        )
        .expect("explicit catalog spelling wins");
        let DrawerResolveOutcome::Create { title, .. } = created.outcome else {
            panic!("new task drawer should be created")
        };
        assert_eq!(title, "Future Work");

        let mut legacy_only = BTreeMap::new();
        legacy_only.insert(
            "NON-GOALS".to_string(),
            DrawerCatalogRecord {
                key: "NON-GOALS".to_string(),
                title: "NON-GOALS".to_string(),
                slug: "non-goals".to_string(),
                spellings: HashMap::from([("NON-GOALS".to_string(), 9)]),
                spelling_markers: HashMap::from([(
                    "NON-GOALS".to_string(),
                    HashMap::from([("legacy_caps".to_string(), 9)]),
                )]),
                ..DrawerCatalogRecord::default()
            },
        );
        let created = resolve_drawer_for_write(
            &[],
            "non-goals",
            false,
            &legacy_only,
            "one",
            Some("parent"),
            DrawerScope::Task,
        )
        .expect("legacy-only falls back to the notebook rule");
        let DrawerResolveOutcome::Create { title, .. } = created.outcome else {
            panic!("new task drawer should be created")
        };
        assert_eq!(title, "Non Goals");
    }

    #[test]
    fn last_entry_text_is_the_direct_entry_a_capture_lands_after() {
        let contents = concat!(
            "- [ ] #task Parent ^p\n",
            "\t- REQUIREMENTS\n",
            "\t\t- first\n",
            "\t\t- last   entry\n",
            "\t\t\t- nested\n",
            "\t\t- \u{1f5d3}\u{fe0f} **SCHEDULE LOG**\n",
            "\t- EMPTY\n",
        );
        let found = drawers_of(contents);
        assert_eq!(
            drawer_last_entry_text(
                contents,
                drawer_named(&found, "REQUIREMENTS")
            )
            .as_deref(),
            Some("last entry")
        );
        assert_eq!(
            drawer_last_entry_text(contents, drawer_named(&found, "EMPTY")),
            None
        );
    }

    #[test]
    fn reserved_refusals_name_the_reserved_key() {
        let empty = BTreeMap::new();
        for (selector, exact, key) in [
            ("tasks", false, "TASKS"),
            ("links", false, "LINKS"),
            ("Tasks", true, "TASKS"),
        ] {
            let error = resolve_drawer_for_write(
                &[],
                selector,
                exact,
                &empty,
                "foo",
                Some("bar"),
                DrawerScope::Task,
            )
            .expect_err("reserved key refuses");
            assert_eq!(error.code, "reserved_drawer");
            assert_eq!(
                error.message,
                format!(
                    "`{key}` is managed by Bob; name the drawer something else"
                )
            );
        }
    }

    #[test]
    fn slug_trims_collapses_whitespace_and_lowercases() {
        assert_eq!(
            capture_language::selector_slug("REQUIREMENTS"),
            "requirements"
        );
        assert_eq!(
            capture_language::selector_slug("FUTURE WORK"),
            "future-work"
        );
        assert_eq!(
            capture_language::selector_slug("FUTURE  WORK"),
            "future-work"
        );
        assert_eq!(capture_language::selector_slug("  Q&A  "), "q&a");
        assert_eq!(capture_language::selector_slug("NON-GOALS"), "non-goals");
        assert_eq!(
            capture_language::selector_slug("WHAT'S NEXT"),
            "what's-next"
        );
        assert_eq!(capture_language::selector_slug(""), "");
    }

    #[test]
    fn title_whitelist_edges() {
        assert_eq!(
            parse_drawer_title("1 REQUIREMENTS"),
            Some("1 REQUIREMENTS")
        );
        assert_eq!(
            parse_drawer_title("Q&A (DRAFT), V2.0/OK-GO"),
            Some("Q&A (DRAFT), V2.0/OK-GO")
        );
        assert_eq!(parse_drawer_title("WHAT'S NEXT"), Some("WHAT'S NEXT"));
        assert_eq!(parse_drawer_title("FUTURE  WORK"), Some("FUTURE  WORK"));
        assert_eq!(
            parse_drawer_title("  REQUIREMENTS  "),
            Some("REQUIREMENTS")
        );

        for body in [
            "SNAKE_CASE",
            "requirements",
            "Requirements",
            "123",
            "",
            "[[SOME_NOTE]]",
            "#TODO",
            "REQUIREMENTS ^abc",
            "`CODE`",
            "FOO_BAR",
            "A_B",
            "A+B",
            "C++",
            "BOB+SASE",
        ] {
            assert_eq!(parse_drawer_title(body), None, "{body:?}");
        }

        assert!(is_pomodoro_name("C++"));
        assert!(is_pomodoro_name("BOB+SASE"));
        assert!(is_pomodoro_name("A+B"));
        assert!(!is_legacy_drawer_title("C++"));
        assert!(!is_legacy_drawer_title("BOB+SASE"));
        assert!(!is_legacy_drawer_title("A+B"));
        // Depends-On lines are never drawers
        // (`docs/task-dependencies.md` §2.4).
        for title in [
            "⛓️ **DEPENDS ON:** [[#^a]]",
            "🔗 **DEPENDENCIES:** [[#^a]]",
            "**DEPENDS ON:** [[#^a]]",
            "⛓ **DEPENDS ON:**",
        ] {
            assert!(!is_legacy_drawer_title(title), "{title}");
        }
        for invalid in ["+", "+A", "++"] {
            assert!(!is_pomodoro_name(invalid), "{invalid}");
            assert!(!is_legacy_drawer_title(invalid), "{invalid}");
        }

        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- 1 REQUIREMENTS\n",
            "\t- Q&A (DRAFT), V2.0/OK-GO\n",
            "\t- FUTURE  WORK\n",
            "\t- SNAKE_CASE\n",
            "\t- requirements\n",
            "\t- 123\n",
            "\t- [[SOME_NOTE]]\n",
            "\t- #TODO\n",
            "\t- REQUIREMENTS ^abc\n",
            "\t- `CODE`\n",
            "\t- A+B\n",
            "\t- NOTES\n",
        );
        assert_eq!(
            titles_of(contents),
            [
                "1 REQUIREMENTS",
                "Q&A (DRAFT), V2.0/OK-GO",
                "FUTURE  WORK",
                "NOTES"
            ]
        );
        assert_eq!(
            drawers_of(contents)
                .into_iter()
                .map(|drawer| drawer.slug)
                .collect::<Vec<_>>(),
            [
                "1-requirements",
                "q&a-(draft),-v2.0/ok-go",
                "future-work",
                "notes"
            ]
        );
    }

    #[test]
    fn sentence_punctuation_drawer_titles_qualify() {
        // `punctuation = sentence`: `?`, `!`, `;` anywhere after the lead,
        // `:` only as the final character.
        for title in [
            "WHAT IS SASE?",
            "WOW!",
            "WHAT'S NEXT?",
            "A;B",
            "OPTIONS:",
            "WHAT NEXT:",
        ] {
            assert!(is_legacy_drawer_title(title), "{title:?}");
            assert_eq!(parse_drawer_title(title), Some(title), "{title:?}");
        }
        for title in [
            "?WHY",
            "!WOW",
            ";LEAD",
            ":LEAD",
            "NOTE: X",
            "PHONE: (609) 597-4111",
            "KEY:: VALUE",
            "OPTIONS::",
            "C++",
            "A+B",
            "FOO:BAR",
        ] {
            assert!(!is_legacy_drawer_title(title), "{title:?}");
            assert_eq!(parse_drawer_title(title), None, "{title:?}");
        }
        // Pomodoro names never take the drawer punctuation.
        assert!(!is_pomodoro_name("WHY?"));
        assert!(!is_pomodoro_name("WHAT IS SASE?"));
        assert!(!is_pomodoro_name("OPTIONS:"));
        assert!(is_pomodoro_name("C++"));
        assert!(is_pomodoro_name("A+B"));

        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- WHAT IS SASE?\n",
            "\t\t- a note\n",
        );
        let found = drawers_of(contents);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "WHAT IS SASE?");
        assert_eq!(found[0].key, "WHAT IS SASE?");
        assert_eq!(found[0].slug, "what-is-sase?");
    }

    #[test]
    fn bold_drawer_titles_keep_authored_bytes_and_unicode_keys() {
        for title in ["API Design", "C++ Notes", "Q1: Goals", "Café", "123"] {
            assert_eq!(
                drawer_body_title(&format!("**{title}**")),
                Some((title, DrawerMarker::Bold)),
            );
        }
        assert_eq!(drawer_key("Straße"), "STRASSE");
        assert_eq!(drawer_key("Café"), "CAFÉ");
        for body in [
            "** API Design**",
            "**API Design **",
            "****",
            "**Note:**",
            "**Name* With Star**",
            "**API Design** trailing",
            "**API Design** ^id",
            "**API Design** #tag",
            "__API Design__",
            "- [ ] requirement",
            "[ ] requirement",
        ] {
            assert_eq!(drawer_body_title(body), None, "{body:?}");
        }
    }

    #[test]
    fn drawer_heading_mapping_is_lossless_and_creation_stays_nonempty() {
        for title in [
            "API Design",
            "C++ Notes",
            "Q1: Goals",
            "Café",
            "Internal   whitespace",
        ] {
            let heading = heading_for_drawer(title, DrawerMarker::Bold);
            assert_eq!(heading, title);
            assert_eq!(drawer_for_heading(&heading), Ok(title.to_string()));
        }
        assert_eq!(
            heading_for_drawer("API DESIGN", DrawerMarker::LegacyCaps),
            format_project_section_title("API DESIGN")
        );
        assert_eq!(drawer_for_heading(""), Ok(String::new()));
        assert_eq!(heading_for_drawer("", DrawerMarker::Bold), "");
        for title in ["***", "Label:", "!!!"] {
            assert_eq!(
                drawer_for_heading(title).unwrap_err().code(),
                "not_a_drawer_title"
            );
            assert_eq!(
                can_create_drawer(title, true).unwrap_err().code(),
                "not_a_drawer_title"
            );
        }
        for title in ["Tasks", "Projects"] {
            assert_eq!(
                drawer_for_heading(title).unwrap_err().code(),
                "reserved"
            );
            assert_eq!(
                can_create_drawer(title, true).unwrap_err().code(),
                "reserved"
            );
        }
        assert_eq!(
            can_create_drawer("Future Work", false).unwrap_err().code(),
            "empty_drawer"
        );
        assert_eq!(can_create_drawer("Future Work", true), Ok(()));
    }

    #[test]
    fn contextual_reader_filters_false_drawers_and_reports_byte_ranges() {
        let contents = concat!(
            "- [ ] #task Parent\r\n",
            "  - **API Design**\r\n",
            "    - note\r\n",
            "\t- **C++ Notes**\r\n",
            "  - **Q1: Goals**\r\n",
            "  - **Café**\r\n",
            "  - NOTES\r\n",
            "  - **Notes**\r\n",
            "  - **Tasks**\r\n",
            "  - **Note:**\r\n",
            "  - **Trailing** text\r\n",
            "  - __Escaped__\r\n",
            "  - > **Quoted**\r\n",
            "  - ```md\r\n",
            "    - **Fenced**\r\n",
            "  - ```\r\n",
            "  - ordinary note\r\n",
            "    - **Nested note**\r\n",
        );
        let found = drawers_of(contents);
        assert_eq!(
            found
                .iter()
                .map(|drawer| drawer.title.as_str())
                .collect::<Vec<_>>(),
            [
                "API Design",
                "C++ Notes",
                "Q1: Goals",
                "Café",
                "NOTES",
                "Notes"
            ]
        );
        let api = drawer_named(&found, "API Design");
        assert_eq!(&contents[api.header_range.clone()], "  - **API Design**");
        assert_eq!(&contents[api.title_range.clone()], "API Design");
        let cafe = drawer_named(&found, "Café");
        assert_eq!(&contents[cafe.title_range.clone()], "Café");
        let notes = found.iter().filter(|drawer| drawer.key == "NOTES").count();
        assert_eq!(
            notes, 2,
            "case twins remain visible for ambiguity handling"
        );
    }

    #[test]
    fn near_duplicate_suggests_quoted_title_when_slug_is_invalid() {
        // `slug_reach = exact`: `#what-is-sase` does not select
        // `WHAT IS SASE?`; it refuses with a quoted suggestion.
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- WHAT IS SASE?\n",
            "\t\t- existing\n",
        );
        let found = drawers_of(contents);
        assert_eq!(found.len(), 1);
        let error = resolve_drawer_for_write(
            &found,
            "what-is-sase",
            false,
            &BTreeMap::new(),
            "foo",
            Some("bar"),
            DrawerScope::Task,
        )
        .expect_err("punctuated drawer is a near-duplicate");
        assert_eq!(error.code, "drawer_near_duplicate");
        assert_eq!(
            error.message,
            "did you mean `#\"WHAT IS SASE?\"`? write `#\"What Is Sase\"` to create What Is Sase"
        );
    }

    #[test]
    fn checkboxed_all_caps_children_are_not_drawers() {
        assert_eq!(parse_drawer_title("[ ] REQUIREMENTS"), None);
        assert_eq!(parse_drawer_title("[x] REQUIREMENTS"), None);
        assert_eq!(parse_drawer_title("[/] REQUIREMENTS"), None);
        assert_eq!(parse_drawer_title("[x]REQUIREMENTS"), None);

        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- [ ] REQUIREMENTS\n",
            "\t- [x] REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
        );
        assert_eq!(titles_of(contents), ["FUTURE WORK"]);
    }

    #[test]
    fn grandchild_is_not_a_direct_child_drawer() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- GRANDCHILD\n",
            "\t- FUTURE WORK\n",
        );
        let drawers = drawers_of(contents);
        assert_eq!(
            drawers
                .iter()
                .map(|drawer| drawer.title.as_str())
                .collect::<Vec<_>>(),
            ["REQUIREMENTS", "FUTURE WORK"]
        );
        assert_eq!(drawer_named(&drawers, "REQUIREMENTS").entry_count, 1);
        assert_eq!(drawer_named(&drawers, "FUTURE WORK").entry_count, 0);
    }

    #[test]
    fn empty_drawer_bullet_still_qualifies() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
        );
        let drawers = drawers_of(contents);
        assert_eq!(drawers.len(), 2);
        assert_eq!(drawers[0].title, "REQUIREMENTS");
        assert_eq!(drawers[0].entry_count, 0);
        assert_eq!(drawers[1].entry_count, 0);
    }

    #[test]
    fn ordered_and_star_plus_markers_qualify() {
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t1. REQUIREMENTS\n",
            "\t* FUTURE WORK\n",
            "\t+ NOTES\n",
            "\t2) Q&A\n",
        );
        assert_eq!(
            titles_of(contents),
            ["REQUIREMENTS", "FUTURE WORK", "NOTES", "Q&A"]
        );
    }

    #[test]
    fn tab_two_space_four_space_and_mixed_indentation() {
        let tab = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORK\n",
        );
        let tab_drawers = drawers_of(tab);
        assert_eq!(
            tab_drawers
                .iter()
                .map(|drawer| drawer.title.as_str())
                .collect::<Vec<_>>(),
            ["REQUIREMENTS", "FUTURE WORK"]
        );
        assert_eq!(tab_drawers[0].indentation, "\t");
        assert_eq!(drawer_insertion(tab, &tab_drawers[0]).indentation, "\t\t");

        let two_space = concat!(
            "- [ ] #task Parent\n",
            "  - REQUIREMENTS\n",
            "    - existing\n",
            "  - FUTURE WORK\n",
        );
        let two_space_drawers = drawers_of(two_space);
        assert_eq!(two_space_drawers[0].indentation, "  ");
        assert_eq!(
            drawer_insertion(two_space, &two_space_drawers[0]).indentation,
            "    "
        );

        let four_space = concat!(
            "- [ ] #task Parent\n",
            "    - REQUIREMENTS\n",
            "        - existing\n",
            "    - FUTURE WORK\n",
        );
        let four_space_drawers = drawers_of(four_space);
        assert_eq!(four_space_drawers[0].indentation, "    ");
        assert_eq!(
            drawer_insertion(four_space, &four_space_drawers[0]).indentation,
            "        "
        );

        // Mixed-indent siblings are both direct children via nearest-shallower
        // ancestry; do not collapse to the plugin's shallowest-only rule.
        let mixed = concat!(
            "- [ ] #task Parent\n",
            "    - REQUIREMENTS\n",
            "  - FUTURE WORK\n",
        );
        assert_eq!(titles_of(mixed), ["REQUIREMENTS", "FUTURE WORK"]);
    }

    #[test]
    fn managed_logs_are_never_drawers_plain_titles_are() {
        let recognized = [
            "\t- 🗓️ **SCHEDULE LOG**",
            "\t* **SCHEDULE LOG**",
            "\t+ **SCHEDULE LOG:**",
            "\t1. **Schedule log:**",
            "\t2) 🗓️ **Schedule log**",
            "\t- 🛠️ **WORK LOG**",
            "\t* **WORK LOG**",
            "\t+ **Work log:**",
            "\t10. **Work log**",
            "\t3) 🛠️ **WORK LOG:**",
            "\t- 🥀 **ROTTEN LOG**",
            "\t* **ROTTEN LOG**",
            "\t+ **Rotten log:**",
            "\t10. **Rotten log**",
            "\t3) 🥀 **ROTTEN LOG:**",
        ];
        for marker in recognized {
            let contents = format!("- [ ] #task Parent\n{marker}\n\t- NOTES\n");
            assert_eq!(titles_of(&contents), ["NOTES"], "{marker}");
        }

        let plain = concat!(
            "- [ ] #task Parent\n",
            "\t- SCHEDULE LOG\n",
            "\t- WORK LOG\n",
            "\t- ROTTEN LOG\n",
        );
        assert_eq!(
            titles_of(plain),
            ["SCHEDULE LOG", "WORK LOG", "ROTTEN LOG"]
        );
    }

    #[test]
    fn live_resolver_selects_by_key_and_refuses_prefix_as_near_duplicate() {
        let empty = BTreeMap::new();
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- FUTURE WORKFLOW\n",
            "\t- FUTURE WORK\n",
        );
        let drawers = drawers_of(contents);
        let resolved = resolve_drawer_for_write(
            &drawers,
            "future-work",
            false,
            &empty,
            "note",
            Some("parent"),
            DrawerScope::Task,
        )
        .expect("key form selects FUTURE WORK");
        let DrawerResolveOutcome::Existing { drawer } = resolved.outcome else {
            panic!("expected existing drawer: {resolved:?}");
        };
        assert_eq!(drawer.title, "FUTURE WORK");
        assert_eq!(drawer.line, 3);

        let exact = resolve_drawer_for_write(
            &drawers,
            "Future Work",
            true,
            &empty,
            "note",
            Some("parent"),
            DrawerScope::Task,
        )
        .expect("quoted key matches FUTURE WORK");
        let DrawerResolveOutcome::Existing { drawer } = exact.outcome else {
            panic!("expected existing drawer: {exact:?}");
        };
        assert_eq!(drawer.title, "FUTURE WORK");

        let prefix = resolve_drawer_for_write(
            &drawers,
            "future",
            false,
            &empty,
            "note",
            Some("parent"),
            DrawerScope::Task,
        )
        .expect_err("prefix is a near-duplicate, not a selection");
        assert_eq!(prefix.code, "drawer_near_duplicate");
    }

    #[test]
    fn live_resolver_suggests_the_earliest_distance_1_drawer() {
        let empty = BTreeMap::new();
        let contents = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t- FUTURE WORK\n",
            "\t- NOTES\n",
            "\t- NOTE\n",
        );
        let drawers = drawers_of(contents);
        let error = resolve_drawer_for_write(
            &drawers,
            "requirments",
            false,
            &empty,
            "note",
            Some("parent"),
            DrawerScope::Task,
        )
        .expect_err("distance 1 is a near-duplicate");
        assert_eq!(error.code, "drawer_near_duplicate");
        assert!(error.message.contains("#requirements"), "{}", error.message);
    }

    #[test]
    fn insertion_geometry_for_middle_last_blank_and_managed_log() {
        let middle = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t- FUTURE WORK\n",
            "\t\t- later\n",
            "\t- NOTES\n",
        );
        let drawers = drawers_of(middle);
        let requirements = drawer_named(&drawers, "REQUIREMENTS");
        let future = drawer_named(&drawers, "FUTURE WORK");
        let notes = drawer_named(&drawers, "NOTES");
        assert_eq!(
            drawer_insertion(middle, requirements).offset,
            middle.find("\t- FUTURE WORK").expect("middle sibling")
        );
        assert_eq!(
            drawer_insertion(middle, future).offset,
            middle.find("\t- NOTES").expect("last-but-one sibling")
        );
        assert_eq!(drawer_insertion(middle, notes).offset, middle.len());
        assert_eq!(notes.entry_count, 0);

        let blank = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\n",
            "\t- FUTURE WORK\n",
        );
        let blank_drawers = drawers_of(blank);
        assert_eq!(
            drawer_insertion(blank, &blank_drawers[0]).offset,
            blank.find("\t- FUTURE WORK").expect("after blank")
        );

        let managed = concat!(
            "- [ ] #task Parent\n",
            "\t- REQUIREMENTS\n",
            "\t\t- 🗓️ **SCHEDULE LOG**\n",
            "\t\t\t- *2026-08-01* — scheduled\n",
            "\t- FUTURE WORK\n",
        );
        let managed_drawers = drawers_of(managed);
        let managed_requirements =
            drawer_named(&managed_drawers, "REQUIREMENTS");
        assert_eq!(managed_requirements.entry_count, 1);
        assert_eq!(
            drawer_insertion(managed, managed_requirements).offset,
            managed
                .find("\t\t- 🗓️ **SCHEDULE LOG**")
                .expect("managed log")
        );
        assert_eq!(
            drawer_insertion(managed, managed_requirements).indentation,
            "\t\t"
        );
    }

    #[test]
    fn insertion_geometry_preserves_crlf_offsets() {
        let contents = concat!(
            "- [ ] #task Parent\r\n",
            "\t- REQUIREMENTS\r\n",
            "\t\t- existing\r\n",
            "\t- FUTURE WORK\r\n",
        );
        let drawers = drawers_of(contents);
        let requirements = drawer_named(&drawers, "REQUIREMENTS");
        let offset = drawer_insertion(contents, requirements).offset;
        assert_eq!(
            offset,
            contents.find("\t- FUTURE WORK").expect("crlf sibling")
        );
        assert!(contents[..offset].ends_with("\r\n"));
        assert_eq!(
            drawer_insertion(contents, drawer_named(&drawers, "FUTURE WORK"))
                .offset,
            contents.len()
        );
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        build_cli().debug_assert();
    }

    #[test]
    fn lists_drawers_in_document_order_for_a_block_id() {
        let temp = TempDir::new("bob-cli-capture-drawers-list");
        write_settings(temp.path());
        write_file(
            &temp.path().join("foo.md"),
            concat!(
                "# Tasks\n",
                "- [ ] #task Parent ^bar\n",
                "\t- REQUIREMENTS\n",
                "\t\t- existing\n",
                "\t- FUTURE WORK\n",
                "\t- NOTES\n",
            ),
        );

        let result = list_capture_drawers(&request(
            temp.path(),
            "Foo",
            ParentSelector::BlockId("bar".to_string()),
        ))
        .expect("list");

        assert_eq!(result.schema_version, 1);
        assert_eq!(result.route, "foo");
        assert_eq!(result.block_id.as_deref(), Some("bar"));
        assert_eq!(result.count, 3);
        assert_eq!(result.drawers[0].title, "REQUIREMENTS");
        assert_eq!(result.drawers[0].slug, "requirements");
        assert_eq!(result.drawers[0].line, 3);
        assert_eq!(result.drawers[0].key, "REQUIREMENTS");
        assert_eq!(result.drawers[0].entry_count, 1);
        assert_eq!(result.drawers[1].title, "FUTURE WORK");
        assert_eq!(result.drawers[1].slug, "future-work");
        assert_eq!(result.drawers[2].title, "NOTES");
        assert_eq!(result.drawers[2].entry_count, 0);
        assert!(result.task_ref.starts_with("2:"));
    }

    #[test]
    fn resolved_task_with_no_drawers_is_a_successful_empty_list() {
        let temp = TempDir::new("bob-cli-capture-drawers-empty");
        write_settings(temp.path());
        write_file(&temp.path().join("foo.md"), "- [ ] #task Parent ^bar\n");

        let result = list_capture_drawers(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("bar".to_string()),
        ))
        .expect("empty list");

        assert_eq!(result.count, 0);
        assert!(result.drawers.is_empty());
        let human = human_success(&result, &Styler::plain());
        assert!(human.contains("No drawers found."), "{human}");
        assert!(human.contains("0 drawers"), "{human}");
    }

    #[test]
    fn missing_note_and_unresolvable_parents_are_errors() {
        let temp = TempDir::new("bob-cli-capture-drawers-errors");
        write_settings(temp.path());
        let original = concat!(
            "Plain heading ^plain-id\n",
            "- [ ] #task Ready ^ready-id\n",
            "- [ ] #task Dup ^dup-id\n",
            "- [ ] #task Also dup ^dup-id\n",
            "- [ ] #task Same\n",
            "- [ ] #task Same\n",
        );
        write_file(&temp.path().join("foo.md"), original);
        let same_digest = task_ref_for(original, "Same").digest;

        let missing_note = list_capture_drawers(&request(
            temp.path(),
            "missing",
            ParentSelector::BlockId("bar".to_string()),
        ))
        .expect_err("missing note");
        assert_eq!(missing_note.kind, CaptureDrawersErrorKind::Io);
        assert!(
            missing_note.message.contains("does not exist"),
            "{}",
            missing_note.message
        );

        let missing = list_capture_drawers(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("bar".to_string()),
        ))
        .expect_err("missing id");
        assert!(
            missing.message.contains("no task with block ID ^bar")
                && missing.message.contains("capture-tasks -r foo"),
            "{}",
            missing.message
        );

        let close = list_capture_drawers(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("ready-i".to_string()),
        ))
        .expect_err("close match");
        assert!(
            close.message.contains("did you mean ^ready-id"),
            "{}",
            close.message
        );

        let duplicate = list_capture_drawers(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("dup-id".to_string()),
        ))
        .expect_err("duplicate");
        assert!(
            duplicate.message.contains("appears 2 times"),
            "{}",
            duplicate.message
        );

        let not_a_task = list_capture_drawers(&request(
            temp.path(),
            "foo",
            ParentSelector::BlockId("plain-id".to_string()),
        ))
        .expect_err("not a task");
        assert!(
            not_a_task.message.contains("is not a task")
                && not_a_task.message.contains("Plain heading"),
            "{}",
            not_a_task.message
        );

        let stale = list_capture_drawers(&request(
            temp.path(),
            "foo",
            ParentSelector::TaskRef(TaskRef {
                line: 99,
                digest: "deadbeef".to_string(),
            }),
        ))
        .expect_err("stale");
        assert!(
            stale.message.contains("no longer in foo.md"),
            "{}",
            stale.message
        );

        let ambiguous = list_capture_drawers(&request(
            temp.path(),
            "foo",
            ParentSelector::TaskRef(TaskRef {
                line: 99,
                digest: same_digest,
            }),
        ))
        .expect_err("ambiguous");
        assert!(
            ambiguous.message.contains("matches more than one line"),
            "{}",
            ambiguous.message
        );
    }

    #[test]
    fn task_ref_resolves_a_task_without_a_block_id() {
        let temp = TempDir::new("bob-cli-capture-drawers-ref");
        write_settings(temp.path());
        let original =
            concat!("# Tasks\n", "- [ ] #task Parent\n", "\t- REQUIREMENTS\n",);
        write_file(&temp.path().join("foo.md"), original);
        let task_ref = task_ref_for(original, "Parent");

        let result = list_capture_drawers(&request(
            temp.path(),
            "foo",
            ParentSelector::TaskRef(task_ref),
        ))
        .expect("list by ref");

        assert!(result.block_id.is_none());
        assert_eq!(result.count, 1);
        assert_eq!(result.drawers[0].title, "REQUIREMENTS");
        assert_eq!(
            result.task_ref,
            task_ref_for(original, "Parent").to_string()
        );
    }

    #[test]
    fn request_validation_covers_route_and_exclusive_selectors() {
        let both = parse_request(&[
            "--route",
            "foo",
            "--block-id",
            "bar",
            "--task-ref",
            "1:abcd1234",
        ])
        .expect_err("both");
        assert_eq!(both.kind, CaptureDrawersErrorKind::Usage);
        assert!(
            both.message
                .contains("exactly one of --block-id or --task-ref"),
            "{}",
            both.message
        );

        let neither = parse_request(&["--route", "foo"]).expect_err("neither");
        assert!(
            neither
                .message
                .contains("exactly one of --block-id or --task-ref"),
            "{}",
            neither.message
        );

        let missing_route =
            parse_request(&["--block-id", "bar"]).expect_err("route");
        assert_eq!(missing_route.message, "--route is required");

        let invalid_route =
            parse_request(&["--route", "../bad", "--block-id", "bar"])
                .expect_err("invalid route");
        assert!(invalid_route.message.contains("must contain only"));

        let invalid_id =
            parse_request(&["--route", "foo", "--block-id", "bad.id"])
                .expect_err("invalid id");
        assert!(invalid_id.message.contains("--block-id must"));

        let invalid_ref =
            parse_request(&["--route", "foo", "--task-ref", "nope"])
                .expect_err("invalid ref");
        assert!(invalid_ref.message.contains("--task-ref must use"));
    }

    #[test]
    fn json_success_shape_and_key_order_are_stable() {
        let result = CaptureDrawersResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            route: "foo".to_string(),
            block_id: Some("bar".to_string()),
            task_ref: "2:abcd1234".to_string(),
            count: 1,
            drawers: vec![ListedDrawer {
                title: "REQUIREMENTS".to_string(),
                key: "REQUIREMENTS".to_string(),
                slug: "requirements".to_string(),
                marker: DrawerMarker::LegacyCaps,
                line: 3,
                entry_count: 1,
            }],
            recent: Vec::new(),
        };

        let raw = success_json(&result);
        assert_key_order(
            &raw,
            &[
                "\"ok\"",
                "\"schema_version\"",
                "\"route\"",
                "\"block_id\"",
                "\"ref\"",
                "\"count\"",
                "\"drawers\"",
            ],
        );
        let drawer_start = raw.find("\"drawers\"").expect("drawers");
        assert_key_order(
            &raw[drawer_start..],
            &[
                "\"title\"",
                "\"key\"",
                "\"slug\"",
                "\"marker\"",
                "\"line\"",
                "\"entry_count\"",
            ],
        );

        let value: serde_json::Value =
            serde_json::from_str(&raw).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["route"], "foo");
        assert_eq!(value["block_id"], "bar");
        assert_eq!(value["ref"], "2:abcd1234");
        assert_eq!(value["count"], 1);
        assert_eq!(value["drawers"][0]["title"], "REQUIREMENTS");
        assert_eq!(value["drawers"][0]["slug"], "requirements");
        assert_eq!(value["drawers"][0]["line"], 3);
        assert_eq!(value["drawers"][0]["key"], "REQUIREMENTS");
        assert_eq!(value["drawers"][0]["entry_count"], 1);
    }

    fn request(
        bob_dir: &Path,
        route: &str,
        parent: ParentSelector,
    ) -> CaptureDrawersRequest {
        CaptureDrawersRequest {
            bob_dir: bob_dir.to_path_buf(),
            route: route.to_ascii_lowercase(),
            parent,
        }
    }

    fn parse_request(
        args: &[&str],
    ) -> Result<CaptureDrawersRequest, CaptureDrawersError> {
        let matches = build_cli()
            .try_get_matches_from(
                std::iter::once(COMMAND_NAME.to_string())
                    .chain(args.iter().map(|arg| (*arg).to_string())),
            )
            .expect("clap parse");
        CaptureDrawersRequest::from_matches(&matches)
    }

    fn task_ref_for(contents: &str, description: &str) -> TaskRef {
        let settings = note_tasks::read_settings(Path::new("/nonexistent"));
        let scan = note_tasks::scan(contents, &settings);
        let task = scan
            .task_named(description)
            .unwrap_or_else(|| panic!("missing task {description}"));
        TaskRef::from_task(task)
    }

    fn write_settings(root: &Path) {
        write_file(
            &root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Todo","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"}
                ],
                "customStatuses": []
              }
            }"##,
        );
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("file parent"))
            .expect("create file parent");
        fs::write(path, contents).expect("write file");
    }

    fn assert_key_order(text: &str, needles: &[&str]) {
        let mut last = 0;
        for needle in needles {
            let position = text[last..].find(needle).unwrap_or_else(|| {
                panic!("expected `{needle}` after {last} in {text}")
            }) + last;
            last = position + needle.len();
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("failed to remove {}: {error}", self.path.display());
            }
        }
    }
}
