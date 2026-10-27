//! Read-only plan construction for `bob ref rename`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{super::*, links};
use crate::native::{
    highlights_ref::reference_mutation::{
        acquire_mutation_session, committed_despite_error, dirty_touched_paths,
        display_paths, git_head, has_managed_highlights, intake_query,
        is_in_sync, is_legacy, latest_commit_for_path, normalized_path,
        restore_bytes_if_uncontended, rollback_moves,
        unstage_paths as unstage_scoped_paths, vault_path, DirtyPathError,
        MutationSessionError,
    },
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
    pub(super) task: Option<KeptTask>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct IdInfo {
    pub(super) from: Option<String>,
    pub(super) to: Option<String>,
    pub(super) changed: bool,
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
    pub(super) id: IdInfo,
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
    if !offline
        && let Some(detail) =
            super::super::intake::vault_identity_problem(config)
    {
        return print_failure(
            format,
            RenameFailure::new(
                "vault_mismatch",
                detail,
                Some("point BOB_DIR at the vault passed with -b, or use --offline"),
            ),
        );
    }
    let child_env = ob::child_env();
    let json_mode = format == Format::Json;
    let _locks = match acquire_mutation_session(
        &config.bob_dir,
        &child_env,
        move || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                eprintln!(
                    "  {}",
                    crate::native::style::Styler::detect()
                        .dim("waiting for another vault maintenance run…")
                );
            }
        },
    ) {
        Ok(locks) => locks,
        Err(MutationSessionError::Maintenance(error)) => {
            return print_failure(
                format,
                RenameFailure::new(
                    "lock_busy",
                    format!("could not acquire the vault maintenance lock: {error}"),
                    Some("wait for the other maintenance run to finish, then retry"),
                ),
            );
        }
        Err(MutationSessionError::NotAWorktree) => {
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
        Err(MutationSessionError::Scan(error)) => {
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

    match crate::native::highlights_ref::overwrite::recovery::recover_with_locks_held(
        config, &child_env, false,
    ) {
        Ok(Some(notice)) => eprintln!("{notice}"),
        Ok(None) => {}
        Err(error) => {
            return print_failure(
                format,
                RenameFailure::new(
                    error.code.unwrap_or("recovery_failed"),
                    error.to_string(),
                    Some("run bob ref scan on this host to recover the interrupted overwrite"),
                ),
            );
        }
    }

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
    let session = match MarkerWriteSession::for_explicit_write() {
        Ok(session) => session,
        Err(error) => {
            return print_failure(
                format,
                RenameFailure::new(
                    error.code.unwrap_or("not_in_sync"),
                    format!("could not load Highlights configuration: {error}"),
                    Some("check the Highlights configuration and retry"),
                ),
            );
        }
    };
    let sha = match execute_rename_with_session(
        config,
        &prepared,
        &child_env,
        old_head.as_deref(),
        &prior_row,
        &session,
    ) {
        Ok(sha) => sha,
        Err(failure) => return print_failure(format, failure),
    };
    let subject = rename_subject(&prepared);

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
        id: IdInfo {
            from: row.marker_id.clone(),
            to: row.marker_id.clone(),
            changed: false,
        },
        moves: Vec::new(),
        links: LinksInfo {
            rewritten: 0,
            files: Vec::new(),
            skipped_ambiguous: Vec::new(),
        },
        frontmatter: Vec::new(),
        kept: KeptInfo {
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

#[derive(Debug, Default)]
struct AppliedRename {
    before: crate::native::ref_library::migrate_tasks::BeforeImages,
    after: BTreeMap<PathBuf, Vec<u8>>,
    pdf_before: Option<(PathBuf, Vec<u8>)>,
    pdf_after: Option<Vec<u8>>,
}

struct RenameSnapshots {
    from_abs: PathBuf,
    to_abs: PathBuf,
    from_bytes: Vec<u8>,
    from_marker: String,
    touched: BTreeMap<PathBuf, Option<Vec<u8>>>,
}

fn snapshot_rename_inputs(
    config: &Config,
    prepared: &PreparedPlan,
) -> std::result::Result<RenameSnapshots, RenameFailure> {
    let from_rel = PathBuf::from(&prepared.report.from.pdf);
    let to_rel = PathBuf::from(&prepared.report.to.pdf);
    let from_abs = config.bob_dir.join(&from_rel);
    let to_abs = config.bob_dir.join(&to_rel);
    let from_bytes = fs::read(&from_abs).map_err(|error| {
        RenameFailure::new(
            "not_in_sync",
            format!("could not read {}: {error}", from_abs.display()),
            Some("run bob ref scan and retry"),
        )
    })?;
    let from_marker = read_pdf_marker_from_bytes(&from_abs, &from_bytes)
        .map_err(|error| {
            RenameFailure::new(
                "not_in_sync",
                format!("could not read PDF marker: {error}"),
                Some("run bob ref scan and retry"),
            )
        })?;
    let mut touched = BTreeMap::new();
    for relative in &prepared.touched_paths {
        let absolute = config.bob_dir.join(relative);
        touched.insert(absolute.clone(), fs::read(&absolute).ok());
    }
    Ok(RenameSnapshots {
        from_abs,
        to_abs,
        from_bytes: from_bytes.clone(),
        from_marker: from_marker.contents,
        touched,
    })
}

fn rename_hold_failure(code: &'static str, message: String) -> RenameFailure {
    let hint = if matches!(
        code,
        "marker_changed" | "coordination_timeout" | "coordination_failed"
    ) {
        "close it in Highlights and retry the rename"
    } else {
        "run bob ref scan -w to commit the save, then retry the rename"
    };
    RenameFailure::new(code, message, Some(hint))
}

fn current_marker_contents(
    path: &Path,
    bytes: &[u8],
) -> std::result::Result<String, String> {
    read_pdf_marker_from_bytes(path, bytes)
        .map(|marker| marker.contents)
        .map_err(|error| format!("read PDF marker {}: {error}", path.display()))
}

fn collect_rewrites(prepared: &PreparedPlan) -> BTreeMap<PathBuf, String> {
    let mut rewrites = BTreeMap::<PathBuf, String>::new();
    for rewrite in &prepared.link_rewrites {
        rewrites.insert(rewrite.path.clone(), rewrite.contents.clone());
    }
    if let Some((path, contents)) = &prepared.own_note_rewrite {
        rewrites.insert(path.clone(), contents.clone());
    }
    rewrites
}

fn check_touched_unchanged(
    config: &Config,
    prepared: &PreparedPlan,
    snapshots: &RenameSnapshots,
    skip_pdf: bool,
) -> std::result::Result<(), RenameFailure> {
    for relative in &prepared.touched_paths {
        let absolute = config.bob_dir.join(relative);
        if skip_pdf && absolute == snapshots.from_abs {
            continue;
        }
        let expected = snapshots.touched.get(&absolute);
        let current = fs::read(&absolute).ok();
        if Some(&current) != expected {
            return Err(rename_hold_failure(
                "preimage_changed",
                format!(
                    "a planned path changed while the rename waited; no files were written: {}",
                    vault_path(absolute.strip_prefix(&config.bob_dir).unwrap_or(&absolute)),
                ),
            ));
        }
    }
    Ok(())
}

fn check_destinations_absent(
    config: &Config,
    prepared: &PreparedPlan,
) -> std::result::Result<(), RenameFailure> {
    for movement in &prepared.report.moves {
        let to_abs = config.bob_dir.join(PathBuf::from(&movement.to));
        if to_abs.exists() {
            return Err(rename_hold_failure(
                "preimage_changed",
                format!(
                    "the rename destination is now occupied; no files were written: {}",
                    movement.to,
                ),
            ));
        }
    }
    Ok(())
}

fn revalidate_full_rename(
    config: &Config,
    prepared: &PreparedPlan,
    snapshots: &RenameSnapshots,
) -> std::result::Result<Vec<u8>, RenameFailure> {
    let current = fs::read(&snapshots.from_abs).map_err(|error| {
        rename_hold_failure(
            "preimage_changed",
            format!(
                "source PDF changed during coordination; no files were written: {}: {error}",
                snapshots.from_abs.display()
            ),
        )
    })?;
    if current != snapshots.from_bytes {
        let current_marker =
            current_marker_contents(&snapshots.from_abs, &current).ok();
        let saved_marker_ok = current_marker
            .as_deref()
            .is_some_and(|contents| contents == snapshots.from_marker);
        if current_marker.is_some() && !saved_marker_ok {
            return Err(rename_hold_failure(
                "marker_changed",
                "marker changed in Highlights during the rename; no files were written; close it in Highlights and retry".to_string(),
            ));
        }
        return Err(rename_hold_failure(
            "preimage_changed",
            "the source PDF was saved in Highlights while the rename waited; no files were written".to_string(),
        ));
    }
    check_destinations_absent(config, prepared)?;
    check_touched_unchanged(config, prepared, snapshots, true)?;
    Ok(current)
}

enum SameStemRecheck {
    Unchanged,
    Saved(Vec<u8>),
}

fn revalidate_same_stem(
    config: &Config,
    prepared: &PreparedPlan,
    snapshots: &RenameSnapshots,
) -> std::result::Result<SameStemRecheck, RenameFailure> {
    let current = fs::read(&snapshots.from_abs).map_err(|error| {
        rename_hold_failure(
            "preimage_changed",
            format!(
                "source PDF changed during coordination; no files were written: {}: {error}",
                snapshots.from_abs.display()
            ),
        )
    })?;
    check_touched_unchanged(config, prepared, snapshots, true)?;
    if current == snapshots.from_bytes {
        return Ok(SameStemRecheck::Unchanged);
    }
    let current_marker = current_marker_contents(&snapshots.from_abs, &current)
        .map_err(|_| {
            rename_hold_failure(
                "marker_changed",
                "marker changed in Highlights during the rename; no files were written; close it in Highlights and retry".to_string(),
            )
        })?;
    if current_marker != snapshots.from_marker {
        return Err(rename_hold_failure(
            "marker_changed",
            "marker changed in Highlights during the rename; no files were written; close it in Highlights and retry".to_string(),
        ));
    }
    Ok(SameStemRecheck::Saved(current))
}

fn rename_subject(prepared: &PreparedPlan) -> String {
    if prepared.report.outcome == "id_updated" {
        format!(
            "bob ref rename: {} (id {} → {})",
            prepared.report.from.stem,
            prepared.report.id.from.as_deref().unwrap_or("(none)"),
            prepared.report.id.to.as_deref().unwrap_or("(none)")
        )
    } else {
        format!(
            "bob ref rename: {} → {}",
            prepared.report.from.stem, prepared.report.to.stem
        )
    }
}

#[cfg(test)]
thread_local! {
    static RENAME_FORCE_VERIFY: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    static RENAME_FORCE_COMMIT_FAIL: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    static RENAME_COMMIT_REPORTED_AS_ERROR: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn test_force_verify_failure() -> bool {
    RENAME_FORCE_VERIFY.with(|flag| flag.get())
}

#[cfg(test)]
pub(crate) fn test_set_verify_failure(value: bool) {
    RENAME_FORCE_VERIFY.with(|flag| flag.set(value));
}

#[cfg(test)]
pub(crate) fn test_set_commit_failure(value: bool) {
    RENAME_FORCE_COMMIT_FAIL.with(|flag| flag.set(value));
}

#[cfg(test)]
pub(crate) fn test_set_commit_reported_as_error(value: bool) {
    RENAME_COMMIT_REPORTED_AS_ERROR.with(|flag| flag.set(value));
}

fn commit_inside_scope(
    config: &Config,
    child_env: &ob::ChildEnv,
    prepared: &PreparedPlan,
    old_head: Option<&str>,
    subject: &str,
) -> std::result::Result<String, (String, String)> {
    #[cfg(test)]
    if RENAME_FORCE_COMMIT_FAIL.with(|flag| flag.get()) {
        let commit_paths =
            prepared.touched_paths.iter().cloned().collect::<Vec<_>>();
        let unstage = unstage_paths(&config.bob_dir, child_env, &commit_paths);
        return Err((
            format!("Git commit failed: injected commit failure{unstage}"),
            unstage,
        ));
    }
    let commit_paths =
        prepared.touched_paths.iter().cloned().collect::<Vec<_>>();
    let commit_result =
        ob::commit_paths(&config.bob_dir, child_env, subject, &commit_paths);
    #[cfg(test)]
    let commit_result = if RENAME_COMMIT_REPORTED_AS_ERROR
        .with(|flag| flag.get())
        && matches!(commit_result, Ok(Some(_)))
    {
        Err("injected commit reporting error".to_string())
    } else {
        commit_result
    };
    match commit_result {
        Ok(Some(sha)) => Ok(sha),
        Ok(None) => {
            let unstage =
                unstage_paths(&config.bob_dir, child_env, &commit_paths);
            Err((
                format!(
                    "rename verified but produced no scoped Git commit{unstage}"
                ),
                unstage,
            ))
        }
        Err(error) => {
            if let Some((sha, committed_subject)) = committed_despite_error(
                &config.bob_dir,
                child_env,
                old_head,
                subject,
            ) && committed_subject == subject
            {
                return Ok(sha);
            }
            let unstage =
                unstage_paths(&config.bob_dir, child_env, &commit_paths);
            Err((format!("Git commit failed: {error}{unstage}"), unstage))
        }
    }
}

fn rollback_inside_move_scope(
    transaction: &AppliedRename,
    pdf_moved: bool,
    other_moves: &[(PathBuf, PathBuf)],
    ctx: &mut crate::native::file_coordination::MoveContext<'_>,
) -> String {
    let mut failures = Vec::new();
    if pdf_moved && let Err(error) = ctx.move_back() {
        failures
            .push(format!("could not move PDF back to its source: {error}"));
    }
    failures.extend(rollback_moves(other_moves));
    if let Some((path, before)) = &transaction.pdf_before
        && let Some(message) = restore_bytes_if_uncontended(
            path,
            before,
            transaction.pdf_after.as_deref(),
            "PDF",
        )
    {
        failures.push(message);
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

fn rollback_inside_merge_scope(transaction: &AppliedRename) -> String {
    let mut failures = Vec::new();
    if let Some((path, before)) = &transaction.pdf_before
        && let Some(message) = restore_bytes_if_uncontended(
            path,
            before,
            transaction.pdf_after.as_deref(),
            "PDF",
        )
    {
        failures.push(message);
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

fn write_id_marker_with_saved(
    config: &Config,
    prepared: &PreparedPlan,
    transaction: &mut AppliedRename,
    saved_pdf: &[u8],
) -> std::result::Result<(), String> {
    let pdf_path = config.bob_dir.join(&prepared.report.to.pdf);
    transaction.pdf_before = Some((
        config.bob_dir.join(&prepared.report.from.pdf),
        saved_pdf.to_vec(),
    ));
    let context = ScanContext::build(config);
    let mut plan = plan_pdf_sync(
        config,
        &pdf_path,
        SyncOptions {
            dry_run: false,
            write_pdf: true,
            prefer: None,
            ignore_unannotated_sidecar: false,
        },
        &context,
    )
    .map_err(|error| {
        format!("plan marker sync for {}: {error}", pdf_path.display())
    })?;
    if plan.marker.source_pdf_sha256
        != super::super::source_store::sha256_hex(saved_pdf)
    {
        return Err(
            "PDF changed between the coordination recheck and the marker plan; no files were written"
                .to_string(),
        );
    }
    let mut plans = [&mut plan];
    finalize_annotation_task_plans(config, &mut plans, &context)
        .map_err(|error| format!("finalize marker sync plan: {error}"))?;
    reserve_preview_ids(config, &mut plans, &context);

    let safe = plan.marker_write_needed
        && plan.reading_task_plan.as_ref().is_some_and(|reading| {
            reading.action == ReadingTaskAction::NoWrite
        })
        && plan.annotation_tasks_created == 0
        && plan.routed_intents.is_empty()
        && plan
            .routed_task_note_writes
            .iter()
            .all(|write| write.action == "none")
        && plan.image_assets.is_empty();
    if !safe {
        return Err(
            "marker sync plan included an unsafe or unexpected side effect"
                .to_string(),
        );
    }

    let mut session = MarkerWriteSession::for_external_coordination();
    let execution = execute_pdf_sync(
        config,
        &plan,
        &mut RunWriteState::default(),
        &mut session,
    );
    if let Ok(bytes) = fs::read(&pdf_path) {
        transaction.pdf_after = Some(bytes);
    }
    if let Ok(bytes) = fs::read(config.bob_dir.join(&prepared.report.to.path)) {
        transaction
            .after
            .insert(config.bob_dir.join(&prepared.report.from.path), bytes);
    }
    match execution {
        Ok(PdfSyncOutcome::Written(_)) => Ok(()),
        Ok(PdfSyncOutcome::Held(hold)) => {
            Err(format!("write id to PDF marker: {}", hold.message))
        }
        Err(error) => Err(format!("write id to PDF marker: {error}")),
    }
}

fn execute_rename_with_session(
    config: &Config,
    prepared: &PreparedPlan,
    child_env: &ob::ChildEnv,
    old_head: Option<&str>,
    prior_row: &RefRow,
    session: &MarkerWriteSession,
) -> std::result::Result<String, RenameFailure> {
    let snapshots = snapshot_rename_inputs(config, prepared)?;
    let is_move = !prepared.report.moves.is_empty();
    if is_move {
        let coordination = session.coordinate_explicit_pdf_move_with(
            &snapshots.from_abs,
            &snapshots.to_abs,
            |ctx| {
                rename_move_accessor(
                    config, prepared, &snapshots, child_env, old_head,
                    prior_row, ctx,
                )
            },
        );
        match coordination {
            Ok(crate::native::file_coordination::Coordination::Granted {
                value,
                ..
            }) => value,
            Ok(crate::native::file_coordination::Coordination::Held(timeout)) => {
                Err(rename_hold_failure(
                    "coordination_timeout",
                    format!(
                        "Highlights didn't answer within {:.2}s; no rename was written; close it in Highlights and retry",
                        timeout.waited.as_secs_f64()
                    ),
                ))
            }
            Ok(crate::native::file_coordination::Coordination::Failed(
                message,
            )) => Err(rename_hold_failure(
                "coordination_failed",
                format!(
                    "file coordination failed: {message}; no rename was written"
                ),
            )),
            Err(error) => Err(RenameFailure::new(
                "coordination_failed",
                format!("could not coordinate the rename: {error}"),
                Some("close it in Highlights and retry the rename"),
            )),
        }
    } else {
        let pdf_abs = snapshots.from_abs.clone();
        let coordination =
            session.coordinate_explicit_pdf_write_with(&pdf_abs, || {
                rename_merge_accessor(
                    config, prepared, &snapshots, child_env, old_head,
                    prior_row,
                )
            });
        match coordination {
            Ok(crate::native::file_coordination::Coordination::Granted {
                value,
                ..
            }) => value,
            Ok(crate::native::file_coordination::Coordination::Held(timeout)) => {
                Err(rename_hold_failure(
                    "coordination_timeout",
                    format!(
                        "Highlights didn't answer within {:.2}s; no rename was written; close it in Highlights and retry",
                        timeout.waited.as_secs_f64()
                    ),
                ))
            }
            Ok(crate::native::file_coordination::Coordination::Failed(
                message,
            )) => Err(rename_hold_failure(
                "coordination_failed",
                format!(
                    "file coordination failed: {message}; no rename was written"
                ),
            )),
            Err(error) => Err(RenameFailure::new(
                "coordination_failed",
                format!("could not coordinate the rename: {error}"),
                Some("close it in Highlights and retry the rename"),
            )),
        }
    }
}

fn rename_move_accessor(
    config: &Config,
    prepared: &PreparedPlan,
    snapshots: &RenameSnapshots,
    child_env: &ob::ChildEnv,
    old_head: Option<&str>,
    prior_row: &RefRow,
    ctx: &mut crate::native::file_coordination::MoveContext<'_>,
) -> std::result::Result<String, RenameFailure> {
    let saved = revalidate_full_rename(config, prepared, snapshots)?;
    let rewrites = collect_rewrites(prepared);
    let mut transaction = AppliedRename::default();
    let relative_paths = rewrites
        .keys()
        .map(|path| vault_path(path))
        .collect::<Vec<_>>();
    transaction.before =
        crate::native::ref_library::migrate_tasks::snapshot_before_images(
            &config.bob_dir,
            &relative_paths,
        );
    if let Some((path, _)) = transaction
        .before
        .iter()
        .find(|(_, before)| before.is_none())
    {
        return Err(rename_hold_failure(
            "preimage_changed",
            format!(
                "could not snapshot rewrite source {}; no files were written",
                path.display()
            ),
        ));
    }
    let mut pdf_moved = false;
    let mut other_moves: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut move_failed: Option<String> = None;
    for movement in &prepared.report.moves {
        let from_abs = config.bob_dir.join(PathBuf::from(&movement.from));
        let to_abs = config.bob_dir.join(PathBuf::from(&movement.to));
        if let Some(parent) = to_abs.parent()
            && !parent.is_dir()
        {
            move_failed = Some(format!(
                "destination parent does not exist: {}",
                parent.display()
            ));
            break;
        }
        let is_pdf = matches!(movement.kind, MoveKind::Pdf);
        if is_pdf {
            match ctx.move_forward() {
                Ok(()) => pdf_moved = true,
                Err(error) => {
                    move_failed = Some(format!(
                        "move {} to {}: {error}",
                        movement.from, movement.to
                    ));
                    break;
                }
            }
        } else {
            match super::super::io::rename_no_replace(&from_abs, &to_abs) {
                Ok(()) => other_moves.push((from_abs, to_abs)),
                Err(error) => {
                    move_failed = Some(format!(
                        "move {} to {}: {error}",
                        movement.from, movement.to
                    ));
                    break;
                }
            }
        }
    }
    if let Some(message) = move_failed {
        let transaction_for_rollback = AppliedRename {
            before: transaction.before.clone(),
            after: BTreeMap::new(),
            pdf_before: None,
            pdf_after: None,
        };
        let rollback = rollback_inside_move_scope(
            &transaction_for_rollback,
            pdf_moved,
            &other_moves,
            ctx,
        );
        return Err(RenameFailure::new(
            "preimage_changed",
            format!("{message}{rollback}"),
            Some(
                "run bob ref scan -w to commit the save, then retry the rename",
            ),
        ));
    }
    let moved = prepared
        .report
        .moves
        .iter()
        .map(|movement| {
            (PathBuf::from(&movement.from), PathBuf::from(&movement.to))
        })
        .collect::<BTreeMap<_, _>>();
    for (relative, contents) in &rewrites {
        let write_relative = moved.get(relative).unwrap_or(relative);
        let absolute = config.bob_dir.join(write_relative);
        if let Err(error) = super::super::atomic_write(&absolute, contents) {
            let rollback = rollback_inside_move_scope(
                &transaction,
                pdf_moved,
                &other_moves,
                ctx,
            );
            return Err(RenameFailure::new(
                "verify_failed",
                format!(
                    "rewrite {}: {error}{rollback}",
                    write_relative.display()
                ),
                Some("the rename was rolled back; inspect the listed paths and retry"),
            ));
        }
        transaction.after.insert(
            config.bob_dir.join(relative),
            contents.as_bytes().to_vec(),
        );
    }
    if prepared.report.id.changed {
        if let Err(message) = write_id_marker_with_saved(
            config,
            prepared,
            &mut transaction,
            &saved,
        ) {
            let rollback = rollback_inside_move_scope(
                &transaction,
                pdf_moved,
                &other_moves,
                ctx,
            );
            let code = if message.contains("unsafe or unexpected") {
                "verify_failed"
            } else {
                "preimage_changed"
            };
            return Err(RenameFailure::new(
                code,
                format!("{message}{rollback}"),
                Some(if code == "preimage_changed" {
                    "run bob ref scan -w to commit the save, then retry the rename"
                } else {
                    "the rename was rolled back; inspect the listed paths and retry"
                }),
            ));
        }
    } else {
        transaction.pdf_before =
            Some((snapshots.from_abs.clone(), saved.clone()));
        transaction.pdf_after = Some(saved.clone());
    }
    if let Err(message) = verify_renamed(config, prepared, prior_row) {
        let rollback = rollback_inside_move_scope(
            &transaction,
            pdf_moved,
            &other_moves,
            ctx,
        );
        return Err(RenameFailure::new(
            "verify_failed",
            format!("could not apply and verify the rename: {message}{rollback}"),
            Some("the rename was rolled back; inspect the listed paths and retry"),
        ));
    }
    #[cfg(test)]
    if test_force_verify_failure() {
        let rollback = rollback_inside_move_scope(
            &transaction,
            pdf_moved,
            &other_moves,
            ctx,
        );
        return Err(RenameFailure::new(
            "verify_failed",
            format!("could not apply and verify the rename: injected verification failure{rollback}"),
            Some("the rename was rolled back; inspect the listed paths and retry"),
        ));
    }
    let subject = rename_subject(prepared);
    match commit_inside_scope(config, child_env, prepared, old_head, &subject) {
        Ok(sha) => Ok(sha),
        Err((message, _unstage)) => {
            let rollback = rollback_inside_move_scope(
                &transaction,
                pdf_moved,
                &other_moves,
                ctx,
            );
            Err(RenameFailure::new(
                "verify_failed",
                format!("{message}{rollback}"),
                Some("check the vault Git state and retry"),
            ))
        }
    }
}

fn rename_merge_accessor(
    config: &Config,
    prepared: &PreparedPlan,
    snapshots: &RenameSnapshots,
    child_env: &ob::ChildEnv,
    old_head: Option<&str>,
    prior_row: &RefRow,
) -> std::result::Result<String, RenameFailure> {
    let saved = match revalidate_same_stem(config, prepared, snapshots)? {
        SameStemRecheck::Unchanged => snapshots.from_bytes.clone(),
        SameStemRecheck::Saved(bytes) => bytes,
    };
    let rewrites = collect_rewrites(prepared);
    let mut transaction = AppliedRename::default();
    let relative_paths = rewrites
        .keys()
        .map(|path| vault_path(path))
        .collect::<Vec<_>>();
    transaction.before =
        crate::native::ref_library::migrate_tasks::snapshot_before_images(
            &config.bob_dir,
            &relative_paths,
        );
    if let Some((path, _)) = transaction
        .before
        .iter()
        .find(|(_, before)| before.is_none())
    {
        return Err(rename_hold_failure(
            "preimage_changed",
            format!(
                "could not snapshot rewrite source {}; no files were written",
                path.display()
            ),
        ));
    }
    for (relative, contents) in &rewrites {
        let absolute = config.bob_dir.join(relative);
        if let Err(error) = super::super::atomic_write(&absolute, contents) {
            let rollback = rollback_inside_merge_scope(&transaction);
            return Err(RenameFailure::new(
                "verify_failed",
                format!(
                    "rewrite {}: {error}{rollback}",
                    relative.display()
                ),
                Some("the rename was rolled back; inspect the listed paths and retry"),
            ));
        }
        transaction
            .after
            .insert(absolute, contents.as_bytes().to_vec());
    }
    if prepared.report.id.changed
        && let Err(message) = write_id_marker_with_saved(
            config,
            prepared,
            &mut transaction,
            &saved,
        )
    {
        let rollback = rollback_inside_merge_scope(&transaction);
        let code = if message.contains("unsafe or unexpected") {
            "verify_failed"
        } else {
            "preimage_changed"
        };
        return Err(RenameFailure::new(
            code,
            format!("{message}{rollback}"),
            Some(if code == "preimage_changed" {
                "run bob ref scan -w to commit the save, then retry the rename"
            } else {
                "the rename was rolled back; inspect the listed paths and retry"
            }),
        ));
    }
    if let Err(message) = verify_renamed(config, prepared, prior_row) {
        let rollback = rollback_inside_merge_scope(&transaction);
        return Err(RenameFailure::new(
            "verify_failed",
            format!("could not apply and verify the rename: {message}{rollback}"),
            Some("the rename was rolled back; inspect the listed paths and retry"),
        ));
    }
    #[cfg(test)]
    if test_force_verify_failure() {
        let rollback = rollback_inside_merge_scope(&transaction);
        return Err(RenameFailure::new(
            "verify_failed",
            format!("could not apply and verify the rename: injected verification failure{rollback}"),
            Some("the rename was rolled back; inspect the listed paths and retry"),
        ));
    }
    let subject = rename_subject(prepared);
    match commit_inside_scope(config, child_env, prepared, old_head, &subject) {
        Ok(sha) => Ok(sha),
        Err((message, _unstage)) => {
            let rollback = rollback_inside_merge_scope(&transaction);
            Err(RenameFailure::new(
                "verify_failed",
                format!("{message}{rollback}"),
                Some("check the vault Git state and retry"),
            ))
        }
    }
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
    if prepared.report.from.path != prepared.report.to.path
        && index
            .rows
            .iter()
            .any(|row| row.path == prepared.report.from.path)
    {
        return Err(format!(
            "old index row remains at {}",
            prepared.report.from.path
        ));
    }
    if row.marker_id != prepared.report.id.to {
        return Err("reference id does not match the planned id".to_string());
    }
    let marker = read_pdf_marker(&config.bob_dir.join(&prepared.report.to.pdf))
        .map_err(|error| format!("read renamed PDF marker: {error}"))?;
    let marker_id = parse_marker_with_normalization(&marker.contents)
        .map_err(|error| format!("parse renamed PDF marker: {error}"))?
        .projection
        .get(FIELD_ID)
        .cloned();
    let expected_marker_id = prepared
        .report
        .id
        .to
        .as_ref()
        .map(|value| MarkerValue::String(value.clone()));
    if marker_id != expected_marker_id {
        return Err("PDF marker id does not match the planned id".to_string());
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

fn unstage_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    paths: &[PathBuf],
) -> String {
    match unstage_scoped_paths(vault, child_env, paths) {
        Ok(()) => String::new(),
        Err(error) => format!("; could not unstage rename paths: {error}"),
    }
}

fn allocate_row_id(rows: &[RefRow], self_index: usize, stem: &str) -> String {
    allocate_reference_id(
        stem,
        rows.iter()
            .enumerate()
            .filter(|(index, _)| *index != self_index)
            .map(|(_, row)| {
                (
                    row.marker_id.as_deref(),
                    Path::new(&row.path)
                        .file_stem()
                        .and_then(|value| value.to_str()),
                )
            }),
    )
}

fn allocate_reference_id<'a>(
    stem: &str,
    other_rows: impl IntoIterator<Item = (Option<&'a str>, Option<&'a str>)>,
) -> String {
    let other_rows = other_rows.into_iter().collect::<Vec<_>>();
    let has_id = |candidate: &str| {
        other_rows.iter().any(|(id, _)| {
            id.is_some_and(|id| id.eq_ignore_ascii_case(candidate))
        })
    };
    if !has_id(stem) {
        return stem.to_string();
    }
    for suffix in 2_u64.. {
        let candidate = format!("{stem}_{suffix}");
        let occupied = other_rows.iter().any(|(id, other_stem)| {
            id.is_some_and(|id| id.eq_ignore_ascii_case(&candidate))
                || other_stem
                    .is_some_and(|other| other.eq_ignore_ascii_case(&candidate))
        });
        if !occupied {
            return candidate;
        }
    }
    unreachable!("u64 reference ID suffix space exhausted")
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

    let planned_id = row
        .marker_id
        .as_ref()
        .map(|_| allocate_row_id(&index.rows, resolved.row, &name));
    let id = IdInfo {
        from: row.marker_id.clone(),
        changed: row.marker_id != planned_id,
        to: planned_id,
    };

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
        if !id.changed {
            return Ok(unchanged_plan(row, show_row, bundle, id, dry_run));
        }
        return id_update_plan(
            config, row, show_row, bundle, id, dry_run, offline,
        );
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
    let (mut rewritten_note, mut frontmatter) =
        rewrite_own_frontmatter(note_input, &bundle.frontmatter_map)
            .unwrap_or_else(|| (note_input.to_string(), Vec::new()));
    if id.changed {
        let Some((with_id, _)) =
            rewrite_reference_id_frontmatter(&rewritten_note, &id)
        else {
            return Err(RenameFailure::new(
                "not_in_sync",
                format!(
                    "could not locate the reference id in {}",
                    bundle.note_from.display()
                ),
                Some("run bob ref scan and retry"),
            ));
        };
        rewritten_note = with_id;
        frontmatter.push("id".to_string());
    }
    let own_note_rewrite = if rewritten_note != note_input {
        if let Some(index) = linked_note {
            link_plan.files[index].contents = rewritten_note;
            None
        } else {
            Some((bundle.note_from.clone(), rewritten_note))
        }
    } else {
        None
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
    let dirty_paths = dirty_rename_paths(&config.bob_dir, &touched_paths)?;
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
        id,
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

fn id_update_plan(
    config: &Config,
    row: &RefRow,
    show_row: ShowRow,
    bundle: Bundle,
    id: IdInfo,
    dry_run: bool,
    offline: bool,
) -> std::result::Result<PreparedPlan, RenameFailure> {
    if !is_in_sync(config, &bundle.pdf_from) {
        return Err(RenameFailure::new(
            "not_in_sync",
            format!(
                "the reference note and PDF are not in sync: {}",
                bundle.note_from.display()
            ),
            Some("run bob ref scan, then retry the id repair"),
        ));
    }
    let note_path = config.bob_dir.join(&bundle.note_from);
    let note_contents = fs::read_to_string(&note_path).map_err(|error| {
        RenameFailure::new(
            "not_in_sync",
            format!("could not read {}: {error}", bundle.note_from.display()),
            Some("run bob ref scan and retry"),
        )
    })?;
    let (rewritten, _) = rewrite_reference_id_frontmatter(&note_contents, &id)
        .ok_or_else(|| {
            RenameFailure::new(
                "not_in_sync",
                format!(
                    "could not locate the reference id in {}",
                    bundle.note_from.display()
                ),
                Some("run bob ref scan and retry"),
            )
        })?;

    let touched_paths =
        BTreeSet::from([bundle.note_from.clone(), bundle.pdf_from.clone()]);
    let dirty_paths = dirty_rename_paths(&config.bob_dir, &touched_paths)?;
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
    let report = RenameEnvelope {
        ok: true,
        schema_version: crate::native::ref_library::output::REF_SCHEMA_VERSION,
        command: RENAME_COMMAND,
        generated_at: crate::native::ref_library::output::generated_at(),
        mode: if dry_run { "dry_run" } else { "write" },
        changed: true,
        outcome: "id_updated",
        from,
        to,
        id,
        moves: Vec::new(),
        links: LinksInfo {
            rewritten: 0,
            files: Vec::new(),
            skipped_ambiguous: Vec::new(),
        },
        frontmatter: vec!["id".to_string()],
        kept: KeptInfo {
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
        warnings,
    };
    Ok(PreparedPlan {
        report,
        link_rewrites: Vec::new(),
        own_note_rewrite: Some((bundle.note_from.clone(), rewritten)),
        touched_paths,
    })
}

fn unchanged_plan(
    row: &RefRow,
    show_row: ShowRow,
    bundle: Bundle,
    id: IdInfo,
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
            id,
            moves: Vec::new(),
            links: LinksInfo {
                rewritten: 0,
                files: Vec::new(),
                skipped_ambiguous: Vec::new(),
            },
            frontmatter: Vec::new(),
            kept: KeptInfo {
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

fn rewrite_reference_id_frontmatter(
    contents: &str,
    id: &IdInfo,
) -> Option<(String, Vec<String>)> {
    let value = id.to.as_deref()?;
    id.from.as_ref()?;
    let mut lines = contents.split_inclusive('\n');
    let first = lines.next()?;
    if !matches!(first.trim(), "---") {
        return None;
    }
    let mut output = String::with_capacity(contents.len());
    output.push_str(first);
    let mut replaced = false;
    let mut ended = false;
    for line in lines.by_ref() {
        let content = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .strip_suffix('\r')
            .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line));
        if matches!(content.trim(), "---" | "...") {
            output.push_str(line);
            ended = true;
            break;
        }
        let is_id = !content.chars().next().is_some_and(char::is_whitespace)
            && content
                .split_once(':')
                .is_some_and(|(key, _)| key.trim() == "id");
        if !replaced && is_id {
            let line_ending = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            output.push_str("id: ");
            output.push_str(&super::super::render_frontmatter_string(value));
            output.push_str(line_ending);
            replaced = true;
        } else {
            output.push_str(line);
        }
    }
    if !ended || !replaced {
        return None;
    }
    for line in lines {
        output.push_str(line);
    }
    Some((output, vec!["id".to_string()]))
}

fn dirty_rename_paths(
    vault: &Path,
    touched_paths: &BTreeSet<PathBuf>,
) -> std::result::Result<Vec<PathBuf>, RenameFailure> {
    dirty_touched_paths(vault, touched_paths, false).map_err(
        |error| match error {
            DirtyPathError::GitUnavailable(error) => RenameFailure::new(
                "not_in_sync",
                format!("could not inspect Git status: {error}"),
                Some("check Git availability and retry"),
            ),
            DirtyPathError::StatusFailed => RenameFailure::new(
                "not_in_sync",
                "could not inspect Git status for the rename plan",
                Some("check that the Bob vault is a Git worktree and retry"),
            ),
        },
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_id_allocator_uses_ids_case_insensitively() {
        assert_eq!(
            allocate_reference_id("Target", [(Some("tArGeT"), Some("other"))]),
            "Target_2"
        );
    }

    #[test]
    fn reference_id_allocator_skips_suffixed_ids_and_stems() {
        assert_eq!(
            allocate_reference_id(
                "target",
                [
                    (Some("target"), Some("other")),
                    (Some("occupied"), Some("target_2")),
                    (Some("target_4"), Some("four")),
                ],
            ),
            "target_3"
        );
    }

    #[test]
    fn same_stem_rows_allocate_the_suffix_only_for_the_repaired_duplicate() {
        assert_eq!(
            allocate_reference_id("foo", [(Some("other"), Some("foo"))]),
            "foo"
        );
        assert_eq!(
            allocate_reference_id("foo", [(Some("foo"), Some("foo"))]),
            "foo_2"
        );
    }

    #[test]
    fn reference_id_allocator_keeps_an_already_allocated_suffix() {
        assert_eq!(
            allocate_reference_id("foo_2", [(Some("foo"), Some("foo"))]),
            "foo_2"
        );
    }

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
    fn reference_id_frontmatter_rewrite_replaces_the_whole_value_and_quotes_numbers(
    ) {
        let input = "---\ntitle: A\nid: old-id # comment\nsource_url: https://example.test\n---\nbody\n";
        let id = IdInfo {
            from: Some("old-id".to_string()),
            to: Some("2024".to_string()),
            changed: true,
        };
        let (actual, fields) =
            rewrite_reference_id_frontmatter(input, &id).expect("rewrite id");
        assert_eq!(
            actual,
            "---\ntitle: A\nid: \"2024\"\nsource_url: https://example.test\n---\nbody\n"
        );
        assert_eq!(fields, ["id"]);
    }

    #[cfg(test)]
    mod coordinated {
        use super::super::super::super::marker_write::{
            FakeCoordinator, FakeMoveEvent, MarkerWriteCoordinator,
            MarkerWriteSession,
        };
        use super::super::super::super::pdf_write_policy::{
            PdfWritePolicy, ReadingHoldMode,
        };
        use super::super::*;
        use crate::native::config::HighlightsConfig;
        use std::process::Command;
        use std::time::Duration;

        fn test_config_for(vault: &Path) -> Config {
            Config {
                bob_dir: vault.to_path_buf(),
                lib_dir: vault.join("lib"),
                ref_dir: vault.join("ref"),
                xlib_dir: vault.join("xlib"),
            }
        }

        fn write_area(vault: &Path) {
            let path = vault.join("sase.md");
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, "---\ntype: \"[[area]]\"\n---\n\n# SASE\n")
                .unwrap();
        }

        fn write_pdf_with_marker(path: &Path, marker: &str, pad: bool) {
            use lopdf::{dictionary, Document, Object, Stream};
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut doc = Document::with_version("1.4");
            if pad {
                let _ = doc.add_object(dictionary! { "Dummy" => "pad" });
            }
            let pages_id = doc.new_object_id();
            let content_id =
                doc.add_object(Stream::new(dictionary! {}, Vec::new()));
            let annot_id = doc.add_object(dictionary! {
                "Type" => "Annot",
                "Subtype" => "Text",
                "Rect" => vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(24),
                    Object::Integer(24),
                ],
                "Contents" => super::super::pdf_text_string(marker),
            });
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(612),
                    Object::Integer(792),
                ],
                "Contents" => content_id,
                "Annots" => vec![Object::Reference(annot_id)],
            });
            doc.set_object(
                pages_id,
                dictionary! {
                    "Type" => "Pages",
                    "Kids" => vec![Object::Reference(page_id)],
                    "Count" => 1,
                },
            );
            let catalog_id = doc.add_object(dictionary! {
                "Type" => "Catalog",
                "Pages" => pages_id,
            });
            doc.trailer.set("Root", catalog_id);
            doc.save(path).unwrap();
        }

        fn git_init_commit(vault: &Path) {
            let run = |args: &[&str]| {
                let status = Command::new("git")
                    .arg("-C")
                    .arg(vault)
                    .args(args)
                    .status()
                    .expect("run git");
                assert!(status.success(), "git {args:?} failed");
            };
            run(&["init", "-q"]);
            run(&["config", "user.name", "Test User"]);
            run(&["config", "user.email", "test@example.com"]);
            run(&["config", "commit.gpgsign", "false"]);
            run(&["add", "-A"]);
            run(&["commit", "-q", "-m", "fixture"]);
        }

        fn git_head(vault: &Path) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(vault)
                .args(["rev-parse", "HEAD"])
                .output()
                .expect("git rev-parse");
            assert!(output.status.success());
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        fn git_status(vault: &Path) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(vault)
                .args(["status", "--porcelain=v1", "--untracked-files=all"])
                .output()
                .expect("git status");
            String::from_utf8_lossy(&output.stdout).to_string()
        }

        fn synced_fixture(
            stem: &str,
            marker: &str,
        ) -> (tempfile::TempDir, Config, PathBuf) {
            let temp = tempfile::tempdir().expect("create scratch vault");
            let vault = temp.path().join("vault");
            for dir in ["lib/blogs", "ref/blogs", "xlib", "done", "daily"] {
                fs::create_dir_all(vault.join(dir)).unwrap();
            }
            write_area(&vault);
            let pdf = vault.join(format!("lib/blogs/{stem}.pdf"));
            write_pdf_with_marker(&pdf, marker, false);
            let config = test_config_for(&vault);
            let plan = plan_single_pdf_sync(
                &config,
                &pdf,
                SyncOptions {
                    dry_run: false,
                    write_pdf: false,
                    prefer: None,
                    ignore_unannotated_sidecar: false,
                },
            )
            .expect("plan initial sync");
            let outcome = write_single_pdf_sync(&config, &plan)
                .expect("write initial sync");
            assert!(
                matches!(outcome, PdfSyncOutcome::Written(_)),
                "initial sync should write the note: {outcome:?}"
            );
            git_init_commit(&vault);
            (temp, config, vault)
        }

        fn fake_session(fake: FakeCoordinator) -> MarkerWriteSession {
            let mut policy =
                PdfWritePolicy::from_config(&HighlightsConfig::default());
            policy.coordinate_pdf_writes = true;
            MarkerWriteSession::new(
                policy.with_reading_hold(ReadingHoldMode::Exempt),
                MarkerWriteCoordinator::Fake(fake),
            )
        }

        fn prepare_rename(
            config: &Config,
            query: &str,
            new: &str,
        ) -> PreparedPlan {
            prepare(config, query, new, false, true).expect("prepare rename")
        }

        fn execute_with_fake(
            config: &Config,
            prepared: &PreparedPlan,
            fake: FakeCoordinator,
        ) -> (std::result::Result<String, RenameFailure>, FakeCoordinator)
        {
            let observed = fake.clone();
            let child_env = crate::native::ob::child_env();
            let old_head = git_head(&config.bob_dir).to_string();
            let prior_row = prepared.report.reference.row.clone();
            let session = fake_session(fake);
            let result = execute_rename_with_session(
                config,
                prepared,
                &child_env,
                Some(old_head.as_str()),
                &prior_row,
                &session,
            );
            (result, observed)
        }

        const FOO_MARKER: &str =
            "- status: wip\n- parent: sase\n- title: Foo\n- id: foo\n";
        const BAR_MARKER: &str =
            "- status: wip\n- parent: sase\n- title: Foo\n- id: bar\n";

        #[test]
        fn move_grants_once_and_notifies_both_directions_on_rollback() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let prepared = prepare_rename(&config, "foo", "bar");
            assert!(!prepared.report.moves.is_empty());
            let fake = FakeCoordinator::grant(Duration::from_millis(7));
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            let sha = result.expect("rename should succeed");
            assert_eq!(observed.call_count(), 1, "one outer grant");
            let events = observed.move_events();
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    FakeMoveEvent::Will(from, to)
                    if from.ends_with("lib/blogs/foo.pdf")
                        && to.ends_with("lib/blogs/bar.pdf")
                )),
                "forward will_move: {events:?}"
            );
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    FakeMoveEvent::Did(from, to)
                    if from.ends_with("lib/blogs/foo.pdf")
                        && to.ends_with("lib/blogs/bar.pdf")
                )),
                "forward did_move: {events:?}"
            );
            assert!(vault.join("lib/blogs/bar.pdf").exists());
            assert!(!vault.join("lib/blogs/foo.pdf").exists());
            assert_eq!(git_head(&vault), sha);
            assert_eq!(git_status(&vault), "");
        }

        #[test]
        fn hold_before_writes_leaves_everything_untouched() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let before_pdf = fs::read(vault.join("lib/blogs/foo.pdf")).unwrap();
            let before_note =
                fs::read_to_string(vault.join("ref/blogs/foo.md")).unwrap();
            let head = git_head(&vault);
            let prepared = prepare_rename(&config, "foo", "bar");
            let fake = FakeCoordinator::hold(Duration::from_secs(2));
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            let failure = result.expect_err("held rename");
            assert_eq!(failure.code, "coordination_timeout");
            assert!(
                failure.message.contains("no rename was written"),
                "{}",
                failure.message
            );
            assert!(
                failure.hint.as_deref().unwrap().contains("retry"),
                "{:?}",
                failure.hint
            );
            assert_eq!(observed.call_count(), 1);
            assert_eq!(
                fs::read(vault.join("lib/blogs/foo.pdf")).unwrap(),
                before_pdf
            );
            assert_eq!(
                fs::read_to_string(vault.join("ref/blogs/foo.md")).unwrap(),
                before_note
            );
            assert!(!vault.join("lib/blogs/bar.pdf").exists());
            assert_eq!(git_head(&vault), head);
            assert_eq!(git_status(&vault), "");
        }

        #[test]
        fn platform_failure_before_writes_is_distinct_and_writes_nothing() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let head = git_head(&vault);
            let prepared = prepare_rename(&config, "foo", "bar");
            let fake = FakeCoordinator::fail("injected platform failure");
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            let failure = result.expect_err("failed coordination");
            assert_eq!(failure.code, "coordination_failed");
            assert_eq!(observed.call_count(), 1);
            assert!(vault.join("lib/blogs/foo.pdf").exists());
            assert!(!vault.join("lib/blogs/bar.pdf").exists());
            assert_eq!(git_head(&vault), head);
        }

        #[test]
        fn saved_first_full_rename_holds_conservatively() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let head = git_head(&vault);
            let prepared = prepare_rename(&config, "foo", "bar");
            let saved_path = vault.join("lib/blogs/foo.pdf");
            let _original = fs::read(&saved_path).unwrap();
            // Same marker, different bytes: simulate a presenter save that
            // only adds an unsaved annotation container.
            let pad_pdf = _temp.path().join("saved.pdf");
            write_pdf_with_marker(&pad_pdf, FOO_MARKER, true);
            let saved = fs::read(&pad_pdf).unwrap();
            assert_ne!(
                saved,
                fs::read(&saved_path).unwrap(),
                "saved bytes should differ while the marker stays"
            );
            let fake = FakeCoordinator::grant_after_save(
                Duration::from_millis(5),
                saved.clone(),
            );
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            let failure = result.expect_err("saved-first full rename holds");
            assert_eq!(failure.code, "preimage_changed");
            assert_eq!(observed.call_count(), 1);
            // The presenter's saved bytes survive with no bob writes.
            assert_eq!(fs::read(&saved_path).unwrap(), saved);
            assert!(!vault.join("lib/blogs/bar.pdf").exists());
            assert_eq!(git_head(&vault), head);
        }

        #[test]
        fn changed_marker_is_a_specific_hold() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let head = git_head(&vault);
            let prepared = prepare_rename(&config, "foo", "bar");
            let changed_path = _temp.path().join("changed.pdf");
            write_pdf_with_marker(&changed_path, BAR_MARKER, false);
            let changed = fs::read(&changed_path).unwrap();
            let fake =
                FakeCoordinator::grant_after_save(Duration::ZERO, changed);
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            let failure = result.expect_err("changed marker holds");
            assert_eq!(failure.code, "marker_changed");
            assert_eq!(observed.call_count(), 1);
            assert_eq!(git_head(&vault), head);
            assert!(!vault.join("lib/blogs/bar.pdf").exists());
        }

        #[test]
        fn same_stem_repair_keeps_saved_annotations_with_renumbered_objects() {
            let (_temp, config, vault) = synced_fixture(
                "foo",
                "- status: wip\n- parent: sase\n- title: Foo\n- id: stale-id\n",
            );
            let prepared = prepare_rename(&config, "foo", "foo");
            assert_eq!(prepared.report.outcome, "id_updated");
            // Saved bytes carry the same marker but renumbered objects.
            let renumbered_path = _temp.path().join("renumbered.pdf");
            write_pdf_with_marker(
                &renumbered_path,
                "- status: wip\n- parent: sase\n- title: Foo\n- id: stale-id\n",
                true,
            );
            let saved = fs::read(&renumbered_path).unwrap();
            let fake = FakeCoordinator::grant_after_save(
                Duration::from_millis(3),
                saved.clone(),
            );
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            let sha = result.expect("same-stem repair keeps the save");
            assert_eq!(observed.call_count(), 1);
            let marker =
                crate::native::highlights_ref::marker::read_pdf_marker(
                    &vault.join("lib/blogs/foo.pdf"),
                )
                .expect("read repaired marker");
            assert!(
                marker.contents.contains("- id: foo"),
                "{}",
                marker.contents
            );
            assert_eq!(git_head(&vault), sha);
        }

        #[test]
        fn idless_move_uses_one_grant_and_keeps_bytes() {
            let (_temp, config, vault) = synced_fixture(
                "foo",
                "- status: wip\n- parent: sase\n- title: Foo\n",
            );
            let original = fs::read(vault.join("lib/blogs/foo.pdf")).unwrap();
            let prepared = prepare_rename(&config, "foo", "bar");
            assert!(!prepared.report.id.changed);
            assert!(!prepared.report.moves.is_empty());
            let fake = FakeCoordinator::grant(Duration::ZERO);
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            result.expect("id-less move succeeds");
            assert_eq!(observed.call_count(), 1);
            assert_eq!(
                fs::read(vault.join("lib/blogs/bar.pdf")).unwrap(),
                original
            );
        }

        #[test]
        fn occupied_destination_holds_inside_the_grant() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let head = git_head(&vault);
            // Prepare while the destination is absent, then occupy to
            // simulate the race inside coordination.
            let prepared = prepare_rename(&config, "foo", "bar");
            // Occupy after prepare but before the coordinated accessor runs.
            // The snapshot inside execute sees the occupant and the accessor
            // must refuse without moving.
            fs::write(vault.join("lib/blogs/bar.pdf"), b"occupant").unwrap();
            let fake = FakeCoordinator::grant(Duration::ZERO);
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            let failure = result.expect_err("occupied destination holds");
            assert_eq!(failure.code, "preimage_changed");
            assert_eq!(observed.call_count(), 1);
            assert!(vault.join("lib/blogs/foo.pdf").exists());
            assert_eq!(
                fs::read(vault.join("lib/blogs/bar.pdf")).unwrap(),
                b"occupant"
            );
            assert_eq!(git_head(&vault), head);
        }

        #[test]
        fn verification_failure_reverses_forward_move_and_restores() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let head = git_head(&vault);
            let before_pdf = fs::read(vault.join("lib/blogs/foo.pdf")).unwrap();
            let prepared = prepare_rename(&config, "foo", "bar");
            test_set_verify_failure(true);
            let fake = FakeCoordinator::grant(Duration::ZERO);
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            test_set_verify_failure(false);
            let failure = result.expect_err("injected verification failure");
            assert_eq!(failure.code, "verify_failed");
            assert_eq!(observed.call_count(), 1);
            let events = observed.move_events();
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    FakeMoveEvent::Will(from, to)
                    if from.ends_with("lib/blogs/foo.pdf")
                        && to.ends_with("lib/blogs/bar.pdf")
                )),
                "forward notification: {events:?}"
            );
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    FakeMoveEvent::Did(from, to)
                    if from.ends_with("lib/blogs/bar.pdf")
                        && to.ends_with("lib/blogs/foo.pdf")
                )),
                "reverse notification: {events:?}"
            );
            assert_eq!(
                fs::read(vault.join("lib/blogs/foo.pdf")).unwrap(),
                before_pdf
            );
            assert!(!vault.join("lib/blogs/bar.pdf").exists());
            assert_eq!(git_head(&vault), head);
        }

        #[test]
        fn commit_failure_rolls_back_with_both_notifications() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let head = git_head(&vault);
            let before_pdf = fs::read(vault.join("lib/blogs/foo.pdf")).unwrap();
            let prepared = prepare_rename(&config, "foo", "bar");
            test_set_commit_failure(true);
            let fake = FakeCoordinator::grant(Duration::ZERO);
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            test_set_commit_failure(false);
            let failure = result.expect_err("injected commit failure");
            assert_eq!(failure.code, "verify_failed");
            assert_eq!(observed.call_count(), 1);
            let events = observed.move_events();
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    FakeMoveEvent::Did(from, to)
                    if from.ends_with("lib/blogs/foo.pdf")
                        && to.ends_with("lib/blogs/bar.pdf")
                )),
                "forward: {events:?}"
            );
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    FakeMoveEvent::Did(from, to)
                    if from.ends_with("lib/blogs/bar.pdf")
                        && to.ends_with("lib/blogs/foo.pdf")
                )),
                "reverse: {events:?}"
            );
            assert_eq!(
                fs::read(vault.join("lib/blogs/foo.pdf")).unwrap(),
                before_pdf
            );
            assert!(!vault.join("lib/blogs/bar.pdf").exists());
            assert_eq!(git_head(&vault), head);
        }

        #[test]
        fn saved_bytes_survive_a_rollback() {
            let (_temp, config, vault) = synced_fixture(
                "foo",
                "- status: wip\n- parent: sase\n- title: Foo\n- id: stale-id\n",
            );
            let prepared = prepare_rename(&config, "foo", "foo");
            let renumbered = _temp.path().join("saved.pdf");
            write_pdf_with_marker(
                &renumbered,
                "- status: wip\n- parent: sase\n- title: Foo\n- id: stale-id\n",
                true,
            );
            let saved = fs::read(&renumbered).unwrap();
            test_set_verify_failure(true);
            let fake = FakeCoordinator::grant_after_save(
                Duration::ZERO,
                saved.clone(),
            );
            let (result, _) = execute_with_fake(&config, &prepared, fake);
            test_set_verify_failure(false);
            assert_eq!(result.expect_err("verify fails").code, "verify_failed");
            // The presenter's saved bytes remain; bob never restores
            // pre-save bytes.
            assert_eq!(
                fs::read(vault.join("lib/blogs/foo.pdf")).unwrap(),
                saved
            );
        }

        #[test]
        fn commit_reported_as_error_stays_committed() {
            let (_temp, config, vault) = synced_fixture("foo", FOO_MARKER);
            let prepared = prepare_rename(&config, "foo", "bar");
            test_set_commit_reported_as_error(true);
            let fake = FakeCoordinator::grant(Duration::ZERO);
            let (result, observed) =
                execute_with_fake(&config, &prepared, fake);
            test_set_commit_reported_as_error(false);
            let sha = result.expect("reported-as-error stays committed");
            assert_eq!(observed.call_count(), 1);
            assert!(vault.join("lib/blogs/bar.pdf").exists());
            assert!(!vault.join("lib/blogs/foo.pdf").exists());
            assert_eq!(git_head(&vault), sha);
        }

        #[test]
        fn refusal_renders_in_human_and_json() {
            use crate::native::ref_library::output::Format as RefFormat;
            let timeout = rename_hold_failure(
                "coordination_timeout",
                "Highlights didn't answer; no rename was written".to_string(),
            );
            assert_eq!(timeout.code, "coordination_timeout");
            assert!(timeout.hint.as_deref().unwrap().contains("retry"));
            let marker = rename_hold_failure(
                "marker_changed",
                "marker changed; no files were written".to_string(),
            );
            assert_eq!(marker.code, "marker_changed");
            // Exercise both renderings; output goes to the test harness.
            assert_eq!(
                super::super::print_failure(RefFormat::Json, timeout.clone()),
                1
            );
            assert_eq!(
                super::super::print_failure(RefFormat::Human, marker.clone()),
                1
            );
            assert_eq!(super::super::print_failure(RefFormat::Json, marker), 1);
        }
    }
}
