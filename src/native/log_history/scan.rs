use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::native::{capture_dependency_tasks, collect_done, env as bob_env};

use super::{
    parse::{self, LogUse, NoteScan},
    rank, Excluded, LogCatalog, LogKind, Phrase,
};

const CACHE_VERSION: u32 = 1;
const STALE_TMP_SECS: u64 = 10 * 60;

#[derive(Debug, Default, Serialize, Deserialize)]
struct ScanCache {
    version: u32,
    vault: String,
    last_scan_unix: u64,
    #[serde(default)]
    files: BTreeMap<String, CachedNote>,
    #[serde(default)]
    catalog: LogCatalog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedNote {
    mtime_ns: u128,
    size: u64,
    scan: NoteScan,
}

pub(crate) fn load(
    vault: &Path,
    max_age: Option<Duration>,
) -> io::Result<LogCatalog> {
    if !vault.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("vault directory does not exist: {}", vault.display()),
        ));
    }
    // The walker reports nested directory failures as warnings. Fail if
    // even the vault root cannot be opened.
    fs::read_dir(vault)?;
    let vault_key = vault_key(vault);
    let path = cache_path(&vault_key);
    let mut cache = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<ScanCache>(&text).ok())
        .filter(|cache| {
            cache.version == CACHE_VERSION && cache.vault == vault_key
        })
        .unwrap_or_else(|| ScanCache {
            version: CACHE_VERSION,
            vault: vault_key.clone(),
            ..ScanCache::default()
        });
    if let Some(max_age) = max_age
        && cache.last_scan_unix > 0
        && now_unix().saturating_sub(cache.last_scan_unix) < max_age.as_secs()
    {
        return Ok(cache.catalog);
    }

    let (mut paths, mut warnings) =
        capture_dependency_tasks::vault_note_paths(vault);
    paths.sort();
    let mut old_files = std::mem::take(&mut cache.files);
    let mut files = BTreeMap::new();
    let mut all_uses = Vec::new();
    let mut excluded_by_note: BTreeMap<LogKind, Excluded> = BTreeMap::new();
    for relative in paths {
        let absolute = vault.join(&relative);
        let name = capture_dependency_tasks::display_path(&relative);
        let Some((mtime_ns, size)) = fingerprint(&absolute) else {
            warnings.push(format!("skipping unreadable note {name}"));
            continue;
        };
        let cached = match old_files.remove(&name) {
            Some(entry) if entry.mtime_ns == mtime_ns && entry.size == size => {
                entry
            }
            _ => {
                let scan = match fs::read(&absolute) {
                    Ok(contents) => match String::from_utf8(contents) {
                        Ok(text) => {
                            parse::scan_note(&text, Local::now().date_naive())
                        }
                        Err(_) => NoteScan {
                            excluded: BTreeMap::new(),
                            warnings: vec![format!(
                                "skipping non-UTF8 note {name}"
                            )],
                            ..NoteScan::default()
                        },
                    },
                    Err(error) => NoteScan {
                        excluded: BTreeMap::new(),
                        warnings: vec![format!(
                            "skipping unreadable note {name}: {error}"
                        )],
                        ..NoteScan::default()
                    },
                };
                CachedNote {
                    mtime_ns,
                    size,
                    scan,
                }
            }
        };
        all_uses.extend(cached.scan.uses.iter().cloned());
        for (kind, counts) in &cached.scan.excluded {
            add_excluded(excluded_by_note.entry(*kind).or_default(), counts);
        }
        warnings.extend(cached.scan.warnings.iter().cloned());
        files.insert(name, cached);
    }
    warnings.truncate(8);
    let catalog = aggregate(
        all_uses,
        excluded_by_note,
        warnings,
        Local::now().naive_local(),
    );
    cache.files = files;
    cache.catalog = catalog.clone();
    cache.last_scan_unix = now_unix();
    if let Ok(text) = serde_json::to_string(&cache)
        && collect_done::atomic_write(&path, &text).is_ok()
    {
        sweep_stale_temps(path.parent().unwrap_or(Path::new(".")));
    }
    Ok(catalog)
}

