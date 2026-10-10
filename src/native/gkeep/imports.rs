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
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
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

/// Store file that holds a verified `(id, fp)` receipt, when present.
pub(super) fn verified_file_for<'a>(
    files: &'a [(PathBuf, TransactionFile)],
    id: &str,
    fp: &str,
) -> Option<&'a (PathBuf, TransactionFile)> {
    files.iter().find(|(_, file)| {
        file.entries.iter().any(|entry| {
            entry.state == EntryState::Verified
                && entry.id == id
                && entry.fp == fp
        })
    })
}

/// Vault-relative form of `path` under `bob_dir`.
pub(super) fn vault_rel(bob_dir: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(bob_dir)
        .map(|relative| relative.to_path_buf())
        .unwrap_or_else(|_| {
            PathBuf::from(IMPORTS_DIR)
                .join(path.file_name().unwrap_or_default())
        })
}

/// Unfinished (prepared-entry) transactions.
pub(super) fn unfinished<'a>(
    files: &'a [(PathBuf, TransactionFile)],
) -> Vec<&'a (PathBuf, TransactionFile)> {
    files
        .iter()
        .filter(|(_, file)| !is_completed(file))
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

/// Actionable Git-ignore preflight failure. Callers map this onto the
/// existing `GkeepError` envelope (`kind`, `message`, `hint`).
#[derive(Debug, Clone)]
pub(super) struct TrackError {
    pub(super) message: String,
    pub(super) hint: Option<String>,
}

impl TrackError {
    fn new(message: String, hint: Option<String>) -> Self {
        Self { message, hint }
    }

    pub(super) fn into_gkeep(self) -> super::GkeepError {
        let mut error = super::GkeepError::runtime("commit", self.message);
        if let Some(hint) = self.hint {
            error = error.with_hint(&hint);
        }
        error
    }
}

/// Quiet `git check-ignore` outcome for one path.
enum IgnoreQuiet {
    Ignored,
    Trackable,
}

/// Preflight that paths can be tracked in a Git vault: if any is ignored,
/// fail with the matching rule instead of force-adding or editing
/// `.gitignore`. Tracked paths stay trackable even when an ignore pattern
/// would match an untracked counterpart (`--no-index` is never used).
pub(super) fn preflight_trackable(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    paths: &[PathBuf],
) -> Result<(), TrackError> {
    for rel in paths {
        // Metadata callers sometimes pass a directory before its first
        // receipt exists. Probe a representative JSON child so an explicit
        // file allowlist can make the directory trackable.
        let check_path = if is_gkeep_metadata_directory(rel) {
            rel.join(".bob-trackability-probe.json")
        } else {
            rel.clone()
        };
        match check_ignore_quiet(bob_dir, child_env, &check_path)? {
            IgnoreQuiet::Trackable => {}
            IgnoreQuiet::Ignored => {
                return Err(ignored_path_error(
                    bob_dir,
                    child_env,
                    rel,
                    &check_path,
                ));
            }
        }
    }
    Ok(())
}

