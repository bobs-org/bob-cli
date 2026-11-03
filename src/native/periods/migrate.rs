//! `bob periods migrate`: adopt the period-note tree.
//!
//! Dry run by default. `-a/--apply` runs one `vault_rewrite` transaction
//! that creates missing period notes, adopts in-range year notes,
//! re-parents out-of-range year notes and canonical dailies, and adds
//! the week nav link. The contract lives in `docs/periods.md`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;
use serde_json::json;

use super::super::{
    env as bob_env, ob,
    stats::{
        request::{Grouping, MetricSelection, StatsRequest},
        StatsError,
    },
    vault_rewrite::{self, FileRewrite, ReadyPlan, TxnFailure},
};
use super::calendar::PeriodId;
use super::note;
use super::problems::{collect_problems, scan_canonical_dailies, ProblemKind};
use super::scaffold;

const COMMAND_NAME: &str = "bob periods migrate";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &clap::ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let command = super::cli::migrate_command();
    let mut command = command;
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };
    let format = OutputFormat::from_matches(&matches);
    match execute(&matches) {
        Ok(report) => {
            print_success(&report, format);
            0
        }
        Err(error) => print_error(error, format),
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

#[derive(Debug)]
struct MigrateError {
    code: &'static str,
    message: String,
    hint: String,
    exit_code: i32,
}

impl MigrateError {
    fn usage(message: String) -> Self {
        Self {
            code: "invalid_period",
            message,
            hint: "Pass --since as YYYY-MM-DD.".to_string(),
            exit_code: 2,
        }
    }

    fn io(message: String) -> Self {
        Self {
            code: "io_error",
            message,
            hint: "Check the vault path and file permissions.".to_string(),
            exit_code: 1,
        }
    }

    fn from_txn(failure: TxnFailure) -> Self {
        let exit_code = match failure.code {
            "not_a_worktree" | "dirty_paths" => 3,
            _ => 1,
        };
        Self {
            code: failure.code,
            message: failure.message,
            hint: failure.hint.unwrap_or_else(|| {
                "See docs/periods.md for the migrate contract.".to_string()
            }),
            exit_code,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct MigratePlan {
    files: Vec<FileRewrite>,
    created: Vec<String>,
    filled: Vec<String>,
    adopted: Vec<String>,
    reparented: Vec<String>,
    rewritten: Vec<String>,
    nav_skipped: Vec<String>,
    existing: Vec<String>,
    iso_years: Vec<i32>,
    sample_diff: Option<String>,
    range_start: Option<NaiveDate>,
    range_end: Option<NaiveDate>,
}

#[derive(Debug, Clone)]
struct MigrateReport {
    plan: MigratePlan,
    dry_run: bool,
    sha: Option<String>,
    subject: String,
}

fn execute(matches: &clap::ArgMatches) -> Result<MigrateReport, MigrateError> {
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .unwrap_or_else(bob_env::bob_dir);
    let today = bob_env::current_datetime().date();
    let since = since_from_matches(matches, today)?;
    let apply = matches.get_flag("apply");
    let offline = matches.get_flag("offline");
    let plan = build_plan(&vault, since, today)?;
    let subject = commit_subject(&plan.iso_years);
    if !apply {
        return Ok(MigrateReport {
            plan,
            dry_run: true,
            sha: None,
            subject,
        });
    }
    apply_plan(&vault, plan, subject, offline)
}

fn since_from_matches(
    matches: &clap::ArgMatches,
    today: NaiveDate,
) -> Result<NaiveDate, MigrateError> {
    if let Some(raw) = matches.get_one::<String>("since") {
        NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").map_err(|_| {
            MigrateError::usage(format!(
                "invalid --since {raw:?}: expected YYYY-MM-DD"
            ))
        })
    } else {
        Ok(PeriodId::year_for_date(today)
            .start()
            .expect("ISO year has a start"))
    }
}

fn adoption_range(
    since: NaiveDate,
    today: NaiveDate,
) -> (NaiveDate, NaiveDate) {
    let start = PeriodId::week_for_date(since)
        .start()
        .expect("week has a start");
    let current = PeriodId::week_for_date(today);
    let through = current.next().unwrap_or(current);
    let end = through.end().expect("week has an end");
    (start, end)
}

fn iso_years_in_range(start: NaiveDate, end: NaiveDate) -> Vec<i32> {
    let first = match PeriodId::week_for_date(start) {
        PeriodId::Week { year, .. } => year,
        _ => return Vec::new(),
    };
    let last = match PeriodId::week_for_date(end) {
        PeriodId::Week { year, .. } => year,
        _ => first,
    };
    (first.min(last)..=first.max(last)).collect()
}

fn weeks_covering(start: NaiveDate, end: NaiveDate) -> Vec<PeriodId> {
    let mut week = PeriodId::week_for_date(start);
    let last = PeriodId::week_for_date(end);
    let mut out = Vec::new();
    loop {
        out.push(week);
        if week == last {
            break;
        }
        week = match week.next() {
            Some(next) => next,
            None => break,
        };
    }
    out
}

fn unique_parents(weeks: &[PeriodId]) -> Vec<PeriodId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for week in weeks {
        if let Some(parent) = week.parent()
            && seen.insert(parent)
        {
            out.push(parent);
        }
    }
    out
}

fn build_plan(
    vault: &Path,
    since: NaiveDate,
    today: NaiveDate,
) -> Result<MigratePlan, MigrateError> {
    let (range_start, range_end) = adoption_range(since, today);
    let iso_years = iso_years_in_range(range_start, range_end);
    let mut plan = MigratePlan {
        iso_years: iso_years.clone(),
        range_start: Some(range_start),
        range_end: Some(range_end),
        ..MigratePlan::default()
    };

    plan_create_or_fill(vault, PeriodId::AllTime, &mut plan)?;

    plan_year_notes(vault, &iso_years, &mut plan)?;

    let weeks = weeks_covering(range_start, range_end);
    let quarters = unique_parents(&weeks);
    for id in quarters.into_iter().chain(weeks) {
        plan_create_or_fill(vault, id, &mut plan)?;
    }

    plan_dailies(vault, range_start, &mut plan)?;
    Ok(plan)
}

fn plan_create_or_fill(
    vault: &Path,
    id: PeriodId,
    plan: &mut MigratePlan,
) -> Result<(), MigrateError> {
    let relative = path_string(id.path());
    let target = vault.join(&relative);
    match fs::read_to_string(&target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            push_file(
                plan,
                FileRewrite {
                    path: relative.clone(),
                    before_contents: None,
                    after_contents: scaffold::render(id),
                },
            );
            plan.created.push(relative);
            Ok(())
        }
        Err(error) => Err(MigrateError::io(format!(
            "could not read {relative}: {error}"
        ))),
        Ok(contents) => {
            if contents.trim().is_empty() {
                let after = scaffold::render(id);
                if contents != after {
                    push_file(
                        plan,
                        FileRewrite {
                            path: relative.clone(),
                            before_contents: Some(contents),
                            after_contents: after,
                        },
                    );
                }
                plan.filled.push(relative);
            } else {
                plan.existing.push(relative);
            }
            Ok(())
        }
    }
}

