//! Read-only `bob freshness list` and the guarded cutover
//! `bob freshness seed`, as human output or JSON.
//!
//! The contract lives in `docs/freshness.md` §7. Exit codes: 0 on
//! success; 1 for I/O errors and seed refusals; 2 for an invalid
//! `freshness:` block or a non-Dataview task format.

use std::{ffi::OsString, fmt::Write as _, iter, path::PathBuf};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::json;

use super::super::{
    env as bob_env,
    style::{pad_right, Styler},
};
use super::{
    scan::{
        collect_warnings, refreshed_today, scan, RowCtx, ScanError, Snapshot,
        Warning,
    },
    seed::{run_seed, SeedError, SeedReport},
    state::{
        bucket_for_state, counts, evaluate, queue, Counts, Evaluated,
        FreshState,
    },
};

const COMMAND_NAME: &str = "bob freshness";

/// Bump only for a breaking change to the JSON objects below; new
/// optional fields keep the current version.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let argv: Vec<OsString> = iter::once(OsString::from(COMMAND_NAME))
        .chain(args)
        .collect();
    let first = argv.get(1).and_then(|arg| arg.to_str()).unwrap_or("");

    if first == "list" || first == "seed" {
        let mut command = build_cli();
        let matches = match command.try_get_matches_from_mut(argv) {
            Ok(matches) => matches,
            Err(error) => return print_clap_error(error),
        };
        return match matches.subcommand() {
            Some(("list", sub_matches)) => run_list(sub_matches),
            Some(("seed", sub_matches)) => run_seed_command(sub_matches),
            Some((name, _)) => {
                eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
                2
            }
            None => 2,
        };
    }

    if first == "--help" || first == "-h" {
        // Top-level help names both subcommands.
        let mut command = build_cli();
        return match command.try_get_matches_from_mut(argv) {
            Ok(_) => 0,
            Err(error) => print_clap_error(error),
        };
    }

    // Bare `bob freshness` is `list`: re-parse the trailing args as
    // list options so `bob freshness -f json` works.
    let mut command = list_command();
    let list_argv: Vec<OsString> =
        vec![OsString::from(COMMAND_NAME), OsString::from("list")]
            .into_iter()
            .chain(argv.into_iter().skip(1))
            .collect();
    match command.try_get_matches_from_mut(list_argv) {
        Ok(matches) => {
            let (_, sub_matches) = matches
                .subcommand()
                .expect("list argv parses as the list subcommand");
            run_list(sub_matches)
        }
        Err(error) => print_clap_error(error),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Review Ready tasks for freshness and seed the cutover")
        .long_about(
            "Review Ready tasks for freshness: list the tasks due for \
            review (never confirmed, resurfaced, or rotten) and stamp the \
            one-time cutover seed.\n\n\
            The list subcommand is read-only: it evaluates every visible, \
            non-recurring Ready task at read time — never stored — and \
            shows the NEW → DUE queue with counts. The seed subcommand \
            stamps every unstamped open task once: Ready tasks staggered \
            across the last 7 days by note, everything else today. The \
            seed refuses a second run, aborts on any parse change, and \
            refuses when a file changed since the scan. See \
            docs/freshness.md for the full definition.",
        )
        .after_help(
            "Examples:\n  bob freshness\n  bob freshness list -f json\n  bob freshness list --limit 10\n  bob freshness seed --dry-run\n  bob freshness seed --dry-run -f json",
        )
        .subcommand_required(false)
        .arg_required_else_help(false)
        .arg(bob_dir_arg())
        .subcommand(list_command_inner())
        .subcommand(seed_command())
}

fn list_command() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("List the tasks due for freshness review")
        .subcommand_required(true)
        .subcommand(list_command_inner())
}

