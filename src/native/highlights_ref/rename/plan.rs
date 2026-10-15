//! Read-only plan construction for `bob ref rename`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde::Serialize;

use super::{super::*, links};
use crate::native::{
    ob,
    ref_library::{
        build_index,
        output::ErrorCandidate,
        output::Format,
        show::{load_show_row, resolve_show, ShowFailure, ShowRow},
        LibraryConfig, RefRow,
    },
};

const RENAME_COMMAND: &str = "ref rename";
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "m4a", "ogg", "opus"];
const LEGACY_REF_DIRS: &[&str] = &["ai", "zorg"];
const MAINTENANCE_LOCK_BUDGET: Duration = Duration::from_secs(60);
const SCAN_LOCK_BUDGET: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
struct RenameFailure {
    code: &'static str,
    message: String,
    hint: Option<String>,
}

impl RenameFailure {
    fn new(
        code: &'static str,
        message: impl Into<String>,
        hint: Option<impl Into<String>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MoveKind {
    Note,
    Pdf,
    Sidecar,
    Audio,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct MoveInfo {
    pub(super) kind: MoveKind,
    pub(super) from: String,
    pub(super) to: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RefLocation {
    pub(super) path: String,
    pub(super) pdf: String,
    pub(super) stem: String,
}

#[derive(Debug, Clone, Serialize)]
struct AmbiguousLinkInfo {
    file: String,
    line: usize,
    target: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct LinksInfo {
    pub(super) rewritten: usize,
    pub(super) files: Vec<String>,
    skipped_ambiguous: Vec<AmbiguousLinkInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct KeptTask {
    pub(super) path: String,
    pub(super) block_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct KeptInfo {
    pub(super) marker_id: Option<String>,
    pub(super) task: Option<KeptTask>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SyncInfo {
    pub(super) pre: &'static str,
    pub(super) post: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct RenameEnvelope {
    pub(super) ok: bool,
    pub(super) schema_version: u32,
    pub(super) command: &'static str,
    pub(super) generated_at: String,
    pub(super) mode: &'static str,
    pub(super) changed: bool,
    pub(super) outcome: &'static str,
    pub(super) from: RefLocation,
    pub(super) to: RefLocation,
    pub(super) moves: Vec<MoveInfo>,
    pub(super) links: LinksInfo,
    pub(super) frontmatter: Vec<String>,
    pub(super) kept: KeptInfo,
    #[serde(rename = "ref")]
    pub(super) reference: ShowRow,
    pub(super) commit: Option<serde_json::Value>,
    pub(super) sync: SyncInfo,
    pub(super) warnings: Vec<String>,
}

struct PreparedPlan {
    report: RenameEnvelope,
    link_rewrites: Vec<links::FileRewrite>,
    own_note_rewrite: Option<(PathBuf, String)>,
    touched_paths: BTreeSet<PathBuf>,
}

struct Bundle {
    old_stem: String,
    new_stem: String,
    note_from: PathBuf,
    note_to: PathBuf,
    pdf_from: PathBuf,
    pdf_to: PathBuf,
    moves: Vec<MoveInfo>,
    link_map: BTreeMap<PathBuf, PathBuf>,
    frontmatter_map: Vec<(String, String)>,
    audio_paths: Vec<PathBuf>,
}

pub(crate) fn run(matches: &clap::ArgMatches) -> i32 {
    let format = Format::from_name(
        matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human"),
    );
    let dry_run = matches.get_flag("dry-run");
    let offline = matches.get_flag("offline");
    let config = Config::from_matches(matches);
    let query = matches
        .get_one::<String>("ref")
        .expect("required REF is enforced by clap");
    let requested_name = matches
        .get_one::<String>("name")
        .expect("required NAME is enforced by clap");

    if dry_run {
        return match prepare(&config, query, requested_name, true, offline) {
            Ok(prepared) => print_success(format, &prepared.report),
            Err(failure) => print_failure(format, failure),
        };
    }

    run_live(&config, query, requested_name, offline, format)
}

fn run_live(
    config: &Config,
    query: &str,
    requested_name: &str,
    offline: bool,
    format: Format,
) -> i32 {
    let _maintenance_lock = {
        let json_mode = format == Format::Json;
        let on_first_wait = move || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                eprintln!(
                    "  {}",
                    crate::native::style::Styler::detect()
                        .dim("waiting for another vault maintenance run…")
                );
            }
        };
        match ob::acquire_lock_waiting(MAINTENANCE_LOCK_BUDGET, on_first_wait) {
            Ok(lock) => lock,
            Err(error) => {
                return print_failure(
                    format,
                    RenameFailure::new(
                        "lock_busy",
                        format!("could not acquire the vault maintenance lock: {error}"),
                        Some("wait for the other maintenance run to finish, then retry"),
                    ),
                );
            }
        }
    };
    let child_env = ob::child_env();

    match ob::detect_git_worktree(&config.bob_dir, &child_env) {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            return print_failure(
                format,
                RenameFailure::new(
                    "not_a_worktree",
                    format!(
                        "reference renames require a Git worktree: {}",
                        config.bob_dir.display()
                    ),
                    Some("initialize or open the Bob vault as a Git worktree"),
                ),
            );
        }
    }

    let _scan_lock = match super::super::acquire_scan_writer_lock_with_timeout(
        SCAN_LOCK_BUDGET,
    ) {
        Ok(lock) => lock,
        Err(error) => {
            return print_failure(
                format,
                RenameFailure::new(
                    "scan_busy",
                    format!("could not acquire the reference scan lock: {error}"),
                    Some("wait for the running reference scan to finish, then retry"),
                ),
            );
        }
    };

    if !offline {
        let cycle =
            crate::native::vault_sync::run_cycle_with_existing_lock_report(
                &child_env, true,
            );
        if !cycle.ok || !cycle.conflicts.is_empty() {
            let detail = if cycle.conflicts.is_empty() {
                cycle.error.unwrap_or_else(|| {
                    "vault sync reported failure".to_string()
                })
            } else {
                format!("vault sync conflict in {}", cycle.conflicts.join(", "))
            };
            return print_failure(
                format,
                RenameFailure::new(
                    "sync_failed",
                    format!("pre-sync failed: {detail}"),
                    Some("re-run with --offline to commit locally without syncing"),
                ),
            );
        }
    }

    let mut prepared = match prepare(config, query, requested_name, false, true)
    {
        Ok(prepared) => prepared,
        Err(failure) if failure.code == "unknown_reference" => {
            match already_renamed_report(
                config,
                query,
                requested_name,
                offline,
                &child_env,
            ) {
                Ok(Some(report)) => return print_success(format, &report),
                Ok(None) => return print_failure(format, failure),
                Err(already_failure) => {
                    return print_failure(format, already_failure);
                }
            }
        }
        Err(failure) => return print_failure(format, failure),
    };

    if !prepared.report.changed {
        match already_renamed_report(
            config,
            query,
            requested_name,
            offline,
            &child_env,
        ) {
            Ok(Some(report)) => return print_success(format, &report),
            Ok(None) => {}
            Err(failure) => return print_failure(format, failure),
        }
    }

    prepared.report.sync.pre = if offline { "skipped" } else { "ok" };
    if !prepared.report.changed {
        return print_success(format, &prepared.report);
    }

    let old_head = git_head(&config.bob_dir, &child_env).ok();
    let prior_row = prepared.report.reference.row.clone();
    let applied = match apply_and_verify(&config.bob_dir, &prepared, || {
        verify_renamed(config, &prepared, &prior_row)
    }) {
        Ok(applied) => applied,
        Err(message) => {
            return print_failure(
                format,
                RenameFailure::new(
                    "verify_failed",
                    format!("could not apply and verify the rename: {message}"),
                    Some("the rename was rolled back; inspect the listed paths and retry"),
                ),
            );
        }
    };

    let subject = format!(
        "bob ref rename: {} → {}",
        prepared.report.from.stem, prepared.report.to.stem
    );
    let commit_paths =
        prepared.touched_paths.iter().cloned().collect::<Vec<_>>();
    let sha = match ob::commit_paths(
        &config.bob_dir,
        &child_env,
        &subject,
        &commit_paths,
    ) {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            let unstage =
                unstage_paths(&config.bob_dir, &child_env, &commit_paths);
            let rollback = rollback_applied(&applied);
            return print_failure(
                format,
                RenameFailure::new(
                    "verify_failed",
                    format!("rename verified but produced no scoped Git commit{unstage}{rollback}"),
                    Some("check the vault Git state and retry"),
                ),
            );
        }
        Err(error) => {
            if let Some((sha, committed_subject)) = committed_despite_error(
                &config.bob_dir,
                &child_env,
                old_head.as_deref(),
                &subject,
            ) {
                if committed_subject == subject {
                    let _ = error;
                    sha
                } else {
                    let unstage = unstage_paths(
                        &config.bob_dir,
                        &child_env,
                        &commit_paths,
                    );
                    let rollback = rollback_applied(&applied);
                    return print_failure(
                        format,
                        RenameFailure::new(
                            "verify_failed",
                            format!(
                                "Git commit failed: {error}{unstage}{rollback}"
                            ),
                            Some("check the vault Git state and retry"),
                        ),
                    );
                }
            } else {
                let unstage =
                    unstage_paths(&config.bob_dir, &child_env, &commit_paths);
                let rollback = rollback_applied(&applied);
                return print_failure(
                    format,
                    RenameFailure::new(
                        "verify_failed",
                        format!(
                            "Git commit failed: {error}{unstage}{rollback}"
                        ),
                        Some("check the vault Git state and retry"),
                    ),
                );
            }
        }
    };

    prepared.report.commit = Some(serde_json::json!({
        "sha": sha,
        "subject": subject,
    }));
    if offline {
        prepared.report.sync.post = "skipped";
    } else {
        let cycle =
            crate::native::vault_sync::run_cycle_with_existing_lock_report(
                &child_env, true,
            );
        if !cycle.ok || !cycle.conflicts.is_empty() {
            let detail = if cycle.conflicts.is_empty() {
                cycle.error.unwrap_or_else(|| {
                    "vault sync reported failure".to_string()
                })
            } else {
                format!("vault sync conflict in {}", cycle.conflicts.join(", "))
            };
            prepared.report.sync.post = "failed";
            prepared.report.warnings.push(format!(
                "the rename is committed locally; post-sync failed ({detail}); run bob vault-sync"
            ));
        } else {
            prepared.report.sync.post = "ok";
        }
    }
    print_success(format, &prepared.report)
}

fn print_success(format: Format, report: &RenameEnvelope) -> i32 {
    if format == Format::Json {
        println!(
            "{}",
            serde_json::to_string(report).expect("rename report serializes")
        );
    } else {
        print!("{}", super::report::render_human(report));
    }
    0
}

fn already_renamed_report(
    config: &Config,
    query: &str,
    requested_name: &str,
    offline: bool,
    child_env: &ob::ChildEnv,
) -> std::result::Result<Option<RenameEnvelope>, RenameFailure> {
    let Some(old_stem) = old_stem_from_query(query) else {
        return Ok(None);
    };
    let new_stem = super::super::clip_url::validate_name(requested_name)
        .map_err(|_| invalid_name_failure(requested_name))?;
    let library_config = LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    };
    let index = build_index(&library_config).map_err(|error| {
        RenameFailure::new(
            "unknown_reference",
            format!("could not inspect the renamed reference: {error}"),
            Some(
                "pass the exact reference note path or check the ref directory",
            ),
        )
    })?;
    let expected_subject = format!("bob ref rename: {old_stem} → {new_stem}");
    let mut matches = Vec::new();
    for (row_index, row) in index.rows.iter().enumerate() {
        if Path::new(&row.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            != Some(new_stem.as_str())
            || row.source_pdf.is_none()
        {
            continue;
        }
        let Some((sha, subject)) = latest_commit_for_path(
            &config.bob_dir,
            child_env,
            Path::new(&row.path),
        )
        .map_err(|error| {
            RenameFailure::new(
                "not_in_sync",
                format!("could not inspect rename history: {error}"),
                Some("check Git availability and retry"),
            )
        })?
        else {
            continue;
        };
        if subject == expected_subject {
            matches.push((row_index, sha));
        }
    }
    if matches.len() != 1 {
        return Ok(None);
    }
    let (row_index, sha) = matches.pop().expect("one matched row");
    let resolved = crate::native::ref_library::show::ResolvedShow {
        row: row_index,
        also: Vec::new(),
    };
    let shown = crate::native::ref_library::show::load_show_row(
        &index, &resolved, false, false,
    )
    .map_err(|message| {
        RenameFailure::new(
            "not_in_sync",
            format!("could not read renamed reference: {message}"),
            Some("check the reference note and retry"),
        )
    })?;
    let row = &index.rows[row_index];
    let to_path = PathBuf::from(&row.path);
    let from_path = to_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{old_stem}.md"));
    let to_pdf = PathBuf::from(row.source_pdf.as_deref().unwrap_or_default());
    let from_pdf = to_pdf
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{old_stem}.pdf"));
    Ok(Some(RenameEnvelope {
        ok: true,
        schema_version: crate::native::ref_library::output::REF_SCHEMA_VERSION,
        command: RENAME_COMMAND,
        generated_at: crate::native::ref_library::output::generated_at(),
        mode: "write",
        changed: false,
        outcome: "already_renamed",
        from: RefLocation {
            path: vault_path(&from_path),
            pdf: vault_path(&from_pdf),
            stem: old_stem,
        },
        to: RefLocation {
            path: vault_path(&to_path),
            pdf: vault_path(&to_pdf),
            stem: new_stem,
        },
        moves: Vec::new(),
        links: LinksInfo {
            rewritten: 0,
            files: Vec::new(),
            skipped_ambiguous: Vec::new(),
        },
        frontmatter: Vec::new(),
        kept: KeptInfo {
            marker_id: row.marker_id.clone(),
            task: row.task.as_ref().map(|task| KeptTask {
                path: task.path.clone(),
                block_id: task.block_id.clone(),
            }),
        },
        reference: shown,
        commit: Some(serde_json::json!({
            "sha": sha,
            "subject": expected_subject,
        })),
        sync: SyncInfo {
            pre: if offline { "skipped" } else { "ok" },
            post: "skipped",
        },
        warnings: Vec::new(),
    }))
}

fn old_stem_from_query(query: &str) -> Option<String> {
    let value = query.trim();
    if value.is_empty() {
        return None;
    }
    let looks_like_note_path = value.starts_with("ref/")
        || Path::new(value).is_absolute()
        || value.ends_with(".md");
    let stem = if looks_like_note_path {
        Path::new(value).file_stem()?.to_str()?
    } else if value.contains('/') || value.contains('\\') {
        return None;
    } else {
        value.strip_suffix(".md").unwrap_or(value)
    };
    (!stem.is_empty() && !stem.contains('/') && !stem.contains('\\'))
        .then(|| stem.to_string())
}

fn latest_commit_for_path(
    vault: &Path,
    child_env: &ob::ChildEnv,
    path: &Path,
) -> std::result::Result<Option<(String, String)>, String> {
    let output = ob::git_command(vault, child_env)
        .args(["log", "-1", "--format=%H%x00%s", "--"])
        .arg(path)
        .output()
        .map_err(|error| format!("git log for {}: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git log for {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout);
    let Some((sha, subject)) = value.trim_end().split_once('\0') else {
        return Ok(None);
    };
    Ok(Some((sha.to_string(), subject.to_string())))
}

#[derive(Debug, Default)]
struct AppliedRename {
    moves: Vec<(PathBuf, PathBuf)>,
    before: crate::native::ref_library::migrate_tasks::BeforeImages,
    after: BTreeMap<PathBuf, Vec<u8>>,
}

fn apply_and_verify(
    bob_dir: &Path,
    prepared: &PreparedPlan,
    verify: impl FnOnce() -> std::result::Result<(), String>,
) -> std::result::Result<AppliedRename, String> {
    let mut rewrites = BTreeMap::<PathBuf, String>::new();
    for rewrite in &prepared.link_rewrites {
        rewrites.insert(rewrite.path.clone(), rewrite.contents.clone());
    }
    if let Some((path, contents)) = &prepared.own_note_rewrite {
        rewrites.insert(path.clone(), contents.clone());
    }
    apply_transaction(bob_dir, &prepared.report.moves, &rewrites, verify)
}

fn apply_transaction(
    bob_dir: &Path,
    moves: &[MoveInfo],
    rewrites: &BTreeMap<PathBuf, String>,
    verify: impl FnOnce() -> std::result::Result<(), String>,
) -> std::result::Result<AppliedRename, String> {
    let mut transaction = AppliedRename::default();
    let relative_paths = rewrites
        .keys()
        .map(|path| vault_path(path))
        .collect::<Vec<_>>();
    transaction.before =
        crate::native::ref_library::migrate_tasks::snapshot_before_images(
            bob_dir,
            &relative_paths,
        );
    if let Some((path, _)) = transaction
        .before
        .iter()
        .find(|(_, before)| before.is_none())
    {
        return Err(format!(
            "could not snapshot rewrite source {}",
            path.display()
        ));
    }

    let result = (|| {
        for movement in moves {
            let from = PathBuf::from(&movement.from);
            let to = PathBuf::from(&movement.to);
            let from_absolute = bob_dir.join(&from);
            let to_absolute = bob_dir.join(&to);
            if let Some(parent) = to_absolute.parent()
                && !parent.is_dir()
            {
                return Err(format!(
                    "destination parent does not exist: {}",
                    parent.display()
                ));
            }
            rename_no_replace(&from_absolute, &to_absolute).map_err(
                |error| {
                    format!(
                        "move {} to {}: {error}",
                        from.display(),
                        to.display()
                    )
                },
            )?;
            transaction.moves.push((from_absolute, to_absolute));
        }

        let moved = transaction
            .moves
            .iter()
            .map(|(from, to)| {
                (
                    from.strip_prefix(bob_dir).unwrap_or(from).to_path_buf(),
                    to.strip_prefix(bob_dir).unwrap_or(to).to_path_buf(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (relative, contents) in rewrites {
            let write_relative = moved.get(relative).unwrap_or(relative);
            let absolute = bob_dir.join(write_relative);
            super::super::atomic_write(&absolute, contents).map_err(
                |error| {
                    format!("rewrite {}: {error}", write_relative.display())
                },
            )?;
            transaction
                .after
                .insert(bob_dir.join(relative), contents.as_bytes().to_vec());
        }
        verify()
    })();

    if let Err(message) = result {
        let rollback = rollback_applied(&transaction);
        return Err(format!("{message}{rollback}"));
    }
    Ok(transaction)
}

fn rollback_applied(transaction: &AppliedRename) -> String {
    let mut failures = Vec::new();
    for (from, to) in transaction.moves.iter().rev() {
        if !to.exists() {
            if from.exists() {
                continue;
            }
            failures.push(format!(
                "{} and {} are both missing",
                from.display(),
                to.display()
            ));
            continue;
        }
        if from.exists() {
            failures.push(format!(
                "{} exists; left {} in place",
                from.display(),
                to.display()
            ));
            continue;
        }
        if let Err(error) = rename_no_replace(to, from) {
            failures.push(format!(
                "could not move {} back to {}: {error}",
                to.display(),
                from.display()
            ));
        }
    }
    let diverged =
        crate::native::ref_library::migrate_tasks::restore_before_images(
            &transaction.before,
            &transaction.after,
        );
    failures.extend(diverged.into_iter().map(|path| {
        format!("left concurrently changed path {} in place", path.display())
    }));
    if failures.is_empty() {
        String::new()
    } else {
        format!("; rollback issues: {}", failures.join("; "))
    }
}

#[cfg(target_os = "linux")]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(from.as_os_str().as_bytes()).map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
    })?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
    })?;
    let status = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(from.as_os_str().as_bytes()).map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
    })?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
    })?;
    let status = unsafe {
        libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL)
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", to.display()),
        ));
    }
    fs::rename(from, to)
}

