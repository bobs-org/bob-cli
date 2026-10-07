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
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
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
        PomodoroAdjustSpec, PomodoroCloseSpec, PomodoroShiftSpec,
        PomodoroStartSpec, SubBulletTarget, TaskSectionSelector,
        TaskToggleIntent, POMODORO_CLOSE_INTERNAL_BULLETS_ERROR,
        POMODORO_START_FORCED_ERROR,
    },
    capture_pomodoro_close, capture_pomodoro_start, capture_pomodoros,
    capture_project_note, capture_schedule_log, capture_task_sections,
    capture_task_toggle, collect_done, config, env as bob_env, markdown,
    note_tasks,
    note_tasks::{BlockIdLookup, RefLookup},
    plan_budget, pomodoro,
    projects::{
        frontmatter_is_area, frontmatter_is_project, frontmatter_value,
        parse_frontmatter, ProjectStatus,
    },
    ref_jobs,
    style::Styler,
    task_status_groups,
    url_routing::{self, UrlIntent, UrlRoutingPolicy},
    vault_links,
};

pub(crate) use super::capture_language::is_route_token;

const COMMAND_NAME: &str = "bob capture";
pub(crate) const INBOX_FILE: &str = "mac_inbox.md";

mod batch;
mod block_diff;
mod budget;
pub(crate) mod cli;
mod commit;
mod dependencies;
mod ensure_next;
mod output;
mod plan;
mod pomodoro_adjust;
mod pomodoro_blocks;
mod pomodoro_close;
mod pomodoro_insert;
mod pomodoro_link;
mod pomodoro_start;
mod project_note;
mod sections;
mod start_output;
mod sub_bullet;
mod task_blocks;
mod task_complete;
mod task_toggle;
#[cfg(test)]
mod tests;

use batch::*;
use block_diff::*;
use budget::*;
use cli::*;
use commit::*;
use dependencies::*;
use ensure_next::*;
use output::*;
use plan::*;
use pomodoro_adjust::*;
use pomodoro_blocks::*;
use pomodoro_close::*;
use pomodoro_insert::*;
use pomodoro_link::*;
use pomodoro_start::*;
use project_note::*;
use sections::*;
use start_output::*;
use sub_bullet::*;
use task_blocks::*;
use task_complete::*;
use task_toggle::*;

pub(crate) use batch::StagedTextFile;
pub(crate) use commit::{validate_target_parent, write_staged_files};
pub(crate) use output::{CaptureError, Placement};
pub(crate) use plan::{format_task_line, inbox_route, route_label};
pub(crate) use pomodoro_adjust::{
    adjustment_duration_for_range, format_adjusted_range, normalize_minutes,
    parse_adjustment_range, AdjustRange,
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
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
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
            eprintln!("{COMMAND_NAME}: warning: URL routing is off: {error:?}");
            None
        }
    }
}

fn capture(request: CaptureRequest) -> Result<CaptureResult, CaptureError> {
    let routing = load_capture_routing(request.no_ref);
    let mut batch = plan_capture_batch(&request, routing.as_ref())?;
    append_plan_budget(&request, &mut batch)?;
    if !request.dry_run {
        commit_capture_batch(&mut batch)?;
        if !batch.staged_jobs.is_empty()
            && let Err(error) = ref_jobs::kick()
        {
            eprintln!(
                "{COMMAND_NAME}: warning: could not start the clip worker ({error}); run bob ref jobs run"
            );
        }
    }
    Ok(CaptureResult::from_items(
        batch.items.into_iter().map(|item| item.result).collect(),
        batch.global_destination,
        batch.warnings,
        batch.plan_budget,
        batch.pomodoro_blocks,
        batch.task_blocks,
    ))
}
