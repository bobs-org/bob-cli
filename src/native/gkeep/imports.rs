//! Vault-resident GKeep import history.
//!
//! `pull` no longer emits `%%gkeep:…%%` markers. Import evidence lives in
//! versioned JSON files under `.bob/gkeep/imports/<transaction-id>.json`
//! inside the vault, so vault Git sync carries history between hosts with
//! no local state. Legacy markers remain compatibility inputs.
//!
//! Schema v1: one file per task-write batch with a shared transaction id
//! and vault-relative destination, per-entry `prepared`/`verified`
//! states, before/after file SHA-256 values, and baseline occurrence
//! counts for identical-rendering verification. Prepared entries hold the
//! intended task block; verified receipts are compact (ids, fingerprints,
//! source URLs, original paths, initial block digests, destination
//! digest) with the intended text removed. Completed entries never change
//! and fully completed files are immutable.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Import-store schema version.
pub(super) const SCHEMA_VERSION: u32 = 1;

/// Vault-relative import-store directory.
pub(super) const IMPORTS_DIR: &str = ".bob/gkeep/imports";

/// Per-entry state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum EntryState {
    Prepared,
    Verified,
}

/// One import entry: prepared (with intended block) or verified receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ImportEntry {
    pub(super) id: String,
    pub(super) fp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) url: Option<String>,
    /// Vault-relative destination at import time (e.g. `gkeep_inbox.md`).
    pub(super) path: String,
    /// Digest of the rendered marker-free block at prepare time.
    pub(super) block_digest: String,
    /// Intended marker-free block; `None` once verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) intended: Option<String>,
    pub(super) state: EntryState,
    /// SHA-256 of the destination file after the verified write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) dest_digest: Option<String>,
}

/// One batch file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TransactionFile {
    pub(super) schema_version: u32,
    pub(super) transaction_id: String,
    /// Vault-relative destination (e.g. `gkeep_inbox.md`).
    pub(super) destination: String,
    /// Hex SHA-256 of the destination before the batch.
    pub(super) before_sha256: String,
    /// Hex SHA-256 of the intended destination after the batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) after_sha256: Option<String>,
    /// Baseline task-boundary occurrence counts by block digest.
    #[serde(default)]
    pub(super) baseline_counts: BTreeMap<String, usize>,
    pub(super) entries: Vec<ImportEntry>,
}

/// Actionable import-store failure.
#[derive(Debug, Clone)]
pub(super) struct ImportsError {
    pub(super) message: String,
}

impl ImportsError {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for ImportsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<ImportsError> for io::Error {
    fn from(error: ImportsError) -> io::Error {
        io::Error::other(error.message)
    }
}

/// Vault store directory.
pub(super) fn imports_dir(bob_dir: &Path) -> PathBuf {
    bob_dir.join(IMPORTS_DIR)
}

/// Unique transaction id per batch: nanos + pid + counter.
pub(super) fn new_transaction_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut hasher = Sha256::new();
    hasher.update(format!("{nanos}-{pid}-{count}").as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

/// Hex SHA-256 of bytes.
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Digest of a rendered marker-free block (LF-normalized).
pub(super) fn block_digest(markdown: &str) -> String {
    sha256_hex(markdown.replace("\r\n", "\n").as_bytes())
}

/// Validate a vault-relative path: non-empty, relative, no `..`, no
/// absolute, no symlink escape via components.
fn validate_relative_path(value: &str) -> Result<String, ImportsError> {
    if value.trim().is_empty() {
        return Err(ImportsError::new(
            "import record has an empty path; repair or remove the file before pulling"
                .to_string(),
        ));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(ImportsError::new(format!(
            "import record escapes the vault: `{value}` is absolute; repair or remove the file before pulling"
        )));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => {
                return Err(ImportsError::new(format!(
                    "import record escapes the vault: `{value}` leaves the vault; repair or remove the file before pulling"
                )));
            }
        }
    }
    // Normalized forward-slash form for comparison.
    Ok(value.replace('\\', "/"))
}

fn validate_hex_digest(value: &str, what: &str) -> Result<(), ImportsError> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ImportsError::new(format!(
            "import record has an invalid {what} `{value}`; repair or remove the file before pulling"
        )));
    }
    Ok(())
}