fn verify_renamed(
    config: &Config,
    prepared: &PreparedPlan,
    prior_row: &RefRow,
) -> std::result::Result<(), String> {
    if !is_in_sync(config, Path::new(&prepared.report.to.pdf)) {
        return Err(format!(
            "dry-run ref sync still plans work for {}",
            prepared.report.to.pdf
        ));
    }
    let library_config = LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    };
    let index = build_index(&library_config)
        .map_err(|error| format!("rebuild the reference index: {error}"))?;
    let row = index
        .rows
        .iter()
        .find(|row| row.path == prepared.report.to.path)
        .ok_or_else(|| {
            format!("new index row is missing: {}", prepared.report.to.path)
        })?;
    if index
        .rows
        .iter()
        .any(|row| row.path == prepared.report.from.path)
    {
        return Err(format!(
            "old index row remains at {}",
            prepared.report.from.path
        ));
    }
    if row.marker_id != prepared.report.kept.marker_id {
        return Err("reference id changed during rename".to_string());
    }
    let expected_task = prepared
        .report
        .kept
        .task
        .as_ref()
        .map(|task| (&task.path, &task.block_id));
    let actual_task =
        row.task.as_ref().map(|task| (&task.path, &task.block_id));
    if expected_task != actual_task {
        return Err(
            "reading task path or block ID changed during rename".to_string()
        );
    }
    for code in ["open_ref_without_task", "duplicate_identity"] {
        if row
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code)
        {
            return Err(format!("new index row has {code} diagnostic"));
        }
    }
    let had_orphan = prior_row
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "orphan_ref_task");
    if !had_orphan
        && row
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "orphan_ref_task")
    {
        return Err(
            "rename introduced an orphan_ref_task diagnostic".to_string()
        );
    }
    Ok(())
}

