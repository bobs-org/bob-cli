//! `bob periods brief`: the deterministic, versioned agent bundle.
//!
//! The contract lives in `docs/periods.md`. The brief (brief v2) is the
//! only source of numbers an agent may use when drafting a period
//! Review: header, facts, time, parts, finished (projects, then every
//! non-routine completion grouped by root parent and note), started and
//! dropped, still open, practice (year only), Bryan's words, this note
//! and its neighbours (including the previous Review's Headline,
//! Carried forward, and Questions), off-clock counts, and caveats.
//! Markdown and JSON carry the same content. `all_time` has no review.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    pomodoro_homes::split_minutes,
    stats::{
        clock::StatsClock,
        homes::HomesEngine,
        index::{
            canonical_daily_date, DateDiagnostic, IndexedTask, StatsIndex,
            TaskKindPart,
        },
        series::format_minutes,
    },
};

const COMMAND_NAME: &str = "bob periods brief";
/// Success envelopes carry the brief v2 shape; error envelopes keep the
/// shared period-command error shape (`schema_version: 1`).
const SCHEMA_VERSION: u32 = 2;
const ERROR_SCHEMA_VERSION: u32 = 1;
const BRIEF_VERSION: &str = "brief v2";
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
            hint: "Pass a period id such as 2026-W45, 2026-Q4, 2026, last-week, last-quarter, or last-year. See docs/periods.md.".to_string(),
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
            "schema_version": ERROR_SCHEMA_VERSION,
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
    // `md` (alias `human`) prints the Markdown bundle; `json` the JSON one.
    let human = matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("md")
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
    /// On-clock linked minutes per task key (`source_path || block_id`),
    /// split across a session's links so tasks never outrank their root.
    linked: HashMap<String, u32>,
    /// Minutes per root key across on-clock sessions in the observed span.
    _roots: BTreeMap<String, u32>,
    state: PeriodState,
    observed_end: Option<NaiveDate>,
    span_start: NaiveDate,
    finish_cap: usize,
}

fn execute(matches: &clap::ArgMatches) -> Result<Bundle, BriefError> {
    let vault = bob_env::bob_dir();
    let now = bob_env::current_datetime();
    let today = now.date();
    let id = matches
        .get_one::<String>("period")
        .map(|text| {
            PeriodId::resolve(text, today)
                .map_err(|message| BriefError::usage("invalid_period", message))
        })
        .transpose()?
        .unwrap_or_else(|| PeriodId::week_for_date(today));
    if matches!(id, PeriodId::AllTime) {
        return Err(BriefError::usage(
            "no_review_section",
            "all_time has no review".to_string(),
        ));
    }
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
    fit_finished(&ctx, &mut json);
    let mut markdown = render_markdown(&ctx, &json);
    enforce_budget(&ctx, &mut json, &mut markdown);
    Ok(Bundle { markdown, json })
}

/// Canonical key for matching a session task link to its task row.
/// Archived tasks live in `done/` but keep their original `source_path`
/// as the identity: either spelling resolves to this one key.
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

/// Where a task row lives on disk: the `done/` archive when the task
/// moved, otherwise its original note.
fn task_effective_path(task: &IndexedTask) -> &str {
    task.archive_path.as_deref().unwrap_or(&task.source_path)
}

/// Whether a normalized link target names this task through either its
/// original note or its `done/` archive.
fn task_matches_target(task: &IndexedTask, target: &str) -> bool {
    if normalize_target(&task.source_path) == target {
        return true;
    }
    if let Some(archive) = task.archive_path.as_deref()
        && normalize_target(archive) == target
    {
        return true;
    }
    false
}

