//! Human and JSON reports for `bob gmail sync`.
//!
//! The JSON document is a superset of pull's schema 1: it keeps `ok`,
//! `dry_run`, `pulled`, `results`, and `warnings`, and adds `watch`,
//! `problems`, and `summary`. This phase emits `watch: []`; later watch
//! work fills the actions. Consumers of scan and apply outcomes should
//! use those structs directly rather than parsing this document.

use serde_json::{json, Value};

use super::file::{FilingFailure, FilingOutcome, FilingResult};
use super::scan::WatchScan;
use super::ThreadKey;
use crate::native::env as bob_env;
use crate::native::gmail::cli::SyncArgs;
use crate::native::gmail::route::RouteProblem;
use crate::native::gmail::{save, ui};
use crate::native::style::Styler;

const AWAITING_TAG: &str = "#awaiting_reply";

/// One thread row in the JSON `results` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncResult {
    pub(crate) thread_id: String,
    pub(crate) note: Option<String>,
    pub(crate) link: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) task_file: Option<String>,
    pub(crate) route: Option<String>,
}

/// One route-problem row in the JSON `problems` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncProblem {
    pub(crate) thread_id: String,
    pub(crate) note: Option<String>,
    pub(crate) label: String,
    pub(crate) kind: String,
    pub(crate) message: String,
    pub(crate) hint: String,
    pub(crate) first_seen: bool,
}

/// The document `bob gmail sync` prints, plus the flags the printer needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncReport {
    pub(crate) ok: bool,
    pub(crate) dry_run: bool,
    pub(crate) pulled: usize,
    pub(crate) results: Vec<SyncResult>,
    pub(crate) problems: Vec<SyncProblem>,
    pub(crate) warnings: Vec<String>,
    pub(crate) watching: usize,
    pub(crate) awaiting: usize,
    pub(crate) first_seen: bool,
    /// Guard or vault lock was busy: exit 0 with no human output.
    pub(crate) busy: bool,
}

impl SyncReport {
    /// A quiet skip: no vault writes, no journal, exit 0.
    pub(crate) fn busy(dry_run: bool) -> Self {
        Self {
            ok: true,
            dry_run,
            pulled: 0,
            results: Vec::new(),
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 0,
            awaiting: 0,
            first_seen: false,
            busy: true,
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        if self.ok {
            0
        } else {
            1
        }
    }
}

/// Build the report from apply's outcome plus enrolled discovery rows.
pub(crate) fn from_outcome(
    dry_run: bool,
    outcome: FilingOutcome,
    extra_results: Vec<SyncResult>,
    extra_warnings: Vec<String>,
    watchers: &WatchScan,
    account: &str,
) -> SyncReport {
    let extra_failed = extra_results.iter().any(|row| row.status == "problem");
    let mut results: Vec<SyncResult> = extra_results;
    let mut seen: std::collections::BTreeSet<String> =
        results.iter().map(|row| row.thread_id.clone()).collect();
    let mut problems = Vec::new();
    let mut first_seen = false;
    for result in &outcome.results {
        if !seen.insert(result.key.thread_id().to_string()) {
            continue;
        }
        if let Some(problem) = &result.problem {
            problems.push(sync_problem(
                result.key.thread_id(),
                result.note.as_deref(),
                problem,
                result.first_seen,
            ));
            first_seen |= result.first_seen;
        }
        results.push(from_filing(result));
    }
    for failure in &outcome.failures {
        if !seen.insert(failure.key.thread_id().to_string()) {
            continue;
        }
        results.push(from_failure(failure));
    }

    let mut warnings = extra_warnings;
    warnings.extend(outcome.warnings);

    results.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));
    problems.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));

    let pulled = results
        .iter()
        .filter(|row| matches!(row.status, "filed" | "would_file" | "problem"))
        .count();
    let failed = outcome.failed || !outcome.failures.is_empty() || extra_failed;
    first_seen |= problems.iter().any(|problem| problem.first_seen);

    let watching = watching_count(&results, watchers, account);
    let awaiting = awaiting_count(watchers, account);

    SyncReport {
        ok: !failed && !first_seen,
        dry_run,
        pulled,
        results,
        problems,
        warnings,
        watching,
        awaiting,
        first_seen,
        busy: outcome.busy,
    }
}