fn git_head(
    vault: &Path,
    child_env: &ob::ChildEnv,
) -> std::result::Result<String, String> {
    let output = ob::git_command(vault, child_env)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|error| format!("git rev-parse HEAD: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn unstage_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    paths: &[PathBuf],
) -> String {
    let output = ob::git_command(vault, child_env)
        .arg("reset")
        .arg("--quiet")
        .arg("HEAD")
        .arg("--")
        .args(paths)
        .output();
    match output {
        Ok(output) if output.status.success() => String::new(),
        Ok(output) => format!(
            "; could not unstage rename paths: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => format!("; could not unstage rename paths: {error}"),
    }
}

fn committed_despite_error(
    vault: &Path,
    child_env: &ob::ChildEnv,
    old_head: Option<&str>,
    expected_subject: &str,
) -> Option<(String, String)> {
    let (sha, subject) =
        latest_commit_for_path(vault, child_env, Path::new(".")).ok()??;
    (old_head != Some(sha.as_str()) && subject == expected_subject)
        .then_some((sha, subject))
}

fn prepare(
    config: &Config,
    query: &str,
    requested_name: &str,
    dry_run: bool,
    offline: bool,
) -> std::result::Result<PreparedPlan, RenameFailure> {
    if intake_query(config, query) {
        return Err(RenameFailure::new(
            "intake_reference",
            format!("cannot rename an intake reference: {query}"),
            Some("rename the intake PDF before running bob ref scan"),
        ));
    }

    let library_config = LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    };
    let index = build_index(&library_config).map_err(|error| {
        RenameFailure::new(
            "unknown_reference",
            format!("could not resolve reference: {error}"),
            Some(
                "pass the exact reference note path or check the ref directory",
            ),
        )
    })?;
    let resolved = resolve_show(&index, query).map_err(|failure| {
        let (code, message, candidates) = match failure {
            ShowFailure::Ambiguous { query, candidates } => (
                "ambiguous_reference",
                format!("ambiguous reference: {query}"),
                candidates,
            ),
            ShowFailure::NotFound { query, candidates } => (
                "unknown_reference",
                format!("no reference note matches {query}"),
                candidates,
            ),
        };
        let hint = candidate_hint(&candidates);
        RenameFailure::new(code, message, hint)
    })?;
    let row = &index.rows[resolved.row];
    let old_stem = Path::new(&row.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            RenameFailure::new(
                "legacy_reference",
                format!("reference path has no usable file stem: {}", row.path),
                Some("v1 renames modern PDF-backed notes under ref/<type>/"),
            )
        })?;

    if is_legacy(row, &config.ref_dir) {
        return Err(RenameFailure::new(
            "legacy_reference",
            format!("cannot rename a legacy reference: {}", row.path),
            Some("legacy notes have no PDF pairing, so Obsidian's rename is safe for them"),
        ));
    }

    let name = super::super::clip_url::validate_name(requested_name)
        .map_err(|_| invalid_name_failure(requested_name))?;
    if name.eq_ignore_ascii_case(old_stem) && name != old_stem {
        return Err(RenameFailure::new(
            "invalid_name",
            format!("cannot rename {old_stem:?} using a case-only change"),
            Some("APFS treats names that differ only by ASCII case as the same file"),
        ));
    }

    let show_row =
        load_show_row(&index, &resolved, false, false).map_err(|message| {
            RenameFailure::new(
                "not_in_sync",
                format!("could not read reference note: {message}"),
                Some("run bob ref scan and retry"),
            )
        })?;

    if name == old_stem {
        let bundle = no_op_bundle(row, old_stem, &name)?;
        return Ok(unchanged_plan(row, show_row, bundle, dry_run));
    }

    let bundle = build_bundle(config, row, old_stem, &name)?;
    if destinations_taken(config, &bundle)? {
        return Err(RenameFailure::new(
            "name_taken",
            format!("the name {name:?} is already used in this reference directory or vault"),
            Some("choose a different file stem; names are never auto-suffixed"),
        ));
    }

    let note_absolute = config.bob_dir.join(&bundle.note_from);
    let note_contents =
        fs::read_to_string(&note_absolute).map_err(|error| {
            RenameFailure::new(
                "not_in_sync",
                format!(
                    "could not read {}: {error}",
                    bundle.note_from.display()
                ),
                Some("run bob ref scan and retry"),
            )
        })?;
    let own_highlights = has_managed_highlights(&note_contents);
    let assets_path = note_absolute
        .parent()
        .unwrap_or(&config.ref_dir)
        .join(format!("{old_stem}.assets"));
    let mut link_plan =
        links::plan_vault_links(&config.bob_dir, &bundle.link_map).map_err(
            |error| {
                RenameFailure::new(
                    "not_in_sync",
                    format!("could not inspect inbound links: {error}"),
                    Some("check vault Markdown readability and retry"),
                )
            },
        )?;
    let has_links_with_highlights = !link_plan.highlight_fragments.is_empty()
        || !link_plan.highlight_follow_ups.is_empty();
    if own_highlights || assets_path.exists() || has_links_with_highlights {
        let mut details = Vec::new();
        if own_highlights {
            details.push(
                "the reference note contains live or removed highlights"
                    .to_string(),
            );
        }
        if assets_path.exists() {
            details.push(format!("{} exists", assets_path.display()));
        }
        if has_links_with_highlights {
            details.push(
                "an inbound link targets a highlight or follow-up".to_string(),
            );
        }
        return Err(RenameFailure::new(
            "has_highlights",
            format!("cannot rename an annotated reference: {}", details.join("; ")),
            Some("v1 renames only references with no highlights, highlight assets, or highlight follow-ups"),
        ));
    }

    if pdf_has_listen_card_link(
        &config.bob_dir.join(&bundle.pdf_from),
        &bundle.audio_paths,
    ) {
        return Err(RenameFailure::new(
            "listen_card_link",
            format!("the PDF links to a companion audio file: {}", bundle.pdf_from.display()),
            Some("remove the listen-card link before renaming; the PDF itself is never rewritten"),
        ));
    }

    if !is_in_sync(config, &bundle.pdf_from) {
        return Err(RenameFailure::new(
            "not_in_sync",
            format!(
                "the reference note and PDF are not in sync: {}",
                bundle.note_from.display()
            ),
            Some("run bob ref scan, then retry the rename"),
        ));
    }

    let linked_note = link_plan
        .files
        .iter()
        .position(|rewrite| rewrite.path == bundle.note_from);
    // The Markdown link pass can rewrite wikilink-valued frontmatter (for
    // example `audio`) before the dedicated frontmatter pass sees it. Keep
    // the report complete by tracking the managed fields changed from the
    // original note as well as fields rewritten below.
    let planned_frontmatter =
        rewrite_own_frontmatter(&note_contents, &bundle.frontmatter_map)
            .map(|(_, fields)| fields)
            .unwrap_or_default();
    let note_input = linked_note
        .map(|index| link_plan.files[index].contents.as_str())
        .unwrap_or(&note_contents);
    let (own_note_rewrite, mut frontmatter) =
        match rewrite_own_frontmatter(note_input, &bundle.frontmatter_map) {
            Some((contents, fields)) => {
                if let Some(index) = linked_note {
                    link_plan.files[index].contents = contents;
                    (None, fields)
                } else {
                    (Some((bundle.note_from.clone(), contents)), fields)
                }
            }
            None => (None, Vec::new()),
        };
    frontmatter.extend(planned_frontmatter);
    frontmatter.sort();
    frontmatter.dedup();

    let mut touched_paths = BTreeSet::new();
    for move_info in &bundle.moves {
        touched_paths.insert(PathBuf::from(&move_info.from));
        touched_paths.insert(PathBuf::from(&move_info.to));
    }
    for rewrite in &link_plan.files {
        touched_paths.insert(rewrite.path.clone());
    }
    if let Some((path, _)) = &own_note_rewrite {
        touched_paths.insert(path.clone());
    }
    let dirty_paths = dirty_touched_paths(&config.bob_dir, &touched_paths)?;
    if offline && !dirty_paths.is_empty() {
        return Err(RenameFailure::new(
            "dirty_paths",
            format!("rename paths are dirty: {}", display_paths(&dirty_paths)),
            Some("commit or restore those paths before an offline rename"),
        ));
    }
    let warnings = if dirty_paths.is_empty() {
        Vec::new()
    } else {
        vec![format!(
            "dirty paths will be included by the pre-sync: {}",
            display_paths(&dirty_paths)
        )]
    };

    let from = RefLocation {
        path: vault_path(&bundle.note_from),
        pdf: vault_path(&bundle.pdf_from),
        stem: bundle.old_stem.clone(),
    };
    let to = RefLocation {
        path: vault_path(&bundle.note_to),
        pdf: vault_path(&bundle.pdf_to),
        stem: bundle.new_stem.clone(),
    };
    let links_info = LinksInfo {
        rewritten: link_plan.rewritten,
        files: link_plan
            .files
            .iter()
            .map(|file| vault_path(&file.path))
            .collect(),
        skipped_ambiguous: link_plan
            .skipped_ambiguous
            .iter()
            .map(|link| AmbiguousLinkInfo {
                file: vault_path(&link.file),
                line: link.line,
                target: link.target.clone(),
            })
            .collect(),
    };
    let kept = KeptInfo {
        marker_id: row.marker_id.clone(),
        task: row.task.as_ref().map(|task| KeptTask {
            path: task.path.clone(),
            block_id: task.block_id.clone(),
        }),
    };
    let report = RenameEnvelope {
        ok: true,
        schema_version: crate::native::ref_library::output::REF_SCHEMA_VERSION,
        command: RENAME_COMMAND,
        generated_at: crate::native::ref_library::output::generated_at(),
        mode: if dry_run { "dry_run" } else { "write" },
        changed: true,
        outcome: "renamed",
        from,
        to,
        moves: bundle.moves,
        links: links_info,
        frontmatter,
        kept,
        reference: show_row,
        commit: None,
        sync: SyncInfo {
            pre: "skipped",
            post: "skipped",
        },
        warnings,
    };
    Ok(PreparedPlan {
        report,
        link_rewrites: link_plan.files,
        own_note_rewrite,
        touched_paths,
    })
}

