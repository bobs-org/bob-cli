//! Durable overwrite intents and scoped HEAD recovery.
//!
//! An overwrite transaction persists an intent in
//! `bob_cli_state_dir()/ref/overwrite-intents/` (never in the vault) before
//! mutating the vault, so a crash cannot lose the earlier capture. The next
//! real `ref create` or writing `ref scan` (including intake-only and JSON
//! variants) recovers under the same lock order before any pre-sync can
//! commit half-applied files. Standalone `bob vault-sync` recovers the same
//! way (taking `scan.lock` when an intent is pending). Other pre-syncs
//! (`task move`, `randomize`, migrations, `vault_rewrite`) refuse to stage
//! while an intent is pending. `ref rename` recovers under the locks it
//! already holds. Recovery then:
//!
//! * the recorded overwrite commit is present → keep the new state and
//!   clear the intent;
//! * otherwise → reverse proven annotated moves, remove only recorded
//!   creations, restore the touched paths from the recorded clean HEAD,
//!   unstage those paths, and clear the intent.
//!
//! Schema version 2 adds move evidence for annotated overwrites, which
//! rename (rather than rewrite) the live PDF and assets into the archive
//! bundle: each move records its source, destination, and the source hash,
//! plus transaction-owned replacement bytes where the live path is reused.
//! A crash between any two renames is then distinguishable from a
//! user-created destination or a late editor save. Proven moves reverse
//! before generic cleanup runs, and anything unexpected retains the files
//! and the intent with a recovery hint instead of deleting the only copy.
//! Version-1 intents (no moves or writes) still load and recover exactly
//! as before.
//!
//! A generic matching commit subject alone never proves the commit: proof
//! needs the recorded commit SHA to exist with the expected subject and to
//! be an ancestor of HEAD. Dry runs stay read-only and stop with a
//! recovery-required refusal. Malformed intents and vaults that moved since
//! the intent was written fail without broad restore/clean operations, and
//! the marker is preserved whenever repair fails.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{reference_mutation, CommandError, Config};
use crate::native::{env as bob_env, ob};

/// Intent schema version; bump when the JSON shape changes. Version 1
/// intents carry only `touched`/`created`; version 2 adds `moves` and
/// `writes` for annotated renames. Recovery accepts both.
const INTENT_SCHEMA_VERSION: u32 = 2;

/// Directory (under the state root's `ref/`) holding one intent file per
/// canonical vault.
const INTENT_DIR_NAME: &str = "overwrite-intents";

/// Machine-readable failure code for a dry run blocked by a pending
/// intent. Human rendering ignores it; JSON scans surface it.
const RECOVERY_REQUIRED_CODE: &str = "recovery_required";

/// Durable record of one in-flight overwrite transaction.
///
/// `touched` lists vault-relative paths that existed at `base_head` and may
/// have been modified; `created` lists vault-relative paths the transaction
/// added (directory operations expanded to their actual file manifest, so
/// untracked contents are never deleted with a sidecar directory).
/// `expected_head` is the commit SHA the transaction created, recorded once
/// the commit is proved; it distinguishes this overwrite from a previous
/// same-stem overwrite carrying the same subject.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OverwriteIntent {
    schema_version: u32,
    /// Canonical vault path string the intent was written for.
    pub(crate) vault: String,
    /// Reference stem being overwritten (`lib`/note stem without suffix).
    pub(crate) stem: String,
    /// Reference type directory (`papers`, `blogs`, …).
    pub(crate) ref_type: String,
    /// Clean locked HEAD after pre-sync: the rollback base.
    pub(crate) base_head: String,
    /// Expected transaction commit subject
    /// (`bob ref create: overwrite <type>/<stem>`).
    pub(crate) expected_subject: String,
    /// Commit SHA the transaction created, once proved; `None` while the
    /// transaction has not committed yet.
    pub(crate) expected_head: Option<String>,
    /// Vault-relative paths that existed at the base and may be modified.
    pub(crate) touched: Vec<String>,
    /// Vault-relative paths the transaction created.
    pub(crate) created: Vec<String>,
    /// No-clobber renames the transaction owns: the live PDF and assets
    /// moved into the archive bundle. Empty for version-1 intents and for
    /// unannotated overwrites.
    #[serde(default)]
    pub(crate) moves: Vec<PendingMove>,
    /// Transaction-owned new bytes at created paths (the frozen archive
    /// note and any new live assets). Recovery only removes these bytes
    /// when they still match; anything else fails closed.
    #[serde(default)]
    pub(crate) writes: Vec<PendingWrite>,
}

/// One vault-relative file inside a moved assets directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PendingMoveFile {
    /// Path relative to the moved directory root, using `/` separators.
    pub(crate) rel: String,
    /// Lowercase hex SHA-256 of the file bytes at intent time.
    pub(crate) sha256: String,
}

/// One no-clobber rename the transaction owns: the live PDF or assets
/// moved into the archive bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PendingMove {
    /// Vault-relative source that existed at intent time.
    pub(crate) source: String,
    /// Vault-relative destination that was absent at intent time.
    pub(crate) dest: String,
    /// Hash of the source file bytes at intent time (`None` for
    /// directory moves, which list their files in `files`).
    #[serde(default)]
    pub(crate) source_sha256: Option<String>,
    /// Hash of transaction-owned bytes later published to `source`
    /// (the live path is reused for the replacement PDF). A live path
    /// holding these bytes is safe to clear before reversing the move;
    /// anything else there fails closed.
    #[serde(default)]
    pub(crate) replacement_sha256: Option<String>,
    /// Per-file manifest for a directory move (live `.assets` into the
    /// archive `.assets`). Empty for a single-file move.
    #[serde(default)]
    pub(crate) files: Vec<PendingMoveFile>,
}

/// Transaction-owned new bytes at one vault-relative path. Rewritten
/// notes and link files are not recorded here: they restore from HEAD
/// as before. Only created files (and live-path reuses recorded on
/// their move) need ownership evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PendingWrite {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

/// Constructors for synthetic interrupted transactions in tests. The
/// execute phase promotes these to shared API when it wires annotated
/// transactions; until then they stay test-only so the lint gate holds.
#[cfg(test)]
impl PendingMove {
    /// A single-file rename with the source hash observed before the move.
    pub(crate) fn file(source: &str, dest: &str, source_sha256: &str) -> Self {
        Self {
            source: source.to_string(),
            dest: dest.to_string(),
            source_sha256: Some(source_sha256.to_string()),
            replacement_sha256: None,
            files: Vec::new(),
        }
    }

    /// Record transaction-owned bytes later published to the live path.
    pub(crate) fn with_replacement(mut self, sha256: &str) -> Self {
        self.replacement_sha256 = Some(sha256.to_string());
        self
    }

    /// A directory rename with a per-file manifest (see `asset_manifest`).
    pub(crate) fn dir(
        source: &str,
        dest: &str,
        files: Vec<PendingMoveFile>,
    ) -> Self {
        Self {
            source: source.to_string(),
            dest: dest.to_string(),
            source_sha256: None,
            replacement_sha256: None,
            files,
        }
    }
}

/// Shared shape query used by planning and cleanup in all builds.
impl PendingMove {
    /// True for the single-file form, false for a directory manifest.
    fn is_file(&self) -> bool {
        self.source_sha256.is_some()
    }
}

/// The live-PDF rename into one archive bundle, with the staged
/// replacement bytes that will reuse the live path. The execute phase
/// persists this before the first rename; recovery consumes it without
/// enabling the CLI route.
#[cfg(test)]
pub(crate) fn pdf_move_for_archive(
    bundle: &super::archive::ArchiveBundle,
    live_pdf: &str,
    live_pdf_sha256: &str,
    replacement_sha256: Option<&str>,
) -> PendingMove {
    let mut pending = PendingMove::file(
        live_pdf,
        &bundle.annotated_pdf_path,
        live_pdf_sha256,
    );
    if let Some(replacement) = replacement_sha256 {
        pending.replacement_sha256 = Some(replacement.to_string());
    }
    pending
}

/// The live-assets rename into one archive bundle, with a per-file
/// manifest collected from the vault before the move.
#[cfg(test)]
pub(crate) fn assets_move_for_archive(
    bundle: &super::archive::ArchiveBundle,
    live_assets: &str,
    manifest: Vec<PendingMoveFile>,
) -> PendingMove {
    PendingMove::dir(live_assets, &bundle.assets_path, manifest)
}

/// Hash every regular file under a live assets directory, returning the
/// manifest a directory move records. Refuses symlinks so recovery never
/// follows one out of the vault.
#[cfg(test)]
pub(crate) fn asset_manifest(
    vault: &Path,
    live_assets: &str,
) -> std::result::Result<Vec<PendingMoveFile>, String> {
    let mut manifest = Vec::new();
    collect_manifest_files(
        &vault.join(live_assets),
        Path::new(""),
        &mut manifest,
    )?;
    manifest.sort_by(|left, right| left.rel.cmp(&right.rel));
    Ok(manifest)
}

#[cfg(test)]
fn collect_manifest_files(
    absolute: &Path,
    rel: &Path,
    manifest: &mut Vec<PendingMoveFile>,
) -> std::result::Result<(), String> {
    let file_type = fs::symlink_metadata(absolute).map_err(|error| {
        format!("could not inspect {}: {error}", absolute.display())
    })?;
    if file_type.file_type().is_symlink() {
        return Err(format!(
            "refusing to fingerprint symlink {}",
            absolute.display()
        ));
    }
    if file_type.file_type().is_file() {
        let bytes = fs::read(absolute).map_err(|error| {
            format!("could not read {}: {error}", absolute.display())
        })?;
        manifest.push(PendingMoveFile {
            rel: rel.to_string_lossy().replace('\\', "/"),
            sha256: hex::encode(Sha256::digest(&bytes)),
        });
        return Ok(());
    }
    if !file_type.file_type().is_dir() {
        return Err(format!(
            "unsupported assets path type: {}",
            absolute.display()
        ));
    }
    let mut entries = fs::read_dir(absolute)
        .map_err(|error| {
            format!("could not scan {}: {error}", absolute.display())
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            format!("could not scan {}: {error}", absolute.display())
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        collect_manifest_files(
            &entry.path(),
            &rel.join(entry.file_name()),
            manifest,
        )?;
    }
    Ok(())
}

impl OverwriteIntent {
    /// Build the durable manifest from the exact preimages accepted by the
    /// transaction's locked revalidation.
    pub(in crate::native::highlights_ref) fn for_transaction(
        config: &Config,
        stem: &str,
        ref_type: &str,
        base_head: &str,
        expected_subject: String,
        preimages: &[super::FilePreimage],
    ) -> std::result::Result<Self, CommandError> {
        let vault = canonical_vault(config)?;
        let mut touched = Vec::new();
        let mut created = Vec::new();
        for preimage in preimages {
            if preimage.sha256.is_some() {
                touched.push(preimage.path.clone());
            } else {
                created.push(preimage.path.clone());
            }
        }
        touched.sort();
        touched.dedup();
        created.sort();
        created.dedup();
        Ok(Self {
            schema_version: INTENT_SCHEMA_VERSION,
            vault,
            stem: stem.to_string(),
            ref_type: ref_type.to_string(),
            base_head: base_head.to_string(),
            expected_subject,
            expected_head: None,
            touched,
            created,
            moves: Vec::new(),
            writes: Vec::new(),
        })
    }

    /// Attach annotated move evidence and transaction-owned writes to an
    /// intent built by `for_transaction`. The execute phase calls this
    /// before persisting, so the evidence predates the first rename.
    #[cfg(test)]
    pub(in crate::native::highlights_ref) fn with_annotated_moves(
        mut self,
        moves: Vec<PendingMove>,
        writes: Vec<PendingWrite>,
    ) -> Self {
        self.moves = moves;
        self.writes = writes;
        self
    }

    /// Store the exact commit SHA after Git proves the scoped commit landed.
    pub(in crate::native::highlights_ref) fn record_expected_head(
        &mut self,
        config: &Config,
        head: &str,
    ) -> std::result::Result<(), String> {
        self.expected_head = Some(head.to_string());
        save_intent(config, self).map(|_| ())
    }
}

/// Persist (or replace) the intent for `config`'s vault atomically: write
/// a temp file, fsync it, rename over the destination, then fsync the
/// directory. Consumed by the transaction phase.
pub(crate) fn save_intent(
    config: &Config,
    intent: &OverwriteIntent,
) -> std::result::Result<PathBuf, String> {
    save_intent_to(&state_ref_dir(), config, intent)
}

/// Load the pending intent for `config`'s vault, if any. A present but
/// unreadable file is a hard error: recovery must fail closed rather than
/// treat a half-written marker as absent. Consumed by the transaction
/// phase and by other pre-syncs that must not commit a crashed overwrite.
pub(crate) fn load_intent(
    config: &Config,
) -> std::result::Result<Option<OverwriteIntent>, CommandError> {
    load_intent_from(&state_ref_dir(), config)
}

/// Remove the intent for `config`'s vault. Missing files are fine.
/// Consumed by the transaction phase alongside recovery's own clears.
pub(crate) fn clear_intent(config: &Config) -> std::result::Result<(), String> {
    let canonical =
        canonical_vault(config).map_err(|error| error.to_string())?;
    let path = intent_path_for(&state_ref_dir(), &canonical);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not clear overwrite intent {}: {error}",
            path.display()
        )),
    }
}

