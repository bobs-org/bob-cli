//! Human and JSON reports for `bob gmail sync`.
//!
//! The JSON document is a superset of pull's schema 1: it keeps `ok`,
//! `dry_run`, `pulled`, `results`, and `warnings`, and adds `watch`,
//! `problems`, and `summary`. Consumers of scan and apply outcomes use
//! those structs directly rather than parsing this document.

use serde_json::{json, Value};

use super::file::{FilingFailure, FilingOutcome, FilingResult};
use super::scan::WatchScan;
use super::watch_apply::{WatchGoneOutcome, WatchThreadOutcome};
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
    pub(crate) watch: Vec<WatchThreadOutcome>,
    pub(crate) gone: Vec<WatchGoneOutcome>,
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
            watch: Vec::new(),
            gone: Vec::new(),
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
///
/// Counts come from typed filings, not the public status string:
/// `pulled` counts only `filed`/`would_file` rows (including filed
/// route-problem rows); fetch and planning failures never count. Watching
/// uses the apply boundary's revalidated snapshot so a task closed during
/// preparation leaves the summary, plus this run's newly filed and adopted
/// threads. Failed and deferred adoptions never count as newly enrolled
/// watch.
pub(crate) fn from_outcome(
    dry_run: bool,
    outcome: FilingOutcome,
    extra_results: Vec<SyncResult>,
    extra_warnings: Vec<String>,
    watchers: &WatchScan,
    account: &str,
    enrollment: &super::enroll::EnrollmentFold,
) -> SyncReport {
    // Membership source is explicit:
    // - the revalidated apply snapshot when apply ran;
    // - the projected membership in dry-run (also `applied`);
    // - otherwise the run's own scan.
    // Never fall back to an empty default when apply was skipped.
    let snapshot = if outcome.applied {
        &outcome.watch_snapshot
    } else {
        watchers
    };
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
    warnings.extend(outcome.warnings.clone());
    // Surface filing failure kind/message as body-free warnings.
    for failure in &outcome.failures {
        warnings.push(format!(
            "skipped Gmail thread {}: {}: {}",
            failure.key.thread_id(),
            failure.kind,
            failure.message
        ));
    }

    results.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));
    problems.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));

    // Typed durable filings only; fetch/planning failures never count.
    let pulled = outcome
        .results
        .iter()
        .filter(|row| matches!(row.status, "filed" | "would_file"))
        .count();
    let failed = outcome.failed
        || !outcome.failures.is_empty()
        || extra_failed
        || !outcome.deferred.is_empty();
    first_seen |= problems.iter().any(|problem| problem.first_seen);

    let watching = watching_count(&outcome, snapshot, account, enrollment);
    let awaiting = awaiting_count(&outcome, snapshot, account, enrollment);

    SyncReport {
        ok: !failed && !first_seen,
        dry_run,
        pulled,
        results,
        watch: outcome.watch_threads.clone(),
        gone: outcome.watch_gone.clone(),
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
    // A busy skip is quiet only when preparation recorded no failure.
    // Failures and deferrals already on the report still print (and keep
    // exit 1) even though apply wrote nothing.
    if report.busy && report.ok {
        return;
    }
    let styler = Styler::detect();
    if !args.quiet {
        println!("{}", styler.cyan(&header(report)));
        for row in &report.results {
            if let Some(line) = human_row(row, &report.problems) {
                println!("  {}", color_row(&styler, row.status, &line));
            }
        }
        for row in &report.watch {
            if row.action != "unchanged"
                && let Some(line) = watch_human_row(row)
            {
                println!("  {}", watch_color_row(&styler, row.action, &line));
            }
        }
        for row in &report.gone {
            if let Some(line) = gone_human_row(row) {
                println!("  {}", styler.yellow(&line));
            }
        }
    } else {
        for row in &report.results {
            if row.status == "problem"
                && let Some(line) = human_row(row, &report.problems)
            {
                println!("  {}", styler.yellow(&line));
            }
        }
        for row in &report.watch {
            if row.action == "deferred_running"
                && let Some(line) = watch_human_row(row)
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
        "watch": report.watch.iter().map(watch_json).collect::<Vec<_>>(),
        "problems": problems,
        "summary": {
            "watching": report.watching,
            "awaiting": report.awaiting,
        },
        "warnings": report.warnings,
    })
}

