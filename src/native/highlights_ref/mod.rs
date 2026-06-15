use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error as StdError,
    ffi::{OsStr, OsString},
    fmt, fs, io, iter,
    path::{Component, Path, PathBuf},
    process::{self, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

use chrono::{Local, SecondsFormat, Utc};
use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use lopdf::{
    decode_text_string, encode_utf16_be, Document, Object, ObjectId,
    StringFormat,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    env as bob_env, ob,
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob highlights";
const DEFAULT_LIB_DIR: &str = "lib";
const DEFAULT_REF_DIR: &str = "ref";

const ENV_LIB_DIR: &str = "BOB_HIGHLIGHTS_LIB_DIR";
const ENV_REF_DIR: &str = "BOB_HIGHLIGHTS_REF_DIR";

const FIELD_STATUS: &str = "status";
const FIELD_PARENT: &str = "parent";
const FIELD_NOTE_TYPE: &str = "type";
const FIELD_REF_TYPE: &str = "ref_type";
const NOTE_TYPE_VALUE: &str = "[[ref]]";
const STATUS_UNREAD: &str = "unread";
const STATUS_WIP: &str = "wip";
const STATUS_READ: &str = "read";
const STATUS_ABANDONED: &str = "abandoned";
const STATUS_LEGACY: &str = "legacy";
const DEPRECATED_STATUS_DONE: &str = "done";
const ALLOWED_STATUS_VALUES: &[&str] = &[
    STATUS_UNREAD,
    STATUS_WIP,
    STATUS_READ,
    STATUS_ABANDONED,
    STATUS_LEGACY,
];
const MARKER_REQUIRED_KEYS: &[&str] = &[FIELD_STATUS, FIELD_PARENT];
const COMMAND_MANAGED_FIELDS: &[&str] = &[FIELD_NOTE_TYPE, FIELD_REF_TYPE];
const MANAGED_BODY_BEGIN: &str = "<!-- highlights:begin -->";
const MANAGED_BODY_END: &str = "<!-- highlights:end -->";
const PDF_TASK_BLOCK_ID: &str = "^ref";
const PDF_TASK_PRIORITY: &str = "[p::2]";
const PDF_TASK_TAG: &str = "#task";
const HIGHLIGHT_TASK_FIELD: &str = "h";
const LEGACY_HIGHLIGHT_TASK_FIELD: &str = "highlight_task";
const HIGHLIGHT_TASK_ID_VERSION: &str = "v1";
const SOURCE_TASK_BLOCK_ID_PREFIX: &str = "ht-";
const PIPELINE_VERSION: &str = "highlights-ref-mvp-3";
const REMOVED_HIGHLIGHTS_HEADING: &str = "### Removed highlights";
const SOURCE_LINK_ALIAS: &str = "🔖";
const TEXTBUNDLE_TEXT_FILES: &[&str] = &["text.md", "text.markdown"];

const FIELD_SOURCE_PDF: &str = "source_pdf";
const FIELD_SOURCE_PDF_SHA256: &str = "source_pdf_sha256";
const FIELD_HIGHLIGHTS_SIDECAR: &str = "highlights_sidecar";
const FIELD_HIGHLIGHTS_COUNT: &str = "highlights_count";
const FIELD_HIGHLIGHTS_SYNCED_AT: &str = "highlights_synced_at";
const FIELD_MARKER_BASE: &str = "highlights_marker_base";
const FIELD_MARKER_HASH: &str = "highlights_marker_hash";
const FIELD_MARKER_FIELDS: &str = "highlights_marker_fields";
const FIELD_PIPELINE_VERSION: &str = "pipeline_version";

const PIPELINE_FIELDS: &[&str] = &[
    FIELD_SOURCE_PDF,
    FIELD_SOURCE_PDF_SHA256,
    FIELD_HIGHLIGHTS_SIDECAR,
    FIELD_HIGHLIGHTS_COUNT,
    FIELD_HIGHLIGHTS_SYNCED_AT,
    FIELD_MARKER_BASE,
    FIELD_MARKER_HASH,
    FIELD_MARKER_FIELDS,
    FIELD_PIPELINE_VERSION,
];

const COMMON_USER_FIELDS: &[&str] = &[
    FIELD_PARENT,
    "title",
    "aliases",
    "topics",
    "source_url",
    "author",
    "published",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    bob_dir: PathBuf,
    lib_dir: PathBuf,
    ref_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prefer {
    Marker,
    Frontmatter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncOptions {
    dry_run: bool,
    write_pdf: bool,
    prefer: Option<Prefer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "value")]
enum MarkerValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    List(Vec<MarkerValue>),
}

type Projection = BTreeMap<String, MarkerValue>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandError {
    message: String,
}

impl CommandError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for CommandError {}

type Result<T> = std::result::Result<T, CommandError>;

#[derive(Debug, Clone)]
struct PdfMarker {
    annotation_id: ObjectId,
    contents: String,
    page_number: u32,
    note_number: usize,
    source_pdf_sha256: String,
}

#[derive(Debug, Clone)]
struct FrontmatterEntry {
    key: Option<String>,
    value: Option<MarkerValue>,
    raw: String,
}

#[derive(Debug, Clone)]
struct ParsedNote {
    frontmatter: Vec<FrontmatterEntry>,
    body: String,
    original: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PdfTaskLine {
    line_index: usize,
    checkbox_mark_index: usize,
    checked: bool,
    mark: char,
}

impl PdfTaskLine {
    fn status(self) -> PdfTaskStatus {
        match self.mark {
            'x' | 'X' => PdfTaskStatus::Read,
            '-' => PdfTaskStatus::Abandoned,
            _ => PdfTaskStatus::Unchecked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdfTaskLineState {
    Missing,
    Present(PdfTaskLine),
}

impl PdfTaskLineState {
    fn status(self) -> PdfTaskStatus {
        match self {
            PdfTaskLineState::Missing => PdfTaskStatus::Missing,
            PdfTaskLineState::Present(task_line) => task_line.status(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdfTaskStatus {
    Missing,
    Unchecked,
    Read,
    Abandoned,
}

impl PdfTaskStatus {
    fn target_status(self) -> Option<&'static str> {
        match self {
            PdfTaskStatus::Read => Some(STATUS_READ),
            PdfTaskStatus::Abandoned => Some(STATUS_ABANDONED),
            PdfTaskStatus::Missing | PdfTaskStatus::Unchecked => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            PdfTaskStatus::Missing => "missing",
            PdfTaskStatus::Unchecked => "unchecked",
            PdfTaskStatus::Read => "checked",
            PdfTaskStatus::Abandoned => "cancelled",
        }
    }

    fn contribution_reason(self) -> Option<&'static str> {
        match self {
            PdfTaskStatus::Read => Some("checked PDF task set status read"),
            PdfTaskStatus::Abandoned => {
                Some("cancelled PDF task set status abandoned")
            }
            PdfTaskStatus::Missing | PdfTaskStatus::Unchecked => None,
        }
    }

    fn conflict_action(self) -> &'static str {
        match self {
            PdfTaskStatus::Read => "uncheck",
            PdfTaskStatus::Abandoned => "uncancel",
            PdfTaskStatus::Missing | PdfTaskStatus::Unchecked => "clear",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PdfTaskStatusSignal {
    status: PdfTaskStatus,
    status_contributed: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct PipelineMetadata {
    source_pdf: String,
    source_pdf_sha256: String,
    ref_type: Option<String>,
    highlights_sidecar: Option<MarkerValue>,
    highlights_count: Option<MarkerValue>,
    highlights_synced_at: Option<MarkerValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PdfPathMetadata {
    relative_pdf_path: Option<PathBuf>,
    note_relative_path: PathBuf,
    ref_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncSource {
    Marker,
    Frontmatter,
    AutoMerge,
}

#[derive(Debug, Clone)]
struct SyncDecision {
    source: SyncSource,
    reason: String,
    marker_contributed: bool,
    frontmatter_contributed: bool,
}

#[derive(Debug, Clone)]
struct SyncResolution {
    decision: SyncDecision,
    projection: Projection,
}

#[derive(Debug, Clone, Copy)]
struct SyncInputs<'a> {
    last_hash: Option<&'a str>,
    base_projection: Option<&'a Projection>,
    marker_projection: &'a Projection,
    marker_hash: &'a str,
    frontmatter_projection: &'a Projection,
    frontmatter_hash: &'a str,
    note_exists: bool,
    prefer: Option<Prefer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectionConflict {
    key: String,
    base: Option<MarkerValue>,
    marker: Option<MarkerValue>,
    frontmatter: Option<MarkerValue>,
}

#[derive(Debug, Clone)]
struct SidecarInput {
    path: PathBuf,
    annotations: Vec<SidecarAnnotation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarAnnotation {
    kind: SidecarAnnotationKind,
    page_label: Option<String>,
    linked_page_style: bool,
    text: String,
    comment: Option<String>,
    task_source: Option<String>,
    image: Option<SidecarImage>,
    order: usize,
    ordinal_on_page: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarImage {
    target: String,
    alt_text: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarPageHeading {
    label: String,
    linked_page_style: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidecarAnnotationKind {
    Highlight,
    Image,
    StandaloneNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedHighlights {
    content: String,
    count: usize,
    image_count: usize,
    image_assets: Vec<ImageAssetWrite>,
    block_ids_by_annotation_order: BTreeMap<usize, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageAssetWrite {
    annotation_order: usize,
    source_path: PathBuf,
    dest_path: PathBuf,
    vault_relative_dest_path: PathBuf,
    source_sha256: String,
    block_id: String,
    action: ImageAssetAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageAssetAction {
    Copy,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnnotationTaskCandidate {
    identity: String,
    task_text: String,
    source_block_id: String,
    target: AnnotationTaskTarget,
    source_ref_note_path: PathBuf,
    processed_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AnnotationTaskTarget {
    ReferenceNote,
    RoutedNote(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnnotationTaskSource {
    identity: String,
    task_text: String,
    route_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProcessedTaskIndex {
    legacy_source_task_anchors: BTreeSet<String>,
    processed_ids: BTreeSet<String>,
    legacy_identities: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutedTaskNoteWrite {
    path: PathBuf,
    original_contents: String,
    rendered_contents: String,
    action: &'static str,
}

#[derive(Debug, Clone)]
struct PdfSyncPlan {
    pdf: PathBuf,
    note_path: PathBuf,
    sidecar_path: Option<PathBuf>,
    marker: PdfMarker,
    decision: SyncDecision,
    rendered_highlights_count: Option<usize>,
    synced_projection: Projection,
    synced_hash: String,
    rendered_marker: String,
    marker_write_needed: bool,
    note: ParsedNote,
    sidecar: Option<SidecarInput>,
    rendered_highlights: Option<RenderedHighlights>,
    stable_metadata: PipelineMetadata,
    rendered_body: String,
    stable_rendered_note: String,
    stable_note_action: &'static str,
    image_assets: Vec<ImageAssetWrite>,
    annotation_task_candidates: Vec<AnnotationTaskCandidate>,
    annotation_tasks_created: usize,
    annotation_tasks_skipped: usize,
    routed_task_note_writes: Vec<RoutedTaskNoteWrite>,
    pdf_task_signal: PdfTaskStatusSignal,
    status_normalization: StatusNormalization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncWriteReport {
    note_action: &'static str,
    marker_action: &'static str,
    image_count: usize,
    image_assets_written: usize,
    image_assets_skipped: usize,
    routed_note_actions: usize,
    annotation_tasks_created: usize,
    annotation_tasks_skipped: usize,
}

#[derive(Debug, Clone)]
struct ScanFailure {
    pdf: PathBuf,
    error: CommandError,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StatusNormalization {
    marker: bool,
    frontmatter: bool,
    base: bool,
}

#[derive(Debug, Clone)]
struct NormalizedProjection {
    projection: Projection,
    status_normalized: bool,
}

#[derive(Debug, Clone)]
enum ScanPlanOutcome {
    Planned(Box<PdfSyncPlan>),
    Failed(ScanFailure),
}

#[derive(Debug, Clone)]
enum ScanWriteOutcome {
    Written(SyncWriteReport),
    Failed(ScanFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GitStatus {
    MissingCommand,
    NotWorktree,
    Worktree { entries: Vec<GitStatusEntry> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitStatusEntry {
    index_status: char,
    worktree_status: char,
    path: PathBuf,
    raw: String,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let matches = match build_cli().try_get_matches_from(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let exit_code = error.exit_code();
            if let Err(print_error) = error.print() {
                eprintln!(
                    "{COMMAND_NAME}: failed to print command-line error: {print_error}"
                );
            }
            return exit_code;
        }
    };

    match matches.subcommand() {
        Some(("scan", sub_matches)) => run_scan(sub_matches),
        Some(("sync", sub_matches)) => run_sync(sub_matches),
        Some(("doctor", sub_matches)) => run_doctor(sub_matches),
        Some(("marker", sub_matches)) => run_marker(sub_matches),
        _ => 2,
    }
}

fn run_scan(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    let options = SyncOptions {
        dry_run: matches.get_flag("dry-run"),
        write_pdf: matches.get_flag("write-pdfs"),
        prefer: None,
    };
    report_result(scan_library(
        &config,
        options,
        jobs_from_matches(matches),
        matches.get_flag("verbose"),
    ))
}

/// Resolve the requested degree of cross-PDF parallelism for `scan`.
///
/// Defaults to the number of available CPU cores; `--jobs 1` forces the
/// original sequential behavior.
fn jobs_from_matches(matches: &ArgMatches) -> usize {
    matches
        .get_one::<u64>("jobs")
        .map(|jobs| *jobs as usize)
        .unwrap_or_else(default_jobs)
}

fn default_jobs() -> usize {
    thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(1)
}

fn run_sync(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    let pdf = required_path(matches, "pdf");
    let options = SyncOptions {
        dry_run: matches.get_flag("dry-run"),
        write_pdf: matches.get_flag("write-pdf"),
        prefer: prefer_from_matches(matches),
    };

    report_result(sync_pdf(&config, &pdf, options))
}

fn run_doctor(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    report_result(doctor_vault(&config))
}

fn run_marker(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    let pdf = required_path(matches, "pdf");
    report_result(show_marker(&config, &pdf))
}

fn report_result(result: Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{COMMAND_NAME}: {error}");
            1
        }
    }
}

fn sync_pdf(config: &Config, pdf: &Path, options: SyncOptions) -> Result<()> {
    let mut plan = plan_pdf_sync(config, pdf, options)?;
    finalize_annotation_task_plans(config, &mut [&mut plan])?;
    print_pdf_sync_report("sync", config, &plan, options);

    if options.dry_run {
        println!("note_action: {}", plan.stable_note_action);
        println!(
            "pdf_marker_action: {}",
            if plan.marker_write_needed {
                "would-update"
            } else {
                "none"
            }
        );
        println!("writes: none");
        return Ok(());
    }

    ensure_safe_to_write(config, iter::once(&plan))?;
    let report = execute_pdf_sync(config, &plan)?;
    print_sync_write_report(report);
    Ok(())
}

fn scan_library(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    verbose: bool,
) -> Result<()> {
    let pdfs = collect_pdf_paths(config)?;
    validate_output_collisions(config, &pdfs)?;

    // `plan_pdf_sync` is a pure, read-only computation over an independent
    // `&Config` and one PDF path, so planning is embarrassingly parallel. We
    // collect into a position-keyed vector and reassemble in `pdfs` order so
    // reporting output stays deterministic regardless of completion order.
    let mut plan_outcomes = pdfs
        .iter()
        .zip(plan_pdfs(config, &pdfs, options, jobs))
        .map(|(pdf, result)| match result {
            Ok(plan) => ScanPlanOutcome::Planned(Box::new(plan)),
            Err(error) => ScanPlanOutcome::Failed(ScanFailure {
                pdf: pdf.clone(),
                error,
            }),
        })
        .collect::<Vec<_>>();
    let mut plans_to_finalize = plan_outcomes
        .iter_mut()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_mut()),
            ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    finalize_annotation_task_plans(config, &mut plans_to_finalize)?;
    let plans = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    validate_planned_asset_collisions(&plans)?;
    let plan_failures = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(_) => None,
            ScanPlanOutcome::Failed(failure) => Some(failure),
        })
        .collect::<Vec<_>>();

    let styler = Styler::detect();
    if verbose {
        print_verbose_scan_plan_report(
            config,
            options,
            pdfs.len(),
            &plan_outcomes,
        );
    } else {
        print_scan_header(config, pdfs.len(), options.dry_run, &styler);
    }

    if options.dry_run {
        if verbose {
            print_scan_plan_summary(&plans, plan_failures.len());
            println!("writes: none");
        } else {
            print_concise_scan_plan_report(
                &plan_outcomes,
                &plans,
                pdfs.len(),
                plan_failures.len(),
                &styler,
            );
        }
        return if plan_failures.is_empty() {
            Ok(())
        } else {
            Err(scan_partial_failure_error(&plan_failures, &[]))
        };
    }

    ensure_safe_to_write(config, plans.iter().copied())?;
    let mut write_outcomes = Vec::new();
    for plan in &plans {
        match execute_pdf_sync(config, plan) {
            Ok(report) => {
                write_outcomes.push(ScanWriteOutcome::Written(report))
            }
            Err(error) => {
                write_outcomes.push(ScanWriteOutcome::Failed(ScanFailure {
                    pdf: plan.pdf.clone(),
                    error,
                }));
            }
        }
    }
    let reports = write_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanWriteOutcome::Written(report) => Some(*report),
            ScanWriteOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    let write_failures = write_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanWriteOutcome::Written(_) => None,
            ScanWriteOutcome::Failed(failure) => Some(failure),
        })
        .collect::<Vec<_>>();
    if verbose {
        for failure in &write_failures {
            print_scan_write_failure_entry(failure);
        }
        print_scan_write_summary(
            &reports,
            plan_failures.len(),
            write_failures.len(),
        );
    } else {
        print_concise_scan_write_report(
            &plan_outcomes,
            &write_outcomes,
            &reports,
            pdfs.len(),
            plan_failures.len(),
            write_failures.len(),
            &styler,
        );
    }
    if plan_failures.is_empty() && write_failures.is_empty() {
        Ok(())
    } else {
        Err(scan_partial_failure_error(&plan_failures, &write_failures))
    }
}

/// Plan every PDF, returning results in the same order as `pdfs`.
///
/// With `jobs <= 1` (or a trivial workload) this is the original sequential
/// loop. Otherwise a small fixed pool of scoped threads pulls PDFs off a shared
/// counter; each worker keeps its results paired with their original index so
/// we can restore `pdfs` order before returning.
fn plan_pdfs(
    config: &Config,
    pdfs: &[PathBuf],
    options: SyncOptions,
    jobs: usize,
) -> Vec<Result<PdfSyncPlan>> {
    if jobs <= 1 || pdfs.len() <= 1 {
        return pdfs
            .iter()
            .map(|pdf| plan_pdf_sync(config, pdf, options))
            .collect();
    }

    let next = AtomicUsize::new(0);
    let worker_count = jobs.min(pdfs.len());
    let mut indexed: Vec<(usize, Result<PdfSyncPlan>)> =
        thread::scope(|scope| {
            let handles: Vec<_> = (0..worker_count)
                .map(|_| {
                    let next = &next;
                    scope.spawn(move || {
                        let mut local = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(pdf) = pdfs.get(index) else {
                                break;
                            };
                            local.push((
                                index,
                                plan_pdf_sync(config, pdf, options),
                            ));
                        }
                        local
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| {
                    handle.join().expect("planning worker thread panicked")
                })
                .collect()
        });

    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, result)| result).collect()
}

fn plan_pdf_sync(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
) -> Result<PdfSyncPlan> {
    let marker = read_pdf_marker(pdf)?;
    let marker_input = parse_marker_with_normalization(&marker.contents)?;
    let marker_projection = marker_input.projection;
    let note_path = ref_note_path(config, pdf)?;
    validate_note_target(&note_path)?;
    let note = read_note(&note_path)?;
    let pdf_task_line = parse_pdf_task_line(&note.body)?;
    let frontmatter_input = note.synced_projection_with_normalization()?;
    let frontmatter_projection = frontmatter_input.projection;
    let marker_hash = projection_hash(&marker_projection)?;
    let frontmatter_hash = projection_hash(&frontmatter_projection)?;
    let last_hash = note.marker_hash();
    let (base_projection, base_status_normalized) =
        note.marker_base_projection_with_normalization()?;
    let status_normalization = StatusNormalization {
        marker: marker_input.status_normalized,
        frontmatter: frontmatter_input.status_normalized,
        base: base_status_normalized,
    };
    let mut resolution = resolve_sync_projection(SyncInputs {
        last_hash: last_hash.as_deref(),
        base_projection: base_projection.as_ref(),
        marker_projection: &marker_projection,
        marker_hash: &marker_hash,
        frontmatter_projection: &frontmatter_projection,
        frontmatter_hash: &frontmatter_hash,
        note_exists: note.exists(),
        prefer: options.prefer,
    })?;
    let annotation_task_intake_allowed =
        projection_status_is(&resolution.projection, STATUS_WIP);
    let pdf_task_signal = apply_pdf_task_status_signal(
        &mut resolution,
        &pdf_task_line,
        base_projection.as_ref(),
        &marker_projection,
        &frontmatter_projection,
    )?;
    let decision = resolution.decision;
    let synced_projection = resolution.projection;
    validate_required_marker_keys(
        &synced_projection,
        decision.source.as_str(),
    )?;
    let synced_hash = projection_hash(&synced_projection)?;
    let rendered_marker = render_marker(&synced_projection)?;
    let marker_write_needed = (decision.frontmatter_contributed
        || status_normalization.marker)
        && normalize_line_endings(&rendered_marker)
            != normalize_line_endings(&marker.contents);

    if marker_write_needed && !options.write_pdf && !options.dry_run {
        return Err(CommandError::new(
            "reference note changed but --write-pdf was not supplied; refusing to update the PDF marker",
        ));
    }

    let sidecar = read_sidecar_for_pdf(pdf)?;
    let rendered_highlights = sidecar
        .as_ref()
        .map(|sidecar| {
            render_sidecar_highlights(config, pdf, &note_path, &note, sidecar)
        })
        .transpose()?;
    let sidecar_path = sidecar.as_ref().map(|sidecar| sidecar.path.clone());
    let rendered_highlights_count =
        rendered_highlights.as_ref().map(|rendered| rendered.count);
    let image_assets = rendered_highlights
        .as_ref()
        .map(|rendered| rendered.image_assets.clone())
        .unwrap_or_default();
    let stable_metadata = pipeline_metadata(
        config,
        pdf,
        &marker.source_pdf_sha256,
        &note,
        sidecar.as_ref(),
        rendered_highlights.as_ref(),
        false,
    )?;
    let rendered_body = note.render_body(
        pdf,
        &synced_projection,
        &stable_metadata.source_pdf,
        rendered_highlights.as_ref(),
    )?;
    let annotation_task_candidates = if annotation_task_intake_allowed {
        annotation_task_candidates(
            config,
            &note_path,
            pdf,
            sidecar.as_ref(),
            rendered_highlights.as_ref(),
        )?
    } else {
        Vec::new()
    };
    let stable_rendered_note = note.render_with_projection(
        &synced_projection,
        &synced_hash,
        &stable_metadata,
        &rendered_body,
    );
    let stable_note_action = change_action(
        note.exists(),
        note.contents().as_deref(),
        &stable_rendered_note,
    );

    Ok(PdfSyncPlan {
        pdf: pdf.to_path_buf(),
        note_path,
        sidecar_path,
        marker,
        decision,
        rendered_highlights_count,
        synced_projection,
        synced_hash,
        rendered_marker,
        marker_write_needed,
        note,
        sidecar,
        rendered_highlights,
        stable_metadata,
        rendered_body,
        stable_rendered_note,
        stable_note_action,
        image_assets,
        annotation_task_candidates,
        annotation_tasks_created: 0,
        annotation_tasks_skipped: 0,
        routed_task_note_writes: Vec::new(),
        pdf_task_signal,
        status_normalization,
    })
}

fn finalize_annotation_task_plans(
    config: &Config,
    plans: &mut [&mut PdfSyncPlan],
) -> Result<()> {
    // Building the processed-task index walks the entire vault. Skip it
    // entirely when no plan carries annotation-task candidates: with nothing to
    // accept/reject there are no routed groups to form and no reference-note
    // bodies to mutate, so the index would never be consulted. This keeps the
    // common `sync`/`scan` path (non-`wip` PDFs, or `wip` PDFs with no `#task`
    // bullets) free of the vault-wide scan.
    if plans
        .iter()
        .all(|plan| plan.annotation_task_candidates.is_empty())
    {
        return Ok(());
    }

    let mut processed = processed_task_index(config)?;
    let created_date = current_local_date();
    let mut routed_groups: BTreeMap<PathBuf, RoutedTaskGroup> = BTreeMap::new();

    for (plan_index, plan) in plans.iter_mut().enumerate() {
        let plan = &mut **plan;
        plan.annotation_tasks_created = 0;
        plan.annotation_tasks_skipped = 0;
        plan.routed_task_note_writes.clear();

        let mut reference_task_lines = Vec::new();
        for candidate in std::mem::take(&mut plan.annotation_task_candidates) {
            if !processed.accept(&candidate) {
                plan.annotation_tasks_skipped += 1;
                continue;
            }

            let task_line =
                render_annotation_task_line(config, &candidate, &created_date);
            plan.annotation_tasks_created += 1;
            match &candidate.target {
                AnnotationTaskTarget::ReferenceNote => {
                    reference_task_lines.push(task_line);
                }
                AnnotationTaskTarget::RoutedNote(path) => {
                    let group = routed_groups
                        .entry(path.clone())
                        .or_insert_with(|| RoutedTaskGroup {
                            owner_plan_index: plan_index,
                            lines: Vec::new(),
                        });
                    group.lines.push(task_line);
                }
            }
        }

        if !reference_task_lines.is_empty() {
            plan.rendered_body = insert_annotation_task_lines_after_pdf_task(
                &plan.rendered_body,
                &reference_task_lines,
            )?;
            refresh_stable_rendered_note(plan);
        }
    }

    for (path, group) in routed_groups {
        if group.lines.is_empty() {
            continue;
        }
        let original_contents = fs::read_to_string(&path).map_err(|error| {
            CommandError::new(format!(
                "read routed task note {}: {error}",
                path.display()
            ))
        })?;
        let rendered_contents =
            append_task_lines(&original_contents, &group.lines);
        let action =
            change_action(true, Some(&original_contents), &rendered_contents);
        if action == "none" {
            continue;
        }
        plans[group.owner_plan_index].routed_task_note_writes.push(
            RoutedTaskNoteWrite {
                path,
                original_contents,
                rendered_contents,
                action,
            },
        );
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct RoutedTaskGroup {
    owner_plan_index: usize,
    lines: Vec<String>,
}

impl ProcessedTaskIndex {
    fn accept(&mut self, candidate: &AnnotationTaskCandidate) -> bool {
        let legacy_source_task_anchor =
            annotation_task_legacy_source_task_block_id(candidate);
        if self
            .legacy_source_task_anchors
            .contains(&legacy_source_task_anchor)
            || self.processed_ids.contains(&candidate.processed_id)
            || self.legacy_identities.contains(&candidate.identity)
        {
            return false;
        }

        self.legacy_source_task_anchors
            .insert(legacy_source_task_anchor);
        self.processed_ids.insert(candidate.processed_id.clone());
        self.legacy_identities.insert(candidate.identity.clone());
        true
    }
}

fn refresh_stable_rendered_note(plan: &mut PdfSyncPlan) {
    plan.stable_rendered_note = plan.note.render_with_projection(
        &plan.synced_projection,
        &plan.synced_hash,
        &plan.stable_metadata,
        &plan.rendered_body,
    );
    plan.stable_note_action = change_action(
        plan.note.exists(),
        plan.note.contents().as_deref(),
        &plan.stable_rendered_note,
    );
}

fn execute_pdf_sync(
    config: &Config,
    plan: &PdfSyncPlan,
) -> Result<SyncWriteReport> {
    if note_write_planned(plan) {
        ensure_note_unchanged_for_write(plan)?;
    }
    for write in &plan.routed_task_note_writes {
        if write.action != "none" {
            ensure_routed_note_unchanged_for_write(write)?;
        }
    }
    if plan.marker_write_needed {
        ensure_pdf_unchanged_for_write(plan)?;
        write_pdf_marker(
            &plan.pdf,
            plan.marker.annotation_id,
            &plan.rendered_marker,
        )?;
    }

    let mut image_assets_written = 0usize;
    let mut image_assets_skipped = 0usize;
    for write in &plan.image_assets {
        if execute_image_asset_write(write)? {
            image_assets_written += 1;
        } else {
            image_assets_skipped += 1;
        }
    }

    let refresh_synced_at =
        plan.stable_note_action != "none" && plan.rendered_highlights.is_some();
    let refresh_metadata = plan.marker_write_needed || refresh_synced_at;
    let rendered_note = if refresh_metadata {
        // `write_pdf_marker` rewrites the PDF in place, so the planning-time
        // hash is stale; rehash the file to record the post-write digest.
        // When no marker write happened the PDF is untouched, so the hash the
        // planner already computed from the same bytes is reused for free.
        let source_pdf_sha256 = if plan.marker_write_needed {
            sha256_file(&plan.pdf)?
        } else {
            plan.marker.source_pdf_sha256.clone()
        };
        let metadata = pipeline_metadata(
            config,
            &plan.pdf,
            &source_pdf_sha256,
            &plan.note,
            plan.sidecar.as_ref(),
            plan.rendered_highlights.as_ref(),
            refresh_synced_at,
        )?;
        plan.note.render_with_projection(
            &plan.synced_projection,
            &plan.synced_hash,
            &metadata,
            &plan.rendered_body,
        )
    } else {
        plan.stable_rendered_note.clone()
    };
    let note_action = change_action(
        plan.note.exists(),
        plan.note.contents().as_deref(),
        &rendered_note,
    );
    if plan.note.contents().as_deref() != Some(rendered_note.as_str()) {
        ensure_note_unchanged_for_write(plan)?;
        atomic_write(&plan.note_path, &rendered_note)?;
    }
    let mut routed_note_actions = 0usize;
    for write in &plan.routed_task_note_writes {
        if write.action == "none" {
            continue;
        }
        ensure_routed_note_unchanged_for_write(write)?;
        atomic_write(&write.path, &write.rendered_contents)?;
        routed_note_actions += 1;
    }

    Ok(SyncWriteReport {
        note_action,
        marker_action: if plan.marker_write_needed {
            "updated"
        } else {
            "none"
        },
        image_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.image_count)
            .unwrap_or(0),
        image_assets_written,
        image_assets_skipped,
        routed_note_actions,
        annotation_tasks_created: plan.annotation_tasks_created,
        annotation_tasks_skipped: plan.annotation_tasks_skipped,
    })
}

fn execute_image_asset_write(write: &ImageAssetWrite) -> Result<bool> {
    match fs::read(&write.dest_path) {
        Ok(bytes) => {
            let dest_sha256 = hex::encode(Sha256::digest(bytes));
            if dest_sha256 == write.source_sha256 {
                return Ok(false);
            }
            return Err(CommandError::new(format!(
                "image asset destination exists with different bytes: {}",
                write.dest_path.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CommandError::new(format!(
                "read image asset destination {}: {error}",
                write.dest_path.display()
            )));
        }
    }

    atomic_copy(&write.source_path, &write.dest_path)?;
    Ok(true)
}

fn ensure_note_unchanged_for_write(plan: &PdfSyncPlan) -> Result<()> {
    if note_contents_match_plan(
        &plan.note_path,
        plan.note.contents().as_deref(),
    )? {
        Ok(())
    } else {
        Err(CommandError::new(format!(
            "reference note changed during sync; rerun: {}",
            plan.note_path.display()
        )))
    }
}

fn ensure_routed_note_unchanged_for_write(
    write: &RoutedTaskNoteWrite,
) -> Result<()> {
    if note_contents_match_plan(&write.path, Some(&write.original_contents))? {
        Ok(())
    } else {
        Err(CommandError::new(format!(
            "routed task note changed during sync; rerun: {}",
            write.path.display()
        )))
    }
}

fn note_contents_match_plan(
    path: &Path,
    expected: Option<&str>,
) -> Result<bool> {
    match (expected, fs::read_to_string(path)) {
        (Some(expected), Ok(current)) => Ok(current == expected),
        (None, Err(error)) if error.kind() == io::ErrorKind::NotFound => {
            Ok(true)
        }
        (None, Ok(_)) => Ok(false),
        (Some(_), Err(error)) if error.kind() == io::ErrorKind::NotFound => {
            Ok(false)
        }
        (_, Err(error)) => Err(CommandError::new(format!(
            "read note {} before write: {error}",
            path.display()
        ))),
    }
}

fn ensure_pdf_unchanged_for_write(plan: &PdfSyncPlan) -> Result<()> {
    let current_hash = sha256_file(&plan.pdf)?;
    if current_hash == plan.marker.source_pdf_sha256 {
        Ok(())
    } else {
        Err(CommandError::new(format!(
            "PDF changed during sync; rerun: {}",
            plan.pdf.display()
        )))
    }
}

fn print_pdf_sync_report(
    operation: &str,
    config: &Config,
    plan: &PdfSyncPlan,
    options: SyncOptions,
) {
    print_config_report(operation, config);
    println!("pdf: {}", plan.pdf.display());
    println!("note: {}", plan.note_path.display());
    println!(
        "sidecar: {}",
        plan.sidecar_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    println!("dry_run: {}", options.dry_run);
    println!("write_pdf: {}", options.write_pdf);
    if let Some(prefer) = options.prefer {
        println!("prefer: {}", prefer.as_str());
    }
    println!("marker_page: {}", plan.marker.page_number);
    println!("marker_note: {}", plan.marker.note_number);
    println!("sync_source: {}", plan.decision.source.as_str());
    println!("sync_reason: {}", plan.decision.reason);
    println!("pdf_task: {}", plan.pdf_task_signal.status.label());
    if let Some(status) = plan.pdf_task_signal.status_contributed {
        println!("pdf_task_contribution: status={status}");
    }
    if let Some(label) = status_normalization_label(plan.status_normalization) {
        println!("status_normalization: {label}");
    }
    if plan.decision.source == SyncSource::AutoMerge {
        println!(
            "sync_marker_contributed: {}",
            plan.decision.marker_contributed
        );
        println!(
            "sync_frontmatter_contributed: {}",
            plan.decision.frontmatter_contributed
        );
    }
    if let Some(count) = plan.rendered_highlights_count {
        println!("highlights_count: {count}");
    }
    if let Some(rendered) = &plan.rendered_highlights
        && (rendered.image_count > 0 || !plan.image_assets.is_empty())
    {
        println!("images: {}", rendered.image_count);
        println!("image_assets: {}", planned_image_asset_write_count(plan));
    }
    println!("annotation_tasks_create: {}", plan.annotation_tasks_created);
    println!("annotation_tasks_skip: {}", plan.annotation_tasks_skipped);
    println!(
        "routed_task_note_writes: {}",
        planned_routed_note_write_count(plan)
    );
}

fn print_sync_write_report(report: SyncWriteReport) {
    println!("note_action: {}", report.note_action);
    println!("pdf_marker_action: {}", report.marker_action);
    if report.image_count > 0 || report.image_assets_written > 0 {
        println!("images: {}", report.image_count);
        println!("image_assets_written: {}", report.image_assets_written);
        println!("image_assets_skipped: {}", report.image_assets_skipped);
    }
    println!(
        "annotation_tasks_created: {}",
        report.annotation_tasks_created
    );
    println!(
        "annotation_tasks_skipped: {}",
        report.annotation_tasks_skipped
    );
    println!("routed_task_note_writes: {}", report.routed_note_actions);
    println!("writes: {}", write_summary(report));
}

fn write_summary(report: SyncWriteReport) -> &'static str {
    let note_writes = report.note_action != "none"
        || report.routed_note_actions > 0
        || report.image_assets_written > 0;
    match (note_writes, report.marker_action != "none") {
        (false, false) => "none",
        (false, true) => "pdf",
        (true, false) => "note",
        (true, true) => "note,pdf",
    }
}

fn planned_routed_note_write_count(plan: &PdfSyncPlan) -> usize {
    plan.routed_task_note_writes
        .iter()
        .filter(|write| write.action != "none")
        .count()
}

fn planned_image_asset_write_count(plan: &PdfSyncPlan) -> usize {
    plan.image_assets
        .iter()
        .filter(|write| write.action == ImageAssetAction::Copy)
        .count()
}

fn status_normalization_label(
    normalization: StatusNormalization,
) -> Option<String> {
    let mut sources = Vec::new();
    if normalization.marker {
        sources.push("marker");
    }
    if normalization.frontmatter {
        sources.push("frontmatter");
    }
    if normalization.base {
        sources.push("base");
    }
    (!sources.is_empty()).then(|| {
        format!(
            "{DEPRECATED_STATUS_DONE}->{STATUS_READ} ({})",
            sources.join(",")
        )
    })
}

fn print_verbose_scan_plan_report(
    config: &Config,
    options: SyncOptions,
    pdf_count: usize,
    plan_outcomes: &[ScanPlanOutcome],
) {
    print_config_report("scan", config);
    println!("dry_run: {}", options.dry_run);
    println!("write_pdfs: {}", options.write_pdf);
    println!("ob_sync: not-run");
    println!("pdf_count: {pdf_count}");
    for outcome in plan_outcomes {
        match outcome {
            ScanPlanOutcome::Planned(plan) => print_scan_plan_entry(plan),
            ScanPlanOutcome::Failed(failure) => {
                print_scan_plan_failure_entry(failure);
            }
        }
    }
}

fn print_scan_header(
    config: &Config,
    pdf_count: usize,
    dry_run: bool,
    styler: &Styler,
) {
    let separator = styler.separator();
    let mut header = format!(
        "Scanning {} {} in {}",
        pdf_count,
        plural(pdf_count, "PDF", "PDFs"),
        display_scan_lib_dir(config)
    );
    if dry_run {
        header.push_str(&format!(" {separator} dry-run"));
    }
    println!("{}", styler.dim(&header));
    println!();
}

fn display_scan_lib_dir(config: &Config) -> String {
    config
        .lib_dir
        .strip_prefix(&config.bob_dir)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .map(display_path)
        .unwrap_or_else(|| config.lib_dir.display().to_string())
}

fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn print_concise_scan_plan_report(
    plan_outcomes: &[ScanPlanOutcome],
    plans: &[&PdfSyncPlan],
    pdf_count: usize,
    plan_failure_count: usize,
    styler: &Styler,
) {
    let lines = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => ScanLine::from_plan(plan),
            ScanPlanOutcome::Failed(failure) => {
                Some(ScanLine::from_failure(failure, false))
            }
        })
        .collect::<Vec<_>>();
    print_scan_lines(&lines, true, styler);
    println!(
        "{}",
        scan_summary_line(
            pdf_count,
            ScanCounts::from_plans(plans),
            plan_failure_count,
            "none",
            styler,
        )
    );
}

fn print_concise_scan_write_report(
    plan_outcomes: &[ScanPlanOutcome],
    write_outcomes: &[ScanWriteOutcome],
    reports: &[SyncWriteReport],
    pdf_count: usize,
    plan_failure_count: usize,
    write_failure_count: usize,
    styler: &Styler,
) {
    let mut write_index = 0usize;
    let mut lines = Vec::new();
    for outcome in plan_outcomes {
        match outcome {
            ScanPlanOutcome::Planned(plan) => {
                let write_outcome = write_outcomes.get(write_index);
                write_index += 1;
                match write_outcome {
                    Some(ScanWriteOutcome::Written(report)) => {
                        if let Some(line) = ScanLine::from_write(plan, report) {
                            lines.push(line);
                        }
                    }
                    Some(ScanWriteOutcome::Failed(failure)) => {
                        lines.push(ScanLine::from_failure(failure, true));
                    }
                    None => {}
                }
            }
            ScanPlanOutcome::Failed(failure) => {
                lines.push(ScanLine::from_failure(failure, false));
            }
        }
    }

    print_scan_lines(&lines, false, styler);
    println!(
        "{}",
        scan_summary_line(
            pdf_count,
            ScanCounts::from_reports(reports),
            plan_failure_count + write_failure_count,
            write_summary_from_reports(reports),
            styler,
        )
    );
}

fn print_scan_lines(lines: &[ScanLine], dry_run: bool, styler: &Styler) {
    let prefix_width = lines
        .iter()
        .map(|line| display_width(line.prefix_label(dry_run)))
        .max()
        .unwrap_or(0);
    let name_width = lines
        .iter()
        .map(|line| display_width(line.name()))
        .max()
        .unwrap_or(0);

    for line in lines {
        println!("{}", line.render(prefix_width, name_width, dry_run, styler));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScanLine {
    Success {
        name: String,
        action: String,
        details: Vec<String>,
    },
    Failed {
        name: String,
        message: String,
    },
}

impl ScanLine {
    fn from_plan(plan: &PdfSyncPlan) -> Option<Self> {
        scan_plan_changed(plan).then(|| Self::Success {
            name: scan_display_name(&plan.note_path),
            action: scan_change_action(
                plan.stable_note_action,
                plan.marker_write_needed,
                planned_image_asset_write_count(plan),
                planned_routed_note_write_count(plan),
                true,
            ),
            details: scan_details(plan, plan.annotation_tasks_created),
        })
    }

    fn from_write(
        plan: &PdfSyncPlan,
        report: &SyncWriteReport,
    ) -> Option<Self> {
        scan_report_changed(report).then(|| Self::Success {
            name: scan_display_name(&plan.note_path),
            action: scan_change_action(
                report.note_action,
                report.marker_action != "none",
                report.image_assets_written,
                report.routed_note_actions,
                false,
            ),
            details: scan_details(plan, report.annotation_tasks_created),
        })
    }

    fn from_failure(failure: &ScanFailure, write_failure: bool) -> Self {
        let message = if write_failure {
            format!("write failed: {}", failure.error)
        } else {
            failure.error.to_string()
        };
        Self::Failed {
            name: scan_display_name(&failure.pdf),
            message,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Success { name, .. } | Self::Failed { name, .. } => name,
        }
    }

    fn prefix_label(&self, dry_run: bool) -> &'static str {
        match self {
            Self::Success { .. } => success_prefix_label(dry_run),
            Self::Failed { .. } => "error",
        }
    }

    fn render(
        &self,
        prefix_width: usize,
        name_width: usize,
        dry_run: bool,
        styler: &Styler,
    ) -> String {
        let prefix_label = pad_right(self.prefix_label(dry_run), prefix_width);
        let prefix = match self {
            Self::Success { .. } => styler.green(&prefix_label),
            Self::Failed { .. } => styler.red(&prefix_label),
        };
        let name = styler.cyan(&pad_right(self.name(), name_width));

        match self {
            Self::Success {
                action, details, ..
            } => {
                let mut rendered = format!("  {prefix}  {name}  {action}");
                if !details.is_empty() {
                    let separator = format!(" {} ", styler.separator());
                    rendered.push_str("  ");
                    rendered.push_str(&styler.dim(&details.join(&separator)));
                }
                rendered
            }
            Self::Failed { message, .. } => {
                format!("  {prefix}  {name}  {message}")
            }
        }
    }
}

fn success_prefix_label(dry_run: bool) -> &'static str {
    if dry_run {
        "[dry-run] ok"
    } else {
        "ok"
    }
}

fn scan_plan_changed(plan: &PdfSyncPlan) -> bool {
    plan.stable_note_action != "none"
        || plan.marker_write_needed
        || planned_image_asset_write_count(plan) > 0
        || plan.annotation_tasks_created > 0
        || planned_routed_note_write_count(plan) > 0
}

fn scan_report_changed(report: &SyncWriteReport) -> bool {
    report.note_action != "none"
        || report.marker_action != "none"
        || report.image_assets_written > 0
        || report.annotation_tasks_created > 0
        || report.routed_note_actions > 0
}

fn scan_change_action(
    note_action: &str,
    marker_changed: bool,
    image_asset_count: usize,
    routed_note_count: usize,
    dry_run: bool,
) -> String {
    let mut targets = Vec::new();
    if note_action != "none" {
        targets.push("note".to_string());
    }
    if image_asset_count > 0 {
        targets.push(count_phrase(
            image_asset_count,
            "image asset",
            "image assets",
        ));
    }
    if routed_note_count > 0 {
        targets.push(
            plural(routed_note_count, "routed note", "routed notes")
                .to_string(),
        );
    }
    if marker_changed {
        targets.push("marker".to_string());
    }

    if targets.is_empty() {
        return "no changes".to_string();
    }

    let verb = match (dry_run, note_action) {
        (true, "create") => "would create",
        (true, _) => "would update",
        (false, "create") => "created",
        (false, _) => "updated",
    };
    format!("{verb} {}", targets.join(" + "))
}

fn scan_details(
    plan: &PdfSyncPlan,
    annotation_tasks_created: usize,
) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(count) = plan.rendered_highlights_count {
        details.push(count_phrase(count, "highlight", "highlights"));
    }
    if let Some(rendered) = &plan.rendered_highlights
        && rendered.image_count > 0
    {
        details.push(count_phrase(rendered.image_count, "image", "images"));
    }
    if annotation_tasks_created > 0 {
        details.push(format!(
            "+{}",
            count_phrase(annotation_tasks_created, "task", "tasks")
        ));
    }
    if plan.decision.source == SyncSource::AutoMerge {
        details.push(format!("auto-merge ({})", plan.decision.reason));
    }
    details
}

fn scan_display_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().replace(['_', '-'], " "))
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ScanCounts {
    creates: usize,
    updates: usize,
    unchanged: usize,
    marker_updates: usize,
    images: usize,
    image_assets: usize,
    annotation_tasks_created: usize,
    annotation_tasks_skipped: usize,
    routed_task_note_writes: usize,
}

impl ScanCounts {
    fn from_plans(plans: &[&PdfSyncPlan]) -> Self {
        Self {
            creates: plans
                .iter()
                .filter(|plan| plan.stable_note_action == "create")
                .count(),
            updates: plans
                .iter()
                .map(|plan| {
                    usize::from(plan.stable_note_action == "update")
                        + planned_routed_note_write_count(plan)
                })
                .sum(),
            unchanged: plans
                .iter()
                .filter(|plan| plan.stable_note_action == "none")
                .count(),
            marker_updates: plans
                .iter()
                .filter(|plan| plan.marker_write_needed)
                .count(),
            images: plans
                .iter()
                .filter_map(|plan| plan.rendered_highlights.as_ref())
                .map(|rendered| rendered.image_count)
                .sum(),
            image_assets: plans
                .iter()
                .map(|plan| planned_image_asset_write_count(plan))
                .sum(),
            annotation_tasks_created: plans
                .iter()
                .map(|plan| plan.annotation_tasks_created)
                .sum(),
            annotation_tasks_skipped: plans
                .iter()
                .map(|plan| plan.annotation_tasks_skipped)
                .sum(),
            routed_task_note_writes: plans
                .iter()
                .map(|plan| planned_routed_note_write_count(plan))
                .sum(),
        }
    }

    fn from_reports(reports: &[SyncWriteReport]) -> Self {
        Self {
            creates: reports
                .iter()
                .filter(|report| report.note_action == "create")
                .count(),
            updates: reports
                .iter()
                .map(|report| {
                    usize::from(report.note_action == "update")
                        + report.routed_note_actions
                })
                .sum(),
            unchanged: reports
                .iter()
                .filter(|report| report.note_action == "none")
                .count(),
            marker_updates: reports
                .iter()
                .filter(|report| report.marker_action != "none")
                .count(),
            images: reports.iter().map(|report| report.image_count).sum(),
            image_assets: reports
                .iter()
                .map(|report| report.image_assets_written)
                .sum(),
            annotation_tasks_created: reports
                .iter()
                .map(|report| report.annotation_tasks_created)
                .sum(),
            annotation_tasks_skipped: reports
                .iter()
                .map(|report| report.annotation_tasks_skipped)
                .sum(),
            routed_task_note_writes: reports
                .iter()
                .map(|report| report.routed_note_actions)
                .sum(),
        }
    }
}

fn scan_summary_line(
    pdf_count: usize,
    counts: ScanCounts,
    failure_count: usize,
    writes: &str,
    styler: &Styler,
) -> String {
    let separator = styler.separator();
    let mut summary = format!(
        "{} {pdf_noun} {separator} {} created {separator} {} updated {separator} {} unchanged {separator} {} {marker_noun} {separator} {} {task_noun}",
        pdf_count,
        counts.creates,
        counts.updates,
        counts.unchanged,
        counts.marker_updates,
        counts.annotation_tasks_created,
        pdf_noun = plural(pdf_count, "pdf", "pdfs"),
        marker_noun = plural(counts.marker_updates, "marker", "markers"),
        task_noun = plural(counts.annotation_tasks_created, "task", "tasks"),
    );
    if counts.images > 0 {
        summary.push_str(&format!(
            " {separator} {}",
            count_phrase(counts.images, "image", "images")
        ));
    }
    if counts.image_assets > 0 {
        summary.push_str(&format!(
            " {separator} {}",
            count_phrase(counts.image_assets, "image asset", "image assets")
        ));
    }
    if failure_count > 0 {
        summary.push_str(&format!(
            " {separator} {}",
            styler.red(&count_phrase(failure_count, "failure", "failures"))
        ));
    }
    summary.push_str(&format!(" {separator} writes: {writes}"));
    summary
}

fn write_summary_from_reports(reports: &[SyncWriteReport]) -> &'static str {
    let note_writes = reports.iter().any(|report| {
        report.note_action != "none"
            || report.routed_note_actions > 0
            || report.image_assets_written > 0
    });
    let marker_writes =
        reports.iter().any(|report| report.marker_action != "none");
    match (note_writes, marker_writes) {
        (false, false) => "none",
        (true, false) => "note",
        (false, true) => "pdf",
        (true, true) => "note,pdf",
    }
}

fn count_phrase(count: usize, singular: &str, plural_noun: &str) -> String {
    format!("{count} {}", plural(count, singular, plural_noun))
}

fn plural<'a>(
    count: usize,
    singular: &'a str,
    plural_noun: &'a str,
) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural_noun
    }
}

fn print_scan_plan_entry(plan: &PdfSyncPlan) {
    println!("pdf: {}", plan.pdf.display());
    println!("  note: {}", plan.note_path.display());
    println!(
        "  sidecar: {}",
        plan.sidecar_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    println!("  sync_source: {}", plan.decision.source.as_str());
    if plan.decision.source == SyncSource::AutoMerge {
        println!("  sync_reason: {}", plan.decision.reason);
    }
    println!("  pdf_task: {}", plan.pdf_task_signal.status.label());
    if let Some(status) = plan.pdf_task_signal.status_contributed {
        println!("  pdf_task_contribution: status={status}");
    }
    if let Some(label) = status_normalization_label(plan.status_normalization) {
        println!("  status_normalization: {label}");
    }
    println!("  note_action: {}", plan.stable_note_action);
    println!(
        "  pdf_marker_action: {}",
        if plan.marker_write_needed {
            "would-update"
        } else {
            "none"
        }
    );
    if let Some(count) = plan.rendered_highlights_count {
        println!("  highlights_count: {count}");
    }
    if let Some(rendered) = &plan.rendered_highlights
        && (rendered.image_count > 0 || !plan.image_assets.is_empty())
    {
        println!("  images: {}", rendered.image_count);
        println!("  image_assets: {}", planned_image_asset_write_count(plan));
    }
    println!(
        "  annotation_tasks_create: {}",
        plan.annotation_tasks_created
    );
    println!("  annotation_tasks_skip: {}", plan.annotation_tasks_skipped);
    println!(
        "  routed_task_note_writes: {}",
        planned_routed_note_write_count(plan)
    );
}

fn print_scan_plan_failure_entry(failure: &ScanFailure) {
    println!("pdf: {}", failure.pdf.display());
    println!("  plan_error: {}", failure.error);
}

fn print_scan_write_failure_entry(failure: &ScanFailure) {
    println!("write_failure: {}", failure.pdf.display());
    println!("  error: {}", failure.error);
}

fn print_scan_plan_summary(plans: &[&PdfSyncPlan], plan_failure_count: usize) {
    let counts = ScanCounts::from_plans(plans);
    println!("summary:");
    println!("  notes_create: {}", counts.creates);
    println!("  notes_update: {}", counts.updates);
    println!("  notes_unchanged: {}", counts.unchanged);
    if counts.images > 0 || counts.image_assets > 0 {
        println!("  images: {}", counts.images);
        println!("  image_assets: {}", counts.image_assets);
    }
    println!(
        "  annotation_tasks_create: {}",
        counts.annotation_tasks_created
    );
    println!(
        "  annotation_tasks_skip: {}",
        counts.annotation_tasks_skipped
    );
    println!(
        "  routed_task_note_writes: {}",
        counts.routed_task_note_writes
    );
    println!("  pdf_markers_would_update: {}", counts.marker_updates);
    println!("  pdfs_planned: {}", plans.len());
    println!("  plan_failures: {plan_failure_count}");
    println!("  scan_failures: {plan_failure_count}");
    if plan_failure_count > 0 {
        println!("result: partial-failure");
    }
}

fn print_scan_write_summary(
    reports: &[SyncWriteReport],
    plan_failure_count: usize,
    write_failure_count: usize,
) {
    let counts = ScanCounts::from_reports(reports);
    println!("summary:");
    println!("  notes_created: {}", counts.creates);
    println!("  notes_updated: {}", counts.updates);
    println!("  notes_unchanged: {}", counts.unchanged);
    if counts.images > 0 || counts.image_assets > 0 {
        println!("  images: {}", counts.images);
        println!("  image_assets_written: {}", counts.image_assets);
    }
    println!(
        "  annotation_tasks_created: {}",
        counts.annotation_tasks_created
    );
    println!(
        "  annotation_tasks_skipped: {}",
        counts.annotation_tasks_skipped
    );
    println!(
        "  routed_task_note_writes: {}",
        counts.routed_task_note_writes
    );
    println!("  pdf_markers_updated: {}", counts.marker_updates);
    println!("  write_successes: {}", reports.len());
    println!("  plan_failures: {plan_failure_count}");
    println!("  write_failures: {write_failure_count}");
    println!(
        "  scan_failures: {}",
        plan_failure_count + write_failure_count
    );
    if plan_failure_count + write_failure_count > 0 {
        println!("result: partial-failure");
    }
    println!("writes: {}", write_summary_from_reports(reports));
}

fn scan_partial_failure_error(
    plan_failures: &[&ScanFailure],
    write_failures: &[&ScanFailure],
) -> CommandError {
    let total = plan_failures.len() + write_failures.len();
    let mut message = format!("scan completed with {total} per-PDF failure(s)");
    if !plan_failures.is_empty() {
        message.push_str("\nplanning failures:");
        for failure in plan_failures {
            message.push_str("\n  ");
            message.push_str(&failure.pdf.display().to_string());
            message.push_str(": ");
            message.push_str(&failure.error.to_string());
        }
    }
    if !write_failures.is_empty() {
        message.push_str("\nwrite failures:");
        for failure in write_failures {
            message.push_str("\n  ");
            message.push_str(&failure.pdf.display().to_string());
            message.push_str(": ");
            message.push_str(&failure.error.to_string());
        }
    }
    CommandError::new(message)
}

fn show_marker(config: &Config, pdf: &Path) -> Result<()> {
    let marker = read_pdf_marker(pdf)?;
    let marker_input = parse_marker_with_normalization(&marker.contents)?;
    print_config_report("marker", config);
    println!("pdf: {}", pdf.display());
    println!("marker_page: {}", marker.page_number);
    println!("marker_note: {}", marker.note_number);
    if let Some(label) = status_normalization_label(StatusNormalization {
        marker: marker_input.status_normalized,
        ..StatusNormalization::default()
    }) {
        println!("status_normalization: {label}");
    }
    println!("marker_raw:");
    print!("{}", marker.contents);
    if !marker.contents.ends_with('\n') {
        println!();
    }
    println!("marker_rendered:");
    print!("{}", render_marker(&marker_input.projection)?);
    println!("writes: none");
    Ok(())
}

fn doctor_vault(config: &Config) -> Result<()> {
    print_config_report("doctor", config);
    let mut failures = Vec::new();
    let mut warnings = Vec::new();

    print_path_check("vault_path", &config.bob_dir, config.bob_dir.is_dir());
    if !config.bob_dir.is_dir() {
        failures.push(format!(
            "vault path does not exist or is not a directory: {}",
            config.bob_dir.display()
        ));
    }

    print_path_check("library_dir", &config.lib_dir, config.lib_dir.is_dir());
    if !config.lib_dir.is_dir() {
        failures.push(format!(
            "library directory does not exist or is not a directory: {}",
            config.lib_dir.display()
        ));
    }

    print_path_check("ref_dir", &config.ref_dir, config.ref_dir.is_dir());
    if !config.ref_dir.is_dir() {
        failures.push(format!(
            "reference directory does not exist or is not a directory: {}",
            config.ref_dir.display()
        ));
    }

    let pdfs = if config.lib_dir.is_dir() {
        match collect_pdf_paths(config) {
            Ok(pdfs) => pdfs,
            Err(error) => {
                failures.push(error.to_string());
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    println!("pdf_count: {}", pdfs.len());

    let mut sidecar_count = 0usize;
    let mut missing_sidecars = Vec::new();
    for pdf in &pdfs {
        match discover_sidecar_path(pdf) {
            Ok(Some(_)) => sidecar_count += 1,
            Ok(None) => missing_sidecars.push(pdf.clone()),
            Err(error) => failures.push(error.to_string()),
        }
    }
    println!("sidecars_found: {sidecar_count}");
    println!("sidecars_missing: {}", missing_sidecars.len());
    if !missing_sidecars.is_empty() {
        warnings.push(format!(
            "{} PDF(s) do not have a Highlights sidecar",
            missing_sidecars.len()
        ));
    }

    let mut readable_markers = 0usize;
    for pdf in &pdfs {
        match read_pdf_marker(pdf)
            .and_then(|marker| parse_marker(&marker.contents).map(|_| marker))
        {
            Ok(_) => readable_markers += 1,
            Err(error) => failures.push(format!("{}: {error}", pdf.display())),
        }
    }
    println!("pdf_markers_readable: {readable_markers}");
    println!(
        "pdf_marker_errors: {}",
        pdfs.len().saturating_sub(readable_markers)
    );

    match git_status(config, &[])? {
        GitStatus::MissingCommand => {
            println!("git: fail (command not found)");
            failures.push("git command not found".to_string());
        }
        GitStatus::NotWorktree => {
            println!("git: fail (vault is not a worktree)");
            failures.push(format!(
                "vault is not a Git worktree: {}",
                config.bob_dir.display()
            ));
        }
        GitStatus::Worktree { entries } if entries.is_empty() => {
            println!("git: ok (clean worktree)");
        }
        GitStatus::Worktree { entries } => {
            println!("git: fail (dirty worktree)");
            println!("git_dirty_count: {}", entries.len());
            for entry in entries.iter().take(20) {
                println!("  {}", entry.raw);
            }
            failures
                .push(format!("vault has {} dirty Git path(s)", entries.len()));
        }
    }

    match ob::load_ob_command() {
        Some(command) => {
            println!("ob: available ({})", command.to_string_lossy());
        }
        None => {
            println!("ob: warn (command not found)");
            warnings.push("ob command not found; Obsidian Sync integration is unavailable".to_string());
        }
    }
    println!("ob_sync: not-run");

    if !warnings.is_empty() {
        println!("warnings:");
        for warning in &warnings {
            println!("  {warning}");
        }
    }

    println!("writes: none");
    if failures.is_empty() {
        println!("result: ok");
        Ok(())
    } else {
        println!("result: failed");
        Err(CommandError::new(format!(
            "doctor found failing checks:\n  {}",
            failures.join("\n  ")
        )))
    }
}

fn print_path_check(name: &str, path: &Path, ok: bool) {
    println!(
        "{name}: {} ({})",
        if ok { "ok" } else { "fail" },
        path.display()
    );
}

fn collect_pdf_paths(config: &Config) -> Result<Vec<PathBuf>> {
    if !config.lib_dir.is_dir() {
        return Err(CommandError::new(format!(
            "library directory does not exist or is not a directory: {}",
            config.lib_dir.display()
        )));
    }

    let mut paths = Vec::new();
    collect_pdf_paths_from_dir(&config.lib_dir, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_pdf_paths_from_dir(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CommandError::new(format!("scan {}: {error}", directory.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            CommandError::new(format!("scan {}: {error}", directory.display()))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CommandError::new(format!("stat {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            if !should_skip_scan_dir(&path) {
                collect_pdf_paths_from_dir(&path, paths)?;
            }
        } else if file_type.is_file() && is_pdf_path(&path) {
            paths.push(path);
        }
    }

    Ok(())
}

fn should_skip_scan_dir(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == OsStr::new(".git"))
}

fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn validate_output_collisions(config: &Config, pdfs: &[PathBuf]) -> Result<()> {
    let mut by_note_path: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for pdf in pdfs {
        by_note_path
            .entry(ref_note_path(config, pdf)?)
            .or_default()
            .push(pdf.clone());
    }

    let collisions = by_note_path
        .into_iter()
        .filter(|(_, pdfs)| pdfs.len() > 1)
        .collect::<Vec<_>>();
    if collisions.is_empty() {
        return Ok(());
    }

    let mut message =
        String::from("output path collision(s) detected before writes:");
    for (note_path, pdfs) in collisions {
        message.push('\n');
        message.push_str("  ");
        message.push_str(&note_path.display().to_string());
        message.push_str(" <= ");
        message.push_str(
            &pdfs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    Err(CommandError::new(message))
}

fn validate_planned_asset_collisions(plans: &[&PdfSyncPlan]) -> Result<()> {
    let mut by_asset_path: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for plan in plans {
        for asset in &plan.image_assets {
            by_asset_path
                .entry(asset.dest_path.clone())
                .or_default()
                .push(plan.pdf.clone());
        }
    }

    let collisions = by_asset_path
        .into_iter()
        .filter(|(_, pdfs)| pdfs.len() > 1)
        .collect::<Vec<_>>();
    if collisions.is_empty() {
        return Ok(());
    }

    let mut message =
        String::from("output path collision(s) detected before writes:");
    for (asset_path, pdfs) in collisions {
        message.push('\n');
        message.push_str("  ");
        message.push_str(&asset_path.display().to_string());
        message.push_str(" <= ");
        message.push_str(
            &pdfs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    Err(CommandError::new(message))
}

fn validate_note_target(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Err(CommandError::new(format!(
            "reference note target is a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_safe_to_write<'a, I>(config: &Config, plans: I) -> Result<()>
where
    I: IntoIterator<Item = &'a PdfSyncPlan>,
{
    let plans = plans.into_iter().collect::<Vec<_>>();
    let mut touched_paths = BTreeSet::new();
    for plan in &plans {
        if note_write_planned(plan) {
            touched_paths.insert(plan.note_path.clone());
        }
        for write in &plan.routed_task_note_writes {
            if write.action != "none" {
                touched_paths.insert(write.path.clone());
            }
        }
        for write in &plan.image_assets {
            touched_paths.insert(write.dest_path.clone());
        }
        if plan.marker_write_needed {
            touched_paths.insert(plan.pdf.clone());
        }
    }

    let touched_paths = touched_paths
        .into_iter()
        .filter(|path| path.exists())
        .filter(|path| path.strip_prefix(&config.bob_dir).is_ok())
        .collect::<Vec<_>>();
    if touched_paths.is_empty() {
        return Ok(());
    }

    match git_status(config, &touched_paths)? {
        GitStatus::Worktree { entries } if !entries.is_empty() => {
            let mut refused = Vec::new();
            for entry in &entries {
                if !dirty_entry_allowed_for_plans(config, &plans, entry)? {
                    refused.push(entry.raw.clone());
                }
            }
            if refused.is_empty() {
                Ok(())
            } else {
                Err(CommandError::new(format!(
                    "refusing to modify dirty vault files:\n  {}\ncommit, stash, or clean these files before rerunning",
                    refused.join("\n  ")
                )))
            }
        }
        _ => Ok(()),
    }
}

fn note_write_planned(plan: &PdfSyncPlan) -> bool {
    plan.stable_note_action != "none" || plan.marker_write_needed
}

fn dirty_entry_allowed_for_plans(
    config: &Config,
    plans: &[&PdfSyncPlan],
    entry: &GitStatusEntry,
) -> Result<bool> {
    if entry.index_status != ' ' || entry.worktree_status != 'M' {
        return Ok(false);
    }

    let path = config.bob_dir.join(&entry.path);
    let Some(plan) = plans.iter().find(|plan| plan.note_path == path) else {
        return Ok(false);
    };
    if !note_write_planned(plan) {
        return Ok(false);
    }
    if path.strip_prefix(&config.ref_dir).is_err() {
        return Ok(false);
    }
    if !note_contents_match_plan(&path, plan.note.contents().as_deref())? {
        return Ok(false);
    }

    let Some(head_contents) = git_head_contents(config, &path)? else {
        return Ok(false);
    };
    let current_contents = fs::read_to_string(&path).map_err(|error| {
        CommandError::new(format!("read note {}: {error}", path.display()))
    })?;
    let Some(change) =
        dirty_note_allowed_change(&head_contents, &current_contents)
    else {
        return Ok(false);
    };
    if change.includes_frontmatter() && !plan.decision.frontmatter_contributed {
        return Ok(false);
    }
    Ok(true)
}

fn git_head_contents(config: &Config, path: &Path) -> Result<Option<String>> {
    let child_env = ob::child_env();
    let relative = path.strip_prefix(&config.bob_dir).unwrap_or(path);
    let Some(relative) = relative.to_str() else {
        return Ok(None);
    };
    let output = ob::git_command(&config.bob_dir, &child_env)
        .arg("show")
        .arg(format!("HEAD:{relative}"))
        .output()
        .map_err(|error| {
            CommandError::new(format!(
                "read HEAD version of {}: {error}",
                path.display()
            ))
        })?;
    if output.status.success() {
        return Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()));
    }
    Ok(None)
}

#[cfg(test)]
fn changes_confined_to_frontmatter_or_pdf_task_checkbox(
    base: &str,
    current: &str,
) -> bool {
    dirty_note_allowed_change(base, current).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirtyNoteAllowedChange {
    FrontmatterOnly,
    PdfTaskCheckboxOnly,
    FrontmatterAndPdfTaskCheckbox,
}

impl DirtyNoteAllowedChange {
    fn includes_frontmatter(self) -> bool {
        matches!(
            self,
            DirtyNoteAllowedChange::FrontmatterOnly
                | DirtyNoteAllowedChange::FrontmatterAndPdfTaskCheckbox
        )
    }
}

fn dirty_note_allowed_change(
    base: &str,
    current: &str,
) -> Option<DirtyNoteAllowedChange> {
    let (base_frontmatter, base_body) = split_frontmatter(base)?;
    let (current_frontmatter, current_body) = split_frontmatter(current)?;
    let frontmatter_changed = base_frontmatter != current_frontmatter;
    let task_checkbox_changed =
        bodies_differ_only_by_pdf_task_checkbox(&base_body, &current_body);

    match (
        frontmatter_changed,
        task_checkbox_changed,
        base_body == current_body,
    ) {
        (true, false, true) => Some(DirtyNoteAllowedChange::FrontmatterOnly),
        (false, true, false) => {
            Some(DirtyNoteAllowedChange::PdfTaskCheckboxOnly)
        }
        (true, true, false) => {
            Some(DirtyNoteAllowedChange::FrontmatterAndPdfTaskCheckbox)
        }
        _ => None,
    }
}

fn bodies_differ_only_by_pdf_task_checkbox(
    base_body: &str,
    current_body: &str,
) -> bool {
    let base_task = match parse_pdf_task_line(base_body) {
        Ok(PdfTaskLineState::Present(task_line)) => task_line,
        _ => return false,
    };
    let current_task = match parse_pdf_task_line(current_body) {
        Ok(PdfTaskLineState::Present(task_line)) => task_line,
        _ => return false,
    };
    if base_task.mark == current_task.mark {
        return false;
    }

    replace_pdf_task_checkbox_mark(base_body, current_task.mark)
        .is_ok_and(|toggled| toggled == current_body)
}

fn git_status(config: &Config, paths: &[PathBuf]) -> Result<GitStatus> {
    let child_env = ob::child_env();
    let rev_parse = ob::git_command(&config.bob_dir, &child_env)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let rev_parse = match rev_parse {
        Ok(status) if status.success() => status,
        Ok(_) => return Ok(GitStatus::NotWorktree),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(GitStatus::MissingCommand);
        }
        Err(error) => {
            return Err(CommandError::new(format!(
                "run git rev-parse in {}: {error}",
                config.bob_dir.display()
            )));
        }
    };
    let _ = rev_parse;

    let mut command = ob::git_command(&config.bob_dir, &child_env);
    command
        .arg("-c")
        .arg("color.status=false")
        .arg("status")
        .arg("--short")
        .arg("--untracked-files=all")
        .arg("--");
    for path in paths {
        let pathspec = path.strip_prefix(&config.bob_dir).unwrap_or(path);
        command.arg(pathspec);
    }
    let output = command.output().map_err(|error| {
        CommandError::new(format!(
            "run git status in {}: {error}",
            config.bob_dir.display()
        ))
    })?;
    if !output.status.success() {
        return Err(CommandError::new(format!(
            "git status failed in {}:\n{}",
            config.bob_dir.display(),
            command_output(&output)
        )));
    }

    Ok(GitStatus::Worktree {
        entries: String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_git_status_entry)
            .collect(),
    })
}

fn parse_git_status_entry(line: &str) -> Option<GitStatusEntry> {
    if line.len() < 4 {
        return None;
    }
    let mut chars = line.chars();
    let index_status = chars.next()?;
    let worktree_status = chars.next()?;
    if chars.next()? != ' ' {
        return None;
    }
    Some(GitStatusEntry {
        index_status,
        worktree_status,
        path: PathBuf::from(&line[3..]),
        raw: line.to_string(),
    })
}

fn command_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("stdout:\n{stdout}\nstderr:\n{stderr}")
}

fn is_wikilink(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("[[") && trimmed.ends_with("]]")
}

fn simple_wikilink_target(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if !(trimmed.starts_with("[[") && trimmed.ends_with("]]")) {
        return None;
    }
    let target = &trimmed[2..trimmed.len() - 2];
    if target.trim().is_empty()
        || target.trim() != target
        || target.contains('|')
        || target.contains("#^")
        || target.contains("[[")
        || target.contains("]]")
    {
        return None;
    }
    Some(target)
}

fn canonicalize_parent(
    projection: &mut Projection,
    source: &str,
) -> Result<()> {
    let Some(value) = projection.get_mut(FIELD_PARENT) else {
        return Ok(());
    };

    let canonical = match value {
        MarkerValue::String(value) | MarkerValue::Number(value) => {
            canonical_parent_target(value, source)?
        }
        MarkerValue::Bool(value) => {
            canonical_parent_target(&value.to_string(), source)?
        }
        MarkerValue::Null => return Err(empty_parent_error(source)),
        MarkerValue::List(_) => {
            return Err(CommandError::new(format!(
                "{source} parent must be a scalar note target; inline lists are not supported"
            )));
        }
    };

    *value = MarkerValue::String(canonical);
    Ok(())
}

fn canonical_parent_target(value: &str, source: &str) -> Result<String> {
    let target = value.trim();
    if target.is_empty() {
        return Err(empty_parent_error(source));
    }
    if is_wikilink(target) {
        Ok(target.to_string())
    } else {
        Ok(format!("[[{target}]]"))
    }
}

fn empty_parent_error(source: &str) -> CommandError {
    CommandError::new(format!(
        "{source} has an empty required marker key: {FIELD_PARENT}"
    ))
}

fn read_sidecar_for_pdf(pdf: &Path) -> Result<Option<SidecarInput>> {
    let Some(path) = discover_sidecar_path(pdf)? else {
        return Ok(None);
    };
    let contents = fs::read_to_string(&path).map_err(|error| {
        CommandError::new(format!("read sidecar {}: {error}", path.display()))
    })?;
    Ok(Some(SidecarInput {
        path,
        annotations: parse_sidecar_markdown(&contents),
    }))
}

fn discover_sidecar_path(pdf: &Path) -> Result<Option<PathBuf>> {
    let markdown = pdf.with_extension("md");
    if markdown.is_file() {
        return Ok(Some(markdown));
    }

    let textbundle = pdf.with_extension("textbundle");
    if !textbundle.exists() {
        return Ok(None);
    }
    if !textbundle.is_dir() {
        return Err(CommandError::new(format!(
            "unsupported sidecar {}: expected a .textbundle directory",
            textbundle.display()
        )));
    }

    for file_name in TEXTBUNDLE_TEXT_FILES {
        let text_path = textbundle.join(file_name);
        if text_path.is_file() {
            return Ok(Some(text_path));
        }
    }

    Err(CommandError::new(format!(
        "unsupported textbundle sidecar {}: expected text.md or text.markdown",
        textbundle.display()
    )))
}

fn parse_sidecar_markdown(contents: &str) -> Vec<SidecarAnnotation> {
    let mut annotations = Vec::new();
    let mut chunk = Vec::new();
    let mut page_label = None;
    let mut linked_page_style = false;
    let mut order = 0usize;
    let mut page_ordinals: BTreeMap<String, usize> = BTreeMap::new();

    for line in contents.lines() {
        if let Some(next_page_heading) = sidecar_page_heading_details(line) {
            flush_sidecar_chunk(
                &mut annotations,
                &mut chunk,
                page_label.as_deref(),
                linked_page_style,
                &mut order,
                &mut page_ordinals,
            );
            page_label = Some(next_page_heading.label);
            linked_page_style = next_page_heading.linked_page_style;
            continue;
        }

        if is_horizontal_rule(line) {
            flush_sidecar_chunk(
                &mut annotations,
                &mut chunk,
                page_label.as_deref(),
                linked_page_style,
                &mut order,
                &mut page_ordinals,
            );
            continue;
        }

        chunk.push(line.to_string());
    }

    flush_sidecar_chunk(
        &mut annotations,
        &mut chunk,
        page_label.as_deref(),
        linked_page_style,
        &mut order,
        &mut page_ordinals,
    );
    annotations
}

fn flush_sidecar_chunk(
    annotations: &mut Vec<SidecarAnnotation>,
    chunk: &mut Vec<String>,
    page_label: Option<&str>,
    linked_page_style: bool,
    order: &mut usize,
    page_ordinals: &mut BTreeMap<String, usize>,
) {
    for mut annotation in
        parse_sidecar_chunk(chunk, page_label, linked_page_style)
    {
        *order += 1;
        annotation.order = *order;
        let page_key = annotation.page_label.clone().unwrap_or_default();
        let ordinal = page_ordinals.entry(page_key).or_insert(0);
        *ordinal += 1;
        annotation.ordinal_on_page = *ordinal;
        annotations.push(annotation);
    }
    chunk.clear();
}

fn parse_sidecar_chunk(
    chunk: &[String],
    page_label: Option<&str>,
    linked_page_style: bool,
) -> Vec<SidecarAnnotation> {
    let lines = trim_blank_lines(chunk);
    if lines.is_empty() || lines.iter().all(|line| is_markdown_heading(line)) {
        return Vec::new();
    }

    if let Some(blockquote_index) = lines
        .iter()
        .position(|line| line.trim_start().starts_with('>'))
    {
        let mut quote_lines = Vec::new();
        let mut index = blockquote_index;
        let mut last_quote_line_was_blank = false;
        while index < lines.len() {
            let trimmed = lines[index].trim_start();
            if trimmed.starts_with('>') {
                let quote_line = strip_blockquote_marker(trimmed);
                last_quote_line_was_blank = quote_line.trim().is_empty();
                quote_lines.push(quote_line);
                index += 1;
                continue;
            }
            if trimmed.is_empty() && should_keep_blank_quote_line(&lines, index)
            {
                quote_lines.push(String::new());
                last_quote_line_was_blank = true;
                index += 1;
                continue;
            }
            if !last_quote_line_was_blank
                && linked_page_style
                && is_quote_continuation_line(&lines[index])
            {
                quote_lines.push(trimmed.to_string());
                last_quote_line_was_blank = false;
                index += 1;
                continue;
            }
            break;
        }

        let text = normalize_annotation_text(&quote_lines);
        if text.is_empty() {
            return Vec::new();
        }
        let comment_lines = lines[index..]
            .iter()
            .filter(|line| !is_markdown_heading(line))
            .cloned()
            .collect::<Vec<_>>();
        let comment_source = normalize_annotation_text(&comment_lines);
        let comment = strip_comment_label(&comment_source);

        return vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::Highlight,
            page_label: page_label.map(str::to_string),
            linked_page_style,
            text,
            comment: (!comment.is_empty()).then_some(comment),
            task_source: (!comment_source.is_empty())
                .then(|| strip_comment_label_only(&comment_source)),
            image: None,
            order: 0,
            ordinal_on_page: 0,
        }];
    }

    let non_heading_lines = lines
        .iter()
        .filter(|line| !is_markdown_heading(line))
        .cloned()
        .collect::<Vec<_>>();
    let image_annotations = parse_image_sidecar_annotations(
        &non_heading_lines,
        page_label,
        linked_page_style,
    );
    if !image_annotations.is_empty() {
        return image_annotations;
    }

    let note_lines = non_heading_lines
        .iter()
        .map(|line| strip_standalone_note_marker(line))
        .collect::<Vec<_>>();
    let text = normalize_annotation_text(&note_lines);
    if text.is_empty() {
        Vec::new()
    } else {
        vec![SidecarAnnotation {
            kind: SidecarAnnotationKind::StandaloneNote,
            page_label: page_label.map(str::to_string),
            linked_page_style,
            text,
            comment: None,
            task_source: None,
            image: None,
            order: 0,
            ordinal_on_page: 0,
        }]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownImage {
    target: String,
    alt_text: Option<String>,
    title: Option<String>,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingImageAnnotation {
    image: SidecarImage,
    comment_lines: Vec<String>,
}

fn parse_image_sidecar_annotations(
    lines: &[String],
    page_label: Option<&str>,
    linked_page_style: bool,
) -> Vec<SidecarAnnotation> {
    let mut images = Vec::<PendingImageAnnotation>::new();
    let mut prefix_comment_lines = Vec::<String>::new();

    for line in lines {
        let markdown_images = markdown_images_in_line(line);
        if markdown_images.is_empty() {
            push_image_comment_line(
                &mut images,
                &mut prefix_comment_lines,
                strip_standalone_note_marker(line),
            );
            continue;
        }

        let mut cursor = 0usize;
        for markdown_image in markdown_images {
            let before = line[cursor..markdown_image.start].trim();
            if !before.is_empty() {
                push_image_comment_line(
                    &mut images,
                    &mut prefix_comment_lines,
                    strip_standalone_note_marker(before),
                );
            }

            let mut pending = PendingImageAnnotation {
                image: SidecarImage {
                    target: markdown_image.target,
                    alt_text: markdown_image.alt_text,
                    title: markdown_image.title,
                },
                comment_lines: Vec::new(),
            };
            if images.is_empty() && !prefix_comment_lines.is_empty() {
                pending.comment_lines.append(&mut prefix_comment_lines);
            }
            images.push(pending);
            cursor = markdown_image.end;
        }

        let after = line[cursor..].trim();
        if !after.is_empty() {
            push_image_comment_line(
                &mut images,
                &mut prefix_comment_lines,
                strip_standalone_note_marker(after),
            );
        }
    }

    images
        .into_iter()
        .filter_map(|pending| {
            if pending.image.target.is_empty() {
                return None;
            }
            let comment_source = image_note_source(&pending);
            let comment = strip_comment_label(&comment_source);
            // Alt text is metadata only; never fall back to the asset path so
            // image targets are never rendered as user note text.
            let text = pending.image.alt_text.clone().unwrap_or_default();
            Some(SidecarAnnotation {
                kind: SidecarAnnotationKind::Image,
                page_label: page_label.map(str::to_string),
                linked_page_style,
                text,
                comment: (!comment.is_empty()).then_some(comment),
                task_source: (!comment_source.is_empty())
                    .then(|| strip_comment_label_only(&comment_source)),
                image: Some(pending.image),
                order: 0,
                ordinal_on_page: 0,
            })
        })
        .collect()
}

/// Build the combined user-authored note source for an image annotation.
///
/// Explicit sidecar lines adjacent to the image are the strongest source
/// because that is the documented "regular text below the annotation" shape.
/// Markdown image title text is treated as additional user-authored note text
/// and appended unless it merely duplicates an explicit line. Alt text is left
/// out: it is frequently a generic caption rather than a user note.
fn image_note_source(pending: &PendingImageAnnotation) -> String {
    let explicit = normalize_annotation_text(&pending.comment_lines);
    let Some(title) = pending
        .image
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    else {
        return explicit;
    };
    if explicit.is_empty() {
        return title.to_string();
    }
    // Skip a title that merely repeats text the explicit note already carries,
    // comparing against the label-stripped form so a `Comment:`-prefixed line
    // still matches a bare title.
    let already_present =
        |source: &str| source.lines().any(|line| line.trim() == title);
    if already_present(&explicit)
        || already_present(&strip_comment_label(&explicit))
    {
        return explicit;
    }
    format!("{explicit}\n{title}")
}

fn push_image_comment_line(
    images: &mut [PendingImageAnnotation],
    prefix_comment_lines: &mut Vec<String>,
    line: String,
) {
    if let Some(image) = images.last_mut() {
        image.comment_lines.push(line);
    } else {
        prefix_comment_lines.push(line);
    }
}

fn markdown_images_in_line(line: &str) -> Vec<MarkdownImage> {
    let mut images = Vec::new();
    let mut search_start = 0usize;
    while let Some(relative_start) = line[search_start..].find("![") {
        let start = search_start + relative_start;
        let alt_start = start + 2;
        let Some(label_end_relative) = line[alt_start..].find("](") else {
            break;
        };
        let label_end = alt_start + label_end_relative;
        let target_start = label_end + 2;
        let Some(close_relative) = markdown_image_close(&line[target_start..])
        else {
            break;
        };
        let target_end = target_start + close_relative;
        let end = target_end + 1;
        if let Some((target, title)) =
            markdown_image_target_and_title(&line[target_start..target_end])
        {
            let alt = line[alt_start..label_end].trim();
            images.push(MarkdownImage {
                target,
                alt_text: (!alt.is_empty()).then(|| alt.to_string()),
                title,
                start,
                end,
            });
        }
        search_start = end;
    }
    images
}

/// Find the closing `)` of a Markdown image destination, ignoring `)`
/// characters that appear inside a quoted title such as `"a (b)"`.
fn markdown_image_close(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut index = 0usize;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b')' => return Some(index),
                _ => {}
            },
        }
        index += 1;
    }
    None
}

/// Split a Markdown image destination into its target and optional title text.
///
/// Supports bare targets (`assets/file.png`), angle-bracket targets
/// (`<assets/my file.png>`), and an optional trailing title in double quotes,
/// single quotes, or parentheses (`assets/file.png "note text"`). The title is
/// where Highlights stores a note attached to an image annotation.
fn markdown_image_target_and_title(
    destination: &str,
) -> Option<(String, Option<String>)> {
    let trimmed = destination.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (target, rest) = if let Some(rest) = trimmed.strip_prefix('<') {
        let end = rest.find('>')?;
        (&rest[..end], rest[end + 1..].trim_start())
    } else if image_target_has_supported_extension(trimmed) {
        // The entire destination is the target (e.g. a path with spaces and a
        // supported extension), so there is no title to parse.
        (trimmed, "")
    } else {
        let (target, rest) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        (target, rest.trim_start())
    };

    if !image_target_has_supported_extension(target) {
        return None;
    }
    Some((target.trim().to_string(), markdown_image_title(rest)))
}

fn markdown_image_title(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let close = match rest.chars().next()? {
        '"' => '"',
        '\'' => '\'',
        '(' => ')',
        _ => return None,
    };
    let inner = &rest[1..];
    let end = inner.find(close)?;
    let title = inner[..end].trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn image_target_has_supported_extension(target: &str) -> bool {
    let clean_target = target.split(['?', '#']).next().unwrap_or(target).trim();
    let extension = Path::new(clean_target)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    matches!(
        extension.as_deref(),
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "bmp"
                | "svg"
                | "avif"
                | "heic"
        )
    )
}

fn resolve_sidecar_image_assets(
    config: &Config,
    pdf: &Path,
    ref_note_path: &Path,
    sidecar: &SidecarInput,
) -> Result<BTreeMap<usize, ImageAssetWrite>> {
    let mut assets = BTreeMap::new();
    for annotation in &sidecar.annotations {
        if annotation.kind != SidecarAnnotationKind::Image {
            continue;
        }
        let image = annotation.image.as_ref().ok_or_else(|| {
            CommandError::new("image annotation is missing image metadata")
        })?;
        let source_path =
            sidecar_image_source_path(&sidecar.path, &image.target)?;
        let source_bytes = fs::read(&source_path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CommandError::new(format!(
                    "image asset not found: {} - export the sidecar as a TextBundle so images are included",
                    image.target
                ))
            } else {
                CommandError::new(format!(
                    "read image asset {}: {error}",
                    source_path.display()
                ))
            }
        })?;
        let source_sha256 = hex::encode(Sha256::digest(&source_bytes));
        let block_id = image_annotation_block_id(config, pdf, &source_sha256);
        let extension =
            image_target_extension(&image.target).ok_or_else(|| {
                CommandError::new(format!(
                    "image asset target has no supported extension: {}",
                    image.target
                ))
            })?;
        let dest_path =
            image_asset_dest_path(ref_note_path, &block_id, &extension)?;
        let action = image_asset_action(&dest_path, &source_sha256)?;
        assets.insert(
            annotation.order,
            ImageAssetWrite {
                annotation_order: annotation.order,
                source_path,
                dest_path: dest_path.clone(),
                vault_relative_dest_path: PathBuf::from(
                    vault_relative_path_value(config, &dest_path),
                ),
                source_sha256,
                block_id,
                action,
            },
        );
    }
    Ok(assets)
}

fn sidecar_image_source_path(
    sidecar_path: &Path,
    target: &str,
) -> Result<PathBuf> {
    let filesystem_target =
        target.split(['?', '#']).next().unwrap_or(target).trim();
    let target_path = Path::new(filesystem_target);
    if target_path.is_absolute()
        || target_path.components().any(|component| {
            matches!(component, Component::Prefix(_) | Component::ParentDir)
        })
    {
        return Err(CommandError::new(format!(
            "image asset target must be relative to the sidecar: {target}"
        )));
    }
    let sidecar_dir = sidecar_path.parent().ok_or_else(|| {
        CommandError::new(format!(
            "sidecar has no parent directory: {}",
            sidecar_path.display()
        ))
    })?;
    Ok(sidecar_dir.join(target_path))
}

fn image_target_extension(target: &str) -> Option<String> {
    let clean_target = target.split(['?', '#']).next().unwrap_or(target).trim();
    Path::new(clean_target)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .filter(|extension| {
            image_target_has_supported_extension(&format!("x.{extension}"))
        })
}

fn image_asset_dest_path(
    ref_note_path: &Path,
    block_id: &str,
    extension: &str,
) -> Result<PathBuf> {
    let assets_dir = ref_note_assets_dir(ref_note_path)?;
    Ok(assets_dir.join(format!("{block_id}.{extension}")))
}

fn ref_note_assets_dir(ref_note_path: &Path) -> Result<PathBuf> {
    let stem = ref_note_path.file_stem().ok_or_else(|| {
        CommandError::new(format!(
            "reference note path has no file stem: {}",
            ref_note_path.display()
        ))
    })?;
    let mut dir_name = OsString::from(stem);
    dir_name.push(".assets");
    Ok(ref_note_path.with_file_name(dir_name))
}

fn image_asset_action(
    dest_path: &Path,
    source_sha256: &str,
) -> Result<ImageAssetAction> {
    match fs::read(dest_path) {
        Ok(bytes) => {
            let dest_sha256 = hex::encode(Sha256::digest(bytes));
            if dest_sha256 == source_sha256 {
                Ok(ImageAssetAction::None)
            } else {
                Ok(ImageAssetAction::Copy)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(ImageAssetAction::Copy)
        }
        Err(error) => Err(CommandError::new(format!(
            "read image asset destination {}: {error}",
            dest_path.display()
        ))),
    }
}

fn render_sidecar_highlights(
    config: &Config,
    pdf: &Path,
    ref_note_path: &Path,
    note: &ParsedNote,
    sidecar: &SidecarInput,
) -> Result<RenderedHighlights> {
    let existing_ids = note.generated_block_ids()?;
    let image_assets_by_order =
        resolve_sidecar_image_assets(config, pdf, ref_note_path, sidecar)?;
    let mut image_asset_writes_by_dest =
        BTreeMap::<PathBuf, ImageAssetWrite>::new();
    let mut current_ids = BTreeSet::new();
    let mut block_ids_by_annotation_order = BTreeMap::new();
    let mut rendered = String::new();
    let mut current_page = None;
    let mut skipped_marker_note = false;
    let mut image_count = 0usize;

    for image_asset in image_assets_by_order.values() {
        image_asset_writes_by_dest
            .entry(image_asset.dest_path.clone())
            .or_insert_with(|| image_asset.clone());
    }

    for annotation in &sidecar.annotations {
        if !skipped_marker_note && is_sidecar_marker_mirror(annotation) {
            skipped_marker_note = true;
            continue;
        }

        let image_asset = image_assets_by_order.get(&annotation.order);
        let block_id = match annotation.kind {
            SidecarAnnotationKind::Image => {
                image_asset.map(|asset| asset.block_id.clone()).ok_or_else(
                    || CommandError::new("image annotation was not resolved"),
                )?
            }
            SidecarAnnotationKind::Highlight
            | SidecarAnnotationKind::StandaloneNote => {
                annotation_block_id(config, pdf, annotation)
            }
        };
        block_ids_by_annotation_order
            .insert(annotation.order, block_id.clone());
        if !current_ids.insert(block_id.clone()) {
            continue;
        }
        if annotation.kind == SidecarAnnotationKind::Image {
            image_count += 1;
        }

        if annotation.page_label != current_page {
            if let Some(page_label) = &annotation.page_label {
                if !rendered.is_empty() {
                    rendered.push('\n');
                }
                rendered.push_str("### ");
                rendered.push_str(page_label);
                rendered.push_str("\n\n");
            }
            current_page = annotation.page_label.clone();
        }

        rendered.push_str(&render_annotation_block(
            config,
            ref_note_path,
            annotation,
            &block_id,
            image_asset,
        ));
    }

    let removed_ids = existing_ids
        .difference(&current_ids)
        .cloned()
        .collect::<Vec<_>>();
    if !removed_ids.is_empty() {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(REMOVED_HIGHLIGHTS_HEADING);
        rendered.push_str("\n\n");
        for block_id in removed_ids {
            push_callout_block(
                &mut rendered,
                1,
                "[!warning] Removed highlight",
                "This annotation is no longer present in the Highlights sidecar.",
            );
            rendered.push('\n');
            rendered.push('^');
            rendered.push_str(&block_id);
            rendered.push_str("\n\n");
        }
    }

    Ok(RenderedHighlights {
        content: rendered,
        count: current_ids.len(),
        image_count,
        image_assets: image_asset_writes_by_dest.into_values().collect(),
        block_ids_by_annotation_order,
    })
}

fn annotation_block_id(
    config: &Config,
    pdf: &Path,
    annotation: &SidecarAnnotation,
) -> String {
    debug_assert_ne!(annotation.kind, SidecarAnnotationKind::Image);
    let mut hasher = Sha256::new();
    hasher.update(source_pdf_value(config, pdf));
    hasher.update([0]);
    hasher.update(annotation.kind.as_str());
    hasher.update([0]);
    hasher.update(annotation.page_label.as_deref().unwrap_or_default());
    hasher.update([0]);
    hasher.update(annotation.ordinal_on_page.to_string());
    hasher.update([0]);
    hasher.update(normalized_identity_text(&annotation.text));
    let digest = hex::encode(hasher.finalize());
    format!("h-{}", &digest[..12])
}

fn image_annotation_block_id(
    config: &Config,
    pdf: &Path,
    image_sha256: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_pdf_value(config, pdf));
    hasher.update([0]);
    hasher.update(SidecarAnnotationKind::Image.as_str());
    hasher.update([0]);
    hasher.update(image_sha256);
    let digest = hex::encode(hasher.finalize());
    format!("h-{}", &digest[..12])
}

fn render_annotation_block(
    config: &Config,
    ref_note_path: &Path,
    annotation: &SidecarAnnotation,
    block_id: &str,
    image_asset: Option<&ImageAssetWrite>,
) -> String {
    let mut rendered = String::new();
    match annotation.kind {
        SidecarAnnotationKind::Highlight => {
            let text = beautify_annotation_text(&annotation.text);
            push_callout_block(&mut rendered, 1, "[!quote]", &text);
            push_annotation_comment_callout(&mut rendered, annotation);
        }
        SidecarAnnotationKind::Image => {
            let embed_path = image_asset
                .map(|asset| display_path(&asset.vault_relative_dest_path))
                .unwrap_or_else(|| {
                    vault_relative_note_link(config, ref_note_path)
                });
            let embed = format!("![[{embed_path}]]");
            push_callout_block(&mut rendered, 1, "[!quote] Image", &embed);
            push_annotation_comment_callout(&mut rendered, annotation);
        }
        SidecarAnnotationKind::StandaloneNote => {
            let text = beautify_annotation_text(&annotation.text);
            push_callout_block(&mut rendered, 1, "[!note]", &text);
        }
    }
    rendered.push('\n');
    rendered.push('^');
    rendered.push_str(block_id);
    rendered.push_str("\n\n");
    rendered
}

fn push_annotation_comment_callout(
    rendered: &mut String,
    annotation: &SidecarAnnotation,
) {
    if let Some(comment) = &annotation.comment {
        let comment_source =
            annotation.task_source.as_deref().unwrap_or(comment);
        let comment =
            strip_comment_label(&beautify_annotation_text(comment_source));
        rendered.push_str(">\n");
        push_callout_block(rendered, 2, "[!note] Comment", &comment);
    }
}

fn push_callout_block(
    rendered: &mut String,
    depth: usize,
    header: &str,
    text: &str,
) {
    debug_assert!(depth > 0);
    let prefix = blockquote_prefix(depth);
    let mut lines = text.lines();

    rendered.push_str(&prefix);
    rendered.push_str(header);
    if let Some(first_line) = lines.next()
        && !first_line.is_empty()
    {
        rendered.push(' ');
        rendered.push_str(first_line);
    }
    rendered.push('\n');

    for line in lines {
        push_prefixed_blockquote_line(rendered, &prefix, line);
    }
}

fn blockquote_prefix(depth: usize) -> String {
    let mut prefix = String::new();
    for level in 0..depth {
        if level > 0 {
            prefix.push(' ');
        }
        prefix.push('>');
    }
    prefix.push(' ');
    prefix
}

fn push_prefixed_blockquote_line(
    rendered: &mut String,
    prefix: &str,
    line: &str,
) {
    if line.is_empty() {
        rendered.push_str(prefix.trim_end());
    } else {
        rendered.push_str(prefix);
        rendered.push_str(line);
    }
    rendered.push('\n');
}

impl SidecarAnnotationKind {
    fn as_str(self) -> &'static str {
        match self {
            SidecarAnnotationKind::Highlight => "highlight",
            SidecarAnnotationKind::Image => "image",
            SidecarAnnotationKind::StandaloneNote => "note",
        }
    }
}

#[cfg(test)]
fn sidecar_page_heading(line: &str) -> Option<String> {
    sidecar_page_heading_details(line).map(|heading| heading.label)
}

fn sidecar_page_heading_details(line: &str) -> Option<SidecarPageHeading> {
    let trimmed = line.trim();
    if !trimmed.starts_with('#') {
        return None;
    }
    let heading = trimmed.trim_start_matches('#').trim();
    let (label, linked_page_style) = match markdown_link_label(heading) {
        Some(label) => (label, true),
        None => (heading, false),
    };
    if is_sidecar_page_label(label) {
        Some(SidecarPageHeading {
            label: label.to_string(),
            linked_page_style,
        })
    } else {
        None
    }
}

fn markdown_link_label(text: &str) -> Option<&str> {
    if !text.starts_with('[') {
        return None;
    }
    let (label, destination) = text[1..].split_once("](")?;
    destination.ends_with(')').then_some(label)
}

fn is_sidecar_page_label(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("page ")
        || lower.starts_with("page:")
        || lower.starts_with("p. ")
        || lower.starts_with("p ")
}

fn is_sidecar_marker_mirror(annotation: &SidecarAnnotation) -> bool {
    if annotation.kind == SidecarAnnotationKind::StandaloneNote {
        return true;
    }

    if annotation.kind != SidecarAnnotationKind::Highlight {
        return false;
    }
    if !annotation.linked_page_style {
        return false;
    }

    let Some(comment) = &annotation.comment else {
        return false;
    };
    parse_marker(comment).is_ok()
}

fn is_quote_continuation_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && !is_markdown_heading(line)
        && !is_comment_label_line(trimmed)
        && !is_marker_list_line(trimmed)
}

fn is_comment_label_line(line: &str) -> bool {
    ["Comment:", "comment:", "Note:", "note:"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn is_marker_list_line(line: &str) -> bool {
    let Some(item) =
        line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
    else {
        return false;
    };
    let Some((key, _)) = item.split_once(':') else {
        return false;
    };
    !normalize_key(key).is_empty()
}

fn should_keep_blank_quote_line(lines: &[String], index: usize) -> bool {
    lines[index + 1..]
        .iter()
        .any(|line| line.trim_start().starts_with('>'))
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    trimmed.chars().all(|character| character == '-')
        || trimmed.chars().all(|character| character == '*')
        || trimmed.chars().all(|character| character == '_')
}

fn trim_blank_lines(lines: &[String]) -> Vec<String> {
    let mut start = 0usize;
    let mut end = lines.len();
    while start < end && lines[start].trim().is_empty() {
        start += 1;
    }
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    lines[start..end].to_vec()
}

fn is_markdown_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hash_count = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (1..=6).contains(&hash_count)
        && trimmed
            .as_bytes()
            .get(hash_count)
            .is_some_and(u8::is_ascii_whitespace)
}

fn strip_blockquote_marker(line: &str) -> String {
    line.trim_start()
        .strip_prefix('>')
        .unwrap_or(line)
        .strip_prefix(' ')
        .unwrap_or_else(|| {
            line.trim_start()
                .strip_prefix('>')
                .expect("line starts with blockquote marker")
        })
        .to_string()
}

fn strip_standalone_note_marker(line: &str) -> String {
    let trimmed = line.trim_start();
    for prefix in ["Note:", "note:", "[note]", "[Note]"] {
        if let Some(value) = trimmed.strip_prefix(prefix) {
            return value.trim_start().to_string();
        }
    }
    line.to_string()
}

fn strip_comment_label(text: &str) -> String {
    strip_comment_list_markers(&strip_comment_label_only(text))
}

fn strip_comment_label_only(text: &str) -> String {
    let trimmed = text.trim();
    for prefix in ["Comment:", "comment:", "Note:", "note:"] {
        if let Some(value) = trimmed.strip_prefix(prefix) {
            return value.trim_start().to_string();
        }
    }
    text.to_string()
}

fn strip_comment_list_markers(text: &str) -> String {
    if parse_marker(text).is_ok() {
        return text.to_string();
    }

    let mut saw_list_item = false;
    let mut stripped_lines = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            stripped_lines.push(String::new());
            continue;
        }

        let Some(item) = strip_unordered_list_marker(line) else {
            return text.to_string();
        };
        saw_list_item = true;
        stripped_lines.push(item.to_string());
    }

    if saw_list_item {
        stripped_lines.join("\n")
    } else {
        text.to_string()
    }
}

fn strip_unordered_list_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let bytes = trimmed.as_bytes();
    if !bytes
        .first()
        .is_some_and(|byte| matches!(byte, b'-' | b'*' | b'+'))
    {
        return None;
    }
    if !bytes.get(1).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }

    let mut index = 2usize;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    Some(&trimmed[index..])
}

fn normalize_annotation_text(lines: &[String]) -> String {
    let mut normalized = lines
        .iter()
        .map(|line| line.trim_end().to_string())
        .collect::<Vec<_>>();
    while normalized
        .first()
        .is_some_and(|line| line.trim().is_empty())
    {
        normalized.remove(0);
    }
    while normalized.last().is_some_and(|line| line.trim().is_empty()) {
        normalized.pop();
    }
    normalized.join("\n")
}

struct ReflowLine {
    text: String,
    ended_with_soft_hyphen: bool,
}

fn clean_pdf_text_artifacts(text: &str) -> String {
    text.split('\n')
        .map(clean_pdf_text_artifacts_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean_pdf_text_artifacts_line(line: &str) -> String {
    let mut cleaned = String::new();
    let mut pending_space = false;
    for character in line.chars() {
        match character {
            '\u{fb00}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "ff")
            }
            '\u{fb01}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "fi")
            }
            '\u{fb02}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "fl")
            }
            '\u{fb03}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "ffi")
            }
            '\u{fb04}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "ffl")
            }
            '\u{fb05}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "ft")
            }
            '\u{fb06}' => {
                push_cleaned_fragment(&mut cleaned, &mut pending_space, "st")
            }
            '\u{00ad}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}'
            | '\u{feff}' => {}
            ' ' | '\t' | '\r' | '\u{00a0}' | '\u{2007}' | '\u{202f}' => {
                pending_space = true
            }
            character if character.is_whitespace() => pending_space = true,
            character => {
                if pending_space && !cleaned.is_empty() {
                    cleaned.push(' ');
                }
                pending_space = false;
                cleaned.push(character);
            }
        }
    }
    cleaned
}

fn push_cleaned_fragment(
    cleaned: &mut String,
    pending_space: &mut bool,
    fragment: &str,
) {
    if *pending_space && !cleaned.is_empty() {
        cleaned.push(' ');
    }
    *pending_space = false;
    cleaned.push_str(fragment);
}

fn beautify_annotation_text(text: &str) -> String {
    let cleaned_text = clean_pdf_text_artifacts(text);
    let mut rendered = Vec::new();
    let mut current = None::<ReflowLine>;

    for (raw_line, line) in text.split('\n').zip(cleaned_text.split('\n')) {
        let line = line.trim().to_string();
        let ended_with_soft_hyphen = ends_with_soft_hyphen(raw_line);

        if line.is_empty() {
            flush_reflow_line(&mut rendered, &mut current);
            if !rendered.is_empty()
                && !rendered.last().is_some_and(String::is_empty)
            {
                rendered.push(String::new());
            }
            continue;
        }

        if is_markdown_unordered_list_line(&line) {
            flush_reflow_line(&mut rendered, &mut current);
            current = Some(ReflowLine {
                text: line,
                ended_with_soft_hyphen,
            });
            continue;
        }

        if let Some(current) = &mut current {
            append_reflow_fragment(
                &mut current.text,
                &line,
                current.ended_with_soft_hyphen,
            );
            current.ended_with_soft_hyphen = ended_with_soft_hyphen;
        } else {
            current = Some(ReflowLine {
                text: line,
                ended_with_soft_hyphen,
            });
        }
    }

    flush_reflow_line(&mut rendered, &mut current);
    while rendered.last().is_some_and(String::is_empty) {
        rendered.pop();
    }
    rendered.join("\n")
}

fn flush_reflow_line(
    rendered: &mut Vec<String>,
    current: &mut Option<ReflowLine>,
) {
    if let Some(line) = current.take() {
        rendered.push(line.text);
    }
}

fn append_reflow_fragment(
    current: &mut String,
    fragment: &str,
    previous_ended_with_soft_hyphen: bool,
) {
    if previous_ended_with_soft_hyphen {
        current.push_str(fragment);
        return;
    }

    let mut current_chars = current.chars().rev();
    if let Some(last) = current_chars.next()
        && matches!(last, '-' | '‐')
        && current_chars.next().is_some_and(char::is_alphabetic)
        && let Some(first) = fragment.chars().next()
    {
        if first.is_lowercase() {
            current.pop();
            current.push_str(fragment);
            return;
        }
        if first.is_uppercase() || first.is_ascii_digit() {
            current.push_str(fragment);
            return;
        }
    }

    if !current.is_empty() {
        current.push(' ');
    }
    current.push_str(fragment);
}

fn ends_with_soft_hyphen(line: &str) -> bool {
    line.trim_end_matches(|character| {
        matches!(
            character,
            ' ' | '\t' | '\r' | '\u{00a0}' | '\u{2007}' | '\u{202f}'
        )
    })
    .ends_with('\u{00ad}')
}

fn is_markdown_unordered_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let mut characters = trimmed.chars();
    matches!(characters.next(), Some('-' | '*' | '+'))
        && characters.next().is_some_and(char::is_whitespace)
}

fn normalized_identity_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn annotation_task_candidates(
    config: &Config,
    ref_note_path: &Path,
    pdf: &Path,
    sidecar: Option<&SidecarInput>,
    rendered_highlights: Option<&RenderedHighlights>,
) -> Result<Vec<AnnotationTaskCandidate>> {
    let Some(sidecar) = sidecar else {
        return Ok(Vec::new());
    };

    let mut skipped_marker_note = false;
    let mut candidates = Vec::new();
    for annotation in &sidecar.annotations {
        if !skipped_marker_note && is_sidecar_marker_mirror(annotation) {
            skipped_marker_note = true;
            continue;
        }

        match annotation.kind {
            SidecarAnnotationKind::Highlight => {
                let Some(source) = annotation.task_source.as_deref() else {
                    continue;
                };
                let block_id = annotation_block_id(config, pdf, annotation);
                candidates.extend(annotation_task_candidates_from_text(
                    config,
                    ref_note_path,
                    source,
                    &block_id,
                )?);
            }
            SidecarAnnotationKind::Image => {
                let Some(source) = annotation.task_source.as_deref() else {
                    continue;
                };
                let block_id = rendered_highlights
                    .and_then(|rendered| {
                        rendered
                            .block_ids_by_annotation_order
                            .get(&annotation.order)
                    })
                    .ok_or_else(|| {
                        CommandError::new(
                            "image annotation task source was not rendered",
                        )
                    })?;
                candidates.extend(annotation_task_candidates_from_text(
                    config,
                    ref_note_path,
                    source,
                    block_id,
                )?);
            }
            SidecarAnnotationKind::StandaloneNote => {
                let block_id = annotation_block_id(config, pdf, annotation);
                candidates.extend(annotation_task_candidates_from_text(
                    config,
                    ref_note_path,
                    &annotation.text,
                    &block_id,
                )?);
            }
        }
    }
    Ok(candidates)
}

fn annotation_task_candidates_from_text(
    config: &Config,
    ref_note_path: &Path,
    text: &str,
    source_block_id: &str,
) -> Result<Vec<AnnotationTaskCandidate>> {
    let mut candidates = Vec::new();
    for line in text.lines() {
        if let Some(candidate) = annotation_task_candidate_from_source_line(
            config,
            ref_note_path,
            line,
            source_block_id,
        )? {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

fn annotation_task_candidate_from_source_line(
    config: &Config,
    ref_note_path: &Path,
    line: &str,
    source_block_id: &str,
) -> Result<Option<AnnotationTaskCandidate>> {
    let Some(source) = annotation_task_source_from_source_line(line) else {
        return Ok(None);
    };
    let target = match source.route_name {
        Some(route_name) => AnnotationTaskTarget::RoutedNote(
            route_name_to_note_path(config, &route_name)?,
        ),
        None => AnnotationTaskTarget::ReferenceNote,
    };
    let processed_id = annotation_task_processed_id(
        config,
        ref_note_path,
        source_block_id,
        &source.identity,
    );

    Ok(Some(AnnotationTaskCandidate {
        identity: source.identity,
        task_text: source.task_text,
        source_block_id: source_block_id.to_string(),
        target,
        source_ref_note_path: ref_note_path.to_path_buf(),
        processed_id,
    }))
}

fn annotation_task_source_from_source_line(
    line: &str,
) -> Option<AnnotationTaskSource> {
    let item = strip_unordered_list_marker(line)?;
    annotation_task_source_from_item(item)
}

fn annotation_task_source_from_item(
    item: &str,
) -> Option<AnnotationTaskSource> {
    let task_body = strip_optional_markdown_task_checkbox(item);
    let task_text_with_route = normalized_identity_text(
        &strip_created_task_properties(task_body.trim()),
    );
    let (task_text, route_name) =
        split_annotation_task_route_suffix(&task_text_with_route);
    if !contains_markdown_token(&task_text, PDF_TASK_TAG) {
        return None;
    }
    let identity = annotation_task_identity(&task_text)?;
    Some(AnnotationTaskSource {
        identity,
        task_text,
        route_name,
    })
}

fn split_annotation_task_route_suffix(
    task_text: &str,
) -> (String, Option<String>) {
    let trimmed = task_text.trim();
    let Some((separator_index, separator)) = trimmed
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
    else {
        return (trimmed.to_string(), None);
    };
    let token_start = separator_index + separator.len_utf8();
    let before = &trimmed[..separator_index];
    let token = &trimmed[token_start..];
    let Some(route_name) = strict_annotation_task_route_name(token) else {
        return (trimmed.to_string(), None);
    };
    (before.trim_end().to_string(), Some(route_name))
}

fn strict_annotation_task_route_name(token: &str) -> Option<String> {
    let name = token.strip_prefix('@')?;
    let mut characters = name.chars();
    let first = characters.next()?;
    if !first.is_ascii_alphanumeric() {
        return None;
    }
    characters
        .all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
        })
        .then(|| name.to_string())
}

fn route_name_to_note_path(
    config: &Config,
    route_name: &str,
) -> Result<PathBuf> {
    let path = config.bob_dir.join(format!("{route_name}.md"));
    if path.is_dir() {
        return Err(CommandError::new(format!(
            "routed annotation task target is a directory: {}; create a root-level note named {route_name}.md first",
            path.display()
        )));
    }
    if !path.is_file() {
        return Err(CommandError::new(format!(
            "routed annotation task target does not exist: {}; create a root-level note named {route_name}.md first",
            path.display()
        )));
    }
    Ok(path)
}

fn annotation_task_legacy_source_task_block_id(
    candidate: &AnnotationTaskCandidate,
) -> String {
    format!(
        "{}{}",
        SOURCE_TASK_BLOCK_ID_PREFIX,
        &candidate.processed_id[..12]
    )
}

fn annotation_task_processed_id(
    config: &Config,
    ref_note_path: &Path,
    source_block_id: &str,
    identity: &str,
) -> String {
    annotation_task_source_digest(
        config,
        ref_note_path,
        source_block_id,
        identity,
    )
}

fn annotation_task_source_digest(
    config: &Config,
    ref_note_path: &Path,
    source_block_id: &str,
    identity: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(HIGHLIGHT_TASK_ID_VERSION);
    hasher.update([0]);
    hasher.update(vault_relative_path_value(config, ref_note_path));
    hasher.update([0]);
    hasher.update(source_block_id);
    hasher.update([0]);
    hasher.update(identity);
    hex::encode(hasher.finalize())
}

fn render_annotation_task_line(
    config: &Config,
    candidate: &AnnotationTaskCandidate,
    created_date: &str,
) -> String {
    let source_link = annotation_task_source_link(config, candidate);
    format!(
        "- [ ] {} {}[{}:: {}] [created::{}]",
        candidate.task_text,
        source_link,
        HIGHLIGHT_TASK_FIELD,
        candidate.processed_id,
        created_date
    )
}

fn annotation_task_source_link(
    config: &Config,
    candidate: &AnnotationTaskCandidate,
) -> String {
    if candidate.source_block_id.is_empty() {
        return String::new();
    }

    let target = match candidate.target {
        AnnotationTaskTarget::ReferenceNote => {
            format!("#^{}", candidate.source_block_id)
        }
        AnnotationTaskTarget::RoutedNote(_) => format!(
            "{}#^{}",
            vault_relative_note_link(config, &candidate.source_ref_note_path),
            candidate.source_block_id
        ),
    };

    format!("[[{target}|{SOURCE_LINK_ALIAS}]] ")
}

fn vault_relative_note_link(config: &Config, note_path: &Path) -> String {
    let mut link = vault_relative_path_value(config, note_path);
    if let Some(stripped) = link.strip_suffix(".md") {
        link = stripped.to_string();
    }
    link
}

fn strip_optional_markdown_task_checkbox(item: &str) -> &str {
    strip_markdown_task_checkbox(item).unwrap_or_else(|| item.trim_start())
}

fn strip_markdown_task_checkbox(item: &str) -> Option<&str> {
    let trimmed = item.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'[' || bytes[2] != b']' {
        return None;
    }
    if bytes.get(3).is_some_and(|byte| !byte.is_ascii_whitespace()) {
        return None;
    }
    Some(trimmed[3..].trim_start())
}

fn annotation_task_identity(task_text: &str) -> Option<String> {
    let without_link = strip_source_block_link(task_text);
    let without_properties = strip_obsidian_task_properties(&without_link);
    let (without_route, _) =
        split_annotation_task_route_suffix(&without_properties);
    let identity = normalized_identity_text(&without_route);
    contains_markdown_token(&identity, PDF_TASK_TAG).then_some(identity)
}

/// Removes any `[[ ... ]]` wikilink whose target contains a block reference
/// (`#^`), covering annotation-level `h-...` links and task-specific `ht-...`
/// links in both same-file and full-note forms. PDF wikilinks
/// (`[[lib/example.pdf]]`) have no `#^` and are left untouched, so identity
/// stays stable across the link being injected into created task lines.
fn strip_source_block_link(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(start) = remaining.find("[[") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("]]") else {
            break;
        };
        let inside = &after_start[..end];
        if inside.contains("#^") {
            stripped.push_str(&remaining[..start]);
            remaining = &after_start[end + 2..];
            continue;
        }

        let keep_to = start + 2;
        stripped.push_str(&remaining[..keep_to]);
        remaining = &remaining[keep_to..];
    }

    stripped.push_str(remaining);
    stripped
}

#[cfg(test)]
fn existing_annotation_task_identities(body: &str) -> BTreeSet<String> {
    body.lines()
        .filter_map(existing_annotation_task_identity)
        .collect()
}

#[cfg(test)]
fn existing_annotation_task_identity(line: &str) -> Option<String> {
    if line.as_bytes().first().is_some_and(u8::is_ascii_whitespace) {
        return None;
    }

    let item = strip_unordered_list_marker(line)?;
    let task_body = strip_markdown_task_checkbox(item)?;
    if contains_markdown_token(task_body, PDF_TASK_BLOCK_ID) {
        return None;
    }
    annotation_task_identity(task_body)
}

fn processed_task_index(config: &Config) -> Result<ProcessedTaskIndex> {
    let mut index = ProcessedTaskIndex::default();
    if !config.bob_dir.is_dir() {
        return Ok(index);
    }
    collect_processed_task_index_from_dir(&config.bob_dir, &mut index)?;
    Ok(index)
}

fn collect_processed_task_index_from_dir(
    directory: &Path,
    index: &mut ProcessedTaskIndex,
) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CommandError::new(format!("scan {}: {error}", directory.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            CommandError::new(format!("scan {}: {error}", directory.display()))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CommandError::new(format!("stat {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            if !is_hidden_path_component(&path) {
                collect_processed_task_index_from_dir(&path, index)?;
            }
        } else if file_type.is_file() && is_markdown_note_path(&path) {
            collect_processed_task_index_from_file(&path, index)?;
        }
    }

    Ok(())
}

fn collect_processed_task_index_from_file(
    path: &Path,
    index: &mut ProcessedTaskIndex,
) -> Result<()> {
    let contents = fs::read_to_string(path).map_err(|error| {
        CommandError::new(format!(
            "read Markdown task index {}: {error}",
            path.display()
        ))
    })?;
    for line in contents.lines() {
        let Some(task_body) = markdown_task_body(line) else {
            continue;
        };
        for source_task_anchor in source_task_block_ids(task_body) {
            index.legacy_source_task_anchors.insert(source_task_anchor);
        }
        for processed_id in
            obsidian_task_property_values(task_body, HIGHLIGHT_TASK_FIELD)
                .into_iter()
                .chain(obsidian_task_property_values(
                    task_body,
                    LEGACY_HIGHLIGHT_TASK_FIELD,
                ))
        {
            index.processed_ids.insert(processed_id);
        }
        if let Some(identity) = annotation_task_identity(task_body) {
            index.legacy_identities.insert(identity);
        }
    }
    Ok(())
}

fn is_hidden_path_component(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with('.'))
}

fn is_markdown_note_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn markdown_task_body(line: &str) -> Option<&str> {
    let item = strip_unordered_list_marker(line)?;
    strip_markdown_task_checkbox(item)
}

fn source_task_block_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("[[") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("]]") else {
            break;
        };
        let inside = &after_start[..end];
        let target =
            inside.split_once('|').map_or(inside, |(target, _)| target);
        if let Some((_, block_id)) = target.rsplit_once("#^") {
            let block_id = block_id.trim();
            if block_id.starts_with(SOURCE_TASK_BLOCK_ID_PREFIX)
                && is_valid_block_id(block_id)
            {
                ids.push(block_id.to_string());
            }
        }

        remaining = &after_start[end + 2..];
    }

    ids
}

fn obsidian_task_property_values(text: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find('[') {
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find(']') else {
            break;
        };
        let inside = &after_start[..end];
        if let Some((property_key, value)) = obsidian_task_property(inside)
            && property_key.eq_ignore_ascii_case(key)
        {
            let value = value.trim();
            if !value.is_empty() {
                values.push(value.to_string());
            }
        }

        remaining = &after_start[end + 1..];
    }

    values
}

fn obsidian_task_property(inside_brackets: &str) -> Option<(&str, &str)> {
    let (key, value) = inside_brackets.split_once("::")?;
    let key = key.trim();
    (!key.is_empty()
        && key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
        }))
    .then_some((key, value))
}

fn strip_created_task_properties(text: &str) -> String {
    strip_matching_obsidian_task_properties(text, |key| {
        key.eq_ignore_ascii_case("created")
    })
}

fn strip_obsidian_task_properties(text: &str) -> String {
    strip_matching_obsidian_task_properties(text, |_| true)
}

fn strip_matching_obsidian_task_properties(
    text: &str,
    should_strip: impl Fn(&str) -> bool,
) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(start) = remaining.find('[') {
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find(']') else {
            break;
        };
        let inside = &after_start[..end];
        if let Some(key) = obsidian_task_property_key(inside)
            && should_strip(key)
        {
            stripped.push_str(&remaining[..start]);
            remaining = &after_start[end + 1..];
            continue;
        }

        let bracket_end = start + 1;
        stripped.push_str(&remaining[..bracket_end]);
        remaining = &remaining[bracket_end..];
    }

    stripped.push_str(remaining);
    stripped
}

fn obsidian_task_property_key(inside_brackets: &str) -> Option<&str> {
    obsidian_task_property(inside_brackets).map(|(key, _)| key)
}

fn parse_pdf_task_line(body: &str) -> Result<PdfTaskLineState> {
    let mut found = None;
    let mut task_block_line_count = 0usize;

    for (line_index, line) in body.lines().enumerate() {
        if !contains_markdown_token(line, PDF_TASK_BLOCK_ID) {
            continue;
        }

        task_block_line_count += 1;
        if task_block_line_count > 1 {
            return Err(CommandError::new(
                "reference note has multiple generated PDF task lines with ^ref block ID",
            ));
        }

        let Some((checkbox_mark_index, checked, mark)) =
            parse_markdown_task_checkbox(line)
        else {
            return Err(malformed_pdf_task_line_error(line_index));
        };
        if !contains_markdown_token(line, PDF_TASK_TAG)
            || !contains_pdf_wikilink(line)
        {
            return Err(malformed_pdf_task_line_error(line_index));
        }

        found = Some(PdfTaskLine {
            line_index,
            checkbox_mark_index,
            checked,
            mark,
        });
    }

    Ok(found
        .map(PdfTaskLineState::Present)
        .unwrap_or(PdfTaskLineState::Missing))
}

fn malformed_pdf_task_line_error(line_index: usize) -> CommandError {
    CommandError::new(format!(
        "generated PDF task line on line {} is malformed; expected a generated task such as '- [ ] #task [[...pdf]] [p::2] ^ref', '- [x] #task [[...pdf]] [p::2] ^ref', or '- [-] #task [[...pdf]] [p::2] ^ref'; legacy generated lines without [p::2] are still accepted",
        line_index + 1
    ))
}

fn contains_markdown_token(line: &str, token: &str) -> bool {
    line.split_whitespace().any(|word| word == token)
}

fn parse_markdown_task_checkbox(line: &str) -> Option<(usize, bool, char)> {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }

    if !bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b'-' | b'*' | b'+'))
    {
        return None;
    }
    index += 1;
    if !bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }

    if bytes.get(index) != Some(&b'[') || bytes.get(index + 2) != Some(&b']') {
        return None;
    }
    let mark_index = index + 1;
    let mark = *bytes.get(mark_index)? as char;
    match mark {
        ' ' => Some((mark_index, false, mark)),
        'x' | 'X' => Some((mark_index, true, mark)),
        '-' => Some((mark_index, false, mark)),
        _ => None,
    }
}

fn contains_pdf_wikilink(line: &str) -> bool {
    let mut remaining = line;
    while let Some(start) = remaining.find("[[") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("]]") else {
            return false;
        };
        let target = after_start[..end]
            .split('|')
            .next()
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        if target.to_ascii_lowercase().ends_with(".pdf") {
            return true;
        }
        remaining = &after_start[end + 2..];
    }
    false
}

fn apply_pdf_task_status_signal(
    resolution: &mut SyncResolution,
    task_line: &PdfTaskLineState,
    base_projection: Option<&Projection>,
    marker_projection: &Projection,
    frontmatter_projection: &Projection,
) -> Result<PdfTaskStatusSignal> {
    let status = task_line.status();
    let mut signal = PdfTaskStatusSignal {
        status,
        status_contributed: None,
    };
    let Some(target_status) = status.target_status() else {
        return Ok(signal);
    };
    if projection_status_is(&resolution.projection, target_status) {
        return Ok(signal);
    }

    let conflicts = pdf_task_status_conflicts(
        base_projection,
        marker_projection,
        frontmatter_projection,
        target_status,
    );
    if !conflicts.is_empty() {
        return Err(pdf_task_status_conflict_error(
            status,
            target_status,
            &conflicts,
        ));
    }

    resolution.projection.insert(
        FIELD_STATUS.to_string(),
        MarkerValue::String(target_status.to_string()),
    );
    resolution.decision.frontmatter_contributed = true;
    if !resolution.decision.reason.is_empty() {
        resolution.decision.reason.push_str("; ");
    }
    if let Some(reason) = status.contribution_reason() {
        resolution.decision.reason.push_str(reason);
    }
    signal.status_contributed = Some(target_status);
    Ok(signal)
}

#[derive(Debug, Clone)]
struct PdfTaskStatusConflict {
    source: &'static str,
    base: Option<MarkerValue>,
    value: Option<MarkerValue>,
}

fn pdf_task_status_conflicts(
    base_projection: Option<&Projection>,
    marker_projection: &Projection,
    frontmatter_projection: &Projection,
    target_status: &str,
) -> Vec<PdfTaskStatusConflict> {
    let Some(base_projection) = base_projection else {
        return Vec::new();
    };

    let mut conflicts = Vec::new();
    for (source, projection) in [
        ("marker", marker_projection),
        ("frontmatter", frontmatter_projection),
    ] {
        let base = base_projection.get(FIELD_STATUS);
        let value = projection.get(FIELD_STATUS);
        if value != base && !status_value_is(value, target_status) {
            conflicts.push(PdfTaskStatusConflict {
                source,
                base: base.cloned(),
                value: value.cloned(),
            });
        }
    }
    conflicts
}

fn pdf_task_status_conflict_error(
    status: PdfTaskStatus,
    target_status: &str,
    conflicts: &[PdfTaskStatusConflict],
) -> CommandError {
    let mut message = format!(
        "{} PDF task conflicts with marker/frontmatter status edit:",
        status.label()
    );
    for conflict in conflicts.iter().take(4) {
        message.push_str(&format!(
            "\n  {} status={}, base={}",
            conflict.source,
            diagnostic_projection_value(conflict.value.as_ref()),
            diagnostic_projection_value(conflict.base.as_ref()),
        ));
    }
    message.push_str(&format!(
        "\n{} the PDF task or set the marker/frontmatter status to {target_status}",
        status.conflict_action()
    ));
    CommandError::new(message)
}

fn projection_status_is(projection: &Projection, status: &str) -> bool {
    status_value_is(projection.get(FIELD_STATUS), status)
}

fn projection_pdf_task_mark(projection: &Projection) -> char {
    match projection
        .get(FIELD_STATUS)
        .and_then(MarkerValue::as_string)
    {
        Some(STATUS_READ) => 'x',
        Some(STATUS_ABANDONED) => '-',
        _ => ' ',
    }
}

fn status_value_is(value: Option<&MarkerValue>, status: &str) -> bool {
    value.and_then(MarkerValue::as_string) == Some(status)
}

fn resolve_sync_projection(inputs: SyncInputs<'_>) -> Result<SyncResolution> {
    let SyncInputs {
        last_hash,
        base_projection,
        marker_projection,
        marker_hash,
        frontmatter_projection,
        frontmatter_hash,
        note_exists,
        prefer,
    } = inputs;

    let Some(last_hash) = last_hash else {
        return Ok(match prefer {
            Some(Prefer::Frontmatter) if note_exists => sync_resolution(
                SyncSource::Frontmatter,
                "initial sync; --prefer frontmatter supplied",
                frontmatter_projection.clone(),
                false,
                true,
            ),
            _ => sync_resolution(
                SyncSource::Marker,
                "initial sync",
                marker_projection.clone(),
                true,
                false,
            ),
        });
    };

    let marker_changed = marker_hash != last_hash;
    let frontmatter_changed = frontmatter_hash != last_hash;

    match (marker_changed, frontmatter_changed) {
        (false, false) => Ok(sync_resolution(
            SyncSource::Marker,
            "marker and frontmatter match the stored hash",
            marker_projection.clone(),
            false,
            false,
        )),
        (true, false) => Ok(sync_resolution(
            SyncSource::Marker,
            "marker changed since last sync",
            marker_projection.clone(),
            true,
            false,
        )),
        (false, true) => Ok(sync_resolution(
            SyncSource::Frontmatter,
            "frontmatter changed since last sync",
            frontmatter_projection.clone(),
            false,
            true,
        )),
        (true, true) if marker_hash == frontmatter_hash => Ok(sync_resolution(
            SyncSource::Marker,
            "marker and frontmatter changed to the same projection",
            marker_projection.clone(),
            true,
            true,
        )),
        (true, true) => match prefer {
            Some(Prefer::Marker) => Ok(sync_resolution(
                SyncSource::Marker,
                "conflict overridden with --prefer marker",
                marker_projection.clone(),
                true,
                false,
            )),
            Some(Prefer::Frontmatter) => Ok(sync_resolution(
                SyncSource::Frontmatter,
                "conflict overridden with --prefer frontmatter",
                frontmatter_projection.clone(),
                false,
                true,
            )),
            None => {
                let Some(base_projection) = base_projection else {
                    return Err(CommandError::new(format!(
                        "marker/frontmatter conflict: marker hash {marker_hash}, frontmatter hash {frontmatter_hash}, stored hash {last_hash}; rerun with --prefer marker or --prefer frontmatter after reviewing both sides"
                    )));
                };
                let merge = merge_projection_changes(
                    base_projection,
                    marker_projection,
                    frontmatter_projection,
                );
                if !merge.conflicts.is_empty() {
                    return Err(marker_frontmatter_conflict_error(
                        &merge.conflicts,
                    ));
                }
                Ok(sync_resolution(
                    SyncSource::AutoMerge,
                    "marker and frontmatter changed compatible fields; auto-merged",
                    merge.projection,
                    merge.marker_contributed,
                    merge.frontmatter_contributed,
                ))
            }
        },
    }
}

fn sync_resolution(
    source: SyncSource,
    reason: impl Into<String>,
    projection: Projection,
    marker_contributed: bool,
    frontmatter_contributed: bool,
) -> SyncResolution {
    SyncResolution {
        decision: SyncDecision {
            source,
            reason: reason.into(),
            marker_contributed,
            frontmatter_contributed,
        },
        projection,
    }
}

#[derive(Debug, Clone)]
struct ProjectionMerge {
    projection: Projection,
    conflicts: Vec<ProjectionConflict>,
    marker_contributed: bool,
    frontmatter_contributed: bool,
}

fn merge_projection_changes(
    base: &Projection,
    marker: &Projection,
    frontmatter: &Projection,
) -> ProjectionMerge {
    let mut keys = BTreeSet::new();
    keys.extend(base.keys().cloned());
    keys.extend(marker.keys().cloned());
    keys.extend(frontmatter.keys().cloned());

    let mut projection = Projection::new();
    let mut conflicts = Vec::new();
    let mut marker_contributed = false;
    let mut frontmatter_contributed = false;

    for key in keys {
        let base_value = base.get(&key);
        let marker_value = marker.get(&key);
        let frontmatter_value = frontmatter.get(&key);
        let marker_changed = marker_value != base_value;
        let frontmatter_changed = frontmatter_value != base_value;

        marker_contributed |= marker_changed;
        frontmatter_contributed |= frontmatter_changed;

        let selected = match (marker_changed, frontmatter_changed) {
            (false, false) => base_value,
            (true, false) => marker_value,
            (false, true) => frontmatter_value,
            (true, true) if marker_value == frontmatter_value => marker_value,
            (true, true) => {
                conflicts.push(ProjectionConflict {
                    key,
                    base: base_value.cloned(),
                    marker: marker_value.cloned(),
                    frontmatter: frontmatter_value.cloned(),
                });
                continue;
            }
        };

        if let Some(value) = selected {
            projection.insert(key, value.clone());
        }
    }

    ProjectionMerge {
        projection,
        conflicts,
        marker_contributed,
        frontmatter_contributed,
    }
}

fn marker_frontmatter_conflict_error(
    conflicts: &[ProjectionConflict],
) -> CommandError {
    let mut message = String::from("marker/frontmatter conflict:");
    for conflict in conflicts.iter().take(8) {
        message.push_str(&format!(
            "\n  {}: marker={}, frontmatter={}, base={}",
            conflict.key,
            diagnostic_projection_value(conflict.marker.as_ref()),
            diagnostic_projection_value(conflict.frontmatter.as_ref()),
            diagnostic_projection_value(conflict.base.as_ref()),
        ));
    }
    if conflicts.len() > 8 {
        message.push_str(&format!(
            "\n  ... {} more conflict(s)",
            conflicts.len() - 8
        ));
    }
    message.push_str(
        "\nrerun with --prefer marker or --prefer frontmatter after reviewing both sides",
    );
    CommandError::new(message)
}

fn diagnostic_projection_value(value: Option<&MarkerValue>) -> String {
    let rendered = match value {
        Some(MarkerValue::String(value)) => quote_string(value),
        Some(value) => value.as_marker_value(),
        None => "<deleted>".to_string(),
    };
    truncate_diagnostic_value(&rendered, 80)
}

fn truncate_diagnostic_value(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn print_config_report(operation: &str, config: &Config) {
    println!("Highlights reference sync");
    println!("operation: {operation}");
    println!("bob_dir: {}", config.bob_dir.display());
    println!("lib_dir: {}", config.lib_dir.display());
    println!("ref_dir: {}", config.ref_dir.display());
    println!("managed_body_begin: {MANAGED_BODY_BEGIN}");
    println!("managed_body_end: {MANAGED_BODY_END}");
    println!(
        "pipeline_fields_excluded_from_marker_sync: {}",
        PIPELINE_FIELDS.join(",")
    );
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Sync Highlights PDF annotations into Bob reference notes")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            with_config_args(
                ClapCommand::new("doctor")
                    .about("Check Highlights reference sync prerequisites"),
            )
            .after_help("Checks vault paths, sidecars, PDF markers, Git state, and optional ob support."),
        )
        .subcommand(
            with_config_args(
                ClapCommand::new("marker")
                    .about("Inspect the marker note for one PDF")
                    .arg(pdf_arg("PDF whose marker note should be inspected")),
            )
            .after_help("The marker note is the first standalone /Text annotation on page 1."),
        )
        .subcommand(
            with_scan_args(
                ClapCommand::new("scan")
                    .about("Scan the configured Highlights library"),
            )
            .after_help("Scans PDFs recursively, preflights collisions and dirty targets, then syncs each PDF."),
        )
        .subcommand(with_sync_args(ClapCommand::new("sync")))
}

fn with_config_args(command: ClapCommand) -> ClapCommand {
    command
        .arg(bob_dir_arg())
        .arg(lib_dir_arg())
        .arg(ref_dir_arg())
}

fn with_scan_args(command: ClapCommand) -> ClapCommand {
    command
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(jobs_arg())
        .arg(lib_dir_arg())
        .arg(ref_dir_arg())
        .arg(verbose_arg())
        .arg(write_pdfs_arg())
}

fn with_sync_args(command: ClapCommand) -> ClapCommand {
    command
        .about("Sync one PDF marker note into its Bob reference note")
        .arg(pdf_arg("PDF to sync"))
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(lib_dir_arg())
        .arg(prefer_arg())
        .arg(ref_dir_arg())
        .arg(write_pdf_arg())
        .after_help("The first standalone /Text annotation on page 1 is treated as the marker note.")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn lib_dir_arg() -> Arg {
    Arg::new("lib-dir")
        .long("lib-dir")
        .short('l')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help(
            "Highlights PDF library; defaults to BOB_HIGHLIGHTS_LIB_DIR or lib",
        )
}

fn ref_dir_arg() -> Arg {
    Arg::new("ref-dir")
        .long("ref-dir")
        .short('r')
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Reference note output directory; defaults to BOB_HIGHLIGHTS_REF_DIR or ref")
}

fn pdf_arg(help: &'static str) -> Arg {
    Arg::new("pdf")
        .value_name("PDF")
        .required(true)
        .value_parser(OsStringValueParser::new())
        .help(help)
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview work without modifying the vault or PDF")
}

fn jobs_arg() -> Arg {
    Arg::new("jobs")
        .long("jobs")
        .short('j')
        .value_name("N")
        .value_parser(clap::value_parser!(u64).range(1..))
        .help(
            "Process this many PDFs in parallel; defaults to available CPU cores (use 1 to force sequential)",
        )
}

fn prefer_arg() -> Arg {
    Arg::new("prefer")
        .long("prefer")
        .short('p')
        .value_name("SIDE")
        .value_parser(["marker", "frontmatter"])
        .help("Resolve a marker/frontmatter conflict using this side")
}

fn verbose_arg() -> Arg {
    Arg::new("verbose")
        .long("verbose")
        .short('v')
        .action(ArgAction::SetTrue)
        .help("Print the detailed per-PDF scan report")
}

fn write_pdf_arg() -> Arg {
    Arg::new("write-pdf")
        .long("write-pdf")
        .short('w')
        .action(ArgAction::SetTrue)
        .help("Allow marker writes back to the PDF")
}

fn write_pdfs_arg() -> Arg {
    Arg::new("write-pdfs")
        .long("write-pdfs")
        .short('w')
        .action(ArgAction::SetTrue)
        .help("Allow marker writes back to all PDFs during scan")
}

impl Config {
    fn from_matches(matches: &ArgMatches) -> Self {
        let bob_dir = matches
            .get_one::<OsString>("bob-dir")
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        let lib_dir = configured_path(
            matches,
            "lib-dir",
            ENV_LIB_DIR,
            DEFAULT_LIB_DIR,
            &bob_dir,
        );
        let ref_dir = configured_path(
            matches,
            "ref-dir",
            ENV_REF_DIR,
            DEFAULT_REF_DIR,
            &bob_dir,
        );
        Self {
            bob_dir,
            lib_dir,
            ref_dir,
        }
    }
}

impl Prefer {
    fn as_str(self) -> &'static str {
        match self {
            Prefer::Marker => "marker",
            Prefer::Frontmatter => "frontmatter",
        }
    }
}

impl SyncSource {
    fn as_str(self) -> &'static str {
        match self {
            SyncSource::Marker => "marker",
            SyncSource::Frontmatter => "frontmatter",
            SyncSource::AutoMerge => "auto-merge",
        }
    }
}

impl MarkerValue {
    fn as_marker_value(&self) -> String {
        match self {
            MarkerValue::Null => "null".to_string(),
            MarkerValue::Bool(value) => value.to_string(),
            MarkerValue::Number(value) => value.clone(),
            MarkerValue::String(value) => render_marker_scalar_string(value),
            MarkerValue::List(values) => {
                let values = values
                    .iter()
                    .map(MarkerValue::as_marker_list_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
        }
    }

    fn as_marker_list_value(&self) -> String {
        match self {
            MarkerValue::Null => "null".to_string(),
            MarkerValue::Bool(value) => value.to_string(),
            MarkerValue::Number(value) => value.clone(),
            MarkerValue::String(value) => render_marker_list_string(value),
            MarkerValue::List(values) => {
                let values = values
                    .iter()
                    .map(MarkerValue::as_marker_list_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
        }
    }

    fn as_frontmatter_value(&self) -> String {
        match self {
            MarkerValue::Null => "null".to_string(),
            MarkerValue::Bool(value) => value.to_string(),
            MarkerValue::Number(value) => value.clone(),
            MarkerValue::String(value) => render_frontmatter_string(value),
            MarkerValue::List(values) => {
                let values = values
                    .iter()
                    .map(MarkerValue::as_frontmatter_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
        }
    }

    fn as_string(&self) -> Option<&str> {
        match self {
            MarkerValue::String(value) => Some(value.as_str()),
            _ => None,
        }
    }

    fn is_empty_required_value(&self) -> bool {
        match self {
            MarkerValue::Null => true,
            MarkerValue::String(value) => value.trim().is_empty(),
            MarkerValue::List(values) => values.is_empty(),
            _ => false,
        }
    }
}

impl ParsedNote {
    fn empty() -> Self {
        Self {
            frontmatter: Vec::new(),
            body: String::new(),
            original: None,
        }
    }

    fn exists(&self) -> bool {
        !self.frontmatter.is_empty() || !self.body.is_empty()
    }

    fn contents(&self) -> Option<String> {
        self.exists().then(|| {
            self.original
                .clone()
                .unwrap_or_else(|| self.render_original())
        })
    }

    fn render_original(&self) -> String {
        if self.frontmatter.is_empty() {
            return self.body.clone();
        }
        let mut rendered = String::from("---\n");
        for entry in &self.frontmatter {
            rendered.push_str(&entry.raw);
            rendered.push('\n');
        }
        rendered.push_str("---\n");
        rendered.push_str(&self.body);
        rendered
    }

    fn marker_hash(&self) -> Option<String> {
        self.frontmatter.iter().find_map(|entry| {
            (entry.key.as_deref() == Some(FIELD_MARKER_HASH))
                .then(|| entry.value.as_ref()?.as_string().map(str::to_string))
                .flatten()
        })
    }

    fn marker_base_projection_with_normalization(
        &self,
    ) -> Result<(Option<Projection>, bool)> {
        let Some(entry) = self
            .frontmatter
            .iter()
            .find(|entry| entry.key.as_deref() == Some(FIELD_MARKER_BASE))
        else {
            return Ok((None, false));
        };
        let Some(value) = &entry.value else {
            return Err(CommandError::new(format!(
                "{FIELD_MARKER_BASE} must be a compact JSON string"
            )));
        };
        let Some(json) = value.as_string() else {
            return Err(CommandError::new(format!(
                "{FIELD_MARKER_BASE} must be a compact JSON string"
            )));
        };
        let mut projection = projection_from_snapshot_json(json)?;
        let status_normalized = normalize_deprecated_status(&mut projection);
        Ok((Some(projection), status_normalized))
    }

    fn marker_fields(&self) -> BTreeSet<String> {
        self.frontmatter
            .iter()
            .find_map(|entry| {
                (entry.key.as_deref() == Some(FIELD_MARKER_FIELDS))
                    .then(|| entry.value.as_ref().and_then(value_as_string_set))
                    .flatten()
            })
            .unwrap_or_default()
    }

    fn frontmatter_value(&self, field: &str) -> Option<MarkerValue> {
        self.frontmatter.iter().find_map(|entry| {
            (entry.key.as_deref() == Some(field))
                .then(|| entry.value.clone())
                .flatten()
        })
    }

    fn synced_projection_with_normalization(
        &self,
    ) -> Result<NormalizedProjection> {
        let marker_fields = self.marker_fields();
        let mut projection = Projection::new();

        for entry in &self.frontmatter {
            let Some(key) = &entry.key else {
                continue;
            };
            if is_managed_frontmatter_field(key) {
                continue;
            }
            if (is_standard_user_field(key) || marker_fields.contains(key))
                && let Some(value) = &entry.value
            {
                projection.insert(key.clone(), value.clone());
            }
        }

        canonicalize_parent(&mut projection, "frontmatter")?;
        let status_normalized = normalize_deprecated_status(&mut projection);
        Ok(NormalizedProjection {
            projection,
            status_normalized,
        })
    }

    fn render_with_projection(
        &self,
        projection: &Projection,
        marker_hash: &str,
        metadata: &PipelineMetadata,
        body: &str,
    ) -> String {
        let old_marker_fields = self.marker_fields();
        let mut removed_keys = BTreeSet::new();
        removed_keys
            .extend(PIPELINE_FIELDS.iter().map(|field| (*field).to_string()));
        removed_keys.extend(
            COMMAND_MANAGED_FIELDS
                .iter()
                .map(|field| (*field).to_string()),
        );
        removed_keys.extend(
            COMMON_USER_FIELDS
                .iter()
                .chain(MARKER_REQUIRED_KEYS.iter())
                .map(|field| (*field).to_string()),
        );
        removed_keys.extend(old_marker_fields);
        removed_keys.extend(projection.keys().cloned());

        let mut lines = Vec::new();
        let mut rendered_command_managed_fields = false;
        for key in ordered_projection_keys(projection) {
            let Some(value) = projection.get(&key) else {
                continue;
            };
            lines.push(format!("{key}: {}", value.as_frontmatter_value()));
            if key == FIELD_PARENT {
                push_command_managed_frontmatter_lines(&mut lines, metadata);
                rendered_command_managed_fields = true;
            }
        }
        if !rendered_command_managed_fields {
            push_command_managed_frontmatter_lines(&mut lines, metadata);
        }

        for entry in &self.frontmatter {
            match &entry.key {
                Some(key) if removed_keys.contains(key) => {}
                _ => lines.push(entry.raw.clone()),
            }
        }

        let marker_fields = unknown_synced_fields(projection);
        lines.push(format!(
            "{FIELD_SOURCE_PDF}: {}",
            MarkerValue::String(metadata.source_pdf.clone())
                .as_frontmatter_value()
        ));
        lines.push(format!(
            "{FIELD_SOURCE_PDF_SHA256}: {}",
            MarkerValue::String(metadata.source_pdf_sha256.clone())
                .as_frontmatter_value()
        ));
        if let Some(value) = &metadata.highlights_sidecar {
            lines.push(format!(
                "{FIELD_HIGHLIGHTS_SIDECAR}: {}",
                value.as_frontmatter_value()
            ));
        }
        if let Some(value) = &metadata.highlights_count {
            lines.push(format!(
                "{FIELD_HIGHLIGHTS_COUNT}: {}",
                value.as_frontmatter_value()
            ));
        }
        if let Some(value) = &metadata.highlights_synced_at {
            lines.push(format!(
                "{FIELD_HIGHLIGHTS_SYNCED_AT}: {}",
                value.as_frontmatter_value()
            ));
        }
        lines.push(format!(
            "{FIELD_MARKER_HASH}: {}",
            MarkerValue::String(marker_hash.to_string()).as_frontmatter_value()
        ));
        lines.push(format!(
            "{FIELD_MARKER_BASE}: {}",
            MarkerValue::String(projection_snapshot_json(projection))
                .as_frontmatter_value()
        ));
        if !marker_fields.is_empty() {
            let value = MarkerValue::List(
                marker_fields
                    .into_iter()
                    .map(MarkerValue::String)
                    .collect::<Vec<_>>(),
            );
            lines.push(format!(
                "{FIELD_MARKER_FIELDS}: {}",
                value.as_frontmatter_value()
            ));
        }
        lines.push(format!(
            "{FIELD_PIPELINE_VERSION}: {}",
            MarkerValue::String(PIPELINE_VERSION.to_string())
                .as_frontmatter_value()
        ));

        let mut rendered = String::from("---\n");
        for line in lines {
            rendered.push_str(&line);
            rendered.push('\n');
        }
        rendered.push_str("---\n");
        rendered.push_str(body);
        rendered
    }

    fn render_body(
        &self,
        pdf: &Path,
        projection: &Projection,
        source_pdf: &str,
        rendered_highlights: Option<&RenderedHighlights>,
    ) -> Result<String> {
        if !self.exists() {
            return Ok(default_note_body(
                pdf,
                projection,
                source_pdf,
                rendered_highlights,
            ));
        }

        let Some(_) = self.managed_region()? else {
            return Err(CommandError::new(
                "existing reference note is missing the managed Highlights region; add <!-- highlights:begin --> and <!-- highlights:end --> before syncing",
            ));
        };

        let Some(rendered_highlights) = rendered_highlights else {
            return rewrite_pdf_task_checkbox_for_projection(
                &self.body, projection,
            );
        };
        let replacement = rendered_highlights.content.as_str();
        let body = replace_managed_region(&self.body, replacement)?;
        rewrite_pdf_task_checkbox_for_projection(&body, projection)
    }

    fn managed_region(&self) -> Result<Option<&str>> {
        if !self.exists() {
            return Ok(None);
        }
        managed_region(&self.body)
    }

    fn generated_block_ids(&self) -> Result<BTreeSet<String>> {
        let Some(region) = self.managed_region()? else {
            return Ok(BTreeSet::new());
        };
        Ok(generated_block_ids(region))
    }
}

fn pipeline_metadata(
    config: &Config,
    pdf: &Path,
    source_pdf_sha256: &str,
    note: &ParsedNote,
    sidecar: Option<&SidecarInput>,
    rendered_highlights: Option<&RenderedHighlights>,
    refresh_synced_at: bool,
) -> Result<PipelineMetadata> {
    let highlights_sidecar = match sidecar {
        Some(sidecar) => {
            Some(MarkerValue::String(source_pdf_value(config, &sidecar.path)))
        }
        None => note.frontmatter_value(FIELD_HIGHLIGHTS_SIDECAR),
    };
    let highlights_count = match rendered_highlights {
        Some(rendered) => Some(MarkerValue::Number(rendered.count.to_string())),
        None => note.frontmatter_value(FIELD_HIGHLIGHTS_COUNT),
    };
    let highlights_synced_at = if rendered_highlights.is_some() {
        if refresh_synced_at {
            Some(MarkerValue::String(current_timestamp()))
        } else {
            note.frontmatter_value(FIELD_HIGHLIGHTS_SYNCED_AT)
                .or_else(|| Some(MarkerValue::String(current_timestamp())))
        }
    } else {
        note.frontmatter_value(FIELD_HIGHLIGHTS_SYNCED_AT)
    };
    let ref_type = pdf_path_metadata(config, pdf)?.ref_type;

    Ok(PipelineMetadata {
        source_pdf: source_pdf_value(config, pdf),
        source_pdf_sha256: source_pdf_sha256.to_string(),
        ref_type,
        highlights_sidecar,
        highlights_count,
        highlights_synced_at,
    })
}

fn default_note_body(
    pdf: &Path,
    projection: &Projection,
    source_pdf: &str,
    rendered_highlights: Option<&RenderedHighlights>,
) -> String {
    let title = note_title(pdf, projection);
    let highlights = rendered_highlights
        .map(|rendered| rendered.content.as_str())
        .unwrap_or("");
    let mut body = String::new();
    body.push('\n');
    body.push_str("# ");
    body.push_str(&title);
    body.push_str("\n\n");
    body.push_str("- [");
    body.push(projection_pdf_task_mark(projection));
    body.push_str("] #task [[");
    body.push_str(source_pdf);
    body.push_str("]] ");
    body.push_str(PDF_TASK_PRIORITY);
    body.push(' ');
    body.push_str(PDF_TASK_BLOCK_ID);
    body.push_str("\n\n");
    body.push_str("## Highlights\n\n");
    body.push_str(MANAGED_BODY_BEGIN);
    body.push_str("\n\n");
    body.push_str(highlights);
    if !highlights.is_empty() && !highlights.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(MANAGED_BODY_END);
    body.push('\n');
    body
}

fn note_type_frontmatter_line() -> String {
    format!(
        "{FIELD_NOTE_TYPE}: {}",
        MarkerValue::String(NOTE_TYPE_VALUE.to_string()).as_frontmatter_value()
    )
}

fn push_command_managed_frontmatter_lines(
    lines: &mut Vec<String>,
    metadata: &PipelineMetadata,
) {
    lines.push(note_type_frontmatter_line());
    if let Some(ref_type) = &metadata.ref_type {
        lines.push(format!(
            "{FIELD_REF_TYPE}: {}",
            MarkerValue::String(ref_type.clone()).as_frontmatter_value()
        ));
    }
}

fn note_title(pdf: &Path, projection: &Projection) -> String {
    projection
        .get("title")
        .and_then(MarkerValue::as_string)
        .filter(|title| !title.trim().is_empty())
        .map(|title| title.trim().to_string())
        .or_else(|| {
            pdf.file_stem()
                .and_then(OsStr::to_str)
                .map(|stem| stem.replace(['_', '-'], " "))
        })
        .unwrap_or_else(|| "Reference".to_string())
}

fn managed_region(body: &str) -> Result<Option<&str>> {
    let Some(begin) = body.find(MANAGED_BODY_BEGIN) else {
        return Ok(None);
    };
    let after_begin = begin + MANAGED_BODY_BEGIN.len();
    if body[after_begin..].contains(MANAGED_BODY_BEGIN) {
        return Err(CommandError::new(
            "reference note has multiple managed Highlights begin markers",
        ));
    }
    let Some(relative_end) = body[after_begin..].find(MANAGED_BODY_END) else {
        return Err(CommandError::new(
            "reference note has a managed Highlights begin marker without a matching end marker",
        ));
    };
    let end = after_begin + relative_end;
    if body[end + MANAGED_BODY_END.len()..].contains(MANAGED_BODY_END) {
        return Err(CommandError::new(
            "reference note has multiple managed Highlights end markers",
        ));
    }
    Ok(Some(body[after_begin..end].trim_matches('\n')))
}

fn replace_managed_region(body: &str, replacement: &str) -> Result<String> {
    let begin = body.find(MANAGED_BODY_BEGIN).ok_or_else(|| {
        CommandError::new(
            "reference note is missing the managed Highlights begin marker",
        )
    })?;
    let after_begin = begin + MANAGED_BODY_BEGIN.len();
    let relative_end =
        body[after_begin..].find(MANAGED_BODY_END).ok_or_else(|| {
            CommandError::new(
                "reference note is missing the managed Highlights end marker",
            )
        })?;
    let end = after_begin + relative_end;
    let mut rendered = String::new();
    rendered.push_str(&body[..after_begin]);
    rendered.push_str("\n\n");
    rendered.push_str(replacement);
    if !replacement.is_empty() && !replacement.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str(&body[end..]);
    Ok(rendered)
}

fn rewrite_pdf_task_checkbox_for_projection(
    body: &str,
    projection: &Projection,
) -> Result<String> {
    replace_pdf_task_checkbox_mark(body, projection_pdf_task_mark(projection))
}

#[cfg(test)]
fn insert_missing_annotation_tasks(
    config: &Config,
    body: &str,
    candidates: &[AnnotationTaskCandidate],
    created_date: &str,
) -> Result<String> {
    if candidates.is_empty() {
        return Ok(body.to_string());
    }

    let task_line = match parse_pdf_task_line(body)? {
        PdfTaskLineState::Present(task_line) => task_line,
        PdfTaskLineState::Missing => {
            return Err(CommandError::new(
                "reference note is missing the generated PDF task line with ^ref; cannot create annotation tasks",
            ));
        }
    };
    let mut existing = existing_annotation_task_identities(body);
    let mut missing = Vec::new();
    for candidate in candidates {
        if existing.insert(candidate.identity.clone()) {
            missing.push(render_annotation_task_line(
                config,
                candidate,
                created_date,
            ));
        }
    }

    if missing.is_empty() {
        return Ok(body.to_string());
    }

    Ok(insert_lines_after(body, task_line.line_index, &missing))
}

fn insert_annotation_task_lines_after_pdf_task(
    body: &str,
    lines: &[String],
) -> Result<String> {
    if lines.is_empty() {
        return Ok(body.to_string());
    }

    let task_line = match parse_pdf_task_line(body)? {
        PdfTaskLineState::Present(task_line) => task_line,
        PdfTaskLineState::Missing => {
            return Err(CommandError::new(
                "reference note is missing the generated PDF task line with ^ref; cannot create annotation tasks",
            ));
        }
    };
    Ok(insert_lines_after(body, task_line.line_index, lines))
}

fn append_task_lines(contents: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return contents.to_string();
    }

    let line_ending = preferred_line_ending(contents);
    let mut rendered = String::with_capacity(
        contents.len()
            + lines
                .iter()
                .map(|line| line.len() + line_ending.len())
                .sum::<usize>()
            + line_ending.len(),
    );
    rendered.push_str(contents);
    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push_str(line_ending);
    }
    for line in lines {
        rendered.push_str(line);
        rendered.push_str(line_ending);
    }
    rendered
}

fn preferred_line_ending(contents: &str) -> &'static str {
    contents
        .find('\n')
        .map(|index| {
            if index > 0 && contents.as_bytes().get(index - 1) == Some(&b'\r') {
                "\r\n"
            } else {
                "\n"
            }
        })
        .unwrap_or("\n")
}

fn insert_lines_after(
    body: &str,
    line_index: usize,
    lines: &[String],
) -> String {
    let mut rendered = String::with_capacity(
        body.len() + lines.iter().map(|line| line.len() + 1).sum::<usize>(),
    );
    for (index, segment) in body.split_inclusive('\n').enumerate() {
        rendered.push_str(segment);
        if index != line_index {
            continue;
        }

        let (_, line_ending) = split_line_segment(segment);
        let line_ending = if line_ending.is_empty() {
            rendered.push('\n');
            "\n"
        } else {
            line_ending
        };
        for line in lines {
            rendered.push_str(line);
            rendered.push_str(line_ending);
        }
    }
    rendered
}

fn replace_pdf_task_checkbox_mark(body: &str, mark: char) -> Result<String> {
    let task_line = match parse_pdf_task_line(body)? {
        PdfTaskLineState::Missing => return Ok(body.to_string()),
        PdfTaskLineState::Present(task_line) => task_line,
    };

    if task_line.mark == mark {
        return Ok(body.to_string());
    }

    let mut rendered = String::with_capacity(body.len());
    for (line_index, segment) in body.split_inclusive('\n').enumerate() {
        if line_index != task_line.line_index {
            rendered.push_str(segment);
            continue;
        }
        let (line, line_ending) = split_line_segment(segment);
        rendered.push_str(&line[..task_line.checkbox_mark_index]);
        rendered.push(mark);
        rendered.push_str(&line[task_line.checkbox_mark_index + 1..]);
        rendered.push_str(line_ending);
    }
    if !body.ends_with('\n') {
        // split_inclusive includes the final unterminated segment, so this
        // branch is only here to make the invariant obvious to future edits.
        debug_assert_eq!(
            rendered.lines().count(),
            body.lines().count(),
            "unterminated final line should be rewritten in the loop"
        );
    }
    Ok(rendered)
}

fn split_line_segment(segment: &str) -> (&str, &str) {
    if let Some(line) = segment.strip_suffix("\r\n") {
        (line, "\r\n")
    } else if let Some(line) = segment.strip_suffix('\n') {
        (line, "\n")
    } else {
        (segment, "")
    }
}

fn generated_block_ids(region: &str) -> BTreeSet<String> {
    region
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix('^')
                .filter(|id| id.starts_with("h-") && is_valid_block_id(id))
                .map(str::to_string)
        })
        .collect()
}

fn is_valid_block_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-'
        })
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn current_local_date() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn configured_path(
    matches: &ArgMatches,
    arg_name: &str,
    env_name: &str,
    default_value: &str,
    bob_dir: &Path,
) -> PathBuf {
    let configured = matches
        .get_one::<OsString>(arg_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os(env_name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from(default_value));

    resolve_under_bob(bob_dir, &configured)
}

fn resolve_under_bob(bob_dir: &Path, path: &Path) -> PathBuf {
    let expanded = bob_env::expand_tilde(path);
    if expanded.is_absolute() {
        expanded
    } else {
        bob_dir.join(expanded)
    }
}

fn required_path(matches: &ArgMatches, name: &str) -> PathBuf {
    let value = matches
        .get_one::<OsString>(name)
        .expect("required argument is enforced by clap");
    bob_env::expand_tilde(&PathBuf::from(value))
}

fn prefer_from_matches(matches: &ArgMatches) -> Option<Prefer> {
    matches
        .get_one::<String>("prefer")
        .map(|value| match value.as_str() {
            "marker" => Prefer::Marker,
            "frontmatter" => Prefer::Frontmatter,
            _ => unreachable!("clap value parser restricts prefer"),
        })
}

fn read_pdf_marker(path: &Path) -> Result<PdfMarker> {
    // Read the PDF from disk exactly once and reuse the bytes for both the
    // SHA-256 (pipeline metadata) and the lopdf parse, instead of reading the
    // whole file twice (Document::load plus a separate hash pass).
    let bytes = fs::read(path).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", path.display()))
    })?;
    let source_pdf_sha256 = hex::encode(Sha256::digest(&bytes));
    let document = Document::load_mem(&bytes).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", path.display()))
    })?;
    let Some(first_page_id) = document.page_iter().next() else {
        return Err(CommandError::new(format!(
            "no first page found in {}; marker note must be on page 1",
            path.display()
        )));
    };
    let mut note_number = 0;

    for annotation_id in annotation_ids_for_page(&document, first_page_id)? {
        let annotation =
            document.get_dictionary(annotation_id).map_err(|error| {
                CommandError::new(format!(
                    "read annotation {annotation_id:?} in {}: {error}",
                    path.display()
                ))
            })?;
        if !is_standalone_note(annotation) {
            continue;
        }
        note_number += 1;
        let contents = annotation
            .get(b"Contents")
            .ok()
            .map(decode_marker_contents)
            .transpose()
            .map_err(|error| {
                CommandError::new(format!(
                    "decode marker contents in {}: {error}",
                    path.display()
                ))
            })?
            .unwrap_or_default();
        return Ok(PdfMarker {
            annotation_id,
            contents,
            page_number: 1,
            note_number,
            source_pdf_sha256,
        });
    }

    Err(CommandError::new(format!(
        "no standalone /Text note annotations found on page 1 in {}",
        path.display()
    )))
}