fn validate_fp(value: &str) -> Result<(), ImportsError> {
    if value.len() != 12
        || !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(ImportsError::new(format!(
            "import record has an invalid fingerprint `{value}`; repair or remove the file before pulling"
        )));
    }
    Ok(())
}

/// Validate one loaded transaction file.
fn validate_transaction(
    file: &TransactionFile,
    source: &Path,
) -> Result<(), ImportsError> {
    if file.schema_version != SCHEMA_VERSION {
        return Err(ImportsError::new(format!(
            "unsupported import-store version {} in {}; upgrade bob before pulling",
            file.schema_version,
            source.display()
        )));
    }
    if file.transaction_id.trim().is_empty() {
        return Err(ImportsError::new(format!(
            "import record {} has an empty transaction id; repair or remove the file before pulling",
            source.display()
        )));
    }
    validate_relative_path(&file.destination).map(|_| ())?;
    validate_hex_digest(&file.before_sha256, "before_sha256")?;
    if let Some(after) = file.after_sha256.as_deref() {
        validate_hex_digest(after, "after_sha256")?;
    }
    if file.entries.is_empty() {
        return Err(ImportsError::new(format!(
            "import record {} has no entries; repair or remove the file before pulling",
            source.display()
        )));
    }
    for entry in &file.entries {
        if entry.id.trim().is_empty() {
            return Err(ImportsError::new(format!(
                "import record {} has an empty Keep id; repair or remove the file before pulling",
                source.display()
            )));
        }
        validate_fp(&entry.fp)?;
        validate_relative_path(&entry.path).map(|_| ())?;
        validate_hex_digest(&entry.block_digest, "block_digest")?;
        match entry.state {
            EntryState::Prepared => {
                let Some(intended) = entry.intended.as_deref() else {
                    return Err(ImportsError::new(format!(
                        "import record {} has a prepared entry without intended block; repair or remove the file before pulling",
                        source.display()
                    )));
                };
                if block_digest(intended) != entry.block_digest {
                    return Err(ImportsError::new(format!(
                        "import record {} has a prepared entry whose block digest mismatches; repair or remove the file before pulling",
                        source.display()
                    )));
                }
                if entry.dest_digest.is_some() {
                    return Err(ImportsError::new(format!(
                        "import record {} has a prepared entry with a destination digest; repair or remove the file before pulling",
                        source.display()
                    )));
                }
            }
            EntryState::Verified => {
                if entry.intended.is_some() {
                    return Err(ImportsError::new(format!(
                        "import record {} has a verified entry that still carries intended text; repair or remove the file before pulling",
                        source.display()
                    )));
                }
                let Some(dest) = entry.dest_digest.as_deref() else {
                    return Err(ImportsError::new(format!(
                        "import record {} has a verified entry without a destination digest; repair or remove the file before pulling",
                        source.display()
                    )));
                };
                validate_hex_digest(dest, "dest_digest")?;
            }
        }
    }
    Ok(())
}

/// Read every store file; a genuinely absent store is valid.
///
/// Malformed, unreadable, or unsupported files are an actionable error:
/// history must never be silently lost before task writes or archives.
pub(super) fn read_all(
    bob_dir: &Path,
) -> Result<Vec<(PathBuf, TransactionFile)>, ImportsError> {
    let dir = imports_dir(bob_dir);
    let read_dir = match fs::read_dir(&dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(ImportsError::new(format!(
                "read the gkeep import store {}: {error}; repair permissions before pulling",
                dir.display()
            )));
        }
        Ok(dir) => dir,
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|error| {
            ImportsError::new(format!(
                "read the gkeep import store {}: {error}; repair permissions before pulling",
                dir.display()
            ))
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            paths.push(path);
        }
    }
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let bytes = fs::read(&path).map_err(|error| {
            ImportsError::new(format!(
                "read the gkeep import store {}: {error}; repair or remove the file before pulling",
                path.display()
            ))
        })?;
        let text = String::from_utf8(bytes).map_err(|_| {
            ImportsError::new(format!(
                "import record {} is not valid UTF-8; repair or remove the file before pulling",
                path.display()
            ))
        })?;
        let file: TransactionFile = serde_json::from_str(&text)
            .map_err(|error| {
                ImportsError::new(format!(
                    "import record {} is malformed ({error}); repair or remove the file before pulling",
                    path.display()
                ))
            })?;
        validate_transaction(&file, &path)?;
        // Reject symlink escapes: the file itself must not be a symlink
        // leaving the vault (IDs never become filename components, so the
        // only path risk is the store file itself).
        if fs::symlink_metadata(&path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(ImportsError::new(format!(
                "import record {} is a symlink; remove it before pulling",
                path.display()
            )));
        }
        out.push((path, file));
    }
    Ok(out)
}