fn unchanged_plan(
    row: &RefRow,
    show_row: ShowRow,
    bundle: Bundle,
    dry_run: bool,
) -> PreparedPlan {
    PreparedPlan {
        report: RenameEnvelope {
            ok: true,
            schema_version:
                crate::native::ref_library::output::REF_SCHEMA_VERSION,
            command: RENAME_COMMAND,
            generated_at: crate::native::ref_library::output::generated_at(),
            mode: if dry_run { "dry_run" } else { "write" },
            changed: false,
            outcome: "unchanged",
            from: RefLocation {
                path: vault_path(&bundle.note_from),
                pdf: vault_path(&bundle.pdf_from),
                stem: bundle.old_stem.clone(),
            },
            to: RefLocation {
                path: vault_path(&bundle.note_to),
                pdf: vault_path(&bundle.pdf_to),
                stem: bundle.new_stem.clone(),
            },
            moves: Vec::new(),
            links: LinksInfo {
                rewritten: 0,
                files: Vec::new(),
                skipped_ambiguous: Vec::new(),
            },
            frontmatter: Vec::new(),
            kept: KeptInfo {
                marker_id: row.marker_id.clone(),
                task: row.task.as_ref().map(|task| KeptTask {
                    path: task.path.clone(),
                    block_id: task.block_id.clone(),
                }),
            },
            reference: show_row,
            commit: None,
            sync: SyncInfo {
                pre: "skipped",
                post: "skipped",
            },
            warnings: Vec::new(),
        },
        link_rewrites: Vec::new(),
        own_note_rewrite: None,
        touched_paths: BTreeSet::new(),
    }
}

