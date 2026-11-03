//! `bob periods brief`: the deterministic, versioned agent bundle.
//!
//! The contract lives in `docs/periods.md`. The brief (brief v1) is the
//! only source of numbers an agent may use when drafting a period
//! Review: header, facts, time, parts, finished, started and dropped,
//! still open, practice (year only), Bryan's words, this note and its
//! neighbours, off-clock counts, and caveats. Markdown and JSON carry
//! the same content. `all_time` has no review.

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    calendar::{PeriodId, PeriodState},
    cli, note, report,
};
use crate::native::{
    config, env as bob_env,
    stats::{
        clock::StatsClock,
        homes::HomesEngine,
        index::{IndexedTask, StatsIndex, TaskKindPart},
        series::format_minutes,
    },
};

const COMMAND_NAME: &str = "bob periods brief";
const SCHEMA_VERSION: u32 = 1;
const BRIEF_VERSION: &str = "brief v1";
const WEEK_BUDGET: usize = 6000;
const QUARTER_BUDGET: usize = 10_000;
const YEAR_BUDGET: usize = 12_000;

#[derive(Debug, Clone)]
struct BriefError {
    code: String,
    message: String,
    hint: String,
    exit_code: i32,
}

impl BriefError {
    fn usage(code: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            message,
            hint: "Pass a period id such as 2026-W45, 2026-Q4, or 2026. See docs/periods.md.".to_string(),
            exit_code: 2,
        }
    }

    fn failed(code: String, message: String, hint: String, exit: i32) -> Self {
        Self {
            code,
            message,
            hint,
            exit_code: exit,
        }
    }

    fn json(&self) -> Value {
        json!({
            "ok": false,
            "schema_version": SCHEMA_VERSION,
            "error": {
                "code": self.code,
                "message": self.message,
                "hint": self.hint,
            }
        })
    }
}