fn decode_marker_contents(
    contents: &Object,
) -> std::result::Result<String, lopdf::Error> {
    let Object::String(bytes, _) = contents else {
        return decode_text_string(contents);
    };
    if has_text_string_bom(bytes) {
        return decode_text_string(contents);
    }

    let mut decoded = String::new();
    let mut segment_start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                decoded.push_str(&decode_marker_text_segment(
                    &bytes[segment_start..index],
                )?);
                decoded.push('\n');
                index += 1;
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                segment_start = index;
            }
            b'\n' => {
                decoded.push_str(&decode_marker_text_segment(
                    &bytes[segment_start..index],
                )?);
                decoded.push('\n');
                index += 1;
                segment_start = index;
            }
            _ => index += 1,
        }
    }
    decoded.push_str(&decode_marker_text_segment(&bytes[segment_start..])?);
    Ok(decoded)
}

fn has_text_string_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\xFE\xFF") || bytes.starts_with(b"\xEF\xBB\xBF")
}

fn decode_marker_text_segment(
    segment: &[u8],
) -> std::result::Result<String, lopdf::Error> {
    decode_text_string(&Object::String(segment.to_vec(), StringFormat::Literal))
}

fn annotation_ids_for_page(
    document: &Document,
    page_id: ObjectId,
) -> Result<Vec<ObjectId>> {
    let page = document.get_dictionary(page_id).map_err(|error| {
        CommandError::new(format!("read page {page_id:?}: {error}"))
    })?;
    let Ok(annots) = page.get(b"Annots") else {
        return Ok(Vec::new());
    };
    let annot_array = match annots {
        Object::Reference(id) => document
            .get_object(*id)
            .and_then(Object::as_array)
            .map_err(|error| {
                CommandError::new(format!(
                    "read annotation array {id:?}: {error}"
                ))
            })?,
        Object::Array(array) => array,
        _ => return Ok(Vec::new()),
    };

    let mut ids = Vec::new();
    for annot in annot_array {
        if let Ok(id) = annot.as_reference() {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn is_standalone_note(annotation: &lopdf::Dictionary) -> bool {
    annotation
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|name| name == b"Text")
}

fn write_pdf_marker(
    path: &Path,
    annotation_id: ObjectId,
    contents: &str,
) -> Result<()> {
    let mut document = Document::load(path).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", path.display()))
    })?;
    let annotation = document
        .get_object_mut(annotation_id)
        .and_then(Object::as_dict_mut)
        .map_err(|error| {
            CommandError::new(format!(
                "read marker annotation {annotation_id:?} in {}: {error}",
                path.display()
            ))
        })?;
    annotation.set("Contents", pdf_text_string(contents));
    atomic_save_pdf(path, &mut document)
}

