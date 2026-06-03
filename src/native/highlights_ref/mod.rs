use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error as StdError,
    ffi::{OsStr, OsString},
    fmt, fs, io, iter,
    path::{Path, PathBuf},
    process::{self, Output, Stdio},
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

const COMMAND_NAME: &str = "bob highlights-ref";
const DEFAULT_LIB_DIR: &str = "lib";
const DEFAULT_REF_DIR: &str = "ref";

const ENV_LIB_DIR: &str = "BOB_HIGHLIGHTS_LIB_DIR";
const ENV_REF_DIR: &str = "BOB_HIGHLIGHTS_REF_DIR";

const FIELD_STATUS: &str = "status";
const FIELD_PARENT: &str = "parent";
const FIELD_NOTE_TYPE: &str = "type";
const NOTE_TYPE_VALUE: &str = "[[ref]]";
const MARKER_REQUIRED_KEYS: &[&str] = &[FIELD_STATUS, FIELD_PARENT];
const MANAGED_BODY_BEGIN: &str = "<!-- highlights:begin -->";
const MANAGED_BODY_END: &str = "<!-- highlights:end -->";
const PIPELINE_VERSION: &str = "highlights-ref-mvp-3";
const REMOVED_HIGHLIGHTS_HEADING: &str = "### Removed highlights";
const TEXTBUNDLE_TEXT_FILES: &[&str] = &["text.md", "text.markdown"];

const FIELD_SOURCE_PDF: &str = "source_pdf";
const FIELD_SOURCE_PDF_SHA256: &str = "source_pdf_sha256";
const FIELD_HIGHLIGHTS_SIDECAR: &str = "highlights_sidecar";
const FIELD_HIGHLIGHTS_COUNT: &str = "highlights_count";
const FIELD_HIGHLIGHTS_SYNCED_AT: &str = "highlights_synced_at";
const FIELD_MARKER_HASH: &str = "highlights_marker_hash";
const FIELD_MARKER_FIELDS: &str = "highlights_marker_fields";
const FIELD_PIPELINE_VERSION: &str = "pipeline_version";

const PIPELINE_FIELDS: &[&str] = &[
    FIELD_SOURCE_PDF,
    FIELD_SOURCE_PDF_SHA256,
    FIELD_HIGHLIGHTS_SIDECAR,
    FIELD_HIGHLIGHTS_COUNT,
    FIELD_HIGHLIGHTS_SYNCED_AT,
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

#[derive(Debug, Clone, PartialEq, Serialize)]
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
}

