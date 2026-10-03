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
    config::freshness::{decay_active, decay_active_from},
    env as bob_env,
    style::{pad_right, Styler},
};
use super::{
    scan::{
        collect_warnings, lint_message, refreshed_today, scan, upkeep_today,
        RowCtx, ScanError, Snapshot, Warning,
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
///
/// Schema 5 adds project/reference tracking review: the `projects`
/// walk tier, `projects_due` and the six-key `by_tier` histogram in
/// counts (with `walk = sum(by_tier)`), and decoupled state totals
/// (`due = new + resurfaced + rotten` over Ready states, including
/// eligible Ready trackers). Schema 4 added the keep-streak contract.
/// The seed envelope shares this constant; seed contents are
/// otherwise unchanged.
const SCHEMA_VERSION: u32 = 5;

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

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Walk the tiered freshness review queue and seed the cutover")
        .long_about(
            "Walk the tiered freshness review queue: list the tasks due \
            for review in tier order NEW → PROJECTS → PENDING → NEXT → \
            RETURNED → ROTTEN and stamp the one-time cutover seed.\n\n\
            The list subcommand is read-only: it evaluates every visible, \
            non-recurring Ready, Pending, and Next task at read time — \
            never stored — and shows the tiered walk queue with counts. \
            Pending and Next tasks come due for a daily review set by \
            freshness.pending_interval / next_interval. The seed \
            subcommand stamps every unstamped open task once: Ready tasks \
            staggered across the last 7 days by note, everything else \
            today. The seed refuses a second run, aborts on any parse \
            change, and refuses when a file changed since the scan. See \
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
        .about("List the tiered freshness review queue")
        .subcommand_required(true)
        .subcommand(list_command_inner())
}

fn list_command_inner() -> ClapCommand {
    ClapCommand::new("list")
        .about("List the tiered freshness review queue")
        .long_about(
            "List the tiered freshness review queue: every task with a \
            walk tier, ordered NEW → PROJECTS → PENDING → NEXT → \
            RETURNED → ROTTEN with each tier's comparator, with \
            whole-vault counts. The command is read-only. Counts always \
            cover the whole vault; --limit truncates the queue rows \
            only. See docs/freshness.md for the full definition.",
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

/// Completion-only entry: the runtime `build_cli()` plus the default
/// `list` options mounted on the parent so bare
/// `bob freshness [list options]` parses for completion.
pub(crate) fn completion_command() -> ClapCommand {
    build_cli()
        .disable_help_flag(true)
        .arg(format_arg())
        .arg(limit_arg())
        .arg(help_arg())
        .args_conflicts_with_subcommands(true)
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
    tier: String,
    lane: String,
    state: Option<FreshState>,
    /// Stable read-time bucket (`new`, `rotten`, or null) for
    /// dashboard gating. Lane rows carry null.
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
    /// The valid `[keeps:: N]` semantic count (0 when absent).
    keeps: u32,
    /// A choice is due for this row — not permission to act.
    decide: bool,
}

struct ListReport {
    date: String,
    weekday: String,
    interval: u16,
    pending_interval: Option<u16>,
    next_interval: Option<u16>,
    budget: Option<u32>,
    decay_enabled: bool,
    decay_keeps: u16,
    decay_enter: Option<String>,
    decay_active: bool,
    decay_active_from: String,
    counts: Counts,
    rows: Vec<ListedRow>,
    warnings: Vec<Warning>,
}

fn collect_list(snapshot: &Snapshot) -> ListReport {
    let today = snapshot.today;
    let config = &snapshot.config;
    // The engine applied `is not blocked` to every lane query row;
    // the field stays on RichTask for the seed's contract.
    debug_assert!(
        snapshot.ready.iter().all(|row| !row.task.is_blocked),
        "READY_QUERY rows must never be blocked"
    );
    // The review input is ready ∪ pending ∪ next plus hidden
    // tracker candidates (freshness-specific visibility, hide
    // allowed). Ordinary hidden tasks never enter here.
    let combined: Vec<(&RowCtx, super::state::FreshnessRow)> = snapshot
        .ready
        .iter()
        .chain(snapshot.pending.iter())
        .chain(snapshot.next.iter())
        .map(|row| (row, row.freshness_row(true)))
        .chain(
            snapshot
                .trackers
                .iter()
                .map(|row| (row, row.freshness_row(true))),
        )
        .collect();

    let queue_entries = queue(
        &combined
            .iter()
            .map(|(_, row)| row.clone())
            .collect::<Vec<_>>(),
        today,
        config,
    );
    // Look rows up by (path, line): the same task cannot appear in
    // two lane queries (status symbols are disjoint), so the first
    // write wins.
    let mut evaluated: std::collections::HashMap<
        (String, u32),
        (RowCtx, Evaluated),
    > = std::collections::HashMap::new();
    for (ctx, row) in &combined {
        let key = (ctx.task.path.clone(), ctx.task.line);
        if evaluated.contains_key(&key) {
            continue;
        }
        let result = evaluate(row, today, config);
        evaluated.insert(key, ((*ctx).clone(), result));
    }

    let mut rows = Vec::with_capacity(queue_entries.len());
    for entry in &queue_entries {
        if let Some((ctx, evaluated)) =
            evaluated.get(&(entry.path.clone(), entry.line))
        {
            rows.push(ListedRow {
                rank: entry.rank,
                tier: entry.tier.as_str().to_string(),
                lane: entry.lane.as_str().to_string(),
                state: entry.state,
                bucket: bucket_for_state(entry.state),
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
                keeps: evaluated.keeps,
                decide: evaluated.decide,
            });
        }
    }

    let combined_eval_rows: Vec<_> = snapshot
        .ready
        .iter()
        .chain(snapshot.pending.iter())
        .chain(snapshot.next.iter())
        .map(|row| row.freshness_row(true))
        .chain(snapshot.trackers.iter().map(|row| row.freshness_row(true)))
        .collect();
    let mut counts = counts(&combined_eval_rows, today, config);
    // Tier counts need ready ∪ pending ∪ next plus hidden tracker
    // rows; the meters need every status (S15, B1).
    counts.refreshed_today = refreshed_today(&snapshot.all, today);
    counts.upkeep_today = upkeep_today(&snapshot.all, today);
    counts.budget_met = config
        .rotten_daily_budget
        .is_some_and(|goal| counts.upkeep_today >= goal && counts.new == 0);

    let mut warnings = collect_warnings(&snapshot.all, today, config);
    // One deprecation diagnostic per loaded config — never one per
    // task — when the removed `stale_daily_budget` key supplied the
    // budget or was ignored beside the canonical key.
    if config.stale_budget_deprecated {
        warnings.push(Warning {
            code: "freshness_stale_daily_budget_deprecated".to_string(),
            path: super::super::config::config_path().display().to_string(),
            line: None,
            message: lint_message("freshness_stale_daily_budget_deprecated"),
        });
    }
    warnings.extend(snapshot.today_warnings.clone());

    ListReport {
        date: today.format("%Y-%m-%d").to_string(),
        weekday: snapshot.weekday.clone(),
        interval: config.interval,
        pending_interval: config.pending_interval,
        next_interval: config.next_interval,
        budget: config.rotten_daily_budget,
        decay_enabled: config.decay.enabled,
        decay_keeps: config.decay.keeps,
        decay_enter: config.decay.enter.clone(),
        decay_active: decay_active(today),
        decay_active_from: decay_active_from().format("%Y-%m-%d").to_string(),
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
            format!("✓ {}/{} today", report.counts.upkeep_today, goal)
        }
        None => format!("✓ {} today", report.counts.upkeep_today),
    }
}

fn lane_meter(interval: Option<u16>) -> String {
    match interval {
        Some(days) => format!("{days}d"),
        None => "off".to_string(),
    }
}

/// The keep-streak threshold segment of the human header: the
/// threshold always shows; the off state and the pre-activation date
/// say so explicitly instead of promising a decision.
fn decay_meter(report: &ListReport) -> String {
    if !report.decay_enabled {
        format!("keeps {} · decay off", report.decay_keeps)
    } else if !report.decay_active {
        format!(
            "keeps {} · asks from {}",
            report.decay_keeps, report.decay_active_from
        )
    } else if report.decay_keeps == 0 {
        "keeps 0 · asks every review".to_string()
    } else {
        format!("keeps {}", report.decay_keeps)
    }
}

fn human_list(report: &ListReport, styler: &Styler) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "bob freshness {sep} {weekday} {date} {sep} every {interval}d {sep} pending {pending} {sep} next {next} {sep} {decay}",
        sep = styler.separator(),
        weekday = report.weekday,
        date = report.date,
        interval = report.interval,
        pending = lane_meter(report.pending_interval),
        next = lane_meter(report.next_interval),
        decay = decay_meter(report),
    );
    output.push('\n');
    let _ = writeln!(
        output,
        "  REVIEW {walk} due {sep} {new} new {sep} {projects} projects {sep} {pending} pending {sep} {next} next {sep} {returned} returned {sep} {rotten} rotten {sep} {today}",
        walk = report.counts.walk,
        sep = styler.separator(),
        new = report.counts.new,
        projects = report.counts.projects_due,
        pending = report.counts.pending_due,
        next = report.counts.next_due,
        returned = report.counts.resurfaced,
        rotten = report.counts.rotten,
        today = today_meter(report),
    );

    let tiers: [(&str, &str); 6] = [
        ("new", "NEW"),
        ("projects", "PROJECTS"),
        ("pending", "PENDING"),
        ("next", "NEXT"),
        ("returned", "RETURNED"),
        ("rotten", "ROTTEN"),
    ];
    let mut commitment_rows = 0;
    let mut rotten_rows = 0;
    for (tier, heading) in &tiers {
        let tier_rows: Vec<&ListedRow> =
            report.rows.iter().filter(|row| row.tier == *tier).collect();
        if tier_rows.is_empty() {
            continue;
        }
        // The divider splits commitments from upkeep: it sits
        // between the last commitment tier and the ROTTEN heading,
        // only when rows exist on both sides.
        if *tier == "rotten" && commitment_rows > 0 {
            output.push_str(&format!(
                "\n  {}\n",
                styler.dim("── commitments done above · upkeep below ──"),
            ));
        }
        if *tier == "rotten" {
            rotten_rows = tier_rows.len();
        } else {
            commitment_rows += tier_rows.len();
        }
        output.push_str(&format!(
            "\n  {} {}\n",
            styler.yellow(heading),
            tier_rows.len()
        ));
        for row in tier_rows {
            output.push_str(&human_row(row, styler));
        }
    }
    let _ = (commitment_rows, rotten_rows);
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
    let sep = styler.separator();
    let every = format!(
        "{sep} every {interval}d ({source})",
        interval = row.interval,
        source = row.interval_source,
    );
    let detail = match row.tier.as_str() {
        "new" => match &row.created {
            Some(created) => format!("created {created}"),
            None => "never confirmed".to_string(),
        },
        "projects" => match &row.fresh {
            None => match &row.created {
                Some(created) => {
                    format!(
                        "no Ready tasks in this project {sep} never confirmed {sep} created {created}"
                    )
                }
                None => "no Ready tasks in this project · never confirmed"
                    .to_string(),
            },
            Some(fresh) => {
                let lead = match row.days_overdue {
                    Some(0) => "due today".to_string(),
                    Some(days) => format!("{days}d overdue"),
                    None => match &row.due_on {
                        Some(due) => format!("due {due}"),
                        None => "due".to_string(),
                    },
                };
                format!(
                    "no Ready tasks in this project {sep} {lead} {sep} fresh {fresh}{every}"
                )
            }
        },
        "pending" | "next" => match &row.fresh {
            None => match &row.created {
                Some(created) => {
                    format!("never confirmed {sep} created {created}")
                }
                None => "never confirmed".to_string(),
            },
            Some(fresh) => {
                let lead = match row.days_overdue {
                    Some(0) => "due today".to_string(),
                    Some(days) => format!("{days}d overdue"),
                    None => match &row.due_on {
                        Some(due) => format!("due {due}"),
                        None => "due".to_string(),
                    },
                };
                format!("{lead} {sep} fresh {fresh}{every}")
            }
        },
        "returned" => format!(
            "returned {sep} scheduled {scheduled} {sep} fresh {fresh}",
            scheduled = row.scheduled.as_deref().unwrap_or("?"),
            fresh = row.fresh.as_deref().unwrap_or("?"),
        ),
        _ => {
            let lead = match row.days_overdue {
                Some(0) => "due today".to_string(),
                Some(days) => format!("rotten {days}d"),
                None => "rotten".to_string(),
            };
            match &row.fresh {
                Some(fresh) => format!("{lead} {sep} fresh {fresh}{every}"),
                None => lead,
            }
        }
    };
    // CLI human rows show `kept N×` and `· decide` where true; the
    // header carries the threshold, off state, or pre-activation date.
    let mut detail = detail;
    if row.keeps > 0 {
        detail.push_str(&format!(" {sep} kept {}×", row.keeps));
    }
    if row.decide {
        detail.push_str(&format!(" {sep} decide"));
    }
    format!(
        "    {}  {}  {}\n",
        styler.cyan(&pad_right(&reference, 30)),
        pad_right(&row.text, 40),
        styler.dim(&detail),
    )
}

fn lane_json(interval: Option<u16>) -> serde_json::Value {
    match interval {
        Some(days) => json!(days),
        None => json!(false),
    }
}

fn json_list(report: &ListReport) -> serde_json::Value {
    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "date": report.date,
        "config": {
            "interval": report.interval,
            "pending_interval": lane_json(report.pending_interval),
            "next_interval": lane_json(report.next_interval),
            "rotten_daily_budget": report.budget,
            "decay": {
                "enabled": report.decay_enabled,
                "keeps": report.decay_keeps,
                "enter": report.decay_enter,
                "active_from": report.decay_active_from,
                "active": report.decay_active,
            },
        },
        "counts": {
            "due": report.counts.due,
            "new": report.counts.new,
            "resurfaced": report.counts.resurfaced,
            "rotten": report.counts.rotten,
            "fresh": report.counts.fresh,
            "pending_due": report.counts.pending_due,
            "next_due": report.counts.next_due,
            "projects_due": report.counts.projects_due,
            "by_tier": {
                "new": report.counts.by_tier.new,
                "projects": report.counts.by_tier.projects,
                "pending": report.counts.by_tier.pending,
                "next": report.counts.by_tier.next,
                "returned": report.counts.by_tier.returned,
                "rotten": report.counts.by_tier.rotten,
            },
            "walk": report.counts.walk,
            "decide": report.counts.decide,
            "refreshed_today": report.counts.refreshed_today,
            "upkeep_today": report.counts.upkeep_today,
            "budget": report.counts.budget,
            "budget_met": report.counts.budget_met,
        },
        "queue": report.rows.iter().map(|row| json!({
            "rank": row.rank,
            "tier": row.tier,
            "lane": row.lane,
            "state": row.state.map(|state| state.as_str()),
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
            "scheduled": row.scheduled,
            "keeps": row.keeps,
            "decide": row.decide,
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
