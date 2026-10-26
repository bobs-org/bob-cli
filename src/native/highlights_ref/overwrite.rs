//! Read-only planning and inert prose archives for in-place overwrite.
//!
//! This module owns the prepare phase of overwrite. It resolves an existing
//! family member, proves the current bundle and reading task are safe to
//! replace, and renders any prose archive without writing it. The later
//! transaction consumes the fingerprints and prepared note bytes while it
//! holds the mutation locks.

pub(super) mod execute;
pub(super) mod local;
pub(crate) mod recovery;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde_yaml::{Mapping, Value as YamlValue};
use sha2::{Digest, Sha256};

use super::{
    audio, note, reading_plan, recapture, reference_mutation, sidecar,
    CommandError, Config,
};
use crate::native::{
    markdown, ob,
    ref_library::{
        build_index, lineage, LibraryConfig, ParsedFrontmatter, RefRow,
        RefTaskView,
    },
    ref_tasks::{self, RefTaskIndex},
};

/// User options that affect whether a filed member can be overwritten.
#[derive(Debug, Clone, Default)]
pub(super) struct OverwriteRequest {
    pub(super) name: Option<String>,
    pub(super) output: Option<PathBuf>,
    pub(super) keep_intake: bool,
    pub(super) explicit_ref_type: Option<String>,
    /// Canonical resolved route (without `.md`), not the user's spelling.
    pub(super) resolved_parent: Option<String>,
    /// Destination chosen before rendering for requested companion audio.
    /// Including it here makes its preimage part of the read-only plan.
    pub(super) replacement_audio_path: Option<String>,
    /// Status explicitly supplied by the user. `None` preserves the located
    /// task's checkbox, including blocked and terminal marks.
    pub(super) explicit_status: Option<String>,
    /// Optional explicit title. Otherwise the staged source title wins.
    pub(super) explicit_title: Option<String>,
}

/// A path and its observed preimage. `sha256: None` means it was absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FilePreimage {
    pub(super) path: String,
    pub(super) sha256: Option<String>,
}

/// The exact selected reading task and its subtree fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskFingerprint {
    pub(super) path: String,
    pub(super) block_id: Option<String>,
    pub(super) line_index: usize,
    pub(super) mark: char,
    pub(super) line: String,
    pub(super) subtree_sha256: String,
    pub(super) subtree: String,
    pub(super) file_sha256: String,
}

/// A prose-only archive prepared in memory. Its destination is included as
/// an absent preimage so the transaction can refuse a same-day race.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FrozenNote {
    pub(super) path: String,
    pub(super) contents: String,
}

/// All read-only evidence needed before overwrite can render new material.
#[derive(Debug, Clone)]
pub(super) struct OverwritePreflight {
    pub(super) member: recapture::FamilyMember,
    pub(super) note_path: String,
    pub(super) pdf_path: String,
    pub(super) task: TaskFingerprint,
    pub(super) task_view: RefTaskView,
    pub(super) parent: Option<String>,
    pub(super) bundle_preimages: Vec<FilePreimage>,
    pub(super) bundle_sha256: String,
    pub(super) frozen_note: Option<FrozenNote>,
    pub(super) old_skeleton: String,
    pub(super) authored_prose: bool,
    pub(super) lineage: Vec<lineage::EarlierEntry>,
    pub(super) status: String,
    pub(super) created: Option<YamlValue>,
    pub(super) captured: Option<String>,
    pub(super) title: String,
    pub(super) ref_type: String,
    pub(super) audio_path: Option<String>,
    pub(super) planned_date: String,
}

/// A typed refusal so route renderers can preserve the established JSON code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverwriteRefusal {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) hint: Option<String>,
    pub(super) exit_code: i32,
}

impl OverwriteRefusal {
    fn new(
        code: &'static str,
        message: impl Into<String>,
        hint: Option<impl Into<String>>,
        exit_code: i32,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.map(Into::into),
            exit_code,
        }
    }

    pub(super) fn command_error(&self) -> CommandError {
        let mut message = format!("{}: {}", self.code, self.message);
        if let Some(hint) = &self.hint {
            message.push_str(&format!("\nhint: {hint}"));
        }
        CommandError::new(message).with_exit_code(self.exit_code)
    }
}

/// Resolve URL identity and select the family member that overwrite targets.
pub(super) fn select_url_member(
    config: &Config,
    rows: &[RefRow],
    recorded: &[super::sources::RecordedSource],
    wanted: &BTreeSet<String>,
    name: Option<&str>,
) -> Result<Option<recapture::SelectedFamily>, CommandError> {
    recapture::select_overwrite_for_url(
        rows,
        recorded,
        wanted,
        &config.bob_dir,
        name,
    )
}

/// Resolve title identity and select a member, if this is a filed capture.
pub(super) fn select_stem_member(
    config: &Config,
    rows: &[RefRow],
    stem: &str,
    name: Option<&str>,
    identity: recapture::StemIdentity<'_>,
) -> Result<Option<recapture::SelectedFamily>, CommandError> {
    recapture::select_overwrite_for_stem(config, rows, stem, name, identity)
}