fn list_command_inner() -> ClapCommand {
    ClapCommand::new("list")
        .about("List the tasks due for freshness review")
        .long_about(
            "List the tasks due for freshness review: every in-scope \
            Ready task in state NEW, RESURFACED, or ROTTEN, ordered NEW by \
            (path, line) then DUE by (due_on, path, line), with whole-vault \
            counts. The command is read-only. Counts always cover the \
            whole vault; --limit truncates the queue rows only. See \
            docs/freshness.md for the full definition.",
        )
        .after_help(
            "Examples:\n  bob freshness list\n  bob freshness list -f json\n  bob freshness list -b ~/bob --limit 10\n\nEnvironment:\n  BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  BOB_DAY_FILE              Daily note override; otherwise <bob-dir>/YYYY/YYYYMMDD.md\n  BOB_DIR                   Bob vault root when --bob-dir is omitted\n  BOB_NOW                   Local datetime override for review date selection\n  NO_COLOR                  Disable colored output",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(limit_arg())
}

fn seed_command() -> ClapCommand {
    ClapCommand::new("seed")
        .about("Stamp the one-time freshness cutover seed")
        .long_about(
            "Stamp the one-time freshness cutover seed: in-scope Ready \
            tasks without a valid fresh are bin-packed by note into 7 \
            buckets (bucket k lands on today − 7 + k, raised so nothing \
            is due on cutover day), and every other open, non-recurring \
            task gets today. The seed refuses when any task already \
            carries a fresh date before today (pass --force to override; \
            seeding after cutover would mark every capture since then as \
            reviewed), aborts with no writes when any changed line would \
            parse differently, and refuses when a file changed since the \
            scan. A same-day rerun is an idempotent no-op. See \
            docs/freshness.md for the full definition.",
        )
        .after_help(
            "Examples:\n  bob freshness seed --dry-run\n  bob freshness seed --dry-run -f json\n  bob freshness seed\n\nEnvironment:\n  BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n  BOB_DAY_FILE              Daily note override; otherwise <bob-dir>/YYYY/YYYYMMDD.md\n  BOB_DIR                   Bob vault root when --bob-dir is omitted\n  BOB_NOW                   Local datetime override for review date selection\n  NO_COLOR                  Disable colored output",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(force_arg())
        .arg(format_arg())
        .arg(help_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn limit_arg() -> Arg {
    Arg::new("limit")
        .long("limit")
        .short('l')
        .value_name("N")
        .value_parser(clap::value_parser!(usize))
        .help("Show at most N queue rows; counts always cover the vault")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview the seed without writing files")
}

fn force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .short('F')
        .action(ArgAction::SetTrue)
        .help("Seed even when stamps dated before today exist")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
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

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

/// One evaluated queue row with everything both outputs need.
struct ListedRow {
    rank: u32,
    tier: &'static str,
    state: FreshState,
    /// Stable read-time bucket (`new`, `rotten`, or null); schema 1
    /// keeps the machine `state`/`counts` names, so this additive
    /// field is how dashboards gate without a schema bump.
    bucket: Option<&'static str>,
    path: String,
    line: u32,
    block_id: Option<String>,
    status_symbol: String,
    text: String,
    created: Option<String>,
    fresh: Option<String>,
    interval: u16,
    interval_source: String,
    due_on: Option<String>,
    days_overdue: Option<i64>,
    scheduled: Option<String>,
}

struct ListReport {
    date: String,
    weekday: String,
    interval: u16,
    budget: Option<u32>,
    counts: Counts,
    rows: Vec<ListedRow>,
    warnings: Vec<Warning>,
}

fn collect_list(snapshot: &Snapshot) -> ListReport {
    let today = snapshot.today;
    let config = &snapshot.config;
    // The engine applied `is not blocked` to every READY row; the
    // field stays on RichTask for the seed's contract.
    debug_assert!(
        snapshot.ready.iter().all(|row| !row.task.is_blocked),
        "READY_QUERY rows must never be blocked"
    );
    let ready_rows: Vec<_> = snapshot
        .ready
        .iter()
        .map(|row| (row, row.freshness_row(true)))
        .collect();

    let queue_entries = queue(
        &ready_rows
            .iter()
            .map(|(_, row)| row.clone())
            .collect::<Vec<_>>(),
        today,
        config,
    );
    let evaluated: std::collections::HashMap<
        (String, u32),
        (RowCtx, Evaluated),
    > = ready_rows
        .into_iter()
        .map(|(ctx, row)| {
            let evaluated = evaluate(&row, today, config);
            (
                (ctx.task.path.clone(), ctx.task.line),
                (ctx.clone(), evaluated),
            )
        })
        .collect();

    let mut rows = Vec::with_capacity(queue_entries.len());
    for entry in &queue_entries {
        if let Some((ctx, evaluated)) =
            evaluated.get(&(entry.path.clone(), entry.line))
        {
            rows.push(ListedRow {
                rank: entry.rank,
                tier: entry.tier,
                state: entry.state,
                bucket: bucket_for_state(Some(entry.state)),
                path: entry.path.clone(),
                line: entry.line,
                block_id: ctx.task.block_id.clone(),
                status_symbol: ctx.task.status_symbol.clone(),
                text: ctx.task.text.clone(),
                created: ctx
                    .task
                    .created
                    .map(|date| date.format("%Y-%m-%d").to_string()),
                fresh: evaluated
                    .fresh
                    .map(|date| date.format("%Y-%m-%d").to_string()),
                interval: evaluated.interval_days,
                interval_source: evaluated.interval_source.as_str().to_string(),
                due_on: evaluated
                    .due_on
                    .map(|date| date.format("%Y-%m-%d").to_string()),
                days_overdue: evaluated.days_overdue,
                scheduled: ctx
                    .task
                    .scheduled
                    .map(|date| date.format("%Y-%m-%d").to_string()),
            });
        }
    }

    let ready_eval_rows: Vec<_> = snapshot
        .ready
        .iter()
        .map(|row| row.freshness_row(true))
        .collect();
    let mut counts = counts(&ready_eval_rows, today, config);
    // `counts` over Ready rows cannot see Next, Pending, or done
    // tasks; refreshed_today needs every status (S15).
    counts.refreshed_today = refreshed_today(&snapshot.all, today);
    counts.budget_met = config
        .stale_daily_budget
        .is_some_and(|goal| counts.refreshed_today >= goal && counts.new == 0);

    let mut warnings = collect_warnings(&snapshot.all, today, config);
    warnings.extend(snapshot.today_warnings.clone());

    ListReport {
        date: today.format("%Y-%m-%d").to_string(),
        weekday: snapshot.weekday.clone(),
        interval: config.interval,
        budget: config.stale_daily_budget,
        counts,
        rows,
        warnings,
    }
}

fn run_list(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let format = OutputFormat::from_matches(matches);
    let limit = matches.get_one::<usize>("limit").copied();

    let snapshot = match scan(&bob_dir) {
        Ok(snapshot) => snapshot,
        Err(error) => return print_scan_error(&error, format),
    };
    let mut report = collect_list(&snapshot);
    if let Some(limit) = limit {
        report.rows.truncate(limit);
    }

    match format {
        OutputFormat::Human => {
            let styler = Styler::detect();
            print!("{}", human_list(&report, &styler));
            0
        }
        OutputFormat::Json => {
            println!("{}", json_list(&report));
            0
        }
    }
}

fn today_meter(report: &ListReport) -> String {
    match report.budget {
        Some(goal) => {
            format!("✓ {}/{} today", report.counts.refreshed_today, goal)
        }
        None => format!("✓ {} today", report.counts.refreshed_today),
    }
}

fn human_list(report: &ListReport, styler: &Styler) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "bob freshness {sep} {weekday} {date} {sep} every {interval}d",
        sep = styler.separator(),
        weekday = report.weekday,
        date = report.date,
        interval = report.interval,
    );
    output.push('\n');
    let _ = writeln!(
        output,
        "  REVIEW {due} due {sep} {new} new {sep} {resurfaced} resurfaced {sep} {stale} rotten {sep} {today}",
        due = report.counts.due,
        sep = styler.separator(),
        new = report.counts.new,
        resurfaced = report.counts.resurfaced,
        stale = report.counts.stale,
        today = today_meter(report),
    );

    let new_rows: Vec<&ListedRow> =
        report.rows.iter().filter(|row| row.tier == "new").collect();
    let due_rows: Vec<&ListedRow> =
        report.rows.iter().filter(|row| row.tier != "new").collect();
    if !new_rows.is_empty() {
        output.push_str(&format!("\n  {}\n", styler.yellow("NEW")));
        for row in new_rows {
            output.push_str(&human_row(row, styler));
        }
    }
    if !due_rows.is_empty() {
        output.push_str(&format!("\n  {}\n", styler.yellow("DUE")));
        for row in due_rows {
            output.push_str(&human_row(row, styler));
        }
    }
    if !report.warnings.is_empty() {
        output.push_str(&format!("\n  {}\n", styler.yellow("LINTS")));
        for warning in &report.warnings {
            let _ = writeln!(
                output,
                "    {}:{} {} {}",
                warning.path,
                warning
                    .line
                    .map(|line| line.to_string())
                    .unwrap_or_default(),
                styler.cyan(&warning.code),
                warning.message,
            );
        }
    }
    output
}

fn human_row(row: &ListedRow, styler: &Styler) -> String {
    let reference = format!("{}:{}", row.path, row.line);
    let mut detail = match row.state {
        FreshState::New => match &row.created {
            Some(created) => format!("created {created}"),
            None => "never confirmed".to_string(),
        },
        FreshState::Resurfaced => format!(
            "resurfaced {sep} scheduled {}",
            row.scheduled.as_deref().unwrap_or("?"),
            sep = styler.separator(),
        ),
        FreshState::Stale => match row.days_overdue {
            Some(0) => "due today".to_string(),
            Some(days) => format!("rotten {days}d"),
            None => "rotten".to_string(),
        },
        FreshState::Fresh => "fresh".to_string(),
    };
    if row.state != FreshState::New
        && let Some(fresh) = &row.fresh
    {
        detail.push_str(&format!(
            " {sep} fresh {fresh}",
            sep = styler.separator()
        ));
    }
    if row.state == FreshState::Stale || row.state == FreshState::Fresh {
        detail.push_str(&format!(
            " {sep} every {interval}d ({source})",
            sep = styler.separator(),
            interval = row.interval,
            source = row.interval_source,
        ));
    }
    format!(
        "    {}  {}  {}\n",
        styler.cyan(&pad_right(&reference, 30)),
        pad_right(&row.text, 40),
        styler.dim(&detail),
    )
}

fn json_list(report: &ListReport) -> serde_json::Value {
    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "date": report.date,
        "config": {
            "interval": report.interval,
            "stale_daily_budget": report.budget,
        },
        "counts": {
            "due": report.counts.due,
            "new": report.counts.new,
            "resurfaced": report.counts.resurfaced,
            "stale": report.counts.stale,
            "fresh": report.counts.fresh,
            "refreshed_today": report.counts.refreshed_today,
            "budget": report.counts.budget,
            "budget_met": report.counts.budget_met,
        },
        "queue": report.rows.iter().map(|row| json!({
            "rank": row.rank,
            "tier": row.tier,
            "state": row.state.as_str(),
            "bucket": row.bucket,
            "path": row.path,
            "line": row.line,
            "block_id": row.block_id,
            "status_symbol": row.status_symbol,
            "text": row.text,
            "created": row.created,
            "fresh": row.fresh,
            "interval": row.interval,
            "interval_source": row.interval_source,
            "due_on": row.due_on,
            "days_overdue": row.days_overdue,
        })).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(|warning| json!({
            "code": warning.code,
            "path": warning.path,
            "line": warning.line,
            "message": warning.message,
        })).collect::<Vec<_>>(),
    })
}

