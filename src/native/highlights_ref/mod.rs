use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error as StdError,
    ffi::{OsStr, OsString},
    fmt, fs, iter,
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

use super::{
    config as bob_config, env as bob_env, markdown, ob,
    style::{display_width, pad_right, Styler},
};

pub(crate) mod create;
pub(crate) mod ingest;
pub(crate) mod intake;

mod annotation_tasks;
mod arxiv;
mod attach;
mod audio;
mod birth;
pub(crate) mod cli;
mod clip;
mod clip_adapter;
mod clip_url;
mod collection;
mod collection_name;
mod companion;
mod continuation;
pub(crate) mod diagrams;
mod doctor;
pub(crate) mod duplicate;
mod fetch;
pub(super) mod force;
mod frontmatter;
mod guard;
mod hooks;
mod image_notes;
mod intake_status;
mod io;
mod listen;
mod listen_feed;
mod marker;
mod marker_write;
mod model;
mod note;
// Model and serializer are intentionally defined before their production callers;
// the following snapshot phases wire these crate-visible seams into projection.
#[allow(dead_code)]
pub(crate) mod note_snapshot;
mod pdf_meta;
mod pdf_target;
pub(crate) mod pdf_write_policy;
mod projection;
mod reading_execute;
mod reading_plan;
// The family-planning seam: read-only recapture classification and
// allocation, consumed by the single-source and collection reread writers.
pub(crate) mod recapture;
mod region;
// Lock, precondition, scoped-git, and rollback helpers shared by rename
// and the overwrite transaction.
pub(crate) mod reference_mutation;
// Staged in-place overwrite of unannotated references; recovery hooks run
// before create, scan, rename, and standalone vault-sync can commit.
pub(crate) mod overwrite;
// `ref rename` consumes the shared mutation helpers and recovers a pending
// overwrite intent before its pre-sync.
#[allow(dead_code)]
pub(crate) mod rename;
mod render_tex;
mod report;
// This phase defines the planner seam consumed by the later render phase.
pub(crate) mod return_links;
mod scan_json;
mod scan_receipt;
pub(crate) mod sections;
mod sidecar;
mod sidecar_render;
mod sources;
mod stamp;
mod sync;
mod target;
#[cfg(test)]
mod tests;
mod text;
mod underline;
mod workdir;

use annotation_tasks::*;
use audio::*;
use birth::*;
use cli::*;
use clip::*;
use companion::*;
use continuation::*;
use doctor::*;
use frontmatter::*;
use guard::*;
use hooks::*;
use image_notes::*;
use io::*;
use listen::*;
use marker::*;
use marker_write::*;
use model::*;
use note::*;
use projection::*;
use reading_execute::*;
use reading_plan::*;
use report::*;
use scan_json::*;
use scan_receipt::*;
use sidecar::*;
use sidecar_render::*;
use stamp::*;
use sync::*;
use text::*;
use underline::*;
use workdir::*;

pub(crate) const COMMAND_NAME: &str = "bob ref";
const DEFAULT_LIB_DIR: &str = "lib";
const DEFAULT_REF_DIR: &str = "ref";
const DEFAULT_XLIB_DIR: &str = "xlib";

const ENV_LEGACY_PRE_SCAN_COMMAND: &str = "BOB_HIGHLIGHTS_PRE_SCAN_COMMAND";
const ENV_LIB_DIR: &str = "BOB_HIGHLIGHTS_LIB_DIR";
const ENV_PRE_SCAN_HOOK: &str = "BOB_HIGHLIGHTS_PRE_SCAN_HOOK";
pub(crate) const ENV_REF_DIR: &str = "BOB_HIGHLIGHTS_REF_DIR";
pub(crate) const ENV_XLIB_DIR: &str = "BOB_HIGHLIGHTS_XLIB_DIR";

const FIELD_STATUS: &str = "status";
const FIELD_PARENT: &str = "parent";
const FIELD_ID: &str = "id";
const FIELD_SOURCE_NOTE: &str = "source_note";
const FIELD_RESEARCH: &str = "research";
const FIELD_NOTE_TYPE: &str = "type";
const FIELD_REF_TYPE: &str = "ref_type";
const FIELD_AUDIO: &str = "audio";
const NOTE_TYPE_VALUE: &str = "[[ref]]";
const STATUS_READY: &str = "ready";
const STATUS_NEXT: &str = "next";
const STATUS_WIP: &str = "wip";
const STATUS_READ: &str = "read";
const STATUS_ABANDONED: &str = "abandoned";
const STATUS_LEGACY: &str = "legacy";
const DEPRECATED_STATUS_UNREAD: &str = "unread";
const DEPRECATED_STATUS_DONE: &str = "done";
const ALLOWED_STATUS_VALUES: &[&str] = &[
    STATUS_READY,
    STATUS_NEXT,
    STATUS_WIP,
    STATUS_READ,
    STATUS_ABANDONED,
    STATUS_LEGACY,
];
const MARKER_REQUIRED_KEYS: &[&str] = &[FIELD_STATUS, FIELD_PARENT];
const COMMAND_MANAGED_FIELDS: &[&str] =
    &[FIELD_NOTE_TYPE, FIELD_REF_TYPE, FIELD_AUDIO];