/// How recovery performs the reverse rename of one file. Production
/// coordinates the live PDF path exactly like explicit overwrite does, so
/// a PDF an editor holds fails closed instead of renaming underneath it.
/// Tests inject holds and failures through this seam rather than through
/// a production environment variable pretending Linux is a Mac.
pub(crate) trait ReverseCoordinator {
    fn reverse_file(
        &self,
        vault: &Path,
        from: &Path,
        to: &Path,
    ) -> std::result::Result<(), String>;
}

/// Production reverse moves: coordinate when explicit overwrite would,
/// otherwise rename directly. A hold or failure never falls back to an
/// uncoordinated rename.
pub(crate) struct PlatformReverseCoordinator;

impl ReverseCoordinator for PlatformReverseCoordinator {
    fn reverse_file(
        &self,
        _vault: &Path,
        from: &Path,
        to: &Path,
    ) -> std::result::Result<(), String> {
        let coordinated =
            super::super::marker_write::MarkerWriteSession::for_explicit_write(
            )
            .map(|session| session.coordinates())
            .unwrap_or(false);
        if !coordinated {
            return rename_no_clobber(from, to);
        }
        match crate::native::file_coordination::coordinate_merging_write(
            to,
            std::time::Duration::from_secs(10),
            || rename_no_clobber(from, to),
        ) {
            crate::native::file_coordination::Coordination::Granted {
                value,
                ..
            } => value,
            crate::native::file_coordination::Coordination::Held(timed_out) => {
                Err(format!(
                    "PDF coordination timed out after {:.2}s; close it in Highlights and retry",
                    timed_out.waited.as_secs_f64()
                ))
            }
            crate::native::file_coordination::Coordination::Failed(detail) => {
                Err(format!(
                    "PDF coordination failed: {detail}; close it in Highlights and retry"
                ))
            }
        }
    }
}

/// Direct reverse moves without coordination (unit tests on Linux, where
/// no presenter can hold the PDF).
#[cfg(test)]
pub(crate) struct DirectReverseCoordinator;

#[cfg(test)]
impl ReverseCoordinator for DirectReverseCoordinator {
    fn reverse_file(
        &self,
        _vault: &Path,
        from: &Path,
        to: &Path,
    ) -> std::result::Result<(), String> {
        rename_no_clobber(from, to)
    }
}

/// Roll back a transaction that failed before its commit. The marker stays
/// durable until the scoped restore and unstage both succeed.
pub(in crate::native::highlights_ref) fn rollback_transaction(
    config: &Config,
    child_env: &ob::ChildEnv,
    intent: &OverwriteIntent,
) -> std::result::Result<(), CommandError> {
    let state_dir = state_ref_dir();
    recover_locked(
        &state_dir,
        config,
        child_env,
        intent,
        false,
        &PlatformReverseCoordinator,
    )
    .map(|_| ())
}

/// Entry hook for commands that do not hold locks yet (`ref create` and
/// intake-only scans): fast-path when no intent exists without taking any
/// lock, otherwise recover under a fresh mutation session and release it
/// before returning. Returns a stderr notice when recovery ran, `None`
/// when there was nothing to do.
pub(crate) fn recover_before_command(
    config: &Config,
    dry_run: bool,
) -> std::result::Result<Option<String>, CommandError> {
    let state_dir = state_ref_dir();
    let Some(intent) = load_intent_from(&state_dir, config)? else {
        return Ok(None);
    };
    if dry_run {
        return Err(recovery_required(&intent));
    }
    let child_env = ob::child_env();
    let vault = &config.bob_dir;
    let _locks = reference_mutation::acquire_mutation_session(
        vault,
        &child_env,
        || eprintln!("waiting for another vault maintenance run…"),
    )
    .map_err(|error| match error {
        reference_mutation::MutationSessionError::Maintenance(error) => {
            CommandError::new(format!(
                "could not acquire the vault maintenance lock: {error}"
            ))
            .with_code("lock_busy")
        }
        reference_mutation::MutationSessionError::NotAWorktree => {
            CommandError::new(format!(
                "could not recover the interrupted overwrite of {}: the vault is not a Git worktree",
                intent.stem
            ))
        }
        reference_mutation::MutationSessionError::Scan(error) => error,
    })?;
    recover_locked(
        &state_dir,
        config,
        &child_env,
        &intent,
        false,
        &PlatformReverseCoordinator,
    )
}

/// Entry hook for full scans, called after they hold the maintenance and
/// writer locks: recover without relocking. Dry runs perform no mutation
/// and need no locks; they only report a pending intent.
pub(crate) fn recover_with_locks_held(
    config: &Config,
    child_env: &ob::ChildEnv,
    dry_run: bool,
) -> std::result::Result<Option<String>, CommandError> {
    let state_dir = state_ref_dir();
    let Some(intent) = load_intent_from(&state_dir, config)? else {
        return Ok(None);
    };
    recover_locked(
        &state_dir,
        config,
        child_env,
        &intent,
        dry_run,
        &PlatformReverseCoordinator,
    )
}

/// Standalone `bob vault-sync` already holds `bob_sync.lock`. When a
/// pending overwrite intent exists, take `scan.lock` (same order and
/// timeout as other mutation sessions) and recover before staging. A
/// busy lock or a failed recovery refuses the cycle without staging.
pub(crate) fn recover_for_standalone_sync(
    vault: &Path,
    child_env: &ob::ChildEnv,
    dry_run: bool,
) -> std::result::Result<Option<String>, String> {
    let config = super::super::Config::for_vault(vault);
    let Some(intent) =
        load_intent(&config).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let _scan = super::super::acquire_scan_writer_lock_with_timeout(
        std::time::Duration::from_secs(30),
    )
    .map_err(|error| {
        format!(
            "could not recover the interrupted overwrite of {}: {error}; run bob ref scan on this host",
            intent.stem
        )
    })?;
    recover_with_locks_held(&config, child_env, dry_run)
        .map_err(|error| format!("{error}; run bob ref scan on this host"))
}

/// Other `run_cycle_*` callers already hold `bob_sync.lock` and must not
/// stage or commit while an overwrite intent is pending. The error names
/// the stem and tells the operator to run `bob ref scan` on this host.
pub(crate) fn refuse_sync_if_overwrite_pending(
    vault: &Path,
) -> std::result::Result<(), String> {
    let config = super::super::Config::for_vault(vault);
    match load_intent(&config).map_err(|error| error.to_string())? {
        None => Ok(()),
        Some(intent) => Err(format!(
            "an interrupted overwrite of {} is pending; run bob ref scan on this host",
            intent.stem
        )),
    }
}

/// Core recovery: validate, prove-or-restore, then clear. Never broadens
/// scope on failure: malformed intents, path escapes, moved vaults, and
/// failed repairs all preserve the marker. Annotated moves reverse before
/// generic cleanup so the only annotated copy is never deleted, and any
/// unexpected bytes fail closed with the files and intent retained.
fn recover_locked(
    state_dir: &Path,
    config: &Config,
    child_env: &ob::ChildEnv,
    intent: &OverwriteIntent,
    dry_run: bool,
    coordinator: &dyn ReverseCoordinator,
) -> std::result::Result<Option<String>, CommandError> {
    let vault = &config.bob_dir;
    let canonical = canonical_vault(config)?;
    if intent.vault != canonical {
        return Err(CommandError::new(format!(
            "ignoring overwrite intent for another vault ({}); remove it to proceed",
            intent_display_path(state_dir, &canonical).display()
        )));
    }
    let touched = validated_paths(&intent.touched)?;
    let created = validated_paths(&intent.created)?;
    validate_intent_evidence(intent)?;
    if dry_run {
        return Err(recovery_required(intent));
    }
    if !reference_mutation::vault_is_git_worktree(vault, child_env) {
        return Err(CommandError::new(format!(
            "could not recover the interrupted overwrite of {}: the vault is not a Git worktree",
            intent.stem
        )));
    }
    let head =
        reference_mutation::git_head(vault, child_env).map_err(|error| {
            preserve_message(
                intent,
                &format!("could not read the vault HEAD: {error}"),
            )
        })?;
    if overwrite_committed(vault, child_env, intent, &head) {
        clear_intent_in(state_dir, &canonical).map_err(|error| {
            preserve_message(
                intent,
                &format!("the overwrite finished but {error}"),
            )
        })?;
        return Ok(Some(format!(
            "recovered: kept the completed overwrite of {}",
            intent.stem
        )));
    }
    if head != intent.base_head {
        return Err(preserve_message(
            intent,
            &format!(
                "the vault moved since the overwrite began (HEAD {head} is not the recorded base {}); left the intent in place",
                intent.base_head
            ),
        ));
    }
    // Annotated renames reverse before generic cleanup: a proved move
    // returns the only annotated copy to its live path, and ownership
    // failures stop here with the files and intent retained. Version-1
    // intents (no moves or writes) skip this and restore exactly as before.
    if !intent.moves.is_empty() || !intent.writes.is_empty() {
        let plan = plan_annotated_reversal(vault, intent)?;
        execute_reversal_plan(vault, intent, &plan, coordinator)?;
        verify_created_ownership(vault, intent)?;
    }
    restore_from_base(vault, child_env, intent, &touched, &created)?;
    clear_intent_in(state_dir, &canonical).map_err(|error| {
        preserve_message(intent, &format!("rolled back but {error}"))
    })?;
    Ok(Some(format!(
        "recovered: rolled back an interrupted overwrite of {}",
        intent.stem
    )))
}