/// Whether every entry in the file is verified.
pub(super) fn is_completed(file: &TransactionFile) -> bool {
    file.entries
        .iter()
        .all(|entry| entry.state == EntryState::Verified)
}

/// Verified `(id, fp)` pairs across the store.
pub(super) fn verified_pairs(
    files: &[(PathBuf, TransactionFile)],
) -> BTreeSet<(String, String)> {
    let mut set = BTreeSet::new();
    for (_, file) in files {
        for entry in &file.entries {
            if entry.state == EntryState::Verified {
                set.insert((entry.id.clone(), entry.fp.clone()));
            }
        }
    }
    set
}

/// Verified ids across the store (any revision).
pub(super) fn verified_ids(
    files: &[(PathBuf, TransactionFile)],
) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for (_, file) in files {
        for entry in &file.entries {
            if entry.state == EntryState::Verified {
                set.insert(entry.id.clone());
            }
        }
    }
    set
}

/// Whether a verified receipt covers the exact `(id, fp)` pair.
pub(super) fn has_verified(
    files: &[(PathBuf, TransactionFile)],
    id: &str,
    fp: &str,
) -> bool {
    files.iter().any(|(_, file)| {
        file.entries.iter().any(|entry| {
            entry.state == EntryState::Verified
                && entry.id == id
                && entry.fp == fp
        })
    })
}

/// Whether any verified receipt knows `id`.
pub(super) fn has_verified_id(
    files: &[(PathBuf, TransactionFile)],
    id: &str,
) -> bool {
    files.iter().any(|(_, file)| {
        file.entries
            .iter()
            .any(|entry| entry.state == EntryState::Verified && entry.id == id)
    })
}

/// Unfinished (prepared-entry) transactions.
pub(super) fn unfinished<'a>(
    files: &'a [(PathBuf, TransactionFile)],
) -> Vec<&'a (PathBuf, TransactionFile)> {
    files
        .iter()
        .filter(|(_, file)| {
            file.entries
                .iter()
                .any(|entry| entry.state == EntryState::Prepared)
        })
        .collect()
}

/// Atomically persist a prepared transaction: same-dir temp, fsync,
/// atomic rename, directory fsync.
pub(super) fn persist_prepared(
    bob_dir: &Path,
    file: &TransactionFile,
) -> Result<PathBuf, ImportsError> {
    validate_transaction(file, Path::new("<new>"))?;
    let dir = imports_dir(bob_dir);
    fs::create_dir_all(&dir).map_err(|error| {
        ImportsError::new(format!(
            "create the gkeep import store {}: {error}",
            dir.display()
        ))
    })?;
    let name = format!("{}.json", file.transaction_id);
    // Transaction ids are JSON data, never raw filename components beyond
    // this hex charset; reject anything else defensively.
    if !file
        .transaction_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ImportsError::new(
            "refusing to persist an import transaction with a non-file-safe id"
                .to_string(),
        ));
    }
    let dest = dir.join(name);
    write_atomic(&dest, &serde_json::to_string_pretty(file).expect("json"))?;
    Ok(dest)
}

/// Atomically replace one transaction file after verification: only the
/// successful entries become compact verified receipts; failed entries
/// keep their prepared evidence and never inherit a sibling's state.
pub(super) fn persist_verified(
    tx_path: &Path,
    file: &TransactionFile,
) -> Result<(), ImportsError> {
    validate_transaction(file, tx_path)?;
    write_atomic(tx_path, &serde_json::to_string_pretty(file).expect("json"))
}