fn no_op_bundle(
    row: &RefRow,
    old_stem: &str,
    new_stem: &str,
) -> std::result::Result<Bundle, RenameFailure> {
    let note_from = PathBuf::from(&row.path);
    let note_to = note_from.clone();
    let pdf_from = row
        .source_pdf
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_default();
    if pdf_from.as_os_str().is_empty() {
        return Err(RenameFailure::new(
            "legacy_reference",
            format!("reference has no paired PDF: {}", row.path),
            Some("v1 renames only modern PDF-backed references"),
        ));
    }
    Ok(Bundle {
        old_stem: old_stem.to_string(),
        new_stem: new_stem.to_string(),
        note_from,
        note_to,
        pdf_to: pdf_from.clone(),
        pdf_from,
        moves: Vec::new(),
        link_map: BTreeMap::new(),
        frontmatter_map: Vec::new(),
        audio_paths: Vec::new(),
    })
}

fn build_bundle(
    config: &Config,
    row: &RefRow,
    old_stem: &str,
    new_stem: &str,
) -> std::result::Result<Bundle, RenameFailure> {
    let source_pdf = row.source_pdf.as_deref().ok_or_else(|| {
        RenameFailure::new(
            "legacy_reference",
            format!("reference has no paired PDF: {}", row.path),
            Some("v1 renames only modern PDF-backed references"),
        )
    })?;
    let pdf_absolute = config.bob_dir.join(source_pdf);
    let pdf_from = pdf_absolute
        .strip_prefix(&config.bob_dir)
        .map(Path::to_path_buf)
        .map_err(|_| {
            RenameFailure::new(
                "not_in_sync",
                format!("source_pdf is outside the vault: {source_pdf}"),
                Some("run bob ref scan and retry"),
            )
        })?;
    let lib_relative = pdf_absolute
        .strip_prefix(&config.lib_dir)
        .map_err(|_| {
            RenameFailure::new(
                "not_in_sync",
                format!("source_pdf is not under the configured library: {source_pdf}"),
                Some("run bob ref scan and retry"),
            )
        })?;
    let note_from = PathBuf::from(&row.path);
    let note_under_ref = config
        .bob_dir
        .join(&note_from)
        .strip_prefix(&config.ref_dir)
        .map(Path::to_path_buf)
        .map_err(|_| {
            RenameFailure::new(
                "legacy_reference",
                format!("reference note is outside the configured ref directory: {}", row.path),
                Some("v1 renames only notes under ref/<type>/"),
            )
        })?;
    let note_parent = note_under_ref
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let pdf_parent = lib_relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let note_parent = note_parent.ok_or_else(|| {
        RenameFailure::new(
            "legacy_reference",
            format!("reference is not under ref/<type>/: {}", row.path),
            Some("v1 renames only notes under ref/<type>/"),
        )
    })?;
    let pdf_parent = pdf_parent.ok_or_else(|| {
        RenameFailure::new(
            "not_in_sync",
            format!("PDF is not under lib/<type>/: {source_pdf}"),
            Some("run bob ref scan and retry"),
        )
    })?;
    let note_type = note_parent
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let pdf_type = pdf_parent
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let pdf_stem = lib_relative
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let expected_pdf =
        crate::native::highlights_ref::io::vault_relative_path_value(
            config,
            &pdf_absolute,
        );
    if note_parent.components().count() != 1
        || pdf_parent.components().count() != 1
        || note_type != pdf_type
        || pdf_stem != old_stem
        || Path::new(&row.path)
            .file_stem()
            .and_then(|value| value.to_str())
            != Some(old_stem)
        || source_pdf != expected_pdf
        || !pdf_absolute.is_file()
        || !config.bob_dir.join(&note_from).is_file()
    {
        return Err(RenameFailure::new(
            "not_in_sync",
            format!("the reference is not paired with lib/{note_type}/{old_stem}.pdf"),
            Some("run bob ref scan and retry"),
        ));
    }

    let note_to = note_from
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{new_stem}.md"));
    let pdf_to = pdf_from
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{new_stem}.pdf"));
    let mut moves = vec![
        MoveInfo {
            kind: MoveKind::Note,
            from: vault_path(&note_from),
            to: vault_path(&note_to),
        },
        MoveInfo {
            kind: MoveKind::Pdf,
            from: vault_path(&pdf_from),
            to: vault_path(&pdf_to),
        },
    ];
    let mut link_map = BTreeMap::from([
        (note_from.clone(), note_to.clone()),
        (pdf_from.clone(), pdf_to.clone()),
    ]);
    let mut frontmatter_map = vec![
        (vault_path(&pdf_from), vault_path(&pdf_to)),
        (vault_path(&note_from), vault_path(&note_to)),
    ];

    let lib_parent = pdf_from.parent().unwrap_or_else(|| Path::new(""));
    let sidecar_file_from = lib_parent.join(format!("{old_stem}.md"));
    let sidecar_file_to = lib_parent.join(format!("{new_stem}.md"));
    if config.bob_dir.join(&sidecar_file_from).exists() {
        moves.push(MoveInfo {
            kind: MoveKind::Sidecar,
            from: vault_path(&sidecar_file_from),
            to: vault_path(&sidecar_file_to),
        });
        link_map.insert(sidecar_file_from.clone(), sidecar_file_to.clone());
        frontmatter_map.push((
            vault_path(&sidecar_file_from),
            vault_path(&sidecar_file_to),
        ));
    }

    let bundle_from = lib_parent.join(format!("{old_stem}.textbundle"));
    let bundle_to = lib_parent.join(format!("{new_stem}.textbundle"));
    if config.bob_dir.join(&bundle_from).exists() {
        moves.push(MoveInfo {
            kind: MoveKind::Sidecar,
            from: vault_path(&bundle_from),
            to: vault_path(&bundle_to),
        });
        link_map.insert(bundle_from.clone(), bundle_to.clone());
        link_map.insert(bundle_from.join("text.md"), bundle_to.join("text.md"));
        link_map.insert(
            bundle_from.join("text.markdown"),
            bundle_to.join("text.markdown"),
        );
        frontmatter_map
            .push((vault_path(&bundle_from), vault_path(&bundle_to)));
        frontmatter_map.push((
            vault_path(&bundle_from.join("text.md")),
            vault_path(&bundle_to.join("text.md")),
        ));
        frontmatter_map.push((
            vault_path(&bundle_from.join("text.markdown")),
            vault_path(&bundle_to.join("text.markdown")),
        ));
    }

    let mut audio_paths = Vec::new();
    for extension in AUDIO_EXTENSIONS {
        let audio_from = lib_parent.join(format!("{old_stem}.{extension}"));
        let audio_to = lib_parent.join(format!("{new_stem}.{extension}"));
        if config.bob_dir.join(&audio_from).is_file() {
            moves.push(MoveInfo {
                kind: MoveKind::Audio,
                from: vault_path(&audio_from),
                to: vault_path(&audio_to),
            });
            audio_paths.push(audio_from.clone());
            link_map.insert(audio_from.clone(), audio_to.clone());
            frontmatter_map
                .push((vault_path(&audio_from), vault_path(&audio_to)));
        }
    }

    Ok(Bundle {
        old_stem: old_stem.to_string(),
        new_stem: new_stem.to_string(),
        note_from,
        note_to,
        pdf_from,
        pdf_to,
        moves,
        link_map,
        frontmatter_map,
        audio_paths,
    })
}

