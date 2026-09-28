//! Pure `bob randomize` planner: note snapshots in, per-note postimages out.
//!
//! The planner turns note snapshots into per-note postimages, a reroll list,
//! skip reasons, and load data. It performs no I/O, takes no lock, and
//! writes no disk state; the command phase (a later bead) orchestrates sync,
//! apply, and commit around it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::capture::line_spans;
use super::capture_pomodoros::{self, PomodoroState};
use super::capture_schedule_log::{self, LogInsertion};
use super::capture_task_toggle::set_task_line_status;
use super::collect_done;
use super::config::{self, PriorityProperty};
use super::note_tasks::{self, NoteTask};
use super::pomodoro;
use super::task_fields;
use super::task_status_groups;
use super::task_status_hooks::{self, TasksSettings};

/// One vault note as read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteSnapshot {
    pub(crate) path: PathBuf,
    pub(crate) relative_path: PathBuf,
    pub(crate) contents: String,
}

/// Planner inputs. `today` is the effective day (the `BOB_DAY_FILE` date
/// when it parses, else the current date); `until` is both the due cutoff
/// and the base the windows roll from.
#[derive(Debug, Clone)]
pub(crate) struct PlanContext<'a> {
    pub(crate) today: NaiveDate,
    pub(crate) until: NaiveDate,
    pub(crate) seed: u64,
    pub(crate) priority: &'a PriorityProperty,
    pub(crate) selected_labels: Option<&'a [String]>,
    pub(crate) tasks_settings: &'a TasksSettings,
    pub(crate) daily_path: &'a Path,
    pub(crate) daily_contents: Option<&'a str>,
}

/// Why a task was skipped. The left-alone group (`Next`, `InProgress`,
/// `Pomodoro`, `NotSelected`) is intentional and rendered as counts; the
/// needs-a-look group is listed with `path:line` so it can be fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipReason {
    DuplicateField,
    InvalidScheduled,
    UnknownPriority,
    HardDate,
    Next,
    InProgress,
    OtherStatus,
    Pomodoro,
    NotSelected,
}

impl SkipReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DuplicateField => "duplicate_field",
            Self::InvalidScheduled => "invalid_scheduled",
            Self::UnknownPriority => "unknown_priority",
            Self::HardDate => "hard_date",
            Self::Next => "next",
            Self::InProgress => "in_progress",
            Self::OtherStatus => "other_status",
            Self::Pomodoro => "pomodoro",
            Self::NotSelected => "not_selected",
        }
    }
}

/// One skipped task with its original (pre-edit) position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skip {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) reason: SkipReason,
    pub(crate) detail: Option<String>,
}

/// One re-rolled task with every JSON task field. `line` and `task_ref`
/// use the task's original (pre-edit) position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reroll {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) task_ref: String,
    pub(crate) block_id: Option<String>,
    pub(crate) description: String,
    pub(crate) level: String,
    pub(crate) value: String,
    pub(crate) min_days: u64,
    pub(crate) max_days: u64,
    pub(crate) offset_days: u64,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) status_from: char,
    pub(crate) status_to: char,
    pub(crate) schedule_log: LogInsertion,
}

/// One changed note: its full postimage plus the rerolls it contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotePlan {
    pub(crate) path: PathBuf,
    pub(crate) relative_path: PathBuf,
    pub(crate) original: String,
    pub(crate) updated: String,
    pub(crate) rerolls: Vec<Reroll>,
    pub(crate) regrouped: bool,
}

/// Open-task load for one day in `today+1 ..= today+35`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadDay {
    pub(crate) date: NaiveDate,
    pub(crate) count: usize,
}

/// A status-grouping warning surfaced from a rewritten note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanGroupingWarning {
    pub(crate) path: String,
    pub(crate) original_heading_line: usize,
    pub(crate) heading_ancestry: Vec<String>,
    pub(crate) code: String,
    pub(crate) message: String,
}

/// The full pure plan for a set of note snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Plan {
    pub(crate) notes: Vec<NotePlan>,
    pub(crate) rerolls: Vec<Reroll>,
    pub(crate) skipped: Vec<Skip>,
    pub(crate) still_due_p0: usize,
    pub(crate) unchanged: usize,
    pub(crate) not_selected: usize,
    pub(crate) load: Vec<LoadDay>,
    pub(crate) grouping_warnings: Vec<PlanGroupingWarning>,
    pub(crate) needs_blocked_status: bool,
}

/// Add `days` to `date` without wrapping or panicking. Returns `None`
/// when `days` does not fit in the signed range or the sum leaves the
/// representable calendar range.
fn checked_add_days(date: NaiveDate, days: u64) -> Option<NaiveDate> {
    let days: i64 = days.try_into().ok()?;
    // `try_days` instead of `days`: the latter panics on out-of-bounds
    // spans (for example `i64::MAX`), while the former reports `None`.
    let delta = chrono::Duration::try_days(days)?;
    date.checked_add_signed(delta)
}

/// Plan every snapshot: classify each open task, roll the candidates, and
/// compose one postimage per changed note. Fails deterministically before
/// any write when a cutoff plus a configured priority roll (or the 35-day
/// load horizon) leaves the representable date range.
pub(crate) fn plan_notes(
    snapshots: &[NoteSnapshot],
    ctx: &PlanContext<'_>,
) -> Result<Plan, String> {
    let pomodoro_links = open_pomodoro_block_links(ctx);
    let mut plan = Plan::default();
    let mut load_counts: BTreeMap<NaiveDate, usize> = BTreeMap::new();
    for snapshot in snapshots {
        plan_note(snapshot, ctx, &pomodoro_links, &mut plan, &mut load_counts)?;
    }
    for offset in 1..=35u64 {
        let Some(date) = checked_add_days(ctx.today, offset) else {
            return Err(format!(
                "priority window rolls beyond the supported date range from {}",
                ctx.today.format("%Y-%m-%d")
            ));
        };
        plan.load.push(LoadDay {
            date,
            count: load_counts.get(&date).copied().unwrap_or(0),
        });
    }
    Ok(plan)
}

struct PomodoroBlockLink {
    target: String,
    block_id: String,
}