/// On-clock minutes in the observed span, split across a session's links
/// with the same splitter the stats `root_parent` lanes use, so task rows
/// under a root never outrank that root. Either the original or the
/// `done/` archive spelling resolves to one task identity, and each
/// session–task match counts once even when two spellings name it.
fn linked_minutes(ctx: &Ctx) -> HashMap<String, u32> {
    let Some(end) = ctx.observed_end else {
        return HashMap::new();
    };
    // Task lookup by trimmed lower-cased block id: sessions link tasks by
    // `(target, block_id)`, and tasks are unique per note.
    let mut by_block: HashMap<String, Vec<&IndexedTask>> = HashMap::new();
    for task in &ctx.index.tasks {
        if let Some(block) = task.block_id.as_deref()
            && !block.trim().is_empty()
        {
            by_block
                .entry(block.trim().to_ascii_lowercase())
                .or_default()
                .push(task);
        }
    }
    let mut linked = HashMap::new();
    for (date, daily) in &ctx.index.daily_notes {
        if *date < ctx.span_start || *date > end {
            continue;
        }
        let daily_path = Path::new(&daily.path);
        for session in &daily.closed_pomodoros {
            if session.stats_clock() != Some(StatsClock::Worked) {
                continue;
            }
            let Some(minutes) = session.counted_minutes() else {
                continue;
            };
            // Deduplicate the session's links by resolved note plus block,
            // mirroring the homes attribution: a bare `[[#^block]]` link
            // targets the daily note itself.
            let mut seen_raw: HashSet<(String, String)> = HashSet::new();
            let mut deduped: Vec<(String, String)> = Vec::new();
            for link in &session.links {
                if link.block_id.trim().is_empty() {
                    continue;
                }
                let block_lower = link.block_id.trim().to_ascii_lowercase();
                let resolved = ctx
                    .index
                    .note_index
                    .resolve(Some(daily_path), &link.target)
                    .map(|path| normalize_target(&path.to_string_lossy()))
                    .unwrap_or_else(|| {
                        if link.target.trim().is_empty() {
                            normalize_target(&daily.path)
                        } else {
                            normalize_target(&link.target)
                        }
                    });
                if seen_raw.insert((resolved.clone(), block_lower.clone())) {
                    deduped.push((resolved, block_lower));
                }
            }
            if deduped.is_empty() {
                continue;
            }
            // Map each deduped link to its task identity, collapsing two
            // spellings of the same archived task to one entry.
            let mut seen_tasks: HashSet<String> = HashSet::new();
            let mut final_entries: Vec<Option<String>> = Vec::new();
            for (target, block_lower) in deduped {
                let mut matched: Option<String> = None;
                if let Some(candidates) = by_block.get(&block_lower) {
                    for task in candidates {
                        if task_matches_target(task, &target)
                            && let Some(block) = task.block_id.as_deref()
                        {
                            matched = Some(task_key(&task.source_path, block));
                            break;
                        }
                    }
                }
                if let Some(canonical) = matched {
                    if seen_tasks.insert(canonical.clone()) {
                        final_entries.push(Some(canonical));
                    }
                } else {
                    final_entries.push(None);
                }
            }
            let total = final_entries.len();
            if total == 0 {
                continue;
            }
            let shares = split_minutes(minutes, total);
            for (index, entry) in final_entries.into_iter().enumerate() {
                if let Some(canonical) = entry {
                    let share = shares.get(index).copied().unwrap_or(0);
                    if share == 0 {
                        continue;
                    }
                    *linked.entry(canonical).or_insert(0) += share;
                }
            }
        }
    }
    // Alias the archive spelling to the same minutes so freshness paths
    // (which name the `done/` file) resolve without a second lookup.
    let mut aliases = Vec::new();
    for task in &ctx.index.tasks {
        if let (Some(archive), Some(block)) =
            (task.archive_path.as_deref(), task.block_id.as_deref())
        {
            let canonical = task_key(&task.source_path, block);
            if let Some(minutes) = linked.get(&canonical).copied() {
                aliases.push((task_key(archive, block), minutes));
            }
        }
    }
    for (alias, minutes) in aliases {
        linked.entry(alias).or_insert(minutes);
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

/// Minutes for a freshness-lane row, whose path names the on-disk file:
/// resolve through either the source or the archive spelling to the one
/// canonical identity.
fn linked_for_path(path: &str, block: &str, ctx: &Ctx) -> u32 {
    if block.trim().is_empty() {
        return 0;
    }
    let direct = task_key(path, block);
    if let Some(minutes) = ctx.linked.get(&direct).copied() {
        return minutes;
    }
    let target = normalize_target(path);
    let block_lower = block.trim().to_ascii_lowercase();
    for task in &ctx.index.tasks {
        let task_block = task.block_id.as_deref().unwrap_or_default();
        if task_block.trim().to_ascii_lowercase() != block_lower {
            continue;
        }
        if task_matches_target(task, &target)
            && let Some(canonical_block) = task.block_id.as_deref()
        {
            return ctx
                .linked
                .get(&task_key(&task.source_path, canonical_block))
                .copied()
                .unwrap_or(0);
        }
    }
    0
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
/// Archived tasks render their `done/` home so the link resolves.
fn task_link(task: &IndexedTask) -> String {
    let stem = stem_of(task_effective_path(task));
    let description = task_alias(&task.description, ALIAS_CHARS);
    match task.block_id.as_deref() {
        Some(block) if !block.trim().is_empty() => {
            format!("[[{stem}#^{block}|{description}]]")
        }
        _ => format!("[[{stem}|{description}]]"),
    }
}

/// Alias text for a freshness-scan row: cleaned exactly like every
/// other task alias (`task_alias`), so `^block` ids never survive to be
/// cut mid-id by truncation.
fn clean_scan_alias(text: &str) -> String {
    task_alias(text, ALIAS_CHARS)
}

/// Longest task alias the brief prints.
const ALIAS_CHARS: usize = 70;

/// A short display alias for a task: `[key:: value]` inline fields,
/// `#tags`, `^block` ids, and symbol-only Markdown links (the Keep `💡`
/// link) dropped; wiki-links and other Markdown links flattened to their
/// text; whitespace collapsed; capped at `max_chars` with `…`.
fn task_alias(description: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(description.len());
    let mut rest = description;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        if let Some(inner) = tail.strip_prefix("[[")
            && let Some(end) = inner.find("]]")
        {
            out.push_str(&wikilink_display(&tail[..end + 4]));
            rest = &inner[end + 2..];
            continue;
        }
        let after = &tail[1..];
        let Some(end) = after.find(']') else {
            out.push_str(after);
            rest = "";
            break;
        };
        let label = &after[..end];
        let next = &after[end + 1..];
        if label.contains("::") {
            // Inline field: dropped.
            rest = next;
            continue;
        }
        if next.starts_with('(')
            && let Some(close) = next.find(')')
        {
            // Markdown link: keep words, drop symbol-only labels.
            if label.chars().any(char::is_alphanumeric) {
                out.push_str(label);
            }
            rest = &next[close + 1..];
            continue;
        }
        out.push_str(label);
        rest = next;
    }
    out.push_str(rest);
    let words: Vec<&str> = out
        .split_whitespace()
        .filter(|word| !is_tag_word(word) && !word.starts_with('^'))
        .collect();
    short_description(&words.join(" "), max_chars)
}

/// `#task`, `#ref`, `#prj_is_blocked`, … — a `#` followed by a letter.
fn is_tag_word(word: &str) -> bool {
    word.strip_prefix('#')
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_alphabetic)
}

/// A ready-to-paste note link: `[[stem|label]]`. The label never
/// nests a wiki-link: spans flatten to their display text first.
fn note_link(path: &str, label: &str) -> String {
    format!("[[{}|{}]]", stem_of(path), flatten_wikilinks(label))
}

/// Strip exactly one bullet or checkbox marker. Any other leading
/// character — notably a leading `x` in real prose — survives.
fn strip_bullet_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    for marker in [
        "- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "* [X] ", "- [ ]",
        "- [x]", "- [X]", "* [ ]", "* [x]", "* [X]", "- ", "* ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest.trim();
        }
    }
    trimmed
}

/// Whether a child bullet carries nothing but one wiki-link: it is a
/// Task Link, not a verbatim Bryan quote.
fn is_bare_wikilink(text: &str) -> bool {
    let text = text.trim();
    if !(text.starts_with("[[") && text.ends_with("]]")) {
        return false;
    }
    let inner = &text[2..text.len().saturating_sub(2)];
    !inner.contains("[[") && !inner.contains("]]")
}

/// Display text for one `[[...]]` span: the alias, else the last path
/// segment without any `#section` or `#^block` suffix.
fn wikilink_display(span: &str) -> String {
    let inner = span
        .strip_prefix("[[")
        .unwrap_or(span)
        .strip_suffix("]]")
        .unwrap_or(span);
    if let Some((_, alias)) = inner.split_once('|') {
        return alias.trim().to_string();
    }
    let target = inner.split('#').next().unwrap_or(inner);
    target
        .rsplit('/')
        .next()
        .unwrap_or(target)
        .trim()
        .to_string()
}