fn plan_year_notes(
    vault: &Path,
    iso_years: &[i32],
    plan: &mut MigratePlan,
) -> Result<(), MigrateError> {
    let in_range: BTreeSet<i32> = iso_years.iter().copied().collect();
    let mut seen_in_range: BTreeSet<i32> = BTreeSet::new();
    let Ok(entries) = fs::read_dir(vault) else {
        for year in iso_years {
            plan_create_or_fill(vault, PeriodId::Year(*year), plan)?;
        }
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.len() != 7 || !name.ends_with(".md") {
            continue;
        }
        let Ok(id) = PeriodId::parse(&name[..4]) else {
            continue;
        };
        let PeriodId::Year(year) = id else {
            continue;
        };
        let relative = path_string(id.path());
        let contents = fs::read_to_string(&path).map_err(|error| {
            MigrateError::io(format!("could not read {relative}: {error}"))
        })?;
        if in_range.contains(&year) {
            seen_in_range.insert(year);
            if contents.trim().is_empty() {
                plan_create_or_fill(vault, id, plan)?;
                continue;
            }
            if already_adopted(&contents, id) {
                plan.existing.push(relative);
                continue;
            }
            let after = adopt_year(id, &contents);
            if after != contents {
                push_file(
                    plan,
                    FileRewrite {
                        path: relative.clone(),
                        before_contents: Some(contents),
                        after_contents: after,
                    },
                );
            }
            plan.adopted.push(relative);
        } else {
            match reparent_year_to_all_time(&contents) {
                Some(after) if after != contents => {
                    push_file(
                        plan,
                        FileRewrite {
                            path: relative.clone(),
                            before_contents: Some(contents),
                            after_contents: after,
                        },
                    );
                    plan.reparented.push(relative);
                }
                _ => {
                    plan.existing.push(relative);
                }
            }
        }
    }
    for year in iso_years {
        if !seen_in_range.contains(year) {
            plan_create_or_fill(vault, PeriodId::Year(*year), plan)?;
        }
    }
    Ok(())
}