/// Block links (`[[target#^id]]`, `![[target#^id]]`, aliased forms) found at
/// any depth of the child blocks of open Pomodoros in today's daily note.
/// Over-matching is acceptable because skipping is the safe side.
fn open_pomodoro_block_links(ctx: &PlanContext<'_>) -> Vec<PomodoroBlockLink> {
    let Some(daily) = ctx.daily_contents else {
        return Vec::new();
    };
    let scan = capture_pomodoros::scan(daily);
    if scan.entries.is_empty() {
        return Vec::new();
    }
    let spans = line_spans(daily);
    let lines = spans.iter().map(|span| span.text).collect::<Vec<_>>();
    let section_end = pomodoro::pomodoros_section_range(&lines)
        .map(|range| range.end)
        .unwrap_or(lines.len());
    let mut links = Vec::new();
    for entry in &scan.entries {
        if entry.state != PomodoroState::Open {
            continue;
        }
        let mut index = entry.line;
        while index < section_end {
            let text = lines.get(index).map_or("", |line| *line);
            if !text.trim().is_empty() && leading_indent_len(text) == 0 {
                break;
            }
            links.extend(block_links_in_line(text));
            index += 1;
        }
    }
    links
}

fn leading_indent_len(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn block_links_in_line(line: &str) -> Vec<PomodoroBlockLink> {
    let mut links = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find("[[") {
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("]]") else {
            break;
        };
        let inside = &after_open[..close];
        let target_part = inside.split('|').next().unwrap_or("");
        if let Some(fragment) = target_part.find("#^") {
            let target = target_part[..fragment].trim().to_string();
            let block_id = target_part[fragment + 2..].trim().to_string();
            if !block_id.is_empty()
                && block_id.bytes().all(collect_done::is_block_id_byte)
            {
                links.push(PomodoroBlockLink { target, block_id });
            }
        }
        rest = &after_open[close + 2..];
    }
    links
}

/// Vault-relative path without a trailing `.md`, joined with `/`.
fn relative_key(relative_path: &Path) -> String {
    let mut text = relative_path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => {
                Some(part.to_string_lossy().into_owned())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if text.len() > 3 && text[text.len() - 3..].eq_ignore_ascii_case(".md") {
        text.truncate(text.len() - 3);
    }
    text
}

fn file_stem(relative_path: &Path) -> String {
    relative_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether a Pomodoro link target names `snapshot`'s note: the
/// vault-relative path without `.md` or the file stem, both
/// case-insensitive; an empty target means the daily note itself.
fn link_targets_note(
    target: &str,
    snapshot: &NoteSnapshot,
    daily_path: &Path,
) -> bool {
    if target.is_empty() {
        return snapshot.path == daily_path;
    }
    let target = target.to_lowercase();
    target == relative_key(&snapshot.relative_path).to_lowercase()
        || target == file_stem(&snapshot.relative_path).to_lowercase()
}

struct WriteJob {
    line_index: usize,
    value_start: usize,
    value_end: usize,
    new_date: String,
    status_from: char,
    status_to: char,
    entry: String,
    reroll_index: usize,
}

#[allow(clippy::too_many_lines)]
fn plan_note(
    snapshot: &NoteSnapshot,
    ctx: &PlanContext<'_>,
    pomodoro_links: &[PomodoroBlockLink],
    plan: &mut Plan,
    load_counts: &mut BTreeMap<NaiveDate, usize>,
) -> Result<(), String> {
    let scan = note_tasks::scan(&snapshot.contents, ctx.tasks_settings);
    let display = display_path(&snapshot.relative_path);
    let mut digest_ordinals: HashMap<String, usize> = HashMap::new();
    let mut jobs: Vec<WriteJob> = Vec::new();
    let mut note_rerolls: Vec<Reroll> = Vec::new();

    for task in scan.open_tasks() {
        let line = line_spans(&snapshot.contents)[task.line_index].text;
        if task.block_id.as_deref() == Some("prj") {
            continue;
        }
        let priorities = task_fields::inline_fields(line, "priority");
        let scheduleds = task_fields::inline_fields(line, "scheduled");
        if priorities.is_empty() || scheduleds.is_empty() {
            if priorities.is_empty()
                && scheduleds.len() == 1
                && let Some(date) = task_fields::parse_strict_calendar_date(
                    &scheduleds[0].value,
                )
                && date <= ctx.today
            {
                plan.still_due_p0 += 1;
            }
            count_load(line, None, load_counts, ctx);
            continue;
        }
        if priorities.len() > 1 || scheduleds.len() > 1 {
            plan.skipped.push(Skip {
                path: display.clone(),
                line: task.line_index + 1,
                reason: SkipReason::DuplicateField,
                detail: None,
            });
            count_load(line, None, load_counts, ctx);
            continue;
        }
        let scheduled = &scheduleds[0];
        let Some(old_date) =
            task_fields::parse_strict_calendar_date(&scheduled.value)
        else {
            plan.skipped.push(Skip {
                path: display.clone(),
                line: task.line_index + 1,
                reason: SkipReason::InvalidScheduled,
                detail: None,
            });
            count_load(line, None, load_counts, ctx);
            continue;
        };
        if old_date > ctx.until {
            count_load(line, None, load_counts, ctx);
            continue;
        }
        let Some(level) = ctx.priority.level_for_value(&priorities[0].value)
        else {
            plan.skipped.push(Skip {
                path: display.clone(),
                line: task.line_index + 1,
                reason: SkipReason::UnknownPriority,
                detail: Some(priorities[0].value.clone()),
            });
            count_load(line, None, load_counts, ctx);
            continue;
        };
        if task_fields::has_any_field(line, &["due", "repeat"])
            || line.contains('📅')
            || line.contains('🔁')
        {
            plan.skipped.push(Skip {
                path: display.clone(),
                line: task.line_index + 1,
                reason: SkipReason::HardDate,
                detail: None,
            });
            count_load(line, None, load_counts, ctx);
            continue;
        }
        match task.status_symbol {
            ' ' | '?' => {}
            '*' => {
                plan.skipped.push(skip(task, &display, SkipReason::Next));
                count_load(line, None, load_counts, ctx);
                continue;
            }
            '/' => {
                plan.skipped
                    .push(skip(task, &display, SkipReason::InProgress));
                count_load(line, None, load_counts, ctx);
                continue;
            }
            _ => {
                plan.skipped.push(skip(
                    task,
                    &display,
                    SkipReason::OtherStatus,
                ));
                count_load(line, None, load_counts, ctx);
                continue;
            }
        }
        if let Some(block_id) = task.block_id.as_deref()
            && pomodoro_links.iter().any(|link| {
                link.block_id == block_id
                    && link_targets_note(&link.target, snapshot, ctx.daily_path)
            })
        {
            plan.skipped
                .push(skip(task, &display, SkipReason::Pomodoro));
            count_load(line, None, load_counts, ctx);
            continue;
        }
        if let Some(selected) = ctx.selected_labels
            && !selected
                .iter()
                .any(|label| label.eq_ignore_ascii_case(level.label()))
        {
            plan.skipped
                .push(skip(task, &display, SkipReason::NotSelected));
            plan.not_selected += 1;
            count_load(line, None, load_counts, ctx);
            continue;
        }

        let ordinal = digest_ordinals.entry(task.digest.clone()).or_insert(0);
        let ordinal_value = *ordinal;
        *ordinal += 1;
        let task_seed = config::derive_seed(
            ctx.seed,
            &[
                &relative_key(&snapshot.relative_path),
                task.digest.as_str(),
                &ordinal_value.to_string(),
            ],
        );
        let offset = level.roll_offset(task_seed);
        let Some(new_date) = checked_add_days(ctx.until, offset) else {
            return Err(format!(
                "priority window rolls beyond the supported date range from {}",
                ctx.until.format("%Y-%m-%d")
            ));
        };
        if new_date == old_date {
            plan.unchanged += 1;
            count_load(line, None, load_counts, ctx);
            continue;
        }
        let from = task_fields::format_calendar_date(old_date);
        let to = task_fields::format_calendar_date(new_date);
        let until_text = task_fields::format_calendar_date(ctx.until);
        let status_to = if new_date > ctx.today && task.status_symbol == ' ' {
            '?'
        } else {
            task.status_symbol
        };
        let reason = capture_schedule_log::randomize_reason(
            level.label(),
            offset,
            level.min_days(),
            level.max_days(),
            (ctx.until > ctx.today).then_some(until_text.as_str()),
        );
        let entry = capture_schedule_log::entry_text(Some(&from), &to, &reason);
        count_load(line, Some(new_date), load_counts, ctx);
        let reroll_index = note_rerolls.len();
        // Placeholder; the insertion kind is filled in when the write
        // applies bottom-up below.
        note_rerolls.push(Reroll {
            path: display.clone(),
            line: task.line_index + 1,
            task_ref: task.task_ref(),
            block_id: task.block_id.clone(),
            description: task.description.clone(),
            level: level.label().to_string(),
            value: level.value().to_string(),
            min_days: level.min_days(),
            max_days: level.max_days(),
            offset_days: offset,
            from,
            to: to.clone(),
            status_from: task.status_symbol,
            status_to,
            schedule_log: LogInsertion::Created,
        });
        jobs.push(WriteJob {
            line_index: task.line_index,
            value_start: scheduled.value_start,
            value_end: scheduled.value_end,
            new_date: to,
            status_from: task.status_symbol,
            status_to,
            entry,
            reroll_index,
        });
    }

    if jobs.is_empty() {
        return Ok(());
    }

    let original = snapshot.contents.clone();
    let mut current = original.clone();
    jobs.sort_by_key(|job| std::cmp::Reverse(job.line_index));
    for job in &jobs {
        let spans = line_spans(&current);
        let line_start = if job.line_index == 0 {
            0
        } else {
            spans[job.line_index - 1].end
        };
        let text = spans[job.line_index].text;
        let mut new_line = String::with_capacity(text.len());
        new_line.push_str(&text[..job.value_start]);
        new_line.push_str(&job.new_date);
        new_line.push_str(&text[job.value_end..]);
        if job.status_to != job.status_from {
            new_line = set_task_line_status(&new_line, job.status_to)
                .expect("scanned task line keeps its checkbox");
        }
        let text_end = line_start + text.len();
        current.replace_range(line_start..text_end, &new_line);
        let (next, insertion) = capture_schedule_log::plan_entry_insertion(
            &current,
            job.line_index,
            &job.entry,
        );
        current = next;
        note_rerolls[job.reroll_index].schedule_log = insertion;
    }

    let mut regrouped = false;
    if task_status_hooks::grouping_eligible_note(
        &snapshot.relative_path,
        &snapshot.path,
        &current,
        ctx.daily_path,
    ) {
        let classification =
            task_status_hooks::task_group_classification(ctx.tasks_settings);
        let transformed =
            task_status_groups::transform(&current, &classification);
        for warning in &transformed.warnings {
            plan.grouping_warnings.push(PlanGroupingWarning {
                path: display.clone(),
                original_heading_line: warning.original_heading_line,
                heading_ancestry: warning.heading_ancestry.clone(),
                code: warning.code.as_str().to_string(),
                message: warning.message.clone(),
            });
        }
        if transformed.changed {
            current = transformed.contents;
            regrouped = true;
        }
    }

    if current == original {
        return Ok(());
    }
    if note_rerolls.iter().any(|reroll| reroll.status_to == '?') {
        plan.needs_blocked_status = true;
    }
    plan.rerolls.extend(note_rerolls.clone());
    plan.notes.push(NotePlan {
        path: snapshot.path.clone(),
        relative_path: snapshot.relative_path.clone(),
        original,
        updated: current,
        rerolls: note_rerolls,
        regrouped,
    });
    Ok(())
}

fn display_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => {
                Some(part.to_string_lossy().into_owned())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn skip(task: &NoteTask, display: &str, reason: SkipReason) -> Skip {
    Skip {
        path: display.to_string(),
        line: task.line_index + 1,
        reason,
        detail: None,
    }
}

/// Count one open task line toward the 35-day load at its post-run
/// `scheduled` date: the rerolled date when given, else its single valid
/// date. Tasks without exactly one valid date cannot be placed. Only dates
/// in `today+1 ..= today+35` count.
fn count_load(
    line: &str,
    rerolled: Option<NaiveDate>,
    load_counts: &mut BTreeMap<NaiveDate, usize>,
    ctx: &PlanContext<'_>,
) {
    let single = task_fields::inline_fields(line, "scheduled");
    let date = rerolled.or_else(|| {
        (single.len() == 1)
            .then(|| task_fields::parse_strict_calendar_date(&single[0].value))
            .flatten()
    });
    let Some(horizon_end) = checked_add_days(ctx.today, 35) else {
        return;
    };
    if let Some(date) = date
        && date > ctx.today
        && date <= horizon_end
    {
        *load_counts.entry(date).or_insert(0) += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const TODAY: &str = "2026-09-28";

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("valid test date")
    }

    fn settings() -> TasksSettings {
        note_tasks::read_settings(Path::new("/nonexistent-test-vault"))
    }

    fn snapshot(path: &str, contents: &str) -> NoteSnapshot {
        NoteSnapshot {
            path: PathBuf::from(format!("/vault/{path}")),
            relative_path: PathBuf::from(path),
            contents: contents.to_string(),
        }
    }

    fn context<'a>(
        settings: &'a TasksSettings,
        property: &'a PriorityProperty,
        daily_path: &'a Path,
        daily_contents: Option<&'a str>,
    ) -> PlanContext<'a> {
        PlanContext {
            today: day(TODAY),
            until: day(TODAY),
            seed: 0x7f3a91c2,
            priority: property,
            selected_labels: None,
            tasks_settings: settings,
            daily_path,
            daily_contents,
        }
    }

    fn plan_single(
        contents: &str,
        settings: &TasksSettings,
        property: &PriorityProperty,
    ) -> Plan {
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let ctx = context(settings, property, &daily, None);
        plan_notes(&[snapshot("note.md", contents)], &ctx)
            .expect("test plan succeeds")
    }

    fn skip_reasons(plan: &Plan) -> Vec<&'static str> {
        plan.skipped
            .iter()
            .map(|skip| skip.reason.as_str())
            .collect::<Vec<_>>()
    }

    #[test]
    fn ignores_non_open_and_fieldless_and_foreign_tasks() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            concat!(
                "- [x] Done task [priority:: high] [scheduled:: 2026-09-10] #task\n",
                "- [-] Canceled task [priority:: high] [scheduled:: 2026-09-10] #task\n",
                "- [ ] No priority, future date [scheduled:: 2026-10-10] #task\n",
                "- [ ] No scheduled date [priority:: high] #task\n",
                "- [ ] Owned by projects [priority:: high] [scheduled:: 2026-09-10] #task ^prj\n",
            ),
            &settings,
            &property,
        );
        assert!(plan.rerolls.is_empty());
        assert!(plan.skipped.is_empty());
        assert!(plan.notes.is_empty());
        assert_eq!(plan.still_due_p0, 0);
        assert_eq!(plan.unchanged, 0);
        assert!(!plan.needs_blocked_status);
    }

    #[test]
    fn counts_due_p0_tasks_but_ignores_the_rest() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            concat!(
                "- [ ] Due P0 [scheduled:: 2026-09-10] #task\n",
                "- [ ] Due today P0 [scheduled:: 2026-09-28] #task\n",
                "- [ ] Future P0 [scheduled:: 2026-10-01] #task\n",
                "- [ ] Invalid P0 [scheduled:: someday] #task\n",
                "- [x] Done P0 [scheduled:: 2026-09-10] #task\n",
            ),
            &settings,
            &property,
        );
        assert_eq!(plan.still_due_p0, 2);
        assert!(plan.rerolls.is_empty());
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn skips_duplicate_priority_and_scheduled_fields() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            concat!(
                "- [ ] Two priorities [priority:: high] [priority:: medium] [scheduled:: 2026-09-10] #task\n",
                "- [ ] Two dates [priority:: high] [scheduled:: 2026-09-10] (scheduled:: 2026-09-11) #task\n",
            ),
            &settings,
            &property,
        );
        assert_eq!(
            skip_reasons(&plan),
            vec!["duplicate_field", "duplicate_field"]
        );
        assert!(plan.skipped.iter().all(|skip| skip.detail.is_none()));
        assert_eq!(plan.skipped[0].line, 1);
        assert_eq!(plan.skipped[1].line, 2);
        assert!(plan.rerolls.is_empty());
    }

    #[test]
    fn skips_unparseable_scheduled_dates() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            "- [ ] Bad date [priority:: high] [scheduled:: 2026-13-10] #task\n",
            &settings,
            &property,
        );
        assert_eq!(skip_reasons(&plan), vec!["invalid_scheduled"]);
        assert!(plan.rerolls.is_empty());
    }

    #[test]
    fn ignores_tasks_scheduled_after_until() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            "- [ ] Future [priority:: high] [scheduled:: 2026-09-29] #task\n",
            &settings,
            &property,
        );
        assert!(plan.rerolls.is_empty());
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn skips_unknown_priority_values_with_detail() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            "- [ ] Fancy [priority:: highest] [scheduled:: 2026-09-10] #task\n",
            &settings,
            &property,
        );
        assert_eq!(skip_reasons(&plan), vec!["unknown_priority"]);
        assert_eq!(plan.skipped[0].detail.as_deref(), Some("highest"));
        assert!(plan.rerolls.is_empty());
    }

    #[test]
    fn skips_hard_dates_from_fields_and_emoji() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            concat!(
                "- [ ] Deadline [priority:: high] [scheduled:: 2026-09-10] [due:: 2026-10-01] #task\n",
                "- [ ] Recurring [priority:: high] [scheduled:: 2026-09-10] [repeat:: every day] #task\n",
                "- [ ] Emoji due [priority:: high] [scheduled:: 2026-09-10] 📅 2026-10-01 #task\n",
                "- [ ] Emoji repeat [priority:: high] [scheduled:: 2026-09-10] 🔁 every day #task\n",
            ),
            &settings,
            &property,
        );
        assert_eq!(
            skip_reasons(&plan),
            vec!["hard_date", "hard_date", "hard_date", "hard_date"]
        );
        assert!(plan.rerolls.is_empty());
    }

    #[test]
    fn skips_non_ready_open_statuses() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            concat!(
                "- [*] Next task [priority:: high] [scheduled:: 2026-09-10] #task\n",
                "- [/] Active task [priority:: high] [scheduled:: 2026-09-10] #task\n",
                "- [Q] Custom task [priority:: high] [scheduled:: 2026-09-10] #task\n",
            ),
            &settings,
            &property,
        );
        assert_eq!(
            skip_reasons(&plan),
            vec!["next", "in_progress", "other_status"]
        );
        assert!(plan.rerolls.is_empty());
    }

    #[test]
    fn rerolls_ready_and_blocked_tasks() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            concat!(
                "- [ ] Ready [priority:: high] [scheduled:: 2026-09-10] #task\n",
                "- [?] Blocked [priority:: medium] [scheduled:: 2026-09-10] #task\n",
            ),
            &settings,
            &property,
        );
        assert_eq!(plan.rerolls.len(), 2);
        assert_eq!(plan.rerolls[0].status_from, ' ');
        assert_eq!(plan.rerolls[0].status_to, '?');
        assert_eq!(plan.rerolls[1].status_from, '?');
        assert_eq!(plan.rerolls[1].status_to, '?');
        assert!(plan.needs_blocked_status);
        let note = plan
            .notes
            .iter()
            .find(|note| note.relative_path == Path::new("note.md"))
            .expect("changed note");
        assert!(!note.regrouped);
        assert!(note.updated.contains("- [?] Ready"));
        assert!(note.updated.contains("- [?] Blocked"));
        assert!(!note.updated.contains("[scheduled:: 2026-09-10]"));
    }

    #[test]
    fn keeps_paren_fields_and_spacing_when_replacing_the_date() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            "- [ ] Spaced (priority::high) (scheduled ::  2026-09-10 ) #task\n",
            &settings,
            &property,
        );
        assert_eq!(plan.rerolls.len(), 1);
        let to = plan.rerolls[0].to.clone();
        let updated = &plan.notes[0].updated;
        assert!(
            updated.contains(&format!("(scheduled ::  {to} )")),
            "spacing and parens survive the date swap: {updated}"
        );
        assert!(updated.contains("(priority::high)"));
    }

    fn pomodoro_daily() -> String {
        concat!(
            "# 2026-09-28\n",
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FOCUS\n",
            "  - [[sase#^abc123]]\n",
            "  - ![[sase#^def456]]\n",
            "  - [[sase#^ghi789|alias]]\n",
            "  - [[elsewhere#^zzz000]]\n",
            "  - sub\n",
            "    - [[sase#^nested1]]\n",
            "- [x] (0800-0830) — DONE\n",
            "  - [[sase#^closed1]]\n",
            "- [ ] Daily task [priority:: high] [scheduled:: 2026-09-10] #task ^daily1\n",
            "- [ ] (1000-1030) — LATER\n",
            "  - [[#^daily1]]\n",
        )
        .to_string()
    }

    #[test]
    fn skips_tasks_linked_from_open_pomodoros_in_every_link_form() {
        let settings = settings();
        let property = config::test_property();
        let daily_contents = pomodoro_daily();
        let daily_path = PathBuf::from("/vault/2026/20260928.md");
        let ctx = PlanContext {
            today: day(TODAY),
            until: day(TODAY),
            seed: 0x7f3a91c2,
            priority: &property,
            selected_labels: None,
            tasks_settings: &settings,
            daily_path: &daily_path,
            daily_contents: Some(&daily_contents),
        };
        let plan = plan_notes(
            &[
                snapshot(
                    "sase.md",
                    concat!(
                        "- [ ] Plain link [priority:: high] [scheduled:: 2026-09-10] #task ^abc123\n",
                        "- [ ] Embed link [priority:: high] [scheduled:: 2026-09-10] #task ^def456\n",
                        "- [ ] Aliased link [priority:: high] [scheduled:: 2026-09-10] #task ^ghi789\n",
                        "- [ ] Nested link [priority:: high] [scheduled:: 2026-09-10] #task ^nested1\n",
                        "- [ ] Closed entry link [priority:: high] [scheduled:: 2026-09-10] #task ^closed1\n",
                        "- [ ] Other target [priority:: high] [scheduled:: 2026-09-10] #task ^zzz000\n",
                    ),
                ),
                snapshot("other.md", "- [ ] Elsewhere [priority:: high] [scheduled:: 2026-09-10] #task ^zzz000\n"),
                snapshot("notes/deep.md", "- [ ] Deep path [priority:: high] [scheduled:: 2026-09-10] #task ^deep1\n"),
                snapshot("2026/20260928.md", &daily_contents),
            ],
            &ctx,
        )
        .expect("test plan succeeds");
        let pomodoros = plan
            .skipped
            .iter()
            .filter(|skip| skip.reason == SkipReason::Pomodoro)
            .map(|skip| (skip.path.clone(), skip.line))
            .collect::<Vec<_>>();
        assert!(pomodoros.contains(&("sase.md".to_string(), 1)));
        assert!(pomodoros.contains(&("sase.md".to_string(), 2)));
        assert!(pomodoros.contains(&("sase.md".to_string(), 3)));
        assert!(pomodoros.contains(&("sase.md".to_string(), 4)));
        // A completed Pomodoro entry never protects its links.
        assert!(!pomodoros.contains(&("sase.md".to_string(), 5)));
        // A target naming another note does not match this one.
        assert!(!pomodoros.contains(&("sase.md".to_string(), 6)));
        assert!(!pomodoros.contains(&("other.md".to_string(), 1)));
        // The empty-target link means the daily note itself, so it skips
        // the daily note's own task.
        assert!(plan
            .skipped
            .iter()
            .any(|skip| skip.path == "2026/20260928.md"
                && skip.reason == SkipReason::Pomodoro));
    }

    #[test]
    fn matches_pomodoro_targets_by_path_stem_and_case() {
        let settings = settings();
        let property = config::test_property();
        let daily = concat!(
            "# 2026-09-28\n",
            "## Pomodoros\n",
            "- [ ] (0900-0930) — FOCUS\n",
            "  - [[notes/deep#^deep1]]\n",
            "  - [[DEEP#^deep2]]\n",
            "  - [[Sase#^stem1]]\n",
        );
        let daily_path = PathBuf::from("/vault/2026/20260928.md");
        let ctx = PlanContext {
            today: day(TODAY),
            until: day(TODAY),
            seed: 1,
            priority: &property,
            selected_labels: None,
            tasks_settings: &settings,
            daily_path: &daily_path,
            daily_contents: Some(daily),
        };
        let plan = plan_notes(
            &[
                snapshot(
                    "notes/deep.md",
                    concat!(
                        "- [ ] By path [priority:: high] [scheduled:: 2026-09-10] #task ^deep1\n",
                        "- [ ] By stem [priority:: high] [scheduled:: 2026-09-10] #task ^deep2\n",
                    ),
                ),
                snapshot(
                    "sase.md",
                    "- [ ] By stem case [priority:: high] [scheduled:: 2026-09-10] #task ^stem1\n",
                ),
            ],
            &ctx,
        )
        .expect("test plan succeeds");
        assert_eq!(plan.skipped.len(), 3);
        assert!(plan
            .skipped
            .iter()
            .all(|skip| skip.reason == SkipReason::Pomodoro));
        assert!(plan.rerolls.is_empty());
    }

    #[test]
    fn until_shifts_both_the_cutoff_and_the_roll_base() {
        let settings = settings();
        let property = config::test_property();
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let line =
            "- [ ] Later [priority:: medium] [scheduled:: 2026-10-02] #task\n";
        let today_ctx = context(&settings, &property, &daily, None);
        let today_plan = plan_notes(&[snapshot("note.md", line)], &today_ctx)
            .expect("test plan succeeds");
        assert!(today_plan.rerolls.is_empty());
        assert!(today_plan.skipped.is_empty());

        let mut until_ctx = context(&settings, &property, &daily, None);
        until_ctx.until = day("2026-10-05");
        let plan = plan_notes(&[snapshot("note.md", line)], &until_ctx)
            .expect("test plan succeeds");
        assert_eq!(plan.rerolls.len(), 1);
        let reroll = &plan.rerolls[0];
        assert_eq!(reroll.from, "2026-10-02");
        assert!(reroll.to.as_str() >= "2026-10-13");
        assert!(reroll.to.as_str() <= "2026-11-04");
        assert!(plan.notes[0].updated.contains(&format!(
            "*2026-10-02 → {}* — 🎲 P2 randomize · in **{}** (8–30) days from 2026-10-05",
            reroll.to, reroll.offset_days
        )));
    }

    #[test]
    fn level_filtering_counts_not_selected() {
        let settings = settings();
        let property = config::test_property();
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let selected = vec!["P2".to_string()];
        let ctx = PlanContext {
            selected_labels: Some(&selected),
            ..context(&settings, &property, &daily, None)
        };
        let plan = plan_notes(
            &[snapshot(
                "note.md",
                concat!(
                    "- [ ] P1 task [priority:: high] [scheduled:: 2026-09-10] #task\n",
                    "- [ ] P2 task [priority:: medium] [scheduled:: 2026-09-10] #task\n",
                ),
            )],
            &ctx,
        )
        .expect("test plan succeeds");
        assert_eq!(plan.rerolls.len(), 1);
        assert_eq!(plan.rerolls[0].level, "P2");
        assert_eq!(skip_reasons(&plan), vec!["not_selected"]);
        assert_eq!(plan.not_selected, 1);
    }

    #[test]
    fn level_filtering_matches_labels_case_insensitively() {
        let settings = settings();
        let property = config::test_property();
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let selected = vec!["p1".to_string()];
        let ctx = PlanContext {
            selected_labels: Some(&selected),
            ..context(&settings, &property, &daily, None)
        };
        let plan = plan_notes(
            &[snapshot(
                "note.md",
                "- [ ] P1 task [priority:: high] [scheduled:: 2026-09-10] #task\n",
            )],
            &ctx,
        )
        .expect("test plan succeeds");
        assert_eq!(plan.rerolls.len(), 1);
        assert_eq!(plan.not_selected, 0);
    }

    #[test]
    fn zero_width_window_reports_unchanged_without_writing() {
        let settings = settings();
        let property = config::parse_test_property(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 0
        max_days: 0
"#,
        );
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let mut ctx = context(&settings, &property, &daily, None);
        ctx.seed = 99;
        let plan = plan_notes(
            &[snapshot(
                "note.md",
                "- [ ] Same day [priority:: high] [scheduled:: 2026-09-28] #task\n",
            )],
            &ctx,
        )
        .expect("test plan succeeds");
        assert_eq!(plan.unchanged, 1);
        assert!(plan.rerolls.is_empty());
        assert!(plan.notes.is_empty());
        assert!(!plan.needs_blocked_status);
    }

    #[test]
    fn same_inputs_give_the_same_plan() {
        let settings = settings();
        let property = config::test_property();
        let contents = concat!(
            "- [ ] First [priority:: high] [scheduled:: 2026-09-10] #task\n",
            "- [ ] Second [priority:: medium] [scheduled:: 2026-09-11] #task\n",
        );
        let first = plan_single(contents, &settings, &property);
        let second = plan_single(contents, &settings, &property);
        assert_eq!(first, second);
    }

    #[test]
    fn dates_survive_insertions_above_because_identity_ignores_lines() {
        let settings = settings();
        let property = config::test_property();
        let before = plan_single(
            "- [ ] Anchored [priority:: medium] [scheduled:: 2026-09-10] #task\n",
            &settings,
            &property,
        );
        let after = plan_single(
            concat!(
                "- [ ] Unrelated [priority:: high] [scheduled:: 2026-09-12] #task\n",
                "- [ ] More [priority:: high] [scheduled:: 2026-09-13] #task\n",
                "- [ ] Anchored [priority:: medium] [scheduled:: 2026-09-10] #task\n",
            ),
            &settings,
            &property,
        );
        let anchored_before = before
            .rerolls
            .iter()
            .find(|reroll| reroll.description == "Anchored")
            .expect("anchored reroll");
        let anchored_after = after
            .rerolls
            .iter()
            .find(|reroll| reroll.description == "Anchored")
            .expect("anchored reroll after insertions");
        assert_eq!(anchored_before.to, anchored_after.to);
        assert_eq!(anchored_before.line, 1);
        assert_eq!(anchored_after.line, 3);
        assert_eq!(
            &anchored_before.task_ref[2..],
            &anchored_after.task_ref[2..],
            "same line text keeps the same digest after a line shift"
        );
    }

    #[test]
    fn identical_lines_get_distinct_rolls() {
        let settings = settings();
        let property = config::test_property();
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let mut ctx = context(&settings, &property, &daily, None);
        ctx.seed = 3;
        let plan = plan_notes(
            &[snapshot(
                "note.md",
                concat!(
                    "- [ ] Twin [priority:: medium] [scheduled:: 2026-09-10] #task\n",
                    "- [ ] Twin [priority:: medium] [scheduled:: 2026-09-10] #task\n",
                ),
            )],
            &ctx,
        )
        .expect("test plan succeeds");
        assert_eq!(plan.rerolls.len(), 2);
        assert_eq!(plan.rerolls[0].line, 1);
        assert_eq!(plan.rerolls[1].line, 2);
        assert_eq!(
            &plan.rerolls[0].task_ref[2..],
            &plan.rerolls[1].task_ref[2..],
            "identical lines share a digest"
        );
        assert_ne!(
            plan.rerolls[0].to, plan.rerolls[1].to,
            "identical lines must roll distinct dates via the digest ordinal"
        );
    }

    fn fixed_window_property() -> PriorityProperty {
        config::parse_test_property(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 4
        max_days: 4
"#,
        )
    }

    #[test]
    fn fixed_offsets_cross_month_year_and_leap_boundaries() {
        let settings = settings();
        let property = fixed_window_property();
        let daily = PathBuf::from("/vault/2026/20260928.md");
        for (until, scheduled, expected) in [
            ("2026-09-28", "2026-09-10", "2026-10-02"),
            ("2026-12-30", "2026-12-01", "2027-01-03"),
            ("2024-02-26", "2024-02-01", "2024-03-01"),
            ("2024-03-05", "2024-02-29", "2024-03-09"),
            ("2025-02-24", "2025-02-01", "2025-02-28"),
        ] {
            let mut ctx = context(&settings, &property, &daily, None);
            ctx.until = day(until);
            let plan = plan_notes(
                &[snapshot(
                    "note.md",
                    &format!(
                        "- [ ] Dated [priority:: high] [scheduled:: {scheduled}] #task\n",
                    ),
                )],
                &ctx,
            )
            .expect("test plan succeeds");
            assert_eq!(plan.rerolls.len(), 1, "until {until}");
            assert_eq!(plan.rerolls[0].offset_days, 4);
            assert_eq!(plan.rerolls[0].to, expected, "until {until}");
        }
    }

    #[test]
    fn project_note_postimage_moves_the_task_under_blocked() {
        let settings = settings();
        let property = config::test_property();
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let ctx = context(&settings, &property, &daily, None);
        let contents = concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "# Project\n",
            "\n",
            "## Tasks\n",
            "\n",
            "- [ ] Ship it [priority:: medium] [scheduled:: 2026-09-10] #task\n",
        );
        let plan = plan_notes(&[snapshot("proj.md", contents)], &ctx)
            .expect("test plan succeeds");
        assert_eq!(plan.rerolls.len(), 1);
        let reroll = &plan.rerolls[0];
        assert_eq!(reroll.status_to, '?');
        assert_eq!(reroll.schedule_log, LogInsertion::Created);
        let note = &plan.notes[0];
        assert!(note.regrouped);
        assert!(plan.grouping_warnings.is_empty());
        let expected_task = format!(
            "- [?] Ship it [priority:: medium] [scheduled:: {}] #task",
            reroll.to
        );
        let expected_entry = format!(
            "*2026-09-10 → {}* — 🎲 P2 randomize · in **{}** (8–30) days",
            reroll.to, reroll.offset_days
        );
        for line in [&expected_task, &expected_entry, "### Blocked"] {
            assert!(
                note.updated.contains(line),
                "missing {line:?} in postimage:\n{}",
                note.updated
            );
        }
        assert_eq!(note.original, contents);
    }

    #[test]
    fn project_postimage_is_byte_exact_with_blocked_move_and_badge() {
        let settings = settings();
        let property = config::test_property();
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let ctx = context(&settings, &property, &daily, None);
        let contents = concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "# Project\n",
            "\n",
            "## Tasks\n",
            "\n",
            "- [ ] Ship it [priority:: medium] [scheduled:: 2026-09-10] #task\n",
        );
        let plan = plan_notes(&[snapshot("proj.md", contents)], &ctx)
            .expect("test plan succeeds");
        // Seed 0x7f3a91c2 rolls this task 16 days out, to 2026-10-14.
        assert_eq!(plan.rerolls[0].to, "2026-10-14");
        assert_eq!(
            plan.notes[0].updated,
            concat!(
                "---\n",
                "type: [[project]]\n",
                "---\n",
                "# Project\n",
                "\n",
                "## Tasks\n",
                "<!-- bob:task-status-badges:v1 -->\n",
                "[`⚪ 0 open`](#Project#Tasks) · [`🔵 0 next/wip`](#Project#Tasks#Next%20&%20In%20Progress) · [`🔴 1 blocked`](#Project#Tasks#Blocked) · [`🟢 0 done/canceled`](#Project#Tasks#Done%20&%20Canceled)\n",
                "\n",
                "### Next & In Progress\n",
                "<!-- bob:task-status-group:v1:active -->\n",
                "\n",
                "### Blocked\n",
                "<!-- bob:task-status-group:v1:blocked -->\n",
                "\n",
                "- [?] Ship it [priority:: medium] [scheduled:: 2026-10-14] #task\n",
                "\t- 🗓️ **SCHEDULE LOG**\n",
                "\t\t- *2026-09-10 → 2026-10-14* — 🎲 P2 randomize · in **16** (8–30) days\n",
                "\n",
                "### Done & Canceled\n",
                "<!-- bob:task-status-group:v1:closed -->\n",
            )
        );
    }

    #[test]
    fn ordinary_and_daily_notes_are_edited_but_not_regrouped() {
        let settings = settings();
        let property = config::test_property();
        let daily_path = PathBuf::from("/vault/today.md");
        let ctx = context(&settings, &property, &daily_path, None);
        let ordinary = concat!(
            "---\n",
            "type: [[note]]\n",
            "---\n",
            "## Tasks\n",
            "\n",
            "- [ ] Plain [priority:: high] [scheduled:: 2026-09-10] #task\n",
        );
        let daily_contents = concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "## Tasks\n",
            "\n",
            "- [ ] Daily [priority:: high] [scheduled:: 2026-09-10] #task\n",
        );
        let canonical_daily = concat!(
            "---\n",
            "type: [[project]]\n",
            "---\n",
            "## Tasks\n",
            "\n",
            "- [ ] Canonical [priority:: high] [scheduled:: 2026-09-10] #task\n",
        );
        let plan = plan_notes(
            &[
                snapshot("ordinary.md", ordinary),
                snapshot("today.md", daily_contents),
                snapshot("2026/20260928.md", canonical_daily),
            ],
            &ctx,
        )
        .expect("test plan succeeds");
        assert_eq!(plan.rerolls.len(), 3);
        assert_eq!(plan.notes.len(), 3);
        for note in &plan.notes {
            assert!(
                !note.regrouped,
                "{} must not regroup",
                note.relative_path.display()
            );
            assert!(
                !note.updated.contains("### Blocked"),
                "{} must not gain a Blocked section",
                note.relative_path.display()
            );
            assert!(
                note.updated.contains("- [?]"),
                "{} keeps its status flip",
                note.relative_path.display()
            );
        }
    }

    #[test]
    fn load_covers_35_days_with_p0_and_new_dates() {
        let settings = settings();
        let property = fixed_window_property();
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let mut ctx = context(&settings, &property, &daily, None);
        ctx.until = day("2026-09-28");
        let plan = plan_notes(
            &[snapshot(
                "note.md",
                concat!(
                    "- [ ] Rolled [priority:: high] [scheduled:: 2026-09-10] #task\n",
                    "- [ ] Future P0 [scheduled:: 2026-09-30] #task\n",
                    "- [ ] Past P0 [scheduled:: 2026-09-27] #task\n",
                ),
            )],
            &ctx,
        )
        .expect("test plan succeeds");
        assert_eq!(plan.rerolls.len(), 1);
        assert_eq!(plan.rerolls[0].to, "2026-10-02");
        assert_eq!(plan.load.len(), 35);
        assert_eq!(plan.load[0].date, day("2026-09-29"));
        assert_eq!(plan.load[34].date, day("2026-11-02"));
        let count_on = |text: &str| {
            let want = day(text);
            plan.load
                .iter()
                .find(|entry| entry.date == want)
                .map(|entry| entry.count)
                .unwrap_or(0)
        };
        assert_eq!(count_on("2026-09-30"), 1, "future P0 counts");
        assert_eq!(count_on("2026-10-02"), 1, "new date counts");
        assert_eq!(
            plan.load.iter().map(|day| day.count).sum::<usize>(),
            2,
            "past dates and today never count"
        );
    }

    #[test]
    fn reroll_carries_every_json_task_field() {
        let settings = settings();
        let property = config::test_property();
        let plan = plan_single(
            "- [ ] Documented [priority:: medium] [scheduled:: 2026-09-10] #task ^doc1\n",
            &settings,
            &property,
        );
        assert_eq!(plan.rerolls.len(), 1);
        let reroll = &plan.rerolls[0];
        assert_eq!(reroll.path, "note.md");
        assert_eq!(reroll.line, 1);
        assert!(reroll.task_ref.starts_with("1:"));
        assert_eq!(reroll.task_ref.len(), 10);
        assert_eq!(reroll.block_id.as_deref(), Some("doc1"));
        assert_eq!(reroll.description, "Documented");
        assert_eq!(reroll.level, "P2");
        assert_eq!(reroll.value, "medium");
        assert_eq!(reroll.min_days, 8);
        assert_eq!(reroll.max_days, 30);
        assert!(
            reroll.offset_days >= 8 && reroll.offset_days <= 30,
            "offset {} outside P2 window",
            reroll.offset_days
        );
        assert_eq!(reroll.from, "2026-09-10");
        assert_eq!(reroll.status_from, ' ');
        assert_eq!(reroll.status_to, '?');
        let note = &plan.notes[0];
        assert_eq!(note.original.lines().count(), 1);
        assert!(note.updated.lines().count() > 1);
    }

    #[test]
    fn extreme_priority_window_fails_before_any_write() {
        let settings = settings();
        let property = config::parse_test_property(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 9223372036854775807
        max_days: 9223372036854775807
"#,
        );
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let ctx = context(&settings, &property, &daily, None);
        let error = plan_notes(
            &[snapshot(
                "note.md",
                "- [ ] Far out [priority:: high] [scheduled:: 2026-09-10] #task\n",
            )],
            &ctx,
        )
        .expect_err("extreme roll must not produce a date");
        assert!(
            error.contains("supported date range"),
            "unexpected planner error: {error}"
        );
    }

    #[test]
    fn roll_at_the_representable_boundary_succeeds_and_past_it_fails() {
        let settings = settings();
        let arrival = config::parse_test_property(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 0
        max_days: 0
"#,
        );
        let daily = PathBuf::from("/vault/2026/20260928.md");
        let mut ctx = context(&settings, &arrival, &daily, None);
        ctx.until = NaiveDate::MAX;
        let plan = plan_notes(
            &[snapshot(
                "note.md",
                "- [ ] Edge [priority:: high] [scheduled:: 2026-09-10] #task\n",
            )],
            &ctx,
        )
        .expect("a roll landing exactly on the boundary succeeds");
        assert_eq!(plan.rerolls.len(), 1);
        assert_eq!(
            plan.rerolls[0].to,
            task_fields::format_calendar_date(NaiveDate::MAX)
        );

        let departure = config::parse_test_property(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 1
        max_days: 1
"#,
        );
        let ctx = PlanContext {
            priority: &departure,
            until: NaiveDate::MAX,
            ..context(&settings, &arrival, &daily, None)
        };
        let error = plan_notes(
            &[snapshot(
                "note.md",
                "- [ ] Edge [priority:: high] [scheduled:: 2026-09-10] #task\n",
            )],
            &ctx,
        )
        .expect_err("a roll past the boundary must fail");
        assert!(
            error.contains("supported date range"),
            "unexpected planner error: {error}"
        );
    }
}