/// Whether `rel` is present in `HEAD` with the same blob as the worktree.
/// The index alone is not proof of a commit. A missing HEAD path is
/// `Ok(false)`; a Git inspection failure is an error.
pub(super) fn matches_head(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    rel: &Path,
) -> Result<bool, String> {
    let spec = format!("HEAD:{}", rel.to_string_lossy().replace('\\', "/"));
    let verify = crate::native::ob::git_command(bob_dir, child_env)
        .arg("rev-parse")
        .arg("--verify")
        .arg("--quiet")
        .arg(&spec)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            format!("failed to run git rev-parse --verify {spec}: {error}")
        })?;
    match verify.code() {
        Some(0) => {}
        Some(1) => return Ok(false),
        Some(code) => {
            return Err(format!(
                "git rev-parse --verify {spec} failed (exit {code})"
            ));
        }
        None => {
            return Err(format!(
                "git rev-parse --verify {spec} terminated by signal"
            ));
        }
    }
    let head = crate::native::ob::git_command(bob_dir, child_env)
        .arg("rev-parse")
        .arg("--verify")
        .arg(&spec)
        .output()
        .map_err(|error| {
            format!("failed to run git rev-parse {spec}: {error}")
        })?;
    if !head.status.success() {
        return Err(format!(
            "git rev-parse {spec} failed: {}",
            String::from_utf8_lossy(&head.stderr).trim()
        ));
    }
    let worktree = crate::native::ob::git_command(bob_dir, child_env)
        .arg("hash-object")
        .arg("--")
        .arg(rel)
        .output()
        .map_err(|error| {
            format!(
                "failed to run git hash-object -- {}: {error}",
                rel.display()
            )
        })?;
    if !worktree.status.success() {
        return Err(format!(
            "git hash-object -- {} failed: {}",
            rel.display(),
            String::from_utf8_lossy(&worktree.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&head.stdout).trim()
        == String::from_utf8_lossy(&worktree.stdout).trim())
}

fn check_ignore_quiet(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    rel: &Path,
) -> Result<IgnoreQuiet, TrackError> {
    let output = crate::native::ob::git_command(bob_dir, child_env)
        .arg("check-ignore")
        .arg("--quiet")
        .arg("--")
        .arg(rel)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            TrackError::new(
                format!("failed to run git check-ignore: {error}"),
                None,
            )
        })?;
    match output.status.code() {
        Some(0) => Ok(IgnoreQuiet::Ignored),
        Some(1) => Ok(IgnoreQuiet::Trackable),
        Some(code) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            let message = if detail.is_empty() {
                format!(
                    "cannot inspect Git ignore rules for {}: git check-ignore exited {code}",
                    rel.display()
                )
            } else {
                format!(
                    "cannot inspect Git ignore rules for {}: {detail}",
                    rel.display()
                )
            };
            Err(TrackError::new(message, Some(inspect_hint(bob_dir, rel))))
        }
        None => Err(TrackError::new(
            format!(
                "cannot inspect Git ignore rules for {}: git check-ignore terminated by signal",
                rel.display()
            ),
            Some(inspect_hint(bob_dir, rel)),
        )),
    }
}

fn ignored_path_error(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    rel: &Path,
    check_path: &Path,
) -> TrackError {
    let inspect = inspect_hint(bob_dir, check_path);
    let fallback = TrackError::new(
        format!(
            "{}\nGit ignore inspection failed after the path was ignored; run: {inspect}",
            cannot_track_line(rel)
        ),
        Some(repair_hint(bob_dir, rel, check_path, None)),
    );
    match check_ignore_verbose(bob_dir, child_env, check_path) {
        Ok(Some(diag)) => {
            if diag.pattern.starts_with('!') {
                // Quiet said ignored; a negated last match means the
                // rule changed between queries. Stay failed.
                return fallback;
            }
            let message = format!(
                "{}\nignored by {}:{} (pattern: {})",
                cannot_track_line(rel),
                diag.source,
                diag.line,
                diag.pattern
            );
            TrackError::new(
                message,
                Some(repair_hint(bob_dir, rel, check_path, Some(&diag))),
            )
        }
        Ok(None) | Err(_) => fallback,
    }
}

struct IgnoreDiag {
    source: String,
    line: String,
    pattern: String,
}

fn check_ignore_verbose(
    bob_dir: &Path,
    child_env: &crate::native::ob::ChildEnv,
    rel: &Path,
) -> Result<Option<IgnoreDiag>, TrackError> {
    let mut child = crate::native::ob::git_command(bob_dir, child_env)
        .arg("check-ignore")
        .arg("--stdin")
        .arg("-z")
        .arg("-v")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            TrackError::new(
                format!("failed to run git check-ignore: {error}"),
                None,
            )
        })?;
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| {
            TrackError::new("git check-ignore has no stdin".to_string(), None)
        })?;
        let mut payload = rel.to_string_lossy().into_owned().into_bytes();
        payload.push(0);
        stdin.write_all(&payload).map_err(|error| {
            TrackError::new(
                format!("write git check-ignore stdin: {error}"),
                None,
            )
        })?;
    }
    let output = child.wait_with_output().map_err(|error| {
        TrackError::new(
            format!("failed to wait for git check-ignore: {error}"),
            None,
        )
    })?;
    match output.status.code() {
        Some(0) => Ok(parse_check_ignore_z(&output.stdout)),
        Some(1) => Ok(None),
        _ => Ok(None),
    }
}

