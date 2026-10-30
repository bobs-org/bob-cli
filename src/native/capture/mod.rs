use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    fs,
    io::{self, IsTerminal, Read, Write},
    iter,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{Datelike, Days, NaiveDate};
use clap::{
    builder::OsStringValueParser, error::ErrorKind, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture_clip, capture_language,
    capture_language::{
        is_block_id, unused_project_note_pomodoro_error, AuthoredSubBullet,
        CaptureKind, CaptureParseOptions, ClipRequest, ParsedCaptureItem,
        ParsedCaptureText, ParsedDependencyTarget, ParsedDependencyTargetKind,
        ParsedRelation, PomodoroAdjustSpec, PomodoroCloseSpec,
        PomodoroShiftSpec, PomodoroStartSpec, RelationDirection,
        SubBulletTarget, TaskSectionSelector, TaskToggleIntent,
        POMODORO_CLOSE_INTERNAL_BULLETS_ERROR, POMODORO_START_FORCED_ERROR,
    },
    capture_pomodoro_close, capture_pomodoro_start, capture_pomodoros,
    capture_project_note, capture_schedule_log, capture_task_sections,
    capture_task_toggle, capture_templates, collect_done, config,
    env as bob_env, markdown, note_tasks,
    note_tasks::{BlockIdLookup, RefLookup},
    plan_budget, pomodoro,
    projects::{
        frontmatter_is_area, frontmatter_is_project, frontmatter_value,
        parse_frontmatter, ProjectStatus,
    },
    ref_jobs,
    style::Styler,
    task_status_groups,
    url_routing::{self, RefIntent, UrlIntent, UrlRoutingPolicy},
    vault_links,
};

pub(crate) use super::capture_language::is_route_token;

const COMMAND_NAME: &str = "bob capture";
pub(crate) const INBOX_FILE: &str = "mac_inbox.md";

mod batch;
mod block_diff;
mod blocked_tasks;
mod blocks_mirror;
mod budget;
pub(crate) mod cli;
mod commit;
mod dependencies;
mod dependency_drop;
mod ensure_next;
mod existing_task_link;
mod inbox_route;
mod link_guard;
mod open_notes;
mod output;
mod plan;
mod pomodoro_adjust;
mod pomodoro_blocks;
mod pomodoro_close;
mod pomodoro_insert;
mod pomodoro_link;
mod pomodoro_start;
mod project_deps;
mod project_note;
mod pull_in;
mod relations;
mod relations_cancel;
mod relations_close;
mod relations_complete;
mod relations_mentions;
mod relations_mirror;
mod sections;
mod start_output;
mod sub_bullet;
mod task_blocks;
mod task_complete;
mod task_theme;
mod task_toggle;
#[cfg(test)]
mod tests;

use batch::*;
use block_diff::*;
use blocked_tasks::*;
use blocks_mirror::*;
use budget::*;
use cli::*;
use commit::*;
use dependencies::*;
use dependency_drop::*;
use ensure_next::*;
use inbox_route::*;
use link_guard::*;
use open_notes::*;
use output::*;
use plan::*;
use pomodoro_adjust::*;
use pomodoro_blocks::*;
use pomodoro_close::*;
use pomodoro_insert::*;
use pomodoro_link::*;
use pomodoro_start::*;
use project_deps::*;
use project_note::*;
use pull_in::*;
use relations::*;
use relations_cancel::*;
use relations_close::*;
use relations_complete::*;
use relations_mentions::*;
use relations_mirror::*;
use sections::*;
use start_output::*;
use sub_bullet::*;
use task_blocks::*;
use task_complete::*;
use task_theme::*;
use task_toggle::*;