fn pdf_text_string(contents: &str) -> Object {
    Object::String(encode_utf16_be(contents), StringFormat::Hexadecimal)
}

fn atomic_save_pdf(path: &Path, document: &mut Document) -> Result<()> {
    let temp_path = temporary_write_path(path)?;
    let _ = fs::remove_file(&temp_path);
    document.save(&temp_path).map_err(|error| {
        CommandError::new(format!(
            "write temporary PDF {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        CommandError::new(format!("install PDF {}: {error}", path.display()))
    })
}

fn parse_marker(contents: &str) -> Result<Projection> {
    Ok(parse_marker_with_normalization(contents)?.projection)
}

fn parse_marker_with_normalization(
    contents: &str,
) -> Result<NormalizedProjection> {
    let mut projection = Projection::new();
    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        else {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: expected '- key: value' or '* key: value'"
            )));
        };
        let Some((key, value)) = item.split_once(':') else {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: missing ':'"
            )));
        };
        let key = normalize_key(key);
        if key.is_empty() {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: empty key"
            )));
        }
        if is_pipeline_field(&key) {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: '{key}' is pipeline-owned and cannot be synced from the marker"
            )));
        }
        if is_command_managed_field(&key) {
            return Err(CommandError::new(format!(
                "invalid marker item on line {line_number}: '{key}' is command-managed and cannot be synced from the marker"
            )));
        }
        if projection.contains_key(&key) {
            return Err(CommandError::new(format!(
                "duplicate marker key on line {line_number}: {key}"
            )));
        }
        let parsed_value = parse_value(value);
        if key == FIELD_PARENT {
            validate_marker_parent_value(value, line_number, &parsed_value)?;
        }
        projection.insert(key, parsed_value);
    }

    canonicalize_parent(&mut projection, "marker")?;
    let status_normalized = normalize_deprecated_status(&mut projection);
    validate_required_marker_keys(&projection, "marker")?;
    Ok(NormalizedProjection {
        projection,
        status_normalized,
    })
}

