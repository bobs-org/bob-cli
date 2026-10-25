//! Fast relation-label catalog scan (`docs/task-relations.md` §16,
//! RV2).
//!
//! One row per key counts every accepted LINKS entry with that label
//! across every scanned note: open, closed, `done/`, daily notes,
//! nested projects, and references. The walk reuses the dependency
//! note-path set (every `.md` note including `done/`, minus
//! dot-directories and the always-excluded names), caches each parsed
//! note by modification time and size, and rescans changed notes.
//! Malformed LINKS lines are skipped with a bounded warning;
//! completion tolerates them, rename must not. This scan never calls
//! `capture_dependency_tasks::discover`.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::native::{
    capture_dependency_tasks, collect_done, env as bob_env,
    markdown::fenced_lines,
    task_dependencies::blocks::has_task_tag,
    task_relations::{parse::links_children_of, LinksLine},
};

use super::{
    count_label, sort_spellings, LabelRecord, LabelSpelling, MAX_LABEL_WARNINGS,
};

pub(crate) const CACHE_VERSION: u32 = 2;
const SNAPSHOT_TTL_SECS: u64 = 60;
/// Atomic-write `.tmp` files older than this are swept with the catalog.
const STALE_TMP_SECS: u64 = 10 * 60;

#[derive(Default, Serialize, Deserialize)]
struct ScanCache {
    version: u32,
    vault: String,
    #[serde(default)]
    last_scan_unix: u64,
    #[serde(default)]
    files: BTreeMap<String, CachedNote>,
    #[serde(default)]
    records: BTreeMap<String, LabelRecord>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedNote {
    mtime_ns: u64,
    size: u64,
    labels: Vec<LabelSpelling>,
    warnings: Vec<String>,
    malformed_count: usize,
}

/// Scan the vault into label rows in sorted path order, plus bounded
/// walk warnings.
pub(crate) fn load_scan(
    vault: &Path,
) -> io::Result<(BTreeMap<String, LabelRecord>, Vec<String>)> {
    let cache_path = cache_path(vault);
    let mut cache = fs::read_to_string(&cache_path)
        .ok()
        .and_then(|text| serde_json::from_str::<ScanCache>(&text).ok())
        .filter(|cache| {
            cache.version == CACHE_VERSION && cache.vault == vault_key(vault)
        })
        .unwrap_or_else(|| ScanCache {
            version: CACHE_VERSION,
            vault: vault_key(vault),
            last_scan_unix: 0,
            files: BTreeMap::new(),
            records: BTreeMap::new(),
            warnings: Vec::new(),
        });
    if cache.last_scan_unix > 0
        && now_unix().saturating_sub(cache.last_scan_unix) < SNAPSHOT_TTL_SECS
    {
        return Ok((cache.records, cache.warnings));
    }
    let (mut relative_paths, mut warnings) =
        capture_dependency_tasks::vault_note_paths(vault);
    relative_paths.sort();
    let previous = std::mem::take(&mut cache.files);
    let mut next = BTreeMap::new();
    let mut cache_dirty = true;
    let mut rows: BTreeMap<String, LabelRecord> = BTreeMap::new();
    let mut seen: Vec<String> = Vec::new();
    let mut malformed = 0usize;
    for relative in &relative_paths {
        let absolute = vault.join(relative);
        let Some((mtime_ns, size)) = fingerprint(&absolute) else {
            cache_dirty = true;
            push_warning(
                &mut warnings,
                &mut malformed,
                format!(
                    "skipping unreadable note {}",
                    display_relative(relative)
                ),
            );
            continue;
        };
        let key = display_relative(relative);
        let note = match previous.get(&key) {
            Some(note) if note.mtime_ns == mtime_ns && note.size == size => {
                note.clone()
            }
            _ => {
                cache_dirty = true;
                scan_note(&absolute, relative, mtime_ns, size)?
            }
        };
        warnings.extend(
            note.warnings
                .iter()
                .take(MAX_LABEL_WARNINGS.saturating_sub(malformed))
                .cloned(),
        );
        malformed = malformed.saturating_add(note.malformed_count);
        for spelling in &note.labels {
            for _ in 0..spelling.count {
                count_label(&mut rows, &mut seen, &spelling.name);
            }
        }
        next.insert(key, note);
    }
    if malformed > MAX_LABEL_WARNINGS {
        warnings.push(format!(
            "skipped {malformed} unreadable or malformed relation lines"
        ));
    }
    for record in rows.values_mut() {
        sort_spellings(record);
    }
    cache.files = next;
    cache.last_scan_unix = now_unix();
    cache.records = rows.clone();
    cache.warnings = warnings.clone();
    if cache_dirty {
        if let Ok(text) = serde_json::to_string(&cache) {
            let _ = collect_done::atomic_write(&cache_path, &text);
        }
        // Bound the cache the way the theme catalog does: one file per
        // vault would otherwise grow without bound. A failed sweep never
        // fails the scan, and the 60 s snapshot TTL already bounds how
        // often a full rescan (and so this sweep) runs.
        sweep_catalog_cache(&cache.vault);
    }
    Ok((rows, warnings))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn scan_note(
    absolute: &Path,
    relative: &Path,
    mtime_ns: u64,
    size: u64,
) -> io::Result<CachedNote> {
    let contents = match fs::read(absolute) {
        Ok(contents) => contents,
        Err(error) => {
            return Ok(CachedNote {
                mtime_ns,
                size,
                labels: Vec::new(),
                warnings: vec![format!(
                    "skipping unreadable note {}: {error}",
                    display_relative(relative)
                )],
                malformed_count: 1,
            });
        }
    };
    // Byte-prefilter before parsing: most notes carry no LINKS.
    if !contents.windows(7).any(|window| window == b"**LINKS") {
        return Ok(CachedNote {
            mtime_ns,
            size,
            labels: Vec::new(),
            warnings: Vec::new(),
            malformed_count: 0,
        });
    }
    let Ok(text) = std::str::from_utf8(&contents) else {
        return Ok(CachedNote {
            mtime_ns,
            size,
            labels: Vec::new(),
            warnings: vec![format!(
                "skipping non-UTF8 note {}",
                display_relative(relative)
            )],
            malformed_count: 1,
        });
    };
    let lines: Vec<&str> = text.lines().collect();
    let fenced = fenced_lines(&lines, 0..lines.len());
    let mut labels: Vec<LabelSpelling> = Vec::new();
    let mut warnings = Vec::new();
    let mut malformed = 0;
    for (task_line, line) in lines.iter().enumerate() {
        if !has_task_tag(line) || fenced.contains(&task_line) {
            continue;
        }
        for child in links_children_of(&lines, &fenced, task_line) {
            match child.parsed {
                LinksLine::Accepted { entries, .. } => {
                    for entry in &entries {
                        if let Some(label) = entry.label.as_deref() {
                            if let Some(spelling) = labels
                                .iter_mut()
                                .find(|item| item.name == label)
                            {
                                spelling.count =
                                    spelling.count.saturating_add(1);
                            } else {
                                labels.push(LabelSpelling {
                                    name: label.to_string(),
                                    count: 1,
                                });
                            }
                        }
                    }
                }
                LinksLine::Malformed => push_warning(
                    &mut warnings,
                    &mut malformed,
                    format!(
                        "skipping malformed LINKS line {}:{}",
                        display_relative(relative),
                        child.line_index + 1
                    ),
                ),
                LinksLine::Empty | LinksLine::NotALine => {}
            }
        }
    }
    Ok(CachedNote {
        mtime_ns,
        size,
        labels,
        warnings,
        malformed_count: malformed,
    })
}

fn fingerprint(path: &Path) -> Option<(u64, u64)> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((u64::try_from(modified.as_nanos()).ok()?, metadata.len()))
}

pub(crate) fn relations_dir() -> PathBuf {
    bob_env::bob_cli_cache_dir().join("relations")
}

pub(crate) fn cache_path(vault: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = vault_key(vault);
    let digest = hex::encode(Sha256::digest(key.as_bytes()));
    relations_dir().join(format!("catalog-v{CACHE_VERSION}-{digest}.json"))
}

/// Sweep sibling cache files after a full rescan stores, modeled on the
/// theme catalog's `sweep_orphaned_files`:
/// - remove a `catalog-v{CACHE_VERSION}-<64 hex>.json` whose stored
///   `vault` path is no longer a directory (except the vault just
///   written);
/// - remove `catalog-v<older version>-<64 hex>.json` files; preserve
///   future versions and malformed catalog names;
/// - remove owned atomic-write `.catalog-v<current-or-older>-<64
///   hex>.json.<pid>.tmp` files older than 10 minutes; preserve
///   future-version, malformed, and unrelated `.tmp` names;
/// - leave unparseable current-version files and unrelated files alone.
/// Every I/O failure is ignored: the sweep never fails a scan.
fn parse_catalog_version_and_hash(
    version_and_hash: &str,
) -> Option<(u32, &str)> {
    let (version_text, hash) = version_and_hash.split_once('-')?;
    let version: u32 = version_text.parse().ok()?;
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((version, hash))
}

/// Recognize the atomic-write name produced by
/// `collect_done::temporary_write_path` for a catalog file:
/// `.catalog-v<version>-<64 hex>.json.<pid>.tmp` with a numeric PID.
fn owned_temp_version(name: &str) -> Option<u32> {
    let inner = name.strip_prefix('.')?.strip_suffix(".tmp")?;
    let (catalog_part, pid_text) = inner.rsplit_once('.')?;
    if pid_text.is_empty()
        || !pid_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let _: u32 = pid_text.parse().ok()?;
    let rest = catalog_part
        .strip_suffix(".json")?
        .strip_prefix("catalog-v")?;
    let (version, _) = parse_catalog_version_and_hash(rest)?;
    Some(version)
}

fn sweep_catalog_cache(keep_vault: &str) {
    let Ok(entries) = fs::read_dir(relations_dir()) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if let Some(rest) = name
            .strip_prefix("catalog-v")
            .and_then(|name| name.strip_suffix(".json"))
        {
            sweep_catalog_file(&path, rest, keep_vault);
            continue;
        }
        if let Some(version) = owned_temp_version(name) {
            if version > CACHE_VERSION {
                continue;
            }
            let stale = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age.as_secs() >= STALE_TMP_SECS);
            if stale {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn sweep_catalog_file(path: &Path, version_and_hash: &str, keep_vault: &str) {
    let Some((version, _hash)) =
        parse_catalog_version_and_hash(version_and_hash)
    else {
        return;
    };
    if version < CACHE_VERSION {
        // An obsolete-version file matches the generated catalog basename,
        // so it goes even when its body no longer parses.
        let _ = fs::remove_file(path);
        return;
    }
    if version > CACHE_VERSION {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let Ok(stored) = serde_json::from_str::<ScanCache>(&text) else {
        // Unparseable current-version files stay: only the exact
        // vanished-vault and stale-version patterns are removed.
        return;
    };
    if stored.version != CACHE_VERSION
        || stored.vault == keep_vault
        || Path::new(&stored.vault).is_dir()
    {
        return;
    }
    let _ = fs::remove_file(path);
}

pub(crate) fn invalidate(vault: &Path) {
    let _ = fs::remove_file(cache_path(vault));
}

pub(crate) fn vault_key(vault: &Path) -> String {
    fs::canonicalize(vault)
        .unwrap_or_else(|_| vault.to_path_buf())
        .display()
        .to_string()
}

fn push_warning(
    warnings: &mut Vec<String>,
    malformed: &mut usize,
    warning: String,
) {
    *malformed += 1;
    if *malformed <= MAX_LABEL_WARNINGS {
        warnings.push(warning);
    }
}

fn display_relative(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}