pub(crate) const MANAGED_BODY_BEGIN: &str = "<!-- highlights:begin -->";
pub(crate) const MANAGED_BODY_END: &str = "<!-- highlights:end -->";
// Narrow `index`-phase seams: the sibling `ref_library` module builds its
// read-only rows on these without exposing task-line or projection types.
pub(crate) use arxiv::ArxivPaper;
pub(crate) use cli::{bob_dir_arg, ref_dir_arg, xlib_dir_arg};
pub(crate) use clip_url::{
    dedupe_key_for, humanize_stem, percent_decode, validate_and_clean,
};
pub(crate) use continuation::group_page_label;
pub(crate) use frontmatter::split_frontmatter;
pub(crate) use marker::{
    is_known_ref_task_mark, normalize_deprecated_status_str,
    ref_task_mark_status, ref_task_mark_target_status,
};
pub(crate) use model::{Config, UnresolvedImageNoteState};
pub(crate) use note::configured_path;
pub(crate) use reading_plan::apply_v2_migration_note;
pub(crate) use region::{
    is_marker_mirror_text, parse_follow_up_task, parse_managed_region,
    split_note_body, RegionBlockKind,
};
pub(crate) const SOURCE_LINK_ALIAS: &str = "🔖";
pub(crate) use sidecar::is_wikilink;
pub(crate) use sources::{collect_intake_records, IntakeRecord};
pub(crate) use sources::{collect_recorded_source_urls, RecordedSource};
const TASKS_SECTION_TITLE: &str = "Tasks";
const TASKS_SECTION_HEADING: &str = "## Tasks";
const PDF_TASK_BLOCK_ID: &str = "^ref";
const PDF_TASK_HIDE_TAG: &str = "#hide";
const PDF_TASK_TAG: &str = "#task";
const PDF_TASK_KIND_TAG: &str = "#ref";
const HIGHLIGHT_TASK_FIELD: &str = "h";
const LEGACY_HIGHLIGHT_TASK_FIELD: &str = "highlight_task";
const HIGHLIGHT_TASK_ID_VERSION: &str = "v1";
const SOURCE_TASK_BLOCK_ID_PREFIX: &str = "ht-";
const PIPELINE_VERSION: &str = "highlights-ref-mvp-3";
const REMOVED_HIGHLIGHTS_HEADING: &str = "### Removed highlights";
const TEXTBUNDLE_TEXT_FILES: &[&str] = &["text.md", "text.markdown"];

const FIELD_CREATED: &str = "created";
const FIELD_SOURCE_PDF: &str = "source_pdf";
const FIELD_SOURCE_PDF_SHA256: &str = "source_pdf_sha256";
const FIELD_HIGHLIGHTS_SIDECAR: &str = "highlights_sidecar";
const FIELD_HIGHLIGHTS_COUNT: &str = "highlights_count";
const FIELD_HIGHLIGHTS_SYNCED_AT: &str = "highlights_synced_at";
const FIELD_MARKER_BASE: &str = "highlights_marker_base";
const FIELD_MARKER_REPLACED_HASH: &str = "highlights_marker_replaced_hash";
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
    FIELD_MARKER_REPLACED_HASH,
    FIELD_MARKER_HASH,
    FIELD_MARKER_FIELDS,
    FIELD_PIPELINE_VERSION,
];

const FIELD_CAPTURED: &str = "captured";
const FIELD_RETURN_LINKS: &str = "return_links";
const COMMON_USER_FIELDS: &[&str] = &[
    FIELD_PARENT,
    "title",
    FIELD_ID,
    FIELD_RESEARCH,
    "aliases",
    "topics",
    "source_url",
    FIELD_SOURCE_NOTE,
    "author",
    "published",
    FIELD_CAPTURED,
    FIELD_RETURN_LINKS,
];