fn normalize_deprecated_status(projection: &mut Projection) -> bool {
    let Some(MarkerValue::String(status)) = projection.get_mut(FIELD_STATUS)
    else {
        return false;
    };
    if status == DEPRECATED_STATUS_DONE {
        *status = STATUS_READ.to_string();
        true
    } else {
        false
    }
}

fn validate_required_marker_keys(
    projection: &Projection,
    source: &str,
) -> Result<()> {
    for key in MARKER_REQUIRED_KEYS {
        let Some(value) = projection.get(*key) else {
            return Err(CommandError::new(format!(
                "missing required marker key: {key}"
            )));
        };
        if value.is_empty_required_value() {
            return Err(CommandError::new(format!(
                "{source} has an empty required marker key: {key}"
            )));
        }
    }
    validate_status_value(projection, source)?;
    Ok(())
}

fn validate_status_value(projection: &Projection, source: &str) -> Result<()> {
    let Some(value) = projection.get(FIELD_STATUS) else {
        return Ok(());
    };
    let Some(status) = value.as_string() else {
        return Err(CommandError::new(format!(
            "{source} status must be a scalar string; supported statuses: {}",
            ALLOWED_STATUS_VALUES.join(", ")
        )));
    };
    if ALLOWED_STATUS_VALUES.contains(&status) {
        return Ok(());
    }
    Err(CommandError::new(format!(
        "{source} has unsupported status {}: supported statuses: {}",
        quote_string(status),
        ALLOWED_STATUS_VALUES.join(", ")
    )))
}

