use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::native::{collect_done, env as bob_env};

use super::{scan::FileSummary, ThemeRecord};

pub(crate) const SWEEP_INTERVAL_SECS: u64 = 60;
const CACHE_VERSION: u32 = 1;
const KNOWN_KEYS_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CacheFile {
    #[serde(default)]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) vaults: BTreeMap<String, VaultCache>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct VaultCache {
    #[serde(default)]
    pub(crate) last_sweep_unix: u64,
    #[serde(default)]
    pub(crate) years: BTreeMap<String, YearFingerprint>,
    #[serde(default)]
    pub(crate) files: BTreeMap<String, CachedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct YearFingerprint {
    pub(crate) mtime_ns: u64,
    #[serde(default)]
    pub(crate) size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedFile {
    pub(crate) mtime_ns: u64,
    pub(crate) size: u64,
    pub(crate) summary: FileSummary,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KnownKeysFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    vaults: BTreeMap<String, KnownKeysVault>,
}

#[derive(Debug, Serialize, Deserialize)]
struct KnownKeysVault {
    today: String,
    today_file: Option<FileFingerprint>,
    last_sweep_unix: u64,
    years: BTreeMap<String, YearFingerprint>,
    keys: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileFingerprint {
    mtime_ns: u64,
    size: u64,
}

pub(crate) fn cache_path() -> PathBuf {
    bob_env::bob_cli_cache_dir()
        .join("themes")
        .join("catalog-v1.json")
}

fn known_keys_path() -> PathBuf {
    bob_env::bob_cli_cache_dir()
        .join("themes")
        .join("known-keys-v1.json")
}

fn load_known_keys_file() -> KnownKeysFile {
    let Ok(text) = fs::read_to_string(known_keys_path()) else {
        return KnownKeysFile {
            version: KNOWN_KEYS_VERSION,
            vaults: BTreeMap::new(),
        };
    };
    match serde_json::from_str::<KnownKeysFile>(&text) {
        Ok(cache) if cache.version == KNOWN_KEYS_VERSION => cache,
        _ => KnownKeysFile {
            version: KNOWN_KEYS_VERSION,
            vaults: BTreeMap::new(),
        },
    }
}

fn store_known_keys_file(cache: &KnownKeysFile) {
    let path = known_keys_path();
    let Ok(text) = serde_json::to_string(cache) else {
        return;
    };
    let _ = collect_done::atomic_write(&path, &text);
}

/// Return the cached history membership when its age and vault fingerprints
/// meet the catalog's normal sweep rules. Today's note is checked every time.
pub(crate) fn load_known_keys(
    vault: &Path,
    today: NaiveDate,
    years: &BTreeMap<String, (u64, u64)>,
) -> Option<BTreeSet<String>> {
    let key = vault_key(vault);
    let cache = load_known_keys_file();
    let cached = cache.vaults.get(&key)?;
    if cached.today != today.to_string()
        || now_unix().saturating_sub(cached.last_sweep_unix)
            >= SWEEP_INTERVAL_SECS
        || cached.years.len() != years.len()
        || years.iter().any(|(year, (mtime_ns, size))| {
            cached.years.get(year).is_none_or(|fingerprint| {
                fingerprint.mtime_ns != *mtime_ns || fingerprint.size != *size
            })
        })
    {
        return None;
    }
    let today_path = vault
        .join(today.format("%Y").to_string())
        .join(format!("{}.md", today.format("%Y%m%d")));
    let current_today = fingerprint(&today_path)
        .map(|(mtime_ns, size)| FileFingerprint { mtime_ns, size });
    if cached.today_file != current_today {
        return None;
    }
    Some(cached.keys.clone())
}

pub(crate) fn store_known_keys(
    vault: &Path,
    today: NaiveDate,
    last_sweep_unix: u64,
    years: &BTreeMap<String, YearFingerprint>,
    keys: BTreeSet<String>,
) {
    let key = vault_key(vault);
    let today_path = vault
        .join(today.format("%Y").to_string())
        .join(format!("{}.md", today.format("%Y%m%d")));
    let today_file = fingerprint(&today_path)
        .map(|(mtime_ns, size)| FileFingerprint { mtime_ns, size });
    let mut cache = load_known_keys_file();
    cache.version = KNOWN_KEYS_VERSION;
    cache.vaults.insert(
        key,
        KnownKeysVault {
            today: today.to_string(),
            today_file,
            last_sweep_unix,
            years: years.clone(),
            keys,
        },
    );
    store_known_keys_file(&cache);
}

pub(crate) fn vault_key(vault: &Path) -> String {
    fs::canonicalize(vault)
        .unwrap_or_else(|_| vault.to_path_buf())
        .display()
        .to_string()
}

// The completion hot path needs only nonempty past-day summaries. Keep a
// disposable per-vault view so every keystroke need not decode/rewrite years
// of empty notes (or other vaults). The full cache remains authoritative.
#[derive(Serialize, Deserialize)]
struct CompletionCache {
    version: u32,
    vault: String,
    today: String,
    source: FileFingerprint,
    history: VaultCache,
    records: BTreeMap<String, ThemeRecord>,
    days: BTreeMap<i32, BTreeSet<String>>,
    warnings: Vec<String>,
}

fn completion_path(vault: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = hex::encode(Sha256::digest(vault_key(vault).as_bytes()));
    bob_env::bob_cli_cache_dir()
        .join("themes")
        .join(format!("completion-v1-{key}.json"))
}

fn source_fingerprint() -> Option<FileFingerprint> {
    fingerprint(&cache_path())
        .map(|(mtime_ns, size)| FileFingerprint { mtime_ns, size })
}

pub(crate) fn load_completion(
    vault: &Path,
    today: NaiveDate,
    years: &BTreeMap<String, (u64, u64)>,
) -> Option<(
    BTreeMap<String, ThemeRecord>,
    BTreeMap<NaiveDate, BTreeSet<String>>,
    Vec<String>,
)> {
    let text = fs::read_to_string(completion_path(vault)).ok()?;
    let cached: CompletionCache = serde_json::from_str(&text).ok()?;
    if cached.version != 1
        || cached.vault != vault_key(vault)
        || cached.today != today.to_string()
        || source_fingerprint()? != cached.source
        || needs_sweep(Some(&cached.history), years, false)
    {
        return None;
    }
    let days = cached
        .days
        .into_iter()
        .map(|(day, keys)| {
            Some((NaiveDate::from_num_days_from_ce_opt(day)?, keys))
        })
        .collect::<Option<BTreeMap<_, _>>>()?;
    Some((cached.records, days, cached.warnings))
}

pub(crate) fn store_completion(
    vault: &Path,
    today: NaiveDate,
    history: VaultCache,
    records: BTreeMap<String, ThemeRecord>,
    days: BTreeMap<NaiveDate, BTreeSet<String>>,
    warnings: &[String],
) {
    let Some(source) = source_fingerprint() else {
        return;
    };
    let cached = CompletionCache {
        version: 1,
        vault: vault_key(vault),
        today: today.to_string(),
        source,
        history,
        records,
        days: days
            .into_iter()
            .map(|(day, keys)| (day.num_days_from_ce(), keys))
            .collect(),
        warnings: warnings.to_vec(),
    };
    if let Ok(text) = serde_json::to_string(&cached) {
        let _ = collect_done::atomic_write(&completion_path(vault), &text);
    }
}

pub(crate) fn load() -> CacheFile {
    let path = cache_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return CacheFile {
            version: CACHE_VERSION,
            vaults: BTreeMap::new(),
        };
    };
    match serde_json::from_str::<CacheFile>(&text) {
        Ok(cache) if cache.version == CACHE_VERSION => cache,
        _ => CacheFile {
            version: CACHE_VERSION,
            vaults: BTreeMap::new(),
        },
    }
}

pub(crate) fn store(cache: &CacheFile) {
    let path = cache_path();
    let Ok(text) = serde_json::to_string(cache) else {
        return;
    };
    let _ = collect_done::atomic_write(&path, &text);
}

pub(crate) fn fingerprint(path: &Path) -> Option<(u64, u64)> {
    let metadata = fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Some((
        u64::try_from(mtime.as_nanos()).unwrap_or(u64::MAX),
        metadata.len(),
    ))
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn needs_sweep(
    cached: Option<&VaultCache>,
    years: &BTreeMap<String, (u64, u64)>,
    fresh: bool,
) -> bool {
    if fresh {
        return true;
    }
    let Some(cached) = cached else {
        return true;
    };
    if now_unix().saturating_sub(cached.last_sweep_unix) >= SWEEP_INTERVAL_SECS
    {
        return true;
    }
    if cached.years.len() != years.len() {
        return true;
    }
    years.iter().any(|(year, (mtime_ns, size))| {
        cached.years.get(year).is_none_or(|fingerprint| {
            fingerprint.mtime_ns != *mtime_ns || fingerprint.size != *size
        })
    })
}

#[allow(dead_code)]
pub(crate) fn invalidate_vault(vault: &Path) {
    let key = vault_key(vault);
    let mut cache = load();
    cache.vaults.remove(&key);
    cache.version = CACHE_VERSION;
    store(&cache);
    let _ = fs::remove_file(known_keys_path());
    let _ = fs::remove_file(completion_path(vault));
}

/// Cache dates as integers to avoid reparsing calendar strings on each keypress.
pub(super) mod date {
    use chrono::{Datelike, NaiveDate};
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(
        date: &NaiveDate,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_i32(date.num_days_from_ce())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<NaiveDate, D::Error> {
        NaiveDate::from_num_days_from_ce_opt(i32::deserialize(deserializer)?)
            .ok_or_else(|| serde::de::Error::custom("invalid cached date"))
    }
}