/// A bare `bob ref jobs` (or bare flags) lists: rewrite to
/// `jobs list …` so `list` stays the flag-only, read-only default.
/// Help, explicit subcommands, and unknown words pass through. The
/// rewrite also applies when `bob ref`'s own flags (`-n`/`--no-hooks`)
/// come before `jobs`, so `bob ref -n jobs` lists.
fn default_jobs_args(args: Vec<OsString>) -> Vec<OsString> {
    let mut prefix = 0;
    while args.get(prefix).is_some_and(|arg| {
        arg == OsStr::new("-n") || arg == OsStr::new("--no-hooks")
    }) {
        prefix += 1;
    }
    let Some(first) = args.get(prefix) else {
        return args;
    };
    if first != OsStr::new("jobs") {
        return args;
    }
    let rest = &args[prefix + 1..];
    let Some(second) = rest.first() else {
        let mut out = args[..prefix].to_vec();
        out.push(OsString::from("jobs"));
        out.push(OsString::from("list"));
        return out;
    };
    if second == OsStr::new("-h")
        || second == OsStr::new("--help")
        || second == OsStr::new("list")
        || second == OsStr::new("run")
    {
        return args;
    }
    if second.to_string_lossy().starts_with('-') {
        let mut out = args[..prefix].to_vec();
        out.push(OsString::from("jobs"));
        out.push(OsString::from("list"));
        out.extend(rest.iter().cloned());
        return out;
    }
    args
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let args = default_jobs_args(args);
    // The force-mode decoder reads bareness from the raw argv (clap
    // indices alone cannot separate bare `-f` from an explicit
    // `--force=` value). Only the create path needs it. The subcommand
    // token sits after any top-level options (for example `-n` in
    // `bob ref -n create ...`), so locate it instead of assuming it is
    // first.
    let create_raw: Vec<OsString> = args
        .iter()
        .position(|arg| arg == "create" || arg == "clip")
        .map(|pos| args.iter().skip(pos + 1).cloned().collect())
        .unwrap_or_default();
    let matches = match build_cli().try_get_matches_from(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let exit_code = error.exit_code();
            if let Err(print_error) = error.print() {
                // A closed pipe renders nothing: stay quiet instead of
                // reporting the failed render, but keep clap's own exit
                // code so a usage error never reports success.
                if print_error.kind() != std::io::ErrorKind::BrokenPipe {
                    eprintln!(
                        "{COMMAND_NAME}: failed to print command-line error: {print_error}"
                    );
                }
            }
            return exit_code;
        }
    };

    match matches.subcommand() {
        Some(("create", sub_matches)) => create::run(sub_matches, &create_raw),
        Some(("find", sub_matches)) => {
            crate::native::ref_library::cli::run_find(sub_matches)
        }
        Some(("list", sub_matches)) => {
            crate::native::ref_library::cli::run_list(sub_matches)
        }
        Some(("rename", sub_matches)) => rename::run(sub_matches),
        Some(("show", sub_matches)) => {
            crate::native::ref_library::cli::run_show(sub_matches)
        }
        Some(("today", sub_matches)) => {
            crate::native::ref_library::today::run_today(sub_matches)
        }
        Some(("migrate-tasks", sub_matches)) => {
            crate::native::ref_library::migrate_tasks::run_migrate_tasks(
                sub_matches,
            )
        }
        Some(("migrate-zorg", sub_matches)) => {
            crate::native::ref_library::migrate_zorg::run_migrate_zorg(
                sub_matches,
            )
        }
        Some(("jobs", sub_matches)) => {
            crate::native::ref_jobs::run(sub_matches)
        }
        Some(("scan", sub_matches)) => {
            run_scan(sub_matches, no_hooks_flag(&matches, sub_matches))
        }
        Some(("sync", sub_matches)) => run_sync(sub_matches),
        Some(("doctor", sub_matches)) => {
            run_doctor(sub_matches, no_hooks_flag(&matches, sub_matches))
        }
        Some(("marker", sub_matches)) => run_marker(sub_matches),
        _ => 2,
    }
}

fn no_hooks_flag(root_matches: &ArgMatches, sub_matches: &ArgMatches) -> bool {
    root_matches.get_flag("no-hooks") || sub_matches.get_flag("no-hooks")
}

