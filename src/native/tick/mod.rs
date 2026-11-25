//! One ordered pass for Bob's frequent background work, without an outer vault lock.
mod adapters;
mod catalog;
mod config;
mod gates;
mod lock;
mod receipt;
mod render;
mod run;
mod schedule;
mod signals;
mod state;

use crate::native::env as bob_env;
use chrono::{DateTime, Utc};
use clap::{Arg, ArgAction, Command};
use schedule::Trigger;
use std::{ffi::OsString, path::PathBuf};

pub(super) struct Options {
    vault: PathBuf,
    names: Vec<String>,
    trigger: Trigger,
    dry_run: bool,
    status: bool,
    json: bool,
    quiet: bool,
}
const AFTER_HELP: &str = "Steps, in run order (default cadence · ceiling):
  snooze    bob snooze wake     5m · 2m    due later-today tasks into today's ledger
  gcal      bob gcal sync       1h · 10m   today; yesterday too on the day's first run
  gmail     bob gmail sync      1h · 20m   file bob-labelled threads, watch replies
  gkeep     bob gkeep pull      manual     drain the Keep inbox
  refs      bob ref scan -w     15m · 30m  when lib/ or xlib/ changed, or every 6h
  projects  bob projects sync   15m · 10m  also after any intake step ran
  tasks     bob task reconcile  15m · 10m  also after any intake step ran

Examples:
  bob tick              Run every due step (what cron runs)
  bob tick -d           Show what is due, and why
  bob tick -s           Show the status board
  bob tick -a           Run everything now (the morning catch-up)
  bob tick gmail gcal   Run Gmail and Calendar now, in catalog order
  bob tick -f json      Print the run receipt

Configure tick.steps with intervals (5m to 7d), manual, or off.
SIGINT, SIGTERM, and SIGHUP stop the child group; SIGKILL can orphan it.
For vault Git sync, use `bob vault-sync`.";
pub(crate) fn completion_descriptor() -> Command {
    Command::new("bob tick")
        .about("Run Bob's due background work in one ordered pass: snooze → gcal → gmail → gkeep → refs → projects → tasks. A step runs when its tick.steps interval has passed since it last ran on this host, so cron can call bob tick every few minutes while each step keeps its own pace. bob tick never holds the shared vault lock across steps.")
        .disable_help_flag(true)
        .after_help(AFTER_HELP)
        .arg(Arg::new("step").value_name("STEP").num_args(0..).value_parser(["snooze", "gcal", "gmail", "gkeep", "refs", "projects", "tasks"]).help("Run only these steps now (due times skipped, safety gates kept)"))
        .arg(Arg::new("all").short('a').long("all").action(ArgAction::SetTrue).conflicts_with_all(["step", "status"]).help("Run every step that is not off now, manual steps included").display_order(0))
        .arg(Arg::new("bob-dir").short('b').long("bob-dir").value_name("DIR").value_parser(clap::value_parser!(PathBuf)).help("Bob vault root; defaults to BOB_DIR or ~/bob").display_order(1))
        .arg(Arg::new("dry-run").short('d').long("dry-run").action(ArgAction::SetTrue).conflicts_with("status").help("Show what would run and why; run nothing, write nothing").display_order(2))
        .arg(Arg::new("format").short('f').long("format").value_name("FORMAT").value_parser(["human", "json"]).default_value("human").help("Output format").display_order(3))
        .arg(Arg::new("help").short('h').long("help").action(ArgAction::Help).help("Show help").display_order(4))
        .arg(Arg::new("quiet").short('q').long("quiet").action(ArgAction::SetTrue).help("Print only failures").display_order(5))
        .arg(Arg::new("status").short('s').long("status").action(ArgAction::SetTrue).help("Show each step's last result, last success, and next run").display_order(6))
}
pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let matches = match completion_descriptor().try_get_matches_from(
        std::iter::once(OsString::from("bob tick")).chain(args),
    ) {
        Ok(m) => m,
        Err(e) => {
            let code = e.exit_code();
            let _ = e.print();
            return code;
        }
    };
    let names: Vec<String> = matches
        .get_many::<String>("step")
        .map(|v| v.cloned().collect())
        .unwrap_or_default();
    let trigger = if matches.get_flag("all") {
        Trigger::All
    } else if names.is_empty() || matches.get_flag("status") {
        Trigger::Scheduled
    } else {
        Trigger::Named
    };
    let options = Options {
        vault: matches
            .get_one::<PathBuf>("bob-dir")
            .cloned()
            .unwrap_or_else(bob_env::bob_dir),
        names,
        trigger,
        dry_run: matches.get_flag("dry-run"),
        status: matches.get_flag("status"),
        json: matches
            .get_one::<String>("format")
            .is_some_and(|s| s == "json"),
        quiet: matches.get_flag("quiet"),
    };
    match run::execute(options) {
        Ok(code) => code,
        Err((code, message)) => {
            eprintln!("bob tick: {message}");
            code
        }
    }
}
pub(super) fn now() -> Result<DateTime<Utc>, String> {
    match bob_env::var("BOB_TICK_NOW") {
        Ok(s) => s
            .parse()
            .map_err(|e| format!("BOB_TICK_NOW: expected RFC 3339 ({e})")),
        Err(_) => Ok(Utc::now()),
    }
}

/// The writer pin and host identity used by automatic snooze sweeps. Keep
/// these on the same source as `bob tick`: `tick.writer`, then
/// `gcal.writer`, and the tick journal's hostname slug.
pub(crate) fn snooze_writer() -> Result<(String, Option<String>), String> {
    let config = config::Config::load()?;
    Ok((crate::native::pref_journal::host_label(), config.writer))
}