fn watch_json(row: &WatchThreadOutcome) -> Value {
    let tasks: Vec<Value> = row
        .tasks
        .iter()
        .map(|task| {
            json!({
                "path": task.path,
                "line": task.line,
                "block_id": task.block_id,
            })
        })
        .collect();
    let mut value = json!({
        "thread_id": row.key.thread_id(),
        "note": row.note,
        "tasks": tasks,
        "turn": row.turn,
        "turn_since": row.turn_since,
        "action": row.action,
    });
    if let Some(nudge_on) = &row.nudge_on {
        value["nudge_on"] = json!(nudge_on);
    }
    value
}

fn watch_human_row(row: &WatchThreadOutcome) -> Option<String> {
    let action = match row.action {
        "held" => "awaiting reply",
        "lifted" => "replied",
        "bounced" => "bounced",
        "resurfaced" => "resurfaced",
        "deferred_running" => "deferred (running task)",
        "refreshed" => "refreshed",
        _ => return None,
    };
    let who = row.who.as_deref().unwrap_or("Unknown sender");
    let title = row.title.as_deref().unwrap_or(row.key.thread_id());
    let note = row
        .note
        .as_deref()
        .map(|path| format!(" → {path}"))
        .unwrap_or_default();
    let nudge = row
        .nudge_on
        .as_deref()
        .map(|date| format!(" (nudge {date})"))
        .unwrap_or_default();
    Some(format!("✉ {action:<23} {who} — {title}{nudge}{note}"))
}

fn gone_human_row(row: &WatchGoneOutcome) -> Option<String> {
    let who = row.who.as_deref().unwrap_or("Unknown sender");
    let title = row.title.as_deref().unwrap_or(row.key.thread_id());
    let note = row
        .note
        .as_deref()
        .map(|path| format!(" → {path}"))
        .unwrap_or_default();
    Some(format!("⚠ gone                    {who} — {title}{note}"))
}

fn watch_color_row(styler: &Styler, action: &str, line: &str) -> String {
    match action {
        "lifted" => styler.blue(line),
        "bounced" | "deferred_running" => styler.yellow(line),
        _ => styler.green(line),
    }
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
    outcome: &FilingOutcome,
    watchers: &WatchScan,
    account: &str,
    enrollment: &super::enroll::EnrollmentFold,
) -> usize {
    // Distinct enrolled threads with open watchers, including this run's
    // successful and projected filings and adoptions. Failed or deferred
    // adoptions never count as newly enrolled watch, and fetch failures
    // with no durable row never count.
    let mut keys: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for key in watchers.threads().filter(|key| key.is_account(account)) {
        // The revalidated snapshot lists open watchers; only enrolled
        // threads count, and enrollment never expires.
        if enrollment.is_enrolled(key) {
            keys.insert(key.thread_id().to_string());
        }
    }
    for result in &outcome.results {
        if matches!(result.status, "filed" | "would_file" | "adopted") {
            // Status strings never imply membership: recovered closed
            // tasks, pending save/origin/enrollment, and deferred
            // adoptions stay out of the watching count.
            if !result.watch_member {
                continue;
            }
            if outcome.deferred.contains(&result.key) {
                continue;
            }
            keys.insert(result.key.thread_id().to_string());
        }
    }
    keys.len()
}