pub(crate) use batch::StagedTextFile;
pub(crate) use block_diff::block_depths;
pub(crate) use commit::{validate_target_parent, write_staged_files};
pub(crate) use existing_task_link::{
    link_existing_task, list_today_link_pomodoros, today_error_code,
    ExistingTaskLinkOutcome, TodayPomodoroRow,
};
pub(crate) use output::{CaptureError, CaptureFailureKind, Placement};
pub(crate) use plan::{format_task_line, inbox_route, route_label};
pub(crate) use pomodoro_adjust::{
    adjustment_duration_for_range, duration_from_range_text,
    format_adjusted_range, normalize_minutes, parse_adjustment_range,
    AdjustRange,
};
pub(crate) use sections::{
    insert_task_line, line_spans, non_tasks_section_headings, LineSpan,
    SectionHeading,
};
pub(crate) use sub_bullet::{
    dominant_indent_unit, first_child_indentation,
    first_direct_managed_log_start, leading_spaces_or_tabs_len,
    leading_whitespace, list_item_body, list_marker_len,
    nearest_shallower_list_item_parent, parse_managed_task_log_marker,
    ManagedTaskLogKind,
};

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    if let Some(hit) = super::capture_flag_guard::find_unknown_long_flag(
        &args,
        &super::capture_flag_guard::capture_spec(),
    ) {
        return reject_unknown_long_flag(hit, &mut command);
    }
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    if matches.get_flag("expand") {
        return expand_capture(&matches, output_format);
    }
    let request = match CaptureRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_capture_error(error, output_format),
    };

    match capture(request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_capture_error(error, output_format),
    }
}

/// Reject an unknown alphabetic long option found where TEXT would begin:
/// exit 2 with clap's unexpected-argument wording, or the JSON error
/// envelope when `--format json` preceded the flag. Nothing is written.
fn reject_unknown_long_flag(
    hit: super::capture_flag_guard::GuardHit,
    command: &mut ClapCommand,
) -> i32 {
    let message =
        super::capture_flag_guard::unexpected_argument_message(&hit.flag);
    if hit.format_json {
        return print_capture_error(
            CaptureError::usage(message),
            OutputFormat::Json,
        );
    }
    print_clap_error(command.error(ErrorKind::UnknownArgument, message))
}

#[derive(Serialize)]
struct CaptureExpansionOutput {
    ok: bool,
    expanded: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    template: Option<capture_language::template::TemplateSummary>,
}

fn expand_capture(matches: &ArgMatches, output_format: OutputFormat) -> i32 {
    if let Err(error) = validated_template_origin(matches) {
        return print_capture_error(error, output_format);
    }
    let raw = match raw_text_from_matches(matches) {
        Ok(raw) => raw,
        Err(error) => return print_capture_error(error, output_format),
    };
    let expansion = match capture_language::template::expand(&raw) {
        Ok(expansion) => expansion,
        Err(issue) => {
            return print_capture_error(
                CaptureError::usage_code(issue.message(), issue.code()),
                output_format,
            );
        }
    };
    let (expanded, template) = match expansion {
        Some(expansion) => (expansion.text, Some(expansion.summary)),
        None => (raw, None),
    };
    match output_format {
        OutputFormat::Human => match io::stdout()
            .lock()
            .write_all(expanded.as_bytes())
        {
            Ok(()) => 0,
            Err(error) => {
                // A reader that closed the pipe got what it asked for:
                // end quietly instead of reporting the failed write.
                if error.kind() == io::ErrorKind::BrokenPipe {
                    return 0;
                }
                eprintln!("{COMMAND_NAME}: failed to write expansion: {error}");
                1
            }
        },
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string(&CaptureExpansionOutput {
                    ok: true,
                    expanded,
                    template,
                })
                .expect("serialize capture expansion")
            );
            0
        }
    }
}

/// Load the URL routing policy for `capture`. `-R` disables routing;
/// a config error warns and disables it too (a bare URL then simply
/// stays a task). A `capture: false` toggle disables it silently.
fn load_capture_routing(no_ref: bool) -> Option<UrlRoutingPolicy> {
    if no_ref {
        return None;
    }
    match UrlRoutingPolicy::load() {
        Ok(policy) => policy.capture.then_some(policy),
        Err(error) => {
            eprintln!("{COMMAND_NAME}: warning: URL routing is off: {error}");
            None
        }
    }
}