/// Strict, read-only preflight for one selected overwrite member.
pub(super) fn prepare(
    config: &Config,
    member: &recapture::FamilyMember,
    request: &OverwriteRequest,
    date: &str,
) -> Result<OverwritePreflight, OverwriteRefusal> {
    let index = build_index(&LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    })
    .map_err(|error| {
        OverwriteRefusal::new(
            "not_in_sync",
            format!("could not index the selected reference: {error}"),
            Some("run bob ref scan -w on the Mac first"),
            2,
        )
    })?;
    let row = index
        .rows
        .iter()
        .find(|row| row.path == member.path)
        .ok_or_else(|| {
            OverwriteRefusal::new(
                "not_in_sync",
                format!(
                    "selected reference is not in the live index: {}",
                    member.path
                ),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;

    let ref_type = row.ref_type.clone().unwrap_or_else(|| {
        Path::new(&row.path)
            .parent()
            .and_then(Path::file_name)
            .and_then(|part| part.to_str())
            .unwrap_or("papers")
            .to_string()
    });
    if reference_mutation::is_legacy(row, &config.ref_dir) {
        return Err(OverwriteRefusal::new(
            "legacy_reference",
            format!("{} is not a modern PDF-backed reference", row.path),
            Some("capture the source as a new reference"),
            2,
        ));
    }
    let source_pdf = row.source_pdf.as_deref().ok_or_else(|| {
        OverwriteRefusal::new(
            "legacy_reference",
            format!("{} has no source PDF", row.path),
            Some("capture the source as a new reference"),
            2,
        )
    })?;
    let pdf_relative = PathBuf::from(source_pdf);
    if let Err(error) = checked_vault_path(config, &pdf_relative) {
        return Err(OverwriteRefusal::new(
            "not_in_sync",
            format!("invalid source PDF path {source_pdf}: {error}"),
            Some("run bob ref scan -w on the Mac first"),
            2,
        ));
    }
    let pdf_path = config.bob_dir.join(&pdf_relative);
    if reference_mutation::intake_query(config, source_pdf) {
        return Err(OverwriteRefusal::new(
            "intake_reference",
            format!("{} is an intake reference", row.path),
            Some("file the reference with bob ref scan before overwriting"),
            2,
        ));
    }
    let library_pdf = checked_vault_path(
        config,
        pdf_path
            .strip_prefix(&config.bob_dir)
            .expect("vault-relative source PDF"),
    )
    .ok()
    .is_some_and(|path| {
        let Ok(library) = config.lib_dir.canonicalize() else {
            return false;
        };
        path.starts_with(library)
    });
    if !library_pdf {
        return Err(OverwriteRefusal::new(
            "legacy_reference",
            format!("{} is not backed by a modern library PDF", row.path),
            Some("capture the source as a new reference"),
            2,
        ));
    }
    if request.output.is_some() {
        return Err(OverwriteRefusal::new(
            "output_not_supported",
            "-o cannot select a path for an in-place overwrite",
            Some("drop -o; overwrite keeps the filed reference path"),
            2,
        ));
    }
    if request.keep_intake {
        return Err(OverwriteRefusal::new(
            "intake_not_supported",
            "-k cannot queue an in-place overwrite",
            Some(
                "drop -k; overwrite files directly into the reference library",
            ),
            2,
        ));
    }
    if let Some(explicit_type) = request.explicit_ref_type.as_deref()
        && !explicit_type.eq_ignore_ascii_case(&ref_type)
    {
        return Err(OverwriteRefusal::new(
            "ref_type_mismatch",
            format!(
                "-t {explicit_type} does not match the filed type {ref_type}"
            ),
            Some(format!("use -t {ref_type} or drop -t")),
            2,
        ));
    }
    let task_view = row.task.as_ref().ok_or_else(|| task_refusal(row))?;
    if let Some(parent) = request.resolved_parent.as_deref() {
        let expected = route_for_task(task_view);
        let parent_norm = normalize_route(parent);
        let task_match = parent_norm == normalize_route(&expected);
        let note_parent_match =
            row.parent.as_deref().is_some_and(|note_parent| {
                parent_norm == normalize_route(note_parent)
            });
        if !task_match && !note_parent_match {
            return Err(OverwriteRefusal::new(
                "parent_mismatch",
                format!("-P resolves to {parent}, but the reading task lives in {expected}"),
                Some(format!("move the reading task with bob task move: {}", task_view.link)),
                2,
            ));
        }
    }
    let child_env = ob::child_env();
    if !reference_mutation::vault_is_git_worktree(&config.bob_dir, &child_env) {
        return Err(OverwriteRefusal::new(
            "not_a_worktree",
            "in-place overwrite requires the Bob vault to be a Git worktree",
            Some("open or initialize the vault as a Git worktree"),
            2,
        ));
    }
    if !reference_mutation::is_in_sync(config, &pdf_relative) {
        return Err(OverwriteRefusal::new(
            "not_in_sync",
            format!(
                "the reference note, PDF, or reading task is not in sync: {}",
                row.path
            ),
            Some("run bob ref scan -w on the Mac first"),
            2,
        ));
    }

    let note_absolute = checked_vault_path(config, Path::new(&row.path))
        .map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                format!("invalid reference note path {}: {error}", row.path),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
    let note_contents =
        fs::read_to_string(&note_absolute).map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                format!("could not read {}: {error}", row.path),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
    let (front, body) =
        super::split_frontmatter(&note_contents).ok_or_else(|| {
            OverwriteRefusal::new(
                "not_in_sync",
                format!("{} has no readable frontmatter", row.path),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
    let note_parsed = super::parse_note(&note_contents);
    let task = locate_task_fingerprint(
        config,
        row,
        task_view,
        &body,
        &index.ref_tasks,
    )?;
    let open_tasks = open_note_tasks(&body, Some(&task));
    if !open_tasks.is_empty() {
        return Err(OverwriteRefusal::new(
            "open_note_tasks",
            format!(
                "{} has open checkbox tasks outside its reading task: {}",
                row.path,
                open_tasks.join(", ")
            ),
            Some("close or move those tasks before overwriting the reference"),
            2,
        ));
    }

    let stem = Path::new(&row.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let assets_path = note_absolute
        .parent()
        .unwrap_or(&config.ref_dir)
        .join(format!("{stem}.assets"));
    let sidecar_path =
        sidecar::discover_sidecar_path(&pdf_path).map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                error.to_string(),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
    if let Some(path) = &sidecar_path {
        let relative = path.strip_prefix(&config.bob_dir).map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                format!("sidecar path is outside the vault: {error}"),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
        checked_vault_path(config, relative).map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                format!("invalid sidecar path {}: {error}", path.display()),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
    }
    let sidecar =
        sidecar::read_sidecar_for_pdf(&pdf_path).map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                error.to_string(),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
    let sidecar_has_annotations = sidecar.as_ref().is_some_and(|sidecar| {
        sidecar
            .annotations
            .iter()
            .any(|annotation| !super::is_sidecar_marker_mirror(annotation))
    });
    let note_has_annotations =
        reference_mutation::has_managed_highlights(&note_contents);
    let pdf_has_annotations =
        has_user_pdf_annotations(&pdf_path).map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                format!("could not inspect PDF annotations: {error}"),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
    let links =
        BTreeMap::from([(PathBuf::from(&row.path), PathBuf::from(&row.path))]);
    let inbound =
        super::rename::links::plan_vault_links(&config.bob_dir, &links)
            .map_err(|error| {
                OverwriteRefusal::new(
            "not_in_sync",
            format!("could not inspect inbound highlight links: {error}"),
            Some("check vault Markdown readability and run bob ref scan -w"),
            2,
        )
            })?;
    if note_has_annotations
        || sidecar_has_annotations
        || pdf_has_annotations
        || assets_path.exists()
        || !inbound.highlight_fragments.is_empty()
        || !inbound.highlight_follow_ups.is_empty()
    {
        return Err(annotated_refusal(
            row,
            &assets_path,
            note_has_annotations,
            sidecar_has_annotations,
            pdf_has_annotations,
            &inbound,
        ));
    }

    let mut touched = BTreeSet::from([
        PathBuf::from(&row.path),
        pdf_relative.clone(),
        PathBuf::from(&task.path),
        assets_path
            .strip_prefix(&config.bob_dir)
            .unwrap_or(&assets_path)
            .to_path_buf(),
    ]);
    if let Some(sidecar_path) = sidecar::discover_sidecar_path(&pdf_path)
        .map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                error.to_string(),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?
    {
        if let Ok(relative) = sidecar_path.strip_prefix(&config.bob_dir) {
            touched.insert(relative.to_path_buf());
        }
        if let Some(bundle_root) = textbundle_root(&sidecar_path)
            && let Ok(relative) = bundle_root.strip_prefix(&config.bob_dir)
        {
            touched.insert(relative.to_path_buf());
        }
    }
    let audio_path = audio::discover_companion_audio(config, &pdf_path);
    if let Some(audio) = &audio_path
        && let Ok(relative) = audio.strip_prefix(&config.bob_dir)
    {
        touched.insert(relative.to_path_buf());
    }
    if let Some(replacement_audio) = &request.replacement_audio_path {
        let relative = PathBuf::from(replacement_audio);
        if let Err(error) = checked_vault_path(config, &relative) {
            return Err(OverwriteRefusal::new(
                "not_in_sync",
                format!("invalid replacement audio path {replacement_audio}: {error}"),
                Some("run bob ref scan -w on the Mac first"),
                2,
            ));
        }
        touched.insert(relative);
    }
    let projection = note_parsed
        .synced_projection_with_normalization()
        .map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                error.to_string(),
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?
        .projection;
    let status = task_status(task_view.mark).unwrap_or_else(|| {
        row.status.clone().unwrap_or_else(|| "legacy".to_string())
    });
    let title = projection
        .get("title")
        .and_then(super::MarkerValue::as_string)
        .unwrap_or(&row.title)
        .to_string();
    let fresh_body = render_fresh_skeleton(
        config,
        &row.path,
        &pdf_path,
        &pdf_relative,
        &projection,
        task_view,
        &title,
        audio_path.as_deref(),
    );
    let authored_prose = body_has_authored_prose(
        &body,
        &fresh_body,
        audio_frontmatter_path(&note_parsed),
    )
    .map_err(|error| {
        OverwriteRefusal::new(
            "not_in_sync",
            error.to_string(),
            Some("run bob ref scan -w on the Mac first"),
            2,
        )
    })?;
    let existing_lineage =
        lineage::parse_earlier(&ParsedFrontmatter::parse(&front));
    let frozen_note = if authored_prose {
        let path = allocate_archive_path(config, &ref_type, stem, date);
        let archive_contents = render_frozen_note(
            &note_contents,
            &row.path,
            &ref_type,
            stem,
            date,
            row.captured.as_deref(),
            &status,
            &title,
        )
        .map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                error,
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
        touched.insert(PathBuf::from(&path));
        Some(FrozenNote {
            path,
            contents: archive_contents,
        })
    } else {
        None
    };
    let dirty = reference_mutation::dirty_touched_paths(
        &config.bob_dir,
        &touched,
        true,
    )
    .map_err(|error| {
        OverwriteRefusal::new(
            "dirty_paths",
            format!(
                "could not inspect dirty reference paths: {}",
                dirty_error(error)
            ),
            Some("commit or revert the touched changes before overwriting"),
            2,
        )
    })?;
    if !dirty.is_empty() {
        return Err(OverwriteRefusal::new(
            "dirty_paths",
            format!(
                "overwrite touches dirty vault paths: {}",
                reference_mutation::display_paths(&dirty)
            ),
            Some("commit or revert the touched changes before overwriting"),
            2,
        ));
    }
    let mut bundle_preimages =
        collect_preimages(config, &touched).map_err(|error| {
            OverwriteRefusal::new(
                "not_in_sync",
                error,
                Some("run bob ref scan -w on the Mac first"),
                2,
            )
        })?;
    if let Some(archive) = &frozen_note {
        bundle_preimages.push(FilePreimage {
            path: archive.path.clone(),
            sha256: None,
        });
    }
    bundle_preimages.sort_by(|left, right| left.path.cmp(&right.path));
    bundle_preimages.dedup_by(|left, right| left.path == right.path);
    let bundle_sha256 = fingerprint_preimages(&bundle_preimages);
    let created = yaml_frontmatter_value(&front, "created");
    Ok(OverwritePreflight {
        member: member.clone(),
        note_path: row.path.clone(),
        pdf_path: reference_mutation::vault_path(&pdf_relative),
        task,
        task_view: task_view.clone(),
        parent: row.parent.clone(),
        bundle_preimages,
        bundle_sha256,
        frozen_note,
        old_skeleton: fresh_body,
        authored_prose,
        lineage: existing_lineage,
        status,
        created,
        captured: row.captured.clone(),
        title,
        ref_type,
        audio_path: audio_path.map(|path| {
            reference_mutation::vault_path(
                path.strip_prefix(&config.bob_dir).unwrap_or(&path),
            )
        }),
        planned_date: date.to_string(),
    })
}

fn task_refusal(row: &RefRow) -> OverwriteRefusal {
    let diagnostic = row.diagnostics.iter().find(|diagnostic| {
        matches!(
            diagnostic.code.as_str(),
            "multiple_open_ref_tasks" | "open_ref_without_task"
        )
    });
    let code = diagnostic.map_or("open_ref_without_task", |diagnostic| {
        if diagnostic.code == "multiple_open_ref_tasks" {
            "multiple_open_ref_tasks"
        } else {
            "open_ref_without_task"
        }
    });
    OverwriteRefusal::new(
        code,
        diagnostic
            .map(|diagnostic| diagnostic.detail.clone())
            .unwrap_or_else(|| {
                format!("{} has no uniquely located reading task", row.path)
            }),
        Some("restore or resolve the existing reading task before overwriting"),
        2,
    )
}

fn route_for_task(task: &RefTaskView) -> String {
    task.path
        .strip_suffix(".md")
        .or_else(|| task.path.strip_suffix(".MD"))
        .unwrap_or(&task.path)
        .to_string()
}

fn checked_vault_path(
    config: &Config,
    relative: &Path,
) -> Result<PathBuf, String> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("path must be a clean vault-relative path".to_string());
    }
    let root = config
        .bob_dir
        .canonicalize()
        .map_err(|error| format!("resolve vault root: {error}"))?;
    let mut current = config.bob_dir.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            unreachable!("path components were checked above")
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!("inspect {}: {error}", current.display())
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "symlink path component: {}",
                current.display()
            ));
        }
    }
    let resolved = current
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", current.display()))?;
    if !resolved.starts_with(&root) {
        return Err("path resolves outside the vault".to_string());
    }
    Ok(resolved)
}