fn render_marker(projection: &Projection) -> Result<String> {
    let mut rendered = String::new();
    for key in ordered_projection_keys(projection) {
        let Some(value) = projection.get(&key) else {
            continue;
        };
        rendered.push_str("- ");
        rendered.push_str(&key);
        rendered.push_str(": ");
        if key == FIELD_PARENT {
            rendered.push_str(&render_marker_parent_value(value)?);
        } else {
            rendered.push_str(&value.as_marker_value());
        }
        rendered.push('\n');
    }
    Ok(rendered)
}

fn validate_marker_parent_value(
    raw_value: &str,
    line_number: usize,
    parsed_value: &MarkerValue,
) -> Result<()> {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return Err(empty_parent_error("marker"));
    }
    if matches!(parsed_value, MarkerValue::Null) {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; null is not supported"
        )));
    }
    if matches!(parsed_value, MarkerValue::List(_)) {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; inline lists are not supported"
        )));
    }
    if trimmed.starts_with("![[") && trimmed.ends_with("]]") {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; embeds are not supported"
        )));
    }
    if is_wikilink(trimmed) {
        let detail = if trimmed[2..trimmed.len() - 2].contains('|') {
            "aliases are not supported"
        } else if trimmed[2..trimmed.len() - 2].contains("#^") {
            "block links are not supported"
        } else {
            "wikilinks are not supported"
        };
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; {detail}"
        )));
    }
    if trimmed.contains("[[") || trimmed.contains("]]") {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; wikilinks are not supported"
        )));
    }
    if trimmed.starts_with(['"', '\'']) || trimmed.ends_with(['"', '\'']) {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; quoted parent values are not supported"
        )));
    }
    if trimmed.contains("#^") {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; block links are not supported"
        )));
    }
    if trimmed.contains('|') {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; aliases are not supported"
        )));
    }
    if trimmed.starts_with('[') || trimmed.ends_with(']') {
        return Err(CommandError::new(format!(
            "invalid marker parent on line {line_number}: parent must be a bare note target; structured marker syntax is not supported"
        )));
    }
    Ok(())
}