#[derive(Debug, Clone)]
struct PipelineMetadata {
    source_pdf: String,
    source_pdf_sha256: String,
    highlights_sidecar: Option<MarkerValue>,
    highlights_count: Option<MarkerValue>,
    highlights_synced_at: Option<MarkerValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncSource {
    Marker,
    Frontmatter,
}

#[derive(Debug, Clone)]
struct SyncDecision {
    source: SyncSource,
    reason: &'static str,
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
    text: String,
    comment: Option<String>,
    order: usize,
    ordinal_on_page: usize,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncWriteReport {
    note_action: &'static str,
    marker_action: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GitStatus {
    MissingCommand,
    NotWorktree,
    Worktree { entries: Vec<String> },
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
        write_pdf: false,
        prefer: None,
    };
    report_result(scan_library(&config, options))
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

fn scan_library(config: &Config, options: SyncOptions) -> Result<()> {
    let pdfs = collect_pdf_paths(config)?;
    validate_output_collisions(config, &pdfs)?;

    let mut plans = Vec::new();
    let mut errors = Vec::new();
    for pdf in &pdfs {
        match plan_pdf_sync(config, pdf, options) {
            Ok(plan) => plans.push(plan),
            Err(error) => {
                errors.push(format!("{}: {error}", pdf.display()));
            }
        }
    }

    if !errors.is_empty() {
        return Err(CommandError::new(format!(
            "scan failed before writes:\n  {}",
            errors.join("\n  ")
        )));
    }

    print_config_report("scan", config);
    println!("dry_run: {}", options.dry_run);
    println!("ob_sync: not-run");
    println!("pdf_count: {}", plans.len());
    for plan in &plans {
        print_scan_plan_entry(plan);
    }

    if options.dry_run {
        print_scan_plan_summary(&plans);
        println!("writes: none");
        return Ok(());
    }

    ensure_safe_to_write(config, plans.iter())?;
    let mut reports = Vec::new();
    for plan in &plans {
        reports.push(execute_pdf_sync(config, plan)?);
    }
    print_scan_write_summary(&reports);
    Ok(())
}

fn plan_pdf_sync(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
) -> Result<PdfSyncPlan> {
    let marker = read_pdf_marker(pdf)?;
    let marker_projection = parse_marker(&marker.contents)?;
    let note_path = ref_note_path(config, pdf)?;
    validate_note_target(&note_path)?;
    let note = read_note(&note_path)?;
    let frontmatter_projection = note.synced_projection();
    let marker_hash = projection_hash(&marker_projection)?;
    let frontmatter_hash = projection_hash(&frontmatter_projection)?;
    let last_hash = note.marker_hash();
    let decision = decide_sync_source(
        last_hash.as_deref(),
        &marker_hash,
        &frontmatter_hash,
        note.exists(),
        options.prefer,
    )?;

    let synced_projection = match decision.source {
        SyncSource::Marker => marker_projection.clone(),
        SyncSource::Frontmatter => {
            validate_required_marker_keys(
                &frontmatter_projection,
                "frontmatter",
            )?;
            frontmatter_projection.clone()
        }
    };
    let synced_hash = projection_hash(&synced_projection)?;
    let rendered_marker = render_marker(&synced_projection);
    let marker_write_needed = decision.source == SyncSource::Frontmatter
        && normalize_line_endings(&rendered_marker)
            != normalize_line_endings(&marker.contents);

    if marker_write_needed && !options.write_pdf && !options.dry_run {
        return Err(CommandError::new(
            "frontmatter changed but --write-pdf was not supplied; refusing to update the PDF marker",
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
    })
}

fn execute_pdf_sync(
    config: &Config,
    plan: &PdfSyncPlan,
) -> Result<SyncWriteReport> {
    if plan.marker_write_needed {
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
        let metadata = pipeline_metadata(
            config,
            &plan.pdf,
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

fn print_scan_plan_summary(plans: &[PdfSyncPlan]) {
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
}

fn print_scan_write_summary(reports: &[SyncWriteReport]) {
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

fn show_marker(config: &Config, pdf: &Path) -> Result<()> {
    let marker = read_pdf_marker(pdf)?;
    let projection = parse_marker(&marker.contents)?;
    print_config_report("marker", config);
    println!("pdf: {}", pdf.display());
    println!("marker_page: {}", marker.page_number);
    println!("marker_note: {}", marker.note_number);
    println!("marker_raw:");
    print!("{}", marker.contents);
    if !marker.contents.ends_with('\n') {
        println!();
    }
    println!("marker_rendered:");
    print!("{}", render_marker(&projection));
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
                println!("  {entry}");
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
    let mut touched_paths = BTreeSet::new();
    for plan in plans {
        if plan.stable_note_action != "none" {
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
            Err(CommandError::new(format!(
                "refusing to modify dirty vault files:\n  {}\ncommit, stash, or clean these files before rerunning",
                entries.join("\n  ")
            )))
        }
        _ => Ok(()),
    }
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
            .map(str::to_string)
            .collect(),
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
    let mut order = 0usize;
    let mut page_ordinals: BTreeMap<String, usize> = BTreeMap::new();

    for line in contents.lines() {
        if let Some(next_page_label) = sidecar_page_heading(line) {
            flush_sidecar_chunk(
                &mut annotations,
                &mut chunk,
                page_label.as_deref(),
                &mut order,
                &mut page_ordinals,
            );
            page_label = Some(next_page_label);
            continue;
        }

        if is_horizontal_rule(line) {
            flush_sidecar_chunk(
                &mut annotations,
                &mut chunk,
                page_label.as_deref(),
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
        &mut order,
        &mut page_ordinals,
    );
    annotations
}

fn flush_sidecar_chunk(
    annotations: &mut Vec<SidecarAnnotation>,
    chunk: &mut Vec<String>,
    page_label: Option<&str>,
    order: &mut usize,
    page_ordinals: &mut BTreeMap<String, usize>,
) {
    if let Some(mut annotation) = parse_sidecar_chunk(chunk, page_label) {
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
        while index < lines.len() {
            let trimmed = lines[index].trim_start();
            if trimmed.starts_with('>') {
                quote_lines.push(strip_blockquote_marker(trimmed));
                index += 1;
                continue;
            }
            if trimmed.is_empty()
                && lines[index + 1..]
                    .iter()
                    .any(|line| line.trim_start().starts_with('>'))
            {
                quote_lines.push(String::new());
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
        if annotation.kind == SidecarAnnotationKind::StandaloneNote
            && !skipped_marker_note
        {
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

fn sidecar_page_heading(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('#') {
        return None;
    }
    let heading = trimmed.trim_start_matches('#').trim();
    let lower = heading.to_ascii_lowercase();
    if lower.starts_with("page ")
        || lower.starts_with("page:")
        || lower.starts_with("p. ")
        || lower.starts_with("p ")
    {
        Some(heading.to_string())
    } else {
        None
    }
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
            return value.trim_start().to_string();
        }
    }
    text.to_string()
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

fn decide_sync_source(
    last_hash: Option<&str>,
    marker_hash: &str,
    frontmatter_hash: &str,
    note_exists: bool,
    prefer: Option<Prefer>,
) -> Result<SyncDecision> {
    let Some(last_hash) = last_hash else {
        return Ok(match prefer {
            Some(Prefer::Frontmatter) if note_exists => SyncDecision {
                source: SyncSource::Frontmatter,
                reason: "initial sync; --prefer frontmatter supplied",
            },
            _ => SyncDecision {
                source: SyncSource::Marker,
                reason: "initial sync",
            },
        });
    };

    let marker_changed = marker_hash != last_hash;
    let frontmatter_changed = frontmatter_hash != last_hash;

    match (marker_changed, frontmatter_changed) {
        (false, false) => Ok(SyncDecision {
            source: SyncSource::Marker,
            reason: "marker and frontmatter match the stored hash",
        }),
        (true, false) => Ok(SyncDecision {
            source: SyncSource::Marker,
            reason: "marker changed since last sync",
        }),
        (false, true) => Ok(SyncDecision {
            source: SyncSource::Frontmatter,
            reason: "frontmatter changed since last sync",
        }),
        (true, true) if marker_hash == frontmatter_hash => Ok(SyncDecision {
            source: SyncSource::Marker,
            reason: "marker and frontmatter changed to the same projection",
        }),
        (true, true) => match prefer {
            Some(Prefer::Marker) => Ok(SyncDecision {
                source: SyncSource::Marker,
                reason: "conflict overridden with --prefer marker",
            }),
            Some(Prefer::Frontmatter) => Ok(SyncDecision {
                source: SyncSource::Frontmatter,
                reason: "conflict overridden with --prefer frontmatter",
            }),
            None => Err(CommandError::new(format!(
                "marker/frontmatter conflict: marker hash {marker_hash}, frontmatter hash {frontmatter_hash}, stored hash {last_hash}; rerun with --prefer marker or --prefer frontmatter after reviewing both sides"
            ))),
        },
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
            .after_help("The marker note is the first standalone PDF note annotation."),
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
        .arg(lib_dir_arg())
        .arg(ref_dir_arg())
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
        .after_help("The first standalone /Text annotation in the PDF is treated as the marker note.")
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn lib_dir_arg() -> Arg {
    Arg::new("lib-dir")
        .long("lib-dir")
        .value_name("PATH")
        .value_parser(OsStringValueParser::new())
        .help(
            "Highlights PDF library; defaults to BOB_HIGHLIGHTS_LIB_DIR or lib",
        )
}

fn ref_dir_arg() -> Arg {
    Arg::new("ref-dir")
        .long("ref-dir")
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
        .action(ArgAction::SetTrue)
        .help("Preview work without modifying the vault or PDF")
}

fn prefer_arg() -> Arg {
    Arg::new("prefer")
        .long("prefer")
        .value_name("SIDE")
        .value_parser(["marker", "frontmatter"])
        .help("Resolve a marker/frontmatter conflict using this side")
}

fn write_pdf_arg() -> Arg {
    Arg::new("write-pdf")
        .long("write-pdf")
        .action(ArgAction::SetTrue)
        .help("Allow marker writes back to the PDF")
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
        }
    }
}

impl MarkerValue {
    fn as_marker_value(&self) -> String {
        match self {
            MarkerValue::Null => "null".to_string(),
            MarkerValue::Bool(value) => value.to_string(),
            MarkerValue::Number(value) => value.clone(),
            MarkerValue::String(value) => render_plain_or_quoted(value),
            MarkerValue::List(values) => {
                let values = values
                    .iter()
                    .map(MarkerValue::as_marker_value)
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
        }
    }

    fn exists(&self) -> bool {
        !self.frontmatter.is_empty() || !self.body.is_empty()
    }

    fn contents(&self) -> Option<String> {
        self.exists().then(|| self.render_original())
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

    fn synced_projection(&self) -> Projection {
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

        projection
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
        removed_keys.insert(FIELD_NOTE_TYPE.to_string());
        removed_keys.extend(
            COMMON_USER_FIELDS
                .iter()
                .chain(MARKER_REQUIRED_KEYS.iter())
                .map(|field| (*field).to_string()),
        );
        removed_keys.extend(old_marker_fields);
        removed_keys.extend(projection.keys().cloned());

        let mut lines = Vec::new();
        let mut rendered_note_type = false;
        for key in ordered_projection_keys(projection) {
            let Some(value) = projection.get(&key) else {
                continue;
            };
            lines.push(format!("{key}: {}", value.as_frontmatter_value()));
            if key == FIELD_PARENT {
                lines.push(note_type_frontmatter_line());
                rendered_note_type = true;
            }
        }
        if !rendered_note_type {
            lines.push(note_type_frontmatter_line());
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
            return Ok(self.body.clone());
        };
        let replacement = rendered_highlights.content.as_str();
        replace_managed_region(&self.body, replacement)
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

    Ok(PipelineMetadata {
        source_pdf: source_pdf_value(config, pdf),
        source_pdf_sha256: sha256_file(pdf)?,
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
    body.push_str("PDF: [[");
    body.push_str(source_pdf);
    body.push_str("]]\n\n");
    body.push_str("## Summary\n\n");
    body.push_str("## My Notes\n\n");
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
    let document = Document::load(path).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", path.display()))
    })?;
    let mut note_number = 0;

    for (page_number, page_id) in document.get_pages() {
        for annotation_id in annotation_ids_for_page(&document, page_id)? {
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
                .map(decode_text_string)
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
                page_number,
                note_number,
            });
        }
    }

    Err(CommandError::new(format!(
        "no standalone /Text note annotations found in {}",
        path.display()
    )))
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
        projection.insert(key, parse_value(value));
    }

    validate_required_marker_keys(&projection, "marker")?;
    Ok(projection)
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
    Ok(())
}

fn render_marker(projection: &Projection) -> String {
    let mut rendered = String::new();
    for key in ordered_projection_keys(projection) {
        let Some(value) = projection.get(&key) else {
            continue;
        };
        rendered.push_str("- ");
        rendered.push_str(&key);
        rendered.push_str(": ");
        rendered.push_str(&value.as_marker_value());
        rendered.push('\n');
    }
    rendered
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
        };
    };
    ParsedNote {
        frontmatter: frontmatter
            .into_iter()
            .map(|raw| parse_frontmatter_entry(&raw))
            .collect(),
        body,
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
    key == FIELD_NOTE_TYPE
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

fn projection_hash(projection: &Projection) -> Result<String> {
    let canonical = serde_json::to_vec(projection).map_err(|error| {
        CommandError::new(format!("serialize marker projection: {error}"))
    })?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

fn ref_note_path(config: &Config, pdf: &Path) -> Result<PathBuf> {
    let stem = pdf.file_stem().and_then(OsStr::to_str).ok_or_else(|| {
        CommandError::new(format!(
            "PDF path has no UTF-8 basename: {}",
            pdf.display()
        ))
    })?;
    Ok(config.ref_dir.join(format!("{stem}.md")))
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

fn render_plain_or_quoted(value: &str) -> String {
    if can_render_plain_marker_string(value) {
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

fn can_render_plain_marker_string(value: &str) -> bool {
    if value.starts_with("[[") && value.ends_with("]]") {
        return true;
    }
    !value.is_empty()
        && !value.chars().any(char::is_whitespace)
        && !matches!(
            value,
            "null" | "true" | "false" | "~" | "Null" | "True" | "False"
        )
        && !value.starts_with(['[', '{', '"', '\'', '-', '*', '#', '!', '&'])
        && !value.contains(',')
}

fn can_render_plain_frontmatter_string(value: &str) -> bool {
    can_render_plain_marker_string(value)
        && !value.contains(':')
        && !value.contains('[')
        && !value.contains(']')
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
        parse_marker, parse_note, projection_hash, render_marker,
        resolve_under_bob, MarkerValue, PipelineMetadata, Projection,
    };

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
        assert!(!super::PIPELINE_FIELDS.contains(&"status"));
        assert!(!super::PIPELINE_FIELDS.contains(&"parent"));
        assert!(!super::PIPELINE_FIELDS.contains(&"type"));
        assert!(super::is_command_managed_field("type"));
    }

    #[test]
    fn marker_parser_accepts_yaml_subset_and_normalizes_keys() {
        let projection = parse_marker(
            "\
- Status: wip
* aliases: [\"Systems Performance\", linux]
- source-url: https://example.com/book
- parent: [[obsidian]]
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
            "- status: wip\n- parent: [[obsidian]]\n- type: [[book]]\n",
        )
        .expect_err("marker type should fail");
        assert!(marker_type.to_string().contains("command-managed"));

        let duplicate = parse_marker(
            "- status: wip\n- parent: [[obsidian]]\n- Status: done\n",
        )
        .expect_err("duplicate status should fail");
        assert!(duplicate.to_string().contains("duplicate marker key"));
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
            render_marker(&projection),
            "\
- status: wip
- parent: [[obsidian]]
- title: \"Systems Performance\"
- z_custom: last
"
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

        let projection = note.synced_projection();
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
status: old
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