fn normalize_route(route: &str) -> String {
    let route = route.trim();
    let route = route
        .strip_prefix("[[")
        .and_then(|route| route.strip_suffix("]]"))
        .unwrap_or(route);
    route
        .strip_suffix(".md")
        .or_else(|| route.strip_suffix(".MD"))
        .unwrap_or(route)
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

fn locate_task_fingerprint(
    config: &Config,
    row: &RefRow,
    task: &RefTaskView,
    ref_body: &str,
    index: &RefTaskIndex,
) -> Result<TaskFingerprint, OverwriteRefusal> {
    let absolute =
        checked_vault_path(config, Path::new(&task.path)).map_err(|error| {
            OverwriteRefusal::new(
                "open_ref_without_task",
                format!("invalid reading task path {}: {error}", task.path),
                Some("restore the existing reading task before overwriting"),
                2,
            )
        })?;
    let bytes = fs::read(&absolute).map_err(|error| {
        OverwriteRefusal::new(
            "open_ref_without_task",
            format!("could not read reading task {}: {error}", task.path),
            Some("restore the existing reading task before overwriting"),
            2,
        )
    })?;
    let contents = String::from_utf8(bytes.clone()).map_err(|error| {
        OverwriteRefusal::new(
            "open_ref_without_task",
            format!("reading task file {} is not UTF-8: {error}", task.path),
            Some("restore the existing reading task before overwriting"),
            2,
        )
    })?;
    let row_path = row.path.as_str();
    let line_index = if task.path == row_path {
        let hits = ref_tasks::find_trackers(ref_body);
        if hits.len() != 1 {
            return Err(task_refusal(row));
        }
        contents
            .lines()
            .enumerate()
            .find(|(_, line)| line.trim() == hits[0].line)
            .map(|(index, _)| index)
    } else {
        let candidates = index.candidates(row_path);
        let hits = candidates
            .iter()
            .filter(|candidate| {
                candidate.path == task.path
                    && candidate.block_id == task.block_id
            })
            .collect::<Vec<_>>();
        (hits.len() == 1).then(|| hits[0].line_index)
    }
    .ok_or_else(|| task_refusal(row))?;
    let lines = contents.lines().collect::<Vec<_>>();
    let line = lines
        .get(line_index)
        .ok_or_else(|| task_refusal(row))?
        .to_string();
    let mark = ref_tasks::task_mark(&ref_tasks::strip_blockquote_prefix(&line))
        .ok_or_else(|| task_refusal(row))?;
    let end = task_subtree_end(&lines, line_index);
    let segments = contents.split_inclusive('\n').collect::<Vec<_>>();
    let subtree = segments[line_index..end].concat();
    Ok(TaskFingerprint {
        path: task.path.clone(),
        block_id: task.block_id.clone(),
        line_index,
        mark,
        line,
        subtree_sha256: sha256(subtree.as_bytes()),
        subtree,
        file_sha256: sha256(&bytes),
    })
}

fn task_subtree_end(lines: &[&str], start: usize) -> usize {
    let indent = lines[start]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .count();
    let mut end = start + 1;
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            end = index + 1;
            continue;
        }
        let child_indent = line
            .chars()
            .take_while(|character| matches!(character, ' ' | '\t'))
            .count();
        if child_indent <= indent {
            break;
        }
        end = index + 1;
    }
    end
}