/// True when the recorded overwrite commit is present: its SHA exists with
/// the expected subject and is an ancestor of HEAD. A generic matching
/// subject alone (including a previous same-stem overwrite) never proves
/// it. When the transaction committed but crashed before recording the
/// SHA, the HEAD commit carrying the expected subject proves it instead.
fn overwrite_committed(
    vault: &Path,
    child_env: &ob::ChildEnv,
    intent: &OverwriteIntent,
    head: &str,
) -> bool {
    if let Some(expected) = intent.expected_head.as_deref() {
        return commit_subject(vault, child_env, expected)
            .is_some_and(|subject| subject == intent.expected_subject)
            && is_ancestor(vault, child_env, expected, head);
    }
    head != intent.base_head
        && commit_subject(vault, child_env, head)
            .is_some_and(|subject| subject == intent.expected_subject)
}

/// Subject line of one commit, or `None` when it cannot be read.
fn commit_subject(
    vault: &Path,
    child_env: &ob::ChildEnv,
    sha: &str,
) -> Option<String> {
    let output = ob::git_command(vault, child_env)
        .args(["log", "-1", "--format=%s", sha])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// True when `ancestor` is an ancestor of `head`.
fn is_ancestor(
    vault: &Path,
    child_env: &ob::ChildEnv,
    ancestor: &str,
    head: &str,
) -> bool {
    ob::git_command(vault, child_env)
        .args(["merge-base", "--is-ancestor", ancestor, head])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Reject unknown schema versions and malformed move/write evidence
/// without touching the vault. Version 1 has neither field; version 2 may
/// carry both. Anything newer fails closed so a newer writer's evidence is
/// never misread.
fn validate_intent_evidence(
    intent: &OverwriteIntent,
) -> Result<(), CommandError> {
    if intent.schema_version == 0
        || intent.schema_version > INTENT_SCHEMA_VERSION
    {
        return Err(CommandError::new(format!(
            "ignoring overwrite intent with unsupported schema version {}; remove it to proceed",
            intent.schema_version
        )));
    }
    for step in &intent.moves {
        validate_move(intent, step)?;
    }
    for write in &intent.writes {
        validated_paths(std::slice::from_ref(&write.path))?;
        require_sha256(&write.sha256).map_err(|detail| {
            CommandError::new(format!(
                "ignoring malformed overwrite intent with invalid write hash for {}; remove it to proceed ({detail})",
                write.path
            ))
        })?;
    }
    Ok(())
}

fn validate_move(
    intent: &OverwriteIntent,
    step: &PendingMove,
) -> Result<(), CommandError> {
    let malformed = |detail: &str| {
        CommandError::new(format!(
            "ignoring malformed overwrite intent ({}); remove it to proceed",
            detail.replace("{stem}", &intent.stem)
        ))
    };
    validated_paths(&[step.source.clone(), step.dest.clone()])?;
    if step.source == step.dest {
        return Err(malformed(
            "a move names the same source and destination for {stem}",
        ));
    }
    let is_file = step.source_sha256.is_some();
    let is_dir = !step.files.is_empty();
    if is_file == is_dir {
        return Err(malformed(
            "a move for {stem} must record either one file hash or a directory manifest",
        ));
    }
    if let Some(sha) = &step.source_sha256 {
        require_sha256(sha).map_err(|_| {
            malformed("a move for {stem} records an invalid source hash")
        })?;
    }
    if let Some(sha) = &step.replacement_sha256 {
        require_sha256(sha).map_err(|_| {
            malformed("a move for {stem} records an invalid replacement hash")
        })?;
    }
    let mut seen = BTreeSet::new();
    for file in &step.files {
        validate_manifest_rel(&file.rel).map_err(malformed)?;
        require_sha256(&file.sha256).map_err(|_| {
            malformed("a move for {stem} records an invalid manifest hash")
        })?;
        if !seen.insert(file.rel.clone()) {
            return Err(malformed(
                "a move for {stem} records a duplicate manifest entry",
            ));
        }
    }
    Ok(())
}

/// Vault-relative manifest entries stay below the moved directory root.
fn validate_manifest_rel(rel: &str) -> std::result::Result<(), &'static str> {
    if rel.is_empty() {
        return Err("an empty manifest path");
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err("an absolute manifest path");
    }
    if rel.split(['/', '\\']).any(|part| part == ".") {
        return Err("a manifest path with `.`");
    }
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                if name.eq_ignore_ascii_case(".git") {
                    return Err("a manifest path under `.git`");
                }
                depth += 1;
            }
            Component::CurDir
            | Component::ParentDir
            | Component::Prefix(_)
            | Component::RootDir => {
                return Err("a manifest path that escapes its directory");
            }
        }
    }
    if depth == 0 {
        return Err("an empty manifest path");
    }
    Ok(())
}

fn require_sha256(value: &str) -> std::result::Result<(), &'static str> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err("expected 64 lowercase hex digits")
    }
}

/// Read-only ownership proof for every annotated move and write. Returns
/// the reversal plan: proven renames to run (last move first) plus
/// transaction-owned paths to clear first. Any late editor save, an
/// unexpected new asset, or a user-created destination fails closed with
/// the files and intent retained; nothing is mutated here.
fn plan_annotated_reversal(
    vault: &Path,
    intent: &OverwriteIntent,
) -> std::result::Result<ReversalPlan, CommandError> {
    let mut plan = ReversalPlan::default();
    let owned = write_hashes(intent);
    for step in intent.moves.iter().rev() {
        if step.is_file() {
            plan_file_move(vault, intent, step, &owned, &mut plan)?;
        } else {
            plan_dir_move(vault, intent, step, &owned, &mut plan)?;
        }
    }
    // Transaction-owned created bytes are removed by generic cleanup, but
    // only after the proof below: an existing created file without a
    // recorded write is a destination race and fails closed here.
    for write in &intent.writes {
        let absolute = vault.join(&write.path);
        match fs::symlink_metadata(&absolute) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(preserve_message(
                    intent,
                    &format!(
                        "could not inspect transaction-owned {}: {error}",
                        write.path
                    ),
                ));
            }
            Ok(metadata) => {
                if !metadata.file_type().is_file() {
                    return Err(preserve_message(
                        intent,
                        &format!(
                            "transaction-owned {} is unexpectedly a {}; left the intent in place",
                            write.path,
                            path_kind(&metadata),
                        ),
                    ));
                }
                let current = file_sha256(&absolute)
                    .map_err(|error| preserve_message(intent, &error))?;
                if current != write.sha256 {
                    return Err(preserve_message(
                        intent,
                        &format!(
                            "{} changed after the overwrite began (a late save or a new file at a transaction path); left the intent in place",
                            write.path
                        ),
                    ));
                }
            }
        }
    }
    Ok(plan)
}

/// Transaction-owned write hashes by vault-relative slash path.
fn write_hashes(intent: &OverwriteIntent) -> BTreeMap<&str, &str> {
    intent
        .writes
        .iter()
        .map(|write| (write.path.as_str(), write.sha256.as_str()))
        .collect()
}

/// Classify one single-file rename and record the resulting actions.
fn plan_file_move(
    vault: &Path,
    intent: &OverwriteIntent,
    step: &PendingMove,
    owned: &BTreeMap<&str, &str>,
    plan: &mut ReversalPlan,
) -> std::result::Result<(), CommandError> {
    let source_hash = step.source_sha256.as_deref().unwrap_or_default();
    plan_endpoint_pair(
        vault,
        intent,
        Path::new(&step.source),
        Path::new(&step.dest),
        source_hash,
        step.replacement_sha256.as_deref(),
        owned.get(step.source.as_str()).copied(),
        plan,
    )
}

/// Classify every file in a directory rename, then fail closed on any
/// unexpected file under either root.
fn plan_dir_move(
    vault: &Path,
    intent: &OverwriteIntent,
    step: &PendingMove,
    owned: &BTreeMap<&str, &str>,
    plan: &mut ReversalPlan,
) -> std::result::Result<(), CommandError> {
    let source_root = Path::new(&step.source);
    let dest_root = Path::new(&step.dest);
    let manifest_rels: BTreeSet<&str> =
        step.files.iter().map(|file| file.rel.as_str()).collect();
    for file in &step.files {
        let source = source_root.join(&file.rel);
        let dest = dest_root.join(&file.rel);
        let source_key = vault_slash(&source);
        plan_endpoint_pair(
            vault,
            intent,
            &source,
            &dest,
            &file.sha256,
            None,
            owned.get(source_key.as_str()).copied(),
            plan,
        )?;
    }
    // Anything under the archive root that the manifest does not name is
    // an unexpected new asset and stays as recoverable evidence.
    for found in list_files(vault, dest_root, intent)? {
        if !manifest_rels.contains(found.as_str()) {
            return Err(preserve_message(
                intent,
                &format!(
                    "{} holds an unexpected file ({}); left the intent in place",
                    step.dest, found
                ),
            ));
        }
    }
    // Anything under the live root that is neither a manifest file nor a
    // recorded transaction-owned write is an unexpected late file.
    for found in list_files(vault, source_root, intent)? {
        let full = format!("{}/{}", step.source, found);
        let manifest_hit = manifest_rels.contains(found.as_str());
        let owned_hit = owned.get(full.as_str()).is_some_and(|hash| {
            file_sha256(&vault.join(&full))
                .is_ok_and(|current| current == **hash)
        });
        if !manifest_hit && !owned_hit {
            return Err(preserve_message(
                intent,
                &format!(
                    "{} holds an unexpected file ({}); left the intent in place",
                    step.source, found
                ),
            ));
        }
    }
    Ok(())
}

/// Classify one (source, dest) file pair against the recorded hashes.
/// `intent_hash` is the moved bytes; `replacement` is transaction-owned
/// bytes allowed at the source; `owned_write` is a recorded write hash
/// for the source path (new live assets after a directory move).
fn plan_endpoint_pair(
    vault: &Path,
    intent: &OverwriteIntent,
    source: &Path,
    dest: &Path,
    intent_hash: &str,
    replacement: Option<&str>,
    owned_write: Option<&str>,
    plan: &mut ReversalPlan,
) -> std::result::Result<(), CommandError> {
    let current_source = endpoint_hash(vault, intent, source)?;
    let current_dest = endpoint_hash(vault, intent, dest)?;
    let source_label = vault_slash(source);
    let dest_label = vault_slash(dest);
    match (&current_source, &current_dest) {
        (None, None) => Err(preserve_message(
            intent,
            &format!(
                "both move endpoints are missing ({source_label} and {dest_label}); left the intent in place"
            ),
        )),
        (Some(_), None) => {
            // Pre-move: the source must still hold the intent bytes.
            if current_source.as_deref() == Some(intent_hash) {
                Ok(())
            } else if Some(current_source.as_deref()) == replacement.map(Some)
                || Some(current_source.as_deref()) == owned_write.map(Some)
            {
                Err(preserve_message(
                    intent,
                    &format!(
                        "the archive copy of {source_label} is missing while the live path holds new bytes; left the intent in place"
                    ),
                ))
            } else {
                Err(preserve_message(
                    intent,
                    &format!(
                        "{source_label} changed after the overwrite began; left the intent in place"
                    ),
                ))
            }
        }
        (None, Some(_)) => {
            // Post-move: the destination must still hold the moved bytes.
            if current_dest.as_deref() == Some(intent_hash) {
                plan.reversals.push((
                    vault.join(dest),
                    vault.join(source),
                ));
                Ok(())
            } else {
                Err(preserve_message(
                    intent,
                    &format!(
                        "{dest_label} changed after the overwrite began (a late editor save); left the intent in place"
                    ),
                ))
            }
        }
        (Some(_), Some(_)) => {
            if current_dest.as_deref() == Some(intent_hash)
                && (Some(current_source.as_deref()) == replacement.map(Some)
                    || Some(current_source.as_deref()) == owned_write.map(Some))
            {
                // Published replacement: clear the transaction-owned live
                // bytes, then reverse the proven rename.
                plan.clears.push(vault.join(source));
                plan.reversals.push((
                    vault.join(dest),
                    vault.join(source),
                ));
                Ok(())
            } else if current_dest.as_deref() == Some(intent_hash)
                && current_source.as_deref() == Some(intent_hash)
            {
                // Duplicated original on both sides: the archive copy is
                // redundant and safe to remove.
                plan.duplicates.push(vault.join(dest));
                Ok(())
            } else if current_dest.as_deref() == Some(intent_hash) {
                Err(preserve_message(
                    intent,
                    &format!(
                        "{source_label} changed after the overwrite began; left the intent in place"
                    ),
                ))
            } else {
                Err(preserve_message(
                    intent,
                    &format!(
                        "{dest_label} changed after the overwrite began (a late editor save or a new file); left the intent in place"
                    ),
                ))
            }
        }
    }
}