fn plan_dailies(
    vault: &Path,
    range_start: NaiveDate,
    plan: &mut MigratePlan,
) -> Result<(), MigrateError> {
    for (date, relative, contents) in scan_canonical_dailies(vault) {
        if date < range_start {
            continue;
        }
        let week = PeriodId::week_for_date(date);
        let (after, nav_rewritten, nav_skipped) =
            rewrite_daily(&contents, week);
        if after == contents {
            continue;
        }
        if nav_skipped {
            plan.nav_skipped.push(relative.clone());
        }
        if plan.sample_diff.is_none() {
            plan.sample_diff = Some(sample_diff(&relative, &contents, &after));
        }
        push_file(
            plan,
            FileRewrite {
                path: relative.clone(),
                before_contents: Some(contents),
                after_contents: after,
            },
        );
        plan.rewritten.push(relative);
        let _ = nav_rewritten;
    }
    Ok(())
}

fn push_file(plan: &mut MigratePlan, file: FileRewrite) {
    if file.before_contents.as_deref() == Some(file.after_contents.as_str()) {
        return;
    }
    plan.files.push(file);
}

fn apply_plan(
    vault: &Path,
    plan: MigratePlan,
    subject: String,
    offline: bool,
) -> Result<MigrateReport, MigrateError> {
    if plan.files.is_empty() {
        return Ok(MigrateReport {
            plan,
            dry_run: false,
            sha: None,
            subject,
        });
    }
    let child_env = ob::child_env();
    let range_start = plan.range_start.expect("range");
    let range_end = plan.range_end.expect("range");
    let now = bob_env::current_datetime();
    let before_parity = capture_parity(vault, range_start, range_end, now)
        .map_err(|message| {
            MigrateError::io(format!(
                "could not snapshot stats before migrate: {message}"
            ))
        })?;
    let files = plan.files.clone();
    let vault_for_plan = vault.to_path_buf();
    let subject_for_plan = subject.clone();
    let outcome = vault_rewrite::run(
        vault,
        &child_env,
        COMMAND_NAME,
        "period-note adoption",
        offline,
        false,
        move || {
            let verify_vault = vault_for_plan.clone();
            Ok(ReadyPlan {
                files,
                subject: subject_for_plan,
                post_write: Box::new(|| Ok(Vec::new())),
                verify: Box::new(move || {
                    verify_adoption(
                        &verify_vault,
                        range_start,
                        range_end,
                        now,
                        &before_parity,
                    )
                }),
                on_committed: Box::new(|| {}),
                context: (),
            })
        },
    )
    .map_err(MigrateError::from_txn)?;
    Ok(MigrateReport {
        plan,
        dry_run: false,
        sha: Some(outcome.sha),
        subject: outcome.subject,
    })
}

fn verify_adoption(
    vault: &Path,
    range_start: NaiveDate,
    range_end: NaiveDate,
    now: chrono::NaiveDateTime,
    before: &ParitySnapshot,
) -> Result<(), String> {
    let after = capture_parity(vault, range_start, range_end, now)?;
    compare_parity(before, &after)?;
    let today = now.date();
    let problems = collect_problems(vault, today)?;
    if let Some(problem) = problems.iter().find(|problem| {
        problem.kind == ProblemKind::DanglingDailyParent
            && daily_in_range(vault, &problem.path, range_start)
    }) {
        return Err(format!(
            "dangling_daily_parent remains on {}",
            problem.path
        ));
    }
    Ok(())
}