fn open_note_tasks(
    body: &str,
    selected: Option<&TaskFingerprint>,
) -> Vec<String> {
    let lines = body.lines().collect::<Vec<_>>();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    // The comment scanner operates on the supplied lines without parsing
    // Markdown fences, so hide fenced lines before asking it for comment
    // spans. This prevents an example comment inside code from hiding real
    // prose after the fence.
    let comment_input = lines
        .iter()
        .enumerate()
        .map(
            |(index, line)| {
                if fenced.contains(&index) {
                    ""
                } else {
                    *line
                }
            },
        )
        .collect::<Vec<_>>();
    let comments = markdown::multiline_comment_lines(&comment_input);
    let managed_region = ref_tasks::managed_region_line_range(&lines);
    let selected_tracker =
        selected.filter(|task| task.block_id.as_deref() == Some("ref"));
    let mut open = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if fenced.contains(&index)
            || comments.contains(&index)
            || managed_region
                .is_some_and(|(begin, end)| (begin..=end).contains(&index))
        {
            continue;
        }
        let stripped = ref_tasks::strip_blockquote_prefix(line);
        if ref_tasks::is_embed_task_line(stripped)
            || ref_tasks::find_managed_embed(body)
                .is_some_and(|embed| embed.line_index == index)
        {
            continue;
        }
        let Some(mark) = ref_tasks::task_mark(stripped) else {
            continue;
        };
        if !ref_tasks::is_open_mark(mark) {
            continue;
        }
        if selected_tracker.is_some_and(|task| {
            task.line_index == index
                || ref_tasks::parse_tracker_line(line).is_some()
        }) {
            continue;
        }
        open.push(format!("{}:{}", index + 1, line.trim()));
    }
    open
}