/// Replace every `[[...]]` span with its display text and drop stray
/// brackets, so a quote carrying links never nests inside an alias.
fn flatten_wikilinks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let Some(relative_end) = rest[start + 2..].find("]]") else {
            break;
        };
        let end = start + 2 + relative_end + 2;
        out.push_str(&rest[..start]);
        out.push_str(&wikilink_display(&rest[start..end]));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out.replace(['[', ']'], "")
}

fn build_json(ctx: &mut Ctx) -> Result<Value, BriefError> {
    let header = header_section(ctx);
    let facts = facts_section(ctx);
    let time = time_section(ctx)?;
    let parts = parts_section(ctx);
    let finished = finished_section(ctx);
    let basis = finished_basis(ctx, &finished);
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
        "finished": finished.projects,
        "finished_projects_more": finished.projects_more,
        "finished_groups": finished.groups,
        "finished_total": finished.total,
        "finished_more": finished.total.saturating_sub(finished.shown),
        "finished_routine_omitted": finished.routine_omitted,
        "finished_daily_review_omitted": finished.daily_review_omitted,
        "finished_other_omitted": finished.other_omitted,
        "finished_basis": basis,
        "started": started,
        "dropped": dropped.rows,
        "dropped_total": dropped.total,
        "dropped_more": dropped.total.saturating_sub(dropped.shown),
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
            task_effective_path(task),
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
                // Days have no Review: no review state, Headline, or lead.
                if let Some(object) = part.as_object_mut() {
                    object.remove("review_state");
                    object.remove("headline");
                    object.remove("reflection_lead");
                }
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

/// A daily note's `^gtd` review row (the `[[gtd_daily]]` checklist
/// task). Stats' Done never counts it; Review closed covers it.
fn is_daily_gtd_row(task: &IndexedTask) -> bool {
    task.block_id
        .as_deref()
        .is_some_and(|id| id.trim().eq_ignore_ascii_case("gtd"))
        && canonical_daily_date(&task.source_path).is_some()
}

/// Routine rows the Finished list omits, matching the Done fact's
/// basis: every recurring task (the recurring `#gtd` rows `Practice`
/// tracks among them) and the daily `^gtd` review rows.
fn is_routine(task: &IndexedTask) -> bool {
    task.recurring || is_daily_gtd_row(task)
}

/// Whether the Done fact counts this completion: status DONE with a
/// well-formed, non-future done date (the stats `done` metric's rule).
fn counts_as_done(task: &IndexedTask) -> bool {
    task.status_type == "DONE" && task.done.diagnostic == DateDiagnostic::Ok
}

/// Rows shown behind the Dropped cap, plus the pre-cap total so
/// callers can report `+N more`.
struct CappedRows {
    rows: Value,
    total: usize,
    shown: usize,
}

/// Finished: `^prj` completions, then every other non-routine
/// completion grouped by root parent and home note.
struct Finished {
    projects: Value,
    projects_more: usize,
    groups: Value,
    total: usize,
    shown: usize,
    routine_omitted: usize,
    daily_review_omitted: usize,
    other_omitted: usize,
}

/// Group label for a task's home note: its original note stem (the
/// `done/` archive folds into it), with every daily note pooled.
fn finished_note_label(task: &IndexedTask) -> String {
    if canonical_daily_date(&task.source_path).is_some() {
        return "daily notes".to_string();
    }
    let stem = stem_of(&task.source_path);
    stem.rsplit('/').next().unwrap_or(stem).to_string()
}

/// One Finished group: (root parent key, note label) and its rows with
/// their linked minutes.
type FinishedGroup<'a> = ((String, String), Vec<(&'a IndexedTask, u32)>);