fn is_legacy(row: &RefRow, configured_ref_dir: &Path) -> bool {
    if row.era == "legacy" || row.source_pdf.is_none() {
        return true;
    }
    let path = Path::new(&row.path);
    let Some(relative) = path
        .strip_prefix(configured_ref_dir.file_name().unwrap_or_default())
        .ok()
    else {
        return false;
    };
    relative
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
        .is_some_and(|name| LEGACY_REF_DIRS.contains(&name))
}

fn invalid_name_failure(name: &str) -> RenameFailure {
    let without_pdf = name
        .strip_suffix(".pdf")
        .or_else(|| name.strip_suffix(".PDF"))
        .unwrap_or(name);
    let suggestion = super::super::clip_url::snake_case(without_pdf);
    let hint = if suggestion.is_empty() {
        "use [A-Za-z0-9][A-Za-z0-9_.-]*, for example research_prototype"
            .to_string()
    } else {
        format!("try the snake_case stem `{suggestion}`")
    };
    RenameFailure::new(
        "invalid_name",
        format!("invalid reference file name: {name:?}"),
        Some(hint),
    )
}

fn candidate_hint(candidates: &[ErrorCandidate]) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    Some(format!(
        "Candidates: {}",
        candidates
            .iter()
            .map(|candidate| format!(
                "{} ({})",
                candidate.path, candidate.title
            ))
            .collect::<Vec<_>>()
            .join("; ")
    ))
}