pub(super) fn render_fresh_skeleton(
    config: &Config,
    note_rel: &str,
    pdf: &Path,
    pdf_relative: &Path,
    projection: &super::Projection,
    task: &RefTaskView,
    title: &str,
    audio_path: Option<&Path>,
) -> String {
    let audio_rel =
        audio_path.and_then(|path| path.strip_prefix(&config.bob_dir).ok());
    let audio_value = audio_rel.map(reference_mutation::vault_path);
    let has_external_task = task.path != note_rel;
    if has_external_task || task.block_id.as_deref() != Some("ref") {
        return reading_plan::v2_birth_body(
            title,
            &reading_plan::managed_embed_target(&ref_task_from_view(task)),
            task.block_id.as_deref().unwrap_or("ref-reading"),
            "",
            audio_value.as_deref(),
        );
    }
    note::default_note_body(
        pdf,
        projection,
        &reference_mutation::vault_path(pdf_relative),
        None,
        audio_value.as_deref(),
    )
}

fn ref_task_from_view(task: &RefTaskView) -> ref_tasks::LocatedRefTask {
    ref_tasks::LocatedRefTask {
        path: task.path.clone(),
        line_index: 0,
        line: String::new(),
        mark: task.mark,
        block_id: task.block_id.clone(),
        archived: task.archived,
        residence: None,
        closed_on: None,
        in_capture_target: true,
    }
}

fn body_has_authored_prose(
    body: &str,
    skeleton: &str,
    audio_path: Option<String>,
) -> Result<bool, CommandError> {
    let normalize = |body: &str| -> Result<String, CommandError> {
        let mut normalized = note::replace_managed_region(body, "")?;
        let mut lines =
            normalized.lines().map(str::to_string).collect::<Vec<_>>();
        let audio_line = audio_path
            .as_deref()
            .map(audio::audio_embed_line)
            .unwrap_or_default();
        for line in &mut lines {
            let trimmed = line.trim();
            if ref_tasks::parse_tracker_line(trimmed).is_some() {
                *line = "<managed-reading-tracker>".to_string();
            } else if !audio_line.is_empty() && trimmed == audio_line {
                *line = "<managed-audio>".to_string();
            }
        }
        if let Some(embed) = ref_tasks::find_managed_embed(&normalized)
            && let Some(line) = lines.get_mut(embed.line_index)
        {
            *line = "<managed-reading-task-embed>".to_string();
        }
        normalized = lines.join("\n");
        Ok(normalized)
    };
    Ok(normalize(body)? != normalize(skeleton)?)
}

/// Add one frozen-note link first in a live note's `earlier:` list while
/// carrying every existing entry forward in order.
pub(super) fn prepend_archive_lineage(
    contents: &str,
    archive_path: &str,
    replaced_date: &str,
    highlights: usize,
) -> Result<String, String> {
    let (mut front, body) =
        super::split_frontmatter(contents).ok_or_else(|| {
            "cannot add overwrite lineage without frontmatter".to_string()
        })?;
    let parsed = ParsedFrontmatter::parse(&front);
    let label = format!(
        "replaced {replaced_date} · {highlights} {}",
        if highlights == 1 {
            "highlight"
        } else {
            "highlights"
        }
    );
    let mut entries = vec![lineage::EarlierEntry {
        target: archive_path
            .strip_suffix(".md")
            .unwrap_or(archive_path)
            .to_string(),
        label: Some(label),
        kind: lineage::EarlierKind::Replaced,
    }];
    entries.extend(lineage::parse_earlier(&parsed));

    let earlier_index = front.iter().position(|line| {
        top_level_key(line)
            .is_some_and(|key| key.eq_ignore_ascii_case("earlier"))
    });
    if let Some(start) = earlier_index {
        let end = front[start + 1..]
            .iter()
            .position(|line| {
                !line.trim().is_empty() && !line.starts_with([' ', '\t'])
            })
            .map(|offset| start + 1 + offset)
            .unwrap_or(front.len());
        front.drain(start..end);
    }
    let rendered = render_earlier_entries(&entries);
    let insert_at = earlier_index.unwrap_or(front.len()).min(front.len());
    front.splice(insert_at..insert_at, rendered);
    Ok(format!("---\n{}\n---\n{body}", front.join("\n")))
}

fn top_level_key(line: &str) -> Option<&str> {
    if line.starts_with([' ', '\t']) {
        return None;
    }
    let (key, _) = line.split_once(':')?;
    Some(key.trim())
}

fn render_earlier_entries(entries: &[lineage::EarlierEntry]) -> Vec<String> {
    let mut lines = vec!["earlier:".to_string()];
    for entry in entries {
        let link = match &entry.label {
            Some(label) => format!("[[{}|{}]]", entry.target, label),
            None => format!("[[{}]]", entry.target),
        };
        lines.push(format!("  - {}", yaml_double_quote(&link)));
    }
    lines
}

fn yaml_double_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