fn finished_section(ctx: &Ctx) -> Finished {
    let in_span = done_in_span(&ctx.index.tasks, ctx);
    let mut routine_omitted = 0;
    let mut daily_review_omitted = 0;
    let mut other_omitted = 0;
    let mut counted: Vec<&IndexedTask> = Vec::new();
    for task in in_span {
        if is_routine(task) {
            routine_omitted += 1;
            if is_daily_gtd_row(task) {
                daily_review_omitted += 1;
            }
        } else if !counts_as_done(task) {
            other_omitted += 1;
        } else {
            counted.push(task);
        }
    }
    let total = counted.len();
    let engine = HomesEngine::new(&ctx.index);
    let mut prj: Vec<&IndexedTask> = counted
        .iter()
        .filter(|task| is_prj(task))
        .copied()
        .collect();
    prj.sort_by(|left, right| left.description.cmp(&right.description));
    let projects_total = prj.len();
    prj.truncate(ctx.finish_cap);
    let group_by_area = matches!(ctx.id, PeriodId::Year(_));
    let projects: Vec<Value> = prj
        .iter()
        .map(|task| {
            let mut row = task_row(task, ctx);
            if group_by_area {
                let home = engine.home_for_task(
                    task_effective_path(task),
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
    // Group the rest by root parent, then home note.
    let mut root_labels: HashMap<String, String> = HashMap::new();
    for row in lane_rows(ctx, "root_parent") {
        root_labels.insert(str_field(row, "key"), str_field(row, "label"));
    }
    let mut groups: BTreeMap<(String, String), Vec<(&IndexedTask, u32)>> =
        BTreeMap::new();
    for task in counted.iter().filter(|task| !is_prj(task)) {
        let home = engine.home_for_task(
            &task.source_path,
            task.line_number,
            &task.line,
        );
        groups
            .entry((home.root_parent, finished_note_label(task)))
            .or_default()
            .push((task, linked_for(task, ctx)));
    }
    let mut root_totals: HashMap<String, usize> = HashMap::new();
    for ((root, _), rows) in &groups {
        *root_totals.entry(root.clone()).or_insert(0) += rows.len();
    }
    let mut ordered: Vec<FinishedGroup<'_>> = groups.into_iter().collect();
    for (_, rows) in ordered.iter_mut() {
        rows.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then(left.0.done.date.cmp(&right.0.done.date))
                .then_with(|| {
                    task_alias(&left.0.description, ALIAS_CHARS)
                        .cmp(&task_alias(&right.0.description, ALIAS_CHARS))
                })
        });
    }
    ordered.sort_by(
        |((left_root, left_note), left), ((right_root, right_note), right)| {
            root_totals
                .get(right_root)
                .cmp(&root_totals.get(left_root))
                .then(left_root.cmp(right_root))
                .then(right.len().cmp(&left.len()))
                .then(left_note.cmp(right_note))
        },
    );
    let groups: Vec<Value> = ordered
        .iter()
        .map(|((root, note), rows)| {
            let root_label = root_labels
                .get(root)
                .cloned()
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| stem_of(root).to_string());
            json!({
                "root": root_label,
                "note": note,
                "count": rows.len(),
                "rows": rows
                    .iter()
                    .map(|(task, minutes)| json!({
                        "link": task_link(task),
                        "minutes": minutes,
                        "display": format_minutes(i64::from(*minutes)),
                    }))
                    .collect::<Vec<_>>(),
                "more": 0,
            })
        })
        .collect();
    // Every grouped row starts shown; `fit_finished` trims to budget.
    let given: usize = ordered.iter().map(|(_, rows)| rows.len()).sum();
    let shown = projects.len() + given;
    Finished {
        projects: Value::Array(projects),
        projects_more: projects_total.saturating_sub(prj.len()),
        groups: Value::Array(groups),
        total,
        shown,
        routine_omitted,
        daily_review_omitted,
        other_omitted,
    }
}

/// One line that says how Finished relates to the Done fact.
fn finished_basis(ctx: &Ctx, finished: &Finished) -> String {
    let done_display = ctx
        .report
        .get("facts")
        .and_then(Value::as_array)
        .and_then(|facts| {
            facts
                .iter()
                .find(|fact| str_field(fact, "metric") == "done")
        })
        .map(|fact| str_field(fact, "display"))
        .unwrap_or_default();
    let mut line = format!("{} completions", finished.total);
    if !done_display.is_empty() {
        if done_display == finished.total.to_string() {
            line.push_str(&format!(
                ", the same set the Done fact counts ({done_display})"
            ));
        } else {
            line.push_str(&format!(" (the Done fact shows {done_display})"));
        }
    }
    line.push('.');
    if finished.routine_omitted > 0 {
        line.push_str(&format!(
            " Omitted as routine: {} recurring or daily-review rows",
            finished.routine_omitted
        ));
        if finished.daily_review_omitted > 0 {
            line.push_str(&format!(
                ", {} of them daily `^gtd` reviews (Review closed covers those)",
                finished.daily_review_omitted
            ));
        }
        line.push('.');
    }
    if finished.other_omitted > 0 {
        line.push_str(&format!(
            " Also omitted: {} rows with a done date that Done does not count (status not DONE, or a malformed or future date).",
            finished.other_omitted
        ));
    }
    line
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
fn dropped_section(ctx: &Ctx) -> CappedRows {
    let mut dropped: Vec<&IndexedTask> = ctx
        .index
        .tasks
        .iter()
        .filter(|task| span_contains(task.cancelled.date, ctx))
        .collect();
    dropped.sort_by(|left, right| left.description.cmp(&right.description));
    let total = dropped.len();
    dropped.truncate(ctx.finish_cap);
    let shown = dropped.len();
    CappedRows {
        rows: Value::Array(
            dropped
                .into_iter()
                .map(|task| task_row(task, ctx))
                .collect(),
        ),
        total,
        shown,
    }
}

/// Open work that got linked minutes in the span: open at the span end
/// (today for an open period), not routine, ranked by minutes, capped
/// at the finish cap. Returns the rows and the count behind the cap.
fn worked_open(ctx: &Ctx, end: NaiveDate) -> (Vec<Value>, usize) {
    let mut rows: Vec<(&IndexedTask, u32)> = ctx
        .index
        .tasks
        .iter()
        .filter(|task| !is_routine(task) && is_open_at(task, end))
        .map(|task| (task, linked_for(task, ctx)))
        .filter(|(_, minutes)| *minutes > 0)
        .collect();
    rows.sort_by(|left, right| {
        right.1.cmp(&left.1).then_with(|| {
            task_alias(&left.0.description, ALIAS_CHARS)
                .cmp(&task_alias(&right.0.description, ALIAS_CHARS))
        })
    });
    let total = rows.len();
    rows.truncate(ctx.finish_cap);
    let more = total - rows.len();
    (
        rows.into_iter()
            .map(|(task, _)| task_row(task, ctx))
            .collect(),
        more,
    )
}

fn still_open_section(ctx: &Ctx) -> Value {
    // Ended periods never report today's vault. Rebuild Worked on and
    // Kept 3+ as of the span end; Blocked projects has no history and is
    // omitted with a note. The current (and future) period keeps today's
    // live behavior and adds today's Next/WIP lanes with no linked
    // minutes. A closed period has no such list: lane membership has no
    // history, and zero linked minutes is not a stall.
    let is_past = matches!(ctx.state, super::calendar::PeriodState::Closed)
        && ctx.observed_end.is_some();
    let as_of = ctx
        .observed_end
        .unwrap_or(ctx.today)
        .format("%Y-%m-%d")
        .to_string();
    if !is_past {
        let (worked, worked_more) =
            worked_open(ctx, ctx.observed_end.unwrap_or(ctx.today));
        let snapshot = crate::native::freshness::scan::scan(&ctx.vault).ok();
        let mut zero_minute: Vec<Value> = Vec::new();
        if let Some(snapshot) = snapshot.as_ref() {
            let mut lanes: Vec<&crate::native::freshness::scan::RowCtx> =
                snapshot
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
                let minutes = linked_for_path(&task.path, block, ctx);
                if minutes == 0 {
                    let alias =
                        short_description(&clean_scan_alias(&task.text), 120);
                    let link = if block.trim().is_empty() {
                        note_link(&task.path, &alias)
                    } else {
                        format!("[[{}#^{block}|{alias}]]", stem_of(&task.path))
                    };
                    zero_minute.push(json!({ "link": link }));
                }
            }
        }
        let zero_minute_total = zero_minute.len();
        zero_minute.truncate(ctx.finish_cap);
        let zero_minute_more = zero_minute_total - zero_minute.len();
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
            "worked_open": worked,
            "worked_open_more": worked_more,
            "zero_minute": zero_minute,
            "zero_minute_more": zero_minute_more,
            "blocked_prj": blocked_prj,
            "kept": kept,
            "as_of": as_of,
            "omitted_reason": Value::Null,
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
        return section;
    }
    let end = ctx.observed_end.expect("past has a span end");
    // Worked on, still open as of the span end: open at the end, with
    // linked minutes in the span.
    let (worked, worked_more) = worked_open(ctx, end);
    // Kept 3+ as of the span end: count dated Rotten Log entries on or
    // before the end, not today's `[keeps:: N]`.
    let mut historical: Vec<&IndexedTask> = ctx
        .index
        .tasks
        .iter()
        .filter(|task| created_on_or_before(task, end) && is_open_at(task, end))
        .collect();
    historical.sort_by(|left, right| {
        left.source_path
            .cmp(&right.source_path)
            .then(left.description.cmp(&right.description))
    });
    let mut kept: Vec<Value> = Vec::new();
    for task in historical {
        let entries = rotten_entries_as_of(ctx, task, end);
        if entries.len() >= 3 {
            kept.push(json!({
                "link": task_link(task),
                "keeps": entries.len() as u64,
                "reason": entries.last().map(|(_, reason)| reason.clone()).unwrap_or_default(),
            }));
        }
    }
    kept.truncate(ctx.finish_cap);
    let mut section = json!({
        "worked_open": worked,
        "worked_open_more": worked_more,
        "zero_minute": Value::Null,
        "zero_minute_more": 0,
        "blocked_prj": Value::Array(Vec::new()),
        "kept": kept,
        "as_of": end.format("%Y-%m-%d").to_string(),
        "omitted_reason": "blocked state has no history",
    });
    if matches!(ctx.id, PeriodId::Quarter { .. } | PeriodId::Year(_)) {
        let mut stalled: Vec<Value> = ctx
            .index
            .tasks
            .iter()
            .filter(|task| {
                is_prj(task)
                    && created_on_or_before(task, end)
                    && is_open_at(task, end)
                    && linked_for(task, ctx) == 0
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

/// Whether a task was still open at the span end: neither done nor
/// cancelled on or before that date.
fn is_open_at(task: &IndexedTask, end: NaiveDate) -> bool {
    done_date(task).is_none_or(|date| date > end)
        && cancelled_date(task).is_none_or(|date| date > end)
}

/// Whether a task existed at the span end: its `[created::]` falls on or
/// before that date. Tasks without a created date cannot be placed, so
/// past periods omit them rather than risk a wrong list.
fn created_on_or_before(task: &IndexedTask, end: NaiveDate) -> bool {
    created_date(task).is_some_and(|date| date <= end)
}

/// The task's created date, falling back to the raw line: the Tasks
/// parser stops at the first unrecognized trailing field (such as
/// `[keeps:: N]`), so `task.created` is empty for kept tasks even
/// though the line carries `[created:: …]`.
fn created_date(task: &IndexedTask) -> Option<NaiveDate> {
    task.created
        .date
        .or_else(|| inline_date(&task.line, &["created"]))
}

/// The task's done date, with the same raw-line fallback. The vault
/// writes `[completion:: …]` for completions.
fn done_date(task: &IndexedTask) -> Option<NaiveDate> {
    task.done
        .date
        .or_else(|| inline_date(&task.line, &["completion", "done"]))
}

fn cancelled_date(task: &IndexedTask) -> Option<NaiveDate> {
    task.cancelled
        .date
        .or_else(|| inline_date(&task.line, &["cancelled"]))
}

/// The first `YYYY-MM-DD` in a `[key:: …]` span on a raw task line.
fn inline_date(line: &str, keys: &[&str]) -> Option<NaiveDate> {
    for key in keys {
        let marker = format!("[{key}::");
        let mut rest = line;
        while let Some(start) = rest.find(&marker) {
            let after = rest[start + marker.len()..].trim_start();
            if let Some(day) = after.get(..10)
                && let Ok(date) = NaiveDate::parse_from_str(day, "%Y-%m-%d")
            {
                return Some(date);
            }
            rest = &rest[start + marker.len()..];
        }
    }
    None
}

/// Dated Rotten Log entries on or before the span end, in file order.
fn rotten_entries_as_of(
    ctx: &Ctx,
    task: &IndexedTask,
    end: NaiveDate,
) -> Vec<(NaiveDate, String)> {
    let Some(contents) = ctx.buffers.get(task_effective_path(task)) else {
        return Vec::new();
    };
    let lines: Vec<&str> = contents.lines().collect();
    let at = task.line_number.saturating_sub(1);
    let Some(task_line) = lines.get(at) else {
        return Vec::new();
    };
    let indent = leading_spaces(task_line);
    let mut in_rotten = false;
    let mut entries = Vec::new();
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
            if let Some(reason) = dated_entry_text(next) {
                if let Some(date) = dated_entry_date(next)
                    && date <= end
                {
                    entries.push((date, reason));
                }
            } else if leading_spaces(next) <= indent {
                break;
            }
        }
    }
    entries
}

/// The date of a `*YYYY-MM-DD …* — reason` drawer entry.
fn dated_entry_date(line: &str) -> Option<NaiveDate> {
    let star = line.find('*')?;
    let after = &line[star + 1..];
    let end = after.find('*')?;
    let stamp = after[..end].trim();
    let day = stamp.get(..10)?;
    NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()
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
        task_effective_path(task),
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
    rotten_log_reason(ctx, task_effective_path(task), task.line_number)
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

/// At most this many quotes per day, and this many overall: a heavy
/// early day must never bury the later days of the period.
const WORDS_PER_DAY: usize = 10;
const WORDS_TOTAL: usize = 40;

/// A bare host or role label (`athena agent`, `apollo research`,
/// `mac`, …): a launch marker, not Bryan's words.
fn is_host_label(text: &str) -> bool {
    let lowered = text.trim().trim_end_matches(['.', '!']).to_lowercase();
    let mut words = lowered.split_whitespace();
    let Some(host) = words.next() else {
        return false;
    };
    if !matches!(host, "athena" | "apollo" | "mac") {
        return false;
    }
    match (words.next(), words.next()) {
        (None, _) => true,
        (Some(role), None) => {
            matches!(role, "agent" | "agents" | "research" | "epic" | "epics")
        }
        _ => false,
    }
}

fn bryans_words_section(ctx: &Ctx) -> Value {
    let mut words = work_log_entries(ctx);
    words.extend(pomodoro_notes(ctx));
    words.retain(|word| !is_host_label(&str_field(word, "text")));
    // Dedup mirrored lines: a task's WORK LOG mirror and its daily-note
    // Pomodoro child emit the same (date, text). Pomodoro rows arrive
    // last, so the daily-note link wins the duplicate.
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    let mut deduped: Vec<Value> = Vec::new();
    for word in words {
        let key = (str_field(&word, "date"), str_field(&word, "text"));
        if let Some(&index) = at.get(&key) {
            deduped[index] = word;
        } else {
            at.insert(key, deduped.len());
            deduped.push(word);
        }
    }
    // Chronological only: a stable sort keeps each day's rows in the
    // order they were written instead of alphabetizing link text.
    deduped.sort_by(|left, right| {
        str_field(left, "date").cmp(&str_field(right, "date"))
    });
    // Fair across days: round-robin by date, so every day with quotes
    // keeps its earliest rows even when an early day overflows.
    let mut by_date: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for word in deduped {
        by_date
            .entry(str_field(&word, "date"))
            .or_default()
            .push(word);
    }
    let mut capped: Vec<Value> = Vec::new();
    let mut round = 0;
    while capped.len() < WORDS_TOTAL {
        let mut took = false;
        for words in by_date.values() {
            let Some(word) = words.get(round) else {
                continue;
            };
            if round >= WORDS_PER_DAY {
                continue;
            }
            capped.push(word.clone());
            took = true;
            if capped.len() >= WORDS_TOTAL {
                break;
            }
        }
        if !took {
            break;
        }
        round += 1;
    }
    capped.sort_by(|left, right| {
        str_field(left, "date").cmp(&str_field(right, "date"))
    });
    Value::Array(capped)
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
                let quote = flatten_wikilinks(&text);
                entries.push(json!({
                    "date": date.format("%Y-%m-%d").to_string(),
                    "link": note_link(path, &short_description(&quote, 80)),
                    "text": short_description(&quote, 280),
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
                let stripped = strip_bullet_marker(trimmed);
                if stripped.is_empty() || stripped.contains("#^") {
                    continue;
                }
                if is_bare_wikilink(stripped) {
                    continue;
                }
                let text = flatten_wikilinks(stripped);
                if text.trim().is_empty() {
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
    let prev_sections = ctx
        .buffers
        .get(&prev_path)
        .map(|contents| note::parse(contents).sections)
        .unwrap_or_default();
    let prev_reflection = truncate_words(
        &prev_sections.get("Reflection").cloned().unwrap_or_default(),
        300,
    );
    let prev_review =
        previous_review(prev_sections.get("Review").map_or("", String::as_str));
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
        "prev_review": prev_review,
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

/// Cap for each previous-Review part the brief carries.
const PREV_PART_WORDS: usize = 150;

/// The previous same-kind Review's Headline (its whole first paragraph,
/// joined onto one line), its Carried forward block (`Still open` in v1
/// drafts), and its Questions block, each verbatim and capped. Absent
/// parts are empty strings.
fn previous_review(review: &str) -> Value {
    let parts = review_parts(review);
    // The same paragraph rule parents use, so a hard-wrapped v1
    // Headline arrives whole.
    let headline = note::parse_headline(review).unwrap_or_default();
    let (carried_label, carried) = parts
        .iter()
        .find(|(label, _)| label == "Carried forward")
        .or_else(|| parts.iter().find(|(label, _)| label == "Still open"))
        .map(|(label, body)| {
            (label.clone(), cap_words_keep_lines(body, PREV_PART_WORDS))
        })
        .unwrap_or_default();
    let questions = parts
        .iter()
        .find(|(label, _)| label == "Questions")
        .map(|(_, body)| cap_words_keep_lines(body, PREV_PART_WORDS))
        .unwrap_or_default();
    json!({
        "headline": headline,
        "carried_label": carried_label,
        "carried": carried,
        "questions": questions,
    })
}

/// Split a Review into its `**Label:**` parts, in order. A part runs from
/// a line that opens with `**Label:**` to the next such line; its body is
/// the rest of the label line plus the following lines, trimmed.
fn review_parts(review: &str) -> Vec<(String, String)> {
    let mut parts: Vec<(String, Vec<String>)> = Vec::new();
    for line in review.lines() {
        if let Some(rest) = line.strip_prefix("**")
            && let Some(end) = rest.find(":**")
            && !rest[..end].contains("**")
        {
            let label = rest[..end].trim().to_string();
            let tail = rest[end + 3..].trim().to_string();
            parts.push((label, vec![tail]));
            continue;
        }
        if let Some((_, body)) = parts.last_mut() {
            body.push(line.to_string());
        }
    }
    parts
        .into_iter()
        .map(|(label, body)| (label, body.join("\n").trim().to_string()))
        .collect()
}

/// The first `cap` words of `text` with its line breaks kept, ending in
/// `…` when cut.
fn cap_words_keep_lines(text: &str, cap: usize) -> String {
    let mut left = cap;
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let words = line.split_whitespace().count();
        if words <= left {
            lines.push(line.to_string());
            left -= words;
            continue;
        }
        if left == 0 {
            if let Some(last) = lines.last_mut() {
                last.push('…');
            }
            break;
        }
        let indent: String =
            line.chars().take_while(|ch| ch.is_whitespace()).collect();
        let kept: Vec<&str> = line.split_whitespace().take(left).collect();
        lines.push(format!("{indent}{}…", kept.join(" ")));
        break;
    }
    lines.join("\n").trim_end().to_string()
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
    // Unreviewed children. Days have no Review, so a week skips this.
    if matches!(ctx.id, PeriodId::Week { .. }) {
        return Value::Array(caveats.into_iter().map(Value::String).collect());
    }
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
        // `edited` is a Review Bryan corrected: it counts as reviewed.
        if review_state == "empty" {
            let label = str_field(child, "label");
            if !label.is_empty() {
                unreviewed.push(label);
            }
        }
    }
    if !unreviewed.is_empty() {
        let noun = if matches!(ctx.id, PeriodId::Quarter { .. }) {
            "weeks"
        } else {
            "quarters"
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

fn render_markdown(ctx: &Ctx, json: &Value) -> String {
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
    let basis = str_field(json, "finished_basis");
    if !basis.is_empty() {
        out.push_str(&format!("{basis}\n"));
    }
    out.push_str("Projects (`^prj`):\n");
    render_finished(
        &mut out,
        json.get("finished").and_then(Value::as_array),
        json.get("finished_projects_more")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    );
    if let Some(groups) = json.get("finished_groups").and_then(Value::as_array)
    {
        for group in groups {
            let root = str_field(group, "root");
            let note = str_field(group, "note");
            let label = if note == root {
                root
            } else {
                format!("{root} › {note}")
            };
            out.push_str(&format!(
                "{label} · {} done:\n",
                group.get("count").and_then(Value::as_u64).unwrap_or(0),
            ));
            render_task_rows(
                &mut out,
                group.get("rows").and_then(Value::as_array),
                "None.",
            );
            render_more(
                &mut out,
                group.get("more").and_then(Value::as_u64).unwrap_or(0),
            );
        }
    }
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
    render_more(
        &mut out,
        json.get("dropped_more")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    );
    // Still open. Ended periods carry the span end in the title; the
    // current period keeps today's wording.
    if matches!(ctx.state, super::calendar::PeriodState::Closed)
        && let Some(end) = ctx.observed_end
    {
        out.push_str(&format!(
            "\n## Still open, as of {}\n",
            end.format("%Y-%m-%d")
        ));
    } else {
        out.push_str("\n## Still open, as of today\n");
    }
    if let Some(still) = json.get("still_open") {
        out.push_str("Worked on, still open:\n");
        render_task_rows(
            &mut out,
            still.get("worked_open").and_then(Value::as_array),
            "None.",
        );
        render_more(
            &mut out,
            still
                .get("worked_open_more")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        if let Some(zero) = still.get("zero_minute").and_then(Value::as_array) {
            out.push_str("Next or WIP with no linked minutes:\n");
            render_links(&mut out, Some(zero), "None.");
            render_more(
                &mut out,
                still
                    .get("zero_minute_more")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            );
        }
        out.push_str("Blocked projects:\n");
        if still
            .get("omitted_reason")
            .and_then(Value::as_str)
            .is_some()
        {
            out.push_str(&format!(
                "Omitted — {}.\n",
                still
                    .get("omitted_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("blocked state has no history")
            ));
        } else if let Some(rows) =
            still.get("blocked_prj").and_then(Value::as_array)
        {
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
        if let Some(review) = neighbours.get("prev_review") {
            let headline = str_field(review, "headline");
            if !headline.is_empty() {
                out.push_str(&format!(
                    "### Previous Review: Headline\n{headline}\n"
                ));
            }
            let carried = str_field(review, "carried");
            if !carried.is_empty() {
                out.push_str(&format!(
                    "### Previous Review: {} (≤{PREV_PART_WORDS} words)\n{carried}\n",
                    str_field(review, "carried_label"),
                ));
            }
            let questions = str_field(review, "questions");
            if !questions.is_empty() {
                out.push_str(&format!(
                    "### Previous Review: Questions (≤{PREV_PART_WORDS} words)\n{questions}\n"
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
    let dates = str_field(part, "dates");
    if state == "future" {
        let mut line = format!("- {}", str_field(part, "label"));
        if !dates.is_empty() {
            line.push_str(&format!(" · {dates}"));
        }
        line.push_str(" (not started)\n");
        out.push_str(&line);
        return;
    }
    let is_day = str_field(part, "kind") == "day";
    let mut segments = vec![str_field(part, "label")];
    if !dates.is_empty() {
        segments.push(dates);
    }
    segments.push(str_field(part, "worked_display"));
    // Days have no Review, so day rows carry no review state.
    if !is_day {
        let review_state = str_field(part, "review_state");
        let review_bits = if review_state.is_empty() || review_state == "empty"
        {
            "not reviewed".to_string()
        } else {
            let headline = str_field(part, "headline");
            if headline.is_empty() {
                review_state
            } else {
                format!("{review_state} · {headline}")
            }
        };
        segments.push(review_bits);
    }
    let mut line = format!("- {}", segments.join(" · "));
    let lead = str_field(part, "reflection_lead");
    if !lead.is_empty() {
        line.push_str(&format!(" · {lead}"));
    }
    if is_day {
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

/// The `+N more` tail behind a capped row list.
fn render_more(out: &mut String, more: u64) {
    if more > 0 {
        out.push_str(&format!("- +{more} more\n"));
    }
}

/// Finished `^prj` rows, grouped by area when the rows carry one (year).
fn render_finished(out: &mut String, rows: Option<&Vec<Value>>, more: u64) {
    let Some(rows) = rows else {
        out.push_str("None.\n");
        return;
    };
    if rows.is_empty() {
        out.push_str("None.\n");
        return;
    }
    if rows.iter().all(|row| row.get("area").is_none()) {
        render_task_rows(out, Some(rows), "None.");
        render_more(out, more);
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
        render_task_rows(out, Some(&grouped), "None.");
    }
    render_more(out, more);
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

/// Token budget per kind: a week ≤ 6k, a quarter ≤ 10k, a year ≤ 12k.
fn token_budget(id: PeriodId) -> usize {
    match id {
        PeriodId::Week { .. } => WEEK_BUDGET,
        PeriodId::Quarter { .. } => QUARTER_BUDGET,
        PeriodId::Year(_) => YEAR_BUDGET,
        PeriodId::AllTime => WEEK_BUDGET,
    }
}

/// Characters kept free below the budget after fitting Finished rows.
const FINISHED_HEADROOM: usize = 400;

/// One rendered Finished row: `- link · 25m`, or `- link` at 0m.
fn row_line_len(row: &Value) -> usize {
    let link = str_field(row, "link");
    let display = str_field(row, "display");
    if display.is_empty() || display == "0m" {
        link.len() + 3
    } else {
        link.len() + display.len() + 6
    }
}

/// Fit the grouped Finished rows into the kind's token budget. Every
/// other section renders first; the characters left over go to the
/// groups round-robin (one row per group per round, in group order), so
/// every group shows its top rows before any group shows more. A cut
/// group carries a bob-computed `more`, and `finished_more` counts every
/// completion not shown.
fn fit_finished(ctx: &Ctx, json: &mut Value) {
    let Some(groups) = json
        .get("finished_groups")
        .and_then(Value::as_array)
        .cloned()
    else {
        return;
    };
    let mut base = json.clone();
    if let Some(array) = base
        .get_mut("finished_groups")
        .and_then(Value::as_array_mut)
    {
        for group in array.iter_mut() {
            let count = group.get("count").and_then(Value::as_u64).unwrap_or(0);
            group["rows"] = Value::Array(Vec::new());
            group["more"] = count.into();
        }
    }
    let base_len = render_markdown(ctx, &base).len();
    let mut available = (token_budget(ctx.id) * 4)
        .saturating_sub(base_len)
        .saturating_sub(FINISHED_HEADROOM);
    let mut take = vec![0usize; groups.len()];
    loop {
        let mut progressed = false;
        for (index, group) in groups.iter().enumerate() {
            let Some(row) = group
                .get("rows")
                .and_then(Value::as_array)
                .and_then(|rows| rows.get(take[index]))
            else {
                continue;
            };
            let cost = row_line_len(row);
            if cost > available {
                continue;
            }
            available -= cost;
            take[index] += 1;
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    let mut shown = json
        .get("finished")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if let Some(array) = json
        .get_mut("finished_groups")
        .and_then(Value::as_array_mut)
    {
        for (group, keep) in array.iter_mut().zip(take) {
            let count = group.get("count").and_then(Value::as_u64).unwrap_or(0);
            if let Some(rows) =
                group.get_mut("rows").and_then(Value::as_array_mut)
            {
                rows.truncate(keep);
            }
            group["more"] = count.saturating_sub(keep as u64).into();
            shown += keep;
        }
    }
    let total = json
        .get("finished_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json["finished_more"] = total.saturating_sub(shown as u64).into();
}

/// Token budgets: a week ≤ 6k, a quarter ≤ 10k, a year ≤ 12k tokens.
/// Roughly four characters per token; trim Bryan's words, then
/// caveats, then neighbour bodies until the bundle fits.
fn enforce_budget(ctx: &Ctx, json: &mut Value, markdown: &mut String) {
    let budget = token_budget(ctx.id);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_settings(vault: &Path) {
        let path =
            vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            r##"{"taskFormat":"dataview","globalFilter":"#task"}"##,
        )
        .unwrap();
    }

    fn write_note(vault: &Path, relative: &str, contents: &str) {
        let path = vault.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// Brief attribution follows the resolver across a source rename: the
    /// archived row matches its new source spelling while its effective
    /// (on-disk archive) path stays put.
    #[test]
    fn brief_attribution_survives_source_rename() {
        let vault = tempfile::TempDir::new().unwrap();
        test_settings(vault.path());
        let now = NaiveDate::from_ymd_opt(2026, 10, 11)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        write_note(
            vault.path(),
            "work.md",
            "---\ntype: \"[[project]]\"\ndone_tasks: \"[[done/work_done]]\"\n---\n\n# Work\n",
        );
        write_note(
            vault.path(),
            "done/work_done.md",
            "---\nparent: \"[[work]]\"\ntype: \"[[done]]\"\n---\n\n- [x] #task Ship it [created:: 2026-10-09] [completion:: 2026-10-10] ^ship\n",
        );
        let before = StatsIndex::read(vault.path(), now).unwrap();
        let task = before
            .tasks
            .iter()
            .find(|task| task.description.contains("Ship it"))
            .unwrap();
        assert!(task_matches_target(task, "work"));
        assert_eq!(task_effective_path(task), "done/work_done.md");

        std::fs::rename(
            vault.path().join("work.md"),
            vault.path().join("job.md"),
        )
        .unwrap();
        write_note(
            vault.path(),
            "job.md",
            "---\ntype: \"[[project]]\"\ndone_tasks: \"[[done/work_done]]\"\n---\n\n# Work\n",
        );
        write_note(
            vault.path(),
            "done/work_done.md",
            "---\nparent: \"[[job]]\"\ntype: \"[[done]]\"\n---\n\n- [x] #task Ship it [created:: 2026-10-09] [completion:: 2026-10-10] ^ship\n",
        );
        let after = StatsIndex::read(vault.path(), now).unwrap();
        let task = after
            .tasks
            .iter()
            .find(|task| task.description.contains("Ship it"))
            .unwrap();
        assert!(task_matches_target(task, "job"));
        assert!(!task_matches_target(task, "work"));
        assert_eq!(task_effective_path(task), "done/work_done.md");
    }

    #[test]
    fn task_alias_drops_fields_tags_blocks_and_keep_links() {
        let description = "#task #ref [[ref/blogs/x|The AI-native SDLC playbook]] [fresh:: 2026-10-28] ^ref-x";
        assert_eq!(
            task_alias(description, ALIAS_CHARS),
            "The AI-native SDLC playbook"
        );
        let keep = "#task Add support for \"agent hijack\" turns! [💡](https://keep.google.com/u/0/#NOTE/1a1 \"Open in Google Keep\") [created:: 2026-10-01]";
        assert_eq!(
            task_alias(keep, ALIAS_CHARS),
            "Add support for \"agent hijack\" turns!"
        );
        assert_eq!(
            task_alias("Read [the docs](https://example.com) now", 70),
            "Read the docs now"
        );
        assert_eq!(
            task_alias("Ship `#ref` support #prj_is_blocked", 70),
            "Ship `#ref` support"
        );
        let long = "word ".repeat(40);
        assert!(task_alias(&long, 20).ends_with('…'));
        assert!(task_alias(&long, 20).chars().count() <= 20);
    }

    #[test]
    fn host_labels_are_launch_markers_not_words() {
        for label in [
            "athena agent",
            "apollo research",
            "athena agents",
            "Apollo Epic",
            "mac",
            "athena",
        ] {
            assert!(is_host_label(label), "{label}");
        }
        for words in [
            "athena agent to add songs to playlist",
            "Running agents on athena and apollo.",
            "epic launching on apollo",
            "apollo agent ran out",
        ] {
            assert!(!is_host_label(words), "{words}");
        }
    }

    #[test]
    fn previous_review_joins_the_headline_and_keeps_parts_verbatim() {
        let review = "**Headline:** A 24h 45m week, 5h 28m below typical, put 45% into bob,\nled by WIP/Next auto-decay.\n\n**Shipped:**\n\n- one\n\n**Carried forward:**\n\n- [[a#^b|Docs]] · kept 3×\n- second\n\n**Questions:**\n\n1. Why?\n2. What next?\n";
        let prev = previous_review(review);
        assert_eq!(
            prev["headline"],
            "A 24h 45m week, 5h 28m below typical, put 45% into bob, led by WIP/Next auto-decay."
        );
        assert_eq!(prev["carried_label"], "Carried forward");
        assert_eq!(prev["carried"], "- [[a#^b|Docs]] · kept 3×\n- second");
        assert_eq!(prev["questions"], "1. Why?\n2. What next?");

        let v1 =
            previous_review("**Headline:** Calm.\n\n**Still open:**\n\n- x\n");
        assert_eq!(v1["carried_label"], "Still open");
        assert_eq!(v1["carried"], "- x");
        assert_eq!(v1["questions"], "");

        let empty = previous_review("");
        assert_eq!(empty["headline"], "");
        assert_eq!(empty["carried"], "");
    }

    #[test]
    fn capped_parts_keep_their_lines() {
        assert_eq!(cap_words_keep_lines("- a b\n- c d", 5), "- a b\n- c…");
        assert_eq!(cap_words_keep_lines("- a b\n- c d", 3), "- a b…");
        assert_eq!(cap_words_keep_lines("- a b\n- c d", 10), "- a b\n- c d");
    }
}