fn intake_query(config: &Config, query: &str) -> bool {
    let value = crate::native::env::expand_tilde(Path::new(query));
    let intake_root = &config.xlib_dir;
    if value.is_absolute() && value.starts_with(intake_root) {
        return true;
    }
    let under_vault = if value.is_absolute() {
        value
            .strip_prefix(&config.bob_dir)
            .ok()
            .map(Path::to_path_buf)
    } else {
        Some(value.to_path_buf())
    };
    let Some(relative) = under_vault else {
        return false;
    };
    let xlib_relative = intake_root
        .strip_prefix(&config.bob_dir)
        .unwrap_or(Path::new("xlib"));
    relative.starts_with(xlib_relative)
}

fn destinations_taken(
    config: &Config,
    bundle: &Bundle,
) -> std::result::Result<bool, RenameFailure> {
    let mut sources = BTreeSet::new();
    for movement in &bundle.moves {
        sources.insert(normalized_path(&movement.from));
    }
    let mut destinations = Vec::new();
    for movement in &bundle.moves {
        destinations.push(PathBuf::from(&movement.to));
    }
    let library_parent =
        bundle.pdf_from.parent().unwrap_or_else(|| Path::new(""));
    let reference_parent =
        bundle.note_from.parent().unwrap_or_else(|| Path::new(""));
    destinations.extend([
        reference_parent.join(format!("{}.assets", bundle.new_stem)),
        library_parent.join(format!("{}.md", bundle.new_stem)),
        library_parent.join(format!("{}.textbundle", bundle.new_stem)),
    ]);
    destinations.extend(AUDIO_EXTENSIONS.iter().map(|extension| {
        library_parent.join(format!("{}.{}", bundle.new_stem, extension))
    }));
    let type_name = bundle
        .pdf_from
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let xlib_destination = config
        .xlib_dir
        .join(type_name)
        .join(format!("{}.pdf", bundle.new_stem));
    let xlib_taken =
        case_insensitive_entry_exists(&xlib_destination).map_err(|error| {
            RenameFailure::new(
                "not_in_sync",
                format!("could not inspect intake paths: {error}"),
                Some("check intake directory permissions and retry"),
            )
        })?;
    if let Ok(relative) = xlib_destination.strip_prefix(&config.bob_dir) {
        destinations.push(relative.to_path_buf());
    }
    let mut existing = Vec::new();
    collect_vault_entries(&config.bob_dir, &config.bob_dir, &mut existing)
        .map_err(|error| {
            RenameFailure::new(
                "not_in_sync",
                format!("could not inspect vault paths: {error}"),
                Some("check vault directory permissions and retry"),
            )
        })?;
    if xlib_taken {
        return Ok(true);
    }
    for destination in &destinations {
        let wanted = normalized_path(&vault_path(destination));
        if existing.iter().any(|(path, _)| {
            normalized_path(&vault_path(path)) == wanted
                && !sources.contains(&normalized_path(&vault_path(path)))
        }) {
            return Ok(true);
        }
    }
    let wanted_md = format!("{}.md", bundle.new_stem).to_ascii_lowercase();
    Ok(existing.iter().any(|(path, is_dir)| {
        !is_dir
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase() == wanted_md)
            && !sources.contains(&normalized_path(&vault_path(path)))
    }))
}

fn case_insensitive_entry_exists(path: &Path) -> std::io::Result<bool> {
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return Ok(false);
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    for entry in entries {
        if entry?
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn collect_vault_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(PathBuf, bool)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let file_name = entry.file_name();
        if file_name.to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        entries.push((relative.clone(), kind.is_dir()));
        if kind.is_dir() {
            collect_vault_entries(root, &path, entries)?;
        }
    }
    Ok(())
}

fn has_managed_highlights(contents: &str) -> bool {
    let body = split_frontmatter(contents)
        .map(|(_, body)| body)
        .unwrap_or_else(|| contents.to_string());
    let parts = split_note_body(&body);
    let Some(region) = parts.region else {
        return false;
    };
    let parsed = parse_managed_region(&region);
    parsed
        .blocks
        .iter()
        .any(|block| block.block_id.starts_with("h-"))
        || parsed.removed.iter().any(|id| id.starts_with("h-"))
}

fn pdf_has_listen_card_link(pdf: &Path, audio_paths: &[PathBuf]) -> bool {
    if audio_paths.is_empty() {
        return false;
    }
    let Ok(document) = Document::load(pdf) else {
        return false;
    };
    let audio_paths = audio_paths
        .iter()
        .map(|path| normalized_path(&vault_path(path)))
        .collect::<BTreeSet<_>>();
    for page_id in document.page_iter() {
        let Ok(annotation_ids) = annotation_ids_for_page(&document, page_id)
        else {
            continue;
        };
        for annotation_id in annotation_ids {
            let Ok(annotation) = document.get_dictionary(annotation_id) else {
                continue;
            };
            if !annotation
                .get(b"Subtype")
                .and_then(Object::as_name)
                .is_ok_and(|subtype| subtype == b"Link")
            {
                continue;
            }
            let Ok(action) = annotation.get(b"A") else {
                continue;
            };
            let action = match action {
                Object::Reference(id) => document.get_object(*id).ok(),
                other => Some(other),
            };
            let Some(action) = action.and_then(|action| action.as_dict().ok())
            else {
                continue;
            };
            let Some(uri) = action
                .get(b"URI")
                .ok()
                .and_then(|uri| decode_text_string(uri).ok())
            else {
                continue;
            };
            if !uri.starts_with("obsidian://") {
                continue;
            }
            let Ok(uri) = url::Url::parse(&uri) else {
                continue;
            };
            if uri.query_pairs().any(|(key, value)| {
                key == "file" && audio_paths.contains(&normalized_path(&value))
            }) {
                return true;
            }
        }
    }
    false
}

fn is_in_sync(config: &Config, pdf_relative: &Path) -> bool {
    let pdf = config.bob_dir.join(pdf_relative);
    let ctx = ScanContext::build(config);
    let Ok(mut plan) = plan_pdf_sync(
        config,
        &pdf,
        SyncOptions {
            dry_run: true,
            write_pdf: false,
            prefer: None,
        },
        &ctx,
    ) else {
        return false;
    };
    let mut plans = [&mut plan];
    if finalize_annotation_task_plans(config, &mut plans, &ctx).is_err() {
        return false;
    }
    reserve_preview_ids(config, &mut plans, &ctx);
    let reading_unchanged_or_adopted =
        plan.reading_task_plan.as_ref().is_some_and(|reading| {
            reading.action == ReadingTaskAction::NoWrite
                && (plan.reading_located.is_some()
                    || reading.kind == ReadingTaskKind::Birth)
        });
    plan.stable_note_action == "none"
        && !plan.marker_write_needed
        && reading_unchanged_or_adopted
}