/// Planned annotated recovery: transaction-owned paths to clear first,
/// proven renames to reverse (in execution order), and duplicate copies
/// to remove.
#[derive(Debug, Default)]
struct ReversalPlan {
    clears: Vec<PathBuf>,
    reversals: Vec<(PathBuf, PathBuf)>,
    duplicates: Vec<PathBuf>,
}

/// Run a validated reversal plan. Clears run first so no-clobber
/// reversals find their live path free; duplicates drop last. Any
/// failure preserves the intent for a retry.
fn execute_reversal_plan(
    vault: &Path,
    intent: &OverwriteIntent,
    plan: &ReversalPlan,
    coordinator: &dyn ReverseCoordinator,
) -> std::result::Result<(), CommandError> {
    for path in &plan.clears {
        fs::remove_file(path).map_err(|error| {
            preserve_message(
                intent,
                &format!(
                    "could not clear transaction-owned {}: {error}",
                    path.strip_prefix(vault).unwrap_or(path).display()
                ),
            )
        })?;
    }
    for (from, to) in &plan.reversals {
        coordinator
            .reverse_file(vault, from, to)
            .map_err(|error| preserve_message(intent, &error))?;
    }
    for path in &plan.duplicates {
        fs::remove_file(path).map_err(|error| {
            preserve_message(
                intent,
                &format!(
                    "could not remove duplicated {}: {error}",
                    path.strip_prefix(vault).unwrap_or(path).display()
                ),
            )
        })?;
    }
    // Prune move-destination directories left empty by file reversals so
    // a retried recovery sees the same pre-move shape.
    for step in &intent.moves {
        if !step.is_file() {
            remove_empty_parents(&vault.join(&step.dest), vault);
        }
    }
    Ok(())
}

/// Reads the current bytes at a vault-relative path: `None` when absent.
/// Symlinks and directories where a file is expected fail closed.
fn endpoint_hash(
    vault: &Path,
    intent: &OverwriteIntent,
    relative: &Path,
) -> std::result::Result<Option<String>, CommandError> {
    let absolute = vault.join(relative);
    match fs::symlink_metadata(&absolute) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(preserve_message(
            intent,
            &format!("could not inspect {}: {error}", vault_slash(relative)),
        )),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(preserve_message(
                    intent,
                    &format!(
                        "{} is unexpectedly a symlink; left the intent in place",
                        vault_slash(relative)
                    ),
                ));
            }
            if !metadata.file_type().is_file() {
                return Err(preserve_message(
                    intent,
                    &format!(
                        "{} is unexpectedly a {}; left the intent in place",
                        vault_slash(relative),
                        path_kind(&metadata),
                    ),
                ));
            }
            file_sha256(&absolute)
                .map(Some)
                .map_err(|error| preserve_message(intent, &error))
        }
    }
}

fn path_kind(metadata: &fs::Metadata) -> &'static str {
    if metadata.file_type().is_dir() {
        "directory"
    } else {
        "special file"
    }
}

fn file_sha256(path: &Path) -> std::result::Result<String, String> {
    let bytes = fs::read(path).map_err(|error| {
        format!("could not read {}: {error}", path.display())
    })?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn vault_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Vault-relative `/`-separated file paths under `root` (empty when the
/// root is absent). Symlinks fail closed; other I/O errors do too.
fn list_files(
    vault: &Path,
    root: &Path,
    intent: &OverwriteIntent,
) -> std::result::Result<Vec<String>, CommandError> {
    let absolute = vault.join(root);
    let mut files = Vec::new();
    let mut stack = vec![(absolute.clone(), String::new())];
    let mut descended = false;
    while let Some((directory, prefix)) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if descended || directory != absolute {
                    continue;
                }
                return Ok(Vec::new());
            }
            Err(error) => {
                return Err(preserve_message(
                    intent,
                    &format!("could not scan {}: {error}", vault_slash(root)),
                ));
            }
        };
        descended = true;
        for entry in entries {
            let entry = entry.map_err(|error| {
                preserve_message(
                    intent,
                    &format!("could not scan {}: {error}", vault_slash(root)),
                )
            })?;
            let file_type = entry.file_type().map_err(|error| {
                preserve_message(
                    intent,
                    &format!("could not scan {}: {error}", vault_slash(root)),
                )
            })?;
            if file_type.is_symlink() {
                return Err(preserve_message(
                    intent,
                    &format!(
                        "{} holds an unexpected symlink; left the intent in place",
                        vault_slash(root)
                    ),
                ));
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if file_type.is_dir() {
                stack.push((entry.path(), rel));
            } else if file_type.is_file() {
                files.push(rel);
            } else {
                return Err(preserve_message(
                    intent,
                    &format!(
                        "{} holds an unexpected special file; left the intent in place",
                        vault_slash(root)
                    ),
                ));
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Version-2 created-file ownership: every existing created file that is
/// not a move destination must carry recorded transaction-owned bytes.
/// A user-created destination at a transaction path fails closed here,
/// before generic cleanup could delete it.
fn verify_created_ownership(
    vault: &Path,
    intent: &OverwriteIntent,
) -> std::result::Result<(), CommandError> {
    let owned = write_hashes(intent);
    let mut move_dest_files: BTreeSet<String> = BTreeSet::new();
    for step in &intent.moves {
        if step.is_file() {
            move_dest_files.insert(step.dest.clone());
        } else {
            for file in &step.files {
                move_dest_files.insert(format!("{}/{}", step.dest, file.rel));
            }
        }
    }
    for raw in &intent.created {
        if move_dest_files.contains(raw.as_str()) {
            continue;
        }
        let absolute = vault.join(raw);
        let metadata = match fs::symlink_metadata(&absolute) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => {
                return Err(preserve_message(
                    intent,
                    &format!("could not inspect created {raw}: {error}"),
                ));
            }
            Ok(metadata) => metadata,
        };
        if metadata.file_type().is_dir() {
            continue;
        }
        match owned.get(raw.as_str()) {
            Some(expected) => {
                let current = file_sha256(&absolute)
                    .map_err(|error| preserve_message(intent, &error))?;
                if current != **expected {
                    return Err(preserve_message(
                        intent,
                        &format!(
                            "created {raw} changed after the overwrite began; left the intent in place"
                        ),
                    ));
                }
            }
            None => {
                return Err(preserve_message(
                    intent,
                    &format!(
                        "created {raw} has no recorded transaction bytes (a destination race or an unrecorded write); left the intent in place"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Rename without overwriting: the destination parent is created, the
/// destination itself must be absent, and both endpoints must sit on the
/// same filesystem. Never clobbers, never follows a destination symlink
/// (an existing one refuses first).
fn rename_no_clobber(
    from: &Path,
    to: &Path,
) -> std::result::Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("could not create {}: {error}", parent.display())
        })?;
    }
    if fs::symlink_metadata(to).is_ok() {
        return Err(format!("refusing to overwrite existing {}", to.display()));
    }
    if !same_filesystem(from, to)? {
        return Err(format!(
            "refusing to move {} across filesystems; run bob ref scan on this host",
            from.display()
        ));
    }
    fs::rename(from, to).map_err(|error| {
        format!(
            "could not rename {} to {}: {error}",
            from.display(),
            to.display()
        )
    })?;
    if fs::symlink_metadata(from).is_ok() {
        return Err(format!(
            "rename left the source in place: {}",
            from.display()
        ));
    }
    if fs::symlink_metadata(to).is_err() {
        return Err(format!(
            "rename did not produce the destination: {}",
            to.display()
        ));
    }
    Ok(())
}

/// True when the nearest existing ancestors of both paths share a
/// filesystem. Recovery refuses cross-filesystem moves because they are
/// copy-and-delete, not atomic renames.
fn same_filesystem(
    from: &Path,
    to: &Path,
) -> std::result::Result<bool, String> {
    #[cfg(unix)]
    {
        let from_dev = nearest_dev(from)?;
        let to_dev = nearest_dev(to)?;
        Ok(from_dev == to_dev)
    }
    #[cfg(not(unix))]
    {
        let _ = (from, to);
        Ok(true)
    }
}

#[cfg(unix)]
fn nearest_dev(path: &Path) -> std::result::Result<u64, String> {
    use std::os::unix::fs::MetadataExt;
    let mut current = path.to_path_buf();
    loop {
        match fs::metadata(&current) {
            Ok(metadata) => return Ok(metadata.dev()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match current.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => {
                        current = parent.to_path_buf();
                    }
                    _ => {
                        return Err(format!(
                            "could not find an existing ancestor of {}",
                            path.display()
                        ));
                    }
                }
            }
            Err(error) => {
                return Err(format!(
                    "could not inspect {}: {error}",
                    current.display()
                ));
            }
        }
    }
}

/// Restore `touched` from the recorded base, remove `created`, and unstage
/// exactly those paths. Unrelated working-tree and index content is left
/// untouched; any failure preserves the intent for a retry.
fn restore_from_base(
    vault: &Path,
    child_env: &ob::ChildEnv,
    intent: &OverwriteIntent,
    touched: &[PathBuf],
    created: &[PathBuf],
) -> std::result::Result<(), CommandError> {
    let restore = paths_tracked_in_commit(
        vault,
        child_env,
        &intent.base_head,
        touched,
        intent,
    )?;
    if !restore.is_empty() {
        let status = ob::git_command(vault, child_env)
            .arg("checkout")
            .arg(&intent.base_head)
            .arg("--")
            .args(&restore)
            .status()
            .map_err(|error| {
                preserve_message(
                    intent,
                    &format!("could not restore vault files: {error}"),
                )
            })?;
        if !status.success() {
            return Err(preserve_message(
                intent,
                &format!(
                    "could not restore vault files from {}",
                    intent.base_head
                ),
            ));
        }
    }
    let mut removals = Vec::new();
    for path in created {
        if remove_created_path(
            vault,
            path,
            &BTreeSet::from_iter(created.iter().cloned()),
        )
        .is_err()
        {
            removals.push(reference_mutation::vault_path(path));
        }
    }
    if !removals.is_empty() {
        return Err(preserve_message(
            intent,
            &format!(
                "could not remove created path(s): {}",
                removals.join(", ")
            ),
        ));
    }
    let mut staged: Vec<PathBuf> = touched.to_vec();
    staged.extend(created.iter().cloned());
    if !staged.is_empty()
        && let Err(error) =
            reference_mutation::unstage_paths(vault, child_env, &staged)
    {
        return Err(preserve_message(
            intent,
            &format!("could not unstage recovered paths: {error}"),
        ));
    }
    if !restore.is_empty() {
        let differs = ob::git_command(vault, child_env)
            .args(["diff", "--quiet", &intent.base_head, "--"])
            .args(&restore)
            .status()
            .map(|status| !status.success())
            .unwrap_or(true);
        if differs {
            return Err(preserve_message(
                intent,
                "restored files still differ from the recorded base",
            ));
        }
    }
    Ok(())
}

/// Paths in `candidates` that the recorded base commit actually tracks.
/// Recovery never asks `git checkout` for a path absent from base (an
/// ignored sidecar file, an untracked leftover, a stale intent entry):
/// those would fail the whole restore.
fn paths_tracked_in_commit(
    vault: &Path,
    child_env: &ob::ChildEnv,
    base: &str,
    candidates: &[PathBuf],
    intent: &OverwriteIntent,
) -> std::result::Result<Vec<PathBuf>, CommandError> {
    let mut tracked = Vec::new();
    for path in candidates {
        let output = ob::git_command(vault, child_env)
            .args(["ls-tree", "--name-only", base, "--"])
            .arg(path)
            .output()
            .map_err(|error| {
                preserve_message(
                    intent,
                    &format!("could not inspect recorded base paths: {error}"),
                )
            })?;
        if !output.status.success() {
            return Err(preserve_message(
                intent,
                &format!("could not inspect recorded base {}", base),
            ));
        }
        if !String::from_utf8_lossy(&output.stdout).trim().is_empty() {
            tracked.push(path.clone());
        }
    }
    Ok(tracked)
}

/// Remove one recorded creation: files and symlinks directly, directories
/// only by deleting listed files inside them (then empty parents), so
/// untracked contents the transaction never recorded survive. Missing
/// paths are already restored.
fn remove_created_path(
    vault: &Path,
    path: &Path,
    created: &BTreeSet<PathBuf>,
) -> std::io::Result<()> {
    let absolute = vault.join(path);
    let symlink_type = fs::symlink_metadata(&absolute);
    if symlink_type.is_err() {
        return Ok(());
    }
    // Symlinks (even ones pointing at directories) are removed directly
    // and never traversed. Newly emptied parents are pruned (only while
    // empty, so pre-existing contents always stop the climb).
    let is_real_dir = symlink_type
        .as_ref()
        .is_ok_and(|metadata| metadata.is_dir());
    if !is_real_dir {
        remove_file_or_link(&absolute)?;
        if let Some(parent) = absolute.parent() {
            remove_empty_parents(parent, vault);
        }
        return Ok(());
    }
    // A recorded directory: delete only recorded files beneath it, then
    // prune parents that are left empty.
    let mut stack = vec![absolute.clone()];
    let mut files = Vec::new();
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let entry_path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() && !file_type.is_symlink() {
                stack.push(entry_path);
            } else {
                files.push(entry_path);
            }
        }
    }
    for file in &files {
        let relative = file.strip_prefix(vault).unwrap_or(file).to_path_buf();
        if created.contains(&relative) {
            remove_file_or_link(file)?;
        }
    }
    remove_empty_parents(&absolute, vault);
    Ok(())
}

fn remove_file_or_link(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Remove `directory` and its ancestors up to (not including) `vault`
/// while they are empty.
fn remove_empty_parents(directory: &Path, vault: &Path) {
    let mut current = directory.to_path_buf();
    while current != vault
        && current.starts_with(vault)
        && fs::remove_dir(&current).is_ok()
    {
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
}

/// Reject vault-escaping or non-relative intent paths without touching the
/// vault. Empty paths, `.`, any `CurDir` component, and anything that does
/// not name a path below the vault root are refused so recovery never runs
/// `git checkout <base> -- .`. `.git` roots are rejected outright.
fn validated_paths(
    paths: &[String],
) -> std::result::Result<Vec<PathBuf>, CommandError> {
    let mut validated = Vec::with_capacity(paths.len());
    for raw in paths {
        let path = Path::new(raw);
        let invalid = || {
            CommandError::new(format!(
                "ignoring malformed overwrite intent with invalid path {raw}; remove it to proceed"
            ))
        };
        if raw.is_empty() || path.is_absolute() {
            return Err(invalid());
        }
        // `Path::components()` skips interior `.` (so `ref/./note.md`
        // would otherwise name a real vault file). Reject any CurDir
        // spelling in the raw path, including `./`, `/./`, and `/.`.
        if raw.split(['/', '\\']).any(|part| part == ".") {
            return Err(invalid());
        }
        let mut depth = 0usize;
        let mut under_git = false;
        for component in path.components() {
            match component {
                Component::Normal(name) => {
                    if name.eq_ignore_ascii_case(".git") {
                        under_git = true;
                    }
                    depth += 1;
                }
                Component::CurDir
                | Component::ParentDir
                | Component::Prefix(_)
                | Component::RootDir => {
                    return Err(invalid());
                }
            }
        }
        if depth == 0 || under_git {
            return Err(invalid());
        }
        validated.push(path.to_path_buf());
    }
    Ok(validated)
}

fn recovery_required(intent: &OverwriteIntent) -> CommandError {
    CommandError::new(format!(
        "recovery required: an interrupted overwrite of {} is pending; run a writing command without --dry-run to recover",
        intent.stem
    ))
    .with_code(RECOVERY_REQUIRED_CODE)
}

/// Wrap a repair failure so the report states the marker was preserved.
fn preserve_message(intent: &OverwriteIntent, detail: &str) -> CommandError {
    CommandError::new(format!(
        "could not recover the interrupted overwrite of {}: {detail}; left the intent in place",
        intent.stem
    ))
}

fn state_ref_dir() -> PathBuf {
    bob_env::bob_cli_state_dir().join("ref")
}

fn canonical_vault(
    config: &Config,
) -> std::result::Result<String, CommandError> {
    fs::canonicalize(&config.bob_dir)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| {
            CommandError::new(format!(
                "could not resolve the vault path {}: {error}",
                config.bob_dir.display()
            ))
        })
}

/// File holding the pending intent for one canonical vault.
fn intent_path_for(state_dir: &Path, canonical_vault: &str) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(canonical_vault.as_bytes());
    let hex = hex::encode(digest.finalize());
    state_dir
        .join(INTENT_DIR_NAME)
        .join(format!("overwrite-{hex}.json"))
}

fn intent_display_path(state_dir: &Path, canonical_vault: &str) -> PathBuf {
    intent_path_for(state_dir, canonical_vault)
}

fn save_intent_to(
    state_dir: &Path,
    config: &Config,
    intent: &OverwriteIntent,
) -> std::result::Result<PathBuf, String> {
    let canonical = fs::canonicalize(&config.bob_dir).map_err(|error| {
        format!("could not resolve the vault path: {error}")
    })?;
    let mut stored = intent.clone();
    stored.schema_version = INTENT_SCHEMA_VERSION;
    stored.vault = canonical.to_string_lossy().into_owned();
    let path = intent_path_for(state_dir, &stored.vault);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("could not create {}: {error}", parent.display())
        })?;
    }
    let text = serde_json::to_string(&stored).map_err(|error| {
        format!("could not serialize overwrite intent: {error}")
    })?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, text.as_bytes()).map_err(|error| {
        format!("could not write {}: {error}", temporary.display())
    })?;
    fs::File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            format!("could not fsync {}: {error}", temporary.display())
        })?;
    fs::rename(&temporary, &path).map_err(|error| {
        format!("could not install {}: {error}", path.display())
    })?;
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(path)
}