fn awaiting_count(
    outcome: &FilingOutcome,
    watchers: &WatchScan,
    account: &str,
    enrollment: &super::enroll::EnrollmentFold,
) -> usize {
    let watching: std::collections::BTreeSet<String> = {
        let mut keys: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for key in watchers.threads().filter(|key| key.is_account(account)) {
            if enrollment.is_enrolled(key) {
                keys.insert(key.thread_id().to_string());
            }
        }
        for result in &outcome.results {
            if matches!(result.status, "filed" | "would_file" | "adopted")
                && result.watch_member
                && !outcome.deferred.contains(&result.key)
            {
                keys.insert(result.key.thread_id().to_string());
            }
        }
        keys
    };
    let mut counted = std::collections::BTreeSet::new();
    for id in watching {
        let Some(key) = ThreadKey::new(account, &id) else {
            continue;
        };
        if has_awaiting(watchers, &key) {
            counted.insert(id);
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
    }) || report.watch.iter().any(|row| row.action != "unchanged")
        || !report.gone.is_empty()
    {
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

fn human_row(row: &SyncResult, problems: &[SyncProblem]) -> Option<String> {
    let filed_problem = (row.status == "problem")
        .then(|| {
            problems
                .iter()
                .find(|problem| problem.thread_id == row.thread_id)
        })
        .flatten();
    let action = match row.status {
        "filed" => "filed",
        "would_file" => "would file",
        "adopted" => "adopted",
        "problem" if filed_problem.is_some() => "route",
        "problem" => "failed",
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
    if let Some(problem) = filed_problem {
        // Filed route problems show the resolver's label, message, and
        // hint so Bryan can fix the label without opening JSON.
        let mut line = format!(
            "{glyph} {action:<10} {name} → {dest} — `{}` {}",
            problem.label, problem.message
        );
        if !problem.hint.is_empty() {
            line.push_str(" — ");
            line.push_str(&problem.hint);
        }
        return Some(line);
    }
    if row.status == "problem" {
        // Remote fetch or planning failures have no durable row; keep the
        // per-thread row and point at warnings for the body-free diagnostic.
        return Some(format!(
            "{glyph} {action:<10} {name} → {dest} (see warnings)"
        ));
    }
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
    use super::super::scan::{LinkPlacement, WatchTarget, WatchingTask};
    use super::super::watch_apply::{WatchGoneOutcome, WatchTaskRef};
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
            watch: Vec::new(),
            gone: Vec::new(),
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
    fn watch_json_and_human_rows_keep_the_typed_polling_contract() {
        let row = WatchThreadOutcome {
            key: ThreadKey::new("me@example.com", "aa01").unwrap(),
            note: Some("mail/2026/aa01.md".into()),
            title: Some("Lease renewal".into()),
            who: Some("Dana Whitfield".into()),
            tasks: vec![WatchTaskRef {
                path: "job.md".into(),
                line: 8,
                block_id: Some("follow-up".into()),
            }],
            turn: Some("theirs".into()),
            turn_since: Some("2026-11-05T09:00:00-05:00".into()),
            action: "held",
            nudge_on: Some("2026-11-06".into()),
        };
        let value = watch_json(&row);
        assert_eq!(value["thread_id"], "aa01");
        assert_eq!(value["note"], "mail/2026/aa01.md");
        assert_eq!(value["tasks"][0]["path"], "job.md");
        assert_eq!(value["tasks"][0]["line"], 8);
        assert_eq!(value["tasks"][0]["block_id"], "follow-up");
        assert_eq!(value["turn"], "theirs");
        assert_eq!(value["turn_since"], "2026-11-05T09:00:00-05:00");
        assert_eq!(value["action"], "held");
        assert_eq!(value["nudge_on"], "2026-11-06");

        for (action, expected) in [
            ("held", "awaiting reply"),
            ("lifted", "replied"),
            ("bounced", "bounced"),
            ("deferred_running", "deferred (running task)"),
        ] {
            let row = WatchThreadOutcome {
                action,
                ..row.clone()
            };
            let line = watch_human_row(&row).expect("visible watch action");
            assert!(line.contains(expected), "{line}");
            assert!(line.contains("Dana Whitfield"), "{line}");
            assert!(line.contains("Lease renewal"), "{line}");
        }

        let gone = gone_human_row(&WatchGoneOutcome {
            key: ThreadKey::new("me@example.com", "aa01").unwrap(),
            note: Some("mail/2026/aa01.md".into()),
            title: Some("Lease renewal".into()),
            who: Some("Dana Whitfield".into()),
        })
        .expect("gone warning row");
        assert!(gone.contains("gone"), "{gone}");
        assert!(gone.contains("Dana Whitfield — Lease renewal"), "{gone}");
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
        let filed_problem = SyncProblem {
            thread_id: "bb02".into(),
            note: Some("mail/2026/b.md".into()),
            label: "#/missing".into(),
            kind: "unknown_note".into(),
            message: "unknown note".into(),
            hint: "create the note".into(),
            first_seen: true,
        };
        let already = SyncResult {
            thread_id: "cc03".into(),
            note: Some("mail/2026/c.md".into()),
            link: None,
            status: "already_filed",
            task_file: Some("job.md".into()),
            route: Some("job".into()),
        };
        assert_eq!(human_row(&already, &[]), None);
        let line = human_row(&problem, std::slice::from_ref(&filed_problem))
            .expect("problem row");
        assert!(line.contains("route"), "{line}");
        assert!(line.contains("gmail_inbox.md"), "{line}");
        assert!(line.contains("#/missing"), "{line}");
        assert!(line.contains("unknown note"), "{line}");
        assert!(line.contains("create the note"), "{line}");
        let remote = SyncResult {
            thread_id: "dd04".into(),
            note: None,
            link: None,
            status: "problem",
            task_file: None,
            route: None,
        };
        let failed_line = human_row(&remote, &[]).expect("remote failure");
        assert!(failed_line.contains("failed"), "{failed_line}");
        assert!(failed_line.contains("see warnings"), "{failed_line}");
        let report = SyncReport {
            ok: true,
            dry_run: false,
            pulled: 1,
            results: vec![filed()],
            watch: Vec::new(),
            gone: Vec::new(),
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
            watch: Vec::new(),
            gone: Vec::new(),
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

    fn empty_fold() -> super::super::enroll::EnrollmentFold {
        let temp = tempfile::tempdir().unwrap();
        super::super::enroll::EnrollmentFold::load(temp.path()).unwrap()
    }

    fn filing_key(id: &str) -> ThreadKey {
        ThreadKey::new("me@example.com", id).unwrap()
    }

    fn filed_result(id: &str) -> FilingResult {
        FilingResult {
            key: filing_key(id),
            note: Some(format!("mail/2026/{id}.md")),
            link: Some(format!("[[mail/2026/{id}.md|✉️ Title · Who]]")),
            task_file: Some("job.md".into()),
            status: "filed",
            route: Some("job".into()),
            via: Some(super::super::enroll::FiledVia::Route),
            problem: None,
            first_seen: false,
            watch_member: true,
        }
    }

    #[test]
    fn permanent_fetch_failure_alone_counts_nothing_and_fails() {
        let fold = empty_fold();
        let outcome = FilingOutcome::default();
        let extra = vec![SyncResult {
            thread_id: "aa01".into(),
            note: None,
            link: None,
            status: "problem",
            task_file: None,
            route: None,
        }];
        let report = from_outcome(
            false,
            outcome,
            extra,
            vec!["skipped Gmail thread aa01: not_found".into()],
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.pulled, 0);
        assert_eq!(report.watching, 0);
        assert!(!report.ok);
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn mixed_success_and_failure_counts_only_durable_filings() {
        let fold = empty_fold();
        let mut outcome = FilingOutcome::default();
        outcome.results.push(filed_result("aa01"));
        outcome.failures.push(FilingFailure {
            key: filing_key("bb02"),
            kind: "not_found".into(),
            message: "gone".into(),
        });
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.pulled, 1);
        assert_eq!(report.watching, 1);
        assert!(!report.ok);
        assert!(report.warnings.iter().any(|w| w.contains("not_found")));
    }

    #[test]
    fn pending_journal_without_deferred_is_not_watching() {
        let fold = empty_fold();
        let mut pending = filed_result("aa01");
        pending.watch_member = false;
        let mut outcome = FilingOutcome::default();
        outcome.results.push(pending);
        outcome.failed = true;
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 0);
        assert_eq!(report.awaiting, 0);
        assert!(!report.ok);
    }

    #[test]
    fn deferred_failed_enrollment_is_not_newly_enrolled_watch() {
        let fold = empty_fold();
        let mut adopted = filed_result("aa01");
        adopted.status = "adopted";
        adopted.via = None;
        // A successful adoption counts as watch ...
        let mut outcome = FilingOutcome::default();
        outcome.results.push(adopted.clone());
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 1);
        assert!(report.ok);
        // ... but when its enrollment append failed, the key is deferred:
        // still reported as a failure, never as newly enrolled watch.
        let mut outcome = FilingOutcome::default();
        outcome.results.push(adopted);
        outcome.deferred.push(filing_key("aa01"));
        outcome.failed = true;
        outcome
            .warnings
            .push("could not append Gmail enrollment journal: test".into());
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            vec!["could not append Gmail enrollment journal: test".into()],
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 0);
        assert!(!report.ok);
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn filed_route_problem_counts_once_with_first_seen_failure() {
        let fold = empty_fold();
        let mut outcome = FilingOutcome::default();
        outcome.results.push(FilingResult {
            problem: Some(RouteProblem {
                kind:
                    crate::native::gmail::route::RouteProblemKind::UnknownNote,
                label: "#/missing".into(),
                message: "unknown note".into(),
                hint: "create the note".into(),
            }),
            first_seen: true,
            ..filed_result("aa01")
        });
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.pulled, 1);
        assert_eq!(report.problems.len(), 1);
        assert!(!report.ok);
        assert!(report.problems[0].first_seen);
    }

    #[test]
    fn dry_run_parity_counts_projected_filings() {
        let fold = empty_fold();
        let mut outcome = FilingOutcome::default();
        outcome.results.push(FilingResult {
            status: "would_file",
            ..filed_result("aa01")
        });
        let report = from_outcome(
            true,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(report.pulled, 1);
        assert_eq!(report.watching, 1);
        assert!(report.dry_run);
    }

    fn enrolled_fold_with(
        id: &str,
    ) -> (tempfile::TempDir, super::super::enroll::EnrollmentFold) {
        use std::fs;
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".bob/gmail");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("athena.jsonl"),
            format!(
                "{{\"v\":1,\"ts\":\"2026-11-01T09:00:00-04:00\",\"host\":\"athena\",\
                 \"kind\":\"gmail.filed\",\"value\":{{\"account\":\"me@example.com\",\
                 \"thread_id\":\"{id}\",\"note\":\"mail/2026/{id}.md\",\
                 \"task_file\":\"job.md\",\"via\":\"route\",\"route\":\"job\"}}}}\n"
            ),
        )
        .unwrap();
        let fold =
            super::super::enroll::EnrollmentFold::load(temp.path()).unwrap();
        (temp, fold)
    }

    fn scan_with(id: &str) -> WatchScan {
        WatchScan::from_tasks_for_test(vec![WatchingTask {
            path: "job.md".into(),
            line: 1,
            text: format!("- [ ] #task [[mail/2026/{id}.md]]"),
            block_id: None,
            status: ' ',
            targets: vec![WatchTarget {
                key: filing_key(id),
                note: format!("mail/2026/{id}.md"),
                placement: LinkPlacement::TaskLine,
            }],
        }])
    }

    #[test]
    fn skipped_apply_keeps_the_run_scan_when_fetches_were_deferred() {
        let (_temp, fold) = enrolled_fold_with("aa01");
        let scan = scan_with("aa01");
        let mut outcome = FilingOutcome::default();
        outcome.failed = true;
        outcome.deferred.push(filing_key("bb02"));
        // Apply skipped: empty snapshot must not zero watching/awaiting.
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            vec!["deferred remaining Gmail fetches".into()],
            &scan,
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 1);
        assert!(!report.ok);
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn watcher_closed_between_scan_and_apply_is_not_counted() {
        let (_temp, fold) = enrolled_fold_with("aa01");
        let initial = scan_with("aa01");
        let mut outcome = FilingOutcome::default();
        outcome.applied = true;
        outcome.watch_snapshot = WatchScan::default();
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &initial,
            "me@example.com",
            &fold,
        );
        assert_eq!(report.watching, 0);
        assert_eq!(report.awaiting, 0);
    }

    #[test]
    fn planning_failure_warning_includes_kind_and_message() {
        let fold = empty_fold();
        let mut outcome = FilingOutcome::default();
        outcome.failures.push(FilingFailure {
            key: filing_key("aa01"),
            kind: "note_changed".into(),
            message: "the adopted mail note or task disappeared during filing"
                .into(),
        });
        let report = from_outcome(
            false,
            outcome,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert!(
            report.warnings.iter().any(|warning| {
                warning.contains("note_changed")
                    && warning.contains("disappeared during filing")
            }),
            "{:?}",
            report.warnings
        );
        assert!(!report.ok);
    }

    #[test]
    fn dry_run_and_live_counts_match_on_the_same_projected_row() {
        let fold = empty_fold();
        let mut live = FilingOutcome::default();
        live.applied = true;
        live.results.push(filed_result("aa01"));
        let mut dry = FilingOutcome::default();
        dry.applied = true;
        dry.results.push(FilingResult {
            status: "would_file",
            ..filed_result("aa01")
        });
        let live_report = from_outcome(
            false,
            live,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        let dry_report = from_outcome(
            true,
            dry,
            Vec::new(),
            Vec::new(),
            &WatchScan::default(),
            "me@example.com",
            &fold,
        );
        assert_eq!(live_report.pulled, dry_report.pulled);
        assert_eq!(live_report.watching, dry_report.watching);
        assert_eq!(live_report.pulled, 1);
        assert!(dry_report.dry_run);
        assert!(!live_report.dry_run);
    }
}