fn textbundle_root(sidecar: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for component in sidecar.components() {
        root.push(component.as_os_str());
        if component
            .as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".textbundle")
        {
            return Some(root);
        }
    }
    None
}

fn audio_frontmatter_path(note: &super::ParsedNote) -> Option<String> {
    let value = note.frontmatter_value(super::FIELD_AUDIO)?;
    let raw = value.as_string()?;
    super::sidecar::simple_wikilink_target(raw).map(str::to_string)
}

fn render_frozen_note(
    old_contents: &str,
    live_note: &str,
    ref_type: &str,
    stem: &str,
    date: &str,
    captured: Option<&str>,
    status: &str,
    title: &str,
) -> Result<String, String> {
    let (front_lines, body) = super::split_frontmatter(old_contents)
        .ok_or_else(|| {
            "cannot freeze a reference without frontmatter".to_string()
        })?;
    let old_yaml = YamlValue::Mapping(
        serde_yaml::from_str::<YamlValue>(&front_lines.join("\n"))
            .map_err(|error| format!("parse reference frontmatter: {error}"))?
            .as_mapping()
            .cloned()
            .ok_or_else(|| {
                "reference frontmatter is not a YAML mapping".to_string()
            })?,
    );
    let old_map = old_yaml.as_mapping().expect("mapping was checked");
    let mut archive_front = Mapping::new();
    copy_yaml_key(old_map, &mut archive_front, "title");
    copy_yaml_key(old_map, &mut archive_front, "source_url");
    copy_yaml_key(old_map, &mut archive_front, "created");
    archive_front.insert(
        YamlValue::String("parent".to_string()),
        YamlValue::String(format!("[[ref/{ref_type}/{stem}]]")),
    );
    archive_front.insert(
        YamlValue::String("replaced".to_string()),
        YamlValue::String(date.to_string()),
    );
    archive_front.insert(
        YamlValue::String("status_when_replaced".to_string()),
        YamlValue::String(status.to_string()),
    );
    archive_front.insert(
        YamlValue::String("highlights_count".to_string()),
        YamlValue::Number(0.into()),
    );
    let yaml = serde_yaml::to_string(&YamlValue::Mapping(archive_front))
        .map_err(|error| format!("render archive frontmatter: {error}"))?;
    let body = frozen_body(
        &body,
        live_note,
        title,
        date,
        captured.unwrap_or("unknown"),
        status,
    );
    Ok(format!("---\n{yaml}---\n{body}"))
}

fn copy_yaml_key(source: &Mapping, target: &mut Mapping, key: &str) {
    let key_value = YamlValue::String(key.to_string());
    if let Some(value) = source.get(&key_value) {
        target.insert(key_value, value.clone());
    }
}

fn frozen_body(
    body: &str,
    live_note: &str,
    title: &str,
    date: &str,
    captured: &str,
    status: &str,
) -> String {
    let live_target = live_note.strip_suffix(".md").unwrap_or(live_note);
    let link =
        format!("[[{live_target}|{}]]", title.replace(['[', ']', '|'], ""));
    let banner = vec![
        format!("> [!info] Earlier capture · replaced {date}"),
        format!("> Captured {captured} · {status} · 0 highlights · current:"),
        format!("> {link}"),
    ];
    let mut lines = body.lines().map(str::to_string).collect::<Vec<_>>();
    if let Some(embed) = ref_tasks::find_managed_embed(body)
        && embed.line_index < lines.len()
    {
        lines.splice(embed.line_index..=embed.line_index, banner);
    }
    for line in &mut lines {
        if ref_tasks::parse_tracker_line(line).is_some() {
            *line = neutralize_legacy_tracker(line);
        }
    }
    let mut frozen = lines.join("\n");
    if body.ends_with('\n') && !frozen.ends_with('\n') {
        frozen.push('\n');
    }
    frozen
}

fn neutralize_legacy_tracker(line: &str) -> String {
    let mut output = String::new();
    let mut rest = line;
    if let Some((prefix, after_marker)) = split_checkbox_prefix(rest) {
        output.push_str(prefix);
        output.push_str(after_marker);
        rest = "";
    }
    if !rest.is_empty() {
        output.push_str(rest);
    }
    let mut tokens = output
        .split_whitespace()
        .filter(|token| !matches!(*token, "#task" | "#ref" | "^ref"));
    let first = tokens.next().unwrap_or("");
    let remainder = tokens.collect::<Vec<_>>().join(" ");
    if first.is_empty() {
        "-".to_string()
    } else if remainder.is_empty() {
        first.to_string()
    } else {
        format!("{first} {remainder}")
    }
}

fn split_checkbox_prefix(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    let after_bullet = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))?;
    let bracket_end = after_bullet.find(']')?;
    if !after_bullet.starts_with('[') {
        return None;
    }
    let after = after_bullet[bracket_end + 1..].trim_start();
    Some((&line[..indent_len], after))
}