fn load_intent_from(
    state_dir: &Path,
    config: &Config,
) -> std::result::Result<Option<OverwriteIntent>, CommandError> {
    let canonical = canonical_vault(config)?;
    let path = intent_path_for(state_dir, &canonical);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(CommandError::new(format!(
                "could not read overwrite intent {}: {error}; remove it to proceed",
                path.display()
            )));
        }
    };
    match serde_json::from_str::<OverwriteIntent>(&text) {
        Ok(intent) => Ok(Some(intent)),
        Err(error) => Err(CommandError::new(format!(
            "ignoring malformed overwrite intent at {} ({error}); remove it to proceed",
            path.display()
        ))),
    }
}

fn clear_intent_in(
    state_dir: &Path,
    canonical_vault: &str,
) -> std::result::Result<(), String> {
    let path = intent_path_for(state_dir, canonical_vault);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not clear overwrite intent {}: {error}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const STEM: &str = "half-written";
    const REF_TYPE: &str = "blogs";
    const SUBJECT: &str = "bob ref create: overwrite blogs/half-written";

    fn run_git(vault: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_output(vault: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo(vault: &Path) {
        run_git(vault, &["init", "-q"]);
        run_git(vault, &["config", "user.email", "test@example.com"]);
        run_git(vault, &["config", "user.name", "Test"]);
        run_git(vault, &["config", "commit.gpgsign", "false"]);
    }

    fn test_config(vault: &Path) -> Config {
        Config {
            bob_dir: vault.to_path_buf(),
            lib_dir: vault.join("lib"),
            ref_dir: vault.join("ref"),
            xlib_dir: vault.join("xlib"),
        }
    }

    fn fixture_vault() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        for directory in ["lib/blogs", "ref/blogs", "xlib"] {
            fs::create_dir_all(vault.join(directory)).expect("vault dirs");
        }
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nbody\n",
        )
        .expect("note");
        fs::write(vault.join("lib/blogs/half-written.pdf"), b"pdf-bytes")
            .expect("pdf");
        init_repo(vault);
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", "fixture"]);
        temp
    }

    fn pending_intent(
        base_head: &str,
        expected_head: Option<&str>,
        touched: &[&str],
        created: &[&str],
    ) -> OverwriteIntent {
        OverwriteIntent {
            schema_version: INTENT_SCHEMA_VERSION,
            vault: String::new(),
            stem: STEM.to_string(),
            ref_type: REF_TYPE.to_string(),
            base_head: base_head.to_string(),
            expected_subject: SUBJECT.to_string(),
            expected_head: expected_head.map(str::to_string),
            touched: touched.iter().map(ToString::to_string).collect(),
            created: created.iter().map(ToString::to_string).collect(),
            moves: Vec::new(),
            writes: Vec::new(),
        }
    }

    fn annotated_intent(
        base_head: &str,
        touched: &[&str],
        created: &[&str],
        moves: Vec<PendingMove>,
        writes: Vec<PendingWrite>,
    ) -> OverwriteIntent {
        let mut intent = pending_intent(base_head, None, touched, created);
        intent.moves = moves;
        intent.writes = writes;
        intent
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn write_file(vault: &Path, relative: &str, contents: &[u8]) {
        let absolute = vault.join(relative);
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent).expect("parent dirs");
        }
        fs::write(&absolute, contents).expect("write fixture file");
    }

    fn save_pending(
        state: &Path,
        config: &Config,
        intent: &OverwriteIntent,
    ) -> OverwriteIntent {
        let mut stored = intent.clone();
        stored.vault = fs::canonicalize(&config.bob_dir)
            .expect("canonical vault")
            .to_string_lossy()
            .into_owned();
        save_intent_to(state, config, &stored).expect("save intent");
        stored
    }

    fn intent_file(state: &Path, config: &Config) -> PathBuf {
        let canonical = fs::canonicalize(&config.bob_dir)
            .expect("canonical vault")
            .to_string_lossy()
            .into_owned();
        intent_path_for(state, &canonical)
    }

    fn recover(
        state: &Path,
        config: &Config,
        intent: &OverwriteIntent,
        dry_run: bool,
    ) -> std::result::Result<Option<String>, CommandError> {
        let child_env = ob::child_env();
        recover_locked(
            state,
            config,
            &child_env,
            intent,
            dry_run,
            &DirectReverseCoordinator,
        )
    }

    fn recover_with(
        state: &Path,
        config: &Config,
        intent: &OverwriteIntent,
        coordinator: &dyn ReverseCoordinator,
    ) -> std::result::Result<Option<String>, CommandError> {
        let child_env = ob::child_env();
        recover_locked(state, config, &child_env, intent, false, coordinator)
    }

    #[test]
    fn rollback_restores_half_written_files_and_removes_creations() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        fs::write(vault.join("lib/blogs/half-written.pdf"), b"crashed-pdf")
            .expect("dirty pdf");
        fs::create_dir_all(vault.join("lib/blogs/half-written.textbundle"))
            .expect("sidecar dir");
        fs::write(
            vault.join("lib/blogs/half-written.textbundle/text.md"),
            "sidecar\n",
        )
        .expect("sidecar file");
        fs::create_dir_all(vault.join("ref_history/blogs"))
            .expect("archive dir");
        fs::write(
            vault.join("ref_history/blogs/half-written__2026-10-26.md"),
            "frozen\n",
        )
        .expect("archive");
        // Unrelated dirty and staged content must survive scoped recovery.
        fs::write(vault.join("unrelated.md"), "unrelated-dirty\n")
            .expect("unrelated");
        run_git(vault, &["add", "unrelated.md"]);

        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                None,
                &["ref/blogs/half-written.md", "lib/blogs/half-written.pdf"],
                &[
                    "ref_history/blogs/half-written__2026-10-26.md",
                    "lib/blogs/half-written.textbundle/text.md",
                ],
            ),
        );
        let notice = recover(state, &config, &intent, false).expect("rollback");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: rolled back an interrupted overwrite of half-written")
        );
        assert_eq!(
            fs::read(vault.join("ref/blogs/half-written.md")).expect("note"),
            b"---\nid: half-written\n---\nbody\n"
        );
        assert_eq!(
            fs::read(vault.join("lib/blogs/half-written.pdf")).expect("pdf"),
            b"pdf-bytes"
        );
        assert!(
            !vault
                .join("ref_history/blogs/half-written__2026-10-26.md")
                .exists(),
            "the recorded archive creation is removed"
        );
        assert!(
            !vault.join("lib/blogs/half-written.textbundle").exists(),
            "the emptied sidecar directory is pruned"
        );
        assert!(
            !intent_file(state, &config).exists(),
            "the intent is cleared after a proved rollback"
        );
        assert_eq!(
            fs::read(vault.join("unrelated.md")).expect("unrelated"),
            b"unrelated-dirty\n"
        );
        assert_eq!(
            git_output(vault, &["diff", "--cached", "--name-only"]),
            "unrelated.md"
        );
        // Recovery is idempotent: the intent is gone, so a fresh load finds
        // nothing to do.
        assert!(load_intent_from(state, &config).expect("reload").is_none());
    }

    #[test]
    fn committed_intent_is_retained_and_cleared() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nreplaced\n",
        )
        .expect("new body");
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", SUBJECT]);
        let committed = git_output(vault, &["rev-parse", "HEAD"]);

        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                Some(&committed),
                &["ref/blogs/half-written.md"],
                &[],
            ),
        );
        let notice = recover(state, &config, &intent, false).expect("retain");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: kept the completed overwrite of half-written")
        );
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("replaced"),
            "the committed state is kept"
        );
        assert!(
            !intent_file(state, &config).exists(),
            "the intent is cleared once the commit is proved"
        );
    }

    #[test]
    fn committed_without_recorded_sha_still_retains() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // The transaction committed but crashed before recording the SHA:
        // HEAD advanced past the base carrying the expected subject.
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nreplaced\n",
        )
        .expect("new body");
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", SUBJECT]);

        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &["ref/blogs/half-written.md"], &[]),
        );
        let notice = recover(state, &config, &intent, false).expect("retain");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: kept the completed overwrite of half-written")
        );
        assert!(!intent_file(state, &config).exists());
    }

    #[test]
    fn previous_same_stem_commit_is_not_clobbered_by_rollback() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();

        // An earlier overwrite committed; the new overwrite crashed before
        // committing, so recovery must restore the earlier commit's state.
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nfirst-overwrite\n",
        )
        .expect("first overwrite");
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", SUBJECT]);
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("crashed second overwrite");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &["ref/blogs/half-written.md"], &[]),
        );
        let notice = recover(state, &config, &intent, false).expect("rollback");
        assert!(notice
            .expect("notice")
            .contains("rolled back an interrupted overwrite"));
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("first-overwrite"),
            "rollback restores the previous overwrite, it does not erase it"
        );
    }

    #[test]
    fn moved_vault_preserves_the_intent_without_touching_files() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // An unrelated commit advanced HEAD past the base without the
        // expected subject: recovery must fail closed.
        fs::write(vault.join("unrelated.md"), "unrelated\n").expect("file");
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", "unrelated work"]);
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");

        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &["ref/blogs/half-written.md"], &[]),
        );
        let error =
            recover(state, &config, &intent, false).expect_err("moved vault");
        assert!(
            error.to_string().contains("left the intent in place"),
            "unexpected error: {error}"
        );
        assert!(
            intent_file(state, &config).exists(),
            "the marker is preserved when repair cannot proceed"
        );
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("CRASHED"),
            "no restore runs when HEAD moved"
        );
    }

    #[test]
    fn wrong_subject_expected_head_rolls_back() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // The recorded SHA exists but carries another subject, so it cannot
        // prove this overwrite committed.
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                Some(&base),
                &["ref/blogs/half-written.md"],
                &[],
            ),
        );
        let notice = recover(state, &config, &intent, false).expect("rollback");
        assert!(notice
            .expect("notice")
            .contains("rolled back an interrupted overwrite"));
        assert!(!fs::read_to_string(vault.join("ref/blogs/half-written.md"))
            .expect("note")
            .contains("CRASHED"));
    }

    #[test]
    fn dry_run_is_read_only_and_reports_recovery_required() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &["ref/blogs/half-written.md"], &[]),
        );
        let error =
            recover(state, &config, &intent, true).expect_err("dry run");
        assert_eq!(error.code, Some(RECOVERY_REQUIRED_CODE));
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("CRASHED"),
            "dry runs restore nothing"
        );
        assert!(
            intent_file(state, &config).exists(),
            "dry runs clear nothing"
        );
    }

    #[test]
    fn invalid_paths_fail_closed() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        for invalid in [
            "../evil.md",
            "/absolute.md",
            "ref/blogs/../../evil.md",
            "",
            ".",
            "./ref/blogs/half-written.md",
            "ref/./blogs/half-written.md",
        ] {
            let intent = save_pending(
                state,
                &config,
                &pending_intent(&base, None, &[invalid], &[]),
            );
            let error =
                recover(state, &config, &intent, false).expect_err("invalid");
            assert!(
                error.to_string().contains("invalid path"),
                "unexpected error for {invalid:?}: {error}"
            );
            assert!(
                intent_file(state, &config).exists(),
                "invalid intents are preserved for manual cleanup"
            );
            assert!(!vault.join("evil.md").exists());
        }
        // `.git` roots are never restored or cleaned.
        let intent = save_pending(
            state,
            &config,
            &pending_intent(&base, None, &[".git/refs/heads/main"], &[]),
        );
        recover(state, &config, &intent, false).expect_err(".git refused");
        assert!(intent_file(state, &config).exists());
    }

    #[test]
    fn malformed_intent_fails_closed() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();

        let canonical = fs::canonicalize(vault)
            .expect("canonical vault")
            .to_string_lossy()
            .into_owned();
        let path = intent_path_for(state, &canonical);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(&path, "{not json").expect("malformed marker");

        let error = load_intent_from(state, &config).expect_err("malformed");
        assert!(
            error.to_string().contains("malformed overwrite intent"),
            "unexpected error: {error}"
        );
        assert!(path.exists(), "malformed markers are never deleted");
    }

    #[test]
    fn bogus_base_preserves_the_intent() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                "0".repeat(40).as_str(),
                None,
                &["ref/blogs/half-written.md"],
                &[],
            ),
        );
        let error =
            recover(state, &config, &intent, false).expect_err("repair");
        assert!(
            error.to_string().contains("left the intent in place"),
            "unexpected error: {error}"
        );
        assert!(intent_file(state, &config).exists());
        assert!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note")
                .contains("CRASHED"),
            "failed repairs leave the worktree alone"
        );
    }

    #[test]
    fn restore_skips_paths_absent_from_the_base_commit() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // A touched path absent from the base (gitignored sidecar junk,
        // a stale intent entry) must not fail the whole restore.
        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                None,
                &["ref/blogs/half-written.md", "ref/blogs/no-such-file.md"],
                &[],
            ),
        );
        let notice = recover(state, &config, &intent, false).expect("restore");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: rolled back an interrupted overwrite of half-written")
        );
        assert_eq!(
            fs::read_to_string(vault.join("ref/blogs/half-written.md"))
                .expect("note"),
            "---\nid: half-written\n---\nbody\n"
        );
        assert!(
            !intent_file(state, &config).exists(),
            "a skipped absent path still clears the intent"
        );
    }

    #[test]
    fn validated_paths_rejects_empty_curdir_and_dot_components() {
        for invalid in ["", ".", "./foo", "foo/./bar", "foo/../bar", "/abs"] {
            let error =
                validated_paths(&[invalid.to_string()]).expect_err(invalid);
            assert!(
                error.to_string().contains("invalid path"),
                "unexpected error for {invalid:?}: {error}"
            );
        }
        assert!(
            validated_paths(&["ref/blogs/note.md".into()]).is_ok(),
            "a vault-relative file is accepted"
        );
    }

    #[test]
    fn created_directories_keep_unrecorded_contents() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        // The transaction recorded only the sidecar file; a user file that
        // landed in the same directory afterwards must survive.
        fs::create_dir_all(vault.join("lib/blogs/half-written.textbundle"))
            .expect("sidecar dir");
        fs::write(
            vault.join("lib/blogs/half-written.textbundle/text.md"),
            "sidecar\n",
        )
        .expect("sidecar file");
        fs::write(
            vault.join("lib/blogs/half-written.textbundle/user-kept.md"),
            "user\n",
        )
        .expect("user file");

        let intent = save_pending(
            state,
            &config,
            &pending_intent(
                &base,
                None,
                &[],
                &[
                    "lib/blogs/half-written.textbundle",
                    "lib/blogs/half-written.textbundle/text.md",
                ],
            ),
        );
        recover(state, &config, &intent, false).expect("rollback");
        assert!(
            !vault
                .join("lib/blogs/half-written.textbundle/text.md")
                .exists(),
            "recorded creations are removed"
        );
        assert_eq!(
            fs::read(
                vault.join("lib/blogs/half-written.textbundle/user-kept.md")
            )
            .expect("user file"),
            b"user\n",
            "unrecorded contents survive directory cleanup"
        );
    }

    // ---- Annotated (version-2) recovery: synthetic interruptions ----

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FakeReverseBehavior {
        Grant,
        Hold,
        Fail,
    }

    struct FakeReverseCoordinator {
        behavior: FakeReverseBehavior,
        message: String,
        calls: std::sync::Mutex<usize>,
    }

    impl FakeReverseCoordinator {
        fn grant() -> Self {
            Self {
                behavior: FakeReverseBehavior::Grant,
                message: String::new(),
                calls: std::sync::Mutex::new(0),
            }
        }

        fn hold() -> Self {
            Self {
                behavior: FakeReverseBehavior::Hold,
                message: String::new(),
                calls: std::sync::Mutex::new(0),
            }
        }

        fn fail(message: &str) -> Self {
            Self {
                behavior: FakeReverseBehavior::Fail,
                message: message.to_string(),
                calls: std::sync::Mutex::new(0),
            }
        }

        fn calls(&self) -> usize {
            *self.calls.lock().expect("fake coordinator calls")
        }
    }

    impl ReverseCoordinator for FakeReverseCoordinator {
        fn reverse_file(
            &self,
            _vault: &Path,
            from: &Path,
            to: &Path,
        ) -> std::result::Result<(), String> {
            *self.calls.lock().expect("fake coordinator calls") += 1;
            match self.behavior {
                FakeReverseBehavior::Grant => rename_no_clobber(from, to),
                FakeReverseBehavior::Hold => Err(
                    "PDF coordination timed out after 10.00s; close it in Highlights and retry"
                        .to_string(),
                ),
                FakeReverseBehavior::Fail => Err(self.message.clone()),
            }
        }
    }

    const PAPER_SUBJECT: &str = "bob ref create: overwrite blogs/paper";
    const LIVE_NOTE: &str = "ref/blogs/paper.md";
    const LIVE_PDF: &str = "lib/blogs/paper.pdf";
    const LIVE_ASSETS: &str = "ref/blogs/paper.assets";
    const LIVE_ASSET_FILE: &str = "ref/blogs/paper.assets/shot.png";
    const ARCHIVE_NOTE: &str = "ref_history/blogs/paper__2026-10-26.md";
    const ARCHIVE_PDF: &str =
        "ref_history/blogs/paper__2026-10-26__annotated.pdf";
    const ARCHIVE_ASSETS: &str = "ref_history/blogs/paper__2026-10-26.assets";
    const OLD_PDF: &[u8] = b"old-pdf-bytes";
    const NEW_PDF: &[u8] = b"new-pdf-bytes";
    const OLD_IMG: &[u8] = b"img-bytes";
    const FROZEN_NOTE: &[u8] = b"---\ntitle: Paper\n---\nfrozen\n";
    const LIVE_BODY: &str = "---\nid: paper\n---\nbody\n";

    fn annotated_fixture(
    ) -> (tempfile::TempDir, tempfile::TempDir, Config, String) {
        let vault_temp = tempfile::tempdir().expect("scratch vault");
        let vault = vault_temp.path();
        for directory in ["lib/blogs", "ref/blogs", "xlib"] {
            fs::create_dir_all(vault.join(directory)).expect("vault dirs");
        }
        write_file(vault, LIVE_NOTE, LIVE_BODY.as_bytes());
        write_file(vault, LIVE_PDF, OLD_PDF);
        write_file(vault, LIVE_ASSET_FILE, OLD_IMG);
        init_repo(vault);
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", "fixture"]);
        let base = git_output(vault, &["rev-parse", "HEAD"]);
        let state_temp = tempfile::tempdir().expect("state root");
        let config = test_config(vault);
        (vault_temp, state_temp, config, base)
    }

    fn state_dir_of(state: &tempfile::TempDir) -> PathBuf {
        state.path().to_path_buf()
    }

    /// Build the version-2 intent an annotated transaction would persist
    /// before its first rename: the archive bundle allocates the
    /// destinations, the PDF move reuses the live path for the staged
    /// replacement, and the archive note is a recorded write.
    fn annotated_intent_via_builder(
        config: &Config,
        base: &str,
        manifest: Vec<PendingMoveFile>,
    ) -> (OverwriteIntent, String, String, String) {
        let vault = &config.bob_dir;
        let bundle = super::super::archive::allocate_bundle(
            vault,
            "blogs",
            "paper",
            "2026-10-26",
        );
        assert_eq!(bundle.note_path, ARCHIVE_NOTE);
        assert_eq!(bundle.annotated_pdf_path, ARCHIVE_PDF);
        assert_eq!(bundle.assets_path, ARCHIVE_ASSETS);
        let old_hash = sha256_hex(OLD_PDF);
        let new_hash = sha256_hex(NEW_PDF);
        let frozen_hash = sha256_hex(FROZEN_NOTE);
        let moves = vec![
            pdf_move_for_archive(&bundle, LIVE_PDF, &old_hash, Some(&new_hash)),
            assets_move_for_archive(&bundle, LIVE_ASSETS, manifest),
        ];
        let writes = vec![PendingWrite {
            path: ARCHIVE_NOTE.to_string(),
            sha256: frozen_hash.clone(),
        }];
        let preimages = vec![
            super::super::FilePreimage {
                path: LIVE_NOTE.to_string(),
                sha256: Some(sha256_hex(LIVE_BODY.as_bytes())),
            },
            super::super::FilePreimage {
                path: LIVE_PDF.to_string(),
                sha256: Some(old_hash.clone()),
            },
            super::super::FilePreimage {
                path: LIVE_ASSET_FILE.to_string(),
                sha256: Some(sha256_hex(OLD_IMG)),
            },
            super::super::FilePreimage {
                path: ARCHIVE_NOTE.to_string(),
                sha256: None,
            },
        ];
        let intent = OverwriteIntent::for_transaction(
            config,
            "paper",
            "blogs",
            base,
            PAPER_SUBJECT.to_string(),
            &preimages,
        )
        .expect("annotated intent")
        .with_annotated_moves(moves, writes);
        (intent, old_hash, new_hash, frozen_hash)
    }

    /// Crash after the transaction renamed the PDF and assets and
    /// published the replacement: the archive holds the only annotated
    /// copy and the live path holds transaction-owned bytes.
    fn simulate_post_move_crash(vault: &Path) {
        fs::create_dir_all(vault.join("ref_history/blogs"))
            .expect("archive dir");
        fs::rename(vault.join(LIVE_PDF), vault.join(ARCHIVE_PDF))
            .expect("move pdf");
        fs::rename(vault.join(LIVE_ASSETS), vault.join(ARCHIVE_ASSETS))
            .expect("move assets");
        write_file(vault, LIVE_PDF, NEW_PDF);
        write_file(vault, ARCHIVE_NOTE, FROZEN_NOTE);
        write_file(vault, LIVE_NOTE, b"---\nid: paper\n---\nCRASHED\n");
    }

    fn assert_failed_closed(
        error: &CommandError,
        state: &tempfile::TempDir,
        config: &Config,
    ) {
        assert!(
            error.to_string().contains("left the intent in place"),
            "unexpected error: {error}"
        );
        assert!(
            intent_file(&state_dir_of(state), config).exists(),
            "the marker is preserved when repair cannot proceed"
        );
    }

    #[test]
    fn annotated_pdf_and_assets_reverse_after_crash() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        assert_eq!(manifest.len(), 1);
        let (intent, old_hash, new_hash, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        assert_eq!(intent.schema_version, 2);
        let _ = (old_hash, new_hash);
        // An unrelated dirty file must survive annotated recovery.
        write_file(vault, "unrelated.md", b"unrelated-dirty\n");
        run_git(vault, &["add", "unrelated.md"]);
        let intent = save_pending(&state, &config, &intent);
        simulate_post_move_crash(vault);

        let notice =
            recover(&state, &config, &intent, false).expect("rollback");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: rolled back an interrupted overwrite of paper")
        );
        assert_eq!(
            fs::read(vault.join(LIVE_PDF)).expect("live pdf"),
            OLD_PDF,
            "the original PDF object is restored"
        );
        assert_eq!(
            fs::read(vault.join(LIVE_NOTE)).expect("live note"),
            LIVE_BODY.as_bytes(),
            "the live note is restored"
        );
        assert_eq!(
            fs::read(vault.join(LIVE_ASSET_FILE)).expect("live asset"),
            OLD_IMG,
            "the live asset is restored"
        );
        assert!(
            !vault.join(ARCHIVE_PDF).exists(),
            "the proven rename reverses instead of copying"
        );
        assert!(
            !vault.join(ARCHIVE_NOTE).exists(),
            "the transaction-owned archive note is removed"
        );
        assert!(
            !vault.join(ARCHIVE_ASSETS).exists(),
            "the emptied archive assets directory is pruned"
        );
        assert!(
            !intent_file(&state, &config).exists(),
            "the intent is cleared after a proved rollback"
        );
        assert_eq!(
            fs::read(vault.join("unrelated.md")).expect("unrelated"),
            b"unrelated-dirty\n"
        );
    }

    #[test]
    fn crash_before_any_rename_restores_without_reversal() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        let intent = save_pending(&state, &config, &intent);

        let coordinator = FakeReverseCoordinator::grant();
        let notice = recover_with(&state, &config, &intent, &coordinator)
            .expect("rollback");
        assert!(notice
            .expect("notice")
            .contains("rolled back an interrupted overwrite"));
        assert_eq!(coordinator.calls(), 0, "no rename runs before any move");
        assert_eq!(fs::read(vault.join(LIVE_PDF)).expect("pdf"), OLD_PDF);
        assert!(!intent_file(&state, &config).exists());
    }

    #[test]
    fn crash_between_pdf_and_assets_moves_recovers_both() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let old_hash = sha256_hex(OLD_PDF);
        let new_hash = sha256_hex(NEW_PDF);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let moves = vec![
            PendingMove::file(LIVE_PDF, ARCHIVE_PDF, &old_hash)
                .with_replacement(&new_hash),
            PendingMove::dir(LIVE_ASSETS, ARCHIVE_ASSETS, manifest),
        ];
        let writes = vec![PendingWrite {
            path: ARCHIVE_NOTE.to_string(),
            sha256: sha256_hex(FROZEN_NOTE),
        }];
        let intent = annotated_intent(
            &base,
            &[LIVE_NOTE, LIVE_PDF, LIVE_ASSET_FILE],
            &[ARCHIVE_NOTE],
            moves,
            writes,
        );
        let intent = save_pending(&state, &config, &intent);
        // Only the PDF renamed before the crash; assets never moved.
        fs::create_dir_all(vault.join("ref_history/blogs"))
            .expect("archive dir");
        fs::rename(vault.join(LIVE_PDF), vault.join(ARCHIVE_PDF))
            .expect("move pdf");
        write_file(vault, LIVE_PDF, NEW_PDF);
        write_file(vault, ARCHIVE_NOTE, FROZEN_NOTE);
        write_file(vault, LIVE_NOTE, b"---\nid: paper\n---\nCRASHED\n");

        recover(&state, &config, &intent, false).expect("rollback");
        assert_eq!(fs::read(vault.join(LIVE_PDF)).expect("pdf"), OLD_PDF);
        assert_eq!(
            fs::read(vault.join(LIVE_ASSET_FILE)).expect("asset"),
            OLD_IMG
        );
        assert!(!vault.join(ARCHIVE_PDF).exists());
        assert!(!vault.join(ARCHIVE_ASSETS).exists());
        assert!(!intent_file(&state, &config).exists());
    }

    #[test]
    fn late_archive_save_fails_closed() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        let intent = save_pending(&state, &config, &intent);
        simulate_post_move_crash(vault);
        // A simulated late Highlights autosave lands in the archive copy.
        write_file(vault, ARCHIVE_PDF, b"late-autosave-bytes");

        let error =
            recover(&state, &config, &intent, false).expect_err("late save");
        assert_failed_closed(&error, &state_temp, &config);
        assert!(
            error
                .to_string()
                .contains("changed after the overwrite began"),
            "unexpected error: {error}"
        );
        assert_eq!(
            fs::read(vault.join(ARCHIVE_PDF)).expect("archive"),
            b"late-autosave-bytes",
            "the late save is never discarded"
        );
        assert_eq!(fs::read(vault.join(LIVE_PDF)).expect("live"), NEW_PDF);
    }

    #[test]
    fn unexpected_new_asset_fails_closed() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        let intent = save_pending(&state, &config, &intent);
        simulate_post_move_crash(vault);
        write_file(vault, &format!("{ARCHIVE_ASSETS}/evil.png"), b"evil");

        let error =
            recover(&state, &config, &intent, false).expect_err("new asset");
        assert_failed_closed(&error, &state_temp, &config);
        assert!(
            fs::read(vault.join(format!("{ARCHIVE_ASSETS}/evil.png")))
                .expect("evil")
                == b"evil",
            "the unexpected asset stays as recoverable evidence"
        );
        assert_eq!(
            fs::read(vault.join(ARCHIVE_PDF)).expect("archive"),
            OLD_PDF
        );
    }

    #[test]
    fn user_created_destination_fails_closed() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        let intent = save_pending(&state, &config, &intent);
        // The transaction never renamed, but a file appeared at the
        // archive destination.
        fs::create_dir_all(vault.join("ref_history/blogs"))
            .expect("archive dir");
        write_file(vault, ARCHIVE_PDF, b"user-file");

        let error =
            recover(&state, &config, &intent, false).expect_err("user dest");
        assert_failed_closed(&error, &state_temp, &config);
        assert_eq!(
            fs::read(vault.join(ARCHIVE_PDF)).expect("dest"),
            b"user-file",
            "a user-created destination is never deleted"
        );
        assert_eq!(fs::read(vault.join(LIVE_PDF)).expect("live"), OLD_PDF);
    }

    #[test]
    fn live_path_late_edit_fails_closed() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        let intent = save_pending(&state, &config, &intent);
        // An editor saved new annotations after the intent was written
        // but before any rename.
        write_file(vault, LIVE_PDF, b"late-annotations");

        let error =
            recover(&state, &config, &intent, false).expect_err("late edit");
        assert_failed_closed(&error, &state_temp, &config);
        assert_eq!(
            fs::read(vault.join(LIVE_PDF)).expect("live"),
            b"late-annotations",
            "the late write is preserved instead of rolled back"
        );
    }

    #[test]
    fn committed_annotated_intent_keeps_archive_and_later_saves() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        simulate_post_move_crash(vault);
        run_git(vault, &["add", "-A"]);
        run_git(vault, &["commit", "-q", "-m", PAPER_SUBJECT]);
        let committed = git_output(vault, &["rev-parse", "HEAD"]);
        let mut committed_intent = intent;
        committed_intent.expected_head = Some(committed);
        let committed_intent = save_pending(&state, &config, &committed_intent);
        // A later editor save lands in the archive copy after the commit.
        write_file(vault, ARCHIVE_PDF, b"post-commit-save");

        let notice =
            recover(&state, &config, &committed_intent, false).expect("retain");
        assert_eq!(
            notice.as_deref(),
            Some("recovered: kept the completed overwrite of paper")
        );
        assert_eq!(
            fs::read(vault.join(ARCHIVE_PDF)).expect("archive"),
            b"post-commit-save",
            "later saves survive a committed recovery"
        );
        assert_eq!(fs::read(vault.join(LIVE_PDF)).expect("live"), NEW_PDF);
        assert!(
            !intent_file(&state, &config).exists(),
            "a proved commit clears the leftover intent"
        );
    }

    #[test]
    fn reverse_coordination_hold_then_retry_recovers() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        let intent = save_pending(&state, &config, &intent);
        simulate_post_move_crash(vault);

        let hold = FakeReverseCoordinator::hold();
        let error =
            recover_with(&state, &config, &intent, &hold).expect_err("hold");
        assert_failed_closed(&error, &state_temp, &config);
        assert!(
            error
                .to_string()
                .contains("close it in Highlights and retry"),
            "unexpected error: {error}"
        );
        assert!(
            hold.calls() >= 1,
            "the reverse rename attempted coordination"
        );
        assert_eq!(
            fs::read(vault.join(ARCHIVE_PDF)).expect("archive"),
            OLD_PDF,
            "the only annotated copy survives the hold"
        );

        // The retry after the hold completes the same recovery.
        recover_with(
            &state,
            &config,
            &intent,
            &FakeReverseCoordinator::grant(),
        )
        .expect("retry");
        assert_eq!(fs::read(vault.join(LIVE_PDF)).expect("live"), OLD_PDF);
        assert!(!vault.join(ARCHIVE_PDF).exists());
        assert!(!intent_file(&state, &config).exists());
    }

    #[test]
    fn reverse_coordination_failure_fails_closed() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        let intent = save_pending(&state, &config, &intent);
        simulate_post_move_crash(vault);

        let error = recover_with(
            &state,
            &config,
            &intent,
            &FakeReverseCoordinator::fail("presenter denied the rename"),
        )
        .expect_err("coordination failure");
        assert_failed_closed(&error, &state_temp, &config);
        assert!(
            error.to_string().contains("presenter denied the rename"),
            "unexpected error: {error}"
        );
        assert_eq!(
            fs::read(vault.join(ARCHIVE_PDF)).expect("archive"),
            OLD_PDF,
            "no uncoordinated fallback renames underneath the editor"
        );
    }

    #[test]
    fn duplicated_original_on_both_endpoints_removes_the_copy() {
        let (vault_temp, state_temp, config, base) = annotated_fixture();
        let vault = vault_temp.path();
        let state = state_dir_of(&state_temp);
        let manifest = asset_manifest(vault, LIVE_ASSETS).expect("manifest");
        let (intent, _, _, _) =
            annotated_intent_via_builder(&config, &base, manifest);
        let intent = save_pending(&state, &config, &intent);
        // A copied (not renamed) archive leaves the original on both sides.
        fs::create_dir_all(vault.join("ref_history/blogs"))
            .expect("archive dir");
        fs::copy(vault.join(LIVE_PDF), vault.join(ARCHIVE_PDF))
            .expect("copy pdf");
        write_file(vault, ARCHIVE_NOTE, FROZEN_NOTE);
        write_file(vault, LIVE_NOTE, b"---\nid: paper\n---\nCRASHED\n");

        recover(&state, &config, &intent, false).expect("rollback");
        assert_eq!(fs::read(vault.join(LIVE_PDF)).expect("live"), OLD_PDF);
        assert!(
            !vault.join(ARCHIVE_PDF).exists(),
            "the redundant archive copy is removed"
        );
        assert!(!intent_file(&state, &config).exists());
    }

    #[test]
    fn version_one_intent_without_moves_recovers_as_before() {
        let vault_temp = fixture_vault();
        let vault = vault_temp.path();
        let config = test_config(vault);
        let state_temp = tempfile::tempdir().expect("state root");
        let state = state_temp.path();
        let base = git_output(vault, &["rev-parse", "HEAD"]);

        fs::write(
            vault.join("ref/blogs/half-written.md"),
            "---\nid: half-written\n---\nCRASHED\n",
        )
        .expect("dirty note");
        let canonical = fs::canonicalize(vault)
            .expect("canonical vault")
            .to_string_lossy()
            .into_owned();
        let path = intent_path_for(state, &canonical);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        // A version-1 marker has no `moves` or `writes` keys at all.
        let document = serde_json::json!({
            "schema_version": 1,
            "vault": canonical,
            "stem": STEM,
            "ref_type": REF_TYPE,
            "base_head": base,
            "expected_subject": SUBJECT,
            "expected_head": serde_json::Value::Null,
            "touched": ["ref/blogs/half-written.md"],
            "created": [],
        });
        fs::write(&path, serde_json::to_string(&document).expect("json"))
            .expect("raw v1 intent");

        let loaded = load_intent_from(state, &config)
            .expect("load v1")
            .expect("pending");
        assert!(loaded.moves.is_empty());
        assert!(loaded.writes.is_empty());
        let notice = recover(state, &config, &loaded, false).expect("rollback");
        assert!(notice
            .expect("notice")
            .contains("rolled back an interrupted overwrite"));
        assert_eq!(
            fs::read(vault.join("ref/blogs/half-written.md")).expect("note"),
            b"---\nid: half-written\n---\nbody\n"
        );
        assert!(!intent_file(state, &config).exists());
    }

    #[test]
    fn malformed_move_evidence_fails_closed() {
        let good_hash = sha256_hex(OLD_PDF);
        for build in [
            PendingMove::file(LIVE_PDF, LIVE_PDF, &good_hash),
            PendingMove::file(LIVE_PDF, ARCHIVE_PDF, "not-a-hash"),
            PendingMove {
                source: LIVE_PDF.to_string(),
                dest: ARCHIVE_PDF.to_string(),
                source_sha256: None,
                replacement_sha256: None,
                files: Vec::new(),
            },
            PendingMove::dir(
                LIVE_ASSETS,
                ARCHIVE_ASSETS,
                vec![PendingMoveFile {
                    rel: "../escape.png".to_string(),
                    sha256: good_hash.clone(),
                }],
            ),
            PendingMove::dir(
                LIVE_ASSETS,
                ARCHIVE_ASSETS,
                vec![
                    PendingMoveFile {
                        rel: "shot.png".to_string(),
                        sha256: good_hash.clone(),
                    },
                    PendingMoveFile {
                        rel: "shot.png".to_string(),
                        sha256: good_hash.clone(),
                    },
                ],
            ),
        ] {
            let (vault_temp, state_temp, config, base) = annotated_fixture();
            let vault = vault_temp.path();
            let state = state_dir_of(&state_temp);
            let intent = save_pending(
                &state,
                &config,
                &annotated_intent(&base, &[], &[], vec![build], Vec::new()),
            );
            let error = recover(&state, &config, &intent, false)
                .expect_err("malformed");
            assert!(
                error.to_string().contains("malformed")
                    || error.to_string().contains("invalid"),
                "unexpected error: {error}"
            );
            assert!(
                intent_file(&state, &config).exists(),
                "malformed evidence preserves the marker"
            );
            assert_eq!(
                fs::read(vault.join(LIVE_PDF)).expect("live"),
                OLD_PDF,
                "malformed evidence touches nothing"
            );
        }
    }

    #[test]
    fn unsupported_schema_version_fails_closed() {
        let (_vault_temp, state_temp, config, base) = annotated_fixture();
        let state = state_dir_of(&state_temp);
        let mut intent = pending_intent(&base, None, &[], &[]);
        intent.schema_version = INTENT_SCHEMA_VERSION + 1;
        let intent = save_pending(&state, &config, &intent);
        let error =
            recover(&state, &config, &intent, false).expect_err("version");
        assert!(
            error.to_string().contains("unsupported schema version"),
            "unexpected error: {error}"
        );
        assert!(intent_file(&state, &config).exists());
    }

    #[test]
    fn rename_no_clobber_refuses_an_existing_destination() {
        let temp = tempfile::tempdir().expect("scratch");
        let from = temp.path().join("from.txt");
        let to = temp.path().join("to.txt");
        fs::write(&from, b"from").expect("from");
        fs::write(&to, b"to").expect("to");
        let error = rename_no_clobber(&from, &to).expect_err("clobber");
        assert!(
            error.contains("refusing to overwrite"),
            "unexpected error: {error}"
        );
        assert_eq!(fs::read(&from).expect("from"), b"from");
        assert_eq!(fs::read(&to).expect("to"), b"to");

        let lonely = temp.path().join("lonely.txt");
        fs::write(&lonely, b"lonely").expect("lonely");
        let target = temp.path().join("target.txt");
        rename_no_clobber(&lonely, &target).expect("rename");
        assert!(!lonely.exists());
        assert_eq!(fs::read(&target).expect("target"), b"lonely");
    }
}