impl From<report::ShowError> for BriefError {
    fn from(error: report::ShowError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            hint: error.hint,
            exit_code: error.exit_code,
        }
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let command = cli::brief_command();
    let mut command = command;
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let human = matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("human")
        != "json";
    match execute(&matches) {
        Ok(bundle) => {
            if human {
                print!("{}", bundle.markdown);
            } else {
                println!(
                    "{}",
                    serde_json::to_string(&bundle.json).unwrap_or_default()
                );
            }
            0
        }
        Err(error) => {
            if human {
                eprintln!("{COMMAND_NAME}: {}", error.message);
            } else {
                println!(
                    "{}",
                    serde_json::to_string(&error.json()).unwrap_or_default()
                );
            }
            error.exit_code
        }
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

struct Bundle {
    markdown: String,
    json: Value,
}

struct Ctx {
    id: PeriodId,
    kind: &'static str,
    today: NaiveDate,
    generated_at: String,
    vault: PathBuf,
    report: Value,
    index: StatsIndex,
    buffers: HashMap<String, String>,
    /// On-clock linked minutes per task key (`source_path || block_id`).
    linked: HashMap<String, u32>,
    /// Minutes per root key across on-clock sessions in the observed span.
    _roots: BTreeMap<String, u32>,
    state: PeriodState,
    observed_end: Option<NaiveDate>,
    span_start: NaiveDate,
    finish_cap: usize,
}

fn execute(matches: &clap::ArgMatches) -> Result<Bundle, BriefError> {
    let id = matches
        .get_one::<String>("period")
        .map(|text| {
            PeriodId::parse(text)
                .map_err(|message| BriefError::usage("invalid_period", message))
        })
        .transpose()?
        .unwrap_or_else(|| {
            PeriodId::week_for_date(bob_env::current_datetime().date())
        });
    if matches!(id, PeriodId::AllTime) {
        return Err(BriefError::usage(
            "no_review_section",
            "all_time has no review".to_string(),
        ));
    }
    let vault = bob_env::bob_dir();
    let now = bob_env::current_datetime();
    let today = now.date();
    let report = report::build_report(
        &vault,
        config::config_path(),
        id,
        now,
        StatsClock::default(),
    )
    .map_err(BriefError::from)?;
    let index = StatsIndex::read(&vault, now).map_err(|error| {
        BriefError::failed(
            error.code().to_string(),
            error.message().to_string(),
            match error.exit_code() {
                1 => "Check the vault path and file permissions.".to_string(),
                _ => "Check the stats configuration and docs/periods.md."
                    .to_string(),
            },
            error.exit_code(),
        )
    })?;
    let state = id.state(today);
    let span =
        report::period_span(id, &vault, today).map_err(BriefError::from)?;
    let observed_end = match state {
        PeriodState::Future => None,
        PeriodState::Current => Some(today),
        PeriodState::Closed => Some(span.end),
    };
    let finish_cap = match id {
        PeriodId::Week { .. } => 7,
        PeriodId::Quarter { .. } => 15,
        PeriodId::Year(_) => 25,
        PeriodId::AllTime => unreachable!("all_time refused above"),
    };
    let buffers: HashMap<String, String> = index
        .buffers
        .iter()
        .map(|(path, contents)| (path.clone(), contents.clone()))
        .collect();
    let mut ctx = Ctx {
        kind: match id {
            PeriodId::Week { .. } => "week",
            PeriodId::Quarter { .. } => "quarter",
            PeriodId::Year(_) => "year",
            PeriodId::AllTime => unreachable!("all_time refused above"),
        },
        id,
        today,
        generated_at: now.format("%Y-%m-%dT%H:%M:%S").to_string(),
        vault,
        report,
        index,
        buffers,
        linked: HashMap::new(),
        _roots: BTreeMap::new(),
        state,
        observed_end,
        span_start: span.start,
        finish_cap,
    };
    ctx.linked = linked_minutes(&ctx);
    let mut json = build_json(&mut ctx)?;
    let mut markdown = render_markdown(&ctx, &json);
    enforce_budget(&ctx, &mut json, &mut markdown);
    Ok(Bundle { markdown, json })
}

/// Canonical key for matching a session task link to its task row.
fn task_key(source_path: &str, block_id: &str) -> String {
    format!(
        "{} || {}",
        source_path.trim().to_ascii_lowercase(),
        block_id.trim().to_ascii_lowercase()
    )
}

fn normalize_target(target: &str) -> String {
    let stem = target.trim();
    let stem = stem.strip_suffix(".md").unwrap_or(stem);
    stem.trim().to_ascii_lowercase()
}

/// On-clock minutes in the observed span, credited in full to every
/// task linked from the session.
fn linked_minutes(ctx: &Ctx) -> HashMap<String, u32> {
    let Some(end) = ctx.observed_end else {
        return HashMap::new();
    };
    // Task lookup by lower-cased block id: sessions link tasks by
    // `(target, block_id)`, and tasks are unique per note.
    let mut by_block: HashMap<String, Vec<&IndexedTask>> = HashMap::new();
    for task in &ctx.index.tasks {
        if let Some(block) = task.block_id.as_deref()
            && !block.trim().is_empty()
        {
            by_block
                .entry(block.to_ascii_lowercase())
                .or_default()
                .push(task);
        }
    }
    let mut linked = HashMap::new();
    for (date, daily) in &ctx.index.daily_notes {
        if *date < ctx.span_start || *date > end {
            continue;
        }
        for session in &daily.closed_pomodoros {
            if session.stats_clock() != Some(StatsClock::Worked) {
                continue;
            }
            let Some(minutes) = session.counted_minutes() else {
                continue;
            };
            for link in &session.links {
                if link.block_id.trim().is_empty() {
                    continue;
                }
                let Some(candidates) =
                    by_block.get(&link.block_id.to_ascii_lowercase())
                else {
                    continue;
                };
                // A bare `[[#^block]]` link targets the daily note
                // itself, matching the homes attribution.
                let target = if link.target.trim().is_empty() {
                    normalize_target(&daily.path)
                } else {
                    normalize_target(&link.target)
                };
                for task in candidates {
                    let source = normalize_target(&task.source_path);
                    if source != target {
                        continue;
                    }
                    if let Some(block) = task.block_id.as_deref() {
                        *linked
                            .entry(task_key(&task.source_path, block))
                            .or_insert(0) += minutes;
                    }
                }
            }
        }
    }
    linked
}

fn linked_for(task: &IndexedTask, ctx: &Ctx) -> u32 {
    let Some(block) = task.block_id.as_deref() else {
        return 0;
    };
    ctx.linked
        .get(&task_key(&task.source_path, block))
        .copied()
        .unwrap_or(0)
}

fn stem_of(path: &str) -> &str {
    path.strip_suffix(".md").unwrap_or(path)
}

fn short_description(text: &str, max_chars: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        return one_line;
    }
    let kept: String =
        one_line.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// A ready-to-paste task link: `[[stem#^block|description]]`.
fn task_link(task: &IndexedTask) -> String {
    let stem = stem_of(&task.source_path);
    let description =
        short_description(&clean_task_text(&task.description), 120);
    match task.block_id.as_deref() {
        Some(block) if !block.trim().is_empty() => {
            format!("[[{stem}#^{block}|{description}]]")
        }
        _ => format!("[[{stem}|{description}]]"),
    }
}

/// Strip `[key:: value]` inline fields from a task description for
/// display. Bracket text without `::` is kept.
fn clean_task_text(description: &str) -> String {
    let mut cleaned = String::with_capacity(description.len());
    let mut rest = description;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        match after.find(']') {
            Some(end) if after[..end].contains("::") => {
                cleaned.push_str(rest[..start].trim_end());
                cleaned.push(' ');
                rest = &after[end + 1..];
            }
            _ => {
                cleaned.push_str(rest);
                rest = "";
            }
        }
    }
    cleaned.push_str(rest);
    cleaned
        .split_whitespace()
        .filter(|word| !word.eq_ignore_ascii_case("#prj_is_blocked"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A ready-to-paste note link: `[[stem|label]]`.
fn note_link(path: &str, label: &str) -> String {
    format!("[[{}|{label}]]", stem_of(path))
}

fn build_json(ctx: &mut Ctx) -> Result<Value, BriefError> {
    let header = header_section(ctx);
    let facts = facts_section(ctx);
    let time = time_section(ctx)?;
    let parts = parts_section(ctx);
    let finished = finished_section(ctx);
    let started = started_section(ctx);
    let dropped = dropped_section(ctx);
    let still_open = still_open_section(ctx);
    let practice = practice_section(ctx);
    let bryans_words = bryans_words_section(ctx);
    let neighbours = neighbours_section(ctx);
    let offclock = offclock_section(ctx);
    let caveats = caveats_section(ctx);
    Ok(json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "brief_version": BRIEF_VERSION,
        "today": ctx.today.format("%Y-%m-%d").to_string(),
        "generated_at": ctx.generated_at,
        "id": ctx.id.to_string(),
        "kind": ctx.kind,
        "header": header,
        "facts": facts,
        "time": time,
        "parts": parts,
        "finished": finished,
        "started": started,
        "dropped": dropped,
        "still_open": still_open,
        "practice": practice,
        "bryans_words": bryans_words,
        "neighbours": neighbours,
        "offclock": offclock,
        "caveats": caveats,
    }))
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn header_section(ctx: &Ctx) -> Value {
    let report = &ctx.report;
    let title =
        str_field(report.get("period").unwrap_or(&Value::Null), "title");
    let state_name = match ctx.state {
        PeriodState::Future => "future",
        PeriodState::Current => "open",
        PeriodState::Closed => "closed",
    };
    let review_state = report
        .get("note")
        .and_then(|note| note.get("review"))
        .and_then(|review| review.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("empty")
        .to_string();
    let note_path = report
        .get("note")
        .and_then(|note| note.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let prev_path = report
        .get("links")
        .and_then(|links| links.get("prev"))
        .and_then(|prev| prev.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parent_path = report
        .get("links")
        .and_then(|links| links.get("parent"))
        .and_then(|parent| parent.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let noun = match ctx.kind {
        "quarter" => "quarter",
        "year" => "year",
        _ => "week",
    };
    let stop_advice = if matches!(ctx.state, PeriodState::Future) {
        format!("This {noun} has not started: do not draft it.")
    } else if matches!(ctx.state, PeriodState::Current) {
        format!(
            "This {noun} is open: draft only if Bryan asked for \"so far\"."
        )
    } else if review_state == "edited" || review_state == "reflected" {
        format!(
            "This {noun} is closed but its Review is {review_state}: draft only if Bryan said \"redo\"."
        )
    } else {
        format!("This {noun} is closed: ready to draft.")
    };
    json!({
        "id": ctx.id.to_string(),
        "title": title,
        "state": state_name,
        "review_state": review_state,
        "note_path": note_path,
        "prev_path": prev_path,
        "parent_path": parent_path,
        "stop_advice": stop_advice,
    })
}

fn facts_section(ctx: &Ctx) -> Value {
    let empty = Vec::new();
    let report_facts = ctx
        .report
        .get("facts")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let facts: Vec<Value> = report_facts
        .iter()
        .map(|fact| {
            let typical_words = fact
                .get("typical")
                .and_then(|typical| typical.get("words"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            json!({
                "label": str_field(fact, "label"),
                "display": str_field(fact, "display"),
                "so_far": fact.get("so_far").and_then(Value::as_bool).unwrap_or(false),
                "typical_words": typical_words,
            })
        })
        .collect();
    let coverage = ctx
        .report
        .get("period")
        .and_then(|period| period.get("coverage"))
        .and_then(|coverage| coverage.get("display"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    json!({ "facts": facts, "coverage": coverage })
}

fn share_percent(share: &str) -> f64 {
    share
        .trim()
        .strip_suffix('%')
        .unwrap_or(share.trim())
        .parse::<f64>()
        .unwrap_or(0.0)
}

fn lane_rows<'a>(ctx: &'a Ctx, lane: &str) -> Vec<&'a Value> {
    ctx.report
        .get("lanes")
        .and_then(|lanes| lanes.get(lane))
        .and_then(Value::as_array)
        .map(|rows| rows.iter().collect())
        .unwrap_or_default()
}

fn time_section(ctx: &mut Ctx) -> Result<Value, BriefError> {
    match ctx.id {
        PeriodId::Week { .. } => Ok(week_time(ctx)),
        PeriodId::Quarter { .. } => quarter_time(ctx),
        PeriodId::Year(_) => Ok(year_time(ctx)),
        PeriodId::AllTime => unreachable!("all_time refused above"),
    }
}

/// Root parents at or above 5% of Worked, each with its top three
/// tasks by linked minutes.
fn week_time(ctx: &mut Ctx) -> Value {
    let engine = HomesEngine::new(&ctx.index);
    // Minutes per task plus the task row, for attribution.
    let mut scored: Vec<(&IndexedTask, u32)> = ctx
        .index
        .tasks
        .iter()
        .map(|task| (task, linked_for(task, ctx)))
        .filter(|(_, minutes)| *minutes > 0)
        .collect();
    scored.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then(left.0.description.cmp(&right.0.description))
    });
    // Group tasks by resolved root parent.
    let mut by_root: BTreeMap<String, Vec<(&IndexedTask, u32)>> =
        BTreeMap::new();
    for (task, minutes) in scored {
        let home = engine.home_for_task(
            &task.source_path,
            task.line_number,
            &task.line,
        );
        by_root
            .entry(home.root_parent)
            .or_default()
            .push((task, minutes));
    }
    let mut roots = Vec::new();
    for row in lane_rows(ctx, "root_parent") {
        if share_percent(&str_field(row, "share")) < 5.0 {
            continue;
        }
        let key = str_field(row, "key");
        let tasks: Vec<Value> = by_root
            .get(&key)
            .map(|scored| {
                scored
                    .iter()
                    .take(3)
                    .map(|(task, minutes)| {
                        json!({
                            "link": task_link(task),
                            "minutes": minutes,
                            "display": format_minutes(i64::from(*minutes)),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Folded type lanes, if any, ride along for context.
        roots.push(json!({
            "label": str_field(row, "label"),
            "display": str_field(row, "display"),
            "share": str_field(row, "share"),
            "tasks": tasks,
        }));
    }
    let folded: Vec<Value> = lane_rows(ctx, "theme")
        .into_iter()
        .map(|row| {
            json!({
                "label": str_field(row, "label"),
                "display": str_field(row, "display"),
                "share": str_field(row, "share"),
            })
        })
        .collect();
    json!({ "roots": roots, "types": folded })
}

/// Root-parent shares beside the previous quarter's shares.
fn quarter_time(ctx: &Ctx) -> Result<Value, BriefError> {
    let empty = Vec::new();
    let rows = lane_rows(ctx, "root_parent");
    let mut prev_shares: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(prev) = ctx.id.prev() {
        let now = bob_env::current_datetime();
        let prev_report = report::build_report(
            &ctx.vault,
            config::config_path(),
            prev,
            now,
            StatsClock::default(),
        )
        .map_err(BriefError::from)?;
        for row in prev_report
            .get("lanes")
            .and_then(|lanes| lanes.get("root_parent"))
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            prev_shares.insert(str_field(row, "key"), row.clone());
        }
    }
    let roots: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let key = str_field(row, "key");
            let prev = prev_shares.get(&key);
            json!({
                "label": str_field(row, "label"),
                "display": str_field(row, "display"),
                "share": str_field(row, "share"),
                "prev_display": prev.map(|row| str_field(row, "display")).unwrap_or_default(),
                "prev_share": prev.map(|row| str_field(row, "share")).unwrap_or_default(),
            })
        })
        .collect();
    Ok(json!({ "roots": roots }))
}

/// Root shares by quarter, from the year's week columns.
fn year_time(ctx: &Ctx) -> Value {
    let empty = Vec::new();
    let columns = ctx
        .report
        .get("columns")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    // (group, root key) -> minutes.
    let mut minutes: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut group_order: Vec<String> = Vec::new();
    for column in columns {
        let group = str_field(column, "group");
        if group.is_empty() {
            continue;
        }
        if !group_order.contains(&group) {
            group_order.push(group.clone());
        }
        let stack = column.get("stack").and_then(Value::as_array);
        for entry in stack.into_iter().flatten() {
            let key = str_field(entry, "key");
            let value =
                entry.get("minutes").and_then(Value::as_u64).unwrap_or(0);
            *minutes.entry((group.clone(), key)).or_insert(0) += value;
        }
    }
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    for row in lane_rows(ctx, "root_parent") {
        labels.insert(str_field(row, "key"), str_field(row, "label"));
    }
    let quarters: Vec<Value> = group_order
        .into_iter()
        .map(|group| {
            let mut roots: Vec<(String, u64)> = minutes
                .iter()
                .filter(|((quarter, _), _)| quarter == &group)
                .map(|((_, key), value)| (key.clone(), *value))
                .collect();
            roots
                .sort_by_key(|root| std::cmp::Reverse(root.1));
            let total: u64 = roots.iter().map(|(_, value)| value).sum();
            let roots: Vec<Value> = roots
                .into_iter()
                .map(|(key, value)| {
                    let share = if total > 0 {
                        format!(
                            "{}%",
                            (value as f64 / total as f64 * 100.0).round()
                        )
                    } else {
                        String::new()
                    };
                    json!({
                        "label": labels.get(&key).cloned().unwrap_or_else(|| key.clone()),
                        "display": format_minutes(value as i64),
                        "share": share,
                    })
                })
                .collect();
            json!({ "quarter": group, "roots": roots })
        })
        .collect();
    json!({ "quarters": quarters })
}

fn parts_section(ctx: &Ctx) -> Value {
    let empty = Vec::new();
    let children = ctx
        .report
        .get("children")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let parts: Vec<Value> = children
        .iter()
        .map(|child| {
            let review = child.get("review");
            let mut part = json!({
                "id": str_field(child, "id"),
                "kind": str_field(child, "kind"),
                "label": str_field(child, "label"),
                "dates": str_field(child, "range_label"),
                "path": str_field(child, "path"),
                "state": str_field(child, "state"),
                "worked_display": child.get("worked")
                    .and_then(|worked| worked.get("display"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "review_state": review
                    .and_then(|review| review.get("state"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "headline": review
                    .and_then(|review| review.get("headline"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "reflection_lead": review
                    .and_then(|review| review.get("reflection_lead"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            });
            if str_field(child, "kind") == "day" {
                part["closeout"] =
                    child.get("closeout").cloned().unwrap_or(Value::Null);
                part["mostly_display"] =
                    str_field(child, "mostly_display").into();
            }
            part
        })
        .collect();
    Value::Array(parts)
}

fn span_contains(date: Option<NaiveDate>, ctx: &Ctx) -> bool {
    match (date, ctx.observed_end) {
        (Some(date), Some(end)) => date >= ctx.span_start && date <= end,
        _ => false,
    }
}

fn is_open(task: &IndexedTask) -> bool {
    task.done.date.is_none() && task.cancelled.date.is_none()
}

fn is_prj(task: &IndexedTask) -> bool {
    matches!(task.task_kind, TaskKindPart::Prj)
}

fn done_in_span<'a>(
    tasks: &'a [IndexedTask],
    ctx: &Ctx,
) -> Vec<&'a IndexedTask> {
    let mut done: Vec<&IndexedTask> = tasks
        .iter()
        .filter(|task| span_contains(task.done.date, ctx))
        .collect();
    done.sort_by(|left, right| {
        linked_for(right, ctx)
            .cmp(&linked_for(left, ctx))
            .then(left.description.cmp(&right.description))
    });
    done
}

fn task_row(task: &IndexedTask, ctx: &Ctx) -> Value {
    let minutes = linked_for(task, ctx);
    json!({
        "link": task_link(task),
        "minutes": minutes,
        "display": format_minutes(i64::from(minutes)),
    })
}

/// `^prj` completions first, then done tasks ranked by linked minutes.
/// Year briefs group the projects by area.
fn finished_section(ctx: &mut Ctx) -> Value {
    let done = done_in_span(&ctx.index.tasks, ctx);
    let mut prj: Vec<&IndexedTask> =
        done.iter().filter(|task| is_prj(task)).copied().collect();
    let mut rest: Vec<&IndexedTask> =
        done.into_iter().filter(|task| !is_prj(task)).collect();
    prj.sort_by(|left, right| left.description.cmp(&right.description));
    rest.truncate(ctx.finish_cap.saturating_sub(prj.len().min(ctx.finish_cap)));
    let ordered: Vec<&IndexedTask> =
        prj.into_iter().take(ctx.finish_cap).chain(rest).collect();
    let group_by_area = matches!(ctx.id, PeriodId::Year(_));
    let engine = HomesEngine::new(&ctx.index);
    let rows: Vec<Value> = ordered
        .into_iter()
        .map(|task| {
            let mut row = task_row(task, ctx);
            if group_by_area {
                let home = engine.home_for_task(
                    &task.source_path,
                    task.line_number,
                    &task.line,
                );
                let area = if home.area_label.trim().is_empty() {
                    home.label.clone()
                } else {
                    home.area_label.clone()
                };
                row["area"] = area.into();
            }
            row
        })
        .collect();
    Value::Array(rows)
}

/// `^prj` tasks created in the period.
fn started_section(ctx: &Ctx) -> Value {
    let mut started: Vec<&IndexedTask> = ctx
        .index
        .tasks
        .iter()
        .filter(|task| is_prj(task) && span_contains(task.created.date, ctx))
        .collect();
    started.sort_by(|left, right| left.description.cmp(&right.description));
    Value::Array(
        started
            .into_iter()
            .map(|task| task_row(task, ctx))
            .collect(),
    )
}

/// Tasks cancelled in the period, capped like finished rows.
fn dropped_section(ctx: &Ctx) -> Value {
    let mut dropped: Vec<&IndexedTask> = ctx
        .index
        .tasks
        .iter()
        .filter(|task| span_contains(task.cancelled.date, ctx))
        .collect();
    dropped.sort_by(|left, right| left.description.cmp(&right.description));
    dropped.truncate(ctx.finish_cap);
    Value::Array(
        dropped
            .into_iter()
            .map(|task| task_row(task, ctx))
            .collect(),
    )
}

fn still_open_section(ctx: &Ctx) -> Value {
    let snapshot = crate::native::freshness::scan::scan(&ctx.vault).ok();
    let mut zero_minute: Vec<Value> = Vec::new();
    if let Some(snapshot) = snapshot.as_ref() {
        let mut lanes: Vec<&crate::native::freshness::scan::RowCtx> = snapshot
            .next
            .iter()
            .chain(snapshot.pending.iter())
            .collect();
        lanes.sort_by(|left, right| {
            left.task
                .path
                .cmp(&right.task.path)
                .then(left.task.line.cmp(&right.task.line))
        });
        let mut seen: BTreeMap<String, bool> = BTreeMap::new();
        for row in lanes {
            let task = &row.task;
            let block = task.block_id.as_deref().unwrap_or_default();
            let key = task_key(&task.path, block);
            if seen.contains_key(&key) {
                continue;
            }
            seen.insert(key.clone(), true);
            let minutes = ctx.linked.get(&key).copied().unwrap_or(0);
            if minutes == 0 {
                let link = if block.trim().is_empty() {
                    note_link(&task.path, &short_description(&task.text, 120))
                } else {
                    format!(
                        "[[{}#^{block}|{}]]",
                        stem_of(&task.path),
                        short_description(&task.text, 120)
                    )
                };
                zero_minute.push(json!({ "link": link }));
            }
        }
    }
    zero_minute.truncate(ctx.finish_cap);
    // Blocked `^prj` lines and kept tasks are still open wherever they
    // live, so they come from the vault index, not just the lanes.
    let mut blocked_prj: Vec<Value> = Vec::new();
    let mut kept: Vec<Value> = Vec::new();
    let mut open_tasks: Vec<&IndexedTask> = ctx
        .index
        .tasks
        .iter()
        .filter(|task| is_open(task))
        .collect();
    open_tasks.sort_by(|left, right| {
        left.source_path
            .cmp(&right.source_path)
            .then(left.description.cmp(&right.description))
    });
    for task in open_tasks {
        let link = task_link(task);
        if is_prj(task) && is_blocked(task) {
            blocked_prj.push(json!({
                "link": link.clone(),
                "reason": index_block_reason(ctx, task),
            }));
        }
        let keeps = parse_keeps(&task.line);
        if keeps >= 3 {
            kept.push(json!({
                "link": link,
                "keeps": keeps,
                "reason": index_rotten_log_reason(ctx, task),
            }));
        }
    }
    blocked_prj.truncate(ctx.finish_cap);
    kept.truncate(ctx.finish_cap);
    let mut section = json!({
        "zero_minute": zero_minute,
        "blocked_prj": blocked_prj,
        "kept": kept,
    });
    if matches!(ctx.id, PeriodId::Quarter { .. } | PeriodId::Year(_)) {
        let mut stalled: Vec<Value> = ctx
            .index
            .tasks
            .iter()
            .filter(|task| {
                is_prj(task) && is_open(task) && linked_for(task, ctx) == 0
            })
            .map(|task| task_row(task, ctx))
            .collect();
        stalled.sort_by(|left, right| {
            str_field(left, "link").cmp(&str_field(right, "link"))
        });
        stalled.truncate(ctx.finish_cap);
        section["stalled"] = Value::Array(stalled);
    } else {
        section["stalled"] = Value::Null;
    }
    section
}

/// `[keeps:: N]` from a raw task line (0 when absent).
fn parse_keeps(line: &str) -> u32 {
    let marker = "[keeps::";
    let Some(start) = line.find(marker) else {
        return 0;
    };
    line[start + marker.len()..]
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .parse::<u32>()
        .unwrap_or(0)
}

/// A `^prj` task is Blocked when it carries the blocked tag.
fn is_blocked(task: &IndexedTask) -> bool {
    task.tags.iter().any(|tag| {
        tag.trim_start_matches('#')
            .eq_ignore_ascii_case("prj_is_blocked")
    })
}

/// Why a blocked `^prj` is blocked: its Depends-On links, or its
/// future scheduled date.
fn index_block_reason(ctx: &Ctx, task: &IndexedTask) -> String {
    block_reason(
        ctx,
        &task.source_path,
        task.line_number,
        task.scheduled.date,
    )
}

fn block_reason(
    ctx: &Ctx,
    path: &str,
    line_1based: usize,
    scheduled: Option<NaiveDate>,
) -> String {
    if let Some(contents) = ctx.buffers.get(path) {
        let lines: Vec<&str> = contents.lines().collect();
        let at = line_1based.saturating_sub(1);
        if let Some(task_line) = lines.get(at) {
            let indent = leading_spaces(task_line);
            for next in lines.iter().skip(at + 1) {
                if next.trim().is_empty() {
                    continue;
                }
                if leading_spaces(next) <= indent && !next.trim().is_empty() {
                    break;
                }
                if next.to_ascii_lowercase().contains("depends-on") {
                    let links = wikilinks(next);
                    if !links.is_empty() {
                        return format!("waits on {}", links.join(", "));
                    }
                }
            }
        }
    }
    if let Some(scheduled) = scheduled {
        return format!("scheduled {}", scheduled.format("%Y-%m-%d"));
    }
    "blocked".to_string()
}

fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// Every `[[target]]` span in a line, as written.
fn wikilinks(line: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("]]") {
            links.push(format!("[[{}]]", &after[..end]));
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    links
}

/// The latest Rotten Log entry under a task, `""` when it kept none.
fn index_rotten_log_reason(ctx: &Ctx, task: &IndexedTask) -> String {
    rotten_log_reason(ctx, &task.source_path, task.line_number)
}

fn rotten_log_reason(ctx: &Ctx, path: &str, line_1based: usize) -> String {
    let Some(contents) = ctx.buffers.get(path) else {
        return String::new();
    };
    let lines: Vec<&str> = contents.lines().collect();
    let at = line_1based.saturating_sub(1);
    let Some(task_line) = lines.get(at) else {
        return String::new();
    };
    let indent = leading_spaces(task_line);
    let mut in_rotten = false;
    let mut latest = String::new();
    for next in lines.iter().skip(at + 1) {
        if next.trim().is_empty() {
            continue;
        }
        let trimmed = next.trim_start();
        if leading_spaces(next) <= indent
            && (trimmed.starts_with("- ") || trimmed.starts_with("* "))
            && !in_rotten
        {
            break;
        }
        if next.contains("ROTTEN LOG") {
            in_rotten = true;
            continue;
        }
        if in_rotten {
            if let Some(entry) = dated_entry_text(next) {
                latest = entry;
            } else if leading_spaces(next) <= indent {
                break;
            }
        }
    }
    latest
}

/// Text of a `*YYYY-MM-DD …* — reason` drawer entry, without the date.
fn dated_entry_text(line: &str) -> Option<String> {
    let star = line.find('*')?;
    let after = &line[star + 1..];
    let end = after.find('*')?;
    let stamp = after[..end].trim();
    if stamp.len() < 10
        || !stamp[..4].chars().all(|ch| ch.is_ascii_digit())
        || stamp[..4].parse::<i32>().is_err()
    {
        return None;
    }
    let rest = after[end + 1..].trim();
    let rest = rest
        .strip_prefix(['—', '-', ':'])
        .map(str::trim)
        .unwrap_or(rest);
    // Drop a trailing `· kept N×` tally: the keeps count rides beside.
    let reason = rest.split('·').next().unwrap_or(rest).trim().to_string();
    if reason.is_empty() {
        None
    } else {
        Some(reason)
    }
}

/// Year only: recurring `#gtd` rows created, cancelled, or
/// finished in the year, plus the review-closed rate per quarter.
fn practice_section(ctx: &Ctx) -> Value {
    if !matches!(ctx.id, PeriodId::Year(_)) {
        return Value::Null;
    }
    let mut created: Vec<Value> = Vec::new();
    let mut cancelled: Vec<Value> = Vec::new();
    let mut finished: Vec<Value> = Vec::new();
    for task in &ctx.index.tasks {
        if !task.recurring {
            continue;
        }
        let is_gtd = task
            .tags
            .iter()
            .any(|tag| tag.trim_start_matches('#').eq_ignore_ascii_case("gtd"));
        if !is_gtd {
            continue;
        }
        let row = task_row(task, ctx);
        if span_contains(task.created.date, ctx) {
            created.push(row.clone());
        }
        if span_contains(task.cancelled.date, ctx) {
            cancelled.push(row.clone());
        }
        if span_contains(task.done.date, ctx) {
            finished.push(row);
        }
    }
    for rows in [&mut created, &mut cancelled, &mut finished] {
        rows.sort_by(|left, right| {
            str_field(left, "link").cmp(&str_field(right, "link"))
        });
        rows.truncate(ctx.finish_cap);
    }
    // Review-closed rate per quarter, from the year's day columns:
    // met days over days that carry worked data.
    let empty = Vec::new();
    let columns = ctx
        .report
        .get("columns")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let mut met: BTreeMap<String, u32> = BTreeMap::new();
    let mut with_data: BTreeMap<String, u32> = BTreeMap::new();
    for column in columns {
        let group = str_field(column, "group");
        if group.is_empty() || str_field(column, "state") == "future" {
            continue;
        }
        let has_data = column
            .get("worked")
            .and_then(|worked| worked.get("value"))
            .is_some_and(|value| !value.is_null());
        if !has_data {
            continue;
        }
        *with_data.entry(group.clone()).or_insert(0) += 1;
        if column.get("mark").and_then(Value::as_str) == Some("met") {
            *met.entry(group).or_insert(0) += 1;
        }
    }
    let mut quarters: Vec<String> = with_data.keys().cloned().collect();
    quarters.sort();
    let rates: Vec<Value> = quarters
        .into_iter()
        .map(|quarter| {
            let days_met = met.get(&quarter).copied().unwrap_or(0);
            let days = with_data.get(&quarter).copied().unwrap_or(0);
            json!({
                "quarter": quarter,
                "display": format!("{days_met} of {days} days"),
            })
        })
        .collect();
    json!({
        "recurring_created": created,
        "recurring_cancelled": cancelled,
        "recurring_finished": finished,
        "review_closed_by_quarter": rates,
    })
}

fn bryans_words_section(ctx: &Ctx) -> Value {
    let mut words = work_log_entries(ctx);
    words.extend(pomodoro_notes(ctx));
    words.sort_by(|left, right| {
        str_field(left, "date")
            .cmp(&str_field(right, "date"))
            .then(str_field(left, "link").cmp(&str_field(right, "link")))
    });
    words.truncate(40);
    Value::Array(words)
}

/// Work Log entries dated in the period, across every vault buffer.
fn work_log_entries(ctx: &Ctx) -> Vec<Value> {
    let Some(end) = ctx.observed_end else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    let mut paths: Vec<&String> = ctx.buffers.keys().collect();
    paths.sort();
    for path in paths {
        let contents = &ctx.buffers[path];
        let mut in_work_log = false;
        let mut depth = usize::MAX;
        for line in contents.lines() {
            if line.contains("WORK LOG") && line.contains("**") {
                in_work_log = true;
                depth = leading_spaces(line);
                continue;
            }
            if !in_work_log {
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            if leading_spaces(line) <= depth {
                in_work_log = line.contains("WORK LOG");
                continue;
            }
            if let Some((date, text)) = stamped_entry(line)
                && date >= ctx.span_start
                && date <= end
            {
                entries.push(json!({
                    "date": date.format("%Y-%m-%d").to_string(),
                    "link": note_link(path, &short_description(&text, 80)),
                    "text": short_description(&text, 280),
                }));
            }
        }
    }
    entries
}

/// A stamped `*YYYY-MM-DD[ HH:MM]* — text` entry line.
fn stamped_entry(line: &str) -> Option<(NaiveDate, String)> {
    let text = dated_entry_text(line)?;
    let star = line.find('*')?;
    let after = &line[star + 1..];
    let end = after.find('*')?;
    let stamp = after[..end].trim();
    let day = stamp.get(..10)?;
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some((date, text))
}

/// Free-text Pomodoro child bullets (not Task Links), chronological.
fn pomodoro_notes(ctx: &Ctx) -> Vec<Value> {
    let Some(end) = ctx.observed_end else {
        return Vec::new();
    };
    let mut notes = Vec::new();
    let mut dates: Vec<NaiveDate> =
        ctx.index.daily_notes.keys().copied().collect();
    dates.sort();
    for date in dates {
        if date < ctx.span_start || date > end {
            continue;
        }
        let Some(daily) = ctx.index.daily_notes.get(&date) else {
            continue;
        };
        let Some(contents) = ctx.buffers.get(&daily.path) else {
            continue;
        };
        let lines: Vec<&str> = contents.lines().collect();
        let mut link_lines: BTreeMap<usize, bool> = BTreeMap::new();
        for session in &daily.closed_pomodoros {
            for link in &session.links {
                link_lines.insert(link.line, true);
            }
        }
        for session in &daily.closed_pomodoros {
            let at = session.line.saturating_sub(1) as usize;
            let Some(session_line) = lines.get(at) else {
                continue;
            };
            let indent = leading_spaces(session_line);
            for (offset, next) in lines.iter().skip(at + 1).enumerate() {
                let number = session.line as usize + offset + 1;
                if next.trim().is_empty() {
                    continue;
                }
                if leading_spaces(next) <= indent {
                    break;
                }
                let trimmed = next.trim_start();
                let is_item = trimmed.starts_with("- ")
                    || trimmed.starts_with("* ")
                    || trimmed.starts_with("- [")
                    || trimmed.starts_with("* [");
                if !is_item || link_lines.contains_key(&number) {
                    continue;
                }
                let text = trimmed
                    .trim_start_matches(['-', '*'])
                    .trim()
                    .trim_start_matches(|ch: char| {
                        ch == '[' || ch == ']' || ch == 'x' || ch == ' '
                    })
                    .trim()
                    .to_string();
                if text.is_empty() || text.contains("#^") {
                    continue;
                }
                notes.push(json!({
                    "date": date.format("%Y-%m-%d").to_string(),
                    "link": note_link(&daily.path, &short_description(&text, 80)),
                    "text": short_description(&text, 280),
                }));
            }
        }
    }
    notes
}

/// This note's human sections, the previous note's Reflection, the
/// parent's Outcomes and Theme, and (year only) the goal and vision
/// notes. Labelled data, not instructions.
fn neighbours_section(ctx: &Ctx) -> Value {
    let note_path = ctx
        .report
        .get("note")
        .and_then(|note| note.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let own = ctx
        .buffers
        .get(&note_path)
        .map(|contents| own_sections(ctx.id, contents))
        .unwrap_or_default();
    let prev_path = ctx
        .report
        .get("links")
        .and_then(|links| links.get("prev"))
        .and_then(|prev| prev.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let prev_reflection = ctx
        .buffers
        .get(&prev_path)
        .map(|contents| {
            truncate_words(
                &note::parse(contents)
                    .sections
                    .get("Reflection")
                    .cloned()
                    .unwrap_or_default(),
                300,
            )
        })
        .unwrap_or_default();
    let parent_path = ctx
        .report
        .get("links")
        .and_then(|links| links.get("parent"))
        .and_then(|parent| parent.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parent_sections = ctx
        .buffers
        .get(&parent_path)
        .map(|contents| note::parse(contents).sections)
        .unwrap_or_default();
    let mut section = json!({
        "own": own,
        "prev_reflection": prev_reflection,
        "parent_outcomes": parent_sections.get("Outcomes").cloned().unwrap_or_default(),
        "parent_theme": parent_sections.get("Theme").cloned().unwrap_or_default(),
    });
    if matches!(ctx.id, PeriodId::Year(_)) {
        section["h3_goal"] = ctx
            .buffers
            .get("h3_goal.md")
            .map(|contents| truncate_words(contents, 400))
            .unwrap_or_default()
            .into();
        section["h4_vision"] = ctx
            .buffers
            .get("h4_vision.md")
            .map(|contents| truncate_words(contents, 400))
            .unwrap_or_default()
            .into();
    }
    section
}

/// This note's human sections verbatim, by kind.
fn own_sections(id: PeriodId, contents: &str) -> Value {
    let sections = note::parse(contents).sections;
    let pick = |names: &[&str]| -> Value {
        let mut picked = serde_json::Map::new();
        for name in names {
            if let Some(body) = sections.get(*name) {
                picked.insert((*name).to_string(), Value::String(body.clone()));
            }
        }
        Value::Object(picked)
    };
    match id {
        PeriodId::Week { .. } => pick(&["Intentions"]),
        PeriodId::Quarter { .. } => pick(&["Outcomes"]),
        PeriodId::Year(_) => pick(&["Theme", "Outcomes"]),
        PeriodId::AllTime => Value::Object(serde_json::Map::new()),
    }
}

fn truncate_words(text: &str, cap: usize) -> String {
    super::draft::truncate_words(text, cap)
}

/// Off-clock minutes and session counts by area (counts only: event
/// titles never reach the agent).
fn offclock_section(ctx: &Ctx) -> Value {
    let Some(end) = ctx.observed_end else {
        return json!({ "minutes": 0, "display": format_minutes(0), "sessions": 0, "by_area": [] });
    };
    let engine = HomesEngine::new(&ctx.index);
    let mut total: u32 = 0;
    let mut sessions: u32 = 0;
    let mut by_area: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for (date, daily) in &ctx.index.daily_notes {
        if *date < ctx.span_start || *date > end {
            continue;
        }
        for session in &daily.closed_pomodoros {
            if session.stats_clock() != Some(StatsClock::OffClock) {
                continue;
            }
            let Some(minutes) = session.counted_minutes() else {
                continue;
            };
            total += minutes;
            sessions += 1;
            let attributed = engine.attribute(
                session,
                Path::new(&daily.path),
                Some(minutes),
            );
            let mut split = false;
            for part in &attributed.homes.parts {
                if !matches!(
                    part.kind,
                    crate::native::pomodoro_homes::HomeKind::Area
                ) {
                    continue;
                }
                if let Some(share) = part.minutes {
                    let entry =
                        by_area.entry(part.label.clone()).or_insert((0, 0));
                    entry.0 += share;
                    entry.1 += 1;
                    split = true;
                }
            }
            if !split {
                let entry = by_area
                    .entry(attributed.homes.label.clone())
                    .or_insert((0, 0));
                entry.0 += minutes;
                entry.1 += 1;
            }
        }
    }
    let mut areas: Vec<Value> = by_area
        .into_iter()
        .map(|(label, (minutes, count))| {
            json!({
                "label": label,
                "minutes": minutes,
                "display": format_minutes(i64::from(minutes)),
                "sessions": count,
            })
        })
        .collect();
    areas.sort_by(|left, right| {
        right
            .get("minutes")
            .and_then(Value::as_u64)
            .cmp(&left.get("minutes").and_then(Value::as_u64))
    });
    json!({
        "minutes": total,
        "display": format_minutes(i64::from(total)),
        "sessions": sessions,
        "by_area": areas,
    })
}

fn caveats_section(ctx: &Ctx) -> Value {
    let mut caveats: Vec<String> = Vec::new();
    // Unlinked on-clock minutes: sessions with counted minutes but no
    // task link.
    let Some(end) = ctx.observed_end else {
        return Value::Array(Vec::new());
    };
    let mut unlinked: u32 = 0;
    for (date, daily) in &ctx.index.daily_notes {
        if *date < ctx.span_start || *date > end {
            continue;
        }
        for session in &daily.closed_pomodoros {
            if session.stats_clock() != Some(StatsClock::Worked) {
                continue;
            }
            if session
                .links
                .iter()
                .any(|link| !link.block_id.trim().is_empty())
            {
                continue;
            }
            unlinked += session.counted_minutes().unwrap_or(0);
        }
    }
    if unlinked > 0 {
        caveats.push(format!(
            "{} of Worked minutes are not linked to any task.",
            format_minutes(i64::from(unlinked))
        ));
    }
    // Not-tracked and missing children, from the report columns.
    let empty = Vec::new();
    let columns = ctx
        .report
        .get("columns")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let mut not_tracked: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut journal_gaps: Vec<String> = Vec::new();
    for column in columns {
        if str_field(column, "state") == "future" {
            continue;
        }
        let quality = column
            .get("worked")
            .and_then(|worked| worked.get("quality"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if quality == "not_tracked" {
            not_tracked.push(str_field(column, "label"));
            continue;
        }
        if column.get("exists").and_then(Value::as_bool) == Some(false) {
            missing.push(str_field(column, "label"));
        }
        // A tracked column with worked minutes but no closeout mark
        // leaves a gap in the review journal.
        let has_worked = column
            .get("worked")
            .and_then(|worked| worked.get("value"))
            .is_some_and(|value| {
                value.as_i64().is_some_and(|minutes| minutes > 0)
            });
        if has_worked && column.get("mark").is_none_or(Value::is_null) {
            journal_gaps.push(str_field(column, "label"));
        }
    }
    if !not_tracked.is_empty() {
        caveats.push(format!(
            "Not tracked: {}.",
            summarize_list(&not_tracked, 10)
        ));
    }
    if !missing.is_empty() {
        let noun = if matches!(ctx.id, PeriodId::Week { .. }) {
            "daily notes"
        } else {
            "notes"
        };
        caveats
            .push(format!("Missing {noun}: {}.", summarize_list(&missing, 10)));
    }
    if !journal_gaps.is_empty() {
        caveats.push(format!(
            "Review journal gaps: {}.",
            summarize_list(&journal_gaps, 10)
        ));
    }
    // Unreviewed children.
    let children = ctx
        .report
        .get("children")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let mut unreviewed: Vec<String> = Vec::new();
    for child in children {
        if str_field(child, "state") == "future" {
            continue;
        }
        let review_state = child
            .get("review")
            .and_then(|review| review.get("state"))
            .and_then(Value::as_str)
            .unwrap_or("empty");
        if review_state == "empty" || review_state == "edited" {
            let label = str_field(child, "label");
            if !label.is_empty() {
                unreviewed.push(label);
            }
        }
    }
    if !unreviewed.is_empty() {
        let noun = if matches!(ctx.id, PeriodId::Week { .. }) {
            "days"
        } else {
            "children"
        };
        caveats.push(format!(
            "Not reviewed {noun}: {}.",
            summarize_list(&unreviewed, 10)
        ));
    }
    Value::Array(caveats.into_iter().map(Value::String).collect())
}

fn summarize_list(items: &[String], cap: usize) -> String {
    if items.len() <= cap {
        return items.join(", ");
    }
    format!("{} +{} more", items[..cap].join(", "), items.len() - cap)
}

fn render_markdown(_ctx: &Ctx, json: &Value) -> String {
    let header = json.get("header").unwrap_or(&Value::Null);
    let mut out = String::new();
    out.push_str(&format!(
        "# {BRIEF_VERSION} · {} · {} ({} · {})\n",
        str_field(header, "id"),
        str_field(header, "title"),
        str_field(header, "state"),
        str_field(header, "review_state"),
    ));
    out.push_str(&format!(
        "Note: {} · Previous: {} · Parent: {}\n",
        str_field(header, "note_path"),
        str_field(header, "prev_path"),
        str_field(header, "parent_path"),
    ));
    out.push_str(&format!("{}\n", str_field(header, "stop_advice")));
    // Facts.
    out.push_str("\n## Facts\n");
    if let Some(facts) = json.get("facts") {
        if let Some(coverage) = facts.get("coverage").and_then(Value::as_str)
            && !coverage.is_empty()
        {
            out.push_str(&format!("Coverage: {coverage}.\n"));
        }
        if let Some(rows) = facts.get("facts").and_then(Value::as_array) {
            for fact in rows {
                let so_far = fact
                    .get("so_far")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let typical = fact
                    .get("typical_words")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                out.push_str(&format!(
                    "- {}: {}{}",
                    str_field(fact, "label"),
                    str_field(fact, "display"),
                    if so_far { " so far" } else { "" },
                ));
                if !typical.is_empty() {
                    out.push_str(&format!(" ({typical})"));
                }
                out.push('\n');
            }
        }
    }
    // Time.
    out.push_str("\n## Time\n");
    if let Some(time) = json.get("time") {
        if let Some(roots) = time.get("roots").and_then(Value::as_array) {
            for root in roots {
                out.push_str(&format!(
                    "- {} · {} ({})\n",
                    str_field(root, "label"),
                    str_field(root, "display"),
                    str_field(root, "share"),
                ));
                if let Some(prev_share) =
                    root.get("prev_share").and_then(Value::as_str)
                    && !prev_share.is_empty()
                {
                    out.push_str(&format!(
                        "  - previous: {} ({prev_share})\n",
                        root.get("prev_display")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ));
                }
                if let Some(tasks) = root.get("tasks").and_then(Value::as_array)
                {
                    for task in tasks {
                        out.push_str(&format!(
                            "  - {} · {}\n",
                            str_field(task, "link"),
                            str_field(task, "display"),
                        ));
                    }
                }
            }
        }
        if let Some(quarters) = time.get("quarters").and_then(Value::as_array) {
            for quarter in quarters {
                out.push_str(
                    &format!("- {}\n", str_field(quarter, "quarter"),),
                );
                if let Some(roots) =
                    quarter.get("roots").and_then(Value::as_array)
                {
                    for root in roots {
                        out.push_str(&format!(
                            "  - {} · {} ({})\n",
                            str_field(root, "label"),
                            str_field(root, "display"),
                            str_field(root, "share"),
                        ));
                    }
                }
            }
        }
        if let Some(types) = time.get("types").and_then(Value::as_array) {
            for row in types {
                out.push_str(&format!(
                    "- {} (type) · {} ({})\n",
                    str_field(row, "label"),
                    str_field(row, "display"),
                    str_field(row, "share"),
                ));
            }
        }
    }
    // Parts.
    out.push_str("\n## Parts\n");
    if let Some(parts) = json.get("parts").and_then(Value::as_array) {
        for part in parts {
            render_part(&mut out, part);
        }
    }
    // Finished / started / dropped.
    out.push_str("\n## Finished\n");
    render_finished(&mut out, json.get("finished").and_then(Value::as_array));
    out.push_str("\n## Started and dropped\n");
    out.push_str("Started:\n");
    render_task_rows(
        &mut out,
        json.get("started").and_then(Value::as_array),
        "Nothing started.",
    );
    out.push_str("Dropped:\n");
    render_task_rows(
        &mut out,
        json.get("dropped").and_then(Value::as_array),
        "Nothing dropped.",
    );
    // Still open.
    out.push_str("\n## Still open, as of today\n");
    if let Some(still) = json.get("still_open") {
        out.push_str("No linked minutes:\n");
        render_links(
            &mut out,
            still.get("zero_minute").and_then(Value::as_array),
            "None.",
        );
        out.push_str("Blocked projects:\n");
        if let Some(rows) = still.get("blocked_prj").and_then(Value::as_array) {
            if rows.is_empty() {
                out.push_str("None.\n");
            }
            for row in rows {
                out.push_str(&format!(
                    "- {} · {}\n",
                    str_field(row, "link"),
                    str_field(row, "reason"),
                ));
            }
        }
        out.push_str("Kept 3+ times:\n");
        if let Some(rows) = still.get("kept").and_then(Value::as_array) {
            if rows.is_empty() {
                out.push_str("None.\n");
            }
            for row in rows {
                let reason = str_field(row, "reason");
                out.push_str(&format!(
                    "- {} · kept {}×{}\n",
                    str_field(row, "link"),
                    row.get("keeps").and_then(Value::as_u64).unwrap_or(0),
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!(" · {reason}")
                    },
                ));
            }
        }
        if let Some(stalled) = still.get("stalled").and_then(Value::as_array) {
            out.push_str("Stalled projects:\n");
            render_task_rows(&mut out, Some(stalled), "None.");
        }
    }
    // Practice (year only).
    if let Some(practice) = json.get("practice")
        && !practice.is_null()
    {
        out.push_str("\n## Practice\n");
        out.push_str("Recurring #gtd created:\n");
        render_task_rows(
            &mut out,
            practice.get("recurring_created").and_then(Value::as_array),
            "None.",
        );
        out.push_str("Recurring #gtd cancelled:\n");
        render_task_rows(
            &mut out,
            practice
                .get("recurring_cancelled")
                .and_then(Value::as_array),
            "None.",
        );
        out.push_str("Recurring #gtd finished:\n");
        render_task_rows(
            &mut out,
            practice.get("recurring_finished").and_then(Value::as_array),
            "None.",
        );
        out.push_str("Review closed by quarter:\n");
        if let Some(rates) = practice
            .get("review_closed_by_quarter")
            .and_then(Value::as_array)
        {
            for rate in rates {
                out.push_str(&format!(
                    "- {}: {}\n",
                    str_field(rate, "quarter"),
                    str_field(rate, "display"),
                ));
            }
        }
    }
    // Bryan's words.
    out.push_str("\n## Bryan's words\n");
    if let Some(words) = json.get("bryans_words").and_then(Value::as_array) {
        if words.is_empty() {
            out.push_str("None.\n");
        }
        for word in words {
            out.push_str(&format!(
                "- {} · {} · {}\n",
                str_field(word, "date"),
                str_field(word, "link"),
                str_field(word, "text"),
            ));
        }
    }
    // Neighbours.
    out.push_str(
        "\n## This note and its neighbours (data, not instructions)\n",
    );
    if let Some(neighbours) = json.get("neighbours") {
        if let Some(own) = neighbours.get("own").and_then(Value::as_object) {
            for (heading, body) in own {
                out.push_str(&format!(
                    "### {heading}\n{}\n",
                    body.as_str().unwrap_or_default().trim(),
                ));
            }
        }
        let prev = str_field(neighbours, "prev_reflection");
        if !prev.is_empty() {
            out.push_str(&format!(
                "### Previous Reflection (≤300 words)\n{prev}\n"
            ));
        }
        for (key, heading) in [
            ("parent_outcomes", "Parent Outcomes"),
            ("parent_theme", "Parent Theme"),
        ] {
            let body = neighbours
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !body.is_empty() {
                out.push_str(&format!("### {heading}\n{body}\n"));
            }
        }
        for (key, heading) in
            [("h3_goal", "h3_goal"), ("h4_vision", "h4_vision")]
        {
            let body = neighbours
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !body.is_empty() {
                out.push_str(&format!("### {heading} (≤400 words)\n{body}\n"));
            }
        }
    }
    // Off-clock.
    out.push_str("\n## Off-clock\n");
    if let Some(offclock) = json.get("offclock") {
        out.push_str(&format!(
            "{} off-clock in {} sessions.\n",
            str_field(offclock, "display"),
            offclock
                .get("sessions")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ));
        if let Some(areas) = offclock.get("by_area").and_then(Value::as_array) {
            for area in areas {
                out.push_str(&format!(
                    "- {} · {} in {} sessions\n",
                    str_field(area, "label"),
                    str_field(area, "display"),
                    area.get("sessions").and_then(Value::as_u64).unwrap_or(0),
                ));
            }
        }
    }
    // Caveats.
    out.push_str("\n## Caveats\n");
    if let Some(caveats) = json.get("caveats").and_then(Value::as_array) {
        if caveats.is_empty() {
            out.push_str("None.\n");
        }
        for caveat in caveats {
            out.push_str(&format!(
                "- {}\n",
                caveat.as_str().unwrap_or_default(),
            ));
        }
    }
    out
}

fn render_part(out: &mut String, part: &Value) {
    let state = str_field(part, "state");
    if state == "future" {
        out.push_str(&format!(
            "- {} · {} (not started)\n",
            str_field(part, "label"),
            str_field(part, "dates"),
        ));
        return;
    }
    let review_state = str_field(part, "review_state");
    let review_bits = if review_state.is_empty() || review_state == "empty" {
        "not reviewed".to_string()
    } else {
        let headline = str_field(part, "headline");
        if headline.is_empty() {
            review_state
        } else {
            format!("{review_state} · {headline}")
        }
    };
    let mut line = format!(
        "- {} · {} · {} · {review_bits}",
        str_field(part, "label"),
        str_field(part, "dates"),
        str_field(part, "worked_display"),
    );
    let lead = str_field(part, "reflection_lead");
    if !lead.is_empty() {
        line.push_str(&format!(" · {lead}"));
    }
    if str_field(part, "kind") == "day" {
        let mostly = str_field(part, "mostly_display");
        if !mostly.is_empty() {
            line.push_str(&format!(" · mostly {mostly}"));
        }
        let closeout = part
            .get("closeout")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !closeout.is_empty() {
            line.push_str(&format!(" · {closeout}"));
        }
    }
    line.push('\n');
    out.push_str(&line);
}

/// Finished rows, grouped by area when the rows carry one (year).
fn render_finished(out: &mut String, rows: Option<&Vec<Value>>) {
    let Some(rows) = rows else {
        out.push_str("Nothing finished.\n");
        return;
    };
    if rows.is_empty() {
        out.push_str("Nothing finished.\n");
        return;
    }
    if rows.iter().all(|row| row.get("area").is_none()) {
        render_task_rows(out, Some(rows), "Nothing finished.");
        return;
    }
    let mut areas: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            row.get("area").and_then(Value::as_str).map(str::to_string)
        })
        .collect();
    areas.sort();
    areas.dedup();
    for area in areas {
        out.push_str(&format!("{area}:\n"));
        let grouped: Vec<Value> = rows
            .iter()
            .filter(|row| {
                row.get("area").and_then(Value::as_str) == Some(area.as_str())
            })
            .cloned()
            .collect();
        render_task_rows(out, Some(&grouped), "Nothing finished.");
    }
}

fn render_task_rows(out: &mut String, rows: Option<&Vec<Value>>, empty: &str) {
    match rows {
        Some(rows) if !rows.is_empty() => {
            for row in rows {
                let display = str_field(row, "display");
                if display.is_empty() || display == "0m" {
                    out.push_str(&format!("- {}\n", str_field(row, "link")));
                } else {
                    out.push_str(&format!(
                        "- {} · {display}\n",
                        str_field(row, "link"),
                    ));
                }
            }
        }
        _ => out.push_str(&format!("{empty}\n")),
    }
}

fn render_links(out: &mut String, rows: Option<&Vec<Value>>, empty: &str) {
    match rows {
        Some(rows) if !rows.is_empty() => {
            for row in rows {
                out.push_str(&format!("- {}\n", str_field(row, "link")));
            }
        }
        _ => out.push_str(&format!("{empty}\n")),
    }
}

/// Token budgets: a week ≤ 6k, a quarter ≤ 10k, a year ≤ 12k tokens.
/// Roughly four characters per token; trim Bryan's words, then
/// caveats, then neighbour bodies until the bundle fits.
fn enforce_budget(ctx: &Ctx, json: &mut Value, markdown: &mut String) {
    let budget = match ctx.id {
        PeriodId::Week { .. } => WEEK_BUDGET,
        PeriodId::Quarter { .. } => QUARTER_BUDGET,
        PeriodId::Year(_) => YEAR_BUDGET,
        PeriodId::AllTime => WEEK_BUDGET,
    };
    let tokens = |text: &str| text.len() / 4;
    while tokens(markdown) > budget {
        let words = json.get_mut("bryans_words").and_then(Value::as_array_mut);
        if let Some(words) = words
            && !words.is_empty()
        {
            words.pop();
            *markdown = render_markdown(ctx, json);
            continue;
        }
        let caveats = json.get_mut("caveats").and_then(Value::as_array_mut);
        if let Some(caveats) = caveats
            && !caveats.is_empty()
        {
            caveats.pop();
            *markdown = render_markdown(ctx, json);
            continue;
        }
        break;
    }
}