/// Parse NUL-delimited `git check-ignore --stdin -z -v` output:
/// `source NUL linenum NUL pattern NUL pathname NUL`.
fn parse_check_ignore_z(stdout: &[u8]) -> Option<IgnoreDiag> {
    let mut fields = stdout.split(|byte| *byte == 0);
    let source = std::str::from_utf8(fields.next()?).ok()?.to_string();
    let line = std::str::from_utf8(fields.next()?).ok()?.to_string();
    let pattern = std::str::from_utf8(fields.next()?).ok()?.to_string();
    let pathname = std::str::from_utf8(fields.next()?).ok()?.to_string();
    if source.is_empty() || line.is_empty() || pathname.is_empty() {
        return None;
    }
    if !line.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(IgnoreDiag {
        source,
        line,
        pattern,
    })
}

fn is_import_receipt_path(rel: &Path) -> bool {
    let text = rel.to_string_lossy().replace('\\', "/");
    text.starts_with(".bob/gkeep/imports/") && text.ends_with(".json")
}

fn is_import_history_path(rel: &Path) -> bool {
    is_import_receipt_path(rel)
        || rel.to_string_lossy().replace('\\', "/") == IMPORTS_DIR
}

fn is_gkeep_metadata_path(rel: &Path) -> bool {
    let text = rel.to_string_lossy().replace('\\', "/");
    text == ".bob/gkeep" || text.starts_with(".bob/gkeep/")
}

fn is_gkeep_metadata_directory(rel: &Path) -> bool {
    rel.extension().is_none() && is_gkeep_metadata_path(rel)
}

fn cannot_track_line(rel: &Path) -> String {
    if is_import_history_path(rel) {
        format!("cannot track GKeep import history: {}", rel.display())
    } else if is_gkeep_metadata_path(rel) {
        format!("cannot track GKeep metadata: {}", rel.display())
    } else {
        format!("cannot track GKeep target note: {}", rel.display())
    }
}

fn inspect_hint(bob_dir: &Path, rel: &Path) -> String {
    format!(
        "git -C {} check-ignore -v -- {}",
        shell_quote(&bob_dir.display().to_string()),
        shell_quote(&rel.display().to_string())
    )
}

fn repair_hint(
    bob_dir: &Path,
    rel: &Path,
    check_path: &Path,
    diag: Option<&IgnoreDiag>,
) -> String {
    let inspect = inspect_hint(bob_dir, check_path);
    if is_import_history_path(rel) {
        if let Some(diag) = diag {
            if gitignore_star_rule(diag) {
                return "Allow /.bob/gkeep/imports/*.json in the vault's .gitignore, then retry. Import history must sync with the imported tasks.".to_string();
            }
            if pattern_excludes_ancestor(&diag.pattern, check_path) {
                return format!(
                    "{} excludes a parent directory; add traversal exceptions for each ancestor before allowing the receipt files. A leaf *.json exception alone is not enough. Import history must sync with the imported tasks. Inspect with: {inspect}",
                    diag.pattern
                );
            }
            return format!(
                "Allow the path in {}, then retry. Import history must sync with the imported tasks. Inspect with: {inspect}",
                diag.source
            );
        }
        return "Allow /.bob/gkeep/imports/*.json in the vault's .gitignore, then retry. Import history must sync with the imported tasks.".to_string();
    }
    if is_gkeep_metadata_path(rel) {
        if let Some(diag) = diag {
            return format!(
                "Allow the GKeep metadata path in {}, then retry. Metadata must sync with the vault. Inspect with: {inspect}",
                diag.source
            );
        }
        return format!(
            "Allow the GKeep metadata path in the vault's Git ignore rules, then retry. Metadata must sync with the vault. Inspect with: {inspect}"
        );
    }
    if let Some(diag) = diag {
        format!(
            "Allow the target note in {}, then retry. Inspect with: {inspect}",
            diag.source
        )
    } else {
        format!(
            "Allow the target note in the vault's Git ignore rules, then retry. Inspect with: {inspect}"
        )
    }
}

fn gitignore_star_rule(diag: &IgnoreDiag) -> bool {
    diag.pattern == "*"
        && Path::new(&diag.source)
            .file_name()
            .is_some_and(|name| name == ".gitignore")
}

fn pattern_excludes_ancestor(pattern: &str, rel: &Path) -> bool {
    let trimmed = pattern
        .trim_start_matches('!')
        .trim_start_matches('/')
        .trim_end_matches('/');
    if trimmed.is_empty() || trimmed.contains('*') || trimmed.contains('?') {
        return false;
    }
    let rel = rel.to_string_lossy().replace('\\', "/");
    rel == trimmed || rel.starts_with(&format!("{trimmed}/"))
}