fn daily_in_range(
    _vault: &Path,
    relative: &str,
    range_start: NaiveDate,
) -> bool {
    crate::native::stats::index::canonical_daily_date(relative)
        .is_some_and(|date| date >= range_start)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParitySnapshot {
    days: BTreeMap<(NaiveDate, String), DayParity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DayParity {
    value: Option<i64>,
    parts: BTreeMap<String, BTreeMap<String, i64>>,
}

fn capture_parity(
    vault: &Path,
    start: NaiveDate,
    end: NaiveDate,
    now: chrono::NaiveDateTime,
) -> Result<ParitySnapshot, String> {
    use crate::native::config;
    let mut request =
        StatsRequest::at(vault.to_path_buf(), config::config_path(), now);
    request.selection = MetricSelection::Explicit(vec![
        "worked".to_string(),
        "done".to_string(),
        "created".to_string(),
    ]);
    request.range = Some(format!("{start}..{end}"));
    request.grouping = Grouping::Day;
    request.compare = false;
    let report = crate::native::stats::compute(&request)
        .map_err(|error: StatsError| error.message().to_string())?;
    let mut days = BTreeMap::new();
    for metric in report.metrics {
        if !matches!(metric.id.as_str(), "worked" | "done" | "created") {
            continue;
        }
        for point in metric.points.into_iter().chain(metric.daily_points) {
            days.insert(
                (point.date, metric.id.clone()),
                DayParity {
                    value: point.value,
                    parts: point.parts,
                },
            );
        }
    }
    Ok(ParitySnapshot { days })
}

fn compare_parity(
    before: &ParitySnapshot,
    after: &ParitySnapshot,
) -> Result<(), String> {
    let keys: BTreeSet<_> = before
        .days
        .keys()
        .chain(after.days.keys())
        .cloned()
        .collect();
    for key in keys {
        let left = before.days.get(&key);
        let right = after.days.get(&key);
        if left != right {
            let (date, metric) = key;
            return Err(format!(
                "verify_failed: {date} {metric} changed during migrate"
            ));
        }
    }
    Ok(())
}

fn already_adopted(contents: &str, id: PeriodId) -> bool {
    let parsed = note::parse(contents);
    parsed.frontmatter.get("parent").map(String::as_str) == Some("[[all_time]]")
        && note::structure_ok(id, contents)
}

fn adopt_year(id: PeriodId, existing: &str) -> String {
    let scaffold = scaffold::render(id);
    let (scaffold_fm, scaffold_body) = split_frontmatter(&scaffold);
    let (old_fm, old_body) = split_frontmatter(existing);
    let merged = merge_year_frontmatter(&old_fm, &scaffold_fm);
    let archive = old_body.trim();
    if archive.is_empty() {
        format!("{merged}\n\n{}\n", scaffold_body.trim_end())
    } else {
        format!(
            "{merged}\n\n{}\n\n## Archive\n\n{archive}\n",
            scaffold_body.trim_end()
        )
    }
}

fn reparent_year_to_all_time(contents: &str) -> Option<String> {
    let mut changed = false;
    let mut lines: Vec<String> = Vec::new();
    let mut in_fm = false;
    let mut fm_started = false;
    for line in contents.lines() {
        if !fm_started && line.trim() == "---" {
            fm_started = true;
            in_fm = true;
            lines.push(line.to_string());
            continue;
        }
        if in_fm && line.trim() == "---" {
            in_fm = false;
            lines.push(line.to_string());
            continue;
        }
        if in_fm && line.starts_with("parent:") {
            let replacement = "parent: \"[[all_time]]\"";
            if line.trim() != replacement {
                changed = true;
            }
            lines.push(replacement.to_string());
            continue;
        }
        lines.push(line.to_string());
    }
    changed.then(|| {
        let mut out = lines.join("\n");
        if contents.ends_with('\n') {
            out.push('\n');
        }
        out
    })
}

fn merge_year_frontmatter(old_inner: &str, scaffold_inner: &str) -> String {
    let replacements = key_blocks(scaffold_inner);
    let managed = ["parent", "type", "id", "date", "end", "aliases", "tags"];
    let old_lines: Vec<&str> = old_inner.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < old_lines.len() {
        let line = old_lines[index];
        if let Some(key) = frontmatter_key(line)
            && let Some(block) = replacements.get(key.as_str())
        {
            out.extend(block.lines().map(str::to_string));
            seen.insert(key.clone());
            index += 1;
            if key == "aliases" || key == "tags" {
                while index < old_lines.len() && is_indented(old_lines[index]) {
                    index += 1;
                }
            }
            continue;
        }
        out.push(line.to_string());
        index += 1;
    }
    for key in managed {
        if !seen.contains(key)
            && let Some(block) = replacements.get(key)
        {
            out.extend(block.lines().map(str::to_string));
        }
    }
    format!("---\n{}\n---", out.join("\n"))
}

fn key_blocks(inner: &str) -> BTreeMap<String, String> {
    let mut blocks = BTreeMap::new();
    let lines: Vec<&str> = inner.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(key) = frontmatter_key(line) {
            let mut block = vec![line.to_string()];
            index += 1;
            if key == "aliases" || key == "tags" {
                while index < lines.len() && is_indented(lines[index]) {
                    block.push(lines[index].to_string());
                    index += 1;
                }
            }
            blocks.insert(key, block.join("\n"));
            continue;
        }
        index += 1;
    }
    blocks
}

fn frontmatter_key(line: &str) -> Option<String> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let (key, _) = line.split_once(':')?;
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_string())
}

fn is_indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn split_frontmatter(contents: &str) -> (String, String) {
    let mut lines = contents.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (String::new(), contents.to_string());
    }
    let mut inner = Vec::new();
    for line in lines.by_ref() {
        if line.trim() == "---" {
            let body: Vec<&str> = lines.collect();
            return (inner.join("\n"), body.join("\n"));
        }
        inner.push(line);
    }
    (String::new(), contents.to_string())
}