fn allocate_archive_path(
    config: &Config,
    ref_type: &str,
    stem: &str,
    date: &str,
) -> String {
    let parent = format!("ref_history/{ref_type}");
    let base = format!("{parent}/{stem}__{date}");
    for suffix in 1u32.. {
        let candidate = if suffix == 1 {
            format!("{base}.md")
        } else {
            format!("{base}_{suffix}.md")
        };
        if !config.bob_dir.join(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!("archive suffix space is unbounded")
}

fn yaml_frontmatter_value(lines: &[String], key: &str) -> Option<YamlValue> {
    let yaml = serde_yaml::from_str::<YamlValue>(&lines.join("\n")).ok()?;
    yaml.as_mapping()?
        .get(YamlValue::String(key.to_string()))
        .cloned()
}

fn has_user_pdf_annotations(pdf: &Path) -> Result<bool, String> {
    let bytes = fs::read(pdf)
        .map_err(|error| format!("read {}: {error}", pdf.display()))?;
    let marker = super::marker::read_pdf_marker_from_bytes(pdf, &bytes)
        .map_err(|error| error.to_string())?;
    let document = lopdf::Document::load_mem(&bytes)
        .map_err(|error| format!("parse {}: {error}", pdf.display()))?;
    for page_id in document.page_iter() {
        for annotation_id in
            super::marker::annotation_ids_for_page(&document, page_id)
                .map_err(|error| error.to_string())?
        {
            if annotation_id == marker.annotation_id {
                continue;
            }
            let annotation =
                document.get_dictionary(annotation_id).map_err(|error| {
                    format!("read annotation {annotation_id:?}: {error}")
                })?;
            let subtype = annotation
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .unwrap_or_default();
            // Link annotations are navigation, including audio-player links;
            // the overwrite policy preserves those ordinary links.
            if subtype == b"Link" {
                continue;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn annotated_refusal(
    row: &RefRow,
    assets: &Path,
    note_annotations: bool,
    sidecar_annotations: bool,
    pdf_annotations: bool,
    links: &super::rename::links::LinkRewritePlan,
) -> OverwriteRefusal {
    let mut evidence = Vec::new();
    if note_annotations {
        evidence.push("the reference note has managed highlights".to_string());
    }
    if sidecar_annotations {
        evidence.push("the Highlights sidecar has annotations".to_string());
    }
    if pdf_annotations {
        evidence.push("the PDF has user annotations".to_string());
    }
    if assets.exists() {
        evidence.push(format!("{} exists", assets.display()));
    }
    if !links.highlight_fragments.is_empty() {
        evidence.push("an inbound link targets a highlight".to_string());
    }
    if !links.highlight_follow_ups.is_empty() {
        evidence
            .push("an inbound link carries a highlight follow-up".to_string());
    }
    let _ = row;
    OverwriteRefusal::new(
        "annotated_reference",
        format!(
            "overwriting an annotated reference is not available yet: {}",
            evidence.join("; ")
        ),
        Some("-f starts a new reading"),
        2,
    )
}

fn collect_preimages(
    config: &Config,
    touched: &BTreeSet<PathBuf>,
) -> Result<Vec<FilePreimage>, String> {
    let mut preimages = BTreeMap::new();
    for relative in touched {
        let absolute = config.bob_dir.join(relative);
        collect_path_preimages(relative, &absolute, &mut preimages)?;
    }
    Ok(preimages
        .into_iter()
        .map(|(path, sha256)| FilePreimage { path, sha256 })
        .collect())
}

fn collect_path_preimages(
    relative: &Path,
    absolute: &Path,
    preimages: &mut BTreeMap<String, Option<String>>,
) -> Result<(), String> {
    let key = reference_mutation::vault_path(relative);
    let file_type = match fs::symlink_metadata(absolute) {
        Ok(metadata) => metadata.file_type(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            preimages.insert(key, None);
            return Ok(());
        }
        Err(error) => {
            return Err(format!("stat {}: {error}", absolute.display()));
        }
    };
    if file_type.is_symlink() {
        return Err(format!(
            "refusing to fingerprint symlink {}",
            absolute.display()
        ));
    }
    if file_type.is_file() {
        let bytes = fs::read(absolute)
            .map_err(|error| format!("read {}: {error}", absolute.display()))?;
        preimages.insert(key, Some(sha256(&bytes)));
        return Ok(());
    }
    if !file_type.is_dir() {
        return Err(format!(
            "unsupported touched path type: {}",
            absolute.display()
        ));
    }
    let mut entries = fs::read_dir(absolute)
        .map_err(|error| format!("scan {}: {error}", absolute.display()))?
        .map(|entry| {
            entry.map_err(|error| {
                format!("scan {}: {error}", absolute.display())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let child_relative = relative.join(entry.file_name());
        let child_absolute = entry.path();
        collect_path_preimages(&child_relative, &child_absolute, preimages)?;
    }
    let prefix = format!("{key}/");
    let mut hasher = Sha256::new();
    for (child_path, child_hash) in preimages
        .range(prefix.clone()..)
        .take_while(|(child_path, _)| child_path.starts_with(&prefix))
    {
        hasher.update(child_path.as_bytes());
        hasher.update([0]);
        hasher.update(child_hash.as_deref().unwrap_or("<absent>").as_bytes());
        hasher.update([0]);
    }
    preimages.insert(key, Some(hex::encode(hasher.finalize())));
    Ok(())
}

fn fingerprint_preimages(preimages: &[FilePreimage]) -> String {
    let mut hasher = Sha256::new();
    for preimage in preimages {
        hasher.update(preimage.path.as_bytes());
        hasher.update([0]);
        hasher.update(
            preimage.sha256.as_deref().unwrap_or("<absent>").as_bytes(),
        );
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn task_status(mark: char) -> Option<String> {
    match mark {
        ' ' => Some("ready".to_string()),
        '*' => Some("next".to_string()),
        '/' => Some("wip".to_string()),
        '?' => Some("blocked".to_string()),
        'x' | 'X' => Some("read".to_string()),
        '-' => Some("abandoned".to_string()),
        _ => None,
    }
}

fn dirty_error(error: reference_mutation::DirtyPathError) -> String {
    match error {
        reference_mutation::DirtyPathError::GitUnavailable(message) => message,
        reference_mutation::DirtyPathError::StatusFailed => {
            "git status failed".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_does_not_allocate_a_reread_number() {
        let mut family = recapture::SelectedFamily {
            root_stem: "paper".to_string(),
            root_path: "ref/papers/paper.md".to_string(),
            members: vec![recapture::FamilyMember {
                path: "ref/papers/paper.md".to_string(),
                stem: "paper".to_string(),
                reading: Some(999),
                ref_type: Some("papers".to_string()),
                task_path: Some("area.md".to_string()),
                task_block: Some("ref-paper".to_string()),
                has_audio: false,
                audio_path: None,
                status: Some("ready".to_string()),
                captured: None,
                highlights: Some(0),
                source_keys: Vec::new(),
                is_queued: false,
            }],
            selected: 0,
            source_keys: BTreeSet::new(),
            max_reading: 999,
        };
        family.selected = 0;
        assert_eq!(family.selected_member().stem, "paper");
        assert_eq!(family.max_reading, 999);
    }

    #[test]
    fn frozen_frontmatter_keeps_source_url_yaml_collection_type() {
        let old = "---\ntitle: Paper\nsource_url:\n  - https://example.test/a\n  - https://example.test/b\ncreated: 2020-01-01\nid: private\n---\n# Paper\n";
        let frozen = render_frozen_note(
            old,
            "ref/papers/paper.md",
            "papers",
            "paper",
            "2026-10-25",
            Some("2020-01-01"),
            "ready",
            "Paper",
        )
        .expect("frozen note");
        let (front, _) =
            super::super::split_frontmatter(&frozen).expect("frontmatter");
        let yaml = serde_yaml::from_str::<YamlValue>(&front.join("\n"))
            .expect("valid YAML");
        let source_url = yaml.as_mapping().unwrap().get("source_url").unwrap();
        assert!(
            source_url.is_sequence(),
            "source_url must remain a YAML list"
        );
        assert!(yaml.as_mapping().unwrap().get("id").is_none());
    }

    #[test]
    fn frozen_v2_note_replaces_only_the_managed_reading_embed() {
        let body =
            "\n# Paper\n\n![[area#^ref-paper]]\n\nAuthored prose stays.\n";
        let frozen = frozen_body(
            body,
            "ref/papers/paper.md",
            "Paper",
            "2026-10-25",
            "2020-01-01",
            "ready",
        );
        assert!(frozen.contains("Earlier capture · replaced 2026-10-25"));
        assert!(frozen.contains("Captured 2020-01-01 · ready · 0 highlights"));
        assert!(frozen.contains("[[ref/papers/paper|Paper]]"));
        assert!(frozen.contains("Authored prose stays."));
        assert!(!frozen.contains("![[area#^ref-paper]]"));
    }

    #[test]
    fn repeated_archive_lineage_stays_newest_first() {
        let first = prepend_archive_lineage(
            "---\ntitle: Paper\nearlier:\n  - \"[[ref/papers/paper__v2|v2 · 2026-01-01]]\"\n---\n# Paper\n",
            "ref_history/papers/paper__2026-10-24.md",
            "2026-10-24",
            0,
        )
        .unwrap();
        let second = prepend_archive_lineage(
            &first,
            "ref_history/papers/paper__2026-10-25.md",
            "2026-10-25",
            0,
        )
        .unwrap();
        let (front, _) = super::super::split_frontmatter(&second).unwrap();
        let parsed = ParsedFrontmatter::parse(&front);
        let entries = lineage::parse_earlier(&parsed);
        assert_eq!(entries[0].target, "ref_history/papers/paper__2026-10-25");
        assert_eq!(entries[1].target, "ref_history/papers/paper__2026-10-24");
        assert_eq!(entries[2].target, "ref/papers/paper__v2");
    }

    #[test]
    fn prose_comparison_ignores_managed_regions_and_tracker() {
        let generated = "\n# Paper\n\n- [ ] #task #ref [[lib/papers/paper.pdf]] #hide ^ref\n\n## Highlights\n\n<!-- highlights:begin -->\n\n<!-- highlights:end -->\n";
        let old = generated.replace("<!-- highlights:begin -->\n\n<!-- highlights:end -->", "<!-- highlights:begin -->\n\nold material\n<!-- highlights:end -->");
        assert!(!body_has_authored_prose(&old, generated, None).unwrap());
        let authored = old.replace("# Paper", "# Paper\n\nMy notes");
        assert!(body_has_authored_prose(&authored, generated, None).unwrap());
    }

    #[test]
    fn tracker_neutralization_cannot_leave_a_checkbox_task_source() {
        let line = "- [ ] #task #ref [[lib/papers/paper.pdf]] #hide ^ref";
        let inert = neutralize_legacy_tracker(line);
        assert!(!inert.contains("[ ]"));
        assert!(!inert.contains("#task"));
        assert!(!inert.contains("#ref"));
        assert!(!inert.contains("^ref"));
        assert!(ref_tasks::task_mark(&inert).is_none());
    }

    #[test]
    fn open_note_tasks_ignores_comments_and_fenced_examples() {
        let body = "# Paper\n\n<!--\n- [ ] hidden HTML task\n-->\n\n%%\n- [ ] hidden Obsidian task\n%%\n\n```markdown\n<!--\n- [ ] hidden fenced task\n-->\n```\n\n- [ ] real open task\n";
        assert_eq!(
            open_note_tasks(body, None),
            vec!["17:- [ ] real open task".to_string()]
        );
    }

    #[test]
    fn frozen_archive_neutralizes_legacy_tracker_with_external_task() {
        let body = "# Paper\n\n- [x] #task #ref [[lib/papers/paper.pdf]] ^ref\n\nAuthored prose stays.\n";
        let frozen = frozen_body(
            body,
            "ref/papers/paper.md",
            "Paper",
            "2026-10-25",
            "2020-01-01",
            "read",
        );
        assert!(!frozen.contains("#task"));
        assert!(!frozen.contains("#ref"));
        assert!(!frozen.contains("^ref"));
        assert!(frozen.contains("Authored prose stays."));
        assert!(frozen
            .lines()
            .all(|line| ref_tasks::task_mark(line).is_none()));
    }

    #[test]
    fn archive_path_suffixes_same_day_collisions() {
        let root = std::env::temp_dir()
            .join(format!("bob-overwrite-archive-{}", std::process::id()));
        let config = Config {
            bob_dir: root.clone(),
            ref_dir: root.join("ref"),
            lib_dir: root.join("lib"),
            xlib_dir: root.join("xlib"),
        };
        fs::create_dir_all(root.join("ref_history/papers")).unwrap();
        fs::write(root.join("ref_history/papers/paper__2026-10-25.md"), "old")
            .unwrap();
        assert_eq!(
            allocate_archive_path(&config, "papers", "paper", "2026-10-25"),
            "ref_history/papers/paper__2026-10-25_2.md"
        );
        let _ = fs::remove_dir_all(root);
    }
}