pub(crate) fn print(args: &SyncArgs, report: &SyncReport) {
    if args.format.is_json() {
        println!("{}", json_document(report));
        ui::flush_stdout();
        return;
    }
    if report.busy {
        return;
    }
    let styler = Styler::detect();
    if !args.quiet {
        println!("{}", styler.cyan(&header(report)));
        for row in &report.results {
            if let Some(line) = human_row(row) {
                println!("  {}", color_row(&styler, row.status, &line));
            }
        }
    } else {
        for row in &report.results {
            if row.status == "problem"
                && let Some(line) = human_row(row)
            {
                println!("  {}", styler.yellow(&line));
            }
        }
    }
    for warning in &report.warnings {
        eprintln!("{}", styler.dim(warning));
    }
    ui::flush_stdout();
}

fn json_document(report: &SyncReport) -> Value {
    let results: Vec<Value> = report
        .results
        .iter()
        .map(|row| {
            json!({
                "thread_id": row.thread_id,
                "note": row.note,
                "link": row.link,
                "status": row.status,
                "task_file": row.task_file,
                "route": row.route,
            })
        })
        .collect();
    let problems: Vec<Value> = report
        .problems
        .iter()
        .map(|problem| {
            json!({
                "thread_id": problem.thread_id,
                "note": problem.note,
                "label": problem.label,
                "kind": problem.kind,
                "message": problem.message,
                "hint": problem.hint,
                "first_seen": problem.first_seen,
            })
        })
        .collect();
    json!({
        "schema_version": 1,
        "ok": report.ok,
        "dry_run": report.dry_run,
        "pulled": report.pulled,
        "results": results,
        "watch": [],
        "problems": problems,
        "summary": {
            "watching": report.watching,
            "awaiting": report.awaiting,
        },
        "warnings": report.warnings,
    })
}

fn from_filing(result: &FilingResult) -> SyncResult {
    let status = if result.problem.is_some() {
        "problem"
    } else {
        result.status
    };
    SyncResult {
        thread_id: result.key.thread_id().to_string(),
        note: result.note.clone(),
        link: result.link.clone(),
        status,
        task_file: result.task_file.clone(),
        route: result.route.clone(),
    }
}

fn from_failure(failure: &FilingFailure) -> SyncResult {
    SyncResult {
        thread_id: failure.key.thread_id().to_string(),
        note: None,
        link: None,
        status: "problem",
        task_file: None,
        route: None,
    }
}

fn sync_problem(
    thread_id: &str,
    note: Option<&str>,
    problem: &RouteProblem,
    first_seen: bool,
) -> SyncProblem {
    SyncProblem {
        thread_id: thread_id.to_string(),
        note: note.map(str::to_string),
        label: problem.label.clone(),
        kind: problem.kind.as_str().to_string(),
        message: problem.message.clone(),
        hint: problem.hint.clone(),
        first_seen,
    }
}

fn watching_count(
    results: &[SyncResult],
    watchers: &WatchScan,
    account: &str,
) -> usize {
    let mut keys: std::collections::BTreeSet<String> = watchers
        .threads()
        .filter(|key| key.is_account(account))
        .map(|key| key.thread_id().to_string())
        .collect();
    for row in results {
        if matches!(row.status, "filed" | "would_file" | "adopted" | "problem")
        {
            keys.insert(row.thread_id.clone());
        }
    }
    keys.len()
}

fn awaiting_count(watchers: &WatchScan, account: &str) -> usize {
    let mut counted = std::collections::BTreeSet::new();
    for key in watchers.threads().filter(|key| key.is_account(account)) {
        if has_awaiting(watchers, key) {
            counted.insert(key.thread_id().to_string());
        }
    }
    counted.len()
}

fn has_awaiting(watchers: &WatchScan, key: &ThreadKey) -> bool {
    watchers.watchers(key).any(|task| {
        task.text
            .split_whitespace()
            .any(|token| token == AWAITING_TAG)
    })
}

fn header(report: &SyncReport) -> String {
    let time = local_time();
    let watching = if report.watching == 1 {
        "1 thread".to_string()
    } else {
        format!("{} threads", report.watching)
    };
    with_nothing(
        report,
        format!(
            "✉ gmail sync · {time} · watching {watching} · {} awaiting reply",
            report.awaiting
        ),
    )
}

fn with_nothing(report: &SyncReport, header: String) -> String {
    if report.results.iter().any(|row| {
        matches!(row.status, "filed" | "would_file" | "adopted" | "problem")
    }) {
        header
    } else {
        format!("{header} · nothing new")
    }
}