fn run_seed_command(matches: &ArgMatches) -> i32 {
    let bob_dir = bob_dir_from_matches(matches);
    let format = OutputFormat::from_matches(matches);
    let dry_run = matches.get_flag("dry-run");
    let force = matches.get_flag("force");

    let snapshot = match scan(&bob_dir) {
        Ok(snapshot) => snapshot,
        Err(error) => return print_scan_error(&error, format),
    };
    let warnings = {
        let mut warnings =
            collect_warnings(&snapshot.all, snapshot.today, &snapshot.config);
        warnings.extend(snapshot.today_warnings.clone());
        warnings
    };
    match run_seed(&snapshot, &bob_dir, dry_run, force) {
        Ok(report) => {
            match format {
                OutputFormat::Human => {
                    let styler = Styler::detect();
                    print!(
                        "{}",
                        human_seed(&snapshot, &report, &warnings, &styler)
                    );
                }
                OutputFormat::Json => {
                    println!("{}", json_seed(&report, &warnings));
                }
            }
            0
        }
        Err(error) => print_seed_error(&error, format),
    }
}

fn human_seed(
    snapshot: &Snapshot,
    report: &SeedReport,
    warnings: &[Warning],
    styler: &Styler,
) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "bob freshness seed {sep} {weekday} {date} {sep} every {interval}d",
        sep = styler.separator(),
        weekday = snapshot.weekday,
        date = report.date,
        interval = snapshot.config.interval,
    );
    output.push('\n');
    if report.dry_run {
        output.push_str("  dry run: no files written\n");
    }
    let _ = writeln!(
        output,
        "  stamped {ready} ready {sep} {other} other {sep} skipped {stamped} already stamped {sep} {recurring} recurring {sep} {scope} out of scope",
        ready = report.stamped_ready,
        sep = styler.separator(),
        other = report.stamped_other,
        stamped = report.skipped_already_stamped,
        recurring = report.skipped_recurring,
        scope = report.skipped_out_of_scope,
    );
    output.push_str("  buckets:\n");
    for bucket in &report.buckets {
        let _ = writeln!(
            output,
            "    {} → due {} {sep} {} tasks {sep} {} notes",
            bucket.fresh,
            bucket.due_on,
            bucket.count,
            bucket.notes.len(),
            sep = styler.separator(),
        );
        let mut notes: Vec<(&String, &u32)> = bucket.notes.iter().collect();
        notes.sort();
        for (path, count) in notes {
            let _ = writeln!(output, "      {path} ×{count}");
        }
    }
    if report.files.is_empty() {
        output.push_str("  files: none\n");
    } else {
        let _ = writeln!(output, "  files ({}):", report.files.len());
        for path in &report.files {
            let _ = writeln!(output, "    {path}");
        }
    }
    if !warnings.is_empty() {
        output.push_str(&format!("\n  {}\n", styler.yellow("LINTS")));
        for warning in warnings {
            let _ = writeln!(
                output,
                "    {}:{} {} {}",
                warning.path,
                warning
                    .line
                    .map(|line| line.to_string())
                    .unwrap_or_default(),
                styler.cyan(&warning.code),
                warning.message,
            );
        }
    }
    output
}