fn render_marker_parent_value(value: &MarkerValue) -> Result<String> {
    let Some(parent) = value.as_string() else {
        return Err(CommandError::new(
            "parent cannot be rendered as a PDF marker bare note target: expected a canonical wikilink string",
        ));
    };
    let Some(target) = simple_wikilink_target(parent) else {
        return Err(CommandError::new(format!(
            "parent cannot be rendered as a PDF marker bare note target: expected a simple wikilink like [[memory_ref]], got {}",
            quote_string(parent)
        )));
    };
    Ok(target.to_string())
}

fn ordered_projection_keys(projection: &Projection) -> Vec<String> {
    let mut keys = Vec::new();
    for key in MARKER_REQUIRED_KEYS {
        if projection.contains_key(*key) {
            keys.push((*key).to_string());
        }
    }
    for field in COMMON_USER_FIELDS {
        if projection.contains_key(*field)
            && !MARKER_REQUIRED_KEYS.contains(field)
        {
            keys.push((*field).to_string());
        }
    }
    for key in projection.keys() {
        if !MARKER_REQUIRED_KEYS.contains(&key.as_str())
            && !COMMON_USER_FIELDS.contains(&key.as_str())
        {
            keys.push(key.clone());
        }
    }
    keys
}

fn parse_value(value: &str) -> MarkerValue {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return MarkerValue::String(String::new());
    }
    if trimmed.eq_ignore_ascii_case("null") || trimmed == "~" {
        return MarkerValue::Null;
    }
    if trimmed.eq_ignore_ascii_case("true") {
        return MarkerValue::Bool(true);
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return MarkerValue::Bool(false);
    }
    if let Some(value) = parse_quoted_string(trimmed) {
        return MarkerValue::String(value);
    }
    if is_wikilink(trimmed) {
        return MarkerValue::String(trimmed.to_string());
    }
    if let Some(values) = parse_inline_list(trimmed) {
        return MarkerValue::List(values);
    }
    if is_number_literal(trimmed) {
        return MarkerValue::Number(trimmed.to_string());
    }
    MarkerValue::String(trimmed.to_string())
}

fn parse_quoted_string(value: &str) -> Option<String> {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str::<String>(value).ok();
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        let inner = &value[1..value.len() - 1];
        return Some(inner.replace("''", "'"));
    }
    None
}

fn parse_inline_list(value: &str) -> Option<Vec<MarkerValue>> {
    if !(value.starts_with('[') && value.ends_with(']')) {
        return None;
    }
    let inner = &value[1..value.len() - 1];
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }

    split_inline_list(inner)
        .map(|items| items.into_iter().map(|item| parse_value(&item)).collect())
}

fn split_inline_list(value: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars().peekable();
    let mut quote = None;
    let mut bracket_depth = 0usize;

    while let Some(character) = chars.next() {
        match (quote, character) {
            (Some('"'), '\\') => {
                current.push(character);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(active), c) if c == active => {
                quote = None;
                current.push(c);
            }
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(character);
                current.push(character);
            }
            (None, '[') => {
                bracket_depth += 1;
                current.push(character);
            }
            (None, ']') if bracket_depth > 0 => {
                bracket_depth -= 1;
                current.push(character);
            }
            (None, ',') if bracket_depth == 0 => {
                items.push(current.trim().to_string());
                current.clear();
            }
            (None, c) => current.push(c),
        }
    }

    if quote.is_some() || bracket_depth != 0 {
        return None;
    }
    items.push(current.trim().to_string());
    Some(items)
}

fn is_number_literal(value: &str) -> bool {
    if value.starts_with('+') {
        return false;
    }
    if value.parse::<i64>().is_ok() {
        return true;
    }
    value.contains('.')
        && value.parse::<f64>().is_ok()
        && value.chars().all(|character| {
            character.is_ascii_digit() || matches!(character, '-' | '.')
        })
}

fn normalize_key(key: &str) -> String {
    key.trim()
        .chars()
        .map(|character| match character {
            '-' | ' ' => '_',
            other => other.to_ascii_lowercase(),
        })
        .collect::<String>()
}

fn read_note(path: &Path) -> Result<ParsedNote> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(parse_note(&contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(ParsedNote::empty())
        }
        Err(error) => Err(CommandError::new(format!(
            "read note {}: {error}",
            path.display()
        ))),
    }
}

fn parse_note(contents: &str) -> ParsedNote {
    let Some((frontmatter, body)) = split_frontmatter(contents) else {
        return ParsedNote {
            frontmatter: Vec::new(),
            body: contents.to_string(),
            original: Some(contents.to_string()),
        };
    };
    ParsedNote {
        frontmatter: frontmatter
            .into_iter()
            .map(|raw| parse_frontmatter_entry(&raw))
            .collect(),
        body,
        original: Some(contents.to_string()),
    }
}

fn split_frontmatter(contents: &str) -> Option<(Vec<String>, String)> {
    let marker_len = if contents.starts_with("---\r\n") {
        5
    } else if contents.starts_with("---\n") {
        4
    } else {
        return None;
    };

    let mut offset = marker_len;
    let mut lines = Vec::new();
    while offset < contents.len() {
        let remaining = &contents[offset..];
        let line_len = remaining
            .find('\n')
            .map(|index| index + 1)
            .unwrap_or(remaining.len());
        let line = &remaining[..line_len];
        let trimmed_line = trim_line_ending(line);
        offset += line_len;
        if trimmed_line == "---" {
            return Some((lines, contents[offset..].to_string()));
        }
        lines.push(trimmed_line.to_string());
    }
    None
}

fn parse_frontmatter_entry(raw: &str) -> FrontmatterEntry {
    let Some((key, value)) = raw.split_once(':') else {
        return FrontmatterEntry {
            key: None,
            value: None,
            raw: raw.to_string(),
        };
    };
    let key = normalize_key(key);
    if key.is_empty() {
        return FrontmatterEntry {
            key: None,
            value: None,
            raw: raw.to_string(),
        };
    }
    FrontmatterEntry {
        key: Some(key),
        value: Some(parse_value(value)),
        raw: raw.to_string(),
    }
}

fn trim_line_ending(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

fn value_as_string_set(value: &MarkerValue) -> Option<BTreeSet<String>> {
    match value {
        MarkerValue::List(values) => Some(
            values
                .iter()
                .filter_map(|value| value.as_string().map(normalize_key))
                .filter(|value| !value.is_empty())
                .collect(),
        ),
        MarkerValue::String(value) => {
            let normalized = normalize_key(value);
            (!normalized.is_empty()).then(|| BTreeSet::from([normalized]))
        }
        _ => None,
    }
}

fn is_pipeline_field(key: &str) -> bool {
    PIPELINE_FIELDS.contains(&key)
}

fn is_command_managed_field(key: &str) -> bool {
    COMMAND_MANAGED_FIELDS.contains(&key)
}

fn is_managed_frontmatter_field(key: &str) -> bool {
    is_pipeline_field(key) || is_command_managed_field(key)
}

fn is_standard_user_field(key: &str) -> bool {
    MARKER_REQUIRED_KEYS.contains(&key) || COMMON_USER_FIELDS.contains(&key)
}

fn unknown_synced_fields(projection: &Projection) -> Vec<String> {
    projection
        .keys()
        .filter(|key| !is_standard_user_field(key))
        .cloned()
        .collect()
}

fn projection_snapshot_json(projection: &Projection) -> String {
    let mut object = serde_json::Map::new();
    for (key, value) in projection {
        object.insert(key.clone(), marker_value_to_json(value));
    }
    serde_json::to_string(&serde_json::Value::Object(object))
        .expect("serializing marker projection snapshot cannot fail")
}

fn projection_from_snapshot_json(contents: &str) -> Result<Projection> {
    let value = serde_json::from_str::<serde_json::Value>(contents).map_err(
        |error| {
            CommandError::new(format!(
                "parse {FIELD_MARKER_BASE} JSON projection: {error}"
            ))
        },
    )?;
    let serde_json::Value::Object(object) = value else {
        return Err(CommandError::new(format!(
            "{FIELD_MARKER_BASE} must be a JSON object"
        )));
    };

    let mut projection = Projection::new();
    for (key, value) in object {
        let key = normalize_key(&key);
        if key.is_empty() {
            return Err(CommandError::new(format!(
                "{FIELD_MARKER_BASE} contains an empty key"
            )));
        }
        projection.insert(key, marker_value_from_json(value)?);
    }
    canonicalize_parent(&mut projection, FIELD_MARKER_BASE)?;
    Ok(projection)
}

fn marker_value_to_json(value: &MarkerValue) -> serde_json::Value {
    match value {
        MarkerValue::Null => serde_json::Value::Null,
        MarkerValue::Bool(value) => serde_json::Value::Bool(*value),
        MarkerValue::Number(value) => {
            serde_json::from_str::<serde_json::Value>(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.clone()))
        }
        MarkerValue::String(value) => serde_json::Value::String(value.clone()),
        MarkerValue::List(values) => serde_json::Value::Array(
            values.iter().map(marker_value_to_json).collect(),
        ),
    }
}

fn marker_value_from_json(value: serde_json::Value) -> Result<MarkerValue> {
    match value {
        serde_json::Value::Null => Ok(MarkerValue::Null),
        serde_json::Value::Bool(value) => Ok(MarkerValue::Bool(value)),
        serde_json::Value::Number(value) => {
            Ok(MarkerValue::Number(value.to_string()))
        }
        serde_json::Value::String(value) => Ok(MarkerValue::String(value)),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(marker_value_from_json)
            .collect::<Result<Vec<_>>>()
            .map(MarkerValue::List),
        serde_json::Value::Object(_) => Err(CommandError::new(format!(
            "{FIELD_MARKER_BASE} contains a nested object, which marker projections do not support"
        ))),
    }
}

