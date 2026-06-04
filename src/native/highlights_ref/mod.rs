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

use chrono::{SecondsFormat, Utc};
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

use super::{env as bob_env, ob};

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
const PDF_TASK_BLOCK_ID: &str = "^task";
const PDF_TASK_TAG: &str = "#task";
const PIPELINE_VERSION: &str = "highlights-ref-mvp-3";
const REMOVED_HIGHLIGHTS_HEADING: &str = "### Removed highlights";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdfTaskLineState {
    Missing,
    Present(PdfTaskLine),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PdfTaskCompletion {
    found: bool,
    checked: bool,
    status_contributed: bool,
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
    order: usize,
    ordinal_on_page: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarPageHeading {
    label: String,
    linked_page_style: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidecarAnnotationKind {
    Highlight,
    StandaloneNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedHighlights {
    content: String,
    count: usize,
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
    rendered_body: String,
    stable_rendered_note: String,
    stable_note_action: &'static str,
    pdf_task_completion: PdfTaskCompletion,
    status_normalization: StatusNormalization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncWriteReport {
    note_action: &'static str,
    marker_action: &'static str,
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
    report_result(scan_library(&config, options, jobs_from_matches(matches)))
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
    let plan = plan_pdf_sync(config, pdf, options)?;
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
) -> Result<()> {
    let pdfs = collect_pdf_paths(config)?;
    validate_output_collisions(config, &pdfs)?;

    // `plan_pdf_sync` is a pure, read-only computation over an independent
    // `&Config` and one PDF path, so planning is embarrassingly parallel. We
    // collect into a position-keyed vector and reassemble in `pdfs` order so
    // reporting output stays deterministic regardless of completion order.
    let plan_outcomes = pdfs
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
    let plans = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    let plan_failures = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(_) => None,
            ScanPlanOutcome::Failed(failure) => Some(failure),
        })
        .collect::<Vec<_>>();

    print_config_report("scan", config);
    println!("dry_run: {}", options.dry_run);
    println!("write_pdfs: {}", options.write_pdf);
    println!("ob_sync: not-run");
    println!("pdf_count: {}", pdfs.len());
    for outcome in &plan_outcomes {
        match outcome {
            ScanPlanOutcome::Planned(plan) => print_scan_plan_entry(plan),
            ScanPlanOutcome::Failed(failure) => {
                print_scan_plan_failure_entry(failure);
            }
        }
    }

    if options.dry_run {
        print_scan_plan_summary(&plans, plan_failures.len());
        println!("writes: none");
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
    for failure in &write_failures {
        print_scan_write_failure_entry(failure);
    }
    print_scan_write_summary(
        &reports,
        plan_failures.len(),
        write_failures.len(),
    );
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
    let pdf_task_completion = apply_pdf_task_completion_signal(
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
        .map(|sidecar| render_sidecar_highlights(config, pdf, &note, sidecar))
        .transpose()?;
    let sidecar_path = sidecar.as_ref().map(|sidecar| sidecar.path.clone());
    let rendered_highlights_count =
        rendered_highlights.as_ref().map(|rendered| rendered.count);
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
        rendered_body,
        stable_rendered_note,
        stable_note_action,
        pdf_task_completion,
        status_normalization,
    })
}

fn execute_pdf_sync(
    config: &Config,
    plan: &PdfSyncPlan,
) -> Result<SyncWriteReport> {
    if note_write_planned(plan) {
        ensure_note_unchanged_for_write(plan)?;
    }
    if plan.marker_write_needed {
        ensure_pdf_unchanged_for_write(plan)?;
        write_pdf_marker(
            &plan.pdf,
            plan.marker.annotation_id,
            &plan.rendered_marker,
        )?;
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

    Ok(SyncWriteReport {
        note_action,
        marker_action: if plan.marker_write_needed {
            "updated"
        } else {
            "none"
        },
    })
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
    println!(
        "pdf_task: {}",
        pdf_task_completion_label(plan.pdf_task_completion)
    );
    if plan.pdf_task_completion.status_contributed {
        println!("pdf_task_contribution: status=read");
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
}

fn print_sync_write_report(report: SyncWriteReport) {
    println!("note_action: {}", report.note_action);
    println!("pdf_marker_action: {}", report.marker_action);
    println!("writes: {}", write_summary(report));
}

fn write_summary(report: SyncWriteReport) -> &'static str {
    match (report.note_action, report.marker_action != "none") {
        ("none", false) => "none",
        ("none", true) => "pdf",
        (_, false) => "note",
        (_, true) => "note,pdf",
    }
}

fn pdf_task_completion_label(completion: PdfTaskCompletion) -> &'static str {
    match (completion.found, completion.checked) {
        (false, _) => "missing",
        (true, false) => "unchecked",
        (true, true) => "checked",
    }
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
    println!(
        "  pdf_task: {}",
        pdf_task_completion_label(plan.pdf_task_completion)
    );
    if plan.pdf_task_completion.status_contributed {
        println!("  pdf_task_contribution: status=read");
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
    let creates = plans
        .iter()
        .filter(|plan| plan.stable_note_action == "create")
        .count();
    let updates = plans
        .iter()
        .filter(|plan| plan.stable_note_action == "update")
        .count();
    let unchanged = plans
        .iter()
        .filter(|plan| plan.stable_note_action == "none")
        .count();
    let marker_updates =
        plans.iter().filter(|plan| plan.marker_write_needed).count();
    println!("summary:");
    println!("  notes_create: {creates}");
    println!("  notes_update: {updates}");
    println!("  notes_unchanged: {unchanged}");
    println!("  pdf_markers_would_update: {marker_updates}");
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
    let creates = reports
        .iter()
        .filter(|report| report.note_action == "create")
        .count();
    let updates = reports
        .iter()
        .filter(|report| report.note_action == "update")
        .count();
    let unchanged = reports
        .iter()
        .filter(|report| report.note_action == "none")
        .count();
    let marker_updates = reports
        .iter()
        .filter(|report| report.marker_action != "none")
        .count();
    println!("summary:");
    println!("  notes_created: {creates}");
    println!("  notes_updated: {updates}");
    println!("  notes_unchanged: {unchanged}");
    println!("  pdf_markers_updated: {marker_updates}");
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
    let note_writes = reports.iter().any(|report| report.note_action != "none");
    let marker_writes =
        reports.iter().any(|report| report.marker_action != "none");
    println!(
        "writes: {}",
        match (note_writes, marker_writes) {
            (false, false) => "none",
            (true, false) => "note",
            (false, true) => "pdf",
            (true, true) => "note,pdf",
        }
    );
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
            "{} PDF(s) do not have a Highlights Markdown sidecar",
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
    if base_task.checked == current_task.checked {
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
    if let Some(mut annotation) =
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
) -> Option<SidecarAnnotation> {
    let lines = trim_blank_lines(chunk);
    if lines.is_empty() || lines.iter().all(|line| is_markdown_heading(line)) {
        return None;
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
            return None;
        }
        let comment_lines = lines[index..]
            .iter()
            .filter(|line| !is_markdown_heading(line))
            .cloned()
            .collect::<Vec<_>>();
        let comment = normalize_annotation_text(&comment_lines);

        return Some(SidecarAnnotation {
            kind: SidecarAnnotationKind::Highlight,
            page_label: page_label.map(str::to_string),
            linked_page_style,
            text,
            comment: (!comment.is_empty())
                .then(|| strip_comment_label(&comment)),
            order: 0,
            ordinal_on_page: 0,
        });
    }

    let note_lines = lines
        .iter()
        .filter(|line| !is_markdown_heading(line))
        .map(|line| strip_standalone_note_marker(line))
        .collect::<Vec<_>>();
    let text = normalize_annotation_text(&note_lines);
    (!text.is_empty()).then(|| SidecarAnnotation {
        kind: SidecarAnnotationKind::StandaloneNote,
        page_label: page_label.map(str::to_string),
        linked_page_style,
        text,
        comment: None,
        order: 0,
        ordinal_on_page: 0,
    })
}

fn render_sidecar_highlights(
    config: &Config,
    pdf: &Path,
    note: &ParsedNote,
    sidecar: &SidecarInput,
) -> Result<RenderedHighlights> {
    let existing_ids = note.generated_block_ids()?;
    let mut current_ids = BTreeSet::new();
    let mut rendered = String::new();
    let mut current_page = None;
    let mut skipped_marker_note = false;

    for annotation in &sidecar.annotations {
        if !skipped_marker_note && is_sidecar_marker_mirror(annotation) {
            skipped_marker_note = true;
            continue;
        }

        let block_id = annotation_block_id(config, pdf, annotation);
        current_ids.insert(block_id.clone());

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

        rendered.push_str(&render_annotation_block(annotation, &block_id));
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
            rendered.push_str("> [removed] This annotation is no longer present in the Highlights sidecar.\n\n");
            rendered.push('^');
            rendered.push_str(&block_id);
            rendered.push_str("\n\n");
        }
    }

    Ok(RenderedHighlights {
        content: rendered,
        count: current_ids.len(),
    })
}

fn annotation_block_id(
    config: &Config,
    pdf: &Path,
    annotation: &SidecarAnnotation,
) -> String {
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

fn render_annotation_block(
    annotation: &SidecarAnnotation,
    block_id: &str,
) -> String {
    let mut rendered = String::new();
    match annotation.kind {
        SidecarAnnotationKind::Highlight => {
            push_blockquote(&mut rendered, None, &annotation.text);
            if let Some(comment) = &annotation.comment {
                rendered.push_str(">\n");
                push_blockquote(&mut rendered, Some("[comment] "), comment);
            }
        }
        SidecarAnnotationKind::StandaloneNote => {
            push_blockquote(&mut rendered, Some("[note] "), &annotation.text);
        }
    }
    rendered.push('\n');
    rendered.push('^');
    rendered.push_str(block_id);
    rendered.push_str("\n\n");
    rendered
}

fn push_blockquote(
    rendered: &mut String,
    first_prefix: Option<&str>,
    text: &str,
) {
    for (index, line) in text.lines().enumerate() {
        rendered.push_str("> ");
        if index == 0
            && let Some(prefix) = first_prefix
        {
            rendered.push_str(prefix);
        }
        rendered.push_str(line);
        rendered.push('\n');
    }
}

impl SidecarAnnotationKind {
    fn as_str(self) -> &'static str {
        match self {
            SidecarAnnotationKind::Highlight => "highlight",
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
    let trimmed = text.trim();
    for prefix in ["Comment:", "comment:", "Note:", "note:"] {
        if let Some(value) = trimmed.strip_prefix(prefix) {
            return strip_comment_list_markers(value.trim_start());
        }
    }
    strip_comment_list_markers(text)
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
    ["- ", "* "]
        .iter()
        .find_map(|marker| trimmed.strip_prefix(marker))
        .map(str::trim_start)
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

fn normalized_identity_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
                "reference note has multiple generated PDF task lines with ^task block ID",
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
        "generated PDF task line on line {} is malformed; expected '- [ ] #task [[...pdf]] ^task' or '- [x] #task [[...pdf]] ^task'",
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

fn apply_pdf_task_completion_signal(
    resolution: &mut SyncResolution,
    task_line: &PdfTaskLineState,
    base_projection: Option<&Projection>,
    marker_projection: &Projection,
    frontmatter_projection: &Projection,
) -> Result<PdfTaskCompletion> {
    let checked = matches!(
        task_line,
        PdfTaskLineState::Present(PdfTaskLine { checked: true, .. })
    );
    let mut completion = PdfTaskCompletion {
        found: matches!(task_line, PdfTaskLineState::Present(_)),
        checked,
        status_contributed: false,
    };
    if !checked || projection_status_is_read(&resolution.projection) {
        return Ok(completion);
    }

    let conflicts = checked_task_status_conflicts(
        base_projection,
        marker_projection,
        frontmatter_projection,
    );
    if !conflicts.is_empty() {
        return Err(checked_task_status_conflict_error(&conflicts));
    }

    resolution.projection.insert(
        FIELD_STATUS.to_string(),
        MarkerValue::String(STATUS_READ.to_string()),
    );
    resolution.decision.frontmatter_contributed = true;
    if !resolution.decision.reason.is_empty() {
        resolution.decision.reason.push_str("; ");
    }
    resolution
        .decision
        .reason
        .push_str("checked PDF task set status read");
    completion.status_contributed = true;
    Ok(completion)
}

#[derive(Debug, Clone)]
struct CheckedTaskStatusConflict {
    source: &'static str,
    base: Option<MarkerValue>,
    value: Option<MarkerValue>,
}

fn checked_task_status_conflicts(
    base_projection: Option<&Projection>,
    marker_projection: &Projection,
    frontmatter_projection: &Projection,
) -> Vec<CheckedTaskStatusConflict> {
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
        if value != base && !status_value_is_read(value) {
            conflicts.push(CheckedTaskStatusConflict {
                source,
                base: base.cloned(),
                value: value.cloned(),
            });
        }
    }
    conflicts
}

fn checked_task_status_conflict_error(
    conflicts: &[CheckedTaskStatusConflict],
) -> CommandError {
    let mut message = String::from(
        "checked PDF task conflicts with marker/frontmatter status edit:",
    );
    for conflict in conflicts.iter().take(4) {
        message.push_str(&format!(
            "\n  {} status={}, base={}",
            conflict.source,
            diagnostic_projection_value(conflict.value.as_ref()),
            diagnostic_projection_value(conflict.base.as_ref()),
        ));
    }
    message.push_str(
        "\nuncheck the PDF task or set the marker/frontmatter status to read",
    );
    CommandError::new(message)
}

fn projection_status_is_read(projection: &Projection) -> bool {
    status_value_is_read(projection.get(FIELD_STATUS))
}

fn status_value_is_read(value: Option<&MarkerValue>) -> bool {
    value.and_then(MarkerValue::as_string) == Some(STATUS_READ)
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
            return rewrite_pdf_task_checkbox(
                &self.body,
                projection_status_is_read(projection),
            );
        };
        let replacement = rendered_highlights.content.as_str();
        let body = replace_managed_region(&self.body, replacement)?;
        rewrite_pdf_task_checkbox(&body, projection_status_is_read(projection))
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
    if projection_status_is_read(projection) {
        body.push_str("- [x] #task [[");
    } else {
        body.push_str("- [ ] #task [[");
    }
    body.push_str(source_pdf);
    body.push_str("]] ^task\n\n");
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

fn rewrite_pdf_task_checkbox(body: &str, checked: bool) -> Result<String> {
    replace_pdf_task_checkbox_mark(body, if checked { 'x' } else { ' ' })
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
    pdf.strip_prefix(&config.bob_dir)
        .unwrap_or(pdf)
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
    use std::path::{Path, PathBuf};

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
            "# Example\n\n- [ ] #task [[lib/books/example.pdf]] ^task\n",
        )
        .expect("parse unchecked task");
        match unchecked {
            super::PdfTaskLineState::Present(task) => {
                assert_eq!(task.line_index, 2);
                assert!(!task.checked);
            }
            super::PdfTaskLineState::Missing => panic!("expected task line"),
        }

        let checked = super::parse_pdf_task_line(
            "- [X] #task [[lib/books/example.PDF|Example]] ^task\n",
        )
        .expect("parse checked task");
        match checked {
            super::PdfTaskLineState::Present(task) => assert!(task.checked),
            super::PdfTaskLineState::Missing => panic!("expected task line"),
        }
    }

    #[test]
    fn highlights_ref_task_line_parser_rejects_malformed_and_duplicate_tasks() {
        let missing_tag = super::parse_pdf_task_line(
            "- [ ] [[lib/books/example.pdf]] ^task\n",
        )
        .expect_err("task without tag should fail");
        assert!(
            missing_tag.to_string().contains("malformed"),
            "{missing_tag}"
        );

        let non_pdf = super::parse_pdf_task_line(
            "- [ ] #task [[ref/example.md]] ^task\n",
        )
        .expect_err("task without PDF link should fail");
        assert!(non_pdf.to_string().contains("malformed"), "{non_pdf}");

        let duplicate = super::parse_pdf_task_line(
            "- [ ] #task [[lib/one.pdf]] ^task\n- [x] #task [[lib/two.pdf]] ^task\n",
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
    fn highlights_ref_task_checkbox_rewrite_and_dirty_allowance_are_narrow() {
        let base_body = "\
# Example

- [ ] #task [[lib/example.pdf]] ^task

## Highlights

<!-- highlights:begin -->

<!-- highlights:end -->
";
        let checked_body = base_body.replace("- [ ]", "- [X]");
        assert!(super::bodies_differ_only_by_pdf_task_checkbox(
            base_body,
            &checked_body
        ));

        let rewritten = super::rewrite_pdf_task_checkbox(base_body, true)
            .expect("rewrite task checkbox");
        assert!(rewritten.contains("- [x] #task [[lib/example.pdf]] ^task\n"));

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