fn json_seed(report: &SeedReport, warnings: &[Warning]) -> serde_json::Value {
    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "date": report.date,
        "dry_run": report.dry_run,
        "stamped": {
            "ready": report.stamped_ready,
            "other": report.stamped_other,
        },
        "buckets": report.buckets,
        "skipped": {
            "already_stamped": report.skipped_already_stamped,
            "recurring": report.skipped_recurring,
            "out_of_scope": report.skipped_out_of_scope,
        },
        "files": report.files,
        "warnings": warnings.iter().map(|warning| json!({
            "code": warning.code,
            "path": warning.path,
            "line": warning.line,
            "message": warning.message,
        })).collect::<Vec<_>>(),
    })
}

fn print_scan_error(error: &ScanError, format: OutputFormat) -> i32 {
    let message = error.message().to_string();
    let exit_code = error.exit_code();
    match format {
        OutputFormat::Human => {
            eprintln!("bob freshness: {message}");
        }
        OutputFormat::Json => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "schema_version": SCHEMA_VERSION,
                    "error": message,
                })
            );
        }
    }
    exit_code
}

fn print_seed_error(error: &SeedError, format: OutputFormat) -> i32 {
    let message = error.message();
    match format {
        OutputFormat::Human => {
            eprintln!("{message}");
        }
        OutputFormat::Json => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "schema_version": SCHEMA_VERSION,
                    "error": message,
                })
            );
        }
    }
    1
}