fn capture(mut request: CaptureRequest) -> Result<CaptureResult, CaptureError> {
    let routing = load_capture_routing(request.no_ref);
    let (mut batch, pull_in) =
        plan_with_pull_in(&mut request, routing.as_ref())?;
    if request.require_inbox_route_choice
        && batch
            .inbox_route
            .candidates
            .iter()
            .any(|candidate| candidate.answer.is_none())
    {
        return Err(CaptureError::inbox_route_choice_required(
            &batch.inbox_route,
        ));
    }
    append_plan_budget(&request, &mut batch)?;
    let theme_refs = append_theme_refs(&request, &mut batch);
    append_unconfigured_theme_warnings(&request, &mut batch);
    let open_notes = build_open_notes(&request.bob_dir, &batch);
    if !request.dry_run {
        commit_capture_batch(&mut batch, &request.bob_dir)?;
        if !batch.staged_jobs.is_empty()
            && let Err(error) = ref_jobs::kick()
        {
            eprintln!(
                "{COMMAND_NAME}: warning: could not start the clip worker ({error}); run bob ref jobs run"
            );
        }
    }
    let mut hide_failures = Vec::new();
    if !request.dry_run {
        hide_failures =
            append_committed_relation_label_hides(&batch, &request.bob_dir);
    }
    let mut result = CaptureResult::from_items(
        batch.items.into_iter().map(|item| item.result).collect(),
        batch.global_destination,
        batch.template,
        batch.template_rows,
        batch.warnings,
        batch.plan_budget,
        batch.pomodoro_blocks,
        batch.task_blocks,
        open_notes,
        pull_in,
        batch.inbox_route,
    );
    result.warnings.extend(hide_failures);
    result.theme_refs = theme_refs;
    let has_template = result.template.is_some();
    save_template_shelf(&request, has_template, &mut result);
    Ok(result)
}

/// Best-effort shelf save after a successful real template capture. Never
/// fails the capture: failures set `template.not_saved` and print one
/// stderr warning. Dry runs, forced destinations, and saving-off leave the
/// store untouched and the JSON/human output byte-identical.
fn save_template_shelf(
    request: &CaptureRequest,
    has_template: bool,
    out: &mut CaptureResult,
) {
    if !has_template || request.dry_run {
        return;
    }
    if request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_task_section.is_some()
        || !request.forced_destination_flags.is_empty()
    {
        return;
    }
    let cap = match config::load_capture_config(&config::config_path()) {
        Ok(config) => config.max_saved_templates(),
        Err(error) => {
            let message = match error {
                config::ConfigError::Read(message)
                | config::ConfigError::Invalid(message) => message,
            };
            eprintln!("{COMMAND_NAME}: warning: {message}; using 36");
            config::DEFAULT_MAX_SAVED_TEMPLATES
        }
    };
    if cap == 0 {
        return;
    }
    let Some(canonical) =
        capture_templates::store::canonical_template_text(&request.raw_text)
    else {
        return;
    };
    let now = capture_templates::store::now_timestamp();
    let origin = request.template_origin.as_deref();
    match capture_templates::store::save_template_text(
        &canonical, origin, cap, &now,
    ) {
        Ok(saved) => {
            for warning in &saved.warnings {
                eprintln!("{COMMAND_NAME}: warning: {warning}");
            }
            out.template_origin = request.template_origin.clone();
            out.template_saved = Some(TemplateSavedJson {
                id: saved.id,
                status: saved.status.as_str(),
            });
        }
        Err(error) => {
            let reason = error.reason();
            eprintln!("{COMMAND_NAME}: warning: template not saved: {reason}");
            out.template_not_saved = Some(reason);
        }
    }
}