fn rewrite_daily(contents: &str, week: PeriodId) -> (String, bool, bool) {
    let target = week.link_target();
    let mut lines: Vec<String> = Vec::new();
    let mut in_fm = false;
    let mut fm_started = false;
    let mut fm_open_idx: Option<usize> = None;
    let mut fm_closed = false;
    let mut parent_found = false;
    let mut nav_rewritten = false;
    let mut saw_old_nav = false;
    for line in contents.lines() {
        if !fm_started && line.trim() == "---" {
            fm_started = true;
            in_fm = true;
            fm_open_idx = Some(lines.len());
            lines.push(line.to_string());
            continue;
        }
        if in_fm && line.trim() == "---" {
            in_fm = false;
            fm_closed = true;
            lines.push(line.to_string());
            continue;
        }
        if in_fm
            && line.trim_start().starts_with("parent:")
            && let Some(rewritten) = rewrite_parent_line(line, &target)
        {
            parent_found = true;
            lines.push(rewritten);
            continue;
        }
        if !in_fm && let Some(rewritten) = rewrite_nav_line(line, week) {
            saw_old_nav = true;
            nav_rewritten = true;
            lines.push(rewritten);
            continue;
        }
        lines.push(line.to_string());
    }
    if fm_closed
        && !parent_found
        && let Some(open_idx) = fm_open_idx
    {
        lines.insert(open_idx + 1, format!("parent: \"[[{target}]]\""));
    }
    let mut after = lines.join("\n");
    if contents.ends_with('\n') {
        after.push('\n');
    }
    let nav_skipped = !saw_old_nav;
    (after, nav_rewritten, nav_skipped)
}

fn rewrite_parent_line(line: &str, link_target: &str) -> Option<String> {
    let trimmed_start = line.trim_start();
    let indent_len = line.len() - trimmed_start.len();
    let indent = &line[..indent_len];
    let rest = trimmed_start.strip_prefix("parent:")?;
    let ws_len = rest.len() - rest.trim_start().len();
    let ws = &rest[..ws_len];
    let value = rest.trim();
    let (open, close, _inner) = if value.len() >= 2
        && value.starts_with('\'')
        && value.ends_with('\'')
    {
        ("'", "'", &value[1..value.len() - 1])
    } else if value.len() >= 2 && value.starts_with('"') && value.ends_with('"')
    {
        ("\"", "\"", &value[1..value.len() - 1])
    } else {
        ("", "", value)
    };
    Some(format!("{indent}parent:{ws}{open}[[{link_target}]]{close}"))
}

fn rewrite_nav_line(line: &str, week: PeriodId) -> Option<String> {
    static NAV: OnceLock<Regex> = OnceLock::new();
    let regex = NAV.get_or_init(|| {
        Regex::new(
            r"^\[\[(\d{4}/\d{8})\|prev\]\]\s+\|\s+\[\[(\d{4}/\d{8})\|next\]\]\s*$",
        )
        .expect("nav regex")
    });
    let captures = regex.captures(line)?;
    let prev = captures.get(1)?.as_str();
    let next = captures.get(2)?.as_str();
    let week_link = week.link_target();
    let week_label = match week {
        PeriodId::Week { week, .. } => format!("W{week:02}"),
        _ => super::labels::short_label(week),
    };
    Some(format!(
        "[[{prev}|prev]] · [[{week_link}|{week_label}]] · [[{next}|next]]"
    ))
}

fn sample_diff(path: &str, before: &str, after: &str) -> String {
    let mut lines = vec![format!("--- {path}"), format!("+++ {path}")];
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    let max = before_lines.len().max(after_lines.len());
    let mut shown = 0;
    for index in 0..max {
        let old = before_lines.get(index).copied().unwrap_or("");
        let new = after_lines.get(index).copied().unwrap_or("");
        if old == new {
            continue;
        }
        if index < before_lines.len() {
            lines.push(format!("-{old}"));
        }
        if index < after_lines.len() {
            lines.push(format!("+{new}"));
        }
        shown += 1;
        if shown >= 12 {
            lines.push("…".to_string());
            break;
        }
    }
    lines.join("\n")
}