fn local_time() -> String {
    let utc = save::current_utc();
    let tz = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    utc.with_timezone(&tz).format("%H:%M").to_string()
}

fn human_row(row: &SyncResult) -> Option<String> {
    let action = match row.status {
        "filed" => "filed",
        "would_file" => "would file",
        "adopted" => "adopted",
        "problem" => "route",
        _ => return None,
    };
    let name = display_name(row);
    let dest = row
        .task_file
        .as_deref()
        .or(row.note.as_deref())
        .unwrap_or("saved note unavailable");
    let glyph = match row.status {
        "adopted" => "⇢",
        "problem" => "⚠",
        _ => "+",
    };
    let extra = match row.status {
        "adopted" => " (existing task)",
        _ => "",
    };
    Some(format!("{glyph} {action:<10} {name} → {dest}{extra}"))
}

fn display_name(row: &SyncResult) -> String {
    if let Some(link) = &row.link
        && let Some((_, label)) = link.rsplit_once('|')
    {
        return label.trim_end_matches(']').to_string();
    }
    if let Some(link) = &row.link {
        return link.clone();
    }
    row.thread_id.clone()
}

fn color_row(styler: &Styler, status: &str, line: &str) -> String {
    match status {
        "problem" => styler.yellow(line),
        "adopted" => styler.blue(line),
        _ => styler.green(line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filed() -> SyncResult {
        SyncResult {
            thread_id: "aa01".into(),
            note: Some("mail/2026/a.md".into()),
            link: Some("[[mail/2026/a.md|✉️ Lease · Ada]]".into()),
            status: "filed",
            task_file: Some("job.md".into()),
            route: Some("job".into()),
        }
    }

    #[test]
    fn json_keeps_pull_fields_and_adds_watch_problems_summary() {
        let report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 1,
            results: vec![filed()],
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 1,
            awaiting: 0,
            first_seen: false,
            busy: false,
        };
        let value = json_document(&report);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], true);
        assert_eq!(value["pulled"], 1);
        assert_eq!(value["results"][0]["status"], "filed");
        assert_eq!(value["results"][0]["task_file"], "job.md");
        assert_eq!(value["results"][0]["route"], "job");
        assert!(value["watch"].as_array().unwrap().is_empty());
        assert_eq!(value["summary"]["watching"], 1);
        assert_eq!(value["summary"]["awaiting"], 0);
        assert!(value["problems"].as_array().unwrap().is_empty());
    }

    #[test]
    fn pulled_counts_route_problems_and_human_hides_already_rows() {
        let problem = SyncResult {
            thread_id: "bb02".into(),
            note: Some("mail/2026/b.md".into()),
            link: Some("[[mail/2026/b.md|✉️ Kickoff · Lee]]".into()),
            status: "problem",
            task_file: Some("gmail_inbox.md".into()),
            route: None,
        };
        let already = SyncResult {
            thread_id: "cc03".into(),
            note: Some("mail/2026/c.md".into()),
            link: None,
            status: "already_filed",
            task_file: Some("job.md".into()),
            route: Some("job".into()),
        };
        assert_eq!(human_row(&already), None);
        let line = human_row(&problem).expect("problem row");
        assert!(line.contains("route"), "{line}");
        assert!(line.contains("gmail_inbox.md"), "{line}");
        let report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 1,
            results: vec![filed()],
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 1,
            awaiting: 0,
            first_seen: false,
            busy: false,
        };
        let one = header(&report);
        assert!(one.contains("watching 1 thread ·"), "{one}");
        assert!(!one.contains("1 threads"), "{one}");
        let many = header(&SyncReport {
            watching: 2,
            ..report
        });
        assert!(many.contains("watching 2 threads ·"), "{many}");
    }

    #[test]
    fn nothing_new_when_only_already_enrolled() {
        let report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 0,
            results: vec![SyncResult {
                thread_id: "aa01".into(),
                note: None,
                link: None,
                status: "already_pulled",
                task_file: None,
                route: None,
            }],
            problems: Vec::new(),
            warnings: Vec::new(),
            watching: 0,
            awaiting: 0,
            first_seen: false,
            busy: false,
        };
        let text = with_nothing(&report, "header".into());
        assert!(text.ends_with("nothing new"), "{text}");
    }
}