fn projection_hash(projection: &Projection) -> Result<String> {
    let canonical = serde_json::to_vec(projection).map_err(|error| {
        CommandError::new(format!("serialize marker projection: {error}"))
    })?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

fn ref_note_path(config: &Config, pdf: &Path) -> Result<PathBuf> {
    Ok(config
        .ref_dir
        .join(pdf_path_metadata(config, pdf)?.note_relative_path))
}

fn pdf_path_metadata(config: &Config, pdf: &Path) -> Result<PdfPathMetadata> {
    let stem = pdf.file_stem().and_then(OsStr::to_str).ok_or_else(|| {
        CommandError::new(format!(
            "PDF path has no UTF-8 basename: {}",
            pdf.display()
        ))
    })?;

    if let Ok(relative_pdf_path) = pdf.strip_prefix(&config.lib_dir) {
        let mut note_relative_path = relative_pdf_path.to_path_buf();
        note_relative_path.set_extension("md");
        let ref_type = ref_type_from_relative_pdf_path(pdf, relative_pdf_path)?;
        return Ok(PdfPathMetadata {
            relative_pdf_path: Some(relative_pdf_path.to_path_buf()),
            note_relative_path,
            ref_type,
        });
    }

    Ok(PdfPathMetadata {
        relative_pdf_path: None,
        note_relative_path: PathBuf::from(format!("{stem}.md")),
        ref_type: None,
    })
}

fn ref_type_from_relative_pdf_path(
    pdf: &Path,
    relative_pdf_path: &Path,
) -> Result<Option<String>> {
    let Some(parent) = relative_pdf_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    else {
        return Ok(None);
    };
    let Some(component) = parent.components().find_map(|component| {
        if let Component::Normal(value) = component {
            Some(value)
        } else {
            None
        }
    }) else {
        return Ok(None);
    };
    let ref_type = component.to_str().ok_or_else(|| {
        CommandError::new(format!(
            "PDF path has non-UTF-8 ref_type component: {}",
            pdf.display()
        ))
    })?;
    Ok(Some(ref_type.to_string()))
}

fn source_pdf_value(config: &Config, pdf: &Path) -> String {
    vault_relative_path_value(config, pdf)
}

fn vault_relative_path_value(config: &Config, path: &Path) -> String {
    path.strip_prefix(&config.bob_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|error| {
        CommandError::new(format!(
            "read {} for sha256: {error}",
            path.display()
        ))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "create parent directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let temp_path = temporary_write_path(path)?;
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, contents).map_err(|error| {
        CommandError::new(format!(
            "write temporary file {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        CommandError::new(format!("install file {}: {error}", path.display()))
    })
}

fn atomic_copy(source: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CommandError::new(format!(
                "create parent directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let temp_path = temporary_write_path(dest)?;
    let _ = fs::remove_file(&temp_path);
    fs::copy(source, &temp_path).map_err(|error| {
        CommandError::new(format!(
            "copy image asset {} to temporary file {}: {error}",
            source.display(),
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, dest).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        CommandError::new(format!(
            "install image asset {}: {error}",
            dest.display()
        ))
    })?;
    Ok(())
}

fn temporary_write_path(path: &Path) -> Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        CommandError::new(format!("path has no file name: {}", path.display()))
    })?;

    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", process::id()));
    Ok(path.with_file_name(temp_name))
}

fn render_marker_scalar_string(value: &str) -> String {
    if can_render_plain_marker_scalar_string(value) {
        value.to_string()
    } else {
        quote_string(value)
    }
}

fn render_marker_list_string(value: &str) -> String {
    if can_render_plain_marker_list_string(value) {
        value.to_string()
    } else {
        quote_string(value)
    }
}

fn render_frontmatter_string(value: &str) -> String {
    if can_render_plain_frontmatter_string(value) {
        value.to_string()
    } else {
        quote_string(value)
    }
}

fn can_render_plain_marker_scalar_string(value: &str) -> bool {
    if value.starts_with("[[") && value.ends_with("]]") {
        return true;
    }
    !value.is_empty()
        && value.trim() == value
        && !value
            .chars()
            .any(|character| matches!(character, '\n' | '\r'))
        && !is_marker_typed_literal(value)
        && !value.starts_with(['[', '{', '"', '\'', '-', '*', '#', '!', '&'])
}

fn can_render_plain_marker_list_string(value: &str) -> bool {
    can_render_plain_marker_scalar_string(value)
        && !value.contains(',')
        && (is_wikilink(value)
            || !value
                .chars()
                .any(|character| matches!(character, '[' | ']')))
}

fn can_render_plain_frontmatter_string(value: &str) -> bool {
    can_render_plain_marker_scalar_string(value)
        && !value.chars().any(char::is_whitespace)
        && !value.contains(':')
        && !value.contains('[')
        && !value.contains(']')
}

fn is_marker_typed_literal(value: &str) -> bool {
    matches!(
        value,
        "null" | "true" | "false" | "~" | "Null" | "True" | "False"
    ) || is_number_literal(value)
}

fn quote_string(value: &str) -> String {
    serde_json::to_string(value)
        .expect("serializing string to JSON cannot fail")
}

fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn change_action(
    existed: bool,
    previous: Option<&str>,
    rendered: &str,
) -> &'static str {
    if previous == Some(rendered) {
        "none"
    } else if existed {
        "update"
    } else {
        "create"
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        decode_marker_contents, is_sidecar_marker_mirror, parse_marker,
        parse_note, parse_sidecar_markdown, pdf_path_metadata, projection_hash,
        ref_note_path, render_marker, resolve_under_bob, sidecar_page_heading,
        Config, MarkerValue, PipelineMetadata, Projection,
        SidecarAnnotationKind,
    };

    fn string_value(value: &str) -> MarkerValue {
        MarkerValue::String(value.to_string())
    }

    fn test_projection(entries: Vec<(&str, MarkerValue)>) -> Projection {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    fn test_config() -> Config {
        Config {
            bob_dir: PathBuf::from("/tmp/bob"),
            lib_dir: PathBuf::from("/tmp/bob/lib"),
            ref_dir: PathBuf::from("/tmp/bob/ref"),
        }
    }

    fn test_config_for_bob_dir(bob_dir: PathBuf) -> Config {
        Config {
            lib_dir: bob_dir.join("lib"),
            ref_dir: bob_dir.join("ref"),
            bob_dir,
        }
    }

    fn temp_bob_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "bob-cli-highlights-ref-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp bob dir");
        path
    }

    fn write_test_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create test file parent");
        }
        fs::write(path, contents).expect("write test file");
    }

    #[test]
    fn relative_config_paths_resolve_under_bob_dir() {
        let bob_dir = Path::new("/tmp/bob");

        assert_eq!(
            resolve_under_bob(bob_dir, Path::new("library")),
            PathBuf::from("/tmp/bob/library")
        );
        assert_eq!(
            resolve_under_bob(bob_dir, Path::new("/var/lib/pdfs")),
            PathBuf::from("/var/lib/pdfs")
        );
    }

    #[test]
    fn pipeline_fields_exclude_marker_user_projection() {
        assert!(super::PIPELINE_FIELDS.contains(&"source_pdf"));
        assert!(super::PIPELINE_FIELDS.contains(&"highlights_marker_hash"));
        assert!(super::PIPELINE_FIELDS.contains(&"highlights_marker_base"));
        assert!(!super::PIPELINE_FIELDS.contains(&"status"));
        assert!(!super::PIPELINE_FIELDS.contains(&"parent"));
        assert!(!super::PIPELINE_FIELDS.contains(&"type"));
        assert!(super::is_command_managed_field("type"));
        assert!(super::is_command_managed_field("ref_type"));
    }

    #[test]
    fn projection_snapshot_json_round_trips_compact_user_projection() {
        let projection = test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Systems Performance")),
            (
                "aliases",
                MarkerValue::List(vec![
                    string_value("SP"),
                    string_value("systems perf"),
                ]),
            ),
            ("pages", MarkerValue::Number("42".to_string())),
        ]);

        let snapshot = super::projection_snapshot_json(&projection);
        assert_eq!(
            snapshot,
            r#"{"aliases":["SP","systems perf"],"pages":42,"parent":"[[obsidian]]","status":"wip","title":"Systems Performance"}"#
        );
        assert_eq!(
            super::projection_from_snapshot_json(&snapshot)
                .expect("parse snapshot"),
            projection
        );

        let canonicalized = super::projection_from_snapshot_json(
            r#"{"parent":"obsidian","status":"wip"}"#,
        )
        .expect("parse bare parent snapshot");
        assert_eq!(
            canonicalized.get("parent"),
            Some(&string_value("[[obsidian]]"))
        );
    }

    #[test]
    fn deprecated_done_status_normalizes_to_read_for_synced_inputs() {
        let marker = super::parse_marker_with_normalization(
            "- status: done\n- parent: obsidian\n",
        )
        .expect("parse deprecated marker status");
        assert!(marker.status_normalized);
        assert_eq!(
            marker.projection.get("status"),
            Some(&string_value("read"))
        );

        let note = parse_note(
            "\
---
status: done
parent: obsidian
highlights_marker_base: '{\"parent\":\"obsidian\",\"status\":\"done\"}'
---

Body
",
        );
        let frontmatter = note
            .synced_projection_with_normalization()
            .expect("normalize frontmatter status");
        assert!(frontmatter.status_normalized);
        assert_eq!(
            frontmatter.projection.get("status"),
            Some(&string_value("read"))
        );

        let (base, base_status_normalized) = note
            .marker_base_projection_with_normalization()
            .expect("normalize base status");
        assert!(base_status_normalized);
        assert_eq!(
            base.expect("base projection").get("status"),
            Some(&string_value("read"))
        );
    }

    #[test]
    fn projection_three_way_merge_handles_compatible_changes() {
        let base = test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Old")),
        ]);

        let marker_only = super::merge_projection_changes(
            &base,
            &test_projection(vec![
                ("status", string_value("read")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("Old")),
            ]),
            &base,
        );
        assert!(marker_only.conflicts.is_empty());
        assert_eq!(
            marker_only.projection.get("status"),
            Some(&string_value("read"))
        );
        assert!(marker_only.marker_contributed);
        assert!(!marker_only.frontmatter_contributed);

        let frontmatter_only = super::merge_projection_changes(
            &base,
            &base,
            &test_projection(vec![
                ("status", string_value("wip")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("New")),
            ]),
        );
        assert!(frontmatter_only.conflicts.is_empty());
        assert_eq!(
            frontmatter_only.projection.get("title"),
            Some(&string_value("New"))
        );
        assert!(!frontmatter_only.marker_contributed);
        assert!(frontmatter_only.frontmatter_contributed);

        let same_value = super::merge_projection_changes(
            &base,
            &test_projection(vec![
                ("status", string_value("read")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("Old")),
            ]),
            &test_projection(vec![
                ("status", string_value("read")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("Old")),
            ]),
        );
        assert!(same_value.conflicts.is_empty());
        assert_eq!(
            same_value.projection.get("status"),
            Some(&string_value("read"))
        );
        assert!(same_value.marker_contributed);
        assert!(same_value.frontmatter_contributed);

        let non_overlapping = super::merge_projection_changes(
            &base,
            &test_projection(vec![
                ("status", string_value("read")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("Old")),
            ]),
            &test_projection(vec![
                ("status", string_value("wip")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("New")),
            ]),
        );
        assert!(non_overlapping.conflicts.is_empty());
        assert_eq!(
            non_overlapping.projection,
            test_projection(vec![
                ("status", string_value("read")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("New")),
            ])
        );
    }

    #[test]
    fn projection_three_way_merge_handles_deletes_and_conflicts() {
        let base = test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
            ("title", string_value("Old")),
        ]);
        let without_title = test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
        ]);

        let delete_vs_unchanged =
            super::merge_projection_changes(&base, &without_title, &base);
        assert!(delete_vs_unchanged.conflicts.is_empty());
        assert!(!delete_vs_unchanged.projection.contains_key("title"));

        let delete_vs_change = super::merge_projection_changes(
            &base,
            &without_title,
            &test_projection(vec![
                ("status", string_value("wip")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("New")),
            ]),
        );
        assert_eq!(delete_vs_change.conflicts.len(), 1);
        assert_eq!(delete_vs_change.conflicts[0].key, "title");

        let same_key_different_value = super::merge_projection_changes(
            &base,
            &test_projection(vec![
                ("status", string_value("read")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("Old")),
            ]),
            &test_projection(vec![
                ("status", string_value("abandoned")),
                ("parent", string_value("[[obsidian]]")),
                ("title", string_value("Old")),
            ]),
        );
        assert_eq!(same_key_different_value.conflicts.len(), 1);
        assert_eq!(same_key_different_value.conflicts[0].key, "status");
    }

    #[test]
    fn highlights_ref_task_line_parser_recognizes_generated_pdf_task() {
        assert_eq!(
            super::parse_pdf_task_line("Body without generated task.\n")
                .expect("parse missing task"),
            super::PdfTaskLineState::Missing
        );

        let unchecked = super::parse_pdf_task_line(
            "# Example\n\n- [ ] #task [[lib/books/example.pdf]] [p::2] ^ref\n",
        )
        .expect("parse unchecked task");
        match unchecked {
            super::PdfTaskLineState::Present(task) => {
                assert_eq!(task.line_index, 2);
                assert!(!task.checked);
                assert_eq!(task.status(), super::PdfTaskStatus::Unchecked);
            }
            super::PdfTaskLineState::Missing => panic!("expected task line"),
        }

        let checked = super::parse_pdf_task_line(
            "- [X] #task [[lib/books/example.PDF|Example]] [p::2] ^ref\n",
        )
        .expect("parse checked task");
        match checked {
            super::PdfTaskLineState::Present(task) => {
                assert!(task.checked);
                assert_eq!(task.status(), super::PdfTaskStatus::Read);
            }
            super::PdfTaskLineState::Missing => panic!("expected task line"),
        }

        let cancelled = super::parse_pdf_task_line(
            "- [-] #task [[lib/chat/bulk_obsidian_task_properties.pdf]] [p::2] [cancelled:: 2026-06-04] ^ref\n",
        )
        .expect("parse cancelled task");
        match cancelled {
            super::PdfTaskLineState::Present(task) => {
                assert_eq!(task.mark, '-');
                assert!(!task.checked);
                assert_eq!(task.status(), super::PdfTaskStatus::Abandoned);
            }
            super::PdfTaskLineState::Missing => panic!("expected task line"),
        }

        let legacy_without_priority = super::parse_pdf_task_line(
            "- [ ] #task [[lib/books/example.pdf]] ^ref\n",
        )
        .expect("parse legacy task without priority");
        match legacy_without_priority {
            super::PdfTaskLineState::Present(task) => assert!(!task.checked),
            super::PdfTaskLineState::Missing => panic!("expected task line"),
        }
    }

    #[test]
    fn highlights_ref_task_line_parser_rejects_malformed_and_duplicate_tasks() {
        let missing_tag = super::parse_pdf_task_line(
            "- [ ] [[lib/books/example.pdf]] ^ref\n",
        )
        .expect_err("task without tag should fail");
        assert!(
            missing_tag.to_string().contains("malformed"),
            "{missing_tag}"
        );
        assert!(missing_tag.to_string().contains("[-]"), "{missing_tag}");

        let non_pdf =
            super::parse_pdf_task_line("- [ ] #task [[ref/example.md]] ^ref\n")
                .expect_err("task without PDF link should fail");
        assert!(non_pdf.to_string().contains("malformed"), "{non_pdf}");

        let custom_marker = super::parse_pdf_task_line(
            "- [>] #task [[lib/books/example.pdf]] [p::2] ^ref\n",
        )
        .expect_err("custom task marker should fail");
        assert!(
            custom_marker.to_string().contains("malformed"),
            "{custom_marker}"
        );

        let duplicate = super::parse_pdf_task_line(
            "- [ ] #task [[lib/one.pdf]] ^ref\n- [x] #task [[lib/two.pdf]] ^ref\n",
        )
        .expect_err("duplicate task block id should fail");
        assert!(
            duplicate
                .to_string()
                .contains("multiple generated PDF task"),
            "{duplicate}"
        );
    }

    #[test]
    fn annotation_task_candidates_extract_from_comments_and_notes() {
        let annotations = parse_sidecar_markdown(
            "\
## Page 2

Note: marker note mirrored from the PDF

---

> Quote with a task comment.

- #task Review the contradiction.
- Ordinary comment bullet.

---

Note:
- #task Follow up on the standalone note.
- [x] #task Preserve accepted source checkboxes.
- Untagged standalone bullet.
",
        );
        let sidecar = super::SidecarInput {
            path: PathBuf::from("example.md"),
            annotations,
        };
        let config = test_config();
        let pdf = Path::new("/tmp/bob/lib/example.pdf");
        let ref_note = Path::new("/tmp/bob/ref/example.md");

        let candidates = super::annotation_task_candidates(
            &config,
            ref_note,
            pdf,
            Some(&sidecar),
            None,
        )
        .expect("extract annotation task candidates");

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.task_text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "#task Review the contradiction.",
                "#task Follow up on the standalone note.",
                "#task Preserve accepted source checkboxes.",
            ]
        );
        assert_eq!(
            sidecar.annotations[1].comment.as_deref(),
            Some("#task Review the contradiction.\nOrdinary comment bullet.")
        );

        // The comment task points at the highlight's block; both standalone
        // bullets share the standalone note's block; the two ids differ.
        let highlight_block_id =
            super::annotation_block_id(&config, pdf, &sidecar.annotations[1]);
        let note_block_id =
            super::annotation_block_id(&config, pdf, &sidecar.annotations[2]);
        assert_eq!(candidates[0].source_block_id, highlight_block_id);
        assert_eq!(candidates[1].source_block_id, note_block_id);
        assert_eq!(candidates[2].source_block_id, note_block_id);
        assert_ne!(highlight_block_id, note_block_id);
    }

    #[test]
    fn sidecar_parser_extracts_image_annotations_and_leaves_non_images_as_notes(
    ) {
        let annotations = parse_sidecar_markdown(
            "\
## Page 7

![Figure 1](assets/figure.png)

Comment: Compare this figure with the appendix.
- #task Follow up on the figure.

---

![Diagram](assets/one.svg)
![Table](assets/two.webp)

---

![Not an image](assets/paper.pdf)
",
        );

        assert_eq!(annotations.len(), 4);
        assert_eq!(annotations[0].kind, SidecarAnnotationKind::Image);
        assert_eq!(
            annotations[0]
                .image
                .as_ref()
                .map(|image| image.target.as_str()),
            Some("assets/figure.png")
        );
        assert_eq!(
            annotations[0]
                .image
                .as_ref()
                .and_then(|image| image.alt_text.as_deref()),
            Some("Figure 1")
        );
        assert_eq!(
            annotations[0].comment.as_deref(),
            Some(
                "Compare this figure with the appendix.\n- #task Follow up on the figure."
            )
        );
        assert_eq!(annotations[1].kind, SidecarAnnotationKind::Image);
        assert_eq!(annotations[2].kind, SidecarAnnotationKind::Image);
        assert_eq!(annotations[3].kind, SidecarAnnotationKind::StandaloneNote);
        assert_eq!(annotations[3].text, "![Not an image](assets/paper.pdf)");
    }

    #[test]
    fn sidecar_parser_reads_image_note_from_markdown_title() {
        let annotations = parse_sidecar_markdown(
            "\
## Page 9

![Figure 2](assets/figure.png \"Bryan's note about the figure.\")
",
        );

        assert_eq!(annotations.len(), 1);
        let annotation = &annotations[0];
        assert_eq!(annotation.kind, SidecarAnnotationKind::Image);
        assert_eq!(
            annotation.image.as_ref().map(|image| image.target.as_str()),
            Some("assets/figure.png")
        );
        assert_eq!(
            annotation
                .image
                .as_ref()
                .and_then(|image| image.alt_text.as_deref()),
            Some("Figure 2")
        );
        assert_eq!(
            annotation
                .image
                .as_ref()
                .and_then(|image| image.title.as_deref()),
            Some("Bryan's note about the figure.")
        );
        // The image title becomes the rendered comment; the asset path is
        // never used as note text.
        assert_eq!(
            annotation.comment.as_deref(),
            Some("Bryan's note about the figure.")
        );
        assert_eq!(annotation.text, "Figure 2");
    }

    #[test]
    fn sidecar_parser_prefers_explicit_image_comment_over_title() {
        let annotations = parse_sidecar_markdown(
            "\
## Page 9

![Figure](assets/figure.png \"Title note.\")

Comment: Explicit note below the image.
",
        );

        assert_eq!(annotations.len(), 1);
        let annotation = &annotations[0];
        assert_eq!(annotation.kind, SidecarAnnotationKind::Image);
        // The explicit adjacent line is strongest and comes first; the title
        // is appended as an additional user note line.
        assert_eq!(
            annotation.comment.as_deref(),
            Some("Explicit note below the image.\nTitle note.")
        );
    }

    #[test]
    fn sidecar_parser_does_not_duplicate_matching_image_title_and_comment() {
        let annotations = parse_sidecar_markdown(
            "\
## Page 9

![Figure](assets/figure.png \"Shared note.\")

Comment: Shared note.
",
        );

        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].comment.as_deref(), Some("Shared note."));
    }

    #[test]
    fn sidecar_parser_image_without_note_has_no_comment() {
        let annotations = parse_sidecar_markdown(
            "## Page 9\n\n![Figure 1](assets/figure.png)\n",
        );

        assert_eq!(annotations.len(), 1);
        let annotation = &annotations[0];
        assert_eq!(annotation.kind, SidecarAnnotationKind::Image);
        // Alt text alone is treated as metadata, not a user note, to avoid
        // rendering generic captions as comments.
        assert_eq!(annotation.comment, None);
        assert_eq!(annotation.text, "Figure 1");
    }

    #[test]
    fn render_sidecar_highlights_renders_image_title_note() {
        let bob_dir = temp_bob_dir("image-title-render");
        let sidecar_path = bob_dir.join("lib/books/figures.textbundle/text.md");
        let asset_path =
            bob_dir.join("lib/books/figures.textbundle/assets/figure.png");
        write_test_file(&asset_path, "synthetic image bytes");
        let annotations = parse_sidecar_markdown(
            "\
## Page 4

![Figure](assets/figure.png \"Compare this figure with p.14.\")
",
        );
        let sidecar = super::SidecarInput {
            path: sidecar_path,
            annotations,
        };
        let config = test_config_for_bob_dir(bob_dir.clone());
        let pdf = bob_dir.join("lib/books/figures.pdf");
        let ref_note = bob_dir.join("ref/books/figures.md");
        let note = super::ParsedNote::empty();

        let rendered = super::render_sidecar_highlights(
            &config, &pdf, &ref_note, &note, &sidecar,
        )
        .expect("render image title selection");

        assert_eq!(rendered.count, 1);
        assert_eq!(rendered.image_count, 1);
        let image_asset = &rendered.image_assets[0];
        assert!(
            rendered.content.contains(&format!(
                "> [!quote] Image ![[{}]]\n",
                super::display_path(&image_asset.vault_relative_dest_path)
            )),
            "{}",
            rendered.content
        );
        assert!(
            rendered
                .content
                .contains("> > [!note] Comment Compare this figure with p.14."),
            "{}",
            rendered.content
        );
        // A title-only image note carries no task, since tasks must be
        // list-item lines.
        let candidates = super::annotation_task_candidates(
            &config,
            &ref_note,
            &pdf,
            Some(&sidecar),
            Some(&rendered),
        )
        .expect("extract image title tasks");
        assert!(candidates.is_empty(), "{candidates:?}");
    }

    #[test]
    fn render_sidecar_highlights_renders_image_assets_and_tasks() {
        let bob_dir = temp_bob_dir("image-render");
        let sidecar_path = bob_dir.join("lib/books/figures.textbundle/text.md");
        let asset_path =
            bob_dir.join("lib/books/figures.textbundle/assets/figure.png");
        write_test_file(&asset_path, "synthetic image bytes");
        let annotations = parse_sidecar_markdown(
            "\
## Page 4

Note: marker note mirrored from the PDF

---

![Figure](assets/figure.png)

Comment: Compare this figure.
- #task Revisit this figure.
",
        );
        let sidecar = super::SidecarInput {
            path: sidecar_path,
            annotations,
        };
        let config = test_config_for_bob_dir(bob_dir.clone());
        let pdf = bob_dir.join("lib/books/figures.pdf");
        let ref_note = bob_dir.join("ref/books/figures.md");
        let note = super::ParsedNote::empty();

        let rendered = super::render_sidecar_highlights(
            &config, &pdf, &ref_note, &note, &sidecar,
        )
        .expect("render image selection");

        assert_eq!(rendered.count, 1);
        assert_eq!(rendered.image_count, 1);
        assert_eq!(rendered.image_assets.len(), 1);
        let image_asset = &rendered.image_assets[0];
        assert_eq!(image_asset.action, super::ImageAssetAction::Copy);
        assert_eq!(image_asset.source_path, asset_path);
        assert_eq!(
            image_asset.vault_relative_dest_path.parent(),
            Some(Path::new("ref/books/figures.assets"))
        );
        assert!(
            rendered.content.contains(&format!(
                "> [!quote] Image ![[{}]]\n",
                super::display_path(&image_asset.vault_relative_dest_path)
            )),
            "{}",
            rendered.content
        );
        assert!(
            rendered
                .content
                .contains("> > [!note] Comment Compare this figure."),
            "{}",
            rendered.content
        );
        assert!(
            rendered
                .content
                .contains(&format!("^{}\n", image_asset.block_id)),
            "{}",
            rendered.content
        );

        let candidates = super::annotation_task_candidates(
            &config,
            &ref_note,
            &pdf,
            Some(&sidecar),
            Some(&rendered),
        )
        .expect("extract image comment task");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].task_text, "#task Revisit this figure.");
        assert_eq!(candidates[0].source_block_id, image_asset.block_id);
    }

    #[test]
    fn image_block_id_is_stable_across_asset_renames() {
        let bob_dir = temp_bob_dir("image-id");
        let config = test_config_for_bob_dir(bob_dir.clone());
        let pdf = bob_dir.join("lib/books/figures.pdf");
        let ref_note = bob_dir.join("ref/books/figures.md");
        let note = super::ParsedNote::empty();

        let first_sidecar_path =
            bob_dir.join("lib/books/first.textbundle/text.md");
        write_test_file(
            &bob_dir.join("lib/books/first.textbundle/assets/a.png"),
            "same image bytes",
        );
        let first_sidecar = super::SidecarInput {
            path: first_sidecar_path,
            annotations: parse_sidecar_markdown(
                "## Page 1\n\n![A](assets/a.png)\n",
            ),
        };
        let second_sidecar_path =
            bob_dir.join("lib/books/second.textbundle/text.md");
        write_test_file(
            &bob_dir.join("lib/books/second.textbundle/assets/renamed.png"),
            "same image bytes",
        );
        let second_sidecar = super::SidecarInput {
            path: second_sidecar_path,
            annotations: parse_sidecar_markdown(
                "## Page 1\n\n![A](assets/renamed.png)\n",
            ),
        };

        let first = super::render_sidecar_highlights(
            &config,
            &pdf,
            &ref_note,
            &note,
            &first_sidecar,
        )
        .expect("render first image sidecar");
        let second = super::render_sidecar_highlights(
            &config,
            &pdf,
            &ref_note,
            &note,
            &second_sidecar,
        )
        .expect("render renamed image sidecar");

        assert_eq!(
            first.image_assets[0].block_id,
            second.image_assets[0].block_id
        );
        assert_eq!(
            first.image_assets[0].vault_relative_dest_path,
            second.image_assets[0].vault_relative_dest_path
        );
    }

    #[test]
    fn missing_image_asset_error_points_at_textbundle_export() {
        let bob_dir = temp_bob_dir("image-missing");
        let config = test_config_for_bob_dir(bob_dir.clone());
        let pdf = bob_dir.join("lib/books/missing.pdf");
        let ref_note = bob_dir.join("ref/books/missing.md");
        let sidecar = super::SidecarInput {
            path: bob_dir.join("lib/books/missing.textbundle/text.md"),
            annotations: parse_sidecar_markdown(
                "## Page 1\n\n![Missing](assets/missing.png)\n",
            ),
        };

        let error = super::render_sidecar_highlights(
            &config,
            &pdf,
            &ref_note,
            &super::ParsedNote::empty(),
            &sidecar,
        )
        .expect_err("missing image asset should fail planning");
        assert!(
            error.to_string().contains("image asset not found")
                && error.to_string().contains("TextBundle"),
            "{error}"
        );
    }

    #[test]
    fn pdf_text_artifact_cleanup_normalizes_extraction_noise() {
        assert_eq!(
            super::clean_pdf_text_artifacts(
                "A\u{00a0}\u{2007}\u{202f}B\t\tC \u{fb00}\u{fb01}\u{fb02}\u{fb03}\u{fb04}\u{fb05}\u{fb06}\u{00ad}\u{200b}\u{feff}"
            ),
            "A B C fffiflffifflftst"
        );
    }

    #[test]
    fn beautify_annotation_text_reflows_and_dehyphenates() {
        assert_eq!(
            super::beautify_annotation_text(
                "\
Confusing latency and through-
put leads to mis-sized capa-
city plans for Marie-
Curie and soft\u{00ad}
ware.

Next paragraph keeps a
blank line.
"
            ),
            "\
Confusing latency and throughput leads to mis-sized capacity plans for Marie-Curie and software.

Next paragraph keeps a blank line."
        );
    }

    #[test]
    fn beautify_annotation_text_preserves_list_structure() {
        assert_eq!(
            super::beautify_annotation_text(
                "\
- #task Follow the first wrapped
  continuation line.
* Keep the second item
wrapped too.
+ Plain plus item.
"
            ),
            "\
- #task Follow the first wrapped continuation line.
* Keep the second item wrapped too.
+ Plain plus item."
        );
    }

    #[test]
    fn rendered_annotation_blocks_do_not_include_source_task_anchors() {
        let annotations = parse_sidecar_markdown(
            "\
## Page 2

Note: marker note mirrored from the PDF

---

> Quote with a task comment.

- #task Review the contradiction.
- #task Review the contradiction.
- Ordinary comment bullet.

---

Note:
- #task Follow up on the standalone note.
- [x] #task Preserve accepted source checkboxes.
- Untagged standalone bullet.
",
        );
        let sidecar = super::SidecarInput {
            path: PathBuf::from("example.md"),
            annotations,
        };
        let config = test_config();
        let pdf = Path::new("/tmp/bob/lib/example.pdf");
        let ref_note = Path::new("/tmp/bob/ref/example.md");
        let note = super::ParsedNote::empty();

        let rendered = super::render_sidecar_highlights(
            &config, pdf, ref_note, &note, &sidecar,
        )
        .expect("render annotation blocks");

        assert_eq!(super::generated_block_ids(&rendered.content).len(), 2);
        assert!(!rendered.content.contains(" ^ht-"), "{}", rendered.content);
        let review_lines = rendered
            .content
            .lines()
            .filter(|line| line.contains("#task Review the contradiction."))
            .collect::<Vec<_>>();
        assert_eq!(review_lines.len(), 2, "{}", rendered.content);
        assert_eq!(
            review_lines
                .iter()
                .filter(|line| line.contains(" ^ht-"))
                .count(),
            0,
            "{}",
            rendered.content
        );
        assert!(
            rendered.content.contains(
                "> [!note] - #task Follow up on the standalone note.\n"
            ),
            "{}",
            rendered.content
        );
        assert!(
            rendered
                .content
                .contains("> - Untagged standalone bullet.\n"),
            "{}",
            rendered.content
        );
    }

    #[test]
    fn render_sidecar_highlights_beautifies_callout_text() {
        let annotations = parse_sidecar_markdown(
            "\
## Page 2

Note: marker note mirrored from the PDF

---

> Confusing latency and through-
> put leads to mis-sized capa-
> city plans with \u{fb01}les.

Comment: Compare this with SLO notes.
",
        );
        let sidecar = super::SidecarInput {
            path: PathBuf::from("example.md"),
            annotations,
        };
        let config = test_config();
        let pdf = Path::new("/tmp/bob/lib/example.pdf");
        let ref_note = Path::new("/tmp/bob/ref/example.md");
        let note = super::ParsedNote::empty();

        let rendered = super::render_sidecar_highlights(
            &config, pdf, ref_note, &note, &sidecar,
        )
        .expect("render beautified annotation block");

        assert!(
            rendered.content.contains(
                "> [!quote] Confusing latency and throughput leads to mis-sized capacity plans with files.\n"
            ),
            "{}",
            rendered.content
        );
        assert!(
            rendered
                .content
                .contains("> > [!note] Comment Compare this with SLO notes.\n"),
            "{}",
            rendered.content
        );
        assert_eq!(super::generated_block_ids(&rendered.content).len(), 1);
    }

    #[test]
    fn annotation_block_id_is_stable_across_space_wrapping() {
        let config = test_config();
        let pdf = Path::new("/tmp/bob/lib/example.pdf");
        let mut first = super::SidecarAnnotation {
            kind: SidecarAnnotationKind::Highlight,
            page_label: Some("Page 2".to_string()),
            linked_page_style: true,
            text: "Stable quoted text continues across\nphysical lines."
                .to_string(),
            comment: None,
            task_source: None,
            image: None,
            order: 0,
            ordinal_on_page: 0,
        };
        let mut second = first.clone();
        second.text =
            "Stable quoted text\ncontinues across physical lines.".to_string();

        assert_eq!(
            super::annotation_block_id(&config, pdf, &first),
            super::annotation_block_id(&config, pdf, &second)
        );

        first.comment = Some("Comment changes do not affect IDs.".to_string());
        assert_eq!(
            super::annotation_block_id(&config, pdf, &first),
            super::annotation_block_id(&config, pdf, &second)
        );
    }

    #[test]
    fn annotation_task_route_suffix_is_strict_and_stripped_from_identity() {
        assert_eq!(
            super::split_annotation_task_route_suffix("#task Follow up @alice"),
            ("#task Follow up".to_string(), Some("alice".to_string()))
        );
        assert_eq!(
            super::split_annotation_task_route_suffix("#task Follow @a_b-2"),
            ("#task Follow".to_string(), Some("a_b-2".to_string()))
        );

        for text in [
            "#task Keep @alice.",
            "#task Keep @alice/bob",
            "#task Keep @alice.md",
            "#task Keep @",
            "#task Keep @-alice",
            "#task Keep @..",
            "#task@alice",
        ] {
            assert_eq!(
                super::split_annotation_task_route_suffix(text),
                (text.to_string(), None),
                "{text}"
            );
        }

        assert_eq!(
            super::annotation_task_identity(
                "#task Follow up @alice [created::2026-06-07]"
            )
            .as_deref(),
            Some("#task Follow up")
        );
    }

    #[test]
    fn annotation_task_candidate_records_route_and_processed_id() {
        let bob_dir = temp_bob_dir("candidate-route");
        write_test_file(
            &bob_dir.join("alice.md"),
            "---\nparent: \"[[people]]\"\n---\n",
        );
        let config = test_config_for_bob_dir(bob_dir.clone());
        let ref_note = bob_dir.join("ref/books/task-notes.md");
        let candidates = super::annotation_task_candidates_from_text(
            &config,
            &ref_note,
            "\
- #task Follow up with Alice @alice
- #task Keep unsafe token @alice.md
",
            "h-abc123",
        )
        .expect("extract routed task candidates");

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].task_text, "#task Follow up with Alice");
        assert_eq!(candidates[0].identity, "#task Follow up with Alice");
        assert_eq!(
            candidates[0].target,
            super::AnnotationTaskTarget::RoutedNote(bob_dir.join("alice.md"))
        );
        assert_eq!(
            candidates[0].processed_id,
            super::annotation_task_processed_id(
                &config,
                &ref_note,
                "h-abc123",
                "#task Follow up with Alice",
            )
        );
        let rendered = super::render_annotation_task_line(
            &config,
            &candidates[0],
            "2026-06-07",
        );
        assert!(
            rendered.contains("[[ref/books/task-notes#^h-abc123|"),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("[h:: {}]", candidates[0].processed_id)),
            "new tasks must render short processed marker: {rendered}"
        );
        assert!(
            !rendered.contains("[highlight_task:: "),
            "new tasks must not render legacy processed marker: {rendered}"
        );
        assert!(
            rendered.contains("[created::2026-06-07]"),
            "created date missing: {rendered}"
        );

        assert_eq!(
            candidates[1].target,
            super::AnnotationTaskTarget::ReferenceNote
        );
        assert_eq!(
            candidates[1].task_text,
            "#task Keep unsafe token @alice.md"
        );
        let same_note_rendered = super::render_annotation_task_line(
            &config,
            &candidates[1],
            "2026-06-07",
        );
        assert!(
            same_note_rendered.contains("[[#^h-abc123|"),
            "{same_note_rendered}"
        );
    }

    #[test]
    fn processed_task_index_scans_states_indents_and_done_notes() {
        let bob_dir = temp_bob_dir("processed-index");
        write_test_file(
            &bob_dir.join("root.md"),
            "\
- [ ] #task Active [[ref/books/task#^ht-active|x]] [highlight_task:: active-id]
  - [x] #task Nested child [[ref/books/task#^ht-nested|x]] [h:: nested-id]
- [-] #task Canceled [[ref/books/task#^ht-cancel|x]] @alice [cancelled::2026-06-08]
",
        );
        write_test_file(
            &bob_dir.join("done/alice_done.md"),
            "- [x] #task Archived [[ref/books/task#^h-done|x]] [h:: done-id] [created::2026-06-07]\n",
        );
        write_test_file(
            &bob_dir.join(".git/ignored.md"),
            "- [x] #task Ignored [[ref/books/task#^ht-ignored|x]] [highlight_task:: ignored-id]\n",
        );
        let config = test_config_for_bob_dir(bob_dir);

        let index = super::processed_task_index(&config)
            .expect("build processed index");

        assert!(index.legacy_source_task_anchors.contains("ht-active"));
        assert!(index.legacy_source_task_anchors.contains("ht-nested"));
        assert!(index.legacy_source_task_anchors.contains("ht-cancel"));
        assert!(!index.legacy_source_task_anchors.contains("h-done"));
        assert!(!index.legacy_source_task_anchors.contains("ht-ignored"));
        assert!(index.processed_ids.contains("active-id"));
        assert!(index.processed_ids.contains("nested-id"));
        assert!(index.processed_ids.contains("done-id"));
        assert!(!index.processed_ids.contains("ignored-id"));
        assert!(index.legacy_identities.contains("#task Active"));
        assert!(index.legacy_identities.contains("#task Nested child"));
        assert!(index.legacy_identities.contains("#task Archived"));
        assert!(index.legacy_identities.contains("#task Canceled"));
    }

    #[test]
    fn processed_task_index_legacy_identity_blocks_recreation() {
        let bob_dir = temp_bob_dir("legacy-index");
        write_test_file(
            &bob_dir.join("done/old.md"),
            "- [x] #task Existing follow-up [[ref/books/task#^h-old|x]] [created::2026-06-01]\n",
        );
        let config = test_config_for_bob_dir(bob_dir.clone());
        let mut index = super::processed_task_index(&config)
            .expect("build processed index");
        let ref_note = bob_dir.join("ref/books/task.md");
        let candidate = super::annotation_task_candidates_from_text(
            &config,
            &ref_note,
            "- #task Existing follow-up\n",
            "h-new",
        )
        .expect("extract candidate")
        .pop()
        .expect("candidate");

        assert!(!index.accept(&candidate));
    }

    #[test]
    fn processed_task_index_legacy_ht_backlink_blocks_edited_recreation() {
        let bob_dir = temp_bob_dir("legacy-ht-index");
        let config = test_config_for_bob_dir(bob_dir.clone());
        let ref_note = bob_dir.join("ref/books/task.md");
        let candidate = super::annotation_task_candidates_from_text(
            &config,
            &ref_note,
            "- #task Original follow-up\n",
            "h-source123456",
        )
        .expect("extract candidate")
        .pop()
        .expect("candidate");
        let legacy_anchor =
            super::annotation_task_legacy_source_task_block_id(&candidate);
        write_test_file(
            &bob_dir.join("done/old.md"),
            &format!(
                "- [x] #task Edited archived follow-up [[ref/books/task#^{legacy_anchor}|x]] [created::2026-06-01]\n"
            ),
        );

        let mut index = super::processed_task_index(&config)
            .expect("build processed index");

        assert!(index.legacy_source_task_anchors.contains(&legacy_anchor));
        assert!(!index.processed_ids.contains(&candidate.processed_id));
        assert!(!index.legacy_identities.contains(&candidate.identity));
        assert!(!index.accept(&candidate));
    }

    #[test]
    fn annotation_task_insertion_is_idempotent_and_preserves_existing_states() {
        let body = "\
# Example

- [ ] #task [[lib/example.pdf]] [p::2] ^ref
- [x] #task Existing done [created::2026-06-01] [completion::2026-06-02]
- [-] #task Existing cancelled [created::2026-06-01] [cancelled::2026-06-02] [due::2026-06-03]

## Manual Notes

Keep me here.

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
        let block_id = "h-abc123def456";
        let alias = super::SOURCE_LINK_ALIAS;
        let config = test_config();
        let ref_note = Path::new("/tmp/bob/ref/example.md");
        let candidates = super::annotation_task_candidates_from_text(
            &config,
            ref_note,
            "\
- #task Existing done
- #task Existing cancelled
- #task New follow-up [due::2026-06-08]
",
            block_id,
        )
        .expect("extract annotation task candidates");

        let updated = super::insert_missing_annotation_tasks(
            &config,
            body,
            &candidates,
            "2026-06-07",
        )
        .expect("insert annotation tasks");

        // The created task carries a same-file source backlink followed by the
        // durable processed marker and created date.
        let new_line = format!(
            "- [ ] #task New follow-up [due::2026-06-08] [[#^{block_id}|{alias}]] [h:: {}] [created::2026-06-07]",
            candidates[2].processed_id
        );
        assert!(
            updated.contains(&format!(
                "- [ ] #task [[lib/example.pdf]] [p::2] ^ref\n{new_line}\n"
            )),
            "{updated}"
        );
        assert!(
            !updated.contains("[highlight_task:: "),
            "new tasks must not render legacy processed markers:\n{updated}"
        );
        assert!(
            updated.contains(&format!("[h:: {}]", candidates[2].processed_id)),
            "new tasks must render short processed markers:\n{updated}"
        );
        // The pre-existing link-less tasks are preserved, not recreated.
        assert_eq!(updated.matches("#task Existing done").count(), 1);
        assert_eq!(updated.matches("#task Existing cancelled").count(), 1);
        assert!(updated.contains("## Manual Notes\n\nKeep me here."));
        assert!(updated.contains("## Highlights\n\n<!-- highlights:begin -->"));

        let rerun = super::insert_missing_annotation_tasks(
            &config,
            &updated,
            &candidates,
            "2026-06-07",
        )
        .expect("rerun annotation task insertion");
        assert_eq!(rerun, updated);

        // A completed linked task keeps its link and is not recreated: identity
        // strips the injected block link on the existing-line side.
        let completed_linked = updated.replace(
            &new_line,
            &format!(
                "{} [completion::2026-06-09]",
                new_line.replacen("- [ ]", "- [x]", 1)
            ),
        );
        let rerun_completed = super::insert_missing_annotation_tasks(
            &config,
            &completed_linked,
            &candidates,
            "2026-06-07",
        )
        .expect("rerun completed linked annotation task insertion");
        assert_eq!(rerun_completed, completed_linked);
    }

    #[test]
    fn highlights_ref_pdf_task_status_signal_contributes_abandoned() {
        let base = test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let mut resolution = super::SyncResolution {
            decision: super::SyncDecision {
                source: super::SyncSource::AutoMerge,
                reason: "marker/frontmatter unchanged".to_string(),
                marker_contributed: false,
                frontmatter_contributed: false,
            },
            projection: base.clone(),
        };
        let task = super::parse_pdf_task_line(
            "- [-] #task [[lib/books/example.pdf]] [p::2] ^ref\n",
        )
        .expect("parse cancelled task");

        let signal = super::apply_pdf_task_status_signal(
            &mut resolution,
            &task,
            Some(&base),
            &base,
            &base,
        )
        .expect("apply cancelled task signal");

        assert_eq!(signal.status, super::PdfTaskStatus::Abandoned);
        assert_eq!(signal.status_contributed, Some(super::STATUS_ABANDONED));
        assert_eq!(
            resolution
                .projection
                .get(super::FIELD_STATUS)
                .and_then(MarkerValue::as_string),
            Some(super::STATUS_ABANDONED)
        );
        assert!(resolution.decision.frontmatter_contributed);
        assert!(resolution
            .decision
            .reason
            .contains("cancelled PDF task set status abandoned"));
    }

    #[test]
    fn highlights_ref_task_checkbox_rewrite_and_dirty_allowance_are_narrow() {
        let read_projection = test_projection(vec![
            ("status", string_value("read")),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let abandoned_projection = test_projection(vec![
            ("status", string_value("abandoned")),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let wip_projection = test_projection(vec![
            ("status", string_value("wip")),
            ("parent", string_value("[[obsidian]]")),
        ]);
        let base_body = "\
# Example

- [ ] #task [[lib/example.pdf]] ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
        let checked_body = base_body.replace("- [ ]", "- [X]");
        assert!(super::bodies_differ_only_by_pdf_task_checkbox(
            base_body,
            &checked_body
        ));
        let cancelled_body_from_base = base_body.replace("- [ ]", "- [-]");
        assert!(super::bodies_differ_only_by_pdf_task_checkbox(
            base_body,
            &cancelled_body_from_base
        ));
        assert!(super::bodies_differ_only_by_pdf_task_checkbox(
            &checked_body,
            &cancelled_body_from_base
        ));
        assert!(super::bodies_differ_only_by_pdf_task_checkbox(
            &cancelled_body_from_base,
            base_body
        ));

        let rewritten = super::rewrite_pdf_task_checkbox_for_projection(
            base_body,
            &read_projection,
        )
        .expect("rewrite task checkbox");
        assert!(rewritten.contains("- [x] #task [[lib/example.pdf]] ^ref\n"));

        let prioritized_body = "\
# Example

- [ ] #task [[lib/example.pdf]] [p::2] ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
        let prioritized_rewritten =
            super::rewrite_pdf_task_checkbox_for_projection(
                prioritized_body,
                &read_projection,
            )
            .expect("rewrite prioritized task checkbox");
        assert!(prioritized_rewritten
            .contains("- [x] #task [[lib/example.pdf]] [p::2] ^ref\n"));

        let cancelled_body = "\
# Example

- [-] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] [completion:: 2026-06-05] ^ref

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
        let cancelled_checked =
            super::rewrite_pdf_task_checkbox_for_projection(
                cancelled_body,
                &read_projection,
            )
            .expect("rewrite cancelled task checkbox to checked");
        assert!(cancelled_checked.contains(
            "- [x] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] [completion:: 2026-06-05] ^ref\n"
        ));
        let cancelled_unchecked =
            super::rewrite_pdf_task_checkbox_for_projection(
                cancelled_body,
                &wip_projection,
            )
            .expect("rewrite cancelled task checkbox to unchecked");
        assert!(cancelled_unchecked.contains(
            "- [ ] #task [[lib/example.pdf]] [p::2] [cancelled:: 2026-06-04] [completion:: 2026-06-05] ^ref\n"
        ));
        let unchecked_cancelled =
            super::rewrite_pdf_task_checkbox_for_projection(
                prioritized_body,
                &abandoned_projection,
            )
            .expect("rewrite unchecked task checkbox to cancelled");
        assert!(unchecked_cancelled
            .contains("- [-] #task [[lib/example.pdf]] [p::2] ^ref\n"));

        let unrelated_body = checked_body.replace("## Highlights", "Manual");
        assert!(!super::bodies_differ_only_by_pdf_task_checkbox(
            base_body,
            &unrelated_body
        ));

        let base_note = format!("---\nstatus: wip\n---\n{base_body}");
        let current_note = format!("---\nstatus: read\n---\n{checked_body}");
        assert!(super::changes_confined_to_frontmatter_or_pdf_task_checkbox(
            &base_note,
            &current_note
        ));
    }

    #[test]
    fn pdf_path_metadata_derives_nested_reference_paths() {
        let config = Config {
            bob_dir: PathBuf::from("/tmp/bob"),
            lib_dir: PathBuf::from("/tmp/bob/lib"),
            ref_dir: PathBuf::from("/tmp/bob/ref"),
        };

        let top_level =
            pdf_path_metadata(&config, Path::new("/tmp/bob/lib/example.pdf"))
                .expect("top-level metadata");
        assert_eq!(
            top_level.relative_pdf_path,
            Some(PathBuf::from("example.pdf"))
        );
        assert_eq!(top_level.note_relative_path, PathBuf::from("example.md"));
        assert_eq!(top_level.ref_type, None);
        assert_eq!(
            ref_note_path(&config, Path::new("/tmp/bob/lib/example.pdf"))
                .expect("top-level note path"),
            PathBuf::from("/tmp/bob/ref/example.md")
        );

        let nested = pdf_path_metadata(
            &config,
            Path::new("/tmp/bob/lib/books/example.pdf"),
        )
        .expect("nested metadata");
        assert_eq!(
            nested.relative_pdf_path,
            Some(PathBuf::from("books/example.pdf"))
        );
        assert_eq!(
            nested.note_relative_path,
            PathBuf::from("books/example.md")
        );
        assert_eq!(nested.ref_type.as_deref(), Some("books"));
        assert_eq!(
            ref_note_path(&config, Path::new("/tmp/bob/lib/books/example.pdf"))
                .expect("nested note path"),
            PathBuf::from("/tmp/bob/ref/books/example.md")
        );

        let deeper = pdf_path_metadata(
            &config,
            Path::new("/tmp/bob/lib/books/os/example.PDF"),
        )
        .expect("deeper metadata");
        assert_eq!(
            deeper.note_relative_path,
            PathBuf::from("books/os/example.md")
        );
        assert_eq!(deeper.ref_type.as_deref(), Some("books"));

        let outside =
            pdf_path_metadata(&config, Path::new("/tmp/elsewhere/example.pdf"))
                .expect("outside metadata");
        assert_eq!(outside.relative_pdf_path, None);
        assert_eq!(outside.note_relative_path, PathBuf::from("example.md"));
        assert_eq!(outside.ref_type, None);
        assert_eq!(
            ref_note_path(&config, Path::new("/tmp/elsewhere/example.pdf"))
                .expect("outside note path"),
            PathBuf::from("/tmp/bob/ref/example.md")
        );
    }

    #[test]
    fn sidecar_page_heading_extracts_linked_page_label() {
        assert_eq!(
            sidecar_page_heading(
                "#### [Page 1](highlights://highlights-ref-sync#page=1)"
            )
            .as_deref(),
            Some("Page 1")
        );
        assert_eq!(sidecar_page_heading("## p. 12").as_deref(), Some("p. 12"));
        assert_eq!(sidecar_page_heading("# Systems Performance"), None);
        assert_eq!(sidecar_page_heading("##### 2026-06-03:"), None);
    }

    #[test]
    fn marker_content_decoder_preserves_pdfdoc_line_separators() {
        let contents = lopdf::Object::String(
            b"- status: wip\n- parent: obsidian\r\n- title: Obsidian Docs\r"
                .to_vec(),
            lopdf::StringFormat::Literal,
        );

        assert_eq!(
            decode_marker_contents(&contents)
                .expect("decode literal marker contents"),
            "- status: wip\n- parent: obsidian\n- title: Obsidian Docs\n"
        );
    }

    #[test]
    fn linked_sidecar_parser_keeps_wrapped_quotes_and_marker_mirror() {
        let annotations = parse_sidecar_markdown(
            "\
# Highlights Reference Note Sync

#### [Page 1](highlights://highlights-ref-sync#page=1)

##### 2026-06-03:

> Highlights Reference Note Sync

- status: wip
- parent: obsidian

***

#### [Page 2](highlights://highlights-ref-sync#page=2)

##### 2026-06-03:

> It only writes the PDF marker when frontmatter is the selected
source and --write-pdf is supplied.

***

#### [Page 6](highlights://highlights-ref-sync#page=6)

##### 2026-06-03:

> Comment: Compare this with SLO notes.

Some note...

***
",
        );

        assert_eq!(annotations.len(), 3);
        assert!(is_sidecar_marker_mirror(&annotations[0]));
        assert_eq!(annotations[0].page_label.as_deref(), Some("Page 1"));
        assert!(annotations[0].linked_page_style);
        assert_eq!(
            annotations[0].comment.as_deref(),
            Some("- status: wip\n- parent: obsidian")
        );

        assert_eq!(annotations[1].kind, SidecarAnnotationKind::Highlight);
        assert_eq!(annotations[1].page_label.as_deref(), Some("Page 2"));
        assert!(annotations[1].linked_page_style);
        assert_eq!(
            annotations[1].text,
            "It only writes the PDF marker when frontmatter is the selected\nsource and --write-pdf is supplied."
        );
        assert_eq!(annotations[1].comment, None);

        assert_eq!(annotations[2].page_label.as_deref(), Some("Page 6"));
        assert_eq!(
            annotations[2].text,
            "Comment: Compare this with SLO notes."
        );
        assert_eq!(annotations[2].comment.as_deref(), Some("Some note..."));
    }

    #[test]
    fn linked_sidecar_parser_strips_comment_bullet_markers() {
        let annotations = parse_sidecar_markdown(
            "\
# Highlights Reference Note Sync

#### [Page 2](highlights://highlights-ref-sync#page=2)

##### 2026-06-03:

> A determinism contract keeps replayable tool calls stable.

- Support sase tool call replay?

***

#### [Page 3](highlights://highlights-ref-sync#page=3)

##### 2026-06-03:

> Multi-line bullet comments stay multiline.

- Preserve the first comment line.
- Preserve the second comment line.

***
",
        );

        assert_eq!(annotations.len(), 2);
        assert_eq!(annotations[0].page_label.as_deref(), Some("Page 2"));
        assert_eq!(
            annotations[0].text,
            "A determinism contract keeps replayable tool calls stable."
        );
        assert_eq!(
            annotations[0].comment.as_deref(),
            Some("Support sase tool call replay?")
        );

        assert_eq!(annotations[1].page_label.as_deref(), Some("Page 3"));
        assert_eq!(
            annotations[1].text,
            "Multi-line bullet comments stay multiline."
        );
        assert_eq!(
            annotations[1].comment.as_deref(),
            Some(
                "Preserve the first comment line.\nPreserve the second comment line."
            )
        );
    }

    #[test]
    fn sidecar_quote_continuation_does_not_capture_labeled_comment() {
        let annotations = parse_sidecar_markdown(
            "\
## Page 3

> Stable quoted text.
Comment: revised comment
",
        );

        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].text, "Stable quoted text.");
        assert_eq!(annotations[0].comment.as_deref(), Some("revised comment"));
    }

    #[test]
    fn simple_sidecar_unlabeled_text_after_quote_remains_comment() {
        let annotations = parse_sidecar_markdown(
            "\
## Page 3

> Stable quoted text.
Unlabeled comment
",
        );

        assert_eq!(annotations.len(), 1);
        assert!(!annotations[0].linked_page_style);
        assert_eq!(annotations[0].text, "Stable quoted text.");
        assert_eq!(
            annotations[0].comment.as_deref(),
            Some("Unlabeled comment")
        );
    }

    #[test]
    fn marker_parser_accepts_yaml_subset_and_normalizes_keys() {
        let projection = parse_marker(
            "\
- Status: wip
* aliases: [\"Systems Performance\", linux]
- source-url: https://example.com/book
- parent: obsidian
- rating: 5
- archived: false
",
        )
        .expect("parse marker");

        assert_eq!(
            projection.get("status"),
            Some(&MarkerValue::String("wip".to_string()))
        );
        assert_eq!(
            projection.get("source_url"),
            Some(&MarkerValue::String("https://example.com/book".to_string()))
        );
        assert_eq!(
            projection.get("parent"),
            Some(&MarkerValue::String("[[obsidian]]".to_string()))
        );
        assert_eq!(
            projection.get("rating"),
            Some(&MarkerValue::Number("5".to_string()))
        );
        assert_eq!(projection.get("archived"), Some(&MarkerValue::Bool(false)));
    }

    #[test]
    fn marker_parser_canonicalizes_parent_targets() {
        let bare = parse_marker("- status: wip\n- parent: obsidian\n")
            .expect("parse bare parent marker");
        assert_eq!(
            bare.get("parent"),
            Some(&MarkerValue::String("[[obsidian]]".to_string()))
        );

        let nested = parse_marker("- status: wip\n- parent: projects/foo\n")
            .expect("parse nested parent marker");
        assert_eq!(
            nested.get("parent"),
            Some(&MarkerValue::String("[[projects/foo]]".to_string()))
        );

        let spaced =
            parse_marker("- status: wip\n- parent: Systems Performance\n")
                .expect("parse spaced parent marker");
        assert_eq!(
            spaced.get("parent"),
            Some(&MarkerValue::String("[[Systems Performance]]".to_string()))
        );
    }

    #[test]
    fn marker_parser_rejects_linked_parent_targets() {
        let cases = [
            (
                "- status: wip\n- parent: [[obsidian]]\n",
                "wikilinks are not supported",
            ),
            (
                "- status: wip\n- parent: [[obsidian|Obsidian]]\n",
                "aliases are not supported",
            ),
            (
                "- status: wip\n- parent: ![[obsidian]]\n",
                "embeds are not supported",
            ),
            (
                "- status: wip\n- parent: [[obsidian#^block]]\n",
                "block links are not supported",
            ),
            (
                "- status: wip\n- parent: \"obsidian\"\n",
                "quoted parent values are not supported",
            ),
        ];

        for (marker, expected_error) in cases {
            let error =
                parse_marker(marker).expect_err("linked parent should fail");
            assert!(
                error.to_string().contains(expected_error),
                "expected {expected_error:?} in {error}"
            );
        }
    }

    #[test]
    fn frontmatter_projection_canonicalizes_parent_targets() {
        let note = parse_note(
            "\
---
status: wip
parent: obsidian
---

Body
",
        );
        let projection = note
            .synced_projection_with_normalization()
            .expect("extract frontmatter projection")
            .projection;
        assert_eq!(
            projection.get("parent"),
            Some(&MarkerValue::String("[[obsidian]]".to_string()))
        );

        let marker_projection =
            parse_marker("- status: wip\n- parent: obsidian\n")
                .expect("parse marker");
        let marker_hash =
            projection_hash(&marker_projection).expect("hash marker");
        let frontmatter_hash =
            projection_hash(&projection).expect("hash frontmatter");
        assert_eq!(marker_hash, frontmatter_hash);
    }

    #[test]
    fn parent_canonicalization_rejects_non_scalar_values() {
        let marker_error =
            parse_marker("- status: wip\n- parent: [obsidian]\n")
                .expect_err("list parent marker should fail");
        assert!(
            marker_error
                .to_string()
                .contains("inline lists are not supported"),
            "{marker_error}"
        );

        let note = parse_note(
            "\
---
status: wip
parent: [obsidian]
---

Body
",
        );
        let frontmatter_error = note
            .synced_projection_with_normalization()
            .expect_err("list parent frontmatter should fail");
        assert!(
            frontmatter_error
                .to_string()
                .contains("frontmatter parent must be a scalar note target"),
            "{frontmatter_error}"
        );
    }

    #[test]
    fn marker_parser_rejects_missing_required_keys_type_and_duplicate_status() {
        let missing = parse_marker("- title: Missing\n")
            .expect_err("missing status should fail");
        assert!(missing
            .to_string()
            .contains("missing required marker key: status"));

        let missing_parent = parse_marker("- status: wip\n")
            .expect_err("missing parent should fail");
        assert!(missing_parent
            .to_string()
            .contains("missing required marker key: parent"));

        let marker_type = parse_marker(
            "- status: wip\n- parent: obsidian\n- type: [[book]]\n",
        )
        .expect_err("marker type should fail");
        assert!(marker_type.to_string().contains("command-managed"));

        let marker_ref_type = parse_marker(
            "- status: wip\n- parent: obsidian\n- ref_type: books\n",
        )
        .expect_err("marker ref_type should fail");
        assert!(marker_ref_type.to_string().contains("command-managed"));

        let duplicate =
            parse_marker("- status: wip\n- parent: obsidian\n- Status: done\n")
                .expect_err("duplicate status should fail");
        assert!(duplicate.to_string().contains("duplicate marker key"));
    }

    #[test]
    fn status_validation_rejects_unsupported_and_non_scalar_values() {
        let canonical = parse_marker("- status: read\n- parent: obsidian\n")
            .expect("read status should be supported");
        assert_eq!(
            canonical.get("status"),
            Some(&MarkerValue::String("read".to_string()))
        );

        let unsupported =
            parse_marker("- status: queued\n- parent: obsidian\n")
                .expect_err("unsupported status should fail");
        assert!(
            unsupported
                .to_string()
                .contains("marker has unsupported status \"queued\""),
            "{unsupported}"
        );

        let non_scalar = parse_marker("- status: [wip]\n- parent: obsidian\n")
            .expect_err("list status should fail");
        assert!(
            non_scalar
                .to_string()
                .contains("marker status must be a scalar string"),
            "{non_scalar}"
        );
    }

    #[test]
    fn marker_renderer_uses_stable_key_order() {
        let mut projection = Projection::new();
        projection.insert(
            "z_custom".to_string(),
            MarkerValue::String("last".to_string()),
        );
        projection.insert(
            "status".to_string(),
            MarkerValue::String("wip".to_string()),
        );
        projection.insert(
            "title".to_string(),
            MarkerValue::String("Systems Performance".to_string()),
        );
        projection.insert(
            "parent".to_string(),
            MarkerValue::String("[[obsidian]]".to_string()),
        );

        assert_eq!(
            render_marker(&projection).expect("render marker"),
            "\
- status: wip
- parent: obsidian
- title: Systems Performance
- z_custom: last
"
        );
    }

    #[test]
    fn marker_renderer_rejects_unrepresentable_parent_links() {
        let mut projection = Projection::new();
        projection.insert(
            "status".to_string(),
            MarkerValue::String("wip".to_string()),
        );
        projection.insert(
            "parent".to_string(),
            MarkerValue::String("[[obsidian|Obsidian]]".to_string()),
        );

        let error =
            render_marker(&projection).expect_err("alias parent should fail");
        assert!(
            error.to_string().contains(
                "parent cannot be rendered as a PDF marker bare note target"
            ),
            "{error}"
        );
    }

    #[test]
    fn frontmatter_projection_uses_marker_fields_without_fallback_parent() {
        let note = parse_note(
            "\
---
status: wip
title: Existing
type: \"[[old-type]]\"
custom_flag: true
highlights_marker_fields: [custom_flag]
source_pdf: lib/example.pdf
---

Body
",
        );

        let projection = note
            .synced_projection_with_normalization()
            .expect("extract frontmatter projection")
            .projection;
        assert!(!projection.contains_key("parent"));
        assert!(!projection.contains_key("type"));
        assert_eq!(
            projection.get("custom_flag"),
            Some(&MarkerValue::Bool(true))
        );
        assert!(!projection.contains_key("source_pdf"));
    }

    #[test]
    fn frontmatter_render_preserves_unmanaged_keys() {
        let note = parse_note(
            "\
---
status: legacy
type: \"[[old-type]]\"
owner: Bryan
---

Manual body.
",
        );
        let mut projection = Projection::new();
        projection.insert(
            "status".to_string(),
            MarkerValue::String("wip".to_string()),
        );
        projection.insert(
            "parent".to_string(),
            MarkerValue::String("[[obsidian]]".to_string()),
        );
        let hash = projection_hash(&projection).expect("hash projection");
        let rendered = note.render_with_projection(
            &projection,
            &hash,
            &PipelineMetadata {
                source_pdf: "lib/example.pdf".to_string(),
                source_pdf_sha256: "abc123".to_string(),
                ref_type: None,
                highlights_sidecar: None,
                highlights_count: None,
                highlights_synced_at: None,
            },
            &note.body,
        );

        assert!(rendered.contains("status: wip\n"));
        assert!(rendered.contains("type: \"[[ref]]\"\n"));
        assert!(!rendered.contains("type: \"[[old-type]]\"\n"));
        assert!(rendered.contains("owner: Bryan\n"));
        assert!(rendered.contains("source_pdf: lib/example.pdf\n"));
        assert!(rendered.ends_with("\nManual body.\n"));
    }
}