fn commit_subject(iso_years: &[i32]) -> String {
    let years = if iso_years.is_empty() {
        "year".to_string()
    } else {
        iso_years
            .iter()
            .map(|year| year.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("periods: adopt the period-note tree for ISO {years}")
}

fn path_string(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn print_success(report: &MigrateReport, format: OutputFormat) {
    match format {
        OutputFormat::Human => print_human(report),
        OutputFormat::Json => print_json(report),
    }
}

fn print_human(report: &MigrateReport) {
    let plan = &report.plan;
    let verb = if report.dry_run { "would " } else { "" };
    if plan.files.is_empty() && plan.nav_skipped.is_empty() {
        println!("nothing to do");
        return;
    }
    print_count(verb, "create", &plan.created);
    print_count(verb, "fill", &plan.filled);
    print_count(verb, "adopt", &plan.adopted);
    print_count(verb, "reparent", &plan.reparented);
    print_count(verb, "rewrite", &plan.rewritten);
    if !plan.existing.is_empty() {
        println!("{}existing {}", verb, join_sample(&plan.existing));
    }
    if !plan.nav_skipped.is_empty() {
        println!("nav_skipped {}", join_sample(&plan.nav_skipped));
    }
    if let Some(diff) = &plan.sample_diff {
        println!("sample daily diff:\n{diff}");
    }
    if report.dry_run {
        println!("-a will commit: {}", report.subject);
    } else if let Some(sha) = &report.sha {
        println!("committed {sha} {}", report.subject);
    } else {
        println!("nothing to commit");
    }
}

fn print_count(verb: &str, action: &str, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    println!("{verb}{action} {} {}", paths.len(), join_sample(paths));
}

fn join_sample(paths: &[String]) -> String {
    const CAP: usize = 8;
    if paths.len() <= CAP {
        paths.join(", ")
    } else {
        format!("{} … +{} more", paths[..CAP].join(", "), paths.len() - CAP)
    }
}

fn print_json(report: &MigrateReport) {
    let plan = &report.plan;
    let body = json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "dry_run": report.dry_run,
        "created": plan.created,
        "filled": plan.filled,
        "adopted": plan.adopted,
        "reparented": plan.reparented,
        "rewritten": plan.rewritten,
        "nav_skipped": plan.nav_skipped,
        "existing": plan.existing,
        "commit_subject": report.subject,
        "sha": report.sha,
        "sample_diff": plan.sample_diff,
        "iso_years": plan.iso_years,
    });
    println!("{}", serde_json::to_string(&body).unwrap_or_default());
}

fn print_error(error: MigrateError, format: OutputFormat) -> i32 {
    match format {
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {}", error.message);
            eprintln!("hint: {}", error.hint);
        }
        OutputFormat::Json => {
            let body = json!({
                "ok": false,
                "schema_version": SCHEMA_VERSION,
                "error": {
                    "code": error.code,
                    "message": error.message,
                    "hint": error.hint,
                },
            });
            println!("{}", serde_json::to_string(&body).unwrap_or_default());
        }
    }
    error.exit_code
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("date")
    }

    fn zorg_year() -> String {
        "---\nparent: [[journal]]\ntype: [[day]]\nzorg_id: 2026\n---\n\n# 2026\n\nIndex of 2026.\n"
            .to_string()
    }

    #[test]
    fn default_range_starts_at_iso_year_and_runs_through_next_week() {
        let today = date(2026, 11, 3);
        let since = PeriodId::year_for_date(today).start().unwrap();
        assert_eq!(since, date(2025, 12, 29));
        let (start, end) = adoption_range(since, today);
        assert_eq!(start, date(2025, 12, 29));
        // Current week is W45 (Nov 2–8); +1 is W46 (Nov 9–15).
        assert_eq!(end, date(2026, 11, 15));
        assert_eq!(iso_years_in_range(start, end), vec![2026]);
    }

    #[test]
    fn adopt_year_keeps_zorg_keys_and_archives_the_old_body() {
        let id = PeriodId::Year(2026);
        let adopted = adopt_year(id, &zorg_year());
        assert!(adopted.contains("parent: \"[[all_time]]\""), "{adopted}");
        assert!(adopted.contains("id: \"2026\""), "{adopted}");
        assert!(adopted.contains("zorg_id: 2026"), "{adopted}");
        assert!(adopted.contains("tags:\n  - yearly"), "{adopted}");
        assert!(
            adopted.contains("# 2026 · Dec 29, 2025 – Jan 3, 2027 · 53 weeks"),
            "{adopted}"
        );
        assert!(adopted.contains("## Archive"), "{adopted}");
        assert!(adopted.contains("# 2026\n\nIndex of 2026."), "{adopted}");
        assert!(
            adopted.contains("Agent instructions · yearly v1"),
            "{adopted}"
        );
        let archive_at = adopted.find("## Archive").expect("archive");
        let callout_at = adopted.find("> [!agent]").expect("callout");
        assert!(callout_at < archive_at, "Archive follows the callout");
    }

    #[test]
    fn daily_rewrite_preserves_quote_style_and_adds_week_nav() {
        let week = PeriodId::Week {
            year: 2026,
            week: 45,
        };
        let daily = "---\nparent: '[[2026/202611]]'\ntype: '[[day]]'\n---\n\n[[2026/20261101|prev]]  | [[2026/20261103|next]]\n\n# day\n";
        let (after, nav, skipped) = rewrite_daily(daily, week);
        assert!(!skipped);
        assert!(nav);
        assert!(after.contains("parent: '[[2026/2026-W45]]'"), "{after}");
        assert!(
            after.contains(
                "[[2026/20261101|prev]] · [[2026/2026-W45|W45]] · [[2026/20261103|next]]"
            ),
            "{after}"
        );
    }

    #[test]
    fn nonstandard_nav_is_skipped_and_parent_still_rewritten() {
        let week = PeriodId::Week {
            year: 2026,
            week: 1,
        };
        let daily = "---\nparent: \"[[2025/202512]]\"\n---\n\n[[yesterday]] | [[tomorrow]]\n";
        let (after, nav, skipped) = rewrite_daily(daily, week);
        assert!(skipped);
        assert!(!nav);
        assert!(after.contains("parent: \"[[2026/2026-W01]]\""), "{after}");
        assert!(after.contains("[[yesterday]] | [[tomorrow]]"), "{after}");
    }

    #[test]
    fn missing_parent_line_is_inserted_first_in_frontmatter() {
        let week = PeriodId::Week {
            year: 2026,
            week: 22,
        };
        let daily = "---\ntype: \"[[day]]\"\ndate: 2026-05-28\n---\n\n[[2026/20260527|prev]] | [[2026/20260528|day]] | [[2026/20260529|next]]\n";
        let (after, nav, skipped) = rewrite_daily(daily, week);
        assert!(skipped);
        assert!(!nav);
        assert!(
            after.contains("---\nparent: \"[[2026/2026-W22]]\"\ntype:"),
            "{after}"
        );
        assert_eq!(
            after.matches("parent:").count(),
            1,
            "exactly one parent line: {after}"
        );
    }

    #[test]
    fn out_of_range_year_only_rewrites_parent() {
        let original =
            "---\nparent: [[journal]]\nzorg_id: 2024\n---\n\n# 2024\n";
        let after = reparent_year_to_all_time(original).expect("changed");
        assert!(after.contains("parent: \"[[all_time]]\""), "{after}");
        assert!(after.contains("zorg_id: 2024"), "{after}");
        assert!(after.contains("# 2024"), "{after}");
        assert!(!after.contains("## Archive"), "{after}");
    }

    #[test]
    fn parity_compare_names_the_first_differing_day_and_metric() {
        let mut before = ParitySnapshot {
            days: BTreeMap::new(),
        };
        let mut after = ParitySnapshot {
            days: BTreeMap::new(),
        };
        let day = date(2026, 11, 2);
        before.days.insert(
            (day, "worked".to_string()),
            DayParity {
                value: Some(30),
                parts: BTreeMap::new(),
            },
        );
        after.days.insert(
            (day, "worked".to_string()),
            DayParity {
                value: Some(60),
                parts: BTreeMap::new(),
            },
        );
        let error = compare_parity(&before, &after).expect_err("differs");
        assert!(error.contains("2026-11-02"), "{error}");
        assert!(error.contains("worked"), "{error}");
    }

    #[test]
    fn fixture_vault_plans_the_expected_tree() {
        let dir = std::env::temp_dir()
            .join(format!("bob-periods-migrate-plan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("2026")).unwrap();
        fs::create_dir_all(dir.join("2025")).unwrap();
        fs::write(dir.join("2026.md"), zorg_year()).unwrap();
        fs::write(
            dir.join("2024.md"),
            "---\nparent: [[journal]]\n---\n\n# 2024\n",
        )
        .unwrap();
        fs::write(
            dir.join("2025.md"),
            "---\nparent: [[journal]]\n---\n\n# 2025\n",
        )
        .unwrap();
        fs::write(
            dir.join("2026/202611.md"),
            "---\nparent: [[2026]]\n---\n\n# November\n",
        )
        .unwrap();
        fs::write(
            dir.join("2026/20261102.md"),
            "---\nparent: '[[2026/202611]]'\n---\n\n[[2026/20261101|prev]]  | [[2026/20261103|next]]\n",
        )
        .unwrap();
        fs::write(
            dir.join("2026/20261103.md"),
            "---\nparent: '[[2026/202611]]'\n---\n\nsee you tomorrow\n",
        )
        .unwrap();
        fs::write(
            dir.join("2025/20251229.md"),
            "---\nparent: '[[2025/202512]]'\n---\n\n[[2025/20251228|prev]]  | [[2025/20251230|next]]\n",
        )
        .unwrap();
        let empty_week = dir.join("2026/2026-W44.md");
        fs::write(&empty_week, "   \n").unwrap();
        fs::write(
            dir.join("2026/2026-W43.md"),
            "Bryan already wrote this week.\n",
        )
        .unwrap();

        let today = date(2026, 11, 3);
        let since = date(2025, 12, 29);
        let plan = build_plan(&dir, since, today).expect("plan");
        assert!(
            plan.created.iter().any(|p| p == "all_time.md"),
            "{:?}",
            plan.created
        );
        assert!(
            plan.adopted.iter().any(|p| p == "2026.md"),
            "{:?}",
            plan.adopted
        );
        assert!(
            plan.reparented.iter().any(|p| p == "2024.md"),
            "{:?}",
            plan.reparented
        );
        assert!(
            plan.reparented.iter().any(|p| p == "2025.md"),
            "{:?}",
            plan.reparented
        );
        assert!(
            plan.filled.iter().any(|p| p == "2026/2026-W44.md"),
            "{:?}",
            plan.filled
        );
        assert!(
            plan.existing.iter().any(|p| p == "2026/2026-W43.md"),
            "{:?}",
            plan.existing
        );
        assert!(
            plan.rewritten.iter().any(|p| p == "2026/20261102.md"),
            "{:?}",
            plan.rewritten
        );
        assert!(
            plan.nav_skipped.iter().any(|p| p == "2026/20261103.md"),
            "{:?}",
            plan.nav_skipped
        );
        assert!(
            plan.rewritten.iter().any(|p| p == "2025/20251229.md"),
            "{:?}",
            plan.rewritten
        );
        assert!(
            !plan.files.iter().any(|f| f.path.contains("202611.md")),
            "monthly notes stay untouched"
        );
        let monthly = fs::read_to_string(dir.join("2026/202611.md")).unwrap();
        assert!(monthly.contains("# November"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn injected_attribution_change_restores_every_file() {
        use crate::native::env::TestEnvGuard;
        use crate::native::vault_rewrite::{self, ReadyPlan};

        let temp = tempfile::tempdir().expect("scratch");
        let vault = temp.path().join("vault");
        fs::create_dir_all(vault.join("2026")).unwrap();
        fs::create_dir_all(vault.join("2025")).unwrap();
        let settings =
            vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(&settings, "{\"taskFormat\":\"dataview\"}\n").unwrap();
        fs::write(vault.join("2026.md"), zorg_year()).unwrap();
        fs::write(
            vault.join("2026/20261102.md"),
            "---\nparent: '[[2026/202611]]'\n---\n\n[[2026/20261101|prev]]  | [[2026/20261103|next]]\n\n## Pomodoros\n- [x] (**08:00 - 08:25** [t:: 25m]) — PLAN\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&vault)
                .args(args)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        let lock = temp.path().join("inject.lock");
        let _guard = TestEnvGuard::set(&[(
            "BOB_VAULT_SYNC_LOCK_FILE",
            Some(lock.as_os_str()),
        )]);

        let today = date(2026, 11, 3);
        let since = date(2026, 11, 2);
        let plan = build_plan(&vault, since, today).expect("plan");
        assert!(!plan.files.is_empty());
        let before_year = fs::read_to_string(vault.join("2026.md")).unwrap();
        let before_daily =
            fs::read_to_string(vault.join("2026/20261102.md")).unwrap();
        let range_start = plan.range_start.unwrap();
        let range_end = plan.range_end.unwrap();
        let now = today.and_hms_opt(12, 0, 0).unwrap();
        let before_parity = capture_parity(&vault, range_start, range_end, now)
            .expect("before parity");
        let files = plan.files.clone();
        let daily_path = vault.join("2026/20261102.md");
        let vault_for_plan = vault.clone();
        let child_env = crate::native::ob::ChildEnv::new();
        let failure = vault_rewrite::run(
            &vault,
            &child_env,
            COMMAND_NAME,
            "period-note adoption",
            true,
            false,
            move || {
                let verify_daily = daily_path.clone();
                let verify_vault = vault_for_plan.clone();
                Ok(ReadyPlan {
                    files,
                    subject: "periods: inject".to_string(),
                    post_write: Box::new(|| Ok(Vec::new())),
                    verify: Box::new(move || {
                        let original =
                            fs::read_to_string(&verify_daily).unwrap();
                        let mut injected = original.clone();
                        injected.push_str(
                            "- [x] (**09:00 - 09:30** [t:: 30m]) — INJECT\n",
                        );
                        fs::write(&verify_daily, &injected).unwrap();
                        let result = verify_adoption(
                            &verify_vault,
                            range_start,
                            range_end,
                            now,
                            &before_parity,
                        );
                        // Put the planned after-image back so
                        // vault_rewrite's restore can match it.
                        fs::write(&verify_daily, original).unwrap();
                        result
                    }),
                    on_committed: Box::new(|| {}),
                    context: (),
                })
            },
        )
        .expect_err("inject must fail verify");
        assert_eq!(failure.code, "verify_failed");
        assert!(
            failure.message.contains("2026-11-02")
                || failure.message.contains("worked"),
            "{}",
            failure.message
        );
        assert_eq!(
            fs::read_to_string(vault.join("2026.md")).unwrap(),
            before_year,
            "year note restored"
        );
        assert_eq!(
            fs::read_to_string(vault.join("2026/20261102.md")).unwrap(),
            before_daily,
            "daily restored"
        );
        assert!(
            !vault.join("all_time.md").exists(),
            "failed create of all_time.md is removed"
        );
    }
}