fn aggregate(
    uses: Vec<LogUse>,
    mut excluded: BTreeMap<LogKind, Excluded>,
    warnings: Vec<String>,
    now: NaiveDateTime,
) -> LogCatalog {
    let mut seen = BTreeSet::new();
    let mut groups: BTreeMap<(LogKind, String), PhraseBuild> = BTreeMap::new();
    for usage in uses {
        let identity = (usage.kind, usage.stamp.clone(), usage.key.clone());
        if !seen.insert(identity) {
            excluded.entry(usage.kind).or_default().copies += 1;
            continue;
        }
        let group = groups.entry((usage.kind, usage.key.clone())).or_default();
        group.uses += 1;
        group.score += score_use(usage.kind, &usage, now);
        if usage.sort_key > group.sort_key {
            group.text = usage.text;
            group.last_used = usage.last_used;
            group.sort_key = usage.sort_key;
        } else if usage.sort_key == group.sort_key && usage.text < group.text {
            group.text = usage.text;
        }
    }

    let mut by_kind: BTreeMap<LogKind, Vec<(String, PhraseBuild)>> =
        BTreeMap::new();
    for ((kind, key), phrase) in groups {
        by_kind.entry(kind).or_default().push((key, phrase));
    }
    let mut phrases = BTreeMap::new();
    for kind in LogKind::ALL {
        let rows = by_kind.entry(kind).or_default();
        rows.sort_by(|(left_key, left), (right_key, right)| {
            right
                .sort_key
                .cmp(&left.sort_key)
                .then_with(|| right.score.total_cmp(&left.score))
                .then_with(|| left_key.cmp(right_key))
        });
        phrases.insert(
            kind,
            rows.iter()
                .enumerate()
                .map(|(recency_rank, (key, row))| Phrase {
                    text: row.text.clone(),
                    key: key.clone(),
                    uses: row.uses,
                    last_used: row.last_used.clone(),
                    score: row.score,
                    recency_rank,
                })
                .collect(),
        );
        excluded.entry(kind).or_default();
    }
    LogCatalog {
        phrases,
        excluded,
        warnings,
        scanned_unix: now_unix(),
    }
}

#[derive(Debug, Default)]
struct PhraseBuild {
    text: String,
    last_used: String,
    sort_key: String,
    uses: u64,
    score: f64,
}

fn score_use(kind: LogKind, usage: &LogUse, now: NaiveDateTime) -> f64 {
    let age_days = if kind == LogKind::Work {
        let datetime =
            NaiveDateTime::parse_from_str(&usage.sort_key, "%Y-%m-%d %H:%M")
                .or_else(|_| {
                    NaiveDate::parse_from_str(&usage.last_used, "%Y-%m-%d").map(
                        |date| date.and_hms_opt(0, 0, 0).expect("midnight"),
                    )
                })
                .unwrap_or(now);
        now.signed_duration_since(datetime).num_seconds().max(0) as f64
            / 86_400.0
    } else {
        NaiveDate::parse_from_str(&usage.last_used, "%Y-%m-%d")
            .map(|date| {
                now.date().signed_duration_since(date).num_days().max(0) as f64
            })
            .unwrap_or(0.0)
    };
    rank::use_score(age_days)
}

fn add_excluded(target: &mut Excluded, source: &Excluded) {
    target.machine = target.machine.saturating_add(source.machine);
    target.blank = target.blank.saturating_add(source.blank);
    target.too_long = target.too_long.saturating_add(source.too_long);
    target.copies = target.copies.saturating_add(source.copies);
    target.unparsed = target.unparsed.saturating_add(source.unparsed);
}

fn fingerprint(path: &Path) -> Option<(u128, u64)> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((modified.as_nanos(), metadata.len()))
}

fn vault_key(vault: &Path) -> String {
    let path = vault.canonicalize().unwrap_or_else(|_| vault.to_path_buf());
    path.to_string_lossy().into_owned()
}

fn cache_path(vault: &str) -> PathBuf {
    let digest = hex::encode(Sha256::digest(vault.as_bytes()));
    bob_env::bob_cli_cache_dir()
        .join("log-history")
        .join(format!("catalog-v{CACHE_VERSION}-{digest}.json"))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sweep_stale_temps(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.file_name().and_then(|name| name.to_str()).is_some_and(
            |name| name.starts_with(".catalog-v") && name.ends_with(".tmp"),
        ) {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > Duration::from_secs(STALE_TMP_SECS));
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}