fn run_scan(matches: &ArgMatches, no_hooks: bool) -> i32 {
    let json = matches
        .get_one::<String>("format")
        .is_some_and(|format| format == "json");
    if json && matches.get_flag("verbose") {
        eprintln!(
            "error: the argument '--format <FORMAT>' cannot be used with '--verbose'\n\nUsage: bob ref scan [OPTIONS]\n\nFor more information, try '--help'."
        );
        return 2;
    }
    let config = Config::from_matches(matches);
    if matches.get_flag("intake-only") {
        let dry_run = matches.get_flag("dry-run");
        return if json {
            scan_intake_only_json(&config, dry_run)
        } else {
            intake::scan_intake_only(&config, dry_run)
        };
    }
    let options = SyncOptions {
        dry_run: matches.get_flag("dry-run"),
        write_pdf: matches.get_flag("write-pdfs"),
        prefer: None,
        ignore_unannotated_sidecar: false,
    };
    let queued = (!options.dry_run).then(|| queued_xlib_count(&config));
    if json {
        let code = scan_library_json(
            &config,
            options,
            jobs_from_matches(matches),
            no_hooks,
            matches.get_flag("attended"),
        );
        let failure = (code != 0).then(|| {
            format!("bob ref scan --format json exited with code {code}")
        });
        record_scan_status(
            queued,
            failure.map(|message| ("scan_failed", message)),
        );
        return code;
    }
    let result = scan_library(
        &config,
        options,
        jobs_from_matches(matches),
        matches.get_flag("verbose"),
        no_hooks,
        matches.get_flag("attended"),
    );
    let failure = result.as_ref().err().map(|error| {
        (error.code.unwrap_or("scan_failed"), error.message.clone())
    });
    record_scan_status(queued, failure);
    report_result(result)
}

/// `xlib/` PDFs waiting before a writing full scan, which files them.
fn queued_xlib_count(config: &Config) -> usize {
    let mut pending = Vec::new();
    if config.xlib_dir.is_dir()
        && collect_pdf_paths_from_dir(&config.xlib_dir, &mut pending).is_err()
    {
        return 0;
    }
    pending.len()
}

/// Record a writing full scan in the per-host intake status; `queued` is
/// `None` for a dry run, which records nothing.
fn record_scan_status(
    queued: Option<usize>,
    failure: Option<(&'static str, String)>,
) {
    const COMMAND: &str = "bob ref scan";
    let Some(queued) = queued else { return };
    match failure {
        None => {
            if let Some(warning) =
                intake_status::record_success(COMMAND, queued)
            {
                eprintln!("{warning}");
            }
        }
        Some((reason, message)) => {
            if let Some(warning) =
                intake_status::record_failure(COMMAND, reason, &message)
            {
                eprintln!("{warning}");
            }
        }
    }
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
    let pdf = match resolve_pdf_argument(&config, matches) {
        Ok(pdf) => pdf,
        Err(error) => return report_result(Err(error)),
    };
    let options = SyncOptions {
        dry_run: matches.get_flag("dry-run"),
        write_pdf: matches.get_flag("write-pdf"),
        prefer: prefer_from_matches(matches),
        ignore_unannotated_sidecar: false,
    };

    report_result(sync_pdf(&config, &pdf, options))
}

fn run_doctor(matches: &ArgMatches, no_hooks: bool) -> i32 {
    let config = Config::from_matches(matches);
    report_result(doctor_vault(&config, no_hooks))
}

fn run_marker(matches: &ArgMatches) -> i32 {
    let config = Config::from_matches(matches);
    let pdf = match resolve_pdf_argument(&config, matches) {
        Ok(pdf) => pdf,
        Err(error) => return report_result(Err(error)),
    };
    report_result(show_marker(&config, &pdf))
}

/// Resolve a `sync`/`marker` PDF argument against the cwd and normalize it
/// lexically, keeping the configured `lib/` or `xlib/` spelling when only a
/// symlinked directory separates them. The cwd is physical (macOS reports
/// `/private/var` for `/var`), so a relative path could otherwise miss the
/// library and plan a flat duplicate note.
fn resolve_pdf_argument(
    config: &Config,
    matches: &ArgMatches,
) -> Result<PathBuf> {
    let pdf = stamp::resolve_exact_output_path(&required_path(matches, "pdf"))?;
    Ok(rebase_onto_library(
        pdf,
        &[&config.lib_dir, &config.xlib_dir],
    ))
}

fn rebase_onto_library(pdf: PathBuf, roots: &[&Path]) -> PathBuf {
    if roots.iter().any(|root| pdf.starts_with(root)) {
        return pdf;
    }
    // Canonicalize directories only, so a symlinked PDF file keeps its own
    // path and identity.
    let (Some(parent), Some(name)) = (pdf.parent(), pdf.file_name()) else {
        return pdf;
    };
    let Ok(real_parent) = parent.canonicalize() else {
        return pdf;
    };
    for root in roots {
        if let Ok(real_root) = root.canonicalize()
            && let Ok(relative) = real_parent.strip_prefix(&real_root)
        {
            return root.join(relative).join(name);
        }
    }
    pdf
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