fn write_atomic(dest: &Path, contents: &str) -> Result<(), ImportsError> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!(
        ".{}.tmp.{}",
        dest.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "imports".to_string()),
        std::process::id()
    );
    let tmp = parent.join(tmp_name);
    fs::write(&tmp, contents).map_err(|error| {
        ImportsError::new(format!(
            "write the gkeep import store {}: {error}",
            dest.display()
        ))
    })?;
    File::open(&tmp)
        .and_then(|f| f.sync_all())
        .map_err(|error| {
            ImportsError::new(format!(
                "sync the gkeep import store {}: {error}",
                dest.display()
            ))
        })?;
    fs::rename(&tmp, dest).map_err(|error| {
        let _ = fs::remove_file(&tmp);
        ImportsError::new(format!(
            "install the gkeep import store {}: {error}",
            dest.display()
        ))
    })?;
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|error| {
            ImportsError::new(format!(
                "sync the gkeep import store directory {}: {error}",
                parent.display()
            ))
        })?;
    Ok(())
}

/// Preflight that metadata paths can be tracked in a Git vault: if any is
/// ignored, fail with the specific path instead of force-adding or
/// editing `.gitignore`.
pub(super) fn preflight_trackable(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    paths: &[PathBuf],
) -> Result<(), String> {
    use std::process::Stdio;
    for rel in paths {
        let mut cmd = crate::native::ob::git_command(bob_dir, child_env);
        cmd.arg("check-ignore")
            .arg("--quiet")
            .arg("--")
            .arg(rel)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match cmd.status() {
            Ok(status) if status.success() => {
                return Err(format!(
                    "gkeep metadata {} is ignored by Git; remove the ignore rule or move the vault so import history can be tracked (refusing to force-add or edit .gitignore)",
                    rel.display()
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(format!("failed to run git check-ignore: {error}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_then_verified_round_trip() {
        let dir = tempfile::tempdir().expect("temp");
        let block = "- [ ] #task Call dentist [created::2026-10-10]";
        let file = TransactionFile {
            schema_version: SCHEMA_VERSION,
            transaction_id: new_transaction_id(),
            destination: "gkeep_inbox.md".to_string(),
            before_sha256: sha256_hex(b"before"),
            after_sha256: Some(sha256_hex(b"after")),
            baseline_counts: BTreeMap::from([(block_digest(block), 0)]),
            entries: vec![ImportEntry {
                id: "note-1".to_string(),
                fp: "0123456789ab".to_string(),
                url: Some(
                    "https://keep.google.com/u/0/#NOTE/note-1".to_string(),
                ),
                path: "gkeep_inbox.md".to_string(),
                block_digest: block_digest(block),
                intended: Some(block.to_string()),
                state: EntryState::Prepared,
                dest_digest: None,
            }],
        };
        let dest =
            persist_prepared(dir.path(), &file).expect("persist prepared");
        let loaded = read_all(dir.path()).expect("read");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1.entries[0].state, EntryState::Prepared);
        assert!(has_verified(&loaded, "note-1", "0123456789ab") == false);

        let mut verified = loaded[0].1.clone();
        verified.entries[0].state = EntryState::Verified;
        verified.entries[0].intended = None;
        verified.entries[0].dest_digest = Some(sha256_hex(b"after"));
        persist_verified(&dest, &verified).expect("persist verified");
        let reloaded = read_all(dir.path()).expect("reread");
        assert!(has_verified(&reloaded, "note-1", "0123456789ab"));
        assert!(has_verified_id(&reloaded, "note-1"));
        assert!(!has_verified(&reloaded, "note-1", "ffffffffffff"));
        assert!(is_completed(&reloaded[0].1));
        let _ = dest;
    }

    #[test]
    fn malformed_store_is_actionable() {
        let dir = tempfile::tempdir().expect("temp");
        let store = imports_dir(dir.path());
        fs::create_dir_all(&store).expect("mkdir");
        fs::write(store.join("bad.json"), "{not json").expect("write");
        let error = read_all(dir.path()).expect_err("must fail");
        assert!(error.message.contains("malformed"), "{}", error.message);
    }

    #[test]
    fn unsafe_paths_rejected() {
        assert!(validate_relative_path("../escape.md").is_err());
        assert!(validate_relative_path("/abs.md").is_err());
        assert!(validate_relative_path("ok/note.md").is_ok());
    }

    #[test]
    fn absent_store_is_valid() {
        let dir = tempfile::tempdir().expect("temp");
        let loaded = read_all(dir.path()).expect("absent ok");
        assert!(loaded.is_empty());
    }
}