fn rewrite_own_frontmatter(
    contents: &str,
    replacements: &[(String, String)],
) -> Option<(String, Vec<String>)> {
    if replacements.is_empty() {
        return None;
    }
    let mut lines = contents.split_inclusive('\n');
    let first = lines.next()?;
    if !matches!(first.trim(), "---") {
        return None;
    }
    let mut output = String::with_capacity(contents.len());
    output.push_str(first);
    let mut fields = BTreeSet::new();
    let mut ended = false;
    for line in lines.by_ref() {
        let line_without_ending = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .strip_suffix('\r')
            .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line));
        if matches!(line_without_ending.trim(), "---" | "...") {
            output.push_str(line);
            ended = true;
            break;
        }
        let key = line_without_ending
            .split_once(':')
            .map(|(key, _)| key.trim());
        let allowed =
            matches!(key, Some("source_pdf" | "highlights_sidecar" | "audio"));
        let mut rewritten = line.to_string();
        if allowed {
            for (old, new) in replacements {
                if rewritten.contains(old) {
                    rewritten = rewritten.replace(old, new);
                }
            }
            if rewritten != line {
                fields.insert(key.unwrap().to_string());
            }
        }
        output.push_str(&rewritten);
    }
    if !ended {
        return None;
    }
    for line in lines {
        output.push_str(line);
    }
    (!fields.is_empty()).then_some((output, fields.into_iter().collect()))
}

fn dirty_touched_paths(
    vault: &Path,
    touched_paths: &BTreeSet<PathBuf>,
) -> std::result::Result<Vec<PathBuf>, RenameFailure> {
    if !vault.join(".git").exists() {
        return Ok(Vec::new());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
        .map_err(|error| {
            RenameFailure::new(
                "not_in_sync",
                format!("could not inspect Git status: {error}"),
                Some("check Git availability and retry"),
            )
        })?;
    if !output.status.success() {
        return Err(RenameFailure::new(
            "not_in_sync",
            "could not inspect Git status for the rename plan",
            Some("check that the Bob vault is a Git worktree and retry"),
        ));
    }
    let mut records = output.stdout.split(|byte| *byte == 0);
    let mut dirty = BTreeSet::new();
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]);
        let path =
            PathBuf::from(String::from_utf8_lossy(&record[3..]).as_ref());
        dirty.insert(path);
        if (status.contains('R') || status.contains('C'))
            && let Some(previous) = records.next()
        {
            dirty.insert(PathBuf::from(
                String::from_utf8_lossy(previous).as_ref(),
            ));
        }
    }
    let touched = touched_paths
        .iter()
        .map(|path| normalized_path(&vault_path(path)))
        .collect::<BTreeSet<_>>();
    let mut matches = dirty
        .into_iter()
        .filter(|path| touched.contains(&normalized_path(&vault_path(path))))
        .collect::<Vec<_>>();
    matches.sort();
    Ok(matches)
}

fn print_failure(format: Format, failure: RenameFailure) -> i32 {
    crate::native::ref_library::output::print_find_error(
        format,
        RENAME_COMMAND,
        failure.code,
        &failure.message,
        failure.hint.as_deref(),
    );
    1
}

fn vault_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalized_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| vault_path(path))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_frontmatter_rewrite_changes_only_managed_path_fields() {
        let input = concat!(
            "---\n",
            "id: old\n",
            "source_url: https://example.test/old.pdf\n",
            "source_pdf: lib/blogs/old.pdf\n",
            "highlights_sidecar: \"[[lib/blogs/old.textbundle]]\"\n",
            "audio: \"[[lib/blogs/old.mp3]]\"\n",
            "---\n",
            "![[sase#^ref-old]]\n",
        );
        let map = vec![
            (
                "lib/blogs/old.pdf".to_string(),
                "lib/blogs/new.pdf".to_string(),
            ),
            (
                "lib/blogs/old.textbundle".to_string(),
                "lib/blogs/new.textbundle".to_string(),
            ),
            (
                "lib/blogs/old.mp3".to_string(),
                "lib/blogs/new.mp3".to_string(),
            ),
        ];
        let (actual, fields) = rewrite_own_frontmatter(input, &map).unwrap();
        assert_eq!(
            actual,
            concat!(
                "---\n",
                "id: old\n",
                "source_url: https://example.test/old.pdf\n",
                "source_pdf: lib/blogs/new.pdf\n",
                "highlights_sidecar: \"[[lib/blogs/new.textbundle]]\"\n",
                "audio: \"[[lib/blogs/new.mp3]]\"\n",
                "---\n",
                "![[sase#^ref-old]]\n",
            )
        );
        assert_eq!(fields, ["audio", "highlights_sidecar", "source_pdf"]);
    }

    #[test]
    fn forced_verify_failure_reverses_moves_and_restores_rewrites() {
        let temp = tempfile::tempdir().expect("create scratch vault");
        let vault = temp.path();
        fs::create_dir_all(vault.join("ref/blogs")).unwrap();
        fs::create_dir_all(vault.join("lib/blogs")).unwrap();
        let old_note = vault.join("ref/blogs/old.md");
        let old_pdf = vault.join("lib/blogs/old.pdf");
        let linked_note = vault.join("sase.md");
        fs::write(&old_note, "original reference\n").unwrap();
        fs::write(&old_pdf, b"original PDF bytes").unwrap();
        fs::write(&linked_note, "[[ref/blogs/old]]\n").unwrap();

        let mut rewrites = BTreeMap::new();
        rewrites.insert(
            PathBuf::from("sase.md"),
            "[[ref/blogs/new]]\n".to_string(),
        );
        let result = apply_transaction(
            vault,
            &[
                MoveInfo {
                    kind: MoveKind::Note,
                    from: "ref/blogs/old.md".to_string(),
                    to: "ref/blogs/new.md".to_string(),
                },
                MoveInfo {
                    kind: MoveKind::Pdf,
                    from: "lib/blogs/old.pdf".to_string(),
                    to: "lib/blogs/new.pdf".to_string(),
                },
            ],
            &rewrites,
            || Err("forced verifier failure".to_string()),
        );

        assert!(result.unwrap_err().contains("forced verifier failure"));
        assert_eq!(
            fs::read_to_string(old_note).unwrap(),
            "original reference\n"
        );
        assert_eq!(fs::read(old_pdf).unwrap(), b"original PDF bytes");
        assert_eq!(
            fs::read_to_string(linked_note).unwrap(),
            "[[ref/blogs/old]]\n"
        );
        assert!(!vault.join("ref/blogs/new.md").exists());
        assert!(!vault.join("lib/blogs/new.pdf").exists());
    }

    #[test]
    fn legacy_paths_are_recognized_by_ref_type() {
        let row = RefRow {
            path: "ref/ai/book.md".into(),
            link: String::new(),
            title: String::new(),
            marker_id: None,
            origin: String::new(),
            ref_type: None,
            era: "modern".into(),
            status: None,
            status_sync: String::new(),
            frontmatter_status: None,
            legacy_status: None,
            reading_state: String::new(),
            reading_state_source: String::new(),
            blocked: false,
            parent: None,
            task: None,
            urls: Vec::new(),
            identity: crate::native::ref_library::RefIdentity {
                keys: Vec::new(),
                arxiv: None,
                doi: None,
            },
            author: None,
            published: None,
            captured: None,
            added: None,
            added_source: None,
            finished: None,
            finished_source: None,
            source_pdf: Some("lib/ai/book.pdf".into()),
            audio: None,
            annotation_count: 0,
            comment_count: 0,
            snapshot: None,
            research_ref: None,
            superseded_by: None,
            diagnostics: Vec::new(),
            id: None,
            source_block: None,
            source_path: None,
            source_id: None,
            source_blocks: Vec::new(),
            v2: false,
        };
        assert!(is_legacy(&row, Path::new("/vault/ref")));
    }
}