/// Quote one value for `sh`: bare when every byte is shell-safe,
/// otherwise single-quoted with embedded quotes escaped.
fn shell_quote(value: &str) -> String {
    let is_safe = !value.is_empty()
        && value.bytes().all(|byte| {
            matches!(
                byte,
                b'A'..=b'Z'
                    | b'a'..=b'z'
                    | b'0'..=b'9'
                    | b'_'
                    | b'.'
                    | b'/'
                    | b':'
                    | b'@'
                    | b'%'
                    | b'+'
                    | b'='
                    | b','
                    | b'-'
            )
        });
    if is_safe {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
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

    fn isolated_env() -> crate::native::ob::ChildEnv {
        vec![
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
            ("GIT_CONFIG_SYSTEM".into(), "/dev/null".into()),
        ]
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = crate::native::ob::git_command(dir, &isolated_env())
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_git_repo(dir: &Path) {
        git(dir, &["init"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "Test"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
        git(dir, &["config", "core.excludesFile", "/dev/null"]);
    }

    fn production_allowlist() -> &'static str {
        "*\n!.gitignore\n!*/\n!*.md\n!.obsidian/\n!.obsidian/**/*.json\n"
    }

    #[test]
    fn parse_nul_verbose_ignore_handles_spaces_and_colons() {
        let mut raw = Vec::new();
        for field in [".gitignore", "3", "*", ".bob/gkeep/imports/a: b.json"] {
            raw.extend(field.as_bytes());
            raw.push(0);
        }
        let diag = parse_check_ignore_z(&raw).expect("parse");
        assert_eq!(diag.source, ".gitignore");
        assert_eq!(diag.line, "3");
        assert_eq!(diag.pattern, "*");
    }

    #[test]
    fn preflight_names_star_gitignore_rule() {
        let dir = tempfile::tempdir().expect("temp");
        init_git_repo(dir.path());
        fs::write(dir.path().join(".gitignore"), production_allowlist())
            .expect("gitignore");
        let rel = PathBuf::from(".bob/gkeep/imports/09b30c84e9c330f2.json");
        let error = preflight_trackable(dir.path(), &isolated_env(), &[rel])
            .expect_err("ignored");
        assert!(
            error.message.contains("cannot track GKeep import history"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("ignored by .gitignore:"),
            "{}",
            error.message
        );
        assert!(error.message.contains("pattern: *"), "{}", error.message);
        let hint = error.hint.expect("hint");
        assert!(hint.contains("Allow /.bob/gkeep/imports/*.json"), "{hint}");
        assert!(hint.contains("must sync"), "{hint}");
    }

    #[test]
    fn preflight_allows_receipts_after_star_exception() {
        let dir = tempfile::tempdir().expect("temp");
        init_git_repo(dir.path());
        fs::write(
            dir.path().join(".gitignore"),
            format!("{}!/.bob/gkeep/imports/*.json\n", production_allowlist()),
        )
        .expect("gitignore");
        let rel = PathBuf::from(".bob/gkeep/imports/09b30c84e9c330f2.json");
        preflight_trackable(dir.path(), &isolated_env(), &[rel])
            .expect("trackable");
    }

    #[test]
    fn preflight_negated_allowance_is_trackable() {
        let dir = tempfile::tempdir().expect("temp");
        init_git_repo(dir.path());
        fs::write(
            dir.path().join(".gitignore"),
            "*.json\n!/.bob/gkeep/imports/*.json\n",
        )
        .expect("gitignore");
        let rel = PathBuf::from(".bob/gkeep/imports/abc.json");
        preflight_trackable(dir.path(), &isolated_env(), &[rel])
            .expect("negated allowance is not ignored");
    }

    #[test]
    fn preflight_tracked_path_is_trackable_despite_ignore() {
        let dir = tempfile::tempdir().expect("temp");
        init_git_repo(dir.path());
        let rel = PathBuf::from(".bob/gkeep/imports/tracked.json");
        fs::create_dir_all(dir.path().join(".bob/gkeep/imports"))
            .expect("mkdir");
        fs::write(dir.path().join(&rel), "{}\n").expect("write");
        git(dir.path(), &["add", "-f", rel.to_str().unwrap()]);
        git(dir.path(), &["commit", "-m", "track"]);
        fs::write(dir.path().join(".gitignore"), "*.json\n").expect("ignore");
        preflight_trackable(dir.path(), &isolated_env(), &[rel])
            .expect("tracked remains trackable");
    }

    #[test]
    fn preflight_fatal_is_an_error() {
        let dir = tempfile::tempdir().expect("temp");
        let rel = PathBuf::from(".bob/gkeep/imports/abc.json");
        let error = preflight_trackable(dir.path(), &isolated_env(), &[rel])
            .expect_err("not a repo");
        assert!(
            error.message.contains("cannot inspect Git ignore rules"),
            "{}",
            error.message
        );
    }

    #[test]
    fn preflight_parent_exclusion_names_the_rule() {
        let dir = tempfile::tempdir().expect("temp");
        init_git_repo(dir.path());
        fs::write(dir.path().join(".gitignore"), ".bob/\n").expect("gitignore");
        let rel = PathBuf::from(".bob/gkeep/imports/abc.json");
        let error = preflight_trackable(dir.path(), &isolated_env(), &[rel])
            .expect_err("ignored");
        assert!(
            error.message.contains("pattern: .bob/"),
            "{}",
            error.message
        );
        let hint = error.hint.expect("hint");
        assert!(hint.contains("parent directory"), "{hint}");
        assert!(hint.contains("leaf"), "{hint}");
    }

    #[test]
    fn preflight_ignored_target_is_called_a_note() {
        let dir = tempfile::tempdir().expect("temp");
        init_git_repo(dir.path());
        fs::write(dir.path().join(".gitignore"), "inbox.md\n")
            .expect("gitignore");
        let rel = PathBuf::from("inbox.md");
        let error = preflight_trackable(dir.path(), &isolated_env(), &[rel])
            .expect_err("ignored");
        assert!(
            error.message.contains("cannot track GKeep target note"),
            "{}",
            error.message
        );
        assert!(
            !error.message.contains("import history"),
            "{}",
            error.message
        );
        let hint = error.hint.expect("hint");
        assert!(hint.contains("target note"), "{hint}");
    }

    #[test]
    fn preflight_custom_excludes_file_is_named() {
        let dir = tempfile::tempdir().expect("temp");
        init_git_repo(dir.path());
        let excludes = dir.path().join("custom.excludes");
        fs::write(&excludes, "secret.md\n").expect("excludes");
        git(
            dir.path(),
            &["config", "core.excludesFile", excludes.to_str().unwrap()],
        );
        let rel = PathBuf::from("secret.md");
        let error = preflight_trackable(dir.path(), &isolated_env(), &[rel])
            .expect_err("ignored");
        assert!(
            error.message.contains("custom.excludes"),
            "{}",
            error.message
        );
    }

    #[test]
    fn matches_head_detects_committed_unchanged_receipt() {
        let dir = tempfile::tempdir().expect("temp");
        init_git_repo(dir.path());
        let rel = PathBuf::from(".bob/gkeep/imports/abc.json");
        fs::create_dir_all(dir.path().join(".bob/gkeep/imports"))
            .expect("mkdir");
        fs::write(dir.path().join(&rel), "{}\n").expect("write");
        git(dir.path(), &["add", "-f", rel.to_str().unwrap()]);
        git(dir.path(), &["commit", "-m", "receipt"]);
        assert!(matches_head(dir.path(), &isolated_env(), &rel).expect("head"));
        fs::write(dir.path().join(&rel), "{changed}\n").expect("edit");
        assert!(
            !matches_head(dir.path(), &isolated_env(), &rel).expect("dirty")
        );
        let missing = PathBuf::from(".bob/gkeep/imports/nope.json");
        assert!(!matches_head(dir.path(), &isolated_env(), &missing)
            .expect("missing"));
    }

    #[test]
    fn preflight_metadata_directories_probe_json_entries() {
        let dir = tempfile::tempdir().expect("temp repo");
        init_git_repo(dir.path());
        fs::write(
            dir.path().join(".gitignore"),
            "*\n!*/\n!/.bob/gkeep/imports/*.json\n!*.md\n",
        )
        .expect("write ignore rules");

        let child_env = isolated_env();
        assert!(preflight_trackable(
            dir.path(),
            &child_env,
            &[PathBuf::from(IMPORTS_DIR)],
        )
        .is_ok());
        let error = preflight_trackable(
            dir.path(),
            &child_env,
            &[PathBuf::from(".bob/gkeep/migrate-tasks")],
        )
        .expect_err("migration metadata ignored");
        assert!(error.message.contains("cannot track GKeep metadata"));
        assert!(error
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("must sync"));
    }
}
